//! The top bar: back/forward, home and search in the middle, settings on the right. It's also
//! the window's title bar (drag to move, double-click to zoom).

use gpui_kit::{
	Context, Decorations, Div, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, ParentElement as _,
	StatefulInteractiveElement as _, Styled as _, Window, WindowControlArea,
	component::{Icon as CaptionIcon, IconName, input::Input},
	div,
	prelude::FluentBuilder as _,
	px,
};
use turntable_core::{Msg, Route};

use crate::{
	menu::{Action, Item},
	shell::{Page, Shell},
	theme,
	widgets::{Icon, icon, round_button},
};

/// Room for the macOS traffic lights.
const TRAFFIC_LIGHTS_W: f32 = 84.;

impl Shell {
	pub fn top_bar(&mut self, window: &Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let can_back = !self.back.is_empty();
		let captions = caption_controls_visible(window.window_decorations()) && !window.is_fullscreen();
		// Balance home/search between the side clusters on macOS. Where we draw caption
		// buttons, navigation gives up its spare room first so the controls stay reachable.
		let st = self.st();
		let side = if st.network_notice.is_some() || st.offline { 270. } else { 160. };
		let can_forward = !self.forward.is_empty();
		div()
			.relative()
			.h(theme::TOP_H)
			.flex_none()
			.flex()
			.items_center()
			.pl(theme::GAP)
			.when(!captions, |s| s.pr(theme::GAP))
			.child(drag_area())
			.child(
				div()
					.flex_1()
					.min_w(px(if captions { 72. } else { side }))
					.flex()
					.items_center()
					.gap_2()
					.when(cfg!(target_os = "macos"), |s| s.pl(px(TRAFFIC_LIGHTS_W)))
					// Spotify's back/forward are bare chevrons beside the traffic lights.
					.child(nav_chevron("back", Icon::Back, can_back, cx.listener(|this, _, _, cx| this.go_back(cx))))
					.child(nav_chevron("forward", Icon::Forward, can_forward, cx.listener(|this, _, _, cx| this.go_forward(cx)))),
			)
			.child(
				div()
					.flex()
					.flex_shrink(1.)
					.min_w_0()
					.items_center()
					.gap_2()
					.child(
						round_button(
							"home",
							Icon::Home,
							px(48.),
							px(24.),
							self.page == Page::Core(Route::Home),
							cx.listener(|this, _, _, cx| this.go(Route::Home, cx)),
						)
						.when(cfg!(target_os = "windows"), |s| s.occlude()),
					)
					.child(self.search_field(cx)),
			)
			.child(
				div()
					.flex_1()
					.min_w(px(side))
					.flex()
					.items_center()
					.justify_end()
					.gap_2()
					.children(self.network_pill())
					.child(
						round_button(
							"eq",
							Icon::Sliders,
							px(32.),
							px(16.),
							self.page == Page::Equalizer,
							cx.listener(|this, _, _, cx| this.open(Page::Equalizer, cx)),
						)
						.when(cfg!(target_os = "windows"), |s| s.occlude()),
					)
					.child(
						round_button(
							"settings",
							Icon::Gear,
							px(32.),
							px(16.),
							self.page == Page::Settings,
							cx.listener(|this, _, _, cx| this.open(Page::Settings, cx)),
						)
						.when(cfg!(target_os = "windows"), |s| s.occlude()),
					)
					.child(self.avatar(cx)),
			)
			.when(captions, |s| s.child(caption_controls(window)))
	}

	/// Spotify's account button: the user's initial; it opens Account, Profile, Settings and
	/// Log out.
	fn avatar(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		// The account's initial.
		let initial: String = self
			.st()
			.library
			.me
			.as_ref()
			.and_then(|me| me.display_name.as_deref().or(Some(me.id.as_str())))
			.and_then(|n| n.chars().next())
			.map(|c| c.to_uppercase().collect())
			.unwrap_or_default();
		div()
			.id("avatar")
			.when(cfg!(target_os = "windows"), |s| s.occlude())
			.size(px(48.))
			.flex()
			.items_center()
			.justify_center()
			.rounded_full()
			.bg(theme::raised())
			.cursor_pointer()
			.hover(|s| s.bg(theme::raised_hover()))
			.on_click(cx.listener(|this, e: &gpui_kit::ClickEvent, _, cx| {
				let at = e.position();
				let items = this.account_menu();
				// Under the avatar, its right edge level with the button's.
				this.open_menu(gpui_kit::point(at.x - px(220.), px(56.)), items, cx);
			}))
			.child(
				div()
					.size(px(32.))
					.rounded_full()
					.bg(gpui_kit::rgb(0x00e8_a33d))
					.flex()
					.items_center()
					.justify_center()
					.text_size(px(14.))
					.font_weight(gpui_kit::FontWeight::BOLD)
					.text_color(theme::base())
					.child(initial),
			)
	}

	/// The avatar's menu. Account and Profile open spotify.com in the browser, as the desktop
	/// app does.
	fn account_menu(&self) -> Vec<Item> {
		let profile = self.st().library.me.as_ref().map(|me| format!("https://open.spotify.com/user/{}", me.id));
		let mut items =
			vec![Item::row(Icon::MenuShare, "Account", Action::Send(Msg::OpenUrl("https://www.spotify.com/account/overview/".into())))];
		if let Some(url) = profile {
			items.push(Item::row(Icon::MenuArtist, "Profile", Action::Send(Msg::OpenUrl(url))));
		}
		items.push(Item::row(Icon::Gear, "Settings", Action::OpenPage(Page::Settings)));
		items.push(Item::Divider);
		items.push(Item::row(Icon::Back, "Log out", Action::Send(Msg::SignOut)));
		items
	}

	fn search_field(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let on = self.search_open || self.page == Page::Core(Route::Search);
		let dropdown = self.search_dropdown(cx);
		div()
			.id("search")
			.when(cfg!(target_os = "windows"), |s| s.occlude())
			.group("search")
			.h(px(48.))
			// Spotify's width, giving way to the side clusters in a narrow window.
			.w(px(474.))
			.min_w(px(160.))
			.relative()
			.flex_shrink(1.)
			.flex()
			.items_center()
			.gap_3()
			.px_3()
			.rounded_full()
			.bg(theme::raised())
			.border_2()
			.border_color(if on { theme::text() } else { gpui_kit::transparent_black().into() })
			.hover(|s| s.bg(theme::raised_hover()))
			// Focusing only: the page changes when a search is made (Enter or a suggestion).
			.on_click(cx.listener(|this, _, window, cx| this.search.update(cx, |s, cx| s.focus(window, cx))))
			.child(icon(Icon::Search, px(24.), if on { theme::text() } else { theme::subdued() }))
			.child(div().flex_1().text_size(px(16.)).child(Input::new(&self.search).appearance(false).cleanable(true)))
			.children(dropdown)
	}
}

/// A plain title bar for the welcome and startup-error pages. macOS keeps its traffic lights;
/// Linux draws controls only when the compositor actually grants client-side decorations.
pub fn window_titlebar(window: &Window) -> Option<impl IntoElement> {
	(caption_controls_visible(window.window_decorations()) && !window.is_fullscreen()).then(|| {
		div().absolute().top_0().left_0().right_0().h(theme::TOP_H).flex().justify_end().child(drag_area()).child(caption_controls(window))
	})
}

fn caption_controls_visible(decorations: Decorations) -> bool {
	cfg!(target_os = "windows") || (cfg!(target_os = "linux") && matches!(decorations, Decorations::Client { .. }))
}

fn drag_area() -> Div {
	div()
		.absolute()
		.inset_0()
		// Windows owns non-client dragging, double clicks, and snap gestures. Do not also
		// invoke the click handlers there: a native max button would otherwise toggle twice.
		.window_control_area(WindowControlArea::Drag)
		.when(cfg!(target_os = "macos"), |s| {
			s.on_mouse_down(MouseButton::Left, |e: &MouseDownEvent, window, _| {
				if e.click_count >= 2 {
					window.titlebar_double_click();
				} else {
					window.start_window_move();
				}
			})
		})
		.when(cfg!(target_os = "linux"), |s| {
			s.on_mouse_down(MouseButton::Left, |e, window, _| {
				if e.click_count >= 2 {
					window.zoom_window();
				}
			})
			.on_mouse_move(|e, window, _| {
				// Wait for movement so the compositor does not swallow the second click.
				if e.pressed_button == Some(MouseButton::Left) {
					window.start_window_move();
				}
			})
		})
}

fn caption_controls(window: &Window) -> impl IntoElement {
	let supported = window.window_controls();
	div()
		.flex()
		.flex_none()
		.h_full()
		.ml(theme::GAP)
		.when(supported.minimize, |s| s.child(caption_button("window-minimize", WindowControlArea::Min, IconName::WindowMinimize)))
		.when(supported.maximize, |s| {
			s.child(caption_button(
				"window-maximize",
				WindowControlArea::Max,
				if window.is_maximized() { IconName::WindowRestore } else { IconName::WindowMaximize },
			))
		})
		.child(caption_button("window-close", WindowControlArea::Close, IconName::WindowClose))
}

fn caption_button(id: &'static str, area: WindowControlArea, glyph: IconName) -> impl IntoElement {
	div()
		.id(id)
		// GPUI chooses the first matching control hitbox. Hide the underlying Drag area
		// so this button reports HTMINBUTTON/HTMAXBUTTON/HTCLOSE, not HTCAPTION.
		.occlude()
		.w(px(46.))
		.h_full()
		.flex_none()
		.flex()
		.items_center()
		.justify_center()
		.text_color(theme::text())
		.hover(move |s| s.bg(if area == WindowControlArea::Close { gpui_kit::rgb(0x00e8_1111) } else { theme::raised_hover() }))
		.window_control_area(area)
		.when(cfg!(target_os = "linux"), |s| {
			s.on_mouse_down(MouseButton::Left, |_, window, cx| {
				window.prevent_default();
				cx.stop_propagation();
			})
			.on_click(move |_, window, cx| {
				cx.stop_propagation();
				match area {
					WindowControlArea::Min => window.minimize_window(),
					WindowControlArea::Max => window.zoom_window(),
					WindowControlArea::Close => crate::desktop::request_close(window, cx),
					WindowControlArea::Drag => window.start_window_move(),
				}
			})
		})
		.child(CaptionIcon::new(glyph).size(px(12.)))
}

/// A back/forward chevron: subdued, white on hover, faint and inert when there's nowhere to go.
fn nav_chevron(
	id: &'static str,
	glyph: Icon,
	enabled: bool,
	on_click: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> impl IntoElement {
	div()
		.id(id)
		.when(cfg!(target_os = "windows"), |s| s.occlude())
		.group(id)
		.size(px(32.))
		.flex()
		.items_center()
		.justify_center()
		.when(enabled, |s| s.cursor_pointer().on_click(on_click))
		.child(
			icon(glyph, px(20.), if enabled { theme::subdued().into() } else { theme::alpha(theme::subdued(), 0.4) })
				.when(enabled, |s| s.group_hover(id, |s| s.text_color(theme::text()))),
		)
}

#[cfg(test)]
mod tests {
	use std::{cell::Cell, rc::Rc};

	use gpui_kit::{
		Context, InteractiveElement as _, IntoElement, Modifiers, ParentElement as _, Render, Styled as _, Window, div, point, px, size,
	};
	use gpui_test::{TestAppContext, VisualTestContext};

	use super::{Decorations, IconName, WindowControlArea, caption_button, caption_controls_visible, drag_area};

	#[test]
	fn caption_controls_follow_platform_and_negotiated_decorations() {
		// A Linux compositor can decline our client-decoration request. Its server-side
		// controls must remain the only set; Windows always needs our caption hitboxes.
		assert_eq!(caption_controls_visible(Decorations::Server), cfg!(target_os = "windows"));
		assert_eq!(
			caption_controls_visible(Decorations::Client { tiling: gpui_kit::Tiling::default() }),
			cfg!(any(target_os = "windows", target_os = "linux")),
		);
	}

	struct CaptionHitboxes(Rc<Cell<usize>>);

	impl Render for CaptionHitboxes {
		fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
			let moves = Rc::clone(&self.0);
			div().size_full().relative().child(drag_area().on_mouse_move(move |_, _, _| moves.set(moves.get() + 1))).child(
				div().absolute().top_0().right_0().w(px(46.)).h(crate::theme::TOP_H).child(caption_button(
					"test-caption",
					WindowControlArea::Close,
					IconName::WindowClose,
				)),
			)
		}
	}

	#[test]
	fn caption_buttons_occlude_the_underlying_native_drag_hitbox() {
		let mut app = TestAppContext::single();
		app.update(gpui_kit::init);
		let moves = Rc::new(Cell::new(0));
		let window = app.open_window(size(px(800.), px(200.)), |_, _| CaptionHitboxes(Rc::clone(&moves)));
		let mut cx = VisualTestContext::from_window(window.into(), &app);
		cx.update(|window, cx| window.draw(cx).clear(cx));
		moves.set(0);
		cx.simulate_mouse_move(point(px(777.), px(32.)), None, Modifiers::none());
		assert_eq!(moves.get(), 0, "caption controls must not also hit the Drag area behind them");
		cx.simulate_mouse_move(point(px(100.), px(32.)), None, Modifiers::none());
		assert!(moves.get() > 0, "empty title-bar space must remain draggable");
	}
}
