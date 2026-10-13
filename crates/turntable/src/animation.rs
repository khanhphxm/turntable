//! Long-running animations use a compositor when available and GPUI otherwise.
//! Timing, geometry, accessibility and fallback decisions are shared; the native backend
//! only applies generic layer descriptions and scalar animations.

use gpui_kit::{AnyElement, IntoElement as _, ParentElement as _, Pixels, Styled as _, div};
use turntable_core::app::Anchor;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(target_os = "macos", test))]
mod model;
#[cfg(target_os = "macos")]
mod native;
#[cfg(test)]
mod tests;

pub const BARS: [(u64, f32); 4] = [(900, 0.), (700, 0.45), (1100, 0.2), (800, 0.7)];

#[derive(Clone, Copy)]
pub struct Motion {
	pub anchor:      Anchor,
	pub duration_ms: u64,
	pub playing:     bool,
}

impl Motion {
	pub fn progress(self) -> f32 {
		if self.duration_ms == 0 { 0. } else { self.anchor.now_ms().min(self.duration_ms) as f32 / self.duration_ms as f32 }
	}
}

#[derive(Default)]
pub struct Clock {
	#[cfg(target_os = "macos")]
	link: native::Link,
}

impl Clock {
	pub fn element(&self, label: gpui_kit::SharedString) -> AnyElement {
		#[cfg(target_os = "macos")]
		return native::element("elapsed-clock", model::Kind::Clock(label), Some(self.link.clone()));
		#[cfg(not(target_os = "macos"))]
		clock_fallback(label)
	}

	/// Whether the displayed clock can change without invalidating a GPUI scene.
	pub fn is_native(&self) -> bool {
		#[cfg(target_os = "macos")]
		return self.link.is_native();
		#[cfg(not(target_os = "macos"))]
		false
	}

	pub fn set(&self, label: &gpui_kit::SharedString) {
		#[cfg(target_os = "macos")]
		self.link.set_text(label);
		#[cfg(not(target_os = "macos"))]
		let _ = label;
	}
}

fn clock_fallback(label: gpui_kit::SharedString) -> AnyElement {
	div().w(gpui_kit::px(40.)).flex().justify_end().child(label).into_any_element()
}

pub fn bars(id: &'static str, size: Pixels) -> AnyElement {
	#[cfg(target_os = "macos")]
	return native::element(id, model::Kind::Bars(size), None);
	#[cfg(not(target_os = "macos"))]
	gpui_bars(id, size)
}

fn bar_height(phase: f32) -> f32 {
	0.25 + 0.75 * (1. - (2. * phase.fract() - 1.).abs())
}

fn gpui_bars(id: &'static str, size: Pixels) -> AnyElement {
	use std::time::Duration;

	use gpui_kit::{Animation, AnimationExt as _};
	let bar_w = size * 0.16;
	div()
		.size(size)
		.flex_none()
		.flex()
		.items_end()
		.justify_center()
		.gap(size * 0.08)
		.children(BARS.iter().enumerate().map(move |(i, (ms, phase))| {
			let phase = *phase;
			div().w(bar_w).h(size * 0.3).bg(crate::theme::accent()).with_animation(
				(id, i),
				Animation::new(Duration::from_millis(*ms)).repeat_synced().with_max_fps(15.),
				move |el, t| el.h(size * bar_height(t + phase)),
			)
		}))
		.into_any_element()
}

pub fn progress(motion: Motion, hovered: bool, dragging: Option<f32>) -> AnyElement {
	#[cfg(target_os = "macos")]
	return native::element("seek-motion", model::Kind::Progress { motion, hovered, dragging }, None);
	#[cfg(not(target_os = "macos"))]
	{
		let _ = hovered;
		crate::widgets::slider_foreground(dragging.unwrap_or_else(|| motion.progress()), dragging.is_some())
	}
}

/// App-owned overlays are part of the GPUI scene rather than native layers. Keep composited
/// content in the same scene while they are visible, including cached regions underneath.
pub fn set_obscured(window: &mut gpui_kit::Window, cx: &mut gpui_kit::App, obscured: bool) {
	#[cfg(target_os = "macos")]
	native::set_obscured(window, cx, obscured);
	#[cfg(not(target_os = "macos"))]
	let _ = (window, cx, obscured);
}
