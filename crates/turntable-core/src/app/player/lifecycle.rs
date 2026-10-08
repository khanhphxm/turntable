//! Playback engine and audio-output lifecycle, preferences and machine facts for Settings.

use std::{
	thread,
	time::{Instant, SystemTime},
};

use super::{AUDIO_RETRY, AppEvent, Controller, HEALTH_EVERY, NO_AUDIO, SAVE_DEBOUNCE, StartError, output, persistence::Save, retry_wait};
use crate::{
	app::config::Paths,
	engine::{Command, Engine, EngineConfig},
};

/// Resources detached from the UI and dropped in playback dependency order.
struct Detached {
	audio:  Option<crate::audio::Player>,
	engine: Option<Engine>,
}

impl Controller {
	pub(super) fn initialize_system(&mut self) {
		if crate::audio::CAN_PICK_OUTPUT {
			self.refresh_outputs();
			self._devices = self.watch_devices();
		}
	}

	/// The Connect name supplied by an embedding host, else the saved setting.
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
			balance:   0.0,
			volume_db: crate::audio::volume_db(self.state.volume),
		});
	}

	/// The device the user picked, if it is connected (else the system's default plays).
	fn chosen_output(&self) -> Option<&str> {
		let sys = &self.state.sys;
		self.state.prefs.audio.output.as_ref().filter(|o| sys.outputs.iter().any(|d| d.uid == o.uid)).map(|o| o.uid.as_str())
	}

	/// Makes the output the chosen device, or the system's default while that one isn't there
	/// (unplugged: the music goes on, and moves back when it returns); tried again when it can't be opened.
	pub(super) fn reconcile_output(&mut self) {
		if self.audio_inflight {
			return;
		}
		let want = self.chosen_output();
		let Some(audio) = &self.audio else { return };
		if !audio.is_dead() && (!crate::audio::CAN_PICK_OUTPUT || (audio.has_output() && !audio.is_stale() && audio.device() == want)) {
			return;
		}
		let Some(audio) = self.audio.take() else { return };
		let job = if audio.is_dead() { output::Job::Rebuild(audio) } else { output::Job::Select(audio) };
		self.queue_audio(job);
	}

	/// Enumerates on the output worker; coalesce hotplug bursts without losing the final update.
	pub(super) fn refresh_outputs(&mut self) {
		if self.outputs_inflight {
			self.outputs_dirty = true;
			return;
		}
		self.outputs_inflight = self.output_worker.refresh();
	}

	pub(super) fn on_outputs(&mut self, devices: Vec<crate::audio::OutputDevice>, default: Option<String>) {
		self.outputs_inflight = false;
		self.state.sys.outputs = devices;
		self.state.sys.default_output = default;
		if std::mem::take(&mut self.outputs_dirty) {
			self.refresh_outputs();
		}
		self.reconcile_output();
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

	/// Carries out what the preferences drive outside the state, and saves them.
	pub(super) fn apply_prefs(&mut self) {
		self.push_stage();
		self.reconcile_output();
		if !self.state.prefs.playback.autoplay && self.state.logged_in {
			// What follows the song playing decides whether it is the last.
			self.send_player(Command::GetQueue);
		}
		self.save_settings();
	}

	/// Cancel old authorization before rebuilding: a late callback must not use the old route.
	pub(super) fn reconfigure_network(&mut self) {
		if let Err(error) = crate::net::configure_proxy(&self.state.prefs.network.proxy) {
			self.state.notice = Some(error.into());
			return;
		}
		let restart = self.state.signed_in || self.state.starting;
		self.account_generation = self.account_generation.wrapping_add(1);
		self.start_authorization.cancel();
		self.start_authorization = crate::engine::StartAuthorization::default();
		self.library.reconfigure_network(self.account_generation);
		self.stop_engine();
		self.state.starting = false;
		self.discard_start = false;
		self.state.disconnected("Network settings changed.".into());
		self.save_settings();
		if restart {
			self.start_engine();
		}
	}

	/// Notes what the engine that just started was started with.
	pub(super) fn on_engine_started(&mut self) {
		self.health_at = Instant::now() + HEALTH_EVERY;
	}

	pub(super) fn start_engine(&mut self) {
		#[cfg(any(test, feature = "test-support"))]
		assert!(self.fixture_inbox.is_none(), "offline fixtures cannot start the playback engine");
		if self.state.starting || self.engine.is_some() {
			return;
		}
		self.state.starting = true;
		if !self.state.signed_in {
			self.start_authorization.set_deadline(Some(Instant::now() + crate::engine::SIGN_IN_TIMEOUT));
		}
		self.state.sign_in_error = None;
		self.state.notice = Some("Starting the player…".into());
		self.retry_start_at = None;
		let launch = Launch {
			name:     self.engine_name(),
			volume:   self.state.prefs.device.volume,
			cache_mb: self.state.prefs.device.cache_mb,
			proxy:    self.state.prefs.network.proxy.clone(),
		};
		// The output starts now (it plays silence-free nothing until audio arrives); the decoder
		// gets the other end once the player is online.
		let (writer, feed) = crate::audio::channel();
		self.audio_generation = self.audio_generation.wrapping_add(1);
		self.start_audio(feed);
		let paths = self.paths.clone();
		let (tx, wake) = (self.tx.clone(), self.wake.clone());
		let generation = self.account_generation;
		let authorization = self.start_authorization.clone();
		let send = move |event: AppEvent| {
			if tx.send(event).is_ok() {
				wake();
			}
		};
		let _ = thread::Builder::new().name("engine-start".into()).spawn(move || {
			let result =
				start_engine(&paths, &launch, writer, &mut |text| send(AppEvent::Progress(generation, text.to_owned())), &authorization);
			send(AppEvent::EngineReady(generation, result));
		});
	}

	/// Queues a platform open; its feed remains usable if no output device is available.
	pub(super) fn start_audio(&mut self, feed: crate::audio::Feed) {
		// A failed engine start can leave an output/feed behind. A new path supersedes both;
		// dispose the old backend on the same worker before opening this one.
		if let Some(audio) = self.audio.take() {
			self.output_worker.dispose(Ok(audio));
		}
		self.state.has_audio = false;
		self.pending_feed = None;
		self.audio_retry_at = None;
		self.output_retry_at = None;
		let (tx, wake) = (self.tx.clone(), self.wake.clone());
		let generation = self.audio_generation;
		feed.on_failure(move || {
			if tx.send(AppEvent::AudioFailed(generation)).is_ok() {
				wake();
			}
		});
		self.queue_audio(output::Job::Start(feed));
	}

	fn queue_audio(&mut self, job: output::Job) {
		self.audio_inflight = true;
		let request = output::Request::Open { generation: self.audio_generation, job, device: self.chosen_output().map(str::to_owned) };
		if let Err(error) = self.output_worker.send(request) {
			self.audio_inflight = false;
			self.state.notice = Some(format!("{NO_AUDIO}audio worker stopped. Trying again…"));
			if let output::Request::Open { job, .. } = error.0 {
				match job {
					output::Job::Start(feed) => {
						self.pending_feed = Some(feed);
						self.audio_retry_at = Some(Instant::now() + AUDIO_RETRY);
					},
					output::Job::Select(audio) | output::Job::Rebuild(audio) => {
						self.audio = Some(audio);
						self.output_retry_at = Some(Instant::now() + AUDIO_RETRY);
					},
				}
			}
		}
	}

	pub(super) fn on_audio_ready(
		&mut self,
		generation: u64,
		selected: Option<&str>,
		result: Result<crate::audio::Player, crate::audio::StartError>,
	) -> bool {
		if generation != self.audio_generation {
			self.output_worker.dispose(result);
			return false;
		}
		self.audio_inflight = false;
		match result {
			Ok(audio) => {
				audio.set_eq(self.state.eq);
				let stale = audio.is_stale();
				self.state.has_audio = !audio.is_dead();
				self.audio = Some(audio);
				self.push_stage();
				if self.state.has_audio {
					self.output_retry_at = None;
					if self.state.notice.as_deref().is_some_and(|n| n.starts_with(NO_AUDIO)) {
						self.state.notice = None;
					}
					// A preference/default change during the open must not be lost. Don't repeatedly
					// reopen a busy chosen device that successfully fell back to the default.
					if stale || selected != self.chosen_output() {
						self.reconcile_output();
					}
				} else {
					self.on_audio_failed(generation);
				}
			},
			Err(error) => {
				self.state.has_audio = false;
				self.state.notice = Some(format!("{NO_AUDIO}{}. Trying again…", error.error));
				self.pending_feed = Some(error.feed);
				self.audio_retry_at = Some(Instant::now() + AUDIO_RETRY);
			},
		}
		true
	}

	pub(super) fn on_audio_failed(&mut self, generation: u64) {
		if generation != self.audio_generation || self.audio.is_none() {
			return;
		}
		self.state.has_audio = false;
		self.state.notice = Some(format!("{NO_AUDIO}output stopped responding. Trying again…"));
		self.output_retry_at = Some(Instant::now() + AUDIO_RETRY);
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
		// In-flight opens/switches may finish after cancellation, sign-out or an engine restart.
		self.audio_generation = self.audio_generation.wrapping_add(1);
		self.audio_inflight = false;
		// The feed of the stopped engine is worth nothing to a retry.
		self.pending_feed = None;
		self.audio_retry_at = None;
		self.output_retry_at = None;
		Detached { audio: self.audio.take(), engine: self.engine.take() }
	}

	/// Drops the connection, the audio and the engine (in the background).
	pub(super) fn stop_engine(&mut self) {
		let detached = self.detach_engine();
		if detached.audio.is_none() && detached.engine.is_none() {
			return;
		}
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
		if !self.state.signed_in {
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
		let generation = self.account_generation;
		self.client = engine.connect(super::events::player_delivery(tx, wake, generation));
	}
	/// Revoke before resetting UI state; stale callbacks and engine events remain unauthorized.
	pub(super) fn cancel_sign_in(&mut self, timed_out: bool) {
		if !self.state.starting || self.state.signed_in {
			return;
		}
		self.sign_out();
		self.state.starting = false;
		self.state.notice = Some(if timed_out { "Sign-in timed out. Please try again.".into() } else { "Sign-in canceled.".into() });
	}

	/// Forgets the sign-in on this computer: the player stops, the library empties, and the setup
	/// page asks to sign in again.
	pub(super) fn sign_out(&mut self) {
		self.account_generation = self.account_generation.wrapping_add(1);
		self.start_authorization.cancel();
		self.start_authorization = crate::engine::StartAuthorization::default();
		self.state.recent_searches.clear();
		self.saver.send(Save::Searches(self.state.recent_searches.clone()));
		self.searches_dirty = false;
		self.stop_engine();
		self.library.forget(self.account_generation);
		let credential_error = crate::engine::forget_sign_in(&self.paths.config).err();
		self.state.disconnected("Signed out.".into());
		self.state.reset_account();
		self.state.recent_searches.load(&self.paths.config, None);
		self.state.notice = Some("Signed out.".into());
		if let Some(error) = credential_error {
			self.state.notice = Some(format!("Couldn't forget the sign-in: {error}"));
		}
		self.discard_start = false;
		self.start_failures = 0;
		self.player_offline = false;
		self.library_offline = false;
		self.library_retry_at = None;
		self.playback_interrupted = false;
		self.restore = super::restore::Restore::new(false);
		self.search_at = None;
		self.poll_at = None;
		self.poll_inflight = false;
		self.poll_forced = false;
		self.force_poll = false;
		self.poll_failures = 0;
		self.asked_artist = None;
		self.followed = None;
		self.last_item.clear();
	}

	/// A definitive subscription refusal is a sign-out, never an offline retry.
	pub(super) fn reject_sign_in(&mut self, error: crate::engine::SignInError) {
		self.sign_out();
		self.state.starting = false;
		self.state.notice = Some(error.message());
		self.state.sign_in_error = Some(error);
	}

	/// Repeated UI edits only move this deadline; encoding and writes run off-thread.
	pub(super) fn save_settings(&mut self) {
		self.settings_dirty = true;
		self.save_at = Some(Instant::now() + SAVE_DEBOUNCE);
	}

	pub(super) fn save_searches(&mut self) {
		self.searches_dirty = true;
		self.save_at = Some(Instant::now() + SAVE_DEBOUNCE);
	}

	pub(super) fn flush_preferences(&mut self) {
		self.save_at = None;
		if std::mem::take(&mut self.settings_dirty) {
			self.saver.send(Save::Settings(crate::app::settings::Settings {
				eq:                self.state.eq,
				eq_preset:         self.state.eq_preset.clone(),
				now_playing_view:  self.state.now_playing_view,
				sidebar_collapsed: self.state.sidebar_collapsed,
				prefs:             self.state.prefs.clone(),
			}));
		}
		if std::mem::take(&mut self.searches_dirty) {
			self.saver.send(Save::Searches(self.state.recent_searches.clone()));
		}
	}
}
/// What the player is started with.
struct Launch {
	name:     String,
	volume:   u8,
	cache_mb: u32,
	proxy:    String,
}

fn start_engine(
	paths: &Paths,
	launch: &Launch,
	writer: crate::audio::PcmWriter,
	progress: &mut dyn FnMut(&str),
	authorization: &crate::engine::StartAuthorization,
) -> Result<Engine, StartError> {
	let config = EngineConfig {
		device_name: launch.name.clone(),
		volume:      launch.volume,
		state_dir:   paths.config.clone(),
		cache_dir:   paths.cache.join("player"),
		cache_mb:    launch.cache_mb,
		proxy:       launch.proxy.clone(),
	};
	Engine::start(&config, writer, progress, authorization).map_err(|error| {
		if error.get_ref().is_some_and(|source| source.is::<crate::engine::SignInTimedOut>()) {
			return StartError::SignInTimedOut;
		}
		match error.get_ref().and_then(|source| source.downcast_ref::<crate::engine::SignInError>()) {
			Some(error) => StartError::Premium(error.clone()),
			None => StartError::Other(format!("Can't start the player: {error}")),
		}
	})
}
