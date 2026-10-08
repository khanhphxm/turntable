//! Routes, entries and page data shared by the app, views and authenticated library worker.

use serde::{Deserialize, Deserializer, Serialize, de};

use crate::{
	sources::external,
	spotify::{self, Album, Artist, Playlist, Track, id_of},
};

/// Rows fetched per page of a track listing.
pub const PAGE: u32 = 50;

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Route {
	#[default]
	Home,
	Search,
	/// The library as a page (the compact layout has no sidebar).
	Library,
	Liked,
	Playlist(String),
	Album(String),
	Artist(String),
}

/// Something a page links to: a playlist, an album, an artist or Liked Songs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tile {
	/// Where a click goes.
	pub route: Route,
	/// What its play button plays (`None`: nothing to play as a whole).
	pub play:  Option<String>,
	pub title: String,
	/// "Playlist • Alex", "Album • Daft Punk", "Artist".
	pub sub:   String,
	pub image: Option<String>,
}

impl Tile {
	pub fn playlist(p: &Playlist) -> Self {
		let owner = p.owner_name();
		Self {
			route: Route::Playlist(p.id.clone()),
			play:  Some(p.uri.clone()),
			title: p.name.clone(),
			sub:   format!("Playlist • {owner}"),
			image: spotify::pick_image(&p.images, 300).map(|i| i.url.clone()),
		}
	}

	pub fn album(a: &Album) -> Self {
		Self {
			route: Route::Album(id_of(&a.uri).to_owned()),
			play:  Some(a.uri.clone()),
			title: a.name.clone(),
			sub:   album_sub(a),
			image: spotify::pick_image(&a.images, 300).map(|i| i.url.clone()),
		}
	}

	pub fn artist(a: &Artist) -> Self {
		Self {
			route: Route::Artist(id_of(&a.uri).to_owned()),
			play:  Some(a.uri.clone()),
			title: a.name.clone(),
			sub:   "Artist".into(),
			image: spotify::pick_image(&a.images, 300).map(|i| i.url.clone()),
		}
	}

	/// A release on its artist's page: the year and kind instead of the artist.
	pub fn release(a: &Album) -> Self {
		let year = a.release_date.as_deref().and_then(|d| d.get(..4)).unwrap_or_default();
		let kind = album_kind(a);
		Self { sub: if year.is_empty() { kind.to_owned() } else { format!("{year} • {kind}") }, ..Self::album(a) }
	}

	pub fn liked() -> Self {
		Self { route: Route::Liked, play: None, title: "Liked Songs".into(), sub: "Playlist".into(), image: None }
	}
}

/// "Album", "Single" or "Compilation".
pub(super) fn album_kind(a: &Album) -> &'static str {
	match a.album_type.as_str() {
		"single" => "Single",
		"compilation" => "Compilation",
		_ => "Album",
	}
}

/// "Album • Daft Punk".
fn album_sub(a: &Album) -> String {
	let by = a.artists.iter().map(|x| x.name.as_str()).collect::<Vec<_>>().join(", ");
	let kind = album_kind(a);
	if by.is_empty() { kind.to_owned() } else { format!("{kind} • {by}") }
}

/// What a row of Your Library is, in the order of its filter chips.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
	Playlist,
	Artist,
	Album,
}

impl Kind {
	pub const ALL: [Self; 3] = [Self::Playlist, Self::Artist, Self::Album];

	/// The filter chip's label.
	pub const fn plural(self) -> &'static str {
		match self {
			Self::Playlist => "Playlists",
			Self::Artist => "Artists",
			Self::Album => "Albums",
		}
	}
}

/// The order of Your Library's rows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Sort {
	/// What was played lately first, the rest as Spotify lists them.
	#[default]
	Recents,
	/// As Spotify lists them: newest first.
	Added,
	Alphabetical,
	/// By who made it: a playlist's owner, an album's artist.
	Creator,
}

impl Sort {
	pub const ALL: [Self; 4] = [Self::Recents, Self::Added, Self::Alphabetical, Self::Creator];

	pub const fn label(self) -> &'static str {
		match self {
			Self::Recents => "Recents",
			Self::Added => "Recently added",
			Self::Alphabetical => "Alphabetical",
			Self::Creator => "Creator",
		}
	}
}

/// How Your Library's rows are laid out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum View {
	/// A line each, without covers.
	Compact,
	#[default]
	List,
	/// Covers three across, titled.
	Grid,
	/// Covers two across, titled, with their kind and creator.
	Large,
}

impl View {
	pub const ALL: [Self; 4] = [Self::Compact, Self::List, Self::Grid, Self::Large];

	pub const fn label(self) -> &'static str {
		match self {
			Self::Compact => "Compact list",
			Self::List => "List",
			Self::Grid => "Grid",
			Self::Large => "Large grid",
		}
	}
}

/// A row of Your Library.
#[derive(Debug, Clone, Copy)]
pub enum Entry<'a> {
	/// Liked Songs, with the name of whose they are (empty while unknown).
	Liked(&'a str),
	Playlist(&'a Playlist),
	Album(&'a Album),
	Artist(&'a Artist),
}

impl Entry<'_> {
	pub fn route(&self) -> Route {
		match self {
			Self::Liked(_) => Route::Liked,
			Self::Playlist(p) => Route::Playlist(p.id.clone()),
			Self::Album(a) => Route::Album(id_of(&a.uri).to_owned()),
			Self::Artist(a) => Route::Artist(id_of(&a.uri).to_owned()),
		}
	}

	pub fn title(&self) -> &str {
		match self {
			Self::Liked(_) => "Liked Songs",
			Self::Playlist(p) => &p.name,
			Self::Album(a) => &a.name,
			Self::Artist(a) => &a.name,
		}
	}

	/// Who made it: a playlist's owner, an album's first artist, an artist themselves.
	pub(super) fn creator(&self) -> &str {
		match self {
			Self::Liked(owner) => owner,
			Self::Playlist(p) => p.owner_name(),
			Self::Album(a) => a.artists.first().map_or("", |x| x.name.as_str()),
			Self::Artist(a) => &a.name,
		}
	}

	/// The cover's URL: the smallest picture at least `px` wide.
	pub fn image(&self, px: u32) -> Option<&str> {
		let images = match self {
			Self::Liked(_) => return None,
			Self::Playlist(p) => &p.images,
			Self::Album(a) => &a.images,
			Self::Artist(a) => &a.images,
		};
		spotify::pick_image(images, px).map(|i| i.url.as_str())
	}

	/// "Playlist • Alex", "Album • Daft Punk", "Artist".
	pub fn sub(&self) -> String {
		match self {
			Self::Liked(_) | Self::Playlist(_) => match self.creator() {
				"" => "Playlist".to_owned(),
				by => format!("Playlist • {by}"),
			},
			Self::Album(a) => album_sub(a),
			Self::Artist(_) => "Artist".to_owned(),
		}
	}

	/// Whether `route` opens this row.
	pub(super) fn opens(&self, route: &Route) -> bool {
		match (self, route) {
			(Self::Liked(_), Route::Liked) => true,
			(Self::Playlist(p), Route::Playlist(id)) => p.id == *id,
			(Self::Album(a), Route::Album(id)) => id_of(&a.uri) == id.as_str(),
			(Self::Artist(a), Route::Artist(id)) => id_of(&a.uri) == id.as_str(),
			_ => false,
		}
	}
}

/// `s` lowercased char by char: what filtering and ordering compare, without allocating.
pub(super) fn fold(s: &str) -> impl Iterator<Item = char> + '_ {
	s.chars().flat_map(char::to_lowercase)
}

/// The home page: what was played lately and what the user plays most.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Home {
	/// Recently played playlists, albums and artists, newest first, each once.
	pub recents:       Vec<Tile>,
	/// `None`: the sign-in predates the `user-top-read` permission (reconnect to see these).
	pub top_artists:   Option<Vec<Tile>>,
	pub top_tracks:    Option<Vec<Track>>,
	/// `None` as for top artists (`user-follow-read`).
	pub followed:      Option<Vec<Tile>>,
	/// Saved albums, most recently added first.
	pub albums:        Vec<Tile>,
	/// Recently played songs, newest first, each once.
	pub recent_tracks: Vec<Track>,
	pub loaded:        bool,
}

impl Home {
	/// The songs shelf: top songs when Spotify gives them, else recently played.
	pub fn songs(&self) -> (&'static str, &[Track]) {
		match &self.top_tracks {
			Some(t) if !t.is_empty() => ("Your top songs", t),
			_ => ("Recently played songs", &self.recent_tracks),
		}
	}
}

/// Whether the Web API side is usable.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Conn {
	/// No `client_id` yet: the setup flow asks for one.
	#[default]
	NeedsClientId,
	/// A client id but no token: the setup flow offers "Connect Spotify".
	NeedsLogin,
	/// The browser login is in progress; the URL is shown in case no browser opened.
	LoggingIn(Option<String>),
	Ready,
	Failed(String),
}

/// One track listing (a playlist, an album, Liked Songs).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Listing {
	#[serde(deserialize_with = "listing_kicker")]
	pub kicker:         std::borrow::Cow<'static, str>,
	pub title:          String,
	pub subtitle:       String,
	pub description:    Option<String>,
	pub public:         Option<bool>,
	/// Only the owner may edit playlist details; collaborators may edit songs.
	pub owned:          bool,
	pub cover_url:      Option<String>,
	/// The playable context (playlist/album URI); `None` for Liked Songs.
	pub context_uri:    Option<String>,
	pub tracks:         Vec<Track>,
	pub total:          u32,
	pub next_offset:    Option<u32>,
	pub loading:        bool,
	/// Why tracks aren't listed (e.g. a followed playlist under Dev Mode rules).
	pub note:           Option<String>,
	/// The album an album listing shows (its tracks lack it on the wire).
	pub album:          Option<Album>,
	/// Whether the user follows it (artists, others' playlists); `None` when it doesn't apply.
	pub followed:       Option<bool>,
	/// A playlist the user owns or collaborates on: songs can be removed.
	pub editable:       bool,
	/// Artist pages: releases by kind ("Albums", "Singles and EPs"), releases it
	/// appears on, and the artist for the About card.
	#[serde(skip)]
	pub discography:    Vec<(&'static str, Vec<Tile>)>,
	pub appears_on:     Vec<Tile>,
	pub artist:         Option<Artist>,
	/// Artist pages: Wikipedia's summary, and Spotify's play counts of `tracks`
	/// (empty: the counts are unknown). Albums: the counts of their songs.
	#[serde(skip)]
	pub about:          Option<external::About>,
	pub plays:          Vec<u64>,
	/// Artist pages: whether the popular songs have arrived (the table is a placeholder until then).
	pub popular_loaded: bool,
	/// What the web player says about the artist: listeners, photos, related artists.
	#[serde(skip)]
	pub overview:       Option<spotify::ArtistOverview>,
	/// The cover's dark tone, `#rrggbb`, for the header's gradient.
	pub accent:         Option<String>,
	/// The songs come from the web player: the Web API doesn't serve this playlist.
	pub web:            bool,
}

impl Listing {
	/// Shows a page of the web player's songs: its tracks, and where the next page starts.
	pub(super) fn take_web_page(&mut self, page: spotify::WebPlaylist, offset: u32) {
		let next = offset + page.items;
		self.total = page.total;
		self.next_offset = (page.items > 0 && next < page.total).then_some(next);
		self.tracks = page.tracks;
		self.web = true;
		self.note = None;
	}
}

/// Listing kinds are fixed UI labels; a snapshot never owns or leaks a new static string.
fn listing_kicker<'de, D: Deserializer<'de>>(deserializer: D) -> Result<std::borrow::Cow<'static, str>, D::Error> {
	struct Kicker;
	impl de::Visitor<'_> for Kicker {
		type Value = &'static str;

		fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
			formatter.write_str("a listing kind")
		}

		fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
			match value {
				"" => Ok(""),
				"Playlist" => Ok("Playlist"),
				"Album" => Ok("Album"),
				"Single" => Ok("Single"),
				"Compilation" => Ok("Compilation"),
				"Artist" => Ok("Artist"),
				_ => Err(E::custom("unknown listing kind")),
			}
		}
	}
	deserializer.deserialize_str(Kicker).map(std::borrow::Cow::Borrowed)
}
