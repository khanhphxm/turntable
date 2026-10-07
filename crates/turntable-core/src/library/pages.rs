//! Listing headers and track pages, plus bounded pagination of the user's library.

use std::collections::HashSet;

use super::{Listing, PAGE, Route, artist::artist_header, model::album_kind, worker::Session};
use crate::{
	net::{par, trace},
	spotify::{self, Album, Artist, Client, Page, Playlist, id_of},
};

/// The most of each kind loaded into Your Library.
const LIBRARY_ITEMS: usize = 500;

/// `items` without repeats. A list that changes while it is paged shows an item on two pages,
/// and views key their rows by it (two siblings with one key reject the whole view).
fn unique<T>(items: Vec<T>, id: impl Fn(&T) -> &str) -> Vec<T> {
	let mut seen = HashSet::with_capacity(items.len());
	items.into_iter().filter(|i| seen.insert(id(i).to_owned())).collect()
}

/// Pages fetched at once.
const PAGES_AT_ONCE: usize = 4;

/// Every item of an offset-paged list, up to [`LIBRARY_ITEMS`], each once (`id` names it). The first
/// page says how many there are, so the pages after it are fetched together, not one after another.
fn all_pages<T: Send>(page: impl Fn(u32) -> spotify::Result<Page<T>> + Sync, id: impl Fn(&T) -> &str) -> spotify::Result<Vec<T>> {
	let first = page(0)?;
	let (next, total) = (first.next_offset(), first.total);
	let mut out = first.items;
	let Some(start) = next else { return Ok(unique(out, id)) };
	if total == 0 {
		// No size told: follow `next` a page at a time.
		let mut offset = start;
		loop {
			let more = page(offset)?;
			let next = more.next_offset();
			out.extend(more.items);
			match next {
				Some(o) if out.len() < LIBRARY_ITEMS => offset = o,
				_ => return Ok(unique(out, id)),
			}
		}
	}
	let end = total.min(u32::try_from(LIBRARY_ITEMS).unwrap_or(u32::MAX));
	let step = usize::try_from(start).unwrap_or(1).max(1);
	let offsets: Vec<u32> = (start..end).step_by(step).collect();
	for more in par::map(offsets, PAGES_AT_ONCE, &page) {
		out.extend(more?.items);
	}
	Ok(unique(out, id))
}

pub(super) fn all_playlists(c: &Client) -> spotify::Result<Vec<Playlist>> {
	all_pages(|offset| c.playlists(offset, 50), |p| p.id.as_str())
}

pub(super) fn all_albums(c: &Client) -> spotify::Result<Vec<Album>> {
	all_pages(|offset| c.saved_albums(offset, 50), |a| a.uri.as_str())
}

/// The artists the user follows (none without the `user-follow-read` permission: the home page
/// asks to reconnect). Paged by cursor, so not through [`all_pages`].
pub(super) fn all_followed(c: &mut Client) -> spotify::Result<Vec<Artist>> {
	let mut out: Vec<Artist> = Vec::new();
	while let Some(page) = optional(c.followed_artists(out.last().map(|a| id_of(&a.uri)), 50))? {
		let more = page.next.is_some() && !page.items.is_empty();
		out.extend(page.items);
		if !more || out.len() >= LIBRARY_ITEMS {
			break;
		}
	}
	Ok(unique(out, |a| a.uri.as_str()))
}
/// Loads a page of `route`. The header (title, artwork, context) is fetched with
/// the first page and reused for the next ones.
pub(super) fn open(s: &mut Session, route: &Route, offset: u32) -> spotify::Result<Listing> {
	// The first page of a listing opened lately; "Show more" builds on its header.
	let cached = s.pages.get(route);
	if offset == 0
		&& let Some(listing) = &cached
	{
		trace::hit("page", &format!("{route:?}"), "mem");
		return Ok(listing.clone());
	}
	if let Some(listing) = session_open(s, route, offset)? {
		if offset == 0 {
			s.pages.insert(route.clone(), listing.clone());
		}
		return Ok(listing);
	}
	let header = match cached {
		Some(listing) => listing,
		None => header(s, route)?,
	};
	let page = match route {
		Route::Liked => Some(s.client.saved_tracks(offset, PAGE)?),
		Route::Playlist(id) if header.note.is_none() && !header.web => Some(s.client.playlist_items(id, offset, PAGE)?),
		// An album's header came with its first page.
		Route::Album(_) if offset == 0 && !header.tracks.is_empty() => None,
		Route::Album(id) => Some(s.client.album_tracks(id, offset, PAGE)?),
		_ => None,
	};
	let mut listing = header;
	if let Some(page) = page {
		listing.next_offset = page.next_offset();
		listing.total = page.total;
		listing.tracks = page.items;
	}
	if let Route::Playlist(id) = route
		&& listing.web
		&& offset > 0
	{
		// The header holds the first page; later ones are asked for.
		let page = s.web.playlist(id, offset, PAGE).map_err(spotify::Error::Network)?;
		listing.take_web_page(page, offset);
	}
	if let (Route::Album(_), Some(album)) = (route, &listing.album) {
		// The album tracks endpoint omits each track's album; give them this one.
		for t in &mut listing.tracks {
			t.album.get_or_insert_with(|| album.clone());
		}
	}
	if let Route::Liked = route {
		listing.subtitle = songs(u64::from(listing.total));
	}
	if offset == 0 {
		s.pages.insert(route.clone(), listing.clone());
	}
	Ok(listing)
}

/// Session-backed pages don't inherit Web API ownership restrictions or web-player cursors.
fn session_open(s: &mut Session, route: &Route, offset: u32) -> spotify::Result<Option<Listing>> {
	let Some(player) = &mut s.player else { return Ok(None) };
	let listing = match route {
		Route::Liked => {
			let page = player.liked(offset)?;
			Listing {
				kicker: "Playlist".into(),
				title: "Liked Songs".into(),
				subtitle: songs(u64::from(page.total)),
				next_offset: page.next_offset(),
				total: page.total,
				tracks: page.items,
				..Listing::default()
			}
		},
		Route::Playlist(id) => {
			let (mut p, editable, page) = player.playlist(id, offset, true)?;
			if let Some(known) = s.playlists.iter().find(|known| known.id == *id) {
				p.public = known.public;
				p.owner.display_name.clone_from(&known.owner.display_name);
				if p.images.is_empty() {
					p.images.clone_from(&known.images);
				}
			}
			p.normalize(&s.me, s.me_name.as_deref());
			let owned = p.owner.id == s.me;
			let followed = if owned { None } else { player.contains(&p.uri).ok() };
			let subtitle = format!("{} • {}", p.owner_name(), songs(u64::from(p.track_count.unwrap_or_default())));
			let mut listing = Listing {
				kicker: "Playlist".into(),
				title: p.name,
				description: p.description,
				public: p.public,
				owned,
				subtitle,
				cover_url: spotify::pick_image(&p.images, 300).map(|i| i.url.clone()),
				context_uri: Some(p.uri),
				editable: editable || p.collaborative || owned,
				followed,
				..Listing::default()
			};
			if let Some(page) = page {
				listing.next_offset = page.next_offset();
				listing.total = page.total;
				listing.tracks = page.items;
			}
			listing
		},
		Route::Album(id) => {
			let (album, page) = player.album(id, offset)?;
			let followed = player.contains(&album.uri).ok();
			album_listing(album, Some(page), followed)
		},
		Route::Artist(_) | Route::Home | Route::Search | Route::Library => return Ok(None),
	};
	Ok(Some(listing))
}

/// A listing's header: everything but its tracks.
pub(super) fn header(s: &mut Session, route: &Route) -> spotify::Result<Listing> {
	Ok(match route {
		Route::Liked => Listing { kicker: ("Playlist").into(), title: "Liked Songs".into(), ..Listing::default() },
		Route::Playlist(id) => match s.client.playlist(id) {
			Ok(mut p) => {
				p.normalize(&s.me, s.me_name.as_deref());
				let owner = p.owner_name();
				let editable = p.items_readable(&s.me);
				let followed =
					if p.owner.id == s.me { None } else { part(s.client.library_contains(&[&p.uri]))?.and_then(|f| f.first().copied()) };
				// Songs of a playlist the Web API won't list (Development Mode): the web player's.
				let web = if editable { None } else { s.web.playlist(id, 0, PAGE).ok() };
				let note = (!editable && web.is_none()).then(|| {
					"Spotify only lets third-party apps list songs in playlists you own or collaborate on. \
					 Press play to listen; upcoming songs appear in the queue."
						.to_owned()
				});
				let total = web.as_ref().map(|w| w.total).or(p.track_count);
				let count = total.map(|n| format!(" • {}", songs(u64::from(n)))).unwrap_or_default();
				let mut listing = Listing {
					kicker: ("Playlist").into(),
					title: p.name.clone(),
					description: p.description.clone(),
					public: p.public,
					owned: p.owner.id == s.me,
					subtitle: format!("{owner}{count}"),
					cover_url: spotify::pick_image(&p.images, 300).map(|i| i.url.clone()),
					context_uri: Some(p.uri.clone()),
					note,
					editable,
					followed,
					..Listing::default()
				};
				if let Some(w) = web {
					listing.take_web_page(w, 0);
				}
				listing
			},
			// Spotify's own playlists and others' that Development Mode doesn't serve at all.
			Err(e) if refused(&e) => {
				let Ok(w) = s.web.playlist(id, 0, PAGE) else { return Err(e) };
				let uri = format!("spotify:playlist:{id}");
				let followed = part(s.client.library_contains(&[&uri]))?.and_then(|f| f.first().copied());
				let count = if w.total > 0 { format!(" • {}", songs(u64::from(w.total))) } else { String::new() };
				let mut listing = Listing {
					kicker: ("Playlist").into(),
					title: w.name.clone(),
					subtitle: format!("{}{count}", w.owner),
					cover_url: w.image.clone(),
					context_uri: Some(uri),
					followed,
					..Listing::default()
				};
				listing.take_web_page(w, 0);
				listing
			},
			Err(e) => return Err(e),
		},
		Route::Album(id) => album_header(s, id)?,
		Route::Artist(id) => artist_header(s, id)?,
		Route::Home | Route::Search | Route::Library => Listing::default(),
	})
}

/// An album's header, which comes with its first page of tracks.
fn album_header(s: &Session, id: &str) -> spotify::Result<Listing> {
	let uri = format!("spotify:album:{id}");
	// Whether it is in Your Library (the + / ✓ beside play; unknown leaves the button out) doesn't depend
	// on the album: asked together.
	let client = &s.client;
	let (full, followed) = par::join(|| client.album_full(id), || part(client.library_contains(&[&uri])));
	let (album, page) = full?;
	let followed = followed?.and_then(|f| f.first().copied());
	Ok(album_listing(album, page, followed))
}

fn album_listing(album: Album, page: Option<Page<spotify::Track>>, followed: Option<bool>) -> Listing {
	let by = album.artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ");
	let year = album.release_date.as_deref().and_then(|d| d.get(..4)).unwrap_or_default();
	let songs = songs(u64::from(album.total_tracks));
	let subtitle = [by.as_str(), year, songs.as_str()].into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" • ");
	let mut listing = Listing {
		kicker: (album_kind(&album)).into(),
		title: album.name.clone(),
		subtitle,
		cover_url: spotify::pick_image(&album.images, 300).map(|i| i.url.clone()),
		context_uri: Some(album.uri.clone()),
		followed,
		album: Some(album),
		..Listing::default()
	};
	if let Some(page) = page {
		listing.next_offset = page.next_offset();
		listing.total = page.total;
		listing.tracks = page.items;
	}
	listing
}
/// The tint of a cover (`#rrggbb`) from the web player, asked once a day for each cover.
pub(super) fn accent(s: &mut Session, url: &str) -> Option<String> {
	let key = url.to_owned();
	if let Some(known) = s.colors.get(&key) {
		return known;
	}
	let color = s.web.colors(url);
	s.colors.insert(key, color.clone());
	color
}

/// Whether Spotify refuses an item to this app (Development Mode: a 403 or a 404).
fn refused(e: &spotify::Error) -> bool {
	e.is_forbidden() || matches!(e, spotify::Error::Http { status: 404, .. })
}

/// Any failure but a lost sign-in as `None`: one part of a page shouldn't fail it.
pub(super) fn part<T>(r: spotify::Result<T>) -> spotify::Result<Option<T>> {
	match r {
		Ok(v) => Ok(Some(v)),
		Err(e @ (spotify::Error::AuthRequired | spotify::Error::AuthExpired)) => Err(e),
		Err(_) => Ok(None),
	}
}

/// A 403 as `None` (the sign-in lacks the permission); other errors pass through.
pub(super) fn optional<T>(r: spotify::Result<T>) -> spotify::Result<Option<T>> {
	match r {
		Ok(v) => Ok(Some(v)),
		Err(e) if e.is_forbidden() => Ok(None),
		Err(e) => Err(e),
	}
}
/// "1 song", "12 songs".
fn songs(n: u64) -> String {
	if n == 1 { "1 song".to_owned() } else { format!("{n} songs") }
}
#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn pages_are_followed_to_the_end_and_stop_at_the_cap() {
		let page = |offset: u32, len: u32, more: bool| -> spotify::Result<Page<String>> {
			Ok(Page {
				items: (offset..offset + len).map(|i| i.to_string()).collect(),
				total: 0,
				offset,
				limit: len,
				next: more.then(|| "next".to_owned()),
			})
		};
		let short = all_pages(|o| page(o, 1, o < 2), String::as_str).unwrap();
		assert_eq!(short, ["0", "1", "2"]);
		let endless = all_pages(|o| page(o, 100, true), String::as_str).unwrap();
		assert_eq!(endless.len(), LIBRARY_ITEMS);
	}

	#[test]
	fn an_item_that_moved_between_pages_is_listed_once() {
		// The list changed while it was paged: the last item of page one starts page two.
		let page = |offset: u32| -> spotify::Result<Page<String>> {
			let items = if offset == 0 { ["a", "b"] } else { ["b", "c"] };
			Ok(Page { items: items.map(str::to_owned).into(), total: 3, offset, limit: 2, next: (offset == 0).then(|| "next".to_owned()) })
		};
		assert_eq!(all_pages(page, String::as_str).unwrap(), ["a", "b", "c"]);
	}
}
