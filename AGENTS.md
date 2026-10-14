# Development Rules

## Default Context

Turntable is a native Spotify player for macOS, Linux and Windows, written in Rust on
GPUI (through `gpui-kit`) and librespot. Contributor workflow, CI and PR rules are in
[CONTRIBUTING.md](CONTRIBUTING.md); this file covers how the code is written.

### Layout

| Path                    | Responsibility                                                                                                                   |
| ----------------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| `crates/turntable-core` | Host-neutral library, no GPUI: app state and messages (`app`), librespot engine (`engine`), audio output and DSP (`audio`), library worker (`library`), Spotify clients (`spotify`), shared HTTP/caches/tracing (`net`), `lyrics` |
| `crates/turntable`      | The GPUI app: `shell.rs` root view and `regions.rs`, `pages/`, bar/panel/sidebar/topbar, `covers.rs`, `animation/`, OS now-playing (`media/`), tray/login/window (`desktop/`), `theme`/`widgets`/`components` |
| `vendor/`               | Patched upstream crates (GPUI backends, taffy, librespot), selected by `[patch.crates-io]`                                       |
| `scripts/`              | `lint.sh`, `install-linux-deps.sh`, `run-signed.sh` (macOS cargo runner), audio benchmark, `bench/` measurement                  |
| `packaging/`            | Release bundles and `generate-notices.sh`                                                                                        |

The core/host contract (`turntable-core/src/lib.rs`): `Controller` owns workers and the engine.
The host renders `State`, sends `Msg`, and calls `Controller::pump` when a worker wakes it or
`Controller::deadline()` passes. `State::update` is pure and returns `Effect`s. Image decoding,
windows, input and the allocator belong to the host.

## Commands

```sh
cargo +nightly fmt --all                         # rustfmt.toml needs nightly
scripts/lint.sh [fmt|clippy|docs|deps|ban]       # all steps by default
cargo test --locked --workspace --all-targets
cargo test --locked -p turntable-core <filter>   # focused
```

- Clippy only checks the current OS. Platform code must be checked natively on its OS;
  cross-checking from another OS is not supported.
- NEVER commit or push unless asked. Commits are conventional (`perf(ui): …`); see CONTRIBUTING.

## Code Conventions

### Formatting

- Tabs for indentation, 140 columns, aligned struct fields and initializers
  (`struct_field_align_threshold`), imports grouped std / external / crate with one `use` per
  crate. All of this comes from `rustfmt.toml`, so run nightly rustfmt instead of formatting by
  hand. Stable rustfmt undoes the alignment.
- Import traits for their methods as `use Trait as _`.

### Lints

`[workspace.lints]` in the root `Cargo.toml` is strict. `scripts/lint.sh ban` enforces the rest.

- Production code MUST NOT use `unwrap`, `expect`, `panic!`, `todo!`, `unreachable!`, `exit`,
  `dbg!`, `print!`/`eprint!`, or `.ok()` to discard a `Result`.
- Exceptions go at the site as `#[expect(lint, reason = "…")]`. `#[allow]`, manifest-level
  `"allow"` and `#[ignore]` tests are banned.
- `unsafe` blocks need a `// SAFETY:` comment, and `unsafe fn`s need a `# Safety` section.
- Numeric casts are linted (`cast_possible_truncation`, …). Use `From`/`try_from`, or an
  `expect` that states the proven range.
- Allocation and copy lints (`redundant_clone`, `needless_collect`, `implicit_clone`, …) are on.
  Don't silence them.

### Errors

- Failures are handled and surfaced; they never abort the player or vanish silently.
- The core uses typed error enums. For example, `spotify::Error` uses `Display` for logs and
  `user_message()` for one calm UI sentence. `thiserror` is used where deriving helps.
- OS glue in the host returns `Result<_, String>` holding a user-readable message
  (`desktop/login.rs`).
- `map_err_ignore` is denied. Name an intentionally unused error `_error`.
- `let _ =` is acceptable only for weak-entity updates whose owner may already be gone.

### Comments

- Nearly every module opens with a `//!` doc that states what it owns, its contract and its
  invariants (e.g. `covers.rs`, `media.rs`, `audio/mod.rs`). Keep it accurate when behavior
  changes.
- Comments explain why, or the contract callers rely on, in full sentences. Constants record
  their measured reason.
- No TODO/FIXME, no commented-out code. Rustdoc builds with `-D warnings`, so intra-doc links
  must resolve.

### Tests

- Tests live in `#[cfg(test)] mod tests` at the end of a file, or in a sibling `tests.rs` or
  `<topic>_tests.rs` (`tests_outside_test_module` is denied). `unwrap`, `expect`, `panic` and
  indexing are allowed in tests (`clippy.toml`). Tests may also return
  `Result<(), Box<dyn Error>>`.
- Tests are offline and deterministic: `tempfile::tempdir()` paths, `Controller::fixture`
  (feature `test-support`) and synthetic data. No network, Spotify account or audio device.
- UI tests run the real `Shell` headless with `gpui-test` (`TestAppContext`,
  `VisualTestContext`). Region reuse is asserted through render counters in `cache_tests.rs`.
  Never call `Window::refresh` there, because it bypasses every cache.
- Name tests after the observable behavior, e.g.
  `unchanged_core_pumps_do_not_invalidate_playing_regions`.

## Reuse Existing Infrastructure

- HTTP goes through `turntable_core::net::agent()` / `net::Profile`, which share one pool with
  small buffers. Never build another `ureq::Agent`.
- Use `net::cache::{Cache, Flights}` for bounded caches and request coalescing, and `net::par`
  for parallel requests.
- Measurement goes through `net::trace` (`TURNTABLE_TRACE`). There is no logging crate.
- Artwork goes through `CoverCache` (`covers.rs`). UI pieces come from `theme`, `widgets` and
  `components`.

## Performance Invariants

These are measured results. Changes that regress them need new measurements.

- **No idle redraws or polling.** `shell::run_core` sleeps until a worker wake or
  `Controller::deadline()`. New periodic work must be a core deadline, not a timer or
  per-frame check. `Shell::pump` notifies only when `Controller::pump()` reports a change, so
  an unchanged deadline must not invalidate anything. `Playhead` repaints at the next visible
  pixel or second and stops when paused or hidden. The vendored backends park display links
  and compositor waits when no frame is requested; never add work that keeps them awake.
- **Regions** (`regions.rs`). `Shell` renders through seven cached regions, and every
  `cx.notify()` on `Shell` invalidates all of them. That conservative contract stays: a region
  must never skip a Shell notification. Notify `Shell` only for shared state. Hover, scroll,
  sliders, the playhead and local animation state belong in child entities, so they invalidate
  only their own region. Don't use `window.refresh()` for ordinary updates. Extend
  `cache_tests.rs` when region boundaries change.
- **Animation** (`animation/`). Long-running motion (seek fill, elapsed clock, playing bars)
  goes through `Clock`, `Progress` and `bars`. On macOS these run in Core Animation; elsewhere,
  and while an overlay covers them (`set_obscured`), they fall back to GPUI. Timing, geometry
  and fallback logic stay platform-neutral (`model.rs`, `native.rs`), and `macos.rs` only
  applies layers. No new repeating GPUI animations. One-shot eased transitions are fine;
  repeating fallbacks cap their fps.
- **Artwork** (`covers.rs`). Request covers during render with `CoverCache::get`, inside
  `Shell::with_artwork` under a scope (region bits, `LIBRARY_ROWS`, `LISTING_ROWS`,
  `MINIPLAYER`). Images are decoded off-thread into physical-pixel buckets, at most two at a
  time. Only visible scopes keep full images (12 MiB). Inactive ones become ≤ 96 px warm
  previews (4 MiB, encoded sources included), and atlas tiles are released at the frame
  boundary. No timers. Don't keep `RenderImage`s outside the cache or decode artwork
  elsewhere.
- **Audio.** The render thread never allocates or locks, and every output backend stops
  when idle (`audio/mod.rs`).

## Platform-Specific Code

- Pick the backend at module level, with the shared policy in the platform-neutral parent:
  `#[cfg_attr(target_os = "…", path = "…/<os>.rs")] mod platform;` (`desktop/login.rs`,
  `desktop/window.rs`; `mod output` in `audio/mod.rs`), or `#[cfg(target_os = "…")] mod macos;`
  plus `use macos::Backend` (`media.rs`). OS files only adapt; they don't make app decisions.
- Implement every OS (macOS, Linux, Windows). If a backend genuinely can't apply, keep a
  defined fallback, as `animation.rs` does for GPUI.
- OS dependencies go under `[target.'cfg(…)'.dependencies]` in the crate manifest. Items one
  OS doesn't use take `#[cfg_attr(<os>, expect(dead_code, reason = "…"))]`.
- A new Linux system library must be added to `scripts/install-linux-deps.sh` and the README
  apt list.

## Vendored Crates

- Edit `vendor/<crate>`, never the registry. A new vendored crate is first committed as a
  pristine copy, then patched in a separate commit. Each `[patch.crates-io]` entry has a
  one-line comment explaining why.
- Keep upstream formatting, licenses and attribution. Modified Apache-2.0 files start with
  `// Modified by Turntable: <what>; see ../TURNTABLE.md.` Record the base version, every
  change and its measurements in the crate's `TURNTABLE.md`.
- Vendor patches stay generic: GPUI changes expose mechanisms (paint context, frame demand,
  atlas sizing) without OS types or Turntable widget policy. App policy belongs in
  `crates/turntable`.

## Generated Files

- `THIRD_PARTY_NOTICES`: regenerate with `packaging/generate-notices.sh` (cargo-about 0.9.2)
  after changing dependencies or embedded assets. Never edit it by hand.
- Release notes are generated from commit subjects by git-cliff (`cliff.toml`). There is no
  hand-written changelog.

## Don'ts

- Don't add polling, idle timers or unconditional `cx.notify()`/`window.refresh()` calls.
- Don't put GPUI, windowing or image decoding in `turntable-core`.
- Don't add a second HTTP stack, or dependencies that `cargo machete` reports as unused.
- Don't use real credentials or account data in tests, fixtures, logs or screenshots.
- Don't reformat or restyle vendored code beyond the patch.
