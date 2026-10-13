//! Methods of `str`, with CPython's semantics (character indices, whitespace rules, …).

use crate::ast::Name;
use crate::builtins::to_int;
use crate::interp::{Interp, R};
use crate::ops::chars_len;
use crate::value::*;
use std::rc::Rc;

type Args = Vec<Value>;
type Kw = Vec<(Name, Value)>;

/// `str.isspace()` for one character.
pub fn py_isspace(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

/// Line boundaries of `str.splitlines()`.
fn is_linebreak(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

fn is_cased(c: char) -> bool {
    c.is_lowercase() || c.is_uppercase() || c.to_lowercase().next() != Some(c) || c.to_uppercase().next() != Some(c)
}

/// Byte offset of character index `ci` (clamped).
pub fn char_to_byte(s: &str, ci: usize) -> usize {
    if s.is_ascii() {
        return ci.min(s.len());
    }
    s.char_indices().nth(ci).map(|(b, _)| b).unwrap_or(s.len())
}

/// Character index of byte offset `bi`.
pub fn byte_to_char(s: &str, bi: usize) -> usize {
    if s.is_ascii() {
        return bi;
    }
    s[..bi].chars().count()
}

fn kwarg(kw: &Kw, name: &str) -> Option<Value> {
    kw.iter().find(|(k, _)| &**k == name).map(|(_, v)| v.clone())
}

fn arg(a: &Args, kw: &Kw, i: usize, name: &str) -> Option<Value> {
    a.get(i).cloned().or_else(|| kwarg(kw, name))
}

fn str_arg(i: &mut Interp, v: &Value, what: &str) -> R<Rc<str>> {
    match v.unwrap_base() {
        Value::Str(s) => Ok(s),
        other => i.type_error(format!("{} must be str, not {}", what, other.type_name())),
    }
}

/// Resolve optional `start` / `end` arguments to a character range.
fn range_args(i: &mut Interp, a: &Args, first: usize, len: usize) -> R<(usize, usize)> {
    let get = |i: &mut Interp, k: usize| -> R<Option<i64>> {
        match a.get(k) {
            None | Some(Value::None) => Ok(None),
            Some(v) => Ok(Some(to_int(i, v)?)),
        }
    };
    let n = len as i64;
    let fix = |x: i64| -> usize {
        if x < 0 {
            (x + n).max(0) as usize
        } else {
            x.min(n) as usize
        }
    };
    let start = get(i, first)?.map(fix).unwrap_or(0);
    let end = get(i, first + 1)?.map(fix).unwrap_or(len);
    Ok((start, end))
}

fn s_val(s: String) -> Value {
    Value::Str(s.into())
}

/// Split on runs of whitespace (`str.split()` without a separator).
pub fn split_whitespace(s: &str, maxsplit: i64) -> Vec<String> {
    let mut out = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut k = 0;
    let n = chars.len();
    let mut splits = 0;
    loop {
        while k < n && py_isspace(chars[k]) {
            k += 1;
        }
        if k >= n {
            break;
        }
        if maxsplit >= 0 && splits >= maxsplit {
            // The remainder, with trailing whitespace stripped.
            let mut e = n;
            while e > k && py_isspace(chars[e - 1]) {
                e -= 1;
            }
            out.push(chars[k..e].iter().collect());
            break;
        }
        let start = k;
        while k < n && !py_isspace(chars[k]) {
            k += 1;
        }
        out.push(chars[start..k].iter().collect());
        splits += 1;
    }
    out
}

fn rsplit_whitespace(s: &str, maxsplit: i64) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut k = chars.len();
    let mut splits = 0;
    loop {
        while k > 0 && py_isspace(chars[k - 1]) {
            k -= 1;
        }
        if k == 0 {
            break;
        }
        if maxsplit >= 0 && splits >= maxsplit {
            let mut b = 0;
            while b < k && py_isspace(chars[b]) {
                b += 1;
            }
            out.push(chars[b..k].iter().collect());
            break;
        }
        let end = k;
        while k > 0 && !py_isspace(chars[k - 1]) {
            k -= 1;
        }
        out.push(chars[k..end].iter().collect());
        splits += 1;
    }
    out.reverse();
    out
}

fn strip_chars(s: &str, chars: Option<&str>, left: bool, right: bool) -> String {
    let test = |c: char| match chars {
        Some(cs) => cs.contains(c),
        None => py_isspace(c),
    };
    let mut r: &str = s;
    if left {
        r = r.trim_start_matches(test);
    }
    if right {
        r = r.trim_end_matches(test);
    }
    r.to_string()
}

fn pad(i: &mut Interp, s: &str, a: &Args, how: char) -> R<String> {
    let width = to_int(i, a.first().unwrap_or(&Value::Int(0)))?;
    let fill = match a.get(1) {
        Some(v) => {
            let f = str_arg(i, v, "fill character")?;
            if chars_len(&f) != 1 {
                return i.type_error("The fill character must be exactly one character long");
            }
            f.chars().next().unwrap()
        }
        None => ' ',
    };
    let len = chars_len(s) as i64;
    if width <= len {
        return Ok(s.to_string());
    }
    let total = (width - len) as usize;
    let (l, r) = match how {
        'l' => (0, total),
        'r' => (total, 0),
        _ => {
            // CPython: left = marg/2 + (marg & width & 1)
            let left = total / 2 + (total & width as usize & 1);
            (left, total - left)
        }
    };
    let mut out = String::with_capacity(s.len() + total);
    for _ in 0..l {
        out.push(fill);
    }
    out.push_str(s);
    for _ in 0..r {
        out.push(fill);
    }
    Ok(out)
}

/// Python's `str.replace` (empty `old` inserts between characters).
pub fn py_replace(s: &str, old: &str, new: &str, count: i64) -> String {
    if old.is_empty() {
        let mut out = String::new();
        let mut n = 0;
        let limit = if count < 0 { i64::MAX } else { count };
        if n < limit {
            out.push_str(new);
            n += 1;
        }
        for c in s.chars() {
            out.push(c);
            if n < limit {
                out.push_str(new);
                n += 1;
            }
        }
        return out;
    }
    if count < 0 {
        return s.replace(old, new);
    }
    s.replacen(old, new, count as usize)
}

fn prefix_list(i: &mut Interp, v: &Value) -> R<Vec<Rc<str>>> {
    match v.unwrap_base() {
        Value::Str(s) => Ok(vec![s]),
        Value::Tuple(t) => t.iter().map(|x| str_arg(i, x, "tuple for startswith")).collect(),
        other => i.type_error(format!("startswith first arg must be str or a tuple of str, not {}", other.type_name())),
    }
}

/// `str.title()`.
pub fn py_title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_cased = false;
    for c in s.chars() {
        if prev_cased {
            out.extend(c.to_lowercase());
        } else {
            out.extend(c.to_uppercase());
        }
        prev_cased = is_cased(c) && c.is_alphabetic();
    }
    out
}

pub fn str_method(i: &mut Interp, s: &Rc<str>, name: &str, a: Args, kw: Kw) -> R<Value> {
    let st: &str = s;
    match name {
        "split" | "rsplit" => {
            let sep = arg(&a, &kw, 0, "sep").filter(|v| !v.is_none());
            let maxsplit = match arg(&a, &kw, 1, "maxsplit") {
                Some(v) => to_int(i, &v)?,
                None => -1,
            };
            let parts: Vec<String> = match sep {
                None => {
                    if name == "split" {
                        split_whitespace(st, maxsplit)
                    } else {
                        rsplit_whitespace(st, maxsplit)
                    }
                }
                Some(sv) => {
                    let sep = str_arg(i, &sv, "sep")?;
                    if sep.is_empty() {
                        return i.value_error("empty separator");
                    }
                    if name == "split" {
                        if maxsplit < 0 {
                            st.split(&*sep).map(|x| x.to_string()).collect()
                        } else {
                            st.splitn(maxsplit as usize + 1, &*sep).map(|x| x.to_string()).collect()
                        }
                    } else {
                        let mut v: Vec<String> = if maxsplit < 0 { st.rsplit(&*sep).map(|x| x.to_string()).collect() } else { st.rsplitn(maxsplit as usize + 1, &*sep).map(|x| x.to_string()).collect() };
                        v.reverse();
                        v
                    }
                }
            };
            Ok(Value::list(parts.into_iter().map(s_val).collect()))
        }
        "strip" | "lstrip" | "rstrip" => {
            let chars = match a.first() {
                None | Some(Value::None) => None,
                Some(v) => Some(str_arg(i, v, "strip arg")?),
            };
            Ok(s_val(strip_chars(st, chars.as_deref(), name != "rstrip", name != "lstrip")))
        }
        "startswith" | "endswith" => {
            let prefixes = prefix_list(i, a.first().unwrap_or(&Value::None))?;
            let len = chars_len(st);
            let (b, e) = range_args(i, &a, 1, len)?;
            if b > e {
                return Ok(Value::Bool(false));
            }
            let sub = &st[char_to_byte(st, b)..char_to_byte(st, e)];
            Ok(Value::Bool(prefixes.iter().any(|p| if name == "startswith" { sub.starts_with(&**p) } else { sub.ends_with(&**p) })))
        }
        "find" | "rfind" | "index" | "rindex" | "count" => {
            let sub = str_arg(i, a.first().unwrap_or(&Value::None), "must be str")?;
            let len = chars_len(st);
            let (b, e) = range_args(i, &a, 1, len)?;
            if name == "count" {
                if b > e {
                    return Ok(Value::Int(0));
                }
                let hay = &st[char_to_byte(st, b)..char_to_byte(st, e)];
                if sub.is_empty() {
                    return Ok(Value::Int(chars_len(hay) as i64 + 1));
                }
                return Ok(Value::Int(hay.matches(&*sub).count() as i64));
            }
            let found = if b > e || (e - b) < chars_len(&sub) {
                None
            } else {
                let off = char_to_byte(st, b);
                let hay = &st[off..char_to_byte(st, e)];
                let r = if name.starts_with('r') { hay.rfind(&*sub) } else { hay.find(&*sub) };
                r.map(|p| byte_to_char(st, off + p))
            };
            match found {
                Some(p) => Ok(Value::Int(p as i64)),
                None => {
                    if name.contains("index") {
                        i.value_error("substring not found")
                    } else {
                        Ok(Value::Int(-1))
                    }
                }
            }
        }
        "replace" => {
            let old = str_arg(i, a.first().unwrap_or(&Value::None), "replace() argument 1")?;
            let new = str_arg(i, a.get(1).unwrap_or(&Value::None), "replace() argument 2")?;
            let count = match arg(&a, &kw, 2, "count") {
                Some(v) => to_int(i, &v)?,
                None => -1,
            };
            Ok(s_val(py_replace(st, &old, &new, count)))
        }
        "join" => {
            let items = i.iterate(a.first().unwrap_or(&Value::None))?;
            let mut out = String::new();
            for (k, it) in items.iter().enumerate() {
                if k > 0 {
                    out.push_str(st);
                }
                match it.unwrap_base() {
                    Value::Str(x) => out.push_str(&x),
                    other => return i.type_error(format!("sequence item {}: expected str instance, {} found", k, other.type_name())),
                }
            }
            Ok(s_val(out))
        }
        "upper" => Ok(s_val(st.to_uppercase())),
        "lower" => Ok(s_val(st.to_lowercase())),
        "casefold" => Ok(s_val(st.to_lowercase().replace('ß', "ss"))),
        "swapcase" => Ok(s_val(st.chars().flat_map(|c| -> Vec<char> { if c.is_uppercase() { c.to_lowercase().collect() } else if c.is_lowercase() { c.to_uppercase().collect() } else { vec![c] } }).collect())),
        "capitalize" => {
            let mut cs = st.chars();
            Ok(s_val(match cs.next() {
                Some(c) => {
                    let mut out: String = c.to_uppercase().collect();
                    out.push_str(&cs.as_str().to_lowercase());
                    out
                }
                None => String::new(),
            }))
        }
        "title" => Ok(s_val(py_title(st))),
        "isdigit" | "isnumeric" | "isdecimal" => {
            let f = |c: char| match name {
                "isdecimal" => c.is_ascii_digit() || (c as u32 > 127 && c.is_numeric() && c.to_digit(10).is_some()),
                "isdigit" => c.is_ascii_digit() || (c as u32 > 127 && c.is_numeric()),
                _ => c.is_numeric(),
            };
            Ok(Value::Bool(!st.is_empty() && st.chars().all(|c| f(c) || (c as u32 > 127 && c.is_numeric()))))
        }
        "isalpha" => Ok(Value::Bool(!st.is_empty() && st.chars().all(|c| c.is_alphabetic()))),
        "isalnum" => Ok(Value::Bool(!st.is_empty() && st.chars().all(|c| c.is_alphanumeric()))),
        "isspace" => Ok(Value::Bool(!st.is_empty() && st.chars().all(py_isspace))),
        "isupper" => Ok(Value::Bool(st.chars().any(|c| c.is_uppercase()) && !st.chars().any(|c| c.is_lowercase()))),
        "islower" => Ok(Value::Bool(st.chars().any(|c| c.is_lowercase()) && !st.chars().any(|c| c.is_uppercase()))),
        "istitle" => Ok(Value::Bool(!st.is_empty() && py_title(st) == st && st.chars().any(|c| c.is_uppercase()))),
        "isidentifier" => {
            let mut cs = st.chars();
            Ok(Value::Bool(match cs.next() {
                Some(c) if c == '_' || c.is_alphabetic() => cs.all(|c| c == '_' || c.is_alphanumeric()),
                _ => false,
            }))
        }
        "isprintable" => Ok(Value::Bool(st.chars().all(|c| !c.is_control() && (c == ' ' || !py_isspace(c))))),
        "format" => Ok(s_val(crate::format::str_format(i, st, &a, &kw)?)),
        "format_map" => {
            let m = a.first().cloned().unwrap_or(Value::None);
            Ok(s_val(crate::format::str_format_map(i, st, &m)?))
        }
        "splitlines" => {
            let keep = match arg(&a, &kw, 0, "keepends") {
                Some(v) => i.truthy(&v)?,
                None => false,
            };
            let chars: Vec<char> = st.chars().collect();
            let mut out = Vec::new();
            let mut start = 0;
            let mut k = 0;
            while k < chars.len() {
                if is_linebreak(chars[k]) {
                    let mut end = k + 1;
                    if chars[k] == '\r' && k + 1 < chars.len() && chars[k + 1] == '\n' {
                        end = k + 2;
                    }
                    let line: String = chars[start..if keep { end } else { k }].iter().collect();
                    out.push(s_val(line));
                    start = end;
                    k = end;
                } else {
                    k += 1;
                }
            }
            if start < chars.len() {
                out.push(s_val(chars[start..].iter().collect()));
            }
            Ok(Value::list(out))
        }
        "partition" | "rpartition" => {
            let sep = str_arg(i, a.first().unwrap_or(&Value::None), "sep")?;
            if sep.is_empty() {
                return i.value_error("empty separator");
            }
            let pos = if name == "partition" { st.find(&*sep) } else { st.rfind(&*sep) };
            Ok(Value::tuple(match pos {
                Some(p) => vec![s_val(st[..p].to_string()), Value::Str(sep.clone()), s_val(st[p + sep.len()..].to_string())],
                None => {
                    if name == "partition" {
                        vec![Value::Str(s.clone()), Value::str(""), Value::str("")]
                    } else {
                        vec![Value::str(""), Value::str(""), Value::Str(s.clone())]
                    }
                }
            }))
        }
        "zfill" => {
            let width = to_int(i, a.first().unwrap_or(&Value::Int(0)))?;
            let len = chars_len(st) as i64;
            if width <= len {
                return Ok(Value::Str(s.clone()));
            }
            let fill = "0".repeat((width - len) as usize);
            Ok(s_val(match st.chars().next() {
                Some(c @ ('+' | '-')) => format!("{}{}{}", c, fill, &st[1..]),
                _ => format!("{}{}", fill, st),
            }))
        }
        "ljust" => Ok(s_val(pad(i, st, &a, 'l')?)),
        "rjust" => Ok(s_val(pad(i, st, &a, 'r')?)),
        "center" => Ok(s_val(pad(i, st, &a, 'c')?)),
        "encode" => Ok(Value::Bytes(Rc::new(st.as_bytes().to_vec()))),
        "removeprefix" => {
            let p = str_arg(i, a.first().unwrap_or(&Value::None), "prefix")?;
            Ok(s_val(st.strip_prefix(&*p).unwrap_or(st).to_string()))
        }
        "removesuffix" => {
            let p = str_arg(i, a.first().unwrap_or(&Value::None), "suffix")?;
            Ok(s_val(if p.is_empty() { st.to_string() } else { st.strip_suffix(&*p).unwrap_or(st).to_string() }))
        }
        "expandtabs" => {
            let size = match arg(&a, &kw, 0, "tabsize") {
                Some(v) => to_int(i, &v)?,
                None => 8,
            };
            let mut out = String::new();
            let mut col = 0i64;
            for c in st.chars() {
                match c {
                    '\t' => {
                        if size > 0 {
                            let n = size - col % size;
                            for _ in 0..n {
                                out.push(' ');
                            }
                            col += n;
                        }
                    }
                    '\n' | '\r' => {
                        out.push(c);
                        col = 0;
                    }
                    _ => {
                        out.push(c);
                        col += 1;
                    }
                }
            }
            Ok(s_val(out))
        }
        "maketrans" => {
            // str.maketrans(x[, y[, z]])
            let mut d = Dict::default();
            match (a.first(), a.get(1)) {
                (Some(Value::Dict(m)), None) => {
                    for (k, v) in m.borrow().items() {
                        let key = match k {
                            Value::Str(ks) => Value::Int(ks.chars().next().map(|c| c as i64).unwrap_or(0)),
                            other => other.clone(),
                        };
                        d.insert(hash_key(&key).unwrap(), key, v.clone());
                    }
                }
                (Some(x), Some(y)) => {
                    let (x, y) = (str_arg(i, x, "maketrans")?, str_arg(i, y, "maketrans")?);
                    if chars_len(&x) != chars_len(&y) {
                        return i.value_error("the first two maketrans arguments must have equal length");
                    }
                    for (p, q) in x.chars().zip(y.chars()) {
                        d.insert(HashKey::Int(p as i64), Value::Int(p as i64), Value::Int(q as i64));
                    }
                    if let Some(z) = a.get(2) {
                        let z = str_arg(i, z, "maketrans")?;
                        for c in z.chars() {
                            d.insert(HashKey::Int(c as i64), Value::Int(c as i64), Value::None);
                        }
                    }
                }
                _ => return i.type_error("maketrans() requires a dict or two strings"),
            }
            Ok(Value::dict(d))
        }
        "translate" => {
            let table = a.first().cloned().unwrap_or(Value::None);
            let mut out = String::new();
            for c in st.chars() {
                let k = Value::Int(c as i64);
                match i.get_item(&table, &k) {
                    Ok(Value::None) => {}
                    Ok(Value::Int(n)) => {
                        if let Some(ch) = char::from_u32(n as u32) {
                            out.push(ch);
                        }
                    }
                    Ok(Value::Str(r)) => out.push_str(&r),
                    Ok(_) => return i.type_error("character mapping must return integer, None or str"),
                    Err(crate::interp::Flow::Exc(e)) if i.exc_type_name(&e) == "KeyError" || i.exc_type_name(&e) == "IndexError" => out.push(c),
                    Err(e) => return Err(e),
                }
            }
            Ok(s_val(out))
        }
        "__len__" => Ok(Value::Int(chars_len(st) as i64)),
        "__contains__" => Ok(Value::Bool(i.contains(&Value::Str(s.clone()), a.first().unwrap_or(&Value::None))?)),
        "__getitem__" => i.get_item(&Value::Str(s.clone()), a.first().unwrap_or(&Value::None)),
        _ => Err(i.exc("AttributeError", format!("'str' object has no attribute '{}'", name))),
    }
}
