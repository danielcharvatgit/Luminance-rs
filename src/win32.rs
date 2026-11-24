//! The few Win32 APIs the Windows port needs, declared by hand (like the
//! Wayland wire protocol on Linux) instead of pulling in `windows-sys`.
//!
//! Plus small safe helpers shared by the hardware and dimming backends:
//! monitor enumeration, display names from the CCD API, and process glue
//! (console attachment for the CLI, single instance for the GUI).

#![allow(non_snake_case, non_camel_case_types, clippy::upper_case_acronyms)]

use std::collections::HashMap;
use std::ffi::c_void;
use std::mem;
use std::ptr;

pub type BOOL = i32;
pub type HANDLE = *mut c_void;
pub type HWND = *mut c_void;
pub type HDC = *mut c_void;
pub type HMONITOR = *mut c_void;
pub type HINSTANCE = *mut c_void;
pub type WPARAM = usize;
pub type LPARAM = isize;
pub type LRESULT = isize;

pub type MONITORENUMPROC = unsafe extern "system" fn(HMONITOR, HDC, *mut RECT, LPARAM) -> BOOL;
pub type WNDPROC = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;

pub const INVALID_HANDLE_VALUE: HANDLE = -1isize as HANDLE;
pub const ERROR_ALREADY_EXISTS: u32 = 183;
pub const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
pub const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
pub const GENERIC_READ: u32 = 0x8000_0000;
pub const GENERIC_WRITE: u32 = 0x4000_0000;
pub const FILE_SHARE_READ: u32 = 1;
pub const FILE_SHARE_WRITE: u32 = 2;
pub const OPEN_EXISTING: u32 = 3;
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub const MONITORINFOF_PRIMARY: u32 = 1;

pub const WS_POPUP: u32 = 0x8000_0000;
pub const WS_EX_TOPMOST: u32 = 0x8;
pub const WS_EX_TRANSPARENT: u32 = 0x20;
pub const WS_EX_TOOLWINDOW: u32 = 0x80;
pub const WS_EX_LAYERED: u32 = 0x8_0000;
pub const WS_EX_NOACTIVATE: u32 = 0x0800_0000;
pub const LWA_ALPHA: u32 = 2;
pub const HWND_TOPMOST: HWND = -1isize as HWND;
pub const SWP_NOACTIVATE: u32 = 0x10;
pub const SWP_SHOWWINDOW: u32 = 0x40;
pub const SW_RESTORE: i32 = 9;
pub const WM_DISPLAYCHANGE: u32 = 0x7E;
pub const WM_NCHITTEST: u32 = 0x84;
pub const WM_MOUSEACTIVATE: u32 = 0x21;
pub const WM_TIMER: u32 = 0x113;
pub const WM_APP: u32 = 0x8000;
pub const HTTRANSPARENT: LRESULT = -1;
pub const MA_NOACTIVATE: LRESULT = 3;
pub const PM_NOREMOVE: u32 = 0;
pub const BLACK_BRUSH: i32 = 4;
pub const WDA_EXCLUDEFROMCAPTURE: u32 = 0x11;
/// DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2
pub const DPI_PER_MONITOR_AWARE_V2: isize = -4;

pub const QDC_ONLY_ACTIVE_PATHS: u32 = 2;
pub const DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME: u32 = 1;
pub const DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct RECT {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[repr(C)]
pub struct MONITORINFOEXW {
    pub cbSize: u32,
    pub rcMonitor: RECT,
    pub rcWork: RECT,
    pub dwFlags: u32,
    pub szDevice: [u16; 32],
}

#[repr(C, packed)]
pub struct PHYSICAL_MONITOR {
    pub hPhysicalMonitor: HANDLE,
    pub szPhysicalMonitorDescription: [u16; 128],
}

#[repr(C)]
pub struct POINT {
    pub x: i32,
    pub y: i32,
}

#[repr(C)]
pub struct MSG {
    pub hwnd: HWND,
    pub message: u32,
    pub wParam: WPARAM,
    pub lParam: LPARAM,
    pub time: u32,
    pub pt: POINT,
    pub lPrivate: u32,
}

#[repr(C)]
pub struct WNDCLASSEXW {
    pub cbSize: u32,
    pub style: u32,
    pub lpfnWndProc: Option<WNDPROC>,
    pub cbClsExtra: i32,
    pub cbWndExtra: i32,
    pub hInstance: HINSTANCE,
    pub hIcon: HANDLE,
    pub hCursor: HANDLE,
    pub hbrBackground: HANDLE,
    pub lpszMenuName: *const u16,
    pub lpszClassName: *const u16,
    pub hIconSm: HANDLE,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct LUID {
    pub LowPart: u32,
    pub HighPart: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct DISPLAYCONFIG_PATH_SOURCE_INFO {
    pub adapterId: LUID,
    pub id: u32,
    pub modeInfoIdx: u32,
    pub statusFlags: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct DISPLAYCONFIG_PATH_TARGET_INFO {
    pub adapterId: LUID,
    pub id: u32,
    pub modeInfoIdx: u32,
    pub outputTechnology: u32,
    pub rotation: u32,
    pub scaling: u32,
    pub refreshRate: [u32; 2],
    pub scanLineOrdering: u32,
    pub targetAvailable: BOOL,
    pub statusFlags: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct DISPLAYCONFIG_PATH_INFO {
    pub sourceInfo: DISPLAYCONFIG_PATH_SOURCE_INFO,
    pub targetInfo: DISPLAYCONFIG_PATH_TARGET_INFO,
    pub flags: u32,
}

/// Only needed as buffer space for QueryDisplayConfig.
#[repr(C, align(8))]
#[derive(Clone, Copy)]
pub struct DISPLAYCONFIG_MODE_INFO {
    pub raw: [u8; 64],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct DISPLAYCONFIG_DEVICE_INFO_HEADER {
    pub kind: u32,
    pub size: u32,
    pub adapterId: LUID,
    pub id: u32,
}

#[repr(C)]
pub struct DISPLAYCONFIG_SOURCE_DEVICE_NAME {
    pub header: DISPLAYCONFIG_DEVICE_INFO_HEADER,
    pub viewGdiDeviceName: [u16; 32],
}

#[repr(C)]
pub struct DISPLAYCONFIG_TARGET_DEVICE_NAME {
    pub header: DISPLAYCONFIG_DEVICE_INFO_HEADER,
    pub flags: u32,
    pub outputTechnology: u32,
    pub edidManufactureId: u16,
    pub edidProductCodeId: u16,
    pub connectorInstance: u32,
    pub monitorFriendlyDeviceName: [u16; 64],
    pub monitorDevicePath: [u16; 128],
}

#[repr(C)]
#[derive(Default)]
pub struct SYSTEM_POWER_STATUS {
    pub ACLineStatus: u8,
    pub BatteryFlag: u8,
    pub BatteryLifePercent: u8,
    pub SystemStatusFlag: u8,
    pub BatteryLifeTime: u32,
    pub BatteryFullLifeTime: u32,
}

#[link(name = "kernel32")]
extern "system" {
    pub fn AttachConsole(process: u32) -> BOOL;
    pub fn GetStdHandle(which: u32) -> HANDLE;
    pub fn GetLastError() -> u32;
    pub fn CreateMutexW(attrs: *const c_void, owner: BOOL, name: *const u16) -> HANDLE;
    pub fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *const c_void,
        disposition: u32,
        flags: u32,
        template: HANDLE,
    ) -> HANDLE;
    pub fn DeviceIoControl(
        device: HANDLE,
        code: u32,
        input: *const c_void,
        input_len: u32,
        output: *mut c_void,
        output_len: u32,
        returned: *mut u32,
        overlapped: *mut c_void,
    ) -> BOOL;
    pub fn CloseHandle(handle: HANDLE) -> BOOL;
    pub fn GetCurrentThreadId() -> u32;
    pub fn GetModuleHandleW(name: *const u16) -> HINSTANCE;
    pub fn GetSystemPowerStatus(status: *mut SYSTEM_POWER_STATUS) -> BOOL;
}

#[link(name = "user32")]
extern "system" {
    pub fn EnumDisplayMonitors(hdc: HDC, clip: *const RECT, callback: MONITORENUMPROC, data: LPARAM) -> BOOL;
    pub fn GetMonitorInfoW(monitor: HMONITOR, info: *mut MONITORINFOEXW) -> BOOL;
    pub fn GetDisplayConfigBufferSizes(flags: u32, paths: *mut u32, modes: *mut u32) -> i32;
    pub fn QueryDisplayConfig(
        flags: u32,
        num_paths: *mut u32,
        paths: *mut DISPLAYCONFIG_PATH_INFO,
        num_modes: *mut u32,
        modes: *mut DISPLAYCONFIG_MODE_INFO,
        topology: *mut u32,
    ) -> i32;
    pub fn DisplayConfigGetDeviceInfo(packet: *mut DISPLAYCONFIG_DEVICE_INFO_HEADER) -> i32;
    pub fn FindWindowW(class: *const u16, title: *const u16) -> HWND;
    pub fn SetForegroundWindow(hwnd: HWND) -> BOOL;
    pub fn ShowWindow(hwnd: HWND, cmd: i32) -> BOOL;
    pub fn IsIconic(hwnd: HWND) -> BOOL;
    pub fn RegisterClassExW(class: *const WNDCLASSEXW) -> u16;
    pub fn CreateWindowExW(
        ex_style: u32,
        class: *const u16,
        title: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: HWND,
        menu: HANDLE,
        instance: HINSTANCE,
        param: *mut c_void,
    ) -> HWND;
    pub fn DestroyWindow(hwnd: HWND) -> BOOL;
    pub fn DefWindowProcW(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT;
    pub fn GetMessageW(msg: *mut MSG, hwnd: HWND, min: u32, max: u32) -> BOOL;
    pub fn PeekMessageW(msg: *mut MSG, hwnd: HWND, min: u32, max: u32, remove: u32) -> BOOL;
    pub fn TranslateMessage(msg: *const MSG) -> BOOL;
    pub fn DispatchMessageW(msg: *const MSG) -> LRESULT;
    pub fn PostThreadMessageW(thread: u32, msg: u32, wparam: WPARAM, lparam: LPARAM) -> BOOL;
    pub fn SetLayeredWindowAttributes(hwnd: HWND, key: u32, alpha: u8, flags: u32) -> BOOL;
    pub fn SetWindowPos(hwnd: HWND, after: HWND, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> BOOL;
    pub fn SetWindowDisplayAffinity(hwnd: HWND, affinity: u32) -> BOOL;
    pub fn SetTimer(hwnd: HWND, id: usize, ms: u32, callback: *const c_void) -> usize;
    pub fn SetThreadDpiAwarenessContext(context: isize) -> isize;
}

#[link(name = "gdi32")]
extern "system" {
    pub fn CreateDCW(driver: *const u16, device: *const u16, port: *const u16, mode: *const c_void) -> HDC;
    pub fn DeleteDC(hdc: HDC) -> BOOL;
    pub fn GetDeviceGammaRamp(hdc: HDC, ramp: *mut c_void) -> BOOL;
    pub fn SetDeviceGammaRamp(hdc: HDC, ramp: *const c_void) -> BOOL;
    pub fn GetStockObject(which: i32) -> HANDLE;
}

#[link(name = "dxva2")]
extern "system" {
    pub fn GetNumberOfPhysicalMonitorsFromHMONITOR(monitor: HMONITOR, count: *mut u32) -> BOOL;
    pub fn GetPhysicalMonitorsFromHMONITOR(monitor: HMONITOR, count: u32, monitors: *mut PHYSICAL_MONITOR) -> BOOL;
    pub fn DestroyPhysicalMonitors(count: u32, monitors: *mut PHYSICAL_MONITOR) -> BOOL;
    pub fn GetVCPFeatureAndVCPFeatureReply(
        monitor: HANDLE,
        code: u8,
        kind: *mut u32,
        current: *mut u32,
        maximum: *mut u32,
    ) -> BOOL;
    pub fn SetVCPFeature(monitor: HANDLE, code: u8, value: u32) -> BOOL;
}

// Checked at compile time against the sizes in the Windows SDK headers.
const _: () = {
    assert!(mem::size_of::<DISPLAYCONFIG_PATH_INFO>() == 72);
    assert!(mem::size_of::<DISPLAYCONFIG_MODE_INFO>() == 64);
    assert!(mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() == 84);
    assert!(mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() == 420);
    assert!(mem::size_of::<PHYSICAL_MONITOR>() == 264);
    assert!(mem::size_of::<MONITORINFOEXW>() == 104);
    assert!(mem::size_of::<SYSTEM_POWER_STATUS>() == 12);
};

/// NUL-terminated UTF-16 for Win32 calls.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// UTF-16 buffer up to the first NUL.
pub fn from_wide(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// "\\.\DISPLAY1" -> "DISPLAY1"
pub fn short_device_name(gdi: &str) -> String {
    gdi.trim_start_matches(['\\', '.']).to_string()
}

/// An active monitor as GDI knows it.
#[derive(Clone, Debug)]
pub struct Monitor {
    pub handle: HMONITOR,
    /// Short GDI device name, e.g. "DISPLAY1".
    pub device: String,
    /// Desktop rectangle, in physical pixels when the calling thread is
    /// per-monitor DPI aware.
    pub rect: RECT,
    pub primary: bool,
}

unsafe extern "system" fn collect_monitor(handle: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
    let list = &mut *(data as *mut Vec<Monitor>);
    let mut info: MONITORINFOEXW = mem::zeroed();
    info.cbSize = mem::size_of::<MONITORINFOEXW>() as u32;
    if GetMonitorInfoW(handle, &mut info) != 0 {
        list.push(Monitor {
            handle,
            device: short_device_name(&from_wide(&info.szDevice)),
            rect: info.rcMonitor,
            primary: info.dwFlags & MONITORINFOF_PRIMARY != 0,
        });
    }
    1
}

/// All active monitors, primary first, then in GDI order.
pub fn monitors() -> Vec<Monitor> {
    let mut list: Vec<Monitor> = Vec::new();
    // SAFETY: the callback only runs during this call and gets a pointer to `list`.
    unsafe { EnumDisplayMonitors(ptr::null_mut(), ptr::null(), collect_monitor, &mut list as *mut _ as LPARAM) };
    list.sort_by_key(|m| (!m.primary, display_number(&m.device)));
    list
}

/// "DISPLAY12" -> 12
pub fn display_number(device: &str) -> u32 {
    device.trim_start_matches(|c: char| !c.is_ascii_digit()).parse().unwrap_or(0)
}

/// What the display configuration (CCD) API knows about a monitor.
#[derive(Clone, Debug, Default)]
pub struct TargetInfo {
    /// EDID name, e.g. "DELL U2415"; empty for most built-in panels.
    pub friendly_name: String,
    /// Built-in panel (eDP, LVDS, ...).
    pub internal: bool,
}

/// DISPLAYCONFIG_VIDEO_OUTPUT_TECHNOLOGY values of built-in panels:
/// LVDS, embedded DisplayPort, embedded UDI and "internal".
fn is_internal_technology(tech: u32) -> bool {
    matches!(tech, 6 | 11 | 13 | 0x8000_0000)
}

/// Maps short GDI device names ("DISPLAY1") to their monitors' CCD info.
/// With mirrored displays the first target wins.
pub fn display_targets() -> HashMap<String, TargetInfo> {
    let mut map = HashMap::new();
    // SAFETY: buffers are sized as GetDisplayConfigBufferSizes says, and every
    // packet's header carries its own size.
    unsafe {
        let (mut num_paths, mut num_modes) = (0u32, 0u32);
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut num_paths, &mut num_modes) != 0 {
            return map;
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); num_paths as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO { raw: [0; 64] }; num_modes as usize];
        let rc = QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut num_paths,
            paths.as_mut_ptr(),
            &mut num_modes,
            modes.as_mut_ptr(),
            ptr::null_mut(),
        );
        if rc != 0 {
            return map;
        }
        paths.truncate(num_paths as usize);
        for path in &paths {
            let mut source: DISPLAYCONFIG_SOURCE_DEVICE_NAME = mem::zeroed();
            source.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
                kind: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size: mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                adapterId: path.sourceInfo.adapterId,
                id: path.sourceInfo.id,
            };
            if DisplayConfigGetDeviceInfo(&mut source.header) != 0 {
                continue;
            }
            let mut target: DISPLAYCONFIG_TARGET_DEVICE_NAME = mem::zeroed();
            target.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
                kind: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                size: mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
                adapterId: path.targetInfo.adapterId,
                id: path.targetInfo.id,
            };
            let info = if DisplayConfigGetDeviceInfo(&mut target.header) == 0 {
                TargetInfo {
                    friendly_name: from_wide(&target.monitorFriendlyDeviceName).trim().to_string(),
                    internal: is_internal_technology(target.outputTechnology)
                        || is_internal_technology(path.targetInfo.outputTechnology),
                }
            } else {
                TargetInfo { internal: is_internal_technology(path.targetInfo.outputTechnology), ..Default::default() }
            };
            map.entry(short_device_name(&from_wide(&source.viewGdiDeviceName))).or_insert(info);
        }
    }
    map
}

/// Physical monitors (DDC/CI handles) of one GDI monitor, released on drop.
pub struct PhysicalMonitors(Vec<PHYSICAL_MONITOR>);

impl PhysicalMonitors {
    pub fn of(monitor: HMONITOR) -> Self {
        // SAFETY: the array is allocated for exactly `count` entries.
        unsafe {
            let mut count = 0u32;
            if GetNumberOfPhysicalMonitorsFromHMONITOR(monitor, &mut count) == 0 || count == 0 {
                return Self(Vec::new());
            }
            let mut list: Vec<PHYSICAL_MONITOR> = (0..count).map(|_| mem::zeroed()).collect();
            if GetPhysicalMonitorsFromHMONITOR(monitor, count, list.as_mut_ptr()) == 0 {
                return Self(Vec::new());
            }
            Self(list)
        }
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn handle(&self, index: usize) -> Option<HANDLE> {
        self.0.get(index).map(|m| m.hPhysicalMonitor)
    }

    pub fn description(&self, index: usize) -> String {
        self.0.get(index).map(|m| from_wide(&{ m.szPhysicalMonitorDescription })).unwrap_or_default()
    }
}

impl Drop for PhysicalMonitors {
    fn drop(&mut self) {
        if !self.0.is_empty() {
            // SAFETY: the handles came from GetPhysicalMonitorsFromHMONITOR.
            unsafe { DestroyPhysicalMonitors(self.0.len() as u32, self.0.as_mut_ptr()) };
        }
    }
}

/// Lets a GUI-subsystem executable print to the console it was started
/// from (`luminance-rs.exe --list-displays` in cmd or PowerShell). Output
/// that is already redirected to a file or pipe is left alone.
pub fn attach_parent_console() {
    // SAFETY: plain calls without pointers.
    unsafe {
        let out = GetStdHandle(STD_OUTPUT_HANDLE);
        if out.is_null() || out == INVALID_HANDLE_VALUE {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

/// Single instance without D-Bus: returns `true` (after bringing its window
/// to the front) when Luminance is already running in this session.
pub fn activate_running_instance(app_id: &str, window_title: &str) -> bool {
    let name = wide(&format!("Local\\{app_id}"));
    // SAFETY: the mutex handle is intentionally leaked; it lives as long as
    // the process and marks it as the running instance.
    unsafe {
        let mutex = CreateMutexW(ptr::null(), 0, name.as_ptr());
        if mutex.is_null() || GetLastError() != ERROR_ALREADY_EXISTS {
            return false;
        }
        let title = wide(window_title);
        let class = wide("gdkSurfaceToplevel");
        let mut hwnd = FindWindowW(class.as_ptr(), title.as_ptr());
        if hwnd.is_null() {
            hwnd = FindWindowW(ptr::null(), title.as_ptr());
        }
        if !hwnd.is_null() {
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            }
            SetForegroundWindow(hwnd);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_names() {
        assert_eq!(short_device_name("\\\\.\\DISPLAY12"), "DISPLAY12");
        assert_eq!(display_number("DISPLAY12"), 12);
    }
}
