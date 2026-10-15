//! Native rootlist hierarchy and account-scoped sidebar expansion.
//! Folder rows stay at their native sibling position; each sibling list is sorted
//! independently. Text filters reveal matching descendants even in collapsed folders.

use std::{
	collections::{HashMap, HashSet},
	fs, io,
	path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::{Entry, Kind, Library, Request, Sort, model::fold};
use crate::{app::config, spotify::Playlist};

/// One native rootlist entry. Playlist IDs refer to `Library::playlists`, so metadata
/// and optimistic playlist edits have a single owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RootEntry {
	Playlist(String),
	Folder(Folder),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
	pub id:       String,
	pub name:     String,
	/// Playlists in this folder and all of its nested folders, not track count.
	pub count:    usize,
	pub children: Vec<RootEntry>,
}

/// A visible depth-first row. Explicit pins may have depth zero while `parent`
/// retains their native folder, so display projection never mutates membership.
#[derive(Debug, Clone, Copy)]
pub struct TreeEntry<'a> {
	pub entry:  Entry<'a>,
	pub depth:  usize,
	pub parent: Option<&'a str>,
}

pub(super) fn find<'a>(entries: &'a [RootEntry], id: &str) -> Option<&'a Folder> {
	entries.iter().find_map(|entry| match entry {
		RootEntry::Folder(folder) if folder.id == id => Some(folder),
		RootEntry::Folder(folder) => find(&folder.children, id),
		RootEntry::Playlist(_) => None,
	})
}

fn playlist_ids<'a>(entries: &'a [RootEntry], out: &mut HashSet<&'a str>) {
	for entry in entries {
		match entry {
			RootEntry::Playlist(id) => {
				out.insert(id);
			},
			RootEntry::Folder(folder) => playlist_ids(&folder.children, out),
		}
	}
}

fn playlist_parent<'a>(entries: &'a [RootEntry], id: &str, parent: Option<&'a str>) -> Option<&'a str> {
	entries.iter().find_map(|entry| match entry {
		RootEntry::Playlist(known) if known == id => parent,
		RootEntry::Folder(folder) => playlist_parent(&folder.children, id, Some(&folder.id)),
		RootEntry::Playlist(_) => None,
	})
}

pub(super) fn remove_playlist(entries: &mut Vec<RootEntry>, id: &str) -> usize {
	let mut removed = 0;
	entries.retain_mut(|entry| match entry {
		RootEntry::Playlist(known) if known == id => {
			removed += 1;
			false
		},
		RootEntry::Folder(folder) => {
			let count = remove_playlist(&mut folder.children, id);
			folder.count = folder.count.saturating_sub(count);
			removed += count;
			true
		},
		RootEntry::Playlist(_) => true,
	});
	removed
}

struct Branch<'a> {
	entry:    Entry<'a>,
	pin:      usize,
	recent:   usize,
	parent:   Option<&'a str>,
	children: Vec<Branch<'a>>,
}

struct Rows<'a> {
	library:   &'a Library,
	playlists: HashMap<&'a str, &'a Playlist>,
	seen:      HashSet<&'a str>,
	kinds:     &'a [Kind],
	needle:    String,
	folded:    String,
	root_pins: bool,
}

impl<'a> Rows<'a> {
	fn new(library: &'a Library, filtered: bool) -> Self {
		let mut seen = HashSet::new();
		playlist_ids(&library.rootlist, &mut seen);
		Self {
			library,
			playlists: library.playlists.iter().map(|playlist| (playlist.id.as_str(), playlist)).collect(),
			seen,
			kinds: if filtered { &library.kinds } else { &[] },
			needle: if filtered { fold(library.filter.trim()).collect() } else { String::new() },
			folded: String::new(),
			root_pins: true,
		}
	}

	fn wanted(&self, kind: Kind) -> bool {
		self.kinds.is_empty() || self.kinds.contains(&kind)
	}

	fn matches(&mut self, title: &str) -> bool {
		if self.needle.is_empty() {
			return true;
		}
		self.folded.clear();
		self.folded.extend(fold(title));
		self.folded.contains(&self.needle)
	}

	fn leaf(&mut self, entry: Entry<'a>, kind: Kind, inherited: bool) -> Option<Branch<'a>> {
		(self.wanted(kind) && (inherited || self.matches(entry.title()))).then(|| self.branch(entry, Vec::new()))
	}

	fn branch(&self, entry: Entry<'a>, children: Vec<Branch<'a>>) -> Branch<'a> {
		let pin = self.library.pinned.iter().position(|uri| pinned(&entry, uri)).unwrap_or(usize::MAX);
		let recent = if self.library.sort == Sort::Recents {
			self.library.home.recents.iter().position(|tile| entry.opens(&tile.route)).unwrap_or(usize::MAX)
		} else {
			usize::MAX
		};
		Branch { entry, pin, recent, parent: None, children }
	}

	fn siblings(&mut self, entries: &'a [RootEntry], inherited: bool, parent: Option<&'a str>) -> Vec<Branch<'a>> {
		let mut rows = Vec::with_capacity(entries.len());
		for entry in entries {
			match entry {
				RootEntry::Playlist(id) => {
					if let Some(playlist) = self.playlists.get(id.as_str()).copied()
						&& !(self.root_pins && parent.is_some() && self.library.is_pinned(&playlist.uri))
						&& let Some(row) = self.leaf(Entry::Playlist(playlist), Kind::Playlist, inherited)
					{
						rows.push(row);
					}
				},
				RootEntry::Folder(folder) => {
					let matches = inherited || self.matches(&folder.name);
					let mut children = if !self.needle.is_empty() || (self.root_pins && self.library.folder_expanded(&folder.id)) {
						self.siblings(&folder.children, matches, Some(&folder.id))
					} else {
						Vec::new()
					};
					self.order(&mut children);
					if self.wanted(Kind::Playlist) && (matches || !children.is_empty()) {
						rows.push(self.branch(Entry::Folder { id: &folder.id, name: &folder.name, count: folder.count }, children));
					}
				},
			}
		}
		rows
	}

	fn roots(&mut self) -> Vec<Branch<'a>> {
		let mut rows = self.siblings(&self.library.rootlist, false, None);
		for playlist in &self.library.playlists {
			let parent =
				if self.library.is_pinned(&playlist.uri) { playlist_parent(&self.library.rootlist, &playlist.id, None) } else { None };
			if (!self.seen.contains(playlist.id.as_str()) || parent.is_some())
				&& let Some(mut row) = self.leaf(Entry::Playlist(playlist), Kind::Playlist, false)
			{
				row.parent = parent;
				rows.push(row);
			}
		}
		for artist in &self.library.followed {
			if let Some(row) = self.leaf(Entry::Artist(artist), Kind::Artist, false) {
				rows.push(row);
			}
		}
		for album in &self.library.albums {
			if let Some(row) = self.leaf(Entry::Album(album), Kind::Album, false) {
				rows.push(row);
			}
		}
		let me = self.library.me.as_ref().and_then(|user| user.display_name.as_deref()).unwrap_or("");
		if let Some(row) = self.leaf(Entry::Liked(me), Kind::Playlist, false) {
			rows.push(row);
		}
		self.order(&mut rows);
		rows
	}

	fn order(&self, rows: &mut [Branch<'_>]) {
		if self.library.sort == Sort::Added && self.library.pinned.is_empty() {
			return;
		}
		rows.sort_by(|a, b| {
			a.pin.cmp(&b.pin).then_with(|| match self.library.sort {
				Sort::Recents => a.recent.cmp(&b.recent),
				Sort::Added => std::cmp::Ordering::Equal,
				Sort::Alphabetical => fold(a.entry.title()).cmp(fold(b.entry.title())),
				Sort::Creator => {
					fold(a.entry.creator()).cmp(fold(b.entry.creator())).then_with(|| fold(a.entry.title()).cmp(fold(b.entry.title())))
				},
			})
		});
	}

	fn flatten(&self, rows: Vec<Branch<'a>>, depth: usize, parent: Option<&'a str>, out: &mut Vec<TreeEntry<'a>>) {
		for row in rows {
			out.push(TreeEntry { entry: row.entry, depth, parent: row.parent.or(parent) });
			if let Entry::Folder { id, .. } = row.entry
				&& (self.library.folder_expanded(id) || !self.needle.is_empty())
			{
				self.flatten(row.children, depth + 1, Some(id), out);
			}
		}
	}
}

fn pinned(entry: &Entry<'_>, uri: &str) -> bool {
	match entry {
		Entry::Liked(_) => uri == "spotify:collection:tracks",
		Entry::Playlist(playlist) => uri == playlist.uri,
		Entry::Album(album) => uri == album.uri,
		Entry::Artist(artist) => uri == artist.uri,
		Entry::Folder { id, .. } => uri.strip_prefix("spotify:folder:") == Some(*id),
	}
}

impl Library {
	pub fn folder(&self, id: &str) -> Option<&Folder> {
		find(&self.rootlist, id)
	}

	pub fn folder_expanded(&self, id: &str) -> bool {
		self.expanded_folders.contains(id)
	}

	pub(crate) fn toggle_folder(&mut self, id: String) -> Option<Request> {
		self.folder(&id)?;
		let expanded = if self.expanded_folders.remove(&id) {
			false
		} else {
			self.expanded_folders.insert(id.clone());
			true
		};
		Some(Request::SetFolderExpanded { id, expanded })
	}

	/// Visible sidebar rows. `filtered=false` ignores chips/text, but preserves sibling sort/pins.
	pub fn tree(&self, filtered: bool) -> Vec<TreeEntry<'_>> {
		let mut rows = Rows::new(self, filtered);
		let branches = rows.roots();
		let mut out = Vec::with_capacity(self.playlists.len() + self.albums.len() + self.followed.len() + 1);
		rows.flatten(branches, 0, None, &mut out);
		out
	}

	/// Direct folder-page children, regardless of sidebar expansion.
	pub fn folder_entries(&self, id: &str, filtered: bool) -> Vec<Entry<'_>> {
		let Some(folder) = self.folder(id) else { return Vec::new() };
		let mut rows = Rows::new(self, filtered);
		rows.root_pins = false;
		let mut branches = rows.siblings(&folder.children, false, Some(&folder.id));
		rows.order(&mut branches);
		branches.into_iter().map(|row| row.entry).collect()
	}
}

#[derive(Serialize, Deserialize)]
struct Saved {
	account:  String,
	expanded: HashSet<String>,
}

fn path(config: &Path, account: &str) -> PathBuf {
	config.join("library-folders").join(format!("{}.json", crate::sha256_hex(account.as_bytes())))
}

pub(super) fn load(config: &Path, account: &str) -> io::Result<HashSet<String>> {
	let bytes = match fs::read(path(config, account)) {
		Ok(bytes) => bytes,
		Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(HashSet::new()),
		Err(error) => return Err(error),
	};
	let saved: Saved = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
	if saved.account != account {
		return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid folder expansion for this account"));
	}
	Ok(saved.expanded)
}

pub(super) fn set(config_dir: &Path, account: &str, id: &str, expanded: bool) -> io::Result<HashSet<String>> {
	let mut ids = load(config_dir, account)?;
	let changed = if expanded { ids.insert(id.to_owned()) } else { ids.remove(id) };
	if changed {
		let destination = path(config_dir, account);
		if let Some(parent) = destination.parent() {
			fs::create_dir_all(parent)?;
		}
		let bytes = serde_json::to_vec(&Saved { account: account.to_owned(), expanded: ids.clone() }).map_err(io::Error::other)?;
		config::write_private(&destination, &bytes)?;
	}
	Ok(ids)
}

#[cfg(test)]
mod tests;
