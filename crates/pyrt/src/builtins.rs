//! Built-in functions, types and the methods of built-in values.

use crate::ast::Name;
use crate::interp::{Flow, Interp, R};
use crate::ops::{chars_len, range_len, str_slice};
use crate::value::*;
use std::cell::RefCell;
use std::rc::Rc;

type Args = Vec<Value>;
type Kw = Vec<(Name, Value)>;

fn kwarg(kw: &Kw, name: &str) -> Option<Value> {
    kw.iter().find(|(k, _)| &**k == name).map(|(_, v)| v.clone())
}

fn arg(args: &Args, kw: &Kw, i: usize, name: &str) -> Option<Value> {
    args.get(i).cloned().or_else(|| kwarg(kw, name))
}

pub fn builtin(name: &'static str, f: BuiltinFn) -> Value {
    Value::Builtin(Rc::new(Builtin { name, f, data: None }))
}

pub fn install(interp: &mut Interp) {
    let mut b = interp.builtins.borrow_mut();
    let t = &interp.types;
    for (n, c) in [
        ("object", &t.object),
        ("type", &t.type_),
        ("str", &t.str_),
        ("int", &t.int),
        ("float", &t.float),
        ("bool", &t.bool_),
        ("list", &t.list),
        ("tuple", &t.tuple),
        ("dict", &t.dict),
        ("set", &t.set),
        ("bytes", &t.bytes),
    ] {
        b.insert(n.into(), Value::Class(c.clone()));
    }
    for (n, c) in t.exceptions.iter() {
        b.insert((*n).into(), Value::Class(c.clone()));
    }
    b.insert("EnvironmentError".into(), Value::Class(t.exceptions["OSError"].clone()));
    b.insert("None".into(), Value::None);
    b.insert("NotImplemented".into(), Value::NotImplemented);
    b.insert("Ellipsis".into(), Value::Ellipsis);
    b.insert("__name__".into(), Value::str("builtins"));
    let fns: &[(&'static str, BuiltinFn)] = &[
        ("print", bi_print),
        ("len", bi_len),
        ("range", bi_range),
        ("abs", bi_abs),
        ("min", bi_min),
        ("max", bi_max),
        ("sum", bi_sum),
        ("round", bi_round),
        ("enumerate", bi_enumerate),
        ("zip", bi_zip),
        ("sorted", bi_sorted),
        ("reversed", bi_reversed),
        ("isinstance", bi_isinstance),
        ("issubclass", bi_issubclass),
        ("hasattr", bi_hasattr),
        ("getattr", bi_getattr),
        ("setattr", bi_setattr),
        ("delattr", bi_delattr),
        ("repr", bi_repr),
        ("ord", bi_ord),
        ("chr", bi_chr),
        ("hex", bi_hex),
        ("bin", bi_bin),
        ("oct", bi_oct),
        ("divmod", bi_divmod),
        ("pow", bi_pow),
        ("map", bi_map),
        ("filter", bi_filter),
        ("any", bi_any),
        ("all", bi_all),
        ("iter", bi_iter),
        ("next", bi_next),
        ("id", bi_id),
        ("hash", bi_hash),
        ("callable", bi_callable),
        ("format", bi_format),
        ("staticmethod", bi_staticmethod),
        ("classmethod", bi_classmethod),
        ("property", bi_property),
        ("frozenset", bi_frozenset),
        ("vars", bi_vars),
        ("dir", bi_dir),
        ("super", bi_super),
        ("ascii", bi_repr),
        ("open", bi_open),
    ];
    for (n, f) in fns {
        b.insert((*n).into(), builtin(n, *f));
    }
}

// ---------------------------------------------------------------------- builtin functions

fn bi_print(i: &mut Interp, a: Args, kw: Kw) -> R<Value> {
    let sep = match kwarg(&kw, "sep") {
        Some(Value::Str(s)) => s.to_string(),
        _ => " ".into(),
    };
    let mut parts = Vec::new();
    for v in &a {
        parts.push(i.to_str(v)?);
    }
    let text = parts.join(&sep);
    match i.host.clone() {
        Some(h) => h.print(&text),
        None => eprintln!("{}", text),
    }
    Ok(Value::None)
}

pub fn length(i: &mut Interp, v: &Value) -> R<usize> {
    Ok(match v {
        Value::Str(s) => chars_len(s),
        Value::Bytes(b) => b.len(),
        Value::List(l) => l.borrow().len(),
        Value::Tuple(t) => t.len(),
        Value::Dict(d) | Value::Set(d) | Value::FrozenSet(d) => d.borrow().len(),
        Value::Range(a, b, c) => range_len(*a, *b, *c) as usize,
        Value::Class(c) if c.is_enum => c.members.borrow().len(),
        Value::Instance(inst) => {
            if let Some(m) = inst.class.lookup("__len__") {
                let r = i.call(&m, vec![v.clone()], Vec::new())?;
                return match r {
                    Value::Int(n) => Ok(n as usize),
                    _ => i.type_error("__len__ should return an int"),
                };
            }
            let b = inst.base.borrow().clone();
            if !b.is_none() {
                return length(i, &b);
            }
            return i.type_error(format!("object of type '{}' has no len()", inst.class.name));
        }
        Value::Native(n) => match n.len() {
            Some(l) => l,
            None => return i.type_error(format!("object of type '{}' has no len()", n.type_name())),
        },
        other => return i.type_error(format!("object of type '{}' has no len()", other.type_name())),
    })
}

fn bi_len(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    let v = a.first().cloned().unwrap_or(Value::None);
    Ok(Value::Int(length(i, &v)? as i64))
}

pub fn to_int(i: &mut Interp, v: &Value) -> R<i64> {
    match v.unwrap_base() {
        Value::Int(n) => Ok(n),
        Value::Bool(b) => Ok(b as i64),
        Value::Float(f) => {
            if !f.is_finite() {
                return Err(i.exc("OverflowError", "cannot convert float infinity to integer"));
            }
            Ok(f.trunc() as i64)
        }
        other => {
            if let Value::Instance(inst) = &other {
                if let Some(m) = inst.class.lookup("__index__").or_else(|| inst.class.lookup("__int__")) {
                    let r = i.call(&m, vec![other.clone()], Vec::new())?;
                    return to_int(i, &r);
                }
            }
            i.type_error(format!("'{}' object cannot be interpreted as an integer", other.type_name()))
        }
    }
}

pub fn to_float(i: &mut Interp, v: &Value) -> R<f64> {
    match v.unwrap_base() {
        Value::Int(n) => Ok(n as f64),
        Value::Bool(b) => Ok(b as i64 as f64),
        Value::Float(f) => Ok(f),
        Value::Native(n) => match n.as_any().downcast_ref::<crate::numpy::NdArray>() {
            Some(a) if a.data.borrow().len() == 1 => Ok(a.data.borrow()[0]),
            _ => i.type_error(format!("must be real number, not {}", n.type_name())),
        },
        other => i.type_error(format!("must be real number, not {}", other.type_name())),
    }
}

fn bi_range(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    let n: Vec<i64> = a.iter().map(|v| to_int(i, v)).collect::<R<_>>()?;
    Ok(match n.len() {
        1 => Value::Range(0, n[0], 1),
        2 => Value::Range(n[0], n[1], 1),
        3 => {
            if n[2] == 0 {
                return i.value_error("range() arg 3 must not be zero");
            }
            Value::Range(n[0], n[1], n[2])
        }
        _ => return i.type_error("range expected at most 3 arguments"),
    })
}

fn bi_abs(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    match a.first().map(|v| v.unwrap_base()) {
        Some(Value::Int(n)) => Ok(Value::Int(n.abs())),
        Some(Value::Bool(b)) => Ok(Value::Int(b as i64)),
        Some(Value::Float(f)) => Ok(Value::Float(f.abs())),
        Some(v @ Value::Native(_)) => crate::numpy::call_function(i, "fabs", vec![v], Vec::new()),
        Some(v) => i.type_error(format!("bad operand type for abs(): '{}'", v.type_name())),
        None => i.type_error("abs() takes exactly one argument"),
    }
}

fn min_max(i: &mut Interp, a: Args, kw: Kw, want_max: bool) -> R<Value> {
    let items = if a.len() == 1 { i.iterate(&a[0])? } else { a };
    let key = kwarg(&kw, "key").filter(|k| !k.is_none());
    if items.is_empty() {
        if let Some(d) = kwarg(&kw, "default") {
            return Ok(d);
        }
        return i.value_error(format!("{}() arg is an empty sequence", if want_max { "max" } else { "min" }));
    }
    let mut best = items[0].clone();
    let mut best_k = match &key {
        Some(k) => i.call(k, vec![best.clone()], Vec::new())?,
        None => best.clone(),
    };
    for v in items.into_iter().skip(1) {
        let k = match &key {
            Some(kf) => i.call(kf, vec![v.clone()], Vec::new())?,
            None => v.clone(),
        };
        let better = if want_max { i.less(&best_k, &k)? } else { i.less(&k, &best_k)? };
        if better {
            best = v;
            best_k = k;
        }
    }
    Ok(best)
}

fn bi_min(i: &mut Interp, a: Args, kw: Kw) -> R<Value> {
    min_max(i, a, kw, false)
}
fn bi_max(i: &mut Interp, a: Args, kw: Kw) -> R<Value> {
    min_max(i, a, kw, true)
}

fn bi_sum(i: &mut Interp, a: Args, kw: Kw) -> R<Value> {
    let items = i.iterate(a.first().unwrap_or(&Value::list(Vec::new())))?;
    let mut acc = arg(&a, &kw, 1, "start").unwrap_or(Value::Int(0));
    // CPython 3.12: exact ints, then Neumaier-compensated float summation.
    let mut k = 0;
    while k < items.len() {
        if let Value::Float(start) = acc {
            let mut f = start;
            let mut c = 0.0f64;
            while k < items.len() {
                let x = match &items[k] {
                    Value::Float(x) => *x,
                    Value::Int(n) => *n as f64,
                    Value::Bool(b) => *b as i64 as f64,
                    _ => break,
                };
                let t = f + x;
                if f.abs() >= x.abs() {
                    c += (f - t) + x;
                } else {
                    c += (x - t) + f;
                }
                f = t;
                k += 1;
            }
            if c != 0.0 && c.is_finite() {
                f += c;
            }
            acc = Value::Float(f);
            if k >= items.len() {
                break;
            }
        }
        acc = i.binop(crate::ast::BinOp::Add, &acc, &items[k])?;
        k += 1;
    }
    Ok(acc)
}

/// Python's `round(x, n)`: correctly rounded via the decimal representation.
pub fn py_round(x: f64, n: i64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    if n > 22 {
        return x;
    }
    if n >= 0 {
        let s = format!("{:.*}", n as usize, x);
        s.parse().unwrap_or(x)
    } else {
        let p = 10f64.powi((-n) as i32);
        let y = x / p;
        let r = y.round();
        let r = if (y - y.trunc()).abs() == 0.5 { 2.0 * (y / 2.0).round() } else { r };
        r * p
    }
}

fn bi_round(i: &mut Interp, a: Args, kw: Kw) -> R<Value> {
    let x = a.first().cloned().unwrap_or(Value::None).unwrap_base();
    let nd = arg(&a, &kw, 1, "ndigits").filter(|v| !v.is_none());
    if let Value::Instance(inst) = &x {
        if let Some(m) = inst.class.lookup("__round__") {
            let mut args = vec![x.clone()];
            if let Some(n) = nd {
                args.push(n);
            }
            return i.call(&m, args, Vec::new());
        }
    }
    match (x, nd) {
        (Value::Int(n), None) => Ok(Value::Int(n)),
        (Value::Bool(b), None) => Ok(Value::Int(b as i64)),
        (Value::Int(n), Some(d)) => {
            let d = to_int(i, &d)?;
            if d >= 0 {
                Ok(Value::Int(n))
            } else {
                Ok(Value::Int(py_round(n as f64, d) as i64))
            }
        }
        (Value::Float(f), None) => {
            if !f.is_finite() {
                return i.value_error("cannot convert float NaN or infinity to integer");
            }
            let r = f.round();
            let r = if (f - f.trunc()).abs() == 0.5 { 2.0 * (f / 2.0).round() } else { r };
            Ok(Value::Int(r as i64))
        }
        (Value::Float(f), Some(d)) => {
            let d = to_int(i, &d)?;
            Ok(Value::Float(py_round(f, d)))
        }
        (v @ Value::Native(_), d) => {
            let mut args = vec![v];
            if let Some(d) = d {
                args.push(d);
            }
            crate::numpy::call_function(i, "round", args, Vec::new())
        }
        (v, _) => i.type_error(format!("type {} doesn't define __round__ method", v.type_name())),
    }
}

fn bi_enumerate(i: &mut Interp, a: Args, kw: Kw) -> R<Value> {
    let items = i.iterate(a.first().unwrap_or(&Value::None))?;
    let start = match arg(&a, &kw, 1, "start") {
        Some(v) => to_int(i, &v)?,
        None => 0,
    };
    let out: Vec<Value> = items.into_iter().enumerate().map(|(k, v)| Value::tuple(vec![Value::Int(start + k as i64), v])).collect();
    Ok(Value::Iter(Rc::new(RefCell::new(PyIter::Seq(out, 0)))))
}

fn bi_zip(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    let lists: Vec<Vec<Value>> = a.iter().map(|v| i.iterate(v)).collect::<R<_>>()?;
    let n = lists.iter().map(|l| l.len()).min().unwrap_or(0);
    let out: Vec<Value> = (0..n).map(|k| Value::tuple(lists.iter().map(|l| l[k].clone()).collect())).collect();
    Ok(Value::Iter(Rc::new(RefCell::new(PyIter::Seq(out, 0)))))
}

/// Stable sort with a fallible comparison (merge sort).
pub fn sort_values(i: &mut Interp, items: &mut Vec<Value>, key: Option<Value>, reverse: bool) -> R<()> {
    let keys: Vec<Value> = match &key {
        Some(k) if !k.is_none() => items.iter().map(|v| i.call(k, vec![v.clone()], Vec::new())).collect::<R<_>>()?,
        _ => items.clone(),
    };
    let mut idx: Vec<usize> = (0..items.len()).collect();
    // Insertion into runs + merge: simple top-down merge sort.
    fn merge_sort(i: &mut Interp, idx: &mut [usize], keys: &[Value], reverse: bool, tmp: &mut Vec<usize>) -> R<()> {
        let n = idx.len();
        if n <= 1 {
            return Ok(());
        }
        let mid = n / 2;
        merge_sort(i, &mut idx[..mid], keys, reverse, tmp)?;
        merge_sort(i, &mut idx[mid..], keys, reverse, tmp)?;
        tmp.clear();
        let (mut a, mut b) = (0, mid);
        while a < mid && b < n {
            // Take from the right only when strictly less (stability); reverse keeps stability
            // like CPython (equal elements keep their order).
            let take_right = if reverse { i.less(&keys[idx[a]], &keys[idx[b]])? } else { i.less(&keys[idx[b]], &keys[idx[a]])? };
            if take_right {
                tmp.push(idx[b]);
                b += 1;
            } else {
                tmp.push(idx[a]);
                a += 1;
            }
        }
        tmp.extend_from_slice(&idx[a..mid]);
        tmp.extend_from_slice(&idx[b..n]);
        idx.copy_from_slice(tmp);
        Ok(())
    }
    let mut tmp = Vec::with_capacity(idx.len());
    merge_sort(i, &mut idx, &keys, reverse, &mut tmp)?;
    let sorted: Vec<Value> = idx.into_iter().map(|k| items[k].clone()).collect();
    *items = sorted;
    Ok(())
}

fn bi_sorted(i: &mut Interp, a: Args, kw: Kw) -> R<Value> {
    let mut items = i.iterate(a.first().unwrap_or(&Value::None))?;
    let reverse = match kwarg(&kw, "reverse") {
        Some(v) => i.truthy(&v)?,
        None => false,
    };
    sort_values(i, &mut items, kwarg(&kw, "key"), reverse)?;
    Ok(Value::list(items))
}

fn bi_reversed(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    let mut items = i.iterate(a.first().unwrap_or(&Value::None))?;
    items.reverse();
    Ok(Value::Iter(Rc::new(RefCell::new(PyIter::Seq(items, 0)))))
}

/// The class of a value (built-in values map to the built-in classes).
pub fn class_of(i: &Interp, v: &Value) -> Rc<Class> {
    let t = &i.types;
    match v {
        Value::Instance(inst) => inst.class.clone(),
        Value::Str(_) => t.str_.clone(),
        Value::Int(_) => t.int.clone(),
        Value::Float(_) => t.float.clone(),
        Value::Bool(_) => t.bool_.clone(),
        Value::List(_) => t.list.clone(),
        Value::Tuple(_) => t.tuple.clone(),
        Value::Dict(_) => t.dict.clone(),
        Value::Set(_) | Value::FrozenSet(_) => t.set.clone(),
        Value::Bytes(_) => t.bytes.clone(),
        Value::Class(_) => t.type_.clone(),
        _ => t.object.clone(),
    }
}

pub fn isinstance(i: &mut Interp, v: &Value, c: &Value) -> R<bool> {
    match c {
        Value::Tuple(items) => {
            for it in items.iter() {
                if isinstance(i, v, it)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Value::Class(cls) => {
            let vc = class_of(i, v);
            Ok(vc.is_subclass_of(cls) || (Rc::ptr_eq(cls, &i.types.object)))
        }
        Value::Native(n) => Ok(match (v, n.as_any().downcast_ref::<crate::modules::TypeAlias>()) {
            (_, Some(alias)) => alias.matches(i, v),
            _ => false,
        }),
        _ => i.type_error("isinstance() arg 2 must be a type or tuple of types"),
    }
}

fn bi_isinstance(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    if a.len() != 2 {
        return i.type_error("isinstance expected 2 arguments");
    }
    Ok(Value::Bool(isinstance(i, &a[0], &a[1])?))
}

fn bi_issubclass(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    match (a.first(), a.get(1)) {
        (Some(Value::Class(x)), Some(Value::Class(y))) => Ok(Value::Bool(x.is_subclass_of(y))),
        (Some(Value::Class(x)), Some(Value::Tuple(t))) => Ok(Value::Bool(t.iter().any(|y| matches!(y, Value::Class(y) if x.is_subclass_of(y))))),
        _ => i.type_error("issubclass() arg 1 must be a class"),
    }
}

fn bi_hasattr(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    let name = i.to_str(a.get(1).unwrap_or(&Value::None))?;
    match i.get_attr_opt(&a[0], &name) {
        Ok(v) => Ok(Value::Bool(v.is_some())),
        Err(Flow::Exc(_)) => Ok(Value::Bool(false)),
        Err(e) => Err(e),
    }
}

fn bi_getattr(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    let name = i.to_str(a.get(1).unwrap_or(&Value::None))?;
    match i.get_attr_opt(&a[0], &name)? {
        Some(v) => Ok(v),
        None => match a.get(2) {
            Some(d) => Ok(d.clone()),
            None => Err(i.exc("AttributeError", format!("'{}' object has no attribute '{}'", a[0].type_name(), name))),
        },
    }
}

fn bi_setattr(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    let name = i.to_str(a.get(1).unwrap_or(&Value::None))?;
    i.set_attr(&a[0], &name, a.get(2).cloned().unwrap_or(Value::None))?;
    Ok(Value::None)
}

fn bi_delattr(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    let name = i.to_str(a.get(1).unwrap_or(&Value::None))?;
    if let Value::Instance(inst) = &a[0] {
        inst.dict.borrow_mut().remove(name.as_str());
    }
    Ok(Value::None)
}

fn bi_repr(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    Ok(Value::Str(i.repr(a.first().unwrap_or(&Value::None))?.into()))
}

fn bi_ord(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    match a.first().map(|v| v.unwrap_base()) {
        Some(Value::Str(s)) if chars_len(&s) == 1 => Ok(Value::Int(s.chars().next().unwrap() as i64)),
        Some(Value::Bytes(b)) if b.len() == 1 => Ok(Value::Int(b[0] as i64)),
        _ => i.type_error("ord() expected a character"),
    }
}

fn bi_chr(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    let n = to_int(i, a.first().unwrap_or(&Value::None))?;
    match char::from_u32(n as u32) {
        Some(c) => Ok(Value::Str(c.to_string().into())),
        None => i.value_error("chr() arg not in range(0x110000)"),
    }
}

fn radix_str(i: &mut Interp, a: &Args, prefix: &str, radix: u32) -> R<Value> {
    let n = to_int(i, a.first().unwrap_or(&Value::None))?;
    let digits = match radix {
        16 => format!("{:x}", n.unsigned_abs()),
        8 => format!("{:o}", n.unsigned_abs()),
        _ => format!("{:b}", n.unsigned_abs()),
    };
    Ok(Value::Str(format!("{}{}{}", if n < 0 { "-" } else { "" }, prefix, digits).into()))
}

fn bi_hex(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    radix_str(i, &a, "0x", 16)
}
fn bi_bin(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    radix_str(i, &a, "0b", 2)
}
fn bi_oct(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    radix_str(i, &a, "0o", 8)
}

fn bi_divmod(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    let q = i.binop(crate::ast::BinOp::FloorDiv, &a[0], &a[1])?;
    let r = i.binop(crate::ast::BinOp::Mod, &a[0], &a[1])?;
    Ok(Value::tuple(vec![q, r]))
}

fn bi_pow(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    if a.len() == 3 {
        let (b, e, m) = (to_int(i, &a[0])?, to_int(i, &a[1])?, to_int(i, &a[2])?);
        let mut r: i128 = 1;
        let mut base = (b as i128).rem_euclid(m as i128);
        let mut e = e;
        while e > 0 {
            if e & 1 == 1 {
                r = r * base % m as i128;
            }
            base = base * base % m as i128;
            e >>= 1;
        }
        return Ok(Value::Int(r as i64));
    }
    i.binop(crate::ast::BinOp::Pow, &a[0], &a[1])
}

fn bi_map(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    let f = a[0].clone();
    let lists: Vec<Vec<Value>> = a[1..].iter().map(|v| i.iterate(v)).collect::<R<_>>()?;
    let n = lists.iter().map(|l| l.len()).min().unwrap_or(0);
    let mut out = Vec::with_capacity(n);
    for k in 0..n {
        let args: Vec<Value> = lists.iter().map(|l| l[k].clone()).collect();
        out.push(i.call(&f, args, Vec::new())?);
    }
    Ok(Value::Iter(Rc::new(RefCell::new(PyIter::Seq(out, 0)))))
}

fn bi_filter(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    let f = a[0].clone();
    let items = i.iterate(&a[1])?;
    let mut out = Vec::new();
    for v in items {
        let keep = if f.is_none() { i.truthy(&v)? } else {
            let r = i.call(&f, vec![v.clone()], Vec::new())?;
            i.truthy(&r)?
        };
        if keep {
            out.push(v);
        }
    }
    Ok(Value::Iter(Rc::new(RefCell::new(PyIter::Seq(out, 0)))))
}

fn bi_any(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    for v in i.iterate(&a[0])? {
        if i.truthy(&v)? {
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(false))
}

fn bi_all(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    for v in i.iterate(&a[0])? {
        if !i.truthy(&v)? {
            return Ok(Value::Bool(false));
        }
    }
    Ok(Value::Bool(true))
}

fn bi_iter(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    let v = a.first().cloned().unwrap_or(Value::None);
    if let Value::Iter(_) = &v {
        return Ok(v);
    }
    if let Value::Instance(inst) = &v {
        if let Some(m) = inst.class.lookup("__iter__") {
            return i.call(&m, vec![v.clone()], Vec::new());
        }
    }
    let items = i.iterate(&v)?;
    Ok(Value::Iter(Rc::new(RefCell::new(PyIter::Seq(items, 0)))))
}

fn bi_next(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    let v = a.first().cloned().unwrap_or(Value::None);
    match &v {
        Value::Iter(it) => {
            let mut it = it.borrow_mut();
            let r = match &mut *it {
                PyIter::Seq(items, k) => {
                    if *k < items.len() {
                        *k += 1;
                        Some(items[*k - 1].clone())
                    } else {
                        None
                    }
                }
                PyIter::Range(s, e, st) => {
                    if (*st > 0 && *s < *e) || (*st < 0 && *s > *e) {
                        let r = *s;
                        *s += *st;
                        Some(Value::Int(r))
                    } else {
                        None
                    }
                }
                PyIter::Chars(s, k) => {
                    let c = s.chars().nth(*k);
                    *k += 1;
                    c.map(|c| Value::Str(c.to_string().into()))
                }
                PyIter::Done => None,
            };
            drop(it);
            match r {
                Some(x) => Ok(x),
                None => match a.get(1) {
                    Some(d) => Ok(d.clone()),
                    None => Err(i.exc("StopIteration", "")),
                },
            }
        }
        Value::Instance(inst) => match inst.class.lookup("__next__") {
            Some(m) => match i.call(&m, vec![v.clone()], Vec::new()) {
                Err(Flow::Exc(e)) if i.exc_type_name(&e) == "StopIteration" && a.len() > 1 => Ok(a[1].clone()),
                r => r,
            },
            None => i.type_error(format!("'{}' object is not an iterator", inst.class.name)),
        },
        other => i.type_error(format!("'{}' object is not an iterator", other.type_name())),
    }
}

fn bi_id(_i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    Ok(Value::Int(match hash_key(&a[0]) {
        Some(HashKey::Id(p)) => p as i64,
        _ => match &a[0] {
            Value::List(l) => Rc::as_ptr(l) as *const u8 as i64,
            Value::Dict(d) => Rc::as_ptr(d) as *const u8 as i64,
            _ => 0x1000,
        },
    }))
}

fn bi_hash(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    use std::hash::{Hash, Hasher};
    let k = i.hash(&a[0])?;
    let mut h = Fnv::default();
    k.hash(&mut h);
    Ok(Value::Int(h.finish() as i64))
}

fn bi_callable(_i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    Ok(Value::Bool(match &a[0] {
        Value::Func(_) | Value::Builtin(_) | Value::Method(_) | Value::BoundMethod(_) | Value::Class(_) => true,
        Value::Instance(inst) => inst.class.lookup("__call__").is_some(),
        _ => false,
    }))
}

fn bi_format(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    let spec = match a.get(1) {
        Some(v) => i.to_str(v)?,
        None => String::new(),
    };
    Ok(Value::Str(crate::format::format_value(i, &a[0], &spec)?.into()))
}

fn bi_staticmethod(_i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    Ok(Value::StaticMethod(Rc::new(a[0].clone())))
}
fn bi_classmethod(_i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    Ok(Value::ClassMethod(Rc::new(a[0].clone())))
}
fn bi_property(_i: &mut Interp, a: Args, kw: Kw) -> R<Value> {
    let getter = arg(&a, &kw, 0, "fget").unwrap_or(Value::None);
    let setter = arg(&a, &kw, 1, "fset").unwrap_or(Value::None);
    Ok(Value::Property(Rc::new((getter, setter))))
}

fn bi_frozenset(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    let mut d = Dict::default();
    if let Some(v) = a.first() {
        for x in i.iterate(v)? {
            let hk = i.hash(&x)?;
            d.insert(hk, x, Value::None);
        }
    }
    Ok(Value::FrozenSet(Rc::new(RefCell::new(d))))
}

fn bi_vars(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    i.get_attr(&a[0], "__dict__")
}

fn bi_dir(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    let mut names: Vec<String> = Vec::new();
    match a.first() {
        Some(Value::Instance(inst)) => {
            names.extend(inst.dict.borrow().keys().map(|k| k.to_string()));
            for c in inst.class.mro.borrow().iter() {
                names.extend(c.dict.borrow().keys().map(|k| k.to_string()));
            }
        }
        Some(Value::Module(m)) => names.extend(m.dict.borrow().keys().map(|k| k.to_string())),
        Some(Value::Class(c)) => {
            for k in c.mro.borrow().iter() {
                names.extend(k.dict.borrow().keys().map(|k| k.to_string()));
            }
        }
        _ => {}
    }
    names.sort();
    names.dedup();
    let _ = i;
    Ok(Value::list(names.into_iter().map(|n| Value::Str(n.into())).collect()))
}

fn bi_super(i: &mut Interp, a: Args, _kw: Kw) -> R<Value> {
    match (a.first(), a.get(1)) {
        (Some(Value::Class(c)), Some(obj)) => Ok(Value::Super(Rc::new((c.clone(), obj.clone())))),
        _ => Err(i.exc("RuntimeError", "super(): no arguments")),
    }
}

fn bi_open(i: &mut Interp, _a: Args, _kw: Kw) -> R<Value> {
    Err(i.exc("PermissionError", "post-processing scripts can't open files in Curust"))
}

// ---------------------------------------------------------------------- constructors

/// Calling a built-in type: `str(x)`, `int("3")`, `list(it)`, …
pub fn construct_builtin(i: &mut Interp, c: &Rc<Class>, a: &Args, kw: &Kw) -> Option<R<Value>> {
    let first = a.first().cloned();
    Some((|| -> R<Value> {
        match c.builtin {
            "str" => {
                if let (Some(Value::Bytes(b)), Some(_)) = (&first, a.get(1).or(kwarg(kw, "encoding").as_ref())) {
                    return Ok(Value::Str(String::from_utf8_lossy(b).into_owned().into()));
                }
                Ok(match first {
                    None => Value::str(""),
                    Some(v) => Value::Str(i.to_str(&v)?.into()),
                })
            }
            "int" => match first.map(|v| v.unwrap_base()) {
                None => Ok(Value::Int(0)),
                Some(Value::Str(s)) => {
                    let base = match arg(a, kw, 1, "base") {
                        Some(b) => to_int(i, &b)? as u32,
                        None => 10,
                    };
                    let t = s.trim().replace('_', "");
                    let (neg, body) = match t.strip_prefix('-') {
                        Some(r) => (true, r.to_string()),
                        None => (false, t.strip_prefix('+').unwrap_or(&t).to_string()),
                    };
                    let (body, base) = if base == 16 || base == 0 {
                        match body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
                            Some(r) => (r.to_string(), 16),
                            None => (body, if base == 0 { 10 } else { base }),
                        }
                    } else {
                        (body, base)
                    };
                    match i64::from_str_radix(&body, base) {
                        Ok(n) if !body.is_empty() && !body.starts_with(['+', '-']) => Ok(Value::Int(if neg { -n } else { n })),
                        _ => i.value_error(format!("invalid literal for int() with base {}: {}", base, crate::value::str_repr(&s))),
                    }
                }
                Some(Value::Bytes(b)) => {
                    let s = String::from_utf8_lossy(&b).trim().to_string();
                    match s.parse::<i64>() {
                        Ok(n) => Ok(Value::Int(n)),
                        Err(_) => i.value_error("invalid literal for int()"),
                    }
                }
                Some(v) => Ok(Value::Int(to_int(i, &v)?)),
            },
            "float" => match first.map(|v| v.unwrap_base()) {
                None => Ok(Value::Float(0.0)),
                Some(Value::Str(s)) => match parse_float(&s) {
                    Some(f) => Ok(Value::Float(f)),
                    None => i.value_error(format!("could not convert string to float: {}", crate::value::str_repr(&s))),
                },
                Some(v) => Ok(Value::Float(to_float(i, &v)?)),
            },
            "bool" => Ok(Value::Bool(match first {
                None => false,
                Some(v) => i.truthy(&v)?,
            })),
            "list" => Ok(Value::list(match first {
                None => Vec::new(),
                Some(v) => i.iterate(&v)?,
            })),
            "tuple" => Ok(Value::tuple(match first {
                None => Vec::new(),
                Some(v) => i.iterate(&v)?,
            })),
            "set" => {
                let mut d = Dict::default();
                if let Some(v) = first {
                    for x in i.iterate(&v)? {
                        let hk = i.hash(&x)?;
                        d.insert(hk, x, Value::None);
                    }
                }
                Ok(Value::Set(Rc::new(RefCell::new(d))))
            }
            "dict" => {
                let mut d = Dict::default();
                if let Some(v) = first {
                    match &v {
                        Value::Dict(src) => d = src.borrow().clone(),
                        _ => {
                            for pair in i.iterate(&v)? {
                                let kv = i.iterate(&pair)?;
                                if kv.len() != 2 {
                                    return i.value_error("dictionary update sequence element has wrong length");
                                }
                                let hk = i.hash(&kv[0])?;
                                d.insert(hk, kv[0].clone(), kv[1].clone());
                            }
                        }
                    }
                }
                for (k, v) in kw {
                    d.insert(HashKey::Str(k.clone()), Value::Str(k.clone()), v.clone());
                }
                Ok(Value::dict(d))
            }
            "bytes" => match first {
                None => Ok(Value::Bytes(Rc::new(Vec::new()))),
                Some(Value::Str(s)) => Ok(Value::Bytes(Rc::new(s.as_bytes().to_vec()))),
                Some(Value::Int(n)) => Ok(Value::Bytes(Rc::new(vec![0; n.max(0) as usize]))),
                Some(v) => {
                    let items = i.iterate(&v)?;
                    let b: Vec<u8> = items.iter().map(|x| to_int(i, x).map(|n| n as u8)).collect::<R<_>>()?;
                    Ok(Value::Bytes(Rc::new(b)))
                }
            },
            "type" => match first {
                Some(v) if a.len() == 1 => Ok(Value::Class(class_of(i, &v))),
                _ => i.type_error("type() with 3 arguments is not supported"),
            },
            "object" => Ok(Value::Instance(Rc::new(Instance { class: c.clone(), dict: RefCell::new(Namespace::default()), base: RefCell::new(Value::None) }))),
            _ => Err(Flow::Break),
        }
    })())
    .and_then(|r| match r {
        Err(Flow::Break) => None,
        other => Some(other),
    })
}

/// Python's `float(str)`.
pub fn parse_float(s: &str) -> Option<f64> {
    let t = s.trim().replace('_', "");
    let l = t.to_ascii_lowercase();
    let body = l.trim_start_matches(['+', '-']);
    let neg = l.starts_with('-');
    match body {
        "inf" | "infinity" => return Some(if neg { f64::NEG_INFINITY } else { f64::INFINITY }),
        "nan" => return Some(f64::NAN),
        _ => {}
    }
    if t.is_empty() || t.contains(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'))) {
        return None;
    }
    t.parse::<f64>().ok()
}

// ---------------------------------------------------------------------- attributes of values

const STR_METHODS: &[&str] = &[
    "split", "rsplit", "strip", "lstrip", "rstrip", "startswith", "endswith", "find", "rfind", "index", "rindex", "replace", "join", "upper", "lower", "isdigit", "isnumeric", "isdecimal", "isalpha", "isalnum",
    "isspace", "isupper", "islower", "format", "count", "splitlines", "partition", "rpartition", "zfill", "ljust", "rjust", "center", "capitalize", "title", "swapcase", "encode", "removeprefix",
    "removesuffix", "casefold", "expandtabs", "format_map", "isidentifier", "isprintable", "istitle", "maketrans", "translate", "__len__", "__contains__", "__getitem__",
];
const LIST_METHODS: &[&str] = &["append", "extend", "insert", "pop", "remove", "index", "count", "sort", "reverse", "copy", "clear", "__len__", "__getitem__", "__contains__"];
const TUPLE_METHODS: &[&str] = &["index", "count", "__len__", "__getitem__"];
const DICT_METHODS: &[&str] = &["get", "items", "keys", "values", "setdefault", "update", "pop", "popitem", "copy", "clear", "fromkeys", "__getitem__", "__contains__", "__len__"];
const SET_METHODS: &[&str] = &["add", "remove", "discard", "pop", "union", "intersection", "difference", "symmetric_difference", "issubset", "issuperset", "isdisjoint", "copy", "clear", "update", "__contains__", "__len__"];
const BYTES_METHODS: &[&str] = &["decode", "hex", "startswith", "endswith", "find", "split", "strip", "replace", "join", "__len__"];
const INT_METHODS: &[&str] = &["bit_length", "to_bytes", "conjugate", "__index__"];
const FLOAT_METHODS: &[&str] = &["is_integer", "as_integer_ratio", "hex", "conjugate"];

pub fn method_lookup(v: &Value, name: &str) -> Option<&'static str> {
    let table: &[&'static str] = match v {
        Value::Str(_) => STR_METHODS,
        Value::List(_) => LIST_METHODS,
        Value::Tuple(_) => TUPLE_METHODS,
        Value::Dict(_) => DICT_METHODS,
        Value::Set(_) | Value::FrozenSet(_) => SET_METHODS,
        Value::Bytes(_) => BYTES_METHODS,
        Value::Int(_) | Value::Bool(_) => INT_METHODS,
        Value::Float(_) => FLOAT_METHODS,
        _ => return None,
    };
    table.iter().find(|m| **m == name).copied()
}

pub fn value_attr(_i: &mut Interp, v: &Value, name: &str) -> Option<Value> {
    if let Some(m) = method_lookup(v, name) {
        return Some(Value::Method(Rc::new((v.clone(), m))));
    }
    match (v, name) {
        (Value::Int(n), "real" | "numerator") => Some(Value::Int(*n)),
        (Value::Int(_), "imag") => Some(Value::Int(0)),
        (Value::Int(_), "denominator") => Some(Value::Int(1)),
        (Value::Float(f), "real") => Some(Value::Float(*f)),
        (Value::Float(_), "imag") => Some(Value::Float(0.0)),
        (Value::Slice(s), "start") => Some(s.0.clone()),
        (Value::Slice(s), "stop") => Some(s.1.clone()),
        (Value::Slice(s), "step") => Some(s.2.clone()),
        (Value::Range(a, _, _), "start") => Some(Value::Int(*a)),
        (Value::Range(_, b, _), "stop") => Some(Value::Int(*b)),
        (Value::Range(_, _, c), "step") => Some(Value::Int(*c)),
        _ => None,
    }
}

/// Attributes of the built-in classes (`str.join`, `dict.fromkeys`, …).
pub fn class_attr(i: &mut Interp, c: &Rc<Class>, name: &str) -> Option<Value> {
    if name == "__name__" {
        return Some(Value::Str(c.name.clone()));
    }
    let sample = match c.builtin {
        "str" => Value::str(""),
        "list" => Value::list(Vec::new()),
        "dict" => Value::dict(Dict::default()),
        "int" => Value::Int(0),
        "float" => Value::Float(0.0),
        "tuple" => Value::tuple(Vec::new()),
        _ => return None,
    };
    let _ = i;
    let m = method_lookup(&sample, name)?;
    if name == "fromkeys" || name == "maketrans" {
        return Some(Value::Method(Rc::new((sample, m))));
    }
    Some(Value::Native(Rc::new(UnboundMethod { name: m })))
}

/// An unbound built-in method (`str.upper`) callable with the instance as first argument.
pub struct UnboundMethod {
    pub name: &'static str,
}
impl NativeObject for UnboundMethod {
    fn type_name(&self) -> &str {
        "method_descriptor"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn call(&self, i: &mut Interp, mut a: Vec<Value>, kw: Vec<(Name, Value)>) -> Option<R<Value>> {
        if a.is_empty() {
            return Some(i.type_error("unbound method needs an argument"));
        }
        let this = a.remove(0);
        Some(call_method(i, &this, self.name, a, kw))
    }
}

// ---------------------------------------------------------------------- method dispatch

pub fn call_method(i: &mut Interp, this: &Value, name: &str, a: Args, kw: Kw) -> R<Value> {
    match this {
        Value::Str(s) => crate::strings::str_method(i, s, name, a, kw),
        Value::List(l) => list_method(i, l, name, a, kw),
        Value::Tuple(t) => match name {
            "index" => {
                for (k, v) in t.iter().enumerate() {
                    if i.equals(v, &a[0])? {
                        return Ok(Value::Int(k as i64));
                    }
                }
                i.value_error("tuple.index(x): x not in tuple")
            }
            "count" => {
                let mut n = 0;
                for v in t.iter() {
                    if i.equals(v, &a[0])? {
                        n += 1;
                    }
                }
                Ok(Value::Int(n))
            }
            "__len__" => Ok(Value::Int(t.len() as i64)),
            "__getitem__" => i.get_item(this, &a[0]),
            _ => i.type_error(format!("tuple has no method {}", name)),
        },
        Value::Dict(d) => dict_method(i, this, d, name, a, kw),
        Value::Set(d) | Value::FrozenSet(d) => set_method(i, this, d, name, a),
        Value::Bytes(b) => match name {
            "decode" => Ok(Value::Str(String::from_utf8_lossy(b).into_owned().into())),
            "hex" => Ok(Value::Str(b.iter().map(|x| format!("{:02x}", x)).collect::<String>().into())),
            "__len__" => Ok(Value::Int(b.len() as i64)),
            _ => {
                // Delegate to the str implementation on a latin-1 view.
                let s: String = b.iter().map(|&c| c as char).collect();
                let a2: Vec<Value> = a.iter().map(|v| match v {
                    Value::Bytes(x) => Value::Str(x.iter().map(|&c| c as char).collect::<String>().into()),
                    o => o.clone(),
                }).collect();
                let r = crate::strings::str_method(i, &Rc::from(s.as_str()), name, a2, kw)?;
                Ok(to_bytes_like(r))
            }
        },
        Value::Int(n) => match name {
            "bit_length" => Ok(Value::Int(64 - n.unsigned_abs().leading_zeros() as i64)),
            "to_bytes" => {
                let len = to_int(i, a.first().unwrap_or(&Value::Int(1)))? as usize;
                let little = matches!(arg(&a, &kw, 1, "byteorder"), Some(Value::Str(s)) if &*s == "little");
                let mut b: Vec<u8> = (0..len).map(|k| ((*n as u64) >> (8 * k)) as u8).collect();
                if !little {
                    b.reverse();
                }
                Ok(Value::Bytes(Rc::new(b)))
            }
            "conjugate" | "__index__" => Ok(Value::Int(*n)),
            _ => i.type_error(format!("int has no method {}", name)),
        },
        Value::Bool(b) => call_method(i, &Value::Int(*b as i64), name, a, kw),
        Value::Float(f) => match name {
            "is_integer" => Ok(Value::Bool(f.fract() == 0.0)),
            "conjugate" => Ok(Value::Float(*f)),
            _ => i.type_error(format!("float has no method {}", name)),
        },
        Value::Native(n) => match n.call_method(i, this, name, a, kw) {
            Some(r) => r,
            None => i.type_error(format!("'{}' object has no method '{}'", n.type_name(), name)),
        },
        other => i.type_error(format!("'{}' object has no method '{}'", other.type_name(), name)),
    }
}

fn to_bytes_like(v: Value) -> Value {
    match v {
        Value::Str(s) => Value::Bytes(Rc::new(s.chars().map(|c| c as u32 as u8).collect())),
        Value::List(l) => Value::list(l.borrow().iter().cloned().map(to_bytes_like).collect()),
        other => other,
    }
}

fn list_method(i: &mut Interp, l: &Rc<RefCell<Vec<Value>>>, name: &str, a: Args, kw: Kw) -> R<Value> {
    match name {
        "append" => {
            l.borrow_mut().push(a.into_iter().next().unwrap_or(Value::None));
            Ok(Value::None)
        }
        "extend" => {
            let items = i.iterate(&a[0])?;
            l.borrow_mut().extend(items);
            Ok(Value::None)
        }
        "insert" => {
            let len = l.borrow().len() as i64;
            let mut k = to_int(i, &a[0])?;
            if k < 0 {
                k = (k + len).max(0);
            }
            let k = (k.min(len)) as usize;
            l.borrow_mut().insert(k, a[1].clone());
            Ok(Value::None)
        }
        "pop" => {
            let len = l.borrow().len();
            if len == 0 {
                return Err(i.exc("IndexError", "pop from empty list"));
            }
            let k = match a.first() {
                Some(v) => match i.index_of(len, v)? {
                    Some(k) => k,
                    None => return Err(i.exc("IndexError", "pop index out of range")),
                },
                None => len - 1,
            };
            Ok(l.borrow_mut().remove(k))
        }
        "remove" => {
            let items = l.borrow().clone();
            for (k, v) in items.iter().enumerate() {
                if i.equals(v, &a[0])? {
                    l.borrow_mut().remove(k);
                    return Ok(Value::None);
                }
            }
            i.value_error("list.remove(x): x not in list")
        }
        "index" => {
            let items = l.borrow().clone();
            let start = match a.get(1) {
                Some(v) => to_int(i, v)?.max(0) as usize,
                None => 0,
            };
            for (k, v) in items.iter().enumerate().skip(start) {
                if i.equals(v, &a[0])? {
                    return Ok(Value::Int(k as i64));
                }
            }
            let r = i.repr(&a[0])?;
            i.value_error(format!("{} is not in list", r))
        }
        "count" => {
            let items = l.borrow().clone();
            let mut n = 0;
            for v in items.iter() {
                if i.equals(v, &a[0])? {
                    n += 1;
                }
            }
            Ok(Value::Int(n))
        }
        "sort" => {
            let mut items = l.borrow().clone();
            let reverse = match kwarg(&kw, "reverse") {
                Some(v) => i.truthy(&v)?,
                None => false,
            };
            sort_values(i, &mut items, kwarg(&kw, "key"), reverse)?;
            *l.borrow_mut() = items;
            Ok(Value::None)
        }
        "reverse" => {
            l.borrow_mut().reverse();
            Ok(Value::None)
        }
        "copy" => Ok(Value::list(l.borrow().clone())),
        "clear" => {
            l.borrow_mut().clear();
            Ok(Value::None)
        }
        "__len__" => Ok(Value::Int(l.borrow().len() as i64)),
        "__getitem__" => i.get_item(&Value::List(l.clone()), &a[0]),
        "__contains__" => Ok(Value::Bool(i.contains(&Value::List(l.clone()), &a[0])?)),
        _ => i.type_error(format!("list has no method {}", name)),
    }
}

fn dict_method(i: &mut Interp, this: &Value, d: &Rc<RefCell<Dict>>, name: &str, a: Args, kw: Kw) -> R<Value> {
    match name {
        "get" => {
            let hk = i.hash(&a[0])?;
            Ok(d.borrow().get(&hk).cloned().unwrap_or_else(|| a.get(1).cloned().unwrap_or(Value::None)))
        }
        "items" => Ok(Value::list(d.borrow().items().map(|(k, v)| Value::tuple(vec![k.clone(), v.clone()])).collect())),
        "keys" => Ok(Value::list(d.borrow().keys())),
        "values" => Ok(Value::list(d.borrow().items().map(|(_, v)| v.clone()).collect())),
        "setdefault" => {
            let hk = i.hash(&a[0])?;
            if let Some(v) = d.borrow().get(&hk) {
                return Ok(v.clone());
            }
            let v = a.get(1).cloned().unwrap_or(Value::None);
            d.borrow_mut().insert(hk, a[0].clone(), v.clone());
            Ok(v)
        }
        "update" => {
            if let Some(m) = a.first() {
                let pairs = match m {
                    Value::Dict(_) => i.dict_items(m)?,
                    _ => {
                        let mut out = Vec::new();
                        for p in i.iterate(m)? {
                            let kv = i.iterate(&p)?;
                            out.push((kv[0].clone(), kv[1].clone()));
                        }
                        out
                    }
                };
                for (k, v) in pairs {
                    let hk = i.hash(&k)?;
                    d.borrow_mut().insert(hk, k, v);
                }
            }
            for (k, v) in kw {
                d.borrow_mut().insert(HashKey::Str(k.clone()), Value::Str(k), v);
            }
            Ok(Value::None)
        }
        "pop" => {
            let hk = i.hash(&a[0])?;
            let r = d.borrow_mut().remove(&hk);
            match r {
                Some(v) => Ok(v),
                None => match a.get(1) {
                    Some(dv) => Ok(dv.clone()),
                    None => {
                        let r = i.repr(&a[0])?;
                        Err(i.exc("KeyError", r))
                    }
                },
            }
        }
        "popitem" => {
            let last = d.borrow().items().last().cloned();
            match last {
                Some((k, v)) => {
                    let hk = i.hash(&k)?;
                    d.borrow_mut().remove(&hk);
                    Ok(Value::tuple(vec![k, v]))
                }
                None => Err(i.exc("KeyError", "popitem(): dictionary is empty")),
            }
        }
        "copy" => Ok(Value::dict(d.borrow().clone())),
        "clear" => {
            d.borrow_mut().clear();
            Ok(Value::None)
        }
        "fromkeys" => {
            let mut nd = Dict::default();
            let v = a.get(1).cloned().unwrap_or(Value::None);
            for k in i.iterate(&a[0])? {
                let hk = i.hash(&k)?;
                nd.insert(hk, k, v.clone());
            }
            Ok(Value::dict(nd))
        }
        "__getitem__" => i.get_item(this, &a[0]),
        "__contains__" => Ok(Value::Bool(i.contains(this, &a[0])?)),
        "__len__" => Ok(Value::Int(d.borrow().len() as i64)),
        _ => i.type_error(format!("dict has no method {}", name)),
    }
}

fn set_method(i: &mut Interp, this: &Value, d: &Rc<RefCell<Dict>>, name: &str, a: Args) -> R<Value> {
    let other_set = |i: &mut Interp, v: &Value| -> R<Dict> {
        let mut o = Dict::default();
        for x in i.iterate(v)? {
            let hk = i.hash(&x)?;
            o.insert(hk, x, Value::None);
        }
        Ok(o)
    };
    match name {
        "add" => {
            let hk = i.hash(&a[0])?;
            d.borrow_mut().insert(hk, a[0].clone(), Value::None);
            Ok(Value::None)
        }
        "remove" | "discard" => {
            let hk = i.hash(&a[0])?;
            if d.borrow_mut().remove(&hk).is_none() && name == "remove" {
                let r = i.repr(&a[0])?;
                return Err(i.exc("KeyError", r));
            }
            Ok(Value::None)
        }
        "pop" => {
            let first = d.borrow().items().next().cloned();
            match first {
                Some((k, _)) => {
                    let hk = i.hash(&k)?;
                    d.borrow_mut().remove(&hk);
                    Ok(k)
                }
                None => Err(i.exc("KeyError", "pop from an empty set")),
            }
        }
        "update" => {
            for v in &a {
                for x in i.iterate(v)? {
                    let hk = i.hash(&x)?;
                    d.borrow_mut().insert(hk, x, Value::None);
                }
            }
            Ok(Value::None)
        }
        "union" | "intersection" | "difference" | "symmetric_difference" => {
            let mut acc = this.clone();
            for v in &a {
                let o = Value::Set(Rc::new(RefCell::new(other_set(i, v)?)));
                let op = match name {
                    "union" => crate::ast::BinOp::BitOr,
                    "intersection" => crate::ast::BinOp::BitAnd,
                    "difference" => crate::ast::BinOp::Sub,
                    _ => crate::ast::BinOp::BitXor,
                };
                let acc_set = match &acc {
                    Value::FrozenSet(x) => Value::Set(x.clone()),
                    o => o.clone(),
                };
                acc = i.binop(op, &acc_set, &o)?;
            }
            Ok(acc)
        }
        "issubset" => {
            let o = other_set(i, &a[0])?;
            Ok(Value::Bool(d.borrow().items().all(|(k, _)| o.get(&hash_key(k).unwrap()).is_some())))
        }
        "issuperset" => {
            let o = other_set(i, &a[0])?;
            let r = o.items().all(|(k, _)| d.borrow().get(&hash_key(k).unwrap()).is_some());
            Ok(Value::Bool(r))
        }
        "isdisjoint" => {
            let o = other_set(i, &a[0])?;
            let r = !o.items().any(|(k, _)| d.borrow().get(&hash_key(k).unwrap()).is_some());
            Ok(Value::Bool(r))
        }
        "copy" => Ok(Value::Set(Rc::new(RefCell::new(d.borrow().clone())))),
        "clear" => {
            d.borrow_mut().clear();
            Ok(Value::None)
        }
        "__contains__" => Ok(Value::Bool(i.contains(this, &a[0])?)),
        "__len__" => Ok(Value::Int(d.borrow().len() as i64)),
        _ => i.type_error(format!("set has no method {}", name)),
    }
}

/// `str_slice` re-export for other modules.
pub fn slice_chars(s: &str, a: usize, b: usize) -> String {
    str_slice(s, a, b)
}
