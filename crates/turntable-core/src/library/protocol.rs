//! Requests to the authenticated library worker and responses delivered to the app.
//! Playlist attributes and library writes use the authenticated playback Session.
//! Only operations without that Session may fall back to the Web API.

use std::{
	collections::{HashMap, HashSet},
	sync::Arc,
};

use super::{
	Facet, Home, HomeFeed, HomeSection, Listing, Popularity, RootEntry, Route,
	added::{RefreshStats, TrackId},
};
use crate::spotify::{self, Album, Artist, Playlist, SearchResults, Track, User};

#[derive(Debug, Clone)]
pub enum Request {
	/// (Re)open the client and load the profile and playlists.
	Connect,
	/// Run the browser login, then connect.
	Login,
	/// Load the home page.
	Home,
	/// Select and load a personalised Home facet.
	HomeFacet(Facet),
	/// Show all in a Home section; offsets count raw Spotify items, including unmapped ones.
	HomeSectionMore {
		uri:    String,
		offset: u32,
	},
	/// Load the albums saved to the library.
	Albums,
	/// Load the artists the user follows.
	Following,
	/// Fetch an artist (for the now-playing view).
	Artist(String),
	/// An artist page's popular songs and overview (Spotify's web player).
	ArtistPopular(String),
	/// An artist's overview alone, for the now-playing card.
	ArtistCard(String),
	/// Who made a song (its URI).
	Credits(String),
	/// Follow (true) or unfollow an artist or playlist.
	SetFollowed(String, bool),
	/// Create a playlist and add these tracks to it.
	CreatePlaylist(String, Vec<String>),
	RenamePlaylist(String, spotify::PlaylistChanges),
	/// Native rootlist folders; no Web API equivalent.
	CreateFolder,
	RenameFolder(String, String),
	/// Remove markers only; children move to the containing folder or top level.
	DeleteFolder(String),
	MoveToFolder {
		uri:    String,
		folder: Option<String>,
	},
	FolderExpansion,
	SetFolderExpanded {
		id:       String,
		expanded: bool,
	},
	/// Resolve the current editable header without opening its playlist page.
	PlaylistDetails(String),
	SetCollaborative(String, bool),
	/// Every native context page, for the playlist picker rather than playback.
	AddContextToPlaylist(Route),
	/// Restore local account-scoped pin preferences.
	Pins,
	SetPinned {
		uri:    String,
		pinned: bool,
	},
	Exclusions,
	SetExcluded {
		uri:      String,
		excluded: bool,
	},
	/// Append tracks to a playlist (id, name for the notice, URIs).
	AddToPlaylist(String, String, Vec<String>),
	/// Remove one occurrence (playlist id, URI, absolute zero-based row).
	RemoveFromPlaylist(String, String, u32),
	/// Move one row to its final zero-based position.
	MovePlaylistRow(String, u32, u32),
	/// The tracks of an album or playlist, to add to the queue.
	QueueContext(Route),
	/// Whether `uri` is in Liked Songs.
	CheckLiked(String),
	/// Add `uri` to (true) or remove it from (false) Liked Songs.
	SetLiked(String, bool),
	Open(Route, u32),
	/// Speculative first-page read; duplicate hovers are coalesced and failures do not alter the UI.
	Prefetch(Route),
	Search(String),
	/// The Connect devices of the account (the device menu).
	Devices,
	/// What plays and on which device (polled while another device plays).
	Playback,
	/// Last played track/context; requested only if the session has no paused player state.
	LastPlayback,
	/// Persist a position checkpoint without any network request.
	RememberPlayback(super::PlaybackPosition),
	/// Move playback to the device `id`, playing or staying paused.
	Transfer {
		id:   String,
		play: bool,
	},
	/// A transport command for the device playing elsewhere.
	Control {
		device:  String,
		command: spotify::PlayerCommand,
	},
	Logout,
	/// Forget disk library data when the playback account signs out.
	ForgetSnapshot,
}

impl Request {
	/// The variant and what it is about, in at most 80 characters: what a trace logs.
	pub fn label(&self) -> String {
		format!("{self:?}").chars().take(80).collect()
	}
}

impl Response {
	/// The variant and what it is about, for traces (see [`crate::net::trace`]).
	pub fn label(&self) -> String {
		match self {
			Self::Connected { playlists, .. } => format!("Connected({})", playlists.len()),
			Self::Home(_) => "Home".into(),
			Self::HomeRecentTracks(tracks) => format!("HomeRecentTracks({})", tracks.len()),
			Self::HomeFeed { facet, feed } => format!("HomeFeed({facet:?}, {})", feed.sections.len()),
			Self::HomeSection { section, offset, .. } => format!("HomeSection({}@{offset})", section.uri),
			Self::HomeUnavailable { facet } => format!("HomeUnavailable({facet:?})"),
			Self::HomeSectionFailed { uri, message } => format!("HomeSectionFailed({uri}, {message})"),
			Self::Albums(albums) => format!("Albums({})", albums.len()),
			Self::Following(artists) => format!("Following({})", artists.len()),
			Self::Playlists(playlists) => format!("Playlists({})", playlists.len()),
			Self::Added { tracks, stats } => format!("Added({}, {stats:?})", tracks.len()),
			Self::LikedTracks(tracks) => format!("LikedTracks({})", tracks.len()),
			Self::Listing { route, offset, .. } => format!("Listing({route:?}@{offset})"),
			Self::Search { query, .. } => format!("Search({query})"),
			Self::ArtistPopular(id, _) => format!("ArtistPopular({id})"),
			Self::Error { message, .. } => format!("Error({message})"),
			_ => "other".into(),
		}
	}
}

#[derive(Debug)]
#[expect(clippy::large_enum_variant, reason = "sent boxed (`AppEvent::Library`), once per request")]
pub enum Response {
	Connected {
		me:        User,
		playlists: Vec<Playlist>,
	},
	/// A network failure, or a successful online account refresh (never a disk snapshot).
	Offline(bool),
	NeedsClientId,
	NeedsLogin,
	LoginFailed(String),
	/// The authorization URL, for when no browser opens by itself.
	LoginUrl(String),
	Home(Home),
	/// First usable native Home rows; keep every cached shelf until full enrichment arrives.
	HomeRecentTracks(Vec<Track>),
	HomeFeed {
		facet: Facet,
		feed:  HomeFeed,
	},
	HomeSection {
		section:     HomeSection,
		offset:      u32,
		next_offset: Option<u32>,
	},
	/// A failed personalised refresh leaves cached data visible; first load uses legacy shelves.
	HomeUnavailable {
		facet: Facet,
	},
	HomeSectionFailed {
		uri:     String,
		message: String,
	},
	Albums(Vec<Album>),
	Following(Vec<Artist>),
	Artist(Artist),
	ArtistPopular(String, Popularity),
	ArtistCard(String, spotify::ArtistOverview),
	Credits(String, Vec<spotify::Credit>),
	/// The cover color of the listing for `route`, `#rrggbb`.
	Accent(Route, String),
	/// Play counts by track id for the album listing of `route`.
	Plays(Route, HashMap<String, u64>),
	Playlists(Vec<Playlist>),
	Created(Playlist),
	Rootlist {
		account: String,
		entries: Vec<RootEntry>,
	},
	FolderCreated {
		id:   String,
		name: String,
	},
	FolderExpansion {
		account:  String,
		expanded: HashSet<String>,
	},
	PlaylistDetails(Playlist),
	ContextToPlaylist(Route, Vec<String>, String),
	Pins {
		account: String,
		uris:    Vec<String>,
	},
	Exclusions {
		account: String,
		uris:    Vec<String>,
	},
	Removed(String, String, u32),
	Moved(String, u32, u32),
	/// Track URIs to add to the queue, and what they came from.
	Queue(Vec<String>, String),
	/// A one-line confirmation for a toast.
	Done(String),
	/// `uri` is (or isn't) in Liked Songs.
	Liked(String, bool),
	/// Sorted, duplicate-free track membership in the user's owned playlists.
	/// `stats` is absent for a disk/local-edit answer and present after a refresh.
	Added {
		tracks: Arc<[TrackId]>,
		stats:  Option<RefreshStats>,
	},
	/// Complete, sorted Liked Songs membership from the playback Session's collection.
	LikedTracks(Arc<[TrackId]>),
	Listing {
		route:   Route,
		offset:  u32,
		listing: Listing,
	},
	Search {
		query:   String,
		results: SearchResults,
	},
	/// `route`: the listing the failed request was for, if any.
	Error {
		route:   Option<Route>,
		message: String,
	},
	/// The Connect devices Spotify lists, or why they couldn't be listed.
	Devices(Result<Vec<spotify::Device>, String>),
	/// What plays and where: `Ok(None)` when no device is active.
	Playback(Result<Option<spotify::Playback>, String>),
	/// Passive recently-played fallback, bound to the authenticated account.
	LastPlayback {
		account:  String,
		playback: Option<crate::engine::LastPlayback>,
	},
	/// Playback was moved to the device asked for.
	Transferred,
	/// A transfer or a transport command to another device failed (a sentence for the user).
	PlayerError(String),
}
