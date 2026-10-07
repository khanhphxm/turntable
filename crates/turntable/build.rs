//! Embed the Windows icon; GPUI's default `windows-manifest` supplies the manifest.

use std::{env, error::Error, fs, path::PathBuf};

#[expect(clippy::print_stdout, reason = "Cargo build-script directives use stdout")]
fn main() -> Result<(), Box<dyn Error>> {
	println!("cargo:rerun-if-env-changed=TURNTABLE_ICON");
	if env::var("CARGO_CFG_TARGET_OS")? != "windows" {
		return Ok(());
	}
	let icon = env::var_os("TURNTABLE_ICON")
		.map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../packaging/icons/turntable.ico"), PathBuf::from);
	let icon = std::path::absolute(icon)?;
	println!("cargo:rerun-if-changed={}", icon.display());
	let output = PathBuf::from(env::var_os("OUT_DIR").ok_or("Cargo did not set OUT_DIR")?);
	// Use a slash-normalized absolute path so rc.exe and llvm-rc agree.
	let resource = output.join("turntable-icon.rc");
	fs::write(&resource, format!("1 ICON \"{}\"\n", icon.display().to_string().replace('\\', "/")))?;
	embed_resource::compile_for(resource, ["turntable"], embed_resource::NONE).manifest_required()?;
	Ok(())
}
