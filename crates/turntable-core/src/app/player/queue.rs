//! Native player-command delivery, including immediate queue mutations.

use super::Controller;
use crate::engine::Command;

impl Controller {
	pub(super) fn send_player(&mut self, cmd: Command) {
		let Some(client) = &self.client else {
			if matches!(cmd, Command::Load(_) | Command::EditQueue { .. }) {
				self.state.queue_edit.cancel();
				self.state.show_toast("The player isn't running yet.".into());
			}
			return;
		};
		client.send(cmd);
	}
}
