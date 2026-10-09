//! Desktop integration lives on GPUI's main thread. Login state is read from the OS,
//! never serialized in player preferences; changing it writes the native registration
//! and reads it back so Settings also reflects external changes and failed writes.

mod login;

use gpui_kit::{App, Global};

pub struct Desktop {
	pub login_enabled: bool,
	pub error:         Option<String>,
}

impl Global for Desktop {}

pub fn install(cx: &mut App) {
	let (login_enabled, error) = match login::is_enabled() {
		Ok(enabled) => (enabled, None),
		Err(error) => (false, Some(error)),
	};
	cx.set_global(Desktop { login_enabled, error });
}

pub fn set_login(enabled: bool, cx: &mut App) {
	let changed = login::set_enabled(enabled);
	let actual = login::is_enabled();
	let desktop = cx.global_mut::<Desktop>();
	desktop.login_enabled = actual.as_ref().copied().unwrap_or(false);
	desktop.error = changed.err().or_else(|| actual.err());
}
