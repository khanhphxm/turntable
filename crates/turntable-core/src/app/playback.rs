//! Player events, the local playback clock and transport state used by queue edits.

use std::time::Instant;

use super::{Anchor, State};
use crate::engine::{Command, Entity, Event, Position, Status};

#[derive(Debug, Clone, Copy)]
struct Snapshot {
	status: Status,
	anchor: Anchor,
}

#[derive(Debug)]
struct Pending {
	id:        u64,
	desired:   Status,
	observed:  Status,
	confirmed: Snapshot,
}

#[derive(Debug, Default)]
pub(crate) struct Transport {
	revision: u64,
	pending:  Option<Pending>,
}

/// A transport-only cluster update must not erase display data already fetched for this URI.
pub(super) fn refresh_entity(current: &mut Entity, update: Entity) {
	if current.uri == update.uri && update.name().is_empty() && !current.name().is_empty() {
		if update.duration_ms() > 0 {
			current.decorations.playback.duration_ms = update.duration_ms();
		}
	} else {
		*current = update;
	}
}

impl State {
	pub(super) fn anchor_to(&mut self, position_ms: u64, speed: f64) {
		self.anchor = Anchor { position_ms, at: Instant::now(), speed };
	}

	/// Predicts only a user-requested transport change; engine state remains authoritative.
	pub(super) fn optimistic_playback(&mut self, command: &Command) {
		// An idempotent Pause still reaches the engine (the bar may be stale), but
		// an already-stopped passive bar has no speculative change to roll back.
		if matches!(command, Command::Pause)
			&& self.transport.pending.is_none()
			&& !matches!(self.status, Status::Playing | Status::Buffering)
		{
			return;
		}
		let position = self.position_ms();
		let (desired, position_ms, speed) = match command {
			Command::Play { uri: None } | Command::Activate => (Status::Playing, position, 1.0),
			Command::Play { uri: Some(_) } => (Status::Playing, 0, 0.0),
			Command::Pause => (Status::Paused, position, 0.0),
			Command::SkipNext | Command::SkipPrev => {
				let desired = if self.playing() || self.status == Status::Buffering { Status::Playing } else { self.status };
				(desired, 0, 0.0)
			},
			Command::Load(start) => {
				let position_ms = match start {
					crate::spotify::PlayStart::Context { position_ms, .. } | crate::spotify::PlayStart::List { position_ms, .. } => {
						*position_ms
					},
					crate::spotify::PlayStart::Tracks { .. } => 0,
				};
				(Status::Playing, position_ms, 0.0)
			},
			_ => return,
		};
		let confirmed =
			self.transport.pending.as_ref().map_or(Snapshot { status: self.status, anchor: self.anchor }, |pending| pending.confirmed);
		self.transport.revision = self.transport.revision.wrapping_add(1).max(1);
		self.transport.pending = Some(Pending { id: self.transport.revision, desired, observed: confirmed.status, confirmed });
		self.status = desired;
		self.anchor_to(position_ms, speed);
		self.trace_playback("optimistic-state");
	}

	pub(super) fn playback_command_id(&self, command: &Command) -> u64 {
		if matches!(
			command,
			Command::Activate | Command::Play { .. } | Command::Pause | Command::SkipNext | Command::SkipPrev | Command::Load(_)
		) {
			self.transport.pending.as_ref().map_or(0, |pending| pending.id)
		} else {
			0
		}
	}

	/// Stale failures and non-transport failures cannot undo a newer user action.
	pub(super) fn playback_failed(&mut self, id: u64) -> bool {
		if !self.transport.pending.as_ref().is_some_and(|pending| pending.id == id) {
			return false;
		}
		if let Some(pending) = self.transport.pending.take() {
			self.status = pending.confirmed.status;
			self.anchor = pending.confirmed.anchor;
		}
		self.trace_playback("rollback-state");
		true
	}

	pub(super) fn trace_playback(&self, stage: &str) {
		if crate::net::trace::enabled() {
			crate::net::trace::playback(stage, self.status.name(), Some(self.position_ms() as f64));
		}
	}

	pub(super) fn playback_status(&mut self, status: Status) {
		let position_ms = self.position_ms();
		let speed = if status == Status::Playing { self.anchor.speed.max(1.0) } else { 0.0 };
		if let Some(pending) = &mut self.transport.pending {
			pending.observed = status;
			if status != Status::Buffering {
				pending.confirmed = Snapshot { status, anchor: Anchor { position_ms, at: Instant::now(), speed } };
			}
			if status == pending.desired || status == Status::Idle {
				self.transport.pending = None;
			}
			if status != Status::Buffering {
				self.status = status;
			}
		} else {
			self.status = status;
		}
		// Keep the optimistic icon during loading, but don't run a clock without audio.
		self.anchor_to(position_ms, if status == Status::Buffering || !self.playing() { 0.0 } else { speed.max(1.0) });
	}

	pub(super) fn playback_pending(&self) -> bool {
		self.transport.pending.is_some()
	}

	pub(super) fn playback_position(&mut self, position: Position) {
		let anchor = Anchor::new(position.position_ms, position.timestamp_ms, position.speed);
		self.anchor = anchor;
		if let Some(pending) = &mut self.transport.pending
			&& pending.observed != Status::Buffering
		{
			pending.confirmed.anchor = anchor;
		}
	}

	/// Restores the account's stopped player for the bar, without claiming Connect playback.
	/// The frozen anchor is the position used by the next [`crate::Msg::TogglePlay`].
	/// Live playback, here or on another device, always outranks a late startup read.
	pub fn restore_playback(&mut self, playback: crate::engine::LastPlayback) -> bool {
		if self.active || self.playing() || self.connect.remote.is_some() || playback.item.uri.is_empty() {
			return false;
		}
		refresh_entity(&mut self.item, playback.item);
		refresh_entity(&mut self.context, playback.context);
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
			Event::MetadataChanged { entity } => {
				if entity.uri == self.item.uri {
					refresh_entity(&mut self.item, entity);
				} else if entity.uri == self.context.uri {
					refresh_entity(&mut self.context, entity);
				} else {
					return false;
				}
			},
			Event::PlaybackChanged { status } => self.playback_status(status),
			Event::VolumeChanged { volume } => self.volume = volume,
			Event::DeviceChanged { is_active } => self.active = is_active,
			Event::ContextChanged { context } => self.context = context,
			Event::OptionsChanged { options } => self.options = options,
			Event::PositionSync { position } => self.playback_position(position),
			Event::QueueChanged { upcoming, revision } => {
				if self.queue_edit.reconcile(revision) {
					self.upcoming = upcoming;
				}
			},
		}
		self.trace_playback("state");
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
		self.transport.pending = None;
		self.anchor_to(self.position_ms(), 0.0);
		self.notice = Some(reason);
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		Msg,
		app::Effect,
		engine::{ClusterPlayback, LastPlayback, Options},
		spotify::{Device, PlayStart},
	};

	fn state(status: Status) -> State {
		let mut state = State { active: true, status, ..State::default() };
		state.item.uri = "spotify:track:current".into();
		state.item.decorations.playback.duration_ms = 200_000;
		state.anchor_to(12_000, if status == Status::Playing { 1.0 } else { 0.0 });
		state
	}

	fn command(effects: &[Effect]) -> &Command {
		let Some(Effect::Player(command)) = effects.last() else { panic!("expected a player command") };
		command
	}

	#[test]
	fn toggle_flips_and_freezes_before_any_engine_event_and_failure_rolls_back() {
		let mut state = state(Status::Playing);
		let confirmed = state.anchor;
		let effects = state.update(Msg::TogglePlay);
		assert_eq!(command(&effects), &Command::Pause);
		assert!(!state.playing());
		assert_eq!(state.anchor.speed, 0.0);
		assert!(state.playback_failed(state.playback_command_id(command(&effects))));
		assert!(state.playing());
		assert_eq!(state.anchor.at, confirmed.at);
		assert_eq!(state.anchor.position_ms, confirmed.position_ms);
		assert_eq!(state.anchor.speed, 1.0);
	}

	#[test]
	fn rapid_toggles_send_opposite_commands_and_stale_failures_do_not_undo_the_latest() {
		let mut state = state(Status::Paused);
		let first = state.update(Msg::TogglePlay);
		let first_id = state.playback_command_id(command(&first));
		assert_eq!(command(&first), &Command::Play { uri: None });
		assert!(state.playing());
		assert_eq!(state.anchor.speed, 1.0);
		let second = state.update(Msg::TogglePlay);
		let second_id = state.playback_command_id(command(&second));
		assert_eq!(command(&second), &Command::Pause);
		assert!(second_id > first_id);
		assert!(!state.playing());
		assert!(!state.playback_failed(first_id));
		assert!(!state.playing());
		assert!(state.playback_failed(second_id));
		assert_eq!(state.status, Status::Paused);
		assert_eq!(state.position_ms(), 12_000);
	}

	#[test]
	fn buffering_keeps_the_requested_icon_but_only_playing_starts_the_confirmed_clock() {
		let mut state = state(Status::Paused);
		let effects = state.update(Msg::PlayContext("spotify:album:next".into()));
		let id = state.playback_command_id(command(&effects));
		assert!(state.playing());
		assert_eq!(state.position_ms(), 0);
		state.apply(Event::PlaybackChanged { status: Status::Buffering });
		state.apply(Event::PositionSync { position: Position { position_ms: 0.0, speed: 0.0, ..Position::default() } });
		assert!(state.playing());
		assert_eq!(state.anchor.speed, 0.0);
		state.apply(Event::PlaybackChanged { status: Status::Playing });
		state.apply(Event::PositionSync { position: Position { position_ms: 42.0, speed: 1.0, ..Position::default() } });
		assert!(state.playing());
		assert_eq!(state.anchor.position_ms, 42);
		assert_eq!(state.anchor.speed, 1.0);
		assert!(!state.playback_failed(id), "an acknowledged request cannot be rolled back later");
	}

	#[test]
	fn failed_load_after_buffering_restores_the_last_real_player_not_buffering() {
		let mut state = state(Status::Paused);
		let effects = state.update(Msg::PlayContext("spotify:album:missing".into()));
		let id = state.playback_command_id(command(&effects));
		state.apply(Event::PlaybackChanged { status: Status::Buffering });
		state.apply(Event::PositionSync { position: Position::default() });
		assert!(state.playback_failed(id));
		assert_eq!(state.status, Status::Paused);
		assert_eq!(state.position_ms(), 12_000);
	}

	#[test]
	fn next_resets_position_immediately_and_rolls_it_back_without_restarting_the_clock() {
		let mut state = state(Status::Playing);
		let before = state.anchor;
		let effects = state.update(Msg::Next);
		assert_eq!(command(&effects), &Command::SkipNext);
		assert!(state.playing());
		assert_eq!(state.position_ms(), 0);
		assert_eq!(state.anchor.speed, 0.0);
		assert!(state.playback_failed(state.playback_command_id(command(&effects))));
		assert_eq!(state.anchor.at, before.at);
		assert_eq!(state.anchor.position_ms, 12_000);
		assert_eq!(state.anchor.speed, 1.0);
	}

	#[test]
	fn explicit_media_play_and_pause_do_not_toggle_in_the_wrong_direction() {
		let mut state = state(Status::Paused);
		state.update(Msg::Play);
		assert!(state.playing());
		assert!(state.update(Msg::Play).is_empty());
		state.update(Msg::Pause);
		assert!(!state.playing());
		assert_eq!(command(&state.update(Msg::Pause)), &Command::Pause);
		assert!(!state.playing());
	}

	#[test]
	fn pausing_a_passive_bar_still_sends_pause_without_a_false_optimistic_revision() {
		let mut state = state(Status::Paused);
		state.active = false;
		let effects = state.update(Msg::Pause);
		assert_eq!(command(&effects), &Command::Pause);
		assert_eq!(state.playback_command_id(command(&effects)), 0);
		assert!(!state.playback_pending());
		assert_eq!(state.position_ms(), 12_000);
	}

	#[test]
	fn terminal_engine_events_reconcile_a_prediction_and_events_without_input_are_immediate() {
		let mut state = state(Status::Playing);
		state.update(Msg::Pause);
		state.apply(Event::PlaybackChanged { status: Status::Playing });
		state.apply(Event::PositionSync { position: Position { position_ms: 13_000.0, speed: 1.0, ..Position::default() } });
		assert!(state.playing(), "actual audio events outrank a prediction, including controls from another device");
		assert_eq!(state.anchor.position_ms, 13_000);
		assert_eq!(state.anchor.speed, 1.0);
		state.apply(Event::PlaybackChanged { status: Status::Paused });
		assert!(!state.playback_pending());
		state.apply(Event::PlaybackChanged { status: Status::Playing });
		assert!(state.playing());
		state.apply(Event::PlaybackChanged { status: Status::Paused });
		assert!(!state.playing());
		assert_eq!(state.anchor.speed, 0.0);
	}

	#[test]
	fn remote_control_rollback_uses_its_revision_and_cluster_pushes_reconcile_without_polling() {
		let mut state = state(Status::Playing);
		state.active = false;
		state.connect.session = true;
		let device = Device {
			id:              "phone".into(),
			name:            "Phone".into(),
			kind:            "Smartphone".into(),
			is_active:       true,
			volume_percent:  None,
			supports_volume: None,
		};
		state.connect.remote = Some(device.clone());
		let effects = state.update(Msg::Pause);
		let Some(Effect::Library(crate::library::Request::Control { revision, command: crate::spotify::PlayerCommand::Pause, .. })) =
			effects.first()
		else {
			panic!("expected a revision-tagged remote pause");
		};
		assert!(!state.playing());
		assert!(state.playback_failed(*revision));
		assert!(state.playing());
		state.apply_connect_state(
			vec![device.clone()],
			Some(ClusterPlayback {
				playback: LastPlayback {
					item:        state.item.clone(),
					context:     Entity::default(),
					position_ms: 15_000,
					options:     Options::default(),
				},
				device:   Some(device),
				status:   Status::Paused,
				position: Position { position_ms: 15_000.0, speed: 0.0, ..Position::default() },
			}),
			false,
		);
		assert!(!state.playing());
		assert_eq!(state.position_ms(), 15_000);
		assert!(!state.wants_poll());
	}

	#[test]
	fn restored_load_prediction_preserves_its_resume_position() {
		let mut state = state(Status::Paused);
		state.active = false;
		let effects = state.update(Msg::Play);
		assert!(matches!(command(&effects), Command::Load(PlayStart::List { position_ms: 12_000, .. })));
		assert!(state.playing());
		assert_eq!(state.position_ms(), 12_000);
	}
}
