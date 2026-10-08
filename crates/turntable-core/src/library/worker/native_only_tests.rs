//! Disabling Web API must not make native attachment wait for separate OAuth setup.

use super::*;

#[test]
fn native_only_worker_waits_for_engine_without_requesting_oauth_client_id() {
	let directory = tempfile::tempdir().unwrap();
	let (tx, rx) = mpsc::channel();
	let _worker = Worker::spawn(directory.path().join("config"), directory.path(), false, move |response| {
		tx.send(response).unwrap();
	});
	assert!(matches!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Response::NeedsLogin));
	// Native saved-credential discovery can create the config directory; it
	// must not require or initialize the separate OAuth account's files.
	assert!(!directory.path().join("config/client_id").exists());
	assert!(!directory.path().join("config/token.json").exists());
}

#[test]
fn fresh_native_connection_is_ready_with_web_api_disabled_and_no_oauth_account() {
	let directory = tempfile::tempdir().unwrap();
	let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
	let native = {
		let _context = runtime.enter();
		librespot_core::Session::new(librespot_core::SessionConfig::default(), None)
	};
	native.set_username("alice");
	let _ = native.set_user_attribute("display_name", "Alice");
	let _ = native.set_user_attribute("type", "premium");
	let (_stop, rx) = tokio::sync::watch::channel(false);
	let shared = SessionReader::from_session(native, runtime.handle().clone(), rx);
	let snapshots = Arc::new(Mutex::new(Store::new(directory.path())));
	let mut session = None;
	let mut player = Some(Reader::for_test(shared));
	let replies = std::cell::RefCell::new(Vec::new());
	connect(directory.path(), &snapshots, &mut session, &mut player, false, &|response| replies.borrow_mut().push(response));
	assert!(player.is_none());
	assert!(session.as_ref().unwrap().player.is_some());
	assert!(replies.borrow().iter().any(|response| matches!(response,
		Response::Connected { me, .. } if me.id == "alice" && me.display_name.as_deref() == Some("Alice"))));
	assert!(
		!replies
			.borrow()
			.iter()
			.any(|response| matches!(response, Response::NeedsClientId | Response::NeedsLogin | Response::LoginFailed(_)))
	);
	assert!(
		matches!(session.as_ref().unwrap().client.me_with_plan(), Err(spotify::Error::Network(message)) if message.contains("disabled"))
	);
}
