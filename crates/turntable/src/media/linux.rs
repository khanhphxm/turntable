use std::{
	io::{Read as _, Write as _},
	os::{
		fd::AsRawFd as _,
		unix::{ffi::OsStrExt as _, net::UnixStream},
	},
	sync::{Arc, mpsc},
	thread::{self, JoinHandle},
	time::{Duration, Instant},
};

use dbus::{
	Message, MethodErr, Path,
	arg::{PropMap, Variant},
	blocking::{Connection, stdintf::org_freedesktop_dbus::RequestNameReply},
	channel::{BusType, Channel, Sender as _},
};
use dbus_crossroads::{Crossroads, IfaceToken};
use tempfile::NamedTempFile;

use super::{Command, Metadata, Playback};

const NAME: &str = "org.mpris.MediaPlayer2.turntable";
const PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";
const NO_TRACK: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";

type Reply = mpsc::SyncSender<Result<(), String>>;

enum Update {
	Metadata(Metadata, Reply),
	Playback(Playback, Reply),
}

pub(super) struct Backend {
	updates: mpsc::Sender<Update>,
	wake:    UnixStream,
	worker:  Option<JoinHandle<()>>,
}

impl Backend {
	pub(super) fn new(on_command: Arc<dyn Fn(Command) + Send + Sync>) -> Result<Self, String> {
		let (wake, worker_wake) = UnixStream::pair().map_err(|error| error.to_string())?;
		let (updates, worker_updates) = mpsc::channel();
		let (ready_tx, ready_rx) = mpsc::sync_channel(1);
		let worker = thread::Builder::new()
			.name("turntable-mpris".into())
			.spawn(move || {
				let session = Session::new(on_command);
				match session {
					Ok(mut session) => {
						if ready_tx.send(Ok(())).is_ok() {
							session.run(worker_wake, &worker_updates);
						}
					},
					Err(error) => {
						let _ = ready_tx.send(Err(error));
					},
				}
			})
			.map_err(|error| error.to_string())?;
		let mut backend = Self { updates, wake, worker: Some(worker) };
		ready_rx.recv().map_err(|error| error.to_string())??;
		// Ensure queued name-acquisition messages are serviced before construction
		// returns; subsequent updates have the same request/reply ordering.
		backend.set_playback(Playback::Stopped)?;
		Ok(backend)
	}

	pub(super) fn set_metadata(&mut self, metadata: &Metadata) -> Result<(), String> {
		let (reply, received) = mpsc::sync_channel(1);
		self.update(Update::Metadata(metadata.clone(), reply), &received)
	}

	pub(super) fn set_playback(&mut self, playback: Playback) -> Result<(), String> {
		let (reply, received) = mpsc::sync_channel(1);
		self.update(Update::Playback(playback, reply), &received)
	}

	fn update(&mut self, update: Update, received: &mpsc::Receiver<Result<(), String>>) -> Result<(), String> {
		self.updates.send(update).map_err(|error| error.to_string())?;
		self.wake.write_all(&[1]).map_err(|error| error.to_string())?;
		received.recv().map_err(|error| error.to_string())?
	}
}

impl Drop for Backend {
	fn drop(&mut self) {
		// EOF wakes poll immediately, including when there are no D-Bus clients.
		let _ = self.wake.shutdown(std::net::Shutdown::Both);
		if let Some(worker) = self.worker.take() {
			let _ = worker.join();
		}
	}
}

struct Player {
	on_command:   Arc<dyn Fn(Command) + Send + Sync>,
	metadata:     Metadata,
	artwork_file: Option<NamedTempFile>,
	artwork_url:  Option<String>,
	track_number: u64,
	track_id:     Path<'static>,
	playback:     Playback,
	updated_at:   Instant,
}

impl Player {
	fn position_ms(&self) -> u64 {
		let elapsed = if matches!(self.playback, Playback::Playing { .. }) {
			u64::try_from(self.updated_at.elapsed().as_millis()).unwrap_or(u64::MAX)
		} else {
			0
		};
		let position = self.playback.position_ms().saturating_add(elapsed);
		if self.metadata.duration_ms == 0 { position } else { position.min(self.metadata.duration_ms) }
	}

	fn status(&self) -> &'static str {
		match self.playback {
			Playback::Playing { .. } => "Playing",
			Playback::Paused { .. } => "Paused",
			Playback::Stopped => "Stopped",
		}
	}

	fn metadata_map(&self) -> PropMap {
		let mut map = PropMap::with_capacity(6);
		if matches!(self.playback, Playback::Stopped) {
			map.insert("mpris:trackid".into(), Variant(Box::new(Path::from(NO_TRACK))));
			return map;
		}
		map.insert("mpris:trackid".into(), Variant(Box::new(self.track_id.clone())));
		map.insert("mpris:length".into(), Variant(Box::new(microseconds(self.metadata.duration_ms))));
		map.insert("xesam:title".into(), Variant(Box::new(self.metadata.title.clone())));
		map.insert("xesam:artist".into(), Variant(Box::new(vec![self.metadata.artist.clone()])));
		map.insert("xesam:album".into(), Variant(Box::new(self.metadata.album.clone())));
		if let Some(url) = &self.artwork_url {
			map.insert("mpris:artUrl".into(), Variant(Box::new(url.clone())));
		}
		map
	}

	fn set_metadata(&mut self, metadata: Metadata) -> Result<(), String> {
		// Artwork updates of the same track must not invalidate a client's TrackId.
		let track_changed = self.metadata.title != metadata.title
			|| self.metadata.artist != metadata.artist
			|| self.metadata.album != metadata.album
			|| self.metadata.duration_ms != metadata.duration_ms;
		let artwork_changed = self.metadata.artwork != metadata.artwork;
		let artwork_file = if artwork_changed {
			match &metadata.artwork {
				Some(bytes) => {
					let mut file = tempfile::Builder::new().prefix("turntable-artwork-").tempfile().map_err(|error| error.to_string())?;
					file.write_all(bytes).map_err(|error| error.to_string())?;
					Some(file)
				},
				None => None,
			}
		} else {
			None
		};
		if track_changed || self.track_number == 0 {
			self.track_number = self.track_number.saturating_add(1);
			self.track_id = Path::new(format!("/org/mpris/MediaPlayer2/track/{}", self.track_number))?;
		}
		if artwork_changed {
			self.artwork_url = artwork_file.as_ref().map(|file| file_url(file.path()));
			self.artwork_file = artwork_file;
		}
		self.metadata = metadata;
		Ok(())
	}
}

struct Session {
	connection: Connection,
	crossroads: Crossroads,
	path:       Path<'static>,
}

impl Session {
	fn new(on_command: Arc<dyn Fn(Command) + Send + Sync>) -> Result<Self, String> {
		let mut channel = Channel::get_private(BusType::Session).map_err(|error| error.to_string())?;
		channel.set_watch_enabled(true);
		let connection = Connection::from(channel);
		let mut crossroads = Crossroads::new();
		let path = Path::from(PATH);
		let root = register_root(&mut crossroads);
		let player = crossroads.register(PLAYER, |builder| {
			for (name, command) in [
				("Play", Command::Play),
				("Pause", Command::Pause),
				("PlayPause", Command::Toggle),
				("Next", Command::Next),
				("Previous", Command::Previous),
			] {
				builder.method(name, (), (), move |_, player: &mut Player, ()| {
					(player.on_command)(command);
					Ok(())
				});
			}
			builder.method("Stop", (), (), |_, player: &mut Player, ()| {
				(player.on_command)(Command::Pause);
				(player.on_command)(Command::SetPosition(0));
				Ok(())
			});
			builder.method("Seek", ("Offset",), (), |_, player: &mut Player, (offset,): (i64,)| {
				if player.metadata.duration_ms > 0 && !matches!(player.playback, Playback::Stopped) {
					let target = i128::from(player.position_ms()) * 1_000 + i128::from(offset);
					if target > i128::from(player.metadata.duration_ms) * 1_000 {
						(player.on_command)(Command::Next);
					} else if target < 0 {
						(player.on_command)(Command::SetPosition(0));
					} else {
						(player.on_command)(Command::Seek(offset / 1_000));
					}
				}
				Ok(())
			});
			builder.method(
				"SetPosition",
				("TrackId", "Position"),
				(),
				|_, player: &mut Player, (track_id, position): (Path<'static>, i64)| {
					let Ok(position) = u64::try_from(position) else {
						return Ok(());
					};
					if !matches!(player.playback, Playback::Stopped)
						&& track_id == player.track_id
						&& position <= player.metadata.duration_ms.saturating_mul(1_000)
					{
						(player.on_command)(Command::SetPosition(position / 1_000));
					}
					Ok(())
				},
			);
			builder.method("OpenUri", ("Uri",), (), |_, _: &mut Player, (_uri,): (String,)| -> Result<(), MethodErr> {
				Err(MethodErr::from(("org.freedesktop.DBus.Error.NotSupported", "Opening arbitrary URIs is not supported")))
			});
			builder.signal::<(i64,), _>("Seeked", ("Position",));
			builder.property::<String, _>("PlaybackStatus").get(|_, player: &mut Player| Ok(player.status().into()));
			builder.property::<PropMap, _>("Metadata").get(|_, player: &mut Player| Ok(player.metadata_map()));
			builder
				.property::<i64, _>("Position")
				.emits_changed_false()
				.get(|_, player: &mut Player| Ok(microseconds(player.position_ms())));
			builder.property::<f64, _>("Rate").emits_changed_const().get(|_, _: &mut Player| Ok(1.0)).set(|_, _: &mut Player, value| {
				if value == 1.0 {
					Ok(Some(value))
				} else {
					Err(MethodErr::from(("org.freedesktop.DBus.Error.NotSupported", "Only rate 1.0 is supported")))
				}
			});
			builder.property::<f64, _>("Volume").emits_changed_const().get(|_, _: &mut Player| Ok(1.0)).set(|_, _: &mut Player, _value| {
				Err(MethodErr::from(("org.freedesktop.DBus.Error.NotSupported", "Volume is controlled in turntable")))
			});
			builder.property::<f64, _>("MinimumRate").emits_changed_const().get(|_, _: &mut Player| Ok(1.0));
			builder.property::<f64, _>("MaximumRate").emits_changed_const().get(|_, _: &mut Player| Ok(1.0));
			for name in ["CanGoNext", "CanGoPrevious", "CanPlay", "CanPause", "CanControl"] {
				builder.property::<bool, _>(name).emits_changed_const().get(|_, _: &mut Player| Ok(true));
			}
			builder
				.property::<bool, _>("CanSeek")
				.get(|_, player: &mut Player| Ok(player.metadata.duration_ms > 0 && !matches!(player.playback, Playback::Stopped)));
		});
		crossroads.insert(path.clone(), &[root, player], Player {
			on_command,
			metadata: Metadata::default(),
			artwork_file: None,
			artwork_url: None,
			track_number: 0,
			track_id: Path::from(NO_TRACK),
			playback: Playback::Stopped,
			updated_at: Instant::now(),
		});
		if connection.request_name(NAME, false, false, true).map_err(|error| error.to_string())? != RequestNameReply::PrimaryOwner {
			return Err("Another turntable MPRIS session is already running".into());
		}
		Ok(Self { connection, crossroads, path })
	}

	fn run(&mut self, mut wake: UnixStream, updates: &mpsc::Receiver<Update>) {
		loop {
			let channel = self.connection.channel();
			if channel.read_write(Some(Duration::ZERO)).is_err() {
				break;
			}
			while let Some(message) = channel.pop_message() {
				if message.msg_type() == dbus::MessageType::MethodCall {
					let _ = self.crossroads.handle_message(message, &self.connection);
				}
			}
			let watch = channel.watch();
			let mut descriptors = [
				libc::pollfd {
					fd:      watch.fd,
					events:  (if watch.read { libc::POLLIN } else { 0 }) | (if watch.write { libc::POLLOUT } else { 0 }),
					revents: 0,
				},
				libc::pollfd { fd: wake.as_raw_fd(), events: libc::POLLIN, revents: 0 },
			];
			// SAFETY: both fds are owned for this loop's lifetime, and poll receives
			// exactly the length of this initialized, writable descriptor array.
			if unsafe { libc::poll(descriptors.as_mut_ptr(), descriptors.len() as libc::nfds_t, -1) } < 0 {
				if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
					continue;
				}
				break;
			}
			if descriptors[1].revents != 0 {
				let mut bytes = [0; 64];
				if !matches!(wake.read(&mut bytes), Ok(count) if count > 0) {
					break;
				}
				while let Ok(update) = updates.try_recv() {
					let (result, reply) = match update {
						Update::Metadata(metadata, reply) => (self.set_metadata(metadata), reply),
						Update::Playback(playback, reply) => (self.set_playback(playback), reply),
					};
					let _ = reply.send(result);
				}
			}
		}
	}

	fn set_metadata(&mut self, metadata: Metadata) -> Result<(), String> {
		self.crossroads.data_mut::<Player>(&self.path).ok_or("MPRIS player is missing")?.set_metadata(metadata)?;
		self.changed(true)
	}

	fn set_playback(&mut self, playback: Playback) -> Result<(), String> {
		let player = self.crossroads.data_mut::<Player>(&self.path).ok_or("MPRIS player is missing")?;
		let previous_position = player.position_ms();
		let previous_status = player.status();
		player.playback = playback;
		player.updated_at = Instant::now();
		let metadata_changed = previous_status == "Stopped" || player.status() == "Stopped";
		let seeked = previous_position != playback.position_ms();
		self.changed(metadata_changed)?;
		if seeked {
			let message = Message::new_signal(PATH, PLAYER, "Seeked")?.append1(microseconds(playback.position_ms()));
			self.connection.send(message).map_err(|()| "Unable to publish MPRIS seek position")?;
		}
		Ok(())
	}

	fn changed(&mut self, metadata_changed: bool) -> Result<(), String> {
		let player = self.crossroads.data_mut::<Player>(&self.path).ok_or("MPRIS player is missing")?;
		let mut changed = PropMap::with_capacity(3);
		changed.insert("PlaybackStatus".into(), Variant(Box::new(player.status().to_owned())));
		changed
			.insert("CanSeek".into(), Variant(Box::new(player.metadata.duration_ms > 0 && !matches!(player.playback, Playback::Stopped))));
		if metadata_changed {
			changed.insert("Metadata".into(), Variant(Box::new(player.metadata_map())));
		}
		let signal = Message::new_signal(PATH, "org.freedesktop.DBus.Properties", "PropertiesChanged")?.append3(
			PLAYER,
			changed,
			Vec::<String>::new(),
		);
		self.connection.send(signal).map_err(|()| "Unable to publish MPRIS state")?;
		Ok(())
	}
}

fn register_root(crossroads: &mut Crossroads) -> IfaceToken<Player> {
	crossroads.register("org.mpris.MediaPlayer2", |builder| {
		builder.property::<bool, _>("CanQuit").emits_changed_const().get(|_, _: &mut Player| Ok(false));
		builder.property::<bool, _>("CanRaise").emits_changed_const().get(|_, _: &mut Player| Ok(false));
		builder.property::<bool, _>("HasTrackList").emits_changed_const().get(|_, _: &mut Player| Ok(false));
		builder.property::<String, _>("Identity").emits_changed_const().get(|_, _: &mut Player| Ok("Turntable".into()));
		builder.property::<String, _>("DesktopEntry").emits_changed_const().get(|_, _: &mut Player| Ok("turntable".into()));
		builder.property::<Vec<String>, _>("SupportedUriSchemes").emits_changed_const().get(|_, _: &mut Player| Ok(Vec::new()));
		builder.property::<Vec<String>, _>("SupportedMimeTypes").emits_changed_const().get(|_, _: &mut Player| Ok(Vec::new()));
		builder.method("Raise", (), (), |_, _: &mut Player, ()| -> Result<(), MethodErr> {
			Err(MethodErr::from(("org.freedesktop.DBus.Error.NotSupported", "Raising the window is not supported")))
		});
		builder.method("Quit", (), (), |_, _: &mut Player, ()| -> Result<(), MethodErr> {
			Err(MethodErr::from(("org.freedesktop.DBus.Error.NotSupported", "Quitting remotely is not supported")))
		});
	})
}

fn microseconds(milliseconds: u64) -> i64 {
	i64::try_from(milliseconds.saturating_mul(1_000)).unwrap_or(i64::MAX)
}

fn file_url(path: &std::path::Path) -> String {
	let mut url = String::from("file://");
	for &byte in path.as_os_str().as_bytes() {
		if byte.is_ascii_alphanumeric() || b"/-_.~".contains(&byte) {
			url.push(char::from(byte));
		} else {
			url.push('%');
			for nibble in [byte >> 4, byte & 15] {
				let digit = if nibble < 10 { b'0' + nibble } else { b'A' + nibble - 10 };
				url.push(char::from(digit));
			}
		}
	}
	url
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn time_conversion_saturates() {
		assert_eq!(microseconds(12_345), 12_345_000);
		assert_eq!(microseconds(u64::MAX), i64::MAX);
	}

	#[test]
	fn artwork_url_escapes_paths() {
		assert_eq!(file_url(std::path::Path::new("/tmp/album art #1.png")), "file:///tmp/album%20art%20%231.png");
	}
}
