//! Desktop integration lives on GPUI's main thread. Login state is read from the OS,
//! never serialized in player preferences; changing it writes the native registration
//! and reads it back so Settings also reflects external changes and failed writes.

mod login;
mod tray;
mod window;

use gpui_kit::{AnyWindowHandle, App, Entity, Global, KeyBinding, WeakEntity, Window, actions};
use turntable_core::Msg;

use crate::shell::Shell;

actions!(turntable_desktop, [CloseMainWindow]);

pub struct Desktop {
	pub login_enabled: bool,
	pub error:         Option<String>,
	icon:              Option<tray::StatusIcon>,
	requested:         bool,
	dock_hidden:       bool,
	window:            Option<AnyWindowHandle>,
	visibility:        Option<window::Visibility>,
	shell:             Option<WeakEntity<Shell>>,
}

impl Global for Desktop {}

impl Desktop {
	pub fn icon_enabled(&self) -> bool {
		self.icon.is_some()
	}
}

pub fn install(cx: &mut App) {
	let (login_enabled, error) = match login::is_enabled() {
		Ok(enabled) => (enabled, None),
		Err(error) => (false, Some(error)),
	};
	cx.set_global(Desktop {
		login_enabled,
		error,
		icon: None,
		requested: false,
		dock_hidden: false,
		window: None,
		visibility: None,
		shell: None,
	});
	cx.bind_keys([KeyBinding::new(if cfg!(target_os = "macos") { "cmd-w" } else { "ctrl-w" }, CloseMainWindow, Some("Shell"))]);
	cx.on_action(|_: &CloseMainWindow, cx| {
		if let Some(window) = cx.global::<Desktop>().window {
			let _ = window.update(cx, |_, window, cx| request_close(window, cx));
		}
	});
	let commands = tray::events();
	cx.spawn(async move |cx| {
		while let Ok(command) = commands.recv().await {
			cx.update(|cx| dispatch(command, cx));
		}
	})
	.detach();
}

pub fn set_login(enabled: bool, cx: &mut App) {
	let changed = login::set_enabled(enabled);
	let actual = login::is_enabled();
	let desktop = cx.global_mut::<Desktop>();
	desktop.login_enabled = actual.as_ref().copied().unwrap_or(false);
	desktop.error = changed.err().or_else(|| actual.err());
}

/// Attach after opening the real window: GPUI is now running its Cocoa/Win32 loop.
pub fn attach(shell: &Entity<Shell>, window: &mut Window, cx: &mut App) {
	let visibility = window::Visibility::new(window);
	let desktop = cx.global_mut::<Desktop>();
	desktop.window = Some(window.window_handle());
	desktop.shell = Some(shell.downgrade());
	match visibility {
		Ok(visibility) => desktop.visibility = Some(visibility),
		Err(error) => desktop.error = Some(error),
	}
	window.on_window_should_close(cx, |window, cx| {
		request_close(window, cx);
		false
	});
	shell.update(cx, |shell, cx| sync(shell, cx));
}

/// Native title-bar closes, the Linux client-side button and ⌘W/Ctrl+W all use
/// this path. Only a successfully created icon can keep the player alive.
pub fn request_close(window: &mut Window, cx: &mut App) {
	let Some(desktop) = cx.try_global::<Desktop>() else {
		window.remove_window();
		return;
	};
	if desktop.icon.is_none() {
		cx.quit();
		return;
	}
	if let Some(visibility) = &desktop.visibility
		&& let Err(error) = visibility.hide(window)
	{
		cx.global_mut::<Desktop>().error = Some(error);
		cx.refresh_windows();
	}
}

/// Also used for macOS Dock activation. Showing never recreates the core/window.
pub fn show(cx: &mut App) {
	let Some(handle) = cx.try_global::<Desktop>().and_then(|desktop| desktop.window) else { return };
	let _ = handle.update(cx, |_, window, cx| {
		let result = cx.global::<Desktop>().visibility.as_ref().map(|visibility| visibility.show(window));
		if let Some(Err(error)) = result {
			cx.global_mut::<Desktop>().error = Some(error);
		}
		cx.activate(true);
	});
}

/// Shows the window with the About dialog over it (the app menu's About, even while hidden).
pub fn show_about(cx: &mut App) {
	show(cx);
	let Some(desktop) = cx.try_global::<Desktop>() else { return };
	let (Some(handle), Some(shell)) = (desktop.window, desktop.shell.clone()) else { return };
	let _ = handle.update(cx, |_, window, cx| shell.update(cx, |shell, cx| shell.open_about(window, cx)));
}

fn dispatch(command: tray::Command, cx: &mut App) {
	match command {
		tray::Command::Show => show(cx),
		tray::Command::Quit => cx.quit(),
		command => {
			let shell = cx.global::<Desktop>().shell.clone();
			let msg = match command {
				tray::Command::Toggle => Msg::TogglePlay,
				tray::Command::Next => Msg::Next,
				tray::Command::Previous => Msg::Prev,
				tray::Command::Show | tray::Command::Quit => return,
			};
			if let Some(shell) = shell {
				let _ = shell.update(cx, |shell, cx| shell.send(msg, cx));
			}
		},
	}
}

/// Apply before persisting an interactive change; a missing Linux tray host must
/// not strand a hidden window or leave Settings claiming an icon was enabled.
pub fn set_icon(enabled: bool, cx: &mut App) -> bool {
	let desktop = cx.global_mut::<Desktop>();
	desktop.requested = enabled;
	if !enabled {
		desktop.icon = None;
		desktop.error = None;
		return true;
	}
	if desktop.visibility.is_none() {
		desktop.error = Some("Background windows are not supported by this window system".into());
		return false;
	}
	match tray::StatusIcon::new() {
		Ok(icon) => {
			desktop.icon = Some(icon);
			desktop.error = None;
			true
		},
		Err(error) => {
			desktop.error = Some(format!("Could not enable the status icon: {error}"));
			false
		},
	}
}

pub fn sync(shell: &Shell, cx: &mut App) {
	let Some(desktop) = cx.try_global::<Desktop>() else { return };
	if desktop.window.is_none() {
		return;
	}
	let prefs = &shell.st().prefs.desktop;
	if desktop.requested != prefs.status_icon {
		set_icon(prefs.status_icon, cx);
	}
	let desktop = cx.global_mut::<Desktop>();
	if let Some(icon) = &mut desktop.icon
		&& let Err(error) = icon.sync(shell.st().item.name(), shell.st().playing())
	{
		desktop.error = Some(error);
	}
	let hide_dock = cfg!(target_os = "macos") && prefs.hide_dock && desktop.icon.is_some();
	if desktop.dock_hidden != hide_dock {
		desktop.dock_hidden = hide_dock;
		cx.set_activation_policy(if hide_dock { gpui_kit::ActivationPolicy::Accessory } else { gpui_kit::ActivationPolicy::Regular });
		if !hide_dock {
			cx.activate(true);
		}
	}
}
