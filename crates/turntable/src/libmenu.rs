//! Right-click on a Your Library row, as Spotify's: what's offered depends on the row (your
//! playlist, someone else's, an album, an artist, a folder, Liked Songs).

use gpui_kit::{
	App, WeakEntity, Window,
	component::menu::{PopupMenu, PopupMenuItem},
};
use turntable_core::{Msg, Route, library::RootEntry};

use crate::shell::Shell;

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

/// A share link for the row, as Spotify's "Copy link to …".
fn link(route: &Route) -> Option<String> {
	let (kind, id) = match route {
		Route::Playlist(id) => ("playlist", id),
		Route::Album(id) => ("album", id),
		Route::Artist(id) => ("artist", id),
		_ => return None,
	};
	Some(format!("https://open.spotify.com/{kind}/{id}"))
}

pub fn build(
	menu: PopupMenu,
	shell: &WeakEntity<Shell>,
	t: &Target,
	window: &mut Window,
	cx: &mut gpui_kit::Context<'_, PopupMenu>,
) -> PopupMenu {
	let send = |label: &str, msg: Msg| {
		let shell = shell.clone();
		PopupMenuItem::new(label.to_owned()).on_click(move |_, _, cx: &mut App| {
			let _ = shell.update(cx, |this, cx| this.send(msg.clone(), cx));
		})
	};
	let mut menu = menu.min_w(gpui_kit::px(240.));
	let pin = |label: &str| send(label, Msg::SetPinned { uri: t.uri.clone(), pinned: !t.pinned });
	match &t.route {
		Route::Folder(id) => {
			menu = menu
				.item(send("Rename", Msg::StartRenameFolder(id.clone())))
				.item(send("Delete", Msg::DeleteFolder(id.clone())))
				.item(PopupMenuItem::separator())
				.item(send("Create playlist", Msg::CreatePlaylist))
				.item(send("Create folder", Msg::CreateFolder))
				.item(PopupMenuItem::separator())
				.item(pin(if t.pinned { "Unpin folder" } else { "Pin folder" }));
		},
		Route::Liked => {
			menu = menu.item(pin(if t.pinned { "Unpin playlist" } else { "Pin playlist" }));
		},
		Route::Playlist(id) => {
			menu = menu.item(send("Add to queue", Msg::QueueContext(t.route.clone())));
			if t.owned {
				menu = menu
					.item(PopupMenuItem::separator())
					.item(send("Edit details", Msg::EditPlaylist(id.clone())))
					.item(send("Delete", Msg::DeletePlaylist(id.clone())));
			} else {
				menu = menu.item(send("Remove from Your Library", Msg::SetSaved { uri: t.uri.clone(), saved: false }));
			}
			menu = menu
				.item(PopupMenuItem::separator())
				.item(send("Create playlist", Msg::CreatePlaylist))
				.item(send("Create folder", Msg::CreateFolder));
			if t.owned {
				let public = t.public == Some(true);
				menu = menu
					.item(send(if public { "Make private" } else { "Make public" }, Msg::SetPlaylistPublic(id.clone(), !public)))
					.item(send(
						if t.collaborative { "Stop collaborating" } else { "Invite collaborators" },
						Msg::SetCollaborative(id.clone(), !t.collaborative),
					));
			}
			menu = menu.item(send(
				if t.excluded { "Include in your taste profile" } else { "Exclude from your taste profile" },
				Msg::ExcludeFromTaste { uri: t.uri.clone(), excluded: !t.excluded },
			));
		},
		Route::Album(_) | Route::Artist(_) => {
			menu = menu.item(send("Add to queue", Msg::QueueContext(t.route.clone())));
			if matches!(t.route, Route::Artist(_)) {
				menu = menu.item(send("Unfollow", Msg::SetSaved { uri: t.uri.clone(), saved: false }));
			} else {
				menu = menu.item(send("Remove from Your Library", Msg::SetSaved { uri: t.uri.clone(), saved: false }));
			}
		},
		_ => {},
	}
	menu = submenus(menu, shell, t, window, cx);
	if !matches!(t.route, Route::Folder(_) | Route::Liked) {
		menu = menu.item(pin(match t.route {
			Route::Playlist(_) if t.pinned => "Unpin playlist",
			Route::Playlist(_) => "Pin playlist",
			Route::Album(_) if t.pinned => "Unpin album",
			Route::Album(_) => "Pin album",
			_ if t.pinned => "Unpin artist",
			_ => "Pin artist",
		}));
	}
	if let Some(url) = link(&t.route) {
		let label = match t.route {
			Route::Album(_) => "Copy link to album",
			Route::Artist(_) => "Copy link to artist",
			_ => "Copy link to playlist",
		};
		menu = menu.item(PopupMenuItem::separator()).submenu("Share", window, cx, move |sub, _, _| {
			let url = url.clone();
			sub.item(PopupMenuItem::new(label).on_click(move |_, _, cx: &mut App| {
				cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(url.clone()));
			}))
		});
	}
	menu
}

/// "Move to folder ▸" and "Add to other playlist ▸".
fn submenus(
	mut menu: PopupMenu,
	shell: &WeakEntity<Shell>,
	t: &Target,
	window: &mut Window,
	cx: &mut gpui_kit::Context<'_, PopupMenu>,
) -> PopupMenu {
	if !matches!(t.route, Route::Liked | Route::Artist(_)) {
		// Move to folder ▸ (playlists and folders; Spotify keeps albums out of folders).
		if matches!(t.route, Route::Playlist(_) | Route::Folder(_)) {
			let (shell, uri, parent, folders) = (shell.clone(), t.uri.clone(), t.parent.clone(), t.folders.clone());
			let own = match &t.route {
				Route::Folder(id) => Some(id.clone()),
				_ => None,
			};
			menu = menu.submenu("Move to folder", window, cx, move |sub, _, _| {
				let mut sub = sub.min_w(gpui_kit::px(220.));
				let item = |label: String, folder: Option<String>| {
					let (shell, uri) = (shell.clone(), uri.clone());
					PopupMenuItem::new(label).on_click(move |_, _, cx: &mut App| {
						let _ = shell.update(cx, |this, cx| this.send(Msg::MoveToFolder { uri: uri.clone(), folder: folder.clone() }, cx));
					})
				};
				sub = sub.item(item("Create folder".to_owned(), None).disabled(true));
				if parent.is_some() {
					sub = sub.item(item("Remove from folder".to_owned(), None));
				}
				sub = sub.item(PopupMenuItem::separator());
				for (id, name) in &folders {
					if Some(id) != parent.as_ref() && Some(id) != own.as_ref() {
						sub = sub.item(item(name.clone(), Some(id.clone())));
					}
				}
				sub
			});
		}
		if matches!(t.route, Route::Playlist(_) | Route::Album(_)) {
			let (shell, route, playlists) = (shell.clone(), t.route.clone(), t.playlists.clone());
			menu = menu.submenu("Add to other playlist", window, cx, move |sub, _, _| {
				let mut sub = sub.min_w(gpui_kit::px(220.)).max_h(gpui_kit::px(420.)).scrollable(true);
				let new = {
					let (shell, route) = (shell.clone(), route.clone());
					PopupMenuItem::new("New playlist").on_click(move |_, _, cx: &mut App| {
						let _ = shell.update(cx, |this, cx| {
							this.send(Msg::AddContextToPlaylist(route.clone()), cx);
							this.send(Msg::AddToPlaylist(None), cx);
						});
					})
				};
				sub = sub.item(new).item(PopupMenuItem::separator());
				for (id, name) in &playlists {
					let (shell, route, id) = (shell.clone(), route.clone(), id.clone());
					sub = sub.item(PopupMenuItem::new(name.clone()).on_click(move |_, _, cx: &mut App| {
						let _ = shell.update(cx, |this, cx| {
							this.send(Msg::AddContextToPlaylist(route.clone()), cx);
							this.send(Msg::AddToPlaylist(Some(id.clone())), cx);
						});
					}));
				}
				sub
			});
		}
	}
	menu
}
