//! Native HTTP regression smoke tests: loopback only, without Spotify or an authenticated account.

use std::{
	io::{Read, Write},
	net::TcpListener,
	sync::{Arc, mpsc},
	thread,
	time::{Duration, Instant},
};

use librespot_core::http_client::HttpClient;

use super::Error;

fn runtime() -> tokio::runtime::Runtime {
	tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
}

fn mock(responses: Vec<(&str, &str)>) -> (String, mpsc::Receiver<Instant>, thread::JoinHandle<()>) {
	let listener = TcpListener::bind("127.0.0.1:0").unwrap();
	let base = format!("http://{}", listener.local_addr().unwrap());
	let (tx, rx) = mpsc::channel();
	let responses: Vec<_> = responses
		.into_iter()
		.map(|(status, headers)| format!("HTTP/1.1 {status}\r\n{headers}Content-Length: 0\r\nConnection: close\r\n\r\n"))
		.collect();
	let handle = thread::spawn(move || {
		for response in responses {
			let (mut stream, _) = listener.accept().unwrap();
			stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
			let mut request = Vec::new();
			let mut bytes = [0; 1024];
			while !request.windows(4).any(|window| window == b"\r\n\r\n") {
				let len = stream.read(&mut bytes).unwrap();
				assert_ne!(len, 0);
				request.extend_from_slice(&bytes[..len]);
			}
			tx.send(Instant::now()).unwrap();
			stream.write_all(response.as_bytes()).unwrap();
		}
	});
	(base, rx, handle)
}

fn request(client: &HttpClient, method: http::Method, url: &str) -> Result<(), librespot_core::Error> {
	let request = http::Request::builder().method(method).uri(url).body(Vec::new().into()).unwrap();
	runtime().block_on(client.request_body(request)).map(|_| ())
}

#[test]
fn native_get_retries_after_backoff_and_a_success_clears_the_incident() {
	let (url, seen, server) = mock(vec![("429 Too Many Requests", "Retry-After: 1\r\n"), ("200 OK", "")]);
	let client = HttpClient::new(None);
	assert!(request(&client, http::Method::GET, &url).is_ok());
	let initial = seen.recv_timeout(Duration::from_secs(5)).unwrap();
	let retry = seen.recv_timeout(Duration::from_secs(5)).unwrap();
	server.join().unwrap();
	assert!(retry.duration_since(initial) >= Duration::from_secs(1));
	assert!(seen.try_recv().is_err());
	assert!(client.rate_limit_until().is_none(), "only a successful network reply clears the incident");
}

#[test]
fn native_concurrent_callers_share_one_gate_without_sending_or_waiting() {
	let listener = TcpListener::bind("127.0.0.1:0").unwrap();
	listener.set_nonblocking(true).unwrap();
	let url = format!("http://{}", listener.local_addr().unwrap());
	let client = Arc::new(HttpClient::new(None));
	let error = Error::from(client.rate_limit(None));
	assert_eq!(error.user_message(), Error::RateLimited { retry_after: None }.user_message());
	thread::scope(|scope| {
		let readers: Vec<_> = (0..8).map(|_| scope.spawn(|| request(&client, http::Method::GET, &url))).collect();
		for reader in readers {
			assert!(matches!(Error::from(reader.join().unwrap().unwrap_err()), Error::RateLimited { .. }));
		}
	});
	assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
}

#[test]
fn native_repeated_429_retries_only_once_and_keeps_the_gate_closed() {
	let (url, seen, server) = mock(vec![("429 Too Many Requests", "Retry-After: 0\r\n"), ("429 Too Many Requests", "Retry-After: 1\r\n")]);
	let client = HttpClient::new(None);
	let error = Error::from(request(&client, http::Method::GET, &url).unwrap_err());
	assert!(matches!(error, Error::RateLimited { retry_after: Some(wait) } if wait == Duration::from_secs(1)));
	let first = seen.recv_timeout(Duration::from_secs(5)).unwrap();
	let second = seen.recv_timeout(Duration::from_secs(5)).unwrap();
	assert!(second.duration_since(first) >= Duration::from_secs(1), "even Retry-After: 0 cannot spin");
	server.join().unwrap();
	assert!(matches!(Error::from(request(&client, http::Method::GET, &url).unwrap_err()), Error::RateLimited { .. }));
	assert!(seen.try_recv().is_err());
}

#[test]
fn native_missing_retry_after_uses_a_shared_default_and_never_replays_writes() {
	let (url, seen, server) = mock(vec![("429 Too Many Requests", "")]);
	let client = HttpClient::new(None);
	let error = Error::from(request(&client, http::Method::POST, &url).unwrap_err());
	assert!(matches!(error, Error::RateLimited { retry_after: Some(wait) } if wait == Duration::from_secs(5)));
	server.join().unwrap();
	assert!(matches!(Error::from(request(&client, http::Method::GET, &url).unwrap_err()), Error::RateLimited { .. }));
	assert_eq!(seen.try_iter().count(), 1);
}

#[test]
fn native_long_retry_after_is_preserved_without_sleeping_or_sending_again() {
	let (url, seen, server) = mock(vec![("429 Too Many Requests", "Retry-After: 3600\r\n")]);
	let client = HttpClient::new(None);
	let started = Instant::now();
	let error = Error::from(request(&client, http::Method::GET, &url).unwrap_err());
	assert!(matches!(error, Error::RateLimited { retry_after: Some(wait) } if wait == Duration::from_secs(3600)));
	assert!(started.elapsed() < Duration::from_secs(5));
	server.join().unwrap();
	assert!(matches!(Error::from(request(&client, http::Method::GET, &url).unwrap_err()), Error::RateLimited { .. }));
	assert_eq!(seen.try_iter().count(), 1);
}
