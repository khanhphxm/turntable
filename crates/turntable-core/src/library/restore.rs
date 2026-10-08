//! Startup fallback when Connect supplies no paused player: native session history first,
//! otherwise Web API history, with account-scoped cached track metadata when offline.
//! Position checkpoints are private config files; only a matching track borrows the saved
//! position. Reads and writes run on the library worker, independently of the current page.

use std::{
	fs, io,
	path::{Path, PathBuf},
};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{session::Reader, snapshot::Store};
use crate::{
	app::{config, connect::entity_of},
	engine::{Entity, LastPlayback, Options},
	spotify::{self, PlayContext, Track},
};

const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaybackPosition {
	pub account:     String,
	pub uri:         String,
	pub context_uri: String,
	pub position_ms: u64,
}

#[derive(Serialize, Deserialize)]
struct Saved {
	version:  u32,
	position: PlaybackPosition,
}

fn path(config: &Path, account: &str) -> PathBuf {
	config.join("playback").join(format!("{:x}.json", Sha256::digest(account.as_bytes())))
}

pub(super) fn remember(config: &Path, position: PlaybackPosition) -> io::Result<()> {
	fs::create_dir_all(config.join("playback"))?;
	let file = path(config, &position.account);
	let bytes = serde_json::to_vec(&Saved { version: VERSION, position }).map_err(io::Error::other)?;
	config::write_private(&file, &bytes)
}

fn read(config: &Path, account: &str) -> Option<PlaybackPosition> {
	let saved: Saved = serde_json::from_slice(&fs::read(path(config, account)).ok()?).ok()?;
	(saved.version == VERSION && saved.position.account == account).then_some(saved.position)
}

pub(super) fn recent(
	config: &Path,
	account: &str,
	client: &spotify::Client,
	player: Option<&Reader>,
	playlists: &[spotify::Playlist],
	snapshots: &Mutex<Store>,
) -> spotify::Result<Option<LastPlayback>> {
	let saved = read(config, account);
	let recent = match player.filter(|player| player.matches(account)) {
		Some(player) => player.recently_played(1),
		None => client.recently_played(1),
	}
	.map(|recent| recent.first().map(|(track, context)| from_recent(track, context.as_ref(), playlists, saved.as_ref())));
	match recent {
		Err(error @ (spotify::Error::AuthRequired | spotify::Error::AuthExpired | spotify::Error::NativeAuthRequired)) => Err(error),
		Ok(Some(playback)) => Ok(Some(playback)),
		result => {
			let cached = snapshots.lock().recent_track(account, saved.as_ref().map(|saved| saved.uri.as_str()));
			if let Some(track) = cached { Ok(Some(from_recent(&track, None, playlists, saved.as_ref()))) } else { result.map(|_| None) }
		},
	}
}

fn from_recent(
	track: &Track,
	context: Option<&PlayContext>,
	playlists: &[spotify::Playlist],
	saved: Option<&PlaybackPosition>,
) -> LastPlayback {
	let saved = saved.filter(|saved| saved.uri == track.uri);
	let position_ms = saved.map_or(0, |saved| saved.position_ms);
	let context_uri = context
		.map(|context| context.uri.as_str())
		.filter(|uri| !uri.is_empty())
		.or_else(|| saved.map(|saved| saved.context_uri.as_str()))
		.unwrap_or_default();
	let name = playlists
		.iter()
		.find(|playlist| playlist.uri == context_uri)
		.map(|playlist| playlist.name.as_str())
		.or_else(|| track.album.as_ref().filter(|album| album.uri == context_uri).map(|album| album.name.as_str()))
		.unwrap_or_default();
	let mut context = Entity { uri: context_uri.to_owned(), ..Entity::default() };
	context.decorations.identity.name = name.to_owned();
	LastPlayback { item: entity_of(track), context, position_ms, options: Options::default() }
}

#[cfg(test)]
mod tests {
	use super::*;

	fn track(uri: &str) -> Track {
		serde_json::from_value(serde_json::json!({"uri": uri, "name": "Last song", "duration_ms": 248_000})).unwrap()
	}

	#[test]
	fn recently_played_uses_only_the_matching_tracks_saved_position() {
		let saved = PlaybackPosition {
			account:     "alex".into(),
			uri:         "spotify:track:last".into(),
			context_uri: "spotify:playlist:last".into(),
			position_ms: 2_000,
		};
		let restored = from_recent(&track(&saved.uri), None, &[], Some(&saved));
		assert_eq!(restored.position_ms, 2_000);
		assert_eq!(restored.context.uri, saved.context_uri);
		let different = from_recent(&track("spotify:track:new"), None, &[], Some(&saved));
		assert_eq!(different.position_ms, 0);
		assert!(different.context.uri.is_empty());
	}

	#[test]
	fn positions_are_account_scoped_and_corruption_is_ignored() {
		let dir = tempfile::tempdir().unwrap();
		let saved = PlaybackPosition {
			account:     "alex".into(),
			uri:         "spotify:track:last".into(),
			context_uri: String::new(),
			position_ms: 2_000,
		};
		remember(dir.path(), saved.clone()).unwrap();
		assert_eq!(read(dir.path(), "alex"), Some(saved));
		assert!(read(dir.path(), "other").is_none());
		fs::write(path(dir.path(), "alex"), b"broken").unwrap();
		assert!(read(dir.path(), "alex").is_none());
	}

	#[test]
	fn cache_only_restore_uses_matching_cached_metadata_and_position_without_auth_or_io() {
		let directory = tempfile::tempdir().unwrap();
		let client = spotify::Client::open(std::sync::Arc::new(|| panic!("cache-only restore must not request a Web API token")), false);
		let mut store = Store::new(directory.path());
		store.record(&crate::library::Response::Connected {
			me:        spotify::User { id: "alex".into(), display_name: None },
			playlists: Vec::new(),
		});
		store.record(&crate::library::Response::Home(crate::library::Home {
			recent_tracks: vec![track("spotify:track:newer"), track("spotify:track:saved")],
			..crate::library::Home::default()
		}));
		let snapshots = Mutex::new(store);
		remember(directory.path(), PlaybackPosition {
			account:     "alex".into(),
			uri:         "spotify:track:saved".into(),
			context_uri: "spotify:playlist:context".into(),
			position_ms: 42_000,
		})
		.unwrap();
		let restored = recent(directory.path(), "alex", &client, None, &[], &snapshots).unwrap().unwrap();
		assert_eq!(restored.item.uri, "spotify:track:saved");
		assert_eq!(restored.item.name(), "Last song");
		assert_eq!(restored.item.duration_ms(), 248_000);
		assert_eq!(restored.position_ms, 42_000);
		assert_eq!(restored.context.uri, "spotify:playlist:context");
		assert!(recent(directory.path(), "another-account", &client, None, &[], &snapshots).is_err());
	}
}
