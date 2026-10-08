//! Keeps the OS's Now Playing (Control Center, SMTC, MPRIS) in step with the player, and routes
//! its buttons and media keys back to it.

use std::sync::Arc;

use gpui_kit::Entity;
use turntable_core::{Msg, engine::CoverSize};

use crate::{
	media::{Command, MediaControls, Metadata, Playback},
	shell::Shell,
};

/// What was last published, so unchanged state isn't sent again.
#[derive(Default, PartialEq)]
pub struct Published {
	uri:     String,
	art:     bool,
	playing: bool,
	/// Where the core last anchored the position: it moves on every seek, pause and resume.
	anchor:  u64,
}

pub struct NowPlaying {
	controls:  MediaControls,
	published: Published,
}

impl NowPlaying {
	/// Starts the session (after the window opened: Windows needs it) and forwards commands.
	pub fn start(shell: &Entity<Shell>, cx: &mut gpui_kit::App) -> Option<Self> {
		let (tx, rx) = smol::channel::unbounded::<Command>();
		let controls = MediaControls::new(move |c| {
			let _ = tx.try_send(c);
		})
		.ok()?;
		let weak = shell.downgrade();
		cx.spawn(async move |cx| {
			while let Ok(command) = rx.recv().await {
				let msg = match command {
					Command::Toggle => Msg::TogglePlay,
					Command::Next => Msg::Next,
					Command::Previous => Msg::Prev,
					Command::Seek(by) => Msg::SeekBy(by),
					Command::SetPosition(ms) => Msg::Seek(ms),
					Command::Play | Command::Pause => {
						let want = matches!(command, Command::Play);
						let Ok(playing) = weak.update(cx, |s, _| s.st().playing()) else { break };
						if playing == want {
							continue;
						}
						Msg::TogglePlay
					},
				};
				if weak.update(cx, |s, cx| s.send(msg, cx)).is_err() {
					break;
				}
			}
		})
		.detach();
		Some(Self { controls, published: Published::default() })
	}

	/// Publishes what changed since last time.
	pub fn sync(&mut self, shell: &Shell) {
		let st = shell.st();
		let item = &st.item;
		let url = item.cover(CoverSize::Default);
		// Asking also queues the cover's fetch, so the OS gets artwork once it arrives.
		let art = st.image(url).map(|c| Arc::<[u8]>::from(c.bytes.as_slice()));
		let position = st.position_ms();
		let now = Published { uri: item.uri.clone(), art: art.is_some(), playing: st.playing(), anchor: st.anchor.position_ms };
		let track_changed = now.uri != self.published.uri || now.art != self.published.art;
		if now == self.published {
			return;
		}
		if track_changed {
			let _ = self.controls.set_metadata(&Metadata {
				title:       item.name().to_owned(),
				artist:      item.creators(),
				album:       item.parent_name().to_owned(),
				duration_ms: st.duration_ms(),
				artwork:     art,
			});
		}
		let playback = if item.uri.is_empty() {
			Playback::Stopped
		} else if now.playing {
			Playback::Playing { position_ms: position }
		} else {
			Playback::Paused { position_ms: position }
		};
		let _ = self.controls.set_playback(playback);
		self.published = now;
	}
}
