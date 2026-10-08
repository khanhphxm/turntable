//! Web API request contracts, authenticated errors, pagination and request coalescing tests.

use std::{
	io::{Read, Write},
	net::TcpListener,
	sync::mpsc,
	thread,
};

use super::*;
use crate::spotify::auth::{Token, now_unix};

/// Request log of a [`mock`] server; read it after joining the server thread.
struct Seen(mpsc::Receiver<String>);

impl Seen {
	fn all(&self) -> Vec<String> {
		self.0.try_iter().collect()
	}
}

/// Serves one scripted response per connection and records the requests.
fn mock(responses: Vec<String>) -> (String, Seen, thread::JoinHandle<()>) {
	let listener = TcpListener::bind("127.0.0.1:0").unwrap();
	let base = format!("http://{}", listener.local_addr().unwrap());
	let (log, seen) = mpsc::channel();
	let handle = thread::spawn(move || {
		for response in responses {
			let (mut stream, _) = listener.accept().unwrap();
			let mut buf = Vec::new();
			let mut chunk = [0u8; 1024];
			loop {
				if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
					let head = String::from_utf8_lossy(&buf[..end]).to_ascii_lowercase();
					let len =
						head.lines().find_map(|l| l.strip_prefix("content-length:")).map_or(0, |v| v.trim().parse::<usize>().unwrap());
					if buf.len() >= end + 4 + len {
						break;
					}
				}
				let n = stream.read(&mut chunk).unwrap();
				if n == 0 {
					break;
				}
				buf.extend_from_slice(&chunk[..n]);
			}
			log.send(String::from_utf8_lossy(&buf).into_owned()).unwrap();
			stream.write_all(response.as_bytes()).unwrap();
		}
	});
	(base, Seen(seen), handle)
}

fn reply(status: &str, headers: &str, body: &str) -> String {
	format!("HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
}

fn client(base: String) -> Client {
	let source: TokenSource = Arc::new(|| Ok(Token { access_token: "secret-at".into(), expires_at: now_unix() + 3600 }));
	Client::new(Session::new(source), base, true)
}

#[test]
fn plain_429_is_retried_once_after_retry_after() {
	let (base, seen, handle) = mock(vec![
		reply("429 Too Many Requests", "Retry-After: 0\r\n", "rate limit"),
		reply("200 OK", "", r#"{"id":"me","display_name":"Me"}"#),
	]);
	let (me, _) = client(base).me_with_plan().unwrap();
	handle.join().unwrap();
	assert_eq!(me.id, "me");
	let seen = seen.all();
	assert_eq!(seen.len(), 2);
	assert!(seen[0].starts_with("GET /me HTTP/1.1"));
	assert!(seen[1].to_ascii_lowercase().contains("authorization: bearer secret-at"));
}

#[test]
fn second_429_is_surfaced_not_looped() {
	let (base, seen, handle) =
		mock(vec![reply("429 Too Many Requests", "Retry-After: 0\r\n", ""), reply("429 Too Many Requests", "Retry-After: 5\r\n", "")]);
	let err = client(base).me_with_plan().unwrap_err();
	handle.join().unwrap();
	assert!(matches!(err, Error::RateLimited { retry_after: Some(d) } if d.as_secs() == 5));
	assert_eq!(seen.all().len(), 2);
}

#[test]
fn long_retry_after_is_not_slept_on() {
	let (base, seen, handle) = mock(vec![reply("429 Too Many Requests", "Retry-After: 3600\r\n", "")]);
	let err = client(base).me_with_plan().unwrap_err();
	handle.join().unwrap();
	assert!(matches!(err, Error::RateLimited { retry_after: Some(d) } if d.as_secs() == 3600));
	assert_eq!(seen.all().len(), 1);
}

#[test]
fn quota_exceeded_is_never_retried() {
	let (base, seen, handle) = mock(vec![reply(
		"429 Too Many Requests",
		"Retry-After: 0\r\nContent-Type: application/json\r\n",
		r#"{"reason":"QUOTA_EXCEEDED"}"#,
	)]);
	let err = client(base).saved_tracks(0, 50).unwrap_err();
	handle.join().unwrap();
	assert!(matches!(err, Error::QuotaExceeded { .. }));
	assert_eq!(seen.all().len(), 1);
}

#[test]
fn library_calls_chunk_uris_and_writes_are_not_retried() {
	let uris: Vec<String> = (0..45).map(|i| format!("spotify:track:{i:022}")).collect();
	let refs: Vec<&str> = uris.iter().map(String::as_str).collect();
	let first = format!("[{}]", vec!["true"; 40].join(","));
	let second = format!("[{}]", ["false"; 5].join(","));
	let (base, seen, handle) = mock(vec![reply("200 OK", "", &first), reply("200 OK", "", &second)]);
	let flags = client(base).library_contains(&refs).unwrap();
	handle.join().unwrap();
	assert_eq!(flags.len(), 45);
	assert_eq!(flags.iter().filter(|b| **b).count(), 40);
	let seen = seen.all();
	assert!(seen[0].starts_with("GET /me/library/contains?uris=spotify%3Atrack%3A"));
	assert_eq!(seen[0].split("%2C").count(), 40);
	assert_eq!(seen[1].split("%2C").count(), 5);

	let (base, seen, handle) = mock(vec![reply("429 Too Many Requests", "Retry-After: 0\r\n", "")]);
	let err = client(base).save(&["spotify:track:a"]).unwrap_err();
	handle.join().unwrap();
	assert!(matches!(err, Error::RateLimited { .. }));
	assert!(seen.all()[0].starts_with("PUT /me/library?uris=spotify%3Atrack%3Aa"));
}

#[test]
fn playlist_details_put_sends_only_changed_fields() {
	let (base, seen, handle) = mock(vec![reply("200 OK", "", "")]);
	let changes =
		super::super::PlaylistChanges { description: Some(String::new()), public: Some(false), ..super::super::PlaylistChanges::default() };
	client(base).rename_playlist("playlist/id", &changes).unwrap();
	handle.join().unwrap();
	let requests = seen.all();
	assert_eq!(requests.len(), 1);
	let request = requests.first().unwrap();
	assert!(request.starts_with("PUT /playlists/playlist%2Fid HTTP/1.1"));
	let body: serde_json::Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
	assert_eq!(body, serde_json::json!({ "description": "", "public": false }));
}

#[test]
fn playlist_items_end_to_end() {
	let body = r#"{"limit":50,"offset":50,"total":120,"next":"x","items":[
			{"item":{"type":"track","uri":"spotify:track:a","name":"A","duration_ms":5,"artists":[],"album":{"name":"Al"}}},
			{"item":null}]}"#;
	let (base, seen, handle) = mock(vec![reply("200 OK", "", body)]);
	let page = client(base).playlist_items("37i9/../x", 50, 500).unwrap();
	handle.join().unwrap();
	assert_eq!(page.items.len(), 1);
	assert_eq!(page.next_offset(), Some(100));
	let head = &seen.all()[0];
	assert!(head.starts_with("GET /playlists/37i9%2F..%2Fx/items?offset=50&limit=50 HTTP/1.1"), "{head}");
}

#[test]
fn devices_are_parsed_and_idless_ones_dropped() {
	let body = r#"{"devices":[
			{"id":"abc123","is_active":true,"is_private_session":false,"is_restricted":false,
			 "name":"turntable","type":"Speaker","volume_percent":42,"supports_volume":true},
			{"id":null,"is_active":false,"name":"Restricted TV","type":"TV","volume_percent":null},
			{"id":"phone","name":"Phone","type":"Smartphone"}]}"#;
	let (base, seen, handle) = mock(vec![reply("200 OK", "", body)]);
	let devices = client(base).devices().unwrap();
	handle.join().unwrap();
	assert!(seen.all()[0].starts_with("GET /me/player/devices HTTP/1.1"));
	assert_eq!(devices.len(), 2);
	assert_eq!(devices[0], Device {
		id:              "abc123".into(),
		name:            "turntable".into(),
		is_active:       true,
		volume_percent:  Some(42),
		supports_volume: Some(true),
		kind:            "Speaker".into(),
	});
	assert!(!devices[1].is_active && devices[1].volume_percent.is_none());
}

#[test]
fn playback_is_none_without_an_active_device_and_parsed_otherwise() {
	let body = r#"{"device":{"id":"tv1","is_active":true,"name":"Living room","type":"TV","volume_percent":null,"supports_volume":false},
			"repeat_state":"context","shuffle_state":true,"context":{"type":"playlist","uri":"spotify:playlist:p"},
			"progress_ms":4200,"is_playing":true,
			"item":{"type":"track","uri":"spotify:track:a","name":"A","duration_ms":9000,"artists":[{"name":"X","uri":"spotify:artist:x"}]}}"#;
	let (base, seen, handle) = mock(vec![reply("204 No Content", "", ""), reply("200 OK", "", body)]);
	let c = client(base);
	assert!(c.playback().unwrap().is_none());
	let p = c.playback().unwrap().unwrap();
	handle.join().unwrap();
	assert!(seen.all()[0].starts_with("GET /me/player HTTP/1.1"));
	assert_eq!(
		(p.device.name.as_str(), p.device.category(), p.device.volume_capable()),
		("Living room", crate::spotify::DeviceKind::Tv, false)
	);
	assert!(p.is_playing && p.shuffle && p.repeat == RepeatMode::Context && p.progress_ms == 4200);
	assert_eq!(p.item.unwrap().name, "A");
	assert_eq!(p.context.unwrap().uri, "spotify:playlist:p");
}

#[test]
fn transfer_and_control_target_explicit_devices() {
	let (base, seen, handle) = mock(vec![reply("204 No Content", "", ""), reply("204 No Content", "", "")]);
	let c = client(base);
	c.transfer("tv", false).unwrap();
	c.control("phone", &PlayerCommand::Seek(4200)).unwrap();
	handle.join().unwrap();
	let requests = seen.all();
	assert_eq!(requests.len(), 2);
	assert!(requests[0].starts_with("PUT /me/player HTTP/1.1"));
	let body: serde_json::Value = serde_json::from_str(requests[0].split("\r\n\r\n").nth(1).unwrap()).unwrap();
	assert_eq!(body, serde_json::json!({ "device_ids": ["tv"], "play": false }));
	assert!(requests[1].starts_with("PUT /me/player/seek?device_id=phone&position_ms=4200 HTTP/1.1"));
}

#[test]
fn control_without_active_device_is_device_not_found_and_not_retried() {
	let body = r#"{"error":{"status":404,"message":"Player command failed: No active device found","reason":"NO_ACTIVE_DEVICE"}}"#;
	let (base, seen, handle) = mock(vec![reply("404 Not Found", "", body)]);
	let err = client(base).control("d", &PlayerCommand::Resume).unwrap_err();
	handle.join().unwrap();
	assert!(matches!(err, Error::DeviceNotFound));
	assert_eq!(seen.all().len(), 1);
}

#[test]
fn recently_played_drops_null_tracks_and_clamps_the_limit() {
	let recent = r#"{"limit":2,"next":null,"items":[
			{"played_at":"2026-01-01T00:00:00Z","track":{"type":"track","uri":"spotify:track:a","name":"A","duration_ms":5}},
			{"played_at":"2026-01-01T00:00:01Z","track":null}]}"#;
	let (base, seen, handle) = mock(vec![reply("200 OK", "", recent)]);
	let tracks = client(base).recently_played(99).unwrap();
	handle.join().unwrap();
	assert_eq!(tracks.len(), 1);
	assert!(tracks[0].1.is_none());
	let seen = seen.all();
	assert!(seen[0].starts_with("GET /me/player/recently-played?limit=50"), "{}", seen[0]);
}

#[test]
fn saved_albums_page_by_offset_and_drop_null_albums() {
	let body = r#"{"limit":50,"offset":50,"total":120,"next":"x","items":[
			{"added_at":"2026-01-01T00:00:00Z","album":{"name":"A","uri":"spotify:album:a"}},
			{"added_at":"2026-01-01T00:00:01Z","album":null}]}"#;
	let (base, seen, handle) = mock(vec![reply("200 OK", "", body)]);
	let page = client(base).saved_albums(50, 99).unwrap();
	handle.join().unwrap();
	assert_eq!(page.items.len(), 1);
	assert_eq!(page.next_offset(), Some(100));
	let seen = seen.all();
	assert!(seen[0].starts_with("GET /me/albums?offset=50&limit=50 HTTP/1.1"), "{}", seen[0]);
}

#[test]
fn followed_artists_page_by_cursor() {
	let first = r#"{"artists":{"limit":50,"next":"https://api.spotify.com/v1/me/following?after=b","total":2,"cursors":{"after":"b"},
			"items":[{"name":"A","uri":"spotify:artist:a"},{"name":"B","uri":"spotify:artist:b"}]}}"#;
	let second = r#"{"artists":{"limit":50,"next":null,"total":2,"items":[]}}"#;
	let (base, seen, handle) = mock(vec![reply("200 OK", "", first), reply("200 OK", "", second)]);
	let c = client(base);
	let page = c.followed_artists(None, 99).unwrap();
	assert_eq!(page.items.len(), 2);
	assert!(page.next.is_some());
	assert!(c.followed_artists(Some("b"), 50).unwrap().next.is_none());
	handle.join().unwrap();
	let seen = seen.all();
	assert!(seen[0].starts_with("GET /me/following?type=artist&limit=50 HTTP/1.1"), "{}", seen[0]);
	assert!(seen[1].starts_with("GET /me/following?type=artist&limit=50&after=b HTTP/1.1"), "{}", seen[1]);
}

#[test]
fn an_identical_get_a_moment_later_comes_from_memory_and_a_write_forgets_it() {
	let list = r#"{"limit":50,"offset":0,"total":1,"next":null,"items":[{"name":"A","uri":"spotify:artist:a"}]}"#;
	let (base, seen, handle) = mock(vec![
		reply("200 OK", "", list),
		reply("204 No Content", "", ""),
		reply("200 OK", "", r#"{"limit":50,"offset":0,"total":0,"next":null,"items":[]}"#),
	]);
	let c = client(base);
	let first: Page<Artist> = c.get_page("/me/top/artists", 0, 50).unwrap();
	let again: Page<Artist> = c.get_page("/me/top/artists", 0, 50).unwrap();
	assert_eq!((first.items.len(), again.items.len()), (1, 1), "the second answer came from memory");
	c.save(&["spotify:artist:a"]).unwrap();
	let after: Page<Artist> = c.get_page("/me/top/artists", 0, 50).unwrap();
	assert!(after.items.is_empty(), "a write empties the memory, so the list is read again");
	handle.join().unwrap();
	assert_eq!(seen.all().len(), 3, "three requests for four calls");
}

#[test]
fn callers_asking_for_the_same_page_together_send_one_request() {
	let (base, seen, handle) = mock(vec![reply("200 OK", "", r#"{"limit":50,"offset":0,"total":0,"next":null,"items":[]}"#)]);
	let c = Arc::new(client(base));
	let callers: Vec<_> = (0..4)
		.map(|_| {
			let c = c.clone();
			thread::spawn(move || c.playlists(0, 50).unwrap().total)
		})
		.collect();
	for caller in callers {
		assert_eq!(caller.join().unwrap(), 0);
	}
	handle.join().unwrap();
	assert_eq!(seen.all().len(), 1, "four callers, one request");
}

#[test]
fn an_album_carries_its_first_page_of_tracks() {
	let body = r#"{"name":"Al","uri":"spotify:album:a","total_tracks":2,"tracks":{"limit":50,"offset":0,"total":2,"next":null,
			"items":[{"type":"track","uri":"spotify:track:1","name":"One","duration_ms":1},{"type":"track","uri":"spotify:track:2","name":"Two","duration_ms":2}]}}"#;
	let (base, seen, handle) = mock(vec![reply("200 OK", "", body)]);
	let (album, page) = client(base).album_full("a").unwrap();
	handle.join().unwrap();
	assert_eq!(album.name, "Al");
	assert_eq!(page.unwrap().items.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["One", "Two"]);
	assert!(seen.all()[0].starts_with("GET /albums/a HTTP/1.1"), "one request, not the album and its tracks");
}

#[test]
fn a_conditional_get_sends_if_none_match_and_reads_a_304() {
	let (base, seen, handle) = mock(vec![reply("304 Not Modified", "ETag: \"abc\"\r\n", "")]);
	let url = format!("{base}/x");
	let req = Req { method: Method::Get, url: &url, query: &[], bearer: None, form: None, json: None, limit: 1024 };
	let got = Http::new().send_conditional(&req, Some("\"abc\"")).unwrap();
	handle.join().unwrap();
	assert!(got.not_modified && got.body.is_empty());
	assert_eq!(got.etag.as_deref(), Some("\"abc\""));
	assert!(seen.all()[0].to_ascii_lowercase().contains("if-none-match: \"abc\""));
}

#[test]
fn playlist_row_write_contracts_keep_duplicates_and_final_move_indices() {
	let (base, seen, handle) = mock(vec![reply("200 OK", "", "{}"), reply("200 OK", "", "{}"), reply("200 OK", "", "{}")]);
	let client = client(base);
	client.remove_playlist_row("scratch", "spotify:track:a", 2).unwrap();
	client.move_playlist_row("scratch", 1, 4).unwrap();
	client.move_playlist_row("scratch", 4, 1).unwrap();
	client.move_playlist_row("scratch", 1, 1).unwrap();
	handle.join().unwrap();
	let requests = seen.all();
	assert_eq!(requests.len(), 3);
	let body = |index: usize| serde_json::from_str::<serde_json::Value>(requests[index].split("\r\n\r\n").nth(1).unwrap()).unwrap();
	assert!(requests[0].starts_with("DELETE /playlists/scratch/items HTTP/1.1"));
	assert_eq!(body(0), serde_json::json!({ "items": [{ "uri": "spotify:track:a", "positions": [2] }] }));
	assert!(requests[1].starts_with("PUT /playlists/scratch/items HTTP/1.1"));
	assert_eq!(body(1), serde_json::json!({ "range_start": 1, "range_length": 1, "insert_before": 5 }));
	assert_eq!(body(2), serde_json::json!({ "range_start": 4, "range_length": 1, "insert_before": 1 }));
}

#[test]
fn cache_only_refuses_reads_writes_and_conditional_requests_before_auth_or_io() {
	let listener = TcpListener::bind("127.0.0.1:0").unwrap();
	listener.set_nonblocking(true).unwrap();
	let base = format!("http://{}", listener.local_addr().unwrap());
	let source: TokenSource = Arc::new(|| panic!("cache-only requests must not ask for access tokens"));
	let client = Client::new(Session::new(source), base, false);
	for method in [Method::Get, Method::Post, Method::Put, Method::Delete] {
		assert!(matches!(client.request(method, "/me", &[], None), Err(Error::Network(message)) if message.contains("cache-only")));
	}
	assert!(matches!(client.fetch(Method::Get, "/me", &[], None, Some("etag")), Err(Error::Network(_))));
	assert!(matches!(client.get_bytes("/me", &[]), Err(Error::Network(_))));
	assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
}
