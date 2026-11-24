//! Session bus service, compatible with the original Luminance interface
//! (GetMonitors / SetBrightness / SetBrightnessQuiet / SetContrast / Quit and
//! the ShowOSD / ShowContrastOSD signals), so OSD helpers such as the GNOME
//! Shell extension keep working after changing the bus name.

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use crate::config::{object_path, APP_ID};
use crate::controller::Controller;
use crate::json::Value;

fn introspection_xml() -> String {
    format!(
        r#"<node>
  <interface name="{APP_ID}">
    <signal name="ShowOSD"><arg type="d" name="brightness_percentage"/><arg type="s" name="monitor_name"/></signal>
    <signal name="ShowContrastOSD"><arg type="d" name="contrast_percentage"/><arg type="s" name="monitor_name"/></signal>
    <method name="GetMonitors"><arg type="s" name="monitors_json" direction="out"/></method>
    <method name="SetBrightness"><arg type="s" name="monitor_id" direction="in"/><arg type="d" name="value" direction="in"/></method>
    <method name="SetBrightnessQuiet"><arg type="s" name="monitor_id" direction="in"/><arg type="d" name="value" direction="in"/></method>
    <method name="SetContrast"><arg type="s" name="monitor_id" direction="in"/><arg type="d" name="value" direction="in"/></method>
    <method name="Refresh"/>
    <method name="Quit"/>
  </interface>
</node>"#
    )
}

pub fn monitors_json(controller: &Controller) -> String {
    let list: Vec<Value> = controller
        .displays()
        .iter()
        .enumerate()
        .map(|(i, d)| {
            Value::object([
                ("id", i.to_string().into()),
                ("name", d.info.name.as_str().into()),
                ("brightness", ((d.level * 100.0).round() / 100.0).into()),
                ("has_contrast", d.info.contrast.is_some().into()),
                ("contrast", d.info.contrast.map_or(-1.0, |c| (c * 100.0).round() / 100.0).into()),
                ("connector", d.info.connector.as_deref().into()),
                ("kind", d.info.control.kind().into()),
                ("software_dim", ((d.dim * 1000.0).round() / 1000.0).into()),
            ])
        })
        .collect();
    Value::Array(list).to_string()
}

type Job = Box<dyn FnOnce(&Rc<Controller>)>;

/// The exported service object. It is registered on the shared session bus
/// connection *before* `Application::run()`, i.e. before GApplication takes
/// the bus name: calls that triggered D-Bus activation are delivered as soon
/// as the name is owned, which is earlier than the `startup` signal.
pub struct Service {
    /// Newer GLib keeps the shared bus connection only weakly; holding it
    /// here makes GApplication reuse this connection (and our registration).
    connection: OnceCell<gio::DBusConnection>,
    controller: OnceCell<Rc<Controller>>,
    queued: RefCell<Vec<Job>>,
}

impl Service {
    pub fn export(app: &adw::Application) -> Rc<Self> {
        let service = Rc::new(Self {
            connection: OnceCell::new(),
            controller: OnceCell::new(),
            queued: RefCell::new(Vec::new()),
        });
        if cfg!(windows) {
            // There is no session bus on Windows; the CLI controls the
            // hardware directly there.
            return service;
        }
        let conn = match gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("luminance: no session bus: {e}");
                return service;
            }
        };
        let node = match gio::DBusNodeInfo::for_xml(&introspection_xml()) {
            Ok(n) => n,
            Err(e) => {
                eprintln!("luminance: bad introspection XML: {e}");
                return service;
            }
        };
        let Some(iface) = node.lookup_interface(APP_ID) else { return service };
        let _ = service.connection.set(conn.clone());

        let weak = Rc::downgrade(&service);
        let app_weak = app.downgrade();
        let result = conn
            .register_object(&object_path(), &iface)
            .method_call(move |_conn, _sender, _path, _iface, method, params, invocation| {
                let Some(service) = weak.upgrade() else { return };
                let app = app_weak.clone();
                let method = method.to_string();
                service.run(Box::new(move |ctrl| {
                    // Requests can arrive during the initial scan.
                    let ctrl2 = ctrl.clone();
                    ctrl.when_loaded(move || handle(&ctrl2, app.upgrade(), &method, params, invocation));
                }));
            })
            .build();
        if let Err(e) = result {
            eprintln!("luminance: failed to export D-Bus object: {e}");
        }

        let conn_weak = conn.downgrade();
        service.run(Box::new(move |ctrl| {
            ctrl.set_osd_handler(move |value, name, contrast| {
                let Some(conn) = conn_weak.upgrade() else { return };
                let signal = if contrast { "ShowContrastOSD" } else { "ShowOSD" };
                let _ = conn.emit_signal(None, &object_path(), APP_ID, signal, Some(&(value, name).to_variant()));
            });
        }));
        service
    }

    fn run(&self, job: Job) {
        match self.controller.get() {
            Some(ctrl) => job(ctrl),
            None => self.queued.borrow_mut().push(job),
        }
    }

    /// Called from `startup` once the controller exists.
    pub fn attach(&self, controller: &Rc<Controller>) {
        let _ = self.controller.set(controller.clone());
        let jobs: Vec<Job> = self.queued.borrow_mut().drain(..).collect();
        for job in jobs {
            job(controller);
        }
    }
}

/// Parses the `(sd)` arguments of SetBrightness/SetContrast into a display
/// index below `count` and a value.
fn parse_target(params: &glib::Variant, count: usize) -> Result<(usize, f64), String> {
    let (id, value): (String, f64) = params.get().ok_or("expected (sd)")?;
    let index: usize = id.parse().map_err(|_| format!("invalid monitor id {id}"))?;
    if index >= count {
        return Err(format!("monitor id {id} out of range"));
    }
    if !value.is_finite() {
        return Err(format!("invalid value {value}"));
    }
    Ok((index, value))
}

fn handle(
    ctrl: &Rc<Controller>,
    app: Option<adw::Application>,
    method: &str,
    params: glib::Variant,
    invocation: gio::DBusMethodInvocation,
) {
    let parse = |params: &glib::Variant| parse_target(params, ctrl.display_count());
    let fail = |invocation: gio::DBusMethodInvocation, msg: String| {
        invocation.return_error(gio::DBusError::InvalidArgs, &msg);
    };

    match method {
        "GetMonitors" => invocation.return_value(Some(&(monitors_json(ctrl),).to_variant())),
        "SetBrightness" | "SetBrightnessQuiet" => match parse(&params) {
            Ok((i, v)) => {
                ctrl.set_level(i, v, method == "SetBrightness");
                invocation.return_value(None);
            }
            Err(e) => fail(invocation, e),
        },
        "SetContrast" => match parse(&params) {
            Ok((i, v)) if ctrl.set_contrast(i, v, true) => invocation.return_value(None),
            Ok(_) => fail(invocation, "display does not support contrast".into()),
            Err(e) => fail(invocation, e),
        },
        "Refresh" => {
            ctrl.refresh();
            invocation.return_value(None);
        }
        "Quit" => {
            invocation.return_value(None);
            if let Some(app) = app {
                app.quit();
            }
        }
        _ => invocation.return_error(gio::DBusError::UnknownMethod, method),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_set_arguments() {
        let args = |id: &str, v: f64| (id.to_string(), v).to_variant();
        assert_eq!(parse_target(&args("1", 40.0), 2), Ok((1, 40.0)));
        // Out of 0-100 is clamped later; only the id must be valid here.
        assert_eq!(parse_target(&args("0", 150.0), 1), Ok((0, 150.0)));
        assert!(parse_target(&args("2", 40.0), 2).is_err());
        assert!(parse_target(&args("x", 40.0), 2).is_err());
        assert!(parse_target(&("0", 1u32).to_variant(), 2).is_err());
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(parse_target(&args("0", bad), 1).is_err(), "accepted {bad}");
        }
    }
}
