//! Library navigation, selection, filtering and application of worker responses.

use std::collections::HashMap;

use super::{Conn, Entry, Home, Kind, Listing, Request, Response, Route, Sort, View, model::fold};
use crate::spotify::{self, Album, Artist, Playlist, SearchResults, Track, User, id_of};

#[derive(Debug, Default)]
pub struct Library {
	pub conn:           Conn,
	pub me:             Option<User>,
	pub playlists:      Vec<Playlist>,
	/// Albums saved to the library, most recently added first.
	pub albums:         Vec<Album>,
	/// Artists the user follows.
	pub followed:       Vec<Artist>,
	/// What Your Library lists: the kind chips that are on (none: every kind), the order, the text
	/// filter, and how the rows are laid out.
	pub kinds:          Vec<Kind>,
	pub sort:           Sort,
	pub filter:         String,
	pub view:           View,
	pub route:          Route,
	pub listing:        Option<Listing>,
	pub query:          String,
	pub search:         Option<SearchResults>,
	pub home:           Home,
	/// Artists fetched for the now-playing view, by id.
	pub artists:        HashMap<String, Artist>,
	/// What the web player says about them, by id.
	pub overviews:      HashMap<String, spotify::ArtistOverview>,
	/// Who made the song that last asked: (track URI, credits).
	pub credits:        Option<(String, Vec<spotify::Credit>)>,
	/// Whether the URI last checked is in Liked Songs.
	pub liked:          Option<(String, bool)>,
	/// Selected row in the content area, if any.
	pub selected:       Option<usize>,
	/// Whether the selection is drawn: only after keyboard navigation or a
	/// click, never just because a list loaded.
	pub show_selection: bool,
	/// A track to select once the page being opened lists it (a song card opens its album there).
	pub reveal:         Option<String>,
	pub error:          Option<String>,
}

impl Library {
	/// Tracks currently listed in the content area (for selection and playback).
	pub fn rows(&self) -> &[Track] {
		match self.route {
			Route::Search => self.search.as_ref().map_or(&[][..], |s| &s.tracks.items),
			Route::Home => self.home.songs().1,
			_ => self.listing.as_ref().map_or(&[][..], |l| &l.tracks),
		}
	}

	/// What the page being shown has fully drawn, for traces (see [`crate::net::trace`]): `home`, `album:<id>`,
	/// `artist:<id>:header+popular+about` ... A page still waiting for its data says `:loading`.
	pub fn ready_label(&self) -> String {
		let loaded = self.listing.as_ref().filter(|l| !l.loading);
		match &self.route {
			Route::Home if self.home.loaded => "home".into(),
			Route::Home => "home:loading".into(),
			Route::Search if self.search.is_some() => "search".into(),
			Route::Search => "search:empty".into(),
			Route::Library => "library".into(),
			Route::Liked if loaded.is_some() => "liked".into(),
			Route::Playlist(id) if loaded.is_some() => format!("playlist:{id}"),
			Route::Album(id) if loaded.is_some() => format!("album:{id}"),
			Route::Artist(id) => match loaded {
				Some(l) => format!(
					"artist:{id}:header{}{}",
					if l.popular_loaded { "+popular" } else { "" },
					if l.about.is_some() { "+about" } else { "" }
				),
				None => format!("artist:{id}:loading"),
			},
			route => format!("{route:?}:loading"),
		}
	}

	/// The rows of Your Library: Liked Songs pinned first, then the kinds the chips let through
	/// whose title holds the filter text, in the chosen order. Unfiltered, it lists everything
	/// (the covers-only sidebar has no controls to say why rows would be missing).
	pub fn entries(&self, filtered: bool) -> Vec<Entry<'_>> {
		let kinds = if filtered { self.kinds.as_slice() } else { &[] };
		let wanted = |kind| kinds.is_empty() || kinds.contains(&kind);
		let needle: String = if filtered { fold(self.filter.trim()).collect() } else { String::new() };
		let mut folded = String::new();
		let mut matches = |e: &Entry<'_>| {
			needle.is_empty() || {
				folded.clear();
				folded.extend(fold(e.title()));
				folded.contains(&needle)
			}
		};
		let me = self.me.as_ref().map_or("", |u| u.display_name.as_deref().unwrap_or(&u.id));
		let mut rows = Vec::with_capacity(self.playlists.len() + self.followed.len() + self.albums.len() + 1);
		if wanted(Kind::Playlist) {
			rows.extend(self.playlists.iter().map(Entry::Playlist));
		}
		if wanted(Kind::Artist) {
			rows.extend(self.followed.iter().map(Entry::Artist));
		}
		if wanted(Kind::Album) {
			rows.extend(self.albums.iter().map(Entry::Album));
		}
		rows.retain(&mut matches);
		match self.sort {
			Sort::Recents => rows.sort_by_key(|e| self.home.recents.iter().position(|t| e.opens(&t.route)).unwrap_or(usize::MAX)),
			Sort::Added => {},
			Sort::Alphabetical => rows.sort_by(|a, b| fold(a.title()).cmp(fold(b.title()))),
			Sort::Creator => {
				rows.sort_by(|a, b| fold(a.creator()).cmp(fold(b.creator())).then_with(|| fold(a.title()).cmp(fold(b.title()))));
			},
		}
		if wanted(Kind::Playlist) && matches(&Entry::Liked(me)) {
			rows.insert(0, Entry::Liked(me));
		}
		rows
	}

	/// Moves the selection by `delta`, clamped to the rows; the first move selects the first row.
	pub fn move_selection(&mut self, delta: i32) {
		let n = self.rows().len();
		if n == 0 {
			self.selected = None;
			return;
		}
		self.show_selection = true;
		self.selected = Some(match self.selected {
			None => 0,
			Some(i) => {
				i.saturating_add_signed(isize::try_from(delta).unwrap_or(if delta < 0 { isize::MIN } else { isize::MAX })).min(n - 1)
			},
		});
	}

	/// Selects row `i` (clamped) and shows the selection.
	pub fn select(&mut self, i: usize) {
		let n = self.rows().len();
		if n > 0 {
			self.selected = Some(i.min(n - 1));
			self.show_selection = true;
		}
	}

	/// What playing row `index` of the current view should do.
	pub fn play_start(&self, index: usize) -> Option<spotify::PlayStart> {
		let track = self.rows().get(index)?;
		if let (Some(listing), Route::Playlist(_) | Route::Album(_)) = (&self.listing, &self.route)
			&& let Some(context) = &listing.context_uri
		{
			return Some(spotify::PlayStart::Context {
				uri:         context.clone(),
				offset_uri:  Some(track.uri.clone()),
				position_ms: 0,
			});
		}
		// No playable context (Liked Songs, search, recents): play the listed tracks
		// starting at the chosen one, as the Spotify app does. The whole list goes
		// with an offset (not the rows from it on), so shuffle starts with the chosen
		// song and draws the rest from the list.
		let uris: Vec<String> = self.rows().iter().map(|t| t.uri.clone()).collect();
		Some(spotify::PlayStart::Tracks { uris, offset_uri: Some(track.uri.clone()) })
	}

	/// Opens `route`, returning the requests that load it.
	pub fn navigate(&mut self, route: Route) -> Vec<Request> {
		self.selected = None;
		self.show_selection = false;
		self.reveal = None;
		self.error = None;
		self.route = route.clone();
		match &route {
			Route::Home => vec![Request::Home],
			Route::Search | Route::Library => Vec::new(),
			Route::Artist(id) => {
				self.listing = Some(Listing { loading: true, ..self.placeholder(&route) });
				// The web player's songs before the Web API's releases: they're the slow part of a page, and the
				// open data's About (Wikipedia) last, so it never holds the rest back.
				vec![Request::ArtistPopular(id.clone()), Request::Open(route.clone(), 0), Request::ArtistAbout(id.clone())]
			},
			Route::Liked | Route::Playlist(_) | Route::Album(_) => {
				self.listing = Some(Listing { loading: true, ..self.placeholder(&route) });
				vec![Request::Open(route, 0)]
			},
		}
	}

	/// A listing header shown while the tracks load.
	fn placeholder(&self, route: &Route) -> Listing {
		match route {
			Route::Liked => Listing { kicker: ("Playlist").into(), title: "Liked Songs".into(), ..Listing::default() },
			Route::Playlist(id) => {
				let p = self.playlists.iter().find(|p| &p.id == id);
				Listing { kicker: ("Playlist").into(), title: p.map_or_else(String::new, |p| p.name.clone()),
					description: p.and_then(|p| p.description.clone()),
					public: p.and_then(|p| p.public),
					owned: p.is_some_and(|p| self.me.as_ref().is_some_and(|me| me.id == p.owner.id)),
					context_uri: p.map(|p| p.uri.clone()),
					cover_url: p.and_then(|p| spotify::pick_image(&p.images, 300)).map(|i| i.url.clone()),
					..Listing::default() }
			},
			Route::Artist(id) => {
				let a = self.artists.get(id);
				Listing { kicker: ("Artist").into(), title: a.map_or_else(String::new, |a| a.name.clone()),
					context_uri: Some(format!("spotify:artist:{id}")),
					cover_url: a.and_then(|a| spotify::pick_image(&a.images, 300)).map(|i| i.url.clone()),
					..Listing::default() }
			},
			_ => Listing { kicker: ("Album").into(), ..Listing::default() },
		}
	}

	pub fn apply(&mut self, response: Response) {
		match response {
			Response::Connected { me, playlists } => {
				self.conn = Conn::Ready;
				self.me = Some(me);
				self.playlists = playlists;
				// The albums and artists load next: none of a previous account's meanwhile.
				self.albums.clear();
				self.followed.clear();
			},
			Response::NeedsLogin => {
				self.conn = Conn::NeedsLogin;
				if let Some(l) = &mut self.listing {
					l.loading = false;
				}
			},
			Response::LoginUrl(url) => {
				if matches!(self.conn, Conn::LoggingIn(_)) {
					self.conn = Conn::LoggingIn(Some(url));
				}
			},
			Response::NeedsClientId => self.conn = Conn::NeedsClientId,
			Response::Liked(uri, liked) => self.liked = Some((uri, liked)),
			Response::Home(home) => self.home = home,
			Response::Artist(artist) => {
				self.artists.insert(id_of(&artist.uri).to_owned(), artist);
			},
			Response::ArtistPopular(id, popular) => {
				if self.route == Route::Artist(id.clone())
					&& let Some(l) = &mut self.listing
				{
					l.popular_loaded = true;
					l.tracks = popular.tracks;
					l.plays = popular.plays;
					l.accent = popular.accent;
					l.overview = popular.overview.clone();
				}
				if let Some(overview) = popular.overview {
					self.overviews.insert(id, overview);
				}
			},
			Response::ArtistAbout(id, about) => {
				if self.route == Route::Artist(id)
					&& let Some(l) = &mut self.listing
				{
					l.about = Some(about);
				}
			},
			Response::ArtistCard(id, overview) => {
				self.overviews.insert(id, overview);
			},
			Response::Credits(uri, credits) => self.credits = Some((uri, credits)),
			Response::Accent(route, accent) => {
				if route == self.route
					&& let Some(l) = &mut self.listing
				{
					l.accent = Some(accent);
				}
			},
			Response::Plays(route, plays) => {
				if route == self.route
					&& let Some(l) = &mut self.listing
				{
					// Parallel to the songs shown, or none when any is unknown.
					let counts: Option<Vec<u64>> = l.tracks.iter().map(|t| plays.get(id_of(&t.uri)).copied()).collect();
					l.plays = counts.unwrap_or_default();
				}
			},
			Response::Playlists(playlists) => self.playlists = playlists,
			Response::Albums(albums) => self.albums = albums,
			Response::Following(artists) => self.followed = artists,
			Response::Created(p) => self.playlists.insert(0, p),
			Response::Removed(id, uri) => {
				if self.route == Route::Playlist(id)
					&& let Some(l) = &mut self.listing
				{
					l.tracks.retain(|t| t.uri != uri);
					l.total = l.total.saturating_sub(1);
				}
			},
			// Playback and queue replies belong to `State`, not to the library.
			Response::Queue(..)
			| Response::Done(_)
			| Response::Devices(_)
			| Response::Playback(_)
			| Response::Transferred
			| Response::PlayerError(_) => {},
			Response::Listing { route, offset, listing } => self.apply_listing(&route, offset, listing),
			Response::Search { query, results } => {
				if query == self.query.trim() {
					self.search = Some(results);
					self.selected = None;
					self.show_selection = false;
				}
			},
			Response::Error { route, message } => self.apply_error(route.as_ref(), message),
			Response::LoginFailed(e) => self.conn = Conn::Failed(e),
		}
	}

	fn apply_error(&mut self, route: Option<&Route>, message: String) {
		// An error from a page the user already left isn't about this one.
		if route.is_some_and(|r| *r != self.route) {
			return;
		}
		if let Some(l) = &mut self.listing {
			l.loading = false;
		}
		self.error = Some(message);
	}

	fn apply_listing(&mut self, route: &Route, offset: u32, mut listing: Listing) {
		if *route != self.route {
			return;
		}
		match (&mut self.listing, offset) {
			(Some(current), o) if o > 0 => {
				current.tracks.extend(listing.tracks);
				current.next_offset = listing.next_offset;
				current.loading = false;
			},
			(current, _) => {
				// An artist's popular songs may arrive before its releases.
				if let (Route::Artist(_), Some(early)) = (route, current.take())
					&& early.popular_loaded
				{
					(listing.tracks, listing.plays, listing.popular_loaded) = (early.tracks, early.plays, true);
					listing.overview = early.overview;
					listing.accent = early.accent;
				}
				self.listing = Some(listing);
				if let Some(uri) = self.reveal.take()
					&& let Some(i) = self.listing.as_ref().and_then(|l| l.tracks.iter().position(|t| t.uri == uri))
				{
					self.selected = Some(i);
					self.show_selection = true;
				}
			},
		}
	}
}
#[cfg(test)]
mod tests {
	use super::*;
	use crate::library::Tile;

	fn playlist(id: &str, name: &str, owner: &str) -> Playlist {
		serde_json::from_value(serde_json::json!({ "id": id, "name": name, "owner": { "id": owner, "display_name": owner } })).unwrap()
	}

	fn album(id: &str, name: &str, artist: &str) -> Album {
		serde_json::from_value(serde_json::json!({ "name": name, "uri": format!("spotify:album:{id}"), "artists": [{ "name": artist }] }))
			.unwrap()
	}

	fn artist(id: &str, name: &str) -> Artist {
		serde_json::from_value(serde_json::json!({ "name": name, "uri": format!("spotify:artist:{id}") })).unwrap()
	}

	/// Two playlists, two followed artists and two albums, for the account Alex.
	fn library() -> Library {
		Library {
			conn: Conn::Ready,
			me: Some(User { id: "alex".into(), display_name: Some("Alex".into()) }),
			playlists: vec![playlist("p1", "Road trip", "alex"), playlist("p2", "focus", "Zed")],
			followed: vec![artist("r1", "Radiohead"), artist("r2", "daft punk")],
			albums: vec![album("a1", "Blonde", "Frank Ocean"), album("a2", "Abbey Road", "The Beatles")],
			..Library::default()
		}
	}

	fn titles(lib: &Library, filtered: bool) -> Vec<String> {
		lib.entries(filtered).iter().map(|e| e.title().to_owned()).collect()
	}

	#[test]
	fn playing_a_missing_row_has_no_start() {
		let mut lib = library();
		assert_eq!(lib.play_start(0), None);
		lib.route = Route::Liked;
		lib.listing = Some(Listing::default());
		assert_eq!(lib.play_start(0), None);
	}

	#[test]
	fn entries_pin_liked_songs_then_playlists_artists_and_albums() {
		let lib = library();
		assert_eq!(titles(&lib, true), ["Liked Songs", "Road trip", "focus", "Radiohead", "daft punk", "Blonde", "Abbey Road"]);
		let routes: Vec<Route> = lib.entries(true).iter().map(Entry::route).collect();
		assert_eq!(routes[..4], [Route::Liked, Route::Playlist("p1".into()), Route::Playlist("p2".into()), Route::Artist("r1".into())]);
		assert_eq!(routes[5], Route::Album("a1".into()));
	}

	#[test]
	fn chips_keep_their_kinds_and_none_keeps_all() {
		let mut lib = library();
		lib.kinds = vec![Kind::Artist];
		assert_eq!(titles(&lib, true), ["Radiohead", "daft punk"], "Liked Songs is a playlist");
		lib.kinds = vec![Kind::Album, Kind::Playlist];
		assert_eq!(titles(&lib, true), ["Liked Songs", "Road trip", "focus", "Blonde", "Abbey Road"]);
	}

	#[test]
	fn the_filter_matches_titles_ignoring_case() {
		let mut lib = library();
		lib.filter = "  OA ".into();
		assert_eq!(titles(&lib, true), ["Road trip", "Abbey Road"]);
		lib.filter = "liked".into();
		assert_eq!(titles(&lib, true), ["Liked Songs"]);
		lib.kinds = vec![Kind::Album];
		assert!(titles(&lib, true).is_empty());
	}

	#[test]
	fn the_covers_only_sidebar_ignores_the_chips_and_the_filter() {
		let mut lib = library();
		lib.kinds = vec![Kind::Artist];
		lib.filter = "zzz".into();
		assert!(lib.entries(true).is_empty());
		assert_eq!(lib.entries(false).len(), 7);
	}

	#[test]
	fn the_orders_keep_liked_songs_first() {
		let mut lib = library();
		lib.sort = Sort::Added;
		assert_eq!(
			titles(&lib, true),
			["Liked Songs", "Road trip", "focus", "Radiohead", "daft punk", "Blonde", "Abbey Road"],
			"as Spotify lists them"
		);
		lib.sort = Sort::Alphabetical;
		assert_eq!(titles(&lib, true), ["Liked Songs", "Abbey Road", "Blonde", "daft punk", "focus", "Radiohead", "Road trip"]);
		// By creator: alex, daft punk, Frank Ocean, Radiohead, The Beatles, Zed.
		lib.sort = Sort::Creator;
		assert_eq!(titles(&lib, true), ["Liked Songs", "Road trip", "daft punk", "Blonde", "Radiohead", "Abbey Road", "focus"]);
	}

	#[test]
	fn recents_put_what_was_played_lately_first_and_keep_spotifys_order_for_the_rest() {
		let mut lib = library();
		lib.home.recents = vec![Tile::album(&lib.albums[1]), Tile::artist(&lib.followed[1]), Tile::playlist(&lib.playlists[1])];
		assert_eq!(titles(&lib, true), ["Liked Songs", "Abbey Road", "daft punk", "focus", "Road trip", "Radiohead", "Blonde"]);
	}

	#[test]
	fn a_new_account_starts_without_the_last_ones_albums_and_artists() {
		let mut lib = library();
		lib.apply(Response::Connected { me: User { id: "bo".into(), display_name: None }, playlists: Vec::new() });
		assert!(lib.albums.is_empty() && lib.followed.is_empty());
		lib.apply(Response::Albums(vec![album("a1", "Blonde", "Frank Ocean")]));
		lib.apply(Response::Following(vec![artist("r1", "Radiohead")]));
		assert_eq!((lib.albums.len(), lib.followed.len()), (1, 1));
	}
}
