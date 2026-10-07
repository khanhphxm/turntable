//! The output stage's loudness leveler and true-peak limiter.
//!
//! [`Leveler`] listens to what the player sends (K-weighted, ITU-R BS.1770) and moves one gain
//! toward the loudness the user chose: slowly when the music is louder than that (a slow
//! attack), more slowly still when it is quieter, and not at all while the audio is silent. The
//! slowness is what keeps it from pumping: the gain follows a track's level over seconds, never
//! its beat. What it cannot catch in time (the first notes of a louder track, a boost meeting a
//! loud passage) is the job of [`Limiter`], a look-ahead limiter whose detector sees inter-sample
//! peaks too, so the output never passes [`CEILING_DB`] true peak.
//!
//! Both are plain f32/f64 code, allocation-free once built, so they run on the audio thread.

use std::{collections::VecDeque, f64::consts::PI};

use super::eq::SAMPLE_RATE;

/// [`SAMPLE_RATE`] as a float.
const FS: f64 = 44_100.0;
const _: () = assert!(SAMPLE_RATE == 44_100, "the filters below are designed for 44.1 kHz");

/// Frames per measurement block: the gain is recomputed this often (1.45 ms).
pub const BLOCK: usize = 64;

/// The output's true-peak ceiling, in dB: 1 dB under full scale leaves room for the
/// converter that follows (CoreAudio resamples to the device's rate).
pub const CEILING_DB: f32 = -1.0;

/// A float as `f32` (audio samples keep f32 precision; the maths that makes them is f64).
fn narrow(x: f64) -> f32 {
	#[expect(clippy::cast_possible_truncation, reason = "audio samples are f32")]
	{
		x as f32
	}
}

fn to_linear(db: f64) -> f64 {
	10f64.powf(db / 20.0)
}

fn to_db(linear: f64) -> f64 {
	20.0 * linear.max(1e-9).log10()
}

/// BS.1770 loudness (LUFS) of a mean-square power summed over the channels.
fn lufs(power: f64) -> f64 {
	-0.691 + 10.0 * power.max(1e-12).log10()
}

// ------------------------------------------------------------------ K weighting

/// Normalised biquad coefficients (a0 = 1).
#[derive(Clone, Copy)]
struct Coefs {
	b0: f64,
	b1: f64,
	b2: f64,
	a1: f64,
	a2: f64,
}

impl Coefs {
	/// BS.1770's first stage: a high shelf (about +4 dB above 1.7 kHz) modelling the head.
	fn shelf() -> Self {
		let (f0, gain_db, q) = (1_681.974_450_955_533, 3.999_843_853_973_347, 0.707_175_236_955_419_6);
		let k = (PI * f0 / FS).tan();
		let vh = 10f64.powf(gain_db / 20.0);
		let vb = vh.powf(0.499_666_774_154_541_6);
		let a0 = 1.0 + k / q + k * k;
		Self {
			b0: (vh + vb * k / q + k * k) / a0,
			b1: 2.0 * (k * k - vh) / a0,
			b2: (vh - vb * k / q + k * k) / a0,
			a1: 2.0 * (k * k - 1.0) / a0,
			a2: (1.0 - k / q + k * k) / a0,
		}
	}

	/// Its second stage: a high-pass at 38 Hz.
	fn high_pass() -> Self {
		let (f0, q) = (38.135_470_876_024_44, 0.500_327_037_323_877_3);
		let k = (PI * f0 / FS).tan();
		let a0 = 1.0 + k / q + k * k;
		Self { b0: 1.0, b1: -2.0, b2: 1.0, a1: 2.0 * (k * k - 1.0) / a0, a2: (1.0 - k / q + k * k) / a0 }
	}
}

/// One biquad's memory (transposed direct form II).
#[derive(Clone, Copy, Default)]
struct Section {
	z1: f64,
	z2: f64,
}

impl Section {
	fn run(&mut self, c: &Coefs, x: f64) -> f64 {
		let y = c.b0 * x + self.z1;
		self.z1 = c.b1 * x - c.a1 * y + self.z2;
		self.z2 = c.b2 * x - c.a2 * y;
		y
	}
}

/// K weighting of one channel.
#[derive(Clone, Copy, Default)]
struct Weighting {
	shelf:     Section,
	high_pass: Section,
}

// ---------------------------------------------------------------------- leveler

/// How fast the gain moves: toward its goal with this time constant (s), at most this many dB/s.
#[derive(Clone, Copy)]
struct Slew {
	tau: f64,
	cap: f64,
}

/// Gain reduction in normal running: slow, so a louder passage is eased down, not clamped.
const CUT: Slew = Slew { tau: 1.5, cap: 6.0 };
/// Gain increase in normal running: slower still; quiet passages are raised, not chased.
const BOOST: Slew = Slew { tau: 4.0, cap: 1.5 };
/// The same in the seconds after a new track starts, when the old gain means nothing.
const FAST_CUT: Slew = Slew { tau: 0.4, cap: 18.0 };
const FAST_BOOST: Slew = Slew { tau: 0.8, cap: 6.0 };
/// How long a new track gets the fast slews (s of audible audio).
const FAST_SECONDS: f64 = 3.0;
/// The loudness that guides the gain is averaged over this long (s); a track's first seconds
/// are averaged over what has played of it.
const SHORT_TAU: f64 = 3.0;
/// Audio needed before the first estimate is trusted (s).
const MIN_ACTIVE: f64 = 0.4;
/// The smoothing of the loudness that tells audio from silence (s).
const MOMENTARY_TAU: f64 = 0.1;
/// Below this momentary loudness (LUFS) the stream is silence (a pause, a gap).
const GATE_SILENCE: f64 = -85.0;
/// Below this loudness, volume taken out, there is nothing worth levelling.
const GATE_QUIET: f64 = -55.0;
/// Gain errors this small are left alone: no creeping.
const DEAD_ZONE: f64 = 0.5;
const MAX_BOOST: f64 = 9.0;
const MAX_CUT: f64 = 18.0;
/// How far above the ceiling the loudest recent peaks may be taken by a boost before the
/// boost stops: the limiter trims a few dB now and then; more would be pumping.
const LIMIT_ALLOWANCE: f64 = 3.0;
/// How long the recent peak is remembered (s).
const PEAK_TAU: f64 = 3.0;
/// Estimates ignore a volume change's first moments (s): the player's event and the audio
/// that carries the change are not exactly in step.
const FREEZE: f64 = 0.5;
/// A block quieter than this (K-weighted power, about -100 dBFS) is digital silence: the player
/// paused and the output supplies zeros. It is left out of the average at once, instead of
/// pulling it down while the momentary level fades out.
const SILENT_BLOCK: f64 = 1e-10;

/// Moves one gain toward a loudness target. See the module docs.
pub struct Leveler {
	shelf:        Coefs,
	high_pass:    Coefs,
	left:         Weighting,
	right:        Weighting,
	target:       f64,
	/// The player's volume as attenuation in dB (<= 0), taken out of what is measured.
	volume_db:    f64,
	/// The power factor that takes the volume out.
	volume_power: f64,
	gain_db:      f64,
	/// The gain at the end of the last block.
	gain:         f32,
	/// Fast average of the power (silence detection), and the average the gain follows
	/// (volume taken out).
	momentary:    f64,
	short:        f64,
	/// Loudest recent sample, decaying.
	peak:         f64,
	/// Seconds of audible audio since the last new track.
	active:       f64,
	fast_left:    f64,
	freeze_left:  f64,
}

impl Default for Leveler {
	fn default() -> Self {
		Self::new()
	}
}

impl Leveler {
	pub fn new() -> Self {
		Self {
			shelf:        Coefs::shelf(),
			high_pass:    Coefs::high_pass(),
			left:         Weighting::default(),
			right:        Weighting::default(),
			target:       -14.0,
			volume_db:    0.0,
			volume_power: 1.0,
			gain_db:      0.0,
			gain:         1.0,
			momentary:    0.0,
			short:        0.0,
			peak:         0.0,
			active:       0.0,
			fast_left:    FAST_SECONDS,
			freeze_left:  0.0,
		}
	}

	/// The loudness to aim for, in LUFS.
	pub fn set_target(&mut self, lufs: f64) {
		self.target = lufs;
	}

	/// The player's volume as attenuation in dB (0 at full volume, negative below). The
	/// leveler takes it out of what it measures, so turning the volume down is not undone.
	pub fn set_volume_db(&mut self, db: f64) {
		let db = db.clamp(-90.0, 0.0);
		if (db - self.volume_db).abs() > 0.05 {
			self.freeze_left = FREEZE;
		}
		self.volume_db = db;
		self.volume_power = 10f64.powf(-db / 10.0);
	}

	/// A new track starts: what was measured of the last one means nothing now, so the gain
	/// settles fast, once enough of the new one has played.
	pub fn new_program(&mut self) {
		self.short = 0.0;
		self.peak = 0.0;
		self.active = 0.0;
		self.fast_left = FAST_SECONDS;
	}

	/// Starts over from unity gain with nothing measured (the stage was switched on).
	pub fn restart(&mut self) {
		self.gain_db = 0.0;
		self.gain = 1.0;
		self.momentary = 0.0;
		self.freeze_left = 0.0;
		self.new_program();
	}

	/// The gain being applied, in dB.
	#[cfg(test)]
	pub fn gain_db(&self) -> f64 {
		self.gain_db
	}

	/// Measures `buf` (interleaved stereo), moves the gain and applies it.
	pub fn process(&mut self, buf: &mut [f32]) {
		for chunk in buf.chunks_mut(BLOCK * 2) {
			let (power, peak) = self.measure(chunk);
			let frames = chunk.len() / 2;
			// A whole number of frames in a block never exceeds 64: exact in f64.
			self.update(power, peak, frames as f64 / FS);
			self.apply(chunk);
		}
	}

	/// The K-weighted mean power of a block summed over both channels, and its sample peak.
	fn measure(&mut self, chunk: &[f32]) -> (f64, f64) {
		let (frames, _) = chunk.as_chunks::<2>();
		let mut sum = 0.0;
		let mut peak = 0f32;
		for &[l, r] in frames {
			peak = peak.max(l.abs()).max(r.abs());
			let kl = self.left.high_pass.run(&self.high_pass, self.left.shelf.run(&self.shelf, f64::from(l)));
			let kr = self.right.high_pass.run(&self.high_pass, self.right.shelf.run(&self.shelf, f64::from(r)));
			sum += kl * kl + kr * kr;
		}
		let power = if frames.is_empty() { 0.0 } else { sum / frames.len() as f64 };
		(power, f64::from(peak))
	}

	fn update(&mut self, power: f64, peak: f64, dt: f64) {
		let a = 1.0 - (-dt / MOMENTARY_TAU).exp();
		self.momentary += a * (power - self.momentary);
		let raw = lufs(self.momentary);
		let audible = raw >= GATE_SILENCE && raw - self.volume_db >= GATE_QUIET;
		if self.freeze_left > 0.0 {
			self.freeze_left -= dt;
			return;
		}
		if !audible || power < SILENT_BLOCK {
			return;
		}
		self.active += dt;
		// The average of everything heard so far, until a full window of it exists.
		self.short += dt / self.active.min(SHORT_TAU) * (power * self.volume_power - self.short);
		self.peak = peak.max(self.peak * (-dt / PEAK_TAU).exp());
		if self.active < MIN_ACTIVE {
			return;
		}
		self.fast_left = (self.fast_left - dt).max(0.0);
		let fast = self.fast_left > 0.0;
		// A boost that would push the recent peaks well past the ceiling waits.
		let headroom = f64::from(CEILING_DB) - to_db(self.peak) + LIMIT_ALLOWANCE;
		let goal = (self.target - lufs(self.short)).clamp(-MAX_CUT, MAX_BOOST.min(headroom.max(0.0)));
		let error = goal - self.gain_db;
		let error = if error.abs() <= DEAD_ZONE { 0.0 } else { error - error.signum() * DEAD_ZONE };
		let slew = match (error < 0.0, fast) {
			(true, false) => CUT,
			(false, false) => BOOST,
			(true, true) => FAST_CUT,
			(false, true) => FAST_BOOST,
		};
		let rate = (error.abs() / slew.tau).min(slew.cap);
		self.gain_db += error.signum() * (rate * dt).min(error.abs());
	}

	/// Applies the gain to the block, ramping from the last block's to the new one.
	fn apply(&mut self, chunk: &mut [f32]) {
		let end = narrow(to_linear(self.gain_db));
		let (frames, _) = chunk.as_chunks_mut::<2>();
		let n = frames.len() as f32;
		let step = (end - self.gain) / n.max(1.0);
		let mut gain = self.gain;
		for frame in frames {
			gain += step;
			frame[0] *= gain;
			frame[1] *= gain;
		}
		self.gain = end;
	}
}

// ---------------------------------------------------------------------- limiter

/// Frames the limiter looks ahead (5 ms): a gain reduction ramps in over this long, so it
/// is complete when the peak arrives.
const LOOKAHEAD: usize = 220;
/// [`LOOKAHEAD`] for the sample counter.
const LOOKAHEAD_FRAMES: u64 = 220;
/// Taps of the true-peak detector's interpolator: it reads six frames ahead of the point it
/// measures.
const TAPS: usize = 12;
/// The tap that holds the frame an interval starts at.
const CENTER: usize = 5;
/// How far the detector's newest frame is past the interval it can measure.
const FIR_LAG: usize = TAPS - 1 - CENTER;
/// Points measured between two frames: four times oversampling.
const PHASES: usize = 3;
/// The frames the limiter delays the audio by.
pub const DELAY_FRAMES: usize = LOOKAHEAD + FIR_LAG;
/// How slowly the gain recovers after a reduction (s).
const RELEASE: f64 = 0.12;

/// A look-ahead limiter with a true-peak detector. The gain for a peak is decided
/// [`DELAY_FRAMES`] before the peak plays; the audio is delayed by as much.
pub struct Limiter {
	ceiling:    f32,
	kernels:    [[f32; TAPS]; PHASES],
	recover:    f32,
	/// The last [`TAPS`] frames of each channel, oldest first.
	history:    [[f32; TAPS]; 2],
	delay:      VecDeque<[f32; 2]>,
	/// Candidates for the smallest of the last [`LOOKAHEAD`] + 1 requirements, as
	/// (arrival number, gain): increasing, so the front is the smallest.
	window:     VecDeque<(u64, f32)>,
	/// The last [`LOOKAHEAD`] + 1 window minima and their sum: their mean is the gain to
	/// apply, which ramps down over the look-ahead and never exceeds any requirement in it.
	minima:     VecDeque<f32>,
	minima_sum: f64,
	/// The previous frame's requirement (a point between two frames is held by both).
	previous:   f32,
	/// The gain being applied (reductions at once, recovery slowly).
	gain:       f32,
	arrived:    u64,
}

impl Default for Limiter {
	fn default() -> Self {
		Self::new()
	}
}

/// The modified Bessel function I0, for the Kaiser window.
fn bessel_i0(x: f64) -> f64 {
	let (mut sum, mut term) = (1.0, 1.0);
	for k in 1..40 {
		let half = x / (2.0 * f64::from(k));
		term *= half * half;
		sum += term;
	}
	sum
}

/// The interpolator for the point `phase` (0..1) after frame [`CENTER`]: a windowed sinc,
/// scaled to a gain of exactly 1 at DC.
fn kernel(phase: f64) -> [f32; TAPS] {
	const BETA: f64 = 8.0;
	const HALF_WIDTH: f64 = 6.0;
	let mut taps = [0f64; TAPS];
	for (tap, i) in taps.iter_mut().zip(0..) {
		let x = f64::from(i) - CENTER as f64 - phase;
		let sinc = if x.abs() < 1e-9 { 1.0 } else { (PI * x).sin() / (PI * x) };
		let ratio = (x / HALF_WIDTH).abs().min(1.0);
		*tap = sinc * bessel_i0(BETA * (1.0 - ratio * ratio).sqrt()) / bessel_i0(BETA);
	}
	let total: f64 = taps.iter().sum();
	taps.map(|t| narrow(t / total))
}

impl Limiter {
	pub fn new() -> Self {
		let mut limiter = Self {
			ceiling:    narrow(to_linear(f64::from(CEILING_DB))),
			kernels:    [kernel(0.25), kernel(0.5), kernel(0.75)],
			recover:    narrow(1.0 - (-1.0 / (RELEASE * FS)).exp()),
			history:    [[0.0; TAPS]; 2],
			delay:      VecDeque::with_capacity(DELAY_FRAMES + 1),
			window:     VecDeque::with_capacity(LOOKAHEAD + 2),
			minima:     VecDeque::with_capacity(LOOKAHEAD + 2),
			minima_sum: 0.0,
			previous:   1.0,
			gain:       1.0,
			arrived:    0,
		};
		limiter.reset();
		limiter
	}

	/// Forgets everything: the audio that follows starts from silence.
	pub fn reset(&mut self) {
		self.history = [[0.0; TAPS]; 2];
		self.delay.clear();
		self.delay.extend(std::iter::repeat_n([0.0; 2], DELAY_FRAMES));
		self.window.clear();
		self.minima.clear();
		self.minima.extend(std::iter::repeat_n(1.0, LOOKAHEAD + 1));
		self.minima_sum = (LOOKAHEAD + 1) as f64;
		self.previous = 1.0;
		self.gain = 1.0;
		self.arrived = 0;
	}

	/// The gain reduction now being applied, in dB (0 or negative).
	#[cfg(test)]
	pub fn reduction_db(&self) -> f64 {
		to_db(f64::from(self.gain))
	}

	/// Limits interleaved stereo in place; the output is [`DELAY_FRAMES`] late.
	pub fn process(&mut self, buf: &mut [f32]) {
		let (frames, _) = buf.as_chunks_mut::<2>();
		for frame in frames {
			*frame = self.frame(*frame);
		}
	}

	fn frame(&mut self, input: [f32; 2]) -> [f32; 2] {
		// A non-finite sample must not poison the gain.
		let input = input.map(|s| if s.is_finite() { s } else { 0.0 });
		self.delay.push_back(input);
		let delayed = self.delay.pop_front().unwrap_or_default();
		let mut peak = 0f32;
		for (history, sample) in self.history.iter_mut().zip(input) {
			history.rotate_left(1);
			if let Some(newest) = history.last_mut() {
				*newest = sample;
			}
			peak = peak.max(history[CENTER].abs());
			for kernel in &self.kernels {
				let between: f32 = kernel.iter().zip(history.iter()).map(|(k, h)| k * h).sum();
				peak = peak.max(between.abs());
			}
		}
		let need = if peak > self.ceiling { self.ceiling / peak } else { 1.0 };
		let held = need.min(self.previous);
		self.previous = need;
		// The smallest requirement of the look-ahead, then its smoothing.
		let arrival = self.arrived;
		self.arrived += 1;
		while self.window.back().is_some_and(|&(_, v)| v >= held) {
			self.window.pop_back();
		}
		self.window.push_back((arrival, held));
		let oldest = arrival.saturating_sub(LOOKAHEAD_FRAMES);
		while self.window.front().is_some_and(|&(i, _)| i < oldest) {
			self.window.pop_front();
		}
		let smallest = self.window.front().map_or(1.0, |&(_, v)| v);
		self.minima.push_back(smallest);
		self.minima_sum += f64::from(smallest);
		if let Some(old) = self.minima.pop_front() {
			self.minima_sum -= f64::from(old);
		}
		let smooth = narrow(self.minima_sum / (LOOKAHEAD + 1) as f64).min(1.0);
		self.gain = if smooth < self.gain { smooth } else { self.gain + (smooth - self.gain) * self.recover };
		delayed.map(|s| s * self.gain)
	}
}

// ------------------------------------------------------------------------ tests

#[cfg(test)]
pub(super) mod testing {
	//! Test signals and a reference meter, independent of the code under test where it matters.

	use std::f64::consts::PI;

	use super::FS;

	fn frames(secs: f64) -> usize {
		let duration = std::time::Duration::try_from_secs_f64(secs).unwrap();
		usize::try_from(duration.as_nanos() * 44_100 / 1_000_000_000).unwrap()
	}

	/// Round the high-precision reference signal once to the PCM format consumed by the DSP.
	pub fn pcm(sample: f64) -> f32 {
		#[expect(clippy::cast_possible_truncation, reason = "reference signals are computed in f64, then intentionally rounded to f32 PCM")]
		let sample = sample as f32;
		sample
	}

	/// Stereo sine of `secs` seconds at `hz` and peak `amp`, phase `phase` radians.
	pub fn sine(hz: f64, secs: f64, amp: f64, phase: f64) -> Vec<f32> {
		let frames = frames(secs);
		(0..frames)
			.flat_map(|i| {
				let s = pcm(amp * (2.0 * PI * hz * i as f64 / FS + phase).sin());
				[s, s]
			})
			.collect()
	}

	/// Deterministic white noise in [-1, 1), the same in both channels or independent.
	pub fn noise(secs: f64, amp: f64, seed: u32, stereo: bool) -> Vec<f32> {
		let frames = frames(secs);
		let mut state = seed;
		let mut next = move || {
			state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
			(f64::from(state >> 8) / f64::from(1u32 << 24) * 2.0 - 1.0) * amp
		};
		let mut out = Vec::with_capacity(frames * 2);
		for _ in 0..frames {
			let l = pcm(next());
			let r = if stereo { pcm(next()) } else { l };
			out.extend([l, r]);
		}
		out
	}

	/// Pink-ish noise: white noise through a few one-pole lowpass sections (Paul Kellet's
	/// filter), scaled to RMS `rms` per channel.
	pub fn pink(secs: f64, rms: f64, seed: u32) -> Vec<f32> {
		let white = noise(secs, 1.0, seed, false);
		let (mut b0, mut b1, mut b2) = (0.0f64, 0.0f64, 0.0f64);
		let mut out: Vec<f64> = white
			.as_chunks::<2>()
			.0
			.iter()
			.map(|f| {
				let w = f64::from(f[0]);
				b0 = 0.997_65 * b0 + w * 0.099_046_0;
				b1 = 0.963_00 * b1 + w * 0.296_516_4;
				b2 = 0.570_00 * b2 + w * 1.052_691_3;
				b0 + b1 + b2 + w * 0.1848
			})
			.collect();
		let mean_square = out.iter().map(|s| s * s).sum::<f64>() / out.len() as f64;
		let scale = rms / mean_square.sqrt();
		for sample in &mut out {
			*sample *= scale;
		}
		out.into_iter()
			.flat_map(|s| {
				let sample = pcm(s);
				[sample, sample]
			})
			.collect()
	}

	/// Integrated loudness (LUFS) of stereo `buf`, silence skipped: BS.1770 with 400 ms blocks.
	pub fn loudness(buf: &[f32]) -> f64 {
		let mut meter = super::Leveler::new();
		let mut powers = Vec::new();
		for block in buf.chunks(super::BLOCK * 2) {
			powers.push(meter.measure(block).0);
		}
		let mean = powers.iter().sum::<f64>() / powers.len() as f64;
		super::lufs(mean)
	}

	/// True peak in dBFS of stereo `buf`, from 8 times oversampling with a long windowed sinc
	/// (a much better interpolator than the limiter's own).
	pub fn true_peak_db(buf: &[f32]) -> f64 {
		const UP: usize = 8;
		const HALF: usize = 24;
		let kernels: Vec<Vec<f64>> = (0..UP)
			.map(|phase| {
				let frac = phase as f64 / UP as f64;
				(0..2 * HALF)
					.map(|t| {
						// Tap t reads the frame t - HALF + 1 from the one the point follows.
						let x = t as f64 - (HALF as f64 - 1.0) - frac;
						let sinc = if x.abs() < 1e-9 { 1.0 } else { (PI * x).sin() / (PI * x) };
						// Blackman window over the kernel's width.
						let u = (x / HALF as f64).midpoint(1.0);
						sinc * (0.42 - 0.5 * (2.0 * PI * u).cos() + 0.08 * (4.0 * PI * u).cos())
					})
					.collect()
			})
			.collect();
		let mut peak = 0f64;
		for channel in 0..2 {
			let x: Vec<f64> = buf.iter().skip(channel).step_by(2).map(|&s| f64::from(s)).collect();
			for window in x.windows(2 * HALF) {
				for taps in &kernels {
					let y: f64 = taps.iter().zip(window).map(|(t, s)| t * s).sum();
					peak = peak.max(y.abs());
				}
			}
		}
		20.0 * peak.max(1e-9).log10()
	}
}

#[cfg(test)]
mod tests;
