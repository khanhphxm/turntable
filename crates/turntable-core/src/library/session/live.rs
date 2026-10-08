//! Opt-in session-only acceptance: no Connect registration, activation, or playback.
//! Run `TURNTABLE_TEST_CONFIG=~/.config/turntable TURNTABLE_TRACE=/tmp/session-writes.jsonl`
//! `CARGO_TARGET_DIR=target/agent-sessionwrites cargo test -p turntable-core session_only_write_round_trip -- --ignored --nocapture`.
//! Every membership assertion reads a fresh native collection; cleanup also runs after panics.

use std::{error::Error, path::PathBuf};

use librespot_core::{SessionConfig, cache::Cache};

use super::*;
use crate::spotify::PlaylistChanges;

type Outcome = Result<(), Box<dyn Error>>;

fn round_trip(reader: &mut Reader, candidates: &[&str], label: &str) -> Outcome {
	let mut selected = None;
	for uri in candidates {
		if !reader.contains(uri)? {
			selected = Some(*uri);
			break;
		}
	}
	let uri = selected.ok_or("All acceptance candidates already belong to the library; none were changed")?;
	eprintln!("{label} initially absent: {uri}");
	let changed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Outcome {
		reader.set_saved(uri, true)?;
		assert!(Reader::new(reader.shared.clone()).contains(uri)?, "native write must be visible to a fresh collection read");
		eprintln!("{label} saved/followed: {uri}");
		Ok(())
	}));
	let cleanup = reader.set_saved(uri, false);
	cleanup?;
	assert!(!Reader::new(reader.shared.clone()).contains(uri)?, "cleanup must restore absence, not merely update the local cache");
	eprintln!("{label} restored absent: {uri}");
	match changed {
		Ok(result) => result,
		Err(panic) => std::panic::resume_unwind(panic),
	}
}

fn scratch_visibility(reader: &mut Reader) -> Outcome {
	let scratch = reader.create_playlist("turntable session-write check", &[])?;
	eprintln!("scratch created: {}", scratch.uri);
	let changed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Outcome {
		for public in [true, false] {
			reader.rename_playlist(&scratch.id, &PlaylistChanges { public: Some(public), ..Default::default() })?;
			let fresh = Reader::new(reader.shared.clone());
			let (returned, _, _) = fresh.playlist(&scratch.id, 0, false)?;
			assert_eq!(returned.public, Some(public));
			eprintln!("scratch visibility verified: public={public}");
		}
		Ok(())
	}));
	reader.follow_playlist(&scratch.uri, false)?;
	assert!(!Reader::new(reader.shared.clone()).contains(&scratch.uri)?);
	eprintln!("scratch deleted from rootlist: {}", scratch.uri);
	match changed {
		Ok(result) => result,
		Err(panic) => std::panic::resume_unwind(panic),
	}
}

fn trace_summary() -> Outcome {
	if let Some(path) = std::env::var_os("TURNTABLE_TRACE") {
		let path = if path == "1" { std::env::temp_dir().join("turntable-trace.jsonl") } else { PathBuf::from(path) };
		let mut api = 0;
		let mut session = 0;
		for line in std::fs::read_to_string(&path)?.lines() {
			let row: serde_json::Value = serde_json::from_str(line)?;
			if row["k"] == "http" {
				if row["c"] == "api" || row["ep"].as_str().is_some_and(|endpoint| endpoint.starts_with("api.spotify.com/")) {
					api += 1;
				}
				if row["c"] == "session" {
					session += 1;
				}
			}
		}
		eprintln!(
			"trace {}: native requests={session}; api.spotify.com requests={api}; playback requests=0 (no player constructed)",
			path.display()
		);
		assert_eq!(api, 0);
	}
	Ok(())
}

#[test]
#[ignore = "temporarily edits native library membership and a new scratch playlist; requires saved real-account credentials"]
fn session_only_write_round_trip() -> Outcome {
	let config =
		std::env::var_os("TURNTABLE_TEST_CONFIG").map(PathBuf::from).ok_or("Set TURNTABLE_TEST_CONFIG to the saved account directory")?;
	trace::init();
	let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build()?;
	let cache = Cache::new(Some(&config), Some(&config), None::<&PathBuf>, None)?;
	let credentials = cache.credentials().ok_or("No saved credentials in the test config")?;
	let session = {
		let _context = runtime.enter();
		Session::new(SessionConfig::default(), None)
	};
	runtime.block_on(session.connect(credentials, false))?;
	// Await native login5 before inspecting ProductInfo; no Web API token refresh or calls.
	runtime.block_on(session.login5().auth_token())?;
	let (_stop, rx) = tokio::sync::watch::channel(false);
	let shared = SessionReader::from_session(session.clone(), runtime.handle().clone(), rx);
	let mut reader = Reader::new(shared);
	let (me, plan) = reader.profile(None)?;
	assert_eq!(me.id, session.username());
	assert_eq!(plan.as_deref(), Some("premium"));
	eprintln!("native profile: display_name={:?}; plan={plan:?}", me.display_name);
	let result = (|| -> Outcome {
		round_trip(
			&mut reader,
			&["spotify:track:4uLU6hMCjMI75M1A2tKUQC", "spotify:track:7qiZfU4dY1lWllzX7mPBI3", "spotify:track:11dFghVXANMlKmJXsNCbNl"],
			"track",
		)?;
		round_trip(
			&mut reader,
			&["spotify:artist:6eUKZXaKkcviH0Ku9w2n3V", "spotify:artist:0OdUWJ0sBjDrqHygGUXeCF", "spotify:artist:1Xyo4u8uXC1ZmMpatF05PJ"],
			"artist",
		)?;
		scratch_visibility(&mut reader)
	})();
	session.shutdown();
	trace_summary()?;
	result
}

fn folder_names(entries: &[super::super::RootEntry], depth: usize) {
	for entry in entries {
		if let super::super::RootEntry::Folder(folder) = entry {
			eprintln!("native folder: depth={depth}; name={:?}; playlists={}", folder.name, folder.count);
			folder_names(&folder.children, depth + 1);
		}
	}
}

#[test]
#[ignore = "reads real folder names/counts and temporarily edits only new scratch folders/playlists; requires saved credentials"]
fn session_only_folder_round_trip() -> Outcome {
	let config =
		std::env::var_os("TURNTABLE_TEST_CONFIG").map(PathBuf::from).ok_or("Set TURNTABLE_TEST_CONFIG to the saved account directory")?;
	trace::init();
	let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build()?;
	let cache = Cache::new(Some(&config), Some(&config), None::<&PathBuf>, None)?;
	let credentials = cache.credentials().ok_or("No saved credentials in the test config")?;
	let session = {
		let _context = runtime.enter();
		Session::new(SessionConfig::default(), None)
	};
	runtime.block_on(session.connect(credentials, false))?;
	runtime.block_on(session.login5().auth_token())?;
	let (_stop, rx) = tokio::sync::watch::channel(false);
	let mut reader = Reader::new(SessionReader::from_session(session.clone(), runtime.handle().clone(), rx));
	let original = reader.rootlist()?;
	folder_names(&original, 0);
	let mut outer = None;
	let mut inner = None;
	let mut scratch = None;
	let changed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Outcome {
		let parent = reader.create_folder("turntable folder check")?;
		outer = Some(parent.clone());
		let nested = reader.create_folder("turntable nested check")?;
		inner = Some(nested.clone());
		let playlist = reader.create_playlist("turntable folder item check", &[])?;
		scratch = Some(playlist.clone());
		reader
			.move_to_folder(&format!("spotify:folder:{nested}"), Some(&parent))
			.map_err(|error| format!("Nesting scratch folder: {error}"))?;
		reader.move_to_folder(&playlist.uri, Some(&nested)).map_err(|error| format!("Moving scratch playlist into folder: {error}"))?;
		reader.rename_folder(&parent, "turntable 雨: + folder").map_err(|error| format!("Renaming scratch folder: {error}"))?;
		let roots = Reader::new(reader.shared.clone()).rootlist()?;
		let folder = super::super::folders::find(&roots, &parent).ok_or("Created folder missing from fresh rootlist")?;
		assert_eq!(folder.name, "turntable 雨: + folder");
		assert_eq!(folder.count, 1);
		assert_eq!(super::super::folders::find(&folder.children, &nested).map(|folder| folder.count), Some(1));
		eprintln!("scratch nested folder names/counts verified; rename URI escaping verified");
		reader.delete_folder(&parent)?;
		outer = None;
		let roots = Reader::new(reader.shared.clone()).rootlist()?;
		assert!(
			roots.iter().any(|entry| matches!(entry, super::super::RootEntry::Folder(folder) if folder.id == nested && folder.count == 1)),
			"deleting folder must promote the nested folder, not remove its children"
		);
		reader.move_to_folder(&playlist.uri, None)?;
		let roots = Reader::new(reader.shared.clone()).rootlist()?;
		assert!(roots.iter().any(|entry| matches!(entry, super::super::RootEntry::Playlist(id) if id == &playlist.id)));
		assert_eq!(super::super::folders::find(&roots, &nested).map(|folder| folder.count), Some(0));
		eprintln!("delete-folder promotion and move-to-top-level verified");
		Ok(())
	}));
	let cleanup = (|| -> Outcome {
		if let Some(playlist) = scratch {
			reader.follow_playlist(&playlist.uri, false)?;
		}
		if let Some(id) = inner {
			reader.delete_folder(&id)?;
		}
		if let Some(id) = outer {
			reader.delete_folder(&id)?;
		}
		assert_eq!(
			Reader::new(reader.shared.clone()).rootlist()?,
			original,
			"scratch cleanup must restore the original hierarchy and order"
		);
		eprintln!("scratch folders/playlists removed; original rootlist hierarchy/order restored");
		Ok(())
	})();
	session.shutdown();
	trace_summary()?;
	cleanup?;
	match changed {
		Ok(result) => result,
		Err(panic) => std::panic::resume_unwind(panic),
	}
}

#[test]
#[ignore = "reads native playlist pictures and validates real account artwork; requires saved credentials"]
fn session_only_playlist_covers() -> Outcome {
	let config =
		std::env::var_os("TURNTABLE_TEST_CONFIG").map(PathBuf::from).ok_or("Set TURNTABLE_TEST_CONFIG to the saved account directory")?;
	trace::init();
	let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build()?;
	let cache = Cache::new(Some(&config), Some(&config), None::<&PathBuf>, None)?;
	let credentials = cache.credentials().ok_or("No saved credentials in the test config")?;
	let session = {
		let _context = runtime.enter();
		Session::new(SessionConfig::default(), None)
	};
	runtime.block_on(session.connect(credentials, false))?;
	runtime.block_on(session.login5().auth_token())?;
	let (_stop, rx) = tokio::sync::watch::channel(false);
	let mut reader = Reader::new(SessionReader::from_session(session.clone(), runtime.handle().clone(), rx));
	let roots = reader.rootlist()?;
	let mut targets = Vec::new();
	for entry in &roots {
		if let super::super::RootEntry::Folder(folder) = entry
			&& matches!(folder.name.as_str(), "folder one" | "folder two")
		{
			for child in &folder.children {
				if let super::super::RootEntry::Playlist(id) = child {
					targets.push(id.as_str());
				}
			}
		}
	}
	let playlists = reader.playlists()?;
	let nordic nights = playlists.iter().find(|playlist| playlist.name == "nordic nights").ok_or("Real account nordic nights playlist missing")?;
	targets.push(&nordic nights.id);
	assert_eq!(targets.len(), 13, "nordic nights plus every playlist in both folders");
	for id in targets {
		let playlist = playlists.iter().find(|playlist| playlist.id == id).ok_or("Folder playlist metadata missing")?;
		let cover =
			spotify::pick_image(&playlist.images, 300).ok_or_else(|| format!("Native image still missing for {}", playlist.name))?;
		assert!(cover.url.starts_with("https://"));
		assert!(Tile::playlist(playlist).image.is_some(), "Home/sidebar tile carries the native cover");
		eprintln!("native artwork verified: name={:?}; image={}", playlist.name, cover.url);
	}
	assert!(!reader.playlist(&nordic nights.id, 0, false)?.0.images.is_empty(), "playlist header obtains native artwork too");
	let cache = tempfile::tempdir()?;
	let mut store = super::super::snapshot::Store::new(cache.path());
	store.record(&super::super::Response::Connected { me: reader.identity().0, playlists });
	store.save()?;
	let mut restored = super::super::snapshot::Store::new(cache.path());
	restored.load(Some(&session.username()));
	let Some(super::super::Response::Connected { playlists, .. }) = restored.connected() else {
		return Err("Artwork snapshot missing".into());
	};
	assert!(playlists.iter().find(|playlist| playlist.name == "nordic nights").is_some_and(|playlist| !playlist.images.is_empty()));
	eprintln!("native playlist header, Home/sidebar tiles, and disk snapshot artwork verified");
	session.shutdown();
	trace_summary()
}
