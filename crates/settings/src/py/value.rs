//! Python values with CPython 3.12 semantics for the operations setting formulas use: `str()` /
//! `repr()` (including the shortest float repr), equality, ordering and truthiness.

use std::cmp::Ordering;
use std::fmt::Write as _;
use std::rc::Rc;

/// A Python exception: (type name, message).
#[derive(Clone, Debug, PartialEq)]
pub struct PyErr {
    pub kind: &'static str,
    pub msg: String,
}

impl PyErr {
    pub fn new(kind: &'static str, msg: impl Into<String>) -> Self {
        PyErr { kind, msg: msg.into() }
    }
    pub fn type_err(msg: impl Into<String>) -> Self {
        Self::new("TypeError", msg)
    }
    pub fn value_err(msg: impl Into<String>) -> Self {
        Self::new("ValueError", msg)
    }
}

impl std::fmt::Display for PyErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind, self.msg)
    }
}

pub type PyResult<T> = Result<T, PyErr>;

/// Built-in callables reachable from formulas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    Abs,
    All,
    Any,
    Bool,
    Divmod,
    Float,
    Int,
    Len,
    Max,
    Min,
    Map,
    Pow,
    Round,
    Str,
    Sum,
    Hash,
    // Other builtins (blacklisted by Uranium, so never callable from a valid formula; kept so
    // that names resolve to *something*).
    List,
    Tuple,
    Dict,
    Range,
    Sorted,
    Reversed,
    Enumerate,
    Zip,
    Isinstance,
    Set,
    Repr,
    Chr,
    Ord,
    Other,
}

/// `math` module functions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MathFn {
    Ceil,
    Floor,
    Sqrt,
    Log,
    Log10,
    Log2,
    Exp,
    Tan,
    Cos,
    Sin,
    Atan,
    Atan2,
    Acos,
    Asin,
    Radians,
    Degrees,
    Fabs,
    Hypot,
    Pow,
    Trunc,
    Isclose,
    Copysign,
    Fmod,
}

#[derive(Clone, Debug)]
pub enum Value {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Rc<str>),
    List(Rc<Vec<Value>>),
    Tuple(Rc<Vec<Value>>),
    Dict(Rc<Vec<(Value, Value)>>),
    Set(Rc<Vec<Value>>),
    Builtin(Builtin),
    MathModule,
    Math(MathFn),
    /// A method bound to a value (`'abc'.lower`, `(1, 2).index`).
    Method(Rc<Value>, Rc<str>),
    /// A function provided by the evaluation environment (Cura's formula functions, `debug`).
    Operator(Rc<str>),
    /// A user-defined `lambda`.
    Lambda(Rc<(Vec<Rc<str>>, super::ast::Expr)>),
    /// Any other object a formula can name but not use meaningfully (modules like `uuid`, classes...).
    Opaque(Rc<str>),
}

impl Value {
    pub fn str(s: &str) -> Value {
        Value::Str(s.into())
    }
    pub fn list(v: Vec<Value>) -> Value {
        Value::List(Rc::new(v))
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::None => "NoneType",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Str(_) => "str",
            Value::List(_) => "list",
            Value::Tuple(_) => "tuple",
            Value::Dict(_) => "dict",
            Value::Set(_) => "set",
            Value::Builtin(_) => "builtin_function_or_method",
            Value::MathModule => "module",
            Value::Math(_) => "builtin_function_or_method",
            Value::Method(..) => "builtin_function_or_method",
            Value::Operator(_) => "method",
            Value::Lambda(_) => "function",
            Value::Opaque(_) => "object",
        }
    }

    pub fn is_none(&self) -> bool {
        matches!(self, Value::None)
    }

    /// Python truthiness.
    pub fn truthy(&self) -> bool {
        match self {
            Value::None => false,
            Value::Bool(b) => *b,
            Value::Int(i) => *i != 0,
            Value::Float(f) => *f != 0.0,
            Value::Str(s) => !s.is_empty(),
            Value::List(v) | Value::Tuple(v) | Value::Set(v) => !v.is_empty(),
            Value::Dict(d) => !d.is_empty(),
            _ => true,
        }
    }

    /// Numeric view: (is_float, int value, float value).
    pub fn as_number(&self) -> Option<Num> {
        match self {
            Value::Bool(b) => Some(Num::Int(*b as i64)),
            Value::Int(i) => Some(Num::Int(*i)),
            Value::Float(f) => Some(Num::Float(*f)),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        self.as_number().map(|n| n.to_f64())
    }

    /// Items of an iterable (list, tuple, str characters, dict keys, set).
    pub fn iter_items(&self) -> PyResult<Vec<Value>> {
        match self {
            Value::List(v) | Value::Tuple(v) | Value::Set(v) => Ok(v.as_ref().clone()),
            Value::Dict(d) => Ok(d.iter().map(|(k, _)| k.clone()).collect()),
            Value::Str(s) => Ok(s.chars().map(|c| Value::Str(c.to_string().into())).collect()),
            _ => Err(PyErr::type_err(format!("'{}' object is not iterable", self.type_name()))),
        }
    }

    /// Python `str(value)`.
    pub fn py_str(&self) -> String {
        match self {
            Value::Str(s) => s.to_string(),
            _ => self.py_repr(),
        }
    }

    /// Python `repr(value)`.
    pub fn py_repr(&self) -> String {
        let mut out = String::new();
        self.write_repr(&mut out);
        out
    }

    fn write_repr(&self, out: &mut String) {
        match self {
            Value::None => out.push_str("None"),
            Value::Bool(true) => out.push_str("True"),
            Value::Bool(false) => out.push_str("False"),
            Value::Int(i) => {
                let _ = write!(out, "{}", i);
            }
            Value::Float(f) => out.push_str(&float_repr(*f)),
            Value::Str(s) => out.push_str(&str_repr(s)),
            Value::List(v) => {
                out.push('[');
                for (i, x) in v.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    x.write_repr(out);
                }
                out.push(']');
            }
            Value::Tuple(v) => {
                out.push('(');
                for (i, x) in v.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    x.write_repr(out);
                }
                if v.len() == 1 {
                    out.push(',');
                }
                out.push(')');
            }
            Value::Set(v) => {
                if v.is_empty() {
                    out.push_str("set()");
                    return;
                }
                out.push('{');
                for (i, x) in v.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    x.write_repr(out);
                }
                out.push('}');
            }
            Value::Dict(d) => {
                out.push('{');
                for (i, (k, v)) in d.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    k.write_repr(out);
                    out.push_str(": ");
                    v.write_repr(out);
                }
                out.push('}');
            }
            Value::Builtin(b) => {
                let _ = write!(out, "<built-in function {:?}>", b);
            }
            Value::MathModule => out.push_str("<module 'math' (built-in)>"),
            Value::Math(m) => {
                let _ = write!(out, "<built-in function {:?}>", m);
            }
            Value::Method(_, n) => {
                let _ = write!(out, "<built-in method {}>", n);
            }
            Value::Operator(n) => {
                let _ = write!(out, "<bound method {}>", n);
            }
            Value::Lambda(_) => out.push_str("<function <lambda>>"),
            Value::Opaque(n) => {
                let _ = write!(out, "<{}>", n);
            }
        }
    }

    /// Python `==`.
    pub fn py_eq(&self, other: &Value) -> bool {
        if let (Some(a), Some(b)) = (self.as_number(), other.as_number()) {
            return a.num_eq(&b);
        }
        match (self, other) {
            (Value::None, Value::None) => true,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::List(a), Value::List(b)) | (Value::Tuple(a), Value::Tuple(b)) => {
                a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.py_eq(y))
            }
            (Value::Set(a), Value::Set(b)) => a.len() == b.len() && a.iter().all(|x| b.iter().any(|y| x.py_eq(y))),
            (Value::Dict(a), Value::Dict(b)) => {
                a.len() == b.len() && a.iter().all(|(k, v)| b.iter().any(|(k2, v2)| k.py_eq(k2) && v.py_eq(v2)))
            }
            (Value::Builtin(a), Value::Builtin(b)) => a == b,
            (Value::Math(a), Value::Math(b)) => a == b,
            (Value::MathModule, Value::MathModule) => true,
            (Value::Operator(a), Value::Operator(b)) => a == b,
            (Value::Opaque(a), Value::Opaque(b)) => a == b,
            _ => false,
        }
    }

    /// Python ordering (`<` and friends); `TypeError` for unorderable types.
    pub fn py_cmp(&self, other: &Value) -> PyResult<Ordering> {
        if let (Some(a), Some(b)) = (self.as_number(), other.as_number()) {
            return a.num_cmp(&b).ok_or_else(|| PyErr::new("Unordered", "nan"));
        }
        match (self, other) {
            (Value::Str(a), Value::Str(b)) => Ok(a.chars().cmp(b.chars())),
            (Value::List(a), Value::List(b)) | (Value::Tuple(a), Value::Tuple(b)) => {
                for (x, y) in a.iter().zip(b.iter()) {
                    if !x.py_eq(y) {
                        return x.py_cmp(y);
                    }
                }
                Ok(a.len().cmp(&b.len()))
            }
            _ => Err(PyErr::type_err(format!(
                "'<' not supported between instances of '{}' and '{}'",
                self.type_name(),
                other.type_name()
            ))),
        }
    }

    /// `a < b` with NaN semantics (all comparisons with NaN are false).
    pub fn py_lt(&self, other: &Value) -> PyResult<bool> {
        match self.py_cmp(other) {
            Ok(o) => Ok(o == Ordering::Less),
            Err(e) if e.kind == "Unordered" => Ok(false),
            Err(e) => Err(e),
        }
    }
    pub fn py_le(&self, other: &Value) -> PyResult<bool> {
        match self.py_cmp(other) {
            Ok(o) => Ok(o != Ordering::Greater),
            Err(e) if e.kind == "Unordered" => Ok(false),
            Err(e) => Err(e),
        }
    }
    pub fn py_gt(&self, other: &Value) -> PyResult<bool> {
        other.py_lt(self)
    }
    pub fn py_ge(&self, other: &Value) -> PyResult<bool> {
        other.py_le(self)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Num {
    Int(i64),
    Float(f64),
}

impl Num {
    pub fn to_f64(self) -> f64 {
        match self {
            Num::Int(i) => i as f64,
            Num::Float(f) => f,
        }
    }
    pub fn to_value(self) -> Value {
        match self {
            Num::Int(i) => Value::Int(i),
            Num::Float(f) => Value::Float(f),
        }
    }
    /// Exact comparison of an int and a float, like CPython's `float_richcompare`.
    fn num_cmp(&self, other: &Num) -> Option<Ordering> {
        match (*self, *other) {
            (Num::Int(a), Num::Int(b)) => Some(a.cmp(&b)),
            (Num::Float(a), Num::Float(b)) => a.partial_cmp(&b),
            (Num::Int(a), Num::Float(b)) => cmp_int_float(a, b),
            (Num::Float(a), Num::Int(b)) => cmp_int_float(b, a).map(|o| o.reverse()),
        }
    }
    fn num_eq(&self, other: &Num) -> bool {
        self.num_cmp(other) == Some(Ordering::Equal)
    }
}

fn cmp_int_float(i: i64, f: f64) -> Option<Ordering> {
    if f.is_nan() {
        return None;
    }
    if f.is_infinite() {
        return Some(if f > 0.0 { Ordering::Less } else { Ordering::Greater });
    }
    // Compare exactly: split the float into integral and fractional parts.
    let fi = f.trunc();
    if fi >= 9.3e18 {
        return Some(Ordering::Less);
    }
    if fi <= -9.3e18 {
        return Some(Ordering::Greater);
    }
    let fi_int = fi as i64;
    match i.cmp(&fi_int) {
        Ordering::Equal => {
            let frac = f - fi;
            if frac > 0.0 {
                Some(Ordering::Less)
            } else if frac < 0.0 {
                Some(Ordering::Greater)
            } else {
                Some(Ordering::Equal)
            }
        }
        o => Some(o),
    }
}

/// CPython `repr(float)` (shortest round-tripping digits, 'r' format rules).
pub fn float_repr(f: f64) -> String {
    if f.is_nan() {
        return "nan".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if f == 0.0 {
        return if f.is_sign_negative() { "-0.0".into() } else { "0.0".into() };
    }
    // Shortest digits and exponent from Rust's (shortest, round-trip) LowerExp formatting.
    let s = format!("{:e}", f.abs());
    let (mant, exp) = s.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let ndigits = digits.len() as i32;
    let decpt = exp + 1;
    let mut out = String::new();
    if f < 0.0 {
        out.push('-');
    }
    if decpt <= -4 || decpt > 16 {
        out.push_str(&digits[..1]);
        if ndigits > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        let e = decpt - 1;
        let _ = write!(out, "e{}{:02}", if e < 0 { '-' } else { '+' }, e.abs());
    } else if decpt <= 0 {
        out.push_str("0.");
        for _ in 0..(-decpt) {
            out.push('0');
        }
        out.push_str(&digits);
    } else if decpt >= ndigits {
        out.push_str(&digits);
        for _ in 0..(decpt - ndigits) {
            out.push('0');
        }
        out.push_str(".0");
    } else {
        out.push_str(&digits[..decpt as usize]);
        out.push('.');
        out.push_str(&digits[decpt as usize..]);
    }
    out
}

/// CPython `repr(str)`.
pub fn str_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                let _ = write!(out, "\\x{:02x}", c as u32);
            }
            c if (0x80..0xa0).contains(&(c as u32)) || c == '\u{ad}' => {
                let _ = write!(out, "\\x{:02x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// CPython `float(str)` parsing (accepts surrounding whitespace, underscores between digits,
/// `inf`/`nan`).
pub fn parse_float(s: &str) -> Option<f64> {
    let t = s.trim();
    let lower = t.to_ascii_lowercase();
    let (sign, body) = match lower.strip_prefix('-') {
        Some(b) => (-1.0, b),
        None => (1.0, lower.strip_prefix('+').unwrap_or(&lower)),
    };
    match body {
        "inf" | "infinity" => return Some(sign * f64::INFINITY),
        "nan" => return Some(f64::NAN),
        _ => {}
    }
    if body.is_empty() || body.starts_with('_') || body.ends_with('_') || body.contains("__") {
        return None;
    }
    if !body.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | '+' | '-' | '_')) {
        return None;
    }
    let cleaned: String = t.trim().chars().filter(|c| *c != '_').collect();
    cleaned.parse::<f64>().ok()
}

/// CPython `int(str)` parsing (base 10).
pub fn parse_int(s: &str) -> Option<i64> {
    let t = s.trim();
    if t.is_empty() || t.starts_with('_') || t.ends_with('_') || t.contains("__") {
        return None;
    }
    let cleaned: String = t.chars().filter(|c| *c != '_').collect();
    let digits = cleaned.strip_prefix(['+', '-']).unwrap_or(&cleaned);
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    cleaned.parse::<i64>().ok()
}
