//! Command line interface, compatible with the original Luminance options,
//! plus xrandr-style options and an interactive terminal mode (`--cli`).
//!
//!  * [`args`]   parses and validates options into a [`args::Command`]
//!  * [`client`] runs it against the Luminance service or the hardware
//!  * [`tui`]    the interactive mode

mod args;
mod client;
mod tui;

use std::process::ExitCode;

use args::{Adjust, Change, Command, Invocation, ListFormat, HELP};
use client::{Backend, Monitor};

use crate::config::{APP_NAME, VERSION};
use crate::json::Value;

fn list(monitors: &[Monitor], indices: &[usize], format: ListFormat) {
    if format == ListFormat::Json {
        let list: Vec<Value> = indices
            .iter()
            .map(|&i| {
                let m = &monitors[i];
                Value::object([
                    ("number", (i + 1).into()),
                    ("name", m.name.as_str().into()),
                    ("connector", m.connector.as_deref().into()),
                    ("kind", m.kind.as_str().into()),
                    ("brightness", m.brightness.into()),
                    ("contrast", m.contrast.into()),
                    ("software_dim", m.software_dim.into()),
                ])
            })
            .collect();
        println!("{}", Value::Array(list));
        return;
    }
    for &i in indices {
        let m = &monitors[i];
        let mut line = format!("Display {}: Label: {}, Brightness: {:.2}%", i + 1, m.name, m.brightness);
        if let Some(c) = m.contrast {
            line += &format!(", Contrast: {c:.2}%");
        }
        if format == ListFormat::Verbose {
            line += &format!(", Connector: {}, Control: {}", m.connector.as_deref().unwrap_or("unknown"), m.kind);
            if let Some(dim) = m.software_dim.filter(|d| *d < 0.999) {
                line += &format!(", Software dim: {:.0}%", dim * 100.0);
            }
        } else if m.kind == "software" {
            line += " (software dimming only)";
        }
        println!("{line}");
    }
}

/// Options for [`adjust`] that do not come from [`Adjust`].
#[derive(Clone, Copy)]
pub struct AdjustOpts {
    /// The displays were named by the user: unsupported ones are errors
    /// instead of being skipped.
    pub explicit: bool,
    pub dry_run: bool,
    pub report: bool,
}

fn report(opts: AdjustOpts, i: usize, m: &Monitor, what: &str, from: f64, to: f64) {
    if opts.dry_run {
        println!("Would set display {} ({}) {what}: {from:.2}% -> {to:.2}%", i + 1, m.label());
    } else if opts.report {
        println!("Display {} ({}) {what}: {from:.2}% -> {to:.2}%", i + 1, m.label());
    }
}

/// Applies brightness/contrast changes and updates `monitors` to the new values.
pub fn adjust(
    backend: &Backend,
    monitors: &mut [Monitor],
    indices: &[usize],
    brightness: Option<Change>,
    contrast: Option<Change>,
    linked: bool,
    opts: AdjustOpts,
) -> Result<(), String> {
    if let Some(change) = brightness {
        let direct = matches!(backend, Backend::Direct(_));
        let mut linked_value: Option<f64> = None;
        for &i in indices {
            if direct && !opts.explicit && monitors[i].kind == "software" {
                continue; // nothing to control without the service
            }
            let from = monitors[i].brightness;
            let to = match linked_value {
                Some(v) if linked => v,
                _ => change.apply(from),
            };
            linked_value.get_or_insert(to);
            report(opts, i, &monitors[i], "brightness", from, to);
            if !opts.dry_run {
                backend.set_brightness(i, to).map_err(|e| format!("Display {}: {e}", i + 1))?;
                monitors[i].brightness = to;
            }
        }
    }

    if let Some(change) = contrast {
        let mut any = false;
        for &i in indices {
            let Some(from) = monitors[i].contrast else {
                if opts.explicit {
                    return Err(format!("Display {} does not support contrast control via DDC/CI.", i + 1));
                }
                continue;
            };
            let to = change.apply(from);
            report(opts, i, &monitors[i], "contrast", from, to);
            if !opts.dry_run {
                backend.set_contrast(i, to).map_err(|e| format!("Display {}: {e}", i + 1))?;
                monitors[i].contrast = Some(to);
            }
            any = true;
        }
        if !any {
            return Err("No DDC/CI displays with contrast control found.".into());
        }
    }
    Ok(())
}

fn execute(inv: Invocation) -> Result<(), String> {
    match &inv.command {
        Command::Help => {
            println!("{HELP}");
            return Ok(());
        }
        Command::Version => {
            println!("{APP_NAME} (luminance-rs) {VERSION}");
            return Ok(());
        }
        _ => {}
    }

    let backend = Backend::connect(!inv.current);
    let mut monitors = backend.monitors()?;
    if monitors.is_empty() {
        return Err("No displays found.".into());
    }

    match inv.command {
        Command::Help | Command::Version => unreachable!(),
        Command::List { targets, format } => list(&monitors, &client::resolve(&monitors, &targets)?, format),
        Command::Get { target, contrast } => {
            let i = client::resolve(&monitors, &[target])?[0];
            let value = if contrast {
                monitors[i]
                    .contrast
                    .ok_or(format!("Display {} does not support contrast control via DDC/CI.", i + 1))?
            } else {
                monitors[i].brightness
            };
            println!("{value:.2}%");
        }
        Command::Adjust(Adjust { targets, brightness, contrast, linked }) => {
            let indices = client::resolve(&monitors, &targets)?;
            let opts = AdjustOpts { explicit: !targets.is_empty(), dry_run: inv.dry_run, report: inv.verbose };
            adjust(&backend, &mut monitors, &indices, brightness, contrast, linked, opts)?;
        }
        Command::Interactive { targets } => {
            let indices = client::resolve(&monitors, &targets)?;
            tui::run(&backend, monitors, (!targets.is_empty()).then_some(indices), inv.dry_run)?;
        }
    }
    Ok(())
}

/// Entry point for any invocation with arguments (`argv[0]` is skipped).
/// Exit status: 0 success, 1 failure, 2 invalid usage.
pub fn run(argv: &[String]) -> ExitCode {
    let inv = match args::parse(&argv[1..]) {
        Ok(inv) => inv,
        Err(e) => {
            eprintln!("luminance-rs: {e}");
            eprintln!("Try 'luminance-rs --help' for more information.");
            return ExitCode::from(2);
        }
    };
    match execute(inv) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("luminance-rs: {e}");
            ExitCode::FAILURE
        }
    }
}
