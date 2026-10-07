//! Host-driven application controller. Workers report through a channel and wake the host;
//! the host dispatches messages, pumps events, and schedules the next deadline.

mod connect;
mod events;
mod lifecycle;
mod metadata;
mod queue;

use std::{
	sync::{Arc, mpsc},
	time::{Duration, Instant},
};

use crate::{
	app::{State, config::Paths, queue as queue_edit},
	engine::{Client, Engine, Incoming},
	library::{self, covers},
	sources::lyrics,
};

/// Overrides supplied by the application host.
#[derive(Debug, Default)]
pub struct Options {
	/// Overrides the persisted Spotify Connect device name for this launch.
	pub device_name: Option<String>,
}

enum AppEvent {
	Progress(String),
	EngineReady(Result<Engine, String>),
	Player(Incoming),
	Library(Box<library::Response>),
	Lyrics(lyrics::Response),
	Image { url: String, bytes: Vec<u8>, mime: String },
	ImageFailed(String),
	Devices,
	CacheSize(u64),
	CacheCleared(Result<(), String>),
}

const RETRY_START: Duration = Duration::from_secs(5);
const MAX_RESTARTS: u32 = 5;
const HEALTH_EVERY: Duration = Duration::from_secs(30);
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(350);
const AUDIO_RETRY: Duration = Duration::from_secs(5);
const NO_AUDIO: &str = "No audio output: ";
const CONTROL_POLL: Duration = Duration::from_millis(800);
const TRANSFER_POLL: Duration = Duration::from_millis(1200);
const SYSTEM_EVERY: Duration = Duration::from_secs(5);

/// Owns application state, background workers and the in-process player.
///
/// Keep this on the host's UI thread. A worker calls `wake` after publishing an event;
/// the callback must be thread-safe, nonblocking and only schedule host work. Call
/// [`Self::pump`] on a wake and at [`Self::deadline`], and redraw when it returns true.
/// Read [`Self::state`] to render, dispatch [`crate::Msg`] for user actions, and pump after
/// rendering to request covers looked up through [`State::image`]. Signals, window
/// management, decoding images and rendering belong to the host.
///
/// Dropping the controller tears playback down in dependency order: client, audio, engine.
/// Dropping it does not sign the user out.
pub struct Controller {
	paths:           Paths,
	state:           State,
	tx:              mpsc::Sender<AppEvent>,
	rx:              mpsc::Receiver<AppEvent>,
	wake:            Arc<dyn Fn() + Send + Sync>,
	engine:          Option<Engine>,
	audio:           Option<crate::audio::Player>,
	settings:        crate::app::settings::Settings,
	client:          Option<Client>,
	library:         library::Worker,
	images:          covers::Fetcher,
	lyrics:          Option<lyrics::Fetcher>,
	health_at:       Instant,
	search_at:       Option<Instant>,
	retry_start_at:  Option<Instant>,
	pending_feed:    Option<crate::audio::Feed>,
	audio_retry_at:  Option<Instant>,
	start_failures:  u32,
	asked_artists:   std::collections::HashSet<String>,
	discard_start:   bool,
	restarts:        u32,
	/// Last presented lyrics: a changed line invalidates the host's view.
	followed:        Option<(String, Option<usize>)>,
	last_item:       String,
	queue_reply:     bool,
	visible:         bool,
	poll_at:         Option<Instant>,
	poll_inflight:   bool,
	poll_forced:     bool,
	force_poll:      bool,
	poll_failures:   u32,
	output_retry_at: Option<Instant>,
	system_at:       Option<Instant>,
	_devices:        Option<crate::audio::DeviceWatch>,
}

impl Controller {
	/// Creates the controller and starts library/cover workers. A saved playback
	/// sign-in starts the engine asynchronously; otherwise [`crate::Msg::SignIn`] starts it.
	/// Lyrics are fetched lazily when their panel is opened.
	///
	/// Creates the config/cache directories, returning their filesystem errors as text.
	pub fn new(paths: &Paths, opts: Options, wake: Arc<dyn Fn() + Send + Sync>) -> Result<Self, String> {
		for dir in [&paths.config, &paths.cache] {
			std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
		}
		crate::net::trace::init();
		let (tx, rx) = mpsc::channel();
		let images = spawn_covers(paths, tx.clone(), wake.clone());
		let settings = crate::app::settings::Settings::load(&paths.config);
		crate::app::settings::allowed::set(settings.prefs.sources);
		let device_name = opts.device_name.clone().unwrap_or_else(|| settings.prefs.device.name.clone());
		let library = {
			let (tx, wake) = (tx.clone(), wake.clone());
			library::Worker::spawn(paths.config.clone(), &paths.cache, move |response| {
				if tx.send(AppEvent::Library(Box::new(response))).is_ok() {
					wake();
				}
			})
		};
		let state = State {
			signed_in: crate::engine::has_sign_in(&paths.config),
			device_name,
			sys: crate::app::system::System { name_override: opts.device_name, ..crate::app::system::System::default() },
			eq: settings.eq,
			eq_preset: settings.eq_preset.clone(),
			now_playing_view: settings.now_playing_view,
			sidebar_collapsed: settings.sidebar_collapsed,
			prefs: settings.prefs.clone(),
			device_input: settings.prefs.device.name.clone(),
			..State::default()
		};
		let mut controller = Self {
			paths: paths.clone(),
			state,
			tx,
			rx,
			wake,
			engine: None,
			audio: None,
			settings,
			client: None,
			library,
			images,
			lyrics: None,
			health_at: Instant::now() + HEALTH_EVERY,
			search_at: None,
			retry_start_at: None,
			pending_feed: None,
			audio_retry_at: None,
			start_failures: 0,
			asked_artists: std::collections::HashSet::new(),
			discard_start: false,
			restarts: 0,
			followed: None,
			last_item: String::new(),
			queue_reply: false,
			visible: true,
			poll_at: None,
			poll_inflight: false,
			poll_forced: false,
			force_poll: false,
			poll_failures: 0,
			output_retry_at: None,
			system_at: None,
			_devices: None,
		};
		controller.initialize_system();
		if controller.state.signed_in {
			controller.start_engine();
		}
		Ok(controller)
	}

	/// The current state for rendering. Missing cover lookups are recorded by
	/// [`State::image`] and sent to the fetcher on the next pump.
	pub fn state(&self) -> &State {
		&self.state
	}

	/// Drains worker events, runs due timers and reconciles playback polling.
	/// Returns true when the host should redraw, including a new current lyric line.
	/// This never reads host input, blocks waiting for an event or renders a view.
	pub fn pump(&mut self) -> bool {
		let mut changed = false;
		while let Ok(event) = self.rx.try_recv() {
			changed |= self.on_event(event);
		}
		changed |= self.run_timers();
		if self.poll_at.is_some_and(|at| Instant::now() >= at) {
			self.poll_at = None;
			self.poll_playback();
		}
		self.reconcile_poll();
		changed |= self.request_lyrics();
		changed |= self.follow_lyrics();
		if self.state.focus.is_some_and(|field| !self.state.field_visible(field)) {
			self.state.focus = None;
			changed = true;
		}
		for url in self.state.take_missing_images() {
			self.images.request(url);
		}
		if crate::net::trace::enabled() {
			crate::net::trace::mark_if_changed("ready", &self.state.library.ready_label());
			crate::net::trace::mark_if_changed("covers", &self.images.outstanding().to_string());
		}
		changed
	}

	/// The earliest pending timer, or `None` when only a worker wake/user action
	/// can require work. Sleep until this instant or a wake, then call [`Self::pump`].
	pub fn deadline(&self) -> Option<Instant> {
		[
			self.engine.as_ref().map(|_| self.health_at),
			self.search_at,
			self.retry_start_at,
			self.audio_retry_at,
			self.visible.then(|| self.state.next_lyric_at()).flatten(),
			self.poll_at,
			self.state.queue_edit.deadline(),
			self.state.rules_deadline(),
			self.output_retry_at,
			self.system_at,
		]
		.into_iter()
		.flatten()
		.min()
	}

	fn run_timers(&mut self) -> bool {
		let now = Instant::now();
		let mut changed = false;
		if self.audio_retry_at.is_some_and(|at| now >= at) {
			self.audio_retry_at = None;
			if let Some(feed) = self.pending_feed.take() {
				self.start_audio(feed);
				changed = true;
			}
		}
		if self.retry_start_at.is_some_and(|at| now >= at) {
			self.retry_start_at = None;
			self.start_engine();
			changed = true;
		}
		if self.search_at.is_some_and(|at| now >= at) {
			self.search_at = None;
			let query = self.state.library.query.trim().to_owned();
			if !query.is_empty() {
				self.library.send(library::Request::Search(query));
			}
		}
		match self.state.queue_edit.due(now) {
			queue_edit::Due::Nothing => {},
			queue_edit::Due::Rebuild => changed |= self.rebuild_queue(),
			queue_edit::Due::GiveUp => changed |= self.give_up_queue(),
		}
		if self.state.rules_deadline().is_some_and(|at| now >= at) {
			for command in self.state.rules_due(now) {
				self.send_player(command);
			}
			changed = true;
		}
		if self.output_retry_at.is_some_and(|at| now >= at) {
			self.output_retry_at = None;
			self.reconcile_output();
			changed = true;
		}
		if self.system_at.is_some_and(|at| now >= at) {
			self.system_at = None;
			self.refresh_memory();
			changed = true;
		}
		if self.engine.is_some() && now >= self.health_at {
			self.health_at = now + HEALTH_EVERY;
			changed |= self.check_engine();
		}
		changed
	}
}

impl Drop for Controller {
	fn drop(&mut self) {
		self.client = None;
		self.audio = None;
		self.engine = None;
	}
}

/// Appends a line to `$TMPDIR/turntable-debug.log` when `TURNTABLE_DEBUG` is set.
fn debug_log(line: &str) {
	use std::io::Write;
	if std::env::var_os("TURNTABLE_DEBUG").is_none() {
		return;
	}
	let path = std::env::temp_dir().join("turntable-debug.log");
	if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
		let _ = writeln!(file, "{line}");
	}
}

fn spawn_covers(paths: &Paths, tx: mpsc::Sender<AppEvent>, wake: Arc<dyn Fn() + Send + Sync>) -> covers::Fetcher {
	covers::Fetcher::spawn(paths.cache.join("covers"), move |cover| {
		let event = match cover {
			covers::Cover::Loaded { url, bytes, mime } => AppEvent::Image { url, bytes, mime },
			covers::Cover::Failed(url) => AppEvent::ImageFailed(url),
		};
		if tx.send(event).is_ok() {
			wake();
		}
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		app::{Anchor, LyricsState, Panel},
		sources::external::Lyrics,
	};

	#[test]
	fn deadlines_pick_the_earliest_timer_and_hidden_lyrics_do_not_tick() {
		let root = std::env::temp_dir().join(format!("turntable-deadlines-{}", std::process::id()));
		let _ = std::fs::remove_dir_all(&root);
		let paths = Paths { config: root.join("config"), cache: root.join("cache") };
		let mut controller = Controller::new(&paths, Options::default(), Arc::new(|| {})).unwrap();
		controller.state.item.uri = "spotify:track:a".into();
		controller.state.panel = Some(Panel::Lyrics);
		let now = Instant::now();
		controller.state.anchor = Anchor { position_ms: 0, at: now, speed: 1.0 };
		controller.state.lyrics = Some(("spotify:track:a".into(), LyricsState::Found(Lyrics::Synced(vec![(10_000, "line".into())]))));
		assert_eq!(controller.deadline(), Some(now + Duration::from_secs(10)));
		controller.search_at = Some(now + Duration::from_secs(3));
		controller.poll_at = Some(now + Duration::from_secs(2));
		assert_eq!(controller.deadline(), controller.poll_at);
		controller.set_visible(false);
		assert!(controller.poll_at.is_none());
		assert_eq!(controller.deadline(), controller.search_at);
		controller.search_at = None;
		assert!(controller.deadline().is_none());
		drop(controller);
		std::fs::remove_dir_all(root).unwrap();
	}
}
