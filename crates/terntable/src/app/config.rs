//! Application filesystem locations and user-provided secrets.
//!
//! - config/state: Windows `%APPDATA%\terntable`; elsewhere `$XDG_CONFIG_HOME/terntable`
//!   or `~/.config/terntable` (saved player sign-in, Connect device id, `client_id`,
//!   `settings.json`, and the Web API's `token.json` where no OS credential store is
//!   used; see `crate::spotify::token_store`).
//! - cache: covers and the player's audio files under `player/files`.
//!   Windows `%LOCALAPPDATA%\terntable`, macOS `~/Library/Caches/terntable`,
//!   elsewhere `$XDG_CACHE_HOME/terntable` or `~/.cache/terntable`.
//!
//! `TERNTABLE_HOME=<dir>` overrides both, on every platform: config is
//! `<dir>/config`, cache `<dir>/cache`. Isolated test runs point it at a scratch
//! directory so they never read or write the user's real settings or tokens.
//! The Spotify Web API token then stays in a file under `<dir>/config`, never
//! the OS credential store.

#[cfg_attr(unix, path = "config/unix.rs")]
#[cfg_attr(windows, path = "config/windows.rs")]
mod platform;

use std::{
	env, fs, io,
	ffi::OsString,
	path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct Paths {
	pub config: PathBuf,
	pub cache:  PathBuf,
}

/// The `TERNTABLE_HOME` scratch root, when set (and non-empty).
pub fn home_override() -> Option<PathBuf> {
	env::var_os("TERNTABLE_HOME").filter(|v| !v.is_empty()).map(PathBuf::from)
}

impl Paths {
	pub fn resolve() -> io::Result<Self> {
		let paths = Self::from_env(env::consts::OS, |name| env::var_os(name))?;
		for dir in [&paths.config, &paths.cache] {
			fs::create_dir_all(dir)?;
		}
		Ok(paths)
	}

	/// Resolves locations without touching process state or creating directories.
	fn from_env(target_os: &str, lookup: impl Fn(&str) -> Option<OsString>) -> io::Result<Self> {
		if let Some(root) = lookup("TERNTABLE_HOME").filter(|value| !value.is_empty()).map(PathBuf::from) {
			return Ok(Self { config: root.join("config"), cache: root.join("cache") });
		}
		if target_os == "windows" {
			let dir = |name: &str| {
				lookup(name)
					.filter(|value| !value.is_empty())
					.map(|value| PathBuf::from(value).join("terntable"))
					.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{name} is not set")))
			};
			return Ok(Self { config: dir("APPDATA")?, cache: dir("LOCALAPPDATA")? });
		}
		let home = lookup("HOME").map(PathBuf::from).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
		let xdg = |name: &str, fallback: &str| {
			lookup(name).filter(|value| !value.is_empty()).map_or_else(|| home.join(fallback), PathBuf::from).join("terntable")
		};
		let config = xdg("XDG_CONFIG_HOME", ".config");
		let cache = if target_os == "macos" { home.join("Library/Caches/terntable") } else { xdg("XDG_CACHE_HOME", ".cache") };
		Ok(Self { config, cache })
	}
}

/// Reads a one-line secret file, trimming surrounding whitespace.
pub fn read_secret(path: &Path) -> io::Result<String> {
	let text = fs::read_to_string(path)?;
	let value = text.trim();
	if value.is_empty() {
		return Err(io::Error::new(io::ErrorKind::InvalidData, format!("{} is empty", path.display())));
	}
	Ok(value.to_owned())
}

/// Writes a file readable only by the current user.
pub fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
	use std::io::Write;
	let tmp = path.with_extension("tmp");
	let mut opts = fs::OpenOptions::new();
	opts.write(true).create(true).truncate(true);
	platform::private_permissions(&mut opts);
	let mut file = opts.open(&tmp)?;
	file.write_all(bytes)?;
	file.sync_all()?;
	fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn resolve(target_os: &str, values: &[(&str, &str)]) -> Paths {
		Paths::from_env(target_os, |name| values.iter().find(|(key, _)| *key == name).map(|(_, value)| OsString::from(value))).unwrap()
	}

	#[test]
	fn macos_keeps_xdg_config_and_native_cache() {
		let home = Path::new("/user");
		let paths = resolve("macos", &[("HOME", "/user")]);
		assert_eq!(paths.config, home.join(".config/terntable"));
		assert_eq!(paths.cache, home.join("Library/Caches/terntable"));
		let paths = resolve("macos", &[("HOME", "/user"), ("XDG_CONFIG_HOME", "/config"), ("XDG_CACHE_HOME", "/ignored")]);
		assert_eq!(paths.config, Path::new("/config").join("terntable"));
		assert_eq!(paths.cache, home.join("Library/Caches/terntable"));
	}

	#[test]
	fn linux_uses_xdg_roots_or_home_fallbacks() {
		let paths = resolve("linux", &[("HOME", "/user"), ("XDG_CONFIG_HOME", "/config"), ("XDG_CACHE_HOME", "/cache")]);
		assert_eq!(paths.config, Path::new("/config").join("terntable"));
		assert_eq!(paths.cache, Path::new("/cache").join("terntable"));
		for values in [vec![("HOME", "/user")], vec![("HOME", "/user"), ("XDG_CONFIG_HOME", ""), ("XDG_CACHE_HOME", "")]] {
			let paths = resolve("linux", &values);
			assert_eq!(paths.config, Path::new("/user").join(".config/terntable"));
			assert_eq!(paths.cache, Path::new("/user").join(".cache/terntable"));
		}
	}

	#[test]
	fn windows_uses_roaming_config_and_local_cache_without_home() {
		let paths = resolve("windows", &[("APPDATA", "/roaming"), ("LOCALAPPDATA", "/local")]);
		assert_eq!(paths.config, Path::new("/roaming").join("terntable"));
		assert_eq!(paths.cache, Path::new("/local").join("terntable"));
	}

	#[test]
	fn home_override_wins_on_every_platform_and_empty_values_are_ignored() {
		for target_os in ["macos", "linux", "windows"] {
			let paths = resolve(target_os, &[("TERNTABLE_HOME", "/isolated")]);
			assert_eq!(paths.config, Path::new("/isolated").join("config"));
			assert_eq!(paths.cache, Path::new("/isolated").join("cache"));
		}
		let paths = resolve("linux", &[("TERNTABLE_HOME", ""), ("HOME", "/user")]);
		assert_eq!(paths.config, Path::new("/user").join(".config/terntable"));
	}

	#[test]
	fn missing_required_roots_report_the_variable() {
		for target_os in ["macos", "linux", "windows"] {
			let error = Paths::from_env(target_os, |_| None).unwrap_err();
			assert_eq!(error.kind(), io::ErrorKind::NotFound);
			assert_eq!(error.to_string(), if target_os == "windows" { "APPDATA is not set" } else { "HOME is not set" });
		}
		for local in [None, Some(OsString::new())] {
			let error = Paths::from_env("windows", |name| match name {
				"APPDATA" => Some(OsString::from("/roaming")),
				"LOCALAPPDATA" => local.clone(),
				_ => None,
			})
			.unwrap_err();
			assert_eq!(error.to_string(), "LOCALAPPDATA is not set");
		}
	}
}
