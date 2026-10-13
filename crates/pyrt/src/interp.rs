//! The tree-walking evaluator: statements, expressions, calls, attributes and exceptions.

use crate::ast::*;
use crate::value::*;
use std::cell::RefCell;
use std::rc::Rc;

/// Non-local control flow.
pub enum Flow {
    Exc(Value),
    Return(Value),
    Break,
    Continue,
}

pub type R<T> = Result<T, Flow>;

/// Services of the embedding application.
pub trait Host {
    /// Import a module that is not built in (e.g. `UM.Application`); `None` if unknown.
    fn import(&self, interp: &mut Interp, name: &str) -> Option<R<Value>>;
    fn print(&self, text: &str) {
        eprintln!("{}", text);
    }
}

pub struct Frame {
    pub locals: Rc<RefCell<Namespace>>,
    pub globals: Rc<RefCell<Namespace>>,
    pub closure: Vec<Rc<RefCell<Namespace>>>,
    pub declared_global: Vec<Name>,
    pub declared_nonlocal: Vec<Name>,
    pub is_module: bool,
    pub is_class: bool,
    /// For `super()`: the class of the running method and its first argument.
    pub method_class: Option<Rc<Class>>,
    pub first_arg: Option<Value>,
}

pub struct Types {
    pub object: Rc<Class>,
    pub type_: Rc<Class>,
    pub str_: Rc<Class>,
    pub int: Rc<Class>,
    pub float: Rc<Class>,
    pub bool_: Rc<Class>,
    pub list: Rc<Class>,
    pub tuple: Rc<Class>,
    pub dict: Rc<Class>,
    pub set: Rc<Class>,
    pub bytes: Rc<Class>,
    pub enum_: Rc<Class>,
    pub exceptions: FastMap<&'static str, Rc<Class>>,
}

pub struct Interp {
    pub builtins: Rc<RefCell<Namespace>>,
    pub modules: FastMap<String, Value>,
    pub types: Types,
    pub host: Option<Rc<dyn Host>>,
    /// The exception being handled (for a bare `raise`).
    pub handling: Vec<Value>,
    pub depth: usize,
    /// Source of modules importable by name (scripts), with their package.
    pub sources: FastMap<String, String>,
    /// Interrupt long-running scripts.
    pub deadline: Option<std::time::Instant>,
    pub steps: u64,
}

const EXCEPTIONS: &[(&str, &str)] = &[
    ("BaseException", ""),
    ("SystemExit", "BaseException"),
    ("KeyboardInterrupt", "BaseException"),
    ("Exception", "BaseException"),
    ("ArithmeticError", "Exception"),
    ("ZeroDivisionError", "ArithmeticError"),
    ("OverflowError", "ArithmeticError"),
    ("AssertionError", "Exception"),
    ("AttributeError", "Exception"),
    ("LookupError", "Exception"),
    ("IndexError", "LookupError"),
    ("KeyError", "LookupError"),
    ("NameError", "Exception"),
    ("UnboundLocalError", "NameError"),
    ("RuntimeError", "Exception"),
    ("NotImplementedError", "RuntimeError"),
    ("RecursionError", "RuntimeError"),
    ("StopIteration", "Exception"),
    ("TypeError", "Exception"),
    ("ValueError", "Exception"),
    ("UnicodeError", "ValueError"),
    ("UnicodeDecodeError", "UnicodeError"),
    ("UnicodeEncodeError", "UnicodeError"),
    ("ImportError", "Exception"),
    ("ModuleNotFoundError", "ImportError"),
    ("OSError", "Exception"),
    ("IOError", "OSError"),
    ("FileNotFoundError", "OSError"),
    ("PermissionError", "OSError"),
    ("TimeoutError", "OSError"),
    ("Warning", "Exception"),
    ("UserWarning", "Warning"),
    ("DeprecationWarning", "Warning"),
    ("SyntaxError", "Exception"),
];

impl Interp {
    pub fn new() -> Interp {
        let object = new_class("object", Vec::new(), "object");
        let mk = |n: &str, b: &'static str, base: &Rc<Class>| new_class(n, vec![base.clone()], b);
        let int = mk("int", "int", &object);
        let types = Types {
            type_: mk("type", "type", &object),
            str_: mk("str", "str", &object),
            float: mk("float", "float", &object),
            bool_: mk("bool", "bool", &int),
            list: mk("list", "list", &object),
            tuple: mk("tuple", "tuple", &object),
            dict: mk("dict", "dict", &object),
            set: mk("set", "set", &object),
            bytes: mk("bytes", "bytes", &object),
            enum_: mk("Enum", "Enum", &object),
            int,
            object,
            exceptions: FastMap::default(),
        };
        let mut interp = Interp {
            builtins: Rc::new(RefCell::new(Namespace::default())),
            modules: FastMap::default(),
            types,
            host: None,
            handling: Vec::new(),
            depth: 0,
            sources: FastMap::default(),
            deadline: None,
            steps: 0,
        };
        for (name, base) in EXCEPTIONS {
            let bases = if base.is_empty() { vec![interp.types.object.clone()] } else { vec![interp.types.exceptions[base].clone()] };
            let c = new_class(name, bases, "Exception");
            interp.types.exceptions.insert(name, c);
        }
        crate::builtins::install(&mut interp);
        interp
    }

    // ------------------------------------------------------------------ errors

    pub fn exc_class(&self, name: &str) -> Rc<Class> {
        self.types.exceptions.get(name).cloned().unwrap_or_else(|| self.types.exceptions["Exception"].clone())
    }

    /// An exception instance of a built-in class.
    pub fn exc(&self, class: &str, msg: impl Into<String>) -> Flow {
        let msg: String = msg.into();
        let inst = Instance { class: self.exc_class(class), dict: RefCell::new(Namespace::default()), base: RefCell::new(Value::tuple(vec![Value::str(&msg)])) };
        Flow::Exc(Value::Instance(Rc::new(inst)))
    }

    pub fn type_error<T>(&self, msg: impl Into<String>) -> R<T> {
        Err(self.exc("TypeError", msg))
    }
    pub fn value_error<T>(&self, msg: impl Into<String>) -> R<T> {
        Err(self.exc("ValueError", msg))
    }

    /// The message of an exception value (`str(e)`).
    pub fn exc_message(&mut self, e: &Value) -> String {
        self.to_str(e).unwrap_or_else(|_| "<exception>".into())
    }

    pub fn exc_type_name(&self, e: &Value) -> String {
        match e {
            Value::Instance(i) => i.class.name.to_string(),
            other => other.type_name(),
        }
    }

    fn check_budget(&mut self) -> R<()> {
        self.steps += 1;
        if self.steps & 0xfff == 0 {
            if let Some(d) = self.deadline {
                if std::time::Instant::now() > d {
                    return Err(self.exc("TimeoutError", "the script took too long and was stopped"));
                }
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------ modules

    /// Run module source; returns the module's globals.
    pub fn run_module(&mut self, name: &str, src: &str) -> R<Rc<Module>> {
        let stmts = crate::parser::parse_module(src).map_err(|e| self.exc("SyntaxError", format!("{}: {}", name, e)))?;
        let globals = Rc::new(RefCell::new(Namespace::default()));
        globals.borrow_mut().insert("__name__".into(), Value::str(name));
        let module = Rc::new(Module { name: name.to_string(), dict: globals.clone() });
        self.modules.insert(name.to_string(), Value::Module(module.clone()));
        let mut frame = Frame { locals: globals.clone(), globals, closure: Vec::new(), declared_global: Vec::new(), declared_nonlocal: Vec::new(), is_module: true, is_class: false, method_class: None, first_arg: None };
        match self.exec_block(&stmts, &mut frame) {
            Ok(()) => Ok(module),
            Err(Flow::Exc(e)) => {
                self.modules.remove(name);
                Err(Flow::Exc(e))
            }
            Err(_) => Ok(module),
        }
    }

    pub fn import_module(&mut self, name: &str) -> R<Value> {
        if let Some(m) = self.modules.get(name) {
            return Ok(m.clone());
        }
        if let Some(m) = crate::modules::builtin_module(self, name) {
            let m = m?;
            self.modules.insert(name.to_string(), m.clone());
            return Ok(m);
        }
        if let Some(h) = self.host.clone() {
            if let Some(m) = h.import(self, name) {
                let m = m?;
                self.modules.insert(name.to_string(), m.clone());
                return Ok(m);
            }
        }
        if let Some(src) = self.sources.get(name).cloned() {
            return Ok(Value::Module(self.run_module(name, &src)?));
        }
        Err(self.exc("ModuleNotFoundError", format!("No module named '{}'", name)))
    }

    // ------------------------------------------------------------------ statements

    pub fn exec_block(&mut self, stmts: &[Stmt], f: &mut Frame) -> R<()> {
        for s in stmts {
            self.exec_stmt(s, f)?;
        }
        Ok(())
    }

    fn exec_stmt(&mut self, s: &Stmt, f: &mut Frame) -> R<()> {
        self.check_budget()?;
        let r = self.exec_stmt_inner(s, f);
        if let Err(Flow::Exc(e)) = &r {
            if let Value::Instance(i) = e {
                if !i.dict.borrow().contains_key("__line__") {
                    i.dict.borrow_mut().insert("__line__".into(), Value::Int(s.line as i64));
                }
            }
        }
        r
    }

    fn exec_stmt_inner(&mut self, s: &Stmt, f: &mut Frame) -> R<()> {
        match &s.kind {
            StmtKind::Expr(e) => {
                self.eval(e, f)?;
            }
            StmtKind::Assign(targets, value) => {
                let v = self.eval(value, f)?;
                for t in targets {
                    self.assign(t, v.clone(), f)?;
                }
            }
            StmtKind::AugAssign(target, op, value) => {
                match target {
                    Expr::Name(n) => {
                        let cur = self.load_name(n, f)?;
                        let v = self.eval(value, f)?;
                        let r = self.inplace_op(*op, cur, v)?;
                        self.store_name(n, r, f);
                    }
                    Expr::Attr(obj, name) => {
                        let o = self.eval(obj, f)?;
                        let cur = self.get_attr(&o, name)?;
                        let v = self.eval(value, f)?;
                        let r = self.inplace_op(*op, cur, v)?;
                        self.set_attr(&o, name, r)?;
                    }
                    Expr::Subscript(obj, idx) => {
                        let o = self.eval(obj, f)?;
                        let k = self.eval(idx, f)?;
                        let cur = self.get_item(&o, &k)?;
                        let v = self.eval(value, f)?;
                        let r = self.inplace_op(*op, cur, v)?;
                        self.set_item(&o, k, r)?;
                    }
                    _ => return Err(self.exc("SyntaxError", "illegal expression for augmented assignment")),
                }
            }
            StmtKind::AnnAssign(target, value) => {
                if let Some(v) = value {
                    let v = self.eval(v, f)?;
                    self.assign(target, v, f)?;
                }
            }
            StmtKind::If(branches, orelse) => {
                for (test, body) in branches {
                    let t = self.eval(test, f)?;
                    if self.truthy(&t)? {
                        return self.exec_block(body, f);
                    }
                }
                return self.exec_block(orelse, f);
            }
            StmtKind::While(test, body, orelse) => {
                loop {
                    let t = self.eval(test, f)?;
                    if !self.truthy(&t)? {
                        return self.exec_block(orelse, f);
                    }
                    match self.exec_block(body, f) {
                        Ok(()) | Err(Flow::Continue) => {}
                        Err(Flow::Break) => break,
                        Err(e) => return Err(e),
                    }
                }
            }
            StmtKind::For(target, iter, body, orelse) => {
                let it = self.eval(iter, f)?;
                let mut broke = false;
                let step = |this: &mut Interp, v: Value, f: &mut Frame| -> R<bool> {
                    this.assign(target, v, f)?;
                    match this.exec_block(body, f) {
                        Ok(()) | Err(Flow::Continue) => Ok(true),
                        Err(Flow::Break) => Ok(false),
                        Err(e) => Err(e),
                    }
                };
                match &it {
                    Value::Range(a, b, c) => {
                        let (mut i, end, st) = (*a, *b, *c);
                        while (st > 0 && i < end) || (st < 0 && i > end) {
                            if !step(self, Value::Int(i), f)? {
                                broke = true;
                                break;
                            }
                            i += st;
                        }
                    }
                    Value::List(l) => {
                        // Iterate the live list by index, like CPython.
                        let mut i = 0;
                        loop {
                            let v = match l.borrow().get(i) {
                                Some(v) => v.clone(),
                                None => break,
                            };
                            i += 1;
                            if !step(self, v, f)? {
                                broke = true;
                                break;
                            }
                        }
                    }
                    _ => {
                        let items = self.iterate(&it)?;
                        for v in items {
                            if !step(self, v, f)? {
                                broke = true;
                                break;
                            }
                        }
                    }
                }
                if !broke {
                    self.exec_block(orelse, f)?;
                }
            }
            StmtKind::Break => return Err(Flow::Break),
            StmtKind::Continue => return Err(Flow::Continue),
            StmtKind::Pass => {}
            StmtKind::Return(v) => {
                let v = match v {
                    Some(e) => self.eval(e, f)?,
                    None => Value::None,
                };
                return Err(Flow::Return(v));
            }
            StmtKind::Raise(e, cause) => {
                let Some(e) = e else {
                    return match self.handling.last() {
                        Some(e) => Err(Flow::Exc(e.clone())),
                        None => Err(self.exc("RuntimeError", "No active exception to reraise")),
                    };
                };
                let v = self.eval(e, f)?;
                let exc = self.make_exception(v)?;
                if let Some(c) = cause {
                    let c = self.eval(c, f)?;
                    if let Value::Instance(i) = &exc {
                        i.dict.borrow_mut().insert("__cause__".into(), c);
                    }
                }
                return Err(Flow::Exc(exc));
            }
            StmtKind::Try(body, handlers, orelse, finally) => {
                let r = self.exec_block(body, f);
                let r = match r {
                    Err(Flow::Exc(e)) => {
                        let mut handled = None;
                        for h in handlers {
                            let matches = match &h.typ {
                                None => true,
                                Some(t) => {
                                    let t = self.eval(t, f)?;
                                    self.exception_matches(&e, &t)
                                }
                            };
                            if matches {
                                if let Some(n) = &h.name {
                                    self.store_name(n, e.clone(), f);
                                }
                                self.handling.push(e.clone());
                                let r = self.exec_block(&h.body, f);
                                self.handling.pop();
                                handled = Some(r);
                                break;
                            }
                        }
                        match handled {
                            Some(r) => r,
                            None => Err(Flow::Exc(e)),
                        }
                    }
                    Ok(()) => self.exec_block(orelse, f),
                    other => other,
                };
                if !finally.is_empty() {
                    self.exec_block(finally, f)?;
                }
                return r;
            }
            StmtKind::With(items, body) => {
                let mut exits = Vec::new();
                for (e, t) in items {
                    let mgr = self.eval(e, f)?;
                    let entered = match self.get_attr_opt(&mgr, "__enter__")? {
                        Some(enter) => self.call(&enter, Vec::new(), Vec::new())?,
                        None => mgr.clone(),
                    };
                    if let Some(t) = t {
                        self.assign(t, entered, f)?;
                    }
                    exits.push(mgr);
                }
                let r = self.exec_block(body, f);
                for mgr in exits.iter().rev() {
                    if let Some(exit) = self.get_attr_opt(mgr, "__exit__")? {
                        self.call(&exit, vec![Value::None, Value::None, Value::None], Vec::new())?;
                    }
                }
                return r;
            }
            StmtKind::FuncDef(def) => {
                let func = self.make_function(def.clone(), f)?;
                let mut v = func;
                for d in def.decorators.iter().rev() {
                    let dec = self.eval(d, f)?;
                    v = self.call(&dec, vec![v], Vec::new())?;
                }
                self.store_name(&def.name, v, f);
            }
            StmtKind::ClassDef(def) => {
                let c = self.make_class(def, f)?;
                let mut v = c;
                for d in def.decorators.iter().rev() {
                    let dec = self.eval(d, f)?;
                    v = self.call(&dec, vec![v], Vec::new())?;
                }
                self.store_name(&def.name, v, f);
            }
            StmtKind::Import(items) => {
                for (path, alias) in items {
                    let full: Vec<&str> = path.iter().map(|n| &**n).collect();
                    let full_name = full.join(".");
                    let m = self.import_module(&full_name)?;
                    match alias {
                        Some(a) => self.store_name(a, m, f),
                        None => {
                            let top = self.import_module(full[0])?;
                            self.store_name(&path[0], top, f);
                        }
                    }
                }
            }
            StmtKind::ImportFrom(level, path, names) => {
                let mut name: String = path.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(".");
                if *level > 0 {
                    // Relative imports resolve by the module name alone (scripts import `..Script`).
                    if name.is_empty() {
                        name = "__init__".into();
                    }
                }
                let m = self.import_module(&name)?;
                for (n, alias) in names {
                    if &**n == "*" {
                        if let Value::Module(md) = &m {
                            let items: Vec<(Name, Value)> = md.dict.borrow().iter().filter(|(k, _)| !k.starts_with('_')).map(|(k, v)| (k.clone(), v.clone())).collect();
                            for (k, v) in items {
                                self.store_name(&k, v, f);
                            }
                        }
                        continue;
                    }
                    let v = match self.get_attr_opt(&m, n)? {
                        Some(v) => v,
                        None => match self.import_module(&format!("{}.{}", name, n)) {
                            Ok(v) => v,
                            Err(_) => return Err(self.exc("ImportError", format!("cannot import name '{}' from '{}'", n, name))),
                        },
                    };
                    self.store_name(alias.as_ref().unwrap_or(n), v, f);
                }
            }
            StmtKind::Global(names) => f.declared_global.extend(names.iter().cloned()),
            StmtKind::Nonlocal(names) => f.declared_nonlocal.extend(names.iter().cloned()),
            StmtKind::Del(targets) => {
                for t in targets {
                    match t {
                        Expr::Name(n) => {
                            f.locals.borrow_mut().remove(n);
                        }
                        Expr::Subscript(o, k) => {
                            let o = self.eval(o, f)?;
                            let k = self.eval(k, f)?;
                            self.del_item(&o, &k)?;
                        }
                        Expr::Attr(o, n) => {
                            let o = self.eval(o, f)?;
                            if let Value::Instance(i) = &o {
                                i.dict.borrow_mut().remove(n);
                            }
                        }
                        Expr::Tuple(items) | Expr::List(items) => {
                            for it in items {
                                if let Expr::Name(n) = it {
                                    f.locals.borrow_mut().remove(n);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            StmtKind::Assert(test, msg) => {
                let t = self.eval(test, f)?;
                if !self.truthy(&t)? {
                    let m = match msg {
                        Some(m) => {
                            let v = self.eval(m, f)?;
                            self.to_str(&v)?
                        }
                        None => String::new(),
                    };
                    return Err(self.exc("AssertionError", m));
                }
            }
            StmtKind::Match(subject, cases) => {
                let v = self.eval(subject, f)?;
                for c in cases {
                    if self.match_pattern(&c.pattern, &v, f)? {
                        if let Some(g) = &c.guard {
                            let gv = self.eval(g, f)?;
                            if !self.truthy(&gv)? {
                                continue;
                            }
                        }
                        return self.exec_block(&c.body, f);
                    }
                }
            }
        }
        Ok(())
    }

    fn match_pattern(&mut self, p: &Pattern, v: &Value, f: &mut Frame) -> R<bool> {
        Ok(match p {
            Pattern::Wildcard => true,
            Pattern::Capture(n) => {
                self.store_name(n, v.clone(), f);
                true
            }
            Pattern::Value(e) => {
                let pv = self.eval(e, f)?;
                self.equals(v, &pv)?
            }
            Pattern::Or(ps) => {
                for q in ps {
                    if self.match_pattern(q, v, f)? {
                        return Ok(true);
                    }
                }
                false
            }
            Pattern::As(q, n) => {
                if self.match_pattern(q, v, f)? {
                    self.store_name(n, v.clone(), f);
                    true
                } else {
                    false
                }
            }
            Pattern::Sequence(ps) => {
                let items = match v {
                    Value::List(l) => l.borrow().clone(),
                    Value::Tuple(t) => (**t).clone(),
                    _ => return Ok(false),
                };
                if items.len() != ps.len() {
                    return Ok(false);
                }
                for (q, it) in ps.iter().zip(items.iter()) {
                    if !self.match_pattern(q, it, f)? {
                        return Ok(false);
                    }
                }
                true
            }
        })
    }

    fn make_exception(&mut self, v: Value) -> R<Value> {
        match &v {
            Value::Class(c) => self.call(&Value::Class(c.clone()), Vec::new(), Vec::new()),
            Value::Instance(_) => Ok(v),
            _ => Err(self.exc("TypeError", "exceptions must derive from BaseException")),
        }
    }

    pub fn exception_matches(&self, e: &Value, t: &Value) -> bool {
        match t {
            Value::Tuple(items) => items.iter().any(|x| self.exception_matches(e, x)),
            Value::Class(c) => match e {
                Value::Instance(i) => i.class.is_subclass_of(c),
                _ => false,
            },
            _ => false,
        }
    }

    // ------------------------------------------------------------------ names

    pub fn load_name(&mut self, n: &Name, f: &Frame) -> R<Value> {
        if let Some(v) = f.locals.borrow().get(n) {
            return Ok(v.clone());
        }
        for c in &f.closure {
            if let Some(v) = c.borrow().get(n) {
                return Ok(v.clone());
            }
        }
        if let Some(v) = f.globals.borrow().get(n) {
            return Ok(v.clone());
        }
        if let Some(v) = self.builtins.borrow().get(n) {
            return Ok(v.clone());
        }
        Err(self.exc("NameError", format!("name '{}' is not defined", n)))
    }

    pub fn store_name(&mut self, n: &Name, v: Value, f: &Frame) {
        if !f.is_module && f.declared_global.iter().any(|g| g == n) {
            f.globals.borrow_mut().insert(n.clone(), v);
            return;
        }
        if f.declared_nonlocal.iter().any(|g| g == n) {
            for c in &f.closure {
                if c.borrow().contains_key(n) {
                    c.borrow_mut().insert(n.clone(), v);
                    return;
                }
            }
        }
        f.locals.borrow_mut().insert(n.clone(), v);
    }

    pub fn assign(&mut self, target: &Expr, v: Value, f: &mut Frame) -> R<()> {
        match target {
            Expr::Name(n) => {
                self.store_name(n, v, f);
                Ok(())
            }
            Expr::Attr(obj, name) => {
                let o = self.eval(obj, f)?;
                self.set_attr(&o, name, v)
            }
            Expr::Subscript(obj, idx) => {
                let o = self.eval(obj, f)?;
                let k = self.eval(idx, f)?;
                self.set_item(&o, k, v)
            }
            Expr::Tuple(targets) | Expr::List(targets) => {
                let items = self.iterate(&v)?;
                let star = targets.iter().position(|t| matches!(t, Expr::Starred(_)));
                match star {
                    None => {
                        if items.len() != targets.len() {
                            return if items.len() > targets.len() {
                                self.value_error(format!("too many values to unpack (expected {})", targets.len()))
                            } else {
                                self.value_error(format!("not enough values to unpack (expected {}, got {})", targets.len(), items.len()))
                            };
                        }
                        for (t, it) in targets.iter().zip(items) {
                            self.assign(t, it, f)?;
                        }
                    }
                    Some(si) => {
                        let after = targets.len() - si - 1;
                        if items.len() < si + after {
                            return self.value_error("not enough values to unpack");
                        }
                        for (t, it) in targets[..si].iter().zip(items.iter()) {
                            self.assign(t, it.clone(), f)?;
                        }
                        let mid: Vec<Value> = items[si..items.len() - after].to_vec();
                        if let Expr::Starred(inner) = &targets[si] {
                            self.assign(inner, Value::list(mid), f)?;
                        }
                        for (t, it) in targets[si + 1..].iter().zip(items[items.len() - after..].iter()) {
                            self.assign(t, it.clone(), f)?;
                        }
                    }
                }
                Ok(())
            }
            Expr::Starred(inner) => self.assign(inner, v, f),
            _ => Err(self.exc("SyntaxError", "cannot assign to expression")),
        }
    }

    // ------------------------------------------------------------------ functions & classes

    fn make_function(&mut self, def: Rc<FuncDef>, f: &mut Frame) -> R<Value> {
        let mut defaults = Vec::new();
        for (_, d) in &def.params.args {
            if let Some(d) = d {
                defaults.push(self.eval(d, f)?);
            }
        }
        let mut kw_defaults = Vec::new();
        for (_, d) in &def.params.kwonly {
            kw_defaults.push(match d {
                Some(d) => Some(self.eval(d, f)?),
                None => None,
            });
        }
        // Closure: the enclosing function scopes (a class body is not visible to its methods).
        let mut closure = Vec::new();
        if !f.is_module {
            if !f.is_class {
                closure.push(f.locals.clone());
            }
            closure.extend(f.closure.iter().cloned());
        }
        Ok(Value::Func(Rc::new(Function {
            qualname: def.name.to_string(),
            def,
            defaults,
            kw_defaults,
            globals: f.globals.clone(),
            closure,
            class: RefCell::new(None),
        })))
    }

    fn make_class(&mut self, def: &ClassDef, f: &mut Frame) -> R<Value> {
        let mut bases = Vec::new();
        for b in &def.bases {
            match self.eval(b, f)? {
                Value::Class(c) => bases.push(c),
                Value::Builtin(_) | Value::Native(_) => {}
                other => return self.type_error(format!("cannot inherit from {}", other.type_name())),
            }
        }
        if bases.is_empty() {
            bases.push(self.types.object.clone());
        }
        let ns = Rc::new(RefCell::new(Namespace::default()));
        ns.borrow_mut().insert("__qualname__".into(), Value::Str(def.name.clone()));
        let mut frame = Frame {
            locals: ns.clone(),
            globals: f.globals.clone(),
            closure: if f.is_module { Vec::new() } else { std::iter::once(f.locals.clone()).filter(|_| !f.is_class).chain(f.closure.iter().cloned()).collect() },
            declared_global: Vec::new(),
            declared_nonlocal: Vec::new(),
            is_module: false,
            is_class: true,
            method_class: None,
            first_arg: None,
        };
        // Record definition order.
        let mut order: Vec<Name> = Vec::new();
        for s in &def.body {
            match &s.kind {
                StmtKind::Assign(targets, _) => {
                    for t in targets {
                        if let Expr::Name(n) = t {
                            order.push(n.clone());
                        }
                    }
                }
                StmtKind::AnnAssign(Expr::Name(n), Some(_)) => order.push(n.clone()),
                _ => {}
            }
        }
        self.exec_block(&def.body, &mut frame)?;
        let builtin = bases.iter().find_map(|b| b.mro.borrow().iter().find(|c| c.builtin != "object").map(|c| c.builtin)).unwrap_or("object");
        let is_enum = bases.iter().any(|b| b.derives_builtin("Enum"));
        let class = Rc::new(Class {
            name: def.name.clone(),
            bases,
            mro: RefCell::new(Vec::new()),
            dict: RefCell::new(ns.borrow().clone()),
            order: RefCell::new(order.clone()),
            builtin,
            is_enum,
            members: RefCell::new(Vec::new()),
        });
        *class.mro.borrow_mut() = compute_mro(&class);
        for v in class.dict.borrow().values() {
            let func = match v {
                Value::Func(func) => Some(func.clone()),
                Value::StaticMethod(inner) | Value::ClassMethod(inner) => match &**inner {
                    Value::Func(func) => Some(func.clone()),
                    _ => None,
                },
                Value::Property(p) => match &p.0 {
                    Value::Func(func) => Some(func.clone()),
                    _ => None,
                },
                _ => None,
            };
            if let Some(func) = func {
                *func.class.borrow_mut() = Some(Rc::downgrade(&class));
            }
        }
        if is_enum {
            crate::modules::finish_enum(self, &class, &order)?;
        }
        Ok(Value::Class(class))
    }

    /// Call a value with positional and keyword arguments.
    pub fn call(&mut self, func: &Value, args: Vec<Value>, kw: Vec<(Name, Value)>) -> R<Value> {
        match func {
            Value::Func(fu) => self.call_function(fu, args, kw),
            Value::Builtin(b) => (b.f)(self, args, kw),
            Value::Method(m) => crate::builtins::call_method(self, &m.0, m.1, args, kw),
            Value::BoundMethod(bm) => {
                let mut a = Vec::with_capacity(args.len() + 1);
                a.push(bm.0.clone());
                a.extend(args);
                self.call(&bm.1, a, kw)
            }
            Value::Class(c) => self.instantiate(c, args, kw),
            Value::StaticMethod(inner) => self.call(inner, args, kw),
            Value::Instance(i) => match i.class.lookup("__call__") {
                Some(m) => {
                    let mut a = vec![func.clone()];
                    a.extend(args);
                    self.call(&m, a, kw)
                }
                None => self.type_error(format!("'{}' object is not callable", i.class.name)),
            },
            Value::Native(n) => match n.call(self, args, kw) {
                Some(r) => r,
                None => self.type_error(format!("'{}' object is not callable", n.type_name())),
            },
            other => self.type_error(format!("'{}' object is not callable", other.type_name())),
        }
    }

    pub fn call_function(&mut self, fu: &Rc<Function>, args: Vec<Value>, kw: Vec<(Name, Value)>) -> R<Value> {
        if self.depth > 400 {
            return Err(self.exc("RecursionError", "maximum recursion depth exceeded"));
        }
        let p = &fu.def.params;
        let mut locals = Namespace::default();
        let nargs = p.args.len();
        let mut extra = Vec::new();
        let first_arg = args.first().cloned();
        for (i, a) in args.into_iter().enumerate() {
            if i < nargs {
                locals.insert(p.args[i].0.clone(), a);
            } else {
                extra.push(a);
            }
        }
        if let Some(v) = &p.vararg {
            locals.insert(v.clone(), Value::tuple(std::mem::take(&mut extra)));
        } else if !extra.is_empty() {
            return self.type_error(format!("{}() takes {} positional arguments but {} were given", fu.def.name, nargs, nargs + extra.len()));
        }
        let mut kwargs = Dict::default();
        for (k, v) in kw {
            if p.args.iter().any(|(n, _)| *n == k) || p.kwonly.iter().any(|(n, _)| *n == k) {
                if locals.contains_key(&k) {
                    return self.type_error(format!("{}() got multiple values for argument '{}'", fu.def.name, k));
                }
                locals.insert(k, v);
            } else if p.kwarg.is_some() {
                kwargs.insert(HashKey::Str(k.clone()), Value::Str(k), v);
            } else {
                return self.type_error(format!("{}() got an unexpected keyword argument '{}'", fu.def.name, k));
            }
        }
        // Defaults for the trailing positional parameters.
        let ndef = fu.defaults.len();
        for (i, (n, _)) in p.args.iter().enumerate() {
            if !locals.contains_key(n) {
                if i + ndef >= nargs {
                    locals.insert(n.clone(), fu.defaults[i + ndef - nargs].clone());
                } else {
                    return self.type_error(format!("{}() missing required positional argument: '{}'", fu.def.name, n));
                }
            }
        }
        for (i, (n, _)) in p.kwonly.iter().enumerate() {
            if !locals.contains_key(n) {
                match &fu.kw_defaults[i] {
                    Some(d) => {
                        locals.insert(n.clone(), d.clone());
                    }
                    None => return self.type_error(format!("{}() missing required keyword-only argument: '{}'", fu.def.name, n)),
                }
            }
        }
        if let Some(k) = &p.kwarg {
            locals.insert(k.clone(), Value::dict(kwargs));
        }
        let mut frame = Frame {
            locals: Rc::new(RefCell::new(locals)),
            globals: fu.globals.clone(),
            closure: fu.closure.clone(),
            declared_global: Vec::new(),
            declared_nonlocal: Vec::new(),
            is_module: false,
            is_class: false,
            method_class: fu.class.borrow().as_ref().and_then(|w| w.upgrade()),
            first_arg,
        };
        // Pre-scan `global` / `nonlocal` declarations.
        collect_decls(&fu.def.body, &mut frame.declared_global, &mut frame.declared_nonlocal);
        self.depth += 1;
        let r = self.exec_block(&fu.def.body, &mut frame);
        self.depth -= 1;
        match r {
            Ok(()) => Ok(Value::None),
            Err(Flow::Return(v)) => Ok(v),
            Err(Flow::Break) | Err(Flow::Continue) => Ok(Value::None),
            Err(e) => Err(e),
        }
    }

    pub fn instantiate(&mut self, c: &Rc<Class>, args: Vec<Value>, kw: Vec<(Name, Value)>) -> R<Value> {
        // Built-in types called directly.
        if c.builtin != "object" && c.bases.len() == 1 && Rc::ptr_eq(&c.bases[0], &self.types.object) || Rc::ptr_eq(c, &self.types.bool_) {
            if let Some(r) = crate::builtins::construct_builtin(self, c, &args, &kw) {
                return r;
            }
        }
        if c.is_enum {
            // Lookup by value: Color("red").
            let v = args.first().cloned().unwrap_or(Value::None);
            for m in c.members.borrow().iter() {
                if let Value::Instance(i) = m {
                    let mv = i.dict.borrow().get("_value_").cloned().unwrap_or(Value::None);
                    if self.equals(&mv, &v)? {
                        return Ok(m.clone());
                    }
                }
            }
            let r = self.repr(&v)?;
            return self.value_error(format!("{} is not a valid {}", r, c.name));
        }
        let inst = Rc::new(Instance { class: c.clone(), dict: RefCell::new(Namespace::default()), base: RefCell::new(Value::None) });
        let obj = Value::Instance(inst.clone());
        if c.builtin == "Exception" {
            *inst.base.borrow_mut() = Value::tuple(args.clone());
        } else if c.builtin != "object" && c.builtin != "Enum" {
            let base_class = c.mro.borrow().iter().find(|k| k.builtin == c.builtin && k.bases.len() == 1 && Rc::ptr_eq(&k.bases[0], &self.types.object)).cloned();
            if let Some(bc) = base_class {
                if let Some(r) = crate::builtins::construct_builtin(self, &bc, &args, &kw) {
                    *inst.base.borrow_mut() = r?;
                }
            }
        }
        if let Some(init) = c.lookup("__init__") {
            let mut a = vec![obj.clone()];
            a.extend(args);
            self.call(&init, a, kw)?;
        }
        Ok(obj)
    }

    // ------------------------------------------------------------------ attributes

    pub fn get_attr(&mut self, o: &Value, name: &str) -> R<Value> {
        match self.get_attr_opt(o, name)? {
            Some(v) => Ok(v),
            None => Err(self.exc("AttributeError", format!("'{}' object has no attribute '{}'", o.type_name(), name))),
        }
    }

    fn bind(&mut self, owner: &Value, v: Value, class: &Rc<Class>) -> R<Value> {
        Ok(match v {
            Value::Func(_) | Value::Builtin(_) => Value::BoundMethod(Rc::new((owner.clone(), v))),
            Value::ClassMethod(inner) => Value::BoundMethod(Rc::new((Value::Class(class.clone()), (*inner).clone()))),
            Value::StaticMethod(inner) => (*inner).clone(),
            Value::Property(p) => {
                let getter = p.0.clone();
                self.call(&getter, vec![owner.clone()], Vec::new())?
            }
            other => other,
        })
    }

    pub fn get_attr_opt(&mut self, o: &Value, name: &str) -> R<Option<Value>> {
        match o {
            Value::Instance(i) => {
                if let Some(v) = i.dict.borrow().get(name) {
                    return Ok(Some(v.clone()));
                }
                if name == "__class__" {
                    return Ok(Some(Value::Class(i.class.clone())));
                }
                if name == "__dict__" {
                    let mut d = Dict::default();
                    for (k, v) in i.dict.borrow().iter() {
                        d.insert(HashKey::Str(k.clone()), Value::Str(k.clone()), v.clone());
                    }
                    return Ok(Some(Value::dict(d)));
                }
                if let Some(v) = i.class.lookup(name) {
                    let class = i.class.clone();
                    return Ok(Some(self.bind(o, v, &class)?));
                }
                if i.class.builtin == "Exception" && name == "args" {
                    return Ok(Some(i.base.borrow().clone()));
                }
                // Methods of the built-in base type (str / int / tuple subclasses, enums).
                let base = i.base.borrow().clone();
                if !base.is_none() {
                    if let Some(m) = crate::builtins::method_lookup(&base, name) {
                        return Ok(Some(Value::Method(Rc::new((base, m)))));
                    }
                }
                if let Some(ga) = i.class.lookup("__getattr__") {
                    let r = self.call(&ga, vec![o.clone(), Value::str(name)], Vec::new());
                    return match r {
                        Ok(v) => Ok(Some(v)),
                        Err(Flow::Exc(e)) if self.exc_type_name(&e) == "AttributeError" => Ok(None),
                        Err(e) => Err(e),
                    };
                }
                Ok(None)
            }
            Value::Class(c) => {
                if name == "__name__" {
                    return Ok(Some(Value::Str(c.name.clone())));
                }
                if name == "__mro__" {
                    return Ok(Some(Value::tuple(c.mro.borrow().iter().map(|k| Value::Class(k.clone())).collect())));
                }
                match c.lookup(name) {
                    Some(Value::ClassMethod(inner)) => Ok(Some(Value::BoundMethod(Rc::new((o.clone(), (*inner).clone()))))),
                    Some(Value::StaticMethod(inner)) => Ok(Some((*inner).clone())),
                    Some(v) => Ok(Some(v)),
                    None => Ok(crate::builtins::class_attr(self, c, name)),
                }
            }
            Value::Module(m) => Ok(m.dict.borrow().get(name).cloned()),
            Value::Super(s) => {
                let (cls, obj) = (&s.0, &s.1);
                let owner_class = match obj {
                    Value::Instance(i) => i.class.clone(),
                    Value::Class(c) => c.clone(),
                    _ => cls.clone(),
                };
                let mro = owner_class.mro.borrow().clone();
                let start = mro.iter().position(|c| Rc::ptr_eq(c, cls)).map(|p| p + 1).unwrap_or(mro.len());
                for c in &mro[start..] {
                    if let Some(v) = c.dict.borrow().get(name).cloned() {
                        return Ok(Some(self.bind(obj, v, &owner_class)?));
                    }
                    if c.builtin == "object" && name == "__init__" {
                        return Ok(Some(Value::Builtin(Rc::new(Builtin { name: "__init__", f: |_, _, _| Ok(Value::None), data: None }))));
                    }
                }
                if name == "__init__" {
                    return Ok(Some(Value::Builtin(Rc::new(Builtin { name: "__init__", f: |_, _, _| Ok(Value::None), data: None }))));
                }
                Ok(None)
            }
            Value::Native(n) => match n.get_attr(self, o, name) {
                Some(r) => r.map(Some),
                None => Ok(None),
            },
            Value::Func(fu) => Ok(match name {
                "__name__" => Some(Value::Str(fu.def.name.clone())),
                "__qualname__" => Some(Value::str(&fu.qualname)),
                _ => None,
            }),
            Value::Builtin(b) => Ok(match name {
                "__name__" => Some(Value::str(b.name)),
                _ => None,
            }),
            other => Ok(crate::builtins::value_attr(self, other, name)),
        }
    }

    pub fn set_attr(&mut self, o: &Value, name: &str, v: Value) -> R<()> {
        match o {
            Value::Instance(i) => {
                if let Some(Value::Property(p)) = i.class.lookup(name) {
                    if !p.1.is_none() {
                        let setter = p.1.clone();
                        self.call(&setter, vec![o.clone(), v], Vec::new())?;
                        return Ok(());
                    }
                }
                i.dict.borrow_mut().insert(name.into(), v);
                Ok(())
            }
            Value::Class(c) => {
                c.dict.borrow_mut().insert(name.into(), v);
                Ok(())
            }
            Value::Module(m) => {
                m.dict.borrow_mut().insert(name.into(), v);
                Ok(())
            }
            Value::Native(n) => {
                if n.set_attr(name, v) {
                    Ok(())
                } else {
                    Err(self.exc("AttributeError", format!("cannot set attribute '{}' of '{}'", name, n.type_name())))
                }
            }
            other => Err(self.exc("AttributeError", format!("'{}' object attribute '{}' is read-only", other.type_name(), name))),
        }
    }

    // ------------------------------------------------------------------ expressions

    pub fn eval(&mut self, e: &Expr, f: &mut Frame) -> R<Value> {
        match e {
            Expr::Name(n) => self.load_name(n, f),
            Expr::Const(c) => Ok(match c {
                Const::None => Value::None,
                Const::Bool(b) => Value::Bool(*b),
                Const::Int(i) => Value::Int(*i),
                Const::Float(x) => Value::Float(*x),
                Const::Str(s) => Value::Str(s.clone()),
                Const::Bytes(b) => Value::Bytes(b.clone()),
                Const::Ellipsis => Value::Ellipsis,
            }),
            Expr::FString(parts) => {
                let mut out = String::new();
                self.eval_fparts(parts, f, &mut out)?;
                Ok(Value::Str(out.into()))
            }
            Expr::List(items) => Ok(Value::list(self.eval_items(items, f)?)),
            Expr::Tuple(items) => Ok(Value::tuple(self.eval_items(items, f)?)),
            Expr::Set(items) => {
                let vals = self.eval_items(items, f)?;
                let mut d = Dict::default();
                for v in vals {
                    let hk = self.hash(&v)?;
                    d.insert(hk, v, Value::None);
                }
                Ok(Value::Set(Rc::new(RefCell::new(d))))
            }
            Expr::Dict(items) => {
                let mut d = Dict::default();
                for (k, v) in items {
                    match k {
                        Some(k) => {
                            let kv = self.eval(k, f)?;
                            let vv = self.eval(v, f)?;
                            let hk = self.hash(&kv)?;
                            d.insert(hk, kv, vv);
                        }
                        None => {
                            let m = self.eval(v, f)?;
                            for (k, v) in self.dict_items(&m)? {
                                let hk = self.hash(&k)?;
                                d.insert(hk, k, v);
                            }
                        }
                    }
                }
                Ok(Value::dict(d))
            }
            Expr::Starred(_) => Err(self.exc("SyntaxError", "can't use starred expression here")),
            Expr::Attr(obj, name) => {
                let o = self.eval(obj, f)?;
                self.get_attr(&o, name)
            }
            Expr::Subscript(obj, idx) => {
                let o = self.eval(obj, f)?;
                let k = self.eval(idx, f)?;
                self.get_item(&o, &k)
            }
            Expr::Slice(a, b, c) => {
                let a = match a {
                    Some(x) => self.eval(x, f)?,
                    None => Value::None,
                };
                let b = match b {
                    Some(x) => self.eval(x, f)?,
                    None => Value::None,
                };
                let c = match c {
                    Some(x) => self.eval(x, f)?,
                    None => Value::None,
                };
                Ok(Value::Slice(Rc::new((a, b, c))))
            }
            Expr::Call(func, args) => {
                // `super()` without arguments.
                if let Expr::Name(n) = &**func {
                    if &**n == "super" && args.is_empty() && !f.locals.borrow().contains_key(n) {
                        let cls = f.method_class.clone().ok_or_else(|| self.exc("RuntimeError", "super(): no arguments"))?;
                        let obj = f.first_arg.clone().unwrap_or(Value::None);
                        return Ok(Value::Super(Rc::new((cls, obj))));
                    }
                }
                let fv = self.eval(func, f)?;
                let mut pos = Vec::with_capacity(args.len());
                let mut kw = Vec::new();
                for a in args {
                    match a {
                        Arg::Pos(e) => pos.push(self.eval(e, f)?),
                        Arg::Star(e) => {
                            let v = self.eval(e, f)?;
                            pos.extend(self.iterate(&v)?);
                        }
                        Arg::Kw(n, e) => kw.push((n.clone(), self.eval(e, f)?)),
                        Arg::StarStar(e) => {
                            let m = self.eval(e, f)?;
                            for (k, v) in self.dict_items(&m)? {
                                let ks = self.to_str(&k)?;
                                kw.push((ks.into(), v));
                            }
                        }
                    }
                }
                self.call(&fv, pos, kw)
            }
            Expr::BinOp(a, op, b) => {
                let av = self.eval(a, f)?;
                let bv = self.eval(b, f)?;
                self.binop(*op, &av, &bv)
            }
            Expr::Unary(op, x) => {
                let v = self.eval(x, f)?;
                self.unary(*op, &v)
            }
            Expr::And(a, b) => {
                let av = self.eval(a, f)?;
                if !self.truthy(&av)? {
                    return Ok(av);
                }
                self.eval(b, f)
            }
            Expr::Or(a, b) => {
                let av = self.eval(a, f)?;
                if self.truthy(&av)? {
                    return Ok(av);
                }
                self.eval(b, f)
            }
            Expr::Compare(first, rest) => {
                let mut left = self.eval(first, f)?;
                for (op, e) in rest {
                    let right = self.eval(e, f)?;
                    // NumPy arrays compare element-wise.
                    if rest.len() == 1 && !matches!(op, CmpOp::Is | CmpOp::IsNot | CmpOp::In | CmpOp::NotIn) {
                        let is_arr = |v: &Value| matches!(v, Value::Native(n) if n.as_any().is::<crate::numpy::NdArray>());
                        if is_arr(&left) || is_arr(&right) {
                            return crate::numpy::compare(self, *op, &left, &right);
                        }
                    }
                    if !self.compare(*op, &left, &right)? {
                        return Ok(Value::Bool(false));
                    }
                    left = right;
                }
                Ok(Value::Bool(true))
            }
            Expr::IfExp(test, body, orelse) => {
                let t = self.eval(test, f)?;
                if self.truthy(&t)? {
                    self.eval(body, f)
                } else {
                    self.eval(orelse, f)
                }
            }
            Expr::Lambda(def) => self.make_function(def.clone(), f),
            Expr::ListComp(elt, comps) => {
                let mut out = Vec::new();
                self.comprehension(comps, 0, f, &mut |this, fr| {
                    out.push(this.eval(elt, fr)?);
                    Ok(())
                })?;
                Ok(Value::list(out))
            }
            Expr::GenExp(elt, comps) => {
                let mut out = Vec::new();
                self.comprehension(comps, 0, f, &mut |this, fr| {
                    out.push(this.eval(elt, fr)?);
                    Ok(())
                })?;
                Ok(Value::Iter(Rc::new(RefCell::new(PyIter::Seq(out, 0)))))
            }
            Expr::SetComp(elt, comps) => {
                let mut d = Dict::default();
                self.comprehension(comps, 0, f, &mut |this, fr| {
                    let v = this.eval(elt, fr)?;
                    let hk = this.hash(&v)?;
                    d.insert(hk, v, Value::None);
                    Ok(())
                })?;
                Ok(Value::Set(Rc::new(RefCell::new(d))))
            }
            Expr::DictComp(k, v, comps) => {
                let mut d = Dict::default();
                self.comprehension(comps, 0, f, &mut |this, fr| {
                    let kv = this.eval(k, fr)?;
                    let vv = this.eval(v, fr)?;
                    let hk = this.hash(&kv)?;
                    d.insert(hk, kv, vv);
                    Ok(())
                })?;
                Ok(Value::dict(d))
            }
            Expr::NamedExpr(n, v) => {
                let val = self.eval(v, f)?;
                self.store_name(n, val.clone(), f);
                Ok(val)
            }
        }
    }

    fn eval_items(&mut self, items: &[Expr], f: &mut Frame) -> R<Vec<Value>> {
        let mut out = Vec::with_capacity(items.len());
        for it in items {
            if let Expr::Starred(inner) = it {
                let v = self.eval(inner, f)?;
                out.extend(self.iterate(&v)?);
            } else {
                out.push(self.eval(it, f)?);
            }
        }
        Ok(out)
    }

    fn eval_fparts(&mut self, parts: &[FPartE], f: &mut Frame, out: &mut String) -> R<()> {
        for p in parts {
            match p {
                FPartE::Lit(s) => out.push_str(s),
                FPartE::Expr { expr, conv, spec, debug } => {
                    let v = self.eval(expr, f)?;
                    if let Some(d) = debug {
                        out.push_str(d);
                    }
                    let v = match conv {
                        Some('r') => Value::Str(self.repr(&v)?.into()),
                        Some('s') => Value::Str(self.to_str(&v)?.into()),
                        Some('a') => Value::Str(self.repr(&v)?.into()),
                        _ => v,
                    };
                    let mut spec_s = String::new();
                    self.eval_fparts(spec, f, &mut spec_s)?;
                    if spec_s.is_empty() && debug.is_some() && conv.is_none() {
                        out.push_str(&self.repr(&v)?);
                    } else {
                        out.push_str(&crate::format::format_value(self, &v, &spec_s)?);
                    }
                }
            }
        }
        Ok(())
    }

    /// Run comprehension clauses in a child scope.
    fn comprehension(&mut self, comps: &[Comprehension], i: usize, f: &mut Frame, body: &mut dyn FnMut(&mut Interp, &mut Frame) -> R<()>) -> R<()> {
        if i == 0 {
            let mut child = Frame {
                locals: Rc::new(RefCell::new(Namespace::default())),
                globals: f.globals.clone(),
                closure: if f.is_module { Vec::new() } else { std::iter::once(f.locals.clone()).chain(f.closure.iter().cloned()).collect() },
                declared_global: Vec::new(),
                declared_nonlocal: Vec::new(),
                is_module: false,
                is_class: false,
                method_class: f.method_class.clone(),
                first_arg: f.first_arg.clone(),
            };
            // The first iterable is evaluated in the enclosing scope.
            let it = self.eval(&comps[0].iter, f)?;
            let items = self.iterate(&it)?;
            for v in items {
                self.assign(&comps[0].target, v, &mut child)?;
                if self.comp_ifs(&comps[0], &mut child)? {
                    self.comprehension_rest(comps, 1, &mut child, body)?;
                }
            }
            return Ok(());
        }
        self.comprehension_rest(comps, i, f, body)
    }

    fn comprehension_rest(&mut self, comps: &[Comprehension], i: usize, f: &mut Frame, body: &mut dyn FnMut(&mut Interp, &mut Frame) -> R<()>) -> R<()> {
        if i >= comps.len() {
            return body(self, f);
        }
        let it = self.eval(&comps[i].iter, f)?;
        let items = self.iterate(&it)?;
        for v in items {
            self.assign(&comps[i].target, v, f)?;
            if self.comp_ifs(&comps[i], f)? {
                self.comprehension_rest(comps, i + 1, f, body)?;
            }
        }
        Ok(())
    }

    fn comp_ifs(&mut self, c: &Comprehension, f: &mut Frame) -> R<bool> {
        for cond in &c.ifs {
            let v = self.eval(cond, f)?;
            if !self.truthy(&v)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

impl Default for Interp {
    fn default() -> Self {
        Interp::new()
    }
}

fn collect_decls(stmts: &[Stmt], globals: &mut Vec<Name>, nonlocals: &mut Vec<Name>) {
    for s in stmts {
        match &s.kind {
            StmtKind::Global(n) => globals.extend(n.iter().cloned()),
            StmtKind::Nonlocal(n) => nonlocals.extend(n.iter().cloned()),
            StmtKind::If(branches, orelse) => {
                for (_, b) in branches {
                    collect_decls(b, globals, nonlocals);
                }
                collect_decls(orelse, globals, nonlocals);
            }
            StmtKind::While(_, b, o) | StmtKind::For(_, _, b, o) => {
                collect_decls(b, globals, nonlocals);
                collect_decls(o, globals, nonlocals);
            }
            StmtKind::Try(b, hs, o, fi) => {
                collect_decls(b, globals, nonlocals);
                for h in hs {
                    collect_decls(&h.body, globals, nonlocals);
                }
                collect_decls(o, globals, nonlocals);
                collect_decls(fi, globals, nonlocals);
            }
            StmtKind::With(_, b) => collect_decls(b, globals, nonlocals),
            _ => {}
        }
    }
}
