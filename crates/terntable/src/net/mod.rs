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

pub(crate) mod cache;
pub(crate) mod par;
pub(crate) mod trace;

use std::{sync::LazyLock, time::Duration};

use ureq::{Agent, RequestBuilder};

/// Bytes of each input and output buffer. A response head larger than this is refused.
const BUFFER: usize = 16 * 1024;
/// Idle connections kept, in all and per host; others are closed.
const IDLE: usize = 6;
const IDLE_PER_HOST: usize = 3;
/// How long an idle connection stays pooled.
const IDLE_AGE: Duration = Duration::from_secs(20);

static AGENT: LazyLock<Agent> = LazyLock::new(|| {
	Agent::config_builder()
		.http_status_as_error(false)
		.input_buffer_size(BUFFER)
		.output_buffer_size(BUFFER)
		.max_response_header_size(BUFFER)
		.max_idle_connections(IDLE)
		.max_idle_connections_per_host(IDLE_PER_HOST)
		.max_idle_age(IDLE_AGE)
		.build()
		.into()
});

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
	AGENT.clone()
}

/// Gives the allocator's free pages back to the OS once a burst of work (artwork, a page of
/// JSON, a track loading) is over: jemalloc returns dirty pages lazily, as allocations go on,
/// and an idle player allocates nothing.
#[cfg(not(windows))]
pub fn trim_heap() {
	// `arena.<MALLCTL_ARENAS_ALL>.purge`: every arena, every unused dirty page, now.
	const PURGE_ALL: &[u8] = b"arena.4096.purge\0";
	// SAFETY: a NUL-terminated control name with no old or new value: a plain command.
	unsafe {
		tikv_jemalloc_sys::mallctl(PURGE_ALL.as_ptr().cast(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), 0);
	}
}

/// Windows' heap returns freed pages by itself.
#[cfg(windows)]
pub fn trim_heap() {}
