//! The container tree (`cura.Machines`: machine → variant → material → quality → intent nodes),
//! machine creation (`CuraStackBuilder`) and the profile switching logic of `MachineManager`.

use crate::definition::Prop;
use crate::instance::{InstValue, InstanceContainer};
use crate::py::Value;
use crate::registry::{MetaEntry, Registry};
use crate::stack::{index, parse_bool, Container, Ctx, Machine, Stack, StackId};
use std::rc::Rc;

/// An insertion-ordered map with Python dict assignment semantics.
#[derive(Clone, Debug)]
pub struct OrderedMap<V> {
    pub items: Vec<(String, V)>,
}

impl<V> Default for OrderedMap<V> {
    fn default() -> Self {
        OrderedMap { items: Vec::new() }
    }
}

impl<V> OrderedMap<V> {
    pub fn insert(&mut self, k: &str, v: V) {
        if let Some(slot) = self.items.iter_mut().find(|(x, _)| x == k) {
            slot.1 = v;
        } else {
            self.items.push((k.to_string(), v));
        }
    }
    pub fn get(&self, k: &str) -> Option<&V> {
        self.items.iter().find(|(x, _)| x == k).map(|(_, v)| v)
    }
    pub fn contains(&self, k: &str) -> bool {
        self.items.iter().any(|(x, _)| x == k)
    }
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.items.iter().map(|(_, v)| v)
    }
    pub fn first(&self) -> Option<&V> {
        self.items.first().map(|(_, v)| v)
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

fn meta_bool(m: &MetaEntry, key: &str, default: &str) -> bool {
    match m.get(key) {
        Some(v) => parse_bool(v),
        None => parse_bool(&Value::str(default)),
    }
}

#[derive(Debug, Clone)]
pub struct IntentNode {
    pub container_id: String,
    pub intent_category: String,
}

#[derive(Debug, Clone)]
pub struct QualityNode {
    pub container_id: String,
    pub quality_type: String,
    pub material: Option<String>,
    pub intents: OrderedMap<IntentNode>,
}

#[derive(Debug, Clone)]
pub struct MaterialNode {
    pub container_id: String,
    pub base_file: String,
    pub material_type: String,
    pub brand: String,
    pub guid: String,
    pub qualities: OrderedMap<QualityNode>,
}

#[derive(Debug, Clone)]
pub struct VariantNode {
    pub container_id: String,
    pub variant_name: String,
    pub materials: OrderedMap<MaterialNode>,
}

#[derive(Debug, Clone)]
pub struct MachineNode {
    pub container_id: String,
    pub has_materials: bool,
    pub has_variants: bool,
    pub has_machine_quality: bool,
    pub quality_definition: String,
    pub exclude_materials: Vec<String>,
    pub preferred_variant_name: String,
    pub preferred_material: String,
    pub preferred_quality_type: String,
    pub supports_abstract_color: bool,
    pub variants: OrderedMap<VariantNode>,
    pub global_qualities: OrderedMap<QualityNode>,
}

impl QualityNode {
    fn new(reg: &Registry, id: &str, machine: Option<(&MachineNode, &str)>) -> QualityNode {
        let meta = reg.metadata(id);
        let quality_type = meta.and_then(|m| m.get_str("quality_type")).unwrap_or_default();
        let material = meta.and_then(|m| m.get_str("material"));
        let mut intents = OrderedMap::default();
        if let Some((machine, variant_name)) = machine {
            let mut filters: Vec<(&str, &str)> = vec![("type", "intent"), ("definition", &machine.quality_definition), ("variant", variant_name)];
            let mat;
            match &material {
                Some(m) => {
                    mat = m.clone();
                    filters.push(("material", &mat));
                }
                // `material = None` never matches.
                None => filters.push(("material", "\u{0}none")),
            }
            filters.push(("quality_type", &quality_type));
            for intent in reg.find_instances(&filters) {
                let cat = intent.get_str("intent_category").unwrap_or_else(|| "default".into());
                intents.insert(&intent.id, IntentNode { container_id: intent.id.clone(), intent_category: cat });
            }
        }
        intents.insert("empty_intent", IntentNode { container_id: "empty_intent".into(), intent_category: "default".into() });
        QualityNode { container_id: id.into(), quality_type, material, intents }
    }
}

impl MaterialNode {
    fn new(reg: &Registry, id: &str, machine: &MachineNode, variant_name: &str) -> MaterialNode {
        let m = reg.metadata(id);
        let g = |k: &str| m.and_then(|m| m.get_str(k)).unwrap_or_default();
        let mut node = MaterialNode { container_id: id.into(), base_file: g("base_file"), material_type: g("material"), brand: g("brand"), guid: g("GUID"), qualities: OrderedMap::default() };
        let qualities: Vec<&MetaEntry> = if !machine.has_machine_quality {
            reg.find_instances(&[("type", "quality"), ("definition", "fdmprinter")])
        } else if !machine.has_materials {
            reg.find_instances(&[("type", "quality"), ("definition", &machine.quality_definition)])
        } else {
            let mut q: Vec<&MetaEntry> = if machine.has_variants {
                reg.find_instances(&[("type", "quality"), ("definition", &machine.quality_definition), ("variant", variant_name), ("material", &node.base_file)])
            } else {
                reg.find_instances(&[("type", "quality"), ("definition", &machine.quality_definition), ("material", &node.base_file)])
            };
            if q.is_empty() {
                let any_material: Vec<&MetaEntry> = if machine.has_variants {
                    reg.find_instances(&[("type", "quality"), ("definition", &machine.quality_definition), ("variant", variant_name)])
                } else {
                    reg.find_instances(&[("type", "quality"), ("definition", &machine.quality_definition)])
                };
                let right_brand: std::collections::HashSet<String> = reg
                    .find_instances(&[("type", "material"), ("material", &node.material_type), ("brand", &node.brand)])
                    .iter()
                    .filter_map(|m| m.get_str("base_file"))
                    .collect();
                let rb: Vec<&MetaEntry> = any_material.iter().copied().filter(|qm| qm.get_str("material").map(|m| right_brand.contains(&m)).unwrap_or(false)).collect();
                if !rb.is_empty() {
                    q.extend(rb);
                } else {
                    let all_bases: std::collections::HashSet<String> =
                        reg.find_instances(&[("type", "material"), ("material", &node.material_type)]).iter().filter_map(|m| m.get_str("base_file")).collect();
                    q.extend(any_material.iter().copied().filter(|qm| qm.get_str("material").map(|m| all_bases.contains(&m)).unwrap_or(false)));
                }
                if q.is_empty() {
                    for mm in reg.find_instances(&[("type", "material"), ("guid", &node.guid)]) {
                        let bf = mm.get_str("base_file");
                        q.extend(any_material.iter().copied().filter(|qm| qm.get_str("material") == bf));
                    }
                }
                if q.is_empty() {
                    q.extend(any_material.iter().copied().filter(|qm| qm.get_str("global_quality").unwrap_or_else(|| "False".into()) != "False"));
                }
            }
            q
        };
        let is_global_parent = false;
        let _ = is_global_parent;
        for qm in qualities {
            if !node.qualities.contains(&qm.id) {
                let qn = QualityNode::new(reg, &qm.id, Some((machine, variant_name)));
                node.qualities.insert(&qm.id, qn);
            }
        }
        if node.qualities.is_empty() {
            node.qualities.insert("empty_quality", QualityNode::new(reg, "empty_quality", Some((machine, variant_name))));
        }
        node
    }

    /// `MaterialNode.preferredQuality`.
    pub fn preferred_quality(&self, machine: &MachineNode) -> &QualityNode {
        for q in self.qualities.values() {
            if machine.preferred_quality_type == q.quality_type {
                return q;
            }
        }
        self.qualities.first().unwrap()
    }
}

impl VariantNode {
    fn new(reg: &Registry, id: &str, machine: &MachineNode) -> VariantNode {
        let variant_name = reg.metadata(id).and_then(|m| m.get_str("name")).unwrap_or_default();
        let mut node = VariantNode { container_id: id.into(), variant_name: variant_name.clone(), materials: OrderedMap::default() };
        if !machine.has_materials {
            node.materials.insert("empty_material", MaterialNode::new(reg, "empty_material", machine, &variant_name));
            return node;
        }
        let base = reg.find_instances(&[("type", "material"), ("definition", "fdmprinter")]);
        let printer = reg.find_instances(&[("type", "material"), ("definition", &machine.container_id)]);
        let variant_specific = reg.find_instances(&[("type", "material"), ("definition", &machine.container_id), ("variant_name", &variant_name)]);
        let mut per_base: OrderedMap<&MetaEntry> = OrderedMap::default();
        for m in base.into_iter().chain(printer).chain(variant_specific) {
            per_base.insert(&m.get_str("base_file").unwrap_or_default(), m);
        }
        for m in per_base.values() {
            if machine.is_excluded_material(&m.id) {
                continue;
            }
            if m.get("abstract_color").map(|v| v.truthy()).unwrap_or(false) && !machine.supports_abstract_color {
                continue;
            }
            let bf = m.get_str("base_file").unwrap_or_default();
            if !node.materials.contains(&bf) {
                node.materials.insert(&bf, MaterialNode::new(reg, &m.id, machine, &variant_name));
            }
        }
        if node.materials.is_empty() {
            node.materials.insert("empty_material", MaterialNode::new(reg, "empty_material", machine, &variant_name));
        }
        node
    }

    /// `VariantNode.preferredMaterial`.
    pub fn preferred_material(&self, reg: &Registry, machine: &MachineNode, approximate_diameter: i64) -> &MaterialNode {
        let diameter_of = |m: &MaterialNode| -> Option<i64> { reg.metadata(&m.container_id).and_then(|e| e.get_str("approximate_diameter")).and_then(|s| s.trim().parse::<i64>().ok()) };
        for (base, m) in &self.materials.items {
            if machine.preferred_material == *base && diameter_of(m) == Some(approximate_diameter) {
                return m;
            }
        }
        if approximate_diameter == 2 {
            let preferred = format!("{}_175", machine.preferred_material);
            for (base, m) in &self.materials.items {
                if preferred == *base && diameter_of(m) == Some(approximate_diameter) {
                    return m;
                }
            }
        }
        for m in self.materials.values() {
            if diameter_of(m) == Some(approximate_diameter) {
                return m;
            }
        }
        self.materials.first().unwrap()
    }
}

impl MachineNode {
    pub fn new(reg: &Registry, id: &str) -> MachineNode {
        let meta = reg.metadata(id).cloned().unwrap_or(MetaEntry { id: id.into(), source: crate::registry::Source::Runtime, meta: Vec::new() });
        let has_machine_quality = meta_bool(&meta, "has_machine_quality", "false");
        let mut node = MachineNode {
            container_id: id.into(),
            has_materials: meta_bool(&meta, "has_materials", "true"),
            has_variants: meta_bool(&meta, "has_variants", "false"),
            has_machine_quality,
            quality_definition: if has_machine_quality { meta.get_str("quality_definition").unwrap_or_else(|| id.to_string()) } else { "fdmprinter".into() },
            exclude_materials: match meta.get("exclude_materials") {
                Some(Value::List(l)) => l.iter().map(|v| v.py_str()).collect(),
                _ => Vec::new(),
            },
            preferred_variant_name: meta.get_str("preferred_variant_name").unwrap_or_default(),
            preferred_material: meta.get_str("preferred_material").unwrap_or_default(),
            preferred_quality_type: meta.get_str("preferred_quality_type").unwrap_or_default(),
            supports_abstract_color: meta_bool(&meta, "supports_abstract_color", "false"),
            variants: OrderedMap::default(),
            global_qualities: OrderedMap::default(),
        };
        if !node.has_variants {
            let v = VariantNode::new(reg, "empty_variant", &node);
            node.variants.insert("empty", v);
        } else {
            let variants = reg.find_instances(&[("type", "variant"), ("definition", id), ("hardware_type", "nozzle")]);
            for v in variants {
                let name = v.get_str("name").unwrap_or_default();
                if !node.variants.contains(&name) {
                    let vn = VariantNode::new(reg, &v.id, &node);
                    node.variants.insert(&name, vn);
                }
            }
            if node.variants.is_empty() {
                let v = VariantNode::new(reg, "empty_variant", &node);
                node.variants.insert("empty", v);
            }
        }
        let mut gq = reg.find_instances(&[("type", "quality"), ("definition", &node.quality_definition), ("global_quality", "True")]);
        if gq.is_empty() {
            gq = reg.find_instances(&[("type", "quality"), ("definition", "fdmprinter"), ("global_quality", "True")]);
        }
        let ids: Vec<String> = if gq.is_empty() { vec!["empty_quality".into()] } else { gq.iter().map(|m| m.id.clone()).collect() };
        for qid in ids {
            let qn = QualityNode::new(reg, &qid, None);
            node.global_qualities.insert(&qn.quality_type.clone(), qn);
        }
        node
    }

    pub fn is_excluded_material(&self, base_file: &str) -> bool {
        self.exclude_materials.iter().any(|e| base_file.contains(e.as_str()))
    }

    /// `MachineNode.preferredGlobalQuality`.
    pub fn preferred_global_quality(&self) -> &QualityNode {
        self.global_qualities.get(&self.preferred_quality_type).unwrap_or_else(|| self.global_qualities.first().unwrap())
    }

    /// `MachineNode.getQualityGroups`.
    pub fn quality_groups(&self, variant_names: &[String], material_bases: &[String], enabled: &[bool]) -> OrderedMap<QualityGroup> {
        let n = variant_names.len();
        let mut per_extruder: Vec<Option<OrderedMap<QualityNode>>> = vec![None; n];
        for i in 0..n {
            if !enabled[i] {
                continue;
            }
            let qmap = match self.variants.get(&variant_names[i]).and_then(|v| v.materials.get(&material_bases[i])) {
                None => self.global_qualities.clone(),
                Some(mn) => {
                    let mut m = OrderedMap::default();
                    for q in mn.qualities.values() {
                        m.insert(&q.quality_type, q.clone());
                    }
                    m
                }
            };
            per_extruder[i] = Some(qmap);
        }
        let mut groups: OrderedMap<QualityGroup> = OrderedMap::default();
        for (qt, gq) in &self.global_qualities.items {
            let mut g = QualityGroup { quality_type: qt.clone(), name: String::new(), node_for_global: gq.container_id.clone(), nodes_for_extruders: Vec::new(), is_available: false };
            for (i, pe) in per_extruder.iter().enumerate() {
                // Disabled extruders still use the (shared) empty dict of the other extruders in
                // Cura (`[{}] * n`), i.e. no nodes.
                if let Some(pe) = pe {
                    if let Some(q) = pe.get(qt) {
                        g.nodes_for_extruders.push((i, q.container_id.clone()));
                    }
                }
            }
            groups.insert(qt, g);
        }
        let mut available: Vec<String> = groups.items.iter().map(|(k, _)| k.clone()).collect();
        for (i, pe) in per_extruder.iter().enumerate() {
            if !enabled[i] {
                continue;
            }
            let keys: Vec<String> = pe.as_ref().map(|p| p.items.iter().map(|(k, _)| k.clone()).collect()).unwrap_or_default();
            available.retain(|q| keys.contains(q));
        }
        for (k, g) in groups.items.iter_mut() {
            if available.contains(k) {
                g.is_available = true;
            }
        }
        groups
    }
}

#[derive(Clone, Debug)]
pub struct QualityGroup {
    pub quality_type: String,
    pub name: String,
    pub node_for_global: String,
    pub nodes_for_extruders: Vec<(usize, String)>,
    pub is_available: bool,
}

/// `ContainerTree` (machine nodes are built on demand).
#[derive(Default)]
pub struct ContainerTree {
    machines: std::cell::RefCell<std::collections::HashMap<String, Rc<MachineNode>>>,
}

impl ContainerTree {
    pub fn machine(&self, reg: &Registry, id: &str) -> Rc<MachineNode> {
        if let Some(m) = self.machines.borrow().get(id) {
            return m.clone();
        }
        let m = Rc::new(MachineNode::new(reg, id));
        self.machines.borrow_mut().insert(id.to_string(), m.clone());
        m
    }
}

fn instance_or_empty(reg: &Registry, id: &str, fallback: &str) -> Rc<InstanceContainer> {
    reg.instance(id).or_else(|| reg.instance(fallback)).expect("empty containers exist")
}

/// `CuraStackBuilder.createMachine` followed by `MachineManager.setActiveMachine`.
pub fn create_machine(reg: &Rc<Registry>, tree: &ContainerTree, name: &str, definition_id: &str) -> Result<Machine, String> {
    let definition = reg.definition(definition_id).ok_or_else(|| format!("Definition {} was not found", definition_id))?;
    let machine_node = tree.machine(reg, definition_id);
    let stack_id = name.to_string();

    let global_quality = instance_or_empty(reg, &machine_node.preferred_global_quality().container_id, "empty_quality");
    let mut gu = InstanceContainer::new(&format!("{}_user", stack_id), &format!("{}_user", stack_id), definition_id);
    gu.set_meta("type", Value::str("user"));
    gu.set_meta("setting_version", Value::str("25"));
    gu.set_meta("machine", Value::str(&stack_id));
    let mut gdc = InstanceContainer::new(&format!("{}_settings", stack_id), &format!("{}_settings", stack_id), definition_id);
    gdc.set_meta("type", Value::str("definition_changes"));
    gdc.set_meta("setting_version", Value::str("25"));
    let global_containers = vec![
        Container::Instance(Rc::new(gu)),
        Container::Instance(instance_or_empty(reg, "empty_quality_changes", "empty")),
        Container::Instance(instance_or_empty(reg, "empty_intent", "empty")),
        Container::Instance(global_quality),
        Container::Instance(instance_or_empty(reg, "empty_material", "empty")),
        Container::Instance(instance_or_empty(reg, "empty_variant", "empty")),
        Container::Instance(Rc::new(gdc)),
        Container::Definition(definition.clone()),
    ];
    let mut global = Stack::new(&stack_id, name, global_containers);
    global.set_meta("type", Value::str("machine"));

    // Extruder trains (from the deep-merged definition metadata).
    let trains: Vec<(String, String)> = match definition.metadata.get("machine_extruder_trains") {
        Some(crate::json::Json::Object(m)) => m.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string())).collect(),
        _ => Vec::new(),
    };
    let mut extruders = Vec::new();
    for (position, ext_def_id) in &trains {
        let ext_def = reg.definition(ext_def_id).ok_or_else(|| format!("Unable to find extruder definition with the id [{}]", ext_def_id))?;
        let approx = match ext_def.get_property("material_diameter", "value") {
            Some(Prop::Value(v)) => crate::py::builtins::py_round(&v, None).ok().and_then(|v| v.as_f64()).map(|f| f as i64).unwrap_or(3),
            _ => 3,
        };
        let variant_node = machine_node.variants.get(&machine_node.preferred_variant_name).unwrap_or_else(|| machine_node.variants.first().unwrap());
        let material_node = variant_node.preferred_material(reg, &machine_node, approx);
        let quality_node = material_node.preferred_quality(&machine_node);
        // `registry.uniqueName(extruder_definition_id)`: the definition already uses the id.
        let eid = format!("{} #2", ext_def_id);
        let mut eu = InstanceContainer::new(&format!("{}_user", eid), &format!("{}_user", eid), definition_id);
        eu.set_meta("type", Value::str("user"));
        eu.set_meta("setting_version", Value::str("25"));
        eu.set_meta("extruder", Value::str(&eid));
        let mut edc = InstanceContainer::new(&format!("{}_settings", eid), &format!("{}_settings", eid), ext_def_id);
        edc.set_meta("type", Value::str("definition_changes"));
        edc.set_meta("setting_version", Value::str("25"));
        let containers = vec![
            Container::Instance(Rc::new(eu)),
            Container::Instance(instance_or_empty(reg, "empty_quality_changes", "empty")),
            Container::Instance(instance_or_empty(reg, "empty_intent", "empty")),
            Container::Instance(instance_or_empty(reg, &quality_node.container_id, "empty_quality")),
            Container::Instance(instance_or_empty(reg, &material_node.container_id, "empty_material")),
            Container::Instance(instance_or_empty(reg, &variant_node.container_id, "empty_variant")),
            Container::Instance(Rc::new(edc)),
            Container::Definition(ext_def.clone()),
        ];
        let mut st = Stack::new(&eid, &ext_def.name, containers);
        st.set_meta("position", Value::str(position));
        st.set_meta("machine", Value::str(&stack_id));
        st.set_meta("type", Value::str("extruder_train"));
        extruders.push(st);
    }
    let mut machine = Machine::new(reg.clone(), global, extruders);
    activate(&mut machine, tree);
    Ok(machine)
}

/// Side effects of `MachineManager.setActiveMachine` (and `_onGlobalContainerChanged`).
pub fn activate(m: &mut Machine, tree: &ContainerTree) {
    update_default_extruder(m);
    update_number_extruders_enabled(m);
    // Global variant must be a buildplate; global material is always empty.
    let gv = m.global.container(index::VARIANT).clone();
    if gv.id() != "empty_variant" && gv.meta_str("hardware_type").as_deref() != Some("buildplate") {
        let e = instance_or_empty(&m.reg, "empty_variant", "empty");
        m.global.containers[index::VARIANT] = Container::Instance(e);
    }
    if m.global.container(index::MATERIAL).id() != "empty_material" {
        let e = instance_or_empty(&m.reg, "empty_material", "empty");
        m.global.containers[index::MATERIAL] = Container::Instance(e);
    }
    update_default_extruder(m);
    validate_variants_and_materials(m, tree);
    m.clear_caches();
}

pub fn update_default_extruder(m: &Machine) {
    let mut pos = "0".to_string();
    for i in m.extruder_list() {
        if m.extruders[i].is_enabled() {
            pos = m.extruders[i].position().unwrap_or_else(|| "0".into());
            break;
        }
    }
    *m.default_extruder_position.borrow_mut() = pos;
}

pub fn update_number_extruders_enabled(m: &Machine) {
    let count = m.get_value(StackId::Global, "machine_extruder_count").and_then(|v| v.as_f64()).unwrap_or(1.0) as i64;
    let mut enabled = 0;
    for (pos, i) in m.extruder_list().into_iter().enumerate() {
        if m.extruders[i].is_enabled() && (pos as i64) < count {
            enabled += 1;
        }
    }
    let dc = m.global.instance_at(index::DEFINITION_CHANGES).clone();
    let current = dc.raw_value("extruders_enabled_count").map(|v| v.serialize());
    if current.as_deref() != Some(&enabled.to_string()) {
        dc.set_value("extruders_enabled_count", InstValue::Typed(Value::Int(enabled)));
    }
    m.clear_caches();
}

fn validate_variants_and_materials(m: &mut Machine, tree: &ContainerTree) {
    let node = tree.machine(&m.reg, &m.global.definition().id);
    for i in m.extruder_list() {
        let variant_name = m.extruders[i].container(index::VARIANT).name();
        let pos = m.extruders[i].position().unwrap_or_default();
        let mut variant = node.variants.get(&variant_name).cloned();
        if variant.is_none() {
            set_variant_by_name(m, tree, &pos, &node.preferred_variant_name);
            variant = node.variants.get(&node.preferred_variant_name).cloned();
        }
        let base_file = m.extruders[i].container(index::MATERIAL).meta_str("base_file").unwrap_or_default();
        let has = variant.map(|v| v.materials.contains(&base_file)).unwrap_or(false);
        if !has {
            set_material_by_id(m, tree, &pos, &node.preferred_material);
        }
    }
}

fn extruder_index_at(m: &Machine, position: &str) -> Option<usize> {
    let list = m.extruder_list();
    let p: usize = position.parse().ok()?;
    list.get(p).copied()
}

/// `MachineManager.setVariantByName`.
pub fn set_variant_by_name(m: &mut Machine, tree: &ContainerTree, position: &str, variant_name: &str) -> bool {
    let node = tree.machine(&m.reg, &m.global.definition().id);
    let Some(variant) = node.variants.get(variant_name) else { return false };
    let Some(i) = extruder_index_at(m, position) else { return false };
    m.extruders[i].containers[index::VARIANT] = Container::Instance(instance_or_empty(&m.reg, &variant.container_id, "empty_variant"));
    m.clear_caches();
    update_material_with_variant(m, tree, Some(position));
    update_quality_with_material(m, tree);
    true
}

/// `ExtruderStack.getApproximateMaterialDiameter`.
pub fn approximate_material_diameter(m: &Machine, i: usize) -> i64 {
    let ctx = Ctx::new(Some(StackId::Extruder(i)));
    ctx_set_from(&ctx, index::VARIANT as i64);
    let d = m.get_property(StackId::Extruder(i), "material_diameter", "value", Some(&ctx)).and_then(|v| v.as_f64()).unwrap_or(2.85);
    crate::py::builtins::py_round(&Value::Float(d), None).ok().and_then(|v| v.as_f64()).unwrap_or(3.0) as i64
}

fn ctx_set_from(ctx: &Ctx, i: i64) {
    ctx.set_evaluate_from(i);
}

/// `MachineManager.updateMaterialWithVariant`.
pub fn update_material_with_variant(m: &mut Machine, tree: &ContainerTree, position: Option<&str>) {
    let node = tree.machine(&m.reg, &m.global.definition().id);
    let positions: Vec<String> = match position {
        Some(p) => vec![p.to_string()],
        None => (0..m.extruder_list().len()).map(|p| p.to_string()).collect(),
    };
    for pos in positions {
        let Some(i) = extruder_index_at(m, &pos) else { continue };
        let current_base = m.extruders[i].container(index::MATERIAL).meta_str("base_file").unwrap_or_default();
        let nozzle_name = m.extruders[i].container(index::VARIANT).meta_str("name").unwrap_or_else(|| m.extruders[i].container(index::VARIANT).name());
        let Some(nozzle) = node.variants.get(&nozzle_name) else { continue };
        let old_d = m.extruders[i].container(index::MATERIAL).meta_str("approximate_diameter").and_then(|s| s.parse::<i64>().ok()).unwrap_or(3);
        let new_d = approximate_material_diameter(m, i);
        let mat = if new_d == old_d && nozzle.materials.contains(&current_base) {
            nozzle.materials.get(&current_base).unwrap().clone()
        } else {
            nozzle.preferred_material(&m.reg, &node, new_d).clone()
        };
        m.extruders[i].containers[index::MATERIAL] = Container::Instance(instance_or_empty(&m.reg, &mat.container_id, "empty_material"));
        m.clear_caches();
    }
}

/// `MachineManager.setMaterialById`.
pub fn set_material_by_id(m: &mut Machine, tree: &ContainerTree, position: &str, root_material_id: &str) -> bool {
    let node = tree.machine(&m.reg, &m.global.definition().id);
    let Some(i) = extruder_index_at(m, position) else { return false };
    let nozzle = m.extruders[i].container(index::VARIANT).name();
    let Some(variant) = node.variants.get(&nozzle) else { return false };
    let Some(mat) = variant.materials.get(root_material_id) else { return false };
    m.extruders[i].containers[index::MATERIAL] = Container::Instance(instance_or_empty(&m.reg, &mat.container_id, "empty_material"));
    m.clear_caches();
    update_quality_with_material(m, tree);
    true
}

/// `ContainerTree.getCurrentQualityGroups`.
pub fn current_quality_groups(m: &Machine, tree: &ContainerTree) -> OrderedMap<QualityGroup> {
    let node = tree.machine(&m.reg, &m.global.definition().id);
    let list = m.extruder_list();
    let variants: Vec<String> = list.iter().map(|&i| m.extruders[i].container(index::VARIANT).name()).collect();
    let materials: Vec<String> = list.iter().map(|&i| m.extruders[i].container(index::MATERIAL).meta_str("base_file").unwrap_or_default()).collect();
    let enabled: Vec<bool> = list.iter().map(|&i| m.extruders[i].is_enabled()).collect();
    let mut groups = node.quality_groups(&variants, &materials, &enabled);
    for (_, g) in groups.items.iter_mut() {
        g.name = m.reg.metadata(&g.node_for_global).and_then(|e| e.get_str("name")).unwrap_or_else(|| "Unnamed profile".into());
    }
    groups
}

fn active_materials_compatible(m: &Machine) -> bool {
    let has_materials = m.global.meta("has_materials").cloned().or_else(|| m.global.definition().metadata.get("has_materials").map(crate::definition::json_to_value));
    if has_materials.map(|v| parse_bool(&v)).unwrap_or(false) {
        for i in m.extruder_list() {
            if !m.extruders[i].is_enabled() {
                continue;
            }
            let compatible = match m.extruders[i].container(index::MATERIAL) {
                Container::Instance(c) => c.meta("compatible").cloned(),
                _ => None,
            };
            if !compatible.map(|v| v.truthy()).unwrap_or(false) {
                return false;
            }
        }
    }
    true
}

fn set_empty_quality(m: &mut Machine, tree: &ContainerTree) {
    let eq = instance_or_empty(&m.reg, "empty_quality", "empty");
    let eqc = instance_or_empty(&m.reg, "empty_quality_changes", "empty");
    m.global.containers[index::QUALITY] = Container::Instance(eq.clone());
    m.global.containers[index::QUALITY_CHANGES] = Container::Instance(eqc.clone());
    for i in m.extruder_list() {
        m.extruders[i].containers[index::QUALITY] = Container::Instance(eq.clone());
        m.extruders[i].containers[index::QUALITY_CHANGES] = Container::Instance(eqc.clone());
    }
    m.clear_caches();
    update_intent_with_quality(m, tree);
}

/// `MachineManager._setQualityGroup`.
pub fn set_quality_group(m: &mut Machine, tree: &ContainerTree, group: &QualityGroup, empty_quality_changes: bool) {
    m.global.containers[index::QUALITY] = Container::Instance(instance_or_empty(&m.reg, &group.node_for_global, "empty_quality"));
    let eqc = instance_or_empty(&m.reg, "empty_quality_changes", "empty");
    if empty_quality_changes {
        m.global.containers[index::QUALITY_CHANGES] = Container::Instance(eqc.clone());
    }
    let list = m.extruder_list();
    for (pos, qid) in &group.nodes_for_extruders {
        if let Some(&i) = list.get(*pos) {
            m.extruders[i].containers[index::QUALITY] = Container::Instance(instance_or_empty(&m.reg, qid, "empty_quality"));
            if empty_quality_changes {
                m.extruders[i].containers[index::QUALITY_CHANGES] = Container::Instance(eqc.clone());
            }
        }
    }
    m.clear_caches();
    update_intent_with_quality(m, tree);
}

/// `MachineManager._setQualityChangesGroup`: activate a custom profile (quality_changes
/// containers for the global stack and per extruder position) on top of the quality group of its
/// quality type, then the intent of its category.
pub fn set_quality_changes(m: &mut Machine, tree: &ContainerTree, quality_type: &str, intent_category: &str, global_qc: Rc<InstanceContainer>, extruder_qcs: &[(String, Rc<InstanceContainer>)]) {
    let groups = current_quality_groups(m, tree);
    let group = if quality_type == "not_supported" { None } else { groups.get(quality_type).cloned() };
    let eq = instance_or_empty(&m.reg, "empty_quality", "empty");
    let eqc = instance_or_empty(&m.reg, "empty_quality_changes", "empty");
    m.global.containers[index::QUALITY] = match &group {
        Some(g) => Container::Instance(instance_or_empty(&m.reg, &g.node_for_global, "empty_quality")),
        None => Container::Instance(eq.clone()),
    };
    m.global.containers[index::QUALITY_CHANGES] = Container::Instance(global_qc);
    let list = m.extruder_list();
    for (pos, &i) in list.iter().enumerate() {
        let quality = group.as_ref().and_then(|g| g.nodes_for_extruders.iter().find(|(p, _)| *p == pos).map(|(_, id)| instance_or_empty(&m.reg, id, "empty_quality"))).unwrap_or_else(|| eq.clone());
        let position = m.extruders[i].position().unwrap_or_default();
        let qc = extruder_qcs.iter().find(|(p, _)| *p == position).map(|(_, c)| c.clone()).unwrap_or_else(|| eqc.clone());
        m.extruders[i].containers[index::QUALITY] = Container::Instance(quality);
        m.extruders[i].containers[index::QUALITY_CHANGES] = Container::Instance(qc);
    }
    m.clear_caches();
    set_intent_by_category(m, tree, intent_category);
    m.clear_caches();
}

/// `MachineManager.setQualityGroupByQualityType`.
pub fn set_quality_type(m: &mut Machine, tree: &ContainerTree, quality_type: &str) -> bool {
    let groups = current_quality_groups(m, tree);
    match groups.get(quality_type) {
        Some(g) => {
            let g = g.clone();
            set_quality_group(m, tree, &g, true);
            true
        }
        None => false,
    }
}

/// `MachineManager._updateQualityWithMaterial`.
pub fn update_quality_with_material(m: &mut Machine, tree: &ContainerTree) {
    let current = m.global.container(index::QUALITY).meta_str("quality_type");
    let groups = current_quality_groups(m, tree);
    let mut available: Vec<String> = groups.items.iter().filter(|(_, g)| g.is_available).map(|(k, _)| k.clone()).collect();
    if !active_materials_compatible(m) {
        if current.is_some() {
            set_empty_quality(m, tree);
        }
        return;
    }
    if available.is_empty() {
        set_empty_quality(m, tree);
        return;
    }
    if let Some(c) = &current {
        if available.contains(c) {
            let g = groups.get(c).unwrap().clone();
            set_quality_group(m, tree, &g, false);
            return;
        }
    }
    available.sort();
    let mut qt = available[0].clone();
    let preferred = m.global.definition().metadata_str("preferred_quality_type");
    if let Some(p) = preferred {
        if available.contains(&p) {
            qt = p;
        }
    }
    let g = groups.get(&qt).unwrap().clone();
    set_quality_group(m, tree, &g, true);
}

/// `MachineManager._updateIntentWithQuality`.
pub fn update_intent_with_quality(m: &mut Machine, tree: &ContainerTree) {
    let mut category = "default".to_string();
    for i in m.extruder_list() {
        if !m.extruders[i].is_enabled() {
            continue;
        }
        let cur = m.extruders[i].container(index::INTENT).meta_str("intent_category").unwrap_or_else(|| "default".into());
        if cur != "default" && cur != category {
            category = cur;
            continue;
        }
        let qc = m.extruders[i].container(index::QUALITY_CHANGES);
        if qc.id() == "empty_quality_changes" {
            continue;
        }
        let cur = qc.meta_str("intent_category").unwrap_or_else(|| "default".into());
        if cur != "default" && cur != category {
            category = cur;
        }
    }
    set_intent_by_category(m, tree, &category);
}

/// `MachineManager.setIntentByCategory`.
pub fn set_intent_by_category(m: &mut Machine, tree: &ContainerTree, category: &str) {
    let node = tree.machine(&m.reg, &m.global.definition().id);
    let empty_intent = instance_or_empty(&m.reg, "empty_intent", "empty");
    for i in m.extruder_list() {
        let quality_id = m.extruders[i].container(index::QUALITY).id().to_string();
        if quality_id == "empty_quality" {
            m.extruders[i].containers[index::INTENT] = Container::Instance(empty_intent.clone());
            continue;
        }
        let variant_name = m.extruders[i].container(index::VARIANT).name();
        let base = m.extruders[i].container(index::MATERIAL).meta_str("base_file").unwrap_or_default();
        let Some(qn) = node.variants.get(&variant_name).and_then(|v| v.materials.get(&base)).and_then(|mn| mn.qualities.get(&quality_id)) else {
            continue;
        };
        let mut chosen = None;
        for intent in qn.intents.values() {
            if intent.intent_category == category {
                chosen = Some(intent.container_id.clone());
                break;
            }
        }
        m.extruders[i].containers[index::INTENT] = Container::Instance(match chosen {
            Some(id) => instance_or_empty(&m.reg, &id, "empty_intent"),
            None => empty_intent.clone(),
        });
    }
    m.clear_caches();
}

/// `MachineManager.setExtruderEnabled`.
pub fn set_extruder_enabled(m: &mut Machine, tree: &ContainerTree, position: usize, enabled: bool) {
    let list = m.extruder_list();
    let Some(&i) = list.get(position) else { return };
    m.extruders[i].set_meta("enabled", Value::str(if enabled { "True" } else { "False" }));
    update_default_extruder(m);
    update_number_extruders_enabled(m);
    correct_extruder_settings(m);
    update_quality_with_material(m, tree);
}

/// `MachineManager.correctExtruderSettings`: drop user values of extruder settings pointing at
/// missing or disabled extruders.
pub fn correct_extruder_settings(m: &mut Machine) {
    let user = m.global.user().clone();
    let count = m.get_value(StackId::Global, "machine_extruder_count").and_then(|v| v.as_f64()).unwrap_or(1.0) as i64;
    let list = m.extruder_list();
    for key in user.keys() {
        let ty = m.global.definition().find(&key).map(|d| d.setting_type()).unwrap_or_default();
        if ty != "extruder" && ty != "optional_extruder" {
            continue;
        }
        let v = match user.get_value(&key, Some(m.global.definition())) {
            Some(Prop::Value(v)) => v,
            Some(Prop::Func(f)) => m.call_function(&f, StackId::Global, None, None).unwrap_or(Value::Int(0)),
            None => continue,
        };
        let Some(n) = v.as_f64().map(|f| f as i64) else { continue };
        if n < 0 {
            continue;
        }
        if n >= count || list.get(n as usize).map(|&i| !m.extruders[i].is_enabled()).unwrap_or(true) {
            user.remove_value(&key);
        }
    }
    m.clear_caches();
}
