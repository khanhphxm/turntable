<p align="center">
  <img src="packaging/icon.svg" width="128" height="128" alt="Turntable icon">
</p>

<h1 align="center">Turntable</h1>

<p align="center">
  A fast, native Spotify player for macOS, Linux and Windows.<br>
  Built with <a href="https://www.gpui.rs/">GPUI</a> and <a href="https://github.com/librespot-org/librespot">librespot</a>. Requires Spotify Premium.
</p>

<!-- screenshot: home -->

## Features

- Your whole library: playlists, folders, albums, artists and Liked Songs
- Search
- Create and edit playlists
- Now Playing panel, full-screen view and miniplayer
- Synced lyrics
- Spotify Connect: play here or on your other devices
- Equalizer, loudness normalization and crossfade
- Media keys and keyboard shortcuts

<!-- screenshot: album -->
<!-- screenshot: lyrics -->

## Install

```sh
brew install --cask khanhphxm/tap/turntable   # macOS
brew install khanhphxm/tap/turntable          # Linux
```

Or download the macOS disk image, Windows installer or Linux package from
[Releases](https://github.com/khanhphxm/turntable/releases).

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

Turntable talks only to Spotify. Your sign-in stays on your computer.

## License

[MIT](LICENSE). Icons from [Lucide](https://lucide.dev) (ISC).
Turntable is not affiliated with, endorsed or sponsored by Spotify. Spotify is a trademark of
Spotify AB.
