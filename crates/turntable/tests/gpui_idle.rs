//! GPUI quiescent scratch reclamation without a native window or user credentials.

#![expect(
	unused_crate_dependencies,
	reason = "the GPUI integration test exercises the public UI API, not the app's implementation dependencies"
)]

#[cfg(test)]
mod tests {
	use std::{cell::Cell, rc::Rc};

	use gpui_kit::{FocusHandle, Keystroke, Window, WindowHandle, WindowVisibility, div, prelude::*, px, rgb};
	use gpui_test::TestAppContext;

	struct IdleView {
		renders: Rc<Cell<usize>>,
		keys:    Rc<Cell<usize>>,
		focus:   FocusHandle,
	}

	impl Render for IdleView {
		fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
			self.renders.set(self.renders.get() + 1);
			let keys = self.keys.clone();
			div()
				.id("idle-root")
				.track_focus(&self.focus)
				.size_full()
				.bg(rgb(0x0010_2030))
				.on_key_down(move |_, _, _| keys.set(keys.get() + 1))
				.child(
					div()
						.id("clipped-layer")
						.w(px(80.25))
						.h(px(60.5))
						.overflow_hidden()
						.bg(rgb(0x0040_5060))
						.child(div().w(px(100.5)).h(px(90.25)).bg(rgb(0x0070_8090))),
				)
		}
	}

	fn draw(window: WindowHandle<IdleView>, cx: &mut TestAppContext, refresh: bool) -> String {
		cx.update_window(window.into(), |_, window, cx| {
			if refresh {
				window.refresh();
			}
			window.draw(cx).clear(cx);
			let quads = window.painted_quads();
			assert!(!quads.is_empty());
			format!("{quads:?}")
		})
		.unwrap()
	}

	#[test]
	fn idle_and_hidden_preserve_live_paint_focus_and_input() {
		let mut app = TestAppContext::single();
		let cx = &mut app;
		for hidden in [false, true] {
			let renders = Rc::new(Cell::new(0));
			let keys = Rc::new(Cell::new(0));
			let window = cx.add_window({
				let renders = renders.clone();
				let keys = keys.clone();
				move |_, cx| IdleView { renders, keys, focus: cx.focus_handle() }
			});
			window.update(cx, |view, window, cx| window.focus(&view.focus, cx)).unwrap();
			draw(window, cx, true);
			let before = draw(window, cx, false);
			cx.dispatch_keystroke(window.into(), Keystroke::parse("a").unwrap());
			assert_eq!(keys.get(), 1);
			let renders_before = renders.get();

			if hidden {
				cx.simulate_window_visibility_change(window.into(), WindowVisibility::Hidden);
			} else {
				// Repeated notifications must also be harmless and must not draw.
				for _ in 0..3 {
					cx.simulate_window_frame_idle(window.into());
				}
			}
			assert_eq!(renders.get(), renders_before);
			window
				.update(cx, |view, window, _| {
					assert!(view.focus.is_focused(window));
					assert_eq!(format!("{:?}", window.painted_quads()), before);
				})
				.unwrap();

			if hidden {
				cx.simulate_window_visibility_change(window.into(), WindowVisibility::Visible);
			}
			cx.dispatch_keystroke(window.into(), Keystroke::parse("b").unwrap());
			assert_eq!(keys.get(), 2);
			assert_eq!(draw(window, cx, false), before, "cached replay after idle");
			assert_eq!(draw(window, cx, true), before, "full layout after idle");
			cx.dispatch_keystroke(window.into(), Keystroke::parse("c").unwrap());
			assert_eq!(keys.get(), 3);
		}
	}
}
