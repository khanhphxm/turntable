//! Spotify color-lyrics reads on the player's authenticated session, without Web API calls.

use librespot_core::error::ErrorKind;
use serde::Deserialize;

use crate::{engine::SessionReader, lyrics::Lyrics, net::trace};

/// Spotify's lyrics for a track URI or base62 id. A 404 or empty lyrics means unavailable;
/// authentication, network and malformed-response errors stay errors, not missing lyrics.
pub fn lyrics(session: &SessionReader, track: &str) -> Result<Option<Lyrics>, String> {
	let id = track.strip_prefix("spotify:track:").unwrap_or(track);
	let endpoint = format!("/color-lyrics/v2/track/{id}?format=json&market=from_token");
	let mut headers = http::HeaderMap::new();
	headers.insert("app-platform", http::HeaderValue::from_static("WebPlayer"));
	headers.insert(http::header::ACCEPT, http::HeaderValue::from_static("application/json"));
	let span = trace::start();
	let result = session.block_on(session.session().spclient().request(&http::Method::GET, &endpoint, Some(headers), None));
	let (status, bytes) = match &result {
		Ok(bytes) => (200, bytes.len()),
		Err(error) if error.kind == ErrorKind::NotFound => (404, 0),
		Err(_) => (0, 0),
	};
	span.done("session", "GET", &format!("https://spclient.spotify.com{endpoint}"), trace::Reply {
		status,
		bytes,
		..trace::Reply::default()
	});
	match result {
		Ok(bytes) => parse(&bytes),
		Err(error) if error.kind == ErrorKind::NotFound => Ok(None),
		Err(error) => Err(crate::spotify::Error::from(error).user_message()),
	}
}

#[derive(Deserialize)]
struct Response {
	lyrics: Option<ColorLyrics>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ColorLyrics {
	sync_type: String,
	lines:     Vec<Line>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Line {
	start_time_ms: Option<String>,
	words:         String,
}

fn parse(bytes: &[u8]) -> Result<Option<Lyrics>, String> {
	let response: Response = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
	let Some(lyrics) = response.lyrics.filter(|lyrics| lyrics.lines.iter().any(|line| !line.words.trim().is_empty())) else {
		return Ok(None);
	};
	let lyrics = if lyrics.sync_type == "LINE_SYNCED" {
		let mut lines: Vec<_> = lyrics
			.lines
			.into_iter()
			.map(|line| {
				let time = line
					.start_time_ms
					.as_deref()
					.ok_or_else(|| "Spotify lyric line has no timestamp".to_owned())?
					.parse::<u64>()
					.map_err(|error| format!("Spotify lyric line has an invalid timestamp: {error}"))?;
				Ok((time, line.words))
			})
			.collect::<Result<_, String>>()?;
		lines.sort_by_key(|(time, _)| *time);
		Lyrics::Synced(lines)
	} else {
		let mut text = String::new();
		for (index, line) in lyrics.lines.into_iter().enumerate() {
			if index > 0 {
				text.push('\n');
			}
			text.push_str(&line.words);
		}
		Lyrics::Plain(text)
	};
	Ok(Some(lyrics))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn synced_lines_preserve_pauses_and_are_ordered() {
		let lyrics = parse(br#"{"lyrics":{"syncType":"LINE_SYNCED","lines":[{"startTimeMs":"2000","words":"two"},{"startTimeMs":"1500","words":""},{"startTimeMs":"1000","words":"one"}]}}"#).unwrap();
		assert_eq!(lyrics, Some(Lyrics::Synced(vec![(1000, "one".into()), (1500, "".into()), (2000, "two".into())])));
	}

	#[test]
	fn unsynced_lines_become_plain_text() {
		assert_eq!(
			parse(br#"{"lyrics":{"syncType":"UNSYNCED","lines":[{"words":"one"},{"words":""},{"words":"two"}]}}"#).unwrap(),
			Some(Lyrics::Plain("one\n\ntwo".into()))
		);
	}

	#[test]
	fn missing_and_empty_lyrics_are_missing_but_invalid_data_is_an_error() {
		for json in [
			r#"{}"#,
			r#"{"lyrics":null}"#,
			r#"{"lyrics":{"syncType":"UNSYNCED","lines":[]}}"#,
			r#"{"lyrics":{"syncType":"LINE_SYNCED","lines":[{"startTimeMs":"0","words":" "}]}}"#,
		] {
			assert_eq!(parse(json.as_bytes()).unwrap(), None);
		}
		assert!(parse(br#"{"lyrics":{"syncType":"LINE_SYNCED","lines":[{"startTimeMs":"bad","words":"one"}]}}"#).is_err());
		assert!(parse(br#"{"lyrics":{"syncType":"LINE_SYNCED","lines":[{"words":"one"}]}}"#).is_err());
		assert!(parse(b"not json").is_err());
	}
}
