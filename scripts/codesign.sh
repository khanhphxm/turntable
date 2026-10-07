#!/bin/sh
# Signs a terntable binary with the hardened runtime. macOS only.
#
#   scripts/codesign.sh <binary>
#
# CODESIGN_IDENTITY  signing identity. Unset: a "Developer ID Application" identity in the
#                    keychain, else an "Apple Development" one, else
#                    "-" (ad-hoc). A real identity matters beyond distribution:
#                    the Spotify sign-in lives in the Keychain, which trusts a
#                    binary by its signature. A certificate signature survives
#                    rebuilds; an ad-hoc one changes with every build, and macOS
#                    then asks again whether terntable may read its own item.
#                    For distribution use "Developer ID Application: ..." and
#                    notarize the result.
set -eu

bin=${1:?usage: codesign.sh <binary>}

if [ -z "${CODESIGN_IDENTITY:-}" ]; then
	ids=$(security find-identity -v -p codesigning 2>/dev/null)
	CODESIGN_IDENTITY=$(printf '%s\n' "$ids" | grep -Eo '"Developer ID Application: [^"]*"' | head -n 1 | tr -d '"')
	[ -n "$CODESIGN_IDENTITY" ] ||
		CODESIGN_IDENTITY=$(printf '%s\n' "$ids" | grep -Eo '"Apple Development: [^"]*"' | head -n 1 | tr -d '"')
fi
identity=${CODESIGN_IDENTITY:--}

if [ "$identity" = "-" ]; then
	set -- --force --sign - --identifier dev.terntable --options runtime
else
	# A secure timestamp only for distribution builds (it needs the network).
	case $identity in "Developer ID Application"*) stamp=--timestamp ;; *) stamp=--timestamp=none ;; esac
	set -- --force --sign "$identity" --identifier dev.terntable --options runtime $stamp
fi
exec codesign "$@" "$bin"
