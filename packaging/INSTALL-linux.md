# Linux installation

These archives contain a dynamically linked native desktop app, not a terminal
plugin. Use the archive matching your CPU: `linux-x86_64` or `linux-aarch64`.
Release builds use Ubuntu 24.04 (glibc 2.39); older distributions are not supported
by these binaries. Build from source on older systems instead.

For the matching Ubuntu 24.04 `.deb`, use
`sudo apt install ./turntable-<version>-linux-<arch>.deb`; apt installs its runtime
dependencies and desktop entry. Remove it with `sudo apt remove turntable`.
The package architecture is `amd64` for x86_64 or `arm64` for aarch64. The archive
instructions below are for portable/per-user installations.

## Runtime dependencies

A working graphical session (Wayland or X11), a Vulkan-capable GPU/driver, and an
audio device are required. Install your GPU vendor's Vulkan driver; Mesa's Vulkan
drivers work for supported Intel/AMD GPUs. A headless server is not sufficient.

On Ubuntu 24.04 / recent Debian:

```sh
sudo apt install libasound2t64 libpipewire-0.3-0 pipewire-bin libxkbcommon0 libxkbcommon-x11-0 \
  libvulkan1 mesa-vulkan-drivers libwayland-client0 libwayland-cursor0 \
  libwayland-egl1 libx11-6 libxcb1 libxcb-shape0 libxcb-xfixes0 libxrandr2 \
  libxi6 libxcursor1 libxinerama1 libxrender1 libxfixes3 libegl1 libgl1 \
  libfontconfig1 libfreetype6 libdbus-1-3 libssl3t64
```

Older Debian uses `libasound2` / `libssl3` rather than the `t64` package names.
Fedora/Arch equivalents are ALSA (`alsa-lib`), PipeWire, libxkbcommon (including
X11 support), Vulkan loader + GPU drivers, Wayland, X11/XCB client libraries,
EGL/OpenGL, Fontconfig, FreeType, D-Bus and OpenSSL. PipeWire should run in the
user session; ALSA provides the device backend. D-Bus is needed for media keys
and session integration. X11 libraries remain required even in a Wayland session
because GPUI includes both platform backends.

Playback also needs an audio utility on `PATH`: Turntable tries `pw-cat`
(`pipewire-bin`), then `pacat` (`pulseaudio-utils`), then `aplay` (`alsa-utils`)
when the preceding executable is absent. A PipeWire/PulseAudio user session or
working ALSA output must be available; installing only the shared audio libraries
is not enough. Playback follows the session's default sink. If the utility exits
or the output stops draining, Turntable retries the output without requiring an
application restart.

## Per-user install

Extract the archive, enter its directory, then:

```sh
mkdir -p "$HOME/.local/bin" "$HOME/.local/share/applications" "$HOME/.local/share/icons"
install -m 755 bin/turntable "$HOME/.local/bin/turntable"
install -m 644 share/applications/turntable.desktop "$HOME/.local/share/applications/"
cp -R share/icons/hicolor "$HOME/.local/share/icons/"
command -v update-desktop-database >/dev/null && update-desktop-database "$HOME/.local/share/applications"
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -f -t "$HOME/.local/share/icons/hicolor"
```

Put `$HOME/.local/bin` on your session's `PATH` (log out/in after changing it).
Launch **Turntable** from your desktop's application menu. For a system install,
copy `bin/turntable` to `/usr/local/bin` and `share/*` into `/usr/local/share`.

Uninstall by removing the installed executable, `turntable.desktop`, and only
`turntable.png` / `turntable.svg` from the hicolor `apps` directories. Do not
remove the shared hicolor tree, which also contains other applications' icons.

## Window decorations

Turntable requests client-side decorations on X11 and Wayland. When the window
manager grants them, the top bar provides minimize, maximize/restore and close
buttons at its right edge (also on the welcome screen). Drag an empty part of
the top bar to move the window, double-click to maximize/restore, and use the
frame's edges or corners to resize. Minimize/maximize are shown only when the
window manager supports those actions.

X11 client-side decorations require a compositor and a window manager supporting
`_GTK_FRAME_EXTENTS`. If they are unavailable, or a Wayland compositor insists
on server-side decorations, Turntable keeps the window manager's native frame
and does not draw a duplicate set of caption buttons.
