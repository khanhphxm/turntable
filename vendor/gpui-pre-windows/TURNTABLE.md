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

## Windows measurement (2026-10-14)

Windows 11 build 26200.9711, x86-64 MSVC release, NVIDIA RTX 4070, 24 logical
processors. A 1200×800 window was shown on a 120Hz desktop, using a copied
benchmark profile, distinct Connect device and zero device volume. No playback
or transfer commands were issued. Each CPU sample spans 30 seconds after a
90-second warmup. Memory below is MiB (1,048,576 bytes).

Idle UI was projected by a temporary, uncommitted environment-gated override
of the read-only `State::playing` method. This stopped UI animation without
pausing or otherwise affecting the user's remote device. It left the normal
connected session, loaded Home and artwork intact.

| Idle UI | Private working set | Total working set | Private commit | One-core CPU |
| --- | ---: | ---: | ---: | ---: |
| Upstream, visible | 14.47 | 71.77 | 100.38 | 0.000% |
| Patched, visible | 12.81 | 69.62 | 74.92 | 0.052% |
| Upstream, minimized | 13.40 | 66.05 | 100.30 | 0.052% |
| Patched, minimized | 13.03 | 66.30 | 74.91 | 0.052% |

Working-set residency varies as Windows pages idle allocations. The robust
resource change is GPU commitment: **52.87 → 23.93 MiB**; dedicated GPU usage
was **30.25 → 20.43 MiB**. Private commit fell 25.46 MiB. Neither GPU commitment
nor private commit is Task Manager's target Memory column.

An additional 30-second idle sample counted **9,937 → 2,037** total process
thread context switches. The approximately 120Hz UI and compositor-wait loops
disappeared. A remaining approximately 66Hz thread starts in NVIDIA's
`nvwgf2umx.dll`; an independent sample counted 1,975 switches on that driver
thread and only 28 elsewhere. The measured 0.052% of one core is 0.0022% of
this machine, which rounds to 0.0% in Task Manager. The CPU counter's tick
granularity makes short samples unsuitable for claiming an exact zero.

`VirtualQueryEx` plus `QueryWorkingSet` separated private, image and mapped
pages. Patched visible idle had 9.29 MiB private-region resident pages and
3.52 MiB private image pages; most of the remaining 56.81 MiB of total working
set was shared executable/OS/driver image code. DirectWrite mapped two shared
FontCache files (16+8 MiB virtual) and Segoe UI, bold, symbol and emoji fonts;
these mappings contributed **zero private working-set pages** in the sample.
Discarding the system font collection would not recover their mapped sizes
from Task Manager Memory and would break fallback behavior.

For a matched playing-UI comparison after remote playback paused, a temporary
uncommitted override projected `Status::Playing` and a speed-1 monotonic anchor
only after normal controller work. Its command-delivery guard refused every
player command. It used the real loaded track, Home, artwork, playhead and EQ
animations; it did not decode or emit audio. Both 30-second samples explicitly
confirmed the bench window was foreground at the start and end, with the
pointer outside it. Uncontrolled-focus runs varied substantially, so their
CPU numbers are not used to claim savings.

| Simulated playing UI, foreground | Private working set | Total working set | Private commit | One-core CPU | Task Manager CPU |
| --- | ---: | ---: | ---: | ---: | ---: |
| Upstream | 15.73 | 69.27 | 103.44 | 3.696% | 0.154% |
| Patched | 13.80 | 70.68 | 77.48 | 1.875% | 0.078% |

That sample reduced playing-UI CPU 49.3% and private working set 1.93 MiB;
total working set was 1.41 MiB higher, demonstrating why the private metric
must be reported separately. Total context switches fell 20,428 → 13,806.
GPU commitment was 52.87 → 24.93 MiB. Shared artwork reuse additionally avoids
118,784 decoded bytes (plus GPU copies) for a cached 192px cover subsequently
displayed at 64/96/128px; its regression test asserts the same pixel Arc and
one cache entry, with no queued decode.

Desktop `System.Drawing.CopyFromScreen` captures verified Home artwork/text,
the seek bar and equalizer curve after the changes, including resize and
minimize/restore. The remaining playing-UI floor includes full-window DirectX
presentation and scene-buffer uploads for each changed pixel/animation;
the app still redraws the GPUI fallback seek/clock and EQ animation. Shared
shell-clock and audio optimizations being developed separately are not in
these branch-only figures. Audio decode/output memory and CPU must be measured
with the offline audio harness, not inferred from this UI-only simulation.
