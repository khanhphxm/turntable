//! Search: a hint while the field is empty, else a top result beside the songs, then artists,
//! albums and playlists.

use gpui_kit::{
	Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, StatefulInteractiveElement as _, Styled as _, div,
	prelude::FluentBuilder as _, px,
};
use turntable_core::{Route, library::Tile};

use crate::{
	components::Column,
	shell::Shell,
	theme,
	widgets::{Icon, icon, play_disc, section_title},
};

/// Songs shown beside the top result.
const SONGS: usize = 4;

impl Shell {
	pub fn search_page(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let page = div().id("search-page").size_full().overflow_y_scroll().px_6().pt_4().pb_8().flex().flex_col().gap_8();
		let query = self.st().library.query.clone();
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
					.child(div().text_size(px(16.)).text_color(theme::subdued()).child("Songs, artists, albums and playlists. ⌘K searches from anywhere.")),
			);
		};
		let artists: Vec<Tile> = results.artists.items.iter().map(Tile::artist).collect();
		let albums: Vec<Tile> = results.albums.items.iter().map(Tile::album).collect();
		let playlists: Vec<Tile> = results.playlists.items.iter().map(Tile::playlist).collect();
		let top = artists.first().or(albums.first()).or(playlists.first()).cloned();
		if results.tracks.items.is_empty() && top.is_none() {
			return page.child(
				div()
					.pt_16()
					.flex()
					.flex_col()
					.items_center()
					.gap_2()
					.child(div().text_size(px(24.)).font_weight(FontWeight::BOLD).child(format!("No results found for \"{query}\"")))
					.child(div().text_size(px(16.)).text_color(theme::subdued()).child("Check the spelling, or try fewer or different keywords.")),
			);
		}
		// Top result beside the songs when there's room, above them when not.
		let wide = self.main_w >= 900.;
		let songs: Vec<_> = results.tracks.items.iter().take(SONGS).enumerate().map(|(i, t)| self.track_row(i, t, Column::None, None, true, cx)).collect();
		let mut shelves = Vec::new();
		for (key, title, tiles) in [("s-artists", "Artists", &artists), ("s-albums", "Albums", &albums), ("s-playlists", "Playlists", &playlists)] {
			if !tiles.is_empty() {
				let cards = self.cards(key, tiles, cx);
				shelves.push(div().mx(px(-12.)).child(self.shelf(title, cards)));
			}
		}
		page.child(
			div()
				.flex()
				.when(!wide, |s| s.flex_col())
				.gap_6()
				.when_some(top, |s, t| {
					s.child(div().when(wide, |s| s.w(px(420.)).flex_none()).flex().flex_col().gap_2().child(section_title("Top result")).child(self.top_result(&t, cx)))
				})
				.when(!songs.is_empty(), |s| s.child(div().flex_1().min_w_0().flex().flex_col().gap_2().child(section_title("Songs")).children(songs))),
		)
		.children(shelves)
	}

	fn top_result(&self, t: &Tile, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let playing = self.playing_context(t.play.as_deref());
		let route = t.route.clone();
		let play = t.play.clone();
		let kind = match t.route {
			Route::Artist(_) => "Artist".to_owned(),
			_ => t.sub.clone(),
		};
		div()
			.id("top-result")
			.group("top-result")
			.relative()
			.h(px(232.))
			.p_5()
			.flex()
			.flex_col()
			.gap_5()
			.rounded(px(8.))
			.bg(theme::raised())
			.hover(|s| s.bg(theme::raised_hover()))
			.cursor_pointer()
			.on_click(cx.listener(move |this, _, _, cx| this.go(route.clone(), cx)))
			.child(self.art(&t.route, t.image.as_deref(), px(92.), cx).shadow_lg())
			.child(
				div()
					.flex()
					.flex_col()
					.gap_1()
					.child(div().text_size(px(32.)).font_weight(FontWeight::BOLD).truncate().child(t.title.clone()))
					.child(div().text_size(px(14.)).text_color(theme::subdued()).child(kind)),
			)
			.when_some(play, |s, uri| {
				s.child(div().absolute().right(px(20.)).bottom(px(20.)).when(!playing, |s| s.invisible().group_hover("top-result", |s| s.visible())).child(
					play_disc("top-play", px(48.), playing, cx.listener(move |this, _, _, cx| {
						cx.stop_propagation();
						this.play_context(&uri, cx);
					})),
				))
			})
	}
}
