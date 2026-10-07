//! macOS output straight to a CoreAudio device (raw FFI, no crates, no AudioUnit).
//!
//! An IOProc on the device (the system's default output, or the one the user picked) asks for
//! audio at the device's own rate and layout; [`IoState`] renders the player's 44.1 kHz stereo,
//! converts the rate ([`super::resample`]) and spreads it over the device's channels. Skipping the
//! AudioUnit layer skips AudioToolbox's component registry and converter (~1 MB measured). The
//! list of output devices, the system default and a watcher for devices coming and going (and
//! the device's rate changing) live here too.

use std::{
	ffi::{c_char, c_void},
	io, ptr, slice,
	sync::{
		Arc, Mutex, OnceLock, PoisonError,
		atomic::{AtomicBool, Ordering},
	},
	thread::{self, JoinHandle},
};

use super::{DeviceControl, OutputDevice, Render, Shared, resample::Resampler};

pub(super) const CAN_PICK_OUTPUT: bool = true;

#[derive(Default)]
pub(super) struct State {
	/// Swapped while holding the lock, so the control thread cannot touch a disposed IOProc.
	unit: Mutex<Option<Unit>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
	mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Owns the device and its control thread, including device changes and idle start/stop.
pub(super) struct Output {
	shared:  Arc<Shared>,
	out:     Option<Stream>,
	control: Option<JoinHandle<()>>,
	device:  Option<String>,
}

impl Output {
	pub(super) fn start(shared: Arc<Shared>, device: Option<&str>) -> io::Result<Self> {
		let (out, device) = open_output(&shared, device)?;
		let control = {
			let shared = shared.clone();
			thread::Builder::new().name("turntable-audio-ctl".into()).stack_size(64 * 1024).spawn(move || control_output(&shared))
		};
		let control = match control {
			Ok(control) => control,
			Err(error) => {
				*lock(&shared._output.unit) = None;
				drop(out);
				return Err(error);
			},
		};
		let _ = shared.control.set(control.thread().clone());
		Ok(Self { shared, out: Some(out), control: Some(control), device })
	}
}

impl DeviceControl for Output {
	fn device(&self) -> Option<&str> {
		self.device.as_deref()
	}

	fn has_output(&self) -> bool {
		self.out.is_some()
	}

	fn set_device(&mut self, uid: Option<&str>) -> io::Result<()> {
		if self.out.as_ref().is_some_and(|out| !out.is_stale()) && self.device.as_deref() == uid {
			return Ok(());
		}
		{
			let mut unit = lock(&self.shared._output.unit);
			*unit = None;
			// Disposing the device waits for its last render callback.
			drop(self.out.take());
		}
		let (out, device) = open_output(&self.shared, uid)?;
		self.out = Some(out);
		self.device = device;
		Ok(())
	}
}

impl Drop for Output {
	fn drop(&mut self) {
		self.shared.stop.store(true, Ordering::Release);
		if let Some(writer) = self.shared.writer.get() {
			writer.unpark();
		}
		// The control thread may be calling into the device: finish it before disposal.
		if let Some(control) = self.control.take() {
			control.thread().unpark();
			let _ = control.join();
		}
		*lock(&self.shared._output.unit) = None;
		self.out.take();
	}
}

/// A disconnected or unopenable selection falls back to the system default.
fn open_output(shared: &Arc<Shared>, device: Option<&str>) -> io::Result<(Stream, Option<String>)> {
	if let Some(uid) = device
		&& let Ok(out) = Stream::start(Render::new(shared.clone()), Some(uid))
	{
		return Ok((attach(shared, out), Some(uid.to_owned())));
	}
	let out = Stream::start(Render::new(shared.clone()), None)?;
	Ok((attach(shared, out), None))
}

fn attach(shared: &Shared, out: Stream) -> Stream {
	*lock(&shared._output.unit) = Some(out.unit());
	shared.running.store(true, Ordering::Release);
	out
}

fn start_unit(shared: &Shared) {
	if let Some(unit) = *lock(&shared._output.unit) {
		unit.start();
	}
}

fn stop_unit(shared: &Shared) {
	if let Some(unit) = *lock(&shared._output.unit) {
		unit.stop();
	}
}

/// Only this thread starts/stops devices; neither the decoder nor real-time render does.
fn control_output(shared: &Shared) {
	loop {
		thread::park();
		if shared.stop.load(Ordering::Acquire) {
			return;
		}
		if shared.want_start.swap(false, Ordering::AcqRel) && !shared.running.load(Ordering::Acquire) {
			shared.running.store(true, Ordering::Release);
			start_unit(shared);
		}
		if shared.want_stop.swap(false, Ordering::AcqRel) && shared.ring.len() == 0 && shared.running.swap(false, Ordering::AcqRel) {
			stop_unit(shared);
			// PCM that landed during stop must not wait for the next push.
			if shared.ring.len() > 0 {
				shared.running.store(true, Ordering::Release);
				start_unit(shared);
			}
		}
	}
}

type OSStatus = i32;

#[repr(C)]
struct AudioBuffer {
	number_channels: u32,
	data_byte_size:  u32,
	data:            *mut c_void,
}

#[repr(C)]
struct AudioBufferList {
	number_buffers: u32,
	buffers:        [AudioBuffer; 1],
}

#[repr(C)]
struct AudioObjectPropertyAddress {
	selector: u32,
	scope:    u32,
	element:  u32,
}

type ListenerProc = extern "C" fn(object: u32, count: u32, addresses: *const AudioObjectPropertyAddress, client: *mut c_void) -> OSStatus;

type IoProc = extern "C" fn(
	device: u32,
	now: *const c_void,
	input: *const AudioBufferList,
	input_time: *const c_void,
	output: *mut AudioBufferList,
	output_time: *const c_void,
	client: *mut c_void,
) -> OSStatus;

type IoProcId = *mut c_void;

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
	fn AudioObjectGetPropertyDataSize(
		object: u32,
		address: *const AudioObjectPropertyAddress,
		qualifier_size: u32,
		qualifier: *const c_void,
		size: *mut u32,
	) -> OSStatus;
	fn AudioObjectGetPropertyData(
		object: u32,
		address: *const AudioObjectPropertyAddress,
		qualifier_size: u32,
		qualifier: *const c_void,
		size: *mut u32,
		data: *mut c_void,
	) -> OSStatus;
	fn AudioObjectSetPropertyData(
		object: u32,
		address: *const AudioObjectPropertyAddress,
		qualifier_size: u32,
		qualifier: *const c_void,
		size: u32,
		data: *const c_void,
	) -> OSStatus;
	fn AudioObjectAddPropertyListener(
		object: u32,
		address: *const AudioObjectPropertyAddress,
		listener: ListenerProc,
		client: *mut c_void,
	) -> OSStatus;
	fn AudioObjectRemovePropertyListener(
		object: u32,
		address: *const AudioObjectPropertyAddress,
		listener: ListenerProc,
		client: *mut c_void,
	) -> OSStatus;
	fn AudioDeviceCreateIOProcID(device: u32, proc_: IoProc, client: *mut c_void, id: *mut IoProcId) -> OSStatus;
	fn AudioDeviceDestroyIOProcID(device: u32, id: IoProcId) -> OSStatus;
	fn AudioDeviceStart(device: u32, id: IoProcId) -> OSStatus;
	fn AudioDeviceStop(device: u32, id: IoProcId) -> OSStatus;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
	fn CFStringGetLength(string: *const c_void) -> isize;
	fn CFStringGetMaximumSizeForEncoding(length: isize, encoding: u32) -> isize;
	fn CFStringGetCString(string: *const c_void, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
	fn CFRelease(object: *const c_void);
}

const fn fourcc(s: [u8; 4]) -> u32 {
	u32::from_be_bytes(s)
}

/// `kAudioDevicePropertyBufferFrameSize`: this process's IO buffer on the device, in frames.
const K_PROP_BUFFER_FRAME_SIZE: u32 = fourcc(*b"fsiz");
/// `kAudioDevicePropertyNominalSampleRate`.
const K_PROP_NOMINAL_RATE: u32 = fourcc(*b"nsrt");
/// The IO buffer asked for: ~43 ms at 48 kHz, so the device's IO thread (and the render
/// callback) wakes ~23 times a second instead of the default 512 frames' ~94. Music doesn't
/// need the low latency; the per-process setting leaves other apps' buffers alone.
const IO_BUFFER_FRAMES: u32 = 2048;
/// The most frames one IO call is rendered in at once (a device asking for more gets several).
const MAX_FRAMES: usize = super::resample::MAX_OUT_FRAMES;

/// The HAL's system object, which knows the devices.
const SYSTEM_OBJECT: u32 = 1;
const K_ELEMENT_MAIN: u32 = 0;
const K_AUDIO_SCOPE_GLOBAL: u32 = fourcc(*b"glob");
const K_AUDIO_SCOPE_OUTPUT: u32 = fourcc(*b"outp");
const K_AUDIO_SCOPE_INPUT: u32 = fourcc(*b"inpt");
/// `kAudioDevicePropertyIOProcStreamUsage`: which of a device's streams an IOProc uses.
const K_PROP_IOPROC_STREAM_USAGE: u32 = fourcc(*b"suse");
/// `kAudioHardwarePropertyDevices`: every device.
const K_PROP_DEVICES: u32 = fourcc(*b"dev#");
/// `kAudioHardwarePropertyDefaultOutputDevice`.
const K_PROP_DEFAULT_OUTPUT: u32 = fourcc(*b"dOut");
/// `kAudioDevicePropertyDeviceUID`.
const K_PROP_UID: u32 = fourcc(*b"uid ");
/// `kAudioObjectPropertyName`.
const K_PROP_NAME: u32 = fourcc(*b"lnam");
/// `kAudioDevicePropertyStreams`: a device with output streams plays sound.
const K_PROP_STREAMS: u32 = fourcc(*b"stm#");
/// `kCFStringEncodingUTF8`.
const CF_UTF8: u32 = 0x0800_0100;

fn check(what: &str, status: OSStatus) -> io::Result<()> {
	if status == 0 { Ok(()) } else { Err(io::Error::other(format!("{what} failed (OSStatus {status})"))) }
}

/// The byte size of a `T` as CoreAudio counts it.
fn size_of_u32<T>() -> u32 {
	u32::try_from(size_of::<T>()).unwrap_or(u32::MAX)
}

/// What the IO thread owns: the render chain, the rate converter and a scratch buffer.
struct IoState {
	render:    Render,
	resampler: Resampler,
	/// Interleaved stereo at the device's rate, before it is spread over the device's channels.
	stereo:    Vec<f32>,
}

/// Audio playing on one device, until dropped.
struct Stream {
	device:          u32,
	proc_:           IoProcId,
	state:           *mut IoState,
	/// Opened on the system's default device (so it must move when the default does).
	follows_default: bool,
	/// The device's rate changed under it (set by a listener): it must be opened again.
	stale:           Arc<AtomicBool>,
}

// SAFETY: the device and IOProc ids are only used from the owner (start/stop/destroy, thread safe
// in CoreAudio); `state` is touched solely by the IO thread until the IOProc is destroyed.
unsafe impl Send for Stream {}

impl Stream {
	/// Starts playing `render`'s audio on the device with this UID, or on the system's default
	/// output when there is none.
	fn start(render: Render, device: Option<&str>) -> io::Result<Self> {
		let id = match device {
			Some(uid) => device_id(uid).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "that output device is not connected"))?,
			None => default_device().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "there is no output device"))?,
		};
		// A device that can't take this buffer size clamps or refuses it; either way it plays.
		let frames = IO_BUFFER_FRAMES;
		let addr = address(K_PROP_BUFFER_FRAME_SIZE, K_AUDIO_SCOPE_GLOBAL);
		// SAFETY: `addr` and `frames` outlive the call, which copies 4 bytes.
		let _ =
			unsafe { AudioObjectSetPropertyData(id, &raw const addr, 0, ptr::null(), size_of_u32::<u32>(), (&raw const frames).cast()) };
		let rate = nominal_rate(id).ok_or_else(|| io::Error::other("the output device has no sample rate"))?;
		let state = Box::into_raw(Box::new(IoState {
			render,
			resampler: Resampler::new(super::eq::SAMPLE_RATE, rate),
			stereo: vec![0.0; MAX_FRAMES * 2],
		}));
		let stale = Arc::new(AtomicBool::new(false));
		// On error `out` drops and releases everything acquired so far.
		let mut out = Self { device: id, proc_: ptr::null_mut(), state, follows_default: device.is_none(), stale };
		// SAFETY: `state` stays allocated until `Stream` drops, after the IOProc is destroyed.
		check("create the IOProc", unsafe { AudioDeviceCreateIOProcID(id, io_proc, state.cast(), &raw mut out.proc_) })?;
		// A headset is one device with a microphone too: without this the IOProc would record from
		// it (and macOS would show the microphone in use).
		disable_input(id, out.proc_);
		watch_rate(id, &out.stale);
		// SAFETY: the IOProc was created on this device above.
		check("start the device", unsafe { AudioDeviceStart(id, out.proc_) })?;
		Ok(out)
	}

	/// Whether it must be opened again: the device's rate changed, or the system's default moved
	/// to another device while this one plays the default.
	pub fn is_stale(&self) -> bool {
		self.stale.load(Ordering::Acquire) || (self.follows_default && default_device() != Some(self.device))
	}

	pub fn unit(&self) -> Unit {
		Unit { device: self.device, proc_: self.proc_ }
	}
}

/// The running IOProc, for starting and stopping it from the audio control thread (CoreAudio's
/// start/stop are thread safe). Only valid while its `Stream` lives; the output drops it from the
/// control thread's reach first.
#[derive(Clone, Copy)]
struct Unit {
	device: u32,
	proc_:  IoProcId,
}

// SAFETY: see above; the ids are CoreAudio object references.
unsafe impl Send for Unit {}

impl Unit {
	/// Resumes IO calls.
	pub fn start(self) {
		// SAFETY: the IOProc is alive (see the type's docs).
		unsafe { AudioDeviceStart(self.device, self.proc_) };
	}

	/// Stops IO calls (and the device's IO, if nothing else plays); waits for one in flight.
	pub fn stop(self) {
		// SAFETY: as in `start`.
		unsafe { AudioDeviceStop(self.device, self.proc_) };
	}
}

impl Drop for Stream {
	fn drop(&mut self) {
		unwatch_rate(self.device, &self.stale);
		if !self.proc_.is_null() {
			// SAFETY: the IOProc is ours. Stopping waits for an IO call in flight to finish.
			unsafe {
				AudioDeviceStop(self.device, self.proc_);
				AudioDeviceDestroyIOProcID(self.device, self.proc_);
			}
		}
		// SAFETY: made by `Box::into_raw` in `start`; no IO call can run any more.
		drop(unsafe { Box::from_raw(self.state) });
	}
}

extern "C" fn io_proc(
	_device: u32,
	_now: *const c_void,
	_input: *const AudioBufferList,
	_input_time: *const c_void,
	output: *mut AudioBufferList,
	_output_time: *const c_void,
	client: *mut c_void,
) -> OSStatus {
	// SAFETY: `client` is the `IoState` owned by `Stream`, alive until the IOProc is destroyed; the
	// HAL serialises IO calls on one thread.
	let state = unsafe { &mut *client.cast::<IoState>() };
	if output.is_null() {
		return 0;
	}
	// SAFETY: the HAL hands a valid buffer list for the duration of the call.
	let list = unsafe { &mut *output };
	let count = usize::try_from(list.number_buffers).unwrap_or(0);
	// SAFETY: the list declares `number_buffers` buffers back to back from its first.
	let buffers = unsafe { slice::from_raw_parts_mut(list.buffers.as_mut_ptr(), count) };
	let Some(first) = buffers.first() else { return 0 };
	let channels = usize::try_from(first.number_channels).unwrap_or(0).max(1);
	let mut frames = usize::try_from(first.data_byte_size).unwrap_or(0) / 4 / channels;
	let mut done = 0;
	while frames > 0 {
		let n = frames.min(MAX_FRAMES);
		let Some(stereo) = state.stereo.get_mut(..n * 2) else { break };
		let IoState { render, resampler, .. } = state;
		resampler.process(stereo, &mut |chunk: &mut [f32]| {
			render.render(chunk);
		});
		spread(stereo, buffers, done);
		done += n;
		frames -= n;
	}
	0
}

/// Writes interleaved stereo `stereo` into the device's buffers from frame `at`: left and right
/// to the first two channels (whether one buffer holds them all or each its own), silence to the
/// rest, and a mono device gets the mix.
fn spread(stereo: &[f32], buffers: &mut [AudioBuffer], at: usize) {
	let frames = stereo.len() / 2;
	let total: usize = buffers.iter().map(|b| usize::try_from(b.number_channels).unwrap_or(0)).sum();
	let mono = total == 1;
	let mut channel = 0usize;
	for b in buffers.iter_mut() {
		if b.data.is_null() {
			continue;
		}
		let width = usize::try_from(b.number_channels).unwrap_or(0).max(1);
		let len = usize::try_from(b.data_byte_size).unwrap_or(0) / 4;
		// SAFETY: the buffer holds `data_byte_size` bytes of f32 samples.
		let data = unsafe { slice::from_raw_parts_mut(b.data.cast::<f32>(), len) };
		let Some(region) = data.get_mut(at * width..(at + frames) * width) else { continue };
		for (out, &[l, r]) in region.chunks_exact_mut(width).zip(stereo.as_chunks::<2>().0) {
			for (c, sample) in out.iter_mut().enumerate() {
				*sample = match channel + c {
					_ if mono => f32::midpoint(l, r),
					0 => l,
					1 => r,
					_ => 0.0,
				};
			}
		}
		channel += width;
	}
}

// ---------------------------------------------------------------------- devices

fn address(selector: u32, scope: u32) -> AudioObjectPropertyAddress {
	AudioObjectPropertyAddress { selector, scope, element: K_ELEMENT_MAIN }
}

/// A 4-byte property of an audio object.
fn read_u32(object: u32, selector: u32, scope: u32) -> Option<u32> {
	let addr = address(selector, scope);
	let mut value = 0u32;
	let mut size = size_of_u32::<u32>();
	// SAFETY: `addr`, `size` and `value` outlive the call; CoreAudio writes at most `size` bytes to `value`.
	let status = unsafe { AudioObjectGetPropertyData(object, &raw const addr, 0, ptr::null(), &raw mut size, (&raw mut value).cast()) };
	(status == 0).then_some(value)
}

/// Every audio device the HAL knows, outputs or not.
fn device_ids() -> Vec<u32> {
	let addr = address(K_PROP_DEVICES, K_AUDIO_SCOPE_GLOBAL);
	let mut size = 0u32;
	// SAFETY: `addr` and `size` outlive the call.
	if unsafe { AudioObjectGetPropertyDataSize(SYSTEM_OBJECT, &raw const addr, 0, ptr::null(), &raw mut size) } != 0 {
		return Vec::new();
	}
	let mut ids = vec![0u32; usize::try_from(size).unwrap_or(0) / 4];
	let mut size = u32::try_from(ids.len() * 4).unwrap_or(0);
	// SAFETY: `ids` has room for `size` bytes, which is all CoreAudio writes.
	let status =
		unsafe { AudioObjectGetPropertyData(SYSTEM_OBJECT, &raw const addr, 0, ptr::null(), &raw mut size, ids.as_mut_ptr().cast()) };
	if status != 0 {
		return Vec::new();
	}
	ids.truncate(usize::try_from(size).unwrap_or(0) / 4);
	ids
}

/// Whether `device` has output streams (a microphone doesn't).
fn has_output(device: u32) -> bool {
	let addr = address(K_PROP_STREAMS, K_AUDIO_SCOPE_OUTPUT);
	let mut size = 0u32;
	// SAFETY: `addr` and `size` outlive the call.
	let status = unsafe { AudioObjectGetPropertyDataSize(device, &raw const addr, 0, ptr::null(), &raw mut size) };
	status == 0 && size > 0
}

/// A string property (a retained CFString that this releases).
fn string_property(object: u32, selector: u32) -> Option<String> {
	let addr = address(selector, K_AUDIO_SCOPE_GLOBAL);
	let mut string: *const c_void = ptr::null();
	let mut size = size_of_u32::<*const c_void>();
	// SAFETY: `addr`, `size` and `string` outlive the call; CoreAudio writes one pointer to `string`.
	let status = unsafe { AudioObjectGetPropertyData(object, &raw const addr, 0, ptr::null(), &raw mut size, (&raw mut string).cast()) };
	if status != 0 || string.is_null() {
		return None;
	}
	let text = cf_string(string);
	// SAFETY: the property handed over a retained CFString; this is the one release.
	unsafe { CFRelease(string) };
	text
}

fn cf_string(string: *const c_void) -> Option<String> {
	// SAFETY: `string` is a live CFString.
	let length = unsafe { CFStringGetLength(string) };
	// SAFETY: plain arithmetic on a length.
	let most = unsafe { CFStringGetMaximumSizeForEncoding(length, CF_UTF8) }.checked_add(1)?;
	let mut buf = vec![0u8; usize::try_from(most).ok()?];
	// SAFETY: `buf` has `most` bytes, which the call is told; it writes a terminated string within them.
	let ok = unsafe { CFStringGetCString(string, buf.as_mut_ptr().cast(), most, CF_UTF8) };
	if ok == 0 {
		return None;
	}
	buf.truncate(buf.iter().position(|&b| b == 0)?);
	String::from_utf8(buf).ok()
}

/// The devices that can play sound.
pub fn devices() -> Vec<OutputDevice> {
	device_ids()
		.into_iter()
		.filter(|&id| has_output(id))
		.filter_map(|id| Some(OutputDevice { uid: string_property(id, K_PROP_UID)?, name: string_property(id, K_PROP_NAME)? }))
		.collect()
}

/// The HAL's id of the device with this UID, if it is connected.
fn device_id(uid: &str) -> Option<u32> {
	device_ids().into_iter().find(|&id| has_output(id) && string_property(id, K_PROP_UID).as_deref() == Some(uid))
}

/// The system's default output device.
fn default_device() -> Option<u32> {
	read_u32(SYSTEM_OBJECT, K_PROP_DEFAULT_OUTPUT, K_AUDIO_SCOPE_GLOBAL).filter(|&id| id != 0)
}

/// The name of the system's default output device.
pub fn default_device_name() -> Option<String> {
	string_property(default_device()?, K_PROP_NAME)
}

/// Turns off the input streams of `device` for the IOProc `proc_`, so only output runs.
fn disable_input(device: u32, proc_: IoProcId) {
	let addr = address(K_PROP_IOPROC_STREAM_USAGE, K_AUDIO_SCOPE_INPUT);
	let mut size = 0u32;
	// SAFETY: `addr` and `size` outlive the call.
	if unsafe { AudioObjectGetPropertyDataSize(device, &raw const addr, 0, ptr::null(), &raw mut size) } != 0 {
		return;
	}
	// `AudioHardwareIOProcStreamUsage`: the IOProc id, a stream count, then one u32 flag per stream.
	// u64 words keep the buffer aligned for the pointer at its head.
	let mut usage = vec![0u64; usize::try_from(size).unwrap_or(0).div_ceil(8).max(2)];
	let Some(head) = usage.first_mut() else { return };
	*head = proc_ as u64;
	// SAFETY: `usage` has room for `size` bytes and starts with the IOProc id the query names.
	let status = unsafe { AudioObjectGetPropertyData(device, &raw const addr, 0, ptr::null(), &raw mut size, usage.as_mut_ptr().cast()) };
	if status != 0 {
		return;
	}
	// The flags start after the id (8 bytes) and the count (4 bytes).
	let bytes = usize::try_from(size).unwrap_or(0);
	// SAFETY: viewing the buffer's first `size` bytes, which the call filled.
	let raw = unsafe { slice::from_raw_parts_mut(usage.as_mut_ptr().cast::<u8>(), bytes) };
	if let Some(flags) = raw.get_mut(12..) {
		flags.fill(0);
	}
	// SAFETY: the same layout, now with every input stream off, `size` bytes long.
	let _ = unsafe { AudioObjectSetPropertyData(device, &raw const addr, 0, ptr::null(), size, usage.as_ptr().cast()) };
}

/// The device's sample rate, in whole hertz.
fn nominal_rate(device: u32) -> Option<u32> {
	let addr = address(K_PROP_NOMINAL_RATE, K_AUDIO_SCOPE_GLOBAL);
	let mut rate = 0f64;
	let mut size = size_of_u32::<f64>();
	// SAFETY: `addr`, `size` and `rate` outlive the call; CoreAudio writes one f64 to `rate`.
	let status = unsafe { AudioObjectGetPropertyData(device, &raw const addr, 0, ptr::null(), &raw mut size, (&raw mut rate).cast()) };
	#[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "audio rates are positive and far below u32::MAX")]
	let hz = rate.round() as u32;
	(status == 0 && hz > 0).then_some(hz)
}

extern "C" fn rate_changed(_object: u32, _count: u32, _addresses: *const AudioObjectPropertyAddress, client: *mut c_void) -> OSStatus {
	// SAFETY: `client` is the `AtomicBool` inside the `Arc` that `watch_rate` registered; the
	// registration holds one strong count, released by `unwatch_rate`.
	let stale = unsafe { &*client.cast::<AtomicBool>() };
	stale.store(true, Ordering::Release);
	// The app looks at its output again (as when devices come and go).
	if let Some(notify) = NOTIFY.get() {
		notify();
	}
	0
}

/// Marks `stale` when `device`'s rate changes.
fn watch_rate(device: u32, stale: &Arc<AtomicBool>) {
	let addr = address(K_PROP_NOMINAL_RATE, K_AUDIO_SCOPE_GLOBAL);
	let client = Arc::into_raw(stale.clone()).cast_mut();
	// SAFETY: `addr` outlives the call; `client` stays valid until `unwatch_rate` takes it back.
	if unsafe { AudioObjectAddPropertyListener(device, &raw const addr, rate_changed, client.cast()) } != 0 {
		// SAFETY: not registered: take the count back at once.
		drop(unsafe { Arc::from_raw(client) });
	}
}

fn unwatch_rate(device: u32, stale: &Arc<AtomicBool>) {
	let addr = address(K_PROP_NOMINAL_RATE, K_AUDIO_SCOPE_GLOBAL);
	let client = Arc::as_ptr(stale).cast_mut();
	// SAFETY: removes the registration `watch_rate` made with this pointer; its count is
	// released only when the removal succeeded (then the listener can't run any more).
	if unsafe { AudioObjectRemovePropertyListener(device, &raw const addr, rate_changed, client.cast()) } == 0 {
		// SAFETY: the count `watch_rate` leaked for the registration.
		drop(unsafe { Arc::from_raw(client) });
	}
}

/// What a watcher tells: devices came or went, or the default changed.
type Notify = Box<dyn Fn() + Send + Sync>;

/// Listens for devices coming and going and for the default changing, until dropped.
pub struct Watch {
	client: *mut Notify,
}

// SAFETY: the pointer is only dereferenced by the listener, which treats it as a shared reference
// to a `Send + Sync` closure.
unsafe impl Send for Watch {}

const WATCHED: [u32; 2] = [K_PROP_DEVICES, K_PROP_DEFAULT_OUTPUT];

extern "C" fn listener(_object: u32, _count: u32, _addresses: *const AudioObjectPropertyAddress, client: *mut c_void) -> OSStatus {
	// SAFETY: `client` is the leaked `Notify` that `watch` registered; it is never freed.
	let notify = unsafe { &*client.cast::<Notify>() };
	notify();
	0
}

/// The app's watcher, also told when a playing device's rate changes.
static NOTIFY: OnceLock<Arc<Notify>> = OnceLock::new();

/// Starts watching; `notify` runs on a CoreAudio thread, so it must not block.
pub fn watch(notify: impl Fn() + Send + Sync + 'static) -> Option<Watch> {
	let notify: Arc<Notify> = Arc::new(Box::new(notify));
	let _ = NOTIFY.set(notify.clone());
	// The closure is leaked on purpose: CoreAudio may run the listener once more while it is being
	// removed, and a few bytes per watch (one per run) are cheaper than that race.
	let forward: Notify = Box::new(move || notify());
	let client = Box::into_raw(Box::new(forward));
	for selector in WATCHED {
		let addr = address(selector, K_AUDIO_SCOPE_GLOBAL);
		// SAFETY: `addr` outlives the call; `client` is never freed.
		let status = unsafe { AudioObjectAddPropertyListener(SYSTEM_OBJECT, &raw const addr, listener, client.cast()) };
		if status != 0 {
			return None;
		}
	}
	Some(Watch { client })
}

impl Drop for Watch {
	fn drop(&mut self) {
		for selector in WATCHED {
			let addr = address(selector, K_AUDIO_SCOPE_GLOBAL);
			// SAFETY: removes the registration made in `watch`; `client` is never freed.
			unsafe { AudioObjectRemovePropertyListener(SYSTEM_OBJECT, &raw const addr, listener, self.client.cast()) };
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn an_unknown_device_cannot_be_opened() {
		let shared = Arc::new(Shared::new());
		let err = Stream::start(Render::new(shared), Some("no-such-device-uid")).err().map(|e| e.kind());
		assert_eq!(err, Some(io::ErrorKind::NotFound));
	}
}
