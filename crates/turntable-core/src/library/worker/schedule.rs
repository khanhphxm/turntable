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
		if matches!(&message, Inbox::Request(Request::Logout | Request::ForgetSnapshot | Request::Login) | Inbox::Session(None)) {
			self.pending.clear();
		}
		if matches!(&message, Inbox::Request(Request::Open(_, 0) | Request::Search(_))) {
			// Shell setup can enqueue Home before opening its actual destination. Only the
			// latest first-page/search navigation matters; writes and pagination never coalesce.
			self.pending.retain(|known| {
				!matches!(known, Inbox::Request(Request::Home | Request::HomeFacet(_) | Request::Open(_, 0) | Request::Search(_)))
			});
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
		Inbox::Session(_) | Inbox::Request(Request::Connect | Request::Login | Request::Logout | Request::ForgetSnapshot) => 0,
		Inbox::Request(request) if foreground(request) => 1,
		Inbox::Request(Request::Prefetch(_)) => 3,
		Inbox::Request(_) => 2,
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
			| Request::Prefetch(_)
	)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::library::Route;

	#[test]
	fn navigation_waits_for_session_and_overtakes_startup_metadata() {
		let mut queue = Queue::default();
		queue.push(Inbox::Request(Request::Albums));
		queue.push(Inbox::Request(Request::Home));
		queue.push(Inbox::Request(Request::Open(Route::Liked, 0)));
		assert!(queue.pop(false).is_none(), "the early page must not become a NeedsLogin error");
		queue.push(Inbox::Request(Request::Connect));
		assert!(matches!(queue.pop(false), Some(Inbox::Request(Request::Connect))));
		assert!(matches!(queue.pop(true), Some(Inbox::Request(Request::Open(Route::Liked, 0)))));
		assert!(matches!(queue.pop(true), Some(Inbox::Request(Request::Albums))));
		assert!(queue.pop(true).is_none());
	}

	#[test]
	fn search_overtakes_metadata_and_obsolete_route_but_not_prior_writes() {
		let mut queue = Queue::default();
		queue.push(Inbox::Request(Request::ArtistCard("artist".into())));
		queue.push(Inbox::Request(Request::SetLiked("track".into(), true)));
		queue.push(Inbox::Request(Request::Open(Route::Album("old".into()), 0)));
		queue.push(Inbox::Request(Request::Search("keshi".into())));
		assert!(matches!(queue.pop(true), Some(Inbox::Request(Request::SetLiked(_, true)))));
		assert!(matches!(queue.pop(true), Some(Inbox::Request(Request::Search(query))) if query == "keshi"));
		assert!(matches!(queue.pop(true), Some(Inbox::Request(Request::ArtistCard(_)))));
		assert!(queue.pop(true).is_none());
	}

	#[test]
	fn pagination_and_playlist_occurrence_writes_remain_in_order() {
		let mut queue = Queue::default();
		let route = Route::Playlist("playlist".into());
		queue.push(Inbox::Request(Request::Open(route.clone(), 50)));
		queue.push(Inbox::Request(Request::RemoveFromPlaylist("playlist".into(), "track".into(), 55)));
		queue.push(Inbox::Request(Request::Open(route, 100)));
		assert!(matches!(queue.pop(true), Some(Inbox::Request(Request::Open(_, 50)))));
		assert!(matches!(queue.pop(true), Some(Inbox::Request(Request::RemoveFromPlaylist(_, _, 55)))));
		assert!(matches!(queue.pop(true), Some(Inbox::Request(Request::Open(_, 100)))));
	}

	#[test]
	fn signout_discards_pending_account_work_and_speculation_is_last() {
		let mut queue = Queue::default();
		queue.push(Inbox::Request(Request::Prefetch(Route::Liked)));
		queue.push(Inbox::Request(Request::Albums));
		assert!(matches!(queue.pop(true), Some(Inbox::Request(Request::Albums))));
		queue.push(Inbox::Request(Request::Search("query".into())));
		queue.push(Inbox::Session(None));
		assert!(matches!(queue.pop(false), Some(Inbox::Session(None))));
		assert!(queue.pop(true).is_none());
	}
}
