//! A folder of Your Library: its name, then the playlists (and folders) inside as cards.

use gpui_kit::{
	Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, StatefulInteractiveElement as _, Styled as _, div, px,
};
use turntable_core::library::{Entry, Tile};

use crate::{shell::Shell, theme};

impl Shell {
	pub fn folder_page(&mut self, id: &str, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let lib = &self.st().library;
		let name = lib.folder(id).map(|f| f.name.clone()).unwrap_or_default();
		let tiles: Vec<Tile> = lib
			.folder_entries(id, false)
			.iter()
			.map(|e| match e {
				Entry::Playlist(p) => Tile::playlist(p),
				other => Tile { route: other.route(), play: None, title: other.title().to_owned(), sub: other.sub(), image: None },
			})
			.collect();
		let count = tiles.len();
		let cards: Vec<_> = tiles.iter().enumerate().map(|(i, t)| self.card(("folder-card", i), t, cx)).collect();
		div()
			.id("folder-page")
			.size_full()
			.overflow_y_scroll()
			.px_6()
			.pt_16()
			.pb_8()
			.flex()
			.flex_col()
			.gap_6()
			.child(
				div()
					.flex()
					.flex_col()
					.gap_2()
					.child(div().text_size(px(14.)).child("Folder"))
					.child(div().text_size(px(72.)).font_weight(FontWeight::EXTRA_BOLD).line_clamp(1).child(name))
					.child(div().text_size(px(14.)).text_color(theme::subdued()).child(if count == 1 {
						"1 playlist".to_owned()
					} else {
						format!("{count} playlists")
					})),
			)
			.child(div().mx(px(-12.)).flex().flex_wrap().children(cards))
	}
}
