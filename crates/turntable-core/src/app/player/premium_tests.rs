//! Offline Premium-gate acceptance; no account, Web API, audio output or playback is started.

use super::*;
use crate::engine::{Event, SignInError};

#[test]
fn free_sign_in_rejects_clears_credentials_and_account_snapshot_and_shows_reason() {
	let root = tempfile::tempdir().unwrap();
	let paths = Paths { config: root.path().join("config"), cache: root.path().join("cache") };
	let mut controller = Controller::new(&paths, Options { web_api: false, ..Options::default() }, Arc::new(|| {})).unwrap();
	std::fs::write(paths.config.join("credentials.json"), b"saved credentials").unwrap();
	let library_cache = paths.cache.join("library");
	std::fs::create_dir_all(&library_cache).unwrap();
	std::fs::write(library_cache.join("alice.json"), b"account-scoped snapshot").unwrap();
	controller.state.signed_in = true;
	controller.state.starting = true;
	controller.retry_start_at = Some(Instant::now());
	controller.state.library.conn = library::Conn::Ready;
	controller.state.item.uri = "spotify:track:old".into();
	let error = SignInError::PremiumRequired { account: Some("Alice".into()), product: Some("free".into()) };
	controller.on_event(AppEvent::EngineReady(Err(StartError::Premium(error.clone()))));
	assert!(!crate::engine::has_sign_in(&paths.config));
	assert!(!library_cache.exists());
	assert!(!controller.state.signed_in);
	assert!(!controller.state.starting);
	assert!(!controller.state.logged_in);
	assert!(!controller.state.connected);
	assert!(!controller.state.offline);
	assert!(controller.retry_start_at.is_none());
	assert!(controller.engine.is_none());
	assert!(controller.client.is_none());
	assert!(controller.audio.is_none());
	assert!(controller.state.item.uri.is_empty());
	assert!(controller.state.needs_setup());
	assert_eq!(controller.state.sign_in_error, Some(error.clone()));
	assert_eq!(controller.state.notice.as_deref(), Some(error.message().as_str()));
	assert!(error.message().contains("Alice is on Spotify Free"));
	assert!(error.message().contains("spotify.com/premium"));
	assert_eq!(error.upgrade_url(), "https://www.spotify.com/premium/");
	// Teardown can leave events already in the UI queue. They cannot clear the reason
	// or repopulate the rejected account's library/player.
	assert!(!controller.on_event(AppEvent::Player(Incoming::Event(Event::VolumeChanged { volume: 50 }))));
	assert!(!controller.on_event(AppEvent::Library(Box::new(library::Response::Offline(true)))));
	assert!(!controller.on_event(AppEvent::Progress("late connection progress".into())));
	assert_eq!(controller.state.notice.as_deref(), Some(error.message().as_str()));
	assert!(!controller.state.offline);
}
