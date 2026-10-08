//! Opt-in real-account acceptance: a Session only, with the Web API disabled.
//! No Connect player, audio output, window activation or playback is created.
//!
//! `TURNTABLE_TEST_CONFIG=~/.config/turntable TURNTABLE_TRACE=/tmp/library-actions.jsonl`
//! `cargo test -p turntable-core library_actions_round_trip -- --ignored --nocapture`

use std::{cell::RefCell, error::Error};

use librespot_core::{Session as NativeSession, SessionConfig, cache::Cache as NativeCache};

use super::*;
use crate::{
	app::{Dialog, Effect, Msg, State},
	library::added::{PlaylistId, PlaylistIndex},
	spotify::User,
};

type Result<T, E = Box<dyn Error>> = std::result::Result<T, E>;

struct Harness {
	config:     tempfile::TempDir,
	cache:      tempfile::TempDir,
	session:    Option<Session>,
	snapshots:  Arc<Mutex<Store>>,
	membership: Membership,
	state:      State,
}

impl Harness {
	fn request(&mut self, request: Request) -> Result<Option<Playlist>> {
		let responses = RefCell::new(Vec::new());
		serve(
			self.config.path(),
			&self.snapshots,
			&mut self.session,
			&mut None,
			&mut self.membership,
			&mut false,
			false,
			request,
			&|response| responses.borrow_mut().push(response),
		);
		let mut created = None;
		for mut response in responses.into_inner() {
			match &response {
				Response::Error { message, .. } => return Err(message.clone().into()),
				Response::NeedsLogin => return Err("Unexpected login requirement with an authenticated native Session".into()),
				Response::Created(playlist) => created = Some(playlist.clone()),
				Response::Done(message) => self.state.show_toast(message.clone()),
				_ => {},
			}
			self.state.apply_library_dialog(&mut response);
			self.state.library.apply(response);
		}
		Ok(created)
	}

	fn dispatch(&mut self, message: Msg) -> Result<()> {
		for effect in self.state.update(message) {
			let Effect::Library(request) = effect else { return Err("Library action attempted a non-library effect".into()) };
			self.request(request)?;
		}
		assert_eq!(self.state.library.route, Route::Library, "no action may navigate");
		Ok(())
	}

	fn reader(&mut self) -> Result<&mut Reader> {
		self.session.as_mut().and_then(|session| session.player.as_mut()).ok_or_else(|| "Native reader missing".into())
	}
}

#[test]
#[ignore = "mutates only a new scratch playlist and an initially unsaved album; requires saved account credentials"]
fn library_actions_round_trip() -> Result<()> {
	let config =
		std::env::var_os("TURNTABLE_TEST_CONFIG").map(PathBuf::from).ok_or("Set TURNTABLE_TEST_CONFIG to the saved account directory")?;
	trace::init();
	let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build()?;
	let credentials = NativeCache::new(Some(&config), Some(&config), None::<&PathBuf>, None)?
		.credentials()
		.ok_or("No saved credentials in the test config")?;
	let native = {
		let _context = runtime.enter();
		NativeSession::new(SessionConfig::default(), None)
	};
	runtime.block_on(native.connect(credentials, false))?;
	let (_stop, rx) = tokio::sync::watch::channel(false);
	let shared = SessionReader::from_session(native.clone(), runtime.handle().clone(), rx);
	let mut reader = Reader::new(shared.clone());
	let playlists = reader.playlists()?;
	let album = ["spotify:album:4aawyAB9vmqN3uQ7FjRGTy", "spotify:album:1ATL5GLyefJaxhQzSPVrLX", "spotify:album:0sNOF9WDwhWunNAHPD3Baj"]
		.into_iter()
		.map(|uri| reader.contains(uri).map(|saved| (uri, saved)))
		.collect::<spotify::Result<Vec<_>>>()?
		.into_iter()
		.find_map(|(uri, saved)| (!saved).then_some(uri))
		.ok_or("All acceptance album candidates are already saved")?;
	let cache = tempfile::tempdir()?;
	let snapshots = Arc::new(Mutex::new(Store::new(cache.path())));
	let me = User { id: native.username(), display_name: None };
	let connected = Response::Connected { me: me.clone(), playlists: playlists.clone() };
	snapshots.lock().unwrap_or_else(PoisonError::into_inner).record(&connected);
	let mut membership = Membership::new(cache.path());
	membership.attach(Some(shared.clone()), &|_| {});
	let mut session = Session::new(Client::open(Arc::new(|| Err(spotify::Error::AuthRequired)), false), me, playlists, snapshots.clone());
	session.player = Some(reader);
	let mut state = State::default();
	state.library.apply(connected);
	state.library.route = Route::Library;
	let mut harness = Harness { config: tempfile::tempdir()?, cache, session: Some(session), snapshots, membership, state };
	let track = "spotify:track:4uLU6hMCjMI75M1A2tKUQC";
	let scratch = harness
		.request(Request::CreatePlaylist("turntable test".into(), vec![track.into(); 51]))?
		.ok_or("Create did not return a scratch playlist")?;
	eprintln!("scratch created: {}", scratch.uri);
	let edited =
		std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| exercise(&mut harness, &scratch, &shared, &native, album, track)));
	// All cleanup is attempted even after a failed assertion or a write/read error.
	let album_cleanup = harness.dispatch(Msg::SetSaved { uri: album.into(), saved: false });
	let exclusion_cleanup = harness.dispatch(Msg::ExcludeFromTaste { uri: scratch.uri.clone(), excluded: false });
	let pin_cleanup = harness.dispatch(Msg::SetPinned { uri: scratch.uri.clone(), pinned: false });
	let deletion = harness.dispatch(Msg::DeletePlaylist(scratch.id.clone()));
	assert!(!Reader::new(shared.clone()).playlists()?.iter().any(|playlist| playlist.id == scratch.id));
	assert!(!Reader::new(shared).contains(album)?);
	assert!(harness.state.toast.as_ref().is_some_and(|(_, text)| text == "Removed from Your Library"));
	let index = PlaylistIndex::load(harness.cache.path(), &native.username())?;
	assert!(index.playlist_tracks(&PlaylistId::from_uri(&scratch.uri)?).is_none());
	assert!(harness.snapshots.lock().unwrap_or_else(PoisonError::into_inner).listing(&Route::Playlist(scratch.id.clone())).is_none());
	eprintln!("scratch deleted: {}; album restored; Web API client disabled; playback effects rejected", scratch.uri);
	album_cleanup?;
	exclusion_cleanup?;
	pin_cleanup?;
	deletion?;
	match edited {
		Ok(result) => result,
		Err(panic) => std::panic::resume_unwind(panic),
	}
}

fn exercise(
	harness: &mut Harness,
	scratch: &Playlist,
	shared: &SessionReader,
	native: &NativeSession,
	album: &str,
	track: &str,
) -> Result<()> {
	harness.snapshots.lock().unwrap_or_else(PoisonError::into_inner).record(&Response::Listing {
		route:   Route::Playlist(scratch.id.clone()),
		offset:  0,
		listing: Listing { title: scratch.name.clone(), ..Listing::default() },
	});
	let index = PlaylistIndex::load(harness.cache.path(), &native.username())?;
	assert_eq!(index.playlist_tracks(&PlaylistId::from_uri(&scratch.uri)?).map(<[_]>::len), Some(51));
	harness.state.library.playlists.iter_mut().find(|playlist| playlist.id == scratch.id).unwrap().public = None;
	harness.dispatch(Msg::EditPlaylist(scratch.id.clone()))?;
	assert!(matches!(harness.state.dialog, Some(Dialog::Rename { ref name, .. }) if name == "turntable test"));
	assert_eq!(harness.state.public_input, Some(false));
	harness.dispatch(Msg::CloseDialog)?;
	for public in [true, false] {
		harness.dispatch(Msg::SetPlaylistPublic(scratch.id.clone(), public))?;
		assert_eq!(harness.reader()?.playlist(&scratch.id, 0, false)?.0.public, Some(public));
		eprintln!("scratch public: {public}");
	}
	for collaborative in [true, false] {
		harness.dispatch(Msg::SetCollaborative(scratch.id.clone(), collaborative))?;
		assert_eq!(harness.reader()?.playlist(&scratch.id, 0, false)?.0.collaborative, collaborative);
		eprintln!("scratch collaborative: {collaborative}");
	}
	for pinned in [true, false] {
		harness.dispatch(Msg::SetPinned { uri: scratch.uri.clone(), pinned })?;
		assert_eq!(harness.state.library.is_pinned(&scratch.uri), pinned);
		assert_eq!(super::super::pins::load(harness.config.path(), &native.username())?.contains(&scratch.uri), pinned);
		eprintln!("scratch local pin: {pinned}");
	}
	harness.dispatch(Msg::AddContextToPlaylist(Route::Playlist(scratch.id.clone())))?;
	assert!(matches!(harness.state.dialog, Some(Dialog::AddToPlaylist { ref uris, ref label })
		if uris.len() == 51 && uris.iter().all(|uri| uri == track) && label == "turntable test"));
	harness.dispatch(Msg::CloseDialog)?;
	for excluded in [true, false] {
		harness.dispatch(Msg::ExcludeFromTaste { uri: scratch.uri.clone(), excluded })?;
		assert_eq!(Reader::new(shared.clone()).excluded()?.contains(&scratch.uri), excluded);
		assert_eq!(harness.state.library.is_excluded(&scratch.uri), excluded);
		eprintln!("scratch taste exclusion: {excluded}");
	}
	harness.dispatch(Msg::AddContextToPlaylist(Route::Album(id_of(album).into())))?;
	assert!(matches!(harness.state.dialog, Some(Dialog::AddToPlaylist { ref uris, ref label }) if !uris.is_empty() && !label.is_empty()));
	harness.dispatch(Msg::CloseDialog)?;
	for saved in [true, false] {
		harness.dispatch(Msg::SetSaved { uri: album.into(), saved })?;
		assert_eq!(Reader::new(shared.clone()).contains(album)?, saved);
		assert_eq!(harness.state.library.albums.iter().any(|known| known.uri == album), saved);
		eprintln!("album saved: {saved} ({album})");
	}
	Ok(())
}
