//! The settings part of Cura's `StartSliceJob`: everything the front-end sends to CuraEngine
//! besides the mesh data (global settings with expanded start/end G-code, `limit_to_extruder`,
//! per-extruder settings and per-object settings), plus `ExtruderManager.getInitialExtruderNr`.

use crate::definition::Prop;
use crate::instance::InstValue;
use crate::py::{Function, Value};
use crate::stack::{index, parse_bool, Container, Machine, StackId};
use std::collections::{HashMap, HashSet};

/// `StartJobResult`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartJobResult {
    Finished = 1,
    Error = 2,
    SettingError = 3,
    NothingToSlice = 4,
    MaterialIncompatible = 5,
    BuildPlateError = 6,
    ObjectSettingError = 7,
    ObjectsWithDisabledExtruder = 8,
}

/// A model in the scene, as far as the settings are concerned.
#[derive(Clone, Debug)]
pub struct SceneObject {
    /// Index into `Machine::object_stacks`.
    pub stack: usize,
    /// Extruders used by texture painting (`getPaintedExtruders`), if any.
    pub painted_extruders: Option<Vec<i64>>,
}

/// The fixed time tokens (`{time}`, `{date}`, `{day}`).
#[derive(Clone, Debug)]
pub struct TimeTokens {
    pub time: String,
    pub date: String,
    /// `%w`: 0 = Sunday.
    pub weekday: usize,
}

impl TimeTokens {
    /// The current local time (UTC when the local offset is unknown).
    pub fn now() -> TimeTokens {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0) + local_offset_seconds();
        let days = secs.div_euclid(86400);
        let rem = secs.rem_euclid(86400);
        let (y, m, d) = civil_from_days(days);
        TimeTokens {
            time: format!("{:02}:{:02}:{:02}", rem / 3600, (rem / 60) % 60, rem % 60),
            date: format!("{:02}-{:02}-{:04}", d, m, y),
            weekday: (days + 4).rem_euclid(7) as usize,
        }
    }
    /// The tokens the reference comparisons use.
    pub fn fixed() -> TimeTokens {
        TimeTokens { time: "12:34:56".into(), date: "01-02-2026".into(), weekday: 1 }
    }
}

fn local_offset_seconds() -> i64 {
    std::env::var("CURUST_UTC_OFFSET_MINUTES").ok().and_then(|s| s.parse::<i64>().ok()).map(|m| m * 60).unwrap_or_else(platform_offset)
}

#[cfg(windows)]
fn platform_offset() -> i64 {
    #[repr(C)]
    struct SystemTime {
        w: [u16; 8],
    }
    #[repr(C)]
    struct TimeZoneInformation {
        bias: i32,
        standard_name: [u16; 32],
        standard_date: SystemTime,
        standard_bias: i32,
        daylight_name: [u16; 32],
        daylight_date: SystemTime,
        daylight_bias: i32,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetTimeZoneInformation(tz: *mut TimeZoneInformation) -> u32;
    }
    unsafe {
        let mut tz: TimeZoneInformation = std::mem::zeroed();
        let r = GetTimeZoneInformation(&mut tz);
        let bias = tz.bias + if r == 2 { tz.daylight_bias } else { 0 };
        -(bias as i64) * 60
    }
}

#[cfg(not(windows))]
fn platform_offset() -> i64 {
    // `date +%z` style offset from /etc/localtime is not worth parsing here; use TZ-less UTC.
    0
}

/// Days since 1970-01-01 → (year, month, day).
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The settings messages of a slice.
#[derive(Clone, Debug, Default)]
pub struct SliceSettings {
    pub global_settings: Vec<(String, String)>,
    pub limit_to_extruder: Vec<(String, i64)>,
    /// (extruder message id = position, settings).
    pub extruders: Vec<(i64, Vec<(String, String)>)>,
    /// Per-object settings, parallel to the `objects` given to [`build_slice_settings`].
    pub object_settings: Vec<Vec<(String, String)>>,
}

type Tokens = HashMap<String, Value>;

/// Metadata entry of a container, including the implicit `id` and `name` entries.
fn container_meta(c: &Container, key: &str) -> Option<Value> {
    match c {
        Container::Instance(i) => {
            if let Some(v) = i.meta(key) {
                return Some(v.clone());
            }
            match key {
                "name" => Some(Value::str(&i.name)),
                "id" => Some(Value::str(&i.id)),
                "definition" => Some(Value::str(&i.definition)),
                _ => None,
            }
        }
        Container::Definition(d) => match key {
            "name" => Some(Value::str(&d.name)),
            "id" => Some(Value::str(&d.id)),
            _ => d.metadata.get(key).map(crate::definition::json_to_value),
        },
    }
}

fn meta_or(c: &Container, key: &str, default: &str) -> Value {
    container_meta(c, key).unwrap_or_else(|| Value::str(default))
}

/// `ContainerStack.getMetaDataEntry` of the global stack (own metadata, then its containers).
pub fn global_meta(m: &Machine, key: &str) -> Option<Value> {
    if let Some(v) = m.global.meta(key) {
        return Some(v.clone());
    }
    for c in &m.global.containers {
        if let Some(v) = container_meta(c, key) {
            if !v.is_none() {
                return Some(v);
            }
        }
    }
    None
}

/// Raw value of a per-object user setting, as `InstanceContainer.getProperty` returns it, made
/// truthy the way Python's `bool()` would see it.
fn raw_user_truthy(m: &Machine, o: usize, key: &str) -> bool {
    match m.object_stacks[o].user.raw_value(key) {
        None => false,
        Some(InstValue::Typed(v)) => v.truthy(),
        Some(InstValue::Func(_)) => true,
        Some(InstValue::Str(s)) => !s.is_empty(),
    }
}

pub fn is_anti_overhang_mesh(m: &Machine, o: usize) -> bool {
    raw_user_truthy(m, o, "anti_overhang_mesh")
}
pub fn is_support_mesh(m: &Machine, o: usize) -> bool {
    raw_user_truthy(m, o, "support_mesh")
}
/// `SettingOverrideDecorator._evaluateIsNonPrintingMesh`.
pub fn is_non_printing_mesh(m: &Machine, o: usize) -> bool {
    ["anti_overhang_mesh", "infill_mesh", "cutting_mesh"].iter().any(|k| m.get_property(StackId::Object(o), k, "value", None).map(|v| v.truthy()).unwrap_or(false))
}

/// `SettingOverrideDecorator.getActiveExtruderPosition`.
pub fn active_extruder_position(m: &Machine, o: usize) -> String {
    if is_support_mesh(m, o) {
        return m.get_value(StackId::Global, "support_extruder_nr").map(|v| v.py_str()).unwrap_or_else(|| "None".into());
    }
    m.extruders[m.object_stacks[o].extruder_position].position().unwrap_or_else(|| "None".into())
}

/// `ExtruderManager.extruderIds`: position → extruder index (for the extruders in `extruderList`).
fn extruder_ids(m: &Machine) -> Vec<(String, usize)> {
    m.extruder_list().into_iter().map(|i| (m.extruders[i].position().unwrap_or_default(), i)).collect()
}

fn ids_get(ids: &[(String, usize)], pos: &str) -> Option<usize> {
    ids.iter().find(|(p, _)| p == pos).map(|(_, i)| *i)
}

fn as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Int(i) => Some(*i),
        Value::Bool(b) => Some(*b as i64),
        Value::Float(f) => Some(f.trunc() as i64),
        Value::Str(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// `ExtruderManager.getUsedExtruderStacks` (as extruder indices; unordered like the Python set).
pub fn used_extruders(m: &Machine, objects: &[SceneObject]) -> Vec<usize> {
    let ids = extruder_ids(m);
    let mut used: Vec<usize> = Vec::new();
    let add = |used: &mut Vec<usize>, i: usize| {
        if !used.contains(&i) {
            used.push(i);
        }
    };
    if ids.is_empty() {
        return used;
    }
    let number_active = m.extruder_list().into_iter().filter(|&i| m.extruders[i].is_enabled()).count();
    let mut support_enabled = false;
    let mut support_bottom = false;
    let mut support_roof = false;
    for obj in objects {
        let o = obj.stack;
        if is_anti_overhang_mesh(m, o) || is_support_mesh(m, o) {
            continue;
        }
        add(&mut used, m.object_stacks[o].extruder_position);
        if used.len() == number_active {
            break;
        }
        let s = StackId::Object(o);
        let b = |k: &str| m.get_property(s, k, "value", None).map(|v| parse_bool(&v)).unwrap_or(false);
        if !support_enabled {
            support_enabled |= b("support_enable");
        }
        if !support_bottom {
            support_bottom |= b("support_bottom_enable");
        }
        if !support_roof {
            support_roof |= b("support_roof_enable");
        }
        if let Some(p) = &obj.painted_extruders {
            for nr in p {
                if let Some(i) = ids_get(&ids, &nr.to_string()) {
                    add(&mut used, i);
                }
            }
        }
    }
    let g = |k: &str| m.get_value(StackId::Global, k).unwrap_or(Value::None);
    let default_pos = m.default_extruder_position.borrow().clone();
    for feature in ["wall_0_extruder_nr", "wall_x_extruder_nr", "roofing_extruder_nr", "flooring_extruder_nr", "top_bottom_extruder_nr", "infill_extruder_nr"] {
        let Some(mut nr) = as_i64(&g(feature)) else { continue };
        if nr == -1 {
            continue;
        }
        if ids_get(&ids, &nr.to_string()).is_none() {
            nr = default_pos.parse().unwrap_or(0);
        }
        if let Some(i) = ids_get(&ids, &nr.to_string()) {
            add(&mut used, i);
        }
    }
    let with_default = |v: Value| -> String {
        let s = v.py_str();
        if s == "-1" {
            default_pos.clone()
        } else {
            s
        }
    };
    if support_enabled {
        for k in ["support_infill_extruder_nr", "support_extruder_nr_layer_0"] {
            if let Some(i) = ids_get(&ids, &with_default(g(k))) {
                add(&mut used, i);
            }
        }
        if support_bottom {
            if let Some(i) = ids_get(&ids, &with_default(g("support_bottom_extruder_nr"))) {
                add(&mut used, i);
            }
        }
        if support_roof {
            if let Some(i) = ids_get(&ids, &with_default(g("support_roof_extruder_nr"))) {
                add(&mut used, i);
            }
        }
    }
    let adhesion = g("adhesion_type").py_str();
    let gt0 = |k: &str| g(k).py_gt(&Value::Int(0)).unwrap_or(false);
    let mut adhesion_keys: Vec<&str> = Vec::new();
    if adhesion == "skirt" && (gt0("skirt_line_count") || gt0("skirt_brim_minimal_length")) {
        adhesion_keys.push("skirt_brim_extruder_nr");
    }
    if (adhesion == "brim" || g("prime_tower_brim_enable").truthy()) && (gt0("brim_line_count") || gt0("skirt_brim_minimal_length")) {
        adhesion_keys.push("skirt_brim_extruder_nr");
    }
    if adhesion == "raft" {
        adhesion_keys.push("raft_base_extruder_nr");
        if gt0("raft_interface_layers") {
            adhesion_keys.push("raft_interface_extruder_nr");
        }
        if gt0("raft_surface_layers") {
            adhesion_keys.push("raft_surface_extruder_nr");
        }
    }
    for k in adhesion_keys {
        let s = g(k).py_str();
        if s == "-1" {
            continue;
        }
        if let Some(i) = ids_get(&ids, &s) {
            add(&mut used, i);
        }
    }
    used
}

/// `ExtruderManager.getFirstUsedExtruderStack().getValue("extruder_nr")`.
fn first_used_extruder_nr(m: &Machine, objects: &[SceneObject]) -> Value {
    let mut used: Vec<(Value, usize)> = used_extruders(m, objects).into_iter().map(|i| (m.get_value(StackId::Extruder(i), "extruder_nr").unwrap_or(Value::None), i)).collect();
    used.sort_by(|a, b| a.0.py_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    // Python raises IndexError on an empty list; slicing is impossible without extruders anyway.
    used.first().map(|(v, _)| v.clone()).unwrap_or(Value::Int(0))
}

/// `ExtruderManager.getInitialExtruderNr`.
pub fn initial_extruder_nr(m: &Machine, objects: &[SceneObject]) -> Value {
    let g = |k: &str| m.get_value(StackId::Global, k).unwrap_or(Value::None);
    let adhesion = g("adhesion_type");
    let a = adhesion.py_str();
    if matches!(adhesion, Value::Str(_)) && (a == "skirt" || a == "brim") {
        let nr = g("skirt_brim_extruder_nr");
        if nr.py_eq(&Value::Int(-1)) {
            return first_used_extruder_nr(m, objects);
        }
        return nr;
    }
    if matches!(adhesion, Value::Str(_)) && a == "raft" {
        return g("raft_base_extruder_nr");
    }
    if (g("support_enable").truthy() || g("support_structure").py_eq(&Value::str("tree"))) && g("support_brim_enable").truthy() {
        return g("support_infill_extruder_nr");
    }
    first_used_extruder_nr(m, objects)
}

/// `StartSliceJob._buildReplacementTokens`.
fn replacement_tokens(m: &Machine, s: StackId, initial: &Value, time: &TimeTokens) -> Tokens {
    let mut result: Tokens = HashMap::new();
    for key in m.all_keys(s) {
        let v = m.get_property(s, &key, "value", None).unwrap_or(Value::None);
        result.insert(key, v);
    }
    let st = m.stack(s);
    let material = st.container(index::MATERIAL);
    result.insert("material_id".into(), meta_or(material, "base_file", ""));
    result.insert("material_type".into(), meta_or(material, "material", ""));
    result.insert("material_name".into(), meta_or(material, "name", ""));
    result.insert("material_brand".into(), meta_or(material, "brand", ""));
    result.insert("quality_name".into(), meta_or(st.container(index::QUALITY), "name", ""));
    result.insert("quality_changes_name".into(), container_meta(st.container(index::QUALITY_CHANGES), "name").unwrap_or(Value::None));
    let copy = |r: &mut Tokens, to: &str, from: &str| {
        let v = r.get(from).cloned().unwrap_or(Value::None);
        r.insert(to.into(), v);
    };
    copy(&mut result, "print_bed_temperature", "material_bed_temperature");
    copy(&mut result, "print_temperature", "material_print_temperature");
    copy(&mut result, "travel_speed", "speed_travel");
    result.insert("time".into(), Value::str(&time.time));
    result.insert("date".into(), Value::str(&time.date));
    result.insert("day".into(), Value::str(["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][time.weekday % 7]));
    result.insert("initial_extruder_nr".into(), initial.clone());
    result
}

/// All extruder token sets: "-1" for the global stack, then per active extruder by `extruder_nr`.
pub struct AllTokens {
    map: HashMap<String, Tokens>,
}

impl AllTokens {
    pub fn build(m: &Machine, objects: &[SceneObject], time: &TimeTokens) -> AllTokens {
        let initial = initial_extruder_nr(m, objects);
        let mut map = HashMap::new();
        map.insert("-1".to_string(), replacement_tokens(m, StackId::Global, &initial, time));
        for i in m.extruder_list() {
            let nr = m.get_value(StackId::Extruder(i), "extruder_nr").unwrap_or(Value::None).py_str();
            let t = replacement_tokens(m, StackId::Extruder(i), &initial, time);
            map.insert(nr, t);
        }
        AllTokens { map }
    }
    pub fn get(&self, nr: &str) -> Option<&Tokens> {
        self.map.get(nr)
    }
}

/// Python `str.isspace` for a single character.
fn py_isspace(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

struct Instruction<'t> {
    start: usize,
    end: usize,
    condition: Option<&'static str>,
    expression: &'t str,
    extruder_nr_expr: Option<&'t str>,
    end_of_line: &'t str,
}

/// `GcodeStartEndFormatter._instruction_regex.search`:
/// `{(?P<condition>if|else|elif|endif)?\s*(?P<expression>[^{}]*?)\s*(?:,\s*(?P<extruder_nr_expr>[^{}]*))?\s*}(?P<end_of_line>\n?)`
fn find_instruction(text: &str) -> Option<Instruction<'_>> {
    let bytes = text.as_bytes();
    let mut p = 0;
    while p < bytes.len() {
        let open = p + text[p..].find('{')?;
        let close_rel = text[open + 1..].find(['{', '}']);
        let Some(cr) = close_rel else { return None };
        let q = open + 1 + cr;
        if bytes[q] == b'{' {
            p = q;
            continue;
        }
        let body = &text[open + 1..q];
        let condition = ["if", "else", "elif", "endif"].into_iter().find(|c| body.starts_with(c));
        let rest = &body[condition.map(|c| c.len()).unwrap_or(0)..];
        let rest = rest.trim_start_matches(py_isspace);
        let (expression, extruder_nr_expr) = match rest.find(',') {
            Some(c) => {
                let expr = rest[..c].trim_end_matches(py_isspace);
                let ext = rest[c + 1..].trim_start_matches(py_isspace);
                (expr, Some(ext))
            }
            None => (rest.trim_end_matches(py_isspace), None),
        };
        let mut end = q + 1;
        let mut eol = "";
        if bytes.get(end) == Some(&b'\n') {
            eol = "\n";
            end += 1;
        }
        return Some(Instruction { start: open, end, condition, expression, extruder_nr_expr, end_of_line: eol });
    }
    None
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CondState {
    Outside,
    False,
    True,
    Done,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Instr {
    Skip = 1,
    Evaluate = 2,
    EvaluateAndWrite = 3,
}

struct Formatter<'a> {
    m: &'a Machine,
    tokens: &'a AllTokens,
    default_extruder_nr: String,
    state: CondState,
}

impl Formatter<'_> {
    fn format(&mut self, text: &str) -> Result<String, ()> {
        let mut remaining = text;
        let mut result = String::new();
        self.state = CondState::Outside;
        while !remaining.is_empty() {
            match find_instruction(remaining) {
                Some(ins) => {
                    if ins.start > 0 {
                        result.push_str(self.statement(&remaining[..ins.start]));
                    }
                    let code = self.code(&ins)?;
                    result.push_str(&code);
                    remaining = &remaining[ins.end..];
                }
                None => {
                    result.push_str(self.statement(remaining));
                    remaining = "";
                }
            }
        }
        Ok(result)
    }

    fn statement<'s>(&self, s: &'s str) -> &'s str {
        if matches!(self.state, CondState::Outside | CondState::True) {
            s
        } else {
            ""
        }
    }

    fn code(&mut self, ins: &Instruction) -> Result<String, ()> {
        const POST_SLICE: [&str; 5] = ["filament_cost", "print_time", "filament_amount", "filament_weight", "jobname"];
        if POST_SLICE.contains(&ins.expression) {
            return Ok(format!("{{{}}}", ins.expression));
        }
        let mut extruder_nr = self.default_extruder_nr.clone();
        let mut instruction = Instr::Skip;
        match ins.condition {
            None => {
                if matches!(self.state, CondState::Outside | CondState::True) {
                    instruction = Instr::EvaluateAndWrite;
                }
            }
            Some(cond) => {
                if cond == "if" {
                    if self.state != CondState::Outside {
                        return Err(()); // Nested conditions are not supported.
                    }
                } else if self.state == CondState::Outside {
                    return Err(()); // Condition should start with an 'if' statement.
                }
                if cond == "if" {
                    instruction = Instr::Evaluate;
                } else {
                    if self.state == CondState::True {
                        self.state = CondState::Done;
                    }
                    if cond == "elif" {
                        instruction = if self.state == CondState::False { Instr::Evaluate } else { Instr::Skip };
                    } else if cond == "else" {
                        instruction = Instr::Skip;
                        if self.state == CondState::False {
                            self.state = CondState::True;
                        }
                    } else if cond == "endif" {
                        instruction = Instr::Skip;
                        self.state = CondState::Outside;
                    }
                }
            }
        }
        if instruction >= Instr::Evaluate {
            if let Some(e) = ins.extruder_nr_expr {
                let f = Function::new(e);
                let globals = self.tokens.get("-1").unwrap();
                extruder_nr = self.m.evaluate_with_locals(&f, globals).unwrap_or(Value::None).py_str();
            }
        }
        if instruction >= Instr::Evaluate {
            let vars = self.tokens.get(&extruder_nr).or_else(|| self.tokens.get("-1")).unwrap();
            let f = Function::new(ins.expression);
            let value = self.m.evaluate_with_locals(&f, vars).unwrap_or(Value::None);
            if instruction == Instr::Evaluate {
                self.state = if value.truthy() { CondState::True } else { CondState::False };
                return Ok(String::new());
            }
            let mut s = value.py_str();
            s.push_str(ins.end_of_line);
            return Ok(s);
        }
        Ok(String::new())
    }
}

/// `StartSliceJob._expandGcodeTokens`.
pub fn expand_gcode_tokens(m: &Machine, tokens: &AllTokens, value: &str, default_extruder_nr: &str) -> String {
    let mut f = Formatter { m, tokens, default_extruder_nr: default_extruder_nr.to_string(), state: CondState::Outside };
    f.format(value).unwrap_or_else(|_| value.to_string())
}

/// `re.sub(r";.+?(\n|$)", "\n", start_gcode)`.
fn strip_comments(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(p) = rest.find(';') {
        let after = &rest[p + 1..];
        let line_len = after.find('\n').unwrap_or(after.len());
        if line_len == 0 {
            // `;` directly followed by a newline (or the end): no match here.
            out.push_str(&rest[..p + 1]);
            rest = after;
            continue;
        }
        out.push_str(&rest[..p]);
        out.push('\n');
        let consumed = if line_len < after.len() { line_len + 1 } else { line_len };
        rest = &after[consumed..];
    }
    out.push_str(rest);
    out
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `re.search(r"\{(name1|name2|...)(,\s?\w+)?\}", text) is not None`.
fn has_setting_reference(text: &str, names: &[&str]) -> bool {
    for (p, _) in text.match_indices('{') {
        let after = &text[p + 1..];
        for n in names {
            let Some(rest) = after.strip_prefix(n) else { continue };
            if rest.starts_with('}') {
                return true;
            }
            if let Some(r) = rest.strip_prefix(',') {
                let mut chars = r.chars().peekable();
                let mut r2 = r;
                if let Some(c) = chars.peek() {
                    if py_isspace(*c) {
                        r2 = &r[c.len_utf8()..];
                    }
                }
                let word_len: usize = r2.chars().take_while(|c| is_word_char(*c)).map(|c| c.len_utf8()).sum();
                if word_len > 0 && r2[word_len..].starts_with('}') {
                    return true;
                }
                // Without the optional whitespace (only differs when it was consumed above).
                let word_len: usize = r.chars().take_while(|c| is_word_char(*c)).map(|c| c.len_utf8()).sum();
                if word_len > 0 && r[word_len..].starts_with('}') {
                    return true;
                }
            }
        }
    }
    false
}

/// Python `int(round(float(v)))`.
fn round_to_int(v: &Value) -> Option<i64> {
    let f = match v {
        Value::Int(i) => *i as f64,
        Value::Bool(b) => *b as i64 as f64,
        Value::Float(f) => *f,
        Value::Str(s) => crate::py::value::parse_float(s)?,
        _ => return None,
    };
    if !f.is_finite() {
        return None;
    }
    // Python's round() rounds half to even.
    let r = f.round();
    let r = if (f - f.trunc()).abs() == 0.5 { 2.0 * (f / 2.0).round() } else { r };
    Some(r as i64)
}

/// The keys of a per-object stack's user container plus everything related to them
/// (`StartSliceJob._addRelations` on the definitions the instances were created from:
/// `stack.getSettingDefinition`, i.e. the extruder definition first, then the machine's).
fn changed_keys_with_relations(m: &Machine, o: usize) -> Vec<String> {
    let user = &m.object_stacks[o].user;
    let ext_def = m.extruders[m.object_stacks[o].extruder_position].definition().clone();
    let global_def = m.global.definition().clone();
    let mut keys: Vec<String> = Vec::new();
    let mut set: HashSet<String> = HashSet::new();
    for k in user.keys() {
        let def = if ext_def.has_key(&k) {
            &ext_def
        } else if global_def.has_key(&k) {
            &global_def
        } else {
            continue;
        };
        if set.insert(k.clone()) {
            keys.push(k.clone());
        }
        let mut related = HashSet::new();
        def.add_relations(&k, &mut related);
        let mut related: Vec<String> = related.into_iter().collect();
        related.sort();
        for r in related {
            if set.insert(r.clone()) {
                keys.push(r);
            }
        }
    }
    keys
}

/// The keys `_handlePerObjectSettings` sends for an object.
pub fn per_object_keys(m: &Machine, o: usize) -> Vec<String> {
    let mut keys = changed_keys_with_relations(m, o);
    if !keys.iter().any(|k| k == "extruder_nr") {
        keys.push("extruder_nr".into());
    }
    keys
}

/// Build all settings messages of `StartSliceJob.run` (after the checks).
pub fn build_slice_settings(m: &Machine, objects: &[SceneObject], time: &TimeTokens) -> SliceSettings {
    let tokens = AllTokens::build(m, objects, time);
    let mut out = SliceSettings::default();
    let initial = initial_extruder_nr(m, objects).py_str();

    // Global settings.
    let mut settings: Vec<(String, Value)> = tokens.get("-1").unwrap().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    settings.sort_by(|a, b| a.0.cmp(&b.0));
    let get = |s: &Vec<(String, Value)>, k: &str| s.iter().find(|(x, _)| x == k).map(|(_, v)| v.clone()).unwrap_or(Value::None);
    let set = |s: &mut Vec<(String, Value)>, k: &str, v: Value| {
        if let Some(slot) = s.iter_mut().find(|(x, _)| x == k) {
            slot.1 = v;
        } else {
            s.push((k.to_string(), v));
        }
    };
    let start_gcode = strip_comments(&get(&settings, "machine_start_gcode").py_str());
    if get(&settings, "material_bed_temp_prepend").truthy() {
        let found = has_setting_reference(&start_gcode, &["material_bed_temperature", "material_bed_temperature_layer_0"]);
        set(&mut settings, "material_bed_temp_prepend", Value::Bool(!found));
    }
    if get(&settings, "material_print_temp_prepend").truthy() {
        let names = [
            "material_print_temperature",
            "material_print_temperature_layer_0",
            "default_material_print_temperature",
            "material_initial_print_temperature",
            "material_final_print_temperature",
            "material_standby_temperature",
            "print_temperature",
        ];
        let found = has_setting_reference(&start_gcode, &names);
        set(&mut settings, "material_print_temp_prepend", Value::Bool(!found));
    }
    for k in ["machine_start_gcode", "machine_end_gcode"] {
        let v = get(&settings, k).py_str();
        set(&mut settings, k, Value::str(&expand_gcode_tokens(m, &tokens, &v, &initial)));
    }
    set(&mut settings, "nozzle_offsetting_for_disallowed_areas", global_meta(m, "nozzle_offsetting_for_disallowed_areas").unwrap_or(Value::Bool(true)));
    out.global_settings = settings.into_iter().map(|(k, v)| (k, v.py_str())).collect();

    // limit_to_extruder.
    let mut keys = m.all_keys(StackId::Global);
    keys.sort();
    for key in keys {
        let v = m.get_property(StackId::Global, &key, "limit_to_extruder", None).unwrap_or(Value::None);
        if let Some(pos) = round_to_int(&v) {
            if pos >= 0 {
                out.limit_to_extruder.push((key, pos));
            }
        }
    }

    // Extruders.
    let global_def = m.global.definition().clone();
    for i in m.extruder_list() {
        let s = StackId::Extruder(i);
        let id = m.extruders[i].position().and_then(|p| p.parse().ok()).unwrap_or(0);
        let nr = m.get_value(s, "extruder_nr").unwrap_or(Value::None).py_str();
        let Some(t) = tokens.get(&nr) else { continue };
        let mut settings: Vec<(String, Value)> = t.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        settings.sort_by(|a, b| a.0.cmp(&b.0));
        set(&mut settings, "material_guid", meta_or(m.extruders[i].container(index::MATERIAL), "GUID", ""));
        for k in ["machine_extruder_prestart_code", "machine_extruder_start_code", "machine_extruder_end_code"] {
            let v = get(&settings, k).py_str();
            set(&mut settings, k, Value::str(&expand_gcode_tokens(m, &tokens, &v, &nr)));
        }
        let own_def = m.extruders[i].definition().clone();
        let settable = |d: &crate::definition::DefinitionContainer, k: &str| match d.get_property(k, "settable_per_extruder") {
            Some(Prop::Value(v)) => v.truthy(),
            Some(Prop::Func(_)) => true,
            None => false,
        };
        let filtered: Vec<(String, String)> =
            settings.into_iter().filter(|(k, _)| settable(&global_def, k) || settable(&own_def, k)).map(|(k, v)| (k, v.py_str())).collect();
        out.extruders.push((id, filtered));
    }

    // Per-object settings.
    for obj in objects {
        let o = obj.stack;
        let keys = per_object_keys(m, o);
        let mut list = Vec::new();
        for key in keys {
            let v = m.get_property(StackId::Object(o), &key, "value", None).unwrap_or(Value::None);
            list.push((key, v.py_str()));
        }
        out.object_settings.push(list);
    }
    out
}

/// The checks at the start of `StartSliceJob.run` that depend only on settings: material
/// compatibility of the enabled extruders and per-object setting errors.
pub fn check_materials(m: &Machine) -> bool {
    for i in m.extruder_list() {
        if !m.extruders[i].is_enabled() {
            continue;
        }
        if let Container::Instance(c) = m.extruders[i].container(index::MATERIAL) {
            if matches!(c.meta("compatible"), Some(Value::Bool(false))) {
                return false;
            }
        }
    }
    true
}

/// `StartSliceJob._checkStackForErrors` for a per-object stack.
pub fn object_has_errors(m: &Machine, o: usize) -> bool {
    for k in changed_keys_with_relations(m, o) {
        let s = StackId::Object(o);
        if !m.get_property(s, &k, "enabled", None).map(|v| v.truthy()).unwrap_or(false) {
            continue;
        }
        if m.validation_state(s, &k).is_error() {
            return true;
        }
    }
    false
}
