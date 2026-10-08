//! Native, occurrence-based queue management through the in-process Connect player.
//!
//! `QueueRemove`, `QueueJump` and `QueueMoveTo` index all of [`State::upcoming`], including
//! context/autoplay occurrences. A move's destination is its final index after removal;
//! occurrence UIDs and source metadata stay intact. `QueueClear` clears only the user queue. `QueueRow`
//! appends and `QueuePlayNext` inserts at its front.
//! Album/playlist `QueueContext` reads every page through the authenticated native session.
//!
//! Edits mutate `State::upcoming` immediately and carry a monotonically increasing revision.
//! Older snapshots cannot undo newer clicks. Once Spirc acknowledges that revision, its actual
//! queue replaces the optimistic one, including the context's real shuffled playback order.
//! No restart, mute, shuffle reset, context replacement or Web API request is involved.

use super::State;
use crate::engine::{Command, Entity, QueueEdit, QueueEntry};

/// Revision barrier between optimistic edits and authoritative Connect snapshots.
#[derive(Debug, Default)]
pub struct Edit {
	revision: u64,
	pending:  bool,
}

impl Edit {
	pub fn busy(&self) -> bool {
		self.pending
	}

	fn command(&mut self, edit: QueueEdit) -> Command {
		self.revision += 1;
		self.pending = true;
		Command::EditQueue { revision: self.revision, edit }
	}

	/// Accept only snapshots that include every edit already displayed.
	pub fn reconcile(&mut self, revision: u64) -> bool {
		if self.pending && revision < self.revision {
			return false;
		}
		self.pending = false;
		self.revision = self.revision.max(revision);
		true
	}

	pub fn cancel(&mut self) {
		self.pending = false;
	}
}

impl State {
	pub(super) fn queue_remove(&mut self, index: usize, cmds: &mut Vec<Command>) {
		if index < self.upcoming.len() {
			let entry = self.upcoming.remove(index);
			cmds.push(self.queue_edit.command(QueueEdit::Remove { uid: entry.uid }));
		}
	}

	pub(super) fn queue_move(&mut self, from: usize, to: usize, cmds: &mut Vec<Command>) {
		if from == to || from >= self.upcoming.len() || to >= self.upcoming.len() {
			return;
		}
		let entry = self.upcoming.remove(from);
		let before = self.upcoming.get(to).map(|e| e.uid.clone());
		let uid = entry.uid.clone();
		self.upcoming.insert(to, entry);
		cmds.push(self.queue_edit.command(QueueEdit::Move { uid, before }));
	}

	pub(super) fn queue_clear(&mut self, cmds: &mut Vec<Command>) {
		if self.upcoming.iter().any(|e| e.source == "queue") {
			self.upcoming.retain(|e| e.source != "queue");
			cmds.push(self.queue_edit.command(QueueEdit::Clear));
		}
	}

	pub(super) fn queue_jump(&mut self, index: usize, cmds: &mut Vec<Command>) {
		if let Some(entry) = self.upcoming.get(index) {
			let uid = entry.uid.clone();
			self.upcoming.drain(..=index);
			cmds.push(self.queue_edit.command(QueueEdit::Jump { uid }));
		}
	}

	/// Inserts playable URIs atomically, preserving duplicates and the supplied order.
	pub fn queue_tracks(&mut self, uris: Vec<String>, next: bool) -> Vec<Command> {
		let revision = self.queue_edit.revision + 1;
		let batch = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
		let tracks: Vec<_> = uris
			.into_iter()
			.filter(|uri| uri.starts_with("spotify:track:") || uri.starts_with("spotify:episode:"))
			.enumerate()
			.map(|(i, uri)| (format!("turntable-{batch}-{revision}-{i}"), uri))
			.collect();
		if tracks.is_empty() {
			return Vec::new();
		}
		let at = if next { 0 } else { self.upcoming.iter().take_while(|e| e.source == "queue").count() };
		let entries = tracks.iter().map(|(uid, uri)| QueueEntry {
			uid:    uid.clone(),
			source: "queue".into(),
			item:   Entity { uri: uri.clone(), ..Entity::default() },
		});
		self.upcoming.splice(at..at, entries);
		vec![self.queue_edit.command(QueueEdit::Add { tracks, next })]
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{Msg, engine::Event};

	fn entry(uid: &str, source: &str) -> QueueEntry {
		QueueEntry {
			uid:    uid.into(),
			source: source.into(),
			item:   Entity { uri: format!("spotify:track:{uid}"), ..Entity::default() },
		}
	}

	fn state() -> State {
		State { upcoming: vec![entry("a", "queue"), entry("b", "queue"), entry("c", "queue"), entry("d", "context")], ..State::default() }
	}

	fn uids(s: &State) -> Vec<&str> {
		s.upcoming.iter().map(|e| e.uid.as_str()).collect()
	}

	#[test]
	fn remove_move_clear_are_immediate_and_keep_context() {
		let mut s = state();
		s.update(Msg::QueueMoveTo(0, 2));
		assert_eq!(uids(&s), ["b", "c", "a", "d"]);
		s.update(Msg::QueueMoveTo(2, 0));
		assert_eq!(uids(&s), ["a", "b", "c", "d"]);
		assert!(s.update(Msg::QueueMoveTo(3, 3)).is_empty());
		assert!(s.update(Msg::QueueMoveTo(99, 0)).is_empty());
		s.update(Msg::QueueRemove(1));
		assert_eq!(uids(&s), ["a", "c", "d"]);
		s.update(Msg::QueueClear);
		assert_eq!(uids(&s), ["d"]);
	}

	#[test]
	fn context_and_autoplay_rows_move_across_sections_without_losing_their_occurrence() {
		let mut s = state();
		s.upcoming.push(entry("e", "context"));
		s.upcoming.push(entry("auto", "autoplay"));
		let effects = s.update(Msg::QueueMoveTo(4, 3));
		assert!(matches!(effects.as_slice(), [crate::app::Effect::Player(Command::EditQueue {
			edit: QueueEdit::Move { uid, before: Some(before) }, ..
		})] if uid == "e" && before == "d"));
		assert_eq!(uids(&s), ["a", "b", "c", "e", "d", "auto"]);
		let effects = s.update(Msg::QueueMoveTo(5, 0));
		assert!(matches!(effects.as_slice(), [crate::app::Effect::Player(Command::EditQueue {
			edit: QueueEdit::Move { uid, before: Some(before) }, ..
		})] if uid == "auto" && before == "a"));
		assert_eq!(uids(&s), ["auto", "a", "b", "c", "e", "d"]);
		assert_eq!(s.upcoming.first().map(|e| e.source.as_str()), Some("autoplay"));
		let effects = s.update(Msg::QueueMoveTo(1, 5));
		assert!(matches!(effects.as_slice(), [crate::app::Effect::Player(Command::EditQueue {
			edit: QueueEdit::Move { uid, before: None }, ..
		})] if uid == "a"));
		assert_eq!(uids(&s), ["auto", "b", "c", "e", "d", "a"]);
		s.update(Msg::QueueClear);
		assert_eq!(uids(&s), ["auto", "e", "d"]);
	}

	#[test]
	fn batches_append_and_play_next_preserves_duplicates() {
		let mut s = state();
		let uris = vec!["spotify:track:x".into(), "spotify:track:x".into()];
		let commands = s.queue_tracks(uris, false);
		assert!(
			matches!(commands.first(), Some(Command::EditQueue { edit: QueueEdit::Add { next: false, tracks }, .. }) if tracks.len() == 2)
		);
		assert_eq!(s.upcoming.iter().map(|e| e.item.uri.as_str()).collect::<Vec<_>>(), [
			"spotify:track:a",
			"spotify:track:b",
			"spotify:track:c",
			"spotify:track:x",
			"spotify:track:x",
			"spotify:track:d"
		]);
		assert_ne!(s.upcoming.get(3).map(|e| &e.uid), s.upcoming.get(4).map(|e| &e.uid));
		s.queue_tracks(vec!["spotify:track:y".into()], true);
		assert_eq!(s.upcoming.first().map(|e| e.item.uri.as_str()), Some("spotify:track:y"));
		assert_eq!(s.upcoming.last().map(|e| e.source.as_str()), Some("context"));
		assert!(s.queue_tracks(vec!["spotify:local:x".into()], false).is_empty());
	}

	#[test]
	fn row_queue_and_play_next_use_native_commands_and_context_uses_session_worker() {
		let mut s = state();
		s.library.route = crate::Route::Album("album".into());
		s.library.listing = Some(crate::library::Listing {
			tracks: vec![crate::spotify::Track {
				uri:         "spotify:track:row".into(),
				name:        "Row".into(),
				duration_ms: 1_000,
				disc_number: 0,
				explicit:    false,
				artists:     Vec::new(),
				album:       None,
				is_local:    false,
				is_episode:  false,
			}],
			..crate::library::Listing::default()
		});
		let effects = s.update(Msg::QueueRow(s.library.row_target(0).unwrap()));
		assert!(matches!(
			effects.first(),
			Some(crate::app::Effect::Player(Command::EditQueue { edit: QueueEdit::Add { next: false, .. }, .. }))
		));
		let effects = s.update(Msg::QueuePlayNext(s.library.row_target(0).unwrap()));
		assert!(matches!(
			effects.first(),
			Some(crate::app::Effect::Player(Command::EditQueue { edit: QueueEdit::Add { next: true, .. }, .. }))
		));
		assert_eq!(s.upcoming.first().map(|entry| entry.item.uri.as_str()), Some("spotify:track:row"));
		let effects = s.update(Msg::QueueContext(crate::Route::Playlist("playlist".into())));
		assert!(
			matches!(effects.first(), Some(crate::app::Effect::Library(crate::library::Request::QueueContext(crate::Route::Playlist(id)))) if id == "playlist")
		);
	}

	#[test]
	fn stale_snapshots_cannot_undo_edits_then_native_shuffle_reconciles() {
		let mut s = state();
		s.update(Msg::QueueRemove(0));
		s.update(Msg::QueueMoveTo(0, 1));
		s.apply(Event::QueueChanged { upcoming: vec![entry("a", "queue")], revision: 1 });
		assert_eq!(uids(&s), ["c", "b", "d"]);
		let native = vec![entry("c", "queue"), entry("b", "queue"), entry("shuffled", "context")];
		s.apply(Event::QueueChanged { upcoming: native, revision: 2 });
		assert_eq!(uids(&s), ["c", "b", "shuffled"]);
		assert!(!s.queue_edit.busy());
	}
}
