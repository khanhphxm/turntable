//! How the settings page's messages change the state. The values live in
//! [`crate::app::settings::Prefs`]; what depends on them (the audio stage and the player's
//! configuration) is carried out by the player loop on
//! [`Effect::ApplyPrefs`], and the playback rules read them as they go ([`crate::app::rules`]).

use crate::app::{Effect, Panel, State, settings::Pref};

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
			other => {
				self.prefs.set(&other);
			},
		}
		vec![Effect::ApplyPrefs]
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::app::Msg;

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
}
