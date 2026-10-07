//! Tern supplies resize, Ctrl+C and input-close teardown on Windows; no signal thread is needed.

pub(crate) struct Signals;

pub(super) fn tty_size() -> (u32, u16) {
	// TSP hello/Resize supplies the pane width; this SDK does not report rows.
	(80, 0)
}

pub(super) fn block_signals() -> Signals {
	Signals
}

pub(super) fn spawn_signal_waiter(_signals: Signals, _notify: impl Fn(bool) -> bool + Send + 'static) {}
