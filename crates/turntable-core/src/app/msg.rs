//! Messages sent by the views and effects returned by application state transitions.

use super::{Field, Panel};
use crate::{
	engine::Command,
	library::{self, Route},
};

#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
	TogglePlay,
	Next,
	Prev,
	/// Seek to this absolute position in milliseconds (clamped to the track).
	Seek(u64),
	/// Relative seek in seconds.
	SeekBy(i64),
	/// Set volume to this percentage (clamped to 0..=100).
	Volume(u8),
	VolumeBy(i8),
	ToggleMute,
	ToggleShuffle,
	CycleRepeat,
	/// Open the device menu (listing the devices again), or close it.
	DevicesMenu,
	/// List the devices again (the menu's retry).
	RefreshDevices,
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
	/// Open the playlist picker for row `n` of the listing, or for what's playing.
	AddRowToPlaylist(usize),
	AddNowPlayingToPlaylist,
	/// Add the picked tracks to playlist `id`, or to a new playlist (`None`).
	AddToPlaylist(Option<String>),
	/// Add directly from search results without opening or consuming a dialog.
	AddToPlaylistDirect {
		playlist: String,
		uris:     Vec<String>,
	},
	RemoveRow(usize),
	/// Move a playlist row to final zero-based row `to` (drag-and-drop).
	MoveRow(usize, usize),
	RenamePlaylist,
	SaveRename,
	TogglePlaylistPublic,
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
	/// Select Spotify's personalised Home filter chip.
	HomeFacet(library::Facet),
	/// Open a Home section's Show all page (URI from `HomeSection`).
	HomeSectionMore(String),
	/// Return from a Show all page to the selected Home feed without refetching it.
	CloseHomeSection,
	EqEnabled(bool),
	/// Set band `0..10` to step `0..=EQ_STEPS` (bottom to top).
	EqBand(usize, u8),
	EqPreset(&'static str),
	/// Open the equalizer's preset menu, or close it.
	EqMenu,
	/// Open a link in the browser (the cover's link back to Spotify).
	OpenUrl(String),
	Navigate(Route),
	/// Warm a tile's first page on hover/visibility without navigating or changing UI state.
	Prefetch(Route),
	/// Remember a selected search result, moving its route to the front.
	RememberSearch(library::Tile),
	/// Remove the recent search selection with this route.
	ForgetSearch(Route),
	/// Empty this account's recent search selections.
	ClearRecentSearches,
	Select(usize),
	MoveSelection(i32),
	/// Play row `n` of the current listing.
	PlayRow(usize),
	/// A song card: open its album with the song selected.
	OpenTrack {
		album: String,
		uri:   String,
	},
	PlaySelected,
	/// Play the current listing from the top.
	PlayListing,
	/// Add the now-playing track to Liked Songs, or take it out.
	ToggleLike,
	/// Add a row's track URI to Liked Songs (never changes owned playlists).
	AddToLiked(String),
	/// Remove a row's track URI from Liked Songs (never changes owned playlists).
	RemoveFromLiked(String),
	/// Append row `n` of the current listing to the user queue.
	QueueRow(usize),
	/// Insert row `n` at the front of the user queue, without starting it now.
	QueuePlayNext(usize),
	/// Queue playable URIs in order (`next`: insert at front instead of append).
	QueueTracks {
		uris: Vec<String>,
		next: bool,
	},
	/// Remove absolute upcoming row `n`, immediately and without restarting playback.
	QueueRemove(usize),
	/// Move upcoming queued row `n` one place up (-1) or down (+1).
	QueueMove(usize, i8),
	/// Drag a user-queued row to its final absolute upcoming index; cannot cross into context.
	QueueMoveTo(usize, usize),
	/// Clear the songs queued by hand.
	QueueClear,
	/// Play upcoming song `n` now.
	QueueJump(usize),
	LoadMore,
	SearchSubmit(String),
	/// Sign in to Spotify (in the browser) and start the player.
	SignIn,
	/// Forget the sign-in on this computer and stop the player.
	SignOut,
	/// Save the library app's Client ID.
	SaveClientId(String),
	/// Connect the library's Spotify app (browser sign-in), or disconnect it.
	ConnectSpotify,
	DisconnectSpotify,
	/// Give a text field the caret (`None`: no field).
	Focus(Option<Field>),
	/// Typed or pasted text into the focused field.
	Type(String),
	Backspace,
	/// Replace UTF-16 code-unit range `from..to` of the focused field with `text`.
	Edit {
		from: usize,
		to:   usize,
		text: String,
	},
	/// Enter in the focused field.
	Submit,
	Quit,
	/// Change a setting on the settings page.
	Pref(crate::app::settings::Pref),
	/// Open a dropdown of the settings page, or close it when it is the open one.
	Dropdown(crate::app::system::Menu),
	/// Start, change or cancel the sleep timer.
	Sleep(crate::app::rules::SleepChoice),
	/// Take the player's name from the field of the settings page.
	SaveDeviceName,
	/// Restart the player, so a new name or cache size takes effect.
	RestartPlayer,
	/// Empty the player's playback cache.
	ClearCache,
	/// Open the player's log, the settings folder or the cache in the system's file manager.
	OpenFolder(crate::app::system::Folder),
}

/// What the player loop must do after an update.
#[derive(Debug, Clone)]
pub enum Effect {
	Player(Command),
	Library(library::Request),
	/// Start the player, signing in first when there is no sign-in.
	SignIn,
	/// Stop the player and delete the sign-in.
	SignOut,
	SaveClientId(String),
	/// Apply and persist new EQ settings.
	SetEq,
	/// Persist the settings.
	SaveSettings,
	/// Persist this account's recent search selections.
	SaveRecentSearches,
	OpenUrl(String),
	/// Search for the current query once typing pauses (the loop debounces).
	SearchSoon,
	/// Drop a pending debounced search (a submit made it redundant).
	CancelSearchSoon,
	/// Apply what the settings drive outside the state (the audio stage and device, the sources
	/// that may be asked) and save the settings.
	ApplyPrefs,
	/// Restart the player (a changed name or cache size only takes effect then).
	RestartPlayer,
	/// Empty the player's playback cache (the player stops while it is emptied).
	ClearCache,
}
