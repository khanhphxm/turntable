//! Request tracing, for measuring: `TERNTABLE_TRACE=<file>` appends one JSON object per line
//! (`TERNTABLE_TRACE=1`: `$TMPDIR/terntable-trace.jsonl`). Off by default, and then every call
//! below is one atomic load.
//!
//! - `k:"http"`: an outgoing request. `c` is the client (`api` Web API, `web` the web player,
//!   `open` open data, `img` artwork), `ep` the endpoint with ids masked, `st` the status (0:
//!   the transport failed), `ms` how long it took, `b` the body's bytes, `via` how it was
//!   served (`net`; `joined` an identical request in flight answered it; `skip` refused
//!   without the network because Spotify asked us to wait), `etag` whether the answer carried
//!   an ETag.
//! - `k:"hit"`: something answered without a request (`via` `mem` or `disk`).
//! - `k:"mark"`: what the UI loop saw (a request sent, a page drawn, the covers done).
//!
//! `t` is milliseconds since the process started and `th` the thread.

use std::{
	collections::HashMap,
	fs::{File, OpenOptions},
	io::Write,
	path::PathBuf,
	sync::{LazyLock, Mutex},
	time::{Instant, SystemTime, UNIX_EPOCH},
};

struct Sink {
	start: Instant,
	file:  Mutex<File>,
	/// The detail each `mark_if_changed` name last logged.
	last:  Mutex<HashMap<String, String>>,
}

static SINK: LazyLock<Option<Sink>> = LazyLock::new(|| {
	let value = std::env::var_os("TERNTABLE_TRACE").filter(|v| !v.is_empty())?;
	let path = if value == "1" { std::env::temp_dir().join("terntable-trace.jsonl") } else { PathBuf::from(value) };
	let file = OpenOptions::new().create(true).append(true).open(path).ok()?;
	let sink = Sink { start: Instant::now(), file: Mutex::new(file), last: Mutex::default() };
	let epoch_ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
	sink.line(&format!(r#""k":"start","epoch_ms":{epoch_ms},"pid":{}"#, std::process::id()));
	Some(sink)
});

/// Starts the clock (`t` counts from here) when tracing is on. Call first thing in `main`.
pub fn init() {
	LazyLock::force(&SINK);
}

pub fn enabled() -> bool {
	SINK.is_some()
}

impl Sink {
	fn line(&self, fields: &str) {
		let t = self.start.elapsed().as_secs_f64() * 1000.0;
		let thread = std::thread::current();
		let line = format!("{{\"t\":{t:.1},\"th\":{},{fields}}}\n", json(thread.name().unwrap_or("?")));
		if let Ok(mut file) = self.file.lock() {
			let _ = file.write_all(line.as_bytes());
		}
	}
}

fn json(s: &str) -> String {
	serde_json::to_string(s).unwrap_or_else(|_| "\"?\"".into())
}

/// How a request was answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
	Net,
	/// An identical request already in flight answered it.
	Joined,
	/// Refused without the network: Spotify asked us to wait.
	#[expect(dead_code, reason = "for a request refused without the network; the gate's refusals are not traced yet")]
	Skip,
}

impl Via {
	const fn name(self) -> &'static str {
		match self {
			Self::Net => "net",
			Self::Joined => "joined",
			Self::Skip => "skip",
		}
	}
}

/// What came back: the status (0: the transport failed), the body's bytes, whether it carried an
/// ETag, and how it was served.
#[derive(Clone, Copy, Debug)]
pub struct Reply {
	pub status: u16,
	pub bytes:  usize,
	pub etag:   bool,
	pub via:    Via,
}

impl Default for Reply {
	fn default() -> Self {
		Self { status: 0, bytes: 0, etag: false, via: Via::Net }
	}
}

/// A request's clock; inert (and free) while tracing is off.
pub struct Span(Option<Instant>);

pub fn start() -> Span {
	Span(enabled().then(Instant::now))
}

impl Span {
	/// Logs the request: `url` is masked into an endpoint (see [`endpoint`]).
	pub fn done(self, client: &str, method: &str, url: &str, reply: Reply) {
		let (Some(at), Some(sink)) = (self.0, SINK.as_ref()) else { return };
		let ms = at.elapsed().as_secs_f64() * 1000.0;
		let Reply { status, bytes, etag, via } = reply;
		sink.line(&format!(
			r#""k":"http","c":{},"m":{},"ep":{},"st":{status},"ms":{ms:.1},"b":{bytes},"etag":{etag},"via":"{}""#,
			json(client),
			json(method),
			json(&endpoint(url)),
			via.name()
		));
	}
}

/// Something answered from a cache (`what`: the kind of entity, `key` which one, `via` `mem` or `disk`).
pub fn hit(what: &str, key: &str, via: &str) {
	if let Some(sink) = SINK.as_ref() {
		sink.line(&format!(r#""k":"hit","n":{},"d":{},"via":{}"#, json(what), json(key), json(via)));
	}
}

/// A moment the UI loop saw: `req`, `ready`, `covers` ...
pub fn mark(name: &str, detail: &str) {
	if let Some(sink) = SINK.as_ref() {
		sink.line(&format!(r#""k":"mark","n":{},"d":{}"#, json(name), json(detail)));
	}
}

/// [`mark`], unless the last `name` mark said the same (the loop reports its state after every render).
pub fn mark_if_changed(name: &str, detail: &str) {
	let Some(sink) = SINK.as_ref() else { return };
	{
		let mut last = sink.last.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
		if last.get(name).is_some_and(|d| d == detail) {
			return;
		}
		last.insert(name.to_owned(), detail.to_owned());
	}
	mark(name, detail);
}

/// `host/path` of `url` with the ids masked (`api.spotify.com/albums/:id/tracks`); a local mock
/// server is `mock`. Queries are dropped: they carry search text and ids.
pub fn endpoint(url: &str) -> String {
	let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
	let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
	let path = path.split(['?', '#']).next().unwrap_or_default();
	let host = if host.starts_with("127.0.0.1") || host.starts_with("localhost") { "mock" } else { host };
	let path = if host == "mock" || host == "api.spotify.com" { path.strip_prefix("v1/").unwrap_or(path) } else { path };
	let masked: Vec<&str> = path.split('/').map(|seg| if is_id(seg) { ":id" } else { seg }).collect();
	format!("{host}/{}", masked.join("/"))
}

/// Spotify ids (22 base-62 characters, nearly always with a digit) and image ids (hex, 40 characters).
/// Words like `queryArtistOverview` are not.
fn is_id(segment: &str) -> bool {
	segment.len() >= 22 && segment.bytes().all(|b| b.is_ascii_alphanumeric()) && segment.bytes().any(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn endpoints_lose_ids_and_queries() {
		assert_eq!(
			endpoint("https://api.spotify.com/v1/albums/4aawyAB9vmqN3uQ7FjRGTy/tracks?offset=0&limit=50"),
			"api.spotify.com/albums/:id/tracks"
		);
		assert_eq!(endpoint("http://127.0.0.1:5000/v1/me/playlists?limit=50"), "mock/me/playlists");
		assert_eq!(
			endpoint("https://api-partner.spotify.com/pathfinder/v1/query/fetchExtractedColors"),
			"api-partner.spotify.com/pathfinder/v1/query/fetchExtractedColors"
		);
		assert_eq!(endpoint("https://i.scdn.co/image/ab67616d00001e02ec548c00d3ac2f10be73366d"), "i.scdn.co/image/:id");
		assert_eq!(
			endpoint("https://api-partner.spotify.com/pathfinder/v1/query?operationName=x"),
			"api-partner.spotify.com/pathfinder/v1/query"
		);
		assert_eq!(endpoint("https://open.spotify.com"), "open.spotify.com/");
	}
}
