//! turntable: a lightweight native Spotify player.

// A GUI app: no console window on Windows.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod assets;
mod bar;
mod components;
mod covers;
mod mock;
mod pages;
mod panel;
mod shell;
mod sidebar;
mod theme;
mod topbar;
mod widgets;

use gpui_kit::{AppContext as _, Bounds, TitlebarOptions, WindowBounds, WindowOptions, point, px, size};

fn main() {
	gpui_kit::application().with_assets(assets::Assets).run(|cx| {
		gpui_kit::init(cx);
		gpui_kit::component::Theme::change(gpui_kit::component::ThemeMode::Dark, None, cx);
		shell::bind_keys(cx);
		let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
		let options = WindowOptions {
			window_bounds: Some(WindowBounds::Windowed(bounds)),
			titlebar: Some(TitlebarOptions {
				title:                  Some("turntable".into()),
				appears_transparent:    true,
				traffic_light_position: Some(point(px(14.), px(14.))),
			}),
			window_min_size: Some(size(px(800.), px(560.))),
			..Default::default()
		};
		if gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| shell::Shell::new(window, cx))).is_err() {
			cx.quit();
		}
		cx.activate(true);
	});
}
