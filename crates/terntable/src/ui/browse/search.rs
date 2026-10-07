//! Search input and its artist, track, album and playlist results.

use tern_sdk::ui::{self, Node, html};

use super::{cards::shelf, tracks::tracks};
use crate::{
	app::{Field, Msg, State},
	library::{Route, Tile},
	spotify,
	ui::{
		Tap, art, div, icon,
		icons::{Glyph, Icons},
		text,
	},
};

pub(super) fn search(state: &State, icons: &Icons) -> Vec<Node<Msg>> {
	let lib = &state.library;
	let field = ui::input()
		.key(Field::Search.id())
		.text(lib.query.clone())
		.placeholder("What do you want to play?")
		.on_focus(Msg::Focus(Some(Field::Search)))
		.on_edit(|e| Msg::Edit { from: e.from, to: e.to, text: e.text.clone() });
	let mut out: Vec<Node<Msg>> = vec![
		div(if state.focus == Some(Field::Search) { "tt-search tt-focus" } else { "tt-search" })
			.key("searchbar")
			.child(icon(icons, Glyph::Search, 16.0, Tap::Msg(Msg::Focus(Some(Field::Search)), None)))
			.child(field)
			.into(),
	];
	let Some(results) = &lib.search else {
		out.push(text("tt-empty tt-dim", "Search songs, artists, albums and playlists. Press / to search from anywhere."));
		return out;
	};
	if !results.artists.items.is_empty() {
		let tiles: Vec<Tile> = results.artists.items.iter().map(Tile::artist).collect();
		out.push(shelf(state, icons, "search-artists", "Artists", &tiles));
	}
	if !results.tracks.items.is_empty() {
		out.push(text("tt-h2", "Songs"));
		out.push(tracks(state, &results.tracks.items, true));
	}
	if !results.albums.items.is_empty() {
		out.push(text("tt-h2", "Albums"));
		let mut cards = div("tt-cards");
		for a in &results.albums.items {
			let id = a.uri.rsplit(':').next().unwrap_or_default().to_owned();
			let url = spotify::pick_image(&a.images, 300).map(|i| i.url.as_str());
			let by = a.artists.iter().map(|x| x.name.as_str()).collect::<Vec<_>>().join(", ");
			let go = Msg::Navigate(Route::Album(id.clone()));
			cards = cards.child(
				html::div()
					.class("tt-card")
					.key(format!("album-{id}"))
					.child(art(state, url, "", &a.name, Tap::Msg(go.clone(), None)))
					.child(text("tt-card-title tt-trunc", a.name.clone()))
					.child(text("tt-pl-sub tt-trunc tt-dim", by))
					.on_click(go),
			);
		}
		out.push(cards.into());
	}
	if !results.playlists.items.is_empty() {
		out.push(text("tt-h2", "Playlists"));
		let mut cards = div("tt-cards");
		for p in &results.playlists.items {
			let url = spotify::pick_image(&p.images, 300).map(|i| i.url.as_str());
			let go = Msg::Navigate(Route::Playlist(p.id.clone()));
			cards = cards.child(
				html::div()
					.class("tt-card")
					.key(format!("sp-{}", p.id))
					.child(art(state, url, "", &p.name, Tap::Msg(go.clone(), None)))
					.child(text("tt-card-title tt-trunc", p.name.clone()))
					.child(text("tt-pl-sub tt-trunc tt-dim", p.owner.display_name.clone().unwrap_or_default()))
					.on_click(go),
			);
		}
		out.push(cards.into());
	}
	out
}
