//! Song metadata and lazy lyrics requests, plus clocked lyric view invalidation.

use super::{AppEvent, Controller};
use crate::{app::LyricsState, library, lyrics};

impl Controller {
	/// A changed track gets a fresh leveler, like status, artist and credits.
	pub(super) fn item_changed(&mut self, was_item: &str) {
		let uri = self.state.item.uri.clone();
		if uri != was_item {
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
		if !self.state.library.artists.contains_key(&id) && self.asked_artist.as_ref() != Some(&id) {
			self.asked_artist = Some(id.clone());
			self.library.send(library::Request::Artist(id.clone()));
			self.library.send(library::Request::ArtistCard(id));
		}
	}

	/// Returns whether a lazy lookup changed the visible lyrics state to Loading.
	pub(super) fn request_lyrics(&mut self) -> bool {
		let state = &mut self.state;
		let item = &state.item;
		if !self.visible || !state.lyrics_shown() || !item.uri.starts_with("spotify:track:") || !state.lyrics_due(std::time::Instant::now())
		{
			return false;
		}
		let Some(session) = self.engine.as_ref().and_then(crate::engine::Engine::session_reader) else { return false };
		let song = lyrics::Song { uri: item.uri.clone() };
		if state.lyrics_state().is_none() {
			state.lyrics_failures = 0;
		}
		state.lyrics_retry_at = None;
		state.lyrics = Some((song.uri.clone(), LyricsState::Loading));
		let (tx, wake) = (&self.tx, &self.wake);
		let generation = self.account_generation;
		self.lyrics
			.get_or_insert_with(|| {
				let (tx, wake) = (tx.clone(), wake.clone());
				lyrics::Fetcher::spawn(session, move |response| {
					if tx.send(AppEvent::Lyrics(generation, response)).is_ok() {
						wake();
					}
				})
			})
			.request(song);
		true
	}

	/// The host scrolls to `state().lyric_line()` when this invalidates its view.
	pub(super) fn follow_lyrics(&mut self) -> bool {
		if !self.visible || self.state.synced_lyrics().is_none() {
			return self.followed.take().is_some();
		}
		let line = self.state.lyric_line();
		if self.followed.as_ref().is_some_and(|(uri, known)| *uri == self.state.item.uri && *known == line) {
			return false;
		}
		self.followed = Some((self.state.item.uri.clone(), line));
		true
	}
}
