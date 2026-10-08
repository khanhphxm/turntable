//! Account-scoped stale-while-revalidate data under `<cache>/library/<sha256(user-id)>.json`.
//!
//! Only one account, Home, and eight first-page listings are kept, at most 2 MiB altogether
//! (an atomic replacement can temporarily take another 2 MiB). The identity comes from the
//! stored authorization, never from the cache. Corrupt, oversized or old-version files are
//! ignored; a snapshot is a first paint, not proof that its data or authorization is current.

use std::{
	fs,
	io::{self, Read, Write},
	path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{Home, Listing, Response, Route, Tile};
use crate::{
	app::config,
	spotify::{PlayContext, Playlist, Track, User, id_of},
};

const VERSION: u32 = 1;
const MAX_BYTES: usize = 2 * 1024 * 1024;
pub(super) const LISTINGS_KEPT: usize = 8;

#[derive(Serialize, Deserialize)]
struct Snapshot {
	version:   u32,
	me:        User,
	playlists: Vec<Playlist>,
	home:      Option<Home>,
	/// Independent of Home's short TTL: private top-content reads may use the Web API daily.
	#[serde(default)]
	top:       TopShelves,
	/// Least recently opened first; only playlists, albums and Liked Songs.
	listings:  Vec<SavedListing>,
}

#[derive(Serialize, Deserialize)]
struct SavedListing {
	route:   Route,
	listing: Listing,
}

/// Top artists/tracks change slowly. Persist attempts as well as successful results, so
/// restarting after a rate limit or an outage cannot issue the same reads again that day.
#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct TopShelves {
	pub(super) checked_at: Option<u64>,
	pub(super) artists:    Option<Vec<Tile>>,
	pub(super) tracks:     Option<Vec<Track>>,
}

impl TopShelves {
	pub(super) fn fresh(&self, now: u64) -> bool {
		// Clock rollback must not permit another attempt inside the same 24-hour window.
		self.checked_at.is_some_and(|at| now.saturating_sub(at) < 24 * 60 * 60)
	}
}

pub(super) struct Store {
	dir:     PathBuf,
	data:    Option<Snapshot>,
	/// Sign-out disables writes from already queued requests until the next connection.
	enabled: bool,
}

impl Store {
	pub(super) fn new(cache: &Path) -> Self {
		Self { dir: cache.join("library"), data: None, enabled: true }
	}

	fn path(&self, user_id: &str) -> PathBuf {
		self.dir.join(format!("{:x}.json", Sha256::digest(user_id.as_bytes())))
	}

	/// A new authorization without a verified identity cannot borrow the previous account's data.
	pub(super) fn load(&mut self, user_id: Option<&str>) {
		self.enabled = true;
		self.data = user_id.and_then(|id| self.read(id));
	}

	fn read(&self, user_id: &str) -> Option<Snapshot> {
		let file = fs::File::open(self.path(user_id)).ok()?;
		let cap = u64::try_from(MAX_BYTES).unwrap_or(u64::MAX);
		if file.metadata().ok()?.len() > cap {
			return None;
		}
		let mut bytes = Vec::new();
		file.take(cap + 1).read_to_end(&mut bytes).ok()?;
		if bytes.len() > MAX_BYTES {
			return None;
		}
		let mut snapshot: Snapshot = serde_json::from_slice(&bytes).ok()?;
		if snapshot.version != VERSION || snapshot.me.id != user_id || snapshot.listings.len() > LISTINGS_KEPT {
			return None;
		}
		for playlist in &mut snapshot.playlists {
			playlist.normalize(&snapshot.me.id, snapshot.me.display_name.as_deref());
		}
		Some(snapshot)
	}

	pub(super) fn connected(&self) -> Option<Response> {
		let data = self.data.as_ref()?;
		Some(Response::Connected { me: data.me.clone(), playlists: data.playlists.clone() })
	}

	pub(super) fn home(&self) -> Option<Home> {
		self.data.as_ref()?.home.clone()
	}

	pub(super) fn top(&self, user: &str) -> TopShelves {
		let Some(data) = self.data.as_ref().filter(|data| data.me.id == user) else { return TopShelves::default() };
		if data.top.checked_at.is_none()
			&& let Some(home) = &data.home
		{
			// Snapshots from before the daily cache still have useful shelf contents.
			return TopShelves { artists: home.top_artists.clone(), tracks: home.top_tracks.clone(), ..TopShelves::default() };
		}
		data.top.clone()
	}

	pub(super) fn record_top(&mut self, user: &str, top: TopShelves) {
		if self.enabled
			&& let Some(data) = self.data.as_mut().filter(|data| data.me.id == user)
		{
			data.top = top;
		}
	}

	/// Leaves an unknown recent tile in place while its metadata is being revalidated.
	pub(super) fn context(&self, context: &PlayContext) -> Option<Tile> {
		let id = id_of(&context.uri);
		self.data
			.as_ref()?
			.home
			.as_ref()?
			.recents
			.iter()
			.find(|tile| match (context.kind.as_str(), &tile.route) {
				("playlist", Route::Playlist(known)) | ("artist", Route::Artist(known)) => known == id,
				_ => false,
			})
			.cloned()
	}

	pub(super) fn listing(&mut self, route: &Route) -> Option<Listing> {
		let listings = &mut self.data.as_mut()?.listings;
		let index = listings.iter().position(|saved| &saved.route == route)?;
		let saved = listings.remove(index);
		let listing = saved.listing.clone();
		listings.push(saved);
		Some(listing)
	}

	/// Updates only the data worth persisting; unrelated responses do not cause disk writes.
	pub(super) fn record(&mut self, response: &Response) -> bool {
		if !self.enabled {
			return false;
		}
		match response {
			Response::Connected { me, playlists } => {
				if let Some(data) = &mut self.data
					&& data.me.id == me.id
				{
					data.me = me.clone();
					data.playlists = playlists.clone();
				} else {
					self.data = Some(Snapshot {
						version:   VERSION,
						me:        me.clone(),
						playlists: playlists.clone(),
						home:      None,
						listings:  Vec::new(),
						top:       TopShelves::default(),
					});
				}
			},
			Response::Home(home) => {
				let Some(data) = &mut self.data else { return false };
				data.home = Some(home.clone());
			},
			Response::Listing { route: route @ (Route::Playlist(_) | Route::Album(_) | Route::Liked), offset: 0, listing } => {
				let Some(data) = &mut self.data else { return false };
				data.listings.retain(|saved| &saved.route != route);
				if data.listings.len() == LISTINGS_KEPT {
					data.listings.remove(0);
				}
				data.listings.push(SavedListing { route: route.clone(), listing: listing.clone() });
			},
			Response::Playlists(playlists) => {
				let Some(data) = &mut self.data else { return false };
				data.playlists = playlists.clone();
			},
			Response::Created(playlist) => {
				let Some(data) = &mut self.data else { return false };
				data.playlists.insert(0, playlist.clone());
			},
			_ => return false,
		}
		true
	}

	/// Writes after delivery, so filesystem latency never holds up fresh data's first paint.
	pub(super) fn save(&mut self) -> io::Result<()> {
		let Some(data) = &self.data else { return Ok(()) };
		let path = self.path(&data.me.id);
		let Some(data) = &mut self.data else { return Ok(()) };
		let bytes = loop {
			match encode(data) {
				Ok(bytes) => break bytes,
				Err(_) if !data.listings.is_empty() => {
					data.listings.remove(0);
				},
				Err(error) => return Err(error),
			}
		};
		fs::create_dir_all(&self.dir)?;
		// This directory belongs to the snapshot store. Keeping only the current account bounds
		// the total, not just each file, and discards abandoned atomic replacements as well.
		for entry in fs::read_dir(&self.dir)? {
			let entry = entry?;
			if entry.path() != path && entry.file_type()?.is_file() {
				fs::remove_file(entry.path())?;
			}
		}
		config::write_private(&path, &bytes)
	}

	/// A successful unfollow must survive even when the following refresh fails.
	/// Remove only this playlist's library entry and saved page; keep the account,
	/// unrelated playlists and other cached pages intact.
	pub(super) fn remove_playlist(&mut self, id: &str) {
		if let Some(data) = &mut self.data {
			data.playlists.retain(|playlist| playlist.id != id);
			data.listings.retain(|saved| !matches!(&saved.route, Route::Playlist(known) if known == id));
		}
		let _ = self.save();
	}

	pub(super) fn forget(&mut self, route: &Route) {
		if let Some(data) = &mut self.data {
			data.listings.retain(|saved| &saved.route != route);
		}
		let _ = self.save();
	}

	pub(super) fn forget_listings(&mut self) {
		if let Some(data) = &mut self.data {
			data.listings.clear();
			data.home = None;
		}
		let _ = self.save();
	}

	pub(super) fn clear(&mut self) {
		self.data = None;
		self.enabled = false;
		let _ = remove(&self.dir);
	}
}

/// Both the player's sign-out and the Web API's Disconnect forget library snapshots.
pub(crate) fn clear(cache: &Path) -> io::Result<()> {
	remove(&cache.join("library"))
}

fn remove(dir: &Path) -> io::Result<()> {
	match fs::remove_dir_all(dir) {
		Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
		result => result,
	}
}

/// Refuses an oversized snapshot while serializing, rather than allocating its full JSON first.
fn encode(data: &Snapshot) -> io::Result<Vec<u8>> {
	struct Bounded(Vec<u8>);
	impl Write for Bounded {
		fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
			if bytes.len() > MAX_BYTES.saturating_sub(self.0.len()) {
				return Err(io::Error::new(io::ErrorKind::InvalidData, "library snapshot exceeds its size cap"));
			}
			self.0.extend_from_slice(bytes);
			Ok(bytes.len())
		}

		fn flush(&mut self) -> io::Result<()> {
			Ok(())
		}
	}
	let mut writer = Bounded(Vec::new());
	serde_json::to_writer(&mut writer, data).map_err(io::Error::other)?;
	Ok(writer.0)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn scratch(name: &str) -> PathBuf {
		let dir = std::env::temp_dir().join(format!("turntable-snapshot-{name}-{}", std::process::id()));
		let _ = fs::remove_dir_all(&dir);
		fs::create_dir_all(&dir).unwrap();
		dir
	}

	fn connect(store: &mut Store, user: &str) {
		let playlist = serde_json::from_value(serde_json::json!({
			"id": "p", "name": "Road trip", "owner": { "id": user }, "items": { "total": 51 },
			"images": [{ "url": "https://cover", "width": 300, "height": 300 }]
		}))
		.unwrap();
		store.record(&Response::Connected {
			me:        User { id: user.into(), display_name: Some("Alex".into()) },
			playlists: vec![playlist],
		});
	}

	#[test]
	fn snapshot_round_trip_preserves_home_tracks_playlist_counts_and_first_pages() {
		let dir = scratch("round-trip");
		let mut store = Store::new(&dir);
		connect(&mut store, "alex");
		let track = serde_json::from_value(serde_json::json!({
			"uri": "spotify:episode:e", "name": "Episode", "type": "episode",
			"show": { "name": "Show", "images": [{ "url": "https://show" }] }
		}))
		.unwrap();
		store.record(&Response::Home(Home { recents: vec![Tile::liked()], recent_tracks: vec![track], loaded: true, ..Home::default() }));
		let listing = Listing {
			kicker: ("Playlist").into(),
			title: "Liked Songs".into(),
			total: 51,
			next_offset: Some(50),
			accent: Some("#123456".into()),
			..Listing::default()
		};
		store.record(&Response::Listing { route: Route::Liked, offset: 0, listing });
		store.save().unwrap();
		let path = store.path("alex");
		#[cfg(unix)]
		{
			use std::os::unix::fs::PermissionsExt;
			assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
		}
		let mut restored = Store::new(&dir);
		restored.load(Some("alex"));
		let Response::Connected { me, playlists } = restored.connected().unwrap() else { panic!("missing profile") };
		assert_eq!(me.id, "alex");
		assert_eq!(playlists.first().unwrap().track_count, Some(51));
		assert_eq!(playlists.first().unwrap().images.first().unwrap().url, "https://cover");
		let home = restored.home().unwrap();
		assert!(home.loaded && home.recent_tracks.first().unwrap().is_episode);
		assert_eq!(home.recents, [Tile::liked()]);
		let listing = restored.listing(&Route::Liked).unwrap();
		assert_eq!((listing.kicker.as_ref(), listing.total, listing.next_offset), ("Playlist", 51, Some(50)));
		assert_eq!(listing.accent.as_deref(), Some("#123456"));
		assert!(restored.listing(&Route::Album("missing".into())).is_none());
		assert!(!path.with_extension("tmp").exists(), "atomic replacement leaves no temporary file");
		fs::remove_dir_all(dir).unwrap();
	}

	#[test]
	fn snapshots_never_cross_accounts_or_a_new_authorization() {
		let dir = scratch("accounts");
		let mut store = Store::new(&dir);
		connect(&mut store, "alex");
		store.save().unwrap();
		let alex = store.path("alex");
		store.load(Some("bo"));
		assert!(store.connected().is_none() && store.home().is_none());
		store.load(None);
		assert!(store.connected().is_none(), "an unbound browser login cannot use the last account");
		// Even a file moved under another account's key must not be shown.
		fs::copy(&alex, store.path("bo")).unwrap();
		store.load(Some("bo"));
		assert!(store.connected().is_none());
		connect(&mut store, "bo");
		store.save().unwrap();
		assert!(!alex.exists(), "only one account takes disk space");
		store.clear();
		assert!(!dir.join("library").exists());
		connect(&mut store, "alex");
		store.save().unwrap();
		assert!(!dir.join("library").exists(), "a queued response cannot recreate a signed-out snapshot");
		fs::remove_dir_all(dir).unwrap();
	}

	#[test]
	fn listings_keep_only_eight_recent_first_pages_and_obey_the_byte_cap() {
		let dir = scratch("bounds");
		let mut store = Store::new(&dir);
		connect(&mut store, "alex");
		for id in 0..10 {
			store.record(&Response::Listing { route: Route::Album(id.to_string()), offset: 0, listing: Listing::default() });
		}
		assert!(store.listing(&Route::Album("0".into())).is_none());
		assert!(store.listing(&Route::Album("2".into())).is_some());
		store.record(&Response::Listing { route: Route::Liked, offset: 50, listing: Listing::default() });
		assert!(store.listing(&Route::Liked).is_none(), "later pages are not snapshots");
		store.record(&Response::Listing {
			route:   Route::Liked,
			offset:  0,
			listing: Listing { title: "x".repeat(MAX_BYTES), ..Listing::default() },
		});
		store.save().unwrap();
		assert!(store.listing(&Route::Liked).is_none(), "oversized pages are evicted");
		assert!(fs::metadata(store.path("alex")).unwrap().len() <= u64::try_from(MAX_BYTES).unwrap());
		fs::write(store.path("alex"), b"{broken").unwrap();
		store.load(Some("alex"));
		assert!(store.connected().is_none());
		fs::write(store.path("alex"), vec![b' '; MAX_BYTES + 1]).unwrap();
		store.load(Some("alex"));
		assert!(store.connected().is_none());
		fs::remove_dir_all(dir).unwrap();
	}

	#[test]
	fn offline_errors_finish_loading_without_losing_disk_data() {
		let dir = scratch("offline");
		let mut store = Store::new(&dir);
		connect(&mut store, "alex");
		store.record(&Response::Home(Home { recents: vec![Tile::liked()], loaded: true, ..Home::default() }));
		store.record(&Response::Listing {
			route:   Route::Liked,
			offset:  0,
			listing: Listing { title: "Saved liked songs".into(), total: 51, ..Listing::default() },
		});
		store.save().unwrap();
		let mut restored = Store::new(&dir);
		restored.load(Some("alex"));
		let mut library = crate::library::Library::default();
		library.apply(restored.connected().unwrap());
		library.apply(Response::Home(restored.home().unwrap()));
		library.apply(Response::Error { route: None, message: "simulated offline".into() });
		assert_eq!(library.conn, crate::library::Conn::Ready);
		assert!(library.home.loaded);
		assert_eq!(library.home.recents, [Tile::liked()]);
		library.navigate(Route::Liked);
		library.apply(Response::Listing { route: Route::Liked, offset: 0, listing: restored.listing(&Route::Liked).unwrap() });
		library.apply(Response::Error { route: Some(Route::Liked), message: "simulated offline".into() });
		let listing = library.listing.unwrap();
		assert!(!listing.loading);
		assert_eq!((listing.title.as_str(), listing.total), ("Saved liked songs", 51));
		let mut empty = crate::library::Library::default();
		empty.navigate(Route::Home);
		empty.apply(Response::Error { route: None, message: "simulated offline".into() });
		assert!(empty.home.loaded, "even an uncached Home must stop spinning");
		fs::remove_dir_all(dir).unwrap();
	}

	#[test]
	fn daily_top_cache_survives_restarts_and_is_account_scoped() {
		let dir = scratch("daily-top");
		let mut store = Store::new(&dir);
		connect(&mut store, "alex");
		store.record_top("alex", TopShelves {
			checked_at: Some(100_000),
			artists:    Some(vec![Tile::liked()]),
			tracks:     Some(Vec::new()),
		});
		store.save().unwrap();
		let mut restored = Store::new(&dir);
		restored.load(Some("alex"));
		let top = restored.top("alex");
		assert!(top.fresh(100_000 + 24 * 60 * 60 - 1));
		assert!(!top.fresh(100_000 + 24 * 60 * 60));
		assert!(top.fresh(99_999), "clock rollback does not cause another API attempt");
		assert_eq!(top.artists.unwrap(), [Tile::liked()]);
		assert_eq!(top.tracks, Some(Vec::new()));
		assert!(restored.top("bo").checked_at.is_none());
		restored.record_top("bo", TopShelves { checked_at: Some(200_000), ..TopShelves::default() });
		assert_eq!(restored.top("alex").checked_at, Some(100_000));
		restored.clear();
		assert!(restored.top("alex").checked_at.is_none());
		fs::remove_dir_all(dir).unwrap_or_else(|error| assert_eq!(error.kind(), io::ErrorKind::NotFound));
	}

	#[test]
	fn playlist_removal_persists_before_refresh_without_touching_other_entries() {
		let dir = scratch("remove-playlist");
		let mut store = Store::new(&dir);
		connect(&mut store, "alex");
		let scratch: Playlist = serde_json::from_value(serde_json::json!({
			"id": "scratch", "name": "turntable test", "owner": { "id": "alex" }
		}))
		.unwrap();
		store.record(&Response::Created(scratch));
		for id in ["p", "scratch"] {
			store.record(&Response::Listing {
				route:   Route::Playlist(id.into()),
				offset:  0,
				listing: Listing { title: id.into(), ..Listing::default() },
			});
		}
		store.save().unwrap();
		store.remove_playlist("scratch");
		let mut restored = Store::new(&dir);
		restored.load(Some("alex"));
		let Response::Connected { me, playlists } = restored.connected().unwrap() else { panic!() };
		assert_eq!(me.id, "alex");
		assert_eq!(playlists.len(), 1);
		assert_eq!(playlists[0].id, "p");
		assert!(restored.listing(&Route::Playlist("scratch".into())).is_none());
		assert_eq!(restored.listing(&Route::Playlist("p".into())).unwrap().title, "p");
		fs::remove_dir_all(dir).unwrap();
	}

	#[test]
	fn cached_spotify_playlist_owner_has_the_known_display_name() {
		let dir = scratch("spotify-owner");
		let mut store = Store::new(&dir);
		connect(&mut store, "alex");
		let spotify: Playlist = serde_json::from_value(serde_json::json!({
			"id": "top", "name": "Your All-Time Top Songs", "owner": { "id": "spotify", "display_name": null }
		}))
		.unwrap();
		store.record(&Response::Created(spotify));
		store.save().unwrap();
		let mut restored = Store::new(&dir);
		restored.load(Some("alex"));
		let Response::Connected { playlists, .. } = restored.connected().unwrap() else { panic!() };
		assert_eq!(playlists[0].owner.display_name.as_deref(), Some("Spotify"));
		assert_eq!(super::super::Entry::Playlist(&playlists[0]).sub(), "Playlist • Spotify");
		fs::remove_dir_all(dir).unwrap();
	}
}
