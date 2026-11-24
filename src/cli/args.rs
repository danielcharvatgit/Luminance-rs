//! Command line parsing and validation. Pure: no I/O, so it is fully unit tested.
//!
//! Two option styles are accepted and can be mixed:
//!  * the original Luminance options (`-s [DISPLAY] -p 40`, `-g 1`, ...);
//!  * xrandr-style options (`--output DP-2 --brightness 40`, `--listmonitors`, ...).
//!
//! Both are turned into the same [`Command`].

use std::fmt;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Op {
    Set,
    Increase,
    Decrease,
}

/// A brightness or contrast change in percent.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Change {
    pub op: Op,
    pub value: f64,
}

impl Change {
    pub fn apply(self, current: f64) -> f64 {
        match self.op {
            Op::Set => self.value,
            Op::Increase => current + self.value,
            Op::Decrease => current - self.value,
        }
        .clamp(0.0, 100.0)
    }

    /// Parses "40", "40%", "+10" or "-10%".
    pub fn parse(s: &str) -> Result<Self, String> {
        let (op, rest) = match s.as_bytes().first() {
            Some(b'+') => (Op::Increase, &s[1..]),
            Some(b'-') => (Op::Decrease, &s[1..]),
            _ => (Op::Set, s),
        };
        Ok(Self { op, value: percentage(rest).map_err(|_| format!("Invalid value: '{s}' (expected 0-100, +N or -N)"))? })
    }
}

/// Selects a display by its number (as listed, 1-based) or by connector/name.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Target {
    Number(usize),
    Name(String),
}

impl Target {
    pub fn parse(s: &str) -> Result<Self, String> {
        if s.is_empty() {
            return Err("Display must not be empty".into());
        }
        if s.bytes().all(|b| b.is_ascii_digit()) {
            return match s.parse::<usize>() {
                Ok(n) if n >= 1 => Ok(Target::Number(n)),
                _ => Err(format!("Invalid display number: {s}")),
            };
        }
        Ok(Target::Name(s.to_string()))
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Target::Number(n) => write!(f, "{n}"),
            Target::Name(s) => write!(f, "{s}"),
        }
    }
}

/// Displays to act on; empty means all of them.
pub type Selection = Vec<Target>;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ListFormat {
    #[default]
    Plain,
    Verbose,
    Json,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Adjust {
    pub targets: Selection,
    pub brightness: Option<Change>,
    pub contrast: Option<Change>,
    /// Original Luminance behaviour for relative changes of all displays:
    /// every display ends up on the value computed from the first one.
    pub linked: bool,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Command {
    Help,
    Version,
    List { targets: Selection, format: ListFormat },
    Get { target: Target, contrast: bool },
    Adjust(Adjust),
    Interactive { targets: Selection },
}

/// A fully validated invocation.
#[derive(Clone, PartialEq, Debug)]
pub struct Invocation {
    pub command: Command,
    pub dry_run: bool,
    pub verbose: bool,
    /// Use the service's cached values instead of rescanning the hardware.
    pub current: bool,
}

pub const HELP: &str = "\
Usage: luminance-rs [OPTIONS]
Control brightness of built-in and external (DDC/CI) displays.
Without options the graphical application starts.

Modes:
      --cli                        Interactive terminal interface: pick a display
                                     from a list and adjust it
      --noninteractive             Only run the given options; never prompt and
                                     never start the GUI (error if nothing to do)
  -h, --help                       Show this help
  -v, --version                    Show the version

Selecting displays (DISPLAY is a number from --list-displays, a connector
such as eDP-1 or DP-2 (DISPLAY1 on Windows), or the monitor name;
case-insensitive):
      --output DISPLAY             Act on this display; may be repeated
      --all                        Act on all displays (default)

Brightness and contrast (VALUE: 40 or 40% sets, +10 increases, -10 decreases):
      --brightness VALUE           Set or change brightness of the selected displays
      --contrast VALUE             Set or change contrast (DDC/CI displays only)

Listing:
  -l, --list-displays              List displays and their brightness
  -q, --query, --listmonitors      Same as --list-displays
      --verbose                    More detail in listings; report each change made
      --json                       List displays as JSON
      --current                    Use the running service's cached values
                                     instead of re-reading the hardware

Other:
      --dryrun, --dry-run          Show what would change without changing it

Original Luminance options:
  -g, --get-percentage DISPLAY     Print the brightness percentage of a display
      --get-contrast DISPLAY       Print the contrast percentage of a DDC/CI display
  -s, --set-brightness [DISPLAY]   Set brightness to the -p value
  -i, --increase-brightness [DISPLAY]
                                   Increase brightness by the -p value
  -d, --decrease-brightness [DISPLAY]
                                   Decrease brightness by the -p value
      --set-contrast [DISPLAY]     Set contrast to the -p value
      --increase-contrast [DISPLAY]
                                   Increase contrast by the -p value
      --decrease-contrast [DISPLAY]
                                   Decrease contrast by the -p value
  -p, --percentage PERCENTAGE      Value (0-100) for the options above
  -o, --show-osd PROVIDER          Accepted for compatibility; OSD signals are always sent
  If DISPLAY is omitted, all displays are changed.

With extra dimming enabled (default), brightness values below 25 dim the screen
in software below the hardware minimum. The screen never goes fully black.

Examples:
  luminance-rs --cli
  luminance-rs --noninteractive --output DP-2 --brightness 30
  luminance-rs --output eDP-1 --brightness -10
  luminance-rs -s 1 -p 50";

fn percentage(s: &str) -> Result<f64, String> {
    let number = s.strip_suffix('%').unwrap_or(s);
    // f64 parsing accepts a sign, "inf" and "nan"; only plain numbers are wanted.
    if number.starts_with(['+', '-']) {
        return Err(format!("Invalid percentage: '{s}'"));
    }
    let v: f64 = number.parse().map_err(|_| format!("Invalid percentage: '{s}'"))?;
    if !v.is_finite() || !(0.0..=100.0).contains(&v) {
        return Err(format!("Percentage out of range (0-100): '{s}'"));
    }
    Ok(v)
}

/// Raw options as given; turned into an [`Invocation`] by [`validate`].
#[derive(Default, Debug)]
struct Raw {
    help: bool,
    version: bool,
    cli: bool,
    noninteractive: bool,
    list: bool,
    verbose: bool,
    json: bool,
    current: bool,
    dry_run: bool,
    all: bool,
    outputs: Vec<Target>,
    get: Option<Target>,
    get_contrast: Option<Target>,
    brightness: Option<Change>,
    contrast: Option<Change>,
    legacy_brightness: Option<(Op, Option<Target>)>,
    legacy_contrast: Option<(Op, Option<Target>)>,
    percentage: Option<f64>,
}

fn set_once<T>(slot: &mut Option<T>, value: T, name: &str) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("Option {name} given more than once"));
    }
    *slot = Some(value);
    Ok(())
}

fn flag(slot: &mut bool, name: &str) -> Result<(), String> {
    if std::mem::replace(slot, true) {
        return Err(format!("Option {name} given more than once"));
    }
    Ok(())
}

fn parse_raw(args: &[String]) -> Result<Raw, String> {
    let mut raw = Raw::default();
    let mut it = args.iter().peekable();

    while let Some(arg) = it.next() {
        if !arg.starts_with('-') || arg == "-" {
            return Err(format!("Unexpected argument: '{arg}'"));
        }
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n, Some(v.to_string())),
            _ => (arg.as_str(), None),
        };
        // Values may start with '-' (e.g. "--brightness -10"), so take the next word as is.
        let mut value = || -> Result<String, String> {
            match inline.clone().or_else(|| it.next().cloned()) {
                Some(v) if !v.is_empty() => Ok(v),
                _ => Err(format!("Option {name} requires a value")),
            }
        };
        let no_value = || match &inline {
            Some(_) => Err(format!("Option {name} does not take a value")),
            None => Ok(()),
        };
        match name {
            "-h" | "--help" => no_value().and_then(|_| flag(&mut raw.help, name))?,
            "-v" | "--version" => no_value().and_then(|_| flag(&mut raw.version, name))?,
            "--cli" => no_value().and_then(|_| flag(&mut raw.cli, name))?,
            "--noninteractive" => no_value().and_then(|_| flag(&mut raw.noninteractive, name))?,
            "-l" | "--list-displays" | "-q" | "--query" | "--listmonitors" => {
                no_value().and_then(|_| flag(&mut raw.list, name))?
            }
            "--verbose" => no_value().and_then(|_| flag(&mut raw.verbose, name))?,
            "--json" => no_value().and_then(|_| flag(&mut raw.json, name))?,
            "--current" => no_value().and_then(|_| flag(&mut raw.current, name))?,
            "--dryrun" | "--dry-run" => no_value().and_then(|_| flag(&mut raw.dry_run, name))?,
            "--all" => no_value().and_then(|_| flag(&mut raw.all, name))?,
            "--output" => raw.outputs.push(Target::parse(&value()?)?),
            "--brightness" => set_once(&mut raw.brightness, Change::parse(&value()?)?, name)?,
            "--contrast" => set_once(&mut raw.contrast, Change::parse(&value()?)?, name)?,
            "-g" | "--get-percentage" => set_once(&mut raw.get, Target::parse(&value()?)?, name)?,
            "--get-contrast" => set_once(&mut raw.get_contrast, Target::parse(&value()?)?, name)?,
            "-p" | "--percentage" => set_once(&mut raw.percentage, percentage(&value()?)?, name)?,
            "-o" | "--show-osd" => {
                value()?;
            }
            _ => {
                let (op, contrast) = match name {
                    "-s" | "--set-brightness" => (Op::Set, false),
                    "-i" | "--increase-brightness" => (Op::Increase, false),
                    "-d" | "--decrease-brightness" => (Op::Decrease, false),
                    "--set-contrast" => (Op::Set, true),
                    "--increase-contrast" => (Op::Increase, true),
                    "--decrease-contrast" => (Op::Decrease, true),
                    _ => return Err(format!("Unknown option: '{name}'")),
                };
                // Optional DISPLAY: the next word unless it is another option.
                let target = match inline {
                    Some(v) => Some(Target::parse(&v)?),
                    None => match it.peek() {
                        Some(v) if !v.starts_with('-') => Some(Target::parse(it.next().unwrap())?),
                        _ => None,
                    },
                };
                let slot = if contrast { &mut raw.legacy_contrast } else { &mut raw.legacy_brightness };
                if slot.is_some() {
                    let what = if contrast { "contrast" } else { "brightness" };
                    return Err(format!("Only one {what} operation can be given"));
                }
                *slot = Some((op, target));
            }
        }
    }
    Ok(raw)
}

fn validate(raw: Raw) -> Result<Invocation, String> {
    let mut inv = Invocation { command: Command::Help, dry_run: raw.dry_run, verbose: raw.verbose, current: raw.current };
    if raw.help {
        return Ok(inv);
    }
    if raw.version {
        inv.command = Command::Version;
        return Ok(inv);
    }
    if raw.cli && raw.noninteractive {
        return Err("--cli and --noninteractive cannot be used together".into());
    }
    if raw.all && !raw.outputs.is_empty() {
        return Err("--all and --output cannot be used together".into());
    }
    if raw.brightness.is_some() && raw.legacy_brightness.is_some() {
        return Err("--brightness cannot be combined with -s/-i/-d".into());
    }
    if raw.contrast.is_some() && raw.legacy_contrast.is_some() {
        return Err("--contrast cannot be combined with --set/increase/decrease-contrast".into());
    }
    let has_legacy = raw.legacy_brightness.is_some() || raw.legacy_contrast.is_some();
    if has_legacy && raw.percentage.is_none() {
        return Err("Percentage value is required. Use -p or --percentage to specify the percentage.".into());
    }
    if !has_legacy && raw.percentage.is_some() {
        return Err("-p/--percentage is only used with -s/-i/-d or --set/increase/decrease-contrast".into());
    }

    let mut commands = Vec::new();
    if raw.list {
        let format = match (raw.json, raw.verbose) {
            (true, true) => return Err("--json and --verbose cannot be used together".into()),
            (true, false) => ListFormat::Json,
            (false, true) => ListFormat::Verbose,
            (false, false) => ListFormat::Plain,
        };
        commands.push(Command::List { targets: raw.outputs.clone(), format });
    } else if raw.json {
        return Err("--json is only used with --list-displays".into());
    }
    if let Some(target) = raw.get {
        commands.push(Command::Get { target, contrast: false });
    }
    if let Some(target) = raw.get_contrast {
        commands.push(Command::Get { target, contrast: true });
    }

    if has_legacy || raw.brightness.is_some() || raw.contrast.is_some() {
        let p = raw.percentage.unwrap_or(0.0);
        let mut targets = raw.outputs.clone();
        let mut legacy = |op: Option<(Op, Option<Target>)>| -> Result<Option<Change>, String> {
            let Some((op, target)) = op else { return Ok(None) };
            if let Some(t) = target {
                if !raw.outputs.is_empty() {
                    return Err("A display given to -s/-i/-d/--*-contrast cannot be combined with --output".into());
                }
                if !targets.is_empty() && targets != [t.clone()] {
                    return Err("Brightness and contrast operations name different displays".into());
                }
                targets = vec![t];
            }
            Ok(Some(Change { op, value: p }))
        };
        let legacy_b = legacy(raw.legacy_brightness.clone())?;
        let legacy_c = legacy(raw.legacy_contrast.clone())?;
        let linked = targets.is_empty() && !raw.all && legacy_b.is_some_and(|c| c.op != Op::Set);
        commands.push(Command::Adjust(Adjust {
            targets,
            brightness: raw.brightness.or(legacy_b),
            contrast: raw.contrast.or(legacy_c),
            linked,
        }));
    }

    if raw.cli {
        if !commands.is_empty() {
            return Err("--cli cannot be combined with listing or brightness/contrast options".into());
        }
        inv.command = Command::Interactive { targets: raw.outputs };
        return Ok(inv);
    }
    if commands.len() > 1 {
        return Err("Give only one of: listing, -g/--get-*, or brightness/contrast changes".into());
    }
    let Some(command) = commands.pop() else {
        return Err(if raw.noninteractive {
            "Nothing to do in --noninteractive mode; give e.g. --brightness or --list-displays (see --help)".into()
        } else if !raw.outputs.is_empty() || raw.all {
            "--output/--all select displays; also give e.g. --brightness or --list-displays".into()
        } else {
            "Nothing to do; see --help".into()
        });
    };
    if raw.dry_run && !matches!(command, Command::Adjust(_)) {
        return Err("--dryrun is only used with brightness/contrast changes or --cli".into());
    }
    if matches!(command, Command::Get { .. }) && (!raw.outputs.is_empty() || raw.all) {
        return Err("-g/--get-* take the display directly, without --output/--all".into());
    }
    inv.command = command;
    Ok(inv)
}

/// Parses everything after the program name.
pub fn parse(args: &[String]) -> Result<Invocation, String> {
    parse_raw(args).and_then(validate)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_str(s: &str) -> Result<Invocation, String> {
        parse(&s.split_whitespace().map(String::from).collect::<Vec<_>>())
    }

    fn command(s: &str) -> Command {
        parse_str(s).unwrap_or_else(|e| panic!("{s}: {e}")).command
    }

    fn adjust(s: &str) -> Adjust {
        match command(s) {
            Command::Adjust(a) => a,
            other => panic!("{s}: {other:?}"),
        }
    }

    fn set(v: f64) -> Option<Change> {
        Some(Change { op: Op::Set, value: v })
    }

    #[test]
    fn legacy_options() {
        let a = adjust("-s 2 -p 40");
        assert_eq!((a.targets, a.brightness, a.linked), (vec![Target::Number(2)], set(40.0), false));
        let a = adjust("--decrease-brightness -p 5");
        assert_eq!(a.brightness, Some(Change { op: Op::Decrease, value: 5.0 }));
        assert!(a.targets.is_empty() && a.linked);
        let a = adjust("-s eDP-1 -p 40%");
        assert_eq!(a.targets, vec![Target::Name("eDP-1".into())]);
        let a = adjust("--set-brightness=1 --set-contrast=1 -p 30");
        assert_eq!((a.brightness, a.contrast), (set(30.0), set(30.0)));
        assert_eq!(command("-g 1"), Command::Get { target: Target::Number(1), contrast: false });
        assert_eq!(command("-l"), Command::List { targets: vec![], format: ListFormat::Plain });
        assert!(matches!(command("-i -p 5 -o gnome"), Command::Adjust(_)));
    }

    #[test]
    fn xrandr_style_options() {
        let a = adjust("--output DP-2 --output 1 --brightness 30");
        assert_eq!(a.targets, vec![Target::Name("DP-2".into()), Target::Number(1)]);
        assert_eq!(a.brightness, set(30.0));
        let a = adjust("--noninteractive --output=eDP-1 --brightness -10 --contrast +5%");
        assert_eq!(a.brightness, Some(Change { op: Op::Decrease, value: 10.0 }));
        assert_eq!(a.contrast, Some(Change { op: Op::Increase, value: 5.0 }));
        assert!(!a.linked);
        assert!(!adjust("--all --brightness +5").linked);
        assert_eq!(command("--listmonitors --json"), Command::List { targets: vec![], format: ListFormat::Json });
        assert_eq!(
            command("-q --verbose --output 2"),
            Command::List { targets: vec![Target::Number(2)], format: ListFormat::Verbose }
        );
        let inv = parse_str("--dryrun --brightness 50 --current").unwrap();
        assert!(inv.dry_run && inv.current);
    }

    #[test]
    fn modes() {
        assert_eq!(command("--help"), Command::Help);
        assert_eq!(command("-s --help"), Command::Help);
        assert_eq!(command("-v"), Command::Version);
        assert_eq!(command("--cli"), Command::Interactive { targets: vec![] });
        assert_eq!(command("--cli --output DP-2"), Command::Interactive { targets: vec![Target::Name("DP-2".into())] });
        assert!(parse_str("--cli --dryrun").unwrap().dry_run);
    }

    #[test]
    fn rejects_invalid() {
        for bad in [
            "--bogus",
            "-x",
            "foo",
            "-",
            "-lq",
            "-s 0 -p 5",
            "-s 1",
            "-p 40",
            "-s -p 101",
            "-s -p -5",
            "-s -p nan",
            "-s -p inf",
            "-s -p abc",
            "-s -p",
            "-g",
            "-g 0",
            "--brightness",
            "--brightness=",
            "--brightness 150",
            "--brightness ++5",
            "--brightness +-5",
            "--brightness 40%%",
            "-s -p +5",
            "--brightness +nan",
            "--brightness x",
            "--brightness 10 --brightness 20",
            "--brightness 10 -s -p 10",
            "-s -i -p 5",
            "-p 5 -p 6 -s",
            "--output",
            "--output=",
            "--output 0 --brightness 5",
            "--output 1",
            "--all",
            "--all --output 1 --brightness 5",
            "-s 1 -p 5 --output 2",
            "-s 1 --set-contrast 2 -p 5",
            "--list-displays=3",
            "--help=yes",
            "--cli --noninteractive",
            "--cli --brightness 5",
            "--cli -l",
            "--noninteractive",
            "--noninteractive --dryrun",
            "--json",
            "-l --json --verbose",
            "-l --dryrun",
            "-l -g 1",
            "-g 1 --output 2",
            "-l -l",
            "--cli --cli",
        ] {
            assert!(parse_str(bad).is_err(), "accepted: {bad}");
        }
    }

    #[test]
    fn change_apply_clamps() {
        assert_eq!(Change { op: Op::Increase, value: 30.0 }.apply(90.0), 100.0);
        assert_eq!(Change { op: Op::Decrease, value: 30.0 }.apply(10.0), 0.0);
        assert_eq!(Change { op: Op::Set, value: 42.0 }.apply(10.0), 42.0);
    }
}
