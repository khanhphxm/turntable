//! Primitives every view uses: icons, icon buttons, covers, chips and the bar slider.

use std::{cell::Cell, rc::Rc};

use gpui_kit::{
	App, AppContext as _, Bounds, ClickEvent, Context, DispatchPhase, Div, ElementId, Entity, EventEmitter, FontWeight, Hsla,
	InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point, Render,
	SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, StyledImage as _, Svg, Window, canvas, div,
	prelude::FluentBuilder as _, px, relative, rgb, svg,
};

pub use crate::assets::Icon;
use crate::theme;

/// Our glyphs leave ~2.5 px of air inside their 24 px grid, where Spotify's fill their box; a
/// button's glyph is drawn this much larger so the two read at the same size.
pub const OPTICAL: f32 = 1.2;

pub fn icon(icon: Icon, size: Pixels, color: impl Into<Hsla>) -> Svg {
	svg().path(icon.path()).size(size).flex_none().text_color(color)
}

/// A bare icon button: subdued, white on hover, green with a dot under it while `active`.
/// The hit area is the icon plus 8 px on each side.
pub fn icon_button(
	id: impl Into<ElementId>,
	glyph: Icon,
	size: Pixels,
	active: bool,
	on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
	let color = if active { theme::accent() } else { theme::subdued() };
	div()
		.id(id)
		.group("icon-button")
		.relative()
		.flex()
		.flex_none()
		.items_center()
		.justify_center()
		.size(size + px(16.))
		.cursor_pointer()
		.on_click(on_click)
		.child(icon(glyph, size * OPTICAL, color).when(!active, |s| s.group_hover("icon-button", |s| s.text_color(theme::text()))))
		// Spotify marks an active toggle with a small dot under the icon.
		.when(active, |el| el.child(div().absolute().bottom(px(0.)).size(px(4.)).rounded_full().bg(theme::accent())))
}

/// A round button in a raised fill (top bar's home, back and forward).
pub fn round_button(
	id: impl Into<ElementId>,
	glyph: Icon,
	diameter: Pixels,
	icon_size: Pixels,
	on: bool,
	on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
	div()
		.id(id)
		.group("round-button")
		.size(diameter)
		.flex_none()
		.rounded_full()
		.flex()
		.items_center()
		.justify_center()
		.bg(theme::raised())
		.hover(|s| s.bg(theme::raised_hover()))
		.cursor_pointer()
		.on_click(on_click)
		.child(
			icon(glyph, icon_size * OPTICAL, if on { theme::text() } else { theme::subdued() })
				.group_hover("round-button", |s| s.text_color(theme::text())),
		)
}

/// Spotify's green play/pause disc (listing headers, cards).
pub fn play_disc(
	id: impl Into<ElementId>,
	diameter: Pixels,
	playing: bool,
	on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
	div()
		.id(id)
		.size(diameter)
		.flex_none()
		.rounded_full()
		.bg(theme::accent())
		.flex()
		.items_center()
		.justify_center()
		.cursor_pointer()
		.shadow_md()
		.hover(|s| s.bg(theme::accent_hover()))
		.on_click(on_click)
		.child(icon(if playing { Icon::Pause } else { Icon::Play }, diameter * 0.43, theme::base()))
}

/// A selectable pill (library filters, EQ presets, search filters).
pub fn chip(
	id: impl Into<ElementId>,
	label: impl Into<SharedString>,
	selected: bool,
	on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
	div()
		.id(id)
		.h(px(32.))
		.px_3()
		.flex()
		.flex_none()
		.items_center()
		.rounded_full()
		.text_size(px(14.))
		.cursor_pointer()
		.when(selected, |s| s.bg(theme::text()).text_color(theme::base()))
		.when(!selected, |s| s.bg(theme::raised()).hover(|s| s.bg(theme::raised_hover())))
		.on_click(on_click)
		.child(label.into())
}

/// Artwork: the image, or until there is one a tile in the artwork's colour.
pub fn cover(image: Option<gpui_kit::ImageSource>, color: u32, size: Pixels, radius: Pixels) -> Div {
	let frame = div().size(size).flex_none().rounded(radius).overflow_hidden().bg(rgb(color));
	match image {
		// GPUI clips children to rectangles: the image rounds its own corners.
		Some(src) => frame.child(gpui_kit::img(src).size_full().object_fit(gpui_kit::ObjectFit::Cover).rounded(radius)),
		None => frame,
	}
}

/// The ✓ / ⊕ toggle: a green disc with a check once the track is in Liked Songs or any of the
/// user's playlists, else a ring with a plus (shown on hover by rows, always by the bar).
pub fn added_badge(added: bool, size: Pixels) -> Div {
	if added {
		div().size(size).flex_none().rounded_full().bg(theme::accent()).flex().items_center().justify_center().child(icon(
			Icon::Check,
			size * 0.7,
			theme::base(),
		))
	} else {
		div().size(size).flex_none().flex().items_center().justify_center().child(icon(Icon::Save, size, theme::subdued()))
	}
}

pub fn section_title(text: impl Into<SharedString>) -> Div {
	div().text_size(px(24.)).font_weight(FontWeight::BOLD).child(text.into())
}

/// What a [`Slider`] tells its owner.
pub enum SliderEvent {
	/// Dragging: the value under the pointer (0..=1), not yet committed.
	Scrub(f32),
	/// Released (or clicked): the value to apply.
	Commit(f32),
}

/// Spotify's bar slider: a 4 px track that turns green with a knob on hover or drag.
pub struct Slider {
	value:    f32,
	/// The value under the pointer while dragging.
	dragging: Option<f32>,
	hovered:  bool,
	motion:   Option<crate::animation::Motion>,
	bounds:   Rc<Cell<Bounds<Pixels>>>,
}

impl EventEmitter<SliderEvent> for Slider {}

impl Slider {
	pub fn new(cx: &mut App, value: f32) -> Entity<Self> {
		cx.new(|_| Self { value, dragging: None, hovered: false, motion: None, bounds: Rc::default() })
	}

	/// The owner's value, unless a drag holds the knob.
	pub fn set(&mut self, value: f32, cx: &mut Context<'_, Self>) {
		if (self.value - value).abs() > f32::EPSILON {
			self.value = value;
			if self.dragging.is_none() {
				cx.notify();
			}
		}
	}

	pub fn follow(&mut self, motion: crate::animation::Motion, cx: &mut Context<'_, Self>) {
		self.motion = Some(motion);
		self.set(motion.progress(), cx);
	}

	pub fn is_dragging(&self) -> bool {
		self.dragging.is_some()
	}

	fn at(&self, p: Point<Pixels>) -> f32 {
		let b = self.bounds.get();
		if b.size.width <= px(0.) {
			return self.value;
		}
		((p.x - b.origin.x) / b.size.width).clamp(0., 1.)
	}

	fn scrub(&mut self, p: Point<Pixels>, cx: &mut Context<'_, Self>) {
		let v = self.at(p);
		self.dragging = Some(v);
		cx.emit(SliderEvent::Scrub(v));
		cx.notify();
	}

	/// While a drag is on, every move and the release anywhere in the window belong to it.
	fn capture(entity: &Entity<Self>, window: &mut Window) {
		let moves = entity.clone();
		window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
			if phase == DispatchPhase::Bubble && e.dragging() {
				moves.update(cx, |this, cx| {
					if this.dragging.is_some() {
						this.scrub(e.position, cx);
					}
				});
			}
		});
		let ups = entity.clone();
		window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
			if phase == DispatchPhase::Bubble && e.button == MouseButton::Left {
				ups.update(cx, |this, cx| {
					if this.dragging.take().is_some() {
						let v = this.at(e.position);
						this.value = v;
						cx.emit(SliderEvent::Commit(v));
						cx.notify();
					}
				});
			}
		});
	}
}

impl Render for Slider {
	fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let shown = self.dragging.unwrap_or_else(|| self.motion.map_or(self.value, crate::animation::Motion::progress));
		let active = self.dragging.is_some();
		let bounds = self.bounds.clone();
		let entity = active.then(|| cx.entity());
		let foreground = match self.motion {
			Some(motion) => crate::animation::progress(motion, self.hovered, self.dragging),
			None => slider_foreground(shown, active),
		};
		div()
			.id("slider")
			.group("slider")
			.relative()
			.flex()
			.items_center()
			.justify_center()
			.cursor_pointer()
			.h(px(16.))
			.w_full()
			.child(
				canvas(
					move |b, _, _| bounds.set(b),
					move |_, (), window, _| {
						if let Some(entity) = &entity {
							Self::capture(entity, window);
						}
					},
				)
				.absolute()
				.size_full(),
			)
			.child(div().relative().h(px(4.)).w_full().rounded_full().bg(theme::track()))
			.child(div().absolute().size_full().child(foreground))
			.on_hover(cx.listener(|this, hovered: &bool, _, cx| {
				if this.hovered != *hovered {
					this.hovered = *hovered;
					cx.notify();
				}
			}))
			.on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, _, cx| this.scrub(e.position, cx)))
	}
}

/// The exact GPUI foreground for volume, seek dragging and compositor fallback.
pub(crate) fn slider_foreground(shown: f32, active: bool) -> gpui_kit::AnyElement {
	let fill = div()
		.absolute()
		.left_0()
		.top_0()
		.h_full()
		.w(relative(shown))
		.rounded_full()
		.bg(if active { theme::accent() } else { theme::text() })
		.group_hover("slider", |s| s.bg(theme::accent()));
	let knob = div()
		.absolute()
		.left(relative(shown))
		.ml(px(-6.))
		.size(px(12.))
		.rounded_full()
		.bg(theme::text())
		.shadow_md()
		.when(!active, |s| s.invisible())
		.group_hover("slider", |s| s.visible());
	div()
		.relative()
		.w_full()
		.h(px(16.))
		.flex()
		.items_center()
		.justify_center()
		.child(div().relative().h(px(4.)).w_full().child(fill))
		.child(knob)
		.into_any_element()
}

/// Spotify's outlined pill button (Follow, Sign out).
pub fn outline_button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
	div()
		.id(id)
		.h(px(32.))
		.px_4()
		.flex()
		.flex_none()
		.items_center()
		.rounded_full()
		.border_1()
		.border_color(theme::faint())
		.hover(|s| s.border_color(theme::text()))
		.cursor_pointer()
		.text_size(px(14.))
		.font_weight(FontWeight::BOLD)
		.child(label.into())
}

/// Rounds a panel's corners over whatever scrolls inside it. GPUI clips children to rectangles,
/// so rows scrolled under a panel's top would show square corners; this frame, laid over the
/// panel, paints the window's colour outside the rounded outline (it takes no input).
pub fn corner_mask() -> Div {
	let w = theme::GAP;
	div().absolute().top(-w).left(-w).right(-w).bottom(-w).rounded(theme::RADIUS + w).border(w).border_color(theme::base())
}

/// A bare icon that opens a menu (the "…" of pages, rows and the sort control).
pub fn menu_icon(id: impl Into<ElementId>, glyph: Icon, size: Pixels) -> Stateful<Div> {
	div()
		.id(id)
		.group("menu-icon")
		.size(size + px(16.))
		.flex()
		.flex_none()
		.items_center()
		.justify_center()
		.cursor_pointer()
		.child(icon(glyph, size * OPTICAL, theme::subdued()).group_hover("menu-icon", |s| s.text_color(theme::text())))
}

/// Spotify's four out-of-phase bars. macOS delegates their paint lifetime and motion to Core
/// Animation; other platforms retain the phase-locked, 15-fps GPUI animation.
pub fn eq_bars(id: &'static str, size: Pixels) -> impl IntoElement {
	crate::animation::bars(id, size)
}

/// Hover state for a header that reveals an icon beside its title (Your Library, the right
/// panel). `generation` restarts the slide each time the pointer enters or leaves.
#[derive(Debug, Default, Clone, Copy)]
pub struct Reveal {
	pub on:         bool,
	pub generation: usize,
}

impl Reveal {
	/// Records the pointer entering or leaving; true when that changed anything.
	pub fn set(&mut self, on: bool) -> bool {
		if self.on == on {
			return false;
		}
		self.on = on;
		self.generation += 1;
		true
	}
}

/// Spotify's header reveal: at rest the row sits shifted left so the icon is out of sight; on
/// hover it slides right (100 ms) and then the icon fades in (100 ms); leaving reverses that. Only
/// positions and opacity animate, so the layout never changes with the pointer.
pub fn reveal_row(id: &'static str, state: Reveal, icon: impl IntoElement, title: impl IntoElement) -> impl IntoElement {
	use std::time::Duration;

	use gpui_kit::{Animation, AnimationExt as _, ease_out_quint};
	/// The icon (24 px) and the gap after it.
	const SHIFT: f32 = 32.;
	let (on, generation) = (state.on, state.generation);
	// Halves of the 200 ms: the slide, then the fade (reversed on the way out).
	let staged = move |d: f32| -> (f32, f32) {
		let (first, second) = ((d * 2.).min(1.), (d * 2. - 1.).max(0.));
		if on { (first, second) } else { (1. - second, 1. - first) }
	};
	let place = move |el: Div, slide: f32| el.left(px(-SHIFT * (1. - slide)));
	let fade = move |el: Div, shown: f32| el.opacity(shown);
	let row = div().relative().flex().items_center().gap_2().min_w_0();
	let icon = div().size(px(24.)).flex_none().flex().items_center().justify_center().child(icon);
	if generation == 0 {
		// Never hovered yet: draw the resting state without an animation.
		return place(row, 0.).child(fade(icon, 0.)).child(title).into_any_element();
	}
	let duration = Duration::from_millis(200);
	row.child(
		icon.with_animation((id, generation * 2), Animation::new(duration).with_easing(ease_out_quint()), move |el, d| {
			fade(el, staged(d).1)
		}),
	)
	.child(title)
	.with_animation((id, generation * 2 + 1), Animation::new(duration).with_easing(ease_out_quint()), move |el, d| place(el, staged(d).0))
	.into_any_element()
}
