//! Personalised Home and Show all from Pathfinder, never the quota-limited Web API.
//! The persisted document contains both operations. Its hash was read from
//! `web-player.1eb7daa6.js` on 2026-10-13. An anonymous answer is not account data:
//! production only uses the attached playback Session's bearer, keeping cached Home
//! or the native history/collection shelves when Spotify rejects that client.
//!
//! Shows, episodes, chapters and audiobooks are omitted: the app has no navigable
//! spoken-word page. DJ, onboarding, ads, videos and unresolved entities are omitted
//! rather than linked to a fake route or given an unplayable play button.

use serde_json::{Value, json};

use super::{allowed, array, image, plain, query, text};
use crate::{
	engine::SessionReader,
	library::{Facet, HomeFeed, HomeItem, HomeSection, HomeStyle, Route, Tile},
	spotify::id_of,
};

const HASH: &str = "5fb7da7a03ee2776c0670856609cb85a189a3cecc36cc160b91cde79063b9cf1";
const ITEMS: u32 = 10;
const SECTION_ITEMS: u32 = 50;

/// The machine's IANA zone, matching the desktop client's resolved time zone.
/// An empty zone lets Spotify choose when the operating system cannot provide one.
pub fn time_zone() -> String {
	iana_time_zone::get_timezone().unwrap_or_default()
}

/// Session-authenticated Home; rejection is an error, never an anonymous replacement.
pub fn fetch(reader: &SessionReader, facet: Facet) -> Result<HomeFeed, String> {
	allowed()?;
	let (token, client_token) = credentials(reader)?;
	parse(&fetch_json(&token.access_token, Some(&client_token), facet, &time_zone())?)
}

/// Raw Home for the read-only diagnostic and captured parser fixtures.
/// Callers supply their bearer; this function never logs or persists it.
pub fn fetch_json(token: &str, client_token: Option<&str>, facet: Facet, zone: &str) -> Result<Value, String> {
	allowed()?;
	query(token, client_token, ("home", HASH), &variables(zone, facet, None).to_string())
}

/// A section's next page, using Spotify's raw offset even when unsupported items are skipped.
pub fn fetch_section(reader: &SessionReader, uri: &str, offset: u32) -> Result<(HomeSection, Option<u32>), String> {
	allowed()?;
	let (token, client_token) = credentials(reader)?;
	let json = query(
		&token.access_token,
		Some(&client_token),
		("homeSection", HASH),
		&variables(&time_zone(), Facet::All, Some((uri, offset))).to_string(),
	)?;
	parse_section_page(&json, uri, offset)
}

fn variables(zone: &str, facet: Facet, section: Option<(&str, u32)>) -> Value {
	match section {
		Some((uri, offset)) => json!({
			"timeZone": zone, "sp_t": "", "homeEndUserIntegration": "INTEGRATION_WEB_PLAYER",
			"includeHomeChapterVideoCards": false, "includeEpisodeContentRatingsV2": false,
			"uri": uri, "sectionItemsOffset": offset, "sectionItemsLimit": SECTION_ITEMS
		}),
		None => json!({
			"timeZone": zone, "sp_t": "", "homeEndUserIntegration": "INTEGRATION_WEB_PLAYER",
			"includeHomeChapterVideoCards": false, "includeEpisodeContentRatingsV2": false,
			"facet": facet.value(), "sectionItemsLimit": ITEMS
		}),
	}
}

fn credentials(reader: &SessionReader) -> Result<(librespot_core::token::Token, String), String> {
	reader
		.block_on(async {
			let token = reader.session().login5().auth_token().await?;
			let client = reader.session().spclient().client_token().await?;
			Ok::<_, librespot_core::Error>((token, client))
		})
		.map_err(|error| error.to_string())
}

/// Explicit anonymous diagnostic only; the application never presents this as account data.
pub fn anonymous_json(facet: Facet, zone: &str) -> Result<Value, String> {
	allowed()?;
	let web = super::WebPlayer::new();
	fetch_json(&web.token()?, None, facet, zone)
}

/// Preserve the server's ordering, not a locally ranked or deduplicated approximation.
pub fn parse(json: &Value) -> Result<HomeFeed, String> {
	let home = json
		.pointer("/data/home")
		.filter(|home| home.get("__typename").and_then(Value::as_str) == Some("HomeResponsePayload"))
		.ok_or_else(|| answer_error(json, "Spotify returned no Home feed"))?;
	let sections = home.pointer("/sectionContainer/sections/items").and_then(Value::as_array).ok_or("Spotify returned no Home sections")?;
	Ok(HomeFeed {
		greeting: text(home, "/greeting/transformedLabel").unwrap_or_default(),
		sections: sections.iter().filter_map(parse_section).collect(),
	})
}

fn parse_section_page(json: &Value, uri: &str, offset: u32) -> Result<(HomeSection, Option<u32>), String> {
	let raw = array(json, "/data/homeSections/sections")
		.iter()
		.find(|section| section.get("uri").and_then(Value::as_str) == Some(uri))
		.ok_or_else(|| answer_error(json, "Spotify returned no Home section"))?;
	let section = parse_section(raw).ok_or("This Home section has no supported destination")?;
	let next = raw
		.pointer("/sectionItems/pagingInfo/nextOffset")
		.and_then(Value::as_u64)
		.and_then(|next| u32::try_from(next).ok())
		.filter(|next| *next > offset);
	Ok((HomeSection { more: next.is_some(), ..section }, next))
}

fn answer_error(json: &Value, default: &str) -> String {
	text(json, "/errors/0/message").unwrap_or_else(|| default.to_owned())
}

fn parse_section(raw: &Value) -> Option<HomeSection> {
	let uri = text(raw, "/uri")?;
	let style = match raw.pointer("/data/__typename")?.as_str()? {
		"HomeShortsSectionData" => HomeStyle::Shortcuts,
		"HomeFeedBaselineSectionData" | "HomeSpotlightSectionData" => HomeStyle::Feed,
		"HomeGenericSectionData" | "HomeGridSectionData" | "HomeRecentlyPlayedSectionData" => HomeStyle::Shelf,
		_ => return None,
	};
	let (title, kicker) = if style == HomeStyle::Feed {
		(text(raw, "/data/title/transformedLabel").unwrap_or_default(), nonempty(text(raw, "/data/subtitle/transformedLabel")))
	} else {
		section_title(raw)
	};
	let items = array(raw, "/sectionItems/items")
		.iter()
		.filter_map(|raw| {
			let mut item = parse_item(raw)?;
			if style == HomeStyle::Feed {
				item.label = nonempty(Some(title.clone()));
				item.backdrop = backdrop(raw.pointer("/content/data")?);
			}
			Some(item)
		})
		.collect();
	Some(HomeSection { more: style == HomeStyle::Shelf && uri.starts_with("spotify:section:"), uri, title, kicker, style, items })
}

/// Desktop separates a localised template prefix (`Made For {0}`) from its personalised name.
fn section_title(raw: &Value) -> (String, Option<String>) {
	let title = text(raw, "/data/title/transformedLabel").unwrap_or_default();
	let prefix = raw
		.pointer("/data/title/translatedBaseText")
		.and_then(Value::as_str)
		.and_then(|base| base.split_once("{0}"))
		.map(|(prefix, _)| prefix);
	if let Some(prefix) = prefix.filter(|prefix| !prefix.is_empty())
		&& let Some(name) = title.strip_prefix(prefix)
	{
		return (name.trim().to_owned(), Some(prefix.trim().to_owned()));
	}
	(title, nonempty(text(raw, "/data/subtitle/transformedLabel")))
}

fn parse_item(raw: &Value) -> Option<HomeItem> {
	if raw.get("uri").and_then(Value::as_str).is_some_and(|uri| uri.ends_with(":collection") || uri == "spotify:collection:tracks") {
		return Some(HomeItem { tile: Tile::liked(), label: None, description: None, backdrop: None, color: None });
	}
	let content = raw.pointer("/content/data")?;
	let uri = content.get("uri").and_then(Value::as_str).or_else(|| raw.get("uri").and_then(Value::as_str))?;
	let kind = content.get("__typename").and_then(Value::as_str)?;
	let (route, title, sub, artwork) = match kind {
		"Playlist" => {
			let title = text(content, "/name")?;
			let liked = title == "Liked Songs"
				&& array(content, "/images/items/0/sources")
					.iter()
					.any(|source| source.get("url").and_then(Value::as_str).is_some_and(|url| url.contains("/liked-songs/")));
			let route = if liked { Route::Liked } else { Route::Playlist(id_of(uri).to_owned()) };
			let owner = text(content, "/ownerV2/data/name").unwrap_or_default();
			(route, title, by("Playlist", &owner), content.pointer("/images/items/0"))
		},
		"Album" => {
			let kind = match content.get("albumType").and_then(Value::as_str) {
				Some("SINGLE") => "Single",
				Some("COMPILATION") => "Compilation",
				_ => "Album",
			};
			(Route::Album(id_of(uri).to_owned()), text(content, "/name")?, by(kind, &artists(content)), content.get("coverArt"))
		},
		"Artist" => (
			Route::Artist(id_of(uri).to_owned()),
			text(content, "/profile/name")?,
			"Artist".into(),
			content.pointer("/visuals/avatarImage"),
		),
		"Track" => {
			let album = content.pointer("/albumOfTrack/uri")?.as_str()?;
			(
				Route::Album(id_of(album).to_owned()),
				text(content, "/name")?,
				by("Song", &artists(content)),
				content.pointer("/albumOfTrack/coverArt"),
			)
		},
		"LikedSongs" => (Route::Liked, "Liked Songs".into(), "Playlist".into(), content.get("image")),
		_ => return None,
	};
	let cover = artwork.and_then(|art| image(array(art, "/sources"), 300));
	let color = artwork
		.and_then(|art| art.pointer("/extractedColors/colorDark/hex"))
		.and_then(Value::as_str)
		.and_then(|hex| hex.strip_prefix('#'))
		.filter(|hex| hex.len() == 6)
		.and_then(|hex| u32::from_str_radix(hex, 16).ok());
	Some(HomeItem {
		tile: Tile {
			play: (route != Route::Liked && content.pointer("/playability/playable").and_then(Value::as_bool) != Some(false))
				.then(|| uri.to_owned()),
			route,
			title,
			sub,
			image: cover,
		},
		label: nonempty(text(raw, "/data/title/transformedLabel")),
		description: nonempty(text(content, "/description").map(|description| plain(&description))),
		backdrop: None,
		color,
	})
}

fn artwork(content: &Value) -> Option<&Value> {
	match content.get("__typename").and_then(Value::as_str)? {
		"Playlist" => content.pointer("/images/items/0"),
		"Artist" => content.pointer("/visuals/avatarImage"),
		"Track" => content.pointer("/albumOfTrack/coverArt"),
		"Album" => content.get("coverArt"),
		_ => None,
	}
}

/// Desktop large cards show the first preview track's artwork, then the entity cover
/// when no preview metadata is supplied. Keep it separate from the small title cover.
fn backdrop(content: &Value) -> Option<String> {
	let art =
		array(content, "/previewItems/items").iter().find_map(|preview| artwork(preview.get("data")?)).or_else(|| artwork(content))?;
	image(array(art, "/sources"), 1000)
}

fn artists(content: &Value) -> String {
	let mut names = String::new();
	for name in array(content, "/artists/items").iter().filter_map(|artist| artist.pointer("/profile/name").and_then(Value::as_str)) {
		if !names.is_empty() {
			names.push_str(", ");
		}
		names.push_str(name);
	}
	names
}

fn by(kind: &str, creator: &str) -> String {
	if creator.is_empty() { kind.to_owned() } else { format!("{kind} • {creator}") }
}

fn nonempty(text: Option<String>) -> Option<String> {
	text.filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests;
