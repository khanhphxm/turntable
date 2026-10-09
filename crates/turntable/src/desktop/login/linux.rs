use std::{
	env, fs, io,
	path::{Path, PathBuf},
};

fn path() -> io::Result<PathBuf> {
	let config = env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()).map(PathBuf::from);
	let config = match config {
		Some(config) if config.is_absolute() => config,
		_ => PathBuf::from(env::var_os("HOME").ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?).join(".config"),
	};
	Ok(config.join("autostart/turntable.desktop"))
}

pub(super) fn is_enabled() -> io::Result<bool> {
	read(&path()?, &env::current_exe()?)
}

pub(super) fn set_enabled(enabled: bool) -> io::Result<()> {
	write(&path()?, &env::current_exe()?, enabled)
}

fn read(path: &Path, executable: &Path) -> io::Result<bool> {
	let text = match fs::read_to_string(path) {
		Ok(text) => text,
		Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
		Err(error) => return Err(error),
	};
	let expected = exec(executable)?;
	let mut entry = false;
	let (mut application, mut command, mut hidden, mut disabled) = (false, false, false, false);
	for line in text.lines().map(str::trim) {
		if line.starts_with('[') {
			entry = line == "[Desktop Entry]";
		} else if entry && let Some((key, value)) = line.split_once('=') {
			match key.trim() {
				"Type" => application = value.trim() == "Application",
				"Exec" => command = value.trim() == expected,
				"Hidden" => hidden = value.trim() == "true",
				"X-GNOME-Autostart-enabled" => disabled = value.trim() == "false",
				_ => {},
			}
		}
	}
	Ok(application && command && !hidden && !disabled)
}

fn write(path: &Path, executable: &Path, enabled: bool) -> io::Result<()> {
	if !enabled {
		return super::remove(path);
	}
	let command = exec(executable)?;
	if let Some(parent) = path.parent() {
		fs::create_dir_all(parent)?;
	}
	turntable_core::app::config::write_private(path, format!(
		"[Desktop Entry]\nType=Application\nName=Turntable\nComment=A lightweight Spotify player\nExec={command}\nIcon=turntable\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
	).as_bytes())
}

/// Exec has two escape layers: desktop strings, then quoted command arguments.
/// Percent signs are field codes, not shell expansions. Never invoke a shell.
fn exec(executable: &Path) -> io::Result<String> {
	let path = executable.to_str().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "The executable path is not UTF-8"))?;
	if path.contains('=') {
		return Err(io::Error::new(io::ErrorKind::InvalidInput, "Desktop autostart cannot represent an executable path containing '='"));
	}
	let mut command = String::with_capacity(path.len() + 2);
	command.push('"');
	for character in path.chars() {
		match character {
			'\\' => command.push_str("\\\\\\\\"),
			'"' => command.push_str("\\\\\""),
			'`' => command.push_str("\\\\`"),
			'$' => command.push_str("\\\\$"),
			'%' => command.push_str("%%"),
			'\n' => command.push_str("\\n"),
			'\r' => command.push_str("\\r"),
			'\t' => command.push_str("\\t"),
			character => command.push(character),
		}
	}
	command.push('"');
	Ok(command)
}

#[cfg(test)]
mod tests {
	use std::{fs, path::Path};

	use super::{exec, read, write};

	#[test]
	fn registration_is_read_from_disk_not_a_cached_preference() {
		let directory = tempfile::tempdir().unwrap();
		let path = directory.path().join("autostart/turntable.desktop");
		let executable = Path::new("/opt/Turntable player/turntable");
		assert!(!read(&path, executable).unwrap());
		write(&path, executable, true).unwrap();
		assert!(read(&path, executable).unwrap());
		let registered = fs::read_to_string(&path).unwrap();
		fs::write(&path, format!("{registered}Hidden=true\n")).unwrap();
		assert!(!read(&path, executable).unwrap());
		fs::write(&path, registered.replace("X-GNOME-Autostart-enabled=true", "X-GNOME-Autostart-enabled=false")).unwrap();
		assert!(!read(&path, executable).unwrap());
		write(&path, executable, true).unwrap();
		assert!(!read(&path, Path::new("/other/turntable")).unwrap());
		write(&path, executable, false).unwrap();
		write(&path, executable, false).unwrap();
		assert!(!read(&path, executable).unwrap());
	}

	#[test]
	fn desktop_exec_escapes_reserved_characters_without_a_shell() {
		assert_eq!(exec(Path::new("/opt/a b/turntable")).unwrap(), "\"/opt/a b/turntable\"");
		assert_eq!(exec(Path::new("/opt/a\\b\"$`%/turntable")).unwrap(), "\"/opt/a\\\\\\\\b\\\\\"\\\\$\\\\`%%/turntable\"");
		assert!(exec(Path::new("/opt/a=b/turntable")).is_err());
	}
}
