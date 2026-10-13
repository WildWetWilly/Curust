//! Container stacks (`ContainerStack`, `CuraContainerStack`, `GlobalStack`, `ExtruderStack`,
//! per-object stacks) and setting evaluation with Cura's formula functions
//! (`CuraFormulaFunctions`), reproducing Uranium/Cura 5.12 semantics including the evaluation
//! context (`PropertyEvaluationContext`).

use crate::definition::{DefinitionContainer, Prop};
use crate::instance::InstanceContainer;
use crate::py::{Env, Function, PyErr, PyResult, Value};
use crate::registry::Registry;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// Container positions in a Cura stack.
pub mod index {
    pub const USER: usize = 0;
    pub const QUALITY_CHANGES: usize = 1;
    pub const INTENT: usize = 2;
    pub const QUALITY: usize = 3;
    pub const MATERIAL: usize = 4;
    pub const VARIANT: usize = 5;
    pub const DEFINITION_CHANGES: usize = 6;
    pub const DEFINITION: usize = 7;
}

#[derive(Clone, Debug)]
pub enum Container {
    Instance(Rc<InstanceContainer>),
    Definition(Rc<DefinitionContainer>),
}

impl Container {
    pub fn id(&self) -> &str {
        match self {
            Container::Instance(c) => &c.id,
            Container::Definition(d) => &d.id,
        }
    }
    pub fn instance(&self) -> Option<&Rc<InstanceContainer>> {
        match self {
            Container::Instance(c) => Some(c),
            _ => None,
        }
    }
    pub fn meta_str(&self, key: &str) -> Option<String> {
        match self {
            Container::Instance(c) => c.meta_str(key),
            Container::Definition(d) => d.metadata_str(key),
        }
    }
    pub fn name(&self) -> String {
        match self {
            Container::Instance(c) => c.name.clone(),
            Container::Definition(d) => d.name.clone(),
        }
    }
}

/// Which stack of a machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StackId {
    Global,
    /// Index into `Machine::extruders` (all extruder stacks, sorted by position).
    Extruder(usize),
    /// A per-object stack (index into `Machine::object_stacks`).
    Object(usize),
}

#[derive(Debug)]
pub struct Stack {
    pub id: String,
    pub name: String,
    pub containers: Vec<Container>,
    pub metadata: Vec<(String, Value)>,
    settable_per_extruder_cache: RefCell<HashMap<String, Option<Value>>>,
}

impl Stack {
    pub fn new(id: &str, name: &str, containers: Vec<Container>) -> Stack {
        Stack { id: id.into(), name: name.into(), containers, metadata: Vec::new(), settable_per_extruder_cache: RefCell::new(HashMap::new()) }
    }
    pub fn meta(&self, key: &str) -> Option<&Value> {
        self.metadata.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    pub fn set_meta(&mut self, key: &str, v: Value) {
        if let Some(slot) = self.metadata.iter_mut().find(|(k, _)| k == key) {
            slot.1 = v;
        } else {
            self.metadata.push((key.into(), v));
        }
    }
    /// The bottom (definition) container.
    pub fn definition(&self) -> &Rc<DefinitionContainer> {
        match self.containers.last() {
            Some(Container::Definition(d)) => d,
            _ => panic!("stack {} has no definition", self.id),
        }
    }
    pub fn container(&self, i: usize) -> &Container {
        &self.containers[i]
    }
    pub fn instance_at(&self, i: usize) -> &Rc<InstanceContainer> {
        self.containers[i].instance().expect("instance container")
    }
    pub fn user(&self) -> &Rc<InstanceContainer> {
        self.instance_at(index::USER)
    }
    pub fn position(&self) -> Option<String> {
        self.meta("position").map(|v| v.py_str())
    }
    pub fn is_enabled(&self) -> bool {
        match self.meta("enabled") {
            None => true,
            Some(v) => parse_bool(v),
        }
    }
    pub fn clear_caches(&self) {
        self.settable_per_extruder_cache.borrow_mut().clear();
    }
}

/// `UM.Util.parseBool`.
pub fn parse_bool(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Str(s) => matches!(&**s, "True" | "true" | "1" | "yes" | "Yes" | "on" | "On"),
        Value::Int(i) => *i != 0,
        _ => v.truthy(),
    }
}

#[derive(Debug, Default)]
struct CtxData {
    stack: Vec<StackId>,
    extruder_position: Option<Option<Value>>,
    evaluate_from: Option<i64>,
    default_operators: bool,
    original_limit_to_extruder: Option<String>,
}

/// `PropertyEvaluationContext` (shared, mutable).
#[derive(Clone, Debug)]
pub struct Ctx(Rc<RefCell<CtxData>>);

impl Ctx {
    pub fn new(source: Option<StackId>) -> Ctx {
        let mut d = CtxData::default();
        if let Some(s) = source {
            d.stack.push(s);
        }
        Ctx(Rc::new(RefCell::new(d)))
    }
    fn root(&self) -> Option<StackId> {
        self.0.borrow().stack.first().copied()
    }
    fn push(&self, s: StackId) {
        self.0.borrow_mut().stack.push(s);
    }
    fn pop(&self) {
        self.0.borrow_mut().stack.pop();
    }
    fn evaluate_from(&self) -> i64 {
        self.0.borrow().evaluate_from.unwrap_or(0)
    }
    pub fn set_evaluate_from(&self, i: i64) {
        self.0.borrow_mut().evaluate_from = Some(i);
    }
}

fn ctx_push(ctx: Option<&Ctx>, s: StackId) {
    if let Some(c) = ctx {
        c.push(s);
    }
}
fn ctx_pop(ctx: Option<&Ctx>) {
    if let Some(c) = ctx {
        c.pop();
    }
}

/// A configured printer: the global stack, its extruder stacks and the per-object stacks of the
/// scene's models.
pub struct Machine {
    pub reg: Rc<Registry>,
    pub global: Stack,
    pub extruders: Vec<Stack>,
    pub object_stacks: Vec<ObjectStack>,
    resolving: RefCell<HashSet<String>>,
    pub default_extruder_position: RefCell<String>,
}

/// A per-object (per-model) settings stack (`PerObjectContainerStack`): a single user container on
/// top of the stack of the extruder that prints the model.
#[derive(Debug)]
pub struct ObjectStack {
    pub user: Rc<InstanceContainer>,
    pub extruder_position: usize,
}

const OPERATORS: &[&str] = &[
    "extruderValue",
    "extruderValues",
    "anyExtruderWithMaterial",
    "anyExtruderNrWithOrDefault",
    "resolveOrValue",
    "defaultExtruderPosition",
    "valueFromContainer",
    "extruderValueFromContainer",
    "debug",
];

struct FormulaEnv<'a> {
    machine: &'a Machine,
    locals: Vec<(String, Value)>,
    extra: Option<&'a HashMap<String, Value>>,
    default_ops: bool,
}

impl Env for FormulaEnv<'_> {
    fn local(&self, name: &str) -> Option<Value> {
        if let Some(v) = self.extra.and_then(|e| e.get(name)) {
            return Some(v.clone());
        }
        self.locals.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone())
    }
    fn global(&self, name: &str) -> Option<Value> {
        if OPERATORS.contains(&name) {
            Some(Value::Operator(name.into()))
        } else {
            None
        }
    }
    fn call_operator(&self, name: &str, args: &[Value], kwargs: &[(Rc<str>, Value)]) -> PyResult<Value> {
        self.machine.call_operator(name, args, kwargs, self.default_ops)
    }
}

fn arg<'v>(args: &'v [Value], kwargs: &'v [(Rc<str>, Value)], i: usize, name: &str) -> PyResult<&'v Value> {
    if let Some(v) = args.get(i) {
        return Ok(v);
    }
    kwargs.iter().find(|(k, _)| &**k == name).map(|(_, v)| v).ok_or_else(|| PyErr::type_err(format!("missing argument '{}'", name)))
}

/// Python `int(x)` for operator arguments.
fn py_int(v: &Value) -> PyResult<i64> {
    match crate::py::builtins::to_int(v)? {
        Value::Int(i) => Ok(i),
        _ => Err(PyErr::type_err("int expected")),
    }
}

impl Machine {
    pub fn new(reg: Rc<Registry>, global: Stack, extruders: Vec<Stack>) -> Machine {
        Machine { reg, global, extruders, object_stacks: Vec::new(), resolving: RefCell::new(HashSet::new()), default_extruder_position: RefCell::new("0".into()) }
    }

    pub fn stack(&self, s: StackId) -> &Stack {
        match s {
            StackId::Global => &self.global,
            StackId::Extruder(i) => &self.extruders[i],
            StackId::Object(_) => panic!("object stacks have no Stack"),
        }
    }

    /// Positions → extruder index (`GlobalStack._extruders`).
    pub fn extruder_by_position(&self, position: &str) -> Option<usize> {
        self.extruders.iter().position(|e| e.position().as_deref() == Some(position))
    }

    /// `GlobalStack.extruderList`: extruders sorted by position, truncated to `machine_extruder_count`.
    pub fn extruder_list(&self) -> Vec<usize> {
        let mut v: Vec<(i64, usize)> = self.extruders.iter().enumerate().map(|(i, e)| (e.position().and_then(|p| p.parse().ok()).unwrap_or(0), i)).collect();
        v.sort();
        let count = match self.get_property(StackId::Global, "machine_extruder_count", "value", None) {
            Some(Value::Int(i)) => i.max(0) as usize,
            Some(Value::Bool(b)) => b as usize,
            _ => v.len(),
        };
        v.into_iter().map(|(_, i)| i).take(count).collect()
    }

    fn definition_of_instance(&self, c: &InstanceContainer) -> Option<Rc<DefinitionContainer>> {
        self.reg.definition(&c.definition)
    }

    /// `container.getProperty(key, property)` for one container.
    fn container_property(&self, c: &Container, key: &str, prop: &str) -> Option<Prop> {
        match c {
            Container::Definition(d) => d.get_property(key, prop),
            Container::Instance(i) => {
                if prop != "value" || i.is_empty {
                    return None;
                }
                let def = self.definition_of_instance(i);
                i.get_value(key, def.as_deref())
            }
        }
    }

    fn next_stack(&self, s: StackId) -> Option<StackId> {
        match s {
            StackId::Global => None,
            StackId::Extruder(_) => Some(StackId::Global),
            StackId::Object(o) => Some(StackId::Extruder(self.object_stacks[o].extruder_position)),
        }
    }

    /// `ContainerStack.getRawProperty(key, property, context=ctx)`.
    pub fn raw_property(&self, s: StackId, key: &str, prop: &str, ctx: Option<&Ctx>) -> Option<Prop> {
        let start = ctx.map(|c| c.evaluate_from()).unwrap_or(0).max(0) as usize;
        if let StackId::Object(o) = s {
            // A per-object stack: its user container on top of six empty containers and an empty
            // "definition", then the extruder stack.
            if start >= 8 {
                return None;
            }
            if start == 0 && (prop == "value" || prop == "state" || prop == "validationState") {
                if let Some(v) = self.object_user_value(o, key, prop) {
                    return Some(v);
                }
            }
            return self.raw_property(self.next_stack(s).unwrap(), key, prop, ctx);
        }
        let st = self.stack(s);
        if start >= st.containers.len() {
            return None;
        }
        let containers = &st.containers[start..];
        if prop != "value" && prop != "state" && prop != "validationState" {
            if let Some(v) = self.container_property(containers.last().unwrap(), key, prop) {
                return Some(v);
            }
        } else {
            for c in containers {
                if let Some(v) = self.container_property(c, key, prop) {
                    return Some(v);
                }
            }
        }
        match self.next_stack(s) {
            Some(n) => self.raw_property(n, key, prop, ctx),
            None => None,
        }
    }

    /// `ContainerStack.getProperty`.
    fn base_get_property(&self, s: StackId, key: &str, prop: &str, ctx: Option<&Ctx>) -> Option<Value> {
        let v = match self.raw_property(s, key, prop, ctx)? {
            Prop::Value(v) => Some(v),
            Prop::Func(f) => {
                ctx_push(ctx, s);
                let v = self.call_function(&f, s, ctx, None);
                ctx_pop(ctx);
                v
            }
        };
        v.filter(|v| !v.is_none())
    }

    /// `CuraContainerStack.getProperty` (caches `settable_per_extruder`).
    fn cura_get_property(&self, s: StackId, key: &str, prop: &str, ctx: Option<&Ctx>) -> Option<Value> {
        if prop == "settable_per_extruder" && !matches!(s, StackId::Object(_)) {
            if let Some(v) = self.stack(s).settable_per_extruder_cache.borrow().get(key) {
                return v.clone();
            }
            let v = self.base_get_property(s, key, prop, ctx);
            self.stack(s).settable_per_extruder_cache.borrow_mut().insert(key.to_string(), v.clone());
            return v;
        }
        self.base_get_property(s, key, prop, ctx)
    }

    /// `CuraContainerStack.hasUserValue`.
    pub fn has_user_value(&self, s: StackId, key: &str) -> bool {
        match s {
            StackId::Object(o) => self.object_stacks[o].user.has_value(key),
            _ => {
                let st = self.stack(s);
                st.instance_at(index::USER).has_value(key) || st.instance_at(index::QUALITY_CHANGES).has_value(key)
            }
        }
    }

    /// `getProperty` of the stack (dispatching to the Global/Extruder specialisations).
    pub fn get_property(&self, s: StackId, key: &str, prop: &str, ctx: Option<&Ctx>) -> Option<Value> {
        match s {
            StackId::Global => self.global_get_property(key, prop, ctx),
            StackId::Extruder(i) => self.extruder_get_property(i, key, prop, ctx),
            StackId::Object(o) => self.object_get_property(o, key, prop, ctx),
        }
    }

    fn object_user_value(&self, o: usize, key: &str, prop: &str) -> Option<Prop> {
        if prop != "value" {
            return None;
        }
        let user = &self.object_stacks[o].user;
        match user.raw_value(key)? {
            crate::instance::InstValue::Typed(v) => Some(Prop::Value(v)),
            crate::instance::InstValue::Func(f) => Some(Prop::Func(f)),
            crate::instance::InstValue::Str(st) => {
                let ty = self.global.definition().find(key).map(|d| d.setting_type()).unwrap_or_default();
                Some(Prop::Value(crate::instance::setting_value_from_string(&ty, &st)))
            }
        }
    }

    /// `PerObjectContainerStack.getProperty`.
    fn object_get_property(&self, o: usize, key: &str, prop: &str, ctx: Option<&Ctx>) -> Option<Value> {
        let s = StackId::Object(o);
        let owned;
        let ctx: &Ctx = match ctx {
            Some(c) => c,
            None => {
                owned = Ctx::new(None);
                &owned
            }
        };
        ctx.push(s);
        if self.object_stacks[o].user.has_value(key) && prop == "value" {
            let result = self.base_get_property(s, key, prop, Some(ctx));
            ctx.pop();
            return result;
        }
        let mut limit = self.base_get_property(s, key, "limit_to_extruder", Some(ctx)).map(|v| v.py_str());
        if limit.as_deref() == Some("-1") {
            if let Some(orig) = ctx.0.borrow().original_limit_to_extruder.clone() {
                limit = Some(orig);
            }
        }
        if let Some(l) = limit.clone() {
            if l != "-1" {
                if let Ok(n) = l.trim().parse::<i64>() {
                    let list = self.extruder_list();
                    if n <= list.len() as i64 {
                        if ctx.0.borrow().original_limit_to_extruder.is_none() {
                            ctx.0.borrow_mut().original_limit_to_extruder = Some(l.clone());
                        }
                        if self.base_get_property(s, key, "settable_per_extruder", Some(ctx)).map(|v| v.truthy()).unwrap_or(false) {
                            let idx = if n < 0 { n + list.len() as i64 } else { n };
                            if idx >= 0 && (idx as usize) < list.len() {
                                let result = self.extruder_get_property(list[idx as usize], key, prop, Some(ctx));
                                if result.is_some() {
                                    ctx.pop();
                                    return result;
                                }
                            }
                        }
                    }
                }
            }
        }
        let result = self.base_get_property(s, key, prop, Some(ctx));
        ctx.pop();
        result
    }

    pub fn get_value(&self, s: StackId, key: &str) -> Option<Value> {
        self.get_property(s, key, "value", None)
    }

    fn global_get_property(&self, key: &str, prop: &str, ctx: Option<&Ctx>) -> Option<Value> {
        let s = StackId::Global;
        if !self.global.definition().has_key(key) {
            return None;
        }
        ctx_push(ctx, s);
        // Resolve.
        let should_resolve = prop == "value"
            && matches!(self.global.definition().get_property(key, "resolve"), Some(Prop::Func(_)))
            && !self.resolving.borrow().contains(key)
            && !self.has_user_value(s, key);
        if should_resolve {
            self.resolving.borrow_mut().insert(key.to_string());
            let resolve = self.cura_get_property(s, key, "resolve", ctx);
            self.resolving.borrow_mut().remove(key);
            if let Some(r) = resolve {
                if !r.is_none() {
                    // Note: Cura returns here without popping the context.
                    return Some(r);
                }
            }
        }
        let limit = self.cura_get_property(s, key, "limit_to_extruder", ctx);
        let limit = limit.filter(|v| !v.is_none()).map(|v| v.py_str());
        if let Some(limit) = limit {
            if limit != "-1" {
                if let Some(ei) = self.extruder_by_position(&limit) {
                    if self.cura_get_property(s, key, "settable_per_extruder", ctx).map(|v| v.truthy()).unwrap_or(false) {
                        let result = self.extruder_get_property(ei, key, prop, ctx);
                        if result.as_ref().map(|v| !v.is_none()).unwrap_or(false) {
                            ctx_pop(ctx);
                            return result;
                        }
                    }
                }
            }
        }
        let result = self.cura_get_property(s, key, prop, ctx);
        ctx_pop(ctx);
        result
    }

    fn extruder_get_property(&self, i: usize, key: &str, prop: &str, ctx: Option<&Ctx>) -> Option<Value> {
        let s = StackId::Extruder(i);
        ctx_push(ctx, s);
        let settable = self.cura_get_property(s, key, "settable_per_extruder", ctx);
        if !settable.map(|v| v.truthy()).unwrap_or(false) {
            let result = self.global_get_property(key, prop, ctx);
            ctx_pop(ctx);
            return result;
        }
        let owned_ctx;
        let ctx: &Ctx = match ctx {
            Some(c) => c,
            None => {
                owned_ctx = Ctx::new(Some(s));
                &owned_ctx
            }
        };
        let has_pos = ctx.0.borrow().extruder_position.is_some();
        if !has_pos {
            let v = self.cura_get_property(s, key, "limit_to_extruder", Some(ctx));
            ctx.0.borrow_mut().extruder_position = Some(v);
        }
        let limit = ctx.0.borrow().extruder_position.clone().unwrap();
        let limit = limit.filter(|v| !v.is_none()).map(|v| v.py_str());
        if let Some(limit) = limit {
            if limit != "-1" && self.extruders[i].position().as_deref() != Some(limit.as_str()) {
                // `extruderList[int(limit)]` (IndexError is ignored; other errors propagate).
                if let Ok(n) = limit.trim().parse::<i64>() {
                    let list = self.extruder_list();
                    let idx = if n < 0 { n + list.len() as i64 } else { n };
                    if idx >= 0 && (idx as usize) < list.len() {
                        let result = self.extruder_get_property(list[idx as usize], key, prop, Some(ctx));
                        if result.as_ref().map(|v| !v.is_none()).unwrap_or(false) {
                            ctx.pop();
                            return result;
                        }
                    }
                }
            }
        }
        let result = self.cura_get_property(s, key, prop, Some(ctx));
        ctx.pop();
        result
    }

    /// `SettingFunction.__call__(value_provider, context, additional_variables=...)`.
    pub fn call_function(&self, f: &Function, provider: StackId, ctx: Option<&Ctx>, additional: Option<&[(String, Value)]>) -> Option<Value> {
        if !f.is_valid() {
            return None;
        }
        let provider = ctx.and_then(|c| c.root()).unwrap_or(provider);
        let mut locals: Vec<(String, Value)> = Vec::new();
        for name in f.used_values() {
            if let Some(v) = self.get_property(provider, name, "value", ctx) {
                if !v.is_none() {
                    locals.push((name.clone(), v));
                }
            }
        }
        if let Some(extra) = additional {
            for (k, v) in extra {
                if let Some(slot) = locals.iter_mut().find(|(n, _)| n == k) {
                    slot.1 = v.clone();
                } else {
                    locals.push((k.clone(), v.clone()));
                }
            }
        }
        let default_ops = ctx.map(|c| c.0.borrow().default_operators).unwrap_or(false);
        let env = FormulaEnv { machine: self, locals, extra: None, default_ops };
        f.evaluate(&env)
    }

    /// Evaluate a formula with the given variables as its locals (`SettingFunction.__call__` with
    /// `additional_variables` that cover every setting of the stack).
    pub fn evaluate_with_locals(&self, f: &Function, locals: &HashMap<String, Value>) -> Option<Value> {
        if !f.is_valid() {
            return None;
        }
        let env = FormulaEnv { machine: self, locals: Vec::new(), extra: Some(locals), default_ops: false };
        f.evaluate(&env)
    }

    // ---- CuraFormulaFunctions ----

    fn call_operator(&self, name: &str, args: &[Value], kwargs: &[(Rc<str>, Value)], default_ops: bool) -> PyResult<Value> {
        match name {
            "debug" => Ok(arg(args, kwargs, 0, "value")?.clone()),
            "defaultExtruderPosition" => Ok(Value::str(&self.default_extruder_position.borrow())),
            "extruderValue" => {
                let pos = arg(args, kwargs, 0, "extruder_position")?;
                let key = arg(args, kwargs, 1, "property_key")?.py_str();
                if default_ops {
                    self.default_value_in_extruder(pos, &key)
                } else {
                    self.value_in_extruder(pos, &key, None)
                }
            }
            "extruderValues" => {
                let key = arg(args, kwargs, 0, "property_key")?.py_str();
                if default_ops {
                    let ctx = self.default_value_context(StackId::Global);
                    self.values_in_all_extruders(&key, Some(&ctx))
                } else {
                    self.values_in_all_extruders(&key, None)
                }
            }
            "anyExtruderWithMaterial" => {
                let key = arg(args, kwargs, 0, "filter_key")?.py_str();
                self.extruder_position_with_material(&key, None)
            }
            "anyExtruderNrWithOrDefault" => {
                let key = arg(args, kwargs, 0, "filter_key")?.py_str();
                self.any_extruder_position_with_or_default(&key, None)
            }
            "resolveOrValue" => {
                let key = arg(args, kwargs, 0, "property_key")?.py_str();
                if default_ops {
                    let ctx = self.default_value_context(StackId::Global);
                    Ok(self.global_get_property(&key, "value", Some(&ctx)).unwrap_or(Value::None))
                } else {
                    Ok(self.global_get_property(&key, "value", None).unwrap_or(Value::None))
                }
            }
            "valueFromContainer" => {
                let key = arg(args, kwargs, 0, "property_key")?.py_str();
                let idx = py_int(arg(args, kwargs, 1, "container_index")?)?;
                let ctx = self.default_value_context(StackId::Global);
                ctx.0.borrow_mut().evaluate_from = Some(idx);
                Ok(self.global_get_property(&key, "value", Some(&ctx)).unwrap_or(Value::None))
            }
            "extruderValueFromContainer" => {
                let pos = arg(args, kwargs, 0, "extruder_position")?;
                let key = arg(args, kwargs, 1, "property_key")?.py_str();
                let idx = py_int(arg(args, kwargs, 2, "container_index")?)?;
                let mut p = py_int(pos)?;
                if p == -1 {
                    p = self.default_extruder_position.borrow().parse().unwrap_or(0);
                }
                let list = self.extruder_list();
                let ii = if p < 0 { p + list.len() as i64 } else { p };
                if ii < 0 || ii as usize >= list.len() {
                    return Ok(Value::None);
                }
                let ctx = self.default_value_context(StackId::Extruder(list[ii as usize]));
                ctx.0.borrow_mut().evaluate_from = Some(idx);
                self.value_in_extruder(&Value::Int(p), &key, Some(&ctx))
            }
            _ => Err(PyErr::new("NameError", format!("name '{}' is not defined", name))),
        }
    }

    fn default_value_context(&self, source: StackId) -> Ctx {
        let c = Ctx::new(Some(source));
        {
            let mut d = c.0.borrow_mut();
            d.evaluate_from = Some(1);
            d.default_operators = true;
        }
        c
    }

    /// `getValueInExtruder`.
    pub fn value_in_extruder(&self, pos: &Value, key: &str, ctx: Option<&Ctx>) -> PyResult<Value> {
        let mut p = match pos {
            Value::Int(i) => *i,
            Value::Bool(b) => *b as i64,
            other => {
                // `extruder_position == -1` is false for non-numbers; `int(...)` below may raise.
                py_int(other)?
            }
        };
        if pos.py_eq(&Value::Int(-1)) {
            p = self.default_extruder_position.borrow().parse().unwrap_or(0);
        }
        let list = self.extruder_list();
        let idx = if p < 0 { p + list.len() as i64 } else { p };
        if idx < 0 || idx as usize >= list.len() {
            if p != 0 {
                return self.value_in_extruder(&Value::Int(0), key, ctx);
            }
            return Ok(Value::None);
        }
        let ei = list[idx as usize];
        let s = StackId::Extruder(ei);
        let value = match self.raw_property(s, key, "value", ctx) {
            None => Value::None,
            Some(Prop::Value(v)) => v,
            Some(Prop::Func(f)) => self.call_function(&f, s, ctx, None).unwrap_or(Value::None),
        };
        Ok(match value {
            Value::Str(s) => Value::Str(s.to_lowercase().into()),
            v => v,
        })
    }

    fn default_value_in_extruder(&self, pos: &Value, key: &str) -> PyResult<Value> {
        let p = py_int(pos).unwrap_or(0);
        let list = self.extruder_list();
        let idx = if p < 0 { p + list.len() as i64 } else { p };
        if idx < 0 || idx as usize >= list.len() {
            return Ok(Value::None);
        }
        let ctx = self.default_value_context(StackId::Extruder(list[idx as usize]));
        self.value_in_extruder(pos, key, Some(&ctx))
    }

    /// `_getActiveExtruders`.
    fn active_extruders(&self, ctx: Option<&Ctx>) -> Vec<usize> {
        let count = self.global_get_property("machine_extruder_count", "value", ctx);
        self.extruder_list()
            .into_iter()
            .filter(|&i| {
                let e = &self.extruders[i];
                if !e.is_enabled() {
                    return false;
                }
                let pos: i64 = e.position().and_then(|p| p.parse().ok()).unwrap_or(0);
                match &count {
                    Some(c) => !Value::Int(pos).py_ge(c).unwrap_or(false),
                    None => true,
                }
            })
            .collect()
    }

    /// `getValuesInAllExtruders`.
    pub fn values_in_all_extruders(&self, key: &str, ctx: Option<&Ctx>) -> PyResult<Value> {
        let mut result = Vec::new();
        for i in self.active_extruders(ctx) {
            let s = StackId::Extruder(i);
            let value = match self.raw_property(s, key, "value", ctx) {
                None => continue,
                Some(Prop::Value(Value::None)) => continue,
                Some(Prop::Value(v)) => v,
                Some(Prop::Func(f)) => self.call_function(&f, s, ctx, None).unwrap_or(Value::None),
            };
            result.push(value);
        }
        if result.is_empty() {
            result.push(self.global_get_property(key, "value", ctx).unwrap_or(Value::None));
        }
        Ok(Value::list(result))
    }

    fn any_extruder_position_with_or_default(&self, key: &str, ctx: Option<&Ctx>) -> PyResult<Value> {
        for i in self.active_extruders(ctx) {
            let truthy = match self.raw_property(StackId::Extruder(i), key, "value", ctx) {
                None => false,
                Some(Prop::Value(v)) => v.truthy(),
                Some(Prop::Func(_)) => true,
            };
            if truthy {
                return Ok(Value::str(&self.extruders[i].position().unwrap_or_default()));
            }
        }
        Ok(Value::None)
    }

    fn extruder_position_with_material(&self, key: &str, _ctx: Option<&Ctx>) -> PyResult<Value> {
        for i in self.active_extruders(None) {
            let mat = self.extruders[i].container(index::MATERIAL).clone();
            if let Some(p) = self.container_property(&mat, key, "value") {
                let is_some = match &p {
                    Prop::Value(v) => !v.is_none(),
                    Prop::Func(_) => true,
                };
                if is_some {
                    return Ok(Value::str(&self.extruders[i].position().unwrap_or_default()));
                }
            }
        }
        Ok(Value::str(&self.default_extruder_position.borrow()))
    }

    // ---- Keys ----

    /// `ContainerStack.getAllKeys` (definition keys of the stack and its next stack).
    pub fn all_keys(&self, s: StackId) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut seen = HashSet::new();
        let mut push_def = |d: &DefinitionContainer, out: &mut Vec<String>| {
            for k in d.all_keys() {
                if seen.insert(k.to_string()) {
                    out.push(k.to_string());
                }
            }
        };
        match s {
            StackId::Global => push_def(self.global.definition(), &mut out),
            StackId::Extruder(i) => {
                push_def(self.extruders[i].definition(), &mut out);
                push_def(self.global.definition(), &mut out);
            }
            StackId::Object(o) => {
                push_def(self.extruders[self.object_stacks[o].extruder_position].definition(), &mut out);
                push_def(self.global.definition(), &mut out);
            }
        }
        out
    }

    /// Validation state of a setting in a stack (`Validator`), as Cura's settings panel shows it.
    pub fn validation_state(&self, s: StackId, key: &str) -> ValidationState {
        let get = |p: &str| self.get_property(s, key, p, None);
        let r = (|| -> Result<ValidationState, ()> {
            let minimum = get("minimum_value");
            let maximum = get("maximum_value");
            let min_w = get("minimum_value_warning");
            let max_w = get("maximum_value_warning");
            let warn_v = get("warning_value");
            let err_v = get("error_value");
            if let (Some(mn), Some(mx)) = (&minimum, &maximum) {
                if !mn.is_none() && !mx.is_none() && mn.py_gt(mx).map_err(|_| ())? {
                    return Err(());
                }
            }
            let value = get("value").ok_or(())?;
            if value.is_none() || matches!(value, Value::Float(f) if f.is_nan()) {
                return Err(());
            }
            let ty = self.setting_definition(s, key).map(|d| d.setting_type()).unwrap_or_default();
            if ty == "str" {
                let allow_empty = get("allow_empty");
                if matches!(allow_empty, Some(Value::Bool(false))) && value.py_str().is_empty() {
                    return Ok(ValidationState::Invalid);
                }
                return Ok(ValidationState::Valid);
            }
            if ty == "bool" {
                if let Some(w) = warn_v.filter(|v| !v.is_none()) {
                    if value.py_eq(&w) {
                        return Ok(ValidationState::MaximumWarning);
                    }
                }
                if let Some(e) = err_v.filter(|v| !v.is_none()) {
                    if value.py_eq(&e) {
                        return Ok(ValidationState::MaximumError);
                    }
                }
                return Ok(ValidationState::Valid);
            }
            let lt = |a: &Value, b: &Option<Value>| -> Result<bool, ()> {
                match b {
                    Some(b) if !b.is_none() => a.py_lt(b).map_err(|_| ()),
                    _ => Ok(false),
                }
            };
            let gt = |a: &Value, b: &Option<Value>| -> Result<bool, ()> {
                match b {
                    Some(b) if !b.is_none() => a.py_gt(b).map_err(|_| ()),
                    _ => Ok(false),
                }
            };
            if lt(&value, &minimum)? {
                return Ok(ValidationState::MinimumError);
            }
            if gt(&value, &maximum)? {
                return Ok(ValidationState::MaximumError);
            }
            if lt(&value, &min_w)? {
                return Ok(ValidationState::MinimumWarning);
            }
            if gt(&value, &max_w)? {
                return Ok(ValidationState::MaximumWarning);
            }
            Ok(ValidationState::Valid)
        })();
        r.unwrap_or(ValidationState::Exception)
    }

    /// The setting definition a stack uses for `key` (its own definition first, then the machine's).
    pub fn setting_definition(&self, s: StackId, key: &str) -> Option<&crate::definition::SettingDefinition> {
        match s {
            StackId::Global => self.global.definition().find(key),
            StackId::Extruder(i) => self.extruders[i].definition().find(key).or_else(|| self.global.definition().find(key)),
            StackId::Object(o) => {
                let i = self.object_stacks[o].extruder_position;
                self.extruders[i].definition().find(key).or_else(|| self.global.definition().find(key))
            }
        }
    }

    pub fn clear_caches(&self) {
        self.global.clear_caches();
        for e in &self.extruders {
            e.clear_caches();
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValidationState {
    Exception,
    Valid,
    Invalid,
    MinimumError,
    MinimumWarning,
    MaximumError,
    MaximumWarning,
}

impl ValidationState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ValidationState::Exception => "Exception",
            ValidationState::Valid => "Valid",
            ValidationState::Invalid => "Invalid",
            ValidationState::MinimumError => "MinimumError",
            ValidationState::MinimumWarning => "MinimumWarning",
            ValidationState::MaximumError => "MaximumError",
            ValidationState::MaximumWarning => "MaximumWarning",
        }
    }
    pub fn is_error(&self) -> bool {
        matches!(self, ValidationState::Exception | ValidationState::Invalid | ValidationState::MinimumError | ValidationState::MaximumError)
    }
}
