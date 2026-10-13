//! Core Animation adapter for generic layers and start/rate/duration scalar motion.
//! Widget roles, playback timing, fallback, clipping policy and paint lifetime live elsewhere.

use gpui_kit::{Bounds, Pixels, Rgba, SharedString, Window};
use objc2::{msg_send, rc::Retained};
use objc2_app_kit::{NSFont, NSView};
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGColor, CGPath};
use objc2_foundation::{NSNumber, NSString, ns_string};
use objc2_quartz_core::{
	CABasicAnimation, CACurrentMediaTime, CALayer, CAMediaTiming as _, CAMediaTimingFunction, CATextLayer, CATransaction,
	kCAAlignmentRight, kCAMediaTimingFunctionLinear,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use super::model::{Appearance, Layer, Layers, Property, ScalarMotion};

pub struct Parent(Retained<CALayer>);

impl Parent {
	pub fn for_window(window: &Window) -> Option<Self> {
		let RawWindowHandle::AppKit(handle) = HasWindowHandle::window_handle(window).ok()?.as_raw() else {
			return None;
		};
		// SAFETY: GPUI's live AppKit handle borrows its NSView on the window's main thread.
		let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
		view.layer().map(Self)
	}
}

pub struct Canvas {
	host:     Retained<CALayer>,
	parent:   Retained<CALayer>,
	nodes:    [Option<Node>; 4],
	geometry: Option<(Bounds<Pixels>, Bounds<Pixels>, f64, f32, f64)>,
}

impl Canvas {
	pub fn new(parent: &Parent) -> Self {
		transaction(|| {
			let host = CALayer::layer();
			host.setGeometryFlipped(true);
			host.setMasksToBounds(true);
			parent.0.addSublayer(&host);
			Self { host, parent: parent.0.clone(), nodes: std::array::from_fn(|_| None), geometry: None }
		})
	}

	pub fn hide(&self) {
		transaction(|| self.host.setHidden(true));
	}

	pub fn paint(&mut self, bounds: Bounds<Pixels>, clip: Bounds<Pixels>, scale: f64, opacity: f32, layers: Layers) {
		transaction(|| {
			let parent_height = self.parent.bounds().size.height;
			let geometry = (bounds, clip, scale, opacity, parent_height);
			if self.geometry != Some(geometry) {
				self.geometry = Some(geometry);
				let x = scalar(clip.origin.x);
				let y = scalar(clip.origin.y);
				let w = scalar(clip.size.width);
				let h = scalar(clip.size.height);
				let native_y = if self.parent.isGeometryFlipped() { y } else { parent_height - y - h };
				self.host.setFrame(rect(x, native_y, w, h));
				self.host.setBounds(rect(scalar(clip.origin.x - bounds.origin.x), scalar(clip.origin.y - bounds.origin.y), w, h));
				self.host.setContentsScale(scale);
				self.host.setOpacity(opacity);
			}
			self.host.setHidden(false);
			for (node, spec) in self.nodes.iter_mut().zip(layers) {
				if let Some(spec) = spec {
					let node = node.get_or_insert_with(|| Node::new(&self.host, &spec));
					node.configure(spec, scale);
				} else if let Some(node) = node.take() {
					node.surface.layer().removeFromSuperlayer();
				}
			}
		});
	}

	pub fn set_text(&mut self, label: &SharedString) {
		transaction(|| {
			for node in self.nodes.iter_mut().flatten() {
				if let Surface::Text(text) = &node.surface
					&& let Some(Layer { appearance: Appearance::Text { label: old, .. }, .. }) = &mut node.spec
					&& old != label
				{
					*old = label.clone();
					// SAFETY: CATextLayer's string property accepts and retains NSString.
					unsafe {
						text.setString(Some(&NSString::from_str(label)));
					}
				}
			}
		});
	}
}

impl Drop for Canvas {
	fn drop(&mut self) {
		transaction(|| {
			self.host.removeAllAnimations();
			self.host.removeFromSuperlayer();
		});
	}
}

enum Surface {
	Solid(Retained<CALayer>),
	Text(Retained<CATextLayer>),
}

impl Surface {
	fn layer(&self) -> &CALayer {
		match self {
			Self::Solid(layer) => layer,
			Self::Text(layer) => layer,
		}
	}
}

struct Node {
	surface: Surface,
	spec:    Option<Layer>,
	scale:   f64,
}

impl Node {
	fn new(host: &CALayer, spec: &Layer) -> Self {
		let surface = if matches!(spec.appearance, Appearance::Text { .. }) {
			let text = CATextLayer::layer();
			text.setAllowsFontSubpixelQuantization(true);
			// SAFETY: The framework owns this immutable alignment string.
			unsafe {
				text.setAlignmentMode(kCAAlignmentRight);
			}
			Surface::Text(text)
		} else {
			Surface::Solid(CALayer::layer())
		};
		surface.layer().setAllowsEdgeAntialiasing(true);
		host.addSublayer(surface.layer());
		Self { surface, spec: None, scale: 0. }
	}

	fn configure(&mut self, spec: Layer, scale: f64) {
		if self.spec.as_ref() == Some(&spec) && self.scale == scale {
			return;
		}
		let layer = self.surface.layer();
		layer.setContentsScale(scale);
		layer.setHidden(!spec.visible);
		layer.setAnchorPoint(CGPoint::new(f64::from(spec.anchor.x), f64::from(spec.anchor.y)));
		let width = scalar(spec.frame.size.width);
		let height = scalar(spec.frame.size.height);
		layer.setBounds(rect(0., 0., width, height));
		layer.setPosition(CGPoint::new(
			scalar(spec.frame.origin.x) + width * f64::from(spec.anchor.x),
			scalar(spec.frame.origin.y) + height * f64::from(spec.anchor.y),
		));
		layer.setCornerRadius(spec.corner_radius);
		match (&self.surface, &spec.appearance) {
			(Surface::Solid(_), Appearance::Fill(fill)) => layer.setBackgroundColor(Some(&color(*fill))),
			(Surface::Solid(_), Appearance::Shadow { color: shadow, offset, blur }) => {
				layer.setShadowColor(Some(&color(Rgba { a: 1., ..*shadow })));
				layer.setShadowOpacity(shadow.a);
				layer.setShadowOffset(CGSize::new(scalar(offset.x), scalar(offset.y)));
				layer.setShadowRadius(*blur / 2.);
				// SAFETY: A null transform means identity; the rectangle is passed by value.
				let path = unsafe { CGPath::with_ellipse_in_rect(rect(0., 0., width, height), std::ptr::null()) };
				layer.setShadowPath(Some(&path));
			},
			(Surface::Text(text), Appearance::Text { label, color: foreground, font_size }) => {
				let font = NSFont::systemFontOfSize(*font_size);
				// SAFETY: CATextLayer's font property explicitly accepts NSFont on macOS.
				unsafe {
					let _: () = msg_send![&**text, setFont: &*font];
				}
				text.setFontSize(*font_size);
				text.setForegroundColor(Some(&color(*foreground)));
				// SAFETY: CATextLayer accepts and retains NSString.
				unsafe {
					text.setString(Some(&NSString::from_str(label)));
				}
			},
			_ => unreachable!("a native element keeps a stable layer appearance kind"),
		}
		if self.spec.as_ref().and_then(|old| old.motion) != spec.motion {
			layer.removeAllAnimations();
			if let Some(motion) = spec.motion {
				animate(layer, motion);
			}
		}
		self.spec = Some(spec);
		self.scale = scale;
	}
}

fn animate(layer: &CALayer, motion: ScalarMotion) {
	let property = match motion.property {
		Property::Width => ns_string!("bounds.size.width"),
		Property::Height => ns_string!("bounds.size.height"),
		Property::PositionX => ns_string!("position.x"),
	};
	let animation = CABasicAnimation::animationWithKeyPath(Some(property));
	// SAFETY: NSNumber is a valid scalar value for each supported property.
	unsafe {
		animation.setFromValue(Some(&NSNumber::new_f64(motion.from)));
		animation.setToValue(Some(&NSNumber::new_f64(motion.end())));
	}
	animation.setDuration(motion.duration);
	animation.setBeginTime(CACurrentMediaTime() - motion.started_at.elapsed().as_secs_f64());
	animation.setTimeOffset(motion.phase);
	animation.setAutoreverses(motion.ping_pong);
	if motion.ping_pong {
		animation.setRepeatCount(f32::INFINITY);
	}
	// SAFETY: The framework owns this immutable timing-function name.
	animation.setTimingFunction(Some(unsafe { CAMediaTimingFunction::functionWithName(kCAMediaTimingFunctionLinear) }.as_ref()));
	layer.addAnimation_forKey(&animation, Some(ns_string!("scalar")));
}

fn transaction<R>(f: impl FnOnce() -> R) -> R {
	CATransaction::begin();
	CATransaction::setDisableActions(true);
	let result = f();
	CATransaction::commit();
	result
}

fn scalar(value: Pixels) -> f64 {
	f64::from(f32::from(value))
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> CGRect {
	CGRect::new(CGPoint::new(x, y), CGSize::new(width, height))
}

fn color(color: Rgba) -> CFRetained<CGColor> {
	CGColor::new_srgb(f64::from(color.r), f64::from(color.g), f64::from(color.b), f64::from(color.a))
}

#[cfg(test)]
mod tests {
	use gpui_kit::{point, px, size};

	use super::*;

	fn layers() -> Layers {
		[
			Some(Layer {
				frame:         Bounds::new(point(px(0.), px(6.)), size(px(80.), px(4.))),
				anchor:        point(0., 0.5),
				corner_radius: 2.,
				appearance:    Appearance::Fill(crate::theme::text()),
				visible:       true,
				motion:        Some(ScalarMotion {
					property:   Property::Width,
					from:       20.,
					rate:       10.,
					duration:   6.,
					started_at: std::time::Instant::now(),
					phase:      0.,
					ping_pong:  false,
				}),
			}),
			None,
			None,
			None,
		]
	}

	#[test]
	fn canvas_applies_clipping_opacity_scale_and_scalar_without_submission() {
		let parent = Parent(CALayer::layer());
		parent.0.setBounds(rect(0., 0., 400., 300.));
		let mut canvas = Canvas::new(&parent);
		let bounds = Bounds::new(point(px(100.), px(200.)), size(px(80.), px(16.)));
		let clip = bounds.dilate(px(12.));
		let model = layers();
		canvas.paint(bounds, clip, 2., 0.4, model.clone());
		assert_eq!(canvas.host.frame(), rect(88., 72., 104., 40.));
		assert_eq!(canvas.host.bounds(), rect(-12., -12., 104., 40.));
		assert_eq!(canvas.host.opacity(), 0.4);
		assert_eq!(canvas.host.contentsScale(), 2.);
		let fill = canvas.nodes[0].as_ref().unwrap().surface.layer();
		assert_eq!(fill.bounds(), rect(0., 0., 80., 4.));
		assert_eq!(fill.position(), CGPoint::new(0., 8.));
		// SAFETY: The returned animation is framework-retained and read on this test thread.
		assert!(unsafe { fill.animationForKey(ns_string!("scalar")) }.is_some());
		canvas.hide();
		assert!(canvas.host.isHidden());
		canvas.paint(bounds, clip, 2., 0.4, model.clone());
		assert!(!canvas.host.isHidden());
		parent.0.setBounds(rect(0., 0., 400., 500.));
		canvas.paint(bounds, clip, 2., 0.4, model);
		assert_eq!(canvas.host.frame(), rect(88., 272., 104., 40.));
		drop(canvas);
		// SAFETY: CALayer retains its own typed sublayers; the array is borrowed for this read.
		assert!(unsafe { parent.0.sublayers() }.is_none_or(|layers| layers.is_empty()));
	}
}
