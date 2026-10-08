//! The top bar: back/forward, home and search in the middle, settings on the right. It's also
//! the window's title bar (drag to move, double-click to zoom).

use gpui_kit::{
	Context, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, ParentElement as _, StatefulInteractiveElement as _,
	Styled as _, component::input::Input, div, prelude::FluentBuilder as _, px,
};
use turntable_core::Route;

use crate::{
	shell::{Page, Shell},
	theme,
	widgets::{Icon, icon, round_button},
};

/// Room for the macOS traffic lights.
const TRAFFIC_LIGHTS_W: f32 = 84.;

impl Shell {
	pub fn top_bar(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let can_back = !self.back.is_empty();
		// The account's initial in the avatar.
		let initial: String = self
			.st()
			.library
			.me
			.as_ref()
			.and_then(|me| me.display_name.as_deref().or(Some(me.id.as_str())))
			.and_then(|n| n.chars().next())
			.map(|c| c.to_uppercase().collect())
			.unwrap_or_default();
		let can_forward = !self.forward.is_empty();
		div()
			.relative()
			.h(theme::TOP_H)
			.flex_none()
			.flex()
			.items_center()
			.px(theme::GAP)
			.child(div().absolute().inset_0().on_mouse_down(MouseButton::Left, |e: &MouseDownEvent, window, _| {
				if e.click_count >= 2 {
					window.titlebar_double_click();
				} else {
					window.start_window_move();
				}
			}))
			.child(
				div()
					.flex_1()
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
					.items_center()
					.gap_2()
					.child(round_button(
						"home",
						Icon::Home,
						px(48.),
						px(24.),
						self.page == Page::Core(Route::Home),
						cx.listener(|this, _, _, cx| this.go(Route::Home, cx)),
					))
					.child(self.search_field(cx)),
			)
			.child(
				div()
					.flex_1()
					.flex()
					.items_center()
					.justify_end()
					.gap_2()
					.children(self.network_pill())
					.child(round_button(
						"eq",
						Icon::Sliders,
						px(32.),
						px(16.),
						self.page == Page::Equalizer,
						cx.listener(|this, _, _, cx| this.open(Page::Equalizer, cx)),
					))
					.child(round_button(
						"settings",
						Icon::Gear,
						px(32.),
						px(16.),
						self.page == Page::Settings,
						cx.listener(|this, _, _, cx| this.open(Page::Settings, cx)),
					))
					.child(
						div().size(px(48.)).flex().items_center().justify_center().rounded_full().bg(theme::raised()).child(
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
						),
					),
			)
	}

	fn search_field(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let on = self.page == Page::Core(Route::Search);
		div()
			.id("search")
			.group("search")
			.h(px(48.))
			.w(px(474.))
			.flex()
			.items_center()
			.gap_3()
			.px_3()
			.rounded_full()
			.bg(theme::raised())
			.border_2()
			.border_color(if on { theme::text() } else { gpui_kit::transparent_black().into() })
			.hover(|s| s.bg(theme::raised_hover()))
			.on_click(cx.listener(|this, _, window, cx| {
				this.search.update(cx, |s, cx| s.focus(window, cx));
				this.go(Route::Search, cx);
			}))
			.child(icon(Icon::Search, px(24.), if on { theme::text() } else { theme::subdued() }))
			.child(div().flex_1().text_size(px(16.)).child(Input::new(&self.search).appearance(false).cleanable(true)))
	}
}

/// A back/forward chevron: subdued, white on hover, faint and inert when there's nowhere to go.
fn nav_chevron(
	id: &'static str,
	glyph: Icon,
	enabled: bool,
	on_click: impl Fn(&gpui_kit::ClickEvent, &mut gpui_kit::Window, &mut gpui_kit::App) + 'static,
) -> impl IntoElement {
	div()
		.id(id)
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
