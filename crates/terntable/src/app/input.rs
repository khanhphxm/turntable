//! Text-field state, search debouncing and Tern's UTF-16 edits.

use super::{Effect, Field, Msg, State};

impl State {
	/// After the text of `field` changed: searching follows typing.
	pub(super) fn after_edit(&mut self, field: Field, effects: &mut Vec<Effect>) {
		if field == Field::Search {
			if self.library.query.trim().is_empty() {
				self.library.search = None;
			} else {
				effects.push(Effect::SearchSoon);
			}
		}
	}
	pub(super) fn field_text(&mut self, field: Field) -> &mut String {
		match field {
			Field::Search => &mut self.library.query,
			Field::ClientId => &mut self.id_input,
			Field::PlaylistName => &mut self.name_input,
			Field::PlaylistDescription => &mut self.description_input,
			Field::LibraryFilter => &mut self.library.filter,
			Field::DeviceName => &mut self.device_input,
		}
	}

	pub(super) fn type_input(&mut self, mut text: String, effects: &mut Vec<Effect>) {
		if let Some(field) = self.focus {
			if field != Field::PlaylistDescription {
				text.retain(|c| c != '\n' && c != '\r');
			}
			self.field_text(field).push_str(&text);
			self.after_edit(field, effects);
		}
	}

	pub(super) fn backspace_input(&mut self, effects: &mut Vec<Effect>) {
		if let Some(field) = self.focus {
			self.field_text(field).pop();
			self.after_edit(field, effects);
		}
	}

	pub(super) fn edit_input(&mut self, from: usize, to: usize, mut text: String, effects: &mut Vec<Effect>) {
		if let Some(field) = self.focus {
			if field != Field::PlaylistDescription {
				text.retain(|c| c != '\n' && c != '\r');
			}
			replace_utf16(self.field_text(field), from, to, &text);
			self.after_edit(field, effects);
		}
	}

	pub(super) fn submit_input(&mut self) -> Vec<Effect> {
		let Some(field) = self.focus else { return Vec::new() };
		let msg = match field {
			Field::Search => Msg::SearchSubmit(self.field_text(field).clone()),
			Field::ClientId => Msg::SaveClientId(self.field_text(field).clone()),
			Field::PlaylistName => Msg::SaveRename,
			Field::PlaylistDescription => Msg::Type("\n".into()),
			Field::LibraryFilter => match self.library.entries(true).first() {
				Some(entry) => Msg::Navigate(entry.route()),
				None => Msg::Focus(None),
			},
			Field::DeviceName => Msg::SaveDeviceName,
		};
		self.update(msg)
	}
}
/// Replaces the UTF-16 range `from..to` of `s` with `text` (Tern's edit events count UTF-16 units).
pub(super) fn replace_utf16(s: &mut String, from: usize, to: usize, text: &str) {
	let byte_at = |units: usize| {
		let mut seen = 0;
		for (i, ch) in s.char_indices() {
			if seen >= units {
				return i;
			}
			seen += ch.len_utf16();
		}
		s.len()
	};
	let (a, b) = (byte_at(from.min(to)), byte_at(to.max(from)));
	s.replace_range(a..b, text);
}
