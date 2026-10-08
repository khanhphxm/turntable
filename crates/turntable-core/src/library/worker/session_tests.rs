//! Routing contracts: even native errors must not fall through to the quota-limited API.

use std::{cell::RefCell, error::Error, sync::atomic::AtomicUsize};

use librespot_core::{Session as PlayerSession, SessionConfig};
use librespot_protocol::connect::{Cluster, DeviceInfo};

use super::*;

struct Fixture {
	runtime:   tokio::runtime::Runtime,
	stop:      tokio::sync::watch::Sender<bool>,
	directory: tempfile::TempDir,
	session:   Session,
	minted:    Arc<AtomicUsize>,
	shared:    SessionReader,
}

fn fixture() -> Result<Fixture, Box<dyn Error>> {
	let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build()?;
	let player = {
		let _context = runtime.enter();
		PlayerSession::new(SessionConfig::default(), None)
	};
	player.set_username("alice");
	let _ = player.set_user_attribute("display_name", "Alice");
	let _ = player.set_user_attribute("product", "premium");
	let (stop, rx) = tokio::sync::watch::channel(false);
	let shared = SessionReader::from_session(player, runtime.handle().clone(), rx);
	let directory = tempfile::tempdir()?;
	let minted = Arc::new(AtomicUsize::new(0));
	let attempts = minted.clone();
	let client = Client::open(
		Arc::new(move || {
			attempts.fetch_add(1, Ordering::Relaxed);
			Err(spotify::Error::AuthRequired)
		}),
		true,
	);
	let snapshots = Arc::new(Mutex::new(Store::new(directory.path())));
	let mut session = Session::new(client, spotify::User { id: "alice".into(), display_name: None }, Vec::new(), snapshots);
	session.player = Some(Reader::for_test(shared.clone()));
	Ok(Fixture { runtime, stop, directory, session, minted, shared })
}

#[test]
fn like_and_follow_switch_to_session_and_native_failures_never_use_api() -> Result<(), Box<dyn Error>> {
	let Fixture { runtime: _runtime, stop, directory: _directory, mut session, minted, .. } = fixture()?;
	stop.send_replace(true);
	for uri in ["spotify:track:0000000000000000000001", "spotify:album:0000000000000000000002"] {
		for saved in [true, false] {
			assert!(set_liked(&mut session, uri, saved, &|_| {}).is_err());
		}
	}
	for uri in ["spotify:artist:0000000000000000000003", "spotify:album:0000000000000000000002", "spotify:playlist:0000000000000000000004"]
	{
		for saved in [true, false] {
			assert!(set_followed(&mut session, uri, saved).is_err());
		}
	}
	assert_eq!(minted.load(Ordering::Relaxed), 0);
	session.player = None;
	assert!(matches!(set_liked(&mut session, "spotify:track:track", true, &|_| {}), Err(spotify::Error::AuthRequired)));
	assert!(matches!(set_followed(&mut session, "spotify:artist:artist", false), Err(spotify::Error::AuthRequired)));
	assert_eq!(minted.load(Ordering::Relaxed), 2, "no-session writes retain the API fallback");
	Ok(())
}

#[test]
fn visibility_switches_to_session_even_when_private_is_requested() -> Result<(), Box<dyn Error>> {
	let Fixture { runtime: _runtime, stop, directory, mut session, minted, .. } = fixture()?;
	stop.send_replace(true);
	let mut added = Membership::new(directory.path());
	for public in [false, true] {
		let changes = spotify::PlaylistChanges { public: Some(public), ..Default::default() };
		assert!(rename_playlist(&mut session, &mut added, "0000000000000000000001".into(), &changes, &|_| {}).is_err());
	}
	assert_eq!(minted.load(Ordering::Relaxed), 0);
	session.player = None;
	let changes = spotify::PlaylistChanges { public: Some(false), ..Default::default() };
	assert!(matches!(rename_playlist(&mut session, &mut added, "playlist".into(), &changes, &|_| {}), Err(spotify::Error::AuthRequired)));
	assert_eq!(minted.load(Ordering::Relaxed), 1);
	Ok(())
}

#[test]
fn session_connect_uses_native_profile_and_preserves_snapshot_first_paint() -> Result<(), Box<dyn Error>> {
	let Fixture { runtime: _runtime, stop: _stop, directory, session: previous, minted, .. } = fixture()?;
	let snapshots = previous.snapshots.clone();
	{
		let mut store = snapshots.lock();
		store.record(&Response::Connected {
			me:        spotify::User { id: "alice".into(), display_name: Some("Cached Alice".into()) },
			playlists: Vec::new(),
		});
		store.record(&Response::Home(Home { loaded: true, ..Default::default() }));
		store.save()?;
	}
	let mut session = Some(previous);
	let mut reader = session.as_mut().unwrap().player.take();
	let replies = RefCell::new(Vec::new());
	connect(directory.path(), &snapshots, &mut session, &mut reader, true, &|reply| replies.borrow_mut().push(reply));
	let replies = replies.into_inner();
	assert!(matches!(replies.first(), Some(Response::Connected { me, .. }) if me.display_name.as_deref() == Some("Cached Alice")));
	assert!(replies.iter().any(|reply| matches!(reply, Response::Home(home) if home.loaded)));
	assert!(
		replies.iter().any(
			|reply| matches!(reply, Response::Connected { me, .. } if me.id == "alice" && me.display_name.as_deref() == Some("Alice"))
		)
	);
	assert!(session.unwrap().player.is_some());
	assert_eq!(minted.load(Ordering::Relaxed), 0);
	Ok(())
}

#[test]
fn session_product_free_enforces_premium_without_me_or_a_snapshot() -> Result<(), Box<dyn Error>> {
	let Fixture { runtime: _runtime, stop: _stop, directory, session: mut previous, minted, shared } = fixture()?;
	let mut reader = previous.player.take();
	// librespot itself exits on a non-premium `type`; testing `product` exercises our gate.
	let _ = shared.session().set_user_attribute("product", "free");
	let snapshots = previous.snapshots.clone();
	let mut session = Some(previous);
	let replies = RefCell::new(Vec::new());
	connect(directory.path(), &snapshots, &mut session, &mut reader, true, &|reply| replies.borrow_mut().push(reply));
	assert!(session.is_none());
	assert!(matches!(replies.borrow().as_slice(), [Response::SignInRejected(_)]));
	assert_eq!(minted.load(Ordering::Relaxed), 0);
	Ok(())
}

#[test]
fn connect_devices_and_remote_state_read_the_dealer_cluster_without_api() -> Result<(), Box<dyn Error>> {
	let Fixture { runtime: _runtime, stop: _stop, directory: _directory, session, minted, shared } = fixture()?;
	let mut cluster = Cluster { active_device_id: "phone".into(), ..Default::default() };
	cluster.device.insert("phone".into(), DeviceInfo { name: "Phone".into(), can_play: true, ..Default::default() });
	shared.set_cluster(cluster);
	assert!(
		matches!(connect_request(&session, Request::Devices)?, Some(Response::Devices(Ok(devices))) if devices.len() == 1 && devices[0].name == "Phone")
	);
	assert!(matches!(connect_request(&session, Request::Playback)?, Some(Response::Playback(Ok(None)))));
	assert_eq!(minted.load(Ordering::Relaxed), 0);
	Ok(())
}

#[test]
fn connect_transfers_controls_and_missing_cluster_never_fall_back_with_session() -> Result<(), Box<dyn Error>> {
	let Fixture { runtime: _runtime, stop, directory: _directory, mut session, minted, .. } = fixture()?;
	stop.send_replace(true);
	for request in [Request::Devices, Request::Playback, Request::Transfer { id: "phone".into(), play: false }, Request::Control {
		device:   "phone".into(),
		command:  spotify::PlayerCommand::Pause,
		revision: 1,
	}] {
		let result = connect_request(&session, request)?;
		assert!(matches!(
			result,
			Some(
				Response::Devices(Err(_))
					| Response::Playback(Err(_))
					| Response::PlayerError(_)
					| Response::ControlFailed { revision: 1, .. }
			)
		));
	}
	assert_eq!(minted.load(Ordering::Relaxed), 0);
	session.player = None;
	for request in [Request::Devices, Request::Playback, Request::Transfer { id: "phone".into(), play: false }, Request::Control {
		device:   "phone".into(),
		command:  spotify::PlayerCommand::Pause,
		revision: 1,
	}] {
		let result = connect_request(&session, request)?;
		assert!(
			matches!(
				result,
				Some(
					Response::Devices(Err(_))
						| Response::Playback(Err(_))
						| Response::PlayerError(_)
						| Response::ControlFailed { revision: 1, .. }
				)
			),
			"a Web API token failure is a request error, never a lost native session"
		);
	}
	assert_eq!(minted.load(Ordering::Relaxed), 4);
	Ok(())
}
