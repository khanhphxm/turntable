//! Visibility-aware polling and responses for playback on other Spotify Connect devices.

use std::time::{Duration, Instant};

use super::{CONTROL_POLL, Controller, TRANSFER_POLL};
use crate::library;

impl Controller {
	/// Maps host focus/minimize state to visibility. Hidden windows do not poll
	/// remote playback; showing the window probes it immediately.
	pub fn set_visible(&mut self, visible: bool) {
		if visible == self.visible {
			return;
		}
		self.visible = visible;
		if visible {
			self.probe();
		} else {
			self.poll_at = None;
		}
	}

	/// Asks now what plays and where, if another device might be playing.
	pub(super) fn probe(&mut self) {
		if !self.restore.pending && !self.state.starting && self.state.may_probe() {
			self.poll_playback();
		}
	}

	/// Sends `GET /me/player` (one at a time, and only while the window shows).
	pub(super) fn poll_playback(&mut self) {
		if self.state.connect.session || self.poll_inflight || !self.visible || !self.state.web_ready() {
			return;
		}
		self.poll_inflight = true;
		self.poll_forced = std::mem::take(&mut self.force_poll);
		self.poll_at = None;
		self.library.send(library::Request::Playback);
	}

	/// A poll in `after`, unless one is due sooner (what a command did, once the device answers).
	pub(super) fn poll_soon(&mut self, after: Duration) {
		if self.visible && self.state.wants_poll() {
			let at = Instant::now() + after;
			self.poll_at = Some(self.poll_at.map_or(at, |t| t.min(at)));
		}
	}

	/// Keeps the polling schedule in step with the state: polls run only while another device
	/// plays and the window shows, every few seconds, further apart after failures.
	pub(super) fn reconcile_poll(&mut self) {
		if self.state.connect.session || !self.visible || !(self.state.wants_poll() || self.force_poll) {
			self.poll_at = None;
		} else if self.poll_at.is_none() && !self.poll_inflight {
			self.poll_at = Some(Instant::now() + crate::app::connect::poll_delay(self.poll_failures));
		}
	}

	/// An answer about devices or playback. Returns whether anything visible changed.
	pub(super) fn on_player_response(&mut self, response: library::Response) -> bool {
		match response {
			library::Response::Devices(result) => {
				if self.state.apply_devices(result) {
					self.probe();
				}
			},
			library::Response::Playback(result) => {
				self.poll_inflight = false;
				let forced = std::mem::take(&mut self.poll_forced);
				self.poll_failures = if result.is_ok() { 0 } else { self.poll_failures.saturating_add(1) };
				let was_item = self.last_item.clone();
				let changed = self.state.apply_playback(result, forced);
				self.remember_playback(!self.state.playing());
				self.item_changed(&was_item);
				return changed;
			},
			library::Response::Transferred => {
				// Spotify needs a moment to move; then the poll says where playback went.
				self.poll_at = Some(Instant::now() + TRANSFER_POLL);
			},
			library::Response::PlayerError(message) => {
				self.force_poll = false;
				self.state.show_toast(message);
				self.poll_soon(CONTROL_POLL);
			},
			library::Response::ControlFailed { revision, message } => {
				self.state.playback_failed(revision);
				self.state.show_toast(message);
				self.poll_soon(CONTROL_POLL);
			},
			_ => return false,
		}
		true
	}
}
