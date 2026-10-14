//! Composite pieces reused across pages: artwork, the track row, cards and shelves, page
//! headers. Every page builds from these, so a track looks and behaves the same wherever it's
//! listed.

use gpui_kit::{
	AnyElement, AppContext as _, ClickEvent, Context, Div, ElementId, FontWeight, InteractiveElement as _, IntoElement, MouseButton,
	MouseDownEvent, ParentElement as _, Pixels, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, div,
	linear_color_stop, linear_gradient, prelude::FluentBuilder as _, px, rgb,
};
use turntable_core::{
	Msg, Route,
	app::clock,
	library::{RowTarget, Tile},
	spotify::{Track, id_of, pick_image},
};

use crate::{
	menu::{Action, Item},
	shell::Shell,
	theme,
	widgets::{Icon, added_badge, cover, eq_bars, icon, menu_icon, play_disc, section_title},
};

/// 1234567 → "1,234,567".
pub fn grouped(n: u64) -> String {
	let s = n.to_string();
	let mut out = String::with_capacity(s.len() + s.len() / 3);
	for (i, c) in s.chars().enumerate() {
		if i > 0 && (s.len() - i).is_multiple_of(3) {
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
	/// The album (search).
	Album,
	/// The album and when the song was added (playlists, Liked Songs).
	AlbumAdded,
	/// Play counts (an artist's popular tracks), when known.
	Plays,
	/// Nothing (albums: every row is the same album).
	None,
}

/// Track table geometry, shared by the column header and the rows so they line up.
pub const INDEX_W: f32 = 24.;
/// The "Date added" column.
const ADDED_W: f32 = 160.;
pub const TAIL_W: f32 = 112.;
pub const ROW_PAD_X: f32 = 16.;

/// The neutral tint of a page whose artwork colour isn't known yet.
const NEUTRAL: u32 = 0x0053_5353;

impl Shell {
	/// Artwork for something a route opens, at `size`: round for artists, Liked Songs' heart on
	/// its gradient, a grey tile until the image arrives.
	pub fn art(&self, route: &Route, url: Option<&str>, size: Pixels, cx: &mut Context<'_, Self>) -> Div {
		#[expect(
			clippy::cast_possible_truncation,
			clippy::cast_sign_loss,
			reason = "artwork sizes are nonnegative UI pixels; rounding up preserves full-resolution coverage"
		)]
		let logical = f32::from(size).ceil() as u32;
		match route {
			Route::Liked => div()
				.size(size)
				.flex_none()
				.rounded(px(4.))
				.bg(linear_gradient(135., linear_color_stop(rgb(0x0045_0af5), 0.), linear_color_stop(rgb(0x00c4_efd9), 1.)))
				.flex()
				.items_center()
				.justify_center()
				.child(icon(Icon::Heart, size * 0.42, theme::text())),
			Route::Artist(_) => cover(self.image(url, logical, cx), theme::RAISED, size, size / 2.),
			// A folder: Spotify's grey tile with a folder outline.
			Route::Folder(_) => cover(None, theme::RAISED, size, if size > px(100.) { px(6.) } else { px(4.) })
				.flex()
				.items_center()
				.justify_center()
				.child(icon(Icon::Folder, size * 0.4, theme::subdued())),
			// A playlist without artwork (new or empty) shows a note, as Spotify does.
			_ if url.is_none() => cover(None, theme::RAISED, size, if size > px(100.) { px(6.) } else { px(4.) })
				.flex()
				.items_center()
				.justify_center()
				.child(icon(Icon::Music, size * 0.32, theme::subdued())),
			_ => cover(self.image(url, logical, cx), theme::RAISED, size, if size > px(100.) { px(6.) } else { px(4.) }),
		}
	}

	/// The column header above a track table.
	#[expect(clippy::unused_self, reason = "view builders share Shell's method-call convention")]
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
			.when(matches!(column, Column::Album | Column::AlbumAdded), |s| s.child(div().flex_1().child("Album")))
			.when(column == Column::AlbumAdded, |s| s.child(div().w(px(ADDED_W)).child("Date added")))
			.when(column == Column::Plays, |s| s.child(div().flex_1().flex().justify_end().child("Plays")))
			.child(div().w(px(TAIL_W)).flex().justify_end().pr(px(28.)).child(icon(Icon::Clock, px(16.), theme::subdued())))
	}

	/// Row `i` of the page's tracks (`State::library.rows()`): number (play on hover), cover,
	/// title and artists, optional column, duration and a menu. Click selects, double-click plays.
	#[expect(clippy::too_many_lines, reason = "keep the declarative row layout and its event handlers together")]
	pub fn track_row(
		&self,
		i: usize,
		t: &Track,
		column: Column,
		plays: Option<u64>,
		with_cover: bool,
		cx: &mut Context<'_, Self>,
	) -> AnyElement {
		let st = self.st();
		let Some(row) = st.library.row_target(i) else { return div().into_any_element() };
		let clicked_row = row.clone();
		let play_row = row.clone();
		let add_row = row.clone();
		let current = !st.item.uri.is_empty() && t.uri == st.item.uri;
		let playing = current && st.playing();
		let selected = st.library.show_selection && st.library.selected == Some(i);
		let blocked = t.explicit && !st.prefs.playback.explicit;
		let added = st.added(&t.uri);
		let uri = t.uri.clone();
		let album = t.album.as_ref();
		let thumb = album.and_then(|a| pick_image(&a.images, 64)).map(|im| im.url.clone());
		let editable = st.library.listing.as_ref().is_some_and(|l| l.editable) && matches!(st.library.route, Route::Playlist(_));
		let artist_id = t.artists.first().map(|a| id_of(&a.uri).to_owned());
		let album_id = album.map(|a| id_of(&a.uri).to_owned());
		let menu_artist = artist_id.clone();
		let menu_album = album_id.clone();
		// Compact rows (Settings → Display) drop the cover and tighten to Spotify's 32 px.
		let compact = st.prefs.display.compact;
		let with_cover = with_cover && !compact;
		div()
			.id(("row", i))
			.group("row")
			.w_full()
			.h(if compact { theme::ROW_COMPACT_H } else { theme::ROW_H })
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
					this.send(Msg::PlayRow(clicked_row.clone()), cx);
				} else {
					this.send(Msg::Select(clicked_row.clone()), cx);
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
							.map(|s| if playing { s.child(eq_bars("row-bars", px(14.))) } else { s.child((i + 1).to_string()) }),
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
								if this.st().library.resolve_row(&play_row).is_some() {
									this.send(
										if this.st().item.uri == play_row.uri { Msg::TogglePlay } else { Msg::PlayRow(play_row.clone()) },
										cx,
									);
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
					.when(with_cover, |s| s.child(cover(self.image(thumb.as_deref(), 40, cx), theme::RAISED, px(40.), px(4.))))
					.child(
						div()
							.min_w_0()
							.flex()
							// Compact: title and artists on one line, as Spotify's compact list.
							.when(!compact, |s| s.flex_col())
							.when(compact, |s| s.items_center().gap_2())
							.child(
								div()
									.text_size(px(16.))
									.text_color(if current { theme::accent() } else { theme::text() })
									.truncate()
									.child(t.name.clone()),
							)
							.child(div().flex().items_center().gap_1().min_w_0().when(t.explicit, |s| s.child(explicit_badge())).child(
								match &artist_id {
									Some(id) => self.link(("row-artist", i), artists(t), Route::Artist(id.clone()), cx).into_any_element(),
									None => div().truncate().child(artists(t)).into_any_element(),
								},
							)),
					),
			)
			.when(matches!(column, Column::Album | Column::AlbumAdded), |s| {
				let name = album.map_or(String::new(), |a| a.name.clone());
				s.child(div().flex_1().min_w_0().flex().child(match &album_id {
					Some(id) => self.link(("row-album", i), name, Route::Album(id.clone()), cx).into_any_element(),
					None => div().child(name).into_any_element(),
				}))
			})
			.when(column == Column::AlbumAdded, |s| {
				let added = st.library.listing.as_ref().and_then(|l| l.added_at(i)).map(date_added).unwrap_or_default();
				s.child(div().w(px(ADDED_W)).truncate().child(added))
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
							// Spotify's ✓ (in Liked Songs or any of the user's playlists) and ⊕ both show
							// on hover only. ⊕ saves to Liked Songs; ✓ opens the playlist picker.
							.invisible()
							.group_hover("row", |s| s.visible())
							.on_click(cx.listener(move |this, _, _, cx| {
								cx.stop_propagation();
								this.send(if added { Msg::AddRowToPlaylist(add_row.clone()) } else { Msg::AddToLiked(uri.clone()) }, cx);
							}))
							.child(added_badge(added, px(16.))),
					)
					.child(div().w(px(40.)).flex().justify_end().child(clock(u64::from(t.duration_ms))))
					.child(div().invisible().group_hover("row", |s| s.visible()).child(icon(Icon::Ellipsis, px(16.), theme::subdued()))),
			)
			// On the user's own playlist, rows drag to a new place; the drop target shows a line
			// where the row will land.
			.when(editable, |row| {
				let title = t.name.clone();
				row.on_drag(DraggedRow { from: i, title }, |d, _, _, cx| cx.new(|_| d.clone()))
					.drag_over::<DraggedRow>(|s, _, _, _| s.border_t_2().border_color(theme::accent()))
					.on_drop(cx.listener(move |this, d: &DraggedRow, _, cx| {
						if d.from != i {
							this.send(Msg::MoveRow(d.from, i), cx);
						}
					}))
			})
			.on_mouse_down(
				MouseButton::Right,
				cx.listener(move |this, e: &MouseDownEvent, _, cx| {
					this.open_menu(e.position, track_menu(row.clone(), menu_artist.clone(), menu_album.clone(), editable), cx);
				}),
			)
			.into_any_element()
	}

	/// Subdued text that underlines and turns white on hover and opens `route`.
	#[expect(clippy::unused_self, reason = "view builders share Shell's method-call convention")]
	pub fn link(&self, id: impl Into<ElementId>, text: impl Into<SharedString>, route: Route, cx: &mut Context<'_, Self>) -> Stateful<Div> {
		div()
			.id(id)
			.min_w_0()
			.truncate()
			// Spotify's secondary text: grey, white and underlined on hover.
			.text_color(theme::subdued())
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
		let hovered = tile.route.clone();
		div()
			.id(id)
			.group("card")
			// Spotify warms a page while the pointer rests on its card.
			.on_hover(cx.listener(move |this, over: &bool, _, cx| this.prefetch_on_hover(&hovered, *over, cx)))
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
			.child(div().relative().child(self.art(&tile.route, tile.image.as_deref(), px(theme::CARD_W - 24.), cx).shadow_lg()).when_some(
				play,
				|s, uri| {
					s.child(
						div()
							.absolute()
							.right(px(8.))
							.bottom(px(8.))
							.when(!playing, |s| s.invisible().group_hover("card", |s| s.visible()))
							.child(play_disc(
								"card-play",
								px(48.),
								playing,
								cx.listener(move |this, _, _, cx| {
									cx.stop_propagation();
									this.play_context(&uri, cx);
								}),
							)),
					)
				},
			))
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
	#[expect(clippy::unused_self, reason = "view builders share Shell's method-call convention")]
	pub fn shelf(&self, title: impl Into<SharedString>, cards: Vec<Stateful<Div>>) -> Div {
		div()
			.flex()
			.flex_col()
			.gap_2()
			.child(div().px(px(12.)).child(section_title(title)))
			.child(div().h(px(theme::CARD_W + 70.)).overflow_hidden().flex().flex_wrap().children(cards))
	}

	/// The tinted header every listing and artist page opens with.
	#[expect(clippy::too_many_arguments, reason = "the declarative page header receives its independent display fields directly")]
	pub fn page_header(
		&self,
		route: &Route,
		cover_url: Option<&str>,
		tint: u32,
		kicker: SharedString,
		title: SharedString,
		meta: AnyElement,
		cx: &mut Context<'_, Self>,
	) -> Div {
		// Spotify sizes the title to the room it has: the largest step at which it fits on one
		// line (bold glyphs average ~0.6 em), else the smallest, which may wrap to two.
		let room = self.main_w - 232. - 3. * 24.;
		let chars = title.chars().count().max(1) as f32;
		let size = [96., 72., 48., 32.].into_iter().find(|s| chars * s * 0.6 <= room).unwrap_or(32.);
		let editable = self.editable_playlist();
		div()
			.h(px(320.))
			.w_full()
			// GPUI clips children to rectangles: whatever touches the panel's top rounds itself.
			.rounded_t(theme::RADIUS)
			.flex()
			.items_end()
			.gap_6()
			.p_6()
			.bg(linear_gradient(180., linear_color_stop(rgb(tint), 0.), linear_color_stop(theme::alpha(rgb(tint), 0.55), 1.)))
			.child(self.header_cover(route, cover_url, editable, cx))
			.child(
				div()
					.flex_1()
					.min_w_0()
					.flex()
					.flex_col()
					.gap_2()
					.child(
						div()
							.flex()
							.items_center()
							.gap_2()
							.text_size(px(14.))
							.when(kicker.starts_with("Verified"), |s| {
								// Spotify's verified badge: a blue disc with a white check.
								s.child(
									div()
										.size(px(22.))
										.rounded_full()
										.bg(rgb(0x004c_b3ff))
										.flex()
										.items_center()
										.justify_center()
										.child(icon(Icon::Check, px(14.), theme::text())),
								)
							})
							.child(kicker),
					)
					.child(
						div()
							.id("header-title")
							.text_size(px(size))
							.line_height(px(size * 1.1))
							.font_weight(FontWeight::EXTRA_BOLD)
							.line_clamp(2)
							.when(editable, |s| {
								s.cursor_pointer().on_click(cx.listener(|this, _, _, cx| this.send(Msg::RenamePlaylist, cx)))
							})
							.child(title),
					)
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
			.child(play_disc(
				"page-play",
				px(56.),
				playing,
				cx.listener(move |this, _, _, cx| match &context {
					Some(uri) if this.st().context.uri == *uri => this.send(Msg::TogglePlay, cx),
					_ => this.send(Msg::PlayListing, cx),
				}),
			))
			.child(actions)
			.child(self.page_menu(cx))
	}

	/// The open page is a playlist the user owns (its details can be edited).
	pub fn editable_playlist(&self) -> bool {
		let lib = &self.st().library;
		matches!(lib.route, Route::Playlist(_)) && lib.listing.as_ref().is_some_and(|l| l.owned)
	}

	/// The header's artwork; on the user's own playlist, hovering offers to edit its details.
	fn header_cover(&self, route: &Route, cover_url: Option<&str>, editable: bool, cx: &mut Context<'_, Self>) -> AnyElement {
		let url = self.header_art_url(route, cover_url, cx);
		let art = self.art(route, url.as_deref(), px(232.), cx).shadow_2xl();
		if !editable {
			return art.into_any_element();
		}
		div()
			.id("header-cover")
			.group("header-cover")
			.relative()
			.cursor_pointer()
			.on_click(cx.listener(|this, _, _, cx| this.send(Msg::RenamePlaylist, cx)))
			.child(art)
			.child(
				div()
					.absolute()
					.inset_0()
					.rounded(px(6.))
					.bg(theme::alpha(theme::base(), 0.6))
					.flex()
					.flex_col()
					.items_center()
					.justify_center()
					.gap_2()
					.invisible()
					.group_hover("header-cover", |s| s.visible())
					.child(icon(Icon::Pencil, px(48.), theme::text()))
					.child(div().text_size(px(16.)).child("Edit details")),
			)
			.into_any_element()
	}

	/// The header's artwork URL. A page can learn a better URL after it opens (an artist's
	/// overview carries a larger avatar than the library's), and that one still has to load:
	/// keep showing the artwork already on screen for this page until the new one is ready,
	/// rather than blanking the header in between.
	fn header_art_url(&self, route: &Route, wanted: Option<&str>, cx: &mut Context<'_, Self>) -> Option<String> {
		let wanted = wanted?;
		let shown = self.header_art.borrow().clone();
		let wanted_ready = self.image(Some(wanted), 232, cx).is_some();
		let url = header_art_choice(shown.as_ref(), route, wanted, wanted_ready).to_owned();
		if url == wanted && wanted_ready || url != wanted && self.image(Some(&url), 232, cx).is_some() {
			*self.header_art.borrow_mut() = Some((route.clone(), url.clone()));
		}
		Some(url)
	}

	/// The page's "…": what Spotify offers for the open playlist, album or artist.
	fn page_menu(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let lib = &self.st().library;
		let route = lib.route.clone();
		let listing = lib.listing.as_ref();
		let owned = self.editable_playlist();
		let followed = listing.and_then(|l| l.followed);
		let artist = listing.and_then(|l| l.album.as_ref()).and_then(|a| a.artists.first()).map(|a| id_of(&a.uri).to_owned());
		let mut items = vec![Item::row(Icon::MenuQueue, "Add to queue", Action::Send(Msg::QueueContext(route.clone())))];
		if owned {
			items.push(Item::row(Icon::MenuEdit, "Edit details", Action::Send(Msg::RenamePlaylist)));
		} else if let Some(followed) = followed {
			let (glyph, label) =
				if followed { (Icon::MenuRemove, "Remove from Your Library") } else { (Icon::MenuPlus, "Add to Your Library") };
			items.push(Item::row(glyph, label, Action::Send(Msg::ToggleFollow)));
		}
		if matches!(route, Route::Playlist(_) | Route::Album(_)) {
			items.push(Item::row(
				Icon::MenuAddTo,
				"Add to other playlist",
				Action::SendAll(vec![Msg::AddContextToPlaylist(route), Msg::AddToPlaylist(None)]),
			));
		}
		if let Some(id) = artist {
			items.push(Item::Divider);
			items.push(Item::row(Icon::MenuArtist, "Go to artist", Action::Go(Route::Artist(id))));
		}
		menu_icon("page-more", Icon::Ellipsis, px(32.)).on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
			this.open_menu(e.position(), items.clone(), cx);
		}))
	}

	/// The colour wash behind a page's action bar and first rows.
	#[expect(clippy::unused_self, reason = "view builders share Shell's method-call convention")]
	pub fn wash(&self, tint: u32) -> Div {
		div().bg(linear_gradient(180., linear_color_stop(theme::alpha(rgb(tint), 0.35), 0.), linear_color_stop(theme::panel(), 1.)))
	}

	/// A page's tint: its artwork's colour once known.
	pub fn tint(accent: Option<&str>) -> u32 {
		accent.and_then(theme::parse_hex).unwrap_or(NEUTRAL)
	}
}

/// Spotify's "Date added": how long ago for the last few weeks, else the date ("Jun 8, 2026").
fn date_added(unix: i64) -> String {
	let Some(at) = chrono::DateTime::from_timestamp(unix, 0) else { return String::new() };
	let ago = chrono::Utc::now().signed_duration_since(at);
	let plural = |n: i64, unit: &str| if n == 1 { format!("1 {unit} ago") } else { format!("{n} {unit}s ago") };
	match ago {
		d if d.num_minutes() < 1 => "just now".to_owned(),
		d if d.num_hours() < 1 => plural(d.num_minutes(), "minute"),
		d if d.num_days() < 1 => plural(d.num_hours(), "hour"),
		d if d.num_days() < 7 => plural(d.num_days(), "day"),
		d if d.num_weeks() < 5 => plural(d.num_weeks(), "week"),
		_ => at.with_timezone(&chrono::Local).format("%b %-d, %Y").to_string(),
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

/// Right-click on a track: Spotify's song menu.
fn track_menu(row: RowTarget, artist: Option<String>, album: Option<String>, editable: bool) -> Vec<Item> {
	let send = |glyph: Icon, label: &str, msg: Msg| Item::row(glyph, label, Action::Send(msg));
	let mut items = vec![
		send(Icon::MenuAddTo, "Add to playlist", Msg::AddRowToPlaylist(row.clone())),
		send(Icon::MenuQueue, "Add to queue", Msg::QueueRow(row.clone())),
		send(Icon::MenuNext, "Play next", Msg::QueuePlayNext(row.clone())),
	];
	if editable {
		items.push(send(Icon::MenuDelete, "Remove from this playlist", Msg::RemoveRow(row)));
	}
	if artist.is_some() || album.is_some() {
		items.push(Item::Divider);
	}
	if let Some(id) = artist {
		items.push(Item::row(Icon::MenuArtist, "Go to artist", Action::Go(Route::Artist(id))));
	}
	if let Some(id) = album {
		items.push(Item::row(Icon::MenuAlbum, "Go to album", Action::Go(Route::Album(id))));
	}
	items
}

/// A track row being dragged within the user's playlist; it draws itself under the pointer.
#[derive(Clone)]
pub struct DraggedRow {
	from:  usize,
	title: String,
}

/// A song being dragged within the user's queue.
#[derive(Clone)]
pub struct DraggedQueue {
	pub from:  usize,
	pub title: String,
}

/// What follows the pointer while a row is dragged: its title on a raised chip.
fn drag_chip(title: &str) -> Div {
	div()
		.px_3()
		.py_1p5()
		.rounded(px(4.))
		.bg(theme::raised_hover())
		.shadow_lg()
		.text_size(px(14.))
		.text_color(theme::text())
		.child(title.to_owned())
}

impl gpui_kit::Render for DraggedRow {
	fn render(&mut self, _: &mut gpui_kit::Window, _: &mut Context<'_, Self>) -> impl IntoElement {
		drag_chip(&self.title)
	}
}

impl gpui_kit::Render for DraggedQueue {
	fn render(&mut self, _: &mut gpui_kit::Window, _: &mut Context<'_, Self>) -> impl IntoElement {
		drag_chip(&self.title)
	}
}

/// The header URL to draw: the wanted one once it's ready, else what this page already shows.
fn header_art_choice<'a>(shown: Option<&'a (Route, String)>, route: &Route, wanted: &'a str, wanted_ready: bool) -> &'a str {
	match shown {
		Some((shown_route, shown_url)) if shown_route == route && shown_url != wanted && !wanted_ready => shown_url,
		_ => wanted,
	}
}

#[cfg(test)]
mod header_art_tests {
	use turntable_core::Route;

	use super::header_art_choice;

	#[test]
	fn a_loading_better_url_keeps_the_page_artwork_on_screen() {
		let artist = Route::Artist("a".into());
		let shown = (artist.clone(), "small".to_owned());
		// The overview's larger avatar is still loading: keep the library's.
		assert_eq!(header_art_choice(Some(&shown), &artist, "large", false), "small");
		// Once it's ready, switch.
		assert_eq!(header_art_choice(Some(&shown), &artist, "large", true), "large");
		// Another page never inherits this page's artwork.
		assert_eq!(header_art_choice(Some(&shown), &Route::Artist("b".into()), "other", false), "other");
		assert_eq!(header_art_choice(None, &artist, "large", false), "large");
	}
}
