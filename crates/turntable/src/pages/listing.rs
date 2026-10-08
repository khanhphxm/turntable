//! A playlist, album or Liked Songs: header, action bar, then the track table (virtualised;
//! the next page loads as the end comes into view).

use gpui_kit::{
	AnyElement, Context, FontWeight, IntoElement, ParentElement as _, Styled as _, Window, div, list, prelude::FluentBuilder as _, px,
};
use turntable_core::{Msg, Route, spotify::id_of};

use crate::{
	components::Column,
	shell::Shell,
	theme,
	widgets::{Icon, icon_button},
};

impl Shell {
	pub fn listing_page(&mut self, route: Route, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let rows = self.st().library.listing.as_ref().map_or(0, |l| l.tracks.len());
		let album = matches!(route, Route::Album(_));
		// The list's items: the header, the rows, then a footer (loading / show more).
		if self.list_for.as_ref() != Some(&(route.clone(), rows)) {
			if self.list_for.as_ref().is_some_and(|(r, _)| *r == route) {
				// More rows arrived for the same page: keep the scroll position.
				self.list.splice(0..self.list.item_count(), rows + 2);
			} else {
				self.list.reset(rows + 2);
			}
			self.list_for = Some((route, rows));
		}
		// The footer's height follows the finder and its results: measure it afresh each frame.
		self.list.remeasure_items(rows + 1..rows + 2);
		list(
			self.list.clone(),
			cx.processor(move |this, ix: usize, window, cx| {
				if ix == 0 {
					return this.listing_header(cx);
				}
				let i = ix - 1;
				let Some(t) = this.st().library.listing.as_ref().and_then(|l| l.tracks.get(i)).cloned() else {
					return this.listing_footer(window, cx);
				};
				let plays = this.st().library.listing.as_ref().and_then(|l| l.plays.get(i).copied());
				let (column, plays) = if album {
					// An album's rows show play counts when known (and the setting allows).
					let shown = this.st().prefs.display.play_counts && plays.is_some();
					(if shown { Column::Plays } else { Column::None }, plays)
				} else {
					(Column::Album, None)
				};
				div().px_6().child(this.track_row(i, &t, column, plays, !album, cx)).into_any_element()
			}),
		)
		.size_full()
	}

	fn listing_footer(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> AnyElement {
		let Some(l) = self.st().library.listing.as_ref() else {
			return div().into_any_element();
		};
		// Loading, or why there's nothing (the page failed, or offline without a saved copy).
		let text: Option<String> = if l.loading && l.tracks.is_empty() {
			Some("Loading…".to_owned())
		} else if l.tracks.is_empty() && !l.editable {
			Some(self.st().library.error.clone().unwrap_or_else(|| {
				if self.st().offline {
					"You're offline, and this page isn't saved on this computer.".to_owned()
				} else {
					"Nothing here.".to_owned()
				}
			}))
		} else {
			None
		};
		let empty = l.tracks.is_empty();
		// The user's own playlist ends with the finder (open while it's empty).
		let own = match &self.st().library.route {
			Route::Playlist(id) if l.editable && !l.loading => Some(id.clone()),
			_ => None,
		};
		let more = l.next_offset.is_some();
		let loading = l.loading;
		if more && !loading {
			// The end came into view: fetch the next page (Spotify's lists never need a click).
			self.send(Msg::LoadMore, cx);
		}
		div()
			.px_6()
			.pt_4()
			.pb_10()
			.text_size(px(14.))
			.text_color(theme::subdued())
			.when_some(text, |s, t| s.child(t))
			.when(more, |s| s.child("Loading more…"))
			.when_some(own.filter(|_| !more), |s, id| s.child(self.finder(&id, empty, window, cx)))
			.into_any_element()
	}

	fn listing_header(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
		let Some(l) = self.st().library.listing.as_ref() else {
			return div().into_any_element();
		};
		let route = self.st().library.route.clone();
		let kicker = match (&route, l.public) {
			(Route::Playlist(_), Some(true)) => "Public Playlist".to_owned(),
			(Route::Playlist(_), Some(false)) => "Private Playlist".to_owned(),
			_ => l.kicker.to_string(),
		};
		let (title, cover_url, context) = (l.title.clone(), l.cover_url.clone(), l.context_uri.clone());
		// Spotify ends the line with the total length: the album's own, or the sum of the rows
		// once every page is in.
		let total = l.duration_ms.or_else(|| l.next_offset.is_none().then(|| l.tracks.iter().map(|t| u64::from(t.duration_ms)).sum()));
		let subtitle = match total.filter(|ms| *ms > 0) {
			Some(ms) => format!("{}, {}", l.subtitle, length(ms)),
			None => l.subtitle.clone(),
		};
		// Spotify stores a missing description as the text "null" on some playlists.
		let description = l.description.clone().filter(|d| !d.is_empty() && d != "null");
		let note = l.note.clone();
		let tint = Self::tint(l.accent.as_deref());
		// An album names its artist as a link.
		let artist = l.album.as_ref().and_then(|a| a.artists.first()).map(|a| (a.name.clone(), id_of(&a.uri).to_owned()));
		let dim = theme::alpha(theme::text(), 0.7);
		let meta = div()
			.flex()
			.flex_col()
			.gap_2()
			.when_some(description, |s, d| s.child(div().text_color(dim).line_clamp(2).child(d)))
			.child(
				div()
					.flex()
					.items_center()
					.gap_1()
					.when_some(artist, |s, (name, id)| {
						s.child(
							self.link("header-artist", name, Route::Artist(id), cx).text_color(theme::text()).font_weight(FontWeight::BOLD),
						)
						.child(div().text_color(dim).child("•"))
					})
					.child(div().text_color(dim).child(subtitle)),
			)
			.into_any_element();
		let shuffle = self.st().options.shuffle;
		let actions =
			icon_button("page-shuffle", Icon::Shuffle, px(32.), shuffle, cx.listener(|this, _, _, cx| this.send(Msg::ToggleShuffle, cx)));
		let column = if matches!(route, Route::Album(_)) { Column::None } else { Column::Album };
		div()
			.w_full()
			.flex()
			.flex_col()
			.child(self.page_header(&route, cover_url.as_deref(), tint, kicker.into(), title.into(), meta, cx))
			.child(
				self.wash(tint)
					.child(self.action_bar(context, actions, cx))
					.when_some(note, |s, n| s.child(div().px_6().pb_4().text_size(px(14.)).text_color(theme::subdued()).child(n)))
					.child(div().px_6().child(self.table_head(column)))
					.child(div().h_4()),
			)
			.into_any_element()
	}
}

/// "2 hr 31 min", "48 min", "3 min 20 sec": Spotify's header lengths.
fn length(ms: u64) -> String {
	let secs = ms / 1000;
	let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
	match (h, m) {
		(0, 0) => format!("{s} sec"),
		(0, m) if m < 10 => format!("{m} min {s} sec"),
		(0, m) => format!("{m} min"),
		(h, m) => format!("{h} hr {m} min"),
	}
}
