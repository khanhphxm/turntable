//! Account-scoped stale-while-revalidate data under `<cache>/library/<sha256(user-id)>.json`.
//!
//! Only one account, native Home, eight first-page listings and eight searches are kept,
//! at most 2 MiB altogether. Encoding holds the state lock; disk writes do not and never fsync
//! (an atomic replacement can temporarily take another 2 MiB). The identity comes from the
//! stored authorization, never from the cache. Corrupt, oversized or old-version files are
//! ignored; a snapshot is a first paint, not proof that its data or authorization is current.

use std::{
	fs,
	io::{self, Read, Write},
	path::{Path, PathBuf},
	sync::{
		Arc,
		atomic::{AtomicU64, Ordering},
	},
};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::{Home, Listing, Response, Route, Tile};
use crate::{
	app::config,
	spotify::{PlayContext, Playlist, SearchResults, Track, User, id_of},
};

const VERSION: u32 = 1;
const MAX_BYTES: usize = 2 * 1024 * 1024;
pub(super) const LISTINGS_KEPT: usize = 8;

#[derive(Serialize, Deserialize)]
struct Snapshot {
	version:   u32,
	me:        User,
	playlists: Vec<Playlist>,
	#[serde(default)]
	rootlist:  Vec<super::RootEntry>,
	home:      Option<Home>,
	/// Least recently opened first; playlists, albums, artists and Liked Songs.
	listings:  Vec<SavedListing>,
	#[serde(default)]
	searches:  Vec<SavedSearch>,
}

#[derive(Serialize, Deserialize)]
struct SavedListing {
	route:   Route,
	listing: Listing,
}

#[derive(Serialize, Deserialize)]
struct SavedSearch {
	query:   String,
	results: SearchResults,
}

pub(super) struct Store {
	dir:        PathBuf,
	data:       Option<Snapshot>,
	/// Sign-out disables writes from already queued requests until the next connection.
	enabled:    bool,
	generation: Option<(Arc<AtomicU64>, u64)>,
}

impl Store {
	pub(super) fn new(cache: &Path) -> Self {
		Self { dir: cache.join("library"), data: None, enabled: true, generation: None }
	}

	pub(super) fn bind_generation(&mut self, live: Arc<AtomicU64>, generation: u64) {
		self.generation = Some((live, generation));
	}

	fn current(&self) -> bool {
		self.generation.as_ref().is_none_or(|(live, generation)| live.load(Ordering::Acquire) == *generation)
	}

	fn path(&self, user_id: &str) -> PathBuf {
		self.dir.join(format!("{}.json", crate::sha256_hex(user_id.as_bytes())))
	}

	/// A new authorization without a verified identity cannot borrow the previous account's data.
	pub(super) fn load(&mut self, user_id: Option<&str>) {
		if !self.current() {
			return;
		}
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
		if snapshot.version != VERSION
			|| snapshot.me.id != user_id
			|| snapshot.listings.len() > LISTINGS_KEPT
			|| snapshot.searches.len() > LISTINGS_KEPT
		{
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

	pub(super) fn rootlist(&self) -> Option<Response> {
		let data = self.data.as_ref()?;
		Some(Response::Rootlist { account: data.me.id.clone(), entries: data.rootlist.clone() })
	}

	pub(super) fn home(&self) -> Option<Home> {
		self.data.as_ref()?.home.clone()
	}

	/// Last-played metadata for offline startup, preferring the checkpoint's exact track.
	pub(super) fn recent_track(&self, account: &str, uri: Option<&str>) -> Option<Track> {
		let tracks = &self.data.as_ref().filter(|data| data.me.id == account)?.home.as_ref()?.recent_tracks;
		uri.and_then(|uri| tracks.iter().find(|track| track.uri == uri)).or_else(|| tracks.first()).cloned()
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

	pub(super) fn search(&mut self, query: &str) -> Option<SearchResults> {
		let searches = &mut self.data.as_mut()?.searches;
		let index = searches.iter().position(|saved| saved.query == query.trim())?;
		let saved = searches.remove(index);
		let results = saved.results.clone();
		searches.push(saved);
		Some(results)
	}

	/// Updates only the data worth persisting; unrelated responses do not cause disk writes.
	pub(super) fn record(&mut self, response: &Response) -> bool {
		if !self.enabled || !self.current() {
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
						rootlist:  Vec::new(),
						home:      None,
						listings:  Vec::new(),
						searches:  Vec::new(),
					});
				}
			},
			Response::Rootlist { account, entries } => {
				let Some(data) = &mut self.data else { return false };
				if data.me.id != *account {
					return false;
				}
				data.rootlist = entries.clone();
			},
			Response::Home(home) => {
				let Some(data) = &mut self.data else { return false };
				data.home = Some(home.clone());
			},
			Response::HomeRecentTracks(tracks) => {
				let Some(data) = &mut self.data else { return false };
				data.home.get_or_insert_with(Home::default).take_recent_tracks(tracks.clone());
			},
			Response::Listing {
				route: route @ (Route::Playlist(_) | Route::Album(_) | Route::Artist(_) | Route::Liked),
				offset: 0,
				listing,
			} => {
				let Some(data) = &mut self.data else { return false };
				data.listings.retain(|saved| &saved.route != route);
				if data.listings.len() == LISTINGS_KEPT {
					data.listings.remove(0);
				}
				data.listings.push(SavedListing { route: route.clone(), listing: listing.clone() });
			},
			Response::Search { query, results } => {
				let Some(data) = &mut self.data else { return false };
				data.searches.retain(|saved| saved.query != query.trim());
				if data.searches.len() == LISTINGS_KEPT {
					data.searches.remove(0);
				}
				data.searches.push(SavedSearch { query: query.trim().to_owned(), results: results.clone() });
			},
			Response::ArtistPopular(id, popular) => {
				let Some(listing) = self.saved_listing(&Route::Artist(id.clone())) else { return false };
				listing.tracks.clone_from(&popular.tracks);
				listing.plays.clone_from(&popular.plays);
				listing.accent.clone_from(&popular.accent);
				listing.popular_loaded = true;
				listing.take_artist_overview(popular.overview.clone());
			},
			Response::Accent(route, accent) => {
				let Some(listing) = self.saved_listing(route) else { return false };
				listing.accent = Some(accent.clone());
			},
			Response::Plays(route, plays) => {
				let Some(listing) = self.saved_listing(route) else { return false };
				listing.plays =
					listing.tracks.iter().map(|track| plays.get(id_of(&track.uri)).copied()).collect::<Option<_>>().unwrap_or_default();
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

	fn saved_listing(&mut self, route: &Route) -> Option<&mut Listing> {
		self.data.as_mut()?.listings.iter_mut().find(|saved| &saved.route == route).map(|saved| &mut saved.listing)
	}

	/// Encodes a bounded cache under the state lock; the caller owns off-lock disk I/O.
	fn prepare(&mut self) -> io::Result<Option<PendingWrite>> {
		if !self.enabled || !self.current() {
			return Ok(None);
		}
		let Some(data) = &self.data else { return Ok(None) };
		let path = self.path(&data.me.id);
		let Some(data) = &mut self.data else { return Ok(None) };
		let bytes = loop {
			match encode(data) {
				Ok(bytes) => break bytes,
				Err(_) if !data.listings.is_empty() => {
					data.listings.remove(0);
				},
				Err(_) if !data.searches.is_empty() => {
					data.searches.remove(0);
				},
				Err(error) => return Err(error),
			}
		};
		Ok(Some(PendingWrite { dir: self.dir.clone(), path, bytes, generation: self.generation.clone() }))
	}

	#[cfg(test)]
	pub(super) fn save(&mut self) -> io::Result<()> {
		if let Some(write) = self.prepare()? {
			let temporary = write.stage()?;
			if write.current() {
				fs::rename(temporary, write.path)?;
			} else {
				let _ = fs::remove_file(temporary);
			}
		}
		Ok(())
	}

	/// A successful unfollow must survive even when the following refresh fails.
	/// Remove only this playlist's library entry and saved page; keep the account,
	/// unrelated playlists and other cached pages intact.
	pub(super) fn remove_playlist(&mut self, id: &str) {
		if let Some(data) = &mut self.data {
			data.playlists.retain(|playlist| playlist.id != id);
			super::folders::remove_playlist(&mut data.rootlist, id);
			data.listings.retain(|saved| !matches!(&saved.route, Route::Playlist(known) if known == id));
		}
	}

	pub(super) fn forget(&mut self, route: &Route) {
		if let Some(data) = &mut self.data {
			data.listings.retain(|saved| &saved.route != route);
		}
	}

	pub(super) fn forget_listings(&mut self) {
		if let Some(data) = &mut self.data {
			data.listings.clear();
			data.searches.clear();
			data.home = None;
		}
	}

	pub(super) fn clear(&mut self) {
		self.invalidate();
		let _ = remove(&self.dir);
	}

	/// Invalidates account memory immediately; the worker removes the directory off-thread.
	pub(super) fn invalidate(&mut self) {
		self.data = None;
		self.enabled = false;
	}
}

struct PendingWrite {
	dir:        PathBuf,
	path:       PathBuf,
	bytes:      Vec<u8>,
	generation: Option<(Arc<AtomicU64>, u64)>,
}

impl PendingWrite {
	fn current(&self) -> bool {
		self.generation.as_ref().is_none_or(|(live, generation)| live.load(Ordering::Acquire) == *generation)
	}

	fn stage(&self) -> io::Result<PathBuf> {
		fs::create_dir_all(&self.dir)?;
		for entry in fs::read_dir(&self.dir)? {
			let entry = entry?;
			if entry.path() != self.path && entry.file_type()?.is_file() {
				fs::remove_file(entry.path())?;
			}
		}
		config::stage_cache(&self.path, &self.bytes)
	}

	fn commit(self, store: &Mutex<Store>, temporary: PathBuf) -> io::Result<()> {
		let guard = store.lock();
		if guard.enabled
			&& guard.current()
			&& self.current()
			&& guard.generation.as_ref().map(|(_, generation)| generation) == self.generation.as_ref().map(|(_, generation)| generation)
		{
			fs::rename(temporary, self.path)
		} else {
			let _ = fs::remove_file(temporary);
			Ok(())
		}
	}
}

/// All production saves encode under the lock and write the cache outside it. Only the
/// atomic rename shares the state lock, preventing a stale account from being recreated.
pub(super) fn save(store: &Mutex<Store>) -> io::Result<()> {
	let Some(write) = store.lock().prepare()? else { return Ok(()) };
	let temporary = write.stage()?;
	write.commit(store, temporary)
}

/// Playback sign-out forgets every account's library snapshot.
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
	fn late_account_generation_cannot_reload_or_recreate_forgotten_snapshots() {
		let directory = tempfile::tempdir().unwrap();
		let live = Arc::new(AtomicU64::new(0));
		let mut store = Store::new(directory.path());
		store.bind_generation(live.clone(), 0);
		connect(&mut store, "old-account");
		store.save().unwrap();
		live.store(1, Ordering::Release);
		store.clear();
		// An in-flight connect must not re-enable writes after the synchronous clear.
		store.load(Some("old-account"));
		connect(&mut store, "old-account");
		store.save().unwrap();
		assert!(store.connected().is_none());
		assert!(!directory.path().join("library").exists());
		store.bind_generation(live, 1);
		store.load(Some("new-account"));
		connect(&mut store, "new-account");
		store.save().unwrap();
		assert!(matches!(store.connected(), Some(Response::Connected { me, .. }) if me.id == "new-account"));
	}

	#[test]
	fn snapshot_round_trip_preserves_home_tracks_playlist_counts_and_first_pages() {
		let dir = scratch("round-trip");
		let mut store = Store::new(&dir);
		connect(&mut store, "alex");
		let track: Track = serde_json::from_value(serde_json::json!({
			"uri": "spotify:episode:e", "name": "Episode", "type": "episode",
			"show": { "name": "Show", "images": [{ "url": "https://show" }] }
		}))
		.unwrap();
		store.record(&Response::Home(Home {
			recents: vec![Tile::liked()],
			recent_tracks: vec![track.clone()],
			loaded: true,
			..Home::default()
		}));
		let listing = Listing {
			kicker: ("Playlist").into(),
			title: "Liked Songs".into(),
			total: 51,
			next_offset: Some(50),
			accent: Some("#123456".into()),
			tracks: vec![track],
			added_at: vec![Some(1_781_001_234)],
			added_by: vec![Some("alex".into())],
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
		assert_eq!(listing.added_at(0), Some(1_781_001_234));
		assert_eq!(listing.added_by(0).as_deref(), Some("alex"));
		assert!(restored.listing(&Route::Album("missing".into())).is_none());
		assert!(!path.with_extension("tmp").exists(), "atomic replacement leaves no temporary file");
		fs::remove_dir_all(dir).unwrap();
	}

	#[test]
	fn old_listing_snapshots_default_added_attributes_to_unknown() {
		let mut saved = serde_json::to_value(Listing::default()).unwrap();
		let object = saved.as_object_mut().unwrap();
		object.remove("added_at");
		object.remove("added_by");
		object.insert("tracks".into(), serde_json::json!([{ "uri": "spotify:track:a" }]));
		let listing: Listing = serde_json::from_value(saved).unwrap();
		assert_eq!(listing.tracks.len(), 1);
		assert_eq!(listing.added_at(0), None);
		assert_eq!(listing.added_by(0), None);
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
	fn old_top_shelves_are_ignored_without_losing_the_snapshot() {
		let dir = scratch("old-top");
		let mut store = Store::new(&dir);
		connect(&mut store, "alex");
		store.record(&Response::Home(Home { recent_tracks: Vec::new(), loaded: true, ..Home::default() }));
		store.save().unwrap();
		let path = store.path("alex");
		let mut old: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
		old["top"] = serde_json::json!({"checked_at": 100_000, "artists": [], "tracks": []});
		old["home"]["top_artists"] = serde_json::json!([]);
		old["home"]["top_tracks"] = serde_json::json!([]);
		fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
		let mut restored = Store::new(&dir);
		restored.load(Some("alex"));
		assert!(restored.connected().is_some());
		let home = restored.home().unwrap();
		assert!(home.loaded);
		assert_eq!(home.songs().0, "Recently played songs");
		restored.save().unwrap();
		let saved: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
		assert!(saved.get("top").is_none());
		assert!(saved["home"].get("top_artists").is_none());
		assert!(saved["home"].get("top_tracks").is_none());
		fs::remove_dir_all(dir).unwrap();
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
		store.save().unwrap();
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

	#[test]
	fn native_folder_snapshot_preserves_hierarchy_and_prunes_deleted_playlist_counts() {
		let directory = tempfile::tempdir().unwrap();
		let mut store = Store::new(directory.path());
		connect(&mut store, "alice");
		let entries = vec![super::super::RootEntry::Folder(super::super::Folder {
			id:       "folder".into(),
			name:     "Music 雨".into(),
			count:    1,
			children: vec![super::super::RootEntry::Playlist("p".into())],
		})];
		assert!(store.record(&Response::Rootlist { account: "alice".into(), entries: entries.clone() }));
		assert!(!store.record(&Response::Rootlist { account: "bob".into(), entries: Vec::new() }));
		store.save().unwrap();
		let mut restored = Store::new(directory.path());
		restored.load(Some("alice"));
		let Some(Response::Rootlist { entries: fresh, .. }) = restored.rootlist() else { panic!("missing native folder snapshot") };
		assert_eq!(fresh, entries);
		restored.remove_playlist("p");
		let Some(Response::Rootlist { entries, .. }) = restored.rootlist() else { panic!("missing folder after playlist removal") };
		let super::super::RootEntry::Folder(folder) = &entries[0] else { panic!("folder was removed with its playlist") };
		assert_eq!(folder.count, 0);
		assert!(folder.children.is_empty());
		restored.load(Some("bob"));
		assert!(restored.rootlist().is_none());
	}

	#[test]
	fn staged_snapshot_does_not_hold_the_state_lock_or_survive_invalidation() {
		let directory = tempfile::tempdir().unwrap();
		let mut state = Store::new(directory.path());
		connect(&mut state, "account");
		let store = Mutex::new(state);
		let pending = store.lock().prepare().unwrap().unwrap();
		let temporary = pending.stage().unwrap();
		store.try_lock().unwrap().invalidate();
		pending.commit(&store, temporary).unwrap();
		assert!(store.lock().connected().is_none());
		assert_eq!(fs::read_dir(directory.path().join("library")).unwrap().count(), 0);
	}
}
