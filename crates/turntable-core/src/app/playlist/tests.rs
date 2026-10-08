//! Library menu actions never depend on, or navigate to, the current page.

use super::*;
use crate::{
	app::Msg,
	library::{Entry, Library, Listing, Response, Sort},
	spotify::{Album, Artist, Playlist, SearchResults, User},
};

fn playlist(id: &str, owner: &str) -> Playlist {
	serde_json::from_value(serde_json::json!({
		"id": id, "name": format!("Playlist {id}"), "description": "Details", "public": false,
		"owner": { "id": owner }
	}))
	.unwrap()
}

fn state() -> State {
	State {
		library: Library {
			me: Some(User { id: "me".into(), display_name: Some("Me".into()) }),
			playlists: vec![playlist("own", "me"), playlist("other", "someone")],
			route: Route::Album("current".into()),
			listing: Some(Listing { title: "Current album".into(), ..Listing::default() }),
			..Library::default()
		},
		..State::default()
	}
}

#[test]
fn edit_playlist_uses_cached_details_without_navigating() {
	let mut state = state();
	assert!(state.update(Msg::EditPlaylist("own".into())).is_empty());
	assert_eq!(state.library.route, Route::Album("current".into()));
	assert_eq!(state.library.listing.as_ref().unwrap().title, "Current album");
	assert_eq!(state.name_input, "Playlist own");
	assert_eq!(state.description_input, "Details");
	assert_eq!(state.public_input, Some(false));
	assert!(matches!(state.dialog, Some(Dialog::Rename { ref id, .. }) if id == "own"));
	state.update(Msg::CloseDialog);
	assert!(state.update(Msg::EditPlaylist("other".into())).is_empty());
	assert!(state.dialog.is_none(), "only owners may edit details");
}

#[test]
fn edit_playlist_loads_missing_details_and_ignores_a_closed_dialog() {
	let mut state = state();
	state.library.playlists[0].public = None;
	assert!(matches!(state.update(Msg::EditPlaylist("own".into())).as_slice(),
		[Effect::Library(library::Request::PlaylistDetails(id))] if id == "own"));
	let mut response = Response::PlaylistDetails(playlist("own", "me"));
	state.apply_library_dialog(&mut response);
	assert_eq!(state.name_input, "Playlist own");
	assert_eq!(state.public_input, Some(false));
	assert_eq!(state.library.route, Route::Album("current".into()));
	state.update(Msg::EditPlaylist("missing".into()));
	state.update(Msg::CloseDialog);
	state.apply_library_dialog(&mut Response::PlaylistDetails(playlist("missing", "me")));
	assert!(state.dialog.is_none());
}

#[test]
fn delete_playlist_removes_owned_and_followed_rows_without_navigation() {
	for id in ["own", "other"] {
		let mut state = state();
		let uri = format!("spotify:playlist:{id}");
		assert!(matches!(state.update(Msg::DeletePlaylist(id.into())).as_slice(),
			[Effect::Library(library::Request::SetFollowed(target, false))] if target == &uri));
		assert!(!state.library.playlists.iter().any(|playlist| playlist.id == id));
		assert_eq!(state.library.route, Route::Album("current".into()));
	}
}

#[test]
fn set_playlist_public_changes_only_the_target_owned_playlist() {
	let mut state = state();
	assert!(matches!(state.update(Msg::SetPlaylistPublic("own".into(), true)).as_slice(),
		[Effect::Library(library::Request::RenamePlaylist(id, changes))]
		if id == "own" && changes.public == Some(true) && changes.name.is_none() && changes.description.is_none()));
	assert_eq!(state.library.playlists[0].public, Some(true));
	assert_eq!(state.library.listing.as_ref().unwrap().public, None);
	assert!(state.update(Msg::SetPlaylistPublic("other".into(), true)).is_empty());
	assert_eq!(state.library.playlists[1].public, Some(false));
	assert_eq!(state.library.route, Route::Album("current".into()));
}

#[test]
fn set_saved_immediately_adds_and_removes_cached_albums_artists_and_playlists() {
	let album: Album = serde_json::from_value(serde_json::json!({ "uri": "spotify:album:album", "name": "Album" })).unwrap();
	let artist: Artist = serde_json::from_value(serde_json::json!({ "uri": "spotify:artist:artist", "name": "Artist" })).unwrap();
	let mut state = state();
	let mut search = SearchResults::default();
	search.albums.items.push(album);
	search.artists.items.push(artist);
	search.playlists.items.push(playlist("saved", "someone"));
	state.library.search = Some(search);
	for uri in ["spotify:album:album", "spotify:artist:artist", "spotify:playlist:saved"] {
		assert!(matches!(state.update(Msg::SetSaved { uri: uri.into(), saved: true }).as_slice(),
			[Effect::Library(library::Request::SetFollowed(target, true))] if target == uri));
		state.update(Msg::SetSaved { uri: uri.into(), saved: true });
	}
	assert_eq!((state.library.albums.len(), state.library.followed.len(), state.library.playlists.len()), (1, 1, 3));
	for uri in ["spotify:album:album", "spotify:artist:artist", "spotify:playlist:saved"] {
		state.update(Msg::SetSaved { uri: uri.into(), saved: false });
	}
	assert_eq!((state.library.albums.len(), state.library.followed.len(), state.library.playlists.len()), (0, 0, 2));
	assert_eq!(state.library.route, Route::Album("current".into()));
}

#[test]
fn pinned_entries_lead_every_sort_and_fifth_pin_shows_a_toast() {
	let mut state = state();
	state.update(Msg::SetPinned { uri: "spotify:playlist:other".into(), pinned: true });
	for sort in Sort::ALL {
		state.library.sort = sort;
		assert!(matches!(state.library.entries(false).first(), Some(Entry::Playlist(playlist)) if playlist.id == "other"));
	}
	for uri in ["spotify:playlist:own", "spotify:album:album", "spotify:artist:artist"] {
		state.update(Msg::SetPinned { uri: uri.into(), pinned: true });
	}
	assert!(state.update(Msg::SetPinned { uri: "spotify:playlist:fifth".into(), pinned: true }).is_empty());
	assert!(state.toast.as_ref().is_some_and(|(_, text)| text.contains("4 items")));
	assert!(state.library.is_pinned("spotify:playlist:other"));
	assert!(matches!(state.update(Msg::SetPinned { uri: "spotify:playlist:other".into(), pinned: false }).as_slice(), [Effect::Library(
		library::Request::SetPinned { pinned: false, .. }
	)]));
	assert!(!state.library.is_pinned("spotify:playlist:other"));
}

#[test]
fn pin_and_exclusion_responses_are_bound_to_the_signed_in_account() {
	let mut state = state();
	state.library.apply(Response::Pins { account: "someone".into(), uris: vec!["spotify:playlist:own".into()] });
	assert!(!state.library.is_pinned("spotify:playlist:own"));
	state.library.apply(Response::Pins { account: "me".into(), uris: vec!["spotify:playlist:own".into()] });
	state.library.apply(Response::Exclusions { account: "me".into(), uris: vec!["spotify:playlist:own".into()] });
	assert!(state.library.is_pinned("spotify:playlist:own"));
	assert!(state.library.is_excluded("spotify:playlist:own"));
	state.library.apply(Response::Connected { me: User { id: "new".into(), display_name: None }, playlists: Vec::new() });
	assert!(!state.library.is_pinned("spotify:playlist:own"));
	assert!(!state.library.is_excluded("spotify:playlist:own"));
}

#[test]
fn add_context_to_playlist_opens_only_its_own_completed_response() {
	let mut state = state();
	let route = Route::Playlist("own".into());
	assert!(matches!(state.update(Msg::AddContextToPlaylist(route.clone())).as_slice(),
		[Effect::Library(library::Request::AddContextToPlaylist(target))] if target == &route));
	assert!(state.dialog.is_none());
	state.apply_library_dialog(&mut Response::ContextToPlaylist(Route::Album("stale".into()), Vec::new(), "Stale".into()));
	assert!(state.dialog.is_none());
	state.apply_library_dialog(&mut Response::ContextToPlaylist(
		route.clone(),
		vec!["spotify:track:a".into(), "spotify:track:a".into()],
		"Source".into(),
	));
	assert!(matches!(state.dialog, Some(Dialog::AddToPlaylist { ref uris, ref label }) if uris.len() == 2 && label == "Source"));
	assert_eq!(state.library.route, Route::Album("current".into()));
	state.update(Msg::AddContextToPlaylist(route.clone()));
	state.update(Msg::CloseDialog);
	state.apply_library_dialog(&mut Response::ContextToPlaylist(route, Vec::new(), "Late".into()));
	assert!(state.dialog.is_none());
}

#[test]
fn collaborative_toggle_is_item_specific_and_owner_only() {
	let mut state = state();
	assert!(matches!(state.update(Msg::SetCollaborative("own".into(), true)).as_slice(),
		[Effect::Library(library::Request::SetCollaborative(id, true))] if id == "own"));
	assert!(state.library.playlists[0].collaborative);
	assert!(state.update(Msg::SetCollaborative("other".into(), true)).is_empty());
	assert!(!state.library.playlists[1].collaborative);
	assert_eq!(state.library.route, Route::Album("current".into()));
}

#[test]
fn folder_creation_and_existing_inline_rename_submit_and_cancel_without_navigation() {
	let mut state = state();
	assert!(matches!(state.update(Msg::CreateFolder).as_slice(), [Effect::Library(library::Request::CreateFolder)]));
	state.library.rootlist = vec![library::RootEntry::Folder(library::Folder {
		id:       "folder".into(),
		name:     "New Folder".into(),
		count:    0,
		children: Vec::new(),
	})];
	state.apply_library_dialog(&mut Response::FolderCreated { id: "folder".into(), name: "New Folder".into() });
	assert!(matches!(state.dialog, Some(Dialog::RenameFolder { ref id, ref name }) if id == "folder" && name == "New Folder"));
	assert!(state.field_visible(Field::PlaylistName));
	assert!(!state.field_visible(Field::PlaylistDescription));
	state.name_input = "  Renamed Folder  ".into();
	assert!(
		matches!(state.update(Msg::Submit).as_slice(), [Effect::Library(library::Request::RenameFolder(id, name))] if id == "folder" && name == "Renamed Folder")
	);
	assert!(state.dialog.is_none());
	assert!(state.update(Msg::StartRenameFolder("folder".into())).is_empty());
	assert_eq!(state.name_input, "New Folder");
	assert_eq!(state.focus, Some(Field::PlaylistName));
	state.name_input = "Saved inline".into();
	assert!(
		matches!(state.update(Msg::SaveRename).as_slice(), [Effect::Library(library::Request::RenameFolder(id, name))] if id == "folder" && name == "Saved inline")
	);
	state.update(Msg::StartRenameFolder("folder".into()));
	assert!(state.update(Msg::CloseDialog).is_empty());
	assert!(state.dialog.is_none());
	assert_eq!(state.focus, None);
	assert_eq!(state.library.route, Route::Album("current".into()), "inline rename does not navigate");
}

#[test]
fn folder_expansion_move_and_delete_emit_item_specific_requests_without_navigation() {
	let mut state = state();
	state.library.rootlist = vec![library::RootEntry::Folder(library::Folder {
		id:       "folder".into(),
		name:     "Folder".into(),
		count:    0,
		children: Vec::new(),
	})];
	assert!(matches!(state.update(Msg::ToggleFolder("folder".into())).as_slice(), [Effect::Library(
		library::Request::SetFolderExpanded { expanded: true, .. }
	)]));
	assert!(state.library.folder_expanded("folder"));
	assert!(matches!(state.update(Msg::MoveToFolder { uri: "spotify:playlist:own".into(), folder: Some("folder".into()) }).as_slice(), [
		Effect::Library(library::Request::MoveToFolder { .. })
	]));
	assert!(
		matches!(state.update(Msg::DeleteFolder("folder".into())).as_slice(), [Effect::Library(library::Request::DeleteFolder(id))] if id == "folder")
	);
	assert_eq!(state.library.route, Route::Album("current".into()), "folder menu actions do not navigate");
}

#[test]
fn taste_exclusion_toggles_native_membership_without_touching_saves() {
	let mut state = state();
	let uri = "spotify:playlist:own";
	assert!(matches!(state.update(Msg::ExcludeFromTaste { uri: uri.into(), excluded: true }).as_slice(),
		[Effect::Library(library::Request::SetExcluded { uri: target, excluded: true })] if target == uri));
	assert!(state.library.is_excluded(uri));
	assert_eq!(state.library.playlists.len(), 2);
	state.update(Msg::ExcludeFromTaste { uri: uri.into(), excluded: false });
	assert!(!state.library.is_excluded(uri));
	assert!(state.update(Msg::ExcludeFromTaste { uri: "spotify:album:album".into(), excluded: true }).is_empty());
}

#[test]
fn liked_songs_only_leads_the_library_after_an_explicit_pin() {
	let mut state = state();
	state.library.sort = Sort::Alphabetical;
	assert!(!state.library.is_pinned("spotify:collection:tracks"));
	state.update(Msg::SetPinned { uri: "spotify:collection:tracks".into(), pinned: true });
	assert!(matches!(state.library.entries(false).first(), Some(Entry::Liked(_))));
	state.update(Msg::SetPinned { uri: "spotify:collection:tracks".into(), pinned: false });
	assert!(!state.library.is_pinned("spotify:collection:tracks"));
	state.library.sort = Sort::Recents;
	assert!(!matches!(state.library.entries(false).first(), Some(Entry::Liked(_))));
}

#[test]
fn an_older_request_error_cannot_cancel_the_new_context_dialog() {
	let mut state = state();
	state.update(Msg::AddContextToPlaylist(Route::Album("old".into())));
	let current = Route::Playlist("own".into());
	state.update(Msg::AddContextToPlaylist(current.clone()));
	state.apply_library_dialog(&mut Response::Error { route: None, message: "Older album failed".into() });
	let mut response = Response::ContextToPlaylist(current, vec!["spotify:track:current".into()], "Current".into());
	state.apply_library_dialog(&mut response);
	assert!(matches!(state.dialog, Some(Dialog::AddToPlaylist { ref label, .. }) if label == "Current"));
}
