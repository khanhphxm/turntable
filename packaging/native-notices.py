#!/usr/bin/env python3
"""Retain asset/native-library licenses not represented by Rust wrappers."""
import json
from pathlib import Path
import re
import sys

metadata = json.loads(Path(sys.argv[1]).read_text())
with Path(sys.argv[2]).open("a") as output:
    def append(title, text):
        output.write(f"\n{'=' * 80}\n{title}\n\n{text}\n")

    for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
        name = package["name"]
        root = Path(package["manifest_path"]).parent
        version = package["version"]
        source = f"https://crates.io/api/v1/crates/{name}/{version}/download"
        if name == "gpui-kit-assets":
            append(f"Lucide icons (ISC) and Feather-derived icons (MIT), from {name} {version}",
                   (root / "LICENSE-LUCIDE").read_text())
        elif name == "tikv-jemalloc-sys":
            append(f"jemalloc bundled by {name} {version} (BSD-2-Clause)\nSource: {source}",
                   (root / "jemalloc/COPYING").read_text())
        elif name == "freetype-sys":
            append(f"FreeType bundled by {name} {version} (FTL)\nSource: {source}\n"
                   "This software is based in part on the work of the FreeType Team\n"
                   "(https://www.freetype.org).",
                   (root / "freetype2/docs/FTL.TXT").read_text())
        elif name == "libdbus-sys":
            append(f"D-Bus bundled by {name} {version}\nSource: {source}\n"
                   "Turntable distributes this library under the Academic Free License 2.1\n"
                   "option, not the alternative GPL. Before installing/using the bundled\n"
                   "library, review and accept the AFL-2.1 terms below. The COPYING file\n"
                   "is retained verbatim and also explains upstream's alternative license.",
                   (root / "vendor/dbus/COPYING").read_text())
            # Keep complete attribution/permission headers, not guessed authors.
            # The library sources (including Unix helpers) are all under dbus/;
            # bus/, tools/ and tests/ standalone programs are not linked.
            headers = set()
            for path in sorted((root / "vendor/dbus/dbus").iterdir()):
                if path.suffix not in (".c", ".h"):
                    continue
                text = path.read_text()
                for match in re.finditer(r"/\*.*?\*/", text, re.DOTALL):
                    if "copyright" in match[0].lower():
                        headers.add(match[0])
                        break
            if not headers:
                raise RuntimeError("D-Bus source attribution headers were not found")
            append("D-Bus library source attribution and permission notices",
                   "\n\n".join(sorted(headers)))
