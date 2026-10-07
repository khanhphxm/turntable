# turntable

A lightweight, native Spotify desktop player for macOS, Linux and Windows.
Built with [GPUI](https://www.gpui.rs/) and an in-process
[librespot](https://github.com/librespot-org/librespot) player—not a browser wrapper.
**Spotify Premium is required for playback.**

<!-- screenshot: home -->

## Features

- Browse Home, your library, search results, artists, albums and playlists.
- Play on this computer or use Spotify Connect devices.
- View and manage the playback queue.
- Follow synced lyrics from [LRCLIB](https://lrclib.net/), when available.
- Shape the sound with a 10-band equalizer and presets.
- Loudness normalization with Off, Quiet, Normal and Loud settings; optional mono audio.
- Native media keys and Now Playing integration: macOS Control Center,
  Linux MPRIS and Windows system media controls.
- A **✓** marks songs already in Liked Songs or one of your playlists.
- Optional artist biographies from Wikipedia.

<!-- screenshot: album -->
<!-- screenshot: lyrics -->
<!-- screenshot: equalizer -->

## Install

### Homebrew

The packaging targets the `khanhphxm/tap` tap:

```sh
# macOS (Apple Silicon)
brew install --cask khanhphxm/tap/turntable

# Linux (x86_64 or aarch64)
brew install khanhphxm/tap/turntable
```

**Distribution status:** the repository currently contains cask/formula templates,
not proof of a published tap package. These commands require the filled-in tap
entries and publicly accessible release assets. See
[packaging](packaging/README.md#homebrew-tap-templates) for publication details.

### Release archives

When published, download the archive for your platform from
[GitHub Releases](https://github.com/khanhphxm/turntable/releases):

- **macOS arm64:** extract the ZIP and move `turntable.app` to Applications.
  Ad-hoc signed builds are not Gatekeeper-trusted; trusted distribution requires
  Developer ID signing and notarization.
- **Linux x86_64 / aarch64:** extract the tarball and follow its `INSTALL.md` or
  [the Linux installation guide](packaging/INSTALL-linux.md). Packaged binaries
  target Ubuntu 24.04 / glibc 2.39 and need a graphical session and Vulkan driver.
- **Windows x86_64:** extract the ZIP and follow
  [the Windows installation guide](packaging/INSTALL-windows.md).

Archive contents, signing and bundling instructions are documented in
[packaging/](packaging/README.md).

## Build from source

Use **Rust stable 1.90 or newer** (the workspace's `rust-version` in
[`Cargo.toml`](Cargo.toml)), plus your platform's native C/C++ build tools.

```sh
git clone https://github.com/khanhphxm/turntable.git
cd turntable
cargo build --release -p turntable
cargo run --release -p turntable
```

On Linux, install the platform libraries listed in
[packaging/INSTALL-linux.md](packaging/INSTALL-linux.md). Building also needs
corresponding development headers, Clang/libclang, CMake, Ninja and pkg-config;
see [packaging](packaging/README.md#linux). Audio uses `pw-cat`, falling back to
`pacat`, then `aplay` when the preceding executable is absent: install at least
one (`pipewire-bin`, `pulseaudio-utils` or `alsa-utils` on Debian/Ubuntu).

On Debian/Ubuntu, source builds also need development packages including
`libfontconfig1-dev`, `libfreetype6-dev`, `libxkbcommon-dev`,
`libxkbcommon-x11-dev`, `libxcb-xkb-dev`, `libasound2-dev`, `libdbus-1-dev`,
`libssl-dev` and `libwayland-dev`.

On macOS, Cargo's runner signs the executable automatically for stable Keychain
access. To package an application bundle, use the scripts in
[packaging/](packaging/README.md). Windows builds use the native MSVC toolchain.

The workspace separates the GPUI application (`crates/turntable`) from the
UI-independent playback, DSP and library engine (`crates/turntable-core`).
The retired terminal application in `old/tern` is not part of the build.

For development, build and lint both workspace crates:

```sh
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo +nightly fmt --all
```

To cross-check Windows compilation from macOS with LLVM/lld and `cargo-xwin`:

```sh
PATH="/opt/homebrew/opt/llvm/bin:/opt/homebrew/opt/lld/bin:$PATH" \
CARGO_TARGET_DIR=target/win \
cargo xwin check --workspace --all-targets --target x86_64-pc-windows-msvc
```

Compilation checks do not exercise native audio, credential storage, browser
sign-in or media controls; those require testing on the target platform.

### Core library reads

The library worker shares the playback engine's authenticated librespot Session.
Playlist pages use Playlist4; album pages, artist discography/top tracks and
track rows use extended metadata batches of up to 100 entities. Liked Songs and
saved-item membership come from the complete collection; playlists come from
the decorated rootlist. These reads do not use the quota-limited Web API once
the matching playback account connects. Before that connection, the worker
uses the existing Web API account, without holding up browsing or changing
the host-facing `Response`/listing models.

The Web API remains for search, personalized Home history/top shelves, remote
playback controls and library/playlist writes. Optional web-player data still
adds play counts, listener statistics, credits and cover colors that Session
metadata does not supply. Account-scoped disk snapshots retain their existing
stale-while-revalidate behavior; a Session attachment drops only in-memory page
answers, so subsequent fresh pages come from the player.

### Core membership API

Hosts render the saved-track check with
`controller.state().added("spotify:track:...") -> bool`. It is the union of
**Liked Songs and playlists owned by the signed-in user**; followed playlists,
collaborators' playlists, episodes and local files do not count. The query uses
immutable worker-delivered membership caches, without I/O or allocation. An
unknown track returns `false` until membership arrives. The worker wakes the
host; call `Controller::pump()` and redraw as for other library responses.

`Msg::ToggleLike` still toggles only the now-playing track's Liked Songs status,
not the union. For row actions, dispatch `Msg::AddToLiked(String)` or
`Msg::RemoveFromLiked(String)` with a Spotify track URI. Removing a like leaves
the added check on while that track remains in any owned playlist.

Owned-playlist membership uses the engine's connected librespot Session and
Playlist4, off the UI thread. Startup and successful playlist add/remove/create
operations schedule a refresh after pending library requests. Account-scoped
binary caches under `cache/playlist-index/<sha256(username)>.plidx` retain
playlist revisions and duplicate occurrences. An unchanged refresh fetches
only rootlist pages, not playlist contents; only new, changed or locally edited
owned playlists are refetched. Writes are atomic, with private Unix permissions.
Liked Songs membership reuses the Session's complete collection, loaded pages
and cached per-track answers; no per-row Web API contains requests are needed.

## Privacy and external services

turntable needs network access to Spotify for sign-in, playback, Spotify Connect,
library browsing and artwork. It also uses optional external sources:

| Service | What is requested | Settings control |
| --- | --- | --- |
| LRCLIB | Lyrics matched using song title, artist and duration | Sources → Lyrics |
| Wikidata / Wikipedia | Artist identification and biography text | Sources → Artist bios |
| Spotify's web player | Additional artist pages, listeners and play counts | Sources → Spotify's web player |

All three optional sources are enabled by default and can be disabled in
**Settings → Sources**. Disabling them does not disable Spotify playback or
library requests. As with other direct network requests, these services receive
your network address.

Settings and account-scoped library snapshots are stored locally. The Web API
token uses the OS credential store (Keychain, Secret Service or Credential
Manager); covers use a separate disk cache. Configuration lives under
`$XDG_CONFIG_HOME/turntable` or `~/.config/turntable` on macOS/Linux, and
`%APPDATA%\turntable` on Windows. Caches live under
`~/Library/Caches/turntable` on macOS, `$XDG_CACHE_HOME/turntable` or
`~/.cache/turntable` on Linux, and `%LOCALAPPDATA%\turntable` on Windows.

## Memory goals

The goal is a small native player, around **50 MB** rather than the footprint of
a browser-based desktop app. The current reference observation is approximately
**43 MB after settling, without playback**. This is not a playback measurement,
a startup-peak measurement or a guarantee across platforms and libraries.
Artwork, navigation and playback can change memory use. This number will be
updated as representative measurements become available.

## FAQ

**Do I need Spotify Premium?**  
Yes. Local playback through librespot requires a Premium account.

**Does Spotify's desktop app need to be running?**  
No. turntable includes its own player. Spotify Connect also lets you control
other available devices. Starting playback here can transfer playback away from
another device using the same account.

**Why are lyrics missing or not synced?**  
LRCLIB does not have every recording. turntable uses synced lyrics when found,
can show plain lyrics instead, and may find no matching lyrics. Check that
Settings → Sources → Lyrics is enabled.

**What does the ✓ beside a song mean?**  
It means the song is in Liked Songs or one of your playlists, not that it has
been downloaded for offline playback.

**Why does a Linux archive not run on my distribution?**  
Release archives target glibc 2.39 and require the graphical, GPU and audio
libraries in the [Linux guide](packaging/INSTALL-linux.md). Build from source
on older systems rather than assuming the packaged executable is compatible.

**Is this an official Spotify application?**  
No. turntable is independent and is not affiliated with or endorsed by Spotify.
Spotify is a trademark of Spotify AB.

## License

[MIT](LICENSE).
