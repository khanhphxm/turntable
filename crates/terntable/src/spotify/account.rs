//! The library's own Spotify app (the Web API quota is per app): Authorization Code + PKCE
//! (RFC 7636, S256) with a loopback redirect, plus token storage and refresh.
//!
//! Setup (once, by the user):
//! 1. Create an app at <https://developer.spotify.com/dashboard>.
//! 2. Add the redirect URI [`REDIRECT_URI`] exactly (Spotify requires the loopback IP literal
//!    `127.0.0.1`, not `localhost`).
//! 3. Write the app's Client ID into `<config>/client_id`.

use std::{
	io::{self, BufRead, BufReader, Read, Write},
	net::{TcpListener, TcpStream},
	path::{Path, PathBuf},
	time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use super::{
	auth::now_unix,
	error::{Error, Result},
	http::{Http, Method, Req},
	pkce,
	token_store::TokenStore,
};
use crate::app::config;

pub const REDIRECT_URI: &str = "http://127.0.0.1:8898/callback";
const LISTEN_ADDR: &str = "127.0.0.1:8898";
const AUTHORIZE_URL: &str = "https://accounts.spotify.com/authorize";
const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
const SCOPES: &str = "playlist-read-private playlist-read-collaborative user-library-read user-library-modify \
                      user-read-playback-state user-modify-playback-state user-read-recently-played user-top-read \
                      user-follow-read playlist-modify-public playlist-modify-private user-read-private";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);
/// Refresh this long before the access token actually expires.
const REFRESH_SKEW_SECS: u64 = 60;

pub fn client_id_path(config_dir: &Path) -> PathBuf {
	config_dir.join("client_id")
}

pub fn read_client_id(config_dir: &Path) -> Result<String> {
	let path = client_id_path(config_dir);
	config::read_secret(&path).map_err(|e| {
		if e.kind() == io::ErrorKind::NotFound {
			Error::Auth(format!(
				"missing {}: put your Spotify app's Client ID there (redirect URI to register: {REDIRECT_URI})",
				path.display()
			))
		} else {
			Error::Io(e)
		}
	})
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredToken {
	pub access_token:  String,
	pub refresh_token: String,
	/// Unix seconds.
	pub expires_at:    u64,
	#[serde(default)]
	pub scope:         String,
	/// Bound by a successful `/me`; a new browser sign-in has no cached identity.
	#[serde(default)]
	pub user_id:       Option<String>,
}

impl std::fmt::Debug for StoredToken {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("Token").field("expires_at", &self.expires_at).field("scope", &self.scope).finish_non_exhaustive()
	}
}

#[derive(Deserialize)]
struct TokenResponse {
	access_token:  String,
	expires_in:    u64,
	refresh_token: Option<String>,
	scope:         Option<String>,
}

impl StoredToken {
	/// True when the access token is still usable for at least [`REFRESH_SKEW_SECS`].
	pub fn is_fresh(&self, now: u64) -> bool {
		now.saturating_add(REFRESH_SKEW_SECS) < self.expires_at
	}

	/// Builds the stored token from a token-endpoint response. Spotify may rotate the refresh
	/// token; when it does not, the previous one stays valid and is kept.
	fn from_response(resp: TokenResponse, now: u64, previous: Option<&StoredToken>) -> Result<Self> {
		let refresh_token = resp
			.refresh_token
			.or_else(|| previous.map(|t| t.refresh_token.clone()))
			.ok_or_else(|| Error::Parse("token response without refresh_token".into()))?;
		Ok(Self {
			access_token: resp.access_token,
			refresh_token,
			expires_at: now.saturating_add(resp.expires_in),
			scope: resp.scope.or_else(|| previous.map(|t| t.scope.clone())).unwrap_or_default(),
			user_id: previous.and_then(|t| t.user_id.clone()),
		})
	}
}

/// Loads the stored token. `AuthRequired` when the user never logged in (or logged out).
pub fn load_token(config_dir: &Path) -> Result<StoredToken> {
	match TokenStore::open(config_dir).load()? {
		// A stored token this version cannot read is no token: the user connects again.
		Some(bytes) => match serde_json::from_slice(&bytes) {
			Ok(token) => Ok(token),
			Err(_) => Err(Error::AuthRequired),
		},
		None => Err(Error::AuthRequired),
	}
}

fn store_token(config_dir: &Path, token: &StoredToken) -> Result<()> {
	TokenStore::open(config_dir).save(&serde_json::to_vec(token)?)?;
	Ok(())
}

/// Disconnect: deletes the stored tokens (credential store entry and any `token.json`). The
/// user can additionally revoke the app at <https://www.spotify.com/account/apps/>.
pub fn logout(config_dir: &Path) -> Result<()> {
	TokenStore::open(config_dir).delete()?;
	Ok(())
}

/// A logged-in session: the refreshable token and where to persist it.
pub struct Stored {
	client_id:  String,
	token:      StoredToken,
	config_dir: PathBuf,
}

impl Stored {
	pub fn open(config_dir: &Path) -> Result<Self> {
		let client_id = read_client_id(config_dir)?;
		let token = load_token(config_dir)?;
		Ok(Self { client_id, token, config_dir: config_dir.to_owned() })
	}

	/// Exchanges the refresh token for a new access token and persists the result (including a
	/// rotated refresh token) before returning.
	pub fn refresh(&mut self, http: &Http) -> Result<()> {
		let form =
			[("grant_type", "refresh_token"), ("refresh_token", self.token.refresh_token.as_str()), ("client_id", self.client_id.as_str())];
		let body = http.send(&Req {
			method: Method::Post,
			url:    TOKEN_URL,
			query:  &[],
			bearer: None,
			form:   Some(&form),
			json:   None,
			limit:  64 * 1024,
		})?;
		let resp: TokenResponse = serde_json::from_slice(&body)?;
		self.token = StoredToken::from_response(resp, now_unix(), Some(&self.token))?;
		// `terntable logout` (another process) deletes the stored token: a running
		// player must stay logged out rather than quietly write it back.
		if TokenStore::open(&self.config_dir).load()?.is_none() {
			return Err(Error::AuthRequired);
		}
		store_token(&self.config_dir, &self.token)
	}
}

/// Interactive login. `announce` receives the authorization URL (print it in case the browser
/// cannot be opened). Blocks until the redirect arrives or [`LOGIN_TIMEOUT`] passes, then
/// stores the token in the OS credential store (or `<config>/token.json`, see [`TokenStore`]).
pub fn login(config_dir: &Path, announce: impl FnOnce(&str)) -> Result<()> {
	let client_id = read_client_id(config_dir)?;
	let listener =
		TcpListener::bind(LISTEN_ADDR).map_err(|e| Error::Auth(format!("cannot listen on {LISTEN_ADDR} for the redirect: {e}")))?;
	listener.set_nonblocking(true)?;

	let verifier = pkce::verifier()?;
	let state = pkce::random_b64url::<16>()?;
	let url = authorize_url(&client_id, &pkce::challenge(&verifier), &state);
	announce(&url);
	super::open_browser(&url);

	let code = check_callback(wait_for_callback(&listener, LOGIN_TIMEOUT)?, &state)?;

	let http = Http::new();
	let form = [
		("grant_type", "authorization_code"),
		("code", code.as_str()),
		("redirect_uri", REDIRECT_URI),
		("client_id", client_id.as_str()),
		("code_verifier", verifier.as_str()),
	];
	let body = http
		.send(&Req {
			method: Method::Post,
			url:    TOKEN_URL,
			query:  &[],
			bearer: None,
			form:   Some(&form),
			json:   None,
			limit:  64 * 1024,
		})
		.map_err(|e| match e {
			Error::AuthExpired => Error::Auth("authorization code was rejected".into()),
			other => other,
		})?;
	let token = StoredToken::from_response(serde_json::from_slice(&body)?, now_unix(), None)?;
	store_token(config_dir, &token)
}

fn authorize_url(client_id: &str, challenge: &str, state: &str) -> String {
	format!(
		"{AUTHORIZE_URL}?response_type=code&client_id={}&scope={}&redirect_uri={}&state={}&code_challenge_method=S256&\
		 code_challenge={}",
		pkce::urlencode(client_id),
		pkce::urlencode(SCOPES),
		pkce::urlencode(REDIRECT_URI),
		pkce::urlencode(state),
		pkce::urlencode(challenge),
	)
}

/// The stored authorization and the account it was last verified to belong to. Keeping the
/// identity beside the refresh token lets the library read only this account's snapshot before
/// asking the network, and keeps the binding through token rotation.
pub struct Account {
	stored: std::sync::Arc<std::sync::Mutex<Stored>>,
}

impl Account {
	pub fn open(config_dir: &Path) -> Result<Self> {
		Ok(Self { stored: std::sync::Arc::new(std::sync::Mutex::new(Stored::open(config_dir)?)) })
	}

	pub fn user_id(&self) -> Option<String> {
		self.stored.lock().unwrap_or_else(std::sync::PoisonError::into_inner).token.user_id.clone()
	}

	/// Records the identity verified by `/me`, never one guessed from the previous snapshot.
	pub fn remember_user(&self, user_id: &str) -> Result<()> {
		let mut stored = self.stored.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
		if stored.token.user_id.as_deref() != Some(user_id) {
			stored.token.user_id = Some(user_id.to_owned());
			store_token(&stored.config_dir, &stored.token)?;
		}
		Ok(())
	}

	/// Refreshes the access token on demand, persisting the token and its account binding.
	pub fn source(&self) -> super::TokenSource {
		let stored = self.stored.clone();
		let http = Http::new();
		std::sync::Arc::new(move || {
			let mut stored = stored.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
			if !stored.token.is_fresh(now_unix()) {
				stored.refresh(&http)?;
			}
			Ok(super::Token { access_token: stored.token.access_token.clone(), expires_at: stored.token.expires_at })
		})
	}
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Callback {
	code:  Option<String>,
	state: Option<String>,
	error: Option<String>,
}

/// Parses a request head. `None` for anything that is not `GET /callback`.
fn parse_callback(head: &str) -> Option<Callback> {
	let line = head.lines().next()?;
	let mut parts = line.split(' ');
	if parts.next()? != "GET" {
		return None;
	}
	let target = parts.next()?;
	let (path, query) = target.split_once('?').unwrap_or((target, ""));
	if path != "/callback" {
		return None;
	}
	let mut cb = Callback::default();
	for pair in query.split('&') {
		let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
		match k {
			"code" => cb.code = Some(pkce::urldecode(v)),
			"state" => cb.state = Some(pkce::urldecode(v)),
			"error" => cb.error = Some(pkce::urldecode(v)),
			_ => {},
		}
	}
	Some(cb)
}

/// Validates the redirect and returns the authorization code.
fn check_callback(cb: Callback, expected_state: &str) -> Result<String> {
	if let Some(err) = cb.error {
		return Err(Error::Auth(format!("authorization denied ({err})")));
	}
	if cb.state.as_deref() != Some(expected_state) {
		return Err(Error::Auth("state mismatch in redirect".into()));
	}
	cb.code.ok_or_else(|| Error::Auth("redirect carried no authorization code".into()))
}

fn wait_for_callback(listener: &TcpListener, timeout: Duration) -> Result<Callback> {
	let deadline = Instant::now() + timeout;
	loop {
		let left = deadline.saturating_duration_since(Instant::now());
		if left.is_zero() {
			return Err(Error::Auth("timed out waiting for the browser redirect".into()));
		}
		match listener.accept() {
			Ok((stream, _)) => {
				if let Some(cb) = serve(stream) {
					return Ok(cb);
				}
			},
			Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
				// Only the interactive sign-in waits here; std has no portable listener timeout.
				std::thread::sleep(left.min(Duration::from_millis(100)));
			},
			Err(e) if e.kind() == io::ErrorKind::Interrupted => {},
			Err(e) => return Err(Error::Io(e)),
		}
	}
}

/// Answers one connection. Returns the callback if this was the `/callback` request.
fn serve(mut stream: TcpStream) -> Option<Callback> {
	// Accepted sockets inherit O_NONBLOCK on some platforms.
	stream.set_nonblocking(false).ok()?;
	stream.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
	// The request line is all that matters; the cap keeps a hostile peer from making us buffer more.
	let mut line = Vec::new();
	let _ = BufReader::new(Read::take(&mut stream, 4096)).read_until(b'\n', &mut line);
	let cb = parse_callback(&String::from_utf8_lossy(&line));
	let (status, body) = match &cb {
		Some(c) if c.code.is_some() && c.error.is_none() => ("200 OK", "terntable is connected to Spotify. You can close this tab."),
		Some(_) => ("400 Bad Request", "Spotify login failed. Return to the terminal."),
		None => ("404 Not Found", "not found"),
	};
	let _ = write!(
		stream,
		"HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
		body.len()
	);
	let _ = stream.flush();
	cb
}

#[cfg(test)]
mod tests {
	use super::*;

	fn token(expires_at: u64) -> StoredToken {
		StoredToken { access_token: "at".into(), refresh_token: "rt".into(), expires_at, scope: "s".into(), user_id: None }
	}

	#[test]
	fn refresh_decision_uses_skew() {
		let t = token(1_000);
		assert!(t.is_fresh(0));
		assert!(t.is_fresh(1_000 - REFRESH_SKEW_SECS - 1));
		assert!(!t.is_fresh(1_000 - REFRESH_SKEW_SECS));
		assert!(!t.is_fresh(1_000));
		assert!(!t.is_fresh(5_000));
		assert!(!token(0).is_fresh(u64::MAX));
	}

	#[test]
	fn rotated_refresh_token_is_adopted_and_absent_one_kept() {
		let old = StoredToken { user_id: Some("alex".into()), ..token(10) };
		let rotated =
			TokenResponse { access_token: "at2".into(), expires_in: 3600, refresh_token: Some("rt2".into()), scope: None };
		let t = StoredToken::from_response(rotated, 100, Some(&old)).unwrap();
		assert_eq!((t.access_token.as_str(), t.refresh_token.as_str()), ("at2", "rt2"));
		assert_eq!(t.expires_at, 3700);
		assert_eq!(t.scope, "s");
		assert_eq!(t.user_id.as_deref(), Some("alex"));

		let same = TokenResponse { access_token: "at3".into(), expires_in: 60, refresh_token: None, scope: Some("n".into()) };
		let t = StoredToken::from_response(same, 100, Some(&old)).unwrap();
		assert_eq!((t.refresh_token.as_str(), t.scope.as_str()), ("rt", "n"));

		let first = TokenResponse { access_token: "a".into(), expires_in: 1, refresh_token: None, scope: None };
		assert!(matches!(StoredToken::from_response(first, 0, None), Err(Error::Parse(_))));
		let login = TokenResponse { access_token: "new".into(), expires_in: 3600, refresh_token: Some("new-rt".into()), scope: None };
		assert_eq!(StoredToken::from_response(login, 0, None).unwrap().user_id, None, "a different sign-in has no old identity");
	}

	#[test]
	fn callback_parsing_and_validation() {
		let head = "GET /callback?code=AQD%2Bxyz&state=st%3D HTTP/1.1\r\nHost: 127.0.0.1:8898\r\n\r\n";
		let cb = parse_callback(head).unwrap();
		assert_eq!(cb.code.as_deref(), Some("AQD+xyz"));
		assert_eq!(check_callback(cb, "st=").unwrap(), "AQD+xyz");

		assert!(parse_callback("GET /favicon.ico HTTP/1.1\r\n\r\n").is_none());
		assert!(parse_callback("POST /callback?code=x HTTP/1.1\r\n\r\n").is_none());
		assert!(parse_callback("").is_none());

		let cb = parse_callback("GET /callback?code=c&state=evil HTTP/1.1\r\n\r\n").unwrap();
		assert!(matches!(check_callback(cb, "good"), Err(Error::Auth(m)) if m.contains("state")));
		let cb = parse_callback("GET /callback?error=access_denied&state=good HTTP/1.1\r\n\r\n").unwrap();
		assert!(matches!(check_callback(cb, "good"), Err(Error::Auth(m)) if m.contains("access_denied")));
		let cb = parse_callback("GET /callback?state=good HTTP/1.1\r\n\r\n").unwrap();
		assert!(matches!(check_callback(cb, "good"), Err(Error::Auth(_))));
	}

	#[test]
	fn loopback_listener_ignores_other_paths_and_accepts_the_redirect() {
		let listener = TcpListener::bind("127.0.0.1:0").unwrap();
		listener.set_nonblocking(true).unwrap();
		let addr = listener.local_addr().unwrap();
		let browser = std::thread::spawn(move || {
			for (path, status) in [("/favicon.ico", "404 Not Found"), ("/callback?code=code&state=state", "200 OK")] {
				let mut stream = TcpStream::connect(addr).unwrap();
				stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
				write!(stream, "GET {path} HTTP/1.1\r\nHost: {addr}\r\n\r\n").unwrap();
				let mut response = String::new();
				stream.read_to_string(&mut response).unwrap();
				assert!(response.starts_with(&format!("HTTP/1.1 {status}\r\n")));
			}
		});
		let callback = wait_for_callback(&listener, Duration::from_secs(3)).unwrap();
		assert_eq!(check_callback(callback, "state").unwrap(), "code");
		browser.join().unwrap();
	}

	#[test]
	fn loopback_listener_times_out_without_a_redirect() {
		let listener = TcpListener::bind("127.0.0.1:0").unwrap();
		listener.set_nonblocking(true).unwrap();
		assert!(matches!(wait_for_callback(&listener, Duration::from_millis(10)), Err(Error::Auth(m)) if m.contains("timed out")));
	}

	#[test]
	fn authorize_url_carries_pkce_and_exact_redirect() {
		let url = authorize_url("cid", "chal", "st");
		assert!(url.starts_with("https://accounts.spotify.com/authorize?response_type=code&client_id=cid&"));
		assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A8898%2Fcallback"));
		assert!(url.contains("code_challenge_method=S256&code_challenge=chal"));
		// What the app reads and controls; library-modify is the Like button.
		let scope = url.split("scope=").nth(1).unwrap_or_default().split('&').next().unwrap_or_default().replace("%20", " ");
		for needed in
			["playlist-read-private", "user-library-read", "user-library-modify", "user-modify-playback-state", "user-read-recently-played"]
		{
			assert!(scope.split(' ').any(|s| s == needed), "missing {needed} in {scope}");
		}
	}
}
