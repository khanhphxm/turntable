# Turntable patch: bounded CDN body buffering

Upstream: `librespot-audio` 0.8.0 (pristine crate in the preceding vendor commit).

`src/fetch/receive.rs` forwards each HTTP data frame to the existing temporary-file
writer through a bounded eight-message channel. Previously each ranged response was
collected and coalesced into one contiguous allocation before being written to disk.
At fast measured network throughput, a range can cover the rest of a song, so that
path could temporarily hold two copies of several megabytes of compressed audio.

The downloader's range sizes, prefetch timing, throughput estimate, retry policy,
single download slot, on-disk cache and decoder buffers are unchanged. Written
prefixes become readable before an HTTP body finishes. After an incomplete response,
only the unreceived suffix is released for the existing range retry mechanism.
Backpressure ends immediately when the file writer is closed.

Offline tests in `src/fetch/receive/tests.rs` cover incremental reads, a seek while a
response is paused, noncontiguous ranges, final-range truncation/EOF, short bodies,
connection failures before headers and mid-body, rejected HTTP responses, retries,
and writer cancellation. All HTTP traffic in these tests is localhost-only.

The selected `transfer_memory_is_bounded_for_a_three_minute_320_kbps_track` test uses
dhat to assert that forwarding a 7,200,000-byte (180 s at 320 kbps) body in 16 KiB
frames peaks below 256 KiB, independent of total response size. Select only this test
with `--ignored --test-threads=2` because allocation counters are process-wide. Set
`TURNTABLE_AUDIO_COLLECT_BASELINE=1` for the former collect/coalesce control, or
`TURNTABLE_AUDIO_MEMORY_PAUSE=1` to attach a native memory profiler after the run.
These measurements do not start playback or connect to Spotify.

Measured control versus streaming for that body: peak requested Rust heap
14,416,448 → 151,488 bytes (13.60 MiB less); total allocation traffic
14,432,705 → 7,204,096 bytes. In separate instances of the same test executable,
RSS was 21,776 → 7,856 KiB, physical footprint approximately 16 MiB → 2,464 KiB,
and dirty malloc pages 14,432 → 560 KiB. Both paused CPU samples were 0.0%.
This is a fast-CDN large-range/allocator-retention saving, not a claim that idle
Home becomes 13.60 MiB smaller. No real playback was used for verification.
