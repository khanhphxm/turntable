//! The window's root view: the core controller, navigation and the layout (top bar, library,
//! main view, right-hand panel, player bar). Pages and panels live in their own modules as
//! `impl Shell` blocks and read `self.st()`.

use std::{sync::Arc, time::Instant};

use gpui_kit::{
	AppContext as _, Context, Entity, FocusHandle, Focusable, ImageSource, InteractiveElement as _, IntoElement, KeyBinding, ListAlignment,
	ListState, ParentElement as _, Render, Styled as _, Window, actions,
	component::input::{InputEvent, InputState},
	div,
	prelude::FluentBuilder as _,
	px,
};
use turntable_core::{Controller, Field, Msg, Panel, Route, State};

use crate::{
	covers::CoverCache,
	theme,
	widgets::{Slider, SliderEvent},
};

actions!(turntable, [TogglePlay, Next, Previous, Back, Forward, FocusSearch, ToggleLibrary, ToggleQueue, ToggleLyrics, ToggleNowPlaying]);

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

/// A page of the main view: one of the core's routes, or one of the app's own pages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Page {
	Core(Route),
	Equalizer,
	Settings,
}

/// What the right-hand panel shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
	Core(Panel),
	Devices,
}

pub struct Shell {
	pub core:          Controller,
	pub page:          Page,
	pub back:          Vec<Page>,
	pub forward:       Vec<Page>,
	/// Bumped whenever the right-hand panel changes, so its entrance animation runs again.
	pub panel_gen:     usize,
	pub seek:          Entity<Slider>,
	pub volume_slider: Entity<Slider>,
	/// The main view's virtual list (listing pages), and what it was built for.
	pub list:          ListState,
	pub list_for:      Option<(Route, usize)>,
	pub search:        Entity<InputState>,
	pub eq:            Vec<Entity<Slider>>,
	pub focus:         FocusHandle,
	/// The main view's width as of this frame (titles size themselves to it).
	pub main_w:        f32,
	/// The window's scale factor as of this frame (covers decode to it).
	pub scale:         f32,
	pub lyrics_scroll: gpui_kit::ScrollHandle,
	/// The lyric line last scrolled to.
	pub lyrics_line:   Option<usize>,
}

impl Focusable for Shell {
	fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
		self.focus.clone()
	}
}

impl Shell {
	pub fn new(core: Controller, window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
		let seek = Slider::new(cx, 0.);
		let volume_slider = Slider::new(cx, f32::from(core.state().volume) / 100.);
		cx.subscribe(&seek, |this, _, e: &SliderEvent, cx| {
			if let SliderEvent::Commit(v) = e {
				let ms = (f64::from(*v) * this.st().duration_ms() as f64) as u64;
				this.send(Msg::Seek(ms), cx);
			}
		})
		.detach();
		cx.subscribe(&volume_slider, |this, _, e: &SliderEvent, cx| {
			let (SliderEvent::Scrub(v) | SliderEvent::Commit(v)) = e;
			this.send(Msg::Volume((v * 100.).round() as u8), cx);
		})
		.detach();
		let search = cx.new(|cx| InputState::new(window, cx).placeholder("What do you want to play?"));
		cx.subscribe(&search, |this, input, e: &InputEvent, cx| match e {
			InputEvent::Change => {
				let text = input.read(cx).value().to_string();
				// The core owns the query (and debounces the search it sends).
				this.send(Msg::Focus(Some(Field::Search)), cx);
				let old = this.st().library.query.encode_utf16().count();
				this.send(Msg::Edit { from: 0, to: old, text }, cx);
				if this.page != Page::Core(Route::Search) {
					this.open(Page::Core(Route::Search), cx);
				}
			},
			InputEvent::PressEnter { .. } => {
				let text = input.read(cx).value().to_string();
				this.send(Msg::SearchSubmit(text), cx);
			},
			_ => {},
		})
		.detach();
		let eq: Vec<Entity<Slider>> =
			core.state().eq.gains_db.iter().map(|db| Slider::vertical(cx, crate::pages::eq::db_to_value(*db))).collect();
		for (band, slider) in eq.iter().enumerate() {
			cx.subscribe(slider, move |this, _, e: &SliderEvent, cx| {
				if let SliderEvent::Commit(v) = e {
					this.send(Msg::EqBand(band, crate::pages::eq::value_to_step(*v)), cx);
				}
			})
			.detach();
		}
		let page = Page::Core(core.state().library.route.clone());
		let mut shell = Self {
			core,
			page,
			back: Vec::new(),
			forward: Vec::new(),
			panel_gen: 0,
			seek,
			volume_slider,
			list: ListState::new(0, ListAlignment::Top, px(600.)),
			list_for: None,
			search,
			eq,
			focus: cx.focus_handle(),
			main_w: 600.,
			scale: window.scale_factor(),
			lyrics_scroll: gpui_kit::ScrollHandle::new(),
			lyrics_line: None,
		};
		shell.send(Msg::Navigate(Route::Home), cx);
		shell
	}

	pub fn st(&self) -> &State {
		self.core.state()
	}

	/// Hands `msg` to the core and redraws when it changed something.
	pub fn send(&mut self, msg: Msg, cx: &mut Context<'_, Self>) {
		let before = self.st().side_panel();
		if self.core.dispatch(msg) {
			self.after_change(before, cx);
		}
	}

	/// Workers published or a timer came due: take it in.
	pub fn pump(&mut self, cx: &mut Context<'_, Self>) {
		let before = self.st().side_panel();
		// While something plays the clock and seek bar move on every tick.
		if self.core.pump() || self.st().playing() {
			self.after_change(before, cx);
		}
	}

	/// When to wake next: the core's next timer, or the next whole second of playback.
	pub fn next_wake(&self) -> Option<Instant> {
		let tick = self.st().playing().then(|| {
			let ms = self.st().position_ms();
			self.st().anchor.instant_at(ms - ms % 1000 + 1000)
		});
		[self.core.deadline(), tick.flatten()].into_iter().flatten().min()
	}

	fn after_change(&mut self, panel_before: Option<Panel>, cx: &mut Context<'_, Self>) {
		if self.st().side_panel() != panel_before {
			self.panel_gen += 1;
		}
		let st = self.core.state();
		let total = st.duration_ms().max(1);
		let (pos, vol) = (st.position_ms() as f32 / total as f32, f32::from(st.volume) / 100.);
		self.seek.update(cx, |s, cx| s.set(pos, cx));
		self.volume_slider.update(cx, |s, cx| s.set(vol, cx));
		// The band sliders follow the core (a preset moves them all).
		let gains = self.core.state().eq.gains_db;
		for (slider, db) in self.eq.iter().zip(gains) {
			slider.update(cx, |s, cx| s.set(crate::pages::eq::db_to_value(db), cx));
		}
		if self.st().quit {
			cx.quit();
		}
		cx.notify();
	}

	/// A cover at `logical` px, decoded off-thread: `None` until it's ready (the core fetches it).
	pub fn image(&self, url: Option<&str>, logical: u32, cx: &mut Context<'_, Self>) -> Option<ImageSource> {
		let cover = Arc::clone(self.st().image(url)?);
		CoverCache::get(&cover, logical, self.scale, cx).map(ImageSource::Render)
	}

	// --- Navigation -----------------------------------------------------------------------

	pub fn open(&mut self, page: Page, cx: &mut Context<'_, Self>) {
		if self.page != page {
			let from = std::mem::replace(&mut self.page, page.clone());
			self.back.push(from);
			self.forward.clear();
			self.show(page, cx);
		}
	}

	pub fn go(&mut self, route: Route, cx: &mut Context<'_, Self>) {
		self.open(Page::Core(route), cx);
	}

	pub fn go_back(&mut self, cx: &mut Context<'_, Self>) {
		if let Some(p) = self.back.pop() {
			let from = std::mem::replace(&mut self.page, p.clone());
			self.forward.push(from);
			self.show(p, cx);
		}
	}

	pub fn go_forward(&mut self, cx: &mut Context<'_, Self>) {
		if let Some(p) = self.forward.pop() {
			let from = std::mem::replace(&mut self.page, p.clone());
			self.back.push(from);
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

	/// What the right-hand panel shows: the device list while it's open, else the core's panel
	/// (only the panels that belong there; the equalizer and settings are pages here).
	pub fn side(&self) -> Option<Side> {
		if self.st().connect.menu {
			return Some(Side::Devices);
		}
		match self.st().side_panel()? {
			p @ (Panel::NowPlaying | Panel::Queue | Panel::Lyrics) => Some(Side::Core(p)),
			Panel::Eq | Panel::Settings => None,
		}
	}

	pub fn toggle_side(&mut self, side: Side, cx: &mut Context<'_, Self>) {
		self.panel_gen += 1;
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
}

/// Drives the core: sleeps until a worker wakes it or its next timer is due, then pumps.
pub fn run_core(shell: &Entity<Shell>, wake: smol::channel::Receiver<()>, cx: &mut gpui_kit::App) {
	let shell = shell.downgrade();
	cx.spawn(async move |cx| {
		loop {
			let deadline = shell.update(cx, |s, _| s.next_wake()).ok().flatten();
			let woken = wake.recv();
			match deadline {
				Some(at) => {
					let timer = cx.background_executor().timer(at.saturating_duration_since(Instant::now()));
					smol::future::or(async { woken.await.map(|()| ()).ok() }, async {
						timer.await;
						Some(())
					})
					.await;
				},
				None => {
					if woken.await.is_err() {
						break;
					}
				},
			}
			if shell.update(cx, |s, cx| s.pump(cx)).is_err() {
				break;
			}
		}
	})
	.detach();
}

impl Render for Shell {
	fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
		if !self.st().signed_in {
			return self.welcome_page(cx).into_any_element();
		}
		self.scale = window.scale_factor();
		let side = self.side();
		let left = if self.st().sidebar_collapsed() { theme::SIDEBAR_COLLAPSED_W } else { theme::SIDEBAR_W };
		let right = if side.is_some() { theme::RIGHT_W + theme::GAP } else { px(0.) };
		self.main_w = f32::from(window.viewport_size().width - left - right - theme::GAP * 3.);
		let main = match self.page.clone() {
			Page::Core(Route::Home | Route::Library) => self.home_page(cx).into_any_element(),
			Page::Core(Route::Search) => self.search_page(cx).into_any_element(),
			Page::Core(Route::Artist(_)) => self.artist_page(cx).into_any_element(),
			Page::Core(route @ (Route::Liked | Route::Playlist(_) | Route::Album(_))) => self.listing_page(route, cx).into_any_element(),
			Page::Settings => self.settings_page(cx).into_any_element(),
			Page::Equalizer => self.eq_page(cx).into_any_element(),
		};
		let toast = self.st().toast.clone();
		div()
			.key_context("Shell")
			.track_focus(&self.focus)
			.on_action(cx.listener(|this, _: &TogglePlay, _, cx| this.send(Msg::TogglePlay, cx)))
			.on_action(cx.listener(|this, _: &Next, _, cx| this.send(Msg::Next, cx)))
			.on_action(cx.listener(|this, _: &Previous, _, cx| this.send(Msg::Prev, cx)))
			.on_action(cx.listener(|this, _: &Back, _, cx| this.go_back(cx)))
			.on_action(cx.listener(|this, _: &Forward, _, cx| this.go_forward(cx)))
			.on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
				this.search.update(cx, |s, cx| s.focus(window, cx));
				this.go(Route::Search, cx);
			}))
			.on_action(cx.listener(|this, _: &ToggleLibrary, _, cx| this.send(Msg::ToggleSidebar, cx)))
			.on_action(cx.listener(|this, _: &ToggleQueue, _, cx| this.toggle_side(Side::Core(Panel::Queue), cx)))
			.on_action(cx.listener(|this, _: &ToggleLyrics, _, cx| this.toggle_side(Side::Core(Panel::Lyrics), cx)))
			.on_action(cx.listener(|this, _: &ToggleNowPlaying, _, cx| this.toggle_side(Side::Core(Panel::NowPlaying), cx)))
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
							.when_some(toast, |s, (id, text)| s.child(toast_view(text, id))),
					)
					.when_some(side, |row, side| row.child(self.right_panel(side, cx))),
			)
			.child(self.player_bar(cx))
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
