//! Rendering, image and song metadata requests, and clocked lyric presentation.

use tern_sdk::wire::RevealAt;

use super::{AppEvent, Player, tsp};
use crate::{
	app::{Field, Layout, LyricsState, State},
	library,
	sources::lyrics,
	ui,
};

impl Player<'_> {
	pub(super) fn render(&mut self) -> Result<(), String> {
		// The equalizer's response curve is a picture of its gains: Tern needs it before the view that shows it.
		ui::send_eq_curve(self.session, &mut self.state).map_err(tsp)?;
		self.session.render(self.sf, ui::view(&self.state, &self.icons)).map_err(tsp)?;
		// Fetch exactly the images that render showed and Tern doesn't have yet,
		// each URL once (or again after a pause when it failed).
		for url in self.state.take_missing_images() {
			self.images.request(url);
		}
		if crate::net::trace::enabled() {
			crate::net::trace::mark_if_changed("ready", &self.state.library.ready_label());
			crate::net::trace::mark_if_changed("covers", &self.images.outstanding().to_string());
		}
		let key = Some(SheetKey::of(&self.state));
		if key != self.sheet_key {
			// A new position (or track) must restart the seek bar's animation: flip its name each time,
			// however many position changes came between two renders.
			if self.sheet_key.as_ref().is_none_or(|k| k.seek_gen != self.state.seek_gen) {
				self.seek_flip = !self.seek_flip;
			}
			self.sheet_key = key;
			self.session.stylesheet(self.sf, "tt-dynamic", Some(&ui::dynamic_sheet(&self.state, self.seek_flip))).map_err(tsp)?;
		}
		let lyrics_key = LyricsKey::of(&self.state);
		if lyrics_key != self.lyrics_key {
			self.lyrics_key = lyrics_key;
			// Other keyframe names make Tern start the animations over.
			self.lyrics_flip = !self.lyrics_flip;
			self.session.stylesheet(self.sf, "tt-lyrics", ui::lyrics_sheet(&self.state, self.lyrics_flip).as_deref()).map_err(tsp)?;
		}
		// Anything can hide a field (a library event, a resize): drop its caret.
		if self.state.focus.is_some_and(|f| !self.state.field_visible(f)) {
			self.state.focus = None;
		}
		if self.focused != self.state.focus {
			self.focused = self.state.focus;
			// The field may not be on screen (another route); Tern ignores that.
			let _ = self.session.focus(self.sf, self.state.focus.map(Field::node_id));
		}
		let title = self.state.pane_title();
		if title != self.title {
			// OSC 2: the pane (and tab) title. The Tern plugin's status segment reads it.
			let clean: String = title.chars().filter(|c| !c.is_control()).collect();
			self.session.write_raw(format!("\x1b]2;{clean}\x07").as_bytes()).map_err(tsp)?;
			self.title = title;
		}
		Ok(())
	}
	/// After the now-playing item may have changed (from either source): a new track is checked
	/// for being liked (one call per track), and its artist and credits are asked for.
	pub(super) fn item_changed(&mut self, was_item: &str) {
		let uri = self.state.item.uri.clone();
		if uri != was_item {
			// A new track: the output stage's leveler settles on it instead of averaging in the last.
			if let Some(audio) = &self.audio {
				audio.new_track();
			}
			if uri.starts_with("spotify:track:") && self.state.library.conn == library::Conn::Ready {
				self.library.send(library::Request::CheckLiked(uri.clone()));
			}
			self.request_artist();
		}
		self.last_item = uri;
	}
	/// Asks for the now-playing track's first artist, its overview and the track's credits
	/// (for the now-playing view), unless they're known or were asked for already.
	pub(super) fn request_artist(&mut self) {
		if self.state.library.conn != library::Conn::Ready {
			return;
		}
		let track = &self.state.item.uri;
		if track.starts_with("spotify:track:") && self.state.library.credits.as_ref().is_none_or(|(uri, _)| uri != track) {
			self.library.send(library::Request::Credits(track.clone()));
		}
		let Some(creator) = self.state.item.decorations.creators.first() else { return };
		let uri = &creator.entity.uri;
		if !uri.starts_with("spotify:artist:") {
			return;
		}
		let id = crate::spotify::id_of(uri).to_owned();
		if !self.state.library.artists.contains_key(&id) && self.asked_artists.insert(id.clone()) {
			self.library.send(library::Request::Artist(id.clone()));
			self.library.send(library::Request::ArtistCard(id));
		}
	}

	/// Asks for the now-playing track's lyrics once they're on screen, unless they were asked for.
	pub(super) fn request_lyrics(&mut self) {
		let state = &mut self.state;
		let item = &state.item;
		if !state.lyrics_shown() || !state.prefs.sources.lyrics || !item.uri.starts_with("spotify:track:") || state.lyrics_state().is_some()
		{
			return;
		}
		let song = lyrics::Song {
			uri:         item.uri.clone(),
			title:       item.name().to_owned(),
			artist:      item.decorations.creators.first().map_or_else(String::new, |c| c.entity.name().to_owned()),
			duration_ms: item.duration_ms(),
		};
		state.lyrics = Some((song.uri.clone(), LyricsState::Loading));
		let (tx, waker) = (&self.tx, &self.waker);
		self.lyrics
			.get_or_insert_with(|| {
				let (tx, waker) = (tx.clone(), waker.clone());
				lyrics::Fetcher::spawn(move |r| {
					if tx.send(AppEvent::Lyrics(r)).is_ok() {
						waker.wake();
					}
				})
			})
			.request(song);
	}

	/// Keeps the lit lyric line in view: scrolls to it when it changes (the loop wakes for each).
	pub(super) fn follow_lyrics(&mut self) -> Result<(), String> {
		if self.state.synced_lyrics().is_none() {
			self.followed = None;
			return Ok(());
		}
		let line = self.state.lyric_line().unwrap_or(0);
		let pane = (self.state.cols, self.state.rows);
		if self.followed.as_ref().is_some_and(|f| f.uri == self.state.item.uri && f.line == line && f.pane == pane) {
			return Ok(());
		}
		self.followed = Some(Followed { uri: self.state.item.uri.clone(), line, pane });
		self.session.reveal(self.sf, &ui::line_id(line), RevealAt::Start).map_err(tsp)
	}
}
#[derive(PartialEq)]
pub(super) struct SheetKey {
	seek_gen:  u64,
	playing:   bool,
	duration:  u64,
	volume:    u8,
	queue:     bool,
	/// Pane size in whole px.
	pane:      (f64, f64),
	/// Pane size in cells.
	cells:     (u32, u16),
	layout:    Layout,
	collapsed: bool,
	/// The page tint (`#rrggbb`).
	tint:      Option<String>,
}
impl SheetKey {
	fn of(s: &State) -> Self {
		let (w, h) = s.pane_px();
		Self {
			seek_gen:  s.seek_gen,
			playing:   s.playing(),
			duration:  s.duration_ms(),
			volume:    s.volume,
			queue:     s.side_panel().is_some(),
			pane:      (w.trunc(), h.trunc()),
			cells:     (s.cols, s.rows),
			layout:    s.layout(),
			collapsed: s.sidebar_collapsed(),
			tint:      ui::tint(s).map(str::to_owned),
		}
	}
}

/// What the lyrics sheet is built from: the lines and where playback is in them.
#[derive(PartialEq)]
pub(super) struct LyricsKey {
	uri:      String,
	seek_gen: u64,
	playing:  bool,
}

impl LyricsKey {
	/// `None` while no synced lyrics are on screen.
	fn of(s: &State) -> Option<Self> {
		s.synced_lyrics().map(|_| Self { uri: s.item.uri.clone(), seek_gen: s.seek_gen, playing: s.playing() })
	}
}

/// The lyric line last scrolled to, in a pane of that size.
pub(super) struct Followed {
	uri:  String,
	line: usize,
	pane: (u32, u16),
}
