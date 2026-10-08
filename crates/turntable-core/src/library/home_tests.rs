//! Personalised Home cache selection, Show all navigation and native fallback state.

use super::{Facet, HomeFeed, HomeItem, HomeSection, HomeStyle, Library, Request, Response, Route, Tile};
use crate::{Msg, State, app::Effect, spotify::User};

fn feed(title: &str) -> HomeFeed {
	HomeFeed {
		greeting: "Good afternoon".into(),
		sections: vec![HomeSection {
			uri:    "spotify:section:test".into(),
			title:  title.into(),
			kicker: Some("Made For".into()),
			style:  HomeStyle::Shelf,
			items:  vec![HomeItem {
				tile:        Tile::liked(),
				label:       None,
				description: None,
				backdrop:    None,
				color:       None,
			}],
			more:   true,
		}],
	}
}

fn connect(library: &mut Library, account: &str) {
	library
		.apply(Response::Connected { me: User { id: account.into(), display_name: Some(account.into()) }, playlists: Vec::new() });
}

#[test]
fn home_facets_keep_cached_answers_and_failed_refreshes() {
	let mut library = Library::default();
	connect(&mut library, "alex");
	library.apply(Response::HomeFeed { facet: Facet::All, feed: feed("All") });
	assert_eq!(library.ready_label(), "home");
	assert!(matches!(library.select_home_facet(Facet::Music), Request::HomeFacet(Facet::Music)));
	assert!(library.home_feed().is_none(), "another facet's data is not mislabeled as Music");
	library.apply(Response::HomeFeed { facet: Facet::Music, feed: feed("Music") });
	library.apply(Response::HomeFeed { facet: Facet::All, feed: feed("New All") });
	library.apply(Response::HomeUnavailable { facet: Facet::Music });
	assert_eq!(library.home_feed(), Some(&feed("Music")), "a failed refresh keeps the cached facet");
	library.select_home_facet(Facet::All);
	assert_eq!(library.home_feed(), Some(&feed("New All")));
	connect(&mut library, "bo");
	assert!(library.home_feed().is_none(), "a different account cannot borrow personalised recommendations");
}

#[test]
fn home_section_show_all_paging_and_close_do_not_change_feed() -> Result<(), Box<dyn std::error::Error>> {
	let mut library = Library::default();
	connect(&mut library, "alex");
	let home = feed("alex");
	let section = home.sections.first().ok_or("missing section")?.clone();
	let uri = section.uri.clone();
	library.apply(Response::HomeFeed { facet: Facet::All, feed: home.clone() });
	assert!(matches!(library.open_home_section(uri.clone()), Request::HomeSectionMore { offset: 0, .. }));
	assert_eq!(library.home_section.as_ref(), Some(&section), "the cached shelf paints before Show all returns");
	assert!(library.home_section_loading);
	assert!(library.home_section_more().is_none());
	library.apply(Response::HomeSection { section: section.clone(), offset: 0, next_offset: Some(17) });
	assert!(matches!(library.home_section_more(), Some(Request::HomeSectionMore { offset: 17, .. })));
	library.apply(Response::HomeSectionFailed { uri, message: "offline".into() });
	assert!(!library.home_section_loading);
	assert!(matches!(library.home_section_more(), Some(Request::HomeSectionMore { offset: 17, .. })));
	library.apply(Response::HomeSection { section: section.clone(), offset: 17, next_offset: None });
	assert!(library.error.is_none(), "a successful section retry clears the previous error");
	assert_eq!(library.home_section.as_ref().ok_or("missing Show all")?.items.len(), 2);
	assert_eq!(library.home_feed(), Some(&home));
	library.close_home_section();
	library.apply(Response::HomeSection { section, offset: 0, next_offset: Some(17) });
	assert!(library.home_section.is_none(), "late replies never reopen a closed Show all page");
	assert_eq!(library.home_feed(), Some(&home));
	Ok(())
}

#[test]
fn home_messages_expose_section_navigation_without_a_new_route_or_playback() {
	let mut state = State::default();
	connect(&mut state.library, "alex");
	state.library.apply(Response::HomeFeed { facet: Facet::All, feed: feed("alex") });
	let effects = state.update(Msg::HomeSectionMore("spotify:section:test".into()));
	assert!(matches!(effects.as_slice(), [Effect::Library(Request::HomeSectionMore { offset: 0, .. })]));
	assert_eq!(state.library.route, Route::Home);
	assert!(state.home_section().is_some());
	assert!(state.update(Msg::CloseHomeSection).is_empty());
	assert!(state.home_section().is_none());
	assert!(state.home_feed().is_some());
	let effects = state.update(Msg::HomeFacet(Facet::Podcasts));
	assert!(matches!(effects.as_slice(), [Effect::Library(Request::HomeFacet(Facet::Podcasts))]));
	assert!(state.home_feed().is_none());
}

#[test]
fn home_disk_snapshots_restore_facets_without_crossing_accounts() -> Result<(), Box<dyn std::error::Error>> {
	let directory = tempfile::tempdir()?;
	let mut store = super::snapshot::Store::new(directory.path());
	store.record(&Response::Connected { me: User { id: "alex".into(), display_name: Some("Alex".into()) }, playlists: Vec::new() });
	store.record(&Response::HomeFeed { facet: Facet::All, feed: feed("All") });
	store.record(&Response::HomeFeed { facet: Facet::Music, feed: feed("Music") });
	store.save()?;
	let mut restored = super::snapshot::Store::new(directory.path());
	restored.load(Some("alex"));
	assert_eq!(restored.home_feed(Facet::All), Some(feed("All")));
	assert_eq!(restored.home_feed(Facet::Music), Some(feed("Music")));
	assert!(restored.home_feed(Facet::Podcasts).is_none());
	restored.load(Some("bo"));
	assert!(restored.home_feed(Facet::All).is_none());
	Ok(())
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
