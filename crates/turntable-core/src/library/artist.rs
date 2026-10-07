//! Artist discography, popular songs, About data and cached artist and artwork metadata.

use super::{Listing, Tile, pages::part, worker::Session};
use crate::{
	net::par,
	sources::external,
	spotify::{self, Artist, Track},
};

/// An artist page's popular songs in Spotify's order with their play counts (empty:
/// unknown), the web player's overview of the artist, and the color of its photo.
#[derive(Debug, Clone, Default)]
pub struct Popularity {
	pub tracks:   Vec<Track>,
	pub plays:    Vec<u64>,
	pub overview: Option<spotify::ArtistOverview>,
	pub accent:   Option<String>,
}
/// Artists kept for the session before the lot is let go (names and pictures: small).
const ARTISTS_KEPT: usize = 128;
/// An artist's header: the artist, their releases by kind, where they appear, and whether they are followed.
pub(super) fn artist_header(s: &mut Session, id: &str) -> spotify::Result<Listing> {
	let uri = format!("spotify:artist:{id}");
	let known = s.artists.get(id).cloned();
	let client = &s.client;
	let kinds = vec![("Albums", "album"), ("Singles and EPs", "single"), ("Compilations", "compilation")];
	// The artist, each group of releases and whether it is followed don't depend on each other: asked
	// together. A part that fails (a refused group, a hiccup) is left out, not the page.
	let (a, (groups, (appears_on, followed))) = par::join(
		|| match known {
			Some(a) => Ok(a),
			None => client.artist(id),
		},
		|| {
			par::join(
				|| par::map(kinds, 3, |(title, groups)| (title, part(client.artist_albums(id, groups, spotify::MAX_ARTIST_ALBUMS)))),
				|| {
					par::join(
						|| part(client.artist_albums(id, "appears_on", spotify::MAX_ARTIST_ALBUMS)),
						|| part(client.library_contains(&[&uri])),
					)
				},
			)
		},
	);
	let a = a?;
	if s.artists.len() >= ARTISTS_KEPT {
		s.artists.clear();
	}
	s.artists.insert(id.to_owned(), a.clone());
	let mut discography = Vec::new();
	for (title, releases) in groups {
		if let Some(releases) = releases?.filter(|r| !r.is_empty()) {
			discography.push((title, releases.iter().map(Tile::release).collect()));
		}
	}
	let appears_on = appears_on?.unwrap_or_default();
	let followed = followed?.and_then(|f| f.first().copied());
	Ok(Listing {
		kicker: ("Artist").into(),
		title: a.name.clone(),
		subtitle: a.genres.iter().take(3).map(String::as_str).collect::<Vec<_>>().join(", "),
		cover_url: spotify::pick_image(&a.images, 300).map(|i| i.url.clone()),
		context_uri: Some(uri),
		followed,
		discography,
		appears_on: appears_on.iter().map(Tile::album).collect(),
		artist: Some(a),
		..Listing::default()
	})
}
/// An artist page's popular songs, built from the web player's overview alone (no Web API call per
/// song); Spotify's search order when the web player fails. Asked once a session.
pub(super) fn popularity(s: &mut Session, id: &str) -> spotify::Result<Popularity> {
	if let Some(p) = s.popular.get(&id.to_owned()) {
		return Ok(p);
	}
	let page = s.web.artist(id).ok().filter(|p| !p.popular.is_empty());
	let p = match page {
		Some(page) => {
			let counted = page.popular.iter().all(|t| t.plays.is_some());
			Popularity {
				plays:    if counted { page.popular.iter().filter_map(|t| t.plays).collect() } else { Vec::new() },
				tracks:   page.popular.into_iter().map(|t| t.track).collect(),
				accent:   page.overview.color.clone(),
				overview: Some(page.overview),
			}
		},
		None => {
			let a = artist(s, id)?;
			Popularity {
				tracks: part(s.client.artist_tracks(&a))?.unwrap_or_default(),
				overview: s.web.overview(id).ok(),
				..Popularity::default()
			}
		},
	};
	s.popular.insert(id.to_owned(), p.clone());
	Ok(p)
}

/// An artist's About (Wikidata and Wikipedia), asked once a session; empty when they don't answer.
/// An answer given while Wikipedia is turned off is not remembered, so turning it on asks again.
pub(super) fn about(s: &mut Session, id: &str) -> external::About {
	if let Some(a) = s.abouts.get(&id.to_owned()) {
		return a;
	}
	let about = s.external.about(id).unwrap_or_default();
	if crate::app::settings::allowed::wikipedia() {
		s.abouts.insert(id.to_owned(), about.clone());
	}
	about
}
/// An artist, from the session's cache when it was fetched before.
pub(super) fn artist(s: &mut Session, id: &str) -> spotify::Result<Artist> {
	if let Some(a) = s.artists.get(id) {
		return Ok(a.clone());
	}
	let a = s.client.artist(id)?;
	s.artists.insert(id.to_owned(), a.clone());
	Ok(a)
}
