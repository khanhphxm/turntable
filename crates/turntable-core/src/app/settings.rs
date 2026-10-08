//! User preferences that survive restarts: `<config>/settings.json`.
//! Unknown or missing fields fall back to defaults, so older files keep working.

use std::{fs, io, path::Path};

use serde::{Deserialize, Serialize};

use crate::audio::{EqSettings, Normalize};

/// The Spotify Connect name of the player until the user picks one.
pub const DEFAULT_DEVICE_NAME: &str = "turntable";
/// Longest device name kept (Spotify shows it in a menu).
pub const MAX_DEVICE_NAME: usize = 48;
/// The playback cache limits offered, in MB (0 disables the cache).
pub const CACHE_SIZES: [u32; 6] = [100, 250, 500, 1024, 2048, 0];
/// The volume the player starts at until the user picks one.
pub const DEFAULT_VOLUME: u8 = 50;
/// Longest overlap between naturally advancing songs, in seconds (0 disables it).
pub const MAX_CROSSFADE: u8 = 12;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
	pub eq:                EqSettings,
	/// The preset the EQ was last set from; `None` once a band is moved by hand.
	pub eq_preset:         Option<String>,
	/// The now-playing view in the right-hand slot (on unless turned off).
	pub now_playing_view:  bool,
	/// The library sidebar shows covers only.
	pub sidebar_collapsed: bool,
	/// What the settings page sets.
	#[serde(flatten)]
	pub prefs:             Prefs,
}

impl Default for Settings {
	fn default() -> Self {
		Self {
			eq:                EqSettings::default(),
			eq_preset:         None,
			now_playing_view:  true,
			sidebar_collapsed: false,
			prefs:             Prefs::default(),
		}
	}
}

/// Everything the settings page sets, in the page's groups.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
	pub playback: Playback,
	pub audio:    Audio,
	pub device:   Device,
	pub display:  Display,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Playback {
	/// Play recommended songs when what was chosen runs out; off pauses there instead.
	pub autoplay:  bool,
	/// Songs marked explicit play; off skips them.
	pub explicit:  bool,
	/// Equal-power overlap in seconds; ordered adjacent album songs remain gapless.
	pub crossfade: u8,
}

impl Default for Playback {
	fn default() -> Self {
		Self { autoplay: true, explicit: true, crossfade: 0 }
	}
}

/// The audio output preferences (macOS).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Audio {
	pub normalize: Normalize,
	pub mono:      bool,
	/// The output device the user picked; `None` plays on the system's default.
	pub output:    Option<Output>,
}

/// An output device by the identifier CoreAudio keeps across reboots, with the name it had.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Output {
	pub uid:  String,
	pub name: String,
}

/// The player itself: how it shows up in Spotify and how it starts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Device {
	/// The Spotify Connect name.
	pub name:     String,
	/// The volume the player starts at (0 to 100).
	pub volume:   u8,
	/// The player's audio cache limit in MB; 0 disables caching.
	pub cache_mb: u32,
}

impl Default for Device {
	fn default() -> Self {
		Self { name: DEFAULT_DEVICE_NAME.to_owned(), volume: DEFAULT_VOLUME, cache_mb: CACHE_SIZES[0] }
	}
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Display {
	/// Tighter rows.
	pub compact:     bool,
	/// Play counts beside songs where Spotify's web player shares them.
	pub play_counts: bool,
}

impl Default for Display {
	fn default() -> Self {
		Self { compact: false, play_counts: true }
	}
}

impl Prefs {
	/// Brings hand-edited or newer settings files into the app's supported ranges.
	pub fn sanitized(mut self) -> Self {
		self.playback.crossfade = self.playback.crossfade.min(MAX_CROSSFADE);
		self.device.volume = self.device.volume.min(100);
		self.device.cache_mb = sanitize_cache(self.device.cache_mb);
		self.device.name = sanitize_name(&self.device.name);
		self
	}
}

/// The supported cache range: off, or 100 MB to 2 GB.
fn sanitize_cache(mb: u32) -> u32 {
	if mb == 0 { 0 } else { mb.clamp(CACHE_SIZES[0], 2048) }
}

/// A Connect device name: no control characters, no stray blanks, not empty, not long.
pub fn sanitize_name(name: &str) -> String {
	let clean: String = name.chars().filter(|c| !c.is_control()).collect();
	let clean: String = clean.trim().chars().take(MAX_DEVICE_NAME).collect();
	let clean = clean.trim_end();
	if clean.is_empty() { DEFAULT_DEVICE_NAME.to_owned() } else { clean.to_owned() }
}

/// One change the settings page makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pref {
	Autoplay(bool),
	Explicit(bool),
	/// Seconds of overlap on automatic transitions, clamped to `0..=MAX_CROSSFADE`.
	Crossfade(u8),
	Normalize(Normalize),
	Mono(bool),
	/// A device to play on (`None`: the system's default).
	Output(Option<Output>),
	StartupVolume(u8),
	/// Whether the now-playing view opens by itself (kept apart from [`Prefs`], with the layout).
	NowPlayingView(bool),
	Compact(bool),
	PlayCounts(bool),
}

impl Prefs {
	/// Makes a change. Returns false for the one that isn't kept here ([`Pref::NowPlayingView`]).
	pub fn set(&mut self, pref: &Pref) -> bool {
		match pref {
			Pref::Autoplay(on) => self.playback.autoplay = *on,
			Pref::Explicit(on) => self.playback.explicit = *on,
			Pref::Crossfade(seconds) => self.playback.crossfade = (*seconds).min(MAX_CROSSFADE),
			Pref::Normalize(level) => self.audio.normalize = *level,
			Pref::Mono(on) => self.audio.mono = *on,
			Pref::Output(device) => self.audio.output.clone_from(device),
			Pref::StartupVolume(volume) => self.device.volume = (*volume).min(100),
			Pref::Compact(on) => self.display.compact = *on,
			Pref::PlayCounts(on) => self.display.play_counts = *on,
			Pref::NowPlayingView(_) => return false,
		}
		true
	}
}

impl Settings {
	pub fn load(config: &Path) -> Self {
		let mut settings: Self =
			fs::read(config.join("settings.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
		settings.eq = settings.eq.sanitized();
		settings.prefs = settings.prefs.sanitized();
		settings
	}

	pub fn save(&self, config: &Path) -> io::Result<()> {
		let json = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
		// 0600 and fsynced before the rename, like every file in the config dir.
		crate::app::config::write_private(&config.join("settings.json"), &json)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn eq_integer_db_settings_migrate_and_half_db_values_survive_restart() {
		let directory = tempfile::tempdir().unwrap();
		let dir = directory.path();
		fs::write(
			dir.join("settings.json"),
			br#"{"eq":{"enabled":true,"preamp_db":-3,"gains_db":[4,3,2,0,-1,-1,0,2,3,4]},"eq_preset":"rock"}"#,
		)
		.unwrap();
		let mut settings = Settings::load(dir);
		let rock = EqSettings::preset("rock").unwrap();
		assert_eq!(settings.eq.gains_db, rock.gains_db, "integers are dB, not slider or half-dB units");
		assert!(settings.eq.enabled);
		assert_eq!(settings.eq.preamp_db, -3.0);
		assert_eq!(settings.eq_preset.as_deref(), Some("rock"));
		settings.eq.gains_db = [-11.5, -3.5, -0.5, 0.0, 0.5, 1.5, 3.5, 5.5, 9.5, 11.5];
		settings.eq_preset = None;
		settings.save(dir).unwrap();
		assert_eq!(Settings::load(dir), settings);
		fs::write(dir.join("settings.json"), br#"{"eq":{"preamp_db":-99,"gains_db":[99,-99,0.25,-0.25,3.74,-3.74,11.9,-11.9,1,-1]}}"#)
			.unwrap();
		let settings = Settings::load(dir);
		assert_eq!(settings.eq.gains_db, [12.0, -12.0, 0.5, -0.5, 3.5, -3.5, 12.0, -12.0, 1.0, -1.0]);
		assert_eq!(settings.eq.preamp_db, -24.0);
	}

	#[test]
	fn older_and_corrupt_files_do_not_block_startup() {
		let directory = tempfile::tempdir().unwrap();
		let dir = directory.path();
		// A file from an older version with fewer fields still loads.
		fs::write(dir.join("settings.json"), br#"{"eq_preset":"vocal"}"#).unwrap();
		assert_eq!(Settings::load(dir).eq_preset.as_deref(), Some("vocal"));
		// A corrupt file falls back to defaults instead of failing startup.
		fs::write(dir.join("settings.json"), b"{not json").unwrap();
		assert_eq!(Settings::load(dir), Settings::default());
	}

	#[test]
	fn retired_sources_do_not_reset_saved_preferences() {
		let directory = tempfile::tempdir().unwrap();
		let dir = directory.path();
		fs::write(
			dir.join("settings.json"),
			br#"{"eq_preset":"rock","sources":{"lyrics":false,"wikipedia":false,"web_player":false},"audio":{"mono":true}}"#,
		)
		.unwrap();
		let settings = Settings::load(dir);
		assert_eq!(settings.eq_preset.as_deref(), Some("rock"));
		assert!(settings.prefs.audio.mono);
		settings.save(dir).unwrap();
		let saved: serde_json::Value = serde_json::from_slice(&fs::read(dir.join("settings.json")).unwrap()).unwrap();
		assert!(saved.get("sources").is_none());
		assert_eq!(Settings::load(dir), settings);
	}

	#[test]
	fn an_unknown_normalize_level_costs_nothing_else() {
		let directory = tempfile::tempdir().unwrap();
		let dir = directory.path();
		fs::write(dir.join("settings.json"), br#"{"eq_preset":"rock","audio":{"normalize":"ultra","mono":true},"future_thing":[1,2]}"#)
			.unwrap();
		let s = Settings::load(dir);
		assert_eq!(s.eq_preset.as_deref(), Some("rock"));
		assert_eq!(s.prefs.audio.normalize, Normalize::Off);
		assert!(s.prefs.audio.mono);
	}

	#[test]
	fn values_a_hand_edit_could_break_are_brought_into_range() {
		let directory = tempfile::tempdir().unwrap();
		let dir = directory.path();
		fs::write(
			dir.join("settings.json"),
			br#"{"audio":{"balance":100},"device":{"name":"  \u0007Den\u0000 ","volume":250,"cache_mb":50}}"#,
		)
		.unwrap();
		let s = Settings::load(dir);
		let saved = serde_json::to_value(&s).unwrap();
		assert!(saved["audio"].get("balance").is_none(), "retired balance settings cannot pan audio or persist");
		assert_eq!(s.prefs.device.volume, 100);
		assert_eq!(s.prefs.device.cache_mb, 100);
		assert_eq!(s.prefs.device.name, "Den");
		fs::write(dir.join("settings.json"), br#"{"device":{"name":"   ","cache_mb":99999}}"#).unwrap();
		let s = Settings::load(dir);
		assert_eq!(s.prefs.device.name, DEFAULT_DEVICE_NAME);
		assert_eq!(s.prefs.device.cache_mb, 2048);
	}

	type Check = fn(&Prefs) -> bool;

	#[test]
	fn each_change_lands_in_its_own_preference_and_stays_in_range() {
		let mut p = Prefs::default();
		let out = Output { uid: "u".into(), name: "n".into() };
		let checks: [(Pref, Check); 11] = [
			(Pref::Autoplay(false), |p| !p.playback.autoplay),
			(Pref::Explicit(false), |p| !p.playback.explicit),
			(Pref::Crossfade(255), |p| p.playback.crossfade == MAX_CROSSFADE),
			(Pref::Crossfade(0), |p| p.playback.crossfade == 0),
			(Pref::Normalize(Normalize::Quiet), |p| p.audio.normalize == Normalize::Quiet),
			(Pref::Mono(true), |p| p.audio.mono),
			(Pref::Output(Some(out)), |p| p.audio.output.as_ref().is_some_and(|out| out.uid == "u" && out.name == "n")),
			(Pref::Output(None), |p| p.audio.output.is_none()),
			(Pref::StartupVolume(250), |p| p.device.volume == 100),
			(Pref::Compact(true), |p| p.display.compact),
			(Pref::PlayCounts(false), |p| !p.display.play_counts),
		];
		for (pref, check) in checks {
			assert!(p.set(&pref), "{pref:?}");
			assert!(check(&p), "{pref:?}");
		}
		// The now-playing view is kept elsewhere: nothing here changes.
		let before = p.clone();
		assert!(!p.set(&Pref::NowPlayingView(false)));
		assert_eq!(p, before);
	}

	#[test]
	fn crossfade_survives_restart_and_old_settings_keep_it_off() {
		let directory = tempfile::tempdir().unwrap();
		let dir = directory.path();
		let mut settings = Settings::default();
		assert_eq!(settings.prefs.playback.crossfade, 0);
		settings.prefs.set(&Pref::Crossfade(6));
		settings.save(dir).unwrap();
		assert_eq!(Settings::load(dir).prefs.playback.crossfade, 6);
		fs::write(dir.join("settings.json"), br#"{"playback":{"crossfade":255}}"#).unwrap();
		assert_eq!(Settings::load(dir).prefs.playback.crossfade, MAX_CROSSFADE);
		fs::write(dir.join("settings.json"), br#"{"playback":{"autoplay":false}}"#).unwrap();
		assert_eq!(Settings::load(dir).prefs.playback.crossfade, 0);
	}

	#[test]
	fn names_are_cleaned_and_cut() {
		assert_eq!(sanitize_name("Living room"), "Living room");
		assert_eq!(sanitize_name(&"é".repeat(100)).chars().count(), MAX_DEVICE_NAME);
		assert_eq!(sanitize_name("a\nb"), "ab");
		assert_eq!(sanitize_name(""), DEFAULT_DEVICE_NAME);
	}
}
