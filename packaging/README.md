# Native application packaging

`turntable` is a standalone GPUI desktop app. The scripts package existing
release executables; they never launch the app or build the old Tern plugin.
Artifacts are written to `dist/` (gitignored).

## Icons

The shared artwork is `icon.svg`. Checked-in generated assets live in `icons/`:
PNG hicolor sizes 16, 24, 32, 48, 64, 128, 256, 512 and 1024; multi-resolution
Windows `turntable.ico`; macOS `turntable.icns` with normal and Retina variants.
Ordinary builds need no icon-generation dependencies. After changing the SVG:

```sh
cargo install --locked resvg --version 0.45.1
python3 packaging/generate-icons.py             # PNG + ICO, any platform
python3 packaging/generate-icons.py --icns      # PNG + ICO + ICNS, macOS only
```

Commit the source and all regenerated assets together. `resvg` + Python's
standard library are sufficient; macOS `iconutil` supplies ICNS conversion.

## macOS (Apple Silicon)

```sh
MACOSX_DEPLOYMENT_TARGET=12.0 cargo build --release -p turntable
packaging/bundle-macos.sh
plutil -lint dist/turntable.app/Contents/Info.plist
codesign -dv dist/turntable.app
```

The default input is `target/release/turntable`. For an alternate target directory:

```sh
packaging/bundle-macos.sh target/app/release/turntable dist/turntable.app
```

The version defaults to Cargo metadata; set `TURNTABLE_VERSION` to override it.
The bundle declares macOS 12+, music category, Retina support, and identifier
`com.khanhphxm.turntable`. Set `CODESIGN_IDENTITY="Developer ID Application: ..."`
to use a trusted identity; otherwise the script signs ad-hoc. Developer ID
signatures enable hardened runtime and secure timestamps. Ad-hoc packages are
suitable for local development, not Gatekeeper-trusted public distribution.

Create the distributable **after** signing and, when configured, notarization:

```sh
ditto -c -k --keepParent dist/turntable.app dist/turntable-VERSION-macos-arm64.zip
```

## Linux

Build natively on each architecture using Ubuntu 24.04 (glibc 2.39):

```sh
cargo build --release -p turntable
TURNTABLE_VERSION=0.1.0 packaging/bundle-linux.sh
```

The script detects `x86_64` / `aarch64`; for cross-build output use
`TURNTABLE_ARCH=aarch64 packaging/bundle-linux.sh target/aarch64-unknown-linux-gnu/release/turntable`.
Each tarball includes `bin/turntable`, the `.desktop` entry, hicolor PNG/SVG
icons, MIT license, and `INSTALL.md`. See [INSTALL-linux.md](INSTALL-linux.md) for
installation and ALSA/PipeWire, libxkbcommon, Vulkan, Wayland/X11 runtime packages.
Both workflows install the corresponding development headers, plus Clang,
libclang, CMake, Ninja, and pkg-config.

## Windows

```powershell
cargo build --release -p turntable
packaging/bundle-windows.ps1 -Version 0.1.0
```

The ZIP contains `turntable.exe`, license, and [INSTALL.md](INSTALL-windows.md).
`crates/turntable/build.rs` uses `embed-resource` to put the checked-in ICO into
the executable (MSVC `rc.exe`, or `llvm-rc` for cross-compilation). Set
`TURNTABLE_ICON` to an absolute ICO path when building outside the workspace.
GPUI supplies the Windows manifest through its default `windows-manifest`
feature: **gpui-kit 0.7.3 does not itself expose a feature of that name**; its
`gpui-pre` dependency enables it. Do not disable GPUI default features or embed a
second manifest. No Authenticode signing is configured.

## GitHub Actions

Both `.github/workflows/ci.yml` and `release.yml` have **only
`on: workflow_dispatch`** while the repository is private. Comments describe
where tag triggers may be enabled later. CI builds, tests, and runs clippy on
both `turntable` and `turntable-core` on macOS arm64, Ubuntu x86_64, and Windows
x86_64, with a separate nightly rustfmt check.

Manually dispatch Release after updating the Cargo workspace version and lockfile.
It builds macOS arm64, Linux x86_64/aarch64, and Windows x86_64; publishes ZIPs /
tarballs plus `SHA256SUMS` in GitHub release `v<version>` at the dispatched commit.
There is no automatic Homebrew tap push. Keep the tag/version immutable.

Optional macOS secrets:

| Secret | Value |
| --- | --- |
| `DEVELOPER_ID_P12` | Base64-encoded Developer ID Application certificate + private key (.p12) |
| `DEVELOPER_ID_P12_PASSWORD` | Password for that .p12 |
| `NOTARY_KEY` | Base64-encoded App Store Connect API private key (.p8) |
| `NOTARY_KEY_ID` | App Store Connect API key ID |
| `NOTARY_ISSUER_ID` | App Store Connect issuer UUID |

Import/signing requires both Developer ID secrets. Notarization requires the
imported identity and all three notary secrets; without them it is skipped.
Notary submissions wait for success, then staple and validate the ticket before
creating the release ZIP. Temporary keys/keychains are cleaned up. Without a
Developer ID identity, the release remains ad-hoc signed.

## Homebrew tap templates

For `khanhphxm/homebrew-tap`:

- Copy `homebrew/turntable.rb` to `Casks/turntable.rb` (macOS arm64).
- Copy `homebrew/formula/turntable.rb` to `Formula/turntable.rb` (Linux both CPUs).
- Replace `@VERSION@`, `@MACOS_ARM64_SHA256@`, `@LINUX_X86_64_SHA256@`, and
  `@LINUX_AARCH64_SHA256@` with the release version and values from `SHA256SUMS`.
- Validate with `brew audit --cask turntable` / `brew audit --strict turntable`
  on their supported platforms, then commit the filled-in templates to the tap.

Install with `brew install --cask khanhphxm/tap/turntable` on macOS or
`brew install khanhphxm/tap/turntable` on Linux. GitHub private-release downloads
require authentication and are not anonymous Homebrew distribution; publish
publicly accessible assets before enabling the public tap. The cask/formula
files here intentionally contain substitution tokens and are not live releases.
