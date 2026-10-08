//! Route-independent startup restoration and cheap position checkpoints on the existing worker.

use std::time::{Duration, Instant};

use super::Controller;
use crate::{
	engine::{LastPlayback, Status},
	library::{self, PlaybackPosition, QueuedTrack},
};

const CHECKPOINT_EVERY: Duration = Duration::from_secs(5);

pub(super) struct Restore {
	pub(super) pending:       bool,
	pub(super) idle_cluster:  bool,
	account:                  Option<String>,
	fallback:                 bool,
	requested:                bool,
	remembered:               Option<PlaybackPosition>,
	remember_at:              Instant,
	checkpoint_pending:       bool,
	pub(super) queue_changed: bool,
}

impl Restore {
	pub(super) fn new(signed_in: bool) -> Self {
		Self {
			pending:            signed_in,
			idle_cluster:       false,
			account:            None,
			fallback:           !signed_in,
			requested:          false,
			remembered:         None,
			remember_at:        Instant::now(),
			checkpoint_pending: false,
			queue_changed:      false,
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
		self.restore_account(update.account);
		let was_item = self.last_item.clone();
		let changed = self.state.apply_connect_state(update.devices, update.playback, update.local_active);
		let changed = self.restore_queue() || changed;
		self.item_changed(&was_item);
		self.remember_playback(!self.state.playing());
		self.request_last_playback();
		changed
	}

	pub(super) fn on_restore_unavailable(&mut self, account: String) {
		self.restore.pending = false;
		self.state.recent_searches.load(&self.paths.config, (!account.is_empty()).then(|| account.clone()));
		if !account.is_empty() {
			self.restore_account(account);
		}
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
		self.restore_account(account);
		self.restore.fallback = false;
		let was_item = self.last_item.clone();
		let changed = playback.is_some_and(|playback| self.state.restore_playback(playback));
		let changed = self.restore_queue() || changed;
		self.item_changed(&was_item);
		self.remember_playback(true);
		changed
	}

	fn restore_account(&mut self, account: String) {
		if self.restore.account.as_ref() == Some(&account) {
			return;
		}
		self.restore.account = Some(account.clone());
		self.restore.remembered = None;
		self.restore.checkpoint_pending = true;
		self.library.send(library::Request::PlaybackCheckpoint(account));
	}

	pub(super) fn on_playback_checkpoint(&mut self, account: &str, position: Option<PlaybackPosition>) -> bool {
		if self.restore.account.as_deref() != Some(account) || !self.restore.checkpoint_pending {
			return false;
		}
		self.restore.checkpoint_pending = false;
		self.restore.remembered = position.filter(|saved| saved.account == account);
		let changed = self.restore_queue();
		self.remember_playback(true);
		changed
	}

	/// Restore display data only. A later explicit Play loads the preserved occurrences;
	/// a live player, newer queue snapshot or user edit always wins over this disk read.
	fn restore_queue(&mut self) -> bool {
		let Some(saved) = &self.restore.remembered else { return false };
		if self.restore.queue_changed
			|| self.state.active
			|| self.state.playing()
			|| self.state.connect.remote.is_some()
			|| !self.state.upcoming.is_empty()
			|| saved.uri != self.state.item.uri
		{
			return false;
		}
		self.state.upcoming = saved.upcoming.iter().cloned().map(QueuedTrack::entry).collect();
		self.state.queue_restored = !self.state.upcoming.is_empty();
		self.state.queue_restored
	}

	pub(super) fn remember_playback(&mut self, force: bool) {
		let Some(account) = &self.restore.account else { return };
		if self.restore.checkpoint_pending
			|| self.state.item.uri.is_empty()
			|| self.state.status == Status::Idle
			|| (!force && Instant::now() < self.restore.remember_at)
		{
			return;
		}
		let position_ms = self.state.position_ms();
		self.restore.remember_at = Instant::now() + CHECKPOINT_EVERY;
		if self.restore.remembered.as_ref().is_some_and(|saved| {
			saved.account == *account
				&& saved.uri == self.state.item.uri
				&& saved.context_uri == self.state.context.uri
				&& saved.position_ms == position_ms
				&& saved.upcoming.len() == self.state.upcoming.len()
				&& saved
					.upcoming
					.iter()
					.zip(&self.state.upcoming)
					.all(|(saved, entry)| saved.uid == entry.uid && saved.uri == entry.item.uri && saved.source == entry.source)
		}) {
			return;
		}
		let position = PlaybackPosition {
			account: account.clone(),
			uri: self.state.item.uri.clone(),
			context_uri: self.state.context.uri.clone(),
			position_ms,
			upcoming: self
				.state
				.upcoming
				.iter()
				.map(|entry| QueuedTrack { uid: entry.uid.clone(), uri: entry.item.uri.clone(), source: entry.source.clone() })
				.collect(),
		};
		self.library.send(library::Request::RememberPlayback(position.clone()));
		self.restore.remembered = Some(position);
	}

	pub(super) fn checkpoint_deadline(&self) -> Option<Instant> {
		(self.state.playing() && self.restore.account.is_some() && !self.restore.checkpoint_pending).then_some(self.restore.remember_at)
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

	fn checkpoint() -> PlaybackPosition {
		PlaybackPosition {
			account:     "alex".into(),
			uri:         "spotify:track:last".into(),
			context_uri: "spotify:playlist:context".into(),
			position_ms: 2_000,
			upcoming:    vec![
				QueuedTrack { uid: "first".into(), uri: "spotify:track:duplicate".into(), source: "queue".into() },
				QueuedTrack { uid: "second".into(), uri: "spotify:track:duplicate".into(), source: "context".into() },
				QueuedTrack { uid: "third".into(), uri: "spotify:episode:next".into(), source: "autoplay".into() },
			],
		}
	}

	#[test]
	fn checkpoint_queue_waits_for_matching_account_and_track_without_autoplay() {
		let (mut controller, _root) = controller();
		controller.on_restore_unavailable("alex".into());
		assert!(!controller.on_playback_checkpoint("other", Some(checkpoint())));
		assert!(controller.restore.checkpoint_pending);
		assert!(!controller.on_playback_checkpoint("alex", Some(checkpoint())));
		assert!(controller.state.upcoming.is_empty(), "wait for the fallback's track identity");
		assert!(controller.on_recent_playback("alex".into(), Some(playback())));
		assert_eq!(
			controller
				.state
				.upcoming
				.iter()
				.map(|entry| (entry.uid.as_str(), entry.item.uri.as_str(), entry.source.as_str()))
				.collect::<Vec<_>>(),
			vec![
				("first", "spotify:track:duplicate", "queue"),
				("second", "spotify:track:duplicate", "context"),
				("third", "spotify:episode:next", "autoplay")
			]
		);
		assert_eq!(controller.state.status, Status::Paused);
		assert_eq!(controller.state.anchor.speed, 0.0);
		assert!(!controller.state.active);
		assert!(!controller.state.apply(crate::engine::Event::QueueChanged { upcoming: Vec::new(), revision: 0 }));
		let effects = controller.state.update(Msg::TogglePlay);
		let [Effect::Player(Command::Load(start))] = effects.as_slice() else { panic!("expected explicit resume") };
		assert!(matches!(start, crate::spotify::PlayStart::Restored { position_ms: 2_000, upcoming, context_uri, .. }
			if upcoming.len() == 3 && context_uri == "spotify:playlist:context"));
		controller.sign_out();
		assert!(controller.state.upcoming.is_empty());
		assert!(!controller.on_playback_checkpoint("alex", Some(checkpoint())));
		assert!(!controller.state.queue_restored);
	}

	#[test]
	fn a_newer_queue_edit_or_live_player_wins_over_the_disk_read() {
		for live in [false, true] {
			let (mut controller, _root) = controller();
			controller.on_restore_unavailable("alex".into());
			controller.state.restore_playback(playback());
			if live {
				controller.state.active = true;
			} else {
				controller.restore.queue_changed = true;
			}
			assert!(!controller.on_playback_checkpoint("alex", Some(checkpoint())));
			assert!(controller.state.upcoming.is_empty());
		}
	}

	#[test]
	fn passive_queue_edits_checkpoint_immediately_and_jump_loads_the_chosen_occurrence() {
		let (mut controller, _root) = controller();
		controller.on_restore_unavailable("alex".into());
		controller.state.restore_playback(playback());
		assert!(controller.on_playback_checkpoint("alex", Some(checkpoint())));
		controller.dispatch(Msg::QueueRemove(0));
		assert_eq!(controller.state.upcoming.len(), 2);
		assert!(!controller.state.queue_edit.busy());
		assert!(controller.state.toast.is_none(), "passive edits do not require a running player");
		assert_eq!(controller.restore.remembered.as_ref().unwrap().upcoming.len(), 2);
		assert_eq!(controller.state.status, Status::Paused);
		let effects = controller.state.update(Msg::QueueJump(0));
		assert!(matches!(effects.as_slice(), [Effect::Player(Command::Load(crate::spotify::PlayStart::Restored {
			uri, position_ms: 0, upcoming, ..
		}))] if uri == "spotify:track:duplicate" && upcoming.len() == 1 && upcoming.first().is_some_and(|entry| entry.0 == "third")));
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
