//! Interactive terminal mode (`--cli`): choose a display from a list, then
//! adjust it. Line based, so it also works over ssh and with piped input;
//! end of input (Ctrl+D) quits.

use std::io::{self, BufRead, Write};

use super::args::{Change, Target};
use super::client::{self, Backend, Monitor};
use super::AdjustOpts;

const ADJUST_HELP: &str = "  40     set brightness to 40%      +10 / -10   brighter / darker by 10%
  c 70   set contrast to 70%        c +5 / c -5 change contrast
  Enter  back to the display list   q           quit";

enum Next {
    Back,
    Quit,
}

/// Prints `prompt` and reads one trimmed line; `None` at end of input.
fn ask(prompt: &str) -> Option<String> {
    print!("{prompt}");
    let _ = io::stdout().flush();
    let mut line = String::new();
    match io::stdin().lock().read_line(&mut line) {
        Ok(0) | Err(_) => {
            println!();
            None
        }
        Ok(_) => Some(line.trim().to_string()),
    }
}

fn print_table(monitors: &[Monitor]) {
    let width = monitors.iter().map(|m| m.label().chars().count()).max().unwrap_or(0).max(7);
    println!("  #  {:<width$}  {:<9}  {:>10}  {:>8}", "Display", "Control", "Brightness", "Contrast");
    for (i, m) in monitors.iter().enumerate() {
        let contrast = m.contrast.map(|c| format!("{c:.0}%")).unwrap_or_else(|| "-".into());
        println!(
            "{:>3}  {:<width$}  {:<9}  {:>9.0}%  {:>8}",
            i + 1,
            m.label(),
            m.kind,
            m.brightness,
            contrast
        );
    }
}

fn adjust_loop(backend: &Backend, monitors: &mut [Monitor], indices: &[usize], dry_run: bool) -> Next {
    let title = match indices {
        [i] => monitors[*i].label(),
        _ if indices.len() == monitors.len() => "all displays".into(),
        _ => format!("{} displays", indices.len()),
    };
    println!("\nAdjusting {title}:\n{ADJUST_HELP}");
    // A display picked on its own must support what is asked; in a group,
    // displays without e.g. contrast are skipped.
    let opts = AdjustOpts { explicit: indices.len() == 1, dry_run, report: true };
    loop {
        let status: Vec<String> = indices.iter().map(|&i| format!("{:.0}%", monitors[i].brightness)).collect();
        let Some(input) = ask(&format!("{title} [{}]> ", status.join(", "))) else { return Next::Quit };
        let (contrast, value) = match input.as_str() {
            "" | "b" | "back" => return Next::Back,
            "q" | "quit" | "exit" => return Next::Quit,
            "?" | "h" | "help" => {
                println!("{ADJUST_HELP}");
                continue;
            }
            s => match s.strip_prefix('c') {
                Some(rest) => (true, rest.trim()),
                None => (false, s),
            },
        };
        let change = match Change::parse(value) {
            Ok(c) => c,
            Err(e) => {
                println!("{e}. Type ? for help.");
                continue;
            }
        };
        let (b, c) = if contrast { (None, Some(change)) } else { (Some(change), None) };
        if let Err(e) = super::adjust(backend, monitors, indices, b, c, false, opts) {
            println!("Error: {e}");
        }
    }
}

pub fn run(backend: &Backend, mut monitors: Vec<Monitor>, preselected: Option<Vec<usize>>, dry_run: bool) -> Result<(), String> {
    println!("Luminance-rs interactive mode ({}){}", backend.description(), if dry_run { ", dry run" } else { "" });
    if let Some(indices) = preselected {
        if let Next::Quit = adjust_loop(backend, &mut monitors, &indices, dry_run) {
            return Ok(());
        }
    }
    loop {
        println!();
        print_table(&monitors);
        let n = monitors.len();
        let range = if n == 1 { "1".to_string() } else { format!("1-{n}") };
        let Some(input) = ask(&format!("Select display [{range} or connector], a = all, r = reload, q = quit: ")) else {
            return Ok(());
        };
        let indices = match input.to_lowercase().as_str() {
            "q" | "quit" | "exit" => return Ok(()),
            "" => continue,
            "r" | "reload" => {
                match backend.monitors() {
                    Ok(m) if !m.is_empty() => monitors = m,
                    Ok(_) => println!("No displays found."),
                    Err(e) => println!("Error: {e}"),
                }
                continue;
            }
            "a" | "all" => (0..n).collect(),
            _ => {
                let picked = input
                    .split([',', ' '])
                    .filter(|s| !s.is_empty())
                    .map(Target::parse)
                    .collect::<Result<Vec<_>, _>>()
                    .and_then(|targets| client::resolve(&monitors, &targets));
                match picked {
                    Ok(indices) => indices,
                    Err(e) => {
                        println!("{e}");
                        continue;
                    }
                }
            }
        };
        if let Next::Quit = adjust_loop(backend, &mut monitors, &indices, dry_run) {
            return Ok(());
        }
    }
}
