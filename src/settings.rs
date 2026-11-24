//! Preferences stored in a key file under the user config directory
//! (~/.var/app/<app-id>/config/... inside Flatpak). Unlike GSettings this
//! needs no installed schema, so `cargo run` works out of the box.

use std::path::PathBuf;

use gtk::glib;

use crate::dim::Technique;

const GROUP: &str = "Luminance";

#[derive(Clone, Debug)]
pub struct Settings {
    pub brightness_linked: bool,
    pub hide_internal_if_lid_closed: bool,
    pub contrast_hidden: bool,
    /// Extend sliders below the hardware minimum using software dimming.
    pub extra_dimming: bool,
    /// Darkest software dim factor (0.05 - 0.50).
    pub dim_floor: f64,
    pub technique: Technique,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            brightness_linked: false,
            hide_internal_if_lid_closed: false,
            contrast_hidden: false,
            extra_dimming: true,
            dim_floor: 0.10,
            technique: Technique::Auto,
        }
    }
}

/// Range of [`Settings::dim_floor`] (5 % to 50 %, as offered in the menu).
pub const MAX_DIM_FLOOR: f64 = 0.50;

/// Validates a darkest-level value: non-finite values are rejected, others
/// clamped to the allowed range.
pub fn sanitize_dim_floor(value: f64) -> Option<f64> {
    value.is_finite().then(|| value.clamp(crate::dim::MIN_FACTOR, MAX_DIM_FLOOR))
}

/// Parses a darkest-level value as sent by the `dim-floor` action ("0.10").
pub fn parse_dim_floor(text: &str) -> Option<f64> {
    text.trim().parse().ok().and_then(sanitize_dim_floor)
}

pub fn config_dir() -> PathBuf {
    glib::user_config_dir().join("luminance-rs")
}

fn path() -> PathBuf {
    config_dir().join("settings.ini")
}

impl Settings {
    pub fn load() -> Self {
        let mut s = Self::default();
        let kf = glib::KeyFile::new();
        if kf.load_from_file(path(), glib::KeyFileFlags::NONE).is_err() {
            return s;
        }
        let get_bool = |key: &str, default: bool| kf.boolean(GROUP, key).unwrap_or(default);
        s.brightness_linked = get_bool("brightness-linked", s.brightness_linked);
        s.hide_internal_if_lid_closed = get_bool("hide-internal-if-lid-closed", s.hide_internal_if_lid_closed);
        s.contrast_hidden = get_bool("contrast-hidden", s.contrast_hidden);
        s.extra_dimming = get_bool("extra-dimming", s.extra_dimming);
        if let Some(floor) = kf.double(GROUP, "dim-floor").ok().and_then(sanitize_dim_floor) {
            s.dim_floor = floor;
        }
        if let Ok(t) = kf.string(GROUP, "dimming-technique") {
            s.technique = Technique::parse(&t);
        }
        s
    }

    pub fn save(&self) {
        let kf = glib::KeyFile::new();
        kf.set_boolean(GROUP, "brightness-linked", self.brightness_linked);
        kf.set_boolean(GROUP, "hide-internal-if-lid-closed", self.hide_internal_if_lid_closed);
        kf.set_boolean(GROUP, "contrast-hidden", self.contrast_hidden);
        kf.set_boolean(GROUP, "extra-dimming", self.extra_dimming);
        kf.set_double(GROUP, "dim-floor", self.dim_floor);
        kf.set_string(GROUP, "dimming-technique", self.technique.as_str());
        let _ = std::fs::create_dir_all(config_dir());
        if let Err(e) = kf.save_to_file(path()) {
            eprintln!("luminance: failed to save settings: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dim_floor_is_validated() {
        assert_eq!(parse_dim_floor("0.10"), Some(0.10));
        assert_eq!(parse_dim_floor("0.5"), Some(0.5));
        assert_eq!(parse_dim_floor("1.0"), Some(MAX_DIM_FLOOR));
        assert_eq!(parse_dim_floor("0.01"), Some(crate::dim::MIN_FACTOR));
        assert_eq!(parse_dim_floor("-3"), Some(crate::dim::MIN_FACTOR));
        for bad in ["nan", "NaN", "inf", "-inf", "", "abc", "0.1x"] {
            assert_eq!(parse_dim_floor(bad), None, "accepted {bad:?}");
        }
        assert_eq!(sanitize_dim_floor(f64::NAN), None);
        assert_eq!(sanitize_dim_floor(f64::INFINITY), None);
        assert_eq!(sanitize_dim_floor(0.3), Some(0.3));
    }
}
