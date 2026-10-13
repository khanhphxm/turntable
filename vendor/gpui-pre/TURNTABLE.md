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

Only `src/window.rs` is modified. Its modification header identifies this patch. Layout,
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
fallback, including when the containing region was cached. The native hover knob retains its
12-pixel white circle, green fill and two medium-shadow components.

Normal application state changes, layout, pointer interaction and timed lyric transitions
still draw the GPUI scene. Compositor clock updates and scalar motion do not request GPUI
frames, so the display link can park and submission-only graphics backing can retire. Neither
four-sample Metal antialiasing nor triple buffering is changed by this application cutover.

Regression coverage includes the shared playback/bar models, paused/end/zero-duration
progress, inherited opacity, cached foreground-overlay changes, same-frame fallback repaint,
GPUI fallback availability and native layer clipping/scalar/lifetime behavior.
