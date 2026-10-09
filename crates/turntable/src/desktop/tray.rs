//! `tray-icon` uses GPUI's existing Cocoa/Win32 loop. On Linux its KSNI backend
//! owns a blocking D-Bus worker, not a GTK loop or a UI polling timer. Native
//! callbacks only send commands; GPUI receives them through a wakeable channel.

use tray_icon::{
	Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
	menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

const ID: &str = "turntable-status";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Command {
	Toggle,
	Next,
	Previous,
	Show,
	Quit,
}

fn command(id: &str) -> Option<Command> {
	match id {
		"turntable-tray-play" => Some(Command::Toggle),
		"turntable-tray-next" => Some(Command::Next),
		"turntable-tray-previous" => Some(Command::Previous),
		"turntable-tray-show" => Some(Command::Show),
		"turntable-tray-quit" => Some(Command::Quit),
		_ => None,
	}
}

pub(super) fn events() -> smol::channel::Receiver<Command> {
	let (tx, rx) = smol::channel::unbounded();
	let menu_tx = tx.clone();
	MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
		if let Some(command) = command(event.id.as_ref()) {
			let _ = menu_tx.try_send(command);
		}
	}));
	TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
		if event.id().as_ref() != ID {
			return;
		}
		let activate = matches!(event, TrayIconEvent::DoubleClick { button: MouseButton::Left, .. })
			|| (cfg!(target_os = "linux")
				&& matches!(event, TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. }));
		if activate {
			let _ = tx.try_send(Command::Show);
		}
	}));
	rx
}

pub(super) struct StatusIcon {
	icon:              TrayIcon,
	title:             MenuItem,
	play:              MenuItem,
	published_title:   String,
	published_playing: bool,
}

impl StatusIcon {
	pub(super) fn new() -> Result<Self, String> {
		#[cfg(target_os = "linux")]
		require_host()?;
		let title = MenuItem::new("No track playing", false, None);
		let play = MenuItem::with_id("turntable-tray-play", "Play", true, None);
		let previous = MenuItem::with_id("turntable-tray-previous", "Previous", true, None);
		let next = MenuItem::with_id("turntable-tray-next", "Next", true, None);
		let show = MenuItem::with_id("turntable-tray-show", "Show Turntable", true, None);
		let quit = MenuItem::with_id("turntable-tray-quit", "Quit Turntable", true, None);
		let menu = Menu::with_items(&[
			&title,
			&PredefinedMenuItem::separator(),
			&play,
			&next,
			&previous,
			&PredefinedMenuItem::separator(),
			&show,
			&quit,
		])
		.map_err(|error| error.to_string())?;
		let builder = TrayIconBuilder::new().with_id(ID).with_menu(Box::new(menu));
		#[cfg(target_os = "macos")]
		let builder = builder.with_icon_templated(icon()?);
		#[cfg(not(target_os = "macos"))]
		let builder = builder.with_icon(icon()?);
		let icon = builder
			.with_tooltip("Turntable")
			.with_autosave_name(ID)
			.with_guid(0xb16f_a608_b73b_4c53_8ee1_11a6_9e8a_2401)
			.build()
			.map_err(|error| error.to_string())?;
		Ok(Self { icon, title, play, published_title: "No track playing".into(), published_playing: false })
	}

	pub(super) fn sync(&mut self, title: &str, playing: bool) -> Result<(), String> {
		let title = if title.is_empty() { "No track playing" } else { title };
		if self.published_title != title {
			// Native menu labels interpret '&' as a mnemonic on Windows/Linux.
			if title.contains('&') {
				self.title.set_text(title.replace('&', "&&"));
			} else {
				self.title.set_text(title);
			}
			self.icon.set_tooltip(Some(title)).map_err(|error| error.to_string())?;
			self.published_title.clear();
			self.published_title.push_str(title);
		}
		if self.published_playing != playing {
			self.play.set_text(if playing { "Pause" } else { "Play" });
			self.published_playing = playing;
		}
		Ok(())
	}
}

#[cfg(not(target_os = "macos"))]
fn icon() -> Result<Icon, String> {
	let image = image::load_from_memory(include_bytes!("../../../../packaging/icons/32x32/apps/turntable.png"))
		.map_err(|error| error.to_string())?
		.into_rgba8();
	let (width, height) = image.dimensions();
	Icon::from_rgba(image.into_raw(), width, height).map_err(|error| error.to_string())
}

#[cfg(target_os = "macos")]
fn icon() -> Result<Icon, String> {
	// A template record silhouette, with a transparent spindle hole. AppKit supplies
	// the appropriate foreground color in light/dark menu bars; computed at compile time.
	const PIXELS: [u8; 18 * 18 * 4] = record();
	Icon::from_rgba(PIXELS.to_vec(), 18, 18).map_err(|error| error.to_string())
}

#[cfg(target_os = "macos")]
#[expect(
	clippy::indexing_slicing,
	clippy::cast_sign_loss,
	reason = "compile-time rasterization bounds the RGBA index and const range checks"
)]
const fn record() -> [u8; 18 * 18 * 4] {
	let mut pixels = [0; 18 * 18 * 4];
	let mut y = 0;
	while y < 18 {
		let mut x = 0;
		while x < 18 {
			let dx = x * 2 - 17_i32;
			let dy = y * 2 - 17_i32;
			let radius = dx * dx + dy * dy;
			if let 9..=256 = radius {
				pixels[((y * 18 + x) * 4 + 3) as usize] = 255;
			}
			x += 1;
		}
		y += 1;
	}
	pixels
}

#[cfg(target_os = "linux")]
fn require_host() -> Result<(), String> {
	use dbus::blocking::{Connection, stdintf::org_freedesktop_dbus::Properties as _};
	let connection = Connection::new_session().map_err(|error| error.to_string())?;
	let watcher = connection.with_proxy("org.kde.StatusNotifierWatcher", "/StatusNotifierWatcher", std::time::Duration::from_secs(2));
	let available: bool = watcher
		.get("org.kde.StatusNotifierWatcher", "IsStatusNotifierHostRegistered")
		.map_err(|error| format!("A StatusNotifier tray host is required (for example, Xfce's Status Tray): {error}"))?;
	if available { Ok(()) } else { Err("No StatusNotifier tray host is running".into()) }
}

#[cfg(test)]
mod tests {
	use super::{Command, command};

	#[test]
	fn menu_commands_cover_playback_window_and_quit_without_foreign_events() {
		assert_eq!(command("turntable-tray-play"), Some(Command::Toggle));
		assert_eq!(command("turntable-tray-next"), Some(Command::Next));
		assert_eq!(command("turntable-tray-previous"), Some(Command::Previous));
		assert_eq!(command("turntable-tray-show"), Some(Command::Show));
		assert_eq!(command("turntable-tray-quit"), Some(Command::Quit));
		assert_eq!(command("turntable-tray-title"), None);
		assert_eq!(command("another-application"), None);
	}

	#[cfg(target_os = "linux")]
	#[test]
	#[ignore = "requires a session D-Bus and a real StatusNotifier host (run under Xvfb + xfce4-panel)"]
	fn native_title_and_playback_updates() {
		let mut icon = super::StatusIcon::new().unwrap();
		icon.sync("Offline fixture title", true).unwrap();
		assert_eq!(icon.title.text(), "Offline fixture title");
		assert_eq!(icon.play.text(), "Pause");
		icon.sync("", false).unwrap();
		assert_eq!(icon.title.text(), "No track playing");
		assert_eq!(icon.play.text(), "Play");
	}
}
