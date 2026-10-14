//! Cache boundaries around Shell's existing view builders. Shell still owns the controller and
//! handlers; each region owns its GPUI render identity, layout/paint cache and local animations.

use gpui_kit::{AppContext as _, Context, Entity, IntoElement, Render, Subscription, WeakEntity, Window};

use crate::{
	covers::{CoverCache, LIBRARY_ROWS, LISTING_ROWS},
	shell::{FullView, Shell},
};

#[derive(Clone, Copy)]
pub(crate) enum Kind {
	Top,
	Library,
	Page,
	Panel,
	NowPlaying,
	Transport,
	Extras,
}

impl Kind {
	pub fn artwork_scope(self) -> u16 {
		1 << self as u8
	}
}

pub(crate) struct Region {
	shell:       WeakEntity<Shell>,
	kind:        Kind,
	_watch:      Subscription,
	#[cfg(test)]
	pub renders: usize,
}

impl Region {
	fn new(shell: &Entity<Shell>, kind: Kind, cx: &mut Context<'_, Self>) -> Self {
		// Every Shell notification (core updates, navigation, inputs, artwork completion,
		// menus, resize, etc.) invalidates all regions. Keep this conservative contract:
		// missing a dependency would freeze a cached subtree. GPUI's local hover, scroll,
		// animation and child notifications instead invalidate only their ancestor region.
		let watch = cx.observe(shell, |_, _, cx| cx.notify());
		Self {
			shell: shell.downgrade(),
			kind,
			_watch: watch,
			#[cfg(test)]
			renders: 0,
		}
	}
}

impl Render for Region {
	fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
		#[cfg(test)]
		{
			self.renders += 1;
		}
		self.shell
			.update(cx, |shell, cx| {
				// List processors run later during layout, outside this render call.
				match self.kind {
					Kind::Library => CoverCache::reset(LIBRARY_ROWS, cx),
					Kind::Page => CoverCache::reset(LISTING_ROWS, cx),
					_ => {},
				}
				shell.with_artwork(self.kind.artwork_scope(), true, cx, |shell, cx| match self.kind {
					Kind::Top => shell.top_bar(window, cx).into_any_element(),
					Kind::Library => shell.sidebar(cx).into_any_element(),
					Kind::Page if shell.full_view != FullView::Off => shell.full_view_page(cx),
					Kind::Page => shell.page_content(cx),
					Kind::Panel => match shell.side() {
						Some(side) => shell.right_panel(side, cx).into_any_element(),
						None => shell.folded_panel(cx).into_any_element(),
					},
					Kind::NowPlaying => shell.bar_now(cx).into_any_element(),
					Kind::Transport => shell.bar_buttons(cx).into_any_element(),
					Kind::Extras => shell.bar_extras(cx).into_any_element(),
				})
			})
			// The weak owner may be gone while the window is being torn down.
			.unwrap_or_else(|_| gpui_kit::Empty.into_any_element())
	}
}

pub(crate) struct Regions {
	pub top:         Entity<Region>,
	pub library:     Entity<Region>,
	pub page:        Entity<Region>,
	pub panel:       Entity<Region>,
	pub now_playing: Entity<Region>,
	pub transport:   Entity<Region>,
	pub extras:      Entity<Region>,
}

impl Regions {
	pub fn new(cx: &mut Context<'_, Shell>) -> Self {
		let shell = cx.entity();
		Self {
			top:         cx.new(|cx| Region::new(&shell, Kind::Top, cx)),
			library:     cx.new(|cx| Region::new(&shell, Kind::Library, cx)),
			page:        cx.new(|cx| Region::new(&shell, Kind::Page, cx)),
			panel:       cx.new(|cx| Region::new(&shell, Kind::Panel, cx)),
			now_playing: cx.new(|cx| Region::new(&shell, Kind::NowPlaying, cx)),
			transport:   cx.new(|cx| Region::new(&shell, Kind::Transport, cx)),
			extras:      cx.new(|cx| Region::new(&shell, Kind::Extras, cx)),
		}
	}
}
