//! Main window.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::config::{APP_ID, APP_NAME, DESCRIPTION, VERSION};
use crate::controller::{Controller, Event};
use crate::hw::Issue;

struct Row {
    scale: gtk::Scale,
    handler: glib::SignalHandlerId,
    value: gtk::Label,
    zone: f64,
    contrast: Option<(gtk::Scale, glib::SignalHandlerId, gtk::Box)>,
}

struct Ui {
    alive: Cell<bool>,
    stack: gtk::Stack,
    content: gtk::Box,
    banner: adw::Banner,
    rows: RefCell<Vec<Row>>,
}

fn level_text(level: f64, zone: f64) -> String {
    if zone > 0.0 && zone < 100.0 && level < zone {
        format!("{level:.0}% · dimmed")
    } else {
        format!("{level:.0}%")
    }
}

fn set_scale_value(scale: &gtk::Scale, handler: &glib::SignalHandlerId, value: f64) {
    if (scale.value() - value).abs() > 0.001 {
        scale.block_signal(handler);
        scale.set_value(value);
        scale.unblock_signal(handler);
    }
}

fn icon(name: &str, size: i32) -> gtk::Image {
    let image = gtk::Image::from_icon_name(name);
    image.set_pixel_size(size);
    image
}

fn build_menu() -> gio::Menu {
    let menu = gio::Menu::new();

    let dimming = gio::Menu::new();
    dimming.append(Some("Extra Dimming Below Minimum"), Some("app.extra-dim"));
    let floor = gio::Menu::new();
    for (label, value) in [("5 %", "0.05"), ("10 %", "0.10"), ("20 %", "0.20"), ("30 %", "0.30"), ("50 %", "0.50")] {
        floor.append(Some(label), Some(&format!("app.dim-floor::{value}")));
    }
    dimming.append_submenu(Some("Darkest Level"), &floor);
    let method = gio::Menu::new();
    method.append(Some("Automatic"), Some("app.dim-method::auto"));
    method.append(Some("Gamma Ramp"), Some("app.dim-method::gamma"));
    method.append(Some("Overlay"), Some("app.dim-method::overlay"));
    dimming.append_submenu(Some("Dimming Method"), &method);
    menu.append_section(None, &dimming);

    let view = gio::Menu::new();
    view.append(Some("Hide Contrast Controls"), Some("app.hide-contrast"));
    // The lid state comes from UPower, which Windows does not have.
    if cfg!(not(windows)) {
        view.append(Some("Hide Built-in Display When Lid Is Closed"), Some("app.hide-internal"));
    }
    menu.append_section(None, &view);

    let other = gio::Menu::new();
    other.append(Some(if cfg!(windows) { "Help" } else { "Permissions Help" }), Some("app.permissions"));
    other.append(Some("About Luminance"), Some("app.about"));
    other.append(Some("Quit"), Some("app.quit"));
    menu.append_section(None, &other);
    menu
}

pub fn build_window(app: &adw::Application, ctrl: &Rc<Controller>) -> adw::ApplicationWindow {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(APP_NAME)
        .default_width(460)
        .default_height(420)
        .build();

    let header = adw::HeaderBar::new();
    let refresh = gtk::Button::from_icon_name("view-refresh-symbolic");
    refresh.set_tooltip_text(Some("Refresh Displays"));
    refresh.set_action_name(Some("app.refresh"));
    header.pack_start(&refresh);
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&build_menu())
        .primary(true)
        .tooltip_text("Main Menu")
        .build();
    header.pack_end(&menu_button);

    let banner = adw::Banner::new("");
    banner.set_button_label(Some("Help"));
    banner.set_action_name(Some("app.permissions"));

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.add_top_bar(&banner);

    let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build();

    let loading = gtk::Box::new(gtk::Orientation::Vertical, 12);
    loading.set_valign(gtk::Align::Center);
    let spinner = adw::Spinner::new();
    spinner.set_size_request(32, 32);
    loading.append(&spinner);
    let loading_label = gtk::Label::new(Some("Detecting displays…"));
    loading_label.add_css_class("dim-label");
    loading.append(&loading_label);
    stack.add_named(&loading, Some("loading"));

    let empty = adw::StatusPage::builder()
        .icon_name("video-display-symbolic")
        .title("No Displays Found")
        .description("No display with controllable brightness was detected.")
        .build();
    let help = gtk::Button::builder()
        .label(if cfg!(windows) { "Help" } else { "Permissions Help" })
        .action_name("app.permissions")
        .halign(gtk::Align::Center)
        .build();
    help.add_css_class("pill");
    empty.set_child(Some(&help));
    stack.add_named(&empty, Some("empty"));

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(12);
    content.set_margin_bottom(18);
    content.set_margin_start(12);
    content.set_margin_end(12);
    let clamp = adw::Clamp::builder().maximum_size(640).child(&content).build();
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .child(&clamp)
        .build();
    stack.add_named(&scrolled, Some("displays"));

    toolbar.set_content(Some(&stack));
    window.set_content(Some(&toolbar));

    let ui = Rc::new(Ui { alive: Cell::new(true), stack, content, banner, rows: RefCell::new(Vec::new()) });
    rebuild(&ui, ctrl);

    let weak_ctrl = Rc::downgrade(ctrl);
    let ui_ref = ui.clone();
    ctrl.subscribe(move |event| {
        let (Some(ctrl), true) = (weak_ctrl.upgrade(), ui_ref.alive.get()) else { return };
        match event {
            Event::Loading | Event::Loaded | Event::Settings => rebuild(&ui_ref, &ctrl),
            Event::Level(i, level) => {
                if let Some(row) = ui_ref.rows.borrow().get(*i) {
                    set_scale_value(&row.scale, &row.handler, *level);
                    row.value.set_text(&level_text(*level, row.zone));
                }
            }
            Event::Contrast(i, value) => {
                if let Some((scale, handler, _)) = ui_ref.rows.borrow().get(*i).and_then(|r| r.contrast.as_ref()) {
                    set_scale_value(scale, handler, *value);
                }
            }
        }
    });

    let ui_ref = ui.clone();
    let weak_ctrl = Rc::downgrade(ctrl);
    window.connect_close_request(move |window| {
        ui_ref.alive.set(false);
        if let (Some(ctrl), Some(app)) = (weak_ctrl.upgrade(), window.application()) {
            // Only with D-Bus does the app stay in the background (see app.rs).
            if ctrl.any_dimmed() && app.dbus_connection().is_some() {
                let n = gio::Notification::new("Luminance keeps running");
                n.set_body(Some(
                    "Software dimming stays active in the background. Open Luminance again to change it, \
                     or choose Quit from its menu to restore normal brightness.",
                ));
                app.send_notification(Some("background"), &n);
            }
        }
        glib::Propagation::Proceed
    });

    window
}

fn rebuild(ui: &Ui, ctrl: &Rc<Controller>) {
    update_banner(ui, ctrl);
    if ctrl.is_loading() {
        ui.stack.set_visible_child_name("loading");
        return;
    }
    while let Some(child) = ui.content.first_child() {
        ui.content.remove(&child);
    }
    ui.rows.borrow_mut().clear();

    let displays = ctrl.displays();
    if displays.is_empty() {
        ui.stack.set_visible_child_name("empty");
        return;
    }
    let settings = ctrl.settings();

    for (index, d) in displays.iter().enumerate() {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
        card.add_css_class("card");
        let inner = gtk::Box::new(gtk::Orientation::Vertical, 6);
        inner.set_margin_top(12);
        inner.set_margin_bottom(12);
        inner.set_margin_start(12);
        inner.set_margin_end(12);
        card.append(&inner);

        let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        head.append(&icon(if d.info.internal { "computer-symbolic" } else { "video-display-symbolic" }, 32));
        let titles = gtk::Box::new(gtk::Orientation::Vertical, 2);
        titles.set_hexpand(true);
        titles.set_valign(gtk::Align::Center);
        let title = gtk::Label::builder()
            .label(d.info.name.as_str())
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        title.add_css_class("heading");
        let subtitle = gtk::Label::builder()
            .label(d.info.kind_label())
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        subtitle.add_css_class("dim-label");
        subtitle.add_css_class("caption");
        titles.append(&title);
        titles.append(&subtitle);
        head.append(&titles);
        let zone = ctrl.dim_zone(&d.info);
        let value = gtk::Label::new(Some(&level_text(d.level, zone)));
        value.add_css_class("numeric");
        value.add_css_class("dim-label");
        head.append(&value);
        inner.append(&head);

        let brightness_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let brightness_icon = icon("display-brightness-symbolic", 16);
        brightness_icon.set_opacity(0.6);
        brightness_row.append(&brightness_icon);
        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
        scale.set_draw_value(false);
        scale.set_hexpand(true);
        scale.set_value(d.level);
        if zone > 0.0 && zone < 100.0 {
            scale.add_mark(zone, gtk::PositionType::Bottom, None);
            scale.set_tooltip_text(Some("Left of the mark the screen is dimmed in software, below the hardware minimum"));
        }
        let weak = Rc::downgrade(ctrl);
        let handler = scale.connect_value_changed(move |s| {
            if let Some(c) = weak.upgrade() {
                c.set_level_linked(index, s.value().round(), false);
            }
        });
        brightness_row.append(&scale);
        inner.append(&brightness_row);

        let contrast = d.info.contrast.map(|c| {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            let contrast_icon = icon("weather-clear-night-symbolic", 16);
            contrast_icon.set_opacity(0.6);
            contrast_icon.set_tooltip_text(Some("Contrast"));
            row.append(&contrast_icon);
            let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
            scale.set_draw_value(false);
            scale.set_hexpand(true);
            scale.set_value(c);
            let weak = Rc::downgrade(ctrl);
            let handler = scale.connect_value_changed(move |s| {
                if let Some(c) = weak.upgrade() {
                    c.set_contrast(index, s.value().round(), false);
                }
            });
            row.append(&scale);
            row.set_visible(!settings.contrast_hidden);
            inner.append(&row);
            (scale, handler, row)
        });

        ui.content.append(&card);
        ui.rows.borrow_mut().push(Row { scale, handler, value, zone, contrast });
    }

    if displays.len() > 1 {
        let link = gtk::CheckButton::builder()
            .label("Link brightness of all displays")
            .action_name("app.link")
            .build();
        ui.content.append(&link);
    }

    let footer = gtk::Label::builder()
        .label(if ctrl.dimmer.available() {
            format!("Software dimming: {}", ctrl.dimmer.description())
        } else {
            "Software dimming is not available in this session".to_string()
        })
        .wrap(true)
        .build();
    footer.add_css_class("dim-label");
    footer.add_css_class("caption");
    ui.content.append(&footer);

    ui.stack.set_visible_child_name("displays");
}

fn update_banner(ui: &Ui, ctrl: &Controller) {
    let issues = ctrl.issues();
    let text = if issues.contains(&Issue::I2cPermissionDenied) {
        Some("No permission to control external monitors (DDC/CI)")
    } else if issues.contains(&Issue::I2cUnavailable) {
        Some("External monitors: i2c interface is not available")
    } else {
        None
    };
    ui.banner.set_title(text.unwrap_or(""));
    ui.banner.set_revealed(text.is_some());
}

pub fn show_about(parent: &impl IsA<gtk::Widget>) {
    let about = adw::AboutDialog::builder()
        .application_name(APP_NAME)
        .application_icon(APP_ID)
        .version(VERSION)
        .comments(DESCRIPTION)
        .developer_name("Luminance-rs contributors")
        .license_type(gtk::License::Gpl30Only)
        .build();
    about.add_acknowledgement_section(
        Some("Based on"),
        &["Luminance by Swapnil Devesh https://github.com/sidevesh/Luminance"],
    );
    about.present(Some(parent));
}

fn present_help(parent: &impl IsA<gtk::Widget>, heading: &str, body: &str) {
    let label = gtk::Label::builder().use_markup(true).label(body).wrap(true).selectable(true).xalign(0.0).build();
    let dialog = adw::AlertDialog::builder().heading(heading).extra_child(&label).build();
    dialog.add_response("close", "Close");
    dialog.set_default_response(Some("close"));
    dialog.present(Some(parent));
}

#[cfg(windows)]
pub fn show_permissions(parent: &impl IsA<gtk::Widget>, ctrl: &Controller) {
    let body = format!(
        "<b>Current session</b>
• Software dimming: {}

<b>Built-in displays</b>
Controlled through the display driver's brightness interface, or WMI when the driver has none. No setup or administrator rights are needed.

<b>External monitors (DDC/CI)</b>
Controlled through the Windows Monitor Configuration API. Enable DDC/CI in the monitor's on-screen menu. Some docks, KVM switches and DisplayLink adapters do not pass DDC/CI through; those monitors still get software dimming.

<b>Software dimming below the minimum</b>
• Gamma ramp: dims the whole picture. Windows only allows ramps close to neutral, so for deeper dimming Luminance switches to the overlay. Setting the DWORD <tt>GdiIcmGammaRange</tt> to <tt>256</tt> under <tt>HKLM\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\ICM</tt> lifts the limit.
• Overlay: a click-through dark window above everything else.
Closing the window restores normal brightness.",
        glib::markup_escape_text(&ctrl.dimmer.description()),
    );
    present_help(parent, "Help", &body);
}

#[cfg(not(windows))]
pub fn show_permissions(parent: &impl IsA<gtk::Widget>, ctrl: &Controller) {
    let i2c_buses = std::fs::read_dir("/dev")
        .map(|d| d.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("i2c-")).count())
        .unwrap_or(0);
    let flatpak = std::path::Path::new("/.flatpak-info").exists();
    let status = format!(
        "<b>Current session</b>\n• Running in Flatpak: {}\n• Software dimming: {}\n• i2c devices visible: {}\n",
        if flatpak { "yes" } else { "no" },
        glib::markup_escape_text(&ctrl.dimmer.description()),
        i2c_buses,
    );
    let body = format!(
        "{status}
<b>Built-in displays</b>
Work without setup through systemd-logind (system bus name <tt>org.freedesktop.login1</tt>, granted by default).

<b>External monitors (DDC/CI)</b>
1. Flatpak device access: in Flatseal enable <i>All devices</i> (granted by default).
2. Load the <tt>i2c-dev</tt> kernel module on the host:
<tt>sudo modprobe i2c-dev
echo i2c-dev | sudo tee /etc/modules-load.d/i2c-dev.conf</tt>
3. Give your user access to <tt>/dev/i2c-*</tt>: install your distribution's <tt>ddcutil</tt> package (its udev rule grants access to the logged-in user), or join the <tt>i2c</tt> group:
<tt>sudo usermod -aG i2c $USER</tt>
Also enable DDC/CI in the monitor's on-screen menu.

<b>Software dimming below the minimum</b>
• Hyprland, Sway and other wlroots compositors: Wayland gamma control.
• KDE Plasma: click-through overlay layer.
  Both need only the Wayland socket.
• GNOME: Mutter gamma, needs session bus access to <tt>org.gnome.Mutter.DisplayConfig</tt>.
• Cinnamon: Muffin gamma, needs session bus access to <tt>org.cinnamon.Muffin.DisplayConfig</tt>.
  Both are granted by default.
Another program holding the gamma ramp (e.g. wlsunset, gammastep) makes Luminance fall back to the overlay on Wayland."
    );
    present_help(parent, "Permissions", &body);
}
