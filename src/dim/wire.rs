//! Minimal Wayland wire-protocol client, replacing `wayland-client` and the
//! generated protocol crates. Luminance sends a couple of dozen request types
//! and reacts to a handful of events, so the wire format is handled directly:
//!
//! ```text
//! u32 object id | u32 (size << 16 | opcode) | arguments, each padded to 4 bytes
//! ```
//!
//! File descriptors travel out of band as `SCM_RIGHTS` ancillary data.
//! Requests are buffered and written by [`Conn::flush`].

use std::collections::VecDeque;
use std::env;
use std::io;
use std::mem;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

pub const DISPLAY: u32 = 1;
const HEADER: usize = 8;
/// libwayland refuses bigger messages.
const MAX_MESSAGE: usize = 4096;
/// libwayland accepts at most this many fds per sendmsg.
const MAX_FDS: usize = 28;

/// Request argument.
pub enum Arg<'a> {
    Uint(u32),
    Int(i32),
    /// Object id; 0 means null.
    Object(u32),
    Str(&'a str),
    Fd(OwnedFd),
}

/// An event addressed to a client object, with the raw argument bytes.
pub struct Event {
    pub sender: u32,
    pub opcode: u16,
    data: Vec<u8>,
}

impl Event {
    pub fn args(&self) -> Args<'_> {
        Args { data: &self.data }
    }
}

/// Sequential reader for event arguments. Out-of-range reads yield `None`.
pub struct Args<'a> {
    data: &'a [u8],
}

impl<'a> Args<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let padded = n.checked_add(3)? & !3;
        if self.data.len() < padded {
            return None;
        }
        let (head, rest) = self.data.split_at(padded);
        self.data = rest;
        Some(&head[..n])
    }

    pub fn uint(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_ne_bytes(b.try_into().unwrap()))
    }

    pub fn string(&mut self) -> Option<String> {
        let len = self.uint()? as usize;
        let bytes = self.take(len)?;
        // Length includes the terminating NUL.
        let bytes = bytes.strip_suffix(&[0]).unwrap_or(bytes);
        Some(String::from_utf8_lossy(bytes).into_owned())
    }
}

pub struct Conn {
    sock: UnixStream,
    out: Vec<u8>,
    out_fds: Vec<OwnedFd>,
    input: Vec<u8>,
    next_id: u32,
    /// Ids released by the server (`wl_display.delete_id`), ready for reuse.
    free_ids: Vec<u32>,
    events: VecDeque<Event>,
}

/// Path of the compositor socket, from `WAYLAND_DISPLAY` (absolute, or
/// relative to `XDG_RUNTIME_DIR`). No `WAYLAND_DISPLAY` means no Wayland
/// session. `WAYLAND_SOCKET` is deliberately ignored: that fd belongs to GTK.
fn socket_path() -> Option<PathBuf> {
    let name = PathBuf::from(env::var_os("WAYLAND_DISPLAY")?);
    if name.is_absolute() {
        return Some(name);
    }
    let dir = PathBuf::from(env::var_os("XDG_RUNTIME_DIR")?);
    dir.is_absolute().then(|| dir.join(name))
}

fn pad4(n: usize) -> usize {
    (n + 3) & !3
}

impl Conn {
    pub fn connect() -> Option<Self> {
        let sock = UnixStream::connect(socket_path()?).ok()?;
        Some(Self {
            sock,
            out: Vec::with_capacity(MAX_MESSAGE),
            out_fds: Vec::new(),
            input: Vec::with_capacity(MAX_MESSAGE),
            next_id: DISPLAY + 1,
            free_ids: Vec::new(),
            events: VecDeque::new(),
        })
    }

    pub fn fd(&self) -> RawFd {
        self.sock.as_raw_fd()
    }

    /// Allocates a client-side object id for a `new_id` argument.
    pub fn new_id(&mut self) -> u32 {
        self.free_ids.pop().unwrap_or_else(|| {
            let id = self.next_id;
            self.next_id += 1;
            id
        })
    }

    /// Queues a request. Returns an error only if the message is malformed.
    pub fn send<'a>(&mut self, object: u32, opcode: u16, args: impl IntoIterator<Item = Arg<'a>>) -> io::Result<()> {
        let start = self.out.len();
        self.out.extend_from_slice(&[0; HEADER]);
        for arg in args {
            match arg {
                Arg::Uint(v) | Arg::Object(v) => self.out.extend_from_slice(&v.to_ne_bytes()),
                Arg::Int(v) => self.out.extend_from_slice(&v.to_ne_bytes()),
                Arg::Str(s) => {
                    let len = s.len() + 1;
                    self.out.extend_from_slice(&(len as u32).to_ne_bytes());
                    self.out.extend_from_slice(s.as_bytes());
                    self.out.resize(self.out.len() + pad4(len) - s.len(), 0);
                }
                Arg::Fd(fd) => self.out_fds.push(fd),
            }
        }
        let size = self.out.len() - start;
        if size > MAX_MESSAGE {
            self.out.truncate(start);
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "wayland message too large"));
        }
        self.out[start..start + 4].copy_from_slice(&object.to_ne_bytes());
        self.out[start + 4..start + 8].copy_from_slice(&((size as u32) << 16 | opcode as u32).to_ne_bytes());
        // Keep the fd count per sendmsg within what the server accepts.
        if self.out_fds.len() >= MAX_FDS {
            self.flush()?;
        }
        Ok(())
    }

    /// Writes all queued requests, passing queued fds with the first chunk.
    pub fn flush(&mut self) -> io::Result<()> {
        let mut sent = 0;
        while sent < self.out.len() {
            let fds: Vec<RawFd> = self.out_fds.iter().map(AsRawFd::as_raw_fd).collect();
            let n = sendmsg(self.fd(), &self.out[sent..], &fds)?;
            // The kernel duplicated the fds; ours can be closed now.
            self.out_fds.clear();
            sent += n;
        }
        self.out.clear();
        Ok(())
    }

    /// Reads whatever is available (blocking if nothing is) and queues the
    /// complete events. Display events are handled here.
    pub fn read(&mut self) -> io::Result<()> {
        let mut buf = [0u8; MAX_MESSAGE];
        let n = recvmsg(self.fd(), &mut buf)?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "compositor closed the connection"));
        }
        self.input.extend_from_slice(&buf[..n]);

        let mut offset = 0;
        while self.input.len() - offset >= HEADER {
            let word = |i: usize| u32::from_ne_bytes(self.input[offset + i..offset + i + 4].try_into().unwrap());
            let sender = word(0);
            let size = (word(4) >> 16) as usize;
            let opcode = (word(4) & 0xFFFF) as u16;
            if size < HEADER {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "malformed wayland message"));
            }
            if self.input.len() - offset < size {
                break;
            }
            let event = Event { sender, opcode, data: self.input[offset + HEADER..offset + size].to_vec() };
            offset += size;
            if sender == DISPLAY {
                self.display_event(&event)?;
            } else {
                self.events.push_back(event);
            }
        }
        self.input.drain(..offset);
        Ok(())
    }

    fn display_event(&mut self, event: &Event) -> io::Result<()> {
        let mut args = event.args();
        match event.opcode {
            0 => {
                let object = args.uint().unwrap_or(0);
                let code = args.uint().unwrap_or(0);
                let message = args.string().unwrap_or_default();
                Err(io::Error::other(format!("protocol error on object {object} (code {code}): {message}")))
            }
            1 => {
                if let Some(id) = args.uint() {
                    self.free_ids.push(id);
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    pub fn next_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    /// `wl_display.sync`: returns the callback id whose `done` event marks
    /// that the server has processed everything sent before it.
    pub fn sync(&mut self) -> io::Result<u32> {
        let id = self.new_id();
        self.send(DISPLAY, 0, [Arg::Uint(id)])?;
        Ok(id)
    }

    /// `wl_display.get_registry`.
    pub fn registry(&mut self) -> io::Result<u32> {
        let id = self.new_id();
        self.send(DISPLAY, 1, [Arg::Uint(id)])?;
        Ok(id)
    }
}

fn sendmsg(fd: RawFd, data: &[u8], fds: &[RawFd]) -> io::Result<usize> {
    let mut iov = libc::iovec { iov_base: data.as_ptr() as *mut _, iov_len: data.len() };
    // u64 elements keep the control buffer aligned for cmsghdr.
    let mut control = [0u64; 32];
    // SAFETY: zeroed msghdr is a valid "no name, no control" header.
    let mut msg: libc::msghdr = unsafe { mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    if !fds.is_empty() {
        let payload = mem::size_of_val(fds) as u32;
        // SAFETY: the control buffer (256 bytes) fits CMSG_SPACE of MAX_FDS fds,
        // and CMSG_FIRSTHDR is non-null because msg_controllen is set.
        unsafe {
            msg.msg_control = control.as_mut_ptr() as *mut _;
            msg.msg_controllen = libc::CMSG_SPACE(payload) as _;
            let cmsg = libc::CMSG_FIRSTHDR(&msg);
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = libc::CMSG_LEN(payload) as _;
            std::ptr::copy_nonoverlapping(fds.as_ptr(), libc::CMSG_DATA(cmsg) as *mut RawFd, fds.len());
        }
    }
    loop {
        // SAFETY: msg points to live buffers for the duration of the call.
        let n = unsafe { libc::sendmsg(fd, &msg, libc::MSG_NOSIGNAL) };
        if n >= 0 {
            return Ok(n as usize);
        }
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::Interrupted {
            return Err(err);
        }
    }
}

/// Receives bytes; any fds the server attaches are closed, since none of the
/// events Luminance handles carry one.
fn recvmsg(fd: RawFd, buf: &mut [u8]) -> io::Result<usize> {
    let mut iov = libc::iovec { iov_base: buf.as_mut_ptr() as *mut _, iov_len: buf.len() };
    let mut control = [0u64; 32];
    // SAFETY: see sendmsg.
    let mut msg: libc::msghdr = unsafe { mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr() as *mut _;
    msg.msg_controllen = mem::size_of_val(&control) as _;
    let n = loop {
        // SAFETY: msg points to live buffers for the duration of the call.
        let n = unsafe { libc::recvmsg(fd, &mut msg, libc::MSG_CMSG_CLOEXEC) };
        if n >= 0 {
            break n as usize;
        }
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::Interrupted {
            return Err(err);
        }
    };
    // SAFETY: iterating the control messages the kernel just filled in.
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                let data = libc::CMSG_DATA(cmsg) as *const RawFd;
                let count = ((*cmsg).cmsg_len as usize - libc::CMSG_LEN(0) as usize) / mem::size_of::<RawFd>();
                for i in 0..count {
                    drop(OwnedFd::from_raw_fd(data.add(i).read_unaligned()));
                }
            }
            cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn pair() -> (Conn, UnixStream) {
        let (a, b) = UnixStream::pair().unwrap();
        let conn = Conn {
            sock: a,
            out: Vec::new(),
            out_fds: Vec::new(),
            input: Vec::new(),
            next_id: 2,
            free_ids: Vec::new(),
            events: VecDeque::new(),
        };
        (conn, b)
    }

    #[test]
    fn encodes_requests() {
        let (mut conn, mut server) = pair();
        conn.send(5, 3, [Arg::Uint(7), Arg::Str("abc"), Arg::Int(-1)]).unwrap();
        conn.flush().unwrap();
        let mut buf = [0u8; 24];
        server.read_exact(&mut buf).unwrap();
        let words: Vec<u32> = buf.chunks(4).map(|c| u32::from_ne_bytes(c.try_into().unwrap())).collect();
        assert_eq!(words[0], 5);
        assert_eq!(words[1], 24 << 16 | 3);
        assert_eq!(words[2], 7);
        assert_eq!(words[3], 4); // "abc\0"
        assert_eq!(&buf[16..20], b"abc\0");
        assert_eq!(words[5] as i32, -1);
    }

    #[test]
    fn decodes_events_and_reuses_ids() {
        let (mut conn, mut server) = pair();
        use std::io::Write;
        let mut msg = |sender: u32, opcode: u32, args: &[u8]| {
            let mut m = sender.to_ne_bytes().to_vec();
            m.extend_from_slice(&(((8 + args.len()) as u32) << 16 | opcode).to_ne_bytes());
            m.extend_from_slice(args);
            server.write_all(&m).unwrap();
        };
        // wl_registry.global(name=3, "wl_output", version=4)
        let mut args = 3u32.to_ne_bytes().to_vec();
        args.extend_from_slice(&10u32.to_ne_bytes());
        args.extend_from_slice(b"wl_output\0\0\0");
        args.extend_from_slice(&4u32.to_ne_bytes());
        msg(2, 0, &args);
        // wl_display.delete_id(9)
        msg(DISPLAY, 1, &9u32.to_ne_bytes());
        conn.read().unwrap();

        let event = conn.next_event().unwrap();
        assert_eq!((event.sender, event.opcode), (2, 0));
        let mut a = event.args();
        assert_eq!(a.uint(), Some(3));
        assert_eq!(a.string().as_deref(), Some("wl_output"));
        assert_eq!(a.uint(), Some(4));
        assert_eq!(a.uint(), None);
        assert!(conn.next_event().is_none());
        assert_eq!(conn.new_id(), 9);
        assert_eq!(conn.new_id(), 2);
    }

    #[test]
    fn protocol_error_is_reported() {
        let (mut conn, mut server) = pair();
        use std::io::Write;
        let mut m = DISPLAY.to_ne_bytes().to_vec();
        m.extend_from_slice(&(24u32 << 16).to_ne_bytes());
        m.extend_from_slice(&[3, 0, 0, 0, 1, 0, 0, 0, 3, 0, 0, 0]);
        m.extend_from_slice(b"no\0\0");
        server.write_all(&m).unwrap();
        let err = conn.read().unwrap_err().to_string();
        assert!(err.contains("object 3") && err.contains("no"), "{err}");
    }
}
