use librespot_protocol::playlist4_external::{ListAttributes, ListItems};
use protobuf::Message;

use super::*;

fn page(uris: &[&str]) -> SelectedListContent {
	SelectedListContent {
		contents: MessageField::some(ListItems {
			items: uris.iter().map(|uri| item((*uri).to_owned())).collect(),
			meta_items: uris
				.iter()
				.map(|uri| MetaItem {
					owner_username: Some("alice".into()),
					attributes: MessageField::some(ListAttributes { name: Some((*uri).into()), ..Default::default() }),
					..Default::default()
				})
				.collect(),
			..Default::default()
		}),
		..Default::default()
	}
}

#[test]
fn folder_markers_keep_nested_native_order_names_counts_across_pages() -> Result<(), librespot_core::Error> {
	let mut parser = Parser::default();
	parser.page(&page(&[
		"spotify:start-group:outer:Summer+%E9%9B%A8",
		"spotify:playlist:0000000000000000000001",
		"spotify:start-group:inner:Quiet%3A%20Mix",
	]))?;
	parser.page(&page(&[
		"spotify:user:alice:playlist:0000000000000000000002",
		"spotify:end-group:inner",
		"spotify:end-group:outer",
		"spotify:playlist:0000000000000000000003",
		"spotify:start-group:empty:Empty",
		"spotify:end-group:empty",
	]))?;
	let data = parser.finish()?;
	assert_eq!(data.playlists.len(), 3);
	let RootEntry::Folder(outer) = &data.entries[0] else { panic!("missing outer folder") };
	assert_eq!(outer.name, "Summer 雨");
	assert_eq!(outer.count, 2);
	assert_eq!(outer.children[0], RootEntry::Playlist("0000000000000000000001".into()));
	let RootEntry::Folder(inner) = &outer.children[1] else { panic!("missing nested folder") };
	assert_eq!(inner.name, "Quiet: Mix");
	assert_eq!(inner.count, 1);
	assert_eq!(data.entries[1], RootEntry::Playlist("0000000000000000000003".into()));
	let RootEntry::Folder(empty) = &data.entries[2] else { panic!("missing empty folder") };
	assert_eq!(empty.count, 0);
	assert!(empty.children.is_empty());
	Ok(())
}

#[test]
fn unbalanced_folder_markers_are_not_silently_flattened() -> Result<(), librespot_core::Error> {
	let mut parser = Parser::default();
	parser.page(&page(&["spotify:start-group:open:Unclosed"]))?;
	assert!(parser.finish().is_err());
	assert!(Parser::default().page(&page(&["spotify:end-group:orphan"])).is_err());
	Ok(())
}

#[test]
fn native_folder_rename_delete_and_group_move_preserve_raw_occurrences() -> Result<(), librespot_core::Error> {
	let mut raw = page(&[
		"spotify:playlist:0000000000000000000000",
		"spotify:start-group:outer:Folder",
		"spotify:playlist:0000000000000000000001",
		"spotify:start-group:inner:Nested",
		"spotify:playlist:0000000000000000000002",
		"spotify:end-group:inner",
		"spotify:end-group:outer",
		"spotify:playlist:0000000000000000000003",
	]);
	let items = &raw.contents.items;
	let renamed = rename(items, 1, "outer", "雨: + Mix")?;
	assert_eq!(renamed.len(), 2);
	let removed = Op::parse_from_bytes(&renamed[0].write_to_bytes()?)?;
	let added = Op::parse_from_bytes(&renamed[1].write_to_bytes()?)?;
	assert_eq!(removed.kind(), Kind::REM);
	assert_eq!(removed.rem.from_index(), 1);
	assert_eq!(removed.rem.length(), 1);
	assert_eq!(added.kind(), Kind::ADD);
	assert_eq!(added.add.from_index(), 1);
	assert_eq!(start(added.add.items[0].uri()).map(|(_, name)| spotify::urldecode(name)), Some("雨: + Mix".into()));
	let moved = move_to(items, "spotify:folder:outer", None)?.expect("folder changes root position");
	assert_eq!(moved.mov.from_index(), 1);
	assert_eq!(moved.mov.length(), 6);
	assert_eq!(moved.mov.to_index(), 0);
	let moved = move_to(items, "spotify:playlist:0000000000000000000003", Some("inner"))?.expect("playlist moves into nested folder");
	assert_eq!(moved.mov.from_index(), 7);
	assert_eq!(moved.mov.to_index(), 4);
	assert_eq!(moved.mov.length(), 1);
	assert!(move_to(items, "spotify:folder:outer", Some("inner")).is_err());
	assert!(move_to(items, "spotify:folder:outer", Some("outer")).is_err());
	let deleted = delete(items, "outer")?;
	assert_eq!(deleted.iter().map(|op| op.rem.from_index()).collect::<Vec<_>>(), [6, 1]);
	for op in deleted {
		let index = usize::try_from(op.rem.from_index())?;
		let contents = raw.contents.mut_or_insert_default();
		contents.items.remove(index);
		contents.meta_items.remove(index);
	}
	let mut parser = Parser::default();
	parser.page(&raw)?;
	let data = parser.finish()?;
	assert_eq!(data.playlists.len(), 4, "deleting a folder removes no playlists");
	assert_eq!(data.entries[1], RootEntry::Playlist("0000000000000000000001".into()));
	let RootEntry::Folder(inner) = &data.entries[2] else { panic!("nested folder must move up a level") };
	assert_eq!(inner.id, "inner");
	assert_eq!(inner.count, 1);
	Ok(())
}
