#!/usr/bin/env bash
# Builds the release packages into dist/:
#
#   dist/windows/Luminance-<version>-setup.exe        Windows installer (Inno Setup)
#   dist/windows/Luminance-<version>-windows-x86_64.zip  portable Windows build
#   dist/windows/Luminance/ + luminance.iss           to rebuild the installer on Windows
#   dist/luminance-rs_<version>_amd64.deb             Debian 13+ / Ubuntu 25.04+
#   dist/luminance-rs-<version>-x86_64.flatpak        Flatpak bundle
#
# Usage: ./build.sh [all|exe|deb|flatpak]...      (default: all)
#
# Needs Docker for the .exe and the .deb (images are built on first use and
# cached), and flatpak with org.flatpak.Builder (or flatpak-builder) plus the
# GNOME 49 SDK for the Flatpak. Nothing else has to be installed on the host.
set -euo pipefail

cd "$(dirname "$(readlink -f "$0")")"
ROOT=$PWD
APP_ID=io.github.luminance_rs.Luminance
# The version follows Semantic Versioning (https://semver.org/); Cargo.toml
# is the single source. The packaging scripts derive what each format needs
# from it (Debian: "~" before a pre-release, Windows: numeric x.y.z.0).
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
SEMVER='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-((0|[1-9][0-9]*|[0-9]*[a-zA-Z-][0-9a-zA-Z-]*)(\.(0|[1-9][0-9]*|[0-9]*[a-zA-Z-][0-9a-zA-Z-]*))*))?(\+([0-9a-zA-Z-]+(\.[0-9a-zA-Z-]+)*))?$'
[[ $VERSION =~ $SEMVER ]] || { echo "build.sh: version '$VERSION' in Cargo.toml is not a semantic version (https://semver.org/)" >&2; exit 1; }
DIST=$ROOT/dist
DOCKER=${DOCKER:-docker}

log() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
die() { printf 'build.sh: %s\n' "$*" >&2; exit 1; }

# Some build hosts have no (writable) home directory.
if [ -z "${HOME:-}" ] || [ ! -w "${HOME}" ]; then
    export HOME=$ROOT/target/home
    mkdir -p "$HOME"
fi

need_docker() {
    command -v "$DOCKER" >/dev/null || die "$DOCKER is needed for this target"
    "$DOCKER" info >/dev/null 2>&1 || die "cannot talk to the Docker daemon (is your user in the docker group?)"
}

image() { # name dockerfile context
    log "Docker image $1"
    "$DOCKER" build -t "$1" -f "$2" "$3"
}

# Runs a script in a container: the repository at /src (read-only unless
# SRC_MODE=rw), $2 at /out, and a cached cargo target directory at /target.
# Output files are handed back to the calling user.
in_docker() { # image out-dir command...
    local img=$1 out=$2
    shift 2
    mkdir -p "$out"
    "$DOCKER" run --rm \
        -v "$ROOT:/src:${SRC_MODE:-ro}" \
        -v "$out:/out" \
        -v "luminance-rs-target-$img:/target" \
        -e CARGO_HOME=/target/cargo-home \
        -e VERSION="$VERSION" \
        "$img" sh -c "$* && { chown -R $(id -u):$(id -g) /out 2>/dev/null || true; }"
}

# All crates come from vendor/ (see .cargo/config.toml), so the builds
# themselves need no network. Re-vendor when Cargo.lock changed.
vendor() {
    if [ -f vendor/.stamp ] && [ vendor/.stamp -nt Cargo.lock ]; then
        return
    fi
    log "Vendoring crates"
    if command -v cargo >/dev/null; then
        cargo vendor --locked --versioned-dirs vendor > .cargo/config.toml
    else
        need_docker
        image luminance-rs-build-deb packaging/deb/Dockerfile packaging/deb
        SRC_MODE=rw in_docker luminance-rs-build-deb "$ROOT/target/vendor-log" \
            "cd /src && cargo vendor --locked --versioned-dirs vendor > .cargo/config.toml && { chown -R $(id -u):$(id -g) vendor .cargo 2>/dev/null || true; }"
    fi
    touch vendor/.stamp
}

build_exe() {
    need_docker
    image luminance-rs-build-windows packaging/windows/Dockerfile packaging/windows
    log "Windows build"
    in_docker luminance-rs-build-windows "$DIST/windows" sh /src/packaging/windows/build-windows.sh

    image luminance-rs-iscc packaging/windows/iscc.Dockerfile packaging/windows
    log "Windows installer (Inno Setup)"
    "$DOCKER" run --rm -e XDG_RUNTIME_DIR=/tmp -v "$DIST/windows:/out" luminance-rs-iscc sh -c \
        "cd /out && wine 'C:\\InnoSetup\\ISCC.exe' /Q luminance.iss \
         && { chown $(id -u):$(id -g) Luminance-$VERSION-setup.exe 2>/dev/null || true; }"
    echo "dist/windows/Luminance-$VERSION-setup.exe"
}

build_deb() {
    need_docker
    image luminance-rs-build-deb packaging/deb/Dockerfile packaging/deb
    log "Debian package"
    in_docker luminance-rs-build-deb "$DIST" sh /src/packaging/deb/build-deb.sh
}

build_flatpak() {
    local builder
    if flatpak info org.flatpak.Builder >/dev/null 2>&1; then
        builder=(flatpak run org.flatpak.Builder)
    elif command -v flatpak-builder >/dev/null; then
        builder=(flatpak-builder)
    else
        die "the Flatpak needs org.flatpak.Builder (flatpak install flathub org.flatpak.Builder) or flatpak-builder"
    fi
    # flatpak-builder's OSTree cache needs a local file system (hard links,
    # xattrs), so it is kept outside the source tree, which may be a network
    # mount. FLATPAK_WORK overrides the location.
    local work=${FLATPAK_WORK:-${XDG_CACHE_HOME:-$HOME/.cache}/luminance-rs/flatpak}
    mkdir -p "$work"
    log "Flatpak (work directory $work)"
    "${builder[@]}" --user --install-deps-from=flathub --force-clean \
        --state-dir="$work/state" --repo="$work/repo" \
        "$work/build-dir" flatpak/$APP_ID.yml
    mkdir -p "$DIST"
    flatpak build-bundle "$work/repo" "$DIST/luminance-rs-$VERSION-x86_64.flatpak" $APP_ID
    echo "dist/luminance-rs-$VERSION-x86_64.flatpak"
}

targets=("$@")
[ ${#targets[@]} -eq 0 ] && targets=(all)
for t in "${targets[@]}"; do
    case $t in
        all | exe | windows | deb | flatpak) ;;
        -h | --help) sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) die "unknown target '$t' (expected all, exe, deb or flatpak)" ;;
    esac
done

vendor
for t in "${targets[@]}"; do
    case $t in
        all) build_exe; build_deb; build_flatpak ;;
        exe | windows) build_exe ;;
        deb) build_deb ;;
        flatpak) build_flatpak ;;
    esac
done
log "Done: $(cd "$DIST" && find . -maxdepth 2 -type f \( -name '*.exe' -o -name '*.zip' -o -name '*.deb' -o -name '*.flatpak' \) | sed 's|^\./|dist/|' | tr '\n' ' ')"
