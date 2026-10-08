//! Optional names, artwork and queue rows never hold up transport commands or decoder events.

use std::{sync::Arc, time::Duration};

use futures_util::{StreamExt as _, stream};
use librespot_core::{SpotifyUri, session::Session};
use librespot_metadata::{Album, Artist, Episode, Metadata as _, Playlist, Track, image::Images};
use parking_lot::Mutex;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use super::{Cover, Decorations, Entity, Identity, PlaybackInfo, QueueEntry, Related, Transitions, VisualIdentity, named, uri_of};
use crate::net::cache::Cache;

const ENTITY_CACHE: usize = 256;
const ENTITY_TTL: Duration = Duration::from_secs(30 * 60);
const QUEUE_CONCURRENCY: usize = 8;

pub(super) enum Request {
	Track(String),
	Context(String),
	Queue { next: Vec<(String, String, String)>, revision: u64, generation: u64 },
}

pub(super) enum Ready {
	Entity(Entity),
	Queue { upcoming: Vec<QueueEntry>, revision: u64, generation: u64 },
}

struct Metadata {
	session:     Session,
	transitions: Arc<Transitions>,
	entities:    Mutex<Cache<String, Entity>>,
}

pub(super) fn spawn(session: Session, transitions: Arc<Transitions>) -> (UnboundedSender<Request>, UnboundedReceiver<Ready>) {
	let (tx, requests) = mpsc::unbounded_channel();
	let (results, rx) = mpsc::unbounded_channel();
	let metadata = Arc::new(Metadata { session, transitions, entities: Mutex::new(Cache::new(ENTITY_CACHE, ENTITY_TTL)) });
	tokio::spawn(metadata.run(requests, results));
	(tx, rx)
}

/// The decoder already fetched this track's display data before starting its audio.
/// Publish it by moving its fields, rather than doing another metadata round trip first.
pub(super) fn audio_entity(item: librespot_metadata::audio::AudioItem) -> Entity {
	use librespot_metadata::audio::UniqueFields;
	let (parent, creators) = match item.unique_fields {
		UniqueFields::Track { artists, album, .. } => {
			let creators =
				artists.0.into_iter().map(|artist| Related { entity: Box::new(named(uri_of(&artist.id), artist.name)) }).collect();
			(Some(Related { entity: Box::new(named(String::new(), album)) }), creators)
		},
		UniqueFields::Episode { show_name, .. } => {
			let parent = Some(Related { entity: Box::new(named(String::new(), show_name.clone())) });
			(parent, vec![Related { entity: Box::new(named(String::new(), show_name)) }])
		},
		UniqueFields::Local { artists, album, .. } => {
			let parent = album.map(|album| Related { entity: Box::new(named(String::new(), album)) });
			let creators = artists.into_iter().map(|artist| Related { entity: Box::new(named(String::new(), artist)) }).collect();
			(parent, creators)
		},
	};
	Entity {
		uri:         item.uri,
		decorations: Decorations {
			identity: Identity { name: item.name },
			visual_identity: VisualIdentity {
				cover: item
					.covers
					.into_iter()
					.map(|cover| Cover { url: cover.url, size: format!("{:?}", cover.size).to_ascii_lowercase() })
					.collect(),
			},
			parent,
			creators,
			playback: PlaybackInfo {
				duration_ms:     u64::from(item.duration_ms),
				content_ratings: if item.is_explicit { vec!["explicit".to_owned()] } else { Vec::new() },
			},
		},
	}
}

impl Metadata {
	async fn run(self: Arc<Self>, mut requests: UnboundedReceiver<Request>, results: UnboundedSender<Ready>) {
		let mut queue: Option<tokio::task::JoinHandle<()>> = None;
		while let Some(request) = requests.recv().await {
			let ready = match request {
				Request::Track(uri) => Ready::Entity(self.track(uri).await),
				Request::Context(uri) => Ready::Entity(self.context(uri).await),
				Request::Queue { next, revision, generation } => {
					if let Some(previous) = queue.take() {
						previous.abort();
					}
					let metadata = self.clone();
					let results = results.clone();
					queue = Some(tokio::spawn(async move {
						let upcoming = ordered_queue(next, |uri| metadata.track(uri)).await;
						let _ = results.send(Ready::Queue { upcoming, revision, generation });
					}));
					continue;
				},
			};
			if results.send(ready).is_err() {
				break;
			}
		}
		if let Some(queue) = queue {
			queue.abort();
		}
	}
	fn remember(&self, entity: Entity) {
		self.entities.lock().insert(entity.uri.clone(), entity);
	}

	/// A track or episode cached with bounded LRU eviction and expiry.
	async fn track(&self, uri: String) -> Entity {
		if let Some(entity) = self.entities.lock().get(&uri) {
			return entity;
		}
		let looked_up = super::network_timeout(super::NETWORK_TIMEOUT, async {
			match SpotifyUri::from_uri(&uri)? {
				id @ SpotifyUri::Track { .. } => Track::get(&self.session, &id).await.map(|track| self.track_entity(&track)),
				id @ SpotifyUri::Episode { .. } => Episode::get(&self.session, &id).await.map(|episode| self.episode_entity(&episode)),
				_ => Err(librespot_core::Error::invalid_argument("Not a playable Spotify URI")),
			}
		})
		.await;
		let Ok(entity) = looked_up else { return Entity { uri, ..Entity::default() } };
		self.remember(entity.clone());
		entity
	}

	fn track_entity(&self, track: &Track) -> Entity {
		let album = Entity {
			uri:         uri_of(&track.album.id),
			decorations: Decorations {
				identity: Identity { name: track.album.name.clone() },
				visual_identity: VisualIdentity { cover: self.covers(&track.album.covers) },
				..Decorations::default()
			},
		};
		let creators = track.artists.iter().map(|a| Related { entity: Box::new(named(uri_of(&a.id), a.name.clone())) }).collect();
		Entity {
			uri:         uri_of(&track.id),
			decorations: Decorations {
				identity: Identity { name: track.name.clone() },
				visual_identity: VisualIdentity::default(),
				parent: Some(Related { entity: Box::new(album) }),
				creators,
				playback: PlaybackInfo {
					duration_ms:     u64::try_from(track.duration).unwrap_or_default(),
					content_ratings: if track.is_explicit { vec!["explicit".to_owned()] } else { Vec::new() },
				},
			},
		}
	}

	fn episode_entity(&self, episode: &Episode) -> Entity {
		let show = Entity {
			decorations: Decorations {
				identity: Identity { name: episode.show_name.clone() },
				visual_identity: VisualIdentity { cover: self.covers(&episode.covers) },
				..Decorations::default()
			},
			..Entity::default()
		};
		Entity {
			uri:         uri_of(&episode.id),
			decorations: Decorations {
				identity:        Identity { name: episode.name.clone() },
				visual_identity: VisualIdentity::default(),
				parent:          Some(Related { entity: Box::new(show) }),
				creators:        vec![Related { entity: Box::new(named(String::new(), episode.show_name.clone())) }],
				playback:        PlaybackInfo {
					duration_ms:     u64::try_from(episode.duration).unwrap_or_default(),
					content_ratings: if episode.is_explicit { vec!["explicit".to_owned()] } else { Vec::new() },
				},
			},
		}
	}

	/// Cover URLs by the size names the UI picks from.
	fn covers(&self, images: &Images) -> Vec<Cover> {
		let base = self.session.get_user_attribute("image-url").unwrap_or_else(|| "https://i.scdn.co/image/{file_id}".to_owned());
		images
			.iter()
			.map(|i| Cover { url: base.replace("{file_id}", &i.id.to_string()), size: format!("{:?}", i.size).to_ascii_lowercase() })
			.collect()
	}

	/// What plays, by name: a playlist, an album, an artist, Liked Songs.
	async fn context(&self, uri: String) -> Entity {
		if !uri.starts_with("spotify:album:")
			&& let Some(entity) = self.entities.lock().get(&uri)
		{
			return entity;
		}
		let name = if uri.ends_with(":collection") {
			Some("Liked Songs".to_owned())
		} else {
			match SpotifyUri::from_uri(&uri) {
				Ok(id @ SpotifyUri::Playlist { .. }) => Playlist::get(&self.session, &id).await.ok().map(|p| p.name().to_owned()),
				Ok(id @ SpotifyUri::Album { .. }) => Album::get(&self.session, &id).await.ok().map(|a| {
					self.transitions.album(&a);
					a.name
				}),
				Ok(id @ SpotifyUri::Artist { .. }) => Artist::get(&self.session, &id).await.ok().map(|a| a.name),
				_ => None,
			}
		};
		let entity = named(uri, name.unwrap_or_default());
		if !entity.uri.is_empty() && !entity.name().is_empty() {
			self.remember(entity.clone());
		}
		entity
	}
}

async fn ordered_queue<F, Fut>(next: Vec<(String, String, String)>, lookup: F) -> Vec<QueueEntry>
where
	F: Fn(String) -> Fut,
	Fut: Future<Output = Entity>,
{
	stream::iter(next)
		.map(|(uid, uri, provider)| {
			let item = lookup(uri);
			async move {
				let source = match provider.as_str() {
					"queue" => "queue",
					"autoplay" => "autoplay",
					_ => "context",
				};
				QueueEntry { uid, source: source.to_owned(), item: item.await }
			}
		})
		.buffered(QUEUE_CONCURRENCY)
		.collect()
		.await
}

#[cfg(test)]
mod tests {
	use std::sync::atomic::{AtomicUsize, Ordering};

	use super::*;

	#[tokio::test]
	async fn queue_metadata_is_bounded_parallel_and_retains_playback_order() {
		let active = AtomicUsize::new(0);
		let maximum = AtomicUsize::new(0);
		let tracks = (0..24).map(|i| (i.to_string(), i.to_string(), "context".into())).collect();
		let rows = ordered_queue(tracks, |uri| {
			let active = &active;
			let maximum = &maximum;
			async move {
				let current = active.fetch_add(1, Ordering::Relaxed) + 1;
				maximum.fetch_max(current, Ordering::Relaxed);
				tokio::task::yield_now().await;
				active.fetch_sub(1, Ordering::Relaxed);
				named(uri.clone(), uri)
			}
		})
		.await;
		assert_eq!(maximum.load(Ordering::Relaxed), QUEUE_CONCURRENCY);
		assert_eq!(active.load(Ordering::Relaxed), 0);
		assert_eq!(
			rows.iter().map(|entry| &entry.uid).collect::<Vec<_>>(),
			(0..24).map(|i| i.to_string()).collect::<Vec<_>>().iter().collect::<Vec<_>>()
		);
		assert!(rows.iter().all(|entry| entry.source == "context" && entry.item.uri == entry.uid));
	}

	#[tokio::test]
	async fn foreground_context_metadata_is_delivered_ahead_of_queue_enrichment() -> Result<(), Box<dyn std::error::Error>> {
		let (requests, mut results) = spawn(Session::new(Default::default(), None), Arc::new(Transitions::default()));
		requests.send(Request::Queue {
			next:       (0..80).map(|i| (i.to_string(), format!("invalid:{i}"), "context".into())).collect(),
			revision:   1,
			generation: 1,
		})?;
		requests.send(Request::Context("spotify:user:account:collection".into()))?;
		let ready = tokio::time::timeout(Duration::from_secs(1), results.recv()).await?;
		assert!(matches!(ready, Some(Ready::Entity(entity)) if entity.name() == "Liked Songs"));
		Ok(())
	}

	#[tokio::test]
	async fn metadata_cache_evicts_only_cold_entries_and_expired_metadata_refreshes() {
		let metadata = Metadata {
			session:     Session::new(Default::default(), None),
			transitions: Arc::new(Transitions::default()),
			entities:    Mutex::new(Cache::new(ENTITY_CACHE, ENTITY_TTL)),
		};
		for i in 0..ENTITY_CACHE {
			metadata.remember(named(format!("track:{i}"), i.to_string()));
		}
		assert_eq!(metadata.track("track:0".into()).await.name(), "0");
		metadata.remember(named("track:new".into(), "new".into()));
		assert_eq!(metadata.track("track:0".into()).await.name(), "0", "inserting at capacity must not clear the whole cache");
		let mut cache = metadata.entities.lock();
		assert!(cache.get(&"track:1".to_owned()).is_none());
		let now = std::time::Instant::now();
		cache.insert_at("expired".into(), named("expired".into(), "stale".into()), now - ENTITY_TTL);
		assert!(cache.get_at(&"expired".to_owned(), now).is_none());
	}
}
