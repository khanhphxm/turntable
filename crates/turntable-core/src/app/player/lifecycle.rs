//! Playback engine and audio-output lifecycle, preferences and machine facts for Settings.

use std::{
	thread,
	time::{Instant, SystemTime},
};

use super::{AUDIO_RETRY, AppEvent, Controller, HEALTH_EVERY, NO_AUDIO, SYSTEM_EVERY, StartError, retry_wait};
use crate::{
	app::{Panel, config::Paths},
	engine::{Command, Engine, EngineConfig},
};

/// Resources detached from the UI and dropped in playback dependency order.
struct Detached {
	audio:  Option<crate::audio::Player>,
	engine: Option<Engine>,
}

impl Controller {
	pub(super) fn initialize_system(&mut self) {
		self.state.sys.places = crate::app::system::Places { config: self.paths.config.clone(), cache: self.paths.cache.clone() };
		if crate::audio::CAN_PICK_OUTPUT {
			self.refresh_outputs();
			self._devices = self.watch_devices();
		}
	}

	/// The Connect name the player starts with: the command line's, else the setting.
	fn engine_name(&self) -> String {
		self.state.sys.device_name(&self.state.prefs.device.name).to_owned()
	}

	/// Hands the audio output stage the preferences and the player's volume.
	fn push_stage(&self) {
		let Some(audio) = &self.audio else { return };
		let a = &self.state.prefs.audio;
		audio.set_crossfade(self.state.prefs.playback.crossfade);
		audio.set_stage(&crate::audio::StageParams {
			normalize: a.normalize,
			mono:      a.mono,
			balance:   f32::from(a.balance) / f32::from(crate::app::settings::BALANCE_STEPS),
			volume_db: crate::audio::volume_db(self.state.volume),
		});
	}

	/// The device the user picked, if it is connected (else the system's default plays).
	fn chosen_output(&self) -> Option<String> {
		let sys = &self.state.sys;
		self.state.prefs.audio.output.as_ref().filter(|o| sys.outputs.iter().any(|d| d.uid == o.uid)).map(|o| o.uid.clone())
	}

	/// Makes the output the chosen device, or the system's default while that one isn't there
	/// (unplugged: the music goes on, and moves back when it returns); tried again when it can't be opened.
	pub(super) fn reconcile_output(&mut self) {
		if !crate::audio::CAN_PICK_OUTPUT {
			return;
		}
		let want = self.chosen_output();
		let Some(audio) = &mut self.audio else { return };
		if audio.has_output() && audio.device() == want.as_deref() {
			return;
		}
		match audio.set_device(want.as_deref()) {
			Ok(()) => {
				self.output_retry_at = None;
				if self.state.notice.as_deref().is_some_and(|n| n.starts_with(NO_AUDIO)) {
					self.state.notice = None;
				}
			},
			Err(e) => {
				self.state.notice = Some(format!("{NO_AUDIO}{e}. Trying again…"));
				self.output_retry_at = Some(Instant::now() + AUDIO_RETRY);
			},
		}
	}

	/// Looks at the output devices again.
	pub(super) fn refresh_outputs(&mut self) {
		self.state.sys.outputs = crate::audio::output_devices();
		self.state.sys.default_output = crate::audio::default_output_name();
	}

	/// Has the audio side tell the loop when devices come and go.
	pub(super) fn watch_devices(&self) -> Option<crate::audio::DeviceWatch> {
		let (tx, wake) = (self.tx.clone(), self.wake.clone());
		crate::audio::watch_devices(move || {
			if tx.send(AppEvent::Devices).is_ok() {
				wake();
			}
		})
	}

	/// Looks at app memory (again in a few seconds while the page shows it).
	pub(super) fn refresh_memory(&mut self) {
		self.state.sys.app_kb = crate::app::system::memory_kb(std::process::id());
		self.system_at = (self.state.panel == Some(Panel::Settings)).then(|| Instant::now() + SYSTEM_EVERY);
	}

	/// Measures the player's cache on a thread of its own (a big one takes a moment).
	fn measure_cache(&self) {
		let dir = self.paths.cache.join("player").join("files");
		let (tx, wake) = (self.tx.clone(), self.wake.clone());
		let _ = thread::Builder::new().name("cache-size".into()).spawn(move || {
			if tx.send(AppEvent::CacheSize(crate::app::system::dir_bytes(&dir))).is_ok() {
				wake();
			}
		});
	}

	/// The settings page opened: look at everything it shows.
	pub(super) fn refresh_system(&mut self) {
		self.refresh_outputs();
		self.refresh_memory();
		self.measure_cache();
	}

	/// Carries out what the preferences drive outside the state, and saves them.
	pub(super) fn apply_prefs(&mut self) {
		crate::app::settings::allowed::set(self.state.prefs.sources);
		self.push_stage();
		self.reconcile_output();
		if !self.state.prefs.playback.autoplay && self.state.logged_in {
			// What follows the song playing decides whether it is the last.
			self.send_player(Command::GetQueue);
		}
		self.save_settings();
	}

	/// Restarts the player so a new name or cache size takes effect (playback stops for a few seconds).
	pub(super) fn restart_player(&mut self) {
		if self.state.sys.clearing_cache {
			return;
		}
		self.discard_start = self.state.starting;
		self.stop_engine();
		self.start_failures = 0;
		let name = self.engine_name();
		self.state.disconnected(format!("Restarting the player as “{name}”…"));
		self.start_engine();
	}

	/// Empties the player's playback cache. The player keeps files open while it runs, so it is
	/// stopped first and started again afterwards.
	pub(super) fn clear_cache(&mut self) {
		self.discard_start = self.state.starting;
		let detached = self.detach_engine();
		self.state.disconnected("Emptying the player's cache…".into());
		let dir = self.paths.cache.join("player").join("files");
		let (tx, wake) = (self.tx.clone(), self.wake.clone());
		let _ = thread::Builder::new().name("cache-clear".into()).spawn(move || {
			drop(detached.audio);
			drop(detached.engine);
			let result = crate::app::system::clear_dir(&dir).map_err(|e| e.to_string());
			if tx.send(AppEvent::CacheCleared(result)).is_ok() {
				wake();
			}
		});
	}

	pub(super) fn on_cache_cleared(&mut self, result: Result<(), String>) {
		self.state.sys.clearing_cache = false;
		match result {
			Ok(()) => self.state.show_toast("Cache emptied".into()),
			Err(e) => self.state.show_toast(format!("Couldn't empty the cache: {e}")),
		}
		self.measure_cache();
		if self.state.signed_in {
			self.start_engine();
		}
	}

	/// Notes what the engine that just started was started with.
	pub(super) fn on_engine_started(&mut self) {
		let sys = &mut self.state.sys;
		sys.running = sys.launching.take();
		self.refresh_memory();
		self.health_at = Instant::now() + HEALTH_EVERY;
	}

	pub(super) fn start_engine(&mut self) {
		if self.state.starting || self.engine.is_some() || self.state.sys.clearing_cache {
			return;
		}
		self.state.starting = true;
		self.state.sign_in_error = None;
		self.state.notice = Some("Starting the player…".into());
		self.retry_start_at = None;
		let launch =
			Launch { name: self.engine_name(), volume: self.state.prefs.device.volume, cache_mb: self.state.prefs.device.cache_mb };
		self.state.sys.launching = Some(crate::app::system::Running { name: launch.name.clone(), cache_mb: launch.cache_mb });
		// The output starts now (it plays silence-free nothing until audio arrives); the decoder
		// gets the other end once the player is online.
		let (writer, feed) = crate::audio::channel();
		self.start_audio(feed);
		let paths = self.paths.clone();
		let (tx, wake) = (self.tx.clone(), self.wake.clone());
		let send = move |event: AppEvent| {
			if tx.send(event).is_ok() {
				wake();
			}
		};
		let _ = thread::Builder::new().name("engine-start".into()).spawn(move || {
			let result = start_engine(&paths, &launch, writer, &mut |text| send(AppEvent::Progress(text.to_owned())));
			send(AppEvent::EngineReady(result));
		});
	}

	/// Plays the audio path. If the platform output cannot start, keeps the feed and retries.
	pub(super) fn start_audio(&mut self, feed: crate::audio::Feed) {
		let device = self.chosen_output();
		match crate::audio::Player::start(feed, device.as_deref()) {
			Ok(audio) => {
				audio.set_eq(self.state.eq);
				self.audio = Some(audio);
				self.push_stage();
				self.state.has_audio = true;
				if self.state.notice.as_deref().is_some_and(|n| n.starts_with(NO_AUDIO)) {
					self.state.notice = None;
				}
			},
			Err(e) => {
				self.state.notice = Some(format!("{NO_AUDIO}{}. Trying again…", e.error));
				self.pending_feed = Some(e.feed);
				self.audio_retry_at = Some(Instant::now() + AUDIO_RETRY);
			},
		}
	}

	/// Lets go of the connection, the audio and the engine, and hands the last two back to be
	/// dropped (in the background: the player says goodbye to Spotify first).
	fn detach_engine(&mut self) -> Detached {
		if let Some(engine) = &self.engine {
			engine.stop();
		}
		self.retry_start_at = None;
		self.client = None;
		self.library.set_session(None);
		self.lyrics = None;
		self.state.lyrics = None;
		self.state.has_audio = false;
		self.state.sys.running = None;
		self.state.sys.launching = None;
		// The feed of the stopped engine is worth nothing to a retry.
		self.pending_feed = None;
		self.audio_retry_at = None;
		self.output_retry_at = None;
		Detached { audio: self.audio.take(), engine: self.engine.take() }
	}

	/// Drops the connection, the audio and the engine (in the background).
	pub(super) fn stop_engine(&mut self) {
		let detached = self.detach_engine();
		let _ = thread::Builder::new().name("engine-stop".into()).spawn(move || {
			drop(detached.audio);
			drop(detached.engine);
		});
	}

	/// The engine's periodic maintenance; an ended engine or event stream is dropped and started
	/// again. Returns whether anything visible changed.
	pub(super) fn check_engine(&mut self) -> bool {
		let Some(engine) = self.engine.as_mut() else { return false };
		if self.client.is_some() && engine.alive() {
			return false;
		}
		self.network_disconnected("Spotify disconnected; reconnecting…");
		self.start_failures = self.start_failures.saturating_add(1);
		self.retry_start_at = Some(Instant::now() + retry_wait(self.start_failures));
		true
	}

	/// Call on a host wake notification. The periodic pump also detects long sleep gaps.
	/// Re-authenticates with saved credentials; never starts playback automatically.
	pub fn resume(&mut self) {
		if !self.state.signed_in || self.state.sys.clearing_cache {
			return;
		}
		self.last_tick = (Instant::now(), SystemTime::now());
		self.discard_start = self.state.starting;
		self.network_disconnected("Reconnecting to Spotify after wake…");
		self.start_engine();
	}

	pub(super) fn network_disconnected(&mut self, reason: &str) {
		self.playback_interrupted |= self.state.playing() && self.state.active;
		self.stop_engine();
		self.player_offline = true;
		self.refresh_network_notice();
		self.state.disconnected(reason.into());
	}

	pub(super) fn refresh_network_notice(&mut self) {
		self.state.offline = self.player_offline || self.library_offline;
		self.state.network_notice = if self.state.offline {
			Some("Can't reach Spotify. Showing saved library data; reconnecting…".into())
		} else if self.playback_interrupted {
			Some("Reconnected to Spotify. Playback stopped; press Play to resume.".into())
		} else {
			None
		};
	}

	pub(super) fn connect(&mut self) {
		if self.client.is_some() {
			return;
		}
		let Some(engine) = self.engine.as_mut() else { return };
		let (tx, wake) = (self.tx.clone(), self.wake.clone());
		self.client = engine.connect(move |incoming| {
			if tx.send(AppEvent::Player(incoming)).is_ok() {
				wake();
			}
		});
	}
	/// Forgets the sign-in on this computer: the player stops, the library empties, and the setup
	/// page asks to sign in again.
	pub(super) fn sign_out(&mut self) {
		self.state.recent_searches.clear();
		if let Err(error) = self.state.recent_searches.save(&self.paths.config) {
			self.state.notice = Some(format!("Couldn't clear recent searches: {error}"));
		}
		self.state.recent_searches.load(&self.paths.config, None);
		self.discard_start = self.state.starting;
		self.stop_engine();
		self.library.send(crate::library::Request::ForgetSnapshot);
		if let Err(e) = crate::engine::forget_sign_in(&self.paths.config) {
			self.state.notice = Some(format!("Couldn't forget the sign-in: {e}"));
		}
		self.state.signed_in = false;
		self.player_offline = false;
		self.library_offline = false;
		self.library_retry_at = None;
		self.playback_interrupted = false;
		self.refresh_network_notice();
		self.state.disconnected("Signed out.".into());
	}

	/// A definitive subscription refusal is a sign-out, never an offline retry.
	pub(super) fn reject_sign_in(&mut self, error: crate::engine::SignInError) {
		self.sign_out();
		self.state.starting = false;
		self.state.library = crate::library::Library::default();
		self.state.item = crate::engine::Entity::default();
		self.state.context = crate::engine::Entity::default();
		self.state.upcoming.clear();
		self.state.connect = crate::app::connect::Connect::default();
		self.restore = super::restore::Restore::new(false);
		self.search_at = None;
		self.poll_at = None;
		self.poll_inflight = false;
		self.force_poll = false;
		self.state.notice = Some(error.message());
		self.state.sign_in_error = Some(error);
	}

	/// Copies the persisted preferences out of the state and writes them.
	pub(super) fn save_settings(&mut self) {
		self.settings.eq = self.state.eq;
		self.settings.eq_preset.clone_from(&self.state.eq_preset);
		self.settings.now_playing_view = self.state.now_playing_view;
		self.settings.sidebar_collapsed = self.state.sidebar_collapsed;
		self.settings.prefs.clone_from(&self.state.prefs);
		if let Err(e) = self.settings.save(&self.paths.config) {
			self.state.notice = Some(format!("Couldn't save settings: {e}"));
		}
	}
}
/// What the player is started with.
struct Launch {
	name:     String,
	volume:   u8,
	cache_mb: u32,
}

fn start_engine(
	paths: &Paths,
	launch: &Launch,
	writer: crate::audio::PcmWriter,
	progress: &mut dyn FnMut(&str),
) -> Result<Engine, StartError> {
	let config = EngineConfig {
		device_name: launch.name.clone(),
		volume:      launch.volume,
		state_dir:   paths.config.clone(),
		cache_dir:   paths.cache.join("player"),
		cache_mb:    launch.cache_mb,
	};
	Engine::start(&config, writer, progress).map_err(|error| {
		match error.get_ref().and_then(|source| source.downcast_ref::<crate::engine::SignInError>()) {
			Some(error) => StartError::Premium(error.clone()),
			None => StartError::Other(format!("Can't start the player: {error}")),
		}
	})
}
