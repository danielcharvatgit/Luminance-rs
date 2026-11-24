//! Laptop lid state from UPower (system bus). Not available on Windows.

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

pub struct Lid {
    proxy: Option<gio::DBusProxy>,
}

impl Lid {
    pub fn new() -> Self {
        if cfg!(windows) {
            // No UPower; the lid option has no effect there.
            return Self { proxy: None };
        }
        let proxy = gio::DBusProxy::for_bus_sync(
            gio::BusType::System,
            gio::DBusProxyFlags::NONE,
            None,
            "org.freedesktop.UPower",
            "/org/freedesktop/UPower",
            "org.freedesktop.UPower",
            gio::Cancellable::NONE,
        )
        .ok();
        Self { proxy }
    }

    fn property(&self, name: &str) -> Option<bool> {
        self.proxy.as_ref()?.cached_property(name)?.get()
    }

    pub fn is_closed(&self) -> bool {
        self.property("LidIsPresent").unwrap_or(false) && self.property("LidIsClosed").unwrap_or(false)
    }

    pub fn connect_changed<F: Fn() + 'static>(&self, f: F) {
        if let Some(proxy) = &self.proxy {
            proxy.connect_local("g-properties-changed", false, move |args| {
                let changed: Option<glib::Variant> = args.get(1).and_then(|v| v.get().ok());
                if changed.is_some_and(|c| glib::VariantDict::new(Some(&c)).contains("LidIsClosed")) {
                    f();
                }
                None
            });
        }
    }
}
