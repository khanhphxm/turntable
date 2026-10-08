//! Lean Web API models: only what the UI renders. Everything optional in practice is
//! `Option`/defaulted, since Development Mode strips many fields and Spotify returns `null`
//! liberally.

use serde::{Deserialize, Deserializer, Serialize};

/// `null` and missing both become `T::default()`.
fn or_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
	D: Deserializer<'de>,
	T: Default + Deserialize<'de>,
{
	Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Image {
	pub url:    String,
	pub width:  Option<u32>,
	pub height: Option<u32>,
}

/// Picks the smallest image at least `target_px` wide, else the largest one available.
/// Images without dimensions (common for playlists) are only used when none has dimensions:
/// Spotify lists them widest first, so that is the first entry.
pub fn pick_image(images: &[Image], target_px: u32) -> Option<&Image> {
	let sized = images.iter().filter_map(|i| i.width.map(|w| (w, i)));
	let at_least = sized.clone().filter(|(w, _)| *w >= target_px).min_by_key(|(w, _)| *w);
	at_least.or_else(|| sized.max_by_key(|(w, _)| *w)).map(|(_, i)| i).or_else(|| images.first())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
	pub id:           String,
	pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artist {
	pub name:   String,
	#[serde(default, deserialize_with = "or_default")]
	pub uri:    String,
	/// Full artist objects only (`/artists/{id}`, top artists); empty in tracks.
	#[serde(default, deserialize_with = "or_default")]
	pub images: Vec<Image>,
	#[serde(default, deserialize_with = "or_default")]
	pub genres: Vec<String>,
}

/// An album, or the show of a podcast episode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Album {
	pub name:         String,
	#[serde(default, deserialize_with = "or_default")]
	pub uri:          String,
	#[serde(default, deserialize_with = "or_default")]
	pub images:       Vec<Image>,
	/// Only present on search results and full album objects.
	#[serde(default, deserialize_with = "or_default")]
	pub artists:      Vec<Artist>,
	#[serde(default, deserialize_with = "or_default")]
	pub total_tracks: u32,
	pub release_date: Option<String>,
	/// `album`, `single` or `compilation` (empty when Spotify leaves it out).
	#[serde(default, deserialize_with = "or_default")]
	pub album_type:   String,
	/// Full album objects only: "© 2018 …" and "℗ 2018 …" lines, as Spotify lists them.
	#[serde(default, deserialize_with = "or_default")]
	pub copyrights:   Vec<Copyright>,
	/// Record label, when supplied by album metadata.
	pub label:        Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Copyright {
	pub text: String,
	/// `C` (copyright) or `P` (sound recording copyright).
	#[serde(rename = "type", default, deserialize_with = "or_default")]
	pub kind: String,
}

impl Copyright {
	/// The footer line with its copyright symbol, including legacy `(P)`/`(C)` prefixes.
	pub fn line(&self) -> String {
		let text = self.text.trim();
		if text.starts_with(['©', '℗']) {
			return text.to_owned();
		}
		let (symbol, text) = if let Some(text) = text.strip_prefix("(P)").or_else(|| text.strip_prefix("(p)")) {
			('℗', text.trim_start())
		} else if let Some(text) = text.strip_prefix("(C)").or_else(|| text.strip_prefix("(c)")) {
			('©', text.trim_start())
		} else {
			(if self.kind == "P" { '℗' } else { '©' }, text)
		};
		format!("{symbol} {text}")
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Track {
	pub uri:         String,
	pub name:        String,
	pub duration_ms: u32,
	/// One-based album disc number; zero when unknown or not applicable.
	pub disc_number: u32,
	pub explicit:    bool,
	/// Empty for podcast episodes.
	pub artists:     Vec<Artist>,
	/// The album, or the show for an episode.
	pub album:       Option<Album>,
	/// Local files cannot be played by the built-in player.
	pub is_local:    bool,
	pub is_episode:  bool,
}

#[derive(Deserialize)]
struct RawTrack {
	#[serde(default, deserialize_with = "or_default")]
	uri:         String,
	#[serde(default, deserialize_with = "or_default")]
	name:        String,
	#[serde(default, deserialize_with = "or_default")]
	duration_ms: u32,
	#[serde(default, deserialize_with = "or_default")]
	disc_number: u32,
	#[serde(default, deserialize_with = "or_default")]
	explicit:    bool,
	#[serde(default, deserialize_with = "or_default")]
	artists:     Vec<Artist>,
	album:       Option<Album>,
	show:        Option<Album>,
	#[serde(rename = "type")]
	kind:        Option<String>,
	#[serde(default, deserialize_with = "or_default")]
	is_local:    bool,
	/// Present in a library snapshot (Web API episodes use `type`/`show`).
	#[serde(default)]
	is_episode:  bool,
}

impl<'de> Deserialize<'de> for Track {
	fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
		let raw = RawTrack::deserialize(d)?;
		Ok(Self {
			is_episode:  raw.is_episode || raw.kind.as_deref() == Some("episode") || (raw.album.is_none() && raw.show.is_some()),
			uri:         raw.uri,
			name:        raw.name,
			duration_ms: raw.duration_ms,
			disc_number: raw.disc_number,
			explicit:    raw.explicit,
			artists:     raw.artists,
			album:       raw.album.or(raw.show),
			is_local:    raw.is_local,
		})
	}
}

/// `{ "total": N }` inside a playlist (`items`, formerly `tracks`).
#[derive(Deserialize)]
struct Count {
	#[serde(default, deserialize_with = "or_default")]
	total: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Playlist {
	pub id:            String,
	pub uri:           String,
	pub name:          String,
	pub description:   Option<String>,
	/// Spotify omits visibility for some playlists.
	pub public:        Option<bool>,
	pub owner:         User,
	pub images:        Vec<Image>,
	pub collaborative: bool,
	/// `None` when Spotify omitted the item count (non-owned playlists in Development Mode).
	pub track_count:   Option<u32>,
}

#[derive(Deserialize)]
struct RawPlaylist {
	id:            String,
	#[serde(default, deserialize_with = "or_default")]
	uri:           String,
	#[serde(default, deserialize_with = "or_default")]
	name:          String,
	description:   Option<String>,
	public:        Option<bool>,
	owner:         User,
	#[serde(default, deserialize_with = "or_default")]
	images:        Vec<Image>,
	#[serde(default, deserialize_with = "or_default")]
	collaborative: bool,
	/// Post-February-2026 name.
	items:         Option<Count>,
	/// Deprecated name, still sent by some endpoints.
	tracks:        Option<Count>,
	/// Present in a library snapshot, rather than the Web API's nested count.
	track_count:   Option<u32>,
}

impl<'de> Deserialize<'de> for Playlist {
	fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
		let raw = RawPlaylist::deserialize(d)?;
		Ok(Self {
			uri:           if raw.uri.is_empty() { format!("spotify:playlist:{}", raw.id) } else { raw.uri },
			id:            raw.id,
			name:          raw.name,
			description:   raw.description.filter(|d| Playlist::has_description(d)),
			public:        raw.public,
			owner:         raw.owner,
			images:        raw.images,
			collaborative: raw.collaborative,
			track_count:   raw.track_count.or_else(|| raw.items.or(raw.tracks).map(|c| c.total)),
		})
	}
}

impl Playlist {
	/// Native playlist attributes can contain the literal null sentinel instead
	/// of an absent description, including in old disk snapshots.
	pub(crate) fn has_description(value: &str) -> bool {
		!value.trim().is_empty() && value.trim() != "null"
	}

	/// Human-readable attribution, never an opaque account ID.
	pub fn owner_name(&self) -> &str {
		if self.owner.id == "spotify" {
			return "Spotify";
		}
		self.owner.display_name.as_deref().map(str::trim).filter(|name| !name.is_empty()).unwrap_or("Spotify user")
	}

	/// Reuse the signed-in profile's name for owned playlists on every ingress.
	pub(crate) fn normalize(&mut self, account: &str, display_name: Option<&str>) {
		if self.description.as_deref().is_some_and(|description| !Self::has_description(description)) {
			self.description = None;
		}
		if self.owner.id == "spotify" && self.owner.display_name.as_deref() != Some("Spotify") {
			self.owner.display_name = Some("Spotify".to_owned());
		}
		if self.owner.id == account
			&& let Some(name) = display_name.filter(|name| !name.trim().is_empty())
			&& self.owner.display_name.as_deref() != Some(name)
		{
			self.owner.display_name = Some(name.to_owned());
		}
	}

	/// Whether `GET /playlists/{id}/items` is allowed: Development Mode only serves items of
	/// playlists the user owns or collaborates on (anything else is a 403).
	pub fn items_readable(&self, me: &str) -> bool {
		self.owner.id == me || self.collaborative
	}
}

/// Only changed playlist details are sent to Spotify; an empty description clears it.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct PlaylistChanges {
	#[serde(skip_serializing_if = "Option::is_none")]
	pub name:        Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub description: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub public:      Option<bool>,
}

impl PlaylistChanges {
	pub fn is_empty(&self) -> bool {
		self.name.is_none() && self.description.is_none() && self.public.is_none()
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
pub struct Page<T> {
	#[serde(default, deserialize_with = "or_default")]
	pub items:  Vec<T>,
	#[serde(default, deserialize_with = "or_default")]
	pub total:  u32,
	#[serde(default, deserialize_with = "or_default")]
	pub offset: u32,
	/// The page size Spotify applied (not `items.len()`: unplayable entries are dropped).
	#[serde(default, deserialize_with = "or_default")]
	pub limit:  u32,
	pub next:   Option<String>,
}

impl<T> Default for Page<T> {
	fn default() -> Self {
		Self { items: Vec::new(), total: 0, offset: 0, limit: 0, next: None }
	}
}

impl<T> Page<T> {
	/// Offset to request for the following page, if there is one.
	pub fn next_offset(&self) -> Option<u32> {
		self.next.as_ref().map(|_| self.offset + self.limit.max(1))
	}

	/// Transforms entries, dropping `None`s. Paging metadata is kept as Spotify sent it, so
	/// `next_offset` stays correct even when entries were dropped.
	pub fn filter_map<U>(self, f: impl FnMut(T) -> Option<U>) -> Page<U> {
		Page {
			items:  self.items.into_iter().filter_map(f).collect(),
			total:  self.total,
			offset: self.offset,
			limit:  self.limit,
			next:   self.next,
		}
	}
}

/// An entry of a playlist-items or saved-tracks page. The track is `item` since
/// February 2026 (`track` before), and may be `null` for removed content.
#[derive(Debug, Deserialize)]
pub struct Entry {
	item:  Option<Track>,
	track: Option<Track>,
}

impl Entry {
	pub fn into_track(self) -> Option<Track> {
		self.item.or(self.track)
	}
}

/// A recently played entry: the track, and what it was played from.
#[derive(Debug, Deserialize)]
pub struct Played {
	pub track:   Option<Track>,
	pub context: Option<PlayContext>,
}

/// Where a track was played from (`playlist`, `album`, `artist`, `collection` for Liked Songs).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PlayContext {
	#[serde(default, deserialize_with = "or_default")]
	pub uri:  String,
	#[serde(rename = "type", default, deserialize_with = "or_default")]
	pub kind: String,
}

/// An entry of the saved-albums page.
#[derive(Debug, Deserialize)]
pub struct SavedAlbum {
	pub album: Option<Album>,
}

/// The id part of `spotify:<kind>:<id>` (the whole string when it has no colon).
pub fn id_of(uri: &str) -> &str {
	uri.rsplit(':').next().unwrap_or(uri)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResults {
	pub tracks:    Page<Track>,
	pub albums:    Page<Album>,
	pub playlists: Page<Playlist>,
	pub artists:   Page<Artist>,
}

#[derive(Deserialize)]
pub(super) struct RawSearch {
	#[serde(default, deserialize_with = "or_default")]
	tracks:    Page<Track>,
	#[serde(default, deserialize_with = "or_default")]
	albums:    Page<Album>,
	/// Spotify interleaves `null`s in playlist results.
	#[serde(default, deserialize_with = "or_default")]
	playlists: Page<Option<Playlist>>,
	#[serde(default, deserialize_with = "or_default")]
	artists:   Page<Artist>,
}

/// `GET /me/following`: the artists sit in a cursor page under `artists`.
#[derive(Deserialize)]
pub(super) struct Following {
	pub artists: Page<Artist>,
}

impl From<RawSearch> for SearchResults {
	fn from(raw: RawSearch) -> Self {
		Self { tracks: raw.tracks, albums: raw.albums, playlists: raw.playlists.filter_map(|p| p), artists: raw.artists }
	}
}

/// A Spotify Connect device.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Device {
	/// Empty for restricted devices that cannot be targeted.
	#[serde(default, deserialize_with = "or_default")]
	pub id:              String,
	#[serde(default, deserialize_with = "or_default")]
	pub name:            String,
	#[serde(default, deserialize_with = "or_default")]
	pub is_active:       bool,
	pub volume_percent:  Option<u8>,
	/// Whether the device can be told a volume; Spotify leaves it out for some, then a missing
	/// `volume_percent` says the same (see [`Device::volume_capable`]).
	#[serde(default)]
	pub supports_volume: Option<bool>,
	/// "Computer", "Smartphone", "Speaker", ...
	#[serde(default, rename = "type", deserialize_with = "or_default")]
	pub kind:            String,
}

/// What a device looks like, from Spotify's device types (`Computer`, `Smartphone`, `TV`, ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
	Computer,
	/// Smartphones and tablets.
	Phone,
	/// Speakers, receivers, dongles and anything unknown.
	Speaker,
	/// TVs, set-top boxes, game consoles and Cast video devices.
	Tv,
}

impl Device {
	pub fn category(&self) -> DeviceKind {
		match self.kind.to_ascii_lowercase().as_str() {
			"computer" => DeviceKind::Computer,
			"smartphone" | "tablet" => DeviceKind::Phone,
			"tv" | "stb" | "castvideo" | "gameconsole" => DeviceKind::Tv,
			_ => DeviceKind::Speaker,
		}
	}

	/// Whether a volume can be set on the device.
	pub fn volume_capable(&self) -> bool {
		self.supports_volume.unwrap_or(self.volume_percent.is_some())
	}
}

/// How `repeat_state` loops.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RepeatMode {
	#[default]
	Off,
	Context,
	Track,
}

impl RepeatMode {
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::Off => "off",
			Self::Context => "context",
			Self::Track => "track",
		}
	}
}

/// `GET /me/player`: what plays on which device.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Playback {
	pub device:      Device,
	#[serde(default, deserialize_with = "or_default")]
	pub is_playing:  bool,
	#[serde(default, deserialize_with = "or_default")]
	pub progress_ms: u64,
	#[serde(default, rename = "shuffle_state", deserialize_with = "or_default")]
	pub shuffle:     bool,
	#[serde(default, rename = "repeat_state", deserialize_with = "or_default")]
	pub repeat:      RepeatMode,
	pub context:     Option<PlayContext>,
	/// `None` for ads and unknown content.
	pub item:        Option<Track>,
}

#[derive(Deserialize)]
pub(super) struct RawDevices {
	#[serde(default, deserialize_with = "or_default")]
	pub devices: Vec<Device>,
}

#[cfg(test)]
mod tests {
	use super::*;

	fn img(url: &str, w: Option<u32>) -> Image {
		Image { url: url.into(), width: w, height: w }
	}

	#[test]
	fn image_selection_covers_size_boundaries_unknown_dimensions_and_empty_input() {
		for (images, requested, expected) in [
			(vec![img("640", Some(640)), img("300", Some(300)), img("64", Some(64))], 200, Some("300")),
			(vec![img("640", Some(640)), img("64", Some(64))], 64, Some("64")),
			(vec![img("640", Some(640)), img("64", Some(64))], 1000, Some("640")),
			(vec![img("640", Some(640)), img("64", Some(64))], 1, Some("64")),
			(vec![img("first", None), img("second", None)], 300, Some("first")),
			(Vec::new(), 300, None),
		] {
			assert_eq!(pick_image(&images, requested).map(|image| image.url.as_str()), expected);
		}
	}

	const PLAYLIST_ITEMS: &str = r#"{
		"href": "https://api.spotify.com/v1/playlists/37i9dQZF1DX/items?offset=0&limit=2",
		"limit": 2, "offset": 0, "total": 5,
		"next": "https://api.spotify.com/v1/playlists/37i9dQZF1DX/items?offset=2&limit=2",
		"previous": null,
		"items": [
			{
				"added_at": "2026-03-01T10:00:00Z",
				"added_by": {"id": "me", "type": "user", "uri": "spotify:user:me"},
				"is_local": false,
				"item": {
					"type": "track",
					"uri": "spotify:track:4uLU6hMCjMI75M1A2tKUQC",
					"name": "Never Gonna Give You Up",
					"duration_ms": 213573,
					"explicit": false,
					"popularity": 80,
					"artists": [{"id": "0gxyHStUsqpMadRV0Di1Qt", "name": "Rick Astley", "uri": "spotify:artist:0gxyHStUsqpMadRV0Di1Qt", "type": "artist"}],
					"album": {
						"album_type": "album", "total_tracks": 10, "name": "Whenever You Need Somebody",
						"uri": "spotify:album:6N9PS4QXF1D0OWPk0Sxtb4", "release_date": "1987-11-12",
						"images": [{"url": "https://i.scdn.co/image/a640", "height": 640, "width": 640},
						           {"url": "https://i.scdn.co/image/a64", "height": 64, "width": 64}],
						"artists": [{"name": "Rick Astley", "uri": "spotify:artist:0gxyHStUsqpMadRV0Di1Qt"}]
					}
				}
			},
			{"added_at": null, "is_local": false, "item": null},
			{
				"is_local": false,
				"item": {
					"type": "episode",
					"uri": "spotify:episode:512ojhOuo1ktJprKbVcKyQ",
					"name": "Episode 1", "duration_ms": 1800000, "explicit": true,
					"show": {"name": "Some Show", "uri": "spotify:show:38bS44xjbVVZ3No3ByF1dJ",
					         "images": [{"url": "https://i.scdn.co/image/show", "height": null, "width": null}]}
				}
			},
			{
				"track": {"type": "track", "uri": "spotify:track:old", "name": "Legacy Shape", "duration_ms": 1000,
				          "artists": [{"name": "A"}], "album": {"name": "B"}}
			}
		]
	}"#;

	#[test]
	fn playlist_items_page_2026_shape() {
		let page: Page<Entry> = serde_json::from_str(PLAYLIST_ITEMS).unwrap();
		assert_eq!((page.total, page.offset, page.limit), (5, 0, 2));
		assert_eq!(page.next_offset(), Some(2));
		let page = page.filter_map(Entry::into_track);
		// The null entry is dropped, paging metadata is untouched.
		assert_eq!(page.items.len(), 3);
		assert_eq!(page.next_offset(), Some(2));

		let song = &page.items[0];
		assert_eq!(song.uri, "spotify:track:4uLU6hMCjMI75M1A2tKUQC");
		assert_eq!(song.duration_ms, 213_573);
		assert!(!song.explicit && !song.is_episode && !song.is_local);
		assert_eq!(song.artists[0].name, "Rick Astley");
		let album = song.album.as_ref().unwrap();
		assert_eq!(album.name, "Whenever You Need Somebody");
		assert_eq!(pick_image(&album.images, 100).unwrap().url, "https://i.scdn.co/image/a640");

		let episode = &page.items[1];
		assert!(episode.is_episode && episode.explicit);
		assert!(episode.artists.is_empty());
		assert_eq!(episode.album.as_ref().unwrap().name, "Some Show");

		// Deprecated `track` key and sparse album/artist objects still parse.
		let legacy = &page.items[2];
		assert_eq!(legacy.name, "Legacy Shape");
		assert_eq!(legacy.album.as_ref().unwrap().uri, "");
		assert_eq!(legacy.artists[0].uri, "");
	}

	#[test]
	fn last_page_has_no_next() {
		let page: Page<Entry> = serde_json::from_str(r#"{"limit":50,"offset":100,"total":103,"next":null,"items":[]}"#).unwrap();
		assert_eq!(page.next_offset(), None);
	}

	#[test]
	fn owned_playlist_with_items_count() {
		let json = r#"{
			"id": "3cEYpjA9oz9GiPac4AsH4n", "uri": "spotify:playlist:3cEYpjA9oz9GiPac4AsH4n",
			"name": "Road trip", "description": "", "collaborative": false, "public": false,
			"snapshot_id": "abc", "images": null,
			"owner": {"id": "me", "display_name": "Me", "uri": "spotify:user:me", "type": "user"},
			"items": {"href": "https://api.spotify.com/v1/playlists/3cE/items", "total": 42}
		}"#;
		let p: Playlist = serde_json::from_str(json).unwrap();
		assert_eq!(p.track_count, Some(42));
		assert_eq!(p.description, None);
		assert_eq!(p.public, Some(false));
		assert!(p.images.is_empty());
		assert!(p.items_readable("me"));
		assert!(!p.items_readable("someone-else"));
	}

	#[test]
	fn full_playlist_with_both_items_and_deprecated_tracks() {
		let json = r#"{
			"id": "p1", "name": "Both", "owner": {"id": "me"},
			"items": {"total": 7, "items": [], "limit": 100, "offset": 0, "next": null},
			"tracks": {"total": 7, "items": []}
		}"#;
		let p: Playlist = serde_json::from_str(json).unwrap();
		assert_eq!(p.track_count, Some(7));
		assert_eq!(p.uri, "spotify:playlist:p1");
	}

	#[test]
	fn followed_playlist_without_items_is_unreadable() {
		let json = r#"{
			"id": "37i9dQZF1DXcBWIGoYBM5M", "uri": "spotify:playlist:37i9dQZF1DXcBWIGoYBM5M",
			"name": "Today's Top Hits", "description": "Top hits.", "collaborative": false,
			"images": [{"url": "https://i.scdn.co/image/x", "height": null, "width": null}],
			"owner": {"id": "spotify", "display_name": null}
		}"#;
		let p: Playlist = serde_json::from_str(json).unwrap();
		assert_eq!(p.track_count, None);
		assert_eq!(p.description.as_deref(), Some("Top hits."));
		assert_eq!(p.public, None);
		assert_eq!(p.owner.display_name, None);
		assert!(!p.items_readable("me"));
		let collab = Playlist { collaborative: true, ..p };
		assert!(collab.items_readable("me"));
	}

	#[test]
	fn search_results_paging_and_null_playlists() {
		let json = r#"{
			"tracks": {"limit": 10, "offset": 10, "total": 1200,
				"next": "https://api.spotify.com/v1/search?offset=20&limit=10&query=x&type=track",
				"items": [{"type":"track","uri":"spotify:track:t1","name":"T1","duration_ms":1,"explicit":true,
				           "artists":[{"name":"A","uri":"spotify:artist:a"}],
				           "album":{"name":"Al","uri":"spotify:album:al","images":[]}}]},
			"albums": {"limit": 10, "offset": 10, "total": 3, "next": null,
				"items": [{"album_type":"album","total_tracks":12,"name":"Al","uri":"spotify:album:al",
				           "release_date":"2020","images":[{"url":"u","width":300,"height":300}],
				           "artists":[{"name":"A","uri":"spotify:artist:a"}]}]},
			"playlists": {"limit": 10, "offset": 10, "total": 2, "next": null,
				"items": [null, {"id":"pl","name":"P","owner":{"id":"o","display_name":"O"},"images":[],"items":{"total":3}}]}
		}"#;
		let r: SearchResults = serde_json::from_str::<RawSearch>(json).unwrap().into();
		assert_eq!(r.tracks.total, 1200);
		assert_eq!(r.tracks.next_offset(), Some(20));
		assert!(r.tracks.items[0].explicit);
		assert_eq!(r.albums.items[0].total_tracks, 12);
		assert_eq!(r.albums.items[0].artists[0].name, "A");
		assert_eq!(r.albums.next_offset(), None);
		assert_eq!(r.playlists.items.len(), 1);
		assert_eq!(r.playlists.items[0].track_count, Some(3));
		assert_eq!(r.playlists.total, 2);
	}

	#[test]
	fn playlist_null_descriptions_and_missing_owner_names_are_not_displayed() {
		for description in [serde_json::Value::Null, serde_json::json!("null"), serde_json::json!("")] {
			let mut playlist: Playlist = serde_json::from_value(
				serde_json::json!({ "id": "scratch", "description": description, "owner": { "id": "opaque-account-id" } }),
			)
			.unwrap();
			assert_eq!(playlist.description, None);
			assert_eq!(playlist.owner_name(), "Spotify user");
			playlist.normalize("opaque-account-id", Some("Alex"));
			assert_eq!(playlist.owner_name(), "Alex");
		}
	}

	#[test]
	fn spotify_owned_playlists_use_the_known_display_name() {
		for name in [None, Some("spotify"), Some("Spotify")] {
			let playlist: Playlist =
				serde_json::from_value(serde_json::json!({ "id": "dj", "owner": { "id": "spotify", "display_name": name } })).unwrap();
			assert_eq!(playlist.owner_name(), "Spotify");
		}
	}
}
