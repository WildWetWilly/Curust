//! Printing over the network: upload the G-code to OctoPrint, Moonraker (Klipper), PrusaLink
//! or a Duet (RepRapFirmware) and start the print; monitor temperatures and progress; pause,
//! resume and cancel. Plain HTTP is spoken directly; HTTPS (and Digest authentication) go
//! through the system's curl.

use curust_settings::json::{self, Json};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    OctoPrint,
    Moonraker,
    PrusaLink,
    Duet,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s.to_lowercase().as_str() {
            "octoprint" => Kind::OctoPrint,
            "moonraker" | "klipper" | "mainsail" | "fluidd" => Kind::Moonraker,
            "prusalink" | "prusa" => Kind::PrusaLink,
            "duet" | "rrf" | "reprapfirmware" => Kind::Duet,
            _ => return None,
        })
    }
    pub fn name(&self) -> &'static str {
        match self {
            Kind::OctoPrint => "octoprint",
            Kind::Moonraker => "moonraker",
            Kind::PrusaLink => "prusalink",
            Kind::Duet => "duet",
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            Kind::OctoPrint => "OctoPrint",
            Kind::Moonraker => "Klipper (Moonraker: Mainsail, Fluidd)",
            Kind::PrusaLink => "PrusaLink",
            Kind::Duet => "Duet (RepRapFirmware)",
        }
    }
}

/// A network printer connection.
#[derive(Clone, Debug)]
pub struct NetPrinter {
    pub kind: Kind,
    /// Base URL, e.g. `http://192.168.1.20` or `http://octopi.local:5000`.
    pub url: String,
    /// API key (OctoPrint, Moonraker, PrusaLink) or password (Duet, PrusaLink digest).
    pub key: String,
    /// PrusaLink Digest user name ("maker").
    pub user: String,
}

#[derive(Clone, Debug, Default)]
pub struct NetStatus {
    pub state: String,
    pub printing: bool,
    pub paused: bool,
    pub file: String,
    pub progress: f64,
    pub elapsed: f64,
    pub remaining: f64,
    pub hotends: Vec<(f64, f64)>,
    pub bed: (f64, f64),
}

// ---------------------------------------------------------------------- HTTP

pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn json(&self) -> Result<Json, String> {
        json::parse(&String::from_utf8_lossy(&self.body)).map_err(|e| format!("unexpected response: {}", e))
    }
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
    fn err(&self, what: &str) -> String {
        let text = String::from_utf8_lossy(&self.body);
        let detail: String = text.chars().take(200).collect();
        match self.status {
            401 | 403 => format!("{}: the printer refused the request (HTTP {}). Check the API key / password.", what, self.status),
            404 => format!("{}: not found (HTTP 404). Is this the right kind of printer and address?", what),
            409 => format!("{}: the printer is busy or not ready (HTTP 409). {}", what, detail.trim()),
            s => format!("{}: HTTP {} {}", what, s, detail.trim()),
        }
    }
}

fn split_url(url: &str) -> Result<(bool, String, u16, String), String> {
    let (https, rest) = if let Some(r) = url.strip_prefix("http://") {
        (false, r)
    } else if let Some(r) = url.strip_prefix("https://") {
        (true, r)
    } else {
        (false, url)
    };
    let (hostport, path) = match rest.find('/') {
        Some(p) => (&rest[..p], &rest[p..]),
        None => (rest, "/"),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) if !h.contains(']') || h.ends_with(']') => (h.to_string(), p.parse().map_err(|_| format!("bad port in {}", url))?),
        _ => (hostport.to_string(), if https { 443 } else { 80 }),
    };
    if host.is_empty() {
        return Err(format!("no host in {}", url));
    }
    Ok((https, host, port, path.to_string()))
}

/// One HTTP/1.1 request.
pub fn request(method: &str, url: &str, headers: &[(String, String)], body: &[u8], digest: Option<(&str, &str)>) -> Result<HttpResponse, String> {
    let (https, host, port, path) = split_url(url)?;
    if https || digest.is_some() {
        return curl_request(method, url, headers, body, digest);
    }
    let addr = (host.as_str(), port).to_socket_addrs().map_err(|e| format!("{}: {}", host, e))?.next().ok_or_else(|| format!("{}: no address", host))?;
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(8)).map_err(|e| format!("Can't reach {}: {}", host, e))?;
    s.set_read_timeout(Some(Duration::from_secs(120))).ok();
    s.set_write_timeout(Some(Duration::from_secs(120))).ok();
    let mut req = format!("{} {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: Curust/{}\r\nAccept: */*\r\nConnection: close\r\nContent-Length: {}\r\n", method, path, if port == 80 { host.clone() } else { format!("{}:{}", host, port) }, env!("CARGO_PKG_VERSION"), body.len());
    for (k, v) in headers {
        req.push_str(&format!("{}: {}\r\n", k, v));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    s.write_all(body).map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).map_err(|e| format!("{}: {}", host, e))?;
    let end = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or("malformed HTTP response")?;
    let head = String::from_utf8_lossy(&raw[..end]).to_string();
    let mut body = raw[end + 4..].to_vec();
    let status: u16 = head.split_whitespace().nth(1).and_then(|x| x.parse().ok()).ok_or("malformed HTTP status")?;
    if head.to_lowercase().contains("transfer-encoding: chunked") {
        let mut out = Vec::new();
        let mut k = 0;
        while k < body.len() {
            let Some(e) = body[k..].windows(2).position(|w| w == b"\r\n") else { break };
            let size = usize::from_str_radix(String::from_utf8_lossy(&body[k..k + e]).trim().split(';').next().unwrap_or("0"), 16).unwrap_or(0);
            k += e + 2;
            if size == 0 || k + size > body.len() {
                break;
            }
            out.extend_from_slice(&body[k..k + size]);
            k += size + 2;
        }
        body = out;
    }
    Ok(HttpResponse { status, body })
}

fn curl_request(method: &str, url: &str, headers: &[(String, String)], body: &[u8], digest: Option<(&str, &str)>) -> Result<HttpResponse, String> {
    let tmp = std::env::temp_dir().join(format!("curust-upload-{}-{}", std::process::id(), body.len()));
    let mut cmd = std::process::Command::new(if cfg!(windows) { "curl.exe" } else { "curl" });
    cmd.args(["-sS", "-k", "--max-time", "600", "-X", method, "-w", "\n%{http_code}"]);
    for (k, v) in headers {
        cmd.args(["-H", &format!("{}: {}", k, v)]);
    }
    if let Some((u, p)) = digest {
        cmd.args(["--digest", "-u", &format!("{}:{}", u, p)]);
    }
    if !body.is_empty() {
        std::fs::write(&tmp, body).map_err(|e| e.to_string())?;
        cmd.args(["--data-binary", &format!("@{}", tmp.display())]);
    }
    cmd.arg(url);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().map_err(|e| format!("curl: {}", e));
    let _ = std::fs::remove_file(&tmp);
    let out = out?;
    if !out.status.success() && out.stdout.is_empty() {
        return Err(format!("Can't reach {}: {}", url, String::from_utf8_lossy(&out.stderr).trim()));
    }
    let text = out.stdout;
    let nl = text.iter().rposition(|&b| b == b'\n').unwrap_or(0);
    let status: u16 = String::from_utf8_lossy(&text[nl..]).trim().parse().unwrap_or(0);
    Ok(HttpResponse { status, body: text[..nl].to_vec() })
}

/// A multipart/form-data body: (field, file name, content type, data).
pub fn multipart(parts: &[(&str, Option<&str>, &str, &[u8])]) -> (String, Vec<u8>) {
    let boundary = format!("----CurustBoundary{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    let mut body = Vec::new();
    for (name, file, ct, data) in parts {
        body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
        match file {
            Some(f) => body.extend_from_slice(format!("Content-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\nContent-Type: {}\r\n\r\n", name, f.replace('"', "'"), ct).as_bytes()),
            None => body.extend_from_slice(format!("Content-Disposition: form-data; name=\"{}\"\r\n\r\n", name).as_bytes()),
        }
        body.extend_from_slice(data);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{}--\r\n", boundary).as_bytes());
    (format!("multipart/form-data; boundary={}", boundary), body)
}

fn url_enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn f(j: Option<&Json>) -> f64 {
    j.and_then(|v| v.as_f64()).unwrap_or(0.0)
}

fn now_iso() -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let days = t.div_euclid(86400);
    let secs = t.rem_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", y, m, d, secs / 3600, secs % 3600 / 60, secs % 60)
}

// ---------------------------------------------------------------------- printers

impl NetPrinter {
    fn base(&self) -> String {
        let u = self.url.trim().trim_end_matches('/');
        if u.starts_with("http://") || u.starts_with("https://") {
            u.to_string()
        } else {
            format!("http://{}", u)
        }
    }

    fn headers(&self) -> Vec<(String, String)> {
        match self.kind {
            Kind::OctoPrint | Kind::Moonraker | Kind::PrusaLink if !self.key.is_empty() && self.user.is_empty() => vec![("X-Api-Key".into(), self.key.clone())],
            _ => Vec::new(),
        }
    }

    fn digest(&self) -> Option<(&str, &str)> {
        if self.kind == Kind::PrusaLink && !self.user.is_empty() {
            Some((self.user.as_str(), self.key.as_str()))
        } else {
            None
        }
    }

    fn get(&self, path: &str) -> Result<HttpResponse, String> {
        request("GET", &format!("{}{}", self.base(), path), &self.headers(), &[], self.digest())
    }

    fn send(&self, method: &str, path: &str, ct: Option<&str>, body: &[u8], extra: &[(&str, &str)]) -> Result<HttpResponse, String> {
        let mut h = self.headers();
        if let Some(ct) = ct {
            h.push(("Content-Type".into(), ct.into()));
        }
        for (k, v) in extra {
            h.push((k.to_string(), v.to_string()));
        }
        request(method, &format!("{}{}", self.base(), path), &h, body, self.digest())
    }

    fn duet_connect(&self) -> Result<(), String> {
        let r = self.get(&format!("/rr_connect?password={}&time={}", url_enc(&self.key), now_iso()))?;
        if !r.ok() {
            return Err(r.err("Connecting to the Duet"));
        }
        let j = r.json()?;
        match j.get("err").and_then(|v| v.as_f64()) {
            Some(e) if e != 0.0 => Err(if e == 1.0 { "The Duet rejected the password.".into() } else { "The Duet has no free session.".into() }),
            _ => Ok(()),
        }
    }

    /// Check the connection; returns a description of the printer.
    pub fn test(&self) -> Result<String, String> {
        match self.kind {
            Kind::OctoPrint => {
                let r = self.get("/api/version")?;
                if !r.ok() {
                    return Err(r.err("OctoPrint"));
                }
                let j = r.json()?;
                Ok(j.get("text").and_then(|v| v.as_str()).map(String::from).unwrap_or_else(|| "OctoPrint".into()))
            }
            Kind::Moonraker => {
                let r = self.get("/server/info")?;
                if !r.ok() {
                    return Err(r.err("Moonraker"));
                }
                let j = r.json()?;
                let res = j.get("result");
                Ok(format!(
                    "Moonraker {} (Klipper {})",
                    res.and_then(|r| r.get("moonraker_version")).and_then(|v| v.as_str()).unwrap_or("?"),
                    res.and_then(|r| r.get("klippy_state")).and_then(|v| v.as_str()).unwrap_or("?")
                ))
            }
            Kind::PrusaLink => {
                let r = self.get("/api/version")?;
                if !r.ok() {
                    return Err(r.err("PrusaLink"));
                }
                let j = r.json()?;
                Ok(format!("{} {}", j.get("text").and_then(|v| v.as_str()).unwrap_or("PrusaLink"), j.get("server").and_then(|v| v.as_str()).unwrap_or("")).trim().to_string())
            }
            Kind::Duet => {
                self.duet_connect()?;
                let r = self.get("/rr_model?key=boards&flags=d99v")?;
                let name = r.json().ok().and_then(|j| j.get("result").and_then(|b| match b {
                    Json::Array(a) => a.first().and_then(|x| x.get("name")).and_then(|v| v.as_str()).map(String::from),
                    _ => None,
                }));
                Ok(name.unwrap_or_else(|| "Duet".into()))
            }
        }
    }

    /// Upload the G-code and (optionally) start printing it.
    pub fn upload(&self, name: &str, gcode: &[u8], start: bool) -> Result<String, String> {
        let name = if name.to_lowercase().ends_with(".gcode") { name.to_string() } else { format!("{}.gcode", name) };
        match self.kind {
            Kind::OctoPrint => {
                let print = if start { b"true".as_slice() } else { b"false".as_slice() };
                let (ct, body) = multipart(&[("file", Some(&name), "application/octet-stream", gcode), ("select", None, "text/plain", print), ("print", None, "text/plain", print)]);
                let r = self.send("POST", "/api/files/local", Some(&ct), &body, &[])?;
                if !r.ok() {
                    return Err(r.err("Uploading to OctoPrint"));
                }
            }
            Kind::Moonraker => {
                let print = if start { b"true".as_slice() } else { b"false".as_slice() };
                let (ct, body) = multipart(&[("file", Some(&name), "application/octet-stream", gcode), ("root", None, "text/plain", b"gcodes"), ("print", None, "text/plain", print)]);
                let r = self.send("POST", "/server/files/upload", Some(&ct), &body, &[])?;
                if !r.ok() {
                    return Err(r.err("Uploading to Moonraker"));
                }
            }
            Kind::PrusaLink => {
                // The first writable storage (usb on MK4/XL/Mini, local on SL1/MK3).
                let storage = self
                    .get("/api/v1/storage")
                    .ok()
                    .and_then(|r| r.json().ok())
                    .and_then(|j| match j.get("storage_list") {
                        Some(Json::Array(a)) => a.iter().find(|s| s.get("available").and_then(|v| v.as_bool()).unwrap_or(true) && !s.get("read_only").and_then(|v| v.as_bool()).unwrap_or(false)).and_then(|s| s.get("path").and_then(|v| v.as_str()).map(|p| p.trim_matches('/').to_string())),
                        _ => None,
                    })
                    .unwrap_or_else(|| "usb".into());
                let r = self.send("PUT", &format!("/api/v1/files/{}/{}", storage, url_enc(&name)), Some("text/x.gcode"), gcode, &[("Print-After-Upload", if start { "?1" } else { "?0" }), ("Overwrite", "?1")])?;
                if !r.ok() {
                    return Err(r.err("Uploading to PrusaLink"));
                }
            }
            Kind::Duet => {
                self.duet_connect()?;
                let path = format!("0:/gcodes/{}", name);
                let r = self.send("POST", &format!("/rr_upload?name={}&time={}", url_enc(&path), now_iso()), Some("application/octet-stream"), gcode, &[])?;
                if !r.ok() || r.json().ok().and_then(|j| j.get("err").and_then(|v| v.as_f64())).unwrap_or(0.0) != 0.0 {
                    return Err(r.err("Uploading to the Duet"));
                }
                if start {
                    let r = self.get(&format!("/rr_gcode?gcode={}", url_enc(&format!("M32 \"{}\"", path))))?;
                    if !r.ok() {
                        return Err(r.err("Starting the print"));
                    }
                }
            }
        }
        Ok(if start { format!("Sent {} to the printer; the print is starting.", name) } else { format!("Uploaded {} to the printer.", name) })
    }

    pub fn status(&self) -> Result<NetStatus, String> {
        let mut st = NetStatus::default();
        match self.kind {
            Kind::OctoPrint => {
                let p = self.get("/api/printer")?;
                if p.status == 409 {
                    st.state = "Printer not connected to OctoPrint".into();
                } else if p.ok() {
                    let j = p.json()?;
                    st.state = j.get("state").and_then(|s| s.get("text")).and_then(|v| v.as_str()).unwrap_or("").to_string();
                    if let Some(Json::Object(t)) = j.get("temperature") {
                        let mut tools: Vec<(usize, (f64, f64))> = t
                            .iter()
                            .filter_map(|(k, v)| k.strip_prefix("tool").and_then(|n| n.parse().ok()).map(|n: usize| (n, (f(v.get("actual")), f(v.get("target"))))))
                            .collect();
                        tools.sort_by_key(|x| x.0);
                        st.hotends = tools.into_iter().map(|x| x.1).collect();
                        if let Some(b) = t.iter().find(|(k, _)| k == "bed").map(|(_, v)| v) {
                            st.bed = (f(b.get("actual")), f(b.get("target")));
                        }
                    }
                } else {
                    return Err(p.err("OctoPrint"));
                }
                let jr = self.get("/api/job")?;
                if jr.ok() {
                    let j = jr.json()?;
                    let state = j.get("state").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    st.printing = state.starts_with("Printing") || state.starts_with("Pausing") || state.starts_with("Paused");
                    st.paused = state.starts_with("Paused") || state.starts_with("Pausing");
                    if !state.is_empty() {
                        st.state = state;
                    }
                    st.file = j.get("job").and_then(|x| x.get("file")).and_then(|x| x.get("name")).and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let pr = j.get("progress");
                    st.progress = f(pr.and_then(|p| p.get("completion"))) / 100.0;
                    st.elapsed = f(pr.and_then(|p| p.get("printTime")));
                    st.remaining = f(pr.and_then(|p| p.get("printTimeLeft")));
                }
            }
            Kind::Moonraker => {
                let r = self.get("/printer/objects/query?print_stats&virtual_sdcard&extruder&extruder1&heater_bed")?;
                if !r.ok() {
                    return Err(r.err("Moonraker"));
                }
                let j = r.json()?;
                let s = j.get("result").and_then(|x| x.get("status")).cloned().unwrap_or(Json::Null);
                let ps = s.get("print_stats");
                let state = ps.and_then(|p| p.get("state")).and_then(|v| v.as_str()).unwrap_or("standby").to_string();
                st.printing = state == "printing" || state == "paused";
                st.paused = state == "paused";
                st.state = state;
                st.file = ps.and_then(|p| p.get("filename")).and_then(|v| v.as_str()).unwrap_or("").to_string();
                st.elapsed = f(ps.and_then(|p| p.get("print_duration")));
                st.progress = f(s.get("virtual_sdcard").and_then(|v| v.get("progress")));
                if st.progress > 0.0 {
                    st.remaining = st.elapsed / st.progress - st.elapsed;
                }
                for e in ["extruder", "extruder1"] {
                    if let Some(x) = s.get(e) {
                        st.hotends.push((f(x.get("temperature")), f(x.get("target"))));
                    }
                }
                if let Some(b) = s.get("heater_bed") {
                    st.bed = (f(b.get("temperature")), f(b.get("target")));
                }
            }
            Kind::PrusaLink => {
                let r = self.get("/api/v1/status")?;
                if !r.ok() {
                    return Err(r.err("PrusaLink"));
                }
                let j = r.json()?;
                let p = j.get("printer");
                let state = p.and_then(|p| p.get("state")).and_then(|v| v.as_str()).unwrap_or("").to_string();
                st.printing = state == "PRINTING" || state == "PAUSED";
                st.paused = state == "PAUSED";
                st.state = state;
                st.hotends.push((f(p.and_then(|p| p.get("temp_nozzle"))), f(p.and_then(|p| p.get("target_nozzle")))));
                st.bed = (f(p.and_then(|p| p.get("temp_bed"))), f(p.and_then(|p| p.get("target_bed"))));
                let job = j.get("job");
                st.progress = f(job.and_then(|x| x.get("progress"))) / 100.0;
                st.elapsed = f(job.and_then(|x| x.get("time_printing")));
                st.remaining = f(job.and_then(|x| x.get("time_remaining")));
            }
            Kind::Duet => {
                self.duet_connect()?;
                let status = self.get("/rr_model?key=state.status&flags=v")?.json()?;
                let state = status.get("result").and_then(|v| v.as_str()).unwrap_or("").to_string();
                st.printing = matches!(state.as_str(), "processing" | "paused" | "pausing" | "resuming" | "simulating");
                st.paused = state == "paused" || state == "pausing";
                st.state = state;
                if let Ok(h) = self.get("/rr_model?key=heat&flags=v").and_then(|r| r.json()) {
                    let heat = h.get("result");
                    let heaters: Vec<(f64, f64)> = match heat.and_then(|x| x.get("heaters")) {
                        Some(Json::Array(a)) => a.iter().map(|x| (f(x.get("current")), f(x.get("active")))).collect(),
                        _ => Vec::new(),
                    };
                    let bed_idx = match heat.and_then(|x| x.get("bedHeaters")) {
                        Some(Json::Array(a)) => a.first().and_then(|v| v.as_f64()).map(|v| v as i64).unwrap_or(-1),
                        _ => -1,
                    };
                    for (k, hv) in heaters.iter().enumerate() {
                        if k as i64 == bed_idx {
                            st.bed = *hv;
                        } else {
                            st.hotends.push(*hv);
                        }
                    }
                }
                if let Ok(jb) = self.get("/rr_model?key=job&flags=v").and_then(|r| r.json()) {
                    let job = jb.get("result");
                    st.file = job.and_then(|x| x.get("file")).and_then(|x| x.get("fileName")).and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let size = f(job.and_then(|x| x.get("file")).and_then(|x| x.get("size")));
                    if size > 0.0 {
                        st.progress = f(job.and_then(|x| x.get("filePosition"))) / size;
                    }
                    st.elapsed = f(job.and_then(|x| x.get("duration")));
                    st.remaining = f(job.and_then(|x| x.get("timesLeft")).and_then(|x| x.get("file")));
                }
            }
        }
        Ok(st)
    }

    /// "pause", "resume" or "cancel".
    pub fn control(&self, action: &str) -> Result<(), String> {
        let r = match (self.kind.clone(), action) {
            (Kind::OctoPrint, "pause" | "resume") => self.send("POST", "/api/job", Some("application/json"), format!("{{\"command\": \"pause\", \"action\": \"{}\"}}", action).as_bytes(), &[])?,
            (Kind::OctoPrint, "cancel") => self.send("POST", "/api/job", Some("application/json"), b"{\"command\": \"cancel\"}", &[])?,
            (Kind::Moonraker, a) => self.send("POST", &format!("/printer/print/{}", a), None, &[], &[])?,
            (Kind::PrusaLink, a) => {
                let id = self.get("/api/v1/status")?.json()?.get("job").and_then(|j| j.get("id")).and_then(|v| v.as_f64()).ok_or("No print job is running.")? as i64;
                match a {
                    "cancel" => self.send("DELETE", &format!("/api/v1/job/{}", id), None, &[], &[])?,
                    _ => self.send("PUT", &format!("/api/v1/job/{}/{}", id, a), None, &[], &[])?,
                }
            }
            (Kind::Duet, a) => {
                self.duet_connect()?;
                let code = match a {
                    "pause" => "M25",
                    "resume" => "M24",
                    _ => "M25\nM0",
                };
                self.get(&format!("/rr_gcode?gcode={}", url_enc(code)))?
            }
            _ => return Err(format!("unknown action {}", action)),
        };
        if !r.ok() {
            return Err(r.err(action));
        }
        Ok(())
    }

    /// Send G-code commands (console / jog / preheat).
    pub fn gcode(&self, code: &str) -> Result<(), String> {
        let r = match self.kind {
            Kind::OctoPrint => {
                let cmds: Vec<Json> = code.lines().filter(|l| !l.trim().is_empty()).map(|l| Json::Str(l.trim().to_string())).collect();
                let body = Json::Object(vec![("commands".into(), Json::Array(cmds))]).to_string_compact();
                self.send("POST", "/api/printer/command", Some("application/json"), body.as_bytes(), &[])?
            }
            Kind::Moonraker => self.send("POST", &format!("/printer/gcode/script?script={}", url_enc(code)), None, &[], &[])?,
            Kind::Duet => {
                self.duet_connect()?;
                self.get(&format!("/rr_gcode?gcode={}", url_enc(code)))?
            }
            Kind::PrusaLink => return Err("PrusaLink does not accept G-code commands over the network.".into()),
        };
        if !r.ok() {
            return Err(r.err("Sending G-code"));
        }
        Ok(())
    }

    pub fn to_json(&self) -> Json {
        Json::Object(vec![("kind".into(), Json::Str(self.kind.name().into())), ("url".into(), Json::Str(self.url.clone())), ("key".into(), Json::Str(self.key.clone())), ("user".into(), Json::Str(self.user.clone()))])
    }

    pub fn from_json(j: &Json) -> Option<NetPrinter> {
        Some(NetPrinter {
            kind: Kind::parse(j.get("kind")?.as_str()?)?,
            url: j.get("url")?.as_str()?.to_string(),
            key: j.get("key").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            user: j.get("user").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(split_url("http://octopi.local:5000/api").unwrap(), (false, "octopi.local".into(), 5000, "/api".into()));
        assert_eq!(split_url("192.168.1.2").unwrap(), (false, "192.168.1.2".into(), 80, "/".into()));
        assert!(now_iso().len() == 19);
    }
}
