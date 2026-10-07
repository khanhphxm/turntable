//! A playlist, album or Liked Songs: header, action bar, then the track table (virtualised).

use gpui_kit::{AnyElement, Context, FontWeight, IntoElement, ParentElement as _, Styled as _, div, list, prelude::FluentBuilder as _, px};

use crate::{
	components::{Column, Row},
	mock::Collection,
	shell::{Route, Shell},
	theme,
	widgets::{Icon, icon_button},
};

impl Shell {
	pub fn collection_page(&mut self, c: Collection, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let tracks = self.lib.tracks_of(c);
		list(
			self.list.clone(),
			cx.processor(move |this, ix: usize, _, cx| match ix.checked_sub(1) {
				None => this.collection_header(c, cx),
				Some(pos) => match tracks.get(pos) {
					Some(&track) => {
						let column = if matches!(c, Collection::Album(_)) { Column::None } else { Column::Album };
						let row = this.track_row(Row { key: pos, from: c, pos, track, column, cover: column != Column::None }, cx);
						div().px_6().child(row).when(pos + 1 == tracks.len(), |s| s.pb_8()).into_any_element()
					},
					None => div().into_any_element(),
				},
			}),
		)
		.size_full()
	}

	fn collection_header(&mut self, c: Collection, cx: &mut Context<'_, Self>) -> AnyElement {
		let tracks = self.lib.tracks_of(c);
		let total_ms: u64 = tracks.iter().filter_map(|&t| self.lib.tracks.get(t)).map(|t| t.ms).sum();
		let mins = total_ms / 60_000;
		let length = if mins >= 60 { format!("{} hr {} min", mins / 60, mins % 60) } else { format!("{mins} min") };
		let count = format!("{} songs, {length}", tracks.len());
		let (kicker, owner, extra): (&'static str, String, Option<String>) = match c {
			Collection::Liked => ("Playlist", "khanh".to_owned(), None),
			Collection::Playlist(p) => {
				let pl = self.lib.playlists.get(p);
				(
					"Public Playlist",
					pl.map_or(String::new(), |p| p.owner.to_owned()),
					pl.map(|p| p.description.to_owned()).filter(|d| !d.is_empty()),
				)
			},
			Collection::Album(a) => {
				let al = self.lib.albums.get(a);
				(al.map_or("Album", |a| a.kind), al.map_or(String::new(), |a| self.lib.artist_name(a.artist).to_owned()), None)
			},
			Collection::Artist(_) => ("Artist", String::new(), None),
		};
		let year = match c {
			Collection::Album(a) => self.lib.albums.get(a).map(|a| a.year.to_string()),
			_ => None,
		};
		let owner_el = match c {
			Collection::Album(a) => {
				let artist = self.lib.albums.get(a).map_or(0, |a| a.artist);
				self.link("header-owner", owner, Route::Artist(artist), cx)
					.text_color(theme::text())
					.font_weight(FontWeight::BOLD)
					.into_any_element()
			},
			_ => div().font_weight(FontWeight::BOLD).child(owner).into_any_element(),
		};
		let meta = div()
			.flex()
			.flex_col()
			.gap_2()
			.when_some(extra, |s, d| s.child(div().text_color(theme::alpha(theme::text(), 0.7)).child(d)))
			.child(
				div()
					.flex()
					.items_center()
					.gap_1()
					.child(owner_el)
					.when_some(year, |s, y| s.child(div().text_color(theme::alpha(theme::text(), 0.7)).child(format!("• {y}"))))
					.child(div().text_color(theme::alpha(theme::text(), 0.7)).child(format!("• {count}"))),
			)
			.into_any_element();
		let column = if matches!(c, Collection::Album(_)) { Column::None } else { Column::Album };
		let actions = div().flex().items_center().gap_4().when(!matches!(c, Collection::Liked), |s| {
			s.child(icon_button(
				"page-shuffle",
				Icon::Shuffle,
				px(32.),
				self.shuffle,
				cx.listener(|this, _, _, cx| {
					this.shuffle = !this.shuffle;
					cx.notify();
				}),
			))
		});
		div()
			.w_full()
			.flex()
			.flex_col()
			.child(self.page_header(c, kicker, self.lib.name_of(c).to_owned().into(), meta))
			.child(
				self.wash(c).child(self.action_bar(c, actions, cx)).child(div().px_6().child(self.table_head(column))).child(div().h_4()),
			)
			.into_any_element()
	}
}
