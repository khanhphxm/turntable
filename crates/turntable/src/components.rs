//! Composite pieces reused across pages: the track row, cards and shelves, and page headers.
//! Every page builds from these, so a track looks and behaves the same wherever it's listed.

use gpui_kit::{
	AnyElement, ClickEvent, Context, Div, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, SharedString,
	Stateful, StatefulInteractiveElement as _, Styled as _, div, linear_color_stop, linear_gradient, prelude::FluentBuilder as _, px, rgb,
};

use gpui_kit::{WeakEntity, component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem}};

use crate::{
	mock::Collection,
	shell::{Route, Shell},
	theme,
	widgets::{Icon, added_badge, cover, icon, icon_button, play_disc, section_title},
};

pub fn clock(ms: u64) -> String {
	let s = ms / 1000;
	format!("{}:{:02}", s / 60, s % 60)
}

/// 1234567 → "1,234,567".
pub fn grouped(n: u64) -> String {
	let s = n.to_string();
	let mut out = String::with_capacity(s.len() + s.len() / 3);
	for (i, c) in s.chars().enumerate() {
		if i > 0 && (s.len() - i) % 3 == 0 {
			out.push(',');
		}
		out.push(c);
	}
	out
}

/// Which optional column a track table shows between the title and the duration.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Column {
	/// The album (playlists, Liked Songs, search).
	Album,
	/// Play count (an artist's popular tracks).
	Plays,
	/// Nothing (albums: every row is the same album).
	None,
}

/// Track table geometry, shared by the column header and the rows so they line up.
pub const INDEX_W: f32 = 24.;
pub const TAIL_W: f32 = 120.;
pub const ROW_PAD_X: f32 = 16.;

/// One track in a table.
pub struct Row {
	/// Unique within the list (element ids).
	pub key:    usize,
	/// Where it's listed (and what plays from it).
	pub from:   Collection,
	/// Its position there.
	pub pos:    usize,
	/// Index into the library's tracks.
	pub track:  usize,
	pub column: Column,
	/// Small cover beside the title (not on album pages).
	pub cover:  bool,
}

impl Shell {
	/// A collection's artwork at `size`: round for artists, Liked Songs' heart on its gradient.
	pub fn art(&self, c: Collection, size: Pixels) -> Div {
		match c {
			Collection::Liked => div()
				.size(size)
				.flex_none()
				.rounded(px(4.))
				.bg(linear_gradient(135., linear_color_stop(rgb(0x450af5), 0.), linear_color_stop(rgb(0xc4efd9), 1.)))
				.flex()
				.items_center()
				.justify_center()
				.child(icon(Icon::Heart, size * 0.42, theme::text())),
			Collection::Artist(_) => cover(None, self.lib.color_of(c), size, size / 2.),
			_ => cover(None, self.lib.color_of(c), size, if size > px(100.) { px(6.) } else { px(4.) }),
		}
	}

	/// The column header above a track table.
	pub fn table_head(&self, column: Column) -> Div {
		div()
			.h(px(36.))
			.flex()
			.items_center()
			.gap_4()
			.px(px(ROW_PAD_X))
			.border_b_1()
			.border_color(theme::divider())
			.text_size(px(14.))
			.text_color(theme::subdued())
			.child(div().w(px(INDEX_W)).flex().justify_end().child("#"))
			.child(div().flex_1().child("Title"))
			.when(column == Column::Album, |s| s.child(div().flex_1().child("Album")))
			.when(column == Column::Plays, |s| s.child(div().flex_1().flex().justify_end().child("Plays")))
			.child(div().w(px(TAIL_W)).flex().justify_end().pr(px(40.)).child(icon(Icon::Clock, px(16.), theme::subdued())))
	}

	/// A track row: number (play on hover), cover, title and artist, optional column, the ✓ and
	/// the duration. Click selects, double-click plays from `row.from`.
	pub fn track_row(&self, row: Row, cx: &mut Context<'_, Self>) -> AnyElement {
		let Some(t) = self.lib.tracks.get(row.track) else {
			return div().into_any_element();
		};
		let shell = cx.entity().downgrade();
		let track = row.track;
		let current = self.is_current(row.from, row.pos, row.track);
		let playing = current && self.playing;
		let selected = self.selected == Some(row.key);
		let key = row.key;
		let added = self.lib.added(row.track);
		let artist = t.artist;
		let album = t.album;
		let (from, pos) = (row.from, row.pos);
		div()
			.id(("row", row.key))
			.group("row")
			.w_full()
			.h(theme::ROW_H)
			.flex()
			.items_center()
			.gap_4()
			.px(px(ROW_PAD_X))
			.rounded(px(4.))
			.text_size(px(14.))
			.text_color(theme::subdued())
			.when(selected, |s| s.bg(theme::selected()))
			.when(!selected, |s| s.hover(|s| s.bg(theme::hover())))
			.on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
				if e.click_count() >= 2 {
					this.play_from(from, pos, cx);
				} else {
					this.selected = Some(key);
					cx.notify();
				}
			}))
			.child(
				div()
					.w(px(INDEX_W))
					.flex()
					.justify_end()
					.items_center()
					.relative()
					.child(
						div()
							.text_size(px(16.))
							.when(current, |s| s.text_color(theme::accent()))
							.group_hover("row", |s| s.invisible())
							.map(|s| {
								if playing {
									s.child(icon(Icon::Volume, px(16.), theme::accent()))
								} else {
									s.child((row.key + 1).to_string())
								}
							}),
					)
					.child(
						div()
							.id(("row-play", row.key))
							.absolute()
							.right_0()
							.invisible()
							.group_hover("row", |s| s.visible())
							.on_click(cx.listener(move |this, _, _, cx| {
								cx.stop_propagation();
								if this.is_current(from, pos, usize::MAX) {
									this.toggle_play(cx);
								} else {
									this.play_from(from, pos, cx);
								}
							}))
							.child(icon(if playing { Icon::Pause } else { Icon::Play }, px(16.), theme::text())),
					),
			)
			.child(
				div()
					.flex_1()
					.min_w_0()
					.flex()
					.items_center()
					.gap_3()
					.when(row.cover, |s| s.child(cover(None, self.lib.track_color(row.track), px(40.), px(4.))))
					.child(
						div()
							.min_w_0()
							.flex()
							.flex_col()
							.child(
								div()
									.text_size(px(16.))
									.text_color(if current { theme::accent() } else { theme::text() })
									.truncate()
									.child(t.title),
							)
							.child(self.link(("row-artist", row.key), self.lib.artist_name(artist).to_owned(), Route::Artist(artist), cx)),
					),
			)
			.when(row.column == Column::Album, |s| {
				s.child(div().flex_1().min_w_0().flex().child(self.link(
					("row-album", row.key),
					self.lib.albums.get(album).map_or("", |a| a.name).to_owned(),
					Route::Collection(Collection::Album(album)),
					cx,
				)))
			})
			.when(row.column == Column::Plays, |s| s.child(div().flex_1().flex().justify_end().child(grouped(t.plays))))
			.child(
				div()
					.w(px(TAIL_W))
					.flex()
					.items_center()
					.justify_end()
					.gap_3()
					.child(
						div()
							.id(("row-add", row.key))
							.cursor_pointer()
							.when(!added, |s| s.invisible().group_hover("row", |s| s.visible()))
							.on_click(cx.listener(move |this, _, _, cx| {
								cx.stop_propagation();
								this.toggle_liked(row.track, cx);
							}))
							.child(added_badge(added, px(16.))),
					)
					.child(div().w(px(36.)).flex().justify_end().child(clock(t.ms)))
					.child(div().invisible().group_hover("row", |s| s.visible()).child(icon(Icon::Ellipsis, px(16.), theme::subdued()))),
			)
			.context_menu(move |menu, _, _| track_menu(menu, &shell, artist, album, track))
			.into_any_element()
	}

	/// Subdued text that underlines and turns white on hover and opens `route`.
	pub fn link(
		&self,
		id: impl Into<gpui_kit::ElementId>,
		text: impl Into<SharedString>,
		route: Route,
		cx: &mut Context<'_, Self>,
	) -> Stateful<Div> {
		div()
			.id(id)
			.min_w_0()
			.truncate()
			.cursor_pointer()
			.hover(|s| s.underline().text_color(theme::text()))
			.on_click(cx.listener(move |this, _, _, cx| {
				cx.stop_propagation();
				this.open(route, cx);
			}))
			.child(text.into())
	}

	/// A card: artwork (round for artists), title and subtitle; a play disc rises in on hover.
	pub fn card(
		&self,
		id: impl Into<gpui_kit::ElementId>,
		c: Collection,
		subtitle: impl Into<SharedString>,
		cx: &mut Context<'_, Self>,
	) -> Stateful<Div> {
		let playing = self.playing && self.now.is_some_and(|n| n.from == c);
		let route = c.route();
		let id = id.into();
		div()
			.id(id)
			.group("card")
			.w(px(theme::CARD_W))
			.flex_none()
			.p(px(12.))
			.flex()
			.flex_col()
			.gap_2()
			.rounded(px(6.))
			.hover(|s| s.bg(theme::hover()))
			.cursor_pointer()
			.on_click(cx.listener(move |this, _, _, cx| this.open(route, cx)))
			.child(div().relative().child(self.art(c, px(theme::CARD_W - 24.)).shadow_lg()).child(
				div().absolute().right(px(8.)).bottom(px(8.)).when(!playing, |s| s.invisible().group_hover("card", |s| s.visible())).child(
					play_disc(
						("card-play", c.key()),
						px(48.),
						playing,
						cx.listener(move |this, _, _, cx| {
							cx.stop_propagation();
							this.play_collection(c, cx);
						}),
					),
				),
			))
			.child(
				div()
					.flex()
					.flex_col()
					.gap_0p5()
					.child(div().text_size(px(16.)).truncate().child(self.lib.name_of(c).to_owned()))
					.child(div().text_size(px(14.)).text_color(theme::subdued()).truncate().child(subtitle.into())),
			)
	}

	/// A titled row of cards, as many as fit (Spotify's shelves don't scroll sideways).
	pub fn shelf(&self, title: impl Into<SharedString>, cards: Vec<Stateful<Div>>) -> Div {
		div()
			.flex()
			.flex_col()
			.gap_2()
			.child(div().px(px(12.)).child(section_title(title)))
			.child(div().h(px(theme::CARD_W + 70.)).overflow_hidden().flex().flex_wrap().children(cards))
	}

	/// The tinted header every collection and artist page opens with.
	pub fn page_header(&self, c: Collection, kicker: &'static str, title: SharedString, meta: AnyElement) -> Div {
		let color = self.lib.color_of(c);
		// Spotify sizes the title to the room it has: the largest step at which it fits on one
		// line (bold glyphs average ~0.6 em), else the smallest, which may wrap to two.
		let room = self.main_w - 232. - 3. * 24.;
		let chars = title.chars().count().max(1) as f32;
		let size = [96., 72., 48., 32.].into_iter().find(|s| chars * s * 0.6 <= room).unwrap_or(32.);
		div()
			.h(px(320.))
			.w_full()
			.flex()
			.items_end()
			.gap_6()
			.p_6()
			.bg(linear_gradient(180., linear_color_stop(rgb(color), 0.), linear_color_stop(theme::alpha(rgb(color), 0.55), 1.)))
			.child(self.art(c, px(232.)).shadow_2xl())
			.child(
				div()
					.flex_1()
					.min_w_0()
					.flex()
					.flex_col()
					.gap_2()
					.child(div().text_size(px(14.)).child(kicker))
					.child(
						div()
							.text_size(px(size))
							.line_height(px(size * 1.1))
							.font_weight(FontWeight::EXTRA_BOLD)
							.line_clamp(2)
							.child(title),
					)
					.child(div().text_size(px(14.)).child(meta)),
			)
	}

	/// The strip under a header: the big play disc, then the page's own actions.
	pub fn action_bar(&self, c: Collection, actions: impl IntoElement, cx: &mut Context<'_, Self>) -> Div {
		let playing = self.playing && self.now.is_some_and(|n| n.from == c);
		div()
			.flex()
			.items_center()
			.gap_6()
			.px_6()
			.py_6()
			.child(play_disc("page-play", px(56.), playing, cx.listener(move |this, _, _, cx| this.play_collection(c, cx))))
			.child(actions)
			.child(icon_button("page-more", Icon::Ellipsis, px(32.), false, |_, _, _| {}))
	}

	/// The colour wash behind a page's action bar and first rows.
	pub fn wash(&self, c: Collection) -> Div {
		div().bg(linear_gradient(
			180.,
			linear_color_stop(theme::alpha(rgb(self.lib.color_of(c)), 0.35), 0.),
			linear_color_stop(theme::panel(), 1.),
		))
	}
}

impl Collection {
	/// The page that shows it.
	pub fn route(self) -> Route {
		match self {
			Self::Artist(a) => Route::Artist(a),
			other => Route::Collection(other),
		}
	}

	/// A stable small number per collection, for element ids.
	pub fn key(self) -> usize {
		match self {
			Self::Liked => 0,
			Self::Playlist(i) => 1 + i * 4,
			Self::Album(i) => 2 + i * 4,
			Self::Artist(i) => 3 + i * 4,
		}
	}
}

/// Right-click on a track.
fn track_menu(menu: PopupMenu, shell: &WeakEntity<Shell>, artist: usize, album: usize, track: usize) -> PopupMenu {
	let item = |label: &'static str, f: fn(&mut Shell, usize, usize, usize, &mut Context<'_, Shell>)| {
		let shell = shell.clone();
		PopupMenuItem::new(label).on_click(move |_, _, cx| {
			// The window may be closing; then there's nothing to do.
			let _ = shell.update(cx, |this, cx| f(this, artist, album, track, cx));
		})
	};
	menu.item(item("Add to queue", |this, _, _, t, cx| this.add_to_queue(t, cx)))
		.item(PopupMenuItem::separator())
		.item(item("Go to artist", |this, a, _, _, cx| this.open(Route::Artist(a), cx)))
		.item(item("Go to album", |this, _, al, _, cx| this.open(Route::Collection(Collection::Album(al)), cx)))
		.item(PopupMenuItem::separator())
		.item(item("Save to Liked Songs", |this, _, _, t, cx| this.toggle_liked(t, cx)))
}
