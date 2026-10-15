//! Local, account-scoped pins. Spotify's private your-library pin protocol is not
//! exposed by librespot; these preferences never spend Web API quota.

use std::{
	fs, io,
	path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::app::config;

pub(super) const LIMIT: usize = 4;
pub(super) const FULL: &str = "You can only pin 4 items to Your Library";

#[derive(Serialize, Deserialize)]
struct Saved {
	account: String,
	uris:    Vec<String>,
}

fn path(config: &Path, account: &str) -> PathBuf {
	config.join("library-pins").join(format!("{}.json", crate::sha256_hex(account.as_bytes())))
}

pub(super) fn load(config: &Path, account: &str) -> io::Result<Vec<String>> {
	let bytes = match fs::read(path(config, account)) {
		Ok(bytes) => bytes,
		Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
		Err(error) => return Err(error),
	};
	let saved: Saved = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
	if saved.account != account || saved.uris.len() > LIMIT {
		return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid library pins for this account"));
	}
	Ok(saved.uris)
}

pub(super) fn change(uris: &mut Vec<String>, uri: &str, pinned: bool) -> Result<bool, &'static str> {
	if let Some(index) = uris.iter().position(|known| known == uri) {
		if pinned {
			return Ok(false);
		}
		uris.remove(index);
		return Ok(true);
	}
	if !pinned {
		return Ok(false);
	}
	if uris.len() == LIMIT {
		return Err(FULL);
	}
	uris.push(uri.to_owned());
	Ok(true)
}

pub(super) fn set(config: &Path, account: &str, uri: &str, pinned: bool) -> io::Result<Vec<String>> {
	let mut uris = load(config, account)?;
	if change(&mut uris, uri, pinned).map_err(io::Error::other)? {
		let destination = path(config, account);
		if let Some(parent) = destination.parent() {
			fs::create_dir_all(parent)?;
		}
		let saved = Saved { account: account.to_owned(), uris };
		let bytes = serde_json::to_vec(&saved).map_err(io::Error::other)?;
		config::write_private(&destination, &bytes)?;
		return Ok(saved.uris);
	}
	Ok(uris)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn pins_are_idempotent_bounded_and_persisted_per_account() -> io::Result<()> {
		let directory = tempfile::tempdir()?;
		assert!(load(directory.path(), "alice")?.is_empty());
		for uri in ["spotify:playlist:a", "spotify:album:b", "spotify:artist:c", "spotify:playlist:d"] {
			set(directory.path(), "alice", uri, true)?;
		}
		assert_eq!(set(directory.path(), "alice", "spotify:playlist:a", true)?.len(), LIMIT);
		assert!(set(directory.path(), "alice", "spotify:playlist:full", true).is_err());
		assert!(load(directory.path(), "bob")?.is_empty());
		set(directory.path(), "bob", "spotify:playlist:other", true)?;
		assert_eq!(set(directory.path(), "alice", "spotify:album:b", false)?.len(), 3);
		assert_eq!(load(directory.path(), "bob")?, ["spotify:playlist:other"]);
		assert_eq!(set(directory.path(), "alice", "spotify:album:b", false)?.len(), 3);
		Ok(())
	}
}
