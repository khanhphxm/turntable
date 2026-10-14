//! The application menu (macOS's menu bar; other platforms ignore it).

use gpui_kit::{
	App, Menu, MenuItem, OsAction, SystemMenuType, actions,
	component::input::{Copy, Cut, Paste, Redo, SelectAll, Undo},
};

actions!(turntable_app, [About, Quit, Hide, HideOthers, ShowAll, Minimize, Zoom]);

/// The app's display name.
pub const NAME: &str = "Turntable";

/// Installs the menu bar and its app-wide handlers. `settings` opens the settings page in the
/// main window (bound to ⌘, by the caller's key map).
pub fn install(settings: impl gpui_kit::Action, cx: &mut App) {
	cx.on_action(|_: &About, cx| crate::desktop::show_about(cx));
	cx.on_action(|_: &Quit, cx| cx.quit());
	cx.on_action(|_: &Hide, cx| cx.hide());
	cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
	cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
	cx.on_action(|_: &Minimize, cx| {
		if let Some(window) = cx.active_window() {
			let _ = window.update(cx, |_, window, _| window.minimize_window());
		}
	});
	cx.on_action(|_: &Zoom, cx| {
		if let Some(window) = cx.active_window() {
			let _ = window.update(cx, |_, window, _| window.zoom_window());
		}
	});
	cx.bind_keys([
		gpui_kit::KeyBinding::new("cmd-q", Quit, None),
		gpui_kit::KeyBinding::new("cmd-h", Hide, None),
		gpui_kit::KeyBinding::new("alt-cmd-h", HideOthers, None),
		gpui_kit::KeyBinding::new("cmd-m", Minimize, None),
	]);
	cx.set_menus([
		Menu {
			name:     NAME.into(),
			items:    vec![
				MenuItem::action(format!("About {NAME}"), About),
				MenuItem::separator(),
				MenuItem::action("Settings…", settings),
				MenuItem::separator(),
				MenuItem::os_submenu("Services", SystemMenuType::Services),
				MenuItem::separator(),
				MenuItem::action(format!("Hide {NAME}"), Hide),
				MenuItem::action("Hide Others", HideOthers),
				MenuItem::action("Show All", ShowAll),
				MenuItem::separator(),
				MenuItem::action(format!("Quit {NAME}"), Quit),
			],
			disabled: false,
		},
		Menu {
			name:     "Edit".into(),
			items:    vec![
				MenuItem::os_action("Undo", Undo, OsAction::Undo),
				MenuItem::os_action("Redo", Redo, OsAction::Redo),
				MenuItem::separator(),
				MenuItem::os_action("Cut", Cut, OsAction::Cut),
				MenuItem::os_action("Copy", Copy, OsAction::Copy),
				MenuItem::os_action("Paste", Paste, OsAction::Paste),
				MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
			],
			disabled: false,
		},
		Menu {
			name:     "Window".into(),
			items:    vec![MenuItem::action("Minimize", Minimize), MenuItem::action("Zoom", Zoom)],
			disabled: false,
		},
	]);
}
