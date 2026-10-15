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
reduce the supported element or event size. Allocated chunks are reused across frames; this
does not add allocation churn to cached draws, shrink text/layout caches, change rendering or
schedule idle work. The mechanism is shared by every platform.

Regression tests cover lazy allocation, reuse, values larger than a normal chunk, alignment,
destructor execution, stale references and nested clear deferral.

On macOS arm64, back-to-back signed release runs (1920×950 at 1×, Home, 30 seconds idle)
reduced jemalloc's dirty `App-Specific Tag 15` from 13,352,960 to 11,501,568 bytes
(−1.77 MiB) and physical footprint from 50,889,688 to 48,956,400 bytes (−1.84 MiB).
IOSurface (7,438,336 bytes), IOAccelerator graphics (5,570,560 bytes) and owned unmapped
graphics (1,294,336 bytes) were unchanged. System Malloc Small changed by −65,536 bytes;
the change itself only affects Rust arena allocations.

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


