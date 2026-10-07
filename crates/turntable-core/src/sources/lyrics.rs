//! Fetches lyrics on a thread of their own: LRCLIB needs no account, so they
//! don't wait behind the Web API worker. The thread remembers what it found.

use std::{collections::HashMap, sync::mpsc, thread};

use crate::sources::external::{Client, Lyrics};

/// A song to find lyrics for.
pub struct Song {
	/// The track's URI, which its answer carries back.
	pub uri:         String,
	pub title:       String,
	/// The first artist.
	pub artist:      String,
	pub duration_ms: u64,
}

/// What a lookup found: `Ok(None)` when LRCLIB has no lyrics for the song.
pub struct Response {
	pub uri:    String,
	pub lyrics: Result<Option<Lyrics>, String>,
}

pub struct Fetcher {
	tx: mpsc::Sender<Song>,
}

impl Fetcher {
	/// Starts the thread; `deliver` runs on it.
	pub fn spawn(deliver: impl Fn(Response) + Send + 'static) -> Self {
		let (tx, rx) = mpsc::channel::<Song>();
		let _ = thread::Builder::new().name("lyrics".into()).stack_size(512 * 1024).spawn(move || {
			let client = Client::new();
			// Only answers are kept: a failed lookup runs again when asked again.
			let mut known: HashMap<String, Option<Lyrics>> = HashMap::new();
			while let Ok(mut song) = rx.recv() {
				// Skipping through tracks queues lookups nobody waits for any more.
				while let Ok(newer) = rx.try_recv() {
					song = newer;
				}
				let lyrics = match known.get(&song.uri) {
					Some(found) => Ok(found.clone()),
					None => client.lyrics(&song.title, &song.artist, song.duration_ms).inspect(|found| {
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
