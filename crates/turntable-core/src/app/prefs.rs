//! How the settings page's messages change the state. The values live in
//! [`crate::app::settings::Prefs`]; what depends on them (the audio stage, the sources that may be
//! asked, the player's configuration) is carried out by the player loop on
//! [`Effect::ApplyPrefs`], and the playback rules read them as they go ([`crate::app::rules`]).

use crate::app::{
	Effect, Panel, State,
	settings::{Pref, sanitize_name},
};

impl State {
	/// Makes a change from the settings page.
	pub fn set_pref(&mut self, pref: Pref) -> Vec<Effect> {
		match pref {
			Pref::NowPlayingView(on) => {
				self.now_playing_view = on;
				// Turned off, it also goes from where it was opened by hand.
				if !on && self.panel == Some(Panel::NowPlaying) {
					self.panel = None;
				}
			},
			Pref::Lyrics(_) => {
				// Looked up again (or not at all) the next time the panel is open.
				self.prefs.set(&pref);
				self.lyrics = None;
			},
			Pref::WebPlayer(false) => {
				self.prefs.set(&pref);
				// What the page being shown took from the web player goes with it.
				if let Some(listing) = &mut self.library.listing {
					listing.plays.clear();
					listing.overview = None;
				}
			},
			other => {
				self.prefs.set(&other);
			},
		}
		vec![Effect::ApplyPrefs]
	}

	/// Takes the name typed in the settings page for the player. It applies at the next start of the
	/// player; the page offers the restart.
	pub fn save_device_name(&mut self) -> Vec<Effect> {
		let name = sanitize_name(&self.device_input);
		self.device_input.clone_from(&name);
		self.prefs.device.name = name;
		// The keys go back to the player.
		self.focus = None;
		vec![Effect::ApplyPrefs]
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::app::{Field, Msg, settings::DEFAULT_DEVICE_NAME};

	fn only_apply(effects: &[Effect]) -> bool {
		matches!(effects, [Effect::ApplyPrefs])
	}

	#[test]
	fn the_now_playing_view_is_the_states_own_flag() {
		let mut s = State { now_playing_view: true, panel: Some(Panel::NowPlaying), ..State::default() };
		s.update(Msg::Pref(Pref::NowPlayingView(false)));
		assert!(!s.now_playing_view);
		assert_eq!(s.panel, None, "closed where it was opened by hand too");
		s.panel = Some(Panel::Queue);
		s.update(Msg::Pref(Pref::NowPlayingView(true)));
		assert!(s.now_playing_view);
		assert_eq!(s.panel, Some(Panel::Queue), "other panels stay");
	}

	#[test]
	fn lyrics_are_asked_for_again_when_the_source_changes() {
		let mut s = State { lyrics: Some(("spotify:track:a".into(), crate::app::LyricsState::Missing)), ..State::default() };
		s.update(Msg::Pref(Pref::Lyrics(false)));
		assert!(s.lyrics.is_none() && !s.prefs.sources.lyrics);
		s.lyrics = Some(("spotify:track:a".into(), crate::app::LyricsState::Missing));
		s.update(Msg::Pref(Pref::Lyrics(true)));
		assert!(s.lyrics.is_none() && s.prefs.sources.lyrics);
	}

	#[test]
	fn turning_the_web_player_off_clears_what_the_page_took_from_it() {
		let mut s = State::default();
		s.library.listing = Some(crate::library::Listing { plays: vec![1, 2, 3], ..crate::library::Listing::default() });
		s.update(Msg::Pref(Pref::WebPlayer(false)));
		assert!(!s.prefs.sources.web_player);
		assert!(s.library.listing.as_ref().is_some_and(|l| l.plays.is_empty()));
	}

	#[test]
	fn the_device_name_is_cleaned_saved_and_gives_the_keys_back() {
		let mut s = State { panel: Some(Panel::Settings), ..State::default() };
		s.update(Msg::Focus(Some(Field::DeviceName)));
		assert_eq!(s.focus, Some(Field::DeviceName), "the field is on the settings page");
		s.update(Msg::Type("Living room\u{7}".into()));
		assert_eq!(s.device_input, "Living room\u{7}");
		let effects = s.update(Msg::Submit);
		assert!(only_apply(&effects), "{effects:?}");
		assert_eq!(s.prefs.device.name, "Living room");
		assert_eq!(s.device_input, "Living room");
		assert_eq!(s.focus, None);
		// Nothing typed is the default name.
		s.device_input.clear();
		s.update(Msg::SaveDeviceName);
		assert_eq!(s.prefs.device.name, DEFAULT_DEVICE_NAME);
	}

	#[test]
	fn the_name_field_exists_only_on_the_settings_page() {
		let mut s = State::default();
		assert!(!s.field_visible(Field::DeviceName));
		s.panel = Some(Panel::Settings);
		assert!(s.field_visible(Field::DeviceName));
		s.update(Msg::Focus(Some(Field::DeviceName)));
		s.update(Msg::TogglePanel(Panel::Queue));
		assert_eq!(s.focus, None, "leaving the page gives the keys back");
	}

	#[test]
	fn a_dropdown_opens_toggles_and_closes_on_anything_else() {
		use crate::app::system::Menu;
		let mut s = State::default();
		s.update(Msg::Dropdown(Menu::Sleep));
		assert_eq!(s.sys.menu, Some(Menu::Sleep));
		s.update(Msg::Dropdown(Menu::Output));
		assert_eq!(s.sys.menu, Some(Menu::Output), "one at a time");
		s.update(Msg::Dropdown(Menu::Output));
		assert_eq!(s.sys.menu, None, "the button toggles it");
		s.update(Msg::Dropdown(Menu::Sleep));
		s.update(Msg::Sleep(crate::app::rules::SleepChoice::EndOfTrack));
		assert_eq!(s.sys.menu, None, "a pick closes it");
		s.update(Msg::Dropdown(Menu::Sleep));
		s.update(Msg::ToggleMute);
		assert_eq!(s.sys.menu, None, "so does a click elsewhere");
	}

	#[test]
	fn clearing_the_cache_is_asked_once() {
		let mut s = State::default();
		assert!(matches!(s.update(Msg::ClearCache).as_slice(), [Effect::ClearCache]));
		assert!(s.sys.clearing_cache);
		assert!(s.update(Msg::ClearCache).is_empty(), "not again while it is going");
	}
}
