//! Where the OAuth token (JSON, includes the refresh token) is persisted.
//!
//! Normally the OS credential store: macOS Keychain, Linux Secret Service or Windows
//! Credential Manager, with service `turntable` and account `spotify-token`. A `token.json`
//! (0600 on Unix) in the config dir is used instead when
//! - `TURNTABLE_HOME` is set (the test harness must never touch the real keychain),
//! - the store is unreachable (Linux without a Secret Service daemon); a line is appended to
//!   `$TMPDIR/turntable-debug.log` once per process.
//!
//! A `token.json` left by an older version is moved into the credential store on first load.

mod credential;

use std::{
	fs,
	io::{self, Write},
	path::{Path, PathBuf},
	sync::Once,
};

use crate::app::config;

#[derive(Debug)]
enum VaultError {
	/// No credential store to talk to; callers may fall back to the file.
	Unavailable(String),
	/// The store is there but refused or failed; surfaced to the caller.
	Failed(String),
}

impl VaultError {
	fn into_io(self) -> io::Error {
		match self {
			Self::Unavailable(m) | Self::Failed(m) => io::Error::other(format!("credential store: {m}")),
		}
	}
}

/// One secret slot in an OS credential store.
trait Vault {
	fn read(&self) -> Result<Option<Vec<u8>>, VaultError>;
	fn write(&self, secret: &[u8]) -> Result<(), VaultError>;
	/// Deleting an absent entry succeeds.
	fn remove(&self) -> Result<(), VaultError>;
}

pub struct TokenStore {
	vault: Option<Box<dyn Vault>>,
	file:  PathBuf,
}

impl TokenStore {
	/// The store for `config_dir`: the OS credential store unless `TURNTABLE_HOME` is set.
	pub fn open(config_dir: &Path) -> Self {
		Self { vault: os_vault(), file: config_dir.join("token.json") }
	}

	/// The stored token bytes, migrating a legacy `token.json` into the credential store.
	pub fn load(&self) -> io::Result<Option<Vec<u8>>> {
		let Some(vault) = &self.vault else { return self.read_file() };
		match vault.read() {
			Ok(Some(secret)) => {
				// A file next to a keychain entry is stale (saved while the store was down).
				self.remove_file()?;
				Ok(Some(secret))
			},
			Ok(None) => {
				let Some(bytes) = self.read_file()? else { return Ok(None) };
				match vault.write(&bytes) {
					Ok(()) => self.remove_file()?,
					// Keep the working file; the token is still usable and we retry next time.
					Err(e) => note(&format!("cannot move token.json into the credential store: {}", e.into_io())),
				}
				Ok(Some(bytes))
			},
			Err(VaultError::Unavailable(m)) => {
				note(&format!("credential store unavailable ({m}); using token.json"));
				self.read_file()
			},
			Err(e) => Err(e.into_io()),
		}
	}

	pub fn save(&self, bytes: &[u8]) -> io::Result<()> {
		let Some(vault) = &self.vault else { return config::write_private(&self.file, bytes) };
		match vault.write(bytes) {
			Ok(()) => self.remove_file(),
			Err(VaultError::Unavailable(m)) => {
				note(&format!("credential store unavailable ({m}); writing token.json"));
				config::write_private(&self.file, bytes)
			},
			Err(e) => Err(e.into_io()),
		}
	}

	/// Deletes the token from the credential store and the file. Both are always attempted.
	pub fn delete(&self) -> io::Result<()> {
		let file = self.remove_file();
		let vault = match self.vault.as_ref().map(|v| v.remove()) {
			None | Some(Ok(())) => Ok(()),
			Some(Err(VaultError::Unavailable(m))) => {
				note(&format!("credential store unavailable ({m}); nothing to delete there"));
				Ok(())
			},
			Some(Err(e)) => Err(e.into_io()),
		};
		file.and(vault)
	}

	fn read_file(&self) -> io::Result<Option<Vec<u8>>> {
		match fs::read(&self.file) {
			Ok(bytes) => Ok(Some(bytes)),
			Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
			Err(e) => Err(e),
		}
	}

	fn remove_file(&self) -> io::Result<()> {
		match fs::remove_file(&self.file) {
			Ok(()) => Ok(()),
			Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
			Err(e) => Err(e),
		}
	}
}

fn os_vault() -> Option<Box<dyn Vault>> {
	// Unit tests never reach the real credential store.
	if cfg!(test) || config::home_override().is_some() {
		return None;
	}
	match credential::Keyring::open() {
		Ok(vault) => Some(Box::new(vault)),
		Err(error) => {
			note(&format!("credential store unusable ({}); using token.json", error.into_io()));
			None
		},
	}
}

/// Records a credential-store fallback once per process in `$TMPDIR/turntable-debug.log`.
fn note(line: &str) {
	static ONCE: Once = Once::new();
	ONCE.call_once(|| {
		let path = std::env::temp_dir().join("turntable-debug.log");
		if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(path) {
			let _ = writeln!(f, "token store: {line}");
		}
	});
}

#[cfg(test)]
mod tests {
	use std::{cell::RefCell, rc::Rc};

	use super::*;

	#[derive(Clone, Copy, PartialEq)]
	enum Mode {
		Working,
		Down,
		Broken,
	}

	/// In-memory credential store; the slot and mode are shared so tests can inspect them.
	#[derive(Clone)]
	struct Fake {
		slot: Rc<RefCell<Option<Vec<u8>>>>,
		mode: Rc<RefCell<Mode>>,
	}

	impl Fake {
		fn new(mode: Mode) -> Self {
			Self { slot: Rc::default(), mode: Rc::new(RefCell::new(mode)) }
		}

		fn check(&self) -> Result<(), VaultError> {
			match *self.mode.borrow() {
				Mode::Working => Ok(()),
				Mode::Down => Err(VaultError::Unavailable("down".into())),
				Mode::Broken => Err(VaultError::Failed("denied".into())),
			}
		}
	}

	impl Vault for Fake {
		fn read(&self) -> Result<Option<Vec<u8>>, VaultError> {
			self.check()?;
			Ok(self.slot.borrow().clone())
		}

		fn write(&self, secret: &[u8]) -> Result<(), VaultError> {
			self.check()?;
			*self.slot.borrow_mut() = Some(secret.to_vec());
			Ok(())
		}

		fn remove(&self) -> Result<(), VaultError> {
			self.check()?;
			*self.slot.borrow_mut() = None;
			Ok(())
		}
	}

	fn scratch(name: &str) -> PathBuf {
		let dir = std::env::temp_dir().join(format!("turntable-store-{name}-{}", std::process::id()));
		let _ = fs::remove_dir_all(&dir);
		fs::create_dir_all(&dir).unwrap();
		dir
	}

	fn store(dir: &Path, vault: Option<Fake>) -> TokenStore {
		TokenStore { vault: vault.map(|v| -> Box<dyn Vault> { Box::new(v) }), file: dir.join("token.json") }
	}

	#[test]
	fn file_only_roundtrip_and_delete() {
		let dir = scratch("file");
		let s = store(&dir, None);
		assert_eq!(s.load().unwrap(), None);
		s.save(b"one").unwrap();
		assert_eq!(fs::read(dir.join("token.json")).unwrap(), b"one");
		assert_eq!(s.load().unwrap().as_deref(), Some(&b"one"[..]));
		s.delete().unwrap();
		s.delete().unwrap();
		assert_eq!(s.load().unwrap(), None);
		fs::remove_dir_all(dir).unwrap();
	}

	#[test]
	fn save_goes_to_vault_and_drops_file() {
		let dir = scratch("save");
		let vault = Fake::new(Mode::Working);
		let s = store(&dir, Some(vault.clone()));
		fs::write(dir.join("token.json"), b"old").unwrap();
		s.save(b"new").unwrap();
		assert_eq!(vault.slot.borrow().as_deref(), Some(&b"new"[..]));
		assert!(!dir.join("token.json").exists());
		assert_eq!(s.load().unwrap().as_deref(), Some(&b"new"[..]));
		fs::remove_dir_all(dir).unwrap();
	}

	#[test]
	fn legacy_file_migrates_into_empty_vault() {
		let dir = scratch("migrate");
		let vault = Fake::new(Mode::Working);
		let s = store(&dir, Some(vault.clone()));
		fs::write(dir.join("token.json"), b"legacy").unwrap();
		assert_eq!(s.load().unwrap().as_deref(), Some(&b"legacy"[..]));
		assert_eq!(vault.slot.borrow().as_deref(), Some(&b"legacy"[..]));
		assert!(!dir.join("token.json").exists());
		assert_eq!(s.load().unwrap().as_deref(), Some(&b"legacy"[..]));
		fs::remove_dir_all(dir).unwrap();
	}

	#[test]
	fn vault_wins_over_stale_file() {
		let dir = scratch("stale");
		let vault = Fake::new(Mode::Working);
		*vault.slot.borrow_mut() = Some(b"vault".to_vec());
		let s = store(&dir, Some(vault));
		fs::write(dir.join("token.json"), b"stale").unwrap();
		assert_eq!(s.load().unwrap().as_deref(), Some(&b"vault"[..]));
		assert!(!dir.join("token.json").exists());
		fs::remove_dir_all(dir).unwrap();
	}

	#[test]
	fn logout_clears_vault_and_file() {
		let dir = scratch("logout");
		let vault = Fake::new(Mode::Working);
		*vault.slot.borrow_mut() = Some(b"vault".to_vec());
		let s = store(&dir, Some(vault.clone()));
		fs::write(dir.join("token.json"), b"file").unwrap();
		s.delete().unwrap();
		assert!(vault.slot.borrow().is_none());
		assert!(!dir.join("token.json").exists());
		assert_eq!(s.load().unwrap(), None);
		fs::remove_dir_all(dir).unwrap();
	}

	#[test]
	fn unavailable_vault_falls_back_to_file() {
		let dir = scratch("down");
		let vault = Fake::new(Mode::Down);
		let s = store(&dir, Some(vault.clone()));
		s.save(b"tok").unwrap();
		assert_eq!(fs::read(dir.join("token.json")).unwrap(), b"tok");
		assert_eq!(s.load().unwrap().as_deref(), Some(&b"tok"[..]));
		// Back up: the file moves into the vault.
		*vault.mode.borrow_mut() = Mode::Working;
		assert_eq!(s.load().unwrap().as_deref(), Some(&b"tok"[..]));
		assert_eq!(vault.slot.borrow().as_deref(), Some(&b"tok"[..]));
		assert!(!dir.join("token.json").exists());
		fs::remove_dir_all(dir).unwrap();
	}

	#[test]
	fn failed_vault_is_an_error_not_a_fallback() {
		let dir = scratch("broken");
		let s = store(&dir, Some(Fake::new(Mode::Broken)));
		assert!(s.save(b"tok").is_err());
		assert!(!dir.join("token.json").exists());
		assert!(s.load().is_err());
		assert!(s.delete().is_err());
		fs::remove_dir_all(dir).unwrap();
	}

	#[test]
	fn failed_migration_keeps_file_and_token() {
		// Reads succeed (empty) but the write is refused.
		struct ReadOnly(Fake);
		impl Vault for ReadOnly {
			fn read(&self) -> Result<Option<Vec<u8>>, VaultError> {
				self.0.read()
			}

			fn write(&self, _: &[u8]) -> Result<(), VaultError> {
				Err(VaultError::Failed("denied".into()))
			}

			fn remove(&self) -> Result<(), VaultError> {
				self.0.remove()
			}
		}
		let dir = scratch("migfail");
		let vault = Fake::new(Mode::Working);
		let s = store(&dir, Some(vault.clone()));
		fs::write(dir.join("token.json"), b"legacy").unwrap();
		let s = TokenStore { vault: Some(Box::new(ReadOnly(vault))), file: s.file };
		assert_eq!(s.load().unwrap().as_deref(), Some(&b"legacy"[..]));
		assert!(dir.join("token.json").exists());
		fs::remove_dir_all(dir).unwrap();
	}
}
