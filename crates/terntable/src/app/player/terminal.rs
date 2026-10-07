//! Terminal dimensions and, on Unix, the signal waiter that wakes the application event loop.

#[cfg_attr(unix, path = "terminal/unix.rs")]
#[cfg_attr(windows, path = "terminal/windows.rs")]
mod platform;

use std::sync::mpsc;

use tern_sdk::Waker;

use super::AppEvent;

pub(super) use platform::Signals;

// ------------------------------------------------------------- terminal size

/// The pane's size in cells. Tern's width is authoritative; a screen surface's
/// pty can keep a stale size (seen: 80×24 in a freshly opened tab), so the
/// tty's rows count only when its width agrees with Tern's. Rows 0 = unknown.
pub(super) fn pane_size(tsp_cols: Option<u32>) -> (u32, u16) {
	let (tty_cols, tty_rows) = platform::tty_size();
	match tsp_cols {
		Some(cols) if cols != tty_cols => (cols, 0),
		Some(cols) => (cols, tty_rows),
		None => (tty_cols, tty_rows),
	}
}

/// Blocks native signals before threads are started (a no-op where Tern supplies them).
pub(super) fn block_signals() -> Signals {
	platform::block_signals()
}

pub(super) fn spawn_signal_waiter(signals: Signals, tx: mpsc::Sender<AppEvent>, waker: Waker) {
	platform::spawn_signal_waiter(signals, move |resized| {
		let event = if resized { AppEvent::Resized } else { AppEvent::Quit };
		if tx.send(event).is_err() {
			return false;
		}
		waker.wake();
		true
	});
}
