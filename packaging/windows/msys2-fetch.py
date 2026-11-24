#!/usr/bin/env python3
"""Downloads prebuilt MSYS2 CLANG64 packages (GTK 4, libadwaita and their
dependencies) and unpacks them into a sysroot for cross compiling.

    msys2-fetch.py DEST PACKAGE...

Package names are given without the "mingw-w64-clang-x86_64-" prefix.
Dependencies are resolved from the repository database, so only the roots
need to be listed. Needs `tar` with zstd support.
"""

import io
import os
import re
import subprocess
import sys
import tarfile
import urllib.request

MIRROR = os.environ.get("MSYS2_MIRROR", "https://mirror.msys2.org/mingw/clang64")
PREFIX = "mingw-w64-clang-x86_64-"
# Never needed at runtime or for linking; they would pull in large trees.
SKIP = re.compile(r"^%s(python|perl|tcl|tk|gcc|clang|llvm|lld|cmake|ninja|meson|gettext-tools)(\b|-)" % PREFIX)


def fetch(url):
    with urllib.request.urlopen(url, timeout=120) as r:
        return r.read()


def zstd_decompress(data):
    return subprocess.run(["zstd", "-dc"], input=data, capture_output=True, check=True).stdout


def load_db():
    db = tarfile.open(fileobj=io.BytesIO(zstd_decompress(fetch(MIRROR + "/clang64.db"))))
    packages, provides = {}, {}
    for member in db.getmembers():
        if not member.name.endswith("/desc"):
            continue
        fields, key = {}, None
        for line in db.extractfile(member).read().decode().splitlines():
            if line.startswith("%") and line.endswith("%"):
                key = line.strip("%")
                fields[key] = []
            elif line and key:
                fields[key].append(line)
        name = fields["NAME"][0]
        packages[name] = fields
        for p in fields.get("PROVIDES", []):
            provides.setdefault(re.split(r"[<>=]", p)[0], name)
    return packages, provides


def resolve(packages, provides, roots):
    order, seen = [], set()
    stack = [PREFIX + r for r in roots]
    while stack:
        dep = re.split(r"[<>=]", stack.pop())[0]
        name = dep if dep in packages else provides.get(dep)
        if name is None:
            sys.exit(f"msys2-fetch: unknown package {dep}")
        if name in seen or SKIP.match(name):
            continue
        seen.add(name)
        order.append(name)
        stack.extend(packages[name].get("DEPENDS", []))
    return sorted(order)


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    dest, roots = sys.argv[1], sys.argv[2:]
    os.makedirs(dest, exist_ok=True)
    packages, provides = load_db()
    names = resolve(packages, provides, roots)
    with open(os.path.join(dest, "packages.txt"), "w") as manifest:
        for name in names:
            filename = packages[name]["FILENAME"][0]
            version = packages[name]["VERSION"][0]
            print(f"  {name} {version}", flush=True)
            manifest.write(f"{name} {version}\n")
            data = zstd_decompress(fetch(f"{MIRROR}/{filename}"))
            with tarfile.open(fileobj=io.BytesIO(data)) as pkg:
                members = [m for m in pkg.getmembers() if not m.name.startswith(".")]
                pkg.extractall(dest, members=members, filter="fully_trusted")


if __name__ == "__main__":
    main()
