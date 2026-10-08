//! Native player-command delivery, including immediate queue mutations.

use super::Controller;
use crate::engine::Command;

impl Controller {
	pub(super) fn send_player(&mut self, cmd: Command) {
		let id = self.state.playback_command_id(&cmd);
		let queue_edit = matches!(cmd, Command::Load(_) | Command::EditQueue { .. });
		if self.client.is_none() && id == 0 && !queue_edit {
			return;
		}
		let result =
			self.client.as_ref().map_or_else(|| Err(std::io::Error::other("The player isn't running yet.")), |client| client.send(cmd, id));
		if let Err(error) = result {
			self.state.playback_failed(id);
			if queue_edit {
				self.state.queue_edit.cancel();
			}
			self.state.show_toast(error.to_string());
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		Msg, Paths,
		app::{Anchor, State},
		engine::{Incoming, Status},
	};

	#[test]
	fn missing_engine_rolls_back_the_optimistic_flip_synchronously() -> Result<(), Box<dyn std::error::Error>> {
		let root = tempfile::tempdir()?;
		let paths = Paths { config: root.path().join("config"), cache: root.path().join("cache") };
		let state =
			State { active: true, status: Status::Paused, anchor: Anchor { position_ms: 7_000, ..Anchor::default() }, ..State::default() };
		let mut controller = Controller::fixture(&paths, state)?;
		controller.dispatch(Msg::Play);
		assert!(!controller.state().playing());
		assert_eq!(controller.state().position_ms(), 7_000);
		assert!(controller.state().toast.as_ref().is_some_and(|(_, message)| message.contains("isn't running")));
		Ok(())
	}

	#[test]
	fn queued_engine_failure_rolls_back_on_the_same_pump_without_waiting_for_a_deadline() -> Result<(), Box<dyn std::error::Error>> {
		let root = tempfile::tempdir()?;
		let paths = Paths { config: root.path().join("config"), cache: root.path().join("cache") };
		let mut controller = Controller::fixture(&paths, State { active: true, status: Status::Paused, ..State::default() })?;
		controller.state.update(Msg::Play);
		let id = controller.state.playback_command_id(&Command::Play { uri: None });
		assert!(controller.state().playing());
		controller.tx.send(super::super::AppEvent::Player(controller.account_generation, Incoming::CommandFailed {
			id,
			message: "rejected".into(),
		}))?;
		assert!(controller.pump());
		assert!(!controller.state().playing());
		assert_eq!(controller.state().toast.as_ref().map(|(_, message)| message.as_str()), Some("rejected"));
		Ok(())
	}
}
