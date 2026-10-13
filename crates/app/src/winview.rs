//! The Windows application window: a native Win32 window hosting the interface in Microsoft
//! Edge WebView2 (the Chromium-based web view that ships with Windows 10/11, also present when the
//! Edge browser itself has been removed).
//!
//! No WebView2Loader.dll is needed: like the loader, Curust finds the installed WebView2 runtime
//! (registry key of the stable channel, or the runtime's folder) and calls
//! `CreateWebViewEnvironmentWithOptionsInternal` in its `EmbeddedBrowserWebView.dll`. The COM
//! interfaces are used through their vtables (method indices from WebView2.h).

use std::cell::RefCell;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

type Hwnd = *mut c_void;
type Hresult = i32;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct Guid(u32, u16, u16, [u8; 8]);

const IID_IUNKNOWN: Guid = Guid(0, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_ENV_COMPLETED: Guid = Guid(0x4e8a3389, 0xc9d8, 0x4bd2, [0xb6, 0xb5, 0x12, 0x4f, 0xee, 0x6c, 0xc1, 0x4d]);
const IID_CONTROLLER_COMPLETED: Guid = Guid(0x6c4819f3, 0xc9b7, 0x4260, [0x81, 0x27, 0xc9, 0xf5, 0xbd, 0xe7, 0xf6, 0x8c]);

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
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

#[repr(C)]
struct WndClassExW {
    cb_size: u32,
    style: u32,
    wnd_proc: unsafe extern "system" fn(Hwnd, u32, usize, isize) -> isize,
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

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}

#[link(name = "user32")]
extern "system" {
    fn RegisterClassExW(class: *const WndClassExW) -> u16;
    fn CreateWindowExW(ex_style: u32, class: *const u16, title: *const u16, style: u32, x: i32, y: i32, w: i32, h: i32, parent: Hwnd, menu: *mut c_void, instance: *mut c_void, param: *mut c_void) -> Hwnd;
    fn ShowWindow(hwnd: Hwnd, cmd: i32) -> i32;
    fn UpdateWindow(hwnd: Hwnd) -> i32;
    fn DestroyWindow(hwnd: Hwnd) -> i32;
    fn GetMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32) -> i32;
    fn TranslateMessage(msg: *const Msg) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn DefWindowProcW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn PostQuitMessage(code: i32);
    fn GetClientRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
    fn LoadIconW(instance: *mut c_void, name: *const u16) -> *mut c_void;
    fn LoadCursorW(instance: *mut c_void, name: *const u16) -> *mut c_void;
    fn SystemParametersInfoW(action: u32, param: u32, pv: *mut c_void, ini: u32) -> i32;
    fn SetProcessDpiAwarenessContext(value: isize) -> i32;
}

#[link(name = "ole32")]
extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, coinit: u32) -> Hresult;
}

#[link(name = "advapi32")]
extern "system" {
    fn RegOpenKeyExW(key: isize, sub_key: *const u16, options: u32, sam: u32, result: *mut isize) -> i32;
    fn RegQueryValueExW(key: isize, name: *const u16, reserved: *mut u32, kind: *mut u32, data: *mut u8, len: *mut u32) -> i32;
    fn RegCloseKey(key: isize) -> i32;
}

const WM_DESTROY: u32 = 0x0002;
const WM_MOVE: u32 = 0x0003;
const WM_SIZE: u32 = 0x0005;
const WM_SETFOCUS: u32 = 0x0007;
const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as i32 as isize;
const HKEY_LOCAL_MACHINE: isize = 0x8000_0002u32 as i32 as isize;
const KEY_READ: u32 = 0x20019;
const KEY_WOW64_32KEY: u32 = 0x0200;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Method `index` of a COM object's vtable, as the function pointer type `F`.
unsafe fn method<F: Copy>(obj: *mut c_void, index: usize) -> F {
    let vtbl = *(obj as *const *const usize);
    std::mem::transmute_copy::<usize, F>(&*vtbl.add(index))
}

// ------------------------------------------------------------------------------------------------
// Locating the runtime
// ------------------------------------------------------------------------------------------------

const RUNTIME_DLL: &str = "EBWebView\\x64\\EmbeddedBrowserWebView.dll";

fn registry_runtime_dir() -> Option<PathBuf> {
    let sub = wide("Software\\Microsoft\\EdgeUpdate\\ClientState\\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}");
    let name = wide("EBWebView");
    for root in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        // SAFETY: registry reads into a correctly sized buffer.
        unsafe {
            let mut key: isize = 0;
            if RegOpenKeyExW(root, sub.as_ptr(), 0, KEY_READ | KEY_WOW64_32KEY, &mut key) != 0 {
                continue;
            }
            let mut buf = vec![0u16; 1024];
            let mut len = (buf.len() * 2) as u32;
            let r = RegQueryValueExW(key, name.as_ptr(), std::ptr::null_mut(), std::ptr::null_mut(), buf.as_mut_ptr() as *mut u8, &mut len);
            RegCloseKey(key);
            if r == 0 {
                let n = buf.iter().position(|&c| c == 0).unwrap_or((len / 2) as usize);
                let dir = PathBuf::from(String::from_utf16_lossy(&buf[..n]));
                if dir.join(RUNTIME_DLL).is_file() {
                    return Some(dir);
                }
            }
        }
    }
    None
}

/// Newest `<version>` folder under the runtime's install folders.
fn scanned_runtime_dir() -> Option<PathBuf> {
    let mut bases = Vec::new();
    for var in ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"] {
        if let Ok(p) = std::env::var(var) {
            bases.push(PathBuf::from(p).join("Microsoft").join("EdgeWebView").join("Application"));
        }
    }
    let mut best: Option<(Vec<u64>, PathBuf)> = None;
    for base in bases {
        let Ok(rd) = std::fs::read_dir(&base) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let v: Vec<u64> = name.split('.').map(|p| p.parse().unwrap_or(0)).collect();
            if v.len() == 4 && e.path().join(RUNTIME_DLL).is_file() && best.as_ref().map(|b| v > b.0).unwrap_or(true) {
                best = Some((v, e.path()));
            }
        }
    }
    best.map(|b| b.1)
}

pub fn runtime_dll() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER") {
        let p = PathBuf::from(p).join(RUNTIME_DLL);
        if p.is_file() {
            return Some(p);
        }
    }
    registry_runtime_dir().or_else(scanned_runtime_dir).map(|d| d.join(RUNTIME_DLL))
}

// ------------------------------------------------------------------------------------------------
// Completion handlers (minimal COM objects)
// ------------------------------------------------------------------------------------------------

#[repr(C)]
struct HandlerVtbl {
    query_interface: unsafe extern "system" fn(*mut Handler, *const Guid, *mut *mut c_void) -> Hresult,
    add_ref: unsafe extern "system" fn(*mut Handler) -> u32,
    release: unsafe extern "system" fn(*mut Handler) -> u32,
    invoke: unsafe extern "system" fn(*mut Handler, Hresult, *mut c_void) -> Hresult,
}

#[repr(C)]
struct Handler {
    vtbl: *const HandlerVtbl,
    refs: AtomicU32,
    iid: Guid,
}

unsafe extern "system" fn h_query_interface(this: *mut Handler, iid: *const Guid, out: *mut *mut c_void) -> Hresult {
    if *iid == IID_IUNKNOWN || *iid == (*this).iid {
        h_add_ref(this);
        *out = this as *mut c_void;
        0
    } else {
        *out = std::ptr::null_mut();
        0x8000_4002u32 as i32 // E_NOINTERFACE
    }
}

unsafe extern "system" fn h_add_ref(this: *mut Handler) -> u32 {
    (*this).refs.fetch_add(1, Ordering::SeqCst) + 1
}

unsafe extern "system" fn h_release(this: *mut Handler) -> u32 {
    let n = (*this).refs.fetch_sub(1, Ordering::SeqCst) - 1;
    if n == 0 {
        drop(Box::from_raw(this));
    }
    n
}

static ENV_VTBL: HandlerVtbl = HandlerVtbl { query_interface: h_query_interface, add_ref: h_add_ref, release: h_release, invoke: env_completed };
static CTRL_VTBL: HandlerVtbl = HandlerVtbl { query_interface: h_query_interface, add_ref: h_add_ref, release: h_release, invoke: controller_completed };

fn new_handler(vtbl: &'static HandlerVtbl, iid: Guid) -> *mut Handler {
    Box::into_raw(Box::new(Handler { vtbl, refs: AtomicU32::new(1), iid }))
}

// ------------------------------------------------------------------------------------------------
// Window state (one window, on its own thread)
// ------------------------------------------------------------------------------------------------

struct State {
    hwnd: Hwnd,
    url: Vec<u16>,
    controller: *mut c_void,
    status: Option<mpsc::Sender<Result<(), String>>>,
    failed: bool,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State { hwnd: std::ptr::null_mut(), url: Vec::new(), controller: std::ptr::null_mut(), status: None, failed: false });
}

fn report(result: Result<(), String>) {
    let tx = STATE.with(|s| {
        let mut s = s.borrow_mut();
        if result.is_err() {
            s.failed = true;
        }
        s.status.take()
    });
    if let Some(tx) = tx {
        let _ = tx.send(result);
    }
}

fn fail(msg: String) {
    report(Err(msg));
    let hwnd = STATE.with(|s| s.borrow().hwnd);
    // SAFETY: our own window, on its thread.
    unsafe {
        DestroyWindow(hwnd);
    }
}

fn resize_webview() {
    let (hwnd, controller) = STATE.with(|s| (s.borrow().hwnd, s.borrow().controller));
    if controller.is_null() {
        return;
    }
    // SAFETY: valid window and controller; put_Bounds is ICoreWebView2Controller method 6.
    unsafe {
        let mut r = Rect::default();
        GetClientRect(hwnd, &mut r);
        let put_bounds: unsafe extern "system" fn(*mut c_void, Rect) -> Hresult = method(controller, 6);
        put_bounds(controller, r);
    }
}

unsafe extern "system" fn env_completed(_this: *mut Handler, hr: Hresult, env: *mut c_void) -> Hresult {
    if hr < 0 || env.is_null() {
        fail(format!("WebView2 environment creation failed (0x{:08x})", hr as u32));
        return 0;
    }
    let hwnd = STATE.with(|s| s.borrow().hwnd);
    let handler = new_handler(&CTRL_VTBL, IID_CONTROLLER_COMPLETED);
    // ICoreWebView2Environment::CreateCoreWebView2Controller (method 3).
    let create: unsafe extern "system" fn(*mut c_void, Hwnd, *mut Handler) -> Hresult = method(env, 3);
    let r = create(env, hwnd, handler);
    h_release(handler);
    if r < 0 {
        fail(format!("WebView2 controller creation failed (0x{:08x})", r as u32));
    }
    0
}

unsafe extern "system" fn controller_completed(_this: *mut Handler, hr: Hresult, controller: *mut c_void) -> Hresult {
    if hr < 0 || controller.is_null() {
        fail(format!("WebView2 controller creation failed (0x{:08x})", hr as u32));
        return 0;
    }
    let add_ref: unsafe extern "system" fn(*mut c_void) -> u32 = method(controller, 1);
    add_ref(controller);
    STATE.with(|s| s.borrow_mut().controller = controller);
    resize_webview();
    // ICoreWebView2Controller::get_CoreWebView2 (25) → ICoreWebView2.
    let get_core: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult = method(controller, 25);
    let mut webview: *mut c_void = std::ptr::null_mut();
    if get_core(controller, &mut webview) < 0 || webview.is_null() {
        fail("WebView2 has no web view".into());
        return 0;
    }
    // ICoreWebView2::get_Settings (3) → ICoreWebView2Settings::put_IsStatusBarEnabled (10).
    let get_settings: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult = method(webview, 3);
    let mut settings: *mut c_void = std::ptr::null_mut();
    if get_settings(webview, &mut settings) >= 0 && !settings.is_null() {
        let put_status_bar: unsafe extern "system" fn(*mut c_void, i32) -> Hresult = method(settings, 10);
        put_status_bar(settings, 0);
        let release: unsafe extern "system" fn(*mut c_void) -> u32 = method(settings, 2);
        release(settings);
    }
    // ICoreWebView2::Navigate (5).
    let url = STATE.with(|s| s.borrow().url.clone());
    let navigate: unsafe extern "system" fn(*mut c_void, *const u16) -> Hresult = method(webview, 5);
    let r = navigate(webview, url.as_ptr());
    let release: unsafe extern "system" fn(*mut c_void) -> u32 = method(webview, 2);
    release(webview);
    if r < 0 {
        fail(format!("WebView2 navigation failed (0x{:08x})", r as u32));
        return 0;
    }
    report(Ok(()));
    0
}

unsafe extern "system" fn wnd_proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        WM_SIZE => {
            resize_webview();
            0
        }
        WM_MOVE => {
            let controller = STATE.with(|s| s.borrow().controller);
            if !controller.is_null() {
                let notify: unsafe extern "system" fn(*mut c_void) -> Hresult = method(controller, 23);
                notify(controller);
            }
            0
        }
        WM_SETFOCUS => {
            let controller = STATE.with(|s| s.borrow().controller);
            if !controller.is_null() {
                // MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC)
                let move_focus: unsafe extern "system" fn(*mut c_void, i32) -> Hresult = method(controller, 12);
                move_focus(controller, 0);
            }
            0
        }
        WM_DESTROY => {
            let controller = STATE.with(|s| std::mem::replace(&mut s.borrow_mut().controller, std::ptr::null_mut()));
            if !controller.is_null() {
                let close: unsafe extern "system" fn(*mut c_void) -> Hresult = method(controller, 24);
                close(controller);
                let release: unsafe extern "system" fn(*mut c_void) -> u32 = method(controller, 2);
                release(controller);
            }
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

type CreateEnvironmentInternal = unsafe extern "system" fn(bool, i32, *const u16, *mut c_void, *mut Handler) -> Hresult;

/// Creates the window and runs its message loop until it is closed.
fn window_thread(url: String, data_dir: PathBuf, dll: PathBuf, status: mpsc::Sender<Result<(), String>>, closed: Arc<AtomicBool>) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.url = wide(&url);
        s.status = Some(status);
    });
    // SAFETY: plain Win32 / COM calls on this thread; pointers are valid for the calls.
    unsafe {
        SetProcessDpiAwarenessContext(-4); // per-monitor aware v2
        CoInitializeEx(std::ptr::null_mut(), 0x2); // COINIT_APARTMENTTHREADED
        let module = LoadLibraryW(wide(&dll.to_string_lossy()).as_ptr());
        if module.is_null() {
            report(Err(format!("cannot load {}", dll.display())));
            return;
        }
        let proc_addr = GetProcAddress(module, b"CreateWebViewEnvironmentWithOptionsInternal\0".as_ptr());
        if proc_addr.is_null() {
            report(Err("the WebView2 runtime has no CreateWebViewEnvironmentWithOptionsInternal".into()));
            return;
        }
        let create_env: CreateEnvironmentInternal = std::mem::transmute(proc_addr);

        let instance = GetModuleHandleW(std::ptr::null());
        let class_name = wide("CurustWindow");
        let icon = LoadIconW(instance, 1 as *const u16);
        let class = WndClassExW {
            cb_size: std::mem::size_of::<WndClassExW>() as u32,
            style: 0,
            wnd_proc,
            cls_extra: 0,
            wnd_extra: 0,
            instance,
            icon,
            cursor: LoadCursorW(std::ptr::null_mut(), 32512 as *const u16),
            background: 6 as *mut c_void, // COLOR_WINDOW + 1
            menu_name: std::ptr::null(),
            class_name: class_name.as_ptr(),
            icon_sm: icon,
        };
        RegisterClassExW(&class);
        // 85 % of the work area, centred.
        let mut work = Rect::default();
        SystemParametersInfoW(0x30, 0, &mut work as *mut Rect as *mut c_void, 0); // SPI_GETWORKAREA
        let (ww, wh) = (work.right - work.left, work.bottom - work.top);
        let (w, h) = if ww > 0 && wh > 0 { (ww * 85 / 100, wh * 85 / 100) } else { (1400, 900) };
        let (x, y) = (work.left + (ww - w) / 2, work.top + (wh - h) / 2);
        let hwnd = CreateWindowExW(0, class_name.as_ptr(), wide("Curust").as_ptr(), WS_OVERLAPPEDWINDOW, x, y, w, h, std::ptr::null_mut(), std::ptr::null_mut(), instance, std::ptr::null_mut());
        if hwnd.is_null() {
            report(Err("cannot create the window".into()));
            return;
        }
        STATE.with(|s| s.borrow_mut().hwnd = hwnd);
        ShowWindow(hwnd, 1); // SW_SHOWNORMAL
        UpdateWindow(hwnd);

        let _ = std::fs::create_dir_all(&data_dir);
        let data = wide(&data_dir.to_string_lossy());
        let handler = new_handler(&ENV_VTBL, IID_ENV_COMPLETED);
        let hr = create_env(true, 0, data.as_ptr(), std::ptr::null_mut(), handler);
        h_release(handler);
        if hr < 0 {
            fail(format!("WebView2 is not available (0x{:08x})", hr as u32));
        }

        let mut msg: Msg = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    let failed = STATE.with(|s| s.borrow().failed);
    // Closed before the page was shown: still a deliberate close.
    report(Ok(()));
    if !failed {
        closed.store(true, Ordering::SeqCst);
    }
}

/// Opens the application window on its own thread. Returns a flag that becomes true when the
/// user closes the window, or an error (no WebView2 runtime, creation failed) so the caller can
/// fall back to a browser.
pub fn open(url: &str, data_dir: &Path) -> Result<Arc<AtomicBool>, String> {
    let dll = runtime_dll().ok_or("the Microsoft Edge WebView2 runtime is not installed")?;
    let closed = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let (url, data_dir, flag) = (url.to_string(), data_dir.to_path_buf(), closed.clone());
    std::thread::Builder::new()
        .name("window".into())
        .stack_size(8 << 20)
        .spawn(move || window_thread(url, data_dir, dll, tx, flag))
        .map_err(|e| e.to_string())?;
    match rx.recv_timeout(Duration::from_secs(60)) {
        Ok(Ok(())) => Ok(closed),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("WebView2 did not start in time".into()),
    }
}

/// Opens a URL with the default browser (ShellExecute, no console window).
pub fn open_in_default_browser(url: &str) {
    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteW(hwnd: Hwnd, op: *const u16, file: *const u16, params: *const u16, dir: *const u16, show: i32) -> *mut c_void;
    }
    // SAFETY: NUL-terminated wide strings.
    unsafe {
        ShellExecuteW(std::ptr::null_mut(), wide("open").as_ptr(), wide(url).as_ptr(), std::ptr::null(), std::ptr::null(), 1);
    }
}
