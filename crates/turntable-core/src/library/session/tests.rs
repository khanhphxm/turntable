use std::{collections::HashMap, sync::Arc};

use librespot_core::{Error, SpotifyId};
use librespot_protocol::{
	metadata,
	playlist4_external::{Item, ItemAttributes, ListAttributes, MetaItem},
};
use protobuf::MessageField;

use super::{
	CollectionItem, album, episode, image_models, liked_snapshot, local, page, releases, resolve_rows, root_playlist, track, window,
};

fn album_proto() -> metadata::Album {
	let mut artist = metadata::Artist::new();
	artist.set_gid(SpotifyId { id: 3 }.to_raw().to_vec());
	artist.set_name("Artist".into());
	let mut album = metadata::Album::new();
	album.set_gid(SpotifyId { id: 2 }.to_raw().to_vec());
	album.set_name("Album".into());
	album.artist.push(artist);
	album
}

#[test]
fn album_keeps_release_precision_discs_ep_and_copyrights() -> Result<(), Error> {
	let mut raw = album_proto();
	raw.set_type(metadata::album::Type::EP);
	raw.set_label("Test Records".into());
	let mut date = metadata::Date::new();
	date.set_year(2020);
	raw.date = MessageField::some(date);
	let mut disc = metadata::Disc::new();
	disc.track = vec![metadata::Track::new(), metadata::Track::new()];
	raw.disc.push(disc.clone());
	raw.disc.push(disc);
	let mut copyright = metadata::Copyright::new();
	copyright.set_type(metadata::copyright::Type::P);
	copyright.set_text("Label".into());
	raw.copyright.push(copyright);
	let converted = album(&raw)?;
	assert_eq!(converted.total_tracks, 4);
	assert_eq!(converted.album_type, "single");
	assert_eq!(converted.release_date.as_deref(), Some("2020"));
	assert_eq!(converted.copyrights.first().map(|c| c.line()), Some("℗ Label".into()));
	assert_eq!(converted.label.as_deref(), Some("Test Records"));
	if let Some(date) = raw.date.as_mut() {
		date.set_month(4);
	}
	assert_eq!(album(&raw)?.release_date.as_deref(), Some("2020-04"));
	if let Some(date) = raw.date.as_mut() {
		date.set_day(9);
	}
	assert_eq!(album(&raw)?.release_date.as_deref(), Some("2020-04-09"));
	Ok(())
}

#[test]
fn track_preserves_original_uri_and_complete_row_metadata() -> Result<(), Error> {
	let mut raw = metadata::Track::new();
	raw.set_gid(SpotifyId { id: 7 }.to_raw().to_vec());
	raw.set_name("Song".into());
	raw.set_duration(180_000);
	raw.set_explicit(true);
	raw.set_disc_number(2);
	let album = album_proto();
	raw.artist = album.artist.clone();
	raw.album = MessageField::some(album);
	let original = "spotify:track:0000000000000000000001";
	let row = track(original, &raw, None)?;
	assert_eq!(row.uri, original);
	assert_eq!(row.name, "Song");
	assert_eq!(row.duration_ms, 180_000);
	assert_eq!(row.disc_number, 2);
	assert!(row.explicit);
	assert_eq!(row.artists.first().map(|a| a.name.as_str()), Some("Artist"));
	assert_eq!(row.album.as_ref().map(|a| a.name.as_str()), Some("Album"));
	assert!(!row.is_episode && !row.is_local);
	raw.set_duration(-1);
	assert_eq!(track(original, &raw, None)?.duration_ms, 0);
	raw.set_disc_number(-1);
	assert_eq!(track(original, &raw, None)?.disc_number, 0);
	Ok(())
}

#[test]
fn local_files_and_episodes_stay_distinct_from_spotify_tracks() -> Result<(), Error> {
	let local = local("spotify:local:Artist:Album:Song:123");
	assert!(local.is_some());
	if let Some(row) = local {
		assert!(row.is_local && !row.is_episode);
		assert_eq!(row.duration_ms, 123_000);
		assert_eq!(row.name, "Song");
	}
	let mut raw = metadata::Episode::new();
	raw.set_name("Episode".into());
	raw.set_duration(120_000);
	let mut show = metadata::Show::new();
	show.set_gid(SpotifyId { id: 5 }.to_raw().to_vec());
	show.set_name("Show".into());
	raw.show = MessageField::some(show);
	let row = episode("spotify:episode:0000000000000000000001", &raw)?;
	assert!(row.is_episode && !row.is_local);
	assert!(row.artists.is_empty());
	assert_eq!(row.album.as_ref().map(|a| a.name.as_str()), Some("Show"));
	Ok(())
}

#[test]
fn metadata_images_use_spotify_file_id_and_expected_sizes() {
	let mut small = metadata::Image::new();
	small.set_file_id(vec![0xab; 20]);
	small.set_size(metadata::image::Size::SMALL);
	let mut sized = small.clone();
	sized.set_width(500);
	sized.set_height(400);
	let images = image_models(&[small, sized, metadata::Image::new()]);
	assert_eq!(images.len(), 2);
	assert_eq!(images.first().and_then(|image| image.width), Some(64));
	assert_eq!(images.first().map(|image| image.url.as_str()), Some("https://i.scdn.co/image/abababababababababababababababababababab"));
	assert_eq!(images.last().and_then(|image| image.height), Some(400));
}

#[test]
fn rootlist_keeps_followed_and_owned_playlists_but_not_folders_or_errors() {
	let mut attrs = ListAttributes::new();
	attrs.set_name("Playlist".into());
	attrs.set_collaborative(true);
	let mut meta = MetaItem::new();
	meta.attributes = MessageField::some(attrs);
	meta.set_owner_username("alice".into());
	meta.set_length(123);
	let uri = "spotify:user:alice:playlist:0000000000000000000001";
	let playlist = root_playlist(uri, &meta);
	assert!(playlist.is_some());
	if let Some(playlist) = playlist {
		assert_eq!(playlist.uri, "spotify:playlist:0000000000000000000001");
		assert_eq!(playlist.track_count, Some(123));
		assert!(playlist.collaborative);
	}
	assert!(root_playlist("spotify:start-group:folder", &meta).is_none());
	meta.set_status_code(404);
	assert!(root_playlist(uri, &meta).is_none());
}

#[test]
fn page_offsets_count_raw_entries_not_resolved_metadata_rows() {
	let raw: Vec<_> = (0..121).collect();
	assert_eq!(window(&raw, 100, 50).len(), 21);
	assert!(window(&raw, u32::MAX, 50).is_empty());
	let filtered = page(vec![1, 2], 50, 50, 121);
	assert_eq!(filtered.next_offset(), Some(100));
	assert_eq!(page(vec![1], 100, 50, 121).next_offset(), None);
}

#[test]
fn artist_discography_chooses_one_variant_per_release_in_server_order() -> Result<(), Error> {
	let album = album_proto();
	let mut variant = album.clone();
	variant.set_gid(SpotifyId { id: 9 }.to_raw().to_vec());
	let groups = [metadata::AlbumGroup { album: vec![album, variant], ..metadata::AlbumGroup::default() }, metadata::AlbumGroup::new()];
	assert_eq!(releases(&groups)?, ["spotify:album:0000000000000000000002"]);
	Ok(())
}

#[test]
fn metadata_answers_restore_playlist_order_keep_duplicates_and_drop_missing_entries() -> Result<(), Error> {
	let a = "spotify:track:0000000000000000000001";
	let b = "spotify:track:0000000000000000000002";
	let missing = "spotify:track:0000000000000000000003";
	let mut alpha = metadata::Track::new();
	alpha.set_name("Alpha".into());
	let mut beta = metadata::Track::new();
	beta.set_name("Beta".into());
	let raw = HashMap::from([(b.to_owned(), beta), (a.to_owned(), alpha)]);
	let uris = [a, missing, b, a].map(str::to_owned);
	let rows = resolve_rows(&uris, &raw, &HashMap::new(), None)?;
	assert_eq!(rows.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Alpha", "Beta", "Alpha"]);
	assert_eq!(rows.iter().map(|t| t.uri.as_str()).collect::<Vec<_>>(), [a, b, a]);
	Ok(())
}

#[test]
fn album_rows_use_the_full_page_album_without_parsing_sparse_embedded_albums() -> Result<(), Error> {
	let entity = "spotify:track:0000000000000000000001";
	let mut raw = metadata::Track::new();
	raw.set_name("Song".into());
	raw.album = MessageField::some(metadata::Album::new());
	let known = album(&album_proto())?;
	let row = track(entity, &raw, Some(&known))?;
	assert_eq!(row.album.as_ref(), Some(&known));
	Ok(())
}

#[test]
fn liked_snapshots_publish_changes_once_and_reconcile_removed_tracks() -> Result<(), Box<dyn std::error::Error>> {
	let one = "spotify:track:0000000000000000000001";
	let two = "spotify:track:0000000000000000000002";
	let items: Vec<_> = [two, one, one, "spotify:album:0000000000000000000003"]
		.into_iter()
		.map(|uri| CollectionItem { uri: uri.into(), ..CollectionItem::default() })
		.collect();
	let (mut cached, mut changed) = (None, true);
	let first = liked_snapshot(&items, &mut cached, &mut changed).ok_or("changed collection did not publish")?;
	assert_eq!(first.len(), 2);
	assert!(first.windows(2).all(|pair| pair.first() < pair.get(1)));
	assert!(liked_snapshot(&items, &mut cached, &mut changed).is_none());
	changed = true;
	let reused = liked_snapshot(&items, &mut cached, &mut changed).ok_or("cached snapshot did not publish")?;
	assert!(Arc::ptr_eq(&first, &reused));
	// A successful collection TTL refresh invalidates the old ID array, even if no
	// corresponding track happened to occur in the visible Liked Songs page.
	cached = None;
	changed = true;
	let empty = liked_snapshot(&[], &mut cached, &mut changed).ok_or("refreshed empty collection did not publish")?;
	assert!(empty.is_empty());
	assert!(liked_snapshot(&[], &mut cached, &mut changed).is_none());
	Ok(())
}

#[test]
fn playlist4_null_description_is_absent_but_normal_copy_is_preserved() {
	let mut attributes = ListAttributes::new();
	attributes.set_description("null".into());
	assert_eq!(super::playlist("scratch", "alex", &attributes, 0).description, None);
	attributes.set_description("A mix".into());
	assert_eq!(super::playlist("scratch", "alex", &attributes, 0).description.as_deref(), Some("A mix"));
}

#[test]
fn more_by_excludes_current_deduplicates_and_bounds_own_releases() {
	let groups = vec![
		("Albums", vec!["current".into(), "other".into(), "other".into()]),
		("Singles and EPs", (0..20).map(|n| format!("single-{n}")).collect()),
	];
	let uris = super::more_by_uris(&groups, "current");
	assert_eq!(uris.len(), 10);
	assert_eq!(uris[0], "other");
	assert_eq!(uris[1], "single-0");
	assert_eq!(uris[9], "single-8");
}

#[test]
fn paginated_album_keeps_full_duration_shelf_and_disc_numbers() -> Result<(), Error> {
	let mut album = album(&album_proto())?;
	album.total_tracks = 51;
	let mut raw = metadata::Track::new();
	raw.set_duration(100);
	raw.set_disc_number(1);
	let mut tracks = vec![track("spotify:track:one", &raw, Some(&album))?; 50];
	raw.set_disc_number(2);
	tracks.push(track("spotify:track:two", &raw, Some(&album))?);
	let more_by = vec![crate::library::Tile::release(&album)];
	let data = super::AlbumData { album, tracks, duration_ms: 5100, disc_count: 2, more_by: more_by.clone() };
	let first = data.listing(0, None);
	assert_eq!(first.tracks.len(), 50);
	assert_eq!(first.next_offset, Some(50));
	assert_eq!(first.duration_ms, Some(5100));
	assert_eq!(first.disc_count, 2, "disc 2 is known before its track page arrives");
	assert_eq!(first.more_by, more_by);
	assert!(first.tracks.iter().all(|track| track.disc_number == 1));
	assert_eq!(first.added_at(0), None, "albums have no per-occurrence added date");
	assert_eq!(first.added_by(0), None);
	let next = data.listing(50, None);
	assert_eq!(next.tracks.len(), 1);
	assert_eq!(next.tracks[0].disc_number, 2);
	assert_eq!(next.next_offset, None);
	assert_eq!(next.duration_ms, first.duration_ms);
	assert_eq!(next.disc_count, first.disc_count);
	assert_eq!(next.more_by, first.more_by);
	Ok(())
}

#[test]
fn playlist_added_attributes_survive_metadata_deduplication_and_unavailable_rows() -> Result<(), Error> {
	let a = "spotify:track:0000000000000000000001";
	let b = "spotify:track:0000000000000000000002";
	let missing = "spotify:track:0000000000000000000003";
	let entries: Vec<_> =
		[(a, Some(1_781_001_234_567), Some("alex")), (missing, Some(1), None), (a, Some(1_781_001_235_000), Some("bo")), (b, None, None)]
			.into_iter()
			.map(|(uri, timestamp, added_by)| Item {
				uri: Some(uri.into()),
				attributes: MessageField::some(ItemAttributes { timestamp, added_by: added_by.map(str::to_owned), ..Default::default() }),
				..Default::default()
			})
			.collect();
	let uris = entries.iter().map(|item| item.uri().to_owned()).collect::<Vec<_>>();
	let metadata = HashMap::from([(a.to_owned(), metadata::Track::new()), (b.to_owned(), metadata::Track::new())]);
	let rows = resolve_rows(&uris, &metadata, &HashMap::new(), None)?;
	let rows = super::added_page(rows, entries.iter().map(super::playlist::row_attributes), 0, 4);
	let mut listing = crate::library::Listing::default();
	super::super::pages::take_session_page(&mut listing, rows);
	assert_eq!(listing.tracks.iter().map(|row| row.uri.as_str()).collect::<Vec<_>>(), [a, a, b]);
	assert_eq!(listing.positions, [0, 2, 3]);
	assert_eq!(listing.added_at, [Some(1_781_001_234), Some(1_781_001_235), None]);
	assert_eq!(listing.added_by(0).as_deref(), Some("alex"));
	assert_eq!(listing.added_by(1).as_deref(), Some("bo"));
	assert_eq!(listing.added_by(2), None);
	Ok(())
}

#[test]
fn liked_added_attributes_keep_collection_seconds_and_skip_missing_metadata() -> Result<(), Error> {
	let a = "spotify:track:0000000000000000000001";
	let b = "spotify:track:0000000000000000000002";
	let items = [CollectionItem { uri: a.into(), added_at: 123, ..Default::default() }, CollectionItem {
		uri: b.into(),
		added_at: 456,
		..Default::default()
	}];
	let rows = vec![track(b, &metadata::Track::new(), None)?];
	let rows = super::added_page(rows, items.iter().map(|item| (item.uri.as_str(), Some(item.added_at), None)), 0, 2);
	assert_eq!(rows.added_at, [Some(456)]);
	assert_eq!(rows.added_by, [None]);
	assert_eq!(rows.positions, [1]);
	Ok(())
}
