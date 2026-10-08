#!/usr/bin/env python3
"""Fill the checked-in Homebrew templates from the published SHA256SUMS file."""

import argparse
from pathlib import Path
import re


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--checksums", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True, help="homebrew-tap checkout")
    args = parser.parse_args()
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:\+[0-9A-Za-z.-]+)?", args.version):
        parser.error("Only stable release versions can update the tap")
    checksums = {}
    for line in args.checksums.read_text().splitlines():
        digest, filename = line.split(maxsplit=1)
        if not re.fullmatch(r"[0-9a-f]{64}", digest):
            parser.error(f"Invalid SHA-256: {digest}")
        checksums[filename.removeprefix("*")] = digest
    replacements = {"@VERSION@": args.version}
    for token, platform, extension in (
        ("MACOS_UNIVERSAL", "macos-universal", "zip"),
        ("LINUX_X86_64", "linux-x86_64", "tar.gz"),
        ("LINUX_AARCH64", "linux-aarch64", "tar.gz"),
    ):
        filename = f"turntable-{args.version}-{platform}.{extension}"
        replacements[f"@{token}_SHA256@"] = checksums[filename]
    templates = Path(__file__).resolve().parent
    for source, destination in (("turntable.rb", "Casks/turntable.rb"), ("formula/turntable.rb", "Formula/turntable.rb")):
        text = (templates / source).read_text()
        for token, value in replacements.items():
            text = text.replace(token, value)
        if re.search(r"@[A-Z0-9_]+@", text):
            parser.error(f"Unfilled template token in {source}")
        target = args.output / destination
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text)
        print(f"Rendered {target}")


if __name__ == "__main__":
    main()
