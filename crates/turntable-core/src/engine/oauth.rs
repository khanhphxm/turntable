//! Browser PKCE sign-in with a cancellation-owned, nonblocking loopback listener.

use std::{
	io::{self, Read as _, Write as _},
	net::{SocketAddr, TcpStream},
	thread,
	time::{Duration, Instant},
};

use oauth2::{
	AuthUrl, AuthorizationCode, ClientId, CsrfToken, PkceCodeChallenge, RedirectUrl, Scope, TokenResponse as _, TokenUrl,
	basic::BasicClient, url::Url,
};

use super::{NETWORK_TIMEOUT, OAUTH_REDIRECT, OAUTH_SCOPES, StartAuthorization, start::SignInTimedOut};

pub(crate) const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const POLL: Duration = Duration::from_millis(20);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_REQUEST_LINE: usize = 8192;

pub(super) fn sign_in(
	client_id: &str,
	runtime: &tokio::runtime::Runtime,
	authorization: &StartAuthorization,
	proxy: &str,
) -> io::Result<super::Credentials> {
	if authorization.deadline().is_none() {
		authorization.set_deadline(Some(Instant::now() + SIGN_IN_TIMEOUT));
	}
	authorization.check()?;
	let redirect = Url::parse(OAUTH_REDIRECT).map_err(io::Error::other)?;
	let address = redirect
		.socket_addrs(|| None)
		.map_err(io::Error::other)?
		.into_iter()
		.next()
		.ok_or_else(|| io::Error::other("Sign-in redirect has no loopback address"))?;
	let client = BasicClient::new(ClientId::new(client_id.to_owned()))
		.set_auth_uri(AuthUrl::new("https://accounts.spotify.com/authorize".into()).map_err(io::Error::other)?)
		.set_token_uri(TokenUrl::new("https://accounts.spotify.com/api/token".into()).map_err(io::Error::other)?)
		.set_redirect_uri(RedirectUrl::new(OAUTH_REDIRECT.into()).map_err(io::Error::other)?);
	let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
	let (url, state) = client
		.authorize_url(CsrfToken::new_random)
		.add_scopes(OAUTH_SCOPES.iter().map(|scope| Scope::new((*scope).to_owned())))
		.set_pkce_challenge(challenge)
		.url();
	// Bind before opening the browser, so even an immediate redirect can be accepted.
	let callback = Callback::bind(authorization, address)?;
	crate::spotify::open_browser(url.as_str());
	let code = callback.wait(&redirect, state.secret())?;
	drop(callback);
	authorization.check()?;
	// No detached token-exchange thread: dropping this future closes an in-flight request.
	let token = runtime.block_on(async {
		let http = crate::net::proxy::oauth(proxy, NETWORK_TIMEOUT)?;
		tokio::select! {
			biased;
			error = cancelled(authorization) => Err(error),
			result = client.exchange_code(code).set_pkce_verifier(verifier).request_async(&http) => result.map_err(io::Error::other),
		}
	})?;
	authorization.check()?;
	Ok(super::Credentials::with_access_token(token.access_token().secret()))
}

async fn cancelled(authorization: &StartAuthorization) -> io::Error {
	loop {
		if let Err(error) = authorization.check() {
			return error;
		}
		tokio::time::sleep(POLL).await;
	}
}

/// The authorization owns the socket so Cancel can close it before a retry starts.
struct Callback<'a>(&'a StartAuthorization);

impl<'a> Callback<'a> {
	fn bind(authorization: &'a StartAuthorization, address: SocketAddr) -> io::Result<Self> {
		authorization.listen(address)?;
		Ok(Self(authorization))
	}

	fn wait(&self, redirect: &Url, state: &str) -> io::Result<AuthorizationCode> {
		loop {
			let mut stream = match self.0.accept_callback() {
				Ok(stream) => stream,
				Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
					thread::sleep(POLL);
					continue;
				},
				Err(error) => return Err(error),
			};
			stream.set_nonblocking(true)?;
			let request = match read_request(&mut stream, self.0) {
				Ok(request) => request,
				Err(error)
					if matches!(error.kind(), io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof | io::ErrorKind::TimedOut)
						&& !error.get_ref().is_some_and(|source| source.is::<SignInTimedOut>()) =>
				{
					continue;
				},
				Err(error) => return Err(error),
			};
			self.0.check()?;
			match callback_code(&request, redirect, state) {
				Some(result) => {
					respond(&mut stream, self.0, "200 OK", "You can close this tab and return to Turntable.")?;
					self.0.check()?;
					return result;
				},
				None => respond(&mut stream, self.0, "400 Bad Request", "This sign-in link is no longer valid. Return to Turntable.")?,
			}
		}
	}
}

impl Drop for Callback<'_> {
	fn drop(&mut self) {
		self.0.stop_listening();
	}
}

fn read_request(stream: &mut TcpStream, authorization: &StartAuthorization) -> io::Result<String> {
	let until = Instant::now() + REQUEST_TIMEOUT;
	let mut line = Vec::with_capacity(512);
	let mut buffer = [0; 512];
	loop {
		authorization.check()?;
		if Instant::now() >= until {
			return Err(io::Error::new(io::ErrorKind::TimedOut, "Sign-in callback request was incomplete"));
		}
		match stream.read(&mut buffer) {
			Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Sign-in callback closed")),
			Ok(count) => {
				line.extend_from_slice(buffer.get(..count).ok_or_else(|| io::Error::other("Invalid callback read length"))?);
				if line.len() > MAX_REQUEST_LINE {
					return Err(io::Error::new(io::ErrorKind::InvalidData, "Sign-in callback request is too long"));
				}
				if let Some(end) = line.iter().position(|byte| *byte == b'\n') {
					line.truncate(end);
					return String::from_utf8(line).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
				}
			},
			Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
			Err(error) => return Err(error),
		}
	}
}

/// Unrelated requests and old-attempt callbacks must not consume this attempt's verifier.
fn callback_code(request: &str, redirect: &Url, state: &str) -> Option<io::Result<AuthorizationCode>> {
	let mut parts = request.split_whitespace();
	if parts.next()? != "GET" {
		return None;
	}
	let target = parts.next()?;
	if !target.starts_with('/') || target.starts_with("//") {
		return None;
	}
	let url = redirect.join(target).ok()?;
	if url.path() != redirect.path() {
		return None;
	}
	let mut code = None;
	let mut matches_state = false;
	let mut denied = false;
	for (key, value) in url.query_pairs() {
		match key.as_ref() {
			"state" => matches_state = value == state,
			"code" => code = Some(value.into_owned()),
			"error" => denied = true,
			_ => {},
		}
	}
	if !matches_state {
		return None;
	}
	if denied {
		return Some(Err(io::Error::new(io::ErrorKind::PermissionDenied, "Spotify sign-in was declined. Please try again.")));
	}
	code.filter(|code| !code.is_empty()).map(|code| Ok(AuthorizationCode::new(code)))
}

fn respond(stream: &mut TcpStream, authorization: &StartAuthorization, status: &str, body: &str) -> io::Result<()> {
	let response = format!(
		"HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
		body.len()
	);
	let mut remaining = response.as_bytes();
	let until = Instant::now() + REQUEST_TIMEOUT;
	while !remaining.is_empty() {
		authorization.check()?;
		if Instant::now() >= until {
			return Err(io::Error::new(io::ErrorKind::TimedOut, "Sign-in callback response timed out"));
		}
		match stream.write(remaining) {
			Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "Sign-in callback response closed")),
			Ok(count) => remaining = remaining.get(count..).ok_or_else(|| io::Error::other("Invalid callback write length"))?,
			Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
			Err(error) => return Err(error),
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use std::{
		io::{self, Read as _, Write as _},
		net::TcpStream,
		thread,
		time::{Duration, Instant},
	};

	use oauth2::url::Url;

	use super::{Callback, callback_code};
	use crate::engine::{SignInTimedOut, StartAuthorization};

	fn redirect() -> Result<Url, Box<dyn std::error::Error>> {
		Ok(Url::parse("http://127.0.0.1:5588/login")?)
	}

	#[test]
	fn cancel_before_callback_stops_worker_and_releases_listener() -> Result<(), Box<dyn std::error::Error>> {
		let authorization = StartAuthorization::default();
		let address = authorization.listen("127.0.0.1:0".parse()?)?;
		let worker_authorization = authorization.clone();
		let redirect = redirect()?;
		let worker = thread::spawn(move || Callback(&worker_authorization).wait(&redirect, "state"));
		authorization.cancel();
		let rebound = std::net::TcpListener::bind(address)?;
		assert_eq!(
			worker
				.join()
				.map_err(|_panic| io::Error::other("callback worker panicked"))?
				.err()
				.ok_or("cancel did not stop listener")?
				.kind(),
			io::ErrorKind::Interrupted
		);
		drop(rebound);
		Ok(())
	}

	#[test]
	fn deadline_expiry_stops_listener() -> Result<(), Box<dyn std::error::Error>> {
		let authorization = StartAuthorization::default();
		let address = authorization.listen("127.0.0.1:0".parse()?)?;
		authorization.set_deadline(Some(Instant::now() + Duration::from_millis(25)));
		let error = Callback(&authorization).wait(&redirect()?, "state").err().ok_or("deadline did not stop listener")?;
		assert!(error.get_ref().is_some_and(|source| source.is::<SignInTimedOut>()));
		let rebound = std::net::TcpListener::bind(address)?;
		drop(rebound);
		Ok(())
	}

	#[test]
	fn late_callback_after_cancel_is_ignored_by_next_attempt() -> Result<(), Box<dyn std::error::Error>> {
		let old = StartAuthorization::default();
		let address = old.listen("127.0.0.1:0".parse()?)?;
		old.cancel();
		assert!(old.accept_callback().is_err());
		let new = StartAuthorization::default();
		let callback = Callback::bind(&new, address)?;
		let request = b"GET /login?state=old-state&code=late HTTP/1.1\r\n\r\n";
		let mut late = TcpStream::connect(address)?;
		late.write_all(request)?;
		let valid = thread::spawn(move || -> io::Result<()> {
			let mut response = String::new();
			late.set_read_timeout(Some(Duration::from_secs(2)))?;
			late.read_to_string(&mut response)?;
			assert!(response.starts_with("HTTP/1.1 400"));
			let mut stream = TcpStream::connect(address)?;
			stream.write_all(b"GET /login?state=new-state&code=current HTTP/1.1\r\n\r\n")?;
			stream.read_to_end(&mut Vec::new())?;
			Ok(())
		});
		new.set_deadline(Some(Instant::now() + Duration::from_secs(3)));
		assert_eq!(callback.wait(&redirect()?, "new-state")?.secret(), "current");
		valid.join().map_err(|_panic| io::Error::other("callback sender panicked"))??;
		Ok(())
	}

	#[test]
	fn callback_requires_matching_path_state_and_code() -> Result<(), Box<dyn std::error::Error>> {
		let redirect = redirect()?;
		for request in [
			"GET /favicon.ico HTTP/1.1",
			"GET /login?code=c HTTP/1.1",
			"GET /login?state=old&code=c HTTP/1.1",
			"GET //elsewhere/login?state=s&code=c HTTP/1.1",
		] {
			assert!(callback_code(request, &redirect, "s").is_none());
		}
		assert!(callback_code("GET /login?state=s&error=access_denied HTTP/1.1", &redirect, "s").ok_or("missing rejection")?.is_err());
		assert_eq!(callback_code("GET /login?state=s&code=valid HTTP/1.1", &redirect, "s").ok_or("missing code")??.secret(), "valid");
		Ok(())
	}
}
