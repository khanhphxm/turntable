//! Primitives every view uses: icons, icon buttons, covers, chips and the bar slider.

use std::{cell::Cell, rc::Rc};

use gpui_kit::{
	App, AppContext as _, Bounds, ClickEvent, Context, DispatchPhase, Div, ElementId, Entity, EventEmitter, FontWeight, Hsla,
	InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point,
	Render, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, Svg, Window, canvas, div, prelude::FluentBuilder as _,
	px, relative, rgb, svg,
};

pub use crate::assets::Icon;
use crate::theme;

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
		.child(icon(glyph, size, color).when(!active, |s| s.group_hover("icon-button", |s| s.text_color(theme::text()))))
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
			icon(glyph, icon_size, if on { theme::text() } else { theme::subdued() })
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
		Some(src) => frame.child(gpui_kit::img(src).size_full()),
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
/// Horizontal (seek, volume) or vertical (EQ bands, 0 at the bottom).
pub struct Slider {
	value:    f32,
	vertical: bool,
	/// The value under the pointer while dragging.
	dragging: Option<f32>,
	bounds:   Rc<Cell<Bounds<Pixels>>>,
}

impl EventEmitter<SliderEvent> for Slider {}

impl Slider {
	pub fn new(cx: &mut App, value: f32) -> Entity<Self> {
		cx.new(|_| Self { value, vertical: false, dragging: None, bounds: Rc::default() })
	}

	pub fn vertical(cx: &mut App, value: f32) -> Entity<Self> {
		cx.new(|_| Self { value, vertical: true, dragging: None, bounds: Rc::default() })
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

	fn at(&self, p: Point<Pixels>) -> f32 {
		let b = self.bounds.get();
		if self.vertical {
			if b.size.height <= px(0.) {
				return self.value;
			}
			(1. - (p.y - b.origin.y) / b.size.height).clamp(0., 1.)
		} else {
			if b.size.width <= px(0.) {
				return self.value;
			}
			((p.x - b.origin.x) / b.size.width).clamp(0., 1.)
		}
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
		let shown = self.dragging.unwrap_or(self.value);
		let active = self.dragging.is_some();
		let vertical = self.vertical;
		let bounds = self.bounds.clone();
		let entity = active.then(|| cx.entity());
		let fill = div()
			.absolute()
			.rounded_full()
			.bg(if active { theme::accent() } else { theme::text() })
			.group_hover("slider", |s| s.bg(theme::accent()));
		let fill =
			if vertical { fill.left_0().bottom_0().w_full().h(relative(shown)) } else { fill.left_0().top_0().h_full().w(relative(shown)) };
		let knob = div()
			.absolute()
			.size(px(12.))
			.rounded_full()
			.bg(theme::text())
			.shadow_md()
			.when(!active, |s| s.invisible())
			.group_hover("slider", |s| s.visible());
		let knob = if vertical { knob.bottom(relative(shown)).mb(px(-6.)) } else { knob.left(relative(shown)).ml(px(-6.)) };
		let rail = div().relative().rounded_full().bg(theme::track()).child(fill);
		let rail = if vertical { rail.w(px(4.)).h_full() } else { rail.h(px(4.)).w_full() };
		div()
			.id("slider")
			.group("slider")
			.relative()
			.flex()
			.items_center()
			.justify_center()
			.cursor_pointer()
			.map(|s| if vertical { s.w(px(16.)).h_full() } else { s.h(px(16.)).w_full() })
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
			.child(rail)
			.child(knob)
			.on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, _, cx| this.scrub(e.position, cx)))
	}
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
