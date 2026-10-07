//! Shared bounded caches and request coalescing.
//!
//! [`Cache`] holds what a page needed (an artist, an album's tracks) so visiting it again costs no
//! request: a least-recently-used map with a size cap and a lifetime. [`Flights`] makes callers
//! that ask for the same thing at the same time share one answer.

use std::{
	collections::HashMap,
	hash::Hash,
	sync::{Arc, Condvar, Mutex, PoisonError},
	time::{Duration, Instant},
};

/// A least-recently-used map whose entries also expire.
pub struct Cache<K, V> {
	map:  HashMap<K, Slot<V>>,
	cap:  usize,
	ttl:  Duration,
	tick: u64,
}

struct Slot<V> {
	value: V,
	at:    Instant,
	/// The last use, on the cache's own clock (`Cache::tick`).
	used:  u64,
}

impl<K: Hash + Eq + Clone, V: Clone> Cache<K, V> {
	pub fn new(cap: usize, ttl: Duration) -> Self {
		Self { map: HashMap::new(), cap, ttl, tick: 0 }
	}

	/// The entry, while it is younger than the lifetime.
	pub fn get(&mut self, key: &K) -> Option<V> {
		self.get_at(key, Instant::now())
	}

	pub fn get_at(&mut self, key: &K, now: Instant) -> Option<V> {
		match self.get_stale_at(key, now) {
			Some((value, true)) => Some(value),
			Some((_, false)) => {
				self.map.remove(key);
				None
			},
			None => None,
		}
	}

	fn get_stale_at(&mut self, key: &K, now: Instant) -> Option<(V, bool)> {
		self.tick += 1;
		let tick = self.tick;
		let slot = self.map.get_mut(key)?;
		slot.used = tick;
		Some((slot.value.clone(), now.saturating_duration_since(slot.at) < self.ttl))
	}

	pub fn insert(&mut self, key: K, value: V) {
		self.insert_at(key, value, Instant::now());
	}

	pub fn insert_at(&mut self, key: K, value: V, now: Instant) {
		self.tick += 1;
		self.map.insert(key, Slot { value, at: now, used: self.tick });
		while self.map.len() > self.cap {
			let oldest = self.map.iter().min_by_key(|(_, slot)| slot.used).map(|(key, _)| key.clone());
			match oldest {
				Some(key) => self.map.remove(&key),
				None => break,
			};
		}
	}

	pub fn remove(&mut self, key: &K) -> Option<V> {
		self.map.remove(key).map(|slot| slot.value)
	}

	pub fn clear(&mut self) {
		self.map.clear();
	}

	/// Drops the entries older than the lifetime (they would not be answered anyway).
	pub fn evict_expired(&mut self) {
		let (now, ttl) = (Instant::now(), self.ttl);
		self.map.retain(|_, slot| now.saturating_duration_since(slot.at) < ttl);
	}

	#[cfg(test)]
	pub fn len(&self) -> usize {
		self.map.len()
	}
}

/// What became of a flight: its answer, or no answer because its leader gave up.
enum Outcome<V> {
	Running,
	Done(V),
	Abandoned,
}

struct Flight<V> {
	outcome: Mutex<Outcome<V>>,
	changed: Condvar,
}

/// Callers that ask for the same `key` while one of them is working share its answer: the first
/// does the work, the others wait for it. Asked again later, the work runs again (nothing is
/// remembered; that is what [`Cache`] is for).
pub struct Flights<K, V> {
	map: Mutex<HashMap<K, Arc<Flight<V>>>>,
}

impl<K, V> Default for Flights<K, V> {
	fn default() -> Self {
		Self { map: Mutex::new(HashMap::new()) }
	}
}

/// Ends a flight when the leader is done with it, or unwinds out of it.
struct Leader<'a, K: Hash + Eq, V> {
	flights: &'a Flights<K, V>,
	key:     &'a K,
	flight:  Arc<Flight<V>>,
	done:    bool,
}

impl<K: Hash + Eq, V> Drop for Leader<'_, K, V> {
	fn drop(&mut self) {
		self.flights.map.lock().unwrap_or_else(PoisonError::into_inner).remove(self.key);
		if !self.done {
			*self.flight.outcome.lock().unwrap_or_else(PoisonError::into_inner) = Outcome::Abandoned;
			self.flight.changed.notify_all();
		}
	}
}

impl<K: Hash + Eq + Clone, V: Clone> Flights<K, V> {
	/// The answer for `key` and whether it was another caller's (`true`: this one waited and
	/// `work` did not run).
	pub fn run(&self, key: &K, work: impl FnOnce() -> V) -> (V, bool) {
		let mut work = Some(work);
		loop {
			let (flight, leading) = {
				let mut map = self.map.lock().unwrap_or_else(PoisonError::into_inner);
				match map.get(key) {
					Some(flight) => (flight.clone(), false),
					None => {
						let flight = Arc::new(Flight { outcome: Mutex::new(Outcome::Running), changed: Condvar::new() });
						map.insert(key.clone(), flight.clone());
						(flight, true)
					},
				}
			};
			if leading {
				let Some(work) = work.take() else { continue };
				let mut leader = Leader { flights: self, key, flight, done: false };
				let value = work();
				*leader.flight.outcome.lock().unwrap_or_else(PoisonError::into_inner) = Outcome::Done(value.clone());
				leader.done = true;
				leader.flight.changed.notify_all();
				return (value, false);
			}
			let mut outcome = flight.outcome.lock().unwrap_or_else(PoisonError::into_inner);
			while matches!(*outcome, Outcome::Running) {
				outcome = flight.changed.wait(outcome).unwrap_or_else(PoisonError::into_inner);
			}
			if let Outcome::Done(value) = &*outcome {
				return (value.clone(), true);
			}
			// The leader gave up without an answer: lead.
		}
	}
}

#[cfg(test)]
mod tests {
	use std::{
		sync::atomic::{AtomicUsize, Ordering},
		thread,
	};

	use super::*;

	const TTL: Duration = Duration::from_secs(60);

	#[test]
	fn entries_expire_after_their_lifetime_but_stay_readable_as_stale() {
		let mut cache = Cache::new(4, TTL);
		let start = Instant::now();
		cache.insert_at("a", 1, start);
		assert_eq!(cache.get_at(&"a", start + Duration::from_secs(59)), Some(1));
		assert_eq!(cache.get_stale_at(&"a", start + Duration::from_secs(61)), Some((1, false)));
		assert_eq!(cache.get_at(&"a", start + Duration::from_secs(61)), None, "expired");
		assert_eq!(cache.get_stale_at(&"a", start + Duration::from_secs(61)), None, "and gone");
	}

	#[test]
	fn the_least_recently_used_entry_makes_room() {
		let mut cache = Cache::new(2, TTL);
		cache.insert("a", 1);
		cache.insert("b", 2);
		assert_eq!(cache.get(&"a"), Some(1), "a is used again, so b is the oldest");
		cache.insert("c", 3);
		assert_eq!((cache.get(&"a"), cache.get(&"b"), cache.get(&"c")), (Some(1), None, Some(3)));
		assert_eq!(cache.len(), 2);
	}

	#[test]
	fn inserting_again_replaces_the_value_and_restarts_the_lifetime() {
		let mut cache = Cache::new(2, TTL);
		let start = Instant::now();
		cache.insert_at("a", 1, start);
		cache.insert_at("a", 2, start + Duration::from_secs(50));
		assert_eq!(cache.get_at(&"a", start + Duration::from_secs(100)), Some(2));
		assert_eq!(cache.len(), 1);
	}

	#[test]
	#[expect(clippy::needless_collect, reason = "every thread must start before the first join")]
	fn callers_asking_together_share_one_run_of_the_work() {
		let flights = Arc::new(Flights::<&'static str, usize>::default());
		let runs = Arc::new(AtomicUsize::new(0));
		let handles: Vec<_> = (0..6)
			.map(|_| {
				let (flights, runs) = (flights.clone(), runs.clone());
				thread::spawn(move || {
					flights.run(&"k", || {
						thread::sleep(Duration::from_millis(150));
						runs.fetch_add(1, Ordering::SeqCst) + 100
					})
				})
			})
			.collect();
		let answers: Vec<(usize, bool)> = handles.into_iter().map(|h| h.join().unwrap()).collect();
		assert_eq!(runs.load(Ordering::SeqCst), 1, "one request for six callers");
		assert!(answers.iter().all(|(value, _)| *value == 100));
		assert_eq!(answers.iter().filter(|(_, shared)| !shared).count(), 1, "one leader");
	}

	#[test]
	fn a_waiter_takes_over_when_the_leader_fails_to_answer() {
		let flights = Arc::new(Flights::<u8, u8>::default());
		let leading = {
			let flights = flights.clone();
			thread::spawn(move || {
				std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
					flights.run(&1, || {
						thread::sleep(Duration::from_millis(200));
						panic!("the leader died");
					})
				}))
				.is_err()
			})
		};
		thread::sleep(Duration::from_millis(60));
		let (value, shared) = flights.run(&1, || 5);
		assert!(leading.join().unwrap(), "the leader panicked");
		assert_eq!((value, shared), (5, false), "the waiter did the work itself");
	}
}
