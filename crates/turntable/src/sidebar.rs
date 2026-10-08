//! Your Library: the left-hand panel of Liked Songs, playlists, albums and artists. Collapses to
//! a rail of covers.

use gpui_kit::{
	AnyElement, AppContext as _, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
	StatefulInteractiveElement as _, Styled as _,
	component::{
		input::{Input, InputEvent, InputState},
		menu::ContextMenuExt as _,
	},
	deferred, div,
	prelude::FluentBuilder as _,
	px, uniform_list,
};
use turntable_core::{
	Dialog, Field, Msg, Route,
	library::{Entry as LibEntry, Kind, Sort, View},
};

use crate::{
	libmenu,
	shell::{Page, Shell},
	theme,
	widgets::{Icon, chip, icon, icon_button},
};

/// One row of Your Library, owned so the virtual list can render it later.
#[derive(Clone)]
struct Entry {
	route:  Route,
	title:  String,
	sub:    String,
	image:  Option<String>,
	/// Nesting under folders (0 at the top).
	depth:  usize,
	/// A folder's expansion.
	open:   Option<bool>,
	pinned: bool,
	menu:   libmenu::Target,
}

impl Shell {
	fn library_entries(&self) -> Vec<Entry> {
		let lib = &self.st().library;
		let me = lib.me.as_ref().map(|m| m.id.as_str()).unwrap_or_default();
		let mut folders = Vec::new();
		libmenu::folders(&lib.rootlist, 0, &mut folders);
		let playlists: Vec<(String, String)> =
			lib.playlists.iter().filter(|p| p.owner.id == me || p.collaborative).map(|p| (p.id.clone(), p.name.clone())).collect();
		lib.tree(true)
			.iter()
			.map(|row| {
				let e = &row.entry;
				let (uri, owned, public, collaborative) = match e {
					LibEntry::Liked(_) => ("spotify:collection:tracks".to_owned(), true, None, false),
					LibEntry::Playlist(p) => (p.uri.clone(), p.owner.id == me, p.public, p.collaborative),
					LibEntry::Folder { id, .. } => (format!("spotify:folder:{id}"), true, None, false),
					LibEntry::Album(a) => (a.uri.clone(), false, None, false),
					LibEntry::Artist(a) => (a.uri.clone(), false, None, false),
				};
				let pinned = lib.is_pinned(&uri);
				let open = match e {
					LibEntry::Folder { id, .. } => Some(lib.folder_expanded(id)),
					_ => None,
				};
				Entry {
					route: e.route(),
					title: e.title().to_owned(),
					sub: e.sub(),
					image: e.image(64).map(str::to_owned),
					depth: row.depth,
					open,
					pinned,
					menu: libmenu::Target {
						route: e.route(),
						excluded: lib.is_excluded(&uri),
						uri,
						owned,
						public,
						collaborative,
						pinned,
						parent: row.parent.map(str::to_owned),
						folders: folders.clone(),
						playlists: playlists.clone(),
					},
				}
			})
			.collect()
	}

	pub fn sidebar(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let entries = self.library_entries();
		let collapsed = self.sidebar_folded();
		div()
			.w(self.sidebar_w())
			.group("library")
			.flex_none()
			.h_full()
			.rounded(theme::RADIUS)
			.bg(theme::panel())
			.flex()
			.flex_col()
			.relative()
			.overflow_hidden()
			.child(self.library_header(collapsed, cx))
			.when(!collapsed, |s| s.child(self.library_filters(cx)))
			.child(
				uniform_list(
					"library",
					entries.len(),
					cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
						range.filter_map(|i| entries.get(i).map(|e| this.library_row(i, e, collapsed, cx))).collect::<Vec<_>>()
					}),
				)
				.flex_1()
				.px_2()
				.pb_2(),
			)
			.child(crate::widgets::corner_mask())
	}

	fn library_header(&mut self, collapsed: bool, cx: &mut Context<'_, Self>) -> impl IntoElement {
		// Spotify's: the title alone; hovering the panel shows the fold icon beside it.
		let toggle = div()
			.id("library-title")
			.group("library-title")
			.flex()
			.items_center()
			.gap_2()
			.h(px(40.))
			.cursor_pointer()
			.on_click(cx.listener(|this, _, _, cx| this.toggle_library(cx)))
			.child(
				div().size(px(24.)).flex_none().when(!collapsed, |s| s.invisible().group_hover("library", |s| s.visible())).child(
					icon(if collapsed { Icon::SidebarOpen } else { Icon::SidebarClose }, px(24.), theme::subdued())
						.group_hover("library-title", |s| s.text_color(theme::text())),
				),
			)
			.when(!collapsed, |s| s.child(div().text_size(px(16.)).font_weight(FontWeight::BOLD).child("Your Library")));
		let wide = self.library_wide;
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
						.flex()
						.items_center()
						.gap_1()
						.child(
							div()
								.id("create")
								.size(px(32.))
								.flex()
								.items_center()
								.justify_center()
								.rounded_full()
								.bg(theme::raised())
								.hover(|s| s.bg(theme::raised_hover()))
								.cursor_pointer()
								.on_click(cx.listener(|this, _, _, cx| this.send(Msg::CreatePlaylist, cx)))
								.child(icon(Icon::Plus, px(16.), theme::text())),
						)
						.child(icon_button(
							"library-wide",
							if wide { Icon::Collapse } else { Icon::Expand },
							px(16.),
							false,
							cx.listener(|this, _, _, cx| {
								this.library_wide = !this.library_wide;
								cx.notify();
							}),
						)),
				)
			})
	}

	fn library_filters(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let kinds = self.st().library.kinds.clone();
		let sort = self.st().library.sort;
		let view = self.st().library.view;
		let menu_open = self.st().library_menu;
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
					.when(!kinds.is_empty(), |s| {
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
								.on_click(cx.listener(|this, _, _, cx| this.send(Msg::ClearLibraryFilter, cx)))
								.child(icon(Icon::Close, px(16.), theme::text())),
						)
					})
					.children(Kind::ALL.iter().enumerate().map(|(i, k)| {
						let k = *k;
						chip(
							("kind", i),
							k.plural(),
							kinds.contains(&k),
							cx.listener(move |this, _, _, cx| this.send(Msg::LibraryKind(k), cx)),
						)
					})),
			)
			.child(
				div().flex().items_center().justify_between().pl_2().pr_2().child(self.library_search(cx)).child(
					div()
						.relative()
						.child(
							div()
								.id("sort")
								.h(px(32.))
								.px_2()
								.flex()
								.items_center()
								.gap_2()
								.rounded(px(4.))
								.text_size(px(14.))
								.text_color(theme::subdued())
								.hover(|s| s.text_color(theme::text()))
								.cursor_pointer()
								.on_click(cx.listener(|this, _, _, cx| this.send(Msg::LibraryMenu, cx)))
								.child(sort.label())
								.child(icon(view_icon(view), px(16.), theme::subdued())),
						)
						.when(menu_open, |s| s.child(sort_menu(sort, view, cx))),
				),
			)
	}

	fn library_row(&mut self, i: usize, e: &Entry, collapsed: bool, cx: &mut Context<'_, Self>) -> AnyElement {
		let open = self.page == Page::Core(e.route.clone());
		let here = self.st().played_from.as_ref() == Some(&e.route);
		let compact = self.st().library.view == View::Compact;
		let folder = match &e.route {
			Route::Folder(id) => Some(id.clone()),
			_ => None,
		};
		// Spotify renames a folder in place: the row's title becomes the field.
		let renaming = folder.as_ref().is_some_and(|id| matches!(&self.st().dialog, Some(Dialog::RenameFolder { id: r, .. }) if r == id));
		let art = if folder.is_some() {
			div().size(px(48.)).flex_none().rounded(px(4.)).bg(theme::raised()).flex().items_center().justify_center().child(icon(
				Icon::Folder,
				px(24.),
				theme::subdued(),
			))
		} else {
			self.art(&e.route, e.image.as_deref(), px(48.), cx)
		};
		let route = e.route.clone();
		let hovered = e.route.clone();
		let target = e.menu.clone();
		let shell = cx.entity().downgrade();
		let indent = if collapsed { 0. } else { 16. * e.depth as f32 };
		div()
			.id(i)
			.on_hover(cx.listener(move |this, over: &bool, _, cx| {
				if *over {
					this.send(Msg::Prefetch(hovered.clone()), cx);
				}
			}))
			.w_full()
			.h(if compact && !collapsed { px(32.) } else { px(64.) })
			.p_2()
			.pl(px(8. + indent))
			.flex()
			.items_center()
			.gap_3()
			.rounded(px(6.))
			.cursor_pointer()
			.when(open, |s| s.bg(theme::selected()))
			.when(!open, |s| s.hover(|s| s.bg(theme::hover())))
			.on_click(cx.listener(move |this, ev: &gpui_kit::ClickEvent, _, cx| match &route {
				// A folder opens its page; its chevron (or a double-click) folds it open in place.
				Route::Folder(id) if ev.click_count() >= 2 => this.send(Msg::ToggleFolder(id.clone()), cx),
				_ => this.go(route.clone(), cx),
			}))
			.context_menu(move |menu, window, cx| libmenu::build(menu, &shell, &target, window, cx))
			.when(!compact || collapsed, |s| s.child(art))
			.when(!collapsed, |s| {
				let title: AnyElement = if renaming {
					div()
						.flex_1()
						.text_size(px(16.))
						.rounded(px(4.))
						.bg(theme::raised())
						.px_1()
						.child(Input::new(&self.folder_name).appearance(false))
						.into_any_element()
				} else {
					div()
						.text_size(px(16.))
						.truncate()
						.when(here, |s| s.text_color(theme::accent()))
						.child(e.title.clone())
						.into_any_element()
				};
				s.child(
					div()
						.flex_1()
						.min_w_0()
						.flex()
						.when(!compact, |s| s.flex_col().gap_0p5())
						.when(compact, |s| s.items_center().gap_2())
						.child(title)
						.child(
							div()
								.flex()
								.items_center()
								.gap_1()
								.min_w_0()
								.when(e.pinned, |s| s.child(icon(Icon::Pin, px(12.), theme::accent())))
								.child(div().text_size(px(14.)).text_color(theme::subdued()).truncate().child(e.sub.clone())),
						),
				)
				.when(here && self.st().playing(), |s| s.child(icon(Icon::Volume, px(16.), theme::accent())))
				.when_some(folder.clone().zip(e.open), |s, (id, expanded)| {
					s.child(
						div()
							.id(("folder-toggle", i))
							.size(px(24.))
							.flex()
							.items_center()
							.justify_center()
							.on_click(cx.listener(move |this, _, _, cx| {
								cx.stop_propagation();
								this.send(Msg::ToggleFolder(id.clone()), cx);
							}))
							.child(icon(if expanded { Icon::Down } else { Icon::Forward }, px(16.), theme::subdued())),
					)
				})
			})
			.into_any_element()
	}
}

/// Spotify's sort-and-view menu: sort rows with a green check on the right, then the views as
/// a row of icons.
fn sort_menu(sort: Sort, view: View, cx: &mut Context<'_, Shell>) -> impl IntoElement {
	let row = |i: usize, s: Sort, cx: &mut Context<'_, Shell>| {
		let on = s == sort;
		div()
			.id(("sort-by", i))
			.h(px(40.))
			.px_3()
			.flex()
			.items_center()
			.justify_between()
			.rounded(px(2.))
			.text_size(px(14.))
			.when(on, |d| d.text_color(theme::accent()))
			.hover(|d| d.bg(theme::raised_hover()))
			.cursor_pointer()
			.on_click(cx.listener(move |this, _, _, cx| this.send(Msg::LibrarySort(s), cx)))
			.child(s.label())
			.when(on, |d| d.child(icon(Icon::Check, px(16.), theme::accent())))
	};
	let sorts: Vec<_> = Sort::ALL.iter().enumerate().map(|(i, s)| row(i, *s, cx)).collect();
	let views: Vec<_> = View::ALL
		.iter()
		.enumerate()
		.map(|(i, v)| {
			let (v, on) = (*v, *v == view);
			div()
				.id(("view-as", i))
				.flex_1()
				.h(px(36.))
				.flex()
				.items_center()
				.justify_center()
				.rounded(px(4.))
				.when(on, |d| d.bg(theme::raised_hover()))
				.hover(|d| d.bg(theme::raised_hover()))
				.cursor_pointer()
				.on_click(cx.listener(move |this, _, _, cx| this.send(Msg::LibraryView(v), cx)))
				.child(icon(view_icon(v), px(16.), if on { theme::accent() } else { theme::subdued() }))
		})
		.collect();
	let label =
		|t: &'static str| div().px_3().pt_3().pb_1().text_size(px(12.)).font_weight(FontWeight::BOLD).text_color(theme::subdued()).child(t);
	deferred(
		div()
			.id("sort-menu")
			.occlude()
			.absolute()
			.top(px(36.))
			.right_0()
			.w(px(220.))
			.p_1()
			.flex()
			.flex_col()
			.rounded(px(4.))
			.bg(theme::raised())
			.shadow_2xl()
			.on_mouse_down_out(cx.listener(|this, _, _, cx| {
				if this.st().library_menu {
					this.send(Msg::LibraryMenu, cx);
				}
			}))
			.child(label("Sort by"))
			.children(sorts)
			.child(div().mx_3().my_1().h(px(1.)).bg(theme::raised_hover()))
			.child(label("View as"))
			.child(div().flex().gap_1().p_1().children(views)),
	)
	.with_priority(1)
}

fn view_icon(v: View) -> Icon {
	match v {
		View::Compact => Icon::ViewCompact,
		View::List => Icon::ViewList,
		View::Grid => Icon::ViewGrid,
		View::Large => Icon::ViewLarge,
	}
}

impl Shell {
	/// Spotify's "Search in Your Library": a magnifier that opens into a field filtering the rows.
	fn library_search(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
		let Some(input) = self.library_filter.clone() else {
			return icon_button(
				"library-search",
				Icon::Search,
				px(16.),
				false,
				cx.listener(|this, _, window, cx| {
					let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search in Your Library"));
					cx.subscribe(&input, |this, input, e: &InputEvent, cx| match e {
						InputEvent::Change => {
							let text = input.read(cx).value().to_string();
							this.send(Msg::Focus(Some(Field::LibraryFilter)), cx);
							let old = this.st().library.filter.encode_utf16().count();
							this.send(Msg::Edit { from: 0, to: old, text }, cx);
						},
						// Enter opens the first match (the core's Submit for this field).
						InputEvent::PressEnter { .. } => this.send(Msg::Submit, cx),
						// Leaving an empty field folds it back into the magnifier.
						InputEvent::Blur if input.read(cx).value().is_empty() => {
							this.library_filter = None;
							cx.notify();
						},
						_ => {},
					})
					.detach();
					input.update(cx, |s, cx| s.focus(window, cx));
					this.library_filter = Some(input);
					cx.notify();
				}),
			)
			.into_any_element();
		};
		div()
			.w(px(188.))
			.h(px(32.))
			.px_2()
			.flex()
			.items_center()
			.gap_2()
			.rounded(px(4.))
			.bg(theme::raised_hover())
			.child(icon(Icon::Search, px(16.), theme::subdued()))
			.child(div().flex_1().text_size(px(14.)).child(Input::new(&input).appearance(false).cleanable(true)))
			.into_any_element()
	}
}
