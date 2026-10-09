//! Background-decoded, physical-pixel-bucketed cover artwork.
//!
//! During a view's render, call `CoverCache::get(cover, logical_side, scale, cx)`
//! and render a hit with `img(ImageSource::Render(image)).size(px(logical_side))`.
//! A miss returns `None`; render the normal placeholder. All requesting views are
//! notified once decoding finishes, including views sharing the same artwork.
//! Decoding is limited to two in-flight tasks. JPEG uses reduced IDCT (1/2,
//! 1/4 or 1/8) before final resizing, so row artwork does not allocate full-size
//! JPEG pixel buffers. PNG/WebP lack that decode-time reduction, but share the
//! same concurrency limit. Decoder state and resize inputs are dropped before
//! BGRA conversion; the host's allocator controls when freed pages are purged.
//!
//! The default budget is 12 MiB of decoded BGRA pixels, not compressed source
//! bytes or GPU atlas overhead. Eviction also releases GPUI's atlas entry. Sources
//! are held weakly except while decoding; dead sources are swept on misses and
//! completions. Call `prune` after replacing a library to reclaim them immediately
//! without a polling task or idle CPU cost.

use std::{
	collections::{HashMap, VecDeque},
	sync::{Arc, Weak},
};

use gpui_kit::{AnyWeakEntity, App, BorrowAppContext as _, Context, Global, RenderImage};
use image::{DynamicImage, Frame, ImageFormat, RgbaImage};
use turntable_core::Cover;

const DEFAULT_BUDGET: usize = 12 * 1024 * 1024;
// Avoid the old 320→640 jump: a 164-logical-pixel shelf cover at 2x needs 328,
// not 640, physical pixels. Every bucket still covers the requested display size.
const BUCKETS: [u32; 9] = [64, 96, 128, 192, 256, 320, 384, 512, 640];
const MAX_DECODES: usize = 2;
/// A blurred backdrop is decoded this small, then blurred: stretched over a panel it reads as
/// soft colour, like Apple Music's lyrics.
const BLUR_SIDE: u32 = 96;
const BLUR_SIGMA: f32 = 10.;

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
	blur:   bool,
}

impl Key {
	fn new(cover: &Arc<Cover>, side: u32) -> Self {
		Self { source: Arc::as_ptr(cover) as usize, side, blur: false }
	}

	fn blurred(cover: &Arc<Cover>) -> Self {
		Self { source: Arc::as_ptr(cover) as usize, side: BLUR_SIDE, blur: true }
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
	queue:   VecDeque<Key>,
	running: usize,
}

impl Global for CoverCache {}

impl Default for CoverCache {
	fn default() -> Self {
		Self::new(DEFAULT_BUDGET)
	}
}

impl CoverCache {
	pub fn new(budget: usize) -> Self {
		Self { entries: HashMap::new(), budget, bytes: 0, clock: 0, queue: VecDeque::new(), running: 0 }
	}

	/// `size_px` is the desired logical side; `scale_factor` is the window scale
	/// cached by the host for this frame. Keep source Arc identities stable across
	/// snapshots. Decode/resize/BGRA conversion run off the UI thread.
	pub fn get<V: 'static>(cover: &Arc<Cover>, size_px: u32, scale_factor: f32, cx: &mut Context<'_, V>) -> Option<Arc<RenderImage>> {
		Self::lookup_or_queue(cover, Key::new(cover, bucket(size_px, scale_factor)), cx)
	}

	/// The cover small and heavily blurred, for a backdrop (cached like any size).
	pub fn blurred<V: 'static>(cover: &Arc<Cover>, cx: &mut Context<'_, V>) -> Option<Arc<RenderImage>> {
		Self::lookup_or_queue(cover, Key::blurred(cover), cx)
	}

	fn lookup_or_queue<V: 'static>(cover: &Arc<Cover>, key: Key, cx: &mut Context<'_, V>) -> Option<Arc<RenderImage>> {
		let requester = cx.entity().into_any().downgrade();
		if !cx.has_global::<Self>() {
			cx.set_global(Self::default());
		}
		let (image, start) = cx.update_global::<Self, _>(|cache, cx| {
			if cache.entries.contains_key(&key) {
				return (cache.lookup(key, requester), false);
			}
			// Rendering temporarily removes the current window from App's map.
			// Defer atlas release until it is restored, so every window is covered.
			cache.sweep(|image| cx.defer(move |cx| cx.drop_image(image, None)));
			cache.clock += 1;
			cache.entries.insert(key, Entry {
				source: Arc::downgrade(cover),
				state:  State::Pending(vec![requester]),
				bytes:  0,
				used:   cache.clock,
			});
			cache.queue.push_back(key);
			(None, true)
		});
		if start {
			Self::pump(cx);
		}
		image
	}

	// Queue on the foreground, not a blocking semaphore on background workers.
	// At most two tasks (including results awaiting foreground completion) own
	// decoding buffers. Queued entries hold only weak references to sources.
	fn pump(cx: &mut App) {
		loop {
			let next = cx.update_global::<Self, _>(|cache, _| cache.next_decode());
			let Some((key, source)) = next else { break };
			let work =
				cx.background_executor().spawn(async move { if key.blur { decode_blurred(&source) } else { decode(&source, key.side) } });
			// App-scoped: shared artwork must finish if its first window closes.
			cx.spawn(async move |cx| {
				let decoded = work.await;
				cx.update(|cx| {
					cx.update_global::<Self, _>(|cache, cx| {
						cache.running -= 1;
						let waiters = cache.complete(key, decoded, |image| cx.drop_image(image, None));
						for waiter in waiters {
							if waiter.is_upgradable() {
								cx.notify(waiter.entity_id());
							}
						}
					});
					Self::pump(cx);
				});
			})
			.detach();
		}
	}

	fn next_decode(&mut self) -> Option<(Key, Arc<Cover>)> {
		if self.running >= MAX_DECODES {
			return None;
		}
		while let Some(key) = self.queue.pop_front() {
			let Some(entry) = self.entries.get(&key) else { continue };
			if let Some(source) = entry.source.upgrade() {
				self.running += 1;
				return Some((key, source));
			}
			self.entries.remove(&key);
		}
		None
	}

	/// Reclaim artwork whose source Arcs were dropped, e.g. on library changes.
	pub fn prune(cx: &mut App) {
		if cx.has_global::<Self>() {
			cx.update_global::<Self, _>(|cache, cx| cache.sweep(|image| cx.defer(move |cx| cx.drop_image(image, None))));
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
	Some(render(decode_rgba(cover, side)?))
}

fn decode_blurred(cover: &Cover) -> Option<Arc<RenderImage>> {
	Some(render(image::imageops::blur(&decode_rgba(cover, BLUR_SIDE)?, BLUR_SIGMA)))
}

/// GPUI draws BGRA.
fn render(mut bgra: RgbaImage) -> Arc<RenderImage> {
	for pixel in bgra.pixels_mut() {
		pixel.0.swap(0, 2);
	}
	Arc::new(RenderImage::new([Frame::new(bgra)]))
}

fn decode_rgba(cover: &Cover, side: u32) -> Option<RgbaImage> {
	let format = ImageFormat::from_mime_type(&cover.mime)?;
	if !matches!(format, ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::WebP) {
		return None;
	}
	let resized = {
		let source = if format == ImageFormat::Jpeg {
			decode_jpeg(&cover.bytes, side)?
		} else {
			image::load_from_memory_with_format(&cover.bytes, format).ok()?
		};
		if source.width() > side || source.height() > side { source.thumbnail(side, side) } else { source }
	};
	Some(resized.into_rgba8())
}

fn decode_jpeg(bytes: &[u8], side: u32) -> Option<DynamicImage> {
	let (info, pixels) = {
		let mut decoder = jpeg_decoder::Decoder::new(std::io::Cursor::new(bytes));
		let side = u16::try_from(side).ok()?;
		decoder.scale(side, side).ok()?;
		let pixels = decoder.decode().ok()?;
		(decoder.info()?, pixels)
	};
	// Decoder coefficient/plane storage is gone before pixel conversion.
	let (width, height) = (u32::from(info.width), u32::from(info.height));
	match info.pixel_format {
		jpeg_decoder::PixelFormat::RGB24 => image::RgbImage::from_raw(width, height, pixels).map(DynamicImage::ImageRgb8),
		jpeg_decoder::PixelFormat::L8 => image::GrayImage::from_raw(width, height, pixels).map(DynamicImage::ImageLuma8),
		jpeg_decoder::PixelFormat::L16 => {
			let values = pixels.as_chunks::<2>().0.iter().map(|pair| u16::from_ne_bytes(*pair)).collect();
			image::ImageBuffer::from_raw(width, height, values).map(DynamicImage::ImageLuma16)
		},
		jpeg_decoder::PixelFormat::CMYK32 => {
			// jpeg-decoder normalizes Adobe's inverted samples to ordinary CMYK.
			// Reuse its four-channel allocation for RGBA rather than allocate RGB.
			let mut rgba = RgbaImage::from_raw(width, height, pixels)?;
			for pixel in rgba.pixels_mut() {
				let [c, m, y, k] = pixel.0;
				pixel.0 = [cmyk_channel(c, k), cmyk_channel(m, k), cmyk_channel(y, k), 255];
			}
			Some(DynamicImage::ImageRgba8(rgba))
		},
	}
}

#[expect(clippy::cast_possible_truncation, reason = "CMYK channel product divided by 255 is bounded to 0..=255")]
fn cmyk_channel(ink: u8, black: u8) -> u8 {
	(u16::from(255 - ink) * u16::from(255 - black) / 255) as u8
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
			(65, 1., 96),
			(40, 2., 96),
			(64, 2., 128),
			(65, 2., 192),
			(128, 2.5, 320),
			(129, 2.5, 384),
			(164, 2., 384),
			(232, 2., 512),
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
	fn cached_covers_return_the_same_pixels_without_decode_or_queue_work() {
		use gpui_kit::AppContext as _;

		let app = gpui_test::TestAppContext::single();
		app.update(|cx| {
			let source = source();
			let expected = image(64);
			let mut cache = CoverCache::default();
			let key = pending(&mut cache, &source, 64);
			cache.complete(key, Some(Arc::clone(&expected)), |_| {});
			cx.set_global(cache);
			let requester = cx.new(|_| ());
			requester.update(cx, |(), cx| {
				for _ in 0..3 {
					let hit = CoverCache::get(&source, 32, 2., cx);
					assert!(hit.is_some_and(|image| Arc::ptr_eq(&image, &expected)));
				}
			});
			let cache = cx.global::<CoverCache>();
			assert_eq!(cache.entries.len(), 1);
			assert_eq!(cache.running, 0);
			assert!(cache.queue.is_empty());
		});
	}

	#[test]
	fn default_cache_stays_bounded_across_page_working_sets() {
		let mut cache = CoverCache::default();
		let sources: Vec<_> = (0..40).map(|_| source()).collect();
		let mut released = 0;
		for source in &sources {
			let key = pending(&mut cache, source, 640);
			cache.complete(key, Some(image(640)), |_| released += 1);
			assert!(cache.bytes <= 12 * 1024 * 1024);
		}
		assert!(released > 0);
		assert_eq!(cache.entries.len() + released, sources.len());
	}

	#[test]
	fn only_two_decodes_can_be_in_flight() {
		let mut cache = CoverCache::default();
		let sources = [source(), source(), source()];
		for source in &sources {
			let key = pending(&mut cache, source, 64);
			cache.queue.push_back(key);
		}
		let first = cache.next_decode();
		let second = cache.next_decode();
		assert!(first.is_some() && second.is_some());
		assert_eq!(cache.running, MAX_DECODES);
		assert!(cache.next_decode().is_none());
		cache.running -= 1;
		assert!(cache.next_decode().is_some());
		assert_eq!(cache.running, MAX_DECODES);
	}

	#[test]
	fn queued_decodes_do_not_retain_dropped_sources() {
		let mut cache = CoverCache::default();
		let dropped = source();
		let key = pending(&mut cache, &dropped, 64);
		cache.queue.push_back(key);
		assert_eq!(Arc::strong_count(&dropped), 1);
		drop(dropped);
		assert!(cache.next_decode().is_none());
		assert_eq!(cache.running, 0);
		assert!(cache.entries.is_empty());
	}

	#[test]
	fn jpeg_pixel_buffers_are_reduced_before_resizing() -> Result<(), image::ImageError> {
		let source = image::RgbImage::from_pixel(640, 640, image::Rgb([210, 80, 30]));
		let mut bytes = std::io::Cursor::new(Vec::new());
		source.write_to(&mut bytes, ImageFormat::Jpeg)?;
		for (requested, intermediate) in [(64, 80), (128, 160), (320, 320), (640, 640)] {
			let decoded = decode_jpeg(bytes.get_ref(), requested);
			assert!(decoded.is_some());
			if let Some(decoded) = decoded {
				assert_eq!(decoded.width(), intermediate);
				assert_eq!(decoded.height(), intermediate);
				assert_eq!(decoded.as_bytes().len(), (intermediate * intermediate * 3) as usize);
			}
		}
		Ok(())
	}

	#[test]
	fn cmyk_channels_use_subtractive_black() {
		assert_eq!(cmyk_channel(0, 0), 255);
		assert_eq!(cmyk_channel(255, 0), 0);
		assert_eq!(cmyk_channel(0, 255), 0);
		assert_eq!(cmyk_channel(128, 128), 63);
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
			DynamicImage::ImageRgba8(source).into_rgb8().write_to(&mut bytes, format)?;
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
