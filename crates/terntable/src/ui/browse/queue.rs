//! Now-playing and upcoming queue rows, source headings and editing controls.

use tern_sdk::ui::{Node, html};

use crate::{
	app::{Msg, State},
	engine::{CoverSize, QueueEntry},
	library::Conn,
	ui::{
		Tap, art, button, div,
		icons::{Glyph, Icons},
		text,
	},
};

// ------------------------------------------------------------------- queue

/// Whether the queue can be changed: this player is the one playing, with a song a list can start from.
fn queue_editable(state: &State) -> bool {
	state.active
		&& state.logged_in
		&& state.library.conn == Conn::Ready
		&& state.connect.remote.is_none()
		&& crate::app::queue::playable(&state.item.uri)
}

/// The queue: what plays, then the upcoming songs in runs by where they come from (queued by
/// hand, the playlist, autoplay), with the controls that edit them while this player plays.
pub(super) fn queue(state: &State, icons: &Icons) -> Node<Msg> {
	let editable = queue_editable(state);
	let upcoming = state.queue_edit.view(&state.upcoming);
	let mut class = String::from("tt-panel tt-queue");
	if state.queue_edit.applying() {
		class.push_str(" tt-busy");
	}
	let mut head = div("tt-qtop").child(text("tt-h2", "Queue"));
	if editable && upcoming.iter().any(|e| e.source == "queue") {
		head = head.child(html::el("button").class("tt-link tt-dim").text("Clear queue").on_click(Msg::QueueClear));
	}
	let mut panel = div(&class).key("queue").child(head);
	if state.queue_edit.busy() {
		panel = panel.child(text(
			"tt-qnote tt-dim",
			"Applying your changes: the song restarts for a moment, shuffle turns off, and this list replaces the playlist.",
		));
	}
	if !state.item.uri.is_empty() {
		panel = panel.child(text("tt-qhead", "Now playing")).child(now_row(state));
	}
	if upcoming.is_empty() {
		return panel.child(text("tt-dim", "Nothing queued.")).into();
	}
	let mut run: Option<&str> = None;
	for (i, e) in upcoming.iter().enumerate() {
		if run != Some(e.source.as_str()) {
			run = Some(e.source.as_str());
			panel = panel.child(text("tt-qhead", run_title(state, &e.source)));
			if e.source == "autoplay" {
				panel = panel.child(text("tt-qnote tt-dim", "Spotify's picks, after your queue and playlist end."));
			}
		}
		panel = panel.child(queue_row(state, icons, i, upcoming.len(), e, editable));
	}
	panel.into()
}

/// The heading of a run of upcoming songs by where they come from.
fn run_title(state: &State, source: &str) -> String {
	match source {
		"queue" => "Next in queue".to_owned(),
		"autoplay" if state.options.smart_shuffle() => "Recommended".to_owned(),
		"autoplay" => "Autoplay".to_owned(),
		_ => match state.context.name() {
			"" => "Next up".to_owned(),
			name => format!("Next from {name}"),
		},
	}
}

fn now_row(state: &State) -> Node<Msg> {
	let name = state.item.name();
	html::div()
		.class("tt-qrow tt-now")
		.key("now")
		.child(art(state, state.cover_url(), "tt-art-40", name, Tap::NONE))
		.child(div("tt-tname-wrap").child(text("tt-tname tt-trunc", name)).child(text("tt-tsub tt-trunc tt-dim", state.item.creators())))
		.into()
}

/// Upcoming song `i` of `n`. Double click (or the menu) plays it; hovering shows the move and
/// remove buttons when the queue can be changed.
fn queue_row(state: &State, icons: &Icons, i: usize, n: usize, e: &QueueEntry, editable: bool) -> Node<Msg> {
	let name = e.item.name();
	let mut row = html::div()
		.class(if e.source == "autoplay" { "tt-qrow tt-auto" } else { "tt-qrow" })
		// Songs can repeat in a queue, and siblings must not share a key.
		.key(format!("q{i}-{}", e.uid))
		.on_dblclick(Msg::QueueJump(i))
		.on_menu("play", Msg::QueueJump(i))
		.child(art(state, e.item.cover(CoverSize::Small), "tt-art-40", name, Tap::Msg(Msg::Noop, Some(Msg::QueueJump(i)))))
		.child(div("tt-tname-wrap").child(text("tt-tname tt-trunc", name)).child(text("tt-tsub tt-trunc tt-dim", e.item.creators())));
	if editable {
		let mut actions = div("tt-qact");
		if i > 0 {
			actions = actions.child(button(icons, Glyph::Down, "Move up", Msg::QueueMove(i, -1), "tt-q tt-flip", 14.0));
		}
		if i + 1 < n {
			actions = actions.child(button(icons, Glyph::Down, "Move down", Msg::QueueMove(i, 1), "tt-q", 14.0));
		}
		actions = actions.child(button(icons, Glyph::Close, "Remove from queue", Msg::QueueRemove(i), "tt-q", 14.0));
		row = row.on_menu("remove-from-queue", Msg::QueueRemove(i)).child(actions);
	}
	row.into()
}
