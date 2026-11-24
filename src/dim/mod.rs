//! Software dimming below the hardware minimum.
//!
//! Backend selection:
//!  * Wayland compositor with wlr-gamma-control and/or wlr-layer-shell
//!    (Hyprland, Sway, KDE Plasma, ...) -> [`wayland::WaylandDimmer`]
//!  * GNOME (Mutter) or Cinnamon (Muffin), X11 or Wayland
//!    -> [`display_config::DisplayConfigDimmer`]
//!  * Windows: GDI gamma ramp or overlay windows -> [`windows::WindowsDimmer`]

#[cfg(not(windows))]
pub mod display_config;
#[cfg(not(windows))]
pub mod wayland;
#[cfg(windows)]
pub mod windows;
#[cfg(not(windows))]
mod wire;

#[cfg(not(windows))]
use std::rc::Rc;

/// How the Wayland and Windows backends dim an output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Technique {
    /// Gamma where possible, overlay otherwise.
    Auto,
    GammaOnly,
    OverlayOnly,
}

impl Technique {
    pub fn as_str(self) -> &'static str {
        match self {
            Technique::Auto => "auto",
            Technique::GammaOnly => "gamma",
            Technique::OverlayOnly => "overlay",
        }
    }

    /// Unknown values fall back to [`Technique::Auto`].
    pub fn parse(s: &str) -> Self {
        match s {
            "gamma" => Technique::GammaOnly,
            "overlay" => Technique::OverlayOnly,
            _ => Technique::Auto,
        }
    }
}

/// Never dim below this factor: a fully black screen would be unusable and
/// impossible to recover from without knowing where the slider is.
pub const MIN_FACTOR: f64 = 0.05;

/// Kernel connectors ("HDMI-A-1") and X11 outputs ("HDMI-1", or "HDMI1" with
/// older drivers) name the same output differently; Wayland compositors use
/// the kernel names. Compare names in this normalized form.
fn output_key(name: &str) -> String {
    let key: String = name.chars().filter(|c| *c != '-').flat_map(char::to_lowercase).collect();
    match key.strip_prefix("hdmia") {
        Some(rest) => format!("hdmi{rest}"),
        None => key,
    }
}

enum Backend {
    #[cfg(not(windows))]
    Wayland(wayland::WaylandDimmer),
    #[cfg(not(windows))]
    DisplayConfig(Rc<display_config::DisplayConfigDimmer>),
    #[cfg(windows)]
    Windows(windows::WindowsDimmer),
    #[cfg_attr(windows, allow(dead_code))]
    None,
}

pub struct Dimmer {
    backend: Backend,
}

impl Dimmer {
    #[cfg(windows)]
    pub fn new(technique: Technique) -> Self {
        Self { backend: Backend::Windows(windows::WindowsDimmer::start(technique)) }
    }

    #[cfg(not(windows))]
    pub fn new(technique: Technique) -> Self {
        let backend = if let Some(w) = wayland::WaylandDimmer::start(technique) {
            Backend::Wayland(w)
        } else if let Some(d) = display_config::DisplayConfigDimmer::start() {
            Backend::DisplayConfig(d)
        } else {
            Backend::None
        };
        Self { backend }
    }

    pub fn available(&self) -> bool {
        !matches!(self.backend, Backend::None)
    }

    pub fn description(&self) -> String {
        match &self.backend {
            #[cfg(not(windows))]
            Backend::Wayland(w) => match (w.caps.gamma, w.caps.overlay) {
                (true, true) => "Wayland gamma control, overlay fallback".into(),
                (true, false) => "Wayland gamma control".into(),
                (false, _) => "Wayland overlay layer".into(),
            },
            #[cfg(not(windows))]
            Backend::DisplayConfig(d) => d.compositor().label().into(),
            #[cfg(windows)]
            Backend::Windows(_) => "Windows gamma ramp, overlay fallback".into(),
            Backend::None => "Unavailable".into(),
        }
    }

    pub fn supports_technique_choice(&self) -> bool {
        match &self.backend {
            #[cfg(not(windows))]
            Backend::Wayland(w) => w.caps.gamma && w.caps.overlay,
            #[cfg(windows)]
            Backend::Windows(_) => true,
            _ => false,
        }
    }

    pub fn set_technique(&self, technique: Technique) {
        match &self.backend {
            #[cfg(not(windows))]
            Backend::Wayland(w) => w.set_technique(technique),
            #[cfg(windows)]
            Backend::Windows(w) => w.set_technique(technique),
            _ => {}
        }
    }

    fn outputs(&self) -> Vec<String> {
        match &self.backend {
            #[cfg(not(windows))]
            Backend::Wayland(w) => w.outputs(),
            #[cfg(not(windows))]
            Backend::DisplayConfig(d) => d.outputs(),
            #[cfg(windows)]
            Backend::Windows(w) => w.outputs(),
            Backend::None => Vec::new(),
        }
    }

    /// The backend's name for a kernel connector, if it knows that output.
    fn output_name(&self, connector: &str) -> Option<String> {
        let key = output_key(connector);
        self.outputs().into_iter().find(|o| output_key(o) == key)
    }

    /// Whether `connector` can be dimmed. Assumes yes while the backend has
    /// not reported its outputs yet.
    pub fn has_output(&self, connector: &str) -> bool {
        self.outputs().is_empty() || self.output_name(connector).is_some()
    }

    /// `target` is a connector name or "*" for all outputs; 1.0 = no dimming.
    pub fn set(&self, target: &str, factor: f64) {
        let factor = factor.clamp(MIN_FACTOR, 1.0);
        let target = match target {
            "*" => target.to_string(),
            _ => self.output_name(target).unwrap_or_else(|| target.to_string()),
        };
        match &self.backend {
            #[cfg(not(windows))]
            Backend::Wayland(w) => w.set(&target, factor),
            #[cfg(not(windows))]
            Backend::DisplayConfig(d) => d.set(&target, factor),
            #[cfg(windows)]
            Backend::Windows(w) => w.set(&target, factor),
            Backend::None => {}
        }
    }

    pub fn shutdown(&self) {
        match &self.backend {
            #[cfg(not(windows))]
            Backend::Wayland(w) => w.shutdown(),
            #[cfg(not(windows))]
            Backend::DisplayConfig(d) => d.shutdown(),
            #[cfg(windows)]
            Backend::Windows(w) => w.shutdown(),
            Backend::None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::output_key;

    #[test]
    fn x11_and_kernel_output_names_match() {
        assert_eq!(output_key("HDMI-A-1"), output_key("HDMI-1"));
        assert_eq!(output_key("HDMI-A-2"), output_key("HDMI2"));
        assert_eq!(output_key("eDP-1"), output_key("eDP1"));
        assert_eq!(output_key("DVI-I-1"), output_key("DVI-I-1"));
        assert_ne!(output_key("HDMI-A-1"), output_key("HDMI-B-1"));
        assert_ne!(output_key("DP-1"), output_key("eDP-1"));
        assert_ne!(output_key("DP-1"), output_key("DP-11"));
    }
}
