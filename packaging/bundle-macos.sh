#!/bin/bash
# Package an existing universal release executable; never build or launch the app.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
binary=${1:-"$root/target/universal/release/turntable"}
app=${2:-"$root/dist/Turntable.app"}
version=${TURNTABLE_VERSION:-$(cargo metadata --no-deps --format-version=1 --manifest-path "$root/Cargo.toml" | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "turntable"))')}
identity=${CODESIGN_IDENTITY:--}

if [[ ! -x "$binary" ]]; then
	printf 'Missing universal executable: %s\nBuild both macOS targets and combine them with lipo first.\n' "$binary" >&2
	exit 1
fi
if [[ "$app" != *.app ]]; then
	printf 'Bundle output must end in .app: %s\n' "$app" >&2
	exit 1
fi
for arch in arm64 x86_64; do
	lipo "$binary" -verify_arch "$arch"
done
# Apple's bundle versions must be numeric, even for a SemVer prerelease.
bundle_version=${version%%[-+]*}
# Regenerate missing/stale assets only; ordinary packaging needs no renderer.
if [[ ! -f "$root/packaging/icons/turntable.icns" || "$root/packaging/icon.svg" -nt "$root/packaging/icons/turntable.icns" ]]; then
	python3 "$root/packaging/generate-icons.py" --icns
fi
# Recreate the explicit output so stale resources and old filename casing cannot
# leak into a release ZIP on case-insensitive macOS filesystems.
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
install -m 755 "$binary" "$app/Contents/MacOS/turntable"
install -m 644 "$root/packaging/Info.plist" "$app/Contents/Info.plist"
install -m 644 "$root/packaging/icons/turntable.icns" "$app/Contents/Resources/turntable.icns"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $bundle_version" "$app/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $bundle_version" "$app/Contents/Info.plist"
printf 'APPL????' > "$app/Contents/PkgInfo"
# This is not a sandboxed app: playback/network need no hardened-runtime exceptions.
if [[ "$identity" == - ]]; then
	printf 'WARNING: no Developer ID configured; signing ad-hoc (not Gatekeeper-trusted).\n' >&2
	codesign --force --options runtime --sign - "$app"
else
	codesign --force --options runtime --timestamp --sign "$identity" "$app"
fi
plutil -lint "$app/Contents/Info.plist"
codesign --verify --strict "$app"
printf 'Bundled %s\n' "$app"
