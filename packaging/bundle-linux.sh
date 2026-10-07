#!/bin/bash
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
binary=${1:-"$root/target/release/turntable"}
version=${TURNTABLE_VERSION:?Set TURNTABLE_VERSION to the release version}
arch=${TURNTABLE_ARCH:-$(uname -m)}
case "$arch" in
	x86_64|aarch64) ;;
	*) printf 'Unsupported Linux architecture: %s\n' "$arch" >&2; exit 1 ;;
esac
[[ -x "$binary" ]] || { printf 'Missing executable: %s\n' "$binary" >&2; exit 1; }
if [[ ! -f "$root/packaging/icons/turntable.ico" || "$root/packaging/icon.svg" -nt "$root/packaging/icons/turntable.ico" ]]; then
	python3 "$root/packaging/generate-icons.py"
fi
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
