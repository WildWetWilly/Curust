//! A small Python expression interpreter with CPython 3.12 semantics, as needed to evaluate Cura's
//! setting formulas exactly like Uranium's `SettingFunction` does.

pub mod ast;
pub mod builtins;
pub mod eval;
pub mod lexer;
pub mod value;

pub use eval::{Env, Evaluator};
pub use value::{float_repr, PyErr, PyResult, Value};

use ast::{Comprehension, Expr, Target};
use std::collections::BTreeSet;
use std::rc::Rc;

/// `dir(builtins)` of CPython 3.12.
pub const BUILTIN_NAMES: &[&str] = &[
    "ArithmeticError", "AssertionError", "AttributeError", "BaseException", "BaseExceptionGroup", "BlockingIOError", "BrokenPipeError",
    "BufferError", "BytesWarning", "ChildProcessError", "ConnectionAbortedError", "ConnectionError", "ConnectionRefusedError",
    "ConnectionResetError", "DeprecationWarning", "EOFError", "Ellipsis", "EncodingWarning", "EnvironmentError", "Exception",
    "ExceptionGroup", "False", "FileExistsError", "FileNotFoundError", "FloatingPointError", "FutureWarning", "GeneratorExit", "IOError",
    "ImportError", "ImportWarning", "IndentationError", "IndexError", "InterruptedError", "IsADirectoryError", "KeyError",
    "KeyboardInterrupt", "LookupError", "MemoryError", "ModuleNotFoundError", "NameError", "None", "NotADirectoryError", "NotImplemented",
    "NotImplementedError", "OSError", "OverflowError", "PendingDeprecationWarning", "PermissionError", "ProcessLookupError",
    "RecursionError", "ReferenceError", "ResourceWarning", "RuntimeError", "RuntimeWarning", "StopAsyncIteration", "StopIteration",
    "SyntaxError", "SyntaxWarning", "SystemError", "SystemExit", "TabError", "TimeoutError", "True", "TypeError", "UnboundLocalError",
    "UnicodeDecodeError", "UnicodeEncodeError", "UnicodeError", "UnicodeTranslateError", "UnicodeWarning", "UserWarning", "ValueError",
    "Warning", "ZeroDivisionError", "__build_class__", "__debug__", "__doc__", "__import__", "__loader__", "__name__", "__package__",
    "__spec__", "abs", "aiter", "all", "anext", "any", "ascii", "bin", "bool", "breakpoint", "bytearray", "bytes", "callable", "chr",
    "classmethod", "compile", "complex", "copyright", "credits", "delattr", "dict", "dir", "divmod", "enumerate", "eval", "exec", "exit",
    "filter", "float", "format", "frozenset", "getattr", "globals", "hasattr", "hash", "help", "hex", "id", "input", "int", "isinstance",
    "issubclass", "iter", "len", "license", "list", "locals", "map", "max", "memoryview", "min", "next", "object", "oct", "open", "ord",
    "pow", "print", "property", "quit", "range", "repr", "reversed", "round", "set", "setattr", "slice", "sorted", "staticmethod", "str",
    "sum", "super", "tuple", "type", "vars", "zip",
];

/// `_SettingExpressionVisitor._knownNames` (before operator registration).
const KNOWN_NAMES: &[&str] = &[
    "math", "round", "max", "ceil", "min", "sqrt", "log", "tan", "cos", "sin", "atan", "acos", "asin", "pi", "floor", "debug", "sum", "len",
    "uuid", "hashlib", "base64", "uuid3", "NAMESPACE_DNS", "decode", "encode", "b64encode", "digest", "md5", "radians", "degrees", "lower",
    "upper", "startswith", "endswith", "capitalize", "index",
];

/// The operators Cura registers (`SettingFunction.registerOperator`), which also become known names.
pub const OPERATOR_NAMES: &[&str] = &[
    "extruderValue", "extruderValues", "anyExtruderWithMaterial", "anyExtruderNrWithOrDefault", "resolveOrValue", "defaultExtruderPosition",
    "valueFromContainer", "extruderValueFromContainer",
];

const BLACKLIST: &[&str] = &[
    "sys", "os", "delattr", "getattr", "dir", "open", "write", "compile", "import", "__import__", "__self__", "_", "__", ".", "_._",
    "__enter__", "__builtins__", "eval", "exec", "execfile", "file", "subprocess", "globals", "__class__", "__globals__", "hasattr",
    "raw_input", "input", "reload", "setattr", "vars", "locals", "system", "literal_eval", "ast.literal_eval", "ast", "lambda",
    "__getattribute__", "__setattr__", "find_module", "__pycache__", "__file__",
];

const ALLOWED_BUILTINS: &[&str] = &[
    "bool", "True", "False", "None", "float", "int", "str", "sum", "pow", "abs", "all", "any", "round", "divmod", "hash", "len", "max", "min",
    "map",
];

fn is_known(name: &str) -> bool {
    KNOWN_NAMES.contains(&name) || OPERATOR_NAMES.contains(&name)
}

fn is_blacklisted(name: &str) -> bool {
    BLACKLIST.contains(&name) || (BUILTIN_NAMES.contains(&name) && !ALLOWED_BUILTINS.contains(&name))
}

/// Result of `_SettingExpressionVisitor`: (used values, used keys), or the illegal name.
struct Visitor {
    values: BTreeSet<String>,
    keys: BTreeSet<String>,
}

impl Visitor {
    fn name(&mut self, n: &str) -> Result<(), String> {
        if is_blacklisted(n) {
            return Err(n.to_string());
        }
        if !is_known(n) && !BUILTIN_NAMES.contains(&n) {
            self.values.insert(n.to_string());
            self.keys.insert(n.to_string());
        }
        Ok(())
    }

    fn string(&mut self, s: &str) -> Result<(), String> {
        if BLACKLIST.contains(&s) || (BUILTIN_NAMES.contains(&s) && !ALLOWED_BUILTINS.contains(&s)) {
            return Err(s.to_string());
        }
        if s.starts_with('_') {
            return Err(s.to_string());
        }
        if !is_known(s) && !BUILTIN_NAMES.contains(&s) {
            self.keys.insert(s.to_string());
        }
        Ok(())
    }

    fn target(&mut self, t: &Target) -> Result<(), String> {
        match t {
            Target::Name(n) => self.name(n),
            Target::Tuple(ts) => {
                for t in ts {
                    self.target(t)?;
                }
                Ok(())
            }
        }
    }

    fn comps(&mut self, gens: &[Comprehension]) -> Result<(), String> {
        for g in gens {
            self.target(&g.target)?;
            self.visit(&g.iter)?;
            for i in &g.ifs {
                self.visit(i)?;
            }
        }
        Ok(())
    }

    fn visit(&mut self, e: &Expr) -> Result<(), String> {
        match e {
            Expr::Const(Value::Str(s)) => self.string(s),
            Expr::Const(_) => Ok(()),
            Expr::Name(n) => self.name(n),
            Expr::List(v) | Expr::Tuple(v) | Expr::Set(v) => {
                for x in v {
                    self.visit(x)?;
                }
                Ok(())
            }
            Expr::Dict(v) => {
                for (k, x) in v {
                    self.visit(k)?;
                    self.visit(x)?;
                }
                Ok(())
            }
            Expr::BinOp(a, _, b) => {
                self.visit(a)?;
                self.visit(b)
            }
            Expr::UnaryOp(_, a) | Expr::Starred(a) => self.visit(a),
            Expr::BoolOp(_, v) => {
                for x in v {
                    self.visit(x)?;
                }
                Ok(())
            }
            Expr::Compare(a, ops) => {
                self.visit(a)?;
                for (_, x) in ops {
                    self.visit(x)?;
                }
                Ok(())
            }
            Expr::IfExp(a, b, c) => {
                self.visit(a)?;
                self.visit(b)?;
                self.visit(c)
            }
            Expr::Call(f, args, kwargs) => {
                self.visit(f)?;
                for a in args {
                    self.visit(a)?;
                }
                for (_, v) in kwargs {
                    self.visit(v)?;
                }
                Ok(())
            }
            Expr::Attribute(v, attr) => {
                if !is_known(attr) {
                    return Err(attr.to_string());
                }
                self.visit(v)
            }
            Expr::Subscript(v, i) => {
                if let Expr::Const(Value::Str(_)) = **v {
                    return Err("Indexing on strings is not allowed".into());
                }
                self.visit(v)?;
                self.visit(i)
            }
            Expr::Slice(..) => Err("Slices are not allowed".into()),
            Expr::ListComp(..) => Err("List Comprehension is not allowed".into()),
            Expr::DictComp(..) => Err("Dict Comprehension is not allowed".into()),
            Expr::GeneratorExp(elt, gens) | Expr::SetComp(elt, gens) => {
                self.visit(elt)?;
                self.comps(gens)
            }
            Expr::Lambda(params, body) => {
                let _ = params;
                self.visit(body)
            }
        }
    }
}

/// A compiled setting formula (`UM.Settings.SettingFunction`).
#[derive(Clone, Debug)]
pub struct Function {
    pub code: Rc<str>,
    expr: Option<Rc<Expr>>,
    used_values: Rc<Vec<String>>,
    used_keys: Rc<Vec<String>>,
}

impl PartialEq for Function {
    fn eq(&self, other: &Self) -> bool {
        self.code == other.code
    }
}

impl Function {
    pub fn new(code: &str) -> Function {
        let invalid = |code: &str| Function { code: code.into(), expr: None, used_values: Rc::new(Vec::new()), used_keys: Rc::new(Vec::new()) };
        // `ast.parse(code, "eval")` parses in *exec* mode (the second argument is the file name), so
        // leading indentation is a syntax error even though `compile(..., "eval")` would accept it.
        if code.starts_with([' ', '\t', '\x0c']) {
            return invalid(code);
        }
        let expr = match ast::parse_expression(code) {
            Ok(e) => e,
            Err(_) => return invalid(code),
        };
        let mut v = Visitor { values: BTreeSet::new(), keys: BTreeSet::new() };
        if v.visit(&expr).is_err() {
            return invalid(code);
        }
        Function {
            code: code.into(),
            expr: Some(Rc::new(expr)),
            used_values: Rc::new(v.values.into_iter().collect()),
            used_keys: Rc::new(v.keys.into_iter().collect()),
        }
    }

    pub fn is_valid(&self) -> bool {
        self.expr.is_some()
    }

    /// Names the formula reads as variables (setting values to look up).
    pub fn used_values(&self) -> &[String] {
        &self.used_values
    }

    /// Setting keys the formula depends on (names and string constants).
    pub fn used_keys(&self) -> &[String] {
        &self.used_keys
    }

    /// Evaluate with the given environment; `None` for an invalid function, `0` on any exception
    /// (like Uranium).
    pub fn evaluate(&self, env: &dyn Env) -> Option<Value> {
        let expr = self.expr.as_ref()?;
        let mut ev = Evaluator::new(env);
        match ev.eval(expr) {
            Ok(v) => Some(v),
            Err(_) => Some(Value::Int(0)),
        }
    }

    /// Evaluate, returning the exception instead of `0`.
    pub fn try_evaluate(&self, env: &dyn Env) -> Option<PyResult<Value>> {
        let expr = self.expr.as_ref()?;
        let mut ev = Evaluator::new(env);
        Some(ev.eval(expr))
    }
}

impl std::fmt::Display for Function {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "={}", self.code)
    }
}

/// `ast.literal_eval(text)`.
pub fn literal_eval(text: &str) -> PyResult<Value> {
    // literal_eval strips leading spaces and tabs.
    let t = text.trim_start_matches([' ', '\t']);
    let e = ast::parse_expression(t).map_err(|m| PyErr::new("SyntaxError", m))?;
    literal(&e)
}

fn literal(e: &Expr) -> PyResult<Value> {
    use ast::{BinOp, UnOp};
    match e {
        Expr::Const(v) => Ok(v.clone()),
        Expr::List(items) => Ok(Value::List(Rc::new(items.iter().map(literal).collect::<PyResult<_>>()?))),
        Expr::Tuple(items) => Ok(Value::Tuple(Rc::new(items.iter().map(literal).collect::<PyResult<_>>()?))),
        Expr::Set(items) => Ok(Value::Set(Rc::new(items.iter().map(literal).collect::<PyResult<_>>()?))),
        Expr::Dict(items) => Ok(Value::Dict(Rc::new(items.iter().map(|(k, v)| Ok((literal(k)?, literal(v)?))).collect::<PyResult<_>>()?))),
        Expr::UnaryOp(UnOp::Neg, a) | Expr::UnaryOp(UnOp::Pos, a) => {
            let v = literal(a)?;
            match v {
                Value::Int(_) | Value::Float(_) => eval::unaryop(if matches!(e, Expr::UnaryOp(UnOp::Neg, _)) { UnOp::Neg } else { UnOp::Pos }, &v),
                _ => Err(PyErr::value_err("malformed node or string")),
            }
        }
        Expr::BinOp(a, op @ (BinOp::Add | BinOp::Sub), b) => {
            // Only complex-number literals (`1+2j`) are allowed; we don't support complex numbers.
            let _ = (a, op, b);
            Err(PyErr::value_err("malformed node or string"))
        }
        _ => Err(PyErr::value_err("malformed node or string")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoEnv;
    impl Env for NoEnv {
        fn local(&self, _: &str) -> Option<Value> {
            None
        }
        fn global(&self, _: &str) -> Option<Value> {
            None
        }
        fn call_operator(&self, _: &str, _: &[Value], _: &[(Rc<str>, Value)]) -> PyResult<Value> {
            Err(PyErr::type_err("no operators"))
        }
    }

    fn ev(code: &str) -> String {
        let f = Function::new(code);
        match f.try_evaluate(&NoEnv) {
            None => "<invalid>".into(),
            Some(Ok(v)) => v.py_repr(),
            Some(Err(e)) => format!("<{}>", e.kind),
        }
    }

    #[test]
    fn basics() {
        assert_eq!(ev("1 + 2 * 3"), "7");
        assert_eq!(ev("7 / 2"), "3.5");
        assert_eq!(ev("-7 // 2"), "-4");
        assert_eq!(ev("-7 % 3"), "2");
        assert_eq!(ev("0.15 % 0.1"), "0.04999999999999999");
        assert_eq!(ev("2 ** 10"), "1024");
        assert_eq!(ev("2 ** -1"), "0.5");
        assert_eq!(ev("round(2.5)"), "2");
        assert_eq!(ev("round(0.125, 2)"), "0.12");
        assert_eq!(ev("round(1234.5, -2)"), "1200.0");
        assert_eq!(ev("max([1, 3.0, 3])"), "3.0");
        assert_eq!(ev("min(('raft', 'brim'), key=('raft', 'brim', 'skirt', 'none').index)"), "'raft'");
        assert_eq!(ev("'a' if 1 < 2 < 3 else 'b'"), "'a'");
        assert_eq!(ev("all(x > 0 for x in [1, 2])"), "True");
        assert_eq!(ev("sum([0.1] * 10)"), "1.0");
        assert_eq!(ev("1e16"), "1e+16");
        assert_eq!(ev("1e-5"), "1e-05");
        assert_eq!(ev("0.0001"), "0.0001");
        assert_eq!(ev("math.ceil(2.1)"), "3");
        assert_eq!(ev("[1, (2,), 'x']"), "[1, (2,), 'x']");
        assert_eq!(ev("range(3)"), "<invalid>");
        assert_eq!(ev(" 1"), "<invalid>");
        assert_eq!(ev("[x for x in [1]]"), "<invalid>");
    }
}
