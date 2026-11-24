// No console window for the GUI; the CLI attaches to the caller's console.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod app;
mod cli;
mod config;
mod controller;
mod dbus;
mod dim;
mod hw;
mod json;
mod lid;
mod settings;
mod ui;
#[cfg(windows)]
mod win32;

use std::process::ExitCode;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    let gapplication_arg = argv.iter().skip(1).all(|a| a.starts_with("--gapplication"));
    if argv.len() > 1 && !gapplication_arg {
        #[cfg(windows)]
        win32::attach_parent_console();
        return cli::run(&argv);
    }
    gtk::glib::set_application_name(config::APP_NAME);
    ExitCode::from(app::run().get())
}
