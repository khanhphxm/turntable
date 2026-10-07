//! Home's quick picks and personalized shelves of artists, playlists and songs.

use tern_sdk::ui::{Node, html};

use super::cards::{card_frame, context_button, play_button, shelf, shelf_head};
use crate::{
	app::{Msg, State},
	library::{Route, Tile},
	spotify::{self, Track, id_of},
	ui::{
		Tap, art, artists, card_columns, div, icon,
		icons::{Glyph, Icons},
		text,
	},
};

pub(super) fn home(state: &State, icons: &Icons) -> Vec<Node<Msg>> {
	let lib = &state.library;
	let h = &lib.home;
	let mut out: Vec<Node<Msg>> = vec![text("tt-greet", greeting())];

	// Quick picks: Liked Songs, then what was played lately, then playlists.
	let mut quick: Vec<Tile> = vec![Tile::liked()];
	for t in h.recents.iter().cloned().chain(lib.playlists.iter().map(Tile::playlist)) {
		if quick.len() == 8 {
			break;
		}
		if !quick.iter().any(|q| q.route == t.route) {
			quick.push(t);
		}
	}
	let mut grid = div("tt-quick-grid").key("quick");
	for (i, t) in quick.iter().enumerate() {
		grid = grid.child(quick_tile(state, icons, i, t));
	}
	out.push(grid.into());

	if !h.loaded {
		out.push(text("tt-empty tt-dim", "Loading…"));
		return out;
	}
	if !h.recents.is_empty() {
		out.push(shelf(state, icons, "recents", "Recents", &h.recents));
	}
	if let Some(a) = h.top_artists.as_ref().filter(|a| !a.is_empty()) {
		out.push(shelf(state, icons, "artists", "Your top artists", a));
	}
	if let Some(a) = h.followed.as_ref().filter(|a| !a.is_empty()) {
		out.push(shelf(state, icons, "followed", "Artists you follow", a));
	}
	let (songs_title, songs) = h.songs();
	if !songs.is_empty() {
		out.push(song_shelf(state, icons, songs_title, songs));
	}
	if !h.albums.is_empty() {
		out.push(shelf(state, icons, "albums", "Your albums", &h.albums));
	}
	if !lib.playlists.is_empty() {
		let tiles: Vec<Tile> = lib.playlists.iter().map(Tile::playlist).collect();
		out.push(shelf(state, icons, "playlists", "Your playlists", &tiles));
	}
	out
}

/// A compact tile of the home page's top grid: cover, name, a play button on hover.
fn quick_tile(state: &State, icons: &Icons, i: usize, t: &Tile) -> Node<Msg> {
	let go = Msg::Navigate(t.route.clone());
	let cover: Node<Msg> = if t.route == Route::Liked {
		html::div()
			.class("tt-art tt-art-48 tt-art-liked tt-art-fallback")
			.child(icon(icons, Glyph::Heart, 18.0, Tap::Msg(go.clone(), None)))
			.into()
	} else {
		art(state, t.image.as_deref(), "tt-art-48", &t.title, Tap::Msg(go.clone(), None))
	};
	let mut tile = html::div().class("tt-quick").key(format!("q{i}")).child(cover).child(text("tt-quick-title tt-trunc", t.title.clone()));
	if let Some(uri) = &t.play {
		tile = tile.child(context_button(state, icons, uri));
	}
	tile.on_click(go).into()
}
/// Songs as cards (top songs, else recently played), as in Spotify: a click opens the
/// song's album with the song selected; the play button plays from it, or pauses and
/// resumes it once it is the song playing.
fn song_shelf(state: &State, icons: &Icons, title: &'static str, songs: &[Track]) -> Node<Msg> {
	let row = card_columns(state);
	let open = state.open_shelves.contains("songs");
	let mut cards = div("tt-cards");
	for (i, t) in songs.iter().enumerate().take(if open { usize::MAX } else { row }) {
		let now = t.uri == state.item.uri;
		let play = if now { Msg::TogglePlay } else { Msg::PlayRow(i) };
		let go = t.album.as_ref().map_or_else(|| play.clone(), |a| Msg::OpenTrack { album: id_of(&a.uri).to_owned(), uri: t.uri.clone() });
		let url = t.album.as_ref().and_then(|a| spotify::pick_image(&a.images, 300)).map(|i| i.url.as_str());
		let cover = art(state, url, "", &t.name, Tap::Msg(go.clone(), None));
		let button = play_button(icons, play, now && state.playing());
		cards = cards.child(card_frame(&format!("song-{i}"), now, cover, &t.name, &artists(t), Some(button), go));
	}
	div("tt-shelf").key("songs").child(shelf_head("songs", title, songs.len() > row, open)).child(cards).into()
}
/// "Good morning" / "Good afternoon" / "Good evening" by local time.
fn greeting() -> String {
	let hour = local_hour();
	match hour {
		5..=11 => "Good morning",
		12..=17 => "Good afternoon",
		_ => "Good evening",
	}
	.to_owned()
}

#[cfg(unix)]
fn local_hour() -> i32 {
	// SAFETY: localtime_r fills a tm struct from the current time.
	unsafe {
		let now = libc::time(std::ptr::null_mut());
		let mut tm: libc::tm = std::mem::zeroed();
		libc::localtime_r(&raw const now, &raw mut tm);
		tm.tm_hour
	}
}

#[cfg(windows)]
fn local_hour() -> i32 {
	// SAFETY: GetLocalTime returns an initialized SYSTEMTIME in the user's local time zone.
	i32::from(unsafe { windows::Win32::System::SystemInformation::GetLocalTime() }.wHour)
}
