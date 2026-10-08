//! Session-first Home shelves. Recent contexts use native history (latest song per context),
//! followed artists and saved albums use collection2. Web API reads remain fallback;
//! top artists/tracks are persisted per account and attempted only once every 24 hours.

use std::{
	collections::HashSet,
	sync::{Mutex, PoisonError},
	time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use super::{
	Home, Tile,
	artist::artist,
	pages::optional,
	snapshot::{Store, TopShelves},
	worker::Session,
};
use crate::{
	net::par,
	spotify::{self, Client, PlayContext, Track, id_of},
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
	let (top, (played, (followed, albums))) = par::join(
		|| top_shelves(client, &s.snapshots, &s.me),
		|| {
			let played = match player.as_ref() {
				Some(player) => player.recently_played(50).or_else(|_| client.recently_played(50)),
				None => client.recently_played(50),
			};
			let library = match player {
				Some(player) => {
					let followed = player
						.followed(50)
						.map(Some)
						.or_else(|_| optional(client.followed_artists(None, 50)).map(|page| page.map(|page| page.items)));
					let albums = player.albums(50).or_else(|_| client.saved_albums(0, 50).map(|page| page.items));
					(followed, albums)
				},
				None => par::join(
					|| optional(client.followed_artists(None, 50)).map(|page| page.map(|page| page.items)),
					|| client.saved_albums(0, 50).map(|page| page.items),
				),
			};
			(played, library)
		},
	);
	let played = played?;
	let top_artists = top.artists;
	let top_tracks = top.tracks;
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
			&& let Some(tile) =
				context_tile(s, context, track).or_else(|| s.snapshots.lock().unwrap_or_else(PoisonError::into_inner).context(context))
		{
			recents.push(tile);
		}
	}
	let h = Home { recents, top_artists, top_tracks, followed, albums, recent_tracks, loaded: true };
	s.home = Some((Instant::now(), h.clone()));
	Ok(h)
}

fn top_shelves(client: &Client, snapshots: &Mutex<Store>, me: &str) -> TopShelves {
	let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_secs());
	let mut top = snapshots.lock().unwrap_or_else(PoisonError::into_inner).top(me);
	if top.fresh(now) || !client.web_api_enabled() {
		return top;
	}
	// Persist the attempt before I/O. An unavailable endpoint retains yesterday's content,
	// and neither another Home refresh nor another launch can spend the quota again today.
	top.checked_at = Some(now);
	{
		let mut store = snapshots.lock().unwrap_or_else(PoisonError::into_inner);
		store.record_top(me, top.clone());
		let _ = store.save();
	}
	let (artists, tracks) = par::join(|| client.top_artists(SHELF), || client.top_tracks(SHELF));
	if let Ok(artists) = artists {
		top.artists = Some(artists.iter().map(Tile::artist).collect());
	}
	if let Ok(tracks) = tracks {
		top.tracks = Some(tracks);
	}
	{
		let mut store = snapshots.lock().unwrap_or_else(PoisonError::into_inner);
		store.record_top(me, top.clone());
		let _ = store.save();
	}
	top
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
			if !s.others.contains_key(id) {
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
				.get(id)
				.and_then(Option::as_ref)
				.map(|p| if p.images.iter().any(|i| i.url.contains("/liked-songs/")) { Tile::liked() } else { Tile::playlist(p) })
		},
		"album" => track.album.as_ref().filter(|a| a.uri == context.uri).map(Tile::album),
		"artist" => artist(s, id).ok().map(|a| Tile::artist(&a)),
		_ => None,
	}
}

#[cfg(test)]
mod tests {
	use std::sync::{
		Arc,
		atomic::{AtomicU32, Ordering},
	};

	use super::*;
	use crate::{
		library::Response,
		spotify::{TokenSource, User},
	};

	#[test]
	fn cache_only_top_reads_preserve_shelves_without_consuming_a_daily_attempt() {
		let dir = tempfile::tempdir().unwrap();
		let mut store = Store::new(dir.path());
		store.record(&Response::Connected { me: User { id: "u".into(), display_name: None }, playlists: Vec::new() });
		store.record_top("u", TopShelves { artists: Some(vec![Tile::liked()]), ..TopShelves::default() });
		let snapshots = Mutex::new(store);
		let minted = Arc::new(AtomicU32::new(0));
		let counter = minted.clone();
		let source: TokenSource = Arc::new(move || {
			counter.fetch_add(1, Ordering::Relaxed);
			Err(spotify::Error::AuthRequired)
		});
		let client = Client::open(source, false);
		let top = top_shelves(&client, &snapshots, "u");
		assert!(top.checked_at.is_none());
		assert_eq!(top.artists.unwrap(), [Tile::liked()]);
		assert!(snapshots.into_inner().unwrap().top("u").checked_at.is_none());
		assert_eq!(minted.load(Ordering::Relaxed), 0);
	}

	#[test]
	fn a_failed_daily_attempt_is_persisted_and_not_repeated_after_restart() {
		let dir = tempfile::tempdir().unwrap();
		let mut store = Store::new(dir.path());
		store.record(&Response::Connected { me: User { id: "u".into(), display_name: None }, playlists: Vec::new() });
		let snapshots = Mutex::new(store);
		let minted = Arc::new(AtomicU32::new(0));
		let counter = minted.clone();
		let source: TokenSource = Arc::new(move || {
			counter.fetch_add(1, Ordering::Relaxed);
			Err(spotify::Error::AuthRequired)
		});
		let client = Client::open(source, true);
		let first = top_shelves(&client, &snapshots, "u");
		assert!(first.checked_at.is_some());
		let attempts = minted.load(Ordering::Relaxed);
		assert_eq!(attempts, 2, "one token attempt for each top shelf, with no network");
		let mut restored = Store::new(dir.path());
		restored.load(Some("u"));
		let again = top_shelves(&client, &Mutex::new(restored), "u");
		assert_eq!(again.checked_at, first.checked_at);
		assert_eq!(minted.load(Ordering::Relaxed), attempts);
	}
}
