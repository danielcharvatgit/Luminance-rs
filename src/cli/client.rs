//! Where CLI commands are executed.
//!
//! Commands go to the running Luminance service over D-Bus, which is started
//! on demand (D-Bus activation, or by spawning ourselves with
//! `--gapplication-service`). Only a long-running process can keep software
//! dimming active, so this is what lets e.g. a hotkey dim below the hardware
//! minimum. If the service cannot be reached, hardware is controlled directly.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use super::args::Target;
use crate::config::{object_path, APP_ID};
use crate::hw;
use crate::json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct Monitor {
    pub name: String,
    pub connector: Option<String>,
    /// "backlight", "ddc" or "software".
    pub kind: String,
    pub brightness: f64,
    pub contrast: Option<f64>,
    /// Software dim factor, 1.0 = not dimmed (service only).
    pub software_dim: Option<f64>,
}

impl Monitor {
    /// "DELL U2415 (DP-2)"
    pub fn label(&self) -> String {
        match &self.connector {
            Some(c) if *c != self.name => format!("{} ({c})", self.name),
            _ => self.name.clone(),
        }
    }
}

pub enum Backend {
    Service(gio::DBusConnection),
    Direct(Vec<hw::Display>),
}

impl Backend {
    /// Connects to (or starts) the service, falling back to direct control.
    /// `refresh` re-reads the hardware so the values are not stale.
    pub fn connect(refresh: bool) -> Self {
        if cfg!(windows) {
            // No D-Bus service on Windows: always direct, hardware percentages.
            return Backend::Direct(hw::scan(true).displays);
        }
        match connect_service(refresh) {
            Some(conn) => Backend::Service(conn),
            None => {
                eprintln!(
                    "Luminance service unavailable, using direct hardware control: \
                     values are hardware percentages, no software dimming."
                );
                Backend::Direct(hw::scan(true).displays)
            }
        }
    }

    pub fn monitors(&self) -> Result<Vec<Monitor>, String> {
        match self {
            Backend::Service(conn) => {
                let (json,): (String,) = call(conn, "GetMonitors", None)?.get().ok_or("unexpected reply")?;
                parse_monitors(&json)
            }
            Backend::Direct(displays) => Ok(displays
                .iter()
                .map(|d| Monitor {
                    name: d.name.clone(),
                    connector: d.connector.clone(),
                    kind: d.control.kind().into(),
                    brightness: d.brightness,
                    contrast: d.contrast,
                    software_dim: None,
                })
                .collect()),
        }
    }

    /// `index` is 0-based, in the order returned by [`Backend::monitors`].
    pub fn set_brightness(&self, index: usize, value: f64) -> Result<(), String> {
        match self {
            Backend::Service(conn) => call(conn, "SetBrightness", Some(&(index.to_string(), value).to_variant())).map(|_| ()),
            Backend::Direct(displays) => match &displays[index].control {
                hw::Control::Software => Err("software dimming needs the Luminance service".into()),
                control => hw::set_brightness(control, value),
            },
        }
    }

    pub fn set_contrast(&self, index: usize, value: f64) -> Result<(), String> {
        match self {
            Backend::Service(conn) => call(conn, "SetContrast", Some(&(index.to_string(), value).to_variant())).map(|_| ()),
            Backend::Direct(displays) => hw::set_contrast(&displays[index].control, value),
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Backend::Service(_) => "Luminance service",
            Backend::Direct(_) => "direct hardware control",
        }
    }
}

fn parse_monitors(json: &str) -> Result<Vec<Monitor>, String> {
    let value = Value::parse(json)?;
    let list = value.as_array().ok_or("unexpected GetMonitors reply")?;
    Ok(list
        .iter()
        .map(|m| {
            let text = |key| m.get(key).and_then(Value::as_str);
            let num = |key| m.get(key).and_then(Value::as_f64);
            Monitor {
                name: text("name").unwrap_or("Unknown").to_string(),
                connector: text("connector").map(str::to_string),
                kind: text("kind").unwrap_or("").to_string(),
                brightness: num("brightness").unwrap_or(0.0),
                contrast: m
                    .get("has_contrast")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                    .then(|| num("contrast").unwrap_or(0.0)),
                software_dim: num("software_dim"),
            }
        })
        .collect())
}

/// Resolves a selection to 0-based indices (all monitors when empty),
/// without duplicates and in the given order.
pub fn resolve(monitors: &[Monitor], targets: &[Target]) -> Result<Vec<usize>, String> {
    if targets.is_empty() {
        return Ok((0..monitors.len()).collect());
    }
    let mut indices = Vec::new();
    for target in targets {
        let index = match target {
            Target::Number(n) if *n <= monitors.len() => n - 1,
            Target::Number(n) => {
                return Err(format!("Invalid display number: {n} (found {} display{})", monitors.len(), if monitors.len() == 1 { "" } else { "s" }));
            }
            Target::Name(name) => {
                // Connector names are unique; monitor names may not be.
                let by_connector = monitors
                    .iter()
                    .position(|m| m.connector.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(name)));
                let by_name: Vec<usize> = (0..monitors.len()).filter(|&i| monitors[i].name.eq_ignore_ascii_case(name)).collect();
                match (by_connector, by_name.as_slice()) {
                    (Some(i), _) | (None, &[i]) => i,
                    (None, []) => {
                        let available: Vec<String> = monitors.iter().map(Monitor::label).collect();
                        return Err(format!("No display named '{name}'. Available: {}", available.join(", ")));
                    }
                    (None, _) => return Err(format!("'{name}' matches several displays; use the connector or number")),
                }
            }
        };
        if !indices.contains(&index) {
            indices.push(index);
        }
    }
    Ok(indices)
}

fn call(conn: &gio::DBusConnection, method: &str, args: Option<&glib::Variant>) -> Result<glib::Variant, String> {
    conn.call_sync(
        Some(APP_ID),
        &object_path(),
        APP_ID,
        method,
        args,
        None,
        gio::DBusCallFlags::NONE,
        30_000,
        gio::Cancellable::NONE,
    )
    .map_err(|e| e.to_string())
}

fn name_has_owner(conn: &gio::DBusConnection) -> bool {
    conn.call_sync(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "NameHasOwner",
        Some(&(APP_ID,).to_variant()),
        None,
        gio::DBusCallFlags::NONE,
        2000,
        gio::Cancellable::NONE,
    )
    .ok()
    .and_then(|v| v.get::<(bool,)>())
    .is_some_and(|(b,)| b)
}

fn connect_service(refresh: bool) -> Option<gio::DBusConnection> {
    let conn = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).ok()?;
    if name_has_owner(&conn) {
        // A running service may hold values from before e.g. a brightness
        // key press. GetMonitors waits for the rescan to finish.
        if refresh {
            let _ = call(&conn, "Refresh", None);
        }
        return Some(conn);
    }
    // D-Bus activation (installed service file). A fresh service scans on
    // startup, so use a read-only call: "Refresh" would scan a second time.
    if call(&conn, "GetMonitors", None).is_ok() || name_has_owner(&conn) {
        return Some(conn);
    }
    // Inside Flatpak a spawned child would die with this sandbox; there the
    // exported D-Bus service file is the way to start the service.
    if std::path::Path::new("/.flatpak-info").exists() {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    Command::new(exe)
        .arg("--gapplication-service")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        if name_has_owner(&conn) {
            return Some(conn);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub fn sample() -> Vec<Monitor> {
        parse_monitors(
            r#"[
            {"id":"0","name":"Built-in Display","brightness":40.5,"has_contrast":false,"contrast":-1,"connector":"eDP-1","kind":"backlight","software_dim":1.0},
            {"id":"1","name":"DELL U2415","brightness":70,"has_contrast":true,"contrast":75,"connector":"DP-2","kind":"ddc","software_dim":1.0},
            {"id":"2","name":"DELL U2415","brightness":10,"has_contrast":false,"contrast":-1,"connector":"HDMI-A-1","kind":"software","software_dim":0.46}
        ]"#,
        )
        .unwrap()
    }

    #[test]
    fn parses_service_reply() {
        let m = sample();
        assert_eq!(m.len(), 3);
        assert_eq!(m[0].contrast, None);
        assert_eq!(m[1].contrast, Some(75.0));
        assert_eq!(m[2].software_dim, Some(0.46));
        assert_eq!(m[1].label(), "DELL U2415 (DP-2)");
        assert!(parse_monitors("{}").is_err());
    }

    #[test]
    fn resolves_targets() {
        let m = sample();
        let name = |s: &str| Target::Name(s.into());
        assert_eq!(resolve(&m, &[]).unwrap(), vec![0, 1, 2]);
        assert_eq!(resolve(&m, &[name("dp-2"), Target::Number(1), name("DP-2")]).unwrap(), vec![1, 0]);
        assert_eq!(resolve(&m, &[name("built-in display")]).unwrap(), vec![0]);
        assert!(resolve(&m, &[Target::Number(4)]).is_err());
        assert!(resolve(&m, &[name("DP-9")]).is_err());
        assert!(resolve(&m, &[name("DELL U2415")]).is_err(), "ambiguous name");
    }
}
