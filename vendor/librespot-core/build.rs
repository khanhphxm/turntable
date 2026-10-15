// Modified by Turntable: the upstream script embedded the enclosing repository's git commit,
// commit date, build date and a random build ID via vergen, so every commit (and every day)
// rebuilt librespot-core and everything above it. The values only feed informational strings,
// so they are fixed (see TURNTABLE.md).
fn main() {
    let version = env!("CARGO_PKG_VERSION");
    println!("cargo:rustc-env=VERGEN_GIT_SHA={version}");
    println!("cargo:rustc-env=VERGEN_GIT_COMMIT_DATE=unknown");
    println!("cargo:rustc-env=VERGEN_BUILD_DATE=unknown");
    println!("cargo:rustc-env=LIBRESPOT_BUILD_ID=turntable");
    println!("cargo:rerun-if-changed=build.rs");
}
