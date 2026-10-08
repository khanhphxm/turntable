//! Optional Web API token plumbing and platform browser handoff. The application uses
//! native playback authentication and never loads a separate OAuth account.

#[cfg(unix)]
use std::process::{Command, Stdio};
use std::{
	sync::Arc,
	time::{SystemTime, UNIX_EPOCH},
};

use super::error::{Error, Result};

/// Get a new token this long before the current one expires.
const REFRESH_SKEW_SECS: u64 = 60;

pub fn now_unix() -> u64 {
	SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[derive(Clone, PartialEq, Eq)]
pub struct Token {
	pub access_token: String,
	/// Unix seconds.
	pub expires_at:   u64,
}

impl std::fmt::Debug for Token {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("Token").field("expires_at", &self.expires_at).finish_non_exhaustive()
	}
}

impl Token {
	/// True when the access token remains usable beyond the refresh safety margin.
	pub fn is_fresh(&self, now: u64) -> bool {
		now.saturating_add(REFRESH_SKEW_SECS) < self.expires_at
	}
}

/// Mints an access token (blocking; it may ask Spotify). Shared by every thread that calls the
/// Web API.
pub type TokenSource = Arc<dyn Fn() -> Result<Token> + Send + Sync>;

/// The current token and where the next one comes from.
pub struct Session {
	source: Option<TokenSource>,
	token:  Option<Token>,
}

impl Session {
	pub fn new(source: TokenSource) -> Self {
		Self { source: Some(source), token: None }
	}

	pub(super) fn empty() -> Self {
		Self { source: None, token: None }
	}

	pub(super) fn has_source(&self) -> bool {
		self.source.is_some()
	}

	/// An access token with time left, fetching a new one first when it runs out.
	pub fn access_token(&mut self) -> Result<String> {
		match &self.token {
			Some(t) if t.is_fresh(now_unix()) => Ok(t.access_token.clone()),
			_ => self.renew(),
		}
	}

	/// A new token (after the Web API refused the last one).
	pub fn renew(&mut self) -> Result<String> {
		let token = self.source.as_ref().ok_or(Error::AuthRequired)?()?;
		if token.access_token.is_empty() {
			return Err(Error::AuthRequired);
		}
		let access = token.access_token.clone();
		self.token = Some(token);
		Ok(access)
	}

	/// The token last handed out, if any.
	pub fn current(&self) -> Option<&str> {
		self.token.as_ref().map(|t| t.access_token.as_str())
	}
}

#[cfg(unix)]
pub fn open_browser(url: &str) {
	let program = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
	// Best effort: the URL was announced, so the user can open it by hand.
	// `open`/`xdg-open` hand off and exit at once; wait so no zombie is left.
	let _ = Command::new(program).arg(url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
}

#[cfg(windows)]
pub fn open_browser(url: &str) {
	use windows::{
		Win32::{
			System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize},
			UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
		},
		core::PCWSTR,
	};
	let url: Vec<u16> = url.encode_utf16().chain([0]).collect();
	// SAFETY: the URL is NUL-terminated and lives through ShellExecuteW. Initializing COM
	// lets shell protocol handlers run; an existing apartment is left alone if already set.
	unsafe {
		let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
		// Best effort, as on Unix: the announced URL can still be opened by hand.
		let _ = ShellExecuteW(None, None, PCWSTR(url.as_ptr()), None, None, SW_SHOWNORMAL);
		if com.is_ok() {
			CoUninitialize();
		}
	}
}

#[cfg(test)]
mod tests {
	use std::sync::atomic::{AtomicU32, Ordering};

	use super::*;

	#[test]
	fn a_token_is_reused_until_it_nears_expiry_then_renewed() {
		let minted = Arc::new(AtomicU32::new(0));
		let counter = minted.clone();
		let source: TokenSource = Arc::new(move || {
			let n = counter.fetch_add(1, Ordering::Relaxed);
			// The first token is already within the skew of expiring; the second has an hour.
			let expires_at = if n == 0 { now_unix() + 30 } else { now_unix() + 3600 };
			Ok(Token { access_token: format!("t{n}"), expires_at })
		});
		let mut s = Session::new(source);
		assert_eq!(s.access_token().unwrap(), "t0");
		assert_eq!(s.access_token().unwrap(), "t1", "a token about to expire is replaced");
		assert_eq!(s.access_token().unwrap(), "t1");
		assert_eq!(s.renew().unwrap(), "t2", "a refused token is replaced at once");
		assert_eq!(minted.load(Ordering::Relaxed), 3);
	}
}
