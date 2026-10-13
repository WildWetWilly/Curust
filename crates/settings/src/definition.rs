//! `SettingDefinition` and `DefinitionContainer` (`*.def.json` with inheritance and overrides).

use crate::json::Json;
use crate::py::{Function, Value};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// A raw property value as returned by `getattr(definition, property)`.
#[derive(Clone, Debug)]
pub enum Prop {
    Value(Value),
    Func(Function),
}

impl Prop {
    pub fn as_value(&self) -> Option<&Value> {
        match self {
            Prop::Value(v) => Some(v),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PropType {
    Any,
    String,
    TranslatedString,
    Function,
}

/// The supported definition properties (Uranium's plus Cura's additions) and their defaults.
pub const PROPERTIES: &[(&str, PropType, bool /*required*/)] = &[
    ("label", PropType::TranslatedString, true),
    ("type", PropType::String, true),
    ("icon", PropType::String, false),
    ("unit", PropType::String, false),
    ("description", PropType::TranslatedString, true),
    ("warning_description", PropType::TranslatedString, false),
    ("error_description", PropType::TranslatedString, false),
    ("default_value", PropType::Any, false),
    ("value", PropType::Function, false),
    ("enabled", PropType::Function, false),
    ("minimum_value", PropType::Function, false),
    ("maximum_value", PropType::Function, false),
    ("minimum_value_warning", PropType::Function, false),
    ("maximum_value_warning", PropType::Function, false),
    ("options", PropType::Any, false),
    ("comments", PropType::String, false),
    ("allow_empty", PropType::Function, false),
    ("is_uuid", PropType::Function, false),
    ("regex_blacklist_pattern", PropType::String, false),
    ("warning_value", PropType::Function, false),
    ("error_value", PropType::Function, false),
    ("force_depends_on_settings", PropType::Any, false),
    ("settable_per_mesh", PropType::Any, false),
    ("settable_per_extruder", PropType::Any, false),
    ("settable_per_meshgroup", PropType::Any, false),
    ("settable_globally", PropType::Any, false),
    ("limit_to_extruder", PropType::Function, false),
    ("resolve", PropType::Function, false),
];

pub fn property_type(name: &str) -> Option<PropType> {
    PROPERTIES.iter().find(|(n, _, _)| *n == name).map(|(_, t, _)| *t)
}

/// The default of a supported property (what `getattr` returns when the property isn't set).
pub fn property_default(name: &str) -> Option<Prop> {
    let v = match name {
        "label" | "type" | "icon" | "unit" | "description" | "warning_description" | "error_description" | "comments"
        | "regex_blacklist_pattern" => Value::str(""),
        "default_value" => Value::Int(0),
        "value" | "minimum_value" | "maximum_value" | "minimum_value_warning" | "maximum_value_warning" | "warning_value"
        | "error_value" | "resolve" => Value::None,
        "enabled" | "allow_empty" => Value::Bool(true),
        "is_uuid" => Value::Bool(false),
        "options" => Value::Dict(Rc::new(Vec::new())),
        "force_depends_on_settings" => Value::list(Vec::new()),
        "settable_per_mesh" | "settable_per_extruder" | "settable_per_meshgroup" | "settable_globally" => Value::Bool(true),
        "limit_to_extruder" => Value::str("-1"),
        _ => return None,
    };
    Some(Prop::Value(v))
}

/// The setting types (`SettingDefinition.__type_definitions` plus Cura's).
pub const SETTING_TYPES: &[&str] = &["int", "bool", "category", "str", "enum", "float", "polygon", "polygons", "vec3", "extruder", "optional_extruder", "[int]"];

/// Convert JSON (as `json.loads` produces it) to a Python value.
pub fn json_to_value(j: &Json) -> Value {
    match j {
        Json::Null => Value::None,
        Json::Bool(b) => Value::Bool(*b),
        Json::Int(i) => Value::Int(*i),
        Json::Float(f) => Value::Float(*f),
        Json::Str(s) => Value::Str(s.as_str().into()),
        Json::Array(a) => Value::list(a.iter().map(json_to_value).collect()),
        Json::Object(m) => Value::Dict(Rc::new(m.iter().map(|(k, v)| (Value::Str(k.as_str().into()), json_to_value(v))).collect())),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelationType {
    RequiresTarget,
    RequiredByTarget,
}

#[derive(Clone, Debug)]
pub struct Relation {
    pub kind: RelationType,
    /// Index of the target definition, or the key of a setting this container doesn't define.
    pub target: Result<usize, String>,
    pub role: &'static str,
}

#[derive(Clone, Debug)]
pub struct SettingDefinition {
    pub key: String,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    props: Vec<(&'static str, Prop)>,
    pub relations: Vec<Relation>,
}

impl SettingDefinition {
    /// `getattr(definition, name)`: the property value or its default; `None` for unknown names.
    pub fn get(&self, name: &str) -> Option<Prop> {
        if let Some((_, p)) = self.props.iter().find(|(n, _)| *n == name) {
            return Some(p.clone());
        }
        property_default(name)
    }
    pub fn has(&self, name: &str) -> bool {
        property_type(name).is_some()
    }
    pub fn str_prop(&self, name: &str) -> String {
        match self.get(name) {
            Some(Prop::Value(Value::Str(s))) => s.to_string(),
            Some(Prop::Value(v)) => v.py_str(),
            Some(Prop::Func(f)) => f.to_string(),
            None => String::new(),
        }
    }
    pub fn setting_type(&self) -> String {
        self.str_prop("type")
    }
    pub fn label(&self) -> String {
        self.str_prop("label")
    }
    /// Enum options in order: (value, label).
    pub fn options(&self) -> Vec<(String, String)> {
        match self.get("options") {
            Some(Prop::Value(Value::Dict(d))) => d.iter().map(|(k, v)| (k.py_str(), v.py_str())).collect(),
            _ => Vec::new(),
        }
    }
    pub fn raw_props(&self) -> &[(&'static str, Prop)] {
        &self.props
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum DefinitionError {
    NotFound(String),
    Invalid(String),
}

impl std::fmt::Display for DefinitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DefinitionError::NotFound(s) => write!(f, "definition not found: {}", s),
            DefinitionError::Invalid(s) => write!(f, "invalid definition: {}", s),
        }
    }
}

/// A loaded definition (`DefinitionContainer`).
#[derive(Debug)]
pub struct DefinitionContainer {
    pub id: String,
    pub name: String,
    /// Metadata, deep-merged over the inheritance chain.
    pub metadata: Json,
    pub defs: Vec<SettingDefinition>,
    /// Top-level (category) definitions in order.
    pub roots: Vec<usize>,
    index: HashMap<String, usize>,
    /// Files of the inheritance chain, from this definition up to the root.
    pub inherited_files: Vec<String>,
}

const DEFINITION_VERSION: i64 = 2;

/// `_mergeDicts`: recursively merge `second` into `first`.
pub fn merge_dicts(first: &mut Json, second: &Json) {
    if let (Json::Object(_), Json::Object(sm)) = (&*first, second) {
        for (k, v) in sm {
            match first.get_mut(k) {
                Some(existing) => {
                    if matches!(v, Json::Object(_)) && matches!(existing, Json::Object(_)) {
                        merge_dicts(existing, v);
                    } else {
                        *existing = v.clone();
                    }
                }
                None => first.set(k, v.clone()),
            }
        }
    }
}

/// `_findInDict`: depth-first search for `key` in nested dicts.
fn find_in_dict<'a>(d: &'a mut Json, key: &str) -> Option<&'a mut Json> {
    let has = d.get(key).is_some();
    if has {
        return d.get_mut(key);
    }
    if let Json::Object(m) = d {
        for (_, v) in m.iter_mut() {
            if matches!(v, Json::Object(_)) {
                if let Some(found) = find_in_dict(v, key) {
                    return Some(found);
                }
            }
        }
    }
    None
}

impl DefinitionContainer {
    /// Load the definition with `id` using `find` to locate `<id>.def.json` files.
    pub fn load(id: &str, find: &dyn Fn(&str) -> Option<std::path::PathBuf>) -> Result<DefinitionContainer, DefinitionError> {
        let mut files = Vec::new();
        let parsed = Self::resolve(id, find, &mut files)?;
        Self::from_merged(id, parsed, files)
    }

    fn load_json(id: &str, find: &dyn Fn(&str) -> Option<std::path::PathBuf>, files: &mut Vec<String>) -> Result<Json, DefinitionError> {
        let path = find(id).ok_or_else(|| DefinitionError::NotFound(id.to_string()))?;
        let text = std::fs::read_to_string(&path).map_err(|e| DefinitionError::Invalid(format!("{}: {}", path.display(), e)))?;
        files.push(path.to_string_lossy().into_owned());
        crate::json::parse(&text).map_err(|e| DefinitionError::Invalid(format!("{}: {}", path.display(), e)))
    }

    fn resolve(id: &str, find: &dyn Fn(&str) -> Option<std::path::PathBuf>, files: &mut Vec<String>) -> Result<Json, DefinitionError> {
        let mut json = Self::load_json(id, find, files)?;
        if let Some(parent) = json.get("inherits").and_then(|v| v.as_str()).map(|s| s.to_string()) {
            let mut inherited = Self::resolve(&parent, find, files)?;
            merge_dicts(&mut inherited, &json);
            json = inherited;
        }
        for field in ["version", "name", "settings", "metadata"] {
            if json.get(field).is_none() {
                return Err(DefinitionError::Invalid(format!("{}: missing required property {}", id, field)));
            }
        }
        if json.get("version").and_then(|v| v.as_i64()) != Some(DEFINITION_VERSION) {
            return Err(DefinitionError::Invalid(format!("{}: unsupported definition version", id)));
        }
        Ok(json)
    }

    pub fn from_merged(id: &str, mut parsed: Json, inherited_files: Vec<String>) -> Result<DefinitionContainer, DefinitionError> {
        // Apply the (merged) overrides.
        if let Some(Json::Object(overrides)) = parsed.get("overrides").cloned() {
            if let Some(settings) = parsed.get_mut("settings") {
                for (key, value) in overrides {
                    if let Some(setting) = find_in_dict(settings, &key) {
                        if let (Json::Object(sm), Json::Object(vm)) = (setting, &value) {
                            for (k, v) in vm {
                                if let Some(slot) = sm.iter_mut().find(|(x, _)| x == k) {
                                    slot.1 = v.clone();
                                } else {
                                    sm.push((k.clone(), v.clone()));
                                }
                            }
                        }
                    }
                }
            }
        }
        let name = parsed.get("name").and_then(|v| v.as_str()).unwrap_or(id).to_string();
        let metadata = parsed.get("metadata").cloned().unwrap_or(Json::Object(Vec::new()));
        let mut c = DefinitionContainer { id: id.to_string(), name, metadata, defs: Vec::new(), roots: Vec::new(), index: HashMap::new(), inherited_files };
        if let Some(Json::Object(settings)) = parsed.get("settings") {
            for (key, value) in settings {
                let idx = c.deserialize_definition(key, value, None)?;
                c.roots.push(idx);
            }
        }
        // Index: first definition of each key in tree order.
        let order = c.tree_order();
        for i in order {
            c.index.entry(c.defs[i].key.clone()).or_insert(i);
        }
        c.update_relations();
        Ok(c)
    }

    fn deserialize_definition(&mut self, key: &str, value: &Json, parent: Option<usize>) -> Result<usize, DefinitionError> {
        let idx = self.defs.len();
        self.defs.push(SettingDefinition { key: key.to_string(), parent, children: Vec::new(), props: Vec::new(), relations: Vec::new() });
        let obj = value.as_object().ok_or_else(|| DefinitionError::Invalid(format!("setting {} is not an object", key)))?;
        let mut props: Vec<(&'static str, Prop)> = Vec::new();
        let mut children: Vec<usize> = Vec::new();
        for (k, v) in obj {
            if k == "children" {
                if let Json::Object(cm) = v {
                    for (ck, cv) in cm {
                        let ci = self.deserialize_definition(ck, cv, Some(idx))?;
                        children.push(ci);
                    }
                }
                continue;
            }
            let Some(&(name, ptype, _)) = PROPERTIES.iter().find(|(n, _, _)| n == k) else {
                continue; // "Unrecognised property" (warning in Uranium)
            };
            if name == "type" {
                if let Some(t) = v.as_str() {
                    if !SETTING_TYPES.contains(&t) {
                        return Err(DefinitionError::Invalid(format!("Type {} is not a correct setting type.", t)));
                    }
                }
            }
            let prop = match ptype {
                PropType::Any => Prop::Value(json_to_value(v)),
                PropType::String | PropType::TranslatedString => Prop::Value(Value::Str(json_to_value(v).py_str().into())),
                PropType::Function => Prop::Func(Function::new(&json_to_value(v).py_str())),
            };
            if let Some(slot) = props.iter_mut().find(|(n, _)| *n == name) {
                slot.1 = prop;
            } else {
                props.push((name, prop));
            }
        }
        for (name, _, required) in PROPERTIES {
            if *required && !props.iter().any(|(n, _)| n == name) {
                return Err(DefinitionError::Invalid(format!("Setting {} is missing required property {}", key, name)));
            }
        }
        self.defs[idx].props = props;
        self.defs[idx].children = children;
        Ok(idx)
    }

    /// All definitions in depth-first tree order.
    pub fn tree_order(&self) -> Vec<usize> {
        let mut out = Vec::with_capacity(self.defs.len());
        fn walk(c: &DefinitionContainer, i: usize, out: &mut Vec<usize>) {
            out.push(i);
            for &ch in &c.defs[i].children {
                walk(c, ch, out);
            }
        }
        for &r in &self.roots {
            walk(self, r, &mut out);
        }
        out
    }

    fn update_relations(&mut self) {
        let fn_props: Vec<&'static str> = PROPERTIES.iter().filter(|(_, t, _)| *t == PropType::Function).map(|(n, _, _)| *n).collect();
        for i in self.tree_order() {
            for &pname in &fn_props {
                let mut deps: Vec<String> = Vec::new();
                if let Some(Prop::Func(f)) = self.defs[i].props.iter().find(|(n, _)| *n == pname).map(|(_, p)| p.clone()) {
                    deps.extend(f.used_keys().iter().cloned());
                }
                if let Some(Prop::Value(Value::List(l))) = self.defs[i].props.iter().find(|(n, _)| *n == "force_depends_on_settings").map(|(_, p)| p.clone()) {
                    for v in l.iter() {
                        deps.push(v.py_str());
                    }
                }
                let mut seen = HashSet::new();
                for dep in deps {
                    if !seen.insert(dep.clone()) {
                        continue;
                    }
                    if dep == self.defs[i].key && pname == "value" {
                        continue;
                    }
                    let other = self.index.get(&dep).copied();
                    self.defs[i].relations.push(Relation { kind: RelationType::RequiresTarget, target: other.ok_or(dep.clone()), role: pname });
                    if let Some(o) = other {
                        self.defs[o].relations.push(Relation { kind: RelationType::RequiredByTarget, target: Ok(i), role: pname });
                    }
                }
            }
        }
    }

    pub fn find(&self, key: &str) -> Option<&SettingDefinition> {
        self.index.get(key).map(|&i| &self.defs[i])
    }
    pub fn find_index(&self, key: &str) -> Option<usize> {
        self.index.get(key).copied()
    }
    pub fn has_key(&self, key: &str) -> bool {
        self.index.contains_key(key)
    }

    /// `DefinitionContainer.getProperty`.
    pub fn get_property(&self, key: &str, property: &str) -> Option<Prop> {
        let d = self.find(key)?;
        let v = d.get(property)?;
        if property == "value" {
            if let Prop::Value(Value::None) = v {
                return d.get("default_value");
            }
        }
        Some(v)
    }

    /// All setting keys (definitions and their descendants).
    pub fn all_keys(&self) -> impl Iterator<Item = &str> {
        self.defs.iter().map(|d| d.key.as_str())
    }

    pub fn metadata_str(&self, key: &str) -> Option<String> {
        match self.metadata.get(key)? {
            Json::Str(s) => Some(s.clone()),
            Json::Bool(b) => Some(if *b { "True".into() } else { "False".into() }),
            Json::Int(i) => Some(i.to_string()),
            Json::Float(f) => Some(crate::py::float_repr(*f)),
            _ => None,
        }
    }

    /// Keys of all settings that (transitively) depend on `key` through `value` /
    /// `limit_to_extruder` formulas (StartSliceJob `_addRelations`).
    pub fn add_relations(&self, key: &str, out: &mut HashSet<String>) {
        let Some(i) = self.find_index(key) else { return };
        let mut stack = vec![i];
        let mut visited = HashSet::new();
        while let Some(i) = stack.pop() {
            if !visited.insert(i) {
                continue;
            }
            for r in &self.defs[i].relations {
                if r.kind == RelationType::RequiresTarget || !(r.role == "value" || r.role == "limit_to_extruder") {
                    continue;
                }
                if let Ok(t) = r.target {
                    out.insert(self.defs[t].key.clone());
                    stack.push(t);
                }
            }
        }
    }
}
