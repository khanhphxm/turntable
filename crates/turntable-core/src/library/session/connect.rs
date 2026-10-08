//! Worker Connect requests use the player's pushed cluster and native commands.
//! Missing/stopped session state is an error, never a reason to spend Web API quota.

use super::{Reader, network, tracks};
use crate::spotify::{self, Device, PlayContext, Playback, PlayerCommand, RepeatMode};

impl Reader {
	pub(in crate::library) fn devices(&self) -> spotify::Result<Vec<Device>> {
		self.shared.devices().map_err(network)
	}

	pub(in crate::library) fn playback(&self) -> spotify::Result<Option<Playback>> {
		let cluster = self.shared.cluster().ok_or_else(|| spotify::Error::Network("Spotify Connect state has not arrived yet".into()))?;
		let Some(info) = cluster.device.get(&cluster.active_device_id).filter(|info| !info.is_offline && info.can_play) else {
			return Ok(None);
		};
		let Some(player) = cluster.player_state.as_ref() else { return Ok(None) };
		let item = if let Some(track) = player.track.as_ref().filter(|track| !track.uri.is_empty()) {
			self.shared.block_on(tracks(self.shared.session(), std::slice::from_ref(&track.uri), None)).map_err(network)?.into_iter().next()
		} else {
			None
		};
		let options = player.options.as_ref();
		let repeat = if options.is_some_and(|options| options.repeating_track) {
			RepeatMode::Track
		} else if options.is_some_and(|options| options.repeating_context) {
			RepeatMode::Context
		} else {
			RepeatMode::Off
		};
		let context = if player.context_uri.is_empty() || player.context_uri == "-" {
			None
		} else {
			Some(PlayContext {
				uri:  player.context_uri.clone(),
				kind: player.context_uri.split(':').nth(1).unwrap_or_default().to_owned(),
			})
		};
		Ok(Some(Playback {
			device: crate::engine::device_of(&cluster.active_device_id, info, &cluster.active_device_id),
			is_playing: player.is_playing && !player.is_paused,
			progress_ms: u64::try_from(player.position_as_of_timestamp).unwrap_or_default(),
			shuffle: options.is_some_and(|options| options.shuffling_context),
			repeat,
			context,
			item,
		}))
	}

	pub(in crate::library) fn transfer(&self, device: &str, play: bool) -> spotify::Result<()> {
		self.shared.transfer(device, play).map_err(network)
	}

	pub(in crate::library) fn control(&self, device: &str, command: PlayerCommand) -> spotify::Result<()> {
		self.shared.control(device, command).map_err(network)
	}
}
