//! The player: one Tern screen surface, in-process playback and library events in, commands out.
//!
//! Threads: this one (the TSP loop, sleeping in the session's poll), the librespot runtime,
//! the library (Web API) worker, an artwork fetcher, a lyrics fetcher (started when lyrics
//! are first asked for), a Unix signal waiter, and a one-shot engine starter.
//! Workers sleep until woken; the UI loop also handles its timers.

mod connect;
mod events;
mod input;
mod lifecycle;
mod queue;
mod terminal;
mod view;

use std::{
	sync::mpsc,
	time::{Duration, Instant},
};

use tern_sdk::{Event as TspEvent, Input, Session, Surface, SurfaceOptions, Waker};

#[cfg(unix)]
use self::terminal::{block_signals, spawn_signal_waiter};
use self::{
	input::remote_msg,
	terminal::pane_size,
	view::{Followed, LyricsKey, SheetKey},
};
use crate::{
	app::{Field, Msg, State, config::Paths, queue as queue_edit},
	engine::{Client, Engine, Incoming},
	library::{
		self,
		covers::{self, Cover},
	},
	sources::lyrics,
	ui,
	ui::icons::Icons,
};

pub struct Options {
	/// A name given on the command line (`--device-name`), which wins over the setting.
	pub device_name: Option<String>,
}

enum AppEvent {
	Progress(String),
	EngineReady(Result<Engine, String>),
	Player(Incoming),
	Library(Box<library::Response>),
	Lyrics(lyrics::Response),
	#[cfg(unix)]
	Resized,
	#[cfg(unix)]
	/// SIGHUP/SIGTERM/SIGINT: leave through the normal teardown.
	Quit,
	Image {
		url:   String,
		bytes: Vec<u8>,
		mime:  String,
	},
	ImageFailed(String),
	/// Output devices came or went, or the system's default changed.
	Devices,
	/// The player's playback cache holds this many bytes.
	CacheSize(u64),
	/// The playback cache was emptied (or couldn't be).
	CacheCleared(Result<(), String>),
}

/// First retry delay after a failed engine start (doubles, up to 5 min).
const RETRY_START: Duration = Duration::from_secs(5);
/// Engine deaths after start that are restarted before giving up.
const MAX_RESTARTS: u32 = 5;
/// How often the engine releases free heap and is checked for having stopped.
const HEALTH_EVERY: Duration = Duration::from_secs(30);
/// Typing must pause this long before a search goes out (Web API quota).
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(350);
/// How often a failed audio output start is tried again.
const AUDIO_RETRY: Duration = Duration::from_secs(5);
/// The notice shown while no output starts (cleared once one does).
const NO_AUDIO: &str = "No audio output: ";
/// How soon a poll follows a command sent to another device, and a transfer to one.
const CONTROL_POLL: Duration = Duration::from_millis(800);
const TRANSFER_POLL: Duration = Duration::from_millis(1200);
/// How often memory is looked at again while the settings page shows it.
const SYSTEM_EVERY: Duration = Duration::from_secs(5);

pub fn run(paths: &Paths, opts: &Options) -> Result<(), String> {
	#[cfg(unix)]
	// Before any thread exists, so every thread inherits the mask and only
	// the waiter thread ever takes these signals.
	let signals = block_signals();
	let options = tern_sdk::Options::new().app("terntable").version(env!("CARGO_PKG_VERSION")).feature("edit");
	let Some(mut session) = Session::<Msg>::connect(options).map_err(|e| e.to_string())? else {
		return Err("terntable draws its UI natively in Tern; run it in a Tern pane".into());
	};
	// Windows receives resize and Ctrl+C through the SDK; closing the pane closes its input,
	// and Session::next returns InputClosed, which also goes through Player's normal Drop.
	let result = Player::new(
		&mut session,
		paths,
		opts,
		#[cfg(unix)]
		signals,
	)
	.and_then(|mut p| p.serve());
	let closed = session.close().map_err(|e| e.to_string());
	result.and(closed)
}

struct Player<'a> {
	session:         &'a mut Session<Msg>,
	sf:              Surface,
	icons:           Icons,
	paths:           Paths,
	opts:            &'a Options,
	state:           State,
	tx:              mpsc::Sender<AppEvent>,
	rx:              mpsc::Receiver<AppEvent>,
	waker:           Waker,
	engine:          Option<Engine>,
	/// Plays the engine's PCM through the EQ and output stage.
	audio:           Option<crate::audio::Player>,
	settings:        crate::app::settings::Settings,
	client:          Option<Client>,
	library:         library::Worker,
	images:          covers::Fetcher,
	/// Looks lyrics up; started the first time they're asked for.
	lyrics:          Option<lyrics::Fetcher>,
	health_at:       Instant,
	/// The pane width Tern last reported (authoritative: the pty can lag).
	tsp_cols:        Option<u32>,
	/// When to send the pending search (typing paused).
	search_at:       Option<Instant>,
	/// When to try starting the engine again after it failed to start.
	retry_start_at:  Option<Instant>,
	/// The audio path's feed while no output device would start, and when to try again.
	pending_feed:    Option<crate::audio::Feed>,
	audio_retry_at:  Option<Instant>,
	start_failures:  u32,
	/// Artist ids asked for, so a failing one isn't asked for again each track.
	asked_artists:   std::collections::HashSet<String>,
	/// An engine start in flight was superseded by a restart or sign-out.
	discard_start:   bool,
	/// Engine deaths after a successful start, this session.
	restarts:        u32,
	/// What the dynamic sheet was last built from; it's re-sent only when this changes
	/// (re-sending mid-track would restart the seek bar's animation needlessly).
	sheet_key:       Option<SheetKey>,
	/// What the lyrics sheet was last built from, and which keyframe names it used.
	lyrics_key:      Option<LyricsKey>,
	lyrics_flip:     bool,
	/// Which of the seek bar's two keyframe names the dynamic sheet uses.
	seek_flip:       bool,
	/// The lyric line last scrolled to.
	followed:        Option<Followed>,
	focused:         Option<Field>,
	/// The pane title last sent.
	title:           String,
	/// The now-playing URI as of the last player event (to spot track changes).
	last_item:       String,
	/// A queue request is out: the next queue event answers it, rather than announcing a change.
	queue_reply:     bool,
	/// Tern says the pane shows (nothing polls while it doesn't).
	visible:         bool,
	/// When to ask what plays on the other device next.
	poll_at:         Option<Instant>,
	poll_inflight:   bool,
	/// The poll in flight was sent after a transfer, so it outranks the local player (see `State::apply_playback`).
	poll_forced:     bool,
	/// The next poll follows a transfer.
	force_poll:      bool,
	/// Failed polls in a row (they back off).
	poll_failures:   u32,
	/// When to try opening the chosen output device again.
	output_retry_at: Option<Instant>,
	/// When to look at memory again (while the settings page shows it).
	system_at:       Option<Instant>,
	/// Told when output devices come and go.
	_devices:        Option<crate::audio::DeviceWatch>,
}

#[expect(
	clippy::needless_pass_by_value,
	reason = "Result::map_err passes its error by value; this callback converts it to the UI's error text"
)]
fn tsp(e: tern_sdk::Error) -> String {
	e.to_string()
}

impl<'a> Player<'a> {
	fn new(session: &'a mut Session<Msg>, paths: &Paths, opts: &'a Options, #[cfg(unix)] signals: terminal::Signals) -> Result<Self, String> {
		let sf = session.open(SurfaceOptions::screen().title("terntable").role("terntable.player")).map_err(tsp)?;
		session.stylesheet(sf, "tt", Some(ui::SHEET)).map_err(tsp)?;
		let icons = Icons::send(session, sf).map_err(tsp)?;
		let waker = session.waker().ok_or("this terminal transport can't be woken")?;
		let (tx, rx) = mpsc::channel::<AppEvent>();
		#[cfg(unix)]
		spawn_signal_waiter(signals, tx.clone(), waker.clone());
		let images = spawn_covers(paths, tx.clone(), waker.clone());
		let settings = crate::app::settings::Settings::load(&paths.config);
		// What the user's choices allow, before any worker asks for anything.
		crate::app::settings::allowed::set(settings.prefs.sources);
		let device_name = opts.device_name.clone().unwrap_or_else(|| settings.prefs.device.name.clone());
		let library = {
			let (tx, waker) = (tx.clone(), waker.clone());
			library::Worker::spawn(paths.config.clone(), paths.cache.clone(), move |r| {
				if tx.send(AppEvent::Library(Box::new(r))).is_ok() {
					waker.wake();
				}
			})
		};
		let tsp_cols = Some(session.caps().cols).filter(|c| *c > 0);
		let (cols, rows) = pane_size(tsp_cols);
		let cell = session.caps().cell.unwrap_or_default();
		let state = State {
			rows,
			cols,
			cell_w: cell.w,
			cell_h: cell.h,
			signed_in: crate::engine::has_sign_in(&paths.config),
			device_name,
			eq: settings.eq,
			eq_preset: settings.eq_preset.clone(),
			now_playing_view: settings.now_playing_view,
			sidebar_collapsed: settings.sidebar_collapsed,
			prefs: settings.prefs.clone(),
			device_input: settings.prefs.device.name.clone(),
			..State::default()
		};
		let mut player = Self {
			session,
			sf,
			icons,
			paths: paths.clone(),
			opts,
			state,
			tx,
			rx,
			waker,
			engine: None,
			audio: None,
			settings,
			client: None,
			library,
			images,
			lyrics: None,
			health_at: Instant::now() + HEALTH_EVERY,
			tsp_cols,
			search_at: None,
			retry_start_at: None,
			pending_feed: None,
			audio_retry_at: None,
			start_failures: 0,
			discard_start: false,
			asked_artists: std::collections::HashSet::new(),
			restarts: 0,
			sheet_key: None,
			lyrics_key: None,
			lyrics_flip: false,
			seek_flip: false,
			followed: None,
			focused: None,
			title: String::new(),
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
		player.initialize_system();
		if player.state.signed_in {
			player.start_engine();
		}
		Ok(player)
	}

	fn serve(&mut self) -> Result<(), String> {
		let mut dirty = true;
		while !self.state.quit {
			if dirty {
				self.request_lyrics();
				self.render()?;
				dirty = false;
				// A render builds and drops the whole view: give back what it leaves free.
				crate::net::trim_heap();
			}
			self.follow_lyrics()?;
			// Sleep until the earliest timer: engine health check, debounced search, next lyric line.
			let health = self.engine.as_ref().map(|_| self.health_at);
			let lyric = self.state.next_lyric_at();
			let queue_at = self.state.queue_edit.deadline();
			let rules_at = self.state.rules_deadline();
			let next = [
				health,
				self.search_at,
				self.retry_start_at,
				self.audio_retry_at,
				lyric,
				self.poll_at,
				queue_at,
				rules_at,
				self.output_retry_at,
				self.system_at,
			]
			.into_iter()
			.flatten()
			.min();
			let timeout = next.map(|at| at.saturating_duration_since(Instant::now()));
			match self.session.next(timeout).map_err(tsp)? {
				Some(Input::Msg(msg, _)) => dirty |= self.dispatch(msg),
				Some(Input::Key(key)) => {
					if let Some(msg) = self.key_msg(&key) {
						dirty |= self.dispatch(msg);
					}
				},
				Some(Input::Event(TspEvent::Resize(r))) => {
					self.tsp_cols = Some(r.cols).filter(|c| *c > 0);
					(self.state.cols, self.state.rows) = pane_size(self.tsp_cols);
					if let Some(cell) = r.cell {
						(self.state.cell_w, self.state.cell_h) = (cell.w, cell.h);
					}
					if let Some(visible) = r.visible {
						self.set_visible(visible);
					}
					dirty = true;
				},
				Some(Input::Event(TspEvent::Visible(v))) => self.set_visible(v.visible),
				Some(Input::Event(TspEvent::Action(a))) => {
					// Remote control from the Tern plugin (global key binds, palette).
					if let Some(msg) = remote_msg(&a.act) {
						dirty |= self.dispatch(msg);
					}
				},
				Some(Input::Event(TspEvent::Error(e))) => debug_log(&format!("tern error: {}", serde_json::Value::Object(e.raw.clone()))),
				Some(_) | None => {},
			}
			dirty |= self.run_timers();
			while let Ok(event) = self.rx.try_recv() {
				dirty |= self.on_event(event)?;
			}
			if self.poll_at.is_some_and(|at| Instant::now() >= at) {
				self.poll_at = None;
				self.poll_playback();
			}
			self.reconcile_poll();
		}
		Ok(())
	}

	fn run_timers(&mut self) -> bool {
		let mut dirty = false;
		if self.audio_retry_at.is_some_and(|at| Instant::now() >= at) {
			self.audio_retry_at = None;
			if let Some(feed) = self.pending_feed.take() {
				self.start_audio(feed);
				dirty = true;
			}
		}
		if self.retry_start_at.is_some_and(|at| Instant::now() >= at) {
			self.retry_start_at = None;
			self.start_engine();
			dirty = true;
		}
		if self.search_at.is_some_and(|at| Instant::now() >= at) {
			self.search_at = None;
			let query = self.state.library.query.trim().to_owned();
			if !query.is_empty() {
				self.library.send(library::Request::Search(query));
			}
		}
		match self.state.queue_edit.due(Instant::now()) {
			queue_edit::Due::Nothing => {},
			queue_edit::Due::Rebuild => dirty |= self.rebuild_queue(),
			queue_edit::Due::GiveUp => dirty |= self.give_up_queue(),
		}
		if self.state.rules_deadline().is_some_and(|at| Instant::now() >= at) {
			for cmd in self.state.rules_due(Instant::now()) {
				self.send_player(cmd);
			}
			dirty = true;
		}
		if self.output_retry_at.is_some_and(|at| Instant::now() >= at) {
			self.output_retry_at = None;
			self.reconcile_output();
			dirty = true;
		}
		if self.system_at.is_some_and(|at| Instant::now() >= at) {
			self.system_at = None;
			self.refresh_memory();
			dirty = true;
		}
		if self.engine.is_some() && Instant::now() >= self.health_at {
			self.health_at = Instant::now() + HEALTH_EVERY;
			dirty |= self.check_engine();
		}
		dirty
	}
}

impl Drop for Player<'_> {
	/// Tears down in dependency order on every exit path, errors included: the client first,
	/// then the audio reader, then the in-process engine.
	fn drop(&mut self) {
		self.client = None;
		self.audio = None;
		self.engine = None;
	}
}

/// Appends a line to `$TMPDIR/terntable-debug.log` when `TERNTABLE_DEBUG` is set.
fn debug_log(line: &str) {
	use std::io::Write;
	if std::env::var_os("TERNTABLE_DEBUG").is_none() {
		return;
	}
	let path = std::env::temp_dir().join("terntable-debug.log");
	if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
		let _ = writeln!(f, "{line}");
	}
}

fn spawn_covers(paths: &Paths, tx: mpsc::Sender<AppEvent>, waker: Waker) -> covers::Fetcher {
	covers::Fetcher::spawn(paths.cache.join("covers"), move |cover| {
		let event = match cover {
			Cover::Loaded { url, bytes, mime } => AppEvent::Image { url, bytes, mime },
			Cover::Failed(url) => AppEvent::ImageFailed(url),
		};
		if tx.send(event).is_ok() {
			waker.wake();
		}
	})
}
