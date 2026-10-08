//! Device enumeration, opening and disposal are serialized away from the host UI thread.

use std::{
	io,
	sync::{Arc, mpsc},
	thread,
};

use super::AppEvent;
use crate::audio::{self, Feed, Player, StartError};

pub(super) enum Job {
	Start(Feed),
	Rebuild(Player),
	Select(Player),
}

impl Job {
	fn run(self, device: Option<&str>) -> Result<Player, StartError> {
		match self {
			Self::Start(feed) => Player::start(feed, device),
			Self::Rebuild(audio) => Player::start(audio.into_feed(), device),
			Self::Select(mut audio) => match audio.set_device(device) {
				Ok(()) => Ok(audio),
				Err(error) => Err(StartError { error, feed: audio.into_feed() }),
			},
		}
	}
}

pub(super) enum Request {
	Open { generation: u64, job: Job, device: Option<String> },
	Devices,
	Dispose(Result<Player, StartError>),
	Finish { audio: Option<Player>, engine: Option<crate::engine::Engine>, events: mpsc::Receiver<AppEvent> },
}

pub(super) struct Worker(mpsc::Sender<Request>);

impl Worker {
	pub(super) fn spawn(tx: mpsc::Sender<AppEvent>, wake: Arc<dyn Fn() + Send + Sync>) -> io::Result<Self> {
		let (send, receive) = mpsc::channel();
		thread::Builder::new().name("audio-output".into()).spawn(move || {
			while let Ok(request) = receive.recv() {
				let event = match request {
					Request::Open { generation, job, device } => {
						let result = job.run(device.as_deref());
						AppEvent::AudioReady(generation, device, result)
					},
					Request::Devices => AppEvent::Outputs(audio::output_devices(), audio::default_output_name()),
					Request::Dispose(result) => {
						drop(result);
						continue;
					},
					Request::Finish { audio, engine, events } => {
						drop(audio);
						drop(engine);
						// Queued and late AudioReady values are disposed here too, never during the
						// controller's UI-thread field teardown.
						drop(events);
						break;
					},
				};
				if tx.send(event).is_err() {
					break;
				}
				wake();
			}
		})?;
		Ok(Self(send))
	}

	pub(super) fn send(&self, request: Request) -> Result<(), Box<mpsc::SendError<Request>>> {
		self.0.send(request).map_err(Box::new)
	}

	pub(super) fn refresh(&self) -> bool {
		self.0.send(Request::Devices).is_ok()
	}

	pub(super) fn dispose(&self, result: Result<Player, StartError>) {
		// A disconnected receiver owns no platform output; normal late results are disposed
		// here while the worker is alive, including after sign-out/cancellation.
		let _ = self.0.send(Request::Dispose(result));
	}

	pub(super) fn finish(&self, audio: Option<Player>, engine: Option<crate::engine::Engine>, events: mpsc::Receiver<AppEvent>) {
		let _ = self.0.send(Request::Finish { audio, engine, events });
	}
}

#[cfg(test)]
mod tests {
	use std::time::Duration;

	use super::*;
	use crate::{Paths, State, app::player::Controller};

	#[test]
	fn finishing_the_worker_closes_its_queued_event_receiver_off_the_host_thread() {
		let (tx, events) = mpsc::channel();
		let (awaken, wakes) = mpsc::channel::<()>();
		let worker = Worker::spawn(
			tx.clone(),
			Arc::new(move || {
				let _ = awaken.send(());
			}),
		)
		.unwrap();
		tx.send(AppEvent::SaveFailed("queued before shutdown".into())).unwrap();
		worker.finish(None, None, events);
		assert_eq!(wakes.recv_timeout(Duration::from_secs(3)), Err(mpsc::RecvTimeoutError::Disconnected));
		assert!(tx.send(AppEvent::SaveFailed("after shutdown".into())).is_err());
	}

	#[test]
	fn enumeration_posts_an_event_and_wakes_from_the_output_worker() {
		let host = thread::current().id();
		let (tx, events) = mpsc::channel();
		let (awaken, wakes) = mpsc::channel();
		let worker = Worker::spawn(
			tx,
			Arc::new(move || {
				awaken.send(thread::current().id()).unwrap();
			}),
		)
		.unwrap();
		assert!(worker.refresh());
		assert!(matches!(events.recv_timeout(Duration::from_secs(3)).unwrap(), AppEvent::Outputs(_, _)));
		assert_ne!(wakes.recv_timeout(Duration::from_secs(3)).unwrap(), host);
	}

	#[test]
	fn cancelled_audio_results_cannot_replace_a_new_generations_inflight_open() {
		let root = tempfile::tempdir().unwrap();
		let paths = Paths { config: root.path().join("config"), cache: root.path().join("cache") };
		let mut controller = Controller::fixture(&paths, State::default()).unwrap();
		controller.audio_generation = 2;
		controller.audio_inflight = true;
		let (_, feed) = audio::channel();
		let error = StartError { error: io::Error::new(io::ErrorKind::NotFound, "no output"), feed };
		assert!(!controller.on_event(AppEvent::AudioReady(1, None, Err(error))));
		assert!(controller.audio_inflight);
		assert!(!controller.state.has_audio);
		assert!(controller.pending_feed.is_none() && controller.audio_retry_at.is_none());
	}

	#[test]
	fn an_open_failure_keeps_its_feed_for_retry_without_blocking_dispatch() {
		let root = tempfile::tempdir().unwrap();
		let paths = Paths { config: root.path().join("config"), cache: root.path().join("cache") };
		let mut controller = Controller::fixture(&paths, State::default()).unwrap();
		controller.audio_inflight = true;
		let (_, feed) = audio::channel();
		let error = StartError { error: io::Error::new(io::ErrorKind::NotFound, "no output"), feed };
		assert!(controller.on_event(AppEvent::AudioReady(0, None, Err(error))));
		assert!(!controller.audio_inflight && !controller.state.has_audio);
		assert!(controller.pending_feed.is_some() && controller.audio_retry_at.is_some());
		assert!(controller.state.notice.as_deref().unwrap().starts_with(super::super::NO_AUDIO));
	}
}
