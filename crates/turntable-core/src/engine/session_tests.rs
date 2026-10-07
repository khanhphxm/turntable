use std::{
	error::Error as StdError,
	sync::{
		atomic::{AtomicBool, Ordering},
		mpsc,
	},
	time::Duration,
};

use librespot_core::{Error, Session, SessionConfig};
use tokio::{runtime::Runtime, sync::watch};

use super::SessionReader;

fn runtime() -> std::io::Result<Runtime> {
	tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build()
}

fn reader(runtime: &Runtime, stop: watch::Receiver<bool>) -> SessionReader {
	let _context = runtime.enter();
	SessionReader { session: Session::new(SessionConfig::default(), None), runtime: runtime.handle().clone(), stop }
}

#[test]
fn a_stopped_reader_never_polls_io_after_its_runtime_is_gone() -> Result<(), Box<dyn StdError>> {
	let runtime = runtime()?;
	let (stop, rx) = watch::channel(false);
	let reader = reader(&runtime, rx);
	stop.send_replace(true);
	runtime.shutdown_timeout(Duration::from_millis(100));
	let polled = AtomicBool::new(false);
	let result: Result<(), Error> = reader.block_on(async {
		polled.store(true, Ordering::Relaxed);
		tokio::time::sleep(Duration::from_secs(1)).await;
		Ok(())
	});
	assert!(result.is_err());
	assert!(!polled.load(Ordering::Relaxed));
	Ok(())
}

#[test]
fn stopping_cancels_an_in_flight_read_without_waiting_for_network() -> Result<(), Box<dyn StdError>> {
	let runtime = runtime()?;
	let (stop, rx) = watch::channel(false);
	let reader = reader(&runtime, rx);
	let (started, ready) = mpsc::channel();
	std::thread::scope(|scope| -> Result<(), Box<dyn StdError>> {
		let job = scope.spawn(move || {
			reader.block_on(async {
				let _ = started.send(());
				std::future::pending::<Result<(), Error>>().await
			})
		});
		let started = ready.recv_timeout(Duration::from_secs(5));
		stop.send_replace(true);
		started?;
		let result = job.join().map_err(|payload| std::io::Error::other(format!("reader thread failed: {payload:?}")))?;
		assert!(result.is_err());
		Ok(())
	})?;
	Ok(())
}
