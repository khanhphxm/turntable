# Turntable's GPUI Apple renderer patch

Based on gpui-pre-apple 0.3.9 (Apache-2.0), the Apple Metal renderer used by gpui-pre-macos. Registry sources and the GPUI shader-binding inputs under `vendor/gpui` are retained.

## Resource sizing and lifetime

- Small glyphs and images share 512×512 atlas pages instead of 1024×1024 pages. Polychrome tiles larger than 256 pixels in either dimension receive a tightly sized page rather than a minimum 1024-pixel page. A 640×640 BGRA cover therefore reserves 1.56 MiB instead of 4 MiB. Small images cannot pin an otherwise-unused large cover page. Image pixels, sampling, glyph resolution and rendering shaders are unchanged.
- Instance buffers start at 128 KiB instead of 2 MiB per in-flight frame. The existing geometric growth, 256-byte binding alignment, 256 MiB maximum, Metal command-buffer resource retention, and completion-handler recycling remain unchanged. Complex scenes still grow normally; reducing the initial reservation is not a primitive-count limit.
- Full-viewport path resolve/MSAA textures are allocated only for scenes containing paths, reused while their size matches, and released when a scene no longer needs them. Metal retains resources used by older in-flight command buffers. Four-sample antialiasing and Apple GPU memoryless MSAA are unchanged.

Atlas removal still follows GPUI's explicit `drop_image` ownership API. Removing the last tile releases its texture; cached scenes can replay old sprite bindings, so arbitrary per-frame atlas eviction would be incorrect. Turntable's cover cache owns unused-image eviction. This patch does not add a polling timer or disable cached painting.

## Idle drawable pools

On macOS, the display-link source clears the backing `CAMetalLayer`'s [nullable `device`](https://developer.apple.com/documentation/quartzcore/cametallayer/device) when its existing idle-frame tail parks. Core Animation retains the last presented image, while the layer releases the pool's other IOSurfaces. No image readback, copy, replacement layer, additional surface, polling timer, or change to the three-drawable limit is required. The renderer restores its original device immediately before the next `nextDrawable`. The backing layer and its application-owned animation sublayers keep their identity.

Live resize keeps both the pool and display-link subscription, including pauses with the resize grip held. The first unused frame after AppKit ends the resize can park normally. Transaction-backed rendering and drawable-size updates still use the same layer. iOS and headless rendering are unchanged. Native regression tests cover repeated park/resume, the original device and layer identity, overlay ownership, size, transparency, transaction mode, and ignoring non-Metal layers.

### Native measurements (2026-10-14)

The isolated release Turntable process used a copied `/tmp/tt-bench` profile, regenerated Connect identity, `TURNTABLE_BACKGROUND=1`, a visible bottom strip on the 1× primary display, and no system input or Spotify controls. Network access was denied because the account was playing in the user's car; the cached Home UI still rendered its normal covers and playlists. An external Objective-C instrumentation library primed the original three-surface pool, then allowed normal idle parking. The 1280×800 comparison used the same binary and instrumentation, suppressing only the new nil-device setter for the baseline. The 1920×950 comparison used that retained-pool build and the final release with the completed live-resize guard. The table uses the OS's `task_vm_info.phys_footprint` readings before screenshots; these offline totals are not a signed-in playback measurement.

| Cached idle Home | Physical footprint MiB, retained → retired pool | IOSurface MiB, retained → retired pool |
| --- | ---: | ---: |
| 1280×800, 1× | 45.67 → 37.16 | 11.91 → 3.97 |
| 1920×950, 1× | 60.49 → 46.83 | 21.28 → 7.09 |

An independent AppKit/Metal clear probe at 1920×950 retained three drawables before retirement: physical footprint fell from 41.079 to 26.829 MiB and IOSurface charge from approximately 21.28 to 7.09 MiB. Its first resumed drawable acquisition took 1.419 ms; the first completed clear took 10.430 ms including the driver restart. At 1280×800, acquisition took 0.361 ms and the first completed clear 4.237 ms. Before/parked screenshots were pixel-identical at both sizes; the cached Turntable Home image was also unchanged between consecutive parked screenshots.

In the final 1920×950 Turntable build, twelve synthetic hover events were delivered directly to the isolated GPUIView, without posting system input or changing the cursor/focus. The first resumed `nextDrawable` took 0.401 ms median / 0.840 ms maximum, versus 0.060 / 0.070 ms with the retained pool. Event-to-first-drawable latency was 1.048 / 2.441 ms with retirement versus 1.650 / 7.449 ms in the retained-pool run. The first drawable's presentation-handler callback arrived in 9.758 / 11.662 ms; `presentedTime` was zero for these callbacks in both builds, so that callback is not claimed as an actual display-vsync timestamp.

A 20-second window video contained 1,139 original decoded frames across hover/park/resume cycles. The visible Home patch remained stable (mean RGB 35.474–35.589), with no blank or white-flash frames. Both ordinary size changes and a native AppKit live-resize sequence rendered correctly. The live-resize sequence set the real window and view `inLiveResize` flags, performed 60 timed size changes, then ended through AppKit; it used private lifecycle selectors only in the disposable external probe, not in delivered code. Live-resize drawable acquisition was 0.640 ms median / 2.066 ms maximum. The final idle layer's device was nil again, confirming the pool retired after live-resize ended.

An additional uninstrumented 1920×950 offline run reported approximately 46 MiB footprint and 0.46% process CPU over ten seconds. Do not claim the 0% idle-CPU goal or signed-in playing-footprint goal from this network-denied assay; the change removes retained drawable memory, not application/connection work.

Rejected alternatives: setting `drawableSize` to zero retained all three large surfaces (40.720 → 40.688 MiB footprint, Metal allocated size unchanged at 22.188 MiB). Freezing the last IOSurface into a persistent parent's `contents` and removing/recreating its child Metal layer saved the same memory as clearing `device`, but its first completed 1280×800 clear took 22.372 ms instead of 4.237 ms. Lowering `maximumDrawableCount` at runtime was already observed not to release the pool; permanently using two drawables also remains rejected by the earlier scrolling latency measurements below.

The remaining drawable floor is one full-resolution presented surface: approximately 3.97 MiB at 1280×800 or 7.09 MiB at 1920×950, multiplied by four at 2× scale. This patch does not remove the driver's approximately 105 MiB submission transient while continuous GPUI animations run; animation frame demand is a separate cost.

## Regression coverage

The native Metal tests cover compact-page sharing, independent release of large cover textures, tile-space reuse, buffer growth preserving earlier bindings, lazy/reused/resized/released path targets, and rendered image/path pixels after allocation and lifetime changes.

## Memory accounting

Metal shared buffers and atlas textures are demand-backed: allocated capacity is not the same as resident memory. RSS and physical footprint (the Activity Monitor Memory column) account graphics backing differently. Measure both, and include IOAccelerator graphics, IOSurface and owned-unmapped graphics categories. A first render submission can temporarily allocate over 100 MiB of driver backing even for a 64×64 clear; it can retire a few seconds after submissions stop. Reducing queue capacity from 64 to 3 did not reduce that allocation in the isolated Metal experiment, so this patch does not change command-queue capacity.

## Native measurements (2026-10-13)

Signed release, copied scratch config/cache with a regenerated Connect ID and device name `Turntable Bench`, `TURNTABLE_BACKGROUND=1`, no playback or system input. The 1280×800 window was visibly positioned on a 2× display without activation. The same no-input script waited 25 seconds on Home, opened the largest saved playlist (144 tracks), scrolled 480 steps of 24 logical pixels at 16 ms intervals, then waited five seconds. Memory was sampled without screenshot readback. Experimental renderer sizing/statistics knobs and the navigation hook were removed from the delivered source.

| State / renderer | RSS MiB | Physical footprint MiB | IOAccelerator graphics MiB | IOSurface MiB | Owned unmapped graphics MiB |
| --- | ---: | ---: | ---: | ---: | ---: |
| Home, original sizing | 131.1 | 100.5 | 22.94 | 31.75 | 2.48 |
| Home, compact sizing | 109.4 | 105.2 | 12.11 | 47.63 | 2.48 |
| Home, compact sizing + parked display links | 131.7 | 106.4 | 12.11 | 47.63 | 2.48 |
| After playlist scrolling, original sizing | 139.0 | 132.1 | 28.00 | 47.63 | 2.48 |
| After playlist scrolling, compact sizing | 99.1 | 120.0 | 15.41 | 47.63 | 4.97 |
| After playlist scrolling, compact sizing + parked display links | 144.7 | 119.0 | 15.25 | 47.63 | 2.48 |

The stable reduction is IOAccelerator dirty/charged bytes: 10.83 MiB on Home and 12.59 MiB after scrolling. The original Home run had only two of the three allowed drawables resident, while the compact run had all three; that extra 15.875 MiB IOSurface explains the misleading Home total. RSS additionally varied with clean-page residency and system compression (not requested by this patch); its decrease is not independently claimed as a saving. The after-scrolling physical-footprint reduction was 12.1 MiB with equal drawable counts. Polychrome atlas capacity in the trace fell from 24 to 10.17 MiB on Home and from 28 to approximately 12.73 MiB after scrolling. Reducing unused buffer/path reservations is not counted as a physical-memory saving on path-free Home.

Two drawables were tested rather than assumed safe. They saved 15.875 MiB of IOSurface at this viewport, but native `nextDrawable` blocked up to 22.332 ms during navigation and repeatedly 6–13 ms during scrolling. Three drawables generally took about 15 microseconds during steady scrolling, with a 9.706 ms transition maximum. Keep triple buffering: the two-drawable result does not satisfy the no-stutter requirement.

The combined renderer/display-link build used the same visible 2× workload and retained all three drawables. Idle CPU was 0.0% (five-second `top` delta sample), versus 0.2% in the original/compact-renderer-only samples. Its Home footprint was 106.4 MiB and RSS 131.7 MiB; after scrolling these were 119.0 MiB and 144.7 MiB. No compressed pages were reported in the combined samples. The steady owned-unmapped graphics category was already small before display-link pausing; do not claim the over-100-MiB submission transient as a steady idle saving. In the isolated 64×64 Metal clear experiment, the immediate 114 MB physical footprint included 104 MB owned-unmapped graphics while concurrent RSS was only 14.02 MiB, demonstrating why this driver backing cannot be inferred from RSS alone.

A second matched Home-only comparison used the 1× primary display at (320, 260), the same 1280×800 viewport, 25-second settle, copied profiles and three resident drawables. This is closer to the original user baseline than the 2× graphics stress case:

| 1× Home | RSS MiB | Physical footprint MiB | IOAccelerator graphics MiB | IOSurface MiB | Owned unmapped graphics MiB | Idle CPU |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Original renderer/display links | 114.50 | 54.89 | 7.66 | 11.91 | 1.08 | 0.3% |
| Compact renderer + parked display links | 114.98 | 52.86 | 4.95 | 11.91 | 1.08 | 0.0% |

Both 1× snapshots had no compressed pages. Physical footprint fell 2.03 MiB and IOAccelerator dirty/charged memory fell 2.70 MiB while RSS was essentially unchanged. Thus the change is a real graphics-footprint improvement, not a clean-page or compression trick. The remaining dominant GPU cost at 2× is retained full-resolution drawables; cover decoding and page/cache retention are separate application-level work.

## Application frame demand

Turntable's monotonic playhead already owns the seek/clock repaint schedule. The application's
core pump no longer invalidates every cached region just because playback is active, and it
sleeps until a real core deadline or worker notification rather than adding a whole-second
playback deadline. Actual state changes still invalidate all dependent regions. The core's
timed-lyric deadlines continue to update highlighting and following; the miniplayer owns its
own one-second, visibility-aware repaint without repainting the main window.

The portable GPUI cache tests cover unchanged core pumps while playing as well as actual
state-change invalidation. This scheduling fix is independent of Metal, antialiasing and
drawable count. Native compositor animations can then let GPU submission backing retire
instead of keeping it alive with an otherwise-redundant once-a-second full scene submission.
