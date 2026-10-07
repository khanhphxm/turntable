#!/bin/sh
# Cargo runner (see .cargo/config.toml): signs the executable for stable Keychain access,
# then runs it. Cargo relinks it after every build, so sign only when our identifier is absent.
set -eu

exe=${1:?usage: run-signed.sh <executable> [args...]}
here=$(cd "$(dirname "$0")" && pwd)

if ! codesign -d "$exe" 2>&1 | grep -q '^Identifier=dev\.turntable$'; then
	"$here/codesign.sh" "$exe" >/dev/null 2>&1
fi
exec "$@"
