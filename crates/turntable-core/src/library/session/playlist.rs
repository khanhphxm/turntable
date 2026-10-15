//! Revision-checked Playlist4 edits. A row removal targets one occurrence; moves
//! use final, zero-based row indices (Playlist4's insertion index is pre-removal).

use std::time::{SystemTime, UNIX_EPOCH};

use http::Method;
use librespot_core::{Session, SpotifyId, SpotifyUri};
use librespot_protocol::{
	playlist_permission::{Permission, PermissionLevel, SetPermissionLevelRequest},
	playlist4_external::{
		Add, ChangeInfo, Delta, Item, ItemAttributes, ListAttributes, ListAttributesPartialState, ListChanges, Mov, Op, Rem,
		SelectedListContent, UpdateListAttributes, op::Kind,
	},
};
use protobuf::{Message, MessageField};

use super::Reader;
use crate::{
	net::trace,
	spotify::{self, Playlist, PlaylistChanges},
};

fn item(uri: &str) -> Item {
	Item { uri: Some(uri.to_owned()), ..Default::default() }
}

/// Playlist4 stores added timestamps in milliseconds, unlike collection Unix seconds.
pub(super) fn row_attributes(item: &Item) -> (&str, Option<i64>, Option<&str>) {
	let attributes = item.attributes.get_or_default();
	(item.uri(), attributes.timestamp.map(|timestamp| timestamp.div_euclid(1000)), attributes.added_by.as_deref())
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

fn visibility(public: bool) -> SetPermissionLevelRequest {
	SetPermissionLevelRequest {
		permission_level: Some(if public { PermissionLevel::VIEWER } else { PermissionLevel::BLOCKED }.into()),
		..Default::default()
	}
}

/// Playlist privacy is a permission, not the rootlist's separate profile-publication flag.
pub(super) async fn public(session: &Session, id: &str) -> Result<Option<bool>, librespot_core::Error> {
	let endpoint = format!("/playlist-permission/v1/playlist/{id}/permission/base");
	let span = trace::start();
	let bytes = session.spclient().request(&Method::GET, &endpoint, None, None).await?;
	if trace::enabled() {
		span.done("session", "GET", &format!("https://spclient.spotify.com{endpoint}"), trace::Reply {
			status: 200,
			bytes: bytes.len(),
			..trace::Reply::default()
		});
	}
	let permission = Permission::parse_from_bytes(&bytes)?;
	Ok(match permission.permission_level() {
		PermissionLevel::VIEWER | PermissionLevel::CONTRIBUTOR => Some(true),
		PermissionLevel::BLOCKED => Some(false),
		PermissionLevel::UNKNOWN | PermissionLevel::MADE_FOR => None,
	})
}

async fn set_public(session: &Session, id: &str, public: bool) -> Result<(), librespot_core::Error> {
	let endpoint = format!("/playlist-permission/v1/playlist/{id}/permission/base/level");
	let span = trace::start();
	let bytes = session.spclient().request_with_protobuf(&Method::POST, &endpoint, None, &visibility(public)).await?;
	if trace::enabled() {
		span.done("session", "POST", &format!("https://spclient.spotify.com{endpoint}"), trace::Reply {
			status: 200,
			bytes: bytes.len(),
			..trace::Reply::default()
		});
	}
	Ok(())
}

pub(super) fn same_playlist(left: &str, right: &str) -> bool {
	matches!((SpotifyUri::from_uri(left), SpotifyUri::from_uri(right)),
		(Ok(SpotifyUri::Playlist { id: left, .. }), Ok(SpotifyUri::Playlist { id: right, .. })) if left == right)
}

fn require_owner(session: &Session, playlist: &SelectedListContent) -> Result<(), librespot_core::Error> {
	if playlist.owner_username() != session.username() {
		return Err(librespot_core::Error::permission_denied("Only the playlist owner can edit these details"));
	}
	Ok(())
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
	let span = trace::start();
	let bytes = session.spclient().request(&Method::GET, &endpoint, None, None).await?;
	if trace::enabled() {
		span.done("session", "GET", &format!("https://spclient.spotify.com{endpoint}"), trace::Reply {
			status: 200,
			bytes: bytes.len(),
			..trace::Reply::default()
		});
	}
	Ok(SelectedListContent::parse_from_bytes(&bytes)?)
}

pub(super) async fn post(session: &Session, endpoint: &str, revision: &[u8], ops: Vec<Op>) -> Result<(), librespot_core::Error> {
	let body = changes(&session.username(), revision, ops);
	// Sync/resync flags describe the returned view, not whether the write applied.
	let span = trace::start();
	let bytes = session.spclient().request_with_protobuf(&Method::POST, endpoint, None, &body).await?;
	if trace::enabled() {
		span.done("session", "POST", &format!("https://spclient.spotify.com{endpoint}"), trace::Reply {
			status: 200,
			bytes: bytes.len(),
			..trace::Reply::default()
		});
	}
	Ok(())
}

pub(super) async fn root(session: &Session, offset: u32, length: u32) -> Result<SelectedListContent, librespot_core::Error> {
	let span = trace::start();
	let bytes = session.spclient().get_rootlist(usize::try_from(offset)?, Some(usize::try_from(length)?)).await?;
	span.done("session", "GET", "https://spclient.spotify.com/playlist/v2/user/me/rootlist", trace::Reply {
		status: 200,
		bytes: bytes.len(),
		..trace::Reply::default()
	});
	Ok(SelectedListContent::parse_from_bytes(&bytes)?)
}

/// The rootlist's raw indices include folders; edit the matching occurrence, not a flattened row.
async fn change_root(session: &Session, uri: &str, operation: impl Fn(i32, &str) -> Op) -> Result<bool, librespot_core::Error> {
	let mut offset = 0;
	loop {
		let current = root(session, offset, 120).await?;
		let contents = current.contents.get_or_default();
		if let Some((index, entry)) = contents.items.iter().enumerate().find(|(_, entry)| same_playlist(entry.uri(), uri)) {
			let index = i32::try_from(index)? + contents.pos();
			let endpoint = format!("/playlist/v2/user/{}/rootlist/changes", session.username());
			post(session, &endpoint, current.revision(), vec![operation(index, entry.uri())]).await?;
			return Ok(true);
		}
		if !contents.truncated() {
			return Ok(false);
		}
		let next = u32::try_from(contents.pos())? + u32::try_from(contents.items.len())?;
		if next <= offset {
			return Err(librespot_core::Error::failed_precondition("Rootlist pagination made no progress"));
		}
		offset = next;
	}
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
				change_root(session, uri, remove).await?;
				Ok(())
			})
			.map_err(spotify::Error::from)?;
		self.roots = None;
		Ok(())
	}

	pub(in crate::library) fn create_playlist(&mut self, name: &str, uris: &[String]) -> spotify::Result<Playlist> {
		let raw: [u8; 16] = rand::random();
		let id = SpotifyId::from_raw(&raw).map_err(spotify::Error::from)?.to_base62().map_err(spotify::Error::from)?;
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
				set_public(self.shared.session(), &id, false).await?;
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
			.map_err(spotify::Error::from)?;
		self.roots = None;
		let mut created = super::playlist(
			&id,
			&self.shared.session().username(),
			&ListAttributes { name: Some(name.to_owned()), ..Default::default() },
			i32::try_from(uris.len()).unwrap_or(i32::MAX),
		);
		created.public = Some(false);
		Ok(created)
	}

	pub(in crate::library) fn rename_playlist(&mut self, id: &str, edits: &PlaylistChanges) -> spotify::Result<()> {
		self.shared
			.block_on(async {
				let session = self.shared.session();
				let current = fetch(session, id, 0, 0).await?;
				require_owner(session, &current)?;
				if edits.name.is_some() || edits.description.is_some() {
					post(session, &format!("/playlist/v2/playlist/{id}/changes"), current.revision(), vec![attributes(edits)]).await?;
				}
				if let Some(public) = edits.public {
					set_public(session, id, public).await?;
				}
				Ok::<_, librespot_core::Error>(())
			})
			.map_err(spotify::Error::from)?;
		self.roots = None;
		Ok(())
	}

	pub(in crate::library) fn set_collaborative(&mut self, id: &str, collaborative: bool) -> spotify::Result<()> {
		self.shared
			.block_on(async {
				let session = self.shared.session();
				let current = fetch(session, id, 0, 0).await?;
				require_owner(session, &current)?;
				let op = Op {
					kind: Some(Kind::UPDATE_LIST_ATTRIBUTES.into()),
					update_list_attributes: MessageField::some(UpdateListAttributes {
						new_attributes: MessageField::some(ListAttributesPartialState {
							values: MessageField::some(ListAttributes { collaborative: Some(collaborative), ..Default::default() }),
							..Default::default()
						}),
						..Default::default()
					}),
					..Default::default()
				};
				post(session, &format!("/playlist/v2/playlist/{id}/changes"), current.revision(), vec![op]).await
			})
			.map_err(spotify::Error::from)?;
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
			.map_err(spotify::Error::from)?;
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
			.map_err(spotify::Error::from)?;
		self.roots = None;
		Ok(())
	}

	pub(in crate::library) fn move_playlist_row(
		&mut self,
		id: &str,
		from: u32,
		to: u32,
		source_uri: &str,
		target_uri: &str,
	) -> spotify::Result<()> {
		let (source, target) = (position(from)?, position(to)?);
		if from == to {
			return Ok(());
		}
		self.shared
			.block_on(async {
				let (current, destination) =
					tokio::try_join!(fetch(self.shared.session(), id, from, 1), fetch(self.shared.session(), id, to, 1))?;
				validate_move(&current, &destination, source_uri, target_uri)?;
				post(self.shared.session(), &format!("/playlist/v2/playlist/{id}/changes"), current.revision(), vec![move_row(
					source, target,
				)])
				.await
			})
			.map_err(spotify::Error::from)?;
		self.roots = None;
		Ok(())
	}
}

fn validate_move(
	source: &SelectedListContent,
	target: &SelectedListContent,
	source_uri: &str,
	target_uri: &str,
) -> Result<(), librespot_core::Error> {
	if source.revision() != target.revision()
		|| source.contents.get_or_default().items.first().is_none_or(|item| item.uri() != source_uri)
		|| target.contents.get_or_default().items.first().is_none_or(|item| item.uri() != target_uri)
	{
		return Err(librespot_core::Error::failed_precondition("The playlist changed; reopen it before editing"));
	}
	Ok(())
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
	fn public_and_private_use_base_permissions_not_rootlist_publication() -> Result<(), protobuf::Error> {
		for public in [false, true] {
			let decoded = SetPermissionLevelRequest::parse_from_bytes(&visibility(public).write_to_bytes()?)?;
			assert_eq!(decoded.permission_level(), if public { PermissionLevel::VIEWER } else { PermissionLevel::BLOCKED });
			assert!(decoded.has_permission_level());
		}
		assert!(same_playlist("spotify:user:alice:playlist:0000000000000000000001", "spotify:playlist:0000000000000000000001"));
		assert!(!same_playlist("spotify:start-group:0000000000000000000001", "spotify:playlist:0000000000000000000001"));
		Ok(())
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

	#[test]
	fn move_rejects_changed_source_target_or_revision() {
		let mut source = SelectedListContent::new();
		source.set_revision(vec![1]);
		source.contents = MessageField::some(Default::default());
		source.contents.as_mut().unwrap().items = vec![item("source")];
		let mut target = source.clone();
		target.contents.as_mut().unwrap().items = vec![item("target")];
		assert!(validate_move(&source, &target, "source", "target").is_ok());
		assert!(validate_move(&source, &target, "wrong", "target").is_err());
		assert!(validate_move(&source, &target, "source", "wrong").is_err());
		target.set_revision(vec![2]);
		assert!(validate_move(&source, &target, "source", "target").is_err());
	}
}
