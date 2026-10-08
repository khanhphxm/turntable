//! The player: a Spotify Connect device (librespot) running in this process.
//!
//! [`Engine::start`] signs in, registers the device and decodes into the audio path's
//! [`PcmWriter`]; [`Engine::connect`] hands out the [`Client`] the UI drives it with. What the
//! player does comes back as the [`Incoming`] events the UI already speaks: librespot's player
//! events, plus the connect state (context, options, upcoming tracks) the vendored
//! `librespot-connect` sends after every change.
//!
//! Natural playback overlaps leave adjacent tracks in unshuffled albums gapless;
//! other eligible transitions use the core audio mixer. Manual transport commands cut
//! immediately; every PCM stream passes through the same EQ/leveler/output.

mod connect;
mod load;
mod metadata;
mod model;
mod oauth;
mod premium;
#[cfg(test)]
mod session_tests;
mod start;
mod transition;

use std::{
	fs, io,
	path::PathBuf,
	sync::{
		Arc,
		atomic::{AtomicBool, Ordering},
	},
	time::{Duration, SystemTime, UNIX_EPOCH},
};

pub(crate) use connect::device_of;
use librespot_connect::{ConnectConfig, LoadRequest, LoadRequestOptions, PlaybackCommand, Spirc, StateSnapshot};
use librespot_core::{
	SpotifyUri,
	authentication::Credentials,
	cache::Cache,
	config::{DeviceType, SessionConfig},
	dealer::protocol::TransferOptions,
	session::Session,
	spclient::TransferRequest,
};
use librespot_playback::{
	config::{Bitrate, PlayerConfig},
	mixer::{Mixer, MixerConfig, softmixer::SoftMixer},
	player::{Player as Decoder, PlayerEvent},
};
pub use model::*;
pub(crate) use oauth::SIGN_IN_TIMEOUT;
pub use premium::SignInError;
pub(crate) use start::SignInTimedOut;
pub use start::StartAuthorization;
use tokio::{
	runtime::Runtime,
	sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
};
use transition::{RingSink, Transitions};

type SharedCluster = tokio::sync::watch::Sender<Option<Arc<librespot_protocol::connect::Cluster>>>;
use crate::audio::PcmWriter;

/// What the player is started with.
#[derive(Debug, Clone)]
pub struct EngineConfig {
	/// The Connect device name.
	pub device_name: String,
	/// The volume it starts at (0 to 100).
	pub volume:      u8,
	/// Where the sign-in and the device id are kept.
	pub state_dir:   PathBuf,
	/// Where audio files are cached.
	pub cache_dir:   PathBuf,
	/// The audio cache's size limit; 0 is no cache at all.
	pub cache_mb:    u32,
	/// HTTP CONNECT proxy; empty preserves environment defaults.
	pub proxy:       String,
}

/// The browser sign-in's registered loopback redirect.
const OAUTH_REDIRECT: &str = "http://127.0.0.1:5588/login";
/// What the sign-in asks for: playback, and what the library pages read and change.
const OAUTH_SCOPES: &[&str] = &[
	"streaming",
	"app-remote-control",
	"playlist-modify-private",
	"playlist-modify-public",
	"playlist-read-collaborative",
	"playlist-read-private",
	"ugc-image-upload",
	"user-follow-modify",
	"user-follow-read",
	"user-library-modify",
	"user-library-read",
	"user-modify-playback-state",
	"user-read-currently-playing",
	"user-read-email",
	"user-read-playback-position",
	"user-read-playback-state",
	"user-read-private",
	"user-read-recently-played",
];
/// How long commands wait for a takeover (transfer) to land before this device starts on its own.
const TAKEOVER_WAIT: Duration = Duration::from_secs(4);
/// Bounds connection and session reads even while librespot is retrying a dead network.
const NETWORK_TIMEOUT: Duration = Duration::from_secs(20);

pub struct Engine {
	/// Taken only by `Drop`, to shut it down by value.
	runtime:     Option<Runtime>,
	session:     Session,
	spirc:       Arc<Spirc>,
	streams:     Option<Streams>,
	/// Spirc's task has ended (signed out elsewhere, the session dropped).
	done:        Arc<AtomicBool>,
	stop:        tokio::sync::watch::Sender<bool>,
	cluster:     SharedCluster,
	name:        String,
	volume:      u8,
	transitions: Arc<Transitions>,
}

/// A handle for library and remote Connect I/O on the player's authenticated session/runtime.
/// Workers share its pushed cluster; it never creates another session or implicitly starts playback.
#[derive(Clone)]
pub struct SessionReader {
	session: Session,
	runtime: tokio::runtime::Handle,
	stop:    tokio::sync::watch::Receiver<bool>,
	cluster: SharedCluster,
}

impl SessionReader {
	/// Shares an already authenticated session without registering a Connect device or starting playback.
	/// The caller owns the runtime and keeps the stop sender alive; sending `true` cancels library I/O.
	pub fn from_session(session: Session, runtime: tokio::runtime::Handle, stop: tokio::sync::watch::Receiver<bool>) -> Self {
		Self { session, runtime, stop, cluster: tokio::sync::watch::channel(None).0 }
	}

	pub fn session(&self) -> &Session {
		&self.session
	}

	pub(crate) fn cluster(&self) -> Option<Arc<librespot_protocol::connect::Cluster>> {
		if *self.stop.borrow() || self.session.is_invalid() {
			return None;
		}
		self.cluster.borrow().clone()
	}

	#[cfg(test)]
	pub(crate) fn set_cluster(&self, cluster: librespot_protocol::connect::Cluster) {
		self.cluster.send_replace(Some(Arc::new(cluster)));
	}

	/// Runs session I/O on a blocking worker. Engine shutdown cancels reads before
	/// its runtime stops, so reconnecting never leaves a worker polling dead timers.
	pub fn block_on<F, T, E>(&self, future: F) -> Result<T, E>
	where
		F: Future<Output = Result<T, E>>,
		E: From<librespot_core::Error>,
	{
		let mut stop = self.stop.clone();
		if *stop.borrow() {
			return Err(librespot_core::Error::unavailable("player session stopped").into());
		}
		self.runtime.block_on(async {
			tokio::select! {
				biased;
				_ = stop.changed() => Err(librespot_core::Error::unavailable("player session stopped").into()),
				result = network_timeout(NETWORK_TIMEOUT, future) => result,
			}
		})
	}
}

/// What the engine reports, until a client takes it.
struct Streams {
	player:    UnboundedReceiver<PlayerEvent>,
	snapshots: UnboundedReceiver<StateSnapshot>,
}

impl Engine {
	/// Signs in (with the saved sign-in, else in the browser), registers the Connect device and
	/// starts decoding into `writer`. Blocks until the device is online.
	/// Credential writes are serialized with `authorization.cancel()`, so an unfinished
	/// browser/Spirc authentication can never restore credentials after sign-out.
	pub fn start(
		config: &EngineConfig,
		writer: PcmWriter,
		progress: &mut dyn FnMut(&str),
		authorization: &StartAuthorization,
	) -> io::Result<Self> {
		authorization.check()?;
		fs::create_dir_all(&config.state_dir)?;
		let runtime = tokio::runtime::Builder::new_multi_thread()
			.worker_threads(1)
			.thread_name("librespot")
			// Login/Connect's protobuf state can exceed 256 KiB, even before playback starts.
			.thread_stack_size(512 << 10)
			.enable_all()
			.build()?;
		let audio = (config.cache_mb > 0).then(|| config.cache_dir.join("files"));
		let limit = (config.cache_mb > 0).then(|| u64::from(config.cache_mb) << 20);
		let mut cache = Cache::new(Some(&config.state_dir), Some(&config.state_dir), audio.as_ref(), limit).map_err(io::Error::other)?;
		authorization.guard_cache(&mut cache, &config.state_dir);
		let session_config = SessionConfig {
			device_id: device_id(&config.state_dir)?,
			proxy: crate::net::proxy::session(&config.proxy).map_err(io::Error::other)?,
			..SessionConfig::default()
		};
		let credentials = match cache.credentials() {
			Some(c) => c,
			None => {
				progress("Sign in to Spotify in your browser…");
				oauth::sign_in(&session_config.client_id, &runtime, authorization, &config.proxy)?
			},
		};
		authorization.check()?;
		progress("Connecting to Spotify…");
		// librespot spawns its tasks on the ambient runtime.
		let _context = runtime.enter();
		let session = Session::new(session_config, Some(cache));
		let mixer: Arc<dyn Mixer> = Arc::new(SoftMixer::open(MixerConfig::default()).map_err(io::Error::other)?);
		let transitions = Arc::new(Transitions::default());
		let sink_transitions = transitions.clone();
		let output_volume = mixer.get_soft_volume();
		let player_config = PlayerConfig { bitrate: Bitrate::Bitrate320, ..PlayerConfig::default() };
		let decoder = Decoder::new(player_config, session.clone(), mixer.get_soft_volume(), move || {
			Box::new(RingSink::new(writer, sink_transitions, output_volume))
		});
		let player = decoder.get_player_event_channel();
		if crate::net::trace::enabled() {
			let mut observed = decoder.get_player_event_channel();
			runtime.spawn(async move {
				while let Some(event) = observed.recv().await {
					trace_player_event(&event, "player-event");
				}
			});
		}
		let (snap_tx, snapshots) = mpsc::unbounded_channel();
		let connect = ConnectConfig {
			name: config.device_name.clone(),
			device_type: DeviceType::Computer,
			initial_volume: to_u16(config.volume),
			..ConnectConfig::default()
		};
		let account = credentials.username.clone();
		let result = runtime.block_on(async {
			tokio::try_join!(
				async {
					network_timeout(NETWORK_TIMEOUT, Spirc::new(connect, session.clone(), credentials, decoder, mixer, Some(snap_tx)))
						.await
						.map_err(|error| premium::connection_error(error, &session, account.as_deref()))
				},
				premium::wait(&session, account.as_deref()),
			)
		});
		let ((spirc, task), ()) = match result {
			Ok(ready) => ready,
			Err(error) => {
				session.shutdown();
				return Err(error);
			},
		};
		if let Err(error) = authorization.check() {
			session.shutdown();
			return Err(error);
		}
		let done = Arc::new(AtomicBool::new(false));
		let ended = done.clone();
		let (stop, _) = tokio::sync::watch::channel(false);
		let stopped = stop.clone();
		runtime.spawn(async move {
			task.await;
			ended.store(true, Ordering::Release);
			stopped.send_replace(true);
		});
		Ok(Self {
			runtime: Some(runtime),
			session,
			spirc: Arc::new(spirc),
			streams: Some(Streams { player, snapshots }),
			done,
			stop,
			name: config.device_name.clone(),
			cluster: tokio::sync::watch::channel(None).0,
			volume: config.volume,
			transitions,
		})
	}

	/// The client the UI drives the player with; `deliver` gets every event, on the engine's
	/// thread. One client per engine: a second call returns `None`.
	pub fn connect(&mut self, deliver: impl FnMut(Incoming) + Send + 'static) -> Option<Client> {
		let streams = self.streams.take()?;
		let (tx, requests) = mpsc::unbounded_channel();
		let runtime = self.runtime.as_ref()?;
		let _context = runtime.enter();
		let (metadata, enriched) = metadata::spawn(self.session.clone(), self.transitions.clone());
		let forward = Forward {
			spirc: self.spirc.clone(),
			deferred: Vec::new(),
			takeover_by: None,
			takeover_id: 0,
			load_id: 0,
			loading_request: None,
			session: self.session.clone(),
			deliver: Box::new(deliver),
			metadata,
			requests: tx.clone(),
			state: Tracker::default(),
			queue_generation: 0,
			transitions: self.transitions.clone(),
			cluster: self.cluster.clone(),
		};
		let (name, volume) = (self.name.clone(), self.volume);
		runtime.spawn(forward.run(streams, requests, enriched, name, volume));
		Some(Client { requests: tx })
	}

	/// Shares authenticated library I/O; stopping the engine cancels its in-flight requests.
	pub fn session_reader(&self) -> Option<SessionReader> {
		Some(SessionReader {
			session: self.session.clone(),
			runtime: self.runtime.as_ref()?.handle().clone(),
			stop:    self.stop.subscribe(),
			cluster: self.cluster.clone(),
		})
	}

	/// Whether the player still runs and its session remains valid.
	pub fn alive(&mut self) -> bool {
		!self.done.load(Ordering::Acquire) && !self.session.is_invalid()
	}

	/// Cancels library reads and asks Spirc to stop before background resource teardown.
	pub(crate) fn stop(&self) {
		self.stop.send_replace(true);
		let _ = self.spirc.shutdown();
	}
}

/// Dropping a timed-out future cancels its I/O; the caller may establish a fresh session.
async fn network_timeout<F, T, E>(timeout: Duration, future: F) -> Result<T, E>
where
	F: Future<Output = Result<T, E>>,
	E: From<librespot_core::Error>,
{
	tokio::time::timeout(timeout, future)
		.await
		.unwrap_or_else(|_| Err(librespot_core::Error::unavailable("Spotify connection timed out; check your network").into()))
}

impl Drop for Engine {
	fn drop(&mut self) {
		self.stop();
		// Give the shutdown a moment to tell Spotify the device left.
		if let Some(runtime) = self.runtime.take() {
			runtime.shutdown_timeout(Duration::from_millis(500));
		}
	}
}

/// The file librespot keeps the sign-in in (in the state directory).
const CREDENTIALS: &str = "credentials.json";

/// Whether a sign-in is saved in `dir` (the player starts without asking).
pub fn has_sign_in(dir: &std::path::Path) -> bool {
	dir.join(CREDENTIALS).is_file()
}

/// Deletes the saved sign-in.
pub fn forget_sign_in(dir: &std::path::Path) -> io::Result<()> {
	match fs::remove_file(dir.join(CREDENTIALS)) {
		Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
		_ => Ok(()),
	}
}

/// A stable device id across launches, so Spotify sees one device rather than a new one each time.
fn device_id(dir: &std::path::Path) -> io::Result<String> {
	let path = dir.join("device_id");
	if let Ok(id) = fs::read_to_string(&path)
		&& !id.trim().is_empty()
	{
		return Ok(id.trim().to_owned());
	}
	let id = SessionConfig::default().device_id;
	crate::app::config::write_private(&path, id.as_bytes())?;
	Ok(id)
}

/// Percent (0 to 100) to librespot's volume (0 to 65535).
fn to_u16(percent: u8) -> u16 {
	u16::try_from(u32::from(percent.min(100)) * 65_535 / 100).unwrap_or(u16::MAX)
}

/// librespot's volume to percent.
fn to_percent(volume: u16) -> u8 {
	u8::try_from((u32::from(volume) * 100 + 32_767) / 65_535).unwrap_or(100)
}

/// Sleeps until `at`; never returns without one (the caller only polls it with a deadline set).
async fn sleep_until(at: Option<tokio::time::Instant>) {
	match at {
		Some(at) => tokio::time::sleep_until(at).await,
		None => std::future::pending().await,
	}
}

fn now_ms() -> f64 {
	SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

// -------------------------------------------------------------------- client

/// Drives the player. Every call queues and returns at once.
pub struct Client {
	requests: UnboundedSender<Request>,
}

/// Commands and asynchronous failures share the engine's immediate event loop.
enum Request {
	Command { command: Command, id: u64 },
	Failed { id: u64, message: String },
}

impl Client {
	/// Queues a command, tagged with the host's optimistic playback revision (0 if untracked).
	/// A closed engine stream is reported synchronously; dispatch/load failures arrive as events.
	pub fn send(&self, command: Command, id: u64) -> io::Result<()> {
		crate::net::trace::playback("command", command.name(), None);
		self.requests.send(Request::Command { command, id }).map_err(|error| io::Error::other(format!("The player stopped: {error}")))
	}
}

/// Hands a command to Connect, preserving errors for optimistic rollback.
fn dispatch(spirc: &Spirc, cmd: Command, id: u64) -> Result<(), librespot_core::Error> {
	crate::net::trace::playback("engine-command", cmd.name(), None);
	match cmd {
		Command::GetQueue | Command::Activate | Command::Load(_) | Command::Transfer { .. } => Ok(()),
		Command::Play { uri: None } => spirc.playback(id, PlaybackCommand::Play),
		Command::Play { uri: Some(uri) } => {
			let options = LoadRequestOptions { start_playing: true, ..LoadRequestOptions::default() };
			let request = if uri.starts_with("spotify:track:") || uri.starts_with("spotify:episode:") {
				LoadRequest::from_tracks(vec![uri], options)
			} else {
				LoadRequest::from_context_uri(uri, options)
			};
			spirc.playback(id, PlaybackCommand::Load(request))
		},
		Command::Pause => spirc.playback(id, PlaybackCommand::Pause),
		Command::SkipNext => spirc.playback(id, PlaybackCommand::Next),
		Command::SkipPrev => spirc.playback(id, PlaybackCommand::Previous),
		Command::Seek { position_ms } => spirc.set_position_ms(u32::try_from(position_ms).unwrap_or(u32::MAX)),
		Command::SetVolume { volume } => spirc.set_volume(to_u16(volume)),
		Command::SetShuffle { enabled } => spirc.shuffle(enabled),
		Command::SetRepeatContext { enabled } => spirc.repeat(enabled),
		Command::SetRepeatTrack { enabled } => spirc.repeat_track(enabled),
		Command::EditQueue { revision, edit } => spirc.edit_queue(revision, edit),
	}
}

// ------------------------------------------------------------------- forward

/// What the UI has been told, so only changes are sent.
#[derive(Default)]
struct Tracker {
	status:         Status,
	active:         bool,
	options:        (bool, bool, bool),
	context:        String,
	track:          String,
	upcoming:       Vec<(String, String, String)>,
	queue_revision: u64,
}

struct Forward {
	spirc:            Arc<Spirc>,
	/// Transport commands waiting for a requested takeover, never for metadata.
	deferred:         Vec<(Command, u64)>,
	takeover_by:      Option<tokio::time::Instant>,
	takeover_id:      u64,
	load_id:          u64,
	loading_request:  Option<u64>,
	session:          Session,
	deliver:          Box<dyn FnMut(Incoming) + Send>,
	metadata:         UnboundedSender<metadata::Request>,
	requests:         UnboundedSender<Request>,
	state:            Tracker,
	queue_generation: u64,
	transitions:      Arc<Transitions>,
	cluster:          SharedCluster,
}

impl Forward {
	async fn run(
		mut self,
		mut streams: Streams,
		mut requests: UnboundedReceiver<Request>,
		mut enriched: UnboundedReceiver<metadata::Ready>,
		name: String,
		volume: u8,
	) {
		self.event(Event::AuthState { logged_in: true, is_active: false, device_name: name });
		self.event(Event::VolumeChanged { volume });
		loop {
			tokio::select! {
				event = streams.player.recv() => match event {
					Some(event) => self.player_event(event),
					None => break,
				},
				snapshot = streams.snapshots.recv() => match snapshot {
					Some(snapshot) => self.snapshot(snapshot),
					None => break,
				},
				Some(ready) = enriched.recv() => self.enriched(ready),
				() = sleep_until(self.takeover_by), if self.takeover_by.is_some() => {
					self.takeover_by = None;
					match self.spirc.playback(self.takeover_id, PlaybackCommand::Activate) {
						Ok(()) => self.flush_deferred(),
						Err(error) => {
							self.failed(self.takeover_id, error.to_string());
							self.deferred.clear();
						},
					}
				},
				Some(request) = requests.recv() => match request {
					Request::Command { command, id } => self.command(command, id),
					Request::Failed { id, message } => self.failed(id, message),
				},
			}
		}
		(self.deliver)(Incoming::Closed("The player stopped".into()));
	}

	fn command(&mut self, command: Command, id: u64) {
		match command {
			Command::GetQueue => self.send_queue(),
			Command::Transfer { device, play } => self.transfer_device(device, play, id),
			Command::Activate => {
				crate::net::trace::playback("engine-command", "activate", None);
				self.takeover_id = id;
				self.takeover_by = Some(tokio::time::Instant::now() + TAKEOVER_WAIT);
				let resume = TransferOptions { restore_paused: Some("resume".to_owned()), ..TransferOptions::default() };
				if let Err(error) = self.spirc.playback(id, PlaybackCommand::Transfer(Some(TransferRequest { transfer_options: resume }))) {
					self.takeover_by = None;
					self.failed(id, error.to_string());
				}
			},
			command if self.takeover_by.is_some() => self.deferred.push((command, id)),
			command => self.dispatch(command, id),
		}
	}

	fn flush_deferred(&mut self) {
		for (command, id) in std::mem::take(&mut self.deferred) {
			self.dispatch(command, id);
		}
	}

	fn dispatch(&mut self, command: Command, id: u64) {
		if matches!(command, Command::Load(_) | Command::Play { uri: Some(_) } | Command::SkipNext | Command::SkipPrev) {
			self.load_id = id;
		}
		if matches!(command, Command::Load(_) | Command::EditQueue { .. })
			&& !self.state.active
			&& let Err(error) = self.spirc.playback(id, PlaybackCommand::Activate)
		{
			self.failed(id, error.to_string());
			return;
		}
		let result = if let Command::Load(start) = command {
			crate::net::trace::playback("engine-command", "load", None);
			self.spirc.playback(id, PlaybackCommand::Load(load::request(start, self.state.options)))
		} else {
			dispatch(&self.spirc, command, id)
		};
		if let Err(error) = result {
			self.failed(id, error.to_string());
		}
	}

	fn failed(&mut self, id: u64, message: String) {
		crate::net::trace::playback("command-failed", "playback", None);
		(self.deliver)(Incoming::CommandFailed { id, message });
	}

	fn event(&mut self, event: Event) {
		(self.deliver)(Incoming::Event(event));
	}

	fn status(&mut self, status: Status, position_ms: u32) {
		if status != Status::Buffering {
			self.loading_request = None;
		}
		self.state.status = status;
		self.event(Event::PlaybackChanged { status });
		self.position(position_ms);
	}

	fn position(&mut self, position_ms: u32) {
		let speed = if self.state.status == Status::Playing { 1.0 } else { 0.0 };
		self.event(Event::PositionSync { position: Position { position_ms: f64::from(position_ms), timestamp_ms: now_ms(), speed } });
	}

	fn player_event(&mut self, event: PlayerEvent) {
		trace_player_event(&event, "forward-event");
		match event {
			PlayerEvent::Playing { position_ms, .. } => self.status(Status::Playing, position_ms),
			PlayerEvent::Paused { position_ms, .. } => self.status(Status::Paused, position_ms),
			PlayerEvent::Loading { play_request_id, position_ms, .. } => {
				self.loading_request = Some(play_request_id);
				self.status(Status::Buffering, position_ms);
			},
			PlayerEvent::Stopped { .. } => self.status(Status::Idle, 0),
			PlayerEvent::Seeked { position_ms, .. } | PlayerEvent::PositionCorrection { position_ms, .. } => self.position(position_ms),
			PlayerEvent::TrackChanged { audio_item } => {
				self.state.track.clone_from(&audio_item.uri);
				let _ = self.metadata.send(metadata::Request::Track(audio_item.uri.clone()));
				self.event(Event::TrackChanged { item: metadata::audio_entity(*audio_item) });
			},
			PlayerEvent::Unavailable { play_request_id, .. } if self.loading_request == Some(play_request_id) => {
				self.loading_request = None;
				self.failed(self.load_id, "Spotify couldn't load this track".into());
			},
			PlayerEvent::VolumeChanged { volume } => self.event(Event::VolumeChanged { volume: to_percent(volume) }),
			_ => {},
		}
	}

	fn snapshot(&mut self, snap: StateSnapshot) {
		if let Some((id, message)) = snap.command_error {
			if id == self.takeover_id {
				self.takeover_by = None;
				self.deferred.clear();
			}
			self.failed(id, message);
			return;
		}
		if let Some(cluster) = snap.cluster {
			let cluster = Arc::new(cluster);
			self.cluster.send_replace(Some(cluster.clone()));
			self.cluster(&cluster, snap.initial_cluster);
			return;
		}
		if snap.initial_cluster {
			(self.deliver)(Incoming::RestoreUnavailable { account: self.session.username() });
			return;
		}
		// An inactive local player has no loaded context. Its empty snapshots must not
		// erase the account's passive bar or the options used by its next explicit load.
		if !snap.active && !self.state.active {
			return;
		}
		if snap.active != self.state.active {
			self.state.active = snap.active;
			self.event(Event::DeviceChanged { is_active: snap.active });
		}
		if snap.active && self.takeover_by.take().is_some() {
			self.flush_deferred();
		}
		self.transitions.context(&snap.context_uri, snap.shuffle);
		let options = (snap.shuffle, snap.repeat_context, snap.repeat_track);
		if options != self.state.options {
			self.state.options = options;
			let repeat = if snap.repeat_track {
				Repeat::Track
			} else if snap.repeat_context {
				Repeat::Context
			} else {
				Repeat::Off
			};
			self.event(Event::OptionsChanged { options: Options { shuffle: snap.shuffle, repeat, smart_shuffle: false } });
		}
		if snap.context_uri != self.state.context {
			self.state.context.clone_from(&snap.context_uri);
			let _ = self.metadata.send(metadata::Request::Context(snap.context_uri.clone()));
			self.event(Event::ContextChanged { context: named(snap.context_uri, String::new()) });
		}
		if snap.next != self.state.upcoming || snap.queue_revision != self.state.queue_revision {
			self.state.upcoming = snap.next;
			self.state.queue_revision = snap.queue_revision;
			self.send_queue();
		}
	}

	fn send_queue(&mut self) {
		self.queue_generation = self.queue_generation.wrapping_add(1);
		let _ = self.metadata.send(metadata::Request::Queue {
			next:       self.state.upcoming.clone(),
			revision:   self.state.queue_revision,
			generation: self.queue_generation,
		});
	}

	fn enriched(&mut self, ready: metadata::Ready) {
		match ready {
			metadata::Ready::Entity(entity) if entity.uri == self.state.track || entity.uri == self.state.context => {
				self.event(Event::MetadataChanged { entity });
			},
			metadata::Ready::Queue { upcoming, revision, generation } if generation == self.queue_generation => {
				self.event(Event::QueueChanged { upcoming, revision });
			},
			_ => {},
		}
	}
}

/// An entity that is only a name (an artist, a context).
fn named(uri: String, name: String) -> Entity {
	Entity { uri, decorations: Decorations { identity: Identity { name }, ..Decorations::default() } }
}

fn uri_of(id: &SpotifyUri) -> String {
	id.to_uri().unwrap_or_default()
}

fn trace_player_event(event: &PlayerEvent, stage: &str) {
	let (name, position_ms) = match event {
		PlayerEvent::Playing { position_ms, .. } => ("playing", Some(*position_ms)),
		PlayerEvent::Paused { position_ms, .. } => ("paused", Some(*position_ms)),
		PlayerEvent::Loading { position_ms, .. } => ("buffering", Some(*position_ms)),
		PlayerEvent::Stopped { .. } => ("idle", Some(0)),
		PlayerEvent::Seeked { position_ms, .. } | PlayerEvent::PositionCorrection { position_ms, .. } => ("position", Some(*position_ms)),
		PlayerEvent::TrackChanged { .. } => ("track", None),
		PlayerEvent::Unavailable { .. } => ("unavailable", None),
		_ => return,
	};
	crate::net::trace::playback(stage, name, position_ms.map(f64::from));
}
