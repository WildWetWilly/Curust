//! A small `numpy`: n-dimensional float64 / int64 / bool arrays in C order with broadcasting,
//! indexing, slicing and the functions Cura's scripts use. Reductions follow NumPy's
//! summation order (pairwise along the contiguous axis), so results are bit-identical.

use crate::ast::{BinOp, CmpOp, Name};
use crate::builtins::{builtin, to_float, to_int};
use crate::interp::{Interp, R};
use crate::modules::make_module;
use crate::ops::slice_indices;
use crate::value::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum DType {
    Bool,
    Int,
    Float,
}

impl DType {
    fn name(self) -> &'static str {
        match self {
            DType::Bool => "bool",
            DType::Int => "int64",
            DType::Float => "float64",
        }
    }
    fn promote(a: DType, b: DType) -> DType {
        if a == DType::Float || b == DType::Float {
            DType::Float
        } else {
            DType::Int
        }
    }
}

pub struct NdArray {
    pub data: RefCell<Vec<f64>>,
    pub shape: RefCell<Vec<usize>>,
    pub dtype: Cell<DType>,
}

fn strides(shape: &[usize]) -> Vec<usize> {
    let mut s = vec![1; shape.len()];
    for k in (0..shape.len().saturating_sub(1)).rev() {
        s[k] = s[k + 1] * shape[k + 1];
    }
    s
}

fn size(shape: &[usize]) -> usize {
    shape.iter().product()
}

pub fn new_array(data: Vec<f64>, shape: Vec<usize>, dtype: DType) -> Value {
    Value::Native(Rc::new(NdArray { data: RefCell::new(data), shape: RefCell::new(shape), dtype: Cell::new(dtype) }))
}

fn scalar_value(x: f64, dt: DType) -> Value {
    match dt {
        DType::Float => Value::Float(x),
        DType::Int => Value::Int(x as i64),
        DType::Bool => Value::Bool(x != 0.0),
    }
}

fn as_nd(v: &Value) -> Option<&NdArray> {
    match v {
        Value::Native(n) => n.as_any().downcast_ref::<NdArray>(),
        _ => None,
    }
}

/// Convert any array-like (nested lists / tuples / scalars / arrays) to (data, shape, dtype).
pub fn to_array(i: &mut Interp, v: &Value) -> R<(Vec<f64>, Vec<usize>, DType)> {
    if let Some(a) = as_nd(v) {
        return Ok((a.data.borrow().clone(), a.shape.borrow().clone(), a.dtype.get()));
    }
    match v.unwrap_base() {
        Value::Bool(b) => Ok((vec![b as i64 as f64], vec![], DType::Bool)),
        Value::Int(n) => Ok((vec![n as f64], vec![], DType::Int)),
        Value::Float(f) => Ok((vec![f], vec![], DType::Float)),
        Value::List(_) | Value::Tuple(_) | Value::Iter(_) | Value::Range(..) => {
            let items = i.iterate(v)?;
            if items.is_empty() {
                return Ok((Vec::new(), vec![0], DType::Float));
            }
            let mut data = Vec::new();
            let mut sub_shape: Option<Vec<usize>> = None;
            let mut dt = DType::Bool;
            for it in &items {
                let (d, s, t) = to_array(i, it)?;
                match &sub_shape {
                    None => sub_shape = Some(s),
                    Some(ss) if *ss != s => return i.value_error("setting an array element with a sequence. The requested array has an inhomogeneous shape"),
                    _ => {}
                }
                dt = match (dt, t) {
                    (DType::Float, _) | (_, DType::Float) => DType::Float,
                    (DType::Int, _) | (_, DType::Int) => DType::Int,
                    _ => DType::Bool,
                };
                data.extend(d);
            }
            let mut shape = vec![items.len()];
            shape.extend(sub_shape.unwrap_or_default());
            Ok((data, shape, dt))
        }
        other => i.type_error(format!("cannot convert '{}' to an array", other.type_name())),
    }
}

fn broadcast(a: &[usize], b: &[usize]) -> Option<Vec<usize>> {
    let n = a.len().max(b.len());
    let mut out = vec![0; n];
    for k in 0..n {
        let x = if k + a.len() >= n { a[k + a.len() - n] } else { 1 };
        let y = if k + b.len() >= n { b[k + b.len() - n] } else { 1 };
        out[k] = if x == y {
            x
        } else if x == 1 {
            y
        } else if y == 1 {
            x
        } else {
            return None;
        };
    }
    Some(out)
}

/// Linear source index of output index `lin` for an input broadcast to `out_shape`.
fn source_indices(in_shape: &[usize], out_shape: &[usize]) -> Vec<usize> {
    let total = size(out_shape);
    let in_size = size(in_shape);
    if in_shape == out_shape {
        return (0..total).collect();
    }
    if in_size == 1 {
        return vec![0; total];
    }
    // Suffix broadcast: (m,) or (1, m) against (n, m).
    let trimmed: Vec<usize> = in_shape.iter().copied().skip_while(|&d| d == 1).collect();
    if out_shape.ends_with(&trimmed) {
        return (0..total).map(|k| k % in_size).collect();
    }
    let n = out_shape.len();
    let in_str = strides(in_shape);
    let out_str = strides(out_shape);
    (0..total)
        .map(|lin| {
            let mut src = 0;
            for k in 0..n {
                let idx = lin / out_str[k] % out_shape[k];
                let kk = k as isize - (n as isize - in_shape.len() as isize);
                if kk >= 0 {
                    let kk = kk as usize;
                    if in_shape[kk] != 1 {
                        src += idx * in_str[kk];
                    }
                }
            }
            src
        })
        .collect()
}

fn py_floor_div(a: f64, b: f64) -> f64 {
    if b == 0.0 {
        return a / b;
    }
    let m = a % b;
    let mut div = (a - m) / b;
    if m != 0.0 && ((b < 0.0) != (m < 0.0)) {
        div -= 1.0;
    }
    if div != 0.0 {
        let mut fl = div.floor();
        if div - fl > 0.5 {
            fl += 1.0;
        }
        fl
    } else {
        0.0f64.copysign(a / b)
    }
}

fn py_mod(a: f64, b: f64) -> f64 {
    if b == 0.0 {
        return f64::NAN;
    }
    let m = a % b;
    if m != 0.0 {
        if (b < 0.0) != (m < 0.0) {
            m + b
        } else {
            m
        }
    } else {
        0.0f64.copysign(b)
    }
}

fn apply(op: BinOp, x: f64, y: f64, dt: DType) -> f64 {
    match op {
        BinOp::Add => x + y,
        BinOp::Sub => x - y,
        BinOp::Mul => x * y,
        BinOp::Div => x / y,
        BinOp::FloorDiv => {
            if dt == DType::Float {
                py_floor_div(x, y)
            } else if y == 0.0 {
                0.0
            } else {
                (x / y).floor()
            }
        }
        BinOp::Mod => {
            if dt == DType::Float {
                py_mod(x, y)
            } else if y == 0.0 {
                0.0
            } else {
                x - (x / y).floor() * y
            }
        }
        BinOp::Pow => {
            if y == 2.0 {
                x * x
            } else {
                x.powf(y)
            }
        }
        BinOp::BitAnd => ((x as i64) & (y as i64)) as f64,
        BinOp::BitOr => ((x as i64) | (y as i64)) as f64,
        BinOp::BitXor => ((x as i64) ^ (y as i64)) as f64,
        BinOp::LShift => ((x as i64) << (y as i64)) as f64,
        BinOp::RShift => ((x as i64) >> (y as i64)) as f64,
        BinOp::MatMul => f64::NAN,
    }
}

pub fn elementwise(i: &mut Interp, a: &Value, b: &Value, f: &dyn Fn(f64, f64) -> f64, dt: DType) -> R<Value> {
    let (ad, ash, _) = to_array(i, a)?;
    let (bd, bsh, _) = to_array(i, b)?;
    let Some(out) = broadcast(&ash, &bsh) else {
        return i.value_error(format!("operands could not be broadcast together with shapes {:?} {:?}", ash, bsh));
    };
    let n = size(&out);
    let data: Vec<f64> = if ash == out && bsh == out {
        ad.iter().zip(bd.iter()).map(|(x, y)| f(*x, *y)).collect()
    } else {
        let ia = source_indices(&ash, &out);
        let ib = source_indices(&bsh, &out);
        (0..n).map(|k| f(ad[ia[k]], bd[ib[k]])).collect()
    };
    if out.is_empty() {
        return Ok(scalar_value(data[0], dt));
    }
    Ok(new_array(data, out, dt))
}

fn binop_arrays(i: &mut Interp, op: BinOp, a: &Value, b: &Value) -> R<Value> {
    let ta = dtype_of(a);
    let tb = dtype_of(b);
    let dt = match op {
        BinOp::Div => DType::Float,
        BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor if ta == DType::Bool && tb == DType::Bool => DType::Bool,
        _ => DType::promote(ta, tb),
    };
    if op == BinOp::MatMul {
        return dot(i, a, b);
    }
    elementwise(i, a, b, &|x, y| apply(op, x, y, dt), dt)
}

fn dtype_of(v: &Value) -> DType {
    match v.unwrap_base() {
        Value::Native(n) => n.as_any().downcast_ref::<NdArray>().map(|a| a.dtype.get()).unwrap_or(DType::Float),
        Value::Bool(_) => DType::Bool,
        Value::Int(_) => DType::Int,
        Value::List(l) => {
            let items = l.borrow();
            if items.iter().all(|x| matches!(x, Value::Int(_) | Value::Bool(_))) {
                DType::Int
            } else {
                DType::Float
            }
        }
        _ => DType::Float,
    }
}

/// Array comparison (`a < b` → bool array).
pub fn compare(i: &mut Interp, op: CmpOp, a: &Value, b: &Value) -> R<Value> {
    let f: fn(f64, f64) -> f64 = match op {
        CmpOp::Lt => |x, y| (x < y) as i64 as f64,
        CmpOp::LtE => |x, y| (x <= y) as i64 as f64,
        CmpOp::Gt => |x, y| (x > y) as i64 as f64,
        CmpOp::GtE => |x, y| (x >= y) as i64 as f64,
        CmpOp::Eq => |x, y| (x == y) as i64 as f64,
        CmpOp::NotEq => |x, y| (x != y) as i64 as f64,
        _ => return i.type_error("unsupported array comparison"),
    };
    elementwise(i, a, b, &f, DType::Bool)
}

// ---------------------------------------------------------------------- indexing

enum Ix {
    Int(i64),
    Slice(Value, Value, Value),
    NewAxis,
    Ellipsis,
}

impl NdArray {
    fn snapshot(&self) -> (Vec<f64>, Vec<usize>, DType) {
        (self.data.borrow().clone(), self.shape.borrow().clone(), self.dtype.get())
    }

    /// Source linear indices and the result shape of an index expression.
    fn select(&self, i: &mut Interp, key: &Value) -> R<(Vec<usize>, Vec<usize>)> {
        let shape = self.shape.borrow().clone();
        let st = strides(&shape);
        let items: Vec<Ix> = {
            let list: Vec<Value> = match key {
                Value::Tuple(t) => (**t).clone(),
                other => vec![other.clone()],
            };
            let mut out = Vec::new();
            for k in list {
                out.push(match &k {
                    Value::None => Ix::NewAxis,
                    Value::Ellipsis => Ix::Ellipsis,
                    Value::Slice(s) => Ix::Slice(s.0.clone(), s.1.clone(), s.2.clone()),
                    other => Ix::Int(to_int(i, other)?),
                });
            }
            out
        };
        let consumed = items.iter().filter(|x| matches!(x, Ix::Int(_) | Ix::Slice(..))).count();
        if consumed > shape.len() {
            return Err(i.exc("IndexError", format!("too many indices for array: array is {}-dimensional, but {} were indexed", shape.len(), consumed)));
        }
        // Per output dimension: the list of source offsets.
        let mut dims: Vec<Vec<usize>> = Vec::new();
        let mut fixed = 0usize;
        let mut d = 0usize;
        let mut expanded: Vec<Ix> = Vec::new();
        for it in items {
            if let Ix::Ellipsis = it {
                for _ in 0..shape.len() - consumed {
                    expanded.push(Ix::Slice(Value::None, Value::None, Value::None));
                }
            } else {
                expanded.push(it);
            }
        }
        for it in expanded {
            match it {
                Ix::Int(k) => {
                    let n = shape[d] as i64;
                    let kk = if k < 0 { k + n } else { k };
                    if kk < 0 || kk >= n {
                        return Err(i.exc("IndexError", format!("index {} is out of bounds for axis {} with size {}", k, d, n)));
                    }
                    fixed += kk as usize * st[d];
                    d += 1;
                }
                Ix::Slice(a, b, c) => {
                    let (s, e, stp) = slice_indices(shape[d] as i64, &a, &b, &c).ok_or_else(|| i.exc("ValueError", "slice step cannot be zero"))?;
                    let mut idx = Vec::new();
                    let mut x = s;
                    while (stp > 0 && x < e) || (stp < 0 && x > e) {
                        idx.push(x as usize * st[d]);
                        x += stp;
                    }
                    dims.push(idx);
                    d += 1;
                }
                Ix::NewAxis => dims.push(vec![0]),
                Ix::Ellipsis => {}
            }
        }
        while d < shape.len() {
            dims.push((0..shape[d]).map(|x| x * st[d]).collect());
            d += 1;
        }
        let out_shape: Vec<usize> = dims.iter().map(|v| v.len()).collect();
        let mut idx = vec![fixed];
        for dl in &dims {
            let mut next = Vec::with_capacity(idx.len() * dl.len());
            for base in &idx {
                for off in dl {
                    next.push(base + off);
                }
            }
            idx = next;
        }
        Ok((idx, out_shape))
    }

    fn mask_select(&self, mask: &NdArray) -> Option<(Vec<f64>, Vec<usize>)> {
        if mask.dtype.get() != DType::Bool || *mask.shape.borrow() != *self.shape.borrow() {
            return None;
        }
        let data: Vec<f64> = self.data.borrow().iter().zip(mask.data.borrow().iter()).filter(|(_, m)| **m != 0.0).map(|(x, _)| *x).collect();
        let n = data.len();
        Some((data, vec![n]))
    }
}

impl NativeObject for NdArray {
    fn type_name(&self) -> &str {
        "numpy.ndarray"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn repr(&self) -> String {
        let (d, s, t) = self.snapshot();
        format!("array({})", fmt_nested(&d, &s, t))
    }
    fn truthy(&self) -> bool {
        let d = self.data.borrow();
        d.len() == 1 && d[0] != 0.0 || d.len() > 1
    }
    fn len(&self) -> Option<usize> {
        self.shape.borrow().first().copied()
    }
    fn get_item(&self, i: &mut Interp, k: &Value) -> Option<R<Value>> {
        if let Some(m) = as_nd(k) {
            if let Some((d, s)) = self.mask_select(m) {
                return Some(Ok(new_array(d, s, self.dtype.get())));
            }
            // Integer fancy indexing on the first axis.
            let idx: Vec<f64> = m.data.borrow().clone();
            let mut out = Vec::new();
            let mut n = 0;
            for x in idx {
                match self.select(i, &Value::Int(x as i64)) {
                    Ok((ix, _)) => {
                        let data = self.data.borrow();
                        out.extend(ix.iter().map(|&j| data[j]));
                        n += 1;
                    }
                    Err(e) => return Some(Err(e)),
                }
            }
            let mut shape = vec![n];
            shape.extend(self.shape.borrow().iter().skip(1));
            return Some(Ok(new_array(out, shape, self.dtype.get())));
        }
        Some((|| {
            let (idx, shape) = self.select(i, k)?;
            let data = self.data.borrow();
            if shape.is_empty() {
                return Ok(scalar_value(data[idx[0]], self.dtype.get()));
            }
            Ok(new_array(idx.iter().map(|&j| data[j]).collect(), shape, self.dtype.get()))
        })())
    }
    fn set_item(&self, i: &mut Interp, k: &Value, v: Value) -> Option<R<()>> {
        Some((|| {
            let (idx, shape) = self.select(i, k)?;
            let (vd, vs, vt) = to_array(i, &v)?;
            let src = match broadcast(&vs, &shape) {
                Some(b) if b == shape => source_indices(&vs, &shape),
                _ => return i.value_error(format!("could not broadcast input array from shape {:?} into shape {:?}", vs, shape)),
            };
            let dt = self.dtype.get();
            let mut data = self.data.borrow_mut();
            for (n, &j) in idx.iter().enumerate() {
                let x = vd[src[n]];
                data[j] = match dt {
                    DType::Int if vt == DType::Float => x.trunc(),
                    DType::Bool => (x != 0.0) as i64 as f64,
                    _ => x,
                };
            }
            Ok(())
        })())
    }
    fn iter(&self, i: &mut Interp) -> Option<R<Vec<Value>>> {
        let n = self.shape.borrow().first().copied();
        let Some(n) = n else {
            return Some(i.type_error("iteration over a 0-d array"));
        };
        let mut out = Vec::with_capacity(n);
        for k in 0..n {
            match self.get_item(i, &Value::Int(k as i64)).unwrap() {
                Ok(v) => out.push(v),
                Err(e) => return Some(Err(e)),
            }
        }
        Some(Ok(out))
    }
    fn binop(&self, i: &mut Interp, op: BinOp, other: &Value, reflected: bool) -> Option<R<Value>> {
        let me = new_array(self.data.borrow().clone(), self.shape.borrow().clone(), self.dtype.get());
        Some(if reflected { binop_arrays(i, op, other, &me) } else { binop_arrays(i, op, &me, other) })
    }
    fn get_attr(&self, _i: &mut Interp, this: &Value, name: &str) -> Option<R<Value>> {
        let v = match name {
            "shape" => Value::tuple(self.shape.borrow().iter().map(|&d| Value::Int(d as i64)).collect()),
            "ndim" => Value::Int(self.shape.borrow().len() as i64),
            "size" => Value::Int(self.data.borrow().len() as i64),
            "dtype" => Value::str(self.dtype.get().name()),
            "T" => {
                let (d, s, t) = self.snapshot();
                if s.len() == 2 {
                    let (r, c) = (s[0], s[1]);
                    let mut out = vec![0.0; d.len()];
                    for y in 0..r {
                        for x in 0..c {
                            out[x * r + y] = d[y * c + x];
                        }
                    }
                    new_array(out, vec![c, r], t)
                } else {
                    new_array(d, s, t)
                }
            }
            "sum" | "max" | "min" | "argmax" | "argmin" | "copy" | "tolist" | "reshape" | "astype" | "mean" | "flatten" | "ravel" | "any" | "all" | "round" | "clip" | "dot" | "item" | "fill" | "cumsum" | "prod" => Value::Method(Rc::new((this.clone(), method_name(name)))),
            _ => return None,
        };
        Some(Ok(v))
    }
    fn call_method(&self, i: &mut Interp, this: &Value, name: &str, a: Vec<Value>, kw: Vec<(Name, Value)>) -> Option<R<Value>> {
        let mut args = vec![this.clone()];
        args.extend(a);
        Some(match name {
            "copy" => {
                let (d, s, t) = self.snapshot();
                Ok(new_array(d, s, t))
            }
            "tolist" => Ok(to_list(&self.data.borrow(), &self.shape.borrow(), self.dtype.get())),
            "item" => {
                let d = self.data.borrow();
                if d.len() == 1 {
                    Ok(scalar_value(d[0], self.dtype.get()))
                } else {
                    i.value_error("can only convert an array of size 1 to a Python scalar")
                }
            }
            "fill" => {
                let x = to_float(i, args.get(1).unwrap_or(&Value::Int(0)));
                match x {
                    Ok(x) => {
                        self.data.borrow_mut().iter_mut().for_each(|v| *v = x);
                        Ok(Value::None)
                    }
                    Err(e) => Err(e),
                }
            }
            "astype" => {
                let (d, s, _) = self.snapshot();
                let t = dtype_from(args.get(1));
                let d = match t {
                    DType::Int => d.iter().map(|x| x.trunc()).collect(),
                    DType::Bool => d.iter().map(|x| (*x != 0.0) as i64 as f64).collect(),
                    DType::Float => d,
                };
                Ok(new_array(d, s, t))
            }
            "flatten" | "ravel" => {
                let (d, _, t) = self.snapshot();
                let n = d.len();
                Ok(new_array(d, vec![n], t))
            }
            "reshape" => {
                let dims: Vec<Value> = if args.len() == 2 { match &args[1] { Value::Tuple(t) => (**t).clone(), Value::List(l) => l.borrow().clone(), other => vec![other.clone()] } } else { args[1..].to_vec() };
                reshape(i, this, &dims)
            }
            other => call_function(i, other, args, kw),
        })
    }
}

fn method_name(n: &str) -> &'static str {
    const NAMES: &[&str] = &["sum", "max", "min", "argmax", "argmin", "copy", "tolist", "reshape", "astype", "mean", "flatten", "ravel", "any", "all", "round", "clip", "dot", "item", "fill", "cumsum", "prod"];
    NAMES.iter().find(|x| **x == n).copied().unwrap_or("copy")
}

fn dtype_from(v: Option<&Value>) -> DType {
    match v {
        Some(Value::Class(c)) => match c.builtin {
            "int" => DType::Int,
            "bool" => DType::Bool,
            _ => DType::Float,
        },
        Some(Value::Str(s)) => {
            if s.starts_with("int") || s.starts_with("uint") {
                DType::Int
            } else if &**s == "bool" {
                DType::Bool
            } else {
                DType::Float
            }
        }
        Some(Value::Native(n)) => match n.as_any().downcast_ref::<NpFunc>().map(|f| f.0) {
            Some("int64" | "int32" | "int_" | "intc") => DType::Int,
            Some("bool_") => DType::Bool,
            _ => DType::Float,
        },
        Some(Value::Builtin(b)) => match b.name {
            "int64" | "int32" | "int_" | "uint8" => DType::Int,
            "bool_" => DType::Bool,
            _ => DType::Float,
        },
        _ => DType::Float,
    }
}

fn to_list(d: &[f64], s: &[usize], t: DType) -> Value {
    if s.is_empty() {
        return scalar_value(d[0], t);
    }
    if s.len() == 1 {
        return Value::list(d.iter().map(|&x| scalar_value(x, t)).collect());
    }
    let inner = size(&s[1..]);
    Value::list((0..s[0]).map(|k| to_list(&d[k * inner..(k + 1) * inner], &s[1..], t)).collect())
}

fn fmt_nested(d: &[f64], s: &[usize], t: DType) -> String {
    let one = |x: f64| -> String {
        match t {
            DType::Float => {
                let r = float_repr(x);
                if let Some(p) = r.strip_suffix(".0") {
                    format!("{}.", p)
                } else {
                    r
                }
            }
            DType::Int => format!("{}", x as i64),
            DType::Bool => if x != 0.0 { "True" } else { "False" }.into(),
        }
    };
    if s.is_empty() {
        return one(d[0]);
    }
    if s.len() == 1 {
        return format!("[{}]", d.iter().map(|&x| one(x)).collect::<Vec<_>>().join(", "));
    }
    let inner = size(&s[1..]);
    format!("[{}]", (0..s[0]).map(|k| fmt_nested(&d[k * inner..(k + 1) * inner], &s[1..], t)).collect::<Vec<_>>().join(", "))
}

fn reshape(i: &mut Interp, a: &Value, dims: &[Value]) -> R<Value> {
    let (d, _, t) = to_array(i, a)?;
    let mut shape: Vec<i64> = dims.iter().map(|v| to_int(i, v)).collect::<R<_>>()?;
    let known: i64 = shape.iter().filter(|&&x| x >= 0).product();
    if let Some(p) = shape.iter().position(|&x| x < 0) {
        shape[p] = if known == 0 { 0 } else { d.len() as i64 / known };
    }
    let shape: Vec<usize> = shape.into_iter().map(|x| x as usize).collect();
    if size(&shape) != d.len() {
        return i.value_error(format!("cannot reshape array of size {} into shape {:?}", d.len(), shape));
    }
    Ok(new_array(d, shape, t))
}

// ---------------------------------------------------------------------- reductions

/// NumPy's pairwise summation (`pairwise_sum_DOUBLE`).
pub fn pairwise_sum(a: &[f64]) -> f64 {
    let n = a.len();
    if n < 8 {
        let mut res = 0.0;
        for &x in a {
            res += x;
        }
        res
    } else if n <= 128 {
        let mut r = [0.0f64; 8];
        r.copy_from_slice(&a[..8]);
        let mut k = 8;
        while k < n - (n % 8) {
            for j in 0..8 {
                r[j] += a[k + j];
            }
            k += 8;
        }
        let mut res = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
        while k < n {
            res += a[k];
            k += 1;
        }
        res
    } else {
        let mut n2 = n / 2;
        n2 -= n2 % 8;
        pairwise_sum(&a[..n2]) + pairwise_sum(&a[n2..])
    }
}

fn axis_arg(i: &mut Interp, a: &[Value], kw: &[(Name, Value)], pos: usize) -> R<Option<i64>> {
    match a.get(pos).cloned().or_else(|| kw.iter().find(|(k, _)| &**k == "axis").map(|(_, v)| v.clone())) {
        None | Some(Value::None) => Ok(None),
        Some(v) => Ok(Some(to_int(i, &v)?)),
    }
}

/// Reduce along `axis` (or everything) with `f` over each lane (as a contiguous Vec).
fn reduce(i: &mut Interp, v: &Value, axis: Option<i64>, f: &dyn Fn(&[f64]) -> f64, out_dt: Option<DType>) -> R<Value> {
    let (d, s, t) = to_array(i, v)?;
    let dt = out_dt.unwrap_or(t);
    match axis {
        None => Ok(scalar_value(f(&d), dt)),
        Some(ax) => {
            let nd = s.len() as i64;
            let ax = if ax < 0 { ax + nd } else { ax };
            if ax < 0 || ax >= nd.max(1) {
                return Err(i.exc("IndexError", format!("axis {} is out of bounds for array of dimension {}", ax, nd)));
            }
            if s.is_empty() {
                return Ok(scalar_value(f(&d), dt));
            }
            let ax = ax as usize;
            let outer: usize = s[..ax].iter().product();
            let len = s[ax];
            let inner: usize = s[ax + 1..].iter().product();
            let mut out = Vec::with_capacity(outer * inner);
            let mut lane = Vec::with_capacity(len);
            for o in 0..outer {
                for n in 0..inner {
                    lane.clear();
                    for k in 0..len {
                        lane.push(d[(o * len + k) * inner + n]);
                    }
                    out.push(f(&lane));
                }
            }
            let mut shape = s.clone();
            shape.remove(ax);
            if shape.is_empty() {
                return Ok(scalar_value(out[0], dt));
            }
            Ok(new_array(out, shape, dt))
        }
    }
}

fn sum_lane(lane: &[f64], contiguous: bool) -> f64 {
    if contiguous {
        0.0 + pairwise_sum(lane)
    } else {
        let mut acc = 0.0;
        for &x in lane {
            acc += x;
        }
        acc
    }
}

fn nan_max(lane: &[f64]) -> f64 {
    let mut m = lane[0];
    for &x in &lane[1..] {
        if m.is_nan() {
            return m;
        }
        if x.is_nan() || x > m {
            m = x;
        }
    }
    m
}

fn nan_min(lane: &[f64]) -> f64 {
    let mut m = lane[0];
    for &x in &lane[1..] {
        if m.is_nan() {
            return m;
        }
        if x.is_nan() || x < m {
            m = x;
        }
    }
    m
}

fn arg_best(lane: &[f64], max: bool) -> f64 {
    let mut best = 0;
    for k in 1..lane.len() {
        if lane[best].is_nan() {
            break;
        }
        if lane[k].is_nan() || (max && lane[k] > lane[best]) || (!max && lane[k] < lane[best]) {
            best = k;
        }
    }
    best as f64
}

fn dot(i: &mut Interp, a: &Value, b: &Value) -> R<Value> {
    let (ad, ash, at) = to_array(i, a)?;
    let (bd, bsh, bt) = to_array(i, b)?;
    let dt = DType::promote(at, bt);
    match (ash.len(), bsh.len()) {
        (0, _) | (_, 0) => binop_arrays(i, BinOp::Mul, a, b),
        (1, 1) => {
            if ash[0] != bsh[0] {
                return i.value_error("shapes not aligned");
            }
            let mut acc = 0.0;
            for k in 0..ash[0] {
                acc += ad[k] * bd[k];
            }
            Ok(scalar_value(acc, dt))
        }
        (2, 1) => {
            let (r, c) = (ash[0], ash[1]);
            if c != bsh[0] {
                return i.value_error("shapes not aligned");
            }
            let out: Vec<f64> = (0..r).map(|y| (0..c).fold(0.0, |acc, k| acc + ad[y * c + k] * bd[k])).collect();
            Ok(new_array(out, vec![r], dt))
        }
        (1, 2) => {
            let (r, c) = (bsh[0], bsh[1]);
            if ash[0] != r {
                return i.value_error("shapes not aligned");
            }
            let out: Vec<f64> = (0..c).map(|x| (0..r).fold(0.0, |acc, k| acc + ad[k] * bd[k * c + x])).collect();
            Ok(new_array(out, vec![c], dt))
        }
        (2, 2) => {
            let (r, n, c) = (ash[0], ash[1], bsh[1]);
            if n != bsh[0] {
                return i.value_error("shapes not aligned");
            }
            let mut out = vec![0.0; r * c];
            for y in 0..r {
                for x in 0..c {
                    out[y * c + x] = (0..n).fold(0.0, |acc, k| acc + ad[y * n + k] * bd[k * c + x]);
                }
            }
            Ok(new_array(out, vec![r, c], dt))
        }
        _ => i.value_error("dot of arrays with more than 2 dimensions is not supported"),
    }
}

fn unary_map(i: &mut Interp, v: &Value, f: fn(f64) -> f64, keep_int: bool) -> R<Value> {
    let (d, s, t) = to_array(i, v)?;
    let dt = if keep_int && t != DType::Float { DType::Int } else { DType::Float };
    let out: Vec<f64> = d.iter().map(|&x| f(x)).collect();
    if s.is_empty() {
        return Ok(scalar_value(out[0], dt));
    }
    Ok(new_array(out, s, dt))
}

fn kwarg(kw: &[(Name, Value)], name: &str) -> Option<Value> {
    kw.iter().find(|(k, _)| &**k == name).map(|(_, v)| v.clone())
}

/// `numpy.<name>(*args)`; also used for array methods (`a.sum(1)` = `sum(a, 1)`).
pub fn call_function(i: &mut Interp, name: &str, a: Vec<Value>, kw: Vec<(Name, Value)>) -> R<Value> {
    let first = a.first().cloned().unwrap_or(Value::None);
    match name {
        "array" | "asarray" | "copy" | "asanyarray" => {
            let (d, s, mut t) = to_array(i, &first)?;
            if let Some(dt) = kwarg(&kw, "dtype").or_else(|| a.get(1).cloned()) {
                t = dtype_from(Some(&dt));
            }
            if s.is_empty() {
                return Ok(new_array(d, s, t));
            }
            Ok(new_array(d, s, t))
        }
        "empty" | "zeros" | "ones" | "full" => {
            let shape: Vec<usize> = match &first {
                Value::Tuple(t) => t.iter().map(|v| to_int(i, v).map(|x| x as usize)).collect::<R<_>>()?,
                Value::List(l) => {
                    let items = l.borrow().clone();
                    items.iter().map(|v| to_int(i, v).map(|x| x as usize)).collect::<R<_>>()?
                }
                v => vec![to_int(i, v)? as usize],
            };
            let fill = match name {
                "ones" => 1.0,
                "full" => to_float(i, a.get(1).unwrap_or(&Value::Int(0)))?,
                _ => 0.0,
            };
            let dt = match kwarg(&kw, "dtype") {
                Some(d) => dtype_from(Some(&d)),
                None => {
                    if name == "full" {
                        dtype_of(a.get(1).unwrap_or(&Value::Float(0.0)))
                    } else {
                        DType::Float
                    }
                }
            };
            Ok(new_array(vec![fill; size(&shape)], shape, dt))
        }
        "zeros_like" | "ones_like" | "empty_like" => {
            let (d, s, t) = to_array(i, &first)?;
            Ok(new_array(vec![if name == "ones_like" { 1.0 } else { 0.0 }; d.len()], s, t))
        }
        "arange" => {
            let nums: Vec<f64> = a.iter().map(|v| to_float(i, v)).collect::<R<_>>()?;
            let (start, stop, step) = match nums.len() {
                1 => (0.0, nums[0], 1.0),
                2 => (nums[0], nums[1], 1.0),
                _ => (nums[0], nums[1], nums[2]),
            };
            let n = ((stop - start) / step).ceil().max(0.0) as usize;
            let dt = if a.iter().all(|v| matches!(v, Value::Int(_))) { DType::Int } else { DType::Float };
            Ok(new_array((0..n).map(|k| start + k as f64 * step).collect(), vec![n], dt))
        }
        "linspace" => {
            let s = to_float(i, &first)?;
            let e = to_float(i, a.get(1).unwrap_or(&Value::Int(0)))?;
            let n = match a.get(2).cloned().or_else(|| kwarg(&kw, "num")) {
                Some(v) => to_int(i, &v)? as usize,
                None => 50,
            };
            let step = if n > 1 { (e - s) / (n - 1) as f64 } else { 0.0 };
            let mut out: Vec<f64> = (0..n).map(|k| s + k as f64 * step).collect();
            if n > 1 {
                out[n - 1] = e;
            }
            Ok(new_array(out, vec![n], DType::Float))
        }
        "concatenate" | "vstack" | "hstack" => {
            let parts = i.iterate(&first)?;
            let mut data = Vec::new();
            let mut shape: Option<Vec<usize>> = None;
            let mut dt: Option<DType> = None;
            let axis = match kwarg(&kw, "axis").or_else(|| a.get(1).cloned()) {
                Some(v) if !v.is_none() => to_int(i, &v)?,
                _ => 0,
            };
            if axis != 0 && name == "concatenate" {
                // Concatenate along the last axis of 2-D arrays.
                let arrays: Vec<(Vec<f64>, Vec<usize>, DType)> = parts.iter().map(|p| to_array(i, p)).collect::<R<_>>()?;
                let rows = arrays[0].1[0];
                let mut out = Vec::new();
                let mut cols = 0;
                for (_, s, _) in &arrays {
                    cols += s[1];
                }
                for r in 0..rows {
                    for (d, s, _) in &arrays {
                        out.extend_from_slice(&d[r * s[1]..(r + 1) * s[1]]);
                    }
                }
                let t = arrays.iter().fold(DType::Bool, |acc, x| DType::promote(acc, x.2));
                return Ok(new_array(out, vec![rows, cols], t));
            }
            for p in parts {
                let (d, mut s, t) = to_array(i, &p)?;
                if name == "vstack" && s.len() == 1 {
                    s = vec![1, s[0]];
                }
                dt = Some(match dt {
                    None => t,
                    Some(x) => DType::promote(x, t),
                });
                match &mut shape {
                    None => shape = Some(s),
                    Some(sh) => {
                        if sh.len() != s.len() || sh[1..] != s[1..] {
                            return i.value_error(format!("all the input array dimensions except for the concatenation axis must match exactly, but along dimension 1, the array at index 0 has size {:?} and the array at index 1 has size {:?}", sh, s));
                        }
                        sh[0] += s[0];
                    }
                }
                data.extend(d);
            }
            Ok(new_array(data, shape.unwrap_or(vec![0]), dt.unwrap_or(DType::Float)))
        }
        "resize" => {
            let (d, _, t) = to_array(i, &first)?;
            let shape: Vec<usize> = match a.get(1) {
                Some(Value::Tuple(tp)) => tp.iter().map(|v| to_int(i, v).map(|x| x.max(0) as usize)).collect::<R<_>>()?,
                Some(v) => vec![to_int(i, v)?.max(0) as usize],
                None => vec![d.len()],
            };
            let n = size(&shape);
            let out: Vec<f64> = if d.is_empty() { vec![0.0; n] } else { (0..n).map(|k| d[k % d.len()]).collect() };
            Ok(new_array(out, shape, t))
        }
        "roll" => {
            let (d, s, t) = to_array(i, &first)?;
            let shift = to_int(i, a.get(1).unwrap_or(&Value::Int(0)))?;
            let axis = axis_arg(i, &a, &kw, 2)?;
            match axis {
                None => {
                    let n = d.len() as i64;
                    if n == 0 {
                        return Ok(new_array(d, s, t));
                    }
                    let out: Vec<f64> = (0..n).map(|k| d[((k - shift).rem_euclid(n)) as usize]).collect();
                    Ok(new_array(out, s, t))
                }
                Some(ax) => {
                    let ax = if ax < 0 { ax + s.len() as i64 } else { ax } as usize;
                    let outer: usize = s[..ax].iter().product();
                    let len = s[ax] as i64;
                    let inner: usize = s[ax + 1..].iter().product();
                    let mut out = vec![0.0; d.len()];
                    if len > 0 {
                        for o in 0..outer {
                            for k in 0..len {
                                let src = (k - shift).rem_euclid(len) as usize;
                                for n in 0..inner {
                                    out[(o * len as usize + k as usize) * inner + n] = d[(o * len as usize + src) * inner + n];
                                }
                            }
                        }
                    }
                    Ok(new_array(out, s, t))
                }
            }
        }
        "sum" => {
            let axis = axis_arg(i, &a, &kw, 1)?;
            let (_, s, t) = to_array(i, &first)?;
            let contiguous = match axis {
                None => true,
                Some(ax) => {
                    let nd = s.len() as i64;
                    let ax = if ax < 0 { ax + nd } else { ax };
                    ax == nd - 1
                }
            };
            let dt = if t == DType::Bool { DType::Int } else { t };
            reduce(i, &first, axis, &|lane| sum_lane(lane, contiguous), Some(dt))
        }
        "prod" => {
            let axis = axis_arg(i, &a, &kw, 1)?;
            reduce(i, &first, axis, &|lane| lane.iter().fold(1.0, |acc, x| acc * x), None)
        }
        "cumsum" => {
            let (d, _, t) = to_array(i, &first)?;
            let mut acc = 0.0;
            let out: Vec<f64> = d.iter().map(|x| {
                acc += x;
                acc
            }).collect();
            let n = out.len();
            Ok(new_array(out, vec![n], t))
        }
        "mean" | "average" => {
            let axis = axis_arg(i, &a, &kw, 1)?;
            let (_, s, _) = to_array(i, &first)?;
            let contiguous = axis.is_none() || axis == Some(s.len() as i64 - 1) || axis == Some(-1);
            reduce(i, &first, axis, &|lane| sum_lane(lane, contiguous) / lane.len() as f64, Some(DType::Float))
        }
        "amax" | "max" | "nanmax" => {
            let axis = axis_arg(i, &a, &kw, 1)?;
            let (d, _, _) = to_array(i, &first)?;
            if d.is_empty() {
                return i.value_error("zero-size array to reduction operation maximum which has no identity");
            }
            reduce(i, &first, axis, &nan_max, None)
        }
        "amin" | "min" | "nanmin" => {
            let axis = axis_arg(i, &a, &kw, 1)?;
            let (d, _, _) = to_array(i, &first)?;
            if d.is_empty() {
                return i.value_error("zero-size array to reduction operation minimum which has no identity");
            }
            reduce(i, &first, axis, &nan_min, None)
        }
        "argmax" | "argmin" => {
            let axis = axis_arg(i, &a, &kw, 1)?;
            let (d, _, _) = to_array(i, &first)?;
            if d.is_empty() {
                return i.value_error(format!("attempt to get {} of an empty sequence", name));
            }
            let max = name == "argmax";
            reduce(i, &first, axis, &move |lane| arg_best(lane, max), Some(DType::Int))
        }
        "any" => {
            let ax = axis_arg(i, &a, &kw, 1)?;
            reduce(i, &first, ax, &|lane| lane.iter().any(|x| *x != 0.0) as i64 as f64, Some(DType::Bool))
        }
        "all" => {
            let ax = axis_arg(i, &a, &kw, 1)?;
            reduce(i, &first, ax, &|lane| lane.iter().all(|x| *x != 0.0) as i64 as f64, Some(DType::Bool))
        }
        "clip" => {
            let lo = a.get(1).cloned().or_else(|| kwarg(&kw, "a_min")).unwrap_or(Value::None);
            let hi = a.get(2).cloned().or_else(|| kwarg(&kw, "a_max")).unwrap_or(Value::None);
            let max_f = |x: f64, y: f64| if x.is_nan() { x } else if y.is_nan() || y > x { y } else { x };
            let min_f = |x: f64, y: f64| if x.is_nan() { x } else if y.is_nan() || y < x { y } else { x };
            let t = DType::promote(dtype_of(&first), DType::promote(if lo.is_none() { DType::Int } else { dtype_of(&lo) }, if hi.is_none() { DType::Int } else { dtype_of(&hi) }));
            let mut v = first.clone();
            if !lo.is_none() {
                v = elementwise(i, &v, &lo, &max_f, t)?;
            }
            if !hi.is_none() {
                v = elementwise(i, &v, &hi, &min_f, t)?;
            }
            Ok(v)
        }
        "dot" | "matmul" => dot(i, &first, a.get(1).unwrap_or(&Value::None)),
        "sqrt" => unary_map(i, &first, f64::sqrt, false),
        "fabs" => unary_map(i, &first, f64::abs, false),
        "abs" | "absolute" => unary_map(i, &first, f64::abs, true),
        "floor" => unary_map(i, &first, f64::floor, false),
        "ceil" => unary_map(i, &first, f64::ceil, false),
        "trunc" => unary_map(i, &first, f64::trunc, false),
        "exp" => unary_map(i, &first, f64::exp, false),
        "log" => unary_map(i, &first, f64::ln, false),
        "log10" => unary_map(i, &first, f64::log10, false),
        "sin" => unary_map(i, &first, f64::sin, false),
        "cos" => unary_map(i, &first, f64::cos, false),
        "tan" => unary_map(i, &first, f64::tan, false),
        "arcsin" => unary_map(i, &first, f64::asin, false),
        "arccos" => unary_map(i, &first, f64::acos, false),
        "arctan" => unary_map(i, &first, f64::atan, false),
        "radians" | "deg2rad" => unary_map(i, &first, f64::to_radians, false),
        "degrees" | "rad2deg" => unary_map(i, &first, f64::to_degrees, false),
        "square" => unary_map(i, &first, |x| x * x, true),
        "isnan" => {
            let (d, s, _) = to_array(i, &first)?;
            let out: Vec<f64> = d.iter().map(|x| x.is_nan() as i64 as f64).collect();
            if s.is_empty() {
                return Ok(Value::Bool(out[0] != 0.0));
            }
            Ok(new_array(out, s, DType::Bool))
        }
        "round" | "around" => {
            let dec = match a.get(1).cloned().or_else(|| kwarg(&kw, "decimals")) {
                Some(v) => to_int(i, &v)?,
                None => 0,
            };
            let p = 10f64.powi(dec as i32);
            // NumPy: multiply, round half to even, divide.
            let f = move |x: f64| -> f64 {
                let y = if dec >= 0 { x * p } else { x / 10f64.powi(-dec as i32) };
                let r = y.round();
                let r = if (y - y.trunc()).abs() == 0.5 { 2.0 * (y / 2.0).round() } else { r };
                if dec >= 0 {
                    r / p
                } else {
                    r * 10f64.powi(-dec as i32)
                }
            };
            let (d, s, t) = to_array(i, &first)?;
            let out: Vec<f64> = d.iter().map(|&x| f(x)).collect();
            if s.is_empty() {
                return Ok(scalar_value(out[0], t));
            }
            Ok(new_array(out, s, t))
        }
        "floor_divide" => binop_arrays(i, BinOp::FloorDiv, &first, a.get(1).unwrap_or(&Value::Int(1))),
        "add" => binop_arrays(i, BinOp::Add, &first, a.get(1).unwrap_or(&Value::Int(0))),
        "subtract" => binop_arrays(i, BinOp::Sub, &first, a.get(1).unwrap_or(&Value::Int(0))),
        "multiply" => binop_arrays(i, BinOp::Mul, &first, a.get(1).unwrap_or(&Value::Int(1))),
        "divide" | "true_divide" => binop_arrays(i, BinOp::Div, &first, a.get(1).unwrap_or(&Value::Int(1))),
        "power" => binop_arrays(i, BinOp::Pow, &first, a.get(1).unwrap_or(&Value::Int(1))),
        "mod" | "remainder" => binop_arrays(i, BinOp::Mod, &first, a.get(1).unwrap_or(&Value::Int(1))),
        "maximum" => elementwise(i, &first, a.get(1).unwrap_or(&Value::None), &|x, y| if x.is_nan() || y.is_nan() { f64::NAN } else { x.max(y) }, DType::promote(dtype_of(&first), dtype_of(a.get(1).unwrap_or(&Value::None)))),
        "minimum" => elementwise(i, &first, a.get(1).unwrap_or(&Value::None), &|x, y| if x.is_nan() || y.is_nan() { f64::NAN } else { x.min(y) }, DType::promote(dtype_of(&first), dtype_of(a.get(1).unwrap_or(&Value::None)))),
        "hypot" => elementwise(i, &first, a.get(1).unwrap_or(&Value::None), &|x, y| x.hypot(y), DType::Float),
        "arctan2" => elementwise(i, &first, a.get(1).unwrap_or(&Value::None), &|y, x| y.atan2(x), DType::Float),
        "where" => {
            let c = first.clone();
            let x = a.get(1).cloned().unwrap_or(Value::None);
            let y = a.get(2).cloned().unwrap_or(Value::None);
            let (cd, cs, _) = to_array(i, &c)?;
            if x.is_none() {
                let idx: Vec<f64> = cd.iter().enumerate().filter(|(_, v)| **v != 0.0).map(|(k, _)| k as f64).collect();
                let n = idx.len();
                return Ok(Value::tuple(vec![new_array(idx, vec![n], DType::Int)]));
            }
            let (xd, xs, xt) = to_array(i, &x)?;
            let (yd, ys, yt) = to_array(i, &y)?;
            let shape = broadcast(&cs, &xs).and_then(|s| broadcast(&s, &ys)).ok_or_else(|| i.exc("ValueError", "operands could not be broadcast together"))?;
            let (ic, ix, iy) = (source_indices(&cs, &shape), source_indices(&xs, &shape), source_indices(&ys, &shape));
            let out: Vec<f64> = (0..size(&shape)).map(|k| if cd[ic[k]] != 0.0 { xd[ix[k]] } else { yd[iy[k]] }).collect();
            Ok(new_array(out, shape, DType::promote(xt, yt)))
        }
        "reshape" => {
            let dims: Vec<Value> = match a.get(1) {
                Some(Value::Tuple(t)) => (**t).clone(),
                Some(v) => vec![v.clone()],
                None => Vec::new(),
            };
            reshape(i, &first, &dims)
        }
        "shape" => {
            let (_, s, _) = to_array(i, &first)?;
            Ok(Value::tuple(s.iter().map(|&x| Value::Int(x as i64)).collect()))
        }
        "size" => {
            let (d, _, _) = to_array(i, &first)?;
            Ok(Value::Int(d.len() as i64))
        }
        "transpose" => i.get_attr(&first, "T"),
        "float64" | "float32" | "float_" | "double" => Ok(Value::Float(to_float(i, &first)?)),
        "int64" | "int32" | "int_" | "intc" => Ok(Value::Int(to_int(i, &first)?)),
        "bool_" => Ok(Value::Bool(i.truthy(&first)?)),
        other => Err(i.exc("AttributeError", format!("module 'numpy' has no attribute '{}'", other))),
    }
}

const FUNCS: &[&str] = &[
    "array", "asarray", "asanyarray", "copy", "empty", "zeros", "ones", "full", "zeros_like", "ones_like", "empty_like", "arange", "linspace", "concatenate", "vstack", "hstack", "resize", "roll", "sum", "prod", "cumsum", "mean", "average", "amax",
    "max", "nanmax", "amin", "min", "nanmin", "argmax", "argmin", "any", "all", "clip", "dot", "matmul", "sqrt", "fabs", "abs", "absolute", "floor", "ceil", "trunc", "exp", "log", "log10", "sin", "cos", "tan", "arcsin", "arccos",
    "arctan", "radians", "deg2rad", "degrees", "rad2deg", "square", "isnan", "round", "around", "floor_divide", "add", "subtract", "multiply", "divide", "true_divide", "power", "mod", "remainder", "maximum", "minimum", "hypot",
    "arctan2", "where", "reshape", "shape", "size", "transpose", "float64", "float32", "float_", "double", "int64", "int32", "int_", "intc", "bool_",
];

/// A numpy function object: a builtin whose name selects the function.
struct NpFunc(&'static str);
impl NativeObject for NpFunc {
    fn type_name(&self) -> &str {
        "numpy.ufunc"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn repr(&self) -> String {
        format!("<function {}>", self.0)
    }
    fn call(&self, i: &mut Interp, args: Vec<Value>, kw: Vec<(Name, Value)>) -> Option<R<Value>> {
        Some(call_function(i, self.0, args, kw))
    }
}

pub fn module() -> Value {
    let mut entries: Vec<(&str, Value)> = vec![
        ("pi", Value::Float(std::f64::consts::PI)),
        ("e", Value::Float(std::f64::consts::E)),
        ("inf", Value::Float(f64::INFINITY)),
        ("nan", Value::Float(f64::NAN)),
        ("newaxis", Value::None),
        ("ndarray", builtin("ndarray", |i, _a, _kw| i.type_error("use numpy.array() to create arrays"))),
        ("__version__", Value::str("1.26.4")),
    ];
    for f in FUNCS {
        entries.push((f, Value::Native(Rc::new(NpFunc(f)))));
    }
    make_module("numpy", entries)
}
