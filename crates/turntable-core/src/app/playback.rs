//! Player events, the local playback clock and transport state used by queue edits.

use std::time::Instant;

use super::{Anchor, State};
use crate::{
	app::queue,
	engine::{Command, Event, Status},
};

impl State {
	pub(super) fn anchor_to(&mut self, position_ms: u64, speed: f64) {
		self.anchor = Anchor { position_ms, at: Instant::now(), speed };
	}

	/// Applies a player event. Returns whether anything visible changed.
	pub fn apply(&mut self, event: Event) -> bool {
		// While another device plays, its state comes from the Web API; this player's
		// track and position are stale. Only this player becoming the active one ends that.
		if self.connect.remote.is_some() {
			match &event {
				Event::AuthState { is_active: true, .. } | Event::DeviceChanged { is_active: true } => self.connect.remote = None,
				Event::TrackChanged { .. }
				| Event::PlaybackChanged { .. }
				| Event::VolumeChanged { .. }
				| Event::ContextChanged { .. }
				| Event::OptionsChanged { .. }
				| Event::PositionSync { .. }
				| Event::QueueChanged { .. } => return false,
				_ => {},
			}
		}
		self.queue_edit.on_event(&event, Instant::now());
		match event {
			Event::AuthState { logged_in, is_active, device_name } => {
				self.logged_in = logged_in;
				self.active = is_active;
				self.device_name = device_name;
			},
			Event::TrackChanged { item } => {
				self.queue_edit.track_changed(&item.uri);
				self.item = item;
			},
			Event::PlaybackChanged { status } => {
				let pos = self.position_ms();
				self.status = status;
				// Freeze or resume the local clock now; position_sync refines it.
				self.anchor_to(pos, if status == Status::Playing { self.anchor.speed.max(1.0) } else { 0.0 });
			},
			Event::VolumeChanged { volume } => {
				// Muted for a queue rebuild: the volume on screen stays where it was.
				if !self.queue_edit.muting() {
					self.volume = volume;
				}
			},
			Event::DeviceChanged { is_active } => self.active = is_active,
			Event::ContextChanged { context } => self.context = context,
			Event::OptionsChanged { options } => self.options = options,
			Event::PositionSync { position } => {
				self.anchor = Anchor::new(position.position_ms, position.timestamp_ms, position.speed);
			},
			Event::QueueChanged { upcoming } => self.upcoming = upcoming,
		}
		self.notice = None;
		true
	}

	pub(super) fn seek(&mut self, pos: u64) -> Command {
		let speed = self.anchor.speed;
		self.anchor_to(pos, speed);
		Command::Seek { position_ms: pos }
	}

	pub(super) fn set_volume(&mut self, volume: u8) -> Command {
		self.volume = volume;
		self.queue_edit.volume_set(volume);
		Command::SetVolume { volume }
	}

	/// The rebuild for the waiting queue edit, made from what plays now.
	pub fn queue_plan(&mut self) -> Option<queue::Plan> {
		let (position_ms, duration_ms) = (self.position_ms(), self.duration_ms());
		let ctx = queue::Ctx {
			item: &self.item,
			status: self.status,
			position_ms,
			duration_ms,
			volume: self.volume,
			shuffle: self.options.shuffle,
			repeat: self.options.repeat,
			upcoming: &self.upcoming,
		};
		self.queue_edit.plan(&ctx, Instant::now())
	}

	/// Clears playback state after the engine stops.
	pub fn disconnected(&mut self, reason: String) {
		self.connected = false;
		self.logged_in = false;
		self.active = false;
		self.status = Status::Idle;
		self.anchor_to(self.position_ms(), 0.0);
		self.notice = Some(reason);
	}
}
