//! Blocking Web API client, endpoint methods and shared authenticated request handling.

use std::{
	sync::{Arc, Mutex, PoisonError},
	time::Duration,
};

use serde::de::DeserializeOwned;

use super::{
	auth::{Session, TokenSource},
	error::{Error, Result},
	http::{Fetched, Http, Method, Req, urlencode},
	model::{
		Album, Artist, Device, Entry, Following, Page, PlayContext, Playback, Played, Playlist, RawDevices, RawSearch, RepeatMode,
		SavedAlbum, SearchResults, Track, User,
	},
};
use crate::net::{
	cache::{Cache, Flights},
	trace,
};

/// A GET answered within this long ago is answered again from memory: the home page and Your Library
/// both start from the first page of the same lists, and a page and its prefetch from the same album.
const RECENT_TTL: Duration = Duration::from_secs(10);
/// Recent answers kept, and the largest worth keeping.
const RECENT_ENTRIES: usize = 6;
const RECENT_MAX_BYTES: usize = 256 * 1024;

const API: &str = "https://api.spotify.com/v1";
/// Development Mode caps (February 2026): search 10, everything else 50.
const MAX_PAGE: u32 = 50;
const SEARCH_LIMIT: u32 = 10;
/// `/me/library*` accept at most 40 URIs per call.
const MAX_URIS: usize = 40;
const JSON_LIMIT: u64 = 4 * 1024 * 1024;
/// Only these fields are needed from a full playlist; skips the embedded first page of items.
const PLAYLIST_FIELDS: &str = "id,uri,name,description,public,collaborative,images,owner(id,display_name),items.total,tracks.total";

/// Blocking Web API client. Refreshes the access token on demand and persists rotated tokens.
/// Methods take `&self`: threads share one client. The token sits behind a lock, and identical GETs
/// in flight at once are one request.
pub struct Client {
	http:    Http,
	session: Mutex<Session>,
	base:    String,
	/// GETs in flight, by URL: a caller asking for one that is under way waits for it.
	flights: Flights<String, Result<Arc<Vec<u8>>>>,
	/// The last few GET answers (see [`RECENT_TTL`]). Any write empties it.
	recent:  Mutex<Cache<String, Arc<Vec<u8>>>>,
}

/// Playlist item writes take at most this many items per request.
const MAX_PLAYLIST_ITEMS: usize = 100;
/// `GET /artists/{id}/albums` takes `limit` up to 10 (since February 2026).
pub const MAX_ARTIST_ALBUMS: u32 = 10;

/// What to start playing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayStart {
	/// An album, playlist or artist context, optionally at a given track and position.
	Context { uri: String, offset_uri: Option<String>, position_ms: u64 },
	/// An explicit track list for sources without a playable context (Liked Songs, search
	/// results), starting at the chosen track.
	Tracks { uris: Vec<String>, offset_uri: Option<String> },
	/// A list played from its first track at `position_ms`: a queue rebuilt after an edit.
	List { uris: Vec<String>, position_ms: u64 },
}

/// A transport command for the device that is playing (Spotify's `/me/player/*` endpoints).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerCommand {
	Resume,
	Pause,
	Next,
	Previous,
	Seek(u64),
	/// Percent, 0–100.
	Volume(u8),
	Shuffle(bool),
	Repeat(RepeatMode),
}

impl Client {
	/// A client that authenticates with tokens from `source` (the player's Spotify session).
	pub fn open(source: TokenSource) -> Self {
		// The UI harness points an isolated run (TERNTABLE_HOME) at its mock Web API;
		// a normal run always talks to Spotify.
		let base = crate::app::config::home_override()
			.and_then(|_| std::env::var("TERNTABLE_API").ok())
			.filter(|b| !b.is_empty())
			.unwrap_or_else(|| API.to_owned());
		Self::new(Session::new(source), base)
	}

	fn new(session: Session, base: String) -> Self {
		Self {
			http: Http::new(),
			session: Mutex::new(session),
			base,
			flights: Flights::default(),
			recent: Mutex::new(Cache::new(RECENT_ENTRIES, RECENT_TTL)),
		}
	}

	/// The profile and the subscription (`premium`, `free`, `open`). The subscription only comes
	/// with the `user-read-private` permission, so a sign-in from before it has none.
	pub fn me_with_plan(&self) -> Result<(User, Option<String>)> {
		#[derive(serde::Deserialize)]
		struct Plan {
			product: Option<String>,
		}
		let body = self.request(Method::Get, "/me", &[], None)?;
		Ok((serde_json::from_slice(&body)?, serde_json::from_slice::<Plan>(&body)?.product))
	}

	pub fn playlists(&self, offset: u32, limit: u32) -> Result<Page<Playlist>> {
		self.get_page("/me/playlists", offset, limit)
	}

	/// Playlist metadata only (no items).
	pub fn playlist(&self, id: &str) -> Result<Playlist> {
		self.get_json(&format!("/playlists/{}", urlencode(id)), &[("fields", PLAYLIST_FIELDS)])
	}

	/// Items of a playlist the user owns or collaborates on; others fail with a 403
	/// ([`Error::is_forbidden`]). Null/removed entries are dropped from `items`; `next_offset`
	/// still points at the following raw page.
	pub fn playlist_items(&self, id: &str, offset: u32, limit: u32) -> Result<Page<Track>> {
		let page: Page<Entry> = self.get_page(&format!("/playlists/{}/items", urlencode(id)), offset, limit)?;
		Ok(page.filter_map(Entry::into_track))
	}

	pub fn saved_tracks(&self, offset: u32, limit: u32) -> Result<Page<Track>> {
		let page: Page<Entry> = self.get_page("/me/tracks", offset, limit)?;
		Ok(page.filter_map(Entry::into_track))
	}

	/// Tracks, artists, albums and playlists matching `query`, 10 of each starting at `offset`.
	pub fn search(&self, query: &str, offset: u32) -> Result<SearchResults> {
		let offset = offset.to_string();
		let limit = SEARCH_LIMIT.to_string();
		let raw: RawSearch =
			self.get_json("/search", &[("q", query), ("type", "track,artist,album,playlist"), ("limit", &limit), ("offset", &offset)])?;
		Ok(raw.into())
	}

	/// Tracks of an album (search results link to albums only). Single request, up to 50 tracks.
	pub fn album_tracks(&self, id: &str, offset: u32, limit: u32) -> Result<Page<Track>> {
		self.get_page(&format!("/albums/{}/tracks", urlencode(id)), offset, limit)
	}

	/// An album with its first page of tracks, which the album object carries: one request where the
	/// album and its tracks endpoint took two. The page is `None` when the answer has none.
	pub fn album_full(&self, id: &str) -> Result<(Album, Option<Page<Track>>)> {
		#[derive(serde::Deserialize)]
		struct Embedded {
			tracks: Option<Page<Track>>,
		}
		let body = self.get_bytes(&format!("/albums/{}", urlencode(id)), &[])?;
		Ok((serde_json::from_slice(&body)?, serde_json::from_slice::<Embedded>(&body)?.tracks))
	}

	/// Whether each URI (track, album, playlist, ...) is in the user's library.
	pub fn library_contains(&self, uris: &[&str]) -> Result<Vec<bool>> {
		let mut out = Vec::with_capacity(uris.len());
		for chunk in uris.chunks(MAX_URIS) {
			let joined = chunk.join(",");
			let part: Vec<bool> = self.get_json("/me/library/contains", &[("uris", &joined)])?;
			out.extend(part);
		}
		Ok(out)
	}

	pub fn save(&self, uris: &[&str]) -> Result<()> {
		self.modify_library(Method::Put, uris)
	}

	pub fn unsave(&self, uris: &[&str]) -> Result<()> {
		self.modify_library(Method::Delete, uris)
	}

	/// The user's most recently played tracks with what they were played from, newest
	/// first (duplicates are possible). Max 50. Removed tracks are dropped.
	pub fn recently_played(&self, limit: u32) -> Result<Vec<(Track, Option<PlayContext>)>> {
		let limit = limit.clamp(1, MAX_PAGE).to_string();
		let page: Page<Played> = self.get_json("/me/player/recently-played", &[("limit", &limit)])?;
		Ok(page.items.into_iter().filter_map(|p| Some((p.track?, p.context))).collect())
	}

	/// The user's top artists of the last ~6 months. Needs the `user-top-read` scope:
	/// a token from before terntable asked for it gets a 403 ([`Error::is_forbidden`]).
	pub fn top_artists(&self, limit: u32) -> Result<Vec<Artist>> {
		let limit = limit.clamp(1, MAX_PAGE).to_string();
		let page: Page<Artist> = self.get_json("/me/top/artists", &[("limit", &limit), ("time_range", "medium_term")])?;
		Ok(page.items)
	}

	/// The user's top tracks of the last ~6 months (`user-top-read`, as [`Self::top_artists`]).
	pub fn top_tracks(&self, limit: u32) -> Result<Vec<Track>> {
		let limit = limit.clamp(1, MAX_PAGE).to_string();
		let page: Page<Track> = self.get_json("/me/top/tracks", &[("limit", &limit), ("time_range", "medium_term")])?;
		Ok(page.items)
	}

	/// A page of the albums saved to the user's library, most recently added first.
	pub fn saved_albums(&self, offset: u32, limit: u32) -> Result<Page<Album>> {
		let page: Page<SavedAlbum> = self.get_page("/me/albums", offset, limit)?;
		Ok(page.filter_map(|s| s.album))
	}

	/// An artist's most popular tracks, as far as search can tell: the top tracks
	/// endpoint is gone since February 2026, and search ranks by popularity.
	pub fn artist_tracks(&self, artist: &Artist) -> Result<Vec<Track>> {
		let query = format!("artist:\"{}\"", artist.name.replace('"', ""));
		let limit = SEARCH_LIMIT.to_string();
		let raw: RawSearch = self.get_json("/search", &[("q", &query), ("type", "track"), ("limit", &limit)])?;
		Ok(SearchResults::from(raw).tracks.items.into_iter().filter(|t| t.artists.iter().any(|a| a.uri == artist.uri)).collect())
	}

	/// A page of the artists the user follows (`user-follow-read`). Paging is by cursor: `after`
	/// is the last artist of the previous page.
	pub fn followed_artists(&self, after: Option<&str>, limit: u32) -> Result<Page<Artist>> {
		let limit = limit.clamp(1, MAX_PAGE).to_string();
		let query = [("type", "artist"), ("limit", &limit), ("after", after.unwrap_or_default())];
		let raw: Following = self.get_json("/me/following", if after.is_some() { &query } else { &query[..2] })?;
		Ok(raw.artists)
	}

	/// A new private playlist of the user's (`playlist-modify-private`).
	pub fn create_playlist(&self, name: &str) -> Result<Playlist> {
		let body = serde_json::json!({ "name": name, "public": false }).to_string();
		let reply = self.request(Method::Post, "/me/playlists", &[], Some(&body))?;
		Ok(serde_json::from_slice(&reply)?)
	}

	pub fn rename_playlist(&self, id: &str, changes: &super::PlaylistChanges) -> Result<()> {
		let body = serde_json::to_string(changes)?;
		self.request(Method::Put, &format!("/playlists/{}", urlencode(id)), &[], Some(&body))?;
		Ok(())
	}

	/// Appends `uris` to a playlist the user owns or collaborates on.
	pub fn add_to_playlist(&self, id: &str, uris: &[&str]) -> Result<()> {
		let path = format!("/playlists/{}/items", urlencode(id));
		for chunk in uris.chunks(MAX_PLAYLIST_ITEMS) {
			let body = serde_json::json!({ "uris": chunk }).to_string();
			self.request(Method::Post, &path, &[], Some(&body))?;
		}
		Ok(())
	}

	/// Removes every occurrence of `uris` from a playlist the user owns or collaborates on.
	pub fn remove_from_playlist(&self, id: &str, uris: &[&str]) -> Result<()> {
		let path = format!("/playlists/{}/items", urlencode(id));
		for chunk in uris.chunks(MAX_PLAYLIST_ITEMS) {
			let items: Vec<_> = chunk.iter().map(|uri| serde_json::json!({ "uri": uri })).collect();
			let body = serde_json::json!({ "items": items }).to_string();
			self.request(Method::Delete, &path, &[], Some(&body))?;
		}
		Ok(())
	}

	/// An artist: name, images, genres (followers and popularity are gone since February 2026).
	pub fn artist(&self, id: &str) -> Result<Artist> {
		self.get_json(&format!("/artists/{}", urlencode(id)), &[])
	}

	/// An artist's releases of `groups` (`album`, `single`, `compilation`, `appears_on`; comma-joined).
	/// At most [`MAX_ARTIST_ALBUMS`] per call: Spotify answers 400 to a larger `limit`.
	pub fn artist_albums(&self, id: &str, groups: &str, limit: u32) -> Result<Vec<Album>> {
		let limit = limit.clamp(1, MAX_ARTIST_ALBUMS).to_string();
		let page: Page<Album> =
			self.get_json(&format!("/artists/{}/albums", urlencode(id)), &[("include_groups", groups), ("limit", &limit)])?;
		Ok(page.items)
	}

	/// Connect devices of the user. Devices Spotify reports without an id (restricted) cannot be
	/// targeted and are left out. Never answered from the recent-GET memory: the device menu and
	/// the playback checks must see devices as they are now.
	pub fn devices(&self) -> Result<Vec<Device>> {
		let body = self.request(Method::Get, "/me/player/devices", &[], None)?;
		let raw: RawDevices = serde_json::from_slice(&body)?;
		Ok(raw.devices.into_iter().filter(|d| !d.id.is_empty()).collect())
	}

	/// What plays, and on which device: `None` when no device is active (HTTP 204).
	pub fn playback(&self) -> Result<Option<Playback>> {
		let body = self.request(Method::Get, "/me/player", &[], None)?;
		if body.iter().all(u8::is_ascii_whitespace) {
			return Ok(None);
		}
		Ok(Some(serde_json::from_slice(&body)?))
	}

	/// Moves playback to `device_id`, playing or staying paused as `play` says.
	/// [`Error::DeviceNotFound`] when Spotify does not know the device.
	pub fn transfer(&self, device_id: &str, play: bool) -> Result<()> {
		let body = serde_json::json!({ "device_ids": [device_id], "play": play }).to_string();
		self.request(Method::Put, "/me/player", &[], Some(&body))?;
		Ok(())
	}

	/// Sends a transport command to `device_id`.
	pub fn control(&self, device_id: &str, command: &PlayerCommand) -> Result<()> {
		let value = match command {
			PlayerCommand::Seek(ms) => ms.to_string(),
			PlayerCommand::Volume(percent) => percent.min(&100).to_string(),
			PlayerCommand::Shuffle(on) => on.to_string(),
			PlayerCommand::Repeat(mode) => mode.as_str().to_owned(),
			PlayerCommand::Resume | PlayerCommand::Pause | PlayerCommand::Next | PlayerCommand::Previous => String::new(),
		};
		let (method, path, key) = match command {
			PlayerCommand::Resume => (Method::Put, "/me/player/play", ""),
			PlayerCommand::Pause => (Method::Put, "/me/player/pause", ""),
			PlayerCommand::Next => (Method::Post, "/me/player/next", ""),
			PlayerCommand::Previous => (Method::Post, "/me/player/previous", ""),
			PlayerCommand::Seek(_) => (Method::Put, "/me/player/seek", "position_ms"),
			PlayerCommand::Volume(_) => (Method::Put, "/me/player/volume", "volume_percent"),
			PlayerCommand::Shuffle(_) => (Method::Put, "/me/player/shuffle", "state"),
			PlayerCommand::Repeat(_) => (Method::Put, "/me/player/repeat", "state"),
		};
		let mut query = vec![("device_id", device_id)];
		if !key.is_empty() {
			query.push((key, value.as_str()));
		}
		self.request(method, path, &query, None)?;
		Ok(())
	}

	fn modify_library(&self, method: Method, uris: &[&str]) -> Result<()> {
		for chunk in uris.chunks(MAX_URIS) {
			let joined = chunk.join(",");
			self.request(method, "/me/library", &[("uris", &joined)], None)?;
		}
		Ok(())
	}

	fn get_page<T: DeserializeOwned>(&self, path: &str, offset: u32, limit: u32) -> Result<Page<T>> {
		let offset = offset.to_string();
		let limit = limit.clamp(1, MAX_PAGE).to_string();
		self.get_json(path, &[("offset", &offset), ("limit", &limit)])
	}

	fn get_json<T: DeserializeOwned>(&self, path: &str, query: &[(&str, &str)]) -> Result<T> {
		Ok(serde_json::from_slice(&self.get_bytes(path, query)?)?)
	}

	/// The body of a GET. Identical requests in flight at once are one request, and one answered a
	/// moment ago is not asked again.
	fn get_bytes(&self, path: &str, query: &[(&str, &str)]) -> Result<Arc<Vec<u8>>> {
		let key = format!("{path}?{}", query.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&"));
		let recent = self.recent.lock().unwrap_or_else(PoisonError::into_inner).get(&key);
		if let Some(body) = recent {
			trace::hit("api", &trace::endpoint(&format!("{}{path}", self.base)), "mem");
			return Ok(body);
		}
		let span = trace::start();
		let (answer, joined) = self.flights.run(&key, || self.request(Method::Get, path, query, None).map(Arc::new));
		match &answer {
			Ok(body) if joined => {
				let reply = trace::Reply { status: 200, bytes: body.len(), via: trace::Via::Joined, ..trace::Reply::default() };
				span.done("api", "GET", &format!("{}{path}", self.base), reply);
			},
			Ok(body) if body.len() <= RECENT_MAX_BYTES => {
				self.recent.lock().unwrap_or_else(PoisonError::into_inner).insert(key, body.clone());
			},
			_ => {},
		}
		answer
	}

	/// Lets go of the recent GET answers (they only serve requests made close together).
	pub fn forget_recent(&self) {
		self.recent.lock().unwrap_or_else(PoisonError::into_inner).clear();
	}

	/// Authenticated call. A 401 triggers one forced refresh and one retry.
	fn request(&self, method: Method, path: &str, query: &[(&str, &str)], json: Option<&str>) -> Result<Vec<u8>> {
		Ok(self.fetch(method, path, query, json, None)?.body)
	}

	/// [`Self::request`] for a copy held under `etag`.
	fn fetch(&self, method: Method, path: &str, query: &[(&str, &str)], json: Option<&str>, etag: Option<&str>) -> Result<Fetched> {
		if method != Method::Get {
			// What was read before this write may not hold after it.
			self.recent.lock().unwrap_or_else(PoisonError::into_inner).clear();
		}
		let url = format!("{}{path}", self.base);
		let mut token = self.token()?;
		let mut refreshed = false;
		loop {
			let result = self
				.http
				.send_conditional(&Req { method, url: &url, query, bearer: Some(&token), form: None, json, limit: JSON_LIMIT }, etag);
			match result {
				Err(Error::AuthExpired) if !refreshed => {
					refreshed = true;
					token = self.refreshed(&token)?;
				},
				other => return other,
			}
		}
	}

	/// An access token with time left, renewed first when it runs out.
	fn token(&self) -> Result<String> {
		self.session.lock().unwrap_or_else(PoisonError::into_inner).access_token()
	}

	/// A new token after Spotify refused `stale`, unless another thread has renewed it already.
	fn refreshed(&self, stale: &str) -> Result<String> {
		let mut session = self.session.lock().unwrap_or_else(PoisonError::into_inner);
		match session.current() {
			Some(current) if current != stale => Ok(current.to_owned()),
			_ => session.renew(),
		}
	}
}

#[cfg(test)]
mod tests;
