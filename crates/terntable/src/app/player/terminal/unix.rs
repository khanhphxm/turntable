//! Unix tty dimensions and the process-wide signal mask/waiter.

use std::thread;

pub(crate) struct Signals(libc::sigset_t);

pub(super) fn tty_size() -> (u32, u16) {
	// SAFETY: TIOCGWINSZ fills a winsize struct on our own stdout.
	unsafe {
		let mut ws: libc::winsize = std::mem::zeroed();
		if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0 { (u32::from(ws.ws_col), ws.ws_row) } else { (0, 0) }
	}
}

/// Every thread inherits this mask; only the waiter takes SIGWINCH and polite termination.
pub(super) fn block_signals() -> Signals {
	// SAFETY: building a signal set and masking it on the current thread.
	unsafe {
		let mut set: libc::sigset_t = std::mem::zeroed();
		libc::sigemptyset(&raw mut set);
		for sig in [libc::SIGWINCH, libc::SIGHUP, libc::SIGTERM, libc::SIGINT] {
			libc::sigaddset(&raw mut set, sig);
		}
		libc::pthread_sigmask(libc::SIG_BLOCK, &raw const set, std::ptr::null_mut());
		Signals(set)
	}
}

pub(super) fn spawn_signal_waiter(signals: Signals, notify: impl Fn(bool) -> bool + Send + 'static) {
	let _ = thread::Builder::new().name("signals".into()).stack_size(64 * 1024).spawn(move || {
		loop {
			let mut sig = 0;
			// SAFETY: sigwait on a set blocked in every thread.
			if unsafe { libc::sigwait(&raw const signals.0, &raw mut sig) } != 0 || !notify(sig == libc::SIGWINCH) {
				return;
			}
		}
	});
}
