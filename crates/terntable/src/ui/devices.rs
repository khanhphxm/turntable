//! Spotify Connect in the views: the devices button, the menu that hangs from it, and the strip
//! that says where the music plays when it isn't here.

use tern_sdk::ui::{self, Node, html};

use super::{Tap, div, icon, text};
use crate::{
	app::{Layout, Msg, State, connect::DeviceList},
	spotify::{Device, DeviceKind},
	ui::icons::{Glyph, Icons},
};

/// Where the button sits in the view (the menu hangs from it): the bar's, and the compact
/// now-playing page's.
const BAR_NODE: &str = "dock.bar.right.devices";
const PAGE_NODE: &str = "main.app.now.top.devices";

/// The picture of a device, by type.
fn glyph(device: &Device) -> Glyph {
	match device.category() {
		DeviceKind::Computer => Glyph::Computer,
		DeviceKind::Phone => Glyph::Phone,
		DeviceKind::Speaker => Glyph::Speaker,
		DeviceKind::Tv => Glyph::Tv,
	}
}

/// What type of thing a device is, in words.
fn kind_label(device: &Device) -> &str {
	match device.category() {
		DeviceKind::Computer => "Computer",
		DeviceKind::Phone => "Phone",
		DeviceKind::Speaker => "Speaker",
		DeviceKind::Tv => "TV",
	}
}

/// The devices button: in accent while the music plays somewhere else.
pub fn button(state: &State, icons: &Icons, px: f64) -> Node<Msg> {
	let class = if state.remote().is_some() {
		"tt-btn tt-cast"
	} else if state.connect.menu {
		"tt-btn tt-on"
	} else {
		"tt-btn"
	};
	let label = match state.remote() {
		Some(device) => format!("Playing on {}", device.name),
		None => "Connect to a device".to_owned(),
	};
	html::el("button")
		.class(class)
		.key("devices")
		.title(label.clone())
		.aria(label)
		.child(icon(icons, Glyph::Device, px, Tap::Msg(Msg::DevicesMenu, None)))
		.on_click(Msg::DevicesMenu)
		.into()
}

/// `Playing on <device>` under the bar, in the accent color, while another device plays.
pub fn strip(state: &State, icons: &Icons) -> Option<Node<Msg>> {
	let device = state.remote()?;
	Some(
		div("tt-playing-on")
			.key("playing-on")
			.child(icon(icons, glyph(device), 14.0, Tap::NONE))
			.child(text("tt-trunc", format!("Playing on {}", device.name)))
			.into(),
	)
}

/// One device of the menu: its picture, name and what it is.
fn row(state: &State, icons: &Icons, device: &Device) -> Node<Msg> {
	let this = state.is_this_device(device);
	let kind = if this { "This player".to_owned() } else { kind_label(device).to_owned() };
	let sub = if device.volume_capable() || this { kind } else { format!("{kind} · its own volume") };
	let body = div("tt-dev-text").child(text("tt-dev-name tt-trunc", device.name.clone())).child(text("tt-dev-sub tt-trunc tt-dim", sub));
	let pick = Msg::PickDevice(device.id.clone());
	let key = if device.id.is_empty() { "this".to_owned() } else { device.id.clone() };
	html::el("button")
		.class("tt-menu-item tt-dev")
		.key(key)
		.child(icon(icons, glyph(device), 20.0, Tap::Msg(pick.clone(), None)))
		.child(body)
		.on_click(pick)
		.into()
}

/// The device that plays now: "Listening on" in accent, or a hint when nothing plays anywhere.
fn current(state: &State, icons: &Icons) -> Node<Msg> {
	let device = match state.remote() {
		Some(device) => Some(device.clone()),
		None if state.active => Some(Device {
			id:              String::new(),
			name:            state.device_name.clone(),
			is_active:       true,
			volume_percent:  Some(state.volume),
			supports_volume: Some(true),
			kind:            "Computer".into(),
		}),
		None => None,
	};
	match device {
		Some(device) => div("tt-dev tt-dev-now")
			.key("now")
			.child(icon(icons, glyph(&device), 20.0, Tap::NONE))
			.child(
				div("tt-dev-text").child(text("tt-dev-listening", "Listening on")).child(text("tt-dev-name tt-trunc", device.name.clone())),
			)
			.into(),
		None => div("tt-dev tt-dev-none")
			.key("now")
			.child(icon(icons, Glyph::Device, 20.0, Tap::NONE))
			.child(
				div("tt-dev-text")
					.child(text("tt-dev-name", "Nothing is playing"))
					.child(text("tt-dev-sub tt-dim", "Pick a device to listen on")),
			)
			.into(),
	}
}

/// The menu: the device that plays, then the others Spotify lists.
pub fn menu(state: &State, icons: &Icons) -> Node<Msg> {
	let mut menu = div("tt-menu tt-devices")
		.key("menu")
		.child(text("tt-menu-head", "Current device"))
		.child(current(state, icons))
		.child(div("tt-menu-rule").key("rule"))
		.child(text("tt-menu-head", "Select another device"));

	let note = |key: &'static str, message: String| -> Node<Msg> { html::div().class("tt-menu-note tt-dim").key(key).text(message).into() };
	if !state.web_ready() {
		menu = menu.child(note("note", "Connect your Spotify account in Settings to pick a device.".into()));
	} else {
		match &state.connect.devices {
			DeviceList::Unknown => menu = menu.child(note("note", "Looking for devices…".into())),
			DeviceList::Failed(message) => {
				menu = menu.child(note("note", message.clone())).child(
					html::el("button")
						.class("tt-menu-item tt-dev-retry")
						.key("retry")
						.child(html::span("Try again"))
						.on_click(Msg::RefreshDevices),
				);
			},
			DeviceList::Ready(_) => {
				let others = state.other_devices();
				if others.is_empty() {
					menu = menu.child(note("note", "No other devices found. Open Spotify on another device.".into()));
				}
				for device in &others {
					menu = menu.child(row(state, icons, device));
				}
			},
		}
	}

	// Compact: the button is at the top of the now-playing page, so the menu opens below it.
	let (id, above) = if state.layout() == Layout::Compact { (PAGE_NODE, false) } else { (BAR_NODE, true) };
	ui::overlay()
		.key("devices-menu")
		.role("tt.devices")
		.anchor(ui::Anchor::Node { id: id.to_owned(), above })
		.size(ui::OverlaySize::Sm)
		.child(menu)
		.into()
}
