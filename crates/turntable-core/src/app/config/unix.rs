//! Private token/settings files are readable only by the current Unix user.

use std::{fs::OpenOptions, os::unix::fs::OpenOptionsExt};

pub(super) fn private_permissions(options: &mut OpenOptions) {
	options.mode(0o600);
}
