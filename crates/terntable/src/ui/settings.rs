//! The settings page, laid out like Spotify's: sections, and in each a card of rows with a label
//! and a one-line description on the left and the control on the right (a switch, a segmented
//! control, a dropdown, a row of steps). On the compact layout, rows with wide controls stack.
//! It is a page of its own (in place of the content), not a panel beside it.
//!
//! The keys below spell the node paths the dropdowns hang from and the name field is focused by
//! (`main.app.settings.page.<section>.card.<row>.control...`): keep them in step with
//! `Field::node_id` and the `*_NODE` constants.

use tern_sdk::ui::{self, Node, html};

use super::{Tap, div, icon, text};
use crate::{
	app::{
		Field, Msg, State,
		rules::{SLEEP_MINUTES, Sleep, SleepChoice},
		settings::{BALANCE_STEPS, CACHE_SIZES, Output, Pref, cache_label},
		system::{self, Folder, Menu},
	},
	audio,
	library::Conn,
	ui::icons::{Glyph, Icons},
};

/// Keys and what they do, as shown (the bindings live in the player loop).
const SHORTCUTS: [(&str, &str); 14] = [
	("Space", "Play / pause"),
	("N / P", "Next / previous track"),
	("← / →", "Seek 10 s"),
	("+ / −", "Volume"),
	("0", "Mute"),
	("S / R", "Shuffle / repeat"),
	("/", "Search"),
	("G / L", "Home / Liked Songs"),
	("↑ ↓ / J K", "Move the selection"),
	("Q", "Queue"),
	("Y", "Lyrics"),
	("E", "Equalizer"),
	(",", "Settings"),
	("Enter", "Play the selected song"),
];

/// Where the dropdowns' buttons sit in the view (the menus hang from them).
const SLEEP_NODE: &str = "main.app.settings.page.playback.card.sleep.control.select";
const OUTPUT_NODE: &str = "main.app.settings.page.audio.card.output.control.select";

pub fn panel(state: &State) -> Node<Msg> {
	div("tt-panel tt-settings")
		.key("settings")
		.child(
			div("tt-set-page")
				.key("page")
				.child(text("tt-set-head", "Settings"))
				.child(section("account", "Spotify account", account(state)))
				.child(section("playback", "Playback", playback(state)))
				.child(section("audio", "Audio", audio_rows(state)))
				.child(section("device", "Player", device(state)))
				.child(section("display", "Display", display(state)))
				.child(section("sources", "Data sources", sources(state)))
				.child(section("keys", "Keyboard", vec![shortcuts()]))
				.child(section("about", "About", about(state))),
		)
		.into()
}

fn section(key: &str, title: &str, rows: Vec<Node<Msg>>) -> Node<Msg> {
	div("tt-set-sec").key(key).child(text("tt-set-title", title)).child(div("tt-set-card").key("card").children(rows)).into()
}

/// A row: the label and its description, and the control. `wide` rows stack on the compact layout.
fn row(key: &str, label: &str, description: impl Into<String>, control: Node<Msg>, wide: bool) -> Node<Msg> {
	div(if wide { "tt-set-row tt-wide" } else { "tt-set-row" })
		.key(key)
		.child(div("tt-set-text").key("text").child(text("tt-set-label", label)).child(text("tt-set-desc tt-dim", description)))
		.child(div("tt-set-control").key("control").child(control))
		.into()
}

fn note(key: &str, words: impl Into<String>) -> Node<Msg> {
	div("tt-set-note").key(key).child(html::span(words)).into()
}

fn switch(on: bool, label: &str, msg: Msg) -> Node<Msg> {
	html::el("button")
		.class(if on { "tt-sw tt-on" } else { "tt-sw" })
		.key("switch")
		.title(label)
		.aria(label)
		.child(html::span("").class("tt-sw-knob"))
		.on_click(msg)
		.into()
}

/// One of a few choices; the current one is lit.
fn segmented(options: Vec<(String, bool, Msg)>) -> Node<Msg> {
	let mut seg = div("tt-seg").key("seg");
	for (i, (label, current, msg)) in options.into_iter().enumerate() {
		seg = seg.child(
			html::el("button")
				.class(if current { "tt-seg-opt tt-cur" } else { "tt-seg-opt" })
				.key(format!("o{i}"))
				.text(label)
				.on_click(msg),
		);
	}
	seg.into()
}

/// A dropdown's button, showing the choice made; its menu is [`dropdown`].
fn select(label: &str, menu: Menu) -> Node<Msg> {
	html::el("button")
		.class("tt-select")
		.key("select")
		.title(label.to_owned())
		.child(html::span(label.to_owned()))
		.child(html::span("").class("tt-chev"))
		.on_click(Msg::Dropdown(menu))
		.into()
}

/// A row of clickable steps, `lit(i)` saying whether step `i` is filled and whether it is the middle mark.
fn steps(count: u32, lit: impl Fn(u32) -> (bool, bool), pick: impl Fn(u32) -> Msg) -> Node<Msg> {
	let mut cells = div("tt-cells").key("cells");
	for i in 0..count {
		let (on, middle) = lit(i);
		let class = match (on, middle) {
			(true, _) => "tt-pcell tt-lit",
			(false, true) => "tt-pcell tt-zero",
			(false, false) => "tt-pcell",
		};
		cells = cells.child(html::span("").class(class).key(i).on_click(pick(i)));
	}
	cells.into()
}

fn pill(label: &str, title: &str, msg: Msg) -> Node<Msg> {
	html::el("button").class("tt-pill tt-quiet").key("pill").title(title.to_owned()).text(label.to_owned()).on_click(msg).into()
}

fn link(label: &str, title: &str, msg: Msg) -> Node<Msg> {
	html::el("button").class("tt-link").key(label.to_owned()).title(title.to_owned()).text(label.to_owned()).on_click(msg).into()
}

// ---------------------------------------------------------------------- account

fn account(state: &State) -> Vec<Node<Msg>> {
	let (words, control) = if state.signed_in {
		(
			"Signed in: terntable plays on this Mac".to_owned(),
			pill("Sign out", "Forget the player's sign-in on this computer", Msg::SignOut),
		)
	} else {
		("Not signed in".to_owned(), pill("Sign in", "Sign in to Spotify in your browser", Msg::SignIn))
	};
	let mut rows = vec![row("account", "Account", words, control, false)];
	let (words, control) = match &state.library.conn {
		Conn::Ready => {
			let who = state.library.me.as_ref().and_then(|u| u.display_name.clone()).unwrap_or_else(|| "your account".into());
			(format!("Connected as {who}"), pill("Disconnect", "Forget the library's sign-in on this computer", Msg::DisconnectSpotify))
		},
		Conn::NeedsLogin | Conn::Failed(_) => (
			"Not connected: playlists and liked songs are hidden".to_owned(),
			pill("Connect", "Sign in with your Spotify app in the browser", Msg::ConnectSpotify),
		),
		Conn::LoggingIn(_) => ("Waiting for you to finish in the browser…".to_owned(), html::span("").into()),
		Conn::NeedsClientId => ("Finish setup to connect your library".to_owned(), html::span("").into()),
	};
	rows.push(row("library", "Library", words, control, false));
	let name = if state.device_name.is_empty() { "terntable" } else { state.device_name.as_str() };
	let status = if !state.signed_in {
		"Starts once you sign in".to_owned()
	} else if !state.connected {
		state.notice.clone().unwrap_or_else(|| "Starting…".to_owned())
	} else if state.active {
		format!("Playing here as “{name}”")
	} else {
		format!("Ready as “{name}”")
	};
	rows.push(row("player", "Player", status, html::span("").into(), false));
	rows
}

// --------------------------------------------------------------------- playback

fn playback(state: &State) -> Vec<Node<Msg>> {
	let p = &state.prefs.playback;
	let sleep = match state.rules.sleep {
		None => "Off".to_owned(),
		Some(Sleep::At { minutes, .. }) => format!("{minutes} min"),
		Some(Sleep::EndOfTrack) => "End of song".to_owned(),
	};
	vec![
		row(
			"autoplay",
			"Autoplay",
			"Keep playing similar songs when your music ends; off pauses there",
			switch(p.autoplay, "Autoplay", Msg::Pref(Pref::Autoplay(!p.autoplay))),
			false,
		),
		row(
			"explicit",
			"Allow explicit content",
			"Songs marked E are skipped when this is off",
			switch(p.explicit, "Allow explicit content", Msg::Pref(Pref::Explicit(!p.explicit))),
			false,
		),
		row(
			"sleep",
			"Sleep timer",
			"Pause after a while, or when the song ends; it counts down in the bar",
			select(&sleep, Menu::Sleep),
			true,
		),
	]
}

// ------------------------------------------------------------------------ audio

fn audio_rows(state: &State) -> Vec<Node<Msg>> {
	let mut rows = Vec::new();
	let a = &state.prefs.audio;
	let levels = audio::Normalize::ALL.map(|n| (n.label().to_owned(), a.normalize == n, Msg::Pref(Pref::Normalize(n)))).into();
	rows.push(row("normalize", "Normalize volume", "Plays quiet and loud songs at about the same loudness", segmented(levels), true));
	rows.push(row(
		"mono",
		"Mono audio",
		"Plays the left and right channels as one",
		switch(a.mono, "Mono audio", Msg::Pref(Pref::Mono(!a.mono))),
		false,
	));
	let cells = u32::try_from(2 * BALANCE_STEPS + 1).unwrap_or(21);
	let mid = i32::from(BALANCE_STEPS);
	let balance = i32::from(a.balance);
	let bar = steps(
		cells,
		move |i| {
			let at = i32::try_from(i).unwrap_or(0) - mid;
			let lit = (balance > 0 && at > 0 && at <= balance) || (balance < 0 && at < 0 && at >= balance);
			(lit, at == 0)
		},
		move |i| Msg::Pref(Pref::Balance(i8::try_from(i32::try_from(i).unwrap_or(0) - mid).unwrap_or(0))),
	);
	let value = match a.balance {
		0 => "Center".to_owned(),
		b if b < 0 => format!("{}% left", i32::from(-b) * 10),
		b => format!("{}% right", i32::from(b) * 10),
	};
	let control = div("tt-set-field").child(bar).child(text("tt-set-value", value));
	rows.push(row("balance", "Balance", "Turn one side down", control.into(), true));
	if audio::CAN_PICK_OUTPUT {
		rows.push(row("output", "Output device", output_description(state), select(&output_label(state), Menu::Output), true));
	} else {
		rows.push(note("output", "Audio plays on the system default output device."));
	}
	rows.push(note("quality", "Music streams at 320 kbps."));
	rows
}

/// What the output dropdown's button says: the device in use, or the system's default.
fn output_label(state: &State) -> String {
	match &state.prefs.audio.output {
		Some(o) if state.sys.outputs.iter().any(|d| d.uid == o.uid) => o.name.clone(),
		Some(o) => format!("{} (not connected)", o.name),
		None => "System default".to_owned(),
	}
}

fn output_description(state: &State) -> String {
	match (&state.prefs.audio.output, &state.sys.default_output) {
		(Some(o), _) if !state.sys.outputs.iter().any(|d| d.uid == o.uid) => {
			format!("“{}” isn't connected: playing on the system default for now", o.name)
		},
		(None, Some(default)) => format!("Follows the system: now {default}"),
		_ => "Where the sound plays; follows the system's default unless you pick one".to_owned(),
	}
}

// ----------------------------------------------------------------------- device

fn device(state: &State) -> Vec<Node<Msg>> {
	let d = &state.prefs.device;
	let sys = &state.sys;
	let mut rows = Vec::new();
	let name_control: Node<Msg> = if let Some(fixed) = &sys.name_override {
		text("tt-set-value", format!("{fixed} (set by --device-name)"))
	} else {
		let focused = state.focus == Some(Field::DeviceName);
		let input = ui::input()
			.key(Field::DeviceName.id())
			.text(state.device_input.clone())
			.placeholder(if focused { String::new() } else { "terntable".to_owned() })
			.on_focus(Msg::Focus(Some(Field::DeviceName)))
			.on_edit(|e| Msg::Edit { from: e.from, to: e.to, text: e.text.clone() });
		let mut field = div("tt-set-field").key("field").child(input);
		if state.device_input.trim() != d.name {
			field = field.child(pill("Save", "Use this name (the player restarts to take it)", Msg::SaveDeviceName));
		}
		field.into()
	};
	rows.push(row(
		"name",
		"Device name",
		"How this player shows up in Spotify's device list; changing it restarts the player",
		name_control,
		true,
	));
	let volume = u32::from(d.volume);
	let bar = steps(20, move |i| (i * 5 < volume, false), |i| Msg::Pref(Pref::StartupVolume(u8::try_from((i + 1) * 5).unwrap_or(100))));
	rows.push(row(
		"volume",
		"Startup volume",
		"The volume the player starts at",
		div("tt-set-field").child(bar).child(text("tt-set-value", format!("{}%", d.volume))).into(),
		true,
	));
	let sizes = CACHE_SIZES.map(|mb| (cache_label(mb), d.cache_mb == mb, Msg::Pref(Pref::CacheMb(mb)))).into();
	rows.push(row(
		"cache",
		"Playback cache",
		"How much music the player keeps on disk; changing it restarts the player",
		segmented(sizes),
		true,
	));
	let used = sys.cache_bytes.map_or_else(|| "Measuring…".to_owned(), |b| format!("{} in the cache", system::size_label(b)));
	let clear: Node<Msg> = if sys.clearing_cache {
		text("tt-set-value", "Clearing…")
	} else {
		pill("Clear cache", "Delete the saved songs (the player restarts)", Msg::ClearCache)
	};
	rows.push(row("clear", "Clear cache", used, clear, false));
	if sys.restart_needed(&d.name, d.cache_mb) {
		let words = "Playback stops for a few seconds while the player starts again with the new settings";
		rows.push(row("restart", "Restart the player", words, pill("Restart", "Restart the player now", Msg::RestartPlayer), false));
	}
	rows
}

// ---------------------------------------------------------------------- display

fn display(state: &State) -> Vec<Node<Msg>> {
	let d = &state.prefs.display;
	vec![
		row(
			"nowplaying",
			"Show the now-playing view",
			"Opens beside your music when there is room",
			switch(state.now_playing_view, "Show the now-playing view", Msg::Pref(Pref::NowPlayingView(!state.now_playing_view))),
			false,
		),
		row(
			"compact",
			"Compact density",
			"Tighter rows in lists and the queue",
			switch(d.compact, "Compact density", Msg::Pref(Pref::Compact(!d.compact))),
			false,
		),
		row(
			"plays",
			"Show play counts",
			"Plays beside songs on albums and artist pages",
			switch(d.play_counts, "Show play counts", Msg::Pref(Pref::PlayCounts(!d.play_counts))),
			false,
		),
	]
}

// ---------------------------------------------------------------------- sources

fn sources(state: &State) -> Vec<Node<Msg>> {
	let s = &state.prefs.sources;
	vec![
		row(
			"lyrics",
			"Lyrics from LRCLIB",
			"Sends a song's title, first artist and length to lrclib.net while lyrics are open",
			switch(s.lyrics, "Lyrics from LRCLIB", Msg::Pref(Pref::Lyrics(!s.lyrics))),
			false,
		),
		row(
			"wikipedia",
			"Artist info from Wikipedia",
			"Looks an artist up on Wikidata and Wikipedia for the About card",
			switch(s.wikipedia, "Artist info from Wikipedia", Msg::Pref(Pref::Wikipedia(!s.wikipedia))),
			false,
		),
		row(
			"webplayer",
			"Spotify web-player data",
			"Listeners, photos, credits, play counts and others' playlists, from the public web player; no account",
			switch(s.web_player, "Spotify web-player data", Msg::Pref(Pref::WebPlayer(!s.web_player))),
			false,
		),
	]
}

// --------------------------------------------------------------------- keyboard

fn shortcuts() -> Node<Msg> {
	let mut list = div("tt-keys tt-set-keys").key("list");
	for (keys, what) in SHORTCUTS {
		list = list.child(div("tt-key-row").child(html::span(keys).class("tt-kbd")).child(html::span(what).class("tt-dim")));
	}
	list.into()
}

// ------------------------------------------------------------------------ about

fn about(state: &State) -> Vec<Node<Msg>> {
	let sys = &state.sys;
	let memory = sys.app_kb.map_or_else(|| "Not measured yet".to_owned(), system::memory_label);
	let links = div("tt-set-links")
		.key("links")
		.child(link("Settings", "Open the folder with settings and saved sign-in", Msg::OpenFolder(Folder::Config)))
		.child(link("Cache", "Open the folder with the playback cache", Msg::OpenFolder(Folder::Cache)));
	vec![
		row("version", "terntable", format!("Version {}", env!("CARGO_PKG_VERSION")), html::span("").into(), false),
		row("memory", "Memory", memory, html::span("").into(), false),
		row("diagnostics", "Folders", "Settings and cached music", links.into(), true),
	]
}

// -------------------------------------------------------------------- dropdowns

type Menu3 = (&'static str, &'static str, Vec<(String, bool, Msg)>);

/// The open dropdown's anchor, title and choices (each: label, whether it is the current one, what picking sends).
fn open_menu(state: &State) -> Option<Menu3> {
	use crate::app::Panel;
	let menu = state.sys.menu.filter(|_| state.panel == Some(Panel::Settings))?;
	Some(match menu {
		Menu::Sleep => (SLEEP_NODE, "Sleep timer", sleep_items(state)),
		Menu::Output => (OUTPUT_NODE, "Output device", output_items(state)),
	})
}

/// The menu of the open dropdown, hanging from its button.
pub fn dropdown(state: &State, icons: &Icons) -> Option<Node<Msg>> {
	let (anchor, title, items) = open_menu(state)?;
	let mut list = div("tt-menu").key("menu").child(text("tt-menu-head", title));
	for (i, (label, current, msg)) in items.into_iter().enumerate() {
		let mut item = html::el("button")
			.class(if current { "tt-menu-item tt-on" } else { "tt-menu-item" })
			.key(format!("i{i}"))
			.child(html::span(label).class("tt-trunc"))
			.on_click(msg.clone());
		if current {
			item = item.child(icon(icons, Glyph::Check, 16.0, Tap::Msg(msg, None)));
		}
		list = list.child(item);
	}
	Some(
		ui::overlay()
			.key("settings-menu")
			.role("tt.menu")
			.anchor(ui::Anchor::Node { id: anchor.to_owned(), above: false })
			.size(ui::OverlaySize::Sm)
			.child(list)
			.into(),
	)
}

fn sleep_items(state: &State) -> Vec<(String, bool, Msg)> {
	let now = state.rules.sleep;
	let mut items = vec![("Off".to_owned(), now.is_none(), Msg::Sleep(SleepChoice::Off))];
	for minutes in SLEEP_MINUTES {
		let current = matches!(now, Some(Sleep::At { minutes: m, .. }) if m == minutes);
		items.push((format!("{minutes} minutes"), current, Msg::Sleep(SleepChoice::Minutes(minutes))));
	}
	items.push(("End of song".to_owned(), now == Some(Sleep::EndOfTrack), Msg::Sleep(SleepChoice::EndOfTrack)));
	items
}

fn output_items(state: &State) -> Vec<(String, bool, Msg)> {
	let chosen = state.prefs.audio.output.as_ref();
	let default = state.sys.default_output.as_deref().map_or_else(|| "System default".to_owned(), |d| format!("System default ({d})"));
	let mut items = vec![(default, chosen.is_none(), Msg::Pref(Pref::Output(None)))];
	for d in &state.sys.outputs {
		let current = chosen.is_some_and(|o| o.uid == d.uid);
		items.push((d.name.clone(), current, Msg::Pref(Pref::Output(Some(Output { uid: d.uid.clone(), name: d.name.clone() })))));
	}
	items
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{app::system::Running, audio::OutputDevice};

	fn state() -> State {
		State { rows: 50, cols: 200, cell_w: 9.0, cell_h: 18.0, panel: Some(crate::app::Panel::Settings), ..State::default() }
	}

	#[test]
	fn the_output_dropdown_lists_the_default_and_each_device_and_marks_the_choice() {
		let mut s = state();
		s.sys.outputs =
			vec![OutputDevice { uid: "a".into(), name: "Speakers".into() }, OutputDevice { uid: "b".into(), name: "AirPods".into() }];
		s.sys.default_output = Some("Speakers".into());
		let items = output_items(&s);
		assert_eq!(items.len(), 3);
		assert!(items[0].1 && items[0].0.contains("System default (Speakers)"));
		s.prefs.audio.output = Some(Output { uid: "b".into(), name: "AirPods".into() });
		let items = output_items(&s);
		assert!(!items[0].1 && items[2].1);
		assert_eq!(output_label(&s), "AirPods");
		// Picked and then unplugged: still named, said to be away.
		s.sys.outputs.truncate(1);
		assert_eq!(output_label(&s), "AirPods (not connected)");
		assert!(output_description(&s).contains("isn't connected"));
	}

	#[test]
	fn the_page_builds_in_every_state_and_the_dropdown_only_while_it_is_open() {
		let mut s = state();
		let _ = panel(&s);
		assert!(open_menu(&s).is_none());
		s.sys.running = Some(Running { name: "terntable".into(), cache_mb: 100 });
		s.prefs.device.cache_mb = 500;
		assert!(s.sys.restart_needed(&s.prefs.device.name, s.prefs.device.cache_mb));
		let _ = panel(&s);
		s.sys.menu = Some(Menu::Sleep);
		assert!(open_menu(&s).is_some_and(|m| m.2.len() == 6));
		s.panel = None;
		assert!(open_menu(&s).is_none(), "gone with the page");
	}
}
