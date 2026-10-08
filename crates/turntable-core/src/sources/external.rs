//! The lyrics value shared by the application and source worker.
//! Spotify supplies both timestamped color lyrics and unsynced text.

/// A song's lyrics from Spotify.
#[derive(Debug, Clone, PartialEq)]
pub enum Lyrics {
	/// Each line with the millisecond it is sung at, in order. A blank line is a pause.
	Synced(Vec<(u64, String)>),
	/// Text only: lines apart by `\n`, stanzas by a blank line.
	Plain(String),
}
