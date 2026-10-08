//! Spotify's context menu: a dark rounded panel at the pointer with icon rows, full-width
//! dividers and submenus that open beside their row. One menu is open at a time; it closes on
//! a click outside, Escape, or after an item runs.

use gpui_kit::{
	AnyElement, App, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Point,
	StatefulInteractiveElement as _, Styled as _, deferred, div, prelude::FluentBuilder as _, px,
};
use turntable_core::Msg;

use crate::{
	shell::Shell,
	theme,
	widgets::{Icon, icon},
};

const WIDTH: f32 = 252.;
const SUB_WIDTH: f32 = 220.;
const ROW_H: f32 = 40.;

/// What a row does.
#[derive(Clone)]
pub enum Action {
	Send(Msg),
	/// Several messages in order (e.g. resolve a context, then add it).
	SendAll(Vec<Msg>),
	CopyText(String),
	/// Opens a page.
	Go(turntable_core::Route),
	/// Opens one of the app's own pages (Settings, Equalizer).
	OpenPage(crate::shell::Page),
	/// Opens a submenu beside this row.
	Sub(Vec<Item>),
}

#[derive(Clone)]
pub enum Item {
	Row { icon: Option<Icon>, label: String, action: Action, disabled: bool },
	Divider,
}

impl Item {
	pub fn row(icon: Icon, label: impl Into<String>, action: Action) -> Self {
		Self::Row { icon: Some(icon), label: label.into(), action, disabled: false }
	}

	/// A row without an icon (submenu entries).
	pub fn plain(label: impl Into<String>, action: Action) -> Self {
		Self::Row { icon: None, label: label.into(), action, disabled: false }
	}

	pub fn disabled(mut self) -> Self {
		if let Self::Row { disabled, .. } = &mut self {
			*disabled = true;
		}
		self
	}
}

/// An open menu: where it was opened and what it lists.
pub struct Open {
	pub at:    Point<Pixels>,
	pub items: Vec<Item>,
	/// The row whose submenu is showing.
	pub sub:   Option<usize>,
}

impl Shell {
	/// Opens `items` at the pointer (replacing any open menu).
	pub fn open_menu(&mut self, at: Point<Pixels>, items: Vec<Item>, cx: &mut Context<'_, Self>) {
		self.menu = Some(Open { at, items, sub: None });
		cx.notify();
	}

	pub fn close_menu(&mut self, cx: &mut Context<'_, Self>) {
		if self.menu.take().is_some() {
			cx.notify();
		}
	}

	fn run(&mut self, action: &Action, cx: &mut Context<'_, Self>) {
		match action {
			Action::Send(msg) => self.send(msg.clone(), cx),
			Action::SendAll(msgs) => {
				for msg in msgs {
					self.send(msg.clone(), cx);
				}
			},
			Action::CopyText(text) => cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text.clone())),
			Action::Go(route) => self.go(route.clone(), cx),
			Action::OpenPage(page) => self.open(page.clone(), cx),
			Action::Sub(_) => return,
		}
		self.close_menu(cx);
	}

	/// The open menu, drawn above everything, kept inside the window.
	pub fn menu_overlay(&mut self, window: &gpui_kit::Window, cx: &mut Context<'_, Self>) -> Option<AnyElement> {
		let open = self.menu.as_ref()?;
		let viewport = window.viewport_size();
		let rows = open.items.iter().filter(|i| matches!(i, Item::Row { .. })).count();
		let dividers = open.items.len() - rows;
		let height = ROW_H * rows as f32 + 9. * dividers as f32 + 8.;
		let x = f32::from(open.at.x).min(f32::from(viewport.width) - WIDTH - 8.).max(8.);
		let y = f32::from(open.at.y).min(f32::from(viewport.height) - height - 8.).max(8.);
		// Submenus open to the right, or to the left when there's no room.
		let sub_left = x + WIDTH + SUB_WIDTH + 8. > f32::from(viewport.width);
		let (items, sub) = (open.items.clone(), open.sub);
		let mut offset = 4.;
		let mut sub_panel = None;
		let mut children = Vec::with_capacity(items.len());
		for (i, item) in items.into_iter().enumerate() {
			match item {
				Item::Divider => {
					children.push(div().my(px(4.)).h(px(1.)).bg(theme::alpha(theme::text(), 0.1)).into_any_element());
					offset += 9.;
				},
				Item::Row { icon: glyph, label, action, disabled } => {
					let open_sub = sub == Some(i);
					if open_sub && let Action::Sub(entries) = &action {
						sub_panel = Some((offset, entries.clone()));
					}
					children.push(Self::menu_row(("menu-row", i), Row { glyph, label, action, disabled, sub_open: open_sub }, Some(i), cx));
					offset += ROW_H;
				},
			}
		}
		let main = panel(WIDTH).children(children);
		let sub_el = sub_panel.map(|(top, entries)| {
			let rows: Vec<AnyElement> = entries
				.into_iter()
				.enumerate()
				.map(|(j, item)| match item {
					Item::Divider => div().my(px(4.)).h(px(1.)).bg(theme::alpha(theme::text(), 0.1)).into_any_element(),
					Item::Row { icon: glyph, label, action, disabled } => {
						Self::menu_row(("menu-sub-row", j), Row { glyph, label, action, disabled, sub_open: false }, None, cx)
					},
				})
				.collect();
			panel(SUB_WIDTH)
				.absolute()
				.top(px(top - 4.))
				.when(sub_left, |s| s.right(px(WIDTH - 4.)))
				.when(!sub_left, |s| s.left(px(WIDTH - 4.)))
				.max_h(px(f32::from(viewport.height) - y - top - 16.))
				.id("menu-sub")
				.overflow_y_scroll()
				.children(rows)
		});
		Some(
			deferred(
				div()
					.absolute()
					.inset_0()
					.id("menu-scrim")
					.occlude()
					.on_mouse_down(gpui_kit::MouseButton::Left, cx.listener(|this, _, _, cx| this.close_menu(cx)))
					.on_mouse_down(gpui_kit::MouseButton::Right, cx.listener(|this, _, _, cx| this.close_menu(cx)))
					.child(
						div()
							.id("menu")
							.occlude()
							.absolute()
							.left(px(x))
							.top(px(y))
							.on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx: &mut App| cx.stop_propagation())
							.child(main)
							.children(sub_el),
					),
			)
			.with_priority(2)
			.into_any_element(),
		)
	}

	/// One row: icon, label, a chevron for submenus. `index` is the row's place in the top
	/// level (`None` inside a submenu), which hovering opens or closes a submenu by.
	fn menu_row(id: (&'static str, usize), row: Row, index: Option<usize>, cx: &mut Context<'_, Self>) -> AnyElement {
		let Row { glyph, label, action, disabled, sub_open } = row;
		let has_sub = matches!(action, Action::Sub(_));
		// Spotify's menu text is 90% white.
		let color = theme::alpha(theme::text(), if disabled { 0.4 } else { 0.9 });
		div()
			.id(id)
			.h(px(ROW_H))
			.pl_3()
			.pr_2()
			.flex()
			.items_center()
			.gap_3()
			.rounded(px(2.))
			.text_size(px(14.))
			.text_color(color)
			.when(sub_open, |s| s.bg(theme::alpha(theme::text(), 0.1)))
			.when(!disabled, |s| s.cursor_pointer().hover(|s| s.bg(theme::alpha(theme::text(), 0.1))))
			// Resting on a row opens its submenu, or closes another's.
			.when_some(index, |s, i| {
				s.on_hover(cx.listener(move |this, over: &bool, _, cx| {
					if *over && let Some(menu) = &mut this.menu {
						let want = has_sub.then_some(i);
						if menu.sub != want {
							menu.sub = want;
							cx.notify();
						}
					}
				}))
			})
			.when(!disabled, |s| {
				s.on_click(cx.listener(move |this, _, _, cx| {
					if let (Action::Sub(_), Some(i)) = (&action, index) {
						if let Some(menu) = &mut this.menu {
							menu.sub = Some(i);
							cx.notify();
						}
					} else {
						this.run(&action, cx);
					}
				}))
			})
			.when_some(glyph, |s, g| s.child(icon(g, px(16.), color)))
			.child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::NORMAL).child(label))
			.when(has_sub, |s| s.child(icon(Icon::MenuChevron, px(16.), color)))
			.into_any_element()
	}
}

/// A row's parts, as `menu_row` draws them.
struct Row {
	glyph:    Option<Icon>,
	label:    String,
	action:   Action,
	disabled: bool,
	sub_open: bool,
}

/// The menu's panel: Spotify's raised grey with a soft shadow.
fn panel(width: f32) -> gpui_kit::Div {
	div().w(px(width)).p_1().flex().flex_col().rounded(px(4.)).bg(theme::menu()).shadow_lg().text_color(theme::text())
}
