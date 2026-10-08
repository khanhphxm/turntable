//! Revision-checked Playlist4 edits. A row removal targets one occurrence; moves
//! use final, zero-based row indices (Playlist4's insertion index is pre-removal).

#[cfg(test)]
mod live;

use std::time::{SystemTime, UNIX_EPOCH};

use http::Method;
use librespot_core::{Session, SpotifyId};
use librespot_protocol::playlist4_external::{
	Add, ChangeInfo, Delta, Item, ItemAttributes, ListAttributes, ListAttributesPartialState, ListChanges, Mov, Op, Rem,
	SelectedListContent, UpdateListAttributes, op::Kind,
};
use protobuf::{Message, MessageField};
use rand::RngCore;

use super::{Reader, network};
use crate::spotify::{self, Playlist, PlaylistChanges};

#[cfg(test)]
static REQUESTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn item(uri: &str) -> Item {
	Item { uri: Some(uri.to_owned()), ..Default::default() }
}

fn append(uris: &[String]) -> Op {
	Op {
		kind: Some(Kind::ADD.into()),
		add: MessageField::some(Add { add_last: Some(true), items: uris.iter().map(|uri| item(uri)).collect(), ..Default::default() }),
		..Default::default()
	}
}

fn remove(position: i32, uri: &str) -> Op {
	Op {
		kind: Some(Kind::REM.into()),
		rem: MessageField::some(Rem { from_index: Some(position), length: Some(1), items: vec![item(uri)], ..Default::default() }),
		..Default::default()
	}
}

fn move_row(from: i32, to: i32) -> Op {
	Op {
		kind: Some(Kind::MOV.into()),
		mov: MessageField::some(Mov {
			from_index: Some(from),
			length: Some(1),
			to_index: Some(if to > from { to + 1 } else { to }),
			..Default::default()
		}),
		..Default::default()
	}
}

fn attributes(changes: &PlaylistChanges) -> Op {
	Op {
		kind: Some(Kind::UPDATE_LIST_ATTRIBUTES.into()),
		update_list_attributes: MessageField::some(UpdateListAttributes {
			new_attributes: MessageField::some(ListAttributesPartialState {
				values: MessageField::some(ListAttributes {
					name: changes.name.clone(),
					description: changes.description.clone(),
					..Default::default()
				}),
				..Default::default()
			}),
			..Default::default()
		}),
		..Default::default()
	}
}

fn changes(username: &str, revision: &[u8], ops: Vec<Op>) -> ListChanges {
	ListChanges {
		base_revision: Some(revision.to_vec()),
		deltas: vec![Delta {
			ops,
			info: MessageField::some(ChangeInfo {
				user: Some(username.to_owned()),
				timestamp: Some(
					SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX)),
				),
				..Default::default()
			}),
			..Default::default()
		}],
		want_resulting_revisions: Some(true),
		want_sync_result: Some(true),
		..Default::default()
	}
}

fn create_attributes(name: &str) -> Op {
	attributes(&PlaylistChanges { name: Some(name.to_owned()), description: Some(String::new()), public: None })
}

pub(super) async fn fetch(session: &Session, id: &str, offset: u32, length: u32) -> Result<SelectedListContent, librespot_core::Error> {
	let id = SpotifyId::from_base62(id)?.to_base62()?;
	let endpoint = format!("/playlist/v2/playlist/{id}?from={offset}&length={length}");
	#[cfg(test)]
	REQUESTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
	let bytes = session.spclient().request(&Method::GET, &endpoint, None, None).await?;
	Ok(SelectedListContent::parse_from_bytes(&bytes)?)
}

async fn post(session: &Session, endpoint: &str, revision: &[u8], ops: Vec<Op>) -> Result<(), librespot_core::Error> {
	let body = changes(&session.username(), revision, ops);
	#[cfg(test)]
	REQUESTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
	// Sync/resync flags describe the returned view, not whether the write applied.
	session.spclient().request_with_protobuf(&Method::POST, endpoint, None, &body).await?;
	Ok(())
}

async fn root(session: &Session, offset: u32, length: u32) -> Result<SelectedListContent, librespot_core::Error> {
	#[cfg(test)]
	REQUESTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
	let bytes = session.spclient().get_rootlist(usize::try_from(offset)?, Some(usize::try_from(length)?)).await?;
	Ok(SelectedListContent::parse_from_bytes(&bytes)?)
}

fn position(position: u32) -> spotify::Result<i32> {
	i32::try_from(position).map_err(|error| spotify::Error::Parse(format!("Playlist row is out of range: {error}")))
}

impl Reader {
	pub(in crate::library) fn follow_playlist(&mut self, uri: &str, followed: bool) -> spotify::Result<()> {
		self.shared
			.block_on(async {
				let session = self.shared.session();
				let endpoint = format!("/playlist/v2/user/{}/rootlist/changes", session.username());
				if followed {
					let current = root(session, 0, 0).await?;
					let op = Op {
						kind: Some(Kind::ADD.into()),
						add: MessageField::some(Add { add_first: Some(true), items: vec![item(uri)], ..Default::default() }),
						..Default::default()
					};
					return post(session, &endpoint, current.revision(), vec![op]).await;
				}
				let mut offset = 0;
				loop {
					let current = root(session, offset, 120).await?;
					let contents = current.contents.get_or_default();
					if let Some(index) = contents.items.iter().position(|item| item.uri() == uri) {
						let index = i32::try_from(index)? + contents.pos();
						return post(session, &endpoint, current.revision(), vec![remove(index, uri)]).await;
					}
					if !contents.truncated() {
						return Ok(());
					}
					let next = u32::try_from(contents.pos())? + u32::try_from(contents.items.len())?;
					if next <= offset {
						return Err(librespot_core::Error::failed_precondition("Rootlist pagination made no progress"));
					}
					offset = next;
				}
			})
			.map_err(network)?;
		self.roots = None;
		Ok(())
	}

	pub(in crate::library) fn create_playlist(&mut self, name: &str, uris: &[String]) -> spotify::Result<Playlist> {
		let mut raw = [0; 16];
		rand::rng().fill_bytes(&mut raw);
		let id = SpotifyId::from_raw(&raw).map_err(network)?.to_base62().map_err(network)?;
		let uri = format!("spotify:playlist:{id}");
		self.shared
			.block_on(async {
				// An omitted native description is initialized by Spotify as literal
				// "null". Explicit empty text prevents storing that sentinel.
				let mut ops = vec![create_attributes(name)];
				if !uris.is_empty() {
					ops.push(append(uris));
				}
				post(self.shared.session(), &format!("/playlist/v2/playlist/{id}/changes"), b"\0\0\0\0root", ops).await?;
				let root = root(self.shared.session(), 0, 0).await?;
				let mut entry = item(&uri);
				entry.attributes = MessageField::some(ItemAttributes { public: Some(false), ..Default::default() });
				let op = Op {
					kind: Some(Kind::ADD.into()),
					add: MessageField::some(Add { add_first: Some(true), items: vec![entry], ..Default::default() }),
					..Default::default()
				};
				post(
					self.shared.session(),
					&format!("/playlist/v2/user/{}/rootlist/changes", self.shared.session().username()),
					root.revision(),
					vec![op],
				)
				.await
			})
			.map_err(network)?;
		self.roots = None;
		Ok(super::playlist(
			&id,
			&self.shared.session().username(),
			&ListAttributes { name: Some(name.to_owned()), ..Default::default() },
			i32::try_from(uris.len()).unwrap_or(i32::MAX),
		))
	}

	pub(in crate::library) fn rename_playlist(&mut self, id: &str, edits: &PlaylistChanges) -> spotify::Result<()> {
		self.shared
			.block_on(async {
				let current = fetch(self.shared.session(), id, 0, 0).await?;
				post(self.shared.session(), &format!("/playlist/v2/playlist/{id}/changes"), current.revision(), vec![attributes(edits)])
					.await
			})
			.map_err(network)?;
		self.roots = None;
		Ok(())
	}

	pub(in crate::library) fn add_to_playlist(&mut self, id: &str, uris: &[String]) -> spotify::Result<()> {
		if uris.is_empty() {
			return Ok(());
		}
		self.shared
			.block_on(async {
				let current = fetch(self.shared.session(), id, 0, 0).await?;
				post(self.shared.session(), &format!("/playlist/v2/playlist/{id}/changes"), current.revision(), vec![append(uris)]).await
			})
			.map_err(network)?;
		self.roots = None;
		Ok(())
	}

	pub(in crate::library) fn remove_from_playlist(&mut self, id: &str, uri: &str, row: u32) -> spotify::Result<()> {
		let index = position(row)?;
		self.shared
			.block_on(async {
				let current = fetch(self.shared.session(), id, row, 1).await?;
				if current.contents.get_or_default().items.first().is_none_or(|item| item.uri() != uri) {
					return Err(librespot_core::Error::failed_precondition("The playlist changed; reopen it before editing"));
				}
				post(self.shared.session(), &format!("/playlist/v2/playlist/{id}/changes"), current.revision(), vec![remove(index, uri)])
					.await
			})
			.map_err(network)?;
		self.roots = None;
		Ok(())
	}

	pub(in crate::library) fn move_playlist_row(&mut self, id: &str, from: u32, to: u32) -> spotify::Result<()> {
		let (source, target) = (position(from)?, position(to)?);
		if from == to {
			return Ok(());
		}
		self.shared
			.block_on(async {
				let current = fetch(self.shared.session(), id, 0, 0).await?;
				if source >= current.length() || target >= current.length() {
					return Err(librespot_core::Error::failed_precondition("Playlist row is out of range"));
				}
				post(self.shared.session(), &format!("/playlist/v2/playlist/{id}/changes"), current.revision(), vec![move_row(
					source, target,
				)])
				.await
			})
			.map_err(network)?;
		self.roots = None;
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn operations_preserve_occurrences_and_move_insertion_semantics() -> Result<(), protobuf::Error> {
		let add = append(&["spotify:track:a".into(), "spotify:track:a".into()]);
		assert_eq!(add.add.items.len(), 2);
		assert!(add.add.add_last());
		let rem = remove(3, "spotify:track:a");
		assert_eq!(rem.rem.from_index(), 3);
		assert_eq!(rem.rem.length(), 1);
		assert_eq!(rem.rem.items[0].uri(), "spotify:track:a");
		assert_eq!(move_row(1, 4).mov.to_index(), 5);
		assert_eq!(move_row(4, 1).mov.to_index(), 1);
		let body = changes("alice", &[1, 2], vec![add, rem, move_row(1, 4)]);
		let decoded = ListChanges::parse_from_bytes(&body.write_to_bytes()?)?;
		assert_eq!(decoded.base_revision(), &[1, 2]);
		assert_eq!(decoded.deltas[0].info.user(), "alice");
		assert!(decoded.want_resulting_revisions());
		Ok(())
	}

	#[test]
	fn rename_only_sets_requested_attributes() {
		let op = attributes(&PlaylistChanges { name: Some("Mix".into()), ..Default::default() });
		let values = &op.update_list_attributes.new_attributes.values;
		assert_eq!(values.name(), "Mix");
		assert!(!values.has_description());
		assert!(position(u32::MAX).is_err());
	}

	#[test]
	fn native_creation_explicitly_initializes_an_empty_description() -> Result<(), protobuf::Error> {
		let decoded = Op::parse_from_bytes(&create_attributes("My Playlist").write_to_bytes()?)?;
		let values = &decoded.update_list_attributes.new_attributes.values;
		assert_eq!(values.name(), "My Playlist");
		assert!(values.has_description());
		assert_eq!(values.description(), "");
		Ok(())
	}
}
