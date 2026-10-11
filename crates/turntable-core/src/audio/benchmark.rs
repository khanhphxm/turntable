//! Offline access to the production ring, render DSP and device-rate converter.
//! Enabled only by `audio-bench`; never opens a device or contacts Spotify.

use std::sync::atomic::Ordering;

use super::{EqSettings, Feed, Normalize, Render, StageParams, resample::Resampler};

/// A device-less consumer of the exact PCM path used by platform output callbacks.
pub struct NullOutput {
	render:    Render,
	resampler: Resampler,
	callbacks: u64,
}

/// Scheduling counters for the measured stream (not timer interrupts or hardware IRQs).
#[derive(Debug, Clone, Copy)]
pub struct Scheduling {
	pub callbacks:    u64,
	pub writer_parks: u64,
	pub writer_wakes: u64,
	pub underruns:    u64,
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
			callbacks:    self.callbacks,
			writer_parks: shared.writer_parks.load(Ordering::Relaxed),
			writer_wakes: shared.writer_wakes.load(Ordering::Relaxed),
			underruns:    shared.underruns.load(Ordering::Relaxed),
		}
	}
}

/// Linux's process-wide CPU time, scheduler events and resident-memory high water mark.
#[cfg(target_os = "linux")]
pub struct ResourceUsage {
	pub cpu_seconds:          f64,
	pub voluntary_switches:   u64,
	pub involuntary_switches: u64,
	pub peak_rss_kib:         u64,
}

#[cfg(target_os = "linux")]
pub fn resource_usage() -> std::io::Result<ResourceUsage> {
	let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
	// SAFETY: usage is writable storage for the OS to initialize; RUSAGE_SELF includes all threads.
	if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
		return Err(std::io::Error::last_os_error());
	}
	// SAFETY: successful getrusage initialized the complete rusage value.
	let usage = unsafe { usage.assume_init() };
	let status = std::fs::read_to_string("/proc/self/status")?;
	let peak_rss = status
		.lines()
		.find_map(|line| line.strip_prefix("VmHWM:"))
		.and_then(|value| value.split_whitespace().next())
		.ok_or_else(|| std::io::Error::other("Linux process status lacks VmHWM"))?;
	Ok(ResourceUsage {
		cpu_seconds:          usage.ru_utime.tv_sec as f64
			+ usage.ru_utime.tv_usec as f64 * 1e-6
			+ usage.ru_stime.tv_sec as f64
			+ usage.ru_stime.tv_usec as f64 * 1e-6,
		voluntary_switches:   u64::try_from(usage.ru_nvcsw).map_err(std::io::Error::other)?,
		involuntary_switches: u64::try_from(usage.ru_nivcsw).map_err(std::io::Error::other)?,
		peak_rss_kib:         peak_rss.parse().map_err(std::io::Error::other)?,
	})
}
