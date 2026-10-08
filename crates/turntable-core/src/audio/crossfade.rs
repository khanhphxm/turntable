//! Sample-counted equal-power overlap, before the ring, EQ and output stage.
//!
//! Both inputs are 44.1 kHz interleaved stereo. The decoder thread owns this
//! mixer; the real-time output callback still has only one PCM stream. An
//! oscillator advances the sine/cosine envelopes without per-frame trig, heap
//! work or locks. The first frame is entirely outgoing, the last entirely
//! incoming. Cancelling a transition makes the next packet pass untouched.

/// Equal-power envelopes for one natural track transition.
#[derive(Default)]
pub(crate) struct Crossfade {
	frames:      usize,
	position:    usize,
	sine:        f64,
	cosine:      f64,
	step_sine:   f64,
	step_cosine: f64,
}

impl Crossfade {
	/// Starts an overlap of `frames` stereo frames. Zero means gapless, not a fade.
	pub(crate) fn start(&mut self, frames: usize) {
		self.frames = frames;
		self.position = 0;
		self.sine = 0.0;
		self.cosine = 1.0;
		(self.step_sine, self.step_cosine) = (std::f64::consts::FRAC_PI_2 / frames.saturating_sub(1).max(1) as f64).sin_cos();
	}

	pub(crate) fn remaining(&self) -> usize {
		self.frames.saturating_sub(self.position)
	}

	pub(crate) fn cancel(&mut self) {
		self.frames = 0;
		self.position = 0;
	}

	/// Mixes at most the remaining overlap. Missing outgoing frames are silence;
	/// incoming samples beyond the boundary (or an incomplete frame) are untouched.
	#[expect(clippy::cast_possible_truncation, reason = "unit gain envelopes intentionally use f32 PCM")]
	pub(crate) fn mix(&mut self, incoming: &mut [f32], outgoing: &[f32]) {
		let frames = incoming.len() / 2;
		let count = frames.min(self.remaining());
		let old = outgoing.as_chunks::<2>().0.iter().copied().chain(std::iter::repeat([0.0; 2]));
		for (new, old) in incoming.as_chunks_mut::<2>().0.iter_mut().take(count).zip(old) {
			let (out_gain, in_gain) = if self.position + 1 == self.frames { (0.0, 1.0) } else { (self.cosine as f32, self.sine as f32) };
			for (new, old) in new.iter_mut().zip(old) {
				*new = old.mul_add(out_gain, *new * in_gain);
			}
			let sine = self.sine * self.step_cosine + self.cosine * self.step_sine;
			self.cosine = self.cosine * self.step_cosine - self.sine * self.step_sine;
			self.sine = sine;
			self.position += 1;
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn curves_have_exact_endpoints_equal_power_and_linked_stereo() {
		let frames = 529_200; // the full twelve-second setting
		let mut mixer = Crossfade::default();
		mixer.start(frames);
		// Isolate outgoing on the left and incoming on the right.
		let mut incoming: Vec<f32> = (0..frames).flat_map(|_| [0.0, 1.0]).collect();
		let outgoing: Vec<f32> = (0..frames).flat_map(|_| [1.0, 0.0]).collect();
		mixer.mix(&mut incoming, &outgoing);
		assert_eq!(&incoming[..2], &[1.0, 0.0]);
		assert_eq!(&incoming[incoming.len() - 2..], &[0.0, 1.0]);
		for pair in incoming.as_chunks::<2>().0.windows(2) {
			let [left, right] = pair[0];
			assert!((left * left + right * right - 1.0).abs() < 2e-6);
			assert!(left >= pair[1][0] && right <= pair[1][1]);
		}
		assert_eq!(mixer.remaining(), 0);
	}

	#[test]
	fn packet_boundaries_do_not_change_the_curve_or_touch_the_next_audio() {
		let mut one = Crossfade::default();
		let mut split = Crossfade::default();
		one.start(101);
		split.start(101);
		let mut whole = vec![0.25; 256];
		let mut chunks = whole.clone();
		one.mix(&mut whole, &[0.5; 256]);
		for chunk in chunks.chunks_mut(14) {
			split.mix(chunk, &[0.5; 14]);
		}
		assert_eq!(whole, chunks);
		assert!(whole[202..].iter().all(|s| *s == 0.25));
		assert_eq!(whole[200], 0.25);
	}

	#[test]
	fn off_single_frame_and_short_sources_have_defined_boundaries() {
		let mut mixer = Crossfade::default();
		let mut input = [0.2, -0.2, 0.4];
		mixer.start(0);
		mixer.mix(&mut input, &[1.0; 2]);
		assert_eq!(input, [0.2, -0.2, 0.4]);
		mixer.start(1);
		mixer.mix(&mut input, &[1.0; 2]);
		assert_eq!(input, [0.2, -0.2, 0.4]);
		mixer.start(3);
		let mut input = [1.0; 6];
		mixer.mix(&mut input, &[0.5; 2]);
		assert_eq!(&input[..2], &[0.5; 2]);
		assert!((input[2] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
		assert_eq!(&input[4..], &[1.0; 2]);
	}

	#[test]
	fn a_user_skip_cancels_both_envelopes_immediately() {
		let mut mixer = Crossfade::default();
		mixer.start(441_000);
		mixer.mix(&mut [1.0; 512], &[0.5; 512]);
		assert!(mixer.remaining() > 0);
		mixer.cancel();
		let mut next = [0.75; 512];
		mixer.mix(&mut next, &[0.5; 512]);
		assert_eq!(next, [0.75; 512]);
		assert_eq!(mixer.remaining(), 0);
		mixer.start(3);
		mixer.mix(&mut next, &[0.5; 512]);
		assert_eq!(next[0], 0.5, "a later transition starts from its own outgoing song");
		assert_eq!(next[4], 0.75);
	}
}
