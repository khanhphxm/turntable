#!/bin/bash
# Create a distributable DMG after the app has been signed and (optionally) stapled.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
app=${1:-"$root/dist/Turntable.app"}
version=${TURNTABLE_VERSION:?Set TURNTABLE_VERSION to the release version}
output=${2:-"$root/dist/turntable-$version-macos-universal.dmg"}
identity=${CODESIGN_IDENTITY:--}

codesign --verify --strict "$app"
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
ditto "$app" "$stage/Turntable.app"
ln -s /Applications "$stage/Applications"
mkdir -p "$(dirname "$output")"
hdiutil create -ov -volname Turntable -srcfolder "$stage" -format UDZO "$output"
if [[ "$identity" != - ]]; then
	codesign --force --timestamp --sign "$identity" "$output"
	codesign --verify --strict "$output"
fi
hdiutil verify "$output"
printf 'Bundled %s\n' "$output"
