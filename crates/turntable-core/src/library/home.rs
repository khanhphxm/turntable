//! Session-first Home shelves. Recent contexts use native history (latest song per context),
//! followed artists and saved albums use collection2. Web API reads remain no-session fallback.

use std::{
	collections::HashSet,
	time::{Duration, Instant},
};

use super::{Home, Response, Tile, artist::artist, pages::optional, worker::Session};
use crate::{
	net::par,
	spotify::{self, PlayContext, Track, id_of},
};

/// How long a built home page is reused (navigating back and forth costs no requests).
const HOME_TTL: Duration = Duration::from_secs(60);
/// Recently played playlists/albums/artists shown on the home page.
const RECENTS: usize = 12;
/// Shelf items shown from a longer page.
const SHELF_ITEMS: usize = 12;

pub(super) fn home(s: &mut Session, deliver: &dyn Fn(Response)) -> spotify::Result<Home> {
	if let Some((at, h)) = &s.home
		&& at.elapsed() < HOME_TTL
	{
		return Ok(h.clone());
	}
	let client = &s.client;
	let player = &mut s.player;
	let played = match player.as_ref() {
		Some(player) => player.recently_played(50),
		None => client.recently_played(50),
	}?;
	let recent_tracks = recent_tracks(&played);
	deliver(Response::HomeRecentTracks(recent_tracks.clone()));
	let (followed, albums) = match player {
		Some(player) => (player.followed(50).map(Some), player.albums(50)),
		None => par::join(
			|| optional(client.followed_artists(None, 50)).map(|page| page.map(|page| page.items)),
			|| client.saved_albums(0, 50).map(|page| page.items),
		),
	};
	let followed = followed?.map(|artists| artists.iter().take(SHELF_ITEMS).map(Tile::artist).collect());
	let albums = albums?.iter().take(SHELF_ITEMS).map(Tile::album).collect();
	learn_contexts(s, &played);
	let (mut recents, mut seen) = (Vec::new(), HashSet::new());
	for (track, context) in &played {
		if let Some(context) = context
			&& recents.len() < RECENTS
			&& seen.insert(context.uri.clone())
			&& let Some(tile) = context_tile(s, context, track).or_else(|| s.snapshots.lock().context(context))
		{
			recents.push(tile);
		}
	}
	let h = Home { recents, followed, albums, recent_tracks, loaded: true };
	s.home = Some((Instant::now(), h.clone()));
	Ok(h)
}

pub(super) fn recent_tracks(played: &[(Track, Option<PlayContext>)]) -> Vec<Track> {
	let mut seen = HashSet::new();
	played.iter().filter(|(track, _)| seen.insert(track.uri.as_str())).take(20).map(|(track, _)| track.clone()).collect()
}

/// What the recent plays came from needs the artists and the others' playlists not met before:
/// they are asked for together, so naming the plays one by one finds them known.
fn learn_contexts(s: &mut Session, played: &[(Track, Option<PlayContext>)]) {
	let (mut seen, mut artists, mut playlists) = (HashSet::new(), Vec::new(), Vec::new());
	for context in played.iter().filter_map(|(_, context)| context.as_ref()) {
		if seen.len() >= RECENTS || !seen.insert(context.uri.as_str()) {
			continue;
		}
		let id = id_of(&context.uri);
		match context.kind.as_str() {
			"artist" if s.artists.get_ref(id).is_none() => artists.push(id.to_owned()),
			"playlist"
				if !context.uri.ends_with(":collection") && !s.playlists.iter().any(|p| p.id == id) && s.others.get_ref(id).is_none() =>
			{
				playlists.push(id.to_owned());
			},
			_ => {},
		}
	}
	let client = &s.client;
	let player = s.player.as_ref();
	let (artists, playlists) = par::join(
		|| match player {
			Some(player) => {
				let uris: Vec<_> = artists.iter().map(|id| format!("spotify:artist:{id}")).collect();
				player
					.artists(&uris)
					.unwrap_or_default()
					.into_iter()
					.map(|artist| {
						let id = id_of(&artist.uri).to_owned();
						(Some(artist), id)
					})
					.collect()
			},
			None => par::map(artists, 4, |id| (client.artist(&id).ok(), id)),
		},
		|| {
			par::map(playlists, 4, |id| {
				let playlist = match player {
					Some(player) => player.playlist(&id, 0, false).map(|(p, _, _)| p),
					None => client.playlist(&id),
				};
				(playlist.ok(), id)
			})
		},
	);
	for (artist, id) in artists {
		if let Some(artist) = artist {
			s.cache_artist(id, artist);
		}
	}
	for (playlist, id) in playlists {
		s.cache_other(id, playlist);
	}
}
/// The tile for what `track` was played from, if it can be named.
fn context_tile(s: &mut Session, context: &PlayContext, track: &Track) -> Option<Tile> {
	// Liked Songs is `spotify:user:<id>:collection`, whatever kind Spotify files the play under
	// (it can come as a "playlist", which would resolve to a second Liked Songs tile).
	if context.kind == "collection" || context.uri.ends_with(":collection") {
		return Some(Tile::liked());
	}
	let id = id_of(&context.uri);
	match context.kind.as_str() {
		"playlist" => {
			if let Some(p) = s.playlists.iter().find(|p| p.id == id) {
				return Some(Tile::playlist(p));
			}
			// Someone else's playlist: its session metadata, asked once (Web API before playback connects).
			if s.others.get_ref(id).is_none() {
				let p = match &s.player {
					Some(player) => player.playlist(id, 0, false).map(|(p, _, _)| p),
					None => s.client.playlist(id),
				}
				.ok();
				s.cache_other(id.to_owned(), p);
			}
			// Spotify also plays Liked Songs as a generated playlist of its own (a `37i9dQZF1F…`
			// id, named "Liked Songs", with the Liked Songs artwork): that is the pinned tile.
			s.others
				.get_ref(id)
				.and_then(Option::as_ref)
				.map(|p| if p.images.iter().any(|i| i.url.contains("/liked-songs/")) { Tile::liked() } else { Tile::playlist(p) })
		},
		"album" => track.album.as_ref().filter(|a| a.uri == context.uri).map(Tile::album),
		"artist" => artist(s, id).ok().map(|a| Tile::artist(&a)),
		_ => None,
	}
}
