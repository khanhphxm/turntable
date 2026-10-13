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
- Glyph and small-image atlas pages start at 512×512 rather than 1024×1024.
  Polychrome artwork larger than 256px gets a tightly sized page, matching the
  Metal backend's policy. An unrelated small glyph/image cannot keep that
  cover's large backing page alive after eviction.

The atlas tests use the WARP device to verify page size, reuse and independent
release. DirectWrite continues to use its shared system factory and collection;
font fallback and font-identity caching are intentionally unchanged.
