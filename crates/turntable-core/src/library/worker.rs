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
	pub fn spawn(config_dir: PathBuf, cache_dir: &Path, deliver: impl Fn(Response) + Send + Sync + 'static) -> Self {
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
	pub(super) snapshots: Arc<Mutex<Store>>,
	/// The user's playlists, to name recently played ones without a request.
	pub(super) playlists: Vec<Playlist>,
	/// Artists and others' playlists fetched before (images and names don't change often).
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
	fn new(client: Client, me: String, playlists: Vec<Playlist>, snapshots: Arc<Mutex<Store>>) -> Self {
		Self {
			client,
			player: None,
			me,
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

	fn attach(&mut self, player: Option<Reader>, deliver: &dyn Fn(Response)) {
		self.player = player;
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
		Request::Connect => connect(config, snapshots, session, player, &output),
		Request::Login => match spotify::login(config, |url| deliver(Response::LoginUrl(url.to_owned()))) {
			Ok(()) => connect(config, snapshots, session, player, &output),
			Err(e) => deliver(Response::LoginFailed(e.user_message())),
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
			);
			match run(s, added, refresh_added, other, deliver) {
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
				Err(e) => deliver(Response::Error { route, message: e.user_message() }),
			}
		},
	}
}

fn connect(
	config: &Path,
	snapshots: &Arc<Mutex<Store>>,
	session: &mut Option<Session>,
	player: &mut Option<Reader>,
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
	let client = Client::open(account.source());
	let has_snapshot = cached.is_some();
	if let Some(Response::Connected { me, playlists }) = cached {
		*session = Some(Session::new(client, me.id.clone(), playlists.clone(), snapshots.clone()));
		trace::hit("library", "Connected", "disk");
		deliver(Response::Connected { me, playlists });
		if let Some(home) = home {
			trace::hit("library", "Home", "disk");
			deliver(Response::Home(home));
		}
	} else {
		*session = Some(Session::new(client, String::new(), Vec::new(), snapshots.clone()));
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
			let _ = account.remember_user(&me.id);
			s.me = me.id.clone();
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
			if !has_snapshot {
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
			response
		},
		Request::CreatePlaylist(name, uris) => {
			let response = create_playlist(s, &name, &uris)?;
			*refresh_added = true;
			response
		},
		Request::RenamePlaylist(id, changes) => {
			s.client.rename_playlist(&id, &changes)?;
			if let Some(public) = changes.public
				&& let Some(playlist) = s.playlists.iter_mut().find(|playlist| playlist.id == id)
			{
				playlist.public = Some(public);
			}
			*refresh_added = true;
			s.forget(&Route::Playlist(id));
			s.playlists = s.load_playlists()?;
			Response::Playlists(s.playlists.clone())
		},
		Request::AddToPlaylist(id, name, uris) => {
			let borrowed: Vec<&str> = uris.iter().map(String::as_str).collect();
			s.client.add_to_playlist(&id, &borrowed)?;
			added.edited(&format!("spotify:playlist:{id}"), &uris, &[], deliver);
			*refresh_added = true;
			s.forget(&Route::Playlist(id));
			Response::Done(format!("Added to {name}"))
		},
		Request::RemoveFromPlaylist(id, uri) => {
			s.client.remove_from_playlist(&id, &[&uri])?;
			added.edited(&format!("spotify:playlist:{id}"), &[], std::slice::from_ref(&uri), deliver);
			*refresh_added = true;
			s.forget(&Route::Playlist(id.clone()));
			Response::Removed(id, uri)
		},
		Request::QueueContext(route) => {
			let listing = open(s, &route, 0)?;
			Response::Queue(listing.tracks.into_iter().filter(|t| !t.is_local).map(|t| t.uri).collect(), listing.title)
		},
		Request::CheckLiked(uri) => {
			let liked = match &mut s.player {
				Some(player) => player.contains(&uri)?,
				None => s.client.library_contains(&[&uri])?.first().copied().unwrap_or(false),
			};
			s.publish_liked(deliver);
			Response::Liked(uri, liked)
		},
		Request::SetLiked(uri, liked) => set_liked(s, &uri, liked, deliver)?,
		Request::Open(route, offset) => {
			let listing = open(s, &route, offset)?;
			if route == Route::Liked {
				s.publish_liked(deliver);
			}
			Response::Listing { route, offset, listing }
		},
		Request::Search(query) => {
			let results = s.client.search(&query, 0)?;
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
		Request::Connect | Request::Login | Request::Logout | Request::ForgetSnapshot => return Ok(None),
	}))
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
	if follow {
		s.client.save(&[uri])?;
	} else {
		s.client.unsave(&[uri])?;
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
	let playlist = s.client.create_playlist(name)?;
	if !uris.is_empty() {
		let uris: Vec<&str> = uris.iter().map(String::as_str).collect();
		s.client.add_to_playlist(&playlist.id, &uris)?;
	}
	s.playlists.insert(0, playlist.clone());
	Ok(Response::Created(playlist))
}
