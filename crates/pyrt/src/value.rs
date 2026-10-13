//! Runtime values and objects.

use crate::ast::{FuncDef, Name};
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::hash::{BuildHasherDefault, Hasher};
use std::rc::Rc;

/// FNV-1a: a small fast hasher for name maps.
#[derive(Default)]
pub struct Fnv(u64);
impl Hasher for Fnv {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        let mut h = if self.0 == 0 { 0xcbf29ce484222325 } else { self.0 };
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        self.0 = h;
    }
}
pub type FastMap<K, V> = HashMap<K, V, BuildHasherDefault<Fnv>>;
pub type Namespace = FastMap<Name, Value>;

#[derive(Clone)]
pub enum Value {
    None,
    NotImplemented,
    Ellipsis,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Rc<str>),
    Bytes(Rc<Vec<u8>>),
    List(Rc<RefCell<Vec<Value>>>),
    Tuple(Rc<Vec<Value>>),
    Dict(Rc<RefCell<Dict>>),
    Set(Rc<RefCell<Dict>>),
    FrozenSet(Rc<RefCell<Dict>>),
    Func(Rc<Function>),
    Builtin(Rc<Builtin>),
    /// A method of a built-in value: (self, method name).
    Method(Rc<(Value, &'static str)>),
    BoundMethod(Rc<(Value, Value)>),
    Class(Rc<Class>),
    Instance(Rc<Instance>),
    Module(Rc<Module>),
    Range(i64, i64, i64),
    Slice(Rc<(Value, Value, Value)>),
    Iter(Rc<RefCell<PyIter>>),
    StaticMethod(Rc<Value>),
    ClassMethod(Rc<Value>),
    Property(Rc<(Value, Value)>),
    /// `super()` bound to (class it was called in, instance).
    Super(Rc<(Rc<Class>, Value)>),
    Native(Rc<dyn NativeObject>),
}

/// Objects implemented in Rust (regex patterns, matches, arrays, host objects, …).
pub trait NativeObject: Any {
    fn type_name(&self) -> &str;
    fn get_attr(&self, _interp: &mut crate::interp::Interp, _this: &Value, _name: &str) -> Option<crate::interp::R<Value>> {
        None
    }
    fn set_attr(&self, _name: &str, _v: Value) -> bool {
        false
    }
    fn repr(&self) -> String {
        format!("<{} object>", self.type_name())
    }
    fn as_any(&self) -> &dyn Any;
    fn call(&self, _interp: &mut crate::interp::Interp, _args: Vec<Value>, _kw: Vec<(Name, Value)>) -> Option<crate::interp::R<Value>> {
        None
    }
    fn truthy(&self) -> bool {
        true
    }
    fn len(&self) -> Option<usize> {
        None
    }
    fn get_item(&self, _interp: &mut crate::interp::Interp, _k: &Value) -> Option<crate::interp::R<Value>> {
        None
    }
    fn set_item(&self, _interp: &mut crate::interp::Interp, _k: &Value, _v: Value) -> Option<crate::interp::R<()>> {
        None
    }
    fn iter(&self, _interp: &mut crate::interp::Interp) -> Option<crate::interp::R<Vec<Value>>> {
        None
    }
    fn binop(&self, _interp: &mut crate::interp::Interp, _op: crate::ast::BinOp, _other: &Value, _reflected: bool) -> Option<crate::interp::R<Value>> {
        None
    }
    fn equals(&self, _other: &Value) -> Option<bool> {
        None
    }
    /// A method call through `Value::Method((this, name))`.
    fn call_method(&self, _interp: &mut crate::interp::Interp, _this: &Value, _name: &str, _args: Vec<Value>, _kw: Vec<(Name, Value)>) -> Option<crate::interp::R<Value>> {
        None
    }
}

pub enum PyIter {
    Seq(Vec<Value>, usize),
    Range(i64, i64, i64),
    Chars(Rc<str>, usize),
    Done,
}

pub type BuiltinFn = fn(&mut crate::interp::Interp, Vec<Value>, Vec<(Name, Value)>) -> crate::interp::R<Value>;

pub struct Builtin {
    pub name: &'static str,
    pub f: BuiltinFn,
    /// Bound data for closures over Rust state (e.g. a module object).
    pub data: Option<Value>,
}

pub struct Function {
    pub def: Rc<FuncDef>,
    pub defaults: Vec<Value>,
    pub kw_defaults: Vec<Option<Value>>,
    pub globals: Rc<RefCell<Namespace>>,
    /// Enclosing function scopes (innermost first).
    pub closure: Vec<Rc<RefCell<Namespace>>>,
    pub qualname: String,
    /// The class a method was defined in (for `super()`).
    pub class: RefCell<Option<std::rc::Weak<Class>>>,
}

pub struct Class {
    pub name: Name,
    pub bases: Vec<Rc<Class>>,
    pub mro: RefCell<Vec<Rc<Class>>>,
    pub dict: RefCell<Namespace>,
    /// Definition order of the class body's names (for Enum members).
    pub order: RefCell<Vec<Name>>,
    /// The built-in type this class is or derives from ("object", "str", "int", "Exception", …).
    pub builtin: &'static str,
    pub is_enum: bool,
    /// Enum members in definition order.
    pub members: RefCell<Vec<Value>>,
}

pub struct Instance {
    pub class: Rc<Class>,
    pub dict: RefCell<Namespace>,
    /// The built-in value of instances of subclasses of built-in types (str, tuple, int, …)
    /// and the `args` of exceptions.
    pub base: RefCell<Value>,
}

pub struct Module {
    pub name: String,
    pub dict: Rc<RefCell<Namespace>>,
}

/// An insertion-ordered dictionary.
#[derive(Default, Clone)]
pub struct Dict {
    pub entries: Vec<Option<(Value, Value)>>,
    pub index: HashMap<HashKey, usize>,
    pub live: usize,
}

/// A hashable key.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum HashKey {
    None,
    Int(i64),
    /// Floats that are not integral, by bits.
    Float(u64),
    Str(Rc<str>),
    Bytes(Rc<Vec<u8>>),
    Tuple(Vec<HashKey>),
    Id(usize),
}

impl Dict {
    pub fn get(&self, k: &HashKey) -> Option<&Value> {
        self.index.get(k).and_then(|&i| self.entries[i].as_ref().map(|(_, v)| v))
    }
    pub fn insert(&mut self, hk: HashKey, k: Value, v: Value) {
        if let Some(&i) = self.index.get(&hk) {
            if let Some(e) = self.entries[i].as_mut() {
                e.1 = v;
                return;
            }
        }
        self.entries.push(Some((k, v)));
        self.index.insert(hk, self.entries.len() - 1);
        self.live += 1;
    }
    pub fn remove(&mut self, hk: &HashKey) -> Option<Value> {
        let i = self.index.remove(hk)?;
        let e = self.entries[i].take();
        self.live -= 1;
        if self.entries.len() > 32 && self.live * 2 < self.entries.len() {
            self.compact();
        }
        e.map(|(_, v)| v)
    }
    pub fn compact(&mut self) {
        let entries: Vec<(Value, Value)> = self.entries.drain(..).flatten().collect();
        self.index.clear();
        self.live = 0;
        for (k, v) in entries {
            let hk = hash_key(&k).unwrap_or(HashKey::None);
            self.insert(hk, k, v);
        }
    }
    pub fn len(&self) -> usize {
        self.live
    }
    pub fn is_empty(&self) -> bool {
        self.live == 0
    }
    pub fn items(&self) -> impl Iterator<Item = &(Value, Value)> {
        self.entries.iter().flatten()
    }
    pub fn keys(&self) -> Vec<Value> {
        self.items().map(|(k, _)| k.clone()).collect()
    }
    pub fn clear(&mut self) {
        self.entries.clear();
        self.index.clear();
        self.live = 0;
    }
}

/// The hash key of a value (`None` for unhashable values).
pub fn hash_key(v: &Value) -> Option<HashKey> {
    Some(match v {
        Value::None => HashKey::None,
        Value::Bool(b) => HashKey::Int(*b as i64),
        Value::Int(i) => HashKey::Int(*i),
        Value::Float(f) => {
            if f.fract() == 0.0 && f.abs() < 9.2e18 {
                HashKey::Int(*f as i64)
            } else {
                HashKey::Float(f.to_bits())
            }
        }
        Value::Str(s) => HashKey::Str(s.clone()),
        Value::Bytes(b) => HashKey::Bytes(b.clone()),
        Value::Tuple(t) => HashKey::Tuple(t.iter().map(hash_key).collect::<Option<Vec<_>>>()?),
        Value::FrozenSet(d) => HashKey::Id(Rc::as_ptr(d) as *const u8 as usize),
        Value::List(_) | Value::Dict(_) | Value::Set(_) => return None,
        Value::Instance(i) => {
            // Subclasses of str / int / tuple hash like their value; enum members by identity.
            let base = i.base.borrow();
            if !matches!(&*base, Value::None) && !i.class.is_enum {
                return hash_key(&base);
            }
            HashKey::Id(Rc::as_ptr(i) as *const u8 as usize)
        }
        Value::Class(c) => HashKey::Id(Rc::as_ptr(c) as *const u8 as usize),
        Value::Func(f) => HashKey::Id(Rc::as_ptr(f) as *const u8 as usize),
        Value::Builtin(f) => HashKey::Id(Rc::as_ptr(f) as *const u8 as usize),
        Value::Module(m) => HashKey::Id(Rc::as_ptr(m) as *const u8 as usize),
        Value::Native(n) => HashKey::Id(Rc::as_ptr(n) as *const u8 as usize),
        Value::Range(a, b, c) => HashKey::Tuple(vec![HashKey::Int(*a), HashKey::Int(*b), HashKey::Int(*c)]),
        _ => HashKey::Id(0),
    })
}

impl Value {
    pub fn str(s: &str) -> Value {
        Value::Str(Rc::from(s))
    }
    pub fn list(v: Vec<Value>) -> Value {
        Value::List(Rc::new(RefCell::new(v)))
    }
    pub fn tuple(v: Vec<Value>) -> Value {
        Value::Tuple(Rc::new(v))
    }
    pub fn dict(d: Dict) -> Value {
        Value::Dict(Rc::new(RefCell::new(d)))
    }
    pub fn is_none(&self) -> bool {
        matches!(self, Value::None)
    }

    /// The value of an instance of a subclass of a built-in type.
    pub fn unwrap_base(&self) -> Value {
        if let Value::Instance(i) = self {
            let b = i.base.borrow();
            if !matches!(&*b, Value::None) {
                return b.clone();
            }
        }
        self.clone()
    }

    pub fn type_name(&self) -> String {
        match self {
            Value::None => "NoneType".into(),
            Value::NotImplemented => "NotImplementedType".into(),
            Value::Ellipsis => "ellipsis".into(),
            Value::Bool(_) => "bool".into(),
            Value::Int(_) => "int".into(),
            Value::Float(_) => "float".into(),
            Value::Str(_) => "str".into(),
            Value::Bytes(_) => "bytes".into(),
            Value::List(_) => "list".into(),
            Value::Tuple(_) => "tuple".into(),
            Value::Dict(_) => "dict".into(),
            Value::Set(_) => "set".into(),
            Value::FrozenSet(_) => "frozenset".into(),
            Value::Func(_) => "function".into(),
            Value::Builtin(_) | Value::Method(_) => "builtin_function_or_method".into(),
            Value::BoundMethod(_) => "method".into(),
            Value::Class(_) => "type".into(),
            Value::Instance(i) => i.class.name.to_string(),
            Value::Module(_) => "module".into(),
            Value::Range(..) => "range".into(),
            Value::Slice(_) => "slice".into(),
            Value::Iter(_) => "iterator".into(),
            Value::StaticMethod(_) => "staticmethod".into(),
            Value::ClassMethod(_) => "classmethod".into(),
            Value::Property(_) => "property".into(),
            Value::Super(_) => "super".into(),
            Value::Native(n) => n.type_name().to_string(),
        }
    }
}

/// Python's `repr()` of a float.
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
        for _ in 0..-decpt {
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

/// Python's `repr()` of a string.
pub fn str_repr(s: &str) -> String {
    let q = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(q);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == q => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                let _ = write!(out, "\\x{:02x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push(q);
    out
}

pub fn new_class(name: &str, bases: Vec<Rc<Class>>, builtin: &'static str) -> Rc<Class> {
    let c = Rc::new(Class {
        name: name.into(),
        bases,
        mro: RefCell::new(Vec::new()),
        dict: RefCell::new(Namespace::default()),
        order: RefCell::new(Vec::new()),
        builtin,
        is_enum: false,
        members: RefCell::new(Vec::new()),
    });
    let mro = compute_mro(&c);
    *c.mro.borrow_mut() = mro;
    c
}

/// C3 linearization (falling back to depth-first order when inconsistent).
pub fn compute_mro(c: &Rc<Class>) -> Vec<Rc<Class>> {
    let mut seqs: Vec<Vec<Rc<Class>>> = c.bases.iter().map(|b| b.mro.borrow().clone()).collect();
    seqs.push(c.bases.clone());
    let mut out = vec![c.clone()];
    loop {
        seqs.retain(|s| !s.is_empty());
        if seqs.is_empty() {
            return out;
        }
        let mut found = None;
        for s in &seqs {
            let cand = &s[0];
            if !seqs.iter().any(|t| t[1..].iter().any(|x| Rc::ptr_eq(x, cand))) {
                found = Some(cand.clone());
                break;
            }
        }
        let Some(next) = found else {
            // Inconsistent: append the rest depth-first.
            for s in seqs {
                for x in s {
                    if !out.iter().any(|o| Rc::ptr_eq(o, &x)) {
                        out.push(x);
                    }
                }
            }
            return out;
        };
        out.push(next.clone());
        for s in seqs.iter_mut() {
            if Rc::ptr_eq(&s[0], &next) {
                s.remove(0);
            }
        }
    }
}

impl Class {
    pub fn lookup(&self, name: &str) -> Option<Value> {
        for c in self.mro.borrow().iter() {
            if let Some(v) = c.dict.borrow().get(name) {
                return Some(v.clone());
            }
        }
        None
    }
    pub fn is_subclass_of(&self, other: &Rc<Class>) -> bool {
        self.mro.borrow().iter().any(|c| Rc::ptr_eq(c, other))
    }
    pub fn derives_builtin(&self, b: &str) -> bool {
        self.mro.borrow().iter().any(|c| c.builtin == b)
    }
}
