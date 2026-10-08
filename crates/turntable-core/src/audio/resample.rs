//! Allocation-free, causal stereo sample-rate conversion for the HAL callback.
//!
//! A gcd-reduced rational clock selects Kaiser-windowed sinc phases; the filter
//! has a constant, linear-phase delay of half its tap count in input frames.
//! Small reduced numerators (including 44.1->48 kHz's 160/147) have an exact row
//! for every phase. Larger ones interpolate between a bounded number of rows,
//! without approximating the clock or accumulating floating-point timing error. The
//! complete coefficient table is always smaller than 256 KiB.
//!
//! The 48-tap base filter keeps 18 kHz within 0.1 dB at 44.1->48 kHz and
//! rejects the stopband by at least 70 dB. Pulls are deinterleaved once into
//! contiguous L/R history; the FIR uses eight-lane f32 accumulators per channel.
//!
//! Downsampling by more than two first uses cascaded 2:1 sinc decimators. They
//! share one coefficient row, keeping both the final filter length and its
//! phase table bounded even for extreme rate ratios. All history and 1,024-frame
//! pull buffers are allocated by `new`; `process` only reads or reuses them.

/// Maximum stereo output frames in one callback.
pub const MAX_OUT_FRAMES: usize = 8192;

const MAX_PULL_FRAMES: usize = 1024;
const BASE_TAPS: usize = 48;
const DECIMATION_TAPS: usize = BASE_TAPS * 2;
const COEFFICIENT_BUDGET: usize = 256 * 1024 / size_of::<f32>() - 1;
const KAISER_BETA: f64 = 7.5;
// At a 44.1 kHz lower rate: flat through 18 kHz, >=70 dB rejection from 22.05 kHz.
const CUTOFF: f64 = 19_750.0 / 44_100.0;

/// A streaming converter for interleaved stereo f32 PCM.
pub struct Resampler {
	filter:                  Option<Polyphase>,
	decimators:              Box<[Input]>,
	decimation_coefficients: Box<[f32]>,
}

impl Resampler {
	/// A converter from `from_hz` to `to_hz` for interleaved stereo f32.
	/// Allocates (not on the audio thread). Rates are in positive Hz.
	pub fn new(from_hz: u32, to_hz: u32) -> Self {
		if from_hz == to_hz {
			return Self {
				filter:                  None,
				decimators:              Box::default(),
				decimation_coefficients: Box::default(),
			};
		}
		let from = u64::from(from_hz.max(1));
		let to = u64::from(to_hz.max(1));
		let mut factor = 1;
		let mut stages = 0;
		while from > to * factor * 2 {
			factor *= 2;
			stages += 1;
		}
		let divisor = gcd(from, to * factor);
		let decimation_coefficients = if stages == 0 { Box::default() } else { design(DECIMATION_TAPS, CUTOFF * 0.5, 1, false) };
		let budget = COEFFICIENT_BUDGET - decimation_coefficients.len();
		Self {
			filter: Some(Polyphase::new(to * factor / divisor, from / divisor, budget)),
			decimators: (0..stages).map(|_| Input::new(DECIMATION_TAPS)).collect(),
			decimation_coefficients,
		}
	}

	/// Whether the rates are equal (then `process` is a plain copy through `pull`).
	#[cfg(test)]
	pub fn is_identity(&self) -> bool {
		self.filter.is_none()
	}

	/// Fills interleaved stereo `out` at the output rate. Its length must be even
	/// and at most `MAX_OUT_FRAMES * 2`. `pull` must fill every sample it is given
	/// at the input rate; those slices are even and at most 1,024 stereo frames.
	/// Never allocates, locks or blocks; work per frame is constant for the rates.
	pub fn process(&mut self, out: &mut [f32], pull: &mut impl FnMut(&mut [f32])) {
		let Some(filter) = self.filter.as_mut() else {
			for block in out.chunks_mut(MAX_PULL_FRAMES * 2) {
				pull(block);
			}
			return;
		};
		if self.decimators.is_empty() {
			filter.process(out, pull);
		} else {
			let stages = &mut self.decimators;
			let coefficients = &self.decimation_coefficients;
			filter.process(out, &mut |input| pull_decimated(stages, coefficients, input, pull));
		}
	}

	/// Forgets the history (after a gap: the next audio starts from silence).
	#[cfg(test)]
	pub fn reset(&mut self) {
		if let Some(filter) = self.filter.as_mut() {
			filter.phase = 0;
			filter.input.reset();
		}
		for stage in &mut self.decimators {
			stage.reset();
		}
	}
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
	while b != 0 {
		(a, b) = (b, a % b);
	}
	a
}

struct Polyphase {
	coefficients: Box<[f32]>,
	input:        Input,
	taps:         usize,
	phases:       usize,
	interpolated: bool,
	numerator:    u64,
	remainder:    u64,
	step:         usize,
	phase:        u64,
}

impl Polyphase {
	#[expect(
		clippy::cast_possible_truncation,
		clippy::cast_sign_loss,
		reason = "the staged input/output ratio is at most two: 48–96 taps, bounded phases and a 0–2 frame step"
	)]
	fn new(numerator: u64, denominator: u64, budget: usize) -> Self {
		let ratio = denominator as f64 / numerator as f64;
		let taps = ((BASE_TAPS as f64 * ratio.max(1.0)).ceil() as usize).next_multiple_of(8);
		let max_rows = budget / taps;
		let interpolated = numerator > max_rows as u64;
		let phases = if interpolated { max_rows - 1 } else { numerator as usize };
		Self {
			coefficients: design(taps, CUTOFF / ratio.max(1.0), phases, interpolated),
			input: Input::new(taps),
			taps,
			phases,
			interpolated,
			numerator,
			remainder: denominator % numerator,
			step: (denominator / numerator) as usize,
			phase: 0,
		}
	}

	#[expect(
		clippy::cast_possible_truncation,
		reason = "row indices fit the bounded coefficient table; rational interpolation weights are intentionally f32"
	)]
	fn process(&mut self, out: &mut [f32], pull: &mut impl FnMut(&mut [f32])) {
		for frame in out.as_chunks_mut::<2>().0 {
			let (left, right) = self.input.window(pull);
			*frame = if self.interpolated {
				// The scaled clock remains exact; only the coefficient rows interpolate.
				let scaled = self.phase * self.phases as u64;
				let index = (scaled / self.numerator) as usize;
				let fraction = (scaled % self.numerator) as f32 / self.numerator as f32;
				let rows = self.coefficients.split_at(index * self.taps).1;
				let (first, rest) = rows.split_at(self.taps);
				let second = rest.split_at(self.taps).0;
				let [a_left, a_right] = dot_stereo(left, right, first);
				let [b_left, b_right] = dot_stereo(left, right, second);
				[fraction.mul_add(b_left - a_left, a_left), fraction.mul_add(b_right - a_right, a_right)]
			} else {
				let row = self.coefficients.split_at(self.phase as usize * self.taps).1.split_at(self.taps).0;
				dot_stereo(left, right, row)
			};
			self.input.cursor += self.step;
			self.phase += self.remainder;
			if self.phase >= self.numerator {
				self.phase -= self.numerator;
				self.input.cursor += 1;
			}
		}
	}
}

/// Separate contiguous channel histories followed by one deinterleaved pull
/// block. `cursor` points to the first sample of the next FIR window.
struct Input {
	left:   Box<[f32]>,
	right:  Box<[f32]>,
	pulled: Box<[f32]>,
	taps:   usize,
	cursor: usize,
}

impl Input {
	fn new(taps: usize) -> Self {
		Self {
			left: vec![0.0; taps - 1 + MAX_PULL_FRAMES].into_boxed_slice(),
			right: vec![0.0; taps - 1 + MAX_PULL_FRAMES].into_boxed_slice(),
			pulled: vec![0.0; MAX_PULL_FRAMES * 2].into_boxed_slice(),
			taps,
			cursor: MAX_PULL_FRAMES,
		}
	}

	#[cfg(test)]
	fn reset(&mut self) {
		// Only this suffix becomes history on the next refill; all other input
		// will be overwritten by pull. The interleaved scratch needs no clear.
		self.left.split_at_mut(MAX_PULL_FRAMES).1.fill(0.0);
		self.right.split_at_mut(MAX_PULL_FRAMES).1.fill(0.0);
		self.cursor = MAX_PULL_FRAMES;
	}

	fn window<P: FnMut(&mut [f32]) + ?Sized>(&mut self, pull: &mut P) -> (&[f32], &[f32]) {
		if self.cursor >= MAX_PULL_FRAMES {
			let history = self.taps - 1;
			self.left.copy_within(MAX_PULL_FRAMES..MAX_PULL_FRAMES + history, 0);
			self.right.copy_within(MAX_PULL_FRAMES..MAX_PULL_FRAMES + history, 0);
			pull(&mut self.pulled);
			let left = self.left.split_at_mut(history).1;
			let right = self.right.split_at_mut(history).1;
			for ((left, right), [input_left, input_right]) in left.iter_mut().zip(right).zip(self.pulled.as_chunks::<2>().0) {
				*left = *input_left;
				*right = *input_right;
			}
			self.cursor -= MAX_PULL_FRAMES;
		}
		let left = self.left.split_at(self.cursor).1.split_at(self.taps).0;
		let right = self.right.split_at(self.cursor).1.split_at(self.taps).0;
		(left, right)
	}
}

fn pull_decimated(stages: &mut [Input], coefficients: &[f32], out: &mut [f32], pull: &mut dyn FnMut(&mut [f32])) {
	let Some((last, preceding)) = stages.split_last_mut() else {
		pull(out);
		return;
	};
	let mut upstream = |input: &mut [f32]| pull_decimated(preceding, coefficients, input, pull);
	for frame in out.as_chunks_mut::<2>().0 {
		let (left, right) = last.window(&mut upstream);
		*frame = dot_stereo(left, right, coefficients);
		last.cursor += 2;
	}
}

/// Eight independent accumulators per channel let LLVM use packed f32 FMAs
/// over plain contiguous slices, without stereo shuffles or tap-loop bounds checks.
#[inline]
fn dot_stereo(left: &[f32], right: &[f32], coefficients: &[f32]) -> [f32; 2] {
	let mut left_sums = [0.0f32; 8];
	let mut right_sums = [0.0f32; 8];
	for ((left, right), coefficients) in left.as_chunks::<8>().0.iter().zip(right.as_chunks::<8>().0).zip(coefficients.as_chunks::<8>().0) {
		for (((left_sum, right_sum), (&left, &right)), &coefficient) in
			left_sums.iter_mut().zip(&mut right_sums).zip(left.iter().zip(right)).zip(coefficients)
		{
			*left_sum = coefficient.mul_add(left, *left_sum);
			*right_sum = coefficient.mul_add(right, *right_sum);
		}
	}
	let [l0, l1, l2, l3, l4, l5, l6, l7] = left_sums;
	let [r0, r1, r2, r3, r4, r5, r6, r7] = right_sums;
	[((l0 + l1) + (l2 + l3)) + ((l4 + l5) + (l6 + l7)), ((r0 + r1) + (r2 + r3)) + ((r4 + r5) + (r6 + r7))]
}

fn design(taps: usize, cutoff: f64, phases: usize, endpoint: bool) -> Box<[f32]> {
	let radius = taps as f64 * 0.5;
	let window_scale = 1.0 / bessel_i0(KAISER_BETA);
	let mut table = Vec::with_capacity((phases + usize::from(endpoint)) * taps);
	for phase in 0..phases + usize::from(endpoint) {
		let fraction = phase as f64 / phases as f64;
		let start = table.len();
		let mut sum = 0.0;
		for tap in 0..taps {
			let distance = tap as f64 - (radius - 1.0) - fraction;
			let coefficient = if distance.abs() >= radius {
				0.0
			} else {
				let argument = std::f64::consts::TAU * cutoff * distance;
				let sinc = if argument.abs() < f64::EPSILON { 2.0 * cutoff } else { argument.sin() / (std::f64::consts::PI * distance) };
				let relative = distance / radius;
				let window = bessel_i0(KAISER_BETA * (1.0 - relative * relative).sqrt()) * window_scale;
				sinc * window
			};
			table.push(super::dsp_f32(coefficient));
			sum += coefficient;
		}
		for coefficient in table.split_at_mut(start).1 {
			*coefficient = super::dsp_f32(f64::from(*coefficient) / sum);
		}
	}
	table.into_boxed_slice()
}

fn bessel_i0(x: f64) -> f64 {
	let mut sum = 1.0;
	let mut term = 1.0;
	for order in 1..=64 {
		let order = f64::from(order);
		term *= x * x / (4.0 * order * order);
		sum += term;
		if term <= sum * f64::EPSILON {
			break;
		}
	}
	sum
}

#[cfg(test)]
mod tests {

	use super::*;

	const WARMUP: usize = 1024;
	const INPUT_AMPLITUDE: [f64; 2] = [0.5, 0.25];

	fn noise(seed: &mut u32) -> f32 {
		*seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
		(*seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
	}

	fn render_noise(resampler: &mut Resampler, frames: usize, blocks: &[usize]) -> (Vec<f32>, usize) {
		let mut output = vec![0.0; frames * 2];
		let mut seed = 37;
		let mut pulls = 0;
		let mut pull = |input: &mut [f32]| {
			assert!(!input.is_empty() && input.len().is_multiple_of(2));
			assert!(input.len() <= MAX_PULL_FRAMES * 2);
			pulls += 1;
			for sample in input {
				*sample = noise(&mut seed);
			}
		};
		let mut start = 0;
		for block in blocks.iter().cycle() {
			if start == output.len() {
				break;
			}
			let end = (start + block * 2).min(output.len());
			resampler.process(&mut output[start..end], &mut pull);
			start = end;
		}
		(output, pulls)
	}

	#[expect(clippy::cast_possible_truncation, reason = "bounded test sine samples are rounded to f32")]
	fn render_tone(resampler: &mut Resampler, from_hz: u32, to_hz: u32, hz: f64) -> Vec<f32> {
		resampler.reset();
		let mut input_frame = 0u32;
		let omega = std::f64::consts::TAU * hz / f64::from(from_hz);
		let mut pull = |input: &mut [f32]| {
			assert!(input.len().is_multiple_of(2) && input.len() <= MAX_PULL_FRAMES * 2);
			for frame in input.as_chunks_mut::<2>().0 {
				let phase = omega * f64::from(input_frame);
				*frame = [(INPUT_AMPLITUDE[0] * phase.sin()) as f32, (INPUT_AMPLITUDE[1] * phase.cos()) as f32];
				input_frame += 1;
			}
		};
		let mut output = vec![0.0; (WARMUP + to_hz as usize) * 2];
		for block in output.chunks_mut(MAX_OUT_FRAMES * 2) {
			resampler.process(block, &mut pull);
		}
		output
	}

	struct Fit {
		amplitude: f64,
		phase:     f64,
		noise_db:  f64,
	}

	// Test windows contain whole periods, so sine/cosine/DC form an orthogonal
	// least-squares basis. The residual includes every harmonic and every spur,
	// not just a small selection of DFT bins.
	fn fit(output: &[f32], channel: usize, to_hz: u32, hz: f64) -> Fit {
		let omega = std::f64::consts::TAU * hz / f64::from(to_hz);
		let frames = output.len() / 2;
		let mut sine = 0.0;
		let mut cosine = 0.0;
		let mut dc = 0.0;
		for (index, frame) in output.as_chunks::<2>().0.iter().enumerate() {
			let value = f64::from(frame[channel]);
			sine += value * (omega * index as f64).sin();
			cosine += value * (omega * index as f64).cos();
			dc += value;
		}
		sine *= 2.0 / frames as f64;
		cosine *= 2.0 / frames as f64;
		dc /= frames as f64;
		let residual = output
			.as_chunks::<2>()
			.0
			.iter()
			.enumerate()
			.map(|(index, frame)| {
				let phase = omega * index as f64;
				(f64::from(frame[channel]) - sine * phase.sin() - cosine * phase.cos() - dc).powi(2)
			})
			.sum::<f64>();
		let amplitude = sine.hypot(cosine);
		Fit { amplitude, phase: cosine.atan2(sine), noise_db: 10.0 * (residual / (frames as f64 * amplitude * amplitude * 0.5)).log10() }
	}

	fn gain_db(output: &[f32], channel: usize) -> f64 {
		let energy = output.as_chunks::<2>().0.iter().map(|frame| f64::from(frame[channel]).powi(2)).sum::<f64>();
		10.0 * (energy / (output.len() as f64 * 0.25 * INPUT_AMPLITUDE[channel].powi(2))).log10()
	}

	#[test]
	fn identity_is_bit_exact_and_bounds_pulls() {
		let mut seed = 17u32;
		let mut input: Vec<f32> = (0..MAX_OUT_FRAMES * 2)
			.map(|_| {
				seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
				f32::from_bits(seed)
			})
			.collect();
		input[..6].copy_from_slice(&[0.0, -0.0, f32::INFINITY, f32::NEG_INFINITY, f32::from_bits(1), f32::from_bits(0x7fc0_1234)]);
		for hz in [1, 44_100, 48_000, 192_000] {
			let mut resampler = Resampler::new(hz, hz);
			assert!(resampler.is_identity());
			let mut position = 0;
			let mut output = vec![0.0; input.len()];
			resampler.process(&mut output, &mut |chunk| {
				assert!(chunk.len().is_multiple_of(2) && chunk.len() <= MAX_PULL_FRAMES * 2);
				chunk.copy_from_slice(&input[position..position + chunk.len()]);
				position += chunk.len();
			});
			assert_eq!(position, input.len());
			assert!(output.iter().zip(&input).all(|(got, want)| got.to_bits() == want.to_bits()));
		}
	}

	#[test]
	fn one_kilohertz_has_correct_frequency_gain_and_noise() {
		let mut resampler = Resampler::new(44_100, 48_000);
		assert!(!resampler.is_identity());
		let output = render_tone(&mut resampler, 44_100, 48_000, 1000.0);
		let steady = &output[WARMUP * 2..];
		for (channel, amplitude) in INPUT_AMPLITUDE.iter().enumerate() {
			let fitted = fit(steady, channel, 48_000, 1000.0);
			let gain = 20.0 * (fitted.amplitude / amplitude).log10();
			assert!(gain.abs() < 0.1, "channel {channel}: {gain:.6} dB");
			assert!(fitted.noise_db <= -70.0, "channel {channel}: THD+N {:.2} dB", fitted.noise_db);
			let first = fit(&steady[..48_000], channel, 48_000, 1000.0);
			let second = fit(&steady[48_000..], channel, 48_000, 1000.0);
			let phase_delta = (second.phase - first.phase + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI;
			let measured_hz = 1000.0 + phase_delta / (std::f64::consts::TAU * 0.5);
			assert!((measured_hz - 1000.0).abs() < 0.001, "{measured_hz:.6} Hz");
			println!("resample 1 kHz channel {channel}: {measured_hz:.6} Hz, gain {gain:.6} dB, THD+N {:.2} dB", fitted.noise_db);
		}
	}

	#[test]
	fn passband_is_flat_through_eighteen_kilohertz_and_images_are_rejected() {
		let mut resampler = Resampler::new(44_100, 48_000);
		let mut worst_gain = 0.0f64;
		let mut worst_noise = f64::NEG_INFINITY;
		for hz in [20.0, 1000.0, 5000.0, 10_000.0, 16_000.0, 17_000.0, 18_000.0] {
			let output = render_tone(&mut resampler, 44_100, 48_000, hz);
			for (channel, amplitude) in INPUT_AMPLITUDE.iter().enumerate() {
				let fitted = fit(&output[WARMUP * 2..], channel, 48_000, hz);
				let gain = 20.0 * (fitted.amplitude / amplitude).log10();
				worst_gain = worst_gain.max(gain.abs());
				worst_noise = worst_noise.max(fitted.noise_db);
				assert!(gain.abs() < 0.1, "{hz} Hz: {gain:.6} dB");
				assert!(fitted.noise_db <= -70.0, "{hz} Hz: residual/images {:.2} dB", fitted.noise_db);
			}
		}
		println!("resample 44.1->48 kHz: worst passband {worst_gain:.6} dB, worst residual/images {worst_noise:.2} dB");
	}

	#[test]
	fn downsampling_suppresses_twenty_three_kilohertz_and_the_stopband() {
		let mut resampler = Resampler::new(48_000, 44_100);
		let mut worst = f64::NEG_INFINITY;
		for hz in [22_050.0, 22_250.0, 22_500.0, 23_000.0, 23_500.0, 24_000.0] {
			let output = render_tone(&mut resampler, 48_000, 44_100, hz);
			for channel in 0..2 {
				let gain = gain_db(&output[WARMUP * 2..], channel);
				worst = worst.max(gain);
				assert!(gain <= -70.0, "{hz} Hz channel {channel}: {gain:.2} dB");
				if hz == 23_000.0 {
					println!("resample 48->44.1 kHz, 23 kHz channel {channel}: {gain:.2} dB");
				}
			}
		}
		println!("resample 48->44.1 kHz: worst measured stopband {worst:.2} dB");
	}

	#[test]
	fn common_hal_rates_coprime_phases_and_staged_decimation_preserve_tones() {
		for (from, to) in [
			(44_100, 88_200),
			(44_100, 96_000),
			(44_100, 176_400),
			(44_100, 192_000),
			(44_100, 47_999),
			(192_000, 44_100),
			(192_000, 48_000),
		] {
			let mut resampler = Resampler::new(from, to);
			for hz in [1000.0, 18_000.0] {
				let output = render_tone(&mut resampler, from, to, hz);
				for (channel, amplitude) in INPUT_AMPLITUDE.iter().enumerate() {
					let fitted = fit(&output[WARMUP * 2..], channel, to, hz);
					let gain = 20.0 * (fitted.amplitude / amplitude).log10();
					assert!(gain.abs() < 0.1, "{from}->{to}, {hz} Hz: {gain:.6} dB");
					assert!(fitted.noise_db <= -70.0, "{from}->{to}, {hz} Hz: {:.2} dB", fitted.noise_db);
				}
			}
		}
	}

	#[test]
	fn staged_decimation_does_not_fold_high_frequencies_into_the_passband() {
		let mut resampler = Resampler::new(192_000, 44_100);
		for hz in [23_000.0, 44_000.0, 76_000.0, 95_000.0] {
			let output = render_tone(&mut resampler, 192_000, 44_100, hz);
			for channel in 0..2 {
				let gain = gain_db(&output[WARMUP * 2..], channel);
				assert!(gain <= -70.0, "staged 192->44.1 kHz, {hz} Hz: {gain:.2} dB");
			}
		}
	}

	#[test]
	fn arbitrary_blocks_are_sample_exact() {
		for (from, to) in [(44_100, 48_000), (48_000, 44_100), (192_000, 44_100), (44_100, 47_999)] {
			let mut resampler = Resampler::new(from, to);
			let (expected, pulls) = render_noise(&mut resampler, MAX_OUT_FRAMES, &[MAX_OUT_FRAMES]);
			for blocks in [&[1][..], &[7], &[333], &[1, 7, 333, MAX_OUT_FRAMES]] {
				resampler.reset();
				let (actual, actual_pulls) = render_noise(&mut resampler, MAX_OUT_FRAMES, blocks);
				assert_eq!(actual, expected, "{from}->{to}, blocks {blocks:?}");
				assert_eq!(actual_pulls, pulls);
			}
			resampler.reset();
			let (expected, pulls) = render_noise(&mut resampler, MAX_OUT_FRAMES * 3 + 777, &[MAX_OUT_FRAMES]);
			resampler.reset();
			let (actual, actual_pulls) = render_noise(&mut resampler, MAX_OUT_FRAMES * 3 + 777, &[1, 7, 333, MAX_OUT_FRAMES]);
			assert_eq!(actual, expected);
			assert_eq!(actual_pulls, pulls);
		}
	}

	#[test]
	fn reset_discards_history_clock_and_unconsumed_input() {
		for (from, to) in [(44_100, 48_000), (48_000, 44_100), (192_000, 44_100), (44_100, 47_999)] {
			let mut used = Resampler::new(from, to);
			let _ = render_noise(&mut used, 1739, &[7, 333]);
			used.reset();
			let mut fresh = Resampler::new(from, to);
			assert_eq!(render_noise(&mut used, MAX_OUT_FRAMES, &[333]), render_noise(&mut fresh, MAX_OUT_FRAMES, &[333]));
			let mut silent = [1.0; 777 * 2];
			used.reset();
			used.process(&mut silent, &mut |input| input.fill(0.0));
			assert!(silent.iter().all(|sample| *sample == 0.0));
		}
	}

	#[test]
	fn coefficient_memory_is_bounded_even_for_extreme_and_coprime_rates() {
		for (from, to) in [
			(44_100, 48_000),
			(44_100, 88_200),
			(44_100, 96_000),
			(44_100, 176_400),
			(44_100, 192_000),
			(191_999, 192_000),
			(192_000, 191_999),
			(192_000, 44_100),
			(192_000, 1),
			(1, 192_000),
			(123_457, 190_081),
		] {
			let resampler = Resampler::new(from, to);
			let filter = resampler.filter.as_ref().unwrap();
			let bytes = (filter.coefficients.len() + resampler.decimation_coefficients.len()) * size_of::<f32>();
			assert!(bytes < 256 * 1024, "{from}->{to}: {bytes} bytes");
			assert!((BASE_TAPS..=DECIMATION_TAPS).contains(&filter.taps));
			if (from, to) == (44_100, 48_000) {
				assert_eq!((filter.numerator, filter.remainder), (160, 147));
				assert_eq!(filter.taps, BASE_TAPS);
				assert_eq!(filter.phases, 160);
				assert!(!filter.interpolated);
				println!("resample 44.1->48 kHz: {} taps/phase, {} coefficient bytes", filter.taps, bytes);
			}
		}
	}

	#[test]
	fn rational_clock_does_not_drift() {
		let mut resampler = Resampler::new(44_100, 48_000);
		let frames = MAX_OUT_FRAMES * 3 + 777;
		let (_, pulls) = render_noise(&mut resampler, frames, &[333]);
		let filter = resampler.filter.as_ref().unwrap();
		assert_eq!(filter.phase, (frames as u64 * 147) % 160);
		assert_eq!((pulls - 1) * MAX_PULL_FRAMES + filter.input.cursor, frames * 147 / 160);
	}

	#[test]
	fn empty_output_does_not_pull_or_change_state() {
		for to in [44_100, 48_000, 47_999, 8000] {
			let mut resampler = Resampler::new(44_100, to);
			resampler.process(&mut [], &mut |_| panic!("empty output must not pull"));
			let mut fresh = Resampler::new(44_100, to);
			assert_eq!(render_noise(&mut resampler, 333, &[333]), render_noise(&mut fresh, 333, &[333]));
		}
	}
}
