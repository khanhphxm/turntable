//! Home: a greeting, a grid of quick picks, then shelves.

use gpui_kit::{
	Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, StatefulInteractiveElement as _, Styled as _, div,
	linear_color_stop, linear_gradient, prelude::FluentBuilder as _, px, rgb,
};

use crate::{mock::Collection, shell::Shell, theme, widgets::play_disc};

impl Shell {
	/// A quick pick: small artwork on the left, the name, and a play disc on hover.
	fn quick_pick(&self, i: usize, c: Collection, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let playing = self.playing && self.now.is_some_and(|n| n.from == c);
		let route = c.route();
		div()
			.id(("pick", i))
			.group("pick")
			.h(px(64.))
			.flex()
			.items_center()
			.gap_3()
			.pr_2()
			.rounded(px(4.))
			.overflow_hidden()
			.bg(theme::alpha(theme::text(), 0.07))
			.hover(|s| s.bg(theme::alpha(theme::text(), 0.2)))
			.cursor_pointer()
			.on_click(cx.listener(move |this, _, _, cx| this.open(route, cx)))
			.child(self.art(c, px(64.)).rounded(px(0.)).shadow_lg())
			.child(
				div()
					.flex_1()
					.min_w(px(0.))
					.overflow_hidden()
					.text_size(px(14.))
					.font_weight(FontWeight::BOLD)
					.line_clamp(2)
					.child(self.lib.name_of(c).to_owned()),
			)
			.child(div().when(!playing, |s| s.invisible().group_hover("pick", |s| s.visible())).child(play_disc(
				("pick-play", i),
				px(32.),
				playing,
				cx.listener(move |this, _, _, cx| {
					cx.stop_propagation();
					this.play_collection(c, cx);
				}),
			)))
	}

	pub fn home_page(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let picks: Vec<Collection> = std::iter::once(Collection::Liked).chain((0..7).map(Collection::Playlist)).collect();
		let quick: Vec<_> = picks.iter().enumerate().map(|(i, &c)| self.quick_pick(i, c, cx).into_any_element()).collect();
		let tint = self.now.map_or(0x333333, |n| self.lib.color_of(n.from));
		let made_for_you = (0..self.lib.playlists.len()).map(|i| self.card(("mfy", i), Collection::Playlist(i), "By khanh", cx)).collect();
		let albums = (0..self.lib.albums.len())
			.map(|i| {
				let sub = self.lib.albums.get(i).map_or(String::new(), |a| self.lib.artist_name(a.artist).to_owned());
				self.card(("recent", i), Collection::Album(i), sub, cx)
			})
			.collect();
		let artists = (0..self.lib.artists.len()).map(|i| self.card(("top-artist", i), Collection::Artist(i), "Artist", cx)).collect();
		div()
			.id("home")
			.size_full()
			.overflow_y_scroll()
			.child(
				div()
					.bg(linear_gradient(180., linear_color_stop(theme::alpha(rgb(tint), 0.45), 0.), linear_color_stop(theme::panel(), 1.)))
					.px_6()
					.pt_6()
					.pb_4()
					.flex()
					.flex_col()
					.gap_4()
					.child(div().text_size(px(32.)).font_weight(FontWeight::BOLD).child("Good evening"))
					.child(div().grid().grid_cols(if self.main_w >= 900. { 4 } else { 2 }).gap_2().children(quick)),
			)
			.child(
				div()
					.px_3()
					.pb_8()
					.flex()
					.flex_col()
					.gap_8()
					.child(self.shelf("Made for khanh", made_for_you))
					.child(self.shelf("Recently played", albums))
					.child(self.shelf("Your top artists", artists)),
			)
	}
}
