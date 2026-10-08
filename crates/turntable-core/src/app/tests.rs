//! Tests of application state and the messages that transition it.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::{input::replace_utf16, *};
use crate::{
	engine::{Command, Entity, Event, Position, QueueEntry, Status},
	library::{self, Route},
	lyrics::{self, Lyrics},
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
fn prefetch_never_navigates_closes_controls_or_starts_playback() {
	let mut state = State { focus: Some(Field::Search), library_menu: true, ..State::default() };
	state.library.selected = Some(3);
	let route = Route::Album("album".into());
	let effects = state.update(Msg::Prefetch(route.clone()));
	assert!(matches!(effects.as_slice(), [Effect::Library(library::Request::Prefetch(known))] if known == &route));
	assert_eq!(state.library.route, Route::Home);
	assert_eq!(state.library.selected, Some(3));
	assert_eq!(state.focus, Some(Field::Search));
	assert!(state.library_menu);
	for route in [Route::Home, Route::Search, Route::Library, Route::Folder("folder".into())] {
		assert!(state.update(Msg::Prefetch(route)).is_empty());
	}
}

#[test]
fn absolute_seek_clamps_to_the_track() {
	let mut s = playing(240_000, 0.0);
	assert_eq!(cmds(s.update(Msg::Seek(120_000))), vec![Command::Seek { position_ms: 120_000 }]);
	assert_eq!(cmds(s.update(Msg::Seek(300_000))), vec![Command::Seek { position_ms: 240_000 }]);
	// Nothing to seek in without a duration.
	let mut idle = State::default();
	assert!(idle.update(Msg::Seek(3)).is_empty());
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
fn volume_clamps_and_mute_restores() {
	let mut s = State { volume: 98, ..State::default() };
	assert_eq!(cmds(s.update(Msg::Volume(120))), vec![Command::SetVolume { volume: 100 }]);
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
fn eq_band_steps_cover_every_half_db() {
	let mut state = State::default();
	for step in 0..=EQ_STEPS {
		let db = eq_step_db(step);
		assert_eq!(eq_db_step(db), step);
		assert_eq!(db, f32::from(step) * 0.5 - 12.0);
		assert!(matches!(state.update(Msg::EqBand(9, step)).as_slice(), [Effect::SetEq]));
		assert_eq!(state.eq.gains_db[9], db);
	}
	assert_eq!(eq_step_db(255), 12.0);
	assert_eq!(eq_db_step(f32::NAN), 24);
	for db in [-99.0, -3.75, -0.25, 0.25, 3.75, 99.0] {
		assert_eq!(eq_step_db(eq_db_step(db)), crate::audio::sanitize_gain_db(db));
	}
	assert!(state.update(Msg::EqBand(10, 0)).is_empty());
	assert_eq!(state.eq.gains_db[9], 12.0);
}

#[test]
fn typed_eq_gains_round_clamp_and_keep_an_unchanged_preset() {
	let mut state = State::default();
	state.update(Msg::EqPreset("rock"));
	state.update(Msg::EqSet(0, 3.75));
	assert_eq!(state.eq_preset.as_deref(), Some("rock"));
	state.update(Msg::EqEnabled(false));
	for (db, want) in [(3.5, 3.5), (-0.25, -0.5), (99.0, 12.0), (-99.0, -12.0), (f32::NAN, 0.0)] {
		assert!(matches!(state.update(Msg::EqSet(0, db)).as_slice(), [Effect::SetEq]));
		assert_eq!(state.eq.gains_db[0], want);
		assert!(state.eq.enabled);
		assert!(state.eq_preset.is_none());
	}
	let before = state.eq;
	assert!(state.update(Msg::EqSet(usize::MAX, 6.5)).is_empty());
	assert_eq!(state.eq, before);
}

#[test]
fn pause_freezes_the_local_clock() {
	let mut s = playing(200_000, 10_000.0);
	s.apply(Event::PlaybackChanged { status: Status::Paused });
	assert_eq!(s.anchor.speed, 0.0);
	let frozen = s.anchor.position_ms;
	s.anchor.at = Instant::now().checked_sub(Duration::from_secs(60)).unwrap();
	assert_eq!(s.position_ms(), frozen);
}

#[test]
fn position_sync_ignores_a_stale_timestamp() {
	let mut s = playing(200_000, 0.0);
	// Timestamp an hour in the past: trust arrival time, not the stale clock.
	let hour_ago = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as f64 - 3_600_000.0;
	s.apply(Event::PositionSync { position: Position { position_ms: 30_000.0, timestamp_ms: hour_ago, speed: 1.0 } });
	assert_eq!(s.anchor.position_ms, 30_000);
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
	let mut s = State::default();
	// Search, once connected: Enter searches now and drops the pending debounce.
	s.library.conn = library::Conn::Ready;
	s.library.route = Route::Search;
	s.focus = Some(Field::Search);
	s.update(Msg::SetText(Field::Search, "cigarettes".into()));
	let effects = s.update(Msg::Submit);
	assert!(matches!(
		effects.as_slice(),
		[Effect::CancelSearchSoon, Effect::Library(library::Request::Search(q))] if q == "cigarettes"
	));
}

#[test]
fn navigation_keeps_the_always_visible_host_search_field_focus() {
	let mut s = State::default();
	s.library.conn = library::Conn::Ready;
	s.library.route = Route::Search;
	s.focus = Some(Field::Search);
	s.update(Msg::Navigate(Route::Liked));
	assert_eq!(s.focus, Some(Field::Search), "navigation does not hide the GPUI host's search field");
}

/// Signed in, with a playlist in Your Library.
fn with_library() -> State {
	let mut s = State::default();
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
fn playing_a_row_loads_its_context_at_the_chosen_track() {
	for (route, uri) in [(Route::Playlist("p".into()), "spotify:playlist:p"), (Route::Album("a".into()), "spotify:album:a")] {
		let mut s = with_listing(route.clone(), Some(uri));
		let expected = PlayStart::Context { uri: uri.into(), offset_uri: Some("spotify:track:180".into()), position_ms: 0 };
		assert!(
			matches!(s.update(Msg::PlayRow(s.library.row_target(180).unwrap())).as_slice(), [Effect::Player(Command::Load(start))] if *start == expected)
		);
		assert_eq!(s.library.selected, Some(180));
		assert_eq!(s.played_from, Some(route));
	}
}

#[test]
fn playing_without_a_context_loads_all_tracks_and_preserves_the_offset() {
	for (route, context) in [(Route::Liked, None), (Route::Album("a".into()), None), (Route::Artist("r".into()), Some("spotify:artist:r"))]
	{
		let mut s = with_listing(route, context);
		let uris: Vec<String> = (0..250).map(|i| format!("spotify:track:{i}")).collect();
		let expected = PlayStart::Tracks { uris: uris.clone(), offset_uri: Some("spotify:track:180".into()) };
		assert!(
			matches!(s.update(Msg::PlayRow(s.library.row_target(180).unwrap())).as_slice(), [Effect::Player(Command::Load(start))] if *start == expected)
		);
		if context.is_none() {
			let expected = PlayStart::Tracks { uris, offset_uri: Some("spotify:track:0".into()) };
			assert!(matches!(s.update(Msg::PlayListing).as_slice(), [Effect::Player(Command::Load(start))] if *start == expected));
		}
	}
	let mut s = State::default();
	assert!(s.update(Msg::PlayRow(library::RowTarget { route: Route::Home, uri: "missing".into(), occurrence: None })).is_empty());
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
fn clearing_the_library_filter_empties_it_and_keeps_the_caret_there() {
	let mut s = with_library();
	s.library.filter = "road".into();
	s.update(Msg::ClearLibraryFilter);
	assert!(s.library.filter.is_empty());
	assert_eq!(s.focus, Some(Field::LibraryFilter), "ready to type again");
}

#[test]
fn typing_in_the_library_filter_filters_without_searching() {
	let mut s = with_library();
	s.update(Msg::Focus(Some(Field::LibraryFilter)));
	assert!(s.update(Msg::SetText(Field::LibraryFilter, "road".into())).is_empty(), "no search, no request");
	assert_eq!(s.library.filter, "road");
	s.update(Msg::SetText(Field::LibraryFilter, "roa".into()));
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
	s.update(Msg::Select(library::RowTarget { route: Route::Search, uri: "missing".into(), occurrence: None }));
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
fn a_jump_skips_to_the_occurrence_without_muting_or_restarting_intermediate_songs() {
	let mut s = playing(200_000, 10_000.0);
	s.volume = 50;
	s.upcoming = vec![queued("a", "queue"), queued("b", "context"), queued("c", "context")];
	assert_eq!(cmds(s.update(Msg::QueueJump(1))), [Command::EditQueue {
		revision: 1,
		edit:     crate::engine::QueueEdit::Jump { uid: "b".into() },
	}]);
	assert_eq!(s.volume, 50);
	assert_eq!(s.upcoming.first().map(|e| e.uid.as_str()), Some("c"));
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
	let effects = s.update(Msg::SaveRename);
	let expected = PlaylistChanges { name: None, description: Some("First line\nSecond line".into()), public: None };
	assert!(
		matches!(effects.as_slice(), [Effect::Library(library::Request::RenamePlaylist(id, changes))] if id == "p1" && *changes == expected)
	);
	let listing = s.library.listing.as_ref().unwrap();
	assert_eq!(listing.title, "Original");
	assert_eq!(listing.description.as_deref(), Some("First line\nSecond line"));
	assert_eq!(listing.public, Some(false));
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

#[test]
fn encoded_covers_keep_identity_and_missing_lookups_are_deduplicated() {
	use std::sync::Arc;
	let mut state = State::default();
	assert!(state.image(None).is_none());
	assert!(state.image(Some("")).is_none());
	assert!(state.image(Some("https://cover")).is_none());
	assert!(state.image(Some("https://cover")).is_none());
	assert_eq!(state.take_missing_images(), ["https://cover".to_owned()].into_iter().collect());
	assert!(state.take_missing_images().is_empty());
	state.cache_image("https://cover".into(), Cover { bytes: vec![1, 2, 3], mime: "image/jpeg".into() });
	let cover = state.image(Some("https://cover")).unwrap().clone();
	state.cache_image("https://cover".into(), Cover { bytes: vec![4, 5, 6], mime: "image/jpeg".into() });
	assert!(Arc::ptr_eq(state.image(Some("https://cover")).unwrap(), &cover));
	assert!(state.take_missing_images().is_empty());
}

#[test]
fn encoded_covers_evict_oldest_by_bytes_and_count_and_can_reload() {
	let mut state = State::default();
	for index in 0..20 {
		state.cache_image(format!("cover-{index}"), Cover { bytes: vec![0; 1024 * 1024], mime: "image/jpeg".into() });
	}
	assert_eq!(state.image_cache_usage(), (16, 16 * 1024 * 1024));
	assert!(state.image(Some("cover-0")).is_none());
	assert_eq!(state.take_missing_images(), ["cover-0".to_owned()].into_iter().collect());
	state.cache_image("cover-0".into(), Cover { bytes: vec![1], mime: "image/jpeg".into() });
	assert_eq!(state.image(Some("cover-0")).unwrap().bytes, [1]);
	assert!(state.image_cache_usage().1 <= 16 * 1024 * 1024);
	let mut state = State::default();
	for index in 0..600 {
		state.cache_image(format!("cover-{index}"), Cover { bytes: vec![0], mime: "image/jpeg".into() });
	}
	assert_eq!(state.image_cache_usage(), (512, 512));
	assert!(state.image(Some("cover-87")).is_none());
	assert!(state.image(Some("cover-88")).is_some());
}

#[test]
fn recently_used_encoded_covers_keep_identity_under_cache_pressure() {
	use std::sync::Arc;
	let mut state = State::default();
	state.cache_image("visible".into(), Cover { bytes: vec![1], mime: "image/jpeg".into() });
	let visible = state.image(Some("visible")).unwrap().clone();
	for index in 0..600 {
		assert!(Arc::ptr_eq(state.image(Some("visible")).unwrap(), &visible));
		state.cache_image(format!("cover-{index}"), Cover { bytes: vec![0], mime: "image/jpeg".into() });
	}
	assert_eq!(state.image_cache_usage(), (512, 512));
	assert!(Arc::ptr_eq(state.image(Some("visible")).unwrap(), &visible));
	assert!(state.image(Some("cover-0")).is_none());
}

fn owned_tracks(uris: &[&str]) -> Result<library::Response, library::added::IndexError> {
	let mut tracks: Vec<_> = uris.iter().map(|uri| library::added::TrackId::from_uri(uri)).collect::<Result<_, _>>()?;
	tracks.sort_unstable();
	tracks.dedup();
	Ok(library::Response::Added { tracks: tracks.into(), stats: None })
}

#[test]
fn added_merges_owned_playlists_liked_collection_and_now_playing() -> Result<(), library::added::IndexError> {
	let owned = "spotify:track:0000000000000000000001";
	let saved = "spotify:track:0000000000000000000002";
	let playing = "spotify:track:0000000000000000000003";
	let checked = "spotify:track:0000000000000000000004";
	let mut s = State::default();
	s.library.apply(owned_tracks(&[owned])?);
	s.library.apply(library::Response::LikedTracks(vec![library::added::TrackId::from_uri(saved)?].into()));
	s.library.apply(library::Response::Liked(checked.into(), true));
	s.item.uri = playing.into();
	s.library.liked = Some((playing.into(), true));
	for uri in [owned, saved, playing, checked] {
		assert!(s.added(uri), "membership source for {uri}");
	}
	assert!(!s.added("spotify:track:0000000000000000000005"));
	Ok(())
}

#[test]
fn added_keeps_owned_membership_after_unlike_and_drops_removed_playlists() -> Result<(), library::added::IndexError> {
	let uri = "spotify:track:0000000000000000000001";
	let mut s = State::default();
	s.item.uri = uri.into();
	s.library.apply(owned_tracks(&[uri])?);
	s.library.apply(library::Response::Liked(uri.into(), true));
	assert!(s.added(uri));
	s.library.apply(library::Response::Liked(uri.into(), false));
	assert_eq!(s.liked(), Some(false));
	assert!(s.added(uri), "unliking does not remove owned-playlist membership");
	s.library.apply(owned_tracks(&[])?);
	assert!(!s.added(uri));
	Ok(())
}

#[test]
fn added_retains_liked_page_knowledge_after_navigation_and_respects_unlike() -> Result<(), serde_json::Error> {
	let uri = "spotify:track:0000000000000000000001";
	let track = serde_json::from_value(serde_json::json!({ "uri": uri }))?;
	let mut s = State::default();
	s.library.apply(library::Response::Listing {
		route:   Route::Liked,
		offset:  0,
		listing: library::Listing { tracks: vec![track], ..library::Listing::default() },
	});
	s.library.navigate(Route::Library);
	assert!(s.added(uri));
	s.library.apply(library::Response::Liked(uri.into(), false));
	assert!(!s.added(uri), "an explicit unlike wins over an older Liked Songs page");
	Ok(())
}

#[test]
fn added_replaces_full_liked_membership_and_rejects_nontrack_uris() {
	let uri = "spotify:track:0000000000000000000001";
	let mut s = State::default();
	s.library.apply(library::Response::Liked(uri.into(), true));
	s.library.apply(library::Response::LikedTracks(std::sync::Arc::from([])));
	assert!(!s.added(uri), "a fresh full collection replaces older contains answers");
	for uri in ["", "not a URI", "spotify:episode:0000000000000000000001", "spotify:local:artist:album:track:180"] {
		s.item.uri = uri.into();
		s.library.liked = Some((uri.into(), true));
		assert!(!s.added(uri), "only canonical Spotify tracks have added membership");
	}
}

#[test]
fn added_respects_optimistic_unlike_over_full_liked_collection() -> Result<(), library::added::IndexError> {
	let uri = "spotify:track:0000000000000000000001";
	let mut s = State::default();
	s.library.apply(library::Response::LikedTracks(vec![library::added::TrackId::from_uri(uri)?].into()));
	assert!(s.added(uri));
	s.item.uri = uri.into();
	assert_eq!(s.liked(), Some(true), "now playing can reuse full collection membership without a contains request");
	s.update(Msg::ToggleLike);
	assert!(!s.added(uri), "optimistic removal wins while the worker's cached full collection still contains the URI");
	assert_eq!(s.liked(), Some(false));
	s.library.apply(owned_tracks(&[uri])?);
	assert!(s.added(uri), "playlist membership still wins over the cached unlike");
	Ok(())
}

#[test]
fn toggle_like_still_toggles_only_liked_songs_when_track_is_in_owned_playlist() -> Result<(), library::added::IndexError> {
	let uri = "spotify:track:0000000000000000000001";
	let mut s = State::default();
	s.item.uri = uri.into();
	s.library.apply(owned_tracks(&[uri])?);
	s.library.apply(library::Response::Liked(uri.into(), false));
	assert!(s.added(uri));
	assert!(matches!(s.update(Msg::ToggleLike).as_slice(), [Effect::Library(library::Request::SetLiked(track, true))] if track == uri));
	assert_eq!(s.liked(), Some(true));
	assert!(matches!(s.update(Msg::ToggleLike).as_slice(), [Effect::Library(library::Request::SetLiked(track, false))] if track == uri));
	assert_eq!(s.liked(), Some(false));
	assert!(s.added(uri));
	Ok(())
}

#[test]
fn row_liked_actions_are_explicit_and_optimistic_without_changing_now_playing() {
	let row = "spotify:track:0000000000000000000001";
	let now = "spotify:track:0000000000000000000002";
	let mut s = State::default();
	s.item.uri = now.into();
	s.library.apply(library::Response::Liked(now.into(), true));
	assert!(
		matches!(s.update(Msg::AddToLiked(row.into())).as_slice(), [Effect::Library(library::Request::SetLiked(track, true))] if track == row)
	);
	assert!(s.added(row));
	assert_eq!(s.liked(), Some(true));
	s.library.apply(library::Response::Liked(row.into(), true));
	assert_eq!(s.liked(), Some(true), "a row's reply retains cached now-playing membership");
	s.library.apply(library::Response::Liked(row.into(), false));
	assert_eq!(s.liked(), Some(true));
}

#[test]
fn playlist_row_edits_target_one_occurrence_and_apply_only_after_success() {
	let mut state = State::default();
	state.library.route = Route::Playlist("scratch".into());
	let track = |uri: &str| serde_json::from_value::<crate::spotify::Track>(serde_json::json!({ "uri": uri })).unwrap();
	state.library.listing = Some(library::Listing {
		editable: true,
		tracks: vec![track("a"), track("b"), track("a")],
		positions: vec![0, 1, 2],
		total: 3,
		plays: vec![10, 20, 30],
		..Default::default()
	});
	state.library.selected = Some(1);
	assert!(
		matches!(state.update(Msg::MoveRow(0, 2)).as_slice(), [Effect::Library(library::Request::MovePlaylistRow { id, from: 0, to: 2, source, target })] if id == "scratch" && source == "a" && target == "a")
	);
	assert_eq!(state.library.rows()[0].uri, "a", "wait for successful acknowledgment");
	state.library.apply(library::Response::Moved("scratch".into(), 0, 2));
	assert_eq!(state.library.rows().iter().map(|track| track.uri.as_str()).collect::<Vec<_>>(), ["b", "a", "a"]);
	assert_eq!(state.library.selected, Some(0));
	assert_eq!(state.library.listing.as_ref().unwrap().plays, [20, 30, 10]);
	assert!(
		matches!(state.update(Msg::RemoveRow(state.library.row_target(2).unwrap())).as_slice(), [Effect::Library(library::Request::RemoveFromPlaylist(id, uri, 2))] if id == "scratch" && uri == "a")
	);
	state.library.apply(library::Response::Removed("scratch".into(), "a".into(), 2));
	assert_eq!(state.library.rows().iter().map(|track| track.uri.as_str()).collect::<Vec<_>>(), ["b", "a"]);
	assert_eq!(state.library.listing.as_ref().unwrap().total, 2);
	assert_eq!(state.library.listing.as_ref().unwrap().plays, [20, 30]);
	assert!(state.update(Msg::MoveRow(0, 0)).is_empty());
	assert!(state.update(Msg::MoveRow(0, 99)).is_empty());
	state.library.listing.as_mut().unwrap().editable = false;
	assert!(state.update(Msg::MoveRow(0, 1)).is_empty());
	assert!(state.update(Msg::RemoveRow(state.library.row_target(0).unwrap())).is_empty());
}

#[test]
fn direct_playlist_add_uses_the_same_request_without_consuming_a_dialog() {
	let mut state = State::default();
	let playlist = serde_json::from_value(serde_json::json!({ "id": "scratch", "name": "Scratch", "owner": { "id": "me" } })).unwrap();
	state.library.playlists.push(playlist);
	state.dialog = Some(Dialog::AddToPlaylist { uris: vec!["picked".into()], label: "Picked".into() });
	let effects = state.update(Msg::AddToPlaylistDirect { playlist: "scratch".into(), uris: vec!["result".into()] });
	assert!(
		matches!(effects.as_slice(), [Effect::Library(library::Request::AddToPlaylist(id, name, uris))] if id == "scratch" && name == "Scratch" && uris == &["result"])
	);
	assert!(matches!(&state.dialog, Some(Dialog::AddToPlaylist { uris, .. }) if uris == &["picked"]));
	let dialog_effects = state.update(Msg::AddToPlaylist(Some("scratch".into())));
	assert!(
		matches!(dialog_effects.as_slice(), [Effect::Library(library::Request::AddToPlaylist(id, name, uris))] if id == "scratch" && name == "Scratch" && uris == &["picked"])
	);
}

#[test]
fn host_search_and_finder_text_edits_do_not_depend_on_route_or_focus() {
	for route in [Route::Home, Route::Playlist("playlist".into())] {
		let mut state = with_library();
		state.library.route = route.clone();
		state.focus = None;
		assert!(state.field_visible(Field::Search));
		assert!(matches!(state.update(Msg::SetText(Field::Search, "find\nthis".into())).as_slice(), [Effect::SearchSoon]));
		assert_eq!(state.library.query, "findthis");
		assert_eq!(state.library.route, route);
		assert_eq!(state.focus, None);
		assert!(state.update(Msg::SetText(Field::Search, "findthis".into())).is_empty());
	}
}

#[test]
fn opening_a_track_keeps_reveal_through_same_route_navigation_and_partial_answers() {
	let mut state = with_library();
	let route = Route::Album("album".into());
	assert!(matches!(state.update(Msg::OpenTrack { album: "album".into(), uri: "song".into() }).as_slice(),
		[Effect::Library(library::Request::Open(wanted, 0))] if wanted == &route));
	assert!(state.update(Msg::Navigate(route.clone())).is_empty());
	assert_eq!(state.library.reveal.as_deref(), Some("song"));
	state.library.apply(library::Response::Listing {
		route:   route.clone(),
		offset:  0,
		listing: library::Listing { loading: true, ..Default::default() },
	});
	assert_eq!(state.library.reveal.as_deref(), Some("song"));
	let track = serde_json::from_value(serde_json::json!({"uri": "song"})).unwrap();
	state.library.apply(library::Response::Listing {
		route,
		offset: 0,
		listing: library::Listing { tracks: vec![track], ..Default::default() },
	});
	assert_eq!(state.library.selected, Some(0));
	assert!(state.library.show_selection);
	assert!(state.library.reveal.is_none());
	assert!(state.update(Msg::OpenTrack { album: "album".into(), uri: "song".into() }).is_empty());
	assert_eq!(state.library.selected, Some(0));
}

#[test]
fn continuation_pages_append_only_once_and_keep_loading_across_first_page_refresh() {
	let route = Route::Playlist("playlist".into());
	let track = |uri: &str| serde_json::from_value(serde_json::json!({"uri": uri})).unwrap();
	let listing = |uri, next_offset| library::Listing { tracks: vec![track(uri)], next_offset, ..Default::default() };
	let mut state = State::default();
	state.library.route = route.clone();
	state.library.apply(library::Response::Listing { route: route.clone(), offset: 0, listing: listing("cached", Some(50)) });
	assert!(matches!(state.update(Msg::LoadMore).as_slice(), [Effect::Library(library::Request::Open(_, 50))]));
	state.library.apply(library::Response::Listing {
		route:   route.clone(),
		offset:  0,
		listing: library::Listing { loading: true, ..Default::default() },
	});
	assert_eq!(state.library.pending_offset, Some(50), "a partial refreshed header does not cancel the awaited page");
	assert!(state.update(Msg::LoadMore).is_empty());
	state.library.apply(library::Response::Listing { route: route.clone(), offset: 0, listing: listing("fresh", Some(50)) });
	assert!(state.library.listing.as_ref().unwrap().loading);
	assert!(state.update(Msg::LoadMore).is_empty());
	state.library.apply(library::Response::Listing { route: route.clone(), offset: 100, listing: listing("stale", None) });
	assert!(state.library.listing.as_ref().unwrap().loading);
	state.library.apply(library::Response::Listing { route: route.clone(), offset: 50, listing: listing("more", Some(100)) });
	state.library.apply(library::Response::Listing { route: route.clone(), offset: 50, listing: listing("duplicate", Some(100)) });
	state.library.apply(library::Response::Listing { route, offset: 100, listing: listing("unsolicited", None) });
	assert_eq!(state.library.rows().iter().map(|row| row.uri.as_str()).collect::<Vec<_>>(), ["fresh", "more"]);
	assert!(!state.library.listing.as_ref().unwrap().loading);
}

#[test]
fn playlist_edits_use_raw_positions_across_missing_rows_and_reject_legacy_positions() {
	let track = |uri: &str| serde_json::from_value(serde_json::json!({"uri": uri})).unwrap();
	let mut state = State::default();
	state.library.route = Route::Playlist("playlist".into());
	state.library.listing = Some(library::Listing {
		editable: true,
		tracks: vec![track("a"), track("b"), track("c")],
		positions: vec![10, 12, 14],
		..Default::default()
	});
	assert!(
		matches!(state.update(Msg::RemoveRow(state.library.row_target(1).unwrap())).as_slice(), [Effect::Library(library::Request::RemoveFromPlaylist(_, uri, 12))] if uri == "b")
	);
	assert!(
		matches!(state.update(Msg::MoveRow(0, 2)).as_slice(), [Effect::Library(library::Request::MovePlaylistRow { from: 10, to: 14, source, target, .. })] if source == "a" && target == "c")
	);
	state.library.apply(library::Response::Moved("playlist".into(), 10, 14));
	assert_eq!(state.library.rows().iter().map(|row| row.uri.as_str()).collect::<Vec<_>>(), ["b", "c", "a"]);
	assert_eq!(state.library.listing.as_ref().unwrap().positions, [11, 13, 14]);
	state.library.apply(library::Response::Removed("playlist".into(), "b".into(), 11));
	assert_eq!(state.library.listing.as_ref().unwrap().positions, [12, 13]);
	state.library.listing.as_mut().unwrap().positions.clear();
	assert!(state.update(Msg::RemoveRow(state.library.row_target(0).unwrap())).is_empty());
	assert!(state.update(Msg::MoveRow(0, 1)).is_empty());
}

#[test]
fn failed_lyrics_retry_with_exponential_backoff_and_success_stops_retrying() {
	let mut state = State::default();
	state.item.uri = "spotify:track:a".into();
	let failure = || lyrics::Response { uri: "spotify:track:a".into(), lyrics: Err("offline".into()) };
	state.apply_lyrics(failure());
	let first = state.lyrics_retry_at.unwrap();
	assert!(!state.lyrics_due(Instant::now()));
	assert!(state.lyrics_due(first));
	state.apply_lyrics(failure());
	assert!(state.lyrics_retry_at.unwrap().duration_since(first) >= Duration::from_secs(5));
	for _ in 0..20 {
		state.apply_lyrics(failure());
	}
	assert!(state.lyrics_retry_at.unwrap().duration_since(Instant::now()) <= Duration::from_secs(300));
	state.apply_lyrics(lyrics::Response { uri: "spotify:track:a".into(), lyrics: Ok(None) });
	assert!(state.lyrics_retry_at.is_none());
	assert!(!state.lyrics_due(Instant::now() + Duration::from_secs(600)));
	state.item.uri = "spotify:track:b".into();
	assert!(state.lyrics_due(Instant::now()));
}

#[test]
fn empty_window_title_uses_the_application_name() {
	assert_eq!(State::default().window_title(), "Turntable");
}

#[test]
fn row_actions_resolve_reordered_tracks_and_ignore_removed_or_wrong_route_targets() {
	let mut state = with_listing(Route::Liked, None);
	let target = state.library.row_target(180).unwrap();
	state.library.listing.as_mut().unwrap().tracks.swap(0, 180);
	state.update(Msg::Select(target.clone()));
	assert_eq!(state.library.selected, Some(0));
	assert!(matches!(state.update(Msg::PlayRow(target.clone())).as_slice(),
		[Effect::Player(Command::Load(PlayStart::Tracks { offset_uri: Some(uri), .. }))] if uri == &target.uri));
	state.update(Msg::AddRowToPlaylist(target.clone()));
	assert!(matches!(&state.dialog, Some(Dialog::AddToPlaylist { uris, .. }) if uris == std::slice::from_ref(&target.uri)));
	for action in [Msg::QueueRow(target.clone()), Msg::QueuePlayNext(target.clone())] {
		assert!(matches!(state.update(action).as_slice(), [Effect::Player(Command::EditQueue { .. })]));
		assert!(state.upcoming.iter().all(|entry| entry.item.uri == target.uri));
	}
	state.dialog = None;
	state.library.listing.as_mut().unwrap().tracks.remove(0);
	let selection = state.library.selected;
	for action in [
		Msg::Select(target.clone()),
		Msg::PlayRow(target.clone()),
		Msg::AddRowToPlaylist(target.clone()),
		Msg::QueueRow(target.clone()),
		Msg::QueuePlayNext(target.clone()),
		Msg::RemoveRow(target),
	] {
		assert!(state.update(action).is_empty());
		assert_eq!(state.library.selected, selection);
		assert!(state.dialog.is_none());
	}
	let other_target = state.library.row_target(0).unwrap();
	state.library.route = Route::Album("other".into());
	assert!(state.update(Msg::PlayRow(other_target)).is_empty(), "same URI on another route cannot consume a stale click");
}

#[test]
fn playlist_row_targets_reject_replaced_raw_occurrences_and_disambiguate_duplicates() {
	let track = |uri: &str| serde_json::from_value(serde_json::json!({"uri": uri})).unwrap();
	let mut state = State::default();
	state.library.route = Route::Playlist("playlist".into());
	state.library.listing = Some(library::Listing {
		editable: true,
		tracks: vec![track("a"), track("b"), track("a")],
		positions: vec![10, 12, 14],
		..Default::default()
	});
	let second_a = state.library.row_target(2).unwrap();
	assert!(matches!(state.update(Msg::RemoveRow(second_a.clone())).as_slice(),
		[Effect::Library(library::Request::RemoveFromPlaylist(_, uri, 14))] if uri == "a"));
	let stale = state.library.row_target(1).unwrap();
	state.library.listing.as_mut().unwrap().tracks.swap(0, 1);
	for action in [
		Msg::Select(stale.clone()),
		Msg::PlayRow(stale.clone()),
		Msg::QueueRow(stale.clone()),
		Msg::QueuePlayNext(stale.clone()),
		Msg::AddRowToPlaylist(stale.clone()),
		Msg::RemoveRow(stale),
	] {
		assert!(state.update(action).is_empty());
	}
	state.library.listing.as_mut().unwrap().positions.clear();
	assert_eq!(state.library.resolve_row(&second_a), None);
	assert_eq!(state.library.resolve_row(&state.library.row_target(2).unwrap()), None, "legacy duplicate has no occurrence identity");
}

#[test]
fn stale_duplicate_row_cannot_remove_the_next_identical_occurrence() {
	let track = || serde_json::from_value(serde_json::json!({"uri": "spotify:track:a"})).unwrap();
	let mut state = State::default();
	state.library.route = Route::Playlist("playlist".into());
	state.library.listing =
		Some(library::Listing { editable: true, tracks: vec![track(), track()], positions: vec![0, 1], ..Default::default() });
	let stale = state.library.row_target(0).unwrap();
	state.library.apply(library::Response::Removed("playlist".into(), "spotify:track:a".into(), 0));
	assert_eq!(state.library.rows().len(), 1);
	assert_eq!(state.library.listing.as_ref().unwrap().positions, [0]);
	assert!(state.update(Msg::RemoveRow(stale.clone())).is_empty());
	assert!(state.update(Msg::PlayRow(stale)).is_empty());
	assert!(!state.update(Msg::RemoveRow(state.library.row_target(0).unwrap())).is_empty(), "newly rendered duplicate remains actionable");
}
