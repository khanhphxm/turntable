//! An artist: banner, popular tracks, discography and about.

use gpui_kit::{
	Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, StatefulInteractiveElement as _, Styled as _, div, px,
};

use crate::{
	components::{Column, Row, grouped},
	mock::Collection,
	shell::Shell,
	theme,
	widgets::{outline_button, section_title},
};

impl Shell {
	pub fn artist_page(&mut self, a: usize, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let c = Collection::Artist(a);
		let Some(artist) = self.lib.artists.get(a) else {
			return div().id("artist");
		};
		let (name, listeners, about) = (artist.name, artist.listeners, artist.about);
		let popular = self.lib.tracks_of(c);
		let albums: Vec<usize> = self.lib.albums.iter().enumerate().filter(|(_, al)| al.artist == a).map(|(i, _)| i).collect();
		let following = a % 2 == 0;
		let meta = div().child(format!("{} monthly listeners", grouped(u64::from(listeners)))).into_any_element();
		let follow = outline_button("follow", if following { "Following" } else { "Follow" });
		let cards = albums
			.iter()
			.map(|&i| {
				let sub = self.lib.albums.get(i).map_or(String::new(), |al| format!("{} • {}", al.year, al.kind));
				self.card(("disc", i), Collection::Album(i), sub, cx)
			})
			.collect();
		div()
			.id("artist")
			.size_full()
			.overflow_y_scroll()
			.child(self.page_header(c, "Artist", name.into(), meta))
			.child(
				self.wash(c).child(self.action_bar(c, follow, cx)).child(
					div().px_6().flex().flex_col().gap_2().child(section_title("Popular")).children(
						popular.iter().take(5).enumerate().map(|(pos, &track)| {
							self.track_row(Row { key: pos, from: c, pos, track, column: Column::Plays, cover: true }, cx)
						}),
					),
				),
			)
			.child(div().px_3().pt_8().child(self.shelf("Discography", cards)))
			.child(
				div().px_6().pt_8().pb_8().flex().flex_col().gap_4().child(section_title("About")).child(
					div()
						.max_w(px(672.))
						.h(px(240.))
						.rounded(px(8.))
						.p_6()
						.flex()
						.flex_col()
						.justify_end()
						.gap_2()
						.bg(gpui_kit::linear_gradient(
							180.,
							gpui_kit::linear_color_stop(theme::alpha(gpui_kit::rgb(self.lib.color_of(c)), 0.6), 0.),
							gpui_kit::linear_color_stop(theme::raised(), 1.),
						))
						.child(
							div()
								.text_size(px(16.))
								.font_weight(FontWeight::BOLD)
								.child(format!("{} monthly listeners", grouped(u64::from(listeners)))),
						)
						.child(div().text_size(px(16.)).text_color(theme::subdued()).line_clamp(3).child(about)),
				),
			)
	}
}
