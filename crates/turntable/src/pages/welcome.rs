//! First run: the app's mark, one sentence, and a single button that signs in through the browser.

use gpui_kit::{
	Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, StatefulInteractiveElement as _, Styled as _, div, img,
	prelude::FluentBuilder as _, px,
};

use crate::{shell::Shell, theme};

impl Shell {
	pub fn welcome_page(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let waiting = self.st().starting_player();
		div()
			.size_full()
			.flex()
			.flex_col()
			.items_center()
			.justify_center()
			.gap_6()
			.bg(theme::base())
			.text_color(theme::text())
			.child(img("icons/app.svg").size(px(128.)))
			.child(div().text_size(px(32.)).font_weight(FontWeight::BOLD).child("turntable"))
			.child(div().text_size(px(16.)).text_color(theme::subdued()).child("A lightweight Spotify player. Spotify Premium required."))
			.child(
				div()
					.id("sign-in")
					.mt_4()
					.h(px(48.))
					.px_8()
					.flex()
					.items_center()
					.rounded_full()
					.bg(theme::accent())
					.text_color(theme::base())
					.text_size(px(16.))
					.font_weight(FontWeight::BOLD)
					.cursor_pointer()
					.hover(|s| s.bg(theme::accent_hover()))
					.when(waiting, |s| s.opacity(0.6))
					.on_click(cx.listener(|this, _, _, cx| this.send(turntable_core::Msg::SignIn, cx)))
					.child(if waiting { "Waiting for your browser…" } else { "Sign in with Spotify" }),
			)
			.when_some(self.st().notice.clone(), |s, n| {
				s.child(div().max_w(px(480.)).text_size(px(14.)).text_color(theme::subdued()).child(n))
			})
			.when(waiting, |s| {
				s.child(
					div()
						.text_size(px(14.))
						.text_color(theme::subdued())
						.child("Finish signing in in the browser window that opened, then come back here."),
				)
			})
	}
}
