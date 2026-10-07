//! Requests to the Web API worker and the responses it delivers to the application.

use std::{collections::HashMap, sync::Arc};

use super::{
	Home, Listing, Popularity, Route,
	added::{RefreshStats, TrackId},
};
use crate::{
	sources::external,
	spotify::{self, Album, Artist, Playlist, SearchResults, User},
};

#[derive(Debug, Clone)]
pub enum Request {
	/// (Re)open the client and load the profile and playlists.
	Connect,
	/// Run the browser login, then connect.
	Login,
	/// Load the home page.
	Home,
	/// Load the albums saved to the library.
	Albums,
	/// Load the artists the user follows.
	Following,
	/// Fetch an artist (for the now-playing view).
	Artist(String),
	/// An artist page's popular songs and overview (Spotify's web player).
	ArtistPopular(String),
	/// An artist page's About (open data: Wikidata, Wikipedia): slow, so it comes last.
	ArtistAbout(String),
	/// An artist's overview alone, for the now-playing card.
	ArtistCard(String),
	/// Who made a song (its URI).
	Credits(String),
	/// Follow (true) or unfollow an artist or playlist.
	SetFollowed(String, bool),
	/// Create a playlist and add these tracks to it.
	CreatePlaylist(String, Vec<String>),
	RenamePlaylist(String, spotify::PlaylistChanges),
	/// Append tracks to a playlist (id, name for the notice, URIs).
	AddToPlaylist(String, String, Vec<String>),
	RemoveFromPlaylist(String, String),
	/// The tracks of an album or playlist, to add to the queue.
	QueueContext(Route),
	/// Whether `uri` is in Liked Songs.
	CheckLiked(String),
	/// Add `uri` to (true) or remove it from (false) Liked Songs.
	SetLiked(String, bool),
	Open(Route, u32),
	Search(String),
	/// The Connect devices of the account (the device menu).
	Devices,
	/// What plays and on which device (polled while another device plays).
	Playback,
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
			Self::Albums(albums) => format!("Albums({})", albums.len()),
			Self::Following(artists) => format!("Following({})", artists.len()),
			Self::Playlists(playlists) => format!("Playlists({})", playlists.len()),
			Self::Added { tracks, stats } => format!("Added({}, {stats:?})", tracks.len()),
			Self::LikedTracks(tracks) => format!("LikedTracks({})", tracks.len()),
			Self::Listing { route, offset, .. } => format!("Listing({route:?}@{offset})"),
			Self::Search { query, .. } => format!("Search({query})"),
			Self::ArtistPopular(id, _) => format!("ArtistPopular({id})"),
			Self::ArtistAbout(id, _) => format!("ArtistAbout({id})"),
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
	Albums(Vec<Album>),
	Following(Vec<Artist>),
	Artist(Artist),
	ArtistPopular(String, Popularity),
	ArtistAbout(String, external::About),
	ArtistCard(String, spotify::ArtistOverview),
	Credits(String, Vec<spotify::Credit>),
	/// The cover color of the listing for `route`, `#rrggbb`.
	Accent(Route, String),
	/// Play counts by track id for the album listing of `route`.
	Plays(Route, HashMap<String, u64>),
	Playlists(Vec<Playlist>),
	Created(Playlist),
	Removed(String, String),
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
	/// Playback was moved to the device asked for.
	Transferred,
	/// A transfer or a transport command to another device failed (a sentence for the user).
	PlayerError(String),
}
