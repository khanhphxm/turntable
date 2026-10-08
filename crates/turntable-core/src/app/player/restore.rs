//! Startup restore orchestration and cheap position checkpoints on the existing worker.

use std::time::{Duration, Instant};

use super::Controller;
use crate::{
	engine::{LastPlayback, Status},
	library::{self, PlaybackPosition},
};

const CHECKPOINT_EVERY: Duration = Duration::from_secs(5);

pub(super) struct Restore {
	pub(super) pending:      bool,
	pub(super) idle_cluster: bool,
	account:                 Option<String>,
	fallback:                bool,
	requested:               bool,
	remembered:              Option<PlaybackPosition>,
	remember_at:             Instant,
}

impl Restore {
	pub(super) fn new(signed_in: bool) -> Self {
		Self {
			pending:      signed_in,
			idle_cluster: false,
			account:      None,
			fallback:     !signed_in,
			requested:    false,
			remembered:   None,
			remember_at:  Instant::now(),
		}
	}
}

impl Controller {
	pub(super) fn on_connect_state(&mut self, update: crate::engine::ConnectState) -> bool {
		let startup = self.restore.pending || update.initial;
		self.restore.pending = false;
		self.restore.idle_cluster = !update.playback.as_ref().is_some_and(|player| player.status == Status::Playing);
		self.restore.fallback = startup && update.playback.is_none() && !update.local_active;
		self.restore.account = Some(update.account);
		let was_item = self.last_item.clone();
		let changed = self.state.apply_connect_state(update.devices, update.playback, update.local_active);
		self.item_changed(&was_item);
		self.remember_playback(!self.state.playing());
		self.request_last_playback();
		changed
	}

	pub(super) fn on_last_playback(&mut self, account: String, playback: Option<LastPlayback>, playing_elsewhere: bool) -> bool {
		self.restore.pending = false;
		self.restore.idle_cluster = !playing_elsewhere;
		self.restore.account = (!account.is_empty()).then_some(account);
		self.restore.fallback = playback.is_none() && !playing_elsewhere;
		let was_item = self.last_item.clone();
		let changed = playback.is_some_and(|playback| self.state.restore_playback(playback));
		self.item_changed(&was_item);
		self.remember_playback(true);
		self.request_last_playback();
		if playing_elsewhere {
			self.probe();
		}
		changed
	}

	pub(super) fn restore_without_session(&mut self) {
		self.restore.pending = false;
		self.restore.fallback = self.state.item.uri.is_empty();
		self.request_last_playback();
	}

	pub(super) fn request_last_playback(&mut self) {
		if self.restore.pending || !self.restore.fallback || self.restore.requested || !self.state.web_ready() {
			return;
		}
		self.restore.requested = true;
		self.library.send(library::Request::LastPlayback);
	}

	pub(super) fn on_recent_playback(&mut self, account: String, playback: Option<LastPlayback>) -> bool {
		if self.restore.account.as_ref().is_some_and(|native| *native != account) || self.restore.pending || !self.restore.fallback {
			return false;
		}
		self.restore.account = Some(account);
		self.restore.fallback = false;
		let was_item = self.last_item.clone();
		let changed = playback.is_some_and(|playback| self.state.restore_playback(playback));
		self.item_changed(&was_item);
		self.remember_playback(true);
		changed
	}

	pub(super) fn remember_playback(&mut self, force: bool) {
		let Some(account) = &self.restore.account else { return };
		if self.state.item.uri.is_empty() || self.state.status == Status::Idle || (!force && Instant::now() < self.restore.remember_at) {
			return;
		}
		let position_ms = self.state.position_ms();
		self.restore.remember_at = Instant::now() + CHECKPOINT_EVERY;
		if self.restore.remembered.as_ref().is_some_and(|saved| {
			saved.account == *account
				&& saved.uri == self.state.item.uri
				&& saved.context_uri == self.state.context.uri
				&& saved.position_ms == position_ms
		}) {
			return;
		}
		let position = PlaybackPosition {
			account: account.clone(),
			uri: self.state.item.uri.clone(),
			context_uri: self.state.context.uri.clone(),
			position_ms,
		};
		self.library.send(library::Request::RememberPlayback(position.clone()));
		self.restore.remembered = Some(position);
	}

	pub(super) fn checkpoint_deadline(&self) -> Option<Instant> {
		(self.state.playing() && self.restore.account.is_some()).then_some(self.restore.remember_at)
	}
}
