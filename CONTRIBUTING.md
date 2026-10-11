# Contributing

Keep changes focused and explain the user-visible problem in your issue or PR.
Use synthetic account data in tests and redact logs/screenshots. Report security
issues privately as described in [SECURITY.md](SECURITY.md).

## Build and check

Use Rust 1.90+ (stable), plus nightly rustfmt:

```sh
rustup toolchain install nightly --component rustfmt
cargo run --release --locked -p turntable
cargo +nightly fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace --all-targets
```

On Linux, install native headers with `scripts/install-linux-deps.sh`
(Debian/Ubuntu; uses sudo). Windows requires the MSVC toolchain; macOS requires
Xcode command-line tools. Run platform checks natively; CI checks all three OSes.
`scripts/lint.sh` also checks rustdoc, unused dependencies (install
`cargo-machete`) and banned lint patterns. Workflow changes need `actionlint`
and `zizmor .github/workflows`.

## Commits and vendored code

Use conventional commit/PR titles, e.g. `fix(player): preserve shuffle state`.
Include tests for behavior changes.

Vendored crates are selected by `[patch.crates-io]` in the root `Cargo.toml`.
Edit the matching `vendor/` copy, not your Cargo registry. Keep patches small,
retain upstream licenses/attribution and record the upstream version plus local
changes in that crate's `TURNTABLE.md`. For modified Apache-licensed files, also
retain a prominent in-file modification notice. Commit any necessary lockfile
update with the patch. See [packaging/README.md](packaging/README.md) for releases.
