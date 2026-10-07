//! Dialogs over the window: pick a playlist to add songs to, and edit a playlist's details.
//! The core owns what's open (`State::dialog`) and the edited text; these views draw it and
//! feed edits back.

use gpui_kit::{
	AnyElement, AppContext as _, Context, Div, Entity, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
	StatefulInteractiveElement as _, Styled as _, Window,
	component::input::{Input, InputEvent, InputState, Textarea, TextareaState},
	div,
	prelude::FluentBuilder as _,
	px,
};
use turntable_core::{Dialog, Field, Msg, Route, library::Tile};

use crate::{
	shell::Shell,
	theme,
	widgets::{Icon, icon, icon_button},
};

/// The text fields of an open "Edit details" dialog.
pub struct DetailsInputs {
	name:        Entity<InputState>,
	description: Entity<TextareaState>,
}

/// UTF-16 length, the unit the core's edits count in.
fn utf16_len(s: &str) -> usize {
	s.encode_utf16().count()
}

/// The dim layer under a dialog, centring it; a click on the layer closes the dialog.
fn scrim(card: impl IntoElement, cx: &mut Context<'_, Shell>) -> AnyElement {
	div()
		.absolute()
		.inset_0()
		.flex()
		.items_center()
		.justify_center()
		.bg(theme::alpha(theme::base(), 0.7))
		.child(
			div()
				.id("scrim")
				.absolute()
				.inset_0()
				.on_click(cx.listener(|this, _, _, cx| this.send(Msg::CloseDialog, cx))),
		)
		.child(card)
		.into_any_element()
}

/// A dialog's card: title, close button, body.
fn card(title: &'static str, body: impl IntoElement, cx: &mut Context<'_, Shell>) -> Div {
	div()
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
			div()
				.flex()
				.items_center()
				.justify_between()
				.child(div().text_size(px(24.)).font_weight(FontWeight::BOLD).child(title))
				.child(icon_button("dialog-close", Icon::Close, px(16.), false, cx.listener(|this, _, _, cx| this.send(Msg::CloseDialog, cx)))),
		)
		.child(body)
}

impl Shell {
	/// The open dialog, if any, over everything else.
	pub fn dialog(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> Option<AnyElement> {
		match self.st().dialog.clone() {
			None => {
				self.details = None;
				None
			},
			Some(Dialog::AddToPlaylist { label, .. }) => Some(self.add_to_playlist_dialog(label, cx)),
			Some(Dialog::Rename { .. }) => Some(self.details_dialog(window, cx)),
		}
	}

	fn add_to_playlist_dialog(&mut self, label: String, cx: &mut Context<'_, Self>) -> AnyElement {
		let st = self.st();
		let me = st.library.me.as_ref().map(|m| m.id.clone()).unwrap_or_default();
		// Only playlists the user may add to: their own and collaborative ones.
		let mine: Vec<Tile> =
			st.library.playlists.iter().filter(|p| p.owner.id == me || p.collaborative).map(Tile::playlist).collect();
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
			.child(div().size(px(40.)).rounded(px(4.)).bg(theme::raised_hover()).flex().items_center().justify_center().child(icon(Icon::Plus, px(20.), theme::text())))
			.child(div().text_size(px(16.)).child("New playlist"));
		let body = div()
			.flex()
			.flex_col()
			.gap_2()
			.child(div().text_size(px(14.)).text_color(theme::subdued()).truncate().child(label))
			.child(div().id("pick-list").max_h(px(400.)).overflow_y_scroll().flex().flex_col().child(new).children(rows));
		scrim(card("Add to playlist", body, cx), cx)
	}

	fn details_dialog(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> AnyElement {
		if self.details.is_none() {
			self.details = Some(self.details_inputs(window, cx));
		}
		let Some(inputs) = self.details.as_ref() else { return div().into_any_element() };
		let (name, description) = (inputs.name.clone(), inputs.description.clone());
		let public = self.st().public_input;
		let cover = self.st().library.listing.as_ref().and_then(|l| l.cover_url.clone());
		let route = self.st().library.route.clone();
		let can_save = !self.st().name_input.trim().is_empty();
		let body = div()
			.flex()
			.flex_col()
			.gap_4()
			.child(
				div()
					.flex()
					.gap_4()
					.child(self.art(&route, cover.as_deref(), px(180.), cx).shadow_lg())
					.child(
						div()
							.flex_1()
							.flex()
							.flex_col()
							.gap_3()
							.child(Input::new(&name))
							.child(div().flex_1().child(Textarea::new(&description).h_full())),
					),
			)
			.child(
				div()
					.flex()
					.items_center()
					.justify_between()
					.child(
						div()
							.id("details-public")
							.flex()
							.items_center()
							.gap_2()
							.text_size(px(14.))
							.text_color(theme::subdued())
							.hover(|s| s.text_color(theme::text()))
							.cursor_pointer()
							.on_click(cx.listener(|this, _, _, cx| this.send(Msg::TogglePlaylistPublic, cx)))
							.child(icon(if public == Some(true) { Icon::Globe } else { Icon::Lock }, px(16.), theme::subdued()))
							.child(if public == Some(true) { "Public: anyone can find it" } else { "Private: only you can see it" }),
					)
					.child(
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
							.when(can_save, |s| s.cursor_pointer().hover(|s| s.bg(theme::alpha(theme::text(), 0.9))).on_click(cx.listener(|this, _, _, cx| this.send(Msg::SaveRename, cx))))
							.when(!can_save, |s| s.opacity(0.5))
							.child("Save"),
					),
			);
		scrim(card("Edit details", body, cx), cx)
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
				this.send(Msg::Focus(Some(Field::PlaylistName)), cx);
				let old = utf16_len(&this.st().name_input);
				this.send(Msg::Edit { from: 0, to: old, text }, cx);
			},
			InputEvent::PressEnter { .. } => this.send(Msg::SaveRename, cx),
			_ => {},
		})
		.detach();
		cx.subscribe(&description, |this, input, e: &InputEvent, cx| {
			if matches!(e, InputEvent::Change) {
				let text = input.read(cx).value().to_string();
				this.send(Msg::Focus(Some(Field::PlaylistDescription)), cx);
				let old = utf16_len(&this.st().description_input);
				this.send(Msg::Edit { from: 0, to: old, text }, cx);
			}
		})
		.detach();
		name.update(cx, |s, cx| s.focus(window, cx));
		DetailsInputs { name, description }
	}
}
