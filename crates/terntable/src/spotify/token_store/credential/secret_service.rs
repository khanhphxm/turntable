//! A missing or locked Secret Service daemon permits the private-file fallback on Linux.

pub(super) fn unavailable(error: &keyring::Error) -> bool {
	matches!(error, keyring::Error::NoStorageAccess(_) | keyring::Error::PlatformFailure(_))
}
