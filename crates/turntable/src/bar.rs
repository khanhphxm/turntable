//! The player bar: now playing on the left, transport and seek in the middle, panels and volume on the right.

use gpui_kit::{
	Context, InteractiveElement as _, IntoElement, ParentElement as _, StatefulInteractiveElement as _, Styled as _, div,
	prelude::FluentBuilder as _, px, relative,
};
use turntable_core::{
	Msg, Panel, Route,
	engine::{CoverSize, Repeat},
	spotify::id_of,
};

use crate::{
	components::clock,
	shell::{Shell, Side},
	theme,
	widgets::{Icon, added_badge, cover, icon, icon_button},
};

/// The bar's side columns: 30% of the width, at least this wide.
const SIDE_MIN_W: f32 = 180.;

impl Shell {
	pub fn player_bar(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		div()
			.h(theme::BAR_H)
			.flex_none()
			.flex()
			.items_center()
			.justify_between()
			.px(theme::GAP)
			.child(self.bar_now(cx))
			.child(self.bar_transport(cx))
			.child(self.bar_extras(cx))
	}

	fn bar_now(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let side = div().w(relative(0.3)).min_w(px(SIDE_MIN_W)).flex().items_center().gap_3().pl_2();
		let item = &self.st().item;
		if item.uri.is_empty() {
			return side;
		}
		let title = item.name().to_owned();
		let artists = item.creators();
		let artist = item.decorations.creators.first().map(|c| id_of(&c.entity.uri).to_owned());
		let album = item.decorations.parent.as_ref().map(|p| id_of(&p.entity.uri).to_owned());
		let url = item.cover(CoverSize::Small).map(str::to_owned);
		let liked = self.st().liked();
		let from = self.st().played_from.clone();
		side.child(
			div()
				.id("bar-cover")
				.cursor_pointer()
				.on_click(cx.listener(move |this, _, _, cx| {
					if let Some(route) = from.clone() {
						this.go(route, cx);
					}
				}))
				.child(cover(self.image(url.as_deref(), 56, cx), theme::RAISED, px(56.), px(4.))),
		)
		.child(
			div()
				.min_w_0()
				.flex()
				.flex_col()
				.child(match album {
					Some(id) => {
						self.link("bar-title", title, Route::Album(id), cx).text_size(px(14.)).text_color(theme::text()).into_any_element()
					},
					None => div().text_size(px(14.)).truncate().child(title).into_any_element(),
				})
				.child(match artist {
					Some(id) => self.link("bar-artist", artists, Route::Artist(id), cx).text_size(px(12.)).into_any_element(),
					None => div().text_size(px(12.)).text_color(theme::subdued()).truncate().child(artists).into_any_element(),
				}),
		)
		.when_some(liked, |s, liked| {
			s.child(
				div()
					.id("bar-add")
					.size(px(32.))
					.flex()
					.flex_none()
					.items_center()
					.justify_center()
					.cursor_pointer()
					.on_click(cx.listener(|this, _, _, cx| this.send(Msg::ToggleLike, cx)))
					.child(added_badge(liked, px(16.))),
			)
		})
	}

	fn bar_transport(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let st = self.st();
		let total = st.duration_ms();
		let at = st.position_ms().min(total);
		let playing = st.playing();
		let shuffle = st.options.shuffle;
		let repeat = st.options.repeat;
		div()
			.flex_1()
			.max_w(px(722.))
			.flex()
			.flex_col()
			.items_center()
			.gap_1()
			.child(
				div()
					.flex()
					.items_center()
					.gap_2()
					.child(icon_button(
						"shuffle",
						Icon::Shuffle,
						px(16.),
						shuffle,
						cx.listener(|this, _, _, cx| this.send(Msg::ToggleShuffle, cx)),
					))
					.child(icon_button("prev", Icon::Prev, px(16.), false, cx.listener(|this, _, _, cx| this.send(Msg::Prev, cx))))
					.child(
						div()
							.id("play")
							.size(px(32.))
							.mx_2()
							.rounded_full()
							.bg(theme::text())
							.flex()
							.items_center()
							.justify_center()
							.cursor_pointer()
							.hover(|s| s.bg(theme::alpha(theme::text(), 0.9)))
							.active(|s| s.bg(theme::subdued()))
							.on_click(cx.listener(|this, _, _, cx| this.send(Msg::TogglePlay, cx)))
							.child(icon(if playing { Icon::Pause } else { Icon::Play }, px(16.), theme::base())),
					)
					.child(icon_button("next", Icon::Next, px(16.), false, cx.listener(|this, _, _, cx| this.send(Msg::Next, cx))))
					.child(icon_button(
						"repeat",
						if repeat == Repeat::Track { Icon::RepeatOne } else { Icon::Repeat },
						px(16.),
						repeat != Repeat::Off,
						cx.listener(|this, _, _, cx| this.send(Msg::CycleRepeat, cx)),
					)),
			)
			.child(
				div()
					.w_full()
					.flex()
					.items_center()
					.gap_2()
					.text_size(px(12.))
					.text_color(theme::subdued())
					.child(div().w(px(40.)).flex().justify_end().child(clock(at)))
					.child(div().flex_1().child(self.seek.clone()))
					.child(div().w(px(40.)).child(clock(total))),
			)
	}

	fn bar_extras(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let side = self.side();
		let muted = self.st().volume == 0;
		let remote = self.st().remote().map(|d| d.name.clone());
		let lyrics = self.st().prefs.sources.lyrics;
		let panel_button = |id: &'static str, glyph: Icon, s: Side, cx: &mut Context<'_, Self>| {
			icon_button(id, glyph, px(16.), side == Some(s), cx.listener(move |this, _, _, cx| this.toggle_side(s, cx)))
		};
		div()
			.w(relative(0.3))
			.min_w(px(SIDE_MIN_W))
			.flex()
			.flex_col()
			.items_end()
			.child(
				div()
					.flex()
					.items_center()
					.justify_end()
					.pr_2()
					.child(panel_button("np", Icon::NowPlaying, Side::Core(Panel::NowPlaying), cx))
					.when(lyrics, |s| s.child(panel_button("lyrics", Icon::Mic, Side::Core(Panel::Lyrics), cx)))
					.child(panel_button("queue", Icon::Queue, Side::Core(Panel::Queue), cx))
					.child(panel_button("devices", Icon::Device, Side::Devices, cx))
					.child(icon_button(
						"mute",
						if muted { Icon::Mute } else { Icon::Volume },
						px(16.),
						false,
						cx.listener(|this, _, _, cx| this.send(Msg::ToggleMute, cx)),
					))
					.child(div().w(px(93.)).child(self.volume_slider.clone())),
			)
			// Playing on another device: Spotify's green strip names it.
			.when_some(remote, |s, name| {
				s.child(
					div()
						.flex()
						.items_center()
						.gap_1()
						.pr_4()
						.text_size(px(12.))
						.text_color(theme::accent())
						.child(icon(Icon::Device, px(12.), theme::accent()))
						.child(format!("Playing on {name}")),
				)
			})
	}
}
