#!/usr/bin/env python3
"""Collect the licenses of every crate built into the doctopus binary.

Usage: third-party-licenses.py [--target TRIPLE] > THIRD-PARTY-LICENSES.txt

Release archives ship this file next to the binary, since the licenses of
the bundled crates ask for their notices to accompany it. Only normal
dependencies of the `doctopus` package for the given target are listed
(not build or dev dependencies). Crates are fetched by Cargo first, so run
it after a build.
"""

import argparse
import json
import os
import subprocess
import sys

ROOT_PACKAGE = "doctopus"
LICENSE_PREFIXES = ("license", "licence", "copying", "notice", "unlicense", "copyright")


def metadata(target):
    command = ["cargo", "metadata", "--format-version", "1", "--locked"]
    if target:
        command += ["--filter-platform", target]
    return json.loads(subprocess.check_output(command))


def bundled(meta):
    """Packages reachable from the root package through normal dependencies."""
    packages = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    root = next(p["id"] for p in meta["packages"] if p["name"] == ROOT_PACKAGE)
    seen, stack = set(), [root]
    while stack:
        current = stack.pop()
        if current in seen:
            continue
        seen.add(current)
        for dep in nodes[current]["deps"]:
            if any(kind["kind"] is None for kind in dep["dep_kinds"]):
                stack.append(dep["pkg"])
    third_party = [packages[i] for i in seen if packages[i]["source"] is not None]
    return sorted(third_party, key=lambda p: (p["name"], p["version"]))


def license_files(package):
    directory = os.path.dirname(package["manifest_path"])
    names = sorted(
        name
        for name in os.listdir(directory)
        if name.lower().startswith(LICENSE_PREFIXES)
        and os.path.isfile(os.path.join(directory, name))
    )
    if package.get("license_file"):
        extra = os.path.basename(package["license_file"])
        if extra not in names and os.path.isfile(os.path.join(directory, extra)):
            names.append(extra)
    return [(name, os.path.join(directory, name)) for name in names]


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--target", help="target triple, e.g. x86_64-unknown-linux-musl")
    args = parser.parse_args()
    packages = bundled(metadata(args.target))

    # The same license text is shipped by many crates: print each once.
    texts = {}  # text -> notice number
    used_by = {}  # notice number -> crates
    crate_notices = []
    for p in packages:
        numbers = []
        for _, path in license_files(p):
            with open(path, encoding="utf-8", errors="replace") as f:
                text = f.read().strip()
            number = texts.setdefault(text, len(texts) + 1)
            used_by.setdefault(number, []).append(f"{p['name']} {p['version']}")
            numbers.append(number)
        crate_notices.append((p, numbers))

    out = sys.stdout
    out.write("Third-party software in doctopus\n")
    out.write("================================\n\n")
    out.write(
        "The doctopus binary includes the crates below. The numbers refer to\n"
        "the license texts that follow the list.\n\n"
    )
    for p, numbers in crate_notices:
        refs = " ".join(f"[{n}]" for n in numbers) or "(no license file; see its source)"
        out.write(f"- {p['name']} {p['version']}: {p.get('license') or 'see source'} {refs}\n")
    for text, number in texts.items():
        out.write("\n" + "=" * 72 + "\n")
        out.write(f"[{number}] used by: {', '.join(used_by[number])}\n\n")
        out.write(text + "\n")


if __name__ == "__main__":
    main()
