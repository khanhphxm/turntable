#!/usr/bin/env python3
"""Compare two offline audio-bench builds using real codec fixtures, never Spotify/devices."""

import array
import json
import math
from pathlib import Path
import subprocess
import sys
import tempfile


def pcm(path):
    samples = array.array("f")
    samples.frombytes(path.read_bytes())
    if sys.byteorder != "little":
        samples.byteswap()
    return samples


def compare(before, after, fixtures):
    cases = [
        ("off", [], 0.0),
        ("normalized-overlap", ["--normalize", "normal", "--crossfade-ms", "1500"], 0.0),
        (
            "full-48k",
            ["--normalize", "normal", "--eq", "rock", "--crossfade-ms", "1500", "--rate", "48000", "--frames", "512"],
            5e-7,
        ),
    ]
    with tempfile.TemporaryDirectory(prefix="turntable-audio-equivalence-") as scratch:
        scratch = Path(scratch)
        for codec in ("vorbis.ogg", "mp3.mp3", "flac.flac"):
            for name, args, tolerance in cases:
                paths = []
                for label, binary in (("before", before), ("after", after)):
                    path = scratch / f"{codec}-{name}-{label}.f32"
                    result = subprocess.run(
                        [str(binary), str(fixtures / codec), *args, "--output", str(path)],
                        check=True, capture_output=True, text=True,
                    )
                    metrics = json.loads(result.stdout)
                    if metrics["underruns"] or metrics["render_allocations"]:
                        raise AssertionError(f"invalid {codec}/{name}/{label} run")
                    paths.append(path)
                reference, actual = map(pcm, paths)
                common = min(len(reference), len(actual))
                # Rate-aware EOF draining may add zero padding, never remove/change audio.
                if any(reference[common:]) or any(actual[common:]):
                    raise AssertionError(f"{codec}/{name}: unequal non-silent output lengths")
                worst = 0.0
                error_power = signal_power = 0.0
                changed = 0
                for old, new in zip(reference, actual):
                    if not math.isfinite(old) or not math.isfinite(new):
                        raise AssertionError(f"{codec}/{name}: non-finite output")
                    difference = abs(old - new)
                    worst = max(worst, difference)
                    error_power += difference * difference
                    signal_power += old * old
                    changed += old != new
                if worst > tolerance:
                    raise AssertionError(f"{codec}/{name}: peak error {worst} exceeds {tolerance}")
                if tolerance == 0.0 and reference[:common].tobytes() != actual[:common].tobytes():
                    raise AssertionError(f"{codec}/{name}: non-bit-exact bypass/non-resampled output")
                print(json.dumps({
                    "codec": codec, "case": name, "samples": common, "changed_samples": changed,
                    "max_error": worst,
                    "error_rms": math.sqrt(error_power / max(common, 1)),
                    "snr_db": 10.0 * math.log10(signal_power / error_power) if error_power else None,
                    "bit_exact": reference[:common].tobytes() == actual[:common].tobytes(),
                }), flush=True)


if __name__ == "__main__":
    if len(sys.argv) != 4:
        raise SystemExit("usage: audio-equivalence.py BEFORE_BINARY AFTER_BINARY FIXTURE_DIRECTORY")
    compare(*(Path(arg).resolve() for arg in sys.argv[1:]))
