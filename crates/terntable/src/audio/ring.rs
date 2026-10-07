//! Lock-free single-producer / single-consumer ring of f32 samples.

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

/// Storage in samples; a power of two so wrapping counters index correctly.
const CAPACITY: usize = 1 << 14;
const MASK: usize = CAPACITY - 1;

pub struct Ring {
	buf:  Box<[AtomicU32]>,
	/// Total samples consumed (written by the consumer only).
	head: AtomicUsize,
	/// Total samples produced (written by the producer only).
	tail: AtomicUsize,
}

impl Ring {
	pub fn new() -> Ring {
		Ring { buf: (0..CAPACITY).map(|_| AtomicU32::new(0)).collect(), head: AtomicUsize::new(0), tail: AtomicUsize::new(0) }
	}

	/// Samples currently buffered.
	pub fn len(&self) -> usize {
		self.tail.load(Ordering::Acquire).wrapping_sub(self.head.load(Ordering::Acquire))
	}

	/// Producer: appends as many samples as fit and returns the count.
	pub fn push(&self, src: &[f32]) -> usize {
		let tail = self.tail.load(Ordering::Relaxed);
		let free = CAPACITY - tail.wrapping_sub(self.head.load(Ordering::Acquire));
		let n = src.len().min(free);
		let slots = self.buf.iter().skip(tail & MASK).chain(self.buf.iter());
		for (s, slot) in src.iter().take(n).zip(slots) {
			slot.store(s.to_bits(), Ordering::Relaxed);
		}
		self.tail.store(tail.wrapping_add(n), Ordering::Release);
		n
	}

	/// Consumer: moves up to `dst.len()` samples out and returns the count.
	pub fn pop(&self, dst: &mut [f32]) -> usize {
		let head = self.head.load(Ordering::Relaxed);
		let avail = self.tail.load(Ordering::Acquire).wrapping_sub(head);
		let n = dst.len().min(avail);
		let slots = self.buf.iter().skip(head & MASK).chain(self.buf.iter());
		for (d, slot) in dst.iter_mut().take(n).zip(slots) {
			*d = f32::from_bits(slot.load(Ordering::Relaxed));
		}
		self.head.store(head.wrapping_add(n), Ordering::Release);
		n
	}
}

#[cfg(test)]
mod tests {
	use std::sync::Arc;

	use super::*;

	#[test]
	fn push_pop_wraps_and_respects_capacity() {
		let r = Ring::new();
		let data: Vec<f32> = (0..CAPACITY + 10).map(|i| i as f32).collect();
		assert_eq!(r.push(&data), CAPACITY);
		assert_eq!(r.push(&[1.0]), 0);
		let mut out = vec![0.0; 100];
		assert_eq!(r.pop(&mut out), 100);
		assert_eq!(out[99], 99.0);
		// Wrap around the end.
		assert_eq!(r.push(&data[CAPACITY..]), 10);
		let mut rest = vec![0.0; CAPACITY];
		assert_eq!(r.pop(&mut rest), CAPACITY - 100 + 10);
		assert_eq!(rest[0], 100.0);
		assert_eq!(rest[CAPACITY - 100 - 1], (CAPACITY - 1) as f32);
		assert_eq!(rest[CAPACITY - 100], CAPACITY as f32);
		assert_eq!(r.len(), 0);
	}

	#[test]
	fn threaded_stream_arrives_in_order() {
		let r = Arc::new(Ring::new());
		let total = 2_000_000usize;
		let p = {
			let r = r.clone();
			std::thread::spawn(move || {
				let mut next = 0usize;
				let mut chunk = [0f32; 333];
				while next < total {
					let n = chunk.len().min(total - next);
					for (i, c) in chunk[..n].iter_mut().enumerate() {
						*c = (next + i) as f32;
					}
					let mut sent = 0;
					while sent < n {
						sent += r.push(&chunk[sent..n]);
						std::thread::yield_now();
					}
					next += n;
				}
			})
		};
		let mut got = 0usize;
		let mut buf = [0f32; 512];
		while got < total {
			let n = r.pop(&mut buf);
			for (i, v) in buf[..n].iter().enumerate() {
				assert_eq!(*v, (got + i) as f32);
			}
			got += n;
		}
		p.join().unwrap();
	}
}
