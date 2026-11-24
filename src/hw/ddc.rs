//! Minimal DDC/CI (VESA MCCS) implementation over Linux i2c-dev.
//!
//! Only the two VCP features Luminance needs are used: brightness (0x10) and
//! contrast (0x12). Talking to /dev/i2c-N directly avoids a libddcutil
//! dependency, which keeps the Flatpak small and self contained.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::thread::sleep;
use std::time::Duration;

pub const VCP_BRIGHTNESS: u8 = 0x10;
pub const VCP_CONTRAST: u8 = 0x12;

const I2C_SLAVE: libc::c_ulong = 0x0703;
const DDC_CI_ADDR: libc::c_ulong = 0x37;
const EDID_ADDR: libc::c_ulong = 0x50;
/// 8-bit write address of the display (0x37 << 1), used as checksum seed.
const DISPLAY_ADDR_W: u8 = 0x6E;
/// Virtual host address used as checksum seed for replies.
const HOST_ADDR_R: u8 = 0x50;
/// Source address byte sent by the host.
const HOST_ADDR_SRC: u8 = 0x51;

const REPLY_DELAY: Duration = Duration::from_millis(40);
const WRITE_DELAY: Duration = Duration::from_millis(50);
const RETRIES: usize = 3;

pub struct Ddc {
    file: File,
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

fn open_bus(bus: u32, addr: libc::c_ulong) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(format!("/dev/i2c-{bus}"))?;
    // SAFETY: I2C_SLAVE takes the 7-bit address as an integer argument.
    let rc = unsafe { libc::ioctl(file.as_raw_fd(), I2C_SLAVE as _, addr) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(file)
}

impl Ddc {
    pub fn open(bus: u32) -> io::Result<Self> {
        Ok(Self { file: open_bus(bus, DDC_CI_ADDR)? })
    }

    fn write_message(&mut self, payload: &[u8]) -> io::Result<()> {
        let mut buf = Vec::with_capacity(payload.len() + 3);
        buf.push(HOST_ADDR_SRC);
        buf.push(0x80 | payload.len() as u8);
        buf.extend_from_slice(payload);
        let checksum = buf.iter().fold(DISPLAY_ADDR_W, |acc, b| acc ^ b);
        buf.push(checksum);
        self.file.write_all(&buf)
    }

    fn try_get_vcp(&mut self, code: u8) -> io::Result<(u16, u16)> {
        self.write_message(&[0x01, code])?;
        sleep(REPLY_DELAY);
        // Reply: src, len, 0x02, result, code, type, max_hi, max_lo, cur_hi, cur_lo, checksum
        let mut reply = [0u8; 11];
        self.file.read_exact(&mut reply)?;
        if reply[1] & 0x7F == 0 {
            return Err(invalid("display returned a null message"));
        }
        let checksum = reply[..10].iter().fold(HOST_ADDR_R, |acc, b| acc ^ b);
        if checksum != reply[10] {
            return Err(invalid("bad DDC/CI checksum"));
        }
        if reply[2] != 0x02 || reply[4] != code {
            return Err(invalid("unexpected DDC/CI reply"));
        }
        if reply[3] != 0 {
            return Err(io::Error::new(io::ErrorKind::Unsupported, "VCP feature not supported"));
        }
        let max = u16::from_be_bytes([reply[6], reply[7]]);
        let current = u16::from_be_bytes([reply[8], reply[9]]);
        if max == 0 {
            return Err(invalid("VCP feature reports zero maximum"));
        }
        Ok((current, max))
    }

    /// Returns `(current, maximum)` of a continuous VCP feature.
    pub fn get_vcp(&mut self, code: u8) -> io::Result<(u16, u16)> {
        let mut last_err = invalid("no attempt");
        for _ in 0..RETRIES {
            match self.try_get_vcp(code) {
                Ok(value) => return Ok(value),
                Err(e) if e.kind() == io::ErrorKind::Unsupported => return Err(e),
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied => return Err(e),
                Err(e) => last_err = e,
            }
            sleep(WRITE_DELAY);
        }
        Err(last_err)
    }

    pub fn set_vcp(&mut self, code: u8, value: u16) -> io::Result<()> {
        let [hi, lo] = value.to_be_bytes();
        let result = self.write_message(&[0x03, code, hi, lo]);
        sleep(WRITE_DELAY);
        result
    }
}

/// Reads the 128 byte base EDID block of a display over its DDC bus.
pub fn read_edid(bus: u32) -> io::Result<Vec<u8>> {
    let mut file = open_bus(bus, EDID_ADDR)?;
    file.write_all(&[0])?;
    let mut edid = vec![0u8; 128];
    file.read_exact(&mut edid)?;
    if edid[..8] != [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00] {
        return Err(invalid("no EDID header"));
    }
    Ok(edid)
}
