//! Loudness-meter, leveler and true-peak limiter contracts.

use super::{testing::*, *};

fn run(leveler: &mut Leveler, buf: &mut [f32]) {
	// In callbacks of an awkward size, as CoreAudio delivers them.
	for chunk in buf.chunks_mut(2 * 470) {
		leveler.process(chunk);
	}
}

fn db(x: f64) -> f64 {
	20.0 * x.log10()
}

#[test]
fn the_meter_reads_a_997_hz_sine_at_its_level() {
	// BS.1770's calibration: a full-scale 997 Hz sine in both channels is 0 LUFS.
	for dbfs in [0.0, -10.0, -23.0] {
		let buf = sine(997.0, 5.0, 10f64.powf(dbfs / 20.0), 0.0);
		let lufs = loudness(&buf);
		assert!((lufs - dbfs).abs() < 0.1, "{dbfs} dBFS read {lufs} LUFS");
	}
}

#[test]
fn the_meter_weights_high_frequencies_up_and_low_ones_down() {
	let level = |hz| loudness(&sine(hz, 4.0, 0.1, 0.0));
	let mid = level(997.0);
	assert!(level(10_000.0) - mid > 3.0, "the head's shelf lifts the highs");
	assert!(mid - level(30.0) > 3.0, "the high-pass cuts the lows");
}

#[test]
fn the_kernels_are_unity_at_dc_and_interpolate_a_sine() {
	for phase in [0.25, 0.5, 0.75] {
		let k = kernel(phase);
		assert!((k.iter().sum::<f32>() - 1.0).abs() < 1e-6);
	}
	// A 5 kHz sine sampled at 44.1 kHz: the interpolated point lies on the sine.
	let (hz, phase) = (5_000.0, 0.7);
	let at = |t: f64| (2.0 * PI * hz * t / FS + phase).sin();
	let history: Vec<f64> = (0..TAPS).map(|i| at(i as f64)).collect();
	for p in [0.25, 0.5, 0.75] {
		let y: f64 = kernel(p).iter().zip(&history).map(|(k, h)| f64::from(*k) * h).sum();
		let want = at(CENTER as f64 + p);
		assert!((y - want).abs() < 0.01, "phase {p}: {y} vs {want}");
	}
}

#[test]
fn a_quiet_steady_signal_is_raised_to_the_target_and_a_loud_one_lowered() {
	// Pink noise reads about 2.7 LUFS above its RMS in dBFS: these need +7.7, -6.2, -1.7 and +6.3 dB.
	for (rms, target) in [(0.06, -14.0), (0.3, -14.0), (0.1, -19.0), (0.1, -11.0)] {
		let mut leveler = Leveler::new();
		leveler.set_target(target);
		let mut buf = pink(30.0, rms, 3);
		run(&mut leveler, &mut buf);
		let tail = &buf[buf.len() - 2 * 44_100 * 5..];
		let out = loudness(tail);
		assert!((out - target).abs() < 1.0, "rms {rms} -> target {target}: out {out} LUFS (gain {})", leveler.gain_db());
	}
}

#[test]
fn a_boost_stops_at_its_cap_and_a_cut_at_its_own() {
	// 14 dB below the target: the cap (less the dead zone, where it stops) is as far as it goes.
	let mut leveler = Leveler::new();
	let mut quiet = pink(30.0, 0.03, 3);
	run(&mut leveler, &mut quiet);
	assert!((leveler.gain_db() - (MAX_BOOST - DEAD_ZONE)).abs() < 0.3, "boost {}", leveler.gain_db());
	// Far above a quiet target: the cut stops at its cap.
	let mut leveler = Leveler::new();
	leveler.set_target(-30.0);
	let mut loud = noise(30.0, 0.95, 4, true);
	run(&mut leveler, &mut loud);
	assert!((leveler.gain_db() + (MAX_CUT - DEAD_ZONE)).abs() < 0.3, "cut {}", leveler.gain_db());
}

#[test]
fn the_gain_does_not_pump_with_the_music() {
	// A beat at 2 Hz that swings the level by 6 dB, and a slow swell: the gain must not follow.
	let mut leveler = Leveler::new();
	let mut buf = pink(60.0, 0.05, 11);
	for (i, frame) in buf.as_chunks_mut::<2>().0.iter_mut().enumerate() {
		let t = i as f64 / FS;
		let beat = 10f64.powf(3.0 * (2.0 * PI * 2.0 * t).sin() / 20.0);
		let swell = 10f64.powf(1.5 * (2.0 * PI * 0.1 * t).sin() / 20.0);
		let g = pcm(beat * swell);
		frame[0] *= g;
		frame[1] *= g;
	}
	let mut gains = Vec::new();
	for chunk in buf.chunks_mut(2 * 4410) {
		leveler.process(chunk);
		gains.push(leveler.gain_db());
	}
	// After convergence (20 s) the gain's whole movement stays small.
	let settled = &gains[200..];
	let (lo, hi) = settled.iter().fold((f64::MAX, f64::MIN), |(lo, hi), g| (lo.min(*g), hi.max(*g)));
	assert!(hi - lo < 1.0, "gain moved {:.2} dB peak to peak while the music swung 6 dB", hi - lo);
	// And never by more than a dB between two 100 ms looks: no following of the beat.
	let step = settled.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f64::max);
	assert!(step < 0.3, "a step of {step} dB in 0.1 s");
}

#[test]
fn reductions_are_slow_outside_a_track_start() {
	// Settled on a quiet signal, a loud one arrives with no new-track hint: the gain falls by
	// no more than its cap per second.
	let mut leveler = Leveler::new();
	let mut quiet = pink(20.0, 0.02, 5);
	run(&mut leveler, &mut quiet);
	let before = leveler.gain_db();
	let mut loud = pink(10.0, 0.3, 6);
	let mut worst = 0f64;
	let mut last = before;
	for chunk in loud.chunks_mut(2 * 4410) {
		leveler.process(chunk);
		worst = worst.max(last - leveler.gain_db());
		last = leveler.gain_db();
	}
	// 0.1 s at the cap of 6 dB/s (plus a little for the block that straddles).
	assert!(worst <= 0.65, "fell {worst} dB in 0.1 s");
	assert!(leveler.gain_db() < before - 5.0, "but it does come down");
}

#[test]
fn boosts_are_slower_than_cuts() {
	let mut leveler = Leveler::new();
	let mut loud = pink(20.0, 0.3, 8);
	run(&mut leveler, &mut loud);
	let before = leveler.gain_db();
	let mut quiet = pink(10.0, 0.02, 9);
	let mut worst = 0f64;
	let mut last = before;
	for chunk in quiet.chunks_mut(2 * 4410) {
		leveler.process(chunk);
		worst = worst.max(leveler.gain_db() - last);
		last = leveler.gain_db();
	}
	assert!(worst <= 0.2, "rose {worst} dB in 0.1 s (cap 1.5 dB/s)");
	assert!(leveler.gain_db() > before + 3.0, "but it does come up");
}

#[test]
fn a_new_track_settles_within_a_few_seconds() {
	let mut leveler = Leveler::new();
	let mut first = pink(30.0, 0.05, 21);
	run(&mut leveler, &mut first);
	// Track two is 14 dB louder; the player says so.
	leveler.new_program();
	let mut second = pink(8.0, 0.25, 22);
	run(&mut leveler, &mut second);
	let tail = &second[second.len() - 2 * 44_100 * 2..];
	let out = loudness(tail);
	assert!((out - -14.0).abs() < 2.0, "6 s into the new track: {out} LUFS");
}

#[test]
fn silence_holds_the_gain() {
	let mut leveler = Leveler::new();
	let mut buf = pink(25.0, 0.06, 1);
	run(&mut leveler, &mut buf);
	let settled = leveler.gain_db();
	assert!(settled > 3.0);
	let mut silence = vec![0.0; 2 * 44_100 * 60];
	run(&mut leveler, &mut silence);
	// The first tenths of a second of fading out are still measured; after that, nothing.
	assert!((leveler.gain_db() - settled).abs() < 0.05, "drifted to {} from {settled}", leveler.gain_db());
	let held = leveler.gain_db();
	let mut more = vec![0.0; 2 * 44_100 * 60];
	run(&mut leveler, &mut more);
	assert_eq!(leveler.gain_db(), held, "a minute of silence moved it");
	// Resuming the same material keeps the level.
	let mut again = pink(10.0, 0.06, 2);
	run(&mut leveler, &mut again);
	assert!((leveler.gain_db() - settled).abs() < 1.0);
}

#[test]
fn turning_the_volume_down_is_not_undone() {
	// 30 dB down on the player's side, told to the leveler: the gain must stay put, and the
	// output must be 30 dB lower than at full volume.
	let mut full = Leveler::new();
	let mut buf = pink(40.0, 0.1, 4);
	run(&mut full, &mut buf);
	let g_full = full.gain_db();
	let out_full = loudness(&buf[buf.len() - 2 * 44_100 * 5..]);

	let mut quiet = Leveler::new();
	quiet.set_volume_db(-30.0);
	let mut buf2: Vec<f32> = pink(40.0, 0.1, 4).iter().map(|s| s * 10f32.powf(-30.0 / 20.0)).collect();
	run(&mut quiet, &mut buf2);
	let out_quiet = loudness(&buf2[buf2.len() - 2 * 44_100 * 5..]);
	assert!((quiet.gain_db() - g_full).abs() < 1.0, "gain {} vs {}", quiet.gain_db(), g_full);
	assert!((out_full - out_quiet - 30.0).abs() < 1.5, "{out_full} vs {out_quiet}");
}

#[test]
fn a_volume_change_mid_track_does_not_move_the_gain() {
	let mut leveler = Leveler::new();
	let mut buf = pink(30.0, 0.1, 4);
	run(&mut leveler, &mut buf);
	let settled = leveler.gain_db();
	// The player's volume drops 20 dB: told first, then the audio follows 0.2 s later.
	leveler.set_volume_db(-20.0);
	let mut gap = pink(0.2, 0.1, 5);
	run(&mut leveler, &mut gap);
	let mut after: Vec<f32> = pink(15.0, 0.1, 6).iter().map(|s| s * 0.1).collect();
	run(&mut leveler, &mut after);
	assert!((leveler.gain_db() - settled).abs() < 1.0, "{} vs {settled}", leveler.gain_db());
}

#[test]
fn a_boost_never_leaves_the_limiter_much_to_do() {
	// Quiet on average but with peaks near full scale every quarter second: the boost stops
	// short of them (the loudest recent peak plus the gain stays within the allowance).
	let mut leveler = Leveler::new();
	let mut buf = pink(30.0, 0.04, 2);
	for frame in buf.as_chunks_mut::<2>().0.iter_mut().step_by(11_025) {
		frame[0] = 0.9;
		frame[1] = 0.9;
	}
	run(&mut leveler, &mut buf);
	assert!(leveler.gain_db() < 3.9, "boosted {} dB into peaks of -0.9 dBFS", leveler.gain_db());
	assert!(leveler.gain_db() > 1.0, "but it did level the track up: {}", leveler.gain_db());
}

#[test]
fn the_limiter_is_transparent_below_the_ceiling() {
	let mut limiter = Limiter::new();
	let input = sine(1_000.0, 1.0, 0.5, 0.0);
	let mut out = input.clone();
	limiter.process(&mut out);
	for (i, (&o, &x)) in out[2 * DELAY_FRAMES..].iter().zip(&input).enumerate() {
		assert!((o - x).abs() < 1e-6, "sample {i}: {o} vs {x}");
	}
}

#[test]
fn the_limiter_keeps_the_true_peak_under_the_ceiling() {
	// Content far above the ceiling, and content whose inter-sample peaks do the damage:
	// a sine at a quarter of the sample rate, phased so every sample is 3 dB under its peak.
	let cases: Vec<(&str, Vec<f32>)> = vec![
		("hot pink noise", pink(1.0, 2.0, 1)),
		("fs/4 sine, samples at 0 dBFS", sine(FS / 4.0, 0.5, 2f64.sqrt(), PI / 4.0)),
		("1 kHz sine +12 dB", sine(1_000.0, 0.5, 4.0, 0.0)),
		("11 kHz sine +6 dB", sine(11_000.0, 0.5, 2.0, 0.3)),
		("step to full scale", {
			let mut v = vec![0.0; 2 * 11_025];
			v.extend(sine(60.0, 0.3, 3.0, 0.0));
			v
		}),
	];
	for (name, input) in cases {
		let mut limiter = Limiter::new();
		let mut out = input.clone();
		limiter.process(&mut out);
		assert!(out.iter().all(|s| s.is_finite()));
		// The samples themselves never pass the ceiling, exactly.
		let sample_peak = out.iter().fold(0f32, |m, s| m.max(s.abs()));
		assert!(db(f64::from(sample_peak)) <= f64::from(CEILING_DB) + 0.01, "{name}: sample peak {:.3} dBFS", db(f64::from(sample_peak)));
		// And so does what a converter will make of them (measured with a better interpolator).
		let peak = true_peak_db(&out[2 * 2000..]);
		assert!(peak <= f64::from(CEILING_DB) + 0.3, "{name}: true peak {peak:.2} dBFS");
		// It is not squashed flat: where the input was loud, the output sits at the ceiling.
		assert!(
			db(f64::from(sample_peak)) > -8.0 || name.starts_with("fs/4"),
			"{name}: over-limited to {:.1} dB",
			db(f64::from(sample_peak))
		);
	}
}

#[test]
fn white_noise_is_limited_at_the_sample_level_and_only_roughly_between_samples() {
	// Noise fills the band up to Nyquist, where any short interpolator under-reads (the
	// standard's own 48-tap one does too): its samples are held exactly, its true peak to a few dB.
	let mut limiter = Limiter::new();
	let mut out = noise(0.5, 4.0, 1, true);
	limiter.process(&mut out);
	let sample_peak = out.iter().fold(0f32, |m, s| m.max(s.abs()));
	assert!(db(f64::from(sample_peak)) <= f64::from(CEILING_DB) + 0.01);
	assert!(true_peak_db(&out[2 * 2000..]) < f64::from(CEILING_DB) + 6.0);
}

#[test]
fn the_limiter_recovers_after_a_peak() {
	let mut limiter = Limiter::new();
	let mut buf = sine(1_000.0, 0.1, 0.3, 0.0);
	buf.extend(sine(1_000.0, 0.1, 3.0, 0.0));
	buf.extend(sine(1_000.0, 1.5, 0.3, 0.0));
	limiter.process(&mut buf);
	assert!(limiter.reduction_db() > -0.1, "{} dB still taken off", limiter.reduction_db());
	let tail = &buf[buf.len() - 2 * 4410..];
	let peak = tail.iter().fold(0f32, |m, s| m.max(s.abs()));
	assert!((peak - 0.3).abs() < 0.01, "a quiet tone comes out at {peak}");
}

#[test]
fn the_limiter_cannot_be_poisoned_by_a_bad_sample() {
	let mut limiter = Limiter::new();
	let mut buf = sine(1_000.0, 0.2, 0.5, 0.0);
	buf[1000] = f32::NAN;
	buf[1001] = f32::INFINITY;
	limiter.process(&mut buf);
	assert!(buf.iter().all(|s| s.is_finite()));
	assert!(limiter.reduction_db() > -0.1);
}
