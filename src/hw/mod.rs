//! Hardware brightness of built-in panels and external (DDC/CI) monitors.
//!
//!  * Linux ([`linux`]): sysfs backlights through systemd-logind, DDC/CI
//!    over i2c-dev.
//!  * Windows ([`windows`]): the `\\.\LCD` brightness IOCTLs or WMI for
//!    built-in panels, DDC/CI through the Monitor Configuration API (dxva2).
//!
//! Everything in here is blocking and is meant to run off the GTK main thread.

#[cfg(not(windows))]
pub mod ddc;
#[cfg(not(windows))]
pub mod edid;
#[cfg(not(windows))]
mod linux;
#[cfg(windows)]
mod windows;
pub mod worker;

#[cfg(not(windows))]
pub use linux::{scan, set_brightness, set_contrast};
#[cfg(windows)]
pub use windows::{scan, set_brightness, set_contrast};

#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    /// Linux: /sys/class/backlight/<device>. Windows: "LCD" (brightness
    /// IOCTLs) or "WMI", with `max` = 100.
    Backlight { device: String, max: u32 },
    /// Linux: DDC/CI on /dev/i2c-<bus>. Windows: physical monitor
    /// `bus % 16` of the GDI display `\\.\DISPLAY<bus / 16>`.
    Ddc { bus: u32, max: u16, contrast_max: Option<u16> },
    /// No hardware control available; only software dimming is possible.
    Software,
}

impl Control {
    /// Machine-readable kind, as used in the D-Bus and JSON output.
    pub fn kind(&self) -> &'static str {
        match self {
            Control::Backlight { .. } => "backlight",
            Control::Ddc { .. } => "ddc",
            Control::Software => "software",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Display {
    pub name: String,
    /// DRM connector name, e.g. "eDP-1" or "DP-2" (matches Wayland output
    /// names). On Windows the GDI display name, e.g. "DISPLAY1".
    pub connector: Option<String>,
    pub internal: bool,
    pub control: Control,
    /// Current hardware brightness in percent of the usable hardware range.
    pub brightness: f64,
    /// Current contrast in percent, if the display supports it.
    pub contrast: Option<f64>,
}

impl Display {
    /// Stable identity across rescans, used to carry software dim state over.
    pub fn key(&self) -> String {
        match (&self.connector, &self.control) {
            (Some(c), _) => c.clone(),
            (None, Control::Backlight { device, .. }) => format!("backlight:{device}"),
            (None, Control::Ddc { bus, .. }) => format!("i2c:{bus}"),
            (None, Control::Software) => self.name.clone(),
        }
    }

    pub fn kind_label(&self) -> String {
        let kind = match &self.control {
            Control::Backlight { device, .. } => format!("Built-in backlight ({device})"),
            Control::Ddc { .. } => "DDC/CI".to_string(),
            Control::Software => "Software dimming only".to_string(),
        };
        match &self.connector {
            Some(c) => format!("{kind} · {c}"),
            None => kind,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Issue {
    /// /dev/i2c-* does not exist: the i2c-dev kernel module is not loaded,
    /// or the Flatpak has no device access.
    I2cUnavailable,
    /// /dev/i2c-* exists but cannot be opened.
    I2cPermissionDenied,
}

#[derive(Default)]
pub struct Scan {
    pub displays: Vec<Display>,
    pub issues: Vec<Issue>,
}

/// Smallest raw backlight value we are willing to write. Zero turns the panel
/// off on many laptops, so hardware never goes below ~1%; anything darker is
/// done in software.
pub(crate) fn backlight_min_raw(max: u32) -> u32 {
    max.div_ceil(100).max(1)
}

pub(crate) fn backlight_percent(raw: u32, max: u32) -> f64 {
    let min = backlight_min_raw(max);
    if max <= min {
        return 100.0;
    }
    (raw.saturating_sub(min) as f64 * 100.0 / (max - min) as f64).clamp(0.0, 100.0)
}

pub(crate) fn backlight_raw(percent: f64, max: u32) -> u32 {
    let min = backlight_min_raw(max);
    min + ((max - min) as f64 * percent.clamp(0.0, 100.0) / 100.0).round() as u32
}

pub(crate) fn ddc_percent(raw: u16, max: u16) -> f64 {
    (raw as f64 * 100.0 / max as f64).clamp(0.0, 100.0)
}

pub(crate) fn ddc_raw(percent: f64, max: u16) -> u16 {
    (max as f64 * percent.clamp(0.0, 100.0) / 100.0).round() as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backlight_never_reaches_zero() {
        assert_eq!(backlight_raw(0.0, 21333), 214);
        assert_eq!(backlight_raw(100.0, 21333), 21333);
        assert_eq!(backlight_raw(0.0, 7), 1);
        assert!((backlight_percent(backlight_raw(42.0, 96000), 96000) - 42.0).abs() < 0.01);
    }
}
