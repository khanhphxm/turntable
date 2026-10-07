//! The lyrics view: the song's words with the line being sung lit. A side
//! panel in the full layouts, a page in the compact one. Synced lines are drawn
//! once per track; Tern lights each at its moment from the animations of
//! [`sheet`], so nothing is redrawn as the song goes on.

use std::fmt::Write as _;

use tern_sdk::ui::{Node, html};

use super::{button, div, text};
use crate::{
	app::{Layout, LyricsState, Msg, Panel, State},
	sources::external::Lyrics,
	ui::icons::{Glyph, Icons},
};

/// Where the credit leads.
const LRCLIB: &str = "https://lrclib.net";

/// The node id of synced line `i` in the view (the keyed path from the region root), which
/// Tern's reveal op needs to scroll to it.
pub fn line_id(i: usize) -> String {
	format!("main.app.lyrics.body.l{i}")
}

pub fn panel(state: &State, icons: &Icons) -> Node<Msg> {
	let compact = state.layout() == Layout::Compact;
	let mut head = div("tt-ly-head").key("head");
	if compact {
		head = head.child(button(icons, Glyph::Down, "Close", Msg::TogglePanel(Panel::Lyrics), "", 22.0));
	}
	let mut panel = div(if compact { "tt-panel tt-lyrics tt-lyrics-full" } else { "tt-panel tt-lyrics" })
		.key("lyrics")
		.child(head.child(text("tt-h2", "Lyrics")))
		.child(body(state));
	if matches!(state.lyrics_state(), Some(LyricsState::Found(_))) {
		let credit = html::div()
			.class("tt-link tt-dim tt-source")
			.title("Lyrics from LRCLIB, a free, open lyrics database")
			.text("Lyrics from LRCLIB")
			.on_click(Msg::OpenUrl(LRCLIB.into()));
		panel = panel.child(div("tt-ly-foot").key("foot").child(credit));
	}
	panel.into()
}

fn body(state: &State) -> Node<Msg> {
	let note = |words: &str| text("tt-ly-note tt-dim", words);
	let mut body = div("tt-ly-body").key("body");
	if state.item.uri.is_empty() {
		return body.child(note("Nothing playing")).into();
	}
	if !state.prefs.sources.lyrics {
		let open = html::el("button").class("tt-link").text("Open settings").on_click(Msg::TogglePanel(Panel::Settings));
		return body.child(note("Lyrics from LRCLIB are turned off.")).child(div("tt-ly-note").child(open)).into();
	}
	match state.lyrics_state() {
		Some(LyricsState::Found(Lyrics::Synced(lines))) => {
			for (i, (_, line)) in lines.iter().enumerate() {
				body = body.child(html::div().class("tt-ly").key(format!("l{i}")).text(line.as_str()));
			}
		},
		Some(LyricsState::Found(Lyrics::Plain(words))) => {
			for (i, stanza) in words.split("\n\n").enumerate() {
				body = body.child(html::div().class("tt-ly-plain").key(format!("p{i}")).text(stanza));
			}
		},
		Some(LyricsState::Loading) => body = body.child(note("Loading lyrics…")),
		Some(LyricsState::Failed) => {
			let retry = html::el("button").class("tt-link").text("Try again").on_click(Msg::RetryLyrics);
			body = body.child(note("Couldn't load lyrics")).child(div("tt-ly-note").child(retry));
		},
		Some(LyricsState::Missing) | None => body = body.child(note("No lyrics for this song")),
	}
	body.into()
}

/// Lights each synced line at its moment: every line fades up when it starts and down when the next
/// one does, from animations whose delays count from the playback position. Built again, with other
/// keyframe names (`flip`) so Tern restarts them, whenever the position jumps or playback stops or
/// resumes. `None` while the lyrics aren't on screen.
pub fn sheet(state: &State, flip: bool) -> Option<String> {
	let lines = state.synced_lyrics()?;
	let position = state.position_ms().cast_signed();
	let n = u8::from(flip);
	let run = if state.playing() { "running" } else { "paused" };
	// The opacities are `.tt-ly`'s in the sheet, lit and sung.
	let mut css = format!(
		"@keyframes tt-in{n} {{ from {{ opacity: .45 }} to {{ opacity: 1 }} }}\
		 @keyframes tt-out{n} {{ from {{ opacity: 1 }} to {{ opacity: .3 }} }}\
		 .tt-ly {{ animation: tt-in{n} 250ms forwards, tt-out{n} 350ms forwards; animation-play-state: {run}; }}\
		 .sf-still .tt-ly {{ animation-play-state: {run}; }}"
	);
	// The last line is lit to the end of the song.
	let end = lines.last().map_or(0, |(ms, _)| *ms).max(state.duration_ms());
	for (i, (ms, _)) in lines.iter().enumerate() {
		let next = lines.get(i + 1).map_or(end, |(ms, _)| *ms);
		let _ = write!(
			css,
			" .tt-ly:nth-child({}) {{ animation-delay: {}ms, {}ms; }}",
			i + 1,
			ms.cast_signed() - position,
			next.cast_signed() - position
		);
	}
	Some(css)
}

#[cfg(test)]
mod tests {
	use std::time::Instant;

	use super::*;
	use crate::{app::Anchor, engine::Status};

	/// Playing 25 s into a 240 s track with the lyrics open: sung at 10 s and 20 s, a pause from 30 s.
	fn singing() -> State {
		let mut s = State { status: Status::Playing, panel: Some(Panel::Lyrics), ..State::default() };
		s.item.uri = "spotify:track:abc".into();
		s.item.decorations.playback.duration_ms = 240_000;
		s.anchor = Anchor { position_ms: 25_000, at: Instant::now(), speed: 1.0 };
		let lines = vec![(10_000, "one".to_owned()), (20_000, "two".to_owned()), (30_000, String::new())];
		s.lyrics = Some((s.item.uri.clone(), LyricsState::Found(Lyrics::Synced(lines))));
		s
	}

	#[test]
	fn there_is_nothing_to_light_unless_synced_lyrics_are_on_screen() {
		let mut s = singing();
		s.panel = None;
		assert_eq!(sheet(&s, false), None, "panel closed");
		let mut s = singing();
		s.lyrics = Some((s.item.uri.clone(), LyricsState::Found(Lyrics::Plain("words".into()))));
		assert_eq!(sheet(&s, false), None, "no timing");
		s.item.uri = "spotify:track:other".into();
		assert_eq!(sheet(&s, false), None, "another track's lyrics");
	}
}
