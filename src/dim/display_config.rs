//! Software dimming on GNOME (Mutter) and Cinnamon (Muffin) through their
//! DisplayConfig D-Bus API. Works in both X11 and Wayland sessions.
//!
//! Neither compositor implements gamma-control or layer-shell, but both
//! expose `SetCrtcGamma`, the call their settings daemons use for Night
//! Light. We scale the ramp that is active when dimming starts (so an active
//! Night Light tint is kept), and periodically re-assert it in case the
//! settings daemon replaces it.
//!
//! Unlike the Wayland protocols, a ramp set this way outlives the process, so
//! a marker file records that dimming is active and the next start restores a
//! neutral ramp after a crash.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compositor {
    Mutter,
    Muffin,
}

impl Compositor {
    fn bus_name(self) -> &'static str {
        match self {
            Compositor::Mutter => "org.gnome.Mutter.DisplayConfig",
            Compositor::Muffin => "org.cinnamon.Muffin.DisplayConfig",
        }
    }

    fn path(self) -> &'static str {
        match self {
            Compositor::Mutter => "/org/gnome/Mutter/DisplayConfig",
            Compositor::Muffin => "/org/cinnamon/Muffin/DisplayConfig",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Compositor::Mutter => "GNOME (Mutter) gamma",
            Compositor::Muffin => "Cinnamon (Muffin) gamma",
        }
    }

    fn marker(self) -> PathBuf {
        let name = match self {
            Compositor::Mutter => "mutter-gamma-active",
            Compositor::Muffin => "muffin-gamma-active",
        };
        crate::settings::config_dir().join(name)
    }
}

type Ramp = [Vec<u16>; 3];

struct Crtc {
    id: u32,
    base: Option<Ramp>,
    applied: Option<Ramp>,
    factor: f64,
}

/// The entry for output `name` on CRTC `id` after a reload, keeping state
/// from `previous` (the entries before the reload).
fn carry_over(previous: &mut HashMap<String, Crtc>, name: &str, id: u32) -> Crtc {
    match previous.remove(name) {
        // `base` is the ramp from before dimming: reading the current ramp
        // again would scale an already dimmed ramp a second time.
        Some(c) if c.id == id => c,
        Some(c) => Crtc { id, base: None, applied: None, factor: c.factor },
        None => Crtc { id, base: None, applied: None, factor: 1.0 },
    }
}

/// The entries after a reload, for `outputs` as (connector name, CRTC id).
///
/// The dim factor follows the output, but a gamma ramp belongs to the CRTC:
/// when another output now drives a CRTC we dimmed, that CRTC still holds
/// the dimmed ramp, so its undimmed `base` is carried over by CRTC id.
fn remap(mut previous: HashMap<String, Crtc>, outputs: Vec<(String, u32)>) -> HashMap<String, Crtc> {
    let mut ramps: HashMap<u32, (Option<Ramp>, Option<Ramp>)> =
        previous.values().map(|c| (c.id, (c.base.clone(), c.applied.clone()))).collect();
    let mut crtcs = HashMap::new();
    for (name, id) in outputs {
        let mut entry = carry_over(&mut previous, &name, id);
        let ramp = ramps.remove(&id);
        if entry.base.is_none() {
            if let Some((base, applied)) = ramp {
                entry.base = base;
                entry.applied = applied;
            }
        }
        crtcs.insert(name, entry);
    }
    crtcs
}

struct Inner {
    serial: u32,
    /// Keyed by connector name, e.g. "eDP-1".
    crtcs: HashMap<String, Crtc>,
    desired: HashMap<String, f64>,
}

#[derive(Clone)]
struct Api {
    conn: gio::DBusConnection,
    compositor: Compositor,
    inner: Rc<RefCell<Inner>>,
}

pub struct DisplayConfigDimmer {
    api: Api,
    _signal: gio::SignalSubscription,
}

impl DisplayConfigDimmer {
    /// Tries Mutter, then Muffin. Returns `None` if neither is running.
    pub fn start() -> Option<Rc<Self>> {
        let conn = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).ok()?;
        let api = [Compositor::Mutter, Compositor::Muffin].into_iter().find_map(|compositor| {
            let api = Api {
                conn: conn.clone(),
                compositor,
                inner: Rc::new(RefCell::new(Inner { serial: 0, crtcs: HashMap::new(), desired: HashMap::new() })),
            };
            api.reload().ok().map(|_| api)
        })?;

        let marker = api.compositor.marker();
        if marker.exists() {
            api.restore_linear();
            let _ = std::fs::remove_file(marker);
        }

        let signal_api = api.clone();
        let signal = conn.subscribe_to_signal(
            Some(api.compositor.bus_name()),
            Some(api.compositor.bus_name()),
            Some("MonitorsChanged"),
            Some(api.compositor.path()),
            None,
            gio::DBusSignalFlags::NONE,
            move |_| {
                if signal_api.reload().is_ok() {
                    signal_api.reapply_all();
                }
            },
        );

        let dimmer = Rc::new(Self { api, _signal: signal });
        let weak = Rc::downgrade(&dimmer);
        glib::timeout_add_local(Duration::from_secs(5), move || {
            let Some(d) = weak.upgrade() else { return glib::ControlFlow::Break };
            d.api.reassert();
            glib::ControlFlow::Continue
        });
        Some(dimmer)
    }

    pub fn compositor(&self) -> Compositor {
        self.api.compositor
    }

    pub fn outputs(&self) -> Vec<String> {
        self.api.inner.borrow().crtcs.keys().cloned().collect()
    }

    pub fn set(&self, target: &str, factor: f64) {
        self.api.set(target, factor);
    }

    pub fn shutdown(&self) {
        self.api.set("*", 1.0);
    }
}

impl Api {
    fn call(&self, method: &str, args: Option<&glib::Variant>) -> Result<glib::Variant, glib::Error> {
        self.conn.call_sync(
            Some(self.compositor.bus_name()),
            self.compositor.path(),
            self.compositor.bus_name(),
            method,
            args,
            None,
            gio::DBusCallFlags::NONE,
            2000,
            gio::Cancellable::NONE,
        )
    }

    /// Re-reads the CRTC/output mapping from GetResources.
    fn reload(&self) -> Result<(), glib::Error> {
        let res = self.call("GetResources", None)?;
        let serial: u32 = res.child_value(0).get().unwrap_or(0);
        let outputs = res.child_value(2);
        let mut assigned = Vec::new();
        for i in 0..outputs.n_children() {
            // (u id, x winsys_id, i current_crtc, au possible_crtcs, s name, ...)
            let output = outputs.child_value(i);
            let crtc: i32 = output.child_value(2).get().unwrap_or(-1);
            let name: String = output.child_value(4).get().unwrap_or_default();
            if crtc < 0 || name.is_empty() {
                continue;
            }
            assigned.push((name, crtc as u32));
        }
        let mut inner = self.inner.borrow_mut();
        let previous = std::mem::take(&mut inner.crtcs);
        inner.crtcs = remap(previous, assigned);
        inner.serial = serial;
        Ok(())
    }

    fn get_gamma(&self, serial: u32, crtc: u32) -> Option<Ramp> {
        let v = self.call("GetCrtcGamma", Some(&(serial, crtc).to_variant())).ok()?;
        let (r, g, b): (Vec<u16>, Vec<u16>, Vec<u16>) = v.get()?;
        (!r.is_empty()).then_some([r, g, b])
    }

    fn set_gamma(&self, serial: u32, crtc: u32, ramp: &Ramp) -> Result<(), glib::Error> {
        let args = (serial, crtc, ramp[0].clone(), ramp[1].clone(), ramp[2].clone()).to_variant();
        self.call("SetCrtcGamma", Some(&args)).map(|_| ())
    }

    fn restore_linear(&self) {
        let (serial, ids) = {
            let inner = self.inner.borrow();
            (inner.serial, inner.crtcs.values().map(|c| c.id).collect::<Vec<_>>())
        };
        for id in ids {
            if let Some(current) = self.get_gamma(serial, id) {
                let n = current[0].len();
                let linear: Vec<u16> = (0..n).map(|i| (i * 65535 / (n - 1).max(1)) as u16).collect();
                let _ = self.set_gamma(serial, id, &[linear.clone(), linear.clone(), linear]);
            }
        }
    }

    fn apply(&self, name: &str) {
        let serial = self.inner.borrow().serial;
        let (id, factor, base) = {
            let inner = self.inner.borrow();
            let Some(c) = inner.crtcs.get(name) else { return };
            (c.id, c.factor, c.base.clone())
        };

        if factor >= 0.999 {
            if let Some(base) = base {
                let _ = self.set_gamma(serial, id, &base);
            }
            if let Some(c) = self.inner.borrow_mut().crtcs.get_mut(name) {
                c.base = None;
                c.applied = None;
            }
            self.update_marker();
            return;
        }

        let Some(base) = base.or_else(|| self.get_gamma(serial, id)) else { return };
        let scaled: Ramp = [0, 1, 2].map(|ch| base[ch].iter().map(|&v| (v as f64 * factor).round() as u16).collect());
        if let Err(e) = self.set_gamma(serial, id, &scaled) {
            eprintln!("luminance: SetCrtcGamma failed: {e}");
            return;
        }
        if let Some(c) = self.inner.borrow_mut().crtcs.get_mut(name) {
            c.base = Some(base);
            c.applied = Some(scaled);
        }
        self.update_marker();
    }

    fn update_marker(&self) {
        let marker = self.compositor.marker();
        if self.inner.borrow().crtcs.values().any(|c| c.factor < 0.999) {
            let _ = std::fs::create_dir_all(crate::settings::config_dir());
            let _ = std::fs::write(marker, b"1");
        } else {
            let _ = std::fs::remove_file(marker);
        }
    }

    fn reapply_all(&self) {
        let names: Vec<String> = {
            let mut inner = self.inner.borrow_mut();
            let desired = inner.desired.clone();
            for (name, crtc) in inner.crtcs.iter_mut() {
                crtc.factor = desired.get(name).or_else(|| desired.get("*")).copied().unwrap_or(1.0);
            }
            inner.crtcs.keys().cloned().collect()
        };
        for name in names {
            self.apply(&name);
        }
    }

    /// If something else (Night Light) replaced our ramp, adopt it as the new base.
    fn reassert(&self) {
        let serial = self.inner.borrow().serial;
        let targets: Vec<(String, u32, Ramp)> = self
            .inner
            .borrow()
            .crtcs
            .iter()
            .filter(|(_, c)| c.factor < 0.999)
            .filter_map(|(n, c)| c.applied.clone().map(|a| (n.clone(), c.id, a)))
            .collect();
        for (name, id, applied) in targets {
            if let Some(current) = self.get_gamma(serial, id) {
                if current != applied {
                    if let Some(c) = self.inner.borrow_mut().crtcs.get_mut(&name) {
                        c.base = Some(current);
                    }
                    self.apply(&name);
                }
            }
        }
    }

    fn set(&self, target: &str, factor: f64) {
        let names: Vec<String> = {
            let mut inner = self.inner.borrow_mut();
            if target == "*" {
                inner.desired.clear();
            }
            inner.desired.insert(target.to_string(), factor);
            inner
                .crtcs
                .iter_mut()
                .filter(|(n, c)| (target == "*" || n.as_str() == target) && (c.factor - factor).abs() > f64::EPSILON)
                .map(|(n, c)| {
                    c.factor = factor;
                    n.clone()
                })
                .collect()
        };
        for name in names {
            self.apply(&name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(v: u16) -> Ramp {
        [vec![0, v], vec![0, v], vec![0, v]]
    }

    #[test]
    fn reload_keeps_the_undimmed_ramp() {
        let mut previous = HashMap::new();
        previous.insert(
            "eDP-1".to_string(),
            Crtc { id: 7, base: Some(ramp(65535)), applied: Some(ramp(32768)), factor: 0.5 },
        );
        previous.insert(
            "DP-1".to_string(),
            Crtc { id: 8, base: Some(ramp(60000)), applied: Some(ramp(30000)), factor: 0.5 },
        );
        // Same CRTC: the original ramp stays the base, so re-applying does
        // not scale the already dimmed ramp a second time.
        let same = carry_over(&mut previous, "eDP-1", 7);
        assert_eq!((same.id, same.factor), (7, 0.5));
        assert_eq!(same.base, Some(ramp(65535)));
        assert_eq!(same.applied, Some(ramp(32768)));
        // Output moved to another CRTC: its ramp is read again.
        let moved = carry_over(&mut previous, "DP-1", 9);
        assert_eq!((moved.id, moved.factor), (9, 0.5));
        assert_eq!((moved.base, moved.applied), (None, None));
        // New output.
        let new = carry_over(&mut previous, "HDMI-1", 3);
        assert_eq!((new.id, new.factor, new.base, new.applied), (3, 1.0, None, None));
        assert!(previous.is_empty());
    }

    fn entry(id: u32, base: u16, factor: f64) -> Crtc {
        Crtc { id, base: Some(ramp(base)), applied: Some(ramp((base as f64 * factor) as u16)), factor }
    }

    fn reload(previous: Vec<(&str, Crtc)>, outputs: &[(&str, u32)]) -> HashMap<String, Crtc> {
        let previous = previous.into_iter().map(|(n, c)| (n.to_string(), c)).collect();
        remap(previous, outputs.iter().map(|&(n, id)| (n.to_string(), id)).collect())
    }

    #[test]
    fn swapped_crtcs_keep_their_undimmed_ramps() {
        let after = reload(
            vec![("eDP-1", entry(7, 65535, 0.5)), ("DP-1", entry(8, 60000, 0.5))],
            &[("eDP-1", 8), ("DP-1", 7)],
        );
        let edp = &after["eDP-1"];
        assert_eq!((edp.id, edp.factor, edp.base.clone()), (8, 0.5, Some(ramp(60000))));
        let dp = &after["DP-1"];
        assert_eq!((dp.id, dp.factor, dp.base.clone()), (7, 0.5, Some(ramp(65535))));
    }

    #[test]
    fn new_output_on_a_dimmed_crtc_gets_its_undimmed_ramp() {
        let after = reload(vec![("DP-1", entry(8, 60000, 0.5))], &[("HDMI-1", 8)]);
        let hdmi = &after["HDMI-1"];
        assert_eq!((hdmi.id, hdmi.factor, hdmi.base.clone()), (8, 1.0, Some(ramp(60000))));
        assert_eq!(after.len(), 1);
    }

    #[test]
    fn unused_crtcs_have_no_base() {
        let after = reload(
            vec![("eDP-1", entry(7, 65535, 0.5)), ("DP-1", entry(8, 60000, 0.3))],
            &[("eDP-1", 7), ("DP-1", 9), ("HDMI-1", 3)],
        );
        let edp = &after["eDP-1"];
        assert_eq!((edp.id, edp.factor, edp.base.clone(), edp.applied.clone()), (7, 0.5, Some(ramp(65535)), Some(ramp(32767))));
        let dp = &after["DP-1"];
        assert_eq!((dp.id, dp.factor, dp.base.clone(), dp.applied.clone()), (9, 0.3, None, None));
        let hdmi = &after["HDMI-1"];
        assert_eq!((hdmi.id, hdmi.factor, hdmi.base.clone(), hdmi.applied.clone()), (3, 1.0, None, None));
    }
}
