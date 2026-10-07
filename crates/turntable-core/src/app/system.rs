//! What the settings page reports about the machine and the running player: output devices,
//! the playback cache and app memory. The player loop gathers it (the slow parts on a thread
//! of their own); the views only read it.

use std::{
	fs, io,
	path::{Path, PathBuf},
};

use crate::audio::OutputDevice;

/// A dropdown of the settings page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Menu {
	/// The sleep timer's choices.
	Sleep,
	/// The output devices.
	Output,
}

/// A folder (or file) the diagnostics links open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Folder {
	/// Settings and keys.
	Config,
	/// The playback cache.
	Cache,
}

/// Where those are.
#[derive(Debug, Clone, Default)]
pub struct Places {
	pub config: PathBuf,
	pub cache:  PathBuf,
}

impl Places {
	pub fn get(&self, folder: Folder) -> &Path {
		match folder {
			Folder::Config => &self.config,
			Folder::Cache => &self.cache,
		}
	}
}

/// What the running player was started with: changing these settings needs a restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
	pub name:     String,
	pub cache_mb: u32,
}

/// Facts the settings page shows. Everything is `None` or empty until it is known.
#[derive(Debug, Default)]
pub struct System {
	/// The output devices that can be picked (macOS), and the name of the system's default.
	pub outputs:        Vec<OutputDevice>,
	pub default_output: Option<String>,
	/// Bytes in the player's cache, and whether it is being emptied.
	pub cache_bytes:    Option<u64>,
	pub clearing_cache: bool,
	/// Memory (kB) held by turntable, including the in-process player.
	pub app_kb:         Option<u64>,
	/// What the running player was started with (`None`: not running).
	pub running:        Option<Running>,
	/// The name given on the command line (`--device-name`), which wins over the setting.
	pub name_override:  Option<String>,
	/// What the player being started now is started with (becomes `running` once it is up).
	pub launching:      Option<Running>,
	/// Which dropdown is open.
	pub menu:           Option<Menu>,
	pub places:         Places,
}

impl System {
	/// The name the player is (or will be) started with.
	pub fn device_name<'a>(&'a self, setting: &'a str) -> &'a str {
		self.name_override.as_deref().unwrap_or(setting)
	}

	/// Whether the running player differs from what is set, so a restart would change something.
	pub fn restart_needed(&self, setting_name: &str, setting_cache_mb: u32) -> bool {
		self.running.as_ref().is_some_and(|r| r.name != self.device_name(setting_name) || r.cache_mb != setting_cache_mb)
	}
}

// ------------------------------------------------------------------------- memory

/// The memory a process holds, in kB: on macOS what Activity Monitor calls Memory (the physical
/// footprint, which counts what the process compressed or shares as its own), elsewhere its
/// resident set.
pub fn memory_kb(pid: u32) -> Option<u64> {
	imp::memory_kb(pid)
}

#[cfg(target_os = "macos")]
mod imp {
	pub fn memory_kb(pid: u32) -> Option<u64> {
		let pid = libc::c_int::try_from(pid).ok()?;
		// SAFETY: zeroed plain data that `proc_pid_rusage` fills.
		let mut info: libc::rusage_info_v0 = unsafe { std::mem::zeroed() };
		let buffer = (&raw mut info).cast::<libc::rusage_info_t>();
		// SAFETY: the buffer is a `rusage_info_v0`, which is what flavor 0 writes.
		let status = unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V0, buffer) };
		(status == 0).then_some(info.ri_phys_footprint / 1024)
	}
}

#[cfg(not(target_os = "macos"))]
mod imp {
	pub fn memory_kb(pid: u32) -> Option<u64> {
		let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
		let line = status.lines().find_map(|l| l.strip_prefix("VmRSS:"))?;
		line.split_whitespace().next()?.parse().ok()
	}
}

/// "62 MB", "1.4 GB" (decimal units, as Finder shows sizes).
pub fn size_label(bytes: u64) -> String {
	const MB: u64 = 1_000_000;
	const GB: u64 = 1_000 * MB;
	match bytes {
		b if b >= GB => format!("{:.1} GB", b as f64 / GB as f64),
		b if b >= MB => format!("{} MB", b / MB),
		b if b >= 1_000 => format!("{} KB", b / 1_000),
		b => format!("{b} B"),
	}
}

/// "62 MB" for memory counted in kB.
pub fn memory_label(kb: u64) -> String {
	size_label(kb.saturating_mul(1_024))
}

// -------------------------------------------------------------------------- cache

/// Bytes in the files under `dir` (links are not followed).
pub fn dir_bytes(dir: &Path) -> u64 {
	let Ok(entries) = fs::read_dir(dir) else { return 0 };
	entries
		.flatten()
		.map(|e| match e.file_type() {
			Ok(t) if t.is_dir() => dir_bytes(&e.path()),
			Ok(t) if t.is_file() => e.metadata().map_or(0, |m| m.len()),
			_ => 0,
		})
		.sum()
}

/// Removes everything inside `dir`, and keeps `dir`. Links are removed, never followed.
pub fn clear_dir(dir: &Path) -> io::Result<()> {
	// A path this short is a mistake, not a cache.
	if dir.components().count() < 3 {
		return Err(io::Error::new(io::ErrorKind::InvalidInput, "refusing to empty a top-level folder"));
	}
	let entries = match fs::read_dir(dir) {
		Ok(entries) => entries,
		Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
		Err(e) => return Err(e),
	};
	let mut first_error = None;
	for entry in entries.flatten() {
		let path = entry.path();
		let result = match entry.file_type() {
			Ok(t) if t.is_dir() => fs::remove_dir_all(&path),
			_ => fs::remove_file(&path),
		};
		if let Err(e) = result {
			first_error.get_or_insert(e);
		}
	}
	first_error.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn scratch(name: &str) -> PathBuf {
		let dir = std::env::temp_dir().join(format!("tt-system-{name}-{}", std::process::id()));
		let _ = fs::remove_dir_all(&dir);
		fs::create_dir_all(&dir).unwrap();
		dir
	}

	#[test]
	fn a_cache_is_measured_and_emptied_but_kept() {
		let dir = scratch("cache").join("player").join("files");
		fs::create_dir_all(dir.join("a/b")).unwrap();
		fs::write(dir.join("one"), vec![0u8; 1000]).unwrap();
		fs::write(dir.join("a/two"), vec![0u8; 2500]).unwrap();
		fs::write(dir.join("a/b/three"), vec![0u8; 500]).unwrap();
		assert_eq!(dir_bytes(&dir), 4000);
		clear_dir(&dir).unwrap();
		assert!(dir.is_dir(), "the folder itself stays: the player is going to use it");
		assert_eq!(dir_bytes(&dir), 0);
		assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
		// Emptying what isn't there is fine.
		clear_dir(&dir.join("missing")).unwrap();
		fs::remove_dir_all(dir.parent().unwrap().parent().unwrap()).unwrap();
	}

	// Unix symlinks need no privilege; Windows symlink creation requires Developer Mode/admin.
	#[cfg(unix)]
	#[test]
	fn emptying_never_follows_a_link_out_of_the_cache() {
		let root = scratch("links");
		let outside = root.join("precious");
		fs::create_dir_all(&outside).unwrap();
		fs::write(outside.join("file"), b"keep me").unwrap();
		let cache = root.join("cache").join("player").join("files");
		fs::create_dir_all(&cache).unwrap();
		std::os::unix::fs::symlink(&outside, cache.join("link-to-dir")).unwrap();
		std::os::unix::fs::symlink(outside.join("file"), cache.join("link-to-file")).unwrap();
		assert_eq!(dir_bytes(&cache), 0, "links are not counted");
		clear_dir(&cache).unwrap();
		assert_eq!(fs::read(outside.join("file")).unwrap(), b"keep me");
		assert_eq!(fs::read_dir(&cache).unwrap().count(), 0);
		fs::remove_dir_all(root).unwrap();
	}

	#[test]
	fn a_top_level_folder_is_never_emptied() {
		assert!(clear_dir(Path::new("/")).is_err());
		assert!(clear_dir(Path::new("/tmp")).is_err());
	}

	#[test]
	fn a_restart_is_needed_only_when_the_running_player_differs() {
		let mut sys = System::default();
		assert!(!sys.restart_needed("turntable", 100), "no player is running");
		sys.running = Some(Running { name: "turntable".into(), cache_mb: 100 });
		assert!(!sys.restart_needed("turntable", 100));
		assert!(sys.restart_needed("Kitchen", 100));
		assert!(sys.restart_needed("turntable", 500));
		// A name given on the command line wins over the setting.
		sys.name_override = Some("turntable".into());
		assert!(!sys.restart_needed("Kitchen", 100));
		assert_eq!(sys.device_name("Kitchen"), "turntable");
	}
}
