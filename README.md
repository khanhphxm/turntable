<h1 align="center">terntable</h1>

<p align="center"><strong>Spotify in a <a href="https://docs.stencil.so/tern/">Tern</a> pane, in about 11 MB.</strong></p>

<p align="center">
  <a href="https://github.com/khanhphxm/terntable/actions/workflows/ci.yml"><img src="https://github.com/khanhphxm/terntable/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <img src="https://img.shields.io/badge/macOS-Apple_Silicon-24292f" alt="macOS on Apple Silicon">
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-1ed760" alt="MIT license"></a>
</p>

terntable is a Spotify client that lives inside Tern. It plays Spotify itself with
[librespot](https://github.com/librespot-org/librespot), an open-source Spotify Connect
implementation running in-process. Your library comes from Spotify's Web API.
It's a Rust binary with no browser engine.

**macOS, Linux and Windows (x86_64 MSVC). Needs Spotify Premium and Tern.**

![Home](.github/assets/home.png)

| | Memory | CPU |
|---|---|---|
| terntable, paused | ~11 MB | ~0% |
| terntable, playing | ~11 MB | ~1.3% |
| Spotify.app, paused | ~975 MB | |
| Spotify.app, playing | ~1,100 MB | |

<sub>Apple Silicon, macOS: physical footprint (`footprint`), CPU averaged over 30–60 s, 320 kbit/s
playback, EQ on.</sub>

## What it does

- **Home, search, Your Library** (playlists, albums, artists; filter, sort, list or grid), Liked Songs.
- **Artist pages** with Spotify's Popular list and play counts, monthly listeners, Fans also like, About.
- **Queue** you can reorder, trim, clear and jump around in.
- **Spotify Connect**: play here, or control your phone, speaker or TV from terntable.
- **Synced lyrics** from [LRCLIB](https://lrclib.net).
- **10-band EQ**, volume normalization, mono, balance, output device picker (macOS), sleep timer.

<table>
  <tr>
    <td><img src=".github/assets/artist.png" alt="Artist page"></td>
    <td><img src=".github/assets/album.png" alt="Album page"></td>
  </tr>
  <tr>
    <td><img src=".github/assets/eq.png" alt="Equalizer"></td>
    <td><img src=".github/assets/compact.png" alt="Narrow pane"></td>
  </tr>
</table>

## Install

```sh
brew install khanhphxm/tap/terntable
tern plugin link "$(brew --prefix)/share/terntable/plugin"
```

Open it with **terntable: Open** (⌘⌥M) or run `terntable` in any pane. Needs Tern 0.8 or newer.

### Build from source

You need Rust (stable), the platform's C build tools and Tern. Playback decoding is
built into the app; no container runtime or separate player image is needed.

```sh
git clone https://github.com/khanhphxm/terntable && cd terntable
cargo build --release -p terntable
scripts/codesign.sh target/release/terntable
cp target/release/terntable ~/.local/bin/
tern plugin link plugin
```

On Linux, install `build-essential`, `pkg-config` and `libdbus-1-dev` to build on
Debian/Ubuntu, then run `cargo build --release -p terntable` and copy the binary as
above (skip the macOS codesigning step). Audio uses `pw-cat` (`pipewire-bin`), falling
back to `pacat` (`pulseaudio-utils`) and then `aplay` (`alsa-utils`) when a utility is
not installed. Install at least one; the selected utility plays raw 44.1 kHz stereo
float PCM on the system default output. EQ, normalization, mono and balance run in
terntable on every platform; the output-device picker is macOS-only. After 1.5 seconds
without PCM, Linux kills the output subprocess and parks until audio arrives again.

On Windows, install Rust's `x86_64-pc-windows-msvc` toolchain and Visual Studio's C++
build tools, then run `cargo build --release -p terntable`. The executable is
`target\release\terntable.exe`; put it on your `PATH` and run `tern plugin link plugin`.
No separate audio utility is needed: shared-mode, event-driven WASAPI plays float32
stereo at the default render endpoint's mix rate, using a ~100 ms buffer. The client
stops and the render thread parks after 1.5 seconds without PCM. Resuming opens the
current default; an invalidated or stalled endpoint is reopened automatically. Changing
the default while the old endpoint still works takes effect on the next resume.

## First run

Setup has three steps, once:

1. **Sign in to play** in your browser with a Spotify Premium account.
2. **Create your Spotify app** in the [developer dashboard](https://developer.spotify.com/dashboard).
   Enable Web API, register `http://127.0.0.1:8898/callback` as the redirect URI, and paste its Client ID.
3. **Connect your library** by signing in with that app in the browser.

The saved sign-ins are reused on later launches. Start a song in terntable, or transfer
existing playback here from Spotify's device list.

## Keys

| Key | Action | | Key | Action |
|---|---|---|---|---|
| Space | Play / pause | | `/` | Search |
| ← → | Seek 10 s | | `g` / `l` | Home / Liked Songs |
| `n` / `p` | Next / previous | | `q` | Queue |
| `+` / `-` / `0` | Volume / mute | | `y` | Lyrics |
| `s` / `r` | Shuffle / repeat | | `e` / `,` | Equalizer / settings |

With the plugin linked: ⌘⌥Space play/pause, ⌘⌥← → previous/next, ⌘⌥↑ ↓ volume, from anywhere in Tern.

## Good to know

- **Queue edits** restart playback with the edited list, so the current song blips once.
- **What leaves your machine**: Spotify (playback, Web API, and the anonymous web player for
  play counts and artist details), LRCLIB (song title, artist, length) and Wikipedia/Wikidata
  (artist id). Each outside source can be switched off in Settings.
- **Player settings**: choose the Connect name, startup volume and audio cache size. **Off**
  disables the cache. Changing the name or cache size needs a restart; clearing the cache
  also restarts the player. Memory in Settings includes the in-process player.
- **Where things live**: on macOS, settings and the player's saved sign-in are in
  `~/.config/terntable`; covers and music are cached under `~/Library/Caches/terntable`.
  Linux uses `$XDG_CONFIG_HOME/terntable` and `$XDG_CACHE_HOME/terntable` (falling back
  to `~/.config/terntable` and `~/.cache/terntable`). Windows uses `%APPDATA%\terntable`
  for config/state and `%LOCALAPPDATA%\terntable` for cache. The Web API token uses
  macOS Keychain, Linux Secret Service or Windows Credential Manager; `TERNTABLE_HOME`
  overrides config/cache on every platform and keeps that token in a file instead.
  Settings links to the config and cache folders. **Sign out** forgets the player's
  sign-in; **Disconnect** forgets the library's sign-in.
- **Offline first paint**: account-scoped library snapshots cache the profile, playlists, Home
  and up to eight first-page listings (2 MiB total). They appear before network refreshes;
  signing out or disconnecting deletes them. A new authorization cannot reuse another
  account's snapshot.
- Tern reports clicks but not drags, so sliders (EQ, seek, volume) move by clicking.

## Development

```sh
cargo test -p terntable          # unit tests
scripts/lint.sh                  # rustfmt (nightly), clippy, docs, unused deps
```

To release, bump `version` in `Cargo.toml` and `plugin/plugin.toml`, merge, then run the
**Release** workflow from the Actions tab: it builds, signs and notarizes the app, publishes
`v<version>` on GitHub and updates the Homebrew formula.

`crates/terntable` is the app, `plugin/` is the Tern plugin, and `scripts/` contains
development, signing and benchmark helpers.

Unit tests cover external parsers, playback state transitions, concurrency and DSP behavior.
The previous playback-protocol UI harness has been retired; UI automation is being reworked.
Tests that need live audio hardware or a PipeWire daemon are not part of the unit suite.
Keep generated-text snapshots, default/serde round trips and mock request echoes out of it.

CI checks all host targets with `RUSTFLAGS='-D warnings' cargo clippy -p terntable --all-targets`.
Run Linux checks natively, or in an arm64 Debian container with `build-essential`,
`pkg-config` and `libdbus-1-dev`: `cargo build --release -p terntable`,
`cargo test -p terntable` and `cargo clippy -p terntable --all-targets -- -D warnings`.
The former `scripts/lint.sh linux` macOS cross-check has been removed: its synthetic
C compiler cannot build jemalloc. Installing only a Linux Rust target is not enough.

Cross-check Windows from macOS with LLVM/lld and `cargo-xwin` installed:

```sh
export PATH=/opt/homebrew/opt/llvm/bin:/opt/homebrew/opt/lld/bin:$PATH
export CARGO_TARGET_DIR=target/win
cargo xwin build --release -p terntable --target x86_64-pc-windows-msvc
cargo xwin test -p terntable --target x86_64-pc-windows-msvc --no-run
cargo xwin clippy -p terntable --target x86_64-pc-windows-msvc --all-targets -- -D warnings
```

Cross-compilation cannot exercise WASAPI playback/device recovery, Windows Credential
Manager, the browser redirect or Tern's Windows input/resize/close handling; check those
on a Windows machine. The Unix signal waiter is not used on Windows: Tern supplies
resize and Ctrl+C input, and closing the pane closes the SDK input and tears down playback.

### Playback benchmarks

`scripts/bench/play.sh` controls a Spotify Connect device through the Web API.
Python 3 and a current `SPOTIFY_ACCESS_TOKEN` environment variable are required; the
token needs `user-read-playback-state` and `user-modify-playback-state` scopes. The
helper does not read or change terntable's saved sign-in. `--list` only lists devices,
while playback commands can transfer the active player and interrupt another device:

```sh
scripts/bench/play.sh --list
scripts/bench/play.sh --device-name terntable --resume
```

On macOS, `scripts/bench/measure.sh -n 30 PID` measures the given process's CPU over
the interval and reports a physical-memory snapshot with `footprint`. Pass the PID
of the running terntable process; the helper does not control playback.


---

terntable is an independent project, not affiliated with Spotify. Spotify is a
trademark of Spotify AB. [MIT](LICENSE).
