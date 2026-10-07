//! The keyring adapter shared by macOS Keychain, Linux Secret Service and Windows Credential Manager.

#[cfg_attr(target_os = "linux", path = "credential/secret_service.rs")]
#[cfg_attr(any(target_os = "macos", windows), path = "credential/native.rs")]
mod platform;

use super::{Vault, VaultError};

const SERVICE: &str = "terntable";
const ACCOUNT: &str = "spotify-token";

pub(super) struct Keyring(keyring::Entry);

impl Keyring {
	pub(super) fn open() -> Result<Self, VaultError> {
		keyring::Entry::new(SERVICE, ACCOUNT).map(Self).map_err(|error| classify(&error))
	}
}

fn classify(error: &keyring::Error) -> VaultError {
	let message = error.to_string();
	if platform::unavailable(error) { VaultError::Unavailable(message) } else { VaultError::Failed(message) }
}

impl Vault for Keyring {
	fn read(&self) -> Result<Option<Vec<u8>>, VaultError> {
		match self.0.get_secret() {
			Ok(secret) => Ok(Some(secret)),
			Err(keyring::Error::NoEntry) => Ok(None),
			Err(error) => Err(classify(&error)),
		}
	}

	fn write(&self, secret: &[u8]) -> Result<(), VaultError> {
		self.0.set_secret(secret).map_err(|error| classify(&error))
	}

	fn remove(&self) -> Result<(), VaultError> {
		match self.0.delete_credential() {
			Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
			Err(error) => Err(classify(&error)),
		}
	}
}
