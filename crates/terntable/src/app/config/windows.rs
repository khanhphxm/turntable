//! Windows files inherit the user's directory ACL; Unix mode bits do not apply.

use std::fs::OpenOptions;

pub(super) fn private_permissions(_options: &mut OpenOptions) {}
