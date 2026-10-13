//! Curust: a complete slicer. Without arguments it starts the graphical interface (a local web
//! application shown in an Edge/Chrome app window); it also provides command line slicing and,
//! internally, the slicing engine (`curust engine-job`, started by the interface for every
//! slice, like Cura starts CuraEngine).
//!
//! Two executables share this library: `curust` (the application; a windowed program on
//! Windows) and `curust-cli` (the same commands as a console program).

mod buildvolume;
mod hull;
mod image_import;
mod jpeg;
mod qimage;
mod mesh_io;
mod materials;
mod marketplace;
mod netprint;
mod persist;
mod postproc;
mod snapshot;
mod usbprint;
mod profiles;
mod resources;
mod scene;
mod serial;
mod server;
mod session;
mod slicing;
mod web;
#[cfg(windows)]
mod winview;
mod xmlmini;
mod zip;

use curust_settings::slice_message::TimeTokens;
use session::{Session, Target};

fn usage() {
    println!(
        "Curust {} — a 3D printing slicer compatible with UltiMaker Cura 5.12

Usage:
  curust [gui] [--port N] [--no-browser] [--resources DIR] [MODEL...]
      Start the graphical interface (and open the given models).
  curust slice-models --machine DEFINITION [options] -o OUTPUT.gcode MODEL [MODEL...]
      Slice models from the command line exactly like Cura would. Options:
        --resources DIR           Cura resources directory (auto-detected otherwise)
        --printer NAME            use a printer saved in the interface instead of --machine
        --machine-name NAME       name of the new printer instance
        --variant POS=NAME        nozzle of an extruder (e.g. 0=\"AA 0.4\")
        --material POS=BASE_ID    material of an extruder (e.g. 0=generic_pla)
        --import-material FILE    import an .xml.fdm_material first
        --quality TYPE            quality type (e.g. normal, draft)
        --intent CATEGORY         intent (e.g. engineering)
        --disable-extruder POS
        --set KEY=VALUE           user setting (stored like Cura's settings panel)
        --extruder-set POS:KEY=VALUE
        --raw-set KEY=JSON        store a typed value directly in the global user container
        --raw-extruder-set POS:KEY=JSON
        --create-profile NAME     save the settings as a custom profile and slice with it
        --import-profile FILE     import a .curaprofile (or a profile from a G-code file)
        --export-profile FILE     write the active custom profile as a .curaprofile
        --at X,Y                  place the next model at X,Y (mm from the plate centre)
        --extruder POS            print the next model with this extruder
        --object-set KEY=VALUE    per-model setting for the next model
        --support-blocker X,Y,Z   add a support blocker on the previous model where a click at
                                  scene point X,Y,Z (Y up) would put it
        --image-options K=V,...   options for image (heightmap) models:
                                  size, peak, base, smoothing, lighter_is_higher, transparency, transmittance
        --post-process FILE       post-processing scripts, in Cura's stored format
        --add-script KEY[;k=v...] add a post-processing script (Cura's script key)
        --add-script-json JSON    the same as [\"Key\", {{\"setting\": value}}]
        --fixed-time              use fixed {{time}}/{{date}} tokens (reproducible output)
  curust slice ...              CuraEngine-compatible command line slicing
  curust list-machines          printer definitions
  curust scripts                post-processing scripts that can be used
  curust market-search [--type plugin|material] [TEXT]
                                search the Cura Marketplace
  curust install-package FILE.curapackage|PACKAGE_ID
  curust uninstall-package PACKAGE_ID
  curust packages               installed packages (Curust's and Cura's)
  curust usb-ports              serial ports with a printer
  curust usb-print --port PORT [--baud N] FILE.gcode
                                print over USB (Cura's USB printing protocol)
  curust net-test|net-status --kind octoprint|moonraker|prusalink|duet --url URL [--key KEY] [--user U]
  curust net-print --kind ... --url URL [--key KEY] [--no-start] FILE.gcode
                                upload (and start) a print on a network printer
  curust help",
        env!("CARGO_PKG_VERSION")
    );
}

/// Stack size of the thread that does all the work. Windows gives the main thread only 1 MiB,
/// less than slicing deep geometry or evaluating long setting formulas may need.
const STACK_SIZE: usize = 64 << 20;

/// The project's home page (source code, releases, issues).
pub const HOMEPAGE: &str = "https://github.com/WildWetWilly/Curust";

/// Runs Curust with a command line (`args[0]` is the program name).
pub fn run(args: Vec<String>) {
    // Without a console (the Windows application), show panics in a message box too.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_hook(info);
        #[cfg(windows)]
        win::message_box_if_no_console("Curust", &format!("Curust stopped because of an internal error:\n\n{}", info));
    }));
    let worker = std::thread::Builder::new().name("curust".into()).stack_size(STACK_SIZE).spawn(move || dispatch(args));
    match worker {
        Ok(handle) => {
            if handle.join().is_err() {
                // The panic message has been printed by the panic hook.
                std::process::exit(101);
            }
        }
        Err(e) => fatal(&format!("Unable to start: {}", e)),
    }
}

/// Reports an error that ends the program. Programs without a console (the Windows
/// application started from Explorer) show it in a message box.
pub fn fatal(msg: &str) -> ! {
    eprintln!("Error: {}", msg);
    #[cfg(windows)]
    win::message_box_if_no_console("Curust", msg);
    std::process::exit(1);
}

#[cfg(windows)]
mod win {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetConsoleWindow() -> *mut core::ffi::c_void;
    }
    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(hwnd: *mut core::ffi::c_void, text: *const u16, caption: *const u16, kind: u32) -> i32;
    }

    pub fn message_box_if_no_console(caption: &str, text: &str) {
        let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        // SAFETY: plain Win32 calls with valid, NUL-terminated UTF-16 strings.
        unsafe {
            if GetConsoleWindow().is_null() {
                let (t, c) = (wide(text), wide(caption));
                MessageBoxW(std::ptr::null_mut(), t.as_ptr(), c.as_ptr(), 0x10 /* MB_ICONERROR */);
            }
        }
    }
}

fn dispatch(args: Vec<String>) {
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("gui");
    match cmd {
        "engine-job" | "slice" => {
            curust_engine::application::run(args);
        }
        "help" | "--help" | "-h" => usage(),
        "version" | "--version" | "-V" => println!("Curust {}", env!("CARGO_PKG_VERSION")),
        "list-machines" => {
            let res = arg_value(&args, "--resources");
            match resources::find_resources(res.as_deref()) {
                Ok(r) => {
                    let s = Session::new(r);
                    for (id, name, manufacturer) in s.available_definitions() {
                        println!("{:40} {:30} {}", id, manufacturer, name);
                    }
                }
                Err(e) => fail(&e),
            }
        }
        "image-debug" => {
            // Testing aid: what Curust reads from an image (Qt-equivalent pixels, scaled copies
            // and ImageReader meshes), for comparison with Cura (tools/oracle/image_check.py).
            if let Err(e) = image_debug(args.get(2..).unwrap_or(&[])) {
                fail(&e);
            }
        }
        "usb-ports" => {
            for p in serial::list_ports() {
                println!("{}\t{}", p.name, p.description);
            }
        }
        "usb-print" => {
            // usb-print --port PORT [--baud N] FILE.gcode
            let a = args.get(2..).unwrap_or(&[]);
            let port = arg_value(a, "--port").unwrap_or_else(|| fail("--port is required"));
            let baud = arg_value(a, "--baud").and_then(|b| b.parse().ok());
            let file = positional(a, &["--port", "--baud"]).into_iter().last().unwrap_or_else(|| fail("G-code file missing"));
            let gcode = std::fs::read_to_string(&file).unwrap_or_else(|e| fail(&format!("{}: {}", file, e)));
            let p = usbprint::UsbPrinter::connect(&port, baud, 1);
            let t0 = std::time::Instant::now();
            loop {
                let st = p.state.lock().unwrap().clone();
                if st.status == "connected" {
                    break;
                }
                if st.status == "error" || t0.elapsed().as_secs() > 180 {
                    fail(&format!("could not connect: {}", st.message));
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            eprintln!("connected at {} baud", p.state.lock().unwrap().baud);
            p.print(gcode, &file, 0).unwrap_or_else(|e| fail(&e));
            let mut last = 0;
            loop {
                std::thread::sleep(std::time::Duration::from_millis(200));
                let st = p.state.lock().unwrap().clone();
                if st.line / 1000 != last {
                    last = st.line / 1000;
                    eprintln!("line {}/{} {:.0}°C", st.line, st.lines, st.hotends.first().map(|t| t.current).unwrap_or(0.0));
                }
                if !st.printing {
                    eprintln!("done: {} lines, firmware {:?}", st.lines, st.firmware);
                    break;
                }
                if st.status == "error" {
                    fail(&st.message);
                }
            }
            p.close();
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        "net-print" | "net-status" | "net-test" => {
            // net-print --kind octoprint|moonraker|prusalink|duet --url URL [--key KEY] [--user U] [--no-start] FILE.gcode
            let a = args.get(2..).unwrap_or(&[]);
            let np = netprint::NetPrinter {
                kind: netprint::Kind::parse(&arg_value(a, "--kind").unwrap_or_default()).unwrap_or_else(|| fail("--kind octoprint|moonraker|prusalink|duet")),
                url: arg_value(a, "--url").unwrap_or_else(|| fail("--url is required")),
                key: arg_value(a, "--key").unwrap_or_default(),
                user: arg_value(a, "--user").unwrap_or_default(),
            };
            match args[1].as_str() {
                "net-test" => println!("{}", np.test().unwrap_or_else(|e| fail(&e))),
                "net-status" => {
                    let st = np.status().unwrap_or_else(|e| fail(&e));
                    println!("{:?}", st);
                }
                _ => {
                    let file = positional(a, &["--kind", "--url", "--key", "--user"]).into_iter().last().unwrap_or_else(|| fail("G-code file missing"));
                    let data = std::fs::read(&file).unwrap_or_else(|e| fail(&format!("{}: {}", file, e)));
                    let name = std::path::Path::new(&file).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(file.clone());
                    println!("{}", np.upload(&name, &data, !a.iter().any(|x| x == "--no-start")).unwrap_or_else(|e| fail(&e)));
                }
            }
        }
        "market-search" => {
            // market-search [--type plugin|material] [TEXT]
            let a = args.get(2..).unwrap_or(&[]);
            let kind = arg_value(a, "--type").unwrap_or_default();
            let text = positional(a, &["--type"]).join(" ");
            let page = marketplace::search(&marketplace::search_url(&kind, &text)).unwrap_or_else(|e| fail(&e));
            if let Some(curust_settings::json::Json::Array(items)) = page.get("data") {
                for p in items {
                    let g = |k: &str| p.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let author = p.get("author").and_then(|a| a.get("display_name")).and_then(|v| v.as_str()).unwrap_or("");
                    println!("{:<32} {:<9} {:<10} {} — {}", g("package_id"), g("package_type"), g("package_version"), g("display_name"), author);
                }
            }
        }
        "install-package" => {
            // install-package FILE.curapackage | PACKAGE_ID
            let target = args.get(2).cloned().unwrap_or_else(|| fail("install-package FILE.curapackage|PACKAGE_ID"));
            let report = if std::path::Path::new(&target).is_file() {
                let data = std::fs::read(&target).unwrap_or_else(|e| fail(&format!("{}: {}", target, e)));
                marketplace::install_package(&data)
            } else {
                marketplace::download_and_install(&target, "")
            }
            .unwrap_or_else(|e| fail(&e));
            println!("{}", report.summary());
        }
        "uninstall-package" => {
            let id = args.get(2).cloned().unwrap_or_else(|| fail("uninstall-package PACKAGE_ID"));
            marketplace::uninstall(&id).unwrap_or_else(|e| fail(&e));
            println!("Removed {}", id);
        }
        "packages" => {
            for (id, info) in marketplace::installed() {
                println!("{:<32} {}", id, info.get("curust_summary").and_then(|v| v.as_str()).unwrap_or(""));
            }
            for (id, info) in marketplace::cura_installed() {
                println!("{:<32} (installed in UltiMaker Cura) {}", id, info.get("display_name").and_then(|v| v.as_str()).unwrap_or(""));
            }
        }
        "scripts" => {
            // Post-processing scripts Curust can run (bundled, user folders, plugins, Cura's).
            for f in postproc::script_files() {
                println!("{:<32} {}", f.name, f.origin);
            }
        }
        "slice-models" => {
            if let Err(e) = slice_models(args.get(2..).unwrap_or(&[])) {
                fail(&e);
            }
        }
        "gui" => server::run_gui(args.get(2..).unwrap_or(&[])),
        other if other.starts_with("--") => server::run_gui(&args[1..]),
        // "Open with Curust" / drag onto the executable: model files.
        other if std::path::Path::new(other).is_file() => server::run_gui(&args[1..]),
        other => {
            eprintln!("Unknown command: {}", other);
            usage();
            std::process::exit(1);
        }
    }
}

fn fail(msg: &str) -> ! {
    fatal(msg)
}

/// Arguments that are neither options nor the value of one of `with_value`.
fn positional(args: &[String], with_value: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if with_value.contains(&args[i].as_str()) {
            i += 2;
            continue;
        }
        if !args[i].starts_with("--") {
            out.push(args[i].clone());
        }
        i += 1;
    }
    out
}

fn arg_value(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

/// Parse `key=value,...` image import options.
fn parse_image_options(spec: &str) -> Result<image_import::ImageOptions, String> {
    let mut o = image_import::ImageOptions::default();
    for kv in spec.split(',').filter(|s| !s.trim().is_empty()) {
        let (k, v) = kv.split_once('=').ok_or_else(|| format!("expected key=value in {}", kv))?;
        let f = || v.trim().parse::<f64>().map_err(|_| format!("bad number {}", v));
        let b = || matches!(v.trim(), "1" | "true" | "True" | "yes");
        match k.trim() {
            "size" => o.size = f()?,
            "peak" | "height" => o.peak_height = f()?,
            "base" => o.base_height = f()?,
            "smoothing" => o.smoothing = f()? as usize,
            "lighter_is_higher" => o.lighter_is_higher = b(),
            "transparency" | "translucency" => o.use_transparency_model = b(),
            "transmittance" => o.transmittance_1mm = f()?,
            other => return Err(format!("unknown image option {}", other)),
        }
    }
    Ok(o)
}

fn image_debug(args: &[String]) -> Result<(), String> {
    let (file, out) = (args.first().ok_or("image-debug FILE OUTDIR [OPTIONS]...")?, args.get(1).ok_or("OUTDIR missing")?);
    let data = std::fs::read(file).map_err(|e| format!("{}: {}", file, e))?;
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let img = qimage::load(&data);
    let mut info = format!("{{\"null\": {}", img.is_none());
    if let Some(img) = &img {
        let (w, h) = (img.width, img.height);
        info.push_str(&format!(", \"format\": {}, \"width\": {}, \"height\": {}, \"alpha\": {}", img.format.qt_value(), w, h, img.has_alpha_channel()));
        let dump = |im: &qimage::QImage, path: std::path::PathBuf| {
            let mut b = Vec::with_capacity(im.width * im.height * 4);
            for y in 0..im.height {
                for x in 0..im.width {
                    b.extend_from_slice(&im.pixel(x, y).to_le_bytes());
                }
            }
            let _ = std::fs::write(path, b);
        };
        dump(img, std::path::Path::new(out).join("pixels.u32"));
        let big = w.max(h) as f64;
        let py_round = |x: f64| if (x - x.trunc()).abs() == 0.5 { 2.0 * (x / 2.0).round() } else { x.round() };
        for (tw, th) in [((py_round(w as f64 * 512.0 / big) as usize).max(2), (py_round(h as f64 * 512.0 / big) as usize).max(2)), ((w / 3).max(2), (h / 2).max(2))] {
            dump(&img.scaled(tw, th), std::path::Path::new(out).join(format!("scaled_{}x{}.u32", tw, th)));
        }
        for (i, spec) in args[2..].iter().enumerate() {
            let o = parse_image_options(spec)?;
            let mesh = image_import::generate_mesh(img, &o);
            let mut b = Vec::with_capacity(mesh.len() * 12);
            for v in &mesh {
                for c in v {
                    b.extend_from_slice(&c.to_le_bytes());
                }
            }
            std::fs::write(std::path::Path::new(out).join(format!("mesh{}.f32", i)), b).map_err(|e| e.to_string())?;
        }
    }
    info.push('}');
    std::fs::write(std::path::Path::new(out).join("info.json"), info).map_err(|e| e.to_string())?;
    Ok(())
}

fn slice_models(args: &[String]) -> Result<(), String> {
    let resources = resources::find_resources(arg_value(args, "--resources").as_deref())?;
    let mut s = Session::new(resources);
    if let Some(p) = arg_value(args, "--printer") {
        persist::load(&mut s);
        let idx = s.machines.iter().position(|m| m.global.name == p).ok_or_else(|| format!("No saved printer named {}", p))?;
        s.set_active_machine(idx);
    } else {
        let machine = arg_value(args, "--machine").ok_or("--machine is required")?;
        s.add_machine(&machine, arg_value(args, "--machine-name").as_deref())?;
    }
    let mut output = None;
    let mut fixed_time = false;
    let mut pending_at: Option<(f64, f64)> = None;
    let mut pending_extruder: Option<usize> = None;
    let mut pending_settings: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    let split_eq = |v: &str| -> Result<(String, String), String> {
        let p = v.find('=').ok_or_else(|| format!("expected KEY=VALUE, got {}", v))?;
        Ok((v[..p].to_string(), v[p + 1..].to_string()))
    };
    // Profile choices first (they reset nothing but must precede user settings like in Cura).
    let mut user_global = Vec::new();
    let mut user_extruder = Vec::new();
    let mut raw_global: Vec<(String, String)> = Vec::new();
    let mut raw_extruder: Vec<(usize, String, String)> = Vec::new();
    #[allow(clippy::type_complexity)]
    let mut models: Vec<(String, Option<(f64, f64)>, Option<usize>, Vec<(String, String)>, Vec<[f64; 3]>)> = Vec::new();
    let mut create_profile: Option<String> = None;
    let mut export_profile: Option<String> = None;
    let mut import_profiles: Vec<String> = Vec::new();
    let mut dump_profiles: Option<String> = None;
    let mut dumped_scripts = false;
    let mut pending_image: Option<image_import::ImageOptions> = None;
    let mut image_opts: Vec<(usize, image_import::ImageOptions)> = Vec::new();
    while i < args.len() {
        let a = args[i].as_str();
        let next = || args.get(i + 1).cloned().ok_or_else(|| format!("{} needs a value", a));
        match a {
            "--resources" | "--machine" | "--machine-name" | "--printer" => i += 1,
            "-o" | "--output" => {
                output = Some(next()?);
                i += 1;
            }
            "--fixed-time" => fixed_time = true,
            "--import-material" => {
                let path = next()?;
                let data = std::fs::read(&path).map_err(|e| format!("{}: {}", path, e))?;
                let id = s.import_material(&path, &data)?;
                eprintln!("imported material {}", id);
                i += 1;
            }
            "--variant" => {
                let (p, v) = split_eq(&next()?)?;
                if !s.set_variant(p.parse().map_err(|_| "bad position")?, &v) {
                    eprintln!("Warning: unknown nozzle {} (ignored, like Cura)", v);
                }
                i += 1;
            }
            "--material" => {
                let (p, v) = split_eq(&next()?)?;
                if !s.set_material(p.parse().map_err(|_| "bad position")?, &v) {
                    eprintln!("Warning: material {} is not available for this printer/nozzle (ignored, like Cura)", v);
                }
                i += 1;
            }
            "--quality" => {
                let q = next()?;
                if !s.set_quality(&q) {
                    eprintln!("Warning: quality {} is not available (ignored, like Cura)", q);
                }
                i += 1;
            }
            "--intent" => {
                s.set_intent(&next()?);
                i += 1;
            }
            "--disable-extruder" => {
                s.set_extruder_enabled(next()?.parse().map_err(|_| "bad position")?, false);
                i += 1;
            }
            "--set" => {
                user_global.push(split_eq(&next()?)?);
                i += 1;
            }
            "--extruder-set" => {
                let v = next()?;
                let c = v.find(':').ok_or("expected POS:KEY=VALUE")?;
                let pos: usize = v[..c].parse().map_err(|_| "bad position")?;
                let (k, val) = split_eq(&v[c + 1..])?;
                user_extruder.push((pos, k, val));
                i += 1;
            }
            "--at" => {
                let v = next()?;
                let mut it = v.split(',').map(|t| t.trim().parse::<f64>());
                match (it.next(), it.next()) {
                    (Some(Ok(x)), Some(Ok(y))) => pending_at = Some((x, y)),
                    _ => return Err("--at expects X,Y".into()),
                }
                i += 1;
            }
            "--extruder" => {
                pending_extruder = Some(next()?.parse().map_err(|_| "bad extruder")?);
                i += 1;
            }
            "--object-set" => {
                pending_settings.push(split_eq(&next()?)?);
                i += 1;
            }
            "--create-profile" => {
                create_profile = Some(next()?);
                i += 1;
            }
            "--export-profile" => {
                export_profile = Some(next()?);
                i += 1;
            }
            "--import-profile" => {
                import_profiles.push(next()?);
                i += 1;
            }
            "--dump-profiles" => {
                dump_profiles = Some(next()?);
                i += 1;
            }
            "--dump-scripts" => {
                // The post-processing dialog's data (loaded scripts, definitions, states).
                let path = next()?;
                let list = postproc::working_list(&s);
                std::fs::write(&path, postproc::dialog_json(&s, &list).to_string_pretty()).map_err(|e| format!("{}: {}", path, e))?;
                dumped_scripts = true;
                i += 1;
            }
            "--raw-set" => {
                raw_global.push(split_eq(&next()?)?);
                i += 1;
            }
            "--raw-extruder-set" => {
                let v = next()?;
                let c = v.find(':').ok_or("expected POS:KEY=JSON")?;
                let pos: usize = v[..c].parse().map_err(|_| "bad position")?;
                let (k, val) = split_eq(&v[c + 1..])?;
                raw_extruder.push((pos, k, val));
                i += 1;
            }
            "--image-options" => {
                pending_image = Some(parse_image_options(&next()?)?);
                i += 1;
            }
            "--post-process" => {
                // A file with the `post_processing_scripts` metadata (Cura's format).
                let path = next()?;
                let meta = std::fs::read_to_string(&path).map_err(|e| format!("{}: {}", path, e))?;
                postproc::set_stored_scripts(&mut s, meta.trim_end_matches(['\n', '\r']));
                i += 1;
            }
            "--add-script" => {
                // KEY[;setting=value;...]: add a post-processing script like the dialog does.
                let v = next()?;
                let mut parts = v.split(';');
                let key = parts.next().unwrap_or_default().to_string();
                let mut list = postproc::working_list(&s);
                postproc::apply_op(&mut list, postproc::ScriptOp::Add(key));
                let n = list.len();
                for kv in parts.filter(|p| !p.is_empty()) {
                    let (k, val) = split_eq(kv)?;
                    postproc::apply_op(&mut list, postproc::ScriptOp::Set(n - 1, k, val));
                }
                postproc::commit_scripts(&mut s, &list)?;
                if postproc::working_list(&s).len() < n {
                    return Err(format!("Unknown post-processing script {} (see `curust scripts`)", list[n - 1].key));
                }
                i += 1;
            }
            "--add-script-json" => {
                // ["KEY", {"setting": "value", ...}]
                let v = next()?;
                let j = curust_settings::json::parse(&v).map_err(|e| format!("--add-script-json: {}", e))?;
                let key = j.as_array().and_then(|a| a.first()).and_then(|k| k.as_str()).ok_or("--add-script-json expects [KEY, {settings}]")?.to_string();
                let mut list = postproc::working_list(&s);
                postproc::apply_op(&mut list, postproc::ScriptOp::Add(key.clone()));
                let n = list.len();
                if let Some(curust_settings::json::Json::Object(items)) = j.as_array().and_then(|a| a.get(1)) {
                    for (k, val) in items {
                        let val = match val {
                            curust_settings::json::Json::Str(s) => s.clone(),
                            other => other.to_string_compact(),
                        };
                        postproc::apply_op(&mut list, postproc::ScriptOp::Set(n - 1, k.clone(), val));
                    }
                }
                postproc::commit_scripts(&mut s, &list)?;
                if postproc::working_list(&s).len() < n {
                    return Err(format!("Unknown post-processing script {} (see `curust scripts`)", key));
                }
                i += 1;
            }
            "--support-blocker" => {
                // X,Y,Z in scene coordinates (Y up), for the previous model.
                let v: Vec<f64> = next()?.split(',').map(|x| x.trim().parse::<f64>()).collect::<Result<_, _>>().map_err(|_| "expected X,Y,Z")?;
                if v.len() != 3 {
                    return Err("expected X,Y,Z".into());
                }
                let last = models.last_mut().ok_or("--support-blocker must follow a model")?;
                last.4.push([v[0], v[1], v[2]]);
                i += 1;
            }
            _ if a.starts_with("--") => return Err(format!("Unknown option {}", a)),
            _ => {
                if let Some(o) = pending_image.take() {
                    image_opts.push((models.len(), o));
                }
                models.push((a.to_string(), pending_at.take(), pending_extruder.take(), std::mem::take(&mut pending_settings), Vec::<[f64; 3]>::new()));
            }
        }
        i += 1;
    }
    for (k, v) in user_global {
        // The settings panel stores per-extruder settings in the active extruder.
        let target = match s.stack_for_setting(&k) {
            curust_settings::stack::StackId::Extruder(i) => {
                let m = s.require_machine()?;
                Target::Extruder(m.extruders[i].position().and_then(|p| p.parse().ok()).unwrap_or(0))
            }
            _ => Target::Global,
        };
        s.set_setting(target, &k, &v)?;
    }
    for (p, k, v) in user_extruder {
        s.set_setting(Target::Extruder(p), &k, &v)?;
    }
    // Raw values (JSON) written straight into the user containers, like a script would.
    for (k, v) in raw_global {
        s.set_raw(Target::Global, &k, &v)?;
    }
    for (p, k, v) in raw_extruder {
        s.set_raw(Target::Extruder(p), &k, &v)?;
    }
    if let Some(name) = create_profile {
        // "Create profile from current settings" (the user changes move into the profile).
        s.create_profile(&name)?;
    }
    if let Some(path) = &export_profile {
        let idx = s.active_profile().ok_or("--export-profile: no custom profile is active")?;
        let (_, data) = s.export_profile(idx)?;
        std::fs::write(path, data).map_err(|e| format!("{}: {}", path, e))?;
    }
    for path in &import_profiles {
        let data = std::fs::read(path).map_err(|e| format!("{}: {}", path, e))?;
        let r = s.import_profile(path, &data);
        eprintln!("import {}: {} {}", path, r.status, r.message);
        if let Some(idx) = r.profile {
            s.activate_profile(idx)?;
        }
    }
    if let Some(path) = &dump_profiles {
        let mut items = Vec::new();
        for p in &s.profiles {
            let containers: Vec<(String, curust_settings::json::Json)> =
                p.containers().map(|c| (c.id.clone(), curust_settings::json::Json::Str(profiles::cura_serialize(&s.reg, c)))).collect();
            items.push((p.name.clone(), curust_settings::json::Json::Object(containers)));
        }
        std::fs::write(path, curust_settings::json::Json::Object(items).to_string_pretty()).map_err(|e| format!("{}: {}", path, e))?;
    }
    if models.is_empty() {
        if export_profile.is_some() || !import_profiles.is_empty() || dump_profiles.is_some() || dumped_scripts {
            return Ok(());
        }
        return Err("No models given".into());
    }
    for (mi, (path, at, extruder, settings, blockers)) in models.into_iter().enumerate() {
        let data = std::fs::read(&path).map_err(|e| format!("{}: {}", path, e))?;
        let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(path.clone());
        let ids = if image_import::is_image_file(&name) {
            let o = image_opts.iter().find(|(k, _)| *k == mi).map(|(_, o)| o.clone()).unwrap_or_default();
            s.add_image_data(&name, &data, &o)?
        } else {
            s.add_model_data(&name, &data)?
        };
        for id in ids {
            if let Some(n) = s.node_mut(id) {
                if let Some((x, y)) = at {
                    n.position[0] = x;
                    n.position[2] = -y;
                }
                if let Some(e) = extruder {
                    n.extruder = e;
                }
                for (k, v) in &settings {
                    n.settings.push((k.clone(), v.clone()));
                }
            }
            s.refresh_scene();
            for b in &blockers {
                s.add_support_blocker(id, *b)?;
            }
        }
        s.refresh_scene();
    }
    if let Ok(p) = std::env::var("CURUST_DUMP_PP_META") {
        let _ = std::fs::write(p, postproc::stored_scripts(&s));
    }
    let time = if fixed_time { TimeTokens::fixed() } else { TimeTokens::now() };
    let (gcode, info, _) = slicing::slice_blocking(&s, &time)?;
    let out = output.unwrap_or_else(|| format!("{}.gcode", s.job_name()));
    // Cura saves G-code in text mode ("\r\n" line endings on Windows).
    std::fs::write(&out, curust_engine::utils::text_mode::platform_text(&gcode).as_bytes()).map_err(|e| format!("{}: {}", out, e))?;
    eprintln!("Print time {}  material {:?} m  {:?} g  ({} layers) -> {}", info.print_time_iso, info.material_lengths, info.material_weights, info.layer_count, out);
    Ok(())
}
