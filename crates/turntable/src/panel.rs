//! The right-hand panel: now playing, queue, lyrics and devices. It slides and fades in whenever
//! what it shows changes.

use std::time::Duration;

use gpui_kit::{
	Animation, AnimationExt as _, AnyElement, Context, Div, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
	StatefulInteractiveElement as _, Styled as _, div, ease_out_quint, prelude::FluentBuilder as _, px,
};
use turntable_core::{
	LyricsState, Msg, Panel, Route,
	app::connect::DeviceList,
	engine::{CoverSize, Entity},
	sources::external::Lyrics,
	spotify::{id_of, pick_image},
};

use crate::{
	components::grouped,
	shell::{Shell, Side},
	theme,
	widgets::{Icon, added_badge, cover, icon, icon_button},
};

/// How long a panel takes to slide and fade in.
const ENTER: Duration = Duration::from_millis(260);
/// Upcoming tracks the queue lists.
const QUEUE_SHOWN: usize = 40;

fn heading(text: impl Into<gpui_kit::SharedString>) -> Div {
	div().px_2().pt_4().pb_2().text_size(px(16.)).font_weight(FontWeight::BOLD).child(text.into())
}

fn note(text: impl Into<gpui_kit::SharedString>) -> AnyElement {
	div().p_4().text_size(px(14.)).text_color(theme::subdued()).child(text.into()).into_any_element()
}

impl Shell {
	pub fn right_panel(&mut self, side: Side, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let st = self.st();
		let title: String = match side {
			Side::Core(Panel::NowPlaying) => {
				[st.context.name(), st.item.parent_name()].into_iter().find(|n| !n.is_empty()).unwrap_or("Now playing").to_owned()
			},
			Side::Core(Panel::Lyrics) => "Lyrics".to_owned(),
			Side::Devices => "Connect to a device".to_owned(),
			Side::Core(_) => "Queue".to_owned(),
		};
		let body = match side {
			Side::Core(Panel::NowPlaying) => self.now_playing_body(cx),
			Side::Core(Panel::Lyrics) => self.lyrics_body(),
			Side::Devices => self.devices_body(cx),
			Side::Core(_) => self.queue_body(cx),
		};
		div()
			.w(theme::RIGHT_W)
			.flex_none()
			.h_full()
			.relative()
			.rounded(theme::RADIUS)
			.bg(theme::panel())
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
						cx.listener(move |this, _, _, cx| this.toggle_side(side, cx)),
					)),
			)
			// Lyrics scroll themselves (to follow the sung line); the rest scroll here.
			.child(
				div()
					.id(("panel-scroll", self.panel_gen))
					.flex_1()
					.min_h_0()
					.when(side != Side::Core(Panel::Lyrics), |s| s.overflow_y_scroll())
					.child(body),
			)
			.child(crate::widgets::corner_mask())
			.with_animation(("panel", self.panel_gen), Animation::new(ENTER).with_easing(ease_out_quint()), |el, d| {
				el.opacity(d).relative().left(px(32. * (1. - d)))
			})
	}

	#[expect(clippy::too_many_lines, reason = "keep the declarative now-playing panel sections together")]
	fn now_playing_body(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
		let st = self.st();
		let item = st.item.clone();
		if item.uri.is_empty() {
			return note("Nothing playing");
		}
		let liked = st.liked();
		let artist_id = item.decorations.creators.first().map(|c| id_of(&c.entity.uri).to_owned());
		let artist = artist_id.as_ref().and_then(|id| st.library.artists.get(id).cloned());
		let overview = artist_id.as_ref().and_then(|id| st.library.overviews.get(id).cloned());
		let next = st.upcoming.first().map(|q| q.item.clone());
		let big = item.cover(CoverSize::Large).map(str::to_owned);
		let cover_w = f32::from(theme::RIGHT_W) - 32.;
		#[expect(
			clippy::cast_possible_truncation,
			clippy::cast_sign_loss,
			reason = "the fixed panel width minus padding is positive and bounded to u32 pixels"
		)]
		let cover_pixels = cover_w as u32;
		let image = self.image(big.as_deref(), cover_pixels, cx);
		let about = artist.map(|a| {
			let id = artist_id.clone().unwrap_or_default();
			let photo = overview
				.as_ref()
				.and_then(|o| o.header.clone().or(o.avatar.clone()))
				.or_else(|| pick_image(&a.images, 640).map(|i| i.url.clone()));
			let photo = self.image(photo.as_deref(), cover_pixels, cx);
			let listeners = overview.as_ref().and_then(|o| o.monthly_listeners);
			let bio = overview.as_ref().and_then(|o| o.biography.clone()).or_else(|| (!a.genres.is_empty()).then(|| a.genres.join(", ")));
			div()
				.id("about")
				.rounded(px(8.))
				.overflow_hidden()
				.bg(theme::raised())
				.cursor_pointer()
				.on_click(cx.listener(move |this, _, _, cx| this.go(Route::Artist(id.clone()), cx)))
				.child(
					div()
						.relative()
						.h(px(200.))
						.bg(theme::raised_hover())
						.when_some(photo, |s, src| s.child(gpui_kit::img(src).size_full().rounded_t(px(8.))))
						.child(
							div().absolute().top_4().left_4().text_size(px(16.)).font_weight(FontWeight::BOLD).child("About the artist"),
						),
				)
				.child(
					div()
						.p_4()
						.flex()
						.flex_col()
						.gap_2()
						.child(div().text_size(px(16.)).font_weight(FontWeight::BOLD).child(a.name))
						.when_some(listeners, |s, n| {
							s.child(
								div().text_size(px(14.)).text_color(theme::subdued()).child(format!("{} monthly listeners", grouped(n))),
							)
						})
						.when_some(bio, |s, b| s.child(div().text_size(px(14.)).text_color(theme::subdued()).line_clamp(3).child(b))),
				)
		});
		let next_row = next.map(|e| self.queue_row(0, &e, false, cx));
		div()
			.flex()
			.flex_col()
			.gap_4()
			.px_4()
			.pb_4()
			.child(cover(image, theme::RAISED, px(cover_w), px(8.)).shadow_lg())
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
							.child(div().text_size(px(24.)).font_weight(FontWeight::BOLD).truncate().child(item.name().to_owned()))
							.child(match &artist_id {
								Some(id) => self
									.link("np-artist", item.creators(), Route::Artist(id.clone()), cx)
									.text_size(px(16.))
									.text_color(theme::subdued())
									.into_any_element(),
								None => div().text_size(px(16.)).text_color(theme::subdued()).child(item.creators()).into_any_element(),
							}),
					)
					.when_some(liked, |s, liked| {
						s.child(
							div()
								.id("np-add")
								.cursor_pointer()
								.on_click(cx.listener(|this, _, _, cx| this.send(Msg::ToggleLike, cx)))
								.child(added_badge(liked, px(24.))),
						)
					}),
			)
			.children(about)
			.when_some(next_row, |s, row| {
				s.child(
					div()
						.rounded(px(8.))
						.bg(theme::raised())
						.p_2()
						.flex()
						.flex_col()
						.child(div().px_2().pt_2().pb_1().text_size(px(16.)).font_weight(FontWeight::BOLD).child("Next in queue"))
						.child(row),
				)
			})
			.into_any_element()
	}

	/// A compact track line: cover, title, artists. `jump` is its queue position (`None`: now playing).
	fn queue_row(&self, key: usize, e: &Entity, current: bool, cx: &mut Context<'_, Self>) -> AnyElement {
		let playing = current && self.st().playing();
		let url = e.cover(CoverSize::Small).map(str::to_owned);
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
			.on_click(cx.listener(move |this, _, _, cx| this.send(if current { Msg::TogglePlay } else { Msg::QueueJump(key) }, cx)))
			.child(
				div().relative().child(cover(self.image(url.as_deref(), 40, cx), theme::RAISED, px(40.), px(4.))).child(
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
					.child(div().text_size(px(16.)).truncate().when(current, |s| s.text_color(theme::accent())).child(e.name().to_owned()))
					.child(div().text_size(px(14.)).text_color(theme::subdued()).truncate().child(e.creators())),
			)
			.into_any_element()
	}

	fn queue_body(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
		let st = self.st();
		if st.item.uri.is_empty() {
			return note("Your queue is empty");
		}
		let item = st.item.clone();
		let context = st.context.name().to_owned();
		let upcoming = st.upcoming.clone();
		let (mine, rest): (Vec<_>, Vec<_>) = upcoming.iter().enumerate().take(QUEUE_SHOWN).partition(|(_, q)| q.source == "queue");
		let mine: Vec<_> = mine.iter().map(|(i, q)| self.queue_row(*i, &q.item, false, cx)).collect();
		let rest: Vec<_> = rest.iter().map(|(i, q)| self.queue_row(*i, &q.item, false, cx)).collect();
		div()
			.flex()
			.flex_col()
			.px_2()
			.pb_4()
			.child(heading("Now playing"))
			.child(self.queue_row(usize::MAX, &item, true, cx))
			.when(!mine.is_empty(), |s| {
				s.child(
					div().flex().items_center().justify_between().child(heading("Next in queue")).child(
						div()
							.id("queue-clear")
							.px_2()
							.text_size(px(14.))
							.font_weight(FontWeight::BOLD)
							.text_color(theme::subdued())
							.hover(|s| s.text_color(theme::text()))
							.cursor_pointer()
							.on_click(cx.listener(|this, _, _, cx| this.send(Msg::QueueClear, cx)))
							.child("Clear queue"),
					),
				)
				.children(mine)
			})
			.when(!rest.is_empty(), |s| {
				s.child(heading(if context.is_empty() { "Next up".to_owned() } else { format!("Next from: {context}") })).children(rest)
			})
			.into_any_element()
	}

	fn lyrics_body(&mut self) -> AnyElement {
		let st = self.st();
		let lines: Vec<String> = match st.lyrics_state() {
			None | Some(LyricsState::Loading) => return note("Loading lyrics…"),
			Some(LyricsState::Missing) => return note("No lyrics for this song."),
			Some(LyricsState::Failed) => return note("Couldn't load lyrics."),
			Some(LyricsState::Found(Lyrics::Plain(text))) => text.lines().map(str::to_owned).collect(),
			Some(LyricsState::Found(Lyrics::Synced(lines))) => {
				lines.iter().map(|(_, l)| if l.trim().is_empty() { "♪".to_owned() } else { l.clone() }).collect()
			},
		};
		let synced = st.synced_lyrics().is_some();
		let line = st.lyric_line();
		// Keep the sung line in view, a couple of lines down from the top.
		if let Some(line) = line
			&& self.lyrics_line != Some(line)
		{
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
			.children(lines.into_iter().enumerate().map(|(i, l)| {
				let color = match line.map(|n| i.cmp(&n)) {
					_ if !synced => theme::text().into(),
					Some(std::cmp::Ordering::Equal) => theme::text().into(),
					Some(std::cmp::Ordering::Less) => theme::alpha(theme::text(), 0.55),
					_ => theme::alpha(theme::text(), 0.3),
				};
				div().text_color(color).child(l)
			}))
			.into_any_element()
	}

	fn devices_body(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
		let st = self.st();
		let devices = match &st.connect.devices {
			DeviceList::Unknown => return note("Looking for devices…"),
			DeviceList::Failed(e) => return note(e.clone()),
			DeviceList::Ready(list) => list.clone(),
		};
		let this_name = st.device_name.clone();
		let current = devices.iter().find(|d| d.is_active).cloned();
		let glyph = |kind: &str| match kind.to_ascii_lowercase().as_str() {
			"smartphone" | "tablet" => Icon::Phone,
			"tv" | "castvideo" | "stb" => Icon::Tv,
			"speaker" | "castaudio" | "avr" | "audiodongle" => Icon::Speaker,
			_ => Icon::Computer,
		};
		let rows: Vec<AnyElement> = devices
			.iter()
			.filter(|d| !d.is_active)
			.enumerate()
			.map(|(i, d)| {
				let name = d.name.clone();
				let id = d.id.clone();
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
					.on_click(cx.listener(move |this, _, _, cx| this.send(Msg::PickDevice(id.clone()), cx)))
					.child(icon(glyph(&d.kind), px(24.), theme::text()))
					.child(div().text_size(px(16.)).child(if name == this_name { format!("{name} (this computer)") } else { name }))
					.into_any_element()
			})
			.collect();
		div()
			.flex()
			.flex_col()
			.px_2()
			.pb_4()
			.when_some(current, |s, d| {
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
						.child(icon(glyph(&d.kind), px(32.), theme::accent()))
						.child(div().flex().flex_col().child(div().text_size(px(14.)).child("Current device")).child(
							div().text_size(px(16.)).font_weight(FontWeight::BOLD).text_color(theme::accent()).child(d.name.clone()),
						)),
				)
			})
			.when(!rows.is_empty(), |s| s.child(heading("Select another device")).children(rows))
			.when(devices.is_empty(), |s| s.child(note("No other devices found. Open Spotify on your phone or speaker to see it here.")))
			.into_any_element()
	}
}
