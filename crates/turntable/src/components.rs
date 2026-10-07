//! Composite pieces reused across pages: artwork, the track row, cards and shelves, page
//! headers. Every page builds from these, so a track looks and behaves the same wherever it's
//! listed.

use gpui_kit::{
	AnyElement, ClickEvent, Context, Div, ElementId, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
	SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, WeakEntity,
	component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem},
	div, linear_color_stop, linear_gradient, prelude::FluentBuilder as _, px, rgb,
};
use turntable_core::{
	Msg, Route,
	library::Tile,
	spotify::{Track, id_of, pick_image},
};

use crate::{
	shell::Shell,
	theme,
	widgets::{Icon, cover, icon, icon_button, play_disc, section_title},
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

/// A track's artists, comma-separated.
pub fn artists(t: &Track) -> String {
	t.artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ")
}

/// Which optional column a track table shows between the title and the duration.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Column {
	/// The album (playlists, Liked Songs, search).
	Album,
	/// Play counts (an artist's popular tracks), when known.
	Plays,
	/// Nothing (albums: every row is the same album).
	None,
}

/// Track table geometry, shared by the column header and the rows so they line up.
pub const INDEX_W: f32 = 24.;
pub const TAIL_W: f32 = 112.;
pub const ROW_PAD_X: f32 = 16.;

/// The neutral tint of a page whose artwork colour isn't known yet.
const NEUTRAL: u32 = 0x535353;

impl Shell {
	/// Artwork for something a route opens, at `size`: round for artists, Liked Songs' heart on
	/// its gradient, a grey tile until the image arrives.
	pub fn art(&self, route: &Route, url: Option<&str>, size: Pixels, cx: &mut Context<'_, Self>) -> Div {
		let logical = f32::from(size).ceil() as u32;
		match route {
			Route::Liked => div()
				.size(size)
				.flex_none()
				.rounded(px(4.))
				.bg(linear_gradient(135., linear_color_stop(rgb(0x450af5), 0.), linear_color_stop(rgb(0xc4efd9), 1.)))
				.flex()
				.items_center()
				.justify_center()
				.child(icon(Icon::Heart, size * 0.42, theme::text())),
			Route::Artist(_) => cover(self.image(url, logical, cx), theme::RAISED, size, size / 2.),
			_ => cover(self.image(url, logical, cx), theme::RAISED, size, if size > px(100.) { px(6.) } else { px(4.) }),
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
			.child(div().w(px(TAIL_W)).flex().justify_end().pr(px(28.)).child(icon(Icon::Clock, px(16.), theme::subdued())))
	}

	/// Row `i` of the page's tracks (`State::library.rows()`): number (play on hover), cover,
	/// title and artists, optional column, duration and a menu. Click selects, double-click plays.
	pub fn track_row(&self, i: usize, t: &Track, column: Column, plays: Option<u64>, with_cover: bool, cx: &mut Context<'_, Self>) -> AnyElement {
		let st = self.st();
		let current = !st.item.uri.is_empty() && t.uri == st.item.uri;
		let playing = current && st.playing();
		let selected = st.library.show_selection && st.library.selected == Some(i);
		let blocked = t.explicit && !st.prefs.playback.explicit;
		let album = t.album.as_ref();
		let thumb = album.and_then(|a| pick_image(&a.images, 64)).map(|im| im.url.clone());
		let editable = st.library.listing.as_ref().is_some_and(|l| l.editable) && matches!(st.library.route, Route::Playlist(_));
		let artist_id = t.artists.first().map(|a| id_of(&a.uri).to_owned());
		let album_id = album.map(|a| id_of(&a.uri).to_owned());
		let shell = cx.entity().downgrade();
		let menu_artist = artist_id.clone();
		let menu_album = album_id.clone();
		div()
			.id(("row", i))
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
			// Explicit songs are skipped while they're off: dim, so it's no surprise.
			.when(blocked, |s| s.opacity(0.5))
			.when(selected, |s| s.bg(theme::selected()))
			.when(!selected, |s| s.hover(|s| s.bg(theme::hover())))
			.on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
				if e.click_count() >= 2 {
					this.send(Msg::PlayRow(i), cx);
				} else {
					this.send(Msg::Select(i), cx);
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
							.map(|s| if playing { s.child(icon(Icon::Volume, px(16.), theme::accent())) } else { s.child((i + 1).to_string()) }),
					)
					.child(
						div()
							.id(("row-play", i))
							.absolute()
							.right_0()
							.invisible()
							.group_hover("row", |s| s.visible())
							.on_click(cx.listener(move |this, _, _, cx| {
								cx.stop_propagation();
								this.send(if current { Msg::TogglePlay } else { Msg::PlayRow(i) }, cx);
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
					.when(with_cover, |s| s.child(cover(self.image(thumb.as_deref(), 40, cx), theme::RAISED, px(40.), px(4.))))
					.child(
						div()
							.min_w_0()
							.flex()
							.flex_col()
							.child(div().text_size(px(16.)).text_color(if current { theme::accent() } else { theme::text() }).truncate().child(t.name.clone()))
							.child(
								div()
									.flex()
									.items_center()
									.gap_1()
									.min_w_0()
									.when(t.explicit, |s| s.child(explicit_badge()))
									.child(match &artist_id {
										Some(id) => self.link(("row-artist", i), artists(t), Route::Artist(id.clone()), cx).into_any_element(),
										None => div().truncate().child(artists(t)).into_any_element(),
									}),
							),
					),
			)
			.when(column == Column::Album, |s| {
				let name = album.map_or(String::new(), |a| a.name.clone());
				s.child(div().flex_1().min_w_0().flex().child(match &album_id {
					Some(id) => self.link(("row-album", i), name, Route::Album(id.clone()), cx).into_any_element(),
					None => div().child(name).into_any_element(),
				}))
			})
			.when(column == Column::Plays, |s| s.child(div().flex_1().flex().justify_end().child(plays.map(grouped).unwrap_or_default())))
			.child(
				div()
					.w(px(TAIL_W))
					.flex()
					.items_center()
					.justify_end()
					.gap_3()
					.child(
						div()
							.id(("row-add", i))
							.cursor_pointer()
							.invisible()
							.group_hover("row", |s| s.visible())
							.on_click(cx.listener(move |this, _, _, cx| {
								cx.stop_propagation();
								this.send(Msg::AddRowToPlaylist(i), cx);
							}))
							.child(icon(Icon::Save, px(16.), theme::subdued())),
					)
					.child(div().w(px(40.)).flex().justify_end().child(clock(u64::from(t.duration_ms))))
					.child(div().invisible().group_hover("row", |s| s.visible()).child(icon(Icon::Ellipsis, px(16.), theme::subdued()))),
			)
			.context_menu(move |menu, _, _| track_menu(menu, &shell, i, menu_artist.clone(), menu_album.clone(), editable))
			.into_any_element()
	}

	/// Subdued text that underlines and turns white on hover and opens `route`.
	pub fn link(&self, id: impl Into<ElementId>, text: impl Into<SharedString>, route: Route, cx: &mut Context<'_, Self>) -> Stateful<Div> {
		div()
			.id(id)
			.min_w_0()
			.truncate()
			.cursor_pointer()
			.hover(|s| s.underline().text_color(theme::text()))
			.on_click(cx.listener(move |this, _, _, cx| {
				cx.stop_propagation();
				this.go(route.clone(), cx);
			}))
			.child(text.into())
	}

	/// Whether `uri` is the context playing.
	pub fn playing_context(&self, uri: Option<&str>) -> bool {
		let st = self.st();
		st.playing() && uri.is_some_and(|u| !u.is_empty() && st.context.uri == u)
	}

	/// A card: artwork (round for artists), title and subtitle; a play disc rises in on hover.
	pub fn card(&self, id: impl Into<ElementId>, tile: &Tile, cx: &mut Context<'_, Self>) -> Stateful<Div> {
		let playing = self.playing_context(tile.play.as_deref());
		let route = tile.route.clone();
		let play = tile.play.clone();
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
			.on_click(cx.listener(move |this, _, _, cx| this.go(route.clone(), cx)))
			.child(
				div()
					.relative()
					.child(self.art(&tile.route, tile.image.as_deref(), px(theme::CARD_W - 24.), cx).shadow_lg())
					.when_some(play, |s, uri| {
						s.child(div().absolute().right(px(8.)).bottom(px(8.)).when(!playing, |s| s.invisible().group_hover("card", |s| s.visible())).child(
							play_disc(("card-play", uri.len()), px(48.), playing, cx.listener(move |this, _, _, cx| {
								cx.stop_propagation();
								this.play_context(&uri, cx);
							})),
						))
					}),
			)
			.child(
				div()
					.flex()
					.flex_col()
					.gap_0p5()
					.child(div().text_size(px(16.)).truncate().child(tile.title.clone()))
					.child(div().text_size(px(14.)).text_color(theme::subdued()).truncate().child(tile.sub.clone())),
			)
	}

	/// Play/pause a context: resumes it if it's what plays, else starts it.
	pub fn play_context(&mut self, uri: &str, cx: &mut Context<'_, Self>) {
		if self.st().context.uri == uri {
			self.send(Msg::TogglePlay, cx);
		} else {
			self.send(Msg::PlayContext(uri.to_owned()), cx);
		}
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

	/// The tinted header every listing and artist page opens with.
	pub fn page_header(&self, route: &Route, cover_url: Option<&str>, tint: u32, kicker: SharedString, title: SharedString, meta: AnyElement, cx: &mut Context<'_, Self>) -> Div {
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
			.bg(linear_gradient(180., linear_color_stop(rgb(tint), 0.), linear_color_stop(theme::alpha(rgb(tint), 0.55), 1.)))
			.child(self.art(route, cover_url, px(232.), cx).shadow_2xl())
			.child(
				div()
					.flex_1()
					.min_w_0()
					.flex()
					.flex_col()
					.gap_2()
					.child(div().text_size(px(14.)).child(kicker))
					.child(div().text_size(px(size)).line_height(px(size * 1.1)).font_weight(FontWeight::EXTRA_BOLD).line_clamp(2).child(title))
					.child(div().text_size(px(14.)).child(meta)),
			)
	}

	/// The strip under a header: the big play disc, then the page's own actions.
	pub fn action_bar(&self, context: Option<String>, actions: impl IntoElement, cx: &mut Context<'_, Self>) -> Div {
		let playing = self.playing_context(context.as_deref());
		div()
			.flex()
			.items_center()
			.gap_6()
			.px_6()
			.py_6()
			.child(play_disc("page-play", px(56.), playing, cx.listener(move |this, _, _, cx| {
				match &context {
					Some(uri) if this.st().context.uri == *uri => this.send(Msg::TogglePlay, cx),
					_ => this.send(Msg::PlayListing, cx),
				}
			})))
			.child(actions)
			.child(icon_button("page-more", Icon::Ellipsis, px(32.), false, |_, _, _| {}))
	}

	/// The colour wash behind a page's action bar and first rows.
	pub fn wash(&self, tint: u32) -> Div {
		div().bg(linear_gradient(180., linear_color_stop(theme::alpha(rgb(tint), 0.35), 0.), linear_color_stop(theme::panel(), 1.)))
	}

	/// A page's tint: its artwork's colour once known.
	pub fn tint(accent: Option<&str>) -> u32 {
		accent.and_then(theme::parse_hex).unwrap_or(NEUTRAL)
	}
}

/// Spotify's "E" for explicit songs.
fn explicit_badge() -> Div {
	div()
		.flex_none()
		.size(px(16.))
		.rounded(px(2.))
		.bg(theme::subdued())
		.text_color(theme::base())
		.text_size(px(10.))
		.font_weight(FontWeight::BOLD)
		.flex()
		.items_center()
		.justify_center()
		.child("E")
}

/// Right-click on a track.
fn track_menu(menu: PopupMenu, shell: &WeakEntity<Shell>, row: usize, artist: Option<String>, album: Option<String>, editable: bool) -> PopupMenu {
	let item = |label: &'static str, msg: Msg| {
		let shell = shell.clone();
		PopupMenuItem::new(label).on_click(move |_, _, cx| {
			// The window may be closing; then there's nothing to do.
			let _ = shell.update(cx, |this, cx| match &msg {
				Msg::Navigate(route) => this.go(route.clone(), cx),
				other => this.send(other.clone(), cx),
			});
		})
	};
	let mut menu = menu.item(item("Add to queue", Msg::QueueRow(row))).item(item("Add to playlist", Msg::AddRowToPlaylist(row)));
	if editable {
		menu = menu.item(item("Remove from this playlist", Msg::RemoveRow(row)));
	}
	menu = menu.item(PopupMenuItem::separator());
	if let Some(id) = artist {
		menu = menu.item(item("Go to artist", Msg::Navigate(Route::Artist(id))));
	}
	if let Some(id) = album {
		menu = menu.item(item("Go to album", Msg::Navigate(Route::Album(id))));
	}
	menu
}
