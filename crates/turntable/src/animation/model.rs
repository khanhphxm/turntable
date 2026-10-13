//! Platform-neutral widget geometry and monotonic animation descriptions.

use std::time::Instant;

use gpui_kit::{AnyElement, Bounds, IntoElement as _, Pixels, Point, Rgba, SharedString, Styled as _, div, point, px, size};

use super::{BARS, Motion};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Property {
	Width,
	Height,
	PositionX,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScalarMotion {
	pub property:   Property,
	pub from:       f64,
	pub rate:       f64,
	pub duration:   f64,
	pub started_at: Instant,
	pub phase:      f64,
	pub ping_pong:  bool,
}

impl ScalarMotion {
	pub fn end(self) -> f64 {
		self.from + self.rate * self.duration
	}
}

#[derive(Clone, Debug, PartialEq)]
pub enum Appearance {
	Fill(Rgba),
	Text { label: SharedString, color: Rgba, font_size: f64 },
	Shadow { color: Rgba, offset: Point<Pixels>, blur: f64 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
	pub frame:         Bounds<Pixels>,
	pub anchor:        Point<f32>,
	pub corner_radius: f64,
	pub appearance:    Appearance,
	pub visible:       bool,
	pub motion:        Option<ScalarMotion>,
}

pub type Layers = [Option<Layer>; 4];

pub enum Kind {
	Bars(Pixels),
	Clock(SharedString),
	Progress { motion: Motion, hovered: bool, dragging: Option<f32> },
}

impl Kind {
	pub fn needs_gpui(&self) -> bool {
		matches!(self, Self::Progress { dragging: Some(_), .. })
	}

	pub fn placeholder(&self) -> AnyElement {
		match self {
			Self::Bars(size) => div().size(*size).flex_none().into_any_element(),
			Self::Clock(_) => div().w(px(40.)).h(px(18.)).into_any_element(),
			Self::Progress { .. } => div().w_full().h(px(16.)).into_any_element(),
		}
	}

	pub fn fallback(&self, id: &'static str) -> AnyElement {
		match self {
			Self::Bars(size) => super::gpui_bars(id, *size),
			Self::Clock(label) => super::clock_fallback(label.clone()),
			Self::Progress { motion, dragging, .. } => {
				crate::widgets::slider_foreground(dragging.unwrap_or_else(|| motion.progress()), dragging.is_some())
			},
		}
	}

	pub fn paint_bounds(&self, bounds: Bounds<Pixels>) -> Bounds<Pixels> {
		if matches!(self, Self::Progress { hovered: true, .. }) { bounds.dilate(px(12.)) } else { bounds }
	}

	pub fn layers(&self, bounds: Bounds<Pixels>, color: Rgba, reduced_motion: bool, epoch: Instant) -> Layers {
		match self {
			Self::Bars(size) => bar_layers(*size, reduced_motion, epoch),
			Self::Clock(label) => [
				Some(Layer {
					frame:         Bounds::new(point(px(0.), px(0.)), size(bounds.size.width, px(18.))),
					anchor:        point(0., 0.),
					corner_radius: 0.,
					appearance:    Appearance::Text { label: label.clone(), color, font_size: 12. },
					visible:       true,
					motion:        None,
				}),
				None,
				None,
				None,
			],
			Self::Progress { motion, hovered, .. } => progress_layers(*motion, bounds.size.width, *hovered),
		}
	}
}

fn bar_layers(size: Pixels, reduced_motion: bool, epoch: Instant) -> Layers {
	let extent = f64::from(f32::from(size));
	let mut left = size * 0.06;
	BARS.map(|(period_ms, phase)| {
		let x = left;
		left += size * 0.24;
		let height = if reduced_motion { size * super::bar_height(phase) } else { size * 0.25 };
		let half_period = period_ms as f64 / 2000.;
		Some(Layer {
			frame:         Bounds::new(point(x, size - height), gpui_kit::size(size * 0.16, height)),
			anchor:        point(0.5, 1.),
			corner_radius: 0.,
			appearance:    Appearance::Fill(crate::theme::accent()),
			visible:       true,
			motion:        (!reduced_motion).then(|| ScalarMotion {
				property:   Property::Height,
				from:       extent * 0.25,
				rate:       extent * 0.75 / half_period,
				duration:   half_period,
				started_at: epoch,
				phase:      f64::from(phase) * period_ms as f64 / 1000.,
				ping_pong:  true,
			}),
		})
	})
}

fn progress_layers(motion: Motion, width: Pixels, hovered: bool) -> Layers {
	let extent = f64::from(f32::from(width));
	let moving = motion.playing && motion.anchor.speed > 0. && motion.duration_ms > motion.anchor.position_ms;
	let position = if moving { 1. } else { motion.progress() };
	let scalar = moving.then(|| ScalarMotion {
		property:   Property::Width,
		from:       extent * motion.anchor.position_ms as f64 / motion.duration_ms as f64,
		rate:       extent * motion.anchor.speed * 1000. / motion.duration_ms as f64,
		duration:   (motion.duration_ms - motion.anchor.position_ms) as f64 / 1000. / motion.anchor.speed,
		started_at: motion.anchor.at,
		phase:      0.,
		ping_pong:  false,
	});
	let knob_motion = scalar.map(|scalar| ScalarMotion { property: Property::PositionX, ..scalar });
	let knob_x = width * position;
	let shadow = |spread: f32, offset: f32, blur: f64| Layer {
		frame:         Bounds::new(point(knob_x - px(6. + spread), px(2. - spread)), size(px(12. + spread * 2.), px(12. + spread * 2.))),
		anchor:        point(0.5, 0.5),
		corner_radius: f64::from(6. + spread),
		appearance:    Appearance::Shadow { color: gpui_kit::black().opacity(0.1).to_rgb(), offset: point(px(0.), px(offset)), blur },
		visible:       hovered,
		motion:        knob_motion,
	};
	[
		Some(Layer {
			frame:         Bounds::new(point(px(0.), px(6.)), size(width * position, px(4.))),
			anchor:        point(0., 0.5),
			corner_radius: 2.,
			appearance:    Appearance::Fill(if hovered { crate::theme::accent() } else { crate::theme::text() }),
			visible:       true,
			motion:        scalar,
		}),
		Some(shadow(-1., 4., 6.)),
		Some(shadow(-2., 2., 4.)),
		Some(Layer {
			frame:         Bounds::new(point(knob_x - px(6.), px(2.)), size(px(12.), px(12.))),
			anchor:        point(0.5, 0.5),
			corner_radius: 6.,
			appearance:    Appearance::Fill(crate::theme::text()),
			visible:       hovered,
			motion:        knob_motion,
		}),
	]
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn progress_uses_one_monotonic_scalar_for_fill_and_hover_knob() {
		let at = Instant::now();
		let motion = Motion {
			anchor:      turntable_core::app::Anchor { at, position_ms: 30_000, speed: 1.25 },
			duration_ms: 180_000,
			playing:     true,
		};
		let layers = progress_layers(motion, px(600.), true);
		let fill = layers[0].as_ref().unwrap().motion.unwrap();
		let knob = layers[3].as_ref().unwrap().motion.unwrap();
		assert_eq!(fill.from, 100.);
		assert_eq!(fill.rate, 600. * 1.25 / 180.);
		assert_eq!(fill.duration, 120.);
		assert_eq!(fill.end(), 600.);
		assert_eq!(fill.started_at, at);
		assert_eq!(knob, ScalarMotion { property: Property::PositionX, ..fill });
		assert!(layers.iter().flatten().all(|layer| layer.visible));
	}

	#[test]
	fn paused_zero_duration_and_finished_progress_are_static() {
		for (position, duration, playing) in [(30_000, 180_000, false), (0, 0, true), (180_000, 180_000, true)] {
			let motion = Motion {
				anchor: turntable_core::app::Anchor {
					at:          Instant::now(),
					position_ms: position,
					speed:       if playing { 1. } else { 0. },
				},
				duration_ms: duration,
				playing,
			};
			let layers = progress_layers(motion, px(600.), false);
			assert!(layers.iter().flatten().all(|layer| layer.motion.is_none()));
			assert_eq!(
				layers[0].as_ref().unwrap().frame.size.width,
				px(if duration == 0 { 0. } else { position as f32 / duration as f32 * 600. })
			);
			assert!(layers[1..].iter().flatten().all(|layer| !layer.visible));
		}
	}

	#[test]
	fn bars_share_triangular_timing_and_respect_reduced_motion() {
		let epoch = Instant::now();
		let animated = bar_layers(px(16.), false, epoch);
		let static_layers = bar_layers(px(16.), true, epoch);
		for (i, (period_ms, phase)) in BARS.iter().enumerate() {
			let scalar = animated[i].as_ref().unwrap().motion.unwrap();
			assert!(scalar.ping_pong);
			assert_eq!(scalar.started_at, epoch);
			assert_eq!(scalar.from, 4.);
			assert!((scalar.end() - 16.).abs() <= f64::EPSILON * 16.);
			assert_eq!(scalar.duration * 2000., *period_ms as f64);
			assert_eq!(scalar.phase, f64::from(*phase) * *period_ms as f64 / 1000.);
			let reduced = static_layers[i].as_ref().unwrap();
			assert!(reduced.motion.is_none());
			assert_eq!(reduced.frame.size.height, px(16. * super::super::bar_height(*phase)));
		}
	}
}
