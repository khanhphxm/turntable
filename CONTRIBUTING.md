# Contributing to Turntable

Pull requests are welcome. Keep each one to a single change you understand and
can explain. Report security issues privately as described in
[SECURITY.md](SECURITY.md), never in a public issue.

## Before you start

- **Small fixes** (bugs, docs, narrow improvements): open a pull request directly.
- **Larger changes** (new features, new dependencies, vendored-crate patches,
  cross-platform behavior): open an issue first so the approach can be agreed
  before you write it.

Use synthetic account data in tests, and redact Spotify credentials and personal
library data from logs and screenshots.

## Build and check

Rust 1.90+ (stable) for building; nightly only for rustfmt, because
`rustfmt.toml` uses unstable options.

```sh
rustup toolchain install nightly --component rustfmt
cargo run --release --locked -p turntable
cargo +nightly fmt --all                         # apply formatting
scripts/lint.sh                                  # fmt, clippy, docs, deps, ban
cargo test --locked --workspace --all-targets
```

- **Linux**: install the headers listed in the [README](README.md#build-from-source),
  or run `scripts/install-linux-deps.sh` (Ubuntu 24.04, uses sudo) for the full
  set CI uses. The `deps` lint step needs `cargo install cargo-machete`.
- **macOS**: Xcode command-line tools. `cargo run`/`cargo test` go through
  `scripts/run-signed.sh`, which signs the binary for stable Keychain access.
- **Windows**: the MSVC toolchain.

CI runs `fmt` once, `scripts/lint.sh clippy docs deps ban` on Linux, plain clippy
on macOS and Windows, and the full test suite on all three. Platform code has to
be checked natively on its OS. Workflow changes also need `actionlint` and
`zizmor .github/workflows`.

## Pull requests

- Title it as a conventional commit, e.g. `fix(player): keep shuffle on restore`.
  CI enforces this; allowed types are `feat`, `fix`, `perf`, `refactor`, `docs`,
  `test`, `build`, `ci`, `chore`, `style` and `revert`. The title becomes the
  squash commit and the changelog entry.
- Fill in the template in your own words: what changed, why, and how you
  verified it. Passing CI is not enough. Exercise the changed behavior on the
  platforms it touches and say which ones you could not test.
- Behavior changes need tests; docs that describe the behavior change with it.
- No drive-by refactors or unrelated cleanup.

AI tools are fine. You are still responsible for every line: review the full
diff, keep it within scope, and run the checks yourself before opening the PR.

## Vendored crates

`[patch.crates-io]` in the root `Cargo.toml` points patched dependencies at
`vendor/`. To change one:

1. Edit the `vendor/` copy, never your Cargo registry. New vendored crates land
   as a pristine registry copy in their own commit, then the patch on top.
2. Keep the patch small and keep upstream licenses and attribution.
3. Record the upstream version and every local change in that crate's
   `TURNTABLE.md`.
4. Start each modified upstream file under Apache-2.0 with a one-line notice,
   e.g. `// Modified by Turntable: <what>; see ../TURNTABLE.md.`
5. Commit the resulting `Cargo.lock` change with the patch.

## Third-party notices

After changing dependencies or embedded assets, regenerate
`THIRD_PARTY_NOTICES` with the pinned generator and commit the result:

```sh
cargo install cargo-about --version 0.9.2 --locked --features cli
packaging/generate-notices.sh
```

Releases and packaging are covered in [packaging/README.md](packaging/README.md).

## Audio benchmark

Measures the playback chain (decoder, EQ, normalization, resampler) offline into
a paced null output. Linux only; needs `ffmpeg`, no account or audio device.

```sh
sh scripts/audio-fixtures.sh /tmp/turntable-audio 30
cargo build --release -p turntable-core --example audio-bench --features audio-bench
target/release/examples/audio-bench /tmp/turntable-audio/vorbis.ogg --rate 48000
```

`python3 scripts/audio-equivalence.py <before> <after> <fixtures-dir>` checks
that two builds produce identical output.

## License

Contributions are accepted under the project's [MIT license](LICENSE).
Vendored code keeps its own license.
