//! Loopback-only proxy contract tests. Destinations use .invalid so a bypass cannot succeed.
//! No Spotify credentials, Web API calls or playback are involved.
use std::{
	io::{Read as _, Write as _},
	net::{TcpListener, TcpStream},
	thread::{self, JoinHandle},
	time::{Duration, Instant},
};

use super::{Profile, build_agent, proxy};

pub(crate) const IMAGE: &[u8] = b"\x89PNG\r\n\x1a\nproxy";
const AUTH: &str = "Basic dXNlcjpwYXNz";

pub(crate) struct LocalProxy {
	pub(crate) url: String,
	worker:         JoinHandle<()>,
}

fn head(stream: &mut TcpStream) -> String {
	let mut result = Vec::new();
	while !result.ends_with(b"\r\n\r\n") {
		let mut byte = [0];
		stream.read_exact(&mut byte).unwrap();
		result.push(byte[0]);
		assert!(result.len() < 8192);
	}
	String::from_utf8(result).unwrap()
}

impl LocalProxy {
	pub(crate) fn spawn() -> Self {
		let listener = TcpListener::bind("127.0.0.1:0").unwrap();
		let url = format!("http://user:pass@{}", listener.local_addr().unwrap());
		listener.set_nonblocking(true).unwrap();
		let worker = thread::spawn(move || {
			let until = Instant::now() + Duration::from_secs(10);
			let mut stream = loop {
				match listener.accept() {
					Ok((stream, _)) => break stream,
					Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
						assert!(Instant::now() < until, "client never reached the proxy");
						thread::sleep(Duration::from_millis(10));
					},
					Err(error) => panic!("{error}"),
				}
			};
			// Winsock inherits the listener's nonblocking mode. Only accept polls;
			// the HTTP exchange uses blocking reads bounded by socket timeouts.
			stream.set_nonblocking(false).unwrap();
			stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
			stream.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
			let request = head(&mut stream);
			assert!(
				request
					.lines()
					.filter_map(|line| line.split_once(':'))
					.any(|(name, value)| name.eq_ignore_ascii_case("proxy-authorization") && value.trim() == AUTH),
				"missing proxy authentication: {request}"
			);
			if request.starts_with("CONNECT ") {
				assert!(request.contains("synthetic.invalid:"));
				stream.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").unwrap();
				let tunneled = head(&mut stream);
				assert!(tunneled.starts_with("GET "));
				assert!(!tunneled.to_lowercase().contains("proxy-authorization"), "credentials leaked to destination");
			} else {
				assert!(request.starts_with("GET http://synthetic.invalid/"));
			}
			write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", IMAGE.len())
				.unwrap();
			stream.write_all(IMAGE).unwrap();
		});
		Self { url, worker }
	}

	pub(crate) fn agent(&self) -> ureq::Agent {
		build_agent(Some(ureq::Proxy::new(&self.url).unwrap()))
	}

	pub(crate) fn finish(self) {
		self.worker.join().unwrap();
	}
}

#[test]
fn shared_http_agent_and_profiles_use_proxy() {
	let proxy = LocalProxy::spawn();
	let agent = proxy.agent();
	assert_eq!(agent.config().proxy().unwrap().host(), "127.0.0.1");
	let profile = Profile {
		timeout:         Duration::from_secs(5),
		connect:         Some(Duration::from_secs(3)),
		user_agent:      "proxy-test",
		status_as_error: true,
	};
	let mut response = profile.apply(agent.get("http://synthetic.invalid/metadata")).call().unwrap();
	assert_eq!(response.body_mut().read_to_vec().unwrap(), IMAGE);
	proxy.finish();
}

#[test]
fn oauth_token_client_uses_proxy() {
	let proxy = LocalProxy::spawn();
	let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
	runtime.block_on(async {
		let client = proxy::oauth(&proxy.url, Duration::from_secs(5)).unwrap();
		let response = client.get("http://synthetic.invalid/token").send().await.unwrap();
		assert_eq!(response.bytes().await.unwrap().as_ref(), IMAGE);
	});
	proxy.finish();
}

#[test]
fn native_session_http_client_uses_proxy() {
	let proxy = LocalProxy::spawn();
	let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
	runtime.block_on(async {
		let config = librespot_core::config::SessionConfig { proxy: proxy::session(&proxy.url).unwrap(), ..Default::default() };
		let session = librespot_core::session::Session::new(config, None);
		let request = http::Request::builder().uri("http://synthetic.invalid/session").body(Default::default()).unwrap();
		assert_eq!(session.http_client().request_body(request).await.unwrap().as_ref(), IMAGE);
		session.shutdown();
	});
	proxy.finish();
}
