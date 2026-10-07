//! Keychain/Credential Manager failures (including denied access) must not downgrade to a file.

pub(super) fn unavailable(_error: &keyring::Error) -> bool {
	false
}
