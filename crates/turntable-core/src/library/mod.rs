//! Library state and the session/Web API worker that loads it without blocking the UI.
//!
//! `model` and `state` describe what the views know; `pages`, `home` and `artist` load it.
//! `protocol` carries requests and responses between the application and `worker`.

pub mod added;
mod artist;
pub mod covers;
mod home;
mod model;
mod pages;
mod protocol;
mod restore;
mod session;
pub(crate) mod snapshot;
mod state;
mod worker;

pub use artist::Popularity;
pub use model::{Conn, Entry, Home, Kind, Listing, PAGE, Route, Sort, Tile, View};
pub use protocol::{Request, Response};
pub use restore::PlaybackPosition;
pub use state::Library;
pub use worker::Worker;
