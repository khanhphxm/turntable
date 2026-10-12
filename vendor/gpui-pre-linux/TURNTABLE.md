# Turntable's GPUI Linux backend patch

Based on gpui-pre-linux 0.3.9 (Apache-2.0), the registry snapshot of Zed
`93d745668805029476e6d7fbb19652116fed51de`. The registry sources, normalized
manifest and Apache license are retained. The workspace patches crates.io to
this copy. Modified upstream files carry Turntable notices; `frame_request.rs`
is Turntable's added, event-loop-independent demand latch.

## Demand-driven X11 refresh

Upstream starts a periodic RandR-rate timer for every mapped, unobscured window.
GPUI avoids rendering clean scenes, but every visible window still wakes the
main loop at the display rate. Wayland already parks clean windows using
compositor callbacks; its pacing policy is retained.

- The X11 timer removes itself after a frame with no remaining demand.
- GPUI's existing `frame_waker` and `schedule_frame` hooks coalesce dirty-view,
  refresh and next-frame/animation demand. A single client-wide Calloop ping
  starts a parked source without borrowing the client or window state from a
  re-entrant invalidation callback.
- An armed timer consumes demand at the start of the frame, and demand arriving
  during that frame keeps it running. It does not send another ping each frame.
- Restarted sources respect the last frame's RandR interval; continuous
  animation, deferred/throttled frames and repeated invalidations remain paced,
  rather than forming a zero-delay event-loop busy loop.
- Mapping/revealing a window re-arms it. Hiding or fully obscuring it cancels its
  timer but preserves pending demand. Destruction removes timers and the ping
  registration using the existing connection ownership rules.
- Expose events retain their direct presentation path. Failed presentations,
  device recovery and atlas recovery explicitly schedule another frame now
  that an idle timer no longer provides implicit retries.
- XCB's socket source and after-runnable queue drain are unchanged, so parking
  does not lose native events buffered by synchronous XCB requests.

The pure Rust latch tests cover parking/restarting, coalescing, continuously
requested frames without redundant pings, and hiding/revealing pending demand:

```sh
rustc --edition=2024 --test vendor/gpui-pre-linux/src/linux/x11/frame_request.rs -o /tmp/x11-frame-request-tests
/tmp/x11-frame-request-tests
```


## Wayland dirty-window wakeup

The parked Wayland frame loop already implements `schedule_frame`, but upstream
does not connect GPUI's `frame_waker` invalidation hook. A worker notification
after parking could therefore remain unpainted until a native event or explicit
next-frame request. The patch supplies that hook using weak references to the
existing frame-loop/signal cells and Calloop ping, without borrowing window or
client state re-entrantly. Existing compositor callbacks and retry timers keep
owning the wake while armed; no periodic idle source is added.

Linux graphics in this snapshot use `gpui-pre-wgpu` (Vulkan/OpenGL), not the
older Blade renderer. Its separate resource-sizing patch is documented in
`../gpui-pre-wgpu/TURNTABLE.md`. Xvfb has no DRI3 and uses Mesa's llvmpipe software
renderer; its memory and playing CPU must not be presented as a real-GPU floor.
