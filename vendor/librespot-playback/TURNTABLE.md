# Turntable's librespot-playback patch

Based on librespot-playback 0.8.0 (MIT). `LICENSE` copies the same upstream
Paul Lietar notice retained by librespot-core; the distributed root
`THIRD_PARTY_NOTICES` covers all four vendored librespot crates.

Compared with the registry source:

- `src/player.rs` uses a single-worker Tokio runtime on the dedicated player
  thread instead of one worker per core.
- `Player::set_bitrate` changes future track loaders on the player's command
  queue without resetting the sink or touching the current decoder. Existing
  successor preloads are replaced at the new quality; decoder reuse on repeat
  or explicit loads checks the originally requested bitrate. In-flight current
  loads, seeks and pause/resume keep their decoder.
- File selection retains upstream's first-available format order:
  - Normal: Vorbis 96, MP3 96, Vorbis 160, MP3 160, MP3 256, Vorbis 320, MP3 320.
  - High: Vorbis 160, MP3 160, Vorbis 96, MP3 96, MP3 256, Vorbis 320, MP3 320.
  - Very high: Vorbis 320, MP3 320, MP3 256, Vorbis 160, MP3 160, Vorbis 96, MP3 96.
  The selected quality is a preference, not a cap. Cache entries are keyed by
  file ID, so a cached lower-quality file cannot mask a newly selected format.
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
- The opt-in `audio-bench` feature can construct an outgoing crossfade source
  from a local decoder. Crossfade sources retain only the decoder and download
  controller, not the outgoing track's unused metadata. Production builds do
  not expose the offline constructor.
- Decoder packets use the host's f32 render format instead of f64. Stereo
  interleaving writes directly into each owned packet, removing the redundant
  SampleBuffer scratch and its copy. Turntable's RingSink moves the decoded
  allocation directly; all optional backends consume f32 packets, converting only when
  their device format requires it. Volume/dynamic-normalization calculations
  and integer-output dithering keep f64 intermediates. Decoder conversion
  tests compare the old widen/narrow path, including integer extrema and
  attenuation of 32-bit lossless PCM.

MIT requires retained license notices, not per-file modification notices.
