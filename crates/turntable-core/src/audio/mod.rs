//! Audio output: decoded PCM -> crossfade -> ring -> 10-band EQ -> output stage -> device.
//!
//! PCM is interleaved f32, 44.1 kHz, stereo. The decoder's thread mixes natural
//! transitions and pushes into a lock-free ring ([`PcmWriter`]); the platform
//! render thread runs the EQ, then the output stage (mono, volume normalization
//! with a true-peak limiter, balance). Rendering never allocates or locks.
//! macOS plays through CoreAudio and follows the system default unless a device is picked. Linux writes raw PCM to
//! `pw-cat`, `pacat` or `aplay`; Windows uses shared-mode WASAPI. Both use the system
//! default output. Every backend stops when idle.

mod crossfade;
mod eq;
mod leveler;
#[cfg_attr(target_os = "macos", path = "coreaudio.rs")]
#[cfg_attr(target_os = "linux", path = "linux.rs")]
#[cfg_attr(windows, path = "wasapi.rs")]
mod output;
#[cfg(any(target_os = "macos", windows, test))]
mod resample;
mod ring;
mod stage;

#[cfg(test)]
use std::sync::atomic::AtomicU64;
use std::{
	io,
	sync::{
		Arc, OnceLock,
		atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering},
	},
	thread::{self, Thread},
	time::Duration,
};

pub(crate) use crossfade::Crossfade;
pub use eq::{BANDS_HZ, EqSettings, PRESETS};
use eq::{Equalizer, Mailbox};
use ring::Ring;
use stage::OutputStage;
pub use stage::{Normalize, StageParams, volume_db};

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
	ended:      AtomicBool,
	#[cfg(test)]
	underruns:  AtomicU64,
	/// The writer's thread, for the render callback to unpark.
	writer:     OnceLock<Thread>,
	/// The writer is parked waiting for room in the ring.
	waiting:    AtomicBool,
	/// The output unit is running (render callbacks are happening).
	running:    AtomicBool,
	/// Requests for the control thread, which owns starting and stopping.
	want_stop:  AtomicBool,
	want_start: AtomicBool,
	control:    OnceLock<Thread>,
	stage:      StageShared,
	/// Backend control state; default-output backends need none.
	_output:    output::State,
}

impl Shared {
	/// Asks the control thread to resume output (new audio arrived while stopped).
	fn request_start(&self) {
		if !self.running.load(Ordering::Acquire)
			&& !self.want_start.swap(true, Ordering::AcqRel)
			&& let Some(t) = self.control.get()
		{
			t.unpark();
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
			ended:                  AtomicBool::new(false),
			#[cfg(test)]
			underruns:              AtomicU64::new(0),
			writer:                 OnceLock::new(),
			waiting:                AtomicBool::new(false),
			running:                AtomicBool::new(true),
			want_stop:              AtomicBool::new(false),
			want_start:             AtomicBool::new(false),
			control:                OnceLock::new(),
			stage:                  StageShared::new(),
			_output:                output::State::default(),
		}
	}
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

impl PcmWriter {
	/// The currently requested natural-transition overlap, in seconds.
	pub(crate) fn crossfade(&self) -> u8 {
		self.0.crossfade.load(Ordering::Relaxed)
	}

	/// Drops queued old audio at the next render callback without discarding new PCM.
	pub(crate) fn clear(&self) {
		self.0.ring.discard_buffered();
	}

	/// Queues `samples` (whole stereo frames), blocking while the buffer is full: the output's
	/// pace is the decoder's. Returns false once the player is gone (the samples are dropped).
	pub fn write(&self, mut samples: &[f32]) -> bool {
		let sh = &*self.0;
		sh.writer.get_or_init(thread::current);
		while !samples.is_empty() {
			if sh.stop.load(Ordering::Acquire) {
				return false;
			}
			// Wait for a refill's worth of room (or room for all that's left): the render
			// callback wakes this thread then, so a playing stream costs ~22 wake-ups a second.
			let need = samples.len().min(MIN_REFILL_SAMPLES);
			if FILL_SAMPLES.saturating_sub(sh.ring.len()) < need {
				sh.waiting.store(true, Ordering::Release);
				// A full ring with output stopped would wait forever: wake it up.
				sh.request_start();
				if FILL_SAMPLES.saturating_sub(sh.ring.len()) < need {
					thread::park_timeout(READER_PARK);
				}
				sh.waiting.store(false, Ordering::Release);
				continue;
			}
			let room = FILL_SAMPLES.saturating_sub(sh.ring.len()) & !1;
			let Some(chunk) = samples.get(..room.min(samples.len())) else { return false };
			let n = sh.ring.push(chunk);
			samples = samples.get(n..).unwrap_or_default();
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
		let shared = feed.0;
		match output::Output::start(shared.clone(), device) {
			Ok(out) => Ok(Self { shared, out }),
			Err(error) => Err(StartError { error, feed: Feed(shared) }),
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
	/// Frames of pure silence in a row (for stopping the unit when idle).
	silent_frames: u64,
}

impl Render {
	fn new(shared: Arc<Shared>) -> Render {
		Render { shared, eq: Equalizer::new(), stage: OutputStage::new(), seen: 0, program: 0, buffering: true, silent_frames: 0 }
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

	/// Fills `out` (interleaved stereo f32). Returns true if it is all silence.
	pub(crate) fn render(&mut self, out: &mut [f32]) -> bool {
		let sh = &*self.shared;
		if let Some(s) = sh.mailbox.poll(&mut self.seen) {
			self.eq.set(&s);
		}
		let ended = sh.ended.load(Ordering::Acquire);
		let avail = sh.ring.len();
		if self.buffering && (avail >= REBUFFER_SAMPLES || (ended && avail > 0)) {
			self.buffering = false;
		}
		let want = out.len() & !1;
		let n = if self.buffering { 0 } else { out.get_mut(..want).map_or(0, |frames| sh.ring.pop(frames)) };
		if n < out.len() {
			if let Some(silence) = out.get_mut(n..) {
				silence.fill(0.0);
			}
			if !self.buffering && !ended {
				#[cfg(test)]
				sh.underruns.fetch_add(1, Ordering::Relaxed);
				self.buffering = true;
			}
		}
		// Wake the writer once a refill's worth of room exists. `unpark` doesn't
		// allocate or block, so it's safe on the audio thread.
		if sh.waiting.load(Ordering::Acquire)
			&& FILL_SAMPLES.saturating_sub(sh.ring.len()) >= MIN_REFILL_SAMPLES
			&& sh.waiting.swap(false, Ordering::AcqRel)
			&& let Some(t) = sh.writer.get()
		{
			t.unpark();
		}
		// Run the EQ over silence too so filter tails ring out instead of freezing.
		self.eq.process(out);
		Self::poll_stage(&mut self.stage, &mut self.program, sh);
		self.stage.process(out);
		let silent = n == 0 && self.eq.is_bypassed() && self.stage.is_idle();
		// Nothing to play for a while (paused, idle): have the unit stopped.
		self.silent_frames = if n == 0 { self.silent_frames + (out.len() / 2) as u64 } else { 0 };
		if self.silent_frames >= IDLE_STOP_FRAMES
			&& !sh.want_stop.swap(true, Ordering::AcqRel)
			&& let Some(t) = sh.control.get()
		{
			t.unpark();
		}
		silent
	}
}

#[cfg(test)]
mod tests;
