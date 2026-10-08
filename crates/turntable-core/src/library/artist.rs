//! Artist discography, popular songs, About data and cached artist and artwork metadata.

use super::{Listing, Route, Tile, pages::part, worker::Session};
use crate::{
	net::par,
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
/// An artist's header: the artist, their releases by kind, where they appear, and whether they are followed.
pub(super) fn artist_header(s: &mut Session, id: &str, partial: &dyn Fn(Listing)) -> spotify::Result<Listing> {
	if let Some(player) = &mut s.player {
		let listing = player.artist_header(id, partial)?;
		if let Some(artist) = &listing.artist {
			s.cache_artist(id.to_owned(), artist.clone());
		}
		return Ok(listing);
	}
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
	s.cache_artist(id.to_owned(), a.clone());
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
	if let Some(player) = &mut s.player {
		let mut popular = match s.pages.get_ref(&Route::Artist(id.to_owned())) {
			Some(listing) if listing.popular_loaded => Popularity { tracks: listing.tracks.clone(), ..Popularity::default() },
			_ => player.popularity(id)?,
		};
		// Play counts and monthly listeners aren't in ARTIST_V4. The web overview enriches
		// session tracks without replacing Spotify's market-specific top-track order.
		if let Ok(page) = s.web.artist(id) {
			let counts: std::collections::HashMap<_, _> =
				page.popular.iter().filter_map(|t| t.plays.map(|plays| (t.track.uri.as_str(), plays))).collect();
			if let Some(plays) = popular.tracks.iter().map(|t| counts.get(t.uri.as_str()).copied()).collect::<Option<Vec<_>>>() {
				popular.plays = plays;
			}
			popular.accent.clone_from(&page.overview.color);
			popular.overview = Some(page.overview);
		}
		s.popular.insert(id.to_owned(), popular.clone());
		return Ok(popular);
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

/// An artist, from the session's cache when it was fetched before.
pub(super) fn artist(s: &mut Session, id: &str) -> spotify::Result<Artist> {
	if let Some(a) = s.artists.get(id) {
		return Ok(a.clone());
	}
	let a = match &mut s.player {
		Some(player) => player.artist(id)?,
		None => s.client.artist(id)?,
	};
	s.cache_artist(id.to_owned(), a.clone());
	Ok(a)
}
