//! Software dimming on Windows.
//!
//! Two techniques, picked per display:
//!  * GDI gamma ramp (`SetDeviceGammaRamp`): scales the display's current
//!    ramp, so a calibration profile is kept. Windows refuses ramps that
//!    are far from neutral unless the `GdiIcmGammaRange` registry value
//!    allows it, so deep dimming often needs the overlay.
//!  * Overlay: a click-through, topmost, translucent black window covering
//!    the display. Its windows live on a dedicated thread with its own
//!    message loop, so they do not depend on GTK's.
//!
//! A gamma ramp outlives the process, so like the GNOME backend a marker
//! file records that dimming is active, and the next start restores the
//! neutral ramp after a crash. Overlays disappear with the process.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::ptr;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use super::Technique;
use crate::win32;

/// 3 x 256 entries, red, green and blue.
type Ramp = [u16; 768];

const WM_SYNC: u32 = win32::WM_APP + 1;
const WM_STOP: u32 = win32::WM_APP + 2;
/// Re-asserts the overlays above windows that also want to be topmost.
const TOPMOST_INTERVAL_MS: u32 = 2000;

fn identity_ramp() -> Ramp {
    let mut ramp = [0u16; 768];
    for (i, v) in ramp.iter_mut().enumerate() {
        *v = ((i % 256) * 257) as u16;
    }
    ramp
}

fn marker() -> PathBuf {
    crate::settings::config_dir().join("windows-gamma-active")
}

/// Device context of one display, deleted on drop.
struct Dc(win32::HDC);

impl Dc {
    fn open(device: &str) -> Option<Self> {
        let driver = win32::wide("DISPLAY");
        let name = win32::wide(&format!("\\\\.\\{device}"));
        // SAFETY: valid NUL-terminated strings.
        let hdc = unsafe { win32::CreateDCW(driver.as_ptr(), name.as_ptr(), ptr::null(), ptr::null()) };
        (!hdc.is_null()).then_some(Self(hdc))
    }

    fn ramp(&self) -> Option<Ramp> {
        let mut ramp = [0u16; 768];
        // SAFETY: the buffer has the 3 x 256 entries the API writes.
        (unsafe { win32::GetDeviceGammaRamp(self.0, ramp.as_mut_ptr().cast()) } != 0).then_some(ramp)
    }

    fn set_ramp(&self, ramp: &Ramp) -> bool {
        // SAFETY: the buffer has the 3 x 256 entries the API reads.
        unsafe { win32::SetDeviceGammaRamp(self.0, ramp.as_ptr().cast()) != 0 }
    }
}

impl Drop for Dc {
    fn drop(&mut self) {
        // SAFETY: the DC came from CreateDCW.
        unsafe { win32::DeleteDC(self.0) };
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Method {
    Gamma,
    Overlay,
}

#[derive(Default)]
struct State {
    technique: Option<Technique>,
    /// Requested factor per display (short GDI name).
    factors: HashMap<String, f64>,
    /// How each dimmed display is currently dimmed.
    methods: HashMap<String, Method>,
    /// Ramps from before dimming started, restored afterwards.
    original: HashMap<String, Ramp>,
    /// Displays whose driver refused a dimmed ramp (Automatic mode then
    /// keeps using the overlay for them).
    gamma_refused: Vec<String>,
}

pub struct WindowsDimmer {
    state: RefCell<State>,
    overlay: Overlay,
}

impl WindowsDimmer {
    pub fn start(technique: Technique) -> Self {
        if marker().exists() {
            // A previous run did not restore its ramps.
            let neutral = identity_ramp();
            for m in win32::monitors() {
                if let Some(dc) = Dc::open(&m.device) {
                    dc.set_ramp(&neutral);
                }
            }
            let _ = std::fs::remove_file(marker());
        }
        Self {
            state: RefCell::new(State { technique: Some(technique), ..Default::default() }),
            overlay: Overlay::start(),
        }
    }

    pub fn outputs(&self) -> Vec<String> {
        win32::monitors().into_iter().map(|m| m.device).collect()
    }

    pub fn set_technique(&self, technique: Technique) {
        let factors = {
            let mut state = self.state.borrow_mut();
            state.technique = Some(technique);
            state.gamma_refused.clear();
            state.factors.clone()
        };
        for (device, factor) in factors {
            self.apply(&device, factor);
        }
    }

    /// `target` is a display name or "*" for all of them; 1.0 = no dimming.
    pub fn set(&self, target: &str, factor: f64) {
        let devices = match target {
            "*" => self.outputs(),
            _ => vec![target.to_string()],
        };
        for device in devices {
            self.apply(&device, factor);
        }
    }

    fn apply(&self, device: &str, factor: f64) {
        let mut state = self.state.borrow_mut();
        let dimmed = factor < 0.999;
        if dimmed {
            state.factors.insert(device.to_string(), factor);
        } else {
            state.factors.remove(device);
        }

        let wanted = match state.technique.unwrap_or(Technique::Auto) {
            Technique::OverlayOnly => Method::Overlay,
            Technique::GammaOnly => Method::Gamma,
            Technique::Auto if state.gamma_refused.iter().any(|d| d == device) => Method::Overlay,
            Technique::Auto => Method::Gamma,
        };

        // None: not dimmed (or the only allowed method failed).
        let mut method = dimmed.then_some(wanted);
        if method == Some(Method::Gamma) && !self.set_gamma(&mut state, device, factor) {
            if state.technique == Some(Technique::Auto) {
                eprintln!("luminance: {device}: gamma ramp refused, using the overlay");
                state.gamma_refused.push(device.to_string());
                method = Some(Method::Overlay);
            } else {
                eprintln!("luminance: {device}: gamma ramp refused by the display driver");
                method = None;
            }
        }

        // Undo whatever the other method did.
        if method != Some(Method::Gamma) {
            self.restore_gamma(&mut state, device);
        }
        self.overlay.set(device, if method == Some(Method::Overlay) { factor } else { 1.0 });

        match method {
            Some(m) => state.methods.insert(device.to_string(), m),
            None => state.methods.remove(device),
        };
        let gamma_active = state.methods.values().any(|m| *m == Method::Gamma);
        if gamma_active {
            let _ = std::fs::create_dir_all(crate::settings::config_dir());
            let _ = std::fs::write(marker(), b"");
        } else {
            let _ = std::fs::remove_file(marker());
        }
    }

    fn set_gamma(&self, state: &mut State, device: &str, factor: f64) -> bool {
        let Some(dc) = Dc::open(device) else { return false };
        if !state.original.contains_key(device) {
            let ramp = dc.ramp().unwrap_or_else(identity_ramp);
            state.original.insert(device.to_string(), ramp);
        }
        let original = state.original[device];
        let mut ramp = [0u16; 768];
        for (out, v) in ramp.iter_mut().zip(original) {
            *out = (v as f64 * factor).round() as u16;
        }
        dc.set_ramp(&ramp)
    }

    fn restore_gamma(&self, state: &mut State, device: &str) {
        if let Some(original) = state.original.remove(device) {
            if let Some(dc) = Dc::open(device) {
                dc.set_ramp(&original);
            }
        }
    }

    pub fn shutdown(&self) {
        let devices: Vec<String> = self.state.borrow().factors.keys().cloned().collect();
        for device in devices {
            self.apply(&device, 1.0);
        }
        self.overlay.stop();
        let _ = std::fs::remove_file(marker());
    }
}

// --- Overlay windows --------------------------------------------------------

struct Overlay {
    thread_id: u32,
    /// Requested overlay strength per display: 1.0 = no overlay.
    wanted: Arc<Mutex<HashMap<String, f64>>>,
    handle: RefCell<Option<JoinHandle<()>>>,
}

impl Overlay {
    fn start() -> Self {
        let wanted: Arc<Mutex<HashMap<String, f64>>> = Arc::default();
        let (tx, rx) = mpsc::channel();
        let shared = wanted.clone();
        let handle = thread::Builder::new()
            .name("luminance-overlay".into())
            .spawn(move || overlay_thread(shared, tx))
            .ok();
        let thread_id = rx.recv().unwrap_or(0);
        Self { thread_id, wanted, handle: RefCell::new(handle) }
    }

    fn set(&self, device: &str, factor: f64) {
        {
            let mut wanted = self.wanted.lock().unwrap_or_else(|e| e.into_inner());
            let previous = wanted.get(device).copied().unwrap_or(1.0);
            if factor >= 0.999 && previous >= 0.999 {
                return;
            }
            wanted.insert(device.to_string(), factor);
        }
        // SAFETY: plain message post; the thread ignores unknown messages.
        unsafe { win32::PostThreadMessageW(self.thread_id, WM_SYNC, 0, 0) };
    }

    fn stop(&self) {
        if let Some(handle) = self.handle.borrow_mut().take() {
            // SAFETY: as above.
            unsafe { win32::PostThreadMessageW(self.thread_id, WM_STOP, 0, 0) };
            let _ = handle.join();
        }
    }
}

unsafe extern "system" fn overlay_proc(hwnd: win32::HWND, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        win32::WM_NCHITTEST => win32::HTTRANSPARENT,
        win32::WM_MOUSEACTIVATE => win32::MA_NOACTIVATE,
        win32::WM_DISPLAYCHANGE => {
            // Resolution or layout changed: move the overlays.
            win32::PostThreadMessageW(win32::GetCurrentThreadId(), WM_SYNC, 0, 0);
            0
        }
        _ => win32::DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn overlay_thread(wanted: Arc<Mutex<HashMap<String, f64>>>, ready: mpsc::Sender<u32>) {
    let class_name = win32::wide("LuminanceDimOverlay");
    let empty = win32::wide("");
    // SAFETY: Win32 window management on this thread only; every pointer
    // passed lives until the call returns, the class name for the thread.
    unsafe {
        // Monitor rectangles in physical pixels, matching the window sizes.
        win32::SetThreadDpiAwarenessContext(win32::DPI_PER_MONITOR_AWARE_V2);
        let instance = win32::GetModuleHandleW(ptr::null());
        let class = win32::WNDCLASSEXW {
            cbSize: std::mem::size_of::<win32::WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(overlay_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: ptr::null_mut(),
            hCursor: ptr::null_mut(),
            hbrBackground: win32::GetStockObject(win32::BLACK_BRUSH),
            lpszMenuName: ptr::null(),
            lpszClassName: class_name.as_ptr(),
            hIconSm: ptr::null_mut(),
        };
        win32::RegisterClassExW(&class);

        // Creates the thread's message queue before anyone posts to it.
        let mut msg: win32::MSG = std::mem::zeroed();
        win32::PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, win32::PM_NOREMOVE);
        let _ = ready.send(win32::GetCurrentThreadId());
        win32::SetTimer(ptr::null_mut(), 0, TOPMOST_INTERVAL_MS, ptr::null());

        let mut windows: HashMap<String, win32::HWND> = HashMap::new();
        while win32::GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
            if msg.hwnd.is_null() && msg.message == WM_STOP {
                break;
            }
            if msg.hwnd.is_null() && matches!(msg.message, WM_SYNC | win32::WM_TIMER) {
                let wanted = wanted.lock().unwrap_or_else(|e| e.into_inner()).clone();
                sync_overlays(&mut windows, &wanted, instance, &class_name, &empty);
                continue;
            }
            win32::TranslateMessage(&msg);
            win32::DispatchMessageW(&msg);
        }
        for hwnd in windows.into_values() {
            win32::DestroyWindow(hwnd);
        }
    }
}

/// Creates, moves, updates and removes overlay windows to match `wanted`.
unsafe fn sync_overlays(
    windows: &mut HashMap<String, win32::HWND>,
    wanted: &HashMap<String, f64>,
    instance: win32::HINSTANCE,
    class_name: &[u16],
    title: &[u16],
) {
    let monitors = win32::monitors();
    let dimmed = |device: &str| wanted.get(device).copied().filter(|f| *f < 0.999);

    windows.retain(|device, hwnd| {
        let keep = dimmed(device).is_some() && monitors.iter().any(|m| m.device == *device);
        if !keep {
            win32::DestroyWindow(*hwnd);
        }
        keep
    });

    for m in &monitors {
        let Some(factor) = dimmed(&m.device) else { continue };
        let r = m.rect;
        let hwnd = *windows.entry(m.device.clone()).or_insert_with(|| {
            let hwnd = win32::CreateWindowExW(
                win32::WS_EX_LAYERED
                    | win32::WS_EX_TRANSPARENT
                    | win32::WS_EX_TOPMOST
                    | win32::WS_EX_TOOLWINDOW
                    | win32::WS_EX_NOACTIVATE,
                class_name.as_ptr(),
                title.as_ptr(),
                win32::WS_POPUP,
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                ptr::null_mut(),
                ptr::null_mut(),
                instance,
                ptr::null_mut(),
            );
            // Keep the overlay out of screenshots and screen sharing.
            win32::SetWindowDisplayAffinity(hwnd, win32::WDA_EXCLUDEFROMCAPTURE);
            hwnd
        });
        if hwnd.is_null() {
            windows.remove(&m.device);
            continue;
        }
        let alpha = ((1.0 - factor).clamp(0.0, 1.0) * 255.0).round() as u8;
        win32::SetLayeredWindowAttributes(hwnd, 0, alpha, win32::LWA_ALPHA);
        win32::SetWindowPos(
            hwnd,
            win32::HWND_TOPMOST,
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            win32::SWP_NOACTIVATE | win32::SWP_SHOWWINDOW,
        );
    }
}
