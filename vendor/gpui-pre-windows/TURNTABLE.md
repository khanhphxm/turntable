# Turntable's gpui-pre-windows patch

Based on gpui-pre-windows 0.3.9 (Apache-2.0), the registry snapshot of Zed
`93d745668805029476e6d7fbb19652116fed51de`. The pristine registry source,
normalized manifest and license are retained in the preceding vendor commit.
Changed upstream files carry a Turntable modification notice pointing here.
`src/frame_scheduler.rs` is a Turntable addition. The workspace patches
crates.io to this copy; the distributed root `THIRD_PARTY_NOTICES` already
includes GPUI's Apache license.

## Demand-driven compositor waits

Upstream waits for DWM and invalidates every window on every vsync, including
clean and minimized windows. GPUI can skip drawing them, but the compositor
thread and UI message pump still run at the display refresh rate.

- A condition variable parks the compositor thread until a window requests a
  frame. A per-window atomic coalesces demand; clean sibling windows are not
  invalidated. Active animations remain paced by the existing DWM vsync wait.
- `PlatformWindow::frame_waker` connects GPUI's existing invalidator to that
  demand: notifications, refreshes, next-frame callbacks and deferred/throttled
  frames all resume the compositor thread without a polling timer.
- Native paint/expose, resize and visibility paths remain native. Hidden or
  minimized windows consume no periodic frames. Re-entrant draws explicitly
  retain their next-frame demand instead of relying on a periodic repaint.
- Direct Manipulation requests frames while a gesture or inertia is active,
  and stops requesting them on completion. Device-loss recovery is checked
  whenever the compositor is woken. Shutdown and GPU detach wake a parked
  thread so its GPU references are released.

The companion GPUI-core patch also re-arms the invalidator when
`on_request_frame` defers a request because `draw_in_progress()` is true.
That retry is required for a GPUI-initiated draw that re-enters the native
message pump (rather than a nested native draw caught by `DrawCoordinator`).
It is carried by the shared `gpui-pre` patch, not a Windows-only API.

`frame_scheduler::tests` covers coalescing, independent windows and shutdown.
Windows release measurements use an isolated `TURNTABLE_HOME` profile with a
unique device name and regenerated device ID. Report private working set
(Task Manager's Memory), total working set and CPU separately: private commit
and GPU memory are not interchangeable with the target metric.

## Retained DirectX resources

- Flip-model swap chains keep two buffers (the DXGI minimum), rather than
  three. Vsync pacing remains unchanged.
- The full-window 4x-MSAA path target and its single-sample resolve target are
  allocated only when a scene contains paths, reused while it does, and
  released on the first path-free frame. Quad, image and glyph rendering does
  not use them. Resize and device recovery discard them until needed again;
  path antialiasing and shaders are unchanged.
- Monochrome glyph pages start at 256×256 rather than 1024×1024 and grow to fit
  oversized masks without resampling. If existing rectangles cannot hold a
  color tile, its longest edge selects a new page:
  up to 128px uses 256×256; up to one third of 512px (170px) uses 512×512;
  larger artwork gets exact dimensions, matching Metal and wgpu. Texture-size
  limits and explicit image ownership are unchanged.

The atlas tests use the WARP device to verify page size, reuse and independent
release. DirectWrite continues to use its shared system factory and collection;
font fallback and font-identity caching are intentionally unchanged.

### Exact atlas rectangles

Atlas pages use guillotiere to avoid shelf padding and reclaim individual tiles
without moving live allocations or invalidating cached-scene coordinates.
Device-loss reset, upload bounds checks and explicit image ownership are
unchanged, as is subpixel-color page sizing.

Polychrome fragment samples clamp to tile-edge texel centers to prevent linear
filtering from reading neighboring pixels. Vertex UVs and interior image
mapping are unchanged. The WARP tests cover rectangle reuse, exact-size color
pages, oversized masks and last-tile release.

## Windows measurement (2026-10-14)

Windows 11 build 26200.9711, MSVC release, RTX 4070, 24 logical CPUs,
1200×800 window on a 120Hz desktop. Samples span 30s after 90s warmup with
an isolated, silent benchmark profile. Temporary uncommitted UI projections
froze animation for idle and advanced a monotonic anchor for simulated playing;
the latter refused all player commands. No playback/transfer commands occurred.
Playing samples confirmed foreground focus at both endpoints. Values are MiB.

| UI state | Upstream private WS / total WS | Patched private WS / total WS | One-core CPU, upstream → patched |
| --- | ---: | ---: | ---: |
| Idle visible | 14.47 / 71.77 | 12.81 / 69.62 | 0.000% → 0.052% |
| Idle minimized | 13.40 / 66.05 | 13.03 / 66.30 | 0.052% → 0.052% |
| Simulated playing, foreground | 15.73 / 69.27 | 13.80 / 70.68 | 3.696% → 1.875% |

Private working set is Task Manager Memory; total WS is RSS. Residency varies
as Windows pages idle allocations. Idle GPU commitment fell **52.87 → 23.93 MiB**
and private commit **100.38 → 74.92 MiB**. Playing-UI CPU fell 49.3% (Task Manager
CPU 0.154% → 0.078%). Idle thread switches fell 9,937 → 2,037 per 30s; the
remaining ~66Hz thread starts in NVIDIA's `nvwgf2umx.dll`. 0.052% of one core
is 0.0022% system CPU here, rounding to 0.0%, not an exact measured zero.
A final clean release launch without any UI override, sampled after a shorter
30s warmup, was 24.71 MiB private WS / 80.67 MiB total WS and 0.000% CPU.

`VirtualQueryEx`/`QueryWorkingSet` attributed patched idle's 12.81 MiB private
WS to 9.29 MiB private regions and 3.52 MiB private image pages. The remaining
56.81 MiB total WS was shared image/mapped pages. DirectWrite's two FontCache
files (16+8 MiB virtual) and Segoe UI/bold/symbol/emoji mappings contributed
zero private WS pages; their mapped sizes are not recoverable target memory.
Playing's remaining floor includes full-window DirectX presentation and scene
uploads for GPUI seek/clock/EQ changes. Audio decode/output is not simulated or
included; shared shell-clock/audio changes are outside these branch figures.

## Native verification

Committed sources without simulation passed Windows nightly fmt check,
workspace/all-targets Clippy `-D warnings`, and workspace/all-targets tests:
47 UI, 448 core and two integration tests; one existing manual benchmark ignored.
All 26 backend tests passed in an interactive desktop session, including WARP
atlas sizing, lazy path reuse/resize/release, pixel readback and emoji tests.
SSH's service session lacks DXGI HWND/Credential Manager access: use an
interactive PowerShell or scheduled task, not test skips. Desktop
`System.Drawing.CopyFromScreen` captures verified Home, seek, EQ curve,
resize and minimize/restore rendering.

