# Turntable's GPUI Apple renderer patch

Based on gpui-pre-apple 0.3.9 (Apache-2.0), the Apple Metal renderer used by gpui-pre-macos. Registry sources and the GPUI shader-binding inputs under `vendor/gpui` are retained.

## Resource sizing and lifetime

- Monochrome glyphs share 256×256 atlas pages instead of 1024×1024 pages; oversized glyphs/SVGs still grow a page to fit. New color pages use 256×256 backing for tiles up to 128px, 512×512 backing when tiles fit at least three across, and exact backing for larger tiles. Existing pages are reused first. A 640×640 BGRA cover therefore reserves 1.56 MiB instead of 4 MiB. Image pixels, glyph rasterization and rendering resolution are unchanged.
- Instance buffers start at 128 KiB instead of 2 MiB per in-flight frame. The existing geometric growth, 256-byte binding alignment, 256 MiB maximum, Metal command-buffer resource retention, and completion-handler recycling remain unchanged. Complex scenes still grow normally; reducing the initial reservation is not a primitive-count limit.
- Full-viewport path resolve/MSAA textures are allocated only for scenes containing paths, reused while their size matches, and released when a scene no longer needs them. Metal retains resources used by older in-flight command buffers. Four-sample antialiasing and Apple GPU memoryless MSAA are unchanged.

Atlas removal still follows GPUI's explicit `drop_image` ownership API. Removing the last tile releases its texture; cached scenes can replay old sprite bindings, so arbitrary per-frame atlas eviction would be incorrect. Turntable's cover cache owns unused-image eviction. This patch does not add a polling timer or disable cached painting.

## Exact atlas packing (2026-10-15)

Atlas pages use [guillotiere's dynamic rectangle allocator](https://docs.rs/guillotiere/0.6.2/guillotiere/struct.AtlasAllocator.html)
instead of bucketed shelves. Individual rectangles are reclaimed immediately;
live tiles never move, so cached scene coordinates remain valid. Empty pages
still follow explicit image ownership and are released normally. The allocator's
empty state replaces the redundant per-page key counter. This is also applied
to the wgpu and DirectX backends, without changing their upload, generation,
device-loss or cached-scene rules.

The old allocator rounded 164-pixel artwork into 192-pixel-high shelves. It also
placed late 12–15-pixel glyphs in an existing 160-pixel SVG shelf after running
out of fresh rows. Nine 164×164 tiles now fit one 512×512 page instead of spilling
after six. A regression also removes/reuses a single tile while its neighbors
keep their page alive. Glyph sizes already shared pages; there was not a separate
monochrome texture for every font size.

### Idle Home byte inventory

At 1920×950 and 1×, the original baseline's 5,570,560 charged IOAccelerator
graphics bytes break down exactly as follows. Matching native allocation stacks,
Metal resource sizes and buffer CPU addresses distinguish driver heaps from
application buffers; four 128-KiB mappings are not four instance buffers.

| Resource | Charged bytes |
| --- | ---: |
| Four BGRA8 512×512 atlas pages, 1,064,960 each | 4,259,840 |
| Two A8 512×512 atlas pages, 278,528 each | 557,056 |
| One reusable scene instance buffer | 131,072 |
| Suballocator page containing the 48-byte unit vertex buffer | 131,072 |
| AGX bindless constant-sampler suballocator | 131,072 |
| AGX vertex-program heap | 65,536 |
| AGX fragment-program/ESL-state heap | 65,536 |
| AGX device-initialization heap | 65,536 |
| Device `setupDeferred` internal-buffer suballocator | 131,072 |
| Two command-storage shared-memory tables, 16,384 each | 32,768 |
| **Total** | **5,570,560** |

The integrated idle queue policy releases the last two tables, leaving
5,537,792 bytes. An additional 16-KiB device mapping has zero charge. The first
pipeline creation allocates the shared shader/sampler heaps; the next seven do
not increase `MTLDevice.currentAllocatedSize`. Path, MSAA and video-surface
textures were absent. The final observed frame used 86,784 of its 131,072 scene
bytes, so a 64-KiB initial buffer would immediately regrow.

Live occupancy from the instrumented pre-packing integrated Home, after image-cache releases:

| Page | Live tiles | Actual tile pixels | Pixel-area occupancy | Charged bytes |
| --- | ---: | ---: | ---: | ---: |
| A8 0 | 374 (349 glyphs, 25 SVGs) | 105,587 | 40.3% | 278,528 |
| A8 1 | 56 glyphs | 6,879 | 2.6% | 278,528 |
| BGRA 0 | 14 images | 117,936 | 45.0% | 1,064,960 |
| BGRA 1 | 6 images | 156,948 | 59.9% | 1,064,960 |
| BGRA 2 | 6 images | 136,784 | 52.2% | 1,064,960 |
| BGRA 3 | 3 images | 90,928 | 34.7% | 1,064,960 |

Those pages hold 2,010,384 image bytes and 112,466 monochrome bytes, versus
4,816,896 charged atlas bytes. The 25 SVGs include a 138×138 heart; 12, 14, 16,
24 and 32-pixel glyphs share the first page. Tile pixels exclude unused shelf
space and the driver's per-texture overhead, not antialiased or transparent
pixels within an image.

`vmmap` identifies exactly one charged, full-size `CAMetalLayer Display Drawable`
IOSurface at 7,438,336 bytes. Other tiny IOSurface mappings have zero charge;
their region count is not an extra drawable pool. The displayed surface remains
full resolution and is not removed by atlas packing.

### Native packing comparison

Signed release, private profile, idle full Home, 30-second settle, serialized
back-to-back `/tmp/ttm.sh` runs. The original baseline was also run immediately
before the two integrated variants; its IOAccelerator charge was 5,570,560 and
IOSurface charge 7,438,336 bytes.

| Category | Integrated bucketed shelves | Exact rectangles |
| --- | ---: | ---: |
| Physical footprint | 45,646,808 | 44,352,472 |
| IOAccelerator graphics | 5,537,792 | 4,194,304 |
| IOSurface | 7,438,336 | 7,438,336 |
| Owned-unmapped graphics | 0 | 0 |
| Malloc Small | 13,778,944 | 13,746,176 |
| App-Specific Tag 15 | 8,912,896 | 8,994,816 |

One monochrome and three color pages replace two and four respectively:
**1,343,488 bytes (1.281 MiB) less charged graphics**, with a 1,294,336-byte
total reduction in this pair. Both processes sampled 0.0% idle CPU. Heap
residency varies independently; the graphics reduction matches the removed
page sizes rather than relying on the total alone.

Linear image filtering is clamped in the fragment shader to each tile's outer
texel centers. This prevents adjacent/uninitialized texels contaminating an
upscaled image when packing puts tiles next to each other. Vertex UVs are not
inset and interior image mapping is not rescaled. The GPU pixel regression uses
a two-by-two gradient surrounded by unrelated blue pixels and checks both edges
and bilinear interiors. Metal, WGSL and HLSL apply the same rule.

The separate clamp A/B was allocation-free: both variants reported 44,368,856
total, 4,194,304 IOAccelerator, 7,438,336 IOSurface, zero owned-unmapped graphics,
13,811,712 Malloc Small and 8,945,664 App-Specific Tag 15 bytes; idle CPU was 0.0%.
The fix intentionally corrects image-edge pixels rather than claiming the
previous unclamped output was a valid visual reference at those boundaries.

### Content-sized color pages

When a new page is required, the longest tile edge determines its reservation:
up to 128px uses a 256×256 page; 129–170px uses a shared 512×512 page; larger
tiles use their exact dimensions. The middle cutoff is one third of the shared
page width, keeping nine 164×164 Home tiles together without reserving a whole
512×512 page for a 248×248 portrait or 248×106 artist image. Existing rectangles
are still tried first; there is no relocation, readback or new eviction policy.

Back-to-back signed native Home measurements at the same 1920×950, 1×,
30-second settle (all 0.0% idle CPU):

| Category | Original baseline | Exact packing, 512px pages | Content-sized color pages |
| --- | ---: | ---: | ---: |
| Physical footprint | 49,480,664 | 44,237,760 | 43,992,024 |
| IOAccelerator graphics | 5,570,560 | 4,194,304 | 3,833,856 |
| IOSurface | 7,438,336 | 7,438,336 | 7,438,336 |
| Owned-unmapped graphics | 1,294,336 | 0 | 0 |
| Malloc Small | 13,697,024 | 13,615,104 | 13,746,176 |
| App-Specific Tag 15 | 11,468,800 | 8,994,816 | 8,962,048 |

Color backing changes from three 512×512 pages (3,194,880 bytes) to two
512×512 pages, one 256×256 page, one 248×248 page and one 248×106 page
(2,834,432 bytes). Native backing includes 16-KiB texture metadata and hardware
padding: the last image charges 147,456 bytes, not its 105,152 pixel bytes.
The reduction is **360,448 graphics bytes (352 KiB)** and 245,736 total bytes
in this pair. A preceding comparison with two charged drawables was discarded.
Native screenshots differ at only six of 1,824,000 pixels, at most two channel
levels, in the portrait; text and the rest of the window match exactly. All
image dimensions and sampling geometry are unchanged. The same page policy is
ported directly to wgpu and DirectX; these are macOS, not cross-platform,
footprint figures.

### Compact monochrome pages

Exact packing lets monochrome pages start at 256×256 instead of 512×512.
The Home glyphs and SVGs fit two 81,920-byte pages instead of one 278,528-byte
page, saving **114,688 graphics bytes (112 KiB)**. Pages still grow to the
requested tile dimensions; a 320×144 mask gets a 320×256 page, not a clipped or
rescaled mask. The A8/R8 formats, rasterization and sampling are unchanged.
The wgpu and DirectX ports leave their subpixel-color page policy unchanged.

Back-to-back signed native Home measurements, 1920×950 at 1×, 30 seconds:

| Category | Original baseline | 512px monochrome pages | 256px monochrome pages |
| --- | ---: | ---: | ---: |
| Physical footprint | 51,168,240 | 44,073,944 | 43,795,416 |
| IOAccelerator graphics | 5,570,560 | 3,833,856 | 3,719,168 |
| IOSurface | 7,438,336 | 7,438,336 | 7,438,336 |
| Owned-unmapped graphics | 1,294,336 | 0 | 0 |
| Malloc Small | 13,828,096 | 13,860,864 | 13,680,640 |
| App-Specific Tag 15 | 12,976,128 | 8,945,664 | 8,978,432 |

The paired total reduction is 278,528 bytes; heap residency is not attributed to
the page change. All three sampled 0.0% idle CPU. Native screenshots differ in
one of 1,824,000 pixels by one channel level. Tests exercise small and oversized
monochrome allocations and last-tile release on all three backends.


### Procedural quad vertices

The Metal renderer now derives the four unit-quad corners from `vertex_id` and
draws a triangle strip, matching the existing wgpu and DirectX implementation.
Quads, shadows, underlines, monochrome/color sprites, path resolve sprites and
video surfaces use the same corners and triangles. Raw path rasterization
vertices remain triangle lists. There is no replacement inline buffer or CPU
copy. The removed six-vertex, 48-byte buffer had pinned an entire 131,072-byte
Metal suballocator page; its field, packing helper, bindings and shader inputs
are removed. The exported `PointF` alias remains the path vertex's float point.

Back-to-back signed native Home measurements, 1920×950 at 1×, 30 seconds,
all 0.0% idle CPU:

| Category | Original baseline | With unit buffer | Procedural corners |
| --- | ---: | ---: | ---: |
| Physical footprint | 50,332,632 | 44,024,792 | 43,615,192 |
| IOAccelerator graphics | 5,570,560 | 3,719,168 | 3,588,096 |
| IOSurface | 7,438,336 | 7,438,336 | 7,438,336 |
| Owned-unmapped graphics | 1,294,336 | 0 | 0 |
| Malloc Small | 13,254,656 | 13,828,096 | 13,680,640 |
| App-Specific Tag 15 | 12,812,288 | 9,093,120 | 8,912,896 |

This saves **131,072 graphics bytes (128 KiB)**; the paired total fell 409,600
bytes, including independent heap-residency differences. Captured native Home
pixels before/after were identical. A preceding footprint pair with two charged
drawables was discarded rather than attributing surface retirement to this
change. No new cross-platform port is needed: those backends already use
procedural strips.

### Remaining charged graphics

The final idle Home's 3,588,096 graphics bytes comprise two 512×512 BGRA pages
(2,129,920), one 256×256 BGRA page (278,528), one 248×248 BGRA page (278,528),
one 248×106 BGRA page (147,456), two 256×256 A8 pages (163,840), the reusable
scene buffer (131,072), and the shared sampler/shader/device/deferred heaps
listed above (458,752). No path or video textures are live. The display still
has one full-resolution 7,438,336-byte IOSurface.

These four Round-2 changes remove **1,949,696 graphics bytes (1.859 MiB)** from
the integrated starting point; the original baseline additionally retained the
32-KiB command tables. Further atlas reduction would need measured page growth
or compaction that preserves live coordinates, cached-scene bindings and
in-flight resources, not eviction of still-referenced tiles or lower image
resolution. Instrumentation showed that the seven pipeline creations after the
first did not increase `MTLDevice.currentAllocatedSize`; lazy creation has not
established an additional footprint reduction.

### Round-2 verification (2026-10-15)

Final production sources passed workspace `cargo +nightly fmt --all -- --check`,
22 signed Apple backend tests and 12 signed macOS platform tests, serialized at
one test thread. The native pixel suite includes compact/large images, the
gradient sampling regression, buffer growth, path rendering and CoreVideo
surfaces. Apple/macOS and standalone wgpu all-targets Clippy passed with
`-D warnings`. The wgpu suite passed all 44 tests on macOS, including real Metal
atlas allocations and Naga validation of storage, WebGL and subpixel shaders.
`cargo xwin check --tests` passed for the Windows backend and its WARP test code
at `x86_64-pc-windows-msvc`. Windows runtime/FXC and Linux runtime were not
available; their native behavior is not claimed from host-side checks.

A disposable real-UI harness used the native Metal renderer and text system,
not a mock scene or image resize. A provided integrated reference binary and
the final build each rendered Home, the cached playlist, Liked, Home after
navigation, and immediate/settled Home after Settings. Each comparison ran at
1920×950 (1×) and 3840×1900 (2×), using the same private profile. All captures
reported paused playback; foreground application and mouse position stayed
unchanged. These headless captures are visual evidence, not native footprint
measurements.

| Scene | Changed pixels at 1× / 1,824,000 | Changed pixels at 2× / 7,296,000 |
| --- | ---: | ---: |
| Home | 970 | 1,191 |
| Playlist | 818 | 2,411 |
| Liked | 819 | 2,737 |
| Home after navigation | 992 | 1,212 |
| Immediate Home after Settings | 1,001 | 1,563 |
| Settled Home after Settings | 1,001 | 1,563 |

Maximum channel differences were 72 at 1× and 91 at 2×; diagnostic overlays
locate the larger changes at image perimeters, where fragment-local clamping
corrects neighboring/uninitialized texel bleed. They are not claimed as
pixel-identical to the previous erroneous sampling. The gradient test verifies
bilinear interiors without a vertex-UV inset or rescaling. Immediate and settled
Home captures were pixel-identical within each build at both scales, exercising
cached-scene replay without a delayed image redraw. Native procedural-vertex
A/B screenshots were also pixel-identical.

Standalone backend lockfiles now name guillotiere consistently with their
manifests while retaining registry GPUI/taffy provenance; temporary host-check
patch overrides are not shipped in those lockfiles. Production tracing and
disposable capture/check harnesses are not part of the patch.

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

## Idle command storage (macOS)

The existing three-unused-frame drawable-parking transition also releases an
idle renderer-owned `MTLCommandQueue`. An in-flight frame count is updated by
the existing Metal completion handler. A busy queue is kept: a resumed frame
must not overtake earlier GPU work by submitting on another queue. The next
actual draw recreates only a retired queue; the device, backing layer, pipelines,
atlas, viewport resolution and triple buffering stay unchanged. There is no GPU
wait, additional timer or idle wakeup. A kept busy queue can retire on a later
normal parking transition. iOS and headless callers do not invoke the macOS hook.

An isolated 64×64 Metal clear probe fell from 6.594 to 5.579 MiB physical
footprint when its completed queue was released; `currentAllocatedSize` fell
from 0.906 to 0.453 MiB. A 1920×950 clear probe measured queue recreation at
0.146–0.305 ms and encoding at 0.293–0.466 ms after idle retirement. Completed
clears took 3.456–4.288 ms versus 0.331–2.277 ms with a retained queue, excluding
each process's first submission. These are completion timings, not display-vsync
measurements; the small resume cost does not change continuous-frame pacing.

Signed release Turntable A/B, 1920×950 at 1×, cached signed-in Home, private
profile, 30-second settle, no playback, back-to-back `/tmp/ttm.sh` runs:

| Queue policy | Physical footprint MiB | IOSurface MiB | IOAccelerator dirty MiB | Owned-unmapped graphics MiB | Malloc Small MiB |
| --- | ---: | ---: | ---: | ---: | ---: |
| Baseline retained queue | 48.048 | 7.094 | 5.313 | 1.234 | 12.547 |
| Retired idle queue | 46.970 | 7.094 | 5.406 | 0.000 | 12.500 |

The repeatable saving is the owned-unmapped graphics charge; ordinary atlas
textures remain resident. Recycled instance-buffer counts and Rust/system heap
residency vary between launches, so the total alone is not the attribution.
Both processes sampled 0.0% idle CPU. The approximately 4.5 MiB previously
attributed to encoder creation ends at `IOGPUResourceCreate` and
`IOGPUMetalResourcePoolCreatePooledResource`: it is command-storage VM backing,
not demonstrated malloc heap. Its unused pages were already zero-dirty/purgeable
before retirement and are not counted again as physical-footprint savings.

Native regression coverage parks/resumes the same layer and queue repeatedly,
including transaction presentation, overlay identity and an in-flight headless
frame followed by pixel readback of cached sprites on the new queue.

This queue-retirement operation is Metal-specific. wgpu's shared device/queue
and D3D11's device/immediate context have no direct equivalent to dropping this
per-window Metal queue. Those renderers are intentionally unchanged; a port
would first need backend-specific idle driver-cache attribution and a lifecycle
that does not disrupt other windows or in-flight submissions.

### Idle instance-buffer trimming

The same parking hook retains one completed instance buffer and releases excess
free buffers left by startup or resize bursts. Buffers still owned by in-flight
frames are not touched; their existing completion callbacks return them normally.
The pool's geometric growth and current capacity are unchanged, and retaining
one buffer keeps an ordinary first resumed frame warm. The pool is shared, but
only currently unused buffers are discarded; other windows' active frames keep
their own resources.

In the same back-to-back comparison, keeping the idle queue retired and changing
only this pool policy reduced IOAccelerator dirty bytes from 5,668,864 to
5,537,792 (128 KiB). IOSurface stayed at 7,438,336 bytes and owned-unmapped
graphics stayed at zero. Total physical footprint was 49,251,288 → 49,185,776
bytes (−64 KiB); Malloc Small varied +80 KiB and the Rust heap −64 KiB. The
graphics-category reduction, rather than the drifting total, is the kept result.
The combined queue/buffer policy saved 1.266 MiB of charged graphics memory
against the 50,381,808-byte baseline, with a 49,185,776-byte final total in this
assay. The idle-pool regression checks that one completed buffer is retained and
actually reused; cached-sprite readback also exercises parking while another
headless submission is still in flight.

The pending-frame regression holds an encoded command buffer before submission,
verifies parking keeps its queue, then commits/waits and verifies that the now
idle queue can retire. This protects cross-queue presentation ordering without
waiting for GPU work on the application's idle path.

A separate smaller-atlas experiment was rejected: reducing minimum pages from
512×512 to 256×256 changed IOAccelerator dirty bytes from 5,570,560 to 5,816,320
(+240 KiB), not a saving. Four 1040-KiB color pages became seventeen 272-KiB
pages; smaller glyph pages did not offset the extra color-page fragmentation.
At that stage the original 512-pixel page policy was retained. No texture
resolution, pixel format, sampling or atlas ownership changed in those two optimizations.

The full-Home baseline and compact-idle-policy window captures were pixel-identical:
zero changed pixels out of 1,824,000 at the unchanged 1920×950 resolution. Later
fresh and reused profiles showed the baseline stuck on its Loading/empty-player
screen, so those lower totals are not substituted for the signed-in Home assay.
The final guarded release passed three occlude/reveal cycles on that
screen with zero changed pixels in each cycle. Occlusion used a temporary,
nonactivating opaque window placed immediately above the owned benchmark window,
not system input or controls directed at the user's application.

The final safety-guard A/B on the Loading page left all three graphics categories
unchanged versus the previous policy: 7,438,336 IOSurface, 1,032,192 IOAccelerator
dirty and 1,294,336 owned-unmapped bytes. Its 37,389,272 → 37,716,952 total
variation is not reported as a Home improvement. Native verification passed all
18 Apple and 12 macOS tests, including pending-queue ordering and real GPU pixel
readback, plus strict Clippy for both crates. Their `test-support` features were
unified through a temporary Cargo harness because Cargo rejects explicit feature
selection for these dependency packages outside workspace membership. The harness
was removed after verification. The release build and workspace formatting check
also passed.

Also rejected: calling the same retirement functions directly from the hidden
occlusion-notification branch. In the Loading-screen comparison, baseline,
idle-tail-only and notification-retirement builds all retained 7,438,336 IOSurface,
1,032,192 IOAccelerator-dirty and 1,294,336 owned-unmapped graphics bytes. Totals
were 38,306,776 / 37,323,712 / 38,274,008 bytes; Rust-heap drift, not graphics,
accounted for the variation. The unproven extra hook was removed.

## Regression coverage

The native Metal tests cover compact-page sharing, independent release of large cover textures, tile-space reuse, buffer growth preserving earlier bindings, lazy/reused/resized/released path targets, and rendered image/path pixels after allocation and lifetime changes.

## Memory accounting

Metal shared buffers and atlas textures are demand-backed: allocated capacity is not the same as resident memory. RSS and physical footprint (the Activity Monitor Memory column) account graphics backing differently. Measure both, and include IOAccelerator graphics, IOSurface and owned-unmapped graphics categories. A first render submission can temporarily allocate over 100 MiB of driver backing even for a 64×64 clear; it can retire a few seconds after submissions stop. Reducing queue capacity from 64 to 3 did not reduce that allocation in the isolated Metal experiment, so this patch does not change command-queue capacity.

### Continuous-playback attribution (2026-10-14)

A startup Metal System Trace with `MallocStackLogging=1` showed 26 driver `Wire 4.00 MiB
System Memory` events at the first real submission (1.692–1.701 seconds). The app's peak
`MTLDevice.currentAllocatedSize` was only 28,737,536 bytes. The approximately 104 MiB constant
submission charge is driver-internal GPU backing, not the renderer's application-visible
textures or instance buffers. Apple's trace does not name the private backing's purpose.

A 64×64 native probe reproduced approximately 115 MiB footprint with a render clear despite
less than 1 MiB of app Metal capacity. Blit and compute submissions also incurred large backing
(approximately 82 and 78 MiB total footprint); an empty command buffer remained around 5 MiB.
The backing retired about three seconds after nonempty submissions stopped. Reducing queue
capacity did not avoid it. Full-viewport MSAA/path targets were absent from the path-free Home
scene, and its real Metal trace used one render encoder. Disabling antialiasing, removing
triple buffering or replacing the renderer with compute/blit would therefore miss the cause.

GPUI's cached regions cache layout/paint, not the rendered framebuffer. Every requested draw
still writes the scene's instance data, clears the drawable, traverses all primitive batches
and submits a command buffer. Four-bar GPUI animations invalidate their containing region at
15 fps; the old whole-second Shell playback pump additionally invalidated all seven regions
even when the core had not changed. The playhead itself already avoided those region renders.

In a ten-second Time Profiler capture of the real drawing path, 294 of 540 sampled CPU-ms were
in GPUI layout/paint/frame completion, 20 in Metal/submission and 27 in the display link; 199
were other or missing stacks. Taffy layout, bounds-tree insertion and frame copies were visible
leaves. This was mainly repeated UI work, not audio decoding or shader compilation. The shared
Shell deadline fix and native compositor application cutover remove that steady frame demand
without changing GPU rendering quality; ordinary interaction and genuine state changes still
render normally. The cross-platform native paint boundary is documented in
`../gpui-pre/TURNTABLE.md`.

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
