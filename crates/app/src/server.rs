//! The graphical interface: a small HTTP server on 127.0.0.1 serving the web application (see
//! `web/`) and its JSON API, shown in a Microsoft Edge / Google Chrome "app" window.

use crate::persist;
use crate::session::{value_to_json, Session, Target};
use crate::slicing::{self, EngineRun, PrintInfo};
use crate::web;
use curust_engine::communication::job::SliceResult;
use curust_settings::definition::Prop;
use curust_settings::json::{self, Json};
use curust_settings::machine;
use curust_settings::py::Value;
use curust_settings::slice_message::TimeTokens;
use curust_settings::stack::{index, parse_bool, Container, StackId};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------------------------
// HTTP plumbing
// ---------------------------------------------------------------------------------------------

struct Request {
    method: String,
    path: String,
    query: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn q(&self, k: &str) -> Option<&str> {
        self.query.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str())
    }
    fn json(&self) -> Json {
        json::parse(&String::from_utf8_lossy(&self.body)).unwrap_or(Json::Object(Vec::new()))
    }
}

pub struct Response {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
    extra_headers: Vec<(String, String)>,
}

impl Response {
    fn json(j: Json) -> Response {
        Response { status: 200, content_type: "application/json; charset=utf-8", body: j.to_string_compact().into_bytes(), extra_headers: Vec::new() }
    }
    fn ok() -> Response {
        Response::json(Json::Object(vec![("ok".into(), Json::Bool(true))]))
    }
    fn error(status: u16, msg: &str) -> Response {
        Response {
            status,
            content_type: "application/json; charset=utf-8",
            body: Json::Object(vec![("error".into(), Json::Str(msg.to_string()))]).to_string_compact().into_bytes(),
            extra_headers: Vec::new(),
        }
    }
    fn bytes(content_type: &'static str, body: Vec<u8>) -> Response {
        Response { status: 200, content_type, body, extra_headers: Vec::new() }
    }
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) {
                    out.push(v);
                    i += 2;
                } else {
                    out.push(b'%');
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    stream.set_read_timeout(Some(Duration::from_secs(30))).ok()?;
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let mut content_length = 0usize;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).ok()? == 0 {
            break;
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                content_length = v.trim().parse().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body).ok()?;
    }
    let (path, qs) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target.clone(), String::new()),
    };
    let query = qs
        .split('&')
        .filter(|s| !s.is_empty())
        .map(|kv| match kv.split_once('=') {
            Some((k, v)) => (url_decode(k), url_decode(v)),
            None => (url_decode(kv), String::new()),
        })
        .collect();
    Some(Request { method, path: url_decode(&path), query, body })
}

fn write_response(stream: &mut TcpStream, r: Response) {
    let reason = match r.status {
        200 => "OK",
        404 => "Not Found",
        400 => "Bad Request",
        _ => "Error",
    };
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n",
        r.status,
        reason,
        r.content_type,
        r.body.len()
    );
    for (k, v) in &r.extra_headers {
        head.push_str(&format!("{}: {}\r\n", k, v));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&r.body);
    let _ = stream.flush();
}

// ---------------------------------------------------------------------------------------------
// Application state
// ---------------------------------------------------------------------------------------------

enum SliceState {
    Idle,
    Running(EngineRun),
    Done,
    Unable(String),
    Error(String),
}

struct App {
    s: Option<Session>,
    resource_error: Option<String>,
    slice: SliceState,
    result: Option<(String, PrintInfo, SliceResult)>,
    layers_cache: Option<Vec<u8>>,
    generation: u64,
    auto_slice: bool,
    last_request: Instant,
    toasts: Vec<String>,
    /// The post-processing dialog's working list while it is open.
    pp_working: Option<Vec<crate::postproc::ScriptConfig>>,
    /// The G-code as saved (after post-processing) for the current result.
    exported: Option<String>,
    /// USB printing connection.
    usb: Option<crate::usbprint::UsbPrinter>,
    /// Network printer status and upload state (updated by background threads).
    net: std::sync::Arc<std::sync::Mutex<NetShared>>,
    /// Model files given on the command line ("Open with Curust"), loaded once a printer exists.
    pending_files: Vec<std::path::PathBuf>,
}

#[derive(Default)]
struct NetShared {
    status: Option<Result<crate::netprint::NetStatus, String>>,
    updated: Option<Instant>,
    fetching: bool,
    /// (busy, message, is_error)
    job: Option<(bool, String, bool)>,
}

/// The active printer's network connection (`curust_network_printer` metadata).
fn net_printer(s: &Session) -> Option<crate::netprint::NetPrinter> {
    let m = s.machine()?;
    let text = m.global.meta("curust_network_printer")?.py_str();
    crate::netprint::NetPrinter::from_json(&curust_settings::json::parse(&text).ok()?)
}

fn temp_json(t: (f64, f64)) -> Json {
    Json::Object(vec![("current".into(), jf(t.0)), ("target".into(), jf(t.1))])
}

fn num(v: Option<Value>) -> f64 {
    v.and_then(|v| v.as_f64()).unwrap_or(0.0)
}

fn jstr(s: &str) -> Json {
    Json::Str(s.to_string())
}

fn jf(f: f64) -> Json {
    if f.is_finite() {
        Json::Float(f)
    } else {
        Json::Null
    }
}

fn jobj(items: Vec<(&str, Json)>) -> Json {
    Json::Object(items.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

impl App {
    fn session(&self) -> Result<&Session, String> {
        self.s.as_ref().ok_or_else(|| self.resource_error.clone().unwrap_or_else(|| "No resources".into()))
    }
    fn session_mut(&mut self) -> Result<&mut Session, String> {
        let err = self.resource_error.clone();
        self.s.as_mut().ok_or_else(|| err.unwrap_or_else(|| "No resources".into()))
    }

    /// Something that affects the slice changed.
    /// Load the model files from the command line once a printer is set up.
    fn load_pending_files(&mut self) {
        if self.pending_files.is_empty() || !self.s.as_ref().is_some_and(|s| s.machine().is_some()) {
            return;
        }
        let files = std::mem::take(&mut self.pending_files);
        let s = self.s.as_mut().unwrap();
        let mut errors = Vec::new();
        for f in files {
            let name = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let result = std::fs::read(&f).map_err(|e| e.to_string()).and_then(|data| {
                if crate::image_import::is_image_file(&name) {
                    s.add_image_data(&name, &data, &crate::image_import::ImageOptions::default())
                } else {
                    s.add_model_data(&name, &data)
                }
            });
            if let Err(e) = result {
                errors.push(format!("{}: {}", name, e));
            }
        }
        self.toasts.extend(errors);
        self.invalidate();
    }

    fn invalidate(&mut self) {
        self.generation += 1;
        if let SliceState::Running(_) = self.slice {
            if let SliceState::Running(run) = std::mem::replace(&mut self.slice, SliceState::Idle) {
                run.cancel();
            }
        }
        self.slice = SliceState::Idle;
        self.result = None;
        self.exported = None;
        self.layers_cache = None;
        if let Some(s) = &self.s {
            persist::save(s);
        }
    }

    fn poll_slice(&mut self) {
        let done = match &mut self.slice {
            SliceState::Running(run) => run.poll(),
            _ => None,
        };
        if let Some(r) = done {
            match r {
                Ok(res) => {
                    let s = self.s.as_ref().unwrap();
                    let (gcode, info) = slicing::final_gcode(s, &res);
                    self.result = Some((gcode, info, res));
                    self.exported = None;
                    self.layers_cache = None;
                    self.slice = SliceState::Done;
                }
                Err(e) => self.slice = SliceState::Error(e),
            }
        }
    }

    /// The G-code as it is saved: post-processing scripts run once per slice result, like
    /// Cura's `writeStarted` (the result is marked ";POSTPROCESSED").
    fn exported_gcode(&mut self) -> Result<String, String> {
        if self.exported.is_none() {
            let s = self.s.as_ref().ok_or("No printer")?;
            let (_, _, res) = self.result.as_ref().ok_or("Not sliced")?;
            let (g, _info, pp) = slicing::export_gcode(s, res);
            for (t, m) in &pp.messages {
                self.toasts.push(if t.is_empty() { m.clone() } else { format!("{}: {}", t, m) });
            }
            for l in &pp.log {
                if l.starts_with("[e]") {
                    self.toasts.push(format!("Post-processing: {}", l.trim_start_matches("[e] ")));
                }
            }
            self.exported = Some(g);
        }
        Ok(self.exported.clone().unwrap())
    }

    fn pp_list(&mut self) -> Result<Vec<crate::postproc::ScriptConfig>, String> {
        if self.pp_working.is_none() {
            let s = self.session()?;
            self.pp_working = Some(crate::postproc::working_list(s));
        }
        Ok(self.pp_working.clone().unwrap())
    }

    fn start_slice(&mut self) -> Response {
        let s = match self.session() {
            Ok(s) => s,
            Err(e) => return Response::error(400, &e),
        };
        if let SliceState::Running(_) = self.slice {
            return Response::ok();
        }
        match slicing::prepare(s, &TimeTokens::now()) {
            Ok(job) => match EngineRun::start(&job) {
                Ok(run) => {
                    self.slice = SliceState::Running(run);
                    self.result = None;
                    self.exported = None;
                    self.layers_cache = None;
                }
                Err(e) => self.slice = SliceState::Error(e),
            },
            Err(p) => self.slice = SliceState::Unable(p.message),
        }
        Response::ok()
    }

    fn slice_json(&self) -> Json {
        match &self.slice {
            SliceState::Idle => jobj(vec![("state", jstr("idle"))]),
            SliceState::Running(run) => jobj(vec![("state", jstr("slicing")), ("progress", jf(*run.progress.lock().unwrap())), ("elapsed", jf(run.started.elapsed().as_secs_f64()))]),
            SliceState::Unable(m) => jobj(vec![("state", jstr("unable")), ("message", jstr(m))]),
            SliceState::Error(m) => jobj(vec![("state", jstr("error")), ("message", jstr(m))]),
            SliceState::Done => {
                let (_, info, res) = self.result.as_ref().unwrap();
                let s = self.s.as_ref().unwrap();
                let currency = s.prefs.currency.clone();
                jobj(vec![
                    ("state", jstr("done")),
                    ("print_time", Json::Int(info.print_time)),
                    ("print_time_iso", jstr(&info.print_time_iso)),
                    ("feature_times", Json::Array(info.feature_times.iter().map(|(n, t)| Json::Array(vec![jstr(n), jf(*t)])).collect())),
                    ("material_lengths", Json::Array(info.material_lengths.iter().map(|v| jf(*v)).collect())),
                    ("material_weights", Json::Array(info.material_weights.iter().map(|v| jf(*v)).collect())),
                    ("material_costs", Json::Array(info.material_costs.iter().map(|v| jf(*v)).collect())),
                    ("material_names", Json::Array(info.material_names.iter().map(|v| jstr(v)).collect())),
                    ("currency", jstr(&currency)),
                    ("layer_count", Json::Int(info.layer_count as i64)),
                    ("gcode_size", Json::Int(self.result.as_ref().map(|r| r.0.len() as i64).unwrap_or(0))),
                    ("warnings", Json::Array(res.log.iter().map(|l| jstr(l)).collect())),
                ])
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// State serialisation
// ---------------------------------------------------------------------------------------------

fn color_of(s: &Session, c: &Container) -> String {
    c.meta_str("color_code").or_else(|| s.reg.metadata(c.id()).and_then(|e| e.get_str("color_code"))).unwrap_or_else(|| "#ffc924".into())
}

fn quality_layer_height(s: &Session, quality_id: &str) -> Option<f64> {
    let m = s.machine()?;
    let c = s.reg.instance(quality_id)?;
    let def = m.global.definition();
    match c.get_value("layer_height", Some(def))? {
        Prop::Value(v) => v.as_f64(),
        Prop::Func(f) => m.call_function(&f, StackId::Global, None, None).and_then(|v| v.as_f64()),
    }
}

fn machine_json(s: &Session, active_extruder: usize) -> Json {
    let Some(m) = s.machine() else { return Json::Null };
    let g = |k: &str| m.get_value(StackId::Global, k);
    let def = m.global.definition();
    let node = s.tree.machine(&s.reg, &def.id);
    let list = m.extruder_list();
    let mut exts = Vec::new();
    for (pos, &i) in list.iter().enumerate() {
        let e = &m.extruders[i];
        let mat = e.container(index::MATERIAL);
        let mat_meta = s.reg.metadata(mat.id());
        let gm = |k: &str| mat.meta_str(k).or_else(|| mat_meta.and_then(|e| e.get_str(k))).unwrap_or_default();
        exts.push(jobj(vec![
            ("position", Json::Int(pos as i64)),
            ("name", jstr(&e.definition().name)),
            ("enabled", Json::Bool(e.is_enabled())),
            ("variant", jstr(&e.container(index::VARIANT).name())),
            ("material", jobj(vec![("base", jstr(&gm("base_file"))), ("name", jstr(&mat.name())), ("brand", jstr(&gm("brand"))), ("type", jstr(&gm("material"))), ("color", jstr(&color_of(s, mat)))])),
            ("nozzle_size", jf(num(m.get_value(StackId::Extruder(i), "machine_nozzle_size")))),
            ("diameter", jf(num(m.get_value(StackId::Extruder(i), "material_diameter")))),
            ("intent", jstr(&e.container(index::INTENT).meta_str("intent_category").unwrap_or_else(|| "default".into()))),
        ]));
    }
    let groups = machine::current_quality_groups(m, &s.tree);
    let qualities: Vec<Json> = groups
        .items
        .iter()
        .filter(|(qt, _)| qt != "not_supported")
        .map(|(qt, g)| {
            jobj(vec![
                ("type", jstr(qt)),
                ("name", jstr(&g.name)),
                ("available", Json::Bool(g.is_available)),
                ("layer_height", quality_layer_height(s, &g.node_for_global).map(jf).unwrap_or(Json::Null)),
            ])
        })
        .collect();
    let qualities_available: Vec<String> = groups.items.iter().filter(|(_, g)| g.is_available).map(|(qt, _)| qt.clone()).collect();
    let quality = m.global.container(index::QUALITY);
    // Intents available for the current quality (`IntentCategoryModel`).
    let mut intents: Vec<String> = vec!["default".into()];
    let qt = quality.meta_str("quality_type").unwrap_or_default();
    for &i in &list {
        let e = &m.extruders[i];
        if !e.is_enabled() {
            continue;
        }
        let vn = e.container(index::VARIANT).name();
        let base = e.container(index::MATERIAL).meta_str("base_file").unwrap_or_default();
        if let Some(mn) = node.variants.get(&vn).and_then(|v| v.materials.get(&base)) {
            for q in mn.qualities.values() {
                if q.quality_type == qt {
                    for it in q.intents.values() {
                        if !intents.contains(&it.intent_category) {
                            intents.push(it.intent_category.clone());
                        }
                    }
                }
            }
        }
    }
    let has_user = !m.global.user().keys().is_empty() || m.extruders.iter().any(|e| !e.user().keys().is_empty());
    let areas: Vec<Json> = crate::buildvolume::disallowed_areas(s).iter().map(|a| Json::Array(a.iter().map(|p| Json::Array(vec![jf(p[0]), jf(p[1])])).collect())).collect();
    let active_stack = list.get(active_extruder).map(|&i| StackId::Extruder(i)).unwrap_or(StackId::Global);
    let adhesion = g("adhesion_type").map(|v| v.py_str()).unwrap_or_default();
    jobj(vec![
        ("name", jstr(&m.global.name)),
        ("definition", jstr(&def.id)),
        ("definition_name", jstr(&def.name)),
        ("manufacturer", jstr(&def.metadata_str("manufacturer").unwrap_or_default())),
        ("width", jf(num(g("machine_width")))),
        ("depth", jf(num(g("machine_depth")))),
        ("height", jf(num(g("machine_height")))),
        ("shape", jstr(&g("machine_shape").map(|v| v.py_str()).unwrap_or_else(|| "rectangular".into()))),
        ("center_is_zero", Json::Bool(g("machine_center_is_zero").map(|v| v.truthy()).unwrap_or(false))),
        ("heated_bed", Json::Bool(g("machine_heated_bed").map(|v| v.truthy()).unwrap_or(false))),
        ("has_variants", Json::Bool(node.has_variants)),
        ("has_materials", Json::Bool(node.has_materials)),
        ("variants_name", jstr(&def.metadata_str("variants_name").unwrap_or_else(|| "Nozzle".into()))),
        ("variants", Json::Array(node.variants.items.iter().filter(|(k, _)| k != "empty").map(|(k, _)| jstr(k)).collect())),
        ("disallowed_areas", Json::Array(areas)),
        ("border", jf(crate::buildvolume::edge_disallowed_size(s))),
        ("extruders", Json::Array(exts)),
        ("active_extruder", Json::Int(active_extruder as i64)),
        (
            "quality",
            jobj(vec![
                ("type", jstr(&qt)),
                ("name", jstr(&quality.name())),
                ("supported", Json::Bool(qt != "not_supported" && !qt.is_empty())),
                ("layer_height", jf(num(g("layer_height")))),
                ("custom", Json::Bool(has_user)),
                ("profile", s.active_profile().map(|i| Json::Int(i as i64)).unwrap_or(Json::Null)),
                ("profile_name", s.active_profile().map(|i| jstr(&s.profiles[i].name)).unwrap_or(Json::Null)),
            ]),
        ),
        (
            "custom_profiles",
            Json::Array(
                s.profiles_for_active()
                    .into_iter()
                    .map(|i| {
                        let p = &s.profiles[i];
                        let available = p.quality_type == "not_supported" || qualities_available.iter().any(|q| *q == p.quality_type);
                        let changed: usize = p.containers().map(|c| c.keys().len()).sum();
                        jobj(vec![
                            ("index", Json::Int(i as i64)),
                            ("name", jstr(&p.name)),
                            ("quality_type", jstr(&p.quality_type)),
                            ("intent", jstr(&p.intent_category)),
                            ("available", Json::Bool(available)),
                            ("settings", Json::Int(changed as i64)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("qualities", Json::Array(qualities)),
        ("intent", jstr(&m.extruders.first().map(|e| e.container(index::INTENT).meta_str("intent_category").unwrap_or_else(|| "default".into())).unwrap_or_else(|| "default".into()))),
        ("intents", Json::Array(intents.iter().map(|i| jstr(i)).collect())),
        (
            "recommended",
            jobj(vec![
                ("infill", jf(num(m.get_value(active_stack, "infill_sparse_density")))),
                ("gradual_infill", Json::Bool(num(m.get_value(active_stack, "gradual_infill_steps")) > 0.0)),
                ("support", Json::Bool(g("support_enable").map(|v| v.truthy()).unwrap_or(false))),
                ("support_extruder", Json::Int(num(g("support_extruder_nr")) as i64)),
                ("adhesion", Json::Bool(adhesion == "brim" || adhesion == "raft")),
                ("adhesion_type", jstr(&adhesion)),
            ]),
        ),
    ])
}

fn nodes_json(s: &Session) -> Json {
    let m = s.machine();
    Json::Array(
        s.nodes
            .iter()
            .enumerate()
            .map(|(o, n)| {
                let (min, max) = n.world_bounds();
                let w = n.world();
                let mesh_type = ["infill_mesh", "cutting_mesh", "support_mesh", "anti_overhang_mesh"]
                    .iter()
                    .find(|k| n.settings.iter().any(|(kk, v)| kk == *k && (v == "True" || v == "true")))
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "normal".into());
                let _ = (m, o);
                jobj(vec![
                    ("id", Json::Int(n.id as i64)),
                    ("name", jstr(&n.name)),
                    ("extruder", Json::Int(n.extruder as i64)),
                    ("outside", Json::Bool(n.outside)),
                    (
                        "position",
                        match n.parent {
                            // A child's position is its world translation.
                            Some(_) => Json::Array((0..3).map(|k| jf(w[k][3])).collect()),
                            None => Json::Array(n.position.iter().map(|v| jf(*v)).collect()),
                        },
                    ),
                    ("parent", n.parent.map(|p| Json::Int(p as i64)).unwrap_or(Json::Null)),
                    ("scale", Json::Array(n.scale.iter().map(|v| jf(*v)).collect())),
                    ("world", Json::Array(w.iter().flat_map(|r| r.iter()).map(|v| jf(*v)).collect())),
                    ("min", Json::Array(min.iter().map(|v| jf(*v)).collect())),
                    ("max", Json::Array(max.iter().map(|v| jf(*v)).collect())),
                    ("faces", Json::Int(n.face_count() as i64)),
                    ("mesh_type", jstr(&mesh_type)),
                    ("settings", Json::Array(n.settings.iter().map(|(k, v)| Json::Array(vec![jstr(k), jstr(v)])).collect())),
                ])
            })
            .collect(),
    )
}

fn state_json(app: &App) -> Json {
    let mut items: Vec<(&str, Json)> = vec![("generation", Json::Int(app.generation as i64)), ("version", jstr(env!("CARGO_PKG_VERSION")))];
    match &app.s {
        None => items.push(("resource_error", jstr(app.resource_error.as_deref().unwrap_or("")))),
        Some(s) => {
            items.push(("resources", jstr(&s.resources.display().to_string())));
            items.push(("post_processing", Json::Array(crate::postproc::working_list(s).into_iter().map(|c| jstr(&c.key)).collect())));
            items.push((
                "machines",
                Json::Array(s.machines.iter().map(|m| jobj(vec![("name", jstr(&m.global.name)), ("definition", jstr(&m.global.definition().id)), ("definition_name", jstr(&m.global.definition().name))])).collect()),
            ));
            items.push(("active", s.active.map(|a| Json::Int(a as i64)).unwrap_or(Json::Null)));
            items.push(("machine", machine_json(s, s.active_extruder)));
            items.push(("nodes", nodes_json(s)));
            items.push(("job_name", jstr(&s.job_name())));
            items.push((
                "prefs",
                jobj(vec![
                    ("jobname_prefix", Json::Bool(s.prefs.jobname_prefix)),
                    ("visibility_preset", jstr(&s.prefs.visibility_preset)),
                    ("auto_drop", Json::Bool(s.prefs.auto_drop)),
                    ("auto_slice", Json::Bool(app.auto_slice)),
                    ("currency", jstr(&s.prefs.currency)),
                ]),
            ));
        }
    }
    items.push(("slice", app.slice_json()));
    items.push(("toasts", Json::Array(app.toasts.iter().map(|t| jstr(t)).collect())));
    jobj(items)
}

// ---------------------------------------------------------------------------------------------
// Settings panel
// ---------------------------------------------------------------------------------------------

fn visibility_set(s: &Session, preset: &str) -> Option<HashSet<String>> {
    if preset == "all" {
        return None;
    }
    if preset == "custom" {
        return Some(s.prefs.custom_visible.iter().cloned().collect());
    }
    let path = s.resources.join("setting_visibility").join(format!("{}.cfg", preset));
    let text = std::fs::read_to_string(path).ok()?;
    let mut set = HashSet::new();
    let mut in_general = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            let sec = l.trim_matches(|c| c == '[' || c == ']');
            in_general = sec == "general";
            if !in_general {
                set.insert(sec.to_string());
            }
            continue;
        }
        if !in_general && !l.is_empty() && !l.contains('=') {
            set.insert(l.to_string());
        }
    }
    Some(set)
}

fn parse_target(s: &Session, t: Option<&str>) -> Target {
    match t {
        Some(t) if t.starts_with("e:") => Target::Extruder(t[2..].parse().unwrap_or(0)),
        Some(t) if t.starts_with("o:") => Target::Object(t[2..].parse().unwrap_or(0)),
        Some("global") => Target::Global,
        _ => {
            let _ = s;
            Target::Global
        }
    }
}

fn setting_entry(s: &Session, stack: StackId, key: &str, depth: usize, user_target: Option<Target>) -> Option<Json> {
    let m = s.machine()?;
    let sd = m.setting_definition(stack, key)?;
    let ty = sd.setting_type();
    let enabled = m.get_property(stack, key, "enabled", None).map(|v| parse_bool(&v)).unwrap_or(true);
    let value = m.get_property(stack, key, "value", None).unwrap_or(Value::None);
    // User value (and formula text) at the top of the stack.
    let (user, formula) = match stack {
        StackId::Object(o) => {
            let raw = m.object_stacks[o].user.raw_value(key);
            (raw.is_some(), raw.and_then(|r| if let curust_settings::instance::InstValue::Func(f) = r { Some(f.to_string()) } else { None }))
        }
        _ => {
            let st = m.stack(stack);
            let raw = st.user().raw_value(key);
            (raw.is_some(), raw.and_then(|r| if let curust_settings::instance::InstValue::Func(f) = r { Some(f.to_string()) } else { None }))
        }
    };
    let _ = user_target;
    let state = m.validation_state(stack, key);
    let options: Vec<Json> = sd.options().into_iter().map(|(k, l)| Json::Array(vec![jstr(&k), jstr(&l)])).collect();
    let str_prop = |p: &str| -> String {
        match sd.get(p) {
            Some(Prop::Value(v)) if !v.is_none() => v.py_str(),
            _ => String::new(),
        }
    };
    Some(jobj(vec![
        ("key", jstr(key)),
        ("label", jstr(&sd.label())),
        ("description", jstr(&str_prop("description"))),
        ("type", jstr(&ty)),
        ("unit", jstr(&str_prop("unit"))),
        ("options", Json::Array(options)),
        ("value", value_to_json(&value)),
        ("value_str", jstr(&value.py_str())),
        ("formula", formula.map(|f| jstr(&f)).unwrap_or(Json::Null)),
        ("user", Json::Bool(user)),
        ("enabled", Json::Bool(enabled)),
        ("depth", Json::Int(depth as i64)),
        ("state", jstr(state.as_str())),
        (
            "per_extruder",
            Json::Bool(m.get_property(StackId::Global, key, "settable_per_extruder", None).map(|v| v.truthy()).unwrap_or(false) && m.extruder_list().len() > 1),
        ),
        ("stack", jstr(&match stack {
            StackId::Global => "global".to_string(),
            StackId::Extruder(i) => format!("e:{}", m.extruders[i].position().unwrap_or_default()),
            StackId::Object(o) => format!("o:{}", s.nodes[o].id),
        })),
    ]))
}

fn settings_json(s: &Session, req: &Request) -> Result<Json, String> {
    let m = s.require_machine()?;
    let preset = req.q("preset").unwrap_or(&s.prefs.visibility_preset).to_string();
    let search = req.q("search").unwrap_or("").to_lowercase();
    let target = parse_target(s, req.q("target"));
    let def = m.global.definition();
    if let Target::Object(id) = target {
        // Per-model settings: only the overridden ones.
        let o = s.nodes.iter().position(|n| n.id == id).ok_or("Unknown model")?;
        let mut list = Vec::new();
        for (k, _) in &s.nodes[o].settings {
            if let Some(e) = setting_entry(s, StackId::Object(o), k, 0, Some(target)) {
                list.push(e);
            }
        }
        return Ok(jobj(vec![("settings", Json::Array(list))]));
    }
    let visible = visibility_set(s, &preset);
    let mut total = 0usize;
    let mut shown_count = 0usize;
    let mut categories = Vec::new();
    for &root in &def.roots {
        let cat = &def.defs[root];
        if cat.key == "machine_settings" || cat.key == "command_line_settings" {
            continue;
        }
        let mut entries = Vec::new();
        #[allow(clippy::too_many_arguments)]
        fn walk(s: &Session, def: &curust_settings::definition::DefinitionContainer, i: usize, depth: usize, visible: &Option<HashSet<String>>, search: &str, out: &mut Vec<Json>, counts: &mut (usize, usize)) {
            let d = &def.defs[i];
            let is_visible = visible.as_ref().map(|v| v.contains(&d.key)).unwrap_or(true);
            counts.0 += 1;
            if is_visible {
                counts.1 += 1;
            }
            let shown = if search.is_empty() { is_visible } else { d.key.to_lowercase().contains(search) || d.label().to_lowercase().contains(search) };
            let mut next_depth = depth;
            if shown {
                let m = s.machine().unwrap();
                let stack = s.stack_for_setting(&d.key);
                let enabled = m.get_property(stack, &d.key, "enabled", None).map(|v| parse_bool(&v)).unwrap_or(true);
                if enabled || !search.is_empty() {
                    if let Some(mut e) = setting_entry(s, stack, &d.key, depth, None) {
                        if let Json::Object(items) = &mut e {
                            items.push(("visible".into(), Json::Bool(is_visible)));
                        }
                        out.push(e);
                        next_depth = depth + 1;
                    }
                }
            }
            for &c in &d.children {
                walk(s, def, c, next_depth, visible, search, out, counts);
            }
        }
        let mut counts = (0usize, 0usize);
        for &c in &cat.children {
            walk(s, def, c, 0, &visible, &search, &mut entries, &mut counts);
        }
        total += counts.0;
        shown_count += counts.1;
        if entries.is_empty() && !search.is_empty() {
            continue;
        }
        if entries.is_empty() && counts.1 == 0 {
            continue;
        }
        let icon = match cat.get("icon") {
            Some(Prop::Value(v)) if !v.is_none() => v.py_str(),
            _ => String::new(),
        };
        let modified = entries.iter().filter(|e| e.get("user").and_then(|v| v.as_bool()).unwrap_or(false)).count();
        categories.push(jobj(vec![
            ("key", jstr(&cat.key)),
            ("label", jstr(&cat.label())),
            ("icon", jstr(&icon)),
            ("description", jstr(&match cat.get("description") { Some(Prop::Value(v)) if !v.is_none() => v.py_str(), _ => String::new() })),
            ("total", Json::Int(counts.0 as i64)),
            ("modified", Json::Int(modified as i64)),
            ("settings", Json::Array(entries)),
        ]));
    }
    Ok(jobj(vec![("categories", Json::Array(categories)), ("preset", jstr(&preset)), ("total", Json::Int(total as i64)), ("visible", Json::Int(shown_count as i64))]))
}

/// Every setting (tree order, with depth) and whether a visibility preset shows it: the
/// "Setting visibility" dialog.
fn visibility_tree(s: &Session, preset: &str) -> Result<Json, String> {
    let m = s.require_machine()?;
    let def = m.global.definition();
    let visible = visibility_set(s, preset);
    let mut cats = Vec::new();
    for &root in &def.roots {
        let cat = &def.defs[root];
        if cat.key == "machine_settings" || cat.key == "command_line_settings" {
            continue;
        }
        let mut items = Vec::new();
        fn walk(def: &curust_settings::definition::DefinitionContainer, i: usize, depth: usize, visible: &Option<HashSet<String>>, out: &mut Vec<Json>) {
            let d = &def.defs[i];
            out.push(jobj(vec![
                ("key", jstr(&d.key)),
                ("label", jstr(&d.label())),
                ("depth", Json::Int(depth as i64)),
                ("visible", Json::Bool(visible.as_ref().map(|v| v.contains(&d.key)).unwrap_or(true))),
            ]));
            for &c in &d.children {
                walk(def, c, depth + 1, visible, out);
            }
        }
        for &c in &cat.children {
            walk(def, c, 0, &visible, &mut items);
        }
        cats.push(jobj(vec![("key", jstr(&cat.key)), ("label", jstr(&cat.label())), ("settings", Json::Array(items))]));
    }
    Ok(jobj(vec![("categories", Json::Array(cats)), ("preset", jstr(preset))]))
}



/// All settings that can be overridden per model (for the per-model settings picker).
fn per_mesh_settings(s: &Session, search: &str) -> Json {
    let Some(m) = s.machine() else { return Json::Array(Vec::new()) };
    let def = m.global.definition();
    let search = search.to_lowercase();
    let mut out = Vec::new();
    for &root in &def.roots {
        let cat = &def.defs[root];
        let mut items = Vec::new();
        for i in def.tree_order() {
            let d = &def.defs[i];
            // Only descendants of this category.
            let mut p = d.parent;
            let mut inside = false;
            while let Some(pi) = p {
                if pi == root {
                    inside = true;
                    break;
                }
                p = def.defs[pi].parent;
            }
            if !inside || d.setting_type() == "category" {
                continue;
            }
            let per_mesh = match d.get("settable_per_mesh") {
                Some(Prop::Value(v)) => v.truthy(),
                _ => false,
            };
            if !per_mesh {
                continue;
            }
            if !search.is_empty() && !d.key.contains(&search) && !d.label().to_lowercase().contains(&search) {
                continue;
            }
            items.push(Json::Array(vec![jstr(&d.key), jstr(&d.label())]));
        }
        if !items.is_empty() {
            out.push(jobj(vec![("label", jstr(&cat.label())), ("settings", Json::Array(items))]));
        }
    }
    Json::Array(out)
}

const MACHINE_KEYS: &[&str] = &[
    "machine_width",
    "machine_depth",
    "machine_height",
    "machine_shape",
    "machine_center_is_zero",
    "machine_heated_bed",
    "machine_heated_build_volume",
    "machine_gcode_flavor",
    "gantry_height",
    "machine_extruder_count",
    "machine_extruders_share_heater",
    "machine_extruders_share_nozzle",
    "machine_start_gcode",
    "machine_end_gcode",
];
const EXTRUDER_KEYS: &[&str] = &[
    "machine_nozzle_size",
    "material_diameter",
    "machine_nozzle_offset_x",
    "machine_nozzle_offset_y",
    "machine_extruder_cooling_fan_number",
    "machine_extruder_start_code",
    "machine_extruder_start_code_duration",
    "machine_extruder_end_code",
    "machine_extruder_end_code_duration",
];

fn machine_settings_json(s: &Session) -> Result<Json, String> {
    let m = s.require_machine()?;
    let entry = |stack: StackId, key: &str| -> Option<Json> {
        let sd = m.setting_definition(stack, key)?;
        let v = m.get_property(stack, key, "value", None).unwrap_or(Value::None);
        Some(jobj(vec![
            ("key", jstr(key)),
            ("label", jstr(&sd.label())),
            ("type", jstr(&sd.setting_type())),
            ("unit", jstr(&match sd.get("unit") {
                Some(Prop::Value(u)) if !u.is_none() => u.py_str(),
                _ => String::new(),
            })),
            ("options", Json::Array(sd.options().into_iter().map(|(k, l)| Json::Array(vec![jstr(&k), jstr(&l)])).collect())),
            ("value", value_to_json(&v)),
            ("value_str", jstr(&v.py_str())),
        ]))
    };
    let global: Vec<Json> = MACHINE_KEYS.iter().filter_map(|k| entry(StackId::Global, k)).collect();
    // Print head polygon as min/max distances.
    let head = m.get_value(StackId::Global, "machine_head_with_fans_polygon").and_then(|v| v.iter_items().ok()).unwrap_or_default();
    let pts: Vec<(f64, f64)> = head
        .iter()
        .filter_map(|p| {
            let c = p.iter_items().ok()?;
            Some((c.first()?.as_f64()?, c.get(1)?.as_f64()?))
        })
        .collect();
    let (xmin, xmax, ymin, ymax) = pts.iter().fold((0.0f64, 0.0f64, 0.0f64, 0.0f64), |a, p| (a.0.min(p.0), a.1.max(p.0), a.2.min(p.1), a.3.max(p.1)));
    let mut exts = Vec::new();
    for (pos, &i) in m.extruder_list().iter().enumerate() {
        exts.push(jobj(vec![
            ("position", Json::Int(pos as i64)),
            ("name", jstr(&m.extruders[i].definition().name)),
            ("settings", Json::Array(EXTRUDER_KEYS.iter().filter_map(|k| entry(StackId::Extruder(i), k)).collect())),
        ]));
    }
    Ok(jobj(vec![
        ("printer", Json::Array(global)),
        ("head", jobj(vec![("xmin", jf(-xmin)), ("ymin", jf(-ymin)), ("xmax", jf(xmax)), ("ymax", jf(ymax))])),
        ("extruders", Json::Array(exts)),
    ]))
}

fn materials_json(s: &Session, position: usize) -> Result<Json, String> {
    let m = s.require_machine()?;
    let i = *m.extruder_list().get(position).ok_or("Unknown extruder")?;
    let node = s.tree.machine(&s.reg, &m.global.definition().id);
    let vn = m.extruders[i].container(index::VARIANT).name();
    let variant = node.variants.get(&vn).or_else(|| node.variants.first()).ok_or("No materials")?;
    let approx = machine::approximate_material_diameter(m, i);
    let mut out = Vec::new();
    for (base, mn) in &variant.materials.items {
        let meta = s.reg.metadata(&mn.container_id);
        let get = |k: &str| meta.and_then(|e| e.get_str(k)).unwrap_or_default();
        let diameter_ok = meta.and_then(|e| e.get_str("approximate_diameter")).and_then(|d| d.parse::<i64>().ok()).map(|d| d == approx).unwrap_or(true);
        if !diameter_ok {
            continue;
        }
        out.push(jobj(vec![
            ("base", jstr(base)),
            ("name", jstr(&get("name"))),
            ("brand", jstr(&get("brand"))),
            ("type", jstr(&get("material"))),
            ("color", jstr(&meta.and_then(|e| e.get_str("color_code")).unwrap_or_else(|| "#888888".into()))),
            ("color_name", jstr(&get("color_name"))),
            ("guid", jstr(&get("GUID"))),
        ]));
    }
    Ok(Json::Array(out))
}

/// Preferences > Materials: every material of the active extruder's printer / nozzle (with the
/// active filament diameter), with the fields the material editor shows.
fn material_manager_json(s: &Session, position: usize) -> Result<Json, String> {
    let m = s.require_machine()?;
    let i = *m.extruder_list().get(position).ok_or("Unknown extruder")?;
    let node = s.tree.machine(&s.reg, &m.global.definition().id);
    let vn = m.extruders[i].container(index::VARIANT).name();
    let variant = node.variants.get(&vn).or_else(|| node.variants.first()).ok_or("No materials")?;
    let approx = machine::approximate_material_diameter(m, i);
    let active = m.extruders[i].container(index::MATERIAL).meta_str("base_file").unwrap_or_default();
    let user = s.user_materials();
    let machine_def = m.reg.definition(&m.global.definition().id);
    let mut out = Vec::new();
    for (base, mn) in &variant.materials.items {
        let meta = s.reg.metadata(&mn.container_id);
        let get = |k: &str| meta.and_then(|e| e.get_str(k)).unwrap_or_default();
        if meta.and_then(|e| e.get_str("approximate_diameter")).and_then(|d| d.parse::<i64>().ok()).map(|d| d != approx).unwrap_or(false) {
            continue;
        }
        let Ok(d) = s.material_details(base) else { continue };
        // Print settings: the material container's value, else the definition's default.
        let container = s.reg.instance(&mn.container_id);
        let mut settings = Vec::new();
        for (key, _) in crate::materials::EDITABLE_SETTINGS {
            let v = container
                .as_ref()
                .and_then(|c| c.get_value(key, machine_def.as_deref()))
                .and_then(|p| match p {
                    curust_settings::definition::Prop::Value(v) => v.as_f64(),
                    _ => None,
                })
                .or_else(|| m.get_value(StackId::Extruder(i), key).and_then(|v| v.as_f64()).filter(|_| false))
                .or_else(|| machine_def.as_ref().and_then(|d| d.get_property(key, "default_value")).and_then(|p| match p {
                    curust_settings::definition::Prop::Value(v) => v.as_f64(),
                    _ => None,
                }));
            let def = machine_def.as_ref().and_then(|d| d.find(key));
            settings.push(jobj(vec![
                ("key", jstr(key)),
                ("label", jstr(&def.map(|d| d.label()).unwrap_or_else(|| key.to_string()))),
                ("unit", jstr(&def.map(|d| d.str_prop("unit")).unwrap_or_default())),
                ("description", jstr(&def.map(|d| d.str_prop("description")).unwrap_or_default())),
                ("value", v.map(jf).unwrap_or(Json::Null)),
            ]));
        }
        let guid = get("GUID");
        let cost = s.prefs.material_costs.iter().find(|(g, _, _)| *g == guid);
        out.push(jobj(vec![
            ("base", jstr(base)),
            ("name", jstr(&get("name"))),
            ("brand", jstr(&get("brand"))),
            ("type", jstr(&get("material"))),
            ("color", jstr(&meta.and_then(|e| e.get_str("color_code")).unwrap_or_else(|| "#888888".into()))),
            ("color_name", jstr(&get("color_name"))),
            ("guid", jstr(&guid)),
            ("description", jstr(&d.meta_text("description"))),
            ("adhesion_info", jstr(&d.meta_text("adhesion_info"))),
            ("density", jstr(&d.property("density"))),
            ("diameter", jstr(&d.property("diameter"))),
            ("weight", jstr(&d.property("weight"))),
            ("user", Json::Bool(user.contains(base))),
            ("active", Json::Bool(*base == active)),
            ("in_use", Json::Bool(s.material_in_use(base))),
            ("spool_cost", cost.map(|c| jf(c.1)).unwrap_or(Json::Null)),
            ("spool_weight", cost.map(|c| jf(c.2)).unwrap_or(Json::Null)),
            ("settings", Json::Array(settings)),
        ]));
    }
    Ok(Json::Array(out))
}

fn layers_binary(res: &SliceResult) -> Vec<u8> {
    let mut out = Vec::new();
    let push_u32 = |o: &mut Vec<u8>, v: u32| o.extend_from_slice(&v.to_le_bytes());
    let mut layers: Vec<&curust_engine::communication::job::LayerData> = res.layers.iter().collect();
    layers.sort_by_key(|l| l.id);
    out.extend_from_slice(b"CRLY");
    push_u32(&mut out, layers.len() as u32);
    for l in layers {
        out.extend_from_slice(&(l.id as i32).to_le_bytes());
        out.extend_from_slice(&((l.height as f64 / 1000.0) as f32).to_le_bytes());
        out.extend_from_slice(&((l.thickness as f64 / 1000.0) as f32).to_le_bytes());
        push_u32(&mut out, l.path_segments.len() as u32);
        for p in &l.path_segments {
            let n = p.points.len() / 3;
            push_u32(&mut out, p.extruder as u32);
            push_u32(&mut out, n as u32);
            for f in &p.points {
                out.extend_from_slice(&f.to_le_bytes());
            }
            for k in 0..n.saturating_sub(1) {
                out.push(*p.line_type.get(k).unwrap_or(&0));
            }
            while out.len() % 4 != 0 {
                out.push(0);
            }
            for k in 0..n.saturating_sub(1) {
                out.extend_from_slice(&p.line_width.get(k).copied().unwrap_or(0.0).to_le_bytes());
            }
            for k in 0..n.saturating_sub(1) {
                out.extend_from_slice(&p.line_feedrate.get(k).copied().unwrap_or(0.0).to_le_bytes());
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Request handling
// ---------------------------------------------------------------------------------------------

fn jget_f(j: &Json, k: &str) -> Option<f64> {
    j.get(k).and_then(|v| v.as_f64())
}
fn jget_s<'a>(j: &'a Json, k: &str) -> Option<&'a str> {
    j.get(k).and_then(|v| v.as_str())
}
fn jget_u(j: &Json, k: &str) -> Option<usize> {
    j.get(k).and_then(|v| v.as_f64()).map(|v| v.max(0.0) as usize)
}

fn handle(app: &mut App, req: &Request) -> Response {
    app.last_request = Instant::now();
    app.poll_slice();
    app.load_pending_files();
    if req.method == "GET" {
        if let Some((ct, body)) = web::asset(&req.path) {
            return Response::bytes(ct, body);
        }
    }
    let r = (|| -> Result<Response, String> {
        let p = req.path.as_str();
        let body = req.json();
        let mutating = req.method == "POST";
        let resp = match p {
            "/api/state" => {
                let j = state_json(app);
                app.toasts.clear();
                Response::json(j)
            }
            "/api/definitions" => {
                let s = app.session()?;
                Response::json(Json::Array(s.available_definitions().into_iter().map(|(id, name, mf)| Json::Array(vec![jstr(&id), jstr(&name), jstr(&mf)])).collect()))
            }
            "/api/settings" => Response::json(settings_json(app.session()?, req)?),
            "/api/per_mesh_settings" => Response::json(per_mesh_settings(app.session()?, req.q("search").unwrap_or(""))),
            "/api/machine_settings" => Response::json(machine_settings_json(app.session()?)?),
            "/api/materials" => Response::json(materials_json(app.session()?, req.q("position").and_then(|p| p.parse().ok()).unwrap_or(0))?),
            "/api/mesh" => {
                let s = app.session()?;
                let id: u64 = req.q("id").and_then(|v| v.parse().ok()).unwrap_or(0);
                let n = s.nodes.iter().find(|n| n.id == id).ok_or("Unknown model")?;
                let mut out = Vec::with_capacity(n.mesh.vertices.len() * 12);
                for v in n.mesh.vertices.iter() {
                    for c in v {
                        out.extend_from_slice(&c.to_le_bytes());
                    }
                }
                Response::bytes("application/octet-stream", out)
            }
            "/api/layers" => {
                if app.layers_cache.is_none() {
                    let (_, _, res) = app.result.as_ref().ok_or("Not sliced")?;
                    app.layers_cache = Some(layers_binary(res));
                }
                Response::bytes("application/octet-stream", app.layers_cache.clone().unwrap())
            }
            "/api/gcode" => {
                let gcode = app.exported_gcode()?;
                let name = format!("{}.gcode", app.session()?.job_name());
                // The file as Cura writes it (text mode: "\r\n" line endings on Windows).
                let mut r = Response::bytes("text/x-gcode; charset=utf-8", curust_engine::utils::text_mode::platform_text(&gcode).as_bytes().to_vec());
                r.extra_headers.push(("Content-Disposition".into(), format!("attachment; filename=\"{}\"", name.replace('"', "'"))));
                r
            }
            "/api/export_3mf" => {
                let s = app.session()?;
                let (w, d, _) = s.machine_size();
                let models: Vec<(String, Vec<[f32; 3]>, Vec<(String, String)>)> = s
                    .nodes
                    .iter()
                    .map(|n| {
                        let wm = n.world();
                        let verts = n
                            .mesh
                            .vertices
                            .iter()
                            .map(|p| {
                                let (x, y, z) = (p[0] as f64, p[1] as f64, p[2] as f64);
                                [
                                    (wm[0][0] * x + wm[0][1] * y + wm[0][2] * z + wm[0][3]) as f32,
                                    (wm[1][0] * x + wm[1][1] * y + wm[1][2] * z + wm[1][3]) as f32,
                                    (wm[2][0] * x + wm[2][1] * y + wm[2][2] * z + wm[2][3]) as f32,
                                ]
                            })
                            .collect();
                        let mut settings = n.settings.clone();
                        settings.push(("extruder_nr".into(), n.extruder.to_string()));
                        (n.name.clone(), verts, settings)
                    })
                    .collect();
                let data = crate::mesh_io::write_3mf(&models, w, d);
                let mut r = Response::bytes("model/3mf", data);
                r.extra_headers.push(("Content-Disposition".into(), format!("attachment; filename=\"{}.3mf\"", s.job_name().replace('"', "'"))));
                r
            }
            "/api/save_file" => {
                // Save the G-code to a folder on disk (the folder the user chose).
                let gcode = app.exported_gcode()?;
                let dir = jget_s(&body, "dir").map(std::path::PathBuf::from).unwrap_or_else(default_output_dir);
                let name = jget_s(&body, "name").map(String::from).unwrap_or_else(|| format!("{}.gcode", app.s.as_ref().unwrap().job_name()));
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                let path = dir.join(&name);
                std::fs::write(&path, curust_engine::utils::text_mode::platform_text(&gcode).as_bytes()).map_err(|e| format!("{}: {}", path.display(), e))?;
                Response::json(jobj(vec![("path", jstr(&path.display().to_string()))]))
            }
            "/api/postproc" => {
                let list = app.pp_list()?;
                Response::json(crate::postproc::dialog_json(app.session()?, &list))
            }
            "/api/postproc/op" => {
                let mut list = app.pp_list()?;
                let idx = |k: &str| body.get(k).and_then(|v| v.as_f64()).map(|f| f as usize).unwrap_or(usize::MAX);
                let op = match jget_s(&body, "op").unwrap_or("") {
                    "add" => crate::postproc::ScriptOp::Add(jget_s(&body, "key").ok_or("key missing")?.to_string()),
                    "remove" => crate::postproc::ScriptOp::Remove(idx("index")),
                    "move" => crate::postproc::ScriptOp::Move(idx("index"), idx("to")),
                    "set" => crate::postproc::ScriptOp::Set(idx("index"), jget_s(&body, "setting").ok_or("setting missing")?.to_string(), jget_s(&body, "value").unwrap_or("").to_string()),
                    "clear" => crate::postproc::ScriptOp::Clear,
                    other => return Err(format!("unknown operation {}", other)),
                };
                crate::postproc::apply_op(&mut list, op);
                app.pp_working = Some(list.clone());
                Response::json(crate::postproc::dialog_json(app.session()?, &list))
            }
            "/api/postproc/close" => {
                // writeScriptsToStack when the dialog closes.
                if let Some(list) = app.pp_working.take() {
                    let s = app.session_mut()?;
                    crate::postproc::commit_scripts(s, &list)?;
                    persist::save(s);
                    app.exported = None;
                }
                let s = app.session()?;
                let n = crate::postproc::working_list(s).len();
                Response::json(jobj(vec![("count", Json::Int(n as i64))]))
            }
            "/api/open_homepage" => {
                // Only the project's own pages, never an arbitrary URL from the page.
                let url = match body.get("page").and_then(|v| v.as_str()) {
                    Some("issues") => format!("{}/issues", crate::HOMEPAGE),
                    Some("releases") => format!("{}/releases", crate::HOMEPAGE),
                    _ => crate::HOMEPAGE.to_string(),
                };
                open_path(&url);
                Response::ok()
            }
            "/api/postproc/open_folder" => {
                let dir = crate::postproc::user_script_dirs().into_iter().next().unwrap_or_default();
                let _ = std::fs::create_dir_all(&dir);
                open_path(&dir.display().to_string());
                Response::json(jobj(vec![("path", jstr(&dir.display().to_string()))]))
            }
            "/api/market/search" => {
                let url = match req.q("next").filter(|n| !n.is_empty()) {
                    Some(n) => n.to_string(),
                    None => crate::marketplace::search_url(req.q("type").unwrap_or(""), req.q("q").unwrap_or("")),
                };
                Response::json(crate::marketplace::search(&url)?)
            }
            "/api/market/installed" => {
                let ours: Vec<Json> = crate::marketplace::installed().into_iter().map(|(_, v)| v).collect();
                let cura: Vec<Json> = crate::marketplace::cura_installed().into_iter().map(|(_, v)| v).collect();
                Response::json(jobj(vec![("curust", Json::Array(ours)), ("cura", Json::Array(cura)), ("cura_dirs", Json::Array(crate::marketplace::cura_data_dirs().iter().map(|p| jstr(&p.display().to_string())).collect()))]))
            }
            "/api/market/install" | "/api/market/install_file" => {
                let report = if p == "/api/market/install" {
                    let id = jget_s(&body, "package_id").ok_or("package_id missing")?.to_string();
                    let url = jget_s(&body, "download_url").unwrap_or("").to_string();
                    crate::marketplace::download_and_install(&id, &url)?
                } else {
                    crate::marketplace::install_package(&req.body)?
                };
                let s = app.session_mut()?;
                persist::reload(s);
                Response::json(jobj(vec![("package_id", jstr(&report.package_id)), ("message", jstr(&report.summary())), ("usable", Json::Bool(report.usable()))]))
            }
            "/api/market/uninstall" => {
                let id = jget_s(&body, "package_id").ok_or("package_id missing")?.to_string();
                crate::marketplace::uninstall(&id)?;
                let s = app.session_mut()?;
                persist::reload(s);
                Response::ok()
            }
            "/api/usb/ports" => Response::json(Json::Array(
                crate::serial::list_ports().into_iter().map(|p| jobj(vec![("name", jstr(&p.name)), ("description", jstr(&p.description)), ("usb", Json::Bool(p.usb))])).collect(),
            )),
            "/api/usb/connect" => {
                let port = jget_s(&body, "port").ok_or("port missing")?.to_string();
                let baud = jget_f(&body, "baud").filter(|b| *b > 0.0).map(|b| b as u32);
                let n = app.session().map(|s| s.extruder_count()).unwrap_or(1);
                app.usb = None;
                app.usb = Some(crate::usbprint::UsbPrinter::connect(&port, baud, n));
                Response::ok()
            }
            "/api/usb/disconnect" => {
                if let Some(u) = app.usb.take() {
                    u.close();
                }
                Response::ok()
            }
            "/api/usb/print" => {
                let gcode = app.exported_gcode()?;
                let s = app.session()?;
                let name = s.job_name();
                let est = app.result.as_ref().map(|r| r.1.print_time.max(0) as u64).unwrap_or(0);
                app.usb.as_ref().ok_or("No printer is connected over USB.")?.print(gcode, &name, est)?;
                Response::ok()
            }
            "/api/usb/control" => {
                let u = app.usb.as_ref().ok_or("No printer is connected over USB.")?;
                match jget_s(&body, "action").unwrap_or("") {
                    "pause" => u.pause(),
                    "resume" => u.resume(),
                    "cancel" => u.cancel(),
                    other => return Err(format!("unknown action {}", other)),
                }
                Response::ok()
            }
            "/api/usb/gcode" => {
                app.usb.as_ref().ok_or("No printer is connected over USB.")?.send(jget_s(&body, "gcode").unwrap_or(""));
                Response::ok()
            }
            "/api/net/config" => {
                let kind = jget_s(&body, "kind").unwrap_or("");
                let s = app.session_mut()?;
                let a = s.active.ok_or("Add a printer first")?;
                if kind.is_empty() {
                    s.machines[a].global.set_meta("curust_network_printer", curust_settings::py::Value::Str("".into()));
                } else {
                    let np = crate::netprint::NetPrinter {
                        kind: crate::netprint::Kind::parse(kind).ok_or("unknown printer type")?,
                        url: jget_s(&body, "url").unwrap_or("").trim().to_string(),
                        key: jget_s(&body, "key").unwrap_or("").to_string(),
                        user: jget_s(&body, "user").unwrap_or("").to_string(),
                    };
                    s.machines[a].global.set_meta("curust_network_printer", curust_settings::py::Value::Str(np.to_json().to_string_compact().into()));
                }
                persist::save(s);
                let mut n = app.net.lock().unwrap();
                n.status = None;
                n.updated = None;
                Response::ok()
            }
            "/api/net/test" => {
                let np = crate::netprint::NetPrinter {
                    kind: crate::netprint::Kind::parse(jget_s(&body, "kind").unwrap_or("")).ok_or("Choose the type of printer")?,
                    url: jget_s(&body, "url").unwrap_or("").trim().to_string(),
                    key: jget_s(&body, "key").unwrap_or("").to_string(),
                    user: jget_s(&body, "user").unwrap_or("").to_string(),
                };
                Response::json(jobj(vec![("message", jstr(&np.test()?))]))
            }
            "/api/net/print" => {
                let np = net_printer(app.session()?).ok_or("Set up a network printer first.")?;
                let gcode = app.exported_gcode()?;
                let name = format!("{}.gcode", app.session()?.job_name());
                let start = body.get("start").and_then(|v| v.as_bool()).unwrap_or(true);
                let shared = app.net.clone();
                {
                    let mut n = shared.lock().unwrap();
                    if matches!(n.job, Some((true, _, _))) {
                        return Err("An upload is already in progress.".into());
                    }
                    n.job = Some((true, format!("Sending {} to the printer…", name), false));
                }
                std::thread::spawn(move || {
                    let r = np.upload(&name, gcode.as_bytes(), start);
                    let mut n = shared.lock().unwrap();
                    n.job = Some(match r {
                        Ok(m) => (false, m, false),
                        Err(e) => (false, e, true),
                    });
                    n.updated = None;
                });
                Response::ok()
            }
            "/api/net/control" | "/api/net/gcode" => {
                let np = net_printer(app.session()?).ok_or("Set up a network printer first.")?;
                if p == "/api/net/control" {
                    np.control(jget_s(&body, "action").unwrap_or(""))?;
                } else {
                    np.gcode(jget_s(&body, "gcode").unwrap_or(""))?;
                }
                app.net.lock().unwrap().updated = None;
                Response::ok()
            }
            "/api/monitor" => {
                let mut items: Vec<(&str, Json)> = Vec::new();
                if let Some(u) = &app.usb {
                    let st = u.state.lock().unwrap().clone();
                    items.push((
                        "usb",
                        jobj(vec![
                            ("port", jstr(&st.port)),
                            ("baud", Json::Int(st.baud as i64)),
                            ("status", jstr(&st.status)),
                            ("message", jstr(&st.message)),
                            ("firmware", jstr(&st.firmware)),
                            ("hotends", Json::Array(st.hotends.iter().map(|t| temp_json((t.current, t.target))).collect())),
                            ("bed", temp_json((st.bed.current, st.bed.target))),
                            ("printing", Json::Bool(st.printing)),
                            ("paused", Json::Bool(st.paused)),
                            ("job_name", jstr(&st.job_name)),
                            ("progress", jf(if st.lines > 0 { st.line as f64 / st.lines as f64 } else { 0.0 })),
                            ("elapsed", Json::Int(st.elapsed as i64)),
                            ("estimated", Json::Int(st.estimated as i64)),
                            ("console", Json::Array(st.console.iter().map(|l| jstr(l)).collect())),
                        ]),
                    ));
                }
                if let Some(np) = app.s.as_ref().and_then(net_printer) {
                    let shared = app.net.clone();
                    let mut n = shared.lock().unwrap();
                    if !n.fetching && n.updated.is_none_or(|t| t.elapsed() > Duration::from_secs(2)) {
                        n.fetching = true;
                        let sh = shared.clone();
                        let np2 = np.clone();
                        std::thread::spawn(move || {
                            let r = np2.status();
                            let mut n = sh.lock().unwrap();
                            n.status = Some(r);
                            n.updated = Some(Instant::now());
                            n.fetching = false;
                        });
                    }
                    let mut net = vec![("config", np.to_json()), ("label", jstr(np.kind.label()))];
                    match &n.status {
                        Some(Ok(st)) => net.push((
                            "status",
                            jobj(vec![
                                ("state", jstr(&st.state)),
                                ("printing", Json::Bool(st.printing)),
                                ("paused", Json::Bool(st.paused)),
                                ("file", jstr(&st.file)),
                                ("progress", jf(st.progress)),
                                ("elapsed", jf(st.elapsed)),
                                ("remaining", jf(st.remaining)),
                                ("hotends", Json::Array(st.hotends.iter().map(|t| temp_json(*t)).collect())),
                                ("bed", temp_json(st.bed)),
                            ]),
                        )),
                        Some(Err(e)) => net.push(("error", jstr(e))),
                        None => {}
                    }
                    if let Some((busy, msg, err)) = &n.job {
                        net.push(("job", jobj(vec![("busy", Json::Bool(*busy)), ("message", jstr(msg)), ("error", Json::Bool(*err))])));
                    }
                    items.push(("net", jobj(net)));
                }
                Response::json(jobj(items))
            }
            "/api/visibility_tree" => {
                let s = app.session()?;
                let preset = req.q("preset").unwrap_or(&s.prefs.visibility_preset).to_string();
                Response::json(visibility_tree(s, &preset)?)
            }
            "/api/default_dir" => Response::json(jobj(vec![("dir", jstr(&default_output_dir().display().to_string()))])),
            "/api/slice" if mutating => app.start_slice(),
            "/api/slice" => Response::json(app.slice_json()),
            "/api/slice/cancel" => {
                if let SliceState::Running(run) = std::mem::replace(&mut app.slice, SliceState::Idle) {
                    run.cancel();
                }
                Response::ok()
            }
            "/api/machine/add" => {
                let def = jget_s(&body, "definition").ok_or("definition missing")?.to_string();
                let name = jget_s(&body, "name").map(String::from);
                let s = app.session_mut()?;
                s.add_machine(&def, name.as_deref())?;
                app.invalidate();
                Response::ok()
            }
            "/api/machine/select" => {
                let i = jget_u(&body, "index").ok_or("index missing")?;
                app.session_mut()?.set_active_machine(i);
                app.invalidate();
                Response::ok()
            }
            "/api/machine/remove" => {
                let i = jget_u(&body, "index").ok_or("index missing")?;
                app.session_mut()?.remove_machine(i);
                app.invalidate();
                Response::ok()
            }
            "/api/machine/rename" => {
                let i = jget_u(&body, "index").ok_or("index missing")?;
                let n = jget_s(&body, "name").ok_or("name missing")?.to_string();
                app.session_mut()?.rename_machine(i, &n);
                app.invalidate();
                Response::ok()
            }
            "/api/machine/setting" => {
                let s = app.session_mut()?;
                let target = parse_target(s, jget_s(&body, "target"));
                let key = jget_s(&body, "key").ok_or("key missing")?.to_string();
                let value = body.get("value").map(|v| v.as_str().map(String::from).unwrap_or_else(|| json_plain(v))).unwrap_or_default();
                if key == "machine_head_with_fans_polygon_minmax" {
                    let v: Vec<f64> = value.split(',').filter_map(|t| t.trim().parse().ok()).collect();
                    if v.len() == 4 {
                        let poly = format!("[[{}, {}], [{}, {}], [{}, {}], [{}, {}]]", -v[0], v[3], -v[0], -v[1], v[2], v[3], v[2], -v[1]);
                        s.set_machine_setting(Target::Global, "machine_head_with_fans_polygon", &poly)?;
                    }
                } else {
                    s.set_machine_setting(target, &key, &value)?;
                }
                app.invalidate();
                Response::ok()
            }
            "/api/extruder/active" => {
                let p = jget_u(&body, "position").unwrap_or(0);
                let s = app.session_mut()?;
                s.active_extruder = p.min(s.extruder_count() - 1);
                persist::save(s);
                Response::ok()
            }
            "/api/extruder/variant" => {
                let p = jget_u(&body, "position").unwrap_or(0);
                let v = jget_s(&body, "name").ok_or("name missing")?.to_string();
                app.session_mut()?.set_variant(p, &v);
                app.invalidate();
                Response::ok()
            }
            "/api/extruder/material" => {
                let p = jget_u(&body, "position").unwrap_or(0);
                let v = jget_s(&body, "base").ok_or("base missing")?.to_string();
                app.session_mut()?.set_material(p, &v);
                app.invalidate();
                Response::ok()
            }
            "/api/extruder/enabled" => {
                let p = jget_u(&body, "position").unwrap_or(0);
                let e = body.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
                app.session_mut()?.set_extruder_enabled(p, e);
                app.invalidate();
                Response::ok()
            }
            "/api/quality" => {
                let q = jget_s(&body, "type").ok_or("type missing")?.to_string();
                let s = app.session_mut()?;
                if body.get("discard").and_then(|v| v.as_bool()).unwrap_or(false) {
                    s.clear_user_settings();
                }
                s.set_quality(&q);
                app.invalidate();
                Response::ok()
            }
            "/api/intent" => {
                let c = jget_s(&body, "category").ok_or("category missing")?.to_string();
                app.session_mut()?.set_intent(&c);
                app.invalidate();
                Response::ok()
            }
            "/api/profile/create" => {
                let name = jget_s(&body, "name").ok_or("name missing")?.to_string();
                let s = app.session_mut()?;
                s.create_profile(&name)?;
                persist::save(s);
                app.invalidate();
                Response::ok()
            }
            "/api/profile/update" => {
                let s = app.session_mut()?;
                s.update_profile()?;
                persist::save(s);
                app.invalidate();
                Response::ok()
            }
            "/api/profile/activate" => {
                let i = jget_u(&body, "index").ok_or("index missing")?;
                let s = app.session_mut()?;
                if body.get("discard").and_then(|v| v.as_bool()).unwrap_or(false) {
                    s.clear_user_settings();
                }
                s.activate_profile(i)?;
                persist::save(s);
                app.invalidate();
                Response::ok()
            }
            "/api/profile/rename" => {
                let i = jget_u(&body, "index").ok_or("index missing")?;
                let name = jget_s(&body, "name").ok_or("name missing")?.to_string();
                let s = app.session_mut()?;
                s.rename_profile(i, &name)?;
                persist::save(s);
                app.invalidate();
                Response::ok()
            }
            "/api/profile/duplicate" => {
                let i = jget_u(&body, "index").ok_or("index missing")?;
                let name = jget_s(&body, "name").ok_or("name missing")?.to_string();
                let s = app.session_mut()?;
                s.duplicate_profile(i, &name)?;
                persist::save(s);
                Response::ok()
            }
            "/api/profile/delete" => {
                let i = jget_u(&body, "index").ok_or("index missing")?;
                let s = app.session_mut()?;
                s.delete_profile(i)?;
                persist::save(s);
                app.invalidate();
                Response::ok()
            }
            "/api/profile/import" => {
                let name = req.q("name").unwrap_or("profile.curaprofile").to_string();
                let s = app.session_mut()?;
                let r = s.import_profile(&name, &req.body);
                persist::save(s);
                Response::json(jobj(vec![
                    ("status", jstr(r.status)),
                    ("message", jstr(&r.message)),
                    ("index", r.profile.map(|i| Json::Int(i as i64)).unwrap_or(Json::Null)),
                ]))
            }
            "/api/profile/export" => {
                let i: usize = req.q("index").and_then(|v| v.parse().ok()).ok_or("index missing")?;
                let (name, data) = app.session()?.export_profile(i)?;
                let mut r = Response::bytes("application/x-curaprofile", data);
                r.extra_headers.push(("Content-Disposition".into(), format!("attachment; filename=\"{}\"", name.replace('"', "'"))));
                r
            }
            "/api/material_manager" => Response::json(material_manager_json(app.session()?, req.q("position").and_then(|p| p.parse().ok()).unwrap_or(0))?),
            "/api/material/create" => {
                let s = app.session_mut()?;
                let id = s.create_material()?;
                app.invalidate();
                Response::json(jobj(vec![("base", jstr(&id))]))
            }
            "/api/material/duplicate" => {
                let base = jget_s(&body, "base").ok_or("base missing")?.to_string();
                let s = app.session_mut()?;
                let id = s.duplicate_material(&base)?;
                app.invalidate();
                Response::json(jobj(vec![("base", jstr(&id))]))
            }
            "/api/material/remove" => {
                let base = jget_s(&body, "base").ok_or("base missing")?.to_string();
                app.session_mut()?.remove_material(&base)?;
                app.invalidate();
                Response::ok()
            }
            "/api/material/update" => {
                let base = jget_s(&body, "base").ok_or("base missing")?.to_string();
                let field = jget_s(&body, "field").ok_or("field missing")?.to_string();
                let value = jget_s(&body, "value").map(String::from).or_else(|| jget_f(&body, "value").map(|f| f.to_string())).ok_or("value missing")?;
                app.session_mut()?.update_material(&base, &field, &value)?;
                app.invalidate();
                Response::ok()
            }
            "/api/material/import" => {
                let name = req.q("name").unwrap_or("material.xml.fdm_material").to_string();
                let s = app.session_mut()?;
                let id = s.import_material(&name, &req.body)?;
                let label = s.reg.metadata(&id).and_then(|e| e.get_str("name")).unwrap_or_else(|| id.clone());
                app.invalidate();
                Response::json(jobj(vec![("base", jstr(&id)), ("message", jstr(&format!("Successfully imported material {}", label)))]))
            }
            "/api/material/export" => {
                let base = req.q("base").ok_or("base missing")?.to_string();
                let (name, data) = app.session()?.export_material(&base)?;
                let mut r = Response::bytes("application/x-ultimaker-material-profile", data);
                r.extra_headers.push(("Content-Disposition".into(), format!("attachment; filename=\"{}\"", name.replace('"', "'"))));
                r
            }
            "/api/clear_user" => {
                app.session_mut()?.clear_user_settings();
                app.invalidate();
                Response::ok()
            }
            "/api/setting" => {
                let s = app.session_mut()?;
                let key = jget_s(&body, "key").ok_or("key missing")?.to_string();
                let target = match jget_s(&body, "target") {
                    Some(t) => parse_target(s, Some(t)),
                    None => match s.stack_for_setting(&key) {
                        StackId::Extruder(i) => Target::Extruder(s.machine().unwrap().extruders[i].position().and_then(|p| p.parse().ok()).unwrap_or(0)),
                        _ => Target::Global,
                    },
                };
                if body.get("reset").and_then(|v| v.as_bool()).unwrap_or(false) {
                    s.reset_setting(target, &key);
                } else {
                    let value = body.get("value").map(|v| v.as_str().map(String::from).unwrap_or_else(|| json_plain(v))).unwrap_or_default();
                    s.set_setting(target, &key, &value)?;
                }
                app.invalidate();
                Response::ok()
            }
            "/api/recommended" => {
                let s = app.session_mut()?;
                if let Some(v) = jget_f(&body, "infill") {
                    let key = "infill_sparse_density";
                    let target = match s.stack_for_setting(key) {
                        StackId::Extruder(i) => Target::Extruder(s.machine().unwrap().extruders[i].position().and_then(|p| p.parse().ok()).unwrap_or(0)),
                        _ => Target::Global,
                    };
                    s.set_setting(target, key, &curust_settings::py::float_repr(v).trim_end_matches(".0").to_string())?;
                }
                if let Some(b) = body.get("gradual_infill").and_then(|v| v.as_bool()) {
                    let target = match s.stack_for_setting("gradual_infill_steps") {
                        StackId::Extruder(i) => Target::Extruder(s.machine().unwrap().extruders[i].position().and_then(|p| p.parse().ok()).unwrap_or(0)),
                        _ => Target::Global,
                    };
                    s.set_setting(target, "gradual_infill_steps", if b { "5" } else { "0" })?;
                }
                if let Some(b) = body.get("support").and_then(|v| v.as_bool()) {
                    s.set_setting(Target::Global, "support_enable", if b { "True" } else { "False" })?;
                }
                if let Some(e) = jget_u(&body, "support_extruder") {
                    s.set_setting(Target::Global, "support_extruder_nr", &e.to_string())?;
                }
                if let Some(b) = body.get("adhesion").and_then(|v| v.as_bool()) {
                    // RecommendedAdhesionSelector.
                    let mut t = "skirt".to_string();
                    if b {
                        s.reset_setting(Target::Global, "adhesion_type");
                        t = s.global_value("adhesion_type").map(|v| v.py_str()).unwrap_or_default();
                        if t == "skirt" || t == "none" {
                            t = "brim".into();
                        }
                    }
                    s.set_setting(Target::Global, "adhesion_type", &t)?;
                }
                app.invalidate();
                Response::ok()
            }
            "/api/image_info" => match crate::qimage::load(&req.body) {
                Some(img) => Response::json(jobj(vec![("width", Json::Int(img.width as i64)), ("height", Json::Int(img.height as i64))])),
                None => return Err("The image is corrupt or in an unsupported format".into()),
            },
            "/api/models/upload" => {
                let name = req.q("name").unwrap_or("model.stl").to_string();
                let s = app.session_mut()?;
                if s.machine().is_none() {
                    return Err("Add a printer before loading models".into());
                }
                let ids = if crate::image_import::is_image_file(&name) {
                    let mut o = crate::image_import::ImageOptions::default();
                    let f = |k: &str| req.q(k).and_then(|v| v.parse::<f64>().ok());
                    if let Some(v) = f("size") {
                        o.size = v;
                    }
                    if let Some(v) = f("peak") {
                        o.peak_height = v;
                    }
                    if let Some(v) = f("base") {
                        o.base_height = v;
                    }
                    if let Some(v) = f("smoothing") {
                        o.smoothing = v.max(0.0) as usize;
                    }
                    if let Some(v) = f("transmittance") {
                        o.transmittance_1mm = v;
                    }
                    o.lighter_is_higher = req.q("lighter_is_higher") == Some("1");
                    o.use_transparency_model = req.q("transparency") == Some("1");
                    s.add_image_data(&name, &req.body, &o)?
                } else {
                    s.add_model_data(&name, &req.body)?
                };
                app.invalidate();
                Response::json(jobj(vec![("ids", Json::Array(ids.into_iter().map(|i| Json::Int(i as i64)).collect()))]))
            }
            "/api/model/transform" => {
                let id = jget_u(&body, "id").ok_or("id missing")? as u64;
                let s = app.session_mut()?;
                let auto_drop = s.prefs.auto_drop;
                if s.node_mut(id).ok_or("Unknown model")?.parent.is_some() {
                    // A support blocker: it can only be moved (in world space).
                    if let Some(Json::Array(p)) = body.get("position") {
                        let w = s.node_mut(id).unwrap().world();
                        let mut pos = [w[0][3], w[1][3], w[2][3]];
                        for (k, v) in p.iter().enumerate().take(3) {
                            if let Some(f) = v.as_f64() {
                                pos[k] = f;
                            }
                        }
                        s.move_child(id, pos);
                    }
                    app.invalidate();
                    return Ok(Response::ok());
                }
                let n = s.node_mut(id).ok_or("Unknown model")?;
                if let Some(Json::Array(p)) = body.get("position") {
                    for (k, v) in p.iter().enumerate().take(3) {
                        if let Some(f) = v.as_f64() {
                            n.position[k] = f;
                        }
                    }
                }
                if let Some(Json::Array(sc)) = body.get("scale") {
                    for (k, v) in sc.iter().enumerate().take(3) {
                        if let Some(f) = v.as_f64() {
                            if f.abs() > 1e-6 {
                                n.scale[k] = f;
                            }
                        }
                    }
                }
                if let (Some(axis), Some(deg)) = (jget_u(&body, "rotate_axis"), jget_f(&body, "rotate_degrees")) {
                    let (min, max) = n.world_bounds();
                    let centre = [(min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0, (min[2] + max[2]) / 2.0];
                    let r = crate::scene::rotation(axis.min(2), deg);
                    n.orientation = crate::scene::mul3(&r, &n.orientation);
                    // Rotate around the bounding box centre.
                    let (min2, max2) = n.world_bounds();
                    let c2 = [(min2[0] + max2[0]) / 2.0, (min2[1] + max2[1]) / 2.0, (min2[2] + max2[2]) / 2.0];
                    for k in 0..3 {
                        n.position[k] += centre[k] - c2[k];
                    }
                }
                if let Some(Json::Array(rows)) = body.get("orientation") {
                    // A full rotation matrix (lay flat).
                    let vals: Vec<f64> = rows.iter().filter_map(|v| v.as_f64()).collect();
                    if vals.len() == 9 {
                        let r = [[vals[0], vals[1], vals[2]], [vals[3], vals[4], vals[5]], [vals[6], vals[7], vals[8]]];
                        let (min, max) = n.world_bounds();
                        let centre = [(min[0] + max[0]) / 2.0, (min[2] + max[2]) / 2.0];
                        n.orientation = crate::scene::mul3(&r, &n.orientation);
                        let (min2, max2) = n.world_bounds();
                        n.position[0] += centre[0] - (min2[0] + max2[0]) / 2.0;
                        n.position[2] += centre[1] - (min2[2] + max2[2]) / 2.0;
                    }
                }
                if body.get("reset_rotation").and_then(|v| v.as_bool()).unwrap_or(false) {
                    n.orientation = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
                }
                if let Some(axis) = jget_u(&body, "mirror") {
                    let a = axis.min(2);
                    n.scale[a] = -n.scale[a];
                }
                if auto_drop || body.get("drop").and_then(|v| v.as_bool()).unwrap_or(false) {
                    n.drop_to_plate();
                }
                s.refresh_scene();
                app.invalidate();
                Response::ok()
            }
            "/api/model/support_blocker" => {
                // The support blocker tool: click a model to add a blocker, a blocker to remove it.
                let id = jget_u(&body, "id").ok_or("id missing")? as u64;
                let s = app.session_mut()?;
                let is_blocker = s.nodes.iter().find(|n| n.id == id).map(|n| n.parent.is_some() && n.settings.iter().any(|(k, v)| k == "anti_overhang_mesh" && v == "True")).unwrap_or(false);
                let r = if is_blocker {
                    s.remove_node(id);
                    jobj(vec![("removed", Json::Int(id as i64))])
                } else {
                    let p: Vec<f64> = match body.get("position") {
                        Some(Json::Array(a)) => a.iter().filter_map(|v| v.as_f64()).collect(),
                        _ => Vec::new(),
                    };
                    if p.len() != 3 {
                        return Err("position missing".into());
                    }
                    let nid = s.add_support_blocker(id, [p[0], p[1], p[2]])?;
                    jobj(vec![("added", Json::Int(nid as i64))])
                };
                app.invalidate();
                Response::json(r)
            }
            "/api/model/delete" => {
                let ids: Vec<u64> = match body.get("ids") {
                    Some(Json::Array(a)) => a.iter().filter_map(|v| v.as_i64()).map(|v| v as u64).collect(),
                    _ => jget_u(&body, "id").map(|v| vec![v as u64]).unwrap_or_default(),
                };
                let s = app.session_mut()?;
                for id in ids {
                    s.remove_node(id);
                }
                app.invalidate();
                Response::ok()
            }
            "/api/model/duplicate" => {
                let id = jget_u(&body, "id").ok_or("id missing")? as u64;
                let count = jget_u(&body, "count").unwrap_or(1).clamp(1, 100);
                app.session_mut()?.duplicate_node(id, count);
                app.invalidate();
                Response::ok()
            }
            "/api/model/extruder" => {
                let id = jget_u(&body, "id").ok_or("id missing")? as u64;
                let e = jget_u(&body, "extruder").unwrap_or(0);
                let s = app.session_mut()?;
                let count = s.extruder_count();
                if let Some(n) = s.node_mut(id) {
                    n.extruder = e.min(count - 1);
                }
                s.refresh_scene();
                app.invalidate();
                Response::ok()
            }
            "/api/model/setting" => {
                let id = jget_u(&body, "id").ok_or("id missing")? as u64;
                let key = jget_s(&body, "key").ok_or("key missing")?.to_string();
                let s = app.session_mut()?;
                if body.get("reset").and_then(|v| v.as_bool()).unwrap_or(false) {
                    s.reset_setting(Target::Object(id), &key);
                } else {
                    let mut value = body.get("value").map(|v| v.as_str().map(String::from).unwrap_or_else(|| json_plain(v))).unwrap_or_default();
                    if value.is_empty() {
                        // Newly added override: start from the value the model has now.
                        let o = s.nodes.iter().position(|n| n.id == id).ok_or("Unknown model")?;
                        value = s.machine().and_then(|m| m.get_value(StackId::Object(o), &key)).map(|v| v.py_str()).unwrap_or_default();
                    }
                    s.set_setting(Target::Object(id), &key, &value)?;
                }
                app.invalidate();
                Response::ok()
            }
            "/api/model/mesh_type" => {
                let id = jget_u(&body, "id").ok_or("id missing")? as u64;
                let t = jget_s(&body, "type").unwrap_or("normal").to_string();
                let s = app.session_mut()?;
                let n = s.node_mut(id).ok_or("Unknown model")?;
                n.settings.retain(|(k, _)| !["infill_mesh", "cutting_mesh", "support_mesh", "anti_overhang_mesh"].contains(&k.as_str()));
                if t != "normal" {
                    n.settings.push((t.clone(), "True".into()));
                    if t == "support_mesh" && !n.settings.iter().any(|(k, _)| k == "support_mesh_drop_down") {
                        n.settings.push(("support_mesh_drop_down".into(), "True".into()));
                    }
                }
                s.refresh_scene();
                app.invalidate();
                Response::ok()
            }
            "/api/models/clear" => {
                app.session_mut()?.clear_scene();
                app.invalidate();
                Response::ok()
            }
            "/api/models/arrange" => {
                app.session_mut()?.arrange_all();
                app.invalidate();
                Response::ok()
            }
            "/api/job_name" => {
                let n = jget_s(&body, "name").unwrap_or("").trim().to_string();
                let s = app.session_mut()?;
                s.user_job_name = if n.is_empty() { None } else { Some(n) };
                app.invalidate();
                Response::ok()
            }
            "/api/prefs" => {
                let auto_slice = body.get("auto_slice").and_then(|v| v.as_bool());
                let s = app.session_mut()?;
                if let Some(b) = body.get("jobname_prefix").and_then(|v| v.as_bool()) {
                    s.prefs.jobname_prefix = b;
                }
                if let Some(b) = body.get("auto_drop").and_then(|v| v.as_bool()) {
                    s.prefs.auto_drop = b;
                }
                if let Some(b) = auto_slice {
                    s.prefs.auto_slice = b;
                }
                if let Some(v) = jget_s(&body, "visibility_preset") {
                    s.prefs.visibility_preset = v.to_string();
                }
                if let Some(v) = jget_s(&body, "currency") {
                    s.prefs.currency = v.to_string();
                }
                if let Some(Json::Array(a)) = body.get("custom_visible") {
                    s.prefs.custom_visible = a.iter().filter_map(|v| v.as_str().map(String::from)).collect();
                }
                if let (Some(g), Some(c), Some(w)) = (jget_s(&body, "material_guid"), jget_f(&body, "spool_cost"), jget_f(&body, "spool_weight")) {
                    s.prefs.material_costs.retain(|(x, _, _)| x != g);
                    s.prefs.material_costs.push((g.to_string(), c, w));
                }
                persist::save(s);
                if let Some(b) = auto_slice {
                    app.auto_slice = b;
                }
                if app.result.is_some() {
                    // Costs / job name changes affect the final G-code.
                    if let Some((_, _, res)) = app.result.take() {
                        let (g, i) = slicing::final_gcode(app.s.as_ref().unwrap(), &res);
                        app.result = Some((g, i, res));
                        app.exported = None;
                    }
                }
                Response::ok()
            }
            "/api/visibility" => {
                // Toggle a setting's visibility in the custom preset.
                let key = jget_s(&body, "key").ok_or("key missing")?.to_string();
                let visible = body.get("visible").and_then(|v| v.as_bool()).unwrap_or(true);
                let s = app.session_mut()?;
                if s.prefs.visibility_preset != "custom" {
                    let base = visibility_set(s, &s.prefs.visibility_preset.clone()).unwrap_or_default();
                    let mut v: Vec<String> = base.into_iter().collect();
                    v.sort();
                    s.prefs.custom_visible = v;
                    s.prefs.visibility_preset = "custom".into();
                }
                s.prefs.custom_visible.retain(|k| *k != key);
                if visible {
                    s.prefs.custom_visible.push(key);
                }
                persist::save(s);
                Response::ok()
            }
            "/api/quit" => {
                if let Some(s) = &app.s {
                    persist::save(s);
                }
                std::process::exit(0);
            }
            _ => return Ok(Response::error(404, "Not found")),
        };
        Ok(resp)
    })();
    match r {
        Ok(resp) => resp,
        Err(e) => Response::error(400, &e),
    }
}

/// `json.dumps`-like plain text of a JSON scalar (numbers without quotes, booleans as Python).
fn json_plain(v: &Json) -> String {
    match v {
        Json::Bool(true) => "True".into(),
        Json::Bool(false) => "False".into(),
        Json::Int(i) => i.to_string(),
        Json::Float(f) => curust_settings::py::float_repr(*f),
        Json::Null => String::new(),
        other => other.to_string_compact(),
    }
}

fn default_output_dir() -> std::path::PathBuf {
    #[cfg(windows)]
    {
        if let Ok(p) = std::env::var("USERPROFILE") {
            return std::path::PathBuf::from(p).join("Documents");
        }
    }
    if let Ok(h) = std::env::var("HOME") {
        return std::path::PathBuf::from(h);
    }
    std::path::PathBuf::from(".")
}

// ---------------------------------------------------------------------------------------------
// Browser window
// ---------------------------------------------------------------------------------------------

fn find_browser() -> Option<std::path::PathBuf> {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("CURUST_BROWSER") {
        candidates.push(p.into());
    }
    #[cfg(windows)]
    {
        for var in ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"] {
            if let Ok(base) = std::env::var(var) {
                let b = std::path::PathBuf::from(base);
                candidates.push(b.join("Microsoft\\Edge\\Application\\msedge.exe"));
                candidates.push(b.join("Google\\Chrome\\Application\\chrome.exe"));
            }
        }
    }
    #[cfg(not(windows))]
    {
        for p in ["/usr/bin/microsoft-edge", "/usr/bin/google-chrome", "/usr/bin/chromium", "/usr/bin/chromium-browser", "/opt/pw-browsers/chromium"] {
            candidates.push(p.into());
        }
        #[cfg(target_os = "macos")]
        {
            candidates.push("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge".into());
            candidates.push("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".into());
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn open_window(url: &str) -> Option<std::process::Child> {
    if let Some(b) = find_browser() {
        let profile = crate::resources::config_dir().join("browser-profile");
        let _ = std::fs::create_dir_all(&profile);
        let child = std::process::Command::new(b)
            .arg(format!("--app={}", url))
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--window-size=1500,950")
            .spawn()
            .ok();
        if child.is_some() {
            return child;
        }
    }
    #[cfg(windows)]
    {
        crate::winview::open_in_default_browser(url);
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
    None
}

/// Show a folder in the system's file manager.
fn open_path(path: &str) {
    #[cfg(windows)]
    {
        crate::winview::open_in_default_browser(path);
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(path).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    }
}

pub fn run_gui(args: &[String]) {
    let mut port: u16 = 0;
    let mut open = true;
    // Show the interface in a browser instead of the application window (Windows).
    let mut use_browser = std::env::var("CURUST_USE_BROWSER").is_ok();
    let mut resources: Option<String> = None;
    let mut files = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--port" => {
                port = args.get(i + 1).and_then(|p| p.parse().ok()).unwrap_or(0);
                i += 1;
            }
            "--no-browser" => open = false,
            "--browser" => use_browser = true,
            "--resources" => {
                resources = args.get(i + 1).cloned();
                i += 1;
            }
            a if !a.starts_with("--") && std::path::Path::new(a).is_file() => files.push(std::path::PathBuf::from(a)),
            _ => {}
        }
        i += 1;
    }
    let (session, resource_error) = match crate::resources::find_resources(resources.as_deref()) {
        Ok(r) => {
            let mut s = Session::new(r);
            persist::load(&mut s);
            (Some(s), None)
        }
        Err(e) => (None, Some(e)),
    };
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            crate::fatal(&format!("Unable to start the interface server: {}", e));
        }
    };
    let addr = listener.local_addr().unwrap();
    let url = format!("http://127.0.0.1:{}/", addr.port());
    println!("Curust is running at {}", url);
    let _ = std::io::stdout().flush();
    let mut browser = None;
    // The native application window (Windows: WebView2); quitting follows it.
    #[allow(unused_mut)]
    let mut app_window: Option<std::sync::Arc<std::sync::atomic::AtomicBool>> = None;
    if open {
        #[cfg(windows)]
        if !use_browser {
            match crate::winview::open(&url, &crate::resources::config_dir().join("webview2")) {
                Ok(flag) => app_window = Some(flag),
                Err(e) => eprintln!("Application window unavailable ({}); using a browser", e),
            }
        }
        let _ = use_browser;
        if app_window.is_none() {
            browser = open_window(&url);
        }
    }
    let window_opened = open && app_window.is_none();
    listener.set_nonblocking(true).ok();
    let mut app = App { s: session, resource_error, slice: SliceState::Idle, result: None, layers_cache: None, generation: 0, auto_slice: false, last_request: Instant::now(), toasts: Vec::new(), pp_working: None, exported: None, usb: None, net: Default::default(), pending_files: files };
    app.auto_slice = app.s.as_ref().map(|s| s.prefs.auto_slice).unwrap_or(false);
    let mut last_auto_gen = u64::MAX;
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_nonblocking(false).ok();
                if let Some(req) = read_request(&mut stream) {
                    let resp = handle(&mut app, &req);
                    write_response(&mut stream, resp);
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(15));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
        app.poll_slice();
        if app.auto_slice && matches!(app.slice, SliceState::Idle) && app.generation != last_auto_gen && app.last_request.elapsed() > Duration::from_millis(600) {
            last_auto_gen = app.generation;
            if app.s.as_ref().map(|s| !s.nodes.is_empty()).unwrap_or(false) {
                let _ = app.start_slice();
            }
        }
        // Quit when the app window was closed: the browser process ended, or (when the window
        // belongs to an already running browser) the page stopped polling.
        let mut quit = false;
        if let Some(flag) = &app_window {
            if flag.load(std::sync::atomic::Ordering::SeqCst) {
                quit = true;
            }
        }
        if let Some(b) = browser.as_mut() {
            if let Ok(Some(_)) = b.try_wait() {
                browser = None;
                if app.last_request.elapsed() > Duration::from_secs(4) {
                    quit = true;
                }
            }
        }
        if window_opened && browser.is_none() && app.last_request.elapsed() > Duration::from_secs(25) && !matches!(app.slice, SliceState::Running(_)) {
            quit = true;
        }
        if quit {
            if let Some(s) = &app.s {
                persist::save(s);
            }
            break;
        }
    }
}
