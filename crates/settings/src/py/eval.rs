//! Evaluation of parsed expressions with CPython semantics.

use super::ast::{BinOp, CmpOp, Comprehension, Expr, Target, UnOp};
use super::builtins;
use super::value::{Num, PyErr, PyResult, Value};
use std::rc::Rc;

/// What an evaluation can see beyond its own temporaries.
pub trait Env {
    /// The `locals` mapping of `eval()` (setting values and additional variables).
    fn local(&self, name: &str) -> Option<Value>;
    /// Extra globals (registered formula operators); `None` if not defined.
    fn global(&self, name: &str) -> Option<Value>;
    /// Call an operator returned by `global`.
    fn call_operator(&self, name: &str, args: &[Value], kwargs: &[(Rc<str>, Value)]) -> PyResult<Value>;
}

/// The names `SettingFunction`'s module globals define (besides `math` and the operators).
const MODULE_GLOBALS: &[&str] = &[
    "ast", "base64", "builtins", "hashlib", "uuid", "CodeType", "Any", "Callable", "Dict", "FrozenSet", "NamedTuple", "Optional", "Set",
    "TYPE_CHECKING", "Logger", "ContainerInterface", "PropertyEvaluationContext", "traceback", "IllegalMethodError", "_debug_value",
    "SettingFunction", "_VisitResult", "_SettingExpressionVisitor", "__name__", "__doc__", "__file__", "__builtins__",
];

pub struct Evaluator<'a> {
    env: &'a dyn Env,
    /// Comprehension / lambda scopes (innermost last). When non-empty, the eval locals are not visible
    /// (a generator expression body runs in its own function scope).
    scopes: Vec<Vec<(Rc<str>, Value)>>,
}

impl<'a> Evaluator<'a> {
    pub fn new(env: &'a dyn Env) -> Self {
        Evaluator { env, scopes: Vec::new() }
    }

    fn lookup(&self, name: &str) -> PyResult<Value> {
        if let Some(scope) = self.scopes.last() {
            for (n, v) in scope.iter().rev() {
                if &**n == name {
                    return Ok(v.clone());
                }
            }
            // Enclosing comprehension scopes are visible as closures.
            for scope in self.scopes.iter().rev().skip(1) {
                for (n, v) in scope.iter().rev() {
                    if &**n == name {
                        return Ok(v.clone());
                    }
                }
            }
        } else if let Some(v) = self.env.local(name) {
            return Ok(v);
        }
        if let Some(v) = self.env.global(name) {
            return Ok(v);
        }
        if name == "math" {
            return Ok(Value::MathModule);
        }
        if MODULE_GLOBALS.contains(&name) {
            return Ok(Value::Opaque(name.into()));
        }
        if let Some(b) = builtins::lookup_builtin(name) {
            return Ok(b);
        }
        Err(PyErr::new("NameError", format!("name '{}' is not defined", name)))
    }

    pub fn eval(&mut self, e: &Expr) -> PyResult<Value> {
        match e {
            Expr::Const(v) => Ok(v.clone()),
            Expr::Name(n) => self.lookup(n),
            Expr::List(items) => Ok(Value::List(Rc::new(self.eval_items(items)?))),
            Expr::Tuple(items) => Ok(Value::Tuple(Rc::new(self.eval_items(items)?))),
            Expr::Set(items) => {
                let mut out: Vec<Value> = Vec::new();
                for v in self.eval_items(items)? {
                    if !out.iter().any(|x| x.py_eq(&v)) {
                        out.push(v);
                    }
                }
                Ok(Value::Set(Rc::new(out)))
            }
            Expr::Dict(items) => {
                let mut out: Vec<(Value, Value)> = Vec::new();
                for (k, v) in items {
                    let k = self.eval(k)?;
                    let v = self.eval(v)?;
                    if let Some(slot) = out.iter_mut().find(|(x, _)| x.py_eq(&k)) {
                        slot.1 = v;
                    } else {
                        out.push((k, v));
                    }
                }
                Ok(Value::Dict(Rc::new(out)))
            }
            Expr::BinOp(a, op, b) => {
                let a = self.eval(a)?;
                let b = self.eval(b)?;
                binop(&a, *op, &b)
            }
            Expr::UnaryOp(op, a) => {
                let a = self.eval(a)?;
                unaryop(*op, &a)
            }
            Expr::BoolOp(is_and, items) => {
                let mut last = Value::None;
                for (i, item) in items.iter().enumerate() {
                    last = self.eval(item)?;
                    if i + 1 < items.len() && (last.truthy() != *is_and) {
                        return Ok(last);
                    }
                }
                Ok(last)
            }
            Expr::Compare(first, ops) => {
                let mut left = self.eval(first)?;
                for (op, right) in ops {
                    let right = self.eval(right)?;
                    if !compare(&left, *op, &right)? {
                        return Ok(Value::Bool(false));
                    }
                    left = right;
                }
                Ok(Value::Bool(true))
            }
            Expr::IfExp(test, body, orelse) => {
                if self.eval(test)?.truthy() {
                    self.eval(body)
                } else {
                    self.eval(orelse)
                }
            }
            Expr::Call(func, args, kwargs) => {
                let f = self.eval(func)?;
                let mut argv = Vec::with_capacity(args.len());
                for a in args {
                    if let Expr::Starred(inner) = a {
                        argv.extend(self.eval(inner)?.iter_items()?);
                    } else {
                        argv.push(self.eval(a)?);
                    }
                }
                let mut kw = Vec::with_capacity(kwargs.len());
                for (k, v) in kwargs {
                    kw.push((k.clone(), self.eval(v)?));
                }
                self.call(&f, &argv, &kw)
            }
            Expr::Starred(_) => Err(PyErr::new("SyntaxError", "can't use starred expression here")),
            Expr::Attribute(obj, attr) => {
                let o = self.eval(obj)?;
                builtins::get_attribute(&o, attr)
            }
            Expr::Subscript(obj, idx) => {
                let o = self.eval(obj)?;
                let i = self.eval(idx)?;
                subscript(&o, &i)
            }
            Expr::Slice(..) => Err(PyErr::type_err("slices are not supported")),
            Expr::GeneratorExp(elt, gens) | Expr::ListComp(elt, gens) | Expr::SetComp(elt, gens) => {
                let items = self.comprehension(elt, None, gens)?;
                match e {
                    Expr::SetComp(..) => {
                        let mut out: Vec<Value> = Vec::new();
                        for v in items {
                            if !out.iter().any(|x| x.py_eq(&v)) {
                                out.push(v);
                            }
                        }
                        Ok(Value::Set(Rc::new(out)))
                    }
                    _ => Ok(Value::List(Rc::new(items))),
                }
            }
            Expr::DictComp(k, v, gens) => {
                let items = self.comprehension(k, Some(v), gens)?;
                let mut out: Vec<(Value, Value)> = Vec::new();
                for pair in items {
                    if let Value::Tuple(t) = pair {
                        if let Some(slot) = out.iter_mut().find(|(x, _)| x.py_eq(&t[0])) {
                            slot.1 = t[1].clone();
                        } else {
                            out.push((t[0].clone(), t[1].clone()));
                        }
                    }
                }
                Ok(Value::Dict(Rc::new(out)))
            }
            Expr::Lambda(params, body) => Ok(Value::Lambda(Rc::new((params.clone(), (**body).clone())))),
        }
    }

    fn eval_items(&mut self, items: &[Expr]) -> PyResult<Vec<Value>> {
        let mut out = Vec::with_capacity(items.len());
        for i in items {
            if let Expr::Starred(inner) = i {
                out.extend(self.eval(inner)?.iter_items()?);
            } else {
                out.push(self.eval(i)?);
            }
        }
        Ok(out)
    }

    fn bind(scope: &mut Vec<(Rc<str>, Value)>, target: &Target, value: Value) -> PyResult<()> {
        match target {
            Target::Name(n) => {
                scope.push((n.clone(), value));
                Ok(())
            }
            Target::Tuple(ts) => {
                let items = value.iter_items()?;
                if items.len() != ts.len() {
                    return Err(PyErr::value_err("unpack mismatch"));
                }
                for (t, v) in ts.iter().zip(items) {
                    Self::bind(scope, t, v)?;
                }
                Ok(())
            }
        }
    }

    /// Evaluate a comprehension. The first iterable is evaluated in the enclosing scope; everything
    /// else runs in a new function scope.
    fn comprehension(&mut self, elt: &Expr, value: Option<&Expr>, gens: &[Comprehension]) -> PyResult<Vec<Value>> {
        let first_iter = self.eval(&gens[0].iter)?.iter_items()?;
        self.scopes.push(Vec::new());
        let mut out = Vec::new();
        let r = self.comprehension_level(elt, value, gens, 0, Some(first_iter), &mut out);
        self.scopes.pop();
        r.map(|_| out)
    }

    fn comprehension_level(
        &mut self,
        elt: &Expr,
        value: Option<&Expr>,
        gens: &[Comprehension],
        level: usize,
        items: Option<Vec<Value>>,
        out: &mut Vec<Value>,
    ) -> PyResult<()> {
        let g = &gens[level];
        let items = match items {
            Some(i) => i,
            None => self.eval(&g.iter)?.iter_items()?,
        };
        'items: for item in items {
            let mark = self.scopes.last().unwrap().len();
            Self::bind(self.scopes.last_mut().unwrap(), &g.target, item)?;
            for cond in &g.ifs {
                if !self.eval(cond)?.truthy() {
                    self.scopes.last_mut().unwrap().truncate(mark);
                    continue 'items;
                }
            }
            if level + 1 < gens.len() {
                self.comprehension_level(elt, value, gens, level + 1, None, out)?;
            } else {
                let k = self.eval(elt)?;
                match value {
                    Some(v) => {
                        let v = self.eval(v)?;
                        out.push(Value::Tuple(Rc::new(vec![k, v])));
                    }
                    None => out.push(k),
                }
            }
            self.scopes.last_mut().unwrap().truncate(mark);
        }
        Ok(())
    }

    /// Call a callable value.
    pub fn call(&mut self, f: &Value, args: &[Value], kwargs: &[(Rc<str>, Value)]) -> PyResult<Value> {
        match f {
            Value::Builtin(b) => builtins::call_builtin(self, *b, args, kwargs),
            Value::Math(m) => {
                if !kwargs.is_empty() {
                    return Err(PyErr::type_err("math functions take no keyword arguments"));
                }
                builtins::call_math(*m, args)
            }
            Value::Method(obj, name) => {
                if !kwargs.is_empty() {
                    return Err(PyErr::type_err("method takes no keyword arguments"));
                }
                builtins::call_method(obj, name, args)
            }
            Value::Operator(name) => self.env.call_operator(name, args, kwargs),
            Value::Lambda(l) => {
                let (params, body) = &**l;
                if args.len() != params.len() || !kwargs.is_empty() {
                    return Err(PyErr::type_err("<lambda>() argument mismatch"));
                }
                let scope: Vec<(Rc<str>, Value)> = params.iter().cloned().zip(args.iter().cloned()).collect();
                self.scopes.push(scope);
                let r = self.eval(body);
                self.scopes.pop();
                r
            }
            _ => Err(PyErr::type_err(format!("'{}' object is not callable", f.type_name()))),
        }
    }
}

fn int_overflow() -> PyErr {
    PyErr::new("OverflowError", "integer overflow (curust ints are 64-bit)")
}

/// CPython float floor-division / modulo (`_float_div_mod`).
pub fn float_divmod(vx: f64, wx: f64) -> PyResult<(f64, f64)> {
    if wx == 0.0 {
        return Err(PyErr::new("ZeroDivisionError", "float divmod()"));
    }
    let mut m = vx % wx;
    let mut div = (vx - m) / wx;
    if m != 0.0 {
        if (wx < 0.0) != (m < 0.0) {
            m += wx;
            div -= 1.0;
        }
    } else {
        m = 0.0f64.copysign(wx);
    }
    let floordiv = if div != 0.0 {
        let mut fd = div.floor();
        if div - fd > 0.5 {
            fd += 1.0;
        }
        fd
    } else {
        0.0f64.copysign(vx / wx)
    };
    Ok((floordiv, m))
}

pub fn int_divmod(a: i64, b: i64) -> PyResult<(i64, i64)> {
    if b == 0 {
        return Err(PyErr::new("ZeroDivisionError", "integer division or modulo by zero"));
    }
    let mut q = a.checked_div(b).ok_or_else(int_overflow)?;
    let mut r = a % b;
    if r != 0 && ((r < 0) != (b < 0)) {
        q -= 1;
        r += b;
    }
    Ok((q, r))
}

/// CPython `float ** float` (`float_pow`).
pub fn float_pow(iv: f64, iw: f64) -> PyResult<f64> {
    if iw == 0.0 {
        return Ok(1.0);
    }
    if iv.is_nan() {
        return Ok(if iw == 0.0 { 1.0 } else { iv });
    }
    if iw.is_nan() {
        return Ok(if iv == 1.0 { 1.0 } else { iw });
    }
    if iw.is_infinite() {
        let a = iv.abs();
        return Ok(if a == 1.0 {
            1.0
        } else if (iw > 0.0) == (a > 1.0) {
            f64::INFINITY
        } else {
            0.0
        });
    }
    if iv.is_infinite() {
        let iw_is_odd = iw.rem_euclid(2.0) == 1.0;
        return Ok(if iw > 0.0 {
            if iw_is_odd {
                iv
            } else {
                iv.abs()
            }
        } else if iw_is_odd {
            0.0f64.copysign(iv)
        } else {
            0.0
        });
    }
    if iv == 0.0 {
        let iw_is_odd = iw.rem_euclid(2.0) == 1.0;
        if iw < 0.0 {
            return Err(PyErr::new("ZeroDivisionError", "0.0 cannot be raised to a negative power"));
        }
        return Ok(if iw_is_odd { iv } else { 0.0 });
    }
    let mut negate_result = false;
    let mut iv = iv;
    if iv < 0.0 {
        if iw != iw.floor() {
            // CPython returns a complex number here.
            return Err(PyErr::value_err("negative number cannot be raised to a fractional power"));
        }
        iv = -iv;
        negate_result = iw.rem_euclid(2.0) == 1.0;
    }
    if iv == 1.0 {
        return Ok(if negate_result { -1.0 } else { 1.0 });
    }
    let ix = iv.powf(iw);
    if ix.is_infinite() {
        return Err(PyErr::new("OverflowError", "(34, 'Numerical result out of range')"));
    }
    Ok(if negate_result { -ix } else { ix })
}

fn int_pow(a: i64, b: i64) -> PyResult<Value> {
    if b < 0 {
        if a == 0 {
            return Err(PyErr::new("ZeroDivisionError", "0.0 cannot be raised to a negative power"));
        }
        return float_pow(a as f64, b as f64).map(Value::Float);
    }
    let mut result: i64 = 1;
    let mut base = a;
    let mut e = b as u64;
    while e > 0 {
        if e & 1 == 1 {
            match result.checked_mul(base) {
                Some(r) => result = r,
                None => return Ok(Value::Float(big_pow(a, b))),
            }
        }
        e >>= 1;
        if e > 0 {
            match base.checked_mul(base) {
                Some(r) => base = r,
                None => return Ok(Value::Float(big_pow(a, b))),
            }
        }
    }
    Ok(Value::Int(result))
}

/// Python integers are unbounded; results beyond 64 bits are approximated by floats (they only
/// occur with "infinite" machine limits like `machine_max_feedrate_x ** 2`).
fn big_pow(a: i64, b: i64) -> f64 {
    let mut r = 1.0f64;
    let mut base = a as f64;
    let mut e = b as u64;
    while e > 0 {
        if e & 1 == 1 {
            r *= base;
        }
        e >>= 1;
        if e > 0 {
            base *= base;
        }
    }
    r
}

fn unsupported(op: &str, a: &Value, b: &Value) -> PyErr {
    PyErr::type_err(format!("unsupported operand type(s) for {}: '{}' and '{}'", op, a.type_name(), b.type_name()))
}

fn op_symbol(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::FloorDiv => "//",
        BinOp::Mod => "%",
        BinOp::Pow => "** or pow()",
        BinOp::MatMul => "@",
        BinOp::LShift => "<<",
        BinOp::RShift => ">>",
        BinOp::BitOr => "|",
        BinOp::BitXor => "^",
        BinOp::BitAnd => "&",
    }
}

/// Exactly rounded true division of two integers.
fn int_truediv(a: i64, b: i64) -> PyResult<f64> {
    if b == 0 {
        return Err(PyErr::new("ZeroDivisionError", "division by zero"));
    }
    const LIM: i64 = 1 << 53;
    if a.abs() <= LIM && b.abs() <= LIM {
        return Ok(a as f64 / b as f64);
    }
    // Large operands: divide in 128-bit fixed point and round to nearest even.
    let neg = (a < 0) != (b < 0);
    let (ua, ub) = (a.unsigned_abs() as u128, b.unsigned_abs() as u128);
    // Scale so the quotient has 64+ significant bits.
    let shift = 64u32;
    let q = (ua << shift) / ub;
    let r = (ua << shift) % ub;
    let sticky = (r != 0) as u128;
    let v = (q | sticky) as f64 / (2f64).powi(shift as i32);
    Ok(if neg { -v } else { v })
}

pub fn binop(a: &Value, op: BinOp, b: &Value) -> PyResult<Value> {
    // Numeric operations.
    if let (Some(x), Some(y)) = (a.as_number(), b.as_number()) {
        return match (x, y) {
            (Num::Int(x), Num::Int(y)) => match op {
                BinOp::Add => Ok(x.checked_add(y).map(Value::Int).unwrap_or(Value::Float(x as f64 + y as f64))),
                BinOp::Sub => Ok(x.checked_sub(y).map(Value::Int).unwrap_or(Value::Float(x as f64 - y as f64))),
                BinOp::Mul => Ok(x.checked_mul(y).map(Value::Int).unwrap_or(Value::Float(x as f64 * y as f64))),
                BinOp::Div => int_truediv(x, y).map(Value::Float),
                BinOp::FloorDiv => int_divmod(x, y).map(|(q, _)| Value::Int(q)),
                BinOp::Mod => int_divmod(x, y).map(|(_, r)| Value::Int(r)),
                BinOp::Pow => int_pow(x, y),
                BinOp::LShift => {
                    if y < 0 {
                        Err(PyErr::value_err("negative shift count"))
                    } else if y >= 64 {
                        if x == 0 {
                            Ok(Value::Int(0))
                        } else {
                            Err(int_overflow())
                        }
                    } else {
                        let r = x.checked_shl(y as u32).ok_or_else(int_overflow)?;
                        if (r >> y) != x {
                            Err(int_overflow())
                        } else {
                            Ok(Value::Int(r))
                        }
                    }
                }
                BinOp::RShift => {
                    if y < 0 {
                        Err(PyErr::value_err("negative shift count"))
                    } else {
                        Ok(Value::Int(if y >= 64 { if x < 0 { -1 } else { 0 } } else { x >> y }))
                    }
                }
                BinOp::BitOr => Ok(if matches!((a, b), (Value::Bool(_), Value::Bool(_))) { Value::Bool((x | y) != 0) } else { Value::Int(x | y) }),
                BinOp::BitXor => Ok(if matches!((a, b), (Value::Bool(_), Value::Bool(_))) { Value::Bool((x ^ y) != 0) } else { Value::Int(x ^ y) }),
                BinOp::BitAnd => Ok(if matches!((a, b), (Value::Bool(_), Value::Bool(_))) { Value::Bool((x & y) != 0) } else { Value::Int(x & y) }),
                BinOp::MatMul => Err(unsupported(op_symbol(op), a, b)),
            },
            _ => {
                let (fx, fy) = (x.to_f64(), y.to_f64());
                match op {
                    BinOp::Add => Ok(Value::Float(fx + fy)),
                    BinOp::Sub => Ok(Value::Float(fx - fy)),
                    BinOp::Mul => Ok(Value::Float(fx * fy)),
                    BinOp::Div => {
                        if fy == 0.0 {
                            Err(PyErr::new("ZeroDivisionError", "float division by zero"))
                        } else {
                            Ok(Value::Float(fx / fy))
                        }
                    }
                    BinOp::FloorDiv => float_divmod(fx, fy).map(|(q, _)| Value::Float(q)),
                    BinOp::Mod => {
                        if fy == 0.0 {
                            Err(PyErr::new("ZeroDivisionError", "float modulo"))
                        } else {
                            float_divmod(fx, fy).map(|(_, m)| Value::Float(m))
                        }
                    }
                    BinOp::Pow => float_pow(fx, fy).map(Value::Float),
                    _ => Err(unsupported(op_symbol(op), a, b)),
                }
            }
        };
    }
    match (a, op, b) {
        (Value::Str(x), BinOp::Add, Value::Str(y)) => Ok(Value::Str(format!("{}{}", x, y).into())),
        (Value::List(x), BinOp::Add, Value::List(y)) => {
            let mut v = x.as_ref().clone();
            v.extend(y.iter().cloned());
            Ok(Value::List(Rc::new(v)))
        }
        (Value::Tuple(x), BinOp::Add, Value::Tuple(y)) => {
            let mut v = x.as_ref().clone();
            v.extend(y.iter().cloned());
            Ok(Value::Tuple(Rc::new(v)))
        }
        (Value::Str(_), BinOp::Mul, _) | (Value::List(_), BinOp::Mul, _) | (Value::Tuple(_), BinOp::Mul, _) => repeat(a, b),
        (_, BinOp::Mul, Value::Str(_)) | (_, BinOp::Mul, Value::List(_)) | (_, BinOp::Mul, Value::Tuple(_)) => repeat(b, a),
        (Value::Str(s), BinOp::Mod, _) => {
            if s.contains('%') {
                return Err(PyErr::type_err("printf-style string formatting is not supported"));
            }
            // No conversion specifiers: only a (non-empty) tuple or a non-mapping argument is an error.
            match b {
                Value::Tuple(t) if t.is_empty() => Ok(a.clone()),
                Value::Dict(_) | Value::List(_) => Ok(a.clone()),
                _ => Err(PyErr::type_err("not all arguments converted during string formatting")),
            }
        }
        _ => Err(unsupported(op_symbol(op), a, b)),
    }
}

fn repeat(seq: &Value, n: &Value) -> PyResult<Value> {
    let n = match n {
        Value::Int(i) => *i,
        Value::Bool(b) => *b as i64,
        _ => return Err(PyErr::type_err(format!("can't multiply sequence by non-int of type '{}'", n.type_name()))),
    };
    let n = n.max(0) as usize;
    let len = match seq {
        Value::Str(s) => s.len(),
        Value::List(v) | Value::Tuple(v) => v.len(),
        _ => 0,
    };
    if len.saturating_mul(n) > 100_000_000 {
        return Err(PyErr::new("MemoryError", ""));
    }
    Ok(match seq {
        Value::Str(s) => Value::Str(s.repeat(n).into()),
        Value::List(v) => Value::List(Rc::new(v.iter().cloned().cycle().take(v.len() * n).collect())),
        Value::Tuple(v) => Value::Tuple(Rc::new(v.iter().cloned().cycle().take(v.len() * n).collect())),
        _ => unreachable!(),
    })
}

pub fn unaryop(op: UnOp, a: &Value) -> PyResult<Value> {
    match op {
        UnOp::Not => Ok(Value::Bool(!a.truthy())),
        UnOp::Neg => match a.as_number() {
            Some(Num::Int(i)) => i.checked_neg().map(Value::Int).ok_or_else(int_overflow),
            Some(Num::Float(f)) => Ok(Value::Float(-f)),
            None => Err(PyErr::type_err(format!("bad operand type for unary -: '{}'", a.type_name()))),
        },
        UnOp::Pos => match a.as_number() {
            Some(n) => Ok(n.to_value()),
            None => Err(PyErr::type_err(format!("bad operand type for unary +: '{}'", a.type_name()))),
        },
        UnOp::Invert => match a.as_number() {
            Some(Num::Int(i)) => Ok(Value::Int(!i)),
            _ => Err(PyErr::type_err(format!("bad operand type for unary ~: '{}'", a.type_name()))),
        },
    }
}

pub fn contains(container: &Value, item: &Value) -> PyResult<bool> {
    match container {
        Value::List(v) | Value::Tuple(v) | Value::Set(v) => Ok(v.iter().any(|x| x.py_eq(item))),
        Value::Dict(d) => Ok(d.iter().any(|(k, _)| k.py_eq(item))),
        Value::Str(s) => match item {
            Value::Str(sub) => Ok(s.contains(&**sub)),
            _ => Err(PyErr::type_err(format!("'in <string>' requires string as left operand, not {}", item.type_name()))),
        },
        _ => Err(PyErr::type_err(format!("argument of type '{}' is not iterable", container.type_name()))),
    }
}

fn is_identical(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::None, Value::None) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        // Small ints are cached in CPython.
        (Value::Int(x), Value::Int(y)) => x == y && (-5..=256).contains(x),
        (Value::List(x), Value::List(y)) | (Value::Tuple(x), Value::Tuple(y)) => Rc::ptr_eq(x, y),
        (Value::Str(x), Value::Str(y)) => Rc::ptr_eq(x, y) || x == y,
        (Value::Builtin(x), Value::Builtin(y)) => x == y,
        (Value::MathModule, Value::MathModule) => true,
        _ => false,
    }
}

pub fn compare(a: &Value, op: CmpOp, b: &Value) -> PyResult<bool> {
    match op {
        CmpOp::Eq => Ok(a.py_eq(b)),
        CmpOp::Ne => Ok(!a.py_eq(b)),
        CmpOp::Lt => a.py_lt(b),
        CmpOp::Le => a.py_le(b),
        CmpOp::Gt => a.py_gt(b),
        CmpOp::Ge => a.py_ge(b),
        CmpOp::In => contains(b, a),
        CmpOp::NotIn => contains(b, a).map(|r| !r),
        CmpOp::Is => Ok(is_identical(a, b)),
        CmpOp::IsNot => Ok(!is_identical(a, b)),
    }
}

fn index_of(len: usize, i: &Value) -> PyResult<usize> {
    let i = match i {
        Value::Int(i) => *i,
        Value::Bool(b) => *b as i64,
        _ => return Err(PyErr::type_err(format!("indices must be integers or slices, not {}", i.type_name()))),
    };
    let idx = if i < 0 { i + len as i64 } else { i };
    if idx < 0 || idx >= len as i64 {
        return Err(PyErr::new("IndexError", "index out of range"));
    }
    Ok(idx as usize)
}

pub fn subscript(o: &Value, i: &Value) -> PyResult<Value> {
    match o {
        Value::List(v) | Value::Tuple(v) => Ok(v[index_of(v.len(), i)?].clone()),
        Value::Str(s) => {
            let chars: Vec<char> = s.chars().collect();
            Ok(Value::Str(chars[index_of(chars.len(), i)?].to_string().into()))
        }
        Value::Dict(d) => d.iter().find(|(k, _)| k.py_eq(i)).map(|(_, v)| v.clone()).ok_or_else(|| PyErr::new("KeyError", i.py_repr())),
        _ => Err(PyErr::type_err(format!("'{}' object is not subscriptable", o.type_name()))),
    }
}
