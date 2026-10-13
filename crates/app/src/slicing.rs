//! Slicing: the checks and messages of Cura's `StartSliceJob`, running the engine (a child
//! process, like CuraEngine for Cura) and turning its output into the final G-code the way
//! `CuraEngineBackend`, `PrintInformation` and `GCodeWriter` do.

use crate::hull::{self, P2};
use crate::session::Session;
use curust_engine::communication::job::{JobObject, JobObjectList, SliceJob, SliceResult};
use curust_settings::py::{float_repr, Value};
use curust_settings::slice_message::{self, SceneObject, StartJobResult, TimeTokens};
use curust_settings::stack::{index, Container, Machine, StackId};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

pub struct SliceProblem {
    /// The StartSliceJob result code (kept for diagnostics).
    #[allow(dead_code)]
    pub result: StartJobResult,
    pub message: String,
}

fn problem(result: StartJobResult, message: impl Into<String>) -> SliceProblem {
    SliceProblem { result, message: message.into() }
}

/// `MachineErrorChecker`: settings with an error value in the global or enabled extruder stacks.
pub fn setting_errors(m: &Machine) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut stacks = vec![(StackId::Global, "global".to_string())];
    for i in m.extruder_list() {
        if m.extruders[i].is_enabled() {
            stacks.push((StackId::Extruder(i), format!("extruder {}", m.extruders[i].position().unwrap_or_default())));
        }
    }
    let def = m.global.definition();
    for (s, label) in stacks {
        for sd in def.defs.iter() {
            if sd.setting_type() == "category" {
                continue;
            }
            let key = &sd.key;
            if s != StackId::Global && !m.get_property(StackId::Global, key, "settable_per_extruder", None).map(|v| v.truthy()).unwrap_or(false) {
                continue;
            }
            if !m.get_property(s, key, "enabled", None).map(|v| v.truthy()).unwrap_or(false) {
                continue;
            }
            let state = m.validation_state(s, key);
            if state.is_error() {
                out.push((label.clone(), key.clone(), state.as_str().to_string()));
            }
        }
    }
    out
}

/// One-at-a-time print order (`OneAtATimeIterator` with `HitChecker`).
fn one_at_a_time_order(s: &Session, candidates: &[usize]) -> Vec<usize> {
    let Some(m) = s.machine() else { return Vec::new() };
    if candidates.len() < 2 {
        return candidates.to_vec();
    }
    let g = |k: &str| m.get_value(StackId::Global, k);
    let head: Vec<P2> = g("machine_head_with_fans_polygon")
        .and_then(|v| v.iter_items().ok())
        .unwrap_or_default()
        .iter()
        .filter_map(|p| {
            let c = p.iter_items().ok()?;
            Some([c.first()?.as_f64()?, c.get(1)?.as_f64()?])
        })
        .collect();
    let head = hull::convex_hull(&head);
    let (w, d, _) = s.machine_size();
    let max_len = 0.5 * w.min(d);
    let adhesion_margin = |o: usize| -> f64 {
        let v = |k: &str| m.get_value(StackId::Object(o), k).and_then(|v| v.as_f64()).unwrap_or(0.0);
        match m.get_value(StackId::Object(o), "adhesion_type").map(|v| v.py_str()).unwrap_or_default().as_str() {
            "raft" => v("raft_margin").max(0.0).min(max_len),
            "brim" => (v("brim_line_count") * v("skirt_brim_line_width")).max(0.0).min(max_len),
            "skirt" => (v("skirt_gap") + v("skirt_line_count") * v("skirt_brim_line_width")).max(0.0).min(max_len),
            _ => 0.0,
        }
    };
    let hulls: Vec<Vec<P2>> = candidates.iter().map(|&i| crate::buildvolume::node_hull(&s.nodes[i])).collect();
    let adhesion: Vec<Vec<P2>> = candidates
        .iter()
        .zip(hulls.iter())
        .map(|(&i, h)| {
            let e = adhesion_margin(i);
            if e > 0.0 {
                hull::minkowski(h, &hull::approximated_circle(e))
            } else {
                h.clone()
            }
        })
        .collect();
    let head_full: Vec<Vec<P2>> = adhesion.iter().map(|a| hull::minkowski(a, &head)).collect();
    let n = candidates.len();
    // hit[a][b]: a cannot be printed before b.
    let mut hit = vec![vec![false; n]; n];
    for a in 0..n {
        for b in 0..n {
            if a != b {
                hit[a][b] = hull::intersects(&hulls[a], &head_full[b]) || hull::intersects(&adhesion[a], &adhesion[b]);
            }
        }
    }
    for a in 0..n {
        for b in 0..n {
            if hit[a][b] && hit[b][a] {
                return Vec::new();
            }
        }
    }
    let mut nodes: Vec<usize> = (0..n).collect();
    let score = |a: usize| hit[a].iter().filter(|&&h| h).count() as i64;
    nodes.sort_by_key(|&a| score(a));
    let mut todo: Vec<(Vec<usize>, Vec<usize>)> = vec![(Vec::new(), nodes)];
    let mut iterations = 0;
    while let Some((order, rest)) = todo.pop() {
        iterations += 1;
        if iterations > 100_000 {
            break;
        }
        for &node in &rest {
            let after_ok = order.iter().all(|&o| !hit[o][node]);
            let before_ok = rest.iter().all(|&o| !hit[node][o]);
            if after_ok && before_ok {
                let mut new_rest = rest.clone();
                new_rest.retain(|&x| x != node);
                let mut new_order = order.clone();
                new_order.push(node);
                if new_rest.is_empty() {
                    return new_order.into_iter().map(|k| candidates[k]).collect();
                }
                todo.push((new_order, new_rest));
            }
        }
    }
    Vec::new()
}

/// Build the slice job (`StartSliceJob.run`).
pub fn prepare(s: &Session, time: &TimeTokens) -> Result<SliceJob, SliceProblem> {
    let m = s.machine().ok_or_else(|| problem(StartJobResult::Error, "No printer has been added"))?;
    let errors = setting_errors(m);
    if !errors.is_empty() {
        let list: Vec<String> = errors.iter().take(8).map(|(st, k, e)| format!("{} ({}: {})", k, st, e)).collect();
        return Err(problem(StartJobResult::SettingError, format!("Unable to slice due to some setting values: {}", list.join(", "))));
    }
    if !slice_message::check_materials(m) {
        return Err(problem(StartJobResult::MaterialIncompatible, "The selected material is incompatible with the selected nozzle"));
    }
    for (o, n) in s.nodes.iter().enumerate() {
        if slice_message::object_has_errors(m, o) {
            return Err(problem(StartJobResult::ObjectSettingError, format!("Unable to slice due to per-model settings of {}", n.name)));
        }
    }
    let non_printing: Vec<bool> = (0..s.nodes.len()).map(|o| slice_message::is_non_printing_mesh(m, o)).collect();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let order = s.dfs_order();
    if m.get_value(StackId::Global, "print_sequence").map(|v| v.py_str()) == Some("one_at_a_time".into()) {
        let modifiers: Vec<usize> = order.iter().copied().filter(|&i| non_printing[i]).collect();
        let list = m.extruder_list();
        let candidates: Vec<usize> = (0..s.nodes.len())
            .filter(|&i| s.nodes[i].parent.is_none() && !non_printing[i] && !s.nodes[i].outside && list.get(s.nodes[i].extruder).map(|&e| m.extruders[e].is_enabled()).unwrap_or(false))
            .collect();
        for i in one_at_a_time_order(s, &candidates) {
            // `node.getAllChildren() + [node]`, then all modifier meshes.
            let mut g = s.all_children(i);
            g.push(i);
            g.extend(modifiers.iter().copied());
            groups.push(g);
        }
    } else {
        let mut list = Vec::new();
        let mut has_printing = false;
        for &i in &order {
            if s.nodes[i].outside && !non_printing[i] {
                continue;
            }
            list.push(i);
            if !non_printing[i] {
                has_printing = true;
            }
        }
        if has_printing {
            groups.push(list);
        }
    }
    // Models assigned to disabled extruders.
    let enabled: Vec<bool> = m.extruder_list().iter().map(|&i| m.extruders[i].is_enabled()).collect();
    let mut disabled = Vec::new();
    for g in &groups {
        for &i in g {
            if non_printing[i] {
                continue;
            }
            let pos: usize = slice_message::active_extruder_position(m, i).parse().unwrap_or(0);
            if !enabled.get(pos).copied().unwrap_or(false) {
                disabled.push(pos);
            }
        }
    }
    if !disabled.is_empty() {
        disabled.sort();
        disabled.dedup();
        return Err(problem(StartJobResult::ObjectsWithDisabledExtruder, format!("Models are assigned to disabled extruder(s) {:?}", disabled)));
    }
    if groups.is_empty() {
        return Err(problem(StartJobResult::NothingToSlice, "Nothing to slice: there are no models inside the build volume"));
    }
    let objects: Vec<SceneObject> = (0..s.nodes.len()).map(|i| SceneObject { stack: i, painted_extruders: None }).collect();
    let settings = slice_message::build_slice_settings(m, &objects, time);
    let mut job = SliceJob {
        global_settings: settings.global_settings.clone(),
        extruders: settings.extruders.iter().map(|(id, st)| (*id as i32, st.clone())).collect(),
        limit_to_extruder: settings.limit_to_extruder.iter().map(|(k, e)| (k.clone(), *e as i32)).collect(),
        object_lists: Vec::new(),
    };
    for g in groups {
        let mut ol = JobObjectList::default();
        for i in g {
            let n = &s.nodes[i];
            ol.objects.push(JobObject { name: n.name.clone(), vertices: n.engine_vertices(), uv_coordinates: Vec::new(), texture: Vec::new(), settings: settings.object_settings[i].clone() });
        }
        job.object_lists.push(ol);
    }
    Ok(job)
}

// ---------------------------------------------------------------------------------------------
// Engine process
// ---------------------------------------------------------------------------------------------

pub struct EngineRun {
    child: Child,
    pub progress: Arc<Mutex<f64>>,
    pub log: Arc<Mutex<Vec<String>>>,
    result_path: PathBuf,
    job_path: PathBuf,
    pub started: std::time::Instant,
}

fn engine_executable() -> PathBuf {
    if let Ok(p) = std::env::var("CURUST_ENGINE") {
        return PathBuf::from(p);
    }
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("curust"))
}

fn temp_dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("curust-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&d);
    d
}

impl EngineRun {
    pub fn start(job: &SliceJob) -> Result<EngineRun, String> {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = temp_dir();
        let job_path = dir.join(format!("job{}.bin", n));
        let result_path = dir.join(format!("result{}.bin", n));
        let _ = std::fs::remove_file(&result_path);
        std::fs::write(&job_path, job.encode()).map_err(|e| format!("Unable to write the slice job: {}", e))?;
        let mut cmd = Command::new(engine_executable());
        cmd.arg("engine-job").arg(&job_path).arg(&result_path).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        if std::env::var("CURAENGINE_LOG_LEVEL").is_err() {
            cmd.env("CURAENGINE_LOG_LEVEL", "warn");
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = cmd.spawn().map_err(|e| format!("Unable to start the slicing engine: {}", e))?;
        let progress = Arc::new(Mutex::new(0.0));
        let log = Arc::new(Mutex::new(Vec::new()));
        if let Some(out) = child.stdout.take() {
            let p = progress.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    if let Some(v) = line.strip_prefix("PROGRESS ") {
                        if let Ok(f) = v.trim().parse::<f64>() {
                            *p.lock().unwrap() = f;
                        }
                    }
                }
            });
        }
        if let Some(err) = child.stderr.take() {
            let l = log.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    let mut l = l.lock().unwrap();
                    if l.len() < 2000 {
                        l.push(line);
                    }
                }
            });
        }
        Ok(EngineRun { child, progress, log, result_path, job_path, started: std::time::Instant::now() })
    }

    /// `None` while running; the result once the engine has exited.
    pub fn poll(&mut self) -> Option<Result<SliceResult, String>> {
        match self.child.try_wait() {
            Ok(None) => None,
            Ok(Some(status)) => Some(self.finish(status.success())),
            Err(e) => Some(Err(e.to_string())),
        }
    }

    pub fn wait(mut self) -> Result<SliceResult, String> {
        let status = self.child.wait().map_err(|e| e.to_string())?;
        self.finish(status.success())
    }

    fn finish(&mut self, success: bool) -> Result<SliceResult, String> {
        // Give the reader threads a moment to drain the pipes.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let data = std::fs::read(&self.result_path);
        let _ = std::fs::remove_file(&self.job_path);
        let _ = std::fs::remove_file(&self.result_path);
        match data {
            Ok(d) if success => SliceResult::decode(&d),
            _ => {
                let log = self.log.lock().unwrap();
                let tail: Vec<&String> = log.iter().rev().take(12).collect();
                let tail: Vec<String> = tail.into_iter().rev().cloned().collect();
                Err(format!("The slicing engine failed.\n{}", tail.join("\n")))
            }
        }
    }

    pub fn cancel(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.job_path);
        let _ = std::fs::remove_file(&self.result_path);
    }
}

// ---------------------------------------------------------------------------------------------
// Print information and final G-code
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct PrintInfo {
    /// Seconds, after the machine's time estimation factor.
    pub print_time: i64,
    pub print_time_iso: String,
    pub feature_times: Vec<(String, f64)>,
    pub material_lengths: Vec<f64>,
    pub material_weights: Vec<f64>,
    pub material_costs: Vec<f64>,
    pub material_names: Vec<String>,
    pub layer_count: usize,
}

/// Python `str()` of a list of floats.
fn py_float_list(v: &[f64]) -> String {
    let items: Vec<String> = v.iter().map(|f| float_repr(*f)).collect();
    format!("[{}]", items.join(", "))
}

/// Python `round(x)` (half to even) as an integer.
fn round_half_even(x: f64) -> i64 {
    let r = x.round();
    if (x - x.trunc()).abs() == 0.5 {
        (2.0 * (x / 2.0).round()) as i64
    } else {
        r as i64
    }
}

/// `PrintInformation._updateTotalPrintTimePerFeature` + `_calculateInformation`.
pub fn print_info(s: &Session, res: &SliceResult) -> PrintInfo {
    let mut info = PrintInfo::default();
    let Some(m) = s.machine() else { return info };
    let est = res.time_estimates();
    let get = |k: &str| est.iter().find(|(n, _)| *n == k).map(|(_, v)| *v as f32 as f64).unwrap_or(0.0);
    let factor = m.get_value(StackId::Global, "machine_time_estimation_factor").and_then(|v| v.as_f64()).map(|f| f / 100.0).filter(|f| (f - 1.0).abs() > 1e-6);
    let mut total = 0.0f64;
    for (name, key) in [
        ("inset_0", "time_inset_0"),
        ("inset_x", "time_inset_x"),
        ("skin", "time_skin"),
        ("infill", "time_infill"),
        ("support_infill", "time_support_infill"),
        ("support_interface", "time_support_interface"),
        ("support", "time_support"),
        ("skirt", "time_skirt"),
        ("prime_tower", "time_prime_tower"),
        ("travel", "time_travel"),
        ("retract", "time_retract"),
        ("none", "time_none"),
    ] {
        let mut t = get(key);
        if t.is_nan() {
            info.feature_times.push((name.into(), 0.0));
            continue;
        }
        if let Some(f) = factor {
            t = (t * f).trunc();
        }
        total += t;
        info.feature_times.push((name.into(), t));
    }
    let secs = if total < 0.0 || !total.is_finite() { 0 } else { round_half_even(total) };
    let secs = if secs >= 1 << 31 { 0 } else { secs };
    info.print_time = secs;
    let days = secs / 86400;
    let rem = secs - days * 86400;
    let hours = rem / 3600;
    let minutes = (rem - hours * 3600) / 60;
    let seconds = rem - hours * 3600 - minutes * 60;
    info.print_time_iso = format!("{:02}:{:02}:{:02}", days * 24 + hours, minutes, seconds);
    let amounts: Vec<f64> = res.material_amounts.iter().map(|a| a.map(|v| v as f32 as f64).filter(|v| !v.is_nan()).unwrap_or(0.0)).collect();
    for (index, &i) in m.extruder_list().iter().enumerate() {
        if index >= amounts.len() {
            continue;
        }
        let amount = amounts[index];
        let mat = m.extruders[i].container(index::MATERIAL);
        let props = match mat {
            Container::Instance(c) => c.meta("properties").cloned(),
            _ => None,
        }
        .or_else(|| m.reg.metadata(mat.id()).and_then(|e| e.get("properties").cloned()));
        let prop = |k: &str| -> f64 {
            match &props {
                Some(Value::Dict(d)) => d.iter().find(|(kk, _)| kk.py_str() == k).and_then(|(_, v)| v.as_f64().or_else(|| v.py_str().parse().ok())).unwrap_or(0.0),
                _ => 0.0,
            }
        };
        let density = prop("density");
        let radius = m.get_value(StackId::Extruder(i), "material_diameter").and_then(|v| v.as_f64()).unwrap_or(0.0) / 2.0;
        let weight = amount * density / 1000.0;
        let guid = mat.meta_str("GUID").unwrap_or_default();
        let mut cost = 0.0;
        if let Some((_, spool_cost, spool_weight)) = s.prefs.material_costs.iter().find(|(g, _, _)| *g == guid) {
            let wps = if *spool_weight != 0.0 { *spool_weight } else { prop("weight") };
            if wps != 0.0 {
                cost = spool_cost * weight / wps;
            }
        }
        let length = if radius != 0.0 {
            let l = (amount / (std::f64::consts::PI * radius.powi(2))) / 1000.0;
            curust_settings::py::builtins::py_round(&Value::Float(l), Some(&Value::Int(2))).ok().and_then(|v| v.as_f64()).unwrap_or(l)
        } else {
            0.0
        };
        info.material_weights.push(weight);
        info.material_lengths.push(length);
        info.material_costs.push(cost);
        info.material_names.push(mat.name());
    }
    info.layer_count = res.layers.iter().filter(|l| l.id >= 0).count();
    info
}

/// `_onSlicingFinishedMessage` token replacement.
fn replace_tokens(text: &str, info: &PrintInfo, job_name: &str) -> String {
    text.replace("{print_time}", &info.print_time_iso)
        .replace("{filament_amount}", &py_float_list(&info.material_lengths))
        .replace("{filament_weight}", &py_float_list(&info.material_weights))
        .replace("{filament_cost}", &py_float_list(&info.material_costs))
        .replace("{jobname}", job_name)
}

/// ConfigParser's `write` of an instance container (`InstanceContainer.serialize`).
fn as_refs(meta: &[(String, String)]) -> Vec<(&str, String)> {
    meta.iter().map(|(k, v)| (k.as_str(), v.clone())).collect()
}

fn serialize_container(name: &str, definition: &str, metadata: &[(&str, String)], values: &[(String, String)]) -> String {
    let mut out = String::new();
    out.push_str("[general]\n");
    out.push_str("version = 4\n");
    out.push_str(&format!("name = {}\n", name.replace('\n', "\n\t")));
    out.push_str(&format!("definition = {}\n\n", definition));
    out.push_str("[metadata]\n");
    for (k, v) in metadata {
        out.push_str(&format!("{} = {}\n", k.to_lowercase(), v.replace('\n', "\n\t")));
    }
    out.push('\n');
    out.push_str("[values]\n");
    for (k, v) in values {
        out.push_str(&format!("{} = {}\n", k.to_lowercase(), v.replace('\n', "\n\t")));
    }
    out.push('\n');
    out
}

/// `str(instance.value)` for every key of user ∪ quality-changes containers, sorted.
fn merged_values(m: &Machine, s: StackId) -> Vec<(String, String)> {
    let st = m.stack(s);
    let mut map: Vec<(String, String)> = Vec::new();
    for ci in [index::QUALITY_CHANGES, index::USER] {
        let c = st.instance_at(ci);
        if c.is_empty {
            continue;
        }
        let def = m.reg.definition(&c.definition);
        for k in c.keys() {
            let v = match c.get_value(&k, def.as_deref()) {
                Some(curust_settings::definition::Prop::Value(v)) => v.py_str(),
                Some(curust_settings::definition::Prop::Func(f)) => f.to_string(),
                None => continue,
            };
            if let Some(slot) = map.iter_mut().find(|(kk, _)| *kk == k) {
                slot.1 = v;
            } else {
                map.push((k, v));
            }
        }
    }
    map.sort();
    map
}

/// Python `json.dumps` of a string (ensure_ascii).
fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{:04x}", u));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `GCodeWriter._serialiseSettings`.
pub fn settings_footer(s: &Session) -> String {
    let Some(m) = s.machine() else { return String::new() };
    let quality = m.global.container(index::QUALITY);
    let quality_type = quality.meta_str("quality_type").unwrap_or_else(|| "None".into());
    let quality_definition = s.tree.machine(&m.reg, &m.global.definition().id).quality_definition.clone();
    let setting_version = quality.meta_str("setting_version").unwrap_or_else(|| "None".into());
    // The metadata of the merged container: a copy of the quality-changes container's own
    // metadata (in insertion order) or, without one, the container GCodeWriter makes up.
    let qc_meta = |qc: &Container| -> Vec<(String, String)> {
        let Container::Instance(qc) = qc else { return Vec::new() };
        qc.metadata
            .iter()
            .filter(|(k, _)| !matches!(k.as_str(), "id" | "version" | "name" | "container_type" | "definition"))
            .map(|(k, v)| (k.clone(), v.py_str()))
            .collect()
    };
    let ensure = |meta: &mut Vec<(String, String)>, key: &str, value: String| {
        if !meta.iter().any(|(k, _)| k == key) {
            meta.push((key.to_string(), value));
        }
    };
    let qc = m.global.container(index::QUALITY_CHANGES);
    let (name, mut gmeta) = if qc.id() == "empty_quality_changes" {
        (
            m.reg.unique_name(&quality.name()),
            vec![("type".to_string(), "quality_changes".to_string()), ("quality_type".to_string(), quality_type.clone()), ("setting_version".to_string(), setting_version.clone())],
        )
    } else {
        (qc.name(), qc_meta(&qc))
    };
    ensure(&mut gmeta, "type", "quality_changes".into());
    ensure(&mut gmeta, "quality_type", quality.meta_str("quality_type").unwrap_or_else(|| "normal".into()));
    // `serialize` takes the definition from the container's cached definition object, which is
    // the machine definition (from the user container) once any value was set, and otherwise
    // looked up from the "definition" metadata (the quality definition).
    let machine_def = m.global.definition().id.clone();
    let def_for = |values: &Vec<(String, String)>| if values.is_empty() { quality_definition.clone() } else { machine_def.clone() };
    let global_values = merged_values(m, StackId::Global);
    let mut any = !global_values.is_empty();
    let global = serialize_container(&name, &def_for(&global_values), &as_refs(&gmeta), &global_values);
    let mut extruders = Vec::new();
    for i in m.extruder_list() {
        let values = merged_values(m, StackId::Extruder(i));
        any |= !values.is_empty();
        let eqc = m.extruders[i].container(index::QUALITY_CHANGES);
        let (ename, mut emeta) = if eqc.id() == "empty_quality_changes" {
            (
                m.reg.unique_name(&quality.name()),
                vec![("type".to_string(), "quality_changes".to_string()), ("quality_type".to_string(), quality_type.clone()), ("setting_version".to_string(), setting_version.clone())],
            )
        } else {
            (eqc.name(), qc_meta(&eqc))
        };
        ensure(&mut emeta, "type", "quality_changes".into());
        ensure(&mut emeta, "position", m.extruders[i].position().unwrap_or_default());
        let eq = m.extruders[i].container(index::QUALITY);
        ensure(&mut emeta, "quality_type", eq.meta_str("quality_type").unwrap_or_else(|| "normal".into()));
        extruders.push(serialize_container(&ename, &def_for(&values), &as_refs(&emeta), &values));
    }
    if !any {
        return String::new();
    }
    let mut json = format!("{{\"global_quality\": {}", json_str(&global));
    if !extruders.is_empty() {
        let items: Vec<String> = extruders.iter().map(|e| json_str(e)).collect();
        json.push_str(&format!(", \"extruder_quality\": [{}]", items.join(", ")));
    }
    json.push('}');
    let escaped = json.replace('\\', "\\\\").replace('\n', "\\n").replace('\r', "\\r");
    let prefix = ";SETTING_3 ";
    let chunk = 80 - prefix.len();
    let chars: Vec<char> = escaped.chars().collect();
    let mut out = String::new();
    for c in chars.chunks(chunk) {
        out.push_str(prefix);
        out.extend(c.iter());
        out.push('\n');
    }
    out
}

/// Cura's `scene.gcode_dict[plate]`: the prefix message followed by every G-code message the
/// engine sent, with the time/material tokens replaced (`_onSlicingFinishedMessage`).
pub fn gcode_list(s: &Session, res: &SliceResult) -> (Vec<String>, PrintInfo) {
    let info = print_info(s, res);
    let job = s.job_name();
    let mut list = vec![replace_tokens(&res.gcode_prefix, &info, &job)];
    let total: usize = res.gcode_chunks.iter().map(|&c| c as usize).sum();
    if res.gcode_chunks.is_empty() || total != res.gcode.len() {
        if !res.gcode.is_empty() {
            list.push(replace_tokens(&res.gcode, &info, &job));
        }
    } else {
        let mut pos = 0;
        for &c in &res.gcode_chunks {
            let c = c as usize;
            list.push(replace_tokens(&res.gcode[pos..pos + c], &info, &job));
            pos += c;
        }
    }
    (list, info)
}

/// `GCodeWriter.write`: the chunks, then the settings unless a chunk starts with ";SETTING_".
pub fn write_gcode_list(s: &Session, list: &[String]) -> String {
    let mut out = String::with_capacity(list.iter().map(|c| c.len()).sum::<usize>() + 4096);
    let mut has_settings = false;
    for c in list {
        if c.starts_with(";SETTING_") {
            has_settings = true;
        }
        out.push_str(c);
    }
    if !has_settings {
        out.push_str(&settings_footer(s));
    }
    out
}

/// The G-code file Cura would save for this slice result (without post-processing).
pub fn final_gcode(s: &Session, res: &SliceResult) -> (String, PrintInfo) {
    let (list, info) = gcode_list(s, res);
    (write_gcode_list(s, &list), info)
}

/// Extruders that print something in this slice.
pub fn used_extruders(s: &Session, res: &SliceResult) -> Vec<usize> {
    let n = s.machine().map(|m| m.extruder_list().len()).unwrap_or(1);
    let mut used: Vec<usize> = res.material_amounts.iter().enumerate().filter(|(_, a)| a.is_some_and(|v| v > 0.0)).map(|(i, _)| i).filter(|&i| i < n).collect();
    if used.is_empty() {
        used.push(0);
    }
    used
}

/// The G-code file as saved, after the post-processing scripts ran (`writeStarted`).
pub fn export_gcode(s: &Session, res: &SliceResult) -> (String, PrintInfo, crate::postproc::PostResult) {
    let (list, info) = gcode_list(s, res);
    let ctx = crate::postproc::RunContext { print_info: Some(info.clone()), used_extruders: used_extruders(s, res), snapshot: crate::snapshot::snapshot_fn(s) };
    let pp = crate::postproc::post_process(s, ctx, list);
    (write_gcode_list(s, &pp.gcode), info, pp)
}

/// Run a complete slice synchronously (command line use).
pub fn slice_blocking(s: &Session, time: &TimeTokens) -> Result<(String, PrintInfo, SliceResult), String> {
    let job = prepare(s, time).map_err(|p| p.message)?;
    if let Ok(p) = std::env::var("CURUST_DUMP_JOB") {
        let _ = std::fs::write(p, job.encode());
    }
    if let Ok(dir) = std::env::var("CURUST_DUMP_VERTICES") {
        // Testing aid: the vertices of every object as raw float32.
        for (g, ol) in job.object_lists.iter().enumerate() {
            for (k, o) in ol.objects.iter().enumerate() {
                let b: Vec<u8> = o.vertices.iter().flat_map(|v| v.to_le_bytes()).collect();
                let _ = std::fs::write(std::path::Path::new(&dir).join(format!("{}_{}.f32", g, k)), b);
            }
        }
    }
    let res = EngineRun::start(&job)?.wait()?;
    if let Ok(p) = std::env::var("CURUST_DUMP_CHUNKS") {
        let (list, _) = gcode_list(s, &res);
        let lens: Vec<String> = list.iter().map(|c| c.len().to_string()).collect();
        let _ = std::fs::write(p, format!("[{}]", lens.join(", ")));
    }
    let (gcode, info, pp) = export_gcode(s, &res);
    for l in pp.log.iter().filter(|l| !l.starts_with("[d]")) {
        eprintln!("post-processing: {}", l);
    }
    for (t, m) in &pp.messages {
        eprintln!("message: {}: {}", t, m);
    }
    Ok((gcode, info, res))
}
