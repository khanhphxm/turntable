//! Library navigation, selection, filtering and application of worker responses.

use std::{collections::HashMap, sync::Arc};

use super::{
	Conn, Entry, Facet, Home, HomeFeed, HomeSection, HomeStyle, Kind, Listing, Request, Response, Route, Sort, View,
	added::{RefreshStats, TrackId},
};
use crate::spotify::{self, Album, Artist, Playlist, SearchResults, Track, User, id_of};

#[derive(Debug, Default)]
pub struct Library {
	pub conn:                     Conn,
	pub me:                       Option<User>,
	pub playlists:                Vec<Playlist>,
	/// Native playlist/folder order. Metadata remains in `playlists`.
	pub rootlist:                 Vec<super::RootEntry>,
	pub(crate) expanded_folders:  std::collections::HashSet<String>,
	/// Albums saved to the library, most recently added first.
	pub albums:                   Vec<Album>,
	/// Artists the user follows.
	pub followed:                 Vec<Artist>,
	/// Explicit pins, in pin order; restored separately for each signed-in account.
	pub(crate) pinned:            Vec<String>,
	pub(crate) excluded:          std::collections::HashSet<String>,
	/// What Your Library lists: the kind chips that are on (none: every kind), the order, the text
	/// filter, and how the rows are laid out.
	pub kinds:                    Vec<Kind>,
	pub sort:                     Sort,
	pub filter:                   String,
	pub view:                     View,
	pub route:                    Route,
	pub listing:                  Option<Listing>,
	pub query:                    String,
	pub search:                   Option<SearchResults>,
	pub home:                     Home,
	pub home_facet:               Facet,
	pub(crate) home_feeds:        HashMap<Facet, HomeFeed>,
	/// A Show all page on Home; the UI can keep this URI in its own navigation history.
	pub home_section:             Option<HomeSection>,
	pub home_section_loading:     bool,
	pub(crate) home_section_next: Option<u32>,
	/// Artists fetched for the now-playing view, by id. Together with overviews,
	/// retained for the last 128 distinct artists; listings own their visible data.
	pub artists:                  HashMap<String, Artist>,
	/// What the web player says about them, by id.
	pub overviews:                HashMap<String, spotify::ArtistOverview>,
	pub(crate) artist_order:      std::collections::VecDeque<String>,
	/// Who made the song that last asked: (track URI, credits).
	pub credits:                  Option<(String, Vec<spotify::Credit>)>,
	/// Whether the URI last checked is in Liked Songs.
	pub liked:                    Option<(String, bool)>,
	/// The most recent completed owned-playlist refresh for the current Session.
	pub added_refresh:            Option<RefreshStats>,
	/// Immutable, sorted owned-playlist membership delivered by the library worker.
	pub(crate) owned_tracks:      Arc<[TrackId]>,
	/// Complete Liked Songs membership, when the playback Session has loaded it.
	pub(crate) liked_tracks:      Option<Arc<[TrackId]>>,
	/// Per-track answers and loaded Liked Songs pages, retained across navigation.
	pub(crate) liked_cache:       HashMap<TrackId, bool>,
	/// Selected row in the content area, if any.
	pub selected:                 Option<usize>,
	/// Whether the selection is drawn: only after keyboard navigation or a
	/// click, never just because a list loaded.
	pub show_selection:           bool,
	/// A track to select once the page being opened lists it (a song card opens its album there).
	pub reveal:                   Option<String>,
	pub error:                    Option<String>,
}

impl Library {
	pub fn home_feed(&self) -> Option<&HomeFeed> {
		self.home_feeds.get(&self.home_facet)
	}

	pub(crate) fn select_home_facet(&mut self, facet: Facet) -> Request {
		self.home_facet = facet;
		self.route = Route::Home;
		self.selected = None;
		self.show_selection = false;
		self.reveal = None;
		self.close_home_section();
		self.error = None;
		Request::HomeFacet(facet)
	}

	pub(crate) fn open_home_section(&mut self, uri: String) -> Request {
		self.route = Route::Home;
		self.selected = None;
		self.show_selection = false;
		self.reveal = None;
		self.error = None;
		self.home_section =
			Some(self.home_feed().and_then(|feed| feed.sections.iter().find(|section| section.uri == uri)).cloned().unwrap_or_else(|| {
				HomeSection {
					uri:    uri.clone(),
					title:  String::new(),
					kicker: None,
					style:  HomeStyle::Shelf,
					items:  Vec::new(),
					more:   false,
				}
			}));
		self.home_section_loading = true;
		self.home_section_next = None;
		Request::HomeSectionMore { uri, offset: 0 }
	}

	pub(crate) fn close_home_section(&mut self) {
		self.home_section = None;
		self.home_section_loading = false;
		self.home_section_next = None;
		self.error = None;
	}

	pub(crate) fn home_section_more(&mut self) -> Option<Request> {
		if self.route != Route::Home || self.home_section_loading {
			return None;
		}
		let section = self.home_section.as_ref()?;
		let offset = self.home_section_next?;
		self.home_section_loading = true;
		Some(Request::HomeSectionMore { uri: section.uri.clone(), offset })
	}

	fn apply_home_section(&mut self, mut section: HomeSection, offset: u32, next: Option<u32>) {
		if self.route != Route::Home {
			return;
		}
		let Some(current) = &mut self.home_section else { return };
		if current.uri != section.uri || (offset != 0 && self.home_section_next != Some(offset)) {
			return;
		}
		if offset == 0 {
			*current = section;
		} else {
			current.items.append(&mut section.items);
			current.more = section.more;
		}
		self.home_section_next = next;
		self.home_section_loading = false;
		self.error = None;
	}

	/// What is already known about Liked Songs, without network work or allocation.
	pub(crate) fn track_liked(&self, uri: &str, track: &TrackId) -> Option<bool> {
		if let Some((known_uri, liked)) = &self.liked
			&& known_uri == uri
		{
			return Some(*liked);
		}
		self.liked_cache.get(track).copied().or_else(|| self.liked_tracks.as_ref().map(|tracks| tracks.binary_search(track).is_ok()))
	}

	pub(crate) fn cache_liked(&mut self, uri: &str, liked: bool) -> bool {
		let Ok(track) = TrackId::from_uri(uri) else { return false };
		self.liked_cache.insert(track, liked);
		if let Some((known_uri, known_liked)) = &mut self.liked
			&& known_uri == uri
		{
			*known_liked = liked;
		}
		true
	}

	/// Tracks currently listed in the content area (for selection and playback).
	pub fn rows(&self) -> &[Track] {
		match self.route {
			Route::Search => self.search.as_ref().map_or(&[][..], |s| &s.tracks.items),
			Route::Home if self.home_section.is_none() && self.home_feed().is_none() => self.home.songs().1,
			Route::Home | Route::Library | Route::Folder(_) => &[],
			_ => self.listing.as_ref().map_or(&[][..], |l| &l.tracks),
		}
	}

	/// What the page being shown has fully drawn, for traces (see [`crate::net::trace`]): `home`, `album:<id>`,
	/// `artist:<id>:header+popular+about` ... A page still waiting for its data says `:loading`.
	pub fn ready_label(&self) -> String {
		let loaded = self.listing.as_ref().filter(|l| !l.loading);
		match &self.route {
			Route::Home if self.home.loaded || self.home_feed().is_some() => "home".into(),
			Route::Home => "home:loading".into(),
			Route::Search if self.search.is_some() => "search".into(),
			Route::Search => "search:empty".into(),
			Route::Library => "library".into(),
			Route::Folder(id) => format!("folder:{id}"),
			Route::Liked if loaded.is_some() => "liked".into(),
			Route::Playlist(id) if loaded.is_some() => format!("playlist:{id}"),
			Route::Album(id) if loaded.is_some() => format!("album:{id}"),
			Route::Artist(id) => match loaded {
				Some(l) => format!("artist:{id}:header{}", if l.popular_loaded { "+popular" } else { "" }),
				None => format!("artist:{id}:loading"),
			},
			route => format!("{route:?}:loading"),
		}
	}

	/// Visible tree rows without depth, for library grids and filter keyboard navigation.
	/// Folder pages use `folder_entries`; sidebars use `tree` to keep nesting explicit.
	pub fn entries(&self, filtered: bool) -> Vec<Entry<'_>> {
		self.tree(filtered).into_iter().map(|row| row.entry).collect()
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
		self.close_home_section();
		self.selected = None;
		self.show_selection = false;
		self.reveal = None;
		self.error = None;
		self.route = route.clone();
		match &route {
			Route::Home => vec![Request::HomeFacet(self.home_facet)],
			Route::Search | Route::Library | Route::Folder(_) => {
				self.listing = None;
				Vec::new()
			},
			Route::Artist(id) => {
				self.listing = Some(Listing { loading: true, ..self.placeholder(&route) });
				// Native title and popular rows arrive first; Spotify's overview carries the biography.
				vec![Request::Open(route.clone(), 0), Request::ArtistPopular(id.clone())]
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
				Listing {
					kicker: ("Playlist").into(),
					title: p.map_or_else(String::new, |p| p.name.clone()),
					description: p
						.and_then(|p| p.description.as_ref())
						.filter(|description| Playlist::has_description(description))
						.cloned(),
					public: p.and_then(|p| p.public),
					owned: p.is_some_and(|p| self.me.as_ref().is_some_and(|me| me.id == p.owner.id)),
					subtitle: p.map(|p| format!("{} • {} songs", p.owner_name(), p.track_count.unwrap_or_default())).unwrap_or_default(),
					context_uri: p.map(|p| p.uri.clone()),
					cover_url: p.and_then(|p| spotify::pick_image(&p.images, 300)).map(|i| i.url.clone()),
					..Listing::default()
				}
			},
			Route::Artist(id) => {
				let a = self.artists.get(id);
				Listing {
					kicker: ("Artist").into(),
					title: a.map_or_else(String::new, |a| a.name.clone()),
					context_uri: Some(format!("spotify:artist:{id}")),
					cover_url: a.and_then(|a| spotify::pick_image(&a.images, 300)).map(|i| i.url.clone()),
					..Listing::default()
				}
			},
			_ => Listing { kicker: ("Album").into(), ..Listing::default() },
		}
	}

	pub fn apply(&mut self, response: Response) {
		match response {
			Response::Connected { me, playlists } => self.apply_connected(me, playlists),
			Response::Rootlist { account, entries } => self.apply_rootlist(&account, entries),
			Response::FolderExpansion { account, expanded } => {
				if self.me.as_ref().is_some_and(|me| me.id == account) {
					self.expanded_folders = expanded;
				}
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
			Response::Liked(uri, liked) => self.apply_liked(uri, liked),
			Response::Exclusions { account, uris } => {
				if self.me.as_ref().is_some_and(|me| me.id == account) {
					self.excluded = uris.into_iter().collect();
				}
			},
			Response::Added { tracks, stats } => {
				self.owned_tracks = tracks;
				if stats.is_some() || self.owned_tracks.is_empty() {
					self.added_refresh = stats;
				}
			},
			Response::LikedTracks(tracks) => self.apply_liked_tracks(tracks),
			Response::Home(home) => self.home = home,
			Response::HomeRecentTracks(tracks) => self.home.take_recent_tracks(tracks),
			Response::HomeFeed { facet, feed } => {
				self.home_feeds.insert(facet, feed);
			},
			Response::HomeSection { section, offset, next_offset } => self.apply_home_section(section, offset, next_offset),
			Response::HomeSectionFailed { uri, message } => self.fail_home_section(&uri, message),
			Response::Artist(artist) => {
				let id = id_of(&artist.uri).to_owned();
				self.remember_artist(&id);
				self.artists.insert(id, artist);
			},
			Response::ArtistPopular(id, popular) => self.apply_popularity(id, popular),
			Response::ArtistCard(id, overview) => {
				self.remember_artist(&id);
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
			Response::Playlists(playlists) => self.apply_playlists(playlists),
			Response::PlaylistDetails(playlist) => {
				if let Some(known) = self.playlists.iter_mut().find(|known| known.id == playlist.id) {
					*known = playlist;
				}
			},
			Response::Pins { account, uris } => {
				if self.me.as_ref().is_some_and(|me| me.id == account) {
					self.pinned = uris;
				}
			},
			Response::Albums(albums) => self.albums = albums,
			Response::Following(artists) => self.followed = artists,
			Response::Created(playlist) => self.apply_created(playlist),
			Response::Removed(id, uri, row) => self.removed_playlist_row(&id, &uri, row),
			Response::Moved(id, from, to) => self.moved_playlist_row(&id, from, to),
			// Connection, playback and queue replies belong to `State`, not to the library.
			Response::Offline(_)
			| Response::FolderCreated { .. }
			| Response::HomeUnavailable { .. }
			| Response::Queue(..)
			| Response::ContextToPlaylist(..)
			| Response::Done(_)
			| Response::Devices(_)
			| Response::Playback(_)
			| Response::LastPlayback { .. }
			| Response::Transferred
			| Response::PlayerError(_) => {},
			Response::Listing { route, offset, listing } => self.apply_listing(&route, offset, listing),
			Response::Search { query, results } => self.apply_search(&query, results),
			Response::Error { route, message } => self.apply_error(route.as_ref(), message),
			Response::LoginFailed(e) => self.conn = Conn::Failed(e),
		}
	}

	fn apply_rootlist(&mut self, account: &str, entries: Vec<super::RootEntry>) {
		if self.me.as_ref().is_some_and(|me| me.id == account) {
			self.rootlist = entries;
			if let Route::Folder(id) = &self.route
				&& self.folder(id).is_none()
			{
				self.route = Route::Library;
			}
		}
	}

	fn apply_search(&mut self, query: &str, results: SearchResults) {
		if query == self.query.trim() {
			self.search = Some(results);
			self.selected = None;
			self.show_selection = false;
		}
	}

	fn fail_home_section(&mut self, uri: &str, message: String) {
		if self.route == Route::Home && self.home_section.as_ref().is_some_and(|section| section.uri == uri) {
			self.home_section_loading = false;
			self.error = Some(message);
		}
	}

	fn apply_popularity(&mut self, id: String, popular: super::Popularity) {
		if matches!(&self.route, Route::Artist(known) if known == &id)
			&& let Some(listing) = &mut self.listing
		{
			listing.popular_loaded = true;
			listing.tracks = popular.tracks;
			listing.plays = popular.plays;
			listing.accent = popular.accent;
			listing.take_artist_overview(popular.overview.clone());
		}
		if let Some(overview) = popular.overview {
			self.remember_artist(&id);
			self.overviews.insert(id, overview);
		}
	}

	fn remember_artist(&mut self, id: &str) {
		if self.artists.contains_key(id) || self.overviews.contains_key(id) {
			return;
		}
		if self.artist_order.len() == 128
			&& let Some(oldest) = self.artist_order.pop_front()
		{
			self.artists.remove(&oldest);
			self.overviews.remove(&oldest);
		}
		self.artist_order.push_back(id.to_owned());
	}

	fn apply_created(&mut self, mut playlist: Playlist) {
		if let Some(me) = &self.me {
			playlist.normalize(&me.id, me.display_name.as_deref());
		}
		self.playlists.insert(0, playlist);
	}

	fn apply_playlists(&mut self, mut playlists: Vec<Playlist>) {
		if let Some(me) = &self.me {
			for playlist in &mut playlists {
				playlist.normalize(&me.id, me.display_name.as_deref());
			}
		}
		self.playlists = playlists;
	}

	fn removed_playlist_row(&mut self, id: &str, uri: &str, row: u32) {
		if matches!(&self.route, Route::Playlist(current) if current == id)
			&& let Some(listing) = &mut self.listing
			&& let Ok(row) = usize::try_from(row)
			&& listing.tracks.get(row).is_some_and(|track| track.uri == uri)
		{
			listing.tracks.remove(row);
			if row < listing.added_at.len() {
				listing.added_at.remove(row);
			}
			if row < listing.added_by.len() {
				listing.added_by.remove(row);
			}
			if row < listing.plays.len() {
				listing.plays.remove(row);
			}
			listing.total = listing.total.saturating_sub(1);
			listing.next_offset = listing.next_offset.map(|offset| offset.saturating_sub(1));
			self.selected =
				self.selected.and_then(|selected| if selected == row { None } else { Some(selected - usize::from(selected > row)) });
		}
	}

	fn moved_playlist_row(&mut self, id: &str, from: u32, to: u32) {
		if matches!(&self.route, Route::Playlist(current) if current == id)
			&& let Some(listing) = &mut self.listing
			&& let (Ok(from), Ok(to)) = (usize::try_from(from), usize::try_from(to))
			&& from < listing.tracks.len()
			&& to < listing.tracks.len()
		{
			let track = listing.tracks.remove(from);
			listing.tracks.insert(to, track);
			if !listing.added_at.is_empty() {
				listing.added_at.resize(listing.tracks.len(), None);
				let added_at = listing.added_at.remove(from);
				listing.added_at.insert(to, added_at);
			}
			if !listing.added_by.is_empty() {
				listing.added_by.resize(listing.tracks.len(), None);
				let added_by = listing.added_by.remove(from);
				listing.added_by.insert(to, added_by);
			}
			if listing.plays.len() == listing.tracks.len() {
				let plays = listing.plays.remove(from);
				listing.plays.insert(to, plays);
			}
			self.selected = self.selected.map(|row| {
				if row == from {
					to
				} else if from < row && row <= to {
					row - 1
				} else if to <= row && row < from {
					row + 1
				} else {
					row
				}
			});
		}
	}

	fn apply_connected(&mut self, me: User, mut playlists: Vec<Playlist>) {
		if self.me.as_ref().is_some_and(|old| old.id != me.id) {
			self.liked = None;
			self.liked_cache.clear();
			self.pinned.clear();
			self.excluded.clear();
			self.rootlist.clear();
			self.expanded_folders.clear();
			self.home = Home::default();
			self.home_feeds.clear();
			self.home_facet = Facet::All;
			self.close_home_section();
		}
		for playlist in &mut playlists {
			playlist.normalize(&me.id, me.display_name.as_deref());
		}
		self.conn = Conn::Ready;
		self.me = Some(me);
		self.playlists = playlists;
		// The albums and artists load next: none of a previous account's meanwhile.
		self.albums.clear();
		self.followed.clear();
	}

	fn apply_liked(&mut self, uri: String, liked: bool) {
		self.cache_liked(&uri, liked);
		self.liked = Some((uri, liked));
	}

	fn apply_liked_tracks(&mut self, tracks: Arc<[TrackId]>) {
		self.liked_cache.clear();
		if let Some((uri, liked)) = &mut self.liked {
			*liked = TrackId::from_uri(uri).is_ok_and(|track| tracks.binary_search(&track).is_ok());
		}
		self.liked_tracks = Some(tracks);
	}

	fn apply_error(&mut self, route: Option<&Route>, message: String) {
		// An error from a page the user already left isn't about this one.
		if route.is_some_and(|r| *r != self.route) {
			return;
		}
		if let Some(l) = &mut self.listing {
			l.loading = false;
		}
		if self.route == Route::Home {
			self.home.loaded = true;
		}
		self.error = Some(message);
	}

	fn apply_listing(&mut self, route: &Route, offset: u32, mut listing: Listing) {
		if *route == Route::Liked {
			for track in &listing.tracks {
				if let Ok(id) = TrackId::from_uri(&track.uri)
					&& self.liked_tracks.as_ref().is_none_or(|tracks| tracks.binary_search(&id).is_err())
				{
					self.liked_cache.entry(id).or_insert(true);
				}
			}
		}
		if *route != self.route {
			return;
		}
		match (&mut self.listing, offset) {
			(Some(current), o) if o > 0 => {
				if !current.added_at.is_empty() || !listing.added_at.is_empty() {
					current.added_at.resize(current.tracks.len(), None);
					listing.added_at.resize(listing.tracks.len(), None);
					current.added_at.extend(listing.added_at);
				}
				if !current.added_by.is_empty() || !listing.added_by.is_empty() {
					current.added_by.resize(current.tracks.len(), None);
					listing.added_by.resize(listing.tracks.len(), None);
					current.added_by.extend(listing.added_by);
				}
				current.tracks.extend(listing.tracks);
				current.next_offset = listing.next_offset;
				current.loading = false;
			},
			(current, _) => {
				// An artist's popular songs may arrive before its releases.
				if let (Route::Artist(_), Some(early)) = (route, current.take()) {
					if early.popular_loaded && !listing.popular_loaded {
						(listing.tracks, listing.plays, listing.popular_loaded) = (early.tracks, early.plays, true);
					}
					if listing.overview.is_none() {
						listing.take_artist_overview(early.overview);
					}
					if listing.accent.is_none() {
						listing.accent = early.accent;
					}
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
	fn artist_extras_reach_the_listing_in_either_response_order() {
		let overview = spotify::ArtistOverview {
			name: "Artist".into(),
			biography: Some("The artist's Spotify biography".into()),
			verified: true,
			world_rank: Some(84),
			cities: vec![spotify::City { name: "London".into(), country: "GB".into(), listeners: 740_990 }],
			header: Some("header".into()),
			gallery: vec!["photo".into()],
			latest_release: Some(album("latest", "Latest release", "Artist")),
			pick: Some(spotify::Pick {
				uri:        "spotify:album:pick".into(),
				title:      "Pick".into(),
				comment:    "Listen".into(),
				subtitle:   "Album".into(),
				image:      Some("cover".into()),
				background: Some("backdrop".into()),
			}),
			concerts: vec![spotify::Concert {
				uri:        "spotify:concert:show".into(),
				title:      "Show".into(),
				start_date: "2027-06-08T19:00+09:00".into(),
				venue:      "Arena".into(),
				city:       "City".into(),
				festival:   false,
			}],
			..spotify::ArtistOverview::default()
		};
		for (popular_first, header_title) in [(false, ""), (true, ""), (false, "Header name"), (true, "Header name")] {
			let route = Route::Artist("artist".into());
			let mut lib = Library { route: route.clone(), listing: Some(Listing::default()), ..Library::default() };
			let header = Response::Listing { route, offset: 0, listing: Listing { title: header_title.into(), ..Listing::default() } };
			let popular = Response::ArtistPopular("artist".into(), super::super::Popularity {
				overview: Some(overview.clone()),
				..super::super::Popularity::default()
			});
			if popular_first {
				lib.apply(popular);
				assert_eq!(lib.listing.as_ref().unwrap().title, "Artist", "pathfinder names the page without any header response");
				lib.apply(header);
			} else {
				lib.apply(header);
				lib.apply(popular);
			}
			let listing = lib.listing.as_ref().unwrap();
			assert_eq!(listing.title, if header_title.is_empty() { "Artist" } else { header_title });
			assert!(listing.popular_loaded);
			assert_eq!(listing.overview.as_ref(), Some(&overview));
			assert_eq!(lib.overviews.get("artist"), Some(&overview));
		}
	}

	#[test]
	fn artist_navigation_only_requests_spotify_data() {
		let requests = Library::default().navigate(Route::Artist("artist".into()));
		assert_eq!(requests.len(), 2);
		assert!(requests.iter().any(|request| matches!(request, Request::Open(Route::Artist(id), 0) if id == "artist")));
		assert!(requests.iter().any(|request| matches!(request, Request::ArtistPopular(id) if id == "artist")));
	}

	#[test]
	fn artist_metadata_is_bounded_and_revisits_can_be_cached_again() {
		let mut lib = Library::default();
		for index in 0..300 {
			let id = format!("artist-{index}");
			lib.apply(Response::Artist(artist(&id, &id)));
			lib.apply(Response::ArtistCard(id, spotify::ArtistOverview::default()));
		}
		assert_eq!(lib.artists.len(), 128);
		assert_eq!(lib.overviews.len(), 128);
		assert_eq!(lib.artist_order.len(), 128);
		assert!(!lib.artists.contains_key("artist-0"));
		assert!(!lib.overviews.contains_key("artist-0"));
		lib.apply(Response::Artist(artist("artist-0", "Revisited")));
		assert_eq!(lib.artists.get("artist-0").unwrap().name, "Revisited");
		assert_eq!(lib.artist_order.len(), 128);
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
	fn entries_do_not_pin_liked_songs_without_the_users_choice() {
		let lib = library();
		assert_eq!(titles(&lib, true), ["Road trip", "focus", "Radiohead", "daft punk", "Blonde", "Abbey Road", "Liked Songs"]);
		let routes: Vec<Route> = lib.entries(true).iter().map(Entry::route).collect();
		assert_eq!(routes[..3], [Route::Playlist("p1".into()), Route::Playlist("p2".into()), Route::Artist("r1".into())]);
		assert_eq!(routes[4], Route::Album("a1".into()));
		assert_eq!(routes[6], Route::Liked);
	}

	#[test]
	fn chips_keep_their_kinds_and_none_keeps_all() {
		let mut lib = library();
		lib.kinds = vec![Kind::Artist];
		assert_eq!(titles(&lib, true), ["Radiohead", "daft punk"], "Liked Songs is a playlist");
		lib.kinds = vec![Kind::Album, Kind::Playlist];
		assert_eq!(titles(&lib, true), ["Road trip", "focus", "Blonde", "Abbey Road", "Liked Songs"]);
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
	fn liked_songs_participates_in_every_chosen_order() {
		let mut lib = library();
		lib.sort = Sort::Added;
		assert_eq!(
			titles(&lib, true),
			["Road trip", "focus", "Radiohead", "daft punk", "Blonde", "Abbey Road", "Liked Songs"],
			"as Spotify lists them"
		);
		lib.sort = Sort::Alphabetical;
		assert_eq!(titles(&lib, true), ["Abbey Road", "Blonde", "daft punk", "focus", "Liked Songs", "Radiohead", "Road trip"]);
		// By creator: alex, daft punk, Frank Ocean, Radiohead, The Beatles, Zed.
		lib.sort = Sort::Creator;
		assert_eq!(titles(&lib, true), ["Liked Songs", "Road trip", "daft punk", "Blonde", "Radiohead", "Abbey Road", "focus"]);
	}

	#[test]
	fn recents_put_what_was_played_lately_first_and_keep_spotifys_order_for_the_rest() {
		let mut lib = library();
		lib.home.recents = vec![Tile::album(&lib.albums[1]), Tile::artist(&lib.followed[1]), Tile::playlist(&lib.playlists[1])];
		assert_eq!(titles(&lib, true), ["Abbey Road", "daft punk", "focus", "Road trip", "Radiohead", "Blonde", "Liked Songs"]);
		lib.home.recents.insert(1, Tile::liked());
		assert_eq!(titles(&lib, true), ["Abbey Road", "Liked Songs", "daft punk", "focus", "Road trip", "Radiohead", "Blonde"]);
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

	#[test]
	fn created_playlists_and_cached_headers_use_profile_names_and_no_null_description() {
		let mut lib = library();
		let mut created = playlist("scratch", "My Playlist", "alex");
		created.owner.display_name = None;
		created.description = Some("null".into());
		created.track_count = Some(0);
		lib.apply(Response::Created(created));
		lib.navigate(Route::Playlist("scratch".into()));
		let header = lib.listing.as_ref().unwrap();
		assert_eq!(header.description, None);
		assert_eq!(header.subtitle, "Alex • 0 songs");
		assert_eq!(Entry::Playlist(&lib.playlists[0]).sub(), "Playlist • Alex");
		let mut refreshed = lib.playlists.clone();
		refreshed[0].owner.display_name = None;
		lib.apply(Response::Playlists(refreshed));
		assert_eq!(lib.playlists[0].owner_name(), "Alex");
		let mut other = playlist("other", "Other", "opaque-id");
		other.owner.display_name = None;
		assert_eq!(Entry::Playlist(&other).sub(), "Playlist • Spotify user");
	}

	#[test]
	fn playlist_row_removal_preserves_the_next_raw_page_offset() {
		let mut lib = library();
		lib.route = Route::Playlist("p1".into());
		let track = |uri: &str| serde_json::from_value(serde_json::json!({ "uri": uri })).unwrap();
		lib.listing = Some(Listing {
			tracks: vec![track("a"), track("b"), track("a")],
			added_at: vec![Some(10), None, Some(30)],
			added_by: vec![Some("alex".into()), None, Some("bo".into())],
			total: 5,
			next_offset: Some(3),
			..Listing::default()
		});
		lib.apply(Response::Removed("p1".into(), "a".into(), 0));
		let listing = lib.listing.as_ref().unwrap();
		assert_eq!(listing.tracks.len(), 2);
		assert_eq!(listing.added_at, [None, Some(30)]);
		assert_eq!(listing.added_by(1).as_deref(), Some("bo"));
		assert_eq!(listing.total, 4);
		assert_eq!(listing.next_offset, Some(2));
	}

	#[test]
	fn playlist_added_attributes_follow_duplicate_occurrences_through_moves_and_pages() {
		let route = Route::Playlist("p1".into());
		let track = |uri: &str| serde_json::from_value(serde_json::json!({ "uri": uri })).unwrap();
		let mut lib = Library { route: route.clone(), ..Library::default() };
		lib.apply(Response::Listing {
			route:   route.clone(),
			offset:  0,
			listing: Listing {
				tracks: vec![track("a"), track("b"), track("a")],
				added_at: vec![Some(10), None, Some(30)],
				added_by: vec![Some("alex".into()), None, Some("bo".into())],
				..Listing::default()
			},
		});
		lib.apply(Response::Moved("p1".into(), 2, 0));
		let listing = lib.listing.as_ref().unwrap();
		assert_eq!(listing.added_at, [Some(30), Some(10), None]);
		assert_eq!(listing.added_by(0).as_deref(), Some("bo"));
		lib.apply(Response::Moved("p1".into(), 0, 2));
		lib.apply(Response::Listing {
			route:   route.clone(),
			offset:  3,
			listing: Listing { tracks: vec![track("c")], ..Listing::default() },
		});
		lib.apply(Response::Listing {
			route,
			offset: 4,
			listing: Listing {
				tracks: vec![track("a")],
				added_at: vec![Some(50)],
				added_by: vec![Some("cy".into())],
				..Listing::default()
			},
		});
		let listing = lib.listing.as_ref().unwrap();
		assert_eq!(listing.tracks.iter().map(|track| track.uri.as_str()).collect::<Vec<_>>(), ["a", "b", "a", "c", "a"]);
		assert_eq!(listing.added_at, [Some(10), None, Some(30), None, Some(50)]);
		assert_eq!(listing.added_by(4).as_deref(), Some("cy"));
		assert_eq!(listing.added_at(5), None);
		assert_eq!(listing.added_by(5), None);
	}

	#[test]
	fn dated_pages_pad_old_snapshots_without_shifting_added_attributes() {
		let route = Route::Liked;
		let track = || serde_json::from_value(serde_json::json!({ "uri": "a" })).unwrap();
		let mut lib =
			Library { route: route.clone(), listing: Some(Listing { tracks: vec![track()], ..Listing::default() }), ..Library::default() };
		lib.apply(Response::Listing {
			route,
			offset: 1,
			listing: Listing { tracks: vec![track()], added_at: vec![Some(20)], ..Listing::default() },
		});
		assert_eq!(lib.listing.as_ref().unwrap().added_at, [None, Some(20)]);
	}
}
