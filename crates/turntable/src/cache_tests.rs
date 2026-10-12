//! Exercise cache reuse and invalidation without Window::refresh, which bypasses every cache.

use gpui_kit::{AppContext as _, Entity, KeyDownEvent, Keystroke, Modifiers, point, px, size};
use gpui_test::VisualTestContext;
use turntable_core::{
	Msg, Panel, Route,
	library::{Kind, Response},
};

use super::{Page, Shell, frame, setup};

fn renders(shell: &Entity<Shell>, cx: &VisualTestContext) -> [usize; 7] {
	cx.read_entity(shell, |shell, cx| {
		let regions = &shell.regions;
		[
			regions.top.read(cx).renders,
			regions.library.read(cx).renders,
			regions.page.read(cx).renders,
			regions.panel.read(cx).renders,
			regions.now_playing.read(cx).renders,
			regions.transport.read(cx).renders,
			regions.extras.read(cx).renders,
		]
	})
}

fn assert_all_invalidated(before: [usize; 7], after: [usize; 7]) {
	assert!(before.into_iter().zip(after).all(|(before, after)| after > before), "before {before:?}, after {after:?}");
}

#[test]
fn playhead_only_draws_reuse_every_static_region() {
	let (shell, mut cx, _directory) = setup();
	let before = renders(&shell, &cx);
	assert!(before.into_iter().all(|count| count > 0));
	let playhead = cx.read_entity(&shell, |shell, _| shell.playhead.clone());
	cx.update(|window, cx| {
		for _ in 0..32 {
			playhead.update(cx, |_, cx| cx.notify());
			window.draw(cx).clear(cx);
		}
	});
	assert_eq!(renders(&shell, &cx), before);
}

#[test]
fn unchanged_core_pumps_do_not_invalidate_playing_regions() {
	let mut state = super::fixture_state();
	state.status = turntable_core::engine::Status::Playing;
	state.anchor.speed = 1.;
	state.item.decorations.playback.duration_ms = 180_000;
	let (shell, mut cx, _directory) = super::setup_with_state(state);
	cx.read_entity(&shell, |shell, _| assert!(shell.st().playing()));
	let before = renders(&shell, &cx);
	cx.update(|window, cx| {
		for _ in 0..32 {
			shell.update(cx, |shell, cx| shell.pump(cx));
			window.draw(cx).clear(cx);
		}
	});
	assert_eq!(renders(&shell, &cx), before);
}

#[test]
fn library_hover_and_header_animation_do_not_repaint_other_regions() {
	let (shell, mut cx, _directory) = setup();
	let [top, library, page, panel, now, transport, extras] = renders(&shell, &cx);
	cx.simulate_mouse_move(point(px(80.), px(226.)), None, Modifiers::none());
	frame(&mut cx);
	let [top_after, library_after, page_after, panel_after, now_after, transport_after, extras_after] = renders(&shell, &cx);
	assert!(library_after > library);
	assert_eq!([top_after, page_after, panel_after, now_after, transport_after, extras_after], [top, page, panel, now, transport, extras]);
	// Another row in the same library: hover listeners and hitboxes survived cache reuse.
	cx.simulate_mouse_move(point(px(80.), px(286.)), None, Modifiers::none());
	frame(&mut cx);
	let [top_after, library_next, page_after, panel_after, now_after, transport_after, extras_after] = renders(&shell, &cx);
	assert!(library_next > library_after);
	assert_eq!([top_after, page_after, panel_after, now_after, transport_after, extras_after], [top, page, panel, now, transport, extras]);
}

#[test]
fn child_view_notifications_invalidate_only_their_region() {
	let (shell, mut cx, _directory) = setup();
	cx.update(|_, cx| shell.update(cx, |shell, cx| shell.open(Page::Equalizer, cx)));
	frame(&mut cx);
	let [top, library, page, panel, now, transport, extras] = renders(&shell, &cx);
	cx.update(|_, cx| {
		let graph = shell.read(cx).eq_graph.clone();
		graph.update(cx, |_, cx| cx.notify());
	});
	frame(&mut cx);
	let [top_after, library_after, page_after, panel_after, now_after, transport_after, extras_after] = renders(&shell, &cx);
	assert!(page_after > page);
	assert_eq!([top_after, library_after, panel_after, now_after, transport_after, extras_after], [
		top, library, panel, now, transport, extras
	]);
}

#[test]
fn panel_header_hover_does_not_repaint_sibling_regions() {
	let (shell, mut cx, _directory) = setup();
	let [top, library, page, panel, now, transport, extras] = renders(&shell, &cx);
	cx.simulate_mouse_move(point(px(1248.), px(92.)), None, Modifiers::none());
	frame(&mut cx);
	let [top_after, library_after, page_after, panel_after, now_after, transport_after, extras_after] = renders(&shell, &cx);
	assert!(panel_after > panel);
	assert_eq!([top_after, library_after, page_after, now_after, transport_after, extras_after], [
		top, library, page, now, transport, extras
	]);
}

#[test]
fn transport_hover_does_not_repaint_sibling_regions() {
	let (shell, mut cx, _directory) = setup();
	let [top, library, page, panel, now, transport, extras] = renders(&shell, &cx);
	cx.simulate_mouse_move(point(px(640.), px(752.)), None, Modifiers::none());
	frame(&mut cx);
	let [top_after, library_after, page_after, panel_after, now_after, transport_after, extras_after] = renders(&shell, &cx);
	assert!(transport_after > transport);
	assert_eq!([top_after, library_after, page_after, panel_after, now_after, extras_after], [top, library, page, panel, now, extras]);
}

#[test]
fn shell_changes_invalidate_cached_state_navigation_inputs_and_artwork() {
	let (shell, mut cx, _directory) = setup();
	for msg in [Msg::ToggleShuffle, Msg::Volume(37), Msg::TogglePanel(Panel::Queue), Msg::LibraryKind(Kind::Album)] {
		let before = renders(&shell, &cx);
		cx.update(|_, cx| shell.update(cx, |shell, cx| shell.send(msg, cx)));
		frame(&mut cx);
		assert_all_invalidated(before, renders(&shell, &cx));
	}
	let before = renders(&shell, &cx);
	cx.update(|_, cx| shell.update(cx, |shell, cx| shell.open(Page::Settings, cx)));
	frame(&mut cx);
	assert_all_invalidated(before, renders(&shell, &cx));
	let before = renders(&shell, &cx);
	cx.update(|window, cx| {
		shell.update(cx, |shell, cx| {
			shell.settings_search_open = true;
			shell.settings_query.update(cx, |input, cx| input.set_value("audio", window, cx));
			cx.notify();
		});
	});
	frame(&mut cx);
	assert_all_invalidated(before, renders(&shell, &cx));
	let before = renders(&shell, &cx);
	cx.update(|_, cx| {
		shell.update(cx, |shell, cx| {
			let me = shell.st().library.me.clone().unwrap();
			shell.core.fixture_response(Response::Connected { me, playlists: Vec::new() }).unwrap();
			shell.pump(cx);
		});
	});
	frame(&mut cx);
	assert_all_invalidated(before, renders(&shell, &cx));
	// CoverCache completion also notifies Shell by entity id, not through a Shell method.
	let before = renders(&shell, &cx);
	cx.update(|_, cx| cx.notify(shell.entity_id()));
	frame(&mut cx);
	assert_all_invalidated(before, renders(&shell, &cx));
}

#[test]
fn resize_folding_and_full_view_relayout_cached_regions() {
	let (shell, mut cx, _directory) = setup();
	let before = renders(&shell, &cx);
	cx.simulate_resize(size(px(900.), px(650.)));
	frame(&mut cx);
	assert_all_invalidated(before, renders(&shell, &cx));
	cx.read_entity(&shell, |shell, _| assert!(shell.sidebar_folded()));
	cx.update(|_, cx| shell.update(cx, |shell, cx| shell.toggle_library(cx)));
	frame(&mut cx);
	cx.read_entity(&shell, |shell, _| {
		assert!(!shell.sidebar_folded());
		assert!(shell.panel_folded());
	});
	cx.update(|_, cx| shell.update(cx, |shell, cx| shell.set_full_view(true, cx)));
	frame(&mut cx);
	cx.update(|_, cx| shell.update(cx, |shell, cx| shell.set_full_view(false, cx)));
	frame(&mut cx);
	cx.update(|_, cx| shell.update(cx, |shell, cx| shell.go(Route::Liked, cx)));
	frame(&mut cx);
	cx.read_entity(&shell, |shell, _| assert_eq!(shell.list_for, Some((Route::Liked, 0))));
}

#[test]
fn empty_search_submit_invalidates_the_closed_dropdown() {
	let (shell, mut cx, _directory) = setup();
	cx.update(|window, cx| {
		let input = shell.read(cx).search.clone();
		input.update(cx, |input, cx| input.focus(window, cx));
	});
	frame(&mut cx);
	let before = renders(&shell, &cx);
	cx.simulate_event(KeyDownEvent {
		keystroke:              Keystroke::parse("enter").unwrap(),
		is_held:                false,
		prefer_character_input: false,
	});
	frame(&mut cx);
	cx.read_entity(&shell, |shell, _| {
		assert!(!shell.search_open);
		assert_eq!(shell.page, Page::Core(Route::Home));
	});
	assert_all_invalidated(before, renders(&shell, &cx));
}
