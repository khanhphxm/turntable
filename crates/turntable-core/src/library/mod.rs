//! Library state and the session/Web API worker that loads it without blocking the UI.
//!
//! `model` and `state` describe what the views know; `pages`, `home` and `artist` load it.
//! `protocol` carries requests and responses between the application and `worker`.
//!
//! Item actions never navigate: playlist headers resolve through rootlist/Playlist4,
//! context-to-playlist gathers every native page, and saves use the Session collection
//! service (the Web API is only a no-session fallback). Removing a playlist also drops
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
//! Home exposes `HomeFeed` in Spotify's order (shortcuts, shelves and large feed cards).
//! Facets are account-scoped disk snapshots, painted before background session refresh.
//! Native fallback publishes its deduplicated recent songs as soon as history resolves,
//! before playlist permissions and shelf metadata. Partial updates retain cached shelves
//! in memory and on disk; the complete Home answer replaces them after enrichment.
//! `HomeSectionMore` opens Show all on the existing Home route; `CloseHomeSection` returns
//! to the cached feed and `LoadMore` uses Spotify's raw section cursor. Hosts can keep the
//! URI in their own page history. If Pathfinder rejects the attached Session's client,
//! `State::home_feed()` remains absent and hosts render the native `library.home` shelves;
//! anonymous recommendations are never presented as personalisation. Spoken-word pages
//! and unplayable DJ/promotions have no supported route and are omitted.

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
mod worker;

pub use artist::Popularity;
pub use folders::{Folder, RootEntry, TreeEntry};
pub use model::{Conn, Entry, Facet, Home, HomeFeed, HomeItem, HomeSection, HomeStyle, Kind, Listing, PAGE, Route, Sort, Tile, View};
pub use protocol::{Request, Response};
pub use restore::PlaybackPosition;
pub use state::Library;
pub use worker::Worker;
