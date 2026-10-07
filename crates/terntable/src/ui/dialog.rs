//! Dialogs over the client: pick a playlist to add songs to, edit a playlist's details.

use tern_sdk::ui::{self, Node, html};

use super::{Tap, art, div, icon, text};
use crate::{
	app::{Dialog, Field, Msg, State},
	spotify,
	ui::icons::{Glyph, Icons},
};

pub fn dialog(state: &State, icons: &Icons, dialog: &Dialog) -> Node<Msg> {
	let overlay = ui::overlay().key("dialog").role("tt.dialog").modal(true).size(ui::OverlaySize::Sm);
	match dialog {
		Dialog::AddToPlaylist { .. } => overlay
			.head("Save in")
			.child(add_to_playlist(state, icons))
			.child(
				div("tt-dialog-foot")
					.child(html::el("button").class("tt-link tt-dim tt-dialog-cancel").text("Cancel").on_click(Msg::CloseDialog)),
			)
			.into(),
		Dialog::Rename { .. } => overlay
			.role("tt.edit")
			.child(div("tt-edit-head").child(text("tt-edit-title", "Edit details")).child(super::button(
				icons,
				Glyph::Close,
				"Close",
				Msg::CloseDialog,
				"tt-edit-close",
				20.0,
			)))
			.child(edit_details(state, icons))
			.child(
				div("tt-edit-foot")
					.child(
						html::el("button")
							.class("tt-pill tt-quiet tt-edit-public")
							.child(icon(
								icons,
								if state.public_input == Some(true) { Glyph::Lock } else { Glyph::Globe },
								16.0,
								Tap::Msg(Msg::TogglePlaylistPublic, None),
							))
							.child(html::span(if state.public_input == Some(true) { "Make private" } else { "Make public" }))
							.on_click(Msg::TogglePlaylistPublic),
					)
					.child(html::el("button").class("tt-save").text("Save").on_click(Msg::SaveRename)),
			)
			.into(),
	}
}

/// Spotify's "Save in": Liked Songs (for the playing song, where the picker opens from
/// its ✓), the user's own playlists (the ones Spotify lets them add to), and a new one.
fn add_to_playlist(state: &State, icons: &Icons) -> Node<Msg> {
	let me = state.library.me.as_ref().map(|u| u.id.as_str());
	let new = Msg::AddToPlaylist(None);
	let mut list = div("tt-pick").key("pick");
	if let (Some(Dialog::AddToPlaylist { uris, .. }), Some(liked)) = (&state.dialog, state.liked())
		&& matches!(uris.as_slice(), [only] if *only == state.item.uri)
	{
		let (glyph, label) = if liked { (Glyph::Saved, "Remove from Liked Songs") } else { (Glyph::Save, "Save to Liked Songs") };
		list = list.child(
			html::div()
				.class("tt-pick-row")
				.key("liked")
				.title(label)
				.child(div("tt-art tt-art-40 tt-art-liked tt-art-fallback").child(icon(
					icons,
					Glyph::Heart,
					18.0,
					Tap::Msg(Msg::ToggleLike, None),
				)))
				.child(text("tt-trunc", "Liked Songs"))
				.child(div("tt-pick-state").child(icon(icons, glyph, 20.0, Tap::Msg(Msg::ToggleLike, None))))
				.on_click(Msg::ToggleLike),
		);
	}
	list = list.child(
		html::div()
			.class("tt-pick-row")
			.key("new")
			.child(div("tt-art tt-art-40 tt-art-fallback").child(icon(icons, Glyph::Plus, 18.0, Tap::Msg(new.clone(), None))))
			.child(html::span("New playlist"))
			.on_click(new),
	);
	for p in state.library.playlists.iter().filter(|p| Some(p.owner.id.as_str()) == me || p.collaborative) {
		let pick = Msg::AddToPlaylist(Some(p.id.clone()));
		let url = spotify::pick_image(&p.images, 64).map(|i| i.url.as_str());
		list = list.child(
			html::div()
				.class("tt-pick-row")
				.key(p.id.clone())
				.child(art(state, url, "tt-art-40", &p.name, Tap::Msg(pick.clone(), None)))
				.child(text("tt-trunc", p.name.clone()))
				.on_click(pick),
		);
	}
	list.into()
}

/// Spotify's "Edit details": artwork stays read-only until Tern offers a file picker.
fn edit_details(state: &State, icons: &Icons) -> Node<Msg> {
	let input = ui::input()
		.key(Field::PlaylistName.id())
		.text(state.name_input.clone())
		.placeholder("Name")
		.on_focus(Msg::Focus(Some(Field::PlaylistName)))
		.on_edit(|e| Msg::Edit { from: e.from, to: e.to, text: e.text.clone() });
	let description = ui::editor()
		.key(Field::PlaylistDescription.id())
		.text(state.description_input.clone())
		.placeholder("Add an optional description")
		.on_focus(Msg::Focus(Some(Field::PlaylistDescription)))
		.on_edit(|e| Msg::Edit { from: e.from, to: e.to, text: e.text.clone() });
	let listing = state.library.listing.as_ref();
	let cover = match listing.and_then(|l| l.cover_url.as_deref()) {
		Some(url) => art(state, Some(url), "tt-art-180", "Playlist cover", Tap::NONE),
		None => div("tt-art tt-art-180 tt-art-fallback").child(icon(icons, Glyph::Library, 48.0, Tap::NONE)).into(),
	};
	div("tt-edit")
		.key("edit")
		.child(cover)
		.child(
			div("tt-edit-fields")
				.key("fields")
				.child(div("tt-field").key("field").child(input))
				.child(div("tt-edit-description").key("description").child(description)),
		)
		.into()
}
