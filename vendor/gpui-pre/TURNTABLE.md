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

## Compact transient storage

Element/event arenas use lazy 64 KiB chunks rather than eagerly reserving 1 MiB.
Oversized or over-aligned values get suitable chunks; active scopes still defer
clearing and reclamation. After two clears use less than a quarter of a peak
larger than four chunks, unused tails and oversized destructor storage are
released, keeping at least two chunks of headroom. Stable and alternating
workloads retain capacity; larger workloads grow normally again.

`src/taffy.rs` similarly releases peak node storage after two frames use less
than a quarter of a peak above 256 nodes. Layout IDs remain frame-local; cached
views keep bounds and paint operations, not node IDs. Replacement preserves
fractional layout. `src/bounds_tree.rs` stores child arrays only for internal
nodes, avoiding unused slots in leaves without changing overlap ordering or
clear/reuse behavior. These changes apply across platforms without extra frames
or timers.

## Regenerable image pixels

`RenderImage::new_with_reloader` transfers a decoded BGRA frame into its first
atlas upload without copying. Later misses invoke a thread-safe reloader;
ordinary images still return borrowed pixels through `as_bytes`'s `Cow`.
Metadata stays available without decoding. Reloads must preserve pixels and
dimensions; failed or mismatched reloads report unavailable pixels. Atlas hits,
layout and cached paint do not reload.

Turntable supplies the same decode/resize/blur conversion for initial pixels and
reloads, holding encoded sources weakly so eviction still releases artwork.
Background decoding also prepares a ≤96 px preview for page retirement, avoiding
large synchronous UI-thread decodes and preserving warm Back navigation. Cache
budgets count uncompressed full-size pixels and previews even after upload.
Persistent images remain available when regeneration is unsuitable; texture
resolution, sampling and explicit atlas ownership are unchanged.

## Quiescent frame storage

`PlatformWindow::on_frame_idle` reports an existing frame-source park outside
platform locks. GPUI releases the cleared spare frame, empty layout storage,
scene-building bounds index and empty, unscoped arenas. The rendered frame's
paint records, element state, focus, hitboxes and listeners remain live, as do
text, image and page caches. Active draws, arena values/scopes and layout nodes
prevent unsafe reclamation; the next draw regrows only needed scratch.

The hidden/occluded-window path is shared across platforms. Visible-idle
notification comes from macOS's three-unused-frame transition and the test
platform; Windows and Linux do not yet emit it. No timer or extra frame is added.
`crates/turntable/tests/gpui_idle.rs` covers retained input, cached replay and
full relayout after idle or hide/reveal.

## Compact scene replay records

`src/scene.rs` stores primitive kind/index pairs in paint operations instead of
duplicate primitives; paths and native surfaces move into renderer vectors.
Stable draw-order/atlas sorting remaps every replay reference to its final index,
using reusable index scratch only for batches needing reordering. Cached replay
inserts each referenced primitive once. Partial ranges, nested layer order, path
IDs, GPU layouts and renderer interfaces are unchanged across all backends.

Regression coverage includes arena alignment/destruction/reuse, layout retention
and rounding, bounds-tree overlap ordering, image transfer/reload failures, and
stable sorting with full/partial replay of mixed primitives and nested layers.
