//! turntable: a lightweight native Spotify player.

// A GUI app: no console window on Windows.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod assets;
mod bar;
mod components;
mod covers;
mod media;
mod pages;
mod panel;
mod shell;
mod sidebar;
mod theme;
mod topbar;
mod widgets;

use std::sync::Arc;

use gpui_kit::{
	AppContext as _, Bounds, IntoElement, ParentElement as _, Render, SharedString, Styled as _, TitlebarOptions, WindowBounds,
	WindowOptions, div, point, px, size,
};
use turntable_core::{Controller, Options, Paths};

/// jemalloc with one arena and no thread caches (`.cargo/config.toml`): unlike the system
/// allocators it hands freed pages back, so the footprint follows what is in use rather than the
/// peak. Windows keeps the system allocator (jemalloc doesn't build with MSVC).
#[cfg(not(windows))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

fn main() {
	gpui_kit::application().with_assets(assets::Assets).run(|cx| {
		gpui_kit::init(cx);
		gpui_kit::component::Theme::change(gpui_kit::component::ThemeMode::Dark, None, cx);
		shell::bind_keys(cx);
		let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
		let options = WindowOptions {
			window_bounds: Some(WindowBounds::Windowed(bounds)),
			titlebar: Some(TitlebarOptions {
				title: Some("turntable".into()),
				appears_transparent: true,
				traffic_light_position: Some(point(px(14.), px(14.))),
			}),
			window_min_size: Some(size(px(800.), px(560.))),
			..Default::default()
		};
		// Workers wake the UI through this; one pending wake is as good as many.
		let (wake_tx, wake_rx) = smol::channel::bounded::<()>(1);
		let wake = Arc::new(move || {
			let _ = wake_tx.try_send(());
		});
		let core = Paths::resolve().map_err(|e| e.to_string()).and_then(|paths| Controller::new(&paths, Options::default(), wake));
		let opened = match core {
			Ok(core) => gpui_kit::open_window(options, cx, |window, cx| {
				let shell = cx.new(|cx| shell::Shell::new(core, window, cx));
				shell::run_core(&shell, wake_rx, cx);
				shell
			})
			.map(|_| ()),
			Err(e) => gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| Failed(e.into()))).map(|_| ()),
		};
		if opened.is_err() {
			cx.quit();
		}
		cx.activate(true);
	});
}

/// The player couldn't start (its config or cache folder is unusable).
struct Failed(SharedString);

impl Render for Failed {
	fn render(&mut self, _: &mut gpui_kit::Window, _: &mut gpui_kit::Context<'_, Self>) -> impl IntoElement {
		div()
			.size_full()
			.flex()
			.flex_col()
			.items_center()
			.justify_center()
			.gap_2()
			.bg(theme::base())
			.text_color(theme::text())
			.child(div().text_size(px(20.)).child("turntable couldn't start"))
			.child(div().text_size(px(14.)).text_color(theme::subdued()).child(self.0.clone()))
	}
}
