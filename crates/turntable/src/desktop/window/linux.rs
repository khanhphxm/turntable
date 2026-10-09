use gpui_kit::Window;
use raw_window_handle::RawWindowHandle;
use x11rb::{connection::Connection as _, protocol::xproto::ConnectionExt as _, rust_connection::RustConnection};

#[expect(clippy::large_enum_variant, reason = "one main-window backend is retained; boxing X11 adds an otherwise avoidable allocation")]
pub(crate) enum Visibility {
	X11 { connection: RustConnection, window: u32 },
	Wayland,
}

impl Visibility {
	pub(crate) fn new(window: &Window) -> Result<Self, String> {
		let handle = raw_window_handle::HasWindowHandle::window_handle(window).map_err(|error| error.to_string())?;
		let window = match handle.as_raw() {
			RawWindowHandle::Xcb(handle) => handle.window.get(),
			RawWindowHandle::Xlib(handle) => u32::try_from(handle.window).map_err(|error| error.to_string())?,
			RawWindowHandle::Wayland(_) => return Ok(Self::Wayland),
			_ => return Err("The window system does not support background windows".into()),
		};
		let (connection, _) = x11rb::connect(None).map_err(|error| error.to_string())?;
		Ok(Self::X11 { connection, window })
	}

	pub(crate) fn hide(&self, window: &mut Window) -> Result<(), String> {
		match self {
			Self::X11 { connection, window } => {
				connection.unmap_window(*window).map_err(|error| error.to_string())?.check().map_err(|error| error.to_string())?;
				connection.flush().map_err(|error| error.to_string())
			},
			Self::Wayland => {
				// Wayland does not allow another client connection to unmap a surface.
				// GPUI exposes minimization, so retain the live surface in the background.
				window.minimize_window();
				Ok(())
			},
		}
	}

	pub(crate) fn show(&self, window: &mut Window) -> Result<(), String> {
		if let Self::X11 { connection, window } = self {
			connection.map_window(*window).map_err(|error| error.to_string())?.check().map_err(|error| error.to_string())?;
			connection.flush().map_err(|error| error.to_string())?;
		}
		window.activate_window();
		window.refresh();
		Ok(())
	}
}
