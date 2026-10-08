//! Application state and the read-only projections used by the views.
//! Playback updates, UI messages and I/O live in the neighboring application modules.

use std::{
	sync::Arc,
	time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::{
	app::queue,
	engine::{CoverSize, Entity, Options, QueueEntry, Status},
	library::{self, Library, Route},
	sources::{external::Lyrics, lyrics},
};

/// The panels sharing the right-hand slot of the full layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
	/// What's playing, its artist and what's next (Spotify's "Now playing view").
	NowPlaying,
	Queue,
	/// The song's words, the line being sung lit.
	Lyrics,
	Eq,
	Settings,
}

/// What the lyrics view knows of the now-playing song's lyrics.
#[derive(Debug)]
pub enum LyricsState {
	Loading,
	Found(Lyrics),
	/// LRCLIB has none.
	Missing,
	/// The lookup failed (offline, LRCLIB busy).
	Failed,
}

/// The text fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
	Search,
	ClientId,
	PlaylistName,
	PlaylistDescription,
	/// The filter above Your Library's rows.
	LibraryFilter,
	/// The player's Spotify Connect name, on the settings page.
	DeviceName,
}

/// A dialog over the client.
#[derive(Debug, Clone, PartialEq)]
pub enum Dialog {
	/// Pick a playlist to add these tracks to (`label` names them).
	AddToPlaylist { uris: Vec<String>, label: String },
	/// Edit the user's playlist, retaining originals to send only changed fields.
	Rename { id: String, name: String, description: String, public: Option<bool> },
}

impl Field {
	/// The node key the view gives this field, for focusing it.
	pub const fn id(self) -> &'static str {
		match self {
			Self::Search => "search",
			Self::ClientId => "client-id",
			Self::PlaylistName => "playlist-name",
			Self::PlaylistDescription => "playlist-description",
			Self::LibraryFilter => "library-filter",
			Self::DeviceName => "device-name",
		}
	}
}

/// Where the playback position was last anchored.
#[derive(Debug, Clone, Copy)]
pub struct Anchor {
	pub position_ms: u64,
	pub at:          Instant,
	pub speed:       f64,
}

impl Anchor {
	pub(super) fn new(position_ms: f64, timestamp_ms: f64, speed: f64) -> Self {
		let now = Instant::now();
		let mut pos = position_ms.max(0.0);
		// Advance by the event's delivery delay. Trust arrival time for a stale
		// timestamp or a wall-clock adjustment of more than a few seconds.
		if speed > 0.0 && timestamp_ms > 0.0 {
			let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64() * 1000.0);
			let lag = now_ms - timestamp_ms;
			if (0.0..5000.0).contains(&lag) {
				pos += lag * speed;
			}
		}
		let position_ms =
			Duration::try_from_secs_f64(pos.max(0.0) / 1000.0).map_or(u64::MAX, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
		Self { position_ms, at: now, speed }
	}

	pub fn now_ms(&self) -> u64 {
		let advanced = Duration::try_from_secs_f64(self.at.elapsed().as_secs_f64() * self.speed.max(0.0))
			.map_or(u64::MAX, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
		self.position_ms.saturating_add(advanced)
	}

	/// When playback reaches `ms` (`None` while it stands still, or for a time too far off to count).
	pub fn instant_at(&self, ms: u64) -> Option<Instant> {
		if self.speed <= 0.0 {
			return None;
		}
		let ahead = Duration::try_from_secs_f64(ms.saturating_sub(self.position_ms) as f64 / 1000.0 / self.speed).ok()?;
		self.at.checked_add(ahead)
	}
}

impl Default for Anchor {
	fn default() -> Self {
		Self { position_ms: 0, at: Instant::now(), speed: 0.0 }
	}
}

/// Encoded artwork shared with a host decoder, independent of any GUI image type.
/// A loaded URL retains the same Arc until its encoded cache entry is evicted;
/// a decoded image cache can key on `Arc::as_ptr` while retaining that Arc.
#[derive(Debug)]
pub struct Cover {
	pub bytes: Vec<u8>,
	pub mime:  String,
}

#[derive(Debug)]
pub(crate) struct CachedCover {
	cover: Arc<Cover>,
	used:  std::cell::Cell<u64>,
}

#[derive(Debug, Default)]
pub struct State {
	pub connected:           bool,
	pub logged_in:           bool,
	pub active:              bool,
	pub device_name:         String,
	pub status:              Status,
	pub item:                Entity,
	pub context:             Entity,
	pub anchor:              Anchor,
	pub volume:              u8,
	pub options:             Options,
	pub upcoming:            Vec<QueueEntry>,
	/// Queue edits not yet in the player's own list (see `queue`).
	pub queue_edit:          queue::Edit,
	/// The now-playing song's lyrics once asked for, with the URI of its track.
	pub lyrics:              Option<(String, LyricsState)>,
	/// The last error worth showing (cleared by the next good event).
	pub notice:              Option<String>,
	/// The player or library could not reach Spotify; cached data remains usable.
	/// This reports Spotify reachability, not OS interface status. A disk snapshot
	/// is not an online answer and does not clear it; player/library recover separately.
	pub offline:             bool,
	/// Persistent connection/recovery text, independent of transient player notices.
	/// Render this even after `offline` clears: it can ask the user to resume stopped
	/// playback. Unrelated volume/queue events do not dismiss it.
	pub network_notice:      Option<String>,
	/// The user's library, delivered by the Session/Web API worker.
	pub library:             Library,
	/// The side panel opened by hand (queue, lyrics, equalizer or settings); when none
	/// is, the now-playing view shows if it's on (see [`State::side_panel`]).
	pub panel:               Option<Panel>,
	/// The now-playing view is on (persisted).
	pub now_playing_view:    bool,
	/// The library sidebar shows covers only (persisted).
	pub sidebar_collapsed:   bool,
	/// What the settings page sets (persisted; see [`crate::app::settings`]).
	pub prefs:               crate::app::settings::Prefs,
	/// The sleep timer, and the pauses and skips that carry out the autoplay and explicit settings.
	pub rules:               crate::app::rules::Rules,
	/// What the settings page reports: devices, cache and app memory.
	pub sys:                 crate::app::system::System,
	/// The device name being typed in the settings page.
	pub device_input:        String,
	/// Your Library's sort and view menu is open (it hangs from the order's button).
	pub library_menu:        bool,
	/// The device menu, the devices Spotify lists, and the other device playing (see [`crate::app::connect`]).
	pub connect:             crate::app::connect::Connect,
	/// Shelves shown in full ("Show all"): the home page's, and Your Library's rows.
	pub open_shelves:        std::collections::HashSet<&'static str>,
	/// The page whose list playback was last started from (Liked Songs, search: lists with
	/// no context URI the player could report), so its play button shows pause while it plays.
	pub played_from:         Option<Route>,
	/// The EQ the audio path uses, and the preset it came from (if untouched).
	pub eq:                  crate::audio::EqSettings,
	pub eq_preset:           Option<String>,
	/// The equalizer's preset menu is open (it hangs from the preset button).
	pub eq_menu:             bool,
	/// Whether the EQ is live: an audio output plays the player's sound.
	pub has_audio:           bool,
	/// The player has a Spotify sign-in (saved): it starts by itself, no setup needed.
	pub signed_in:           bool,
	/// The player is starting (signing in, connecting).
	pub starting:            bool,
	/// Volume to restore when unmuting.
	pub unmute_to:           u8,
	/// Image URL → encoded cover. The host chooses when/how to decode it.
	/// LRU retention is bounded to 16 MiB and 512 URLs; evicted artwork reloads
	/// from the disk cache on demand. Hits preserve the original Arc identity.
	pub(crate) images:       std::collections::HashMap<String, CachedCover>,
	pub(crate) image_clock:  std::cell::Cell<u64>,
	pub(crate) image_bytes:  usize,
	/// Missing URLs recorded by [`State::image`], drained by the controller's pump.
	pub(crate) image_misses: std::cell::RefCell<std::collections::HashSet<String>>,
	/// The text field that has the caret, and the text of each field.
	pub focus:               Option<Field>,
	pub id_input:            String,
	pub name_input:          String,
	pub description_input:   String,
	pub public_input:        Option<bool>,
	pub dialog:              Option<Dialog>,
	/// The discography filter of an artist page: 0 for all, else a kind's index + 1.
	pub discography:         usize,
	/// A confirmation shown as a toast, with a counter so a repeat shows again.
	pub toast:               Option<(u32, String)>,
	pub quit:                bool,
}

impl State {
	/// Whether the user chose to collapse the sidebar; layout is the host's job.
	pub fn sidebar_collapsed(&self) -> bool {
		self.sidebar_collapsed
	}

	pub fn playing(&self) -> bool {
		self.status == Status::Playing
	}

	pub fn duration_ms(&self) -> u64 {
		self.item.duration_ms()
	}

	pub fn position_ms(&self) -> u64 {
		let pos = self.anchor.now_ms();
		match self.duration_ms() {
			0 => pos,
			d => pos.min(d),
		}
	}

	/// A suggested window title: "Song · Artist" ("· Paused" when paused),
	/// or the application name when nothing is loaded.
	pub fn window_title(&self) -> String {
		if self.item.uri.is_empty() {
			return "turntable".to_owned();
		}
		let mut title = self.item.name().to_owned();
		let by = self.item.creators();
		if !by.is_empty() {
			title.push_str(" · ");
			title.push_str(&by);
		}
		if !self.playing() {
			title.push_str(" · Paused");
		}
		title
	}

	/// The now-playing cover URL.
	pub fn cover_url(&self) -> Option<&str> {
		self.item.cover(CoverSize::Default)
	}

	/// Whether the now-playing track is in Liked Songs (`None`: not known yet).
	pub fn liked(&self) -> Option<bool> {
		match &self.library.liked {
			Some((uri, liked)) if *uri == self.item.uri => Some(*liked),
			_ => library::added::TrackId::from_uri(&self.item.uri).ok().and_then(|track| self.library.track_liked(&self.item.uri, &track)),
		}
	}

	/// Whether a Spotify track is in Liked Songs or any playlist owned by the user.
	/// Uses worker-delivered caches only: no I/O or allocation. Unknown tracks return
	/// false until the Session's membership answer arrives and wakes the host.
	pub fn added(&self, track_uri: &str) -> bool {
		let Ok(track) = library::added::TrackId::from_uri(track_uri) else { return false };
		self.library.owned_tracks.binary_search(&track).is_ok() || self.library.track_liked(track_uri, &track).unwrap_or(false)
	}

	/// The requested side panel, or the enabled now-playing view when a track is loaded.
	pub fn side_panel(&self) -> Option<Panel> {
		if self.panel.is_some() {
			return self.panel;
		}
		(self.now_playing_view && !self.item.uri.is_empty()).then_some(Panel::NowPlaying)
	}

	/// Whether the lyrics are on screen.
	pub fn lyrics_shown(&self) -> bool {
		self.panel == Some(Panel::Lyrics)
	}

	/// Whether the equalizer's preset menu is on screen: it hangs from the equalizer panel's button.
	pub fn eq_menu_open(&self) -> bool {
		self.eq_menu && self.side_panel() == Some(Panel::Eq)
	}

	/// What's known of the now-playing track's lyrics (`None`: not asked for yet).
	pub fn lyrics_state(&self) -> Option<&LyricsState> {
		self.lyrics.as_ref().filter(|(uri, _)| *uri == self.item.uri).map(|(_, state)| state)
	}

	/// Takes in the answer of a lookup, unless the track has changed since.
	pub fn apply_lyrics(&mut self, response: lyrics::Response) {
		if response.uri == self.item.uri {
			let state = match response.lyrics {
				Ok(Some(lyrics)) => LyricsState::Found(lyrics),
				Ok(None) => LyricsState::Missing,
				Err(_) => LyricsState::Failed,
			};
			self.lyrics = Some((response.uri, state));
		}
	}

	/// The synced lyrics on screen.
	pub fn synced_lyrics(&self) -> Option<&[(u64, String)]> {
		match self.lyrics_state() {
			Some(LyricsState::Found(Lyrics::Synced(lines))) if self.lyrics_shown() => Some(lines),
			_ => None,
		}
	}

	/// The line being sung: the last one that has started (`None` before the first).
	pub fn lyric_line(&self) -> Option<usize> {
		let lines = self.synced_lyrics()?;
		let position = self.position_ms();
		lines.partition_point(|(ms, _)| *ms <= position).checked_sub(1)
	}

	/// When the next line starts, while the music plays.
	pub fn next_lyric_at(&self) -> Option<Instant> {
		let lines = self.synced_lyrics()?;
		let position = self.position_ms();
		let (ms, _) = lines.get(lines.partition_point(|(ms, _)| *ms <= position))?;
		// Playback stops at the end of the track: a line past it would never start, and its
		// time would keep passing without the position moving.
		if self.duration_ms() > 0 && *ms > self.duration_ms() {
			return None;
		}
		self.anchor.instant_at(*ms)
	}

	pub fn show_toast(&mut self, text: String) {
		let n = self.toast.as_ref().map_or(0, |(n, _)| n + 1);
		self.toast = Some((n, text));
	}

	/// Whether the player is on its way up (the setup page shows progress instead of the button).
	/// Offline retries run in the background without showing setup progress again.
	pub fn starting_player(&self) -> bool {
		self.starting && !self.offline
	}

	/// Whether the setup steps still have something for the user to do.
	pub fn needs_setup(&self) -> bool {
		!self.signed_in || !matches!(self.library.conn, library::Conn::Ready)
	}

	/// Whether the current state offers Your Library's filter and ordering controls.
	pub fn library_tools(&self) -> bool {
		self.library.conn == library::Conn::Ready && (!self.sidebar_collapsed || self.library.route == Route::Library)
	}

	/// Whether `field` is on screen (mirrors the view's conditions).
	pub fn field_visible(&self, field: Field) -> bool {
		let lib = &self.library;
		let ready = matches!(lib.conn, library::Conn::Ready);
		if matches!(field, Field::PlaylistName | Field::PlaylistDescription) {
			return matches!(self.dialog, Some(Dialog::Rename { .. }));
		}
		if field == Field::DeviceName {
			return self.panel == Some(Panel::Settings) && self.dialog.is_none();
		}
		if self.dialog.is_some() {
			return false;
		}
		let setup = match lib.route {
			Route::Home => self.needs_setup(),
			Route::Search => !ready,
			_ => false,
		};
		match field {
			Field::Search => lib.route == Route::Search && ready,
			Field::ClientId => setup && matches!(lib.conn, library::Conn::NeedsClientId),
			Field::PlaylistName | Field::PlaylistDescription | Field::DeviceName => false,
			Field::LibraryFilter => self.library_tools(),
		}
	}

	/// Retained encoded artwork entries and bytes (excludes decoded/GPU artwork).
	pub fn image_cache_usage(&self) -> (usize, usize) {
		(self.images.len(), self.image_bytes)
	}

	pub(crate) fn cache_image(&mut self, url: String, cover: Cover) {
		if let Some(cached) = self.images.get(&url) {
			cached.used.set(self.image_use());
			return;
		}
		self.image_bytes += cover.bytes.len();
		let cached = CachedCover { cover: Arc::new(cover), used: std::cell::Cell::new(self.image_use()) };
		self.images.insert(url, cached);
		while self.image_bytes > 16 * 1024 * 1024 || self.images.len() > 512 {
			let Some(oldest) = self.images.values().map(|entry| entry.used.get()).min() else { break };
			let Some((_, cached)) = self.images.extract_if(|_, entry| entry.used.get() == oldest).next() else { break };
			self.image_bytes -= cached.cover.bytes.len();
		}
	}

	fn image_use(&self) -> u64 {
		let used = self.image_clock.get().saturating_add(1);
		self.image_clock.set(used);
		used
	}

	/// The encoded cover for a URL, or `None` while it is loading. A missing URL
	/// is recorded so the next [`crate::Controller::pump`] fetches it off-thread.
	pub fn image(&self, url: Option<&str>) -> Option<&Arc<Cover>> {
		let url = url.filter(|u| !u.is_empty())?;
		match self.images.get(url) {
			Some(cached) => {
				cached.used.set(self.image_use());
				Some(&cached.cover)
			},
			None => {
				self.image_misses.borrow_mut().insert(url.to_owned());
				None
			},
		}
	}

	/// Missing URLs the host requested while rendering.
	pub(crate) fn take_missing_images(&self) -> std::collections::HashSet<String> {
		std::mem::take(&mut *self.image_misses.borrow_mut())
	}
}

/// `m:ss` (or `h:mm:ss`).
pub fn clock(ms: u64) -> String {
	let s = Duration::from_millis(ms).as_secs();
	if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// Steps of an EQ band slider: 0 (−12 dB) … `EQ_STEPS` (+12 dB), 2 dB apart.
pub const EQ_STEPS: u8 = 12;

/// The gain of slider step `step`.
pub fn eq_step_db(step: u8) -> f32 {
	f32::from(step.min(EQ_STEPS)) * 2.0 - 12.0
}

/// The slider step nearest to `db`.
#[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "clamped to ±12 dB, so the rounded step is 0..=EQ_STEPS")]
pub fn eq_db_step(db: f32) -> u8 {
	db.clamp(-12.0, 12.0).midpoint(12.0).round() as u8
}
