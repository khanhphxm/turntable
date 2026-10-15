# Turntable's GPUI native-element paint boundary

Based on the pristine published `gpui-pre` 0.3.9 sources (Apache-2.0).

## Native compositing context

`Window::native_element_paint_context` exposes effective ancestor opacity, the shared repeating
animation epoch and current foreground-overlay bounds without exposing any OS types. Tooltips use their actual bounds;
deferred panels, prompts and dragged views conservatively occlude the viewport. Applications
can keep independently composited content from drawing above GPUI overlays and can use normal
GPUI painting while it is occluded.

`Window::observe_native_element_paint` observes completed scenes, including cached paint reuse,
without requesting display frames. If a native element's fallback decision changes, its observer
may request a refresh; GPUI rebuilds that scene once before presentation. This keeps cached
native content and newly appearing or disappearing overlays consistent in the same displayed
frame. Applications without native observers retain the original single paint path.

Re-entrant platform frame requests preserve pending force-render state and wake the platform
before returning without borrowing the app. Demand-driven backends can therefore retry after
the in-progress draw unwinds instead of losing the native invalidation.

`src/window.rs` carries this boundary; see Smooth wheel scrolling below for the other modified files. Layout,
input handling, scene primitives, shaders and native renderer interfaces are unchanged.

## Application compositor use

Turntable uses the boundary for long-running seek-fill, hover-knob, elapsed-clock and four-bar
playing-indicator animations. The platform-neutral model describes geometry, colors, text,
shadows and one scalar property as monotonic start/rate/duration motion. Equalizer bars retain
their four triangular periods/phases, including GPUI's shared epoch and reduced-motion static
state. Playback motion remains essential; seek dragging uses the original GPUI foreground and
input capture. Windows, Linux and headless windows retain GPUI drawing.

The macOS adapter creates Core Animation layers and applies those generic descriptions; it
does not decide widget roles, playback scheduling or overlay fallback. GPUI paint state owns
the native canvas, while layout retains only a weak link. Cached paint keeps the canvas alive;
hidden, clipped-out or removed elements release it. Native content honors the content mask and
effective ancestor opacity. App overlays and GPUI's deferred overlays/tooltips select GPUI
fallback, including when the containing region was cached. The clock and seek fill report their
compositing modes independently, so a tooltip over only the seek bar still resumes pixel-step
GPUI updates. The native hover knob retains its 12-pixel white circle, green fill and two
medium-shadow components.

Normal application state changes, layout, pointer interaction and timed lyric transitions
still draw the GPUI scene. Compositor clock updates and scalar motion do not request GPUI
frames, so the display link can park and submission-only graphics backing can retire. Neither
four-sample Metal antialiasing nor triple buffering is changed by this application cutover.

Regression coverage includes the shared playback/bar models, paused/end/zero-duration
progress, inherited opacity, cached foreground-overlay changes, same-frame fallback repaint,
GPUI fallback availability and native layer clipping/scalar/lifetime behavior.

## Smooth wheel scrolling

A mouse-wheel notch arrives as an imprecise, line-based delta. Upstream applied it in one jump;
browsers ease it in over a few frames instead, which reads as smooth on high-refresh displays.
`SmoothWheel` (`src/gestures.rs`) queues such deltas and releases them exponentially (40 ms time
constant, a notch lands in about 120 ms); `Div` scroll containers (`src/elements/div.rs`) and
`List` (`src/elements/list.rs`) step it once per painted frame and request the next frame until
it lands. Precise deltas (trackpads, which already arrive per frame with momentum) apply at once
and cancel any easing in flight; reversing direction drops the rest of the old one. This is
shared code: every platform's wheel events take the same path.

## Compact transient arenas

The app's element and event arenas, and the fallback element arena, allocate 64 KiB chunks
instead of eagerly reserving 1 MiB each. Empty arenas allocate no chunk. `src/arena.rs` gives
an oversized or over-aligned value a suitably sized/aligned chunk, so compact defaults do not
reduce the supported element or event size. Allocated chunks are reused while the frame source
is active; normal cached draws do not shrink text/layout caches, change rendering or schedule
idle work. Quiescent reclamation below releases scratch after that source parks. The mechanism
is shared by every platform.

Regression tests cover lazy allocation, reuse, values larger than a normal chunk, alignment,
destructor execution, stale references and nested clear deferral.

On macOS arm64, back-to-back signed release runs (1920×950 at 1×, Home, 30 seconds idle)
reduced jemalloc's dirty `App-Specific Tag 15` from 13,352,960 to 11,501,568 bytes
(−1.77 MiB) and physical footprint from 50,889,688 to 48,956,400 bytes (−1.84 MiB).
IOSurface (7,438,336 bytes), IOAccelerator graphics (5,570,560 bytes) and owned unmapped
graphics (1,294,336 bytes) were unchanged. System Malloc Small changed by −65,536 bytes;
the change itself only affects Rust arena allocations.

## Bounded transient peak retention

The compact arenas also release an unused chunk tail after two clears use less
than a quarter of a peak larger than four chunks. They keep at least two chunks
of headroom. A similarly oversized destructor-record vector is replaced without
copying cleared entries. Stable and alternating full/small workloads keep their
capacity; reopening a formerly large workload grows normally again. Active arena
scopes still defer both dropping and reclamation. No timer or redraw is added.

Relative to the preceding layout/tree/dealer changes, matched offscreen release
samples reduced jemalloc dirty footprint by 327,680 bytes on Home, 868,352 on
Playlist, 917,504 on Liked Songs and 720,896 on Home after navigation. Total
footprint fell by 0.45–1.05 MiB, with independent system/GPU variation. Regression
tests cover the two-clear threshold, alternating reuse, destructor execution and
reclamation of oversized destructor-record storage.

At 1x, Home, Playlist, Liked Songs, Home after navigation and immediate/settled
Back after Settings matched the integrated base pixel-for-pixel. At 2x, text and
image interiors remained identical; differences were confined to one-pixel image
boundaries. Repeating the identical base binary reproduced those boundary changes
(up to 87/255 per channel), and its repeated Playlist and immediate/settled Back
frames exactly matched the candidate. The existing unpadded-atlas/linear-sampling
boundary behavior is independent of these heap changes; no renderer change is
included here.

## Layout capacity after cached frames

`src/taffy.rs` releases Taffy's peak node storage after two consecutive frames
use less than a quarter of that peak (and the peak exceeded 256 nodes). Stable
full layouts and alternating full/cached layouts retain their reusable capacity;
the replacement reserves enough nodes for the smaller frame and keeps rounding
disabled. Layout IDs are valid only within the frame that requested them, and
cached views retain bounds and paint operations rather than Taffy node IDs.
This runs inside normal frame cleanup, without timers, extra redraws or idle work.

On macOS arm64, back-to-back offscreen Metal release runs at 1920×950, 1x,
reduced jemalloc dirty footprint relative to the integrated arena/image changes:
Home 9,142,272 → 8,454,144 bytes; Playlist 10,010,624 → 9,306,112;
Liked Songs 10,272,768 → 9,437,184; Home after both pages 10,240,000 →
9,551,872. Total footprint fell by 0.53–0.84 MiB across those stages. GPU
allocation variation is independent of this CPU-only layout change. The locked
desktop made native-window painting invalid, so these offscreen measurements do
not include the native drawable or establish a new native idle CPU result.

Regression tests cover the two-small-frame threshold, stable/alternating reuse
and preservation of fractional layouts after replacement.

## Compact bounds-tree leaves

`src/bounds_tree.rs` keeps its fixed child arrays in a separate reusable vector,
indexed only by internal nodes. Leaves no longer reserve twelve child slots just
because they share an enum with internal nodes. Tree insertion, overlap ordering,
traversal and GPU primitives are unchanged; clearing resets both vectors without
discarding their reusable capacity. This is shared across all renderers/platforms.

In the same offscreen workload, relative to the layout-capacity change, jemalloc
dirty footprint fell by 180,224 bytes on Playlist, 147,456 on Liked Songs and
180,224 on Home after navigation. Total footprint fell by 0.47–0.59 MiB, with
system-malloc/GPU variation not attributed to this CPU-only tree change. The
initial Home pair had different asynchronously loaded artist/artwork populations
and is not a comparable sample. Regression coverage retains the existing random
overlap oracle and adds leaf-size and repeated large-tree clear/reuse checks.

## Regenerable image pixels

`RenderImage::new_with_reloader` accepts an already-decoded single BGRA frame and a
thread-safe callback that can reproduce it. `as_bytes` returns a `Cow`: ordinary
frames are still borrowed without copying; regenerable images transfer their first
buffer into the atlas upload and generate an owned buffer only on a later atlas
miss. Dimensions, delay, byte length and image identity remain available without
decoding. The reload must preserve dimensions; a failed or mismatched reload is
reported as unavailable pixels instead of uploading an invalid byte range.

`src/assets.rs` owns this generic mechanism; `Window::paint_image` in `src/window.rs`
passes the bytes directly to the existing atlas callback. Atlas hits, layout and
cached paint do not reload. No renderer/backend, texture resolution, image sampling,
animation or atlas page policy changes. The default persistent constructor remains
appropriate for images that cannot be reproduced cheaply.

Turntable's cover cache supplies the same JPEG/PNG/WebP resize, blur and BGRA
conversion for initial background decoding and reloads, using weak encoded-source
references so eviction still releases artwork. Full-size CPU pixels need not live
as long as their uploaded tiles. Cache budgets use the uncompressed byte length
without consuming initial pixels. The initial background decode also prepares a
≤96 px BGRA preview before transferring the full buffer; retiring a page reuses
that preview without synchronously decoding large images on the UI thread. Both
full resource size and the small preview count against the active cache budget.
Warm previews retain the existing no-placeholder Back behavior.

Tests cover buffer ownership transfer without a copy, metadata after transfer,
borrowed persistent frames, invalid indices, failed/mismatched reloads, and a real
image element that reloads only after its atlas entry is explicitly removed.

Measured on macOS/Apple Silicon, 1920×950 at 1x, using the real app in the
offscreen Metal harness and the same cached account. Values are dirty footprint
MiB (baseline 54bf15b → this patch); offscreen targets omit the native drawable:

| Navigation stage | jemalloc (Tag 15) | IOAccelerator (graphics) | Total |
| --- | ---: | ---: | ---: |
| Home | 10.953 → 9.344 | 3.312 → 3.344 | 29.094 → 27.438 |
| Playlist | 11.156 → 10.219 | 2.719 → 2.781 | 28.813 → 27.860 |
| Liked Songs | 11.000 → 10.328 | 3.000 → 3.062 | 29.001 → 28.376 |
| Home after both pages | 12.375 → 10.422 | 4.234 → 4.594 | 31.579 → 29.938 |

This is a CPU-pixel reduction, not a GPU-allocation reduction: tile dimensions
are unchanged and atlas packing varies with decode completion order. Full-frame
1x screenshots, immediate Back and settled Back at 1x/2x matched pixel-for-pixel;
the initial 2x shot differed in 191 edge colour channels (maximum 10/255) at one
circular image's antialiased boundary, with unchanged image interiors and text.
Preparing the previews off-thread removed the rejected synchronous variant's
23–28 ms navigation penalty. Native idle samples remained 0.0% CPU. Final
native-window footprint comparison was inconclusive after the desktop locked:
Home's covers were not painted while occluded, even with the display awake.


## Quiescent frame storage

`PlatformWindow::on_frame_idle` reports an existing platform frame-source park, not
another frame request. macOS emits it at the same three-unused-frame transition
that retires Metal command storage, outside the native window lock. GPUI also
reclaims scratch through the existing hidden/occluded visibility callback. No
timer, new frame, animation-tail change or background purge thread is introduced.

`src/window.rs` releases the already-cleared spare frame, empty layout-engine
storage, completed scene-building bounds index, and empty, unscoped app/fallback
arenas. The live rendered frame's paint records, element state, focus, hitboxes,
input handlers and listeners remain intact, as do text, image and page caches.
An active draw is rejected before borrowing App; live arena values/scopes and
active layout nodes independently prevent their storage from being released.
The next cached or full draw grows only the scratch it needs.

Back-to-back signed native macOS release runs, 1920×950 at 1x, Home, 30 seconds
idle, used the same isolated account/profile. "Before" includes the integrated
GPU/image changes and preceding heap optimizations; only the last column adds
quiescent heap reclamation. All values below are dirty bytes except total:

| Category | Original 54bf15b | Before | Quiescent heap |
| --- | ---: | ---: | ---: |
| jemalloc / App-Specific Tag 15 | 11,337,728 | 8,749,056 | 6,963,200 |
| Malloc Small | 13,615,104 | 13,418,496 | 13,647,872 |
| IOSurface | 7,438,336 | 7,438,336 | 7,438,336 |
| IOAccelerator graphics | 5,570,560 | 5,537,792 | 5,537,792 |
| Owned unmapped graphics | 1,294,336 | 0 | 0 |
| Stack | 819,200 | 786,432 | 819,200 |
| Physical footprint | 49,300,440 | 45,106,136 | 43,615,192 |

The incremental reduction is 1,785,856 bytes (1.70 MiB) in jemalloc and
1,490,944 bytes (1.42 MiB) in total; system-malloc variation is not attributed to
this CPU-only change. Idle CPU remained 0.0%. Native before/after screenshots
were pixel-identical at the full 1920×950 resolution.

The real-app offscreen Metal probe compared each pre-idle frame with the retained
scene, first cached replay and first forced full relayout after the callback:
all 36 comparisons were pixel-identical. It covered Home, Playlist, Liked Songs,
Home after navigation, and immediate/settled Back from Settings at 1920×950 1x
and 1280×800 2x (2560×1600 output). First cached draws took 0.366–0.727 ms;
first full relayouts took 1.424–2.059 ms. Warm controls were 0.155–1.195 ms and
1.133–7.063 ms respectively. No visible redraw delay was introduced; the largest
observed added cached/full CPU time was 0.326/0.554 ms. The probe issued no
playback command and left the real pointer and frontmost application unchanged.

`crates/turntable/tests/gpui_idle.rs` exercises real GPUI window callbacks,
retained paint/focus/keyboard input, cached replay, and full relayout after idle
or hide/reveal. It runs in the normal workspace suite, avoiding unrelated missing
upstream unit-test font fixtures: `cargo test -p turntable --test gpui_idle`.
Arena/layout unit tests additionally cover live-value/scope/node safety,
storage release/reuse and unchanged fractional layout rounding.

The reclamation itself is shared code, including the hidden-window path.
Visible-idle notification is implemented on macOS and the deterministic test
platform. Windows' scheduler parks on a background condvar, not a corresponding
UI-thread window transition; it needs a synchronized main-thread park event
before it can call this hook. wgpu owns GPU submission, not the platform frame
source: its Linux/platform scheduler would likewise need to emit this event.
No speculative Windows/wgpu scheduling change is included.

