#!/bin/bash
# Package an existing release executable; never build or launch the application.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
binary=${1:-"$root/target/release/turntable"}
app=${2:-"$root/dist/turntable.app"}
version=${TURNTABLE_VERSION:-$(cargo metadata --no-deps --format-version=1 --manifest-path "$root/Cargo.toml" | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "turntable"))')}
identity=${CODESIGN_IDENTITY:--}

if [[ ! -x "$binary" ]]; then
	printf 'Missing release executable: %s\nBuild with cargo build --release -p turntable first.\n' "$binary" >&2
	exit 1
fi
if [[ "$app" != *.app ]]; then
	printf 'Bundle output must end in .app: %s\n' "$app" >&2
	exit 1
fi
# Regenerate missing/stale assets only; ordinary packaging needs no renderer.
if [[ ! -f "$root/packaging/icons/turntable.icns" || "$root/packaging/icon.svg" -nt "$root/packaging/icons/turntable.icns" ]]; then
	python3 "$root/packaging/generate-icons.py" --icns
fi
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
# Remove the old signature before replacing signed contents on repeated runs.
if [[ -d "$app/Contents/_CodeSignature" ]]; then
	rm -r "$app/Contents/_CodeSignature"
fi
install -m 755 "$binary" "$app/Contents/MacOS/turntable"
install -m 644 "$root/packaging/Info.plist" "$app/Contents/Info.plist"
install -m 644 "$root/packaging/icons/turntable.icns" "$app/Contents/Resources/turntable.icns"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $version" "$app/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $version" "$app/Contents/Info.plist"
printf 'APPL????' > "$app/Contents/PkgInfo"
if [[ "$identity" == - ]]; then
	codesign --force --sign - "$app"
else
	codesign --force --options runtime --timestamp --sign "$identity" "$app"
fi
plutil -lint "$app/Contents/Info.plist"
codesign --verify --strict "$app"
printf 'Bundled %s\n' "$app"
