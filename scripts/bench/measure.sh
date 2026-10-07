#!/bin/bash
# macOS process CPU deltas and physical memory; does not control playback.
set -euo pipefail
exec python3 - "$@" <<'PY'
import argparse
import math
import os
import subprocess
import sys
import time

parser = argparse.ArgumentParser(description='Average process CPU from ps cumulative time, then macOS footprint.')
parser.add_argument('-n', '--seconds', type=float, default=float(os.environ.get('N', '30')), help='measurement interval (default: N environment variable, or 30 seconds)')
parser.add_argument('pids', type=int, nargs='+')
args = parser.parse_args()
if not math.isfinite(args.seconds) or args.seconds <= 0:
    parser.error('seconds must be a positive finite number')
if any(pid <= 0 for pid in args.pids) or len(set(args.pids)) != len(args.pids):
    parser.error('PIDs must be positive and unique')

def cpu_seconds(pid):
    raw = subprocess.check_output(['ps', '-p', str(pid), '-o', 'time='], text=True).strip()
    if not raw:
        raise RuntimeError(f'PID {pid} is not running')
    days = 0
    if '-' in raw:
        day_text, raw = raw.split('-', 1)
        days = int(day_text)
    fields = raw.split(':')
    if len(fields) == 2:
        minutes, seconds = fields
        hours = 0
    elif len(fields) == 3:
        hours, minutes, seconds = fields
    else:
        raise RuntimeError(f'Unrecognized ps time for PID {pid}: {raw!r}')
    return days * 86400 + int(hours) * 3600 + int(minutes) * 60 + float(seconds), time.monotonic()

try:
    before = {pid: cpu_seconds(pid) for pid in args.pids}
    time.sleep(args.seconds)
    after = {pid: cpu_seconds(pid) for pid in args.pids}
    print('PID\tCPU_SECONDS\tWALL_SECONDS\tAVG_CPU_PERCENT', flush=True)
    total = 0.0
    for pid in args.pids:
        delta = after[pid][0] - before[pid][0]
        elapsed = after[pid][1] - before[pid][1]
        if delta < 0:
            raise RuntimeError(f'CPU counter decreased for PID {pid}; process may have been replaced')
        cpu = 100 * delta / elapsed
        total += cpu
        print(f'{pid}\t{delta:.2f}\t{elapsed:.3f}\t{cpu:.3f}', flush=True)
    print(f'TOTAL_AVG_CPU_PERCENT\t{total:.3f}', flush=True)
    print('CPU: 100% = one fully utilized core; memory is an end-of-window snapshot, not an average.', flush=True)
    failed = False
    for pid in args.pids:
        print(f'\n--- footprint -p {pid} ---', flush=True)
        result = subprocess.run(['footprint', '-p', str(pid)])
        failed = failed or result.returncode != 0
    if failed:
        print('footprint failed for one or more PIDs; elevated privileges may be required.', file=sys.stderr)
        sys.exit(1)
except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
    print(f'measure.sh: {error}', file=sys.stderr)
    sys.exit(1)
PY
