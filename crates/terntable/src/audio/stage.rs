//! The output stage after the equalizer: mono, volume normalization and balance.
//!
//! Order matters. Mono comes first, so the leveler measures what is actually played. Then the
//! leveler and the look-ahead limiter (`leveler`), which keep tracks at one
//! loudness without ever clipping. Balance comes last: it only turns a channel down, so it can
//! neither clip nor make the leveler chase it.
//!
//! Every change is ramped (tens of milliseconds), and with everything off the stage does not
//! touch a sample: off is bit-exact and costs nothing.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::leveler::{BLOCK, Leveler, Limiter};

/// How hard volume normalization works: the loudness it levels tracks to, as Spotify names them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Normalize {
	#[default]
	Off,
	/// -19 LUFS.
	Quiet,
	/// -14 LUFS.
	Normal,
	/// -11 LUFS.
	Loud,
}

impl Normalize {
	pub const ALL: [Self; 4] = [Self::Off, Self::Quiet, Self::Normal, Self::Loud];

	/// The loudness to level to, in LUFS (`None`: don't).
	pub fn target(self) -> Option<f64> {
		match self {
			Self::Off => None,
			Self::Quiet => Some(-19.0),
			Self::Normal => Some(-14.0),
			Self::Loud => Some(-11.0),
		}
	}

	pub fn label(self) -> &'static str {
		match self {
			Self::Off => "Off",
			Self::Quiet => "Quiet",
			Self::Normal => "Normal",
			Self::Loud => "Loud",
		}
	}

	pub(super) fn code(self) -> u8 {
		match self {
			Self::Off => 0,
			Self::Quiet => 1,
			Self::Normal => 2,
			Self::Loud => 3,
		}
	}

	pub(super) fn from_code(code: u8) -> Self {
		match code {
			1 => Self::Quiet,
			2 => Self::Normal,
			3 => Self::Loud,
			_ => Self::Off,
		}
	}

	fn key(self) -> &'static str {
		match self {
			Self::Off => "off",
			Self::Quiet => "quiet",
			Self::Normal => "normal",
			Self::Loud => "loud",
		}
	}
}

impl Serialize for Normalize {
	fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
		s.serialize_str(self.key())
	}
}

/// A settings file from the future, or edited by hand, may say anything: unknown means off,
/// and must not cost the rest of the file.
impl<'de> Deserialize<'de> for Normalize {
	fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
		let text = String::deserialize(d)?;
		Ok(Self::ALL.into_iter().find(|n| n.key() == text).unwrap_or_default())
	}
}

impl fmt::Display for Normalize {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str(self.label())
	}
}

/// What the stage does. `balance` runs from -1 (all left) to 1 (all right).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StageParams {
	pub normalize: Normalize,
	pub mono:      bool,
	pub balance:   f32,
	/// The player's volume as attenuation in dB (<= 0), so normalization can leave it alone.
	pub volume_db: f32,
}

impl Default for StageParams {
	fn default() -> Self {
		Self { normalize: Normalize::Off, mono: false, balance: 0.0, volume_db: 0.0 }
	}
}

/// The attenuation of the player's cubic volume curve (0 to 100), in dB. Normalization
/// measures ahead of this attenuation so it can leave the user's volume knob alone.
pub fn volume_db(percent: u8) -> f32 {
	if percent >= 100 {
		return 0.0;
	}
	(60.0 * (f32::from(percent.max(1)) / 100.0).log10()).max(-60.0)
}

/// Frames the leveled path takes to fade in or out when normalization is switched.
const FADE_FRAMES: f32 = 1024.0;

/// Frames a mono or balance change takes.
const RAMP_FRAMES: f32 = 1536.0;

/// Zero frames in a row after which the stage's output is silence for good.
#[expect(clippy::cast_possible_truncation, reason = "a few hundred frames")]
const SETTLED_ZEROS: u32 = 2 * super::leveler::DELAY_FRAMES as u32;

/// Moves `value` toward `target` by at most `step`.
fn approach(value: f32, target: f32, step: f32) -> f32 {
	if (target - value).abs() <= step { target } else { value + step.copysign(target - value) }
}

pub struct OutputStage {
	normalize:   Normalize,
	mono_to:     f32,
	balance_to:  f32,
	mono:        f32,
	balance:     f32,
	/// How much of the output is the leveled path (the rest is what came in).
	wet:         f32,
	/// The leveled path is being fed (it is, from switching on until it has faded out).
	active:      bool,
	leveler:     Leveler,
	limiter:     Limiter,
	/// What came in, per block, for the fade between it and the leveled path.
	dry:         Vec<f32>,
	zero_frames: u32,
}

impl Default for OutputStage {
	fn default() -> Self {
		Self::new()
	}
}

impl OutputStage {
	pub fn new() -> Self {
		Self {
			normalize:   Normalize::Off,
			mono_to:     0.0,
			balance_to:  0.0,
			mono:        0.0,
			balance:     0.0,
			wet:         0.0,
			active:      false,
			leveler:     Leveler::new(),
			limiter:     Limiter::new(),
			dry:         Vec::with_capacity(BLOCK * 2),
			zero_frames: 0,
		}
	}

	/// Retargets the stage. Cheap to call every callback: only changes do anything.
	pub fn set(&mut self, params: &StageParams) {
		self.mono_to = f32::from(u8::from(params.mono));
		self.balance_to = if params.balance.is_finite() { params.balance.clamp(-1.0, 1.0) } else { 0.0 };
		if params.normalize != self.normalize {
			self.normalize = params.normalize;
			if let Some(target) = params.normalize.target() {
				self.leveler.set_target(target);
			}
		}
		self.leveler.set_volume_db(f64::from(params.volume_db));
	}

	/// A new track starts (the player says so): the leveler settles on it fast.
	pub fn new_program(&mut self) {
		if self.active {
			self.leveler.new_program();
		}
	}

	/// The leveler's gain, in dB (for tests and diagnostics).
	#[cfg(test)]
	pub fn gain_db(&self) -> f64 {
		self.leveler.gain_db()
	}

	fn leveling(&self) -> bool {
		self.normalize != Normalize::Off
	}

	/// Nothing to do: all off and settled.
	fn bypassed(&self) -> bool {
		!self.active && self.mono == 0.0 && self.mono_to == 0.0 && self.balance == 0.0 && self.balance_to == 0.0
	}

	/// The stage's output is silence for as long as its input is: nothing is ringing.
	pub fn is_idle(&self) -> bool {
		self.bypassed() || self.zero_frames >= SETTLED_ZEROS
	}

	/// Processes interleaved stereo in place.
	pub fn process(&mut self, buf: &mut [f32]) {
		if self.leveling() && !self.active {
			self.active = true;
			self.wet = 0.0;
			self.leveler.restart();
			self.limiter.reset();
		} else if !self.leveling() && self.active && self.wet == 0.0 {
			self.active = false;
		}
		if self.bypassed() {
			self.zero_frames = SETTLED_ZEROS;
			return;
		}
		for chunk in buf.chunks_mut(BLOCK * 2) {
			self.block(chunk);
		}
	}

	fn block(&mut self, chunk: &mut [f32]) {
		let step_mono = 1.0 / RAMP_FRAMES;
		let step_balance = 2.0 / RAMP_FRAMES;
		for frame in chunk.as_chunks_mut::<2>().0 {
			self.mono = approach(self.mono, self.mono_to, step_mono);
			if self.mono > 0.0 {
				let mid = f32::midpoint(frame[0], frame[1]);
				frame[0] += self.mono * (mid - frame[0]);
				frame[1] += self.mono * (mid - frame[1]);
			}
			self.zero_frames = if *frame == [0.0, 0.0] { self.zero_frames.saturating_add(1) } else { 0 };
		}
		if self.active {
			self.dry.clear();
			self.dry.extend_from_slice(chunk);
			self.leveler.process(chunk);
			self.limiter.process(chunk);
			let step_wet = 1.0 / FADE_FRAMES;
			let want = if self.leveling() { 1.0 } else { 0.0 };
			for (wet_frame, dry_frame) in chunk.as_chunks_mut::<2>().0.iter_mut().zip(self.dry.as_chunks::<2>().0) {
				self.wet = approach(self.wet, want, step_wet);
				for (out, dry) in wet_frame.iter_mut().zip(dry_frame) {
					// The limiter's ceiling holds even through the fade, and a bad sample can't escape.
					*out = (*dry + self.wet * (limited(*out) - *dry)).clamp(-1.0, 1.0);
				}
			}
		}
		for frame in chunk.as_chunks_mut::<2>().0 {
			self.balance = approach(self.balance, self.balance_to, step_balance);
			if self.balance != 0.0 {
				frame[0] *= (1.0 - self.balance).min(1.0);
				frame[1] *= (1.0 + self.balance).min(1.0);
			}
		}
	}
}

/// A limiter output sample as it will be played: finite.
fn limited(s: f32) -> f32 {
	if s.is_finite() { s } else { 0.0 }
}

#[cfg(test)]
mod tests {
	use super::{super::leveler::testing::*, *};

	fn params(normalize: Normalize) -> StageParams {
		StageParams { normalize, ..StageParams::default() }
	}

	/// Runs `input` through `stage` in callbacks of an awkward size.
	fn run(stage: &mut OutputStage, input: &[f32]) -> Vec<f32> {
		let mut out = input.to_vec();
		for chunk in out.chunks_mut(2 * 470) {
			stage.process(chunk);
		}
		out
	}

	fn tail_loudness(buf: &[f32], secs: usize) -> f64 {
		loudness(&buf[buf.len() - 2 * 44_100 * secs..])
	}

	#[test]
	fn everything_off_leaves_every_sample_alone() {
		let mut stage = OutputStage::new();
		stage.set(&StageParams::default());
		let input = noise(1.0, 0.9, 5, true);
		assert_eq!(run(&mut stage, &input), input);
		assert!(stage.is_idle());
	}

	#[test]
	fn mono_plays_the_sum_of_both_channels_in_both() {
		let mut stage = OutputStage::new();
		stage.set(&StageParams { mono: true, ..StageParams::default() });
		let input = noise(1.0, 0.5, 9, true);
		let out = run(&mut stage, &input);
		// After its ramp (about 35 ms), left and right are the same, and are what both summed to.
		for (o, i) in out.as_chunks::<2>().0.iter().zip(input.as_chunks::<2>().0).skip(4_000) {
			assert!((o[0] - o[1]).abs() < 1e-6);
			assert!((o[0] - i[0].midpoint(i[1])).abs() < 1e-6);
		}
		// And turning it off restores the original.
		stage.set(&StageParams::default());
		let again = run(&mut stage, &input);
		for (o, i) in again.iter().zip(&input).skip(4_000) {
			assert!((o - i).abs() < 1e-6);
		}
		assert!(stage.is_idle() || stage.bypassed());
	}

	#[test]
	fn balance_turns_only_the_far_channel_down() {
		let input = sine(440.0, 1.0, 0.5, 0.0);
		for (balance, left, right) in [(1.0, 0.0, 1.0), (0.5, 0.5, 1.0), (-0.5, 1.0, 0.5), (-1.0, 1.0, 0.0), (0.0, 1.0, 1.0)] {
			let mut stage = OutputStage::new();
			stage.set(&StageParams { balance, ..StageParams::default() });
			let out = run(&mut stage, &input);
			for (o, i) in out.as_chunks::<2>().0.iter().zip(input.as_chunks::<2>().0).skip(4_000) {
				assert!((o[0] - i[0] * left).abs() < 1e-6, "balance {balance}: left {} vs {}", o[0], i[0] * left);
				assert!((o[1] - i[1] * right).abs() < 1e-6, "balance {balance}: right");
			}
		}
		// A bad value is no balance at all.
		let mut stage = OutputStage::new();
		stage.set(&StageParams { balance: f32::NAN, ..StageParams::default() });
		assert_eq!(run(&mut stage, &input), input);
	}

	#[test]
	fn a_balance_move_is_ramped_not_a_click() {
		let input = sine(220.0, 1.0, 0.5, 0.0);
		let mut stage = OutputStage::new();
		let mut out = run(&mut stage, &input[..2 * 20_000]);
		stage.set(&StageParams { balance: 1.0, ..StageParams::default() });
		out.extend(run(&mut stage, &input[2 * 20_000..]));
		// The largest step of the left channel stays at the sine's own (2 pi f A / fs).
		let own = 2.0 * std::f32::consts::PI * 220.0 * 0.5 / 44_100.0;
		let step =
			out.as_chunks::<2>().0.iter().map(|f| f[0]).collect::<Vec<_>>().windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
		assert!(step < own * 1.05, "step {step} vs the sine's own {own}");
	}

	#[test]
	fn normalization_brings_quiet_and_loud_music_to_one_loudness() {
		// Pink noise reads about 2.7 LUFS above its RMS in dBFS: these need +7.7, -6.2, -1.7 and +6.3 dB.
		for (rms, target, level) in [
			(0.06, -14.0, Normalize::Normal),
			(0.3, -14.0, Normalize::Normal),
			(0.1, -19.0, Normalize::Quiet),
			(0.1, -11.0, Normalize::Loud),
		] {
			let mut stage = OutputStage::new();
			stage.set(&params(level));
			let out = run(&mut stage, &pink(30.0, rms, 4));
			let got = tail_loudness(&out, 5);
			assert!((got - target).abs() < 1.2, "{level} at rms {rms}: {got:.2} LUFS, wanted {target} (gain {:.1} dB)", stage.gain_db());
		}
	}

	#[test]
	fn normalization_never_clips_even_on_hot_peaky_material() {
		// Quiet pink noise with full-scale sine bursts on top: the leveler wants to boost, the peaks forbid it.
		let mut input = pink(12.0, 0.04, 12);
		let burst = sine(3_000.0, 0.05, 0.99, 0.0);
		for second in 0..12 {
			let at = 2 * 44_100 * second + 2 * 1000;
			for (i, s) in burst.iter().enumerate() {
				input[at + i] = *s;
			}
		}
		for level in [Normalize::Quiet, Normalize::Normal, Normalize::Loud] {
			let mut stage = OutputStage::new();
			stage.set(&params(level));
			let out = run(&mut stage, &input);
			assert!(out.iter().all(|s| s.abs() <= 1.0), "{level}: a sample over full scale");
			let peak = true_peak_db(&out[2 * 44_100..]);
			assert!(peak <= -0.7, "{level}: true peak {peak:.2} dBFS");
		}
	}

	#[test]
	fn a_loud_dense_master_is_turned_down_not_squashed() {
		// Near-full-scale noise (a brickwalled master): normalization lowers it, with no limiter work to speak of.
		let input = noise(30.0, 0.95, 3, true);
		let mut stage = OutputStage::new();
		stage.set(&params(Normalize::Normal));
		let out = run(&mut stage, &input);
		let got = tail_loudness(&out, 5);
		assert!((got - -14.0).abs() < 1.2, "{got}");
		assert!(stage.gain_db() < -3.0);
	}

	#[test]
	fn the_volume_knob_still_works_with_normalization_on() {
		// The same music at full volume and 20 dB down (the stage is told): 20 dB apart in the output.
		let music = pink(40.0, 0.1, 6);
		let run_at = |db: f32| {
			let mut stage = OutputStage::new();
			stage.set(&StageParams { normalize: Normalize::Normal, volume_db: db, ..StageParams::default() });
			let scaled: Vec<f32> = music.iter().map(|s| s * 10f32.powf(db / 20.0)).collect();
			tail_loudness(&run(&mut stage, &scaled), 5)
		};
		let (full, quiet) = (run_at(0.0), run_at(-20.0));
		assert!((full - quiet - 20.0).abs() < 1.5, "{full} vs {quiet}");
	}

	#[test]
	fn switching_normalization_on_and_off_makes_no_click() {
		let input = sine(500.0, 6.0, 0.5, 0.0);
		let own_step = 2.0 * std::f32::consts::PI * 500.0 * 0.5 / 44_100.0;
		let mut stage = OutputStage::new();
		let mut out = run(&mut stage, &input[..2 * 44_100]);
		stage.set(&params(Normalize::Normal));
		out.extend(run(&mut stage, &input[2 * 44_100..2 * 3 * 44_100]));
		stage.set(&StageParams::default());
		out.extend(run(&mut stage, &input[2 * 3 * 44_100..]));
		let left: Vec<f32> = out.as_chunks::<2>().0.iter().map(|f| f[0]).collect();
		let worst = left.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
		// The level moves a little (the leveler raises a quiet sine) but never jumps.
		assert!(worst < own_step * 1.5, "step {worst} vs the sine's own {own_step}");
		// Faded out again, the stage touches nothing.
		assert!(stage.bypassed());
		let tail = &out[out.len() - 2 * 100..];
		assert_eq!(tail, &input[input.len() - 2 * 100..]);
	}

	#[test]
	fn a_new_track_lets_the_leveler_settle_fast() {
		let mut stage = OutputStage::new();
		stage.set(&params(Normalize::Normal));
		run(&mut stage, &pink(30.0, 0.04, 1));
		stage.new_program();
		let out = run(&mut stage, &pink(8.0, 0.3, 2));
		let got = tail_loudness(&out, 2);
		assert!((got - -14.0).abs() < 2.5, "{got}");
	}

	#[test]
	fn it_is_idle_once_silence_has_run_through() {
		let mut stage = OutputStage::new();
		stage.set(&params(Normalize::Normal));
		run(&mut stage, &noise(0.5, 0.2, 1, true));
		assert!(!stage.is_idle());
		run(&mut stage, &vec![0.0; 2 * 4_000]);
		assert!(stage.is_idle());
		// And what it plays then is silence.
		let out = run(&mut stage, &vec![0.0; 2 * 1_000]);
		assert!(out.iter().all(|s| *s == 0.0));
	}

	#[test]
	fn volume_follows_the_pulseaudio_law() {
		assert_eq!(volume_db(100), 0.0);
		assert!((volume_db(50) - -18.06).abs() < 0.01);
		assert!((volume_db(10) - -60.0).abs() < 0.01);
		assert_eq!(volume_db(0), -60.0);
		assert_eq!(volume_db(1), -60.0);
		let mut last = f32::MIN;
		for p in 0..=100 {
			assert!(volume_db(p) >= last);
			last = volume_db(p);
		}
	}

	#[test]
	fn unknown_normalize_levels_mean_off() {
		assert_eq!(serde_json::from_str::<Normalize>("\"banana\"").unwrap(), Normalize::Off);
	}
}
