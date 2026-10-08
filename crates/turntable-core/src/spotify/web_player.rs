//! Spotify's web-player reads: search, account shelves and metadata the public Web API
//! no longer exposes (artist pages, play counts, playlists of any owner and credits).
//!
//! Public metadata uses the anonymous token carried by an embed page. Account shelves
//! use the playback session's authenticated token. Persisted queries go to the web
//! player's GraphQL endpoint ("pathfinder"), never the quota-limited Web API.
//! These private endpoints can change: library callers keep Web API reads as fallback.
//! Settings → Sources controls all web-player requests, including search and shelves.

mod browse;

use std::{
	collections::HashMap,
	sync::{Mutex, PoisonError},
	thread,
	time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;
use serde_json::Value;

use super::model::{Album, Artist, Image, Track};
use crate::net::Profile;

const BODY_LIMIT: u64 = 4 << 20;
const PROFILE: Profile = Profile {
	timeout:         Duration::from_secs(8),
	connect:         None,
	user_agent:      concat!("turntable/", env!("CARGO_PKG_VERSION"), " (", env!("CARGO_PKG_REPOSITORY"), ")"),
	status_as_error: true,
};
const EMBED: &str = "https://open.spotify.com/embed";
/// Any embed page carries an anonymous token: this one when a call has no page of its own.
const TOKEN_PAGE: &str = "artist/4Z8W4fKeB5YxbusRsdQVPb";
const PATHFINDER: &str = "https://api-partner.spotify.com/pathfinder/v1/query";
const CREDITS: &str = "https://spclient.wg.spotify.com/track-credits-view/v0/experimental";
/// The web player's persisted queries, from its JavaScript bundle (`"<op>","query","<hash>"`).
/// Spotify rotates these: a stale hash costs that part of a page only.
const ARTIST_OVERVIEW: (&str, &str) = ("queryArtistOverview", "9f8134ef565e78621f1e1793555bd6633c5ac144ae0f89604ed3ae3f80b3c8e6");
const ALBUM_TRACKS: (&str, &str) = ("queryAlbumTracks", "11c9cad9cb24c4f9e25ef9456f68621cfd6006a90f679e729c1d2f2fd70dfe16");
const PLAYLIST: (&str, &str) = ("fetchPlaylist", "8964e8eafb21aa992a7d951d256d83285c04be2105d209262901de70cb97584a");
const EXTRACTED_COLORS: (&str, &str) = ("fetchExtractedColors", "36e90fcaea00d47c695fce31874efeb2519b97d4cd0ee1abfb4f8dc9348596ea");
const NEXT_DATA: &str = r#"<script id="__NEXT_DATA__" type="application/json">"#;
/// A token's life is about half an hour; refresh this long before it ends.
const TOKEN_MARGIN_MS: u64 = 60_000;
/// Tracks asked per album request (albums have 100 at most, box sets more).
const ALBUM_TRACKS_LIMIT: u32 = 300;
/// Albums whose play counts and artists whose overviews are kept.
const KEPT: usize = 32;

/// One of an artist's popular tracks, whole: the overview carries all a track row shows
/// (not the album's name: no Popular row has an album column).
#[derive(Debug, Clone, PartialEq)]
pub struct Popular {
	pub track: Track,
	/// All-time plays on Spotify.
	pub plays: Option<u64>,
}

/// What the web player says about an artist beyond the Web API's name and genres.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ArtistOverview {
	pub monthly_listeners: Option<u64>,
	/// Spotify's ranking of artists by listeners (1 = most listened).
	pub world_rank:        Option<u32>,
	pub verified:          bool,
	/// The artist's own words, as plain text.
	pub biography:         Option<String>,
	/// Wide photo for headers, the square avatar, and more photos.
	pub header:            Option<String>,
	pub avatar:            Option<String>,
	pub gallery:           Vec<String>,
	pub cities:            Vec<City>,
	pub related:           Vec<Related>,
	pub pick:              Option<Pick>,
	pub discovered_on:     Vec<PlaylistRef>,
	/// The avatar's dominant color, `#rrggbb`.
	pub color:             Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct City {
	pub name:      String,
	pub country:   String,
	pub listeners: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Related {
	pub id:    String,
	pub name:  String,
	pub image: Option<String>,
}

/// What the artist pinned to their page: an album or playlist, with a comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
	pub uri:      String,
	pub title:    String,
	pub comment:  String,
	/// "Album • New Release".
	pub subtitle: String,
	pub image:    Option<String>,
}

/// A playlist in a shelf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistRef {
	pub id:    String,
	pub name:  String,
	pub owner: String,
	pub image: Option<String>,
}

pub struct ArtistPage {
	/// Spotify's popular tracks in its order.
	pub popular:  Vec<Popular>,
	pub overview: ArtistOverview,
}

/// One person on a track's credits, with what they did ("Composer • Lyricist").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credit {
	pub name:  String,
	/// The artist's id, when they have a page.
	pub id:    Option<String>,
	pub roles: String,
}

/// A page of a playlist of any owner.
#[derive(Debug, Clone)]
pub struct WebPlaylist {
	pub name:   String,
	pub owner:  String,
	pub image:  Option<String>,
	pub total:  u32,
	/// Items this page held, tracks or not (the next page starts after them).
	pub items:  u32,
	pub tracks: Vec<Track>,
}

#[derive(Default)]
pub struct WebPlayer {
	/// The anonymous token and when it ends (ms since the epoch).
	token:     Mutex<Option<(String, u64)>>,
	/// Held while an embed page is fetched for a token: callers needing one at once share it.
	fetching:  Mutex<()>,
	plays:     Mutex<HashMap<String, HashMap<String, u64>>>,
	overviews: Mutex<HashMap<String, ArtistOverview>>,
}

impl WebPlayer {
	pub fn new() -> Self {
		Self::default()
	}

	/// An artist's popular tracks as Spotify ranks them (ten at most) and its overview,
	/// both from one overview request.
	pub fn artist(&self, artist_id: &str) -> Result<ArtistPage, String> {
		allowed()?;
		let id = clean(artist_id);
		// The embed page lists the popular tracks in Spotify's order, which the overview's own list
		// lacks. With a token at hand both are asked at once; else the page gives the token first.
		let page = format!("artist/{id}");
		let (embed, json) = match self.cached_token() {
			Some(token) => thread::scope(|scope| {
				let embed = scope.spawn(|| self.embed(&page).ok());
				let json = self.query(&token, ARTIST_OVERVIEW, &overview_variables(&id));
				(embed.join().ok().flatten(), json)
			}),
			None => {
				let embed = self.embed(&page).ok();
				(embed, self.overview_json(&id))
			},
		};
		let tracks: Vec<String> = embed
			.as_ref()
			.map(|e| {
				e.props
					.page_props
					.state
					.data
					.entity
					.track_list
					.iter()
					.filter_map(|t| t.uri.strip_prefix("spotify:track:").map(str::to_owned))
					.collect()
			})
			.unwrap_or_default();
		let (overview, mut top) = parse_overview(&json?);
		self.keep_overview(&id, &overview);
		// Spotify's order is the embed page's; without it, the overview's own.
		let mut popular = Vec::with_capacity(top.len());
		for track_id in &tracks {
			if let Some(i) = top.iter().position(|p| p.track.uri.strip_prefix("spotify:track:") == Some(track_id)) {
				popular.push(top.remove(i));
			}
		}
		popular.extend(top);
		Ok(ArtistPage { popular, overview })
	}

	/// The artist's overview alone (the now-playing card), reused when `artist` fetched it.
	pub fn overview(&self, artist_id: &str) -> Result<ArtistOverview, String> {
		allowed()?;
		let id = clean(artist_id);
		if let Some(o) = self.overviews.lock().unwrap_or_else(PoisonError::into_inner).get(&id) {
			return Ok(o.clone());
		}
		let (overview, _) = parse_overview(&self.overview_json(&id)?);
		self.keep_overview(&id, &overview);
		Ok(overview)
	}

	fn keep_overview(&self, id: &str, overview: &ArtistOverview) {
		let mut kept = self.overviews.lock().unwrap_or_else(PoisonError::into_inner);
		if kept.len() >= KEPT {
			kept.clear();
		}
		kept.insert(id.to_owned(), overview.clone());
	}

	fn overview_json(&self, id: &str) -> Result<Value, String> {
		let token = self.token()?;
		self.query(&token, ARTIST_OVERVIEW, &overview_variables(id))
	}

	/// All-time plays of an album's tracks, by track id.
	pub fn album_plays(&self, album_id: &str) -> Result<HashMap<String, u64>, String> {
		allowed()?;
		let id = clean(album_id);
		if let Some(p) = self.plays.lock().unwrap_or_else(PoisonError::into_inner).get(&id) {
			return Ok(p.clone());
		}
		let token = self.token()?;
		let mut plays = HashMap::new();
		let mut offset = 0;
		loop {
			let json = self.query(
				&token,
				ALBUM_TRACKS,
				&format!(r#"{{"uri":"spotify:album:{id}","offset":{offset},"limit":{ALBUM_TRACKS_LIMIT}}}"#),
			)?;
			let page = json.pointer("/data/albumUnion/tracksV2").ok_or("no tracks in the album answer")?;
			let items = array(page, "/items");
			for item in items {
				if let (Some(uri), Some(n)) = (text(item, "/track/uri"), text(item, "/track/playcount").and_then(|n| n.parse().ok())) {
					plays.insert(uri.strip_prefix("spotify:track:").unwrap_or(&uri).to_owned(), n);
				}
			}
			offset += u32::try_from(items.len()).unwrap_or(u32::MAX);
			if items.is_empty() || u64::from(offset) >= page.pointer("/totalCount").and_then(Value::as_u64).unwrap_or(0) {
				break;
			}
		}
		let mut kept = self.plays.lock().unwrap_or_else(PoisonError::into_inner);
		if kept.len() >= KEPT {
			kept.clear();
		}
		kept.insert(id, plays.clone());
		Ok(plays)
	}

	/// A page of a playlist, whoever owns it (including Spotify's own).
	pub fn playlist(&self, playlist_id: &str, offset: u32, limit: u32) -> Result<WebPlaylist, String> {
		allowed()?;
		let id = clean(playlist_id);
		let token = self.token()?;
		let json = self.query(
			&token,
			PLAYLIST,
			&format!(r#"{{"uri":"spotify:playlist:{id}","offset":{offset},"limit":{limit},"enableWatchFeedEntrypoint":false}}"#),
		)?;
		let p = json.pointer("/data/playlistV2").filter(|p| p.get("name").is_some()).ok_or("no playlist in the answer")?;
		let items = array(p, "/content/items");
		Ok(WebPlaylist {
			name:   text(p, "/name").unwrap_or_default(),
			owner:  text(p, "/ownerV2/data/name").unwrap_or_default(),
			image:  sources(p, "/images/items/0/sources").and_then(|s| image(s, 300)),
			total:  p.pointer("/content/totalCount").and_then(Value::as_u64).map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX)),
			items:  u32::try_from(items.len()).unwrap_or(u32::MAX),
			tracks: items.iter().filter_map(|i| web_track(i.pointer("/itemV2/data")?)).collect(),
		})
	}

	/// The dark tone of a cover's colors (`#rrggbb`), for tinting a page; `None` when
	/// Spotify had no real colors for it.
	pub fn colors(&self, image_url: &str) -> Option<String> {
		allowed().ok()?;
		let token = self.token().ok()?;
		let variables = serde_json::json!({ "imageUris": [image_url] }).to_string();
		let json = self.query(&token, EXTRACTED_COLORS, &variables).ok()?;
		let dark = json.pointer("/data/extractedColors/0/colorDark")?;
		if dark.get("isFallback").and_then(Value::as_bool).unwrap_or(true) {
			return None;
		}
		hex(dark.get("hex")?.as_str()?)
	}

	/// Who made a track, merged across the credit groups (a writer who also produced is one line).
	pub fn credits(&self, track_id: &str) -> Result<Vec<Credit>, String> {
		allowed()?;
		let token = self.token()?;
		let span = crate::net::trace::start();
		let url = format!("{CREDITS}/{}/credits", clean(track_id));
		let fetched: Result<Vec<u8>, String> = PROFILE
			.get(&url)
			.header("Authorization", format!("Bearer {token}"))
			.header("App-Platform", "WebPlayer")
			.call()
			.map_err(|e| e.to_string())
			.and_then(|mut r| r.body_mut().with_config().limit(BODY_LIMIT).read_to_vec().map_err(|e| e.to_string()));
		span.done("web", "GET", &url, crate::net::trace::Reply {
			status: if fetched.is_ok() { 200 } else { 0 },
			bytes: fetched.as_ref().map_or(0, Vec::len),
			..crate::net::trace::Reply::default()
		});
		let json: Value = serde_json::from_slice(&fetched?).map_err(|e| e.to_string())?;
		Ok(parse_credits(&json))
	}

	/// The anonymous token, from an embed page when none is valid. Callers that need one at the same
	/// time wait for a single embed page instead of each fetching its own.
	fn token(&self) -> Result<String, String> {
		if let Some(token) = self.cached_token() {
			return Ok(token);
		}
		let _fetching = self.fetching.lock().unwrap_or_else(PoisonError::into_inner);
		match self.cached_token() {
			Some(token) => Ok(token),
			None => Ok(self.embed(TOKEN_PAGE)?.props.page_props.state.settings.session.access_token),
		}
	}

	fn cached_token(&self) -> Option<String> {
		let kept = self.token.lock().unwrap_or_else(PoisonError::into_inner);
		kept.as_ref().filter(|(_, until)| *until > now_ms() + TOKEN_MARGIN_MS).map(|(token, _)| token.clone())
	}

	/// Fetches an embed page (`artist/<id>`), keeping the token it carries.
	fn embed(&self, path: &str) -> Result<NextData, String> {
		let span = crate::net::trace::start();
		let url = format!("{EMBED}/{path}");
		let fetched: Result<String, String> = PROFILE
			.get(&url)
			.call()
			.map_err(|e| e.to_string())
			.and_then(|mut r| r.body_mut().with_config().limit(BODY_LIMIT).read_to_string().map_err(|e| e.to_string()));
		span.done("web", "GET", &url, crate::net::trace::Reply {
			status: if fetched.is_ok() { 200 } else { 0 },
			bytes: fetched.as_ref().map_or(0, String::len),
			..crate::net::trace::Reply::default()
		});
		let html = fetched?;
		let data = parse_embed(&html)?;
		let session = &data.props.page_props.state.settings.session;
		let until = session.expires_ms.unwrap_or_else(|| now_ms() + 10 * 60_000);
		*self.token.lock().unwrap_or_else(PoisonError::into_inner) = Some((session.access_token.clone(), until));
		Ok(data)
	}

	/// One persisted query. A refused token is dropped, so the next call gets a new one.
	fn query(&self, token: &str, (op, hash): (&str, &str), variables: &str) -> Result<Value, String> {
		let extensions = format!(r#"{{"persistedQuery":{{"version":1,"sha256Hash":"{hash}"}}}}"#);
		let span = crate::net::trace::start();
		let fetched: Result<Vec<u8>, String> = PROFILE
			.get(PATHFINDER)
			.header("Authorization", format!("Bearer {token}"))
			.header("App-Platform", "WebPlayer")
			.query("operationName", op)
			.query("variables", variables)
			.query("extensions", &extensions)
			.call()
			.map_err(|e| e.to_string())
			.and_then(|mut r| r.body_mut().with_config().limit(BODY_LIMIT).read_to_vec().map_err(|e| e.to_string()));
		span.done("web", "GET", &format!("{PATHFINDER}/{op}"), crate::net::trace::Reply {
			status: if fetched.is_ok() { 200 } else { 0 },
			bytes: fetched.as_ref().map_or(0, Vec::len),
			..crate::net::trace::Reply::default()
		});
		let answer: Result<Value, String> = fetched.and_then(|bytes| serde_json::from_slice(&bytes).map_err(|e| e.to_string()));
		if answer.is_err() {
			*self.token.lock().unwrap_or_else(PoisonError::into_inner) = None;
		}
		answer
	}
}

/// Spotify's web player is asked only while the user allows it (Settings, "Data sources").
fn allowed() -> Result<(), String> {
	if crate::app::settings::allowed::web_player() { Ok(()) } else { Err("Spotify's web player is turned off in Settings".to_owned()) }
}

fn overview_variables(id: &str) -> String {
	format!(r#"{{"uri":"spotify:artist:{id}","locale":"","includePrerelease":false}}"#)
}

fn now_ms() -> u64 {
	SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// An id safe to put in a URL or a query's JSON.
fn clean(id: &str) -> String {
	id.chars().filter(char::is_ascii_alphanumeric).collect()
}

// ------------------------------------------------------------ JSON helpers

fn text(v: &Value, path: &str) -> Option<String> {
	v.pointer(path)?.as_str().map(str::to_owned)
}

fn array<'a>(v: &'a Value, path: &str) -> &'a [Value] {
	v.pointer(path).and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

fn sources<'a>(v: &'a Value, path: &str) -> Option<&'a [Value]> {
	v.pointer(path)?.as_array().map(Vec::as_slice)
}

fn width(source: &Value) -> Option<u64> {
	source.get("width").or_else(|| source.get("maxWidth")).and_then(Value::as_u64)
}

/// The smallest image at least `want` px wide, else the widest; sizeless lists give their first.
fn image(sources: &[Value], want: u64) -> Option<String> {
	let sized = sources.iter().filter_map(|s| Some((width(s)?, s)));
	let best = sized.clone().filter(|(w, _)| *w >= want).min_by_key(|(w, _)| *w).or_else(|| sized.max_by_key(|(w, _)| *w)).map(|(_, s)| s);
	best.or_else(|| sources.first()).and_then(|s| text(s, "/url"))
}

/// `#rrggbb` as sent (case folded), or `None` for anything else.
fn hex(s: &str) -> Option<String> {
	let digits = s.strip_prefix('#')?;
	(digits.len() == 6 && digits.chars().all(|c| c.is_ascii_hexdigit())).then(|| s.to_ascii_lowercase())
}

/// Text with Spotify's links and entities taken out ("<a href=…>Bowie</a> &amp; co").
fn plain(html: &str) -> String {
	let mut out = String::with_capacity(html.len());
	let mut in_tag = false;
	for c in html.chars() {
		match c {
			'<' => in_tag = true,
			'>' if in_tag => in_tag = false,
			c if !in_tag => out.push(c),
			_ => {},
		}
	}
	decode_entities(out.trim())
}

/// HTML character references: the named ones Spotify uses, and every numeric one
/// (`&#39;`, `&#x1f5a4;` → 🖤). Anything unrecognised stays as written.
fn decode_entities(s: &str) -> String {
	let mut out = String::with_capacity(s.len());
	let mut rest = s;
	while let Some((before, after)) = rest.split_once('&') {
		out.push_str(before);
		rest = after;
		let decoded = rest.split_once(';').filter(|(entity, _)| entity.len() <= 11).and_then(|(entity, after)| {
			let c = match entity {
				"amp" => '&',
				"lt" => '<',
				"gt" => '>',
				"quot" => '"',
				"apos" => '\'',
				"nbsp" => ' ',
				n => {
					let n = n.strip_prefix('#')?;
					let code = match n.strip_prefix(['x', 'X']) {
						Some(hex) => u32::from_str_radix(hex, 16).ok()?,
						None => n.parse().ok()?,
					};
					char::from_u32(code)?
				},
			};
			Some((c, after))
		});
		match decoded {
			Some((c, after)) => {
				out.push(c);
				rest = after;
			},
			None => out.push('&'),
		}
	}
	out.push_str(rest);
	out
}

fn id_of(uri: &str) -> String {
	uri.rsplit(':').next().unwrap_or_default().to_owned()
}

// ----------------------------------------------------------------- parsing

fn parse_overview(json: &Value) -> (ArtistOverview, Vec<Popular>) {
	let Some(a) = json.pointer("/data/artistUnion") else {
		return (ArtistOverview::default(), Vec::new());
	};
	let top = array(a, "/discography/topTracks/items")
		.iter()
		.filter_map(|i| {
			Some(Popular { track: web_track(i.get("track")?)?, plays: text(i, "/track/playcount").and_then(|n| n.parse().ok()) })
		})
		.collect();
	let overview = ArtistOverview {
		monthly_listeners: a.pointer("/stats/monthlyListeners").and_then(Value::as_u64).filter(|n| *n > 0),
		world_rank:        a.pointer("/stats/worldRank").and_then(Value::as_u64).filter(|n| *n > 0).and_then(|n| u32::try_from(n).ok()),
		verified:          a.pointer("/onPlatformReputationTrait/verification/isVerified").and_then(Value::as_bool).unwrap_or(false),
		biography:         text(a, "/profile/biography/text").map(|b| plain(&b)).filter(|b| !b.is_empty()),
		header:            sources(a, "/headerImage/data/sources").and_then(|s| image(s, 1200)),
		avatar:            sources(a, "/visuals/avatarImage/sources").and_then(|s| image(s, 640)),
		gallery:           array(a, "/visuals/gallery/items").iter().filter_map(|i| image(sources(i, "/sources")?, 640)).take(12).collect(),
		cities:            array(a, "/stats/topCities/items")
			.iter()
			.filter_map(|c| {
				Some(City {
					name:      text(c, "/city")?,
					country:   text(c, "/country").unwrap_or_default(),
					listeners: c.pointer("/numberOfListeners")?.as_u64()?,
				})
			})
			.collect(),
		related:           array(a, "/relatedContent/relatedArtists/items")
			.iter()
			.filter_map(|r| {
				Some(Related {
					id:    id_of(&text(r, "/uri")?),
					name:  text(r, "/profile/name")?,
					image: sources(r, "/visuals/avatarImage/sources").and_then(|s| image(s, 300)),
				})
			})
			.collect(),
		pick:              a.pointer("/profile/pinnedItem").and_then(parse_pick),
		discovered_on:     array(a, "/relatedContent/discoveredOnV2/items")
			.iter()
			.filter_map(|i| {
				let d = i.get("data")?;
				(text(d, "/__typename")? == "Playlist").then_some(())?;
				Some(PlaylistRef {
					id:    id_of(&text(d, "/uri")?),
					name:  text(d, "/name")?,
					owner: text(d, "/ownerV2/data/name").unwrap_or_default(),
					image: sources(d, "/images/items/0/sources").and_then(|s| image(s, 300)),
				})
			})
			.collect(),
		color:             text(a, "/visuals/avatarImage/extractedColors/colorRaw/hex").and_then(|h| hex(&h)),
	};
	(overview, top)
}

fn parse_pick(p: &Value) -> Option<Pick> {
	let item = p.pointer("/itemV2/data")?;
	let uri = text(item, "/uri")?;
	let image = sources(item, "/coverArt/sources")
		.or_else(|| sources(item, "/images/items/0/sources"))
		.and_then(|s| image(s, 300))
		.or_else(|| sources(p, "/thumbnailImage/data/sources").and_then(|s| s.last()).and_then(|s| text(s, "/url")));
	Some(Pick {
		title: text(p, "/title").or_else(|| text(item, "/name"))?,
		comment: text(p, "/comment").unwrap_or_default(),
		subtitle: text(p, "/subtitle").unwrap_or_default(),
		uri,
		image,
	})
}

/// A track of a playlist page or an artist's top tracks as the Web API would give it
/// (`None` for anything else: episodes, removed items).
fn web_track(t: &Value) -> Option<Track> {
	if !text(t, "/uri")?.starts_with("spotify:track:") {
		return None;
	}
	let images = |path: &str| -> Vec<Image> {
		array(t, path)
			.iter()
			.filter_map(|s| {
				let n = |k: &str| s.get(k).and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok());
				let url = text(s, "/url")?;
				// The overview lists covers without sizes; their ids tell (`…1e02…` 300 px, `…4851…` 64, `…b273…` 640).
				let side = n("width").or(match url.rsplit('/').next().and_then(|id| id.get(12..16)) {
					Some("1e02") => Some(300),
					Some("4851") => Some(64),
					Some("b273") => Some(640),
					_ => None,
				});
				Some(Image { height: n("height").or(side), width: side, url })
			})
			.collect()
	};
	let album = Album {
		name:         text(t, "/albumOfTrack/name").unwrap_or_default(),
		uri:          text(t, "/albumOfTrack/uri").unwrap_or_default(),
		images:       images("/albumOfTrack/coverArt/sources"),
		artists:      Vec::new(),
		total_tracks: 0,
		release_date: text(t, "/albumOfTrack/date/isoString"),
		album_type:   String::new(),
		copyrights:   Vec::new(),
	};
	Some(Track {
		uri:         text(t, "/uri")?,
		name:        text(t, "/name")?,
		duration_ms: t
			.pointer("/trackDuration/totalMilliseconds")
			.or_else(|| t.pointer("/duration/totalMilliseconds"))
			.and_then(Value::as_u64)
			.and_then(|n| u32::try_from(n).ok())
			.unwrap_or(0),
		explicit:    text(t, "/contentRating/label").as_deref() == Some("EXPLICIT"),
		artists:     array(t, "/artists/items")
			.iter()
			.filter_map(|a| {
				Some(Artist {
					name:   text(a, "/profile/name")?,
					uri:    text(a, "/uri").unwrap_or_default(),
					images: Vec::new(),
					genres: Vec::new(),
				})
			})
			.collect(),
		album:       Some(album),
		is_local:    false,
		is_episode:  false,
	})
}

fn parse_credits(json: &Value) -> Vec<Credit> {
	let mut out: Vec<(Credit, Vec<String>)> = Vec::new();
	for group in array(json, "/roleCredits").iter().chain(array(json, "/extendedCredits")) {
		// A group without sub-roles names the role itself ("Writers" → "Writer").
		let fallback = text(group, "/roleTitle").map(|t| t.strip_suffix('s').unwrap_or(&t).to_owned()).unwrap_or_default();
		for a in array(group, "/artists") {
			let (Some(name), id) = (text(a, "/name"), text(a, "/uri").map(|u| id_of(&u)).filter(|i| !i.is_empty())) else { continue };
			let mut roles: Vec<String> = array(a, "/subroles").iter().filter_map(Value::as_str).map(title_case).collect();
			if roles.is_empty() && !fallback.is_empty() {
				roles.push(fallback.clone());
			}
			match out.iter_mut().find(|(c, _)| (id.is_some() && c.id == id) || (id.is_none() && c.id.is_none() && c.name == name)) {
				Some((_, have)) => {
					for r in roles {
						if !have.contains(&r) {
							have.push(r);
						}
					}
				},
				None => out.push((Credit { name, id, roles: String::new() }, roles)),
			}
		}
	}
	out.into_iter().map(|(c, roles)| Credit { roles: roles.join(" • "), ..c }).collect()
}

/// "main artist" → "Main Artist".
fn title_case(s: &str) -> String {
	s.split_whitespace()
		.map(|w| {
			let mut c = w.chars();
			c.next().map_or_else(String::new, |f| f.to_uppercase().chain(c.flat_map(char::to_lowercase)).collect())
		})
		.collect::<Vec<_>>()
		.join(" ")
}

/// The Next.js state an embed page is rendered from.
fn parse_embed(html: &str) -> Result<NextData, String> {
	let (_, data) = html.split_once(NEXT_DATA).ok_or("no page data in the embed page")?;
	let (data, _) = data.split_once("</script>").ok_or("unterminated page data")?;
	serde_json::from_str(data).map_err(|e| e.to_string())
}

#[derive(Deserialize)]
struct NextData {
	props: Props,
}

#[derive(Deserialize)]
struct Props {
	#[serde(rename = "pageProps")]
	page_props: PageProps,
}

#[derive(Deserialize)]
struct PageProps {
	state: State,
}

#[derive(Deserialize)]
struct State {
	data:     Data,
	settings: Settings,
}

#[derive(Deserialize)]
struct Data {
	entity: Entity,
}

#[derive(Deserialize)]
struct Entity {
	#[serde(rename = "trackList", default)]
	track_list: Vec<Uri>,
}

#[derive(Deserialize)]
struct Uri {
	uri: String,
}

#[derive(Deserialize)]
struct Settings {
	session: Session,
}

#[derive(Deserialize)]
struct Session {
	#[serde(rename = "accessToken")]
	access_token: String,
	#[serde(rename = "accessTokenExpirationTimestampMs")]
	expires_ms:   Option<u64>,
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn embed_page_yields_ranked_tracks_and_token() {
		let html = format!(
			r#"<html><body>{NEXT_DATA}{{"props":{{"pageProps":{{"state":{{"data":{{"entity":{{"name":"X","trackList":[{{"uri":"spotify:track:aaa","title":"A"}},{{"uri":"spotify:episode:bbb"}},{{"uri":"spotify:track:ccc"}}]}}}},"settings":{{"session":{{"accessToken":"tok","accessTokenExpirationTimestampMs":1791836064500,"isAnonymous":true}}}}}}}}}}}}</script></body></html>"#
		);
		let d = parse_embed(&html).unwrap();
		assert_eq!(d.props.page_props.state.settings.session.access_token, "tok");
		assert_eq!(d.props.page_props.state.settings.session.expires_ms, Some(1_791_836_064_500));
		let uris: Vec<_> = d.props.page_props.state.data.entity.track_list.iter().map(|t| t.uri.as_str()).collect();
		assert_eq!(uris, ["spotify:track:aaa", "spotify:episode:bbb", "spotify:track:ccc"]);
	}

	#[test]
	fn embed_page_without_data_is_an_error() {
		assert!(parse_embed("<html>rate limited</html>").is_err());
	}

	fn json(s: &str) -> Value {
		serde_json::from_str(s).unwrap()
	}

	#[test]
	fn overview_gives_stats_images_and_shelves() {
		let o = json(
			r##"{"data":{"artistUnion":{
			"stats":{"monthlyListeners":46860023,"worldRank":84,"topCities":{"items":[{"city":"London","country":"GB","numberOfListeners":740990}]}},
			"onPlatformReputationTrait":{"verification":{"isVerified":true}},
			"profile":{"biography":{"text":"Rock <a href=\"spotify:artist:x\">band</a> &amp; more"},
			  "pinnedItem":{"title":"Pick","comment":"Hi","subtitle":"Album","itemV2":{"data":{"uri":"spotify:album:abc","coverArt":{"sources":[{"url":"u64","width":64},{"url":"u640","width":640},{"url":"u300","width":300}]}}}}},
			"headerImage":{"data":{"sources":[{"url":"h1","maxWidth":1494},{"url":"h2","maxWidth":1920}]}},
			"visuals":{"avatarImage":{"sources":[{"url":"a160","width":160},{"url":"a640","width":640}],"extractedColors":{"colorRaw":{"hex":"#C84000"}}},"gallery":{"items":[{"sources":[{"url":"g","width":640}]}]}},
			"relatedContent":{"relatedArtists":{"items":[{"uri":"spotify:artist:r1","profile":{"name":"Rel"},"visuals":{"avatarImage":{"sources":[{"url":"r","width":640}]}}}]},
			  "discoveredOnV2":{"items":[{"data":{"__typename":"GenericError"}},{"data":{"__typename":"Playlist","uri":"spotify:playlist:p1","name":"P","ownerV2":{"data":{"name":"Spotify"}},"images":{"items":[{"sources":[{"url":"pi","width":null}]}]}}}]}},
			"discography":{"topTracks":{"items":[{"track":{"uri":"spotify:track:t1","name":"Creep","playcount":"2617505374","duration":{"totalMilliseconds":238640},"artists":{"items":[{"profile":{"name":"Radiohead"},"uri":"spotify:artist:r"}]},"albumOfTrack":{"uri":"spotify:album:p","coverArt":{"sources":[{"url":"https://i.scdn.co/image/ab67616d00001e02ec548c00d3ac2f10be73366d"}]}}}}]}}}}}"##,
		);
		let (v, top) = parse_overview(&o);
		assert_eq!((v.monthly_listeners, v.world_rank, v.verified), (Some(46_860_023), Some(84), true));
		assert_eq!(v.biography.as_deref(), Some("Rock band & more"));
		assert_eq!(
			(v.header.as_deref(), v.avatar.as_deref(), v.gallery.as_slice()),
			(Some("h1"), Some("a640"), ["g".to_owned()].as_slice())
		);
		assert_eq!(v.cities, [City { name: "London".into(), country: "GB".into(), listeners: 740_990 }]);
		assert_eq!(v.related, [Related { id: "r1".into(), name: "Rel".into(), image: Some("r".into()) }]);
		assert_eq!(
			v.pick,
			Some(Pick {
				uri:      "spotify:album:abc".into(),
				title:    "Pick".into(),
				comment:  "Hi".into(),
				subtitle: "Album".into(),
				image:    Some("u300".into()),
			})
		);
		assert_eq!(v.discovered_on, [PlaylistRef {
			id:    "p1".into(),
			name:  "P".into(),
			owner: "Spotify".into(),
			image: Some("pi".into()),
		}]);
		assert_eq!(v.color.as_deref(), Some("#c84000"));
		assert_eq!((top[0].track.name.as_str(), top[0].plays), ("Creep", Some(2_617_505_374)));
		assert_eq!(top[0].track.artists[0].name, "Radiohead");
		assert_eq!(top[0].track.album.as_ref().unwrap().images[0].width, Some(300));
	}

	#[test]
	fn overview_without_data_is_empty() {
		let (v, top) = parse_overview(&json(r#"{"errors":[]}"#));
		assert_eq!(v, ArtistOverview::default());
		assert!(top.is_empty());
	}

	#[test]
	fn credits_merge_roles_of_one_person() {
		let c = json(
			r#"{"roleCredits":[
			{"roleTitle":"Performers","artists":[{"uri":"spotify:artist:a","name":"Ann","subroles":["main artist"]}]},
			{"roleTitle":"Writers","artists":[{"uri":"spotify:artist:a","name":"Ann","subroles":["composer","lyricist"]},{"uri":"spotify:artist:b","name":"Bob","subroles":[]}]},
			{"roleTitle":"Producers","artists":[]}],"extendedCredits":[]}"#,
		);
		assert_eq!(parse_credits(&c), [
			Credit { name: "Ann".into(), id: Some("a".into()), roles: "Main Artist • Composer • Lyricist".into() },
			Credit { name: "Bob".into(), id: Some("b".into()), roles: "Writer".into() },
		]);
	}

	#[test]
	fn playlist_track_becomes_a_track_and_episodes_are_skipped() {
		let t = json(
			r#"{"uri":"spotify:track:x","name":"Creep","trackDuration":{"totalMilliseconds":238640},"contentRating":{"label":"EXPLICIT"},
			"artists":{"items":[{"profile":{"name":"Radiohead"},"uri":"spotify:artist:r"}]},
			"albumOfTrack":{"name":"Pablo Honey","uri":"spotify:album:p","coverArt":{"sources":[{"url":"c","width":300,"height":300}]}}}"#,
		);
		let track = web_track(&t).unwrap();
		assert_eq!((track.name.as_str(), track.duration_ms, track.explicit), ("Creep", 238_640, true));
		assert_eq!(track.artists[0].name, "Radiohead");
		let album = track.album.unwrap();
		assert_eq!((album.name.as_str(), album.images[0].width), ("Pablo Honey", Some(300)));
		assert!(web_track(&json(r#"{"__typename":"Episode","uri":"spotify:episode:e","name":"E"}"#)).is_none());
		assert!(web_track(&json(r#"{"__typename":"NotFound"}"#)).is_none());
	}

	#[test]
	fn colors_must_be_plain_hex() {
		assert_eq!(hex("#A16B05").as_deref(), Some("#a16b05"));
		assert_eq!(hex("red"), None);
		assert_eq!(hex("#12345"), None);
	}

	#[test]
	fn the_web_player_is_not_asked_for_anything_while_it_is_off() {
		use crate::app::settings::{Sources, allowed};
		let off = Sources { lyrics: true, wikipedia: true, web_player: false };
		allowed::only_with(off, || {
			// Every public call answers before it asks for a token or a page.
			let web = WebPlayer::new();
			assert!(web.artist("4Z8W4fKeB5YxbusRsdQVPb").is_err());
			assert!(web.overview("4Z8W4fKeB5YxbusRsdQVPb").is_err());
			assert!(web.album_plays("4aawyAB9vmqN3uQ7FjRGTy").is_err());
			assert!(web.playlist("37i9dQZF1DXcBWIGoYBM5M", 0, 50).is_err());
			assert!(web.credits("2JRo0gjbX4GrCqBYdRohoo").is_err());
			assert!(web.search("radiohead", 0).is_err());
			assert_eq!(web.colors("https://i.scdn.co/image/ab67616d00001e02"), None);
		});
	}
}
