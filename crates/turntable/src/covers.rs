//! Background-decoded, physical-pixel-bucketed cover artwork.
//!
//! During a view's render, call `CoverCache::get(url, cover, logical_side, scale, cx)`
//! and render a hit with `img(ImageSource::Render(image)).size(px(logical_side))`.
//! A miss returns `None`; render the normal placeholder. All requesting views are
//! notified once decoding finishes, including views sharing the same artwork.
//! Decoding is limited to two in-flight tasks. JPEG uses reduced IDCT (1/2,
//! 1/4 or 1/8) before final resizing, so row artwork does not allocate full-size
//! JPEG pixel buffers. PNG/WebP lack that decode-time reduction, but share the
//! same concurrency limit. Decoder state and resize inputs are dropped before
//! BGRA conversion; the host's allocator controls when freed pages are purged.
//!
//! Active views share a 12 MiB decoded-pixel budget. Cache boundaries retain their
//! leases while GPUI replays a cached subtree; virtual lists refresh theirs when
//! laying out visible rows. At the frame boundary, inactive buckets release their
//! atlas entries and become at-most-96 px warm previews. A separate 4 MiB budget
//! includes these previews and their encoded sources, keeping recent Back visits
//! instant without retaining full-size navigation history. Reclamation is deferred
//! until rendering restores the window; no timer or idle polling is needed.
//! A smaller request reuses the smallest already-cached larger bucket for the
//! same source, avoiding another decoded buffer and GPU upload. Blurred and
//! unblurred artwork never share pixels; a smaller bucket never serves a larger
//! request, so display resolution is preserved.

use std::{
	collections::{HashMap, VecDeque},
	sync::{Arc, Weak},
};

use gpui_kit::{AnyWeakEntity, App, BorrowAppContext as _, Context, Global, RenderImage};
use image::{DynamicImage, Frame, ImageFormat, RgbaImage};
use turntable_core::Cover;

const DEFAULT_BUDGET: usize = 12 * 1024 * 1024;
const WARM_BUDGET: usize = 4 * 1024 * 1024;
const WARM_SIDE: u32 = 96;
pub(crate) const ROOT: u16 = 1 << 7;
pub(crate) const LIBRARY_ROWS: u16 = 1 << 8;
pub(crate) const LISTING_ROWS: u16 = 1 << 9;
pub(crate) const MINIPLAYER: u16 = 1 << 10;
// Match row thumbnails, 164 px shelf cards, 232/240 px headers/full view,
// and the default 248 px right panel at both 1x and 2x. Intermediate buckets
// still cover fractional scales and user-resized panels without undersampling.
const BUCKETS: [u32; 24] =
	[40, 48, 56, 60, 64, 80, 96, 112, 120, 128, 164, 192, 232, 240, 248, 256, 320, 328, 384, 464, 480, 496, 512, 640];
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
	Decoding(Vec<AnyWeakEntity>),
	// A small preview is available immediately while the full bucket is restored.
	Warm(Arc<RenderImage>),
	Ready(Arc<RenderImage>),
	// Negative cache: a corrupt source should not decode again on every render.
	Failed,
}

struct Entry {
	source:      Weak<Cover>,
	warm_source: Option<Arc<Cover>>,
	fallback:    Option<Arc<RenderImage>>,
	state:       State,
	scopes:      u16,
	bytes:       usize,
	used:        u64,
}

struct Source {
	cover:  Weak<Cover>,
	scopes: u16,
}

/// App-global LRU cache. `get` installs the default cache on first use.
/// Install `CoverCache::new(bytes)` with `cx.set_global` to override the budget.
/// The budget bounds cache-owned decoded buffers; a caller retaining a returned
/// Arc can keep that buffer alive after eviction.
pub struct CoverCache {
	entries:           HashMap<Key, Entry>,
	sources:           HashMap<String, Source>,
	scope:             u16,
	batches:           u16,
	reclaim_scheduled: bool,
	budget:            usize,
	bytes:             usize,
	clock:             u64,
	queue:             VecDeque<Key>,
	running:           usize,
}

impl Global for CoverCache {}

impl Default for CoverCache {
	fn default() -> Self {
		Self::new(DEFAULT_BUDGET)
	}
}

impl CoverCache {
	pub fn new(budget: usize) -> Self {
		Self {
			entries: HashMap::new(),
			sources: HashMap::new(),
			scope: ROOT,
			batches: 0,
			reclaim_scheduled: false,
			budget,
			bytes: 0,
			clock: 0,
			queue: VecDeque::new(),
			running: 0,
		}
	}

	/// Preserve stable source identity for warm visits without asking the core to
	/// fetch an already-retained source again.
	pub fn source(url: &str, cx: &App) -> Option<Arc<Cover>> {
		cx.try_global::<Self>()?.sources.get(url)?.cover.upgrade()
	}

	pub fn visible_source(url: &str, cx: &App) -> bool {
		cx.try_global::<Self>().and_then(|cache| cache.sources.get(url)).is_some_and(|source| source.scopes != 0)
	}

	pub fn reset(scopes: u16, cx: &mut App) {
		if cx.has_global::<Self>() {
			cx.update_global::<Self, _>(|cache, _| cache.reset_scopes(scopes));
		}
	}

	fn reset_scopes(&mut self, scopes: u16) {
		for entry in self.entries.values_mut() {
			entry.scopes &= !scopes;
		}
		for source in self.sources.values_mut() {
			source.scopes &= !scopes;
		}
	}

	pub fn enter(scope: u16, reset: bool, cx: &mut App) -> u16 {
		if !cx.has_global::<Self>() {
			cx.set_global(Self::default());
		}
		cx.update_global::<Self, _>(|cache, _| {
			if reset {
				cache.reset_scopes(scope);
			}
			std::mem::replace(&mut cache.scope, scope)
		})
	}

	pub fn leave(previous: u16, cx: &mut App) {
		cx.update_global::<Self, _>(|cache, _| cache.scope = previous);
	}

	/// GPUI renders every visible list item each layout, but invokes its processor
	/// separately for each row. Reset once for that batch, not for each row.
	pub fn first_in_batch(scope: u16, cx: &mut App) -> bool {
		if !cx.has_global::<Self>() {
			cx.set_global(Self::default());
		}
		cx.update_global::<Self, _>(|cache, _| {
			let first = cache.batches & scope == 0;
			cache.batches |= scope;
			first
		})
	}

	pub fn schedule_reclaim(cx: &mut App) -> bool {
		if !cx.has_global::<Self>() {
			return false;
		}
		cx.update_global::<Self, _>(|cache, _| !std::mem::replace(&mut cache.reclaim_scheduled, true))
	}

	pub fn reclaim(cx: &mut App) {
		if cx.has_global::<Self>() {
			cx.update_global::<Self, _>(|cache, cx| {
				cache.reclaim_scheduled = false;
				cache.batches = 0;
				cache.retire(|image| cx.drop_image(image, None));
			});
		}
	}

	/// Signing out must release warm sources too, not just dead weak references.
	pub fn clear(cx: &mut App) {
		if cx.has_global::<Self>() {
			cx.update_global::<Self, _>(|cache, cx| {
				for (_, entry) in cache.entries.drain() {
					if let State::Ready(image) | State::Warm(image) = entry.state {
						cx.defer(move |cx| cx.drop_image(image, None));
					}
					if let Some(image) = entry.fallback {
						cx.defer(move |cx| cx.drop_image(image, None));
					}
				}
				cache.sources.clear();
				cache.queue.clear();
				cache.bytes = 0;
			});
		}
	}

	fn retire(&mut self, mut release: impl FnMut(Arc<RenderImage>)) {
		let bytes = &mut self.bytes;
		self.entries.retain(|_, entry| {
			if entry.scopes != 0 || matches!(entry.state, State::Warm(_) | State::Decoding(_)) {
				return true;
			}
			*bytes -= entry.bytes;
			entry.bytes = 0;
			let State::Ready(image) = &entry.state else {
				if let Some(image) = entry.fallback.take() {
					release(image);
				}
				return false;
			};
			let warm = entry.source.upgrade().and_then(|source| preview(image).map(|image| (source, image)));
			release(Arc::clone(image));
			let Some((source, image)) = warm else { return false };
			entry.bytes = image.as_bytes(0).map_or(0, <[u8]>::len);
			*bytes += entry.bytes;
			entry.warm_source = Some(source);
			entry.state = State::Warm(image);
			true
		});
		let mut warm_bytes: usize =
			self.entries.values().filter(|entry| matches!(entry.state, State::Warm(_))).map(Entry::warm_bytes).sum();
		while warm_bytes > WARM_BUDGET {
			let oldest = self
				.entries
				.iter()
				.filter(|(_, entry)| matches!(entry.state, State::Warm(_)))
				.min_by_key(|(_, entry)| entry.used)
				.map(|(&key, entry)| (key, entry.warm_bytes()));
			let Some((key, bytes)) = oldest else { break };
			self.remove(key, &mut release);
			warm_bytes -= bytes;
		}
		self.sweep(release);
	}

	/// `size_px` is the desired logical side; `scale_factor` is the window scale
	/// cached by the host for this frame. Keep source Arc identities stable across
	/// snapshots. Decode/resize/BGRA conversion run off the UI thread.
	pub fn get<V: 'static>(
		url: &str,
		cover: &Arc<Cover>,
		size_px: u32,
		scale_factor: f32,
		cx: &mut Context<'_, V>,
	) -> Option<Arc<RenderImage>> {
		Self::lookup_or_queue(url, cover, Key::new(cover, bucket(size_px, scale_factor)), cx)
	}

	/// The cover small and heavily blurred, for a backdrop (cached like any size).
	pub fn blurred<V: 'static>(url: &str, cover: &Arc<Cover>, cx: &mut Context<'_, V>) -> Option<Arc<RenderImage>> {
		Self::lookup_or_queue(url, cover, Key::blurred(cover), cx)
	}

	fn lookup_or_queue<V: 'static>(url: &str, cover: &Arc<Cover>, key: Key, cx: &mut Context<'_, V>) -> Option<Arc<RenderImage>> {
		let requester = cx.entity().into_any().downgrade();
		if !cx.has_global::<Self>() {
			cx.set_global(Self::default());
		}
		let (image, start) = cx.update_global::<Self, _>(|cache, cx| {
			if let Some(source) = cache.sources.get_mut(url).filter(|source| source.cover.as_ptr() == Arc::as_ptr(cover)) {
				source.scopes |= cache.scope;
			} else {
				cache.sources.insert(url.to_owned(), Source { cover: Arc::downgrade(cover), scopes: cache.scope });
			}
			if let Some(cached_key) = cache.cached_key(key) {
				let warm = cache.entries.get(&cached_key).is_some_and(|entry| matches!(entry.state, State::Warm(_)));
				let image = cache.lookup(cached_key, requester);
				return (image, warm);
			}
			// Rendering temporarily removes the current window from App's map.
			// Defer atlas release until it is restored, so every window is covered.
			cache.sweep(|image| cx.defer(move |cx| cx.drop_image(image, None)));
			cache.clock += 1;
			cache.entries.insert(key, Entry {
				source:      Arc::downgrade(cover),
				warm_source: None,
				fallback:    None,
				state:       State::Pending(vec![requester]),
				scopes:      cache.scope,
				bytes:       0,
				used:        cache.clock,
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
			// A retired/cleared entry may disappear before completion. Keep its Arc
			// allocation identity reserved until the foreground consumes the result.
			let source_guard = Arc::downgrade(&source);
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
				drop(source_guard);
			})
			.detach();
		}
	}

	fn next_decode(&mut self) -> Option<(Key, Arc<Cover>)> {
		if self.running >= MAX_DECODES {
			return None;
		}
		while let Some(key) = self.queue.pop_front() {
			let Some(entry) = self.entries.get_mut(&key) else { continue };
			let State::Pending(waiters) = &mut entry.state else { continue };
			if let Some(source) = entry.source.upgrade() {
				entry.state = State::Decoding(std::mem::take(waiters));
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

	fn cached_key(&self, key: Key) -> Option<Key> {
		if self.entries.contains_key(&key) {
			return Some(key);
		}
		if key.blur {
			return None;
		}
		BUCKETS
			.into_iter()
			.filter(|&side| side > key.side)
			.map(|side| Key { side, ..key })
			.find(|larger| self.entries.get(larger).is_some_and(|entry| matches!(entry.state, State::Ready(_))))
	}

	fn lookup(&mut self, key: Key, requester: AnyWeakEntity) -> Option<Arc<RenderImage>> {
		self.clock += 1;
		let entry = self.entries.get_mut(&key)?;
		entry.used = self.clock;
		entry.scopes |= self.scope;
		if matches!(entry.state, State::Warm(_)) {
			if let State::Warm(image) = std::mem::replace(&mut entry.state, State::Pending(Vec::new())) {
				entry.fallback = Some(image);
			}
			self.queue.push_back(key);
		}
		match &mut entry.state {
			State::Ready(image) | State::Warm(image) => Some(Arc::clone(image)),
			State::Pending(waiters) | State::Decoding(waiters) => {
				if !waiters.iter().any(|waiter| waiter.entity_id() == requester.entity_id()) {
					waiters.push(requester);
				}
				entry.fallback.clone()
			},
			State::Failed => None,
		}
	}

	fn complete(&mut self, key: Key, decoded: Option<Arc<RenderImage>>, mut release: impl FnMut(Arc<RenderImage>)) -> Vec<AnyWeakEntity> {
		let Some(entry) = self.entries.get_mut(&key) else {
			return Vec::new();
		};
		let waiters = match &mut entry.state {
			State::Pending(waiters) | State::Decoding(waiters) => std::mem::take(waiters),
			_ => return Vec::new(),
		};
		entry.state = State::Failed;
		self.bytes -= entry.bytes;
		entry.bytes = 0;
		if let Some(fallback) = entry.fallback.take() {
			release(fallback);
		}
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
			if let State::Ready(image) | State::Warm(image) = &entry.state {
				release(Arc::clone(image));
			}
			if let Some(fallback) = &entry.fallback {
				release(Arc::clone(fallback));
			}
			false
		});
		self.sources.retain(|_, source| source.cover.strong_count() != 0);
	}

	fn remove(&mut self, key: Key, mut release: impl FnMut(Arc<RenderImage>)) {
		if let Some(entry) = self.entries.remove(&key) {
			self.bytes -= entry.bytes;
			if let State::Ready(image) | State::Warm(image) = entry.state {
				release(image);
			}
			if let Some(fallback) = entry.fallback {
				release(fallback);
			}
		}
	}
}

impl Entry {
	fn warm_bytes(&self) -> usize {
		// Counting a shared encoded source in each bucket is conservative and keeps
		// the entire warm working set bounded without a second ownership registry.
		self.bytes + self.warm_source.as_ref().map_or(0, |cover| cover.bytes.capacity())
	}
}

fn preview(image: &Arc<RenderImage>) -> Option<Arc<RenderImage>> {
	let size = image.size(0);
	let width = u32::try_from(size.width.0).ok()?;
	let height = u32::try_from(size.height.0).ok()?;
	if width <= WARM_SIDE && height <= WARM_SIDE {
		return Some(Arc::clone(image));
	}
	// RenderImage is already BGRA. Borrow its pixels; do not copy the full bucket
	// or swap channels a second time when producing the small preview.
	let pixels = image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(width, height, image.as_bytes(0)?)?;
	let longest = u64::from(width.max(height));
	let scaled = |side: u32| u32::try_from(((u64::from(side) * u64::from(WARM_SIDE) + longest / 2) / longest).max(1)).ok();
	Some(Arc::new(RenderImage::new([Frame::new(image::imageops::thumbnail(&pixels, scaled(width)?, scaled(height)?))])))
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
			(0, 1., 40),
			(40, 1., 40),
			(64, 1., 64),
			(65, 1., 80),
			(40, 2., 80),
			(64, 2., 128),
			(65, 2., 164),
			(128, 2.5, 320),
			(129, 2.5, 328),
			(164, 1.25, 232),
			(164, 2., 328),
			(232, 2., 464),
			(320, 2., 640),
			(641, 1., 640),
			(u32::MAX, 3., 640),
		] {
			assert_eq!(bucket(size, scale), expected);
		}
	}

	#[test]
	fn actual_display_sizes_are_exact_at_one_and_two_x() {
		for logical in [40, 48, 56, 60, 64, 164, 232, 240, 248, 320] {
			assert_eq!(bucket(logical, 1.), logical);
			assert_eq!(bucket(logical, 2.), logical * 2);
		}
		for pair in BUCKETS.windows(2) {
			assert!(pair[0] < pair[1]);
		}
	}

	fn source() -> Arc<Cover> {
		Arc::new(Cover { bytes: Vec::new(), mime: "image/jpeg".into() })
	}

	fn pending(cache: &mut CoverCache, source: &Arc<Cover>, side: u32) -> Key {
		cache.clock += 1;
		let key = Key::new(source, side);
		cache.entries.insert(key, Entry {
			source:      Arc::downgrade(source),
			warm_source: None,
			fallback:    None,
			state:       State::Pending(Vec::new()),
			scopes:      cache.scope,
			bytes:       0,
			used:        cache.clock,
		});
		key
	}

	fn image(side: u32) -> Arc<RenderImage> {
		Arc::new(RenderImage::new([Frame::new(RgbaImage::new(side, side))]))
	}

	#[test]
	fn retirement_cancels_queued_work_but_keeps_running_decodes_unique() {
		let mut cache = CoverCache::default();
		let sources = [source(), source(), source()];
		let keys: Vec<_> = sources
			.iter()
			.map(|source| {
				let key = pending(&mut cache, source, 328);
				cache.queue.push_back(key);
				key
			})
			.collect();
		assert!(cache.next_decode().is_some());
		assert!(cache.next_decode().is_some());
		cache.reset_scopes(ROOT);
		cache.retire(|_| {});
		assert_eq!(cache.entries.len(), 2);
		assert!(matches!(cache.entries[&keys[0]].state, State::Decoding(_)));
		assert!(!cache.entries.contains_key(&keys[2]));
		cache.running -= 1;
		cache.complete(keys[0], Some(image(328)), |_| {});
		assert!(cache.next_decode().is_none());
		cache.complete(keys[0], Some(image(64)), |_| {});
		assert!(matches!(cache.entries[&keys[0]].state, State::Ready(_)));
		assert_eq!(cache.bytes, 328 * 328 * 4);
	}

	#[test]
	fn signout_releases_warm_sources_and_virtual_batches_reset_at_reclaim() {
		let app = gpui_test::TestAppContext::single();
		app.update(|cx| {
			let source = source();
			let mut cache = CoverCache::default();
			let key = pending(&mut cache, &source, 328);
			cache.complete(key, Some(image(328)), |_| {});
			cache.reset_scopes(ROOT);
			cache.retire(|_| {});
			assert_eq!(Arc::strong_count(&source), 2);
			cx.set_global(cache);
			assert!(CoverCache::first_in_batch(LISTING_ROWS, cx));
			assert!(!CoverCache::first_in_batch(LISTING_ROWS, cx));
			assert!(CoverCache::schedule_reclaim(cx));
			assert!(!CoverCache::schedule_reclaim(cx));
			CoverCache::reclaim(cx);
			assert!(CoverCache::first_in_batch(LISTING_ROWS, cx));
			CoverCache::clear(cx);
			assert_eq!(Arc::strong_count(&source), 1);
			assert_eq!(cx.global::<CoverCache>().bytes, 0);
			assert!(cx.global::<CoverCache>().entries.is_empty());
			assert!(cx.global::<CoverCache>().sources.is_empty());
		});
	}

	#[test]
	fn cached_regions_keep_shared_buckets_until_all_leases_end() {
		let mut cache = CoverCache::default();
		let source = source();
		let key = pending(&mut cache, &source, 328);
		cache.complete(key, Some(image(328)), |_| {});
		cache.entries.get_mut(&key).unwrap().scopes = ROOT | LISTING_ROWS;
		cache.reset_scopes(ROOT);
		let mut released = 0;
		cache.retire(|_| released += 1);
		assert_eq!(released, 0);
		assert!(matches!(cache.entries[&key].state, State::Ready(_)));
		cache.reset_scopes(LISTING_ROWS);
		cache.retire(|_| released += 1);
		assert_eq!(released, 1);
		assert!(matches!(cache.entries[&key].state, State::Warm(_)));
		assert_eq!(cache.bytes, (WARM_SIDE * WARM_SIDE * 4) as usize);
	}

	#[test]
	fn immediate_back_has_pixels_before_full_decode_and_preserves_source_identity() {
		use gpui_kit::AppContext as _;
		let app = gpui_test::TestAppContext::single();
		app.update(|cx| {
			let mut cache = CoverCache::default();
			let source = source();
			let weak = Arc::downgrade(&source);
			let key = pending(&mut cache, &source, 328);
			cache.sources.insert("cover".into(), Source { cover: weak.clone(), scopes: ROOT });
			cache.complete(key, Some(image(328)), |_| {});
			cache.reset_scopes(ROOT);
			cache.retire(|_| {});
			drop(source);
			assert_eq!(weak.strong_count(), 1);
			cx.set_global(cache);
			let warm_source = CoverCache::source("cover", cx).unwrap();
			assert_eq!(weak.as_ptr(), Arc::as_ptr(&warm_source));
			assert!(!CoverCache::visible_source("cover", cx));
			let requester = cx.new(|_| ()).into_any().downgrade();
			cx.update_global::<CoverCache, _>(|cache, _| {
				let preview = cache.lookup(key, requester.clone()).unwrap();
				assert_eq!(u32::try_from(preview.size(0).width.0).unwrap(), WARM_SIDE);
				assert!(cache.lookup(key, requester).is_some_and(|next| Arc::ptr_eq(&preview, &next)));
				assert_eq!(cache.queue.len(), 1);
				assert_eq!(cache.running, 0);
				let mut released = 0;
				cache.complete(key, Some(image(328)), |_| released += 1);
				assert_eq!(released, 1);
				assert_eq!(cache.bytes, 328 * 328 * 4);
				assert!(matches!(cache.entries[&key].state, State::Ready(_)));
			});
		});
	}

	#[test]
	fn two_typical_two_x_pages_fit_the_measured_warm_budget() {
		// Measured 1280x800 Home: 21 encoded sources total 1,335,419 bytes;
		// 20 decoded entries. At 2x, 328px full buckets cost 8.2 MiB per page,
		// but warm previews plus these encoded sources cost under 2 MiB per page.
		let mut cache = CoverCache::default();
		let mut keys = Vec::new();
		for _ in 0..3 {
			cache.reset_scopes(ROOT);
			let sources: Vec<_> = (0..20).map(|_| Arc::new(Cover { bytes: vec![0; 1_335_419 / 21], mime: "image/jpeg".into() })).collect();
			for source in &sources {
				let key = pending(&mut cache, source, 328);
				cache.complete(key, Some(image(328)), |_| {});
				keys.push(key);
			}
			cache.retire(|_| {});
			cache.reset_scopes(ROOT);
			cache.retire(|_| {});
		}
		let per_cover = (WARM_SIDE * WARM_SIDE * 4) as usize + 1_335_419 / 21;
		let retained = WARM_BUDGET / per_cover;
		assert!(retained >= 40);
		assert_eq!(cache.entries.len(), retained);
		assert!(keys[..keys.len() - retained].iter().all(|key| !cache.entries.contains_key(key)));
		assert!(keys[keys.len() - retained..].iter().all(|key| matches!(cache.entries[key].state, State::Warm(_))));
		let warm_bytes: usize = cache.entries.values().map(Entry::warm_bytes).sum();
		assert!(warm_bytes <= WARM_BUDGET);
		assert_eq!(warm_bytes, retained * per_cover);
	}

	#[test]
	fn warm_preview_preserves_aspect_and_already_bgra_channels() {
		let full = Arc::new(RenderImage::new([Frame::new(RgbaImage::from_pixel(328, 164, image::Rgba([30, 80, 210, 255])))]));
		let warm = preview(&full).unwrap();
		assert_eq!(warm.size(0).width.0, 96);
		assert_eq!(warm.size(0).height.0, 48);
		assert_eq!(&warm.as_bytes(0).unwrap()[..4], &[30, 80, 210, 255]);
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
					let hit = CoverCache::get("cover", &source, 32, 2., cx);
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
	fn smaller_requests_reuse_larger_pixels_without_another_buffer_or_upload() {
		use gpui_kit::AppContext as _;

		let app = gpui_test::TestAppContext::single();
		app.update(|cx| {
			let source = source();
			let expected = image(192);
			let mut cache = CoverCache::default();
			let key = pending(&mut cache, &source, 192);
			cache.complete(key, Some(Arc::clone(&expected)), |_| {});
			cx.set_global(cache);
			let requester = cx.new(|_| ());
			requester.update(cx, |(), cx| {
				for side in [64, 96, 128, 192] {
					let hit = CoverCache::get("cover", &source, side, 1., cx);
					assert!(hit.is_some_and(|image| Arc::ptr_eq(&image, &expected)));
				}
			});
			let cache = cx.global::<CoverCache>();
			assert_eq!(cache.entries.len(), 1);
			assert_eq!(cache.bytes, 192 * 192 * 4);
			assert_eq!(cache.running, 0);
			assert!(cache.queue.is_empty());
		});
	}

	#[test]
	fn reuse_preserves_resolution_blur_and_pending_decodes() {
		let mut cache = CoverCache::default();
		let source = source();
		for side in [128, 384] {
			let key = pending(&mut cache, &source, side);
			cache.complete(key, Some(image(side)), |_| {});
		}
		assert_eq!(cache.cached_key(Key::new(&source, 64)), Some(Key::new(&source, 128)));
		assert_eq!(cache.cached_key(Key::new(&source, 192)), Some(Key::new(&source, 384)));
		assert_eq!(cache.cached_key(Key::new(&source, 512)), None);
		assert_eq!(cache.cached_key(Key::blurred(&source)), None);
		let pending_key = pending(&mut cache, &source, 64);
		assert_eq!(cache.cached_key(pending_key), Some(pending_key));
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
