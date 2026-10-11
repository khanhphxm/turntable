//! Audio output: decoded PCM -> crossfade -> ring -> 10-band EQ -> output stage -> device.
//!
//! PCM is interleaved f32, 44.1 kHz, stereo. The decoder's thread mixes natural
//! transitions and pushes into a lock-free ring ([`PcmWriter`]); the platform
//! render thread runs the EQ, then the output stage (mono, volume normalization
//! with a true-peak limiter, balance). Rendering never allocates or locks.
//! macOS follows the system default unless a device is picked. Device discovery/opening and
//! stream replacement run on controller workers, never the host UI thread. Linux writes raw
//! PCM to `pw-cat`, `pacat` or `aplay`; Windows uses shared-mode WASAPI. A failed utility or a
//! ring that does not drain for two seconds reports a dead output for rebuilding from its feed.
//! While unavailable, PCM is dropped at a bounded pace rather than pausing/blocking the decoder.
//! Short ramps soften underruns and track starts. Every backend stops when idle.

#[cfg(feature = "audio-bench")]
pub mod benchmark;
mod crossfade;
mod eq;
mod leveler;
#[cfg_attr(target_os = "macos", path = "coreaudio.rs")]
#[cfg_attr(target_os = "linux", path = "linux.rs")]
#[cfg_attr(windows, path = "wasapi.rs")]
mod output;
#[cfg(any(target_os = "macos", windows, test, feature = "audio-bench"))]
mod resample;
mod ring;
mod stage;

#[cfg(any(test, feature = "audio-bench"))]
use std::sync::atomic::AtomicU64;
use std::{
	io,
	sync::{
		Arc, OnceLock,
		atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering},
	},
	task::{Wake, Waker},
	thread::{self, Thread},
	time::{Duration, Instant},
};

pub(crate) use crossfade::Crossfade;
pub use eq::{BANDS_HZ, EqSettings, GAIN_STEP_DB, MAX_GAIN_DB, PRESETS, sanitize_gain_db};
use eq::{Equalizer, Mailbox};
use futures_util::task::AtomicWaker;
use ring::Ring;
use stage::OutputStage;
pub use stage::{Normalize, StageParams, volume_db};

/// Rounds DSP samples, gains and coefficients to the f32 render format.
#[inline]
#[expect(clippy::cast_possible_truncation, reason = "DSP calculations use f64 precision; the render pipeline deliberately stores f32")]
pub(crate) fn dsp_f32(value: f64) -> f32 {
	value as f32
}

/// Whether this platform offers an output-device picker. Other platforms use the system default.
pub const CAN_PICK_OUTPUT: bool = output::CAN_PICK_OUTPUT;

/// An output device the user can pick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputDevice {
	/// The backend's persistent identifier, which survives reboots and replugging.
	pub uid:  String,
	pub name: String,
}

/// The devices the user can pick to play on.
pub fn output_devices() -> Vec<OutputDevice> {
	output::devices()
}

/// The name of the system's default output device, which plays when none is picked.
pub fn default_output_name() -> Option<String> {
	output::default_device_name()
}

/// Watches devices coming and going and the default changing, until dropped.
pub use output::Watch as DeviceWatch;

/// Starts watching the devices (`None` if the listener couldn't be registered).
pub fn watch_devices(notify: impl Fn() + Send + Sync + 'static) -> Option<DeviceWatch> {
	output::watch(notify)
}

/// Samples (not frames) the writer keeps buffered: 6144 frames ~ 139 ms.
const FILL_SAMPLES: usize = 6144 * 2;
/// After running dry, wait for this many samples before resuming (~46 ms): at least one of the
/// output's IO buffers, or the first callback runs dry again.
const REBUFFER_SAMPLES: usize = 2048 * 2;
/// The writer refills in chunks of at least this many samples (~46 ms): the
/// render callback wakes it only once this much room exists, so a playing
/// stream costs ~22 writer wake-ups a second instead of a polling loop.
const MIN_REFILL_SAMPLES: usize = 2048 * 2;
/// A parked writer re-checks at least this often (a missed wake-up, stop).
const READER_PARK: Duration = Duration::from_millis(100);
/// A vanished device must not hold the decoder (and its transport commands) indefinitely.
const WRITER_TIMEOUT: Duration = Duration::from_secs(2);
/// Eight milliseconds at 44.1 kHz; the same duration is used for track-start ramps.
pub(crate) const CLICK_FADE_FRAMES: usize = 353;
/// After this many frames of nothing but silence (1.5 s) the output unit is
/// stopped, so a paused player costs no audio callbacks at all.
const IDLE_STOP_FRAMES: u64 = 44_100 * 3 / 2;

/// The output stage's settings as the audio thread reads them: independent words, each
/// smoothed there, so no handshake is needed.
struct StageShared {
	normalize: AtomicU8,
	mono:      AtomicBool,
	/// `f32` bits, -1 (left) to 1 (right).
	balance:   AtomicU32,
	/// `f32` bits: the player's volume as attenuation in dB.
	volume_db: AtomicU32,
	/// How many tracks the player has reported (the stage looks for a change).
	program:   AtomicU32,
}

impl StageShared {
	fn new() -> Self {
		// 0 is the bits of 0.0f32.
		Self {
			normalize: AtomicU8::new(0),
			mono:      AtomicBool::new(false),
			balance:   AtomicU32::new(0),
			volume_db: AtomicU32::new(0),
			program:   AtomicU32::new(0),
		}
	}
}

struct Shared {
	ring:       Ring,
	mailbox:    Mailbox,
	/// Natural-transition overlap in seconds, read by the decoder (not the callback).
	crossfade:  AtomicU8,
	stop:       AtomicBool,
	/// Output failure, distinct from permanent shutdown: the decoder keeps accepting commands.
	dead:       AtomicBool,
	/// A replacement owns the feed while the old backend is being shut down.
	recovering: AtomicBool,
	failure:    OnceLock<Box<dyn Fn() + Send + Sync>>,
	ended:      AtomicBool,
	#[cfg(any(test, feature = "audio-bench"))]
	underruns:  AtomicU64,
	#[cfg(feature = "audio-bench")]
	writer_parks: AtomicU64,
	#[cfg(feature = "audio-bench")]
	writer_wakes: AtomicU64,
	/// The writer's thread, for the render callback to unpark.
	writer:     OnceLock<Thread>,
	/// The writer is parked waiting for room in the ring.
	waiting:    AtomicBool,
	/// The output unit is running (render callbacks are happening).
	running:    AtomicBool,
	/// Requests for the control thread, which owns starting and stopping.
	want_stop:  AtomicBool,
	want_start: AtomicBool,
	/// Re-registerable: a rebuilt output has a new control thread. Wake never locks/allocates.
	control:    AtomicWaker,
	stage:      StageShared,
	/// Backend control state; default-output backends need none.
	_output:    output::State,
}

impl Shared {
	/// Asks the control thread to resume output (new audio arrived while stopped).
	fn request_start(&self) {
		if !self.running.load(Ordering::Acquire) && !self.want_start.swap(true, Ordering::AcqRel) {
			self.control.wake();
		}
	}

	fn writer_stopped(&self) -> bool {
		// A restart clears stop before releasing recovering. Re-read stop after observing
		// that release, rather than mistaking the old backend's stop for permanent shutdown.
		self.stop.load(Ordering::Acquire) && !self.recovering.load(Ordering::Acquire) && self.stop.load(Ordering::Acquire)
	}

	fn output_failed(&self) {
		self.running.store(false, Ordering::Release);
		if !self.dead.swap(true, Ordering::AcqRel)
			&& let Some(notify) = self.failure.get()
		{
			notify();
		}
		if let Some(writer) = self.writer.get() {
			writer.unpark();
		}
	}
}

impl Shared {
	fn new() -> Self {
		Self {
			ring:                   Ring::new(),
			mailbox:                Mailbox::new(),
			crossfade:              AtomicU8::new(0),
			stop:                   AtomicBool::new(false),
			dead:                   AtomicBool::new(false),
			recovering:             AtomicBool::new(false),
			failure:                OnceLock::new(),
			ended:                  AtomicBool::new(false),
			#[cfg(any(test, feature = "audio-bench"))]
			underruns:              AtomicU64::new(0),
			#[cfg(feature = "audio-bench")]
			writer_parks: AtomicU64::new(0),
			#[cfg(feature = "audio-bench")]
			writer_wakes: AtomicU64::new(0),
			writer:                 OnceLock::new(),
			waiting:                AtomicBool::new(false),
			running:                AtomicBool::new(true),
			want_stop:              AtomicBool::new(false),
			want_start:             AtomicBool::new(false),
			control:                AtomicWaker::new(),
			stage:                  StageShared::new(),
			_output:                output::State::default(),
		}
	}
}

struct ControlThread(Thread);

impl Wake for ControlThread {
	fn wake(self: Arc<Self>) {
		self.0.unpark();
	}

	fn wake_by_ref(self: &Arc<Self>) {
		self.0.unpark();
	}
}

fn control_waker() -> Waker {
	Waker::from(Arc::new(ControlThread(thread::current())))
}

/// The producer's end of the audio path: the decoder pushes interleaved stereo `f32` frames.
pub struct PcmWriter(Arc<Shared>);

/// The consumer's end, handed to [`Player::start`].
pub struct Feed(Arc<Shared>);

/// A new audio path: what the writer pushes, the player plays.
pub fn channel() -> (PcmWriter, Feed) {
	let shared = Arc::new(Shared::new());
	(PcmWriter(shared.clone()), Feed(shared))
}

impl Feed {
	/// Installs the output-failure callback once for this path. It runs on a decoder/output
	/// thread and must only post an event and wake its host, never wait for that host.
	pub fn on_failure(&self, notify: impl Fn() + Send + Sync + 'static) {
		self.0.failure.get_or_init(|| Box::new(notify));
	}
}

impl PcmWriter {
	/// The currently requested natural-transition overlap, in seconds.
	pub(crate) fn crossfade(&self) -> u8 {
		self.0.crossfade.load(Ordering::Relaxed)
	}

	/// Drops queued old audio at the next render callback without discarding new PCM.
	pub(crate) fn clear(&self) {
		self.0.ring.discard_buffered();
	}

	/// Queues whole stereo frames at the output's pace. A stalled output is reported dead
	/// after two seconds; unavailable audio is dropped, without turning a transient device
	/// failure into a librespot pause. Returns false only for permanent shutdown.
	pub fn write(&self, samples: &[f32]) -> bool {
		self.write_with_timeout(samples, WRITER_TIMEOUT)
	}

	fn write_with_timeout(&self, mut samples: &[f32], timeout: Duration) -> bool {
		let sh = &*self.0;
		sh.writer.get_or_init(thread::current);
		let mut progress = Instant::now();
		let mut queued = sh.ring.len();
		while !samples.is_empty() {
			if sh.writer_stopped() {
				return false;
			}
			if sh.dead.load(Ordering::Acquire) {
				sh.ring.discard_buffered();
				// Bound both decoder wake-ups and transport-command latency while there is no device.
				let frames = u64::try_from(samples.len() / 2).unwrap_or(u64::MAX);
				thread::park_timeout(Duration::from_micros(frames.saturating_mul(1_000_000) / 44_100).min(READER_PARK));
				return !sh.writer_stopped();
			}
			let now_queued = sh.ring.len();
			if now_queued < queued {
				progress = Instant::now();
			}
			queued = now_queued;
			// Wait for a refill's worth of room (or room for all that's left): the render
			// callback wakes this thread then, so a playing stream costs ~22 wake-ups a second.
			let need = samples.len().min(MIN_REFILL_SAMPLES);
			if FILL_SAMPLES.saturating_sub(sh.ring.len()) < need {
				if progress.elapsed() >= timeout {
					sh.output_failed();
					continue;
				}
				sh.waiting.store(true, Ordering::Release);
				// A full ring with output stopped would wait forever: wake it up.
				sh.request_start();
				if FILL_SAMPLES.saturating_sub(sh.ring.len()) < need {
					#[cfg(feature = "audio-bench")]
					sh.writer_parks.fetch_add(1, Ordering::Relaxed);
					thread::park_timeout(READER_PARK.min(timeout.saturating_sub(progress.elapsed())));
				}
				sh.waiting.store(false, Ordering::Release);
				continue;
			}
			let room = FILL_SAMPLES.saturating_sub(sh.ring.len()) & !1;
			let Some(chunk) = samples.get(..room.min(samples.len())) else { return false };
			let n = sh.ring.push(chunk);
			samples = samples.get(n..).unwrap_or_default();
			queued = sh.ring.len();
			progress = Instant::now();
			// Audio after a pause: resume the (idle-stopped) output.
			sh.request_start();
		}
		true
	}
}

impl Drop for PcmWriter {
	fn drop(&mut self) {
		self.0.ended.store(true, Ordering::Release);
	}
}

/// Plays the decoder's PCM through the platform output with an EQ and output stage.
pub struct Player {
	shared: Arc<Shared>,
	out:    output::Output,
}

/// Why [`Player::start`] failed, and the feed back, to try again with later.
pub struct StartError {
	pub error: io::Error,
	pub feed:  Feed,
}

impl std::fmt::Debug for StartError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("StartError").field("error", &self.error).finish_non_exhaustive()
	}
}

impl Player {
	/// Starts the platform output on `device`, when selection is supported, or the system default.
	/// Playback begins as soon as enough PCM has been buffered. When the output can't start (no
	/// device, the device busy) the feed comes back with the error, so the caller can try again.
	pub fn start(feed: Feed, device: Option<&str>) -> Result<Self, StartError> {
		Self::start_with(feed, device, output::Output::start)
	}

	fn start_with(
		feed: Feed,
		device: Option<&str>,
		open: impl FnOnce(Arc<Shared>, Option<&str>) -> io::Result<output::Output>,
	) -> Result<Self, StartError> {
		let shared = feed.0;
		// The previous backend has been joined before its feed is returned. Reset its shutdown
		// and idle requests, and let the new thread register its own control waker.
		shared.stop.store(false, Ordering::Release);
		shared.dead.store(false, Ordering::Release);
		shared.recovering.store(false, Ordering::Release);
		shared.want_stop.store(false, Ordering::Release);
		shared.want_start.store(false, Ordering::Release);
		match open(shared.clone(), device) {
			Ok(out) => Ok(Self { shared, out }),
			Err(error) => {
				shared.output_failed();
				Err(StartError { error, feed: Feed(shared) })
			},
		}
	}

	/// Hands new EQ settings to the audio thread (lock-free on its side). The
	/// change is smoothed over ~17 ms.
	pub fn set_eq(&self, settings: EqSettings) {
		self.shared.mailbox.publish(&settings);
	}

	/// Hands the output stage its settings (lock-free on the audio side; changes are ramped).
	pub fn set_stage(&self, params: &StageParams) {
		let st = &self.shared.stage;
		st.normalize.store(params.normalize.code(), Ordering::Relaxed);
		st.mono.store(params.mono, Ordering::Relaxed);
		st.balance.store(params.balance.to_bits(), Ordering::Relaxed);
		st.volume_db.store(params.volume_db.to_bits(), Ordering::Relaxed);
	}

	/// Sets the overlap for the next natural transition. In-flight fades finish normally.
	pub fn set_crossfade(&self, seconds: u8) {
		self.shared.crossfade.store(seconds.min(12), Ordering::Relaxed);
	}

	/// Tells the stage the player's volume (0 to 100), so normalization leaves the knob alone.
	pub fn set_volume(&self, percent: u8) {
		self.shared.stage.volume_db.store(volume_db(percent).to_bits(), Ordering::Relaxed);
	}

	/// A new track starts: normalization settles on it quickly instead of averaging in the last.
	pub fn new_track(&self) {
		self.shared.stage.program.fetch_add(1, Ordering::Relaxed);
	}

	/// The device it plays on (`None`: the system's default output, whichever that is now).
	pub fn device(&self) -> Option<&str> {
		self.out.device()
	}

	/// Whether an output is open (it isn't when the last attempt to open one failed).
	pub fn has_output(&self) -> bool {
		self.out.has_output()
	}

	/// Whether the opened stream must follow a changed default or device format.
	pub fn is_stale(&self) -> bool {
		self.out.is_stale()
	}

	/// Whether the backend exited or its ring stopped draining.
	pub fn is_dead(&self) -> bool {
		self.shared.dead.load(Ordering::Acquire)
	}

	/// Stops/joins the old backend and returns its original feed for rebuilding. Run on an
	/// output worker: disposal can wait for a driver callback or a blocked Linux pipe write.
	pub fn into_feed(self) -> Feed {
		let Self { shared, out } = self;
		shared.recovering.store(true, Ordering::Release);
		shared.dead.store(true, Ordering::Release);
		drop(out);
		Feed(shared)
	}

	/// Changes the output device where selection is supported; otherwise keeps the system default.
	/// A missing device falls back to the default. An error keeps the stream for a later retry.
	pub fn set_device(&mut self, uid: Option<&str>) -> io::Result<()> {
		self.out.set_device(uid)
	}
}

/// Optional device selection. Backends using only the system default keep these defaults.
trait DeviceControl {
	fn device(&self) -> Option<&str> {
		None
	}

	fn has_output(&self) -> bool {
		true
	}

	fn is_stale(&self) -> bool {
		false
	}

	fn set_device(&mut self, _uid: Option<&str>) -> io::Result<()> {
		Ok(())
	}
}

/// Everything the platform callback needs; owned by the audio thread.
pub(crate) struct Render {
	shared:        Arc<Shared>,
	eq:            Equalizer,
	stage:         OutputStage,
	seen:          u32,
	/// How many tracks the stage has been told about.
	program:       u32,
	/// Waiting for the ring to fill (start-up or after running dry).
	buffering:     bool,
	/// Last PCM frame and bounded tails bridge underruns even across short callbacks.
	last:          [f32; 2],
	fade_out:      usize,
	fade_in:       usize,
	/// Frames of pure silence in a row (for stopping the unit when idle).
	silent_frames: u64,
}

impl Render {
	fn new(shared: Arc<Shared>) -> Render {
		Render {
			shared,
			eq: Equalizer::new(),
			stage: OutputStage::new(),
			seen: 0,
			program: 0,
			buffering: true,
			last: [0.0; 2],
			fade_out: 0,
			fade_in: 0,
			silent_frames: 0,
		}
	}

	/// Hands the stage the settings published since the last callback.
	fn poll_stage(stage: &mut OutputStage, program: &mut u32, shared: &Shared) {
		let st = &shared.stage;
		stage.set(&StageParams {
			normalize: Normalize::from_code(st.normalize.load(Ordering::Relaxed)),
			mono:      st.mono.load(Ordering::Relaxed),
			balance:   f32::from_bits(st.balance.load(Ordering::Relaxed)),
			volume_db: f32::from_bits(st.volume_db.load(Ordering::Relaxed)),
		});
		let now = st.program.load(Ordering::Relaxed);
		if now != *program {
			*program = now;
			stage.new_program();
		}
	}

	/// No lookahead/allocation: extend the last frame into an eight-millisecond tail, then
	/// ramp the refill from zero. Keep stereo gains identical and count frames, not packets.
	fn soften_underrun(out: &mut [f32], n: usize, last: &mut [f32; 2], fade_in: &mut usize, fade_out: &mut usize) -> bool {
		let (audio, silence) = out.split_at_mut(n);
		for frame in audio.as_chunks_mut::<2>().0.iter_mut().take(*fade_in) {
			let gain = 1.0 - *fade_in as f32 / CLICK_FADE_FRAMES as f32;
			for sample in frame {
				*sample *= gain;
			}
			*fade_in -= 1;
		}
		if let Some(frame) = audio.as_chunks::<2>().0.last() {
			*last = *frame;
		}
		let tail = *fade_out > 0;
		for frame in silence.as_chunks_mut::<2>().0 {
			if *fade_out == 0 {
				break;
			}
			*fade_out -= 1;
			let gain = *fade_out as f32 / CLICK_FADE_FRAMES as f32;
			for (sample, previous) in frame.iter_mut().zip(last.iter()) {
				*sample = previous * gain;
			}
		}
		tail
	}

	/// Fills `out` (interleaved stereo f32). Returns true if it is all silence.
	pub(crate) fn render(&mut self, out: &mut [f32]) -> bool {
		let sh = &*self.shared;
		if let Some(s) = sh.mailbox.poll(&mut self.seen) {
			self.eq.set(&s);
		}
		let ended = sh.ended.load(Ordering::Acquire);
		let avail = sh.ring.len();
		if self.buffering && self.fade_out == 0 && (avail >= REBUFFER_SAMPLES || (ended && avail > 0)) {
			self.buffering = false;
		}
		let want = out.len() & !1;
		let n = if self.buffering { 0 } else { out.get_mut(..want).map_or(0, |frames| sh.ring.pop(frames)) };
		let underrun = n < want && !self.buffering && !ended;
		if n < out.len() {
			if let Some(silence) = out.get_mut(n..) {
				silence.fill(0.0);
			}
			if !self.buffering && !ended {
				#[cfg(any(test, feature = "audio-bench"))]
				sh.underruns.fetch_add(1, Ordering::Relaxed);
				self.buffering = true;
				self.fade_out = CLICK_FADE_FRAMES;
			}
		}
		let tail = Self::soften_underrun(out, n, &mut self.last, &mut self.fade_in, &mut self.fade_out);
		if underrun {
			self.fade_in = CLICK_FADE_FRAMES;
		}
		// Wake the writer once a refill's worth of room exists. `unpark` doesn't
		// allocate or block, so it's safe on the audio thread.
		if sh.waiting.load(Ordering::Acquire)
			&& FILL_SAMPLES.saturating_sub(sh.ring.len()) >= MIN_REFILL_SAMPLES
			&& sh.waiting.swap(false, Ordering::AcqRel)
			&& let Some(t) = sh.writer.get()
		{
			#[cfg(feature = "audio-bench")]
			sh.writer_wakes.fetch_add(1, Ordering::Relaxed);
			t.unpark();
		}
		// Run the EQ over silence too so filter tails ring out instead of freezing.
		self.eq.process(out);
		Self::poll_stage(&mut self.stage, &mut self.program, sh);
		self.stage.process(out);
		let silent = n == 0 && !tail && self.eq.is_bypassed() && self.stage.is_idle();
		// Nothing to play for a while (paused, idle): have the unit stopped.
		self.silent_frames = if n == 0 { self.silent_frames + (out.len() / 2) as u64 } else { 0 };
		if self.silent_frames >= IDLE_STOP_FRAMES && !sh.want_stop.swap(true, Ordering::AcqRel) {
			sh.control.wake();
		}
		silent
	}
}

#[cfg(test)]
mod tests;
