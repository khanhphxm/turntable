//! Pathfinder search and account shelves, adapted to the same models as the Web API.
//! A missing union is an error (and triggers the caller's Web API fallback); an empty
//! `items` array is a successful empty result. Removed/unavailable entities are skipped.

use serde_json::Value;

use super::{WebPlayer, allowed, array, text, web_track};
use crate::spotify::{Album, Artist, Image, Page, Playlist, SearchResults, User, id_of};

const SEARCH: (&str, &str) = ("searchDesktop", "eef7cc54888d91bdd6802623477873caa3948ae173a0c34fd86827b267e94c03");
const SEARCH_LIMIT: u32 = 10;

impl WebPlayer {
	/// Tracks, artists, albums and playlists, with the Web API's page size and models.
	/// Uses the anonymous web-player token, so searching does not need playback to connect.
	/// Callers retain the Web API as fallback for disabled sources or rotated query hashes.
	pub fn search(&self, query: &str, offset: u32) -> Result<SearchResults, String> {
		allowed()?;
		let token = self.token()?;
		let variables = serde_json::json!({
			"searchTerm": query, "offset": offset, "limit": SEARCH_LIMIT, "numberOfTopResults": 5,
			"includeAudiobooks": false, "includePreReleases": true, "includeAlbumPreReleases": false,
			"includeAuthors": false, "includeEpisodeContentRatingsV2": false,
			"includeArtistHasConcertsField": false, "includeLocalConcertsField": false,
		});
		parse_search(&self.query(&token, SEARCH, &variables.to_string())?, offset)
	}
}

fn parse_search(json: &Value, offset: u32) -> Result<SearchResults, String> {
	let search = json.pointer("/data/searchV2").ok_or("no search results in the answer")?;
	Ok(SearchResults {
		tracks:    search_page(search, "tracksV2", "/item/data", offset, web_track)?,
		artists:   search_page(search, "artists", "/data", offset, web_artist)?,
		albums:    search_page(search, "albumsV2", "/data", offset, web_album)?,
		playlists: search_page(search, "playlists", "/data", offset, web_playlist)?,
	})
}

fn search_page<T>(search: &Value, key: &str, entity: &str, offset: u32, parse: fn(&Value) -> Option<T>) -> Result<Page<T>, String> {
	let section = search.get(key).ok_or_else(|| format!("no {key} in the search answer"))?;
	let total = section.get("totalCount").and_then(Value::as_u64).ok_or_else(|| format!("no total for {key}"))?;
	let items = section.get("items").and_then(Value::as_array).ok_or_else(|| format!("no items for {key}"))?;
	let total = u32::try_from(total).unwrap_or(u32::MAX);
	Ok(Page {
		items: items.iter().filter_map(|item| parse(item.pointer(entity)?)).collect(),
		total,
		offset,
		limit: SEARCH_LIMIT,
		// Like session pages, `next` marks another raw page; callers use `next_offset()`.
		next: (offset.saturating_add(SEARCH_LIMIT) < total).then(|| "web-player".to_owned()),
	})
}

fn images(value: &Value, path: &str) -> Vec<Image> {
	array(value, path)
		.iter()
		.filter_map(|source| {
			let dimension = |key: &str| source.get(key).and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok());
			Some(Image { url: text(source, "/url")?, width: dimension("width"), height: dimension("height") })
		})
		.collect()
}

fn web_artist(value: &Value) -> Option<Artist> {
	let uri = text(value, "/uri").filter(|uri| uri.starts_with("spotify:artist:"))?;
	Some(Artist { name: text(value, "/profile/name")?, uri, images: images(value, "/visuals/avatarImage/sources"), genres: Vec::new() })
}

fn album_artists(value: &Value) -> Vec<Artist> {
	array(value, "/artists/items").iter().filter_map(web_artist).collect()
}

fn web_album(value: &Value) -> Option<Album> {
	let uri = text(value, "/uri").filter(|uri| uri.starts_with("spotify:album:"))?;
	Some(Album {
		name: text(value, "/name")?,
		uri,
		images: images(value, "/coverArt/sources"),
		artists: album_artists(value),
		total_tracks: value.pointer("/tracks/totalCount").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).unwrap_or_default(),
		release_date: text(value, "/date/isoString")
			.or_else(|| value.pointer("/date/year").and_then(Value::as_u64).map(|year| year.to_string())),
		album_type: text(value, "/type").unwrap_or_default().to_ascii_lowercase(),
		copyrights: Vec::new(),
	})
}

fn web_playlist(value: &Value) -> Option<Playlist> {
	let uri = text(value, "/uri").filter(|uri| uri.starts_with("spotify:playlist:"))?;
	let owner = value.pointer("/ownerV2/data");
	Some(Playlist {
		id: id_of(&uri).to_owned(),
		uri,
		name: text(value, "/name")?,
		description: text(value, "/description")
			.map(|description| super::plain(&description))
			.filter(|description| !description.is_empty()),
		public: value.get("isPublic").and_then(Value::as_bool),
		owner: User {
			id:           owner.and_then(|owner| owner.get("uri")).and_then(Value::as_str).map(id_of).unwrap_or_default().to_owned(),
			display_name: owner.and_then(|owner| text(owner, "/name")),
		},
		images: images(value, "/images/items/0/sources"),
		collaborative: value.get("isCollaborative").and_then(Value::as_bool).unwrap_or_default(),
		track_count: value.pointer("/content/totalCount").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()),
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn search_maps_all_sections_and_keeps_raw_pagination() {
		let answer = serde_json::json!({"data":{"searchV2":{
			"tracksV2":{"totalCount":25,"items":[{"item":{"data":{"uri":"spotify:track:t","name":"Song","duration":{"totalMilliseconds":1234},"contentRating":{"label":"EXPLICIT"},"artists":{"items":[{"uri":"spotify:artist:a","profile":{"name":"Artist"}}]},"albumOfTrack":{"uri":"spotify:album:b","name":"Record","coverArt":{"sources":[{"url":"cover","width":300,"height":300}]}}}}},{"item":{"data":{"__typename":"NotFound"}}}]},
			"artists":{"totalCount":11,"items":[{"data":{"uri":"spotify:artist:a","profile":{"name":"Artist"},"visuals":{"avatarImage":{"sources":[{"url":"avatar","width":160,"height":160}]}}}}]},
			"albumsV2":{"totalCount":1,"items":[{"data":{"uri":"spotify:album:b","name":"Record","type":"ALBUM","date":{"year":1997},"artists":{"items":[{"uri":"spotify:artist:a","profile":{"name":"Artist"}}]},"coverArt":{"sources":[{"url":"cover"}]}}}]},
			"playlists":{"totalCount":1,"items":[null,{"data":{"__typename":"NotFound"}},{"data":{"uri":"spotify:playlist:p","name":"Mix","description":"Rock &amp; roll","ownerV2":{"data":{"uri":"spotify:user:u","name":"Owner"}},"images":{"items":[{"sources":[{"url":"playlist"}]}]}}}]}
		}}});
		let result = parse_search(&answer, 10).unwrap();
		assert_eq!((result.tracks.items.len(), result.tracks.total, result.tracks.next_offset()), (1, 25, Some(20)));
		assert_eq!(result.tracks.items[0].duration_ms, 1234);
		assert!(result.tracks.items[0].explicit);
		assert_eq!(result.tracks.items[0].album.as_ref().unwrap().images[0].url, "cover");
		assert_eq!(result.artists.items[0].images[0].url, "avatar");
		assert_eq!(result.artists.next_offset(), None);
		assert_eq!(result.albums.items[0].release_date.as_deref(), Some("1997"));
		assert_eq!(result.albums.items[0].artists[0].name, "Artist");
		assert_eq!(result.playlists.items[0].owner.id, "u");
		assert_eq!(result.playlists.items[0].description.as_deref(), Some("Rock & roll"));
		assert_eq!(result.playlists.items[0].images[0].url, "playlist");
	}

	#[test]
	fn empty_search_is_success_but_missing_unions_fall_back() {
		let mut answer = serde_json::json!({"data":{"searchV2":{
			"tracksV2":{"totalCount":0,"items":[]},"artists":{"totalCount":0,"items":[]},
			"albumsV2":{"totalCount":0,"items":[]},"playlists":{"totalCount":0,"items":[]}
		}}});
		assert!(parse_search(&answer, 0).unwrap().tracks.items.is_empty());
		answer["data"]["searchV2"]["artists"] = Value::Null;
		assert!(parse_search(&answer, 0).is_err());
		assert!(parse_search(&serde_json::json!({"errors":[{"message":"PersistedQueryNotFound"}]}), 0).is_err());
	}
}
