//! Artist page sections: Popular, picks, discography, related artists and About.

use tern_sdk::ui::{Node, html};

use super::{
	cards::{card, shelf, shelf_head},
	tracks::{Middle, plays_shown, tracks_with},
};
use crate::{
	app::{Layout, Msg, State},
	library::{Listing, Route, Tile},
	sources::external,
	spotify::{self, id_of},
	ui::{Tap, art, card_columns, div, grouped, icons::Icons, text},
};

/// An artist page below its header: popular songs, the artist's pick, discography, appears on,
/// fans also like, discovered on, about.
pub(super) fn artist_body(state: &State, icons: &Icons, listing: &Listing) -> Vec<Node<Msg>> {
	let mut out = Vec::new();
	let overview = listing.overview.as_ref();
	if !listing.tracks.is_empty() {
		let open = state.open_shelves.contains("popular");
		let shown = if open { listing.tracks.len() } else { listing.tracks.len().min(5) };
		let tracks = listing.tracks.get(..shown).unwrap_or(&listing.tracks);
		let plays = plays_shown(state, listing.plays.get(..shown).unwrap_or_default());
		let head = div("tt-shelf-head").child(text("tt-h2", "Popular"));
		let mut section = div("tt-shelf").key("popular").child(head).child(tracks_with(state, tracks, Middle::Plays(plays)));
		if listing.tracks.len() > 5 {
			section = section.child(
				html::el("button")
					.class("tt-link tt-dim tt-see-more")
					.text(if open { "Show less" } else { "See more" })
					.on_click(Msg::ToggleShelf("popular")),
			);
		}
		out.push(section.into());
	} else if !listing.popular_loaded && state.library.error.is_none() {
		// The songs come from the web player a moment after the page: quiet rows of their height.
		let mut rows = div("tt-tracks tt-noalbum");
		for i in 0..5 {
			rows = rows.child(div("tt-tr tt-skel").key(format!("s{i}")));
		}
		out.push(div("tt-shelf").key("popular").child(div("tt-shelf-head").child(text("tt-h2", "Popular"))).child(rows).into());
	}
	if listing.loading && listing.artist.is_none() {
		if out.is_empty() {
			out.push(text("tt-empty tt-dim", "Loading…"));
		}
		return out;
	}
	if let Some(p) = overview.and_then(|o| o.pick.as_ref())
		&& let Some(node) = pick(state, p, listing.artist.as_ref(), overview.and_then(|o| o.avatar.as_deref()))
	{
		// Wide enough, Spotify's column beside Popular; else its own row below.
		if state.layout() == Layout::Wide && out.len() == 1 {
			let popular = out.remove(0);
			out.push(div("tt-top-row").child(popular).child(node).into());
		} else {
			out.push(node);
		}
	}
	if !listing.discography.is_empty() {
		out.push(discography(state, icons, &listing.discography));
	}
	if !listing.appears_on.is_empty() {
		out.push(shelf(state, icons, "appears", "Appears on", &listing.appears_on));
	}
	if let Some(o) = overview {
		let fans: Vec<Tile> = o
			.related
			.iter()
			.map(|r| Tile {
				route: Route::Artist(r.id.clone()),
				play:  Some(format!("spotify:artist:{}", r.id)),
				title: r.name.clone(),
				sub:   "Artist".into(),
				image: r.image.clone(),
			})
			.collect();
		if !fans.is_empty() {
			out.push(shelf(state, icons, "fans", "Fans also like", &fans));
		}
		let discovered: Vec<Tile> = o
			.discovered_on
			.iter()
			.map(|p| Tile {
				route: Route::Playlist(p.id.clone()),
				play:  Some(format!("spotify:playlist:{}", p.id)),
				title: p.name.clone(),
				sub:   if p.owner.is_empty() { "Playlist".into() } else { format!("Playlist · {}", p.owner) },
				image: p.image.clone(),
			})
			.collect();
		if !discovered.is_empty() {
			out.push(shelf(state, icons, "discovered", "Discovered on", &discovered));
		}
	}
	if let Some(a) = &listing.artist
		&& let Some(node) = about(state, a, listing.about.as_ref(), overview)
	{
		out.push(node);
	}
	out
}

/// What the artist pinned to their page, as Spotify shows it: a compact row with the cover, who posted
/// it, its title and kind; the artist's comment above them when there is one.
fn pick(state: &State, p: &spotify::Pick, artist: Option<&spotify::Artist>, avatar: Option<&str>) -> Option<Node<Msg>> {
	let id = id_of(&p.uri).to_owned();
	let route = if p.uri.starts_with("spotify:album:") {
		Route::Album(id)
	} else if p.uri.starts_with("spotify:playlist:") {
		Route::Playlist(id)
	} else {
		return None;
	};
	let go = Msg::Navigate(route);
	let mut body = div("tt-apick-text");
	if let Some(a) = artist {
		let mut by = div("tt-apick-by").child(art(state, avatar, "tt-art-16", &a.name, Tap::Msg(go.clone(), None)));
		by = by.child(text("tt-trunc tt-dim", format!("Posted by {}", a.name)));
		body = body.child(by);
	}
	body = body.child(text("tt-apick-title tt-trunc", p.title.clone()));
	// "Album • New Release" is Spotify's subtitle; the kind alone reads calmer.
	let kind = p.subtitle.split('•').next().unwrap_or_default().trim();
	if !kind.is_empty() {
		body = body.child(text("tt-dim tt-trunc", kind.to_owned()));
	}
	if !p.comment.is_empty() {
		body = body.child(text("tt-apick-comment tt-trunc", format!("“{}”", p.comment)));
	}
	Some(
		div("tt-shelf")
			.key("pick")
			.child(text("tt-h2", "Artist pick"))
			.child(
				html::div()
					.class("tt-apick")
					.child(art(state, p.image.as_deref(), "tt-art-pick", &p.title, Tap::Msg(go.clone(), None)))
					.child(body)
					.on_click(go),
			)
			.into(),
	)
}

/// The artist's header photo, avatar and gallery, Wikipedia's summary, genres and top cities;
/// hidden when there's nothing to say.
fn about(
	state: &State,
	a: &spotify::Artist,
	info: Option<&external::About>,
	overview: Option<&spotify::ArtistOverview>,
) -> Option<Node<Msg>> {
	let summary = info.and_then(|i| i.summary.as_ref());
	if summary.is_none() && !overview.is_some_and(|o| o.header.is_some() || !o.gallery.is_empty() || !o.cities.is_empty()) {
		return None;
	}
	let image = overview.and_then(|o| o.avatar.as_deref()).or_else(|| spotify::pick_image(&a.images, 640).map(|i| i.url.as_str()));
	let mut facts = div("tt-about-facts").child(text("tt-about-name", a.name.clone()));
	if let Some(d) = info.and_then(|i| i.description.as_ref()) {
		facts = facts.child(text("tt-dim", d.clone()));
	}
	if let Some(summary) = summary {
		facts = facts.child(text("tt-about-text", summary.clone()));
	}
	if !a.genres.is_empty() {
		facts = facts.child(text("tt-dim", a.genres.join(", ")));
	}
	if let Some(o) = overview
		&& !o.cities.is_empty()
	{
		let mut cities = div("tt-cities").child(text("tt-about-sub", "Top cities"));
		for c in o.cities.iter().take(5) {
			let place = if c.country.is_empty() { c.name.clone() } else { format!("{}, {}", c.name, c.country) };
			cities = cities.child(
				div("tt-city").child(text("tt-trunc", place)).child(text("tt-dim tt-num", format!("{} listeners", grouped(c.listeners)))),
			);
		}
		facts = facts.child(cities);
	}
	if let Some(url) = info.and_then(|i| i.wikipedia.as_ref()) {
		facts = facts.child(
			html::div()
				.class("tt-link tt-dim tt-source")
				.title("Text from Wikipedia, CC BY-SA 4.0")
				.text("From Wikipedia")
				.on_click(Msg::OpenUrl(url.clone())),
		);
	}
	let mut card = div("tt-artist-about");
	if let Some(header) = overview.and_then(|o| o.header.as_deref()) {
		card = card.child(art(state, Some(header), "tt-about-banner", &a.name, Tap::NONE));
	}
	card = card.child(div("tt-about-body").child(art(state, image, "tt-about-photo", &a.name, Tap::NONE)).child(facts));
	if let Some(o) = overview
		&& !o.gallery.is_empty()
	{
		let mut shots = div("tt-about-gallery");
		for url in &o.gallery {
			shots = shots.child(art(state, Some(url), "tt-about-shot", &a.name, Tap::NONE));
		}
		card = card.child(shots);
	}
	Some(div("tt-shelf").key("about").child(text("tt-h2", "About")).child(card).into())
}

/// Releases with filter chips: everything newest first, or one kind.
fn discography(state: &State, icons: &Icons, kinds: &[(&'static str, Vec<Tile>)]) -> Node<Msg> {
	let mut chips = div("tt-chips");
	for (i, label) in std::iter::once("All releases").chain(kinds.iter().map(|(k, _)| *k)).enumerate() {
		let current = state.discography == i;
		chips = chips.child(
			html::el("button")
				.class(if current { "tt-chip tt-cur" } else { "tt-chip" })
				.text(label)
				.key(format!("k{i}"))
				.on_click(Msg::Discography(i)),
		);
	}
	let tiles: Vec<Tile> = match state.discography.checked_sub(1).and_then(|i| kinds.get(i)) {
		Some((_, tiles)) => tiles.clone(),
		None => {
			let mut all: Vec<Tile> = kinds.iter().flat_map(|(_, t)| t.iter().cloned()).collect();
			all.sort_by(|a, b| b.sub.cmp(&a.sub));
			all
		},
	};
	let row = card_columns(state);
	let open = state.open_shelves.contains("discography");
	let mut cards = div("tt-cards");
	for (i, t) in tiles.iter().enumerate().take(if open { usize::MAX } else { row }) {
		cards = cards.child(card(state, icons, &format!("d{i}"), t));
	}
	div("tt-shelf")
		.key("discography")
		.child(shelf_head("discography", "Discography", tiles.len() > row, open))
		.child(chips)
		.child(cards)
		.into()
}
