//! Operators, comparisons, conversions, indexing and iteration.

use crate::ast::{BinOp, CmpOp, UnOp};
use crate::interp::{Flow, Interp, R};
use crate::value::*;
use std::cell::RefCell;
use std::rc::Rc;

pub fn chars_len(s: &str) -> usize {
    if s.is_ascii() {
        s.len()
    } else {
        s.chars().count()
    }
}

/// Character slice of a string (start..end in characters).
pub fn str_slice(s: &str, start: usize, end: usize) -> String {
    if s.is_ascii() {
        s[start.min(s.len())..end.min(s.len()).max(start.min(s.len()))].to_string()
    } else {
        s.chars().skip(start).take(end.saturating_sub(start)).collect()
    }
}

/// Resolve slice bounds like `PySlice_AdjustIndices`.
pub fn slice_indices(len: i64, start: &Value, stop: &Value, step: &Value) -> Option<(i64, i64, i64)> {
    let as_int = |v: &Value| -> Option<Option<i64>> {
        match v {
            Value::None => Some(None),
            Value::Int(i) => Some(Some(*i)),
            Value::Bool(b) => Some(Some(*b as i64)),
            _ => None,
        }
    };
    let step = as_int(step)?.unwrap_or(1);
    if step == 0 {
        return None;
    }
    let (lower, upper) = if step < 0 { (-1, len - 1) } else { (0, len) };
    let adj = |v: Option<i64>, default: i64| -> i64 {
        match v {
            None => default,
            Some(mut x) => {
                if x < 0 {
                    x += len;
                    if x < lower {
                        x = lower;
                    }
                } else if x > upper {
                    x = upper;
                }
                x
            }
        }
    };
    let start = adj(as_int(start)?, if step < 0 { upper } else { lower });
    let stop = adj(as_int(stop)?, if step < 0 { lower } else { upper });
    Some((start, stop, step))
}

fn slice_positions(start: i64, stop: i64, step: i64) -> Vec<usize> {
    let mut out = Vec::new();
    let mut i = start;
    while (step > 0 && i < stop) || (step < 0 && i > stop) {
        out.push(i as usize);
        i += step;
    }
    out
}

impl Interp {
    pub fn truthy(&mut self, v: &Value) -> R<bool> {
        Ok(match v {
            Value::None => false,
            Value::Bool(b) => *b,
            Value::Int(i) => *i != 0,
            Value::Float(f) => *f != 0.0,
            Value::Str(s) => !s.is_empty(),
            Value::Bytes(b) => !b.is_empty(),
            Value::List(l) => !l.borrow().is_empty(),
            Value::Tuple(t) => !t.is_empty(),
            Value::Dict(d) | Value::Set(d) | Value::FrozenSet(d) => !d.borrow().is_empty(),
            Value::Range(a, b, c) => range_len(*a, *b, *c) > 0,
            Value::Instance(i) => {
                if let Some(m) = i.class.lookup("__bool__") {
                    let r = self.call(&m, vec![v.clone()], Vec::new())?;
                    return self.truthy(&r);
                }
                if let Some(m) = i.class.lookup("__len__") {
                    let r = self.call(&m, vec![v.clone()], Vec::new())?;
                    return Ok(!matches!(r, Value::Int(0)));
                }
                let b = i.base.borrow().clone();
                if !b.is_none() && !i.class.is_enum && i.class.builtin != "Exception" {
                    return self.truthy(&b);
                }
                true
            }
            Value::Native(n) => n.truthy(),
            _ => true,
        })
    }

    pub fn hash(&mut self, v: &Value) -> R<HashKey> {
        match hash_key(v) {
            Some(k) => Ok(k),
            None => self.type_error(format!("unhashable type: '{}'", v.type_name())),
        }
    }

    pub fn to_str(&mut self, v: &Value) -> R<String> {
        Ok(match v {
            Value::Str(s) => s.to_string(),
            Value::Float(f) => float_repr(*f),
            Value::Int(i) => i.to_string(),
            Value::Bool(b) => if *b { "True" } else { "False" }.into(),
            Value::None => "None".into(),
            Value::Instance(i) => {
                if let Some(m) = i.class.lookup("__str__") {
                    let r = self.call(&m, vec![v.clone()], Vec::new())?;
                    return self.to_str(&r);
                }
                if i.class.builtin == "Exception" {
                    let args = i.base.borrow().clone();
                    if let Value::Tuple(t) = &args {
                        return match t.len() {
                            0 => Ok(String::new()),
                            1 => self.to_str(&t[0]),
                            _ => self.repr(&args),
                        };
                    }
                }
                if i.class.is_enum {
                    let name = i.dict.borrow().get("_name_").cloned().unwrap_or(Value::None);
                    return Ok(format!("{}.{}", i.class.name, self.to_str(&name)?));
                }
                let b = i.base.borrow().clone();
                if !b.is_none() {
                    return self.to_str(&b);
                }
                return self.repr(v);
            }
            _ => return self.repr(v),
        })
    }

    pub fn repr(&mut self, v: &Value) -> R<String> {
        Ok(match v {
            Value::None => "None".into(),
            Value::NotImplemented => "NotImplemented".into(),
            Value::Ellipsis => "Ellipsis".into(),
            Value::Bool(b) => if *b { "True" } else { "False" }.into(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => float_repr(*f),
            Value::Str(s) => str_repr(s),
            Value::Bytes(b) => {
                let mut s = String::from("b'");
                for &c in b.iter() {
                    match c {
                        b'\\' => s.push_str("\\\\"),
                        b'\'' => s.push_str("\\'"),
                        b'\n' => s.push_str("\\n"),
                        b'\r' => s.push_str("\\r"),
                        b'\t' => s.push_str("\\t"),
                        32..=126 => s.push(c as char),
                        _ => s.push_str(&format!("\\x{:02x}", c)),
                    }
                }
                s.push('\'');
                s
            }
            Value::List(l) => {
                let items = l.borrow().clone();
                let parts: R<Vec<String>> = items.iter().map(|x| self.repr(x)).collect();
                format!("[{}]", parts?.join(", "))
            }
            Value::Tuple(t) => {
                let parts: R<Vec<String>> = t.iter().map(|x| self.repr(x)).collect();
                let parts = parts?;
                if parts.len() == 1 {
                    format!("({},)", parts[0])
                } else {
                    format!("({})", parts.join(", "))
                }
            }
            Value::Dict(d) => {
                let items: Vec<(Value, Value)> = d.borrow().items().cloned().collect();
                let mut parts = Vec::new();
                for (k, v) in items {
                    parts.push(format!("{}: {}", self.repr(&k)?, self.repr(&v)?));
                }
                format!("{{{}}}", parts.join(", "))
            }
            Value::Set(d) | Value::FrozenSet(d) => {
                let keys = d.borrow().keys();
                if keys.is_empty() {
                    return Ok("set()".into());
                }
                let parts: R<Vec<String>> = keys.iter().map(|x| self.repr(x)).collect();
                format!("{{{}}}", parts?.join(", "))
            }
            Value::Range(a, b, c) => {
                if *c == 1 {
                    format!("range({}, {})", a, b)
                } else {
                    format!("range({}, {}, {})", a, b, c)
                }
            }
            Value::Instance(i) => {
                if let Some(m) = i.class.lookup("__repr__") {
                    let r = self.call(&m, vec![v.clone()], Vec::new())?;
                    return self.to_str(&r);
                }
                if i.class.builtin == "Exception" {
                    let args = i.base.borrow().clone();
                    let inner = match &args {
                        Value::Tuple(t) if t.len() == 1 => self.repr(&t[0])?,
                        Value::Tuple(t) => {
                            let parts: R<Vec<String>> = t.iter().map(|x| self.repr(x)).collect();
                            parts?.join(", ")
                        }
                        _ => String::new(),
                    };
                    return Ok(format!("{}({})", i.class.name, inner));
                }
                if i.class.is_enum {
                    let name = i.dict.borrow().get("_name_").cloned().unwrap_or(Value::None);
                    let val = i.dict.borrow().get("_value_").cloned().unwrap_or(Value::None);
                    return Ok(format!("<{}.{}: {}>", i.class.name, self.to_str(&name)?, self.repr(&val)?));
                }
                let b = i.base.borrow().clone();
                if !b.is_none() {
                    return self.repr(&b);
                }
                format!("<{} object>", i.class.name)
            }
            Value::Class(c) => format!("<class '{}'>", c.name),
            Value::Func(f) => format!("<function {}>", f.qualname),
            Value::Builtin(b) => format!("<built-in function {}>", b.name),
            Value::Module(m) => format!("<module '{}'>", m.name),
            Value::Native(n) => n.repr(),
            other => format!("<{}>", other.type_name()),
        })
    }

    // ------------------------------------------------------------------ numbers

    fn num(v: &Value) -> Option<Num> {
        match v {
            Value::Int(i) => Some(Num::I(*i)),
            Value::Bool(b) => Some(Num::I(*b as i64)),
            Value::Float(f) => Some(Num::F(*f)),
            Value::Instance(i) if !i.class.is_enum => {
                let b = i.base.borrow();
                match &*b {
                    Value::Int(x) => Some(Num::I(*x)),
                    Value::Float(x) => Some(Num::F(*x)),
                    Value::Bool(x) => Some(Num::I(*x as i64)),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    pub fn inplace_op(&mut self, op: BinOp, a: Value, b: Value) -> R<Value> {
        // Lists extend in place.
        if op == BinOp::Add {
            if let Value::List(l) = &a {
                let items = self.iterate(&b)?;
                l.borrow_mut().extend(items);
                return Ok(a);
            }
        }
        if let Value::Instance(i) = &a {
            let name = format!("__i{}__", binop_name(op));
            if let Some(m) = i.class.lookup(&name) {
                return self.call(&m, vec![a.clone(), b], Vec::new());
            }
        }
        self.binop(op, &a, &b)
    }

    pub fn binop(&mut self, op: BinOp, a: &Value, b: &Value) -> R<Value> {
        if let (Some(x), Some(y)) = (Self::num(a), Self::num(b)) {
            return self.num_op(op, x, y);
        }
        // User-defined operators.
        if let Value::Instance(i) = a {
            if let Some(m) = i.class.lookup(&format!("__{}__", binop_name(op))) {
                let r = self.call(&m, vec![a.clone(), b.clone()], Vec::new())?;
                if !matches!(r, Value::NotImplemented) {
                    return Ok(r);
                }
            }
        }
        if let Value::Instance(i) = b {
            if let Some(m) = i.class.lookup(&format!("__r{}__", binop_name(op))) {
                let r = self.call(&m, vec![b.clone(), a.clone()], Vec::new())?;
                if !matches!(r, Value::NotImplemented) {
                    return Ok(r);
                }
            }
        }
        if let Value::Native(n) = a {
            if let Some(r) = n.binop(self, op, b, false) {
                return r;
            }
        }
        if let Value::Native(n) = b {
            if let Some(r) = n.binop(self, op, a, true) {
                return r;
            }
        }
        let (ua, ub) = (a.unwrap_base(), b.unwrap_base());
        match (op, &ua, &ub) {
            (BinOp::Add, Value::Str(x), Value::Str(y)) => {
                let mut s = String::with_capacity(x.len() + y.len());
                s.push_str(x);
                s.push_str(y);
                Ok(Value::Str(s.into()))
            }
            (BinOp::Add, Value::List(x), Value::List(y)) => {
                let mut v = x.borrow().clone();
                v.extend(y.borrow().iter().cloned());
                Ok(Value::list(v))
            }
            (BinOp::Add, Value::Tuple(x), Value::Tuple(y)) => {
                let mut v = (**x).clone();
                v.extend(y.iter().cloned());
                Ok(Value::tuple(v))
            }
            (BinOp::Add, Value::Bytes(x), Value::Bytes(y)) => {
                let mut v = (**x).clone();
                v.extend(y.iter());
                Ok(Value::Bytes(Rc::new(v)))
            }
            (BinOp::Mul, Value::Str(s), n) | (BinOp::Mul, n, Value::Str(s)) if Self::num(n).is_some() => {
                let k = match Self::num(n) {
                    Some(Num::I(k)) => k,
                    _ => return self.type_error("can't multiply sequence by non-int of type 'float'"),
                };
                Ok(Value::Str(s.repeat(k.max(0) as usize).into()))
            }
            (BinOp::Mul, Value::List(l), n) | (BinOp::Mul, n, Value::List(l)) if matches!(Self::num(n), Some(Num::I(_))) => {
                let Some(Num::I(k)) = Self::num(n) else { unreachable!() };
                let items = l.borrow().clone();
                let mut v = Vec::with_capacity(items.len() * k.max(0) as usize);
                for _ in 0..k.max(0) {
                    v.extend(items.iter().cloned());
                }
                Ok(Value::list(v))
            }
            (BinOp::Mul, Value::Tuple(t), n) | (BinOp::Mul, n, Value::Tuple(t)) if matches!(Self::num(n), Some(Num::I(_))) => {
                let Some(Num::I(k)) = Self::num(n) else { unreachable!() };
                let mut v = Vec::new();
                for _ in 0..k.max(0) {
                    v.extend(t.iter().cloned());
                }
                Ok(Value::tuple(v))
            }
            (BinOp::Mod, Value::Str(fmt), args) => {
                let r = crate::format::percent_format(self, fmt, args)?;
                Ok(Value::Str(r.into()))
            }
            (BinOp::BitOr, Value::Dict(x), Value::Dict(y)) => {
                let mut d = x.borrow().clone();
                for (k, v) in y.borrow().items() {
                    let hk = self.hash(k)?;
                    d.insert(hk, k.clone(), v.clone());
                }
                Ok(Value::dict(d))
            }
            (BinOp::BitOr | BinOp::BitAnd | BinOp::Sub | BinOp::BitXor, Value::Set(x), Value::Set(y)) => {
                let (x, y) = (x.borrow().clone(), y.borrow().clone());
                let mut d = Dict::default();
                match op {
                    BinOp::BitOr => {
                        d = x.clone();
                        for (k, _) in y.items() {
                            d.insert(hash_key(k).unwrap(), k.clone(), Value::None);
                        }
                    }
                    BinOp::BitAnd => {
                        for (k, _) in x.items() {
                            if y.get(&hash_key(k).unwrap()).is_some() {
                                d.insert(hash_key(k).unwrap(), k.clone(), Value::None);
                            }
                        }
                    }
                    BinOp::Sub => {
                        for (k, _) in x.items() {
                            if y.get(&hash_key(k).unwrap()).is_none() {
                                d.insert(hash_key(k).unwrap(), k.clone(), Value::None);
                            }
                        }
                    }
                    _ => {
                        for (k, _) in x.items() {
                            if y.get(&hash_key(k).unwrap()).is_none() {
                                d.insert(hash_key(k).unwrap(), k.clone(), Value::None);
                            }
                        }
                        for (k, _) in y.items() {
                            if x.get(&hash_key(k).unwrap()).is_none() {
                                d.insert(hash_key(k).unwrap(), k.clone(), Value::None);
                            }
                        }
                    }
                }
                Ok(Value::Set(Rc::new(RefCell::new(d))))
            }
            _ => self.type_error(format!("unsupported operand type(s) for {}: '{}' and '{}'", binop_symbol(op), a.type_name(), b.type_name())),
        }
    }

    fn num_op(&mut self, op: BinOp, x: Num, y: Num) -> R<Value> {
        use Num::*;
        Ok(match (x, y) {
            (I(a), I(b)) => match op {
                BinOp::Add => a.checked_add(b).map(Value::Int).unwrap_or(Value::Float(a as f64 + b as f64)),
                BinOp::Sub => a.checked_sub(b).map(Value::Int).unwrap_or(Value::Float(a as f64 - b as f64)),
                BinOp::Mul => a.checked_mul(b).map(Value::Int).unwrap_or(Value::Float(a as f64 * b as f64)),
                BinOp::Div => {
                    if b == 0 {
                        return Err(self.exc("ZeroDivisionError", "division by zero"));
                    }
                    Value::Float(a as f64 / b as f64)
                }
                BinOp::FloorDiv => {
                    if b == 0 {
                        return Err(self.exc("ZeroDivisionError", "integer division or modulo by zero"));
                    }
                    Value::Int(a.div_euclid(b) - if (a.rem_euclid(b) != 0) && b < 0 { 1 } else { 0 })
                }
                BinOp::Mod => {
                    if b == 0 {
                        return Err(self.exc("ZeroDivisionError", "integer division or modulo by zero"));
                    }
                    let m = a % b;
                    Value::Int(if m != 0 && ((m < 0) != (b < 0)) { m + b } else { m })
                }
                BinOp::Pow => {
                    if b >= 0 {
                        match a.checked_pow(b.min(u32::MAX as i64) as u32) {
                            Some(v) => Value::Int(v),
                            None => Value::Float((a as f64).powf(b as f64)),
                        }
                    } else {
                        Value::Float((a as f64).powf(b as f64))
                    }
                }
                BinOp::LShift => Value::Int(a.checked_shl(b as u32).unwrap_or(0)),
                BinOp::RShift => Value::Int(a >> b.min(63)),
                BinOp::BitOr => Value::Int(a | b),
                BinOp::BitAnd => Value::Int(a & b),
                BinOp::BitXor => Value::Int(a ^ b),
                BinOp::MatMul => return self.type_error("unsupported operand type(s) for @"),
            },
            (a, b) => {
                let (a, b) = (a.f(), b.f());
                match op {
                    BinOp::Add => Value::Float(a + b),
                    BinOp::Sub => Value::Float(a - b),
                    BinOp::Mul => Value::Float(a * b),
                    BinOp::Div => {
                        if b == 0.0 {
                            return Err(self.exc("ZeroDivisionError", "float division by zero"));
                        }
                        Value::Float(a / b)
                    }
                    BinOp::FloorDiv => {
                        if b == 0.0 {
                            return Err(self.exc("ZeroDivisionError", "float floor division by zero"));
                        }
                        // CPython `_float_div_mod`.
                        let mut m = a % b;
                        let mut div = (a - m) / b;
                        if m != 0.0 {
                            if (b < 0.0) != (m < 0.0) {
                                m += b;
                                div -= 1.0;
                            }
                        }
                        let _ = m;
                        let fd = if div != 0.0 {
                            let mut fl = div.floor();
                            if div - fl > 0.5 {
                                fl += 1.0;
                            }
                            fl
                        } else {
                            0.0f64.copysign(a / b)
                        };
                        Value::Float(fd)
                    }
                    BinOp::Mod => {
                        if b == 0.0 {
                            return Err(self.exc("ZeroDivisionError", "float modulo"));
                        }
                        let m = a % b;
                        Value::Float(if m != 0.0 {
                            if (b < 0.0) != (m < 0.0) {
                                m + b
                            } else {
                                m
                            }
                        } else {
                            0.0f64.copysign(b)
                        })
                    }
                    BinOp::Pow => {
                        if a < 0.0 && b.fract() != 0.0 {
                            return self.value_error("complex results are not supported");
                        }
                        if a == 0.0 && b < 0.0 {
                            return Err(self.exc("ZeroDivisionError", "0.0 cannot be raised to a negative power"));
                        }
                        Value::Float(a.powf(b))
                    }
                    _ => return self.type_error(format!("unsupported operand type(s) for {}: 'float'", binop_symbol(op))),
                }
            }
        })
    }

    pub fn unary(&mut self, op: UnOp, v: &Value) -> R<Value> {
        if op == UnOp::Not {
            return Ok(Value::Bool(!self.truthy(v)?));
        }
        if let Value::Native(n) = v {
            if op == UnOp::Neg {
                if let Some(r) = n.binop(self, BinOp::Mul, &Value::Int(-1), true) {
                    return r;
                }
            }
        }
        match (op, Self::num(v)) {
            (UnOp::Neg, Some(Num::I(i))) => Ok(Value::Int(-i)),
            (UnOp::Neg, Some(Num::F(f))) => Ok(Value::Float(-f)),
            (UnOp::Pos, Some(Num::I(i))) => Ok(Value::Int(i)),
            (UnOp::Pos, Some(Num::F(f))) => Ok(Value::Float(f)),
            (UnOp::Invert, Some(Num::I(i))) => Ok(Value::Int(!i)),
            _ => self.type_error(format!("bad operand type for unary operator: '{}'", v.type_name())),
        }
    }

    // ------------------------------------------------------------------ comparison

    pub fn equals(&mut self, a: &Value, b: &Value) -> R<bool> {
        if let (Some(x), Some(y)) = (Self::num(a), Self::num(b)) {
            return Ok(match (x, y) {
                (Num::I(p), Num::I(q)) => p == q,
                (p, q) => p.f() == q.f(),
            });
        }
        match (a, b) {
            (Value::Str(x), Value::Str(y)) => return Ok(x == y),
            (Value::None, Value::None) => return Ok(true),
            (Value::None, _) | (_, Value::None) => return Ok(false),
            _ => {}
        }
        if let Value::Instance(i) = a {
            if let Some(m) = i.class.lookup("__eq__") {
                let r = self.call(&m, vec![a.clone(), b.clone()], Vec::new())?;
                if !matches!(r, Value::NotImplemented) {
                    return self.truthy(&r);
                }
            }
            if i.class.is_enum {
                if let Value::Instance(j) = b {
                    return Ok(Rc::ptr_eq(i, j));
                }
                // Mixed-in enums (str, Enum) compare equal to their value.
                if i.class.builtin != "Enum" {
                    let v = i.base.borrow().clone();
                    return self.equals(&v, b);
                }
                return Ok(false);
            }
        }
        if let Value::Instance(j) = b {
            if j.class.lookup("__eq__").is_some() || (j.class.is_enum && j.class.builtin != "Enum") {
                return self.equals(b, a);
            }
        }
        if let Value::Native(n) = a {
            if let Some(r) = n.equals(b) {
                return Ok(r);
            }
        }
        let (ua, ub) = (a.unwrap_base(), b.unwrap_base());
        Ok(match (&ua, &ub) {
            (Value::Str(x), Value::Str(y)) => x == y,
            (Value::Bytes(x), Value::Bytes(y)) => x == y,
            (Value::List(x), Value::List(y)) => {
                if Rc::ptr_eq(x, y) {
                    return Ok(true);
                }
                let (x, y) = (x.borrow().clone(), y.borrow().clone());
                self.seq_eq(&x, &y)?
            }
            (Value::Tuple(x), Value::Tuple(y)) => self.seq_eq(x, y)?,
            (Value::Dict(x), Value::Dict(y)) => {
                let (x, y) = (x.borrow().clone(), y.borrow().clone());
                if x.len() != y.len() {
                    return Ok(false);
                }
                for (k, v) in x.items() {
                    match y.get(&hash_key(k).unwrap()) {
                        Some(w) => {
                            if !self.equals(v, w)? {
                                return Ok(false);
                            }
                        }
                        None => return Ok(false),
                    }
                }
                true
            }
            (Value::Set(x) | Value::FrozenSet(x), Value::Set(y) | Value::FrozenSet(y)) => {
                let (x, y) = (x.borrow(), y.borrow());
                x.len() == y.len() && x.items().all(|(k, _)| y.get(&hash_key(k).unwrap()).is_some())
            }
            (Value::Range(a1, b1, c1), Value::Range(a2, b2, c2)) => (a1, b1, c1) == (a2, b2, c2),
            (Value::Class(x), Value::Class(y)) => Rc::ptr_eq(x, y),
            (Value::Instance(x), Value::Instance(y)) => Rc::ptr_eq(x, y),
            (Value::Func(x), Value::Func(y)) => Rc::ptr_eq(x, y),
            (Value::Module(x), Value::Module(y)) => Rc::ptr_eq(x, y),
            (Value::Native(x), Value::Native(y)) => Rc::ptr_eq(x, y),
            (Value::Builtin(x), Value::Builtin(y)) => Rc::ptr_eq(x, y),
            _ => false,
        })
    }

    fn seq_eq(&mut self, x: &[Value], y: &[Value]) -> R<bool> {
        if x.len() != y.len() {
            return Ok(false);
        }
        for (p, q) in x.iter().zip(y.iter()) {
            if !self.equals(p, q)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// `a < b` (and the other orderings via it).
    pub fn less(&mut self, a: &Value, b: &Value) -> R<bool> {
        if let (Some(x), Some(y)) = (Self::num(a), Self::num(b)) {
            return Ok(match (x, y) {
                (Num::I(p), Num::I(q)) => p < q,
                (p, q) => p.f() < q.f(),
            });
        }
        if let Value::Instance(i) = a {
            if let Some(m) = i.class.lookup("__lt__") {
                let r = self.call(&m, vec![a.clone(), b.clone()], Vec::new())?;
                return self.truthy(&r);
            }
        }
        let (ua, ub) = (a.unwrap_base(), b.unwrap_base());
        match (&ua, &ub) {
            (Value::Str(x), Value::Str(y)) => Ok(x < y),
            (Value::Bytes(x), Value::Bytes(y)) => Ok(x < y),
            (Value::List(_) | Value::Tuple(_), Value::List(_) | Value::Tuple(_)) => {
                let x = self.iterate(&ua)?;
                let y = self.iterate(&ub)?;
                for (p, q) in x.iter().zip(y.iter()) {
                    if !self.equals(p, q)? {
                        return self.less(p, q);
                    }
                }
                Ok(x.len() < y.len())
            }
            (Value::Set(x), Value::Set(y)) => {
                let (x, y) = (x.borrow(), y.borrow());
                Ok(x.len() < y.len() && x.items().all(|(k, _)| y.get(&hash_key(k).unwrap()).is_some()))
            }
            _ => self.type_error(format!("'<' not supported between instances of '{}' and '{}'", a.type_name(), b.type_name())),
        }
    }

    pub fn compare(&mut self, op: CmpOp, a: &Value, b: &Value) -> R<bool> {
        match op {
            CmpOp::Eq => self.equals(a, b),
            CmpOp::NotEq => Ok(!self.equals(a, b)?),
            CmpOp::Lt => self.less(a, b),
            CmpOp::Gt => self.less(b, a),
            CmpOp::LtE => Ok(self.less(a, b)? || self.equals(a, b)?),
            CmpOp::GtE => Ok(self.less(b, a)? || self.equals(a, b)?),
            CmpOp::Is => Ok(identical(a, b)),
            CmpOp::IsNot => Ok(!identical(a, b)),
            CmpOp::In => self.contains(b, a),
            CmpOp::NotIn => Ok(!self.contains(b, a)?),
        }
    }

    pub fn contains(&mut self, container: &Value, item: &Value) -> R<bool> {
        let c = container.unwrap_base();
        match &c {
            Value::Str(s) => {
                let i = item.unwrap_base();
                match &i {
                    Value::Str(x) => Ok(s.contains(&**x)),
                    _ => self.type_error("'in <string>' requires string as left operand"),
                }
            }
            Value::Bytes(b) => match item {
                Value::Bytes(x) => Ok(x.is_empty() || b.windows(x.len()).any(|w| w == &x[..])),
                Value::Int(i) => Ok(b.contains(&(*i as u8))),
                _ => Ok(false),
            },
            Value::Dict(d) | Value::Set(d) | Value::FrozenSet(d) => {
                let k = self.hash(item)?;
                Ok(d.borrow().get(&k).is_some())
            }
            Value::Range(a, b, st) => match Self::num(item) {
                Some(Num::I(i)) => Ok(if *st > 0 { i >= *a && i < *b } else { i <= *a && i > *b } && (i - a) % st == 0),
                _ => Ok(false),
            },
            Value::Instance(inst) if inst.class.lookup("__contains__").is_some() => {
                let m = inst.class.lookup("__contains__").unwrap();
                let r = self.call(&m, vec![container.clone(), item.clone()], Vec::new())?;
                self.truthy(&r)
            }
            Value::Class(cls) if cls.is_enum => {
                let members = cls.members.borrow().clone();
                for m in members {
                    if self.equals(&m, item)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            _ => {
                let items = self.iterate(&c)?;
                for x in items {
                    if self.equals(&x, item)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        }
    }

    // ------------------------------------------------------------------ iteration

    /// All items of an iterable.
    pub fn iterate(&mut self, v: &Value) -> R<Vec<Value>> {
        Ok(match v {
            Value::List(l) => l.borrow().clone(),
            Value::Tuple(t) => (**t).clone(),
            Value::Str(s) => s.chars().map(|c| Value::Str(c.to_string().into())).collect(),
            Value::Bytes(b) => b.iter().map(|x| Value::Int(*x as i64)).collect(),
            Value::Dict(d) | Value::Set(d) | Value::FrozenSet(d) => d.borrow().keys(),
            Value::Range(a, b, c) => {
                let mut out = Vec::new();
                let mut i = *a;
                while (*c > 0 && i < *b) || (*c < 0 && i > *b) {
                    out.push(Value::Int(i));
                    i += c;
                }
                out
            }
            Value::Iter(it) => {
                let mut it = it.borrow_mut();
                let rest = match &mut *it {
                    PyIter::Seq(v, i) => v[*i..].to_vec(),
                    PyIter::Range(a, b, c) => {
                        let mut out = Vec::new();
                        let mut i = *a;
                        while (*c > 0 && i < *b) || (*c < 0 && i > *b) {
                            out.push(Value::Int(i));
                            i += *c;
                        }
                        out
                    }
                    PyIter::Chars(s, i) => s.chars().skip(*i).map(|c| Value::Str(c.to_string().into())).collect(),
                    PyIter::Done => Vec::new(),
                };
                *it = PyIter::Done;
                rest
            }
            Value::Class(c) if c.is_enum => c.members.borrow().clone(),
            Value::Instance(i) => {
                if let Some(m) = i.class.lookup("__iter__") {
                    let it = self.call(&m, vec![v.clone()], Vec::new())?;
                    if let Value::Instance(ii) = &it {
                        if let Some(next) = ii.class.lookup("__next__") {
                            let mut out = Vec::new();
                            loop {
                                match self.call(&next, vec![it.clone()], Vec::new()) {
                                    Ok(x) => out.push(x),
                                    Err(Flow::Exc(e)) if self.exc_type_name(&e) == "StopIteration" => break,
                                    Err(e) => return Err(e),
                                }
                            }
                            return Ok(out);
                        }
                    }
                    return self.iterate(&it);
                }
                let b = i.base.borrow().clone();
                if !b.is_none() {
                    return self.iterate(&b);
                }
                return self.type_error(format!("'{}' object is not iterable", i.class.name));
            }
            Value::Native(n) => match n.iter(self) {
                Some(r) => r?,
                None => return self.type_error(format!("'{}' object is not iterable", n.type_name())),
            },
            other => return self.type_error(format!("'{}' object is not iterable", other.type_name())),
        })
    }

    /// (key, value) pairs of a mapping.
    pub fn dict_items(&mut self, m: &Value) -> R<Vec<(Value, Value)>> {
        match m {
            Value::Dict(d) => Ok(d.borrow().items().cloned().collect()),
            _ => {
                let keys = self.call_method_by_name(m, "keys", Vec::new())?;
                let keys = self.iterate(&keys)?;
                let mut out = Vec::new();
                for k in keys {
                    let v = self.get_item(m, &k)?;
                    out.push((k, v));
                }
                Ok(out)
            }
        }
    }

    pub fn call_method_by_name(&mut self, obj: &Value, name: &str, args: Vec<Value>) -> R<Value> {
        let m = self.get_attr(obj, name)?;
        self.call(&m, args, Vec::new())
    }

    // ------------------------------------------------------------------ items

    pub fn index_of(&mut self, len: usize, k: &Value) -> R<Option<usize>> {
        let i = match Self::num(k) {
            Some(Num::I(i)) => i,
            _ => return self.type_error(format!("indices must be integers, not {}", k.type_name())),
        };
        let j = if i < 0 { i + len as i64 } else { i };
        Ok(if j < 0 || j >= len as i64 { None } else { Some(j as usize) })
    }

    pub fn get_item(&mut self, o: &Value, k: &Value) -> R<Value> {
        match o {
            Value::List(l) => {
                if let Value::Slice(s) = k {
                    let l = l.borrow();
                    let (a, b, c) = slice_indices(l.len() as i64, &s.0, &s.1, &s.2).ok_or_else(|| self.exc("ValueError", "slice step cannot be zero"))?;
                    return Ok(Value::list(slice_positions(a, b, c).into_iter().map(|i| l[i].clone()).collect()));
                }
                let len = l.borrow().len();
                match self.index_of(len, k)? {
                    Some(i) => Ok(l.borrow()[i].clone()),
                    None => Err(self.exc("IndexError", "list index out of range")),
                }
            }
            Value::Tuple(t) => {
                if let Value::Slice(s) = k {
                    let (a, b, c) = slice_indices(t.len() as i64, &s.0, &s.1, &s.2).ok_or_else(|| self.exc("ValueError", "slice step cannot be zero"))?;
                    return Ok(Value::tuple(slice_positions(a, b, c).into_iter().map(|i| t[i].clone()).collect()));
                }
                match self.index_of(t.len(), k)? {
                    Some(i) => Ok(t[i].clone()),
                    None => Err(self.exc("IndexError", "tuple index out of range")),
                }
            }
            Value::Str(s) => {
                let len = chars_len(s);
                if let Value::Slice(sl) = k {
                    let (a, b, c) = slice_indices(len as i64, &sl.0, &sl.1, &sl.2).ok_or_else(|| self.exc("ValueError", "slice step cannot be zero"))?;
                    if c == 1 {
                        return Ok(Value::Str(str_slice(s, a as usize, b.max(a) as usize).into()));
                    }
                    let chars: Vec<char> = s.chars().collect();
                    return Ok(Value::Str(slice_positions(a, b, c).into_iter().map(|i| chars[i]).collect::<String>().into()));
                }
                match self.index_of(len, k)? {
                    Some(i) => Ok(Value::Str(if s.is_ascii() { s[i..i + 1].into() } else { s.chars().nth(i).unwrap().to_string().into() })),
                    None => Err(self.exc("IndexError", "string index out of range")),
                }
            }
            Value::Bytes(b) => {
                if let Value::Slice(sl) = k {
                    let (a, e, c) = slice_indices(b.len() as i64, &sl.0, &sl.1, &sl.2).ok_or_else(|| self.exc("ValueError", "slice step cannot be zero"))?;
                    return Ok(Value::Bytes(Rc::new(slice_positions(a, e, c).into_iter().map(|i| b[i]).collect())));
                }
                match self.index_of(b.len(), k)? {
                    Some(i) => Ok(Value::Int(b[i] as i64)),
                    None => Err(self.exc("IndexError", "index out of range")),
                }
            }
            Value::Range(a, b, c) => {
                let len = range_len(*a, *b, *c);
                match self.index_of(len as usize, k)? {
                    Some(i) => Ok(Value::Int(a + i as i64 * c)),
                    None => Err(self.exc("IndexError", "range object index out of range")),
                }
            }
            Value::Dict(d) => {
                let hk = self.hash(k)?;
                let v = d.borrow().get(&hk).cloned();
                match v {
                    Some(v) => Ok(v),
                    None => {
                        let r = self.repr(k)?;
                        Err(self.exc("KeyError", r))
                    }
                }
            }
            Value::Class(c) if c.is_enum => {
                // Color["RED"]
                let name = self.to_str(k)?;
                for m in c.members.borrow().iter() {
                    if let Value::Instance(i) = m {
                        if matches!(i.dict.borrow().get("_name_"), Some(Value::Str(n)) if **n == name) {
                            return Ok(m.clone());
                        }
                    }
                }
                Err(self.exc("KeyError", name))
            }
            Value::Instance(i) => {
                if let Some(m) = i.class.lookup("__getitem__") {
                    return self.call(&m, vec![o.clone(), k.clone()], Vec::new());
                }
                let b = i.base.borrow().clone();
                if !b.is_none() {
                    return self.get_item(&b, k);
                }
                self.type_error(format!("'{}' object is not subscriptable", i.class.name))
            }
            Value::Native(n) => match n.get_item(self, k) {
                Some(r) => r,
                None => self.type_error(format!("'{}' object is not subscriptable", n.type_name())),
            },
            other => self.type_error(format!("'{}' object is not subscriptable", other.type_name())),
        }
    }

    pub fn set_item(&mut self, o: &Value, k: Value, v: Value) -> R<()> {
        match o {
            Value::List(l) => {
                if let Value::Slice(s) = &k {
                    let items = self.iterate(&v)?;
                    let mut l = l.borrow_mut();
                    let (a, b, c) = slice_indices(l.len() as i64, &s.0, &s.1, &s.2).ok_or_else(|| self.exc("ValueError", "slice step cannot be zero"))?;
                    if c == 1 {
                        let (a, b) = (a as usize, (b.max(a)) as usize);
                        l.splice(a..b, items);
                    } else {
                        for (p, x) in slice_positions(a, b, c).into_iter().zip(items) {
                            l[p] = x;
                        }
                    }
                    return Ok(());
                }
                let len = l.borrow().len();
                match self.index_of(len, &k)? {
                    Some(i) => {
                        l.borrow_mut()[i] = v;
                        Ok(())
                    }
                    None => Err(self.exc("IndexError", "list assignment index out of range")),
                }
            }
            Value::Dict(d) => {
                let hk = self.hash(&k)?;
                d.borrow_mut().insert(hk, k, v);
                Ok(())
            }
            Value::Instance(i) => match i.class.lookup("__setitem__") {
                Some(m) => {
                    self.call(&m, vec![o.clone(), k, v], Vec::new())?;
                    Ok(())
                }
                None => self.type_error(format!("'{}' object does not support item assignment", i.class.name)),
            },
            Value::Native(n) => match n.set_item(self, &k, v) {
                Some(r) => r,
                None => self.type_error(format!("'{}' object does not support item assignment", n.type_name())),
            },
            other => self.type_error(format!("'{}' object does not support item assignment", other.type_name())),
        }
    }

    pub fn del_item(&mut self, o: &Value, k: &Value) -> R<()> {
        match o {
            Value::List(l) => {
                if let Value::Slice(s) = k {
                    let mut l = l.borrow_mut();
                    let (a, b, c) = slice_indices(l.len() as i64, &s.0, &s.1, &s.2).ok_or_else(|| self.exc("ValueError", "slice step cannot be zero"))?;
                    let mut pos = slice_positions(a, b, c);
                    pos.sort_unstable();
                    for p in pos.into_iter().rev() {
                        l.remove(p);
                    }
                    return Ok(());
                }
                let len = l.borrow().len();
                match self.index_of(len, k)? {
                    Some(i) => {
                        l.borrow_mut().remove(i);
                        Ok(())
                    }
                    None => Err(self.exc("IndexError", "list assignment index out of range")),
                }
            }
            Value::Dict(d) => {
                let hk = self.hash(k)?;
                if d.borrow_mut().remove(&hk).is_none() {
                    let r = self.repr(k)?;
                    return Err(self.exc("KeyError", r));
                }
                Ok(())
            }
            _ => self.type_error(format!("'{}' object doesn't support item deletion", o.type_name())),
        }
    }
}

pub fn range_len(a: i64, b: i64, c: i64) -> i64 {
    if c > 0 && a < b {
        (b - a + c - 1) / c
    } else if c < 0 && a > b {
        (a - b - c - 1) / (-c)
    } else {
        0
    }
}

pub fn identical(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::None, Value::None) => true,
        (Value::NotImplemented, Value::NotImplemented) => true,
        (Value::Ellipsis, Value::Ellipsis) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Int(x), Value::Int(y)) => x == y && (-5..=256).contains(x),
        (Value::Str(x), Value::Str(y)) => Rc::ptr_eq(x, y) || (x.len() <= 1 && x == y),
        (Value::List(x), Value::List(y)) => Rc::ptr_eq(x, y),
        (Value::Dict(x), Value::Dict(y)) => Rc::ptr_eq(x, y),
        (Value::Tuple(x), Value::Tuple(y)) => Rc::ptr_eq(x, y) || (x.is_empty() && y.is_empty()),
        (Value::Instance(x), Value::Instance(y)) => Rc::ptr_eq(x, y),
        (Value::Class(x), Value::Class(y)) => Rc::ptr_eq(x, y),
        (Value::Func(x), Value::Func(y)) => Rc::ptr_eq(x, y),
        (Value::Module(x), Value::Module(y)) => Rc::ptr_eq(x, y),
        (Value::Native(x), Value::Native(y)) => Rc::ptr_eq(x, y),
        _ => false,
    }
}

#[derive(Clone, Copy)]
enum Num {
    I(i64),
    F(f64),
}
impl Num {
    fn f(self) -> f64 {
        match self {
            Num::I(i) => i as f64,
            Num::F(f) => f,
        }
    }
}

pub fn binop_name(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "add",
        BinOp::Sub => "sub",
        BinOp::Mul => "mul",
        BinOp::Div => "truediv",
        BinOp::FloorDiv => "floordiv",
        BinOp::Mod => "mod",
        BinOp::Pow => "pow",
        BinOp::LShift => "lshift",
        BinOp::RShift => "rshift",
        BinOp::BitOr => "or",
        BinOp::BitXor => "xor",
        BinOp::BitAnd => "and",
        BinOp::MatMul => "matmul",
    }
}

pub fn binop_symbol(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::FloorDiv => "//",
        BinOp::Mod => "%",
        BinOp::Pow => "** or pow()",
        BinOp::LShift => "<<",
        BinOp::RShift => ">>",
        BinOp::BitOr => "|",
        BinOp::BitXor => "^",
        BinOp::BitAnd => "&",
        BinOp::MatMul => "@",
    }
}
