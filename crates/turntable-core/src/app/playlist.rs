//! Playlist item actions and dialogs, independent of the page being viewed.

#[cfg(test)]
mod tests;

use super::{Dialog, Effect, Field, State};
use crate::{
	library::{self, Route},
	spotify::PlaylistChanges,
};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PendingDialog {
	EditPlaylist(String),
	/// `None` opens the picker; `Some(None)` creates a new playlist; `Some(Some(id))` adds directly.
	AddContext {
		route:  Route,
		target: Option<Option<String>>,
	},
}

impl State {
	/// A name for a new playlist, as Spotify picks one: "My Playlist #N".
	pub(super) fn new_playlist_name(&self) -> String {
		let me = self.library.me.as_ref().map(|u| u.id.as_str());
		let mine = self.library.playlists.iter().filter(|p| Some(p.owner.id.as_str()) == me).count();
		format!("My Playlist #{}", mine + 1)
	}
	pub(super) fn open_playlist_details(&mut self) {
		self.pending_library_dialog = None;
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
		if matches!(self.dialog, Some(Dialog::RenameFolder { .. })) {
			let Some(Dialog::RenameFolder { id, name: original }) = self.dialog.take() else { return None };
			self.focus = None;
			return (name != original).then(|| library::Request::RenameFolder(id, name.to_owned()));
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

	pub(super) fn edit_playlist(&mut self, id: String) -> Option<library::Request> {
		self.pending_library_dialog = None;
		if matches!(&self.library.route, Route::Playlist(current) if current == &id)
			&& self.library.listing.as_ref().is_some_and(|listing| listing.owned && !listing.loading && listing.public.is_some())
		{
			self.open_playlist_details();
			return None;
		}
		if let Some(playlist) = self.library.playlists.iter().find(|playlist| playlist.id == id) {
			if !self.library.me.as_ref().is_some_and(|me| me.id == playlist.owner.id) {
				return None;
			}
			if playlist.public.is_some() && !playlist.name.is_empty() {
				let (id, name, description, public) =
					(playlist.id.clone(), playlist.name.clone(), playlist.description.clone().unwrap_or_default(), playlist.public);
				self.show_playlist_details(id, name, description, public);
				return None;
			}
		}
		self.dialog = None;
		self.focus = None;
		self.pending_library_dialog = Some(PendingDialog::EditPlaylist(id.clone()));
		Some(library::Request::PlaylistDetails(id))
	}

	fn show_playlist_details(&mut self, id: String, name: String, description: String, public: Option<bool>) {
		self.name_input.clone_from(&name);
		self.description_input.clone_from(&description);
		self.public_input = public;
		self.dialog = Some(Dialog::Rename { id, name, description, public });
		self.focus = Some(Field::PlaylistName);
	}

	pub(super) fn add_context_to_playlist(&mut self, route: Route) -> Option<library::Request> {
		if !matches!(route, Route::Album(_) | Route::Playlist(_)) {
			return None;
		}
		self.dialog = None;
		self.focus = None;
		self.pending_library_dialog = Some(PendingDialog::AddContext { route: route.clone(), target: None });
		Some(library::Request::AddContextToPlaylist(route))
	}

	pub(super) fn start_rename_folder(&mut self, id: String) {
		let Some(folder) = self.library.folder(&id) else { return };
		let name = folder.name.clone();
		self.show_folder_rename(id, name);
	}

	fn show_folder_rename(&mut self, id: String, name: String) {
		self.pending_library_dialog = None;
		self.name_input.clone_from(&name);
		self.dialog = Some(Dialog::RenameFolder { id, name });
		self.focus = Some(Field::PlaylistName);
	}

	pub(crate) fn apply_library_dialog(&mut self, response: &mut library::Response) -> Option<library::Request> {
		match response {
			library::Response::FolderCreated { id, name } => self.show_folder_rename(id.clone(), name.clone()),
			library::Response::PlaylistDetails(playlist) if matches!(&self.pending_library_dialog, Some(PendingDialog::EditPlaylist(wanted)) if wanted == &playlist.id) =>
			{
				self.pending_library_dialog = None;
				if self.library.me.as_ref().is_some_and(|me| me.id == playlist.owner.id) {
					self.show_playlist_details(
						playlist.id.clone(),
						playlist.name.clone(),
						playlist.description.clone().unwrap_or_default(),
						playlist.public,
					);
				}
			},
			library::Response::ContextToPlaylist(route, uris, label)
				if self
					.pending_library_dialog
					.as_ref()
					.is_some_and(|pending| matches!(pending, PendingDialog::AddContext { route: wanted, .. } if wanted == route)) =>
			{
				let Some(PendingDialog::AddContext { target, .. }) = self.pending_library_dialog.take() else { return None };
				let uris = std::mem::take(uris);
				match target {
					Some(Some(id)) => return Some(self.playlist_add_request(id, uris)),
					Some(None) => return Some(library::Request::CreatePlaylist(self.new_playlist_name(), uris)),
					None => self.dialog = Some(Dialog::AddToPlaylist { uris, label: std::mem::take(label) }),
				}
			},
			library::Response::NeedsLogin => self.pending_library_dialog = None,
			_ => {},
		}
		None
	}

	pub(super) fn set_library_saved(&mut self, uri: String, saved: bool, effects: &mut Vec<Effect>) {
		if !self.library.set_saved(&uri, saved) {
			return;
		}
		if !saved && self.library.is_pinned(&uri) {
			self.set_library_pinned(uri.clone(), false, effects);
		}
		effects.push(Effect::Library(library::Request::SetFollowed(uri, saved)));
	}

	pub(super) fn set_library_pinned(&mut self, uri: String, pinned: bool, effects: &mut Vec<Effect>) {
		match self.library.set_pinned(&uri, pinned) {
			Ok(true) => effects.push(Effect::Library(library::Request::SetPinned { uri, pinned })),
			Ok(false) => {},
			Err(message) => self.show_toast(message.into()),
		}
	}

	pub(super) fn set_playlist_public(&mut self, id: String, public: bool) -> Option<library::Request> {
		if let Some(playlist) = self.library.playlists.iter_mut().find(|playlist| playlist.id == id) {
			if !self.library.me.as_ref().is_some_and(|me| me.id == playlist.owner.id) {
				return None;
			}
			playlist.public = Some(public);
		}
		if matches!(&self.library.route, Route::Playlist(current) if current == &id)
			&& let Some(listing) = &mut self.library.listing
		{
			listing.public = Some(public);
		}
		Some(library::Request::RenamePlaylist(id, PlaylistChanges { public: Some(public), ..PlaylistChanges::default() }))
	}

	pub(super) fn set_playlist_collaborative(&mut self, id: String, collaborative: bool) -> Option<library::Request> {
		if let Some(playlist) = self.library.playlists.iter_mut().find(|playlist| playlist.id == id) {
			if !self.library.me.as_ref().is_some_and(|me| me.id == playlist.owner.id) {
				return None;
			}
			playlist.collaborative = collaborative;
		}
		Some(library::Request::SetCollaborative(id, collaborative))
	}
}
