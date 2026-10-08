//! Route-independent startup restoration and cheap position checkpoints on the existing worker.

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
		self.state.recent_searches.load(&self.paths.config, Some(update.account.clone()));
		self.restore.account = Some(update.account);
		let was_item = self.last_item.clone();
		let changed = self.state.apply_connect_state(update.devices, update.playback, update.local_active);
		self.item_changed(&was_item);
		self.remember_playback(!self.state.playing());
		self.request_last_playback();
		changed
	}

	pub(super) fn on_restore_unavailable(&mut self, account: String) {
		self.restore.pending = false;
		self.state.recent_searches.load(&self.paths.config, (!account.is_empty()).then(|| account.clone()));
		self.restore.account = (!account.is_empty()).then_some(account);
		self.restore.fallback = self.state.item.uri.is_empty();
		self.request_last_playback();
		self.probe();
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

#[cfg(test)]
mod tests {
	use std::sync::Arc;

	use super::*;
	use crate::{
		Msg, Options, Paths,
		app::Effect,
		engine::{ClusterPlayback, Command, ConnectState, Entity, Options as PlaybackOptions, Position},
	};

	fn controller() -> (Controller, tempfile::TempDir) {
		let root = tempfile::tempdir().unwrap();
		let paths = Paths { config: root.path().join("config"), cache: root.path().join("cache") };
		let controller = Controller::new(&paths, Options { web_api: false, ..Options::default() }, Arc::new(|| {})).unwrap();
		(controller, root)
	}

	fn playback() -> LastPlayback {
		let mut item = Entity { uri: "spotify:track:last".into(), ..Entity::default() };
		item.decorations.playback.duration_ms = 248_000;
		LastPlayback {
			item,
			context: Entity { uri: "spotify:playlist:context".into(), ..Entity::default() },
			position_ms: 2_000,
			options: PlaybackOptions::default(),
		}
	}

	#[test]
	fn native_startup_restore_never_requests_recent_history_or_web_player() {
		let (mut controller, _root) = controller();
		controller.state.library.conn = library::Conn::Ready;
		controller.restore.pending = true;
		controller.on_connect_state(ConnectState {
			account:      "alex".into(),
			devices:      Vec::new(),
			local_active: false,
			initial:      true,
			playback:     Some(ClusterPlayback {
				playback: playback(),
				device:   None,
				position: Position::default(),
				status:   Status::Paused,
			}),
		});
		assert!(!controller.restore.pending);
		assert!(!controller.restore.requested, "the session supplied the last player, so no recent-history fallback");
		assert!(!controller.poll_inflight);
		assert!(controller.poll_at.is_none());
		assert_eq!(controller.state.anchor.position_ms, 2_000);
		assert_eq!(controller.state.anchor.speed, 0.0);
		assert_eq!(controller.state.status, Status::Paused);
		assert!(!controller.state.active);
		assert!(matches!(controller.state.update(Msg::TogglePlay).as_slice(), [Effect::Player(Command::Load(
			crate::spotify::PlayStart::Context { position_ms: 2_000, .. }
		))]));
	}

	#[test]
	fn recent_fallback_waits_for_the_session_then_stays_bound_to_its_account() {
		let (mut controller, _root) = controller();
		controller.state.library.conn = library::Conn::Ready;
		controller.restore.pending = true;
		controller.request_last_playback();
		assert!(!controller.restore.requested);
		controller.on_restore_unavailable("alex".into());
		assert!(controller.restore.requested);
		assert!(!controller.on_recent_playback("other".into(), Some(playback())));
		assert!(controller.state.item.uri.is_empty());
		assert!(controller.on_recent_playback("alex".into(), Some(playback())));
		assert_eq!(controller.state.item.uri, "spotify:track:last");
		assert_eq!(controller.state.position_ms(), 2_000);
		assert!(!controller.state.active);
	}

	#[test]
	fn failed_page_load_preserves_restored_playback_and_does_not_cancel_a_late_restore() {
		for error_first in [false, true] {
			let (mut controller, _root) = controller();
			controller.state.library.conn = library::Conn::Ready;
			controller.restore.pending = true;
			controller.on_connect_state(ConnectState {
				account:      "alex".into(),
				devices:      Vec::new(),
				local_active: false,
				initial:      true,
				playback:     None,
			});
			let route = library::Route::Album("missing".into());
			controller.state.update(Msg::Navigate(route.clone()));
			let failed_page = || {
				super::super::AppEvent::Library(
					0,
					Box::new(library::Response::Error {
						route:   Some(route.clone()),
						message: "Can't reach Spotify. Check your connection.".into(),
					}),
				)
			};
			if error_first {
				controller.on_event(failed_page());
			}
			controller.on_event(super::super::AppEvent::Library(
				0,
				Box::new(library::Response::LastPlayback { account: "alex".into(), playback: Some(playback()) }),
			));
			let anchor_at = controller.state.anchor.at;
			controller.on_event(super::super::AppEvent::Library(0, Box::new(library::Response::Offline(true))));
			controller.on_event(failed_page());
			assert_eq!(controller.state.item.uri, "spotify:track:last");
			assert_eq!(controller.state.context.uri, "spotify:playlist:context");
			assert_eq!(controller.state.position_ms(), 2_000);
			assert_eq!(controller.state.duration_ms(), 248_000);
			assert_eq!(controller.state.anchor.at, anchor_at);
			assert_eq!(controller.state.anchor.speed, 0.0);
			assert_eq!(controller.state.status, Status::Paused);
			assert!(!controller.state.library.listing.as_ref().unwrap().loading);
			assert!(controller.state.library.error.is_some());
		}
	}
}
