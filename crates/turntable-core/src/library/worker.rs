//! The library worker thread, its connected session and request dispatch.

use std::{
	path::{Path, PathBuf},
	sync::{
		Arc, LazyLock,
		atomic::{AtomicU64, Ordering},
		mpsc,
	},
	thread,
	time::{Duration, Instant},
};

use parking_lot::Mutex;

use super::{
	Home, Listing, Popularity, Request, Response, Route,
	added::Membership,
	artist::{artist, popularity},
	home::home,
	pages::{accent, all_albums, all_followed, all_playlists, open},
	session::Reader,
	snapshot::{self, Store},
};
use crate::{
	engine::SessionReader,
	net::{cache::Cache, par, trace},
	spotify::{self, Artist, Client, Playlist, id_of},
};

mod dispatch;
mod folders;
#[cfg(test)]
mod native_only_tests;
#[cfg(test)]
mod premium_tests;
mod schedule;
#[cfg(test)]
mod session_tests;
#[cfg(test)]
mod speed_tests;

pub struct Worker {
	tx:         mpsc::Sender<Inbox>,
	snapshots:  Arc<Mutex<Store>>,
	deliver:    Arc<dyn Fn(u64, Response) + Send + Sync>,
	prefetches: Mutex<Cache<Route, ()>>,
	urgent:     tokio::sync::watch::Sender<()>,
	generation: Arc<AtomicU64>,
}

/// The real request channel, retained without a runner by offline controller fixtures.
#[cfg(any(test, feature = "test-support"))]
pub(crate) struct FixtureInbox(mpsc::Receiver<Inbox>);

#[cfg(any(test, feature = "test-support"))]
impl FixtureInbox {
	pub(crate) fn take_requests(&self) -> Vec<Request> {
		self.0
			.try_iter()
			.filter_map(|event| match event {
				Inbox::Request(_, request) => Some(request),
				Inbox::Session(_, _) => None,
			})
			.collect()
	}
}

enum Inbox {
	Request(u64, Request),
	Session(u64, Option<SessionReader>),
}

impl Worker {
	/// Uses the production snapshot/send path but never starts a network runner.
	#[cfg(any(test, feature = "test-support"))]
	pub(crate) fn fixture(
		cache: &Path,
		account: Option<&str>,
		deliver: impl Fn(u64, Response) + Send + Sync + 'static,
	) -> (Self, FixtureInbox) {
		let (tx, rx) = mpsc::channel();
		let (urgent, _) = tokio::sync::watch::channel(());
		let generation = Arc::new(AtomicU64::new(0));
		let mut store = Store::new(cache);
		store.load(account);
		store.bind_generation(generation.clone(), 0);
		(
			Self {
				tx,
				snapshots: Arc::new(Mutex::new(store)),
				deliver: Arc::new(deliver),
				prefetches: Mutex::new(Cache::new(snapshot::LISTINGS_KEPT, PAGES_TTL)),
				urgent,
				generation,
			},
			FixtureInbox(rx),
		)
	}

	#[cfg(any(test, feature = "test-support"))]
	pub(crate) fn fixture_response(&self, response: Response) -> std::io::Result<()> {
		let changed = self.snapshots.lock().record(&response);
		if changed {
			snapshot::save(&self.snapshots)?;
		}
		(self.deliver)(self.generation.load(Ordering::Acquire), response);
		Ok(())
	}

	#[cfg(any(test, feature = "test-support"))]
	pub(crate) fn fixture_complete_forget(&self) {
		self.snapshots.lock().clear();
	}

	/// Starts the worker. Disk listings can also be delivered by `send`, without waiting for
	/// an in-flight network request; every snapshot is followed by the worker's fresh answer.
	/// `web_api` false keeps native session library reads/writes enabled but refuses Web API traffic.
	pub fn spawn(config_dir: PathBuf, cache_dir: &Path, web_api: bool, deliver: impl Fn(u64, Response) + Send + Sync + 'static) -> Self {
		let (tx, rx) = mpsc::channel::<Inbox>();
		let (urgent, interrupted) = tokio::sync::watch::channel(());
		let generation = Arc::new(AtomicU64::new(0));
		let mut store = Store::new(cache_dir);
		let account = saved_account(&config_dir);
		store.load(account.as_deref());
		store.bind_generation(generation.clone(), 0);
		let cached = store.connected();
		let snapshots = Arc::new(Mutex::new(store));
		let deliver: Arc<dyn Fn(u64, Response) + Send + Sync> = Arc::new(deliver);
		if let Some(cached) = cached {
			deliver(0, cached);
		}
		let runner =
			dispatch::Runner::new(config_dir, cache_dir, web_api, snapshots.clone(), deliver.clone(), interrupted, generation.clone());
		let worker =
			Self { tx, snapshots, deliver, prefetches: Mutex::new(Cache::new(snapshot::LISTINGS_KEPT, PAGES_TTL)), urgent, generation };
		let _ = thread::Builder::new().name("library".into()).stack_size(512 * 1024).spawn(move || runner.run(&rx));
		worker.send(Request::Connect);
		worker
	}

	pub fn send(&self, request: Request) {
		let generation = self.generation.load(Ordering::Acquire);
		if let Request::Prefetch(route) = &request {
			if matches!(route, Route::Search | Route::Library | Route::Folder(_)) {
				return;
			}
			let mut prefetches = self.prefetches.lock();
			if prefetches.get(route).is_some() {
				return;
			}
			prefetches.insert(route.clone(), ());
		}
		if let Request::Open(route, 0) = &request {
			let listing = self.snapshots.lock().listing(route);
			if let Some(listing) = listing {
				trace::hit("page", &format!("{route:?}"), "disk");
				(self.deliver)(generation, Response::Listing { route: route.clone(), offset: 0, listing });
			}
		} else if matches!(request, Request::Home) {
			let home = self.snapshots.lock().home();
			if let Some(home) = home {
				(self.deliver)(generation, Response::Home(home));
			}
		} else if let Request::Search(query) = &request {
			let results = self.snapshots.lock().search(query);
			if let Some(results) = results {
				trace::hit("search", query.trim(), "disk");
				(self.deliver)(generation, Response::Search { query: query.trim().to_owned(), results });
			}
		} else if matches!(request, Request::ForgetSnapshot) {
			self.snapshots.lock().invalidate();
			self.prefetches.lock().clear();
		}
		let foreground = schedule::foreground(&request);
		let _ = self.tx.send(Inbox::Request(generation, request));
		if foreground {
			self.urgent.send_replace(());
		}
	}

	/// Shares the player's authenticated session for library/profile/Connect I/O; API fallback requires no session.
	pub fn set_session(&self, session: Option<SessionReader>) {
		self.prefetches.lock().clear();
		let _ = self.tx.send(Inbox::Session(self.generation.load(Ordering::Acquire), session));
		self.urgent.send_replace(());
	}

	/// Invalidates queued and in-flight work before synchronously clearing account caches.
	pub fn forget(&self, generation: u64) {
		self.generation.store(generation, Ordering::Release);
		self.send(Request::ForgetSnapshot);
	}
}

/// Identity comes from saved session authorization, never from a library snapshot or Keychain.
fn saved_account(config: &Path) -> Option<String> {
	if !config.join("credentials.json").is_file() {
		return None;
	}
	librespot_core::cache::Cache::new(Some(config), Some(config), None::<&Path>, None).ok()?.credentials()?.username
}

/// Library state, its fallback Web API client and its optional player session.
pub(super) struct Session {
	pub(super) client:    Client,
	pub(super) player:    Option<Reader>,
	pub(super) me:        String,
	pub(super) me_name:   Option<String>,
	pub(super) snapshots: Arc<Mutex<Store>>,
	/// The user's playlists, to name recently played ones without a request.
	pub(super) playlists: Vec<Playlist>,
	/// The most recently used artists and other users' playlists, bounded per session.
	pub(super) artists:   Cache<String, Artist>,
	pub(super) others:    Cache<String, Option<Playlist>>,
	/// The last home page and when it was built: revisits within a minute reuse it.
	pub(super) home:      Option<(Instant, Home)>,
	/// The first page of the listings opened lately (an album, a playlist, Liked Songs, an artist's
	/// header), so visiting one again costs no request. A write drops what it changes.
	pub(super) pages:     Cache<Route, Listing>,
	pub(super) searches:  Cache<String, spotify::SearchResults>,
	pub(super) web:       Arc<spotify::WebPlayer>,
	/// Artist pages' popular songs and About, and the tint of a cover (`None`: Spotify had no color for it).
	pub(super) popular:   Cache<String, Popularity>,
	pub(super) colors:    Cache<String, Option<String>>,
}

/// How long the first page of a listing is reused, and how many listings are kept.
const PAGES_TTL: Duration = Duration::from_secs(180);

// These are anonymous/public metadata, not account data. Reconnects reuse the token and transport.
static WEB: LazyLock<Arc<spotify::WebPlayer>> = LazyLock::new(|| {
	let web = Arc::new(spotify::WebPlayer::new());
	#[cfg(not(test))]
	{
		let warming = web.clone();
		let _ = thread::Builder::new().name("library-warm".into()).stack_size(512 * 1024).spawn(move || warming.warm());
	}
	web
});

impl Session {
	fn new(client: Client, me: spotify::User, playlists: Vec<Playlist>, snapshots: Arc<Mutex<Store>>) -> Self {
		Self {
			client,
			player: None,
			me: me.id,
			me_name: me.display_name,
			snapshots,
			playlists,
			artists: Cache::new(128, Duration::from_secs(1800)),
			others: Cache::new(128, Duration::from_secs(1800)),
			home: None,
			pages: Cache::new(snapshot::LISTINGS_KEPT, PAGES_TTL),
			searches: Cache::new(snapshot::LISTINGS_KEPT, PAGES_TTL),
			web: WEB.clone(),
			popular: Cache::new(12, Duration::from_secs(1800)),
			colors: Cache::new(64, Duration::from_secs(24 * 3600)),
		}
	}

	pub(super) fn cache_artist(&mut self, id: String, artist: Artist) {
		self.artists.insert(id, artist);
	}

	pub(super) fn cache_other(&mut self, id: String, playlist: Option<Playlist>) {
		self.others.insert(id, playlist);
	}

	fn attach(&mut self, player: Option<Reader>, deliver: &dyn Fn(Response)) {
		self.player = player;
		self.home = None;
		self.pages.clear();
		self.popular.clear();
		if self.player.is_some()
			&& let Ok(playlists) = self.load_playlists()
		{
			self.playlists = playlists.clone();
			let response = Response::Playlists(playlists);
			self.snapshots.lock().record(&response);
			if let Some(player) = &mut self.player
				&& let Ok(entries) = player.rootlist()
			{
				let rootlist = Response::Rootlist { account: self.me.clone(), entries };
				self.snapshots.lock().record(&rootlist);
				deliver(rootlist);
			}
			deliver(response);
			let _ = snapshot::save(&self.snapshots);
		}
		self.publish_liked(deliver);
	}

	fn publish_liked(&mut self, deliver: &dyn Fn(Response)) {
		if let Some(reader) = &mut self.player
			&& let Ok(Some(tracks)) = reader.liked_update()
		{
			deliver(Response::LikedTracks(tracks));
		}
	}

	fn load_playlists(&mut self) -> spotify::Result<Vec<Playlist>> {
		let Some(reader) = &mut self.player else { return all_playlists(&self.client) };
		let mut playlists = reader.playlists()?;
		for playlist in &mut playlists {
			if let Some(known) = self.playlists.iter().find(|known| known.id == playlist.id) {
				playlist.public = playlist.public.or(known.public);
				playlist.owner.display_name.clone_from(&known.owner.display_name);
				if playlist.images.is_empty() {
					playlist.images.clone_from(&known.images);
				}
			}
		}
		for playlist in &mut playlists {
			playlist.normalize(&self.me, self.me_name.as_deref());
		}
		Ok(playlists)
	}

	/// Forgets the listing of `route` (after a write that changed it).
	fn forget(&mut self, route: &Route) {
		self.pages.remove(route);
		if let Some(player) = &mut self.player {
			player.forget(route);
		}
		self.snapshots.lock().forget(route);
		let _ = snapshot::save(&self.snapshots);
	}
}

/// Borrows the worker's account and membership state for one request.
struct RequestContext<'a> {
	config:        &'a Path,
	snapshots:     &'a Arc<Mutex<Store>>,
	session:       &'a mut Option<Session>,
	player:        &'a mut Option<Reader>,
	added:         &'a mut Membership,
	refresh_added: &'a mut bool,
	web_api:       bool,
	deliver:       &'a dyn Fn(Response),
}

fn serve(context: RequestContext<'_>, request: Request) {
	let RequestContext { config, snapshots, session, player, added, refresh_added, web_api, deliver } = context;
	let label = trace::enabled().then(|| request.label());
	let output = |response| {
		let changed = snapshots.lock().record(&response);
		deliver(response);
		if changed {
			let _ = snapshot::save(snapshots);
		}
	};
	match request {
		Request::Connect => connect(config, snapshots, session, player, web_api, &output),
		Request::QueueContext(route) => {
			// Connect's reader survives a library reconnect even when the Web API is disabled.
			let reader = session.as_mut().and_then(|s| s.player.as_mut()).or(player.as_mut());
			output(queue_context(reader, &route));
		},
		Request::AddContextToPlaylist(route) => {
			let reader = session.as_mut().and_then(|s| s.player.as_mut()).or(player.as_mut());
			output(add_context_to_playlist(reader, route));
		},
		Request::RememberPlayback(position) => remember_playback(config, position, deliver),
		Request::ForgetSnapshot => forget_library(snapshots, session, player, added, refresh_added, deliver),
		other => {
			let speculative = matches!(other, Request::Prefetch(_));
			let Some(s) = session.as_mut() else {
				// Nothing can load without an account: say so, so no view keeps
				// waiting for an answer that will never come.
				if !speculative {
					deliver(Response::NeedsLogin);
				}
				return;
			};
			if let Request::Prefetch(route) = &other {
				match route {
					Route::Home => {
						let _ = home(s, &|_| {});
					},
					Route::Search | Route::Library | Route::Folder(_) => {},
					_ => {
						let _ = open(s, route, 0, &|_| {});
					},
				}
				return;
			}
			let route = match &other {
				Request::Open(route, _) | Request::Prefetch(route) => Some(route.clone()),
				_ => None,
			};
			// Writes need permissions a sign-in from an older turntable lacks.
			let write = is_write(&other);
			let result = match other {
				Request::LastPlayback => last_playback(config, s, s.player.as_ref().or(player.as_ref())).map(Some),
				Request::Pins => pins_response(&s.me, "load", super::pins::load(config, &s.me)),
				Request::SetPinned { uri, pinned } => pins_response(&s.me, "save", super::pins::set(config, &s.me, &uri, pinned)),
				Request::FolderExpansion => super::folders::load(config, &s.me)
					.map(|expanded| Some(Response::FolderExpansion { account: s.me.clone(), expanded }))
					.map_err(|error| spotify::Error::Parse(format!("Couldn't load folder expansion: {error}"))),
				Request::SetFolderExpanded { id, expanded } => super::folders::set(config, &s.me, &id, expanded)
					.map(|expanded| Some(Response::FolderExpansion { account: s.me.clone(), expanded }))
					.map_err(|error| spotify::Error::Parse(format!("Couldn't save folder expansion: {error}"))),
				other => run(s, added, refresh_added, other, &output),
			}
			.and_then(|response| {
				if matches!(&response, Some(Response::Playlists(_) | Response::Created(_))) {
					folders::publish(s, &output)?;
				}
				Ok(response)
			});
			match result {
				Ok(Some(response)) => {
					// What the Web API lacks of a page follows it, so nothing waits for it: an album's play
					// counts, and the cover's tint (all the page's tracks share the first page's).
					let follow = match &response {
						Response::Listing { route: route @ (Route::Album(_) | Route::Playlist(_)), offset, listing } => {
							Some((route.clone(), *offset, listing.cover_url.clone()))
						},
						_ => None,
					};
					output(response);
					if let Some((route, offset, cover)) = follow {
						enrich_listing(s, route, offset, cover.as_deref(), &output);
					}
				},
				Ok(None) => {},
				Err(_) if speculative => {},
				Err(spotify::Error::NativeAuthRequired) => {
					snapshots.lock().clear();
					*session = None;
					deliver(Response::NeedsLogin);
				},
				Err(e) if write && e.is_forbidden() => {
					deliver(Response::Error { route, message: "Spotify doesn't allow that change for this item.".into() });
				},
				Err(e) => {
					if matches!(e, spotify::Error::Network(_)) {
						deliver(Response::Offline(true));
					}
					deliver(Response::Error { route, message: e.user_message() });
				},
			}
		},
	}
	if let Some(label) = label {
		trace::mark("served", &label);
	}
}

fn pins_response(account: &str, operation: &str, result: std::io::Result<Vec<String>>) -> spotify::Result<Option<Response>> {
	result
		.map(|uris| Some(Response::Pins { account: account.to_owned(), uris }))
		.map_err(|error| spotify::Error::Parse(format!("Couldn't {operation} library pins: {error}")))
}

/// Pending native requests have finished before this account's caches are removed.
fn forget_library(
	snapshots: &Mutex<Store>,
	session: &mut Option<Session>,
	player: &mut Option<Reader>,
	added: &mut Membership,
	refresh_added: &mut bool,
	deliver: &dyn Fn(Response),
) {
	snapshots.lock().clear();
	*session = None;
	*player = None;
	*refresh_added = false;
	added.forget(deliver);
}

fn remember_playback(config: &Path, position: super::PlaybackPosition, deliver: &dyn Fn(Response)) {
	if let Err(error) = super::restore::remember(config, position) {
		deliver(Response::Error { route: None, message: format!("Couldn't save playback position: {error}") });
	}
}

fn is_write(request: &Request) -> bool {
	matches!(
		request,
		Request::SetFollowed(..)
			| Request::CreatePlaylist(..)
			| Request::RenamePlaylist(..)
			| Request::SetCollaborative(..)
			| Request::CreateFolder
			| Request::RenameFolder(..)
			| Request::DeleteFolder(_)
			| Request::MoveToFolder { .. }
			| Request::SetExcluded { .. }
			| Request::AddToPlaylist(..)
			| Request::RemoveFromPlaylist(..)
			| Request::MovePlaylistRow { .. }
	)
}

fn enrich_listing(s: &mut Session, route: Route, offset: u32, cover: Option<&str>, deliver: &dyn Fn(Response)) {
	let web = &s.web;
	let colors = &mut s.colors;
	let (plays, color) = par::join(
		|| match &route {
			Route::Album(id) => web.album_plays(id).ok(),
			_ => None,
		},
		|| if offset == 0 { cover.and_then(|url| accent(web, colors, url)) } else { None },
	);
	if let Some(plays) = plays {
		deliver(Response::Plays(route.clone(), plays));
	}
	if let Some(color) = color {
		deliver(Response::Accent(route, color));
	}
}

fn connect(
	config: &Path,
	snapshots: &Arc<Mutex<Store>>,
	session: &mut Option<Session>,
	player: &mut Option<Reader>,
	web_api: bool,
	deliver: &dyn Fn(Response),
) {
	if let Some(reader) = player.take() {
		connect_session(snapshots, session, reader, web_api, deliver);
		return;
	}
	// Native authentication is owned by the engine. Never probe old OAuth/Keychain accounts.
	if saved_account(config).is_none() {
		deliver(Response::NeedsLogin);
	}
}

/// A player session needs neither the separate Web API login nor `/me` to open the library.
fn connect_session(
	snapshots: &Arc<Mutex<Store>>,
	session: &mut Option<Session>,
	mut reader: Reader,
	web_api: bool,
	deliver: &dyn Fn(Response),
) {
	let (identity, plan) = reader.identity();
	if is_free(plan.as_deref()) {
		snapshots.lock().clear();
		*session = None;
		deliver(Response::SignInRejected(crate::engine::SignInError::PremiumRequired {
			account: identity.display_name.or(Some(identity.id)),
			product: plan,
		}));
		return;
	}
	let (cached, home, roots) = {
		let mut store = snapshots.lock();
		store.load(Some(&identity.id));
		(store.connected(), store.home(), store.rootlist())
	};
	let client = match session.take().filter(|previous| previous.me == identity.id) {
		Some(previous) => previous.client,
		None => Client::without_token(web_api),
	};
	let (me, playlists) = if let Some(Response::Connected { me, playlists }) = cached {
		trace::hit("library", "Connected", "disk");
		deliver(Response::Connected { me: me.clone(), playlists: playlists.clone() });
		if let Some(roots) = roots {
			deliver(roots);
		}
		if let Some(home) = home {
			trace::hit("library", "Home", "disk");
			deliver(Response::Home(home));
		}
		(me, playlists)
	} else {
		(identity, Vec::new())
	};
	let mut s = Session::new(client, me, playlists, snapshots.clone());
	let loaded = reader.profile(s.me_name.as_deref()).and_then(|(me, plan)| {
		if is_free(plan.as_deref()) {
			return Err(spotify::Error::PremiumRequired);
		}
		Ok((me, reader.playlists()?, reader.rootlist()?))
	});
	match loaded {
		Ok((me, mut playlists, entries)) => {
			for playlist in &mut playlists {
				playlist.normalize(&me.id, me.display_name.as_deref());
			}
			s.me = me.id.clone();
			s.me_name.clone_from(&me.display_name);
			s.playlists = playlists.clone();
			deliver(Response::Offline(false));
			deliver(Response::Connected { me, playlists });
			deliver(Response::Rootlist { account: s.me.clone(), entries });
		},
		Err(spotify::Error::NativeAuthRequired) => {
			snapshots.lock().clear();
			deliver(Response::NeedsLogin);
			return;
		},
		Err(spotify::Error::PremiumRequired) => {
			snapshots.lock().clear();
			deliver(Response::SignInRejected(crate::engine::SignInError::PremiumRequired {
				account: s.me_name.clone().or(Some(s.me.clone())),
				product: None,
			}));
			return;
		},
		Err(error) => {
			deliver(Response::Offline(matches!(error, spotify::Error::Network(_))));
			deliver(Response::Error { route: None, message: error.user_message() });
		},
	}
	s.player = Some(reader);
	s.publish_liked(deliver);
	*session = Some(s);
}
/// A player request's outcome as the UI shows it: a sign-in problem still ends the session (the
/// caller's `?`), anything else becomes a sentence for the user.
fn player_reply<T>(result: spotify::Result<T>) -> spotify::Result<Result<T, String>> {
	match result {
		Ok(value) => Ok(Ok(value)),
		Err(e @ spotify::Error::NativeAuthRequired) => Err(e),
		Err(spotify::Error::DeviceNotFound) => Ok(Err("Spotify doesn't see that device anymore. Open Spotify on it and try again.".into())),
		Err(e) => Ok(Err(e.user_message())),
	}
}

/// Whether a subscription (session `type`/`product`, or fallback `/me`) is Spotify's free plan.
/// Unknown attributes are not a reason to spend Web API quota; playback supplies its own error.
fn is_free(plan: Option<&str>) -> bool {
	matches!(plan, Some("free" | "open"))
}

/// Resolve every native page without losing duplicates or raw playlist offsets.
fn context_pages(
	mut page: impl FnMut(u32) -> spotify::Result<(String, Vec<String>, Option<u32>)>,
) -> spotify::Result<(Vec<String>, String)> {
	let mut offset = 0;
	let mut uris = Vec::new();
	let mut title = None;
	loop {
		let (name, items, next) = page(offset)?;
		title.get_or_insert(name);
		uris.extend(items);
		match next {
			Some(next) if next > offset => offset = next,
			Some(_) => return Err(spotify::Error::Parse("Spotify returned a non-advancing context page.".into())),
			None => return Ok((uris, title.unwrap_or_default())),
		}
	}
}

fn queue_context(player: Option<&mut Reader>, route: &Route) -> Response {
	let Some(player) = player else {
		return Response::Error { route: None, message: "Start the player before adding a context to its queue.".into() };
	};
	let result = context_pages(|offset| player.context_page(route, offset));
	match result {
		Ok((mut uris, title)) => {
			uris.retain(|uri| uri.starts_with("spotify:track:") || uri.starts_with("spotify:episode:"));
			Response::Queue(uris, title)
		},
		Err(error) => Response::Error { route: None, message: error.user_message() },
	}
}

fn add_context_to_playlist(player: Option<&mut Reader>, route: Route) -> Response {
	let Some(player) = player else {
		return Response::Error { route: None, message: "Start the player before adding an album or playlist to a playlist.".into() };
	};
	match context_pages(|offset| player.context_page(&route, offset)) {
		Ok((mut uris, title)) => {
			uris.retain(|uri| uri.starts_with("spotify:track:"));
			Response::ContextToPlaylist(route, uris, title)
		},
		Err(error) => Response::Error { route: None, message: error.user_message() },
	}
}

fn playlist_details(s: &mut Session, id: &str) -> spotify::Result<Playlist> {
	let mut playlist = match &mut s.player {
		Some(player) => {
			// Playlist4 and base permissions carry the full current header.
			player.playlists()?;
			player.playlist(id, 0, false)?.0
		},
		None => s.client.playlist(id)?,
	};
	playlist.normalize(&s.me, s.me_name.as_deref());
	if let Some(known) = s.playlists.iter_mut().find(|known| known.id == id) {
		*known = playlist.clone();
	}
	Ok(playlist)
}

fn set_collaborative(s: &mut Session, id: &str, collaborative: bool) -> spotify::Result<Response> {
	match &mut s.player {
		Some(player) => player.set_collaborative(id, collaborative)?,
		None => {
			if s.client.playlist(id)?.owner.id != s.me {
				return Err(spotify::Error::Parse("Only the owner can change playlist collaboration.".into()));
			}
			s.client.set_collaborative(id, collaborative)?;
		},
	}
	s.forget(&Route::Playlist(id.to_owned()));
	s.playlists = s.load_playlists()?;
	Ok(Response::Playlists(s.playlists.clone()))
}

fn exclusions(s: &mut Session) -> spotify::Result<Option<Response>> {
	let Some(player) = &mut s.player else { return Ok(None) };
	Ok(Some(Response::Exclusions { account: s.me.clone(), uris: player.excluded()? }))
}

fn set_excluded(s: &mut Session, uri: &str, excluded: bool) -> spotify::Result<Response> {
	let player =
		s.player.as_mut().ok_or_else(|| spotify::Error::Parse("Start the player before changing taste-profile exclusions.".into()))?;
	player.set_excluded(uri, excluded)?;
	Ok(Response::Exclusions { account: s.me.clone(), uris: player.excluded()? })
}

fn last_playback(config: &Path, s: &Session, player: Option<&Reader>) -> spotify::Result<Response> {
	let playback = super::restore::recent(config, &s.me, &s.client, player, &s.playlists, &s.snapshots)?;
	Ok(Response::LastPlayback { account: s.me.clone(), playback })
}

fn run(
	s: &mut Session,
	added: &mut Membership,
	refresh_added: &mut bool,
	request: Request,
	deliver: &dyn Fn(Response),
) -> spotify::Result<Option<Response>> {
	Ok(Some(match request {
		request @ (Request::CreateFolder | Request::RenameFolder(..) | Request::DeleteFolder(_) | Request::MoveToFolder { .. }) => {
			return folders::request(s, request, deliver);
		},
		Request::Home => Response::Home(home(s, deliver)?),
		Request::Albums => Response::Albums(match &mut s.player {
			Some(player) => player.albums(500)?,
			None => all_albums(&s.client)?,
		}),
		Request::Following => Response::Following(match &mut s.player {
			Some(player) => player.followed(500)?,
			None => all_followed(&mut s.client)?,
		}),
		request @ (Request::Artist(_) | Request::ArtistPopular(_) | Request::ArtistCard(_) | Request::Credits(_)) => {
			return artist_request(s, request);
		},
		Request::SetFollowed(uri, follow) => {
			let response = set_followed(s, &uri, follow)?;
			*refresh_added |= uri.starts_with("spotify:playlist:");
			if !follow && uri.starts_with("spotify:playlist:") {
				added.deleted(&uri, deliver);
			}
			if !follow {
				deliver(Response::Done("Removed from Your Library".into()));
			}
			response
		},
		Request::CreatePlaylist(name, uris) => {
			let response = create_playlist(s, &name, &uris)?;
			if let Response::Created(playlist) = &response {
				added.created(&playlist.uri, &name, &uris, deliver);
			}
			*refresh_added = true;
			response
		},
		Request::RenamePlaylist(id, changes) => {
			let response = rename_playlist(s, added, id, &changes, deliver)?;
			*refresh_added = true;
			response
		},
		Request::PlaylistDetails(id) => Response::PlaylistDetails(playlist_details(s, &id)?),
		Request::SetCollaborative(id, collaborative) => set_collaborative(s, &id, collaborative)?,
		Request::Exclusions => return exclusions(s),
		Request::SetExcluded { uri, excluded } => set_excluded(s, &uri, excluded)?,
		Request::AddToPlaylist(id, name, uris) => {
			let response = append_playlist(s, added, id, &name, &uris, deliver)?;
			*refresh_added = true;
			response
		},
		Request::RemoveFromPlaylist(id, uri, row) => {
			match &mut s.player {
				Some(player) => player.remove_from_playlist(&id, &uri, row)?,
				None => s.client.remove_playlist_row(&id, &uri, row)?,
			}
			added.removed_row(&format!("spotify:playlist:{id}"), &uri, deliver);
			*refresh_added = true;
			s.forget(&Route::Playlist(id.clone()));
			Response::Removed(id, uri, row)
		},
		Request::MovePlaylistRow { id, from, to, source, target } => move_playlist_row(s, id, from, to, &source, &target)?,
		Request::CheckLiked(uri) => check_liked(s, uri, deliver)?,
		Request::SetLiked(uri, liked) => set_liked(s, &uri, liked, deliver)?,
		Request::Open(route, offset) => open_page(s, route, offset, deliver)?,
		Request::Search(query) => search(s, query)?,
		request @ (Request::Devices | Request::Playback | Request::Transfer { .. } | Request::Control { .. }) => {
			return connect_request(s, request);
		},
		Request::Connect
		| Request::Prefetch(_)
		| Request::ForgetSnapshot
		| Request::RememberPlayback(_)
		| Request::LastPlayback
		| Request::Pins
		| Request::SetPinned { .. }
		| Request::FolderExpansion
		| Request::SetFolderExpanded { .. }
		| Request::AddContextToPlaylist(_)
		| Request::QueueContext(_) => return Ok(None),
	}))
}

fn artist_request(s: &mut Session, request: Request) -> spotify::Result<Option<Response>> {
	Ok(Some(match request {
		Request::Artist(id) => Response::Artist(artist(s, &id)?),
		Request::ArtistPopular(id) => {
			let popular = popularity(s, &id)?;
			Response::ArtistPopular(id, popular)
		},
		Request::ArtistCard(id) => match s.web.overview(&id) {
			Ok(overview) => Response::ArtistCard(id, overview),
			Err(_) => return Ok(None),
		},
		Request::Credits(uri) => match s.web.credits(id_of(&uri)) {
			Ok(credits) => Response::Credits(uri, credits),
			Err(_) => return Ok(None),
		},
		_ => return Ok(None),
	}))
}

fn open_page(s: &mut Session, route: Route, offset: u32, deliver: &dyn Fn(Response)) -> spotify::Result<Response> {
	let listing = open(s, &route, offset, &|listing| {
		if offset == 0 {
			deliver(Response::Listing { route: route.clone(), offset, listing });
		}
	})?;
	if route == Route::Liked {
		s.publish_liked(deliver);
	}
	Ok(Response::Listing { route, offset, listing })
}

fn search(s: &mut Session, query: String) -> spotify::Result<Response> {
	let trimmed = query.trim();
	let query = if trimmed.len() == query.len() { query } else { trimmed.to_owned() };
	let results = match s.searches.get(&query) {
		Some(results) => results,
		None => {
			let results = search_result(s.web.search(&query, 0), &s.client, &query)?;
			s.searches.insert(query.clone(), results.clone());
			results
		},
	};
	Ok(Response::Search { query, results })
}

fn search_result(web: Result<spotify::SearchResults, String>, client: &Client, query: &str) -> spotify::Result<spotify::SearchResults> {
	match web {
		Ok(results) => Ok(results),
		Err(_) if client.has_token() => client.search(query, 0),
		Err(message) => Err(spotify::Error::Network(message)),
	}
}

fn connect_request(s: &Session, request: Request) -> spotify::Result<Option<Response>> {
	Ok(Some(match request {
		Request::Devices => Response::Devices(player_reply(match &s.player {
			Some(player) => player.devices(),
			None => s.client.devices(),
		})?),
		Request::Playback => Response::Playback(player_reply(match &s.player {
			Some(player) => player.playback(),
			None => s.client.playback(),
		})?),
		Request::Transfer { id, play } => match player_reply(match &s.player {
			Some(player) => player.transfer(&id, play),
			None => s.client.transfer(&id, play),
		})? {
			Ok(()) => Response::Transferred,
			Err(message) => Response::PlayerError(message),
		},
		Request::Control { device, command, revision } => match player_reply(match &s.player {
			Some(player) => player.control(&device, command),
			None => s.client.control(&device, &command),
		})? {
			Ok(()) => return Ok(None),
			Err(message) => Response::ControlFailed { revision, message },
		},
		_ => return Ok(None),
	}))
}

fn check_liked(s: &mut Session, uri: String, deliver: &dyn Fn(Response)) -> spotify::Result<Response> {
	let liked = match &mut s.player {
		Some(player) => player.contains(&uri)?,
		None => s.client.library_contains(&[&uri])?.first().copied().unwrap_or(false),
	};
	s.publish_liked(deliver);
	Ok(Response::Liked(uri, liked))
}

fn set_liked(s: &mut Session, uri: &str, liked: bool, deliver: &dyn Fn(Response)) -> spotify::Result<Response> {
	match &mut s.player {
		Some(player) => player.set_saved(uri, liked)?,
		None if liked => s.client.save(&[uri])?,
		None => s.client.unsave(&[uri])?,
	}
	s.pages.remove(&Route::Liked);
	s.snapshots.lock().forget(&Route::Liked);
	let _ = snapshot::save(&s.snapshots);
	s.publish_liked(deliver);
	Ok(Response::Liked(uri.to_owned(), liked))
}

fn set_followed(s: &mut Session, uri: &str, follow: bool) -> spotify::Result<Response> {
	match &mut s.player {
		Some(player) => player.set_saved(uri, follow)?,
		None if follow => s.client.save(&[uri])?,
		None => s.client.unsave(&[uri])?,
	}
	if !follow && uri.starts_with("spotify:playlist:") {
		let id = id_of(uri);
		s.playlists.retain(|playlist| playlist.id != id);
		let route = Route::Playlist(id.to_owned());
		s.pages.remove(&route);
		if let Some(player) = &mut s.player {
			player.forget(&route);
		}
		s.snapshots.lock().remove_playlist(id);
		let _ = snapshot::save(&s.snapshots);
		return Ok(Response::Playlists(s.playlists.clone()));
	}
	s.pages.clear();
	s.snapshots.lock().forget_listings();
	let _ = snapshot::save(&s.snapshots);
	Ok(if uri.starts_with("spotify:playlist:") {
		s.playlists = s.load_playlists()?;
		Response::Playlists(s.playlists.clone())
	} else if uri.starts_with("spotify:album:") {
		s.home = None;
		Response::Albums(match &mut s.player {
			Some(player) => player.albums(500)?,
			None => all_albums(&s.client)?,
		})
	} else {
		// Following an artist changes Your Library and its home shelf.
		s.home = None;
		Response::Following(match &mut s.player {
			Some(player) => player.followed(500)?,
			None => all_followed(&mut s.client)?,
		})
	})
}

fn create_playlist(s: &mut Session, name: &str, uris: &[String]) -> spotify::Result<Response> {
	let mut playlist = match &mut s.player {
		Some(player) => player.create_playlist(name, uris)?,
		None => {
			let playlist = s.client.create_playlist(name)?;
			if !uris.is_empty() {
				let uris: Vec<&str> = uris.iter().map(String::as_str).collect();
				s.client.add_to_playlist(&playlist.id, &uris)?;
			}
			playlist
		},
	};
	playlist.normalize(&s.me, s.me_name.as_deref());
	s.playlists.insert(0, playlist.clone());
	Ok(Response::Created(playlist))
}

fn rename_playlist(
	s: &mut Session,
	added: &mut Membership,
	id: String,
	changes: &spotify::PlaylistChanges,
	deliver: &dyn Fn(Response),
) -> spotify::Result<Response> {
	match &mut s.player {
		Some(player) => player.rename_playlist(&id, changes)?,
		None => s.client.rename_playlist(&id, changes)?,
	}
	if let Some(name) = &changes.name {
		added.renamed(&format!("spotify:playlist:{id}"), name, deliver);
	}
	if let Some(public) = changes.public
		&& let Some(playlist) = s.playlists.iter_mut().find(|playlist| playlist.id == id)
	{
		playlist.public = Some(public);
	}
	s.forget(&Route::Playlist(id));
	s.playlists = s.load_playlists()?;
	Ok(Response::Playlists(s.playlists.clone()))
}

fn append_playlist(
	s: &mut Session,
	added: &mut Membership,
	id: String,
	name: &str,
	uris: &[String],
	deliver: &dyn Fn(Response),
) -> spotify::Result<Response> {
	match &mut s.player {
		Some(player) => player.add_to_playlist(&id, uris)?,
		None => {
			let borrowed: Vec<&str> = uris.iter().map(String::as_str).collect();
			s.client.add_to_playlist(&id, &borrowed)?;
		},
	}
	added.appended(&format!("spotify:playlist:{id}"), uris, deliver);
	s.forget(&Route::Playlist(id));
	Ok(Response::Done(format!("Added to {name}")))
}

fn move_playlist_row(s: &mut Session, id: String, from: u32, to: u32, source: &str, target: &str) -> spotify::Result<Response> {
	match &mut s.player {
		Some(player) => player.move_playlist_row(&id, from, to, source, target)?,
		None => return Err(spotify::Error::Network("Start the player before reordering a playlist".into())),
	}
	s.forget(&Route::Playlist(id.clone()));
	// Membership is order-independent: reordering needs no index refresh.
	Ok(Response::Moved(id, from, to))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn context_actions_require_a_player() {
		for response in
			[queue_context(None, &Route::Album("album".into())), add_context_to_playlist(None, Route::Playlist("playlist".into()))]
		{
			assert!(matches!(response, Response::Error { route: None, message } if !message.is_empty()));
		}
	}

	#[test]
	fn context_actions_read_all_pages_preserving_order_and_duplicates() -> spotify::Result<()> {
		let mut requested = Vec::new();
		let (uris, title) = context_pages(|offset| {
			requested.push(offset);
			Ok(("Album".into(), vec!["spotify:track:a".into()], if offset == 0 { Some(50) } else { None }))
		})?;
		assert_eq!(requested, [0, 50]);
		assert_eq!(uris, ["spotify:track:a", "spotify:track:a"]);
		assert_eq!(title, "Album");
		Ok(())
	}

	#[test]
	fn context_actions_do_not_succeed_with_a_partial_page_failure() {
		let result = context_pages(|offset| {
			if offset == 0 { Ok(("Playlist".into(), Vec::new(), Some(50))) } else { Err(spotify::Error::Parse("page failed".into())) }
		});
		assert!(result.is_err());
	}

	#[test]
	fn context_pages_reject_nonadvancing_offsets() {
		assert!(context_pages(|offset| Ok(("Album".into(), Vec::new(), Some(offset)))).is_err());
	}

	#[test]
	fn all_worker_metadata_insertions_obey_the_budget() {
		let client = Client::open(Arc::new(|| Err(spotify::Error::AuthRequired)), false);
		let snapshots = Arc::new(Mutex::new(Store::new(&std::env::temp_dir())));
		let mut session = Session::new(client, spotify::User { id: "test".into(), display_name: None }, Vec::new(), snapshots);
		for index in 0..400 {
			let id = format!("artist-{index}");
			let artist = Artist { uri: format!("spotify:artist:{id}"), name: id.clone(), images: Vec::new(), genres: Vec::new() };
			session.cache_artist(id, artist);
			session.cache_other(format!("playlist-{index}"), None);
		}
		assert!(session.artists.get("artist-0").is_none());
		assert!(session.artists.get("artist-399").is_some());
		assert!(session.artists.get("artist-272").is_some(), "one insertion evicts one old entry, not the entire cache");
		assert!(session.others.get("playlist-0").is_none());
		assert!(session.others.get("playlist-399").is_some());
		assert!(session.others.get("playlist-272").is_some());
	}

	#[test]
	fn failed_web_search_without_a_token_preserves_the_native_account() {
		let directory = tempfile::tempdir().unwrap();
		let client = Client::without_token(true);
		assert!(!client.has_token());
		assert!(
			matches!(search_result(Err("search hash rotated".into()), &client, "query"), Err(spotify::Error::Network(message)) if message == "search hash rotated")
		);
		let snapshots = Arc::new(Mutex::new(Store::new(directory.path())));
		let me = spotify::User { id: "native-account".into(), display_name: None };
		snapshots.lock().record(&Response::Connected { me: me.clone(), playlists: Vec::new() });
		snapshot::save(&snapshots).unwrap();
		let mut session = Some(Session::new(client, me, Vec::new(), snapshots.clone()));
		let mut player = None;
		let mut added = Membership::new(directory.path());
		let mut refresh_added = false;
		let replies = std::cell::RefCell::new(Vec::new());
		// This deliberately missing Web API token fails before any transport call.
		serve(
			RequestContext {
				config:        directory.path(),
				snapshots:     &snapshots,
				session:       &mut session,
				player:        &mut player,
				added:         &mut added,
				refresh_added: &mut refresh_added,
				web_api:       true,
				deliver:       &|response| replies.borrow_mut().push(response),
			},
			Request::Open(Route::Liked, 0),
		);
		assert!(session.is_some());
		assert!(snapshots.lock().connected().is_some());
		assert!(replies.borrow().iter().any(|response| matches!(response, Response::Error { .. })));
		assert!(!replies.borrow().iter().any(|response| matches!(response, Response::NeedsLogin)));
		assert!(player_reply::<()>(Err(spotify::Error::AuthRequired)).unwrap().is_err());
		assert!(matches!(player_reply::<()>(Err(spotify::Error::NativeAuthRequired)), Err(spotify::Error::NativeAuthRequired)));
	}

	#[test]
	fn prefetch_keeps_only_the_memory_page_without_responses_enrichment_or_snapshot_writes() {
		let directory = tempfile::tempdir().unwrap();
		let snapshots = Arc::new(Mutex::new(Store::new(directory.path())));
		let me = spotify::User { id: "account".into(), display_name: None };
		snapshots.lock().record(&Response::Connected { me: me.clone(), playlists: Vec::new() });
		snapshot::save(&snapshots).unwrap();
		let route = Route::Album("album".into());
		let mut connected = Session::new(Client::without_token(false), me, Vec::new(), snapshots.clone());
		connected.pages.insert(route.clone(), Listing { title: "Cached album".into(), ..Default::default() });
		let mut session = Some(connected);
		let mut player = None;
		let mut added = Membership::new(directory.path());
		let mut refresh_added = false;
		serve(
			RequestContext {
				config:        directory.path(),
				snapshots:     &snapshots,
				session:       &mut session,
				player:        &mut player,
				added:         &mut added,
				refresh_added: &mut refresh_added,
				web_api:       false,
				deliver:       &|response| panic!("prefetch published {response:?}"),
			},
			Request::Prefetch(route.clone()),
		);
		assert!(session.as_mut().unwrap().pages.get(&route).is_some());
		assert!(snapshots.lock().listing(&route).is_none());
		let mut disk = Store::new(directory.path());
		disk.load(Some("account"));
		assert!(disk.listing(&route).is_none());
	}
}
