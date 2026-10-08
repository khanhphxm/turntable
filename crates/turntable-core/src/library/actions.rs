//! Item-specific library state changes, independent of the page being viewed.

use super::{Library, pins};
use crate::spotify::{Album, Artist, Playlist, id_of};

pub(super) fn saved_uri(uri: &str) -> bool {
	["spotify:album:", "spotify:playlist:", "spotify:artist:"]
		.iter()
		.any(|prefix| uri.strip_prefix(prefix).is_some_and(|id| !id.is_empty()))
}

impl Library {
	/// Whether an explicitly pinned entry should show a pin glyph.
	pub fn is_pinned(&self, uri: &str) -> bool {
		self.pinned.iter().any(|known| known == uri)
	}

	/// Native taste-profile exclusion membership (Spotify's `ignoreinrecs` set).
	pub fn is_excluded(&self, uri: &str) -> bool {
		self.excluded.contains(uri)
	}

	pub(crate) fn set_excluded(&mut self, uri: &str, excluded: bool) -> bool {
		if !uri.starts_with("spotify:playlist:") && !uri.starts_with("spotify:track:") {
			return false;
		}
		if excluded {
			self.excluded.insert(uri.to_owned());
		} else {
			self.excluded.remove(uri);
		}
		true
	}

	pub(crate) fn set_pinned(&mut self, uri: &str, pinned: bool) -> Result<bool, &'static str> {
		pins::change(&mut self.pinned, uri, pinned)
	}

	/// Use real cached metadata for additions; uncached items arrive in the worker's
	/// complete library response. Removals are always visible in the same update.
	pub(crate) fn set_saved(&mut self, uri: &str, saved: bool) -> bool {
		if !saved_uri(uri) {
			return false;
		}
		if uri.starts_with("spotify:playlist:") {
			if !saved {
				self.playlists.retain(|playlist| playlist.uri != uri);
				super::folders::remove_playlist(&mut self.rootlist, id_of(uri));
			} else if !self.playlists.iter().any(|playlist| playlist.uri == uri)
				&& let Some(playlist) = self.cached_playlist(uri)
			{
				self.playlists.insert(0, playlist);
			}
		} else if uri.starts_with("spotify:album:") {
			if !saved {
				self.albums.retain(|album| album.uri != uri);
			} else if !self.albums.iter().any(|album| album.uri == uri)
				&& let Some(album) = self.cached_album(uri)
			{
				self.albums.insert(0, album);
			}
		} else if uri.starts_with("spotify:artist:") {
			if !saved {
				self.followed.retain(|artist| artist.uri != uri);
			} else if !self.followed.iter().any(|artist| artist.uri == uri)
				&& let Some(artist) = self.cached_artist(uri)
			{
				self.followed.insert(0, artist);
			}
		}
		if let Some(listing) = &mut self.listing
			&& listing.context_uri.as_deref() == Some(uri)
		{
			listing.followed = Some(saved);
		}
		true
	}

	fn cached_playlist(&self, uri: &str) -> Option<Playlist> {
		self.search.as_ref()?.playlists.items.iter().find(|playlist| playlist.uri == uri).cloned()
	}

	fn cached_album(&self, uri: &str) -> Option<Album> {
		self.search
			.as_ref()
			.and_then(|search| search.albums.items.iter().find(|album| album.uri == uri))
			.or_else(|| self.listing.as_ref()?.album.as_ref().filter(|album| album.uri == uri))
			.or_else(|| self.rows().iter().filter_map(|track| track.album.as_ref()).find(|album| album.uri == uri))
			.cloned()
	}

	fn cached_artist(&self, uri: &str) -> Option<Artist> {
		self.artists
			.get(id_of(uri))
			.filter(|artist| artist.uri == uri)
			.or_else(|| self.search.as_ref()?.artists.items.iter().find(|artist| artist.uri == uri))
			.or_else(|| self.listing.as_ref()?.artist.as_ref().filter(|artist| artist.uri == uri))
			.or_else(|| self.rows().iter().flat_map(|track| &track.artists).find(|artist| artist.uri == uri))
			.cloned()
	}
}
