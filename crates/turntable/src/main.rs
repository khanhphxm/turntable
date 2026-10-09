//! turntable: a lightweight native Spotify player.

// A GUI app: no console window on Windows.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod assets;
mod bar;
mod components;
mod covers;
mod desktop;
mod dialogs;
mod eqgraph;
mod fullview;
mod libmenu;
mod lyrics;
mod media;
mod menu;
mod menus;
mod miniplayer;
mod nowplaying;
mod pages;
mod panel;
mod resize;
mod shell;
mod sidebar;
mod theme;
mod topbar;
mod widgets;
mod window_state;

use std::sync::Arc;

use gpui_kit::{
	AppContext as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _, TitlebarOptions, WindowOptions, div, point, px,
	size,
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
		desktop::install(cx);
		shell::bind_keys(cx);
		menus::install(shell::OpenSettings, cx);
		let paths = Paths::resolve();
		let config = paths.as_ref().ok().map(|p| p.config.clone());
		let options = WindowOptions {
			window_bounds: Some(window_state::initial(config.as_deref(), cx)),
			titlebar: Some(TitlebarOptions {
				title:                  Some(menus::NAME.into()),
				appears_transparent:    true,
				// Centred in the 64 px top bar (a light's frame is ~14 px), inset like Spotify's.
				traffic_light_position: Some(point(px(20.), px(25.))),
			}),
			window_min_size: Some(size(px(800.), px(560.))),
			..Default::default()
		};
		// Workers wake the UI through this; one pending wake is as good as many.
		let (wake_tx, wake_rx) = smol::channel::bounded::<()>(1);
		let wake = Arc::new(move || {
			let _ = wake_tx.try_send(());
		});
		let core = paths.map_err(|e| e.to_string()).and_then(|paths| Controller::new(&paths, Options::default(), wake));
		let mut shell = None;
		let opened = match core {
			Ok(core) => gpui_kit::open_window(options, cx, |window, cx| {
				#[cfg(target_os = "linux")]
				window.request_decorations(gpui_kit::WindowDecorations::Client);
				let entity = cx.new(|cx| shell::Shell::new(core, window, cx));
				if let Some(config) = config.clone() {
					remember_bounds(&entity, config, window, cx);
				}
				shell::run_core(&entity, wake_rx, cx);
				shell = Some(entity.clone());
				entity
			})
			.map(|_| ()),
			Err(e) => gpui_kit::open_window(options, cx, |_window, cx| {
				#[cfg(target_os = "linux")]
				_window.request_decorations(gpui_kit::WindowDecorations::Client);
				cx.new(|_| Failed(e.into()))
			})
			.map(|_| ()),
		};
		if opened.is_err() {
			cx.quit();
			return;
		}
		// TURNTABLE_BACKGROUND: open behind other windows without taking focus (automated checks).
		if std::env::var_os("TURNTABLE_BACKGROUND").is_none() {
			cx.activate(true);
		}
		if let Some(shell) = shell {
			let np = nowplaying::NowPlaying::start(&shell, cx);
			shell.update(cx, |s, _| s.now_playing = np);
		}
	});
}

/// Saves the window's size, place and state whenever they change (once moves settle).
fn remember_bounds(
	shell: &gpui_kit::Entity<shell::Shell>,
	config: std::path::PathBuf,
	window: &mut gpui_kit::Window,
	cx: &mut gpui_kit::App,
) {
	shell.update(cx, |_, cx| {
		let generation = std::rc::Rc::new(std::cell::Cell::new(0_u64));
		cx.observe_window_bounds(window, move |_, window, cx| {
			let bounds = window.window_bounds();
			let config = config.clone();
			let current = generation.get() + 1;
			generation.set(current);
			let generation = generation.clone();
			cx.spawn(async move |_, cx| {
				cx.background_executor().timer(window_state::SAVE_AFTER).await;
				if generation.get() == current {
					cx.background_executor().spawn(async move { window_state::save(&config, bounds) }).await;
				}
			})
			.detach();
		})
		.detach();
	});
}

/// The player couldn't start (its config or cache folder is unusable).
struct Failed(SharedString);

impl Render for Failed {
	fn render(&mut self, window: &mut gpui_kit::Window, _: &mut gpui_kit::Context<'_, Self>) -> impl IntoElement {
		div()
			.size_full()
			.relative()
			.flex()
			.flex_col()
			.items_center()
			.justify_center()
			.gap_2()
			.bg(theme::base())
			.text_color(theme::text())
			.child(div().text_size(px(20.)).child("Turntable couldn't start"))
			.child(div().text_size(px(14.)).text_color(theme::subdued()).child(self.0.clone()))
			.children(topbar::window_titlebar(window))
	}
}
