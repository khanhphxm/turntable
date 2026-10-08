//! UI-agnostic Spotify playback, audio processing, library and application state.
//!
//! [`Controller`] owns the workers and engine. The application host renders [`State`],
//! sends [`Msg`], and schedules [`Controller::pump`] using wakes and deadlines.
//! Image decoding, window/input integration, signals and allocator policy belong to the host.

pub mod app;
pub mod audio;
pub mod engine;
pub mod library;
pub mod lyrics;
pub mod net;
pub mod spotify;

pub use app::{
	Cover, Dialog, Field, LyricsState, Msg, Panel, State,
	config::Paths,
	player::{Controller, Options},
};
pub use library::Route;

/// Whether this configuration has a saved playback sign-in.
pub fn has_sign_in(paths: &Paths) -> bool {
	engine::has_sign_in(&paths.config)
}

/// Deletes the saved playback sign-in and account-scoped library snapshots/indexes.
/// Does not delete preferences, covers or the audio cache.
/// A running host should dispatch [`Msg::SignOut`] instead, to stop playback too.
pub fn forget_sign_in(paths: &Paths) -> std::io::Result<()> {
	let credentials = engine::forget_sign_in(&paths.config);
	let snapshot = library::snapshot::clear(&paths.cache);
	let membership = library::added::clear(&paths.cache);
	credentials.and(snapshot).and(membership)
}
