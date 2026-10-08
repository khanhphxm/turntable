//! Read-only, opt-in acceptance for the Date added listing columns. No player,
//! Connect device, Web API requests, or playlist writes are constructed.
//! Run with `TURNTABLE_TEST_CONFIG=~/.config/turntable cargo test -p turntable-core
//! playlist_date_added_live -- --ignored --nocapture`.

use std::{error::Error, path::PathBuf};

use librespot_core::{SessionConfig, cache::Cache};

use super::{Listing, Reader, Session, SessionReader};

#[test]
#[ignore = "reads real native playlist and collection data; requires saved account credentials"]
fn playlist_date_added_live() -> Result<(), Box<dyn Error>> {
	let config =
		std::env::var_os("TURNTABLE_TEST_CONFIG").map(PathBuf::from).ok_or("Set TURNTABLE_TEST_CONFIG to the saved account directory")?;
	crate::net::trace::init();
	let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build()?;
	let cache = Cache::new(Some(&config), Some(&config), None::<&PathBuf>, None)?;
	let credentials = cache.credentials().ok_or("No saved credentials in the test config")?;
	let session = {
		let _context = runtime.enter();
		Session::new(SessionConfig::default(), None)
	};
	runtime.block_on(session.connect(credentials, false))?;
	let (_stop, rx) = tokio::sync::watch::channel(false);
	let shared = SessionReader::from_session(session.clone(), runtime.handle().clone(), rx);
	let mut reader = Reader::new(shared);
	let result = (|| -> Result<(), Box<dyn Error>> {
		let (_, _, page) = reader.playlist("0uYekLRLqH1pzscbhIos1E", 0, true)?;
		let mut listing = Listing::default();
		super::super::pages::take_session_page(&mut listing, page.ok_or("Native playlist returned no page")?);
		assert!(!listing.tracks.is_empty());
		assert_eq!(listing.added_at.len(), listing.tracks.len());
		for (row, track) in listing.tracks.iter().take(5).enumerate() {
			let at = listing.added_at(row).ok_or("First playlist rows have no added date")?;
			// June 8, 2026 UTC, allowing the neighboring UTC date for local timezones.
			assert!((1_780_790_400..1_781_049_600).contains(&at), "unexpected first-row date: {at}");
			eprintln!("playlist row={row} added_at={at} added_by={:?} uri={} title={:?}", listing.added_by(row), track.uri, track.name);
		}
		let mut liked = Listing::default();
		super::super::pages::take_session_page(&mut liked, reader.liked(0)?);
		assert_eq!(liked.added_at.len(), liked.tracks.len());
		for (row, track) in liked.tracks.iter().take(3).enumerate() {
			let at = liked.added_at(row).ok_or("Liked Songs row has no added date")?;
			assert!((1_000_000_000..10_000_000_000).contains(&at), "expected Unix seconds: {at}");
			eprintln!("liked row={row} added_at={at} uri={}", track.uri);
		}
		Ok(())
	})();
	session.shutdown();
	result
}
