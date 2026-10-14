//! Offline application journeys: real Controller dispatch, request channels, event pumping
//! and account-scoped stores. Only service answers are supplied by local fixtures.

use super::*;
use crate::{
	app::{Dialog, Field, Msg},
	engine::{Entity, Event, QueueEntry, SignInError},
	library::{Folder, Listing, Request, Response, RootEntry, Route, Sort, Tile},
	spotify::{Page, Playlist, SearchResults, Track, User},
};

#[derive(serde::Deserialize)]
struct Data {
	playlists: Vec<Playlist>,
	tracks:    Vec<Track>,
}

fn data() -> Data {
	serde_json::from_str(include_str!("../../../tests/fixtures/controller.json")).unwrap()
}

fn account(name: &str) -> State {
	let mut state = State { signed_in: true, ..State::default() };
	state
		.library
		.apply(Response::Connected { me: User { id: name.into(), display_name: Some(name.into()) }, playlists: data().playlists });
	state
}

fn fixture(name: &str) -> (Controller, Paths, tempfile::TempDir) {
	let directory = tempfile::tempdir().unwrap();
	let paths = Paths { config: directory.path().join("config"), cache: directory.path().join("cache") };
	let controller = Controller::fixture(&paths, account(name)).unwrap();
	(controller, paths, directory)
}

fn answer(controller: &mut Controller, response: Response) {
	controller.fixture_response(response).unwrap();
	assert!(controller.pump(), "the service answer must invalidate the host");
}

fn seed_snapshot(controller: &mut Controller) {
	answer(controller, Response::Connected {
		me:        User { id: "alice".into(), display_name: Some("Alice".into()) },
		playlists: data().playlists,
	});
	answer(controller, Response::Listing {
		route:   Route::Playlist("source".into()),
		offset:  0,
		listing: Listing {
			title: "Zebra mix".into(),
			context_uri: Some("spotify:playlist:source".into()),
			tracks: data().tracks,
			total: 3,
			owned: true,
			editable: true,
			added_at: vec![Some(100), Some(200), Some(300)],
			positions: vec![0, 1, 2],
			..Listing::default()
		},
	});
	controller.fixture_requests().unwrap();
}

#[test]
fn playlist_snapshot_sort_selection_and_add_dialog_reach_the_request_channel() {
	let (mut controller, paths, _directory) = fixture("alice");
	seed_snapshot(&mut controller);
	drop(controller);
	let mut controller = Controller::fixture(&paths, account("alice")).unwrap();
	controller.dispatch(Msg::Navigate(Route::Playlist("source".into())));
	assert!(controller.pump(), "opening a playlist must deliver the real disk snapshot");
	assert_eq!(controller.state().library.rows(), data().tracks);
	controller.dispatch(Msg::LibrarySort(Sort::Alphabetical));
	let titles: Vec<_> = controller.state().library.entries(true).iter().map(|entry| entry.title().to_owned()).collect();
	assert_eq!(titles, ["Amber mix", "Followed mix", "Liked Songs", "Zebra mix"]);
	// Sorting the sidebar must not sort playlist occurrences or misalign added dates.
	assert_eq!(controller.state().library.rows(), data().tracks);
	assert_eq!(controller.state().library.listing.as_ref().unwrap().added_at(1), Some(200));
	for expected in [1, 2, 0] {
		controller.dispatch(Msg::Select(controller.state().library.row_target(expected).unwrap()));
		assert_eq!(controller.state().library.selected, Some(expected));
	}
	controller.dispatch(Msg::Select(controller.state().library.row_target(1).unwrap()));
	controller.dispatch(Msg::AddRowToPlaylist(controller.state().library.row_target(1).unwrap()));
	assert!(matches!(&controller.state().dialog, Some(Dialog::AddToPlaylist { uris, .. })
		if uris == &["spotify:track:0000000000000000000002"]));
	controller.fixture_requests().unwrap();
	controller.dispatch(Msg::AddToPlaylist(Some("target".into())));
	assert!(controller.state().dialog.is_none());
	assert!(matches!(controller.fixture_requests().unwrap().as_slice(), [Request::AddToPlaylist(id, name, uris)]
		if id == "target" && name == "Amber mix" && uris == &["spotify:track:0000000000000000000002"]));
	answer(&mut controller, Response::Done("fixture addition acknowledged".into()));
	assert!(controller.state().toast.is_some());
	assert_eq!(controller.state().library.route, Route::Playlist("source".into()));
	// The alternate picker path creates a new playlist with exactly the chosen occurrence.
	controller.dispatch(Msg::AddRowToPlaylist(controller.state().library.row_target(2).unwrap()));
	controller.dispatch(Msg::AddToPlaylist(None));
	assert!(matches!(controller.fixture_requests().unwrap().as_slice(), [Request::CreatePlaylist(_, uris)]
		if uris == &["spotify:track:0000000000000000000001"]));
}

#[test]
fn submitted_search_results_and_recents_survive_restart_only_for_their_account() {
	let (mut controller, paths, _directory) = fixture("alice");
	controller.dispatch(Msg::Focus(Some(Field::Search)));
	controller.dispatch(Msg::SetText(Field::Search, "  fixture query  ".into()));
	assert_eq!(controller.state().library.route, Route::Home);
	assert!(controller.fixture_requests().unwrap().is_empty(), "typing only schedules a debounce");
	controller.dispatch(Msg::SearchSubmit("  fixture query  ".into()));
	controller.dispatch(Msg::Navigate(Route::Search));
	assert!(matches!(controller.fixture_requests().unwrap().as_slice(), [Request::Search(query)] if query == "fixture query"));
	assert!(controller.search_at.is_none(), "Enter cancels the scheduled duplicate request");
	answer(&mut controller, Response::Search {
		query:   "fixture query".into(),
		results: SearchResults { tracks: Page { items: data().tracks, ..Page::default() }, ..SearchResults::default() },
	});
	assert_eq!(controller.state().library.rows(), data().tracks);
	controller.dispatch(Msg::Select(controller.state().library.row_target(1).unwrap()));
	let chosen = Tile {
		route: Route::Album("album".into()),
		play:  Some(data().tracks[1].uri.clone()),
		title: "First alphabetically".into(),
		sub:   "Song • Artist".into(),
		image: None,
	};
	assert_eq!(
		controller.state().recent_searches(),
		std::slice::from_ref(&chosen),
		"selecting a result remembers it without a separate UI message"
	);
	drop(controller);
	let alice = Controller::fixture(&paths, account("alice")).unwrap();
	assert_eq!(alice.state().recent_searches(), std::slice::from_ref(&chosen));
	let mut bob = Controller::fixture(&paths, account("bob")).unwrap();
	assert!(bob.state().recent_searches().is_empty());
	let other = Tile { route: Route::Liked, title: "Bob's selection".into(), ..chosen.clone() };
	bob.dispatch(Msg::RememberSearch(other.clone()));
	drop(bob);
	let mut alice = Controller::fixture(&paths, account("alice")).unwrap();
	assert_eq!(alice.state().recent_searches(), &[chosen]);
	alice.dispatch(Msg::ForgetSearch(Route::Album("album".into())));
	drop(alice);
	assert!(Controller::fixture(&paths, account("alice")).unwrap().state().recent_searches().is_empty());
	assert_eq!(Controller::fixture(&paths, account("bob")).unwrap().state().recent_searches(), &[other]);
}

#[test]
fn playing_a_search_result_remembers_it_but_library_playback_does_not() {
	let (mut controller, _paths, _directory) = fixture("alice");
	controller.dispatch(Msg::SearchSubmit("fixture query".into()));
	controller.dispatch(Msg::Navigate(Route::Search));
	answer(&mut controller, Response::Search {
		query:   "fixture query".into(),
		results: SearchResults { tracks: Page { items: data().tracks, ..Page::default() }, ..SearchResults::default() },
	});
	controller.dispatch(Msg::PlayRow(controller.state().library.row_target(1).unwrap()));
	let selected = Tile::track(&data().tracks[1]).unwrap();
	assert_eq!(controller.state().recent_searches(), std::slice::from_ref(&selected));
	controller.dispatch(Msg::Navigate(Route::Album("album".into())));
	answer(&mut controller, Response::Listing {
		route:   Route::Album("album".into()),
		offset:  0,
		listing: Listing { tracks: data().tracks, ..Listing::default() },
	});
	controller.dispatch(Msg::Select(controller.state().library.row_target(0).unwrap()));
	controller.dispatch(Msg::PlayRow(controller.state().library.row_target(0).unwrap()));
	assert_eq!(controller.state().recent_searches(), &[selected], "ordinary library selections must not become recent searches");
}

#[test]
fn create_rename_and_move_folder_journey_keeps_navigation_and_native_hierarchy() {
	let (mut controller, _paths, _directory) = fixture("alice");
	let renamed = create_named_folder(&mut controller);
	rename_folder_inline_and_cancel_reopening(&mut controller);
	move_playlist_and_delete_folder(&mut controller, renamed);
}

fn create_named_folder(controller: &mut Controller) -> Folder {
	controller.dispatch(Msg::Navigate(Route::Library));
	controller.fixture_requests().unwrap();
	controller.dispatch(Msg::CreateFolder);
	assert!(matches!(controller.fixture_requests().unwrap().as_slice(), [Request::CreateFolder]));
	let folder = Folder { id: "folder".into(), name: "New Folder".into(), count: 0, children: Vec::new() };
	answer(controller, Response::Rootlist {
		account: "alice".into(),
		entries: vec![RootEntry::Folder(folder.clone()), RootEntry::Playlist("source".into())],
	});
	answer(controller, Response::FolderCreated { id: "folder".into(), name: "New Folder".into() });
	assert!(matches!(&controller.state().dialog, Some(Dialog::RenameFolder { id, .. }) if id == "folder"));
	assert_eq!(controller.state().focus, Some(Field::PlaylistName));
	assert!(controller.state().field_visible(Field::PlaylistName));
	assert!(!controller.state().field_visible(Field::PlaylistDescription));
	controller.dispatch(Msg::Edit { from: 0, to: 10, text: "  雨: + mixes  ".into() });
	controller.dispatch(Msg::Submit);
	assert!(matches!(controller.fixture_requests().unwrap().as_slice(), [Request::RenameFolder(id, name)]
		if id == "folder" && name == "雨: + mixes"));
	let renamed = Folder { name: "雨: + mixes".into(), ..folder };
	answer(controller, Response::Rootlist {
		account: "alice".into(),
		entries: vec![RootEntry::Folder(renamed.clone()), RootEntry::Playlist("source".into())],
	});
	assert_eq!(controller.state().library.folder("folder").unwrap().name, "雨: + mixes");
	renamed
}

fn rename_folder_inline_and_cancel_reopening(controller: &mut Controller) {
	controller.dispatch(Msg::StartRenameFolder("folder".into()));
	assert_eq!(controller.state().name_input, "雨: + mixes");
	controller.dispatch(Msg::Edit { from: 0, to: 12, text: "Saved inline".into() });
	controller.dispatch(Msg::SaveRename);
	assert!(matches!(controller.fixture_requests().unwrap().as_slice(), [Request::RenameFolder(id, name)]
		if id == "folder" && name == "Saved inline"));
	assert!(controller.state().dialog.is_none());
	controller.dispatch(Msg::StartRenameFolder("folder".into()));
	controller.dispatch(Msg::CloseDialog);
	assert!(controller.fixture_requests().unwrap().is_empty(), "Escape must not save an inline rename");
}

fn move_playlist_and_delete_folder(controller: &mut Controller, renamed: Folder) {
	controller.dispatch(Msg::MoveToFolder { uri: "spotify:playlist:source".into(), folder: Some("folder".into()) });
	assert!(matches!(controller.fixture_requests().unwrap().as_slice(), [Request::MoveToFolder { uri, folder: Some(id) }]
		if uri == "spotify:playlist:source" && id == "folder"));
	answer(controller, Response::Rootlist {
		account: "alice".into(),
		entries: vec![RootEntry::Folder(Folder { count: 1, children: vec![RootEntry::Playlist("source".into())], ..renamed })],
	});
	controller.dispatch(Msg::ToggleFolder("folder".into()));
	assert!(matches!(controller.fixture_requests().unwrap().as_slice(), [Request::SetFolderExpanded { id, expanded: true }]
		if id == "folder"));
	assert!(controller.state().library.folder_expanded("folder"));
	let tree = controller.state().library.tree(true);
	assert_eq!(tree.iter().filter(|row| row.entry.route() == Route::Playlist("source".into())).count(), 1);
	let moved = tree.iter().find(|row| row.entry.route() == Route::Playlist("source".into())).unwrap();
	assert_eq!((moved.depth, moved.parent), (1, Some("folder")));
	assert_eq!(controller.state().library.route, Route::Library);
	controller.dispatch(Msg::DeleteFolder("folder".into()));
	assert!(matches!(controller.fixture_requests().unwrap().as_slice(), [Request::DeleteFolder(id)] if id == "folder"));
	assert_eq!(controller.state().library.route, Route::Library);
}

fn assert_account_cleared(controller: &Controller) {
	let state = controller.state();
	assert!(!state.signed_in);
	assert!(state.needs_setup());
	assert!(state.library.me.is_none());
	assert!(state.library.playlists.is_empty());
	assert!(state.library.rootlist.is_empty());
	assert!(state.library.rows().is_empty());
	assert!(state.library.search.is_none());
	assert!(state.library.liked_tracks.is_none());
	assert!(state.library.listing.is_none());
	assert!(state.library.query.is_empty());
	assert!(state.item.uri.is_empty());
	assert!(state.context.uri.is_empty());
	assert!(state.upcoming.is_empty());
	assert!(state.recent_searches().is_empty());
	assert!(state.dialog.is_none());
	assert!(state.focus.is_none());
	assert!(state.panel.is_none());
	assert!(!state.library.is_pinned("spotify:playlist:source"));
	assert!(!state.added("spotify:track:0000000000000000000001"));
}

#[test]
fn signout_clears_account_projections_files_and_already_queued_answers() {
	let (mut controller, paths, _directory) = fixture("alice");
	seed_snapshot(&mut controller);
	controller.dispatch(Msg::RememberSearch(Tile::liked()));
	drop(controller);
	let mut controller = Controller::fixture(&paths, account("alice")).unwrap();
	assert_eq!(controller.state().recent_searches(), &[Tile::liked()]);
	answer(&mut controller, Response::Rootlist {
		account: "alice".into(),
		entries: vec![RootEntry::Folder(Folder {
			id:       "folder".into(),
			name:     "Private folder".into(),
			count:    1,
			children: vec![RootEntry::Playlist("source".into())],
		})],
	});
	controller.dispatch(Msg::SearchSubmit("private query".into()));
	answer(&mut controller, Response::Search {
		query:   "private query".into(),
		results: SearchResults { tracks: Page { items: data().tracks, ..Page::default() }, ..SearchResults::default() },
	});
	controller.dispatch(Msg::Navigate(Route::Playlist("source".into())));
	controller.pump();
	controller.dispatch(Msg::SetPinned { uri: "spotify:playlist:source".into(), pinned: true });
	controller.dispatch(Msg::AddToLiked("spotify:track:0000000000000000000001".into()));
	controller.dispatch(Msg::AddRowToPlaylist(controller.state().library.row_target(0).unwrap()));
	controller.state.item = Entity { uri: "spotify:track:0000000000000000000001".into(), ..Entity::default() };
	controller.state.context.uri = "spotify:playlist:source".into();
	controller.state.upcoming.push(QueueEntry { uid: "old".into(), source: "queue".into(), item: controller.state.item.clone() });
	std::fs::write(paths.config.join("credentials.json"), b"fixture credentials").unwrap();
	// Queue before signout, but deliver afterward through the production pump.
	controller
		.fixture_response(Response::Connected { me: User { id: "alice".into(), display_name: None }, playlists: data().playlists })
		.unwrap();
	controller.dispatch(Msg::ToggleSidebar);
	controller.dispatch(Msg::SignOut);
	assert_account_cleared(&controller);
	assert!(controller.state().sidebar_collapsed(), "sign-out must retain local view preferences");
	controller.pump();
	assert_account_cleared(&controller);
	controller.fixture_response(Response::LikedTracks(Arc::from([]))).unwrap();
	controller.pump();
	assert_account_cleared(&controller);
	assert!(!paths.config.join("credentials.json").exists());
	assert!(controller.fixture_requests().unwrap().iter().any(|request| matches!(request, Request::ForgetSnapshot)));
	controller.fixture_complete_forget().unwrap();
	assert!(!paths.cache.join("library").exists());
	drop(controller);
	assert!(crate::app::settings::Settings::load(&paths.config).sidebar_collapsed);
	assert!(Controller::fixture(&paths, account("alice")).unwrap().state().recent_searches().is_empty());
}

#[test]
fn premium_rejection_performs_signout_without_retry_or_late_account_resurrection() {
	for from_library in [false, true] {
		let (mut controller, paths, _directory) = fixture("alice");
		seed_snapshot(&mut controller);
		controller.dispatch(Msg::Navigate(Route::Playlist("source".into())));
		controller.pump();
		controller.dispatch(Msg::AddRowToPlaylist(controller.state().library.row_target(0).unwrap()));
		controller.state.starting = true;
		controller.retry_start_at = Some(Instant::now());
		std::fs::write(paths.config.join("credentials.json"), b"fixture credentials").unwrap();
		let error = SignInError::PremiumRequired { account: Some("Alice".into()), product: Some("free".into()) };
		let refusal = if from_library {
			AppEvent::Library(controller.account_generation, Box::new(Response::SignInRejected(error.clone())))
		} else {
			AppEvent::EngineReady(controller.account_generation, Err(StartError::Premium(error.clone())))
		};
		controller.tx.send(refusal).unwrap();
		controller
			.fixture_response(Response::Connected {
				me:        User { id: "alice".into(), display_name: None },
				playlists: data().playlists,
			})
			.unwrap();
		assert!(controller.pump());
		assert_account_cleared(&controller);
		assert_eq!(controller.state().sign_in_error.as_ref(), Some(&error));
		assert!(controller.state().notice.is_some());
		assert!(!controller.state().offline);
		assert!(!controller.state().starting);
		assert!(controller.retry_start_at.is_none());
		assert!(!paths.config.join("credentials.json").exists());
		assert!(controller.fixture_requests().unwrap().iter().any(|request| matches!(request, Request::ForgetSnapshot)));
		controller.fixture_complete_forget().unwrap();
		assert!(!paths.cache.join("library").exists());
		assert!(!controller.on_event(AppEvent::Player(0, Incoming::Event(Event::VolumeChanged { volume: 50 }))));
	}
}
