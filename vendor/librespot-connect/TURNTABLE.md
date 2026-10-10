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
- `src/state/tracks.rs` preserves context/shuffle information and queued tracks
  beyond the bounded context preview, with offline regression tests.
- Automatic next-track loads preserve the audio ring and coordinate the
  playback crate's opt-in crossfade event; manual loads, seeks and stops cancel
  stale overlap requests until the decoder acknowledges the discontinuity.

The manifest removes the unused upstream `tokio-stream` dependency. The app
workspace supplies the vendored core and playback replacements via Cargo
patches. MIT requires retained license notices, not per-file modification notices.
