//! Web API library state and the worker that loads it without blocking the UI.
//!
//! `model` and `state` describe what the views know; `pages`, `home` and `artist` load it.
//! `protocol` carries requests and responses between the application and `worker`.

mod artist;
pub(crate) mod covers;
mod home;
mod model;
mod pages;
mod protocol;
pub(crate) mod snapshot;
mod state;
mod worker;

pub(crate) use artist::Popularity;
pub(crate) use model::{Conn, Entry, Home, Kind, Listing, PAGE, Route, Sort, Tile, View};
pub(crate) use protocol::{Request, Response};
pub(crate) use state::Library;
pub(crate) use worker::Worker;
