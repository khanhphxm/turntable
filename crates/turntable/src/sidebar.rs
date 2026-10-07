//! Your Library: the left-hand panel of Liked Songs, playlists, albums and artists. Collapses to
//! a rail of covers.

use gpui_kit::{
	Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, StatefulInteractiveElement as _, Styled as _, div,
	prelude::FluentBuilder as _, px, uniform_list,
};

use crate::{
	mock::Collection,
	shell::{LibraryFilter, Route, Shell},
	theme,
	widgets::{Icon, chip, icon, icon_button},
};

/// One entry of Your Library.
#[derive(Clone, Copy)]
struct Entry {
	collection: Collection,
	route:      Route,
}

impl Shell {
	fn library_entries(&self) -> Vec<Entry> {
		let mut out = Vec::new();
		let show = |f: LibraryFilter| self.library_filter.is_none_or(|x| x == f);
		if show(LibraryFilter::Playlists) {
			out.push(Entry { collection: Collection::Liked, route: Route::Collection(Collection::Liked) });
			out.extend(
				(0..self.lib.playlists.len())
					.map(|i| Entry { collection: Collection::Playlist(i), route: Route::Collection(Collection::Playlist(i)) }),
			);
		}
		if show(LibraryFilter::Albums) {
			out.extend(
				(0..self.lib.albums.len())
					.map(|i| Entry { collection: Collection::Album(i), route: Route::Collection(Collection::Album(i)) }),
			);
		}
		if show(LibraryFilter::Artists) {
			out.extend((0..self.lib.artists.len()).map(|i| Entry { collection: Collection::Artist(i), route: Route::Artist(i) }));
		}
		out
	}

	fn subtitle(&self, c: Collection) -> String {
		match c {
			Collection::Liked => format!("Playlist • {} songs", self.lib.liked().len()),
			Collection::Playlist(p) => format!("Playlist • {}", self.lib.playlists.get(p).map_or("", |p| p.owner)),
			Collection::Album(a) => format!("Album • {}", self.lib.albums.get(a).map_or("", |a| self.lib.artist_name(a.artist))),
			Collection::Artist(_) => "Artist".to_owned(),
		}
	}

	pub fn sidebar(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let entries = self.library_entries();
		let collapsed = self.sidebar_collapsed;
		div()
			.w(if collapsed { theme::SIDEBAR_COLLAPSED_W } else { theme::SIDEBAR_W })
			.flex_none()
			.h_full()
			.rounded(theme::RADIUS)
			.bg(theme::panel())
			.flex()
			.flex_col()
			.overflow_hidden()
			.child(self.library_header(collapsed, cx))
			.when(!collapsed, |s| s.child(self.library_filters(cx)))
			.child(
				uniform_list(
					"library",
					entries.len(),
					cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
						range.filter_map(|i| entries.get(i).map(|e| this.library_row(i, *e, collapsed, cx))).collect::<Vec<_>>()
					}),
				)
				.flex_1()
				.px_2()
				.pb_2(),
			)
	}

	fn library_header(&mut self, collapsed: bool, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let toggle = div()
			.id("library-title")
			.group("library-title")
			.flex()
			.items_center()
			.gap_3()
			.h(px(40.))
			.text_color(theme::subdued())
			.hover(|s| s.text_color(theme::text()))
			.cursor_pointer()
			.on_click(cx.listener(|this, _, _, cx| {
				this.sidebar_collapsed = !this.sidebar_collapsed;
				cx.notify();
			}))
			// The icon previews what a click does: the panel collapsing or opening.
			.child(
				div()
					.size(px(24.))
					.relative()
					.child(icon(Icon::Library, px(24.), theme::subdued()).group_hover("library-title", |s| s.invisible()))
					.child(div().absolute().inset_0().invisible().group_hover("library-title", |s| s.visible()).child(icon(
						if collapsed { Icon::SidebarOpen } else { Icon::SidebarClose },
						px(24.),
						theme::text(),
					))),
			)
			.when(!collapsed, |s| s.child(div().text_size(px(16.)).font_weight(FontWeight::BOLD).child("Your Library")));
		div()
			.flex()
			.items_center()
			.justify_between()
			.pt_3()
			.pb_1()
			.map(|s| if collapsed { s.justify_center() } else { s.pl_4().pr_2() })
			.child(toggle)
			.when(!collapsed, |s| {
				s.child(
					div()
						.id("create")
						.h(px(32.))
						.pl(px(10.))
						.pr_4()
						.flex()
						.items_center()
						.gap_1()
						.rounded_full()
						.bg(theme::raised())
						.hover(|s| s.bg(theme::raised_hover()))
						.cursor_pointer()
						.text_size(px(14.))
						.font_weight(FontWeight::BOLD)
						.child(icon(Icon::Plus, px(16.), theme::text()))
						.child("Create"),
				)
			})
	}

	fn library_filters(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let current = self.library_filter;
		let filter_chip = |id: &'static str, label: &'static str, f: LibraryFilter, cx: &mut Context<'_, Self>| {
			chip(
				id,
				label,
				current == Some(f),
				cx.listener(move |this, _, _, cx| {
					this.library_filter = if this.library_filter == Some(f) { None } else { Some(f) };
					cx.notify();
				}),
			)
		};
		div()
			.flex()
			.flex_col()
			.child(
				div()
					.flex()
					.items_center()
					.gap_2()
					.px_4()
					.py_2()
					.when(current.is_some(), |s| {
						s.child(
							div()
								.id("clear-filter")
								.size(px(32.))
								.flex()
								.flex_none()
								.items_center()
								.justify_center()
								.rounded_full()
								.bg(theme::raised())
								.hover(|s| s.bg(theme::raised_hover()))
								.cursor_pointer()
								.on_click(cx.listener(|this, _, _, cx| {
									this.library_filter = None;
									cx.notify();
								}))
								.child(icon(Icon::Close, px(16.), theme::text())),
						)
					})
					.child(filter_chip("f-playlists", "Playlists", LibraryFilter::Playlists, cx))
					.child(filter_chip("f-albums", "Albums", LibraryFilter::Albums, cx))
					.child(filter_chip("f-artists", "Artists", LibraryFilter::Artists, cx)),
			)
			.child(
				div()
					.flex()
					.items_center()
					.justify_between()
					.pl_2()
					.pr_4()
					.child(icon_button("library-search", Icon::Search, px(16.), false, |_, _, _| {}))
					.child(
						div().flex().items_center().gap_2().text_size(px(14.)).text_color(theme::subdued()).child("Recents").child(icon(
							Icon::ViewList,
							px(16.),
							theme::subdued(),
						)),
					),
			)
	}

	fn library_row(&mut self, i: usize, e: Entry, collapsed: bool, cx: &mut Context<'_, Self>) -> impl IntoElement + use<> {
		let open = self.route == e.route;
		let here = self.now.is_some_and(|n| n.from == e.collection);
		let art = self.art(e.collection, px(48.));
		let route = e.route;
		div()
			.id(i)
			.w_full()
			.h(px(64.))
			.p_2()
			.flex()
			.items_center()
			.gap_3()
			.rounded(px(6.))
			.cursor_pointer()
			.when(open, |s| s.bg(theme::selected()))
			.when(!open, |s| s.hover(|s| s.bg(theme::hover())))
			.on_click(cx.listener(move |this, _, _, cx| this.open(route, cx)))
			.child(art)
			.when(!collapsed, |s| {
				s.child(
					div()
						.flex_1()
						.min_w_0()
						.flex()
						.flex_col()
						.gap_0p5()
						.child(
							div()
								.text_size(px(16.))
								.truncate()
								.when(here, |s| s.text_color(theme::accent()))
								.child(self.lib.name_of(e.collection).to_owned()),
						)
						.child(div().text_size(px(14.)).text_color(theme::subdued()).truncate().child(self.subtitle(e.collection))),
				)
				.when(here && self.playing, |s| s.child(icon(Icon::Volume, px(16.), theme::accent())))
			})
	}
}
