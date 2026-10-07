//! Queue-edit timing, rebuilds and the player commands that apply them.

use std::time::Instant;

use super::{Controller, debug_log};
use crate::{
	app::{Panel, queue},
	engine::Command,
};

impl Controller {
	/// Sends the player a command. A queue request marks the next event as its answer,
	/// rather than an unsolicited queue change.
	pub(super) fn send_player(&mut self, cmd: Command) {
		let Some(client) = &self.client else {
			if matches!(cmd, Command::Load(_)) {
				self.state.show_toast("The player isn't running yet.".into());
			}
			return;
		};
		if matches!(cmd, Command::GetQueue { .. }) {
			self.queue_reply = true;
		}
		client.send(cmd);
	}

	/// Sends the queue edit's due commands (volume back, repeat, pause, songs queued again).
	pub(super) fn send_queue_commands(&mut self) {
		for cmd in self.state.queue_edit.take_commands() {
			self.send_player(cmd);
		}
	}

	/// A queue event arrived. With the panel open, ask for the full list after an unsolicited
	/// change. Otherwise a queue edit has landed in the list currently shown.
	pub(super) fn queue_arrived(&mut self) {
		let asked = std::mem::take(&mut self.queue_reply);
		let wide = self.state.panel == Some(Panel::Queue) && self.state.logged_in;
		if !asked && wide && self.state.upcoming.len() >= queue::DEFAULT_QUEUE {
			self.send_player(Command::GetQueue { limit: queue::WIDE });
		} else {
			self.state.queue_edit.settled();
		}
	}

	/// The clicking on the queue paused: carry the edit out. Returns whether anything on screen changed.
	pub(super) fn rebuild_queue(&mut self) -> bool {
		if self.client.is_none() {
			self.state.queue_edit.cancel();
			self.state.show_toast("Couldn't change the queue: the player isn't connected.".into());
			return true;
		}
		let Some(plan) = self.state.queue_plan() else {
			self.state.show_toast("The queue can't be changed while this plays.".into());
			return true;
		};
		debug_log(&format!("queue rebuild: {} songs from {} ms, {} commands first", plan.uris.len(), plan.position_ms, plan.before.len()));
		for cmd in plan.before {
			self.send_player(cmd);
		}
		self.send_player(Command::Load(crate::spotify::PlayStart::List { uris: plan.uris, position_ms: plan.position_ms }));
		self.state.queue_edit.landed(Instant::now());
		true
	}

	/// Neither Spotify nor the player answered a queue edit: put the player back.
	pub(super) fn give_up_queue(&mut self) -> bool {
		let (ago, latency) = self.state.queue_edit.timing(Instant::now());
		debug_log(&format!("queue edit given up after {} ms (restart allowance {latency} ms)", ago.map_or(0, |d| d.as_millis())));
		if self.state.queue_edit.give_up() {
			self.state.show_toast("Couldn't change the queue: Spotify didn't answer.".into());
		}
		self.send_queue_commands();
		true
	}
}
