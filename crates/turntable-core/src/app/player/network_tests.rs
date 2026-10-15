//! Deterministic network-loss acceptance: no account, audio output or playback is started.

use super::*;
use crate::{
	app::Msg,
	engine::{Event, Status},
};

fn controller() -> (Controller, tempfile::TempDir) {
	let root = tempfile::tempdir().unwrap();
	let paths = Paths { config: root.path().join("config"), cache: root.path().join("cache") };
	let controller = Controller::fixture(&paths, State::default()).unwrap();
	(controller, root)
}

#[test]
fn offline_start_finishes_progress_and_retries_beyond_the_old_restart_limit() {
	let (mut controller, _root) = controller();
	for attempt in 1..=20 {
		controller.state.starting = true;
		controller.on_event(AppEvent::EngineReady(0, Err("simulated network unavailable".into())));
		assert!(!controller.state.starting);
		assert!(!controller.state.starting_player());
		assert!(controller.state.offline);
		assert!(controller.state.network_notice.is_some());
		assert!(controller.retry_start_at.is_some());
		assert_eq!(controller.start_failures, attempt);
	}
	assert_eq!(retry_wait(1), Duration::from_secs(10));
	assert_eq!(retry_wait(20), Duration::from_secs(160));
	assert_eq!(retry_wait(u32::MAX), Duration::from_secs(160));
	// Background retries must not put the setup view back into an endless spinner.
	controller.state.starting = true;
	assert!(!controller.state.starting_player());
}

#[test]
fn interrupted_playback_freezes_and_keeps_a_notice_until_the_user_resumes() {
	let (mut controller, _root) = controller();
	controller.state.status = Status::Playing;
	controller.state.active = true;
	controller.state.anchor = crate::app::Anchor { position_ms: 5000, at: Instant::now(), speed: 1.0 };
	controller.network_disconnected("simulated lost connection");
	assert_eq!(controller.state.status, Status::Idle);
	assert_eq!(controller.state.anchor.speed, 0.0);
	assert!(controller.state.offline);
	assert!(controller.playback_interrupted);
	// The same transitions used by a newly authenticated engine, without opening one.
	controller.player_offline = false;
	controller.refresh_network_notice();
	assert!(!controller.state.offline);
	assert!(controller.state.network_notice.is_some());
	controller.on_event(AppEvent::Player(0, Incoming::Event(Event::VolumeChanged { volume: 50 })));
	assert!(controller.state.notice.is_none());
	assert!(controller.state.network_notice.is_some());
	controller.on_event(AppEvent::Player(0, Incoming::Event(Event::PlaybackChanged { status: Status::Playing })));
	assert!(controller.state.network_notice.is_none());
}

#[test]
fn library_and_player_connectivity_recover_independently() {
	let (mut controller, _root) = controller();
	controller.on_event(AppEvent::Library(0, Box::new(library::Response::Offline(true))));
	assert!(controller.state.offline);
	assert_eq!(controller.deadline(), controller.library_retry_at);
	controller.on_event(AppEvent::EngineReady(0, Err("simulated offline".into())));
	controller.on_event(AppEvent::Library(0, Box::new(library::Response::Offline(false))));
	assert!(controller.state.offline, "library recovery cannot hide a disconnected player");
	assert!(controller.library_retry_at.is_none());
	controller.player_offline = false;
	controller.refresh_network_notice();
	assert!(!controller.state.offline);
	assert!(controller.state.network_notice.is_none());
}

#[test]
fn wake_detection_handles_both_clock_models_and_ignores_backward_clock_changes() {
	let before = (Instant::now(), SystemTime::now());
	assert!(!wake_gap(before, (before.0 + Duration::from_secs(30), before.1 + Duration::from_secs(30))));
	// macOS monotonic clocks may exclude sleep; the wall clock still advances.
	assert!(wake_gap(before, (before.0 + Duration::from_secs(1), before.1 + Duration::from_secs(120))));
	assert!(wake_gap(before, (before.0 + Duration::from_secs(1), before.1 + Duration::from_secs(5))));
	assert!(wake_gap(before, (before.0 + Duration::from_secs(120), before.1 + Duration::from_secs(120))));
	assert!(!wake_gap(before, (before.0 + Duration::from_secs(1), before.1 - Duration::from_secs(120))));
}

#[test]
fn native_rate_limit_stays_visible_during_volume_changes_cached_replies_and_retries() {
	let (mut controller, _root) = controller();
	controller.state.library.conn = library::Conn::Ready;
	controller.state.library.route = library::Route::Liked;
	let track = serde_json::from_value(serde_json::json!({"uri": "spotify:track:cached", "name": "Saved song"})).unwrap();
	controller.state.library.listing = Some(library::Listing { tracks: vec![track], ..library::Listing::default() });
	let message = crate::spotify::Error::RateLimited { retry_after: None }.user_message();
	controller.on_event(AppEvent::Player(0, Incoming::RateLimited { id: 0, limited: true }));
	for volume in 1..=100 {
		controller.dispatch(Msg::Volume(volume));
		assert_eq!(controller.state.volume, volume, "local slider state applies immediately");
		controller.on_event(AppEvent::Player(0, Incoming::Event(Event::VolumeChanged { volume })));
		controller.on_event(AppEvent::Player(0, Incoming::RateLimited { id: 0, limited: true }));
		assert_eq!(controller.state.network_notice.as_deref(), Some(message.as_str()));
		assert_eq!(controller.state.library.error.as_deref(), Some(message.as_str()));
		assert!(controller.state.toast.is_none(), "no toast is re-animated for each slider step");
		assert_eq!(controller.state.library.rows().len(), 1, "rate limiting never replaces cached content");
	}
	controller.on_event(AppEvent::Library(
		0,
		Box::new(library::Response::Listing {
			route:   library::Route::Liked,
			offset:  0,
			listing: controller.state.library.listing.clone().unwrap(),
		}),
	));
	assert_eq!(controller.state.network_notice.as_deref(), Some(message.as_str()), "a retry's cached first paint is not recovery");
	controller.on_event(AppEvent::Player(0, Incoming::RateLimited { id: 0, limited: false }));
	assert!(controller.state.network_notice.is_none());
	assert!(controller.state.library.error.is_none());
	assert_eq!(controller.state.library.rows().len(), 1);
}

#[test]
fn native_startup_429_keeps_its_friendly_notice_and_honors_long_backoff() {
	let (mut controller, _root) = controller();
	let native: librespot_core::Error =
		librespot_core::http_client::HttpClientError::RateLimited { retry_after: Duration::from_secs(3600) }.into();
	let error = StartError::from(std::io::Error::other(native));
	assert!(matches!(error, StartError::RateLimited(wait) if wait == Duration::from_secs(3600)));
	let before = Instant::now();
	controller.on_event(AppEvent::EngineReady(0, Err(error)));
	assert!(controller.retry_start_at.unwrap() >= before + Duration::from_secs(3600));
	let message = controller.state.notice.clone();
	assert_eq!(message, Some(crate::spotify::Error::RateLimited { retry_after: None }.user_message()));
	controller.state.starting = true;
	controller.on_event(AppEvent::Progress(0, "Connecting to Spotify…".into()));
	assert_eq!(controller.state.notice, message);
	assert!(!controller.state.starting_player(), "automatic retries must not flash setup progress");
	assert!(!controller.state.offline, "429 is a reachable Spotify service, not a lost network");
}
