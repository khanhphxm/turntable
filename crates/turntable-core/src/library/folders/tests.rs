use super::*;
use crate::{
	library::{Response, Route},
	spotify::User,
};

fn library() -> Library {
	let playlists = [("a", "Zebra"), ("b", "Amber"), ("c", "Quiet"), ("d", "Root")]
		.into_iter()
		.map(|(id, name)| {
			serde_json::from_value(
				serde_json::json!({ "id": id, "uri": format!("spotify:playlist:{id}"), "name": name, "owner": {"id":"alice"} }),
			)
			.unwrap()
		})
		.collect();
	Library {
		me: Some(User { id: "alice".into(), display_name: Some("Alice".into()) }),
		playlists,
		sort: Sort::Added,
		rootlist: vec![
			RootEntry::Folder(Folder {
				id:       "outer".into(),
				name:     "Music".into(),
				count:    3,
				children: vec![
					RootEntry::Playlist("a".into()),
					RootEntry::Folder(Folder {
						id:       "inner".into(),
						name:     "Evening".into(),
						count:    1,
						children: vec![RootEntry::Playlist("c".into())],
					}),
					RootEntry::Playlist("b".into()),
				],
			}),
			RootEntry::Playlist("d".into()),
		],
		..Library::default()
	}
}

fn titles<'a>(rows: &'a [TreeEntry<'_>]) -> Vec<&'a str> {
	rows.iter().map(|row| row.entry.title()).collect()
}

#[test]
fn visible_tree_preserves_native_order_depth_and_parent_until_expanded() {
	let mut library = library();
	assert_eq!(titles(&library.tree(true)), ["Music", "Root", "Liked Songs"]);
	assert!(matches!(library.toggle_folder("outer".into()), Some(Request::SetFolderExpanded { expanded: true, .. })));
	let rows = library.tree(true);
	assert_eq!(titles(&rows), ["Music", "Zebra", "Evening", "Amber", "Root", "Liked Songs"]);
	assert_eq!(rows.iter().map(|row| row.depth).collect::<Vec<_>>(), [0, 1, 1, 1, 0, 0]);
	assert_eq!(rows[1].parent, Some("outer"));
	library.toggle_folder("inner".into());
	let rows = library.tree(true);
	assert_eq!(rows[3].entry.title(), "Quiet");
	assert_eq!(rows[3].depth, 2);
	assert_eq!(rows[3].parent, Some("inner"));
	assert_eq!(library.folder("outer").unwrap().count, 3);
	assert_eq!(library.folder("inner").unwrap().count, 1);
}

#[test]
fn sort_stays_within_siblings_and_explicit_nested_pins_project_once_at_root() {
	let mut library = library();
	library.sort = Sort::Alphabetical;
	library.toggle_folder("outer".into());
	assert_eq!(titles(&library.tree(true)), ["Liked Songs", "Music", "Amber", "Evening", "Zebra", "Root"]);
	library.pinned.push("spotify:playlist:a".into());
	assert_eq!(library.folder_entries("outer", true).iter().map(Entry::title).collect::<Vec<_>>(), ["Zebra", "Amber", "Evening"]);
	let rows = library.tree(true);
	assert_eq!(titles(&rows), ["Zebra", "Liked Songs", "Music", "Amber", "Evening", "Root"]);
	assert_eq!(rows[0].depth, 0);
	assert_eq!(rows[0].parent, Some("outer"), "projection retains native membership");
	library.toggle_folder("outer".into());
	assert_eq!(titles(&library.tree(true)), ["Zebra", "Liked Songs", "Music", "Root"]);
	assert_eq!(library.folder("outer").unwrap().count, 3, "pinning does not mutate native membership");
}

#[test]
fn filtering_reveals_matches_in_collapsed_folders_and_keeps_direct_folder_page_children() {
	let mut library = library();
	library.filter = "quiet".into();
	let rows = library.tree(true);
	assert_eq!(titles(&rows), ["Music", "Evening", "Quiet"]);
	assert!(!library.folder_expanded("outer"), "filtering must not overwrite saved expansion");
	assert_eq!(library.folder_entries("outer", true).iter().map(Entry::title).collect::<Vec<_>>(), ["Evening"]);
	library.filter = "music".into();
	assert_eq!(titles(&library.tree(true)), ["Music", "Zebra", "Evening", "Quiet", "Amber"]);
	library.kinds = vec![Kind::Album];
	assert!(library.tree(true).is_empty());
	assert_eq!(titles(&library.tree(false)), ["Music", "Root", "Liked Songs"]);
}

#[test]
fn folder_navigation_is_local_and_stale_account_responses_are_ignored() {
	let mut library = library();
	assert!(library.navigate(Route::Folder("outer".into())).is_empty());
	assert!(library.rows().is_empty());
	assert_eq!(library.ready_label(), "folder:outer");
	library.apply(Response::Rootlist { account: "bob".into(), entries: Vec::new() });
	assert!(library.folder("outer").is_some());
	library.apply(Response::FolderExpansion { account: "bob".into(), expanded: HashSet::from(["outer".into()]) });
	assert!(!library.folder_expanded("outer"));
	library.apply(Response::Connected { me: User { id: "bob".into(), display_name: None }, playlists: Vec::new() });
	assert!(library.rootlist.is_empty());
}

#[test]
fn expansion_is_idempotent_private_and_persisted_per_account() -> io::Result<()> {
	let config = tempfile::tempdir()?;
	assert!(load(config.path(), "alice")?.is_empty());
	set(config.path(), "alice", "outer", true)?;
	set(config.path(), "alice", "outer", true)?;
	assert_eq!(load(config.path(), "alice")?, HashSet::from(["outer".into()]));
	assert!(load(config.path(), "bob")?.is_empty());
	set(config.path(), "bob", "inner", true)?;
	set(config.path(), "alice", "outer", false)?;
	assert!(load(config.path(), "alice")?.is_empty());
	assert_eq!(load(config.path(), "bob")?, HashSet::from(["inner".into()]));
	Ok(())
}
