//! The gaps between the main view and either side panel are handles, as in Spotify: the gap
//! lights up under the pointer, the cursor turns to a resize arrow, and dragging sets the panel's
//! width. Your Library snaps to its rail when dragged narrow, and opens again when dragged wide.

use gpui_kit::{
	AppContext as _, Context, CursorStyle, DragMoveEvent, InteractiveElement as _, IntoElement, ParentElement as _, Render,
	StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::{shell::Shell, theme};

/// Which edge is dragged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
	Library,
	Panel,
}

/// The dragged handle; draws nothing under the pointer (the panel itself shows the width).
#[derive(Clone)]
pub struct Dragging(Edge);

impl Render for Dragging {
	fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
		div()
	}
}

/// Your Library: narrower than this snaps to the rail, as Spotify's does.
pub const LIBRARY_MIN: f32 = 280.;
pub const LIBRARY_MAX: f32 = 640.;
pub const PANEL_MIN: f32 = 280.;
pub const PANEL_MAX: f32 = 420.;

impl Shell {
	/// A handle filling the gap left (`Edge::Library`) or right (`Edge::Panel`) of the main view.
	pub fn resize_handle(edge: Edge, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let id = match edge {
			Edge::Library => "resize-library",
			Edge::Panel => "resize-panel",
		};
		div()
			.id(id)
			.group(id)
			.w(theme::GAP)
			.h_full()
			.flex_none()
			.flex()
			.justify_center()
			.cursor(CursorStyle::ResizeLeftRight)
			.on_drag(Dragging(edge), |d, _, _, cx| cx.new(|_| d.clone()))
			.on_drag_move(cx.listener(move |this, e: &DragMoveEvent<Dragging>, window, cx| {
				if e.drag(cx).0 != edge {
					return;
				}
				let x = f32::from(e.event.position.x);
				let width = f32::from(window.viewport_size().width);
				this.drag_edge(edge, x, width, cx);
			}))
			// The line lights up under the pointer, like Spotify's.
			.child(div().w(px(1.)).h_full().group_hover(id, |s| s.bg(theme::alpha(theme::text(), 0.3))))
	}

	/// Sets a panel's width from the pointer's `x` in a window `width` wide.
	fn drag_edge(&mut self, edge: Edge, x: f32, width: f32, cx: &mut Context<'_, Self>) {
		let gap = f32::from(theme::GAP);
		match edge {
			Edge::Library => {
				let wanted = x - gap;
				if wanted < LIBRARY_MIN * 0.6 {
					// Far enough left: fold to the rail.
					if !self.sidebar_folded() {
						self.send(turntable_core::Msg::ToggleSidebar, cx);
					}
					return;
				}
				if self.st().sidebar_collapsed() {
					self.send(turntable_core::Msg::ToggleSidebar, cx);
				}
				self.library_w = px(wanted.clamp(LIBRARY_MIN, LIBRARY_MAX));
			},
			Edge::Panel => {
				let wanted = width - x - gap;
				self.panel_w = px(wanted.clamp(PANEL_MIN, PANEL_MAX));
			},
		}
		cx.notify();
	}
}
