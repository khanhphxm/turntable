//! The library worker thread, its connected session and request dispatch.

use std::{
	collections::HashMap,
	path::{Path, PathBuf},
	sync::{Arc, Mutex, PoisonError, mpsc},
	thread,
	time::{Duration, Instant},
};

use super::{
	Home, Listing, PAGE, Popularity, Request, Response, Route,
	artist::{about, artist, popularity},
	home::home,
	pages::{accent, all_albums, all_followed, all_playlists, header, open},
	snapshot::{self, Store},
};
use crate::{
	net::{cache::Cache, par, trace},
	sources::external,
	spotify::{self, Artist, Client, Playlist, id_of},
};

pub struct Worker {
	tx: mpsc::Sender<Request>,
	snapshots: Arc<Mutex<Store>>,
	deliver: Arc<dyn Fn(Response) + Send + Sync>,
}

impl Worker {
	/// Starts the worker. Disk listings can also be delivered by `send`, without waiting for
	/// an in-flight network request; every snapshot is followed by the worker's fresh answer.
	pub fn spawn(config_dir: PathBuf, cache_dir: PathBuf, deliver: impl Fn(Response) + Send + Sync + 'static) -> Self {
		let (tx, rx) = mpsc::channel::<Request>();
		let snapshots = Arc::new(Mutex::new(Store::new(&cache_dir)));
		let deliver: Arc<dyn Fn(Response) + Send + Sync> = Arc::new(deliver);
		let worker = Self { tx, snapshots: snapshots.clone(), deliver: deliver.clone() };
		let _ = thread::Builder::new().name("library".into()).stack_size(512 * 1024).spawn(move || {
			let mut session: Option<Session> = None;
			let mut waiting = None;
			while let Some(request) = waiting.take().or_else(|| rx.recv().ok()) {
				serve(&config_dir, &snapshots, &mut session, request, deliver.as_ref());
				// Once nothing else is waiting, the burst is over: the answers kept to share within it,
				// and the listings that have gone stale, are let go, and the heap they took goes back.
				waiting = rx.try_recv().ok();
				if waiting.is_none() {
					if let Some(s) = &mut session {
						s.client.forget_recent();
						s.pages.evict_expired();
					}
					crate::net::trim_heap();
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
		let _ = self.tx.send(request);
	}
}

/// A connected Web API session and what's worth not asking twice.
pub(super) struct Session {
	pub(super) client:    Client,
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

	/// Forgets the listing of `route` (after a write that changed it).
	fn forget(&mut self, route: &Route) {
		self.pages.remove(route);
		self.snapshots.lock().unwrap_or_else(PoisonError::into_inner).forget(route);
	}
}
fn serve(config: &Path, snapshots: &Arc<Mutex<Store>>, session: &mut Option<Session>, request: Request, deliver: &dyn Fn(Response)) {
	let output = |response| {
		let changed = snapshots.lock().unwrap_or_else(PoisonError::into_inner).record(&response);
		deliver(response);
		if changed {
			let _ = snapshots.lock().unwrap_or_else(PoisonError::into_inner).save();
		}
	};
	match request {
		Request::Connect => connect(config, snapshots, session, &output),
		Request::Login => match spotify::login(config, |url| deliver(Response::LoginUrl(url.to_owned()))) {
			Ok(()) => connect(config, snapshots, session, &output),
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
			// Writes need permissions a sign-in from an older terntable lacks.
			let write = matches!(
				other,
				Request::SetFollowed(..)
					| Request::CreatePlaylist(..)
					| Request::RenamePlaylist(..)
					| Request::AddToPlaylist(..)
					| Request::RemoveFromPlaylist(..)
			);
			match run(s, other) {
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

fn connect(config: &Path, snapshots: &Arc<Mutex<Store>>, session: &mut Option<Session>, deliver: &dyn Fn(Response)) {
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
	let (profile, playlists) = par::join(|| client.me_with_plan(), || all_playlists(client));
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
		Ok((me, _, playlists)) => {
			let _ = account.remember_user(&me.id);
			s.me = me.id.clone();
			s.playlists = playlists.clone();
			deliver(Response::Connected { me, playlists });
		},
		Err(spotify::Error::AuthRequired | spotify::Error::AuthExpired) => {
			snapshots.lock().unwrap_or_else(PoisonError::into_inner).clear();
			*session = None;
			deliver(Response::NeedsLogin);
		},
		Err(e) => {
			if !has_snapshot { *session = None; }
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
/// terntable asked for it) counts as fine: playing then fails with its own sentence.
fn is_free(plan: Option<&str>) -> bool {
	matches!(plan, Some("free" | "open"))
}

fn run(s: &mut Session, request: Request) -> spotify::Result<Option<Response>> {
	Ok(Some(match request {
		Request::Home => Response::Home(home(s)?),
		Request::Albums => Response::Albums(all_albums(&s.client)?),
		Request::Following => Response::Following(all_followed(&mut s.client)?),
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
		Request::SetFollowed(uri, follow) => set_followed(s, &uri, follow)?,
		Request::CreatePlaylist(name, uris) => create_playlist(s, &name, &uris)?,
		Request::RenamePlaylist(id, changes) => {
			s.client.rename_playlist(&id, &changes)?;
			s.forget(&Route::Playlist(id));
			s.playlists = all_playlists(&s.client)?;
			Response::Playlists(s.playlists.clone())
		},
		Request::AddToPlaylist(id, name, uris) => {
			let uris: Vec<&str> = uris.iter().map(String::as_str).collect();
			s.client.add_to_playlist(&id, &uris)?;
			s.forget(&Route::Playlist(id));
			Response::Done(format!("Added to {name}"))
		},
		Request::RemoveFromPlaylist(id, uri) => {
			s.client.remove_from_playlist(&id, &[&uri])?;
			s.forget(&Route::Playlist(id.clone()));
			Response::Removed(id, uri)
		},
		Request::QueueContext(route) => {
			let tracks = match &route {
				Route::Album(id) => s.client.album_tracks(id, 0, PAGE)?.items,
				Route::Playlist(id) => s.client.playlist_items(id, 0, PAGE)?.items,
				Route::Liked => s.client.saved_tracks(0, PAGE)?.items,
				_ => Vec::new(),
			};
			let title = header(s, &route).map(|h| h.title).unwrap_or_default();
			Response::Queue(tracks.into_iter().filter(|t| !t.is_local).map(|t| t.uri).collect(), title)
		},
		Request::CheckLiked(uri) => {
			let liked = s.client.library_contains(&[&uri])?.first().copied().unwrap_or(false);
			Response::Liked(uri, liked)
		},
		Request::SetLiked(uri, liked) => {
			if liked {
				s.client.save(&[&uri])?;
			} else {
				s.client.unsave(&[&uri])?;
			}
			s.forget(&Route::Liked);
			Response::Liked(uri, liked)
		},
		Request::Open(route, offset) => {
			let listing = open(s, &route, offset)?;
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

fn set_followed(s: &mut Session, uri: &str, follow: bool) -> spotify::Result<Response> {
	if follow {
		s.client.save(&[uri])?;
	} else {
		s.client.unsave(&[uri])?;
	}
	s.pages.clear();
	s.snapshots.lock().unwrap_or_else(PoisonError::into_inner).forget_listings();
	Ok(if uri.starts_with("spotify:playlist:") {
		s.playlists = all_playlists(&s.client)?;
		Response::Playlists(s.playlists.clone())
	} else if uri.starts_with("spotify:album:") {
		s.home = None;
		Response::Albums(all_albums(&s.client)?)
	} else {
		// Following an artist changes Your Library and its home shelf.
		s.home = None;
		Response::Following(all_followed(&mut s.client)?)
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
