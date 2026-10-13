#!/bin/bash
# Generate the committed cross-platform license bundle without building the app.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
[[ $(nice -n 19 env CARGO_BUILD_JOBS=4 cargo about --version) == 'cargo-about 0.9.2' ]] || {
	printf 'Install cargo-about 0.9.2 with --locked --features cli first.\n' >&2
	exit 1
}
output=$(mktemp)
metadata=$(mktemp)
trap 'rm -f "$output" "$metadata"' EXIT
nice -n 19 env CARGO_BUILD_JOBS=4 cargo about generate --locked --workspace --fail \
	--manifest-path "$root/Cargo.toml" \
	--config "$root/about.toml" --output-file "$output" "$root/packaging/notices.hbs"
nice -n 19 env CARGO_BUILD_JOBS=4 cargo metadata --locked --format-version=1 \
	--manifest-path "$root/Cargo.toml" > "$metadata"
# Cargo license expressions do not describe embedded assets or copied snippets.
# Keep their original permission texts verbatim, including upstream attribution.
append_notice() {
	{
		printf '\n================================================================================\n%s\n\n' "$1"
		cat "$root/$2"
		printf '\n'
	} >> "$output"
}
append_notice 'GPUI/Zed attribution (Apache-2.0); Turntable modifications: vendor/gpui-pre/TURNTABLE.md and vendor/gpui-pre-*/TURNTABLE.md' vendor/gpui-pre-apple/LICENSE-APACHE
append_notice 'librespot 0.8.0 (MIT), including all four vendored crates' vendor/librespot-core/LICENSE
append_notice 'Taffy 0.13.0 (MIT)' vendor/taffy/LICENSE
append_notice 'num-traits polyfill copied in vendor/taffy/src/util/sys.rs (MIT)' vendor/taffy/LICENSE-num-traits-MIT
append_notice 'Microsoft Terminal gamma-correction snippet in GPUI platform.rs (MIT); upstream 1283c0f5b99a2961673249fa77c6b986efb5086c' packaging/licenses/Microsoft-Terminal-MIT
append_notice 'display-link (Brainium LLC) CoreVideo bindings in GPUI display_link.rs (MIT); https://github.com/BrainiumLLC/display-link' packaging/licenses/display-link-MIT
python3 "$root/packaging/native-notices.py" "$metadata" "$output"
install -m 644 "$output" "$root/THIRD_PARTY_NOTICES"
printf 'Generated %s\n' "$root/THIRD_PARTY_NOTICES"
