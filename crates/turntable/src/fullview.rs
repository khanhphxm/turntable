//! Now playing at full size, as Spotify draws it: expanded over the main view (from the right
//! panel's expand button) or across the whole screen (the bar's full-screen button). The song's
//! cover, blurred, fills the background; the big cover and title sit bottom-left, or the lyrics
//! fill it.

use gpui_kit::{
	AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Styled as _, div,
	prelude::FluentBuilder as _, px,
};
use turntable_core::{Panel, engine::CoverSize};

use crate::{
	shell::{FullView, Shell, Side},
	theme,
	widgets::{Icon, icon_button},
};

impl Shell {
	pub fn full_view_page(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
		let mode = self.full_view;
		let st = self.st();
		let item = st.item.clone();
		let lyrics = st.lyrics_shown();
		let big = item.cover(CoverSize::Large).map(str::to_owned);
		let cover = self.image(big.as_deref(), 320, cx);
		let creators = item.creators();
		let heading = [st.context.name(), item.parent_name()].into_iter().find(|n| !n.is_empty()).unwrap_or("Now playing").to_owned();
		let body: AnyElement = if lyrics {
			self.lyric_lines(40., 120., cx)
		} else {
			div()
				.size_full()
				.flex()
				.items_end()
				.gap_6()
				.p_10()
				.child(crate::widgets::cover(cover, theme::RAISED, px(240.), px(8.)).shadow_2xl())
				.child(
					div()
						.flex()
						.flex_col()
						.gap_2()
						.min_w_0()
						.child(div().text_size(px(48.)).font_weight(FontWeight::BOLD).line_clamp(2).child(item.name().to_owned()))
						.child(div().text_size(px(20.)).text_color(theme::subdued()).truncate().child(creators)),
				)
				.into_any_element()
		};
		let header = div()
			.absolute()
			.top_0()
			.left_0()
			.right_0()
			.h(px(56.))
			.px_4()
			.flex()
			.items_center()
			.justify_between()
			.child(div().text_size(px(16.)).font_weight(FontWeight::BOLD).truncate().child(heading))
			.child(
				div()
					.flex()
					.items_center()
					.child(icon_button(
						"full-lyrics",
						Icon::Mic,
						px(16.),
						lyrics,
						cx.listener(|this, _, _, cx| this.toggle_side(Side::Core(Panel::Lyrics), cx)),
					))
					.when(mode == FullView::Expanded, |s| {
						s.child(icon_button(
							"full-collapse",
							Icon::Collapse,
							px(16.),
							false,
							cx.listener(|this, _, _, cx| this.set_full_view(false, cx)),
						))
					})
					.child(icon_button(
						"full-screen",
						if mode == FullView::Screen { Icon::Collapse } else { Icon::Fullscreen },
						px(16.),
						false,
						cx.listener(|this, _, window, cx| this.toggle_full_screen(window, cx)),
					)),
			);
		div()
			.id("full-view")
			.size_full()
			.relative()
			.overflow_hidden()
			.rounded(theme::RADIUS)
			// The song's cover blurred behind everything, as Apple Music's player is.
			.child(self.lyrics_backdrop(cx))
			.child(div().absolute().inset_0().pt(px(56.)).child(body))
			.child(header)
			.child(crate::widgets::corner_mask())
			.into_any_element()
	}
}
