//! Your Library's sidebar and compact page, including filters, sorting and its view menu.

use tern_sdk::ui::{self, Node, html};

use super::cards::card_frame;
use crate::{
	app::{Field, Layout, Msg, State},
	library::{Conn, Entry, Kind, Route, Sort, View},
	ui::{
		Tap, art, button, div, icon,
		icons::{Glyph, Icons},
		text,
	},
};

// ----------------------------------------------------------------- sidebar

pub(super) fn sidebar(state: &State, icons: &Icons) -> Node<Msg> {
	let lib = &state.library;
	let nav = |glyph: Glyph, label: &str, route: Route| -> Node<Msg> {
		let current = lib.route == route;
		html::div()
			.class(if current { "tt-nav tt-cur" } else { "tt-nav" })
			.title(label)
			.child(icon(icons, glyph, 22.0, Tap::Msg(Msg::Navigate(route.clone()), None)))
			.child(html::span(label))
			.on_click(Msg::Navigate(route))
			.into()
	};
	let navcard = div("tt-panel tt-navcard").key("nav").child(nav(Glyph::Home, "Home", Route::Home)).child(nav(
		Glyph::Search,
		"Search",
		Route::Search,
	));
	div("tt-sidecol").key("side").child(navcard).child(library_card(state, icons)).into()
}

/// Rows of Your Library listed before "Show all": each is a handful of nodes and a thumbnail to fetch.
const LIBRARY_ROWS: usize = 100;

/// Your Library: title, kind chips, filter and order, then the rows. The sidebar's second card,
/// and the page of the compact layout.
pub(super) fn library_card(state: &State, icons: &Icons) -> Node<Msg> {
	let lib = &state.library;
	let compact = state.layout() == Layout::Compact;
	// Collapsed to covers, the chips and the filter are out of sight, so they don't apply.
	let covers = !compact && state.sidebar_collapsed();
	let ready = lib.conn == Conn::Ready;
	let name = html::div().class("tt-side-title");
	let name = if compact {
		// A page title: there is no sidebar to collapse.
		name.child(html::span("Your Library"))
	} else {
		// Collapsed: the library's glyph, turning into the panel with the way it opens on hover.
		// Open: no glyph until the pointer is over the card, then the collapse cue slides in
		// left of the title (Spotify's behaviour).
		let tap = || Tap::Msg(Msg::ToggleSidebar, None);
		let glyph = if covers {
			div("tt-side-ico")
				.child(div("tt-ico-rest").child(icon(icons, Glyph::Library, 22.0, tap())))
				.child(div("tt-ico-hover").child(icon(icons, Glyph::SidebarOpen, 22.0, tap())))
		} else {
			div("tt-side-ico tt-slide").child(icon(icons, Glyph::SidebarClose, 22.0, tap()))
		};
		name.title(if covers { "Open Your Library" } else { "Collapse Your Library" })
			.child(glyph)
			.child(html::span("Your Library"))
			.on_click(Msg::ToggleSidebar)
	};
	let mut head = div("tt-side-head").child(name);
	// Collapsed, the create button sits under the title as a round button, as in Spotify.
	if ready {
		head = head.child(button(icons, Glyph::Plus, "Create playlist", Msg::CreatePlaylist, "tt-side-add", 18.0));
	}
	let mut card = div("tt-panel tt-side").key("library").child(head);
	if !ready {
		return card.child(text("tt-side-empty tt-dim", "Connect your Spotify account to see your library here.")).into();
	}
	if !covers {
		card = card.child(library_tools(state, icons));
	}
	let entries = lib.entries(!covers);
	let open = state.open_shelves.contains("library");
	let shown = entries.iter().take(if open { usize::MAX } else { LIBRARY_ROWS });
	// Covers only, the rows are covers whatever the view.
	card = match if covers { View::List } else { lib.view } {
		view @ (View::Grid | View::Large) => {
			let (class, px) = if view == View::Large { ("tt-lib-grid tt-large", 300) } else { ("tt-lib-grid", 160) };
			card.child(div(class).key("grid").children(shown.map(|e| library_tile(state, icons, e, px))))
		},
		view => card.children(shown.map(|e| library_row(state, icons, e, view))),
	};
	let filter = lib.filter.trim();
	if entries.is_empty() {
		card = card.child(text(
			"tt-side-empty tt-dim",
			if filter.is_empty() { "Nothing here yet.".to_owned() } else { format!("No matches for “{filter}”.") },
		));
	} else if entries.len() > LIBRARY_ROWS {
		card = card.child(
			html::el("button")
				.class("tt-link tt-dim tt-side-more")
				.key("more")
				.text(if open { "Show less" } else { "Show all" })
				.on_click(Msg::ToggleShelf("library")),
		);
	}
	card.into()
}

/// Where the tools row sits in the view: the sort menu hangs from it.
const LIBRARY_BAR_NODE: &str = "main.app.side.library.tools.bar";

/// The kind chips, then a search button that opens into the filter field, and the button for
/// the order and the view, as in Spotify.
fn library_tools(state: &State, icons: &Icons) -> Node<Msg> {
	let lib = &state.library;
	let mut chips = div("tt-chips").key("kinds");
	for kind in Kind::ALL {
		let class = if lib.kinds.contains(&kind) { "tt-chip tt-cur" } else { "tt-chip" };
		chips = chips.child(html::el("button").class(class).key(kind.plural()).text(kind.plural()).on_click(Msg::LibraryKind(kind)));
	}
	let focus = Msg::Focus(Some(Field::LibraryFilter));
	// The field shows while it has the keys or holds text; else only its icon does.
	let open = state.focus == Some(Field::LibraryFilter) || !lib.filter.is_empty();
	let find: Node<Msg> = if open {
		let field = ui::input()
			.key(Field::LibraryFilter.id())
			.text(lib.filter.clone())
			.placeholder("Search in Your Library")
			.on_focus(focus.clone())
			.on_edit(|e| Msg::Edit { from: e.from, to: e.to, text: e.text.clone() });
		let class = if state.focus == Some(Field::LibraryFilter) { "tt-search tt-lib-filter tt-focus" } else { "tt-search tt-lib-filter" };
		let mut find = div(class).key("filter").child(icon(icons, Glyph::Search, 16.0, Tap::Msg(focus, None))).child(field);
		if !lib.filter.is_empty() {
			find = find.child(button(icons, Glyph::Close, "Clear", Msg::ClearLibraryFilter, "tt-lib-clear", 14.0));
		}
		find.into()
	} else {
		html::el("button")
			.class("tt-btn tt-lib-find")
			.key("find")
			.title("Search in Your Library")
			.aria("Search in Your Library")
			.child(icon(icons, Glyph::Search, 18.0, Tap::Msg(focus.clone(), None)))
			.on_click(focus)
			.into()
	};
	let sort = html::el("button")
		.class("tt-lib-sort")
		.key("sort")
		.title("Sort and view")
		.aria("Sort and view")
		.child(html::span(lib.sort.label()))
		.child(icon(icons, view_glyph(lib.view), 18.0, Tap::Msg(Msg::LibraryMenu, None)))
		.on_click(Msg::LibraryMenu);
	let bar = div(if open { "tt-lib-bar tt-open" } else { "tt-lib-bar" }).key("bar").child(find).child(sort);
	div("tt-lib-tools").key("tools").child(chips).child(bar).into()
}

/// The icon of a way to lay Your Library out.
fn view_glyph(view: View) -> Glyph {
	match view {
		View::Compact => Glyph::ViewCompact,
		View::List => Glyph::ViewList,
		View::Grid => Glyph::ViewGrid,
		View::Large => Glyph::ViewLarge,
	}
}

/// Your Library's sort and view menu, hanging from the tools row.
pub(in crate::ui) fn library_menu(state: &State, icons: &Icons) -> Node<Msg> {
	let lib = &state.library;
	let mut menu = div("tt-menu").key("menu").child(text("tt-menu-head", "Sort by"));
	for sort in Sort::ALL {
		let pick = Msg::LibrarySort(sort);
		let on = lib.sort == sort;
		let mut item = html::el("button")
			.class(if on { "tt-menu-item tt-on" } else { "tt-menu-item" })
			.key(sort.label())
			.child(html::span(sort.label()))
			.on_click(pick.clone());
		if on {
			item = item.child(icon(icons, Glyph::Check, 16.0, Tap::Msg(pick, None)));
		}
		menu = menu.child(item);
	}
	let mut views = div("tt-menu-views").key("views");
	for view in View::ALL {
		let pick = Msg::LibraryView(view);
		views = views.child(
			html::el("button")
				.class(if lib.view == view { "tt-view tt-on" } else { "tt-view" })
				.key(view.label())
				.title(view.label())
				.aria(view.label())
				.child(icon(icons, view_glyph(view), 18.0, Tap::Msg(pick.clone(), None)))
				.on_click(pick),
		);
	}
	ui::overlay()
		.key("library-menu")
		.role("tt.menu")
		.anchor(ui::Anchor::Node { id: LIBRARY_BAR_NODE.to_owned(), above: false })
		.size(ui::OverlaySize::Sm)
		.child(menu.child(div("tt-menu-rule").key("rule")).child(text("tt-menu-head", "View as")).child(views))
		.into()
}

/// The node key of `route`'s row or tile in Your Library.
fn entry_key(route: &Route) -> String {
	match route {
		Route::Playlist(id) => format!("pl-{id}"),
		Route::Album(id) => format!("al-{id}"),
		Route::Artist(id) => format!("ar-{id}"),
		_ => "pl-liked".to_owned(),
	}
}

/// A row of Your Library; its picture clicks like the row. Artists' pictures stay square:
/// Spotify's design guidelines forbid cropping artwork.
fn library_row(state: &State, icons: &Icons, entry: &Entry<'_>, view: View) -> Node<Msg> {
	let route = entry.route();
	let go = Msg::Navigate(route.clone());
	let class = match (view, state.library.route == route) {
		(View::Compact, false) => "tt-pl tt-line",
		(View::Compact, true) => "tt-pl tt-line tt-cur",
		(_, false) => "tt-pl",
		(_, true) => "tt-pl tt-cur",
	};
	let row = html::div().class(class).key(entry_key(&route)).title(entry.title()).on_click(go.clone());
	if view == View::Compact {
		return row.child(text("tt-pl-name tt-trunc", entry.title())).child(text("tt-pl-sub tt-trunc tt-dim", entry.sub())).into();
	}
	let tap = Tap::Msg(go, None);
	let cover = match entry {
		Entry::Liked(_) => {
			html::div().class("tt-art tt-art-40 tt-art-liked tt-art-fallback").child(icon(icons, Glyph::Heart, 16.0, tap)).into()
		},
		_ => art(state, entry.image(64), "tt-art-40", entry.title(), tap),
	};
	row.child(cover)
		.child(div("tt-pl-text").child(text("tt-pl-name tt-trunc", entry.title())).child(text("tt-pl-sub tt-trunc tt-dim", entry.sub())))
		.into()
}

/// A tile of Your Library's grid views: a card without a play button, covers at least `px` wide.
fn library_tile(state: &State, icons: &Icons, entry: &Entry<'_>, px: u32) -> Node<Msg> {
	let route = entry.route();
	let go = Msg::Navigate(route.clone());
	let tap = Tap::Msg(go.clone(), None);
	let cover = match entry {
		Entry::Liked(_) => html::div().class("tt-art tt-art-liked tt-art-fallback").child(icon(icons, Glyph::Heart, 48.0, tap)).into(),
		_ => art(state, entry.image(px), "", entry.title(), tap),
	};
	card_frame(&entry_key(&route), state.library.route == route, cover, entry.title(), &entry.sub(), None, go)
}
