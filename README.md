# turntable

A standalone native Spotify desktop app built with GPUI, with an in-process
[librespot](https://github.com/librespot-org/librespot) player. The core supports
macOS, Linux and Windows; playback needs Spotify Premium. No browser engine or
terminal host is involved.

## Workspace

- `crates/turntable`: the GPUI desktop application.
- `crates/turntable-core`: UI-agnostic playback, DSP, Spotify sign-in/Web API,
  library/cover workers, lyrics/external sources, and Elm-style application state.
- `old/tern`: the retired terminal UI, input/rendering/signal helpers, binary entry
  point, plugin, release automation and historical README. Reference only: none
  of it is a workspace member or compiled.

The core retains Home/search/library navigation, playlists, queue editing,
Spotify Connect controls, synced lyrics, a 10-band EQ, normalization, mono,
balance, sleep timer and preferences. The host owns window layout, input,
rendering, image decoding, OS signals and allocator policy.

## Build and run

Use Rust stable (edition 2024) and the platform's C build tools:

```sh
cargo build -p turntable-core
cargo run -p turntable
```

On macOS, `scripts/codesign.sh <binary>` signs the binary for stable Keychain
access. Cargo's macOS runner uses `scripts/run-signed.sh` automatically.

macOS audio uses CoreAudio and supports choosing an output device. Linux uses
`pw-cat` (`pipewire-bin`), then `pacat` (`pulseaudio-utils`), then `aplay`
(`alsa-utils`) if the preceding executable is absent; install at least one.
Windows uses shared-mode, event-driven WASAPI on the default render endpoint.
EQ and the output stage run in the core on every platform. Idle output stops
and resumes when PCM arrives.

## Sign-in and storage

1. Sign in to play using your Spotify Premium account in the browser.
2. Create a Spotify application in the
   [developer dashboard](https://developer.spotify.com/dashboard), enable Web
   API, register `http://127.0.0.1:8898/callback`, and provide its Client ID.
3. Connect the library by signing in with that application.

The default Spotify Connect name is **turntable**. Config/state is
`$XDG_CONFIG_HOME/turntable` or `~/.config/turntable` on macOS/Linux, and
`%APPDATA%\turntable` on Windows. Cache is `~/Library/Caches/turntable` on macOS,
`$XDG_CACHE_HOME/turntable` or `~/.cache/turntable` on Linux, and
`%LOCALAPPDATA%\turntable` on Windows. The Web API token uses Keychain, Secret
Service or Credential Manager with service `turntable`.

`TURNTABLE_HOME=<dir>` uses `<dir>/config` and `<dir>/cache` and keeps the Web API
token in a file rather than an OS credential store. This is useful for isolated
tests. `TURNTABLE_DEBUG` enables the temporary `turntable-debug.log`;
`TURNTABLE_TRACE` enables request tracing (see `net::trace`). This is a clean
cutover: previous application directories and credential-store entries are not
migrated, read, signed out or deleted. Sign in once for the new app.

Account-scoped library snapshots cache the profile, playlists, Home and up to
eight first-page listings (2 MiB total). The saved identity gates snapshot
loading; cached responses arrive before network requests, followed by fresh
responses. Playback sign-out and library Disconnect delete snapshots. A new
browser authorization cannot reuse a previous account's data. Covers retain a
separate bounded disk cache. Queue edits restart the current song once with the
edited list; other devices can still control playback through Spotify Connect.

Outside requests go to Spotify, LRCLIB (song title/artist/length) and
Wikipedia/Wikidata (artist metadata); optional sources can be disabled in
preferences.

## Hosting the core

```rust,no_run
use std::{sync::Arc, time::Instant};
use turntable_core::{Controller, Msg, Options, Paths};

fn start(wake: Arc<dyn Fn() + Send + Sync>) -> Result<Controller, String> {
    let paths = Paths::resolve().map_err(|error| error.to_string())?;
    Controller::new(&paths, Options::default(), wake)
}

fn host_iteration(controller: &mut Controller, action: Option<Msg>) -> (bool, Option<Instant>) {
    let mut redraw = action.is_some_and(|msg| controller.dispatch(msg));
    redraw |= controller.pump();
    // Render controller.state() if redraw; State::image records missing URLs.
    // Pump after rendering to start those cover fetches, then schedule this deadline.
    (redraw, controller.deadline())
}
```

The host calls `pump()` on worker wakes and deadlines. `set_visible(bool)` maps
focus/minimize state and suppresses remote playback polling while hidden.
`state()` exposes host-neutral models; `Msg::Seek(u64)` uses milliseconds and
`Msg::Volume(u8)` uses percent. `Msg::Edit` ranges are UTF-16 code units.
`State::image` returns `Option<&Arc<Cover>>`; `Cover` contains encoded bytes and
MIME, not a GUI image. Decode once and cache by `Arc::as_ptr` while retaining the
Arc: a loaded URL's identity is stable for the controller's lifetime. Lyric
changes invalidate the host at the next line's deadline; the host scrolls using
`State::lyric_line()`. Dropping the controller stops client, audio and engine in
order without signing out.

## Development

```sh
cargo +nightly fmt --all
cargo build -p turntable-core
cargo test -p turntable-core
cargo clippy -p turntable-core --all-targets -- -D warnings
scripts/lint.sh
```

Debug builds optimize dependencies at level 2 while keeping workspace crates quick
to compile. Keep the lockfile's `vergen 9.0.6` pin when refreshing dependencies:
`vergen 9.1.0` uses an incompatible `vergen-lib` version for librespot 0.8's
`vergen-gitcl 1.0.8` build metadata. After a full lockfile regeneration, restore it
with `cargo update -p vergen --precise 9.0.6`.

Cross-check the core's Windows path from macOS using LLVM/lld and `cargo-xwin`:

```sh
PATH=/opt/homebrew/opt/llvm/bin:/opt/homebrew/opt/lld/bin:$PATH \
CARGO_TARGET_DIR=target/win \
cargo xwin check -p turntable-core --target x86_64-pc-windows-msvc
```

Cross-compilation does not exercise WASAPI, credential storage or browser
redirects; those require a Windows host. Unit tests do not use the user's
credentials or live playback. The core has no global allocator or
allocator-specific reclamation; allocator choice belongs to the desktop binary.

`scripts/bench/play.sh` controls a Connect device using an explicitly supplied
`SPOTIFY_ACCESS_TOKEN`; playback commands can interrupt another device.
`scripts/bench/measure.sh` measures a supplied macOS process without controlling
playback. Neither helper reads the app's saved sign-in.

turntable is independent of Spotify. Spotify is a trademark of Spotify AB.
[MIT](LICENSE).
