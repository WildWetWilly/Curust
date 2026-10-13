//! Order-preserving JSON reader/writer (Python `json.loads` with `object_pairs_hook=OrderedDict`
//! semantics: integers stay integers, floats become floats, later duplicate keys overwrite earlier
//! ones in place).

use std::fmt::Write as _;

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Json> {
        match self {
            Json::Object(m) => m.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_object(&self) -> Option<&Vec<(String, Json)>> {
        match self {
            Json::Object(m) => Some(m),
            _ => None,
        }
    }
    pub fn as_array(&self) -> Option<&Vec<Json>> {
        match self {
            Json::Array(a) => Some(a),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Int(i) => Some(*i as f64),
            Json::Float(f) => Some(*f),
            _ => None,
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Int(i) => Some(*i),
            _ => None,
        }
    }
    /// Insert or replace (keeping the position of an existing key).
    pub fn set(&mut self, key: &str, value: Json) {
        if let Json::Object(m) = self {
            if let Some(slot) = m.iter_mut().find(|(k, _)| k == key) {
                slot.1 = value;
            } else {
                m.push((key.to_string(), value));
            }
        }
    }
    pub fn remove(&mut self, key: &str) -> Option<Json> {
        if let Json::Object(m) = self {
            if let Some(i) = m.iter().position(|(k, _)| k == key) {
                return Some(m.remove(i).1);
            }
        }
        None
    }

    pub fn to_string_compact(&self) -> String {
        let mut s = String::new();
        self.write(&mut s, None, 0);
        s
    }
    pub fn to_string_pretty(&self) -> String {
        let mut s = String::new();
        self.write(&mut s, Some(2), 0);
        s
    }
    fn write(&self, out: &mut String, indent: Option<usize>, level: usize) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Int(i) => {
                let _ = write!(out, "{}", i);
            }
            Json::Float(f) => {
                if f.is_finite() {
                    out.push_str(&crate::py::float_repr(*f));
                } else {
                    out.push_str("null");
                }
            }
            Json::Str(s) => write_str(out, s),
            Json::Array(a) => {
                out.push('[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    if let Some(n) = indent {
                        out.push('\n');
                        out.push_str(&" ".repeat(n * (level + 1)));
                    }
                    v.write(out, indent, level + 1);
                }
                if let (Some(n), false) = (indent, a.is_empty()) {
                    out.push('\n');
                    out.push_str(&" ".repeat(n * level));
                }
                out.push(']');
            }
            Json::Object(m) => {
                out.push('{');
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    if let Some(n) = indent {
                        out.push('\n');
                        out.push_str(&" ".repeat(n * (level + 1)));
                    }
                    write_str(out, k);
                    out.push(':');
                    if indent.is_some() {
                        out.push(' ');
                    }
                    v.write(out, indent, level + 1);
                }
                if let (Some(n), false) = (indent, m.is_empty()) {
                    out.push('\n');
                    out.push_str(&" ".repeat(n * level));
                }
                out.push('}');
            }
        }
    }
}

pub fn write_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

pub fn parse(text: &str) -> Result<Json, String> {
    let mut p = P { s: text.as_bytes(), i: 0 };
    // Skip a UTF-8 BOM.
    if p.s.starts_with(&[0xEF, 0xBB, 0xBF]) {
        p.i = 3;
    }
    let v = p.value()?;
    p.ws();
    if p.i != p.s.len() {
        return Err(format!("extra data at offset {}", p.i));
    }
    Ok(v)
}

struct P<'a> {
    s: &'a [u8],
    i: usize,
}

impl<'a> P<'a> {
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }
    fn err<T>(&self, m: &str) -> Result<T, String> {
        Err(format!("{} at offset {}", m, self.i))
    }
    fn value(&mut self) -> Result<Json, String> {
        self.ws();
        if self.i >= self.s.len() {
            return self.err("unexpected end");
        }
        match self.s[self.i] {
            b'{' => {
                self.i += 1;
                let mut m: Vec<(String, Json)> = Vec::new();
                self.ws();
                if self.i < self.s.len() && self.s[self.i] == b'}' {
                    self.i += 1;
                    return Ok(Json::Object(m));
                }
                loop {
                    self.ws();
                    if self.i >= self.s.len() || self.s[self.i] != b'"' {
                        return self.err("expected key");
                    }
                    let k = self.string()?;
                    self.ws();
                    if self.i >= self.s.len() || self.s[self.i] != b':' {
                        return self.err("expected ':'");
                    }
                    self.i += 1;
                    let v = self.value()?;
                    if let Some(slot) = m.iter_mut().find(|(x, _)| *x == k) {
                        slot.1 = v;
                    } else {
                        m.push((k, v));
                    }
                    self.ws();
                    if self.i < self.s.len() && self.s[self.i] == b',' {
                        self.i += 1;
                        continue;
                    }
                    if self.i < self.s.len() && self.s[self.i] == b'}' {
                        self.i += 1;
                        return Ok(Json::Object(m));
                    }
                    return self.err("expected ',' or '}'");
                }
            }
            b'[' => {
                self.i += 1;
                let mut a = Vec::new();
                self.ws();
                if self.i < self.s.len() && self.s[self.i] == b']' {
                    self.i += 1;
                    return Ok(Json::Array(a));
                }
                loop {
                    a.push(self.value()?);
                    self.ws();
                    if self.i < self.s.len() && self.s[self.i] == b',' {
                        self.i += 1;
                        continue;
                    }
                    if self.i < self.s.len() && self.s[self.i] == b']' {
                        self.i += 1;
                        return Ok(Json::Array(a));
                    }
                    return self.err("expected ',' or ']'");
                }
            }
            b'"' => Ok(Json::Str(self.string()?)),
            b't' if self.s[self.i..].starts_with(b"true") => {
                self.i += 4;
                Ok(Json::Bool(true))
            }
            b'f' if self.s[self.i..].starts_with(b"false") => {
                self.i += 5;
                Ok(Json::Bool(false))
            }
            b'n' if self.s[self.i..].starts_with(b"null") => {
                self.i += 4;
                Ok(Json::Null)
            }
            b'N' if self.s[self.i..].starts_with(b"NaN") => {
                self.i += 3;
                Ok(Json::Float(f64::NAN))
            }
            b'I' if self.s[self.i..].starts_with(b"Infinity") => {
                self.i += 8;
                Ok(Json::Float(f64::INFINITY))
            }
            b'-' | b'0'..=b'9' => self.number(),
            _ => self.err("unexpected character"),
        }
    }
    fn number(&mut self) -> Result<Json, String> {
        let start = self.i;
        if self.s[self.i] == b'-' {
            self.i += 1;
            if self.s[self.i..].starts_with(b"Infinity") {
                self.i += 8;
                return Ok(Json::Float(f64::NEG_INFINITY));
            }
        }
        let mut is_float = false;
        while self.i < self.s.len() {
            match self.s[self.i] {
                b'0'..=b'9' => {}
                b'.' | b'e' | b'E' | b'+' | b'-' => is_float = true,
                _ => break,
            }
            self.i += 1;
        }
        let t = std::str::from_utf8(&self.s[start..self.i]).unwrap();
        if is_float {
            t.parse::<f64>().map(Json::Float).map_err(|_| format!("bad number {}", t))
        } else {
            match t.parse::<i64>() {
                Ok(v) => Ok(Json::Int(v)),
                Err(_) => t.parse::<f64>().map(Json::Float).map_err(|_| format!("bad number {}", t)),
            }
        }
    }
    fn string(&mut self) -> Result<String, String> {
        self.i += 1;
        let mut out: Vec<u8> = Vec::new();
        loop {
            if self.i >= self.s.len() {
                return self.err("unterminated string");
            }
            let c = self.s[self.i];
            self.i += 1;
            match c {
                b'"' => break,
                b'\\' => {
                    let e = self.s[self.i];
                    self.i += 1;
                    match e {
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'/' => out.push(b'/'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'u' => {
                            let h = std::str::from_utf8(&self.s[self.i..self.i + 4]).map_err(|_| "bad escape".to_string())?;
                            let mut cp = u32::from_str_radix(h, 16).map_err(|_| "bad escape".to_string())?;
                            self.i += 4;
                            if (0xD800..0xDC00).contains(&cp) && self.s[self.i..].starts_with(b"\\u") {
                                let h2 = std::str::from_utf8(&self.s[self.i + 2..self.i + 6]).map_err(|_| "bad escape".to_string())?;
                                let lo = u32::from_str_radix(h2, 16).map_err(|_| "bad escape".to_string())?;
                                if (0xDC00..0xE000).contains(&lo) {
                                    cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                                    self.i += 6;
                                }
                            }
                            let ch = char::from_u32(cp).unwrap_or('\u{fffd}');
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        }
                        _ => return self.err("bad escape"),
                    }
                }
                _ => out.push(c),
            }
        }
        String::from_utf8(out).map_err(|_| "invalid utf-8".to_string())
    }
}
