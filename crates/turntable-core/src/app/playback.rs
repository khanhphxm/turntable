//! Player events, the local playback clock and transport state used by queue edits.

use std::time::Instant;

use super::{Anchor, State};
use crate::engine::{Command, Event, Status};

impl State {
	pub(super) fn anchor_to(&mut self, position_ms: u64, speed: f64) {
		self.anchor = Anchor { position_ms, at: Instant::now(), speed };
	}

	/// Restores the account's stopped player for the bar, without claiming Connect playback.
	/// The frozen anchor is the position used by the next [`crate::Msg::TogglePlay`].
	/// Live playback, here or on another device, always outranks a late startup read.
	pub fn restore_playback(&mut self, playback: crate::engine::LastPlayback) -> bool {
		if self.active || self.playing() || self.connect.remote.is_some() || playback.item.uri.is_empty() {
			return false;
		}
		self.item = playback.item;
		self.context = playback.context;
		self.options = playback.options;
		self.status = Status::Paused;
		let duration_ms = self.duration_ms();
		self.anchor_to(if duration_ms > 0 { playback.position_ms.min(duration_ms) } else { playback.position_ms }, 0.0);
		true
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
		match event {
			Event::AuthState { logged_in, is_active, device_name } => {
				self.logged_in = logged_in;
				self.active = is_active;
				self.device_name = device_name;
			},
			Event::TrackChanged { item } => self.item = item,
			Event::PlaybackChanged { status } => {
				let pos = self.position_ms();
				self.status = status;
				// Freeze or resume the local clock now; position_sync refines it.
				self.anchor_to(pos, if status == Status::Playing { self.anchor.speed.max(1.0) } else { 0.0 });
			},
			Event::VolumeChanged { volume } => self.volume = volume,
			Event::DeviceChanged { is_active } => self.active = is_active,
			Event::ContextChanged { context } => self.context = context,
			Event::OptionsChanged { options } => self.options = options,
			Event::PositionSync { position } => {
				self.anchor = Anchor::new(position.position_ms, position.timestamp_ms, position.speed);
			},
			Event::QueueChanged { upcoming, revision } => {
				if self.queue_edit.reconcile(revision) {
					self.upcoming = upcoming;
				}
			},
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
		Command::SetVolume { volume }
	}

	/// Clears playback state after the engine stops.
	pub fn disconnected(&mut self, reason: String) {
		self.connected = false;
		self.logged_in = false;
		self.active = false;
		self.connect.session = false;
		self.status = Status::Idle;
		self.queue_edit.cancel();
		self.anchor_to(self.position_ms(), 0.0);
		self.notice = Some(reason);
	}
}
