//! Spotify lyrics fetched off-thread, with a bounded cache of successful and missing answers.

use std::{sync::mpsc, thread, time::Duration};

use crate::{engine::SessionReader, net::cache::Cache};

mod spotify;

/// A song's lyrics from Spotify.
#[derive(Debug, Clone, PartialEq)]
pub enum Lyrics {
	/// Each line with the millisecond it is sung at, in order. A blank line is a pause.
	Synced(Vec<(u64, String)>),
	/// Text only: lines apart by `\n`, stanzas by a blank line.
	Plain(String),
}

/// A song to find lyrics for.
pub struct Song {
	/// The track's URI, which its answer carries back.
	pub uri: String,
}

/// What a lookup found: `Ok(None)` when Spotify has no lyrics for the song.
pub struct Response {
	pub uri:    String,
	pub lyrics: Result<Option<Lyrics>, String>,
}

pub struct Fetcher {
	tx: mpsc::Sender<Song>,
}

impl Fetcher {
	/// Starts the thread; `deliver` runs on it.
	pub fn spawn(session: SessionReader, deliver: impl Fn(Response) + Send + 'static) -> Self {
		let (tx, rx) = mpsc::channel::<Song>();
		let _ = thread::Builder::new().name("lyrics".into()).stack_size(512 * 1024).spawn(move || {
			// Only answers are kept: a failed lookup runs again when asked again.
			let mut known = Cache::new(128, Duration::from_secs(3600));
			while let Ok(mut song) = rx.recv() {
				// Skipping through tracks queues lookups nobody waits for any more.
				while let Ok(newer) = rx.try_recv() {
					song = newer;
				}
				let lyrics = match known.get(&song.uri) {
					Some(found) => Ok(found),
					None => spotify::lyrics(&session, &song.uri).inspect(|found| {
						known.insert(song.uri.clone(), found.clone());
					}),
				};
				deliver(Response { uri: song.uri, lyrics });
			}
		});
		Self { tx }
	}

	pub fn request(&self, song: Song) {
		let _ = self.tx.send(song);
	}
}
