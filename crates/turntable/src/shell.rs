//! The window's root view: state, navigation and the layout (top bar, library, main view,
//! right-hand panel, player bar). Pages and panels live in their own modules as `impl Shell`.

use std::time::{Duration, Instant};

use gpui_kit::{
	AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement as _, IntoElement, KeyBinding, ListAlignment, ListState,
	ParentElement as _, Render, Styled as _, Window, actions,
	component::input::{InputEvent, InputState},
	div,
	prelude::FluentBuilder as _,
	px,
};

use crate::{
	mock::{Collection, Library},
	theme,
	widgets::{Slider, SliderEvent},
};

actions!(turntable, [TogglePlay, Next, Previous, Back, Forward, FocusSearch, ToggleLibrary, ToggleQueue, ToggleLyrics, ToggleNowPlaying]);

/// How long a toast stays up.
const TOAST_FOR: Duration = Duration::from_millis(2600);

pub fn bind_keys(cx: &mut gpui_kit::App) {
	cx.bind_keys([
		KeyBinding::new("space", TogglePlay, Some("Shell")),
		KeyBinding::new("cmd-right", Next, Some("Shell")),
		KeyBinding::new("cmd-left", Previous, Some("Shell")),
		KeyBinding::new("cmd-[", Back, Some("Shell")),
		KeyBinding::new("cmd-]", Forward, Some("Shell")),
		KeyBinding::new("cmd-k", FocusSearch, Some("Shell")),
		KeyBinding::new("cmd-l", FocusSearch, Some("Shell")),
		KeyBinding::new("alt-shift-l", ToggleLibrary, Some("Shell")),
		KeyBinding::new("alt-shift-q", ToggleQueue, Some("Shell")),
		KeyBinding::new("alt-shift-r", ToggleNowPlaying, Some("Shell")),
		KeyBinding::new("alt-shift-y", ToggleLyrics, Some("Shell")),
	]);
}

/// A page of the main view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
	Home,
	Search,
	Collection(Collection),
	Artist(usize),
	Settings,
	Equalizer,
}

/// What the right-hand panel shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
	NowPlaying,
	Queue,
	Lyrics,
	Devices,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repeat {
	Off,
	All,
	One,
}

/// Your Library's filter chips.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryFilter {
	Playlists,
	Albums,
	Artists,
}

/// What plays: a position in a collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NowPlaying {
	pub from: Collection,
	pub pos:  usize,
}

pub struct Prefs {
	pub normalize:  bool,
	pub autoplay:   bool,
	pub crossfade:  bool,
	pub lyrics_bar: bool,
	pub quality:    usize,
}

pub struct Shell {
	pub lib:               Library,
	pub route:             Route,
	pub back:              Vec<Route>,
	pub forward:           Vec<Route>,
	pub panel:             Option<Panel>,
	/// Bumped on every panel change so the panel's entrance animation runs again.
	pub panel_gen:         usize,
	pub sidebar_collapsed: bool,
	pub library_filter:    Option<LibraryFilter>,
	pub now:               Option<NowPlaying>,
	/// Tracks the user queued ("Add to queue"): they play before the collection continues.
	pub queue:             Vec<usize>,
	pub playing:           bool,
	/// Where the track was at `anchor`.
	pub position_ms:       u64,
	pub anchor:            Instant,
	pub shuffle:           bool,
	pub repeat:            Repeat,
	pub volume:            f32,
	/// The volume to go back to on unmute.
	pub unmute_to:         f32,
	pub seek:              Entity<Slider>,
	pub volume_slider:     Entity<Slider>,
	/// The main view's virtual list (collection pages).
	pub list:              ListState,
	/// The selected row of the open track table.
	pub selected:          Option<usize>,
	pub search:            Entity<InputState>,
	pub query:             String,
	pub device:            usize,
	pub eq_on:             bool,
	pub eq_preset:         Option<&'static str>,
	pub eq:                Vec<Entity<Slider>>,
	pub prefs:             Prefs,
	pub focus:             FocusHandle,
	/// The main view's width as of this frame (titles size themselves to it).
	pub main_w:            f32,
	pub toast:             Option<&'static str>,
	pub signed_in:         bool,
	pub signing_in:        bool,
	pub toast_gen:         usize,
	pub lyrics_scroll:     gpui_kit::ScrollHandle,
	/// The lyric line last scrolled to.
	pub lyrics_line:       Option<usize>,
}

impl Focusable for Shell {
	fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
		self.focus.clone()
	}
}

impl Shell {
	pub fn new(window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
		let seek = Slider::new(cx, 0.);
		let volume_slider = Slider::new(cx, 0.7);
		cx.subscribe(&seek, |this, _, e: &SliderEvent, cx| {
			if let SliderEvent::Commit(v) = e {
				let total = this.now_track().map_or(0, |t| t.ms);
				this.position_ms = (f64::from(*v) * total as f64) as u64;
				this.anchor = Instant::now();
				cx.notify();
			}
		})
		.detach();
		cx.subscribe(&volume_slider, |this, _, e: &SliderEvent, cx| {
			let (SliderEvent::Scrub(v) | SliderEvent::Commit(v)) = e;
			this.volume = *v;
			cx.notify();
		})
		.detach();
		let search = cx.new(|cx| InputState::new(window, cx).placeholder("What do you want to play?"));
		cx.subscribe(&search, |this, input, e: &InputEvent, cx| {
			if matches!(e, InputEvent::Change) {
				this.query = input.read(cx).value().trim().to_owned();
				if this.route != Route::Search {
					this.open(Route::Search, cx);
				}
				cx.notify();
			}
		})
		.detach();
		let eq: Vec<Entity<Slider>> = (0..10).map(|_| Slider::vertical(cx, crate::pages::eq::db_to_value(0.))).collect();
		for band in &eq {
			// A hand-set band is no longer the preset.
			cx.subscribe(band, |this, _, e: &SliderEvent, cx| {
				if matches!(e, SliderEvent::Commit(_)) {
					this.eq_preset = None;
					cx.notify();
				}
			})
			.detach();
		}
		let lib = Library::demo();
		let first = Collection::Playlist(0);
		let rows = lib.tracks_of(first).len();
		let mut shell = Self {
			lib,
			route: Route::Collection(first),
			back: Vec::new(),
			forward: Vec::new(),
			panel: Some(Panel::NowPlaying),
			panel_gen: 0,
			sidebar_collapsed: false,
			library_filter: None,
			now: Some(NowPlaying { from: first, pos: 0 }),
			queue: Vec::new(),
			playing: false,
			position_ms: 42_000,
			anchor: Instant::now(),
			shuffle: false,
			repeat: Repeat::Off,
			volume: 0.7,
			unmute_to: 0.7,
			seek,
			volume_slider,
			list: ListState::new(rows + 1, ListAlignment::Top, px(600.)),
			selected: None,
			search,
			query: String::new(),
			device: 0,
			eq_on: true,
			eq_preset: Some("Flat"),
			eq,
			prefs: Prefs { normalize: true, autoplay: true, crossfade: false, lyrics_bar: true, quality: 2 },
			focus: cx.focus_handle(),
			main_w: 600.,
			toast: None,
			signed_in: std::env::var_os("TURNTABLE_WELCOME").is_none(),
			signing_in: false,
			toast_gen: 0,
			lyrics_scroll: gpui_kit::ScrollHandle::new(),
			lyrics_line: None,
		};
		shell.sync_seek(cx);
		shell.start_clock(cx);
		shell
	}

	// --- Playback -------------------------------------------------------------------------

	/// Index into `lib.tracks` of what plays.
	pub fn now_index(&self) -> Option<usize> {
		let n = self.now?;
		self.lib.tracks_of(n.from).get(n.pos).copied()
	}

	pub fn now_track(&self) -> Option<&crate::mock::Track> {
		self.lib.tracks.get(self.now_index()?)
	}

	/// Whether row `pos` of `from` is what plays (`track` = usize::MAX skips the track check).
	pub fn is_current(&self, from: Collection, pos: usize, track: usize) -> bool {
		self.now.is_some_and(|n| n.from == from && n.pos == pos) && (track == usize::MAX || self.now_index() == Some(track))
	}

	pub fn position(&self) -> u64 {
		let total = self.now_track().map_or(0, |t| t.ms);
		let at = if self.playing { self.position_ms + self.anchor.elapsed().as_millis() as u64 } else { self.position_ms };
		at.min(total)
	}

	fn sync_seek(&self, cx: &mut Context<'_, Self>) {
		let total = self.now_track().map_or(1, |t| t.ms).max(1);
		let v = self.position() as f32 / total as f32;
		self.seek.update(cx, |s, cx| s.set(v, cx));
	}

	/// Redraws once a second while something plays; nothing runs while paused.
	fn start_clock(&mut self, cx: &mut Context<'_, Self>) {
		cx.spawn(async move |this, cx| {
			loop {
				cx.background_executor().timer(Duration::from_secs(1)).await;
				let alive = this.update(cx, |this, cx| {
					if this.playing {
						if this.position() >= this.now_track().map_or(0, |t| t.ms) {
							this.skip(1, cx);
						}
						this.sync_seek(cx);
						cx.notify();
					}
				});
				if alive.is_err() {
					break;
				}
			}
		})
		.detach();
	}

	pub fn toggle_play(&mut self, cx: &mut Context<'_, Self>) {
		self.position_ms = self.position();
		self.anchor = Instant::now();
		self.playing = !self.playing;
		cx.notify();
	}

	pub fn play_from(&mut self, from: Collection, pos: usize, cx: &mut Context<'_, Self>) {
		self.now = Some(NowPlaying { from, pos });
		self.position_ms = 0;
		self.anchor = Instant::now();
		self.playing = true;
		self.sync_seek(cx);
		cx.notify();
	}

	/// Play/pause a collection: resumes it if it's what plays, else starts it from the top.
	pub fn play_collection(&mut self, c: Collection, cx: &mut Context<'_, Self>) {
		if self.now.is_some_and(|n| n.from == c) {
			self.toggle_play(cx);
		} else {
			self.play_from(c, 0, cx);
		}
	}

	pub fn skip(&mut self, by: isize, cx: &mut Context<'_, Self>) {
		// Previous restarts the track after its first 3 seconds, like every player.
		if by < 0 && self.position() > 3000 {
			self.position_ms = 0;
			self.anchor = Instant::now();
			self.sync_seek(cx);
			cx.notify();
			return;
		}
		if let Some(n) = self.now {
			let len = self.lib.tracks_of(n.from).len().max(1) as isize;
			let next = (n.pos as isize + by).rem_euclid(len) as usize;
			self.play_from(n.from, next, cx);
		}
	}

	pub fn toggle_liked(&mut self, track: usize, cx: &mut Context<'_, Self>) {
		if let Some(t) = self.lib.tracks.get_mut(track) {
			t.liked = !t.liked;
			let msg = if t.liked { "Added to Liked Songs" } else { "Removed from Liked Songs" };
			self.toast(msg, cx);
		}
	}

	pub fn add_to_queue(&mut self, track: usize, cx: &mut Context<'_, Self>) {
		self.queue.push(track);
		self.toast("Added to queue", cx);
	}

	/// A short message over the bottom of the main view, gone after a few seconds.
	pub fn toast(&mut self, text: &'static str, cx: &mut Context<'_, Self>) {
		self.toast_gen += 1;
		let gen_ = self.toast_gen;
		self.toast = Some(text);
		cx.notify();
		cx.spawn(async move |this, cx| {
			cx.background_executor().timer(TOAST_FOR).await;
			let _ = this.update(cx, |this, cx| {
				// A newer toast keeps its own time.
				if this.toast_gen == gen_ {
					this.toast = None;
					cx.notify();
				}
			});
		})
		.detach();
	}

	pub fn toggle_mute(&mut self, cx: &mut Context<'_, Self>) {
		if self.volume > 0. {
			self.unmute_to = self.volume;
			self.volume = 0.;
		} else {
			self.volume = self.unmute_to.max(0.1);
		}
		let v = self.volume;
		self.volume_slider.update(cx, |s, cx| s.set(v, cx));
		cx.notify();
	}

	// --- Navigation -----------------------------------------------------------------------

	pub fn open(&mut self, route: Route, cx: &mut Context<'_, Self>) {
		if self.route != route {
			self.back.push(self.route);
			self.forward.clear();
			self.show(route, cx);
		}
	}

	pub fn go_back(&mut self, cx: &mut Context<'_, Self>) {
		if let Some(r) = self.back.pop() {
			self.forward.push(self.route);
			self.show(r, cx);
		}
	}

	pub fn go_forward(&mut self, cx: &mut Context<'_, Self>) {
		if let Some(r) = self.forward.pop() {
			self.back.push(self.route);
			self.show(r, cx);
		}
	}

	fn show(&mut self, route: Route, cx: &mut Context<'_, Self>) {
		self.route = route;
		if let Route::Collection(c) = route {
			self.list.reset(self.lib.tracks_of(c).len() + 1);
		}
		self.selected = None;
		cx.notify();
	}

	pub fn toggle_panel(&mut self, panel: Panel, cx: &mut Context<'_, Self>) {
		self.panel = if self.panel == Some(panel) { None } else { Some(panel) };
		self.panel_gen += 1;
		cx.notify();
	}
}

impl Render for Shell {
	fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let side = if self.sidebar_collapsed { theme::SIDEBAR_COLLAPSED_W } else { theme::SIDEBAR_W };
		let right = if self.panel.is_some() { theme::RIGHT_W + theme::GAP } else { px(0.) };
		self.main_w = f32::from(window.viewport_size().width - side - right - theme::GAP * 3.);
		if !self.signed_in {
			return self.welcome_page(cx).into_any_element();
		}
		let main = match self.route {
			Route::Home => self.home_page(cx).into_any_element(),
			Route::Search => self.search_page(cx).into_any_element(),
			Route::Collection(c) => self.collection_page(c, cx).into_any_element(),
			Route::Artist(a) => self.artist_page(a, cx).into_any_element(),
			Route::Settings => self.settings_page(cx).into_any_element(),
			Route::Equalizer => self.eq_page(cx).into_any_element(),
		};
		div()
			.key_context("Shell")
			.track_focus(&self.focus)
			.on_action(cx.listener(|this, _: &TogglePlay, _, cx| this.toggle_play(cx)))
			.on_action(cx.listener(|this, _: &Next, _, cx| this.skip(1, cx)))
			.on_action(cx.listener(|this, _: &Previous, _, cx| this.skip(-1, cx)))
			.on_action(cx.listener(|this, _: &Back, _, cx| this.go_back(cx)))
			.on_action(cx.listener(|this, _: &Forward, _, cx| this.go_forward(cx)))
			.on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
				this.search.update(cx, |s, cx| s.focus(window, cx));
				this.open(Route::Search, cx);
			}))
			.on_action(cx.listener(|this, _: &ToggleLibrary, _, cx| {
				this.sidebar_collapsed = !this.sidebar_collapsed;
				cx.notify();
			}))
			.on_action(cx.listener(|this, _: &ToggleQueue, _, cx| this.toggle_panel(Panel::Queue, cx)))
			.on_action(cx.listener(|this, _: &ToggleLyrics, _, cx| this.toggle_panel(Panel::Lyrics, cx)))
			.on_action(cx.listener(|this, _: &ToggleNowPlaying, _, cx| this.toggle_panel(Panel::NowPlaying, cx)))
			.size_full()
			.flex()
			.flex_col()
			.bg(theme::base())
			.text_color(theme::text())
			.font_family(".SystemUIFont")
			.child(self.top_bar(cx))
			.child(
				div()
					.flex_1()
					.min_h_0()
					.flex()
					.gap(theme::GAP)
					.px(theme::GAP)
					.child(self.sidebar(cx))
					.child(
						div()
							.relative()
							.flex_1()
							.min_w_0()
							.h_full()
							.rounded(theme::RADIUS)
							.overflow_hidden()
							.bg(theme::panel())
							.child(main)
							.when_some(self.toast, |s, text| s.child(toast_view(text, self.toast_gen))),
					)
					.when_some(self.panel, |row, panel| row.child(self.right_panel(panel, cx))),
			)
			.child(self.player_bar(cx))
			.into_any_element()
	}
}

/// Spotify's toast: a white pill centred near the bottom of the main view, rising in.
fn toast_view(text: &'static str, gen_: usize) -> impl IntoElement {
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
			.with_animation(("toast", gen_), Animation::new(Duration::from_millis(220)).with_easing(ease_out_quint()), |el, d| {
				el.opacity(d).mb(px(-12. * (1. - d)))
			}),
	)
}
