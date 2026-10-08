//! One owner for library writes and page caches; foreground work precedes idle refreshes.

use std::{
	path::PathBuf,
	sync::{Arc, Mutex, mpsc},
};

use tokio::sync::watch;

use super::{Inbox, Membership, Reader, Request, Response, Session, Store, schedule::Queue, serve};

pub(super) struct Runner {
	config:        PathBuf,
	web_api:       bool,
	snapshots:     Arc<Mutex<Store>>,
	deliver:       Arc<dyn Fn(Response) + Send + Sync>,
	session:       Option<Session>,
	player:        Option<Reader>,
	added:         Membership,
	refresh_added: bool,
	urgent:        watch::Receiver<()>,
}

impl Runner {
	pub(super) fn new(
		config: PathBuf,
		cache: &std::path::Path,
		web_api: bool,
		snapshots: Arc<Mutex<Store>>,
		deliver: Arc<dyn Fn(Response) + Send + Sync>,
		urgent: watch::Receiver<()>,
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
				self.refresh_added = !self.added.refresh(self.urgent.clone(), self.deliver.as_ref());
				continue;
			}
			match rx.recv() {
				Ok(request) => queue.push(request),
				Err(_) => break,
			}
		}
	}

	fn process(&mut self, request: Inbox) {
		match request {
			Inbox::Session(reader) => {
				self.added.attach(reader.clone(), self.deliver.as_ref());
				self.refresh_added = reader.is_some();
				self.player = reader.map(Reader::new);
				(self.deliver)(Response::LikedTracks(Arc::from([])));
				if self.player.is_some() {
					self.serve(Request::Connect);
				} else if let Some(session) = &mut self.session {
					session.attach(None, self.deliver.as_ref());
				}
			},
			Inbox::Request(request) => {
				if matches!(request, Request::Connect | Request::Login)
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
		serve(
			&self.config,
			&self.snapshots,
			&mut self.session,
			&mut self.player,
			&mut self.added,
			&mut self.refresh_added,
			self.web_api,
			request,
			self.deliver.as_ref(),
		);
	}
}
