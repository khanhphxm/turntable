//! Offline access to the production ring, render DSP and device-rate converter.
//! Enabled only by `audio-bench`; never opens a device or contacts Spotify.

use std::sync::atomic::Ordering;

use super::{EqSettings, Feed, Normalize, Render, StageParams, resample::Resampler};

/// A device-less consumer of the exact PCM path used by platform output callbacks.
pub struct NullOutput {
	render: Render,
	resampler: Resampler,
	callbacks: u64,
}

/// Scheduling counters for the measured stream (not timer interrupts or hardware IRQs).
#[derive(Debug, Clone, Copy)]
pub struct Scheduling {
	pub callbacks: u64,
	pub writer_parks: u64,
	pub writer_wakes: u64,
	pub underruns: u64,
}

impl NullOutput {
	pub fn new(feed: Feed, rate: u32, eq: EqSettings, normalize: Normalize) -> Self {
		feed.0.mailbox.publish(&eq);
		feed.0.stage.normalize.store(normalize.code(), Ordering::Relaxed);
		let mut render = Render::new(feed.0);
		render.stage.set(&StageParams { normalize, ..StageParams::default() });
		Self { render, resampler: Resampler::new(44_100, rate), callbacks: 0 }
	}

	/// Fills the same interleaved f32 buffer a device would consume; the caller paces it.
	pub fn render(&mut self, out: &mut [f32]) {
		debug_assert!(out.len() <= super::resample::MAX_OUT_FRAMES * 2 && out.len().is_multiple_of(2));
		self.resampler.process(out, &mut |input| {
			self.render.render(input);
		});
		self.callbacks += 1;
	}

	pub fn drained(&self) -> bool {
		self.render.shared.ended.load(Ordering::Acquire) && self.render.shared.ring.len() == 0
	}

	pub fn scheduling(&self) -> Scheduling {
		let shared = &self.render.shared;
		Scheduling {
			callbacks: self.callbacks,
			writer_parks: shared.writer_parks.load(Ordering::Relaxed),
			writer_wakes: shared.writer_wakes.load(Ordering::Relaxed),
			underruns: shared.underruns.load(Ordering::Relaxed),
		}
	}
}
