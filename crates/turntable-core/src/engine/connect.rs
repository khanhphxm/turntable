//! Account-wide Connect state is already returned at registration and pushed by Dealer.
//! The device menu, remote bar and paused startup restore need no Web API reads.

use librespot_protocol::{
	connect::{Cluster, DeviceInfo},
	devices::DeviceType,
};

use super::{ClusterPlayback, ConnectState, Forward, Incoming, LastPlayback, Options, Position, Repeat, SessionReader, Status};
use crate::{
	net::trace,
	spotify::{Device, PlayerCommand, RepeatMode},
};

impl Forward {
	pub(super) fn cluster(&mut self, cluster: &Cluster, initial: bool) {
		let devices = devices_of(cluster);
		let local_active = cluster.active_device_id == self.session.device_id() && self.state.active;
		let playback = if !local_active {
			if let Some(player) = cluster.player_state.as_ref()
				&& let Some(track) = player.track.as_ref().filter(|track| !track.uri.is_empty())
			{
				let mut item = super::named(track.uri.clone(), String::new());
				item.decorations.playback.duration_ms = u64::try_from(player.duration).unwrap_or_default();
				// Explicit track lists use "-" rather than a playable context URI.
				let uri = if player.context_uri == "-" { "" } else { &player.context_uri };
				let context = super::named(uri.to_owned(), String::new());
				if self.state.track != track.uri {
					self.state.track.clone_from(&track.uri);
					let _ = self.metadata.send(super::metadata::Request::Track(track.uri.clone()));
				}
				if self.state.context != uri {
					uri.clone_into(&mut self.state.context);
					let _ = self.metadata.send(super::metadata::Request::Context(uri.to_owned()));
				}
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
		if let Some(player) = &playback {
			trace::playback("connect-event", player.status.name(), Some(player.position.position_ms));
		} else {
			trace::playback("connect-event", "inactive", None);
		}
		(self.deliver)(Incoming::ConnectState(ConnectState { account: self.session.username(), devices, playback, local_active, initial }));
	}

	pub(super) fn transfer_device(&self, device: String, play: bool, id: u64) {
		let session = self.session.clone();
		let requests = self.requests.clone();
		tokio::spawn(async move {
			if let Err(error) = super::network_timeout(super::NETWORK_TIMEOUT, transfer(&session, &device, play)).await {
				let _ = requests.send(super::Request::Failed { id, message: format!("Couldn't transfer playback: {error}") });
			}
		});
	}
}

impl SessionReader {
	pub(crate) fn devices(&self) -> Result<Vec<Device>, librespot_core::Error> {
		self.cluster()
			.map(|cluster| devices_of(&cluster))
			.ok_or_else(|| librespot_core::Error::unavailable("Spotify Connect state has not arrived yet"))
	}

	pub(crate) fn transfer(&self, device: &str, play: bool) -> Result<(), librespot_core::Error> {
		self.block_on(transfer(self.session(), device, play))
	}

	pub(crate) fn control(&self, device: &str, command: PlayerCommand) -> Result<(), librespot_core::Error> {
		let (method, endpoint, body) = command_request(self.session().device_id(), device, command);
		let body = serde_json::to_vec(&body)?;
		self.block_on(async {
			let mut headers = http::HeaderMap::new();
			headers.insert(http::header::CONTENT_TYPE, http::HeaderValue::from_static("application/json"));
			let span = trace::start();
			let bytes = self.session().spclient().request(&method, &endpoint, Some(headers), Some(&body)).await?;
			if trace::enabled() {
				span.done("session", method.as_str(), &format!("https://spclient.spotify.com{endpoint}"), trace::Reply {
					status: 200,
					bytes: bytes.len(),
					..trace::Reply::default()
				});
			}
			Ok(())
		})
	}
}

async fn transfer(session: &librespot_core::Session, device: &str, play: bool) -> Result<(), librespot_core::Error> {
	let device = if device.is_empty() { session.device_id() } else { device };
	let request = librespot_core::spclient::TransferRequest {
		transfer_options: librespot_core::dealer::protocol::TransferOptions {
			restore_paused: play.then(|| "resume".to_owned()),
			..Default::default()
		},
	};
	session.spclient().transfer(device, device, Some(&request)).await?;
	Ok(())
}

/// Connect-state command bodies are delivered to the target device over Dealer.
/// Volume uses its separate PUT endpoint and the Connect 0..65535 scale.
fn command_request(from: &str, to: &str, command: PlayerCommand) -> (http::Method, String, serde_json::Value) {
	let mut command_body = serde_json::Map::new();
	let endpoint = match command {
		PlayerCommand::Volume(percent) => {
			return (
				http::Method::PUT,
				format!("/connect-state/v1/connect/volume/from/{from}/to/{to}"),
				serde_json::json!({"volume": super::to_u16(percent)}),
			);
		},
		PlayerCommand::Resume => "resume",
		PlayerCommand::Pause => "pause",
		PlayerCommand::Next => "skip_next",
		PlayerCommand::Previous => "skip_prev",
		PlayerCommand::Seek(position) => {
			let position = u32::try_from(position).unwrap_or(u32::MAX);
			command_body.insert("value".into(), position.into());
			command_body.insert("position".into(), position.into());
			"seek_to"
		},
		PlayerCommand::Shuffle(enabled) => {
			command_body.insert("value".into(), enabled.into());
			"set_shuffling_context"
		},
		PlayerCommand::Repeat(mode) => {
			command_body.insert("repeating_context".into(), (mode == RepeatMode::Context).into());
			command_body.insert("repeating_track".into(), (mode == RepeatMode::Track).into());
			"set_options"
		},
	};
	command_body.insert("endpoint".into(), endpoint.into());
	command_body.insert("logging_params".into(), serde_json::json!({"command_id": format!("{:032x}", rand::random::<u128>())}));
	(http::Method::POST, format!("/connect-state/v1/player/command/from/{from}/to/{to}"), serde_json::json!({"command": command_body}))
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

pub(crate) fn device_of(id: &str, info: &DeviceInfo, active_id: &str) -> Device {
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

	#[test]
	fn every_remote_control_uses_connect_state_and_matches_dealer_wire_commands() -> Result<(), serde_json::Error> {
		use librespot_core::dealer::protocol::Request;
		for command in [
			PlayerCommand::Resume,
			PlayerCommand::Pause,
			PlayerCommand::Next,
			PlayerCommand::Previous,
			PlayerCommand::Seek(u64::MAX),
			PlayerCommand::Shuffle(true),
			PlayerCommand::Repeat(RepeatMode::Off),
			PlayerCommand::Repeat(RepeatMode::Context),
			PlayerCommand::Repeat(RepeatMode::Track),
		] {
			let (method, endpoint, body) = command_request("local", "phone", command);
			assert_eq!(method, http::Method::POST);
			assert_eq!(endpoint, "/connect-state/v1/player/command/from/local/to/phone");
			let mut received = body.clone();
			received["message_id"] = 1.into();
			received["sent_by_device_id"] = "local".into();
			let _: Request = serde_json::from_value(received)?;
			if let PlayerCommand::Repeat(mode) = command {
				assert_eq!(body["command"]["repeating_context"], mode == RepeatMode::Context);
				assert_eq!(body["command"]["repeating_track"], mode == RepeatMode::Track);
			}
			if matches!(command, PlayerCommand::Seek(_)) {
				assert_eq!(body["command"]["value"], u32::MAX);
			}
		}
		let (method, endpoint, body) = command_request("local", "phone", PlayerCommand::Volume(255));
		assert_eq!(method, http::Method::PUT);
		assert_eq!(endpoint, "/connect-state/v1/connect/volume/from/local/to/phone");
		assert_eq!(body["volume"], 65_535);
		Ok(())
	}
}
