//! GApplication setup: actions, D-Bus service, lifetime management.

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::config::APP_ID;
use crate::controller::{Controller, Event};
use crate::dim::Technique;
use crate::settings::{parse_dim_floor, Settings};
use crate::{dbus, ui};

fn bool_action(app: &adw::Application, ctrl: &Rc<Controller>, name: &str, initial: bool, apply: fn(&mut Settings, bool)) {
    let action = gio::SimpleAction::new_stateful(name, None, &initial.to_variant());
    let ctrl = ctrl.clone();
    action.connect_change_state(move |action, value| {
        let Some(value) = value.and_then(|v| v.get::<bool>()) else { return };
        action.set_state(&value.to_variant());
        ctrl.update_settings(|s| apply(s, value));
    });
    app.add_action(&action);
}

/// `apply` stores a value in the settings and `state` reads it back, so the
/// action state always shows what was stored (invalid values are ignored).
fn string_action(
    app: &adw::Application,
    ctrl: &Rc<Controller>,
    name: &str,
    apply: fn(&mut Settings, &str),
    state: fn(&Settings) -> String,
) {
    let action = gio::SimpleAction::new_stateful(name, Some(glib::VariantTy::STRING), &state(&ctrl.settings()).to_variant());
    let ctrl = ctrl.clone();
    action.connect_change_state(move |action, value| {
        let Some(value) = value.and_then(|v| v.get::<String>()) else { return };
        ctrl.update_settings(|s| apply(s, &value));
        action.set_state(&state(&ctrl.settings()).to_variant());
    });
    app.add_action(&action);
}

fn setup_actions(app: &adw::Application, ctrl: &Rc<Controller>) {
    let s = ctrl.settings();
    bool_action(app, ctrl, "link", s.brightness_linked, |s, v| s.brightness_linked = v);
    bool_action(app, ctrl, "extra-dim", s.extra_dimming, |s, v| s.extra_dimming = v);
    bool_action(app, ctrl, "hide-contrast", s.contrast_hidden, |s, v| s.contrast_hidden = v);
    bool_action(app, ctrl, "hide-internal", s.hide_internal_if_lid_closed, |s, v| s.hide_internal_if_lid_closed = v);
    string_action(
        app,
        ctrl,
        "dim-floor",
        |s, v| s.dim_floor = parse_dim_floor(v).unwrap_or(s.dim_floor),
        |s| format!("{:.2}", s.dim_floor),
    );
    string_action(app, ctrl, "dim-method", |s, v| s.technique = Technique::parse(v), |s| s.technique.as_str().into());
    if let Some(a) = app.lookup_action("dim-method").and_downcast::<gio::SimpleAction>() {
        a.set_enabled(ctrl.dimmer.supports_technique_choice());
    }

    let refresh = gio::SimpleAction::new("refresh", None);
    let c = ctrl.clone();
    refresh.connect_activate(move |_, _| c.refresh());
    app.add_action(&refresh);

    let about = gio::SimpleAction::new("about", None);
    let weak = app.downgrade();
    about.connect_activate(move |_, _| {
        if let Some(w) = weak.upgrade().and_then(|a| a.active_window()) {
            ui::show_about(&w);
        }
    });
    app.add_action(&about);

    let permissions = gio::SimpleAction::new("permissions", None);
    let weak = app.downgrade();
    let c = ctrl.clone();
    permissions.connect_activate(move |_, _| {
        if let Some(w) = weak.upgrade().and_then(|a| a.active_window()) {
            ui::show_permissions(&w, &c);
        }
    });
    app.add_action(&permissions);

    let quit = gio::SimpleAction::new("quit", None);
    let weak = app.downgrade();
    quit.connect_activate(move |_, _| {
        if let Some(app) = weak.upgrade() {
            app.quit();
        }
    });
    app.add_action(&quit);
    app.set_accels_for_action("app.quit", &["<Ctrl>q"]);
    app.set_accels_for_action("app.refresh", &["F5", "<Ctrl>r"]);
}

pub fn run() -> glib::ExitCode {
    // GApplication's single instance needs D-Bus; on Windows a second start
    // just brings the running window to the front.
    #[cfg(windows)]
    if crate::win32::activate_running_instance(APP_ID, crate::config::APP_NAME) {
        return glib::ExitCode::SUCCESS;
    }

    let app = adw::Application::builder().application_id(APP_ID).build();
    let controller: Rc<OnceCell<Rc<Controller>>> = Rc::new(OnceCell::new());
    // Keeps the process alive while software dimming is active, because
    // Wayland-based dimming disappears together with the process.
    let hold: Rc<RefCell<Option<gio::ApplicationHoldGuard>>> = Rc::new(RefCell::new(None));

    let service = dbus::Service::export(&app);

    let cell = controller.clone();
    app.connect_startup(move |app| {
        let ctrl = Controller::new();
        service.attach(&ctrl);
        setup_actions(app, &ctrl);
        // When started by D-Bus activation (CLI, OSD extension), linger briefly.
        app.set_inactivity_timeout(10_000);

        let weak_app = app.downgrade();
        let weak_ctrl = Rc::downgrade(&ctrl);
        let hold = hold.clone();
        ctrl.subscribe(move |event| {
            if !matches!(event, Event::Level(..) | Event::Loaded) {
                return;
            }
            let (Some(app), Some(ctrl)) = (weak_app.upgrade(), weak_ctrl.upgrade()) else { return };
            let dimmed = ctrl.any_dimmed();
            let mut hold = hold.borrow_mut();
            // Without a bus (Windows) nothing could reach a windowless
            // instance, so it quits with its window and restores brightness.
            if dimmed && hold.is_none() && app.dbus_connection().is_some() {
                *hold = Some(app.hold());
            } else if !dimmed && hold.is_some() {
                *hold = None;
            }
        });

        ctrl.refresh();
        let _ = cell.set(ctrl);
    });

    let cell = controller.clone();
    app.connect_activate(move |app| {
        let Some(ctrl) = cell.get() else { return };
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        ui::build_window(app, ctrl).present();
    });

    let cell = controller.clone();
    app.connect_shutdown(move |_| {
        if let Some(ctrl) = cell.get() {
            ctrl.shutdown();
            // Give the compositor a moment to process the reset before exit.
            std::thread::sleep(Duration::from_millis(50));
        }
    });

    app.run()
}
