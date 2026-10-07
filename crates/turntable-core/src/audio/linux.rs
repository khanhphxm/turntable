//! Linux output through a system audio utility, with no linked audio libraries.
//!
//! One render/control thread writes 2048-frame blocks to a blocking pipe. The audio
//! server's reads pace it; it parks and kills the child after Render requests an idle
//! stop, and spawns the same utility again when PCM arrives. Drop kills the child
//! before joining, so even a blocked pipe write cannot hold up shutdown.

use std::{
	io::{self, Write},
	process::{Child, ChildStdin, Command, Stdio},
	slice,
	sync::{Arc, Mutex, PoisonError, atomic::Ordering},
	thread::{self, JoinHandle},
};

use super::{Render, Shared};

#[path = "system_default.rs"]
mod picker;

pub use picker::Watch;
pub(super) use picker::{CAN_PICK_OUTPUT, State, default_device_name, devices, watch};

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
	mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

const FRAMES: usize = 2048;
const PLAYERS: [(&str, &[&str]); 3] = [
	("pw-cat", &["--playback", "--raw", "--format", "f32", "--rate", "44100", "--channels", "2", "--latency", "100ms", "-"]),
	("pacat", &["--raw", "--format=float32le", "--rate=44100", "--channels=2"]),
	("aplay", &["-t", "raw", "-f", "FLOAT_LE", "-r", "44100", "-c", "2"]),
];

fn commands() -> impl Iterator<Item = Command> {
	PLAYERS.iter().map(|(program, args)| {
		let mut command = Command::new(program);
		command.args(*args);
		command
	})
}

/// Tries another utility only when the executable is missing, not on other launch errors.
fn select(commands: impl IntoIterator<Item = Command>) -> io::Result<(Command, Child, ChildStdin)> {
	for mut command in commands {
		command.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
		match spawn(&mut command) {
			Ok((child, input)) => return Ok((command, child, input)),
			Err(error) if error.kind() == io::ErrorKind::NotFound => {},
			Err(error) => return Err(error),
		}
	}
	Err(io::Error::new(io::ErrorKind::NotFound, "install pw-cat (pipewire-bin), pacat (pulseaudio-utils) or aplay (alsa-utils)"))
}

fn spawn(command: &mut Command) -> io::Result<(Child, ChildStdin)> {
	let mut child = command.spawn()?;
	let Some(input) = child.stdin.take() else {
		let _ = child.kill();
		let _ = child.wait();
		return Err(io::Error::other("the audio utility has no stdin pipe"));
	};
	Ok((child, input))
}

/// The child handle is shared only for shutdown; pipe writes never hold this mutex.
pub(super) struct Output {
	shared: Arc<Shared>,
	child:  Arc<Mutex<Option<Child>>>,
	worker: Option<JoinHandle<()>>,
}

impl Output {
	pub(super) fn start(shared: Arc<Shared>, _device: Option<&str>) -> io::Result<Self> {
		Self::start_with(shared, commands())
	}

	fn start_with(shared: Arc<Shared>, commands: impl IntoIterator<Item = Command>) -> io::Result<Self> {
		let (command, child, input) = select(commands)?;
		let child = Arc::new(Mutex::new(Some(child)));
		let worker = {
			let shared = shared.clone();
			let child = child.clone();
			thread::Builder::new().name("turntable-audio".into()).spawn(move || {
				shared.control.get_or_init(thread::current);
				let mut render = Render::new(shared.clone());
				let _ = run(&shared, &child, &mut render, command, input);
				// An exited utility must not leave the decoder blocked on a full ring.
				shared.stop.store(true, Ordering::Release);
				shared.running.store(false, Ordering::Release);
				if let Some(writer) = shared.writer.get() {
					writer.unpark();
				}
				terminate(&child);
			})
		};
		match worker {
			Ok(worker) => Ok(Self { shared, child, worker: Some(worker) }),
			Err(error) => {
				// Keep the feed usable for the caller's next start attempt.
				terminate(&child);
				Err(error)
			},
		}
	}
}

impl super::DeviceControl for Output {}

fn terminate(child: &Mutex<Option<Child>>) {
	let child = lock(child).take();
	if let Some(mut child) = child {
		let _ = child.kill();
		let _ = child.wait();
	}
}

impl Drop for Output {
	fn drop(&mut self) {
		self.shared.stop.store(true, Ordering::Release);
		if let Some(writer) = self.shared.writer.get() {
			writer.unpark();
		}
		// Kill before joining: closing stdin on the writing thread cannot interrupt its write.
		terminate(&self.child);
		if let Some(worker) = self.worker.take() {
			worker.thread().unpark();
			let _ = worker.join();
		}
	}
}

fn run(shared: &Shared, child: &Mutex<Option<Child>>, render: &mut Render, mut command: Command, input: ChildStdin) -> io::Result<()> {
	let mut input = Some(input);
	let mut samples = [0.0; FRAMES * 2];
	while !shared.stop.load(Ordering::Acquire) {
		// Stop only if still idle; new PCM may have arrived since Render's request.
		if shared.want_stop.swap(false, Ordering::AcqRel) && shared.ring.len() == 0 {
			shared.running.store(false, Ordering::Release);
			input.take();
			terminate(child);
			continue;
		}
		// Clear before checking the ring, so a writer racing with park leaves a wake-up token.
		shared.want_start.store(false, Ordering::Release);
		if input.is_none() {
			if shared.ring.len() == 0 {
				thread::park();
				continue;
			}
			let mut current = lock(child);
			// Synchronize spawning with Drop's kill, so no child can appear after shutdown.
			if shared.stop.load(Ordering::Acquire) {
				break;
			}
			let (next, pipe) = spawn(&mut command)?;
			*current = Some(next);
			input = Some(pipe);
			shared.running.store(true, Ordering::Release);
		}
		render.render(&mut samples);
		if let Some(input) = &mut input {
			input.write_all(pcm_bytes(&mut samples))?;
		}
	}
	Ok(())
}

/// Borrows the render buffer directly on little-endian machines; no per-block copy/allocation.
fn pcm_bytes(samples: &mut [f32]) -> &[u8] {
	#[cfg(target_endian = "big")]
	for sample in samples.iter_mut() {
		*sample = f32::from_bits(sample.to_bits().swap_bytes());
	}
	// SAFETY: the live f32 slice is contiguous, every byte is initialized, and the borrowed
	// byte view has the same lifetime and exact size. On big-endian hosts its bytes were swapped.
	unsafe { slice::from_raw_parts(samples.as_ptr().cast(), size_of_val(samples)) }
}

#[cfg(test)]
mod tests {
	use std::{
		fs,
		path::PathBuf,
		sync::{atomic::AtomicU64, mpsc},
		time::{Duration, Instant},
	};

	use super::*;
	use crate::audio::{REBUFFER_SAMPLES, channel};

	struct Files(PathBuf);

	impl Files {
		fn new() -> Self {
			static SERIAL: AtomicU64 = AtomicU64::new(0);
			let path =
				std::env::temp_dir().join(format!("turntable-output-{}-{}", std::process::id(), SERIAL.fetch_add(1, Ordering::Relaxed)));
			fs::create_dir(&path).unwrap();
			Self(path)
		}
	}

	impl Drop for Files {
		fn drop(&mut self) {
			fs::remove_dir_all(&self.0).unwrap();
		}
	}

	fn fake(script: &str) -> Command {
		let mut command = Command::new("sh");
		command.args(["-c", script, "turntable-test"]);
		command
	}

	fn wait_for(mut ready: impl FnMut() -> bool) {
		let deadline = Instant::now() + Duration::from_secs(3);
		while !ready() {
			assert!(Instant::now() < deadline, "audio worker did not reach the expected state");
			thread::sleep(Duration::from_millis(5));
		}
	}

	#[test]
	fn utilities_receive_raw_stereo_float_arguments_in_preference_order() {
		let built: Vec<_> = commands().collect();
		assert_eq!(built.iter().map(|c| c.get_program()).collect::<Vec<_>>(), ["pw-cat", "pacat", "aplay"]);
		assert_eq!(built[0].get_args().collect::<Vec<_>>(), [
			"--playback",
			"--raw",
			"--format",
			"f32",
			"--rate",
			"44100",
			"--channels",
			"2",
			"--latency",
			"100ms",
			"-"
		]);
		assert_eq!(built[1].get_args().collect::<Vec<_>>(), ["--raw", "--format=float32le", "--rate=44100", "--channels=2"]);
		assert_eq!(built[2].get_args().collect::<Vec<_>>(), ["-t", "raw", "-f", "FLOAT_LE", "-r", "44100", "-c", "2"]);
	}

	#[test]
	fn missing_utilities_fall_back_but_other_spawn_errors_do_not() {
		let missing = || Command::new("/turntable-test/no-such-audio-utility");
		let (selected, mut child, input) = select([missing(), missing(), fake("exec cat > /dev/null")]).unwrap();
		assert_eq!(selected.get_program(), "sh");
		drop(input);
		assert!(child.wait().unwrap().success());
		assert_eq!(select([missing(), missing(), missing()]).unwrap_err().kind(), io::ErrorKind::NotFound);
		assert_eq!(select([Command::new("/dev/null"), fake("exec cat > /dev/null")]).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
	}

	#[test]
	fn samples_are_written_as_little_endian_float_bytes() {
		let mut samples = [0.0, 0.25, -0.5, 1.0];
		let expected: Vec<_> = samples.iter().flat_map(|s| f32::to_le_bytes(*s)).collect();
		assert_eq!(pcm_bytes(&mut samples), expected);
	}

	#[test]
	fn idle_kills_the_child_and_new_pcm_starts_it_again() {
		let files = Files::new();
		let starts = files.0.join("starts");
		let pcm = files.0.join("pcm");
		let mut command = fake("printf 'start\n' >> \"$1\"; exec cat >> \"$2\"");
		command.arg(&starts).arg(&pcm);
		let (writer, feed) = channel();
		let shared = feed.0;
		let output = Output::start_with(shared.clone(), [command]).unwrap();
		let idle = || !shared.running.load(Ordering::Acquire) && lock(&output.child).is_none();
		wait_for(idle);
		assert_eq!(fs::read_to_string(&starts).unwrap().lines().count(), 1);
		assert!(!output.worker.as_ref().unwrap().is_finished(), "idle worker parks instead of exiting");

		assert!(writer.write(&[0.25; REBUFFER_SAMPLES]));
		wait_for(|| fs::read_to_string(&starts).is_ok_and(|s| s.lines().count() == 2) && idle());
		let bytes = fs::read(pcm).unwrap();
		assert!(
			bytes.as_chunks::<4>().0.iter().any(|sample| sample == &0.25f32.to_le_bytes()),
			"the restarted child receives rendered PCM"
		);
		drop(output);
		assert!(!writer.write(&[0.0; 2]), "shutdown releases the decoder");
	}

	#[test]
	fn dropping_output_interrupts_a_blocked_pipe_write() {
		let (_writer, feed) = channel();
		let output = Output::start_with(feed.0, [fake("exec sleep 30")]).unwrap();
		// The child never reads: after a few blocks the pipe fills and render blocks in write.
		thread::sleep(Duration::from_millis(50));
		let (tx, rx) = mpsc::channel();
		let dropper = thread::spawn(move || {
			drop(output);
			tx.send(()).unwrap();
		});
		rx.recv_timeout(Duration::from_secs(3)).unwrap();
		dropper.join().unwrap();
	}

	#[test]
	fn an_exited_utility_releases_the_writer_and_is_reaped() {
		let (writer, feed) = channel();
		let shared = feed.0;
		let output = Output::start_with(shared.clone(), [fake("exit 7")]).unwrap();
		wait_for(|| shared.stop.load(Ordering::Acquire) && lock(&output.child).is_none());
		assert!(!writer.write(&[0.5; REBUFFER_SAMPLES]));
		drop(output);
	}
}
