//! The fixture is a trimmed live anonymous Home response, not evidence of account personalisation.

use serde_json::{Value, json};

use super::{backdrop, parse, parse_item, parse_section, parse_section_page, variables};
use crate::library::{Facet, HomeStyle, Route};

type Result = std::result::Result<(), Box<dyn std::error::Error>>;

fn fixture() -> std::result::Result<Value, serde_json::Error> {
	serde_json::from_str(include_str!("../../../../tests/fixtures/home.json"))
}

fn first_section() -> std::result::Result<Value, Box<dyn std::error::Error>> {
	fixture()?.pointer("/data/home/sectionContainer/sections/items/0").cloned().ok_or_else(|| "fixture has no first section".into())
}

#[test]
fn home_variables_keep_filter_and_show_all_operations_distinct() {
	let home = variables("Asia/Singapore", Facet::All, None);
	assert_eq!(home.get("facet"), Some(&json!("")));
	assert_eq!(home.get("sectionItemsLimit"), Some(&json!(10)));
	assert!(home.get("uri").is_none());
	let section = variables("Asia/Singapore", Facet::All, Some(("spotify:section:test", 17)));
	assert_eq!(section.get("uri"), Some(&json!("spotify:section:test")));
	assert_eq!(section.get("sectionItemsOffset"), Some(&json!(17)));
	assert_eq!(section.get("sectionItemsLimit"), Some(&json!(50)));
	assert!(section.get("facet").is_none());
	assert_eq!(section.get("homeEndUserIntegration"), Some(&json!("INTEGRATION_WEB_PLAYER")));
}

#[test]
fn captured_home_keeps_spotify_section_and_item_order() -> Result {
	let feed = parse(&fixture()?)?;
	assert_eq!(feed.greeting, "Good morning");
	assert_eq!(feed.sections.iter().map(|section| section.title.as_str()).collect::<Vec<_>>(), [
		"Trending songs",
		"Popular artists",
		"Popular albums and singles",
		"Popular radio",
		"Featured Charts"
	]);
	assert!(feed.sections.iter().all(|section| section.style == HomeStyle::Shelf && section.more && section.items.len() == 2));
	let song = feed.sections.first().and_then(|section| section.items.first()).ok_or("missing track")?;
	assert_eq!(song.tile.title, "pop it");
	assert_eq!(song.tile.route, Route::Album("5VRKZGvWEQZRRYooVjKQkv".into()));
	assert_eq!(song.tile.play.as_deref(), Some("spotify:track:3LVkyYg0PSo8XInTPKKhZM"));
	assert_eq!(song.tile.sub, "Song • Gunna");
	assert_eq!(song.color, Some(0x0095_6e66));
	let artist = feed.sections.get(1).and_then(|section| section.items.first()).ok_or("missing artist")?;
	assert_eq!(artist.tile.title, "Kendrick Lamar");
	assert_eq!(artist.tile.sub, "Artist");
	let playlist = feed.sections.last().and_then(|section| section.items.first()).ok_or("missing playlist")?;
	assert_eq!(playlist.tile.sub, "Playlist • Spotify");
	assert!(playlist.description.as_deref().is_some_and(|description| description.contains("Global")));
	assert_eq!(playlist.color, Some(0x0070_3090));
	Ok(())
}

#[test]
fn desktop_templates_shortcuts_and_feed_recsplanations() -> Result {
	let mut raw = first_section()?;
	*raw.pointer_mut("/data/title").ok_or("missing title")? =
		json!({"transformedLabel":"Made For khanh", "translatedBaseText":"Made For {0}"});
	let section = parse_section(&raw).ok_or("section did not parse")?;
	assert_eq!(section.title, "khanh");
	assert_eq!(section.kicker.as_deref(), Some("Made For"));
	*raw.pointer_mut("/data/__typename").ok_or("missing type")? = json!("HomeShortsSectionData");
	assert_eq!(parse_section(&raw).ok_or("shortcuts missing")?.style, HomeStyle::Shortcuts);
	*raw.pointer_mut("/data/__typename").ok_or("missing type")? = json!("HomeFeedBaselineSectionData");
	*raw.pointer_mut("/data/title").ok_or("missing title")? =
		json!({"transformedLabel":"For fans of keshi", "translatedBaseText":"For fans of {0}"});
	let section = parse_section(&raw).ok_or("feed missing")?;
	assert_eq!(section.style, HomeStyle::Feed);
	assert!(!section.more);
	let card = section.items.first().ok_or("feed item missing")?;
	assert_eq!(card.label.as_deref(), Some("For fans of keshi"));
	assert!(card.backdrop.as_deref().is_some_and(|url| url.contains("ab67616d0000b273")));
	Ok(())
}

#[test]
fn home_feed_backdrop_prefers_preview_album_art_over_the_title_cover() {
	let content = json!({"__typename":"Playlist", "images":{"items":[{"sources":[{"url":"cover","width":640}]}]},
		"previewItems":{"items":[{"__typename":"TrackResponseWrapper","data":{"__typename":"Track",
			"albumOfTrack":{"coverArt":{"sources":[{"url":"preview-backdrop","width":1000}]}}}}]}});
	assert_eq!(backdrop(&content).as_deref(), Some("preview-backdrop"));
}

#[test]
fn liked_songs_and_unroutable_spoken_word_never_get_fake_routes() -> Result {
	let liked = parse_item(&json!({"uri":"spotify:user:account:collection", "content":{"__typename":"UnknownType"}}))
		.ok_or("liked songs missing")?;
	assert_eq!(liked.tile.route, Route::Liked);
	assert_eq!(liked.tile.title, "Liked Songs");
	assert!(liked.tile.play.is_none());
	for kind in ["Episode", "Podcast", "Audiobook", "Chapter", "DJ", "NotFound"] {
		assert!(
			parse_item(
				&json!({"uri":"spotify:episode:episode", "content":{"data":{"__typename":kind,"uri":"spotify:episode:episode","name":"Unroutable"}}})
			)
			.is_none()
		);
	}
	Ok(())
}

#[test]
fn home_section_offsets_count_raw_items_not_only_mapped_items() -> Result {
	let mut raw = first_section()?;
	*raw.pointer_mut("/sectionItems/pagingInfo/nextOffset").ok_or("missing paging")? = json!(17);
	let uri = raw.get("uri").and_then(Value::as_str).ok_or("missing section URI")?.to_owned();
	let json = json!({"data":{"homeSections":{"__typename":"HomeSectionCollection","sections":[raw]}}});
	let (section, next) = parse_section_page(&json, &uri, 0)?;
	assert_eq!(section.items.len(), 2);
	assert_eq!(next, Some(17));
	assert!(section.more);
	let (_, next) = parse_section_page(&json, &uri, 17)?;
	assert_eq!(next, None, "a non-advancing cursor never issues a repeated page");
	assert!(parse_section_page(&json, "spotify:section:missing", 0).is_err());
	Ok(())
}

#[test]
fn captured_home_section_uses_spotifys_cursor_after_trimmed_fixture_items() -> Result {
	let captured: Value = serde_json::from_str(include_str!("../../../../tests/fixtures/home_section.json"))?;
	let uri = "spotify:section:0JQ5DAnM3wGh0gz1MXnu3C";
	let (section, next) = parse_section_page(&captured, uri, 0)?;
	assert_eq!(section.title, "Popular artists");
	assert_eq!(section.items.len(), 3);
	assert_eq!(next, Some(20));
	assert!(section.more);
	Ok(())
}

#[test]
fn singles_and_playlist_descriptions_keep_desktop_labels() -> Result {
	let captured = fixture()?;
	let mut album =
		captured.pointer("/data/home/sectionContainer/sections/items/2/sectionItems/items/0").cloned().ok_or("missing album")?;
	*album.pointer_mut("/content/data/albumType").ok_or("missing album type")? = json!("SINGLE");
	assert_eq!(parse_item(&album).ok_or("missing single")?.tile.sub, "Single • Morgan Wallen");
	let mut playlist =
		captured.pointer("/data/home/sectionContainer/sections/items/4/sectionItems/items/0").cloned().ok_or("missing playlist")?;
	*playlist.pointer_mut("/content/data/description").ok_or("missing description")? =
		json!("With <a href=\"spotify:artist:a\">A</a>, B &amp; more");
	assert_eq!(parse_item(&playlist).ok_or("playlist did not parse")?.description.as_deref(), Some("With A, B & more"));
	Ok(())
}

#[test]
fn service_errors_are_not_empty_successful_home_pages() {
	assert!(parse(&json!({"data":{"home":{"__typename":"GenericError"}}})).is_err());
	assert_eq!(parse(&json!({"errors":[{"message":"client not allowed"}]})), Err("client not allowed".into()));
}
