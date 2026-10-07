#!/usr/bin/env python3
"""Render the shared SVG with resvg; construct PNG, ICO and optional ICNS assets."""

import argparse
import pathlib
import shutil
import struct
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent
SIZES = (16, 24, 32, 48, 64, 128, 256, 512, 1024)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=pathlib.Path, default=ROOT / "icons")
    parser.add_argument("--icns", action="store_true", help="also run macOS iconutil")
    args = parser.parse_args()
    if not shutil.which("resvg"):
        parser.error("resvg is required: cargo install --locked resvg --version 0.45.1")
    source = ROOT / "icon.svg"
    if not source.is_file():
        parser.error(f"missing icon source: {source}")
    args.output.mkdir(parents=True, exist_ok=True)
    for size in SIZES:
        destination = args.output / f"{size}x{size}" / "apps" / "turntable.png"
        destination.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(["resvg", "--width", str(size), "--height", str(size),
                        str(source), str(destination)], check=True)

    # Modern Windows accepts PNG-compressed ICO entries; no imaging library needed.
    entries = []
    payloads = []
    ico_sizes = tuple(size for size in SIZES if size <= 256)
    offset = 6 + 16 * len(ico_sizes)
    for size in ico_sizes:
        png = (args.output / f"{size}x{size}" / "apps" / "turntable.png").read_bytes()
        dimension = size if size < 256 else 0
        entries.append(struct.pack("<BBBBHHII", dimension, dimension, 0, 0,
                                   1, 32, len(png), offset))
        payloads.append(png)
        offset += len(png)
    (args.output / "turntable.ico").write_bytes(
        struct.pack("<HHH", 0, 1, len(entries)) + b"".join(entries + payloads))

    if args.icns:
        if sys.platform != "darwin":
            parser.error("--icns requires macOS iconutil")
        with tempfile.TemporaryDirectory(prefix="turntable-icon-") as temp:
            iconset = pathlib.Path(temp) / "turntable.iconset"
            iconset.mkdir()
            for size in (16, 32, 128, 256, 512):
                for scale in (1, 2):
                    suffix = "@2x" if scale == 2 else ""
                    shutil.copyfile(args.output / f"{size * scale}x{size * scale}" /
                                    "apps" / "turntable.png",
                                    iconset / f"icon_{size}x{size}{suffix}.png")
            subprocess.run(["iconutil", "-c", "icns", str(iconset), "-o",
                            str(args.output / "turntable.icns")], check=True)


if __name__ == "__main__":
    main()
