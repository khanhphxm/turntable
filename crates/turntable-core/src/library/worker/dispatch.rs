//! One owner for library writes and page caches; foreground work precedes idle refreshes.

use std::{
	path::PathBuf,
	sync::{
		Arc,
		atomic::{AtomicU64, Ordering},
		mpsc,
	},
};

use parking_lot::Mutex;
use tokio::sync::watch;

use super::{Inbox, Membership, Reader, Request, RequestContext, Response, Session, Store, schedule::Queue, serve};

pub(super) struct Runner {
	config:            PathBuf,
	web_api:           bool,
	snapshots:         Arc<Mutex<Store>>,
	deliver:           Arc<dyn Fn(u64, Response) + Send + Sync>,
	session:           Option<Session>,
	player:            Option<Reader>,
	added:             Membership,
	refresh_added:     bool,
	urgent:            watch::Receiver<()>,
	generation:        Arc<AtomicU64>,
	active_generation: u64,
}

impl Runner {
	pub(super) fn new(
		config: PathBuf,
		cache: &std::path::Path,
		web_api: bool,
		snapshots: Arc<Mutex<Store>>,
		deliver: Arc<dyn Fn(u64, Response) + Send + Sync>,
		urgent: watch::Receiver<()>,
		generation: Arc<AtomicU64>,
	) -> Self {
		Self {
			config,
			web_api,
			snapshots,
			deliver,
			session: None,
			player: None,
			added: Membership::new(cache),
			refresh_added: false,
			urgent,
			generation,
			active_generation: 0,
		}
	}

	pub(super) fn run(mut self, rx: &mpsc::Receiver<Inbox>) {
		let mut queue = Queue::default();
		loop {
			if self.urgent.has_changed().is_err() {
				break;
			}
			// Mark only notifications already represented by the drained inbox as seen.
			// A foreground send racing with an idle refresh then cancels it immediately.
			let _ = self.urgent.borrow_and_update();
			for request in rx.try_iter() {
				queue.push(request);
			}
			if let Some(request) = queue.pop(self.session.is_some()) {
				self.process(request);
				continue;
			}
			if let Some(session) = &mut self.session {
				session.client.forget_recent();
				session.pages.evict_expired();
			}
			if std::mem::take(&mut self.refresh_added) {
				let generation = self.active_generation;
				let deliver = |response| {
					if self.generation.load(Ordering::Acquire) == generation {
						(self.deliver)(generation, response);
					}
				};
				self.refresh_added = !self.added.refresh(self.urgent.clone(), &deliver);
				continue;
			}
			match rx.recv() {
				Ok(request) => queue.push(request),
				Err(_) => break,
			}
		}
	}

	fn process(&mut self, request: Inbox) {
		let generation = match &request {
			Inbox::Session(generation, _) | Inbox::Request(generation, _) => *generation,
		};
		if generation != self.generation.load(Ordering::Acquire) {
			return;
		}
		self.active_generation = generation;
		self.snapshots.lock().bind_generation(self.generation.clone(), generation);
		let deliver = self.deliver.clone();
		let live = self.generation.clone();
		let output = |response| {
			if live.load(Ordering::Acquire) == generation {
				deliver(generation, response);
			}
		};
		match request {
			Inbox::Session(_, reader) => {
				self.added.attach(reader.clone(), &output);
				self.refresh_added = reader.is_some();
				self.player = reader.map(Reader::new);
				output(Response::LikedTracks(Arc::from([])));
				if self.player.is_some() {
					self.serve(Request::Connect);
				} else if let Some(session) = &mut self.session {
					session.attach(None, &output);
				}
			},
			Inbox::Request(_, request) => {
				if matches!(request, Request::Connect)
					&& let Some(session) = &mut self.session
					&& session.player.is_some()
				{
					self.player = session.player.take();
				}
				self.serve(request);
			},
		}
	}

	fn serve(&mut self, request: Request) {
		let generation = self.active_generation;
		let deliver = |response| {
			if self.generation.load(Ordering::Acquire) == generation {
				(self.deliver)(generation, response);
			}
		};
		serve(
			RequestContext {
				config:        &self.config,
				snapshots:     &self.snapshots,
				session:       &mut self.session,
				player:        &mut self.player,
				added:         &mut self.added,
				refresh_added: &mut self.refresh_added,
				web_api:       self.web_api,
				deliver:       &deliver,
			},
			request,
		);
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn late_account_generation_requests_cannot_clear_the_new_session() {
		let directory = tempfile::tempdir().unwrap();
		let live = Arc::new(AtomicU64::new(1));
		let mut store = Store::new(directory.path());
		store.bind_generation(live.clone(), 1);
		store.record(&Response::Connected {
			me:        crate::spotify::User { id: "new-account".into(), display_name: None },
			playlists: Vec::new(),
		});
		let snapshots = Arc::new(Mutex::new(store));
		let (_, urgent) = watch::channel(());
		let mut runner =
			Runner::new(directory.path().join("config"), directory.path(), false, snapshots.clone(), Arc::new(|_, _| {}), urgent, live);
		runner.process(Inbox::Request(0, Request::ForgetSnapshot));
		assert!(matches!(snapshots.lock().connected(), Some(Response::Connected { me, .. }) if me.id == "new-account"));
		runner.process(Inbox::Request(1, Request::ForgetSnapshot));
		assert!(snapshots.lock().connected().is_none());
	}

	#[test]
	fn queue_checkpoint_round_trip_is_scoped_and_signout_rejects_late_writes() {
		let directory = tempfile::tempdir().unwrap();
		let live = Arc::new(AtomicU64::new(0));
		let snapshots = Arc::new(Mutex::new(Store::new(directory.path())));
		let (tx, rx) = mpsc::channel();
		let (_, urgent) = watch::channel(());
		let mut runner = Runner::new(
			directory.path().join("config"),
			directory.path(),
			false,
			snapshots,
			Arc::new(move |_, response| tx.send(response).unwrap()),
			urgent,
			live.clone(),
		);
		let checkpoint = crate::library::PlaybackPosition {
			account:     "alex".into(),
			uri:         "spotify:track:last".into(),
			context_uri: "spotify:playlist:context".into(),
			position_ms: 2_000,
			upcoming:    vec![crate::library::QueuedTrack {
				uid:    "next".into(),
				uri:    "spotify:track:next".into(),
				source: "queue".into(),
			}],
		};
		runner.process(Inbox::Request(0, Request::RememberPlayback(checkpoint.clone())));
		runner.process(Inbox::Request(0, Request::PlaybackCheckpoint("alex".into())));
		assert!(matches!(rx.recv().unwrap(), Response::PlaybackCheckpoint { position: Some(saved), .. } if saved == checkpoint));
		runner.process(Inbox::Request(0, Request::PlaybackCheckpoint("other".into())));
		assert!(matches!(rx.recv().unwrap(), Response::PlaybackCheckpoint { account, position: None } if account == "other"));
		live.store(1, Ordering::Release);
		runner.process(Inbox::Request(1, Request::ForgetSnapshot));
		runner.process(Inbox::Request(0, Request::RememberPlayback(checkpoint)));
		runner.process(Inbox::Request(1, Request::PlaybackCheckpoint("alex".into())));
		assert!(rx.try_iter().any(|response| matches!(response, Response::PlaybackCheckpoint { position: None, .. })));
		assert!(!directory.path().join("config/playback").exists());
	}
}
