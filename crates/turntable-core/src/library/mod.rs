//! Library state and the native-session worker that loads it without blocking the UI.
//!
//! `model` and `state` describe what the views know; `pages`, `home` and `artist` load it.
//! `protocol` carries requests and responses between the application and `worker`.
//!
//! Item actions never navigate: playlist headers resolve through rootlist/Playlist4,
//! context-to-playlist gathers every native page, and saves use the Session collection
//! service. No separate OAuth account is opened or refreshed. Removing a playlist also drops
//! its disk listing and owned-track index. Pins sort ahead of every unpinned row in all
//! library orders, with a four-item limit, and are stored per account under the config
//! directory's `library-pins`; Spotify pin synchronization is not supported.
//! Liked Songs is not implicitly pinned: it participates in the selected sort, or
//! uses the explicit pin URI `spotify:collection:tracks`. Taste exclusions are native
//! collection `ignoreinrecs` membership for playlist/track URIs; Spotify does not
//! provide the corresponding album/artist operation.
//!
//! Native rootlist folders preserve nested sibling order. `tree(filtered)` returns visible
//! depth-first `TreeEntry { entry, depth, parent }` rows; `folder_entries(id, filtered)`
//! lists a folder page's direct children, and `folder(id)` exposes its name/count/children.
//! Text searches reveal matching descendants without changing expansion preferences.
//! Each sibling list uses the selected sort; explicit nested playlist pins are projected
//! once at the root while retaining their native `parent`, never moving Spotify membership.
//! Sidebar expansion is local per account under `library-folders`; folder names, moves,
//! creation and deletion synchronize through Playlist4 only. Delete removes just the
//! folder markers, moving all children up one level. `StartRenameFolder` and `FolderCreated`
//! enter plain `Dialog::RenameFolder` state for an inline host UI; SaveRename/Enter commits,
//! CloseDialog cancels. Folder IDs use the host context URI `spotify:folder:<id>`.
//! Playlist images decode native uploaded IDs, sized pictures, mosaic URIs and image
//! format attributes. When Spotify omits a picture, Playlist4 rows and native album
//! metadata supply the first four distinct covers; four form a mosaic, fewer use the
//! first album cover. Rootlist enrichment is bounded-concurrent, reused by sidebar/Home
//! tiles, and persisted with playlist snapshots. Empty playlists keep the note placeholder.
//!
//! Home uses native recent history, followed artists and saved albums only. Deduplicated
//! recent songs are published before slower shelf enrichment; cached shelves stay visible
//! until the complete refresh arrives. No personalised Pathfinder feed is requested.

mod actions;
pub mod added;
mod artist;
pub mod covers;
mod folders;
mod home;
#[cfg(test)]
mod home_tests;
mod model;
mod pages;
mod pins;
mod protocol;
mod restore;
mod session;
pub(crate) mod snapshot;
mod state;
pub(crate) mod worker;

pub use artist::Popularity;
pub use folders::{Folder, RootEntry, TreeEntry};
pub use model::{Conn, Entry, Home, Kind, Listing, PAGE, PlaylistOccurrence, Route, RowTarget, Sort, Tile, View};
pub use protocol::{Request, Response};
pub use restore::{PlaybackPosition, QueuedTrack};
pub use state::Library;
pub use worker::Worker;
