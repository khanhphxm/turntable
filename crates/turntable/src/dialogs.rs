//! Dialogs over the window: About, pick a playlist to add songs to, and edit a playlist's
//! details. The core owns what's open (`State::dialog`) and the edited text; these views draw it
//! and feed edits back. About is the shell's own: it needs nothing from the core.

use gpui_kit::{
	AnyElement, App, AppContext as _, ClickEvent, ClipboardItem, Context, Div, Entity, FocusHandle, FontWeight, InteractiveElement as _,
	IntoElement, ParentElement as _, Stateful, StatefulInteractiveElement as _, Styled as _, Window,
	component::input::{Input, InputEvent, InputState, Textarea, TextareaState},
	div, img,
	prelude::FluentBuilder as _,
	px,
};
use turntable_core::{Dialog, Field, Msg, Route, library::Tile};

use crate::{
	menus::NAME,
	shell::Shell,
	theme,
	widgets::{Icon, icon, icon_button},
};

/// The text fields of an open "Edit details" dialog.
pub struct DetailsInputs {
	name:        Entity<InputState>,
	description: Entity<TextareaState>,
}

/// The About dialog while it's open.
pub struct About {
	/// What had focus before it opened (it takes focus so Escape reaches it).
	restore: Option<FocusHandle>,
	/// The version was copied: the copy button shows a check until it closes.
	copied:  bool,
}

/// The OS this build runs on and its processor family, as About names them.
const PLATFORM: (&str, &str) = if cfg!(target_os = "macos") {
	("macOS", if cfg!(target_arch = "aarch64") { "Apple Silicon" } else { "Intel" })
} else if cfg!(target_os = "windows") {
	("Windows", if cfg!(target_arch = "aarch64") { "ARM64" } else { "x64" })
} else {
	("Linux", std::env::consts::ARCH)
};

/// The dim layer under a dialog, centring it; a click on the layer runs `close`.
fn scrim(card: impl IntoElement, close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> AnyElement {
	div()
		.absolute()
		.inset_0()
		.flex()
		.items_center()
		.justify_center()
		.bg(theme::alpha(theme::base(), 0.7))
		.child(div().id("scrim").absolute().inset_0().on_click(close))
		.child(card)
		.into_any_element()
}

/// A dialog's card: title, close button, body.
fn card(title: &'static str, body: impl IntoElement, cx: &mut Context<'_, Shell>) -> Stateful<Div> {
	div()
		.id("dialog")
		// Its own hitbox: clicks inside must not reach the scrim beneath (which closes it).
		.occlude()
		.relative()
		.w(px(524.))
		.max_h(px(560.))
		.rounded(px(8.))
		.bg(theme::raised())
		.shadow_2xl()
		.p_6()
		.flex()
		.flex_col()
		.gap_4()
		.child(
			div().flex().items_center().justify_between().child(div().text_size(px(24.)).font_weight(FontWeight::BOLD).child(title)).child(
				icon_button("dialog-close", Icon::Close, px(16.), false, cx.listener(|this, _, _, cx| this.send(Msg::CloseDialog, cx))),
			),
		)
		.child(body)
}

impl Shell {
	/// Opens About over the window; Escape, its X or a click outside closes it.
	pub fn open_about(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
		if self.about.is_none() {
			self.about = Some(About { restore: window.focused(cx), copied: false });
		}
		window.focus(&self.focus, cx);
		cx.notify();
	}

	pub fn close_about(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
		let Some(about) = self.about.take() else { return };
		if let Some(focus) = about.restore {
			window.focus(&focus, cx);
		}
		cx.notify();
	}

	/// Spotify's About card: the wordmark, this build and its version (copyable for bug reports),
	/// and the legal notes in an inset.
	pub fn about_dialog(&self, cx: &mut Context<'_, Self>) -> Option<AnyElement> {
		let copied = self.about.as_ref()?.copied;
		let (os, arch) = PLATFORM;
		let version = env!("CARGO_PKG_VERSION");
		let copy = cx.listener(move |this, _, _, cx| {
			cx.write_to_clipboard(ClipboardItem::new_string(format!("{NAME} {version} ({os} {arch})")));
			if let Some(about) = &mut this.about {
				about.copied = true;
			}
			cx.notify();
		});
		let card = div()
			.id("about")
			// Its own hitbox: clicks inside must not reach the scrim beneath (which closes it).
			.occlude()
			.relative()
			.w(px(448.))
			.rounded(px(12.))
			.bg(theme::raised())
			.shadow_2xl()
			.p_6()
			.flex()
			.flex_col()
			.gap_5()
			.child(
				div()
					.flex()
					.items_center()
					.justify_between()
					.child(
						div()
							.flex()
							.items_center()
							.gap_2()
							.child(img("icons/app.svg").size(px(32.)))
							.child(div().text_size(px(22.)).font_weight(FontWeight::BOLD).child(NAME)),
					)
					.child(icon_button(
						"about-close",
						Icon::Close,
						px(16.),
						false,
						cx.listener(|this, _, window, cx| this.close_about(window, cx)),
					)),
			)
			.child(
				div()
					.flex()
					.flex_col()
					.child(div().text_size(px(16.)).font_weight(FontWeight::BOLD).child(format!("{NAME} for {os} ({arch})")))
					.child(
						div()
							.flex()
							.items_center()
							.gap_1()
							.child(div().text_size(px(14.)).text_color(theme::subdued()).child(format!("Version {version}")))
							.child(icon_button("about-copy", if copied { Icon::Check } else { Icon::Copy }, px(14.), false, copy)),
					),
			)
			.child(
				div()
					.rounded(px(8.))
					.bg(theme::panel())
					.px_4()
					.py_3()
					.flex()
					.flex_col()
					.gap_1()
					.text_size(px(12.))
					.text_color(theme::subdued())
					.child("Not affiliated with Spotify. Spotify is a trademark of Spotify AB.")
					.child("MIT License · Built with GPUI and librespot"),
			);
		Some(scrim(card, cx.listener(|this, _, window, cx| this.close_about(window, cx))))
	}

	/// The open dialog, if any, over everything else.
	pub fn dialog(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> Option<AnyElement> {
		match self.st().dialog.clone() {
			None => {
				self.details = None;
				self.renaming_folder = None;
				None
			},
			Some(Dialog::AddToPlaylist { label, .. }) => Some(self.add_to_playlist_dialog(label, cx)),
			Some(Dialog::Rename { .. }) => Some(self.details_dialog(window, cx)),
			// Drawn in place in Your Library; here the field is filled and focused once.
			Some(Dialog::RenameFolder { id, name }) => {
				if self.renaming_folder.as_ref() != Some(&id) {
					self.renaming_folder = Some(id);
					self.folder_name.update(cx, |s, cx| {
						s.set_value(name, window, cx);
						s.focus(window, cx);
						s.select_all(window, cx);
					});
				}
				None
			},
		}
	}

	fn add_to_playlist_dialog(&mut self, label: String, cx: &mut Context<'_, Self>) -> AnyElement {
		let st = self.st();
		let me = st.library.me.as_ref().map(|m| m.id.clone()).unwrap_or_default();
		// Only playlists the user may add to: their own and collaborative ones.
		let mine: Vec<Tile> = st.library.playlists.iter().filter(|p| p.owner.id == me || p.collaborative).map(Tile::playlist).collect();
		let rows: Vec<AnyElement> = mine
			.iter()
			.enumerate()
			.map(|(i, t)| {
				let id = match &t.route {
					Route::Playlist(id) => id.clone(),
					_ => String::new(),
				};
				div()
					.id(("pick-playlist", i))
					.h(px(56.))
					.px_2()
					.flex()
					.items_center()
					.gap_3()
					.rounded(px(4.))
					.hover(|s| s.bg(theme::hover()))
					.cursor_pointer()
					.on_click(cx.listener(move |this, _, _, cx| this.send(Msg::AddToPlaylist(Some(id.clone())), cx)))
					.child(self.art(&t.route, t.image.as_deref(), px(40.), cx))
					.child(div().text_size(px(16.)).truncate().child(t.title.clone()))
					.into_any_element()
			})
			.collect();
		let new = div()
			.id("pick-new")
			.h(px(56.))
			.px_2()
			.flex()
			.items_center()
			.gap_3()
			.rounded(px(4.))
			.hover(|s| s.bg(theme::hover()))
			.cursor_pointer()
			.on_click(cx.listener(|this, _, _, cx| this.send(Msg::AddToPlaylist(None), cx)))
			.child(div().size(px(40.)).rounded(px(4.)).bg(theme::raised_hover()).flex().items_center().justify_center().child(icon(
				Icon::Plus,
				px(20.),
				theme::text(),
			)))
			.child(div().text_size(px(16.)).child("New playlist"));
		let body = div()
			.flex()
			.flex_col()
			.gap_2()
			.child(div().text_size(px(14.)).text_color(theme::subdued()).truncate().child(label))
			.child(div().id("pick-list").max_h(px(400.)).overflow_y_scroll().flex().flex_col().child(new).children(rows));
		scrim(card("Add to playlist", body, cx), cx.listener(|this, _, _, cx| this.send(Msg::CloseDialog, cx)))
	}

	fn details_dialog(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> AnyElement {
		if self.details.is_none() {
			self.details = Some(self.details_inputs(window, cx));
		}
		let Some(inputs) = self.details.as_ref() else { return div().into_any_element() };
		let (name, description) = (inputs.name.clone(), inputs.description.clone());
		let cover = self.st().library.listing.as_ref().and_then(|l| l.cover_url.clone());
		let route = self.st().library.route.clone();
		let can_save = !self.st().name_input.trim().is_empty();
		let focused = self.st().focus;
		// Spotify's fields: a grey fill, a label riding on the top edge while focused.
		let field = |label: &'static str, on: bool, input: AnyElement| {
			div()
				.relative()
				.rounded(px(4.))
				.bg(theme::raised_hover())
				.border_1()
				.border_color(if on { theme::subdued().into() } else { gpui_kit::transparent_black() })
				.px_3()
				.py_2()
				.text_size(px(14.))
				.child(input)
				.when(on, |s| {
					s.child(
						div()
							.absolute()
							.top(px(-8.))
							.left(px(10.))
							.text_size(px(11.))
							.font_weight(FontWeight::BOLD)
							.text_color(theme::text())
							.child(label),
					)
				})
		};
		let body = div()
			.flex()
			.flex_col()
			.gap_4()
			.child(
				div().h(px(180.)).flex().gap_4().child(self.art(&route, cover.as_deref(), px(180.), cx).shadow_lg()).child(
					div()
						.flex_1()
						.flex()
						.flex_col()
						.gap_4()
						.child(field("Name", focused == Some(Field::PlaylistName), Input::new(&name).appearance(false).into_any_element()))
						.child(
							field(
								"Description",
								focused == Some(Field::PlaylistDescription),
								Textarea::new(&description).appearance(false).h_full().into_any_element(),
							)
							.flex_1(),
						),
				),
			)
			.child(
				div().flex().justify_end().child(
					div()
						.id("details-save")
						.h(px(48.))
						.px_8()
						.flex()
						.items_center()
						.rounded_full()
						.bg(theme::text())
						.text_color(theme::base())
						.text_size(px(16.))
						.font_weight(FontWeight::BOLD)
						.when(can_save, |s| {
							s.cursor_pointer()
								.hover(|s| s.bg(theme::alpha(theme::text(), 0.9)))
								.on_click(cx.listener(|this, _, _, cx| this.send(Msg::SaveRename, cx)))
						})
						.when(!can_save, |s| s.opacity(0.5))
						.child("Save"),
				),
			);
		scrim(card("Edit details", body, cx), cx.listener(|this, _, _, cx| this.send(Msg::CloseDialog, cx)))
	}

	/// The name and description fields, filled from the core and feeding it every edit.
	fn details_inputs(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> DetailsInputs {
		let st = self.st();
		let (name_text, desc_text) = (st.name_input.clone(), st.description_input.clone());
		let name = cx.new(|cx| InputState::new(window, cx).placeholder("Add a name").default_value(name_text));
		let description = cx.new(|cx| TextareaState::new(window, cx).placeholder("Add an optional description").default_value(desc_text));
		cx.subscribe(&name, |this, input, e: &InputEvent, cx| match e {
			InputEvent::Change => {
				let text = input.read(cx).value().to_string();
				this.send(Msg::SetText(Field::PlaylistName, text), cx);
			},
			InputEvent::PressEnter { .. } => this.send(Msg::SaveRename, cx),
			// The field's label shows while it has focus.
			InputEvent::Focus => this.send(Msg::Focus(Some(Field::PlaylistName)), cx),
			InputEvent::Blur => {},
		})
		.detach();
		cx.subscribe(&description, |this, input, e: &InputEvent, cx| match e {
			InputEvent::Change => {
				let text = input.read(cx).value().to_string();
				this.send(Msg::SetText(Field::PlaylistDescription, text), cx);
			},
			InputEvent::Focus => this.send(Msg::Focus(Some(Field::PlaylistDescription)), cx),
			InputEvent::Blur | InputEvent::PressEnter { .. } => {},
		})
		.detach();
		name.update(cx, |s, cx| s.focus(window, cx));
		DetailsInputs { name, description }
	}
}
