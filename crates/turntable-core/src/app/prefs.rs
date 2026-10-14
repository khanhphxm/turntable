//! How the settings page's messages change the state. The values live in
//! [`crate::app::settings::Prefs`]; what depends on them (the audio stage and the player's
//! configuration) is carried out by the player loop on
//! [`Effect::ApplyPrefs`], and the playback rules read them as they go ([`crate::app::rules`]).

use crate::app::{Effect, Panel, State, settings::Pref};

impl State {
	/// Makes a change from the settings page.
	pub fn set_pref(&mut self, pref: &Pref) -> Vec<Effect> {
		match pref {
			Pref::Proxy(value) => {
				if crate::net::proxy::parse(value).is_err() || value.trim() == self.prefs.network.proxy {
					return vec![];
				}
				self.prefs.set(pref);
				return vec![Effect::ReconfigureNetwork];
			},
			Pref::NowPlayingView(on) => {
				self.now_playing_view = *on;
				// Turned off, it also goes from where it was opened by hand.
				if !*on && self.panel == Some(Panel::NowPlaying) {
					self.panel = None;
				}
			},
			other => {
				self.prefs.set(other);
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
	fn streaming_quality_applies_without_reconnecting() {
		use crate::app::settings::StreamingQuality;
		let mut state = State::default();
		for quality in StreamingQuality::ALL {
			assert!(matches!(state.set_pref(&Pref::StreamingQuality(quality)).as_slice(), [Effect::ApplyPrefs]));
			assert_eq!(state.prefs.audio.streaming_quality, quality);
		}
	}

	#[test]
	fn proxy_changes_validate_and_restart_only_when_changed() {
		let mut state = State::default();
		assert!(state.set_pref(&Pref::Proxy("socks5://host:1080".into())).is_empty());
		assert_eq!(state.prefs.network.proxy, "");
		assert!(matches!(state.set_pref(&Pref::Proxy(" http://user:pass@host:8080 ".into())).as_slice(), [Effect::ReconfigureNetwork]));
		assert_eq!(state.prefs.network.proxy, "http://user:pass@host:8080");
		assert!(state.set_pref(&Pref::Proxy("http://user:pass@host:8080".into())).is_empty());
		assert!(state.set_pref(&Pref::Proxy("not a proxy".into())).is_empty());
		assert_eq!(state.prefs.network.proxy, "http://user:pass@host:8080");
		assert!(matches!(state.set_pref(&Pref::Proxy(String::new())).as_slice(), [Effect::ReconfigureNetwork]));
		assert_eq!(state.prefs.network.proxy, "");
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
}
