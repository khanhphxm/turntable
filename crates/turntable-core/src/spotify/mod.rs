//! Spotify Web API client for library, playlists and search. Playback is the engine's job.
//!
//! The Web API is read with the user's own Spotify app (Development Mode quotas are per app;
//! the player's own client is shared by everyone and runs out). First-time setup:
//! 1. <https://developer.spotify.com/dashboard> -> create an app (Web API), redirect URI
//!    `http://127.0.0.1:8898/callback` exactly (loopback IP literal).
//! 2. Paste its Client ID; [`account::login`] opens the browser and stores the token in the OS
//!    credential store (macOS Keychain, Linux Secret Service), or `token.json` (0600).

pub mod account;
mod auth;
mod client;
mod error;
mod http;
mod model;
mod pkce;
mod token_store;
mod web_player;

pub use account::{REDIRECT_URI, login, logout};
pub use auth::{Token, TokenSource, open_browser};
pub use client::{Client, MAX_ARTIST_ALBUMS, PlayStart, PlayerCommand};
pub use error::{Error, Result};
pub(crate) use http::urlencode;
pub use http::{Priority, priority, set_priority};
pub use model::{
	Album, Artist, Copyright, Device, DeviceKind, Image, Page, PlayContext, Playback, Playlist, PlaylistChanges, RepeatMode, SearchResults,
	Track, User, id_of, pick_image,
};
pub use web_player::{ArtistOverview, Credit, Pick, WebPlayer, WebPlaylist};
