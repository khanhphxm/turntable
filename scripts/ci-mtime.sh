#!/bin/bash
# Makes a restored target/ usable after a fresh checkout. Cargo decides whether a path crate
# (ours and the vendored patches) needs rebuilding by comparing file modification times, and a
# checkout gives every file a new one, so a restored build would be redone almost entirely.
# Every tracked file gets a fixed old time; files changed since the commit the cache was built
# from get the current time, so exactly those (and what depends on them) rebuild.
#
#   scripts/ci-mtime.sh [target-dir]    (default: target)
set -euo pipefail
cd "$(dirname "$0")/.."
stamp=${1:-target}/.ci-source-commit

git ls-files -z | xargs -0 touch -h -d '2000-01-01T00:00:00'
if [ -f "$stamp" ] && base=$(cat "$stamp") && { git cat-file -e "$base^{commit}" 2>/dev/null || git fetch --quiet --depth=1 origin "$base"; }; then
	changed=$(git diff --name-only --diff-filter=d "$base" HEAD)
	echo "Cache built from $base; $(printf '%s' "$changed" | grep -c . || true) files changed since"
	printf '%s\n' "$changed" | while IFS= read -r file; do
		[ -n "$file" ] && touch -h "$file"
	done
else
	# No cache, or its commit is gone: every file counts as changed.
	echo "No usable restored build"
	git ls-files -z | xargs -0 touch -h
fi
mkdir -p "$(dirname "$stamp")"
git rev-parse HEAD > "$stamp"
