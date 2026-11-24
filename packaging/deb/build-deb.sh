#!/bin/sh
# Runs inside the luminance-rs-build-deb container (see ../../build.sh):
#   /src     the repository (read-only, crates vendored in /src/vendor)
#   /target  cargo target directory (cached volume)
#   /out     where the .deb is written
set -eu

VERSION=${VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' /src/Cargo.toml | head -n1)}
# Semver to Debian: a pre-release must sort before its release, which "~"
# does ("2.1.0-rc.1" -> "2.1.0~rc.1"); build metadata ("+...") is kept.
DEB_VERSION=$(printf '%s' "$VERSION" | sed 's/-/~/')
APP_ID=io.github.luminance_rs.Luminance
ARCH=$(dpkg --print-architecture)

cd /src
cargo build --release --locked --offline --target-dir /target

STAGE=/tmp/luminance-rs_${DEB_VERSION}_${ARCH}
rm -rf "$STAGE"
install -Dm755 /target/release/luminance-rs -t "$STAGE/usr/bin"
install -Dm644 data/$APP_ID.desktop -t "$STAGE/usr/share/applications"
install -Dm644 data/$APP_ID.metainfo.xml -t "$STAGE/usr/share/metainfo"
install -Dm644 data/icons/$APP_ID.svg -t "$STAGE/usr/share/icons/hicolor/scalable/apps"
install -Dm644 data/icons/$APP_ID-symbolic.svg -t "$STAGE/usr/share/icons/hicolor/symbolic/apps"
install -d "$STAGE/usr/share/dbus-1/services"
sed 's|^Exec=.*|Exec=/usr/bin/luminance-rs --gapplication-service|' data/$APP_ID.service \
    > "$STAGE/usr/share/dbus-1/services/$APP_ID.service"
install -Dm644 packaging/deb/i2c-dev.rules "$STAGE/usr/lib/udev/rules.d/60-luminance-rs-i2c.rules"
install -d "$STAGE/usr/lib/modules-load.d"
echo i2c-dev > "$STAGE/usr/lib/modules-load.d/luminance-rs.conf"
install -Dm644 LICENSE "$STAGE/usr/share/doc/luminance-rs/copyright"
install -Dm755 packaging/deb/postinst "$STAGE/DEBIAN/postinst"

# Library dependencies as dpkg-shlibdeps finds them in the build system.
WORK=$(mktemp -d)
mkdir -p "$WORK/debian"
printf 'Source: luminance-rs\n\nPackage: luminance-rs\nArchitecture: any\n' > "$WORK/debian/control"
DEPENDS=$(cd "$WORK" && dpkg-shlibdeps -O "$STAGE/usr/bin/luminance-rs" | sed -n 's/^shlibs:Depends=//p')
rm -rf "$WORK"

SIZE=$(du -sk --exclude=DEBIAN "$STAGE" | cut -f1)
cat > "$STAGE/DEBIAN/control" <<CONTROL
Package: luminance-rs
Version: $DEB_VERSION
Architecture: $ARCH
Maintainer: Luminance-rs contributors <noreply@luminance-rs.invalid>
Installed-Size: $SIZE
Depends: $DEPENDS
Recommends: dbus-user-session | dbus-x11
Section: utils
Priority: optional
Homepage: https://github.com/sidevesh/Luminance
Description: Control brightness of built-in and external displays
 A Rust port of Luminance: control the brightness of built-in panels
 (systemd-logind) and external monitors (DDC/CI), with optional software
 dimming below the hardware minimum on GNOME, Cinnamon, KDE Plasma,
 Hyprland, Sway and other wlroots compositors. Includes a command line
 interface and a D-Bus service for hotkeys.
CONTROL

dpkg-deb --root-owner-group --build "$STAGE" "/out/luminance-rs_${DEB_VERSION}_${ARCH}.deb"
