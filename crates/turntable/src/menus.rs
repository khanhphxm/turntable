//! The application menu (macOS's menu bar; other platforms ignore it) and the About window.

use gpui_kit::{
	App, AppContext as _, Bounds, Context, FontWeight, IntoElement, Menu, MenuItem, OsAction, ParentElement as _, Render, Styled as _,
	SystemMenuType, TitlebarOptions, Window, WindowBounds, WindowOptions, actions,
	component::input::{Copy, Cut, Paste, Redo, SelectAll, Undo},
	div, img, point, px, size,
};

use crate::theme;

actions!(turntable_app, [About, Quit, Hide, HideOthers, ShowAll, Minimize, Zoom]);

/// The app's display name.
pub const NAME: &str = "Turntable";

/// Installs the menu bar and its app-wide handlers. `settings` opens the settings page in the
/// main window (bound to ⌘, by the caller's key map).
pub fn install(settings: impl gpui_kit::Action, cx: &mut App) {
	cx.on_action(|_: &About, cx| open_about(cx));
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

/// A small fixed window, like every Mac app's About panel.
fn open_about(cx: &mut App) {
	let bounds = Bounds::centered(None, size(px(360.), px(400.)), cx);
	let options = WindowOptions {
		window_bounds: Some(WindowBounds::Windowed(bounds)),
		titlebar: Some(TitlebarOptions {
			title:                  Some(format!("About {NAME}").into()),
			appears_transparent:    true,
			traffic_light_position: Some(point(px(12.), px(12.))),
		}),
		is_resizable: false,
		is_minimizable: false,
		..Default::default()
	};
	let _ = cx.open_window(options, |_, cx| cx.new(|_| AboutView));
	cx.activate(true);
}

struct AboutView;

impl Render for AboutView {
	fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
		let line = |text: &'static str| div().text_size(px(12.)).text_color(theme::subdued()).child(text);
		div()
			.size_full()
			.flex()
			.flex_col()
			.items_center()
			.justify_center()
			.gap_2()
			.px_8()
			.bg(theme::panel())
			.text_color(theme::text())
			.font_family(".SystemUIFont")
			.text_center()
			.child(img("icons/app.svg").size(px(96.)))
			.child(div().mt_2().text_size(px(20.)).font_weight(FontWeight::BOLD).child(NAME))
			.child(line(concat!("Version ", env!("CARGO_PKG_VERSION"))))
			.child(div().h_4())
			.child(line("A lightweight, native Spotify player."))
			.child(line("Requires Spotify Premium."))
			.child(div().h_2())
			.child(line("Not affiliated with, endorsed or sponsored by Spotify AB."))
			.child(line("Spotify is a trademark of Spotify AB."))
			.child(line("Music, artwork and metadata are provided by Spotify."))
			.child(div().h_2())
			.child(line("Built with GPUI and librespot. Icons by Lucide (ISC)."))
			.child(line("MIT License."))
	}
}
