//! terntable: a lightweight Spotify player for Tern.

mod app;
mod audio;
mod engine;
mod library;
mod net;
mod sources;
mod spotify;
mod ui;

/// jemalloc with one arena and no thread caches, configured in `.cargo/config.toml`
/// (`JEMALLOC_SYS_WITH_MALLOC_CONF`): unlike macOS's allocator it hands freed pages back, so the
/// footprint follows what is in use rather than the peak (see [`net::trim_heap`]). Windows keeps
/// the system allocator (jemalloc doesn't build with MSVC).
#[cfg(not(windows))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use std::{
	io::{self, Write},
	process::ExitCode,
};

use crate::{
	app::{config, player},
	net::trace,
};

const USAGE: &str = "\
terntable: a lightweight Spotify player for Tern

USAGE:
  terntable [--device-name NAME]   run in a Tern pane (signs in through the browser the first time)
  terntable logout                 forget the saved Spotify sign-in

OPTIONS:
  --device-name NAME      Spotify Connect name of this player (default: terntable)
  -V, --version           print the version

FILES (in ~/.config/terntable):
  credentials.json   the saved Spotify sign-in
  settings.json      preferences
";

fn main() -> ExitCode {
	trace::init();
	match run(std::env::args().skip(1).collect()) {
		Ok(()) => ExitCode::SUCCESS,
		Err(err) => {
			let _ = writeln!(io::stderr().lock(), "terntable: {err}");
			ExitCode::FAILURE
		},
	}
}

fn run(args: Vec<String>) -> Result<(), String> {
	let paths = config::Paths::resolve().map_err(|e| e.to_string())?;
	let mut opts = player::Options { device_name: None };
	let mut args = args.into_iter();
	while let Some(arg) = args.next() {
		match arg.as_str() {
			"logout" => {
				engine::forget_sign_in(&paths.config).map_err(|e| e.to_string())?;
				library::snapshot::clear(&paths.cache).map_err(|e| e.to_string())?;
				return writeln!(io::stdout().lock(), "Signed out; the saved sign-in is deleted.").map_err(|e| e.to_string());
			},
			"--device-name" => opts.device_name = Some(args.next().ok_or("--device-name needs a value")?),
			"-h" | "--help" => {
				return write!(io::stdout().lock(), "{USAGE}").map_err(|e| e.to_string());
			},
			"-V" | "--version" => {
				return writeln!(io::stdout().lock(), "terntable {}", env!("CARGO_PKG_VERSION")).map_err(|e| e.to_string());
			},
			other => return Err(format!("unknown argument {other:?}\n\n{USAGE}")),
		}
	}
	player::run(&paths, &opts)
}
