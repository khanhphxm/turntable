//! Your Library: the left-hand panel of Liked Songs, playlists, albums and artists. Collapses to
//! a rail of covers. Scroll frames share cached rows; menu data is gathered only on right-click.

use std::rc::Rc;

use gpui_kit::{
	AnyElement, AppContext as _, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
	StatefulInteractiveElement as _, Styled as _,
	component::input::{Input, InputEvent, InputState},
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
	widgets::{Icon, chip, icon, icon_button, reveal_row},
};

/// One row of Your Library, owned so the virtual list can render it later.
pub(crate) struct Entry {
	route:  Route,
	title:  SharedString,
	sub:    SharedString,
	image:  Option<String>,
	pinned: bool,
}

impl Shell {
	fn library_entries(&self) -> Rc<[Entry]> {
		let lib = &self.st().library;
		// Spotify opens a folder in place: Your Library lists only what's inside it.
		let parent = self.library_folder.last().cloned();
		let rows: Vec<LibEntry<'_>> = match &parent {
			Some(id) => lib.folder_entries(id, true),
			None => lib.tree(true).iter().filter(|row| row.depth == 0).map(|row| row.entry).collect(),
		};
		rows.iter()
			.map(|e| {
				let uri = match e {
					LibEntry::Liked(_) => "spotify:collection:tracks".to_owned(),
					LibEntry::Playlist(p) => p.uri.clone(),
					LibEntry::Folder { id, .. } => format!("spotify:folder:{id}"),
					LibEntry::Album(a) => a.uri.clone(),
					LibEntry::Artist(a) => a.uri.clone(),
				};
				let pinned = lib.is_pinned(&uri);
				Entry {
					route: e.route(),
					title: e.title().to_owned().into(),
					sub: e.sub().into(),
					image: e.image(64).map(str::to_owned),
					pinned,
				}
			})
			.collect::<Vec<_>>()
			.into()
	}

	/// Context menus need the current account/folders, not a copy of every playlist per row.
	fn library_menu(&self, route: &Route) -> Vec<crate::menu::Item> {
		let lib = &self.st().library;
		let me = lib.me.as_ref().map_or("", |m| m.id.as_str());
		let (uri, owned, public, collaborative) = match route {
			Route::Liked => ("spotify:collection:tracks".to_owned(), true, None, false),
			Route::Playlist(id) => {
				let Some(p) = lib.playlists.iter().find(|p| p.id == *id) else { return Vec::new() };
				(p.uri.clone(), p.owner.id == me, p.public, p.collaborative)
			},
			Route::Folder(id) => (format!("spotify:folder:{id}"), true, None, false),
			Route::Album(id) => (format!("spotify:album:{id}"), false, None, false),
			Route::Artist(id) => (format!("spotify:artist:{id}"), false, None, false),
			_ => return Vec::new(),
		};
		let mut folders = Vec::new();
		libmenu::folders(&lib.rootlist, 0, &mut folders);
		libmenu::items(&libmenu::Target {
			route: route.clone(),
			pinned: lib.is_pinned(&uri),
			excluded: lib.is_excluded(&uri),
			uri,
			owned,
			public,
			collaborative,
			parent: self.library_folder.last().cloned(),
			folders,
			playlists: lib
				.playlists
				.iter()
				.filter(|p| p.owner.id == me || p.collaborative)
				.map(|p| (p.id.clone(), p.name.clone()))
				.collect(),
		})
	}

	pub fn sidebar(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let entries = if let Some(entries) = &self.library_rows {
			Rc::clone(entries)
		} else {
			let entries = self.library_entries();
			self.library_rows = Some(Rc::clone(&entries));
			entries
		};
		let collapsed = self.sidebar_folded();
		div()
			.id("library-panel")
			.w(self.sidebar_w())
			.group("library")
			.on_hover(cx.listener(|this, over: &bool, _, cx| {
				if this.library_reveal.set(*over) {
					this.regions.library.update(cx, |_, cx| cx.notify());
				}
			}))
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
		// Spotify's: the title alone; hovering the panel slides it over for the fold icon.
		let fold_icon = icon(if collapsed { Icon::SidebarOpen } else { Icon::SidebarClose }, px(24.), theme::subdued())
			.group_hover("library-title", |s| s.text_color(theme::text()));
		let toggle = div()
			.id("library-title")
			.group("library-title")
			.h(px(40.))
			.min_w_0()
			.flex()
			.items_center()
			.overflow_hidden()
			.cursor_pointer()
			.on_click(cx.listener(|this, _, _, cx| this.toggle_library(cx)))
			.map(|s| {
				if collapsed {
					s.child(fold_icon)
				} else {
					s.child(reveal_row(
						"library-reveal",
						self.library_reveal,
						fold_icon,
						div().text_size(px(16.)).font_weight(FontWeight::BOLD).child("Your Library"),
					))
				}
			});
		// Inside a folder the header is a back arrow and the folder's name.
		let folder = self.library_folder.last().and_then(|id| self.st().library.folder(id)).map(|f| f.name.clone());
		let toggle = match folder.filter(|_| !collapsed) {
			None => toggle.into_any_element(),
			Some(name) => div()
				.h(px(40.))
				.min_w_0()
				.flex()
				.items_center()
				.gap_1()
				.child(icon_button("folder-back", Icon::Back, px(16.), false, cx.listener(|this, _, _, cx| this.leave_folder(cx))))
				.child(div().min_w_0().truncate().text_size(px(16.)).font_weight(FontWeight::BOLD).child(name))
				.into_any_element(),
		};
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
		let art = self.art(&e.route, e.image.as_deref(), px(48.), cx);
		let route = e.route.clone();
		let hovered = e.route.clone();
		let target = e.route.clone();
		div()
			.id(i)
			.on_hover(cx.listener(move |this, over: &bool, _, cx| this.prefetch_on_hover(&hovered, *over, cx)))
			.w_full()
			.h(if compact && !collapsed { px(32.) } else { px(64.) })
			// The rail's 56 px row holds only the 48 px art: a smaller inset keeps it centred, with
			// the hover highlight showing evenly on every side.
			.map(|s| if collapsed { s.p_1().justify_center() } else { s.p_2() })
			.flex()
			.items_center()
			.gap_3()
			.rounded(px(6.))
			.cursor_pointer()
			// The open page's row is highlighted in the full list; the rail only marks hover, as
			// Spotify's does.
			.when(open && !collapsed, |s| s.bg(theme::selected()))
			.when(!open || collapsed, |s| s.hover(|s| s.bg(theme::hover())))
			.on_click(cx.listener(move |this, _, _, cx| match &route {
				// A folder opens in place in Your Library, as Spotify's does.
				Route::Folder(id) => this.enter_folder(id.clone(), cx),
				_ => this.go(route.clone(), cx),
			}))
			.on_mouse_down(
				gpui_kit::MouseButton::Right,
				cx.listener(move |this, e: &gpui_kit::MouseDownEvent, _, cx| {
					this.open_menu(e.position, this.library_menu(&target), cx);
				}),
			)
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
				// The chosen view stands out on a light fill, as in Spotify's menu.
				.when(on, |d| d.bg(theme::alpha(theme::text(), 0.18)))
				.when(!on, |d| d.hover(|d| d.bg(theme::alpha(theme::text(), 0.08))))
				.cursor_pointer()
				.on_click(cx.listener(move |this, _, _, cx| this.send(Msg::LibraryView(v), cx)))
				.child(icon(view_icon(v), px(16.), if on { theme::text() } else { theme::subdued() }))
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
	/// Open (or refocus) the same filter used by the magnifier and Cmd/Ctrl+F.
	pub fn focus_library_filter(&mut self, window: &mut gpui_kit::Window, cx: &mut Context<'_, Self>) {
		if self.st().sidebar_collapsed() {
			self.send(Msg::ToggleSidebar, cx);
		}
		self.library_first = true;
		let input = self.library_filter.clone().unwrap_or_else(|| {
			let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search in Your Library"));
			cx.subscribe(&input, |this, input, e: &InputEvent, cx| match e {
				InputEvent::Change => {
					let text = input.read(cx).value().to_string();
					this.send(Msg::SetText(Field::LibraryFilter, text), cx);
				},
				InputEvent::PressEnter { .. } => this.send(Msg::Submit, cx),
				InputEvent::Focus => this.send(Msg::Focus(Some(Field::LibraryFilter)), cx),
				InputEvent::Blur if input.read(cx).value().is_empty() => {
					this.library_filter = None;
					cx.notify();
				},
				InputEvent::Blur => {},
			})
			.detach();
			input
		});
		input.update(cx, |s, cx| s.focus(window, cx));
		self.library_filter = Some(input);
		cx.notify();
	}

	/// Spotify's "Search in Your Library": a magnifier that opens into a field filtering the rows.
	fn library_search(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
		let Some(input) = self.library_filter.clone() else {
			return icon_button(
				"library-search",
				Icon::Search,
				px(16.),
				false,
				cx.listener(|this, _, window, cx| this.focus_library_filter(window, cx)),
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
