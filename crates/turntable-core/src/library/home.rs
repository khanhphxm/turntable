//! Home shelves and the metadata that names their recently played contexts.

use std::{
	collections::HashSet,
	time::{Duration, Instant},
};

use super::{Home, Tile, artist::artist, pages::optional, worker::Session};
use crate::{
	net::par,
	spotify::{self, PlayContext, Track, id_of},
};

/// How long a built home page is reused (navigating back and forth costs no requests).
const HOME_TTL: Duration = Duration::from_secs(60);
/// Recently played playlists/albums/artists shown on the home page.
const RECENTS: usize = 12;
/// Items per home shelf fetched.
const SHELF: u32 = 12;
/// Shelf items shown from a longer page.
const SHELF_ITEMS: usize = 12;

pub(super) fn home(s: &mut Session) -> spotify::Result<Home> {
	if let Some((at, h)) = &s.home
		&& at.elapsed() < HOME_TTL
	{
		return Ok(h.clone());
	}
	let client = &s.client;
	let player = &mut s.player;
	// Nothing on the page waits for anything else on it: all of it is asked for together. The first
	// pages of the followed artists and the saved albums are the ones Your Library starts from, so
	// they are asked for whole and the shelves take their first items.
	let (played, (top_artists, (top_tracks, (followed, albums)))) = par::join(
		|| client.recently_played(50),
		|| {
			par::join(
				|| optional(client.top_artists(SHELF)),
				|| {
					par::join(
						|| optional(client.top_tracks(SHELF)),
						|| match player {
							Some(player) => {
								let followed = player.followed(50).map(Some);
								(followed, player.albums(50))
							},
							None => par::join(
								|| optional(client.followed_artists(None, 50)).map(|p| p.map(|p| p.items)),
								|| client.saved_albums(0, 50).map(|p| p.items),
							),
						},
					)
				},
			)
		},
	);
	let played = played?;
	let top_artists = top_artists?.map(|a| a.iter().map(Tile::artist).collect());
	let top_tracks = top_tracks?;
	let followed = followed?.map(|artists| artists.iter().take(SHELF_ITEMS).map(Tile::artist).collect());
	let albums = albums?.iter().take(SHELF_ITEMS).map(Tile::album).collect();
	learn_contexts(s, &played);
	let (mut recents, mut seen) = (Vec::new(), HashSet::new());
	let (mut recent_tracks, mut seen_tracks) = (Vec::new(), HashSet::new());
	for (track, context) in &played {
		if recent_tracks.len() < 20 && seen_tracks.insert(track.uri.clone()) {
			recent_tracks.push(track.clone());
		}
		if let Some(context) = context
			&& recents.len() < RECENTS
			&& seen.insert(context.uri.clone())
			&& let Some(tile) = context_tile(s, context, track)
				.or_else(|| s.snapshots.lock().unwrap_or_else(std::sync::PoisonError::into_inner).context(context))
		{
			recents.push(tile);
		}
	}
	let h = Home { recents, top_artists, top_tracks, followed, albums, recent_tracks, loaded: true };
	s.home = Some((Instant::now(), h.clone()));
	Ok(h)
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
			"artist" if !s.artists.contains_key(id) => artists.push(id.to_owned()),
			"playlist"
				if !context.uri.ends_with(":collection") && !s.playlists.iter().any(|p| p.id == id) && !s.others.contains_key(id) =>
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
			s.artists.insert(id, artist);
		}
	}
	for (playlist, id) in playlists {
		s.others.insert(id, playlist);
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
			if !s.others.contains_key(id) {
				let p = match &s.player {
					Some(player) => player.playlist(id, 0, false).map(|(p, _, _)| p),
					None => s.client.playlist(id),
				}
				.ok();
				s.others.insert(id.to_owned(), p);
			}
			// Spotify also plays Liked Songs as a generated playlist of its own (a `37i9dQZF1F…`
			// id, named "Liked Songs", with the Liked Songs artwork): that is the pinned tile.
			s.others
				.get(id)
				.and_then(Option::as_ref)
				.map(|p| if p.images.iter().any(|i| i.url.contains("/liked-songs/")) { Tile::liked() } else { Tile::playlist(p) })
		},
		"album" => track.album.as_ref().filter(|a| a.uri == context.uri).map(Tile::album),
		"artist" => artist(s, id).ok().map(|a| Tile::artist(&a)),
		_ => None,
	}
}
