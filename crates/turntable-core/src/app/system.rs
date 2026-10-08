//! Machine facts used by the host's audio-output selector.

use crate::audio::OutputDevice;

#[derive(Debug, Default)]
pub struct System {
	pub outputs:        Vec<OutputDevice>,
	pub default_output: Option<String>,
	/// An embedding host may override the persisted Spotify Connect name for this launch.
	pub name_override:  Option<String>,
}

impl System {
	pub fn device_name<'a>(&'a self, setting: &'a str) -> &'a str {
		self.name_override.as_deref().unwrap_or(setting)
	}
}
