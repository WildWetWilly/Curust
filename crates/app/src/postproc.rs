//! Post-processing scripts: Cura's PostProcessingPlugin, running Cura's own Python scripts
//! (bundled, plus any the user installed for Cura or Curust) in the embedded interpreter.
//!
//! The configured scripts live in the global stack's `post_processing_scripts` metadata in
//! exactly Cura's format, so profiles and project files carry them between Cura and Curust.

use crate::resources::config_dir;
use crate::session::Session;
use crate::slicing::PrintInfo;
use curust_pyrt::ast::{Expr, Name};
use curust_pyrt::interp::Frame;
use curust_pyrt::value::{Dict, HashKey, Namespace, NativeObject};
use curust_pyrt::{Flow, Host, Interp, Value, R};
use curust_settings::py::Value as SValue;
use curust_settings::stack::{index, StackId};
use curust_settings::upgrade::ConfigParser;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

const SHIM: &str = include_str!("postproc_shim.py");
const SCRIPT_PY: &str = include_str!("../postproc/Script.py");

/// Cura's bundled scripts (PostProcessingPlugin/scripts, Cura 5.12).
const BUNDLED: &[(&str, &str)] = &[
    ("AddCoolingProfile", include_str!("../postproc/scripts/AddCoolingProfile.py")),
    ("AnnealingOrDrying", include_str!("../postproc/scripts/AnnealingOrDrying.py")),
    ("ChangeAtZ", include_str!("../postproc/scripts/ChangeAtZ.py")),
    ("ColorMix", include_str!("../postproc/scripts/ColorMix.py")),
    ("CreateThumbnail", include_str!("../postproc/scripts/CreateThumbnail.py")),
    ("DisplayFilenameAndLayerOnLCD", include_str!("../postproc/scripts/DisplayFilenameAndLayerOnLCD.py")),
    ("DisplayInfoOnLCD", include_str!("../postproc/scripts/DisplayInfoOnLCD.py")),
    ("DisplayProgressOnLCD", include_str!("../postproc/scripts/DisplayProgressOnLCD.py")),
    ("FilamentChange", include_str!("../postproc/scripts/FilamentChange.py")),
    ("InsertAtLayerChange", include_str!("../postproc/scripts/InsertAtLayerChange.py")),
    ("LimitXYAccelJerk", include_str!("../postproc/scripts/LimitXYAccelJerk.py")),
    ("PauseAtHeight", include_str!("../postproc/scripts/PauseAtHeight.py")),
    ("PurgeLinesAndUnload", include_str!("../postproc/scripts/PurgeLinesAndUnload.py")),
    ("RetractContinue", include_str!("../postproc/scripts/RetractContinue.py")),
    ("SearchAndReplace", include_str!("../postproc/scripts/SearchAndReplace.py")),
    ("Stretch", include_str!("../postproc/scripts/Stretch.py")),
    ("TimeLapse", include_str!("../postproc/scripts/TimeLapse.py")),
    ("TweakAtZ", include_str!("../postproc/scripts/TweakAtZ.py")),
    ("UsePreviousProbeMeasurements", include_str!("../postproc/scripts/UsePreviousProbeMeasurements.py")),
    ("ZHopOnTravel", include_str!("../postproc/scripts/ZHopOnTravel.py")),
];

/// The Cura version scripts see (`Application.getVersion()`).
pub const CURA_VERSION: &str = "5.12.0";

/// A script file that can be loaded.
#[derive(Clone, Debug)]
pub struct ScriptFile {
    /// Module (= class) name: the file name without `.py`.
    pub name: String,
    pub source: String,
    /// "Cura (bundled)" or the folder it was found in.
    pub origin: String,
}

/// Folders searched for user scripts, in Cura's order after the bundled ones.
pub fn user_script_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![config_dir().join("scripts")];
    if let Ok(p) = std::env::var("CURUST_SCRIPTS") {
        dirs.insert(0, PathBuf::from(p));
    }
    // The newest UltiMaker Cura configuration (Resources.Preferences / Resources.Resources).
    let mut bases: Vec<PathBuf> = Vec::new();
    #[cfg(windows)]
    {
        if let Ok(a) = std::env::var("APPDATA") {
            bases.push(PathBuf::from(a).join("cura"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(h) = std::env::var("HOME") {
            bases.push(PathBuf::from(h).join("Library").join("Application Support").join("cura"));
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Ok(h) = std::env::var("HOME") {
            bases.push(PathBuf::from(&h).join(".config").join("cura"));
            bases.push(PathBuf::from(&h).join(".local").join("share").join("cura"));
        }
    }
    // Scripts that come with installed plugins (Marketplace packages).
    dirs.extend(crate::marketplace::plugin_script_dirs(std::env::var("CURUST_NO_CURA_SCRIPTS").is_err()));
    if std::env::var("CURUST_NO_CURA_SCRIPTS").is_err() {
        for base in bases {
            let Ok(rd) = std::fs::read_dir(&base) else { continue };
            let mut versions: Vec<(Vec<u64>, PathBuf)> = rd
                .flatten()
                .filter(|e| e.path().is_dir())
                .filter_map(|e| {
                    let n = e.file_name().to_string_lossy().to_string();
                    let v: Vec<u64> = n.split('.').map(|x| x.parse().ok()).collect::<Option<Vec<u64>>>()?;
                    Some((v, e.path()))
                })
                .collect();
            versions.sort();
            if let Some((_, p)) = versions.pop() {
                dirs.push(p.join("scripts"));
            }
        }
    }
    dirs
}

/// All loadable scripts: Cura's bundled scripts first, then user folders (first name wins,
/// like Cura's `sys.modules` check).
pub fn script_files() -> Vec<ScriptFile> {
    let mut out: Vec<ScriptFile> = BUNDLED.iter().map(|(n, s)| ScriptFile { name: n.to_string(), source: s.to_string(), origin: "Cura (bundled)".into() }).collect();
    for dir in user_script_dirs() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "py")).collect();
        files.sort();
        for p in files {
            let name = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
            if name.starts_with('_') || out.iter().any(|f| f.name == name) {
                continue;
            }
            if let Ok(source) = std::fs::read_to_string(&p) {
                out.push(ScriptFile { name, source, origin: dir.display().to_string() });
            }
        }
    }
    out
}

// ---------------------------------------------------------------------- the stored script list

/// A configured script: its key and setting values (strings, as stored).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScriptConfig {
    pub key: String,
    pub settings: Vec<(String, String)>,
}

/// `_restoreScriptInforFromMetadata`: parse the `post_processing_scripts` metadata.
pub fn parse_scripts(meta: &str, log: &mut Vec<String>) -> Vec<ScriptConfig> {
    let mut out = Vec::new();
    for script_str in meta.split('\n') {
        if script_str.is_empty() {
            continue;
        }
        let script_str = script_str.replace("\\\\\\n", "\n").replace("\\\\\\\\", "\\\\");
        let parser = match ConfigParser::read_opts(&script_str, true, false) {
            Ok(p) => p,
            Err(e) => {
                log.push(format!("Stored post-processing scripts have syntax errors: {}", e));
                continue;
            }
        };
        for (name, items) in parser.sections {
            if name == "DEFAULT" {
                continue;
            }
            out.push(ScriptConfig { key: name, settings: items });
        }
    }
    out
}

/// `writeScriptsToStack`: one ConfigParser section per script, escaped, joined by newlines.
pub fn serialize_scripts(scripts: &[ScriptConfig]) -> String {
    let mut parts = Vec::new();
    for s in scripts {
        let p = ConfigParser { sections: vec![(s.key.clone(), s.settings.clone())] };
        let script_str = p.write();
        parts.push(script_str.replace("\\\\", "\\\\\\\\").replace('\n', "\\\\\\n"));
    }
    parts.join("\n")
}

/// The script list stored on the active printer.
pub fn stored_scripts(s: &Session) -> String {
    s.machine().and_then(|m| m.global.meta("post_processing_scripts").map(|v| v.py_str())).unwrap_or_default()
}

pub fn set_stored_scripts(s: &mut Session, meta: &str) {
    if let Some(a) = s.active {
        s.machines[a].global.set_meta("post_processing_scripts", SValue::Str(meta.into()));
    }
}

// ---------------------------------------------------------------------- values

fn to_py(v: &SValue) -> Value {
    match v {
        SValue::None => Value::None,
        SValue::Bool(b) => Value::Bool(*b),
        SValue::Int(i) => Value::Int(*i),
        SValue::Float(f) => Value::Float(*f),
        SValue::Str(s) => Value::Str(s.clone()),
        SValue::List(l) => Value::list(l.iter().map(to_py).collect()),
        SValue::Tuple(l) => Value::tuple(l.iter().map(to_py).collect()),
        SValue::Set(l) => {
            let mut d = Dict::default();
            for x in l.iter() {
                let v = to_py(x);
                if let Some(h) = curust_pyrt::value::hash_key(&v) {
                    d.insert(h, v, Value::None);
                }
            }
            Value::Set(Rc::new(RefCell::new(d)))
        }
        SValue::Dict(items) => {
            let mut d = Dict::default();
            for (k, x) in items.iter() {
                let kv = to_py(k);
                if let Some(h) = curust_pyrt::value::hash_key(&kv) {
                    d.insert(h, kv, to_py(x));
                }
            }
            Value::dict(d)
        }
        other => Value::Str(other.py_str().into()),
    }
}

fn str_dict(items: Vec<(String, Value)>) -> Value {
    let mut d = Dict::default();
    for (k, v) in items {
        d.insert(HashKey::Str(k.as_str().into()), Value::Str(k.as_str().into()), v);
    }
    Value::dict(d)
}

fn py_to_json(v: &Value) -> curust_settings::json::Json {
    use curust_settings::json::Json;
    match v.unwrap_base() {
        Value::None => Json::Null,
        Value::Bool(b) => Json::Bool(b),
        Value::Int(i) => Json::Int(i),
        Value::Float(f) => Json::Float(f),
        Value::Str(s) => Json::Str(s.to_string()),
        Value::List(l) => Json::Array(l.borrow().iter().map(py_to_json).collect()),
        Value::Tuple(t) => Json::Array(t.iter().map(py_to_json).collect()),
        Value::Dict(d) => Json::Object(d.borrow().items().map(|(k, v)| (match k { Value::Str(s) => s.to_string(), other => format!("{:?}", other.type_name()) }, py_to_json(v))).collect()),
        other => Json::Str(other.type_name()),
    }
}

// ---------------------------------------------------------------------- the host

/// What scripts can see of the application.
pub struct HostState {
    session: *const Session,
    pub print_info: Option<PrintInfo>,
    pub job_name: String,
    pub log: RefCell<Vec<String>>,
    pub messages: RefCell<Vec<(String, String)>>,
    stack_meta: RefCell<Vec<(i64, String, Value)>>,
    pub snapshot: Option<Box<dyn Fn(u32, u32) -> Option<Vec<u8>>>>,
    pub used_extruders: Vec<usize>,
}

impl HostState {
    fn session(&self) -> &Session {
        // SAFETY: a HostState only lives inside `with_interp`, which borrows the session for its
        // whole duration.
        unsafe { &*self.session }
    }

    fn stack_id(&self, index: i64) -> Option<StackId> {
        let m = self.session().machine()?;
        if index < 0 {
            return Some(StackId::Global);
        }
        m.extruder_list().get(index as usize).map(|&e| StackId::Extruder(e))
    }
}

struct PpHost {
    st: Rc<HostState>,
}

/// `(module name, names re-exported from the shim)`.
const SHIM_MODULES: &[(&str, &[&str])] = &[
    ("UM.Application", &["Application"]),
    ("UM.Logger", &["Logger"]),
    ("UM.Message", &["Message"]),
    ("UM.Signal", &["Signal", "signalemitter"]),
    ("UM.i18n", &["i18nCatalog"]),
    ("UM.Settings.ContainerStack", &["ContainerStack"]),
    ("UM.Settings.InstanceContainer", &["InstanceContainer"]),
    ("UM.Settings.DefinitionContainer", &["DefinitionContainer"]),
    ("UM.Settings.ContainerRegistry", &["ContainerRegistry"]),
    ("UM.Settings.ContainerFormatError", &["ContainerFormatError"]),
    ("UM.Settings.SettingDefinition", &["SettingDefinition"]),
    ("UM.Settings.SettingFunction", &["SettingFunction"]),
    ("UM.Settings.Interfaces", &[]),
    ("UM.Qt.Duration", &["Duration", "DurationFormat"]),
    ("UM.Math.Vector", &["Vector"]),
    ("cura.CuraApplication", &["CuraApplication"]),
    ("cura.Settings.ExtruderManager", &["ExtruderManager"]),
    ("cura.Snapshot", &["Snapshot"]),
    ("PyQt6.QtCore", &["QByteArray", "QIODevice", "QBuffer"]),
    ("PyQt6.QtGui", &["QImage"]),
];

const PACKAGES: &[&str] = &["UM", "UM.Settings", "UM.Qt", "UM.Math", "cura", "cura.Settings", "PyQt6"];

impl Host for PpHost {
    fn import(&self, i: &mut Interp, name: &str) -> Option<R<Value>> {
        if name == "_curust_host" {
            return Some(Ok(Value::Native(Rc::new(HostModule { st: self.st.clone() }))));
        }
        if PACKAGES.contains(&name) {
            return Some(Ok(curust_pyrt::modules::make_module(name, Vec::new())));
        }
        let (_, names) = SHIM_MODULES.iter().find(|(n, _)| *n == name)?;
        Some((|| {
            let shim = i.import_module("_curust_shim")?;
            let mut entries: Vec<(&str, Value)> = Vec::new();
            for n in names.iter() {
                entries.push((n, i.get_attr(&shim, n)?));
            }
            if name == "UM.Settings.Interfaces" {
                entries.push(("DefinitionContainerInterface", i.get_attr(&shim, "DefinitionContainer")?));
                entries.push(("ContainerInterface", i.get_attr(&shim, "InstanceContainer")?));
            }
            let m = curust_pyrt::modules::make_module(name, entries);
            // Attach to the parent package (`import UM.Application` binds `UM`).
            if let Some((parent, child)) = name.rsplit_once('.') {
                if let Ok(p) = i.import_module(parent) {
                    let _ = i.set_attr(&p, child, m.clone());
                }
            }
            Ok(m)
        })())
    }
    fn print(&self, text: &str) {
        self.st.log.borrow_mut().push(text.to_string());
    }
}

/// The `_curust_host` module.
struct HostModule {
    st: Rc<HostState>,
}

const HOST_FUNCS: &[&str] = &[
    "stack_property",
    "container_info",
    "container_metadata",
    "container_property",
    "container_keys",
    "set_stack_metadata",
    "extruder_count",
    "active_extruder",
    "used_extruders",
    "initial_extruder",
    "print_information",
    "preference",
    "version",
    "log",
    "log_exception",
    "message",
    "snapshot",
    "literal_eval",
    "expr_names",
    "eval_setting",
];

fn container_index(kind: &str) -> Option<usize> {
    Some(match kind {
        "userChanges" => index::USER,
        "qualityChanges" => index::QUALITY_CHANGES,
        "intent" => index::INTENT,
        "quality" => index::QUALITY,
        "material" => index::MATERIAL,
        "variant" => index::VARIANT,
        "definitionChanges" => index::DEFINITION_CHANGES,
        "definition" => index::DEFINITION,
        _ => return None,
    })
}

impl NativeObject for HostModule {
    fn type_name(&self) -> &str {
        "module"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn get_attr(&self, _i: &mut Interp, this: &Value, name: &str) -> Option<R<Value>> {
        let n = HOST_FUNCS.iter().find(|f| **f == name)?;
        Some(Ok(Value::Method(Rc::new((this.clone(), n)))))
    }
    fn call_method(&self, i: &mut Interp, _this: &Value, name: &str, a: Vec<Value>, _kw: Vec<(Name, Value)>) -> Option<R<Value>> {
        Some(self.call(i, name, a))
    }
}

fn arg_str(i: &mut Interp, a: &[Value], k: usize) -> R<String> {
    i.to_str(a.get(k).unwrap_or(&Value::None))
}

fn arg_int(i: &mut Interp, a: &[Value], k: usize) -> R<i64> {
    curust_pyrt::builtins::to_int(i, a.get(k).unwrap_or(&Value::Int(0)))
}

/// Python builtins and the names Uranium's `_SettingExpressionVisitor` treats as known.
const KNOWN_NAMES: &[&str] = &[
    "math", "max", "min", "debug", "sum", "len", "uuid", "hashlib", "base64", "round", "int", "float", "str", "bool", "abs", "all", "any", "list", "tuple", "dict", "set", "range", "map", "filter", "sorted", "reversed", "zip",
    "enumerate", "isinstance", "True", "False", "None", "divmod", "pow", "hex", "oct", "bin", "chr", "ord", "print", "repr", "type", "frozenset", "next", "iter", "format", "hash", "callable", "getattr", "hasattr", "id", "object",
    "slice", "vars", "dir", "ValueError", "TypeError", "KeyError", "IndexError", "Exception",
];

fn collect_names(e: &Expr, bound: &mut Vec<String>, out: &mut Vec<String>) {
    use curust_pyrt::ast::{Arg, FPartE};
    let rec = |x: &Expr, bound: &mut Vec<String>, out: &mut Vec<String>| collect_names(x, bound, out);
    match e {
        Expr::Name(n) => {
            let s = n.to_string();
            if !KNOWN_NAMES.contains(&s.as_str()) && !bound.contains(&s) && !out.contains(&s) {
                out.push(s);
            }
        }
        Expr::Const(_) => {}
        Expr::FString(parts) => {
            for p in parts {
                if let FPartE::Expr { expr, .. } = p {
                    rec(expr, bound, out);
                }
            }
        }
        Expr::List(v) | Expr::Tuple(v) | Expr::Set(v) => v.iter().for_each(|x| rec(x, bound, out)),
        Expr::Dict(v) => {
            for (k, x) in v {
                if let Some(k) = k {
                    rec(k, bound, out);
                }
                rec(x, bound, out);
            }
        }
        Expr::Starred(x) | Expr::Unary(_, x) => rec(x, bound, out),
        Expr::Attr(x, _) => rec(x, bound, out),
        Expr::Subscript(a, b) | Expr::BinOp(a, _, b) | Expr::And(a, b) | Expr::Or(a, b) => {
            rec(a, bound, out);
            rec(b, bound, out);
        }
        Expr::Slice(a, b, c) => {
            for x in [a, b, c].into_iter().flatten() {
                rec(x, bound, out);
            }
        }
        Expr::Call(f, args) => {
            rec(f, bound, out);
            for a in args {
                match a {
                    Arg::Pos(x) | Arg::Star(x) | Arg::Kw(_, x) | Arg::StarStar(x) => rec(x, bound, out),
                }
            }
        }
        Expr::Compare(a, rest) => {
            rec(a, bound, out);
            for (_, x) in rest {
                rec(x, bound, out);
            }
        }
        Expr::IfExp(a, b, c) => {
            rec(a, bound, out);
            rec(b, bound, out);
            rec(c, bound, out);
        }
        Expr::Lambda(def) => {
            let n = bound.len();
            bound.extend(def.params.args.iter().map(|(p, _)| p.to_string()));
            for s in &def.body {
                if let curust_pyrt::ast::StmtKind::Return(Some(x)) = &s.kind {
                    rec(x, bound, out);
                }
            }
            bound.truncate(n);
        }
        Expr::ListComp(x, comps) | Expr::SetComp(x, comps) | Expr::GenExp(x, comps) => {
            let n = bound.len();
            for c in comps.iter() {
                rec(&c.iter, bound, out);
                let mut targets = Vec::new();
                collect_targets(&c.target, &mut targets);
                bound.extend(targets);
                for f in &c.ifs {
                    rec(f, bound, out);
                }
            }
            rec(x, bound, out);
            bound.truncate(n);
        }
        Expr::DictComp(k, v, comps) => {
            let n = bound.len();
            for c in comps.iter() {
                rec(&c.iter, bound, out);
                let mut targets = Vec::new();
                collect_targets(&c.target, &mut targets);
                bound.extend(targets);
                for f in &c.ifs {
                    rec(f, bound, out);
                }
            }
            rec(k, bound, out);
            rec(v, bound, out);
            bound.truncate(n);
        }
        Expr::NamedExpr(_, x) => rec(x, bound, out),
    }
}

fn collect_targets(e: &Expr, out: &mut Vec<String>) {
    match e {
        Expr::Name(n) => out.push(n.to_string()),
        Expr::Tuple(v) | Expr::List(v) => v.iter().for_each(|x| collect_targets(x, out)),
        Expr::Starred(x) => collect_targets(x, out),
        _ => {}
    }
}

/// `ast.literal_eval`.
fn literal_eval(i: &mut Interp, text: &str) -> R<Value> {
    use curust_pyrt::ast::{Const, UnOp};
    let e = curust_pyrt::parser::parse_expression(text).map_err(|e| i.exc("SyntaxError", e))?;
    fn lit(i: &mut Interp, e: &Expr) -> R<Value> {
        Ok(match e {
            Expr::Const(c) => match c {
                Const::None => Value::None,
                Const::Bool(b) => Value::Bool(*b),
                Const::Int(n) => Value::Int(*n),
                Const::Float(f) => Value::Float(*f),
                Const::Str(s) => Value::Str(s.clone()),
                Const::Bytes(b) => Value::Bytes(b.clone()),
                Const::Ellipsis => Value::Ellipsis,
            },
            Expr::Name(n) if &**n == "True" => Value::Bool(true),
            Expr::Name(n) if &**n == "False" => Value::Bool(false),
            Expr::Name(n) if &**n == "None" => Value::None,
            Expr::List(v) => Value::list(v.iter().map(|x| lit(i, x)).collect::<R<_>>()?),
            Expr::Tuple(v) => Value::tuple(v.iter().map(|x| lit(i, x)).collect::<R<_>>()?),
            Expr::Set(v) => {
                let mut d = Dict::default();
                for x in v {
                    let x = lit(i, x)?;
                    let h = i.hash(&x)?;
                    d.insert(h, x, Value::None);
                }
                Value::Set(Rc::new(RefCell::new(d)))
            }
            Expr::Dict(v) => {
                let mut d = Dict::default();
                for (k, x) in v {
                    let Some(k) = k else { return i.value_error("malformed node or string") };
                    let k = lit(i, k)?;
                    let x = lit(i, x)?;
                    let h = i.hash(&k)?;
                    d.insert(h, k, x);
                }
                Value::dict(d)
            }
            Expr::Unary(op @ (UnOp::Neg | UnOp::Pos), x) => {
                let v = lit(i, x)?;
                match v {
                    Value::Int(_) | Value::Float(_) => {
                        if *op == UnOp::Neg {
                            i.unary(UnOp::Neg, &v)?
                        } else {
                            v
                        }
                    }
                    _ => return i.value_error("malformed node or string"),
                }
            }
            _ => return i.value_error("malformed node or string on line 1"),
        })
    }
    lit(i, &e)
}

impl HostModule {
    fn call(&self, i: &mut Interp, name: &str, a: Vec<Value>) -> R<Value> {
        let st = &self.st;
        match name {
            "stack_property" => {
                let idx = arg_int(i, &a, 0)?;
                let key = arg_str(i, &a, 1)?;
                let prop = arg_str(i, &a, 2)?;
                let Some(m) = st.session().machine() else { return Ok(Value::None) };
                let Some(sid) = st.stack_id(idx) else { return Ok(Value::None) };
                Ok(m.get_property(sid, &key, &prop, None).map(|v| to_py(&v)).unwrap_or(Value::None))
            }
            "container_info" => {
                let idx = arg_int(i, &a, 0)?;
                let kind = arg_str(i, &a, 1)?;
                let what = arg_str(i, &a, 2)?;
                let Some(m) = st.session().machine() else { return Ok(Value::str("empty")) };
                let Some(sid) = st.stack_id(idx) else { return Ok(Value::str("empty")) };
                let stack = m.stack(sid);
                if kind == "stack" {
                    return Ok(Value::Str(if what == "id" { stack.id.clone() } else { stack.name.clone() }.into()));
                }
                let Some(ci) = container_index(&kind) else { return Ok(Value::str("empty")) };
                let c = stack.container(ci);
                Ok(Value::Str(if what == "id" { c.id().to_string() } else { c.name() }.into()))
            }
            "container_metadata" => {
                let idx = arg_int(i, &a, 0)?;
                let kind = arg_str(i, &a, 1)?;
                let Some(m) = st.session().machine() else { return Ok(str_dict(Vec::new())) };
                let Some(sid) = st.stack_id(idx) else { return Ok(str_dict(Vec::new())) };
                let stack = m.stack(sid);
                let mut items: Vec<(String, Value)> = Vec::new();
                if kind == "stack" {
                    items.push(("id".into(), Value::Str(stack.id.as_str().into())));
                    items.push(("name".into(), Value::Str(stack.name.as_str().into())));
                    for (k, v) in &stack.metadata {
                        items.retain(|(n, _)| n != k);
                        items.push((k.clone(), to_py(v)));
                    }
                    for (si, k, v) in st.stack_meta.borrow().iter() {
                        if *si == idx {
                            items.retain(|(n, _)| n != k);
                            items.push((k.clone(), v.clone()));
                        }
                    }
                } else if let Some(ci) = container_index(&kind) {
                    let c = stack.container(ci);
                    items.push(("id".into(), Value::Str(c.id().into())));
                    items.push(("name".into(), Value::Str(c.name().into())));
                    match c {
                        curust_settings::stack::Container::Instance(inst) => {
                            for (k, v) in &inst.metadata {
                                items.retain(|(n, _)| n != k);
                                items.push((k.clone(), to_py(v)));
                            }
                        }
                        curust_settings::stack::Container::Definition(d) => {
                            if let curust_settings::json::Json::Object(o) = &d.metadata {
                                for (k, v) in o {
                                    items.retain(|(n, _)| n != k);
                                    items.push((k.clone(), json_to_py(v)));
                                }
                            }
                        }
                    }
                }
                Ok(str_dict(items))
            }
            "container_property" => {
                let idx = arg_int(i, &a, 0)?;
                let kind = arg_str(i, &a, 1)?;
                let key = arg_str(i, &a, 2)?;
                let prop = arg_str(i, &a, 3)?;
                let Some(m) = st.session().machine() else { return Ok(Value::None) };
                let Some(sid) = st.stack_id(idx) else { return Ok(Value::None) };
                let Some(ci) = container_index(&kind) else { return Ok(Value::None) };
                let c = m.stack(sid).container(ci);
                let p = match c {
                    curust_settings::stack::Container::Definition(d) => d.get_property(&key, &prop),
                    curust_settings::stack::Container::Instance(inst) => {
                        if prop != "value" || inst.is_empty {
                            None
                        } else {
                            let def = m.reg.definition(&inst.definition);
                            inst.get_value(&key, def.as_deref())
                        }
                    }
                };
                Ok(match p {
                    Some(curust_settings::definition::Prop::Value(v)) => to_py(&v),
                    Some(curust_settings::definition::Prop::Func(f)) => m.call_function(&f, sid, None, None).map(|v| to_py(&v)).unwrap_or(Value::None),
                    None => Value::None,
                })
            }
            "container_keys" => {
                let idx = arg_int(i, &a, 0)?;
                let kind = arg_str(i, &a, 1)?;
                let Some(m) = st.session().machine() else { return Ok(Value::list(Vec::new())) };
                let Some(sid) = st.stack_id(idx) else { return Ok(Value::list(Vec::new())) };
                let keys: Vec<String> = match container_index(&kind).map(|ci| m.stack(sid).container(ci)) {
                    Some(curust_settings::stack::Container::Instance(inst)) => inst.keys(),
                    Some(curust_settings::stack::Container::Definition(_)) => m.all_keys(sid),
                    None => Vec::new(),
                };
                Ok(Value::list(keys.into_iter().map(|k| Value::Str(k.into())).collect()))
            }
            "set_stack_metadata" => {
                let idx = arg_int(i, &a, 0)?;
                let key = arg_str(i, &a, 1)?;
                let v = a.get(2).cloned().unwrap_or(Value::None);
                let mut sm = st.stack_meta.borrow_mut();
                sm.retain(|(s, k, _)| !(*s == idx && *k == key));
                sm.push((idx, key, v));
                Ok(Value::None)
            }
            "extruder_count" => Ok(Value::Int(st.session().machine().map(|m| m.extruder_list().len()).unwrap_or(1) as i64)),
            "active_extruder" => Ok(Value::Int(st.session().active_extruder as i64)),
            "used_extruders" => Ok(Value::list(st.used_extruders.iter().map(|&e| Value::Int(e as i64)).collect())),
            "initial_extruder" => {
                let s = st.session();
                let Some(m) = s.machine() else { return Ok(Value::Int(0)) };
                let g = |k: &str| m.get_value(StackId::Global, k);
                let adhesion = g("adhesion_type").map(|v| v.py_str()).unwrap_or_default();
                let first_used = || -> Value {
                    let mut used = st.used_extruders.clone();
                    used.sort();
                    Value::Int(used.first().copied().unwrap_or(0) as i64)
                };
                if adhesion == "skirt" || adhesion == "brim" {
                    let nr = g("skirt_brim_extruder_nr").map(|v| to_py(&v)).unwrap_or(Value::Int(-1));
                    if matches!(nr, Value::Int(-1)) {
                        return Ok(first_used());
                    }
                    return Ok(nr);
                }
                if adhesion == "raft" {
                    return Ok(g("raft_base_extruder_nr").map(|v| to_py(&v)).unwrap_or(Value::Int(0)));
                }
                let support = g("support_enable").map(|v| v.truthy()).unwrap_or(false) || g("support_structure").map(|v| v.py_str() == "tree").unwrap_or(false);
                if support && g("support_brim_enable").map(|v| v.truthy()).unwrap_or(false) {
                    return Ok(g("support_infill_extruder_nr").map(|v| to_py(&v)).unwrap_or(Value::Int(0)));
                }
                Ok(first_used())
            }
            "print_information" => {
                let n = st.session().machine().map(|m| m.extruder_list().len()).unwrap_or(1);
                let (time, feats, lengths, weights, costs, names) = match &st.print_info {
                    Some(p) => (p.print_time, p.feature_times.clone(), p.material_lengths.clone(), p.material_weights.clone(), p.material_costs.clone(), p.material_names.clone()),
                    None => (0, Vec::new(), vec![0.0; n], vec![0.0; n], vec![0.0; n], vec![String::new(); n]),
                };
                Ok(str_dict(vec![
                    ("jobName".into(), Value::Str(st.job_name.as_str().into())),
                    ("printTime".into(), Value::Int(time)),
                    ("featureTimes".into(), Value::list(feats.into_iter().map(|(k, v)| Value::tuple(vec![Value::Str(k.into()), Value::Float(v)])).collect())),
                    ("materialLengths".into(), Value::list(lengths.into_iter().map(Value::Float).collect())),
                    ("materialWeights".into(), Value::list(weights.into_iter().map(Value::Float).collect())),
                    ("materialCosts".into(), Value::list(costs.into_iter().map(Value::Float).collect())),
                    ("materialNames".into(), Value::list(names.into_iter().map(|s| Value::Str(s.into())).collect())),
                ]))
            }
            "preference" => {
                let key = arg_str(i, &a, 0)?;
                let p = &st.session().prefs;
                Ok(match key.as_str() {
                    "cura/currency" => Value::Str(p.currency.as_str().into()),
                    "cura/jobname_prefix" => Value::Bool(p.jobname_prefix),
                    "general/language" => Value::str("en_US"),
                    "physics/automatically_drop_models" => Value::Bool(p.auto_drop),
                    _ => Value::None,
                })
            }
            "version" => Ok(Value::str(CURA_VERSION)),
            "log" | "log_exception" => {
                let level = arg_str(i, &a, 0)?;
                let msg = arg_str(i, &a, 1)?;
                let mut line = format!("[{}] {}", level, msg);
                if name == "log_exception" {
                    if let Some(e) = i.handling.last().cloned() {
                        let ty = i.exc_type_name(&e);
                        let m = i.exc_message(&e);
                        let ln = match &e {
                            Value::Instance(inst) => match inst.dict.borrow().get("__line__") {
                                Some(Value::Int(n)) => format!(" (line {})", n),
                                _ => String::new(),
                            },
                            _ => String::new(),
                        };
                        line.push_str(&format!(": {}: {}{}", ty, m, ln));
                    }
                }
                st.log.borrow_mut().push(line);
                Ok(Value::None)
            }
            "message" => {
                let title = arg_str(i, &a, 0)?;
                let text = arg_str(i, &a, 1)?;
                st.messages.borrow_mut().push((title, text));
                Ok(Value::None)
            }
            "snapshot" => {
                let w = arg_int(i, &a, 0)?.clamp(1, 2048) as u32;
                let h = arg_int(i, &a, 1)?.clamp(1, 2048) as u32;
                Ok(match st.snapshot.as_ref().and_then(|f| f(w, h)) {
                    Some(png) => Value::Bytes(Rc::new(png)),
                    None => Value::None,
                })
            }
            "literal_eval" => {
                let text = arg_str(i, &a, 0)?;
                literal_eval(i, &text)
            }
            "expr_names" => {
                let code = arg_str(i, &a, 0)?;
                let e = curust_pyrt::parser::parse_expression(&code).map_err(|e| i.exc("SyntaxError", e))?;
                let mut out = Vec::new();
                collect_names(&e, &mut Vec::new(), &mut out);
                for n in &out {
                    if ["sys", "os", "import", "__import__", "eval", "exec", "subprocess"].contains(&n.as_str()) {
                        return Err(i.exc("ValueError", format!("Use of illegal method {}", n)));
                    }
                }
                Ok(Value::list(out.into_iter().map(|n| Value::Str(n.into())).collect()))
            }
            "eval_setting" => {
                let code = arg_str(i, &a, 0)?;
                let e = curust_pyrt::parser::parse_expression(&code).map_err(|e| i.exc("SyntaxError", e))?;
                let mut locals = Namespace::default();
                if let Some(Value::Dict(d)) = a.get(1) {
                    for (k, v) in d.borrow().items() {
                        if let Value::Str(s) = k {
                            locals.insert(s.clone(), v.clone());
                        }
                    }
                }
                let mut globals = Namespace::default();
                for m in ["math", "base64"] {
                    globals.insert(m.into(), i.import_module(m)?);
                }
                globals.insert("debug".into(), curust_pyrt::builtins::builtin("debug", |_i, a, _kw| Ok(a.first().cloned().unwrap_or(Value::None))));
                let mut frame = Frame {
                    locals: Rc::new(RefCell::new(locals)),
                    globals: Rc::new(RefCell::new(globals)),
                    closure: Vec::new(),
                    declared_global: Vec::new(),
                    declared_nonlocal: Vec::new(),
                    is_module: false,
                    is_class: false,
                    method_class: None,
                    first_arg: None,
                };
                i.eval(&e, &mut frame)
            }
            _ => i.type_error(format!("unknown host function {}", name)),
        }
    }
}

fn json_to_py(j: &curust_settings::json::Json) -> Value {
    use curust_settings::json::Json;
    match j {
        Json::Null => Value::None,
        Json::Bool(b) => Value::Bool(*b),
        Json::Int(i) => Value::Int(*i),
        Json::Float(f) => Value::Float(*f),
        Json::Str(s) => Value::Str(s.as_str().into()),
        Json::Array(a) => Value::list(a.iter().map(json_to_py).collect()),
        Json::Object(o) => str_dict(o.iter().map(|(k, v)| (k.clone(), json_to_py(v))).collect()),
    }
}

// ---------------------------------------------------------------------- running

/// Options for a post-processing run.
#[derive(Default)]
pub struct RunContext {
    pub print_info: Option<PrintInfo>,
    pub used_extruders: Vec<usize>,
    pub snapshot: Option<Box<dyn Fn(u32, u32) -> Option<Vec<u8>>>>,
}

/// An interpreter with Cura's script environment and the scripts loaded.
pub struct ScriptEnv {
    pub interp: Interp,
    pub st: Rc<HostState>,
    /// (key, label, module, origin) of each loaded script.
    pub loaded: Vec<(String, String, String, String)>,
    shim: Value,
}

fn exc_text(i: &mut Interp, f: Flow) -> String {
    match f {
        Flow::Exc(e) => {
            let ty = i.exc_type_name(&e);
            let m = i.exc_message(&e);
            let ln = match &e {
                Value::Instance(inst) => match inst.dict.borrow().get("__line__") {
                    Some(Value::Int(n)) => format!(" (line {})", n),
                    _ => String::new(),
                },
                _ => String::new(),
            };
            format!("{}: {}{}", ty, m, ln)
        }
        _ => "unexpected control flow".into(),
    }
}

impl ScriptEnv {
    /// Build the environment and load every script (`loadAllScripts`).
    pub fn new(s: &Session, ctx: RunContext) -> ScriptEnv {
        let st = Rc::new(HostState {
            session: s as *const Session,
            print_info: ctx.print_info,
            job_name: s.job_name(),
            log: RefCell::new(Vec::new()),
            messages: RefCell::new(Vec::new()),
            stack_meta: RefCell::new(Vec::new()),
            snapshot: ctx.snapshot,
            used_extruders: ctx.used_extruders,
        });
        let mut interp = Interp::new();
        interp.host = Some(Rc::new(PpHost { st: st.clone() }));
        interp.sources.insert("Script".into(), SCRIPT_PY.into());
        interp.sources.insert("_curust_shim".into(), SHIM.into());
        let shim = match interp.import_module("_curust_shim") {
            Ok(m) => m,
            Err(f) => {
                let t = exc_text(&mut interp, f);
                st.log.borrow_mut().push(format!("[e] Curust's script environment failed to load: {}", t));
                Value::None
            }
        };
        let mut env = ScriptEnv { interp, st, loaded: Vec::new(), shim };
        env.load_scripts();
        env
    }

    fn shim_fn(&mut self, name: &str) -> R<Value> {
        let shim = self.shim.clone();
        self.interp.get_attr(&shim, name)
    }

    fn load_scripts(&mut self) {
        if self.shim.is_none() {
            return;
        }
        let reserved = ["re", "math", "json", "time", "datetime", "numpy", "typing", "enum", "collections", "base64", "copy", "string", "sys", "functools", "Script"];
        for f in script_files() {
            if reserved.contains(&f.name.as_str()) || self.interp.modules.contains_key(&f.name) {
                continue;
            }
            self.interp.deadline = Some(std::time::Instant::now() + std::time::Duration::from_secs(20));
            let r = (|| -> R<Value> {
                let module = self.interp.run_module(&f.name, &f.source)?;
                let reg = self.shim_fn("_register")?;
                self.interp.call(&reg, vec![Value::Module(module), Value::Str(f.name.as_str().into())], Vec::new())
            })();
            match r {
                Ok(Value::List(l)) => {
                    let l = l.borrow();
                    let key = self.interp.to_str(&l[0]).unwrap_or_default();
                    let label = self.interp.to_str(&l[1]).unwrap_or_default();
                    self.loaded.push((key, label, f.name.clone(), f.origin.clone()));
                }
                Ok(_) => {}
                Err(e) => {
                    let t = exc_text(&mut self.interp, e);
                    self.st.log.borrow_mut().push(format!("[e] Exception occurred while loading post processing plugin: {} ({})", t, f.name));
                }
            }
        }
        self.interp.deadline = None;
    }

    pub fn has_script(&self, key: &str) -> bool {
        self.loaded.iter().any(|(k, _, _, _)| k == key)
    }

    fn settings_value(settings: &[(String, String)]) -> Value {
        Value::list(settings.iter().map(|(k, v)| Value::tuple(vec![Value::Str(k.as_str().into()), Value::Str(v.as_str().into())])).collect())
    }

    /// The JSON `getSettingData()` of a loaded script.
    pub fn definition_json(&mut self, key: &str) -> Result<String, String> {
        let r = (|| -> R<Value> {
            let f = self.shim_fn("_definition_json")?;
            self.interp.call(&f, vec![Value::Str(key.into())], Vec::new())
        })();
        match r {
            Ok(Value::Str(s)) => Ok(s.to_string()),
            Ok(_) => Err("no definition".into()),
            Err(e) => Err(exc_text(&mut self.interp, e)),
        }
    }

    /// `writeScriptsToStack` values for one script: every setting as `str(getSettingValueByKey())`.
    pub fn normalize(&mut self, cfg: &ScriptConfig) -> Result<ScriptConfig, String> {
        let r = (|| -> R<Value> {
            let f = self.shim_fn("_serialize_settings")?;
            self.interp.call(&f, vec![Value::Str(cfg.key.as_str().into()), Self::settings_value(&cfg.settings)], Vec::new())
        })();
        match r {
            Ok(Value::List(l)) => {
                let mut settings = Vec::new();
                for it in l.borrow().iter() {
                    if let Value::Tuple(t) = it {
                        let k = self.interp.to_str(&t[0]).unwrap_or_default();
                        let v = self.interp.to_str(&t[1]).unwrap_or_default();
                        settings.push((k, v));
                    }
                }
                Ok(ScriptConfig { key: cfg.key.clone(), settings })
            }
            Ok(_) => Err("unexpected result".into()),
            Err(e) => Err(exc_text(&mut self.interp, e)),
        }
    }

    /// Current values / enabled state of a configured script's settings, as JSON.
    pub fn state_json(&mut self, cfg: &ScriptConfig) -> Result<curust_settings::json::Json, String> {
        let r = (|| -> R<Value> {
            let f = self.shim_fn("_script_state")?;
            self.interp.call(&f, vec![Value::Str(cfg.key.as_str().into()), Self::settings_value(&cfg.settings)], Vec::new())
        })();
        match r {
            Ok(v) => Ok(py_to_json(&v)),
            Err(e) => Err(exc_text(&mut self.interp, e)),
        }
    }

    /// Run the configured scripts over the G-code chunks (`PostProcessingPlugin.execute`).
    pub fn execute(&mut self, scripts: &[ScriptConfig], gcode: Vec<String>) -> Result<Vec<String>, String> {
        let list = Value::list(scripts.iter().map(|c| Value::tuple(vec![Value::Str(c.key.as_str().into()), Self::settings_value(&c.settings)])).collect());
        let data = Value::list(gcode.into_iter().map(|s| Value::Str(s.into())).collect());
        self.interp.deadline = Some(std::time::Instant::now() + std::time::Duration::from_secs(600));
        let r = (|| -> R<Value> {
            let f = self.shim_fn("_execute")?;
            self.interp.call(&f, vec![list, data], Vec::new())
        })();
        self.interp.deadline = None;
        match r {
            Ok(v) => {
                let items = self.interp.iterate(&v).map_err(|e| exc_text(&mut self.interp, e))?;
                let mut out = Vec::with_capacity(items.len());
                for it in items {
                    match it.unwrap_base() {
                        Value::Str(s) => out.push(s.to_string()),
                        other => return Err(format!("a post-processing script returned a {} instead of G-code text", other.type_name())),
                    }
                }
                Ok(out)
            }
            Err(e) => Err(exc_text(&mut self.interp, e)),
        }
    }
}

/// Result of post-processing a slice.
#[derive(Default, Debug)]
pub struct PostResult {
    pub gcode: Vec<String>,
    pub applied: bool,
    pub log: Vec<String>,
    pub messages: Vec<(String, String)>,
}

/// `PostProcessingPlugin.execute` for the active printer's stored scripts.
pub fn post_process(s: &Session, ctx: RunContext, mut gcode: Vec<String>) -> PostResult {
    let meta = stored_scripts(s);
    let mut res = PostResult::default();
    if meta.is_empty() || gcode.is_empty() {
        res.gcode = gcode;
        return res;
    }
    if gcode[0].contains(";POSTPROCESSED") {
        res.log.push("[e] Already post processed".into());
        res.gcode = gcode;
        return res;
    }
    let scripts = parse_scripts(&meta, &mut res.log);
    let mut env = ScriptEnv::new(s, ctx);
    let known: Vec<ScriptConfig> = scripts.iter().filter(|c| env.has_script(&c.key)).cloned().collect();
    match env.execute(&scripts, gcode.clone()) {
        Ok(out) => gcode = out,
        Err(e) => res.log.push(format!("[e] Post-processing failed: {}", e)),
    }
    if !known.is_empty() && !gcode.is_empty() {
        gcode[0].push_str(";POSTPROCESSED\n");
        for pp_name in meta.split('\n') {
            let first = pp_name.split(']').next().unwrap_or("");
            gcode[0].push_str(&format!(";  {}]\n", first));
        }
        res.applied = true;
    }
    res.log.extend(env.st.log.borrow().iter().cloned());
    res.messages = env.st.messages.borrow().clone();
    res.gcode = gcode;
    res
}

/// Operations on the configured script list (the "Post Processing Plugin" dialog).
pub enum ScriptOp {
    Add(String),
    Remove(usize),
    Move(usize, usize),
    Set(usize, String, String),
    Clear,
}

/// Apply an edit to the dialog's working list (script instances with the values the user set).
pub fn apply_op(list: &mut Vec<ScriptConfig>, op: ScriptOp) {
    match op {
        ScriptOp::Add(key) => list.push(ScriptConfig { key, settings: Vec::new() }),
        ScriptOp::Remove(i) => {
            if i < list.len() {
                list.remove(i);
            }
        }
        ScriptOp::Move(i, j) => {
            if i < list.len() && j < list.len() {
                list.swap(i, j);
            }
        }
        ScriptOp::Set(i, k, v) => {
            if let Some(c) = list.get_mut(i) {
                match c.settings.iter_mut().find(|(n, _)| *n == k) {
                    Some(slot) => slot.1 = v,
                    None => c.settings.push((k, v)),
                }
            }
        }
        ScriptOp::Clear => list.clear(),
    }
}

/// The working list of the stored scripts.
pub fn working_list(s: &Session) -> Vec<ScriptConfig> {
    parse_scripts(&stored_scripts(s), &mut Vec::new())
}

/// `writeScriptsToStack` (when the dialog closes): every setting of every script is stored as
/// `str(getSettingValueByKey(key))`. Returns the new metadata.
pub fn commit_scripts(s: &mut Session, list: &[ScriptConfig]) -> Result<String, String> {
    let meta = {
        let mut env = ScriptEnv::new(s, RunContext::default());
        let mut out = Vec::new();
        for c in list {
            if !env.has_script(&c.key) {
                // Unknown scripts are dropped (Cura can't restore them either).
                continue;
            }
            out.push(env.normalize(c)?);
        }
        serialize_scripts(&out)
    };
    set_stored_scripts(s, &meta);
    Ok(meta)
}

/// Everything the dialog shows: available scripts and the configured ones with their settings.
pub fn dialog_json(s: &Session, list: &[ScriptConfig]) -> curust_settings::json::Json {
    use curust_settings::json::Json;
    let log: Vec<String> = Vec::new();
    let mut env = ScriptEnv::new(s, RunContext::default());
    let mut available: Vec<(String, String, String, String)> = env.loaded.clone();
    available.sort_by(|a, b| a.0.cmp(&b.0));
    let mut defs: Vec<(String, Json)> = Vec::new();
    let mut active = Vec::new();
    for c in list {
        if !defs.iter().any(|(k, _)| *k == c.key) {
            if let Ok(d) = env.definition_json(&c.key) {
                if let Ok(j) = curust_settings::json::parse(&d) {
                    defs.push((c.key.clone(), j));
                }
            }
        }
        let label = env.loaded.iter().find(|x| x.0 == c.key).map(|x| x.1.clone()).unwrap_or_else(|| c.key.clone());
        let state = match env.state_json(c) {
            Ok(j) => j,
            Err(e) => Json::Object(vec![("error".into(), Json::Str(e))]),
        };
        active.push(Json::Object(vec![("key".into(), Json::Str(c.key.clone())), ("label".into(), Json::Str(label)), ("known".into(), Json::Bool(env.has_script(&c.key))), ("settings".into(), state)]));
    }
    let mut all_log = log;
    all_log.extend(env.st.log.borrow().iter().cloned());
    Json::Object(vec![
        (
            "available".into(),
            Json::Array(available.into_iter().map(|(k, l, m, o)| Json::Object(vec![("key".into(), Json::Str(k)), ("label".into(), Json::Str(l)), ("module".into(), Json::Str(m)), ("origin".into(), Json::Str(o))])).collect()),
        ),
        ("definitions".into(), Json::Object(defs)),
        ("scripts".into(), Json::Array(active)),
        ("folders".into(), Json::Array(user_script_dirs().into_iter().map(|p| Json::Str(p.display().to_string())).collect())),
        ("log".into(), Json::Array(all_log.into_iter().map(Json::Str).collect())),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_round_trip() {
        let cfg = vec![ScriptConfig { key: "PauseAtHeight".into(), settings: vec![("pause_at".into(), "height".into()), ("display_text".into(), "a\\\\b\nc".into())] }];
        let s = serialize_scripts(&cfg);
        let mut log = Vec::new();
        let back = parse_scripts(&s, &mut log);
        assert_eq!(back[0].key, "PauseAtHeight");
        assert_eq!(back[0].settings[0], ("pause_at".to_string(), "height".to_string()));
        assert!(log.is_empty());
    }
}
