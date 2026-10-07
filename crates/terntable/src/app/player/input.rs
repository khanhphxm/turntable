//! Keyboard routing and remote actions sent by the Tern plugin.

use tern_sdk::Key;

use super::Player;
use crate::{
	app::{Field, Layout, Msg, Panel},
	library::Route,
};

impl Player<'_> {
	pub(super) fn key_msg(&self, key: &Key) -> Option<Msg> {
		if self.state.dialog.is_some() && key.is("escape") {
			return Some(Msg::CloseDialog);
		}
		if self.state.library_menu && key.is("escape") {
			return Some(Msg::LibraryMenu);
		}
		if let Some(menu) = self.state.sys.menu.filter(|_| key.is("escape")) {
			return Some(Msg::Dropdown(menu));
		}
		if self.state.devices_menu_open() && key.is("escape") {
			return Some(Msg::DevicesMenu);
		}
		if self.state.eq_menu_open() && key.is("escape") {
			return Some(Msg::EqMenu);
		}
		if self.state.focus.is_some_and(|f| self.state.field_visible(f)) {
			if key.is("escape") {
				return Some(Msg::Focus(None));
			}
			if key.is("enter") {
				return Some(Msg::Submit);
			}
			if key.is("backspace") {
				return Some(Msg::Backspace);
			}
			let typing = key.name == "paste" || (!key.ctrl && !key.meta && !key.alt);
			if typing && let Some(text) = key.typed().or(key.text.as_deref()).filter(|t| !t.is_empty()) {
				return Some(Msg::Type(text.to_owned()));
			}
			return None;
		}
		let full = self.state.layout() != Layout::Strip;
		let pairs: &[(&str, Msg)] = &[
			("space", Msg::TogglePlay),
			("right", Msg::SeekBy(10)),
			("left", Msg::SeekBy(-10)),
			("shift+right", Msg::Next),
			("shift+left", Msg::Prev),
			("n", Msg::Next),
			("p", Msg::Prev),
			("+", Msg::VolumeBy(1)),
			("=", Msg::VolumeBy(1)),
			("-", Msg::VolumeBy(-1)),
			("0", Msg::ToggleMute),
			("s", Msg::ToggleShuffle),
			("r", Msg::CycleRepeat),
			("q", Msg::TogglePanel(Panel::Queue)),
			("y", Msg::TogglePanel(Panel::Lyrics)),
			("e", Msg::TogglePanel(Panel::Eq)),
			(",", Msg::TogglePanel(Panel::Settings)),
			("ctrl+c", Msg::Quit),
		];
		if let Some((_, msg)) = pairs.iter().find(|(chord, _)| key.is(chord)) {
			return Some(msg.clone());
		}
		if !full {
			return None;
		}
		let nav: &[(&str, Msg)] = &[
			("down", Msg::MoveSelection(1)),
			("j", Msg::MoveSelection(1)),
			("up", Msg::MoveSelection(-1)),
			("k", Msg::MoveSelection(-1)),
			("page_down", Msg::MoveSelection(10)),
			("page_up", Msg::MoveSelection(-10)),
			("enter", Msg::PlaySelected),
			("/", Msg::Navigate(Route::Search)),
			("g", Msg::Navigate(Route::Home)),
			("l", Msg::Navigate(Route::Liked)),
		];
		let msg = nav.iter().find(|(chord, _)| key.is(chord)).map(|(_, m)| m.clone())?;
		if msg == Msg::Navigate(Route::Search) && self.state.library.route == Route::Search {
			// `/` on the search page jumps into the field, like the Spotify app.
			return Some(Msg::Focus(Some(Field::Search)));
		}
		Some(msg)
	}
}
/// The actions the Tern plugin sends into this surface (`cx.session:event`).
pub(super) fn remote_msg(act: &str) -> Option<Msg> {
	Some(match act {
		"tt.toggle" => Msg::TogglePlay,
		"tt.next" => Msg::Next,
		"tt.prev" => Msg::Prev,
		"tt.volume_up" => Msg::VolumeBy(1),
		"tt.volume_down" => Msg::VolumeBy(-1),
		"tt.mute" => Msg::ToggleMute,
		"tt.shuffle" => Msg::ToggleShuffle,
		"tt.repeat" => Msg::CycleRepeat,
		"tt.queue" => Msg::TogglePanel(Panel::Queue),
		"tt.lyrics" => Msg::TogglePanel(Panel::Lyrics),
		"tt.eq" => Msg::TogglePanel(Panel::Eq),
		"tt.settings" => Msg::TogglePanel(Panel::Settings),
		"tt.search" => Msg::Navigate(Route::Search),
		_ => return None,
	})
}
