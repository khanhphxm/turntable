//! The player: a Spotify Connect device (librespot) running in this process.
//!
//! [`Engine::start`] signs in, registers the device and decodes into the audio path's
//! [`PcmWriter`]; [`Engine::connect`] hands out the [`Client`] the UI drives it with. What the
//! player does comes back as the [`model`] events the UI already speaks: librespot's player
//! events, plus the connect state (context, options, upcoming tracks) the vendored
//! `librespot-connect` sends after every change.
//!
//! Natural playback overlaps use [`transition`]'s album-order policy and the
//! core audio mixer; manual transport commands cut immediately. Gapless PCM,
//! overlap PCM and ordinary PCM all pass through the same EQ/leveler/output.

mod connect;
mod load;
mod model;
mod premium;
#[cfg(test)]
mod session_tests;
mod transition;

use std::{
	collections::HashMap,
	fs, io,
	path::PathBuf,
	sync::{
		Arc,
		atomic::{AtomicBool, Ordering},
	},
	time::{Duration, SystemTime, UNIX_EPOCH},
};

pub(crate) use connect::device_of;
use librespot_connect::{ConnectConfig, LoadRequest, LoadRequestOptions, Spirc, StateSnapshot};
use librespot_core::{
	SpotifyUri,
	authentication::Credentials,
	cache::Cache,
	config::{DeviceType, SessionConfig},
	dealer::protocol::TransferOptions,
	session::Session,
	spclient::TransferRequest,
};
use librespot_metadata::{Album, Artist, Episode, Metadata, Playlist, Track, image::Images};
use librespot_playback::{
	config::{Bitrate, PlayerConfig},
	mixer::{Mixer, MixerConfig, softmixer::SoftMixer},
	player::{Player as Decoder, PlayerEvent},
};
pub use model::*;
pub use premium::SignInError;
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
}

/// The browser sign-in's redirect (librespot's OAuth helper serves it).
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
/// Entities kept for queue rows and context names, so a queue change looks up only what's new.
const ENTITY_CACHE: usize = 256;
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
	pub fn start(config: &EngineConfig, writer: PcmWriter, progress: &mut dyn FnMut(&str)) -> io::Result<Self> {
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
		let cache = Cache::new(Some(&config.state_dir), Some(&config.state_dir), audio.as_ref(), limit).map_err(io::Error::other)?;
		let session_config = SessionConfig { device_id: device_id(&config.state_dir)?, ..SessionConfig::default() };
		let credentials = match cache.credentials() {
			Some(c) => c,
			None => {
				progress("Sign in to Spotify in your browser…");
				sign_in(&session_config.client_id)?
			},
		};
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
		let activating = Arc::new(AtomicBool::new(false));
		let forward = Forward {
			spirc:         self.spirc.clone(),
			activating:    activating.clone(),
			deferred:      Vec::new(),
			takeover_by:   None,
			session:       self.session.clone(),
			deliver:       Box::new(deliver),
			entities:      HashMap::new(),
			state:         Tracker::default(),
			pending_track: None,
			transitions:   self.transitions.clone(),
			cluster:       self.cluster.clone(),
		};
		let (name, volume) = (self.name.clone(), self.volume);
		self.runtime.as_ref()?.spawn(forward.run(streams, requests, name, volume));
		Some(Client { spirc: self.spirc.clone(), requests: tx, activating })
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

/// Signs in through the browser (Spotify's own sign-in page) and returns the credentials.
fn sign_in(client_id: &str) -> io::Result<Credentials> {
	let token = librespot_oauth::OAuthClientBuilder::new(client_id, OAUTH_REDIRECT, OAUTH_SCOPES.to_vec())
		.open_in_browser()
		.build()
		.and_then(|client| client.get_access_token())
		.map_err(io::Error::other)?;
	Ok(Credentials::with_access_token(token.access_token))
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
	spirc:      Arc<Spirc>,
	requests:   UnboundedSender<Request>,
	/// A takeover is on its way: commands wait for the device to become active (Spotify ignores
	/// them until then) and the forwarding task sends them on.
	activating: Arc<AtomicBool>,
}

/// What the forwarding task does for the client.
enum Request {
	Queue,
	/// A takeover was asked for: commands wait for it (or for [`TAKEOVER_WAIT`]).
	Activating,
	/// A command sent while the device was being activated.
	Deferred(Command),
	Transfer {
		device: String,
		play:   bool,
	},
}

impl Client {
	pub fn send(&self, cmd: Command) {
		match cmd {
			Command::GetQueue => {
				let _ = self.requests.send(Request::Queue);
			},
			Command::Transfer { device, play } => {
				let _ = self.requests.send(Request::Transfer { device, play });
			},
			// Takes playback over from wherever it is (or was last) and plays, like picking this
			// device in Spotify and pressing play.
			Command::Activate => {
				self.activating.store(true, Ordering::Release);
				let _ = self.requests.send(Request::Activating);
				let resume = TransferOptions { restore_paused: Some("resume".to_owned()), ..TransferOptions::default() };
				let _ = self.spirc.transfer(Some(TransferRequest { transfer_options: resume }));
			},
			cmd @ (Command::Load(_) | Command::EditQueue { .. }) => {
				let _ = self.requests.send(Request::Deferred(cmd));
			},
			cmd if self.activating.load(Ordering::Acquire) => {
				let _ = self.requests.send(Request::Deferred(cmd));
			},
			cmd => dispatch(&self.spirc, cmd),
		}
	}
}

/// Hands a command to the Connect device. A closed channel means the player stopped (`alive`
/// reports it), so errors are dropped.
fn dispatch(spirc: &Spirc, cmd: Command) {
	let _ = match cmd {
		Command::GetQueue | Command::Activate | Command::Load(_) | Command::Transfer { .. } => Ok(()),
		Command::Play { uri: None } => spirc.play(),
		Command::Play { uri: Some(uri) } => {
			let options = LoadRequestOptions { start_playing: true, ..LoadRequestOptions::default() };
			let request = if uri.starts_with("spotify:track:") || uri.starts_with("spotify:episode:") {
				LoadRequest::from_tracks(vec![uri], options)
			} else {
				LoadRequest::from_context_uri(uri, options)
			};
			spirc.load(request)
		},
		Command::Pause => spirc.pause(),
		Command::SkipNext => spirc.next(),
		Command::SkipPrev => spirc.prev(),
		Command::Seek { position_ms } => spirc.set_position_ms(u32::try_from(position_ms).unwrap_or(u32::MAX)),
		Command::SetVolume { volume } => spirc.set_volume(to_u16(volume)),
		Command::SetShuffle { enabled } => spirc.shuffle(enabled),
		Command::SetRepeatContext { enabled } => spirc.repeat(enabled),
		Command::SetRepeatTrack { enabled } => spirc.repeat_track(enabled),
		Command::EditQueue { revision, edit } => spirc.edit_queue(revision, edit),
	};
}

// ------------------------------------------------------------------- forward

/// What the UI has been told, so only changes are sent.
#[derive(Default)]
struct Tracker {
	status:         Status,
	active:         bool,
	options:        (bool, bool, bool),
	context:        String,
	upcoming:       Vec<(String, String, String)>,
	queue_revision: u64,
}

struct Forward {
	spirc:         Arc<Spirc>,
	/// Shared with the client: a takeover is on its way.
	activating:    Arc<AtomicBool>,
	/// Commands waiting for the takeover to land, and until when they wait.
	deferred:      Vec<Command>,
	takeover_by:   Option<tokio::time::Instant>,
	session:       Session,
	deliver:       Box<dyn FnMut(Incoming) + Send>,
	entities:      HashMap<String, Entity>,
	state:         Tracker,
	/// A track change seen in the last player event, to look up before telling the UI.
	pending_track: Option<String>,
	transitions:   Arc<Transitions>,
	cluster:       SharedCluster,
}

impl Forward {
	async fn run(mut self, mut streams: Streams, mut requests: UnboundedReceiver<Request>, name: String, volume: u8) {
		self.event(Event::AuthState { logged_in: true, is_active: false, device_name: name });
		self.event(Event::VolumeChanged { volume });
		loop {
			tokio::select! {
				event = streams.player.recv() => match event {
					Some(event) => {
						self.player_event(event);
						if let Some(uri) = self.pending_track.take() {
							let item = self.track(&uri).await;
							self.event(Event::TrackChanged { item });
						}
					},
					None => break,
				},
				snapshot = streams.snapshots.recv() => match snapshot {
					Some(snapshot) => self.snapshot(snapshot).await,
					None => break,
				},
				() = sleep_until(self.takeover_by), if self.takeover_by.is_some() => {
					// Nothing to take over (no session to transfer): this device starts its own.
					self.takeover_by = None;
					if self.activating.swap(false, Ordering::AcqRel) {
						let _ = self.spirc.activate();
						for cmd in std::mem::take(&mut self.deferred) {
							self.dispatch(cmd);
						}
					}
				},
				Some(request) = requests.recv() => match request {
					Request::Queue => self.send_queue().await,
					Request::Activating => self.takeover_by = Some(tokio::time::Instant::now() + TAKEOVER_WAIT),
					Request::Transfer { device, play } => self.transfer_device(&device, play).await,
					Request::Deferred(cmd) => {
						// The takeover may have landed while the command was on its way.
						if self.activating.load(Ordering::Acquire) {
							self.deferred.push(cmd);
						} else {
							self.dispatch(cmd);
						}
					},
				},
			}
		}
		(self.deliver)(Incoming::Closed("The player stopped".into()));
	}

	/// Sends a command on; a load first activates this device (Spotify ignores one otherwise)
	/// and keeps the shuffle and repeat it plays with, as the Web API's play does.
	fn dispatch(&mut self, cmd: Command) {
		if matches!(cmd, Command::EditQueue { .. }) && !self.state.active {
			let _ = self.spirc.activate();
		}
		let Command::Load(start) = cmd else {
			dispatch(&self.spirc, cmd);
			return;
		};
		if !self.state.active {
			let _ = self.spirc.activate();
		}
		let _ = self.spirc.load(load::request(start, self.state.options));
	}

	fn event(&mut self, event: Event) {
		(self.deliver)(Incoming::Event(event));
	}

	fn status(&mut self, status: Status, position_ms: u32) {
		if self.state.status != status {
			self.state.status = status;
			self.event(Event::PlaybackChanged { status });
		}
		self.position(position_ms);
	}

	fn position(&mut self, position_ms: u32) {
		let speed = if self.state.status == Status::Playing { 1.0 } else { 0.0 };
		self.event(Event::PositionSync { position: Position { position_ms: f64::from(position_ms), timestamp_ms: now_ms(), speed } });
	}

	fn player_event(&mut self, event: PlayerEvent) {
		match event {
			PlayerEvent::Playing { position_ms, .. } => self.status(Status::Playing, position_ms),
			PlayerEvent::Paused { position_ms, .. } => self.status(Status::Paused, position_ms),
			PlayerEvent::Loading { position_ms, .. } => self.status(Status::Buffering, position_ms),
			PlayerEvent::Stopped { .. } => self.status(Status::Idle, 0),
			PlayerEvent::Seeked { position_ms, .. } | PlayerEvent::PositionCorrection { position_ms, .. } => self.position(position_ms),
			PlayerEvent::TrackChanged { audio_item } => self.pending_track = Some(audio_item.uri),
			PlayerEvent::VolumeChanged { volume } => self.event(Event::VolumeChanged { volume: to_percent(volume) }),
			_ => {},
		}
	}

	async fn snapshot(&mut self, snap: StateSnapshot) {
		if let Some(cluster) = snap.cluster {
			let cluster = Arc::new(cluster);
			self.cluster.send_replace(Some(cluster.clone()));
			self.cluster(&cluster, snap.initial_cluster).await;
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
		if snap.active && self.activating.swap(false, Ordering::AcqRel) {
			self.takeover_by = None;
			// What was asked meanwhile goes now (a takeover keeps the other device's pause, so a
			// play asked for still has to be sent).
			for cmd in std::mem::take(&mut self.deferred) {
				self.dispatch(cmd);
			}
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
			let context = self.context(&snap.context_uri).await;
			self.event(Event::ContextChanged { context });
		}
		if snap.next != self.state.upcoming || snap.queue_revision != self.state.queue_revision {
			self.state.upcoming = snap.next;
			self.state.queue_revision = snap.queue_revision;
			self.send_queue().await;
		}
	}

	async fn send_queue(&mut self) {
		let wanted = self.state.upcoming.clone();
		let mut upcoming = Vec::with_capacity(wanted.len());
		for (uid, uri, provider) in wanted {
			let item = self.track(&uri).await;
			let source = match provider.as_str() {
				"queue" => "queue",
				"autoplay" => "autoplay",
				_ => "context",
			};
			upcoming.push(QueueEntry { uid, source: source.to_owned(), item });
		}
		self.event(Event::QueueChanged { upcoming, revision: self.state.queue_revision });
	}

	fn remember(&mut self, entity: Entity) {
		if self.entities.len() >= ENTITY_CACHE {
			self.entities.clear();
		}
		self.entities.insert(entity.uri.clone(), entity);
	}

	/// A track or episode (now playing, a queue row), looked up once.
	async fn track(&mut self, uri: &str) -> Entity {
		if let Some(e) = self.entities.get(uri) {
			return e.clone();
		}
		let looked_up = match SpotifyUri::from_uri(uri) {
			Ok(id @ SpotifyUri::Track { .. }) => Track::get(&self.session, &id).await.ok().map(|t| self.track_entity(&t)),
			Ok(id @ SpotifyUri::Episode { .. }) => Episode::get(&self.session, &id).await.ok().map(|e| self.episode_entity(&e)),
			_ => None,
		};
		let entity = looked_up.unwrap_or_else(|| Entity { uri: uri.to_owned(), ..Entity::default() });
		self.remember(entity.clone());
		entity
	}

	fn track_entity(&self, track: &Track) -> Entity {
		let album = Entity {
			uri:         uri_of(&track.album.id),
			decorations: Decorations {
				identity: Identity { name: track.album.name.clone() },
				visual_identity: VisualIdentity { cover: self.covers(&track.album.covers) },
				..Decorations::default()
			},
		};
		let creators = track.artists.iter().map(|a| Related { entity: Box::new(named(uri_of(&a.id), a.name.clone())) }).collect();
		Entity {
			uri:         uri_of(&track.id),
			decorations: Decorations {
				identity: Identity { name: track.name.clone() },
				visual_identity: VisualIdentity::default(),
				parent: Some(Related { entity: Box::new(album) }),
				creators,
				playback: PlaybackInfo {
					duration_ms:     u64::try_from(track.duration).unwrap_or_default(),
					content_ratings: if track.is_explicit { vec!["explicit".to_owned()] } else { Vec::new() },
				},
			},
		}
	}

	fn episode_entity(&self, episode: &Episode) -> Entity {
		let show = Entity {
			decorations: Decorations {
				identity: Identity { name: episode.show_name.clone() },
				visual_identity: VisualIdentity { cover: self.covers(&episode.covers) },
				..Decorations::default()
			},
			..Entity::default()
		};
		Entity {
			uri:         uri_of(&episode.id),
			decorations: Decorations {
				identity:        Identity { name: episode.name.clone() },
				visual_identity: VisualIdentity::default(),
				parent:          Some(Related { entity: Box::new(show) }),
				creators:        vec![Related { entity: Box::new(named(String::new(), episode.show_name.clone())) }],
				playback:        PlaybackInfo {
					duration_ms:     u64::try_from(episode.duration).unwrap_or_default(),
					content_ratings: if episode.is_explicit { vec!["explicit".to_owned()] } else { Vec::new() },
				},
			},
		}
	}

	/// Cover URLs by the size names the UI picks from.
	fn covers(&self, images: &Images) -> Vec<Cover> {
		let base = self.session.get_user_attribute("image-url").unwrap_or_else(|| "https://i.scdn.co/image/{file_id}".to_owned());
		images
			.iter()
			.map(|i| Cover { url: base.replace("{file_id}", &i.id.to_string()), size: format!("{:?}", i.size).to_ascii_lowercase() })
			.collect()
	}

	/// What plays, by name: a playlist, an album, an artist, Liked Songs.
	async fn context(&mut self, uri: &str) -> Entity {
		if !uri.starts_with("spotify:album:")
			&& let Some(e) = self.entities.get(uri)
		{
			return e.clone();
		}
		let name = if uri.ends_with(":collection") {
			Some("Liked Songs".to_owned())
		} else {
			match SpotifyUri::from_uri(uri) {
				Ok(id @ SpotifyUri::Playlist { .. }) => Playlist::get(&self.session, &id).await.ok().map(|p| p.name().to_owned()),
				Ok(id @ SpotifyUri::Album { .. }) => Album::get(&self.session, &id).await.ok().map(|a| {
					self.transitions.album(&a);
					a.name
				}),
				Ok(id @ SpotifyUri::Artist { .. }) => Artist::get(&self.session, &id).await.ok().map(|a| a.name),
				_ => None,
			}
		};
		let entity = named(uri.to_owned(), name.unwrap_or_default());
		if !uri.is_empty() {
			self.remember(entity.clone());
		}
		entity
	}
}

/// An entity that is only a name (an artist, a context).
fn named(uri: String, name: String) -> Entity {
	Entity { uri, decorations: Decorations { identity: Identity { name }, ..Decorations::default() } }
}

fn uri_of(id: &SpotifyUri) -> String {
	id.to_uri().unwrap_or_default()
}
