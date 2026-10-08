#!/bin/bash
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
binary=${1:-"$root/target/release/turntable"}
version=${TURNTABLE_VERSION:?Set TURNTABLE_VERSION to the release version}
arch=${TURNTABLE_ARCH:-$(uname -m)}
case "$arch" in
	x86_64) deb_arch=amd64 ;;
	aarch64) deb_arch=arm64 ;;
	*) printf 'Unsupported Linux architecture: %s\n' "$arch" >&2; exit 1 ;;
esac
[[ -x "$binary" ]] || { printf 'Missing executable: %s\n' "$binary" >&2; exit 1; }
[[ -f "$root/packaging/icons/256x256/apps/turntable.png" ]] || { printf 'Generate packaging icons first.\n' >&2; exit 1; }
name="turntable-$version-linux-$arch"
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/$name/bin" "$stage/$name/share/applications" "$stage/$name/share/icons/hicolor"
install -m 755 "$binary" "$stage/$name/bin/turntable"
install -m 644 "$root/packaging/turntable.desktop" "$stage/$name/share/applications/turntable.desktop"
for size in 16 24 32 48 64 128 256 512 1024; do
	mkdir -p "$stage/$name/share/icons/hicolor/${size}x${size}/apps"
	install -m 644 "$root/packaging/icons/${size}x${size}/apps/turntable.png" "$stage/$name/share/icons/hicolor/${size}x${size}/apps/turntable.png"
done
mkdir -p "$stage/$name/share/icons/hicolor/scalable/apps"
install -m 644 "$root/packaging/icon.svg" "$stage/$name/share/icons/hicolor/scalable/apps/turntable.svg"
install -m 644 "$root/packaging/INSTALL-linux.md" "$stage/$name/INSTALL.md"
install -m 644 "$root/LICENSE" "$stage/$name/LICENSE"
mkdir -p "$root/dist"
tar -C "$stage" -czf "$root/dist/$name.tar.gz" "$name"
printf 'Bundled %s\n' "$root/dist/$name.tar.gz"

# Ubuntu 24.04 packages: include runtime-loaded GUI/audio libraries as well as
# ELF-linked dependencies (GPUI loads several platform libraries dynamically).
deb="$stage/deb"
mkdir -p "$deb/DEBIAN" "$deb/usr/share/doc/turntable"
cp -R "$stage/$name/bin" "$deb/usr/bin"
cp -R "$stage/$name/share/." "$deb/usr/share/"
install -m 644 "$root/LICENSE" "$deb/usr/share/doc/turntable/copyright"
install -m 644 "$root/packaging/INSTALL-linux.md" "$deb/usr/share/doc/turntable/README.md"
deb_version=${version/-/~}
installed_size=$(du -sk "$deb/usr" | cut -f1)
cat > "$deb/DEBIAN/control" <<EOF
Package: turntable
Version: $deb_version
Section: sound
Priority: optional
Architecture: $deb_arch
Maintainer: Khanh Pham <khanhphxm@users.noreply.github.com>
Installed-Size: $installed_size
Depends: libc6 (>= 2.39), libgcc-s1, libstdc++6, libasound2t64, libpipewire-0.3-0, libdbus-1-3, libssl3t64, libxkbcommon0, libxkbcommon-x11-0, libvulkan1, libwayland-client0, libwayland-cursor0, libwayland-egl1, libx11-6, libx11-xcb1, libxcb1, libxcb-shape0, libxcb-xfixes0, libxcb-randr0, libxcb-render0, libxcb-xkb1, libxrandr2, libxi6, libxcursor1, libxinerama1, libxrender1, libxfixes3, libegl1, libgl1, libfontconfig1, libfreetype6
Recommends: mesa-vulkan-drivers | vulkan-icd
Homepage: https://github.com/khanhphxm/turntable
Description: Lightweight native Spotify player
 A GPUI desktop Spotify client using librespot. Requires Spotify Premium,
 a Wayland or X11 session, an audio output, and a working Vulkan GPU driver.
EOF
dpkg-deb --root-owner-group --build "$deb" "$root/dist/$name.deb"
printf 'Bundled %s\n' "$root/dist/$name.deb"
