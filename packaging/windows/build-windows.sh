#!/bin/sh
# Runs inside the luminance-rs-build-windows container (see ../../build.sh):
#   /src     the repository (read-only, crates vendored in /src/vendor)
#   /target  cargo target directory (cached volume)
#   /out     output: Luminance/ (the application folder), luminance.iss
#            (Inno Setup script for it) and the portable .zip
set -eu

APP_ID=io.github.luminance_rs.Luminance
TARGET=x86_64-pc-windows-gnullvm
VERSION=${VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' /src/Cargo.toml | head -n1)}
# Windows version fields are numeric: MAJOR.MINOR.PATCH(.0) of the semver.
NUMERIC=$(printf '%s' "$VERSION" | sed 's/[-+].*//')
HERE=/src/packaging/windows
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# --- Icon and version resource ------------------------------------------------
mkdir -p "$WORK/res"
pngs=""
for size in 16 20 24 32 40 48 64 128 256; do
    rsvg-convert -w $size -h $size "/src/data/icons/$APP_ID.svg" -o "$WORK/res/$size.png"
    pngs="$pngs $WORK/res/$size.png"
done
# shellcheck disable=SC2086
python3 "$HERE/make-ico.py" "$WORK/res/luminance.ico" $pngs
IFS=. read -r V1 V2 V3 <<EOF
$NUMERIC
EOF
sed -e "s/@VERSION@/$VERSION/g" -e "s/@V1@/$V1/g" -e "s/@V2@/$V2/g" -e "s/@V3@/$V3/g" \
    "$HERE/luminance.rc" > "$WORK/res/luminance.rc"
(cd "$WORK/res" && x86_64-w64-mingw32-windres luminance.rc -O coff -o luminance-res.o)
# Fixed path, so cargo's cache is not invalidated by a changing RUSTFLAGS.
mkdir -p /target/windows-res
cmp -s "$WORK/res/luminance-res.o" /target/windows-res/luminance-res.o \
    || cp "$WORK/res/luminance-res.o" /target/windows-res/luminance-res.o

# --- Build --------------------------------------------------------------------
cd /src
RUSTFLAGS="-C link-arg=/target/windows-res/luminance-res.o" \
    cargo build --release --locked --offline --target $TARGET --target-dir /target

# --- Application folder (same layout as an MSYS2 prefix, so GLib, GTK and
# gdk-pixbuf find their data relative to bin/) --------------------------------
APP="$WORK/Luminance"
mkdir -p "$APP/bin" "$APP/share" "$APP/lib" "$APP/etc"
cp "/target/$TARGET/release/luminance-rs.exe" "$APP/bin/"
# Same program as a console executable, for terminals and scripts.
cp "$APP/bin/luminance-rs.exe" "$APP/bin/luminance-rs-cli.exe"
python3 "$HERE/set-subsystem.py" "$APP/bin/luminance-rs-cli.exe" 3

# gdk-pixbuf SVG loader (icons), with a loaders.cache made from the Linux
# build of the same loader: only the module path differs.
LOADERS=lib/gdk-pixbuf-2.0/2.10.0/loaders
SVG_DLL=$(cd "$MSYS2_ROOT/$LOADERS" && ls ./*svg*.dll | head -n1 | sed 's|^\./||')
mkdir -p "$APP/$LOADERS"
cp "$MSYS2_ROOT/$LOADERS/$SVG_DLL" "$APP/$LOADERS/"
QUERY=$(find /usr/lib -name 'gdk-pixbuf-query-loaders*' -type f | head -n1)
LINUX_SVG=$(find /usr/lib -path '*gdk-pixbuf-2.0*' -name '*svg*.so' | head -n1)
# The path stays relative (gdk-pixbuf prefixes it with the install folder)
# and uses forward slashes: gdk-pixbuf runs it through g_strcompress(), which
# would turn single backslashes into nothing ("lib\gdk..." -> "libgdk...").
"$QUERY" "$LINUX_SVG" | sed -e '/^#/d' -e "s|^\"/.*\.so\"|\"$LOADERS/$SVG_DLL\"|" > "$APP/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache"
grep -qx "\"$LOADERS/$SVG_DLL\"" "$APP/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache" \
    || { echo "build-windows.sh: SVG loader path missing from loaders.cache" >&2; exit 1; }

# DLLs: everything the executable and the loader import, recursively, from
# the MSYS2 sysroot and the toolchain. DLLs found in neither are Windows' own.
python3 - "$APP/bin" "$APP/bin/luminance-rs.exe" "$APP/$LOADERS/$SVG_DLL" <<'PY'
import os, shutil, subprocess, sys
dest, todo = sys.argv[1], sys.argv[2:]
search = [os.environ["MSYS2_ROOT"] + "/bin", "/opt/llvm-mingw/x86_64-w64-mingw32/bin"]
index = {}
for d in search:
    for f in os.listdir(d):
        if f.lower().endswith(".dll"):
            index.setdefault(f.lower(), os.path.join(d, f))
done = set()
while todo:
    path = todo.pop()
    out = subprocess.run(["llvm-objdump", "-p", path], capture_output=True, text=True, check=True).stdout
    for line in out.splitlines():
        line = line.strip()
        if not line.startswith("DLL Name:"):
            continue
        name = line.split(":", 1)[1].strip().lower()
        if name in done or name not in index:
            continue
        done.add(name)
        target = os.path.join(dest, os.path.basename(index[name]))
        shutil.copy2(index[name], target)
        todo.append(target)
print(f"  bundled {len(done)} DLLs")
PY

# GSettings schemas (GTK's file chooser and color chooser settings).
mkdir -p "$APP/share/glib-2.0/schemas"
glib-compile-schemas --targetdir="$APP/share/glib-2.0/schemas" "$MSYS2_ROOT/share/glib-2.0/schemas"

# Icons: Adwaita (without X11 cursors), hicolor and the application icon.
mkdir -p "$APP/share/icons"
cp -r "$MSYS2_ROOT/share/icons/Adwaita" "$APP/share/icons/"
rm -rf "$APP/share/icons/Adwaita/cursors"
mkdir -p "$APP/share/icons/hicolor/scalable/apps" "$APP/share/icons/hicolor/symbolic/apps"
cp "$MSYS2_ROOT/share/icons/hicolor/index.theme" "$APP/share/icons/hicolor/"
cp "/src/data/icons/$APP_ID.svg" "$APP/share/icons/hicolor/scalable/apps/"
cp "/src/data/icons/$APP_ID-symbolic.svg" "$APP/share/icons/hicolor/symbolic/apps/"

# Fontconfig configuration, used by Pango's FreeType backend.
[ -d "$MSYS2_ROOT/etc/fonts" ] && cp -r "$MSYS2_ROOT/etc/fonts" "$APP/etc/"

cp /src/LICENSE "$APP/LICENSE.txt"
cp "$WORK/res/luminance.ico" "$APP/"
cp "$MSYS2_ROOT/../packages.txt" "$APP/share/msys2-packages.txt"

# --- Output -------------------------------------------------------------------
rm -rf /out/Luminance
cp -r "$APP" /out/Luminance
sed -e "s/^  #define AppVersion \".*\"/  #define AppVersion \"$VERSION\"/" \
    -e "s/^  #define AppNumericVersion \".*\"/  #define AppNumericVersion \"$NUMERIC.0\"/" \
    "$HERE/luminance.iss" > /out/luminance.iss
ZIP="Luminance-$VERSION-windows-x86_64.zip"
rm -f "/out/$ZIP"
(cd "$WORK" && zip -qr9 "/out/$ZIP" Luminance)
echo "Windows application folder: Luminance/, portable archive: $ZIP"
