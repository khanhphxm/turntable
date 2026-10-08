//! Artwork: a few fetcher threads and a bounded cache of the files on disk.
//!
//! Covers arrive by the dozen (a launch draws a hundred), and one connection at a time took seconds.
//! A handful of threads fetch together, and each cover is kept as a file in the cache directory, so
//! the next launch (and a cover evicted from view) comes from disk. The UI side remembers what it
//! asked for, so each URL is requested once; a failure is tried again after a pause.

use std::{
	collections::HashSet,
	fs,
	path::{Path, PathBuf},
	sync::{Arc, mpsc},
	thread,
	time::{Duration, SystemTime},
};

use parking_lot::Mutex;
use sha2::{Digest, Sha256};

use crate::net::{Profile, cache::Cache, trace};

/// Covers fetched at once.
const THREADS: usize = 2;
/// What the cache keeps on disk; the oldest files go first when it grows past this.
const CACHE_BYTES: u64 = 64 << 20;
/// A failed fetch is tried again no sooner than this.
const RETRY: Duration = Duration::from_secs(300);
const BODY_LIMIT: u64 = 4 * 1024 * 1024;
/// Without timeouts one stalled connection (sleep/wake, captive portal) would hold a thread for good.
const PROFILE: Profile = Profile {
	timeout:         Duration::from_secs(15),
	connect:         Some(Duration::from_secs(5)),
	user_agent:      concat!("turntable/", env!("CARGO_PKG_VERSION")),
	status_as_error: true,
};

/// What a fetch came to.
pub enum Cover {
	Loaded { url: String, bytes: Vec<u8>, mime: String },
	Failed(String),
}

pub struct Fetcher {
	tx:        mpsc::Sender<String>,
	requested: HashSet<String>,
	failed:    Cache<String, ()>,
}

impl Fetcher {
	/// A request sink without threads or network, used only by offline controller fixtures.
	#[cfg(any(test, feature = "test-support"))]
	pub(crate) fn fixture() -> Self {
		let (tx, _) = mpsc::channel();
		Self { tx, requested: HashSet::new(), failed: Cache::new(512, RETRY) }
	}

	/// Starts the fetcher threads; `deliver` runs on them. Covers are cached under `dir`.
	pub fn spawn(dir: PathBuf, deliver: impl Fn(Cover) + Send + Sync + 'static) -> Self {
		let (tx, rx) = mpsc::channel::<String>();
		let rx = Arc::new(Mutex::new(rx));
		let (dir, deliver) = (Arc::new(dir), Arc::new(deliver));
		let pruning = dir.clone();
		let _ = thread::Builder::new().name("cover-prune".into()).stack_size(256 * 1024).spawn(move || prune(&pruning, CACHE_BYTES));
		for n in 0..THREADS {
			let (rx, dir, deliver) = (rx.clone(), dir.clone(), deliver.clone());
			let _ = thread::Builder::new().name(format!("covers{n}")).stack_size(512 * 1024).spawn(move || {
				loop {
					// One thread waits for the next URL, the others for their turn at it.
					let next = rx.lock().recv();
					let Ok(url) = next else { return };
					deliver(match cover(&dir, &url) {
						Some((bytes, mime)) => Cover::Loaded { url, bytes, mime },
						None => Cover::Failed(url),
					});
				}
			});
		}
		Self { tx, requested: HashSet::new(), failed: Cache::new(512, RETRY) }
	}

	pub fn request(&mut self, url: String) {
		if self.requested.contains(&url) || self.failed.get(&url).is_some() {
			return;
		}
		self.failed.remove(&url);
		self.requested.insert(url.clone());
		let _ = self.tx.send(url);
	}

	pub fn loaded(&mut self, url: &str) {
		self.requested.remove(url);
	}

	pub fn failed(&mut self, url: String) {
		self.requested.remove(&url);
		self.failed.insert(url, ());
	}

	/// Covers asked for and not yet answered.
	pub fn outstanding(&self) -> usize {
		self.requested.len()
	}
}

/// A cover from the cache directory, else from the network (and into the cache).
fn cover(dir: &Path, url: &str) -> Option<(Vec<u8>, String)> {
	let file = dir.join(file_name(url));
	if let Ok(bytes) = fs::read(&file)
		&& let Some(mime) = sniff(&bytes)
	{
		trace::hit("cover", &trace::endpoint(url), "disk");
		return Some((bytes, mime.to_owned()));
	}
	let (bytes, mime) = fetch(url)?;
	store(dir, &file, &bytes);
	Some((bytes, mime))
}

fn fetch(url: &str) -> Option<(Vec<u8>, String)> {
	let span = trace::start();
	let fetched = PROFILE.get(url).call().ok().and_then(|mut response| {
		let mime = response.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("image/jpeg").to_owned();
		let bytes = response.body_mut().with_config().limit(BODY_LIMIT).read_to_vec().ok()?;
		Some((bytes, mime))
	});
	let reply = trace::Reply {
		status: if fetched.is_some() { 200 } else { 0 },
		bytes: fetched.as_ref().map_or(0, |(bytes, _)| bytes.len()),
		..trace::Reply::default()
	};
	span.done("img", "GET", url, reply);
	fetched
}

/// The file a URL is cached in: the hex of its hash.
fn file_name(url: &str) -> String {
	format!("{:x}", Sha256::digest(url.as_bytes()))
}

/// The image type of `bytes` by their first bytes (`None`: not an image, so not a cover we cached).
fn sniff(bytes: &[u8]) -> Option<&'static str> {
	match bytes {
		[0xFF, 0xD8, 0xFF, ..] => Some("image/jpeg"),
		[0x89, b'P', b'N', b'G', ..] => Some("image/png"),
		[b'G', b'I', b'F', b'8', ..] => Some("image/gif"),
		[b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some("image/webp"),
		_ => None,
	}
}

/// Writes a fetched cover to the cache, best effort: through a temporary file, so a reader never sees half of one.
fn store(dir: &Path, file: &Path, bytes: &[u8]) {
	if sniff(bytes).is_none() || fs::create_dir_all(dir).is_err() {
		return;
	}
	let temporary = file.with_extension("part");
	if fs::write(&temporary, bytes).is_ok() && fs::rename(&temporary, file).is_err() {
		let _ = fs::remove_file(&temporary);
	}
}

/// Deletes the oldest files of `dir` until it holds at most three quarters of `limit` bytes,
/// when it holds more than `limit`.
fn prune(dir: &Path, limit: u64) {
	let Ok(entries) = fs::read_dir(dir) else { return };
	let mut files: Vec<(SystemTime, u64, PathBuf)> = entries
		.filter_map(Result::ok)
		.filter_map(|entry| {
			let meta = entry.metadata().ok()?;
			meta.is_file().then(|| (meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len(), entry.path()))
		})
		.collect();
	let mut total: u64 = files.iter().map(|(_, len, _)| len).sum();
	if total <= limit {
		return;
	}
	files.sort_by_key(|(modified, ..)| *modified);
	for (_, len, path) in files {
		if total <= limit / 4 * 3 {
			break;
		}
		if fs::remove_file(&path).is_ok() {
			total = total.saturating_sub(len);
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn images_are_told_apart_by_their_first_bytes() {
		assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0, 0]), Some("image/jpeg"));
		assert_eq!(sniff(b"\x89PNG\r\n\x1a\n"), Some("image/png"));
		assert_eq!(sniff(b"RIFF\x10\0\0\0WEBPVP8 "), Some("image/webp"));
		assert_eq!(sniff(b"<html>429</html>"), None, "an error page is not a cover");
		assert_eq!(sniff(b""), None);
	}

	#[test]
	fn a_cover_is_stored_and_read_back_and_junk_is_not_stored() {
		let directory = tempfile::tempdir().unwrap();
		let dir = directory.path();
		let png = b"\x89PNG\r\n\x1a\nrest".to_vec();
		let file = dir.join(file_name("https://i.scdn.co/image/a"));
		store(dir, &file, &png);
		assert_eq!(fs::read(&file).unwrap(), png);
		let junk = dir.join(file_name("https://i.scdn.co/image/b"));
		store(dir, &junk, b"<html>");
		assert!(!junk.exists());
		assert!(fs::read_dir(dir).unwrap().all(|e| e.unwrap().path().extension().is_none()), "no temporary file is left behind");
	}

	#[test]
	fn a_real_cached_cover_decodes_without_a_network_request() -> Result<(), Box<dyn std::error::Error>> {
		use image::ImageEncoder as _;
		let dir = tempfile::tempdir()?;
		let url = "https://invalid.example/cached-cover.png";
		let image = image::RgbaImage::from_pixel(2, 2, image::Rgba([12, 34, 56, 255]));
		let mut encoded = Vec::new();
		image::codecs::png::PngEncoder::new(&mut encoded).write_image(image.as_raw(), 2, 2, image::ExtendedColorType::Rgba8)?;
		store(dir.path(), &dir.path().join(file_name(url)), &encoded);
		let (bytes, mime) = cover(dir.path(), url).ok_or("cached PNG was not served")?;
		assert_eq!(mime, "image/png");
		assert_eq!(image::load_from_memory(&bytes)?.into_rgba8(), image);
		Ok(())
	}

	#[test]
	fn pruning_removes_the_oldest_files_down_to_three_quarters_of_the_limit() {
		let directory = tempfile::tempdir().unwrap();
		let dir = directory.path();
		let start = SystemTime::now();
		for i in 0..10_u64 {
			let path = dir.join(format!("f{i}"));
			fs::write(&path, vec![0_u8; 100]).unwrap();
			fs::File::options().write(true).open(&path).unwrap().set_modified(start + Duration::from_secs(i)).unwrap();
		}
		prune(dir, 1000);
		assert_eq!(fs::read_dir(dir).unwrap().count(), 10, "at the limit nothing goes");
		prune(dir, 800);
		let mut kept: Vec<String> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
		kept.sort();
		assert_eq!(kept, ["f4", "f5", "f6", "f7", "f8", "f9"].map(String::from), "600 bytes are left: the six newest");
	}

	#[test]
	fn failed_cover_cache_evicts_old_entries_and_expires_retries() {
		let mut fetcher = Fetcher::fixture();
		for index in 0..600 {
			fetcher.failed(format!("cover-{index}"));
		}
		assert!(fetcher.failed.get("cover-0").is_none());
		assert!(fetcher.failed.get("cover-599").is_some());
		assert!(fetcher.failed.get("cover-100").is_some(), "retention evicts one old entry at a time");
		assert!(fetcher.failed.get_at("cover-599", std::time::Instant::now() + RETRY).is_none());
	}
}
