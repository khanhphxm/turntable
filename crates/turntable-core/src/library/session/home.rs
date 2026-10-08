//! Home's recent contexts from the authenticated player's native history service.
//! Each context includes its latest played track; batch metadata resolves those songs.
//! This is Spotify Home history, not the Web API's per-play event log.

use serde::Deserialize;

use super::{Reader, tracks};
use crate::{
	net::trace,
	spotify::{self, PlayContext, Track},
};

#[derive(Deserialize)]
struct History {
	#[serde(rename = "playContexts")]
	contexts: Vec<Recent>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Recent {
	uri:                   String,
	last_played_time:      u64,
	last_played_track_uri: Option<String>,
}

impl Reader {
	/// Latest song per recent context, newest first, without touching the Web API.
	pub(in crate::library) fn recently_played(&self, limit: u32) -> spotify::Result<Vec<(Track, Option<PlayContext>)>> {
		self.shared
			.block_on(async {
				let endpoint = format!(
					"/recently-played/v3/user/{}/recently-played?format=json&offset=0&limit={}&filter=default,collection-new-episodes",
					spotify::urlencode(&self.shared.session().username()),
					limit.clamp(1, 50)
				);
				let span = trace::start();
				let reply = self.shared.session().spclient().request(&http::Method::GET, &endpoint, None, None).await;
				span.done("session", "GET", "https://spclient.spotify.com/recently-played/v3/user/:id/recently-played", trace::Reply {
					status: if reply.is_ok() { 200 } else { 0 },
					bytes: reply.as_ref().map_or(0, |bytes| bytes.len()),
					..trace::Reply::default()
				});
				let mut history: History = serde_json::from_slice(&reply?)?;
				history.contexts.sort_by_key(|recent| std::cmp::Reverse(recent.last_played_time));
				let uris: Vec<_> = history.contexts.iter().filter_map(|recent| recent.last_played_track_uri.clone()).collect();
				let rows = tracks(self.shared.session(), &uris, None).await?;
				Ok::<_, librespot_core::Error>(attach_contexts(history.contexts, rows))
			})
			.map_err(spotify::Error::from)
	}
}

fn attach_contexts(contexts: Vec<Recent>, tracks: Vec<Track>) -> Vec<(Track, Option<PlayContext>)> {
	// Batch metadata retains request order but drops removed songs. Walk both streams in
	// order, so missing songs cannot shift a context onto its neighbour (duplicates are valid).
	let mut tracks = tracks.into_iter().peekable();
	contexts
		.into_iter()
		.filter_map(|recent| {
			let uri = recent.last_played_track_uri.as_deref()?;
			if tracks.peek().is_none_or(|track| track.uri != uri) {
				return None;
			}
			Some((tracks.next()?, context(recent.uri)))
		})
		.collect()
}

fn context(mut uri: String) -> Option<PlayContext> {
	if let Some(query) = uri.find('?') {
		uri.truncate(query);
	}
	let kind = if uri.ends_with(":collection") { "collection" } else { uri.strip_prefix("spotify:")?.split(':').next()? };
	let kind = kind.to_owned();
	Some(PlayContext { uri, kind })
}

#[cfg(test)]
mod tests {
	use super::*;

	fn song(uri: &str) -> Track {
		Track {
			uri:         uri.into(),
			name:        "Song".into(),
			duration_ms: 100,
			disc_number: 1,
			explicit:    false,
			artists:     Vec::new(),
			album:       None,
			is_local:    false,
			is_episode:  false,
		}
	}

	#[test]
	fn history_uses_latest_song_of_each_context_and_skips_missing_rows() {
		let history: History = serde_json::from_value(serde_json::json!({"playContexts":[
			{"uri":"spotify:playlist:p?sessionId=123", "lastPlayedTime":300,"lastPlayedTrackUri":"spotify:track:a"},
			{"uri":"spotify:album:b", "lastPlayedTime":200,"lastPlayedTrackUri":"spotify:track:removed"},
			{"uri":"spotify:user:u:collection", "lastPlayedTime":100,"lastPlayedTrackUri":"spotify:track:c"},
			{"uri":"spotify:artist:x", "lastPlayedTime":10}
		]}))
		.unwrap();
		let rows = attach_contexts(history.contexts, vec![song("spotify:track:a"), song("spotify:track:c")]);
		assert_eq!(rows.len(), 2);
		assert_eq!(rows[0].0.uri, "spotify:track:a");
		assert_eq!(rows[0].1, Some(PlayContext { uri: "spotify:playlist:p".into(), kind: "playlist".into() }));
		assert_eq!(rows[1].1.as_ref().unwrap().kind, "collection");
		assert!(serde_json::from_value::<History>(serde_json::json!({"error": "bad history"})).is_err());
	}

	#[test]
	fn the_same_song_can_name_different_recent_contexts() {
		let contexts = vec![
			Recent {
				uri:                   "spotify:album:a".into(),
				last_played_time:      2,
				last_played_track_uri: Some("spotify:track:t".into()),
			},
			Recent {
				uri:                   "spotify:playlist:p".into(),
				last_played_time:      1,
				last_played_track_uri: Some("spotify:track:t".into()),
			},
		];
		let rows = attach_contexts(contexts, vec![song("spotify:track:t"), song("spotify:track:t")]);
		assert_eq!(rows.len(), 2);
		assert_eq!(rows[0].1.as_ref().unwrap().kind, "album");
		assert_eq!(rows[1].1.as_ref().unwrap().kind, "playlist");
	}
}
