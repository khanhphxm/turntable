//! Playlist dialog state and optimistic edits to the current library listing.

use super::{Dialog, Field, State};
use crate::{
	library::{self, Route},
	spotify::PlaylistChanges,
};

impl State {
	/// A name for a new playlist, as Spotify picks one: "My Playlist #N".
	pub(super) fn new_playlist_name(&self) -> String {
		let me = self.library.me.as_ref().map(|u| u.id.as_str());
		let mine = self.library.playlists.iter().filter(|p| Some(p.owner.id.as_str()) == me).count();
		format!("My Playlist #{}", mine + 1)
	}
	pub(super) fn open_playlist_details(&mut self) {
		if let Route::Playlist(id) = &self.library.route
			&& let Some(listing) = self.library.listing.as_ref().filter(|l| l.owned)
		{
			self.name_input.clone_from(&listing.title);
			self.description_input = listing.description.clone().unwrap_or_default();
			self.public_input = listing.public;
			self.dialog = Some(Dialog::Rename {
				id:          id.clone(),
				name:        self.name_input.clone(),
				description: self.description_input.clone(),
				public:      listing.public,
			});
			self.focus = Some(Field::PlaylistName);
		}
	}

	pub(super) fn save_playlist_details(&mut self) -> Option<library::Request> {
		let name = self.name_input.trim();
		if name.is_empty() {
			return None;
		}
		let Some(Dialog::Rename { id, name: original_name, description, public }) = self.dialog.take() else { return None };
		let changes = PlaylistChanges {
			name:        (name != original_name).then(|| name.to_owned()),
			description: (self.description_input != description).then(|| self.description_input.clone()),
			public:      self.public_input.filter(|_| self.public_input != public),
		};
		self.focus = None;
		if changes.is_empty() {
			return None;
		}
		if matches!(&self.library.route, Route::Playlist(route_id) if *route_id == id)
			&& let Some(listing) = &mut self.library.listing
		{
			if let Some(name) = &changes.name {
				listing.title.clone_from(name);
			}
			if let Some(description) = &changes.description {
				listing.description = (!description.is_empty()).then(|| description.clone());
			}
			if let Some(public) = changes.public {
				listing.public = Some(public);
			}
		}
		if let Some(playlist) = self.library.playlists.iter_mut().find(|p| p.id == id) {
			if let Some(name) = &changes.name {
				playlist.name.clone_from(name);
			}
			if let Some(description) = &changes.description {
				playlist.description = (!description.is_empty()).then(|| description.clone());
			}
			if let Some(public) = changes.public {
				playlist.public = Some(public);
			}
		}
		Some(library::Request::RenamePlaylist(id, changes))
	}
}
