//! 10-band graphic equaliser: RBJ-cookbook biquads in f32, smoothed parameter
//! changes, a soft-clip safety stage and a lock-free settings mailbox.
//!
//! Everything here is allocation-free after construction and safe to run on a
//! real-time audio thread.

use std::sync::{
	Mutex,
	atomic::{AtomicU32, Ordering, fence},
};

use serde::{Deserialize, Serialize};

/// Sample rate of the PCM stream (Hz).
pub const SAMPLE_RATE: u32 = 44_100;

/// Centre frequencies of the 10 bands: ISO 266's nominal octave centres (31.5 Hz,
/// 63 Hz, then 125 Hz up), labelled 32, 63, 125… as graphic equalizers are. The
/// first and last are shelves.
pub const BANDS_HZ: [f32; 10] = [31.5, 63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0];

const N: usize = BANDS_HZ.len();

/// Per-band gain limit (dB).
pub const MAX_GAIN_DB: f32 = 12.0;
/// Preamp range (dB).
pub const PREAMP_RANGE_DB: (f32, f32) = (-24.0, 12.0);

/// Peaking-filter Q: roughly one octave between neighbouring bands.
const Q_PEAK: f64 = 1.41;
/// Shelf Q (Butterworth-like, no overshoot).
const Q_SHELF: f64 = std::f64::consts::FRAC_1_SQRT_2;

/// What a band does to the spectrum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
	LowShelf,
	Peaking,
	HighShelf,
}

/// The shape and Q of band `band`: shelves at both ends, peaking filters between.
const fn shape(band: usize) -> (Shape, f64) {
	if band == 0 {
		(Shape::LowShelf, Q_SHELF)
	} else if band == N - 1 {
		(Shape::HighShelf, Q_SHELF)
	} else {
		(Shape::Peaking, Q_PEAK)
	}
}

/// Frames per processing block; coefficients update once per block while ramping.
const BLOCK: usize = 64;

/// Blocks a parameter change is spread over (12 x 64 frames ~ 17 ms).
const RAMP_BLOCKS: u32 = 12;

/// Soft-clip knee: samples below this magnitude pass untouched.
const KNEE: f32 = 0.9;

/// Filter state below this magnitude is considered dead (denormal guard).
const STATE_EPS_ACTIVE: f32 = 1e-30;

/// Decayed-identity-filter state below this is dropped (about -140 dBFS).
const STATE_EPS_IDLE: f32 = 1e-7;

/// Missing fields take their flat defaults, so a partial `settings.json` keeps the rest.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EqSettings {
	pub enabled:   bool,
	pub preamp_db: f32,
	pub gains_db:  [f32; 10],
}

impl EqSettings {
	pub const FLAT: EqSettings = EqSettings { enabled: false, preamp_db: 0.0, gains_db: [0.0; 10] };

	/// Looks a preset up by name (see [`PRESETS`]).
	pub fn preset(name: &str) -> Option<EqSettings> {
		PRESETS.iter().find(|(n, _)| *n == name).map(|(_, s)| *s)
	}

	/// Clamps to the supported ranges and replaces NaN with 0.
	pub fn sanitized(mut self) -> EqSettings {
		let clamp = |v: f32, lo: f32, hi: f32| if v.is_nan() { 0.0 } else { v.clamp(lo, hi) };
		self.preamp_db = clamp(self.preamp_db, PREAMP_RANGE_DB.0, PREAMP_RANGE_DB.1);
		for g in &mut self.gains_db {
			*g = clamp(*g, -MAX_GAIN_DB, MAX_GAIN_DB);
		}
		self
	}
}

impl Default for EqSettings {
	fn default() -> Self {
		Self::FLAT
	}
}

const fn preset(gains_db: [f32; 10]) -> EqSettings {
	EqSettings { enabled: true, preamp_db: 0.0, gains_db }
}

/// Built-in presets (name, settings). "flat" is the disabled EQ.
pub const PRESETS: &[(&str, EqSettings)] = &[
	("flat", EqSettings::FLAT),
	("bass_boost", preset([6.0, 5.0, 4.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0])),
	("bass_cut", preset([-6.0, -5.0, -3.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0])),
	("vocal", preset([-3.0, -3.0, -2.0, 1.0, 3.0, 4.0, 3.0, 1.0, 0.0, -1.0])),
	("treble_boost", preset([0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 4.0, 5.0, 6.0])),
	("loudness", preset([6.0, 4.0, 1.0, 0.0, -1.0, -1.0, 0.0, 2.0, 4.0, 5.0])),
	("rock", preset([4.0, 3.0, 2.0, 0.0, -1.0, -1.0, 0.0, 2.0, 3.0, 4.0])),
];

/// Normalised (a0 = 1) biquad coefficients, transposed direct form II.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Biquad {
	b0: f32,
	b1: f32,
	b2: f32,
	a1: f32,
	a2: f32,
}

impl Biquad {
	const IDENTITY: Biquad = Biquad { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 };
}

/// Designs band `band` at `gain_db` (RBJ audio-EQ cookbook). `cos_w0`/`sin_w0`
/// belong to the band's centre frequency.
#[expect(clippy::cast_possible_truncation, reason = "coefficients are designed in f64 and stored in f32")]
fn design(band: usize, gain_db: f32, cos_w0: f64, sin_w0: f64) -> Biquad {
	let a = 10f64.powf(f64::from(gain_db) / 40.0);
	let (b0, b1, b2, a0, a1, a2);
	let (kind, q) = shape(band);
	if kind != Shape::Peaking {
		let alpha = sin_w0 / (2.0 * q);
		let t = 2.0 * a.sqrt() * alpha;
		if kind == Shape::LowShelf {
			b0 = a * ((a + 1.0) - (a - 1.0) * cos_w0 + t);
			b1 = 2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0);
			b2 = a * ((a + 1.0) - (a - 1.0) * cos_w0 - t);
			a0 = (a + 1.0) + (a - 1.0) * cos_w0 + t;
			a1 = -2.0 * ((a - 1.0) + (a + 1.0) * cos_w0);
			a2 = (a + 1.0) + (a - 1.0) * cos_w0 - t;
		} else {
			b0 = a * ((a + 1.0) + (a - 1.0) * cos_w0 + t);
			b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0);
			b2 = a * ((a + 1.0) + (a - 1.0) * cos_w0 - t);
			a0 = (a + 1.0) - (a - 1.0) * cos_w0 + t;
			a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cos_w0);
			a2 = (a + 1.0) - (a - 1.0) * cos_w0 - t;
		}
	} else {
		let alpha = sin_w0 / (2.0 * q);
		b0 = 1.0 + alpha * a;
		b1 = -2.0 * cos_w0;
		b2 = 1.0 - alpha * a;
		a0 = 1.0 + alpha / a;
		a1 = -2.0 * cos_w0;
		a2 = 1.0 - alpha / a;
	}
	let inv = 1.0 / a0;
	Biquad { b0: (b0 * inv) as f32, b1: (b1 * inv) as f32, b2: (b2 * inv) as f32, a1: (a1 * inv) as f32, a2: (a2 * inv) as f32 }
}

/// Smooth knee above [`KNEE`]; slope-continuous, asymptote at +-1.
#[inline]
fn soft_clip(x: f32) -> f32 {
	let a = x.abs();
	if a <= KNEE {
		return x;
	}
	let room = 1.0 - KNEE;
	let u = (a - KNEE) / room;
	(KNEE + room * u / (1.0 + u)).copysign(x)
}

/// Stereo (interleaved) 10-band equaliser.
pub struct Equalizer {
	trig:  [(f64, f64); N],
	coefs: [Biquad; N],
	/// `[band][channel][z1, z2]`
	z:     [[[f32; 2]; 2]; N],
	/// Band currently needs to run (non-zero gain or filter tail still ringing).
	alive: [bool; N],

	cur_gain:    [f32; N],
	target_gain: [f32; N],
	step_gain:   [f32; N],
	cur_pre:     f32,
	target_pre:  f32,
	step_pre:    f32,
	steps_left:  u32,

	preamp: f32,
	limit:  bool,
	idle:   bool,
}

impl Default for Equalizer {
	fn default() -> Self {
		Self::new()
	}
}

impl Equalizer {
	/// A flat, bypassed equaliser.
	pub fn new() -> Equalizer {
		let mut trig = [(0.0, 0.0); N];
		for (t, hz) in trig.iter_mut().zip(BANDS_HZ) {
			let w0 = 2.0 * std::f64::consts::PI * f64::from(hz) / f64::from(SAMPLE_RATE);
			*t = (w0.cos(), w0.sin());
		}
		Equalizer {
			trig,
			coefs: [Biquad::IDENTITY; N],
			z: [[[0.0; 2]; 2]; N],
			alive: [false; N],
			cur_gain: [0.0; N],
			target_gain: [0.0; N],
			step_gain: [0.0; N],
			cur_pre: 0.0,
			target_pre: 0.0,
			step_pre: 0.0,
			steps_left: 0,
			preamp: 1.0,
			limit: false,
			idle: true,
		}
	}

	/// Retargets the equaliser. The change is spread over ~17 ms so slider moves
	/// do not click. Disabling ramps to flat, then bypasses.
	pub fn set(&mut self, s: &EqSettings) {
		let s = s.sanitized();
		let (gains, pre) = if s.enabled { (s.gains_db, s.preamp_db) } else { ([0.0; N], 0.0) };
		if gains == self.cur_gain && pre == self.cur_pre {
			self.target_gain = gains;
			self.target_pre = pre;
			self.steps_left = 0;
			return;
		}
		self.target_gain = gains;
		self.target_pre = pre;
		let n = RAMP_BLOCKS as f32;
		for ((step, target), cur) in self.step_gain.iter_mut().zip(gains).zip(self.cur_gain) {
			*step = (target - cur) / n;
		}
		self.step_pre = (pre - self.cur_pre) / n;
		self.steps_left = RAMP_BLOCKS;
		self.idle = false;
	}

	/// True when processing is a no-op (flat, ramp finished, tails decayed).
	pub fn is_bypassed(&self) -> bool {
		self.idle
	}

	/// Processes interleaved stereo samples in place.
	pub fn process(&mut self, buf: &mut [f32]) {
		if self.idle {
			return;
		}
		let mut rest = buf;
		while !rest.is_empty() {
			// While ramping, coefficients move once a block; settled, the whole buffer is one run.
			let len = if self.steps_left > 0 { (BLOCK * 2).min(rest.len()) } else { rest.len() };
			let (chunk, tail) = rest.split_at_mut(len);
			rest = tail;
			if self.steps_left > 0 {
				self.advance();
			}
			self.run_block(chunk);
			if self.steps_left == 0 && self.preamp == 1.0 && !self.alive.contains(&true) {
				self.idle = true;
				return;
			}
		}
	}

	/// One ramp step: move parameters and rebuild coefficients.
	#[expect(clippy::indexing_slicing, reason = "band < N and every per-band array has N entries")]
	fn advance(&mut self) {
		self.steps_left -= 1;
		let done = self.steps_left == 0;
		for i in 0..N {
			self.cur_gain[i] = if done { self.target_gain[i] } else { self.cur_gain[i] + self.step_gain[i] };
			let g = self.cur_gain[i];
			if g != 0.0 || self.alive[i] {
				let (c, s) = self.trig[i];
				self.coefs[i] = design(i, g, c, s);
				self.alive[i] = true;
			}
		}
		self.cur_pre = if done { self.target_pre } else { self.cur_pre + self.step_pre };
		self.preamp = if self.cur_pre == 0.0 { 1.0 } else { 10f32.powf(self.cur_pre / 20.0) };
		let max_gain = self.cur_gain.iter().fold(0.0f32, |m, &g| m.max(g));
		self.limit = self.cur_pre + max_gain > 0.0;
	}

	/// Runs the live bands over `chunk` as a wavefront: at step `t` band `k` filters frame `t - k`,
	/// which band `k - 1` finished the step before. The bands' recursions are independent within
	/// a step, so the CPU overlaps them instead of waiting on one band's feedback a frame at a time
	/// (the cascade is the same, sample for sample).
	#[expect(clippy::indexing_slicing, reason = "k < live <= N, and frame t - k < frames because of the step's band range")]
	fn run_block(&mut self, chunk: &mut [f32]) {
		let mut band = [0usize; N];
		let mut live = 0;
		for b in 0..N {
			if self.alive[b] {
				band[live] = b;
				live += 1;
			}
		}
		let mut coefs = [Biquad::IDENTITY; N];
		let mut state = [[0.0f32; 4]; N];
		for k in 0..live {
			let [[l1, l2], [r1, r2]] = self.z[band[k]];
			coefs[k] = self.coefs[band[k]];
			state[k] = [l1, l2, r1, r2];
		}
		let frames = chunk.len() / 2;
		if live > 0 && frames > 0 {
			for t in 0..frames + live - 1 {
				for k in (t + 1).saturating_sub(frames)..live.min(t + 1) {
					let n = 2 * (t - k);
					let c = coefs[k];
					let [l1, l2, r1, r2] = &mut state[k];
					let x = chunk[n];
					let y = c.b0 * x + *l1;
					*l1 = c.b1 * x - c.a1 * y + *l2;
					*l2 = c.b2 * x - c.a2 * y;
					chunk[n] = y;
					let x = chunk[n + 1];
					let y = c.b0 * x + *r1;
					*r1 = c.b1 * x - c.a1 * y + *r2;
					*r2 = c.b2 * x - c.a2 * y;
					chunk[n + 1] = y;
				}
			}
		}
		for k in 0..live {
			let b = band[k];
			let mut st = state[k];
			let eps = if self.cur_gain[b] == 0.0 { STATE_EPS_IDLE } else { STATE_EPS_ACTIVE };
			if st.iter().all(|v| v.abs() < eps) {
				st = [0.0; 4];
				// A decayed identity filter can retire; one with gain must keep running.
				if self.cur_gain[b] == 0.0 && self.steps_left == 0 {
					self.alive[b] = false;
				}
			}
			let [l1, l2, r1, r2] = st;
			self.z[b] = [[l1, l2], [r1, r2]];
		}
		if self.limit {
			for s in chunk {
				*s = soft_clip(*s * self.preamp);
			}
		} else if self.preamp != 1.0 {
			for s in chunk {
				*s *= self.preamp;
			}
		}
	}
}

/// Single-writer-at-a-time, wait-free-reader settings handoff (seqlock).
/// The audio thread polls; it never blocks and never allocates.
pub struct Mailbox {
	seq:     AtomicU32,
	words:   [AtomicU32; N + 2],
	writers: Mutex<()>,
}

impl Mailbox {
	pub fn new() -> Mailbox {
		Mailbox { seq: AtomicU32::new(0), words: std::array::from_fn(|_| AtomicU32::new(0)), writers: Mutex::new(()) }
	}

	pub fn publish(&self, s: &EqSettings) {
		let s = s.sanitized();
		let _guard = self.writers.lock().unwrap_or_else(|e| e.into_inner());
		let seq = self.seq.load(Ordering::Relaxed);
		self.seq.store(seq.wrapping_add(1), Ordering::Relaxed);
		fence(Ordering::Release);
		self.words[0].store(u32::from(s.enabled), Ordering::Relaxed);
		self.words[1].store(s.preamp_db.to_bits(), Ordering::Relaxed);
		for (w, g) in self.words[2..].iter().zip(s.gains_db) {
			w.store(g.to_bits(), Ordering::Relaxed);
		}
		self.seq.store(seq.wrapping_add(2), Ordering::Release);
	}

	/// Returns new settings if something was published since `*seen`.
	/// A concurrent write simply yields `None`; poll again next time.
	pub fn poll(&self, seen: &mut u32) -> Option<EqSettings> {
		let s1 = self.seq.load(Ordering::Acquire);
		if s1 == *seen || s1 & 1 == 1 {
			return None;
		}
		let enabled = self.words[0].load(Ordering::Relaxed) != 0;
		let preamp_db = f32::from_bits(self.words[1].load(Ordering::Relaxed));
		let mut gains_db = [0.0; N];
		for (g, w) in gains_db.iter_mut().zip(&self.words[2..]) {
			*g = f32::from_bits(w.load(Ordering::Relaxed));
		}
		fence(Ordering::Acquire);
		if self.seq.load(Ordering::Relaxed) != s1 {
			return None;
		}
		*seen = s1;
		Some(EqSettings { enabled, preamp_db, gains_db })
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	const FS: f32 = SAMPLE_RATE as f32;

	/// Deterministic white noise in [-0.5, 0.5).
	fn noise(n: usize, mut seed: u32) -> Vec<f32> {
		(0..n)
			.map(|_| {
				seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
				(seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
			})
			.collect()
	}

	#[expect(
		clippy::cast_possible_truncation,
		clippy::cast_sign_loss,
		reason = "test signals: short, positive lengths and samples that fit f32"
	)]
	fn sine_stereo(hz: f32, secs: f32, amp: f32) -> Vec<f32> {
		let frames = (FS * secs) as usize;
		let mut v = Vec::with_capacity(frames * 2);
		for i in 0..frames {
			let s = amp * (2.0 * std::f64::consts::PI * f64::from(hz) * i as f64 / f64::from(FS)).sin() as f32;
			v.push(s);
			v.push(s);
		}
		v
	}

	fn rms(buf: &[f32]) -> f64 {
		(buf.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / buf.len() as f64).sqrt()
	}

	/// Steady-state gain (dB) of the EQ for a sine of `hz`.
	fn gain_db_at(settings: &EqSettings, hz: f32) -> f64 {
		let mut eq = Equalizer::new();
		eq.set(settings);
		let mut buf = sine_stereo(hz, 2.0, 0.1);
		let input = buf.clone();
		eq.process(&mut buf);
		let tail = buf.len() / 2;
		20.0 * (rms(&buf[tail..]) / rms(&input[tail..])).log10()
	}

	fn settings_with(band: usize, db: f32) -> EqSettings {
		let mut s = EqSettings { enabled: true, ..EqSettings::FLAT };
		s.gains_db[band] = db;
		s
	}

	#[test]
	fn flat_is_identity() {
		let input = noise(2 * 44_100, 1);

		// Disabled: bit-exact bypass.
		let mut eq = Equalizer::new();
		let mut out = input.clone();
		eq.process(&mut out);
		assert_eq!(out, input);
		assert!(eq.is_bypassed());

		// Enabled with zero gains: also bypass.
		eq.set(&EqSettings { enabled: true, ..EqSettings::FLAT });
		let mut out = input.clone();
		eq.process(&mut out);
		assert_eq!(out, input);

		// Boost, then back to flat: after settling it must be transparent again.
		eq.set(&settings_with(5, 9.0));
		eq.process(&mut input.clone());
		eq.set(&EqSettings::FLAT);
		eq.process(&mut noise(2 * 44_100, 2));
		assert!(eq.is_bypassed());
		let mut out = input.clone();
		eq.process(&mut out);
		let worst = out.iter().zip(&input).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
		assert!(worst < 1e-5, "worst deviation {worst}");
	}

	#[test]
	fn flat_filters_without_bypass_are_identity() {
		// Run the biquads themselves at 0 dB (no bypass shortcut).
		for (band, hz) in BANDS_HZ.iter().enumerate() {
			let (c, s) = {
				let w0 = 2.0 * std::f64::consts::PI * f64::from(*hz) / f64::from(SAMPLE_RATE);
				(w0.cos(), w0.sin())
			};
			let bq = design(band, 0.0, c, s);
			assert_eq!(bq.b0, 1.0, "band {band}");
			assert_eq!(bq.b1, bq.a1, "band {band}");
			assert_eq!(bq.b2, bq.a2, "band {band}");
		}
	}

	#[test]
	fn band_shapes_match_the_designed_filters() {
		assert_eq!(shape(0).0, Shape::LowShelf);
		assert_eq!(shape(N - 1).0, Shape::HighShelf);
		assert!((1..N - 1).all(|band| shape(band).0 == Shape::Peaking));
		// What `design` builds must behave like the advertised shape: a 6 dB shelf
		// holds its gain at its own end of the spectrum only, a peak at neither.
		for band in [0, 5, 7, N - 1] {
			let s = settings_with(band, 6.0);
			let (low, high) = (gain_db_at(&s, 15.0), gain_db_at(&s, 21_000.0));
			let (want_low, want_high) = match shape(band).0 {
				Shape::LowShelf => (6.0, 0.0),
				Shape::Peaking => (0.0, 0.0),
				Shape::HighShelf => (0.0, 6.0),
			};
			assert!((low - want_low).abs() < 1.0, "band {band}: {low:.2} dB at 15 Hz, want {want_low}");
			assert!((high - want_high).abs() < 1.0, "band {band}: {high:.2} dB at 21 kHz, want {want_high}");
		}
	}

	#[test]
	fn peaking_boost_hits_target_and_stays_local() {
		let s = settings_with(5, 6.0);
		let at_1k = gain_db_at(&s, 1000.0);
		assert!((at_1k - 6.0).abs() < 0.1, "1 kHz gain {at_1k}");
		for hz in [31.0, 16_000.0] {
			let g = gain_db_at(&s, hz);
			assert!(g.abs() < 0.5, "{hz} Hz leaked {g} dB");
		}
	}

	#[test]
	fn cut_and_shelves_reach_target() {
		let g = gain_db_at(&settings_with(5, -9.0), 1000.0);
		assert!((g + 9.0).abs() < 0.1, "{g}");
		// Shelves: full gain well inside the shelf region.
		let low = gain_db_at(&settings_with(0, 6.0), 12.0);
		assert!((low - 6.0).abs() < 0.3, "low shelf {low}");
		let high = gain_db_at(&settings_with(9, 6.0), 21_000.0);
		assert!((high - 6.0).abs() < 0.5, "high shelf {high}");
		// And the shelf leaves the mids alone.
		assert!(gain_db_at(&settings_with(0, 6.0), 1000.0).abs() < 0.2);
		assert!(gain_db_at(&settings_with(9, 6.0), 1000.0).abs() < 0.2);
	}

	#[test]
	fn filters_stable_at_extremes() {
		let in_unit_circle = |bq: Biquad| {
			let (a1, a2) = (f64::from(bq.a1), f64::from(bq.a2));
			a2.abs() < 1.0 && a1.abs() < 1.0 + a2
		};
		for (band, hz) in BANDS_HZ.iter().enumerate() {
			let w0 = 2.0 * std::f64::consts::PI * f64::from(*hz) / f64::from(SAMPLE_RATE);
			for db in [-MAX_GAIN_DB, MAX_GAIN_DB] {
				assert!(in_unit_circle(design(band, db, w0.cos(), w0.sin())), "band {band} {db} dB");
			}
		}
		// Every band at +-12 simultaneously, plus alternating signs.
		let patterns: [[f32; N]; 3] =
			[[MAX_GAIN_DB; N], [-MAX_GAIN_DB; N], std::array::from_fn(|i| if i % 2 == 0 { MAX_GAIN_DB } else { -MAX_GAIN_DB })];
		for gains in patterns {
			let mut eq = Equalizer::new();
			eq.set(&EqSettings { enabled: true, preamp_db: -12.0, gains_db: gains });
			let mut buf = noise(2 * 3 * 44_100, 7);
			eq.process(&mut buf);
			assert!(buf.iter().all(|v| v.is_finite() && v.abs() <= 1.0), "unbounded output");
			// Impulse response must die away.
			let mut imp = vec![0.0f32; 2 * 4 * 44_100];
			imp[0] = 1.0;
			imp[1] = 1.0;
			let mut eq = Equalizer::new();
			eq.set(&EqSettings { enabled: true, preamp_db: 0.0, gains_db: gains });
			// Let the ramp finish before the impulse hits.
			eq.process(&mut vec![0.0; 2 * BLOCK * (RAMP_BLOCKS as usize + 1)]);
			eq.process(&mut imp);
			let tail = &imp[imp.len() - 2 * 1000..];
			assert!(tail.iter().all(|v| v.abs() < 1e-3), "impulse response does not decay");
		}
	}

	#[test]
	fn parameter_jump_does_not_click() {
		// A 1 kHz tone is switched +12 dB mid-stream; compare the largest
		// sample-to-sample step against the steady-state boosted waveform.
		let mut eq = Equalizer::new();
		eq.set(&settings_with(5, 12.0));
		let mut warm = sine_stereo(1000.0, 1.0, 0.1);
		eq.process(&mut warm);
		let steady = warm[warm.len() - 2 * 441..].chunks(2).map(|f| f[0]).collect::<Vec<_>>();
		let steady_step = steady.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);

		let mut eq = Equalizer::new();
		let mut tone = sine_stereo(1000.0, 1.0, 0.1);
		let (a, b) = tone.split_at_mut(2 * 22_050);
		eq.process(a);
		eq.set(&settings_with(5, 12.0));
		eq.process(b);
		let worst = tone.chunks(2).map(|f| f[0]).collect::<Vec<_>>().windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
		assert!(worst <= steady_step * 1.1, "click: {worst} vs {steady_step}");
	}

	#[test]
	fn channels_are_independent() {
		let mut eq = Equalizer::new();
		eq.set(&settings_with(5, 12.0));
		let mut buf = sine_stereo(1000.0, 0.5, 0.1);
		for f in buf.chunks_mut(2) {
			f[1] = 0.0;
		}
		eq.process(&mut buf);
		assert!(buf.chunks(2).all(|f| f[1] == 0.0));
		assert!(rms(&buf) > 0.05);
	}

	#[test]
	fn soft_clip_is_transparent_below_knee_and_bounded_above() {
		for i in -900..=900 {
			let x = i as f32 / 1000.0;
			assert_eq!(soft_clip(x), x);
		}
		let mut prev = soft_clip(KNEE);
		for i in 1..2000 {
			let y = soft_clip(KNEE + i as f32 * 0.01);
			assert!(y > prev && y < 1.0, "{y}");
			prev = y;
		}
		assert_eq!(soft_clip(-5.0), -soft_clip(5.0));
		// Boost hot content: output never exceeds full scale.
		let mut eq = Equalizer::new();
		eq.set(&settings_with(5, 12.0));
		let mut buf = sine_stereo(1000.0, 0.5, 0.9);
		eq.process(&mut buf);
		assert!(buf.iter().all(|s| s.abs() < 1.0));
	}

	#[test]
	fn sanitize_clamps_and_scrubs_nan() {
		let s = EqSettings { enabled: true, preamp_db: f32::NAN, gains_db: [99.0, -99.0, f32::NAN, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0] }
			.sanitized();
		assert_eq!(s.preamp_db, 0.0);
		assert_eq!(&s.gains_db[..3], &[MAX_GAIN_DB, -MAX_GAIN_DB, 0.0]);
	}

	#[test]
	fn mailbox_delivers_latest_and_never_tears() {
		let mb = Mailbox::new();
		let mut seen = 0;
		assert!(mb.poll(&mut seen).is_none());
		let s = settings_with(4, 3.5);
		mb.publish(&s);
		assert_eq!(mb.poll(&mut seen), Some(s));
		assert!(mb.poll(&mut seen).is_none());

		// Writer publishes settings whose fields are all equal to one value; a
		// torn read would show mixed values.
		let mb = std::sync::Arc::new(Mailbox::new());
		let w = {
			let mb = mb.clone();
			std::thread::spawn(move || {
				for i in 0..200_000u32 {
					let v = (i % 12) as f32;
					mb.publish(&EqSettings { enabled: true, preamp_db: v, gains_db: [v; N] });
				}
			})
		};
		let mut seen = 0;
		let mut reads = 0u32;
		while !w.is_finished() {
			if let Some(s) = mb.poll(&mut seen) {
				reads += 1;
				assert!(s.gains_db.iter().all(|&g| g == s.preamp_db), "torn read: {s:?}");
			}
		}
		w.join().unwrap();
		assert!(reads > 0);
	}
}
