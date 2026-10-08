//! Messages sent by the views and effects returned by application state transitions.

use super::{Field, Panel};
use crate::{
	engine::Command,
	library::{self, Route},
};

#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
	TogglePlay,
	/// Resume playback; unlike TogglePlay, repeated media-key Play commands never pause.
	Play,
	/// Pause playback, even when the last decoder event has not reached the bar yet.
	Pause,
	Next,
	Prev,
	/// Seek to this absolute position in milliseconds (clamped to the track).
	Seek(u64),
	/// Relative seek in seconds.
	SeekBy(i64),
	/// Set volume to this percentage (clamped to 0..=100).
	Volume(u8),
	ToggleMute,
	ToggleShuffle,
	CycleRepeat,
	/// Open the device menu (listing the devices again), or close it.
	DevicesMenu,
	/// Move playback to the device with this id (empty: this player, not listed yet).
	PickDevice(String),
	/// Open a side panel, or close it when it is the open one.
	TogglePanel(Panel),
	/// Look the now-playing song's lyrics up again after a failure.
	RetryLyrics,
	/// Play a playlist, album or artist from its start (a tile's play button).
	PlayContext(String),
	/// Follow or unfollow the current page's artist or playlist.
	ToggleFollow,
	/// Follow or unfollow an artist (id, name) from a card.
	FollowArtist(String, String),
	CreatePlaylist,
	/// Create "New Folder" at the top level and enter inline rename state.
	CreateFolder,
	/// Enter plain inline rename state for an existing native folder.
	StartRenameFolder(String),
	RenameFolder(String, String),
	/// Delete markers, preserving contents one level up.
	DeleteFolder(String),
	MoveToFolder {
		uri:    String,
		folder: Option<String>,
	},
	ToggleFolder(String),
	/// Edit an owned playlist's details without changing the current route.
	EditPlaylist(String),
	/// Remove an owned or followed playlist from Your Library (Spotify's Delete).
	DeletePlaylist(String),
	SetPlaylistPublic(String, bool),
	/// Save/follow or remove an album, playlist or artist without opening it.
	SetSaved {
		uri:   String,
		saved: bool,
	},
	/// Pin at most four library entries; pins are persisted separately for each account.
	SetPinned {
		uri:    String,
		pinned: bool,
	},
	/// Resolve every native album/playlist page before opening the playlist picker.
	AddContextToPlaylist(Route),
	/// Only the playlist's owner may change collaboration.
	SetCollaborative(String, bool),
	/// Spotify taste-profile exclusions apply to playlists and tracks, not albums/artists.
	ExcludeFromTaste {
		uri:      String,
		excluded: bool,
	},
	/// Open the playlist picker for the rendered track/occurrence, if still present.
	AddRowToPlaylist(library::RowTarget),
	AddNowPlayingToPlaylist,
	/// Add the picked tracks to playlist `id`, or to a new playlist (`None`).
	AddToPlaylist(Option<String>),
	/// Add directly from search results without opening or consuming a dialog.
	AddToPlaylistDirect {
		playlist: String,
		uris:     Vec<String>,
	},
	RemoveRow(library::RowTarget),
	/// Move a playlist row to final zero-based row `to` (drag-and-drop).
	MoveRow(usize, usize),
	RenamePlaylist,
	SaveRename,
	CloseDialog,
	/// Add an album's or playlist's songs to the queue.
	QueueContext(Route),
	/// Filter an artist's discography (0: everything).
	Discography(usize),
	/// Collapse the library sidebar to covers only, or expand it.
	ToggleSidebar,
	/// Show or hide a kind of Your Library's rows (none shown: all kinds).
	LibraryKind(library::Kind),
	/// Order Your Library's rows by `Sort` (and close the menu).
	LibrarySort(library::Sort),
	/// Lay Your Library's rows out as `View`.
	LibraryView(library::View),
	/// Open Your Library's sort and view menu, or close it.
	LibraryMenu,
	/// Empty Your Library's filter.
	ClearLibraryFilter,
	/// Show a home shelf's every item instead of one row, or one row again.
	ToggleShelf(&'static str),
	EqEnabled(bool),
	/// Set band `0..10` to step `0..=EQ_STEPS` (bottom to top, 0.5 dB apart).
	EqBand(usize, u8),
	/// Set band `0..10` to a typed dB value, clamped to ±12 and rounded to 0.5 dB.
	EqSet(usize, f32),
	EqPreset(&'static str),
	/// Open a link in the browser (the cover's link back to Spotify).
	OpenUrl(String),
	Navigate(Route),
	/// Warm a tile's first page on hover/visibility without navigating or changing UI state.
	Prefetch(Route),
	/// Remember a selected search result, moving its route to the front.
	RememberSearch(library::Tile),
	/// Remove the recent search selection with this route.
	ForgetSearch(Route),
	Select(library::RowTarget),
	/// Play the rendered track/occurrence, resolving its identity at dispatch.
	PlayRow(library::RowTarget),
	/// A song card: open its album with the song selected.
	OpenTrack {
		album: String,
		uri:   String,
	},
	/// Play the current listing from the top.
	PlayListing,
	/// Add the now-playing track to Liked Songs, or take it out.
	ToggleLike,
	/// Add a row's track URI to Liked Songs (never changes owned playlists).
	AddToLiked(String),
	/// Append the rendered track/occurrence to the user queue, if still present.
	QueueRow(library::RowTarget),
	/// Insert the rendered track/occurrence at the front without starting it now.
	QueuePlayNext(library::RowTarget),
	/// Remove absolute upcoming row `n`, immediately and without restarting playback.
	QueueRemove(usize),
	/// Drag any upcoming occurrence to its final absolute upcoming index, across sections.
	QueueMoveTo(usize, usize),
	/// Clear the songs queued by hand.
	QueueClear,
	/// Play upcoming song `n` now.
	QueueJump(usize),
	LoadMore,
	SearchSubmit(String),
	/// Sign in to Spotify (in the browser) and start the player.
	SignIn,
	/// Cancel an unfinished browser sign-in and allow a fresh attempt.
	CancelSignIn,
	/// Forget the sign-in on this computer and stop the player.
	SignOut,
	/// Give a text field the caret (`None`: no field).
	Focus(Option<Field>),
	/// Replace a host text field without depending on core focus or the current route.
	SetText(Field, String),
	/// Replace UTF-16 code-unit range `from..to` of the focused field with `text`.
	Edit {
		from: usize,
		to:   usize,
		text: String,
	},
	/// Enter in the focused field.
	Submit,
	/// Change a setting on the settings page.
	Pref(crate::app::settings::Pref),
	/// Start, change or cancel the sleep timer.
	Sleep(crate::app::rules::SleepChoice),
}

/// What the player loop must do after an update.
#[derive(Debug, Clone)]
pub enum Effect {
	Player(Command),
	Library(library::Request),
	/// Start the player, signing in first when there is no sign-in.
	SignIn,
	/// Revoke the unfinished sign-in's authorization and stop its listener.
	CancelSignIn,
	/// Stop the player and delete the sign-in.
	SignOut,
	/// Apply EQ immediately and schedule a debounced off-thread preference save.
	SetEq,
	/// Schedule a debounced off-thread settings save.
	SaveSettings,
	/// Schedule an ordered off-thread save of this account's recent search selections.
	SaveRecentSearches,
	OpenUrl(String),
	/// Search for the current query once typing pauses (the loop debounces).
	SearchSoon,
	/// Drop a pending debounced search (a submit made it redundant).
	CancelSearchSoon,
	/// Apply what the settings drive outside the state (the audio stage and device, the sources
	/// that may be asked) and save the settings.
	ApplyPrefs,
	/// Replace HTTP pools and restart the native session, including an unfinished sign-in.
	ReconfigureNetwork,
}
