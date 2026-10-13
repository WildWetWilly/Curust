//! Curust Setup: a small, dependency-free Windows installer.
//!
//! * installs for all users to `%ProgramFiles%\Curust` (asks for administrator rights; replaces
//!   a per-user installation of Curust 0.1)
//! * Start menu (and optional desktop) shortcut
//! * "Open with Curust" for .stl / .3mf / .obj files
//! * an entry in Settings › Apps › Installed apps, with an uninstaller
//!
//! `curust-installer.exe /S` installs silently with the defaults; `uninstall.exe --uninstall` removes
//! it again (`--silent` for no questions).
#![cfg_attr(windows, windows_subsystem = "windows")]

#[path = "../../app/src/zip.rs"]
#[allow(dead_code)]
mod zip;

static PAYLOAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload.zip"));
const VERSION: &str = env!("CARGO_PKG_VERSION");
#[allow(dead_code)]
const HOMEPAGE: &str = "https://github.com/WildWetWilly/Curust";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    #[cfg(windows)]
    win::main(args);
    #[cfg(not(windows))]
    {
        let _ = (args, PAYLOAD.len(), VERSION);
        eprintln!("The Curust installer runs on Windows.");
    }
}

#[cfg(windows)]
mod win {
    use super::{PAYLOAD, VERSION};
    use std::ffi::c_void;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicIsize, Ordering};

    type Hwnd = *mut c_void;
    type Hkey = isize;

    #[repr(C)]
    struct WndClassExW {
        cb_size: u32,
        style: u32,
        wnd_proc: extern "system" fn(Hwnd, u32, usize, isize) -> isize,
        cls_extra: i32,
        wnd_extra: i32,
        instance: *mut c_void,
        icon: *mut c_void,
        cursor: *mut c_void,
        background: *mut c_void,
        menu_name: *const u16,
        class_name: *const u16,
        icon_sm: *mut c_void,
    }

    #[repr(C)]
    struct Msg {
        hwnd: Hwnd,
        message: u32,
        wparam: usize,
        lparam: isize,
        time: u32,
        pt: [i32; 2],
        private: u32,
    }

    #[link(name = "user32")]
    extern "system" {
        fn RegisterClassExW(c: *const WndClassExW) -> u16;
        fn CreateWindowExW(ex: u32, class: *const u16, title: *const u16, style: u32, x: i32, y: i32, w: i32, h: i32, parent: Hwnd, menu: isize, inst: *mut c_void, param: *mut c_void) -> Hwnd;
        fn DefWindowProcW(h: Hwnd, m: u32, w: usize, l: isize) -> isize;
        fn GetMessageW(m: *mut Msg, h: Hwnd, a: u32, b: u32) -> i32;
        fn TranslateMessage(m: *const Msg) -> i32;
        fn DispatchMessageW(m: *const Msg) -> isize;
        fn IsDialogMessageW(h: Hwnd, m: *mut Msg) -> i32;
        fn PostQuitMessage(code: i32);
        fn PostMessageW(h: Hwnd, m: u32, w: usize, l: isize) -> i32;
        fn SendMessageW(h: Hwnd, m: u32, w: usize, l: isize) -> isize;
        fn GetDlgItem(h: Hwnd, id: i32) -> Hwnd;
        fn SetWindowTextW(h: Hwnd, t: *const u16) -> i32;
        fn GetWindowTextW(h: Hwnd, t: *mut u16, n: i32) -> i32;
        fn EnableWindow(h: Hwnd, e: i32) -> i32;
        fn ShowWindow(h: Hwnd, c: i32) -> i32;
        fn DestroyWindow(h: Hwnd) -> i32;
        fn MessageBoxW(h: Hwnd, text: *const u16, caption: *const u16, t: u32) -> i32;
        fn LoadCursorW(i: *mut c_void, name: usize) -> *mut c_void;
        fn LoadIconW(i: *mut c_void, name: usize) -> *mut c_void;
        fn GetSystemMetrics(i: i32) -> i32;
        fn SetProcessDPIAware() -> i32;
        fn GetDpiForSystem() -> u32;
    }
    #[link(name = "gdi32")]
    extern "system" {
        fn CreateFontW(h: i32, w: i32, esc: i32, ori: i32, weight: i32, it: u32, ul: u32, so: u32, cs: u32, op: u32, cp: u32, q: u32, pf: u32, face: *const u16) -> *mut c_void;
        fn GetStockObject(i: i32) -> *mut c_void;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(n: *const u16) -> *mut c_void;
    }
    #[link(name = "advapi32")]
    extern "system" {
        fn RegCreateKeyExW(key: Hkey, sub: *const u16, r: u32, class: *const u16, opts: u32, sam: u32, sec: *mut c_void, out: *mut Hkey, disp: *mut u32) -> i32;
        fn RegSetValueExW(key: Hkey, name: *const u16, r: u32, ty: u32, data: *const u8, len: u32) -> i32;
        fn RegCloseKey(key: Hkey) -> i32;
        fn RegDeleteTreeW(key: Hkey, sub: *const u16) -> i32;
        fn RegOpenKeyExW(key: Hkey, sub: *const u16, opts: u32, sam: u32, out: *mut Hkey) -> i32;
        fn RegQueryValueExW(key: Hkey, name: *const u16, r: *mut u32, ty: *mut u32, data: *mut u8, len: *mut u32) -> i32;
        fn RegDeleteValueW(key: Hkey, name: *const u16) -> i32;
    }
    #[repr(C)]
    struct BrowseInfoW {
        owner: Hwnd,
        root: *const c_void,
        display_name: *mut u16,
        title: *const u16,
        flags: u32,
        callback: *const c_void,
        lparam: isize,
        image: i32,
    }

    #[link(name = "ole32")]
    extern "system" {
        fn CoInitializeEx(reserved: *mut c_void, flags: u32) -> i32;
        fn CoTaskMemFree(p: *mut c_void);
    }

    #[link(name = "shell32")]
    extern "system" {
        fn SHBrowseForFolderW(bi: *const BrowseInfoW) -> *mut c_void;
        fn SHGetPathFromIDListW(pidl: *const c_void, path: *mut u16) -> i32;
        fn SHChangeNotify(event: i32, flags: u32, a: *const c_void, b: *const c_void);
        fn ShellExecuteW(h: Hwnd, op: *const u16, file: *const u16, params: *const u16, dir: *const u16, show: i32) -> *mut c_void;
    }

    const HKCU: Hkey = 0x8000_0001u32 as i32 as isize;
    const HKLM: Hkey = 0x8000_0002u32 as i32 as isize;
    const KEY_ALL: u32 = 0xF003F;
    const KEY_READ: u32 = 0x20019;
    const UNINSTALL_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Curust";
    const PROGID: &str = "Curust.Model";
    const EXTENSIONS: [&str; 3] = [".stl", ".3mf", ".obj"];
    /// The list of installed files (relative paths), in the installation folder.
    const MANIFEST: &str = "install-files.txt";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn message(h: Hwnd, text: &str, flags: u32) -> i32 {
        // SAFETY: valid NUL-terminated strings.
        unsafe { MessageBoxW(h, wide(text).as_ptr(), wide("Curust Setup").as_ptr(), flags) }
    }

    // ------------------------------------------------------------------ registry

    fn reg_set(root: Hkey, path: &str, name: Option<&str>, value: &str) -> Result<(), String> {
        let mut k: Hkey = 0;
        // SAFETY: plain registry calls.
        unsafe {
            if RegCreateKeyExW(root, wide(path).as_ptr(), 0, std::ptr::null(), 0, KEY_ALL, std::ptr::null_mut(), &mut k, std::ptr::null_mut()) != 0 {
                return Err(format!("Can't write the registry key {}", path));
            }
            let v = wide(value);
            let n = name.map(wide);
            RegSetValueExW(k, n.as_ref().map(|n| n.as_ptr()).unwrap_or(std::ptr::null()), 0, 1, v.as_ptr() as *const u8, (v.len() * 2) as u32);
            RegCloseKey(k);
        }
        Ok(())
    }

    fn reg_set_dword(root: Hkey, path: &str, name: &str, value: u32) {
        let mut k: Hkey = 0;
        // SAFETY: plain registry calls.
        unsafe {
            if RegCreateKeyExW(root, wide(path).as_ptr(), 0, std::ptr::null(), 0, KEY_ALL, std::ptr::null_mut(), &mut k, std::ptr::null_mut()) == 0 {
                RegSetValueExW(k, wide(name).as_ptr(), 0, 4, &value as *const u32 as *const u8, 4);
                RegCloseKey(k);
            }
        }
    }

    fn reg_get(root: Hkey, path: &str, name: &str) -> Option<String> {
        let mut k: Hkey = 0;
        // SAFETY: plain registry calls with a sized buffer.
        unsafe {
            if RegOpenKeyExW(root, wide(path).as_ptr(), 0, KEY_READ, &mut k) != 0 {
                return None;
            }
            let mut buf = vec![0u16; 2048];
            let mut len = (buf.len() * 2) as u32;
            let mut ty = 0u32;
            let r = RegQueryValueExW(k, wide(name).as_ptr(), std::ptr::null_mut(), &mut ty, buf.as_mut_ptr() as *mut u8, &mut len);
            RegCloseKey(k);
            if r != 0 {
                return None;
            }
            let n = (len as usize / 2).min(buf.len());
            Some(String::from_utf16_lossy(&buf[..n]).trim_end_matches('\0').to_string())
        }
    }

    fn reg_delete_tree(root: Hkey, path: &str) {
        // SAFETY: plain registry call.
        unsafe {
            RegDeleteTreeW(root, wide(path).as_ptr());
        }
    }

    fn reg_delete_value(root: Hkey, path: &str, name: &str) {
        let mut k: Hkey = 0;
        // SAFETY: plain registry calls.
        unsafe {
            if RegOpenKeyExW(root, wide(path).as_ptr(), 0, KEY_ALL, &mut k) == 0 {
                RegDeleteValueW(k, wide(name).as_ptr());
                RegCloseKey(k);
            }
        }
    }

    // ------------------------------------------------------------------ shortcuts

    fn powershell(script: &str) -> bool {
        use std::os::windows::process::CommandExt;
        let encoded: Vec<u8> = script.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        let b64 = base64(&encoded);
        std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &b64])
            .creation_flags(0x0800_0000)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn base64(data: &[u8]) -> String {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for c in data.chunks(3) {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            out.push(T[(n >> 18) as usize & 63] as char);
            out.push(T[(n >> 12) as usize & 63] as char);
            out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
            out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
        }
        out
    }

    fn ps_quote(s: &str) -> String {
        format!("'{}'", s.replace('\'', "''"))
    }

    /// The Start menu of every user (Curust is installed for all users).
    fn start_menu_dir() -> PathBuf {
        PathBuf::from(std::env::var("ProgramData").unwrap_or_else(|_| "C:\\ProgramData".into())).join("Microsoft\\Windows\\Start Menu\\Programs")
    }

    /// The per-user Start menu, used by Curust 0.1 (per-user installs).
    fn user_start_menu_dir() -> PathBuf {
        PathBuf::from(std::env::var("APPDATA").unwrap_or_default()).join("Microsoft\\Windows\\Start Menu\\Programs")
    }

    fn create_shortcuts(dir: &Path, desktop: bool) -> bool {
        let exe = dir.join("curust.exe");
        let mut script = String::from("$ws = New-Object -ComObject WScript.Shell\n");
        let mut targets = vec![format!("{}", ps_quote(&start_menu_dir().join("Curust.lnk").display().to_string()))];
        if desktop {
            targets.push("(Join-Path ([Environment]::GetFolderPath('CommonDesktopDirectory')) 'Curust.lnk')".into());
        }
        for t in targets {
            script.push_str(&format!(
                "$s = $ws.CreateShortcut({}); $s.TargetPath = {}; $s.WorkingDirectory = {}; $s.IconLocation = {}; $s.Description = 'Curust 3D printing slicer'; $s.Save()\n",
                t,
                ps_quote(&exe.display().to_string()),
                ps_quote(&dir.display().to_string()),
                ps_quote(&format!("{},0", exe.display()))
            ));
        }
        powershell(&script)
    }

    fn remove_shortcuts() {
        let _ = std::fs::remove_file(start_menu_dir().join("Curust.lnk"));
        powershell("$p = Join-Path ([Environment]::GetFolderPath('CommonDesktopDirectory')) 'Curust.lnk'; if (Test-Path $p) { Remove-Item $p }");
    }

    /// Curust 0.1 installed per user (HKCU, %LOCALAPPDATA%\Programs\Curust, the user's Start
    /// menu). Remove that installation's files, shortcuts and registry entries so the user is
    /// left with one Curust; their settings, printers and profiles are kept.
    fn remove_per_user_install(new_dir: &Path) {
        let old = reg_get(HKCU, UNINSTALL_KEY, "InstallLocation");
        if old.is_none() && reg_get(HKCU, "Software\\Classes\\Applications\\curust.exe\\shell\\open\\command", "").is_none() {
            return;
        }
        if let Some(old) = old.filter(|o| !o.is_empty()) {
            let old = PathBuf::from(old);
            if !old.as_os_str().eq_ignore_ascii_case(new_dir.as_os_str()) && old.join("curust.exe").exists() {
                remove_installed_files(&old);
            }
        }
        let _ = std::fs::remove_file(user_start_menu_dir().join("Curust.lnk"));
        powershell("$p = Join-Path ([Environment]::GetFolderPath('Desktop')) 'Curust.lnk'; if (Test-Path $p) { Remove-Item $p }");
        remove_registration(HKCU);
    }

    fn remove_registration(root: Hkey) {
        reg_delete_tree(root, UNINSTALL_KEY);
        reg_delete_tree(root, &format!("Software\\Classes\\{}", PROGID));
        reg_delete_tree(root, "Software\\Classes\\Applications\\curust.exe");
        for ext in EXTENSIONS {
            reg_delete_value(root, &format!("Software\\Classes\\{}\\OpenWithProgids", ext), PROGID);
        }
    }

    /// Starts a program as the signed-in user rather than with Setup's administrator rights
    /// (Explorer starts it): an elevated Curust couldn't take files dragged from Explorer.
    fn launch_unelevated(exe: &Path) {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
        let explorer = wide(&format!("{}\\explorer.exe", windir));
        let arg = wide(&format!("\"{}\"", exe.display()));
        // SAFETY: valid NUL-terminated strings.
        unsafe {
            ShellExecuteW(std::ptr::null_mut(), wide("open").as_ptr(), explorer.as_ptr(), arg.as_ptr(), std::ptr::null(), 1);
        }
    }

    // ------------------------------------------------------------------ install / uninstall

    pub struct Options {
        pub dir: PathBuf,
        pub desktop: bool,
        pub associate: bool,
        pub launch: bool,
    }

    pub fn default_dir() -> PathBuf {
        if let Some(prev) = reg_get(HKLM, UNINSTALL_KEY, "InstallLocation") {
            if !prev.is_empty() {
                return PathBuf::from(prev);
            }
        }
        let pf = std::env::var("ProgramW6432").or_else(|_| std::env::var("ProgramFiles")).unwrap_or_else(|_| "C:\\Program Files".into());
        PathBuf::from(pf).join("Curust")
    }

    fn today() -> String {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let z = t.div_euclid(86400) + 719468;
        let era = z.div_euclid(146097);
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        format!("{:04}{:02}{:02}", yoe + era * 400 + if m <= 2 { 1 } else { 0 }, m, d)
    }

    pub fn install(o: &Options, progress: &dyn Fn(u32, &str)) -> Result<Vec<String>, String> {
        let mut warnings = Vec::new();
        if PAYLOAD.is_empty() {
            return Err("This installer was built without the application files.".into());
        }
        progress(2, "Reading the package…");
        let entries = super::zip::read_zip(PAYLOAD).map_err(|e| format!("The installer is damaged: {}", e))?;
        std::fs::create_dir_all(&o.dir).map_err(|e| format!("Can't create {}: {}", o.dir.display(), e))?;
        let total: usize = entries.iter().map(|e| e.data.len()).sum::<usize>().max(1);
        let mut done = 0usize;
        let mut size = 0u64;
        // The files this installation owns; the uninstaller removes exactly these.
        let mut owned: Vec<String> = Vec::new();
        for e in &entries {
            if e.name.ends_with('/') || e.name.contains("..") || e.name.contains(':') {
                continue;
            }
            owned.push(e.name.replace('/', "\\"));
            let p = o.dir.join(e.name.replace('/', "\\"));
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).map_err(|err| format!("{}: {}", parent.display(), err))?;
            }
            progress(5 + (85 * done / total) as u32, &format!("Copying {}…", e.name));
            if let Err(err) = std::fs::write(&p, &e.data) {
                return Err(if err.raw_os_error() == Some(32) {
                    format!("{} is in use. Close Curust and run Setup again.", e.name)
                } else {
                    format!("Can't write {}: {}", p.display(), err)
                });
            }
            done += e.data.len();
            size += e.data.len() as u64;
        }
        // The uninstaller is a copy of this program.
        if let Ok(me) = std::env::current_exe() {
            let target = o.dir.join("uninstall.exe");
            if me != target {
                size += std::fs::copy(&me, &target).map_err(|e| format!("Can't write the uninstaller: {}", e))?;
            }
        }
        owned.push("uninstall.exe".into());
        owned.push(MANIFEST.into());
        // Keep the files of an earlier installation in the list (an update may drop some).
        if let Ok(old) = std::fs::read_to_string(o.dir.join(MANIFEST)) {
            for l in old.lines().map(str::trim).filter(|l| !l.is_empty()) {
                if !owned.iter().any(|x| x.eq_ignore_ascii_case(l)) {
                    owned.push(l.to_string());
                }
            }
        }
        std::fs::write(o.dir.join(MANIFEST), owned.join("\r\n")).map_err(|e| format!("Can't write {}: {}", MANIFEST, e))?;
        progress(90, "Removing the older per-user installation…");
        remove_per_user_install(&o.dir);
        progress(92, "Registering Curust…");
        let dir = o.dir.display().to_string();
        let exe = o.dir.join("curust.exe").display().to_string();
        let un = o.dir.join("uninstall.exe").display().to_string();
        reg_set(HKLM, UNINSTALL_KEY, Some("DisplayName"), "Curust")?;
        reg_set(HKLM, UNINSTALL_KEY, Some("DisplayVersion"), VERSION)?;
        reg_set(HKLM, UNINSTALL_KEY, Some("Publisher"), "Curust contributors")?;
        reg_set(HKLM, UNINSTALL_KEY, Some("InstallLocation"), &dir)?;
        reg_set(HKLM, UNINSTALL_KEY, Some("DisplayIcon"), &format!("{},0", exe))?;
        reg_set(HKLM, UNINSTALL_KEY, Some("UninstallString"), &format!("\"{}\" --uninstall", un))?;
        reg_set(HKLM, UNINSTALL_KEY, Some("QuietUninstallString"), &format!("\"{}\" --uninstall --silent", un))?;
        reg_set(HKLM, UNINSTALL_KEY, Some("InstallDate"), &today())?;
        reg_set(HKLM, UNINSTALL_KEY, Some("Comments"), "3D printing slicer: a Rust port of UltiMaker Cura 5.12")?;
        reg_set_dword(HKLM, UNINSTALL_KEY, "NoModify", 1);
        reg_set_dword(HKLM, UNINSTALL_KEY, "NoRepair", 1);
        reg_set_dword(HKLM, UNINSTALL_KEY, "EstimatedSize", (size / 1024) as u32);
        // "Open with" for models.
        let open_cmd = format!("\"{}\" \"%1\"", exe);
        reg_set(HKLM, UNINSTALL_KEY, Some("URLInfoAbout"), super::HOMEPAGE)?;
        reg_set(HKLM, UNINSTALL_KEY, Some("HelpLink"), &format!("{}/issues", super::HOMEPAGE))?;
        reg_set(HKLM, "Software\\Classes\\Applications\\curust.exe\\shell\\open\\command", None, &open_cmd)?;
        reg_set(HKLM, "Software\\Classes\\Applications\\curust.exe", Some("FriendlyAppName"), "Curust")?;
        for ext in EXTENSIONS {
            reg_set(HKLM, "Software\\Classes\\Applications\\curust.exe\\SupportedTypes", Some(ext), "")?;
        }
        if o.associate {
            reg_set(HKLM, &format!("Software\\Classes\\{}", PROGID), None, "3D model")?;
            reg_set(HKLM, &format!("Software\\Classes\\{}\\DefaultIcon", PROGID), None, &format!("{},0", exe))?;
            reg_set(HKLM, &format!("Software\\Classes\\{}\\shell\\open\\command", PROGID), None, &open_cmd)?;
            for ext in EXTENSIONS {
                reg_set(HKLM, &format!("Software\\Classes\\{}\\OpenWithProgids", ext), Some(PROGID), "")?;
            }
            // SAFETY: SHCNE_ASSOCCHANGED with no items.
            unsafe { SHChangeNotify(0x0800_0000, 0, std::ptr::null(), std::ptr::null()) };
        }
        progress(96, "Creating shortcuts…");
        if !create_shortcuts(&o.dir, o.desktop) {
            warnings.push("The Start menu shortcut could not be created (PowerShell is not available).".to_string());
        }
        progress(100, "Done.");
        Ok(warnings)
    }

    fn remove_dir_retry(p: &Path) -> bool {
        for _ in 0..20 {
            if !p.exists() || std::fs::remove_dir_all(p).is_ok() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        !p.exists()
    }

    fn remove_file_retry(p: &Path) -> bool {
        for _ in 0..20 {
            if !p.exists() || std::fs::remove_file(p).is_ok() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        !p.exists()
    }

    /// Remove the files listed in the installation's manifest, then the folders that are left
    /// empty. Anything else in the folder (files the user put there) stays.
    fn remove_installed_files(dir: &Path) -> bool {
        let Ok(list) = std::fs::read_to_string(dir.join(MANIFEST)) else {
            // No manifest: only remove the files Curust ships.
            let mut ok = true;
            for f in ["curust.exe", "curust-cli.exe", "curust-engine.exe", "curust-resources.zip", "LICENSE.txt", "README.txt", "uninstall.exe"] {
                ok &= remove_file_retry(&dir.join(f));
            }
            let _ = std::fs::remove_dir(dir);
            return ok;
        };
        let mut ok = true;
        let mut parents: Vec<PathBuf> = Vec::new();
        for rel in list.lines().map(str::trim).filter(|l| !l.is_empty()) {
            if rel.contains("..") || rel.contains(':') || rel.starts_with('\\') || rel.starts_with('/') {
                continue;
            }
            let p = dir.join(rel);
            ok &= remove_file_retry(&p);
            let mut q = p.parent().map(|q| q.to_path_buf());
            while let Some(d) = q {
                if d == dir || !d.starts_with(dir) {
                    break;
                }
                if !parents.contains(&d) {
                    parents.push(d.clone());
                }
                q = d.parent().map(|x| x.to_path_buf());
            }
        }
        // Deepest folders first; remove_dir only succeeds on empty folders.
        parents.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
        for d in parents {
            let _ = std::fs::remove_dir(&d);
        }
        let _ = std::fs::remove_dir(dir);
        ok
    }

    /// Runs from a temporary copy so the installed files (including uninstall.exe) can go.
    fn uninstall_run(dir: &Path, silent: bool) {
        std::thread::sleep(std::time::Duration::from_millis(800));
        remove_shortcuts();
        remove_registration(HKLM);
        // SAFETY: SHCNE_ASSOCCHANGED with no items.
        unsafe { SHChangeNotify(0x0800_0000, 0, std::ptr::null(), std::ptr::null()) };
        let ok = remove_installed_files(dir);
        if !silent {
            let settings = PathBuf::from(std::env::var("APPDATA").unwrap_or_default()).join("Curust");
            if settings.is_dir() && message(std::ptr::null_mut(), &format!("Also delete your Curust settings, printers, profiles and custom materials?\n\n{}", settings.display()), 0x24 | 0x100) == 6 {
                remove_dir_retry(&settings);
            }
            if ok {
                message(std::ptr::null_mut(), "Curust was removed from your computer.", 0x40);
            } else {
                message(std::ptr::null_mut(), &format!("Some files could not be removed (is Curust still running?):\n{}", dir.display()), 0x30);
            }
        }
        // Delete this temporary copy once it has exited.
        if let Ok(me) = std::env::current_exe() {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("cmd.exe").args(["/c", &format!("ping 127.0.0.1 -n 3 > nul & del \"{}\"", me.display())]).creation_flags(0x0800_0000).spawn();
        }
    }

    // ------------------------------------------------------------------ the window

    const ID_PATH: i32 = 101;
    const ID_DESKTOP: i32 = 102;
    const ID_ASSOC: i32 = 103;
    const ID_LAUNCH: i32 = 104;
    const ID_INSTALL: i32 = 105;
    const ID_CANCEL: i32 = 106;
    const ID_STATUS: i32 = 107;
    const ID_BROWSE: i32 = 108;
    const WM_APP_PROGRESS: u32 = 0x8001;
    const WM_APP_DONE: u32 = 0x8002;

    static MAIN: AtomicIsize = AtomicIsize::new(0);
    static mut STATUS_TEXT: String = String::new();
    static mut RESULT: Option<Result<Vec<String>, String>> = None;
    static mut LAUNCH: bool = false;
    static mut INSTALLED_DIR: Option<PathBuf> = None;

    fn scale(v: i32) -> i32 {
        // SAFETY: plain call.
        let dpi = unsafe { GetDpiForSystem() } as i32;
        v * dpi.max(96) / 96
    }

    fn text_of(h: Hwnd, id: i32) -> String {
        let mut buf = vec![0u16; 1024];
        // SAFETY: buffer of the given size.
        let n = unsafe { GetWindowTextW(GetDlgItem(h, id), buf.as_mut_ptr(), buf.len() as i32) };
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }

    fn checked(h: Hwnd, id: i32) -> bool {
        // SAFETY: BM_GETCHECK.
        unsafe { SendMessageW(GetDlgItem(h, id), 0x00F0, 0, 0) == 1 }
    }

    extern "system" fn wnd_proc(h: Hwnd, msg: u32, w: usize, l: isize) -> isize {
        // SAFETY: standard window procedure; statics are only touched on the UI thread except
        // where noted (the worker writes them before posting a message).
        unsafe {
            match msg {
                0x0111 => {
                    // WM_COMMAND
                    let id = (w & 0xffff) as i32;
                    if id == ID_CANCEL {
                        DestroyWindow(h);
                    } else if id == ID_BROWSE {
                        if let Some(dir) = browse_folder(h) {
                            // Like other installers: install into a "Curust" folder inside the chosen one.
                            let dir = if dir.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("curust")) { dir } else { dir.join("Curust") };
                            SetWindowTextW(GetDlgItem(h, ID_PATH), wide(&dir.display().to_string()).as_ptr());
                        }
                    } else if id == ID_INSTALL {
                        let dir = PathBuf::from(text_of(h, ID_PATH).trim());
                        if dir.as_os_str().is_empty() {
                            message(h, "Choose a folder to install Curust to.", 0x30);
                            return 0;
                        }
                        let foreign = std::fs::read_dir(&dir).map(|mut r| r.next().is_some()).unwrap_or(false) && !dir.join("curust.exe").exists();
                        if foreign && message(h, &format!("{} already contains other files. Install Curust into this folder anyway?\n\n(Uninstalling only removes Curust's own files.)", dir.display()), 0x24 | 0x100) != 6 {
                            return 0;
                        }
                        let o = Options { dir: dir.clone(), desktop: checked(h, ID_DESKTOP), associate: checked(h, ID_ASSOC), launch: checked(h, ID_LAUNCH) };
                        LAUNCH = o.launch;
                        INSTALLED_DIR = Some(dir);
                        for id in [ID_PATH, ID_BROWSE, ID_DESKTOP, ID_ASSOC, ID_LAUNCH, ID_INSTALL, ID_CANCEL] {
                            EnableWindow(GetDlgItem(h, id), 0);
                        }
                        let hw = h as isize;
                        std::thread::spawn(move || {
                            let r = install(&o, &|pct, text| {
                                STATUS_TEXT = format!("{}%  {}", pct, text);
                                PostMessageW(hw as Hwnd, WM_APP_PROGRESS, 0, 0);
                            });
                            RESULT = Some(r);
                            PostMessageW(hw as Hwnd, WM_APP_DONE, 0, 0);
                        });
                    }
                    0
                }
                WM_APP_PROGRESS => {
                    #[allow(static_mut_refs)]
                    let t = wide(&STATUS_TEXT);
                    SetWindowTextW(GetDlgItem(h, ID_STATUS), t.as_ptr());
                    0
                }
                WM_APP_DONE => {
                    #[allow(static_mut_refs)]
                    match RESULT.take() {
                        Some(Ok(warnings)) => {
                            let mut text = String::from("Curust is installed. You'll find it in the Start menu.");
                            for w in warnings {
                                text.push_str("\n\n");
                                text.push_str(&w);
                            }
                            message(h, &text, 0x40);
                            #[allow(static_mut_refs)]
                            if LAUNCH {
                                if let Some(d) = &INSTALLED_DIR {
                                    launch_unelevated(&d.join("curust.exe"));
                                }
                            }
                            DestroyWindow(h);
                        }
                        Some(Err(e)) => {
                            message(h, &format!("Setup could not finish:\n\n{}", e), 0x10);
                            for id in [ID_PATH, ID_BROWSE, ID_DESKTOP, ID_ASSOC, ID_LAUNCH, ID_INSTALL, ID_CANCEL] {
                                EnableWindow(GetDlgItem(h, id), 1);
                            }
                        }
                        None => {}
                    }
                    0
                }
                0x0002 => {
                    // WM_DESTROY
                    PostQuitMessage(0);
                    0
                }
                _ => DefWindowProcW(h, msg, w, l),
            }
        }
    }

    /// The shell's folder picker.
    fn browse_folder(owner: Hwnd) -> Option<PathBuf> {
        let mut name = vec![0u16; 260];
        let title = wide("Choose the folder to install Curust to:");
        let bi = BrowseInfoW { owner, root: std::ptr::null(), display_name: name.as_mut_ptr(), title: title.as_ptr(), flags: 0x0001 | 0x0040, callback: std::ptr::null(), lparam: 0, image: 0 };
        // SAFETY: BROWSEINFOW with valid buffers; the returned PIDL is freed with CoTaskMemFree.
        unsafe {
            let pidl = SHBrowseForFolderW(&bi);
            if pidl.is_null() {
                return None;
            }
            let mut path = vec![0u16; 1024];
            let ok = SHGetPathFromIDListW(pidl, path.as_mut_ptr()) != 0;
            CoTaskMemFree(pidl);
            if !ok {
                return None;
            }
            let n = path.iter().position(|&c| c == 0).unwrap_or(path.len());
            Some(PathBuf::from(String::from_utf16_lossy(&path[..n])))
        }
    }

    fn run_window() {
        // SAFETY: standard Win32 window creation and message loop on this thread.
        unsafe {
            SetProcessDPIAware();
            CoInitializeEx(std::ptr::null_mut(), 0x2);
            let inst = GetModuleHandleW(std::ptr::null());
            let class = wide("CurustSetup");
            let wc = WndClassExW {
                cb_size: std::mem::size_of::<WndClassExW>() as u32,
                style: 0,
                wnd_proc,
                cls_extra: 0,
                wnd_extra: 0,
                instance: inst,
                icon: LoadIconW(inst, 1),
                cursor: LoadCursorW(std::ptr::null_mut(), 32512),
                background: (15 + 1) as *mut c_void, // COLOR_BTNFACE + 1
                menu_name: std::ptr::null(),
                class_name: class.as_ptr(),
                icon_sm: LoadIconW(inst, 1),
            };
            RegisterClassExW(&wc);
            let (ww, wh) = (scale(560), scale(380));
            let x = (GetSystemMetrics(0) - ww) / 2;
            let y = (GetSystemMetrics(1) - wh) / 2;
            let h = CreateWindowExW(0, class.as_ptr(), wide(&format!("Curust {} Setup", VERSION)).as_ptr(), 0x00C8_0000 | 0x0002_0000, x, y, ww, wh, std::ptr::null_mut(), 0, inst, std::ptr::null_mut());
            MAIN.store(h as isize, Ordering::SeqCst);
            let font = CreateFontW(-scale(15), 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, wide("Segoe UI").as_ptr());
            let bold = CreateFontW(-scale(22), 0, 0, 0, 600, 0, 0, 0, 1, 0, 0, 5, 0, wide("Segoe UI").as_ptr());
            let font = if font.is_null() { GetStockObject(17) } else { font };
            let ctl = |class: &str, text: &str, style: u32, ex: u32, x: i32, y: i32, w: i32, hh: i32, id: i32, f: *mut c_void| -> Hwnd {
                let c = CreateWindowExW(ex, wide(class).as_ptr(), wide(text).as_ptr(), 0x5000_0000 | style, scale(x), scale(y), scale(w), scale(hh), h, id as isize, inst, std::ptr::null_mut());
                SendMessageW(c, 0x0030, f as usize, 1);
                c
            };
            ctl("STATIC", "Curust", 0, 0, 24, 18, 500, 32, 0, bold);
            ctl("STATIC", "A 3D printing slicer: a Rust port of UltiMaker Cura 5.12. Setup installs it for everyone who uses this computer.", 0, 0, 24, 54, 510, 44, 0, font);
            ctl("STATIC", "Install to:", 0, 0, 24, 110, 510, 20, 0, font);
            ctl("EDIT", &default_dir().display().to_string(), 0x0080 | 0x0001_0000, 0x200, 24, 132, 400, 26, ID_PATH, font);
            ctl("BUTTON", "Browse…", 0x0001_0000, 0, 434, 131, 100, 28, ID_BROWSE, font);
            ctl("BUTTON", "Create a desktop shortcut", 0x0003 | 0x0001_0000, 0, 24, 172, 510, 24, ID_DESKTOP, font);
            ctl("BUTTON", "Show Curust in \"Open with\" for STL, 3MF and OBJ files", 0x0003 | 0x0001_0000, 0, 24, 198, 510, 24, ID_ASSOC, font);
            ctl("BUTTON", "Start Curust when Setup finishes", 0x0003 | 0x0001_0000, 0, 24, 224, 510, 24, ID_LAUNCH, font);
            for id in [ID_DESKTOP, ID_ASSOC, ID_LAUNCH] {
                SendMessageW(GetDlgItem(h, id), 0x00F1, 1, 0);
            }
            ctl("STATIC", "", 0, 0, 24, 262, 510, 22, ID_STATUS, font);
            ctl("BUTTON", "Install", 0x0001 | 0x0001_0000, 0, 330, 296, 100, 32, ID_INSTALL, font);
            ctl("BUTTON", "Cancel", 0x0001_0000, 0, 436, 296, 100, 32, ID_CANCEL, font);
            ShowWindow(h, 1);
            let mut m: Msg = std::mem::zeroed();
            while GetMessageW(&mut m, std::ptr::null_mut(), 0, 0) > 0 {
                if IsDialogMessageW(h, &mut m) == 0 {
                    TranslateMessage(&m);
                    DispatchMessageW(&m);
                }
            }
        }
    }

    pub fn main(args: Vec<String>) {
        let has = |f: &str| args.iter().any(|a| a.eq_ignore_ascii_case(f));
        let silent = has("/S") || has("--silent") || has("/silent");
        if let Some(i) = args.iter().position(|a| a == "--uninstall-run") {
            let dir = PathBuf::from(args.get(i + 1).cloned().unwrap_or_default());
            uninstall_run(&dir, silent);
            return;
        }
        if has("--uninstall") {
            let dir = reg_get(HKLM, UNINSTALL_KEY, "InstallLocation").map(PathBuf::from).or_else(|| std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf()))).unwrap_or_default();
            if !silent && message(std::ptr::null_mut(), &format!("Remove Curust from your computer?\n\n{}", dir.display()), 0x24) != 6 {
                return;
            }
            // Continue from a copy in the temp folder so this file can be deleted too.
            let tmp = std::env::temp_dir().join(format!("curust-uninstall-{}.exe", std::process::id()));
            if let Ok(me) = std::env::current_exe() {
                if std::fs::copy(&me, &tmp).is_ok() {
                    let mut c = std::process::Command::new(&tmp);
                    c.arg("--uninstall-run").arg(&dir);
                    if silent {
                        c.arg("--silent");
                    }
                    if c.spawn().is_ok() {
                        return;
                    }
                }
            }
            uninstall_run(&dir, silent);
            return;
        }
        if silent {
            let dir = args.iter().find_map(|a| a.strip_prefix("/D=").or_else(|| a.strip_prefix("--dir="))).map(PathBuf::from).unwrap_or_else(default_dir);
            let o = Options { dir, desktop: !has("--no-desktop"), associate: !has("--no-associate"), launch: false };
            match install(&o, &|_, _| {}) {
                Ok(_) => std::process::exit(0),
                Err(e) => {
                    eprintln!("{}", e);
                    std::process::exit(1)
                }
            }
        }
        run_window();
    }
}
