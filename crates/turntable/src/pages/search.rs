//! Search, as Spotify has it: typing in the top bar shows suggestions under the field; Enter
//! opens this page, with filter chips, a top result and the matches.

use gpui_kit::{
	AnyElement, Context, Div, FontWeight, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _,
	StatefulInteractiveElement as _, Styled as _, deferred, div, prelude::FluentBuilder as _, px,
};
use turntable_core::{
	Msg, Route,
	library::Tile,
	spotify::{SearchResults, Track, id_of, pick_image},
};

use crate::{
	components::{Column, artists},
	shell::Shell,
	theme,
	widgets::{Icon, added_badge, chip, icon, outline_button, play_disc},
};

/// The results page's filter chips.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
	All,
	Songs,
	Albums,
	Artists,
	Playlists,
}

const FILTERS: [(Filter, &str); 5] = [
	(Filter::All, "All"),
	(Filter::Songs, "Songs"),
	(Filter::Albums, "Albums"),
	(Filter::Artists, "Artists"),
	(Filter::Playlists, "Playlists"),
];

/// Rows under the search field while typing.
const SUGGESTIONS: usize = 6;
/// Rows of the "All" list under the top result.
const MIXED: usize = 12;

/// One match in a mixed list: a song (its row in the results) or a page.
#[derive(Clone)]
enum Hit {
	Song(usize, Track),
	Page(Tile),
}

impl Hit {
	fn kind(&self) -> &'static str {
		match self {
			Self::Song(..) => "Song",
			Self::Page(t) => match t.route {
				Route::Artist(_) => "Artist",
				Route::Album(_) => "Album",
				_ => "Playlist",
			},
		}
	}
}

/// The best match first (an artist named just like the query, else the first song), then
/// songs and pages interleaved as Spotify mixes them.
fn mixed(results: &SearchResults, query: &str, limit: usize) -> Vec<Hit> {
	let q = query.trim().to_lowercase();
	let mut out = Vec::new();
	let exact_artist = results.artists.items.first().is_some_and(|a| a.name.to_lowercase() == q);
	let mut s = results.tracks.items.iter().enumerate().map(|(i, t)| Hit::Song(i, t.clone()));
	let mut a = results.artists.items.iter().map(|a| Hit::Page(Tile::artist(a)));
	let mut al = results.albums.items.iter().map(|a| Hit::Page(Tile::album(a)));
	let mut p = results.playlists.items.iter().map(|p| Hit::Page(Tile::playlist(p)));
	if exact_artist {
		out.extend(a.next());
	}
	while out.len() < limit {
		let before = out.len();
		out.extend(s.next());
		out.extend(s.next());
		out.extend(a.next());
		out.extend(al.next());
		out.extend(p.next());
		if out.len() == before {
			break;
		}
	}
	out.truncate(limit);
	out
}

/// The small grey label naming what a row is.
fn kind_pill(kind: &'static str) -> Div {
	div()
		.px_1p5()
		.py_0p5()
		.rounded(px(4.))
		.bg(theme::raised_hover())
		.text_size(px(11.))
		.font_weight(FontWeight::BOLD)
		.text_color(theme::subdued())
		.child(kind)
}

impl Shell {
	/// What a row's artwork is: an album's cover for songs, the page's own otherwise.
	fn hit_art(&self, hit: &Hit, size: u32, cx: &mut Context<'_, Self>) -> Div {
		match hit {
			Hit::Song(_, t) => {
				let url = t.album.as_ref().and_then(|a| pick_image(&a.images, 64)).map(|i| i.url.clone());
				crate::widgets::cover(self.image(url.as_deref(), size, cx), theme::RAISED, px(size as f32), px(4.))
			},
			Hit::Page(t) => self.art(&t.route, t.image.as_deref(), px(size as f32), cx),
		}
	}

	fn hit_text(hit: &Hit) -> (String, String) {
		match hit {
			Hit::Song(_, t) => (t.name.clone(), artists(t)),
			Hit::Page(t) => (t.title.clone(), if matches!(t.route, Route::Artist(_)) { String::new() } else { t.sub.clone() }),
		}
	}

	/// Opens a match: a page goes to its route, a song opens its album with the song picked.
	fn open_hit(&mut self, hit: &Hit, cx: &mut Context<'_, Self>) {
		let tile = match hit {
			Hit::Page(t) => Some(t.clone()),
			Hit::Song(_, t) => Tile::track(t),
		};
		if let Some(tile) = tile {
			self.send(Msg::RememberSearch(tile), cx);
		}
		match hit {
			Hit::Page(t) => self.go(t.route.clone(), cx),
			Hit::Song(_, t) => {
				if let Some(album) = &t.album {
					self.send(Msg::OpenTrack { album: id_of(&album.uri).to_owned(), uri: t.uri.clone() }, cx);
					self.go(Route::Album(id_of(&album.uri).to_owned()), cx);
				}
			},
		}
	}

	/// Suggestions under the top bar's field while it's focused and has text.
	pub fn search_dropdown(&mut self, cx: &mut Context<'_, Self>) -> Option<AnyElement> {
		if !self.search_open {
			return None;
		}
		let typed = self.search.read(cx).value().trim().to_owned();
		if typed.is_empty() {
			return self.recent_dropdown(cx);
		}
		let st = self.st();
		let hits = st
			.library
			.search
			.as_ref()
			.filter(|_| st.library.query.trim() == typed)
			.map(|r| mixed(r, &typed, SUGGESTIONS))
			.unwrap_or_default();
		let rows: Vec<AnyElement> = hits
			.iter()
			.enumerate()
			.map(|(i, hit)| {
				let (title, sub) = Self::hit_text(hit);
				let sub = if sub.is_empty() { hit.kind().to_owned() } else { format!("{} • {sub}", hit.kind()) };
				let target = hit.clone();
				let round = matches!(hit, Hit::Page(t) if matches!(t.route, Route::Artist(_)));
				div()
					.id(("suggest", i))
					.h(px(56.))
					.px_3()
					.flex()
					.items_center()
					.gap_3()
					.rounded(px(4.))
					.hover(|s| s.bg(theme::hover()))
					.cursor_pointer()
					// Mouse-down, not click: the field's blur would close the list first.
					.on_mouse_down(
						MouseButton::Left,
						cx.listener(move |this, _, _, cx| {
							this.search_open = false;
							this.open_hit(&target, cx);
						}),
					)
					.child(self.hit_art(hit, 40, cx).when(round, |s| s.rounded_full()))
					.child(
						div()
							.flex_1()
							.min_w_0()
							.flex()
							.flex_col()
							.child(div().text_size(px(15.)).truncate().child(title))
							.child(div().text_size(px(13.)).text_color(theme::subdued()).truncate().child(sub)),
					)
					.into_any_element()
			})
			.collect();
		let all = div()
			.id("suggest-all")
			.h(px(44.))
			.px_3()
			.flex()
			.items_center()
			.gap_3()
			.rounded(px(4.))
			.hover(|s| s.bg(theme::hover()))
			.cursor_pointer()
			.on_mouse_down(
				MouseButton::Left,
				cx.listener(move |this, _, _, cx| {
					this.search_open = false;
					let text = this.search.read(cx).value().trim().to_owned();
					this.send(Msg::SearchSubmit(text), cx);
					this.go(Route::Search, cx);
				}),
			)
			.child(div().size(px(40.)).flex().items_center().justify_center().child(icon(Icon::Search, px(18.), theme::subdued())))
			.child(div().text_size(px(15.)).truncate().child(format!("Search for “{typed}”")));
		Some(
			deferred(
				div()
					.absolute()
					.top(px(52.))
					.left_0()
					.right_0()
					.p_2()
					.flex()
					.flex_col()
					.rounded(px(8.))
					.bg(theme::raised())
					.shadow_2xl()
					.when(rows.is_empty(), |s| {
						s.child(div().px_3().py_2().text_size(px(14.)).text_color(theme::subdued()).child("Searching…"))
					})
					.children(rows)
					.child(all),
			)
			.with_priority(1)
			.into_any_element(),
		)
	}

	/// The empty field's list: Recent searches, each removable, as Spotify has it.
	fn recent_dropdown(&mut self, cx: &mut Context<'_, Self>) -> Option<AnyElement> {
		let recent = self.st().recent_searches().to_vec();
		if recent.is_empty() {
			return None;
		}
		let rows: Vec<AnyElement> = recent
			.iter()
			.enumerate()
			.map(|(i, t)| {
				let hit = Hit::Page(t.clone());
				let round = matches!(t.route, Route::Artist(_));
				let (open, forget) = (t.clone(), t.route.clone());
				div()
					.id(("recent", i))
					.group("recent")
					.h(px(56.))
					.px_3()
					.flex()
					.items_center()
					.gap_3()
					.rounded(px(4.))
					.hover(|s| s.bg(theme::hover()))
					.cursor_pointer()
					.on_mouse_down(
						MouseButton::Left,
						cx.listener(move |this, _, _, cx| {
							this.search_open = false;
							this.send(Msg::RememberSearch(open.clone()), cx);
							if let Some(uri) = open.play.as_deref().filter(|u| u.starts_with("spotify:track:"))
								&& let Route::Album(album) = &open.route
							{
								this.send(Msg::OpenTrack { album: album.clone(), uri: uri.to_owned() }, cx);
							}
							this.go(open.route.clone(), cx);
						}),
					)
					.child(self.hit_art(&hit, 40, cx).when(round, |s| s.rounded_full()))
					.child(
						div()
							.flex_1()
							.min_w_0()
							.flex()
							.flex_col()
							.child(div().text_size(px(15.)).truncate().child(t.title.clone()))
							.child(div().text_size(px(13.)).text_color(theme::subdued()).truncate().child(t.sub.clone())),
					)
					.child(
						div()
							.id(("recent-forget", i))
							.invisible()
							.group_hover("recent", |s| s.visible())
							.on_mouse_down(
								MouseButton::Left,
								cx.listener(move |this, _, _, cx| {
									cx.stop_propagation();
									this.send(Msg::ForgetSearch(forget.clone()), cx);
								}),
							)
							.child(icon(Icon::Close, px(16.), theme::subdued())),
					)
					.into_any_element()
			})
			.collect();
		Some(
			deferred(
				div()
					.absolute()
					.top(px(52.))
					.left_0()
					.right_0()
					.p_2()
					.flex()
					.flex_col()
					.rounded(px(8.))
					.bg(theme::raised())
					.shadow_2xl()
					.child(div().px_3().py_2().text_size(px(16.)).font_weight(FontWeight::BOLD).child("Recent searches"))
					.children(rows),
			)
			.with_priority(1)
			.into_any_element(),
		)
	}

	pub fn search_page(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let filter = self.search_filter;
		let chips = div().flex().gap_2().children(FILTERS.iter().enumerate().map(|(i, (f, label))| {
			let f = *f;
			chip(
				("search-filter", i),
				*label,
				f == filter,
				cx.listener(move |this, _, _, cx| {
					this.search_filter = f;
					cx.notify();
				}),
			)
		}));
		let page = div().id("search-page").size_full().overflow_y_scroll().px_6().pt_4().pb_8().flex().flex_col().gap_6();
		let query = self.st().library.query.trim().to_owned();
		let Some(results) = self.st().library.search.clone().filter(|_| !query.is_empty()) else {
			return page.child(
				div()
					.pt_16()
					.flex()
					.flex_col()
					.items_center()
					.gap_3()
					.child(icon(Icon::Search, px(48.), theme::subdued()))
					.child(div().text_size(px(24.)).font_weight(FontWeight::BOLD).child("Search Spotify"))
					.child(
						div()
							.text_size(px(16.))
							.text_color(theme::subdued())
							.child("Songs, artists, albums and playlists. ⌘K searches from anywhere."),
					),
			);
		};
		let empty = results.tracks.items.is_empty()
			&& results.artists.items.is_empty()
			&& results.albums.items.is_empty()
			&& results.playlists.items.is_empty();
		if empty {
			return page.child(
				div()
					.pt_16()
					.flex()
					.flex_col()
					.items_center()
					.gap_2()
					.child(div().text_size(px(24.)).font_weight(FontWeight::BOLD).child(format!("No results found for “{query}”")))
					.child(
						div()
							.text_size(px(16.))
							.text_color(theme::subdued())
							.child("Check the spelling, or try fewer or different keywords."),
					),
			);
		}
		let page = page.child(chips);
		match filter {
			Filter::All => {
				let mut hits = mixed(&results, &query, MIXED + 1).into_iter();
				let top = hits.next();
				let rows: Vec<AnyElement> = hits.enumerate().map(|(i, h)| self.hit_row(i, &h, cx)).collect();
				page.when_some(top, |s, t| s.child(self.top_banner(&t, cx))).child(div().flex().flex_col().children(rows))
			},
			Filter::Songs => {
				let rows: Vec<AnyElement> =
					results.tracks.items.iter().enumerate().map(|(i, t)| self.track_row(i, t, Column::Album, None, true, cx)).collect();
				page.child(div().child(self.table_head(Column::Album))).child(div().flex().flex_col().children(rows))
			},
			Filter::Albums | Filter::Artists | Filter::Playlists => {
				let tiles: Vec<Tile> = match filter {
					Filter::Albums => results.albums.items.iter().map(Tile::album).collect(),
					Filter::Artists => results.artists.items.iter().map(Tile::artist).collect(),
					_ => results.playlists.items.iter().map(Tile::playlist).collect(),
				};
				let cards: Vec<_> = tiles.iter().enumerate().map(|(i, t)| self.card(("search-card", i), t, cx)).collect();
				page.child(div().mx(px(-12.)).flex().flex_wrap().children(cards))
			},
		}
	}

	/// The top result: a wide banner with its art, name and kind, ✓/⊕ for songs and a play disc.
	fn top_banner(&mut self, hit: &Hit, cx: &mut Context<'_, Self>) -> AnyElement {
		let (title, sub) = Self::hit_text(hit);
		let sub = if sub.is_empty() { hit.kind().to_owned() } else { format!("{} • {sub}", hit.kind()) };
		let round = matches!(hit, Hit::Page(t) if matches!(t.route, Route::Artist(_)));
		let (playing, play): (bool, Msg) = match hit {
			Hit::Song(i, t) => (self.st().playing() && self.st().item.uri == t.uri, Msg::PlayRow(*i)),
			Hit::Page(t) => (self.playing_context(t.play.as_deref()), Msg::PlayContext(t.play.clone().unwrap_or_default())),
		};
		let added = match hit {
			Hit::Song(_, t) => Some((self.st().added(&t.uri), t.uri.clone())),
			Hit::Page(_) => None,
		};
		let target = hit.clone();
		div()
			.id("top-result")
			.group("top-result")
			.h(px(88.))
			.px_4()
			.flex()
			.items_center()
			.gap_4()
			.rounded(px(8.))
			.bg(theme::raised())
			.hover(|s| s.bg(theme::raised_hover()))
			.cursor_pointer()
			.on_click(cx.listener(move |this, _, _, cx| this.open_hit(&target, cx)))
			.child(self.hit_art(hit, 60, cx).when(round, |s| s.rounded_full()).shadow_lg())
			.child(
				div()
					.flex_1()
					.min_w_0()
					.flex()
					.flex_col()
					.child(div().text_size(px(24.)).font_weight(FontWeight::BOLD).truncate().child(title))
					.child(div().text_size(px(14.)).text_color(theme::subdued()).truncate().child(sub)),
			)
			.when_some(added, |s, (added, uri)| {
				s.child(
					div()
						.id("top-add")
						.cursor_pointer()
						.on_click(cx.listener(move |this, _, _, cx| {
							cx.stop_propagation();
							if !added {
								this.send(Msg::AddToLiked(uri.clone()), cx);
							}
						}))
						.child(added_badge(added, px(20.))),
				)
			})
			.child(play_disc(
				"top-play",
				px(48.),
				playing,
				cx.listener(move |this, _, _, cx| {
					cx.stop_propagation();
					this.send(play.clone(), cx);
				}),
			))
			.into_any_element()
	}

	/// A row of the "All" list: art, name and artists, what it is, and its action.
	fn hit_row(&mut self, i: usize, hit: &Hit, cx: &mut Context<'_, Self>) -> AnyElement {
		let (title, sub) = Self::hit_text(hit);
		let round = matches!(hit, Hit::Page(t) if matches!(t.route, Route::Artist(_)));
		let action: AnyElement = match hit {
			Hit::Song(_, t) => {
				let (added, uri) = (self.st().added(&t.uri), t.uri.clone());
				div()
					.id(("hit-add", i))
					.cursor_pointer()
					.when(!added, |s| s.invisible().group_hover("hit", |s| s.visible()))
					.on_click(cx.listener(move |this, _, _, cx| {
						cx.stop_propagation();
						if !added {
							this.send(Msg::AddToLiked(uri.clone()), cx);
						}
					}))
					.child(added_badge(added, px(16.)))
					.into_any_element()
			},
			Hit::Page(t) => match &t.route {
				Route::Artist(id) => {
					let uri = format!("spotify:artist:{id}");
					let following = self.st().library.followed.iter().any(|a| a.uri == uri);
					let (id, name) = (id.clone(), t.title.clone());
					outline_button(("hit-follow", i), if following { "Following" } else { "Follow" })
						.on_click(cx.listener(move |this, _, _, cx| {
							cx.stop_propagation();
							this.send(Msg::FollowArtist(id.clone(), name.clone()), cx);
						}))
						.into_any_element()
				},
				_ => div().into_any_element(),
			},
		};
		let target = hit.clone();
		div()
			.id(("hit", i))
			.group("hit")
			.h(px(64.))
			.px_2()
			.flex()
			.items_center()
			.gap_3()
			.rounded(px(4.))
			.hover(|s| s.bg(theme::hover()))
			.cursor_pointer()
			.on_click(cx.listener(move |this, e: &gpui_kit::ClickEvent, _, cx| match &target {
				// A song plays on double-click, as in every track list; pages open on click.
				Hit::Song(row, _) if e.click_count() >= 2 => this.send(Msg::PlayRow(*row), cx),
				Hit::Song(row, _) => this.send(Msg::Select(*row), cx),
				Hit::Page(_) => this.open_hit(&target, cx),
			}))
			.child(self.hit_art(hit, 48, cx).when(round, |s| s.rounded_full()))
			.child(
				div()
					.flex_1()
					.min_w_0()
					.flex()
					.flex_col()
					.child(div().text_size(px(16.)).truncate().child(title))
					.when(!sub.is_empty(), |s| s.child(div().text_size(px(14.)).text_color(theme::subdued()).truncate().child(sub))),
			)
			.child(div().w(px(120.)).flex().child(kind_pill(hit.kind())))
			.child(div().w(px(100.)).flex().justify_end().child(action))
			.into_any_element()
	}
}
