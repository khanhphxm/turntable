//! Application state, user messages and the event loop that carries their effects out.
//!
//! State transitions are pure; `player::Controller` owns workers and the playback engine.

pub mod config;
pub mod connect;
mod input;
mod msg;
mod playback;
pub mod player;
mod playlist;
mod prefs;
pub mod queue;
mod recent_searches;
pub mod rules;
pub mod settings;
mod state;
pub mod system;
mod update;

pub use msg::{Effect, Msg};
pub use state::{Anchor, Cover, Dialog, EQ_STEPS, Field, LyricsState, Panel, State, clock, eq_db_step, eq_step_db};

#[cfg(test)]
mod tests;
