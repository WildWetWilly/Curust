//! The standard library subset: math, time, datetime, json, base64, collections, typing, enum,
//! copy, string, sys, functools, re and numpy.

use crate::ast::Name;
use crate::builtins::{builtin, to_float, to_int};
use crate::interp::{Flow, Interp, R};
use crate::value::*;
use std::cell::RefCell;
use std::rc::Rc;

type Args = Vec<Value>;
type Kw = Vec<(Name, Value)>;

fn kwarg(kw: &Kw, name: &str) -> Option<Value> {
    kw.iter().find(|(k, _)| &**k == name).map(|(_, v)| v.clone())
}

pub fn make_module(name: &str, entries: Vec<(&str, Value)>) -> Value {
    let mut ns = Namespace::default();
    ns.insert("__name__".into(), Value::str(name));
    for (k, v) in entries {
        ns.insert(k.into(), v);
    }
    Value::Module(Rc::new(Module { name: name.to_string(), dict: Rc::new(RefCell::new(ns)) }))
}

/// Built-in modules by name.
pub fn builtin_module(i: &mut Interp, name: &str) -> Option<R<Value>> {
    Some(Ok(match name {
        "math" => math_module(),
        "time" => time_module(),
        "_curust_time" => time_module(),
        "json" => json_module(i),
        "base64" => make_module("base64", vec![("b64encode", builtin("b64encode", b64encode)), ("standard_b64encode", builtin("standard_b64encode", b64encode)), ("b64decode", builtin("b64decode", b64decode)), ("standard_b64decode", builtin("standard_b64decode", b64decode)), ("urlsafe_b64encode", builtin("urlsafe_b64encode", b64encode_url)), ("urlsafe_b64decode", builtin("urlsafe_b64decode", b64decode))]),
        "enum" => {
            let auto = builtin("auto", |_i, _a, _kw| Ok(Value::Native(Rc::new(EnumAuto))));
            let unique = builtin("unique", |_i, a, _kw| Ok(a.first().cloned().unwrap_or(Value::None)));
            let m = make_module("enum", vec![("Enum", Value::Class(i.types.enum_.clone())), ("auto", auto), ("unique", unique)]);
            // IntEnum / StrEnum are mix-ins of Enum.
            let src = "class IntEnum(int, Enum):\n    pass\nclass StrEnum(str, Enum):\n    pass\nFlag = Enum\nIntFlag = IntEnum\n";
            if let Err(e) = run_into(i, &m, src) {
                return Some(Err(e));
            }
            m
        }
        "typing" => {
            let mut entries: Vec<(&str, Value)> = vec![("TYPE_CHECKING", Value::Bool(false))];
            let t = &i.types;
            let aliases: &[(&'static str, Option<&Rc<Class>>)] = &[
                ("Any", None),
                ("List", Some(&t.list)),
                ("Dict", Some(&t.dict)),
                ("Tuple", Some(&t.tuple)),
                ("Set", Some(&t.set)),
                ("FrozenSet", None),
                ("Optional", None),
                ("Union", None),
                ("Callable", None),
                ("Iterable", None),
                ("Iterator", None),
                ("Sequence", None),
                ("Mapping", None),
                ("MutableMapping", None),
                ("Type", None),
                ("Generator", None),
                ("NamedTuple", None),
                ("Literal", None),
                ("ClassVar", None),
                ("Final", None),
                ("TypeVar", None),
                ("Generic", None),
                ("Protocol", None),
                ("cast", None),
                ("overload", None),
                ("no_type_check", None),
            ];
            for (n, c) in aliases {
                let v = match *n {
                    "cast" => builtin("cast", |_i, a, _kw| Ok(a.get(1).cloned().unwrap_or(Value::None))),
                    "overload" | "no_type_check" => builtin("overload", |_i, a, _kw| Ok(a.first().cloned().unwrap_or(Value::None))),
                    "TypeVar" => builtin("TypeVar", |_i, a, _kw| Ok(Value::Native(Rc::new(TypeAlias { name: format!("{}", a.first().map(|v| if let Value::Str(s) = v { s.to_string() } else { String::new() }).unwrap_or_default()), origin: None })))),
                    _ => Value::Native(Rc::new(TypeAlias { name: n.to_string(), origin: c.map(|c| Value::Class(c.clone())) })),
                };
                entries.push((n, v));
            }
            make_module("typing", entries)
        }
        "collections" => return Some(py_module(i, "collections", COLLECTIONS_SRC)),
        "datetime" => return Some(py_module(i, "datetime", DATETIME_SRC)),
        "string" => return Some(py_module(i, "string", STRING_SRC)),
        "functools" => return Some(py_module(i, "functools", FUNCTOOLS_SRC)),
        "copy" => make_module("copy", vec![("copy", builtin("copy", copy_copy)), ("deepcopy", builtin("deepcopy", copy_deepcopy))]),
        "sys" => {
            let vi = Value::tuple(vec![Value::Int(3), Value::Int(12), Value::Int(2), Value::str("final"), Value::Int(0)]);
            make_module(
                "sys",
                vec![
                    ("version", Value::str("3.12.2 (Curust embedded interpreter)")),
                    ("version_info", vi),
                    ("platform", Value::str(if cfg!(windows) { "win32" } else if cfg!(target_os = "macos") { "darwin" } else { "linux" })),
                    ("maxsize", Value::Int(i64::MAX)),
                    ("byteorder", Value::str("little")),
                    ("argv", Value::list(Vec::new())),
                    ("path", Value::list(Vec::new())),
                    ("modules", Value::dict(Dict::default())),
                    ("float_info", make_module("float_info", vec![("epsilon", Value::Float(f64::EPSILON)), ("max", Value::Float(f64::MAX)), ("min", Value::Float(f64::MIN_POSITIVE))])),
                ],
            )
        }
        "re" => {
            let m = crate::re::module();
            let err = crate::re::error_class(i);
            if let Value::Module(md) = &m {
                md.dict.borrow_mut().insert("error".into(), err.clone());
                md.dict.borrow_mut().insert("PatternError".into(), err);
            }
            m
        }
        "numpy" => crate::numpy::module(),
        _ => return None,
    }))
}

fn run_into(i: &mut Interp, m: &Value, src: &str) -> R<()> {
    let Value::Module(md) = m else { return Ok(()) };
    let stmts = crate::parser::parse_module(src).map_err(|e| i.exc("SyntaxError", e))?;
    let mut frame = crate::interp::Frame {
        locals: md.dict.clone(),
        globals: md.dict.clone(),
        closure: Vec::new(),
        declared_global: Vec::new(),
        declared_nonlocal: Vec::new(),
        is_module: true,
        is_class: false,
        method_class: None,
        first_arg: None,
    };
    i.exec_block(&stmts, &mut frame)
}

fn py_module(i: &mut Interp, name: &str, src: &str) -> R<Value> {
    Ok(Value::Module(i.run_module(name, src)?))
}

// ---------------------------------------------------------------------- enum

pub struct EnumAuto;
impl NativeObject for EnumAuto {
    fn type_name(&self) -> &str {
        "auto"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Turn the class attributes of an Enum subclass into members.
pub fn finish_enum(i: &mut Interp, class: &Rc<Class>, order: &[Name]) -> R<()> {
    // The `name` / `value` properties live on the Enum base class.
    if !i.types.enum_.dict.borrow().contains_key("value") {
        let getter = |field: &'static str| -> Value {
            let f: BuiltinFn = if field == "_value_" {
                |_i, a, _kw| Ok(match a.first() {
                    Some(Value::Instance(m)) => m.dict.borrow().get("_value_").cloned().unwrap_or(Value::None),
                    _ => Value::None,
                })
            } else {
                |_i, a, _kw| Ok(match a.first() {
                    Some(Value::Instance(m)) => m.dict.borrow().get("_name_").cloned().unwrap_or(Value::None),
                    _ => Value::None,
                })
            };
            Value::Property(Rc::new((builtin("getter", f), Value::None)))
        };
        let mut d = i.types.enum_.dict.borrow_mut();
        d.insert("value".into(), getter("_value_"));
        d.insert("name".into(), getter("_name_"));
    }
    let mut members: Vec<Value> = Vec::new();
    let mut last_int = 0i64;
    // The mix-in type (str, int, tuple, …) of the members' base values.
    let mixin = class.mro.borrow().iter().find(|c| c.builtin != "object" && c.builtin != "Enum" && c.bases.len() == 1 && Rc::ptr_eq(&c.bases[0], &i.types.object)).cloned();
    for name in order {
        if name.starts_with('_') {
            continue;
        }
        let v = match class.dict.borrow().get(name) {
            Some(v) => v.clone(),
            None => continue,
        };
        if matches!(v, Value::Func(_) | Value::StaticMethod(_) | Value::ClassMethod(_) | Value::Property(_) | Value::Builtin(_) | Value::Class(_)) {
            continue;
        }
        let value = match &v {
            Value::Native(n) if n.as_any().is::<EnumAuto>() => Value::Int(last_int + 1),
            _ => v.clone(),
        };
        if let Value::Int(n) = value {
            last_int = n;
        }
        // Aliases: a repeated value names the first member.
        let mut alias = None;
        for m in &members {
            if let Value::Instance(mi) = m {
                let mv = mi.dict.borrow().get("_value_").cloned().unwrap_or(Value::None);
                if i.equals(&mv, &value)? {
                    alias = Some(m.clone());
                    break;
                }
            }
        }
        if let Some(a) = alias {
            class.dict.borrow_mut().insert(name.clone(), a);
            continue;
        }
        let base = match &mixin {
            Some(mc) => {
                let args = match (&value, mc.builtin) {
                    (Value::Tuple(t), b) if b != "tuple" => (**t).clone(),
                    _ => vec![value.clone()],
                };
                match crate::builtins::construct_builtin(i, mc, &args, &Vec::new()) {
                    Some(r) => r?,
                    None => Value::None,
                }
            }
            None => Value::None,
        };
        let inst = Rc::new(Instance { class: class.clone(), dict: RefCell::new(Namespace::default()), base: RefCell::new(base) });
        inst.dict.borrow_mut().insert("_name_".into(), Value::Str(name.clone()));
        inst.dict.borrow_mut().insert("_value_".into(), value.clone());
        let member = Value::Instance(inst);
        if let Some(init) = class.lookup("__init__") {
            if !matches!(init, Value::Builtin(_)) {
                let mut a = vec![member.clone()];
                match &value {
                    Value::Tuple(t) => a.extend(t.iter().cloned()),
                    other => a.push(other.clone()),
                }
                i.call(&init, a, Vec::new())?;
            }
        }
        class.dict.borrow_mut().insert(name.clone(), member.clone());
        members.push(member);
    }
    *class.members.borrow_mut() = members;
    Ok(())
}

/// `typing` aliases (`List`, `Optional[int]`, …): subscriptable, and `isinstance` checks the origin.
pub struct TypeAlias {
    pub name: String,
    pub origin: Option<Value>,
}
impl TypeAlias {
    pub fn matches(&self, i: &mut Interp, v: &Value) -> bool {
        match &self.origin {
            Some(o) => crate::builtins::isinstance(i, v, o).unwrap_or(false),
            None => self.name == "Any",
        }
    }
}
impl NativeObject for TypeAlias {
    fn type_name(&self) -> &str {
        "typing._GenericAlias"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn repr(&self) -> String {
        format!("typing.{}", self.name)
    }
    fn get_item(&self, _i: &mut Interp, _k: &Value) -> Option<R<Value>> {
        Some(Ok(Value::Native(Rc::new(TypeAlias { name: self.name.clone(), origin: self.origin.clone() }))))
    }
    fn call(&self, i: &mut Interp, args: Vec<Value>, kw: Vec<(Name, Value)>) -> Option<R<Value>> {
        match &self.origin {
            Some(o) => Some(i.call(o, args, kw)),
            None => Some(i.type_error(format!("Cannot instantiate typing.{}", self.name))),
        }
    }
}

// ---------------------------------------------------------------------- math

fn f1(i: &mut Interp, a: &Args) -> R<f64> {
    to_float(i, a.first().unwrap_or(&Value::None))
}

fn float_to_int(i: &mut Interp, x: f64) -> R<Value> {
    if x.is_nan() {
        return i.value_error("cannot convert float NaN to integer");
    }
    if x.is_infinite() {
        return Err(i.exc("OverflowError", "cannot convert float infinity to integer"));
    }
    if x.abs() < 9.2e18 {
        Ok(Value::Int(x as i64))
    } else {
        Ok(Value::Float(x))
    }
}

fn math_module() -> Value {
    macro_rules! unary {
        ($name:expr, $f:expr) => {
            ($name, builtin($name, |i, a, _kw| {
                let x = f1(i, &a)?;
                let f: fn(f64) -> f64 = $f;
                let r = f(x);
                if r.is_nan() && !x.is_nan() {
                    return i.value_error("math domain error");
                }
                Ok(Value::Float(r))
            }))
        };
    }
    make_module(
        "math",
        vec![
            ("pi", Value::Float(std::f64::consts::PI)),
            ("e", Value::Float(std::f64::consts::E)),
            ("tau", Value::Float(std::f64::consts::TAU)),
            ("inf", Value::Float(f64::INFINITY)),
            ("nan", Value::Float(f64::NAN)),
            unary!("sqrt", |x| x.sqrt()),
            unary!("sin", |x| x.sin()),
            unary!("cos", |x| x.cos()),
            unary!("tan", |x| x.tan()),
            unary!("asin", |x| x.asin()),
            unary!("acos", |x| x.acos()),
            unary!("atan", |x| x.atan()),
            unary!("sinh", |x| x.sinh()),
            unary!("cosh", |x| x.cosh()),
            unary!("tanh", |x| x.tanh()),
            unary!("exp", |x| x.exp()),
            unary!("log10", |x| x.log10()),
            unary!("log2", |x| x.log2()),
            unary!("log1p", |x| x.ln_1p()),
            unary!("expm1", |x| x.exp_m1()),
            unary!("fabs", |x| x.abs()),
            unary!("degrees", |x| x.to_degrees()),
            unary!("radians", |x| x.to_radians()),
            ("floor", builtin("floor", |i, a, _kw| match a.first().map(|v| v.unwrap_base()) {
                Some(Value::Int(n)) => Ok(Value::Int(n)),
                Some(Value::Bool(b)) => Ok(Value::Int(b as i64)),
                _ => {
                    let x = f1(i, &a)?;
                    float_to_int(i, x.floor())
                }
            })),
            ("ceil", builtin("ceil", |i, a, _kw| match a.first().map(|v| v.unwrap_base()) {
                Some(Value::Int(n)) => Ok(Value::Int(n)),
                _ => {
                    let x = f1(i, &a)?;
                    float_to_int(i, x.ceil())
                }
            })),
            ("trunc", builtin("trunc", |i, a, _kw| match a.first().map(|v| v.unwrap_base()) {
                Some(Value::Int(n)) => Ok(Value::Int(n)),
                _ => {
                    let x = f1(i, &a)?;
                    float_to_int(i, x.trunc())
                }
            })),
            ("log", builtin("log", |i, a, _kw| {
                let x = f1(i, &a)?;
                if x <= 0.0 {
                    return i.value_error("math domain error");
                }
                match a.get(1) {
                    Some(b) => {
                        let b = to_float(i, b)?;
                        Ok(Value::Float(x.ln() / b.ln()))
                    }
                    None => Ok(Value::Float(x.ln())),
                }
            })),
            ("pow", builtin("pow", |i, a, _kw| {
                let x = f1(i, &a)?;
                let y = to_float(i, a.get(1).unwrap_or(&Value::None))?;
                Ok(Value::Float(x.powf(y)))
            })),
            ("atan2", builtin("atan2", |i, a, _kw| {
                let y = f1(i, &a)?;
                let x = to_float(i, a.get(1).unwrap_or(&Value::None))?;
                Ok(Value::Float(y.atan2(x)))
            })),
            ("hypot", builtin("hypot", |i, a, _kw| {
                let mut acc = 0.0f64;
                let vals: Vec<f64> = a.iter().map(|v| to_float(i, v)).collect::<R<_>>()?;
                if vals.len() == 2 {
                    return Ok(Value::Float(vals[0].hypot(vals[1])));
                }
                for v in vals {
                    acc += v * v;
                }
                Ok(Value::Float(acc.sqrt()))
            })),
            ("copysign", builtin("copysign", |i, a, _kw| {
                let x = f1(i, &a)?;
                let y = to_float(i, a.get(1).unwrap_or(&Value::None))?;
                Ok(Value::Float(x.copysign(y)))
            })),
            ("fmod", builtin("fmod", |i, a, _kw| {
                let x = f1(i, &a)?;
                let y = to_float(i, a.get(1).unwrap_or(&Value::None))?;
                if y == 0.0 {
                    return i.value_error("math domain error");
                }
                Ok(Value::Float(x % y))
            })),
            ("modf", builtin("modf", |i, a, _kw| {
                let x = f1(i, &a)?;
                Ok(Value::tuple(vec![Value::Float(x.fract()), Value::Float(x.trunc())]))
            })),
            ("isnan", builtin("isnan", |i, a, _kw| Ok(Value::Bool(f1(i, &a)?.is_nan())))),
            ("isinf", builtin("isinf", |i, a, _kw| Ok(Value::Bool(f1(i, &a)?.is_infinite())))),
            ("isfinite", builtin("isfinite", |i, a, _kw| Ok(Value::Bool(f1(i, &a)?.is_finite())))),
            ("isclose", builtin("isclose", |i, a, kw| {
                let x = f1(i, &a)?;
                let y = to_float(i, a.get(1).unwrap_or(&Value::None))?;
                let rel = match kwarg(&kw, "rel_tol") {
                    Some(v) => to_float(i, &v)?,
                    None => 1e-9,
                };
                let abs = match kwarg(&kw, "abs_tol") {
                    Some(v) => to_float(i, &v)?,
                    None => 0.0,
                };
                if x == y {
                    return Ok(Value::Bool(true));
                }
                if x.is_infinite() || y.is_infinite() {
                    return Ok(Value::Bool(false));
                }
                let d = (y - x).abs();
                Ok(Value::Bool(d <= (rel * y).abs() || d <= (rel * x).abs() || d <= abs))
            })),
            ("gcd", builtin("gcd", |i, a, _kw| {
                let mut g = 0i64;
                for v in &a {
                    let mut b = to_int(i, v)?.abs();
                    let mut x = g;
                    while b != 0 {
                        let t = x % b;
                        x = b;
                        b = t;
                    }
                    g = x;
                }
                Ok(Value::Int(g))
            })),
            ("factorial", builtin("factorial", |i, a, _kw| {
                let n = to_int(i, a.first().unwrap_or(&Value::None))?;
                if n < 0 {
                    return i.value_error("factorial() not defined for negative values");
                }
                let mut r: i64 = 1;
                for k in 2..=n {
                    r = match r.checked_mul(k) {
                        Some(v) => v,
                        None => return Err(i.exc("OverflowError", "factorial result too large")),
                    };
                }
                Ok(Value::Int(r))
            })),
            ("fsum", builtin("fsum", |i, a, _kw| {
                // Shewchuk's exact summation (as CPython).
                let items = i.iterate(a.first().unwrap_or(&Value::None))?;
                let mut partials: Vec<f64> = Vec::new();
                for v in items {
                    let mut x = to_float(i, &v)?;
                    let mut k = 0;
                    for j in 0..partials.len() {
                        let mut y = partials[j];
                        if x.abs() < y.abs() {
                            std::mem::swap(&mut x, &mut y);
                        }
                        let hi = x + y;
                        let lo = y - (hi - x);
                        if lo != 0.0 {
                            partials[k] = lo;
                            k += 1;
                        }
                        x = hi;
                    }
                    partials.truncate(k);
                    partials.push(x);
                }
                let mut hi = 0.0;
                if let Some(mut n) = partials.len().checked_sub(0) {
                    if n > 0 {
                        n -= 1;
                        hi = partials[n];
                        let mut lo = 0.0;
                        while n > 0 {
                            let x = hi;
                            n -= 1;
                            let y = partials[n];
                            hi = x + y;
                            let yr = hi - x;
                            lo = y - yr;
                            if lo != 0.0 {
                                break;
                            }
                        }
                        if n > 0 && ((lo < 0.0 && partials[n - 1] < 0.0) || (lo > 0.0 && partials[n - 1] > 0.0)) {
                            let y = lo * 2.0;
                            let x = hi + y;
                            let yr = x - hi;
                            if y == yr {
                                hi = x;
                            }
                        }
                    }
                }
                Ok(Value::Float(hi))
            })),
            ("prod", builtin("prod", |i, a, _kw| {
                let mut acc = Value::Int(1);
                for v in i.iterate(a.first().unwrap_or(&Value::None))? {
                    acc = i.binop(crate::ast::BinOp::Mul, &acc, &v)?;
                }
                Ok(acc)
            })),
            ("dist", builtin("dist", |i, a, _kw| {
                let p = i.iterate(a.first().unwrap_or(&Value::None))?;
                let q = i.iterate(a.get(1).unwrap_or(&Value::None))?;
                let mut acc = 0.0;
                for (x, y) in p.iter().zip(q.iter()) {
                    let d = to_float(i, x)? - to_float(i, y)?;
                    acc += d * d;
                }
                Ok(Value::Float(acc.sqrt()))
            })),
            ("ldexp", builtin("ldexp", |i, a, _kw| {
                let x = f1(i, &a)?;
                let e = to_int(i, a.get(1).unwrap_or(&Value::None))?;
                Ok(Value::Float(x * 2f64.powi(e as i32)))
            })),
            ("frexp", builtin("frexp", |i, a, _kw| {
                let x = f1(i, &a)?;
                if x == 0.0 || !x.is_finite() {
                    return Ok(Value::tuple(vec![Value::Float(x), Value::Int(0)]));
                }
                let mut e = x.abs().log2().floor() as i64 + 1;
                let mut m = x / 2f64.powi(e as i32);
                if m.abs() >= 1.0 {
                    m /= 2.0;
                    e += 1;
                } else if m.abs() < 0.5 {
                    m *= 2.0;
                    e -= 1;
                }
                Ok(Value::tuple(vec![Value::Float(m), Value::Int(e)]))
            })),
        ],
    )
}

// ---------------------------------------------------------------------- time

/// Broken-down time (`struct tm`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Tm {
    pub year: i64,
    /// 1..=12
    pub mon: i64,
    pub mday: i64,
    pub hour: i64,
    pub min: i64,
    pub sec: i64,
    /// Monday = 0 (Python's `tm_wday`).
    pub wday: i64,
    /// 1..=366
    pub yday: i64,
    pub isdst: i64,
    pub gmtoff: i64,
}

pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn tm_from_utc_seconds(t: i64, offset: i64, isdst: i64) -> Tm {
    let local = t + offset;
    let days = local.div_euclid(86400);
    let secs = local.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    let yday = days - days_from_civil(y, 1, 1) + 1;
    // 1970-01-01 was a Thursday (Monday = 0 → 3).
    let wday = (days + 3).rem_euclid(7);
    Tm { year: y, mon: m, mday: d, hour: secs / 3600, min: secs % 3600 / 60, sec: secs % 60, wday, yday, isdst, gmtoff: offset }
}

#[cfg(unix)]
fn local_offset(t: i64) -> (i64, i64) {
    #[repr(C)]
    struct CTm {
        tm_sec: i32,
        tm_min: i32,
        tm_hour: i32,
        tm_mday: i32,
        tm_mon: i32,
        tm_year: i32,
        tm_wday: i32,
        tm_yday: i32,
        tm_isdst: i32,
        tm_gmtoff: std::os::raw::c_long,
        tm_zone: *const std::os::raw::c_char,
    }
    extern "C" {
        fn localtime_r(t: *const i64, out: *mut CTm) -> *mut CTm;
        fn tzset();
    }
    let mut out: CTm = unsafe { std::mem::zeroed() };
    unsafe {
        tzset();
        if localtime_r(&t, &mut out).is_null() {
            return (0, 0);
        }
    }
    (out.tm_gmtoff as i64, out.tm_isdst as i64)
}

#[cfg(windows)]
fn local_offset(t: i64) -> (i64, i64) {
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct SystemTime {
        year: u16,
        month: u16,
        dow: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        ms: u16,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn SystemTimeToTzSpecificLocalTime(tz: *const u8, utc: *const SystemTime, local: *mut SystemTime) -> i32;
    }
    let tm = tm_from_utc_seconds(t, 0, 0);
    let utc = SystemTime { year: tm.year as u16, month: tm.mon as u16, dow: 0, day: tm.mday as u16, hour: tm.hour as u16, minute: tm.min as u16, second: tm.sec as u16, ms: 0 };
    let mut local = SystemTime::default();
    if unsafe { SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) } == 0 {
        return (0, 0);
    }
    let ld = days_from_civil(local.year as i64, local.month as i64, local.day as i64) * 86400 + local.hour as i64 * 3600 + local.minute as i64 * 60 + local.second as i64;
    (ld - t, 0)
}

#[cfg(not(any(unix, windows)))]
fn local_offset(_t: i64) -> (i64, i64) {
    (0, 0)
}

pub fn now_seconds() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

pub fn localtime(t: f64) -> Tm {
    let s = t.floor() as i64;
    let (off, dst) = local_offset(s);
    tm_from_utc_seconds(s, off, dst)
}

const DAY_ABBR: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const DAY_NAME: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
const MON_ABBR: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const MON_NAME: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

/// C-locale `strftime`; `micro` fills `%f` (datetime only).
pub fn strftime(fmt: &str, t: &Tm, micro: i64) -> String {
    let mut out = String::new();
    let mut cs = fmt.chars().peekable();
    while let Some(c) = cs.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let Some(d) = cs.next() else {
            out.push('%');
            break;
        };
        let wd = t.wday.rem_euclid(7) as usize;
        let sun0 = (t.wday + 1) % 7; // Sunday = 0
        match d {
            'a' => out.push_str(DAY_ABBR[wd]),
            'A' => out.push_str(DAY_NAME[wd]),
            'b' | 'h' => out.push_str(MON_ABBR[(t.mon - 1).clamp(0, 11) as usize]),
            'B' => out.push_str(MON_NAME[(t.mon - 1).clamp(0, 11) as usize]),
            'c' => out.push_str(&format!("{} {} {:>2} {:02}:{:02}:{:02} {}", DAY_ABBR[wd], MON_ABBR[(t.mon - 1).clamp(0, 11) as usize], t.mday, t.hour, t.min, t.sec, t.year)),
            'd' => out.push_str(&format!("{:02}", t.mday)),
            'e' => out.push_str(&format!("{:>2}", t.mday)),
            'f' => out.push_str(&format!("{:06}", micro)),
            'H' => out.push_str(&format!("{:02}", t.hour)),
            'I' => out.push_str(&format!("{:02}", if t.hour % 12 == 0 { 12 } else { t.hour % 12 })),
            'j' => out.push_str(&format!("{:03}", t.yday)),
            'm' => out.push_str(&format!("{:02}", t.mon)),
            'M' => out.push_str(&format!("{:02}", t.min)),
            'p' => out.push_str(if t.hour < 12 { "AM" } else { "PM" }),
            'S' => out.push_str(&format!("{:02}", t.sec)),
            'w' => out.push_str(&sun0.to_string()),
            'u' => out.push_str(&(wd + 1).to_string()),
            'U' => out.push_str(&format!("{:02}", (t.yday - 1 + 7 - sun0) / 7)),
            'W' => out.push_str(&format!("{:02}", (t.yday - 1 + 7 - wd as i64) / 7)),
            'x' => out.push_str(&format!("{:02}/{:02}/{:02}", t.mon, t.mday, t.year.rem_euclid(100))),
            'X' | 'T' => out.push_str(&format!("{:02}:{:02}:{:02}", t.hour, t.min, t.sec)),
            'D' => out.push_str(&format!("{:02}/{:02}/{:02}", t.mon, t.mday, t.year.rem_euclid(100))),
            'F' => out.push_str(&format!("{}-{:02}-{:02}", t.year, t.mon, t.mday)),
            'R' => out.push_str(&format!("{:02}:{:02}", t.hour, t.min)),
            'y' => out.push_str(&format!("{:02}", t.year.rem_euclid(100))),
            'Y' => out.push_str(&t.year.to_string()),
            'C' => out.push_str(&format!("{:02}", t.year / 100)),
            'z' => {
                let o = t.gmtoff;
                out.push_str(&format!("{}{:02}{:02}", if o < 0 { '-' } else { '+' }, o.abs() / 3600, o.abs() % 3600 / 60));
            }
            'Z' => out.push_str(if t.gmtoff == 0 { "UTC" } else { "" }),
            'n' => out.push('\n'),
            't' => out.push('\t'),
            '%' => out.push('%'),
            other => {
                out.push('%');
                out.push(other);
            }
        }
    }
    out
}

pub fn tm_to_value(t: &Tm) -> Value {
    Value::Native(Rc::new(StructTime(*t)))
}

pub struct StructTime(pub Tm);
impl StructTime {
    fn fields(&self) -> Vec<Value> {
        let t = &self.0;
        vec![t.year, t.mon, t.mday, t.hour, t.min, t.sec, t.wday, t.yday, t.isdst].into_iter().map(Value::Int).collect()
    }
}
impl NativeObject for StructTime {
    fn type_name(&self) -> &str {
        "time.struct_time"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn repr(&self) -> String {
        let t = &self.0;
        format!("time.struct_time(tm_year={}, tm_mon={}, tm_mday={}, tm_hour={}, tm_min={}, tm_sec={}, tm_wday={}, tm_yday={}, tm_isdst={})", t.year, t.mon, t.mday, t.hour, t.min, t.sec, t.wday, t.yday, t.isdst)
    }
    fn get_attr(&self, _i: &mut Interp, _this: &Value, name: &str) -> Option<R<Value>> {
        let t = &self.0;
        let v = match name {
            "tm_year" => t.year,
            "tm_mon" => t.mon,
            "tm_mday" => t.mday,
            "tm_hour" => t.hour,
            "tm_min" => t.min,
            "tm_sec" => t.sec,
            "tm_wday" => t.wday,
            "tm_yday" => t.yday,
            "tm_isdst" => t.isdst,
            "tm_gmtoff" => t.gmtoff,
            _ => return None,
        };
        Some(Ok(Value::Int(v)))
    }
    fn len(&self) -> Option<usize> {
        Some(9)
    }
    fn get_item(&self, i: &mut Interp, k: &Value) -> Option<R<Value>> {
        let t = Value::tuple(self.fields());
        Some(i.get_item(&t, k))
    }
    fn iter(&self, _i: &mut Interp) -> Option<R<Vec<Value>>> {
        Some(Ok(self.fields()))
    }
}

/// A `Tm` from a struct_time or a 9-tuple.
fn tm_arg(i: &mut Interp, v: &Value) -> R<Tm> {
    if let Value::Native(n) = v {
        if let Some(st) = n.as_any().downcast_ref::<StructTime>() {
            return Ok(st.0);
        }
    }
    let items = i.iterate(v)?;
    if items.len() < 9 {
        return i.type_error("time tuple must have 9 elements");
    }
    let n: Vec<i64> = items.iter().take(9).map(|x| to_int(i, x)).collect::<R<_>>()?;
    Ok(Tm { year: n[0], mon: n[1], mday: n[2], hour: n[3], min: n[4], sec: n[5], wday: n[6], yday: n[7], isdst: n[8], gmtoff: 0 })
}

fn time_module() -> Value {
    make_module(
        "time",
        vec![
            ("time", builtin("time", |_i, _a, _kw| Ok(Value::Float(now_seconds())))),
            ("time_ns", builtin("time_ns", |_i, _a, _kw| Ok(Value::Int((now_seconds() * 1e9) as i64)))),
            ("monotonic", builtin("monotonic", |_i, _a, _kw| Ok(Value::Float(monotonic())))),
            ("perf_counter", builtin("perf_counter", |_i, _a, _kw| Ok(Value::Float(monotonic())))),
            ("process_time", builtin("process_time", |_i, _a, _kw| Ok(Value::Float(monotonic())))),
            ("sleep", builtin("sleep", |i, a, _kw| {
                let s = to_float(i, a.first().unwrap_or(&Value::Int(0)))?;
                std::thread::sleep(std::time::Duration::from_secs_f64(s.clamp(0.0, 5.0)));
                Ok(Value::None)
            })),
            ("localtime", builtin("localtime", |i, a, _kw| {
                let t = match a.first() {
                    None | Some(Value::None) => now_seconds(),
                    Some(v) => to_float(i, v)?,
                };
                Ok(tm_to_value(&localtime(t)))
            })),
            ("gmtime", builtin("gmtime", |i, a, _kw| {
                let t = match a.first() {
                    None | Some(Value::None) => now_seconds(),
                    Some(v) => to_float(i, v)?,
                };
                Ok(tm_to_value(&tm_from_utc_seconds(t.floor() as i64, 0, 0)))
            })),
            ("mktime", builtin("mktime", |i, a, _kw| {
                let t = tm_arg(i, a.first().unwrap_or(&Value::None))?;
                let local = days_from_civil(t.year, t.mon, t.mday) * 86400 + t.hour * 3600 + t.min * 60 + t.sec;
                let (off, _) = local_offset(local);
                let (off2, _) = local_offset(local - off);
                Ok(Value::Float((local - off2) as f64))
            })),
            ("strftime", builtin("strftime", |i, a, _kw| {
                let fmt = i.to_str(a.first().unwrap_or(&Value::None))?;
                let t = match a.get(1) {
                    Some(v) => tm_arg(i, v)?,
                    None => localtime(now_seconds()),
                };
                Ok(Value::Str(strftime(&fmt, &t, 0).into()))
            })),
            ("asctime", builtin("asctime", |i, a, _kw| {
                let t = match a.first() {
                    Some(v) => tm_arg(i, v)?,
                    None => localtime(now_seconds()),
                };
                Ok(Value::Str(strftime("%c", &t, 0).into()))
            })),
            ("ctime", builtin("ctime", |i, a, _kw| {
                let s = match a.first() {
                    None | Some(Value::None) => now_seconds(),
                    Some(v) => to_float(i, v)?,
                };
                Ok(Value::Str(strftime("%c", &localtime(s), 0).into()))
            })),
            // Helpers for the datetime module: (y, m, d, H, M, S, us, wday, yday, isdst, gmtoff).
            ("_local_parts", builtin("_local_parts", |i, a, _kw| {
                let t = match a.first() {
                    None | Some(Value::None) => now_seconds(),
                    Some(v) => to_float(i, v)?,
                };
                let tm = localtime(t);
                let us = ((t - t.floor()) * 1e6).round() as i64;
                Ok(Value::tuple(vec![tm.year, tm.mon, tm.mday, tm.hour, tm.min, tm.sec, us.min(999_999), tm.wday, tm.yday, tm.isdst, tm.gmtoff].into_iter().map(Value::Int).collect()))
            })),
            ("_strftime_parts", builtin("_strftime_parts", |i, a, _kw| {
                // (fmt, y, m, d, H, M, S, us)
                let fmt = i.to_str(a.first().unwrap_or(&Value::None))?;
                let n: Vec<i64> = a[1..].iter().map(|v| to_int(i, v)).collect::<R<_>>()?;
                let days = days_from_civil(n[0], n[1], n[2]);
                let mut tm = tm_from_utc_seconds(days * 86400 + n[3] * 3600 + n[4] * 60 + n[5], 0, -1);
                tm.gmtoff = 0;
                Ok(Value::Str(strftime(&fmt, &tm, n.get(6).copied().unwrap_or(0)).into()))
            })),
            ("_days_from_civil", builtin("_days_from_civil", |i, a, _kw| {
                let n: Vec<i64> = a.iter().map(|v| to_int(i, v)).collect::<R<_>>()?;
                Ok(Value::Int(days_from_civil(n[0], n[1], n[2])))
            })),
            ("_civil_from_days", builtin("_civil_from_days", |i, a, _kw| {
                let d = to_int(i, a.first().unwrap_or(&Value::Int(0)))?;
                let (y, m, dd) = civil_from_days(d);
                Ok(Value::tuple(vec![Value::Int(y), Value::Int(m), Value::Int(dd)]))
            })),
            ("timezone", Value::Int(-local_offset(now_seconds() as i64).0)),
        ],
    )
}

fn monotonic() -> f64 {
    thread_local! {
        static START: std::time::Instant = std::time::Instant::now();
    }
    START.with(|s| s.elapsed().as_secs_f64())
}

// ---------------------------------------------------------------------- json

fn json_module(i: &mut Interp) -> Value {
    let err = new_class("JSONDecodeError", vec![i.exc_class("ValueError")], "Exception");
    i.types.exceptions.insert("JSONDecodeError", err.clone());
    make_module(
        "json",
        vec![
            ("loads", builtin("loads", json_loads)),
            ("dumps", builtin("dumps", json_dumps)),
            ("JSONDecodeError", Value::Class(err)),
        ],
    )
}

struct JsonParser<'a> {
    c: &'a [char],
    k: usize,
}

fn json_err(i: &Interp, msg: &str, p: &JsonParser) -> Flow {
    let line = p.c[..p.k.min(p.c.len())].iter().filter(|&&c| c == '\n').count() + 1;
    let col = p.k - p.c[..p.k.min(p.c.len())].iter().rposition(|&c| c == '\n').map(|x| x + 1).unwrap_or(0) + 1;
    i.exc("JSONDecodeError", format!("{}: line {} column {} (char {})", msg, line, col, p.k))
}

impl<'a> JsonParser<'a> {
    fn ws(&mut self) {
        while self.k < self.c.len() && matches!(self.c[self.k], ' ' | '\t' | '\n' | '\r') {
            self.k += 1;
        }
    }
    fn value(&mut self, i: &mut Interp, hook: &Option<Value>, ohook: &Option<Value>) -> R<Value> {
        self.ws();
        if self.k >= self.c.len() {
            return Err(json_err(i, "Expecting value", self));
        }
        let rest = |p: &Self, s: &str| -> bool { p.c[p.k..].iter().take(s.chars().count()).copied().eq(s.chars()) };
        match self.c[self.k] {
            '{' => {
                self.k += 1;
                let mut pairs: Vec<(Value, Value)> = Vec::new();
                self.ws();
                if self.k < self.c.len() && self.c[self.k] == '}' {
                    self.k += 1;
                } else {
                    loop {
                        self.ws();
                        if self.k >= self.c.len() || self.c[self.k] != '"' {
                            return Err(json_err(i, "Expecting property name enclosed in double quotes", self));
                        }
                        let key = self.string(i)?;
                        self.ws();
                        if self.k >= self.c.len() || self.c[self.k] != ':' {
                            return Err(json_err(i, "Expecting ':' delimiter", self));
                        }
                        self.k += 1;
                        let v = self.value(i, hook, ohook)?;
                        pairs.push((Value::Str(key.into()), v));
                        self.ws();
                        if self.k < self.c.len() && self.c[self.k] == ',' {
                            self.k += 1;
                            continue;
                        }
                        if self.k < self.c.len() && self.c[self.k] == '}' {
                            self.k += 1;
                            break;
                        }
                        return Err(json_err(i, "Expecting ',' delimiter", self));
                    }
                }
                if let Some(h) = hook {
                    let lst = Value::list(pairs.into_iter().map(|(k, v)| Value::tuple(vec![k, v])).collect());
                    return i.call(h, vec![lst], Vec::new());
                }
                let mut d = Dict::default();
                for (k, v) in pairs {
                    d.insert(hash_key(&k).unwrap(), k, v);
                }
                let dv = Value::dict(d);
                if let Some(h) = ohook {
                    return i.call(h, vec![dv], Vec::new());
                }
                Ok(dv)
            }
            '[' => {
                self.k += 1;
                let mut items = Vec::new();
                self.ws();
                if self.k < self.c.len() && self.c[self.k] == ']' {
                    self.k += 1;
                    return Ok(Value::list(items));
                }
                loop {
                    items.push(self.value(i, hook, ohook)?);
                    self.ws();
                    if self.k < self.c.len() && self.c[self.k] == ',' {
                        self.k += 1;
                        continue;
                    }
                    if self.k < self.c.len() && self.c[self.k] == ']' {
                        self.k += 1;
                        break;
                    }
                    return Err(json_err(i, "Expecting ',' delimiter", self));
                }
                Ok(Value::list(items))
            }
            '"' => Ok(Value::Str(self.string(i)?.into())),
            _ if rest(self, "null") => {
                self.k += 4;
                Ok(Value::None)
            }
            _ if rest(self, "true") => {
                self.k += 4;
                Ok(Value::Bool(true))
            }
            _ if rest(self, "false") => {
                self.k += 5;
                Ok(Value::Bool(false))
            }
            _ if rest(self, "NaN") => {
                self.k += 3;
                Ok(Value::Float(f64::NAN))
            }
            _ if rest(self, "Infinity") => {
                self.k += 8;
                Ok(Value::Float(f64::INFINITY))
            }
            _ if rest(self, "-Infinity") => {
                self.k += 9;
                Ok(Value::Float(f64::NEG_INFINITY))
            }
            c if c == '-' || c.is_ascii_digit() => {
                let start = self.k;
                if self.c[self.k] == '-' {
                    self.k += 1;
                }
                if self.k >= self.c.len() || !self.c[self.k].is_ascii_digit() {
                    self.k = start;
                    return Err(json_err(i, "Expecting value", self));
                }
                if self.c[self.k] == '0' {
                    self.k += 1;
                } else {
                    while self.k < self.c.len() && self.c[self.k].is_ascii_digit() {
                        self.k += 1;
                    }
                }
                let mut is_float = false;
                if self.k + 1 < self.c.len() && self.c[self.k] == '.' && self.c[self.k + 1].is_ascii_digit() {
                    is_float = true;
                    self.k += 1;
                    while self.k < self.c.len() && self.c[self.k].is_ascii_digit() {
                        self.k += 1;
                    }
                }
                if self.k < self.c.len() && matches!(self.c[self.k], 'e' | 'E') {
                    let save = self.k;
                    self.k += 1;
                    if self.k < self.c.len() && matches!(self.c[self.k], '+' | '-') {
                        self.k += 1;
                    }
                    if self.k < self.c.len() && self.c[self.k].is_ascii_digit() {
                        is_float = true;
                        while self.k < self.c.len() && self.c[self.k].is_ascii_digit() {
                            self.k += 1;
                        }
                    } else {
                        self.k = save;
                    }
                }
                let s: String = self.c[start..self.k].iter().collect();
                if is_float {
                    Ok(Value::Float(s.parse().unwrap_or(0.0)))
                } else {
                    match s.parse::<i64>() {
                        Ok(n) => Ok(Value::Int(n)),
                        Err(_) => Ok(Value::Float(s.parse().unwrap_or(0.0))),
                    }
                }
            }
            _ => Err(json_err(i, "Expecting value", self)),
        }
    }
    fn string(&mut self, i: &mut Interp) -> R<String> {
        let start = self.k;
        self.k += 1;
        let mut out = String::new();
        loop {
            if self.k >= self.c.len() {
                self.k = start;
                return Err(json_err(i, "Unterminated string starting at", self));
            }
            let c = self.c[self.k];
            self.k += 1;
            match c {
                '"' => return Ok(out),
                '\\' => {
                    if self.k >= self.c.len() {
                        return Err(json_err(i, "Unterminated string", self));
                    }
                    let e = self.c[self.k];
                    self.k += 1;
                    match e {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        '/' => out.push('/'),
                        'b' => out.push('\x08'),
                        'f' => out.push('\x0c'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'u' => {
                            let hex = |p: &mut Self| -> Option<u32> {
                                if p.k + 4 > p.c.len() {
                                    return None;
                                }
                                let s: String = p.c[p.k..p.k + 4].iter().collect();
                                p.k += 4;
                                u32::from_str_radix(&s, 16).ok()
                            };
                            let Some(u) = hex(self) else {
                                return Err(json_err(i, "Invalid \\uXXXX escape", self));
                            };
                            if (0xd800..0xdc00).contains(&u) && self.k + 1 < self.c.len() && self.c[self.k] == '\\' && self.c[self.k + 1] == 'u' {
                                let save = self.k;
                                self.k += 2;
                                match hex(self) {
                                    Some(l) if (0xdc00..0xe000).contains(&l) => {
                                        let cp = 0x10000 + ((u - 0xd800) << 10) + (l - 0xdc00);
                                        out.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
                                    }
                                    _ => {
                                        self.k = save;
                                        out.push('\u{fffd}');
                                    }
                                }
                            } else {
                                out.push(char::from_u32(u).unwrap_or('\u{fffd}'));
                            }
                        }
                        _ => return Err(json_err(i, "Invalid \\escape", self)),
                    }
                }
                c if (c as u32) < 0x20 => {
                    self.k -= 1;
                    return Err(json_err(i, "Invalid control character at", self));
                }
                c => out.push(c),
            }
        }
    }
}

fn json_loads(i: &mut Interp, a: Args, kw: Kw) -> R<Value> {
    let s = match a.first().map(|v| v.unwrap_base()) {
        Some(Value::Str(s)) => s.to_string(),
        Some(Value::Bytes(b)) => String::from_utf8_lossy(&b).into_owned(),
        _ => return i.type_error("the JSON object must be str, bytes or bytearray"),
    };
    let chars: Vec<char> = s.chars().collect();
    let mut p = JsonParser { c: &chars, k: 0 };
    let hook = kwarg(&kw, "object_pairs_hook").filter(|v| !v.is_none());
    let ohook = kwarg(&kw, "object_hook").filter(|v| !v.is_none());
    let v = p.value(i, &hook, &ohook)?;
    p.ws();
    if p.k < chars.len() {
        return Err(json_err(i, "Extra data", &p));
    }
    Ok(v)
}

fn json_escape(s: &str, ascii: bool, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c if ascii && (c as u32) > 0x7e => {
                let cp = c as u32;
                if cp > 0xffff {
                    let v = cp - 0x10000;
                    out.push_str(&format!("\\u{:04x}\\u{:04x}", 0xd800 + (v >> 10), 0xdc00 + (v & 0x3ff)));
                } else {
                    out.push_str(&format!("\\u{:04x}", cp));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

struct JsonOpts {
    indent: Option<String>,
    item_sep: String,
    key_sep: String,
    sort_keys: bool,
    ascii: bool,
    default: Option<Value>,
}

fn json_encode(i: &mut Interp, v: &Value, o: &JsonOpts, level: usize, out: &mut String) -> R<()> {
    let v = v.unwrap_base();
    match &v {
        Value::None => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Int(n) => out.push_str(&n.to_string()),
        Value::Float(f) => out.push_str(&if f.is_nan() { "NaN".into() } else if f.is_infinite() { if *f > 0.0 { "Infinity".into() } else { "-Infinity".into() } } else { float_repr(*f) }),
        Value::Str(s) => json_escape(s, o.ascii, out),
        Value::List(_) | Value::Tuple(_) => {
            let items = i.iterate(&v)?;
            if items.is_empty() {
                out.push_str("[]");
                return Ok(());
            }
            out.push('[');
            for (k, it) in items.iter().enumerate() {
                if k > 0 {
                    out.push_str(&o.item_sep);
                }
                if let Some(ind) = &o.indent {
                    out.push('\n');
                    out.push_str(&ind.repeat(level + 1));
                }
                json_encode(i, it, o, level + 1, out)?;
            }
            if let Some(ind) = &o.indent {
                out.push('\n');
                out.push_str(&ind.repeat(level));
            }
            out.push(']');
        }
        Value::Dict(d) => {
            let mut items: Vec<(Value, Value)> = d.borrow().items().cloned().collect();
            if items.is_empty() {
                out.push_str("{}");
                return Ok(());
            }
            if o.sort_keys {
                let mut keys: Vec<Value> = items.iter().map(|(k, _)| k.clone()).collect();
                crate::builtins::sort_values(i, &mut keys, None, false)?;
                let mut sorted = Vec::new();
                for k in keys {
                    let pos = items.iter().position(|(x, _)| crate::ops::identical(x, &k) || x.type_name() == k.type_name() && hash_key(x) == hash_key(&k)).unwrap();
                    sorted.push(items.remove(pos));
                }
                items = sorted;
            }
            out.push('{');
            for (k, (key, val)) in items.iter().enumerate() {
                if k > 0 {
                    out.push_str(&o.item_sep);
                }
                if let Some(ind) = &o.indent {
                    out.push('\n');
                    out.push_str(&ind.repeat(level + 1));
                }
                let ks = match key.unwrap_base() {
                    Value::Str(s) => s.to_string(),
                    Value::Bool(b) => if b { "true" } else { "false" }.to_string(),
                    Value::None => "null".into(),
                    Value::Int(n) => n.to_string(),
                    Value::Float(f) => float_repr(f),
                    other => return i.type_error(format!("keys must be str, int, float, bool or None, not {}", other.type_name())),
                };
                json_escape(&ks, o.ascii, out);
                out.push_str(&o.key_sep);
                json_encode(i, val, o, level + 1, out)?;
            }
            if let Some(ind) = &o.indent {
                out.push('\n');
                out.push_str(&ind.repeat(level));
            }
            out.push('}');
        }
        other => match &o.default {
            Some(f) => {
                let r = i.call(f, vec![other.clone()], Vec::new())?;
                json_encode(i, &r, o, level, out)?;
            }
            None => {
                if let Value::Native(n) = other {
                    if let Some(items) = n.iter(i) {
                        let items = items?;
                        return json_encode(i, &Value::list(items), o, level, out);
                    }
                }
                return i.type_error(format!("Object of type {} is not JSON serializable", other.type_name()));
            }
        },
    }
    Ok(())
}

fn json_dumps(i: &mut Interp, a: Args, kw: Kw) -> R<Value> {
    let indent = match kwarg(&kw, "indent") {
        None | Some(Value::None) => None,
        Some(Value::Int(n)) => Some(" ".repeat(n.max(0) as usize)),
        Some(Value::Str(s)) => Some(s.to_string()),
        Some(_) => None,
    };
    let (item_sep, key_sep) = match kwarg(&kw, "separators") {
        Some(Value::Tuple(t)) if t.len() == 2 => (i.to_str(&t[0])?, i.to_str(&t[1])?),
        _ => (if indent.is_some() { ",".into() } else { ", ".into() }, ": ".into()),
    };
    let sort_keys = match kwarg(&kw, "sort_keys") {
        Some(v) => i.truthy(&v)?,
        None => false,
    };
    let ascii = match kwarg(&kw, "ensure_ascii") {
        Some(v) => i.truthy(&v)?,
        None => true,
    };
    let o = JsonOpts { indent, item_sep, key_sep, sort_keys, ascii, default: kwarg(&kw, "default").filter(|v| !v.is_none()) };
    let mut out = String::new();
    json_encode(i, a.first().unwrap_or(&Value::None), &o, 0, &mut out)?;
    Ok(Value::Str(out.into()))
}

// ---------------------------------------------------------------------- base64

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &[u8], url: bool) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        let map = |x: u32| -> char {
            let c = B64[(x & 63) as usize] as char;
            if url {
                match c {
                    '+' => '-',
                    '/' => '_',
                    c => c,
                }
            } else {
                c
            }
        };
        out.push(map(n >> 18));
        out.push(map(n >> 12));
        out.push(if chunk.len() > 1 { map(n >> 6) } else { '=' });
        out.push(if chunk.len() > 2 { map(n) } else { '=' });
    }
    out
}

fn bytes_arg(i: &mut Interp, v: &Value) -> R<Vec<u8>> {
    match v.unwrap_base() {
        Value::Bytes(b) => Ok((*b).clone()),
        Value::Str(s) => Ok(s.as_bytes().to_vec()),
        Value::Native(n) => match n.repr() {
            _ => {
                if let Some(items) = n.iter(i) {
                    let items = items?;
                    items.iter().map(|x| to_int(i, x).map(|n| n as u8)).collect()
                } else {
                    i.type_error("a bytes-like object is required")
                }
            }
        },
        other => i.type_error(format!("a bytes-like object is required, not '{}'", other.type_name())),
    }
}

fn b64encode(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    let data = bytes_arg(i, a.first().unwrap_or(&Value::None))?;
    Ok(Value::Bytes(Rc::new(base64_encode(&data, false).into_bytes())))
}

fn b64encode_url(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    let data = bytes_arg(i, a.first().unwrap_or(&Value::None))?;
    Ok(Value::Bytes(Rc::new(base64_encode(&data, true).into_bytes())))
}

fn b64decode(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    let data = bytes_arg(i, a.first().unwrap_or(&Value::None))?;
    let mut out = Vec::new();
    let mut acc = 0u32;
    let mut bits = 0;
    for &c in &data {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            _ => continue,
        } as u32;
        acc = acc << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(Value::Bytes(Rc::new(out)))
}

// ---------------------------------------------------------------------- copy

fn copy_copy(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    let v = a.first().cloned().unwrap_or(Value::None);
    Ok(match &v {
        Value::List(l) => Value::list(l.borrow().clone()),
        Value::Dict(d) => Value::dict(d.borrow().clone()),
        Value::Set(d) => Value::Set(Rc::new(RefCell::new(d.borrow().clone()))),
        Value::Instance(inst) => {
            if let Some(m) = inst.class.lookup("__copy__") {
                return i.call(&m, vec![v.clone()], Vec::new());
            }
            let base = inst.base.borrow().clone();
            let base = match &base {
                Value::List(l) => Value::list(l.borrow().clone()),
                Value::Dict(d) => Value::dict(d.borrow().clone()),
                other => other.clone(),
            };
            Value::Instance(Rc::new(Instance { class: inst.class.clone(), dict: RefCell::new(inst.dict.borrow().clone()), base: RefCell::new(base) }))
        }
        other => other.clone(),
    })
}

fn deep(i: &mut Interp, v: &Value, depth: usize) -> R<Value> {
    if depth > 200 {
        return Err(i.exc("RecursionError", "maximum recursion depth exceeded in deepcopy"));
    }
    Ok(match v {
        Value::List(l) => {
            let items = l.borrow().clone();
            Value::list(items.iter().map(|x| deep(i, x, depth + 1)).collect::<R<_>>()?)
        }
        Value::Tuple(t) => Value::tuple(t.iter().map(|x| deep(i, x, depth + 1)).collect::<R<_>>()?),
        Value::Dict(d) => {
            let items: Vec<(Value, Value)> = d.borrow().items().cloned().collect();
            let mut nd = Dict::default();
            for (k, x) in items {
                let x = deep(i, &x, depth + 1)?;
                nd.insert(hash_key(&k).unwrap_or(HashKey::None), k, x);
            }
            Value::dict(nd)
        }
        Value::Set(d) => Value::Set(Rc::new(RefCell::new(d.borrow().clone()))),
        Value::Instance(inst) if !inst.class.is_enum => {
            if let Some(m) = inst.class.lookup("__deepcopy__") {
                return i.call(&m, vec![v.clone(), Value::dict(Dict::default())], Vec::new());
            }
            let mut ns = Namespace::default();
            let items: Vec<(Name, Value)> = inst.dict.borrow().iter().map(|(k, x)| (k.clone(), x.clone())).collect();
            for (k, x) in items {
                ns.insert(k, deep(i, &x, depth + 1)?);
            }
            let base = inst.base.borrow().clone();
            Value::Instance(Rc::new(Instance { class: inst.class.clone(), dict: RefCell::new(ns), base: RefCell::new(deep(i, &base, depth + 1)?) }))
        }
        other => other.clone(),
    })
}

fn copy_deepcopy(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    deep(i, a.first().unwrap_or(&Value::None), 0)
}

// ---------------------------------------------------------------------- Python-source modules

const COLLECTIONS_SRC: &str = r#"
OrderedDict = dict

class defaultdict:
    def __init__(self, default_factory=None, *args, **kwargs):
        self.default_factory = default_factory
        self._d = dict(*args, **kwargs)
    def __getitem__(self, key):
        if key in self._d:
            return self._d[key]
        if self.default_factory is None:
            raise KeyError(key)
        v = self.default_factory()
        self._d[key] = v
        return v
    def __setitem__(self, key, value):
        self._d[key] = value
    def __delitem__(self, key):
        del self._d[key]
    def __contains__(self, key):
        return key in self._d
    def __iter__(self):
        return iter(list(self._d.keys()))
    def __len__(self):
        return len(self._d)
    def __repr__(self):
        return "defaultdict(" + repr(self.default_factory) + ", " + repr(self._d) + ")"
    def get(self, key, default=None):
        return self._d.get(key, default)
    def keys(self):
        return self._d.keys()
    def values(self):
        return self._d.values()
    def items(self):
        return self._d.items()
    def pop(self, key, *default):
        return self._d.pop(key, *default)
    def setdefault(self, key, default=None):
        return self._d.setdefault(key, default)
    def update(self, *args, **kwargs):
        self._d.update(*args, **kwargs)
    def copy(self):
        n = defaultdict(self.default_factory)
        n._d = self._d.copy()
        return n

class Counter(defaultdict):
    def __init__(self, iterable=None, **kwargs):
        defaultdict.__init__(self, int)
        if iterable is not None:
            self.update(iterable)
        for k in kwargs:
            self._d[k] = kwargs[k]
    def update(self, iterable=None, **kwargs):
        if iterable is not None:
            if isinstance(iterable, dict):
                for k in iterable:
                    self._d[k] = self._d.get(k, 0) + iterable[k]
            else:
                for k in iterable:
                    self._d[k] = self._d.get(k, 0) + 1
    def __getitem__(self, key):
        return self._d.get(key, 0)
    def most_common(self, n=None):
        items = sorted(self._d.items(), key=lambda kv: kv[1], reverse=True)
        return items if n is None else items[:n]
    def total(self):
        return sum(self._d.values())

class deque:
    def __init__(self, iterable=(), maxlen=None):
        self.maxlen = maxlen
        self._l = []
        for x in iterable:
            self.append(x)
    def append(self, x):
        self._l.append(x)
        if self.maxlen is not None and len(self._l) > self.maxlen:
            self._l.pop(0)
    def appendleft(self, x):
        self._l.insert(0, x)
        if self.maxlen is not None and len(self._l) > self.maxlen:
            self._l.pop()
    def pop(self):
        return self._l.pop()
    def popleft(self):
        return self._l.pop(0)
    def extend(self, it):
        for x in it:
            self.append(x)
    def clear(self):
        self._l.clear()
    def __len__(self):
        return len(self._l)
    def __iter__(self):
        return iter(list(self._l))
    def __getitem__(self, k):
        return self._l[k]
    def __setitem__(self, k, v):
        self._l[k] = v
    def __repr__(self):
        return "deque(" + repr(self._l) + ")"

def namedtuple(typename, field_names, defaults=None):
    if isinstance(field_names, str):
        field_names = field_names.replace(",", " ").split()
    fields = tuple(field_names)
    class _NT(tuple):
        _fields = fields
        def __getattr__(self, name):
            if name in fields:
                return self[fields.index(name)]
            raise AttributeError(name)
        def _asdict(self):
            return {f: self[k] for k, f in enumerate(fields)}
        def __repr__(self):
            return typename + "(" + ", ".join(f + "=" + repr(self[k]) for k, f in enumerate(fields)) + ")"
    def make(*args, **kwargs):
        vals = list(args)
        for f in fields[len(vals):]:
            vals.append(kwargs.get(f))
        return _NT(vals)
    return make
"#;

const DATETIME_SRC: &str = r#"
import _curust_time as _t

def _divmod(a, b):
    return a // b, a % b

class timedelta:
    def __init__(self, days=0, seconds=0, microseconds=0, milliseconds=0, minutes=0, hours=0, weeks=0):
        total = ((((weeks * 7 + days) * 24 + hours) * 60 + minutes) * 60 + seconds) * 1000000 + milliseconds * 1000 + microseconds
        total = round(total)
        d, rem = _divmod(total, 86400000000)
        s, us = _divmod(rem, 1000000)
        self.days = d
        self.seconds = s
        self.microseconds = us
    def _us(self):
        return (self.days * 86400 + self.seconds) * 1000000 + self.microseconds
    def total_seconds(self):
        return self._us() / 1000000
    def __add__(self, other):
        if isinstance(other, timedelta):
            return timedelta(microseconds=self._us() + other._us())
        return NotImplemented
    def __radd__(self, other):
        return self.__add__(other)
    def __sub__(self, other):
        if isinstance(other, timedelta):
            return timedelta(microseconds=self._us() - other._us())
        return NotImplemented
    def __neg__(self):
        return timedelta(microseconds=-self._us())
    def __mul__(self, k):
        return timedelta(microseconds=round(self._us() * k))
    def __rmul__(self, k):
        return self.__mul__(k)
    def __truediv__(self, k):
        if isinstance(k, timedelta):
            return self._us() / k._us()
        return timedelta(microseconds=round(self._us() / k))
    def __floordiv__(self, k):
        if isinstance(k, timedelta):
            return self._us() // k._us()
        return timedelta(microseconds=self._us() // k)
    def __eq__(self, o):
        return isinstance(o, timedelta) and self._us() == o._us()
    def __lt__(self, o):
        return self._us() < o._us()
    def __le__(self, o):
        return self._us() <= o._us()
    def __gt__(self, o):
        return self._us() > o._us()
    def __ge__(self, o):
        return self._us() >= o._us()
    def __bool__(self):
        return self._us() != 0
    def __hash__(self):
        return hash(self._us())
    def __abs__(self):
        return self if self._us() >= 0 else -self
    def __str__(self):
        mm, ss = _divmod(self.seconds, 60)
        hh, mm = _divmod(mm, 60)
        s = "%d:%02d:%02d" % (hh, mm, ss)
        if self.days:
            s = ("%d day%s, " % (self.days, "s" if abs(self.days) != 1 else "")) + s
        if self.microseconds:
            s = s + (".%06d" % self.microseconds)
        return s
    def __repr__(self):
        parts = []
        if self.days:
            parts.append("days=%d" % self.days)
        if self.seconds:
            parts.append("seconds=%d" % self.seconds)
        if self.microseconds:
            parts.append("microseconds=%d" % self.microseconds)
        return "datetime.timedelta(" + (", ".join(parts) if parts else "0") + ")"

class date:
    def __init__(self, year, month, day):
        self.year = year
        self.month = month
        self.day = day
    @classmethod
    def today(cls):
        p = _t._local_parts()
        return cls(p[0], p[1], p[2])
    def toordinal(self):
        return _t._days_from_civil(self.year, self.month, self.day) + 719163
    @classmethod
    def fromordinal(cls, n):
        y, m, d = _t._civil_from_days(n - 719163)
        return cls(y, m, d)
    def weekday(self):
        return (self.toordinal() + 6) % 7
    def isoweekday(self):
        return self.weekday() + 1
    def strftime(self, fmt):
        return _t._strftime_parts(fmt, self.year, self.month, self.day, 0, 0, 0, 0)
    def isoformat(self):
        return "%04d-%02d-%02d" % (self.year, self.month, self.day)
    def __str__(self):
        return self.isoformat()
    def __format__(self, spec):
        return self.strftime(spec) if spec else str(self)
    def __add__(self, td):
        return date.fromordinal(self.toordinal() + td.days)
    def __sub__(self, o):
        if isinstance(o, timedelta):
            return date.fromordinal(self.toordinal() - o.days)
        return timedelta(days=self.toordinal() - o.toordinal())
    def __eq__(self, o):
        return isinstance(o, date) and self.toordinal() == o.toordinal()
    def __lt__(self, o):
        return self.toordinal() < o.toordinal()
    def replace(self, year=None, month=None, day=None):
        return date(self.year if year is None else year, self.month if month is None else month, self.day if day is None else day)

class time:
    def __init__(self, hour=0, minute=0, second=0, microsecond=0):
        self.hour = hour
        self.minute = minute
        self.second = second
        self.microsecond = microsecond
    def strftime(self, fmt):
        return _t._strftime_parts(fmt, 1900, 1, 1, self.hour, self.minute, self.second, self.microsecond)
    def isoformat(self):
        s = "%02d:%02d:%02d" % (self.hour, self.minute, self.second)
        if self.microsecond:
            s += ".%06d" % self.microsecond
        return s
    def __str__(self):
        return self.isoformat()

class datetime(date):
    def __init__(self, year, month, day, hour=0, minute=0, second=0, microsecond=0, tzinfo=None):
        if not 1 <= month <= 12:
            raise ValueError("month must be in 1..12")
        if not 0 <= hour <= 23:
            raise ValueError("hour must be in 0..23")
        if not 0 <= minute <= 59:
            raise ValueError("minute must be in 0..59")
        if not 0 <= second <= 59:
            raise ValueError("second must be in 0..59")
        self.year = year
        self.month = month
        self.day = day
        self.hour = hour
        self.minute = minute
        self.second = second
        self.microsecond = microsecond
        self.tzinfo = tzinfo
    @classmethod
    def now(cls, tz=None):
        p = _t._local_parts()
        return cls(p[0], p[1], p[2], p[3], p[4], p[5], p[6])
    @classmethod
    def today(cls):
        return cls.now()
    @classmethod
    def fromtimestamp(cls, ts, tz=None):
        p = _t._local_parts(ts)
        return cls(p[0], p[1], p[2], p[3], p[4], p[5], p[6])
    def _total_us(self):
        return ((self.toordinal() * 86400 + self.hour * 3600 + self.minute * 60 + self.second) * 1000000) + self.microsecond
    @classmethod
    def _from_us(cls, us):
        days, rem = _divmod(us, 86400000000)
        secs, micro = _divmod(rem, 1000000)
        d = date.fromordinal(days)
        return cls(d.year, d.month, d.day, secs // 3600, secs % 3600 // 60, secs % 60, micro)
    def __add__(self, td):
        if not isinstance(td, timedelta):
            return NotImplemented
        return datetime._from_us(self._total_us() + td._us())
    def __radd__(self, td):
        return self.__add__(td)
    def __sub__(self, o):
        if isinstance(o, timedelta):
            return datetime._from_us(self._total_us() - o._us())
        return timedelta(microseconds=self._total_us() - o._total_us())
    def __eq__(self, o):
        return isinstance(o, datetime) and self._total_us() == o._total_us()
    def __lt__(self, o):
        return self._total_us() < o._total_us()
    def __le__(self, o):
        return self._total_us() <= o._total_us()
    def __gt__(self, o):
        return self._total_us() > o._total_us()
    def __ge__(self, o):
        return self._total_us() >= o._total_us()
    def __hash__(self):
        return hash(self._total_us())
    def strftime(self, fmt):
        return _t._strftime_parts(fmt, self.year, self.month, self.day, self.hour, self.minute, self.second, self.microsecond)
    def date(self):
        return date(self.year, self.month, self.day)
    def time(self):
        return time(self.hour, self.minute, self.second, self.microsecond)
    def timestamp(self):
        return _t.mktime((self.year, self.month, self.day, self.hour, self.minute, self.second, 0, 0, -1)) + self.microsecond / 1000000
    def isoformat(self, sep="T"):
        s = "%04d-%02d-%02d%s%02d:%02d:%02d" % (self.year, self.month, self.day, sep, self.hour, self.minute, self.second)
        if self.microsecond:
            s += ".%06d" % self.microsecond
        return s
    def __str__(self):
        return self.isoformat(" ")
    def __repr__(self):
        return "datetime.datetime(%d, %d, %d, %d, %d, %d)" % (self.year, self.month, self.day, self.hour, self.minute, self.second)
    def replace(self, year=None, month=None, day=None, hour=None, minute=None, second=None, microsecond=None):
        return datetime(self.year if year is None else year, self.month if month is None else month, self.day if day is None else day,
                        self.hour if hour is None else hour, self.minute if minute is None else minute,
                        self.second if second is None else second, self.microsecond if microsecond is None else microsecond)
    def weekday(self):
        return (self.toordinal() + 6) % 7

MINYEAR = 1
MAXYEAR = 9999
"#;

const STRING_SRC: &str = r##"
ascii_lowercase = "abcdefghijklmnopqrstuvwxyz"
ascii_uppercase = "ABCDEFGHIJKLMNOPQRSTUVWXYZ"
ascii_letters = ascii_lowercase + ascii_uppercase
digits = "0123456789"
hexdigits = "0123456789abcdefABCDEF"
octdigits = "01234567"
punctuation = "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~"
whitespace = " \t\n\r\x0b\x0c"
printable = digits + ascii_letters + punctuation + whitespace
def capwords(s, sep=None):
    return (sep or " ").join(x.capitalize() for x in s.split(sep))
"##;

const FUNCTOOLS_SRC: &str = r#"
def reduce(function, iterable, *initial):
    it = list(iterable)
    if initial:
        acc = initial[0]
    else:
        if not it:
            raise TypeError("reduce() of empty iterable with no initial value")
        acc = it[0]
        it = it[1:]
    for x in it:
        acc = function(acc, x)
    return acc

def partial(func, *args, **kwargs):
    def call(*more, **morekw):
        kw = dict(kwargs)
        kw.update(morekw)
        return func(*(args + more), **kw)
    return call

def wraps(wrapped):
    def deco(f):
        return f
    return deco

def lru_cache(maxsize=128, typed=False):
    if callable(maxsize):
        return maxsize
    def deco(f):
        return f
    return deco

cache = lru_cache

def cmp_to_key(mycmp):
    class K:
        def __init__(self, obj):
            self.obj = obj
        def __lt__(self, other):
            return mycmp(self.obj, other.obj) < 0
        def __eq__(self, other):
            return mycmp(self.obj, other.obj) == 0
    return K
"#;
