//! Thin blocking HTTP layer on the app's shared agent: status classification, one `Retry-After`
//! retry for GETs, and the [`Gate`] every Web API request passes.

use std::{
	cell::Cell,
	sync::{Condvar, Mutex, MutexGuard, PoisonError},
	time::{Duration, Instant},
};

use serde::Deserialize;

use super::error::{Error, Result};
use crate::net::{Profile, trace};

/// Longest `Retry-After` we are willing to block for. Longer waits are surfaced to the caller.
const MAX_RETRY_WAIT: Duration = Duration::from_secs(10);
/// Used when a 429 carries no usable `Retry-After`.
const DEFAULT_RETRY_WAIT: Duration = Duration::from_secs(1);
/// How long a spent quota is assumed to last when Spotify doesn't say.
const QUOTA_WAIT: Duration = Duration::from_secs(300);
/// Requests in flight at once. Development Mode answers bursts with 429, which stalls every
/// page; a few at a time is as fast as a page needs and keeps clear of that.
const MAX_IN_FLIGHT: usize = 4;
/// Speculative requests leave the rest of the slots to what the user waits for.
const IDLE_IN_FLIGHT: usize = 2;

const PROFILE: Profile = Profile {
	timeout:         Duration::from_secs(20),
	connect:         None,
	user_agent:      concat!("turntable/", env!("CARGO_PKG_VERSION")),
	status_as_error: false,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Method {
	Get,
	Put,
	Delete,
	Post,
}

impl Method {
	const fn name(self) -> &'static str {
		match self {
			Self::Get => "GET",
			Self::Put => "PUT",
			Self::Delete => "DELETE",
			Self::Post => "POST",
		}
	}
}

pub struct Req<'a> {
	pub method: Method,
	pub url:    &'a str,
	pub query:  &'a [(&'a str, &'a str)],
	pub bearer: Option<&'a str>,
	/// JSON request body (PUT/POST only).
	pub json:   Option<&'a str>,
	/// Maximum response body size in bytes.
	pub limit:  u64,
}

/// What a 2xx (or, for a conditional request, 304) answer carried.
pub struct Fetched {
	pub body:         Vec<u8>,
	/// The `ETag` the answer came with, if Spotify sends them for this endpoint.
	pub etag:         Option<String>,
	/// The server said the copy the request named by `If-None-Match` is current (the body is empty).
	pub not_modified: bool,
}

/// How urgent the requests of the current thread are. Each worker thread sets it once.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Priority {
	/// Playback control: never held back by library loads, and still sent while a spent quota
	/// refuses the rest (it may be a different budget; a refused command says so itself).
	Player,
	#[default]
	Normal,
	/// Speculative loads (prefetching): few at a time, so a page the user asks for never queues behind them.
	Idle,
}

thread_local! {
	static PRIORITY: Cell<Priority> = const { Cell::new(Priority::Normal) };
}

pub fn set_priority(priority: Priority) {
	PRIORITY.with(|p| p.set(priority));
}

pub fn priority() -> Priority {
	PRIORITY.with(Cell::get)
}

#[derive(Default)]
struct GateState {
	in_flight:   usize,
	/// After a 429: nothing is sent until then.
	pause_until: Option<Instant>,
	/// After a 429 for a spent quota: nothing but playback control is sent until then.
	quota_until: Option<Instant>,
}

/// The one door to the Web API: it limits the requests in flight, holds everything back after a
/// 429 until its `Retry-After` has passed (the old way, a pause in the one thread that met the 429,
/// no longer holds with several in flight) and refuses without the network while the quota is spent.
pub struct Gate {
	state:   Mutex<GateState>,
	changed: Condvar,
	normal:  usize,
	idle:    usize,
}

impl Default for Gate {
	fn default() -> Self {
		Self::new(MAX_IN_FLIGHT, IDLE_IN_FLIGHT)
	}
}

/// A slot in flight; the gate gets it back on drop.
pub struct Permit<'a> {
	gate: &'a Gate,
}

impl Drop for Permit<'_> {
	fn drop(&mut self) {
		self.gate.lock().in_flight -= 1;
		self.gate.changed.notify_all();
	}
}

impl Gate {
	pub fn new(normal: usize, idle: usize) -> Self {
		Self { state: Mutex::new(GateState::default()), changed: Condvar::new(), normal, idle }
	}

	fn lock(&self) -> MutexGuard<'_, GateState> {
		self.state.lock().unwrap_or_else(PoisonError::into_inner)
	}

	/// Waits for a slot. `Err` when the request must not be sent: the quota is spent, or Spotify
	/// asked for a pause longer than [`MAX_RETRY_WAIT`].
	pub fn enter(&self, priority: Priority) -> Result<Permit<'_>> {
		let mut state = self.lock();
		loop {
			let now = Instant::now();
			if priority != Priority::Player
				&& let Some(until) = state.quota_until
			{
				if until > now {
					return Err(Error::QuotaExceeded { retry_after: Some(until - now) });
				}
				state.quota_until = None;
			}
			if let Some(until) = state.pause_until {
				let left = until.saturating_duration_since(now);
				if left > MAX_RETRY_WAIT {
					return Err(Error::RateLimited { retry_after: Some(left) });
				}
				if !left.is_zero() {
					state = self.changed.wait_timeout(state, left).unwrap_or_else(PoisonError::into_inner).0;
					continue;
				}
				state.pause_until = None;
			}
			let cap = match priority {
				Priority::Player => usize::MAX,
				Priority::Normal => self.normal,
				Priority::Idle => self.idle,
			};
			if state.in_flight < cap {
				state.in_flight += 1;
				return Ok(Permit { gate: self });
			}
			state = self.changed.wait(state).unwrap_or_else(PoisonError::into_inner);
		}
	}

	/// Nothing is sent for `wait` (a 429 with `Retry-After`).
	pub fn pause(&self, wait: Duration) {
		let until = Instant::now() + wait;
		let mut state = self.lock();
		if state.pause_until.is_none_or(|current| current < until) {
			state.pause_until = Some(until);
		}
		self.changed.notify_all();
	}

	/// The quota is spent: reads fail fast for `retry_after` (or [`QUOTA_WAIT`]).
	pub fn spend_quota(&self, retry_after: Option<Duration>) {
		self.lock().quota_until = Some(Instant::now() + retry_after.unwrap_or(QUOTA_WAIT));
	}
}

#[derive(Default)]
pub struct Http {
	gate: Gate,
}

impl Http {
	pub fn new() -> Self {
		Self::default()
	}

	/// Sends a conditional request. A plain 429 GET is retried once within `MAX_RETRY_WAIT`;
	/// `not_modified` means the caller's cached `etag` remains current.
	pub fn send_conditional(&self, req: &Req<'_>, etag: Option<&str>) -> Result<Fetched> {
		let mut retried = false;
		loop {
			let permit = self.gate.enter(priority())?;
			let result = Self::once(req, etag);
			drop(permit);
			match result {
				Err(Error::RateLimited { retry_after }) => {
					let wait = retry_after.unwrap_or(DEFAULT_RETRY_WAIT);
					// Every request waits, not only this one.
					self.gate.pause(wait);
					if retried || req.method != Method::Get || wait > MAX_RETRY_WAIT {
						return Err(Error::RateLimited { retry_after });
					}
					retried = true;
				},
				Err(Error::QuotaExceeded { retry_after }) => {
					self.gate.spend_quota(retry_after);
					return Err(Error::QuotaExceeded { retry_after });
				},
				other => return other,
			}
		}
	}

	/// One exchange, traced (see [`crate::net::trace`]).
	fn once(req: &Req<'_>, etag: Option<&str>) -> Result<Fetched> {
		let span = trace::start();
		let result = Self::exchange(req, etag);
		let reply = match &result {
			Ok(fetched) => trace::Reply {
				status: if fetched.not_modified { 304 } else { 200 },
				bytes: fetched.body.len(),
				etag: fetched.etag.is_some(),
				..trace::Reply::default()
			},
			Err(Error::Http { status, .. }) => trace::Reply { status: *status, ..trace::Reply::default() },
			Err(Error::RateLimited { .. } | Error::QuotaExceeded { .. }) => trace::Reply { status: 429, ..trace::Reply::default() },
			Err(Error::AuthExpired) => trace::Reply { status: 401, ..trace::Reply::default() },
			Err(_) => trace::Reply::default(),
		};
		span.done("api", req.method.name(), req.url, reply);
		result
	}

	fn exchange(req: &Req<'_>, etag: Option<&str>) -> Result<Fetched> {
		let transport = |e: ureq::Error| Error::Network(e.to_string());
		let agent = crate::net::agent();
		let mut resp = match req.method {
			Method::Get | Method::Delete => {
				let mut b = PROFILE.apply(if req.method == Method::Get { agent.get(req.url) } else { agent.delete(req.url) });
				for (k, v) in req.query {
					b = b.query(k, v);
				}
				if let Some(token) = req.bearer {
					b = b.header("Authorization", format!("Bearer {token}"));
				}
				if let Some(etag) = etag {
					b = b.header("If-None-Match", etag);
				}
				match (req.method, req.json) {
					// A DELETE with a body (playlist items to remove).
					(Method::Delete, Some(json)) => b.force_send_body().header("Content-Type", "application/json").send(json),
					_ => b.call(),
				}
				.map_err(transport)?
			},
			Method::Put | Method::Post => {
				let mut b = PROFILE.apply(if req.method == Method::Put { agent.put(req.url) } else { agent.post(req.url) });
				for (k, v) in req.query {
					b = b.query(k, v);
				}
				if let Some(token) = req.bearer {
					b = b.header("Authorization", format!("Bearer {token}"));
				}
				match req.json {
					Some(json) => b.header("Content-Type", "application/json").send(json),
					None => b.send_empty(),
				}
				.map_err(transport)?
			},
		};
		let status = resp.status().as_u16();
		let header = |name: &str| resp.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_owned);
		let (retry_after, etag) = (header("retry-after"), header("etag"));
		let body = resp.body_mut().with_config().limit(req.limit).read_to_vec().map_err(|e| Error::Network(e.to_string()))?;
		if (200..300).contains(&status) {
			Ok(Fetched { body, etag, not_modified: false })
		} else if status == 304 {
			Ok(Fetched { body: Vec::new(), etag, not_modified: true })
		} else {
			Err(classify(status, retry_after.as_deref(), &body))
		}
	}
}
/// Decodes `%XX` escapes and `+` as space (query strings). Invalid escapes pass through.
pub fn urldecode(s: &str) -> String {
	let mut bytes = s.bytes();
	let mut out = Vec::with_capacity(s.len());
	while let Some(b) = bytes.next() {
		match b {
			b'+' => out.push(b' '),
			b'%' => {
				// Look two bytes ahead without consuming them: an invalid escape passes through as it is.
				let mut ahead = bytes.clone();
				if let (Some(hi), Some(lo)) = (ahead.next().and_then(hex), ahead.next().and_then(hex)) {
					out.push(hi << 4 | lo);
					bytes = ahead;
				} else {
					out.push(b'%');
				}
			},
			b => out.push(b),
		}
	}
	String::from_utf8_lossy(&out).into_owned()
}

fn hex(b: u8) -> Option<u8> {
	char::from(b).to_digit(16).and_then(|d| u8::try_from(d).ok())
}

#[derive(Deserialize, Default)]
struct ErrorBody {
	/// Web API: `{"error": {"status", "message", "reason"}}`; OAuth: `{"error": "invalid_grant"}`.
	error:             Option<ErrorField>,
	error_description: Option<String>,
	message:           Option<String>,
	reason:            Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ErrorField {
	Code(String),
	Detail { message: Option<String>, reason: Option<String> },
}

/// Maps a non-2xx response onto an [`Error`].
pub fn classify(status: u16, retry_after: Option<&str>, body: &[u8]) -> Error {
	let parsed: ErrorBody = serde_json::from_slice(body).unwrap_or_default();
	let (code, detail_message, detail_reason) = match parsed.error {
		Some(ErrorField::Code(c)) => (Some(c), None, None),
		Some(ErrorField::Detail { message, reason }) => (None, message, reason),
		None => (None, None, None),
	};
	let retry_after = retry_after.and_then(|v| v.trim().parse::<u64>().ok()).map(Duration::from_secs);
	let device_missing = status == 404
		&& (detail_reason.as_deref() == Some("NO_ACTIVE_DEVICE")
			|| detail_message.as_deref().is_some_and(|m| m.to_ascii_lowercase().contains("device not found")));
	match status {
		404 if device_missing => Error::DeviceNotFound,
		403 if detail_reason.as_deref() == Some("PREMIUM_REQUIRED") => Error::PremiumRequired,
		401 => Error::AuthExpired,
		400 if code.as_deref() == Some("invalid_grant") => Error::AuthExpired,
		429 => {
			let reason = parsed.reason.as_deref().or(detail_reason.as_deref());
			if reason == Some("QUOTA_EXCEEDED") { Error::QuotaExceeded { retry_after } } else { Error::RateLimited { retry_after } }
		},
		_ => {
			let message = detail_message
				.or(parsed.error_description)
				.or(parsed.message)
				.or(code)
				.unwrap_or_else(|| String::from_utf8_lossy(body).into_owned());
			Error::Http { status, message: summarize(&message) }
		},
	}
}

/// Single-line, at most 200 characters.
fn summarize(text: &str) -> String {
	let text = text.trim();
	let mut out: String = text.chars().take(200).map(|c| if c.is_control() { ' ' } else { c }).collect();
	if text.chars().count() > 200 {
		out.push('…');
	}
	out
}

/// Percent-encodes everything but RFC 3986 unreserved characters (query values).
pub fn urlencode(s: &str) -> String {
	use std::fmt::Write as _;
	let mut out = String::with_capacity(s.len());
	for &b in s.as_bytes() {
		if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
			out.push(char::from(b));
		} else {
			// Writing to a String cannot fail.
			let _ = write!(out, "%{b:02X}");
		}
	}
	out
}

#[cfg(test)]
mod tests {
	use std::{
		sync::{Arc, mpsc},
		thread,
	};

	use super::*;

	#[test]
	fn quota_exceeded_is_distinguished_from_rate_limit() {
		let quota = br#"{"reason":"QUOTA_EXCEEDED","message":"API quota exceeded for the developer account"}"#;
		assert!(matches!(
			classify(429, Some("3600"), quota),
			Error::QuotaExceeded { retry_after: Some(d) } if d == Duration::from_secs(3600)
		));
		let nested = br#"{"error":{"status":429,"message":"quota","reason":"QUOTA_EXCEEDED"}}"#;
		assert!(matches!(classify(429, None, nested), Error::QuotaExceeded { retry_after: None }));

		let plain = b"API rate limit exceeded";
		assert!(matches!(
			classify(429, Some("7"), plain),
			Error::RateLimited { retry_after: Some(d) } if d == Duration::from_secs(7)
		));
		assert!(matches!(classify(429, Some("soon"), b"{}"), Error::RateLimited { retry_after: None }));
	}

	#[test]
	fn missing_player_device_maps_to_device_not_found() {
		let none = br#"{"error":{"status":404,"message":"Player command failed: No active device found","reason":"NO_ACTIVE_DEVICE"}}"#;
		assert!(matches!(classify(404, None, none), Error::DeviceNotFound));
		let unknown = br#"{"error":{"status":404,"message":"Device not found"}}"#;
		assert!(matches!(classify(404, None, unknown), Error::DeviceNotFound));
		let other = br#"{"error":{"status":404,"message":"Not found."}}"#;
		assert!(matches!(classify(404, None, other), Error::Http { status: 404, .. }));
	}

	#[test]
	fn player_commands_on_a_free_account_map_to_premium_required() {
		let free = br#"{"error":{"status":403,"message":"Player command failed: Premium required","reason":"PREMIUM_REQUIRED"}}"#;
		assert!(matches!(classify(403, None, free), Error::PremiumRequired));
		// A 403 of another kind stays a plain HTTP error.
		let other = br#"{"error":{"status":403,"message":"Insufficient client scope"}}"#;
		assert!(matches!(classify(403, None, other), Error::Http { status: 403, .. }));
	}

	#[test]
	fn auth_and_http_errors() {
		assert!(matches!(classify(401, None, br#"{"error":{"status":401,"message":"The access token expired"}}"#), Error::AuthExpired));
		assert!(matches!(
			classify(400, None, br#"{"error":"invalid_grant","error_description":"Refresh token revoked"}"#),
			Error::AuthExpired
		));
		let Error::Http { status, message } = classify(403, None, br#"{"error":{"status":403,"message":"Forbidden"}}"#) else {
			panic!("expected Http");
		};
		assert_eq!((status, message.as_str()), (403, "Forbidden"));
		let Error::Http { message, .. } = classify(502, None, &vec![b'x'; 5000]) else { panic!() };
		assert_eq!(message.chars().count(), 201);
		let Error::Http { message, .. } = classify(400, None, br#"{"error":"invalid_client","error_description":"Invalid client"}"#) else {
			panic!()
		};
		assert_eq!(message, "Invalid client");
	}

	/// Runs `enter` on another thread once the caller is ready; the receiver hears when it got in.
	fn enter_later(gate: &Arc<Gate>, priority: Priority) -> (mpsc::Receiver<()>, thread::JoinHandle<()>) {
		let (started, entered) = (mpsc::channel(), mpsc::channel());
		let gate = gate.clone();
		let handle = thread::spawn(move || {
			started.0.send(()).unwrap();
			drop(gate.enter(priority).unwrap());
			entered.0.send(()).unwrap();
		});
		started.1.recv().unwrap();
		(entered.1, handle)
	}

	#[test]
	fn a_third_request_waits_for_a_slot_and_playback_never_does() {
		let gate = Arc::new(Gate::new(2, 1));
		let (first, _second) = (gate.enter(Priority::Normal).unwrap(), gate.enter(Priority::Normal).unwrap());
		let (entered, waiting) = enter_later(&gate, Priority::Normal);
		assert!(entered.recv_timeout(Duration::from_millis(100)).is_err(), "no slot is free yet");
		assert!(gate.enter(Priority::Player).is_ok(), "playback is not held back");
		drop(first);
		entered.recv().expect("it gets in once a slot comes free");
		waiting.join().unwrap();
	}

	#[test]
	fn speculative_requests_leave_slots_to_the_ones_the_user_waits_for() {
		let gate = Arc::new(Gate::new(3, 1));
		let idle = gate.enter(Priority::Idle).unwrap();
		let (entered, second) = enter_later(&gate, Priority::Idle);
		assert!(entered.recv_timeout(Duration::from_millis(100)).is_err(), "a second speculative one waits for the first");
		assert!(gate.enter(Priority::Normal).is_ok(), "a normal request still finds a slot");
		drop(idle);
		entered.recv().expect("it gets in once the first is done");
		second.join().unwrap();
	}

	#[test]
	fn a_pause_holds_everything_back_and_a_long_one_fails_fast() {
		let gate = Gate::new(4, 2);
		gate.pause(Duration::from_millis(200));
		let started = Instant::now();
		drop(gate.enter(Priority::Normal).unwrap());
		assert!(started.elapsed() >= Duration::from_millis(180), "it waited out the pause");
		gate.pause(Duration::from_secs(60));
		assert!(matches!(gate.enter(Priority::Normal), Err(Error::RateLimited { .. })), "a pause of a minute is reported, not slept");
	}

	#[test]
	fn a_spent_quota_refuses_reads_without_the_network_but_not_playback() {
		let gate = Gate::new(4, 2);
		gate.spend_quota(Some(Duration::from_secs(60)));
		assert!(matches!(gate.enter(Priority::Normal), Err(Error::QuotaExceeded { retry_after: Some(d) }) if d <= Duration::from_secs(60)));
		assert!(gate.enter(Priority::Idle).is_err());
		assert!(gate.enter(Priority::Player).is_ok());
	}
}
