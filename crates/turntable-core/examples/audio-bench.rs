//! Linux offline playback benchmark. See CONTRIBUTING.md for fixture generation and usage.
#![expect(unused_crate_dependencies, reason = "this example uses the public benchmark adapters, not the library's other dependencies")]

#[cfg(target_os = "linux")]
#[path = "audio_bench/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
fn main() -> std::io::Result<()> {
	linux::run()
}

#[cfg(not(target_os = "linux"))]
fn main() -> std::io::Result<()> {
	Err(std::io::Error::other("audio-bench's process CPU/RSS accounting currently requires Linux"))
}
