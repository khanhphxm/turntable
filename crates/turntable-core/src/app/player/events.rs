//! Incoming worker events and execution of the effects returned by application updates.

use std::{
	sync::{Arc, mpsc},
	thread,
	time::Instant,
};

use super::{AppEvent, CONTROL_POLL, Controller, HEALTH_EVERY, SEARCH_DEBOUNCE, StartError, debug_log, retry_wait};
use crate::{
	app::{Effect, Msg},
	engine::{Command, Event, Incoming},
	library::{self, Route},
};

pub(super) fn player_delivery(
	tx: mpsc::Sender<AppEvent>,
	wake: Arc<dyn Fn() + Send + Sync>,
	generation: u64,
) -> impl FnMut(Incoming) + Send {
	move |incoming| {
		match &incoming {
			Incoming::Event(event) => event.trace("ui-wake"),
			Incoming::ConnectState(state) => {
				if let Some(player) = &state.playback {
					crate::net::trace::playback("connect-wake", player.status.name(), Some(player.position.position_ms));
				} else {
					crate::net::trace::playback("connect-wake", "inactive", None);
				}
			},
			_ => {},
		}
		if tx.send(AppEvent::Player(generation, incoming)).is_ok() {
			wake();
		}
	}
}

impl Controller {
	/// Applies a user action and its effects. Returns whether the host should redraw.
	/// Transport state and position are predicted before sending; engine events or failures
	/// reconcile them on an immediate worker wake, never on a playback polling timer.
	pub fn dispatch(&mut self, msg: Msg) -> bool {
		let playback = match &msg {
			Msg::TogglePlay => Some("toggle"),
			Msg::Play => Some("play"),
			Msg::Pause => Some("pause"),
			Msg::Next => Some("next"),
			Msg::Prev => Some("previous"),
			Msg::PlayContext(_) | Msg::PlayRow(_) | Msg::PlayListing => Some("load"),
			Msg::Seek(_) | Msg::SeekBy(_) => Some("seek"),
			_ => None,
		};
		if let Some(action) = playback {
			crate::net::trace::playback("msg", action, None);
		}
		if crate::net::trace::enabled() {
			match &msg {
				Msg::Navigate(route) => crate::net::trace::mark("nav", &format!("{route:?}")),
				Msg::Edit { .. } | Msg::SetText(_, _) | Msg::SearchSubmit(_) => crate::net::trace::mark("key", ""),
				_ => {},
			}
		}
		for effect in self.state.update(msg) {
			match effect {
				Effect::Player(cmd) => self.send_player(cmd),
				Effect::Library(req) => {
					match &req {
						// The other device answers a moment later: look at what it did.
						library::Request::Control { .. } => self.poll_soon(CONTROL_POLL),
						library::Request::Transfer { .. } => self.force_poll = true,
						_ => {},
					}
					if crate::net::trace::enabled() {
						crate::net::trace::mark("req", &req.label());
					}
					self.library.send(req);
				},
				Effect::SignIn => self.start_engine(),
				Effect::CancelSignIn => self.cancel_sign_in(false),
				Effect::SignOut => self.sign_out(),
				Effect::SearchSoon => self.search_at = Some(Instant::now() + SEARCH_DEBOUNCE),
				Effect::CancelSearchSoon => self.search_at = None,
				Effect::SetEq => {
					if let Some(audio) = &self.audio {
						audio.set_eq(self.state.eq);
					}
					self.save_settings();
				},
				Effect::SaveSettings => self.save_settings(),
				Effect::SaveRecentSearches => self.save_searches(),
				// `open` waits for nothing slow, but run it off the UI loop anyway.
				Effect::OpenUrl(url) => drop(thread::spawn(move || crate::spotify::open_browser(&url))),
				Effect::ApplyPrefs => self.apply_prefs(),
				Effect::ReconfigureNetwork => self.reconfigure_network(),
			}
		}
		// The audio stage follows the visible volume immediately; persistence does not block it.
		if let Some(audio) = &self.audio {
			audio.set_volume(self.state.volume);
		}
		self.reconcile_poll();
		self.remember_playback(!self.state.playing());
		true
	}

	pub(super) fn on_event(&mut self, event: AppEvent) -> bool {
		let generation = match &event {
			AppEvent::Player(generation, _)
			| AppEvent::Library(generation, _)
			| AppEvent::Progress(generation, _)
			| AppEvent::EngineReady(generation, _)
			| AppEvent::Lyrics(generation, _) => Some(*generation),
			_ => None,
		};
		// A forgotten worker may acknowledge cleanup in the new generation. Those empty
		// membership answers must not turn signed-out defaults back into loaded projections.
		let signed_out = self.account_generation != 0 && !self.state.signed_in && !self.state.starting;
		if generation.is_some_and(|generation| generation != self.account_generation || signed_out) {
			// Dropping a late engine can block while it disconnects; never do that on the UI thread.
			if let AppEvent::EngineReady(_, Ok(engine)) = event {
				let _ = thread::Builder::new().name("engine-stop".into()).spawn(move || drop(engine));
			}
			return false;
		}
		match event {
			AppEvent::Progress(_, text) => self.state.notice = Some(text),
			AppEvent::EngineReady(_, Ok(engine)) => return self.on_engine_ready(engine),
			AppEvent::EngineReady(_, Err(e)) => {
				if std::mem::take(&mut self.discard_start) {
					self.state.starting = false;
					self.start_authorization.set_deadline(None);
					if self.state.signed_in {
						self.start_engine();
					}
					return true;
				}
				let e = match e {
					StartError::SignInTimedOut => {
						self.cancel_sign_in(true);
						return true;
					},
					StartError::Premium(error) => {
						self.reject_sign_in(error);
						return true;
					},
					StartError::Other(message) => message,
				};
				self.state.starting = false;
				self.start_authorization.set_deadline(None);
				self.state.notice = Some(e);
				// Transient failures (offline, Spotify unreachable) recover on their
				// own: try again later, backing off.
				self.player_offline = true;
				self.refresh_network_notice();
				self.start_failures = self.start_failures.saturating_add(1);
				self.retry_start_at = Some(Instant::now() + retry_wait(self.start_failures));
				self.restore_without_session();
				self.request_startup_home();
			},
			AppEvent::Player(_, Incoming::Event(event)) => return self.on_player_event(event),
			AppEvent::Player(_, Incoming::RestoreUnavailable { account }) => self.on_restore_unavailable(account),
			AppEvent::Player(_, Incoming::ConnectState(state)) => return self.on_connect_state(state),
			AppEvent::Player(_, Incoming::CommandFailed { id, message }) => {
				self.state.playback_failed(id);
				self.state.show_toast(message);
			},
			AppEvent::Player(_, Incoming::Closed(reason)) => {
				debug_log(&format!("player stopped: {reason}"));
				self.client = None;
				// An in-process engine has one event stream: restart it, not its client.
				return self.check_engine();
			},
			AppEvent::Library(_, response) => {
				if let library::Response::Artist(artist) = response.as_ref()
					&& self.asked_artist.as_deref() == Some(crate::spotify::id_of(&artist.uri))
				{
					self.asked_artist = None;
				}
				return self.on_library_response(*response);
			},
			AppEvent::Lyrics(_, response) => {
				if let Err(e) = &response.lyrics {
					debug_log(&format!("lyrics for {}: {e}", response.uri));
				}
				self.state.apply_lyrics(response);
			},
			AppEvent::Image { url, bytes, mime } => {
				self.images.loaded(&url);
				self.state.cache_image(url, crate::app::Cover { bytes, mime });
			},
			AppEvent::ImageFailed(url) => self.images.failed(url),
			AppEvent::Devices => self.refresh_outputs(),
			AppEvent::Outputs(devices, default) => self.on_outputs(devices, default),
			AppEvent::AudioReady(generation, selected, result) => return self.on_audio_ready(generation, selected.as_deref(), result),
			AppEvent::AudioFailed(generation) => self.on_audio_failed(generation),
			AppEvent::SaveFailed(message) => self.state.notice = Some(message),
		}
		true
	}

	fn on_engine_ready(&mut self, engine: crate::engine::Engine) -> bool {
		self.state.starting = false;
		self.start_authorization.set_deadline(None);
		// A restart asked for meanwhile makes this start stale; let it go and start again.
		if std::mem::take(&mut self.discard_start) {
			let _ = thread::Builder::new().name("engine-stop".into()).spawn(move || drop(engine));
			if self.state.signed_in {
				self.start_engine();
			}
			return true;
		}
		self.start_failures = 0;
		self.player_offline = false;
		self.refresh_network_notice();
		self.state.notice = None;
		self.state.signed_in = true;
		self.library.set_session(engine.session_reader());
		self.request_startup_home();
		if self.library_offline && self.library_retry_at.take().is_some() {
			self.library.send(library::Request::Connect);
		}
		self.engine = Some(engine);
		self.restore.pending = true;
		self.on_engine_started();
		self.connect();
		true
	}

	fn on_player_event(&mut self, event: Event) -> bool {
		self.state.connected = true;
		let was_logged_in = self.state.logged_in;
		let was_item = std::mem::take(&mut self.last_item);
		let was_active = self.state.active;
		let before = self.state.rules_before();
		let creator_changed = matches!(&event, Event::MetadataChanged { entity }
			if entity.uri == self.state.item.uri && entity.decorations.creators.first().map(|creator| creator.entity.uri.as_str())
				!= self.state.item.decorations.creators.first().map(|creator| creator.entity.uri.as_str()));
		if matches!(&event, Event::QueueChanged { upcoming, .. } if !upcoming.is_empty() || self.state.active) {
			self.restore.queue_changed = true;
		}
		let changed = self.state.apply(event);
		if self.state.playing() && self.playback_interrupted {
			self.playback_interrupted = false;
			self.refresh_network_notice();
		}
		if let Some(audio) = &self.audio {
			audio.set_volume(self.state.volume);
		}
		for cmd in self.state.rules_after(&before, Instant::now()) {
			self.send_player(cmd);
		}
		// Ask for the queue as soon as a login exists.
		if self.state.logged_in && !was_logged_in {
			self.send_player(Command::GetQueue);
		}
		self.item_changed(&was_item);
		if creator_changed {
			self.request_artist();
		}
		self.remember_playback(!self.state.playing());
		if (was_active && !self.state.active) || (!was_logged_in && self.state.logged_in && !self.state.active) {
			self.probe();
		}
		changed
	}

	fn on_library_response(&mut self, mut response: library::Response) -> bool {
		if crate::net::trace::enabled() {
			crate::net::trace::mark("resp", &response.label());
		}
		if let library::Response::SignInRejected(error) = response {
			self.reject_sign_in(error);
			return true;
		}
		if let library::Response::LastPlayback { account, playback } = response {
			return self.on_recent_playback(account, playback);
		}
		if let library::Response::PlaybackCheckpoint { account, position } = response {
			return self.on_playback_checkpoint(&account, position);
		}
		if matches!(
			response,
			library::Response::Devices(_)
				| library::Response::Playback(_)
				| library::Response::Transferred
				| library::Response::ControlFailed { .. }
				| library::Response::PlayerError(_)
		) {
			return self.on_player_response(response);
		}
		if let Some(request) = self.state.apply_library_dialog(&mut response) {
			self.library.send(request);
		}
		match &response {
			library::Response::Offline(offline) => {
				self.library_offline = *offline;
				self.library_retry_at = offline.then(|| Instant::now() + HEALTH_EVERY);
				self.refresh_network_notice();
			},
			library::Response::NeedsLogin => {
				self.library_offline = false;
				self.library_retry_at = None;
				self.refresh_network_notice();
			},
			library::Response::Done(text) => self.state.show_toast(text.clone()),
			library::Response::Created(playlist) => {
				self.state.show_toast(format!("Created {}", playlist.name));
				let route = Route::Playlist(playlist.id.clone());
				self.state.library.apply(response);
				self.dispatch(Msg::Navigate(route));
				return true;
			},
			library::Response::Queue(uris, title) => {
				for command in self.state.queue_tracks(uris.clone(), false) {
					self.send_player(command);
				}
				self.state.show_toast(format!("Added {title} to the queue"));
			},
			_ => {},
		}
		let connected = matches!(response, library::Response::Connected { .. });
		self.state.library.apply(response);
		self.state.drop_remote_if_signed_out();
		if connected || !self.state.web_ready() {
			self.poll_inflight = false;
			self.poll_forced = false;
		}
		if connected {
			self.request_last_playback();
			if !self.restore.idle_cluster {
				self.probe();
			}
			if !(self.state.starting && self.engine.is_none()) {
				self.request_startup_home();
			}
			self.library.send(library::Request::Albums);
			self.library.send(library::Request::Following);
			self.library.send(library::Request::Pins);
			self.library.send(library::Request::FolderExpansion);
			self.library.send(library::Request::Exclusions);
			self.request_artist();
		}
		true
	}

	fn request_startup_home(&self) {
		if self.state.web_ready() && self.state.library.route == Route::Home {
			self.library.send(library::Request::Home);
		}
	}
}

#[cfg(test)]
mod tests {
	use std::sync::atomic::{AtomicUsize, Ordering};

	use super::*;
	use crate::{
		Paths,
		app::State,
		engine::{ConnectState, Entity, Position, Related, Status},
	};

	#[test]
	fn playback_status_and_position_wake_the_host_on_delivery_without_any_timer() -> Result<(), Box<dyn std::error::Error>> {
		let (tx, rx) = mpsc::channel();
		let wakes = Arc::new(AtomicUsize::new(0));
		let observed = wakes.clone();
		let mut deliver = player_delivery(
			tx,
			Arc::new(move || {
				observed.fetch_add(1, Ordering::Relaxed);
			}),
			7,
		);
		deliver(Incoming::Event(Event::PlaybackChanged { status: Status::Playing }));
		assert_eq!(wakes.load(Ordering::Relaxed), 1);
		assert!(matches!(rx.try_recv()?, AppEvent::Player(7, Incoming::Event(Event::PlaybackChanged { status: Status::Playing }))));
		deliver(Incoming::Event(Event::PositionSync { position: Position { position_ms: 42.0, speed: 1.0, ..Position::default() } }));
		assert_eq!(wakes.load(Ordering::Relaxed), 2);
		assert!(matches!(rx.try_recv()?, AppEvent::Player(7, Incoming::Event(Event::PositionSync { .. }))));
		deliver(Incoming::Event(Event::PlaybackChanged { status: Status::Paused }));
		assert_eq!(wakes.load(Ordering::Relaxed), 3);
		assert!(matches!(rx.try_recv()?, AppEvent::Player(7, Incoming::Event(Event::PlaybackChanged { status: Status::Paused }))));
		deliver(Incoming::ConnectState(ConnectState {
			account:      "fixture".into(),
			devices:      Vec::new(),
			playback:     None,
			local_active: false,
			initial:      false,
		}));
		assert_eq!(wakes.load(Ordering::Relaxed), 4, "a remote stop also wakes immediately");
		assert!(matches!(rx.try_recv()?, AppEvent::Player(7, Incoming::ConnectState(_))));
		Ok(())
	}

	#[test]
	fn late_track_enrichment_requests_the_artist_without_waiting_for_another_track_change() -> Result<(), Box<dyn std::error::Error>> {
		let root = tempfile::tempdir()?;
		let paths = Paths { config: root.path().join("config"), cache: root.path().join("cache") };
		let mut state = State { status: Status::Paused, ..State::default() };
		state.library.conn = library::Conn::Ready;
		state.item.uri = "spotify:track:current".into();
		let mut controller = Controller::fixture(&paths, state)?;
		controller.last_item = "spotify:track:current".into();
		let mut item = Entity { uri: "spotify:track:current".into(), ..Entity::default() };
		item.decorations.identity.name = "Current".into();
		item.decorations.creators.push(Related { entity: Box::new(Entity { uri: "spotify:artist:creator".into(), ..Entity::default() }) });
		assert!(controller.on_player_event(Event::MetadataChanged { entity: item }));
		assert!(controller.fixture_requests()?.iter().any(|request| matches!(request, library::Request::Artist(id) if id == "creator")));
		assert!(!controller.state().playing());
		Ok(())
	}
}
