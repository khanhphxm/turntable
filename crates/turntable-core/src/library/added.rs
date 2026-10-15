//! Owned-playlist membership, using authenticated Spotify Playlist4 endpoints only.
//!
//! `contains` means “in an owned playlist”; the UI's added check is
//! `liked_songs.contains(track) || playlist_index.contains(track)`. Followed and
//! collaborative-but-not-owned playlists deliberately do not count. Local files
//! and episodes are not Spotify tracks and are not indexed.
//!
//! The library worker loads this after the engine's Session connects and refreshes
//! after successful playlist edits. Cache I/O stays on that worker, while network
//! work runs on the engine's runtime with explicitly bounded concurrency.
//! A persistence failure leaves the refreshed in-memory index usable and dirty,
//! so the next `persist`/`refresh` retries the write. Unix cache files are 0600 in
//! a 0700 subdirectory; Windows uses the cache directory's inherited ACL. Atomic
//! replacement prevents partial reads; this rebuildable cache deliberately does not fsync.
//! Detach retains disk data for reconnects; sign-out/Premium rejection forgets it.

use std::{
	collections::{HashMap, hash_map::Entry},
	fs::{self, File},
	io::{self, BufReader, BufWriter, Read, Write},
	num::NonZeroUsize,
	path::{Path, PathBuf},
	sync::Arc,
	time::Instant,
};

use futures_util::{StreamExt, TryStreamExt, stream};
use librespot_core::{Session, SpotifyId, SpotifyUri};
use librespot_metadata::{Metadata, Playlist};
use librespot_protocol::playlist4_external::{ListItems, SelectedListContent};
use protobuf::Message;
use smallvec::SmallVec;
use tempfile::NamedTempFile;
use thiserror::Error;

use super::Response;
use crate::engine::SessionReader;

const CACHE_MAGIC: &[u8; 8] = b"PLINDEX\x01";
const ROOTLIST_PAGE_SIZE: usize = 120;
const PLAYLIST_PAGE_SIZE: usize = 1000;

/// Deletes account-scoped playlist indexes; a running worker must be detached first.
pub(crate) fn clear(cache_dir: &Path) -> io::Result<()> {
	match fs::remove_dir_all(cache_dir.join("playlist-index")) {
		Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
		result => result,
	}
}

/// A typed, allocation-free Spotify track ID (librespot 0.8 has only `SpotifyId`).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TrackId(u128);

impl TrackId {
	pub const fn from_spotify_id(id: SpotifyId) -> Self {
		Self(id.id)
	}

	pub const fn spotify_id(self) -> SpotifyId {
		SpotifyId { id: self.0 }
	}

	pub fn from_uri(uri: &str) -> Result<Self, IndexError> {
		match SpotifyUri::from_uri(uri)? {
			SpotifyUri::Track { id } => Ok(Self::from_spotify_id(id)),
			_ => Err(IndexError::InvalidData("expected a track URI")),
		}
	}
}

/// A typed, allocation-free Spotify playlist ID.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PlaylistId(u128);

impl PlaylistId {
	pub const fn from_spotify_id(id: SpotifyId) -> Self {
		Self(id.id)
	}

	pub const fn spotify_id(self) -> SpotifyId {
		SpotifyId { id: self.0 }
	}

	pub fn from_uri(uri: &str) -> Result<Self, IndexError> {
		match SpotifyUri::from_uri(uri)? {
			SpotifyUri::Playlist { id, .. } => Ok(Self::from_spotify_id(id)),
			_ => Err(IndexError::InvalidData("expected a playlist URI")),
		}
	}
}

#[derive(Debug, Error)]
pub enum IndexError {
	#[error(transparent)]
	Io(#[from] io::Error),
	#[error(transparent)]
	Spotify(#[from] librespot_core::Error),
	#[error(transparent)]
	Protocol(#[from] protobuf::Error),
	#[error("invalid playlist-index data: {0}")]
	InvalidData(&'static str),
	#[error("invalid playlist-index data: {field}: {source}")]
	InvalidNumber {
		field:  &'static str,
		#[source]
		source: std::num::TryFromIntError,
	},
	#[error("invalid playlist-index data: cache string is not UTF-8: {0}")]
	InvalidText(#[source] std::string::FromUtf8Error),
	#[error("the Session username does not match this index")]
	WrongUser,
	#[error("the playlist is not in this user's owned-playlist index")]
	UnknownPlaylist,
}

#[derive(Debug)]
struct CachedPlaylist {
	name:     String,
	revision: Vec<u8>,
	/// Sorted, with duplicates preserved for occurrence-aware local removals.
	tracks:   Vec<TrackId>,
	/// Local edits must be reconciled even if rootlist still has the old revision.
	dirty:    bool,
}

#[derive(Debug)]
struct RootPlaylist {
	name:     String,
	revision: Vec<u8>,
}

struct FetchedPlaylist {
	id:       PlaylistId,
	playlist: CachedPlaylist,
	requests: usize,
	bytes:    usize,
}

/// All counts refer to owned playlists and canonical Spotify tracks only.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct RefreshStats {
	pub rootlist_requests:   usize,
	pub playlist_requests:   usize,
	pub response_bytes:      usize,
	pub playlists_refetched: usize,
	pub playlists_unchanged: usize,
	pub playlists_removed:   usize,
	pub memberships_added:   usize,
	pub memberships_removed: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct PlaylistInfo<'a> {
	pub id:                 PlaylistId,
	pub name:               &'a str,
	pub revision:           &'a [u8],
	pub track_entries:      usize,
	pub local_edit_pending: bool,
}

#[derive(Debug)]
pub struct PlaylistIndex {
	username:    String,
	cache_path:  PathBuf,
	playlists:   HashMap<PlaylistId, CachedPlaylist>,
	by_track:    HashMap<TrackId, SmallVec<[PlaylistId; 1]>>,
	cache_dirty: bool,
}

impl PlaylistIndex {
	/// Load an existing account-specific cache, or an empty index if absent.
	/// Corrupt/incompatible caches are reported, never silently accepted/reset.
	pub fn load(cache_dir: &Path, username: &str) -> Result<Self, IndexError> {
		if username.is_empty() {
			return Err(IndexError::InvalidData("empty username"));
		}
		let cache_path = cache_dir.join("playlist-index").join(format!("{}.plidx", crate::sha256_hex(username.as_bytes())));
		let mut index =
			Self { username: username.to_owned(), cache_path, playlists: HashMap::new(), by_track: HashMap::new(), cache_dirty: true };
		let file = match File::open(&index.cache_path) {
			Ok(file) => file,
			Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(index),
			Err(error) => return Err(error.into()),
		};
		let mut reader = CacheReader { remaining: file.metadata()?.len(), input: BufReader::new(file) };
		if &reader.fixed::<8>()? != CACHE_MAGIC {
			return Err(IndexError::InvalidData("unsupported cache format"));
		}
		if reader.string()? != username {
			return Err(IndexError::WrongUser);
		}
		let count = reader.count(29)?;
		for _ in 0..count {
			let id = PlaylistId(u128::from_be_bytes(reader.fixed()?));
			let name = reader.string()?;
			let revision = reader.bytes()?;
			let dirty = match reader.fixed::<1>()?[0] {
				0 => false,
				1 => true,
				_ => return Err(IndexError::InvalidData("invalid local-edit flag")),
			};
			let track_count = reader.count(16)?;
			let mut tracks = Vec::with_capacity(track_count);
			for _ in 0..track_count {
				let track = TrackId(u128::from_be_bytes(reader.fixed()?));
				if tracks.last().is_some_and(|previous| previous > &track) {
					return Err(IndexError::InvalidData("unsorted cached tracks"));
				}
				tracks.push(track);
			}
			if index.playlists.insert(id, CachedPlaylist { name, revision, tracks, dirty }).is_some() {
				return Err(IndexError::InvalidData("duplicate cached playlist"));
			}
		}
		if reader.remaining != 0 {
			return Err(IndexError::InvalidData("trailing cache data"));
		}
		for (&id, playlist) in &index.playlists {
			for &track in unique_tracks(&playlist.tracks) {
				add_membership(&mut index.by_track, track, id);
			}
		}
		index.cache_dirty = false;
		Ok(index)
	}

	pub fn contains(&self, track: &TrackId) -> bool {
		self.by_track.contains_key(track)
	}

	/// Borrowed, sorted, duplicate-free playlist IDs; no query allocation.
	pub fn playlists_with(&self, track: &TrackId) -> &[PlaylistId] {
		self.by_track.get(track).map_or(&[], SmallVec::as_slice)
	}

	pub fn playlist_count(&self) -> usize {
		self.playlists.len()
	}

	pub fn track_count(&self) -> usize {
		self.by_track.len()
	}

	/// Includes duplicate occurrences within each playlist.
	pub fn track_entry_count(&self) -> usize {
		self.playlists.values().map(|playlist| playlist.tracks.len()).sum()
	}

	pub fn playlists(&self) -> impl Iterator<Item = PlaylistInfo<'_>> {
		self.playlists.iter().map(|(&id, playlist)| PlaylistInfo {
			id,
			name: &playlist.name,
			revision: &playlist.revision,
			track_entries: playlist.tracks.len(),
			local_edit_pending: playlist.dirty,
		})
	}

	/// Sorted track occurrences, not playlist playback order.
	pub fn playlist_tracks(&self, playlist: &PlaylistId) -> Option<&[TrackId]> {
		self.playlists.get(playlist).map(|playlist| playlist.tracks.as_slice())
	}

	pub fn cache_path(&self) -> &Path {
		&self.cache_path
	}

	/// Apply one successfully added occurrence in an already indexed playlist.
	/// Returns whether this track gained membership in this playlist. Persist
	/// explicitly to retain the edit across restarts; refresh also persists it.
	pub fn apply_add(&mut self, playlist: &PlaylistId, track: &TrackId) -> Result<bool, IndexError> {
		let record = self.playlists.get_mut(playlist).ok_or(IndexError::UnknownPlaylist)?;
		let position = record.tracks.partition_point(|existing| existing < track);
		record.tracks.insert(position, *track);
		record.dirty = true;
		self.cache_dirty = true;
		Ok(add_membership(&mut self.by_track, *track, *playlist))
	}

	/// Remove one successfully removed occurrence. Duplicate occurrences keep
	/// the membership until the last is removed. Returns whether one was found.
	pub fn apply_remove(&mut self, playlist: &PlaylistId, track: &TrackId) -> Result<bool, IndexError> {
		let record = self.playlists.get_mut(playlist).ok_or(IndexError::UnknownPlaylist)?;
		let Ok(position) = record.tracks.binary_search(track) else { return Ok(false) };
		record.tracks.remove(position);
		if record.tracks.binary_search(track).is_err() {
			remove_membership(&mut self.by_track, *track, *playlist);
		}
		record.dirty = true;
		self.cache_dirty = true;
		Ok(true)
	}

	/// Register a successfully created owned playlist immediately, including duplicates.
	pub fn apply_create(&mut self, playlist: PlaylistId, name: &str, tracks: &[TrackId]) {
		let mut tracks = tracks.to_vec();
		tracks.sort_unstable();
		self.replace_playlist(
			playlist,
			CachedPlaylist { name: name.to_owned(), revision: Vec::new(), tracks, dirty: true },
			&mut RefreshStats::default(),
		);
	}

	pub fn apply_rename(&mut self, playlist: &PlaylistId, name: &str) -> Result<(), IndexError> {
		let record = self.playlists.get_mut(playlist).ok_or(IndexError::UnknownPlaylist)?;
		if record.name != name {
			record.name = name.to_owned();
			record.dirty = true;
			self.cache_dirty = true;
		}
		Ok(())
	}

	/// Forget an unfollowed/deleted owned playlist without a rootlist round trip.
	pub fn apply_delete(&mut self, playlist: &PlaylistId) {
		if let Some(record) = self.playlists.remove(playlist) {
			for &track in unique_tracks(&record.tracks) {
				remove_membership(&mut self.by_track, track, *playlist);
			}
			self.cache_dirty = true;
		}
	}

	/// Fetch decorated rootlist pages, then only new, changed, or locally edited
	/// owned playlists. At most `concurrency` playlists are in flight; pages of
	/// one playlist are sequential. Unchanged launches make no playlist requests
	/// and no disk writes. Removed/unfollowed/non-owned playlists lose membership.
	pub async fn refresh(&mut self, session: &Session, concurrency: NonZeroUsize) -> Result<RefreshStats, IndexError> {
		let username = session.username();
		if username != self.username {
			return Err(IndexError::WrongUser);
		}
		let (roots, mut stats) = fetch_rootlist(session, &username).await?;
		let fetched: Vec<_> = stream::iter(
			roots.iter().filter(|(id, root)| self.needs_fetch(id, root)).map(|(&id, _)| fetch_playlist(session, id, &username)),
		)
		.buffer_unordered(concurrency.get())
		.try_collect()
		.await?;
		stats.playlists_refetched = fetched.len();
		stats.playlists_unchanged = roots.len() - fetched.len();
		// No in-memory mutations until every required request succeeds.
		for fetched in fetched {
			stats.playlist_requests += fetched.requests;
			stats.response_bytes += fetched.bytes;
			self.replace_playlist(fetched.id, fetched.playlist, &mut stats);
		}
		self.remove_missing(&roots, &mut stats);
		for (id, root) in roots {
			if let Some(record) = self.playlists.get_mut(&id)
				&& record.name != root.name
			{
				record.name = root.name;
				self.cache_dirty = true;
			}
		}
		self.persist()?;
		Ok(stats)
	}

	fn needs_fetch(&self, id: &PlaylistId, root: &RootPlaylist) -> bool {
		self.playlists.get(id).is_none_or(|record| record.dirty || root.revision.is_empty() || record.revision != root.revision)
	}

	fn replace_playlist(&mut self, id: PlaylistId, playlist: CachedPlaylist, stats: &mut RefreshStats) {
		let old = self.playlists.remove(&id);
		let old_tracks = old.as_ref().map_or(&[][..], |old| old.tracks.as_slice());
		let mut before = unique_tracks(old_tracks).peekable();
		let mut after = unique_tracks(&playlist.tracks).peekable();
		loop {
			match (before.peek().copied(), after.peek().copied()) {
				(Some(&left), Some(&right)) if left == right => {
					before.next();
					after.next();
				},
				(Some(&left), Some(&right)) if left < right => {
					stats.memberships_removed += usize::from(remove_membership(&mut self.by_track, left, id));
					before.next();
				},
				(_, Some(&right)) => {
					stats.memberships_added += usize::from(add_membership(&mut self.by_track, right, id));
					after.next();
				},
				(Some(&left), None) => {
					stats.memberships_removed += usize::from(remove_membership(&mut self.by_track, left, id));
					before.next();
				},
				(None, None) => break,
			}
		}
		drop(after);
		self.playlists.insert(id, playlist);
		self.cache_dirty = true;
	}

	fn remove_missing(&mut self, roots: &HashMap<PlaylistId, RootPlaylist>, stats: &mut RefreshStats) {
		self.playlists.retain(|&id, playlist| {
			if roots.contains_key(&id) {
				return true;
			}
			for &track in unique_tracks(&playlist.tracks) {
				stats.memberships_removed += usize::from(remove_membership(&mut self.by_track, track, id));
			}
			stats.playlists_removed += 1;
			false
		});
		self.cache_dirty |= stats.playlists_removed > 0;
	}

	/// Stream a compact, versioned cache to a private temporary file, fsync,
	/// then atomically replace the previous cache (also on Windows). A failed
	/// write never truncates the previous cache. No work if nothing changed.
	pub fn persist(&mut self) -> Result<(), IndexError> {
		if !self.cache_dirty {
			return Ok(());
		}
		let directory = self.cache_path.parent().ok_or(IndexError::InvalidData("cache has no parent"))?;
		let mut builder = fs::DirBuilder::new();
		builder.recursive(true);
		#[cfg(unix)]
		{
			use std::os::unix::fs::DirBuilderExt;
			builder.mode(0o700);
		}
		builder.create(directory)?;
		#[cfg(unix)]
		{
			use std::os::unix::fs::PermissionsExt;
			fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
		}
		let mut temporary = NamedTempFile::new_in(directory)?;
		#[cfg(unix)]
		{
			use std::os::unix::fs::PermissionsExt;
			temporary.as_file().set_permissions(fs::Permissions::from_mode(0o600))?;
		}
		{
			let mut output = BufWriter::new(temporary.as_file_mut());
			output.write_all(CACHE_MAGIC)?;
			write_bytes(&mut output, self.username.as_bytes())?;
			write_count(&mut output, self.playlists.len())?;
			for (&id, playlist) in &self.playlists {
				output.write_all(&id.0.to_be_bytes())?;
				write_bytes(&mut output, playlist.name.as_bytes())?;
				write_bytes(&mut output, &playlist.revision)?;
				output.write_all(&[u8::from(playlist.dirty)])?;
				write_count(&mut output, playlist.tracks.len())?;
				for track in &playlist.tracks {
					output.write_all(&track.0.to_be_bytes())?;
				}
			}
			output.flush()?;
		}
		temporary.persist(&self.cache_path).map_err(|error| error.error)?;
		self.cache_dirty = false;
		Ok(())
	}
}

/// Worker-owned index; the host gets only an immutable, sorted membership snapshot.
pub(super) struct Membership {
	cache_dir: PathBuf,
	reader:    Option<SessionReader>,
	index:     Option<PlaylistIndex>,
	tracks:    Arc<[TrackId]>,
}

impl Membership {
	pub(super) fn new(cache_dir: &Path) -> Self {
		Self { cache_dir: cache_dir.to_owned(), reader: None, index: None, tracks: Arc::from([]) }
	}

	pub(super) fn attach(&mut self, reader: Option<SessionReader>, deliver: &dyn Fn(Response)) {
		self.reader = reader;
		let Some(reader) = &self.reader else {
			self.index = None;
			self.tracks = Arc::from([]);
			self.publish(None, deliver);
			return;
		};
		let username = reader.session().username();
		if self.index.as_ref().is_none_or(|index| index.username != username) {
			self.index = None;
			self.tracks = Arc::from([]);
			self.publish(None, deliver);
			match PlaylistIndex::load(&self.cache_dir, &username) {
				Ok(index) => self.index = Some(index),
				Err(error) => {
					Self::failed(&error, deliver);
					return;
				},
			}
			self.snapshot();
			self.publish(None, deliver);
		}
	}

	/// Runs after pending requests finish, so they cannot recreate the signed-out cache.
	pub(super) fn forget(&mut self, deliver: &dyn Fn(Response)) {
		self.attach(None, deliver);
		if let Err(error) = clear(&self.cache_dir) {
			Self::failed(&IndexError::from(error), deliver);
		}
	}

	/// Cancel idle network work for foreground navigation; staged results never partially update the index.
	/// `false` means interrupted and should be resumed at the next idle boundary.
	pub(super) fn refresh(&mut self, urgent: tokio::sync::watch::Receiver<()>, deliver: &dyn Fn(Response)) -> bool {
		let (Some(reader), Some(index)) = (&self.reader, &mut self.index) else { return true };
		let started = crate::net::trace::enabled().then(Instant::now);
		let result = reader.block_on(async {
			crate::net::par::interruptible(urgent, index.refresh(reader.session(), NonZeroUsize::new(4).unwrap_or(NonZeroUsize::MIN)))
				.await
				.transpose()
		});
		match result {
			Ok(None) => return false,
			Ok(Some(stats)) => {
				if let Some(started) = started {
					crate::net::trace::mark("added", &format!("{}ms {stats:?}", started.elapsed().as_millis()));
				}
				if stats.memberships_added > 0 || stats.memberships_removed > 0 {
					self.snapshot();
				}
				self.publish(Some(stats), deliver);
			},
			Err(error) => {
				// Persistence can fail after a successful fetch; keep the usable memory answer.
				self.snapshot();
				self.publish(None, deliver);
				Self::failed(&error, deliver);
			},
		}
		true
	}

	pub(super) fn created(&mut self, playlist_uri: &str, name: &str, uris: &[String], deliver: &dyn Fn(Response)) {
		if let Some(index) = &mut self.index
			&& let Ok(id) = PlaylistId::from_uri(playlist_uri)
		{
			let tracks: Vec<_> = uris.iter().filter_map(|uri| TrackId::from_uri(uri).ok()).collect();
			index.apply_create(id, name, &tracks);
			self.local_answer(deliver);
		}
	}

	pub(super) fn renamed(&mut self, playlist_uri: &str, name: &str, deliver: &dyn Fn(Response)) {
		if let Some(index) = &mut self.index
			&& let Ok(id) = PlaylistId::from_uri(playlist_uri)
			&& index.playlists.contains_key(&id)
		{
			if let Err(error) = index.apply_rename(&id, name) {
				Self::failed(&error, deliver);
			}
			self.local_answer(deliver);
		}
	}

	pub(super) fn deleted(&mut self, playlist_uri: &str, deliver: &dyn Fn(Response)) {
		if let Some(index) = &mut self.index
			&& let Ok(id) = PlaylistId::from_uri(playlist_uri)
		{
			index.apply_delete(&id);
			self.local_answer(deliver);
		}
	}

	pub(super) fn removed_row(&mut self, playlist_uri: &str, uri: &str, deliver: &dyn Fn(Response)) {
		if let Some(index) = &mut self.index
			&& let (Ok(id), Ok(track)) = (PlaylistId::from_uri(playlist_uri), TrackId::from_uri(uri))
			&& index.playlists.contains_key(&id)
		{
			if let Err(error) = index.apply_remove(&id, &track) {
				Self::failed(&error, deliver);
			}
			self.local_answer(deliver);
		}
	}

	fn local_answer(&mut self, deliver: &dyn Fn(Response)) {
		if let Some(index) = &mut self.index
			&& let Err(error) = index.persist()
		{
			Self::failed(&error, deliver);
		}
		self.snapshot();
		self.publish(None, deliver);
	}

	/// Publish successful additions before reconciling Playlist4 revisions.
	pub(super) fn appended(&mut self, playlist_uri: &str, additions: &[String], deliver: &dyn Fn(Response)) {
		if let Some(index) = &mut self.index
			&& let Ok(playlist) = PlaylistId::from_uri(playlist_uri)
			&& index.playlists.contains_key(&playlist)
		{
			for uri in additions {
				if let Ok(track) = TrackId::from_uri(uri)
					&& let Err(error) = index.apply_add(&playlist, &track)
				{
					Self::failed(&error, deliver);
				}
			}
			self.local_answer(deliver);
		}
	}

	fn snapshot(&mut self) {
		let Some(index) = &self.index else { return };
		if self.tracks.len() == index.track_count() && self.tracks.iter().all(|track| index.contains(track)) {
			return;
		}
		let mut tracks: Vec<_> = index.by_track.keys().copied().collect();
		tracks.sort_unstable();
		self.tracks = tracks.into();
	}

	fn publish(&self, stats: Option<RefreshStats>, deliver: &dyn Fn(Response)) {
		deliver(Response::Added { tracks: self.tracks.clone(), stats });
	}

	fn failed(error: &IndexError, deliver: &dyn Fn(Response)) {
		deliver(Response::Error { route: None, message: format!("Couldn't update playlist membership: {error}") });
	}
}

fn unique_tracks(tracks: &[TrackId]) -> impl Iterator<Item = &TrackId> {
	let mut previous = None;
	tracks.iter().filter(move |&track| {
		let distinct = previous != Some(track);
		previous = Some(track);
		distinct
	})
}

fn add_membership(index: &mut HashMap<TrackId, SmallVec<[PlaylistId; 1]>>, track: TrackId, playlist: PlaylistId) -> bool {
	let playlists = index.entry(track).or_default();
	match playlists.binary_search(&playlist) {
		Ok(_) => false,
		Err(position) => {
			playlists.insert(position, playlist);
			true
		},
	}
}

fn remove_membership(index: &mut HashMap<TrackId, SmallVec<[PlaylistId; 1]>>, track: TrackId, playlist: PlaylistId) -> bool {
	let Entry::Occupied(mut entry) = index.entry(track) else { return false };
	let Ok(position) = entry.get().binary_search(&playlist) else { return false };
	entry.get_mut().remove(position);
	if entry.get().is_empty() {
		entry.remove();
	}
	true
}

/// Check page completeness, position, and progress instead of caching a partial list.
fn page_end(page: &SelectedListContent, from: usize) -> Result<usize, IndexError> {
	let total = usize::try_from(page.length()).map_err(|source| IndexError::InvalidNumber { field: "negative list length", source })?;
	let contents = page.contents.as_ref().ok_or(IndexError::InvalidData("missing list contents"))?;
	if usize::try_from(contents.pos()).ok() != Some(from) {
		return Err(IndexError::InvalidData("unexpected page position"));
	}
	let end = from.checked_add(contents.items.len()).ok_or(IndexError::InvalidData("page length overflow"))?;
	if end > total || (end < total && (end == from || !contents.truncated())) {
		return Err(IndexError::InvalidData("incomplete or non-progressing list page"));
	}
	Ok(end)
}

fn collect_root_page(contents: &ListItems, username: &str, roots: &mut HashMap<PlaylistId, RootPlaylist>) -> Result<(), IndexError> {
	if contents.items.len() != contents.meta_items.len() {
		return Err(IndexError::InvalidData("rootlist decorations are missing"));
	}
	for (item, meta) in contents.items.iter().zip(&contents.meta_items) {
		if meta.has_status_code() && !matches!(meta.status_code(), 0 | 200) {
			return Err(IndexError::InvalidData("rootlist playlist decoration failed"));
		}
		if meta.owner_username() != username {
			continue;
		}
		let id = PlaylistId::from_uri(item.uri())?;
		let root = RootPlaylist { name: meta.attributes.get_or_default().name().to_owned(), revision: meta.revision().to_vec() };
		// The same owned playlist may occur more than once in rootlist/folders.
		if let Some(previous) = roots.insert(id, root)
			&& previous.revision != meta.revision()
		{
			return Err(IndexError::InvalidData("conflicting rootlist playlist revisions"));
		}
	}
	Ok(())
}

async fn fetch_rootlist(session: &Session, username: &str) -> Result<(HashMap<PlaylistId, RootPlaylist>, RefreshStats), IndexError> {
	let mut roots = HashMap::new();
	let mut stats = RefreshStats::default();
	let mut from = 0;
	let mut revision = None;
	let mut total = None;
	loop {
		let bytes = session.spclient().get_rootlist(from, Some(ROOTLIST_PAGE_SIZE)).await?;
		stats.rootlist_requests += 1;
		stats.response_bytes += bytes.len();
		let page = SelectedListContent::parse_from_bytes(&bytes)?;
		if revision.as_ref().is_some_and(|revision: &Vec<u8>| revision != page.revision())
			|| total.is_some_and(|total| total != page.length())
		{
			return Err(IndexError::InvalidData("rootlist changed during pagination"));
		}
		let end = page_end(&page, from)?;
		collect_root_page(page.contents.get_or_default(), username, &mut roots)?;
		if end == usize::try_from(page.length()).map_err(|source| IndexError::InvalidNumber { field: "negative list length", source })? {
			break;
		}
		if revision.is_none() {
			revision = Some(page.revision().to_vec());
			total = Some(page.length());
		}
		from = end;
	}
	Ok((roots, stats))
}

async fn fetch_playlist(session: &Session, id: PlaylistId, username: &str) -> Result<FetchedPlaylist, IndexError> {
	let uri = SpotifyUri::Playlist { id: id.spotify_id(), user: None };
	let bytes = Playlist::request(session, &uri).await?;
	let first = SelectedListContent::parse_from_bytes(&bytes)?;
	if first.owner_username() != username {
		return Err(IndexError::InvalidData("playlist ownership changed during refresh"));
	}
	let total =
		usize::try_from(first.length()).map_err(|source| IndexError::InvalidNumber { field: "negative playlist length", source })?;
	let revision = first.revision().to_vec();
	let mut fetched = FetchedPlaylist {
		id,
		playlist: CachedPlaylist {
			name: first.attributes.get_or_default().name().to_owned(),
			revision,
			tracks: Vec::with_capacity(total),
			dirty: false,
		},
		requests: 1,
		bytes: bytes.len(),
	};
	let mut from = page_end(&first, 0)?;
	collect_tracks(first.contents.get_or_default(), &mut fetched.playlist.tracks)?;
	while from < total {
		let endpoint = format!("/playlist/v2/playlist/{}?from={from}&length={PLAYLIST_PAGE_SIZE}", id.spotify_id().to_base62()?);
		let bytes = session.spclient().request(&http::Method::GET, &endpoint, None, None).await?;
		fetched.requests += 1;
		fetched.bytes += bytes.len();
		let page = SelectedListContent::parse_from_bytes(&bytes)?;
		if page.revision() != fetched.playlist.revision || page.length() != first.length() || page.owner_username() != username {
			return Err(IndexError::InvalidData("playlist changed during pagination"));
		}
		let end = page_end(&page, from)?;
		collect_tracks(page.contents.get_or_default(), &mut fetched.playlist.tracks)?;
		from = end;
	}
	fetched.playlist.tracks.sort_unstable();
	Ok(fetched)
}

fn collect_tracks(contents: &ListItems, tracks: &mut Vec<TrackId>) -> Result<(), IndexError> {
	for item in &contents.items {
		if let SpotifyUri::Track { id } = SpotifyUri::from_uri(item.uri())? {
			tracks.push(TrackId::from_spotify_id(id));
		}
	}
	Ok(())
}

fn write_count(output: &mut impl Write, count: usize) -> Result<(), IndexError> {
	let count = u32::try_from(count).map_err(|source| IndexError::InvalidNumber { field: "cache field exceeds u32", source })?;
	output.write_all(&count.to_be_bytes())?;
	Ok(())
}

fn write_bytes(output: &mut impl Write, bytes: &[u8]) -> Result<(), IndexError> {
	write_count(output, bytes.len())?;
	output.write_all(bytes)?;
	Ok(())
}

struct CacheReader<R> {
	input:     R,
	remaining: u64,
}

impl<R: Read> CacheReader<R> {
	fn read(&mut self, bytes: &mut [u8]) -> Result<(), IndexError> {
		let count = u64::try_from(bytes.len()).map_err(|source| IndexError::InvalidNumber { field: "cache field too large", source })?;
		if count > self.remaining {
			return Err(IndexError::InvalidData("truncated cache field"));
		}
		self.input.read_exact(bytes)?;
		self.remaining -= count;
		Ok(())
	}

	fn fixed<const N: usize>(&mut self) -> Result<[u8; N], IndexError> {
		let mut bytes = [0; N];
		self.read(&mut bytes)?;
		Ok(bytes)
	}

	fn count(&mut self, minimum_item_size: u64) -> Result<usize, IndexError> {
		let count = u32::from_be_bytes(self.fixed()?);
		if u64::from(count) > self.remaining / minimum_item_size {
			return Err(IndexError::InvalidData("cache count exceeds remaining data"));
		}
		usize::try_from(count).map_err(|source| IndexError::InvalidNumber { field: "cache count exceeds usize", source })
	}

	fn bytes(&mut self) -> Result<Vec<u8>, IndexError> {
		let count = self.count(1)?;
		let mut bytes = vec![0; count];
		self.read(&mut bytes)?;
		Ok(bytes)
	}

	fn string(&mut self) -> Result<String, IndexError> {
		String::from_utf8(self.bytes()?).map_err(IndexError::InvalidText)
	}
}

#[cfg(test)]
mod tests {
	use std::{collections::HashMap, fs};

	use librespot_protocol::playlist4_external::{Item, ListAttributes, ListItems, MetaItem, SelectedListContent};
	use protobuf::MessageField;

	use super::{
		CachedPlaylist, IndexError, Membership, PlaylistId, PlaylistIndex, RefreshStats, RootPlaylist, TrackId, collect_root_page,
		collect_tracks, page_end,
	};

	fn record(revision: u8, tracks: &[u128]) -> CachedPlaylist {
		let mut tracks: Vec<_> = tracks.iter().copied().map(TrackId).collect();
		tracks.sort_unstable();
		CachedPlaylist { name: format!("playlist-{revision}"), revision: vec![revision], tracks, dirty: false }
	}

	fn root(revision: u8) -> RootPlaylist {
		RootPlaylist { name: format!("playlist-{revision}"), revision: vec![revision] }
	}

	#[test]
	fn diff_preserves_shared_membership_and_deduplicates_occurrences() -> Result<(), IndexError> {
		let directory = tempfile::tempdir()?;
		let mut index = PlaylistIndex::load(directory.path(), "alice")?;
		let mut stats = RefreshStats::default();
		index.replace_playlist(PlaylistId(1), record(1, &[1, 1, 2]), &mut stats);
		index.replace_playlist(PlaylistId(2), record(1, &[2, 3]), &mut stats);
		assert_eq!(stats.memberships_added, 4);
		assert_eq!(index.playlists_with(&TrackId(1)), &[PlaylistId(1)]);
		assert_eq!(index.playlists_with(&TrackId(2)), &[PlaylistId(1), PlaylistId(2)]);
		assert_eq!(index.track_entry_count(), 5);
		let mut stats = RefreshStats::default();
		index.replace_playlist(PlaylistId(1), record(2, &[2, 4, 4]), &mut stats);
		assert_eq!(stats.memberships_added, 1);
		assert_eq!(stats.memberships_removed, 1);
		assert!(!index.contains(&TrackId(1)));
		assert!(index.contains(&TrackId(3)));
		assert_eq!(index.playlists_with(&TrackId(2)), &[PlaylistId(1), PlaylistId(2)]);
		assert_eq!(index.playlists_with(&TrackId(4)), &[PlaylistId(1)]);
		Ok(())
	}

	#[test]
	fn removing_playlists_keeps_other_memberships_then_removes_last() -> Result<(), IndexError> {
		let directory = tempfile::tempdir()?;
		let mut index = PlaylistIndex::load(directory.path(), "alice")?;
		index.replace_playlist(PlaylistId(1), record(1, &[1, 1, 2]), &mut RefreshStats::default());
		index.replace_playlist(PlaylistId(2), record(1, &[2, 3]), &mut RefreshStats::default());
		let mut stats = RefreshStats::default();
		index.remove_missing(&HashMap::from([(PlaylistId(2), root(1))]), &mut stats);
		assert_eq!(stats.playlists_removed, 1);
		assert_eq!(stats.memberships_removed, 2);
		assert_eq!(index.playlists_with(&TrackId(2)), &[PlaylistId(2)]);
		assert!(!index.contains(&TrackId(1)));
		index.remove_missing(&HashMap::new(), &mut RefreshStats::default());
		assert_eq!(index.playlist_count(), 0);
		assert_eq!(index.track_count(), 0);
		Ok(())
	}

	#[test]
	fn local_edits_preserve_duplicate_occurrences_and_force_reconciliation() -> Result<(), IndexError> {
		let directory = tempfile::tempdir()?;
		let mut index = PlaylistIndex::load(directory.path(), "alice")?;
		let playlist = PlaylistId(1);
		let track = TrackId(7);
		index.replace_playlist(playlist, record(1, &[7]), &mut RefreshStats::default());
		assert!(!index.needs_fetch(&playlist, &root(1)));
		assert!(!index.apply_add(&playlist, &track)?);
		assert_eq!(index.playlists_with(&track), &[playlist]);
		assert!(index.needs_fetch(&playlist, &root(1)));
		assert!(index.apply_remove(&playlist, &track)?);
		assert!(index.contains(&track));
		assert!(index.apply_remove(&playlist, &track)?);
		assert!(!index.contains(&track));
		assert!(!index.apply_remove(&playlist, &track)?);
		assert!(index.apply_add(&playlist, &track)?);
		assert!(matches!(index.apply_add(&PlaylistId(999), &track), Err(IndexError::UnknownPlaylist)));
		assert!(matches!(index.apply_remove(&PlaylistId(999), &track), Err(IndexError::UnknownPlaylist)));
		Ok(())
	}

	#[test]
	fn create_rename_and_row_removals_publish_immediate_persisted_membership() -> Result<(), IndexError> {
		let directory = tempfile::tempdir()?;
		let mut membership = Membership::new(directory.path());
		membership.index = Some(PlaylistIndex::load(directory.path(), "alice")?);
		let playlist = "spotify:playlist:0000000000000000000001";
		let track = "spotify:track:0000000000000000000001";
		membership.created(playlist, "Scratch", &[track.into(), track.into()], &|_| {});
		assert_eq!(membership.tracks.as_ref(), &[TrackId(1)]);
		membership.renamed(playlist, "Renamed", &|_| {});
		let loaded = PlaylistIndex::load(directory.path(), "alice")?;
		assert_eq!(loaded.playlists().next().map(|playlist| playlist.name), Some("Renamed"));
		membership.removed_row(playlist, track, &|_| {});
		assert_eq!(membership.tracks.as_ref(), &[TrackId(1)], "duplicate keeps membership");
		let loaded = PlaylistIndex::load(directory.path(), "alice")?;
		assert_eq!(loaded.playlist_tracks(&PlaylistId(1)), Some([TrackId(1)].as_slice()));
		membership.removed_row(playlist, track, &|_| {});
		assert!(membership.tracks.is_empty());
		membership.appended(playlist, &[track.into()], &|_| {});
		assert_eq!(membership.tracks.as_ref(), &[TrackId(1)]);
		membership.deleted(playlist, &|_| {});
		assert!(membership.tracks.is_empty());
		assert_eq!(PlaylistIndex::load(directory.path(), "alice")?.playlist_count(), 0);
		Ok(())
	}

	#[test]
	fn persist_round_trip_keeps_revisions_duplicates_and_local_dirty_flag() -> Result<(), IndexError> {
		let directory = tempfile::tempdir()?;
		let mut index = PlaylistIndex::load(directory.path(), "alice")?;
		index.replace_playlist(PlaylistId(1), record(1, &[1, 2, 2]), &mut RefreshStats::default());
		index.replace_playlist(PlaylistId(2), record(2, &[]), &mut RefreshStats::default());
		index.apply_add(&PlaylistId(1), &TrackId(3))?;
		index.persist()?;
		let mut loaded = PlaylistIndex::load(directory.path(), "alice")?;
		assert_eq!(loaded.playlist_count(), 2);
		assert_eq!(loaded.track_count(), 3);
		assert_eq!(loaded.playlist_tracks(&PlaylistId(1)), Some(&[TrackId(1), TrackId(2), TrackId(2), TrackId(3)][..]));
		assert!(loaded.needs_fetch(&PlaylistId(1), &root(1)));
		assert!(!loaded.needs_fetch(&PlaylistId(2), &root(2)));
		assert!(loaded.needs_fetch(&PlaylistId(2), &root(3)));
		assert!(!loaded.cache_dirty);
		loaded.replace_playlist(PlaylistId(1), record(2, &[2]), &mut RefreshStats::default());
		loaded.persist()?;
		let reloaded = PlaylistIndex::load(directory.path(), "alice")?;
		assert_eq!(reloaded.track_count(), 1);
		assert!(!reloaded.needs_fetch(&PlaylistId(1), &root(2)));
		assert_eq!(fs::read_dir(reloaded.cache_path().parent().ok_or(IndexError::InvalidData("missing cache parent"))?)?.count(), 1);
		Ok(())
	}

	#[test]
	fn cache_is_account_scoped_and_cannot_escape_directory() -> Result<(), IndexError> {
		let directory = tempfile::tempdir()?;
		let mut alice = PlaylistIndex::load(directory.path(), "alice")?;
		alice.replace_playlist(PlaylistId(1), record(1, &[1]), &mut RefreshStats::default());
		alice.persist()?;
		let bob = PlaylistIndex::load(directory.path(), "../bob")?;
		assert_ne!(alice.cache_path(), bob.cache_path());
		assert_eq!(bob.cache_path().parent(), alice.cache_path().parent());
		assert_eq!(bob.track_count(), 0);
		fs::copy(alice.cache_path(), bob.cache_path())?;
		assert!(matches!(PlaylistIndex::load(directory.path(), "../bob"), Err(IndexError::WrongUser)));
		Ok(())
	}

	#[cfg(unix)]
	#[test]
	fn cache_permissions_are_private_on_creation_and_replacement() -> Result<(), IndexError> {
		use std::os::unix::fs::PermissionsExt;
		let directory = tempfile::tempdir()?;
		let mut index = PlaylistIndex::load(directory.path(), "alice")?;
		index.persist()?;
		assert_eq!(fs::metadata(index.cache_path())?.permissions().mode() & 0o777, 0o600);
		assert_eq!(
			fs::metadata(index.cache_path().parent().ok_or(IndexError::InvalidData("missing cache parent"))?)?.permissions().mode() & 0o777,
			0o700
		);
		fs::set_permissions(index.cache_path(), fs::Permissions::from_mode(0o644))?;
		index.replace_playlist(PlaylistId(1), record(1, &[1]), &mut RefreshStats::default());
		index.persist()?;
		assert_eq!(fs::metadata(index.cache_path())?.permissions().mode() & 0o777, 0o600);
		Ok(())
	}

	#[test]
	fn cache_rejects_truncation_unknown_versions_trailing_bytes_and_huge_counts() -> Result<(), IndexError> {
		let directory = tempfile::tempdir()?;
		let mut index = PlaylistIndex::load(directory.path(), "alice")?;
		index.replace_playlist(PlaylistId(1), record(1, &[1]), &mut RefreshStats::default());
		index.persist()?;
		let valid = fs::read(index.cache_path())?;
		for length in [0, 7, valid.len() - 1] {
			let truncated = valid.get(..length).ok_or(IndexError::InvalidData("invalid test truncation"))?;
			fs::write(index.cache_path(), truncated)?;
			assert!(PlaylistIndex::load(directory.path(), "alice").is_err());
		}
		let mut unknown = valid.clone();
		*unknown.get_mut(7).ok_or(IndexError::InvalidData("missing test version"))? = 99;
		fs::write(index.cache_path(), unknown)?;
		assert!(PlaylistIndex::load(directory.path(), "alice").is_err());
		let mut trailing = valid.clone();
		trailing.push(0);
		fs::write(index.cache_path(), trailing)?;
		assert!(PlaylistIndex::load(directory.path(), "alice").is_err());
		let mut huge_count = valid;
		// Header, username length, username, then the playlist count.
		huge_count.get_mut(17..21).ok_or(IndexError::InvalidData("missing test count"))?.copy_from_slice(&u32::MAX.to_be_bytes());
		fs::write(index.cache_path(), huge_count)?;
		assert!(PlaylistIndex::load(directory.path(), "alice").is_err());
		Ok(())
	}

	#[test]
	fn revision_checks_refetch_only_new_changed_or_dirty_playlists() -> Result<(), IndexError> {
		let directory = tempfile::tempdir()?;
		let mut index = PlaylistIndex::load(directory.path(), "alice")?;
		assert!(index.needs_fetch(&PlaylistId(1), &root(1)));
		index.replace_playlist(PlaylistId(1), record(1, &[1]), &mut RefreshStats::default());
		assert!(!index.needs_fetch(&PlaylistId(1), &root(1)));
		assert!(index.needs_fetch(&PlaylistId(1), &root(2)));
		assert!(index.needs_fetch(&PlaylistId(1), &RootPlaylist { name: String::new(), revision: vec![] }));
		index.apply_remove(&PlaylistId(1), &TrackId(1))?;
		assert!(index.needs_fetch(&PlaylistId(1), &root(1)));
		index.replace_playlist(PlaylistId(1), record(1, &[1]), &mut RefreshStats::default());
		assert!(!index.needs_fetch(&PlaylistId(1), &root(1)));
		Ok(())
	}

	fn page(total: i32, position: i32, truncated: bool, count: usize) -> SelectedListContent {
		let mut page = SelectedListContent::new();
		page.set_length(total);
		let mut contents = ListItems::new();
		contents.set_pos(position);
		contents.set_truncated(truncated);
		contents.items = (0..count).map(|_| Item::new()).collect();
		page.contents = MessageField::some(contents);
		page
	}

	#[test]
	fn pagination_rejects_partial_nonprogressing_and_misaligned_pages() -> Result<(), IndexError> {
		assert_eq!(page_end(&page(5, 0, true, 2), 0)?, 2);
		assert_eq!(page_end(&page(5, 2, false, 3), 2)?, 5);
		assert_eq!(page_end(&page(0, 0, false, 0), 0)?, 0);
		assert!(page_end(&page(5, 0, false, 2), 0).is_err());
		assert!(page_end(&page(5, 0, true, 0), 0).is_err());
		assert!(page_end(&page(5, 0, true, 2), 2).is_err());
		assert!(page_end(&page(1, 0, false, 2), 0).is_err());
		assert!(page_end(&page(-1, 0, false, 0), 0).is_err());
		Ok(())
	}

	fn decorated_item(uri: &str, owner: &str) -> (Item, MetaItem) {
		let mut item = Item::new();
		item.set_uri(uri.to_owned());
		let mut meta = MetaItem::new();
		meta.set_owner_username(owner.to_owned());
		meta.set_revision(vec![1]);
		meta.set_status_code(200);
		let mut attributes = ListAttributes::new();
		attributes.set_name("test playlist".to_owned());
		meta.attributes = MessageField::some(attributes);
		(item, meta)
	}

	#[test]
	fn rootlist_filters_followed_playlists_and_folders_and_coalesces_duplicates() -> Result<(), IndexError> {
		let mut contents = ListItems::new();
		for (uri, owner) in [
			("spotify:playlist:0000000000000000000001", "alice"),
			("spotify:playlist:0000000000000000000002", "bob"),
			("spotify:start-group:folder", ""),
			("spotify:playlist:0000000000000000000001", "alice"),
		] {
			let (item, meta) = decorated_item(uri, owner);
			contents.items.push(item);
			contents.meta_items.push(meta);
		}
		let mut roots = HashMap::new();
		collect_root_page(&contents, "alice", &mut roots)?;
		assert_eq!(roots.len(), 1);
		assert!(roots.contains_key(&PlaylistId(1)));
		contents.meta_items.pop();
		assert!(collect_root_page(&contents, "alice", &mut roots).is_err());
		Ok(())
	}

	#[test]
	fn track_collection_ignores_episodes_and_local_files_but_keeps_duplicates() -> Result<(), IndexError> {
		let mut contents = ListItems::new();
		for uri in [
			"spotify:track:0000000000000000000001",
			"spotify:episode:0000000000000000000002",
			"spotify:local:artist:album:track:180",
			"spotify:track:0000000000000000000001",
		] {
			let mut item = Item::new();
			item.set_uri(uri.to_owned());
			contents.items.push(item);
		}
		let mut tracks = Vec::new();
		collect_tracks(&contents, &mut tracks)?;
		assert_eq!(tracks, [TrackId(1), TrackId(1)]);
		Ok(())
	}

	#[test]
	fn typed_ids_accept_legacy_playlist_uris_and_reject_wrong_types() -> Result<(), IndexError> {
		assert_eq!(PlaylistId::from_uri("spotify:user:alice:playlist:0000000000000000000001")?, PlaylistId(1));
		assert_eq!(TrackId::from_uri("spotify:track:0000000000000000000002")?.spotify_id().id, 2);
		assert!(TrackId::from_uri("spotify:episode:0000000000000000000002").is_err());
		assert!(PlaylistId::from_uri("spotify:track:0000000000000000000001").is_err());
		Ok(())
	}

	#[test]
	fn worker_add_persists_and_publishes_only_owned_playlist_membership() -> Result<(), IndexError> {
		let directory = tempfile::tempdir()?;
		let mut index = PlaylistIndex::load(directory.path(), "alice")?;
		index.replace_playlist(PlaylistId(1), record(1, &[2]), &mut RefreshStats::default());
		let mut membership = Membership::new(directory.path());
		membership.index = Some(index);
		let responses = std::cell::RefCell::new(Vec::new());
		membership.appended("spotify:playlist:0000000000000000000001", &["spotify:track:0000000000000000000001".into()], &|response| {
			responses.borrow_mut().push(response);
		});
		assert!(
			matches!(responses.borrow().as_slice(), [super::Response::Added { tracks, stats: None }] if tracks.as_ref() == [TrackId(1), TrackId(2)])
		);
		let loaded = PlaylistIndex::load(directory.path(), "alice")?;
		assert!(loaded.contains(&TrackId(1)));
		assert!(loaded.needs_fetch(&PlaylistId(1), &root(1)));
		membership.appended("spotify:playlist:0000000000000000000002", &["spotify:track:0000000000000000000003".into()], &|response| {
			responses.borrow_mut().push(response);
		});
		assert!(!membership.tracks.contains(&TrackId(3)), "followed or unindexed playlists cannot be optimistically marked owned");
		assert_eq!(responses.borrow().len(), 1);
		Ok(())
	}

	#[test]
	fn detaching_session_clears_published_membership_without_deleting_account_cache() -> Result<(), IndexError> {
		let directory = tempfile::tempdir()?;
		let mut index = PlaylistIndex::load(directory.path(), "alice")?;
		index.replace_playlist(PlaylistId(1), record(1, &[1]), &mut RefreshStats::default());
		index.persist()?;
		let mut membership = Membership::new(directory.path());
		membership.index = Some(index);
		membership.snapshot();
		let responses = std::cell::RefCell::new(Vec::new());
		membership.attach(None, &|response| responses.borrow_mut().push(response));
		assert!(matches!(responses.borrow().as_slice(), [super::Response::Added { tracks, stats: None }] if tracks.is_empty()));
		assert!(membership.index.is_none());
		assert!(PlaylistIndex::load(directory.path(), "alice")?.contains(&TrackId(1)));
		Ok(())
	}

	#[test]
	fn premium_rejection_forgets_membership_and_deletes_account_cache() -> Result<(), IndexError> {
		let directory = tempfile::tempdir()?;
		let mut index = PlaylistIndex::load(directory.path(), "alice")?;
		index.replace_playlist(PlaylistId(1), record(1, &[1]), &mut RefreshStats::default());
		index.persist()?;
		let mut membership = Membership::new(directory.path());
		membership.index = Some(index);
		membership.snapshot();
		let responses = std::cell::RefCell::new(Vec::new());
		membership.forget(&|response| responses.borrow_mut().push(response));
		assert!(matches!(responses.borrow().as_slice(), [super::Response::Added { tracks, stats: None }] if tracks.is_empty()));
		assert!(membership.reader.is_none());
		assert!(membership.index.is_none());
		assert!(!directory.path().join("playlist-index").exists());
		membership.forget(&|_| {}); // Forgetting twice is harmless and cannot resurrect the index.
		Ok(())
	}

	#[test]
	fn membership_snapshot_reuses_shared_tracks_but_detects_same_size_replacement() -> Result<(), IndexError> {
		let directory = tempfile::tempdir()?;
		let mut index = PlaylistIndex::load(directory.path(), "alice")?;
		index.replace_playlist(PlaylistId(1), record(1, &[1, 2]), &mut RefreshStats::default());
		let mut membership = Membership::new(directory.path());
		membership.index = Some(index);
		membership.snapshot();
		let original = membership.tracks.clone();
		membership.index.as_mut().ok_or(IndexError::InvalidData("missing test index"))?.replace_playlist(
			PlaylistId(2),
			record(1, &[1]),
			&mut RefreshStats::default(),
		);
		membership.snapshot();
		assert!(std::sync::Arc::ptr_eq(&original, &membership.tracks));
		membership.index.as_mut().ok_or(IndexError::InvalidData("missing test index"))?.replace_playlist(
			PlaylistId(1),
			record(2, &[3]),
			&mut RefreshStats::default(),
		);
		membership.snapshot();
		assert_eq!(membership.tracks.as_ref(), &[TrackId(1), TrackId(3)]);
		assert!(!std::sync::Arc::ptr_eq(&original, &membership.tracks));
		Ok(())
	}
}
