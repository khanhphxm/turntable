use std::{
	alloc::{GlobalAlloc, Layout, System},
	cell::Cell,
	fs::File,
	io::{self, BufWriter, Write},
	path::PathBuf,
	sync::{
		Arc, Barrier,
		atomic::{AtomicBool, AtomicU64, Ordering},
	},
	thread,
	time::{Duration, Instant},
};

use serde_json::json;
use turntable_core::{
	audio::{
		self, EqSettings, Normalize,
		benchmark::{NullOutput, resource_usage},
	},
	engine::benchmark::Stream,
};

static TRACKING: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED: AtomicU64 = AtomicU64::new(0);
static LIVE: AtomicU64 = AtomicU64::new(0);
static PEAK: AtomicU64 = AtomicU64::new(0);
static RENDER_ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
thread_local! { static RENDERING: Cell<bool> = const { Cell::new(false) }; }

struct MeasuredAllocator;

fn allocated(size: usize) {
	let size = size as u64;
	let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
	if TRACKING.load(Ordering::Relaxed) {
		ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
		ALLOCATED.fetch_add(size, Ordering::Relaxed);
		PEAK.fetch_max(live, Ordering::Relaxed);
		if RENDERING.get() {
			RENDER_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
		}
	}
}

// SAFETY: every operation delegates the caller's original layout/pointer to System;
// the counters never access allocated memory or change the returned pointer.
unsafe impl GlobalAlloc for MeasuredAllocator {
	unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
		// SAFETY: GlobalAlloc's caller supplies a valid allocation layout.
		let result = unsafe { System.alloc(layout) };
		if !result.is_null() {
			allocated(layout.size());
		}
		result
	}

	unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
		// SAFETY: GlobalAlloc's caller supplies a valid allocation layout.
		let result = unsafe { System.alloc_zeroed(layout) };
		if !result.is_null() {
			allocated(layout.size());
		}
		result
	}

	unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
		LIVE.fetch_sub(layout.size() as u64, Ordering::Relaxed);
		// SAFETY: GlobalAlloc's caller guarantees ptr/layout belong to this allocator.
		unsafe { System.dealloc(ptr, layout) };
	}

	unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
		// SAFETY: GlobalAlloc's caller supplies the live allocation and a valid new size.
		let result = unsafe { System.realloc(ptr, layout, size) };
		if !result.is_null() {
			LIVE.fetch_sub(layout.size() as u64, Ordering::Relaxed);
			allocated(size);
		}
		result
	}
}

#[global_allocator]
static ALLOCATOR: MeasuredAllocator = MeasuredAllocator;

struct Options {
	file:         PathBuf,
	rate:         u32,
	frames:       usize,
	repeats:      u32,
	crossfade_ms: u32,
	normalize:    Normalize,
	eq:           EqSettings,
	output:       Option<PathBuf>,
}

fn invalid(message: impl Into<String>) -> io::Error {
	io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn options() -> io::Result<Options> {
	let mut args = std::env::args().skip(1);
	let file = args.next().ok_or_else(|| invalid("usage: audio-bench FILE [--rate HZ] [--frames N] [--repeat N] [--normalize off|quiet|normal|loud] [--eq flat|enabled-flat|bass_boost|rock|...] [--crossfade-ms N] [--output FILE]"))?;
	let mut options = Options {
		file:         file.into(),
		rate:         44_100,
		frames:       2048,
		repeats:      1,
		crossfade_ms: 0,
		normalize:    Normalize::Off,
		eq:           EqSettings::FLAT,
		output:       None,
	};
	while let Some(key) = args.next() {
		let value = args.next().ok_or_else(|| invalid(format!("missing value for {key}")))?;
		match key.as_str() {
			"--rate" => options.rate = value.parse().map_err(|error| invalid(format!("{key}: {error}")))?,
			"--frames" => options.frames = value.parse().map_err(|error| invalid(format!("{key}: {error}")))?,
			"--repeat" => options.repeats = value.parse().map_err(|error| invalid(format!("{key}: {error}")))?,
			"--crossfade-ms" => options.crossfade_ms = value.parse().map_err(|error| invalid(format!("{key}: {error}")))?,
			"--normalize" => {
				options.normalize = match value.as_str() {
					"off" => Normalize::Off,
					"quiet" => Normalize::Quiet,
					"normal" => Normalize::Normal,
					"loud" => Normalize::Loud,
					_ => return Err(invalid("unknown normalization mode")),
				}
			},
			"--eq" => {
				options.eq = if value == "enabled-flat" {
					EqSettings { enabled: true, ..EqSettings::FLAT }
				} else {
					EqSettings::preset(&value).ok_or_else(|| invalid("unknown EQ preset"))?
				}
			},
			"--output" => options.output = Some(value.into()),
			_ => return Err(invalid(format!("unknown option {key}"))),
		}
	}
	if !(8000..=192_000).contains(&options.rate) || !(64..=8192).contains(&options.frames) || options.repeats == 0 {
		return Err(invalid("rate must be 8000..192000, frames 64..8192, repeat >= 1"));
	}
	Ok(options)
}

pub fn run() -> io::Result<()> {
	let options = options()?;
	let (writer, feed) = audio::channel();
	let stream = Stream::new(&options.file, writer, options.crossfade_ms)?;
	let mut output = NullOutput::new(feed, options.rate, options.eq, options.normalize);
	let mut samples = vec![0.0f32; options.frames * 2];
	let mut dump = options.output.as_ref().map(File::create).transpose()?.map(BufWriter::new);
	let barrier = Arc::new(Barrier::new(2));
	let start_decoder = barrier.clone();
	let repeats = options.repeats;
	let decoder = thread::Builder::new().name("audio-bench-decoder".into()).spawn(move || {
		start_decoder.wait();
		stream.run(repeats)
	})?;
	let heap_start = LIVE.load(Ordering::Relaxed);
	PEAK.store(heap_start, Ordering::Relaxed);
	let before = resource_usage()?;
	let start = Instant::now();
	TRACKING.store(true, Ordering::Relaxed);
	barrier.wait();
	let mut rendered_frames = 0u64;
	let mut drain_callbacks = 0u64;
	let mut late_callbacks = 0u64;
	let drain_frames = (2048u64 * u64::from(options.rate)).div_ceil(44_100);
	loop {
		// Absolute deadlines do not accumulate sleep drift. The initial period lets the ring fill.
		rendered_frames += options.frames as u64;
		let deadline = start + Duration::from_nanos(rendered_frames * 1_000_000_000 / u64::from(options.rate));
		let now = Instant::now();
		if now < deadline {
			thread::sleep(deadline - now);
		} else {
			late_callbacks += 1;
		}
		RENDERING.set(true);
		output.render(&mut samples);
		RENDERING.set(false);
		if let Some(dump) = &mut dump {
			for sample in &samples {
				dump.write_all(&sample.to_le_bytes())?;
			}
		}
		if output.drained() {
			drain_callbacks += 1;
			// Flush both the converter's pull buffer/FIR and the leveler's lookahead.
			if drain_callbacks * options.frames as u64 >= drain_frames {
				break;
			}
		}
	}
	let decoded = match decoder.join() {
		Ok(decoded) => decoded?,
		Err(error) => std::panic::resume_unwind(error),
	};
	TRACKING.store(false, Ordering::Relaxed);
	let elapsed = start.elapsed().as_secs_f64();
	let after = resource_usage()?;
	if let Some(dump) = &mut dump {
		dump.flush()?;
	}
	let scheduling = output.scheduling();
	let cpu = after.cpu_seconds - before.cpu_seconds;
	let audio = decoded.frames as f64 / 44_100.0;
	let heap_end = LIVE.load(Ordering::Relaxed);
	let result = json!({
		"file": options.file, "rate_hz": options.rate, "buffer_frames": options.frames, "normalize": options.normalize,
		"eq": options.eq, "crossfade_ms": options.crossfade_ms, "repeats": options.repeats,
		"decoded_frames": decoded.frames, "packets": decoded.packets, "audio_seconds": audio, "wall_seconds": elapsed,
		"cpu_seconds": cpu, "cpu_percent": cpu / elapsed * 100.0, "cpu_percent_per_audio_stream": cpu / audio * 100.0,
		"cpu_ms_per_audio_second": cpu / audio * 1000.0,
		"callbacks_per_second": scheduling.callbacks as f64 / elapsed,
		"writer_parks_per_second": scheduling.writer_parks as f64 / elapsed,
		"writer_wakes_per_second": scheduling.writer_wakes as f64 / elapsed,
		"voluntary_context_switches_per_second": (after.voluntary_switches - before.voluntary_switches) as f64 / elapsed,
		"involuntary_context_switches_per_second": (after.involuntary_switches - before.involuntary_switches) as f64 / elapsed,
		"allocations": ALLOCATIONS.load(Ordering::Relaxed), "allocated_bytes": ALLOCATED.load(Ordering::Relaxed),
		"allocations_per_audio_second": ALLOCATIONS.load(Ordering::Relaxed) as f64 / audio,
		"render_allocations": RENDER_ALLOCATIONS.load(Ordering::Relaxed),
		"heap_start_bytes": heap_start, "heap_peak_bytes": PEAK.load(Ordering::Relaxed), "heap_end_bytes": heap_end,
		"peak_rss_kib": after.peak_rss_kib, "underruns": scheduling.underruns, "late_callbacks": late_callbacks,
	});
	serde_json::to_writer(io::stdout().lock(), &result)?;
	io::stdout().lock().write_all(b"\n")?;
	if scheduling.underruns != 0 || RENDER_ALLOCATIONS.load(Ordering::Relaxed) != 0 {
		return Err(io::Error::other("invalid run: PCM underrun or render-thread allocation"));
	}
	Ok(())
}
