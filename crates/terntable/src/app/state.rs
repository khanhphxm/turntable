//! Application state and the read-only projections used by the views.
//! Playback updates, UI messages and I/O live in the neighboring application modules.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::{
	app::queue,
	engine::{CoverSize, Entity, Options, QueueEntry, Status},
	library::{self, Library, Route},
	sources::{external::Lyrics, lyrics},
};

/// Layout breakpoints in design px (CSS px at the 13 px design size).
/// Below `COMPACT_PX` wide the client is laid out like Spotify's phone app;
/// below `MEDIUM_PX` the sidebar shows covers only; below `WIDE_PX` the
/// now-playing view only opens when asked for; below `STRIP_PX` tall only
/// the player shows.
const COMPACT_PX: f64 = 720.0;
const MEDIUM_PX: f64 = 1000.0;
const WIDE_PX: f64 = 1100.0;
const STRIP_PX: f64 = 220.0;

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

	/// The field's derived node id in the view (the keyed path from the region
	/// root), which Tern's focus op needs.
	pub const fn node_id(self) -> &'static str {
		match self {
			Self::Search => "main.app.content.searchbar.search",
			Self::ClientId => "main.app.content.setup.step2.body.field.client-id",
			Self::PlaylistName => "layer.dialog.edit.fields.field.playlist-name",
			Self::PlaylistDescription => "layer.dialog.edit.fields.description.playlist-description",
			Self::LibraryFilter => "main.app.side.library.tools.bar.filter.library-filter",
			Self::DeviceName => "main.app.settings.page.device.card.name.control.field.device-name",
		}
	}
}

/// How the client arranges itself for the pane's size. One client, never a
/// different mode: the same views, rearranged as the pane is resized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
	/// Sidebar, content and the right-hand panel; the full now-playing bar.
	Wide,
	/// The same with the sidebar showing covers only.
	Medium,
	/// Phone-like: one column, a mini player and a tab bar at the bottom.
	Compact,
	/// Too short for anything but the player (a picture in picture).
	Strip,
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

#[derive(Debug, Default)]
pub struct State {
	pub connected:         bool,
	pub logged_in:         bool,
	pub active:            bool,
	pub device_name:       String,
	pub status:            Status,
	pub item:              Entity,
	pub context:           Entity,
	pub anchor:            Anchor,
	pub volume:            u8,
	pub options:           Options,
	pub upcoming:          Vec<QueueEntry>,
	/// Queue edits not yet in the player's own list (see `queue`).
	pub queue_edit:        queue::Edit,
	/// The now-playing song's lyrics once asked for, with the URI of its track.
	pub lyrics:            Option<(String, LyricsState)>,
	/// The last error worth showing (cleared by the next good event).
	pub notice:            Option<String>,
	pub rows:              u16,
	pub cols:              u32,
	/// Cell size in px (from Tern's hello and resize events); 0 when unknown.
	pub cell_w:            f64,
	pub cell_h:            f64,
	/// The user's library (Web API side).
	pub library:           Library,
	/// The side panel opened by hand (queue, lyrics, equalizer or settings); when none
	/// is, the now-playing view shows if it's on (see [`State::side_panel`]).
	pub panel:             Option<Panel>,
	/// The now-playing view is on (persisted).
	pub now_playing_view:  bool,
	/// The library sidebar shows covers only (persisted).
	pub sidebar_collapsed: bool,
	/// What the settings page sets (persisted; see [`crate::app::settings`]).
	pub prefs:             crate::app::settings::Prefs,
	/// The sleep timer, and the pauses and skips that carry out the autoplay and explicit settings.
	pub rules:             crate::app::rules::Rules,
	/// What the settings page reports: devices, cache and app memory.
	pub sys:               crate::app::system::System,
	/// The device name being typed in the settings page.
	pub device_input:      String,
	/// Your Library's sort and view menu is open (it hangs from the order's button).
	pub library_menu:      bool,
	/// The device menu, the devices Spotify lists, and the other device playing (see [`crate::app::connect`]).
	pub connect:           crate::app::connect::Connect,
	/// Shelves shown in full ("Show all"): the home page's, and Your Library's rows.
	pub open_shelves:      std::collections::HashSet<&'static str>,
	/// The page whose list playback was last started from (Liked Songs, search: lists with
	/// no context URI the player could report), so its play button shows pause while it plays.
	pub played_from:       Option<Route>,
	/// The EQ the audio path uses, and the preset it came from (if untouched).
	pub eq:                crate::audio::EqSettings,
	pub eq_preset:         Option<String>,
	/// The equalizer's preset menu is open (it hangs from the preset button).
	pub eq_menu:           bool,
	/// The gains the equalizer's response-curve picture was drawn for, and its blob id in Tern.
	pub eq_curve:          Option<([f32; 10], String)>,
	/// Whether the EQ is live: an audio output plays the player's sound.
	pub has_audio:         bool,
	/// The player has a Spotify sign-in (saved): it starts by itself, no setup needed.
	pub signed_in:         bool,
	/// The player is starting (signing in, connecting).
	pub starting:          bool,
	/// Volume to restore when unmuting.
	pub unmute_to:         u8,
	/// Image URL → blob id, for every picture already sent to Tern.
	pub images:            std::collections::HashMap<String, String>,
	/// Image URLs the view looked up during the last render and found missing
	/// (filled by [`State::image`], drained by the player after each render).
	pub image_misses:      std::cell::RefCell<std::collections::HashSet<String>>,
	/// The text field that has the caret, and the text of each field.
	pub focus:             Option<Field>,
	pub id_input:          String,
	pub name_input:        String,
	pub description_input: String,
	pub public_input:      Option<bool>,
	pub dialog:            Option<Dialog>,
	/// The discography filter of an artist page: 0 for all, else a kind's index + 1.
	pub discography:       usize,
	/// A confirmation shown as a toast, with a counter so a repeat shows again.
	pub toast:             Option<(u32, String)>,
	/// Bumped whenever the seek bar's animation must restart.
	pub seek_gen:          u64,
	pub quit:              bool,
}

impl State {
	/// The pane's size in px (0 when unknown).
	pub fn pane_px(&self) -> (f64, f64) {
		(f64::from(self.cols) * self.cell_w, f64::from(self.rows) * self.cell_h)
	}

	/// The pane's size in design px (cells are ~9 px wide at the 14 px default
	/// font); zero when Tern hasn't said.
	fn design_px(&self) -> (f64, f64) {
		let cell_w = if self.cell_w > 0.0 { self.cell_w } else { 9.0 };
		let cell_h = if self.cell_h > 0.0 { self.cell_h } else { 18.0 };
		(f64::from(self.cols) * cell_w / 1.08, f64::from(self.rows) * cell_h / 1.08)
	}

	pub fn layout(&self) -> Layout {
		let (w, h) = self.design_px();
		if h > 0.0 && h < STRIP_PX {
			Layout::Strip
		} else if w > 0.0 && w < COMPACT_PX {
			Layout::Compact
		} else if w > 0.0 && w < MEDIUM_PX {
			Layout::Medium
		} else {
			Layout::Wide
		}
	}

	/// Whether the sidebar shows covers only: by choice, or because the pane is narrow.
	pub fn sidebar_collapsed(&self) -> bool {
		self.sidebar_collapsed || self.layout() == Layout::Medium
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

	/// The pane title: "Song · Artist" while something is loaded ("· Paused" when
	/// paused), else "terntable". Tern shows it on the tab.
	pub fn pane_title(&self) -> String {
		if self.item.uri.is_empty() {
			return "terntable".to_owned();
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
			_ => None,
		}
	}

	/// The panel in the right-hand slot: one opened by hand, else the
	/// now-playing view when it's on, something is loaded and the pane is wide
	/// enough to keep the content readable beside it.
	pub fn side_panel(&self) -> Option<Panel> {
		if self.panel.is_some() {
			return self.panel;
		}
		let (w, _) = self.design_px();
		let wide = self.layout() == Layout::Wide && (w == 0.0 || w >= WIDE_PX);
		(self.now_playing_view && wide && !self.item.uri.is_empty()).then_some(Panel::NowPlaying)
	}

	/// Whether the lyrics are on screen.
	pub fn lyrics_shown(&self) -> bool {
		self.panel == Some(Panel::Lyrics) && self.layout() != Layout::Strip
	}

	/// Whether the equalizer's preset menu is on screen: it hangs from the equalizer panel's button.
	pub fn eq_menu_open(&self) -> bool {
		self.eq_menu && self.side_panel() == Some(Panel::Eq) && self.layout() != Layout::Strip
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
	pub fn starting_player(&self) -> bool {
		self.starting
	}

	/// Whether the setup steps still have something for the user to do.
	pub fn needs_setup(&self) -> bool {
		!self.signed_in || !matches!(self.library.conn, library::Conn::Ready)
	}

	/// Whether Your Library's chips, filter and order are on screen: in the sidebar unless it shows
	/// covers only, or on the compact layout's library page.
	pub fn library_tools(&self) -> bool {
		let lib = &self.library;
		lib.conn == library::Conn::Ready
			&& match self.layout() {
				Layout::Strip => false,
				Layout::Compact => lib.route == Route::Library && self.panel.is_none(),
				Layout::Wide | Layout::Medium => !self.sidebar_collapsed(),
			}
	}

	/// Whether `field` is on screen (mirrors the view's conditions).
	pub fn field_visible(&self, field: Field) -> bool {
		let lib = &self.library;
		let ready = matches!(lib.conn, library::Conn::Ready);
		if matches!(field, Field::PlaylistName | Field::PlaylistDescription) {
			return matches!(self.dialog, Some(Dialog::Rename { .. }));
		}
		if field == Field::DeviceName {
			// The settings page is a page of its own, in every layout.
			return self.panel == Some(Panel::Settings) && self.dialog.is_none() && self.layout() != Layout::Strip;
		}
		if self.dialog.is_some() || self.layout() == Layout::Strip || self.panel.is_some() && self.layout() == Layout::Compact {
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

	/// The blob id for an image URL, once it has been sent. A URL the view asks
	/// for that isn't loaded yet is recorded, so exactly what's on screen gets
	/// fetched (see [`State::take_missing_images`]).
	pub fn image(&self, url: Option<&str>) -> Option<&str> {
		let url = url.filter(|u| !u.is_empty())?;
		match self.images.get(url) {
			Some(id) => Some(id.as_str()),
			None => {
				self.image_misses.borrow_mut().insert(url.to_owned());
				None
			},
		}
	}

	/// The image URLs the last render showed but Tern doesn't have yet.
	pub fn take_missing_images(&self) -> std::collections::HashSet<String> {
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
