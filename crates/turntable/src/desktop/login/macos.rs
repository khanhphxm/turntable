use std::{env, fs, io, path::PathBuf};

use objc2::{available, rc::Retained};
use objc2_foundation::NSBundle;
use objc2_service_management::{SMAppService, SMAppServiceStatus};
use plist::{Dictionary, Value};

const LABEL: &str = "wtf.penguins.turntable";

/// SMAppService needs a signed .app bundle. Older macOS and standalone development
/// binaries use a per-user LaunchAgent instead; a denied modern login item is never
/// bypassed by installing a legacy agent behind the user's back.
fn service() -> Option<Retained<SMAppService>> {
	if available!(macos = 13.0) && NSBundle::mainBundle().bundlePath().to_string().ends_with(".app") {
		// SAFETY: availability was checked and this is the documented main-app service.
		Some(unsafe { SMAppService::mainAppService() })
	} else {
		None
	}
}

fn path() -> io::Result<PathBuf> {
	let home = env::var_os("HOME").ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
	Ok(PathBuf::from(home).join("Library/LaunchAgents").join(format!("{LABEL}.plist")))
}

pub(super) fn is_enabled() -> io::Result<bool> {
	if let Some(service) = service() {
		// SAFETY: service() only creates services on macOS 13+.
		let status = unsafe { service.status() };
		if status == SMAppServiceStatus::Enabled {
			return Ok(true);
		}
		if status == SMAppServiceStatus::RequiresApproval {
			return Err(io::Error::other("Approve Turntable in System Settings → General → Login Items"));
		}
	}
	let value = match Value::from_file(path()?) {
		Ok(value) => value,
		Err(error) if error.as_io().is_some_and(|error| error.kind() == io::ErrorKind::NotFound) => return Ok(false),
		Err(error) => return Err(io::Error::other(error)),
	};
	let Some(agent) = value.as_dictionary() else { return Ok(false) };
	let runs = agent.get("RunAtLoad").and_then(Value::as_boolean).unwrap_or(false);
	let disabled = agent.get("Disabled").and_then(Value::as_boolean).unwrap_or(false);
	let executable = env::current_exe()?;
	let command =
		agent.get("ProgramArguments").and_then(Value::as_array).and_then(|arguments| arguments.first()).and_then(Value::as_string);
	Ok(runs && !disabled && command == executable.to_str())
}

pub(super) fn set_enabled(enabled: bool) -> io::Result<()> {
	if let Some(service) = service() {
		// SAFETY: service() checked availability; registration never starts playback
		// or launches another instance of the main application immediately.
		unsafe {
			let status = service.status();
			if enabled && status != SMAppServiceStatus::Enabled {
				service.registerAndReturnError().map_err(|error| io::Error::other(error.to_string()))?;
			} else if !enabled && (status == SMAppServiceStatus::Enabled || status == SMAppServiceStatus::RequiresApproval) {
				service.unregisterAndReturnError().map_err(|error| io::Error::other(error.to_string()))?;
			}
		}
		// Clean cutover: the modern registration and legacy agent must not both launch us.
		return super::remove(&path()?);
	}
	let path = path()?;
	if !enabled {
		return super::remove(&path);
	}
	if let Some(parent) = path.parent() {
		fs::create_dir_all(parent)?;
	}
	let executable = env::current_exe()?;
	let executable = executable.to_str().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "The executable path is not UTF-8"))?;
	let mut agent = Dictionary::new();
	agent.insert("Label".into(), Value::String(LABEL.into()));
	agent.insert("ProgramArguments".into(), Value::Array(vec![Value::String(executable.into())]));
	agent.insert("RunAtLoad".into(), Value::Boolean(true));
	agent.insert("ProcessType".into(), Value::String("Interactive".into()));
	let mut bytes = Vec::new();
	Value::Dictionary(agent).to_writer_xml(&mut bytes).map_err(io::Error::other)?;
	turntable_core::app::config::write_private(&path, &bytes)
}
