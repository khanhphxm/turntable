//! The right-hand panel: now playing, queue, lyrics and devices. It slides and fades in whenever
//! what it shows changes.

use std::time::Duration;

use gpui_kit::{
	Animation, AnimationExt as _, AnyElement, Context, Div, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
	StatefulInteractiveElement as _, Styled as _, div, ease_out_quint, prelude::FluentBuilder as _, px, rgb,
};

use crate::{
	components::grouped,
	mock::{DEVICES, DeviceKind, LYRICS},
	shell::{Panel, Route, Shell},
	theme,
	widgets::{Icon, added_badge, icon, icon_button},
};

/// How long a panel takes to slide and fade in.
const ENTER: Duration = Duration::from_millis(260);
/// The mock's lyrics advance one line this often.
const LINE_MS: u64 = 4000;

/// A titled card inside the panel (about the artist, next in queue).
fn panel_card(title: &'static str) -> Div {
	div()
		.rounded(px(8.))
		.bg(theme::raised())
		.p_4()
		.flex()
		.flex_col()
		.gap_3()
		.child(div().text_size(px(16.)).font_weight(FontWeight::BOLD).child(title))
}

fn heading(text: impl Into<gpui_kit::SharedString>) -> Div {
	div().px_2().pt_4().pb_2().text_size(px(16.)).font_weight(FontWeight::BOLD).child(text.into())
}

impl Shell {
	pub fn right_panel(&mut self, panel: Panel, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let title: String = match panel {
			Panel::NowPlaying => self.now.map_or("Now playing".to_owned(), |n| self.lib.name_of(n.from).to_owned()),
			Panel::Queue => "Queue".to_owned(),
			Panel::Lyrics => "Lyrics".to_owned(),
			Panel::Devices => "Connect to a device".to_owned(),
		};
		let body = match panel {
			Panel::NowPlaying => self.now_playing_body(cx),
			Panel::Queue => self.queue_body(cx),
			Panel::Lyrics => self.lyrics_body(cx),
			Panel::Devices => self.devices_body(cx),
		};
		// Lyrics sit on the cover's colour, darkened so white text reads on any cover.
		let tint = if panel == Panel::Lyrics { self.now_index().map(|t| theme::darken(rgb(self.lib.track_color(t)), 0.45)) } else { None };
		div()
			.w(theme::RIGHT_W)
			.flex_none()
			.h_full()
			.rounded(theme::RADIUS)
			.bg(theme::panel())
			.when_some(tint, |s, c| s.bg(c))
			.overflow_hidden()
			.flex()
			.flex_col()
			.child(
				div()
					.h(px(56.))
					.flex_none()
					.flex()
					.items_center()
					.justify_between()
					.pl_4()
					.pr_2()
					.child(div().text_size(px(16.)).font_weight(FontWeight::BOLD).truncate().child(title))
					.child(icon_button(
						"panel-close",
						Icon::Close,
						px(16.),
						false,
						cx.listener(move |this, _, _, cx| this.toggle_panel(panel, cx)),
					)),
			)
			// Lyrics scroll themselves (to follow the sung line); the rest scroll here.
			.child(
				div()
					.id(("panel-scroll", self.panel_gen))
					.flex_1()
					.min_h_0()
					.when(panel != Panel::Lyrics, |s| s.overflow_y_scroll())
					.child(body),
			)
			.with_animation(("panel", self.panel_gen), Animation::new(ENTER).with_easing(ease_out_quint()), |el, d| {
				el.opacity(d).relative().left(px(32. * (1. - d)))
			})
	}

	fn now_playing_body(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
		let Some(ix) = self.now_index() else {
			return div().p_4().text_color(theme::subdued()).child("Nothing playing").into_any_element();
		};
		let Some(t) = self.lib.tracks.get(ix) else {
			return div().into_any_element();
		};
		let artist = t.artist;
		let added = self.lib.added(ix);
		let next = self.up_next().into_iter().next();
		div()
			.flex()
			.flex_col()
			.gap_4()
			.px_4()
			.pb_4()
			.child(div().w_full().h(px(308.)).rounded(px(8.)).bg(rgb(self.lib.track_color(ix))).shadow_lg())
			.child(
				div()
					.flex()
					.items_center()
					.gap_3()
					.child(
						div()
							.flex_1()
							.min_w_0()
							.flex()
							.flex_col()
							.child(div().text_size(px(24.)).font_weight(FontWeight::BOLD).truncate().child(t.title))
							.child(
								self.link("np-artist", self.lib.artist_name(artist).to_owned(), Route::Artist(artist), cx)
									.text_size(px(16.))
									.text_color(theme::subdued()),
							),
					)
					.child(
						div()
							.id("np-add")
							.cursor_pointer()
							.on_click(cx.listener(move |this, _, _, cx| this.toggle_liked(ix, cx)))
							.child(added_badge(added, px(24.))),
					),
			)
			.when_some(self.lib.artists.get(artist), |s, a| {
				s.child(
					div()
						.rounded(px(8.))
						.overflow_hidden()
						.bg(theme::raised())
						.child(
							div()
								.h(px(180.))
								.bg(rgb(a.color))
								.p_4()
								.text_size(px(16.))
								.font_weight(FontWeight::BOLD)
								.child("About the artist"),
						)
						.child(
							div()
								.p_4()
								.flex()
								.flex_col()
								.gap_2()
								.child(div().text_size(px(16.)).font_weight(FontWeight::BOLD).child(a.name))
								.child(
									div()
										.text_size(px(14.))
										.text_color(theme::subdued())
										.child(format!("{} monthly listeners", grouped(u64::from(a.listeners)))),
								)
								.child(div().text_size(px(14.)).text_color(theme::subdued()).line_clamp(3).child(a.about)),
						),
				)
			})
			.when_some(next, |s, (key, track)| s.child(panel_card("Next in queue").child(self.queue_row(key, track, false, cx))))
			.into_any_element()
	}

	/// What plays next: the user's queue, then the rest of the collection.
	pub fn up_next(&self) -> Vec<(usize, usize)> {
		let mut out: Vec<(usize, usize)> = self.queue.iter().enumerate().map(|(i, &t)| (10_000 + i, t)).collect();
		if let Some(n) = self.now {
			out.extend(self.lib.tracks_of(n.from).into_iter().enumerate().skip(n.pos + 1).take(40));
		}
		out
	}

	/// A compact track line: cover, title, artist. `key` is the position it plays from.
	fn queue_row(&self, key: usize, track: usize, current: bool, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let Some(t) = self.lib.tracks.get(track) else {
			return div().id(("queue", key));
		};
		let playing = current && self.playing;
		div()
			.id(("queue", key))
			.group("queue-row")
			.h(px(56.))
			.px_2()
			.flex()
			.items_center()
			.gap_3()
			.rounded(px(4.))
			.hover(|s| s.bg(theme::hover()))
			.cursor_pointer()
			.on_click(cx.listener(move |this, _, _, cx| {
				if let Some(n) = this.now
					&& key < 10_000
				{
					this.play_from(n.from, key, cx);
				}
			}))
			.child(
				div().relative().child(crate::widgets::cover(None, self.lib.track_color(track), px(40.), px(4.))).child(
					div()
						.absolute()
						.inset_0()
						.rounded(px(4.))
						.flex()
						.items_center()
						.justify_center()
						.bg(theme::alpha(theme::base(), 0.5))
						.invisible()
						.group_hover("queue-row", |s| s.visible())
						.child(icon(if playing { Icon::Pause } else { Icon::Play }, px(16.), theme::text())),
				),
			)
			.child(
				div()
					.min_w_0()
					.flex()
					.flex_col()
					.child(div().text_size(px(16.)).truncate().when(current, |s| s.text_color(theme::accent())).child(t.title))
					.child(
						div().text_size(px(14.)).text_color(theme::subdued()).truncate().child(self.lib.artist_name(t.artist).to_owned()),
					),
			)
	}

	fn queue_body(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
		let (Some(n), Some(cur)) = (self.now, self.now_index()) else {
			return div().p_4().text_color(theme::subdued()).child("Your queue is empty").into_any_element();
		};
		let up = self.up_next();
		let (mine, rest): (Vec<_>, Vec<_>) = up.into_iter().partition(|(k, _)| *k >= 10_000);
		let mine: Vec<_> = mine.iter().map(|&(k, t)| self.queue_row(k, t, false, cx).into_any_element()).collect();
		let rest_rows: Vec<_> = rest.iter().map(|&(k, t)| self.queue_row(k, t, false, cx).into_any_element()).collect();
		div()
			.flex()
			.flex_col()
			.px_2()
			.pb_4()
			.child(heading("Now playing"))
			.child(self.queue_row(n.pos, cur, true, cx))
			.when(!mine.is_empty(), |s| s.child(heading("Next in queue")).children(mine))
			.when(!rest_rows.is_empty(), |s| s.child(heading(format!("Next from: {}", self.lib.name_of(n.from)))).children(rest_rows))
			.into_any_element()
	}

	fn lyrics_body(&mut self, _: &mut Context<'_, Self>) -> AnyElement {
		let line = usize::try_from(self.position() / LINE_MS).unwrap_or(usize::MAX);
		// Keep the sung line in view, a few lines down from the top.
		if self.lyrics_line != Some(line) {
			self.lyrics_line = Some(line);
			self.lyrics_scroll.scroll_to_top_of_item(line.saturating_sub(2));
		}
		div()
			.id("lyrics")
			.size_full()
			.overflow_y_scroll()
			.track_scroll(&self.lyrics_scroll)
			.flex()
			.flex_col()
			.gap_4()
			.px_4()
			.pb_8()
			.text_size(px(24.))
			.font_weight(FontWeight::BOLD)
			.children(LYRICS.iter().enumerate().map(|(i, l)| {
				let color = match i.cmp(&line) {
					std::cmp::Ordering::Less => theme::alpha(theme::text(), 0.55),
					std::cmp::Ordering::Equal => theme::text().into(),
					std::cmp::Ordering::Greater => theme::alpha(theme::base(), 0.7),
				};
				div().text_color(color).child(*l)
			}))
			.into_any_element()
	}

	fn devices_body(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
		let current = self.device;
		let glyph = |k: DeviceKind| match k {
			DeviceKind::Computer => Icon::Computer,
			DeviceKind::Phone => Icon::Phone,
			DeviceKind::Speaker => Icon::Speaker,
			DeviceKind::Tv => Icon::Tv,
		};
		let row = |i: usize, cx: &mut Context<'_, Self>| {
			let Some(d) = DEVICES.get(i) else {
				return div().id(("device", i));
			};
			let on = i == current;
			div()
				.id(("device", i))
				.h(px(56.))
				.px_2()
				.flex()
				.items_center()
				.gap_3()
				.rounded(px(4.))
				.hover(|s| s.bg(theme::hover()))
				.cursor_pointer()
				.on_click(cx.listener(move |this, _, _, cx| {
					this.device = i;
					cx.notify();
				}))
				.child(icon(glyph(d.kind), px(24.), if on { theme::accent() } else { theme::text() }))
				.child(
					div()
						.flex()
						.flex_col()
						.child(div().text_size(px(16.)).when(on, |s| s.text_color(theme::accent())).child(d.name))
						.when(on, |s| s.child(div().text_size(px(14.)).text_color(theme::accent()).child("Playing here"))),
				)
		};
		let cur = DEVICES.get(current);
		div()
			.flex()
			.flex_col()
			.px_2()
			.pb_4()
			.when_some(cur, |s, d| {
				s.child(
					div()
						.mx_2()
						.mb_2()
						.p_4()
						.rounded(px(8.))
						.bg(theme::alpha(theme::accent(), 0.12))
						.flex()
						.items_center()
						.gap_3()
						.child(icon(glyph(d.kind), px(32.), theme::accent()))
						.child(
							div()
								.flex()
								.flex_col()
								.child(div().text_size(px(14.)).child("Current device"))
								.child(div().text_size(px(16.)).font_weight(FontWeight::BOLD).text_color(theme::accent()).child(d.name)),
						),
				)
			})
			.child(heading("Select another device"))
			.children((0..DEVICES.len()).filter(|&i| i != current).map(|i| row(i, cx)))
			.into_any_element()
	}
}
