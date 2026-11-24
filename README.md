# Luminance-rs

A Rust port of [Luminance](https://github.com/sidevesh/Luminance), the GTK app
for controlling the brightness of built-in and external (DDC/CI) displays. It
adds **software dimming below the hardware minimum** and is built to run as a
**Flatpak/deb** and **EXE**.

## Features

- **Built-in panels** go through `systemd-logind` (`Session.SetBrightness`), so
  no udev rule or `video` group is needed. The hardware never goes below ~1 %,
  because 0 turns the backlight off on many laptops.
- **External monitors** use DDC/CI over `/dev/i2c-*` (brightness and contrast).
  There is a native Rust implementation, so libddcutil is not needed.
- **Few dependencies.** Besides GTK 4 and libadwaita, the only crate is
  `libc` (Linux only). The Wayland dimmer speaks the wire protocol directly
  (`src/dim/wire.rs`) instead of using `wayland-client`, JSON is handled by a
  small built-in module (`src/json.rs`) instead of `serde_json`, and the few
  Win32 functions the Windows build needs are declared in `src/win32.rs`.
- **Extra dimming below the minimum.** The bottom 25 % of every slider, left of
  the mark, dims the picture in software while the backlight stays at its
  minimum:

  | Desktop | Method | Notes |
  |---------|--------|-------|
  | GNOME (Mutter, Wayland and X11) | `org.gnome.Mutter.DisplayConfig.SetCrtcGamma` | Keeps the current Night Light tint and re-applies itself if the settings daemon replaces the ramp |
  | Cinnamon (Muffin, X11 and Wayland) | `org.cinnamon.Muffin.DisplayConfig.SetCrtcGamma` | Same as GNOME |
  | Hyprland, Sway, river, … | `wlr-gamma-control` (gamma ramp) | Also dims the cursor. Falls back to the overlay if another tool (wlsunset, gammastep) holds the ramp |
  | KDE Plasma (KWin) | `wlr-layer-shell` overlay, click-through | |

  The darkest level can be set from 5 % to 50 % (**Menu → Darkest Level**). It
  never goes fully black.
- **Monitors without DDC/CI** (for example when there is no i2c access) still
  get a software-dimming slider.
- **Linked brightness**, an option to hide contrast controls, and an option to
  hide the built-in display while the lid is closed (UPower).
- **CLI** with an interactive terminal mode, xrandr-style options and the
  original's options, plus a **D-Bus interface** compatible with the original
  for hotkeys and OSD helpers.

## Packages

`./build.sh` builds every release package into `dist/`:

```sh
./build.sh            # all of them
./build.sh exe deb    # or just some: exe, deb, flatpak
```

| Package | File | Runs on |
|---------|------|---------|
| Windows installer | `dist/windows/Luminance-<version>-setup.exe` | Windows 10/11, x64 |
| Windows portable | `dist/windows/Luminance-<version>-windows-x86_64.zip` | Windows 10/11, x64 |
| Debian package | `dist/luminance-rs_<version>_amd64.deb` | Debian 13+, Ubuntu 25.04+ |
| Flatpak | `dist/luminance-rs-<version>-x86_64.flatpak` | any distribution with Flatpak |

The .exe and the .deb are built in Docker containers, so the host only needs
Docker. The Windows build cross-compiles with llvm-mingw against GTK 4 and
libadwaita from MSYS2, and the installer is made with Inno Setup under wine.
`dist/windows/` also contains the application folder and `luminance.iss`, so
the installer can be rebuilt on Windows with the Inno Setup Compiler. The
Flatpak needs `org.flatpak.Builder` (see below).

Versions follow [Semantic Versioning](https://semver.org/). The version in
`Cargo.toml` is the only place to change it; `build.sh` rejects anything that
is not a semantic version. Debian packages turn a pre-release such as
`2.2.0-rc.1` into `2.2.0~rc.1` so that it sorts before `2.2.0`.

### Windows

- **Built-in displays** use the display driver's brightness interface
  (`\\.\LCD`), or WMI when the driver has none.
- **External monitors** use DDC/CI through the Windows Monitor Configuration
  API. No drivers or administrator rights are needed.
- **Extra dimming** uses the gamma ramp. Windows refuses ramps far from
  neutral, so for deeper dimming Luminance switches to a click-through overlay
  window. Setting the DWORD `GdiIcmGammaRange` to `256` under
  `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\ICM` lifts that limit.
- Displays are named `DISPLAY1`, `DISPLAY2`, … as in Windows' display settings.
- `luminance-rs-cli.exe` is the command line tool, with the same options as on
  Linux. There is no D-Bus on Windows, so it controls the hardware directly:
  values are hardware percentages, and software dimming is only available in
  the app. The installer can add it to `PATH`.
- Closing the window restores normal brightness. The lid option is not
  available.

### Debian package

Besides the program, the .deb installs a udev rule that gives the logged-in
user access to `/dev/i2c-*` and loads the `i2c-dev` module at boot, so external
monitors work right after installation without the steps under
[Permissions](#permissions).

## Build and install the Flatpak

Requirements (one time):

```sh
flatpak install flathub org.gnome.Sdk//49 org.gnome.Platform//49 \
    org.freedesktop.Sdk.Extension.rust-stable//25.08 org.flatpak.Builder
```

Then:

```sh
make install    # vendors crates (cargo vendor) and builds + installs the Flatpak
make bundle     # optional: single-file luminance-rs.flatpak to copy elsewhere
flatpak run io.github.luminance_rs.Luminance
```

The Flatpak build itself is offline. `make vendor` puts all crates into
`vendor/`, and `.cargo/config.toml` points cargo at them. You need the system
`cargo` only for that vendoring step.

## Permissions

All of these are in the manifest by default and can be changed in **Flatseal**:

| Permission | Used for |
|------------|----------|
| `--share=ipc`, `--socket=fallback-x11` | The window in X11 sessions |
| `--socket=wayland` | Software dimming on Hyprland/Sway/KDE (protocol objects on its own Wayland connection) |
| `--talk-name=org.gnome.Mutter.DisplayConfig` | Software dimming on GNOME |
| `--talk-name=org.cinnamon.Muffin.DisplayConfig` | Software dimming on Cinnamon |
| `--system-talk-name=org.freedesktop.login1` | Built-in backlight |
| `--system-talk-name=org.freedesktop.UPower` | Lid state |
| `--device=all` | `/dev/i2c-*` for DDC/CI monitors |

Built-in displays and software dimming need **no host changes**. External
monitors over DDC/CI are the one exception, because Flatpak cannot load kernel
modules or change device permissions on the host. They need the `i2c-dev`
module and access to `/dev/i2c-*`. Many distributions already provide this
through the `ddcutil` package's udev rule. Otherwise:

```sh
sudo modprobe i2c-dev
echo i2c-dev | sudo tee /etc/modules-load.d/i2c-dev.conf
sudo usermod -aG i2c $USER   # or install ddcutil for its uaccess udev rule
```

The app finds this problem by itself, shows a banner and gives the exact
commands under **Menu → Permissions Help**. Until access is fixed, those
monitors still get software dimming.

## Command line

`luminance-rs --help` lists every option. Inside Flatpak, prefix commands with
`flatpak run io.github.luminance_rs.Luminance` (shortened to `luminance` below).

```sh
luminance --cli                                  # interactive: pick a display, adjust it
luminance --list-displays --verbose              # also: -q, --listmonitors, --json
luminance --output DP-2 --brightness 30          # set; also +10 / -10
luminance --noninteractive --output eDP-1 --brightness -10 --contrast 70
luminance --brightness 10 --dryrun               # show what would change
luminance --set-brightness 1 -p 60               # original Luminance options
luminance --decrease-brightness -p 5             # all displays
```

- **Modes:** with no options the GUI starts. `--cli` runs a line-based
  terminal interface (works over ssh and with piped input). `--noninteractive`
  never prompts or opens the GUI, and fails when it is given nothing to do.
- **Displays** (`--output`, repeatable) can be a number from the list, a
  connector (`eDP-1`, `DP-2`) or the monitor name, case-insensitive. Without
  `--output` all displays change.
- **Values** are 0-100, with an optional `%`. `+N`/`-N` change the current
  value. Values outside 0-100, `nan`, unknown options, repeated options and
  conflicting combinations are rejected with exit status 2.
- **xrandr-style extras:** `--verbose`, `--dryrun`, `--current` (use the
  service's cached values instead of re-reading the hardware), `-v/--version`.
- The original options still work: `-l -g -s -i -d -p` and
  `--get/set/increase/decrease-contrast`.

Brightness values use the slider scale, so values below 25 dim in software.
Commands run in the background service, which is started through D-Bus
activation and stays alive while software dimming is active. If the service
can't be reached, the CLI controls the hardware directly (hardware percentages,
no software dimming). **Quit** (Ctrl+Q) restores normal brightness.

Example hotkeys:

```ini
# Hyprland
bind = , XF86MonBrightnessDown, exec, flatpak run io.github.luminance_rs.Luminance -d -p 5
bind = , XF86MonBrightnessUp,   exec, flatpak run io.github.luminance_rs.Luminance -i -p 5
```

## D-Bus

Bus name and interface `io.github.luminance_rs.Luminance`, object
`/io/github/luminance_rs/Luminance/Service`:
`GetMonitors() → s (JSON)`, `SetBrightness(s id, d value)`,
`SetBrightnessQuiet(s, d)`, `SetContrast(s, d)`, `Refresh()`, `Quit()`, and
the signals `ShowOSD(d, s)` and `ShowContrastOSD(d, s)`.

`GetMonitors` returns an array of objects with the keys `id`, `name`,
`brightness` (slider scale), `has_contrast`, `contrast` (`-1` when
unsupported), `connector` (`null` when unknown), `kind` (`backlight`, `ddc` or
`software`) and `software_dim` (1.0 = not dimmed).

The `Set*` methods take the `id` from `GetMonitors` (a string such as `"0"`)
and a value in percent: brightness on the slider scale (as `brightness`),
contrast in percent of the monitor's range. Values outside 0-100 are clamped; an unknown id, NaN or infinity is rejected with
`org.freedesktop.DBus.Error.InvalidArgs`. `SetBrightness` and `SetContrast`
also emit the OSD signal, `SetBrightnessQuiet` does not.

For example:

```sh
gdbus call --session --dest io.github.luminance_rs.Luminance \
    --object-path /io/github/luminance_rs/Luminance/Service \
    --method io.github.luminance_rs.Luminance.SetBrightness "'0'" 40
```

## Development

```sh
cargo run       # needs GTK 4.14+ and libadwaita 1.6+ development files
cargo test      # or: make test
cargo clippy --all-targets -- -D warnings
```

Building needs `pkg-config` plus the GTK 4 and libadwaita development
packages (on Debian/Ubuntu: `libgtk-4-dev libadwaita-1-dev`). No Wayland
development files are needed. Cargo uses the crates in `vendor/` (see
`.cargo/config.toml`); run `make vendor` (or any `./build.sh` target) after
changing dependencies.

Without those packages on the host, use the Debian image that `./build.sh deb`
builds (`luminance-rs-build-deb`); the repository is mounted read-only and the
build goes to a Docker volume:

```sh
docker run --rm -v "$PWD:/src:ro" -v luminance-rs-target-dev:/target \
    -e CARGO_TARGET_DIR=/target -w /src luminance-rs-build-deb \
    cargo test --offline --locked
```

That image has no clippy; add it in the container with
`rustup component add clippy` before running `cargo clippy`. The Windows code
(`#[cfg(windows)]`) can be type-checked the same way with
`luminance-rs-build-windows` and
`cargo build --offline --locked --target x86_64-pc-windows-gnullvm`.

The unit tests cover option parsing, JSON, the Wayland wire format, EDID and
the brightness/dimming arithmetic. Anything that talks to hardware, the
compositor or D-Bus is not covered and has to be tried on a real session.

Settings are stored in `~/.config/luminance-rs/settings.ini`. Inside Flatpak
that is `~/.var/app/io.github.luminance_rs.Luminance/config/luminance-rs/`, and
on Windows `%LOCALAPPDATA%\luminance-rs\settings.ini`. Invalid values there
fall back to the defaults, and `dim-floor` is limited to 0.05-0.50.

## License

GPL-3.0-only, like the original. Icons are from the original Luminance
project.
