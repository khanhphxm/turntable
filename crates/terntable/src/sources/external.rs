//! Open music data that Spotify's Web API doesn't give apps, all keyless:
//! Wikidata maps a Spotify artist id to its Wikipedia article, Wikipedia gives
//! the summary and LRCLIB gives lyrics. Every value shown is attributed to its
//! source in the UI.

use std::{thread, time::Duration};

use serde::Deserialize;

use crate::net::Profile;

const PROFILE: Profile = Profile {
	timeout:         Duration::from_secs(8),
	connect:         None,
	user_agent:      concat!("terntable/", env!("CARGO_PKG_VERSION"), " (https://github.com/stencil-hq/terntable)"),
	status_as_error: true,
};
const BODY_LIMIT: u64 = 4 << 20;
const LRCLIB: &str = "https://lrclib.net/api";
/// How far, in ms, a lyrics hit's length may be from the song's to count as the same recording.
const SAME_RECORDING_MS: u64 = 3000;
/// LRCLIB answers 503 (busy) now and then: such a request is asked again this many times, after a pause.
const BUSY_RETRIES: u32 = 2;
const BUSY_PAUSE: Duration = Duration::from_millis(600);

/// An artist as Wikidata and Wikipedia know it.
#[derive(Debug, Clone, Default)]
pub struct About {
	/// "English rock band".
	pub description: Option<String>,
	/// The Wikipedia summary, and the article's address.
	pub summary:     Option<String>,
	pub wikipedia:   Option<String>,
}

/// A song's lyrics from LRCLIB.
#[derive(Debug, Clone, PartialEq)]
pub enum Lyrics {
	/// Each line with the millisecond it is sung at, in order. A blank line is a pause.
	Synced(Vec<(u64, String)>),
	/// Text only: lines apart by `\n`, stanzas by a blank line.
	Plain(String),
}

/// Open-data lookups. They share the app's one HTTP agent (see [`crate::net`]).
#[derive(Default)]
pub struct Client;

impl Client {
	pub fn new() -> Self {
		Self
	}

	/// Looks the artist up by Spotify id (Wikidata property P1902).
	pub fn about(&self, spotify_id: &str) -> Result<About, String> {
		if !crate::app::settings::allowed::wikipedia() {
			return Err("Wikipedia is turned off in Settings".to_owned());
		}
		let id: String = spotify_id.chars().filter(char::is_ascii_alphanumeric).collect();
		let query = format!(
			"SELECT ?article ?description WHERE {{ ?item wdt:P1902 \"{id}\". \
			 OPTIONAL {{ ?article schema:about ?item; schema:isPartOf <https://en.wikipedia.org/> }} \
			 OPTIONAL {{ ?item schema:description ?description FILTER(LANG(?description) = \"en\") }} }} LIMIT 1"
		);
		let sparql: Sparql = self.get("https://query.wikidata.org/sparql", &[("query", &query), ("format", "json")])?;
		let Some(row) = sparql.results.bindings.into_iter().next() else { return Ok(About::default()) };
		let value = |v: Option<Value>| v.map(|v| v.value);
		let mut about = About { description: value(row.description), wikipedia: value(row.article), summary: None };
		if let Some(title) = about.wikipedia.as_deref().and_then(|a| a.rsplit('/').next()) {
			let page: Summary = self.get(&format!("https://en.wikipedia.org/api/rest_v1/page/summary/{title}"), &[])?;
			about.summary = page.extract.filter(|e| !e.is_empty());
		}
		Ok(about)
	}

	/// The lyrics of a song: synced when LRCLIB has them, else plain. LRCLIB finds a
	/// song by title, artist and length (give or take 2 s); one filed under another
	/// title, say with a "(feat. …)", takes a search that keeps hits of about the
	/// same length.
	pub fn lyrics(&self, title: &str, artist: &str, duration_ms: u64) -> Result<Option<Lyrics>, String> {
		if !crate::app::settings::allowed::lyrics() {
			return Err("Lyrics from LRCLIB are turned off in Settings".to_owned());
		}
		let seconds = (duration_ms + 500) / 1000;
		let secs = seconds.to_string();
		let signature = [("track_name", title), ("artist_name", artist)];
		let mut with_length = signature.to_vec();
		if seconds > 0 {
			with_length.push(("duration", &secs));
		}
		let exact = self.lrclib::<Record>("get", &with_length)?.and_then(|r| r.lyrics());
		if matches!(exact, Some(Lyrics::Synced(_))) {
			return Ok(exact);
		}
		let hits: Vec<Record> = self.lrclib("search", &signature)?.unwrap_or_default();
		Ok(match best(&hits, duration_ms) {
			Some(synced @ Lyrics::Synced(_)) => Some(synced),
			other => exact.or(other),
		})
	}

	#[expect(clippy::unused_self, reason = "the client is a handle: lookups read as methods of it")]
	fn call(&self, url: &str, query: &[(&str, &str)]) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
		let span = crate::net::trace::start();
		let mut req = PROFILE.get(url).header("Accept", "application/json");
		for (k, v) in query {
			req = req.query(k, v);
		}
		let result = req.call();
		let (status, bytes) = match &result {
			Ok(r) => (
				r.status().as_u16(),
				r.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(0),
			),
			Err(ureq::Error::StatusCode(code)) => (*code, 0),
			Err(_) => (0, 0),
		};
		span.done("open", "GET", url, crate::net::trace::Reply { status, bytes, ..crate::net::trace::Reply::default() });
		result
	}

	fn get<T: serde::de::DeserializeOwned>(&self, url: &str, query: &[(&str, &str)]) -> Result<T, String> {
		read(self.call(url, query).map_err(|e| e.to_string())?)
	}

	/// GETs an LRCLIB endpoint: `None` when it has nothing (404). A busy server (503, which it
	/// often is) is asked again after a pause.
	fn lrclib<T: serde::de::DeserializeOwned>(&self, endpoint: &str, query: &[(&str, &str)]) -> Result<Option<T>, String> {
		let url = format!("{LRCLIB}/{endpoint}");
		let mut response = self.call(&url, query);
		for _ in 0..BUSY_RETRIES {
			if !matches!(response, Err(ureq::Error::StatusCode(503))) {
				break;
			}
			thread::sleep(BUSY_PAUSE);
			response = self.call(&url, query);
		}
		match response {
			Ok(response) => read(response).map(Some),
			Err(ureq::Error::StatusCode(404)) => Ok(None),
			Err(e) => Err(e.to_string()),
		}
	}
}

fn read<T: serde::de::DeserializeOwned>(mut response: ureq::http::Response<ureq::Body>) -> Result<T, String> {
	response.body_mut().with_config().limit(BODY_LIMIT).read_json().map_err(|e| e.to_string())
}

#[derive(Deserialize)]
struct Sparql {
	results: Bindings,
}

#[derive(Deserialize)]
struct Bindings {
	bindings: Vec<Row>,
}

#[derive(Deserialize)]
struct Row {
	article:     Option<Value>,
	description: Option<Value>,
}

#[derive(Deserialize)]
struct Value {
	value: String,
}

#[derive(Deserialize)]
struct Summary {
	extract: Option<String>,
}

/// A song in LRCLIB's database.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
	/// Seconds.
	duration:      Option<f64>,
	#[serde(default)]
	instrumental:  bool,
	plain_lyrics:  Option<String>,
	synced_lyrics: Option<String>,
}

impl Record {
	/// Its synced lyrics if it has any, else its plain ones.
	fn lyrics(&self) -> Option<Lyrics> {
		if self.instrumental {
			return None;
		}
		let synced = self.synced_lyrics.as_deref().map(parse_lrc).filter(|lines| !lines.is_empty());
		if let Some(lines) = synced {
			return Some(Lyrics::Synced(lines));
		}
		let plain = self.plain_lyrics.as_deref()?.trim();
		(!plain.is_empty()).then(|| Lyrics::Plain(plain.to_owned()))
	}
}

/// The lyrics to take from search `hits` for a recording `duration_ms` long: from a hit of about
/// that length, synced before plain, then the closest in length.
fn best(hits: &[Record], duration_ms: u64) -> Option<Lyrics> {
	hits.iter()
		.filter_map(|hit| {
			let gap = match duration_ms {
				0 => 0,
				d => {
					let gap = Duration::try_from_secs_f64((hit.duration? - d as f64 / 1000.0).abs()).ok()?;
					u64::try_from(gap.as_millis()).ok()?
				},
			};
			if gap > SAME_RECORDING_MS {
				return None;
			}
			let lyrics = hit.lyrics()?;
			Some((matches!(lyrics, Lyrics::Plain(_)), gap, lyrics))
		})
		.min_by_key(|(plain, gap, _)| (*plain, *gap))
		.map(|(.., lyrics)| lyrics)
}

/// Lines of synced lyrics in LRC format, by time: `[mm:ss.xx] text`, a line sung again as
/// `[mm:ss.xx][mm:ss.xx] text`. Other tags (`[ar:…]`) are skipped. A stamp without text is a
/// pause: kept once in a row and never first.
fn parse_lrc(lrc: &str) -> Vec<(u64, String)> {
	let mut lines = Vec::new();
	for line in lrc.lines() {
		let mut rest = line.trim_start();
		let mut times = Vec::new();
		while let Some((tag, after)) = rest.strip_prefix('[').and_then(|t| t.split_once(']')) {
			let Some(ms) = stamp(tag) else { break };
			times.push(ms);
			rest = after;
		}
		lines.extend(times.into_iter().map(|ms| (ms, rest.trim().to_owned())));
	}
	lines.sort_by_key(|(ms, _)| *ms);
	lines.dedup_by(|later, earlier| later.1.is_empty() && earlier.1.is_empty());
	if lines.first().is_some_and(|(_, text)| text.is_empty()) {
		lines.remove(0);
	}
	lines
}

/// `mm:ss` with an optional fraction of a second (`.5` is 500 ms, `.57` 570, `.574` 574) in ms.
fn stamp(tag: &str) -> Option<u64> {
	let (minutes, rest) = tag.split_once(':')?;
	let (seconds, fraction) = rest.split_once(['.', ':']).unwrap_or((rest, ""));
	let whole = u64::from(minutes.parse::<u32>().ok()?) * 60 + u64::from(seconds.parse::<u32>().ok()?);
	let mut ms = 0;
	for (digit, unit) in fraction.chars().zip([100, 10, 1]) {
		ms += u64::from(digit.to_digit(10)?) * unit;
	}
	Some(whole * 1000 + ms)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn record(json: &str) -> Record {
		serde_json::from_str(json).unwrap()
	}

	fn synced(lines: &[(u64, &str)]) -> Lyrics {
		Lyrics::Synced(lines.iter().map(|(ms, text)| (*ms, (*text).to_owned())).collect())
	}

	#[test]
	fn lrc_stamps_read_as_milliseconds() {
		assert_eq!(stamp("00:31.55"), Some(31_550));
		assert_eq!(stamp("01:06.280"), Some(66_280));
		assert_eq!(stamp("00:35.8"), Some(35_800));
		assert_eq!(stamp("02:03"), Some(123_000));
		assert_eq!(stamp("02:03:07"), Some(123_070), "some files separate the fraction with a colon");
		assert_eq!(stamp("ar:Daft Punk"), None);
		assert_eq!(stamp("length: 03:20"), None);
		assert_eq!(stamp("offset:+500"), None);
		assert_eq!(stamp("00:1x"), None);
	}

	#[test]
	fn lrc_lines_are_sorted_trimmed_and_tags_skipped() {
		let lrc = "[ar:Daft Punk]\r\n[ti:Get Lucky]\r\n[00:00.00] \r\n[00:31.55] Like the legend of the phoenix\r\n\
		           [01:12.60][02:30.00] We're up all night\r\n[00:35.8]All ends with beginnings\r\n[01:06.28]  [Chorus] \r\nnot lrc\r\n";
		assert_eq!(
			parse_lrc(lrc),
			[
				(31_550, "Like the legend of the phoenix"),
				(35_800, "All ends with beginnings"),
				(66_280, "[Chorus]"),
				(72_600, "We're up all night"),
				(150_000, "We're up all night"),
			]
			.map(|(ms, text)| (ms, text.to_owned()))
		);
	}

	#[test]
	fn a_pause_is_kept_once_and_never_leads() {
		let lrc = "[00:00.00]\n[00:01.00]\n[00:05.00] One\n[00:09.00]\n[00:09.50] \n[00:12.00] Two\n[00:20.00]\n";
		assert_eq!(parse_lrc(lrc), [(5000, "One"), (9000, ""), (12_000, "Two"), (20_000, "")].map(|(ms, text)| (ms, text.to_owned())));
		assert!(parse_lrc("[ar:Nobody]\nplain words").is_empty());
	}

	#[test]
	fn a_record_gives_synced_lyrics_before_plain_and_none_for_an_instrumental() {
		let both = record(
			r#"{"id":1,"duration":367.0,"instrumental":false,"plainLyrics":"a\nb","syncedLyrics":"[00:01.00] a\n[00:02.00] b","lyricsfile":"x"}"#,
		);
		assert_eq!(both.lyrics(), Some(synced(&[(1000, "a"), (2000, "b")])));
		let plain = record(r#"{"plainLyrics":"\n a\nb \n","syncedLyrics":null}"#);
		assert_eq!(plain.lyrics(), Some(Lyrics::Plain("a\nb".into())));
		let unsynced = record(r#"{"plainLyrics":"a","syncedLyrics":"no stamps here"}"#);
		assert_eq!(unsynced.lyrics(), Some(Lyrics::Plain("a".into())), "unusable LRC falls back to the text");
		assert_eq!(record(r#"{"instrumental":true,"plainLyrics":null,"syncedLyrics":null}"#).lyrics(), None);
		assert_eq!(record(r#"{"plainLyrics":"","syncedLyrics":""}"#).lyrics(), None);
	}

	#[test]
	fn the_best_hit_is_synced_first_then_closest_in_length() {
		let hit = |duration: Option<f64>, lrc: bool| {
			let duration = duration.map_or("null".to_owned(), |d| d.to_string());
			let synced = if lrc { r#""[00:01.00] x""# } else { "null" };
			record(&format!(r#"{{"duration":{duration},"plainLyrics":"x","syncedLyrics":{synced}}}"#))
		};
		let hits = [hit(Some(100.0), false), hit(Some(108.0), true), hit(Some(101.5), true), hit(Some(99.0), true), hit(None, true)];
		assert_eq!(best(&hits, 100_000), Some(synced(&[(1000, "x")])));
		// A synced hit just inside the tolerance still beats a plain one on the dot.
		assert!(matches!(best(&[hit(Some(100.0), false), hit(Some(102.9), true)], 100_000), Some(Lyrics::Synced(_))));
		// Without synced ones, plain text from the closest recording.
		assert_eq!(best(&[hit(Some(100.0), false)], 99_500), Some(Lyrics::Plain("x".into())));
		// Other lengths are other recordings; an unknown length takes any.
		assert_eq!(best(&[hit(Some(108.0), true), hit(None, true)], 100_000), None);
		assert!(best(&[hit(None, true)], 0).is_some());
	}

	#[test]
	fn lyrics_and_artist_info_are_not_asked_for_while_they_are_off() {
		use crate::app::settings::{Sources, allowed};
		let off = Sources { lyrics: false, wikipedia: false, web_player: true };
		allowed::only_with(off, || {
			// Both answer before any request is made (a request would take seconds, or fail offline).
			let client = Client::new();
			assert!(client.lyrics("Song", "Artist", 200_000).unwrap_err().contains("turned off"));
			assert!(client.about("4Z8W4fKeB5YxbusRsdQVPb").unwrap_err().contains("turned off"));
		});
	}
}
