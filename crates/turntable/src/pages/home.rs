//! Home: a greeting, a grid of quick picks, then shelves of what the user plays.

use gpui_kit::{
	AnyElement, Context, Div, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, StatefulInteractiveElement as _, Styled as _,
	div, linear_color_stop, linear_gradient, prelude::FluentBuilder as _, px, rgb,
};
use turntable_core::library::Tile;

use crate::{shell::Shell, theme, widgets::play_disc};

/// "Good morning" until noon, "Good afternoon" until six, else "Good evening" (local time).
fn greeting() -> &'static str {
	use chrono::Timelike as _;
	match chrono::Local::now().hour() {
		5..=11 => "Good morning",
		12..=17 => "Good afternoon",
		_ => "Good evening",
	}
}

impl Shell {
	/// A quick pick: small artwork on the left, the name, and a play disc on hover.
	fn quick_pick(&self, i: usize, t: &Tile, cx: &mut Context<'_, Self>) -> AnyElement {
		let playing = self.playing_context(t.play.as_deref());
		let route = t.route.clone();
		let play = t.play.clone();
		div()
			.id(("pick", i))
			.group("pick")
			.h(px(64.))
			.min_w_0()
			.flex()
			.items_center()
			.gap_3()
			.pr_2()
			.rounded(px(4.))
			.overflow_hidden()
			.bg(theme::alpha(theme::text(), 0.07))
			.hover(|s| s.bg(theme::alpha(theme::text(), 0.2)))
			.cursor_pointer()
			.on_click(cx.listener(move |this, _, _, cx| this.go(route.clone(), cx)))
			.child(self.art(&t.route, t.image.as_deref(), px(64.), cx).rounded(px(0.)).shadow_lg())
			.child(div().flex_1().min_w(px(0.)).overflow_hidden().text_size(px(14.)).font_weight(FontWeight::BOLD).line_clamp(2).child(t.title.clone()))
			.when_some(play, |s, uri| {
				s.child(div().when(!playing, |s| s.invisible().group_hover("pick", |s| s.visible())).child(play_disc(
					("pick-play", i),
					px(32.),
					playing,
					cx.listener(move |this, _, _, cx| {
						cx.stop_propagation();
						this.play_context(&uri, cx);
					}),
				)))
			})
			.into_any_element()
	}

	pub fn home_page(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let lib = &self.st().library;
		let h = &lib.home;
		let mut quick: Vec<Tile> = vec![Tile::liked()];
		for t in h.recents.iter().cloned().chain(lib.playlists.iter().map(Tile::playlist)) {
			if quick.len() == 8 {
				break;
			}
			if !quick.iter().any(|q| q.route == t.route) {
				quick.push(t);
			}
		}
		let recents = h.recents.clone();
		let top = h.top_artists.clone().unwrap_or_default();
		let followed = h.followed.clone().unwrap_or_default();
		let albums = h.albums.clone();
		let playlists: Vec<Tile> = lib.playlists.iter().map(Tile::playlist).collect();
		let loaded = h.loaded;
		let picks: Vec<AnyElement> = quick.iter().enumerate().map(|(i, t)| self.quick_pick(i, t, cx)).collect();
		let tint = self.st().library.listing.as_ref().and_then(|l| l.accent.as_deref()).and_then(theme::parse_hex).unwrap_or(0x404040);
		let mut shelves: Vec<Div> = Vec::new();
		for (key, title, tiles) in [
			("recents", "Recents", &recents),
			("top", "Your top artists", &top),
			("followed", "Artists you follow", &followed),
			("albums", "Your albums", &albums),
			("playlists", "Your playlists", &playlists),
		] {
			if !tiles.is_empty() {
				let cards = self.cards(key, tiles, cx);
				shelves.push(self.shelf(title, cards));
			}
		}
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
					.child(div().text_size(px(32.)).font_weight(FontWeight::BOLD).child(greeting()))
					.child(div().grid().grid_cols(if self.main_w >= 900. { 4 } else { 2 }).gap_2().children(picks)),
			)
			.child(
				div()
					.px_3()
					.pb_8()
					.flex()
					.flex_col()
					.gap_8()
					.when(!loaded, |s| s.child(div().px_3().text_color(theme::subdued()).child("Loading…")))
					.children(shelves),
			)
	}
}
