//! Device-picker defaults for backends that only follow the system output.

use super::super::OutputDevice;

pub(crate) const CAN_PICK_OUTPUT: bool = false;

#[derive(Default)]
pub(crate) struct State {}

pub struct Watch;

pub(crate) fn devices() -> Vec<OutputDevice> {
	Vec::new()
}

#[expect(clippy::unnecessary_wraps, reason = "the portable picker API reports no known default-device name on these backends")]
pub(crate) fn default_device_name() -> Option<String> {
	None
}

#[expect(clippy::unnecessary_wraps, reason = "the portable picker API reports that no device watcher is supported")]
pub(crate) fn watch(_notify: impl Fn() + Send + Sync + 'static) -> Option<Watch> {
	None
}
