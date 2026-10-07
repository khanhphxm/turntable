# turntable

A lightweight, native Spotify player for macOS, Linux and Windows.
Built with [GPUI](https://www.gpui.rs/) and [librespot](https://github.com/librespot-org/librespot).
Spotify Premium required.

<!-- screenshot: home -->

## Features

- Home, search, your library, artists, albums and playlists
- Spotify Connect: play here or on your other devices
- Queue, synced lyrics ([LRCLIB](https://lrclib.net/)), artist bios (Wikipedia)
- 10-band equalizer, loudness normalization, mono audio
- Media keys and system Now Playing controls
- A **✓** on songs already in Liked Songs or one of your playlists

<!-- screenshot: album -->
<!-- screenshot: lyrics -->

## Install

```sh
brew install --cask khanhphxm/tap/turntable   # macOS
brew install khanhphxm/tap/turntable          # Linux
```

Or download an archive from [Releases](https://github.com/khanhphxm/turntable/releases).

## Build from source

Requires Rust 1.90+.

```sh
git clone https://github.com/khanhphxm/turntable.git
cd turntable
cargo run --release -p turntable
```

**Linux** also needs (Debian/Ubuntu names):

```sh
sudo apt install clang cmake pkg-config libssl-dev libdbus-1-dev libasound2-dev \
  libfontconfig1-dev libfreetype6-dev libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev libxcb-xkb-dev
```

**Windows** builds with the MSVC toolchain.

## Privacy

Besides Spotify, turntable may contact LRCLIB (lyrics), Wikipedia (artist bios) and Spotify's
web player (play counts, listeners). Each can be turned off in **Settings → Sources**.

## License

[MIT](LICENSE). Not affiliated with Spotify.
