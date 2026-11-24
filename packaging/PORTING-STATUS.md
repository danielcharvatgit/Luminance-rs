# Status: Windows port, .deb, build.sh (2026-11-24)

`./build.sh [all|exe|deb|flatpak]` builds everything into `dist/` (see
README → Packages). Version 3.0.0 (semver; Cargo.toml is the single source).

## Verified

- Linux: `cargo build`, `cargo clippy --all-targets -- -D warnings` and
  `cargo test` (23 tests) pass in the Debian 13 container (clippy added with
  `rustup component add clippy`), with no warnings (2026-11-24).
- Windows: cross-compiles cleanly (`x86_64-pc-windows-gnullvm`, llvm-mingw
  20261020, MSYS2 GTK 4.24.1 / libadwaita 1.10.0). All 63 bundled DLLs resolve.
  Under wine: `luminance-rs-cli.exe --version` and `--list-displays` work (Win32
  monitor enumeration and CCD names). The Inno Setup installer (7.1.0, under
  wine) installs silently.
- .deb: installs on a clean Debian 13 container; `--list-displays` read the
  laptop backlight.

## Not verified

- **The Windows GUI.** Under wine with Xvfb, the app gets through its whole
  startup up to `build_window(...).present()` (checked with temporary
  traces), but wine never maps the GTK 4.24 toplevel window, even with
  `GDK_DISABLE=dcomp,d3d11,d3d12,gl,vulkan GSK_RENDERER=cairo`. This looks
  like a wine limitation, but it needs one test on real Windows.
- Real hardware on Windows: `\\.\LCD` IOCTL / WMI backlight, DDC/CI through
  dxva2, gamma ramp vs. overlay dimming.

## Remote build machine

The builds are meant to run on ssh1@192.168.1.137. That user has no home
directory, so key login can't be set up. Instead, open a shared connection
with the command in `commands.txt`; build.sh then works there too, because it
falls back to `target/home` when `$HOME` is missing.
