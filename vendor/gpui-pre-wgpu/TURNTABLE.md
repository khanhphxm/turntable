# Turntable's GPUI wgpu renderer patch

Based on gpui-pre-wgpu 0.3.9 (Apache-2.0), the registry snapshot of Zed
`93d745668805029476e6d7fbb19652116fed51de`. Registry sources, the normalized
manifest and the Apache license are retained. The workspace patches crates.io
to this copy. Modified upstream files carry Turntable notices.

## Resource sizing and lifetime

This follows the resource policy already used by Turntable's Metal renderer,
without changing image/glyph resolution, text sampling, antialiasing or cache ownership:

- Monochrome glyphs use 512×512 pages rather than 1024×1024. When no existing
  rectangle fits a color tile, a new page uses 256×256 for edges up to 128px,
  512×512 for edges up to one third of that shared page (170px), and exact
  dimensions for larger tiles. A 640×640 cover reserves 1.56 MiB rather than
  4 MiB; device texture-size limits and explicit image ownership are unchanged.
- Instance data starts at 128 KiB instead of 2 MiB, on both the native storage
  buffer and WebGL texture paths. Existing geometric growth, device limits,
  binding alignment and retention of earlier in-flight bindings are unchanged.
  Complex scenes remain supported; this is not a primitive-count limit.
- Path resolve and multisampled textures are allocated only for scenes with
  paths, reused at the same viewport size, and released for path-free scenes.
  The viewport size and supported sample count are unchanged. wgpu retains
  resources still used by in-flight commands.

The implementation is renderer-shared (Linux/FreeBSD native wgpu and the WebGL
path), not an X11 special case. macOS uses Metal and Windows uses its separate
native renderer; this extends the existing Metal policy rather than changing
those backends. GPUI's explicit image ownership and Turntable's cover-cache
budget still control atlas eviction; arbitrary per-frame eviction would break
cached scenes.

### Exact atlas rectangles

Atlas pages use guillotiere 0.6.2 rather than bucketed shelves. This avoids
height-quantization waste and reclaims individual rectangles while neighboring
tiles remain alive. Live tiles never move, preserving cached-scene coordinates;
the allocator's empty state replaces the redundant live-key counter. Upload
swizzling, deferred uploads and texture-generation tracking are unchanged.

Polychrome fragment samples are clamped to the tile's outer texel centers,
preventing linear filtering from reading neighboring/uninitialized pixels when
an image is upscaled. Vertex UVs and interior image mapping are unchanged.
WGSL retains derivative sampling before its alpha-clip branch. The corresponding
Metal Home comparison saved 1,343,488 charged graphics bytes; the clamp itself
was byte-for-byte footprint neutral. See the [Apple byte inventory](../gpui-pre-apple/TURNTABLE.md#exact-atlas-packing-2026-10-15).
These figures are macOS measurements, not Linux/Windows runtime results.

Content-sized color pages additionally saved 360,448 charged graphics bytes
(352 KiB) in the native Metal Home A/B. Tiny images share smaller pages while
nine 164×164 images still fit one 512×512 page. The 248×248 and 248×106 cases
have exact-page regressions. This is a direct allocator-policy port, not a
Linux runtime measurement; see the [native comparison](../gpui-pre-apple/TURNTABLE.md#content-sized-color-pages).

Regression tests cover compact-page sharing/reuse, independent large-image
release, lazy/reused/resized/released path targets, and instance growth with
pixel readback of a scene larger than the initial capacity. Existing shader,
atlas generation and headless image tests are retained.

The normalized registry manifest omits direct `naga` and `criterion`
dependencies used by the retained shader tests and font benchmark. This copy
declares them as development dependencies. The three upstream font fixtures
needed by those tests/benchmarks are retained under `test-fonts`, with their
SIL Open Font License texts, from the exact Zed revision above. They are not
runtime assets or part of Turntable's UI font bundle. Vendor packages are
excluded from workspace membership so their tests can also run explicitly:

```sh
cargo test --manifest-path vendor/gpui-pre-wgpu/Cargo.toml --features test-support
```

## Deferred OpenGL initialization

Native window startup first probes hardware Vulkan alone. If it succeeds,
there is no unused OpenGL/EGL driver context, software-rasterizer worker pool or
adapter database kept beside the active Vulkan device. If it fails, startup
uses the existing combined Vulkan/OpenGL instance and adapter-priority policy,
including software rendering. The hardware-only probe is important: software
Vulkan must not displace a usable hardware OpenGL adapter. Multiple windows
continue to share the chosen instance/device. Recovery retains upstream's
combined-backend retry path. This is shared renderer code, not an Xvfb, vendor
or PCI-device special case.

## Linux memory accounting

Use `/proc/<pid>/smaps_rollup` PSS/RSS, not virtual address reservations. GPU
allocation capacity is not identical to process-resident pages. Xvfb lacks DRI3:
it selects Mesa llvmpipe and includes LLVM JIT code, software rasterizer heaps,
worker pools and host-backed textures in the process. Report those separately
from measurements on a real GPU/compositor. Never subtract all anonymous memory
as if it were driver memory: Rust state, decoded covers, stacks and font caches
share that category. LLVM can also be loaded by the hardware Mesa drivers;
its presence alone does not prove software rendering.

## Debian measurements (2026-10-14)

Release, Debian 13/Mesa 25.0.7, 32 logical CPUs, 1280×800 Home. X11 used
Xvfb/Openbox/llvmpipe (no DRI3); hardware Wayland used Weston headless GL on
AMD Radeon 610M, verified by amdgpu engine counters. An unshipped inert virtual
seat supplied Weston's missing `wl_seat`, without host input devices.

Copied isolated `TURNTABLE_HOME`, muted `Turntable Bench Linux`, fresh Connect ID.
No playback was controlled. Temporary uncommitted UI-only hooks simulated
playing or froze idle notifications while still draining core events; all were
removed before checks. Samples: 30 seconds after 20–40 seconds settling, kernel
PSS/RSS, no screenshot readback; not statistical CPU bounds. CPU 100% = one core.

| Backend / state | Baseline PSS/RSS MiB | Patched PSS/RSS MiB | Baseline/patched CPU % |
| --- | ---: | ---: | ---: |
| X11 software, idle visible | 187.60 / 224.75 | 185.24 / 222.43 | 0.167 / 0.000 |
| X11 software, idle minimized | 187.66 / 224.82 | 185.45 / 222.64 | 0.000 / 0.033 |
| X11 software, simulated playing | 193.84 / 230.97 | 185.27 / 222.41 | 463.729 / 535.094 |
| Wayland hardware, idle visible | 132.81 / 169.58 | 114.73 / 144.17 | 0.133 / 0.033 |
| Wayland hardware, simulated playing | 123.87 / 160.55 | 117.29 / 146.66 | 7.367 / 5.867 |

Compact resources alone: hardware idle 125.29/162.03 MiB PSS/RSS; active-client
VRAM **capacity** 83,872→34,720 KiB (48 MiB), shared swapchain 18,016 KiB unchanged.
Deferred GL plus shared theme initialization: further −10.56/−17.86 MiB PSS/RSS,
61→51 threads. X11 shared-memory residency fell 5.86 MiB. Capacity is not PSS.

**Targets are not met:** hardware PSS ≈120.3 MB idle/123.0 MB simulated playing;
decoder cost excluded. X11 playing CPU did not improve; llvmpipe is not a real-GPU
floor. Hardware idle PSS: executable 31.99 MiB, anonymous allocator/driver/stacks
28.91, fonts 26.67, LLVM/Mesa 19.81, other graphics 5.64, other files 1.69. Font
enumeration avoidance did not establish Linux font-residency savings. Loaded
family faces remain cached; these results do not prove the remaining floor irreducible.

Use `python3 scripts/bench/measure-linux.py <PID> --seconds 30` for passive metrics.
Run Cargo from the repository root (or pass `.cargo/config.toml` explicitly) so
the existing jemalloc configuration applies to comparable builds.
