#!/usr/bin/env python3
"""Validate an existing release tag and expose immutable build inputs to Actions."""

import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tomllib


def git(*args: str) -> str:
    return subprocess.check_output(["git", *args], text=True).strip()


def main() -> None:
    tag = sys.argv[1]
    if not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?", tag):
        sys.exit("Select an existing version tag, e.g. v0.1.0 or v0.2.0-rc.1")
    sha = git("rev-parse", f"refs/tags/{tag}^{{commit}}")
    if sha != git("rev-parse", "HEAD"):
        sys.exit(f"Checkout does not match {tag}")
    workspace = tomllib.loads(Path("Cargo.toml").read_text())["workspace"]["package"]["version"]
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--no-deps", "--format-version=1"], text=True
    ))
    packages = {p["name"]: p["version"] for p in metadata["packages"] if p["id"] in metadata["workspace_members"]}
    versions = {"workspace": workspace, **packages}
    if any(f"v{version}" != tag for version in versions.values()):
        sys.exit(f"Tag {tag} must match every workspace/package version: {versions}")
    values = {"tag": tag, "version": workspace, "sha": sha, "prerelease": str("-" in tag).lower()}
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
        for key, value in values.items():
            print(f"{key}={value}", file=output)
    print(f"Validated {tag} at {sha}: {versions}")


if __name__ == "__main__":
    main()
