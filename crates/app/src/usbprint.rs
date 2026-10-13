//! Printing over USB: a port of Cura's USBPrinting plugin (`USBPrinterOutputDevice`,
//! `AutoDetectBaudJob`): numbered, checksummed lines sent one per "ok", temperature polling
//! with M105, M115 firmware detection, resend handling, pause / resume / abort.

use crate::serial::SerialPort;
use std::collections::VecDeque;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const BAUD_RATES: [u32; 9] = [115200, 250000, 500000, 230400, 76800, 57600, 38400, 19200, 9600];

#[derive(Clone, Debug, Default)]
pub struct Temperature {
    pub current: f64,
    pub target: f64,
}

/// What the Monitor shows.
#[derive(Clone, Debug, Default)]
pub struct UsbState {
    pub port: String,
    pub baud: u32,
    /// "connecting", "connected", "error", "closed"
    pub status: String,
    pub message: String,
    pub firmware: String,
    pub hotends: Vec<Temperature>,
    pub bed: Temperature,
    pub printing: bool,
    pub paused: bool,
    pub job_name: String,
    pub line: usize,
    pub lines: usize,
    pub elapsed: u64,
    pub estimated: u64,
    pub console: VecDeque<String>,
}

enum Cmd {
    Print { gcode: String, name: String, estimated: u64 },
    Pause,
    Resume,
    Cancel,
    Send(String),
    Close,
}

pub struct UsbPrinter {
    pub state: Arc<Mutex<UsbState>>,
    tx: Sender<Cmd>,
}

impl UsbPrinter {
    /// Connect (auto-detecting the baud rate when `baud` is None) in a background thread.
    pub fn connect(port: &str, baud: Option<u32>, extruders: usize) -> UsbPrinter {
        let state = Arc::new(Mutex::new(UsbState { port: port.to_string(), status: "connecting".into(), hotends: vec![Temperature::default(); extruders.max(1)], ..Default::default() }));
        let (tx, rx) = channel();
        let st = state.clone();
        let port = port.to_string();
        std::thread::Builder::new()
            .name("USBPrinterUpdate".into())
            .spawn(move || {
                let r = run(&port, baud, &st, &rx);
                let mut s = st.lock().unwrap();
                s.printing = false;
                match r {
                    Ok(()) => s.status = "closed".into(),
                    Err(e) => {
                        s.status = "error".into();
                        s.message = e;
                    }
                }
            })
            .ok();
        UsbPrinter { state, tx }
    }

    pub fn print(&self, gcode: String, name: &str, estimated: u64) -> Result<(), String> {
        let s = self.state.lock().unwrap();
        if s.printing {
            return Err("A print is still in progress. Curust cannot start another print via USB until the previous print has completed.".into());
        }
        if s.status != "connected" {
            return Err(format!("The printer is not connected ({}).", s.status));
        }
        drop(s);
        {
            let mut s = self.state.lock().unwrap();
            s.printing = true;
            s.paused = false;
            s.job_name = name.to_string();
            s.line = 0;
            s.lines = gcode.matches('\n').count() + 2;
        }
        self.tx.send(Cmd::Print { gcode, name: name.to_string(), estimated }).map_err(|_| "The USB connection is closed.".to_string())
    }
    pub fn pause(&self) {
        let _ = self.tx.send(Cmd::Pause);
    }
    pub fn resume(&self) {
        let _ = self.tx.send(Cmd::Resume);
    }
    pub fn cancel(&self) {
        let _ = self.tx.send(Cmd::Cancel);
    }
    pub fn send(&self, gcode: &str) {
        for l in gcode.lines().map(|l| l.trim()).filter(|l| !l.is_empty()) {
            let _ = self.tx.send(Cmd::Send(l.to_string()));
        }
    }
    pub fn close(&self) {
        let _ = self.tx.send(Cmd::Close);
    }
}

impl Drop for UsbPrinter {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Close);
    }
}

/// `AutoDetectBaudJob.run`.
fn detect_baud(port: &str, st: &Arc<Mutex<UsbState>>) -> Result<u32, String> {
    let wait_response_timeouts = [3.0, 15.0, 30.0];
    let wait_bootloader_times = [1.5, 5.0, 15.0];
    let mut serial: Option<SerialPort> = None;
    for retry in 0..2 {
        for &baud in BAUD_RATES.iter() {
            st.lock().unwrap().message = format!("Trying {} baud…", baud);
            match &mut serial {
                None => match SerialPort::open(port, baud, Duration::from_secs(3)) {
                    Ok(s) => serial = Some(s),
                    Err(_) => continue,
                },
                Some(s) => {
                    if s.set_baud(baud).is_err() {
                        continue;
                    }
                }
            }
            let s = serial.as_mut().unwrap();
            std::thread::sleep(Duration::from_secs_f64(wait_bootloader_times[retry]));
            let _ = s.write_all(b"\n");
            let _ = s.write_all(b"M105\n");
            let deadline = Instant::now() + Duration::from_secs_f64(wait_response_timeouts[retry]);
            while Instant::now() < deadline {
                let line = s.read_line(100)?;
                if contains(&line, b"ok") && contains(&line, b"T:") {
                    return Ok(baud);
                }
                let _ = s.write_all(b"M105\n");
            }
        }
        std::thread::sleep(Duration::from_secs(if std::env::var_os("CURUST_FAST_DETECT").is_some() { 1 } else { 15 }));
    }
    Err("Unable to find a working baud rate. Is the printer switched on, and is the port right?".into())
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// Parse `T:200.0 /210.0 B:60.0 /60.0 T1:...` like `_update`.
fn parse_temperatures(line: &str, s: &mut UsbState) {
    let bytes = line.as_bytes();
    let mut seen: Vec<usize> = Vec::new();
    let mut k = 0;
    // T(\d*): ?(\d+\.?\d*)\s*\/?(\d+\.?\d*)?
    let number = |b: &[u8], mut j: usize| -> (Option<f64>, usize) {
        let start = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j == start {
            return (None, start);
        }
        if j < b.len() && b[j] == b'.' {
            j += 1;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
        }
        (std::str::from_utf8(&b[start..j]).ok().and_then(|x| x.parse().ok()), j)
    };
    while k < bytes.len() {
        let c = bytes[k];
        if c == b'T' || c == b'B' {
            let mut j = k + 1;
            let ds = j;
            while c == b'T' && j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            let idx: Option<usize> = if c == b'T' { if j > ds { std::str::from_utf8(&bytes[ds..j]).ok().and_then(|x| x.parse().ok()) } else { Some(0) } } else { None };
            if j < bytes.len() && bytes[j] == b':' {
                j += 1;
                if j < bytes.len() && bytes[j] == b' ' {
                    j += 1;
                }
                let (cur, mut e) = number(bytes, j);
                if let Some(cur) = cur {
                    while e < bytes.len() && (bytes[e] == b' ' || bytes[e] == b'\t') {
                        e += 1;
                    }
                    if e < bytes.len() && bytes[e] == b'/' {
                        e += 1;
                    }
                    let (target, e2) = number(bytes, e);
                    match idx {
                        Some(i) => {
                            if !seen.contains(&i) {
                                seen.push(i);
                                if let Some(h) = s.hotends.get_mut(i) {
                                    h.current = cur;
                                    if let Some(t) = target {
                                        h.target = t;
                                    }
                                }
                            }
                        }
                        None => {
                            s.bed.current = cur;
                            if let Some(t) = target {
                                s.bed.target = t;
                            }
                        }
                    }
                    k = e2.max(k + 1);
                    continue;
                }
            }
        }
        k += 1;
    }
}

struct Device<'a> {
    serial: SerialPort,
    st: &'a Arc<Mutex<UsbState>>,
    command_received: bool,
    queue: VecDeque<String>,
    gcode: Vec<String>,
    position: usize,
    printing: bool,
    paused: bool,
    start: Instant,
    estimated: u64,
}

impl Device<'_> {
    fn log(&self, dir: &str, line: &str) {
        let mut s = self.st.lock().unwrap();
        s.console.push_back(format!("{} {}", dir, line.trim_end()));
        while s.console.len() > 300 {
            s.console.pop_front();
        }
    }

    /// `sendCommand`: queue while a command is outstanding.
    fn send_command(&mut self, cmd: &str) -> Result<(), String> {
        if !self.command_received {
            self.queue.push_back(cmd.to_string());
            Ok(())
        } else {
            self.send_raw(cmd)
        }
    }

    /// `_sendCommand`.
    fn send_raw(&mut self, cmd: &str) -> Result<(), String> {
        let mut b = cmd.as_bytes().to_vec();
        if !b.ends_with(b"\n") {
            b.push(b'\n');
        }
        self.command_received = false;
        if !cmd.starts_with('N') || std::env::var_os("CURUST_USB_LOG_ALL").is_some() {
            self.log(">", cmd);
        }
        self.serial.write_all(&b)
    }

    /// `_sendNextGcodeLine`.
    fn send_next_line(&mut self) -> Result<(), String> {
        let Some(line) = self.gcode.get(self.position).cloned() else {
            self.printing = false;
            let mut s = self.st.lock().unwrap();
            s.printing = false;
            s.line = s.lines;
            return Ok(());
        };
        let mut line = match line.find(';') {
            Some(p) => line[..p].to_string(),
            None => line,
        };
        line = line.trim().to_string();
        if line.is_empty() || line == "M0" || line == "M1" {
            line = "M105".into();
        }
        let body = format!("N{}{}", self.position, line);
        let checksum = body.bytes().fold(0u8, |a, b| a ^ b);
        self.send_raw(&format!("{}*{}", body, checksum))?;
        let progress = self.position as f64 / self.gcode.len().max(1) as f64;
        let elapsed = self.start.elapsed().as_secs();
        let mut estimated = self.estimated;
        if progress > 0.1 {
            estimated = (self.estimated as f64 * (1.0 - progress) + elapsed as f64) as u64;
        }
        {
            let mut s = self.st.lock().unwrap();
            s.line = self.position;
            s.lines = self.gcode.len();
            s.elapsed = elapsed;
            s.estimated = estimated;
        }
        self.position += 1;
        Ok(())
    }

    fn cancel(&mut self) -> Result<(), String> {
        self.position = 0;
        self.gcode.clear();
        self.printing = false;
        self.paused = false;
        {
            let mut s = self.st.lock().unwrap();
            s.printing = false;
            s.paused = false;
        }
        self.send_raw("M140 S0")?;
        self.send_raw("M104 S0")?;
        self.send_raw("M107")?;
        // Home XY so the nozzle doesn't rest on the aborted print.
        self.send_command("G28 X Y")?;
        self.send_raw("M84")
    }
}

fn run(port: &str, baud: Option<u32>, st: &Arc<Mutex<UsbState>>, rx: &Receiver<Cmd>) -> Result<(), String> {
    let baud = match baud {
        Some(b) => b,
        None => detect_baud(port, st)?,
    };
    // Short reads keep the interface responsive; Cura's 3 s readline timeout is emulated below.
    let serial = SerialPort::open(port, baud, Duration::from_millis(100))?;
    {
        let mut s = st.lock().unwrap();
        s.baud = baud;
        s.status = "connected".into();
        s.message = format!("Connected at {} baud", baud);
    }
    let mut d = Device { serial, st, command_received: true, queue: VecDeque::new(), gcode: Vec::new(), position: 0, printing: false, paused: false, start: Instant::now(), estimated: 0 };
    let mut firmware_name_requested = false;
    let mut last_temperature_request: Option<Instant> = None;
    let mut firmware_idle_count = 0;
    let mut printer_busy = false;
    let mut last_line_at = Instant::now();
    loop {
        // Requests from the interface.
        loop {
            match rx.try_recv() {
                Ok(Cmd::Close) | Err(TryRecvError::Disconnected) => return Ok(()),
                Ok(Cmd::Print { gcode, name, estimated }) => {
                    // `_printGCode`.
                    d.gcode = gcode.split('\n').map(String::from).collect();
                    d.gcode.insert(0, "M110".into());
                    d.position = 0;
                    d.paused = false;
                    d.start = Instant::now();
                    d.estimated = estimated;
                    {
                        let mut s = st.lock().unwrap();
                        s.job_name = name;
                        s.printing = true;
                        s.paused = false;
                        s.lines = d.gcode.len();
                    }
                    for _ in 0..4 {
                        d.send_next_line()?;
                    }
                    d.printing = true;
                    st.lock().unwrap().printing = d.printing;
                }
                Ok(Cmd::Pause) => {
                    d.paused = true;
                    st.lock().unwrap().paused = true;
                }
                Ok(Cmd::Resume) => {
                    d.paused = false;
                    st.lock().unwrap().paused = false;
                    if d.printing {
                        d.send_next_line()?;
                    }
                }
                Ok(Cmd::Cancel) => d.cancel()?,
                Ok(Cmd::Send(c)) => d.send_command(&c)?,
                Err(TryRecvError::Empty) => break,
            }
        }
        let raw = d.serial.read_line(4096)?;
        if raw.is_empty() {
            if last_line_at.elapsed() < Duration::from_secs(3) {
                continue;
            }
        }
        last_line_at = Instant::now();
        let line = String::from_utf8_lossy(&raw).to_string();
        if !raw.is_empty() && !line.starts_with("ok T:") && !(line.trim() == "ok") {
            d.log("<", &line);
        }
        if !firmware_name_requested {
            firmware_name_requested = true;
            d.send_command("M115")?;
        }
        if let Some(p) = line.find("FIRMWARE_NAME:") {
            let rest = &line[p + 14..];
            let name = rest.split(" SOURCE_CODE_URL").next().unwrap_or(rest).split(" PROTOCOL_VERSION").next().unwrap_or(rest).trim().to_string();
            st.lock().unwrap().firmware = name;
        }
        if last_temperature_request.is_none_or(|t| t.elapsed() > Duration::from_secs(3)) && !printer_busy {
            d.send_command("M105")?;
            last_temperature_request = Some(Instant::now());
        }
        if line.contains("T:") || line.contains("B:") {
            parse_temperatures(&line, &mut st.lock().unwrap());
        }
        if raw.is_empty() {
            firmware_idle_count += 1;
        } else {
            firmware_idle_count = 0;
        }
        if line.starts_with("ok") || firmware_idle_count > 1 {
            printer_busy = false;
            d.command_received = true;
            if let Some(c) = d.queue.pop_front() {
                d.send_raw(&c)?;
            } else if d.printing && !d.paused {
                d.send_next_line()?;
            }
        }
        if line.starts_with("echo:busy:") {
            printer_busy = true;
        }
        if d.printing {
            if line.starts_with("!!") {
                d.log("!", "Printer signals fatal error. Cancelling print.");
                d.cancel()?;
            } else if line.to_lowercase().starts_with("resend") || line.starts_with("rs") {
                let cleaned = line.replace("N:", " ").replace('N', " ").replace(':', " ");
                let pos = cleaned.split_whitespace().last().and_then(|x| x.parse::<usize>().ok()).or_else(|| if line.starts_with("rs") { line.split_whitespace().nth(1).and_then(|x| x.parse().ok()) } else { None });
                if let Some(p) = pos {
                    d.position = p;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temperatures() {
        let mut s = UsbState { hotends: vec![Temperature::default(); 2], ..Default::default() };
        parse_temperatures("ok T:201.3 /210.0 B:59.8 /60.0 T0:201.3 /210.0 T1:25.1 /0.0 @:127 B@:0", &mut s);
        assert_eq!(s.hotends[0].current, 201.3);
        assert_eq!(s.hotends[0].target, 210.0);
        assert_eq!(s.hotends[1].current, 25.1);
        assert_eq!(s.bed.current, 59.8);
        assert_eq!(s.bed.target, 60.0);
    }

    #[test]
    fn checksum() {
        let body = "N0M110";
        assert_eq!(body.bytes().fold(0u8, |a, b| a ^ b), 0x4e ^ 0x30 ^ 0x4d ^ 0x31 ^ 0x31 ^ 0x30);
    }
}
