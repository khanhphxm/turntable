//! The library worker thread, its connected session and request dispatch.

use std::{
	collections::HashMap,
	path::{Path, PathBuf},
	sync::{Arc, Mutex, PoisonError, mpsc},
	thread,
	time::{Duration, Instant},
};

use super::{
	Home, Listing, Popularity, Request, Response, Route,
	added::Membership,
	artist::{about, artist, popularity},
	home::home,
	pages::{accent, all_albums, all_followed, all_playlists, open},
	session::Reader,
	snapshot::{self, Store},
};
use crate::{
	engine::SessionReader,
	net::{cache::Cache, par, trace},
	sources::external,
	spotify::{self, Artist, Client, Playlist, id_of},
};

pub struct Worker {
	tx:        mpsc::Sender<Inbox>,
	snapshots: Arc<Mutex<Store>>,
	deliver:   Arc<dyn Fn(Response) + Send + Sync>,
}

enum Inbox {
	Request(Request),
	Session(Option<SessionReader>),
}

impl Worker {
	/// Starts the worker. Disk listings can also be delivered by `send`, without waiting for
	/// an in-flight network request; every snapshot is followed by the worker's fresh answer.
	/// `web_api` false keeps native session reads enabled but refuses Web API traffic.
	pub fn spawn(config_dir: PathBuf, cache_dir: &Path, web_api: bool, deliver: impl Fn(Response) + Send + Sync + 'static) -> Self {
		let (tx, rx) = mpsc::channel::<Inbox>();
		let snapshots = Arc::new(Mutex::new(Store::new(cache_dir)));
		let deliver: Arc<dyn Fn(Response) + Send + Sync> = Arc::new(deliver);
		let mut added = Membership::new(cache_dir);
		let worker = Self { tx, snapshots: snapshots.clone(), deliver: deliver.clone() };
		let _ = thread::Builder::new().name("library".into()).stack_size(512 * 1024).spawn(move || {
			let mut session: Option<Session> = None;
			let mut player: Option<Reader> = None;
			let mut refresh_added = false;
			let mut waiting = None;
			while let Some(request) = waiting.take().or_else(|| rx.recv().ok()) {
				match request {
					Inbox::Session(reader) => {
						added.attach(reader.clone(), deliver.as_ref());
						refresh_added = reader.is_some();
						player = reader.map(Reader::new);
						deliver(Response::LikedTracks(Arc::from([])));
						if let Some(s) = &mut session {
							let matched = player.as_ref().is_some_and(|p| p.matches(&s.me));
							s.attach(if matched { player.take() } else { None }, deliver.as_ref());
						}
						if let Some(reader) = &mut player
							&& let Ok(Some(tracks)) = reader.liked_update()
						{
							deliver(Response::LikedTracks(tracks));
						}
					},
					Inbox::Request(request) => {
						if matches!(request, Request::Connect | Request::Login)
							&& let Some(s) = &mut session
							&& s.player.is_some()
						{
							player = s.player.take();
						}
						serve(
							&config_dir,
							&snapshots,
							&mut session,
							&mut player,
							&mut added,
							&mut refresh_added,
							web_api,
							request,
							deliver.as_ref(),
						);
					},
				}
				// At the end of a request burst, drop shared answers and stale listings.
				waiting = rx.try_recv().ok();
				if waiting.is_none() {
					if let Some(s) = &mut session {
						s.client.forget_recent();
						s.pages.evict_expired();
					}
					if std::mem::take(&mut refresh_added) {
						added.refresh(deliver.as_ref());
					}
				}
			}
		});
		worker.send(Request::Connect);
		worker
	}

	pub fn send(&self, request: Request) {
		if let Request::Open(route, 0) = &request {
			let listing = self.snapshots.lock().unwrap_or_else(PoisonError::into_inner).listing(route);
			if let Some(listing) = listing {
				trace::hit("page", &format!("{route:?}"), "disk");
				(self.deliver)(Response::Listing { route: route.clone(), offset: 0, listing });
			}
		} else if matches!(request, Request::Logout | Request::ForgetSnapshot | Request::Login) {
			self.snapshots.lock().unwrap_or_else(PoisonError::into_inner).clear();
		}
		let _ = self.tx.send(Inbox::Request(request));
	}

	/// Shares the player's authenticated session; before it arrives reads use the Web API.
	pub fn set_session(&self, session: Option<SessionReader>) {
		let _ = self.tx.send(Inbox::Session(session));
	}
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
	/// Artists and others' playlists fetched before. Each map keeps at most 128
	/// entries, then starts a fresh batch; every insertion uses the budget helpers.
	pub(super) artists:   HashMap<String, Artist>,
	pub(super) others:    HashMap<String, Option<Playlist>>,
	/// The last home page and when it was built: revisits within a minute reuse it.
	pub(super) home:      Option<(Instant, Home)>,
	/// The first page of the listings opened lately (an album, a playlist, Liked Songs, an artist's
	/// header), so visiting one again costs no request. A write drops what it changes.
	pub(super) pages:     Cache<Route, Listing>,
	pub(super) external:  external::Client,
	pub(super) web:       spotify::WebPlayer,
	/// Artist pages' popular songs and About, and the tint of a cover (`None`: Spotify had no color for it).
	pub(super) popular:   Cache<String, Popularity>,
	pub(super) abouts:    Cache<String, external::About>,
	pub(super) colors:    Cache<String, Option<String>>,
}

/// How long the first page of a listing is reused, and how many listings are kept.
const PAGES_TTL: Duration = Duration::from_secs(180);

impl Session {
	fn new(client: Client, me: spotify::User, playlists: Vec<Playlist>, snapshots: Arc<Mutex<Store>>) -> Self {
		Self {
			client,
			player: None,
			me: me.id,
			me_name: me.display_name,
			snapshots,
			playlists,
			artists: HashMap::new(),
			others: HashMap::new(),
			home: None,
			pages: Cache::new(snapshot::LISTINGS_KEPT, PAGES_TTL),
			external: external::Client::new(),
			web: spotify::WebPlayer::new(),
			popular: Cache::new(12, Duration::from_secs(1800)),
			abouts: Cache::new(24, Duration::from_secs(24 * 3600)),
			colors: Cache::new(64, Duration::from_secs(24 * 3600)),
		}
	}

	pub(super) fn cache_artist(&mut self, id: String, artist: Artist) {
		if self.artists.len() >= 128 && !self.artists.contains_key(&id) {
			self.artists.clear();
		}
		self.artists.insert(id, artist);
	}

	pub(super) fn cache_other(&mut self, id: String, playlist: Option<Playlist>) {
		if self.others.len() >= 128 && !self.others.contains_key(&id) {
			self.others.clear();
		}
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
			let mut snapshots = self.snapshots.lock().unwrap_or_else(PoisonError::into_inner);
			snapshots.record(&response);
			deliver(response);
			let _ = snapshots.save();
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
				playlist.public = known.public;
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
		self.snapshots.lock().unwrap_or_else(PoisonError::into_inner).forget(route);
	}
}
#[expect(clippy::too_many_arguments, reason = "the worker owns session, snapshot and membership lifecycles separately")]
fn serve(
	config: &Path,
	snapshots: &Arc<Mutex<Store>>,
	session: &mut Option<Session>,
	player: &mut Option<Reader>,
	added: &mut Membership,
	refresh_added: &mut bool,
	web_api: bool,
	request: Request,
	deliver: &dyn Fn(Response),
) {
	let output = |response| {
		let changed = snapshots.lock().unwrap_or_else(PoisonError::into_inner).record(&response);
		deliver(response);
		if changed {
			let _ = snapshots.lock().unwrap_or_else(PoisonError::into_inner).save();
		}
	};
	match request {
		Request::Connect => connect(config, snapshots, session, player, web_api, &output),
		Request::Login => match spotify::login(config, |url| deliver(Response::LoginUrl(url.to_owned()))) {
			Ok(()) => connect(config, snapshots, session, player, web_api, &output),
			Err(e) => deliver(Response::LoginFailed(e.user_message())),
		},
		Request::QueueContext(route) => {
			// Connect's reader survives a library reconnect even when the Web API is disabled.
			let reader = session.as_mut().and_then(|s| s.player.as_mut()).or(player.as_mut());
			output(queue_context(reader, &route));
		},
		Request::RememberPlayback(position) => {
			if let Err(error) = super::restore::remember(config, position) {
				deliver(Response::Error { route: None, message: format!("Couldn't save playback position: {error}") });
			}
		},
		Request::Logout => {
			snapshots.lock().unwrap_or_else(PoisonError::into_inner).clear();
			*session = None;
			let _ = spotify::logout(config);
			deliver(Response::NeedsLogin);
		},
		Request::ForgetSnapshot => snapshots.lock().unwrap_or_else(PoisonError::into_inner).clear(),
		other => {
			let Some(s) = session.as_mut() else {
				// Nothing can load without an account: say so, so no view keeps
				// waiting for an answer that will never come.
				deliver(Response::NeedsLogin);
				return;
			};
			let route = match &other {
				Request::Open(route, _) => Some(route.clone()),
				_ => None,
			};
			// Writes need permissions a sign-in from an older turntable lacks.
			let write = matches!(
				other,
				Request::SetFollowed(..)
					| Request::CreatePlaylist(..)
					| Request::RenamePlaylist(..)
					| Request::AddToPlaylist(..)
					| Request::RemoveFromPlaylist(..)
					| Request::MovePlaylistRow(..)
			);
			let result = match other {
				Request::LastPlayback => last_playback(config, s, s.player.as_ref().or(player.as_ref())).map(Some),
				other => run(s, added, refresh_added, other, deliver),
			};
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
						if let Route::Album(id) = &route
							&& let Ok(plays) = s.web.album_plays(id)
						{
							deliver(Response::Plays(route.clone(), plays));
						}
						if offset == 0
							&& let Some(url) = cover
							&& let Some(accent) = accent(s, &url)
						{
							deliver(Response::Accent(route, accent));
						}
					}
				},
				Ok(None) => {},
				Err(spotify::Error::AuthRequired | spotify::Error::AuthExpired) => {
					snapshots.lock().unwrap_or_else(PoisonError::into_inner).clear();
					*session = None;
					deliver(Response::NeedsLogin);
				},
				Err(e) if write && e.is_forbidden() => deliver(Response::Error {
					route,
					message: "Spotify needs one more permission for that: reconnect your account in Settings.".into(),
				}),
				Err(e) => {
					if matches!(e, spotify::Error::Network(_)) {
						deliver(Response::Offline(true));
					}
					deliver(Response::Error { route, message: e.user_message() });
				},
			}
		},
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
	if !config.join("client_id").is_file() {
		deliver(Response::NeedsClientId);
		return;
	}
	let account = match spotify::account::Account::open(config) {
		Ok(account) => account,
		Err(spotify::Error::AuthRequired | spotify::Error::AuthExpired) => {
			snapshots.lock().unwrap_or_else(PoisonError::into_inner).clear();
			deliver(Response::NeedsLogin);
			return;
		},
		Err(e) => {
			deliver(Response::Error { route: None, message: e.user_message() });
			return;
		},
	};
	let user_id = account.user_id();
	let (cached, home) = {
		let mut store = snapshots.lock().unwrap_or_else(PoisonError::into_inner);
		store.load(user_id.as_deref());
		(store.connected(), store.home())
	};
	let client = Client::open(account.source(), web_api);
	let has_snapshot = cached.is_some();
	if let Some(Response::Connected { me, playlists }) = cached {
		*session = Some(Session::new(client, me.clone(), playlists.clone(), snapshots.clone()));
		trace::hit("library", "Connected", "disk");
		deliver(Response::Connected { me, playlists });
		if let Some(home) = home {
			trace::hit("library", "Home", "disk");
			deliver(Response::Home(home));
		}
	} else {
		*session = Some(Session::new(client, spotify::User { id: String::new(), display_name: None }, Vec::new(), snapshots.clone()));
	}
	let Some(s) = session.as_mut() else { return };
	let client = &s.client;
	// The profile and the playlists don't wait for each other, nor do the playlists' later pages.
	let (profile, playlists) = par::join(
		|| client.me_with_plan(),
		|| match player {
			Some(player) if user_id.as_deref().is_some_and(|id| player.matches(id)) => player.playlists(),
			_ => all_playlists(client),
		},
	);
	let loaded = profile.and_then(|(me, plan)| {
		let playlists = if is_free(plan.as_deref()) { Vec::new() } else { playlists? };
		Ok((me, plan, playlists))
	});
	match loaded {
		// Playback needs Premium: say so now rather than half working.
		Ok((_, plan, _)) if is_free(plan.as_deref()) => {
			snapshots.lock().unwrap_or_else(PoisonError::into_inner).clear();
			*session = None;
			deliver(Response::LoginFailed(spotify::Error::PremiumRequired.user_message()));
		},
		Ok((me, _, mut playlists)) => {
			deliver(Response::Offline(false));
			let _ = account.remember_user(&me.id);
			s.me = me.id.clone();
			s.me_name.clone_from(&me.display_name);
			for playlist in &mut playlists {
				if playlist.owner.id == me.id {
					playlist.owner.display_name.clone_from(&me.display_name);
				}
			}
			s.playlists = playlists.clone();
			deliver(Response::Connected { me, playlists });
			if player.as_ref().is_some_and(|p| p.matches(&s.me)) {
				s.attach(player.take(), deliver);
			}
		},
		Err(spotify::Error::AuthRequired | spotify::Error::AuthExpired) => {
			snapshots.lock().unwrap_or_else(PoisonError::into_inner).clear();
			*session = None;
			deliver(Response::NeedsLogin);
		},
		Err(e) => {
			if matches!(e, spotify::Error::Network(_)) {
				deliver(Response::Offline(true));
			}
			if !has_snapshot && !matches!(e, spotify::Error::Network(_)) {
				*session = None;
			}
			deliver(Response::Error { route: None, message: e.user_message() });
		},
	}
}
/// A player request's outcome as the UI shows it: a sign-in problem still ends the session (the
/// caller's `?`), anything else becomes a sentence for the user.
fn player_reply<T>(result: spotify::Result<T>) -> spotify::Result<Result<T, String>> {
	match result {
		Ok(value) => Ok(Ok(value)),
		Err(e @ (spotify::Error::AuthRequired | spotify::Error::AuthExpired)) => Err(e),
		Err(spotify::Error::DeviceNotFound) => Ok(Err("Spotify doesn't see that device anymore. Open Spotify on it and try again.".into())),
		Err(e) => Ok(Err(e.user_message())),
	}
}

/// Whether a subscription (`/me`'s `product`) is Spotify's free plan. Unknown (a sign-in from before
/// turntable asked for it) counts as fine: playing then fails with its own sentence.
fn is_free(plan: Option<&str>) -> bool {
	matches!(plan, Some("free" | "open"))
}

/// Queue every native page, preserving duplicate occurrences and omitting local files.
fn queue_context_pages(
	mut page: impl FnMut(u32) -> spotify::Result<(String, Vec<spotify::Track>, Option<u32>)>,
) -> spotify::Result<(Vec<String>, String)> {
	let mut offset = 0;
	let mut uris = Vec::new();
	let mut title = None;
	loop {
		let (name, tracks, next) = page(offset)?;
		title.get_or_insert(name);
		uris.extend(tracks.into_iter().filter(|t| !t.is_local).map(|t| t.uri));
		match next {
			Some(next) if next > offset => offset = next,
			Some(_) => return Err(spotify::Error::Parse("Spotify returned a non-advancing queue page.".into())),
			None => return Ok((uris, title.unwrap_or_default())),
		}
	}
}

fn queue_context(player: Option<&mut Reader>, route: &Route) -> Response {
	let Some(player) = player else {
		return Response::Error { route: None, message: "Start the player before adding a context to its queue.".into() };
	};
	let result = queue_context_pages(|offset| match route {
		Route::Album(id) => player.album(id, offset).map(|listing| (listing.title, listing.tracks, listing.next_offset)),
		Route::Playlist(id) => {
			let (playlist, _, page) = player.playlist(id, offset, true)?;
			let page = page.ok_or_else(|| spotify::Error::Parse("Spotify returned no playlist tracks".into()))?;
			let next = page.next_offset();
			Ok((playlist.name, page.items, next))
		},
		_ => Err(spotify::Error::Parse("Only albums and playlists can be queued as a context.".into())),
	});
	match result {
		Ok((uris, title)) => Response::Queue(uris, title),
		Err(error) => Response::Error { route: None, message: error.user_message() },
	}
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
		Request::Home => Response::Home(home(s)?),
		Request::Albums => Response::Albums(match &mut s.player {
			Some(player) => player.albums(500)?,
			None => all_albums(&s.client)?,
		}),
		Request::Following => Response::Following(match &mut s.player {
			Some(player) => player.followed(500)?,
			None => all_followed(&mut s.client)?,
		}),
		Request::Artist(id) => Response::Artist(artist(s, &id)?),
		Request::ArtistPopular(id) => Response::ArtistPopular(id.clone(), popularity(s, &id)?),
		Request::ArtistAbout(id) => Response::ArtistAbout(id.clone(), about(s, &id)),
		Request::ArtistCard(id) => match s.web.overview(&id) {
			Ok(overview) => Response::ArtistCard(id, overview),
			Err(_) => return Ok(None),
		},
		Request::Credits(uri) => match s.web.credits(id_of(&uri)) {
			Ok(credits) => Response::Credits(uri, credits),
			Err(_) => return Ok(None),
		},
		Request::SetFollowed(uri, follow) => {
			let response = set_followed(s, &uri, follow)?;
			*refresh_added |= uri.starts_with("spotify:playlist:");
			if !follow && uri.starts_with("spotify:playlist:") {
				added.deleted(&uri, deliver);
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
		Request::MovePlaylistRow(id, from, to) => move_playlist_row(s, id, from, to)?,
		Request::CheckLiked(uri) => check_liked(s, uri, deliver)?,
		Request::SetLiked(uri, liked) => set_liked(s, &uri, liked, deliver)?,
		Request::Open(route, offset) => {
			let listing = open(s, &route, offset)?;
			if route == Route::Liked {
				s.publish_liked(deliver);
			}
			Response::Listing { route, offset, listing }
		},
		Request::Search(query) => {
			let results = match s.web.search(&query, 0) {
				Ok(results) => results,
				Err(_) => s.client.search(&query, 0)?,
			};
			Response::Search { query, results }
		},
		Request::Devices => Response::Devices(player_reply(s.client.devices())?),
		Request::Playback => Response::Playback(player_reply(s.client.playback())?),
		Request::Transfer { id, play } => match player_reply(s.client.transfer(&id, play))? {
			Ok(()) => Response::Transferred,
			Err(message) => Response::PlayerError(message),
		},
		Request::Control { device, command } => match player_reply(s.client.control(&device, &command))? {
			Ok(()) => return Ok(None),
			Err(message) => Response::PlayerError(message),
		},
		Request::Connect
		| Request::Login
		| Request::Logout
		| Request::ForgetSnapshot
		| Request::RememberPlayback(_)
		| Request::LastPlayback
		| Request::QueueContext(_) => return Ok(None),
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
	if liked {
		s.client.save(&[uri])?;
	} else {
		s.client.unsave(&[uri])?;
	}
	s.pages.remove(&Route::Liked);
	s.snapshots.lock().unwrap_or_else(PoisonError::into_inner).forget(&Route::Liked);
	if let Some(player) = &mut s.player {
		player.saved(uri, liked);
	}
	s.publish_liked(deliver);
	Ok(Response::Liked(uri.to_owned(), liked))
}

fn set_followed(s: &mut Session, uri: &str, follow: bool) -> spotify::Result<Response> {
	if let Some(player) = &mut s.player
		&& uri.starts_with("spotify:playlist:")
	{
		player.follow_playlist(uri, follow)?;
	} else if follow {
		s.client.save(&[uri])?;
	} else {
		s.client.unsave(&[uri])?;
	}
	if !follow && uri.starts_with("spotify:playlist:") {
		let id = id_of(uri);
		s.playlists.retain(|playlist| playlist.id != id);
		let route = Route::Playlist(id.to_owned());
		s.pages.remove(&route);
		if let Some(player) = &mut s.player {
			player.forget(&route);
		}
		s.snapshots.lock().unwrap_or_else(PoisonError::into_inner).remove_playlist(id);
		return Ok(Response::Playlists(s.playlists.clone()));
	}
	s.pages.clear();
	if let Some(player) = &mut s.player {
		player.saved(uri, follow);
	}
	s.snapshots.lock().unwrap_or_else(PoisonError::into_inner).forget_listings();
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
		Some(player) if changes.public.is_none() => player.rename_playlist(&id, changes)?,
		_ => s.client.rename_playlist(&id, changes)?,
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

fn move_playlist_row(s: &mut Session, id: String, from: u32, to: u32) -> spotify::Result<Response> {
	match &mut s.player {
		Some(player) => player.move_playlist_row(&id, from, to)?,
		None => s.client.move_playlist_row(&id, from, to)?,
	}
	s.forget(&Route::Playlist(id.clone()));
	// Membership is order-independent: reordering needs no index refresh.
	Ok(Response::Moved(id, from, to))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn context_queue_without_a_player_reports_that_not_a_web_login_requirement() {
		let response = queue_context(None, &Route::Album("album".into()));
		assert!(matches!(response, Response::Error { message, .. } if message.contains("Start the player")));
	}

	#[test]
	fn queue_context_reads_all_pages_preserving_order_and_duplicates() -> spotify::Result<()> {
		let mut requested = Vec::new();
		let (uris, title) = queue_context_pages(|offset| {
			requested.push(offset);
			let track = |uri: &str, is_local| spotify::Track {
				uri: uri.into(),
				name: "Track".into(),
				duration_ms: 1_000,
				disc_number: 0,
				explicit: false,
				artists: Vec::new(),
				album: None,
				is_local,
				is_episode: false,
			};
			Ok((
				"Album".into(),
				vec![track("spotify:track:a", false), track("spotify:local:b", true)],
				if offset == 0 { Some(50) } else { None },
			))
		})?;
		assert_eq!(requested, [0, 50]);
		assert_eq!(uris, ["spotify:track:a", "spotify:track:a"]);
		assert_eq!(title, "Album");
		Ok(())
	}

	#[test]
	fn queue_context_does_not_succeed_with_a_partial_page_failure() {
		let result = queue_context_pages(|offset| {
			if offset == 0 { Ok(("Playlist".into(), Vec::new(), Some(50))) } else { Err(spotify::Error::Parse("page failed".into())) }
		});
		assert!(result.is_err());
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
			assert!(session.artists.len() <= 128);
			assert!(session.others.len() <= 128);
		}
		assert!(!session.artists.contains_key("artist-0"));
		assert!(session.artists.contains_key("artist-399"));
		assert!(!session.others.contains_key("playlist-0"));
		assert!(session.others.contains_key("playlist-399"));
	}
}
