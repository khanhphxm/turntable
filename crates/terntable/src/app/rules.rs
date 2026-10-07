//! The playback rules the settings add: autoplay off, no explicit songs, and the sleep timer.
//! The app carries them out from player events, through pause, skip and play commands.
//!
//! Pure state, like the rest of [`State`]: the methods return the commands, and the player loop
//! sends them and sleeps until [`State::rules_deadline`]. Nothing here acts while another device
//! plays (the player isn't the active one then), and a queue edit in flight is left alone.

use std::time::{Duration, Instant};

use crate::{
	app::{Panel, State, queue},
	engine::{Command, Entity, QueueEntry},
};

/// How long before a song's end a pause is sent. The audio already on its way (the player's
/// buffer holds about 140 ms) still plays, so the song ends within a blink of its real end; and
/// the player has not yet moved on to the next one.
const HOLD_BEFORE_END: Duration = Duration::from_millis(250);
/// A song that changed this near the previous one's end changed by itself, not because of a skip.
const NEAR_END: Duration = Duration::from_secs(3);
/// Explicit songs skipped in a row before giving up (a list of nothing else would skip forever).
const MAX_SKIPS: u8 = 25;
/// Skips this far apart are not one run.
const SKIP_RUN: Duration = Duration::from_secs(10);

/// The sleep timer's lengths, in minutes.
pub const SLEEP_MINUTES: [u16; 4] = [15, 30, 45, 60];

/// What the sleep timer's dropdown offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SleepChoice {
	Off,
	Minutes(u16),
	EndOfTrack,
}

/// A sleep timer that is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sleep {
	/// Pause at this moment.
	At { when: Instant, minutes: u16 },
	/// Pause when the song playing ends.
	EndOfTrack,
}

/// Where the rules stand. Not saved: a sleep timer doesn't outlive the app.
#[derive(Debug, Default)]
pub struct Rules {
	pub sleep: Option<Sleep>,
	/// The song the player was paused at the end of, so it isn't paused again at once.
	held:      Option<Held>,
	/// Explicit songs skipped in the current run, and when the last was.
	skips:     u8,
	last_skip: Option<Instant>,
}

#[derive(Debug)]
struct Held {
	uri:      String,
	/// Held because autoplay is off (play then starts the context over, as Spotify does).
	autoplay: bool,
}

/// What the rules need to know of the state before an event changed it.
#[derive(Debug)]
pub struct Before {
	uri:          String,
	/// The song was in its last seconds.
	near_end:     bool,
	/// The song that would have come next, if it was one of Spotify's recommendations.
	autoplay_uri: Option<String>,
}

/// Whether Spotify marks the item explicit.
pub fn is_explicit(item: &Entity) -> bool {
	item.decorations.playback.content_ratings.iter().any(|r| r.eq_ignore_ascii_case("explicit"))
}

/// Whether `uri` is something `play` can start over (not a single song, nor a radio).
fn is_context(uri: &str) -> bool {
	["spotify:album:", "spotify:playlist:", "spotify:artist:", "spotify:show:"].iter().any(|p| uri.starts_with(p))
}

impl State {
	/// Starts, changes or cancels the sleep timer.
	pub fn set_sleep(&mut self, choice: SleepChoice, now: Instant) {
		self.rules.sleep = match choice {
			SleepChoice::Off => None,
			SleepChoice::Minutes(minutes) => Some(Sleep::At { when: now + Duration::from_secs(u64::from(minutes) * 60), minutes }),
			SleepChoice::EndOfTrack => Some(Sleep::EndOfTrack),
		};
		let note = match choice {
			SleepChoice::Off => "Sleep timer off".to_owned(),
			SleepChoice::Minutes(minutes) => format!("Music will pause in {minutes} minutes"),
			SleepChoice::EndOfTrack => "Music will pause when this song ends".to_owned(),
		};
		self.show_toast(note);
	}

	/// The time left on the sleep timer, if it counts down (not for "end of song").
	pub fn sleep_left(&self, now: Instant) -> Option<Duration> {
		match self.rules.sleep {
			Some(Sleep::At { when, .. }) => Some(when.saturating_duration_since(now)),
			_ => None,
		}
	}

	/// What is next to play, if it is a recommendation: the player has run out of what was chosen.
	/// (A list that still names the song playing is out of date, not an answer.)
	fn autoplay_next(&self) -> Option<&QueueEntry> {
		self.upcoming.first().filter(|e| e.source == "autoplay" && e.item.uri != self.item.uri)
	}

	/// Whether the player pauses at the end of the song playing: the sleep timer wants it, or
	/// autoplay is off and the song is the last of what was chosen.
	fn holds_at_end(&self) -> bool {
		if !self.active || !self.playing() || self.duration_ms() == 0 || self.queue_edit.busy() {
			return false;
		}
		if self.rules.held.as_ref().is_some_and(|h| h.uri == self.item.uri) {
			return false;
		}
		matches!(self.rules.sleep, Some(Sleep::EndOfTrack)) || (!self.prefs.playback.autoplay && self.autoplay_next().is_some())
	}

	/// When to pause for the end of the song: a moment before it, by the playback clock.
	fn hold_deadline(&self) -> Option<Instant> {
		if !self.holds_at_end() {
			return None;
		}
		let before_end = u64::try_from(HOLD_BEFORE_END.as_millis()).unwrap_or(0);
		self.anchor.instant_at(self.duration_ms().saturating_sub(before_end))
	}

	/// The next moment a rule has something to do, for the player loop to sleep until.
	pub fn rules_deadline(&self) -> Option<Instant> {
		let sleep = match self.rules.sleep {
			Some(Sleep::At { when, .. }) => Some(when),
			_ => None,
		};
		[sleep, self.hold_deadline()].into_iter().flatten().min()
	}

	/// Carries out what is due at `now`: the sleep timer running out, the end of a song that is
	/// to be the last. Returns the commands to send.
	pub fn rules_due(&mut self, now: Instant) -> Vec<Command> {
		let mut cmds = Vec::new();
		if let Some(Sleep::At { when, .. }) = self.rules.sleep
			&& now >= when
		{
			self.rules.sleep = None;
			if self.active && self.playing() {
				cmds.push(Command::Pause);
			}
			self.show_toast("Sleep timer: paused".to_owned());
		}
		if self.hold_deadline().is_some_and(|at| now >= at) {
			let by_timer = matches!(self.rules.sleep, Some(Sleep::EndOfTrack));
			let autoplay = !self.prefs.playback.autoplay && self.autoplay_next().is_some();
			if by_timer {
				self.rules.sleep = None;
			}
			cmds.push(Command::Pause);
			self.rules.held = Some(Held { uri: self.item.uri.clone(), autoplay });
			let note = if by_timer { "Sleep timer: paused at the end of the song" } else { "Paused at the end: autoplay is off" };
			self.show_toast(note.to_owned());
		}
		cmds
	}

	/// A snapshot for [`State::rules_after`], taken before an event is applied.
	pub fn rules_before(&self) -> Before {
		let near = u64::try_from(NEAR_END.as_millis()).unwrap_or(0);
		Before {
			uri:          self.item.uri.clone(),
			near_end:     self.duration_ms() > 0 && self.position_ms() + near >= self.duration_ms(),
			autoplay_uri: self.autoplay_next().map(|e| e.item.uri.clone()),
		}
	}

	/// What the rules do about an event that was just applied: skip an explicit song, pause a song of
	/// recommendations that slipped past the pause at the end, and look at the queue again when
	/// autoplay is off (what follows a song decides whether to pause at its end).
	pub fn rules_after(&mut self, before: &Before, now: Instant) -> Vec<Command> {
		let mut cmds = Vec::new();
		if !self.active || self.item.uri == before.uri {
			return cmds;
		}
		// Whatever was held is behind us now.
		self.rules.held = None;
		if !self.prefs.playback.explicit && is_explicit(&self.item) {
			return self.skip_explicit(now);
		}
		self.rules.skips = 0;
		if !self.prefs.playback.autoplay {
			// A recommendation reached the player by itself, at the end of what was chosen.
			if self.playing() && before.near_end && before.autoplay_uri.as_deref() == Some(self.item.uri.as_str()) {
				cmds.push(Command::Pause);
				self.rules.held = Some(Held { uri: self.item.uri.clone(), autoplay: true });
				self.show_toast("Paused: autoplay is off".to_owned());
			}
			// What follows this song decides whether it is the last: ask rather than assume the
			// player broadcasts the queue at every change.
			let limit = if self.panel == Some(Panel::Queue) { queue::WIDE } else { u32::try_from(queue::DEFAULT_QUEUE).unwrap_or(10) };
			cmds.push(Command::GetQueue { limit });
		}
		cmds
	}

	/// The song that just started is explicit and explicit songs are off: on to the next, unless a
	/// whole run of them has gone by.
	fn skip_explicit(&mut self, now: Instant) -> Vec<Command> {
		if self.queue_edit.busy() {
			return Vec::new();
		}
		if self.rules.last_skip.is_none_or(|t| now.duration_since(t) > SKIP_RUN) {
			self.rules.skips = 0;
		}
		self.rules.last_skip = Some(now);
		self.rules.skips += 1;
		if self.rules.skips > MAX_SKIPS {
			self.rules.skips = 0;
			self.show_toast("Everything coming up is marked explicit".to_owned());
			return vec![Command::Pause];
		}
		if self.rules.skips == 1 {
			let note = format!("Skipped “{}”: explicit songs are off in Settings", self.item.name());
			self.show_toast(note);
		}
		vec![Command::SkipNext]
	}

	/// Play after the player paused itself at the end of what was chosen (autoplay off): starts the
	/// playlist or album over, as Spotify does, instead of letting the recommendations in. `None`
	/// for any other play.
	pub fn rules_resume(&mut self) -> Option<Command> {
		let held = self.rules.held.take_if(|h| h.uri == self.item.uri)?;
		if !held.autoplay {
			return None;
		}
		let uri = if is_context(&self.context.uri) { self.context.uri.clone() } else { self.item.uri.clone() };
		Some(Command::Play { uri: Some(uri) })
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{app::Anchor, engine::Status};

	/// Playing song `a` (200 s long) 100 s in, on this player; the queue starts with `up`.
	fn playing(autoplay: bool, up: &[(&str, &str)]) -> State {
		let mut s = State { active: true, status: Status::Playing, ..State::default() };
		s.prefs.playback.autoplay = autoplay;
		s.item.uri = "spotify:track:a".into();
		s.item.decorations.playback.duration_ms = 200_000;
		s.context.uri = "spotify:playlist:p".into();
		s.anchor = Anchor { position_ms: 100_000, at: Instant::now(), speed: 1.0 };
		s.upcoming = up.iter().map(|(source, uri)| entry(source, uri)).collect();
		s
	}

	fn entry(source: &str, uri: &str) -> QueueEntry {
		let mut e = QueueEntry { uid: uri.into(), source: source.into(), ..QueueEntry::default() };
		e.item.uri = uri.into();
		e
	}

	#[test]
	fn with_autoplay_on_nothing_waits_for_the_end() {
		let s = playing(true, &[("autoplay", "spotify:track:r1")]);
		assert_eq!(s.rules_deadline(), None);
	}

	#[test]
	fn with_autoplay_off_the_last_song_is_paused_just_before_it_ends() {
		let mut s = playing(false, &[("autoplay", "spotify:track:r1"), ("autoplay", "spotify:track:r2")]);
		let due = s.rules_deadline().expect("a pause is scheduled");
		// 100 s to go less the margin, by the playback clock.
		let ms = due.duration_since(s.anchor.at).as_millis();
		assert!((99_700..=99_800).contains(&ms), "{ms}");
		assert!(s.rules_due(due - Duration::from_millis(5)).is_empty(), "not before");
		assert_eq!(s.rules_due(due), vec![Command::Pause]);
		assert!(s.toast.is_some());
		// Sent once: the state says "playing" until the player's event arrives.
		assert!(s.rules_due(due + Duration::from_millis(1)).is_empty());
		assert_eq!(s.rules_deadline(), None);
	}

	#[test]
	fn only_the_last_of_what_was_chosen_is_held() {
		// More of the playlist, or a queued song, comes next: no pause. Nor with nothing known.
		for up in [&[("context", "spotify:track:b")][..], &[("queue", "spotify:track:q"), ("autoplay", "spotify:track:r")], &[]] {
			assert_eq!(playing(false, up).rules_deadline(), None, "{up:?}");
		}
		// An out-of-date list that still has the song playing at its head says nothing.
		assert_eq!(playing(false, &[("autoplay", "spotify:track:a")]).rules_deadline(), None);
	}

	#[test]
	fn nothing_is_held_while_paused_standing_still_or_on_another_device() {
		let mut s = playing(false, &[("autoplay", "spotify:track:r")]);
		s.status = Status::Paused;
		assert_eq!(s.rules_deadline(), None);
		let mut s = playing(false, &[("autoplay", "spotify:track:r")]);
		s.anchor.speed = 0.0;
		assert_eq!(s.rules_deadline(), None, "buffering: the clock isn't running");
		let mut s = playing(false, &[("autoplay", "spotify:track:r")]);
		s.active = false;
		assert_eq!(s.rules_deadline(), None);
		assert!(s.rules_due(Instant::now() + Duration::from_secs(1000)).is_empty());
	}

	#[test]
	fn a_seek_or_a_changed_setting_moves_the_pause() {
		let mut s = playing(true, &[("autoplay", "spotify:track:r")]);
		assert_eq!(s.rules_deadline(), None);
		s.prefs.playback.autoplay = false;
		assert!(s.rules_deadline().is_some(), "turned off with the last song playing");
		s.anchor = Anchor { position_ms: 190_000, at: s.anchor.at, speed: 1.0 };
		let ms = s.rules_deadline().unwrap().duration_since(s.anchor.at).as_millis();
		assert!((9_700..=9_800).contains(&ms), "{ms}");
		// Already past the margin: due at once.
		s.anchor = Anchor { position_ms: 199_900, at: s.anchor.at, speed: 1.0 };
		assert!(s.rules_deadline().unwrap() <= s.anchor.at);
	}

	#[test]
	fn playing_again_after_the_hold_starts_the_playlist_over() {
		let mut s = playing(false, &[("autoplay", "spotify:track:r")]);
		let due = s.rules_deadline().unwrap();
		s.rules_due(due);
		s.status = Status::Paused;
		assert_eq!(s.rules_resume(), Some(Command::Play { uri: Some("spotify:playlist:p".into()) }));
		assert_eq!(s.rules_resume(), None, "the hold is spent");
		// A context that can't be started over: the song itself.
		let mut s = playing(false, &[("autoplay", "spotify:track:r")]);
		s.context.uri = String::new();
		let due = s.rules_deadline().unwrap();
		s.rules_due(due);
		assert_eq!(s.rules_resume(), Some(Command::Play { uri: Some("spotify:track:a".into()) }));
		// A pause that wasn't ours is an ordinary resume.
		assert_eq!(playing(false, &[]).rules_resume(), None);
	}

	#[test]
	fn the_sleep_timer_pauses_when_it_runs_out() {
		let mut s = playing(true, &[]);
		let start = Instant::now();
		s.set_sleep(SleepChoice::Minutes(30), start);
		let due = s.rules_deadline().unwrap();
		assert_eq!(due, start + Duration::from_secs(30 * 60));
		assert_eq!(s.sleep_left(start), Some(Duration::from_secs(1800)));
		assert!(s.rules_due(start + Duration::from_secs(1799)).is_empty());
		assert_eq!(s.rules_due(due), vec![Command::Pause]);
		assert_eq!(s.rules.sleep, None, "it is spent");
		// Already paused when it runs out: nothing to pause.
		s.status = Status::Paused;
		s.set_sleep(SleepChoice::Minutes(15), start);
		assert!(s.rules_due(start + Duration::from_secs(900)).is_empty());
		assert_eq!(s.rules.sleep, None);
	}

	#[test]
	fn the_sleep_timer_can_be_changed_and_cancelled() {
		let mut s = playing(true, &[]);
		let now = Instant::now();
		s.set_sleep(SleepChoice::Minutes(15), now);
		s.set_sleep(SleepChoice::Minutes(60), now);
		assert_eq!(s.sleep_left(now), Some(Duration::from_secs(3600)));
		s.set_sleep(SleepChoice::Off, now);
		assert_eq!(s.rules_deadline(), None);
		assert_eq!(s.sleep_left(now), None);
	}

	#[test]
	fn the_end_of_song_timer_pauses_at_the_end_of_this_song_once() {
		let mut s = playing(true, &[("context", "spotify:track:b")]);
		s.set_sleep(SleepChoice::EndOfTrack, Instant::now());
		assert_eq!(s.sleep_left(Instant::now()), None, "it doesn't count down");
		let due = s.rules_deadline().unwrap();
		assert!((99_700..=99_800).contains(&due.duration_since(s.anchor.at).as_millis()));
		assert_eq!(s.rules_due(due), vec![Command::Pause]);
		assert_eq!(s.rules.sleep, None);
		// Resuming plays on into the next song: it was a one-time wish.
		assert_eq!(s.rules_resume(), None);
		s.status = Status::Playing;
		assert_eq!(s.rules_deadline(), None);
	}

	#[test]
	fn explicit_songs_are_skipped_when_they_are_off() {
		let mut s = playing(true, &[]);
		s.prefs.playback.explicit = false;
		let before = s.rules_before();
		s.item.uri = "spotify:track:e1".into();
		s.item.decorations.playback.content_ratings = vec!["explicit".into()];
		let now = Instant::now();
		assert_eq!(s.rules_after(&before, now), vec![Command::SkipNext]);
		assert!(s.toast.as_ref().is_some_and(|(_, t)| t.contains("explicit")));
		// The next one is clean: played, and the count starts over.
		let before = s.rules_before();
		s.item.uri = "spotify:track:c".into();
		s.item.decorations.playback.content_ratings.clear();
		assert!(!s.rules_after(&before, now).contains(&Command::SkipNext));
		assert_eq!(s.rules.skips, 0);
	}

	#[test]
	fn explicit_songs_play_when_they_are_allowed_and_ratings_read_loosely() {
		let mut s = playing(true, &[]);
		let before = s.rules_before();
		s.item.uri = "spotify:track:e1".into();
		s.item.decorations.playback.content_ratings = vec!["explicit".into()];
		assert!(s.rules_after(&before, Instant::now()).is_empty());
		// Other ratings are not explicit; case doesn't matter.
		assert!(!is_explicit(&Entity::default()));
		let mut e = Entity::default();
		e.decorations.playback.content_ratings = vec!["south_korea_19".into()];
		assert!(!is_explicit(&e));
		e.decorations.playback.content_ratings = vec!["Explicit".into()];
		assert!(is_explicit(&e));
	}

	#[test]
	fn a_list_of_nothing_but_explicit_songs_stops_instead_of_skipping_forever() {
		let mut s = playing(true, &[]);
		s.prefs.playback.explicit = false;
		let now = Instant::now();
		let mut sent = Vec::new();
		for i in 0..40 {
			let before = s.rules_before();
			s.item.uri = format!("spotify:track:e{i}");
			s.item.decorations.playback.content_ratings = vec!["explicit".into()];
			let cmds = s.rules_after(&before, now);
			let stopped = cmds.contains(&Command::Pause);
			sent.extend(cmds);
			// Paused, the player plays no more songs: that ends the run.
			if stopped {
				break;
			}
		}
		let skips = sent.iter().filter(|c| **c == Command::SkipNext).count();
		assert_eq!(skips, usize::from(MAX_SKIPS));
		assert!(sent.contains(&Command::Pause));
		assert!(s.toast.as_ref().is_some_and(|(_, t)| t.contains("Everything coming up")));
	}

	#[test]
	fn skips_a_while_apart_are_separate_runs() {
		let mut s = playing(true, &[]);
		s.prefs.playback.explicit = false;
		let start = Instant::now();
		for (i, secs) in [0u64, 20, 40].into_iter().enumerate() {
			let before = s.rules_before();
			s.item.uri = format!("spotify:track:e{i}");
			s.item.decorations.playback.content_ratings = vec!["explicit".into()];
			s.rules_after(&before, start + Duration::from_secs(secs));
			assert_eq!(s.rules.skips, 1, "run {i}");
		}
	}

	#[test]
	fn a_recommendation_that_slipped_in_at_the_end_is_paused() {
		let mut s = playing(false, &[("autoplay", "spotify:track:r1")]);
		// The player moved on by itself in the last seconds, to the recommendation.
		s.anchor = Anchor { position_ms: 199_900, at: Instant::now(), speed: 1.0 };
		let before = s.rules_before();
		assert!(before.near_end);
		s.item.uri = "spotify:track:r1".into();
		s.anchor = Anchor { position_ms: 0, at: Instant::now(), speed: 1.0 };
		let cmds = s.rules_after(&before, Instant::now());
		assert!(cmds.contains(&Command::Pause), "{cmds:?}");
		assert!(s.rules.held.as_ref().is_some_and(|h| h.uri == "spotify:track:r1"));
		// Resuming it is the user's call: the rule lets that one play.
		assert_eq!(s.rules_resume(), Some(Command::Play { uri: Some("spotify:playlist:p".into()) }));
	}

	#[test]
	fn a_skip_into_the_recommendations_is_the_users_own_choice() {
		let mut s = playing(false, &[("autoplay", "spotify:track:r1")]);
		// Skipped mid-song, not at its end.
		let before = s.rules_before();
		assert!(!before.near_end);
		s.item.uri = "spotify:track:r1".into();
		assert!(!s.rules_after(&before, Instant::now()).contains(&Command::Pause));
	}

	#[test]
	fn with_autoplay_off_the_queue_is_asked_for_again_at_each_new_song() {
		let mut s = playing(false, &[("context", "spotify:track:b")]);
		let before = s.rules_before();
		s.item.uri = "spotify:track:b".into();
		assert_eq!(s.rules_after(&before, Instant::now()), vec![Command::GetQueue { limit: 10 }]);
		// With the queue open the whole list is wanted.
		s.panel = Some(Panel::Queue);
		let before = s.rules_before();
		s.item.uri = "spotify:track:c".into();
		assert_eq!(s.rules_after(&before, Instant::now()), vec![Command::GetQueue { limit: queue::WIDE }]);
		// With autoplay on there is nothing to ask for.
		let mut on = playing(true, &[]);
		let before = on.rules_before();
		on.item.uri = "spotify:track:b".into();
		assert!(on.rules_after(&before, Instant::now()).is_empty());
	}

	#[test]
	fn another_device_playing_is_none_of_the_rules_business() {
		let mut s = playing(false, &[]);
		s.active = false;
		s.prefs.playback.explicit = false;
		let before = s.rules_before();
		s.item.uri = "spotify:track:e".into();
		s.item.decorations.playback.content_ratings = vec!["explicit".into()];
		assert!(s.rules_after(&before, Instant::now()).is_empty());
	}
}
