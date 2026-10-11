# Turntable's gpui-pre-macos patch

Based on gpui-pre-macos 0.3.9 (Apache-2.0), the registry snapshot of Zed
`93d745668805029476e6d7fbb19652116fed51de`. The registry's source, normalized
manifest and license are retained. The workspace patches crates.io to this copy.

The normalized manifest additionally allows upstream's deprecated `cocoa`
usage for this path dependency, matching registry builds' capped warnings.
Changed upstream files carry a Turntable modification notice pointing here;
`src/frame_activity.rs` is Turntable's added frame-demand policy, not an
upstream file. The retained Apache text and the MIT display-link bindings
notice are included in the distributed root `THIRD_PARTY_NOTICES`.

## Demand-driven display links

The upstream macOS backend subscribes every visible window to CVDisplayLink for
its entire visible lifetime. GPUI skips drawing a clean window, but the display
link's I/O thread and main-queue frame callback still wake at the display rate.

- `WindowFrameSource` unsubscribes after three frames with no draw or new frame
  demand. The shared display link stops when its last window unsubscribes.
- `MacWindow::frame_waker` connects GPUI's existing dirty-window invalidator to
  the frame source. `notify`, `refresh`, `on_next_frame` and
  `request_animation_frame` all wake it, including deferred/throttled frames.
- Restarting a stopped source queues one immediate main-queue callback, so a
  background completion or hover redraw does not wait for CVDisplayLink startup.
  Further requests while running only renew the idle tail: animations remain
  vsync-paced rather than creating a self-rescheduling main-queue busy loop.
- Native keyboard/mouse/drag events, resizing/backing-scale changes, appearance,
  occlusion, screen and existing activation/layer-display paths re-arm the link.
  Hidden windows remain unsubscribed; revealing one restarts pending animations.
- Re-arming a running source only renews a demand flag, avoiding AppKit screen
  queries and NSString allocation on each animation frame. Re-arming on the same
  display is idempotent. Moving displays still changes the subscription.
  The upstream immortal per-display registry, safe dispatch-source
  teardown and CoreVideo lock ordering are unchanged.

`frame_activity::tests` covers parking, continuous animation demand, restarting
and resetting the idle tail. The policy is pure Rust and can be tested without
upstream's feature-gated native/headless test dependencies:

```sh
rustc --edition=2024 --test vendor/gpui-pre-macos/src/frame_activity.rs -o target/frame-activity-tests
target/frame-activity-tests
```

Turntable's existing playhead test covers its pending-next-frame chain, pause
transition and high-refresh-rate cap.
For an idle CPU/RSS sample on macOS, run `top -l 3 -s 10 -pid <PID> -stats pid,command,cpu,mem`.
Set `TURNTABLE_HOME` to an isolated profile copy with a distinct Connect device
name and regenerated device ID; never benchmark with the live user profile.
