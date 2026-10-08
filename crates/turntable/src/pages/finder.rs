//! "Let's find something for your playlist": below one of the user's own playlists, a search
//! whose songs each have an Add button. Open on an empty playlist; "Find more" opens it on others.

use gpui_kit::{
	AnyElement, AppContext as _, Context, Div, Entity, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
	StatefulInteractiveElement as _, Styled as _, Window,
	component::input::{Input, InputEvent, InputState},
	div,
	prelude::FluentBuilder as _,
	px,
};
use turntable_core::{Field, Msg, spotify::pick_image};

use crate::{
	components::artists,
	shell::Shell,
	theme,
	widgets::{Icon, cover, icon, icon_button, outline_button},
};

/// Songs listed under the finder's field.
const RESULTS: usize = 10;

/// The finder's field, for the playlist it was made for.
pub struct Finder {
	input:    Entity<InputState>,
	playlist: String,
	/// Shown (an empty playlist opens it; "Find more" and the close button toggle it).
	pub open: bool,
}

impl Shell {
	/// The finder for `playlist`, made on first use. `empty`: the playlist has no songs yet.
	fn finder_for(&mut self, playlist: &str, empty: bool, window: &mut Window, cx: &mut Context<'_, Self>) -> &mut Finder {
		if self.finder.as_ref().is_some_and(|f| f.playlist != playlist) {
			self.finder = None;
		}
		let finder = match self.finder.take() {
			Some(f) => f,
			None => {
				let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search for songs"));
				cx.subscribe(&input, |this, input, e: &InputEvent, cx| {
					if matches!(e, InputEvent::Change) {
						// The core runs the search (debounced) for whichever field is focused.
						let text = input.read(cx).value().to_string();
						this.send(Msg::Focus(Some(Field::Search)), cx);
						let old = this.st().library.query.encode_utf16().count();
						this.send(Msg::Edit { from: 0, to: old, text }, cx);
					}
				})
				.detach();
				Finder { input, playlist: playlist.to_owned(), open: empty }
			},
		};
		self.finder.insert(finder)
	}

	/// The finder, or the "Find more" link that opens it.
	pub fn finder(&mut self, playlist: &str, empty: bool, window: &mut Window, cx: &mut Context<'_, Self>) -> AnyElement {
		let finder = self.finder_for(playlist, empty, window, cx);
		if !finder.open {
			return div()
				.flex()
				.justify_end()
				.child(
					div()
						.id("find-more")
						.text_size(px(14.))
						.font_weight(FontWeight::BOLD)
						.text_color(theme::subdued())
						.hover(|s| s.text_color(theme::text()))
						.cursor_pointer()
						.on_click(cx.listener(|this, _, _, cx| {
							if let Some(f) = this.finder.as_mut() {
								f.open = true;
							}
							cx.notify();
						}))
						.child("Find more"),
				)
				.into_any_element();
		}
		let input = finder.input.clone();
		let typed = input.read(cx).value().trim().to_owned();
		let results: Vec<_> = self
			.st()
			.library
			.search
			.as_ref()
			.filter(|_| !typed.is_empty() && self.st().library.query.trim() == typed)
			.map(|r| r.tracks.items.iter().take(RESULTS).cloned().collect())
			.unwrap_or_default();
		let rows: Vec<AnyElement> = results.iter().enumerate().map(|(i, t)| self.finder_row(i, t, playlist, cx)).collect();
		div()
			.flex()
			.flex_col()
			.gap_4()
			.pt_6()
			.border_t_1()
			.border_color(theme::divider())
			.child(
				div()
					.flex()
					.items_start()
					.justify_between()
					.child(
						div()
							.flex()
							.flex_col()
							.gap_4()
							.child(div().text_size(px(24.)).font_weight(FontWeight::BOLD).text_color(theme::text()).child("Let's find something for your playlist"))
							.child(search_field(&input)),
					)
					.child(icon_button(
						"finder-close",
						Icon::Close,
						px(20.),
						false,
						cx.listener(|this, _, _, cx| {
							if let Some(f) = this.finder.as_mut() {
								f.open = false;
							}
							cx.notify();
						}),
					)),
			)
			.when(!typed.is_empty() && rows.is_empty(), |s| s.child(div().text_size(px(14.)).child("Searching…")))
			.child(div().flex().flex_col().children(rows))
			.into_any_element()
	}

	fn finder_row(&self, i: usize, t: &turntable_core::spotify::Track, playlist: &str, cx: &mut Context<'_, Self>) -> AnyElement {
		let thumb = t.album.as_ref().and_then(|a| pick_image(&a.images, 64)).map(|im| im.url.clone());
		let album = t.album.as_ref().map_or(String::new(), |a| a.name.clone());
		let (uri, playlist) = (t.uri.clone(), playlist.to_owned());
		div()
			.id(("finder-row", i))
			.h(theme::ROW_H)
			.px(px(16.))
			.flex()
			.items_center()
			.gap_4()
			.rounded(px(4.))
			.hover(|s| s.bg(theme::hover()))
			.child(
				div()
					.flex_1()
					.min_w_0()
					.flex()
					.items_center()
					.gap_3()
					.child(cover(self.image(thumb.as_deref(), 40, cx), theme::RAISED, px(40.), px(4.)))
					.child(
						div()
							.min_w_0()
							.flex()
							.flex_col()
							.child(div().text_size(px(16.)).text_color(theme::text()).truncate().child(t.name.clone()))
							.child(div().text_size(px(14.)).truncate().child(artists(t))),
					),
			)
			.child(div().flex_1().min_w_0().text_size(px(14.)).truncate().child(album))
			.child(outline_button(("finder-add", i), "Add").on_click(cx.listener(move |this, _, _, cx| {
				this.send(Msg::AddToPlaylistDirect { playlist: playlist.clone(), uris: vec![uri.clone()] }, cx);
			})))
			.into_any_element()
	}
}

/// Spotify's in-page search field: a filled rounded box with the search glyph.
fn search_field(input: &Entity<InputState>) -> Div {
	div()
		.w(px(400.))
		.h(px(40.))
		.px_3()
		.flex()
		.items_center()
		.gap_2()
		.rounded(px(4.))
		.bg(theme::alpha(theme::text(), 0.1))
		.child(icon(Icon::Search, px(16.), theme::subdued()))
		.child(div().flex_1().text_size(px(14.)).text_color(theme::text()).child(Input::new(input).appearance(false).cleanable(true)))
}
