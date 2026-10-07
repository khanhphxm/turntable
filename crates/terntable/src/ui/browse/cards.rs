//! Shared shelves, cards and context play buttons used across browse pages.

use tern_sdk::ui::{Node, html};

use crate::{
	app::{Msg, State},
	library::{Route, Tile},
	ui::{
		Tap, art, card_columns, div, icon,
		icons::{Glyph, Icons},
		text,
	},
};

/// A titled row of cards; "Show all" opens the whole shelf when it holds more than a row.
pub(super) fn shelf(state: &State, icons: &Icons, id: &'static str, title: &str, tiles: &[Tile]) -> Node<Msg> {
	let row = card_columns(state);
	let open = state.open_shelves.contains(id);
	let mut cards = div("tt-cards");
	for (i, t) in tiles.iter().enumerate().take(if open { usize::MAX } else { row }) {
		cards = cards.child(card(state, icons, &format!("{id}-{i}"), t));
	}
	div("tt-shelf").key(id).child(shelf_head(id, title, tiles.len() > row, open)).child(cards).into()
}

pub(super) fn shelf_head(id: &'static str, title: &str, more: bool, open: bool) -> Node<Msg> {
	let mut head = div("tt-shelf-head").child(text("tt-h2", title.to_owned()));
	if more {
		head = head.child(
			html::el("button").class("tt-link tt-dim").text(if open { "Show less" } else { "Show all" }).on_click(Msg::ToggleShelf(id)),
		);
	}
	head.into()
}

/// A playlist, album or artist card: artwork, then its name and kind beside a play button.
/// (Spotify's guidelines keep controls off artwork.)
pub(super) fn card(state: &State, icons: &Icons, key: &str, t: &Tile) -> Node<Msg> {
	let go = Msg::Navigate(t.route.clone());
	let cover: Node<Msg> = if t.route == Route::Liked {
		html::div().class("tt-art tt-art-liked tt-art-fallback").child(icon(icons, Glyph::Heart, 48.0, Tap::Msg(go.clone(), None))).into()
	} else {
		// Artists are round, as everywhere in Spotify.
		let shape = if matches!(t.route, Route::Artist(_)) { "tt-round" } else { "" };
		art(state, t.image.as_deref(), shape, &t.title, Tap::Msg(go.clone(), None))
	};
	let card = card_frame(key, false, cover, &t.title, &t.sub, t.play.as_deref().map(|uri| context_button(state, icons, uri)), go);
	match &t.route {
		Route::Album(_) | Route::Playlist(_) | Route::Liked => with_menu(card, Msg::QueueContext(t.route.clone())),
		_ => card,
	}
}

/// Adds "Add to queue" to a card's context menu.
fn with_menu(card: Node<Msg>, queue: Msg) -> Node<Msg> {
	html::div().class("tt-card-wrap").child(card).on_menu("add-to-queue", queue).into()
}

pub(super) fn card_frame(key: &str, now: bool, cover: Node<Msg>, title: &str, sub: &str, play: Option<Node<Msg>>, click: Msg) -> Node<Msg> {
	let mut foot = div("tt-card-foot").child(
		div("tt-card-text")
			.child(text("tt-card-title tt-trunc", title.to_owned()))
			.child(text("tt-pl-sub tt-trunc tt-dim", sub.to_owned())),
	);
	if let Some(play) = play {
		foot = foot.child(play);
	}
	html::div().class(if now { "tt-card tt-now" } else { "tt-card" }).key(key.to_owned()).child(cover).child(foot).on_click(click).into()
}

/// The play button of a playlist, album or artist: plays it, or pauses and resumes it
/// while it is what's playing.
pub(super) fn context_button(state: &State, icons: &Icons, uri: &str) -> Node<Msg> {
	if state.context.uri == uri {
		play_button(icons, Msg::TogglePlay, state.playing())
	} else {
		play_button(icons, Msg::PlayContext(uri.to_owned()), false)
	}
}

/// The play button of tiles and cards, shown on hover; `pause` for the one playing now.
pub(super) fn play_button(icons: &Icons, play: Msg, pause: bool) -> Node<Msg> {
	let (glyph, label) = if pause { (Glyph::Pause, "Pause") } else { (Glyph::Play, "Play") };
	html::el("button")
		.class("tt-tile-play")
		.title(label)
		.aria(label)
		.child(icon(icons, glyph, 16.0, Tap::Msg(play.clone(), None)))
		.on_click(play)
		.into()
}
