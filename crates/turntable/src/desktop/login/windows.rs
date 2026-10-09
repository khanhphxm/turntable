use std::{env, io, path::Path};

use winreg::{
	RegKey,
	enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE},
};

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const NAME: &str = "Turntable";

pub(super) fn is_enabled() -> io::Result<bool> {
	read(&RegKey::predef(HKEY_CURRENT_USER), RUN, &env::current_exe()?)
}

pub(super) fn set_enabled(enabled: bool) -> io::Result<()> {
	write(&RegKey::predef(HKEY_CURRENT_USER), RUN, &env::current_exe()?, enabled)
}

fn command(executable: &Path) -> io::Result<String> {
	let executable =
		executable.to_str().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "The executable path is not Unicode"))?;
	Ok(format!("\"{executable}\""))
}

fn read(root: &RegKey, path: &str, executable: &Path) -> io::Result<bool> {
	let key = match root.open_subkey_with_flags(path, KEY_READ) {
		Ok(key) => key,
		Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
		Err(error) => return Err(error),
	};
	match key.get_value::<String, _>(NAME) {
		Ok(value) => Ok(value == command(executable)?),
		Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
		Err(error) => Err(error),
	}
}

fn write(root: &RegKey, path: &str, executable: &Path, enabled: bool) -> io::Result<()> {
	if enabled {
		let (key, _) = root.create_subkey(path)?;
		key.set_value(NAME, &command(executable)?)
	} else {
		let key = match root.open_subkey_with_flags(path, KEY_SET_VALUE) {
			Ok(key) => key,
			Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
			Err(error) => return Err(error),
		};
		match key.delete_value(NAME) {
			Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
			result => result,
		}
	}
}

#[cfg(test)]
mod tests {
	use std::{
		path::Path,
		time::{SystemTime, UNIX_EPOCH},
	};

	use winreg::{RegKey, enums::HKEY_CURRENT_USER};

	use super::{NAME, read, write};

	#[test]
	fn registry_registration_round_trips_and_observes_external_changes() {
		// An isolated key: never alter the test machine's actual Run entries.
		let root = RegKey::predef(HKEY_CURRENT_USER);
		let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
		let path = format!(r"Software\TurntableTests\Login-{}-{nonce}", std::process::id());
		let executable = Path::new(r"C:\Program Files\Turntable\turntable.exe");
		assert!(!read(&root, &path, executable).unwrap());
		write(&root, &path, executable, true).unwrap();
		assert!(read(&root, &path, executable).unwrap());
		let key = root.open_subkey(&path).unwrap();
		assert_eq!(key.get_value::<String, _>(NAME).unwrap(), r#""C:\Program Files\Turntable\turntable.exe""#);
		write(&root, &path, executable, false).unwrap();
		write(&root, &path, executable, false).unwrap();
		assert!(!read(&root, &path, executable).unwrap());
		root.delete_subkey_all(&path).unwrap();
	}
}
