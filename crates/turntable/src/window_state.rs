//! The main window's size, place and state (windowed, maximized, full screen), remembered between
//! launches in the config folder. A saved place that no connected display shows any more (an
//! unplugged monitor) falls back to the centre of the main display.

use std::{path::PathBuf, time::Duration};

use gpui_kit::{App, Bounds, Pixels, WindowBounds, point, px, size};
use serde::{Deserialize, Serialize};

const FILE: &str = "window.json";
/// The first launch's size.
const DEFAULT: (f32, f32) = (1280., 800.);
/// How much of the window must stay on a display for its saved place to be used.
const VISIBLE: f32 = 120.;
/// Moves and resizes arrive continuously; the file is written once they settle.
pub const SAVE_AFTER: Duration = Duration::from_millis(600);

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Mode {
	Windowed,
	Maximized,
	Fullscreen,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct Saved {
	mode:   Mode,
	x:      f32,
	y:      f32,
	width:  f32,
	height: f32,
}

impl Saved {
	fn of(bounds: WindowBounds) -> Self {
		let (mode, b) = match bounds {
			WindowBounds::Windowed(b) => (Mode::Windowed, b),
			WindowBounds::Maximized(b) => (Mode::Maximized, b),
			WindowBounds::Fullscreen(b) => (Mode::Fullscreen, b),
		};
		Self { mode, x: f32::from(b.origin.x), y: f32::from(b.origin.y), width: f32::from(b.size.width), height: f32::from(b.size.height) }
	}

	fn bounds(self) -> Bounds<Pixels> {
		Bounds { origin: point(px(self.x), px(self.y)), size: size(px(self.width), px(self.height)) }
	}

	fn window_bounds(self) -> WindowBounds {
		match self.mode {
			Mode::Windowed => WindowBounds::Windowed(self.bounds()),
			Mode::Maximized => WindowBounds::Maximized(self.bounds()),
			Mode::Fullscreen => WindowBounds::Fullscreen(self.bounds()),
		}
	}
}

fn path(config: &std::path::Path) -> PathBuf {
	config.join(FILE)
}

/// Whether enough of `b` lies on one of `displays` to grab and move it.
fn reachable(b: Bounds<Pixels>, displays: &[Bounds<Pixels>]) -> bool {
	displays.iter().any(|d| {
		let w = (b.origin.x + b.size.width).min(d.origin.x + d.size.width) - b.origin.x.max(d.origin.x);
		let h = (b.origin.y + b.size.height).min(d.origin.y + d.size.height) - b.origin.y.max(d.origin.y);
		w >= px(VISIBLE) && h >= px(VISIBLE.min(f32::from(b.size.height)))
	})
}

/// Where to open the window: the saved place when a display still shows it, else centred.
pub fn initial(config: Option<&std::path::Path>, cx: &App) -> WindowBounds {
	let centred = || WindowBounds::Windowed(Bounds::centered(None, size(px(DEFAULT.0), px(DEFAULT.1)), cx));
	let Some(saved) = config.and_then(|c| std::fs::read(path(c)).ok()).and_then(|bytes| serde_json::from_slice::<Saved>(&bytes).ok())
	else {
		return centred();
	};
	let displays: Vec<_> = cx.displays().iter().map(|d| d.bounds()).collect();
	if saved.width < 200. || saved.height < 200. || !reachable(saved.bounds(), &displays) {
		return centred();
	}
	saved.window_bounds()
}

/// Writes the window's state (off the UI thread: the caller runs this on a background task).
pub fn save(config: &std::path::Path, bounds: WindowBounds) {
	if let Ok(bytes) = serde_json::to_vec(&Saved::of(bounds)) {
		let tmp = path(config).with_extension("json.tmp");
		if std::fs::write(&tmp, bytes).is_ok() {
			let _ = std::fs::rename(&tmp, path(config));
		}
	}
}

#[cfg(test)]
mod tests {
	use gpui_kit::{Bounds, point, px, size};

	use super::{Mode, Saved, reachable};

	fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<gpui_kit::Pixels> {
		Bounds { origin: point(px(x), px(y)), size: size(px(w), px(h)) }
	}

	#[test]
	fn a_window_left_on_an_unplugged_monitor_is_not_reachable() {
		let laptop = [rect(0., 0., 1512., 982.)];
		// It was on an external display to the right.
		assert!(!reachable(rect(1800., 100., 1280., 800.), &laptop));
		// Mostly off the left edge, but a strip is still grabbable.
		assert!(reachable(rect(-1100., 100., 1280., 800.), &laptop));
		// Only a sliver showing: not enough to grab.
		assert!(!reachable(rect(1450., 100., 1280., 800.), &laptop));
	}

	#[test]
	fn the_saved_state_round_trips_with_its_mode() {
		for mode in [Mode::Windowed, Mode::Maximized, Mode::Fullscreen] {
			let saved = Saved { mode, x: 10., y: 20., width: 1300., height: 820. };
			let json = serde_json::to_vec(&saved).unwrap();
			assert_eq!(serde_json::from_slice::<Saved>(&json).unwrap(), saved);
			assert_eq!(Saved::of(saved.window_bounds()), saved);
		}
	}
}
