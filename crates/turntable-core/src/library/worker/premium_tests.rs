//! Premium rejection uses the same account teardown request as explicit sign-out.

use super::*;

#[test]
fn premium_signout_detaches_library_session_and_removes_account_caches_without_api() {
	let directory = tempfile::tempdir().unwrap();
	let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
	let session = {
		let _context = runtime.enter();
		librespot_core::Session::new(librespot_core::SessionConfig::default(), None)
	};
	session.set_username("alice");
	let (stop, rx) = tokio::sync::watch::channel(false);
	let shared = SessionReader::from_session(session, runtime.handle().clone(), rx);
	let snapshots = Arc::new(Mutex::new(Store::new(directory.path())));
	let client = Client::open(Arc::new(|| panic!("sign-out must not mint a Web API token")), false);
	let mut session = Some(Session::new(client, spotify::User { id: "alice".into(), display_name: None }, Vec::new(), snapshots.clone()));
	let mut player = Some(Reader::for_test(shared.clone()));
	let mut added = Membership::new(directory.path());
	added.attach(Some(shared), &|_| {});
	let mut index = super::super::added::PlaylistIndex::load(directory.path(), "alice").unwrap();
	index.persist().unwrap();
	let library_cache = directory.path().join("library");
	std::fs::create_dir_all(&library_cache).unwrap();
	std::fs::write(library_cache.join("alice.json"), b"private account cache").unwrap();
	let mut refresh_added = true;
	stop.send_replace(true);
	serve(directory.path(), &snapshots, &mut session, &mut player, &mut added, &mut refresh_added, false, Request::ForgetSnapshot, &|_| {});
	assert!(session.is_none());
	assert!(player.is_none());
	assert!(!refresh_added);
	assert!(!library_cache.exists());
	assert!(!directory.path().join("playlist-index").exists());
}
