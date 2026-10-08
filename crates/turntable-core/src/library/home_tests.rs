//! Native Home history and account-scoped snapshot state.

use super::{Library, Response, Tile};
use crate::spotify::User;

fn connect(library: &mut Library, account: &str) {
	library
		.apply(Response::Connected { me: User { id: account.into(), display_name: Some(account.into()) }, playlists: Vec::new() });
}

#[test]
fn home_recent_tracks_are_usable_before_enrichment_and_keep_cached_shelves() -> Result<(), Box<dyn std::error::Error>> {
	let mut library = Library::default();
	connect(&mut library, "alex");
	library.home.recents = vec![Tile::liked()];
	library.home.followed = Some(vec![Tile::liked()]);
	library.home.albums = vec![Tile::liked()];
	let track = serde_json::from_value(serde_json::json!({"uri":"spotify:track:recent", "name":"Recent"}))?;
	library.apply(Response::HomeRecentTracks(vec![track]));
	assert!(library.home.loaded);
	assert_eq!(library.ready_label(), "home");
	assert_eq!(library.rows().first().map(|track| track.name.as_str()), Some("Recent"));
	assert_eq!(library.home.recents, [Tile::liked()]);
	assert_eq!(library.home.followed, Some(vec![Tile::liked()]));
	assert_eq!(library.home.albums, [Tile::liked()]);
	library.apply(Response::Error { route: None, message: "context enrichment failed".into() });
	assert_eq!(library.rows().len(), 1, "usable history remains visible after a later shelf failure");
	Ok(())
}

#[test]
fn home_recent_history_dedupes_before_the_twenty_track_limit() -> Result<(), Box<dyn std::error::Error>> {
	let mut played = Vec::new();
	for index in 0..30 {
		let track: crate::spotify::Track = serde_json::from_value(serde_json::json!({
			"uri":format!("spotify:track:{index}"), "name":format!("Track {index}")
		}))?;
		played.push((track.clone(), None));
		played.push((track, None));
	}
	let recent = super::home::recent_tracks(&played);
	assert_eq!(recent.len(), 20);
	assert_eq!(recent.first().map(|track| track.uri.as_str()), Some("spotify:track:0"));
	assert_eq!(recent.last().map(|track| track.uri.as_str()), Some("spotify:track:19"));
	Ok(())
}

#[test]
fn home_partial_disk_updates_keep_cached_shelves_and_refresh_history() -> Result<(), Box<dyn std::error::Error>> {
	let directory = tempfile::tempdir()?;
	let mut store = super::snapshot::Store::new(directory.path());
	store.record(&Response::Connected { me: User { id: "alex".into(), display_name: Some("Alex".into()) }, playlists: Vec::new() });
	store.record(&Response::Home(super::Home { recents: vec![Tile::liked()], albums: vec![Tile::liked()], ..super::Home::default() }));
	let track = serde_json::from_value(serde_json::json!({"uri":"spotify:track:recent", "name":"Recent"}))?;
	store.record(&Response::HomeRecentTracks(vec![track]));
	store.save()?;
	let mut restored = super::snapshot::Store::new(directory.path());
	restored.load(Some("alex"));
	let home = restored.home().ok_or("missing partial Home snapshot")?;
	assert!(home.loaded);
	assert_eq!(home.recents, [Tile::liked()]);
	assert_eq!(home.albums, [Tile::liked()]);
	assert_eq!(home.recent_tracks.first().map(|track| track.name.as_str()), Some("Recent"));
	Ok(())
}
