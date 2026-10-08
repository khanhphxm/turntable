//! Right-click on a Your Library row, as Spotify's: what's offered depends on the row (your
//! playlist, someone else's, an album, an artist, a folder, Liked Songs), in Spotify's order.

use turntable_core::{Msg, Route, library::RootEntry};

use crate::{
	menu::{Action, Item},
	widgets::Icon,
};

/// What the menu needs to know about the row, gathered while rendering.
#[derive(Clone)]
pub struct Target {
	pub route:         Route,
	/// The row's Spotify URI (`spotify:collection:tracks` for Liked Songs, `spotify:folder:…`).
	pub uri:           String,
	pub owned:         bool,
	pub public:        Option<bool>,
	pub collaborative: bool,
	pub pinned:        bool,
	pub excluded:      bool,
	/// The folder the row sits in.
	pub parent:        Option<String>,
	/// Every folder, flattened (id, name), for "Move to folder".
	pub folders:       Vec<(String, String)>,
	/// The user's own playlists (id, name), for "Add to other playlist".
	pub playlists:     Vec<(String, String)>,
}

/// Flattens the rootlist's folders depth-first, indenting nested names like Spotify's submenu.
pub fn folders(entries: &[RootEntry], depth: usize, out: &mut Vec<(String, String)>) {
	for entry in entries {
		if let RootEntry::Folder(f) = entry {
			out.push((f.id.clone(), format!("{}{}", "    ".repeat(depth), f.name)));
			folders(&f.children, depth + 1, out);
		}
	}
}

/// A share link for the row.
fn link(route: &Route) -> Option<(String, &'static str)> {
	let (kind, id, label) = match route {
		Route::Playlist(id) => ("playlist", id, "Copy link to playlist"),
		Route::Album(id) => ("album", id, "Copy link to album"),
		Route::Artist(id) => ("artist", id, "Copy link to artist"),
		_ => return None,
	};
	Some((format!("https://open.spotify.com/{kind}/{id}"), label))
}

fn send(icon: Icon, label: &str, msg: Msg) -> Item {
	Item::row(icon, label, Action::Send(msg))
}

/// The menu for a row of Your Library.
pub fn items(t: &Target) -> Vec<Item> {
	let pin = |noun: &str| {
		send(Icon::MenuPin, &format!("{} {noun}", if t.pinned { "Unpin" } else { "Pin" }), Msg::SetPinned {
			uri:    t.uri.clone(),
			pinned: !t.pinned,
		})
	};
	let mut out = Vec::new();
	match &t.route {
		// Spotify offers only pinning for Liked Songs.
		Route::Liked => out.push(pin("playlist")),
		Route::Folder(id) => {
			out.push(send(Icon::MenuEdit, "Rename", Msg::StartRenameFolder(id.clone())));
			out.push(send(Icon::MenuDelete, "Delete", Msg::DeleteFolder(id.clone())));
			out.push(Item::Divider);
			out.push(send(Icon::MenuNewPlaylist, "Create playlist", Msg::CreatePlaylist));
			out.push(send(Icon::MenuPlus, "Create folder", Msg::CreateFolder));
			out.push(Item::Divider);
			out.push(move_to(t));
			out.push(pin("folder"));
		},
		Route::Playlist(id) => {
			out.push(send(Icon::MenuQueue, "Add to queue", Msg::QueueContext(t.route.clone())));
			if t.owned {
				out.push(send(Icon::MenuEdit, "Edit details", Msg::EditPlaylist(id.clone())));
				out.push(send(Icon::MenuDelete, "Delete", Msg::DeletePlaylist(id.clone())));
			} else {
				out.push(send(Icon::MenuRemove, "Remove from Your Library", Msg::SetSaved { uri: t.uri.clone(), saved: false }));
			}
			out.push(Item::Divider);
			out.push(send(Icon::MenuNewPlaylist, "Create playlist", Msg::CreatePlaylist));
			out.push(send(Icon::MenuPlus, "Create folder", Msg::CreateFolder));
			out.push(Item::Divider);
			if t.owned {
				let public = t.public == Some(true);
				out.push(send(
					Icon::MenuPublic,
					if public { "Make private" } else { "Make public" },
					Msg::SetPlaylistPublic(id.clone(), !public),
				));
				out.push(send(
					Icon::MenuCollab,
					if t.collaborative { "Stop collaborating" } else { "Invite collaborators" },
					Msg::SetCollaborative(id.clone(), !t.collaborative),
				));
			}
			out.push(send(
				Icon::MenuExclude,
				if t.excluded { "Include in your taste profile" } else { "Exclude from your taste profile" },
				Msg::ExcludeFromTaste { uri: t.uri.clone(), excluded: !t.excluded },
			));
			out.push(move_to(t));
			out.push(add_to(t));
			out.push(pin("playlist"));
		},
		Route::Album(_) => {
			out.push(send(Icon::MenuQueue, "Add to queue", Msg::QueueContext(t.route.clone())));
			out.push(send(Icon::MenuRemove, "Remove from Your Library", Msg::SetSaved { uri: t.uri.clone(), saved: false }));
			out.push(Item::Divider);
			out.push(add_to(t));
			out.push(pin("album"));
		},
		Route::Artist(_) => {
			out.push(send(Icon::MenuRemove, "Unfollow", Msg::SetSaved { uri: t.uri.clone(), saved: false }));
			out.push(Item::Divider);
			out.push(pin("artist"));
		},
		_ => {},
	}
	if let Some((url, label)) = link(&t.route) {
		out.push(Item::row(Icon::MenuShare, "Share", Action::Sub(vec![Item::row(Icon::MenuLink, label, Action::CopyText(url))])));
	}
	out
}

/// "Move to folder ▸": every folder but the row's own and its current parent.
fn move_to(t: &Target) -> Item {
	let own = match &t.route {
		Route::Folder(id) => Some(id.as_str()),
		_ => None,
	};
	let moving = |folder: Option<String>| Action::Send(Msg::MoveToFolder { uri: t.uri.clone(), folder });
	let mut sub = Vec::new();
	if t.parent.is_some() {
		sub.push(Item::plain("Remove from folder", moving(None)));
		sub.push(Item::Divider);
	}
	sub.extend(
		t.folders
			.iter()
			.filter(|(id, _)| Some(id) != t.parent.as_ref() && Some(id.as_str()) != own)
			.map(|(id, name)| Item::row(Icon::MenuFolder, name.clone(), moving(Some(id.clone())))),
	);
	if sub.is_empty() {
		sub.push(Item::plain("No folders yet", Action::Sub(Vec::new())).disabled());
	}
	Item::row(Icon::MenuFolder, "Move to folder", Action::Sub(sub))
}

/// "Add to other playlist ▸": a new playlist, then the user's own playlists.
fn add_to(t: &Target) -> Item {
	let route = t.route.clone();
	let adding = |target: Option<String>| Action::SendAll(vec![Msg::AddContextToPlaylist(route.clone()), Msg::AddToPlaylist(target)]);
	let mut sub = vec![Item::row(Icon::MenuPlus, "New playlist", adding(None)), Item::Divider];
	let own = match &t.route {
		Route::Playlist(id) => Some(id),
		_ => None,
	};
	sub.extend(t.playlists.iter().filter(|(id, _)| Some(id) != own).map(|(id, name)| Item::plain(name.clone(), adding(Some(id.clone())))));
	Item::row(Icon::MenuAddTo, "Add to other playlist", Action::Sub(sub))
}
