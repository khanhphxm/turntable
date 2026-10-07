//! Incoming worker events and execution of the effects returned by application updates.

use std::{
	thread,
	time::{Duration, Instant},
};

use super::{AppEvent, CONTROL_POLL, Controller, RETRY_START, SEARCH_DEBOUNCE, debug_log};
use crate::{
	app::{Effect, Msg, Panel, config},
	engine::{Command, Event, Incoming},
	library::{self, Route},
};

impl Controller {
	/// Applies a user action and its effects. Returns whether the host should redraw.
	pub fn dispatch(&mut self, msg: Msg) -> bool {
		if crate::net::trace::enabled() {
			match &msg {
				Msg::Navigate(route) => crate::net::trace::mark("nav", &format!("{route:?}")),
				Msg::Edit { .. } | Msg::Type(_) | Msg::SearchSubmit(_) => crate::net::trace::mark("key", ""),
				_ => {},
			}
		}
		let settings_open = self.state.panel == Some(Panel::Settings);
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
				Effect::SignOut => self.sign_out(),
				Effect::SaveClientId(id) => match config::write_private(&self.paths.config.join("client_id"), id.as_bytes()) {
					Ok(()) => {
						self.state.id_input.clear();
						self.state.focus = None;
						self.state.library.conn = library::Conn::NeedsLogin;
						self.library.send(library::Request::Connect);
					},
					Err(e) => self.state.notice = Some(format!("Couldn't save the client ID: {e}")),
				},
				Effect::SearchSoon => self.search_at = Some(Instant::now() + SEARCH_DEBOUNCE),
				Effect::CancelSearchSoon => self.search_at = None,
				Effect::SetEq => {
					if let Some(audio) = &self.audio {
						audio.set_eq(self.state.eq);
					}
					self.save_settings();
				},
				Effect::SaveSettings => self.save_settings(),
				// `open` waits for nothing slow, but run it off the UI loop anyway.
				Effect::OpenUrl(url) => drop(thread::spawn(move || crate::spotify::open_browser(&url))),
				Effect::ApplyPrefs => self.apply_prefs(),
				Effect::RestartPlayer => self.restart_player(),
				Effect::ClearCache => self.clear_cache(),
			}
		}
		// The volume the audio stage leaves alone, and the settings page's facts when it has just opened.
		if let Some(audio) = &self.audio {
			audio.set_volume(self.state.volume);
		}
		if !settings_open && self.state.panel == Some(Panel::Settings) {
			self.refresh_system();
		}
		self.reconcile_poll();
		true
	}

	pub(super) fn on_event(&mut self, event: AppEvent) -> bool {
		match event {
			AppEvent::Progress(text) => self.state.notice = Some(text),
			AppEvent::EngineReady(Ok(engine)) => return self.on_engine_ready(engine),
			AppEvent::EngineReady(Err(e)) => {
				self.state.starting = false;
				if std::mem::take(&mut self.discard_start) {
					if self.state.signed_in {
						self.start_engine();
					}
					return true;
				}
				self.state.notice = Some(e);
				// Transient failures (offline, Spotify unreachable) recover on their
				// own: try again later, backing off.
				self.start_failures += 1;
				let wait = RETRY_START.saturating_mul(1 << self.start_failures.min(5)).min(Duration::from_secs(300));
				self.retry_start_at = Some(Instant::now() + wait);
			},
			AppEvent::Player(Incoming::Event(event)) => return self.on_player_event(event),
			AppEvent::Player(Incoming::Closed(reason)) => {
				debug_log(&format!("player stopped: {reason}"));
				self.client = None;
				// An in-process engine has one event stream: restart it, not its client.
				return self.check_engine();
			},
			AppEvent::Library(response) => return self.on_library_response(*response),
			AppEvent::Lyrics(response) => {
				if let Err(e) = &response.lyrics {
					debug_log(&format!("lyrics for {}: {e}", response.uri));
				}
				self.state.apply_lyrics(response);
			},
			AppEvent::Image { url, bytes, mime } => {
				self.images.loaded(&url);
				self.state.images.entry(url).or_insert_with(|| std::sync::Arc::new(crate::app::Cover { bytes, mime }));
			},
			AppEvent::ImageFailed(url) => self.images.failed(url),
			AppEvent::Devices => {
				self.refresh_outputs();
				self.reconcile_output();
			},
			AppEvent::CacheSize(bytes) => self.state.sys.cache_bytes = Some(bytes),
			AppEvent::CacheCleared(result) => self.on_cache_cleared(result),
		}
		true
	}

	fn on_engine_ready(&mut self, engine: crate::engine::Engine) -> bool {
		self.state.starting = false;
		// A restart asked for meanwhile makes this start stale; let it go and start again.
		if std::mem::take(&mut self.discard_start) {
			let _ = thread::Builder::new().name("engine-stop".into()).spawn(move || drop(engine));
			if self.state.signed_in {
				self.start_engine();
			}
			return true;
		}
		self.start_failures = 0;
		self.state.notice = None;
		self.state.signed_in = true;
		self.engine = Some(engine);
		self.on_engine_started();
		self.connect();
		true
	}

	fn on_player_event(&mut self, event: Event) -> bool {
		self.state.connected = true;
		let was_logged_in = self.state.logged_in;
		let was_item = std::mem::take(&mut self.last_item);
		let was_active = self.state.active;
		let queue_event = matches!(event, Event::QueueChanged { .. });
		let before = self.state.rules_before();
		let changed = self.state.apply(event);
		if let Some(audio) = &self.audio {
			audio.set_volume(self.state.volume);
		}
		for cmd in self.state.rules_after(&before, Instant::now()) {
			self.send_player(cmd);
		}
		self.send_queue_commands();
		if queue_event {
			self.queue_arrived();
		}
		// Ask for the queue as soon as a login exists.
		if self.state.logged_in && !was_logged_in {
			self.send_player(Command::GetQueue { limit: 20 });
		}
		self.item_changed(&was_item);
		if (was_active && !self.state.active) || (!was_logged_in && self.state.logged_in && !self.state.active) {
			self.probe();
		}
		changed
	}

	fn on_library_response(&mut self, response: library::Response) -> bool {
		if crate::net::trace::enabled() {
			crate::net::trace::mark("resp", &response.label());
		}
		if matches!(
			response,
			library::Response::Devices(_)
				| library::Response::Playback(_)
				| library::Response::Transferred
				| library::Response::PlayerError(_)
		) {
			return self.on_player_response(response);
		}
		match &response {
			library::Response::Done(text) => self.state.show_toast(text.clone()),
			library::Response::Created(playlist) => {
				self.state.show_toast(format!("Created {}", playlist.name));
				let route = Route::Playlist(playlist.id.clone());
				self.state.library.apply(response);
				self.dispatch(Msg::Navigate(route));
				return true;
			},
			library::Response::Queue(uris, title) => {
				if let Some(client) = &self.client {
					for uri in uris {
						client.send(Command::AddToQueue { uri: uri.clone() });
					}
				}
				self.state.show_toast(format!("Added {title} to the queue"));
			},
			_ => {},
		}
		self.send_queue_commands();
		let connected = matches!(response, library::Response::Connected { .. });
		self.state.library.apply(response);
		self.state.drop_remote_if_signed_out();
		if connected || !self.state.web_ready() {
			self.poll_inflight = false;
			self.poll_forced = false;
		}
		if connected {
			self.probe();
			if self.state.library.route == Route::Home {
				self.library.send(library::Request::Home);
			}
			self.library.send(library::Request::Albums);
			self.library.send(library::Request::Following);
			self.request_artist();
		}
		true
	}
}
