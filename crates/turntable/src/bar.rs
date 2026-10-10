//! The player bar: now playing on the left, transport and seek in the middle, panels and volume on the right.

use std::time::Duration;

use gpui_kit::{
	Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render, StatefulInteractiveElement as _, StyleRefinement,
	Styled as _, Subscription, Task, Window, div, prelude::FluentBuilder as _, px, relative,
};
use turntable_core::{
	Msg, Panel, Route,
	app::{Anchor, State, clock},
	engine::{CoverSize, Repeat},
	spotify::id_of,
};

use crate::{
	shell::{Shell, Side},
	theme,
	widgets::{Icon, Slider, added_badge, cover, icon, icon_button},
};

/// The bar's side columns: 30% of the width, at least this wide.
const SIDE_MIN_W: f32 = 180.;

/// The only player view that repaints on its own while playing. Sampling the core's monotonic
/// anchor here redraws the seek slider and its labels, not the library tree, and never pumps the core.
pub struct Playhead {
	seek:           Entity<Slider>,
	anchor:         Anchor,
	duration_ms:    u64,
	playing:        bool,
	repaint:        Option<Task<()>>,
	elapsed_second: Option<u64>,
	elapsed_label:  gpui_kit::SharedString,
	duration_label: gpui_kit::SharedString,
	_visibility:    Subscription,
}

/// The shortest gap between repaints (about 60/s).
const MIN_REPAINT: Duration = Duration::from_millis(16);

/// How long until the playhead next moves a device pixel or the elapsed label ticks over,
/// whichever is sooner. `track_px` is an upper bound on the slider's width in device pixels,
/// so this never waits past a visible step. A 3-minute song on a 1280 px window repaints about
/// 7 times a second instead of every display refresh.
fn repaint_delay(duration_ms: u64, speed: f64, now_ms: u64, track_px: f32) -> Duration {
	let per_pixel_ms = duration_ms as f64 / speed.max(f64::EPSILON) / f64::from(track_px.max(1.));
	let to_next_second_ms = (1000 - now_ms % 1000) as f64 / speed.max(f64::EPSILON);
	Duration::from_secs_f64(per_pixel_ms.min(to_next_second_ms) / 1000.).max(MIN_REPAINT)
}

impl Playhead {
	pub fn new(seek: Entity<Slider>, state: &State, window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
		let view = cx.weak_entity();
		let visibility = window.observe_window_visibility(move |_, window, cx| {
			if window.is_visible() {
				// A fully occluded window stops the repaint chain; restart it.
				let _ = view.update(cx, |_, cx| cx.notify());
			}
		});
		Self {
			seek,
			anchor: state.anchor,
			duration_ms: state.duration_ms(),
			playing: state.playing(),
			repaint: None,
			elapsed_second: None,
			elapsed_label: gpui_kit::SharedString::default(),
			duration_label: clock(state.duration_ms()).into(),
			_visibility: visibility,
		}
	}

	pub fn sync(&mut self, state: &State, cx: &mut Context<'_, Self>) {
		self.anchor = state.anchor;
		if self.duration_ms != state.duration_ms() {
			self.duration_label = clock(state.duration_ms()).into();
		}
		self.duration_ms = state.duration_ms();
		self.playing = state.playing();
		// Restart from the new anchor rather than finishing a delay computed from the old one.
		self.repaint = None;
		cx.notify();
	}

	fn animating(&self, window: &Window) -> bool {
		self.playing && self.anchor.speed > 0.0 && self.duration_ms > 0 && window.is_visible()
	}

	fn animate(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
		if self.repaint.is_some() || !self.animating(window) {
			return;
		}
		let track_px = f32::from(window.viewport_size().width) * window.scale_factor();
		let delay = repaint_delay(self.duration_ms, self.anchor.speed, self.anchor.now_ms(), track_px);
		self.repaint = Some(cx.spawn_in(window, async move |this, cx| {
			cx.background_executor().timer(delay).await;
			let _ = this.update(cx, |this, cx| {
				this.repaint = None;
				cx.notify();
			});
		}));
	}
}

impl Render for Playhead {
	fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let at = self.anchor.now_ms().min(self.duration_ms);
		// Formatting labels once per second avoids allocating strings on every animation frame.
		let second = at / 1000;
		if self.elapsed_second != Some(second) {
			self.elapsed_second = Some(second);
			self.elapsed_label = clock(at).into();
		}
		let progress = if self.duration_ms == 0 { 0. } else { at as f32 / self.duration_ms as f32 };
		self.seek.update(cx, |slider, cx| slider.set(progress, cx));
		self.animate(window, cx);
		div()
			.w_full()
			.flex()
			.items_center()
			.gap_2()
			.text_size(px(12.))
			.text_color(theme::subdued())
			.child(div().w(px(40.)).flex().justify_end().child(self.elapsed_label.clone()))
			.child(div().flex_1().child(self.seek.clone()))
			.child(div().w(px(40.)).child(self.duration_label.clone()))
	}
}

impl Shell {
	pub fn player_bar(&self) -> impl IntoElement {
		div()
			.h(theme::BAR_H)
			.flex_none()
			.flex()
			.items_center()
			.justify_between()
			.px(theme::GAP)
			.child(self.regions.now_playing.clone().cached(StyleRefinement::default().w(relative(0.3)).min_w(px(SIDE_MIN_W)).h_full()))
			.child(self.bar_transport())
			.child(self.regions.extras.clone().cached(StyleRefinement::default().w(relative(0.3)).min_w(px(SIDE_MIN_W)).h_full()))
	}

	pub(crate) fn bar_now(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let side = div().w_full().h_full().flex().items_center().gap_3().pl_2();
		let item = &self.st().item;
		if item.uri.is_empty() {
			return side;
		}
		let title = item.name().to_owned();
		let artists = item.creators();
		let artist = item.decorations.creators.first().map(|c| id_of(&c.entity.uri).to_owned());
		let album = item.decorations.parent.as_ref().map(|p| id_of(&p.entity.uri).to_owned());
		let url = item.cover(CoverSize::Small).map(str::to_owned);
		// Spotify's ✓: in Liked Songs or any of the user's playlists (None: not a track).
		let added = self.st().liked().map(|liked| liked || self.st().added(&item.uri));
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
		.when_some(added, |s, added| {
			s.child(
				div()
					.id("bar-add")
					.size(px(32.))
					.flex()
					.flex_none()
					.items_center()
					.justify_center()
					.cursor_pointer()
					// ⊕ saves to Liked Songs; ✓ opens the playlist picker (where it can be taken out).
					.on_click(cx.listener(move |this, _, _, cx| {
						this.send(if added { Msg::AddNowPlayingToPlaylist } else { Msg::ToggleLike }, cx);
					}))
					.child(added_badge(added, px(16.))),
			)
		})
	}

	fn bar_transport(&self) -> impl IntoElement {
		div()
			.flex_1()
			.max_w(px(722.))
			.flex()
			.flex_col()
			.items_center()
			.gap_1()
			.child(self.regions.transport.clone().cached(StyleRefinement::default().w_full().h(px(32.)).flex_none()))
			.child(self.playhead.clone())
	}

	pub(crate) fn bar_buttons(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let st = self.st();
		let playing = st.playing();
		let shuffle = st.options.shuffle;
		let repeat = st.options.repeat;
		div()
			.w_full()
			.flex()
			.items_center()
			.justify_center()
			.gap_2()
			.child(icon_button("shuffle", Icon::Shuffle, px(16.), shuffle, cx.listener(|this, _, _, cx| this.send(Msg::ToggleShuffle, cx))))
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
			))
	}

	pub(crate) fn bar_extras(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let side = self.side();
		let muted = self.st().volume == 0;
		let remote = self.st().remote().map(|d| d.name.clone());
		let mini = self.mini.is_some();
		let panel_button = |id: &'static str, glyph: Icon, s: Side, cx: &mut Context<'_, Self>| {
			icon_button(id, glyph, px(16.), side == Some(s), cx.listener(move |this, _, _, cx| this.toggle_side(s, cx)))
		};
		div()
			.w_full()
			.h_full()
			.flex()
			.flex_col()
			.items_end()
			.justify_center()
			.child(
				div()
					.flex()
					.items_center()
					.justify_end()
					.pr_2()
					// Spotify's order: lyrics, queue, devices, volume, miniplayer, full screen. Now
					// playing is the panel's resting state, so it has no button of its own.
					.child(panel_button("lyrics", Icon::Mic, Side::Core(Panel::Lyrics), cx))
					.child(panel_button("queue", Icon::Queue, Side::Core(Panel::Queue), cx))
					.child(panel_button("devices", Icon::Device, Side::Devices, cx))
					.child(icon_button(
						"mute",
						if muted { Icon::Mute } else { Icon::Volume },
						px(16.),
						false,
						cx.listener(|this, _, _, cx| this.send(Msg::ToggleMute, cx)),
					))
					.child(div().w(px(93.)).child(self.volume_slider.clone()))
					.child(icon_button(
						"miniplayer",
						Icon::Miniplayer,
						px(16.),
						mini,
						cx.listener(|_, _, _, cx| {
							let shell = cx.entity();
							cx.defer(move |cx| crate::miniplayer::MiniPlayer::toggle(&shell, cx));
						}),
					))
					.child(icon_button(
						"fullscreen",
						Icon::Fullscreen,
						px(16.),
						false,
						cx.listener(|this, _, window, cx| this.toggle_full_screen(window, cx)),
					)),
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

#[cfg(test)]
mod tests {
	use gpui_kit::{AppContext as _, component::Root, size};
	use gpui_test::{TestAppContext, VisualTestContext};
	use turntable_core::engine::Status;

	use super::*;

	#[test]
	fn repaints_when_the_playhead_moves_a_pixel_or_the_label_ticks() {
		let ms = |d: Duration| d.as_millis();
		// 200 s over 2560 device pixels: one pixel every ~78 ms.
		assert_eq!(ms(repaint_delay(200_000, 1.0, 10_000, 2560.)), 78);
		// The elapsed label's next second comes first.
		assert_eq!(ms(repaint_delay(200_000, 1.0, 10_950, 1280.)), 50);
		// Long tracks on narrow windows still tick the label every second.
		assert_eq!(ms(repaint_delay(3_600_000, 1.0, 0, 400.)), 1000);
		// Short tracks on wide displays are capped near 60 repaints a second.
		assert_eq!(repaint_delay(30_000, 1.0, 0, 5120.), MIN_REPAINT);
		// Faster playback reaches each pixel and second sooner.
		assert_eq!(ms(repaint_delay(200_000, 2.0, 10_000, 2560.)), 39);
	}

	#[test]
	fn playing_schedules_one_repaint_and_pause_stops_it_without_a_core_pump() {
		let mut state = State::default();
		state.status = Status::Playing;
		state.anchor = Anchor { position_ms: 1_500, speed: 1.0, ..Anchor::default() };
		state.item.decorations.playback.duration_ms = 200_000;
		let mut app = TestAppContext::single();
		app.update(gpui_kit::init);
		let mut view = None;
		let window = app.open_window(size(px(500.), px(80.)), |window, cx| {
			let seek = Slider::new(cx, 0.);
			let entity = cx.new(|cx| Playhead::new(seek, &state, window, cx));
			view = Some(entity.clone());
			Root::new(entity, window, cx)
		});
		let view = view.unwrap();
		let mut cx = VisualTestContext::from_window(window.into(), &app);
		cx.update(|window, cx| {
			assert!(window.is_visible());
			window.draw(cx).clear(cx);
			view.update(cx, |view, cx| {
				assert!(view.repaint.is_some());
				assert_eq!(view.elapsed_label.as_ref(), "0:01");
				assert_eq!(view.duration_label.as_ref(), "3:20");
				state.status = Status::Paused;
				state.anchor = Anchor { position_ms: 5_000, ..Anchor::default() };
				view.sync(&state, cx);
			});
			window.draw(cx).clear(cx);
			view.read_with(cx, |view, _| {
				assert!(view.repaint.is_none());
				assert_eq!(view.elapsed_label.as_ref(), "0:05");
			});
		});
	}
}
