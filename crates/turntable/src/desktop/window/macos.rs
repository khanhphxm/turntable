use gpui_kit::Window;
use objc2::{MainThreadMarker, rc::Retained};
use objc2_app_kit::{NSView, NSWindow};
use raw_window_handle::RawWindowHandle;

pub(crate) struct Visibility(Retained<NSWindow>);

impl Visibility {
	pub(crate) fn new(window: &Window) -> Result<Self, String> {
		let _main = MainThreadMarker::new().ok_or("Desktop integration requires the main thread")?;
		let handle = raw_window_handle::HasWindowHandle::window_handle(window).map_err(|error| error.to_string())?;
		let RawWindowHandle::AppKit(handle) = handle.as_raw() else { return Err("The window has no AppKit handle".into()) };
		// SAFETY: raw-window-handle guarantees ns_view is a live NSView on the main
		// thread. GPUI retains it throughout this call; we retain its owning NSWindow.
		let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
		let native = view.window().ok_or("The GPUI view is not attached to a native window")?;
		Ok(Self(native))
	}

	#[expect(clippy::unnecessary_wraps, reason = "the platform-neutral visibility interface propagates X11 failures")]
	pub(crate) fn hide(&self, _: &mut Window) -> Result<(), String> {
		// Hide only this window, not the application; leave the Dock icon alone.
		self.0.orderOut(None);
		Ok(())
	}

	#[expect(clippy::unnecessary_wraps, reason = "the platform-neutral visibility interface propagates X11 failures")]
	pub(crate) fn show(&self, window: &mut Window) -> Result<(), String> {
		self.0.makeKeyAndOrderFront(None);
		window.activate_window();
		window.refresh();
		Ok(())
	}
}
