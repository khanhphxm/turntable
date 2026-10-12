#!/usr/bin/env python3
"""Sample Linux process CPU and kernel PSS/RSS; never control playback."""
import argparse
import json
import math
import os
from pathlib import Path
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("pid", type=int)
parser.add_argument("--seconds", type=float, default=30.0)
args = parser.parse_args()
if args.pid <= 0 or not math.isfinite(args.seconds) or args.seconds <= 0:
    parser.error("PID and finite measurement duration must be positive")
proc = Path("/proc") / str(args.pid)
hz = os.sysconf("SC_CLK_TCK")


def sample_cpu():
    fields = (proc / "stat").read_text().rsplit(")", 1)[1].split()
    # The suffix starts at field 3 (state); fields 14/15 are user/system ticks.
    return (int(fields[11]) + int(fields[12])) / hz, int(fields[19]), time.monotonic()


before, started_at, begin = sample_cpu()
time.sleep(args.seconds)
after, current_start, end = sample_cpu()
if current_start != started_at or after < before:
    raise RuntimeError("process was replaced during the measurement")
memory = {}
for line in (proc / "smaps_rollup").read_text().splitlines():
    fields = line.split()
    if len(fields) == 3 and fields[2] == "kB":
        memory[fields[0].rstrip(":")] = int(fields[1])
threads = int(next(line.split()[1] for line in (proc / "status").read_text().splitlines() if line.startswith("Threads:")))

# Descriptor duplicates share a DRM client ID; do not add the same driver
# allocation/engine counter more than once. Capacity is NOT resident PSS.
graphics = {}
for fd in (proc / "fdinfo").iterdir():
    try:
        info = dict(line.split(":", 1) for line in fd.read_text().splitlines() if ":" in line)
    except FileNotFoundError:
        continue
    if "drm-client-id" in info:
        key = info.get("drm-pdev", "").strip() + ":" + info["drm-client-id"].strip()
        graphics[key] = {name: value.strip() for name, value in info.items() if name.startswith(("drm-", "amd-"))}

print(json.dumps({
    "pid": args.pid,
    "wall_seconds": end - begin,
    "cpu_percent": 100 * (after - before) / (end - begin),
    "cpu_convention": "100% is one fully utilized core",
    "memory_kib": memory,
    "threads": threads,
    "graphics_capacity_and_counters": graphics,
}, indent=2))
