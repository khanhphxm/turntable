//! Exercise the public GPUI compositing boundary using the real headless drawing path.

use std::{cell::Cell, rc::Rc};

use gpui_kit::{
	AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, StyleRefinement, Styled as _, Window, canvas, deferred, div,
	px, size,
};
use gpui_test::{TestAppContext, VisualTestContext};

struct Probe {
	paints:  Rc<Cell<usize>>,
	opacity: Rc<Cell<f32>>,
}

impl Render for Probe {
	fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
		let paints = self.paints.clone();
		let opacity = self.opacity.clone();
		div().size_full().opacity(0.5).child(
			div().size_full().opacity(0.4).child(
				canvas(
					|_, _, _| (),
					move |_, (), window, cx| {
						paints.set(paints.get() + 1);
						opacity.set(window.native_element_paint_context(cx).opacity);
					},
				)
				.size_full(),
			),
		)
	}
}

struct Root {
	probe:   Entity<Probe>,
	overlay: bool,
}

impl Render for Root {
	fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
		div()
			.size_full()
			.child(self.probe.clone().cached(StyleRefinement::default().size_full()))
			.children(self.overlay.then(|| deferred(div().size(px(24.)))))
	}
}

fn setup() -> (Entity<Root>, VisualTestContext, Rc<Cell<usize>>, Rc<Cell<f32>>) {
	let mut app = TestAppContext::single();
	let paints = Rc::new(Cell::new(0));
	let opacity = Rc::new(Cell::new(0.));
	let mut root = None;
	let window = app.open_window(size(px(400.), px(300.)), |_, cx| {
		let probe = cx.new(|_| Probe { paints: paints.clone(), opacity: opacity.clone() });
		root = Some(cx.entity());
		Root { probe, overlay: false }
	});
	let cx = VisualTestContext::from_window(window.into(), &app);
	(root.unwrap(), cx, paints, opacity)
}

#[test]
fn inherited_opacity_and_cached_overlay_changes_reach_native_observers() {
	let (root, mut cx, paints, opacity) = setup();
	let observed = Rc::new(Cell::new(false));
	let calls = Rc::new(Cell::new(0));
	let listener_observed = observed.clone();
	let listener_calls = calls.clone();
	let _subscription = cx.update(|window, _| {
		window.observe_native_element_paint(move |window, cx| {
			listener_calls.set(listener_calls.get() + 1);
			listener_observed.set(window.native_element_paint_context(cx).occluding_bounds.is_some());
		})
	});
	cx.update(|window, cx| window.draw(cx).clear(cx));
	assert!((opacity.get() - 0.2).abs() < f32::EPSILON);
	assert!(!observed.get());
	let initial_paints = paints.get();
	cx.update(|window, cx| {
		root.update(cx, |root, cx| {
			root.overlay = true;
			cx.notify();
		});
		window.draw(cx).clear(cx);
	});
	assert!(observed.get());
	assert_eq!(paints.get(), initial_paints, "the observer sees foreground changes even when the underlying paint is reused");
	cx.update(|window, cx| {
		root.update(cx, |root, cx| {
			root.overlay = false;
			cx.notify();
		});
		window.draw(cx).clear(cx);
	});
	assert!(!observed.get());
	assert_eq!(paints.get(), initial_paints);
	assert_eq!(calls.get(), 3);
}

#[test]
fn native_fallback_refresh_rebuilds_once_before_presentation() {
	let (root, mut cx, paints, _) = setup();
	let observed = Rc::new(Cell::new(false));
	let calls = Rc::new(Cell::new(0));
	let listener_calls = calls.clone();
	let _subscription = cx.update(|window, _| {
		window.observe_native_element_paint(move |window, cx| {
			listener_calls.set(listener_calls.get() + 1);
			let occluded = window.native_element_paint_context(cx).occluding_bounds.is_some();
			if observed.replace(occluded) != occluded {
				window.refresh();
			}
		})
	});
	cx.update(|window, cx| window.draw(cx).clear(cx));
	let before = paints.get();
	cx.update(|window, cx| {
		root.update(cx, |root, cx| {
			root.overlay = true;
			cx.notify();
		});
		window.draw(cx).clear(cx);
	});
	assert_eq!(paints.get(), before + 1, "the first pass reuses paint; the fallback refresh rebuilds the cached view");
	assert_eq!(calls.get(), 3, "one initial observation and two observations for the changed scene");
}

struct FallbackDimensions;

impl Render for FallbackDimensions {
	fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
		let kinds = [super::model::Kind::Bars(px(16.)), super::model::Kind::Clock("0:01".into()), super::model::Kind::Progress {
			motion:   super::Motion { anchor: turntable_core::app::Anchor::default(), duration_ms: 1000, playing: false },
			hovered:  false,
			dragging: Some(0.75),
		}];
		assert!(!kinds[0].needs_gpui());
		assert!(kinds[2].needs_gpui());
		let bounds = gpui_kit::Bounds::new(gpui_kit::point(px(10.), px(20.)), size(px(100.), px(16.)));
		assert_eq!(kinds[0].paint_bounds(bounds), bounds);
		assert!(kinds[1].layers(bounds, crate::theme::text(), false, std::time::Instant::now())[0].is_some());
		div().size_full().children(kinds.iter().flat_map(|kind| [kind.placeholder(), kind.fallback("fallback-test")]))
	}
}

#[test]
fn native_widget_placeholders_keep_real_gpui_fallback_available() {
	let mut app = TestAppContext::single();
	let window = app.open_window(size(px(400.), px(300.)), |_, _| FallbackDimensions);
	let mut cx = VisualTestContext::from_window(window.into(), &app);
	cx.update(|window, cx| window.draw(cx).clear(cx));
}
