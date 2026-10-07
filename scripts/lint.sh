#!/bin/sh
# Static checks for the whole workspace. Every step runs even when an earlier one fails, and the
# exit status is 0 only when all of them are clean, so one run lists everything to fix.
#
#   scripts/lint.sh                  all steps
#   scripts/lint.sh fmt deps         only those steps
#   scripts/lint.sh -h
#
# Steps
#   fmt     rustfmt --check with a nightly rustfmt (rustfmt.toml uses unstable options; apply with
#           `cargo +nightly fmt`)
#   clippy  every crate for this machine's target (lib, bins, tests), warnings are errors; the lint
#           set is [workspace.lints] in Cargo.toml and clippy.toml
#           run natively on each supported OS (or in Docker); jemalloc needs a real C compiler
#           the former macOS-to-Linux check with a no-op compiler is no longer supported
#   docs    rustdoc with warnings as errors: broken intra-doc links, bare URLs, bad HTML
#   deps    cargo machete: dependencies no source uses (`cargo install cargo-machete`)
#   ban     no #[allow(..)] (use #[expect(lint, reason = "...")]), no #[ignore]d test, no lint set to
#           "allow" in a manifest
#
# CARGO_TARGET_DIR is honoured. RUSTFMT_TOOLCHAIN (default nightly) picks the rustfmt.
set -u

cd "$(dirname "$0")/.." || exit 2

ALL="fmt clippy docs deps ban"
RUSTFMT_TOOLCHAIN=${RUSTFMT_TOOLCHAIN:-nightly}

case ${1:-} in
	-h|--help) sed -n '2,24p' "$0"; exit 0 ;;
esac
steps=${*:-$ALL}
for s in $steps; do
	case " $ALL " in
		*" $s "*) ;;
		*) echo "lint.sh: unknown step '$s' (steps: $ALL)" >&2; exit 2 ;;
	esac
done

failed=""

mark_failed() {
	case " $failed " in
		*" $1 "*) ;;
		*) failed="$failed $1" ;;
	esac
}

# step NAME COMMAND [ARGS..]: runs the command under a header and records a failure.
step() {
	name=$1; shift
	printf '\n==> %s: %s\n' "$name" "$*"
	if ! "$@"; then
		mark_failed "$name"
		printf '<== %s FAILED\n' "$name"
	fi
}

# unavailable NAME WHY: a step that cannot run is a failure, never a silent pass.
unavailable() {
	name=$1; shift
	printf '\n==> %s\n%s\n<== %s FAILED (cannot run)\n' "$name" "$*" "$name"
	mark_failed "$name"
}

run_fmt() {
	if ! cargo "+$RUSTFMT_TOOLCHAIN" fmt --version >/dev/null 2>&1; then
		unavailable fmt "rustfmt.toml needs a nightly rustfmt: rustup toolchain install $RUSTFMT_TOOLCHAIN --component rustfmt"
		return
	fi
	step fmt cargo "+$RUSTFMT_TOOLCHAIN" fmt --all -- --check
}

run_clippy() {
	step clippy cargo clippy --workspace --all-targets --keep-going -- -D warnings
}

run_docs() {
	RUSTDOCFLAGS="-D warnings"
	export RUSTDOCFLAGS
	step docs cargo doc --workspace --no-deps --document-private-items
}

run_deps() {
	if ! command -v cargo-machete >/dev/null 2>&1; then
		unavailable deps "cargo-machete is not installed: cargo install cargo-machete"
		return
	fi
	step deps cargo machete
}

# grep that fails (and prints) when it FINDS something.
forbid() {
	why=$1; shift
	if hits=$(grep "$@"); then
		printf '%s\n%s\n' "$hits" "  -> $why"
		return 1
	fi
	return 0
}

ban_rules() {
	ok=0
	forbid '#[allow] is banned: #[expect(lint, reason = "...")] fails once the lint stops firing' \
		-rnE --include='*.rs' '#!?\[(cfg_attr\([^]]*,[[:space:]]*)?allow[[:space:]]*\(' crates || ok=1
	forbid 'an #[ignore]d test never runs: delete it, or turn it into a script' \
		-rnE --include='*.rs' '#\[ignore' crates || ok=1
	forbid 'a lint set to "allow" in a manifest hides a whole class of findings; fix them or use #[expect] at the site' \
		-nE '=[[:space:]]*"allow"' Cargo.toml crates/*/Cargo.toml || ok=1
	return $ok
}

run_ban() {
	step ban ban_rules
}

for s in $steps; do
	"run_$s"
done

if [ -n "$failed" ]; then
	printf '\nlint.sh FAILED:%s\n' "$failed"
	exit 1
fi
printf '\nlint.sh: all clean\n'
