//! User preferences that survive restarts: `<config>/settings.json`.
//! Unknown or missing fields fall back to defaults, so older files keep working.

use std::{fs, io, path::Path};

use serde::{Deserialize, Serialize};

use crate::audio::{EqSettings, Normalize};

/// The Spotify Connect name of the player until the user picks one.
pub const DEFAULT_DEVICE_NAME: &str = "terntable";
/// Longest device name kept (Spotify shows it in a menu).
pub const MAX_DEVICE_NAME: usize = 48;
/// The playback cache limits offered, in MB (0 disables the cache).
pub const CACHE_SIZES: [u32; 6] = [100, 250, 500, 1024, 2048, 0];
/// The volume the player starts at until the user picks one.
pub const DEFAULT_VOLUME: u8 = 50;
/// Balance steps from the middle to either side.
pub const BALANCE_STEPS: i8 = 10;

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
	pub sources:  Sources,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Playback {
	/// Play recommended songs when what was chosen runs out; off pauses there instead.
	pub autoplay: bool,
	/// Songs marked explicit play; off skips them.
	pub explicit: bool,
}

impl Default for Playback {
	fn default() -> Self {
		Self { autoplay: true, explicit: true }
	}
}

/// The audio output preferences (macOS).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Audio {
	pub normalize: Normalize,
	pub mono:      bool,
	/// From `-BALANCE_STEPS` (all left) to `BALANCE_STEPS` (all right).
	pub balance:   i8,
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

/// The outside services terntable may ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sources {
	/// Synced lyrics from LRCLIB.
	pub lyrics:     bool,
	/// Artist biographies from Wikidata and Wikipedia.
	pub wikipedia:  bool,
	/// Listeners, photos, credits, play counts and other people's playlists from Spotify's
	/// anonymous web player.
	pub web_player: bool,
}

impl Default for Sources {
	fn default() -> Self {
		Self { lyrics: true, wikipedia: true, web_player: true }
	}
}

impl Prefs {
	/// Brings hand-edited or newer settings files into the app's supported ranges.
	pub fn sanitized(mut self) -> Self {
		self.audio.balance = self.audio.balance.clamp(-BALANCE_STEPS, BALANCE_STEPS);
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

/// What a cache limit is called.
pub fn cache_label(mb: u32) -> String {
	match mb {
		0 => "Off".to_owned(),
		mb if mb >= 1024 && mb.is_multiple_of(1024) => format!("{} GB", mb / 1024),
		mb => format!("{mb} MB"),
	}
}

/// One change the settings page makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pref {
	Autoplay(bool),
	Explicit(bool),
	Normalize(Normalize),
	Mono(bool),
	/// Steps from the middle, `-BALANCE_STEPS` to `BALANCE_STEPS`.
	Balance(i8),
	/// A device to play on (`None`: the system's default).
	Output(Option<Output>),
	StartupVolume(u8),
	CacheMb(u32),
	/// Whether the now-playing view opens by itself (kept apart from [`Prefs`], with the layout).
	NowPlayingView(bool),
	Compact(bool),
	PlayCounts(bool),
	Lyrics(bool),
	Wikipedia(bool),
	WebPlayer(bool),
}

impl Prefs {
	/// Makes a change. Returns false for the one that isn't kept here ([`Pref::NowPlayingView`]).
	pub fn set(&mut self, pref: &Pref) -> bool {
		match pref {
			Pref::Autoplay(on) => self.playback.autoplay = *on,
			Pref::Explicit(on) => self.playback.explicit = *on,
			Pref::Normalize(level) => self.audio.normalize = *level,
			Pref::Mono(on) => self.audio.mono = *on,
			Pref::Balance(steps) => self.audio.balance = (*steps).clamp(-BALANCE_STEPS, BALANCE_STEPS),
			Pref::Output(device) => self.audio.output.clone_from(device),
			Pref::StartupVolume(volume) => self.device.volume = (*volume).min(100),
			Pref::CacheMb(mb) => self.device.cache_mb = sanitize_cache(*mb),
			Pref::Compact(on) => self.display.compact = *on,
			Pref::PlayCounts(on) => self.display.play_counts = *on,
			Pref::Lyrics(on) => self.sources.lyrics = *on,
			Pref::Wikipedia(on) => self.sources.wikipedia = *on,
			Pref::WebPlayer(on) => self.sources.web_player = *on,
			Pref::NowPlayingView(_) => return false,
		}
		true
	}
}

impl Settings {
	pub fn load(config: &Path) -> Self {
		let mut settings: Self =
			fs::read(config.join("settings.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
		settings.prefs = settings.prefs.sanitized();
		settings
	}

	pub fn save(&self, config: &Path) -> io::Result<()> {
		let json = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
		// 0600 and fsynced before the rename, like every file in the config dir.
		crate::app::config::write_private(&config.join("settings.json"), &json)
	}
}

/// What the worker threads (lyrics, open data, the web player) may ask: the current [`Sources`],
/// readable from any thread. The UI thread sets it whenever the preference changes, and every
/// request those threads make checks it first, so a source that is off is never contacted.
pub mod allowed {
	use std::sync::atomic::{AtomicBool, Ordering};

	use super::Sources;

	static LYRICS: AtomicBool = AtomicBool::new(true);
	static WIKIPEDIA: AtomicBool = AtomicBool::new(true);
	static WEB_PLAYER: AtomicBool = AtomicBool::new(true);

	pub fn set(sources: Sources) {
		LYRICS.store(sources.lyrics, Ordering::Relaxed);
		WIKIPEDIA.store(sources.wikipedia, Ordering::Relaxed);
		WEB_PLAYER.store(sources.web_player, Ordering::Relaxed);
	}

	/// Lyrics from LRCLIB.
	pub fn lyrics() -> bool {
		LYRICS.load(Ordering::Relaxed)
	}

	/// Artist biographies from Wikidata and Wikipedia.
	pub fn wikipedia() -> bool {
		WIKIPEDIA.load(Ordering::Relaxed)
	}

	/// Spotify's anonymous web player.
	pub fn web_player() -> bool {
		WEB_PLAYER.load(Ordering::Relaxed)
	}

	/// Runs `f` with these sources allowed and nothing else running meanwhile (the flags are
	/// process-wide, and tests run in parallel), then allows everything again.
	#[cfg(test)]
	pub fn only_with<T>(sources: Sources, f: impl FnOnce() -> T) -> T {
		static EXCLUSIVE: std::sync::Mutex<()> = std::sync::Mutex::new(());
		let _guard = EXCLUSIVE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
		set(sources);
		let result = f();
		set(Sources::default());
		result
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn scratch(name: &str) -> std::path::PathBuf {
		let dir = std::env::temp_dir().join(format!("tt-settings-{name}-{}", std::process::id()));
		fs::create_dir_all(&dir).unwrap();
		dir
	}

	#[test]
	fn older_and_corrupt_files_do_not_block_startup() {
		let dir = scratch("older-and-corrupt");
		// A file from an older version with fewer fields still loads.
		fs::write(dir.join("settings.json"), br#"{"eq_preset":"vocal"}"#).unwrap();
		assert_eq!(Settings::load(&dir).eq_preset.as_deref(), Some("vocal"));
		// A corrupt file falls back to defaults instead of failing startup.
		fs::write(dir.join("settings.json"), b"{not json").unwrap();
		assert_eq!(Settings::load(&dir), Settings::default());
		fs::remove_dir_all(dir).unwrap();
	}

	#[test]
	fn an_unknown_normalize_level_costs_nothing_else() {
		let dir = scratch("future");
		fs::write(dir.join("settings.json"), br#"{"eq_preset":"rock","audio":{"normalize":"ultra","mono":true},"future_thing":[1,2]}"#)
			.unwrap();
		let s = Settings::load(&dir);
		assert_eq!(s.eq_preset.as_deref(), Some("rock"));
		assert_eq!(s.prefs.audio.normalize, Normalize::Off);
		assert!(s.prefs.audio.mono);
		fs::remove_dir_all(dir).unwrap();
	}

	#[test]
	fn values_a_hand_edit_could_break_are_brought_into_range() {
		let dir = scratch("range");
		fs::write(
			dir.join("settings.json"),
			br#"{"audio":{"balance":100},"device":{"name":"  \u0007Den\u0000 ","volume":250,"cache_mb":50}}"#,
		)
		.unwrap();
		let s = Settings::load(&dir);
		assert_eq!(s.prefs.audio.balance, BALANCE_STEPS);
		assert_eq!(s.prefs.device.volume, 100);
		assert_eq!(s.prefs.device.cache_mb, 100);
		assert_eq!(s.prefs.device.name, "Den");
		fs::write(dir.join("settings.json"), br#"{"device":{"name":"   ","cache_mb":99999}}"#).unwrap();
		let s = Settings::load(&dir);
		assert_eq!(s.prefs.device.name, DEFAULT_DEVICE_NAME);
		assert_eq!(s.prefs.device.cache_mb, 2048);
		fs::remove_dir_all(dir).unwrap();
	}

	type Check = fn(&Prefs) -> bool;

	#[test]
	fn each_change_lands_in_its_own_preference_and_stays_in_range() {
		let mut p = Prefs::default();
		let out = Output { uid: "u".into(), name: "n".into() };
		let checks: [(Pref, Check); 16] = [
			(Pref::Autoplay(false), |p| !p.playback.autoplay),
			(Pref::Explicit(false), |p| !p.playback.explicit),
			(Pref::Normalize(Normalize::Quiet), |p| p.audio.normalize == Normalize::Quiet),
			(Pref::Mono(true), |p| p.audio.mono),
			(Pref::Balance(99), |p| p.audio.balance == BALANCE_STEPS),
			(Pref::Balance(-99), |p| p.audio.balance == -BALANCE_STEPS),
			(Pref::Output(Some(out)), |p| p.audio.output.as_ref().is_some_and(|out| out.uid == "u" && out.name == "n")),
			(Pref::Output(None), |p| p.audio.output.is_none()),
			(Pref::StartupVolume(250), |p| p.device.volume == 100),
			(Pref::CacheMb(50), |p| p.device.cache_mb == 100),
			(Pref::CacheMb(0), |p| p.device.cache_mb == 0),
			(Pref::Compact(true), |p| p.display.compact),
			(Pref::PlayCounts(false), |p| !p.display.play_counts),
			(Pref::Lyrics(false), |p| !p.sources.lyrics),
			(Pref::Wikipedia(false), |p| !p.sources.wikipedia),
			(Pref::WebPlayer(false), |p| !p.sources.web_player),
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
	fn names_are_cleaned_and_cut() {
		assert_eq!(sanitize_name("Living room"), "Living room");
		assert_eq!(sanitize_name(&"é".repeat(100)).chars().count(), MAX_DEVICE_NAME);
		assert_eq!(sanitize_name("a\nb"), "ab");
		assert_eq!(sanitize_name(""), DEFAULT_DEVICE_NAME);
	}
}
