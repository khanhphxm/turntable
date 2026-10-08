//! The equalizer's curve: ten bands drawn as a smooth line over a dB grid, each a knob to drag
//! up or down. The band under the pointer shows its gain; every band's gain is listed under its
//! frequency.

use std::{cell::Cell, rc::Rc};

use gpui_kit::{
	App, AppContext as _, Bounds, Context, DispatchPhase, Entity, EventEmitter, FontWeight, InteractiveElement as _, IntoElement,
	MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, PathBuilder, Pixels, Point, Render,
	StatefulInteractiveElement as _, Styled as _, Window, canvas, div, point, prelude::FluentBuilder as _, px, relative,
};
use turntable_core::app::{eq_db_step, eq_step_db};

use crate::theme;

const BANDS: [&str; 10] = ["32", "64", "125", "250", "500", "1K", "2K", "4K", "8K", "16K"];
/// The grid's range, ± dB.
const RANGE_DB: f32 = 12.;
/// Grid lines, dB.
const GRID: [f32; 5] = [12., 6., 0., -6., -12.];
const PLOT_H: f32 = 200.;
const KNOB: f32 = 14.;
/// How far from a knob (horizontally) the pointer still picks its band.
const PICK: f32 = 28.;

/// A band moved to a new slider step (the core's 2 dB steps).
pub struct BandChanged {
	pub band: usize,
	pub step: u8,
}

pub struct EqGraph {
	gains:    [f32; 10],
	enabled:  bool,
	hovered:  Option<usize>,
	dragging: Option<usize>,
	/// The plot's bounds as of the last paint (pointer → band and gain).
	bounds:   Rc<Cell<Bounds<Pixels>>>,
}

impl EventEmitter<BandChanged> for EqGraph {}

/// "+4 dB", "0 dB", "−6 dB".
fn db_label(db: f32) -> String {
	let r = db.round();
	if r > 0. {
		format!("+{r} dB")
	} else if r < 0. {
		format!("−{} dB", -r)
	} else {
		"0 dB".to_owned()
	}
}

/// Where `db` sits on the plot, 0 (top, +12) to 1 (bottom, −12).
fn y_frac(db: f32) -> f32 {
	(RANGE_DB - db.clamp(-RANGE_DB, RANGE_DB)) / (2. * RANGE_DB)
}

/// Band `i`'s horizontal position, 0 to 1.
#[expect(clippy::cast_precision_loss, reason = "band index < 10")]
fn x_frac(i: usize) -> f32 {
	i as f32 / (BANDS.len() - 1) as f32
}

impl EqGraph {
	pub fn new(cx: &mut App, gains: [f32; 10], enabled: bool) -> Entity<Self> {
		cx.new(|_| Self { gains, enabled, hovered: None, dragging: None, bounds: Rc::default() })
	}

	/// The core's gains; a band being dragged keeps the pointer's value until release.
	pub fn set(&mut self, gains: [f32; 10], enabled: bool, cx: &mut Context<'_, Self>) {
		let mut next = gains;
		if let Some(b) = self.dragging
			&& let (Some(n), Some(old)) = (next.get_mut(b), self.gains.get(b))
		{
			*n = *old;
		}
		if next != self.gains || enabled != self.enabled {
			self.gains = next;
			self.enabled = enabled;
			cx.notify();
		}
	}

	fn band_at(&self, p: Point<Pixels>) -> Option<usize> {
		let b = self.bounds.get();
		if b.size.width <= px(0.) {
			return None;
		}
		let x = f32::from(p.x - b.origin.x);
		let w = f32::from(b.size.width);
		(0..BANDS.len())
			.map(|i| (i, (x - x_frac(i) * w).abs()))
			.filter(|(_, d)| *d <= PICK)
			.min_by(|a, b| a.1.total_cmp(&b.1))
			.map(|(i, _)| i)
	}

	fn db_at(&self, p: Point<Pixels>) -> f32 {
		let b = self.bounds.get();
		let f = (f32::from(p.y - b.origin.y) / f32::from(b.size.height).max(1.)).clamp(0., 1.);
		eq_step_db(eq_db_step(RANGE_DB - f * 2. * RANGE_DB))
	}

	fn drag_to(&mut self, p: Point<Pixels>, cx: &mut Context<'_, Self>) {
		let Some(band) = self.dragging else { return };
		let db = self.db_at(p);
		if let Some(g) = self.gains.get_mut(band)
			&& (*g - db).abs() > f32::EPSILON
		{
			*g = db;
			cx.emit(BandChanged { band, step: eq_db_step(db) });
			cx.notify();
		}
	}

	/// While a drag is on, every move and the release anywhere in the window belong to it.
	fn capture(entity: &Entity<Self>, window: &mut Window) {
		let moves = entity.clone();
		window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
			if phase == DispatchPhase::Bubble && e.dragging() {
				moves.update(cx, |this, cx| this.drag_to(e.position, cx));
			}
		});
		let ups = entity.clone();
		window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
			if phase == DispatchPhase::Bubble && e.button == MouseButton::Left {
				ups.update(cx, |this, cx| {
					if this.dragging.take().is_some() {
						cx.notify();
					}
				});
			}
		});
	}

	/// Points on the plot for each band, in window pixels.
	fn points(gains: &[f32; 10], b: Bounds<Pixels>) -> Vec<Point<Pixels>> {
		gains
			.iter()
			.enumerate()
			.map(|(i, db)| point(b.origin.x + b.size.width * x_frac(i), b.origin.y + b.size.height * y_frac(*db)))
			.collect()
	}

	/// A smooth line through `pts` (Catmull-Rom as cubic Béziers), onto `path`.
	fn trace(path: &mut PathBuilder, pts: &[Point<Pixels>]) {
		for (i, w) in pts.windows(2).enumerate() {
			let (Some(&p1), Some(&p2)) = (w.first(), w.get(1)) else { continue };
			let p0 = if i == 0 { p1 } else { pts.get(i - 1).copied().unwrap_or(p1) };
			let p3 = pts.get(i + 2).copied().unwrap_or(p2);
			let c1 = point(p1.x + (p2.x - p0.x) / 6., p1.y + (p2.y - p0.y) / 6.);
			let c2 = point(p2.x - (p3.x - p1.x) / 6., p2.y - (p3.y - p1.y) / 6.);
			path.cubic_bezier_to(p2, c1, c2);
		}
	}

	fn paint(gains: [f32; 10], enabled: bool, b: Bounds<Pixels>, window: &mut Window) {
		let line = if enabled { theme::accent().into() } else { theme::alpha(theme::subdued(), 0.6) };
		// Grid: the 0 dB line brighter than the rest.
		for db in GRID {
			let y = b.origin.y + b.size.height * y_frac(db);
			let mut grid = PathBuilder::stroke(px(1.));
			grid.move_to(point(b.origin.x, y));
			grid.line_to(point(b.origin.x + b.size.width, y));
			if let Ok(path) = grid.build() {
				window.paint_path(path, theme::alpha(theme::text(), if db == 0. { 0.22 } else { 0.08 }));
			}
		}
		let pts = Self::points(&gains, b);
		let (Some(&first), Some(&last)) = (pts.first(), pts.last()) else { return };
		let zero = b.origin.y + b.size.height * y_frac(0.);
		// The area between the curve and 0 dB, lightly tinted.
		let mut fill = PathBuilder::fill();
		fill.move_to(point(first.x, zero));
		fill.line_to(first);
		Self::trace(&mut fill, &pts);
		fill.line_to(point(last.x, zero));
		fill.close();
		if let Ok(path) = fill.build() {
			window.paint_path(path, theme::alpha(if enabled { theme::accent() } else { theme::subdued() }, 0.14));
		}
		let mut curve = PathBuilder::stroke(px(3.));
		curve.move_to(first);
		Self::trace(&mut curve, &pts);
		if let Ok(path) = curve.build() {
			window.paint_path(path, line);
		}
	}
}

impl Render for EqGraph {
	fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let (gains, enabled) = (self.gains, self.enabled);
		let shown = self.dragging.or(self.hovered);
		let bounds = self.bounds.clone();
		let capture = self.dragging.is_some().then(|| cx.entity());
		let plot = div()
			.id("eq-plot")
			.relative()
			.h(px(PLOT_H))
			.w_full()
			.when(enabled, |s| s.cursor_ns_resize())
			.child(
				canvas(
					move |b, _, _| bounds.set(b),
					move |b, (), window, _| {
						Self::paint(gains, enabled, b, window);
						if let Some(entity) = &capture {
							Self::capture(entity, window);
						}
					},
				)
				.absolute()
				.size_full(),
			)
			// Knobs: white discs on the curve; the active one larger, its gain above it.
			.children(gains.iter().enumerate().map(|(i, db)| knob(i, *db, shown == Some(i), enabled)))
			.when(enabled, |s| {
				s.on_mouse_down(
					MouseButton::Left,
					cx.listener(|this, e: &MouseDownEvent, _, cx| {
						if let Some(band) = this.band_at(e.position) {
							this.dragging = Some(band);
							this.drag_to(e.position, cx);
							cx.notify();
						}
					}),
				)
				.on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
					let band = this.band_at(e.position);
					if band != this.hovered {
						this.hovered = band;
						cx.notify();
					}
				}))
				.on_hover(cx.listener(|this, over: &bool, _, cx| {
					if !*over && this.hovered.take().is_some() {
						cx.notify();
					}
				}))
			});
		let values: Vec<String> = gains.iter().map(|db| db_label(*db).replace(" dB", "")).collect();
		let freqs: Vec<String> = BANDS.iter().map(|f| (*f).to_owned()).collect();
		div().flex().gap_4().when(!enabled, |s| s.opacity(0.5)).child(scale()).child(
			div()
				.flex_1()
				.flex()
				.flex_col()
				.gap_3()
				.px(px(KNOB))
				.child(plot)
				.child(div().flex().flex_col().gap_1().child(labels(freqs, shown)).child(labels(values, shown))),
		)
	}
}

/// Band `i`'s knob at `db`: a white disc on the curve; the active one larger, its gain above it.
fn knob(i: usize, db: f32, active: bool, enabled: bool) -> impl IntoElement {
	let size = if active { KNOB + 4. } else { KNOB };
	div()
		.absolute()
		.left(relative(x_frac(i)))
		.top(relative(y_frac(db)))
		.ml(px(-size / 2.))
		.mt(px(-size / 2.))
		.size(px(size))
		.rounded_full()
		.bg(if enabled { theme::text().into() } else { theme::alpha(theme::subdued(), 0.8) })
		.shadow_md()
		.when(active && enabled, |s| {
			s.child(
				div().absolute().bottom(px(size + 8.)).left(px(size / 2. - 32.)).w(px(64.)).flex().justify_center().child(
					div()
						.px_2()
						.py_0p5()
						.rounded(px(4.))
						.bg(theme::raised_hover())
						.text_size(px(12.))
						.font_weight(FontWeight::BOLD)
						.child(db_label(db)),
				),
			)
		})
}

/// A row of per-band labels centred under the knobs; `strong` is drawn white.
fn labels(text: Vec<String>, strong: Option<usize>) -> impl IntoElement {
	div().relative().h(px(18.)).w_full().children(text.into_iter().enumerate().map(move |(i, t)| {
		div()
			.absolute()
			.left(relative(x_frac(i)))
			.ml(px(-24.))
			.w(px(48.))
			.flex()
			.justify_center()
			.text_size(px(12.))
			.text_color(if strong == Some(i) { theme::text() } else { theme::subdued() })
			.child(t)
	}))
}

/// The dB scale left of the plot, its labels level with the grid lines.
fn scale() -> impl IntoElement {
	div().relative().w(px(44.)).h(px(PLOT_H)).children(GRID.iter().map(|db| {
		div()
			.absolute()
			.right_0()
			.top(relative(y_frac(*db)))
			.mt(px(-8.))
			.text_size(px(12.))
			.text_color(theme::subdued())
			.child(db_label(*db))
	}))
}
