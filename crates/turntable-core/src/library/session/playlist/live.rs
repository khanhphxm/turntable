//! Opt-in account test: connects only a Session, never a Connect player or playback.
//!
//! Run with `TURNTABLE_TEST_CONFIG=~/.config/turntable CARGO_TARGET_DIR=target/agent-playlistedit`
//! `cargo test -p turntable-core scratch_playlist_round_trip -- --ignored --nocapture`.
//! Playlist4 request counts include edits and assertions, not login/token traffic.

use std::{error::Error, path::PathBuf, sync::atomic::Ordering};

use librespot_core::{SessionConfig, cache::Cache};

use super::*;
use crate::{
	engine::SessionReader,
	library::added::{Membership, PlaylistId, PlaylistIndex, TrackId},
};

fn rows(reader: &Reader, id: &str) -> Result<SelectedListContent, Box<dyn Error>> {
	Ok(reader.shared.block_on(fetch(reader.shared.session(), id, 0, 100))?)
}

fn uris(content: &SelectedListContent) -> Vec<&str> {
	content.contents.get_or_default().items.iter().map(Item::uri).collect()
}

#[test]
#[ignore = "edits only a newly created scratch playlist; requires saved real-account credentials"]
fn scratch_playlist_round_trip() -> Result<(), Box<dyn Error>> {
	let config =
		std::env::var_os("TURNTABLE_TEST_CONFIG").map(PathBuf::from).ok_or("Set TURNTABLE_TEST_CONFIG to the saved account directory")?;
	let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build()?;
	let cache = Cache::new(Some(&config), Some(&config), None::<&PathBuf>, None)?;
	let credentials = cache.credentials().ok_or("No saved credentials in the test config")?;
	let session = {
		let _context = runtime.enter();
		Session::new(SessionConfig::default(), None)
	};
	runtime.block_on(session.connect(credentials, false))?;
	let (_stop, rx) = tokio::sync::watch::channel(false);
	let mut reader = Reader::new(SessionReader::from_session(session.clone(), runtime.handle().clone(), rx));
	let directory = tempfile::tempdir()?;
	let mut membership = Membership::new(directory.path());
	membership.attach(Some(reader.shared.clone()), &|_| {});
	REQUESTS.store(0, Ordering::Relaxed);
	let scratch = reader.create_playlist("turntable test", &[])?;
	eprintln!("scratch created: {}", scratch.uri);
	membership.created(&scratch.uri, &scratch.name, &[], &|_| {});
	let playlist_id = PlaylistId::from_uri(&scratch.uri)?;
	let a = "spotify:track:4uLU6hMCjMI75M1A2tKUQC";
	let b = "spotify:track:7qiZfU4dY1lWllzX7mPBI3";
	// Cleanup runs after both normal errors and assertion failures, never on another playlist.
	let edited = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(), Box<dyn Error>> {
		let additions = vec![a.to_owned(), b.to_owned(), a.to_owned()];
		reader.add_to_playlist(&scratch.id, &additions)?;
		membership.appended(&scratch.uri, &additions, &|_| {});
		let content = rows(&reader, &scratch.id)?;
		assert_eq!(content.attributes.name(), "turntable test");
		assert_eq!(content.attributes.description(), "", "new native playlists must not store a null description");
		assert_eq!(uris(&content), [a, b, a]);
		let index = PlaylistIndex::load(directory.path(), &session.username())?;
		assert_eq!(index.playlist_tracks(&playlist_id).map(<[TrackId]>::len), Some(3));
		assert!(index.contains(&TrackId::from_uri(a)?));
		reader.move_playlist_row(&scratch.id, 0, 2)?;
		assert_eq!(uris(&rows(&reader, &scratch.id)?), [b, a, a]);
		reader.move_playlist_row(&scratch.id, 2, 0)?;
		assert_eq!(uris(&rows(&reader, &scratch.id)?), [a, b, a]);
		reader.remove_from_playlist(&scratch.id, a, 0)?;
		membership.removed_row(&scratch.uri, a, &|_| {});
		assert_eq!(uris(&rows(&reader, &scratch.id)?), [b, a]);
		let index = PlaylistIndex::load(directory.path(), &session.username())?;
		assert!(index.contains(&TrackId::from_uri(a)?), "duplicate occurrence keeps membership");
		assert_eq!(index.playlist_tracks(&playlist_id).map(<[TrackId]>::len), Some(2));
		let rename = PlaylistChanges {
			name:        Some("turntable test edited".into()),
			description: Some("scratch round trip".into()),
			public:      None,
		};
		reader.rename_playlist(&scratch.id, &rename)?;
		membership.renamed(&scratch.uri, "turntable test edited", &|_| {});
		let content = rows(&reader, &scratch.id)?;
		assert_eq!(content.attributes.name(), "turntable test edited");
		assert_eq!(content.attributes.description(), "scratch round trip");
		let index = PlaylistIndex::load(directory.path(), &session.username())?;
		assert_eq!(index.playlists().next().map(|playlist| playlist.name), Some("turntable test edited"));
		Ok(())
	}));
	let cleanup = reader.follow_playlist(&scratch.uri, false);
	if cleanup.is_ok() {
		membership.deleted(&scratch.uri, &|_| {});
		let index = PlaylistIndex::load(directory.path(), &session.username())?;
		assert_eq!(index.playlist_count(), 0);
		assert_eq!(index.track_count(), 0);
		let root = reader.shared.block_on(root(&session, 0, 120))?;
		assert!(root.contents.get_or_default().items.iter().all(|item| item.uri() != scratch.uri));
		eprintln!("scratch deleted: {}", scratch.uri);
	}
	eprintln!("Playlist4 requests: {}; Web API requests: 0; playback requests: 0", REQUESTS.load(Ordering::Relaxed));
	cleanup?;
	match edited {
		Ok(result) => result,
		Err(panic) => std::panic::resume_unwind(panic),
	}
}
