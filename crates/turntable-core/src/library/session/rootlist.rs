//! Rootlist folder markers and revision-checked folder edits use Playlist4 only.
//! Start names are URI-escaped; all raw pages are kept until the matching end marker,
//! so a folder cannot be silently flattened or cut at the old playlist-only limit.

use std::collections::HashSet;

use futures_util::TryStreamExt as _;
use librespot_core::Session;
use librespot_protocol::playlist4_external::{Add, Item, MetaItem, Mov, Op, Rem, SelectedListContent, op::Kind};
use protobuf::MessageField;

use super::{Reader, artwork, playlist};
use crate::{
	library::{Folder, RootEntry},
	spotify::{self, Playlist},
};

#[derive(Default)]
pub(super) struct Data {
	pub playlists: Vec<Playlist>,
	pub entries:   Vec<RootEntry>,
}

#[derive(Default)]
struct Parser {
	data:  Data,
	stack: Vec<Folder>,
	seen:  HashSet<String>,
}

pub(super) fn start(uri: &str) -> Option<(&str, &str)> {
	let marker = uri.strip_prefix("spotify:start-group:")?;
	Some(marker.split_once(':').unwrap_or((marker, "")))
}

fn start_uri(id: &str, name: &str) -> String {
	format!("spotify:start-group:{id}:{}", spotify::urlencode(name))
}

fn item(uri: String) -> Item {
	Item { uri: Some(uri), ..Default::default() }
}

impl Parser {
	fn append(&mut self, entry: RootEntry) {
		let count = match &entry {
			RootEntry::Playlist(_) => 1,
			RootEntry::Folder(folder) => folder.count,
		};
		if let Some(folder) = self.stack.last_mut() {
			folder.count += count;
			folder.children.push(entry);
		} else {
			self.data.entries.push(entry);
		}
	}

	fn page(&mut self, raw: &SelectedListContent) -> Result<(), librespot_core::Error> {
		let contents = raw.contents.get_or_default();
		for (index, item) in contents.items.iter().enumerate() {
			if let Some((id, name)) = start(item.uri()) {
				let name = if name.is_empty() {
					contents.meta_items.get(index).map_or("", |meta| meta.attributes.get_or_default().name()).to_owned()
				} else {
					spotify::urldecode(name)
				};
				self.stack.push(Folder { id: id.to_owned(), name, count: 0, children: Vec::new() });
			} else if let Some(id) = item.uri().strip_prefix("spotify:end-group:") {
				let folder = self
					.stack
					.pop()
					.filter(|folder| folder.id == id)
					.ok_or_else(|| librespot_core::Error::failed_precondition("Rootlist folder markers are not balanced"))?;
				self.append(RootEntry::Folder(folder));
			} else if let Some(meta) = contents.meta_items.get(index)
				&& let Some(playlist) = super::root_playlist(item.uri(), meta)
				&& self.seen.insert(playlist.id.clone())
			{
				self.append(RootEntry::Playlist(playlist.id.clone()));
				self.data.playlists.push(playlist);
			}
		}
		Ok(())
	}

	fn finish(self) -> Result<Data, librespot_core::Error> {
		if !self.stack.is_empty() {
			return Err(librespot_core::Error::failed_precondition("Rootlist ended inside a folder"));
		}
		Ok(self.data)
	}
}

struct Raw {
	revision:   Vec<u8>,
	items:      Vec<Item>,
	meta_items: Vec<MetaItem>,
}

async fn raw(session: &Session) -> Result<Raw, librespot_core::Error> {
	let mut out = Raw { revision: Vec::new(), items: Vec::new(), meta_items: Vec::new() };
	let mut offset = 0;
	loop {
		let mut page = playlist::root(session, offset, 120).await?;
		if out.revision.is_empty() {
			out.revision = page.revision().to_vec();
		} else if out.revision != page.revision() {
			return Err(librespot_core::Error::failed_precondition("Rootlist changed during pagination"));
		}
		let contents = page.contents.mut_or_insert_default();
		let next = u32::try_from(contents.pos())? + u32::try_from(contents.items.len())?;
		let more = contents.truncated();
		contents.meta_items.resize_with(contents.items.len(), MetaItem::default);
		out.items.append(&mut contents.items);
		out.meta_items.append(&mut contents.meta_items);
		if !more {
			return Ok(out);
		}
		if next <= offset {
			return Err(librespot_core::Error::failed_precondition("Rootlist pagination made no progress"));
		}
		offset = next;
	}
}

pub(super) async fn fetch(session: &Session) -> Result<Data, librespot_core::Error> {
	let raw = raw(session).await?;
	let mut parser = Parser::default();
	parser.page(&SelectedListContent {
		contents: MessageField::some(librespot_protocol::playlist4_external::ListItems {
			items: raw.items,
			meta_items: raw.meta_items,
			..Default::default()
		}),
		..Default::default()
	})?;
	let mut data = parser.finish()?;
	{
		let reads =
			crate::net::par::async_map(
				data.playlists.iter_mut(),
				4,
				|playlist| async move { artwork::fill(session, playlist, None).await },
			);
		futures_util::pin_mut!(reads);
		while reads.try_next().await?.is_some() {}
	}
	Ok(data)
}

fn bounds(items: &[Item], id: &str) -> Result<(usize, usize), librespot_core::Error> {
	let begin = items
		.iter()
		.position(|item| start(item.uri()).is_some_and(|(known, _)| known == id))
		.ok_or_else(|| librespot_core::Error::not_found("Folder is no longer in Your Library"))?;
	let end = items
		.iter()
		.enumerate()
		.skip(begin + 1)
		.find_map(|(index, item)| (item.uri().strip_prefix("spotify:end-group:") == Some(id)).then_some(index))
		.ok_or_else(|| librespot_core::Error::failed_precondition("Folder is missing its end marker"))?;
	Ok((begin, end))
}

fn rename(items: &[Item], index: usize, id: &str, name: &str) -> Result<Vec<Op>, librespot_core::Error> {
	let mut marker = items.get(index).cloned().ok_or_else(|| librespot_core::Error::not_found("Folder marker no longer exists"))?;
	marker.uri = Some(start_uri(id, name));
	let index = i32::try_from(index)?;
	// A single revision-checked delta replaces only the start marker, preserving
	// its attributes, the matching end marker, and all child occurrences.
	Ok(vec![
		Op {
			kind: Some(Kind::REM.into()),
			rem: MessageField::some(Rem { from_index: Some(index), length: Some(1), ..Default::default() }),
			..Default::default()
		},
		Op {
			kind: Some(Kind::ADD.into()),
			add: MessageField::some(Add { from_index: Some(index), items: vec![marker], ..Default::default() }),
			..Default::default()
		},
	])
}

fn delete(items: &[Item], id: &str) -> Result<Vec<Op>, librespot_core::Error> {
	let (begin, end) = bounds(items, id)?;
	let mut ops = Vec::with_capacity(2);
	for index in [end, begin] {
		ops.push(Op {
			kind: Some(Kind::REM.into()),
			rem: MessageField::some(Rem { from_index: Some(i32::try_from(index)?), length: Some(1), ..Default::default() }),
			..Default::default()
		});
	}
	Ok(ops)
}

fn move_to(items: &[Item], uri: &str, folder: Option<&str>) -> Result<Option<Op>, librespot_core::Error> {
	let (begin, end) = if let Some(id) = uri.strip_prefix("spotify:folder:") {
		bounds(items, id)?
	} else {
		let index = items
			.iter()
			.position(|item| playlist::same_playlist(item.uri(), uri))
			.ok_or_else(|| librespot_core::Error::not_found("Playlist is no longer in Your Library"))?;
		(index, index)
	};
	let target = match folder {
		Some(id) => bounds(items, id)?.0 + 1,
		None => 0,
	};
	if target == begin || (folder.is_none() && target == end + 1) {
		return Ok(None);
	}
	if uri.starts_with("spotify:folder:") && (begin..=end).contains(&target) {
		return Err(librespot_core::Error::failed_precondition("A folder cannot be moved into itself or its descendants"));
	}
	if target == end + 1 {
		return Ok(None);
	}
	Ok(Some(Op {
		kind: Some(Kind::MOV.into()),
		mov: MessageField::some(Mov {
			from_index: Some(i32::try_from(begin)?),
			length: Some(i32::try_from(end - begin + 1)?),
			to_index: Some(i32::try_from(target)?),
			..Default::default()
		}),
		..Default::default()
	}))
}

async fn change(session: &Session, current: &Raw, ops: Vec<Op>) -> Result<(), librespot_core::Error> {
	playlist::post(session, &format!("/playlist/v2/user/{}/rootlist/changes", session.username()), &current.revision, ops).await
}

impl Reader {
	pub(in crate::library) fn create_folder(&mut self, name: &str) -> spotify::Result<String> {
		let id = format!("{:016x}", rand::random::<u64>());
		self.shared
			.block_on(async {
				let current = playlist::root(self.shared.session(), 0, 0).await?;
				playlist::post(
					self.shared.session(),
					&format!("/playlist/v2/user/{}/rootlist/changes", self.shared.session().username()),
					current.revision(),
					vec![Op {
						kind: Some(Kind::ADD.into()),
						add: MessageField::some(Add {
							add_first: Some(true),
							items: vec![item(start_uri(&id, name)), item(format!("spotify:end-group:{id}"))],
							..Default::default()
						}),
						..Default::default()
					}],
				)
				.await
			})
			.map_err(spotify::Error::from)?;
		self.roots = None;
		Ok(id)
	}

	pub(in crate::library) fn rename_folder(&mut self, id: &str, name: &str) -> spotify::Result<()> {
		self.shared
			.block_on(async {
				let current = raw(self.shared.session()).await?;
				let (begin, _) = bounds(&current.items, id)?;
				let ops = rename(&current.items, begin, id, name)?;
				change(self.shared.session(), &current, ops).await
			})
			.map_err(spotify::Error::from)?;
		self.roots = None;
		Ok(())
	}

	pub(in crate::library) fn delete_folder(&mut self, id: &str) -> spotify::Result<()> {
		self.shared
			.block_on(async {
				let current = raw(self.shared.session()).await?;
				let ops = delete(&current.items, id)?;
				change(self.shared.session(), &current, ops).await
			})
			.map_err(spotify::Error::from)?;
		self.roots = None;
		Ok(())
	}

	pub(in crate::library) fn move_to_folder(&mut self, uri: &str, folder: Option<&str>) -> spotify::Result<()> {
		self.shared
			.block_on(async {
				let current = raw(self.shared.session()).await?;
				if let Some(op) = move_to(&current.items, uri, folder)? {
					change(self.shared.session(), &current, vec![op]).await?;
				}
				Ok::<_, librespot_core::Error>(())
			})
			.map_err(spotify::Error::from)?;
		self.roots = None;
		Ok(())
	}
}

#[cfg(test)]
mod tests;
