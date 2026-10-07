//! Editing the play queue with the official APIs.
//!
//! The Web API can add to the queue but has no call to remove, move or clear what is in it, and
//! the Connect command the apps use for that (`set_queue`) is refused to our token. A player
//! does take two things: *skip* (the engine) and *start a list* (`PUT /me/player/play` with up to
//! 100 URIs). So an edit is applied by rebuilding the queue:
//!
//! 1. The songs the user queued are skipped over, muted so they aren't heard. Spotify keeps the
//!    manual queue across a `play` call, so left alone they would come back.
//! 2. The edited list starts on the player: the playing song first, at the position it had plus
//!    the delay measured on earlier edits, so the music resumes about where it was.
//! 3. Repeat is set again, a paused player is paused again and the volume is restored.
//!
//! What it costs, and the panel says so: the list becomes what is playing (Spotify shows no
//! playlist any more), shuffle is off (the order is the one on screen), and songs queued later
//! from a phone come after those here. Clicks within [`DEBOUNCE`] of each other go out as one
//! rebuild, since each one restarts the song.

use std::time::{Duration, Instant};

use crate::engine::{Command, Entity, Event, QueueEntry, Repeat, Status};

/// How long clicks may keep coming before the edit goes out.
pub const DEBOUNCE: Duration = Duration::from_millis(900);
/// Songs after the playing one that a rebuilt list can carry: Spotify takes 100 URIs per call.
pub const MAX_UPCOMING: usize = 99;
/// Upcoming songs requested when only the playback rules need the queue.
pub const DEFAULT_QUEUE: usize = 10;
/// How many upcoming songs the panel asks the player for.
pub const WIDE: u32 = 99;
const _: () = assert!(WIDE as usize == MAX_UPCOMING);
/// Skips in one go (a jump over more than a handful of songs isn't a use case).
const MAX_SKIPS: usize = 50;
/// The restart's length before any has been measured (ms).
const LATENCY_START: u64 = 600;
const LATENCY_MIN: u64 = 250;
const LATENCY_MAX: u64 = 2_000;
/// A request Spotify hasn't answered by then is given up on.
const GIVE_UP: Duration = Duration::from_secs(8);
/// Once Spotify took the request, the player has this long to show the new playback.
const LAND_GRACE: Duration = Duration::from_millis(2_500);

/// What shows that a rebuild or a jump has happened.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Sign {
	/// The player reports anything after the request landed (the same song restarts).
	Anything,
	/// The player reports this song as the playing one (a jump).
	Track(String),
}

/// An edit or jump on its way.
#[derive(Debug)]
struct Flight {
	issued:   Instant,
	/// Spotify took the request.
	landed:   Option<Instant>,
	/// The playback showed (volume and the rest are back).
	restored: bool,
	sign:     Sign,
	/// Volume to put back (it was muted for the drain).
	volume:   Option<u8>,
	repeat:   Repeat,
	pause:    bool,
	/// Queues the songs the drain skipped over again, if the rebuild fails.
	undo:     Vec<Command>,
}

/// The queue edits of a session.
#[derive(Debug)]
pub struct Edit {
	/// The upcoming songs as the user left them, shown until the player's queue shows the result.
	list:       Option<Vec<QueueEntry>>,
	/// When the last click was; the rebuild goes out [`DEBOUNCE`] later.
	changed_at: Option<Instant>,
	flight:     Option<Flight>,
	/// Request to first sign of the new playback, as measured on the last rebuild (ms).
	latency_ms: u64,
	/// Commands due for the player (taken by [`Edit::take_commands`]).
	out:        Vec<Command>,
}

impl Default for Edit {
	fn default() -> Self {
		Self { list: None, changed_at: None, flight: None, latency_ms: LATENCY_START, out: Vec::new() }
	}
}

/// What the player has to do about the edit at a deadline.
#[derive(Debug, PartialEq, Eq)]
pub enum Due {
	Nothing,
	/// The clicking paused: send the rebuild ([`Edit::plan`]).
	Rebuild,
	/// Spotify or the player never answered: put everything back ([`Edit::give_up`]).
	GiveUp,
}

/// What the rebuild is made from (the state the edit meets).
pub struct Ctx<'a> {
	pub item:        &'a Entity,
	pub status:      Status,
	pub position_ms: u64,
	pub duration_ms: u64,
	pub volume:      u8,
	pub shuffle:     bool,
	pub repeat:      Repeat,
	/// The player's queue (not the edited one): its leading queued songs are the ones to skip.
	pub upcoming:    &'a [QueueEntry],
}

/// A rebuild to carry out.
#[derive(Debug, PartialEq)]
pub struct Plan {
	/// For the player, first: mute, shuffle off, skip the queued songs.
	pub before:      Vec<Command>,
	/// The new list, the playing song first.
	pub uris:        Vec<String>,
	/// Where in the playing song it starts.
	pub position_ms: u64,
}

/// Whether the Web API can play it from a URI list.
pub fn playable(uri: &str) -> bool {
	uri.starts_with("spotify:track:") || uri.starts_with("spotify:episode:")
}

impl Edit {
	/// The list on screen: the edited one while an edit is out, else the player's.
	pub fn view<'a>(&'a self, upcoming: &'a [QueueEntry]) -> &'a [QueueEntry] {
		self.list.as_deref().unwrap_or(upcoming)
	}

	/// An edit is waiting or on its way: more clicks and jumps wait for it.
	pub fn busy(&self) -> bool {
		self.list.is_some() || self.flight.is_some()
	}

	/// Drops the edit without sending it (the player can't carry it out).
	pub fn cancel(&mut self) {
		self.list = None;
		self.changed_at = None;
	}

	/// How long ago the rebuild went out, and the restart delay the next one will allow for.
	pub fn timing(&self, now: Instant) -> (Option<Duration>, u64) {
		(self.flight.as_ref().map(|f| now.saturating_duration_since(f.issued)), self.latency_ms)
	}

	/// The edit is on its way (rows are shown dimmed).
	pub fn applying(&self) -> bool {
		self.flight.is_some()
	}

	/// The player is muted for the drain: the volume on screen stays where it was.
	pub fn muting(&self) -> bool {
		self.flight.as_ref().is_some_and(|f| f.volume.is_some() && !f.restored)
	}

	/// The user moved the volume while muted: that is what comes back.
	pub fn volume_set(&mut self, volume: u8) {
		if let Some(f) = self.flight.as_mut().filter(|f| f.volume.is_some() && !f.restored) {
			f.volume = Some(volume);
		}
	}

	/// The commands to send the player now.
	pub fn take_commands(&mut self) -> Vec<Command> {
		std::mem::take(&mut self.out)
	}

	/// When the player has to look at the edit.
	pub fn deadline(&self) -> Option<Instant> {
		match (&self.flight, self.changed_at) {
			(Some(f), _) => Some(f.landed.map_or(f.issued + GIVE_UP, |l| l + LAND_GRACE)),
			(None, Some(at)) => Some(at + DEBOUNCE),
			(None, None) => None,
		}
	}

	pub fn due(&self, now: Instant) -> Due {
		match self.deadline() {
			Some(at) if now >= at && self.flight.is_some() => Due::GiveUp,
			Some(at) if now >= at => Due::Rebuild,
			_ => Due::Nothing,
		}
	}

	// ------------------------------------------------------------------ clicks

	/// Takes song `i` out of the list.
	pub fn remove(&mut self, upcoming: &[QueueEntry], i: usize, now: Instant) -> bool {
		self.edit(upcoming, now, |list| {
			let found = i < list.len();
			if found {
				list.remove(i);
			}
			found
		})
	}

	/// Moves song `i` one place up (`delta < 0`) or down.
	pub fn move_by(&mut self, upcoming: &[QueueEntry], i: usize, delta: i8, now: Instant) -> bool {
		self.edit(upcoming, now, |list| {
			let Some(j) = i.checked_add_signed(isize::from(delta)).filter(|j| *j < list.len() && i < list.len()) else {
				return false;
			};
			list.swap(i, j);
			true
		})
	}

	/// Takes the songs the user queued out, as "Clear queue" does.
	pub fn clear_queued(&mut self, upcoming: &[QueueEntry], now: Instant) -> bool {
		self.edit(upcoming, now, |list| {
			let before = list.len();
			list.retain(|e| e.source != "queue");
			list.len() != before
		})
	}

	/// Runs `change` on the list (the first edit starts from the player's). An edit that leaves
	/// the list unchanged is no edit.
	fn edit(&mut self, upcoming: &[QueueEntry], now: Instant, change: impl FnOnce(&mut Vec<QueueEntry>) -> bool) -> bool {
		if self.flight.is_some() {
			return false;
		}
		let mut list = self.list.take().unwrap_or_else(|| upcoming.to_vec());
		let changed = change(&mut list);
		if list.iter().map(|e| e.uid.as_str()).eq(upcoming.iter().map(|e| e.uid.as_str())) {
			self.changed_at = None;
		} else {
			self.list = Some(list);
			if changed {
				self.changed_at = Some(now);
			}
		}
		changed
	}

	// ----------------------------------------------------------------- rebuild

	/// Turns the edit into a rebuild, unless the playing song can't be played from a list (an ad,
	/// a local file). Call when [`Edit::due`] says [`Due::Rebuild`].
	pub fn plan(&mut self, ctx: &Ctx<'_>, now: Instant) -> Option<Plan> {
		self.changed_at = None;
		if !playable(&ctx.item.uri) {
			self.list = None;
			return None;
		}
		let list = self.list.as_ref()?;
		let mut uris = vec![ctx.item.uri.clone()];
		uris.extend(list.iter().map(|e| e.item.uri.as_str()).filter(|u| playable(u)).take(MAX_UPCOMING).map(str::to_owned));

		// The songs queued by hand are the first of the player's list.
		let queued: Vec<&QueueEntry> = ctx.upcoming.iter().take_while(|e| e.source == "queue").take(MAX_SKIPS).collect();
		let mut before = Vec::with_capacity(queued.len() + 2);
		if ctx.volume > 0 {
			before.push(Command::SetVolume { volume: 0 });
		}
		if ctx.shuffle {
			before.push(Command::SetShuffle { enabled: false });
		}
		before.extend(queued.iter().map(|_| Command::SkipNext));

		let playing = ctx.status == Status::Playing;
		let mut position_ms = ctx.position_ms;
		if playing {
			position_ms += self.latency_ms;
		}
		if ctx.duration_ms > 0 {
			position_ms = position_ms.min(ctx.duration_ms.saturating_sub(300));
		}
		self.flight = Some(Flight {
			issued:   now,
			landed:   None,
			restored: false,
			sign:     Sign::Anything,
			volume:   (ctx.volume > 0).then_some(ctx.volume),
			repeat:   ctx.repeat,
			pause:    !playing,
			undo:     queued.iter().map(|e| Command::AddToQueue { uri: e.item.uri.clone() }).collect(),
		});
		Some(Plan { before, uris, position_ms })
	}

	/// Spotify took the rebuild.
	pub fn landed(&mut self, now: Instant) {
		if let Some(f) = self.flight.as_mut() {
			f.landed = Some(now);
		}
	}

	/// Spotify refused it, or couldn't be reached: what the drain took is queued again and the
	/// volume is back. The edited list goes.
	pub fn failed(&mut self) {
		if let Some(f) = self.flight.take() {
			self.out.extend(f.undo);
			if let (Some(volume), false) = (f.volume, f.restored) {
				self.out.push(Command::SetVolume { volume });
			}
		}
		self.list = None;
		self.changed_at = None;
	}

	/// Nothing answered in time: puts everything back as far as it can be. Whether the edit failed
	/// (Spotify never took it) rather than finished without announcing itself.
	pub fn give_up(&mut self) -> bool {
		if self.flight.as_ref().is_some_and(|f| f.landed.is_some()) {
			// Spotify took it; the playback just didn't announce itself. Finish as if it had.
			self.restore();
			self.flight = None;
			self.list = None;
			false
		} else {
			self.failed();
			true
		}
	}

	// -------------------------------------------------------------------- jump

	/// Plays upcoming song `n` (0 is the next one) by skipping to it, muted when that is more than
	/// one skip. That keeps the playlist, shuffle and the rest of the queue as they are.
	pub fn jump(&mut self, n: usize, target: &str, volume: u8, now: Instant) {
		let skips = (n + 1).min(MAX_SKIPS);
		let mute = skips > 1 && volume > 0;
		if mute {
			self.out.push(Command::SetVolume { volume: 0 });
		}
		self.out.extend((0..skips).map(|_| Command::SkipNext));
		if mute {
			self.flight = Some(Flight {
				issued:   now,
				landed:   Some(now),
				restored: false,
				sign:     Sign::Track(target.to_owned()),
				volume:   Some(volume),
				repeat:   Repeat::Off,
				pause:    false,
				undo:     Vec::new(),
			});
		}
	}

	// ------------------------------------------------------------------ events

	/// The player reported a change. Puts back volume, repeat and pause at the first sign
	/// that the new playback is there.
	pub fn on_event(&mut self, event: &Event, now: Instant) {
		let Some((shown, rebuild, issued)) = self.flight.as_ref().filter(|f| !f.restored).map(|f| {
			let shown = match (&f.sign, event) {
				(Sign::Track(uri), Event::TrackChanged { item }) => item.uri == *uri,
				(
					Sign::Anything,
					Event::TrackChanged { .. } | Event::PlaybackChanged { .. } | Event::PositionSync { .. } | Event::QueueChanged { .. },
				) => f.landed.is_some_and(|l| now >= l),
				_ => false,
			};
			(shown, f.sign == Sign::Anything, f.issued)
		}) else {
			return;
		};
		if !shown {
			return;
		}
		if rebuild {
			let measured = u64::try_from(now.saturating_duration_since(issued).as_millis()).unwrap_or(u64::MAX);
			self.latency_ms = blend(self.latency_ms, measured);
		}
		self.restore();
	}

	/// Puts back what the edit changed on the player (once).
	fn restore(&mut self) {
		let Some(f) = self.flight.as_mut().filter(|f| !f.restored) else { return };
		f.restored = true;
		if f.sign == Sign::Anything {
			if f.repeat != Repeat::Off {
				self.out.extend(Command::repeat(f.repeat));
			}
			if f.pause {
				self.out.push(Command::Pause);
			}
		}
		if let Some(volume) = f.volume {
			self.out.push(Command::SetVolume { volume });
		}
	}

	/// The player's queue now shows the result: the edited list has done its job.
	pub fn settled(&mut self) {
		if self.flight.as_ref().is_some_and(|f| f.restored) {
			self.flight = None;
			self.list = None;
		}
	}

	/// The playing song changed by itself while an edit waits: the list moves up with it, or the
	/// edit is void when the player went somewhere the list didn't go.
	pub fn track_changed(&mut self, uri: &str) {
		if self.flight.is_some() {
			return;
		}
		if let Some(list) = self.list.as_mut() {
			match list.iter().position(|e| e.item.uri == uri) {
				Some(j) => {
					list.drain(..=j);
				},
				None => self.list = None,
			}
		}
		if self.list.is_none() {
			self.changed_at = None;
		}
	}
}

/// Half of the old estimate, half of the new measure, within what a restart can take.
fn blend(old: u64, measured: u64) -> u64 {
	old.midpoint(measured).clamp(LATENCY_MIN, LATENCY_MAX)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::engine::Position;

	fn entry(uid: &str, source: &str) -> QueueEntry {
		QueueEntry {
			uid:    uid.into(),
			source: source.into(),
			item:   Entity { uri: format!("spotify:track:{uid}"), ..Entity::default() },
		}
	}

	fn queue() -> Vec<QueueEntry> {
		vec![entry("a", "queue"), entry("b", "queue"), entry("c", "context"), entry("d", "context"), entry("e", "autoplay")]
	}

	fn uids(list: &[QueueEntry]) -> Vec<&str> {
		list.iter().map(|e| e.uid.as_str()).collect()
	}

	fn playing(uri: &str) -> Entity {
		Entity { uri: uri.into(), ..Entity::default() }
	}

	fn ctx<'a>(item: &'a Entity, upcoming: &'a [QueueEntry]) -> Ctx<'a> {
		Ctx {
			item,
			status: Status::Playing,
			position_ms: 10_000,
			duration_ms: 200_000,
			volume: 60,
			shuffle: false,
			repeat: Repeat::Off,
			upcoming,
		}
	}

	#[test]
	fn clicks_edit_a_copy_and_show_it_in_place_of_the_players_list() {
		let up = queue();
		let mut edit = Edit::default();
		assert_eq!(uids(edit.view(&up)), ["a", "b", "c", "d", "e"]);
		let now = Instant::now();
		assert!(edit.remove(&up, 1, now));
		assert_eq!(uids(edit.view(&up)), ["a", "c", "d", "e"]);
		assert!(edit.move_by(&up, 2, -1, now));
		assert_eq!(uids(edit.view(&up)), ["a", "d", "c", "e"]);
		// Out of range does nothing.
		assert!(!edit.move_by(&up, 0, -1, now));
		assert!(!edit.move_by(&up, 3, 1, now));
		assert!(!edit.remove(&up, 9, now));
		assert_eq!(uids(edit.view(&up)), ["a", "d", "c", "e"]);
		assert!(edit.busy());
		// The player's own list is untouched.
		assert_eq!(uids(&up), ["a", "b", "c", "d", "e"]);
	}

	#[test]
	fn clear_takes_only_the_queued_songs() {
		let up = queue();
		let mut edit = Edit::default();
		assert!(edit.clear_queued(&up, Instant::now()));
		assert_eq!(uids(edit.view(&up)), ["c", "d", "e"]);
		// Nothing queued by hand is left to clear.
		assert!(!edit.clear_queued(&up, Instant::now()));
		assert_eq!(uids(edit.view(&up)), ["c", "d", "e"]);
	}

	#[test]
	fn an_edit_undone_is_no_edit() {
		let up = queue();
		let mut edit = Edit::default();
		let now = Instant::now();
		assert!(edit.move_by(&up, 0, 1, now));
		assert!(edit.deadline().is_some());
		assert!(edit.move_by(&up, 1, -1, now));
		assert!(!edit.busy());
		assert_eq!(edit.deadline(), None);
	}

	#[test]
	fn the_rebuild_goes_out_after_the_clicking_pauses() {
		let up = queue();
		let mut edit = Edit::default();
		let t0 = Instant::now();
		edit.remove(&up, 0, t0);
		assert_eq!(edit.due(t0), Due::Nothing);
		edit.remove(&up, 0, t0 + Duration::from_millis(600));
		assert_eq!(edit.due(t0 + Duration::from_millis(1_000)), Due::Nothing);
		assert_eq!(edit.due(t0 + Duration::from_millis(1_500)), Due::Rebuild);
	}

	#[test]
	fn the_plan_mutes_unshuffles_and_skips_the_queued_songs_then_lists_the_rest() {
		let up = queue();
		let item = playing("spotify:track:now");
		let mut edit = Edit::default();
		let t0 = Instant::now();
		edit.remove(&up, 3, t0);
		let mut c = ctx(&item, &up);
		c.shuffle = true;
		let plan = edit.plan(&c, t0).unwrap();
		assert_eq!(plan.before, [
			Command::SetVolume { volume: 0 },
			Command::SetShuffle { enabled: false },
			Command::SkipNext,
			Command::SkipNext
		]);
		assert_eq!(plan.uris, ["spotify:track:now", "spotify:track:a", "spotify:track:b", "spotify:track:c", "spotify:track:e"]);
		// Playing: the list starts a restart's worth ahead.
		assert_eq!(plan.position_ms, 10_000 + LATENCY_START);
		assert!(edit.muting() && edit.applying());
		assert_eq!(edit.due(t0), Due::Nothing);
	}

	#[test]
	fn a_paused_song_restarts_where_it_stopped_and_is_paused_again() {
		let up = vec![entry("c", "context"), entry("d", "context")];
		let item = playing("spotify:track:now");
		let mut edit = Edit::default();
		let t0 = Instant::now();
		edit.remove(&up, 0, t0);
		let mut c = ctx(&item, &up);
		c.status = Status::Paused;
		c.volume = 0;
		let plan = edit.plan(&c, t0).unwrap();
		// Nothing queued by hand to skip, nothing to mute.
		assert!(plan.before.is_empty());
		assert_eq!(plan.position_ms, 10_000);
		assert!(!edit.muting());
		edit.landed(t0);
		edit.on_event(&Event::PositionSync { position: Position::default() }, t0 + Duration::from_millis(300));
		assert_eq!(edit.take_commands(), [Command::Pause]);
	}

	#[test]
	fn the_list_is_capped_and_unplayable_entries_are_left_out() {
		let up: Vec<QueueEntry> = (0..120).map(|i| entry(&i.to_string(), "context")).collect();
		let mut with_local = up.clone();
		with_local[1].item.uri = "spotify:local:a:b:c:1".into();
		let item = playing("spotify:track:now");
		let mut edit = Edit::default();
		edit.remove(&with_local, 0, Instant::now());
		let plan = edit.plan(&ctx(&item, &with_local), Instant::now()).unwrap();
		assert_eq!(plan.uris.len(), 1 + MAX_UPCOMING);
		assert!(!plan.uris.iter().any(|u| u.starts_with("spotify:local:")));
		// An ad (not playable from a list) can't be rebuilt around.
		let ad = playing("spotify:ad:1");
		let mut edit = Edit::default();
		edit.remove(&up, 0, Instant::now());
		assert!(edit.plan(&ctx(&ad, &up), Instant::now()).is_none());
		assert!(!edit.busy());
	}

	#[test]
	fn position_stays_inside_the_song() {
		let up = queue();
		let item = playing("spotify:track:now");
		let mut edit = Edit::default();
		edit.remove(&up, 0, Instant::now());
		let mut c = ctx(&item, &up);
		c.position_ms = 199_800;
		assert_eq!(edit.plan(&c, Instant::now()).unwrap().position_ms, 199_700);
	}

	#[test]
	fn the_first_sign_after_the_request_landed_puts_everything_back() {
		let up = queue();
		let item = playing("spotify:track:now");
		let mut edit = Edit::default();
		let t0 = Instant::now();
		edit.remove(&up, 0, t0);
		let mut c = ctx(&item, &up);
		c.repeat = Repeat::Context;
		let _ = edit.plan(&c, t0).unwrap();
		// Events from the skipping, before the request landed, are no sign.
		edit.on_event(&Event::TrackChanged { item: playing("spotify:track:a") }, t0 + Duration::from_millis(20));
		assert!(edit.take_commands().is_empty() && edit.muting());
		edit.landed(t0 + Duration::from_millis(250));
		edit.on_event(&Event::QueueChanged { upcoming: Vec::new() }, t0 + Duration::from_millis(700));
		assert_eq!(edit.take_commands(), [
			Command::SetRepeatTrack { enabled: false },
			Command::SetRepeatContext { enabled: true },
			Command::SetVolume { volume: 60 }
		]);
		assert!(!edit.muting());
		// Put back once; the list stays until the player's own queue shows it.
		edit.on_event(&Event::QueueChanged { upcoming: Vec::new() }, t0 + Duration::from_millis(800));
		assert!(edit.take_commands().is_empty());
		assert!(edit.busy());
		edit.settled();
		assert!(!edit.busy());
	}

	#[test]
	fn a_failed_rebuild_queues_the_skipped_songs_again_and_unmutes() {
		let up = queue();
		let item = playing("spotify:track:now");
		let mut edit = Edit::default();
		edit.remove(&up, 4, Instant::now());
		let _ = edit.plan(&ctx(&item, &up), Instant::now()).unwrap();
		edit.failed();
		assert_eq!(edit.take_commands(), [
			Command::AddToQueue { uri: "spotify:track:a".into() },
			Command::AddToQueue { uri: "spotify:track:b".into() },
			Command::SetVolume { volume: 60 }
		]);
		assert!(!edit.busy());
	}

	#[test]
	fn giving_up_before_it_landed_is_a_failure_and_after_it_a_finish() {
		let up = queue();
		let item = playing("spotify:track:now");
		let t0 = Instant::now();
		let mut edit = Edit::default();
		edit.remove(&up, 0, t0);
		let _ = edit.plan(&ctx(&item, &up), t0).unwrap();
		assert_eq!(edit.due(t0 + GIVE_UP), Due::GiveUp);
		edit.give_up();
		assert!(edit.take_commands().contains(&Command::SetVolume { volume: 60 }));
		assert!(!edit.busy());

		let mut edit = Edit::default();
		edit.remove(&up, 0, t0);
		let _ = edit.plan(&ctx(&item, &up), t0).unwrap();
		edit.landed(t0 + Duration::from_millis(200));
		assert_eq!(edit.deadline(), Some(t0 + Duration::from_millis(200) + LAND_GRACE));
		edit.give_up();
		assert_eq!(edit.take_commands(), [Command::SetVolume { volume: 60 }]);
		assert!(!edit.busy());
	}

	#[test]
	fn a_jump_skips_muted_until_its_song_plays() {
		let mut edit = Edit::default();
		let t0 = Instant::now();
		edit.jump(2, "spotify:track:c", 40, t0);
		assert_eq!(edit.take_commands(), [Command::SetVolume { volume: 0 }, Command::SkipNext, Command::SkipNext, Command::SkipNext]);
		assert!(edit.muting() && edit.list.is_none());
		// Songs passed on the way aren't the sign.
		edit.on_event(&Event::TrackChanged { item: playing("spotify:track:a") }, t0);
		assert!(edit.take_commands().is_empty());
		edit.on_event(&Event::TrackChanged { item: playing("spotify:track:c") }, t0);
		assert_eq!(edit.take_commands(), [Command::SetVolume { volume: 40 }]);
		edit.settled();
		assert!(!edit.busy());
	}

	#[test]
	fn jumping_to_the_next_song_is_one_plain_skip() {
		let mut edit = Edit::default();
		edit.jump(0, "spotify:track:a", 40, Instant::now());
		assert_eq!(edit.take_commands(), [Command::SkipNext]);
		assert!(!edit.busy() && !edit.muting());
	}

	#[test]
	fn a_song_that_changes_by_itself_moves_the_waiting_edit_up_or_voids_it() {
		let up = queue();
		let mut edit = Edit::default();
		edit.remove(&up, 3, Instant::now());
		// "a" is next in the edited list too: the edit keeps what's after it.
		edit.track_changed("spotify:track:a");
		assert_eq!(uids(edit.view(&up)), ["b", "c", "e"]);
		// Somewhere the list didn't go: void.
		edit.track_changed("spotify:track:zzz");
		assert!(!edit.busy());
		assert_eq!(edit.deadline(), None);
	}

	#[test]
	fn a_flying_edit_ignores_clicks() {
		let up = queue();
		let item = playing("spotify:track:now");
		let mut edit = Edit::default();
		edit.remove(&up, 0, Instant::now());
		let _ = edit.plan(&ctx(&item, &up), Instant::now()).unwrap();
		assert!(!edit.remove(&up, 0, Instant::now()));
		assert!(!edit.clear_queued(&up, Instant::now()));
	}

	#[test]
	fn volume_moved_during_the_mute_is_what_comes_back() {
		let up = queue();
		let item = playing("spotify:track:now");
		let t0 = Instant::now();
		let mut edit = Edit::default();
		edit.remove(&up, 0, t0);
		let _ = edit.plan(&ctx(&item, &up), t0).unwrap();
		edit.volume_set(30);
		edit.landed(t0);
		edit.on_event(&Event::PlaybackChanged { status: Status::Playing }, t0 + Duration::from_millis(10));
		assert_eq!(edit.take_commands(), [Command::SetVolume { volume: 30 }]);
	}
}
