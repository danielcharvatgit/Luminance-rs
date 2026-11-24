//! Windows backend.
//!
//!  * Built-in panels: the `\\.\LCD` brightness IOCTLs (fast, no extra
//!    process), falling back to WMI (`WmiMonitorBrightnessMethods`) through
//!    a PowerShell process that is kept running for later changes.
//!  * External monitors: DDC/CI through the Monitor Configuration API
//!    (dxva2), which works without administrator rights or drivers.

use std::ffi::c_void;
use std::io::{self, Write};
use std::mem;
use std::os::windows::process::CommandExt;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::ptr;
use std::sync::Mutex;

use super::{backlight_percent, backlight_raw, ddc_percent, ddc_raw, Control, Display, Scan};
use crate::win32::{self, PhysicalMonitors, HANDLE};

const VCP_BRIGHTNESS: u8 = 0x10;
const VCP_CONTRAST: u8 = 0x12;

/// CTL_CODE(FILE_DEVICE_VIDEO, 0x125..0x127, METHOD_BUFFERED, FILE_ANY_ACCESS)
const IOCTL_VIDEO_QUERY_SUPPORTED_BRIGHTNESS: u32 = 0x0023_0494;
const IOCTL_VIDEO_QUERY_DISPLAY_BRIGHTNESS: u32 = 0x0023_0498;
const IOCTL_VIDEO_SET_DISPLAY_BRIGHTNESS: u32 = 0x0023_049C;
const DISPLAYPOLICY_AC: u8 = 1;
const DISPLAYPOLICY_DC: u8 = 2;
const DISPLAYPOLICY_BOTH: u8 = 3;

/// Built-in panels report brightness in percent.
const BACKLIGHT_MAX: u32 = 100;
const LCD: &str = "LCD";
const WMI: &str = "WMI";

/// Physical monitors per GDI display that [`Control::Ddc`]'s `bus` can address.
const PHYSICAL_PER_DISPLAY: u32 = 16;

#[repr(C)]
#[derive(Default)]
struct DisplayBrightness {
    policy: u8,
    ac: u8,
    dc: u8,
}

// --- Built-in panel: \\.\LCD ------------------------------------------------

struct Lcd(HANDLE);

impl Lcd {
    fn open() -> io::Result<Self> {
        let name = win32::wide("\\\\.\\LCD");
        // SAFETY: valid NUL-terminated path; the handle is closed on drop.
        let handle = unsafe {
            win32::CreateFileW(
                name.as_ptr(),
                win32::GENERIC_READ | win32::GENERIC_WRITE,
                win32::FILE_SHARE_READ | win32::FILE_SHARE_WRITE,
                ptr::null(),
                win32::OPEN_EXISTING,
                0,
                ptr::null_mut(),
            )
        };
        if handle == win32::INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(handle))
    }

    fn ioctl(&self, code: u32, input: &[u8], output: &mut [u8]) -> io::Result<usize> {
        let mut returned = 0u32;
        // SAFETY: both buffers are valid for their lengths.
        let ok = unsafe {
            win32::DeviceIoControl(
                self.0,
                code,
                input.as_ptr() as *const c_void,
                input.len() as u32,
                output.as_mut_ptr() as *mut c_void,
                output.len() as u32,
                &mut returned,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(returned as usize)
    }

    /// Brightness levels (0-100) the panel supports, ascending.
    fn levels(&self) -> io::Result<Vec<u8>> {
        let mut buf = [0u8; 256];
        let n = self.ioctl(IOCTL_VIDEO_QUERY_SUPPORTED_BRIGHTNESS, &[], &mut buf)?;
        let mut levels = buf[..n].to_vec();
        levels.sort_unstable();
        levels.dedup();
        Ok(levels)
    }

    fn current(&self) -> io::Result<u8> {
        let mut b = DisplayBrightness::default();
        // SAFETY: DisplayBrightness is three plain bytes.
        let buf = unsafe { std::slice::from_raw_parts_mut(&mut b as *mut _ as *mut u8, mem::size_of_val(&b)) };
        self.ioctl(IOCTL_VIDEO_QUERY_DISPLAY_BRIGHTNESS, &[], buf)?;
        Ok(match b.policy {
            DISPLAYPOLICY_AC => b.ac,
            DISPLAYPOLICY_DC => b.dc,
            _ if on_battery() => b.dc,
            _ => b.ac,
        })
    }

    fn set(&self, value: u8) -> io::Result<()> {
        let level = match self.levels() {
            // Drivers may reject values they do not list: use the nearest one.
            Ok(levels) if !levels.is_empty() => {
                *levels.iter().min_by_key(|&&l| (l as i32 - value as i32).abs()).unwrap_or(&value)
            }
            _ => value,
        };
        let b = DisplayBrightness { policy: DISPLAYPOLICY_BOTH, ac: level, dc: level };
        // SAFETY: DisplayBrightness is three plain bytes.
        let buf = unsafe { std::slice::from_raw_parts(&b as *const _ as *const u8, mem::size_of_val(&b)) };
        self.ioctl(IOCTL_VIDEO_SET_DISPLAY_BRIGHTNESS, buf, &mut []).map(|_| ())
    }
}

impl Drop for Lcd {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateFileW.
        unsafe { win32::CloseHandle(self.0) };
    }
}

fn on_battery() -> bool {
    let mut status = win32::SYSTEM_POWER_STATUS::default();
    // SAFETY: plain out-parameter.
    unsafe { win32::GetSystemPowerStatus(&mut status) != 0 && status.ACLineStatus == 0 }
}

fn lcd_brightness() -> Option<u8> {
    let lcd = Lcd::open().ok()?;
    // Panels without IOCTL support report no levels.
    if lcd.levels().ok()?.is_empty() {
        return None;
    }
    lcd.current().ok()
}

// --- Built-in panel: WMI ----------------------------------------------------

fn powershell() -> Command {
    let mut cmd = Command::new("powershell.exe");
    cmd.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass"])
        .creation_flags(win32::CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    cmd
}

fn wmi_brightness() -> Option<u8> {
    let output = powershell()
        .args([
            "-Command",
            "(Get-CimInstance -Namespace root/wmi -ClassName WmiMonitorBrightness -ErrorAction Stop \
             | Where-Object Active | Select-Object -First 1).CurrentBrightness",
        ])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

/// Setting brightness through WMI needs PowerShell, which takes a moment to
/// start. One process is kept and fed one command per change.
static WMI_SESSION: Mutex<Option<(Child, ChildStdin)>> = Mutex::new(None);

fn wmi_set(value: u8) -> Result<(), String> {
    let line = format!(
        "Get-CimInstance -Namespace root/wmi -ClassName WmiMonitorBrightnessMethods \
         | Invoke-CimMethod -MethodName WmiSetBrightness -Arguments @{{Timeout=0; Brightness={value}}} | Out-Null\n"
    );
    let mut session = WMI_SESSION.lock().unwrap_or_else(|e| e.into_inner());
    for _ in 0..2 {
        if session.is_none() {
            let mut child = powershell()
                .args(["-Command", "-"])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .spawn()
                .map_err(|e| format!("cannot start PowerShell for WMI: {e}"))?;
            let stdin = child.stdin.take().ok_or("no PowerShell stdin")?;
            *session = Some((child, stdin));
        }
        if let Some((_, stdin)) = session.as_mut() {
            if stdin.write_all(line.as_bytes()).and_then(|_| stdin.flush()).is_ok() {
                return Ok(());
            }
        }
        // The process died: start a new one.
        if let Some((mut child, _)) = session.take() {
            let _ = child.kill();
        }
    }
    Err("WMI brightness change failed".into())
}

/// Brightness control for the built-in panel, preferring the IOCTLs.
fn internal_backlight() -> Option<(Control, u8)> {
    if let Some(value) = lcd_brightness() {
        return Some((Control::Backlight { device: LCD.into(), max: BACKLIGHT_MAX }, value));
    }
    wmi_brightness().map(|value| (Control::Backlight { device: WMI.into(), max: BACKLIGHT_MAX }, value))
}

// --- External monitors: DDC/CI ----------------------------------------------

fn get_vcp(handle: HANDLE, code: u8) -> io::Result<(u16, u16)> {
    let (mut kind, mut current, mut max) = (0u32, 0u32, 0u32);
    // SAFETY: out-parameters only; `handle` is a live physical monitor handle.
    let ok = unsafe { win32::GetVCPFeatureAndVCPFeatureReply(handle, code, &mut kind, &mut current, &mut max) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    if max == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "VCP feature reports zero maximum"));
    }
    Ok((current.min(u16::MAX as u32) as u16, max.min(u16::MAX as u32) as u16))
}

fn set_vcp(bus: u32, code: u8, value: u16) -> Result<(), String> {
    let device = format!("DISPLAY{}", bus / PHYSICAL_PER_DISPLAY);
    let monitor = win32::monitors()
        .into_iter()
        .find(|m| m.device == device)
        .ok_or_else(|| format!("{device} is no longer connected"))?;
    let physical = PhysicalMonitors::of(monitor.handle);
    let handle = physical
        .handle((bus % PHYSICAL_PER_DISPLAY) as usize)
        .ok_or_else(|| format!("{device}: monitor not found"))?;
    // SAFETY: `handle` stays valid until `physical` is dropped.
    if unsafe { win32::SetVCPFeature(handle, code, value as u32) } == 0 {
        return Err(format!("DDC/CI write to {device} failed: {}", io::Error::last_os_error()));
    }
    Ok(())
}

/// Enumerates controllable displays. Slow (DDC probing): call off the main thread.
pub fn scan(include_internal: bool) -> Scan {
    let mut scan = Scan::default();
    let targets = win32::display_targets();
    let mut internal_seen = false;

    for monitor in win32::monitors() {
        let target = targets.get(&monitor.device).cloned().unwrap_or_default();
        let physical = PhysicalMonitors::of(monitor.handle);

        if target.internal {
            if !include_internal {
                continue;
            }
            let mut display = Display {
                name: "Built-in Display".into(),
                connector: Some(monitor.device.clone()),
                internal: true,
                control: Control::Software,
                brightness: 100.0,
                contrast: None,
            };
            // Only one panel can be reached through \\.\LCD or WMI.
            if !internal_seen {
                internal_seen = true;
                if let Some((control, value)) = internal_backlight() {
                    display.brightness = backlight_percent(value as u32, BACKLIGHT_MAX);
                    display.control = control;
                }
            }
            scan.displays.push(display);
            continue;
        }

        let count = physical.len().clamp(1, PHYSICAL_PER_DISPLAY as usize);
        for index in 0..count {
            let base_name = match (&target.friendly_name, physical.description(index)) {
                (f, _) if !f.is_empty() => f.clone(),
                (_, d) if !d.is_empty() && !d.starts_with("Generic") => d,
                _ => monitor.device.clone(),
            };
            let mut display = Display {
                name: if count > 1 { format!("{base_name} ({})", index + 1) } else { base_name },
                // Mirrored monitors share one GDI display; only the first one
                // carries its name, so each keeps its own identity.
                connector: (index == 0).then(|| monitor.device.clone()),
                internal: false,
                control: Control::Software,
                brightness: 100.0,
                contrast: None,
            };
            if let Some(handle) = physical.handle(index) {
                if let Ok((current, max)) = get_vcp(handle, VCP_BRIGHTNESS) {
                    let contrast = get_vcp(handle, VCP_CONTRAST).ok();
                    display.brightness = ddc_percent(current, max);
                    display.contrast = contrast.map(|(c, m)| ddc_percent(c, m));
                    display.control = Control::Ddc {
                        bus: win32::display_number(&monitor.device) * PHYSICAL_PER_DISPLAY + index as u32,
                        max,
                        contrast_max: contrast.map(|(_, m)| m),
                    };
                }
            }
            scan.displays.push(display);
        }
    }
    // Built-in first, like on Linux.
    scan.displays.sort_by_key(|d| !d.internal);
    scan
}

fn set_backlight(device: &str, percent: f64) -> Result<(), String> {
    let value = backlight_raw(percent, BACKLIGHT_MAX).min(BACKLIGHT_MAX) as u8;
    match device {
        LCD => Lcd::open().and_then(|lcd| lcd.set(value)).map_err(|e| format!("LCD brightness change failed: {e}")),
        _ => wmi_set(value),
    }
}

pub fn set_brightness(control: &Control, percent: f64) -> Result<(), String> {
    match control {
        Control::Backlight { device, .. } => set_backlight(device, percent),
        Control::Ddc { bus, max, .. } => set_vcp(*bus, VCP_BRIGHTNESS, ddc_raw(percent, *max)),
        Control::Software => Ok(()),
    }
}

pub fn set_contrast(control: &Control, percent: f64) -> Result<(), String> {
    match control {
        Control::Ddc { bus, contrast_max: Some(max), .. } => set_vcp(*bus, VCP_CONTRAST, ddc_raw(percent, *max)),
        _ => Err("display does not support contrast".into()),
    }
}
