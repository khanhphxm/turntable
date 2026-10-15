# Turntable's librespot-connect patch

Based on librespot-connect 0.8.0 (MIT). `LICENSE` copies the same upstream
Paul Lietar notice retained by librespot-core; the distributed root
`THIRD_PARTY_NOTICES` covers all four vendored librespot crates.

Compared with the registry source:

- `src/model.rs` adds checkpoint loading with the exact saved current URI and
  upcoming occurrences, including a current queued track outside its context.
- `src/spirc.rs` sends optional host snapshots of queue/context/options, passive
  Dealer clusters and initial registration results. Registration no longer
  automatically takes over playback from cached transfer state.
- Revision-bearing local playback commands report execution errors to the host.
  Native queue edits add, remove, move, clear or jump to occurrence UIDs without
  restarting playback or using Web API mutations; revisions reconcile the UI.
- Command execution errors retain their native error type in snapshots, behind
  an `Arc` for snapshot cloning. The host can recognize rate limits and display
  its existing friendly message rather than librespot's raw Display string.
- Local volume uses a latest-value watch channel, not the transport command
  queue. The old SetVolume command bypassed the 500 ms debounce by falling
  through to `notify()` on every slider step. One debounced report is in flight
  at a time, with only the newest successor retained. A 429 postpones the report
  to the session-wide backoff deadline; slider changes cannot shorten it. Local
  audio gain still applies immediately in the controller. Only an outstanding
  volume report has a timer; there is no idle polling.
- Remote volume commands use the library worker's existing per-device queue
  coalescing. After a 429 the worker retains only the latest refused volume and
  sleeps on its request channel until that backoff deadline or new user work.
  New values replace that retry, and account teardown cancels it. Ordinary
  transport and playlist writes are not replayed.
- `src/state/tracks.rs` preserves context/shuffle information and queued tracks
  beyond the bounded context preview, with offline regression tests.
- Automatic next-track loads preserve the audio ring and coordinate the
  playback crate's opt-in crossfade event; manual loads, seeks and stops cancel
  stale overlap requests until the decoder acknowledges the discontinuity.

The manifest removes the unused upstream `tokio-stream` dependency. The app
workspace supplies the vendored core and playback replacements via Cargo
patches. MIT requires retained license notices, not per-file modification notices.
