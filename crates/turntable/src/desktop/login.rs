//! Native, per-user login registration. No cached enabled flag is kept on disk.

#[cfg_attr(target_os = "linux", path = "login/linux.rs")]
#[cfg_attr(target_os = "macos", path = "login/macos.rs")]
#[cfg_attr(target_os = "windows", path = "login/windows.rs")]
mod platform;

pub(super) fn is_enabled() -> Result<bool, String> {
	platform::is_enabled().map_err(|error| format!("Could not read login settings: {error}"))
}

pub(super) fn set_enabled(enabled: bool) -> Result<(), String> {
	platform::set_enabled(enabled).map_err(|error| format!("Could not change login settings: {error}"))
}

#[cfg(unix)]
fn remove(path: &std::path::Path) -> std::io::Result<()> {
	match std::fs::remove_file(path) {
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
		result => result,
	}
}
