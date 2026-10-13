//! `InstanceContainer`: profiles stored as `.inst.cfg` (and material profiles converted to the same
//! representation), plus Uranium's `FastConfigParser`.

use crate::definition::DefinitionContainer;
use crate::py::{literal_eval, Function, PyErr, Value};
use std::cell::RefCell;

/// A value stored in an instance container.
#[derive(Clone, Debug)]
pub enum InstValue {
    /// A string from a file; converted with the setting type when read.
    Str(String),
    /// A typed value (set programmatically, or a bool from a material profile).
    Typed(Value),
    /// A formula (`=...`).
    Func(Function),
}

impl InstValue {
    /// `SettingInstance.setProperty` conversion of the stored value: strings starting with `=`
    /// become formulas.
    pub fn from_str(s: &str) -> InstValue {
        if s.trim().starts_with('=') {
            InstValue::Func(Function::new(&s[1..]))
        } else {
            InstValue::Str(s.to_string())
        }
    }

    /// The string form used when saving (`=formula` for functions).
    pub fn serialize(&self) -> String {
        match self {
            InstValue::Str(s) => s.clone(),
            InstValue::Typed(v) => v.py_str(),
            InstValue::Func(f) => f.to_string(),
        }
    }
}

#[derive(Debug)]
pub struct InstanceContainer {
    pub id: String,
    pub name: String,
    /// Definition used to create setting instances (`[general] definition`).
    pub definition: String,
    pub metadata: Vec<(String, Value)>,
    pub values: RefCell<Vec<(String, InstValue)>>,
    /// Empty containers never return anything.
    pub is_empty: bool,
}

/// The `[general]` / `[metadata]` / `[values]` sections of a config file.
pub type CfgSections = Vec<(String, Vec<(String, String)>)>;

impl InstanceContainer {
    pub fn new(id: &str, name: &str, definition: &str) -> InstanceContainer {
        InstanceContainer { id: id.into(), name: name.into(), definition: definition.into(), metadata: Vec::new(), values: RefCell::new(Vec::new()), is_empty: false }
    }

    pub fn empty(id: &str, name: &str, metadata: Vec<(String, Value)>) -> InstanceContainer {
        let mut c = InstanceContainer::new(id, name, "fdmprinter");
        c.metadata = metadata;
        c.is_empty = true;
        c
    }

    pub fn meta(&self, key: &str) -> Option<&Value> {
        self.metadata.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    pub fn meta_str(&self, key: &str) -> Option<String> {
        self.meta(key).map(|v| v.py_str())
    }
    pub fn set_meta(&mut self, key: &str, value: Value) {
        if let Some(slot) = self.metadata.iter_mut().find(|(k, _)| k == key) {
            slot.1 = value;
        } else {
            self.metadata.push((key.to_string(), value));
        }
    }

    pub fn from_cfg(id: &str, text: &str) -> Result<InstanceContainer, String> {
        let sections = parse_cfg(text);
        let get = |sec: &str| sections.iter().find(|(s, _)| s == sec).map(|(_, kv)| kv);
        let general = get("general").ok_or("missing [general]")?;
        let g = |k: &str| general.iter().find(|(x, _)| x == k).map(|(_, v)| v.clone());
        let definition = g("definition").ok_or("missing definition")?;
        g("version").ok_or("missing version")?;
        let name = g("name").unwrap_or_else(|| id.to_string());
        let mut c = InstanceContainer::new(id, &name, &definition);
        if let Some(meta) = get("metadata") {
            for (k, v) in meta {
                c.set_meta(k, Value::str(v));
            }
        }
        if let Some(values) = get("values") {
            let mut vals = c.values.borrow_mut();
            for (k, v) in values {
                if let Some(slot) = vals.iter_mut().find(|(x, _)| x == k) {
                    slot.1 = InstValue::from_str(v);
                } else {
                    vals.push((k.clone(), InstValue::from_str(v)));
                }
            }
        }
        Ok(c)
    }

    pub fn has_value(&self, key: &str) -> bool {
        !self.is_empty && self.values.borrow().iter().any(|(k, _)| k == key)
    }

    pub fn raw_value(&self, key: &str) -> Option<InstValue> {
        if self.is_empty {
            return None;
        }
        self.values.borrow().iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    pub fn set_value(&self, key: &str, value: InstValue) {
        let mut vals = self.values.borrow_mut();
        if let Some(slot) = vals.iter_mut().find(|(k, _)| k == key) {
            slot.1 = value;
        } else {
            vals.push((key.to_string(), value));
        }
    }

    pub fn remove_value(&self, key: &str) -> bool {
        let mut vals = self.values.borrow_mut();
        let before = vals.len();
        vals.retain(|(k, _)| k != key);
        vals.len() != before
    }

    pub fn keys(&self) -> Vec<String> {
        self.values.borrow().iter().map(|(k, _)| k.clone()).collect()
    }

    /// `getProperty(key, "value")` given this container's definition: `None` when the setting has
    /// no value here or is unknown to the definition.
    pub fn get_value(&self, key: &str, definition: Option<&DefinitionContainer>) -> Option<Prop> {
        let raw = self.raw_value(key)?;
        let def = definition?;
        let sdef = def.find(key)?;
        Some(match raw {
            InstValue::Func(f) => Prop::Func(f),
            InstValue::Typed(v) => Prop::Value(v),
            InstValue::Str(s) => Prop::Value(setting_value_from_string(&sdef.setting_type(), &s)),
        })
    }

    /// Serialize as an `.inst.cfg` file.
    pub fn serialize(&self) -> String {
        let mut out = String::new();
        out.push_str("[general]\n");
        out.push_str("version = 4\n");
        out.push_str(&format!("name = {}\n", self.name));
        out.push_str(&format!("definition = {}\n\n", self.definition));
        out.push_str("[metadata]\n");
        for (k, v) in &self.metadata {
            out.push_str(&format!("{} = {}\n", k, v.py_str()));
        }
        out.push_str("\n[values]\n");
        for (k, v) in self.values.borrow().iter() {
            out.push_str(&format!("{} = {}\n", k, v.serialize().replace('\n', "\n\t")));
        }
        out.push('\n');
        out
    }
}

pub use crate::definition::Prop;

/// `toFloatConversion`.
fn to_float_conversion(value: &str) -> Value {
    let value = value.replace(',', ".");
    // Strip leading zeros of integer literals that are not part of a name or a decimal fraction:
    // regex (?<!\.|\w|\d)0+(\d+)
    let chars: Vec<char> = value.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '0' {
            let prev_ok = i == 0 || !(chars[i - 1] == '.' || chars[i - 1].is_alphanumeric() || chars[i - 1] == '_');
            if prev_ok {
                // `0+(\d+)` matches the whole digit run (at least two digits, starting with 0); the
                // replacement strips its leading zeros.
                let mut j = i;
                while j < chars.len() && chars[j].is_ascii_digit() {
                    j += 1;
                }
                if j - i >= 2 {
                    let run: String = chars[i..j].iter().collect();
                    out.push_str(run.trim_start_matches('0'));
                    i = j;
                    continue;
                }
            }
        }
        out.push(c);
        i += 1;
    }
    match literal_eval(&out) {
        Ok(v) => v,
        Err(_) => Value::Int(0),
    }
}

/// `toIntConversion`: `ast.literal_eval`, `0` for syntax errors, other errors propagate.
fn to_int_conversion(value: &str) -> Result<Value, PyErr> {
    match literal_eval(value) {
        Ok(v) => Ok(v),
        Err(e) if e.kind == "SyntaxError" => Ok(Value::Int(0)),
        Err(e) => Err(e),
    }
}

/// `SettingDefinition.settingValueFromString` as used by `SettingInstance.__getattr__` (falling back
/// to the raw string when the conversion raises).
pub fn setting_value_from_string(setting_type: &str, s: &str) -> Value {
    let r: Result<Value, PyErr> = match setting_type {
        "int" | "extruder" | "optional_extruder" => to_int_conversion(s),
        "float" => Ok(to_float_conversion(s)),
        "bool" | "polygon" | "polygons" => literal_eval(s),
        _ => Ok(Value::str(s)),
    };
    r.unwrap_or_else(|_| Value::str(s))
}

/// Uranium's `FastConfigParser` (regex based): sections and their key/value pairs.
pub fn parse_cfg(text: &str) -> CfgSections {
    let data = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut out: CfgSections = Vec::new();
    // header_regex = \[(\w+?)\]\n(.*?)(?:(?=\n\[(?:\w+?)\])|\Z)
    let b = data.as_bytes();
    let is_word = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80;
    let header_at = |i: usize| -> Option<(String, usize)> {
        if b.get(i) != Some(&b'[') {
            return None;
        }
        let mut j = i + 1;
        while j < b.len() && is_word(b[j]) {
            j += 1;
        }
        if j == i + 1 || b.get(j) != Some(&b']') || b.get(j + 1) != Some(&b'\n') {
            return None;
        }
        Some((data[i + 1..j].to_string(), j + 2))
    };
    // `(?=\n\[(\w+?)\])`: a newline followed by a bracketed word (no newline requirement after it).
    let section_end_at = |i: usize| -> bool {
        if b.get(i) != Some(&b'\n') || b.get(i + 1) != Some(&b'[') {
            return false;
        }
        let mut j = i + 2;
        while j < b.len() && is_word(b[j]) {
            j += 1;
        }
        j > i + 2 && b.get(j) == Some(&b']')
    };
    let mut i = 0;
    while i < b.len() {
        if let Some((name, content_start)) = header_at(i) {
            let mut e = content_start;
            while e < b.len() && !section_end_at(e) {
                e += 1;
            }
            let content = &data[content_start..e];
            let pairs = parse_key_values(content.trim_end());
            if let Some(slot) = out.iter_mut().find(|(n, _)| *n == name) {
                slot.1 = pairs;
            } else {
                out.push((name, pairs));
            }
            i = e;
        } else {
            i += 1;
        }
    }
    out
}

/// key_value_regex =
/// `([^=\n !]+)[ \t]*=[ \t]*(.*?)(?:(?=\s+(?:^[^=\n\t !<>\[]+)[ \t]*=[ \t]*[^=])|(?=\n\[)|\Z)` (DOTALL, MULTILINE)
fn parse_key_values(content: &str) -> Vec<(String, String)> {
    let b = content.as_bytes();
    let n = b.len();
    let key_char = |c: u8| c != b'=' && c != b'\n' && c != b' ' && c != b'!';
    let next_key_char = |c: u8| !matches!(c, b'=' | b'\n' | b'\t' | b' ' | b'!' | b'<' | b'>' | b'[');
    let is_ws = |c: u8| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | b'\x0b' | b'\x0c');
    // Does a "key = value" line start at `k` (k at a line start)?
    let key_line_at = |k: usize| -> bool {
        let mut j = k;
        while j < n && next_key_char(b[j]) {
            j += 1;
        }
        if j == k {
            return false;
        }
        while j < n && (b[j] == b' ' || b[j] == b'\t') {
            j += 1;
        }
        if j >= n || b[j] != b'=' {
            return false;
        }
        j += 1;
        // [ \t]*[^=] with backtracking: a space/tab right after '=' always works.
        if j < n && (b[j] == b' ' || b[j] == b'\t') {
            return true;
        }
        j < n && b[j] != b'='
    };
    let lookahead = |e: usize| -> bool {
        if e == n {
            return true;
        }
        if b[e] == b'\n' && e + 1 < n && b[e + 1] == b'[' {
            return true;
        }
        // \s+ then ^ (line start) then a key line.
        let mut k = e;
        while k < n && is_ws(b[k]) {
            k += 1;
            if b[k - 1] == b'\n' && key_line_at(k) {
                return true;
            }
        }
        false
    };
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < n {
        // Try to match at `pos`.
        let mut j = pos;
        while j < n && key_char(b[j]) {
            j += 1;
        }
        if j == pos {
            pos += 1;
            continue;
        }
        let key_end = j;
        while j < n && (b[j] == b' ' || b[j] == b'\t') {
            j += 1;
        }
        if j >= n || b[j] != b'=' {
            pos += 1;
            continue;
        }
        j += 1;
        while j < n && (b[j] == b' ' || b[j] == b'\t') {
            j += 1;
        }
        let v0 = j;
        let mut e = v0;
        while !lookahead(e) {
            e += 1;
        }
        let key = content[pos..key_end].to_string();
        let value = content[v0..e].replace("\n\t", "\n");
        if let Some(slot) = out.iter_mut().find(|(k, _): &&mut (String, String)| *k == key) {
            slot.1 = value;
        } else {
            out.push((key, value));
        }
        pos = if e > pos { e } else { pos + 1 };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cfg() {
        let s = "[general]\nversion = 4\nname = Fine\ndefinition = fdmprinter\n\n[metadata]\nsetting_version = 25\ntype = quality\n\n[values]\ninfill = =20\nstart = G28\n\tG1 X0\nx=1\n";
        let p = parse_cfg(s);
        assert_eq!(p.len(), 3);
        assert_eq!(p[2].1, vec![("infill".to_string(), "=20".to_string()), ("start".to_string(), "G28\nG1 X0".to_string()), ("x".to_string(), "1".to_string())]);
        assert_eq!(p[0].1[1], ("name".to_string(), "Fine".to_string()));
    }
}
