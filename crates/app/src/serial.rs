//! Serial ports for USB printing: listing ports and a blocking line-oriented connection
//! (Win32 comm API on Windows, `stty` + the device file on Linux and macOS).

use std::io::{Read, Write};
use std::time::Duration;

/// A serial port name with a human description.
#[derive(Clone, Debug)]
pub struct PortInfo {
    pub name: String,
    pub description: String,
    pub usb: bool,
}

/// `serial.tools.list_ports.comports()` (USB-ish devices only, like Cura's port filter).
pub fn list_ports() -> Vec<PortInfo> {
    let mut out = platform::list_ports();
    if let Ok(pattern) = std::env::var("CURA_DEVICENAMES") {
        out.retain(|p| p.name.contains(pattern.trim_matches(|c| c == '^' || c == '$')));
    }
    out
}

/// An open serial connection.
pub struct SerialPort {
    inner: platform::Port,
    buf: Vec<u8>,
}

impl SerialPort {
    pub fn open(name: &str, baud: u32, timeout: Duration) -> Result<SerialPort, String> {
        Ok(SerialPort { inner: platform::Port::open(name, baud, timeout)?, buf: Vec::new() })
    }

    pub fn set_baud(&mut self, baud: u32) -> Result<(), String> {
        self.inner.set_baud(baud)
    }

    pub fn write_all(&mut self, data: &[u8]) -> Result<(), String> {
        self.inner.write_all(data).map_err(|e| e.to_string())
    }

    /// `readline()`: a line up to and including b'\n'; empty when nothing complete arrived before
    /// the timeout.
    pub fn read_line(&mut self, max: usize) -> Result<Vec<u8>, String> {
        loop {
            if let Some(p) = self.buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=p).collect();
                return Ok(line);
            }
            if self.buf.len() >= max {
                return Ok(self.buf.drain(..).collect());
            }
            let mut chunk = [0u8; 256];
            match self.inner.read(&mut chunk) {
                Ok(0) => return Ok(Vec::new()),
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                // A timeout returns nothing; a partial line stays buffered.
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut || e.kind() == std::io::ErrorKind::WouldBlock => return Ok(Vec::new()),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::PortInfo;
    use std::ffi::c_void;
    use std::time::Duration;

    type Handle = *mut c_void;
    const INVALID: Handle = -1isize as Handle;

    #[repr(C)]
    #[derive(Default)]
    struct Dcb {
        dcb_length: u32,
        baud_rate: u32,
        flags: u32,
        w_reserved: u16,
        xon_lim: u16,
        xoff_lim: u16,
        byte_size: u8,
        parity: u8,
        stop_bits: u8,
        xon_char: i8,
        xoff_char: i8,
        error_char: i8,
        eof_char: i8,
        evt_char: i8,
        w_reserved1: u16,
    }

    #[repr(C)]
    #[derive(Default)]
    struct CommTimeouts {
        read_interval: u32,
        read_total_multiplier: u32,
        read_total_constant: u32,
        write_total_multiplier: u32,
        write_total_constant: u32,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateFileW(name: *const u16, access: u32, share: u32, sec: *mut c_void, disposition: u32, flags: u32, template: Handle) -> Handle;
        fn GetCommState(h: Handle, dcb: *mut Dcb) -> i32;
        fn SetCommState(h: Handle, dcb: *const Dcb) -> i32;
        fn SetCommTimeouts(h: Handle, t: *const CommTimeouts) -> i32;
        fn ReadFile(h: Handle, buf: *mut u8, n: u32, read: *mut u32, ov: *mut c_void) -> i32;
        fn WriteFile(h: Handle, buf: *const u8, n: u32, written: *mut u32, ov: *mut c_void) -> i32;
        fn CloseHandle(h: Handle) -> i32;
        fn PurgeComm(h: Handle, flags: u32) -> i32;
        fn EscapeCommFunction(h: Handle, func: u32) -> i32;
    }
    #[link(name = "advapi32")]
    extern "system" {
        fn RegOpenKeyExW(key: isize, sub: *const u16, opts: u32, sam: u32, out: *mut isize) -> i32;
        fn RegEnumValueW(key: isize, index: u32, name: *mut u16, name_len: *mut u32, reserved: *mut u32, ty: *mut u32, data: *mut u8, data_len: *mut u32) -> i32;
        fn RegCloseKey(key: isize) -> i32;
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn list_ports() -> Vec<PortInfo> {
        const HKEY_LOCAL_MACHINE: isize = 0x8000_0002u32 as i32 as isize;
        const KEY_READ: u32 = 0x20019;
        let mut out = Vec::new();
        let mut key: isize = 0;
        // SAFETY: plain Win32 registry calls with valid buffers.
        unsafe {
            if RegOpenKeyExW(HKEY_LOCAL_MACHINE, wide("HARDWARE\\DEVICEMAP\\SERIALCOMM").as_ptr(), 0, KEY_READ, &mut key) != 0 {
                return out;
            }
            for i in 0..256u32 {
                let mut name = [0u16; 256];
                let mut name_len = name.len() as u32;
                let mut data = [0u8; 512];
                let mut data_len = data.len() as u32;
                let mut ty = 0u32;
                let r = RegEnumValueW(key, i, name.as_mut_ptr(), &mut name_len, std::ptr::null_mut(), &mut ty, data.as_mut_ptr(), &mut data_len);
                if r != 0 {
                    break;
                }
                let device = String::from_utf16_lossy(&name[..name_len as usize]);
                let words: Vec<u16> = data[..data_len as usize].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&w| w != 0).collect();
                let port = String::from_utf16_lossy(&words);
                let d = device.to_lowercase();
                let usb = d.contains("usb") || d.contains("vcp") || d.contains("ch34") || d.contains("cp210") || d.contains("silab") || d.contains("ftdi") || d.contains("acm");
                out.push(PortInfo { name: port, description: device, usb });
            }
            RegCloseKey(key);
        }
        out.sort_by_key(|p| p.name.trim_start_matches("COM").parse::<u32>().unwrap_or(999));
        out
    }

    pub struct Port {
        h: Handle,
    }
    // The handle is only used from one thread at a time.
    unsafe impl Send for Port {}

    impl Port {
        pub fn open(name: &str, baud: u32, timeout: Duration) -> Result<Port, String> {
            const GENERIC_READ: u32 = 0x8000_0000;
            const GENERIC_WRITE: u32 = 0x4000_0000;
            const OPEN_EXISTING: u32 = 3;
            let path = if name.starts_with("\\\\") { name.to_string() } else { format!("\\\\.\\{}", name) };
            // SAFETY: Win32 calls with valid arguments; the handle is closed in Drop.
            unsafe {
                let h = CreateFileW(wide(&path).as_ptr(), GENERIC_READ | GENERIC_WRITE, 0, std::ptr::null_mut(), OPEN_EXISTING, 0, std::ptr::null_mut());
                if h == INVALID {
                    return Err(format!("Can't open {} (is another program using it?)", name));
                }
                let port = Port { h };
                port.configure(baud)?;
                let ms = timeout.as_millis().min(u32::MAX as u128) as u32;
                let t = CommTimeouts { read_interval: 50, read_total_multiplier: 0, read_total_constant: ms, write_total_multiplier: 0, write_total_constant: ms };
                SetCommTimeouts(h, &t);
                PurgeComm(h, 0x0008 | 0x0004);
                // Assert DTR (resets most Arduino-based boards, like pyserial).
                EscapeCommFunction(h, 5);
                Ok(port)
            }
        }

        fn configure(&self, baud: u32) -> Result<(), String> {
            // SAFETY: DCB is plain data.
            unsafe {
                let mut dcb = Dcb { dcb_length: std::mem::size_of::<Dcb>() as u32, ..Default::default() };
                if GetCommState(self.h, &mut dcb) == 0 {
                    return Err("GetCommState failed".into());
                }
                dcb.baud_rate = baud;
                // fBinary, DTR and RTS enabled (like pyserial); no parity / flow control.
                dcb.flags = 0x0001 | (1 << 4) | (1 << 12);
                dcb.byte_size = 8;
                dcb.parity = 0;
                dcb.stop_bits = 0;
                if SetCommState(self.h, &dcb) == 0 {
                    return Err(format!("The port does not support {} baud", baud));
                }
            }
            Ok(())
        }

        pub fn set_baud(&mut self, baud: u32) -> Result<(), String> {
            self.configure(baud)
        }
    }

    impl std::io::Read for Port {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let mut n = 0u32;
            // SAFETY: buf is valid for writes of its length.
            let ok = unsafe { ReadFile(self.h, buf.as_mut_ptr(), buf.len() as u32, &mut n, std::ptr::null_mut()) };
            if ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            if n == 0 {
                return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout"));
            }
            Ok(n as usize)
        }
    }

    impl std::io::Write for Port {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let mut n = 0u32;
            // SAFETY: buf is valid for reads of its length.
            let ok = unsafe { WriteFile(self.h, buf.as_ptr(), buf.len() as u32, &mut n, std::ptr::null_mut()) };
            if ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            if n == 0 {
                return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "write timeout"));
            }
            Ok(n as usize)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Drop for Port {
        fn drop(&mut self) {
            // SAFETY: handle from CreateFileW.
            unsafe {
                CloseHandle(self.h);
            }
        }
    }
}

#[cfg(unix)]
mod platform {
    use super::PortInfo;
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Write};
    use std::time::Duration;

    pub fn list_ports() -> Vec<PortInfo> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir("/dev") else { return out };
        let mut names: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        names.sort();
        for n in names {
            let usb = n.starts_with("ttyUSB") || n.starts_with("ttyACM") || n.starts_with("cu.usbserial") || n.starts_with("cu.usbmodem") || n.starts_with("cu.wchusbserial") || n.starts_with("cu.SLAB");
            if usb {
                let description = if n.contains("ACM") || n.contains("usbmodem") { "USB CDC (Marlin, Prusa, Klipper boards)" } else { "USB serial adapter" };
                out.push(PortInfo { name: format!("/dev/{}", n), description: description.into(), usb });
            }
        }
        if let Ok(extra) = std::env::var("CURUST_EXTRA_SERIAL") {
            for p in extra.split(',').filter(|p| !p.is_empty()) {
                out.push(PortInfo { name: p.to_string(), description: "configured port".into(), usb: true });
            }
        }
        out
    }

    fn stty(name: &str, baud: u32) -> Result<(), String> {
        let flag = if cfg!(target_os = "macos") { "-f" } else { "-F" };
        let out = std::process::Command::new("stty").args([flag, name, &baud.to_string(), "raw", "-echo", "-hupcl", "cs8", "-cstopb", "-parenb", "clocal"]).output().map_err(|e| format!("stty: {}", e))?;
        if !out.status.success() {
            return Err(format!("Can't set {} to {} baud: {}", name, baud, String::from_utf8_lossy(&out.stderr).trim()));
        }
        Ok(())
    }

    #[repr(C)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }
    #[cfg(target_os = "linux")]
    type NfdsT = u64;
    #[cfg(not(target_os = "linux"))]
    type NfdsT = u32;
    extern "C" {
        fn poll(fds: *mut PollFd, nfds: NfdsT, timeout: i32) -> i32;
    }
    const POLLIN: i16 = 1;

    /// A serial port read with `poll` and a timeout on the calling thread (no reader thread that
    /// could outlive the port and swallow the next connection's data).
    pub struct Port {
        name: String,
        file: File,
        timeout: Duration,
    }

    impl Port {
        pub fn open(name: &str, baud: u32, timeout: Duration) -> Result<Port, String> {
            stty(name, baud)?;
            let file = OpenOptions::new().read(true).write(true).open(name).map_err(|e| format!("Can't open {}: {}", name, e))?;
            Ok(Port { name: name.to_string(), file, timeout })
        }

        pub fn set_baud(&mut self, baud: u32) -> Result<(), String> {
            stty(&self.name, baud)
        }
    }

    impl Read for Port {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            use std::os::unix::io::AsRawFd;
            let mut p = PollFd { fd: self.file.as_raw_fd(), events: POLLIN, revents: 0 };
            let ms = self.timeout.as_millis().min(i32::MAX as u128) as i32;
            // SAFETY: one valid pollfd.
            let r = unsafe { poll(&mut p, 1, ms) };
            if r < 0 {
                let e = std::io::Error::last_os_error();
                if e.kind() == std::io::ErrorKind::Interrupted {
                    return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout"));
                }
                return Err(e);
            }
            if r == 0 {
                return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout"));
            }
            match self.file.read(buf) {
                Ok(0) => Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "port closed")),
                other => other,
            }
        }
    }

    impl Write for Port {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.file.write(buf)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.file.flush()
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod platform {
    use super::PortInfo;
    pub fn list_ports() -> Vec<PortInfo> {
        Vec::new()
    }
    pub struct Port;
    impl Port {
        pub fn open(_: &str, _: u32, _: std::time::Duration) -> Result<Port, String> {
            Err("serial ports are not supported on this platform".into())
        }
        pub fn set_baud(&mut self, _: u32) -> Result<(), String> {
            Ok(())
        }
    }
    impl std::io::Read for Port {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Ok(0)
        }
    }
    impl std::io::Write for Port {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
}

#[allow(dead_code)]
fn _assert_io<T: Read + Write>() {}
