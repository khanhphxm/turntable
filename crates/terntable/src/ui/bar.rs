//! The player along the bottom: the full now-playing bar, or in narrow panes
//! a mini player and a tab bar, as on Spotify's phone app.

use tern_sdk::ui::{Node, html};

use super::{Tap, art, button, devices, div, icon, like_button, seek, text, transport, volume};
use crate::{
	app::{Layout, Msg, Panel, State},
	library::Route,
	ui::icons::{Glyph, Icons},
};

pub fn bar(state: &State, icons: &Icons) -> Node<Msg> {
	let mut np = div("tt-np");
	if !state.item.uri.is_empty() {
		let href = super::spotify_href(&state.item.uri);
		let tap = href.clone().map_or(Tap::NONE, Tap::Href);
		let mut cover = html::div().class("tt-np-cover").child(art(state, state.cover_url(), "tt-art-56", state.item.parent_name(), tap));
		if let Some(href) = href {
			cover = cover.href(href).title("Open Spotify");
		}
		np = np.child(cover).child(
			div("tt-np-text")
				.child(text("tt-np-title tt-trunc", state.item.name()))
				.child(text("tt-np-sub tt-trunc tt-dim", state.item.creators())),
		);
		if let Some(like) = like_button(state, icons, 18.0) {
			np = np.child(like);
		}
	} else {
		np = np.child(div("tt-np-text").child(text("tt-np-sub tt-dim", idle_hint(state))));
	}

	// Until the player is signed in nothing can play: show the controls, inert.
	let ready = state.logged_in || state.remote().is_some();
	let center = div(if ready { "tt-center" } else { "tt-center tt-inert" }).child(transport(state, icons, false)).child(seek(state));

	let mut right = div("tt-right").key("right");
	if let Some(chip) = sleep_chip(state, icons) {
		right = right.child(chip);
	}
	right = right
		.child(panel_button(state, icons, Glyph::NowPlaying, "Now playing view", Panel::NowPlaying))
		.child(panel_button(state, icons, Glyph::Mic, "Lyrics", Panel::Lyrics))
		.child(panel_button(state, icons, Glyph::Queue, "Queue", Panel::Queue))
		.child(panel_button(state, icons, Glyph::Sliders, "Equalizer", Panel::Eq))
		.child(panel_button(state, icons, Glyph::Gear, "Settings", Panel::Settings))
		.child(devices::button(state, icons, 18.0))
		.child(div(if ready { "" } else { "tt-inert" }).child(volume(state, icons)));

	let class = if state.layout() == Layout::Medium { "tt-bar tt-bar-m" } else { "tt-bar" };
	div(class).key("bar").child(np).child(center).child(right).into()
}

/// The compact layout's player: cover, title, like and play/pause; a click
/// opens the now-playing page. A thin line along the bottom shows progress.
pub fn mini(state: &State, icons: &Icons) -> Node<Msg> {
	let open = Msg::TogglePanel(Panel::NowPlaying);
	let mut info = html::div().class("tt-mp-info").title("Now playing");
	if state.item.uri.is_empty() {
		info = info.child(div("tt-np-text").child(text("tt-np-sub tt-dim", idle_hint(state))));
	} else {
		info = info.child(art(state, state.cover_url(), "tt-art-40", state.item.parent_name(), Tap::Msg(open.clone(), None))).child(
			div("tt-np-text")
				.child(text("tt-np-title tt-trunc", state.item.name()))
				.child(text("tt-np-sub tt-trunc tt-dim", state.item.creators())),
		);
	}
	let mut row = div("tt-mp-row").child(info.on_click(open));
	if let Some(like) = like_button(state, icons, 20.0) {
		row = row.child(like);
	}
	let (play, label) = if state.playing() { (Glyph::Pause, "Pause") } else { (Glyph::Play, "Play") };
	row = row.child(div(if state.logged_in || state.remote().is_some() { "" } else { "tt-inert" }).child(button(
		icons,
		play,
		label,
		Msg::TogglePlay,
		"tt-mp-play",
		22.0,
	)));
	div("tt-mp").key("mini").child(row).child(div("tt-mp-line").child(div("tt-fill"))).into()
}

/// Home, Search and Your Library, as on Spotify's phone app.
pub fn tabs(state: &State, icons: &Icons) -> Node<Msg> {
	let route = &state.library.route;
	let tab = |glyph: Glyph, label: &str, to: Route| -> Node<Msg> {
		let current = state.panel.is_none() && *route == to;
		let go = Msg::Navigate(to);
		html::div()
			.class(if current { "tt-tab tt-cur" } else { "tt-tab" })
			.child(icon(icons, glyph, 22.0, Tap::Msg(go.clone(), None)))
			.child(html::span(label))
			.on_click(go)
			.into()
	};
	div("tt-tabs")
		.key("tabs")
		.child(tab(Glyph::Home, "Home", Route::Home))
		.child(tab(Glyph::Search, "Search", Route::Search))
		.child(tab(Glyph::Library, "Your Library", Route::Library))
		.into()
}

fn panel_button(state: &State, icons: &Icons, glyph: Glyph, label: &str, panel: Panel) -> Node<Msg> {
	button(icons, glyph, label, Msg::TogglePanel(panel), if state.side_panel() == Some(panel) { "tt-on" } else { "" }, 18.0)
}

pub(super) fn idle_hint(state: &State) -> String {
	if let Some(notice) = &state.notice {
		notice.clone()
	} else if !state.signed_in {
		"Sign in to start listening".into()
	} else if !state.connected || !state.logged_in {
		"Starting the player…".into()
	} else if !state.active {
		"Pick something to play".into()
	} else {
		"Nothing playing".into()
	}
}

/// The sleep timer while one runs: a countdown Tern clocks (or "End of song"); a click cancels it.
fn sleep_chip(state: &State, icons: &Icons) -> Option<Node<Msg>> {
	use crate::app::rules::{Sleep, SleepChoice};
	let label: Node<Msg> = match state.rules.sleep? {
		Sleep::At { .. } => {
			let left = state.sleep_left(std::time::Instant::now())?;
			let age = -i64::try_from(left.as_millis()).unwrap_or(i64::MAX);
			tern_sdk::ui::elapsed().format(tern_sdk::ui::ElapsedFormat::Clock).age(age).into()
		},
		Sleep::EndOfTrack => html::span("End of song").into(),
	};
	Some(
		html::el("button")
			.class("tt-sleep")
			.key("sleep")
			.title("Cancel the sleep timer")
			.aria("Cancel the sleep timer")
			.child(icon(icons, Glyph::Moon, 16.0, Tap::Msg(Msg::Sleep(SleepChoice::Off), None)))
			.child(label)
			.on_click(Msg::Sleep(SleepChoice::Off))
			.into(),
	)
}
