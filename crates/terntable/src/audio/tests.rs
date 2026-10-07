//! Audio pipeline contracts; hardware playback is outside the unit suite.

use super::*;

fn shared() -> Arc<Shared> {
	Arc::new(Shared::new())
}

#[test]
fn long_silence_asks_for_the_unit_to_stop_and_audio_resets_it() {
	let sh = shared();
	let mut r = Render::new(sh.clone());
	let mut buf = [0f32; 1024]; // 512 frames per callback
	let callbacks = IDLE_STOP_FRAMES / 512;
	for _ in 0..callbacks - 1 {
		r.render(&mut buf);
	}
	assert!(!sh.want_stop.load(Ordering::Acquire), "not before 1.5 s of silence");
	r.render(&mut buf);
	r.render(&mut buf);
	assert!(sh.want_stop.load(Ordering::Acquire));
	// Audio arriving restarts the silence count.
	sh.ring.push(&[0.5; 4096]);
	r.render(&mut buf);
	assert_eq!(r.silent_frames, 0);
}

#[test]
fn writer_queues_in_order_and_flags_the_end_when_dropped() {
	let (writer, feed) = channel();
	let want: Vec<f32> = (0..2000).map(|i| i as f32 * 0.25).collect();
	for chunk in want.chunks(14) {
		assert!(writer.write(chunk));
	}
	assert!(!feed.0.ended.load(Ordering::Acquire));
	drop(writer);
	assert!(feed.0.ended.load(Ordering::Acquire));
	let mut got = vec![0.0; 2000];
	assert_eq!(feed.0.ring.pop(&mut got), 2000);
	assert_eq!(got, want);
}

#[test]
fn writer_blocks_at_the_fill_level_and_gives_up_when_stopped() {
	let (writer, feed) = channel();
	let h = thread::spawn(move || writer.write(&vec![0.0; 100_000]));
	thread::sleep(Duration::from_millis(100));
	assert!(feed.0.ring.len() <= FILL_SAMPLES, "never overfills");
	assert!(!h.is_finished(), "waits for room");
	feed.0.stop.store(true, Ordering::Release);
	if let Some(t) = feed.0.writer.get() {
		t.unpark();
	}
	assert!(!h.join().unwrap(), "a stopped player takes no more");
}

#[test]
fn render_buffers_then_plays_then_counts_one_underrun_per_dry_spell() {
	let sh = shared();
	let mut r = Render::new(sh.clone());
	let mut out = vec![1.0; 512];

	// Nothing yet: silence, and start-up buffering is not an underrun.
	assert!(r.render(&mut out));
	assert!(out.iter().all(|&s| s == 0.0));
	assert_eq!(sh.underruns.load(Ordering::Relaxed), 0);

	// A few samples are not enough to start.
	sh.ring.push(&[0.5; 100]);
	assert!(r.render(&mut out));
	assert_eq!(sh.ring.len(), 100);

	// Enough: plays in order.
	let ramp: Vec<f32> = (0..REBUFFER_SAMPLES).map(|i| i as f32 / 8192.0).collect();
	sh.ring.push(&ramp);
	assert!(!r.render(&mut out));
	assert_eq!(&out[..100], &[0.5; 100][..]);
	assert_eq!(&out[100..105], &ramp[..5]);

	// Drain the rest; running dry counts once, further empty callbacks do not.
	for _ in 0..10 {
		r.render(&mut out);
	}
	assert_eq!(sh.underruns.load(Ordering::Relaxed), 1);

	// Resume after a refill, run dry again: second underrun.
	sh.ring.push(&[0.1; REBUFFER_SAMPLES]);
	r.render(&mut out);
	for _ in 0..20 {
		r.render(&mut out);
	}
	assert_eq!(sh.underruns.load(Ordering::Relaxed), 2);
}

#[test]
fn render_after_eof_drains_then_is_silent_without_underrun() {
	let sh = shared();
	let mut r = Render::new(sh.clone());
	sh.ring.push(&[0.25; 600]);
	sh.ended.store(true, Ordering::Release);
	let mut out = vec![9.0; 1000];
	assert!(!r.render(&mut out));
	assert!(out[..600].iter().all(|&s| s == 0.25));
	assert!(out[600..].iter().all(|&s| s == 0.0));
	assert!(r.render(&mut out));
	assert_eq!(sh.underruns.load(Ordering::Relaxed), 0);
}

#[test]
fn render_applies_eq_published_through_the_mailbox() {
	let sh = shared();
	let mut r = Render::new(sh.clone());
	let tone: Vec<f32> = (0..4410 * 2).map(|i| 0.1 * (2.0 * std::f32::consts::PI * 1000.0 * (i / 2) as f32 / 44_100.0).sin()).collect();
	let mut gains = [0.0; 10];
	gains[5] = 12.0;
	sh.mailbox.publish(&EqSettings { enabled: true, preamp_db: 0.0, gains_db: gains });
	sh.ring.push(&tone);
	let mut out = vec![0.0; tone.len()];
	r.render(&mut out);
	let peak = |b: &[f32]| b.iter().fold(0.0f32, |m, s| m.max(s.abs()));
	// Ramp finished long before the tail: ~ +12 dB (x3.98).
	let ratio = peak(&out[out.len() / 2..]) / peak(&tone[tone.len() / 2..]);
	assert!((ratio - 3.98).abs() < 0.2, "ratio {ratio}");
}

#[test]
fn render_applies_the_stage_settings_the_player_published() {
	let sh = shared();
	let mut r = Render::new(sh.clone());
	// Left only, at 0.4: mono makes it 0.2 in both, and balance all the way right silences the left.
	let input: Vec<f32> = (0..4000).flat_map(|_| [0.4, 0.0]).collect();
	let st = &sh.stage;
	st.mono.store(true, Ordering::Relaxed);
	st.balance.store(1.0f32.to_bits(), Ordering::Relaxed);
	sh.ring.push(&input);
	let mut out = vec![0.0; input.len()];
	r.render(&mut out);
	let last = out.as_chunks::<2>().0.last().unwrap();
	assert!(last[0].abs() < 1e-6, "left {}", last[0]);
	assert!((last[1] - 0.2).abs() < 1e-6, "right {}", last[1]);
	// Back to off: the next audio passes untouched.
	st.mono.store(false, Ordering::Relaxed);
	st.balance.store(0.0f32.to_bits(), Ordering::Relaxed);
	for _ in 0..4 {
		sh.ring.push(&input);
		r.render(&mut out);
	}
	let last = out.as_chunks::<2>().0.last().unwrap();
	assert_eq!((last[0], last[1]), (0.4, 0.0));
}
