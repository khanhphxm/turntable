//! Native now-playing metadata and OS media commands.
//!
//! Create and retain one [`MediaControls`] on the GPUI main thread, after application
//! startup. macOS uses that thread's existing Cocoa run loop: there is no worker,
//! timer, event tap, Accessibility permission, or media-key interception. Publishing
//! [`Playback::Playing`] claims the native now-playing session; pausing or stopping
//! updates the OS's session arbitration. Dropping the object unregisters callbacks
//! and clears its session. Windows uses a manually controlled SMTC session, not a
//! second audio player; Linux exports `org.mpris.MediaPlayer2.turntable` on the
//! session bus with one worker blocked on D-Bus and a wake socket (no polling).
//!
//! The command callback may run on an OS thread. Forward commands to the player's
//! existing channel; do not update GPUI views or call back into this object there.
//! Report actual player state with `set_playback`, not speculative button state.
//! Position is in milliseconds; [`Command::Seek`] is a signed relative offset and
//! [`Command::SetPosition`] is an absolute position. Encoded artwork bytes can be
//! shared with the cover-image cache without copying them.
//!
//! Publish metadata on track/artwork changes, and playback on play/pause/stop/seek
//! and authoritative position corrections. macOS and MPRIS extrapolate elapsed
//! time while playing, so they require no per-frame/per-second updates. Windows
//! recommends a timeline correction about every five seconds during playback.

use std::{marker::PhantomData, rc::Rc, sync::Arc};

#[cfg(target_os = "linux")]
#[path = "media/linux.rs"]
mod linux;
#[cfg(target_os = "macos")]
#[path = "media/macos.rs"]
mod macos;
#[cfg(target_os = "windows")]
#[path = "media/windows.rs"]
mod windows;

#[cfg(target_os = "linux")]
use linux::Backend;
#[cfg(target_os = "macos")]
use macos::Backend;
#[cfg(target_os = "windows")]
use windows::Backend;

/// Commands requested by native media buttons or another MPRIS client.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
	Play,
	Pause,
	#[cfg_attr(windows, expect(dead_code, reason = "Windows media buttons emit separate Play and Pause commands"))]
	Toggle,
	Next,
	Previous,
	/// A signed offset from the current position, in milliseconds (MPRIS Seek).
	#[cfg_attr(
		not(target_os = "linux"),
		expect(dead_code, reason = "MPRIS emits relative seeks; native macOS and Windows scrubbers emit SetPosition")
	)]
	Seek(i64),
	/// An absolute position in milliseconds (native scrubbers / MPRIS SetPosition).
	SetPosition(u64),
}

/// Metadata for the current track. Artwork is an encoded PNG, JPEG, or other
/// image format understood by the platform, not a pixel buffer or a remote URL.
#[derive(Clone, Debug, Default)]
pub struct Metadata {
	pub title:       String,
	pub artist:      String,
	pub album:       String,
	pub duration_ms: u64,
	pub artwork:     Option<Arc<[u8]>>,
}

/// Authoritative playback state; a playing position advances at a rate of 1.0.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Playback {
	Playing {
		position_ms: u64,
	},
	Paused {
		position_ms: u64,
	},
	#[default]
	Stopped,
}

impl Playback {
	pub(super) fn position_ms(self) -> u64 {
		match self {
			Self::Playing { position_ms } | Self::Paused { position_ms } => position_ms,
			Self::Stopped => 0,
		}
	}
}

/// A main-thread-owned native media session. Retain it for the player's lifetime.
/// Creating a second session in the same process is not supported.
pub struct MediaControls {
	backend:      Backend,
	// Keep construction, updates, and destruction on the same GUI thread, even on
	// platforms whose underlying handles happen to implement Send.
	_main_thread: PhantomData<Rc<()>>,
}

impl MediaControls {
	/// macOS requires the main thread and an active Cocoa run loop (provided by
	/// GPUI). Linux requires a session D-Bus. Windows requires Windows 10 or newer
	/// and an already activated GPUI window on the calling thread.
	pub fn new(on_command: impl Fn(Command) + Send + Sync + 'static) -> Result<Self, String> {
		Ok(Self { backend: Backend::new(Arc::new(on_command))?, _main_thread: PhantomData })
	}

	/// Replace the current track's metadata, including clearing old artwork when
	/// `artwork` is `None`. Playback position/state is preserved.
	pub fn set_metadata(&mut self, metadata: &Metadata) -> Result<(), String> {
		self.backend.set_metadata(metadata)
	}

	/// Publish actual playback state/position, without allocating a Rust timer or
	/// starting an audio stream. `Stopped` clears the current native track.
	pub fn set_playback(&mut self, playback: Playback) -> Result<(), String> {
		self.backend.set_playback(playback)
	}
}
