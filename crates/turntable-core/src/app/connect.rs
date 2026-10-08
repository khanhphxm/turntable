//! Application state for Spotify Connect: the device menu and playback on another device.
//!
//! turntable plays through its in-process engine. Its authenticated Dealer subscription
//! supplies the account's devices and remote player through [`State::apply_connect_state`],
//! without polling or transferring playback. When no session exists,
//! [`State::apply_playback`] uses `GET /me/player` while the window shows. Device picks use
//! session transfers; remote controls go through the worker's native connect-state commands,
//! with the Web API only when no session exists. Pure state: commands are returned as effects.

use std::time::{Duration, Instant};

use crate::{
	app::{Anchor, Effect, Msg, State},
	engine::{Command, Cover, Decorations, Entity, Identity, Options, PlaybackInfo, Related, Repeat, Status, VisualIdentity},
	library::{Conn, Request},
	spotify::{self, Device, Playback, PlayerCommand, RepeatMode, Track},
};

/// How often a poll follows the last one while another device plays.
const POLL_EVERY: Duration = Duration::from_secs(4);
/// The longest wait between polls after failures.
const POLL_MAX: Duration = Duration::from_secs(60);
/// A position that differs from where the clock would be by more than this is re-anchored.
const DRIFT_MS: u64 = 1500;

/// The wait before the next poll after `failures` failed ones in a row: 4 s, doubling to a minute.
pub fn poll_delay(failures: u32) -> Duration {
	POLL_EVERY.saturating_mul(1 << failures.min(4)).min(POLL_MAX)
}

/// The devices Spotify lists, as last asked.
#[derive(Debug, Default)]
pub enum DeviceList {
	/// Not asked for yet.
	#[default]
	Unknown,
	Ready(Vec<Device>),
	/// The request failed (a sentence for the user).
	Failed(String),
}

#[derive(Debug, Default)]
pub struct Connect {
	/// The device menu is open (it hangs from the devices button).
	pub menu:    bool,
	pub devices: DeviceList,
	/// The device playing, when it isn't turntable's.
	pub remote:  Option<Device>,
	/// Dealer is supplying account-wide devices and playback; Web API polling is unnecessary.
	pub session: bool,
}

impl State {
	/// The other device that plays, if any.
	pub fn remote(&self) -> Option<&Device> {
		self.connect.remote.as_ref()
	}

	/// Whether `device` is this player.
	pub fn is_this_device(&self, device: &Device) -> bool {
		!self.device_name.is_empty() && device.name == self.device_name
	}

	/// Whether the Web API can be asked about devices and playback at all.
	pub fn web_ready(&self) -> bool {
		self.library.conn == Conn::Ready
	}

	/// Whether the host should present the device menu.
	pub fn devices_menu_open(&self) -> bool {
		self.connect.menu
	}

	/// Whether `GET /me/player` is worth asking now (the caller adds that the window shows).
	pub fn wants_poll(&self) -> bool {
		!self.connect.session && self.web_ready() && self.connect.remote.is_some() && !self.active
	}

	/// Whether a single poll could tell that another device plays: turntable isn't the active
	/// one, and the Web API is there to ask.
	pub fn may_probe(&self) -> bool {
		!self.connect.session && self.web_ready() && !self.active
	}

	/// The devices to offer besides the current one, this player first when it isn't current.
	pub fn other_devices(&self) -> Vec<Device> {
		let DeviceList::Ready(list) = &self.connect.devices else { return Vec::new() };
		let mut others: Vec<Device> = list.iter().filter(|d| !self.is_current(d)).cloned().collect();
		// This player is signed in but Spotify hasn't listed it yet: it can still be picked.
		let listed = list.iter().any(|d| self.is_this_device(d));
		if self.logged_in && !listed && !self.active && !self.device_name.is_empty() {
			others.insert(0, Device {
				id:              String::new(),
				name:            self.device_name.clone(),
				is_active:       false,
				volume_percent:  None,
				supports_volume: None,
				kind:            "Computer".into(),
			});
		}
		others.sort_by_key(|d| !self.is_this_device(d));
		others
	}

	/// Whether `device` is the one playing now.
	fn is_current(&self, device: &Device) -> bool {
		match &self.connect.remote {
			Some(remote) => remote.id == device.id,
			None => self.active && self.is_this_device(device),
		}
	}

	/// Opens the device menu (listing the devices again), or closes it.
	pub fn toggle_devices_menu(&mut self) -> Vec<Effect> {
		self.connect.menu = !self.connect.menu;
		if self.connect.menu { self.refresh_devices() } else { Vec::new() }
	}

	pub fn refresh_devices(&mut self) -> Vec<Effect> {
		if self.connect.session || !self.web_ready() {
			return Vec::new();
		}
		if matches!(self.connect.devices, DeviceList::Failed(_)) {
			self.connect.devices = DeviceList::Unknown;
		}
		vec![Effect::Library(Request::Devices)]
	}

	/// Takes in the device list. Returns whether another device turns out to be playing, which
	/// is worth a poll.
	pub fn apply_devices(&mut self, result: Result<Vec<Device>, String>) -> bool {
		if self.connect.session {
			return false;
		}
		match result {
			Ok(list) => {
				let elsewhere =
					!self.active && self.connect.remote.is_none() && list.iter().any(|d| d.is_active && !self.is_this_device(d));
				self.connect.devices = DeviceList::Ready(list);
				elsewhere
			},
			Err(message) => {
				self.connect.devices = DeviceList::Failed(message);
				false
			},
		}
	}

	/// A device click uses native Connect transfer when the session supplies its inventory,
	/// otherwise the Web API, keeping playback playing or paused as it is.
	pub fn pick_device(&mut self, id: &str) -> Vec<Effect> {
		// A player Spotify hasn't listed yet is offered without an id.
		let this = id.is_empty()
			|| matches!(&self.connect.devices, DeviceList::Ready(list) if list.iter().any(|d| d.id == id && self.is_this_device(d)));
		if (this && self.active) || self.connect.remote.as_ref().is_some_and(|device| device.id == id) {
			return Vec::new();
		}
		if self.connect.session {
			return vec![Effect::Player(Command::Transfer { device: id.to_owned(), play: self.playing() })];
		}
		if this {
			return vec![Effect::Player(Command::Activate)];
		}
		vec![Effect::Library(Request::Transfer { id: id.to_owned(), play: self.playing() })]
	}

	/// Takes in what `GET /me/player` said. `force`: the poll was sent after a transfer to
	/// another device, so it outranks this player's idea of being the active device. Returns
	/// whether anything visible changed.
	pub fn apply_playback(&mut self, result: Result<Option<Playback>, String>, force: bool) -> bool {
		if self.connect.session {
			return false;
		}
		let Ok(playback) = result else { return false };
		let Some(playback) = playback else {
			// No device is active anywhere: whatever was playing elsewhere has stopped.
			if self.connect.remote.take().is_none() {
				return false;
			}
			if self.status != Status::Idle {
				self.status = Status::Paused;
			}
			self.anchor_at(self.position_ms(), 0.0);
			return true;
		};
		if self.is_this_device(&playback.device) {
			// The engine reports this player's playback itself.
			return self.connect.remote.take().is_some();
		}
		if self.active && !force {
			return false;
		}
		let first = self.connect.remote.is_none();
		let mut changed = first || self.connect.remote.as_ref() != Some(&playback.device);
		self.active = false;
		if let Some(volume) = playback.device.volume_percent {
			changed |= self.volume != volume;
			self.volume = volume;
		}
		self.connect.remote = Some(playback.device.clone());
		if first {
			// The queue is this player's; the other device's isn't known.
			self.upcoming.clear();
		}

		let item = playback.item.as_ref().map(entity_of).unwrap_or_default();
		let status = match (playback.is_playing, item.uri.is_empty()) {
			(true, _) => Status::Playing,
			(false, false) => Status::Paused,
			(false, true) => Status::Idle,
		};
		let speed = if status == Status::Playing { 1.0 } else { 0.0 };
		let moved = self.position_ms().abs_diff(playback.progress_ms) > DRIFT_MS;
		if item.uri != self.item.uri || status != self.status || moved || first {
			self.item = item;
			self.status = status;
			self.anchor_at(playback.progress_ms, speed);
			changed = true;
		}
		let options = Options { shuffle: playback.shuffle, repeat: repeat_of(playback.repeat), ..Options::default() };
		changed |= options.shuffle != self.options.shuffle || options.repeat != self.options.repeat;
		self.options = options;
		let context = context_of(&playback, &self.library.playlists);
		changed |= context.uri != self.context.uri;
		self.context = context;
		changed
	}

	fn anchor_at(&mut self, position_ms: u64, speed: f64) {
		self.anchor = Anchor { position_ms, at: Instant::now(), speed };
	}

	/// Applies the authenticated session's pushed device inventory and account player.
	/// A paused startup player stays passive: it appears in the bar, not as remote playback.
	pub fn apply_connect_state(
		&mut self,
		devices: Vec<Device>,
		playback: Option<crate::engine::ClusterPlayback>,
		local_active: bool,
	) -> bool {
		self.connect.session = true;
		self.connect.devices = DeviceList::Ready(devices);
		if local_active {
			return true;
		}
		self.active = false;
		let Some(player) = playback else {
			if self.connect.remote.take().is_some() {
				self.playback_status(Status::Paused);
			}
			self.trace_playback("connect-state");
			return true;
		};
		if player.status == Status::Playing || (self.connect.remote.is_some() && player.device.is_some()) {
			self.connect.remote = player.device;
			if let Some(volume) = self.connect.remote.as_ref().and_then(|device| device.volume_percent) {
				self.volume = volume;
			}
			super::playback::refresh_entity(&mut self.item, player.playback.item);
			super::playback::refresh_entity(&mut self.context, player.playback.context);
			self.options = player.playback.options;
			self.playback_status(player.status);
			self.playback_position(player.position);
			self.upcoming.clear();
		} else {
			self.connect.remote = None;
			if !self.playback_pending() {
				self.status = Status::Paused;
			}
			self.restore_playback(player.playback);
		}
		self.trace_playback("connect-state");
		true
	}

	/// Forgets the other device and the device list when the account goes away.
	pub fn drop_remote_if_signed_out(&mut self) {
		if !self.web_ready() && !self.connect.session {
			self.connect.remote = None;
			self.connect.devices = DeviceList::Unknown;
			self.connect.menu = false;
		}
	}

	/// While another device plays, controls use native Connect commands (no-session API fallback).
	/// `None` for messages that are not about playback (or that stay local: taking playback
	/// back to this player).
	pub fn remote_update(&mut self, msg: &Msg) -> Option<Vec<Effect>> {
		let device = self.connect.remote.clone()?;
		let send = |command: PlayerCommand| vec![Effect::Library(Request::Control { device: device.id.clone(), command, revision: 0 })];
		let mut effects = match msg {
			Msg::TogglePlay => send(if self.playing() { PlayerCommand::Pause } else { PlayerCommand::Resume }),
			Msg::Play => send(PlayerCommand::Resume),
			Msg::Pause => send(PlayerCommand::Pause),
			Msg::Next => send(PlayerCommand::Next),
			Msg::Prev => send(PlayerCommand::Previous),
			Msg::Seek(position_ms) => {
				let d = self.duration_ms();
				if d == 0 {
					return Some(Vec::new());
				}
				self.seek_to((*position_ms).min(d), &send)
			},
			Msg::SeekBy(delta) => {
				if self.duration_ms() == 0 {
					return Some(Vec::new());
				}
				let pos = self.position_ms().saturating_add_signed(delta * 1000);
				self.seek_to(pos.min(self.duration_ms().saturating_sub(1000)), &send)
			},
			Msg::Volume(percent) => self.remote_volume(&device, (*percent).min(100), &send),
			Msg::ToggleMute => {
				let v = if self.volume > 0 {
					self.unmute_to = self.volume;
					0
				} else if self.unmute_to > 0 {
					self.unmute_to
				} else {
					50
				};
				self.remote_volume(&device, v, &send)
			},
			Msg::ToggleShuffle => {
				self.options.shuffle = !self.options.shuffle;
				send(PlayerCommand::Shuffle(self.options.shuffle))
			},
			Msg::CycleRepeat => {
				self.options.repeat = match self.options.repeat {
					Repeat::Off => Repeat::Context,
					Repeat::Context => Repeat::Track,
					Repeat::Track => Repeat::Off,
				};
				send(PlayerCommand::Repeat(match self.options.repeat {
					Repeat::Off => RepeatMode::Off,
					Repeat::Context => RepeatMode::Context,
					Repeat::Track => RepeatMode::Track,
				}))
			},
			_ => return None,
		};
		for effect in &mut effects {
			if let Effect::Library(Request::Control { command, revision, .. }) = effect {
				let transport = match command {
					PlayerCommand::Resume => Command::Play { uri: None },
					PlayerCommand::Pause => Command::Pause,
					PlayerCommand::Next => Command::SkipNext,
					PlayerCommand::Previous => Command::SkipPrev,
					_ => continue,
				};
				self.optimistic_playback(&transport);
				*revision = self.playback_command_id(&transport);
			}
		}
		Some(effects)
	}

	fn seek_to(&mut self, position_ms: u64, send: &dyn Fn(PlayerCommand) -> Vec<Effect>) -> Vec<Effect> {
		let speed = self.anchor.speed;
		self.anchor_at(position_ms, speed);
		send(PlayerCommand::Seek(position_ms))
	}

	fn remote_volume(&mut self, device: &Device, volume: u8, send: &dyn Fn(PlayerCommand) -> Vec<Effect>) -> Vec<Effect> {
		if !device.volume_capable() {
			self.show_toast(format!("{} sets its own volume.", device.name));
			return Vec::new();
		}
		self.volume = volume;
		send(PlayerCommand::Volume(volume))
	}
}

fn repeat_of(mode: RepeatMode) -> Repeat {
	match mode {
		RepeatMode::Off => Repeat::Off,
		RepeatMode::Context => Repeat::Context,
		RepeatMode::Track => Repeat::Track,
	}
}

/// The playing context as the views know it: its URI, and its name when it is one of the user's
/// playlists or the track's album.
fn context_of(playback: &Playback, playlists: &[spotify::Playlist]) -> Entity {
	let Some(context) = &playback.context else { return Entity::default() };
	let name = match context.kind.as_str() {
		"playlist" => playlists.iter().find(|p| p.uri == context.uri).map(|p| p.name.clone()),
		"album" => playback.item.as_ref().and_then(|t| t.album.as_ref()).map(|a| a.name.clone()),
		_ => None,
	};
	Entity {
		uri:         context.uri.clone(),
		decorations: Decorations { identity: Identity { name: name.unwrap_or_default() }, ..Decorations::default() },
	}
}

/// A Web API track as the player entity the views are built on.
pub(crate) fn entity_of(track: &Track) -> Entity {
	let covers = |images: &[spotify::Image]| -> Vec<Cover> {
		[("small", 64), ("default", 300), ("large", 640)]
			.into_iter()
			.filter_map(|(size, px)| spotify::pick_image(images, px).map(|i| Cover { url: i.url.clone(), size: size.to_owned() }))
			.collect()
	};
	let parent = track.album.as_ref().map(|a| Related {
		entity: Box::new(Entity {
			uri:         a.uri.clone(),
			decorations: Decorations {
				identity: Identity { name: a.name.clone() },
				visual_identity: VisualIdentity { cover: covers(&a.images) },
				..Decorations::default()
			},
		}),
	});
	let creators = track
		.artists
		.iter()
		.map(|a| Related {
			entity: Box::new(Entity {
				uri:         a.uri.clone(),
				decorations: Decorations { identity: Identity { name: a.name.clone() }, ..Decorations::default() },
			}),
		})
		.collect();
	Entity {
		uri:         track.uri.clone(),
		decorations: Decorations {
			identity: Identity { name: track.name.clone() },
			visual_identity: VisualIdentity::default(),
			parent,
			creators,
			playback: PlaybackInfo { duration_ms: u64::from(track.duration_ms), content_ratings: Vec::new() },
		},
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn device(id: &str, name: &str, kind: &str, active: bool) -> Device {
		Device {
			id:              id.into(),
			name:            name.into(),
			is_active:       active,
			volume_percent:  Some(30),
			supports_volume: Some(true),
			kind:            kind.into(),
		}
	}

	fn playback(device: Device, playing: bool, progress_ms: u64, uri: &str) -> Playback {
		Playback {
			device,
			is_playing: playing,
			progress_ms,
			shuffle: false,
			repeat: RepeatMode::Off,
			context: None,
			item: Some(Track {
				uri:         uri.into(),
				name:        "Song".into(),
				duration_ms: 200_000,
				disc_number: 1,
				explicit:    false,
				artists:     vec![spotify::Artist {
					name:   "Band".into(),
					uri:    "spotify:artist:b".into(),
					images: Vec::new(),
					genres: Vec::new(),
				}],
				album:       None,
				is_local:    false,
				is_episode:  false,
			}),
		}
	}

	fn ready() -> State {
		let mut s = State { device_name: "turntable".into(), logged_in: true, ..State::default() };
		s.library.conn = Conn::Ready;
		s
	}

	fn requests(effects: Vec<Effect>) -> Vec<Request> {
		effects.into_iter().filter_map(|e| if let Effect::Library(r) = e { Some(r) } else { None }).collect()
	}

	#[test]
	fn another_device_playing_takes_over_the_display() {
		let mut s = ready();
		let phone = device("p", "Alex's phone", "Smartphone", true);
		assert!(s.apply_playback(Ok(Some(playback(phone.clone(), true, 5_000, "spotify:track:a"))), false));
		assert_eq!(s.remote().map(|d| d.name.as_str()), Some("Alex's phone"));
		assert!(s.playing() && !s.active);
		assert_eq!((s.item.name(), s.item.creators(), s.duration_ms()), ("Song", "Band".to_owned(), 200_000));
		assert_eq!(s.volume, 30);
		assert!(s.wants_poll());
		// The same state again changes nothing worth drawing.
		assert!(!s.apply_playback(Ok(Some(playback(phone, true, 5_050, "spotify:track:a"))), false));
	}

	#[test]
	fn a_poll_naming_this_player_or_nobody_ends_remote_play() {
		let mut s = ready();
		let phone = device("p", "Phone", "Smartphone", true);
		s.apply_playback(Ok(Some(playback(phone.clone(), true, 0, "spotify:track:a"))), false);
		assert!(s.apply_playback(Ok(Some(playback(device("t", "turntable", "Computer", true), true, 0, "spotify:track:a"))), false));
		assert!(s.remote().is_none());
		s.apply_playback(Ok(Some(playback(phone, true, 0, "spotify:track:a"))), false);
		assert!(s.apply_playback(Ok(None), false));
		assert!(s.remote().is_none() && !s.playing());
	}

	#[test]
	fn this_player_being_active_outranks_a_stale_poll_unless_forced() {
		let mut s = ready();
		s.active = true;
		let phone = device("p", "Phone", "Smartphone", true);
		assert!(!s.apply_playback(Ok(Some(playback(phone.clone(), true, 0, "spotify:track:a"))), false));
		assert!(s.remote().is_none());
		assert!(s.apply_playback(Ok(Some(playback(phone, true, 0, "spotify:track:a"))), true));
		assert!(s.remote().is_some() && !s.active);
	}

	#[test]
	fn controls_drive_the_other_device() {
		let mut s = ready();
		let phone = device("p", "Phone", "Smartphone", true);
		s.apply_playback(Ok(Some(playback(phone, true, 0, "spotify:track:a"))), false);
		let sent = |effects| {
			requests(effects)
				.into_iter()
				.filter_map(|r| if let Request::Control { device, command, .. } = r { Some((device, command)) } else { None })
				.collect::<Vec<_>>()
		};
		assert_eq!(sent(s.update(Msg::TogglePlay)), vec![("p".to_owned(), PlayerCommand::Pause)]);
		assert!(!s.playing());
		assert_eq!(sent(s.update(Msg::TogglePlay)), vec![("p".to_owned(), PlayerCommand::Resume)]);
		assert_eq!(sent(s.update(Msg::Next)), vec![("p".to_owned(), PlayerCommand::Next)]);
		assert_eq!(sent(s.update(Msg::Seek(100_000))), vec![("p".to_owned(), PlayerCommand::Seek(100_000))]);
		assert_eq!(sent(s.update(Msg::Volume(50))), vec![("p".to_owned(), PlayerCommand::Volume(50))]);
		assert_eq!(sent(s.update(Msg::ToggleShuffle)), vec![("p".to_owned(), PlayerCommand::Shuffle(true))]);
		assert_eq!(sent(s.update(Msg::CycleRepeat)), vec![("p".to_owned(), PlayerCommand::Repeat(RepeatMode::Context))]);
		// Taking playback back uses the engine.
		s.apply_devices(Ok(vec![device("t", "turntable", "Computer", false), device("p", "Phone", "Smartphone", true)]));
		assert!(s.update(Msg::PickDevice("t".into())).iter().any(|e| matches!(e, Effect::Player(Command::Activate))));
	}

	#[test]
	fn a_device_without_volume_keeps_its_own() {
		let mut s = ready();
		let mut tv = device("tv", "TV", "TV", true);
		tv.volume_percent = None;
		tv.supports_volume = Some(false);
		s.apply_playback(Ok(Some(playback(tv, true, 0, "spotify:track:a"))), false);
		assert!(requests(s.update(Msg::Volume(25))).is_empty());
		assert!(s.toast.is_some());
	}

	#[test]
	fn the_menu_lists_others_and_transfers_keep_the_playing_state() {
		let mut s = ready();
		s.status = Status::Playing;
		s.active = true;
		assert_eq!(requests(s.update(Msg::DevicesMenu)).len(), 1, "opening lists the devices again");
		assert!(s.connect.menu);
		s.apply_devices(Ok(vec![
			device("t", "turntable", "Computer", true),
			device("p", "Phone", "Smartphone", false),
			device("k", "Kitchen", "Speaker", false),
		]));
		let others = s.other_devices();
		assert_eq!(others.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["Phone", "Kitchen"]);
		let sent = requests(s.update(Msg::PickDevice("k".into())));
		assert!(matches!(sent.as_slice(), [Request::Transfer { id, play: true }] if id == "k"));
		assert!(!s.connect.menu, "a pick closes the menu");
	}

	#[test]
	fn picking_this_player_activates_it_through_the_engine() {
		let mut s = ready();
		s.apply_playback(Ok(Some(playback(device("p", "Phone", "Smartphone", true), true, 0, "spotify:track:a"))), false);
		s.apply_devices(Ok(vec![device("t", "turntable", "Computer", false), device("p", "Phone", "Smartphone", true)]));
		assert_eq!(s.other_devices().iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["turntable"]);
		assert!(s.update(Msg::PickDevice("t".into())).iter().any(|e| matches!(e, Effect::Player(Command::Activate))));
	}

	#[test]
	fn a_device_list_naming_an_active_other_device_asks_for_a_probe() {
		let mut s = ready();
		assert!(s.apply_devices(Ok(vec![device("p", "Phone", "Smartphone", true)])));
		assert!(!s.apply_devices(Ok(vec![device("t", "turntable", "Computer", true)])));
		assert!(!s.apply_devices(Err("x".into())));
		assert!(matches!(s.connect.devices, DeviceList::Failed(_)));
	}

	fn native_player(device: Option<Device>, status: Status, position_ms: u64) -> crate::engine::ClusterPlayback {
		let track: Track = serde_json::from_value(serde_json::json!({
			"uri": "spotify:track:last", "name": "Last song", "duration_ms": 248_000,
			"artists": [{"uri":"spotify:artist:band", "name":"Band"}],
			"album": {"uri":"spotify:album:album", "name":"Album", "images":[{"url":"https://cover", "height":300}]}
		}))
		.unwrap();
		crate::engine::ClusterPlayback {
			playback: crate::engine::LastPlayback {
				item: entity_of(&track),
				context: Entity { uri: "spotify:playlist:context".into(), ..Entity::default() },
				position_ms,
				options: Options::default(),
			},
			device,
			position: crate::engine::Position {
				position_ms:  position_ms as f64,
				timestamp_ms: 0.0,
				speed:        if status == Status::Playing { 1.0 } else { 0.0 },
			},
			status,
		}
	}

	#[test]
	fn native_paused_bar_restores_all_projections_without_activation_or_web_effects() {
		let mut state = ready();
		let desktop = device("desktop", "Spotify desktop", "Computer", true);
		state.apply_connect_state(vec![desktop.clone()], Some(native_player(Some(desktop), Status::Paused, 2_000)), false);
		assert_eq!(state.item.uri, "spotify:track:last");
		assert_eq!(state.item.name(), "Last song");
		assert_eq!(state.item.creators(), "Band");
		assert_eq!(state.cover_url(), Some("https://cover"));
		assert_eq!(state.context.uri, "spotify:playlist:context");
		assert_eq!(state.duration_ms(), 248_000);
		assert_eq!(state.status, Status::Paused);
		assert_eq!(state.anchor.position_ms, 2_000);
		assert_eq!(state.anchor.speed, 0.0);
		assert!(!state.active);
		assert!(state.remote().is_none());
		assert!(!state.may_probe());
		assert!(!state.wants_poll());
		assert!(state.update(Msg::DevicesMenu).is_empty());
		assert!(
			matches!(state.update(Msg::TogglePlay).as_slice(), [Effect::Player(Command::Load(spotify::PlayStart::Context { uri, offset_uri: Some(track), position_ms: 2_000 }))] if uri == "spotify:playlist:context" && track == "spotify:track:last")
		);
	}

	#[test]
	fn native_remote_updates_and_transfers_do_not_request_device_or_player_reads() {
		let mut state = ready();
		let phone = device("phone", "Phone", "Smartphone", true);
		let speaker = device("speaker", "Speaker", "Speaker", false);
		state.apply_connect_state(
			vec![phone.clone(), speaker.clone()],
			Some(native_player(Some(phone.clone()), Status::Playing, 17_000)),
			false,
		);
		assert_eq!(state.remote().map(|device| device.name.as_str()), Some("Phone"));
		assert!(state.playing());
		assert!(!state.wants_poll());
		assert!(state.refresh_devices().is_empty());
		assert!(
			matches!(state.pick_device("speaker").as_slice(), [Effect::Player(Command::Transfer { device, play: true })] if device == "speaker")
		);
		state.apply_connect_state(vec![phone.clone(), speaker], Some(native_player(Some(phone), Status::Paused, 18_000)), false);
		assert_eq!(state.remote().map(|device| device.id.as_str()), Some("phone"));
		assert_eq!(state.anchor.position_ms, 18_000);
		assert!(matches!(state.pick_device("speaker").as_slice(), [Effect::Player(Command::Transfer { play: false, .. })]));
		assert!(!state.apply_devices(Err("old Web API quota response".into())));
		assert!(matches!(state.connect.devices, DeviceList::Ready(_)));
		assert!(!state.apply_playback(Ok(None), false));
		assert!(state.remote().is_some());
	}
}
