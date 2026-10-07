//! Independent requests of one page, sent together.
//!
//! A page used to be a chain of requests, each waiting for the one before it (an artist's page:
//! six of them, one round trip each). These helpers run them side by side on scoped threads. The
//! Web API gate (`spotify::Gate`) still caps what is in flight, so a wide fan-out queues instead
//! of bursting into Spotify's rate limit.

use std::{
	panic::resume_unwind,
	sync::{
		Mutex, PoisonError,
		atomic::{AtomicUsize, Ordering},
	},
	thread,
};

use crate::spotify;

fn spawn_in<'scope, 'env, T: Send + 'scope>(
	scope: &'scope thread::Scope<'scope, 'env>,
	work: impl FnOnce() -> T + Send + 'scope,
) -> thread::ScopedJoinHandle<'scope, T> {
	// A child asks Spotify as urgently as its parent does.
	let priority = spotify::priority();
	scope.spawn(move || {
		spotify::set_priority(priority);
		work()
	})
}

/// `a` and `b`, side by side.
pub fn join<A: Send, B: Send>(a: impl FnOnce() -> A + Send, b: impl FnOnce() -> B + Send) -> (A, B) {
	thread::scope(|scope| {
		let second = spawn_in(scope, b);
		let first = a();
		match second.join() {
			Ok(second) => (first, second),
			Err(panic) => resume_unwind(panic),
		}
	})
}

/// `work` over `items`, at most `width` at a time, results in the order of `items`.
pub fn map<T: Send, R: Send>(items: Vec<T>, width: usize, work: impl Fn(T) -> R + Sync) -> Vec<R> {
	let total = items.len();
	if total <= 1 || width <= 1 {
		return items.into_iter().map(work).collect();
	}
	let slots: Vec<Mutex<Option<T>>> = items.into_iter().map(|item| Mutex::new(Some(item))).collect();
	let results: Vec<Mutex<Option<R>>> = (0..total).map(|_| Mutex::new(None)).collect();
	let next = AtomicUsize::new(0);
	let worker = || {
		loop {
			let i = next.fetch_add(1, Ordering::Relaxed);
			let Some(slot) = slots.get(i) else { return };
			let item = slot.lock().unwrap_or_else(PoisonError::into_inner).take();
			if let (Some(item), Some(out)) = (item, results.get(i)) {
				*out.lock().unwrap_or_else(PoisonError::into_inner) = Some(work(item));
			}
		}
	};
	thread::scope(|scope| {
		let helpers: Vec<_> = (1..width.min(total)).map(|_| spawn_in(scope, worker)).collect();
		worker();
		for helper in helpers {
			if let Err(panic) = helper.join() {
				resume_unwind(panic);
			}
		}
	});
	results.into_iter().filter_map(|slot| slot.into_inner().unwrap_or_else(PoisonError::into_inner)).collect()
}

#[cfg(test)]
mod tests {
	use std::time::Duration;

	use super::*;

	#[test]
	fn join_runs_both_sides_together_and_returns_both() {
		// Each side waits to hear from the other: run one after the other, the first would time out.
		let ((a_says, b_hears), (b_says, a_hears)) = (std::sync::mpsc::channel(), std::sync::mpsc::channel());
		let (a, b) = join(
			move || {
				let _ = a_says.send(());
				a_hears.recv_timeout(Duration::from_secs(5)).is_ok()
			},
			move || {
				let _ = b_says.send(());
				b_hears.recv_timeout(Duration::from_secs(5)).is_ok()
			},
		);
		assert_eq!((a, b), (true, true), "side by side, not one after the other");
	}

	#[test]
	fn map_keeps_the_order_and_never_exceeds_the_width() {
		let (now, peak) = (AtomicUsize::new(0), AtomicUsize::new(0));
		let out = map((0..12).collect(), 3, |i: usize| {
			let running = now.fetch_add(1, Ordering::SeqCst) + 1;
			peak.fetch_max(running, Ordering::SeqCst);
			thread::sleep(Duration::from_millis(20));
			now.fetch_sub(1, Ordering::SeqCst);
			i * 10
		});
		assert_eq!(out, (0..12).map(|i| i * 10).collect::<Vec<_>>());
		assert!(peak.load(Ordering::SeqCst) <= 3, "width 3");
		assert!(peak.load(Ordering::SeqCst) >= 2, "and really in parallel");
	}
}
