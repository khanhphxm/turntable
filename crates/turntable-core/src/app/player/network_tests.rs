//! Deterministic network-loss acceptance: no account, audio output or playback is started.

use super::*;
use crate::engine::{Event, Status};

fn controller() -> (Controller, tempfile::TempDir) {
	let root = tempfile::tempdir().unwrap();
	let paths = Paths { config: root.path().join("config"), cache: root.path().join("cache") };
	let controller = Controller::new(&paths, Options::default(), Arc::new(|| {})).unwrap();
	(controller, root)
}

#[test]
fn offline_start_finishes_progress_and_retries_beyond_the_old_restart_limit() {
	let (mut controller, _root) = controller();
	for attempt in 1..=20 {
		controller.state.starting = true;
		controller.on_event(AppEvent::EngineReady(Err("simulated network unavailable".into())));
		assert!(!controller.state.starting);
		assert!(!controller.state.starting_player());
		assert!(controller.state.offline);
		assert!(controller.state.network_notice.as_deref().unwrap().contains("saved library"));
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
	assert!(controller.state.network_notice.as_deref().unwrap().contains("press Play"));
	controller.on_event(AppEvent::Player(Incoming::Event(Event::VolumeChanged { volume: 50 })));
	assert!(controller.state.notice.is_none());
	assert!(controller.state.network_notice.is_some());
	controller.on_event(AppEvent::Player(Incoming::Event(Event::PlaybackChanged { status: Status::Playing })));
	assert!(controller.state.network_notice.is_none());
}

#[test]
fn library_and_player_connectivity_recover_independently() {
	let (mut controller, _root) = controller();
	controller.on_event(AppEvent::Library(Box::new(library::Response::Offline(true))));
	assert!(controller.state.offline);
	assert_eq!(controller.deadline(), controller.library_retry_at);
	controller.on_event(AppEvent::EngineReady(Err("simulated offline".into())));
	controller.on_event(AppEvent::Library(Box::new(library::Response::Offline(false))));
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
