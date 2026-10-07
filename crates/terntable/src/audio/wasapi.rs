//! Windows output on the default render endpoint through shared-mode, event-driven WASAPI.
//!
//! One MTA thread owns all COM objects, renders stereo float PCM at the device's mix rate
//! with the existing resampler, and lets the audio engine map it to the endpoint's channels.
//! The ~100 ms buffer needs no polling. Idle stops and releases the client, then parks;
//! resuming or recovering from an invalidated/stalled endpoint opens the current default.

use std::{
	io, slice,
	sync::{Arc, atomic::Ordering, mpsc},
	thread::{self, JoinHandle},
	time::Duration,
};

use windows::Win32::{
	Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0},
	Media::Audio::{
		AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
		AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, IAudioClient, IAudioRenderClient, IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX,
		eConsole, eRender,
	},
	System::{
		Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize},
		Threading::{CreateEventW, SetEvent, WaitForSingleObject},
	},
};

use super::{
	Render, Shared,
	resample::{MAX_OUT_FRAMES, Resampler},
};

#[path = "system_default.rs"]
mod picker;

pub use picker::Watch;
pub(super) use picker::{CAN_PICK_OUTPUT, State, default_device_name, devices, watch};

/// WASAPI durations are in 100 ns units: 100 ms, deliberately not a low-latency stream.
const BUFFER_DURATION: i64 = 1_000_000;
/// A removed device can stop signaling its event instead of delivering an error.
const EVENT_TIMEOUT_MS: u32 = 2000;
const REOPEN_AFTER: Duration = Duration::from_secs(1);

struct Event(HANDLE);

// SAFETY: a kernel event can be signaled and waited on by any thread. The last Arc owns
// closing the handle, after the client and the worker have both finished using it.
unsafe impl Send for Event {}
// SAFETY: SetEvent/WaitForSingleObject synchronize access in the kernel; the handle is immutable.
unsafe impl Sync for Event {}

impl Event {
	fn new() -> windows::core::Result<Self> {
		// SAFETY: an unnamed, auto-reset event with default security, owned by this wrapper.
		unsafe { CreateEventW(None, false, false, None).map(Self) }
	}
}

impl Drop for Event {
	fn drop(&mut self) {
		// SAFETY: this wrapper owns the event and no other reference remains.
		let _ = unsafe { CloseHandle(self.0) };
	}
}

struct Com;

impl Com {
	fn initialize() -> windows::core::Result<Self> {
		// SAFETY: this fresh render thread has no apartment yet; all COM objects stay on it.
		unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()? };
		Ok(Self)
	}
}

impl Drop for Com {
	fn drop(&mut self) {
		// SAFETY: balances this thread's successful initialization, after its COM objects drop.
		unsafe { CoUninitialize() };
	}
}

pub(super) struct Output {
	shared: Arc<Shared>,
	event:  Arc<Event>,
	worker: Option<JoinHandle<()>>,
}

impl Output {
	pub(super) fn start(shared: Arc<Shared>, _device: Option<&str>) -> io::Result<Self> {
		let event = Arc::new(Event::new().map_err(io::Error::other)?);
		let (ready, started) = mpsc::channel();
		let worker = {
			let shared = shared.clone();
			let event = event.clone();
			thread::Builder::new().name("terntable-audio".into()).spawn(move || worker(&shared, &event, &ready))?
		};
		match started.recv().unwrap_or_else(|e| Err(io::Error::other(e))) {
			Ok(()) => Ok(Self { shared, event, worker: Some(worker) }),
			Err(error) => {
				// Failed initialization leaves the feed usable: the UI can retry it.
				let _ = worker.join();
				Err(error)
			},
		}
	}
}

impl super::DeviceControl for Output {}

impl Drop for Output {
	fn drop(&mut self) {
		self.shared.stop.store(true, Ordering::Release);
		if let Some(writer) = self.shared.writer.get() {
			writer.unpark();
		}
		if let Some(worker) = self.worker.take() {
			// Wake either the running client's event wait or the idle/reopen park.
			// SAFETY: the event is still owned by this Output and the worker.
			let _ = unsafe { SetEvent(self.event.0) };
			worker.thread().unpark();
			let _ = worker.join();
		}
	}
}

fn worker(shared: &Arc<Shared>, event: &Event, ready: &mpsc::Sender<io::Result<()>>) {
	let _com = match Com::initialize() {
		Ok(com) => com,
		Err(error) => {
			let _ = ready.send(Err(io::Error::other(error)));
			return;
		},
	};
	let mut render = Render::new(shared.clone());
	let stream = match Stream::open(event).and_then(|mut stream| {
		stream.start(&mut render)?;
		Ok(stream)
	}) {
		Ok(stream) => stream,
		Err(error) => {
			let _ = ready.send(Err(io::Error::other(error)));
			return;
		},
	};
	// Do not retain a failed starter's thread in Shared: its feed may be tried again.
	shared.control.get_or_init(thread::current);
	shared.running.store(true, Ordering::Release);
	let _ = ready.send(Ok(()));
	run(shared, event, &mut render, stream);
	shared.running.store(false, Ordering::Release);
}

fn run(shared: &Shared, event: &Event, render: &mut Render, stream: Stream) {
	let mut stream = Some(stream);
	while !shared.stop.load(Ordering::Acquire) {
		// Stop only if still idle; PCM may have arrived since Render asked.
		if shared.want_stop.swap(false, Ordering::AcqRel) && shared.ring.len() == 0 {
			shared.running.store(false, Ordering::Release);
			stream.take();
			continue;
		}
		// Clearing before the ring check leaves a wake-up token if PCM races with park.
		shared.want_start.store(false, Ordering::Release);
		if stream.is_none() {
			if shared.ring.len() == 0 {
				thread::park();
				continue;
			}
			match Stream::open(event).and_then(|mut stream| {
				stream.start(render)?;
				Ok(stream)
			}) {
				Ok(next) => {
					stream = Some(next);
					shared.running.store(true, Ordering::Release);
				},
				Err(_) => {
					thread::park_timeout(REOPEN_AFTER);
					continue;
				},
			}
		}
		// SAFETY: the event outlives this thread; only this thread waits on it.
		let signaled = unsafe { WaitForSingleObject(event.0, EVENT_TIMEOUT_MS) } == WAIT_OBJECT_0;
		if shared.stop.load(Ordering::Acquire) {
			break;
		}
		if !signaled || stream.as_mut().is_some_and(|s| s.fill(render).is_err()) {
			// Invalidation (unplug, audio service restart) or a stalled event: reopen the
			// current default without stopping the decoder or dropping its queued PCM.
			shared.running.store(false, Ordering::Release);
			stream.take();
		}
	}
}

struct Stream {
	render:    IAudioRenderClient,
	client:    IAudioClient,
	frames:    u32,
	resampler: Resampler,
}

impl Stream {
	fn open(event: &Event) -> windows::core::Result<Self> {
		// SAFETY: COM is initialized on this thread; interfaces and allocations are owned here.
		let client: IAudioClient = unsafe {
			let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
			enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?.Activate(CLSCTX_ALL, None)?
		};
		// SAFETY: GetMixFormat returns an allocated, initialized WAVEFORMATEX on success.
		let rate = unsafe {
			let mix = client.GetMixFormat()?;
			let rate = (*mix).nSamplesPerSec;
			CoTaskMemFree(Some(mix.cast()));
			rate
		};
		let format = WAVEFORMATEX {
			wFormatTag:      3, // WAVE_FORMAT_IEEE_FLOAT.
			nChannels:       2,
			nSamplesPerSec:  rate,
			nAvgBytesPerSec: rate.saturating_mul(8),
			nBlockAlign:     8,
			wBitsPerSample:  32,
			cbSize:          0,
		};
		// Shared-mode conversion handles the endpoint's channel layout and sample format;
		// our resampler supplies stereo at its mix rate, so no second rate conversion is needed.
		let flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
		// SAFETY: the format lives through Initialize; the event outlives the client. All
		// following calls use the initialized client on the same MTA thread.
		unsafe {
			client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, BUFFER_DURATION, 0, &raw const format, None)?;
			client.SetEventHandle(event.0)?;
			let frames = client.GetBufferSize()?;
			let render = client.GetService()?;
			Ok(Self { client, render, frames, resampler: Resampler::new(super::eq::SAMPLE_RATE, rate) })
		}
	}

	fn start(&mut self, render: &mut Render) -> windows::core::Result<()> {
		// Prime before Start: the first engine pass must not underrun.
		self.fill(render)?;
		// SAFETY: this client is initialized and primed, and belongs to this thread.
		unsafe { self.client.Start() }
	}

	fn fill(&mut self, render: &mut Render) -> windows::core::Result<()> {
		// SAFETY: this thread is the initialized client's sole user.
		let frames = self.frames.saturating_sub(unsafe { self.client.GetCurrentPadding()? });
		if frames == 0 {
			return Ok(());
		}
		// SAFETY: GetBuffer provides `frames` writable, interleaved stereo f32 frames,
		// valid until the matching ReleaseBuffer below, on this same thread.
		let samples = unsafe {
			let data = self.render.GetBuffer(frames)?;
			slice::from_raw_parts_mut(data.cast::<f32>(), usize::try_from(frames).unwrap_or(0) * 2)
		};
		for chunk in samples.chunks_mut(MAX_OUT_FRAMES * 2) {
			self.resampler.process(chunk, &mut |input| {
				render.render(input);
			});
		}
		// SAFETY: every requested frame was filled, and the buffer has not been released yet.
		unsafe { self.render.ReleaseBuffer(frames, 0) }
	}
}

impl Drop for Stream {
	fn drop(&mut self) {
		// SAFETY: the client stays on its MTA thread; stopping precedes releasing its interfaces.
		let _ = unsafe { self.client.Stop() };
	}
}
