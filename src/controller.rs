//! Application state: the list of displays, their brightness levels, and the
//! glue between hardware control and software dimming.
//!
//! Every display has a single "level" (0-100) shown on its slider. With extra
//! dimming enabled, the bottom [`DIM_ZONE`] of the slider is software dimming
//! while the hardware stays at its minimum; the rest maps to the full hardware
//! range:
//!
//! ```text
//!  0 ............ DIM_ZONE ........................ 100
//!  |  software dim floor..1 | hardware 0%..100%       |
//! ```

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib;

use crate::dim::Dimmer;
use crate::hw::{self, worker::Worker, Control, Issue};
use crate::lid::Lid;
use crate::settings::Settings;

pub const DIM_ZONE: f64 = 25.0;

#[derive(Clone, Debug)]
pub struct DisplayState {
    pub info: hw::Display,
    pub level: f64,
    pub dim: f64,
}

#[derive(Clone, Debug)]
pub enum Event {
    Loading,
    Loaded,
    Level(usize, f64),
    Contrast(usize, f64),
    Settings,
}

type Listener = Box<dyn Fn(&Event)>;
type OsdHandler = Box<dyn Fn(f64, &str, bool)>;

pub struct Controller {
    displays: RefCell<Vec<DisplayState>>,
    issues: RefCell<Vec<Issue>>,
    loading: Cell<bool>,
    settings: RefCell<Settings>,
    pub dimmer: Dimmer,
    worker: Worker,
    lid: Lid,
    listeners: RefCell<Vec<Listener>>,
    pending: RefCell<Vec<Box<dyn FnOnce()>>>,
    osd: RefCell<Option<OsdHandler>>,
}

/// Maps a slider level to (hardware percent, software dim factor).
fn split(zone: f64, floor: f64, level: f64) -> (f64, f64) {
    if zone <= 0.0 || level >= zone {
        let hw = if zone >= 100.0 { 100.0 } else { (level - zone) / (100.0 - zone) * 100.0 };
        (hw, 1.0)
    } else {
        (0.0, floor + (1.0 - floor) * level / zone)
    }
}

/// Inverse of [`split`].
fn join(zone: f64, floor: f64, hw: f64, dim: f64) -> f64 {
    if zone <= 0.0 {
        hw
    } else if dim < 0.999 || zone >= 100.0 {
        (zone * (dim - floor) / (1.0 - floor)).clamp(0.0, zone)
    } else {
        zone + hw * (100.0 - zone) / 100.0
    }
}

impl Controller {
    pub fn new() -> Rc<Self> {
        let settings = Settings::load();
        let dimmer = Dimmer::new(settings.technique);
        let this = Rc::new(Self {
            displays: RefCell::new(Vec::new()),
            issues: RefCell::new(Vec::new()),
            loading: Cell::new(false),
            settings: RefCell::new(settings),
            dimmer,
            worker: Worker::spawn(),
            lid: Lid::new(),
            listeners: RefCell::new(Vec::new()),
            pending: RefCell::new(Vec::new()),
            osd: RefCell::new(None),
        });
        let weak = Rc::downgrade(&this);
        this.lid.connect_changed(move || {
            if let Some(c) = weak.upgrade() {
                if c.settings().hide_internal_if_lid_closed {
                    c.refresh();
                }
            }
        });
        this
    }

    pub fn subscribe<F: Fn(&Event) + 'static>(&self, f: F) {
        self.listeners.borrow_mut().push(Box::new(f));
    }

    pub fn set_osd_handler<F: Fn(f64, &str, bool) + 'static>(&self, f: F) {
        *self.osd.borrow_mut() = Some(Box::new(f));
    }

    fn emit(&self, event: Event) {
        for l in self.listeners.borrow().iter() {
            l(&event);
        }
    }

    pub fn settings(&self) -> Settings {
        self.settings.borrow().clone()
    }

    pub fn is_loading(&self) -> bool {
        self.loading.get()
    }

    pub fn issues(&self) -> Vec<Issue> {
        self.issues.borrow().clone()
    }

    pub fn displays(&self) -> Vec<DisplayState> {
        self.displays.borrow().clone()
    }

    pub fn display_count(&self) -> usize {
        self.displays.borrow().len()
    }

    pub fn any_dimmed(&self) -> bool {
        self.displays.borrow().iter().any(|d| d.dim < 0.999)
    }

    /// Runs `f` now, or once the current scan finishes.
    pub fn when_loaded<F: FnOnce() + 'static>(&self, f: F) {
        if self.loading.get() {
            self.pending.borrow_mut().push(Box::new(f));
        } else {
            f();
        }
    }

    fn extra_dim_active(&self) -> bool {
        self.settings.borrow().extra_dimming && self.dimmer.available()
    }

    /// Size of the software zone at the bottom of a display's slider.
    pub fn dim_zone(&self, info: &hw::Display) -> f64 {
        match info.control {
            Control::Software => 100.0,
            _ if self.extra_dim_active() => DIM_ZONE,
            _ => 0.0,
        }
    }

    fn floor(&self) -> f64 {
        self.settings.borrow().dim_floor
    }

    fn dim_target(&self, displays: &[DisplayState], info: &hw::Display) -> Option<String> {
        match &info.connector {
            Some(c) => Some(c.clone()),
            None if displays.len() == 1 => Some("*".into()),
            None => None,
        }
    }

    pub fn refresh(self: &Rc<Self>) {
        if self.loading.replace(true) {
            return;
        }
        self.emit(Event::Loading);
        let include_internal = !(self.settings.borrow().hide_internal_if_lid_closed && self.lid.is_closed());
        let this = self.clone();
        glib::spawn_future_local(async move {
            let scan = gtk::gio::spawn_blocking(move || hw::scan(include_internal)).await.unwrap_or_default();
            this.apply_scan(scan);
        });
    }

    fn apply_scan(&self, scan: hw::Scan) {
        let previous = self.displays.borrow().clone();
        let show_software = self.extra_dim_active();
        let mut displays: Vec<DisplayState> = scan
            .displays
            .into_iter()
            .filter(|d| match d.control {
                // Software-only entries need a dimmer and a matching output.
                Control::Software => {
                    show_software && d.connector.as_ref().is_some_and(|c| self.dimmer.has_output(c))
                }
                _ => true,
            })
            .map(|info| {
                let dim = previous.iter().find(|p| p.info.key() == info.key()).map(|p| p.dim).unwrap_or(1.0);
                DisplayState { level: 0.0, dim, info }
            })
            .collect();
        for d in displays.iter_mut() {
            d.level = join(self.dim_zone(&d.info), self.floor(), d.info.brightness, d.dim);
        }
        // Displays that disappeared must not stay dimmed.
        for p in previous.iter().filter(|p| p.dim < 0.999) {
            if !displays.iter().any(|d| d.info.key() == p.info.key()) {
                if let Some(c) = &p.info.connector {
                    self.dimmer.set(c, 1.0);
                }
            }
        }
        *self.displays.borrow_mut() = displays;
        *self.issues.borrow_mut() = scan.issues;
        self.loading.set(false);
        self.emit(Event::Loaded);
        let pending: Vec<_> = self.pending.borrow_mut().drain(..).collect();
        for f in pending {
            f();
        }
    }

    pub fn level(&self, index: usize) -> Option<f64> {
        self.displays.borrow().get(index).map(|d| d.level)
    }

    pub fn set_level(&self, index: usize, level: f64, osd: bool) {
        if !level.is_finite() {
            return;
        }
        let level = level.clamp(0.0, 100.0);
        let (info, current_dim, target) = {
            let displays = self.displays.borrow();
            let Some(d) = displays.get(index) else { return };
            (d.info.clone(), d.dim, self.dim_target(&displays, &d.info))
        };
        let (hw, dim) = split(self.dim_zone(&info), self.floor(), level);

        if !matches!(info.control, Control::Software) && (hw - info.brightness).abs() > 0.01 {
            self.worker.set_brightness(info.key(), info.control.clone(), hw);
        }
        if let Some(t) = &target {
            if (dim - current_dim).abs() > 1e-4 {
                self.dimmer.set(t, dim);
            }
        }
        {
            let mut displays = self.displays.borrow_mut();
            let d = &mut displays[index];
            d.level = level;
            d.dim = if target.is_some() { dim } else { 1.0 };
            d.info.brightness = hw;
        }
        self.emit(Event::Level(index, level));
        if osd {
            if let Some(h) = self.osd.borrow().as_ref() {
                h(level, &info.name, false);
            }
        }
    }

    /// Sets one display, or all of them when brightness is linked.
    pub fn set_level_linked(&self, index: usize, level: f64, osd: bool) {
        if self.settings.borrow().brightness_linked {
            for i in 0..self.display_count() {
                self.set_level(i, level, osd && i == index);
            }
        } else {
            self.set_level(index, level, osd);
        }
    }

    pub fn set_contrast(&self, index: usize, value: f64, osd: bool) -> bool {
        if !value.is_finite() {
            return false;
        }
        let value = value.clamp(0.0, 100.0);
        let info = {
            let mut displays = self.displays.borrow_mut();
            let Some(d) = displays.get_mut(index) else { return false };
            if d.info.contrast.is_none() {
                return false;
            }
            d.info.contrast = Some(value);
            d.info.clone()
        };
        self.worker.set_contrast(info.key(), info.control.clone(), value);
        self.emit(Event::Contrast(index, value));
        if osd {
            if let Some(h) = self.osd.borrow().as_ref() {
                h(value, &info.name, true);
            }
        }
        true
    }

    /// Mutates settings, persists them and re-derives levels where needed.
    pub fn update_settings<F: FnOnce(&mut Settings)>(self: &Rc<Self>, f: F) {
        let before = self.settings();
        let after = {
            let mut s = self.settings.borrow_mut();
            f(&mut s);
            s.save();
            s.clone()
        };

        if before.technique != after.technique {
            self.dimmer.set_technique(after.technique);
        }
        if before.extra_dimming != after.extra_dimming {
            if !after.extra_dimming {
                // Leave software dimming; the hardware level is kept as is.
                let targets: Vec<Option<String>> = {
                    let displays = self.displays.borrow();
                    displays.iter().filter(|d| d.dim < 0.999).map(|d| self.dim_target(&displays, &d.info)).collect()
                };
                for t in targets.into_iter().flatten() {
                    self.dimmer.set(&t, 1.0);
                }
                for d in self.displays.borrow_mut().iter_mut() {
                    d.dim = 1.0;
                }
            }
            // Software-only displays appear/disappear: rescan.
            self.refresh();
        } else if (before.dim_floor - after.dim_floor).abs() > f64::EPSILON {
            // Keep slider positions, re-apply the new darkness.
            for i in 0..self.display_count() {
                let level = self.level(i).unwrap_or(100.0);
                self.set_level(i, level, false);
            }
        } else if before.hide_internal_if_lid_closed != after.hide_internal_if_lid_closed && self.lid.is_closed() {
            self.refresh();
        } else if after.brightness_linked && !before.brightness_linked {
            let max = self.displays.borrow().iter().map(|d| d.level).fold(0.0, f64::max);
            for i in 0..self.display_count() {
                self.set_level(i, max, false);
            }
        }
        self.emit(Event::Settings);
    }

    pub fn shutdown(&self) {
        self.dimmer.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zone_mapping() {
        assert_eq!(split(DIM_ZONE, 0.1, 100.0), (100.0, 1.0));
        assert_eq!(split(DIM_ZONE, 0.1, DIM_ZONE), (0.0, 1.0));
        assert_eq!(split(DIM_ZONE, 0.1, 0.0), (0.0, 0.1));
        assert_eq!(split(0.0, 0.1, 40.0), (40.0, 1.0));
        for level in [0.0, 10.0, 24.0, 25.0, 60.0, 100.0] {
            let (hw, dim) = split(DIM_ZONE, 0.1, level);
            assert!((join(DIM_ZONE, 0.1, hw, dim) - level).abs() < 1e-9, "level {level}");
        }
    }
}
