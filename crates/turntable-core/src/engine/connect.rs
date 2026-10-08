//! Account-wide Connect state is already returned at registration and pushed by Dealer.
//! The device menu, remote bar and paused startup restore need no Web API reads.

use librespot_protocol::{
	connect::{Cluster, DeviceInfo},
	devices::DeviceType,
};

use super::{ClusterPlayback, ConnectState, Forward, Incoming, LastPlayback, Options, Position, Repeat, Status};
use crate::spotify::Device;

impl Forward {
	pub(super) async fn cluster(&mut self, cluster: &Cluster, initial: bool) {
		let devices = devices_of(cluster);
		let local_active = cluster.active_device_id == self.session.device_id() && self.state.active;
		let playback = if !local_active {
			if let Some(player) = cluster.player_state.as_ref()
				&& let Some(track) = player.track.as_ref().filter(|track| !track.uri.is_empty())
			{
				let mut item = self.track(&track.uri).await;
				if item.duration_ms() == 0 {
					item.decorations.playback.duration_ms = u64::try_from(player.duration).unwrap_or_default();
				}
				let context = self.context(&player.context_uri).await;
				let device = devices.iter().find(|device| device.is_active && device.id != self.session.device_id()).cloned();
				let status = if device.is_some() && player.is_playing && !player.is_paused { Status::Playing } else { Status::Paused };
				let position_ms = u64::try_from(player.position_as_of_timestamp).unwrap_or_default();
				let options = player.options.as_ref();
				let shuffle = options.is_some_and(|options| options.shuffling_context);
				let repeat_context = options.is_some_and(|options| options.repeating_context);
				let repeat_track = options.is_some_and(|options| options.repeating_track);
				self.state.options = (shuffle, repeat_context, repeat_track);
				let repeat = if repeat_track {
					Repeat::Track
				} else if repeat_context {
					Repeat::Context
				} else {
					Repeat::Off
				};
				Some(ClusterPlayback {
					playback: LastPlayback { item, context, position_ms, options: Options { shuffle, repeat, smart_shuffle: false } },
					device,
					position: Position {
						position_ms:  position_ms as f64,
						timestamp_ms: player.timestamp as f64,
						speed:        if status == Status::Playing { player.playback_speed.max(1.0) } else { 0.0 },
					},
					status,
				})
			} else {
				None
			}
		} else {
			None
		};
		(self.deliver)(Incoming::ConnectState(ConnectState { account: self.session.username(), devices, playback, local_active, initial }));
	}

	pub(super) async fn transfer_device(&mut self, device: &str, play: bool) {
		let device = if device.is_empty() { self.session.device_id() } else { device };
		let request = librespot_core::spclient::TransferRequest {
			transfer_options: librespot_core::dealer::protocol::TransferOptions {
				restore_paused: Some(if play { "resume" } else { "pause" }.to_owned()),
				..Default::default()
			},
		};
		if let Err(error) =
			super::network_timeout(super::NETWORK_TIMEOUT, self.session.spclient().transfer(device, device, Some(&request))).await
		{
			(self.deliver)(Incoming::CommandFailed(format!("Couldn't transfer playback: {error}")));
		}
	}
}

fn devices_of(cluster: &Cluster) -> Vec<Device> {
	let mut devices: Vec<_> = cluster
		.device
		.iter()
		.filter(|(_, info)| !info.is_offline && info.can_play && !info.capabilities.hidden)
		.map(|(id, info)| device_of(id, info, &cluster.active_device_id))
		.collect();
	devices.sort_by(|left, right| left.id.cmp(&right.id));
	devices
}

fn device_of(id: &str, info: &DeviceInfo, active_id: &str) -> Device {
	let volume = u16::try_from(info.volume).unwrap_or(u16::MAX);
	let volume_capable = !info.capabilities.disable_volume && info.capabilities.volume_steps > 0;
	let kind = match info.device_type.enum_value_or_default() {
		DeviceType::COMPUTER | DeviceType::CHROMEBOOK => "Computer",
		DeviceType::SMARTPHONE => "Smartphone",
		DeviceType::TABLET => "Tablet",
		DeviceType::TV | DeviceType::CAST_VIDEO => "TV",
		DeviceType::AUTOMOBILE | DeviceType::CAR_THING => "Automobile",
		DeviceType::GAME_CONSOLE => "GameConsole",
		DeviceType::SMARTWATCH => "Smartwatch",
		_ => "Speaker",
	};
	Device {
		id:              id.to_owned(),
		name:            info.name.clone(),
		is_active:       id == active_id,
		volume_percent:  volume_capable.then(|| super::to_percent(volume)),
		supports_volume: Some(volume_capable),
		kind:            kind.to_owned(),
	}
}

#[cfg(test)]
mod tests {
	use librespot_protocol::connect::Capabilities;
	use protobuf::MessageField;

	use super::*;

	#[test]
	fn native_devices_preserve_active_name_and_volume_capability() {
		let mut cluster = Cluster { active_device_id: "phone".into(), ..Cluster::default() };
		cluster.device.insert("phone".into(), DeviceInfo {
			name: "Phone".into(),
			can_play: true,
			volume: 32_768,
			device_type: DeviceType::SMARTPHONE.into(),
			capabilities: MessageField::some(Capabilities { volume_steps: 64, ..Capabilities::default() }),
			..DeviceInfo::default()
		});
		cluster.device.insert("hidden".into(), DeviceInfo {
			can_play: true,
			capabilities: MessageField::some(Capabilities { hidden: true, ..Capabilities::default() }),
			..DeviceInfo::default()
		});
		let devices = devices_of(&cluster);
		assert_eq!(devices.len(), 1);
		let device = devices.first().unwrap();
		assert_eq!(device.name, "Phone");
		assert_eq!(device.kind, "Smartphone");
		assert!(device.is_active);
		assert_eq!(device.volume_percent, Some(50));
		assert_eq!(device.supports_volume, Some(true));
	}
}
