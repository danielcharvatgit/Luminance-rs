//! Linux backend: built-in backlights (sysfs + systemd-logind) and external
//! monitors (DDC/CI over i2c-dev).

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use super::{backlight_percent, backlight_raw, ddc, ddc_percent, ddc_raw, edid, Control, Display, Issue, Scan};

const DRM_DIR: &str = "/sys/class/drm";
const BACKLIGHT_DIR: &str = "/sys/class/backlight";

struct Connector {
    name: String,
    internal: bool,
    edid_name: Option<String>,
    ddc_bus: Option<u32>,
}

fn read_trimmed(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn read_u32(path: impl AsRef<Path>) -> Option<u32> {
    read_trimmed(path)?.parse().ok()
}

fn i2c_bus_from_name(name: &str) -> Option<u32> {
    name.strip_prefix("i2c-")?.parse().ok()
}

fn is_internal_connector(name: &str) -> bool {
    ["eDP", "LVDS", "DSI"].iter().any(|p| name.starts_with(p))
}

/// "card1-HDMI-A-1" -> "HDMI-A-1"
fn connector_name(dir_name: &str) -> Option<&str> {
    let rest = dir_name.strip_prefix("card")?;
    let (card, connector) = rest.split_once('-')?;
    card.chars().all(|c| c.is_ascii_digit()).then_some(connector)
}

fn connected_connectors() -> Vec<Connector> {
    let Ok(entries) = fs::read_dir(DRM_DIR) else { return Vec::new() };
    let mut connectors: Vec<Connector> = entries
        .flatten()
        .filter_map(|entry| {
            let dir_name = entry.file_name().to_string_lossy().into_owned();
            let name = connector_name(&dir_name)?.to_string();
            let path = entry.path();
            if read_trimmed(path.join("status")).as_deref() != Some("connected") {
                return None;
            }
            let edid_name = fs::read(path.join("edid")).ok().and_then(|e| edid::monitor_name(&e));
            let ddc_bus = fs::read_link(path.join("ddc"))
                .ok()
                .and_then(|l| i2c_bus_from_name(&l.file_name()?.to_string_lossy()))
                .or_else(|| {
                    // DP AUX channels show up as an i2c-N child directory.
                    fs::read_dir(&path).ok()?.flatten().find_map(|e| {
                        i2c_bus_from_name(&e.file_name().to_string_lossy())
                    })
                });
            Some(Connector { internal: is_internal_connector(&name), name, edid_name, ddc_bus })
        })
        .collect();
    connectors.sort_by(|a, b| b.internal.cmp(&a.internal).then_with(|| a.name.cmp(&b.name)));
    connectors.dedup_by(|a, b| a.name == b.name);
    connectors
}

struct Backlight {
    device: String,
    connector: Option<String>,
    kind: String,
    max: u32,
}

fn backlights() -> Vec<Backlight> {
    let Ok(entries) = fs::read_dir(BACKLIGHT_DIR) else { return Vec::new() };
    let mut list: Vec<Backlight> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let max = read_u32(path.join("max_brightness")).filter(|&m| m > 0)?;
            let connector = fs::read_link(path.join("device"))
                .ok()
                .and_then(|l| connector_name(&l.file_name()?.to_string_lossy()).map(str::to_string));
            Some(Backlight {
                device: entry.file_name().to_string_lossy().into_owned(),
                connector,
                kind: read_trimmed(path.join("type")).unwrap_or_default(),
                max,
            })
        })
        .collect();
    // Same preference order as GNOME: firmware > platform > raw.
    let rank = |k: &str| match k {
        "firmware" => 0,
        "platform" => 1,
        _ => 2,
    };
    list.sort_by(|a, b| rank(&a.kind).cmp(&rank(&b.kind)).then_with(|| a.device.cmp(&b.device)));
    list
}

fn probe_ddc(bus: u32) -> io::Result<(f64, Control, Option<f64>)> {
    let mut ddc = ddc::Ddc::open(bus)?;
    let (cur, max) = ddc.get_vcp(ddc::VCP_BRIGHTNESS)?;
    let contrast = ddc.get_vcp(ddc::VCP_CONTRAST).ok();
    Ok((
        ddc_percent(cur, max),
        Control::Ddc { bus, max, contrast_max: contrast.map(|(_, m)| m) },
        contrast.map(|(c, m)| ddc_percent(c, m)),
    ))
}

fn i2c_adapter_name(bus: u32) -> String {
    read_trimmed(format!("/sys/bus/i2c/devices/i2c-{bus}/name")).unwrap_or_default()
}

fn all_i2c_buses() -> Vec<u32> {
    let mut buses: Vec<u32> = fs::read_dir("/dev")
        .map(|d| d.flatten().filter_map(|e| i2c_bus_from_name(&e.file_name().to_string_lossy())).collect())
        .unwrap_or_default();
    buses.sort_unstable();
    buses
}

/// Enumerates controllable displays. Slow (DDC probing): call off the main thread.
pub fn scan(include_internal: bool) -> Scan {
    let mut scan = Scan::default();
    let connectors = connected_connectors();
    let mut used_connectors = HashSet::new();
    let mut probed_buses = HashSet::new();
    let mut i2c_denied = false;
    let mut i2c_missing = false;

    // Built-in panels.
    let backlights = backlights();
    if include_internal {
        let internal_connectors: Vec<&Connector> = connectors.iter().filter(|c| c.internal).collect();
        for bl in &backlights {
            let connector = bl.connector.clone().or_else(|| {
                internal_connectors
                    .iter()
                    .find(|c| !used_connectors.contains(&c.name))
                    .map(|c| c.name.clone())
            });
            if let Some(c) = &connector {
                if !used_connectors.insert(c.clone()) {
                    continue; // e.g. acpi_video0 and intel_backlight driving the same panel
                }
            } else if scan.displays.iter().any(|d| d.internal) {
                continue;
            }
            let raw = read_u32(Path::new(BACKLIGHT_DIR).join(&bl.device).join("brightness")).unwrap_or(bl.max);
            let count = backlights.len();
            scan.displays.push(Display {
                name: if count == 1 { "Built-in Display".into() } else { format!("Built-in Display ({})", bl.device) },
                connector,
                internal: true,
                control: Control::Backlight { device: bl.device.clone(), max: bl.max },
                brightness: backlight_percent(raw, bl.max),
                contrast: None,
            });
        }
    } else {
        for c in connectors.iter().filter(|c| c.internal) {
            used_connectors.insert(c.name.clone());
        }
    }

    // External monitors on connectors that expose their DDC bus.
    let remaining: Vec<&Connector> = connectors.iter().filter(|c| !used_connectors.contains(&c.name)).collect();
    for c in remaining {
        let mut display = Display {
            name: c.edid_name.clone().unwrap_or_else(|| c.name.clone()),
            connector: Some(c.name.clone()),
            internal: c.internal,
            control: Control::Software,
            brightness: 100.0,
            contrast: None,
        };
        if let Some(bus) = c.ddc_bus {
            probed_buses.insert(bus);
            match probe_ddc(bus) {
                Ok((brightness, control, contrast)) => {
                    display.brightness = brightness;
                    display.control = control;
                    display.contrast = contrast;
                }
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied => i2c_denied = true,
                Err(e) if e.kind() == io::ErrorKind::NotFound => i2c_missing = true,
                Err(_) => {}
            }
        }
        used_connectors.insert(c.name.clone());
        scan.displays.push(display);
    }

    // Drivers that do not link connectors to i2c buses (e.g. NVIDIA proprietary):
    // probe the remaining GPU buses and match them by EDID name.
    let buses = all_i2c_buses();
    for &bus in &buses {
        if probed_buses.contains(&bus) || !i2c_adapter_name(bus).to_lowercase().contains("nvidia") {
            continue;
        }
        let Ok((brightness, control, contrast)) = probe_ddc(bus) else { continue };
        let name = ddc::read_edid(bus).ok().and_then(|e| edid::monitor_name(&e));
        let slot = scan
            .displays
            .iter_mut()
            .find(|d| d.control == Control::Software && name.as_deref() == Some(d.name.as_str()));
        match slot {
            Some(d) => {
                d.control = control;
                d.brightness = brightness;
                d.contrast = contrast;
            }
            None => scan.displays.push(Display {
                name: name.unwrap_or_else(|| format!("Display on i2c-{bus}")),
                connector: None,
                internal: false,
                control,
                brightness,
                contrast,
            }),
        }
    }

    let has_external = scan.displays.iter().any(|d| !d.internal);
    if has_external && !scan.displays.iter().any(|d| matches!(d.control, Control::Ddc { .. })) {
        if i2c_denied {
            scan.issues.push(Issue::I2cPermissionDenied);
        } else if i2c_missing || buses.is_empty() {
            scan.issues.push(Issue::I2cUnavailable);
        }
    }
    scan
}

/// Writes the backlight directly when sysfs is writable (udev rule installed),
/// otherwise asks systemd-logind, which allows the active session's user to
/// set brightness without any extra permissions.
fn set_backlight_raw(device: &str, raw: u32) -> Result<(), String> {
    let path = PathBuf::from(BACKLIGHT_DIR).join(device).join("brightness");
    if fs::write(&path, raw.to_string()).is_ok() {
        return Ok(());
    }
    let bus = gio::bus_get_sync(gio::BusType::System, gio::Cancellable::NONE).map_err(|e: glib::Error| e.to_string())?;
    bus.call_sync(
        Some("org.freedesktop.login1"),
        "/org/freedesktop/login1/session/auto",
        "org.freedesktop.login1.Session",
        "SetBrightness",
        Some(&("backlight", device, raw).to_variant()),
        None,
        gio::DBusCallFlags::NONE,
        5000,
        gio::Cancellable::NONE,
    )
    .map(|_| ())
    .map_err(|e| format!("logind SetBrightness failed: {e}"))
}

pub fn set_brightness(control: &Control, percent: f64) -> Result<(), String> {
    match control {
        Control::Backlight { device, max } => set_backlight_raw(device, backlight_raw(percent, *max)),
        Control::Ddc { bus, max, .. } => ddc::Ddc::open(*bus)
            .and_then(|mut d| d.set_vcp(ddc::VCP_BRIGHTNESS, ddc_raw(percent, *max)))
            .map_err(|e| format!("DDC/CI write to i2c-{bus} failed: {e}")),
        Control::Software => Ok(()),
    }
}

pub fn set_contrast(control: &Control, percent: f64) -> Result<(), String> {
    match control {
        Control::Ddc { bus, contrast_max: Some(max), .. } => ddc::Ddc::open(*bus)
            .and_then(|mut d| d.set_vcp(ddc::VCP_CONTRAST, ddc_raw(percent, *max)))
            .map_err(|e| format!("DDC/CI write to i2c-{bus} failed: {e}")),
        _ => Err("display does not support contrast".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connector_names() {
        assert_eq!(connector_name("card1-HDMI-A-1"), Some("HDMI-A-1"));
        assert_eq!(connector_name("card0-eDP-1"), Some("eDP-1"));
        assert_eq!(connector_name("renderD128"), None);
        assert_eq!(connector_name("card1"), None);
    }
}
