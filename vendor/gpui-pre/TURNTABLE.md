# Turntable's GPUI native-element paint boundary

Based on the pristine published `gpui-pre` 0.3.9 sources (Apache-2.0).

## Native compositing context

`Window::native_element_paint_context` exposes effective ancestor opacity and the current
foreground-overlay bounds without exposing any OS types. Tooltips use their actual bounds;
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
