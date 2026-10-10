# Turntable's librespot-playback patch

Based on librespot-playback 0.8.0 (MIT). `LICENSE` copies the same upstream
Paul Lietar notice retained by librespot-core; the distributed root
`THIRD_PARTY_NOTICES` covers all four vendored librespot crates.

Compared with the registry source:

- `src/player.rs` uses a single-worker Tokio runtime on the dedicated player
  thread instead of one worker per core.
- The player exposes automatic `load_next` separately from manual loads. It
  emits `PlayerEvent::Crossfade` at an eligible overlap boundary, hands the
  outgoing preloaded decoder and download lifetime to the opted-in sink, and
  tracks packet duration so the overlap starts after the current packet.
- Overlaps require gapless playback, no passthrough or decoder normalization,
  an available outgoing tail and a ready next decoder. Ordinary sinks retain
  upstream gapless behavior. The host performs post-mix leveling/limiting.
- `src/audio_backend/mod.rs` adds optional sink hooks for overlap duration,
  beginning a fade, a new PCM program and discontinuity resets. Manual loads,
  seeks and stops reset pending overlaps; natural advancement preserves them.

MIT requires retained license notices, not per-file modification notices.
