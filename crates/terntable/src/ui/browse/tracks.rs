//! Track tables and their album or play-count middle columns.

use tern_sdk::ui::{Node, html};

use crate::{
	app::{self, Msg, State},
	library::Route,
	spotify::{self, Track},
	ui::{Tap, art, artists, div, grouped, text},
};

/// The track table. `with_album` adds the album column (playlists, search);
/// albums and recents don't need it.
pub(super) fn tracks(state: &State, rows: &[Track], with_album: bool) -> Node<Msg> {
	tracks_with(state, rows, if with_album { Middle::Album } else { Middle::Nothing })
}

/// What a track table shows between the title and the time.
#[derive(Clone, Copy)]
pub(super) enum Middle<'a> {
	Nothing,
	Album,
	/// An artist's own Popular list: play counts (empty: unknown), and neither
	/// a header nor artist lines, since every row is the artist's.
	Plays(&'a [u64]),
	/// An album's songs with their play counts (empty: unknown).
	Counts(&'a [u64]),
}

pub(super) fn tracks_with(state: &State, rows: &[Track], middle: Middle<'_>) -> Node<Msg> {
	let selected = state.library.selected.filter(|_| state.library.show_selection);
	let now_uri = state.item.uri.as_str();
	let editable = state.library.listing.as_ref().is_some_and(|l| l.editable) && matches!(state.library.route, Route::Playlist(_));
	let popular = matches!(middle, Middle::Plays(_));
	let has_middle = match middle {
		Middle::Nothing => false,
		Middle::Album => true,
		Middle::Plays(plays) | Middle::Counts(plays) => !plays.is_empty(),
	};
	let mut table = div(if has_middle { "tt-tracks" } else { "tt-tracks tt-noalbum" });
	if !popular {
		let mut head = div("tt-tr tt-th").child(text("tt-idx", "#")).child(html::div().text("Title"));
		if has_middle {
			head =
				head.child(if matches!(middle, Middle::Counts(_)) { text("tt-plays", "Plays") } else { html::div().text("Album").into() });
		}
		table = table.child(head.child(text("tt-tdur", "Time")));
	}
	for (i, t) in rows.iter().enumerate() {
		let mut class = String::from("tt-tr");
		// Explicit songs are skipped while they're off: shown dim, so it isn't a surprise.
		if t.explicit && !state.prefs.playback.explicit {
			class.push_str(" tt-blocked");
		}
		if selected == Some(i) {
			class.push_str(" tt-sel");
		}
		if !now_uri.is_empty() && t.uri == now_uri {
			class.push_str(" tt-now");
		}
		let album = t.album.as_ref();
		let album_name = album.map_or("", |a| a.name.as_str());
		let thumb = album.and_then(|a| spotify::pick_image(&a.images, 64)).map(|i| i.url.as_str());
		// Explicit marks lead the line under the title, as in Spotify.
		let mut sub = div("tt-tsub tt-trunc tt-dim");
		if t.explicit {
			sub = sub.child(html::span("").class("tt-explicit").title("Explicit").child(html::span("E")));
		}
		let mut wrap = div("tt-tname-wrap").child(text("tt-tname tt-trunc", t.name.clone()));
		if !popular {
			sub = sub.child(html::span(artists(t)));
		}
		if t.explicit || !popular {
			wrap = wrap.child(sub);
		}
		let mut row = html::div()
			.class(class)
			.key(format!("{i}-{}", t.uri))
			.on_menu("play", Msg::PlayRow(i))
			.on_menu("add-to-queue", Msg::QueueRow(i))
			.on_menu("add-to-playlist", Msg::AddRowToPlaylist(i))
			.child(text("tt-idx tt-num", (i + 1).to_string()))
			.child(
				div("tt-tcell")
					.child(art(state, thumb, "tt-art-40", album_name, Tap::Msg(Msg::Select(i), Some(Msg::PlayRow(i)))))
					.child(wrap),
			);
		match middle {
			Middle::Plays(plays) | Middle::Counts(plays) => {
				if let Some(n) = plays.get(i) {
					row = row.child(text("tt-plays tt-num tt-dim", grouped(*n)));
				}
			},
			Middle::Album => row = row.child(text("tt-trunc tt-dim", album_name)),
			Middle::Nothing => {},
		}
		if editable {
			row = row.on_menu("remove-from-this-playlist", Msg::RemoveRow(i));
		}
		table = table.child(
			row.child(text("tt-tdur tt-num", app::clock(u64::from(t.duration_ms)))).on_click(Msg::Select(i)).on_dblclick(Msg::PlayRow(i)),
		);
	}
	table.into()
}
/// The play counts to show: none when the setting hides them.
pub(super) fn plays_shown<'a>(state: &State, plays: &'a [u64]) -> &'a [u64] {
	if state.prefs.display.play_counts { plays } else { &[] }
}
