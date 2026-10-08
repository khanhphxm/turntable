//! The equalizer's curve: ten bands drawn as a smooth line over a dB grid, each a knob to drag
//! up or down in 0.5 dB steps. The band under the pointer shows its gain; every band's gain is
//! listed under its frequency, and clicking it lets you type an exact value.

use std::{cell::Cell, rc::Rc};

use gpui_kit::{
	AnyElement, App, AppContext as _, Bounds, Context, DispatchPhase, Entity, EventEmitter, FontWeight, InteractiveElement as _,
	IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, PathBuilder, Pixels, Point, Render,
	StatefulInteractiveElement as _, Styled as _, Window, canvas,
	component::input::{Input, InputEvent, InputState},
	div, point,
	prelude::FluentBuilder as _,
	px, relative,
};
use turntable_core::app::{eq_db_step, eq_step_db};

use crate::theme;

const BANDS: [&str; 10] = ["32 Hz", "64 Hz", "125 Hz", "250 Hz", "500 Hz", "1 kHz", "2 kHz", "4 kHz", "8 kHz", "16 kHz"];
/// The grid's range, ± dB.
const RANGE_DB: f32 = 12.;
/// Grid lines, dB.
const GRID: [f32; 5] = [12., 6., 0., -6., -12.];
const PLOT_H: f32 = 200.;
const KNOB: f32 = 14.;
/// How far from a knob (horizontally) the pointer still picks its band.
const PICK: f32 = 28.;

/// A band moved to a new slider step (the core's 0.5 dB steps).
pub struct BandChanged {
	pub band: usize,
	pub step: u8,
}

/// A band's gain typed in, in dB (the core rounds it to a 0.5 dB step and clamps it).
pub struct BandTyped {
	pub band: usize,
	pub db:   f32,
}

pub struct EqGraph {
	gains:    [f32; 10],
	enabled:  bool,
	hovered:  Option<usize>,
	dragging: Option<usize>,
	/// The plot's bounds as of the last paint (pointer → band and gain).
	bounds:   Rc<Cell<Bounds<Pixels>>>,
	/// The band whose gain is being typed, and its field.
	typing:   Option<(usize, Entity<InputState>)>,
}

impl EventEmitter<BandChanged> for EqGraph {}
impl EventEmitter<BandTyped> for EqGraph {}

/// "+4 dB", "+1.5 dB", "0 dB", "−6 dB" (gains move in 0.5 dB steps).
fn db_label(db: f32) -> String {
	format!("{} dB", db_value(db))
}

/// The number alone: "+4", "+1.5", "0", "−6".
fn db_value(db: f32) -> String {
	let r = (db * 2.).round() / 2.;
	let magnitude = if r.fract() == 0. { format!("{}", r.abs()) } else { format!("{:.1}", r.abs()) };
	if r > 0. {
		format!("+{magnitude}")
	} else if r < 0. {
		format!("−{magnitude}")
	} else {
		"0".to_owned()
	}
}

/// A typed gain: "3", "+1.5", "-2", "−6 dB" (`None` when it isn't a number).
fn parse_db(text: &str) -> Option<f32> {
	let t = text.trim().trim_end_matches("dB").trim_end_matches("db").trim().replace('−', "-").replace(',', ".");
	t.trim_start_matches('+').parse::<f32>().ok().filter(|v| v.is_finite())
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
		cx.new(|_| Self { gains, enabled, hovered: None, dragging: None, bounds: Rc::default(), typing: None })
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

	/// A smooth line through `pts` onto `path`: monotone cubic (Fritsch–Carlson), so between two
	/// bands the curve never rises above or dips below both of them (flat neighbours stay flat).
	fn trace(path: &mut PathBuilder, pts: &[Point<Pixels>]) {
		let slopes = monotone_slopes(pts);
		for (i, w) in pts.windows(2).enumerate() {
			let (Some(&p1), Some(&p2)) = (w.first(), w.get(1)) else { continue };
			let (m1, m2) = (slopes.get(i).copied().unwrap_or(0.), slopes.get(i + 1).copied().unwrap_or(0.));
			let h = f32::from(p2.x - p1.x) / 3.;
			let c1 = point(p1.x + px(h), p1.y + px(m1 * h));
			let c2 = point(p2.x - px(h), p2.y - px(m2 * h));
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

impl EqGraph {
	/// Opens band `band`'s value for typing, filled with its gain and selected.
	fn start_typing(&mut self, band: usize, window: &mut Window, cx: &mut Context<'_, Self>) {
		let current = self.gains.get(band).copied().unwrap_or(0.);
		let input = cx.new(|cx| InputState::new(window, cx).default_value(db_value(current)));
		cx.subscribe(&input, move |this, input, e: &InputEvent, cx| match e {
			InputEvent::PressEnter { .. } | InputEvent::Blur => {
				if let Some(db) = parse_db(&input.read(cx).value()) {
					cx.emit(BandTyped { band, db });
				}
				this.typing = None;
				cx.notify();
			},
			InputEvent::Change | InputEvent::Focus => {},
		})
		.detach();
		input.update(cx, |s, cx| {
			s.focus(window, cx);
			s.select_all(window, cx);
		});
		self.typing = Some((band, input));
		cx.notify();
	}

	/// Under each band its gain; a click turns it into a field for an exact value.
	fn values(&self, strong: Option<usize>, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let typing = self.typing.clone();
		let enabled = self.enabled;
		div().relative().h(px(24.)).w_full().children(self.gains.iter().enumerate().map(move |(i, db)| {
			let cell = div().absolute().left(relative(x_frac(i))).ml(px(-28.)).w(px(56.)).h_full().flex().items_center().justify_center();
			let editing = typing.as_ref().filter(|(band, _)| *band == i).map(|(_, input)| input.clone());
			let content: AnyElement = match editing {
				Some(input) => div()
					.w_full()
					.rounded(px(4.))
					.bg(theme::raised_hover())
					.border_1()
					.border_color(theme::subdued())
					.px_1()
					.text_size(px(12.))
					.child(Input::new(&input).appearance(false))
					.into_any_element(),
				None => div()
					.id(("eq-value", i))
					.px_1p5()
					.py_0p5()
					.rounded(px(4.))
					.text_size(px(12.))
					.text_color(if strong == Some(i) { theme::text() } else { theme::subdued() })
					.when(enabled, |s| {
						s.cursor_text()
							.hover(|s| s.bg(theme::raised()).text_color(theme::text()))
							.on_click(cx.listener(move |this, _, window, cx| this.start_typing(i, window, cx)))
					})
					.child(db_value(*db))
					.into_any_element(),
			};
			cell.child(content)
		}))
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
		let freqs: Vec<String> = BANDS.iter().map(|f| (*f).to_owned()).collect();
		let values = self.values(shown, cx);
		div().flex().gap_4().when(!enabled, |s| s.opacity(0.5)).child(scale()).child(
			div()
				.flex_1()
				.flex()
				.flex_col()
				.gap_3()
				.px(px(KNOB))
				.child(plot)
				.child(div().flex().flex_col().gap_1().child(labels(freqs, shown)).child(values)),
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
			.ml(px(-28.))
			.w(px(56.))
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

/// Tangents for a monotone cubic through `pts` (x increasing): zero at local peaks and valleys
/// and on flat stretches, limited elsewhere so no segment overshoots its ends.
fn monotone_slopes(pts: &[Point<Pixels>]) -> Vec<f32> {
	let secants: Vec<f32> = pts
		.windows(2)
		.map(|w| match (w.first(), w.get(1)) {
			(Some(a), Some(b)) => f32::from(b.y - a.y) / f32::from(b.x - a.x).max(f32::EPSILON),
			_ => 0.,
		})
		.collect();
	let n = pts.len();
	let mut m = vec![0.; n];
	for (i, slot) in m.iter_mut().enumerate() {
		let before = i.checked_sub(1).and_then(|j| secants.get(j)).copied();
		let after = secants.get(i).copied();
		*slot = match (before, after) {
			(Some(a), Some(b)) if a * b > 0. => f32::midpoint(a, b),
			(None, Some(d)) | (Some(d), None) => d,
			// A peak, a valley or a flat stretch.
			_ => 0.,
		};
	}
	for (k, d) in secants.iter().enumerate() {
		if *d == 0. {
			if let Some(s) = m.get_mut(k) {
				*s = 0.;
			}
			if let Some(s) = m.get_mut(k + 1) {
				*s = 0.;
			}
			continue;
		}
		let (a, b) = (m.get(k).copied().unwrap_or(0.) / d, m.get(k + 1).copied().unwrap_or(0.) / d);
		let r = a.hypot(b);
		if r > 3. {
			let t = 3. / r;
			if let Some(s) = m.get_mut(k) {
				*s = t * a * d;
			}
			if let Some(s) = m.get_mut(k + 1) {
				*s = t * b * d;
			}
		}
	}
	m
}

#[cfg(test)]
mod tests {
	use gpui_kit::{point, px};

	use super::{monotone_slopes, parse_db};

	/// Samples the cubic between two points with the given tangents.
	fn sample(y1: f32, y2: f32, m1: f32, m2: f32, h: f32, t: f32) -> f32 {
		let (c1, c2) = (y1 + m1 * h / 3., y2 - m2 * h / 3.);
		let u = 1. - t;
		u * u * u * y1 + 3. * u * u * t * c1 + 3. * u * t * t * c2 + t * t * t * y2
	}

	#[test]
	fn curve_never_leaves_the_range_of_its_two_bands() {
		// One raised band beside flat ones (the reported dip between 64 and 125 Hz), a zigzag,
		// and a ramp: every segment stays between its endpoints.
		for ys in [[-60., 0., 0., 0., 0.], [0., 40., -40., 40., 0.], [0., 10., 20., 30., 80.]] {
			let pts: Vec<_> = ys.iter().enumerate().map(|(i, y)| point(px(i as f32 * 50.), px(*y))).collect();
			let m = monotone_slopes(&pts);
			for k in 0..ys.len() - 1 {
				let (lo, hi) = (ys[k].min(ys[k + 1]), ys[k].max(ys[k + 1]));
				for step in 0..=20 {
					let y = sample(ys[k], ys[k + 1], m[k], m[k + 1], 50., step as f32 / 20.);
					assert!(y >= lo - 1e-3 && y <= hi + 1e-3, "segment {k} of {ys:?} reaches {y}");
				}
			}
		}
	}

	#[test]
	fn typed_gains_accept_signs_units_and_decimal_commas() {
		assert_eq!(parse_db("3"), Some(3.));
		assert_eq!(parse_db("+1.5"), Some(1.5));
		assert_eq!(parse_db("−6 dB"), Some(-6.));
		assert_eq!(parse_db("-2,5"), Some(-2.5));
		assert_eq!(parse_db("loud"), None);
	}
}
