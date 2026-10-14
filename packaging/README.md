# Turntable packaging and releases

Scripts package existing executables into `dist/` (gitignored). They never launch
the app, start playback, or access a Spotify account. Binary/package IDs remain
lowercase `turntable`; the displayed app name is **Turntable**.

## Local packages

### macOS universal

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
export MACOSX_DEPLOYMENT_TARGET=12.0
cargo build --release --locked -p turntable --target aarch64-apple-darwin
cargo build --release --locked -p turntable --target x86_64-apple-darwin
mkdir -p target/universal/release
lipo -create target/aarch64-apple-darwin/release/turntable \
  target/x86_64-apple-darwin/release/turntable -output target/universal/release/turntable
export TURNTABLE_VERSION=0.1.0  # match Cargo.toml
export CODESIGN_IDENTITY='Developer ID Application: Khanh Pham (24U222KA6B)'
packaging/bundle-macos.sh
packaging/dmg-macos.sh
ditto -c -k --keepParent dist/Turntable.app "dist/turntable-$TURNTABLE_VERSION-macos-universal.zip"
```

`bundle-macos.sh [universal-binary [output.app]]` defaults to the merged binary
above and recreates `dist/Turntable.app`. It verifies both slices and the signature.
`dmg-macos.sh [input.app [output.dmg]]` adds an Applications shortcut and verifies
the image. Without `CODESIGN_IDENTITY`, signing is explicitly ad-hoc. Developer
ID signing uses hardened runtime and a timestamp. No runtime exceptions are
needed: this is not a sandboxed app and does not record audio or use JIT.
The bundle supports macOS 12+; SemVer suffixes are stripped only from Apple's
numeric bundle-version fields. **Neither local script notarizes.** CI staples
the app before archiving, then notarizes/staples the signed DMG too.

### Linux

Build natively on Ubuntu 24.04 for each CPU, then run
`TURNTABLE_VERSION=0.1.0 packaging/bundle-linux.sh [binary]`.
`TURNTABLE_ARCH=x86_64|aarch64` overrides CPU detection. Each tarball includes
`bin/turntable`, a `.desktop` entry, hicolor PNG/SVG icons, LICENSE,
THIRD_PARTY_NOTICES and INSTALL.md;
each DEB installs under `/usr`, with Ubuntu runtime dependencies and architecture
`amd64` / `arm64`. Prereleases use Debian's `~` ordering. Packaging needs
`dpkg-deb`, not root. Binaries require glibc 2.39+ and a Vulkan-capable GPU.
See [INSTALL-linux.md](INSTALL-linux.md); shared CI build headers are installed by
`scripts/install-linux-deps.sh`.

### Windows

Build with `cargo build --release -p turntable`, then run
`packaging/bundle-windows.ps1 -Version 0.1.0 [-Binary PATH]` in PowerShell.
Install [Inno Setup 6](https://jrsoftware.org/isinfo.php) first (preinstalled on
`windows-2025`); `-Iscc PATH` overrides its standard Program Files (x86) location.
The script produces a portable ZIP and per-user `-setup.exe`. The installer in
`windows/turntable.iss` installs to LocalAppData without elevation, adds a Start
Menu shortcut/uninstaller, and upgrades in place with a permanent AppId.
No Authenticode signing is configured; SmartScreen may warn. GPUI supplies the
Windows manifest; `crates/turntable/build.rs` embeds the ICO. See
[INSTALL-windows.md](INSTALL-windows.md).

### Icons

After changing `icon.svg`, install `resvg` (`cargo install --locked resvg --version
0.45.1`) and run `python3 packaging/generate-icons.py` (PNG/ICO), adding `--icns`
on macOS for ICNS. Commit all generated `icons/` together; ordinary builds need
no renderer or Python packages.

## Third-party notices

Every package includes the root `THIRD_PARTY_NOTICES`: macOS stores it in
`Contents/Resources` before signing, Linux DEBs in `/usr/share/doc/turntable`,
and portable archives and Windows installations beside LICENSE.

After dependency or embedded-asset updates, install the pinned generator with
`nice -n 19 env CARGO_BUILD_JOBS=4 cargo install cargo-about --version 0.9.2 --locked --features cli`,
then run `packaging/generate-notices.sh` and commit the regenerated file.
The config covers the complete dependency graph for all five release targets,
including build-time and development dependencies conservatively. The script
also retains asset, copied-snippet and native-library notices which Cargo
package license expressions alone do not describe. Exact crate-source links
cover MPL-2.0 source availability; bundled D-Bus uses the AFL-2.1 option, whose
terms recipients are asked to review and accept in the Linux install guide.
Before releasing, review changes in bundled native dependencies and their
licensing as well as the Rust crate manifests.

## CI and release setup

CI runs on main pushes, PRs, merge queues and dispatch; Release runs on `v*` tags and
dispatch.
Public Homebrew installs require publicly downloadable releases.

CI runs nightly rustfmt first, then lint and tests per OS; release builds run once both pass.
Required checks: `ci` (passes when every job succeeds, or when a prose-only change skips them) and `pr-title`. Squash-merge PR titles must be conventional commits,
e.g. `feat(player): add shuffle`. Fork PRs need no secrets and cannot save caches.
Tests skip `#[ignore]` manual benchmarks/native tray checks; GPUI TestPlatform
journeys are headless. Dependency/backend native-desktop tests are not workspace
members; run those separately in an interactive desktop session, not hosted CI.
Tag release builds are uncached so published binaries do not depend on CI caches.

Set **Actions secrets** in the `release` environment (Settings → Environments);
only `v*` tags may deploy. After copying the values, remove the corresponding
repository secrets. No PAT is needed; `GITHUB_TOKEN` is built in.

| Secret | Value / creation |
| --- | --- |
| `DEVELOPER_ID_P12` | Base64 Developer ID Application certificate **and private key**; export below. |
| `DEVELOPER_ID_P12_PASSWORD` | Password chosen for the encrypted `.p12` export. |
| `NOTARY_KEY` | Base64 App Store Connect API `.p8` private key. |
| `NOTARY_KEY_ID` | That API key's ID. |
| `NOTARY_ISSUER_ID` | Issuer UUID shown beside the API key. |
| `APPLE_ID` | Fallback: Apple Developer account email. |
| `APPLE_TEAM_ID` | Fallback: developer.apple.com → Membership details → Team ID (`24U222KA6B`). |
| `APPLE_APP_PASSWORD` | Fallback: create at account.apple.com → Sign-In and Security → App-Specific Passwords. |
| `HOMEBREW_TAP_KEY` | Raw OpenSSH private deploy key with write access to `khanhphxm/homebrew-tap`. |

**Developer ID:** create a CSR in Keychain Access → Certificate Assistant →
Request a Certificate From a Certificate Authority; the team's Account Holder
creates a Developer ID Application certificate at developer.apple.com →
Certificates. Import it on the Mac holding the CSR's private key. For a safe CLI
export, create `turntable-release` via Keychain Access → File → New Keychain and
copy **only that identity and private key** into it (not other identities):

```sh
security export -k "$HOME/Library/Keychains/turntable-release.keychain-db" \
  -t identities -f pkcs12 -o DeveloperID.p12
# Secure prompt: save the chosen password as DEVELOPER_ID_P12_PASSWORD.
base64 < DeveloperID.p12 | tr -d '\n' | pbcopy  # paste as DEVELOPER_ID_P12
rm DeveloperID.p12
```

**Notarization:** create a team key (Developer role or higher) at App Store
Connect → Users and Access → Integrations → App Store Connect API. Download the
`.p8` once, record its ID/issuer, and run
`base64 < AuthKey_KEYID.p8 | tr -d '\n' | pbcopy` for `NOTARY_KEY`. Keep the original
in a secure vault. The complete API-key trio takes precedence over the complete
Apple-ID fallback trio. Never commit signing keys or certificates.

**Tap SSH:** run `ssh-keygen -t ed25519 -f turntable-tap -C turntable-release`
(no passphrase), add its `.pub` under the **tap** repository's Settings → Deploy
keys with **Allow write access**, and save the private key as `HOMEBREW_TAP_KEY`.

## Cut a release

1. Update `[workspace.package].version` and `Cargo.lock`; all workspace packages
   must match. Merge after CI passes, then tag that main commit, substituting
   the real version: `git tag -a v0.1.0 -m 'Turntable 0.1.0' && git push origin v0.1.0`.
2. Watch **Release**. For manual dispatch, select the **existing release tag**
   in the workflow ref dropdown (not `main`) and use the same tag as the input;
   the `release` environment only permits `v*` tag refs. It validates the
   tag/version, builds the immutable commit, generates conventional-commit
   notes with `cliff.toml`, and uploads all eight artifacts plus `SHA256SUMS` to a
   draft before publishing: macOS universal ZIP/DMG; Linux x86_64/aarch64
   tarballs/DEBs; Windows x86_64 ZIP/per-user EXE. Never move a published tag.
3. Tags containing `-` are prereleases and skip Homebrew. Stable releases render
   `homebrew/turntable.rb` → `Casks/turntable.rb` and `homebrew/formula/turntable.rb`
   → `Formula/turntable.rb` using `homebrew/render.py` and published checksums,
   then push over SSH. These templates are the single source of truth.

Both Developer ID secrets absent means a logged **ad-hoc** build; missing notary
credentials means signed-but-unnotarized artifacts, not Gatekeeper-trusted
distribution. A missing tap key logs a skipped tap update. A failed publish can
leave a draft; resolve/delete it before rerunning the same immutable tag.

Homebrew: `brew install --cask khanhphxm/tap/turntable` (macOS) or
`brew install khanhphxm/tap/turntable` (Linux, both CPUs). Before editing workflows:
`brew install actionlint zizmor shellcheck`, then `actionlint`,
`zizmor .github/workflows`, and ShellCheck the packaging/scripts shell files.
