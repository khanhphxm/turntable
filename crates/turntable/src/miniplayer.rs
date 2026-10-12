//! Spotify's miniplayer: a small square window floating above the others. The cover fills it;
//! hovering darkens it and shows the controls and progress; the title, the device and ⊕/✓ sit
//! below. The bar's miniplayer button opens it; its red dot closes it.

use std::time::Duration;

use gpui_kit::{
	AppContext as _, Bounds, Context, Entity, FontWeight, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Render,
	StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, WindowBounds, WindowHandle, WindowKind, WindowOptions, div,
	prelude::FluentBuilder as _, px, relative, size,
};
use turntable_core::{
	Msg,
	app::clock,
	engine::{CoverSize, Repeat},
};

use crate::{
	shell::Shell,
	theme,
	widgets::{Icon, added_badge, cover, icon, icon_button},
};

/// The window's side, as Spotify opens it.
const SIDE: f32 = 300.;

pub struct MiniPlayer {
	shell:   Entity<Shell>,
	repaint: Option<Task<()>>,
	_watch:  Subscription,
}

impl MiniPlayer {
	/// Opens the miniplayer, or closes it if it's open.
	pub fn toggle(shell: &Entity<Shell>, cx: &mut gpui_kit::App) {
		if let Some(handle) = shell.read(cx).mini {
			let _ = handle.update(cx, |_, window, _| window.remove_window());
			shell.update(cx, |s, cx| {
				s.mini = None;
				cx.notify();
			});
			return;
		}
		let bounds = Bounds::centered(None, size(px(SIDE), px(SIDE + 64.)), cx);
		let options = WindowOptions {
			window_bounds: Some(WindowBounds::Windowed(bounds)),
			titlebar: None,
			kind: WindowKind::PopUp,
			is_movable: true,
			is_minimizable: false,
			window_min_size: Some(size(px(200.), px(260.))),
			..Default::default()
		};
		let owner = shell.clone();
		let opened = cx.open_window(options, move |_, cx| {
			cx.new(|cx| {
				let watch = cx.observe(&owner, |_, _, cx| cx.notify());
				Self { shell: owner, repaint: None, _watch: watch }
			})
		});
		if let Ok(handle) = opened {
			shell.update(cx, |s, cx| {
				s.mini = Some(handle);
				cx.notify();
			});
		}
	}

	fn send(&self, msg: Msg, cx: &mut Context<'_, Self>) {
		self.shell.update(cx, |s, cx| s.send(msg, cx));
	}
}

impl Render for MiniPlayer {
	#[expect(clippy::too_many_lines, reason = "keep the declarative miniplayer layout together")]
	fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
		if self.repaint.is_none() && window.is_visible() && self.shell.read(cx).st().playing() {
			self.repaint = Some(cx.spawn_in(window, async move |this, cx| {
				cx.background_executor().timer(Duration::from_secs(1)).await;
				let _ = this.update(cx, |this, cx| {
					this.repaint = None;
					cx.notify();
				});
			}));
		}
		let shell = self.shell.read(cx);
		let st = shell.st();
		let item = st.item.clone();
		let playing = st.playing();
		let (shuffle, repeat) = (st.options.shuffle, st.options.repeat);
		let total = st.duration_ms();
		let at = st.position_ms().min(total);
		let progress = if total == 0 { 0. } else { at as f32 / total as f32 };
		let added = st.liked().map(|liked| liked || st.added(&item.uri));
		let device = st.remote().map_or_else(|| st.device_name.clone(), |d| d.name.clone());
		let muted = st.volume == 0;
		let url = item.cover(CoverSize::Large).map(str::to_owned);
		let width = f32::from(window.viewport_size().width);
		#[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "a window's width is a small positive number")]
		let side = width.round() as u32;
		let art = self.shell.update(cx, |s, cx| s.image(url.as_deref(), side, cx));
		let title = if item.uri.is_empty() { "Nothing playing".to_owned() } else { format!("{} • {}", item.name(), item.creators()) };
		let button = |id: &'static str, glyph: Icon, active: bool, msg: Msg, cx: &mut Context<'_, Self>| {
			icon_button(id, glyph, px(18.), active, cx.listener(move |this, _, _, cx| this.send(msg.clone(), cx)))
		};
		div()
			.id("mini")
			.group("mini")
			.size_full()
			.p_1()
			.flex()
			.flex_col()
			.bg(theme::base())
			.text_color(theme::text())
			.font_family(".SystemUIFont")
			.rounded(px(12.))
			.border_1()
			.border_color(theme::raised())
			// The top strip: close dot, drag grip and (Spotify's) settings, shown on hover.
			.child(
				div()
					.h(px(24.))
					.flex_none()
					.px_2()
					.flex()
					.items_center()
					.justify_between()
					.invisible()
					.group_hover("mini", |s| s.visible())
					.on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
					.child(div().id("mini-close").size(px(12.)).rounded_full().bg(gpui_kit::rgb(0x00ED_6A5E)).cursor_pointer().on_click(
						cx.listener(|this, _, _, cx| {
							let shell = this.shell.clone();
							cx.defer(move |cx| MiniPlayer::toggle(&shell, cx));
						}),
					))
					.child(div().text_size(px(10.)).text_color(theme::subdued()).child("⠿"))
					.child(div().size(px(12.))),
			)
			.child(
				div()
					.relative()
					.flex_1()
					.min_h_0()
					.rounded(px(8.))
					.overflow_hidden()
					.bg(theme::raised())
					.flex()
					.items_center()
					.justify_center()
					.child(cover(art, theme::RAISED, px((width * 0.55).max(80.)), px(4.)).shadow_2xl())
					// Hover: a shade with the controls and the progress.
					.child(
						div()
							.absolute()
							.inset_0()
							.invisible()
							.group_hover("mini", |s| s.visible())
							.bg(theme::alpha(theme::base(), 0.7))
							.flex()
							.flex_col()
							.justify_end()
							.child(
								div()
									.flex_1()
									.flex()
									.items_center()
									.justify_center()
									.gap_1()
									.child(button("mini-mute", if muted { Icon::Mute } else { Icon::Volume }, false, Msg::ToggleMute, cx))
									.child(button("mini-shuffle", Icon::Shuffle, shuffle, Msg::ToggleShuffle, cx))
									.child(button("mini-prev", Icon::Prev, false, Msg::Prev, cx))
									.child(
										div()
											.id("mini-play")
											.size(px(48.))
											.rounded_full()
											.bg(theme::text())
											.flex()
											.items_center()
											.justify_center()
											.cursor_pointer()
											.on_click(cx.listener(|this, _, _, cx| this.send(Msg::TogglePlay, cx)))
											.child(icon(if playing { Icon::Pause } else { Icon::Play }, px(20.), theme::base())),
									)
									.child(button("mini-next", Icon::Next, false, Msg::Next, cx))
									.child(button(
										"mini-repeat",
										if repeat == Repeat::Track { Icon::RepeatOne } else { Icon::Repeat },
										repeat != Repeat::Off,
										Msg::CycleRepeat,
										cx,
									))
									.child(icon_button(
										"mini-main",
										Icon::Share,
										px(18.),
										false,
										// Back to the main window.
										cx.listener(|_, _, _, cx| cx.activate(true)),
									)),
							)
							.child(
								div()
									.px_2()
									.pb_2()
									.flex()
									.flex_col()
									.gap_1()
									.child(
										div()
											.flex()
											.justify_between()
											.text_size(px(11.))
											.text_color(theme::subdued())
											.child(clock(at))
											.child(clock(total)),
									)
									.child(
										div()
											.h(px(4.))
											.rounded_full()
											.bg(theme::alpha(theme::text(), 0.3))
											.child(div().h_full().w(relative(progress)).rounded_full().bg(theme::text())),
									),
							),
					),
			)
			.child(
				div()
					.h(px(56.))
					.flex_none()
					.px_1()
					.flex()
					.items_center()
					.gap_2()
					.child(
						div()
							.flex_1()
							.min_w_0()
							.flex()
							.flex_col()
							.child(div().text_size(px(18.)).font_weight(FontWeight::BOLD).truncate().child(title))
							.child(
								div()
									.flex()
									.items_center()
									.gap_1()
									.text_size(px(13.))
									.text_color(theme::accent())
									.child(icon(Icon::Speaker, px(12.), theme::accent()))
									.child(device),
							),
					)
					.when_some(added, |s, added| {
						s.child(
							div()
								.id("mini-add")
								.cursor_pointer()
								.on_click(cx.listener(move |this, _, _, cx| {
									this.send(if added { Msg::AddNowPlayingToPlaylist } else { Msg::ToggleLike }, cx);
								}))
								.child(added_badge(added, px(24.))),
						)
					}),
			)
	}
}

/// The handle the shell keeps to close it again.
pub type Handle = WindowHandle<MiniPlayer>;
