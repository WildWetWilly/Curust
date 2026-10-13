//! `format()`, `str.format()` and `%` formatting with CPython's number formatting rules.

use crate::ast::Name;
use crate::builtins::{to_float, to_int};
use crate::interp::{Interp, R};
use crate::ops::chars_len;
use crate::value::*;

// ---------------------------------------------------------------------- float formatting

/// `%.<prec>e` with a Python exponent (sign and at least two digits).
pub fn fmt_exp(x: f64, prec: usize, upper: bool) -> String {
    let s = format!("{:.*e}", prec, x);
    let (mant, exp) = s.split_once('e').unwrap();
    let e: i32 = exp.parse().unwrap();
    let out = format!("{}e{}{:02}", mant, if e < 0 { '-' } else { '+' }, e.abs());
    if upper {
        out.to_uppercase()
    } else {
        out
    }
}

pub fn fmt_fixed(x: f64, prec: usize) -> String {
    format!("{:.*}", prec, x)
}

/// The `g` presentation type.
pub fn fmt_general(x: f64, prec: usize, alt: bool, upper: bool) -> String {
    let p = if prec == 0 { 1 } else { prec };
    if x == 0.0 {
        let mut out = String::from(if x.is_sign_negative() { "-0" } else { "0" });
        if alt && p > 1 {
            out.push('.');
            for _ in 0..p - 1 {
                out.push('0');
            }
        }
        return out;
    }
    // Exponent after rounding to p significant digits.
    let e_s = format!("{:.*e}", p - 1, x);
    let exp: i32 = e_s.split_once('e').unwrap().1.parse().unwrap();
    let mut out = if exp >= -4 && exp < p as i32 {
        fmt_fixed(x, (p as i32 - 1 - exp) as usize)
    } else {
        fmt_exp(x, p - 1, upper)
    };
    if !alt {
        // Strip trailing zeros of the fraction (and the point).
        let (num, exp_part) = match out.find(['e', 'E']) {
            Some(k) => (out[..k].to_string(), out[k..].to_string()),
            None => (out.clone(), String::new()),
        };
        let num = if num.contains('.') { num.trim_end_matches('0').trim_end_matches('.').to_string() } else { num };
        out = format!("{}{}", num, exp_part);
    } else if !out.contains('.') {
        // '#' keeps the decimal point.
        match out.find(['e', 'E']) {
            Some(k) => out.insert(k, '.'),
            None => out.push('.'),
        }
    }
    out
}

fn special_float(x: f64, upper: bool) -> Option<String> {
    if x.is_nan() {
        Some(if upper { "NAN" } else { "nan" }.into())
    } else if x.is_infinite() {
        Some(if upper { "INF" } else { "inf" }.into())
    } else {
        None
    }
}

// ---------------------------------------------------------------------- format spec

#[derive(Default, Debug)]
struct Spec {
    fill: Option<char>,
    align: Option<char>,
    sign: Option<char>,
    z: bool,
    alt: bool,
    zero: bool,
    width: Option<usize>,
    grouping: Option<char>,
    precision: Option<usize>,
    ty: Option<char>,
}

fn parse_spec(i: &mut Interp, spec: &str) -> R<Spec> {
    let c: Vec<char> = spec.chars().collect();
    let mut s = Spec::default();
    let mut k = 0;
    let is_align = |ch: char| matches!(ch, '<' | '>' | '=' | '^');
    if c.len() >= 2 && is_align(c[1]) {
        s.fill = Some(c[0]);
        s.align = Some(c[1]);
        k = 2;
    } else if !c.is_empty() && is_align(c[0]) {
        s.align = Some(c[0]);
        k = 1;
    }
    if k < c.len() && matches!(c[k], '+' | '-' | ' ') {
        s.sign = Some(c[k]);
        k += 1;
    }
    if k < c.len() && c[k] == 'z' {
        s.z = true;
        k += 1;
    }
    if k < c.len() && c[k] == '#' {
        s.alt = true;
        k += 1;
    }
    if k < c.len() && c[k] == '0' {
        s.zero = true;
        k += 1;
    }
    let mut w = String::new();
    while k < c.len() && c[k].is_ascii_digit() {
        w.push(c[k]);
        k += 1;
    }
    if !w.is_empty() {
        s.width = w.parse().ok();
    }
    if k < c.len() && matches!(c[k], ',' | '_') {
        s.grouping = Some(c[k]);
        k += 1;
    }
    if k < c.len() && c[k] == '.' {
        k += 1;
        let mut p = String::new();
        while k < c.len() && c[k].is_ascii_digit() {
            p.push(c[k]);
            k += 1;
        }
        if p.is_empty() {
            return i.value_error("Format specifier missing precision");
        }
        s.precision = p.parse().ok();
    }
    if k < c.len() {
        s.ty = Some(c[k]);
        k += 1;
    }
    if k < c.len() {
        return i.value_error("Invalid format specifier");
    }
    if s.zero && s.fill.is_none() && s.align.is_none() {
        s.fill = Some('0');
        s.align = Some('=');
    }
    Ok(s)
}

fn group_digits(digits: &str, sep: char, every: usize) -> String {
    let n = digits.len();
    let mut out = String::with_capacity(n + n / every);
    for (k, ch) in digits.chars().enumerate() {
        if k > 0 && (n - k) % every == 0 {
            out.push(sep);
        }
        out.push(ch);
    }
    out
}

/// Pad `sign + prefix + body` per the spec.
fn finish_number(s: &Spec, sign: &str, prefix: &str, body: String, default_align: char, group_every: usize) -> String {
    let width = s.width.unwrap_or(0);
    let align = s.align.unwrap_or(default_align);
    let fill = s.fill.unwrap_or(' ');
    let (int_part, rest) = match body.find(|ch: char| !ch.is_ascii_digit()) {
        Some(k) => (body[..k].to_string(), body[k..].to_string()),
        None => (body.clone(), String::new()),
    };
    let mut int_part = int_part;
    let grouped = |d: &str| -> String {
        match s.grouping {
            Some(g) if !d.is_empty() => group_digits(d, g, group_every),
            _ => d.to_string(),
        }
    };
    if align == '=' && fill == '0' && s.grouping.is_some() {
        // Zero padding is grouped too.
        let need = width.saturating_sub(sign.len() + prefix.len() + chars_len(&rest));
        while grouped(&int_part).len() < need {
            int_part.insert(0, '0');
        }
    }
    let number = format!("{}{}", grouped(&int_part), rest);
    let len = chars_len(sign) + chars_len(prefix) + chars_len(&number);
    if len >= width {
        return format!("{}{}{}", sign, prefix, number);
    }
    let padn = width - len;
    let padding: String = std::iter::repeat(fill).take(padn).collect();
    match align {
        '<' => format!("{}{}{}{}", sign, prefix, number, padding),
        '^' => {
            let l: String = std::iter::repeat(fill).take(padn / 2).collect();
            let r: String = std::iter::repeat(fill).take(padn - padn / 2).collect();
            format!("{}{}{}{}{}", l, sign, prefix, number, r)
        }
        '=' => format!("{}{}{}{}", sign, prefix, padding, number),
        _ => format!("{}{}{}{}", padding, sign, prefix, number),
    }
}

fn pad_str(s: &Spec, text: &str) -> String {
    let width = s.width.unwrap_or(0);
    let len = chars_len(text);
    if len >= width {
        return text.to_string();
    }
    let fill = s.fill.unwrap_or(' ');
    let n = width - len;
    let rep = |k: usize| -> String { std::iter::repeat(fill).take(k).collect() };
    match s.align.unwrap_or('<') {
        '>' => format!("{}{}", rep(n), text),
        '^' => format!("{}{}{}", rep(n / 2), text, rep(n - n / 2)),
        _ => format!("{}{}", text, rep(n)),
    }
}

fn sign_str(neg: bool, s: &Spec) -> &'static str {
    if neg {
        "-"
    } else {
        match s.sign {
            Some('+') => "+",
            Some(' ') => " ",
            _ => "",
        }
    }
}

fn format_int(i: &mut Interp, n: i64, s: &Spec) -> R<String> {
    let ty = s.ty.unwrap_or('d');
    if matches!(ty, 'e' | 'E' | 'f' | 'F' | 'g' | 'G' | '%') {
        return format_float(i, n as f64, s);
    }
    if s.precision.is_some() {
        return i.value_error("Precision not allowed in integer format specifier");
    }
    let neg = n < 0;
    let mag = n.unsigned_abs();
    let (prefix, digits, every) = match ty {
        'd' | 'n' => ("", mag.to_string(), 3),
        'b' => ("0b", format!("{:b}", mag), 4),
        'o' => ("0o", format!("{:o}", mag), 4),
        'x' => ("0x", format!("{:x}", mag), 4),
        'X' => ("0X", format!("{:X}", mag), 4),
        'c' => {
            let ch = char::from_u32(n as u32).unwrap_or('\u{fffd}');
            return Ok(pad_str(&Spec { align: Some(s.align.unwrap_or('>')), fill: s.fill, width: s.width, ..Default::default() }, &ch.to_string()));
        }
        _ => return i.value_error(format!("Unknown format code '{}' for object of type 'int'", ty)),
    };
    let prefix = if s.alt { prefix } else { "" };
    Ok(finish_number(s, sign_str(neg, s), prefix, digits, '>', every))
}

fn format_float(i: &mut Interp, x: f64, s: &Spec) -> R<String> {
    let ty = s.ty;
    let upper = matches!(ty, Some('E' | 'F' | 'G'));
    let mut x = x;
    let neg = x.is_sign_negative() && !(s.z && x == 0.0);
    let body = if let Some(sp) = special_float(x, upper) {
        let mut sp = sp;
        if ty == Some('%') {
            sp.push('%');
        }
        sp
    } else {
        let a = x.abs();
        match ty {
            Some('f' | 'F') => {
                let mut b = fmt_fixed(a, s.precision.unwrap_or(6));
                if s.alt && !b.contains('.') {
                    b.push('.');
                }
                b
            }
            Some('e' | 'E') => {
                let mut b = fmt_exp(a, s.precision.unwrap_or(6), upper);
                if s.alt && !b.contains('.') {
                    let k = b.find(['e', 'E']).unwrap();
                    b.insert(k, '.');
                }
                b
            }
            Some('g' | 'G' | 'n') => fmt_general(a, s.precision.unwrap_or(6), s.alt, upper),
            Some('%') => {
                x *= 100.0;
                let mut b = fmt_fixed(x.abs(), s.precision.unwrap_or(6));
                b.push('%');
                b
            }
            None => match s.precision {
                None => {
                    let r = float_repr(a);
                    r
                }
                Some(p) => {
                    let mut g = fmt_general(a, p, s.alt, false);
                    if !g.contains(['.', 'e', 'E']) && !g.contains("inf") && !g.contains("nan") {
                        g.push_str(".0");
                    }
                    g
                }
            },
            Some(t) => return i.value_error(format!("Unknown format code '{}' for object of type 'float'", t)),
        }
    };
    // A negative value that rounds to zero keeps its sign unless 'z' is given.
    let neg = if s.z && body.trim_start_matches(['0', '.']).trim_end_matches('%').is_empty() { false } else { neg };
    Ok(finish_number(s, sign_str(neg, s), "", body, '>', 3))
}

/// `format(value, spec)`.
pub fn format_value(i: &mut Interp, v: &Value, spec: &str) -> R<String> {
    if let Value::Instance(inst) = v {
        if let Some(m) = inst.class.lookup("__format__") {
            let r = i.call(&m, vec![v.clone(), Value::str(spec)], Vec::new())?;
            return i.to_str(&r);
        }
        if inst.class.is_enum && inst.class.builtin == "Enum" {
            let s = i.to_str(v)?;
            let sp = parse_spec(i, spec)?;
            return Ok(pad_str(&sp, &s));
        }
    }
    let base = v.unwrap_base();
    if let Value::Native(n) = &base {
        if let Some(a) = n.as_any().downcast_ref::<crate::numpy::NdArray>() {
            if a.data.borrow().len() == 1 && a.shape.borrow().is_empty() {
                let x = a.data.borrow()[0];
                return format_value(i, &Value::Float(x), spec);
            }
        }
    }
    if spec.is_empty() {
        return i.to_str(v);
    }
    let s = parse_spec(i, spec)?;
    match base {
        Value::Bool(b) if s.ty.is_none() => Ok(pad_str(&s, if b { "True" } else { "False" })),
        Value::Int(n) => format_int(i, n, &s),
        Value::Bool(b) => format_int(i, b as i64, &s),
        Value::Float(x) => {
            if matches!(s.ty, Some('d' | 'b' | 'o' | 'x' | 'X' | 'c')) {
                return i.value_error(format!("Unknown format code '{}' for object of type 'float'", s.ty.unwrap()));
            }
            format_float(i, x, &s)
        }
        Value::Str(st) => {
            if !matches!(s.ty, None | Some('s')) {
                return i.value_error(format!("Unknown format code '{}' for object of type 'str'", s.ty.unwrap()));
            }
            if s.sign.is_some() {
                return i.value_error("Sign not allowed in string format specifier");
            }
            let text: String = match s.precision {
                Some(p) => st.chars().take(p).collect(),
                None => st.to_string(),
            };
            Ok(pad_str(&s, &text))
        }
        other => {
            let text = i.to_str(&other)?;
            if s.ty.is_some() || s.precision.is_some() {
                return i.type_error(format!("unsupported format string passed to {}.__format__", other.type_name()));
            }
            Ok(pad_str(&s, &text))
        }
    }
}

// ---------------------------------------------------------------------- str.format

enum ArgSource<'a> {
    Pos(&'a [Value], &'a [(Name, Value)]),
    Map(&'a Value),
}

fn lookup_field(i: &mut Interp, field: &str, auto: &mut usize, src: &ArgSource) -> R<Value> {
    // arg_name ("." attr | "[" key "]")*
    let end = field.find(['.', '[']).unwrap_or(field.len());
    let name = &field[..end];
    let mut v = match src {
        ArgSource::Pos(args, kw) => {
            if name.is_empty() {
                let k = *auto;
                *auto += 1;
                match args.get(k) {
                    Some(v) => v.clone(),
                    None => return Err(i.exc("IndexError", format!("Replacement index {} out of range for positional args tuple", k))),
                }
            } else if let Ok(k) = name.parse::<usize>() {
                match args.get(k) {
                    Some(v) => v.clone(),
                    None => return Err(i.exc("IndexError", format!("Replacement index {} out of range for positional args tuple", k))),
                }
            } else {
                match kw.iter().find(|(k, _)| &**k == name) {
                    Some((_, v)) => v.clone(),
                    None => return Err(i.exc("KeyError", str_repr(name))),
                }
            }
        }
        ArgSource::Map(m) => i.get_item(m, &Value::str(name))?,
    };
    let rest: Vec<char> = field[end..].chars().collect();
    let mut k = 0;
    while k < rest.len() {
        if rest[k] == '.' {
            let mut a = String::new();
            k += 1;
            while k < rest.len() && rest[k] != '.' && rest[k] != '[' {
                a.push(rest[k]);
                k += 1;
            }
            v = i.get_attr(&v, &a)?;
        } else if rest[k] == '[' {
            let mut key = String::new();
            k += 1;
            while k < rest.len() && rest[k] != ']' {
                key.push(rest[k]);
                k += 1;
            }
            k += 1;
            let kv = match key.parse::<i64>() {
                Ok(n) => Value::Int(n),
                Err(_) => Value::Str(key.into()),
            };
            v = i.get_item(&v, &kv)?;
        } else {
            return i.value_error("Only '.' or '[' may follow ']' in format field specifier");
        }
    }
    Ok(v)
}

fn format_impl(i: &mut Interp, fmt: &str, src: &ArgSource, auto: &mut usize, depth: usize) -> R<String> {
    let c: Vec<char> = fmt.chars().collect();
    let mut out = String::new();
    let mut k = 0;
    while k < c.len() {
        match c[k] {
            '{' => {
                if k + 1 < c.len() && c[k + 1] == '{' {
                    out.push('{');
                    k += 2;
                    continue;
                }
                // Find the matching '}' (format specs may nest one level).
                let mut level = 1;
                let mut j = k + 1;
                while j < c.len() {
                    if c[j] == '{' {
                        level += 1;
                    } else if c[j] == '}' {
                        level -= 1;
                        if level == 0 {
                            break;
                        }
                    }
                    j += 1;
                }
                if j >= c.len() {
                    return i.value_error("expected '}' before end of string");
                }
                let inner: String = c[k + 1..j].iter().collect();
                k = j + 1;
                // Split field[!conv][:spec]; '[' ... ']' may contain ':' or '!'.
                let ic: Vec<char> = inner.chars().collect();
                let mut p = 0;
                let mut in_brackets = false;
                while p < ic.len() {
                    match ic[p] {
                        '[' => in_brackets = true,
                        ']' => in_brackets = false,
                        '!' | ':' if !in_brackets => break,
                        _ => {}
                    }
                    p += 1;
                }
                let field: String = ic[..p].iter().collect();
                let mut conv = None;
                let mut spec = String::new();
                if p < ic.len() && ic[p] == '!' {
                    conv = ic.get(p + 1).copied();
                    p += 2;
                }
                if p < ic.len() && ic[p] == ':' {
                    spec = ic[p + 1..].iter().collect();
                } else if p < ic.len() {
                    return i.value_error("expected ':' after conversion specifier");
                }
                let v = lookup_field(i, &field, auto, src)?;
                let v = match conv {
                    Some('r') | Some('a') => Value::Str(i.repr(&v)?.into()),
                    Some('s') => Value::Str(i.to_str(&v)?.into()),
                    None => v,
                    Some(ch) => return i.value_error(format!("Unknown conversion specifier {}", ch)),
                };
                let spec = if spec.contains('{') && depth < 2 { format_impl(i, &spec, src, auto, depth + 1)? } else { spec };
                out.push_str(&format_value(i, &v, &spec)?);
            }
            '}' => {
                if k + 1 < c.len() && c[k + 1] == '}' {
                    out.push('}');
                    k += 2;
                } else {
                    return i.value_error("Single '}' encountered in format string");
                }
            }
            ch => {
                out.push(ch);
                k += 1;
            }
        }
    }
    Ok(out)
}

pub fn str_format(i: &mut Interp, fmt: &str, args: &[Value], kw: &[(Name, Value)]) -> R<String> {
    let mut auto = 0;
    format_impl(i, fmt, &ArgSource::Pos(args, kw), &mut auto, 0)
}

pub fn str_format_map(i: &mut Interp, fmt: &str, mapping: &Value) -> R<String> {
    let mut auto = 0;
    format_impl(i, fmt, &ArgSource::Map(mapping), &mut auto, 0)
}

// ---------------------------------------------------------------------- printf-style

pub fn percent_format(i: &mut Interp, fmt: &str, args: &Value) -> R<String> {
    let c: Vec<char> = fmt.chars().collect();
    let (items, mapping): (Vec<Value>, Option<Value>) = match args {
        Value::Tuple(t) => ((**t).clone(), None),
        Value::Dict(_) => (vec![args.clone()], Some(args.clone())),
        other => {
            if let Value::Instance(inst) = other {
                if let Value::Tuple(t) = &*inst.base.borrow() {
                    if !inst.class.is_enum {
                        return percent_format(i, fmt, &Value::Tuple(t.clone()));
                    }
                }
            }
            (vec![other.clone()], None)
        }
    };
    let mut next = 0;
    let mut out = String::new();
    let mut k = 0;
    let mut used_mapping = false;
    while k < c.len() {
        if c[k] != '%' {
            out.push(c[k]);
            k += 1;
            continue;
        }
        k += 1;
        if k >= c.len() {
            return i.value_error("incomplete format");
        }
        // Mapping key.
        let mut keyed: Option<Value> = None;
        if c[k] == '(' {
            let mut depth = 1;
            let mut j = k + 1;
            while j < c.len() && depth > 0 {
                if c[j] == '(' {
                    depth += 1;
                } else if c[j] == ')' {
                    depth -= 1;
                }
                j += 1;
            }
            let key: String = c[k + 1..j - 1].iter().collect();
            let m = match &mapping {
                Some(m) => m.clone(),
                None => return i.type_error("format requires a mapping"),
            };
            keyed = Some(i.get_item(&m, &Value::Str(key.into()))?);
            used_mapping = true;
            k = j;
        }
        let mut flags = String::new();
        while k < c.len() && matches!(c[k], '-' | '+' | ' ' | '#' | '0') {
            flags.push(c[k]);
            k += 1;
        }
        let mut take_arg = |i: &mut Interp, next: &mut usize| -> R<Value> {
            if let Some(v) = keyed.take() {
                return Ok(v);
            }
            match items.get(*next) {
                Some(v) => {
                    *next += 1;
                    Ok(v.clone())
                }
                None => i.type_error("not enough arguments for format string"),
            }
        };
        let mut width: Option<usize> = None;
        if k < c.len() && c[k] == '*' {
            let w = take_arg(i, &mut next)?;
            let w = to_int(i, &w)?;
            if w < 0 {
                flags.push('-');
            }
            width = Some(w.unsigned_abs() as usize);
            k += 1;
        } else {
            let mut w = String::new();
            while k < c.len() && c[k].is_ascii_digit() {
                w.push(c[k]);
                k += 1;
            }
            if !w.is_empty() {
                width = w.parse().ok();
            }
        }
        let mut prec: Option<usize> = None;
        if k < c.len() && c[k] == '.' {
            k += 1;
            if k < c.len() && c[k] == '*' {
                let p = take_arg(i, &mut next)?;
                prec = Some(to_int(i, &p)?.max(0) as usize);
                k += 1;
            } else {
                let mut p = String::new();
                while k < c.len() && c[k].is_ascii_digit() {
                    p.push(c[k]);
                    k += 1;
                }
                prec = Some(p.parse().unwrap_or(0));
            }
        }
        // Length modifiers are ignored.
        while k < c.len() && matches!(c[k], 'h' | 'l' | 'L') {
            k += 1;
        }
        if k >= c.len() {
            return i.value_error("incomplete format");
        }
        let ty = c[k];
        k += 1;
        if ty == '%' {
            out.push('%');
            continue;
        }
        let v = take_arg(i, &mut next)?;
        let left = flags.contains('-');
        let zero = flags.contains('0') && !left;
        let spec = Spec {
            fill: if zero { Some('0') } else { Some(' ') },
            align: Some(if left { '<' } else if zero { '=' } else { '>' }),
            sign: if flags.contains('+') { Some('+') } else if flags.contains(' ') { Some(' ') } else { None },
            alt: flags.contains('#'),
            width,
            precision: prec,
            ..Default::default()
        };
        let piece = match ty {
            's' | 'r' | 'a' => {
                let s = if ty == 's' { i.to_str(&v)? } else { i.repr(&v)? };
                let s: String = match prec {
                    Some(p) => s.chars().take(p).collect(),
                    None => s,
                };
                pad_str(&Spec { fill: Some(' '), align: Some(if left { '<' } else { '>' }), width, ..Default::default() }, &s)
            }
            'd' | 'i' | 'u' => {
                let n = match v.unwrap_base() {
                    Value::Int(n) => n,
                    Value::Bool(b) => b as i64,
                    Value::Float(f) => {
                        if !f.is_finite() {
                            return Err(i.exc("OverflowError", "cannot convert float infinity to integer"));
                        }
                        f.trunc() as i64
                    }
                    other => {
                        if let Value::Native(_) = other {
                            to_float(i, &other)?.trunc() as i64
                        } else {
                            return i.type_error(format!("%{} format: a real number is required, not {}", ty, other.type_name()));
                        }
                    }
                };
                // Precision gives a minimum number of digits.
                let mut digits = n.unsigned_abs().to_string();
                if let Some(p) = prec {
                    while digits.len() < p {
                        digits.insert(0, '0');
                    }
                }
                finish_number(&Spec { precision: None, ..spec }, sign_str(n < 0, &Spec { sign: if flags.contains('+') { Some('+') } else if flags.contains(' ') { Some(' ') } else { None }, ..Default::default() }), "", digits, '>', 3)
            }
            'x' | 'X' | 'o' => {
                let n = match v.unwrap_base() {
                    Value::Int(n) => n,
                    Value::Bool(b) => b as i64,
                    other => return i.type_error(format!("%{} format: an integer is required, not {}", ty, other.type_name())),
                };
                let mut digits = match ty {
                    'x' => format!("{:x}", n.unsigned_abs()),
                    'X' => format!("{:X}", n.unsigned_abs()),
                    _ => format!("{:o}", n.unsigned_abs()),
                };
                if let Some(p) = prec {
                    while digits.len() < p {
                        digits.insert(0, '0');
                    }
                }
                let prefix = if spec.alt {
                    match ty {
                        'x' => "0x",
                        'X' => "0X",
                        _ => "0o",
                    }
                } else {
                    ""
                };
                let sign = sign_str(n < 0, &spec);
                finish_number(&Spec { precision: None, alt: false, ..spec }, sign, prefix, digits, '>', 3)
            }
            'e' | 'E' | 'f' | 'F' | 'g' | 'G' => {
                let x = match v.unwrap_base() {
                    Value::Str(_) => return i.type_error(format!("must be real number, not str")),
                    other => to_float(i, &other)?,
                };
                let sp = Spec { ty: Some(ty), precision: Some(prec.unwrap_or(6)), ..spec };
                format_float(i, x, &sp)?
            }
            'c' => {
                let ch = match v.unwrap_base() {
                    Value::Int(n) => char::from_u32(n as u32).unwrap_or('\u{fffd}').to_string(),
                    Value::Str(s) if chars_len(&s) == 1 => s.to_string(),
                    _ => return i.type_error("%c requires int or char"),
                };
                pad_str(&Spec { fill: Some(' '), align: Some(if left { '<' } else { '>' }), width, ..Default::default() }, &ch)
            }
            other => return i.value_error(format!("unsupported format character '{}' (0x{:x})", other, other as u32)),
        };
        out.push_str(&piece);
    }
    if !used_mapping && mapping.is_none() && next < items.len() {
        return i.type_error("not all arguments converted during string formatting");
    }
    Ok(out)
}
