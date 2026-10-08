//! Shared request infrastructure: the HTTP agent, bounded caches, parallel requests and tracing.
//!
//! ureq keeps an input and an output buffer of 128 KiB in each layer of every connection it
//! pools, and a TLS connection has two layers: half a megabyte of dirty heap per connection.
//! With the Web API, the web player, open data and the artwork CDN each holding an agent of
//! their own, idle connections alone were a few megabytes of the footprint. One agent shares
//! one pool (a few idle connections at most) and 16 KiB buffers, which hold any response head
//! these services send.
//!
//! Each caller keeps its own timeouts and name through a [`Profile`].

pub mod cache;
pub mod par;
pub mod proxy;
#[cfg(test)]
pub(crate) mod proxy_tests;
pub mod trace;

use std::{sync::LazyLock, time::Duration};

use parking_lot::RwLock;
use ureq::{Agent, RequestBuilder};

/// Bytes of each input and output buffer. A response head larger than this is refused.
const BUFFER: usize = 16 * 1024;
/// Idle connections kept, in all and per host; others are closed.
const IDLE: usize = 6;
const IDLE_PER_HOST: usize = 3;
/// How long an idle connection stays pooled.
const IDLE_AGE: Duration = Duration::from_secs(20);

static AGENT: LazyLock<RwLock<Agent>> = LazyLock::new(|| RwLock::new(build_agent(None)));

/// Replaces the pool, so subsequent metadata, lyrics and image requests cannot reuse
/// connections established with the previous proxy. In-flight requests finish normally.
pub(crate) fn configure_proxy(value: &str) -> Result<(), &'static str> {
	let proxy = proxy::parse(value)?;
	let proxy = proxy.map(|url| ureq::Proxy::new(url.as_str())).transpose().map_err(|_error| "Invalid proxy host or credentials.")?;
	*AGENT.write() = build_agent(proxy);
	Ok(())
}

fn build_agent(proxy: Option<ureq::Proxy>) -> Agent {
	let mut builder = Agent::config_builder();
	if let Some(proxy) = proxy {
		builder = builder.proxy(Some(proxy));
	}
	builder
		.http_status_as_error(false)
		.input_buffer_size(BUFFER)
		.output_buffer_size(BUFFER)
		.max_response_header_size(BUFFER)
		.max_idle_connections(IDLE)
		.max_idle_connections_per_host(IDLE_PER_HOST)
		.max_idle_age(IDLE_AGE)
		.build()
		.into()
}

/// How one caller talks: its time limits, the name it gives, and whether an error status is an `Err`.
#[derive(Clone, Copy, Debug)]
pub struct Profile {
	/// The whole exchange, body included.
	pub timeout:         Duration,
	/// Connecting alone (`None`: only the whole-exchange limit).
	pub connect:         Option<Duration>,
	pub user_agent:      &'static str,
	/// ureq's own behaviour: a 4xx or 5xx status is an error (`false`: the caller reads the status).
	pub status_as_error: bool,
}

impl Profile {
	/// `builder` on the shared agent's pool, with this profile's limits.
	pub fn apply<B>(&self, builder: RequestBuilder<B>) -> RequestBuilder<B> {
		builder
			.config()
			.timeout_global(Some(self.timeout))
			.timeout_connect(self.connect)
			.user_agent(self.user_agent)
			.http_status_as_error(self.status_as_error)
			.build()
	}

	pub fn get(&self, url: &str) -> RequestBuilder<ureq::typestate::WithoutBody> {
		self.apply(agent().get(url))
	}
}

/// The shared agent (a handle: cloning shares the pool).
pub fn agent() -> Agent {
	AGENT.read().clone()
}
