//! Lyrics as Apple Music draws them: big bold lines over the song's cover blurred to soft
//! colour; the line being sung bright white (fading up as it's reached), every other line dim.
//! Hovering a line brightens it and a click sings from there.

use std::time::Duration;

use gpui_kit::{
	Animation, AnimationExt as _, AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
	StatefulInteractiveElement as _, Styled as _, StyledImage as _, div, ease_out_quint, prelude::FluentBuilder as _, px,
};
use turntable_core::{LyricsState, Msg, engine::CoverSize, sources::external::Lyrics};

use crate::{shell::Shell, theme};

/// How bright the lines not being sung are.
const DIM: f32 = 0.32;
/// How long the sung line takes to light up.
const LIGHT_UP: Duration = Duration::from_millis(380);

impl Shell {
	/// The song's cover blurred to soft colour and shaded, filling its parent (which must be
	/// `relative` and clip).
	pub fn lyrics_backdrop(&self, cx: &mut Context<'_, Self>) -> AnyElement {
		let url = self.st().item.cover(CoverSize::Large).map(str::to_owned);
		let art = self.blurred_image(url.as_deref(), cx);
		div()
			.absolute()
			.inset_0()
			.bg(theme::raised())
			// Oversized so the blur's soft edges fall outside.
			.when_some(art, |s, src| {
				s.child(
					gpui_kit::img(src)
						.absolute()
						.top(gpui_kit::relative(-0.15))
						.left(gpui_kit::relative(-0.15))
						.w(gpui_kit::relative(1.3))
						.h(gpui_kit::relative(1.3))
						.object_fit(gpui_kit::ObjectFit::Cover),
				)
			})
			.child(div().absolute().inset_0().bg(theme::alpha(theme::base(), 0.3)))
			.into_any_element()
	}

	/// The lyric lines at `size`, `pad` from the sides; a short note when there are none.
	pub fn lyric_lines(&mut self, size: f32, pad: f32, cx: &mut Context<'_, Self>) -> AnyElement {
		let st = self.st();
		let (lines, times): (Vec<String>, Vec<Option<u64>>) = match st.lyrics_state() {
			None | Some(LyricsState::Loading) => return lyric_note("Loading lyrics…", size),
			Some(LyricsState::Missing) => return lyric_note("No lyrics for this song.", size),
			Some(LyricsState::Failed) => return lyric_note("Couldn't load lyrics.", size),
			Some(LyricsState::Found(Lyrics::Plain(text))) => text.lines().map(|l| (l.to_owned(), None)).unzip(),
			Some(LyricsState::Found(Lyrics::Synced(lines))) => {
				lines.iter().map(|(at, l)| (if l.trim().is_empty() { "♪".to_owned() } else { l.clone() }, Some(*at))).unzip()
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
			.gap(px(size * 0.55))
			.px(px(pad))
			.pt(px(size))
			.pb(px(size * 6.))
			.text_size(px(size))
			.line_height(px(size * 1.25))
			.font_weight(FontWeight::EXTRA_BOLD)
			.children(lines.into_iter().zip(times).enumerate().map(|(i, (text, at))| {
				let current = line == Some(i);
				let row = div()
					.id(("lyric", i))
					.text_color(theme::text())
					.when(synced && !current, |s| s.opacity(DIM).hover(|s| s.opacity(0.6)))
					.when_some(at.filter(|_| synced), |s, at| {
						s.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| this.send(Msg::Seek(at), cx)))
					})
					.child(text);
				if current {
					row.with_animation(("lyric-now", i), Animation::new(LIGHT_UP).with_easing(ease_out_quint()), |el, d| {
						el.opacity(DIM + (1. - DIM) * d)
					})
					.into_any_element()
				} else {
					row.into_any_element()
				}
			}))
			.into_any_element()
	}
}

fn lyric_note(text: &'static str, size: f32) -> AnyElement {
	div()
		.p(px(size))
		.text_size(px(size * 0.7))
		.font_weight(FontWeight::BOLD)
		.text_color(theme::alpha(theme::text(), 0.7))
		.child(text)
		.into_any_element()
}
