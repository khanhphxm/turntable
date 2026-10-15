//! Foreground navigation is retained before authentication and runs before startup metadata.
//! Requests at the same priority stay ordered, especially playlist writes and page offsets.

use std::collections::VecDeque;

use super::{Inbox, Request};

#[derive(Default)]
pub(super) struct Queue {
	pending: VecDeque<Inbox>,
}

impl Queue {
	pub(super) fn push(&mut self, message: Inbox) {
		if matches!(&message, Inbox::Request(_, Request::ForgetSnapshot) | Inbox::Session(_, None)) {
			self.pending.clear();
		}
		match &message {
			Inbox::Request(_, Request::Search(_)) => self.pending.retain(|known| !matches!(known, Inbox::Request(_, Request::Search(_)))),
			Inbox::Request(_, Request::Open(_, 0) | Request::Home) => {
				self.pending.retain(|known| !matches!(known, Inbox::Request(_, Request::Home | Request::Open(_, 0))));
			},
			Inbox::Request(_, Request::Control { device, command: crate::spotify::PlayerCommand::Volume(_), .. }) => {
				self.pending.retain(|known| {
					!matches!(known,
					Inbox::Request(_, Request::Control { device: known, command: crate::spotify::PlayerCommand::Volume(_), .. }) if known == device)
				});
			},
			_ => {},
		}
		self.pending.push_back(message);
	}

	pub(super) fn pop(&mut self, connected: bool) -> Option<Inbox> {
		let index = self
			.pending
			.iter()
			.enumerate()
			.filter(|(_, request)| connected || priority(request) == 0)
			.min_by_key(|(_, request)| priority(request))
			.map(|(index, _)| index)?;
		self.pending.remove(index)
	}
}

fn priority(message: &Inbox) -> u8 {
	match message {
		Inbox::Session(_, _)
		| Inbox::Request(_, Request::Connect | Request::ForgetSnapshot | Request::PlaybackCheckpoint(_) | Request::RememberPlayback(_)) => 0,
		Inbox::Request(_, request) if foreground(request) => 1,
		Inbox::Request(_, Request::Prefetch(_)) => 3,
		Inbox::Request(_, _) => 2,
	}
}

pub(super) fn foreground(request: &Request) -> bool {
	!matches!(
		request,
		Request::Albums
			| Request::Following
			| Request::Artist(_)
			| Request::ArtistCard(_)
			| Request::Credits(_)
			| Request::CheckLiked(_)
			| Request::Pins
			| Request::FolderExpansion
			| Request::Exclusions
			| Request::Playback
			| Request::LastPlayback
			| Request::RememberPlayback(_)
			| Request::PlaybackCheckpoint(_)
			| Request::Prefetch(_)
	)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::library::Route;

	#[test]
	fn disk_checkpoint_reads_do_not_wait_for_a_library_session() {
		let mut queue = Queue::default();
		queue.push(Inbox::Request(0, Request::PlaybackCheckpoint("alex".into())));
		assert!(matches!(queue.pop(false), Some(Inbox::Request(0, Request::PlaybackCheckpoint(account))) if account == "alex"));
	}

	#[test]
	fn navigation_waits_for_session_and_overtakes_startup_metadata() {
		let mut queue = Queue::default();
		queue.push(Inbox::Request(0, Request::Albums));
		queue.push(Inbox::Request(0, Request::Home));
		queue.push(Inbox::Request(0, Request::Open(Route::Liked, 0)));
		assert!(queue.pop(false).is_none(), "the early page must not become a NeedsLogin error");
		queue.push(Inbox::Request(0, Request::Connect));
		assert!(matches!(queue.pop(false), Some(Inbox::Request(0, Request::Connect))));
		assert!(matches!(queue.pop(true), Some(Inbox::Request(0, Request::Open(Route::Liked, 0)))));
		assert!(matches!(queue.pop(true), Some(Inbox::Request(0, Request::Albums))));
		assert!(queue.pop(true).is_none());
	}

	#[test]
	fn search_replaces_only_search_and_preserves_navigation_and_prior_writes() {
		let mut queue = Queue::default();
		queue.push(Inbox::Request(0, Request::ArtistCard("artist".into())));
		queue.push(Inbox::Request(0, Request::SetLiked("track".into(), true)));
		queue.push(Inbox::Request(0, Request::Open(Route::Album("old".into()), 0)));
		queue.push(Inbox::Request(0, Request::Search("old query".into())));
		queue.push(Inbox::Request(0, Request::Search("keshi".into())));
		assert!(matches!(queue.pop(true), Some(Inbox::Request(0, Request::SetLiked(_, true)))));
		assert!(matches!(queue.pop(true), Some(Inbox::Request(0, Request::Open(Route::Album(id), 0))) if id == "old"));
		assert!(matches!(queue.pop(true), Some(Inbox::Request(0, Request::Search(query))) if query == "keshi"));
		assert!(matches!(queue.pop(true), Some(Inbox::Request(0, Request::ArtistCard(_)))));
		assert!(queue.pop(true).is_none());
	}

	#[test]
	fn pagination_and_playlist_occurrence_writes_remain_in_order() {
		let mut queue = Queue::default();
		let route = Route::Playlist("playlist".into());
		queue.push(Inbox::Request(0, Request::Open(route.clone(), 50)));
		queue.push(Inbox::Request(0, Request::RemoveFromPlaylist("playlist".into(), "track".into(), 55)));
		queue.push(Inbox::Request(0, Request::Open(route, 100)));
		assert!(matches!(queue.pop(true), Some(Inbox::Request(0, Request::Open(_, 50)))));
		assert!(matches!(queue.pop(true), Some(Inbox::Request(0, Request::RemoveFromPlaylist(_, _, 55)))));
		assert!(matches!(queue.pop(true), Some(Inbox::Request(0, Request::Open(_, 100)))));
	}

	#[test]
	fn signout_discards_pending_account_work_and_speculation_is_last() {
		let mut queue = Queue::default();
		queue.push(Inbox::Request(0, Request::Prefetch(Route::Liked)));
		queue.push(Inbox::Request(0, Request::Albums));
		assert!(matches!(queue.pop(true), Some(Inbox::Request(0, Request::Albums))));
		queue.push(Inbox::Request(0, Request::Search("query".into())));
		queue.push(Inbox::Session(0, None));
		assert!(matches!(queue.pop(false), Some(Inbox::Session(0, None))));
		assert!(queue.pop(true).is_none());
	}

	#[test]
	fn navigation_does_not_cancel_search_and_volume_keeps_only_the_latest_per_device() {
		let mut queue = Queue::default();
		queue.push(Inbox::Request(0, Request::Search("query".into())));
		queue.push(Inbox::Request(0, Request::Home));
		queue.push(Inbox::Request(0, Request::Open(Route::Liked, 0)));
		assert!(matches!(queue.pop(true), Some(Inbox::Request(0, Request::Search(_)))));
		assert!(matches!(queue.pop(true), Some(Inbox::Request(0, Request::Open(Route::Liked, 0)))));
		for (device, volume) in [("a", 10), ("b", 30), ("a", 80)] {
			queue.push(Inbox::Request(0, Request::Control {
				device:   device.into(),
				command:  crate::spotify::PlayerCommand::Volume(volume),
				revision: 0,
			}));
		}
		assert!(
			matches!(queue.pop(true), Some(Inbox::Request(0, Request::Control { device, command: crate::spotify::PlayerCommand::Volume(30), .. })) if device == "b")
		);
		assert!(
			matches!(queue.pop(true), Some(Inbox::Request(0, Request::Control { device, command: crate::spotify::PlayerCommand::Volume(80), .. })) if device == "a")
		);
		assert!(queue.pop(true).is_none());
	}

	#[test]
	fn a_volume_slider_burst_queues_one_latest_report_not_a_request_per_step() {
		let mut queue = Queue::default();
		for volume in 0..=100 {
			queue.push(Inbox::Request(0, Request::Control {
				device:   "phone".into(),
				command:  crate::spotify::PlayerCommand::Volume(volume),
				revision: 0,
			}));
		}
		assert!(matches!(
			queue.pop(true),
			Some(Inbox::Request(0, Request::Control { command: crate::spotify::PlayerCommand::Volume(100), .. }))
		));
		assert!(queue.pop(true).is_none(), "101 slider values cause one worker send");
	}
}
