//! Search: genre tiles while the field is empty, else a top result, songs, artists and albums.

use gpui_kit::{
	Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, StatefulInteractiveElement as _, Styled as _, div,
	prelude::FluentBuilder as _, px, rgb,
};

use crate::{
	components::{Column, Row},
	mock::{Collection, GENRES},
	shell::Shell,
	theme,
	widgets::{play_disc, section_title},
};

impl Shell {
	pub fn search_page(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let page = div().id("search-page").size_full().overflow_y_scroll().px_6().pt_4().pb_8().flex().flex_col().gap_8();
		if self.query.is_empty() {
			return page.child(section_title("Browse all")).child(div().grid().grid_cols(4).gap_6().children(
				GENRES.iter().enumerate().map(|(i, (name, color))| {
					div()
						.id(("genre", i))
						.relative()
						.h(px(180.))
						.rounded(px(8.))
						.overflow_hidden()
						.bg(rgb(*color))
						.cursor_pointer()
						.p_4()
						.text_size(px(24.))
						.font_weight(FontWeight::BOLD)
						.child(*name)
						// A tilted cover in the corner, like Spotify's tiles.
						.child(
							div()
								.absolute()
								.right(px(-16.))
								.bottom(px(-8.))
								.size(px(100.))
								.rounded(px(4.))
								.bg(theme::alpha(theme::base(), 0.25))
								.shadow_lg(),
						)
				}),
			));
		}
		let q = self.query.to_lowercase();
		let tracks: Vec<usize> = self
			.lib
			.tracks
			.iter()
			.enumerate()
			.filter(|(_, t)| t.title.to_lowercase().contains(&q) || self.lib.artist_name(t.artist).to_lowercase().contains(&q))
			.map(|(i, _)| i)
			.take(4)
			.collect();
		let artists: Vec<usize> =
			self.lib.artists.iter().enumerate().filter(|(_, a)| a.name.to_lowercase().contains(&q)).map(|(i, _)| i).collect();
		let albums: Vec<usize> = self
			.lib
			.albums
			.iter()
			.enumerate()
			.filter(|(_, a)| a.name.to_lowercase().contains(&q) || self.lib.artist_name(a.artist).to_lowercase().contains(&q))
			.map(|(i, _)| i)
			.collect();
		let top = artists.first().map(|&a| Collection::Artist(a)).or_else(|| albums.first().map(|&a| Collection::Album(a)));
		if tracks.is_empty() && top.is_none() {
			return page.child(
				div()
					.pt_16()
					.flex()
					.flex_col()
					.items_center()
					.gap_2()
					.child(div().text_size(px(24.)).font_weight(FontWeight::BOLD).child(format!("No results found for \"{}\"", self.query)))
					.child(
						div()
							.text_size(px(16.))
							.text_color(theme::subdued())
							.child("Check the spelling, or try fewer or different keywords."),
					),
			);
		}
		// Top result beside the songs when there's room, above them when not.
		let wide = self.main_w >= 900.;
		page.child(
			div()
				.flex()
				.when(!wide, |s| s.flex_col())
				.gap_6()
				.when_some(top, |s, c| {
					s.child(
						div()
							.when(wide, |s| s.w(px(420.)).flex_none())
							.flex()
							.flex_col()
							.gap_2()
							.child(section_title("Top result"))
							.child(self.top_result(c, cx)),
					)
				})
				.child(div().flex_1().min_w_0().flex().flex_col().gap_2().child(section_title("Songs")).children(
					tracks.iter().enumerate().map(|(key, &track)| {
						// A song found by search plays from its album.
						let album = self.lib.tracks.get(track).map_or(0, |t| t.album);
						let pos = self.lib.albums.get(album).and_then(|a| a.tracks.iter().position(|&t| t == track)).unwrap_or(0);
						self.track_row(Row { key, from: Collection::Album(album), pos, track, column: Column::None, cover: true }, cx)
					}),
				)),
		)
		.when(!artists.is_empty(), |s| {
			let cards = artists.iter().map(|&a| self.card(("s-artist", a), Collection::Artist(a), "Artist", cx)).collect();
			s.child(div().mx(px(-12.)).child(self.shelf("Artists", cards)))
		})
		.when(!albums.is_empty(), |s| {
			let cards = albums
				.iter()
				.map(|&a| {
					let sub =
						self.lib.albums.get(a).map_or(String::new(), |al| format!("{} • {}", al.year, self.lib.artist_name(al.artist)));
					self.card(("s-album", a), Collection::Album(a), sub, cx)
				})
				.collect();
			s.child(div().mx(px(-12.)).child(self.shelf("Albums", cards)))
		})
	}

	fn top_result(&self, c: Collection, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let playing = self.playing && self.now.is_some_and(|n| n.from == c);
		let kind = match c {
			Collection::Artist(_) => "Artist".to_owned(),
			Collection::Album(a) => {
				self.lib.albums.get(a).map_or(String::new(), |al| format!("Album • {}", self.lib.artist_name(al.artist)))
			},
			_ => "Playlist".to_owned(),
		};
		let route = c.route();
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
			.on_click(cx.listener(move |this, _, _, cx| this.open(route, cx)))
			.child(self.art(c, px(92.)).shadow_lg())
			.child(
				div()
					.flex()
					.flex_col()
					.gap_1()
					.child(div().text_size(px(32.)).font_weight(FontWeight::BOLD).truncate().child(self.lib.name_of(c).to_owned()))
					.child(div().text_size(px(14.)).text_color(theme::subdued()).child(kind)),
			)
			.child(
				div()
					.absolute()
					.right(px(20.))
					.bottom(px(20.))
					.when(!playing, |s| s.invisible().group_hover("top-result", |s| s.visible()))
					.child(play_disc(
						"top-play",
						px(48.),
						playing,
						cx.listener(move |this, _, _, cx| {
							cx.stop_propagation();
							this.play_collection(c, cx);
						}),
					)),
			)
	}
}
