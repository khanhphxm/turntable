//! Your Library: the left-hand panel of Liked Songs, playlists, albums and artists. Collapses to
//! a rail of covers.

use gpui_kit::{
	AnyElement, AppContext as _, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
	StatefulInteractiveElement as _, Styled as _,
	component::{
		button::{Button, ButtonVariants as _},
		input::{Input, InputEvent, InputState},
		menu::{DropdownMenu as _, PopupMenuItem},
	},
	div,
	prelude::FluentBuilder as _,
	px, uniform_list,
};
use turntable_core::{
	Field, Msg, Route,
	library::{Kind, Sort, View},
};

use crate::{
	shell::{Page, Shell},
	theme,
	widgets::{Icon, chip, icon, icon_button},
};

/// One row of Your Library, owned so the virtual list can render it later.
#[derive(Clone)]
struct Entry {
	route: Route,
	title: String,
	sub:   String,
	image: Option<String>,
}

impl Shell {
	fn library_entries(&self) -> Vec<Entry> {
		self.st()
			.library
			.entries(true)
			.iter()
			.map(|e| Entry { route: e.route(), title: e.title().to_owned(), sub: e.sub(), image: e.image(64).map(str::to_owned) })
			.collect()
	}

	pub fn sidebar(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let entries = self.library_entries();
		let collapsed = self.st().sidebar_collapsed();
		div()
			.w(if collapsed { theme::SIDEBAR_COLLAPSED_W } else { theme::SIDEBAR_W })
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

	#[expect(clippy::unused_self, reason = "view builders share Shell's method-call convention")]
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
			.on_click(cx.listener(|this, _, _, cx| this.send(Msg::ToggleSidebar, cx)))
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
						.on_click(cx.listener(|this, _, _, cx| this.send(Msg::CreatePlaylist, cx)))
						.child(icon(Icon::Plus, px(16.), theme::text()))
						.child("Create"),
				)
			})
	}

	fn library_filters(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let kinds = self.st().library.kinds.clone();
		let sort = self.st().library.sort;
		let view = self.st().library.view;
		let shell = cx.entity().downgrade();
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
					Button::new("sort")
						.ghost()
						.child(
							div()
								.flex()
								.items_center()
								.gap_2()
								.text_size(px(14.))
								.text_color(theme::subdued())
								.child(sort.label())
								.child(icon(view_icon(view), px(16.), theme::subdued())),
						)
						.dropdown_menu(move |menu, _, _| {
							let mut menu = menu.label("Sort by");
							for s in Sort::ALL {
								let shell = shell.clone();
								menu = menu.item(PopupMenuItem::new(s.label()).checked(s == sort).on_click(move |_, _, cx| {
									let _ = shell.update(cx, |this, cx| this.send(Msg::LibrarySort(s), cx));
								}));
							}
							menu = menu.separator().label("View as");
							for v in View::ALL {
								let shell = shell.clone();
								menu = menu.item(PopupMenuItem::new(v.label()).checked(v == view).on_click(move |_, _, cx| {
									let _ = shell.update(cx, |this, cx| this.send(Msg::LibraryView(v), cx));
								}));
							}
							menu
						}),
				),
			)
	}

	fn library_row(&mut self, i: usize, e: &Entry, collapsed: bool, cx: &mut Context<'_, Self>) -> AnyElement {
		let open = self.page == Page::Core(e.route.clone());
		let here = self.st().played_from.as_ref() == Some(&e.route);
		let compact = self.st().library.view == View::Compact;
		let art = self.art(&e.route, e.image.as_deref(), px(48.), cx);
		let route = e.route.clone();
		div()
			.id(i)
			.w_full()
			.h(if compact && !collapsed { px(32.) } else { px(64.) })
			.p_2()
			.flex()
			.items_center()
			.gap_3()
			.rounded(px(6.))
			.cursor_pointer()
			.when(open, |s| s.bg(theme::selected()))
			.when(!open, |s| s.hover(|s| s.bg(theme::hover())))
			.on_click(cx.listener(move |this, _, _, cx| this.go(route.clone(), cx)))
			.when(!compact || collapsed, |s| s.child(art))
			.when(!collapsed, |s| {
				s.child(
					div()
						.flex_1()
						.min_w_0()
						.flex()
						.when(!compact, |s| s.flex_col().gap_0p5())
						.when(compact, |s| s.items_center().gap_2())
						.child(div().text_size(px(16.)).truncate().when(here, |s| s.text_color(theme::accent())).child(e.title.clone()))
						.child(div().text_size(px(14.)).text_color(theme::subdued()).truncate().child(e.sub.clone())),
				)
				.when(here && self.st().playing(), |s| s.child(icon(Icon::Volume, px(16.), theme::accent())))
			})
			.into_any_element()
	}
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
