use super::*;
use crate::spotify::{SearchResults, Track, User};

fn track(name: &str) -> Track {
	serde_json::from_value(serde_json::json!({"uri":format!("spotify:track:{name}"),"name":name})).unwrap()
}

fn worker(cache: &Path) -> (Worker, mpsc::Receiver<Inbox>, mpsc::Receiver<Response>) {
	let (tx, inbox) = mpsc::channel();
	let (answers, responses) = mpsc::channel();
	let mut store = Store::new(cache);
	store.record(&Response::Connected { me: User { id: "account".into(), display_name: None }, playlists: Vec::new() });
	(
		Worker {
			tx,
			snapshots: Arc::new(Mutex::new(store)),
			deliver: Arc::new(move |_, response| {
				let _ = answers.send(response);
			}),
			prefetches: Mutex::new(Cache::new(snapshot::LISTINGS_KEPT, PAGES_TTL)),
			urgent: tokio::sync::watch::channel(()).0,
			generation: Arc::new(AtomicU64::new(0)),
		},
		inbox,
		responses,
	)
}

#[test]
fn saved_authorization_preloads_disk_listing_before_any_session_arrives() {
	let directory = tempfile::tempdir().unwrap();
	let config = directory.path().join("config");
	let cache = librespot_core::cache::Cache::new(Some(&config), Some(&config), None::<&PathBuf>, None).unwrap();
	cache.save_credentials(&librespot_core::authentication::Credentials { username: Some("account".into()), ..Default::default() });
	let mut store = Store::new(directory.path());
	store.record(&Response::Connected { me: User { id: "account".into(), display_name: None }, playlists: Vec::new() });
	store.record(&Response::Listing {
		route:   Route::Liked,
		offset:  0,
		listing: Listing { title: "Liked Songs".into(), tracks: vec![track("cached")], ..Listing::default() },
	});
	store.save().unwrap();
	let (tx, rx) = mpsc::channel();
	let worker = Worker::spawn(config, directory.path(), false, move |_, response| {
		let _ = tx.send(response);
	});
	assert!(matches!(rx.try_recv(), Ok(Response::Connected { .. })));
	worker.send(Request::Open(Route::Liked, 0));
	assert!(
		matches!(rx.try_recv(), Ok(Response::Listing { listing, .. }) if listing.title == "Liked Songs" && listing.tracks[0].name == "cached")
	);
	assert!(rx.recv_timeout(Duration::from_millis(30)).is_err(), "waiting for native auth must not clear loading via NeedsLogin");
}

#[test]
fn duplicate_prefetches_are_bounded_and_expire_without_navigation() {
	let cache = tempfile::tempdir().unwrap();
	let (worker, inbox, responses) = worker(cache.path());
	let route = Route::Album("album".into());
	for _ in 0..30 {
		worker.send(Request::Prefetch(route.clone()));
	}
	assert!(matches!(inbox.try_recv(), Ok(Inbox::Request(0, Request::Prefetch(known))) if known == route));
	assert!(inbox.try_recv().is_err());
	assert!(responses.try_recv().is_err());
	worker.send(Request::Prefetch(Route::Search));
	worker.send(Request::Prefetch(Route::Library));
	worker.send(Request::Prefetch(Route::Folder("folder".into())));
	assert!(inbox.try_recv().is_err());
	worker.prefetches.lock().insert_at(route.clone(), (), Instant::now().checked_sub(PAGES_TTL).unwrap());
	worker.send(Request::Prefetch(route));
	assert!(matches!(inbox.try_recv(), Ok(Inbox::Request(0, Request::Prefetch(_)))));
	for index in 0..100 {
		worker.send(Request::Prefetch(Route::Playlist(index.to_string())));
	}
	assert_eq!(worker.prefetches.lock().get_ref(&Route::Playlist("0".into())), None);
}

#[test]
fn artist_and_search_snapshots_deliver_before_the_worker_can_read_its_inbox() {
	let cache = tempfile::tempdir().unwrap();
	let (worker, inbox, responses) = worker(cache.path());
	let route = Route::Artist("artist".into());
	let listing = Listing {
		kicker: "Artist".into(),
		title: "Artist".into(),
		tracks: vec![track("first"), track("second")],
		popular_loaded: true,
		discography: vec![("Albums", vec![super::super::Tile::liked()])],
		overview: Some(spotify::ArtistOverview {
			name: "Artist".into(),
			biography: Some("Biography".into()),
			..spotify::ArtistOverview::default()
		}),
		..Listing::default()
	};
	let results = SearchResults {
		tracks: spotify::Page { items: vec![track("second"), track("first")], ..spotify::Page::default() },
		..SearchResults::default()
	};
	{
		let mut store = worker.snapshots.lock();
		store.record(&Response::Listing { route: route.clone(), offset: 0, listing });
		store.record(&Response::Search { query: "query".into(), results: results.clone() });
		store.save().unwrap();
		store.load(Some("account"));
	}
	worker.send(Request::Open(route, 0));
	let Response::Listing { listing, .. } = responses.try_recv().unwrap() else { panic!("snapshot not delivered") };
	assert_eq!(listing.title, "Artist");
	assert_eq!(listing.tracks.iter().map(|track| track.name.as_str()).collect::<Vec<_>>(), ["first", "second"]);
	assert!(listing.popular_loaded);
	assert_eq!(listing.discography, [("Albums", vec![super::super::Tile::liked()])]);
	assert_eq!(listing.overview.as_ref().unwrap().biography.as_deref(), Some("Biography"));
	worker.send(Request::Search(" query ".into()));
	assert!(matches!(responses.try_recv().unwrap(), Response::Search { query, results: known } if query == "query" && known == results));
	assert!(matches!(inbox.try_recv(), Ok(Inbox::Request(0, Request::Open(_, 0)))));
	assert!(matches!(inbox.try_recv(), Ok(Inbox::Request(0, Request::Search(_)))));
	worker.snapshots.lock().load(Some("other-account"));
	worker.send(Request::Search("query".into()));
	assert!(responses.try_recv().is_err(), "search results must never cross accounts");
}

#[test]
fn search_snapshots_use_lru_replace_duplicates_and_clear_on_signout() {
	let cache = tempfile::tempdir().unwrap();
	let mut store = Store::new(cache.path());
	store.record(&Response::Connected { me: User { id: "account".into(), display_name: None }, playlists: Vec::new() });
	for index in 0..snapshot::LISTINGS_KEPT {
		store.record(&Response::Search { query: index.to_string(), results: SearchResults::default() });
	}
	assert!(store.search("0").is_some());
	store.record(&Response::Search { query: "new".into(), results: SearchResults::default() });
	assert!(store.search("1").is_none());
	let replacement = SearchResults {
		tracks: spotify::Page { items: vec![track("replacement")], ..spotify::Page::default() },
		..SearchResults::default()
	};
	store.record(&Response::Search { query: "new".into(), results: replacement.clone() });
	assert_eq!(store.search("new"), Some(replacement));
	store.clear();
	assert!(store.search("new").is_none());
	assert!(!store.record(&Response::Search { query: "late".into(), results: SearchResults::default() }));
}

#[test]
fn cached_search_preserves_order_and_duplicates_without_requesting_a_token() {
	use std::sync::atomic::{AtomicUsize, Ordering};
	let cache = tempfile::tempdir().unwrap();
	let minted = Arc::new(AtomicUsize::new(0));
	let counted = minted.clone();
	let client = Client::open(
		Arc::new(move || {
			counted.fetch_add(1, Ordering::Relaxed);
			Err(spotify::Error::AuthRequired)
		}),
		false,
	);
	let mut session =
		Session::new(client, User { id: "account".into(), display_name: None }, Vec::new(), Arc::new(Mutex::new(Store::new(cache.path()))));
	let results = SearchResults {
		tracks: spotify::Page { items: vec![track("a"), track("a"), track("b")], ..spotify::Page::default() },
		..SearchResults::default()
	};
	session.searches.insert("query".into(), results.clone());
	assert!(
		matches!(search(&mut session, " query ".into()).unwrap(), Response::Search { query, results: known } if query == "query" && known == results)
	);
	assert_eq!(minted.load(Ordering::Relaxed), 0);
}

#[test]
fn a_previous_query_snapshot_cannot_replace_current_results_or_selection() {
	let current =
		SearchResults { tracks: spotify::Page { items: vec![track("current")], ..spotify::Page::default() }, ..SearchResults::default() };
	let mut library = super::super::Library {
		route: Route::Search,
		query: "current".into(),
		search: Some(current.clone()),
		selected: Some(0),
		show_selection: true,
		..super::super::Library::default()
	};
	library.apply(Response::Search { query: "old".into(), results: SearchResults::default() });
	assert_eq!(library.search, Some(current));
	assert_eq!(library.selected, Some(0));
	assert!(library.show_selection);
}

#[test]
fn fresh_artist_rows_replace_stale_rows_and_late_routes_are_ignored() {
	let route = Route::Artist("artist".into());
	let mut library = super::super::Library::default();
	let requests = library.navigate(route.clone());
	assert!(matches!(requests.first(), Some(Request::Open(known, 0)) if known == &route));
	let listing = |name| Listing { title: "Artist".into(), tracks: vec![track(name)], popular_loaded: true, ..Listing::default() };
	let mut stale = listing("stale");
	stale.overview = Some(spotify::ArtistOverview {
		name: "Artist".into(),
		biography: Some("Cached biography".into()),
		..spotify::ArtistOverview::default()
	});
	library.apply(Response::Listing { route: route.clone(), offset: 0, listing: stale });
	library.apply(Response::Listing { route: route.clone(), offset: 0, listing: listing("fresh") });
	assert_eq!(library.rows()[0].name, "fresh");
	assert_eq!(library.listing.as_ref().unwrap().overview.as_ref().unwrap().biography.as_deref(), Some("Cached biography"));
	library.navigate(Route::Liked);
	library.apply(Response::Listing { route, offset: 0, listing: listing("late") });
	assert!(library.rows().is_empty());
	assert_eq!(library.route, Route::Liked);
}

#[test]
fn a_full_listing_replaces_first_paint_instead_of_duplicating_rows() {
	let route = Route::Album("album".into());
	let mut library = super::super::Library::default();
	library.navigate(route.clone());
	let tracks = vec![track("a"), track("a"), track("b")];
	library.apply(Response::Listing {
		route:   route.clone(),
		offset:  0,
		listing: Listing { title: "Album".into(), tracks: tracks.clone(), ..Listing::default() },
	});
	library.apply(Response::Listing {
		route,
		offset: 0,
		listing: Listing { title: "Album".into(), tracks: tracks.clone(), duration_ms: Some(999), ..Listing::default() },
	});
	assert_eq!(library.rows(), tracks);
	assert_eq!(library.listing.as_ref().unwrap().duration_ms, Some(999));
}
