//! Built-in functions, `math` functions and the few methods setting formulas can reach.

use super::eval::{binop, float_divmod, float_pow, int_divmod, Evaluator};
use super::ast::BinOp;
use super::value::{parse_float, parse_int, Builtin, MathFn, Num, PyErr, PyResult, Value};
use std::rc::Rc;

pub fn lookup_builtin(name: &str) -> Option<Value> {
    use Builtin::*;
    let b = match name {
        "abs" => Abs,
        "all" => All,
        "any" => Any,
        "bool" => Bool,
        "divmod" => Divmod,
        "float" => Float,
        "int" => Int,
        "len" => Len,
        "max" => Max,
        "min" => Min,
        "map" => Map,
        "pow" => Pow,
        "round" => Round,
        "str" => Str,
        "sum" => Sum,
        "hash" => Hash,
        "list" => List,
        "tuple" => Tuple,
        "dict" => Dict,
        "range" => Range,
        "sorted" => Sorted,
        "reversed" => Reversed,
        "enumerate" => Enumerate,
        "zip" => Zip,
        "isinstance" => Isinstance,
        "set" => Set,
        "repr" => Repr,
        "chr" => Chr,
        "ord" => Ord,
        _ => {
            if super::BUILTIN_NAMES.contains(&name) {
                Other
            } else {
                return None;
            }
        }
    };
    Some(Value::Builtin(b))
}

fn arg_count(name: &str, args: &[Value], min: usize, max: usize) -> PyResult<()> {
    if args.len() < min || args.len() > max {
        return Err(PyErr::type_err(format!("{}() takes {}..{} arguments ({} given)", name, min, max, args.len())));
    }
    Ok(())
}

fn no_kwargs(name: &str, kwargs: &[(Rc<str>, Value)]) -> PyResult<()> {
    if !kwargs.is_empty() {
        return Err(PyErr::type_err(format!("{}() takes no keyword arguments", name)));
    }
    Ok(())
}

/// `int(x)`.
pub fn to_int(v: &Value) -> PyResult<Value> {
    match v {
        Value::Int(i) => Ok(Value::Int(*i)),
        Value::Bool(b) => Ok(Value::Int(*b as i64)),
        Value::Float(f) => float_to_int(*f),
        Value::Str(s) => parse_int(s).map(Value::Int).ok_or_else(|| PyErr::value_err(format!("invalid literal for int() with base 10: {}", Value::Str(s.clone()).py_repr()))),
        _ => Err(PyErr::type_err(format!("int() argument must be a string, a bytes-like object or a real number, not '{}'", v.type_name()))),
    }
}

pub fn float_to_int(f: f64) -> PyResult<Value> {
    if f.is_nan() {
        return Err(PyErr::value_err("cannot convert float NaN to integer"));
    }
    if f.is_infinite() {
        return Err(PyErr::new("OverflowError", "cannot convert float infinity to integer"));
    }
    let t = f.trunc();
    if !(-9.223372036854775808e18..9.223372036854775808e18).contains(&t) {
        return Err(PyErr::new("OverflowError", "integer too large (curust ints are 64-bit)"));
    }
    Ok(Value::Int(t as i64))
}

/// `float(x)`.
pub fn to_float(v: &Value) -> PyResult<Value> {
    match v {
        Value::Int(i) => Ok(Value::Float(*i as f64)),
        Value::Bool(b) => Ok(Value::Float(*b as i64 as f64)),
        Value::Float(f) => Ok(Value::Float(*f)),
        Value::Str(s) => parse_float(s).map(Value::Float).ok_or_else(|| PyErr::value_err(format!("could not convert string to float: {}", Value::Str(s.clone()).py_repr()))),
        _ => Err(PyErr::type_err(format!("float() argument must be a string or a real number, not '{}'", v.type_name()))),
    }
}

/// Exact decimal expansion of a finite double: (negative, integer digits, fraction digits).
fn exact_decimal(x: f64) -> (bool, String, String) {
    let s = format!("{:.1100}", x.abs());
    let (i, f) = s.split_once('.').unwrap();
    (x.is_sign_negative(), i.to_string(), f.trim_end_matches('0').to_string())
}

/// Round a decimal digit string half-to-even at a power of ten (`ndigits` may be negative).
fn round_decimal(neg: bool, int_part: &str, frac: &str, ndigits: i64) -> f64 {
    // Digits as one string with the decimal point after `int_part.len()` digits.
    let mut digits: Vec<u8> = int_part.bytes().chain(frac.bytes()).map(|c| c - b'0').collect();
    let point = int_part.len() as i64;
    let keep = point + ndigits; // number of leading digits kept
    if keep < 0 {
        return if neg { -0.0 } else { 0.0 };
    }
    let keep = keep as usize;
    if keep >= digits.len() {
        let s = format!("{}{}.{}", if neg { "-" } else { "" }, int_part, frac);
        return s.parse().unwrap();
    }
    let first_dropped = digits[keep];
    let rest_nonzero = digits[keep + 1..].iter().any(|d| *d != 0);
    let last_kept_odd = keep > 0 && digits[keep - 1] % 2 == 1;
    let round_up = first_dropped > 5 || (first_dropped == 5 && (rest_nonzero || last_kept_odd));
    digits.truncate(keep);
    let mut carry_digit = false;
    if round_up {
        let mut i = keep;
        loop {
            if i == 0 {
                carry_digit = true;
                break;
            }
            i -= 1;
            if digits[i] == 9 {
                digits[i] = 0;
            } else {
                digits[i] += 1;
                break;
            }
        }
    }
    let mut s = String::new();
    if neg {
        s.push('-');
    }
    if carry_digit {
        s.push('1');
    }
    if digits.is_empty() && !carry_digit {
        s.push('0');
    }
    for d in &digits {
        s.push((b'0' + d) as char);
    }
    // Scale: the kept digits represent value * 10^(ndigits) shifted.
    let exp = -ndigits;
    s.push_str(&format!("e{}", exp));
    s.parse().unwrap()
}

/// `round(x[, ndigits])`.
pub fn py_round(x: &Value, ndigits: Option<&Value>) -> PyResult<Value> {
    let nd = match ndigits {
        None | Some(Value::None) => None,
        Some(Value::Int(i)) => Some(*i),
        Some(Value::Bool(b)) => Some(*b as i64),
        Some(v) => return Err(PyErr::type_err(format!("'{}' object cannot be interpreted as an integer", v.type_name()))),
    };
    match x.as_number() {
        Some(Num::Int(i)) => match nd {
            None => Ok(Value::Int(i)),
            Some(n) if n >= 0 => Ok(Value::Int(i)),
            Some(n) => {
                if n < -18 {
                    return Ok(Value::Int(0));
                }
                let p = 10i64.pow((-n) as u32);
                let (q, r) = int_divmod(i, p)?;
                let twice = r * 2;
                let q = if twice > p || (twice == p && q % 2 != 0) { q + 1 } else { q };
                q.checked_mul(p).map(Value::Int).ok_or_else(|| PyErr::new("OverflowError", "int too large"))
            }
        },
        Some(Num::Float(f)) => match nd {
            None => {
                let mut r = f.round();
                if (f - r).abs() == 0.5 {
                    r = 2.0 * (f / 2.0).round();
                }
                float_to_int(r)
            }
            Some(n) => {
                if !f.is_finite() {
                    return Ok(Value::Float(f));
                }
                if n > 323 {
                    return Ok(Value::Float(f));
                }
                if n < -308 {
                    return Ok(Value::Float(0.0 * f));
                }
                if n >= 0 {
                    let s = format!("{:.*}", n as usize, f);
                    Ok(Value::Float(s.parse().unwrap()))
                } else {
                    let (neg, ip, fp) = exact_decimal(f);
                    let r = round_decimal(neg, &ip, &fp, n);
                    if r.is_infinite() {
                        return Err(PyErr::new("OverflowError", "rounded value too large to represent"));
                    }
                    Ok(Value::Float(r))
                }
            }
        },
        None => Err(PyErr::type_err(format!("type {} doesn't define __round__ method", x.type_name()))),
    }
}

fn py_abs(v: &Value) -> PyResult<Value> {
    match v.as_number() {
        Some(Num::Int(i)) => i.checked_abs().map(Value::Int).ok_or_else(|| PyErr::new("OverflowError", "int too large")),
        Some(Num::Float(f)) => Ok(Value::Float(f.abs())),
        None => Err(PyErr::type_err(format!("bad operand type for abs(): '{}'", v.type_name()))),
    }
}

/// `sum(iterable, start=0)` with CPython 3.12's fast paths (Neumaier summation for floats).
fn py_sum(items: Vec<Value>, start: Value) -> PyResult<Value> {
    if matches!(start, Value::Str(_)) {
        return Err(PyErr::type_err("sum() can't sum strings [use ''.join(seq) instead]"));
    }
    let mut it = items.into_iter();
    let mut result = start;
    if let Value::Int(mut i_result) = result {
        loop {
            match it.next() {
                None => return Ok(Value::Int(i_result)),
                Some(item) => {
                    let b = match item {
                        Value::Int(b) => Some(b),
                        Value::Bool(b) => Some(b as i64),
                        _ => None,
                    };
                    if let Some(b) = b {
                        if let Some(s) = i_result.checked_add(b) {
                            i_result = s;
                            continue;
                        }
                    }
                    result = binop(&Value::Int(i_result), BinOp::Add, &item)?;
                    break;
                }
            }
        }
    }
    if let Value::Float(mut f_result) = result {
        let mut c = 0.0f64;
        loop {
            match it.next() {
                None => {
                    if c != 0.0 && c.is_finite() {
                        f_result += c;
                    }
                    return Ok(Value::Float(f_result));
                }
                Some(Value::Float(x)) => {
                    let t = f_result + x;
                    if f_result.abs() >= x.abs() {
                        c += (f_result - t) + x;
                    } else {
                        c += (x - t) + f_result;
                    }
                    f_result = t;
                }
                Some(Value::Int(v)) => f_result += v as f64,
                Some(Value::Bool(v)) => f_result += v as i64 as f64,
                Some(item) => {
                    if c != 0.0 && c.is_finite() {
                        f_result += c;
                    }
                    result = binop(&Value::Float(f_result), BinOp::Add, &item)?;
                    break;
                }
            }
        }
    }
    for item in it {
        result = binop(&result, BinOp::Add, &item)?;
    }
    Ok(result)
}

fn min_max(ev: &mut Evaluator, is_max: bool, args: &[Value], kwargs: &[(Rc<str>, Value)]) -> PyResult<Value> {
    let name = if is_max { "max" } else { "min" };
    let mut key: Option<Value> = None;
    let mut default: Option<Value> = None;
    for (k, v) in kwargs {
        match &**k {
            "key" => key = if v.is_none() { None } else { Some(v.clone()) },
            "default" => default = Some(v.clone()),
            _ => return Err(PyErr::type_err(format!("{}() got an unexpected keyword argument '{}'", name, k))),
        }
    }
    let items = match args.len() {
        0 => return Err(PyErr::type_err(format!("{} expected at least 1 argument, got 0", name))),
        1 => args[0].iter_items()?,
        _ => {
            if default.is_some() {
                return Err(PyErr::type_err(format!("Cannot specify a default for {}() with multiple positional arguments", name)));
            }
            args.to_vec()
        }
    };
    let mut best: Option<(Value, Value)> = None;
    for item in items {
        let k = match &key {
            Some(f) => ev.call(f, std::slice::from_ref(&item), &[])?,
            None => item.clone(),
        };
        match &best {
            None => best = Some((k, item)),
            Some((bk, _)) => {
                let better = if is_max { k.py_gt(bk)? } else { k.py_lt(bk)? };
                if better {
                    best = Some((k, item));
                }
            }
        }
    }
    match best {
        Some((_, v)) => Ok(v),
        None => default.ok_or_else(|| PyErr::value_err(format!("{}() iterable argument is empty", name))),
    }
}

pub fn call_builtin(ev: &mut Evaluator, b: Builtin, args: &[Value], kwargs: &[(Rc<str>, Value)]) -> PyResult<Value> {
    use Builtin::*;
    match b {
        Max => min_max(ev, true, args, kwargs),
        Min => min_max(ev, false, args, kwargs),
        Sum => {
            let mut start = Value::Int(0);
            if args.len() == 2 {
                start = args[1].clone();
            }
            for (k, v) in kwargs {
                if &**k == "start" {
                    start = v.clone();
                } else {
                    return Err(PyErr::type_err(format!("sum() got an unexpected keyword argument '{}'", k)));
                }
            }
            arg_count("sum", args, 1, 2)?;
            py_sum(args[0].iter_items()?, start)
        }
        Abs => {
            no_kwargs("abs", kwargs)?;
            arg_count("abs", args, 1, 1)?;
            py_abs(&args[0])
        }
        All => {
            no_kwargs("all", kwargs)?;
            arg_count("all", args, 1, 1)?;
            Ok(Value::Bool(args[0].iter_items()?.iter().all(|v| v.truthy())))
        }
        Any => {
            no_kwargs("any", kwargs)?;
            arg_count("any", args, 1, 1)?;
            Ok(Value::Bool(args[0].iter_items()?.iter().any(|v| v.truthy())))
        }
        Bool => {
            arg_count("bool", args, 0, 1)?;
            Ok(Value::Bool(args.first().map(|v| v.truthy()).unwrap_or(false)))
        }
        Int => {
            arg_count("int", args, 0, 2)?;
            match args.len() {
                0 => Ok(Value::Int(0)),
                1 => to_int(&args[0]),
                _ => match (&args[0], &args[1]) {
                    (Value::Str(s), Value::Int(base)) => {
                        let t = s.trim().to_ascii_lowercase();
                        let t = t.trim_start_matches("0x").trim_start_matches("0o").trim_start_matches("0b");
                        i64::from_str_radix(t, *base as u32).map(Value::Int).map_err(|_| PyErr::value_err("invalid literal for int()"))
                    }
                    _ => Err(PyErr::type_err("int() can't convert non-string with explicit base")),
                },
            }
        }
        Float => {
            arg_count("float", args, 0, 1)?;
            if args.is_empty() {
                return Ok(Value::Float(0.0));
            }
            to_float(&args[0])
        }
        Str => {
            arg_count("str", args, 0, 1)?;
            Ok(Value::Str(args.first().map(|v| v.py_str()).unwrap_or_default().into()))
        }
        Repr => {
            arg_count("repr", args, 1, 1)?;
            Ok(Value::Str(args[0].py_repr().into()))
        }
        Len => {
            arg_count("len", args, 1, 1)?;
            match &args[0] {
                Value::Str(s) => Ok(Value::Int(s.chars().count() as i64)),
                Value::List(v) | Value::Tuple(v) | Value::Set(v) => Ok(Value::Int(v.len() as i64)),
                Value::Dict(d) => Ok(Value::Int(d.len() as i64)),
                v => Err(PyErr::type_err(format!("object of type '{}' has no len()", v.type_name()))),
            }
        }
        Round => {
            let mut nd = args.get(1).cloned();
            for (k, v) in kwargs {
                if &**k == "ndigits" {
                    nd = Some(v.clone());
                } else {
                    return Err(PyErr::type_err(format!("round() got an unexpected keyword argument '{}'", k)));
                }
            }
            arg_count("round", args, 1, 2)?;
            py_round(&args[0], nd.as_ref())
        }
        Pow => {
            arg_count("pow", args, 2, 3)?;
            if args.len() == 3 {
                return Err(PyErr::type_err("3-argument pow() is not supported"));
            }
            binop(&args[0], BinOp::Pow, &args[1])
        }
        Divmod => {
            arg_count("divmod", args, 2, 2)?;
            let q = binop(&args[0], BinOp::FloorDiv, &args[1])?;
            let r = binop(&args[0], BinOp::Mod, &args[1])?;
            Ok(Value::Tuple(Rc::new(vec![q, r])))
        }
        Map => {
            if args.len() < 2 {
                return Err(PyErr::type_err("map() must have at least two arguments."));
            }
            let f = args[0].clone();
            let lists: Vec<Vec<Value>> = args[1..].iter().map(|a| a.iter_items()).collect::<PyResult<_>>()?;
            let n = lists.iter().map(|l| l.len()).min().unwrap_or(0);
            let mut out = Vec::with_capacity(n);
            for i in 0..n {
                let call_args: Vec<Value> = lists.iter().map(|l| l[i].clone()).collect();
                out.push(ev.call(&f, &call_args, &[])?);
            }
            // CPython returns a lazy map object; formulas only ever consume it.
            Ok(Value::List(Rc::new(out)))
        }
        List => Ok(Value::List(Rc::new(args.first().map(|a| a.iter_items()).transpose()?.unwrap_or_default()))),
        Tuple => Ok(Value::Tuple(Rc::new(args.first().map(|a| a.iter_items()).transpose()?.unwrap_or_default()))),
        Range => {
            let ints: Vec<i64> = args.iter().map(|a| match a.as_number() {
                Some(Num::Int(i)) => Ok(i),
                _ => Err(PyErr::type_err("range() integer arguments expected")),
            }).collect::<PyResult<_>>()?;
            let (start, stop, step) = match ints.len() {
                1 => (0, ints[0], 1),
                2 => (ints[0], ints[1], 1),
                3 => (ints[0], ints[1], ints[2]),
                _ => return Err(PyErr::type_err("range expected 1 to 3 arguments")),
            };
            if step == 0 {
                return Err(PyErr::value_err("range() arg 3 must not be zero"));
            }
            let mut out = Vec::new();
            let mut i = start;
            while (step > 0 && i < stop) || (step < 0 && i > stop) {
                out.push(Value::Int(i));
                i += step;
            }
            Ok(Value::List(Rc::new(out)))
        }
        Sorted => {
            let mut items = args.first().ok_or_else(|| PyErr::type_err("sorted expected 1 argument"))?.iter_items()?;
            let mut err = None;
            items.sort_by(|a, b| match a.py_cmp(b) {
                Ok(o) => o,
                Err(e) => {
                    err = Some(e);
                    std::cmp::Ordering::Equal
                }
            });
            match err {
                Some(e) => Err(e),
                None => Ok(Value::List(Rc::new(items))),
            }
        }
        _ => Err(PyErr::type_err(format!("builtin {:?} is not supported in setting formulas", b))),
    }
}

fn math_arg(args: &[Value], name: &str) -> PyResult<f64> {
    if args.len() != 1 {
        return Err(PyErr::type_err(format!("math.{}() takes exactly one argument ({} given)", name, args.len())));
    }
    args[0].as_f64().ok_or_else(|| PyErr::type_err(format!("must be real number, not {}", args[0].type_name())))
}

fn domain_error() -> PyErr {
    PyErr::value_err("math domain error")
}

pub fn call_math(m: MathFn, args: &[Value]) -> PyResult<Value> {
    use MathFn::*;
    match m {
        Ceil | Floor | Trunc => {
            if args.len() != 1 {
                return Err(PyErr::type_err("math function takes exactly one argument"));
            }
            match args[0].as_number() {
                Some(Num::Int(i)) => Ok(Value::Int(i)),
                Some(Num::Float(f)) => float_to_int(match m {
                    Ceil => f.ceil(),
                    Floor => f.floor(),
                    _ => f.trunc(),
                }),
                None => Err(PyErr::type_err(format!("must be real number, not {}", args[0].type_name()))),
            }
        }
        Sqrt => {
            let x = math_arg(args, "sqrt")?;
            if x < 0.0 {
                return Err(domain_error());
            }
            Ok(Value::Float(x.sqrt()))
        }
        Log => {
            if args.is_empty() || args.len() > 2 {
                return Err(PyErr::type_err("log expected 1 or 2 arguments"));
            }
            let x = args[0].as_f64().ok_or_else(|| PyErr::type_err("must be real number"))?;
            let lx = math_log(x)?;
            if args.len() == 2 {
                let b = args[1].as_f64().ok_or_else(|| PyErr::type_err("must be real number"))?;
                let lb = math_log(b)?;
                if lb == 0.0 {
                    return Err(PyErr::new("ZeroDivisionError", "float division by zero"));
                }
                return Ok(Value::Float(lx / lb));
            }
            Ok(Value::Float(lx))
        }
        Log10 => {
            let x = math_arg(args, "log10")?;
            if x <= 0.0 {
                return Err(domain_error());
            }
            Ok(Value::Float(x.log10()))
        }
        Log2 => {
            let x = math_arg(args, "log2")?;
            if x <= 0.0 {
                return Err(domain_error());
            }
            Ok(Value::Float(x.log2()))
        }
        Exp => Ok(Value::Float(math_arg(args, "exp")?.exp())),
        Tan => finite_in(math_arg(args, "tan")?, |x| x.tan()),
        Cos => finite_in(math_arg(args, "cos")?, |x| x.cos()),
        Sin => finite_in(math_arg(args, "sin")?, |x| x.sin()),
        Atan => Ok(Value::Float(math_arg(args, "atan")?.atan())),
        Acos => {
            let x = math_arg(args, "acos")?;
            if !(-1.0..=1.0).contains(&x) {
                return Err(domain_error());
            }
            Ok(Value::Float(x.acos()))
        }
        Asin => {
            let x = math_arg(args, "asin")?;
            if !(-1.0..=1.0).contains(&x) {
                return Err(domain_error());
            }
            Ok(Value::Float(x.asin()))
        }
        Radians => Ok(Value::Float(math_arg(args, "radians")? * (std::f64::consts::PI / 180.0))),
        Degrees => Ok(Value::Float(math_arg(args, "degrees")? * (180.0 / std::f64::consts::PI))),
        Fabs => Ok(Value::Float(math_arg(args, "fabs")?.abs())),
        Atan2 | Hypot | Pow | Copysign | Fmod => {
            if args.len() != 2 {
                return Err(PyErr::type_err("expected 2 arguments"));
            }
            let a = args[0].as_f64().ok_or_else(|| PyErr::type_err("must be real number"))?;
            let b = args[1].as_f64().ok_or_else(|| PyErr::type_err("must be real number"))?;
            Ok(Value::Float(match m {
                Atan2 => a.atan2(b),
                Hypot => a.hypot(b),
                Pow => float_pow(a, b)?,
                Copysign => a.copysign(b),
                _ => {
                    if b == 0.0 {
                        return Err(domain_error());
                    }
                    a % b
                }
            }))
        }
        Isclose => Err(PyErr::type_err("math.isclose is not supported")),
    }
}

fn finite_in(x: f64, f: impl Fn(f64) -> f64) -> PyResult<Value> {
    if x.is_infinite() {
        return Err(domain_error());
    }
    Ok(Value::Float(f(x)))
}

fn math_log(x: f64) -> PyResult<f64> {
    if x.is_nan() {
        return Ok(x);
    }
    if x <= 0.0 {
        return Err(domain_error());
    }
    Ok(x.ln())
}

/// Attribute access (`math.pi`, `math.ceil`, `'x'.lower`, `(1, 2).index`).
pub fn get_attribute(o: &Value, attr: &str) -> PyResult<Value> {
    if let Value::MathModule = o {
        use MathFn::*;
        let f = match attr {
            "pi" => return Ok(Value::Float(std::f64::consts::PI)),
            "e" => return Ok(Value::Float(std::f64::consts::E)),
            "tau" => return Ok(Value::Float(std::f64::consts::TAU)),
            "inf" => return Ok(Value::Float(f64::INFINITY)),
            "nan" => return Ok(Value::Float(f64::NAN)),
            "ceil" => Ceil,
            "floor" => Floor,
            "sqrt" => Sqrt,
            "log" => Log,
            "log10" => Log10,
            "log2" => Log2,
            "exp" => Exp,
            "tan" => Tan,
            "cos" => Cos,
            "sin" => Sin,
            "atan" => Atan,
            "atan2" => Atan2,
            "acos" => Acos,
            "asin" => Asin,
            "radians" => Radians,
            "degrees" => Degrees,
            "fabs" => Fabs,
            "hypot" => Hypot,
            "pow" => Pow,
            "trunc" => Trunc,
            "isclose" => Isclose,
            "copysign" => Copysign,
            "fmod" => Fmod,
            _ => return Err(PyErr::new("AttributeError", format!("module 'math' has no attribute '{}'", attr))),
        };
        return Ok(Value::Math(f));
    }
    let ok = match o {
        Value::Str(_) => ["lower", "upper", "startswith", "endswith", "capitalize", "index", "count", "find", "strip", "split", "replace", "join", "encode"].contains(&attr),
        Value::List(_) | Value::Tuple(_) => ["index", "count"].contains(&attr),
        Value::Dict(_) => ["get", "keys", "values", "items"].contains(&attr),
        _ => false,
    };
    if ok {
        Ok(Value::Method(Rc::new(o.clone()), attr.into()))
    } else {
        Err(PyErr::new("AttributeError", format!("'{}' object has no attribute '{}'", o.type_name(), attr)))
    }
}

pub fn call_method(obj: &Value, name: &str, args: &[Value]) -> PyResult<Value> {
    match (obj, name) {
        (Value::Str(s), "lower") => Ok(Value::Str(s.to_lowercase().into())),
        (Value::Str(s), "upper") => Ok(Value::Str(s.to_uppercase().into())),
        (Value::Str(s), "capitalize") => {
            let mut c = s.chars();
            let r = match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
                None => String::new(),
            };
            Ok(Value::Str(r.into()))
        }
        (Value::Str(s), "strip") => Ok(Value::Str(s.trim().into())),
        (Value::Str(s), "startswith") | (Value::Str(s), "endswith") => {
            let check = |p: &Value| -> PyResult<bool> {
                match p {
                    Value::Str(p) => Ok(if name == "startswith" { s.starts_with(&**p) } else { s.ends_with(&**p) }),
                    _ => Err(PyErr::type_err(format!("{} first arg must be str or a tuple of str", name))),
                }
            };
            match args.first() {
                Some(Value::Tuple(ps)) => {
                    for p in ps.iter() {
                        if check(p)? {
                            return Ok(Value::Bool(true));
                        }
                    }
                    Ok(Value::Bool(false))
                }
                Some(p) => check(p).map(Value::Bool),
                None => Err(PyErr::type_err(format!("{}() takes at least 1 argument", name))),
            }
        }
        (Value::Str(s), "find") | (Value::Str(s), "index") => match args.first() {
            Some(Value::Str(sub)) => match s.find(&**sub) {
                Some(byte_idx) => Ok(Value::Int(s[..byte_idx].chars().count() as i64)),
                None => {
                    if name == "find" {
                        Ok(Value::Int(-1))
                    } else {
                        Err(PyErr::value_err("substring not found"))
                    }
                }
            },
            _ => Err(PyErr::type_err("must be str")),
        },
        (Value::Str(s), "count") => match args.first() {
            Some(Value::Str(sub)) => Ok(Value::Int(if sub.is_empty() { s.chars().count() as i64 + 1 } else { s.matches(&**sub).count() as i64 })),
            _ => Err(PyErr::type_err("must be str")),
        },
        (Value::Str(s), "replace") => match (args.first(), args.get(1)) {
            (Some(Value::Str(a)), Some(Value::Str(b))) => Ok(Value::Str(s.replace(&**a, b).into())),
            _ => Err(PyErr::type_err("replace() arguments must be str")),
        },
        (Value::Str(s), "split") => {
            let parts: Vec<Value> = match args.first() {
                None | Some(Value::None) => s.split_whitespace().map(|p| Value::Str(p.into())).collect(),
                Some(Value::Str(sep)) => s.split(&**sep).map(|p| Value::Str(p.into())).collect(),
                _ => return Err(PyErr::type_err("must be str or None")),
            };
            Ok(Value::List(Rc::new(parts)))
        }
        (Value::Str(s), "join") => {
            let items = args.first().ok_or_else(|| PyErr::type_err("join() takes exactly one argument"))?.iter_items()?;
            let mut parts = Vec::new();
            for i in items {
                match i {
                    Value::Str(p) => parts.push(p.to_string()),
                    _ => return Err(PyErr::type_err("sequence item: expected str instance")),
                }
            }
            Ok(Value::Str(parts.join(s).into()))
        }
        (Value::List(v), "index") | (Value::Tuple(v), "index") => {
            let x = args.first().ok_or_else(|| PyErr::type_err("index expected at least 1 argument"))?;
            v.iter().position(|e| e.py_eq(x)).map(|i| Value::Int(i as i64)).ok_or_else(|| {
                if matches!(obj, Value::Tuple(_)) {
                    PyErr::value_err("tuple.index(x): x not in tuple")
                } else {
                    PyErr::value_err(format!("{} is not in list", x.py_repr()))
                }
            })
        }
        (Value::List(v), "count") | (Value::Tuple(v), "count") => {
            let x = args.first().ok_or_else(|| PyErr::type_err("count() takes exactly one argument"))?;
            Ok(Value::Int(v.iter().filter(|e| e.py_eq(x)).count() as i64))
        }
        (Value::Dict(d), "get") => {
            let k = args.first().ok_or_else(|| PyErr::type_err("get expected at least 1 argument"))?;
            Ok(d.iter().find(|(x, _)| x.py_eq(k)).map(|(_, v)| v.clone()).unwrap_or_else(|| args.get(1).cloned().unwrap_or(Value::None)))
        }
        (Value::Dict(d), "keys") => Ok(Value::List(Rc::new(d.iter().map(|(k, _)| k.clone()).collect()))),
        (Value::Dict(d), "values") => Ok(Value::List(Rc::new(d.iter().map(|(_, v)| v.clone()).collect()))),
        (Value::Dict(d), "items") => Ok(Value::List(Rc::new(d.iter().map(|(k, v)| Value::Tuple(Rc::new(vec![k.clone(), v.clone()]))).collect()))),
        _ => Err(PyErr::new("AttributeError", format!("'{}' object has no attribute '{}'", obj.type_name(), name))),
    }
}

/// `float_divmod` re-exported for `divmod` users.
pub fn divmod_floats(a: f64, b: f64) -> PyResult<(f64, f64)> {
    float_divmod(a, b)
}
