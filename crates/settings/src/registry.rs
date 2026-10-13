//! `ContainerRegistry`: the metadata of every profile in the resource folders, queries over it, and
//! lazy loading of the containers themselves.

use crate::definition::{json_to_value, DefinitionContainer};
use crate::instance::{InstValue, InstanceContainer};
use crate::json::Json;
use crate::material;
use crate::py::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[derive(Clone, Debug)]
pub enum Source {
    Definition(PathBuf),
    Instance(PathBuf),
    /// A material profile (base or machine/variant specific) from an XML file.
    Material { path: PathBuf, machine: Option<String>, hotend: Option<String>, buildplate: Option<String> },
    Empty,
    /// Created at run time (user containers etc.).
    Runtime,
}

#[derive(Clone, Debug)]
pub struct MetaEntry {
    pub id: String,
    pub source: Source,
    pub meta: Vec<(String, Value)>,
}

impl MetaEntry {
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.meta.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    pub fn get_str(&self, key: &str) -> Option<String> {
        self.get(key).map(|v| v.py_str())
    }
    pub fn container_type(&self) -> &'static str {
        match self.source {
            Source::Definition(_) => "definition",
            _ => "instance",
        }
    }
}

/// Decode a container id from a file name (`urllib.parse.unquote_plus`).
pub fn unquote_plus(s: &str) -> String {
    let s = s.replace('+', " ");
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 3 <= b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

static EMPTY_INDEX: Vec<usize> = Vec::new();

/// Case-insensitive suffix strip (Cura's MIME type database matches suffixes ignoring case).
fn strip_suffix_ci<'a>(name: &'a str, suffix: &str) -> Option<&'a str> {
    if name.len() >= suffix.len() && name.is_char_boundary(name.len() - suffix.len()) && name[name.len() - suffix.len()..].eq_ignore_ascii_case(suffix) {
        Some(&name[..name.len() - suffix.len()])
    } else {
        None
    }
}

pub struct Registry {
    pub roots: Vec<PathBuf>,
    entries: Vec<MetaEntry>,
    by_id: HashMap<String, usize>,
    def_paths: HashMap<String, PathBuf>,
    definitions: RefCell<HashMap<String, Rc<DefinitionContainer>>>,
    instances: RefCell<HashMap<String, Rc<InstanceContainer>>>,
    pub product_id_map: HashMap<String, Vec<String>>,
    /// Entry indices by `type` metadata, and by (`type`, `definition`).
    type_index: HashMap<String, Vec<usize>>,
    type_def_index: HashMap<(String, String), Vec<usize>>,
    /// `ContainerQuery` result cache.
    query_cache: RefCell<HashMap<Vec<(String, String)>, Rc<Vec<usize>>>>,
}

fn exact_query(q: &str) -> bool {
    !q.contains('*') && !(q.starts_with('[') && q.ends_with(']'))
}

fn list_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut items: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    items.sort();
    for p in items {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if name.starts_with('.') {
            continue;
        }
        if p.is_dir() {
            list_files(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn empty_meta(id: &str, name: &str, extra: &[(&str, Value)]) -> MetaEntry {
    let mut meta = vec![
        ("id".to_string(), Value::str(id)),
        ("name".to_string(), Value::str(name)),
        ("version".to_string(), Value::Int(4)),
        ("container_type".to_string(), Value::str("InstanceContainer")),
    ];
    for (k, v) in extra {
        if let Some(slot) = meta.iter_mut().find(|(x, _)| x == k) {
            slot.1 = v.clone();
        } else {
            meta.push((k.to_string(), v.clone()));
        }
    }
    MetaEntry { id: id.to_string(), source: Source::Empty, meta }
}

impl Registry {
    /// Load the metadata of all profiles under the given resource roots (each containing
    /// `definitions/`, `extruders/`, `variants/`, `materials/`, `quality/`, `intent/`, `texts/`).
    pub fn load(roots: &[PathBuf]) -> Registry {
        let mut reg = Registry {
            roots: roots.to_vec(),
            entries: Vec::new(),
            by_id: HashMap::new(),
            def_paths: HashMap::new(),
            definitions: RefCell::new(HashMap::new()),
            instances: RefCell::new(HashMap::new()),
            product_id_map: HashMap::new(),
            type_index: HashMap::new(),
            type_def_index: HashMap::new(),
            query_cache: RefCell::new(HashMap::new()),
        };
        reg.load_product_id_map();
        // Definition files: first directory wins per file name.
        let mut seen_names: HashMap<String, PathBuf> = HashMap::new();
        let mut def_files: Vec<PathBuf> = Vec::new();
        for root in roots {
            for sub in ["definitions", "extruders"] {
                let mut files = Vec::new();
                list_files(&root.join(sub), &mut files);
                for f in files {
                    let name = f.file_name().unwrap().to_string_lossy().into_owned();
                    if strip_suffix_ci(&name, ".def.json").is_none() || seen_names.contains_key(&name) {
                        continue;
                    }
                    seen_names.insert(name.clone(), f.clone());
                    let id = unquote_plus(strip_suffix_ci(&name, ".def.json").unwrap());
                    reg.def_paths.insert(id, f.clone());
                    def_files.push(f);
                }
            }
        }
        // Definition metadata needs the parent's metadata first.
        let mut def_meta_cache: HashMap<String, Option<Vec<(String, Value)>>> = HashMap::new();
        let def_ids: Vec<String> = def_files
            .iter()
            .map(|f| {
                let n = f.file_name().unwrap().to_string_lossy().into_owned();
                unquote_plus(strip_suffix_ci(&n, ".def.json").unwrap_or(&n))
            })
            .collect();
        for (id, path) in def_ids.iter().zip(def_files.iter()) {
            if let Some(meta) = reg.definition_metadata(id, &mut def_meta_cache) {
                reg.push(MetaEntry { id: id.clone(), source: Source::Definition(path.clone()), meta });
            }
        }
        // Empty containers.
        reg.push(empty_meta("empty", "empty", &[]));
        reg.push(empty_meta("empty_definition_changes", "empty", &[("type", Value::str("definition_changes"))]));
        reg.push(empty_meta("empty_variant", "empty", &[("type", Value::str("variant"))]));
        reg.push(empty_meta(
            "empty_material",
            "empty",
            &[
                ("type", Value::str("material")),
                ("base_file", Value::str("empty_material")),
                ("GUID", Value::str("FFFFFFFF-FFFF-FFFF-FFFF-FFFFFFFFFFFF")),
                ("material", Value::str("empty")),
                ("brand", Value::str("empty_brand")),
            ],
        ));
        reg.push(empty_meta(
            "empty_quality",
            "Not supported",
            &[("quality_type", Value::str("not_supported")), ("type", Value::str("quality")), ("supported", Value::Bool(false))],
        ));
        reg.push(empty_meta(
            "empty_quality_changes",
            "empty",
            &[("type", Value::str("quality_changes")), ("quality_type", Value::str("not_supported")), ("intent_category", Value::str("not_supported"))],
        ));
        reg.push(empty_meta("empty_intent", "Default", &[("type", Value::str("intent")), ("intent_category", Value::str("default"))]));

        // Profiles. Cura iterates the container ids as a Python set (hash order); Curust uses the
        // sorted id order, which the oracle reproduces by sorting that set.
        let mut seen_files: HashMap<String, ()> = HashMap::new();
        let mut profile_files: Vec<(String, PathBuf)> = Vec::new();
        for root in roots {
            for sub in ["variants", "materials", "quality", "intent"] {
                let mut files = Vec::new();
                list_files(&root.join(sub), &mut files);
                for f in files {
                    let name = f.file_name().unwrap().to_string_lossy().into_owned();
                    if seen_files.contains_key(&name) {
                        continue;
                    }
                    let stem = if let Some(stem) = strip_suffix_ci(&name, ".inst.cfg") {
                        stem
                    } else if let Some(stem) = strip_suffix_ci(&name, ".xml.fdm_material") {
                        stem
                    } else {
                        continue;
                    };
                    seen_files.insert(name.clone(), ());
                    profile_files.push((unquote_plus(stem), f));
                }
            }
        }
        profile_files.sort();
        for (id, f) in profile_files {
            reg.load_profile_file(&id, &f);
        }
        reg
    }

    /// Read the metadata of one profile file (`.inst.cfg` or `.xml.fdm_material`) into the registry.
    pub fn load_profile_file(&mut self, id: &str, f: &Path) {
        let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let Ok(text) = std::fs::read_to_string(f) else { return };
        if strip_suffix_ci(&name, ".inst.cfg").is_some() {
            let Ok(c) = InstanceContainer::from_cfg(id, &text) else { return };
            let mut meta = vec![
                ("id".to_string(), Value::str(id)),
                ("container_type".to_string(), Value::str("InstanceContainer")),
                ("name".to_string(), Value::str(&c.name)),
                ("version".to_string(), Value::str("4")),
                ("definition".to_string(), Value::str(&c.definition)),
            ];
            for (k, v) in &c.metadata {
                if let Some(slot) = meta.iter_mut().find(|(x, _)| x == k) {
                    slot.1 = v.clone();
                } else {
                    meta.push((k.clone(), v.clone()));
                }
            }
            self.push(MetaEntry { id: id.to_string(), source: Source::Instance(f.to_path_buf()), meta });
        } else {
            for (eid, machine, hotend, buildplate, meta) in material::deserialize_metadata(&text, id, self) {
                self.push(MetaEntry { id: eid, source: Source::Material { path: f.to_path_buf(), machine, hotend, buildplate }, meta });
            }
        }
    }

    fn load_product_id_map(&mut self) {
        for root in &self.roots {
            let p = root.join("texts").join("product_to_id.json");
            let Ok(text) = std::fs::read_to_string(&p) else { continue };
            let mut contents = String::new();
            for line in text.split_inclusive('\n') {
                if line.contains('#') {
                    for n in 1..12 {
                        contents.push_str(&line.replace('#', &n.to_string()));
                    }
                } else {
                    contents.push_str(line);
                }
            }
            if let Ok(Json::Object(m)) = crate::json::parse(&contents) {
                for (k, v) in m {
                    if let Json::Str(s) = v {
                        self.product_id_map.insert(k, vec![s]);
                    }
                }
            }
            break;
        }
    }

    /// `DefinitionContainer.deserializeMetadata` (parent metadata, shallowly updated).
    fn definition_metadata(&self, id: &str, cache: &mut HashMap<String, Option<Vec<(String, Value)>>>) -> Option<Vec<(String, Value)>> {
        if let Some(c) = cache.get(id) {
            return c.clone();
        }
        cache.insert(id.to_string(), None); // recursion guard
        let path = self.def_paths.get(id)?;
        let text = std::fs::read_to_string(path).ok()?;
        let parsed = crate::json::parse(&text).ok()?;
        let mut meta: Vec<(String, Value)> = Vec::new();
        let set = |meta: &mut Vec<(String, Value)>, k: &str, v: Value| {
            if let Some(slot) = meta.iter_mut().find(|(x, _)| x == k) {
                slot.1 = v;
            } else {
                meta.push((k.to_string(), v));
            }
        };
        if let Some(parent) = parsed.get("inherits").and_then(|v| v.as_str()) {
            if let Some(pm) = self.definition_metadata(parent, cache) {
                for (k, v) in pm {
                    set(&mut meta, &k, v);
                }
                set(&mut meta, "inherits", Value::str(parent));
            }
        }
        set(&mut meta, "container_type", Value::str("DefinitionContainer"));
        set(&mut meta, "id", Value::str(id));
        set(&mut meta, "name", json_to_value(parsed.get("name")?));
        set(&mut meta, "version", json_to_value(parsed.get("version")?));
        if let Some(Json::Object(m)) = parsed.get("metadata") {
            for (k, v) in m {
                set(&mut meta, k, json_to_value(v));
            }
        }
        cache.insert(id.to_string(), Some(meta.clone()));
        Some(meta)
    }

    fn push(&mut self, e: MetaEntry) {
        if self.by_id.contains_key(&e.id) {
            return;
        }
        let i = self.entries.len();
        self.by_id.insert(e.id.clone(), i);
        self.entries.push(e);
        self.index_entry(i);
    }

    fn index_entry(&mut self, i: usize) {
        self.query_cache.borrow_mut().clear();
        let e = &self.entries[i];
        let ty = e.get("type").map(|v| v.py_str());
        let def = e.get("definition").map(|v| v.py_str());
        if let Some(ty) = ty {
            self.type_index.entry(ty.clone()).or_default().push(i);
            if let Some(def) = def {
                self.type_def_index.entry((ty, def)).or_default().push(i);
            }
        }
    }

    fn rebuild_index(&mut self) {
        self.type_index.clear();
        self.type_def_index.clear();
        for i in 0..self.entries.len() {
            self.index_entry(i);
        }
    }

    /// Register a container created at run time (user containers, definition changes...).
    pub fn add_runtime(&mut self, c: Rc<InstanceContainer>) {
        let mut meta = vec![("id".to_string(), Value::str(&c.id)), ("name".to_string(), Value::str(&c.name)), ("definition".to_string(), Value::str(&c.definition))];
        meta.extend(c.metadata.iter().cloned());
        if let Some(&i) = self.by_id.get(&c.id) {
            self.entries[i] = MetaEntry { id: c.id.clone(), source: Source::Runtime, meta };
            self.rebuild_index();
            self.query_cache.borrow_mut().clear();
        } else {
            self.push(MetaEntry { id: c.id.clone(), source: Source::Runtime, meta });
        }
        self.instances.borrow_mut().insert(c.id.clone(), c);
    }

    pub fn metadata(&self, id: &str) -> Option<&MetaEntry> {
        self.by_id.get(id).map(|&i| &self.entries[i])
    }

    pub fn all(&self) -> &[MetaEntry] {
        &self.entries
    }

    /// `findContainersMetadata(**kwargs)` with Uranium's string matching.
    pub fn find(&self, filters: &[(&str, &str)]) -> Vec<&MetaEntry> {
        let key: Vec<(String, String)> = filters.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        if let Some(hit) = self.query_cache.borrow().get(&key) {
            return hit.iter().map(|&i| &self.entries[i]).collect();
        }
        let found: Vec<usize> = self.find_uncached(filters).into_iter().map(|e| self.by_id[&e.id]).collect();
        let result = found.iter().map(|&i| &self.entries[i]).collect();
        self.query_cache.borrow_mut().insert(key, Rc::new(found));
        result
    }

    fn find_uncached(&self, filters: &[(&str, &str)]) -> Vec<&MetaEntry> {
        let ty = filters.iter().find(|(k, v)| *k == "type" && exact_query(v)).map(|(_, v)| *v);
        let def = filters.iter().find(|(k, v)| *k == "definition" && exact_query(v)).map(|(_, v)| *v);
        let candidates: Option<&Vec<usize>> = match (ty, def) {
            (Some(t), Some(d)) => Some(self.type_def_index.get(&(t.to_string(), d.to_string())).unwrap_or(&EMPTY_INDEX)),
            (Some(t), None) => Some(self.type_index.get(t).unwrap_or(&EMPTY_INDEX)),
            _ => None,
        };
        let check = |e: &MetaEntry| filters.iter().all(|(k, v)| match_value(e.get(k), v));
        match candidates {
            Some(c) => c.iter().map(|&i| &self.entries[i]).filter(|e| check(e)).collect(),
            None => self.entries.iter().filter(|e| check(e)).collect(),
        }
    }

    pub fn find_definitions(&self, filters: &[(&str, &str)]) -> Vec<&MetaEntry> {
        self.find(filters).into_iter().filter(|e| matches!(e.source, Source::Definition(_))).collect()
    }

    pub fn find_instances(&self, filters: &[(&str, &str)]) -> Vec<&MetaEntry> {
        self.find(filters).into_iter().filter(|e| !matches!(e.source, Source::Definition(_))).collect()
    }

    pub fn definition_path(&self, id: &str) -> Option<PathBuf> {
        self.def_paths.get(id).cloned()
    }

    /// Load (and cache) a definition container.
    pub fn definition(&self, id: &str) -> Option<Rc<DefinitionContainer>> {
        if let Some(d) = self.definitions.borrow().get(id) {
            return Some(d.clone());
        }
        let paths = &self.def_paths;
        let d = DefinitionContainer::load(id, &|i: &str| paths.get(i).cloned()).ok()?;
        let d = Rc::new(d);
        self.definitions.borrow_mut().insert(id.to_string(), d.clone());
        Some(d)
    }

    /// Load (and cache) an instance container (profile, material, empty or run-time container).
    pub fn instance(&self, id: &str) -> Option<Rc<InstanceContainer>> {
        if let Some(c) = self.instances.borrow().get(id) {
            return Some(c.clone());
        }
        let e = self.metadata(id)?;
        let c = match &e.source {
            Source::Instance(p) => {
                let text = std::fs::read_to_string(p).ok()?;
                InstanceContainer::from_cfg(id, &text).ok()?
            }
            Source::Empty => InstanceContainer::empty(id, &e.get_str("name").unwrap_or_default(), e.meta.clone()),
            Source::Material { path, machine, hotend, .. } => {
                let text = std::fs::read_to_string(path).ok()?;
                material::build_container(&text, e, machine.as_deref(), hotend.as_deref(), &self.product_id_map)?
            }
            Source::Definition(_) | Source::Runtime => return None,
        };
        let c = Rc::new(c);
        self.instances.borrow_mut().insert(id.to_string(), c.clone());
        Some(c)
    }

    /// A unique container id based on `base` (`ContainerRegistry.uniqueName`).
    pub fn unique_name(&self, original: &str) -> String {
        self.unique_name_where(original, &|_| false)
    }

    /// `uniqueName` where `also_taken` reports names / ids of containers added at run time.
    pub fn unique_name_where(&self, original: &str, also_taken: &dyn Fn(&str) -> bool) -> String {
        let original = original.replace('*', "");
        let stripped = original.trim();
        let taken = |n: &str| -> bool {
            let lower = n.to_lowercase();
            also_taken(n) || self.entries.iter().any(|e| e.id.to_lowercase() == lower || e.get("name").map(|v| matches!(v, Value::Str(s) if &**s == n)).unwrap_or(false))
        };
        // `(.*?)\s*#\d+$`: strip a trailing "#<number>".
        let mut name = stripped.to_string();
        if let Some(p) = stripped.rfind('#') {
            let digits = &stripped[p + 1..];
            if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
                name = stripped[..p].trim_end().to_string();
            }
        }
        if name.is_empty() {
            name = "Profile".to_string();
        } else if !taken(stripped) {
            return stripped.to_string();
        }
        let mut unique = name.clone();
        let mut i = 1;
        while taken(&unique) {
            i += 1;
            unique = format!("{} #{}", name, i);
        }
        unique
    }

}

/// `ContainerQuery` value matching.
pub fn match_value(v: Option<&Value>, query: &str) -> bool {
    let Some(v) = v else { return false };
    if let Value::Str(s) = v {
        if exact_query(query) {
            return &**s == query;
        }
    }
    let s = v.py_str();
    if query.starts_with('[') && query.ends_with(']') && query.len() >= 2 {
        // OPTIONS_REGEX "[a|b|c]": any of the alternatives.
        return query[1..query.len() - 1].split('|').any(|alt| alt == s);
    }
    if query.contains('*') {
        return wildcard_match(query, &s);
    }
    s == query
}

fn wildcard_match(pattern: &str, s: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut pos = 0;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            if !s.starts_with(part) {
                return false;
            }
            pos = part.len();
        } else if i == parts.len() - 1 {
            return s.len() >= pos + part.len() && s.ends_with(part);
        } else {
            match s[pos..].find(part) {
                Some(f) => pos += f + part.len(),
                None => return false,
            }
        }
    }
    pos == s.len() || parts.len() > 1
}

impl InstanceContainer {
    /// Convenience for tests: a value as a typed InstValue.
    pub fn typed(v: Value) -> InstValue {
        InstValue::Typed(v)
    }
}
