//! Spotify models, optional caller-supplied Web API tokens and anonymous web-player metadata.
//! The application authenticates through the native playback engine; there is no separate
//! Web API login, Client ID setup or credential-store access.

mod auth;
mod client;
mod error;
mod http;
mod model;
mod web_player;

pub use auth::{Token, TokenSource, open_browser};
pub use client::{Client, MAX_ARTIST_ALBUMS, PlayStart, PlayerCommand};
pub use error::{Error, Result};
pub use http::{Priority, priority, set_priority};
pub(crate) use http::{urldecode, urlencode};
pub use model::{
	Album, Artist, Copyright, Device, DeviceKind, Image, Page, PlayContext, Playback, Playlist, PlaylistChanges, RepeatMode, SearchResults,
	Track, User, id_of, pick_image,
};
pub use web_player::{ArtistOverview, City, Concert, Credit, Pick, WebPlayer, WebPlaylist};
