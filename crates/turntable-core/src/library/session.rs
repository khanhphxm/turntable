//! Library data and Playlist4 edits from the player's authenticated Session.
//! Album listings carry full-release duration, label, release date, copyright lines,
//! per-track disc numbers and other releases by the first artist, without Web API reads.
//! Playlist4 keeps raw page offsets; metadata resolves pages in batches.
//! Collection2 handles saves/follows, playlist-permission handles privacy (rootlist
//! publication is separate), and profile/Connect commands never fall through to the Web API.
//! Playlist and Liked Songs rows retain occurrence-specific added dates through
//! metadata deduplication and unavailable rows. `Listing::added_at(row)` returns Unix
//! seconds (Playlist4 milliseconds are converted); albums and old caches return `None`.
//! `Listing::added_by(row)` exposes Playlist4's username when present, not a display name.

#[cfg(test)]
mod added_live;
mod artwork;
mod collection;
mod connect;
mod home;
#[cfg(test)]
mod live;
mod playlist;
mod profile;
mod rootlist;
#[cfg(test)]
mod tests;

use std::{
	collections::{HashMap, HashSet},
	sync::Arc,
	time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use collection::{CollectionItem, PageResponse, Set as CollectionSet};
use futures_util::TryStreamExt as _;
use librespot_core::{FileId, Session, SpotifyId, SpotifyUri};
use librespot_protocol::{
	extended_metadata::{BatchedEntityRequest, EntityRequest, ExtensionQuery},
	extension_kind::ExtensionKind,
	metadata,
	playlist4_external::{ListAttributes, MetaItem},
};
use protobuf::{EnumOrUnknown, Message};

use super::{HomeSection, Listing, PAGE, Route, Tile, added::TrackId, artist::Popularity};
use crate::{
	engine::SessionReader,
	net::{cache::Cache, par, trace},
	spotify::{self, Album, Artist, Image, Page, Playlist, Track, User},
};

const BATCH: usize = 100;
const TTL: Duration = Duration::from_secs(180);
const METADATA_TTL: Duration = Duration::from_secs(1800);

pub(super) struct Reader {
	shared:        SessionReader,
	albums:        Cache<String, Arc<AlbumData>>,
	artists:       Cache<String, Arc<ArtistData>>,
	collection:    Option<(Instant, Vec<CollectionItem>)>,
	liked_ids:     Option<Arc<[TrackId]>>,
	liked_changed: bool,
	followed:      Option<(Instant, Vec<CollectionItem>)>,
	excluded:      Option<(Instant, Vec<CollectionItem>)>,
	roots:         Option<(Instant, rootlist::Data)>,
}

struct AlbumData {
	album:       Album,
	tracks:      Vec<Track>,
	duration_ms: u64,
	disc_count:  u32,
	more_by:     Vec<Tile>,
}

impl AlbumData {
	fn listing(&self, offset: u32, followed: Option<bool>) -> Listing {
		let items = window(&self.tracks, offset, PAGE).to_vec();
		let mut listing =
			super::pages::album_listing(self.album.clone(), Some(page(items, offset, PAGE, self.album.total_tracks)), followed);
		listing.duration_ms = Some(self.duration_ms);
		listing.disc_count = self.disc_count;
		listing.more_by.clone_from(&self.more_by);
		listing
	}
}

struct ArtistData {
	artist:  Artist,
	top:     Vec<String>,
	groups:  Vec<(&'static str, Vec<String>)>,
	appears: Vec<String>,
}

pub(super) struct TrackPage {
	pub page:     Page<Track>,
	pub added_at: Vec<Option<i64>>,
	pub added_by: Vec<Option<String>>,
}

/// Metadata batching deduplicates URIs and may omit unavailable items; match each
/// resolved occurrence against the next raw occurrence, never a URI-keyed map.
fn added_page<'a>(
	rows: Vec<Track>,
	mut attributes: impl Iterator<Item = (&'a str, Option<i64>, Option<&'a str>)>,
	offset: u32,
	total: u32,
) -> TrackPage {
	let (added_at, added_by) = rows
		.iter()
		.map(|track| {
			let attributes = attributes.find(|(uri, _, _)| *uri == track.uri);
			attributes.map_or((None, None), |(_, at, by)| (at, by.map(str::to_owned)))
		})
		.unzip();
	TrackPage { page: page(rows, offset, PAGE, total), added_at, added_by }
}

impl Reader {
	pub(super) fn new(shared: SessionReader) -> Self {
		Self {
			shared,
			albums: Cache::new(16, METADATA_TTL),
			artists: Cache::new(16, METADATA_TTL),
			collection: None,
			liked_ids: None,
			liked_changed: false,
			followed: None,
			excluded: None,
			roots: None,
		}
	}

	pub(super) fn session_reader(&self) -> SessionReader {
		self.shared.clone()
	}

	pub(super) fn home_section(&self, uri: &str, offset: u32) -> Result<(HomeSection, Option<u32>), String> {
		spotify::home_feed::fetch_section(&self.shared, uri, offset)
	}

	pub(super) fn matches(&self, username: &str) -> bool {
		!self.shared.session().is_invalid() && self.shared.session().username() == username
	}

	pub(super) fn forget(&mut self, route: &Route) {
		match route {
			Route::Liked => self.collection = None,
			Route::Playlist(_) | Route::Folder(_) => self.roots = None,
			Route::Album(_) | Route::Artist(_) | Route::Home | Route::Search | Route::Library => {},
		}
	}

	pub(super) fn saved(&mut self, uri: &str, saved: bool) {
		let cache = if uri.starts_with("spotify:artist:") { &mut self.followed } else { &mut self.collection };
		if let Some((_, items)) = cache {
			items.retain(|item| item.uri != uri);
			if saved {
				let added_at = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
				items.insert(0, CollectionItem { uri: uri.to_owned(), added_at, ..CollectionItem::default() });
			}
		}
		if uri.starts_with("spotify:track:") {
			self.liked_ids = None;
			self.liked_changed = true;
		}
		if uri.starts_with("spotify:playlist:") {
			self.roots = None;
		}
	}

	/// Saves tracks/albums in Liked Songs' `collection` set, artist follows in `artist`,
	/// and playlist follows in Playlist4's rootlist. Failed writes never edit membership.
	pub(super) fn set_saved(&mut self, uri: &str, saved: bool) -> spotify::Result<()> {
		if uri.starts_with("spotify:playlist:") {
			return self.follow_playlist(uri, saved);
		}
		let set = if uri.starts_with("spotify:artist:") { CollectionSet::Artists } else { CollectionSet::Saved };
		self.write_collection(set, uri, saved)?;
		self.saved(uri, saved);
		Ok(())
	}

	fn write_collection(&self, set: CollectionSet, uri: &str, saved: bool) -> spotify::Result<()> {
		let item = CollectionItem {
			uri:        uri.to_owned(),
			added_at:   SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX)),
			is_removed: !saved,
		};
		let body = collection::write_request(&self.shared.session().username(), set, &item, &format!("{:032x}", rand::random::<u128>()))
			.map_err(|error| spotify::Error::Parse(error.to_string()))?;
		self.shared
			.block_on(async {
				let span = trace::start();
				let bytes = self
					.shared
					.session()
					.spclient()
					.request(&http::Method::POST, "/collection/v2/write", Some(collection_headers()), Some(&body))
					.await?;
				span.done("session", "POST", "https://spclient.spotify.com/collection/v2/write", trace::Reply {
					status: 200,
					bytes: bytes.len(),
					..trace::Reply::default()
				});
				Ok(())
			})
			.map_err(network)
	}

	fn collection(&mut self, set: CollectionSet) -> spotify::Result<&[CollectionItem]> {
		let cache = match set {
			CollectionSet::Saved => &mut self.collection,
			CollectionSet::Artists => &mut self.followed,
			CollectionSet::IgnoreInRecs => &mut self.excluded,
		};
		if cache.as_ref().is_none_or(|(at, _)| at.elapsed() >= TTL) {
			let items = self.shared.block_on(collection(self.shared.session(), set)).map_err(network)?;
			*cache = Some((Instant::now(), items));
			if matches!(set, CollectionSet::Saved) {
				self.liked_ids = None;
				self.liked_changed = true;
			}
		}
		Ok(cache.as_ref().map_or(&[], |(_, items)| items.as_slice()))
	}

	/// A full snapshot only when the collection was refreshed or locally edited.
	/// Checking another row while fresh neither allocates nor republishes the same membership.
	pub(super) fn liked_update(&mut self) -> spotify::Result<Option<Arc<[TrackId]>>> {
		self.collection(CollectionSet::Saved)?;
		let items = self.collection.as_ref().map_or(&[][..], |(_, items)| items.as_slice());
		Ok(liked_snapshot(items, &mut self.liked_ids, &mut self.liked_changed))
	}

	pub(super) fn contains(&mut self, uri: &str) -> spotify::Result<bool> {
		if uri.starts_with("spotify:playlist:") {
			return Ok(self.playlists()?.iter().any(|p| p.uri == uri));
		}
		let set = if uri.starts_with("spotify:artist:") { CollectionSet::Artists } else { CollectionSet::Saved };
		Ok(self.collection(set)?.iter().any(|item| item.uri == uri))
	}

	/// Spotify's desktop client stores taste-profile exclusions in `ignoreinrecs`,
	/// independently of saved tracks/albums and followed artists.
	pub(super) fn excluded(&mut self) -> spotify::Result<Vec<String>> {
		Ok(self.collection(CollectionSet::IgnoreInRecs)?.iter().map(|item| item.uri.clone()).collect())
	}

	pub(super) fn set_excluded(&mut self, uri: &str, excluded: bool) -> spotify::Result<()> {
		if !uri.starts_with("spotify:playlist:") && !uri.starts_with("spotify:track:") {
			return Err(spotify::Error::Parse("Spotify taste-profile exclusions support playlists and tracks only.".into()));
		}
		self.collection(CollectionSet::IgnoreInRecs)?;
		self.write_collection(CollectionSet::IgnoreInRecs, uri, excluded)?;
		if let Some((_, items)) = &mut self.excluded {
			items.retain(|item| item.uri != uri);
			if excluded {
				items.push(CollectionItem { uri: uri.to_owned(), ..CollectionItem::default() });
			}
		}
		Ok(())
	}

	fn roots(&mut self) -> spotify::Result<&rootlist::Data> {
		if self.roots.as_ref().is_none_or(|(at, _)| at.elapsed() >= TTL) {
			let roots = self.shared.block_on(rootlist::fetch(self.shared.session())).map_err(network)?;
			self.roots = Some((Instant::now(), roots));
		}
		self.roots.as_ref().map(|(_, roots)| roots).ok_or_else(|| spotify::Error::Parse("Native rootlist is unavailable".into()))
	}

	pub(super) fn playlists(&mut self) -> spotify::Result<Vec<Playlist>> {
		Ok(self.roots()?.playlists.clone())
	}

	pub(in crate::library) fn rootlist(&mut self) -> spotify::Result<Vec<super::RootEntry>> {
		Ok(self.roots()?.entries.clone())
	}

	pub(super) fn playlist(&self, id: &str, offset: u32, rows: bool) -> spotify::Result<(Playlist, bool, Option<TrackPage>)> {
		self.shared
			.block_on(async {
				let raw = playlist::fetch(self.shared.session(), id, offset, if rows { PAGE } else { 0 }).await?;
				let mut info = playlist(id, raw.owner_username(), raw.attributes.get_or_default(), raw.length());
				info.public = playlist::public(self.shared.session(), id).await?;
				let editable = raw.capabilities.get_or_default().can_edit_items();
				let page = if rows {
					let contents = raw.contents.get_or_default();
					let uris: Vec<_> = contents.items.iter().map(|item| item.uri().to_owned()).collect();
					let items = tracks(self.shared.session(), &uris, None).await?;
					Some(added_page(
						items,
						contents.items.iter().map(playlist::row_attributes),
						offset,
						u32::try_from(raw.length()).unwrap_or_default(),
					))
				} else {
					None
				};
				let first = page.as_ref().filter(|_| offset == 0).map(|page| (&raw, page.page.items.as_slice()));
				artwork::fill(self.shared.session(), &mut info, first).await?;
				Ok((info, editable, page))
			})
			.map_err(network)
	}

	pub(super) fn liked(&mut self, offset: u32) -> spotify::Result<TrackPage> {
		let items = self.collection(CollectionSet::Saved)?;
		let total = u32::try_from(items.iter().filter(|item| item.uri.starts_with("spotify:track:")).count()).unwrap_or(u32::MAX);
		let (uris, timestamps): (Vec<_>, Vec<_>) = items
			.iter()
			.filter(|item| item.uri.starts_with("spotify:track:"))
			.skip(usize::try_from(offset).unwrap_or(usize::MAX))
			.take(PAGE as usize)
			.map(|item| (item.uri.clone(), item.added_at))
			.unzip();
		let tracks = self.shared.block_on(tracks(self.shared.session(), &uris, None)).map_err(network)?;
		Ok(added_page(tracks, uris.iter().zip(timestamps).map(|(uri, at)| (uri.as_str(), Some(at), None)), offset, total))
	}

	fn album_data(&mut self, id: &str, offset: u32, partial: &dyn Fn(Listing)) -> spotify::Result<Arc<AlbumData>> {
		let entity = format!("spotify:album:{id}");
		if let Some(known) = self.albums.get(&entity) {
			return Ok(known);
		}
		let raw = self
			.shared
			.block_on(metadata::<metadata::Album>(self.shared.session(), std::slice::from_ref(&entity), ExtensionKind::ALBUM_V4))
			.map_err(network)?
			.remove(&entity)
			.ok_or_else(|| spotify::Error::Parse("Spotify returned no album metadata".into()))?;
		let album = album(&raw).map_err(network)?;
		let uris = raw.disc.iter().flat_map(|d| &d.track).map(|t| uri("track", t.gid())).collect::<Result<Vec<_>, _>>().map_err(network)?;
		let tracks = self.shared.block_on(tracks(self.shared.session(), &uris, Some(&album))).map_err(network)?;
		let duration_ms = tracks.iter().map(|track| u64::from(track.duration_ms)).sum();
		let disc_count = u32::try_from(raw.disc.len()).unwrap_or(u32::MAX);
		if offset == 0 {
			let mut first = super::pages::album_listing(
				album.clone(),
				Some(page(window(&tracks, 0, PAGE).to_vec(), 0, PAGE, album.total_tracks)),
				None,
			);
			first.duration_ms = Some(duration_ms);
			first.disc_count = disc_count;
			partial(first);
		}
		let more_by = if let Some(artist) = album.artists.first() {
			let artist = self.artist_data(spotify::id_of(&artist.uri))?;
			let uris = more_by_uris(&artist.groups, &album.uri);
			let raw = self
				.shared
				.block_on(metadata::<metadata::Album>(self.shared.session(), &uris, ExtensionKind::ALBUM_V4))
				.map_err(network)?;
			let mut releases = release_models(&uris, &raw).map_err(network)?;
			releases.sort_by(|a, b| b.release_date.cmp(&a.release_date));
			releases.iter().map(Tile::release).collect()
		} else {
			Vec::new()
		};
		let data = Arc::new(AlbumData { album, tracks, duration_ms, disc_count, more_by });
		self.albums.insert(entity, data.clone());
		Ok(data)
	}

	pub(super) fn album(&mut self, id: &str, offset: u32, partial: &dyn Fn(Listing)) -> spotify::Result<Listing> {
		let data = self.album_data(id, offset, partial)?;
		let followed = self.contains(&data.album.uri).ok();
		Ok(data.listing(offset, followed))
	}

	/// Context actions need URIs, not per-track metadata. Read the raw Playlist4
	/// offsets so unavailable rows cannot make a later page disappear.
	pub(super) fn context_page(&mut self, route: &Route, offset: u32) -> spotify::Result<(String, Vec<String>, Option<u32>)> {
		match route {
			Route::Album(id) => {
				let entity = format!("spotify:album:{id}");
				if let Some(data) = self.albums.get(&entity)
					&& usize::try_from(data.album.total_tracks) == Ok(data.tracks.len())
				{
					return Ok((data.album.name.clone(), data.tracks.iter().map(|track| track.uri.clone()).collect(), None));
				}
				let raw = self
					.shared
					.block_on(metadata::<metadata::Album>(self.shared.session(), std::slice::from_ref(&entity), ExtensionKind::ALBUM_V4))
					.map_err(network)?
					.remove(&entity)
					.ok_or_else(|| spotify::Error::Parse("Spotify returned no album metadata".into()))?;
				let uris = raw
					.disc
					.iter()
					.flat_map(|disc| &disc.track)
					.map(|track| uri("track", track.gid()))
					.collect::<Result<Vec<_>, _>>()
					.map_err(network)?;
				Ok((raw.name().to_owned(), uris, None))
			},
			Route::Playlist(id) => {
				let raw = self.shared.block_on(playlist::fetch(self.shared.session(), id, offset, PAGE)).map_err(network)?;
				let contents = raw.contents.get_or_default();
				let next =
					u32::try_from(contents.pos()).unwrap_or(offset).saturating_add(u32::try_from(contents.items.len()).unwrap_or(u32::MAX));
				let total = u32::try_from(raw.length()).unwrap_or_default();
				let uris = contents.items.iter().map(|item| item.uri().to_owned()).collect();
				Ok((raw.attributes.name().to_owned(), uris, (next < total).then_some(next)))
			},
			_ => Err(spotify::Error::Parse("Only albums and playlists have context tracks.".into())),
		}
	}

	fn artist_data(&mut self, id: &str) -> spotify::Result<Arc<ArtistData>> {
		let entity = format!("spotify:artist:{id}");
		if let Some(known) = self.artists.get(&entity) {
			return Ok(known);
		}
		let raw = self
			.shared
			.block_on(metadata::<metadata::Artist>(self.shared.session(), std::slice::from_ref(&entity), ExtensionKind::ARTIST_V4))
			.map_err(network)?
			.remove(&entity)
			.ok_or_else(|| spotify::Error::Parse("Spotify returned no artist metadata".into()))?;
		let country = self.shared.session().country();
		let top = raw.top_track.iter().find(|t| t.country() == country).or_else(|| raw.top_track.iter().find(|t| t.country().is_empty()));
		let top = top
			.into_iter()
			.flat_map(|top| &top.track)
			.take(10)
			.map(|t| uri("track", t.gid()))
			.collect::<Result<_, _>>()
			.map_err(network)?;
		let groups = [("Albums", &raw.album_group), ("Singles and EPs", &raw.single_group), ("Compilations", &raw.compilation_group)]
			.into_iter()
			.map(|(title, groups)| releases(groups).map(|uris| (title, uris)))
			.collect::<Result<_, _>>()
			.map_err(network)?;
		let data = Arc::new(ArtistData {
			artist: artist(&raw).map_err(network)?,
			top,
			groups,
			appears: releases(&raw.appears_on_group).map_err(network)?,
		});
		self.artists.insert(entity, data.clone());
		Ok(data)
	}

	pub(super) fn artist(&mut self, id: &str) -> spotify::Result<Artist> {
		Ok(self.artist_data(id)?.artist.clone())
	}

	pub(super) fn artist_header(&mut self, id: &str, partial: &dyn Fn(Listing)) -> spotify::Result<Listing> {
		let data = self.artist_data(id)?;
		let a = &data.artist;
		let mut listing = Listing {
			kicker: "Artist".into(),
			title: a.name.clone(),
			cover_url: spotify::pick_image(&a.images, 300).map(|image| image.url.clone()),
			context_uri: Some(a.uri.clone()),
			artist: Some(a.clone()),
			..Listing::default()
		};
		let uris: Vec<_> = data.groups.iter().flat_map(|(_, uris)| uris.iter()).chain(&data.appears).collect();
		let (rows, albums) = self
			.shared
			.block_on(async {
				tokio::try_join!(
					async {
						let rows = tracks(self.shared.session(), &data.top, None).await?;
						partial(Listing { tracks: rows.clone(), popular_loaded: true, ..listing.clone() });
						Ok(rows)
					},
					metadata::<metadata::Album>(self.shared.session(), &uris, ExtensionKind::ALBUM_V4)
				)
			})
			.map_err(network)?;
		listing.tracks = rows;
		listing.popular_loaded = true;
		let mut discography = Vec::new();
		for (title, uris) in &data.groups {
			let mut releases = release_models(uris, &albums).map_err(network)?;
			releases.sort_by(|a, b| b.release_date.cmp(&a.release_date));
			if !releases.is_empty() {
				discography.push((*title, releases.iter().map(Tile::release).collect()));
			}
		}
		listing.appears_on = release_models(&data.appears, &albums).map_err(network)?.iter().map(Tile::album).collect();
		listing.followed = self.contains(&a.uri).ok();
		listing.discography = discography;
		Ok(listing)
	}

	pub(super) fn popularity(&mut self, id: &str) -> spotify::Result<Popularity> {
		let data = self.artist_data(id)?;
		let tracks = self.shared.block_on(tracks(self.shared.session(), &data.top, None)).map_err(network)?;
		Ok(Popularity { tracks, ..Popularity::default() })
	}

	pub(super) fn albums(&mut self, limit: usize) -> spotify::Result<Vec<Album>> {
		let uris: Vec<_> = self
			.collection(CollectionSet::Saved)?
			.iter()
			.filter(|i| i.uri.starts_with("spotify:album:"))
			.take(limit)
			.map(|i| i.uri.clone())
			.collect();
		let raw =
			self.shared.block_on(metadata::<metadata::Album>(self.shared.session(), &uris, ExtensionKind::ALBUM_V4)).map_err(network)?;
		release_models(&uris, &raw).map_err(network)
	}

	pub(super) fn followed(&mut self, limit: usize) -> spotify::Result<Vec<Artist>> {
		let uris: Vec<_> = self
			.collection(CollectionSet::Artists)?
			.iter()
			.filter(|i| i.uri.starts_with("spotify:artist:"))
			.take(limit)
			.map(|i| i.uri.clone())
			.collect();
		self.artists(&uris)
	}

	pub(super) fn artists(&self, uris: &[String]) -> spotify::Result<Vec<Artist>> {
		let mut raw =
			self.shared.block_on(metadata::<metadata::Artist>(self.shared.session(), uris, ExtensionKind::ARTIST_V4)).map_err(network)?;
		uris.iter().filter_map(|uri| raw.remove(uri)).map(|raw| artist(&raw)).collect::<Result<_, _>>().map_err(network)
	}
}

fn liked_snapshot(items: &[CollectionItem], cached: &mut Option<Arc<[TrackId]>>, changed: &mut bool) -> Option<Arc<[TrackId]>> {
	if !std::mem::take(changed) {
		return None;
	}
	let tracks = cached.get_or_insert_with(|| {
		let mut ids: Vec<_> = items.iter().filter_map(|item| TrackId::from_uri(&item.uri).ok()).collect();
		ids.sort_unstable();
		ids.dedup();
		ids.into()
	});
	Some(tracks.clone())
}

#[expect(clippy::needless_pass_by_value, reason = "map_err consumes the session error at every request boundary")]
fn network(error: librespot_core::Error) -> spotify::Error {
	spotify::Error::Network(error.to_string())
}

fn page<T>(items: Vec<T>, offset: u32, limit: u32, total: u32) -> Page<T> {
	Page { items, offset, limit, total, next: (offset.saturating_add(limit) < total).then(|| "session".to_owned()) }
}

fn window<T>(items: &[T], offset: u32, limit: u32) -> &[T] {
	let start = usize::try_from(offset).unwrap_or(usize::MAX).min(items.len());
	let end = start.saturating_add(usize::try_from(limit).unwrap_or(usize::MAX)).min(items.len());
	items.get(start..end).unwrap_or_default()
}

async fn metadata<M: Message>(
	session: &Session,
	uris: &[impl AsRef<str>],
	kind: ExtensionKind,
) -> Result<HashMap<String, M>, librespot_core::Error> {
	let mut unique = HashSet::with_capacity(uris.len());
	let uris: Vec<_> = uris.iter().map(AsRef::as_ref).filter(|uri| unique.insert(*uri)).collect();
	let mut out = HashMap::with_capacity(uris.len());
	let replies = par::async_map(uris.chunks(BATCH), 4, |batch| async move {
		let request = BatchedEntityRequest {
			entity_request: batch
				.iter()
				.map(|uri| EntityRequest {
					entity_uri: (*uri).to_owned(),
					query: vec![ExtensionQuery { extension_kind: EnumOrUnknown::new(kind), ..ExtensionQuery::default() }],
					..EntityRequest::default()
				})
				.collect(),
			..BatchedEntityRequest::default()
		};
		let span = trace::start();
		let reply = session.spclient().get_extended_metadata(request).await;
		span.done("session", "POST", "https://spclient.spotify.com/extended-metadata/v0/extended-metadata", trace::Reply {
			status: if reply.is_ok() { 200 } else { 0 },
			bytes: reply.as_ref().map_or(0, |reply| usize::try_from(reply.compute_size()).unwrap_or(usize::MAX)),
			..trace::Reply::default()
		});
		reply
	});
	futures_util::pin_mut!(replies);
	while let Some(reply) = replies.try_next().await? {
		for array in reply.extended_metadata {
			if array.extension_kind.enum_value_or_default() != kind {
				continue;
			}
			for data in array.extension_data {
				if data.header.get_or_default().status_code == 200
					&& let Some(bytes) = data.extension_data.into_option()
				{
					out.insert(data.entity_uri, M::parse_from_bytes(&bytes.value)?);
				}
			}
		}
	}
	Ok(out)
}

async fn tracks(session: &Session, uris: &[String], known_album: Option<&Album>) -> Result<Vec<Track>, librespot_core::Error> {
	let track_uris: Vec<_> = uris.iter().filter(|u| u.starts_with("spotify:track:")).collect();
	let episode_uris: Vec<_> = uris.iter().filter(|u| u.starts_with("spotify:episode:")).collect();
	let (raw, episodes) = tokio::try_join!(
		metadata::<metadata::Track>(session, &track_uris, ExtensionKind::TRACK_V4),
		metadata::<metadata::Episode>(session, &episode_uris, ExtensionKind::EPISODE_V4)
	)?;
	resolve_rows(uris, &raw, &episodes, known_album)
}

fn resolve_rows(
	uris: &[String],
	raw: &HashMap<String, metadata::Track>,
	episodes: &HashMap<String, metadata::Episode>,
	known_album: Option<&Album>,
) -> Result<Vec<Track>, librespot_core::Error> {
	let mut out = Vec::with_capacity(uris.len());
	for entity in uris {
		let row = if let Some(t) = raw.get(entity) {
			Some(track(entity, t, known_album)?)
		} else if let Some(e) = episodes.get(entity) {
			Some(episode(entity, e)?)
		} else {
			local(entity)
		};
		if let Some(row) = row {
			out.push(row);
		}
	}
	Ok(out)
}

fn uri(kind: &str, gid: &[u8]) -> Result<String, librespot_core::Error> {
	Ok(format!("spotify:{kind}:{}", SpotifyId::from_raw(gid)?.to_base62()?))
}

fn artist(raw: &metadata::Artist) -> Result<Artist, librespot_core::Error> {
	let images = if raw.portrait_group.is_some() { &raw.portrait_group.get_or_default().image } else { &raw.portrait };
	Ok(Artist { name: raw.name().to_owned(), uri: uri("artist", raw.gid())?, images: image_models(images), genres: Vec::new() })
}

fn album(raw: &metadata::Album) -> Result<Album, librespot_core::Error> {
	let images = if raw.cover_group.is_some() { &raw.cover_group.get_or_default().image } else { &raw.cover };
	let date = raw.date.as_ref().filter(|date| date.year() > 0).map(|date| {
		if date.has_day() {
			format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
		} else if date.has_month() {
			format!("{:04}-{:02}", date.year(), date.month())
		} else {
			format!("{:04}", date.year())
		}
	});
	Ok(Album {
		name:         raw.name().to_owned(),
		uri:          uri("album", raw.gid())?,
		images:       image_models(images),
		artists:      raw.artist.iter().map(artist).collect::<Result<_, _>>()?,
		total_tracks: u32::try_from(raw.disc.iter().map(|d| d.track.len()).sum::<usize>()).unwrap_or(u32::MAX),
		release_date: date,
		label:        raw.has_label().then(|| raw.label().to_owned()).filter(|label| !label.is_empty()),
		album_type:   match raw.type_() {
			metadata::album::Type::SINGLE | metadata::album::Type::EP => "single",
			metadata::album::Type::COMPILATION => "compilation",
			_ => "album",
		}
		.into(),
		copyrights:   raw
			.copyright
			.iter()
			.map(|c| spotify::Copyright {
				text: c.text().to_owned(),
				kind: if c.type_() == metadata::copyright::Type::P { "P" } else { "C" }.into(),
			})
			.collect(),
	})
}

fn image_models(images: &[metadata::Image]) -> Vec<Image> {
	images
		.iter()
		.filter(|image| image.file_id().len() == 20)
		.map(|image| {
			let id = FileId::from_raw(image.file_id());
			let width = u32::try_from(image.width()).ok().filter(|w| *w > 0).or(Some(match image.size() {
				metadata::image::Size::SMALL => 64,
				metadata::image::Size::LARGE => 640,
				metadata::image::Size::XLARGE => 1024,
				metadata::image::Size::DEFAULT => 300,
			}));
			Image {
				url: format!("https://i.scdn.co/image/{id}"),
				width,
				height: u32::try_from(image.height()).ok().filter(|h| *h > 0).or(width),
			}
		})
		.collect()
}

fn track(entity: &str, raw: &metadata::Track, known_album: Option<&Album>) -> Result<Track, librespot_core::Error> {
	Ok(Track {
		uri:         entity.to_owned(),
		name:        raw.name().to_owned(),
		duration_ms: u32::try_from(raw.duration()).unwrap_or_default(),
		disc_number: u32::try_from(raw.disc_number()).unwrap_or_default(),
		explicit:    raw.explicit(),
		artists:     raw.artist.iter().map(artist).collect::<Result<_, _>>()?,
		album:       match known_album {
			Some(album) => Some(album.clone()),
			None => raw.album.as_ref().map(album).transpose()?,
		},
		is_local:    false,
		is_episode:  false,
	})
}

fn episode(entity: &str, raw: &metadata::Episode) -> Result<Track, librespot_core::Error> {
	let show = raw.show.get_or_default();
	Ok(Track {
		uri:         entity.to_owned(),
		name:        raw.name().to_owned(),
		duration_ms: u32::try_from(raw.duration()).unwrap_or_default(),
		disc_number: 0,
		explicit:    raw.explicit(),
		artists:     Vec::new(),
		album:       Some(Album {
			name:         show.name().to_owned(),
			uri:          uri("show", show.gid())?,
			images:       image_models(&raw.cover_image.get_or_default().image),
			artists:      Vec::new(),
			total_tracks: 0,
			release_date: None,
			album_type:   String::new(),
			copyrights:   Vec::new(),
			label:        None,
		}),
		is_local:    false,
		is_episode:  true,
	})
}

fn local(entity: &str) -> Option<Track> {
	let SpotifyUri::Local { artist, album_title, track_title, duration } = SpotifyUri::from_uri(entity).ok()? else { return None };
	Some(Track {
		uri:         entity.to_owned(),
		name:        track_title,
		duration_ms: u32::try_from(duration.as_millis()).unwrap_or(u32::MAX),
		disc_number: 0,
		explicit:    false,
		artists:     vec![Artist { name: artist, uri: String::new(), images: Vec::new(), genres: Vec::new() }],
		album:       Some(Album {
			name:         album_title,
			uri:          String::new(),
			images:       Vec::new(),
			artists:      Vec::new(),
			total_tracks: 0,
			release_date: None,
			album_type:   String::new(),
			copyrights:   Vec::new(),
			label:        None,
		}),
		is_local:    true,
		is_episode:  false,
	})
}

/// A bounded shelf of the primary artist's own releases, never this album or duplicates.
fn more_by_uris(groups: &[(&str, Vec<String>)], current: &str) -> Vec<String> {
	let mut seen = HashSet::new();
	groups.iter().flat_map(|(_, uris)| uris).filter(|uri| uri.as_str() != current && seen.insert(uri.as_str())).take(10).cloned().collect()
}

fn releases(groups: &[metadata::AlbumGroup]) -> Result<Vec<String>, librespot_core::Error> {
	groups
		.iter()
		.filter_map(|group| group.album.first())
		.take(spotify::MAX_ARTIST_ALBUMS as usize)
		.map(|album| uri("album", album.gid()))
		.collect()
}

fn release_models(uris: &[String], raw: &HashMap<String, metadata::Album>) -> Result<Vec<Album>, librespot_core::Error> {
	uris.iter().filter_map(|uri| raw.get(uri)).map(album).collect()
}

fn playlist(id: &str, owner: &str, attributes: &ListAttributes, length: i32) -> Playlist {
	Playlist {
		id:            id.to_owned(),
		uri:           format!("spotify:playlist:{id}"),
		name:          attributes.name().to_owned(),
		description:   Playlist::has_description(attributes.description()).then(|| attributes.description().to_owned()),
		public:        None,
		owner:         User { id: owner.to_owned(), display_name: None },
		images:        artwork::images(attributes),
		collaborative: attributes.collaborative(),
		track_count:   u32::try_from(length).ok(),
	}
}

fn root_playlist(entity: &str, meta: &MetaItem) -> Option<Playlist> {
	let SpotifyUri::Playlist { id, .. } = SpotifyUri::from_uri(entity).ok()? else { return None };
	if meta.has_status_code() && !matches!(meta.status_code(), 0 | 200) {
		return None;
	}
	Some(playlist(&id.to_base62().ok()?, meta.owner_username(), meta.attributes.get_or_default(), meta.length()))
}

fn collection_headers() -> http::HeaderMap {
	let mut headers = http::HeaderMap::new();
	headers.insert(http::header::CONTENT_TYPE, http::HeaderValue::from_static("application/vnd.collection-v2.spotify.proto"));
	headers.insert(http::header::ACCEPT, http::HeaderValue::from_static("application/vnd.collection-v2.spotify.proto"));
	headers
}

async fn collection(session: &Session, set: CollectionSet) -> Result<Vec<CollectionItem>, librespot_core::Error> {
	let mut token = String::new();
	let mut items = HashMap::new();
	loop {
		let body = collection::request(&session.username(), set, &token)?;
		let span = trace::start();
		let bytes =
			session.spclient().request(&http::Method::POST, "/collection/v2/paging", Some(collection_headers()), Some(&body)).await?;
		span.done("session", "POST", "https://spclient.spotify.com/collection/v2/paging", trace::Reply {
			status: 200,
			bytes: bytes.len(),
			..trace::Reply::default()
		});
		let response = PageResponse::parse_from_bytes(&bytes)?;
		for item in response.items {
			if item.is_removed {
				items.remove(&item.uri);
			} else {
				items.insert(item.uri.clone(), item);
			}
		}
		if response.next_page_token.is_empty() {
			break;
		}
		if token == response.next_page_token {
			return Err(librespot_core::Error::failed_precondition("collection pagination did not advance"));
		}
		token = response.next_page_token;
	}
	let mut items: Vec<_> = items.into_values().collect();
	items.sort_by(|a, b| b.added_at.cmp(&a.added_at).then_with(|| a.uri.cmp(&b.uri)));
	Ok(items)
}
