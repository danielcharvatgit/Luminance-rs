//! Software dimming on wlroots-style compositors and KWin.
//!
//! Two techniques, picked per output:
//!  * `zwlr_gamma_control_v1` (Hyprland, Sway, river, ...): scales the gamma
//!    ramp, which dims everything including the cursor.
//!  * `zwlr_layer_shell_v1` overlay (KWin/Plasma, Hyprland, ...): a
//!    click-through, translucent black surface on the overlay layer.
//!
//! Both only last as long as this connection is alive, so the effect
//! disappears automatically if the app exits or crashes.
//!
//! The protocol objects live on a dedicated thread with its own Wayland
//! connection (see [`super::wire`]), independent of GTK's.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use super::wire::{Arg, Conn, Event};
use super::Technique;

#[derive(Clone, Debug, Default)]
pub struct Capabilities {
    pub gamma: bool,
    pub overlay: bool,
}

enum Command {
    /// Connector name, or "*" for every output.
    Set(String, f64),
    SetTechnique(Technique),
    Quit,
}

pub struct WaylandDimmer {
    tx: Sender<Command>,
    /// Writing a byte wakes the dimmer thread out of poll(2).
    wake: UnixStream,
    thread: Mutex<Option<JoinHandle<()>>>,
    outputs: Arc<Mutex<Vec<String>>>,
    pub caps: Capabilities,
}

impl WaylandDimmer {
    pub fn start(technique: Technique) -> Option<Self> {
        let conn = Conn::connect()?;
        let (wake, wake_rx) = UnixStream::pair().ok()?;
        wake.set_nonblocking(true).ok()?;
        wake_rx.set_nonblocking(true).ok()?;
        let outputs = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);

        let thread_outputs = outputs.clone();
        let thread = thread::Builder::new()
            .name("luminance-wayland".into())
            .spawn(move || {
                if let Err(e) = run(conn, rx, wake_rx, thread_outputs, technique, ready_tx) {
                    eprintln!("luminance: wayland dimmer stopped: {e}");
                }
            })
            .ok()?;

        let caps = ready_rx.recv().ok()??;
        Some(Self { tx, wake, thread: Mutex::new(Some(thread)), outputs, caps })
    }

    fn send(&self, cmd: Command) {
        if self.tx.send(cmd).is_ok() {
            // A full socket buffer already means "wake up": ignore WouldBlock.
            let _ = (&self.wake).write(&[1]);
        }
    }

    pub fn set(&self, target: &str, factor: f64) {
        self.send(Command::Set(target.to_string(), factor));
    }

    pub fn set_technique(&self, technique: Technique) {
        self.send(Command::SetTechnique(technique));
    }

    pub fn outputs(&self) -> Vec<String> {
        self.outputs.lock().map(|o| o.clone()).unwrap_or_default()
    }

    pub fn shutdown(&self) {
        self.send(Command::Quit);
        if let Some(t) = self.thread.lock().ok().and_then(|mut t| t.take()) {
            let _ = t.join();
        }
    }
}

// Request opcodes (from the protocol XML files).
mod op {
    pub const REGISTRY_BIND: u16 = 0;
    pub const COMPOSITOR_CREATE_SURFACE: u16 = 0;
    pub const COMPOSITOR_CREATE_REGION: u16 = 1;
    pub const SHM_CREATE_POOL: u16 = 0;
    pub const SHM_POOL_CREATE_BUFFER: u16 = 0;
    pub const SHM_POOL_DESTROY: u16 = 1;
    pub const BUFFER_DESTROY: u16 = 0;
    pub const SURFACE_DESTROY: u16 = 0;
    pub const SURFACE_ATTACH: u16 = 1;
    pub const SURFACE_DAMAGE: u16 = 2;
    pub const SURFACE_SET_INPUT_REGION: u16 = 5;
    pub const SURFACE_COMMIT: u16 = 6;
    pub const REGION_DESTROY: u16 = 0;
    pub const OUTPUT_RELEASE: u16 = 0;
    pub const VIEWPORTER_GET_VIEWPORT: u16 = 1;
    pub const VIEWPORT_DESTROY: u16 = 0;
    pub const VIEWPORT_SET_DESTINATION: u16 = 2;
    pub const LAYER_SHELL_GET_LAYER_SURFACE: u16 = 0;
    pub const LAYER_SURFACE_SET_SIZE: u16 = 0;
    pub const LAYER_SURFACE_SET_ANCHOR: u16 = 1;
    pub const LAYER_SURFACE_SET_EXCLUSIVE_ZONE: u16 = 2;
    pub const LAYER_SURFACE_SET_KEYBOARD_INTERACTIVITY: u16 = 4;
    pub const LAYER_SURFACE_ACK_CONFIGURE: u16 = 6;
    pub const LAYER_SURFACE_DESTROY: u16 = 7;
    pub const GAMMA_MANAGER_GET_GAMMA_CONTROL: u16 = 0;
    pub const GAMMA_CONTROL_SET_GAMMA: u16 = 0;
    pub const GAMMA_CONTROL_DESTROY: u16 = 1;
}

const LAYER_OVERLAY: u32 = 3;
/// top | bottom | left | right
const ANCHOR_ALL: u32 = 1 | 2 | 4 | 8;
const KEYBOARD_INTERACTIVITY_NONE: u32 = 0;
const SHM_FORMAT_ARGB8888: u32 = 0;

/// What a client object id refers to, for routing events. Objects whose
/// events are irrelevant (surfaces, shm, ...) are simply not tracked.
#[derive(Clone, Copy)]
enum Obj {
    Registry,
    Callback,
    /// Payload: registry name of the output global.
    Output(u32),
    Gamma(u32),
    LayerSurface(u32),
    Buffer,
}

struct Gamma {
    id: u32,
    size: u32,
}

struct Overlay {
    surface: u32,
    layer: u32,
    viewport: Option<u32>,
    size: Option<(u32, u32)>,
}

struct Output {
    global: u32,
    id: u32,
    version: u32,
    name: Option<String>,
    factor: f64,
    gamma: Option<Gamma>,
    gamma_failed: bool,
    overlay: Option<Overlay>,
}

struct State {
    conn: Conn,
    objects: HashMap<u32, Obj>,
    registry: u32,
    technique: Technique,
    compositor: Option<u32>,
    shm: Option<u32>,
    viewporter: Option<u32>,
    layer_shell: Option<u32>,
    gamma_manager: Option<u32>,
    outputs: Vec<Output>,
    /// Desired factors by connector name ("*" = all), applied as outputs appear.
    desired: HashMap<String, f64>,
    shared_outputs: Arc<Mutex<Vec<String>>>,
}

fn memfd(name: &std::ffi::CStr, contents: &[u8]) -> io::Result<OwnedFd> {
    // SAFETY: memfd_create with a valid NUL-terminated name.
    let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fd was just created and is owned by nobody else.
    let mut file = unsafe { File::from_raw_fd(fd) };
    file.write_all(contents)?;
    // Some compositors read() the ramp from the current offset.
    file.rewind()?;
    Ok(file.into())
}

/// Linear ramp scaled by `factor`, repeated for red, green and blue.
fn gamma_ramp(size: u32, factor: f64) -> Vec<u8> {
    let n = size as usize;
    let channel: Vec<u8> = (0..n)
        .flat_map(|i| {
            let linear = if n > 1 { i as f64 / (n - 1) as f64 } else { 1.0 };
            ((linear * factor * 65535.0).round() as u16).to_ne_bytes()
        })
        .collect();
    channel.repeat(3)
}

impl State {
    /// Sends a request; encoding errors are programming errors and only logged.
    fn req<'a>(&mut self, object: u32, opcode: u16, args: impl IntoIterator<Item = Arg<'a>>) {
        if let Err(e) = self.conn.send(object, opcode, args) {
            eprintln!("luminance: wayland request: {e}");
        }
    }

    /// Sends a request that creates an object and returns the new id.
    fn create<'a>(&mut self, object: u32, opcode: u16, args: impl IntoIterator<Item = Arg<'a>>) -> u32 {
        let id = self.conn.new_id();
        self.req(object, opcode, std::iter::once(Arg::Uint(id)).chain(args));
        id
    }

    /// Sends a destructor request and forgets the object.
    fn destroy(&mut self, object: u32, opcode: u16) {
        self.objects.remove(&object);
        self.req(object, opcode, []);
    }

    fn bind(&mut self, name: u32, interface: &str, version: u32) -> u32 {
        let id = self.conn.new_id();
        self.req(self.registry, op::REGISTRY_BIND, [Arg::Uint(name), Arg::Str(interface), Arg::Uint(version), Arg::Uint(id)]);
        id
    }

    fn destroy_overlay(&mut self, o: Overlay) {
        if let Some(v) = o.viewport {
            self.destroy(v, op::VIEWPORT_DESTROY);
        }
        self.destroy(o.layer, op::LAYER_SURFACE_DESTROY);
        self.destroy(o.surface, op::SURFACE_DESTROY);
    }

    fn publish_outputs(&self) {
        if let Ok(mut shared) = self.shared_outputs.lock() {
            *shared = self.outputs.iter().filter_map(|o| o.name.clone()).collect();
        }
    }

    fn desired_for(&self, name: Option<&str>) -> f64 {
        name.and_then(|n| self.desired.get(n))
            .or_else(|| self.desired.get("*"))
            .copied()
            .unwrap_or(1.0)
    }

    fn use_gamma(&self, output: &Output) -> bool {
        self.technique != Technique::OverlayOnly && self.gamma_manager.is_some() && !output.gamma_failed
    }

    fn use_overlay(&self, output: &Output) -> bool {
        let gamma_possible = self.use_gamma(output);
        (match self.technique {
            Technique::GammaOnly => false,
            Technique::OverlayOnly => true,
            Technique::Auto => !gamma_possible,
        }) && self.layer_shell.is_some()
            && self.compositor.is_some()
            && self.shm.is_some()
    }

    fn apply(&mut self, index: usize) {
        let factor = self.outputs[index].factor;
        let dimmed = factor < 0.999;
        let want_gamma = dimmed && self.use_gamma(&self.outputs[index]);
        let want_overlay = dimmed && self.use_overlay(&self.outputs[index]);

        if !want_gamma {
            if let Some(g) = self.outputs[index].gamma.take() {
                self.destroy(g.id, op::GAMMA_CONTROL_DESTROY); // compositor restores the original ramp
            }
        }
        if !want_overlay {
            if let Some(o) = self.outputs[index].overlay.take() {
                self.destroy_overlay(o);
            }
        }

        if want_gamma {
            let output = &self.outputs[index];
            match &output.gamma {
                // The size arrives with the first event; until then, wait.
                Some(g) if g.size == 0 => {}
                Some(g) => {
                    let id = g.id;
                    match memfd(c"luminance-gamma", &gamma_ramp(g.size, factor)) {
                        Ok(fd) => self.req(id, op::GAMMA_CONTROL_SET_GAMMA, [Arg::Fd(fd)]),
                        Err(e) => eprintln!("luminance: gamma ramp: {e}"),
                    }
                }
                None => {
                    let (wl, global) = (output.id, output.global);
                    let manager = self.gamma_manager.unwrap();
                    let id = self.create(manager, op::GAMMA_MANAGER_GET_GAMMA_CONTROL, [Arg::Object(wl)]);
                    self.objects.insert(id, Obj::Gamma(global));
                    self.outputs[index].gamma = Some(Gamma { id, size: 0 });
                }
            }
        }

        if want_overlay {
            if self.outputs[index].overlay.is_some() {
                self.draw_overlay(index);
                return;
            }
            let (wl, global) = (self.outputs[index].id, self.outputs[index].global);
            let compositor = self.compositor.unwrap();
            let surface = self.create(compositor, op::COMPOSITOR_CREATE_SURFACE, []);
            let layer = self.create(
                self.layer_shell.unwrap(),
                op::LAYER_SHELL_GET_LAYER_SURFACE,
                [Arg::Object(surface), Arg::Object(wl), Arg::Uint(LAYER_OVERLAY), Arg::Str("luminance-dim")],
            );
            self.objects.insert(layer, Obj::LayerSurface(global));
            self.req(layer, op::LAYER_SURFACE_SET_ANCHOR, [Arg::Uint(ANCHOR_ALL)]);
            self.req(layer, op::LAYER_SURFACE_SET_SIZE, [Arg::Uint(0), Arg::Uint(0)]);
            self.req(layer, op::LAYER_SURFACE_SET_EXCLUSIVE_ZONE, [Arg::Int(-1)]);
            self.req(layer, op::LAYER_SURFACE_SET_KEYBOARD_INTERACTIVITY, [Arg::Uint(KEYBOARD_INTERACTIVITY_NONE)]);
            // Empty input region: every click goes through to the windows below.
            let region = self.create(compositor, op::COMPOSITOR_CREATE_REGION, []);
            self.req(surface, op::SURFACE_SET_INPUT_REGION, [Arg::Object(region)]);
            self.req(region, op::REGION_DESTROY, []);
            let viewport = self
                .viewporter
                .map(|v| self.create(v, op::VIEWPORTER_GET_VIEWPORT, [Arg::Object(surface)]));
            self.req(surface, op::SURFACE_COMMIT, []);
            self.outputs[index].overlay = Some(Overlay { surface, layer, viewport, size: None });
        }
    }

    fn draw_overlay(&mut self, index: usize) {
        let Some(shm) = self.shm else { return };
        let output = &self.outputs[index];
        let Some(overlay) = &output.overlay else { return };
        let Some((width, height)) = overlay.size else { return };
        let (surface, viewport) = (overlay.surface, overlay.viewport);
        let alpha = ((1.0 - output.factor).clamp(0.0, 1.0) * 255.0).round() as u32;
        // Premultiplied ARGB8888: opaque black scaled by alpha is just the alpha byte.
        let pixel = (alpha << 24).to_le_bytes();
        // With a viewport, a single pixel is scaled up to the whole output.
        let (bw, bh) = if viewport.is_some() { (1, 1) } else { (width.max(1), height.max(1)) };
        let len = (bw * bh * 4) as usize;
        let Ok(fd) = memfd(c"luminance-overlay", &pixel.repeat(len / 4)) else { return };

        let pool = self.create(shm, op::SHM_CREATE_POOL, [Arg::Fd(fd), Arg::Int(len as i32)]);
        let buffer = self.create(
            pool,
            op::SHM_POOL_CREATE_BUFFER,
            [
                Arg::Int(0),
                Arg::Int(bw as i32),
                Arg::Int(bh as i32),
                Arg::Int((bw * 4) as i32),
                Arg::Uint(SHM_FORMAT_ARGB8888),
            ],
        );
        self.objects.insert(buffer, Obj::Buffer);
        self.req(pool, op::SHM_POOL_DESTROY, []);
        if let Some(v) = viewport {
            self.req(v, op::VIEWPORT_SET_DESTINATION, [Arg::Int(width as i32), Arg::Int(height as i32)]);
        }
        self.req(surface, op::SURFACE_ATTACH, [Arg::Object(buffer), Arg::Int(0), Arg::Int(0)]);
        self.req(surface, op::SURFACE_DAMAGE, [Arg::Int(0), Arg::Int(0), Arg::Int(i32::MAX), Arg::Int(i32::MAX)]);
        self.req(surface, op::SURFACE_COMMIT, []);
    }

    fn set(&mut self, target: &str, factor: f64) {
        let factor = factor.clamp(0.0, 1.0);
        if target == "*" {
            self.desired.clear();
        }
        self.desired.insert(target.to_string(), factor);
        for i in 0..self.outputs.len() {
            let matches = target == "*" || self.outputs[i].name.as_deref() == Some(target);
            if matches && (self.outputs[i].factor - factor).abs() > f64::EPSILON {
                self.outputs[i].factor = factor;
                self.apply(i);
            }
        }
    }

    fn reset_all(&mut self) {
        self.desired.clear();
        for i in 0..self.outputs.len() {
            self.outputs[i].factor = 1.0;
            self.apply(i);
        }
    }

    fn output_index(&self, global: u32) -> Option<usize> {
        self.outputs.iter().position(|o| o.global == global)
    }

    /// Handles all queued events. Returns the ids of `wl_callback`s that fired.
    fn dispatch(&mut self) -> Vec<u32> {
        let mut done = Vec::new();
        while let Some(event) = self.conn.next_event() {
            let Some(&obj) = self.objects.get(&event.sender) else { continue };
            match obj {
                Obj::Registry => self.registry_event(&event),
                Obj::Callback => {
                    self.objects.remove(&event.sender);
                    done.push(event.sender);
                }
                Obj::Output(global) => self.output_event(global, &event),
                Obj::Gamma(global) => self.gamma_event(global, &event),
                Obj::LayerSurface(global) => self.layer_event(global, &event),
                Obj::Buffer => {
                    // wl_buffer.release: the compositor is done with it.
                    if event.opcode == 0 {
                        self.destroy(event.sender, op::BUFFER_DESTROY);
                    }
                }
            }
        }
        done
    }

    fn registry_event(&mut self, event: &Event) {
        let mut args = event.args();
        let Some(name) = args.uint() else { return };
        match event.opcode {
            0 => {
                let (Some(interface), Some(version)) = (args.string(), args.uint()) else { return };
                match interface.as_str() {
                    "wl_output" => {
                        let version = version.min(4);
                        let id = self.bind(name, &interface, version);
                        self.objects.insert(id, Obj::Output(name));
                        self.outputs.push(Output {
                            global: name,
                            id,
                            version,
                            name: None,
                            factor: 1.0,
                            gamma: None,
                            gamma_failed: false,
                            overlay: None,
                        });
                    }
                    "wl_compositor" => self.compositor = Some(self.bind(name, &interface, version.min(4))),
                    "wl_shm" => self.shm = Some(self.bind(name, &interface, 1)),
                    "wp_viewporter" => self.viewporter = Some(self.bind(name, &interface, 1)),
                    "zwlr_layer_shell_v1" => self.layer_shell = Some(self.bind(name, &interface, version.min(4))),
                    "zwlr_gamma_control_manager_v1" => self.gamma_manager = Some(self.bind(name, &interface, 1)),
                    _ => {}
                }
            }
            1 => {
                let Some(i) = self.output_index(name) else { return };
                let output = self.outputs.remove(i);
                if let Some(g) = output.gamma {
                    self.destroy(g.id, op::GAMMA_CONTROL_DESTROY);
                }
                if let Some(o) = output.overlay {
                    self.destroy_overlay(o);
                }
                if output.version >= 3 {
                    self.destroy(output.id, op::OUTPUT_RELEASE);
                } else {
                    self.objects.remove(&output.id);
                }
                self.publish_outputs();
            }
            _ => {}
        }
    }

    fn output_event(&mut self, global: u32, event: &Event) {
        // wl_output.name (since version 4)
        if event.opcode != 4 {
            return;
        }
        let Some(name) = event.args().string() else { return };
        let Some(i) = self.output_index(global) else { return };
        let factor = self.desired_for(Some(&name));
        self.outputs[i].name = Some(name);
        self.publish_outputs();
        if (self.outputs[i].factor - factor).abs() > f64::EPSILON {
            self.outputs[i].factor = factor;
            self.apply(i);
        }
    }

    fn gamma_event(&mut self, global: u32, event: &Event) {
        let Some(i) = self.output_index(global) else { return };
        match event.opcode {
            // gamma_size
            0 => {
                let size = event.args().uint().unwrap_or(0);
                if let Some(g) = &mut self.outputs[i].gamma {
                    g.size = size;
                }
                self.apply(i);
            }
            // failed: another client (e.g. a night light tool) owns the ramp,
            // or the output has no gamma support: fall back to the overlay.
            1 => {
                self.destroy(event.sender, op::GAMMA_CONTROL_DESTROY);
                self.outputs[i].gamma = None;
                self.outputs[i].gamma_failed = true;
                self.apply(i);
            }
            _ => {}
        }
    }

    fn layer_event(&mut self, global: u32, event: &Event) {
        let Some(i) = self.output_index(global) else { return };
        match event.opcode {
            // configure(serial, width, height)
            0 => {
                let mut args = event.args();
                let (Some(serial), Some(width), Some(height)) = (args.uint(), args.uint(), args.uint()) else { return };
                self.req(event.sender, op::LAYER_SURFACE_ACK_CONFIGURE, [Arg::Uint(serial)]);
                if let Some(o) = &mut self.outputs[i].overlay {
                    o.size = Some((width, height));
                }
                self.draw_overlay(i);
            }
            // closed
            1 => {
                if let Some(o) = self.outputs[i].overlay.take() {
                    self.destroy_overlay(o);
                }
            }
            _ => {}
        }
    }

    /// Sends everything and processes events until the server has handled it.
    fn roundtrip(&mut self) -> io::Result<()> {
        let callback = self.conn.sync()?;
        self.objects.insert(callback, Obj::Callback);
        self.conn.flush()?;
        loop {
            self.conn.read()?;
            let done = self.dispatch();
            self.conn.flush()?;
            if done.contains(&callback) {
                return Ok(());
            }
        }
    }
}

fn poll_in(fds: &[i32]) -> io::Result<Vec<i16>> {
    let mut pollfds: Vec<libc::pollfd> =
        fds.iter().map(|&fd| libc::pollfd { fd, events: libc::POLLIN, revents: 0 }).collect();
    // SAFETY: pollfds is a valid array of pollfd structs.
    let rc = unsafe { libc::poll(pollfds.as_mut_ptr(), pollfds.len() as _, -1) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(pollfds.iter().map(|p| p.revents).collect())
}

fn run(
    conn: Conn,
    rx: Receiver<Command>,
    mut wake: UnixStream,
    shared_outputs: Arc<Mutex<Vec<String>>>,
    technique: Technique,
    ready: mpsc::SyncSender<Option<Capabilities>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut state = State {
        conn,
        objects: HashMap::new(),
        registry: 0,
        technique,
        compositor: None,
        shm: None,
        viewporter: None,
        layer_shell: None,
        gamma_manager: None,
        outputs: Vec::new(),
        desired: HashMap::new(),
        shared_outputs,
    };

    // First roundtrip binds globals, second collects output names.
    let init = state
        .conn
        .registry()
        .and_then(|registry| {
            state.registry = registry;
            state.objects.insert(registry, Obj::Registry);
            state.roundtrip()
        })
        .and_then(|_| state.roundtrip());
    if let Err(e) = init {
        let _ = ready.send(None);
        return Err(e.into());
    }
    let caps = Capabilities {
        gamma: state.gamma_manager.is_some(),
        overlay: state.layer_shell.is_some() && state.compositor.is_some() && state.shm.is_some(),
    };
    let usable = caps.gamma || caps.overlay;
    let _ = ready.send(usable.then_some(caps));
    if !usable {
        return Ok(());
    }

    loop {
        for cmd in rx.try_iter() {
            match cmd {
                Command::Set(target, factor) => state.set(&target, factor),
                Command::SetTechnique(t) => {
                    let saved: Vec<f64> = state.outputs.iter().map(|o| o.factor).collect();
                    state.reset_all();
                    state.technique = t;
                    for (i, f) in saved.into_iter().enumerate() {
                        state.outputs[i].factor = f;
                        state.apply(i);
                    }
                }
                Command::Quit => {
                    state.reset_all();
                    let _ = state.roundtrip();
                    return Ok(());
                }
            }
        }
        state.conn.flush()?;

        let revents = match poll_in(&[state.conn.fd(), wake.as_raw_fd()]) {
            Ok(r) => r,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        };
        if revents[0] & libc::POLLIN != 0 {
            state.conn.read()?;
            state.dispatch();
        } else if revents[0] & (libc::POLLHUP | libc::POLLERR) != 0 {
            return Err("compositor connection closed".into());
        }
        if revents[1] & libc::POLLIN != 0 {
            let mut buf = [0u8; 64];
            while matches!(wake.read(&mut buf), Ok(n) if n > 0) {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::gamma_ramp;

    #[test]
    fn gamma_ramp_is_scaled_and_repeated() {
        let ramp = gamma_ramp(3, 0.5);
        assert_eq!(ramp.len(), 3 * 3 * 2);
        let values: Vec<u16> = ramp.chunks(2).map(|c| u16::from_ne_bytes([c[0], c[1]])).collect();
        assert_eq!(&values[..3], &[0, 16384, 32768]);
        assert_eq!(&values[..3], &values[3..6]);
        assert_eq!(&values[..3], &values[6..]);
    }
}
