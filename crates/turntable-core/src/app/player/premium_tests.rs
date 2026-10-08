//! Offline Premium-gate acceptance; no account, Web API, audio output or playback is started.

use super::*;
use crate::engine::Event;

#[test]
fn late_account_responses_stay_rejected_after_another_sign_in() {
	let root = tempfile::tempdir().unwrap();
	let paths = Paths { config: root.path().join("config"), cache: root.path().join("cache") };
	let mut controller = Controller::fixture(&paths, State::default()).unwrap();
	let previous = controller.account_generation;
	controller.sign_out();
	let notice = controller.state.notice.clone();
	controller.state.signed_in = true;
	controller.state.library.me = Some(crate::spotify::User { id: "new-account".into(), display_name: None });
	controller.state.library.query = "same query".into();
	let events = [
		AppEvent::Library(
			previous,
			Box::new(library::Response::Connected {
				me:        crate::spotify::User { id: "old-account".into(), display_name: None },
				playlists: Vec::new(),
			}),
		),
		AppEvent::Library(
			previous,
			Box::new(library::Response::Search { query: "same query".into(), results: crate::spotify::SearchResults::default() }),
		),
		AppEvent::Player(
			previous,
			Incoming::Event(Event::TrackChanged { item: crate::engine::Entity { uri: "spotify:track:old".into(), ..Default::default() } }),
		),
		AppEvent::Progress(previous, "old progress".into()),
		AppEvent::EngineReady(previous, Err(StartError::Other("old error".into()))),
	];
	for event in events {
		assert!(!controller.on_event(event));
	}
	assert_eq!(controller.state.library.me.as_ref().unwrap().id, "new-account");
	assert!(controller.state.library.search.is_none());
	assert!(controller.state.item.uri.is_empty());
	assert!(controller.retry_start_at.is_none());
	assert_eq!(controller.state.notice, notice);
	assert!(controller.on_event(AppEvent::Progress(controller.account_generation, "new progress".into())));
	assert_eq!(controller.state.notice.as_deref(), Some("new progress"));
}
