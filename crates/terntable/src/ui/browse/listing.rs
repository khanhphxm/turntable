//! Playlist, album and artist headers, actions, track listings and release notes.

use tern_sdk::ui::{Node, html};

use super::{
	artist::artist_body,
	tracks::{Middle, plays_shown, tracks, tracks_with},
};
use crate::{
	app::{Msg, State},
	library::{Listing, Route},
	spotify,
	ui::{
		Tap, art, button, div, grouped, icon,
		icons::{Glyph, Icons},
		text,
	},
};

pub(super) fn listing_view(state: &State, icons: &Icons, listing: &Listing) -> Vec<Node<Msg>> {
	let route = &state.library.route;
	let overview = listing.overview.as_ref().filter(|_| matches!(route, Route::Artist(_)));
	let cover: Node<Msg> = if *route == Route::Liked {
		html::div().class("tt-art tt-art-180 tt-art-liked tt-art-fallback").child(icon(icons, Glyph::Heart, 64.0, Tap::NONE)).into()
	} else {
		// An artist's own photo, once the web player has it.
		let url = overview.and_then(|o| o.avatar.as_deref()).or(listing.cover_url.as_deref());
		art(state, url, "tt-art-180", &listing.title, Tap::NONE)
	};
	let cover = if listing.owned {
		html::el("button")
			.class("tt-hero-edit-cover")
			.attr("aria-label", "Edit details")
			.child(cover)
			.child(div("tt-hero-edit-overlay").child(icon(icons, Glyph::Pencil, 32.0, Tap::NONE)).child(html::span("Edit details")))
			.on_click(Msg::RenamePlaylist)
			.into()
	} else {
		cover
	};
	let mut title = html::div().class("tt-hero-title").text(listing.title.clone());
	if listing.owned {
		title = title.title("Edit details").on_click(Msg::RenamePlaylist);
	}
	let kicker = match (route, listing.public) {
		(Route::Playlist(_), Some(true)) => "Public playlist",
		(Route::Playlist(_), Some(false)) => "Private playlist",
		_ => listing.kicker.as_ref(),
	};
	let mut texts = div("tt-hero-text").child(text("tt-kicker", kicker)).child(title);
	if let Some(description) = listing.description.as_deref().filter(|d| !d.is_empty()) {
		texts = texts.child(text("tt-hero-description tt-dim", description));
	}
	if let Some(n) = overview.and_then(|o| o.monthly_listeners) {
		let rank = overview.and_then(|o| o.world_rank).map(|r| format!(" · #{r} in the world")).unwrap_or_default();
		texts = texts.child(text("tt-stats", format!("{} monthly listeners{rank}", grouped(n))));
	}
	let hero = div("tt-hero").child(cover).child(texts.child(text("tt-dim", listing.subtitle.clone())));
	let mut out: Vec<Node<Msg>> = vec![hero.into(), actions(state, icons, listing)];
	if let Some(note) = &listing.note {
		out.push(text("tt-note", note.clone()));
	}
	if matches!(route, Route::Artist(_)) {
		out.extend(artist_body(state, icons, listing));
		return out;
	}
	if listing.loading && listing.tracks.is_empty() {
		out.push(text("tt-empty tt-dim", "Loading…"));
	} else if listing.tracks.is_empty() && listing.editable {
		out.push(text("tt-empty tt-dim", "Nothing here yet. Right-click a song anywhere and choose Add to playlist."));
	} else if !listing.tracks.is_empty() {
		out.push(if matches!(route, Route::Album(_)) && listing.plays.len() == listing.tracks.len() {
			tracks_with(state, &listing.tracks, Middle::Counts(plays_shown(state, &listing.plays)))
		} else {
			tracks(state, &listing.tracks, !matches!(route, Route::Album(_)))
		});
		if listing.next_offset.is_some() {
			let label = if listing.loading { "Loading…" } else { "Show more" };
			out.push(html::el("button").class("tt-pill tt-quiet tt-more").text(label).on_click(Msg::LoadMore).into());
		}
	}
	if let Some(album) = listing.album.as_ref().filter(|_| matches!(route, Route::Album(_)) && !listing.tracks.is_empty()) {
		out.push(release_notes(album));
	}
	out
}

/// Under an album's songs, as in Spotify: the release date, then its © and ℗ lines.
fn release_notes(album: &spotify::Album) -> Node<Msg> {
	let mut notes = div("tt-release");
	if let Some(date) = album.release_date.as_deref().and_then(long_date) {
		notes = notes.child(text("tt-release-date", date));
	}
	for c in &album.copyrights {
		notes = notes.child(text("tt-release-line", c.line()));
	}
	notes.into()
}

/// `2018-03-31` → "March 31, 2018"; a year or year-month stays as given ("2018").
fn long_date(date: &str) -> Option<String> {
	const MONTHS: [&str; 12] =
		["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
	let mut parts = date.splitn(3, '-');
	let year = parts.next().filter(|y| !y.is_empty())?;
	let month = parts.next().and_then(|m| m.parse::<usize>().ok()).and_then(|m| m.checked_sub(1)).and_then(|m| MONTHS.get(m));
	let day = parts.next().and_then(|d| d.parse::<u32>().ok());
	Some(match (month, day) {
		(Some(m), Some(d)) => format!("{m} {d}, {year}"),
		(Some(m), None) => format!("{m} {year}"),
		_ => year.to_owned(),
	})
}

/// Spotify's action row: play (pause while this is what's playing), the shuffle mode,
/// save to Your Library (+ / ✓) for albums and playlists or Follow for artists,
/// and the link back to Spotify.
fn actions(state: &State, icons: &Icons, listing: &Listing) -> Node<Msg> {
	// What's playing comes from this page: its context (playlist, album, artist), or for a list
	// without one (Liked Songs) the list started here, still playing one of its songs.
	let current = listing.context_uri.as_deref().is_some_and(|uri| uri == state.context.uri)
		|| (state.played_from.as_ref() == Some(&state.library.route) && listing.tracks.iter().any(|t| t.uri == state.item.uri));
	let (glyph, label, play) = if current && state.playing() {
		(Glyph::Pause, "Pause", Msg::TogglePlay)
	} else if current {
		(Glyph::Play, "Play", Msg::TogglePlay)
	} else {
		(Glyph::Play, "Play", Msg::PlayListing)
	};
	let shuffle = state.options.shuffle;
	let mut row = div("tt-actions").child(button(icons, glyph, label, play, "tt-big", 24.0)).child(button(
		icons,
		Glyph::Shuffle,
		if shuffle { "Disable shuffle" } else { "Enable shuffle" },
		Msg::ToggleShuffle,
		if shuffle { "tt-on" } else { "" },
		24.0,
	));
	let artist = listing.context_uri.as_deref().is_some_and(|uri| uri.starts_with("spotify:artist:"));
	match listing.followed {
		Some(followed) if artist => {
			row = row.child(
				html::el("button")
					.class(if followed { "tt-pill tt-quiet tt-on" } else { "tt-pill tt-quiet" })
					.text(if followed { "Following" } else { "Follow" })
					.on_click(Msg::ToggleFollow),
			);
		},
		Some(saved) => {
			let (glyph, label) = if saved { (Glyph::Saved, "Remove from Your Library") } else { (Glyph::Save, "Save to Your Library") };
			row = row.child(button(icons, glyph, label, Msg::ToggleFollow, if saved { "tt-like tt-on" } else { "tt-like" }, 28.0));
		},
		None => {},
	}
	if let Some(href) = listing.context_uri.as_deref().and_then(crate::ui::spotify_href) {
		row = row.child(crate::ui::open_spotify(icons, href));
	}
	row.into()
}
