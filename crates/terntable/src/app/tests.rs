//! Tests of application state and the messages that transition it.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::{input::replace_utf16, *};
use crate::{
	engine::{Command, Entity, Event, Position, QueueEntry, Status},
	library::{self, Route},
	sources::{external::Lyrics, lyrics},
	spotify::{PlayStart, PlaylistChanges},
};

fn playing(duration_ms: u64, at_ms: f64) -> State {
	let mut s = State { status: Status::Playing, ..State::default() };
	s.item.decorations.playback.duration_ms = duration_ms;
	s.active = true;
	s.anchor = Anchor::new(at_ms, 0.0, 1.0);
	s
}

/// The player commands among `effects`, in order.
fn cmds(effects: Vec<Effect>) -> Vec<Command> {
	effects
		.into_iter()
		.filter_map(|e| match e {
			Effect::Player(c) => Some(c),
			_ => None,
		})
		.collect()
}

#[test]
fn seek_cells_map_onto_the_track_and_clamp() {
	let mut s = playing(240_000, 0.0);
	assert_eq!(cmds(s.update(Msg::SeekCell(SEEK_CELLS / 2))), vec![Command::Seek { position_ms: 120_000 }]);
	assert_eq!(cmds(s.update(Msg::SeekCell(SEEK_CELLS + 7))), vec![Command::Seek { position_ms: 240_000 }]);
	// Nothing to seek in without a duration.
	let mut idle = State::default();
	assert!(idle.update(Msg::SeekCell(3)).is_empty());
}

#[test]
fn relative_seek_stays_inside_the_track() {
	let mut s = playing(60_000, 2_000.0);
	let Command::Seek { position_ms } = cmds(s.update(Msg::SeekBy(-10)))[0] else { panic!() };
	assert_eq!(position_ms, 0);
	let mut s = playing(60_000, 58_000.0);
	let Command::Seek { position_ms } = cmds(s.update(Msg::SeekBy(10)))[0] else { panic!() };
	assert_eq!(position_ms, 59_000, "never seeks past the last second");
}

#[test]
fn volume_steps_saturate_and_mute_restores() {
	let mut s = State { volume: 98, ..State::default() };
	assert_eq!(cmds(s.update(Msg::VolumeBy(1))), vec![Command::SetVolume { volume: 100 }]);
	s.volume = 3;
	assert_eq!(cmds(s.update(Msg::VolumeBy(-1))), vec![Command::SetVolume { volume: 0 }]);
	assert_eq!(cmds(s.update(Msg::VolumeCell(VOLUME_CELLS))), vec![Command::SetVolume { volume: 100 }]);
	s.volume = 35;
	assert_eq!(cmds(s.update(Msg::ToggleMute)), vec![Command::SetVolume { volume: 0 }]);
	assert_eq!(cmds(s.update(Msg::ToggleMute)), vec![Command::SetVolume { volume: 35 }]);
}

#[test]
fn a_band_click_where_the_thumb_stands_keeps_the_preset() {
	let mut s = State::default();
	s.update(Msg::EqPreset("rock"));
	assert_eq!(s.eq_preset.as_deref(), Some("rock"));
	// Rock has band 0 at +4 dB: clicking its own step changes nothing, any other makes the curve the user's.
	s.update(Msg::EqBand(0, eq_db_step(4.0)));
	assert_eq!(s.eq_preset.as_deref(), Some("rock"));
	s.update(Msg::EqBand(0, 0));
	assert_eq!((s.eq_preset.as_deref(), s.eq.gains_db[0]), (None, -12.0));
}

#[test]
fn pause_freezes_the_local_clock() {
	let mut s = playing(200_000, 10_000.0);
	s.apply(Event::PlaybackChanged { status: Status::Paused });
	let frozen = s.position_ms();
	std::thread::sleep(Duration::from_millis(20));
	assert_eq!(s.position_ms(), frozen);
}

#[test]
fn position_sync_ignores_a_stale_timestamp() {
	let mut s = playing(200_000, 0.0);
	// Timestamp an hour in the past: trust arrival time, not the stale clock.
	let hour_ago = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as f64 - 3_600_000.0;
	s.apply(Event::PositionSync { position: Position { position_ms: 30_000.0, timestamp_ms: hour_ago, speed: 1.0 } });
	assert!((30_000..31_000).contains(&s.position_ms()));
}

#[test]
fn setup_needs_both_player_sign_in_and_library_access() {
	let mut s = State::default();
	assert!(s.needs_setup());
	s.signed_in = true;
	assert!(s.needs_setup());
	s.library.conn = library::Conn::Ready;
	assert!(!s.needs_setup());
	s.signed_in = false;
	assert!(s.needs_setup());
}

#[test]
fn native_edits_count_utf16_units() {
	// "🎵" is two UTF-16 units: an edit after it must land after it, not inside it.
	let mut s = "🎵ab".to_owned();
	replace_utf16(&mut s, 2, 3, "X");
	assert_eq!(s, "🎵Xb");
	replace_utf16(&mut s, 4, 4, "!");
	assert_eq!(s, "🎵Xb!");
	replace_utf16(&mut s, 0, 99, "");
	assert_eq!(s, "");
}

#[test]
fn submitting_a_field_routes_to_its_action() {
	// The client-ID field is on screen during setup.
	let mut s = State { rows: 40, cols: 200, focus: Some(Field::ClientId), ..State::default() };
	s.update(Msg::Type(" abc123 \n".into()));
	let effects = s.update(Msg::Submit);
	assert!(matches!(effects.as_slice(), [Effect::SaveClientId(id)] if id == "abc123"));
	// Search, once connected: Enter searches now and drops the pending debounce.
	s.library.conn = library::Conn::Ready;
	s.library.route = Route::Search;
	s.focus = Some(Field::Search);
	s.update(Msg::Type("cigarettes".into()));
	let effects = s.update(Msg::Submit);
	assert!(matches!(
		effects.as_slice(),
		[Effect::CancelSearchSoon, Effect::Library(library::Request::Search(q))] if q == "cigarettes"
	));
}

#[test]
fn focus_leaves_a_field_that_left_the_screen() {
	let mut s = State { rows: 40, cols: 200, ..State::default() };
	s.library.conn = library::Conn::Ready;
	s.library.route = Route::Search;
	s.focus = Some(Field::Search);
	s.update(Msg::Navigate(Route::Liked));
	assert_eq!(s.focus, None, "keys must reach the player again");
}

/// Signed in on a wide pane, with a playlist in Your Library.
fn with_library() -> State {
	let mut s = State { rows: 40, cols: 200, cell_w: 9.0, cell_h: 18.0, ..State::default() };
	s.library.conn = library::Conn::Ready;
	s.library.playlists =
		vec![serde_json::from_value(serde_json::json!({ "id": "p1", "name": "Road trip", "owner": { "id": "alex" } })).unwrap()];
	s
}

fn with_listing(route: Route, context: Option<&str>) -> State {
	let mut s = with_library();
	s.library.route = route;
	s.library.listing = Some(library::Listing {
		context_uri: context.map(str::to_owned),
		tracks: (0..250)
			.map(|i| {
				serde_json::from_value(serde_json::json!({ "uri": format!("spotify:track:{i}"), "name": format!("Song {i}") })).unwrap()
			})
			.collect(),
		..library::Listing::default()
	});
	s
}

#[test]
fn a_context_play_button_loads_the_player_without_library_access() {
	let mut s = State::default();
	assert!(matches!(
		s.update(Msg::PlayContext("spotify:album:a".into())).as_slice(),
		[Effect::Player(Command::Load(PlayStart::Context { uri, offset_uri: None, position_ms: 0 }))] if uri == "spotify:album:a"
	));
}

#[test]
fn playing_a_context_listing_loads_it_from_the_start() {
	for (route, uri) in [
		(Route::Playlist("p".into()), "spotify:playlist:p"),
		(Route::Album("a".into()), "spotify:album:a"),
		(Route::Artist("r".into()), "spotify:artist:r"),
	] {
		let mut s = with_listing(route.clone(), Some(uri));
		assert!(matches!(
			s.update(Msg::PlayListing).as_slice(),
			[Effect::Player(Command::Load(start))] if *start == PlayStart::Context { uri: uri.into(), offset_uri: None, position_ms: 0 }
		));
		assert_eq!(s.played_from, Some(route));
	}
}

#[test]
fn playing_a_row_or_selection_loads_its_context_at_the_chosen_track() {
	for (route, uri) in [(Route::Playlist("p".into()), "spotify:playlist:p"), (Route::Album("a".into()), "spotify:album:a")] {
		let mut s = with_listing(route.clone(), Some(uri));
		let expected = PlayStart::Context { uri: uri.into(), offset_uri: Some("spotify:track:180".into()), position_ms: 0 };
		assert!(matches!(s.update(Msg::PlayRow(180)).as_slice(), [Effect::Player(Command::Load(start))] if *start == expected));
		assert_eq!(s.library.selected, Some(180));
		assert_eq!(s.played_from, Some(route));
		assert!(matches!(s.update(Msg::PlaySelected).as_slice(), [Effect::Player(Command::Load(start))] if *start == expected));
	}
}

#[test]
fn playing_without_a_context_loads_all_tracks_and_preserves_the_offset() {
	for (route, context) in [(Route::Liked, None), (Route::Album("a".into()), None), (Route::Artist("r".into()), Some("spotify:artist:r"))]
	{
		let mut s = with_listing(route, context);
		let uris: Vec<String> = (0..250).map(|i| format!("spotify:track:{i}")).collect();
		let expected = PlayStart::Tracks { uris: uris.clone(), offset_uri: Some("spotify:track:180".into()) };
		assert!(matches!(s.update(Msg::PlayRow(180)).as_slice(), [Effect::Player(Command::Load(start))] if *start == expected));
		assert!(matches!(s.update(Msg::PlaySelected).as_slice(), [Effect::Player(Command::Load(start))] if *start == expected));
		if context.is_none() {
			let expected = PlayStart::Tracks { uris, offset_uri: Some("spotify:track:0".into()) };
			assert!(matches!(s.update(Msg::PlayListing).as_slice(), [Effect::Player(Command::Load(start))] if *start == expected));
		}
	}
	let mut s = State::default();
	assert!(s.update(Msg::PlaySelected).is_empty());
	assert!(s.update(Msg::PlayRow(0)).is_empty());
	assert!(s.update(Msg::PlayListing).is_empty());
}

#[test]
fn the_sort_menu_closes_on_a_sort_or_any_other_message_but_not_on_a_view() {
	use library::{Kind, Sort, View};
	let mut s = with_library();
	s.update(Msg::LibraryMenu);
	assert!(s.library_menu);
	s.update(Msg::LibraryView(View::Grid));
	assert!(s.library_menu, "views are tried with the menu open");
	assert_eq!(s.library.view, View::Grid);
	s.update(Msg::LibrarySort(Sort::Creator));
	assert!((!s.library_menu) && s.library.sort == Sort::Creator);
	s.update(Msg::LibraryMenu);
	s.update(Msg::LibraryKind(Kind::Artist));
	assert!(!s.library_menu, "a click elsewhere");
	s.update(Msg::LibraryMenu);
	s.update(Msg::LibraryMenu);
	assert!(!s.library_menu, "the button toggles it");
}

#[test]
fn the_sort_menu_needs_its_button_on_screen() {
	let mut s = with_library();
	s.sidebar_collapsed = true;
	s.update(Msg::LibraryMenu);
	assert!(!s.library_menu, "covers only: no button to hang from");
	let mut s = State { cols: 44, ..with_library() };
	s.update(Msg::LibraryMenu);
	assert!(!s.library_menu, "the compact layout shows the button on the library page only");
	s.library.route = Route::Library;
	s.update(Msg::LibraryMenu);
	assert!(s.library_menu);
}

#[test]
fn clearing_the_library_filter_empties_it_and_keeps_the_caret_there() {
	let mut s = with_library();
	s.library.filter = "road".into();
	s.update(Msg::ClearLibraryFilter);
	assert!(s.library.filter.is_empty());
	assert_eq!(s.focus, Some(Field::LibraryFilter), "ready to type again");
}

#[test]
fn the_library_filter_is_on_screen_while_the_sidebar_shows_it() {
	let mut s = with_library();
	assert!(s.field_visible(Field::LibraryFilter));
	s.sidebar_collapsed = true;
	assert!(!s.field_visible(Field::LibraryFilter), "covers only");
	s.sidebar_collapsed = false;
	s.library.conn = library::Conn::NeedsLogin;
	assert!(!s.field_visible(Field::LibraryFilter));
	// The compact layout has no sidebar: the field is on the library page.
	let mut s = State { cols: 44, ..with_library() };
	assert_eq!(s.layout(), Layout::Compact);
	assert!(!s.field_visible(Field::LibraryFilter));
	s.library.route = Route::Library;
	assert!(s.field_visible(Field::LibraryFilter));
}

#[test]
fn typing_in_the_library_filter_filters_without_searching() {
	let mut s = with_library();
	s.update(Msg::Focus(Some(Field::LibraryFilter)));
	assert!(s.update(Msg::Type("road".into())).is_empty(), "no search, no request");
	assert_eq!(s.library.filter, "road");
	s.update(Msg::Backspace);
	assert_eq!(s.library.filter, "roa");
	assert_eq!(s.focus, Some(Field::LibraryFilter));
}

#[test]
fn enter_in_the_library_filter_opens_the_first_match() {
	let mut s = with_library();
	s.focus = Some(Field::LibraryFilter);
	s.library.filter = "trip".into();
	let effects = s.update(Msg::Submit);
	assert!(matches!(effects.as_slice(), [Effect::Library(library::Request::Open(Route::Playlist(id), 0))] if id == "p1"), "{effects:?}");
	assert_eq!(s.focus, None, "the keys go back to the player");
	s.focus = Some(Field::LibraryFilter);
	s.library.filter = "zzz".into();
	assert!(s.update(Msg::Submit).is_empty(), "nothing to open");
	assert_eq!(s.focus, None);
}

#[test]
fn a_click_elsewhere_gives_the_filter_keys_back_to_the_player() {
	let mut s = with_library();
	s.focus = Some(Field::LibraryFilter);
	s.update(Msg::Edit { from: 0, to: 0, text: "a".into() });
	assert_eq!(s.focus, Some(Field::LibraryFilter), "editing keeps the caret");
	// The sidebar's field stays on screen after the click: only the click moves the caret.
	s.update(Msg::LibraryMenu);
	assert_eq!(s.focus, None);
	// A message that hands the caret to another field keeps it there.
	s.focus = Some(Field::LibraryFilter);
	s.library.route = Route::Playlist("p1".into());
	s.library.listing = Some(library::Listing { owned: true, ..library::Listing::default() });
	s.update(Msg::RenamePlaylist);
	assert_eq!(s.focus, Some(Field::PlaylistName));
	// The search field is on the search page alone: leaving it is the route change's business.
	s.dialog = None;
	s.library.route = Route::Search;
	s.focus = Some(Field::Search);
	s.update(Msg::Select(0));
	assert_eq!(s.focus, Some(Field::Search));
}

/// Playing at `at_ms` into a track of 240 s with its lyrics on screen: sung at 10 s, 20 s, a pause from 30 s.
fn singing(at_ms: f64) -> State {
	let mut s = playing(240_000, at_ms);
	s.item.uri = "spotify:track:abc".into();
	s.panel = Some(Panel::Lyrics);
	let lines = vec![(10_000, "one".to_owned()), (20_000, "two".to_owned()), (30_000, String::new())];
	s.lyrics = Some((s.item.uri.clone(), LyricsState::Found(Lyrics::Synced(lines))));
	s
}

#[test]
fn the_lit_line_is_the_last_one_that_started() {
	assert_eq!(singing(5_000.0).lyric_line(), None, "before the first line");
	assert_eq!(singing(10_000.0).lyric_line(), Some(0));
	assert_eq!(singing(25_000.0).lyric_line(), Some(1));
	assert_eq!(singing(200_000.0).lyric_line(), Some(2), "the last line holds to the end");
	let mut off = singing(25_000.0);
	off.update(Msg::TogglePanel(Panel::Lyrics));
	assert_eq!(off.lyric_line(), None, "nothing to follow while the lyrics are off screen");
	assert!(off.synced_lyrics().is_none());
}

#[test]
fn the_next_line_is_due_when_playback_reaches_it() {
	let wait = |s: &State| s.next_lyric_at().map(|at| at.saturating_duration_since(Instant::now()).as_millis());
	assert!((4_900..=5_000).contains(&wait(&singing(25_000.0)).unwrap()));
	assert_eq!(wait(&singing(200_000.0)), None, "no line left");
	let mut paused = singing(25_000.0);
	paused.apply(Event::PlaybackChanged { status: Status::Paused });
	assert_eq!(wait(&paused), None, "a paused song reaches nothing");
	let mut short = singing(25_000.0);
	short.item.decorations.playback.duration_ms = 28_000;
	assert_eq!(wait(&short), None, "a line past the end never starts");
}

#[test]
fn a_lookup_answers_only_the_track_still_playing() {
	let mut s = State::default();
	s.item.uri = "spotify:track:a".into();
	let answer = |uri: &str, lyrics| lyrics::Response { uri: uri.into(), lyrics };
	s.apply_lyrics(answer("spotify:track:b", Ok(None)));
	assert!(s.lyrics_state().is_none(), "the song moved on");
	s.apply_lyrics(answer("spotify:track:a", Ok(None)));
	assert!(matches!(s.lyrics_state(), Some(LyricsState::Missing)));
	s.apply_lyrics(answer("spotify:track:a", Err("offline".into())));
	assert!(matches!(s.lyrics_state(), Some(LyricsState::Failed)));
	s.update(Msg::RetryLyrics);
	assert!(s.lyrics_state().is_none(), "asked for again");
	s.item.uri = "spotify:track:b".into();
	s.apply_lyrics(answer("spotify:track:b", Ok(Some(Lyrics::Plain("words".into())))));
	assert!(matches!(s.lyrics_state(), Some(LyricsState::Found(Lyrics::Plain(_)))));
	s.item.uri = "spotify:track:c".into();
	assert!(s.lyrics_state().is_none(), "lyrics belong to their track");
}

fn queued(uid: &str, source: &str) -> QueueEntry {
	QueueEntry { uid: uid.into(), source: source.into(), item: Entity { uri: format!("spotify:track:{uid}"), ..Entity::default() } }
}

#[test]
fn a_jump_skips_to_the_song_and_keeps_the_playlist() {
	let mut s = playing(200_000, 10_000.0);
	s.volume = 50;
	s.upcoming = vec![queued("a", "queue"), queued("b", "context"), queued("c", "context")];
	assert_eq!(cmds(s.update(Msg::QueueJump(1))), [Command::SetVolume { volume: 0 }, Command::SkipNext, Command::SkipNext]);
	assert_eq!(s.volume, 50, "muted for the skips, not shown so");
	s.apply(Event::VolumeChanged { volume: 0 });
	assert_eq!(s.volume, 50);
	s.apply(Event::TrackChanged { item: Entity { uri: "spotify:track:b".into(), ..Entity::default() } });
	assert_eq!(s.queue_edit.take_commands(), [Command::SetVolume { volume: 50 }]);
	s.apply(Event::VolumeChanged { volume: 50 });
	// The next song needs no mute.
	s.queue_edit.settled();
	assert_eq!(cmds(s.update(Msg::QueueJump(0))), [Command::SkipNext]);
}

#[test]
fn playlist_details_prefill_and_save_changed_fields() {
	let mut s = with_library();
	s.library.route = Route::Playlist("p1".into());
	s.library.listing = Some(library::Listing {
		title: "Original".into(),
		description: Some("First line".into()),
		public: Some(false),
		owned: true,
		..library::Listing::default()
	});
	s.update(Msg::RenamePlaylist);
	assert_eq!(s.name_input, "Original");
	assert_eq!(s.description_input, "First line");
	s.update(Msg::Focus(Some(Field::PlaylistDescription)));
	s.update(Msg::Edit { from: 10, to: 10, text: "\nSecond line".into() });
	s.update(Msg::TogglePlaylistPublic);
	let effects = s.update(Msg::SaveRename);
	let expected = PlaylistChanges { name: None, description: Some("First line\nSecond line".into()), public: Some(true) };
	assert!(
		matches!(effects.as_slice(), [Effect::Library(library::Request::RenamePlaylist(id, changes))] if id == "p1" && *changes == expected)
	);
	let listing = s.library.listing.as_ref().unwrap();
	assert_eq!(listing.title, "Original");
	assert_eq!(listing.description.as_deref(), Some("First line\nSecond line"));
	assert_eq!(listing.public, Some(true));
	assert!(s.dialog.is_none());
	assert_eq!(s.focus, None);
	s.update(Msg::RenamePlaylist);
	s.description_input.clear();
	assert!(
		matches!(s.update(Msg::SaveRename).as_slice(), [Effect::Library(library::Request::RenamePlaylist(_, changes))] if changes.description.as_deref() == Some("") && changes.name.is_none() && changes.public.is_none())
	);
	assert_eq!(s.library.listing.as_ref().unwrap().description, None);
}

#[test]
fn playlist_name_enter_saves_and_empty_name_keeps_dialog_open() {
	let mut s = with_library();
	s.library.route = Route::Playlist("p1".into());
	s.library.listing = Some(library::Listing { title: "Original".into(), owned: true, ..library::Listing::default() });
	s.update(Msg::RenamePlaylist);
	s.name_input = "  ".into();
	assert!(s.update(Msg::Submit).is_empty());
	assert!(s.dialog.is_some());
	s.name_input = "Renamed".into();
	assert!(
		matches!(s.update(Msg::Submit).as_slice(), [Effect::Library(library::Request::RenamePlaylist(_, changes))] if changes.name.as_deref() == Some("Renamed") && changes.description.is_none() && changes.public.is_none())
	);
	s.update(Msg::RenamePlaylist);
	assert!(s.update(Msg::SaveRename).is_empty(), "unchanged details send no PUT");
	s.library.listing.as_mut().unwrap().owned = false;
	s.update(Msg::RenamePlaylist);
	assert!(s.dialog.is_none(), "collaborators cannot edit playlist details");
}
