//! Application state, user messages and the event loop that carries their effects out.
//!
//! State transitions are pure; `player` owns the Tern session, workers and playback engine.

pub(crate) mod config;
pub(crate) mod connect;
mod input;
mod msg;
mod playback;
pub(crate) mod player;
mod playlist;
mod prefs;
pub(crate) mod queue;
pub(crate) mod rules;
pub(crate) mod settings;
mod state;
pub(crate) mod system;
mod update;

pub(crate) use msg::{Effect, Msg, SEEK_CELLS, VOLUME_CELLS};
pub(crate) use state::{Anchor, Dialog, EQ_STEPS, Field, Layout, LyricsState, Panel, State, clock, eq_db_step, eq_step_db};

#[cfg(test)]
mod tests;
