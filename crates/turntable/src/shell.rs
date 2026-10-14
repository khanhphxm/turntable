//! The window's root view: the core controller, navigation and the layout (top bar, library,
//! main view, right-hand panel, player bar). Pages and panels live in their own modules as
//! `impl Shell` blocks and read `self.st()`.

#[cfg(test)]
#[path = "shell_tests.rs"]
mod tests;

use std::time::Instant;

use gpui_kit::{
	AnyElement, AppContext as _, Context, Entity, FocusHandle, Focusable, ImageSource, InteractiveElement as _, IntoElement, KeyBinding,
	ListAlignment, ListScrollEvent, ListState, ParentElement as _, Pixels, Render, StatefulInteractiveElement as _, StyleRefinement,
	Styled as _, Task, Window, actions,
	component::{
		WindowExt as _,
		input::{InputEvent, InputState},
	},
	div,
	prelude::FluentBuilder as _,
	px,
};
use turntable_core::{
	Controller, Field, Msg, Panel, Route, State,
	app::settings::{MAX_CROSSFADE, Pref},
};

use crate::{
	covers::{CoverCache, LIBRARY_ROWS, LISTING_ROWS, ROOT},
	regions::Kind,
	resize::Edge,
	theme,
	widgets::{Slider, SliderEvent},
};

/// The main view's narrowest comfortable width: below it plus both side panels at full width,
/// the window is crowded and only one side panel stays open (Spotify's behaviour).
const MAIN_MIN_W: f32 = 480.;

/// How many pages back the history reaches.
const HISTORY: usize = 50;

/// How long a toast stays up.
const TOAST_FOR: std::time::Duration = std::time::Duration::from_millis(3000);

actions!(turntable, [
	CloseDialog,
	TogglePlay,
	Next,
	Previous,
	Back,
	Forward,
	FocusSearch,
	FocusFilter,
	LikeSong,
	Shuffle,
	Repeat,
	Home,
	LikedSongs,
	Mute,
	VolumeUp,
	VolumeDown,
	ShortcutHelp,
	ToggleLibrary,
	ToggleQueue,
	ToggleLyrics,
	ToggleNowPlaying,
	OpenSettings
]);

/// Command on macOS, Control on Windows/Linux.
const fn primary(mac: &'static str, other: &'static str) -> &'static str {
	if cfg!(target_os = "macos") { mac } else { other }
}

pub fn bind_keys(cx: &mut gpui_kit::App) {
	cx.bind_keys([
		KeyBinding::new("escape", CloseDialog, Some("Shell")),
		// Input's context must exclude character shortcuts before action dispatch, so
		// GPUI delivers the character to every text field (including nested dialogs).
		KeyBinding::new("space", TogglePlay, Some("Shell && !Input")),
		KeyBinding::new("m", Mute, Some("Shell && !Input")),
		KeyBinding::new(primary("cmd-right", "ctrl-right"), Next, Some("Shell")),
		KeyBinding::new(primary("cmd-left", "ctrl-left"), Previous, Some("Shell")),
		KeyBinding::new(primary("cmd-[", "ctrl-["), Back, Some("Shell")),
		KeyBinding::new(primary("cmd-]", "ctrl-]"), Forward, Some("Shell")),
		KeyBinding::new(primary("cmd-k", "ctrl-k"), FocusSearch, Some("Shell")),
		KeyBinding::new(primary("cmd-shift-l", "ctrl-shift-l"), FocusSearch, Some("Shell")),
		KeyBinding::new(primary("cmd-f", "ctrl-f"), FocusFilter, Some("Shell")),
		KeyBinding::new(primary("cmd-up", "ctrl-up"), VolumeUp, Some("Shell")),
		KeyBinding::new(primary("cmd-down", "ctrl-down"), VolumeDown, Some("Shell")),
		KeyBinding::new(primary("cmd-/", "ctrl-/"), ShortcutHelp, Some("Shell")),
		KeyBinding::new("alt-shift-b", LikeSong, Some("Shell")),
		KeyBinding::new("alt-s", Shuffle, Some("Shell")),
		KeyBinding::new("alt-r", Repeat, Some("Shell")),
		KeyBinding::new("alt-shift-h", Home, Some("Shell")),
		KeyBinding::new("alt-shift-s", LikedSongs, Some("Shell")),
		KeyBinding::new("alt-shift-j", ToggleNowPlaying, Some("Shell")),
		KeyBinding::new("alt-shift-l", ToggleLibrary, Some("Shell")),
		KeyBinding::new("alt-shift-q", ToggleQueue, Some("Shell")),
		KeyBinding::new("alt-shift-r", ToggleNowPlaying, Some("Shell")),
		KeyBinding::new("alt-shift-y", ToggleLyrics, Some("Shell")),
		KeyBinding::new(primary("cmd-,", "ctrl-,"), OpenSettings, Some("Shell")),
	]);
}

/// A page of the main view: one of the core's routes, or one of the app's own pages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Page {
	Core(Route),
	Equalizer,
	Settings,
}

/// How large now playing is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullView {
	Off,
	/// In place of the main view, beside Your Library.
	Expanded,
	/// The whole window, in macOS full screen.
	Screen,
}

/// What the right-hand panel shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
	Core(Panel),
	Devices,
}

pub struct Shell {
	pub core:                 Controller,
	pub page:                 Page,
	pub back:                 std::collections::VecDeque<Page>,
	pub forward:              Vec<Page>,
	/// Bumped whenever the right-hand panel changes, so its entrance animation runs again.
	pub panel_gen:            usize,
	pub(crate) regions:       crate::regions::Regions,
	pub playhead:             Entity<crate::bar::Playhead>,
	pub volume_slider:        Entity<Slider>,
	/// Settings' crossfade length (0..=MAX_CROSSFADE seconds).
	pub crossfade:            Entity<Slider>,
	/// The main view's virtual list (listing pages), and what it was built for.
	pub list:                 ListState,
	pub list_for:             Option<(Route, usize)>,
	pub list_compact:         bool,
	/// Shared sidebar rows, invalidated by core changes or entering/leaving a folder, not scrolling.
	pub(crate) library_rows:  Option<std::rc::Rc<[crate::sidebar::Entry]>>,
	/// A card/row must remain hovered before warming its page; dropping the task cancels it.
	hover_prefetch:           Option<(Route, Task<()>)>,
	pub search:               Entity<InputState>,
	/// A folder's name while it's renamed in place in Your Library.
	pub folder_name:          Entity<InputState>,
	/// The folder whose rename field was last filled.
	pub renaming_folder:      Option<String>,
	pub eq_graph:             Entity<crate::eqgraph::EqGraph>,
	pub focus:                FocusHandle,
	/// The main view's width as of this frame (titles size themselves to it).
	pub main_w:               f32,
	/// The window's scale factor as of this frame (covers decode to it).
	pub scale:                f32,
	pub lyrics_scroll:        gpui_kit::ScrollHandle,
	/// The window can't fit Your Library, the main view and the right panel at full width.
	pub crowded:              bool,
	/// Now playing fills the main view (Spotify's expand) or the whole screen.
	pub full_view:            FullView,
	/// In a crowded window the user opened Your Library, so the right panel folds to a strip
	/// (otherwise the library folds to its rail).
	pub library_first:        bool,
	/// The user folded the right panel to its strip.
	pub panel_hidden:         bool,
	/// Your Library widened (Spotify's expand button).
	pub library_wide:         bool,
	/// Settings' search field, and whether it's open.
	pub settings_query:       Entity<InputState>,
	pub settings_search_open: bool,
	/// Network proxy draft; committed on Enter or blur, never on each keystroke.
	pub proxy_input:          Entity<InputState>,
	/// The folders opened in Your Library, outermost first (empty: the whole library).
	pub library_folder:       Vec<String>,
	/// Your Library's and the right panel's widths, set by dragging their edges.
	pub library_w:            Pixels,
	pub panel_w:              Pixels,
	/// Hover reveals of Your Library's and the right panel's header icons.
	pub library_reveal:       crate::widgets::Reveal,
	pub panel_reveal:         crate::widgets::Reveal,
	/// The miniplayer window, while open.
	pub mini:                 Option<crate::miniplayer::Handle>,
	/// The open context menu.
	pub menu:                 Option<crate::menu::Open>,
	/// The search field's dropdown (recent searches / suggestions) is showing.
	pub search_open:          bool,
	/// The search results page's filter.
	pub search_filter:        crate::pages::search::Filter,
	/// The lyric line last scrolled to.
	pub lyrics_line:          Option<usize>,
	/// The OS's Now Playing session (set once the window is open).
	pub now_playing:          Option<crate::nowplaying::NowPlaying>,
	/// The open "Edit details" dialog's fields.
	pub details:              Option<crate::dialogs::DetailsInputs>,
	/// The empty-playlist finder's field (for the playlist last shown).
	pub finder:               Option<crate::pages::finder::Finder>,
	/// Your Library's filter field, while open.
	pub library_filter:       Option<Entity<InputState>>,
	/// The compact keyboard-shortcut reference is open.
	pub shortcuts_open:       bool,
	shortcuts_focus:          Option<FocusHandle>,
	/// The toast last shown, and the one whose time is up (the core never clears toasts).
	pub toast_shown:          Option<u32>,
	pub toast_gone:           Option<u32>,
}

impl Focusable for Shell {
	fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
		self.focus.clone()
	}
}

impl Shell {
	pub fn new(core: Controller, window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
		let seek = Slider::new(cx, 0.);
		let playhead = cx.new(|cx| crate::bar::Playhead::new(seek.clone(), core.state(), window, cx));
		let volume_slider = Slider::new(cx, f32::from(core.state().volume) / 100.);
		let crossfade = Slider::new(cx, f32::from(core.state().prefs.playback.crossfade) / f32::from(MAX_CROSSFADE));
		cx.subscribe(&crossfade, |this, _, e: &SliderEvent, cx| {
			if let SliderEvent::Commit(v) = e {
				// Whole seconds; at least one while it's on (the switch turns it off).
				#[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "v is 0..=1, so the product is 0..=12")]
				let seconds = ((v * f32::from(MAX_CROSSFADE)).round() as u8).max(1);
				this.send(Msg::Pref(Pref::Crossfade(seconds)), cx);
			}
		})
		.detach();
		cx.subscribe(&seek, |this, _, e: &SliderEvent, cx| {
			if let SliderEvent::Commit(v) = e {
				#[expect(
					clippy::cast_possible_truncation,
					clippy::cast_sign_loss,
					reason = "the normalized seek slider maps to nonnegative integer milliseconds"
				)]
				let ms = (f64::from(*v) * this.st().duration_ms() as f64) as u64;
				this.send(Msg::Seek(ms), cx);
			}
		})
		.detach();
		cx.subscribe(&volume_slider, |this, _, e: &SliderEvent, cx| {
			let (SliderEvent::Scrub(v) | SliderEvent::Commit(v)) = e;
			#[expect(
				clippy::cast_possible_truncation,
				clippy::cast_sign_loss,
				reason = "the normalized volume slider maps to the bounded 0..=100 integer percent range"
			)]
			this.send(Msg::Volume((v * 100.).round() as u8), cx);
		})
		.detach();
		let search = search_input(window, cx);
		let folder_name = folder_name_input(window, cx);
		let proxy_input = Self::network_proxy_input(&core.state().prefs.network.proxy, window, cx);
		let eq_graph = crate::eqgraph::EqGraph::new(cx, core.state().eq.gains_db, core.state().eq.enabled);
		cx.subscribe(&eq_graph, |this, _, e: &crate::eqgraph::BandChanged, cx| this.send(Msg::EqBand(e.band, e.step), cx)).detach();
		cx.subscribe(&eq_graph, |this, _, e: &crate::eqgraph::BandTyped, cx| this.send(Msg::EqSet(e.band, e.db), cx)).detach();
		let page = Page::Core(core.state().library.route.clone());
		let list = ListState::new(0, ListAlignment::Top, px(600.));
		list.set_scroll_handler(cx.listener(|this, event: &ListScrollEvent, window, cx| {
			if event.visible_range.end.saturating_add(5) >= event.count {
				this.schedule_load_more(window, cx);
			}
		}));
		let mut shell = Self {
			core,
			page,
			back: std::collections::VecDeque::new(),
			forward: Vec::new(),
			panel_gen: 0,
			regions: crate::regions::Regions::new(cx),
			playhead,
			volume_slider,
			crossfade,
			list,
			list_for: None,
			list_compact: false,
			library_rows: None,
			hover_prefetch: None,
			search,
			folder_name,
			renaming_folder: None,
			eq_graph,
			focus: cx.focus_handle(),
			main_w: 600.,
			scale: window.scale_factor(),
			lyrics_scroll: gpui_kit::ScrollHandle::new(),
			crowded: false,
			full_view: FullView::Off,
			library_first: false,
			panel_hidden: false,
			library_wide: false,
			settings_query: cx.new(|cx| InputState::new(window, cx).placeholder("Search in Settings")),
			settings_search_open: false,
			proxy_input,
			library_folder: Vec::new(),
			library_w: theme::SIDEBAR_W,
			panel_w: theme::RIGHT_W,
			library_reveal: crate::widgets::Reveal::default(),
			panel_reveal: crate::widgets::Reveal::default(),
			mini: None,
			menu: None,
			search_open: false,
			search_filter: crate::pages::search::Filter::All,
			lyrics_line: None,
			now_playing: None,
			details: None,
			finder: None,
			library_filter: None,
			shortcuts_open: false,
			shortcuts_focus: None,
			toast_shown: None,
			toast_gone: None,
		};
		shell.send(Msg::Navigate(Route::Home), cx);
		shell
	}

	pub fn st(&self) -> &State {
		self.core.state()
	}

	/// Scrolling carries the pointer across many rows: only prefetch after it rests on one.
	pub fn prefetch_on_hover(&mut self, route: &Route, over: bool, cx: &mut Context<'_, Self>) {
		if !over {
			if self.hover_prefetch.as_ref().is_some_and(|(pending, _)| pending == route) {
				self.hover_prefetch = None;
			}
			return;
		}
		let route = route.clone();
		let pending = route.clone();
		let task = cx.spawn(async move |this, cx| {
			cx.background_executor().timer(std::time::Duration::from_millis(200)).await;
			let _ = this.update(cx, |this, cx| this.send(Msg::Prefetch(route), cx));
		});
		self.hover_prefetch = Some((pending, task));
	}

	/// Hands `msg` to the core and redraws when it changed something.
	pub fn send(&mut self, msg: Msg, cx: &mut Context<'_, Self>) {
		let before = self.st().side_panel();
		let sign_out = matches!(msg, Msg::SignOut);
		if self.core.dispatch(msg) {
			self.after_change(before, cx);
		}
		if sign_out && !self.st().signed_in {
			CoverCache::clear(cx);
		}
	}

	/// Workers published or a timer came due: take it in.
	pub fn pump(&mut self, cx: &mut Context<'_, Self>) {
		let before = self.st().side_panel();
		// Moving clocks sample the monotonic anchor themselves. Core lyric deadlines still
		// publish actual line changes; an unchanged timer must not invalidate every region.
		if self.core.pump() {
			self.after_change(before, cx);
		}
	}

	fn after_change(&mut self, panel_before: Option<Panel>, cx: &mut Context<'_, Self>) {
		self.library_rows = None;
		self.invalidate_listing_chrome();
		if self.st().side_panel() != panel_before {
			self.panel_gen += 1;
		}
		let st = self.core.state();
		self.playhead.update(cx, |playhead, cx| playhead.sync(st, cx));
		let vol = f32::from(st.volume) / 100.;
		self.volume_slider.update(cx, |s, cx| s.set(vol, cx));
		let fade = f32::from(self.core.state().prefs.playback.crossfade) / f32::from(MAX_CROSSFADE);
		self.crossfade.update(cx, |s, cx| s.set(fade, cx));
		// The EQ curve follows the core (a preset moves every band).
		let eq = self.core.state().eq;
		self.eq_graph.update(cx, |g, cx| g.set(eq.gains_db, eq.enabled, cx));
		if let Some(mut np) = self.now_playing.take() {
			np.sync(self);
			self.now_playing = Some(np);
		}
		crate::desktop::sync(self, cx);
		self.time_toast(cx);
		cx.notify();
	}

	/// Your Library shows as the rail: folded by the user, or the window is crowded and the
	/// right panel has the room.
	pub fn sidebar_folded(&self) -> bool {
		self.st().sidebar_collapsed() || (self.crowded && !self.library_first)
	}

	/// Your Library's width: the rail, its usual width or widened.
	pub fn sidebar_w(&self) -> Pixels {
		if self.sidebar_folded() {
			theme::SIDEBAR_COLLAPSED_W
		} else if self.library_wide {
			theme::SIDEBAR_WIDE_W
		} else {
			self.library_w
		}
	}

	/// The right panel shows as a strip with a chevron: the window is crowded and the user
	/// opened Your Library.
	pub fn panel_folded(&self) -> bool {
		self.panel_hidden || (self.crowded && self.library_first && !self.st().sidebar_collapsed())
	}

	/// The right panel's width: a strip while folded.
	pub fn right_w(&self) -> Pixels {
		if self.panel_folded() { theme::RIGHT_FOLDED_W } else { self.panel_w }
	}

	/// Opens or folds Your Library. In a crowded window this trades room with the right panel
	/// for now only (the saved preference is for roomy windows).
	pub fn toggle_library(&mut self, cx: &mut Context<'_, Self>) {
		if !self.crowded {
			self.send(Msg::ToggleSidebar, cx);
			return;
		}
		if self.sidebar_folded() {
			self.library_first = true;
			if self.st().sidebar_collapsed() {
				self.send(Msg::ToggleSidebar, cx);
			}
		} else {
			self.library_first = false;
		}
		cx.notify();
	}

	/// Folds the right panel to its strip (by hand, whatever the window's width).
	pub fn fold_panel(&mut self, cx: &mut Context<'_, Self>) {
		self.panel_hidden = true;
		cx.notify();
	}

	/// Expands now playing over the main view, or back.
	pub fn set_full_view(&mut self, on: bool, cx: &mut Context<'_, Self>) {
		self.full_view = if on { FullView::Expanded } else { FullView::Off };
		cx.notify();
	}

	/// Full-screen now playing, or back: the window goes full screen with it.
	pub fn toggle_full_screen(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
		let on = self.full_view != FullView::Screen;
		self.full_view = if on { FullView::Screen } else { FullView::Off };
		if window.is_fullscreen() != on {
			window.toggle_fullscreen();
		}
		cx.notify();
	}

	/// Opens a folder in place in Your Library (Spotify's sidebar shows just its contents).
	pub fn enter_folder(&mut self, id: String, cx: &mut Context<'_, Self>) {
		self.library_folder.push(id);
		self.library_rows = None;
		cx.notify();
	}

	/// Back out of the open folder to its parent, or to the whole library.
	pub fn leave_folder(&mut self, cx: &mut Context<'_, Self>) {
		if self.library_folder.pop().is_some() {
			self.library_rows = None;
			cx.notify();
		}
	}

	/// Unfolds the right panel's strip (folding Your Library to its rail).
	pub fn unfold_panel(&mut self, cx: &mut Context<'_, Self>) {
		self.library_first = false;
		self.panel_hidden = false;
		self.panel_gen += 1;
		cx.notify();
	}

	/// Hides each new toast after a few seconds.
	fn time_toast(&mut self, cx: &mut Context<'_, Self>) {
		let id = self.st().toast.as_ref().map(|(id, _)| *id);
		if id.is_some() && id != self.toast_shown {
			self.toast_shown = id;
			cx.spawn(async move |this, cx| {
				cx.background_executor().timer(TOAST_FOR).await;
				let _ = this.update(cx, |this, cx| {
					this.toast_gone = id;
					cx.notify();
				});
			})
			.detach();
		}
	}

	/// A cover at `logical` px, decoded off-thread: `None` until it's ready (the core fetches it).
	pub fn image(&self, url: Option<&str>, logical: u32, cx: &mut Context<'_, Self>) -> Option<ImageSource> {
		let url = url?;
		let cover = CoverCache::source(url, cx).or_else(|| self.st().image(Some(url)).cloned())?;
		CoverCache::get(url, &cover, logical, self.scale, cx).map(ImageSource::Render)
	}

	/// The artwork at `url` blurred to soft colour, for a backdrop.
	pub fn blurred_image(&self, url: Option<&str>, cx: &mut Context<'_, Self>) -> Option<ImageSource> {
		let url = url?;
		let cover = CoverCache::source(url, cx).or_else(|| self.st().image(Some(url)).cloned())?;
		CoverCache::blurred(url, &cover, cx).map(ImageSource::Render)
	}

	pub(crate) fn with_artwork<T>(
		&mut self,
		scope: u16,
		reset: bool,
		cx: &mut Context<'_, Self>,
		build: impl FnOnce(&mut Self, &mut Context<'_, Self>) -> T,
	) -> T {
		let previous = CoverCache::enter(scope, reset, cx);
		let result = build(self, cx);
		CoverCache::leave(previous, cx);
		Self::reclaim_artwork(cx);
		result
	}

	fn reclaim_artwork(cx: &mut Context<'_, Self>) {
		if CoverCache::schedule_reclaim(cx) {
			let shell = cx.weak_entity();
			cx.defer(move |cx| {
				// Rendering has restored the window, so atlas release reaches it too.
				CoverCache::reclaim(cx);
				let _ = shell.update(cx, |shell, cx| shell.core.retain_images(|url| CoverCache::visible_source(url, cx)));
				CoverCache::prune(cx);
			});
		}
	}

	// --- Navigation -----------------------------------------------------------------------

	/// Pushes `page` onto the back history, keeping the newest `HISTORY` (as browsers do).
	fn remember(&mut self, page: Page) {
		if self.back.len() == HISTORY {
			self.back.pop_front();
		}
		self.back.push_back(page);
	}

	pub fn open(&mut self, page: Page, cx: &mut Context<'_, Self>) {
		if self.page != page {
			let from = std::mem::replace(&mut self.page, page.clone());
			self.remember(from);
			self.forward.clear();
			self.show(page, cx);
		}
	}

	pub fn go(&mut self, route: Route, cx: &mut Context<'_, Self>) {
		self.open(Page::Core(route), cx);
	}

	pub fn go_back(&mut self, cx: &mut Context<'_, Self>) {
		if let Some(p) = self.back.pop_back() {
			let from = std::mem::replace(&mut self.page, p.clone());
			self.forward.push(from);
			self.show(p, cx);
		}
	}

	pub fn go_forward(&mut self, cx: &mut Context<'_, Self>) {
		if let Some(p) = self.forward.pop() {
			let from = std::mem::replace(&mut self.page, p.clone());
			self.remember(from);
			self.show(p, cx);
		}
	}

	fn show(&mut self, page: Page, cx: &mut Context<'_, Self>) {
		self.page = page.clone();
		if let Page::Core(route) = page {
			self.send(Msg::Navigate(route), cx);
		}
		cx.notify();
	}

	/// What the right-hand panel shows: the device list while it's open, else the core's panel,
	/// else now playing; it's always there, as Spotify's is (`None` while folded to its strip).
	pub fn side(&self) -> Option<Side> {
		if self.panel_folded() {
			return None;
		}
		if self.st().connect.menu {
			return Some(Side::Devices);
		}
		Some(match self.st().side_panel() {
			Some(p @ (Panel::Queue | Panel::Lyrics)) => Side::Core(p),
			_ => Side::Core(Panel::NowPlaying),
		})
	}

	/// Shows a panel, or hides it back to now playing; now playing itself has nowhere to go.
	pub fn toggle_side(&mut self, side: Side, cx: &mut Context<'_, Self>) {
		if self.panel_folded() {
			self.unfold_panel(cx);
			if self.side() == Some(side) {
				return;
			}
		} else if side == Side::Core(Panel::NowPlaying) && self.side() == Some(side) {
			return;
		}
		self.panel_gen += 1;
		if side == Side::Core(Panel::NowPlaying) {
			// Back from the queue, lyrics or devices to now playing.
			if self.st().connect.menu {
				self.send(Msg::DevicesMenu, cx);
			}
			if let Some(p @ (Panel::Queue | Panel::Lyrics)) = self.st().panel {
				self.send(Msg::TogglePanel(p), cx);
			}
			return;
		}
		match side {
			Side::Devices => self.send(Msg::DevicesMenu, cx),
			Side::Core(p) => {
				if self.st().connect.menu {
					self.send(Msg::DevicesMenu, cx);
				}
				self.send(Msg::TogglePanel(p), cx);
			},
		}
	}

	pub(crate) fn page_content(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
		match self.page.clone() {
			Page::Core(Route::Home | Route::Library) => self.home_page(cx).into_any_element(),
			Page::Core(Route::Folder(id)) => self.folder_page(&id, cx).into_any_element(),
			Page::Core(Route::Search) => self.search_page(cx).into_any_element(),
			Page::Core(Route::Artist(_)) => self.artist_page(cx).into_any_element(),
			Page::Core(route @ (Route::Liked | Route::Playlist(_) | Route::Album(_))) => self.listing_page(route, cx).into_any_element(),
			Page::Settings => self.settings_page(cx).into_any_element(),
			Page::Equalizer => self.eq_page(cx).into_any_element(),
		}
	}
}

/// Drives the core: sleeps until a worker wakes it or its next timer is due, then pumps.
pub fn run_core(shell: &Entity<Shell>, wake: smol::channel::Receiver<()>, cx: &mut gpui_kit::App) {
	let shell = shell.downgrade();
	cx.spawn(async move |cx| {
		loop {
			let deadline = shell.update(cx, |s, _| s.core.deadline()).ok().flatten();
			let woken = wake.recv();
			match deadline {
				Some(at) => {
					let timer = cx.background_executor().timer(at.saturating_duration_since(Instant::now()));
					smol::future::or(
						async {
							let _ = woken.await;
						},
						timer,
					)
					.await;
				},
				None => {
					if woken.await.is_err() {
						break;
					}
				},
			}
			if shell
				.update(cx, |s, cx| {
					turntable_core::net::trace::playback("ui-receive", "pump", None);
					s.pump(cx);
				})
				.is_err()
			{
				break;
			}
		}
	})
	.detach();
}

impl Shell {
	/// Tells native animations whether an overlay covers the window, so they fall back to GPUI.
	fn mark_obscured(&self, window: &mut Window, cx: &mut Context<'_, Self>) {
		let toast = self.st().toast.as_ref().is_some_and(|(id, _)| Some(*id) != self.toast_gone);
		let covered = self.st().dialog.is_some() || self.menu.is_some() || self.shortcuts_open || self.search_open || toast;
		crate::animation::set_obscured(window, cx, covered);
	}
}

impl Render for Shell {
	fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
		self.mark_obscured(window, cx);
		// Leaving macOS full screen another way (the green button, ⌃⌘F) ends full-screen now playing.
		if self.full_view == FullView::Screen && !window.is_fullscreen() {
			self.full_view = FullView::Off;
		}
		let mut hidden = ROOT;
		if !self.st().signed_in {
			hidden = u16::MAX;
		} else {
			if self.full_view == FullView::Screen {
				hidden |= Kind::Top.artwork_scope();
			}
			if self.full_view != FullView::Off {
				hidden |= Kind::Library.artwork_scope() | LIBRARY_ROWS | Kind::Panel.artwork_scope() | LISTING_ROWS;
			}
		}
		CoverCache::reset(hidden, cx);
		Self::reclaim_artwork(cx);
		if !self.st().signed_in {
			if self.page == Page::Settings {
				return div()
					.size_full()
					.flex()
					.flex_col()
					.bg(theme::base())
					.text_color(theme::text())
					.child(
						div()
							.id("settings-back-to-sign-in")
							.p_4()
							.cursor_pointer()
							.on_click(cx.listener(|this, _, _, cx| this.open(Page::Core(Route::Home), cx)))
							.child("Back to sign in"),
					)
					.child(div().flex_1().min_h_0().child(self.settings_page(cx)))
					.children(crate::topbar::window_titlebar(window))
					.into_any_element();
			}
			return self.welcome_page(window, cx).into_any_element();
		}
		self.scale = window.scale_factor();
		let width = window.viewport_size().width;
		self.crowded = width < self.library_w + self.panel_w + px(MAIN_MIN_W) + theme::GAP * 4.;
		let region_style = StyleRefinement::default().h_full().flex_none();
		let left = self.sidebar_w();
		let right = self.right_w() + theme::GAP;
		self.main_w = f32::from(window.viewport_size().width - left - right - theme::GAP * 3.);
		let toast = self.st().toast.clone().filter(|(id, _)| Some(*id) != self.toast_gone);
		let dialog = self.dialog(window, cx);
		let menu = self.menu_overlay(window, cx);
		self.key_actions(cx)
			.size_full()
			.flex()
			.flex_col()
			.bg(theme::base())
			.text_color(theme::text())
			.font_family(".SystemUIFont")
			.when(self.full_view != FullView::Screen, |s| {
				s.child(self.regions.top.clone().cached(StyleRefinement::default().w_full().h(theme::TOP_H).flex_none()))
			})
			.child(
				div()
					.flex_1()
					.min_h_0()
					.flex()
					.px(theme::GAP)
					.when(self.full_view == FullView::Off, |row| {
						row.child(self.regions.library.clone().cached(region_style.clone().w(left)))
							.child(Self::resize_handle(Edge::Library, cx))
					})
					.when(self.full_view != FullView::Off, |row| {
						row.child(self.regions.page.clone().cached(StyleRefinement::default().flex_1().min_w_0().h_full()))
					})
					.when(self.full_view == FullView::Off, |row| {
						row.child(
							div()
								.relative()
								.flex_1()
								.min_w_0()
								.h_full()
								.rounded(theme::RADIUS)
								.overflow_hidden()
								.bg(theme::panel())
								.child(self.regions.page.clone().cached(StyleRefinement::default().size_full()))
								.when_some(toast, |s, (id, text)| s.child(toast_view(text, id)))
								.child(crate::widgets::corner_mask()),
						)
					})
					.when(self.full_view == FullView::Off, |row| {
						row.child(Self::resize_handle(Edge::Panel, cx))
							.child(self.regions.panel.clone().cached(region_style.w(self.right_w())))
					}),
			)
			.child(self.player_bar())
			.relative()
			.children(dialog)
			.children(menu)
			.when(self.shortcuts_open, |s| s.child(Self::shortcut_help(cx)))
			.into_any_element()
	}
}

/// Spotify's toast: a white pill centred near the bottom of the main view, rising in.
fn toast_view(text: String, id: u32) -> impl IntoElement {
	use std::time::Duration;

	use gpui_kit::{Animation, AnimationExt as _, ease_out_quint};
	div().absolute().bottom(px(24.)).left_0().right_0().flex().justify_center().child(
		div()
			.px_4()
			.py_2()
			.rounded(px(6.))
			.bg(theme::text())
			.text_color(theme::base())
			.text_size(px(14.))
			.shadow_lg()
			.child(text)
			.with_animation(("toast", id as usize), Animation::new(Duration::from_millis(220)).with_easing(ease_out_quint()), |el, d| {
				el.opacity(d).mb(px(-12. * (1. - d)))
			}),
	)
}

impl Shell {
	/// While the connection is down (or playback needs a nudge after it came back): a compact
	/// pill in the top bar; the full sentence is its tooltip.
	pub fn network_pill(&self) -> Option<impl IntoElement> {
		let st = self.st();
		let text =
			st.network_notice.clone().or_else(|| st.offline.then(|| "You're offline. Showing music saved on this computer.".to_owned()))?;
		let short = if st.offline { "Offline" } else { "Reconnected" };
		Some(
			div()
				.id("network")
				.h(px(32.))
				.px_3()
				.flex()
				.items_center()
				.gap_2()
				.rounded_full()
				.bg(theme::raised())
				.text_size(px(14.))
				.text_color(theme::subdued())
				.tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(text.clone()).build(window, cx))
				.child(crate::widgets::icon(crate::widgets::Icon::Offline, px(16.), theme::subdued()))
				.child(short),
		)
	}
}

/// The field a folder's name is typed into while it's renamed in Your Library; every edit goes
/// to the core, and Enter or leaving the field saves.
fn folder_name_input(window: &mut Window, cx: &mut Context<'_, Shell>) -> Entity<InputState> {
	let folder_name = cx.new(|cx| InputState::new(window, cx));
	cx.subscribe(&folder_name, |this, input, e: &InputEvent, cx| match e {
		InputEvent::Change => {
			let text = input.read(cx).value().to_string();
			this.send(Msg::SetText(Field::PlaylistName, text), cx);
		},
		InputEvent::PressEnter { .. } | InputEvent::Blur => {
			if let Some(turntable_core::Dialog::RenameFolder { id, .. }) = this.st().dialog.clone() {
				let name = this.st().name_input.clone();
				this.send(Msg::RenameFolder(id, name), cx);
			}
		},
		InputEvent::Focus => {},
	})
	.detach();
	folder_name
}

/// The top bar's search field: typing feeds the core's query (the dropdown suggests), Enter
/// searches and opens the results.
fn search_input(window: &mut Window, cx: &mut Context<'_, Shell>) -> Entity<InputState> {
	let search = cx.new(|cx| InputState::new(window, cx).placeholder("What do you want to play?"));
	cx.subscribe(&search, |this, input, e: &InputEvent, cx| match e {
		InputEvent::Change => {
			let text = input.read(cx).value().to_string();
			// The core owns the query (and debounces the search it sends).
			this.send(Msg::SetText(Field::Search, text), cx);
			// Typing shows suggestions under the field; the page changes on Enter (Spotify).
			this.search_open = true;
			cx.notify();
		},
		InputEvent::PressEnter { .. } => {
			let text = input.read(cx).value().trim().to_owned();
			this.search_open = false;
			if !text.is_empty() {
				this.send(Msg::SearchSubmit(text), cx);
				this.go(Route::Search, cx);
			}
			cx.notify();
		},
		InputEvent::Focus => {
			this.search_open = true;
			cx.notify();
		},
		// A click on a suggestion acts on mouse-down, before this blur closes the list.
		InputEvent::Blur => {
			this.search_open = false;
			cx.notify();
		},
	})
	.detach();
	search
}

impl Shell {
	fn key_actions(&self, cx: &mut Context<'_, Self>) -> gpui_kit::Div {
		div()
			.key_context("Shell")
			.track_focus(&self.focus)
			.on_action(cx.listener(|this, _: &CloseDialog, window, cx| {
				if this.shortcuts_open {
					this.toggle_shortcut_help(window, cx);
					return;
				}
				if this.menu.is_some() {
					this.close_menu(cx);
					return;
				}
				match this.full_view {
					FullView::Screen => this.toggle_full_screen(window, cx),
					FullView::Expanded => this.set_full_view(false, cx),
					FullView::Off => this.send(Msg::CloseDialog, cx),
				}
			}))
			.on_action(cx.listener(|this, _: &TogglePlay, window, cx| this.character_shortcut(Msg::TogglePlay, window, cx)))
			.on_action(cx.listener(|this, _: &Mute, window, cx| this.character_shortcut(Msg::ToggleMute, window, cx)))
			.on_action(cx.listener(|this, _: &LikeSong, _, cx| this.send(Msg::ToggleLike, cx)))
			.on_action(cx.listener(|this, _: &Shuffle, _, cx| this.send(Msg::ToggleShuffle, cx)))
			.on_action(cx.listener(|this, _: &Repeat, _, cx| this.send(Msg::CycleRepeat, cx)))
			.on_action(cx.listener(|this, _: &Home, _, cx| this.go(Route::Home, cx)))
			.on_action(cx.listener(|this, _: &LikedSongs, _, cx| this.go(Route::Liked, cx)))
			.on_action(cx.listener(|this, _: &VolumeUp, _, cx| this.send(Msg::Volume(this.st().volume.saturating_add(5).min(100)), cx)))
			.on_action(cx.listener(|this, _: &VolumeDown, _, cx| this.send(Msg::Volume(this.st().volume.saturating_sub(5)), cx)))
			.on_action(cx.listener(|this, _: &FocusFilter, window, cx| this.focus_library_filter(window, cx)))
			.on_action(cx.listener(|this, _: &ShortcutHelp, window, cx| this.toggle_shortcut_help(window, cx)))
			.on_action(cx.listener(|this, _: &Next, _, cx| this.send(Msg::Next, cx)))
			.on_action(cx.listener(|this, _: &Previous, _, cx| this.send(Msg::Prev, cx)))
			.on_action(cx.listener(|this, _: &Back, _, cx| this.go_back(cx)))
			.on_action(cx.listener(|this, _: &Forward, _, cx| this.go_forward(cx)))
			.on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
				this.search.update(cx, |s, cx| s.focus(window, cx));
				this.go(Route::Search, cx);
			}))
			.on_action(cx.listener(|this, _: &ToggleLibrary, _, cx| this.toggle_library(cx)))
			.on_action(cx.listener(|this, _: &OpenSettings, _, cx| this.open(Page::Settings, cx)))
			.on_action(cx.listener(|this, _: &ToggleQueue, _, cx| this.toggle_side(Side::Core(Panel::Queue), cx)))
			.on_action(cx.listener(|this, _: &ToggleLyrics, _, cx| this.toggle_side(Side::Core(Panel::Lyrics), cx)))
			.on_action(cx.listener(|this, _: &ToggleNowPlaying, _, cx| this.toggle_side(Side::Core(Panel::NowPlaying), cx)))
	}

	/// Also guard explicitly dispatched actions, not just key bindings.
	fn character_shortcut(&mut self, msg: Msg, window: &mut Window, cx: &mut Context<'_, Self>) {
		if window.has_focused_input(cx) {
			cx.propagate();
		} else {
			self.send(msg, cx);
		}
	}

	fn toggle_shortcut_help(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
		self.shortcuts_open = !self.shortcuts_open;
		if self.shortcuts_open {
			self.shortcuts_focus = window.focused(cx);
			window.focus(&self.focus, cx);
		} else if let Some(focus) = self.shortcuts_focus.take() {
			window.focus(&focus, cx);
		}
		cx.notify();
	}

	fn shortcut_help(cx: &mut Context<'_, Self>) -> impl IntoElement {
		let shortcuts = [
			("Play / pause (outside text fields)", "Space"),
			("Mute (outside text fields)", "M"),
			("Search", primary("⌘⇧L / ⌘K", "Ctrl+Shift+L / Ctrl+K")),
			("Filter Your Library", primary("⌘F", "Ctrl+F")),
			("Like current song", "Alt+Shift+B"),
			("Shuffle / repeat", "Alt+S / Alt+R"),
			("Home / Liked Songs", "Alt+Shift+H / Alt+Shift+S"),
			("Now Playing", "Alt+Shift+J / Alt+Shift+R"),
			("Library / queue / lyrics", "Alt+Shift+L / Q / Y"),
			("Volume up / down", primary("⌘↑ / ⌘↓", "Ctrl+↑ / Ctrl+↓")),
			("Next / previous", primary("⌘→ / ⌘←", "Ctrl+→ / Ctrl+←")),
			("Back / forward", primary("⌘[ / ⌘]", "Ctrl+[ / Ctrl+]")),
			("Settings", primary("⌘,", "Ctrl+,")),
			("Keyboard shortcuts", primary("⌘/", "Ctrl+/")),
			("Close", "Esc"),
		];
		div()
			.absolute()
			.inset_0()
			.flex()
			.items_center()
			.justify_center()
			.bg(theme::alpha(theme::base(), 0.7))
			.child(
				div()
					.id("shortcut-scrim")
					.absolute()
					.inset_0()
					.on_click(cx.listener(|this, _, window, cx| this.toggle_shortcut_help(window, cx))),
			)
			.child(
				div()
					.id("shortcut-help")
					.occlude()
					.relative()
					.w(px(550.))
					.p_6()
					.rounded(theme::RADIUS)
					.bg(theme::raised())
					.flex()
					.flex_col()
					.gap_2()
					.child(div().text_size(px(22.)).child("Keyboard shortcuts"))
					.children(shortcuts.into_iter().map(|(label, keys)| {
						div()
							.flex()
							.justify_between()
							.gap_4()
							.text_size(px(14.))
							.child(label)
							.child(div().text_color(theme::subdued()).child(keys))
					})),
			)
	}
}
