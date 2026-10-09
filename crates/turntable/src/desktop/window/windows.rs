use gpui_kit::Window;
use raw_window_handle::RawWindowHandle;
use windows::Win32::{
	Foundation::HWND,
	UI::WindowsAndMessaging::{IsIconic, SW_HIDE, SW_RESTORE, SW_SHOW, ShowWindow},
};

pub(crate) struct Visibility(HWND);

impl Visibility {
	pub(crate) fn new(window: &Window) -> Result<Self, String> {
		let handle = raw_window_handle::HasWindowHandle::window_handle(window).map_err(|error| error.to_string())?;
		let RawWindowHandle::Win32(handle) = handle.as_raw() else { return Err("The window has no Win32 handle".into()) };
		Ok(Self(HWND(handle.hwnd.get() as *mut std::ffi::c_void)))
	}

	#[expect(clippy::unnecessary_wraps, reason = "the platform-neutral visibility interface propagates X11 failures")]
	pub(crate) fn hide(&self, _: &mut Window) -> Result<(), String> {
		// SAFETY: this HWND belongs to the retained main GPUI window; all operations
		// and destruction run on its owning UI thread. ShowWindow returns old visibility,
		// not success/failure, so false is not an error.
		unsafe {
			let _ = ShowWindow(self.0, SW_HIDE);
		}
		Ok(())
	}

	#[expect(clippy::unnecessary_wraps, reason = "the platform-neutral visibility interface propagates X11 failures")]
	pub(crate) fn show(&self, window: &mut Window) -> Result<(), String> {
		// SAFETY: the HWND remains live because close-to-tray never destroys the window.
		unsafe {
			let state = if IsIconic(self.0).as_bool() { SW_RESTORE } else { SW_SHOW };
			let _ = ShowWindow(self.0, state);
		}
		window.activate_window();
		window.refresh();
		Ok(())
	}
}
