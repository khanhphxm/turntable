//! Portable GPUI TestPlatform journeys. The real Shell, input handlers, context menus,
//! dialog hitboxes and prepaint/paint pipeline run without native windows, GPU or Spotify.

#[path = "cache_tests.rs"]
mod caching;

use gpui_kit::{AppContext as _, Entity, KeyDownEvent, Keystroke, Modifiers, MouseButton, component::Root, point, px, size};
use gpui_test::{TestAppContext, VisualTestContext};
use turntable_core::{
	Controller, Dialog, Msg, Panel, Paths, Route, State,
	engine::Entity as TrackEntity,
	library::Response,
	spotify::{Playlist, User},
};

use super::{FullView, Page, Shell};

fn fixture_state() -> State {
	let mut state = State::default();
	state.signed_in = true;
	state.panel = Some(Panel::NowPlaying);
	let me = User { id: "fixture".into(), display_name: Some("Fixture account".into()) };
	state.library.apply(Response::Connected {
		me:        me.clone(),
		playlists: vec![Playlist {
			id:            "playlist".into(),
			uri:           "spotify:playlist:playlist".into(),
			name:          "Fixture playlist".into(),
			description:   None,
			public:        Some(false),
			owner:         me,
			images:        Vec::new(),
			collaborative: false,
			track_count:   Some(1),
		}],
	});
	state.item = TrackEntity { uri: "spotify:track:0000000000000000000001".into(), ..TrackEntity::default() };
	state.item.decorations.identity.name = "Fixture song".into();
	state
}

fn setup() -> (Entity<Shell>, VisualTestContext, tempfile::TempDir) {
	setup_with_state(fixture_state())
}

fn setup_with_state(state: State) -> (Entity<Shell>, VisualTestContext, tempfile::TempDir) {
	let directory = tempfile::tempdir().unwrap();
	let paths = Paths { config: directory.path().join("config"), cache: directory.path().join("cache") };
	let controller = Controller::fixture(&paths, state).unwrap();
	let mut app = TestAppContext::single();
	app.update(|cx| {
		crate::theme::init(cx);
		super::bind_keys(cx);
	});
	let mut shell = None;
	let window = app.open_window(size(px(1280.), px(800.)), |window, cx| {
		let entity = cx.new(|cx| Shell::new(controller, window, cx));
		shell = Some(entity.clone());
		Root::new(entity, window, cx)
	});
	let shell = shell.unwrap();
	let mut cx = VisualTestContext::from_window(window.into(), &app);
	cx.update(|window, cx| {
		let focus = shell.read(cx).focus.clone();
		window.focus(&focus, cx);
	});
	frame(&mut cx);
	(shell, cx, directory)
}

fn frame(cx: &mut VisualTestContext) {
	cx.update(|window, cx| {
		window.draw(cx).clear(cx);
	});
	cx.run_until_parked();
}

#[test]
fn proxy_settings_work_before_sign_in_and_reject_invalid_drafts() {
	let (shell, mut cx, _directory) = setup();
	cx.update(|_, cx| {
		shell.update(cx, |shell, cx| {
			shell.send(Msg::SignOut, cx);
			shell.open(Page::Settings, cx);
		});
	});
	frame(&mut cx);
	cx.update(|window, cx| {
		let input = shell.read(cx).proxy_input.clone();
		input.update(cx, |input, cx| {
			input.set_value("socks5://host:1080", window, cx);
			input.focus(window, cx);
		});
	});
	frame(&mut cx);
	cx.simulate_keystrokes("enter");
	frame(&mut cx);
	cx.read_entity(&shell, |shell, cx| {
		assert_eq!(shell.page, Page::Settings);
		assert!(!shell.st().signed_in);
		assert!(shell.st().prefs.network.proxy.is_empty());
		assert!(turntable_core::net::proxy::parse(&shell.proxy_input.read(cx).value()).is_err());
	});
	cx.update(|window, cx| {
		let input = shell.read(cx).proxy_input.clone();
		input.update(cx, |input, cx| input.set_value("http://user:pass@localhost:8080", window, cx));
	});
	cx.simulate_keystrokes("enter");
	frame(&mut cx);
	cx.read_entity(&shell, |shell, _| {
		assert_eq!(shell.st().prefs.network.proxy, "http://user:pass@localhost:8080");
		assert!(!shell.st().starting, "changing a proxy before sign-in must not authenticate");
	});
	cx.update(|_, cx| shell.update(cx, |shell, cx| shell.send(Msg::Pref(turntable_core::app::settings::Pref::Proxy(String::new())), cx)));
}

#[test]
fn search_typing_stays_on_the_current_page_and_enter_navigates() {
	let (shell, mut cx, _directory) = setup();
	cx.update(|window, cx| {
		let input = shell.read(cx).search.clone();
		input.update(cx, |input, cx| input.focus(window, cx));
	});
	cx.simulate_keystrokes("f i x t u r e");
	frame(&mut cx);
	cx.read_entity(&shell, |shell, _| {
		assert_eq!(shell.page, Page::Core(Route::Home));
		assert_eq!(shell.st().library.route, Route::Home);
		assert_eq!(shell.st().library.query, "fixture");
		assert!(shell.search_open);
	});
	// simulate_keystrokes also inserts an IME newline after a propagated Enter; a native
	// single-line submit is a key event, not another text edit that reopens suggestions.
	cx.simulate_event(KeyDownEvent {
		keystroke:              Keystroke::parse("enter").unwrap(),
		is_held:                false,
		prefer_character_input: false,
	});
	frame(&mut cx);
	cx.read_entity(&shell, |shell, _| {
		assert_eq!(shell.page, Page::Core(Route::Search));
		assert_eq!(shell.st().library.route, Route::Search);
		assert!(!shell.search_open);
	});
}

#[test]
fn right_clicking_a_library_row_opens_a_menu_without_navigating() {
	let (shell, mut cx, _directory) = setup();
	cx.simulate_mouse_move(point(px(80.), px(226.)), None, Modifiers::none());
	frame(&mut cx);
	cx.simulate_mouse_down(point(px(80.), px(226.)), MouseButton::Right, Modifiers::none());
	frame(&mut cx);
	cx.read_entity(&shell, |shell, _| {
		assert!(shell.menu.is_some(), "the library row must open its rendered context menu");
		assert_eq!(shell.page, Page::Core(Route::Home));
	});
	cx.simulate_keystrokes("escape");
	frame(&mut cx);
	cx.read_entity(&shell, |shell, _| assert!(shell.menu.is_none()));
}

#[test]
fn escape_exits_expanded_now_playing_after_the_header_expand_click() {
	let (shell, mut cx, _directory) = setup();
	cx.simulate_mouse_move(point(px(1248.), px(92.)), None, Modifiers::none());
	frame(&mut cx);
	cx.simulate_click(point(px(1248.), px(92.)), Modifiers::none());
	frame(&mut cx);
	cx.read_entity(&shell, |shell, _| assert_eq!(shell.full_view, FullView::Expanded));
	cx.simulate_keystrokes("escape");
	frame(&mut cx);
	cx.read_entity(&shell, |shell, _| {
		assert_eq!(shell.full_view, FullView::Off);
		assert_eq!(shell.page, Page::Core(Route::Home));
	});
}

#[test]
fn clicking_dialog_padding_keeps_it_open_but_the_outside_scrim_closes_it() {
	let (shell, mut cx, _directory) = setup();
	cx.update(|_, cx| shell.update(cx, |shell, cx| shell.send(Msg::AddNowPlayingToPlaylist, cx)));
	frame(&mut cx);
	cx.read_entity(&shell, |shell, _| assert!(matches!(shell.st().dialog, Some(Dialog::AddToPlaylist { .. }))));
	// The centered card's title/padding has no action. It must occlude the full-window scrim.
	cx.simulate_click(point(px(640.), px(310.)), Modifiers::none());
	frame(&mut cx);
	cx.read_entity(&shell, |shell, _| assert!(matches!(shell.st().dialog, Some(Dialog::AddToPlaylist { .. }))));
	// Keep the outside click below native titlebar drag regions, which TestPlatform cannot move.
	cx.simulate_click(point(px(20.), px(500.)), Modifiers::none());
	frame(&mut cx);
	cx.read_entity(&shell, |shell, _| assert!(shell.st().dialog.is_none()));
}

#[test]
fn hovering_library_and_right_panel_headers_keeps_layout_across_painted_frames() {
	let (shell, mut cx, _directory) = setup();
	let layout = cx.read_entity(&shell, |shell, _| (shell.sidebar_w(), shell.main_w, shell.right_w()));
	let points = [point(px(1050.), px(92.)), point(px(48.), px(92.)), point(px(640.), px(500.))];
	for _ in 0..4 {
		for position in points {
			cx.simulate_mouse_move(position, None, Modifiers::none());
			frame(&mut cx);
			// Draw another frame at the same position as well as enter/leave frames. This
			// exercises retained elements and the exact prepaint-before-paint regression.
			frame(&mut cx);
			cx.read_entity(&shell, |shell, _| {
				assert_eq!((shell.sidebar_w(), shell.main_w, shell.right_w()), layout);
				assert_eq!(shell.page, Page::Core(Route::Home));
				assert_eq!(shell.full_view, FullView::Off);
			});
		}
	}
}

mod scrolling {
	use std::rc::Rc;

	use gpui_kit::{AppContext as _, Entity, Modifiers, MouseButton, ScrollDelta, ScrollWheelEvent, TouchPhase, point, px};
	use gpui_test::VisualTestContext;
	use turntable_core::{
		Msg, Route,
		library::{Folder, Kind, Listing, Request, Response, RootEntry},
		spotify::Track,
	};

	use super::{Shell, frame, setup};

	fn tracks(range: std::ops::Range<usize>) -> Vec<Track> {
		range
			.map(|i| Track {
				uri:         format!("spotify:track:{i:022}"),
				name:        format!("Scroll song {i}"),
				duration_ms: 180_000,
				disc_number: 1,
				explicit:    false,
				artists:     Vec::new(),
				album:       None,
				is_local:    false,
				is_episode:  false,
			})
			.collect()
	}

	fn listing(shell: &Entity<Shell>, cx: &mut VisualTestContext, rows: usize, next_offset: Option<u32>) -> Route {
		let route = Route::Playlist("playlist".into());
		cx.update(|_, cx| {
			shell.update(cx, |shell, cx| {
				shell.go(route.clone(), cx);
				shell
					.core
					.fixture_response(Response::Listing {
						route:   route.clone(),
						offset:  0,
						listing: Listing {
							title: "Scroll playlist".into(),
							tracks: tracks(0..rows),
							total: 200,
							next_offset,
							..Listing::default()
						},
					})
					.unwrap();
				shell.pump(cx);
			});
		});
		frame(cx);
		route
	}

	#[test]
	fn appended_playlist_page_keeps_the_visible_row_and_fractional_offset() {
		let (shell, mut cx, _directory) = setup();
		let route = listing(&shell, &mut cx, 100, Some(100));
		cx.update(|_, cx| {
			shell.update(cx, |shell, cx| {
				shell.list.scroll_to(gpui_kit::ListOffset { item_ix: 40, offset_in_item: px(11.5) });
				cx.notify();
			});
		});
		frame(&mut cx);
		let before = cx.read_entity(&shell, |shell, _| shell.list.logical_scroll_top());
		cx.update(|_, cx| {
			shell.update(cx, |shell, cx| {
				shell.send(Msg::LoadMore, cx);
				shell
					.core
					.fixture_response(Response::Listing {
						route,
						offset: 100,
						listing: Listing { tracks: tracks(100..150), total: 150, ..Listing::default() },
					})
					.unwrap();
				shell.pump(cx);
			});
		});
		frame(&mut cx);
		cx.read_entity(&shell, |shell, _| {
			assert_eq!(shell.list.item_count(), 152);
			let after = shell.list.logical_scroll_top();
			assert_eq!((after.item_ix, after.offset_in_item), (before.item_ix, before.offset_in_item));
		});
	}

	#[test]
	fn pixel_wheel_packets_keep_subpixel_motion_and_wheel_notches_ease_in() {
		let (shell, mut cx, _directory) = setup();
		listing(&shell, &mut cx, 100, None);
		cx.update(|_, cx| {
			shell.update(cx, |shell, cx| {
				shell.list.scroll_to(gpui_kit::ListOffset { item_ix: 20, offset_in_item: px(10.) });
				cx.notify();
			});
		});
		frame(&mut cx);
		for _ in 0..8 {
			cx.simulate_event(ScrollWheelEvent {
				position: point(px(700.), px(420.)),
				delta: ScrollDelta::Pixels(point(px(0.), px(-0.5))),
				touch_phase: TouchPhase::Moved,
				..Default::default()
			});
			frame(&mut cx);
		}
		let offset = |cx: &mut VisualTestContext| cx.read_entity(&shell, |shell, _| shell.list.logical_scroll_top().offset_in_item);
		assert_eq!(offset(&mut cx), px(14.));
		// A wheel notch (one 20 px line) eases in over a few frames instead of jumping.
		cx.simulate_event(ScrollWheelEvent {
			position: point(px(700.), px(420.)),
			delta: ScrollDelta::Lines(point(0., -1.)),
			..Default::default()
		});
		cx.executor().advance_clock(std::time::Duration::from_millis(8));
		frame(&mut cx);
		let first = offset(&mut cx);
		assert!(first > px(14.) && first < px(34.), "the first frame moves part of the notch: {first:?}");
		for _ in 0..100 {
			if (offset(&mut cx) - px(34.)).abs() < px(0.01) {
				break;
			}
			cx.executor().advance_clock(std::time::Duration::from_millis(8));
			cx.update(|window, cx| {
				window.simulate_next_frame(cx);
			});
			frame(&mut cx);
		}
		let landed = offset(&mut cx);
		assert!((landed - px(34.)).abs() < px(0.01), "the whole notch lands: {landed:?}");
	}

	#[test]
	fn sidebar_rows_are_shared_across_scroll_frames_and_invalidated_by_library_changes() {
		let (shell, mut cx, _directory) = setup();
		cx.update(|_, cx| {
			shell.update(cx, |shell, cx| {
				let me = shell.st().library.me.clone().unwrap();
				let template = shell.st().library.playlists.first().unwrap();
				let playlists = (0..100)
					.map(|i| {
						let mut playlist = template.clone();
						playlist.id = format!("scroll-{i}");
						playlist.uri = format!("spotify:playlist:scroll-{i}");
						playlist.name = format!("Scroll playlist {i}");
						playlist
					})
					.collect();
				shell.core.fixture_response(Response::Connected { me, playlists }).unwrap();
				shell.pump(cx);
			});
		});
		frame(&mut cx);
		let rows = cx.read_entity(&shell, |shell, _| Rc::clone(shell.library_rows.as_ref().unwrap()));
		for _ in 0..8 {
			cx.simulate_event(ScrollWheelEvent {
				position: point(px(150.), px(420.)),
				delta: ScrollDelta::Pixels(point(px(0.), px(-5.))),
				..Default::default()
			});
			frame(&mut cx);
		}
		cx.read_entity(&shell, |shell, _| assert!(Rc::ptr_eq(&rows, shell.library_rows.as_ref().unwrap())));
		cx.update(|_, cx| shell.update(cx, |shell, cx| shell.send(Msg::LibraryKind(Kind::Album), cx)));
		frame(&mut cx);
		cx.read_entity(&shell, |shell, _| {
			let changed = shell.library_rows.as_ref().unwrap();
			assert!(!Rc::ptr_eq(&rows, changed));
			assert!(changed.is_empty());
		});
	}

	#[test]
	fn library_menu_collects_folders_at_right_click_not_when_the_row_was_drawn() {
		use crate::menu::{Action, Item};

		let (shell, mut cx, _directory) = setup();
		cx.simulate_mouse_move(point(px(80.), px(226.)), None, Modifiers::none());
		frame(&mut cx);
		cx.update(|_, cx| {
			shell.update(cx, |shell, _| {
				// Deliver between drawing a row and handling its click, before another host render.
				shell
					.core
					.fixture_response(Response::Rootlist {
						account: "fixture".into(),
						entries: vec![RootEntry::Folder(Folder {
							id:       "fresh".into(),
							name:     "Fresh folder".into(),
							count:    0,
							children: Vec::new(),
						})],
					})
					.unwrap();
				assert!(shell.core.pump());
			});
		});
		cx.simulate_mouse_down(point(px(80.), px(226.)), MouseButton::Right, Modifiers::none());
		cx.read_entity(&shell, |shell, _| {
			let menu = shell.menu.as_ref().unwrap();
			assert!(menu.items.iter().any(|item| matches!(item,
				Item::Row { label, action: Action::Sub(folders), .. }
					if label == "Move to folder" && folders.iter().any(|folder| matches!(folder, Item::Row { label, .. } if label == "Fresh folder"))
			)));
		});
	}

	#[test]
	fn visible_footer_schedules_one_continuation_and_draws_do_not_repeat_it() {
		let (shell, mut cx, _directory) = setup();
		let route = listing(&shell, &mut cx, 1, Some(1));
		let before_frame = cx.read_entity(&shell, |shell, _| shell.core.fixture_requests().unwrap());
		assert!(!before_frame.iter().any(|request| matches!(request, Request::Open(r, 1) if *r == route)));
		cx.read_entity(&shell, |shell, _| assert!(!shell.st().library.listing.as_ref().unwrap().loading));
		cx.update(|window, cx| {
			assert!(window.simulate_next_frame(cx) > 0);
		});
		let requests = cx.read_entity(&shell, |shell, _| shell.core.fixture_requests().unwrap());
		assert_eq!(requests.iter().filter(|request| matches!(request, Request::Open(r, 1) if *r == route)).count(), 1);
		for _ in 0..4 {
			frame(&mut cx);
		}
		cx.read_entity(&shell, |shell, _| {
			assert!(shell.core.fixture_requests().unwrap().is_empty());
			assert!(shell.st().library.listing.as_ref().unwrap().loading);
		});
	}
}

mod shortcuts {
	use gpui_kit::Focusable as _;

	use super::*;

	#[test]
	fn focused_search_types_space_and_m_without_playback_or_mute() {
		let (shell, mut cx, _directory) = setup();
		// Start audible: M at zero volume correctly unmutes instead of muting.
		cx.update(|_, cx| shell.update(cx, |shell, cx| shell.send(Msg::Volume(75), cx)));
		let before = cx.read_entity(&shell, |shell, _| (shell.st().status, shell.st().volume));
		cx.update(|window, cx| {
			let input = shell.read(cx).search.clone();
			input.update(cx, |input, cx| input.focus(window, cx));
		});
		frame(&mut cx);
		cx.simulate_keystrokes("a space m");
		frame(&mut cx);
		cx.read_entity(&shell, |shell, cx| {
			assert_eq!(shell.search.read(cx).value(), "a m");
			assert_eq!(shell.st().library.query, "a m");
			assert_eq!((shell.st().status, shell.st().volume), before);
		});
		// An explicitly dispatched character action has the same focus guard.
		cx.update(|window, cx| shell.update(cx, |shell, cx| shell.character_shortcut(Msg::TogglePlay, window, cx)));
		cx.read_entity(&shell, |shell, _| assert_eq!(shell.st().status, before.0));
		cx.update(|window, cx| {
			let focus = shell.read(cx).focus.clone();
			window.focus(&focus, cx);
		});
		frame(&mut cx);
		cx.update(|window, cx| assert!(shell.read(cx).focus.is_focused(window)));
		cx.simulate_keystrokes("m");
		frame(&mut cx);
		cx.read_entity(&shell, |shell, _| assert_eq!(shell.st().volume, 0));
		cx.read_entity(&shell, |shell, _| assert!(shell.st().toast.is_none()));
		cx.simulate_keystrokes("space");
		frame(&mut cx);
		// The offline fixture rejects transport without an engine, showing its error
		// rather than leaving optimistic playback running. This proves dispatch only.
		cx.read_entity(&shell, |shell, _| assert!(shell.st().toast.is_some(), "Space outside inputs must dispatch transport"));
	}

	#[test]
	fn spotify_navigation_filter_volume_and_help_shortcuts_reach_the_shell() {
		let (shell, mut cx, _directory) = setup();
		cx.simulate_keystrokes(super::super::primary("cmd-shift-l", "ctrl-shift-l"));
		frame(&mut cx);
		cx.read_entity(&shell, |shell, _| assert_eq!(shell.page, Page::Core(Route::Search)));
		cx.simulate_keystrokes(super::super::primary("cmd-f", "ctrl-f"));
		frame(&mut cx);
		cx.update(|window, cx| {
			let input = shell.read(cx).library_filter.as_ref().unwrap().clone();
			assert!(input.read(cx).focus_handle(cx).is_focused(window));
		});
		cx.simulate_keystrokes("f space m");
		frame(&mut cx);
		cx.read_entity(&shell, |shell, _| assert_eq!(shell.st().library.filter, "f m"));
		cx.simulate_keystrokes(super::super::primary("cmd-/", "ctrl-/"));
		frame(&mut cx);
		cx.read_entity(&shell, |shell, _| assert!(shell.shortcuts_open));
		cx.simulate_keystrokes("escape");
		frame(&mut cx);
		cx.read_entity(&shell, |shell, _| assert!(!shell.shortcuts_open));
		cx.update(|window, cx| {
			let focus = shell.read(cx).focus.clone();
			window.focus(&focus, cx);
		});
		frame(&mut cx);
		cx.simulate_keystrokes("alt-shift-s");
		frame(&mut cx);
		cx.read_entity(&shell, |shell, _| assert_eq!(shell.page, Page::Core(Route::Liked)));
		cx.simulate_keystrokes("alt-shift-h");
		frame(&mut cx);
		cx.read_entity(&shell, |shell, _| assert_eq!(shell.page, Page::Core(Route::Home)));
		cx.simulate_keystrokes("alt-shift-q");
		frame(&mut cx);
		cx.read_entity(&shell, |shell, _| assert_eq!(shell.side(), Some(super::super::Side::Core(Panel::Queue))));
		cx.simulate_keystrokes("alt-shift-j");
		frame(&mut cx);
		cx.read_entity(&shell, |shell, _| assert_eq!(shell.side(), Some(super::super::Side::Core(Panel::NowPlaying))));
		let volume = cx.read_entity(&shell, |shell, _| shell.st().volume);
		cx.simulate_keystrokes(super::super::primary("cmd-down", "ctrl-down"));
		frame(&mut cx);
		cx.read_entity(&shell, |shell, _| assert_eq!(shell.st().volume, volume.saturating_sub(5)));
	}

	#[test]
	fn focused_settings_search_keeps_character_shortcuts_as_text() {
		let (shell, mut cx, _directory) = setup();
		cx.update(|window, cx| {
			shell.update(cx, |shell, cx| {
				shell.open(Page::Settings, cx);
				shell.settings_search_open = true;
				shell.settings_query.update(cx, |input, cx| input.focus(window, cx));
			});
		});
		frame(&mut cx);
		let before = cx.read_entity(&shell, |shell, _| (shell.st().status, shell.st().volume));
		cx.simulate_keystrokes("a space m");
		frame(&mut cx);
		cx.read_entity(&shell, |shell, cx| {
			assert_eq!(shell.settings_query.read(cx).value(), "a m");
			assert_eq!((shell.st().status, shell.st().volume), before);
			assert_eq!(shell.page, Page::Settings);
		});
	}

	#[test]
	fn escape_closes_about_and_returns_focus() {
		let (shell, mut cx, _directory) = setup();
		cx.update(|window, cx| {
			shell.update(cx, |shell, cx| {
				shell.search.update(cx, |input, cx| input.focus(window, cx));
				shell.open_about(window, cx);
			});
		});
		frame(&mut cx);
		cx.read_entity(&shell, |shell, _| assert!(shell.about.is_some()));
		cx.simulate_keystrokes("escape");
		frame(&mut cx);
		cx.update(|window, cx| {
			let shell = shell.read(cx);
			assert!(shell.about.is_none());
			assert!(shell.search.read(cx).focus_handle(cx).is_focused(window));
		});
	}
}

#[test]
#[ignore = "manual per-repaint timing; no Spotify or native window"]
fn playhead_repaint_benchmark() {
	let (shell, mut cx, _directory) = setup();
	let playhead = cx.read_entity(&shell, |shell, _| shell.playhead.clone());
	cx.update(|window, cx| {
		// Warm the scene, text cache and allocations before timing just the draw pipeline.
		for _ in 0..20 {
			playhead.update(cx, |_, cx| cx.notify());
			window.draw(cx).clear(cx);
		}
		let start = std::time::Instant::now();
		for _ in 0..200 {
			playhead.update(cx, |_, cx| cx.notify());
			window.draw(cx).clear(cx);
		}
		let elapsed = start.elapsed();
		println!("200 playhead-only draws: {elapsed:?}; per draw: {:?}", elapsed / 200);
	});
}
