//! One supported proxy format for every transport, including librespot's native CONNECT socket.
//! SOCKS and HTTPS proxies are deliberately rejected: librespot's socket only speaks HTTP CONNECT.
//! Empty settings preserve each HTTP client's system/environment discovery; native sessions use
//! the first HTTP proxy in ALL_PROXY, HTTPS_PROXY or HTTP_PROXY (including lowercase variants).

use oauth2::{reqwest, url::Url};

/// Validates a proxy without echoing potentially secret credentials in errors.
pub fn parse(value: &str) -> Result<Option<Url>, &'static str> {
	let value = value.trim();
	if value.is_empty() {
		return Ok(None);
	}
	if value.chars().any(char::is_whitespace) || value.chars().any(char::is_control) {
		return Err("Proxy URL must not contain spaces or control characters.");
	}
	let url = Url::parse(value).map_err(|_error| "Enter a proxy URL such as http://host:8080.")?;
	if url.scheme() != "http" {
		return Err("Use an http:// proxy. HTTPS and SOCKS proxies are not supported by the native session.");
	}
	if url.host_str().is_none() || url.port_or_known_default().is_none_or(|port| port == 0) {
		return Err("Proxy needs a host and a valid port (1–65535).");
	}
	if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
		return Err("Proxy URL must contain only a host, port and optional user:password.");
	}
	if url.password().is_some() && url.username().is_empty() {
		return Err("Proxy authentication needs a username.");
	}
	ureq::Proxy::new(url.as_str()).map_err(|_error| "Invalid proxy host or credentials.")?;
	Ok(Some(url))
}

/// Resolves the proxy for librespot, which has no environment discovery of its own.
pub(crate) fn session(value: &str) -> Result<Option<Url>, &'static str> {
	if let Some(proxy) = parse(value)? {
		return Ok(Some(proxy));
	}
	Ok(["ALL_PROXY", "all_proxy", "HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"]
		.into_iter()
		.filter_map(|key| std::env::var(key).ok())
		.find_map(|value| parse(&value).ok().flatten()))
}

/// Token exchange client; explicit configuration overrides environment bypass rules.
pub(crate) fn oauth(value: &str, timeout: std::time::Duration) -> std::io::Result<reqwest::Client> {
	let mut builder = reqwest::Client::builder().timeout(timeout).redirect(reqwest::redirect::Policy::none());
	if let Some(proxy) = parse(value).map_err(std::io::Error::other)? {
		builder = builder.no_proxy().proxy(reqwest::Proxy::all(proxy.as_str()).map_err(std::io::Error::other)?);
	}
	builder.build().map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn proxy_validation() {
		for value in ["", "  "] {
			assert_eq!(parse(value), Ok(None));
		}
		for value in ["http://localhost:8080", " http://user:pass@host:3128 ", "http://[::1]:8888", "http://host:80"] {
			assert!(parse(value).unwrap().is_some(), "{value}");
		}
		for value in [
			"host:8080",
			"http://",
			"http://host:0",
			"http://host:65536",
			"https://host:80",
			"socks5://host:1080",
			"http://host/path",
			"http://host?x",
			"http://host#x",
			"http://:pass@host:80",
			"http://bad host:80",
		] {
			assert!(parse(value).is_err(), "{value}");
		}
	}
}
