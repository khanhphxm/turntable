//! Opt-in offline hosts exercise the production dispatch, snapshot and event-pump paths.
//! Only Spotify/cover transport and platform device observation are omitted. Requests are
//! captured, and callers supply service responses explicitly; no playback engine can start.

use super::{AppEvent, Controller, HEALTH_EVERY, State, covers, library, mpsc, restore};
use crate::app::config::Paths;

impl Controller {
	/// Creates an isolated offline controller with caller-supplied account state.
	///
	/// Available only with `test-support` (or in core unit tests). This never reads saved
	/// credentials, contacts Spotify, opens/watches audio devices, or starts service/cover
	/// workers. The output worker stays idle. The real persistence worker writes only under
	/// `paths` and is flushed and joined on drop. Disk snapshots use the production stores.
	/// Use [`Self::fixture_response`] followed by [`Self::pump`] to deliver fixture answers,
	/// and [`Self::fixture_requests`] to inspect the requests produced by UI messages.
	/// Signing in through a fixture is a test error; playback has no engine or audio device.
	pub fn fixture(paths: &Paths, mut state: State) -> Result<Self, String> {
		for dir in [&paths.config, &paths.cache] {
			std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
		}
		let account = state.library.me.as_ref().map(|me| me.id.clone());
		state.recent_searches.load(&paths.config, account.clone());
		let (tx, rx) = mpsc::channel();
		let saver = super::spawn_saver(paths, tx.clone(), std::sync::Arc::new(|| {}))?;
		let output_worker = super::output::Worker::spawn(tx.clone(), std::sync::Arc::new(|| {})).map_err(|error| error.to_string())?;
		let delivery = tx.clone();
		let (library, inbox) = library::Worker::fixture(&paths.cache, account.as_deref(), move |generation, response| {
			let _ = delivery.send(AppEvent::Library(generation, Box::new(response)));
		});
		Ok(Self {
			paths: paths.clone(),
			state,
			account_generation: 0,
			start_authorization: crate::engine::StartAuthorization::default(),
			tx,
			rx: Some(rx),
			wake: std::sync::Arc::new(|| {}),
			engine: None,
			audio: None,
			output_worker,
			audio_generation: 0,
			audio_inflight: false,
			outputs_inflight: false,
			outputs_dirty: false,
			saver,
			save_at: None,
			settings_dirty: false,
			searches_dirty: false,
			client: None,
			library,
			images: covers::Fetcher::fixture(),
			lyrics: None,
			health_at: std::time::Instant::now() + HEALTH_EVERY,
			search_at: None,
			retry_start_at: None,
			pending_feed: None,
			audio_retry_at: None,
			start_failures: 0,
			player_offline: false,
			library_offline: false,
			library_retry_at: None,
			playback_interrupted: false,
			last_tick: (std::time::Instant::now(), std::time::SystemTime::now()),
			asked_artist: None,
			discard_start: false,
			followed: None,
			last_item: String::new(),
			restore: restore::Restore::new(false),
			visible: true,
			poll_at: None,
			poll_inflight: false,
			poll_forced: false,
			force_poll: false,
			poll_failures: 0,
			output_retry_at: None,
			_devices: None,
			fixture_inbox: Some(inbox),
		})
	}

	/// Records an explicit fixture answer in the real snapshot store and queues it for `pump`.
	/// Returns an error if used on a production controller or snapshot persistence fails.
	pub fn fixture_response(&self, response: library::Response) -> Result<(), String> {
		if self.fixture_inbox.is_none() {
			return Err("fixture_response requires an offline fixture controller".into());
		}
		self.library.fixture_response(response).map_err(|error| error.to_string())
	}

	/// Acknowledges the captured local ForgetSnapshot request using production snapshot cleanup.
	pub fn fixture_complete_forget(&self) -> Result<(), String> {
		if self.fixture_inbox.is_none() {
			return Err("fixture_complete_forget requires an offline fixture controller".into());
		}
		self.library.fixture_complete_forget();
		Ok(())
	}

	/// Drains requests from an offline fixture's production library request channel.
	pub fn fixture_requests(&self) -> Result<Vec<library::Request>, String> {
		self.fixture_inbox
			.as_ref()
			.ok_or_else(|| "fixture_requests requires an offline fixture controller".to_owned())
			.map(library::worker::FixtureInbox::take_requests)
	}
}
