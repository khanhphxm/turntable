//! Background-decoded, physical-pixel-bucketed cover artwork.
//!
//! During a view's render, call `CoverCache::get(cover, logical_side, window, cx)`
//! and render a hit with `img(ImageSource::Render(image)).size(px(logical_side))`.
//! A miss returns `None`; render the normal placeholder. All requesting views are
//! notified once decoding finishes, including views sharing the same artwork.
//!
//! The default budget is 24 MiB of decoded BGRA pixels, not compressed source
//! bytes or GPU atlas overhead. Eviction also releases GPUI's atlas entry. Sources
//! are held weakly except while decoding; dead sources are swept on misses and
//! completions. Call `prune` after replacing a library to reclaim them immediately
//! without a polling task or idle CPU cost.

use std::{
	collections::HashMap,
	sync::{Arc, Weak},
};

use gpui_kit::{AnyWeakEntity, App, BorrowAppContext as _, Context, Global, RenderImage, Window};
use image::{Frame, ImageFormat, RgbaImage};

/// Temporary stand-in: replace this definition with `use turntable_core::Cover`
/// when the core's host-facing artwork type is available. Arc identity must stay
/// stable across snapshots; mutating artwork requires a new Arc.
#[derive(Debug)]
pub struct Cover {
	pub bytes: Vec<u8>,
	pub mime:  String,
}

const DEFAULT_BUDGET: usize = 24 * 1024 * 1024;
const BUCKETS: [u32; 4] = [64, 128, 320, 640];

/// Select the smallest physical-pixel bucket that covers the logical side.
/// Large requests are capped at 640; small source images are never enlarged.
fn bucket(size_px: u32, scale_factor: f32) -> u32 {
	let physical = (f64::from(size_px) * f64::from(scale_factor)).ceil();
	BUCKETS.into_iter().find(|&side| f64::from(side) >= physical).unwrap_or(640)
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct Key {
	source: usize,
	side:   u32,
}

impl Key {
	fn new(cover: &Arc<Cover>, side: u32) -> Self {
		Self { source: Arc::as_ptr(cover) as usize, side }
	}
}

enum State {
	Pending(Vec<AnyWeakEntity>),
	Ready(Arc<RenderImage>),
	// Negative cache: a corrupt source should not decode again on every render.
	Failed,
}

struct Entry {
	source: Weak<Cover>,
	state:  State,
	bytes:  usize,
	used:   u64,
}

/// App-global LRU cache. `get` installs the default cache on first use.
/// Install `CoverCache::new(bytes)` with `cx.set_global` to override the budget.
/// The budget bounds cache-owned decoded buffers; a caller retaining a returned
/// Arc can keep that buffer alive after eviction.
pub struct CoverCache {
	entries: HashMap<Key, Entry>,
	budget:  usize,
	bytes:   usize,
	clock:   u64,
}

impl Global for CoverCache {}

impl Default for CoverCache {
	fn default() -> Self {
		Self::new(DEFAULT_BUDGET)
	}
}

impl CoverCache {
	pub fn new(budget: usize) -> Self {
		Self { entries: HashMap::new(), budget, bytes: 0, clock: 0 }
	}

	/// `size_px` is the desired logical side; the window's scale factor selects
	/// a physical bucket. Decode/resize/BGRA conversion run off the UI thread.
	pub fn get<V: 'static>(cover: &Arc<Cover>, size_px: u32, window: &mut Window, cx: &mut Context<'_, V>) -> Option<Arc<RenderImage>> {
		let key = Key::new(cover, bucket(size_px, window.scale_factor()));
		let requester = cx.entity().into_any().downgrade();
		if !cx.has_global::<Self>() {
			cx.set_global(Self::default());
		}
		let (image, start) = cx.update_global::<Self, _>(|cache, cx| {
			if cache.entries.contains_key(&key) {
				return (cache.lookup(key, requester), false);
			}
			cache.sweep(|image| cx.drop_image(image, Some(window)));
			cache.clock += 1;
			cache.entries.insert(key, Entry {
				source: Arc::downgrade(cover),
				state:  State::Pending(vec![requester]),
				bytes:  0,
				used:   cache.clock,
			});
			(None, true)
		});
		if start {
			let source = Arc::clone(cover);
			let work = cx.background_executor().spawn(async move { decode(&source, key.side) });
			// App-scoped rather than window-scoped: a shared decode must still
			// complete if its first requesting window closes.
			App::spawn(cx, async move |cx| {
				let decoded = work.await;
				cx.update_global::<Self, _>(|cache, cx| {
					let waiters = cache.complete(key, decoded, |image| cx.drop_image(image, None));
					for waiter in waiters {
						if waiter.is_upgradable() {
							cx.notify(waiter.entity_id());
						}
					}
				});
			})
			.detach();
		}
		image
	}

	/// Reclaim artwork whose source Arcs were dropped, e.g. on library changes.
	pub fn prune(window: &mut Window, cx: &mut App) {
		if cx.has_global::<Self>() {
			cx.update_global::<Self, _>(|cache, cx| cache.sweep(|image| cx.drop_image(image, Some(window))));
		}
	}

	fn lookup(&mut self, key: Key, requester: AnyWeakEntity) -> Option<Arc<RenderImage>> {
		self.clock += 1;
		let entry = self.entries.get_mut(&key)?;
		entry.used = self.clock;
		match &mut entry.state {
			State::Ready(image) => Some(Arc::clone(image)),
			State::Pending(waiters) => {
				if !waiters.iter().any(|waiter| waiter.entity_id() == requester.entity_id()) {
					waiters.push(requester);
				}
				None
			},
			State::Failed => None,
		}
	}

	fn complete(&mut self, key: Key, decoded: Option<Arc<RenderImage>>, mut release: impl FnMut(Arc<RenderImage>)) -> Vec<AnyWeakEntity> {
		let Some(entry) = self.entries.get_mut(&key) else {
			return Vec::new();
		};
		let State::Pending(waiters) = std::mem::replace(&mut entry.state, State::Failed) else {
			return Vec::new();
		};
		if let Some(image) = decoded {
			let bytes = image.as_bytes(0).map_or(0, <[u8]>::len);
			if bytes <= self.budget {
				entry.bytes = bytes;
				self.bytes += bytes;
				entry.state = State::Ready(image);
			}
			// A bucket larger than the entire budget cannot ever be cached.
			// Leave it negative-cached rather than decode/notify on every frame.
		}
		self.sweep(&mut release);
		while self.bytes > self.budget {
			let oldest = self.entries.iter().filter(|(_, entry)| entry.bytes > 0).min_by_key(|(_, entry)| entry.used).map(|(&key, _)| key);
			let Some(oldest) = oldest else { break };
			self.remove(oldest, &mut release);
		}
		waiters
	}

	fn sweep(&mut self, mut release: impl FnMut(Arc<RenderImage>)) {
		let bytes = &mut self.bytes;
		self.entries.retain(|_, entry| {
			if entry.source.strong_count() != 0 {
				return true;
			}
			*bytes -= entry.bytes;
			if let State::Ready(image) = &entry.state {
				release(Arc::clone(image));
			}
			false
		});
	}

	fn remove(&mut self, key: Key, mut release: impl FnMut(Arc<RenderImage>)) {
		if let Some(entry) = self.entries.remove(&key) {
			self.bytes -= entry.bytes;
			if let State::Ready(image) = entry.state {
				release(image);
			}
		}
	}
}

fn decode(cover: &Cover, side: u32) -> Option<Arc<RenderImage>> {
	let format = ImageFormat::from_mime_type(&cover.mime)?;
	if !matches!(format, ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::WebP) {
		return None;
	}
	let source = image::load_from_memory_with_format(&cover.bytes, format).ok()?;
	let resized = if source.width() > side || source.height() > side { source.thumbnail(side, side) } else { source };
	let mut bgra: RgbaImage = resized.into_rgba8();
	for pixel in bgra.pixels_mut() {
		pixel.0.swap(0, 2);
	}
	Some(Arc::new(RenderImage::new([Frame::new(bgra)])))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn buckets_use_physical_pixels_and_cap_large_requests() {
		for (size, scale, expected) in [
			(0, 1., 64),
			(40, 1., 64),
			(64, 1., 64),
			(65, 1., 128),
			(40, 2., 128),
			(64, 2., 128),
			(65, 2., 320),
			(128, 2.5, 320),
			(129, 2.5, 640),
			(320, 2., 640),
			(641, 1., 640),
			(u32::MAX, 3., 640),
		] {
			assert_eq!(bucket(size, scale), expected);
		}
	}

	fn source() -> Arc<Cover> {
		Arc::new(Cover { bytes: Vec::new(), mime: "image/jpeg".into() })
	}

	fn pending(cache: &mut CoverCache, source: &Arc<Cover>, side: u32) -> Key {
		cache.clock += 1;
		let key = Key::new(source, side);
		cache.entries.insert(key, Entry {
			source: Arc::downgrade(source),
			state:  State::Pending(Vec::new()),
			bytes:  0,
			used:   cache.clock,
		});
		key
	}

	fn image(side: u32) -> Arc<RenderImage> {
		Arc::new(RenderImage::new([Frame::new(RgbaImage::new(side, side))]))
	}

	#[test]
	fn eviction_uses_decoded_bytes_and_least_recent_access() {
		let mut cache = CoverCache::new(2 * 64 * 64 * 4);
		let sources = [source(), source(), source()];
		let first = pending(&mut cache, &sources[0], 64);
		cache.complete(first, Some(image(64)), |_| {});
		let second = pending(&mut cache, &sources[1], 64);
		cache.complete(second, Some(image(64)), |_| {});
		// Same touch performed by lookup, without needing a live GPUI entity.
		cache.clock += 1;
		if let Some(entry) = cache.entries.get_mut(&first) {
			entry.used = cache.clock;
		}
		let third = pending(&mut cache, &sources[2], 64);
		let mut released = 0;
		cache.complete(third, Some(image(64)), |_| released += 1);
		assert!(cache.entries.contains_key(&first));
		assert!(!cache.entries.contains_key(&second));
		assert!(cache.entries.contains_key(&third));
		assert_eq!(cache.bytes, cache.budget);
		assert_eq!(released, 1);
	}

	#[test]
	fn dropped_sources_release_every_bucket() {
		let mut cache = CoverCache::default();
		let source = source();
		for side in [64, 128] {
			let key = pending(&mut cache, &source, side);
			cache.complete(key, Some(image(side)), |_| {});
		}
		assert_eq!(Arc::strong_count(&source), 1);
		drop(source);
		let mut released = 0;
		cache.sweep(|_| released += 1);
		assert!(cache.entries.is_empty());
		assert_eq!(cache.bytes, 0);
		assert_eq!(released, 2);
	}

	#[test]
	fn over_budget_image_is_not_retained() {
		let mut cache = CoverCache::new(64 * 64 * 4 - 1);
		let source = source();
		let key = pending(&mut cache, &source, 64);
		cache.complete(key, Some(image(64)), |_| {});
		assert_eq!(cache.bytes, 0);
		assert!(matches!(cache.entries.get(&key).map(|entry| &entry.state), Some(State::Failed)));
	}

	#[test]
	fn codecs_resize_preserve_aspect_and_convert_bgra() -> Result<(), image::ImageError> {
		for format in [ImageFormat::Jpeg, ImageFormat::Png, ImageFormat::WebP] {
			let source = RgbaImage::from_pixel(640, 320, image::Rgba([210, 80, 30, 255]));
			let mut bytes = std::io::Cursor::new(Vec::new());
			image::DynamicImage::ImageRgba8(source).into_rgb8().write_to(&mut bytes, format)?;
			let cover = Cover { bytes: bytes.into_inner(), mime: format.to_mime_type().into() };
			let decoded = decode(&cover, 64);
			assert!(decoded.is_some());
			if let Some(decoded) = decoded {
				assert_eq!(decoded.size(0).width.0, 64);
				assert_eq!(decoded.size(0).height.0, 32);
				if let Some(bytes) = decoded.as_bytes(0) {
					assert!(bytes[0].abs_diff(30) < 4);
					assert!(bytes[2].abs_diff(210) < 4);
					assert_eq!(bytes[3], 255);
				}
			}
		}
		Ok(())
	}

	#[test]
	fn invalid_artwork_is_negative_cached() {
		let mut cache = CoverCache::default();
		let source = source();
		assert!(decode(&source, 64).is_none());
		let key = pending(&mut cache, &source, 64);
		cache.complete(key, None, |_| {});
		assert!(matches!(cache.entries.get(&key).map(|entry| &entry.state), Some(State::Failed)));
		assert_eq!(cache.bytes, 0);
	}
}
