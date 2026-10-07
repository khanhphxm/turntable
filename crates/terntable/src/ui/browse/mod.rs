//! The main-region frame and content routing; individual browse views live beside it.

mod artist;
mod cards;
mod home;
mod listing;
mod queue;
mod search;
mod sidebar;
mod tracks;

pub(super) use sidebar::library_menu;
use tern_sdk::ui::Node;

use self::{
	home::home,
	listing::listing_view,
	queue::queue,
	search::search,
	sidebar::{library_card, sidebar},
};
use crate::{
	app::{Layout, Msg, Panel, State},
	library::{Conn, Route},
	ui::{div, icons::Icons, setup, text},
};

pub(super) fn app(state: &State, icons: &Icons) -> Node<Msg> {
	if state.layout() == Layout::Compact {
		// One column: an open panel takes the page's place.
		let frame = div(if state.prefs.display.compact { "tt-app tt-compact tt-dense" } else { "tt-app tt-compact" }).key("app");
		return match state.panel {
			Some(panel) => frame.child(side_panel(state, icons, panel)).into(),
			// The library page is the sidebar's card, full width.
			None if state.library.route == Route::Library => {
				frame.child(div("tt-sidecol").key("side").child(library_card(state, icons))).into()
			},
			None => frame.child(content(state, icons)).into(),
		};
	}
	// Settings is a page of its own, in the place of the content, with no panel beside it.
	let page = state.panel == Some(Panel::Settings);
	let panel = state.side_panel().filter(|_| !page);
	let mut class = String::from("tt-app");
	if panel.is_some() {
		class.push_str(" tt-with-queue");
	}
	if state.sidebar_collapsed() {
		class.push_str(" tt-collapsed");
	}
	if state.prefs.display.compact {
		class.push_str(" tt-dense");
	}
	let main = if page { crate::ui::settings::panel(state) } else { content(state, icons) };
	let mut frame = div(&class).key("app").child(sidebar(state, icons)).child(main);
	if let Some(panel) = panel {
		frame = frame.child(side_panel(state, icons, panel));
	}
	frame.into()
}

fn side_panel(state: &State, icons: &Icons, panel: Panel) -> Node<Msg> {
	match panel {
		Panel::NowPlaying => crate::ui::now::panel(state, icons),
		Panel::Queue => queue(state, icons),
		Panel::Lyrics => crate::ui::lyrics::panel(state, icons),
		Panel::Eq => crate::ui::eq::panel(state, icons),
		Panel::Settings => crate::ui::settings::panel(state),
	}
}

// ----------------------------------------------------------------- content

fn content(state: &State, icons: &Icons) -> Node<Msg> {
	let lib = &state.library;
	let tinted = crate::ui::tint(state).is_some();
	let panel = div(if tinted { "tt-panel tt-content tt-tinted" } else { "tt-panel tt-content" }).key("content");
	let body: Vec<Node<Msg>> = match &lib.route {
		Route::Home if state.needs_setup() => vec![setup::setup(state, icons)],
		// The library page is the compact layout's; wider, the sidebar is the library.
		Route::Home | Route::Library => home(state, icons),
		Route::Search if !matches!(lib.conn, Conn::Ready) => vec![setup::setup(state, icons)],
		Route::Search => search(state, icons),
		Route::Liked | Route::Playlist(_) | Route::Album(_) | Route::Artist(_) => match &lib.listing {
			Some(listing) => listing_view(state, icons, listing),
			None => Vec::new(),
		},
	};
	let mut panel = panel.children(body);
	if let Some(err) = &lib.error {
		panel = panel.child(text("tt-note", err.clone()));
	}
	panel.into()
}
