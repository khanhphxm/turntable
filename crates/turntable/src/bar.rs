//! The player bar: now playing on the left, transport and seek in the middle, panels and volume on the right.

use gpui_kit::{
	Context, InteractiveElement as _, IntoElement, ParentElement as _, StatefulInteractiveElement as _, Styled as _, div,
	prelude::FluentBuilder as _, px, relative,
};

use crate::{
	components::clock,
	mock::DEVICES,
	shell::{Panel, Repeat, Route, Shell},
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
		let (Some(ix), Some(n)) = (self.now_index(), self.now) else {
			return side;
		};
		let Some(t) = self.lib.tracks.get(ix) else {
			return side;
		};
		let (title, artist, album) = (t.title, t.artist, t.album);
		let added = self.lib.added(ix);
		let from = n.from;
		side.child(
			div().id("bar-cover").cursor_pointer().on_click(cx.listener(move |this, _, _, cx| this.open(from.route(), cx))).child(cover(
				None,
				self.lib.track_color(ix),
				px(56.),
				px(4.),
			)),
		)
		.child(
			div()
				.min_w_0()
				.flex()
				.flex_col()
				.child(
					self.link("bar-title", title, Route::Collection(crate::mock::Collection::Album(album)), cx)
						.text_size(px(14.))
						.text_color(theme::text()),
				)
				.child(self.link("bar-artist", self.lib.artist_name(artist).to_owned(), Route::Artist(artist), cx).text_size(px(12.))),
		)
		.child(
			div()
				.id("bar-add")
				.size(px(32.))
				.flex()
				.flex_none()
				.items_center()
				.justify_center()
				.cursor_pointer()
				.on_click(cx.listener(move |this, _, _, cx| this.toggle_liked(ix, cx)))
				.child(added_badge(added, px(16.))),
		)
	}

	fn bar_transport(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let total = self.now_track().map_or(0, |t| t.ms);
		let at = self.position();
		let playing = self.playing;
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
						self.shuffle,
						cx.listener(|this, _, _, cx| {
							this.shuffle = !this.shuffle;
							cx.notify();
						}),
					))
					.child(icon_button("prev", Icon::Prev, px(16.), false, cx.listener(|this, _, _, cx| this.skip(-1, cx))))
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
							.on_click(cx.listener(|this, _, _, cx| this.toggle_play(cx)))
							.child(icon(if playing { Icon::Pause } else { Icon::Play }, px(16.), theme::base())),
					)
					.child(icon_button("next", Icon::Next, px(16.), false, cx.listener(|this, _, _, cx| this.skip(1, cx))))
					.child(icon_button(
						"repeat",
						if self.repeat == Repeat::One { Icon::RepeatOne } else { Icon::Repeat },
						px(16.),
						self.repeat != Repeat::Off,
						cx.listener(|this, _, _, cx| {
							this.repeat = match this.repeat {
								Repeat::Off => Repeat::All,
								Repeat::All => Repeat::One,
								Repeat::One => Repeat::Off,
							};
							cx.notify();
						}),
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
		let panel = self.panel;
		let muted = self.volume <= 0.;
		let elsewhere = self.device != 0;
		let panel_button = |id: &'static str, glyph: Icon, p: Panel, cx: &mut Context<'_, Self>| {
			icon_button(id, glyph, px(16.), panel == Some(p), cx.listener(move |this, _, _, cx| this.toggle_panel(p, cx)))
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
					.child(panel_button("np", Icon::NowPlaying, Panel::NowPlaying, cx))
					.when(self.prefs.lyrics_bar, |s| s.child(panel_button("lyrics", Icon::Mic, Panel::Lyrics, cx)))
					.child(panel_button("queue", Icon::Queue, Panel::Queue, cx))
					.child(panel_button("devices", Icon::Device, Panel::Devices, cx))
					.child(icon_button(
						"mute",
						if muted { Icon::Mute } else { Icon::Volume },
						px(16.),
						false,
						cx.listener(|this, _, _, cx| this.toggle_mute(cx)),
					))
					.child(div().w(px(93.)).child(self.volume_slider.clone())),
			)
			// Playing on another device: Spotify's green strip names it.
			.when(elsewhere, |s| {
				s.child(
					div()
						.flex()
						.items_center()
						.gap_1()
						.pr_4()
						.text_size(px(12.))
						.text_color(theme::accent())
						.child(icon(Icon::Device, px(12.), theme::accent()))
						.child(format!("Playing on {}", DEVICES.get(self.device).map_or("", |d| d.name))),
				)
			})
	}
}
