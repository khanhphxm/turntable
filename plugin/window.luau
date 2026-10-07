--!strict
-- terntable, window half: palette commands and global keys for the player,
-- its tab icon, and a status line segment with what's playing.
--
-- The player is the `terntable` program running in a pane. It draws its own
-- UI over the Surface Protocol; this half only places it and sends it
-- actions (`cx.session:event`), and reads the pane title it sets
-- ("Song · Artist", "· Paused" when paused).

-- The command that starts the player; `tern.kv` key "command" overrides it
-- (for a binary outside PATH).
local function player_command(): string
	local custom = tern.kv.get("command")
	if type(custom) == "string" and custom ~= "" then
		return custom
	end
	return "terntable"
end

-- Panes this plugin launched, so a player that exited (killed, crashed: its
-- program then reads as the shell) is still recognised and cleaned up.
local function launched(): { [string]: boolean }
	local v = tern.kv.get("panes")
	return if type(v) == "table" then v else {}
end

-- Player panes wear terntable's icon instead of the terminal glyph, wherever
-- Tern draws a pane's face (`span.tn-face[data-pane]`). Plugins can't set a
-- pane's icon, but their sheets reach Tern's chrome.
local icon_panes: { [string]: true } = {}
local icon_key = ""

local function refresh_icons()
	local ids = {}
	for id in icon_panes do
		table.insert(ids, id)
	end
	table.sort(ids)
	local key = table.concat(ids, ",")
	if key == icon_key then
		return
	end
	icon_key = key
	if #ids == 0 then
		tern.css("icon", "")
		return
	end
	local faces, glyphs = {}, {}
	for _, id in ids do
		table.insert(faces, `span.tn-face[data-pane='{id}']`)
		table.insert(glyphs, `span.tn-face[data-pane='{id}'] > svg`)
	end
	tern.css(
		"icon",
		table.concat(faces, ", ")
			.. " { background: url(icon.svg) center / 86% no-repeat; color: transparent !important; }\n"
			.. table.concat(glyphs, ", ")
			.. " { visibility: hidden; }\n"
	)
end

local function mark_icon(pane: number, on: boolean)
	local key = tostring(pane)
	if (icon_panes[key] == true) ~= on then
		if on then
			icon_panes[key] = true
		else
			icon_panes[key] = nil
		end
		refresh_icons()
	end
end

local function remember(pane: number?)
	if pane == nil then
		return
	end
	local panes = launched()
	panes[tostring(pane)] = true
	tern.kv.set("panes", panes)
	mark_icon(pane, true)
end

-- The player's live pane in this window, if it runs. A pane counts when this
-- plugin launched it (the registry) or its program is terntable (started by
-- hand); `program` alone isn't enough, since a pane launched through a shell
-- or `env` reports the shell. Exited players are closed and forgotten.
local function find(cx: WindowCx): PaneInfo?
	local panes = launched()
	local live: PaneInfo? = nil
	local changed = false
	local present: { [string]: boolean } = {}
	for _, pane in cx.session:panes() do
		local key = tostring(pane.pane)
		local ours = panes[key] == true
		present[key] = true
		if pane.exited ~= nil then
			if ours then
				cx.layout:close(pane.pane)
				panes[key] = nil
				changed = true
			end
		elseif ours or pane.program == "terntable" then
			live = live or pane
		end
	end
	-- Forget panes that no longer exist (closed by hand).
	for key in panes do
		if not present[key] then
			panes[key] = nil
			changed = true
		end
	end
	if changed then
		tern.kv.set("panes", panes)
	end
	return live
end

-- Brings the player out of the deck if it was parked there.
local function reveal(cx: WindowCx, pane: PaneInfo)
	if pane.parked then
		cx.layout:unpark(pane.pane)
	end
	cx.layout:focus(pane.pane)
end

local function open_tab(cx: WindowCx)
	local pane = find(cx)
	if pane then
		reveal(cx, pane)
		return
	end
	local id = cx.layout:new_tab({ command = player_command() })
	if id == nil then
		cx:toast("error", "terntable didn't start", "Is `terntable` on your PATH? Set tern.kv \"command\" to its path.")
	end
	remember(id)
end

-- Sends a remote-control action to the player, if it runs.
local function send(act: string): (cx: WindowCx) -> ()
	return function(cx: WindowCx)
		local pane = find(cx)
		if pane == nil then
			cx:toast("info", "terntable isn't running", nil)
			return
		end
		cx.session:event(pane.pane, { ev = "action", id = "main", act = act })
	end
end

local mac = tern.runtime.os == "macos"
local function chord(mac_keys: string, other: string): { string }
	return { if mac then mac_keys else other }
end

tern.command({ id = "open", title = "terntable: Open", icon = "wave", keys = chord("cmd+alt+m", "ctrl+alt+m"), run = open_tab })
tern.command({ id = "toggle", title = "terntable: Play / pause", icon = "play", keys = chord("cmd+alt+space", "ctrl+alt+space"), run = send("tt.toggle") })
tern.command({ id = "next", title = "terntable: Next track", icon = "frame-next", keys = chord("cmd+alt+right", "ctrl+alt+right"), run = send("tt.next") })
tern.command({ id = "prev", title = "terntable: Previous track", icon = "frame-prev", keys = chord("cmd+alt+left", "ctrl+alt+left"), run = send("tt.prev") })
tern.command({ id = "volume_up", title = "terntable: Volume up", icon = "plus", keys = chord("cmd+alt+up", "ctrl+alt+up"), run = send("tt.volume_up") })
tern.command({ id = "volume_down", title = "terntable: Volume down", icon = "minus", keys = chord("cmd+alt+down", "ctrl+alt+down"), run = send("tt.volume_down") })
tern.command({ id = "shuffle", title = "terntable: Toggle shuffle", icon = "swap", run = send("tt.shuffle") })
tern.command({ id = "repeat", title = "terntable: Cycle repeat", icon = "repeat", run = send("tt.repeat") })
tern.command({ id = "search", title = "terntable: Search", icon = "search", run = function(cx: WindowCx)
	open_tab(cx)
	send("tt.search")(cx)
end })

-- ---------------------------------------------------------------- status

-- What the player's pane title says, while it runs.
local now_playing: string? = nil
local player_pane: number? = nil

local function on_title(ev: TitleEvent, cx: WindowCx)
	local pane = cx.session:pane(ev.pane)
	-- The registry catches players launched through a shell (their program
	-- reads as the shell); `program` catches one started by hand.
	if pane == nil or (launched()[tostring(ev.pane)] ~= true and pane.program ~= "terntable") then
		return
	end
	player_pane = ev.pane
	mark_icon(ev.pane, true)
	local title = ev.title
	if title == "" or title == "terntable" then
		now_playing = nil
	else
		now_playing = title
	end
	tern.chrome.refresh()
end

tern.on("title", on_title)
tern.on("pane_closed", function(ev: any, _cx: WindowCx)
	mark_icon(ev.pane, false)
	if ev.pane == player_pane then
		player_pane, now_playing = nil, nil
		tern.chrome.refresh()
	end
end)
-- Players already running when the window starts or the plugin reloads
-- (`window_start` doesn't run again on a reload: the first focus change
-- after one scans too).
local scanned = false
local function scan(cx: WindowCx)
	scanned = true
	for _, pane in cx.session:panes() do
		if pane.exited == nil and (launched()[tostring(pane.pane)] == true or pane.program == "terntable") then
			mark_icon(pane.pane, true)
		end
	end
end
tern.on("window_start", scan)
-- After a reload `window_start` doesn't run again: a timer brings a cx.
tern.timer(0, function(cx: WindowCx?)
	if cx and not scanned then
		scan(cx)
	end
end)

local PAUSED = " · Paused"

tern.chrome.status(function(pane: StatusPaneInfo): { StatusSegment }?
	local text = now_playing
	if text == nil or (pane :: any).pane == player_pane then
		-- Nothing playing, or the player itself has focus (it shows all of this).
		return nil
	end
	local paused = string.sub(text, -#PAUSED) == PAUSED
	if paused then
		text = string.sub(text, 1, -#PAUSED - 1)
	end
	-- Cut on a character boundary: titles are UTF-8 (accents, CJK, emoji).
	-- More than 48 characters: keep 47 and an ellipsis.
	local over = utf8.offset(text, 49)
	if over ~= nil and over <= #text then
		text = string.sub(text, 1, (utf8.offset(text, 48) :: number) - 1) .. "…"
	end
	return {
		{
			text = text,
			icon = if paused then "pause" else "play",
			tone = if paused then "muted" else nil,
			command = "plugin.terntable.toggle",
		},
	}
end)
