#!/bin/bash
# CI only; local packaging never submits an app to Apple implicitly.
set -euo pipefail
artifact=${1:?usage: notarize-macos.sh <app-or-dmg>}
if [[ ${CODESIGN_IDENTITY:--} == - ]]; then
	printf 'WARNING: ad-hoc signature; notarization and stapling are skipped.\n' >&2
	exit 0
fi

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
if [[ -n ${NOTARY_KEY:-} && -n ${NOTARY_KEY_ID:-} && -n ${NOTARY_ISSUER_ID:-} ]]; then
	printf '%s' "$NOTARY_KEY" | base64 --decode > "$stage/key.p8"
	chmod 600 "$stage/key.p8"
	credentials=(--key "$stage/key.p8" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER_ID")
	printf 'Notarizing with App Store Connect API key.\n'
elif [[ -n ${APPLE_ID:-} && -n ${APPLE_TEAM_ID:-} && -n ${APPLE_APP_PASSWORD:-} ]]; then
	credentials=(--apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD")
	printf 'Notarizing with Apple ID fallback.\n'
else
	printf 'WARNING: no complete notarization credentials; shipping signed but unnotarized artifacts.\n' >&2
	exit 0
fi

submission=$artifact
if [[ "$artifact" == *.app ]]; then
	submission="$stage/notarize.zip"
	ditto -c -k --keepParent "$artifact" "$submission"
fi
xcrun notarytool submit "$submission" "${credentials[@]}" --wait --timeout 30m
xcrun stapler staple "$artifact"
xcrun stapler validate "$artifact"
if [[ "$artifact" == *.app ]]; then
	spctl --assess --type execute --verbose "$artifact"
fi
