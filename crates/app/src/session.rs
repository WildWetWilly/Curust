//! The front-end state: printers (global + extruder stacks), the scene and the user's choices,
//! mirroring Cura's `MachineManager`, `ExtruderManager` and the settings panel behaviour.

use crate::scene::{self, Node};
use curust_settings::definition::{json_to_value, Prop};
use curust_settings::instance::{InstValue, InstanceContainer};
use curust_settings::json::Json;
use curust_settings::machine::{self, ContainerTree};
use curust_settings::py::Value;
use curust_settings::registry::Registry;
use curust_settings::stack::{index, Container, Machine, ObjectStack, StackId};
use std::path::PathBuf;
use std::rc::Rc;

/// Where a setting change goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Global,
    /// Extruder position.
    Extruder(usize),
    /// Scene node id.
    Object(u64),
}

#[derive(Clone, Debug)]
pub struct Prefs {
    /// `cura/jobname_prefix`.
    pub jobname_prefix: bool,
    /// Material costs: GUID → (spool cost, spool weight in g).
    pub material_costs: Vec<(String, f64, f64)>,
    /// Setting visibility: preset name ("basic", "advanced", "expert", "custom").
    pub visibility_preset: String,
    pub custom_visible: Vec<String>,
    /// Automatically drop models to the build plate.
    pub auto_drop: bool,
    /// Slice whenever the scene or the settings change (Cura's general/auto_slice, off by default).
    pub auto_slice: bool,
    pub currency: String,
}

/// A custom profile (Cura's quality_changes group): settings on top of a quality type, for one
/// printer definition (its `quality_definition`), with one container for the global stack and
/// one per extruder position.
#[derive(Clone)]
pub struct CustomProfile {
    pub name: String,
    pub definition: String,
    pub quality_type: String,
    pub intent_category: String,
    pub global: Rc<InstanceContainer>,
    pub extruders: Vec<(String, Rc<InstanceContainer>)>,
}

impl CustomProfile {
    pub fn containers(&self) -> impl Iterator<Item = &Rc<InstanceContainer>> {
        std::iter::once(&self.global).chain(self.extruders.iter().map(|(_, c)| c))
    }

    /// A profile from its containers (`MachineNode.getQualityChangesGroups`): the name, quality
    /// type and definition of the global container; the intent category is the first one that
    /// is not "default".
    pub fn from_containers(global: Rc<InstanceContainer>, mut extruders: Vec<(String, Rc<InstanceContainer>)>) -> CustomProfile {
        extruders.sort_by_key(|(p, _)| p.parse::<i64>().unwrap_or(0));
        let mut intent_category = "default".to_string();
        for c in std::iter::once(&global).chain(extruders.iter().map(|(_, c)| c)) {
            if intent_category == "default" {
                intent_category = c.meta_str("intent_category").unwrap_or_else(|| "default".into());
            }
        }
        CustomProfile {
            name: global.name.clone(),
            definition: global.definition.clone(),
            quality_type: global.meta_str("quality_type").unwrap_or_else(|| "not_supported".into()),
            intent_category,
            global,
            extruders,
        }
    }
}

/// A copy of a container with a new id and name (`InstanceContainer.duplicate`).
pub fn duplicate_container(c: &InstanceContainer, id: &str, name: &str) -> InstanceContainer {
    let n = InstanceContainer::new(id, name, &c.definition);
    let mut n = n;
    n.metadata = c.metadata.clone();
    copy_values(&n, c);
    n
}

/// `QualityManager._createQualityChanges`.
/// `CuraApplication.SettingVersion` of Cura 5.12.
pub const CURA_SETTING_VERSION: &str = "26";

pub fn new_quality_changes(id: &str, name: &str, definition: &str, quality_type: &str, intent_category: Option<&str>, position: Option<&str>) -> InstanceContainer {
    let mut c = InstanceContainer::new(id, name, definition);
    c.set_meta("type", Value::str("quality_changes"));
    c.set_meta("quality_type", Value::str(quality_type));
    if let Some(i) = intent_category {
        c.set_meta("intent_category", Value::str(i));
    }
    if let Some(p) = position {
        c.set_meta("position", Value::str(p));
    }
    c.set_meta("setting_version", Value::str(CURA_SETTING_VERSION));
    c
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { jobname_prefix: true, material_costs: Vec::new(), visibility_preset: "advanced".into(), custom_visible: Vec::new(), auto_drop: true, auto_slice: false, currency: "€".into() }
    }
}

pub struct Session {
    pub resources: PathBuf,
    pub reg: Rc<Registry>,
    pub tree: ContainerTree,
    pub machines: Vec<Machine>,
    pub active: Option<usize>,
    /// The extruder (position) the settings panel edits.
    pub active_extruder: usize,
    pub nodes: Vec<Node>,
    next_node_id: u64,
    /// File name the job name is derived from (`PrintInformation._base_name`).
    pub base_name: String,
    /// A job name the user typed.
    pub user_job_name: Option<String>,
    pub prefs: Prefs,
    /// Custom profiles of all printer definitions.
    pub profiles: Vec<CustomProfile>,
}

impl Session {
    /// Cura's resources, then the user's own (custom materials, Marketplace packages).
    pub fn resource_roots(resources: &std::path::Path) -> Vec<PathBuf> {
        let mut roots = vec![resources.to_path_buf()];
        if std::env::var_os("CURUST_NO_USER_RESOURCES").is_some() {
            return roots;
        }
        let user = crate::materials::user_resources_dir();
        if user.is_dir() && user != resources {
            roots.push(user);
        }
        // Resources inside installed plugins (printer definitions, materials, profiles), then
        // the materials and profiles the user installed in UltiMaker Cura.
        roots.extend(crate::marketplace::plugin_resource_roots());
        roots.extend(crate::marketplace::cura_data_dirs());
        roots
    }

    pub fn new(resources: PathBuf) -> Session {
        let reg = Rc::new(Registry::load(&Self::resource_roots(&resources)));
        Session {
            resources,
            reg,
            tree: ContainerTree::default(),
            machines: Vec::new(),
            active: None,
            active_extruder: 0,
            nodes: Vec::new(),
            next_node_id: 1,
            base_name: String::new(),
            user_job_name: None,
            prefs: Prefs::default(),
            profiles: Vec::new(),
        }
    }

    // ---------------------------------------------------------------------------------------
    // Custom profiles (quality_changes)
    // ---------------------------------------------------------------------------------------

    /// The definition custom profiles of the active printer belong to.
    pub fn quality_definition(&self) -> Option<String> {
        let m = self.machine()?;
        Some(self.tree.machine(&self.reg, &m.global.definition().id).quality_definition.clone())
    }

    /// Indices of the custom profiles usable with the active printer.
    pub fn profiles_for_active(&self) -> Vec<usize> {
        let Some(def) = self.quality_definition() else { return Vec::new() };
        (0..self.profiles.len()).filter(|&i| self.profiles[i].definition == def).collect()
    }

    /// The active custom profile, if any.
    pub fn active_profile(&self) -> Option<usize> {
        let m = self.machine()?;
        let qc = m.global.instance_at(index::QUALITY_CHANGES);
        self.profiles.iter().position(|p| Rc::ptr_eq(&p.global, qc))
    }

    /// `ContainerRegistry.uniqueName` over the registry and the custom profiles.
    fn unique_profile_name(&self, base: &str) -> String {
        self.unique_name(base)
    }

    fn unique_container_id(&self, base: &str) -> String {
        self.unique_name(&base.to_lowercase().replace(' ', "_"))
    }

    /// "Create profile from current settings/overrides" (`ContainerManager.createQualityChanges`):
    /// the user changes move into new quality_changes containers, which become active.
    pub fn create_profile(&mut self, name: &str) -> Result<usize, String> {
        let def = self.quality_definition().ok_or("No printer")?;
        let m = self.machine().ok_or("No printer")?;
        let quality = m.global.container(index::QUALITY);
        let quality_type = quality.meta_str("quality_type").unwrap_or_else(|| "not_supported".into());
        // `QualityManagementModel.createQualityChanges`: the name defaults to the active profile's.
        let mut base = name.trim().to_string();
        if base.is_empty() {
            base = match self.active_profile() {
                Some(i) => self.profiles[i].name.clone(),
                None => quality.name(),
            };
        }
        let unique = self.unique_profile_name(&base);
        let gid = self.unique_container_id(&format!("{}_{}", m.global.definition().id, unique));
        let global = new_quality_changes(&gid, &unique, &def, &quality_type, None, None);
        // Existing quality changes are copied, then the user changes move in.
        copy_values(&global, m.global.instance_at(index::QUALITY_CHANGES));
        merge_into(&global, m.global.user());
        let mut extruders = Vec::new();
        let mut intent_category = "default".to_string();
        for e in &m.extruders {
            let pos = e.position().unwrap_or_default();
            let eq = e.container(index::QUALITY).meta_str("quality_type").unwrap_or_else(|| "not_supported".into());
            let intent = e.container(index::INTENT).meta_str("intent_category").unwrap_or_else(|| "default".into());
            if intent_category == "default" {
                intent_category = intent.clone();
            }
            let eid = self.unique_container_id(&format!("{}_{}", e.id, unique));
            let c = new_quality_changes(&eid, &unique, &def, &eq, Some(&intent), Some(&pos));
            copy_values(&c, e.instance_at(index::QUALITY_CHANGES));
            merge_into(&c, e.user());
            extruders.push((pos, Rc::new(c)));
        }
        self.profiles.push(CustomProfile { name: unique, definition: def, quality_type, intent_category, global: Rc::new(global), extruders });
        let idx = self.profiles.len() - 1;
        self.activate_profile(idx)?;
        Ok(idx)
    }

    /// "Update profile with current settings/overrides".
    pub fn update_profile(&mut self) -> Result<(), String> {
        let idx = self.active_profile().ok_or("No custom profile is active")?;
        let m = self.machine().ok_or("No printer")?;
        let p = &self.profiles[idx];
        merge_into(&p.global, m.global.user());
        for e in &m.extruders {
            let pos = e.position().unwrap_or_default();
            if let Some((_, c)) = p.extruders.iter().find(|(q, _)| *q == pos) {
                merge_into(c, e.user());
            }
        }
        m.clear_caches();
        self.refresh_scene();
        Ok(())
    }

    pub fn activate_profile(&mut self, idx: usize) -> Result<(), String> {
        let i = self.active.ok_or("No printer")?;
        let p = self.profiles.get(idx).ok_or("Unknown profile")?.clone();
        machine::set_quality_changes(&mut self.machines[i], &self.tree, &p.quality_type, &p.intent_category, p.global.clone(), &p.extruders);
        self.refresh_scene();
        Ok(())
    }

    pub fn rename_profile(&mut self, idx: usize, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("The profile name is empty".into());
        }
        let old = self.profiles.get(idx).ok_or("Unknown profile")?.clone();
        if old.name == name {
            return Ok(());
        }
        let unique = self.unique_profile_name(name);
        // Containers are shared with the printers' stacks: rebuild them with the new name and
        // swap them in wherever the old ones are used.
        let rebuild = |c: &InstanceContainer, _pos: Option<&str>| Rc::new(duplicate_container(c, &c.id, &unique));
        let global = rebuild(&old.global, None);
        let extruders: Vec<(String, Rc<InstanceContainer>)> = old.extruders.iter().map(|(p, c)| (p.clone(), rebuild(c, Some(p)))).collect();
        for m in &mut self.machines {
            if Rc::ptr_eq(m.global.instance_at(index::QUALITY_CHANGES), &old.global) {
                m.global.containers[index::QUALITY_CHANGES] = Container::Instance(global.clone());
            }
            for e in &mut m.extruders {
                for ((_, oc), (_, nc)) in old.extruders.iter().zip(&extruders) {
                    if Rc::ptr_eq(e.instance_at(index::QUALITY_CHANGES), oc) {
                        e.containers[index::QUALITY_CHANGES] = Container::Instance(nc.clone());
                    }
                }
            }
            m.clear_caches();
        }
        let p = &mut self.profiles[idx];
        p.name = unique;
        p.global = global;
        p.extruders = extruders;
        Ok(())
    }

    pub fn duplicate_profile(&mut self, idx: usize, name: &str) -> Result<usize, String> {
        let src = self.profiles.get(idx).ok_or("Unknown profile")?.clone();
        let unique = self.unique_profile_name(name);
        let copy = |c: &InstanceContainer, id: String, _pos: Option<&str>| Rc::new(duplicate_container(c, &id, &unique));
        let gid = self.unique_container_id(&src.global.id);
        let global = copy(&src.global, gid, None);
        let mut extruders = Vec::new();
        for (pos, c) in &src.extruders {
            let id = self.unique_container_id(&c.id);
            extruders.push((pos.clone(), copy(c, id, Some(pos))));
        }
        self.profiles.push(CustomProfile { name: unique, definition: src.definition.clone(), quality_type: src.quality_type.clone(), intent_category: src.intent_category.clone(), global, extruders });
        Ok(self.profiles.len() - 1)
    }

    /// Remove a custom profile; printers using it fall back to its quality type.
    pub fn delete_profile(&mut self, idx: usize) -> Result<(), String> {
        let p = self.profiles.get(idx).ok_or("Unknown profile")?.clone();
        let active = self.active;
        for i in 0..self.machines.len() {
            if Rc::ptr_eq(self.machines[i].global.instance_at(index::QUALITY_CHANGES), &p.global) {
                self.active = Some(i);
                if !self.set_quality(&p.quality_type) {
                    machine::update_quality_with_material(&mut self.machines[i], &self.tree);
                }
            }
        }
        self.active = active;
        self.profiles.remove(idx);
        self.refresh_scene();
        Ok(())
    }

    pub fn machine(&self) -> Option<&Machine> {
        self.active.and_then(|i| self.machines.get(i))
    }

        pub fn require_machine(&self) -> Result<&Machine, String> {
        self.machine().ok_or_else(|| "No printer has been added yet".to_string())
    }

    // ---------------------------------------------------------------------------------------
    // Printers
    // ---------------------------------------------------------------------------------------

    /// All machine definitions a user can add, as (id, name, manufacturer).
    pub fn available_definitions(&self) -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        for e in self.reg.all() {
            if !matches!(e.source, curust_settings::registry::Source::Definition(_)) {
                continue;
            }
            if e.id == "fdmprinter" || e.id == "fdmextruder" {
                continue;
            }
            if e.get_str("type").as_deref() == Some("extruder") || e.get_str("position").is_some() {
                continue;
            }
            if e.get("visible").map(|v| !curust_settings::stack::parse_bool(v)).unwrap_or(false) {
                continue;
            }
            let name = e.get_str("name").unwrap_or_else(|| e.id.clone());
            let manufacturer = e.get_str("manufacturer").unwrap_or_else(|| "Unknown".into());
            out.push((e.id.clone(), name, manufacturer));
        }
        out.sort_by(|a, b| a.2.to_lowercase().cmp(&b.2.to_lowercase()).then(a.1.to_lowercase().cmp(&b.1.to_lowercase())));
        out
    }

    fn unique_machine_name(&self, base: &str) -> String {
        let base = base.trim();
        let taken = |n: &str| self.machines.iter().any(|m| m.global.name == n);
        if !taken(base) {
            return base.to_string();
        }
        let mut i = 2;
        loop {
            let n = format!("{} #{}", base, i);
            if !taken(&n) {
                return n;
            }
            i += 1;
        }
    }

    /// `CuraStackBuilder.createMachine` + `setActiveMachine`.
    pub fn add_machine(&mut self, definition_id: &str, name: Option<&str>) -> Result<usize, String> {
        let def = self.reg.definition(definition_id).ok_or_else(|| format!("Unknown printer definition {}", definition_id))?;
        let name = self.unique_machine_name(name.filter(|n| !n.trim().is_empty()).unwrap_or(&def.name));
        let m = machine::create_machine(&self.reg, &self.tree, &name, definition_id)?;
        self.machines.push(m);
        let idx = self.machines.len() - 1;
        self.set_active_machine(idx);
        Ok(idx)
    }

    pub fn set_active_machine(&mut self, idx: usize) {
        if idx >= self.machines.len() {
            return;
        }
        self.active = Some(idx);
        self.active_extruder = 0;
        // Models keep their extruder if it exists on the new printer.
        let count = self.extruder_count();
        for n in self.nodes.iter_mut() {
            if n.extruder >= count {
                n.extruder = 0;
            }
        }
        self.refresh_scene();
    }

    pub fn remove_machine(&mut self, idx: usize) {
        if idx >= self.machines.len() {
            return;
        }
        self.machines.remove(idx);
        self.active = if self.machines.is_empty() {
            None
        } else {
            match self.active {
                Some(a) if a > idx => Some(a - 1),
                Some(a) if a == idx => Some(0),
                other => other,
            }
        };
    }

    pub fn rename_machine(&mut self, idx: usize, name: &str) {
        let name = self.unique_machine_name(name);
        if let Some(m) = self.machines.get_mut(idx) {
            m.global.name = name;
        }
    }

    /// Number of extruders in the active machine (`extruderList`).
    pub fn extruder_count(&self) -> usize {
        self.machine().map(|m| m.extruder_list().len()).unwrap_or(1).max(1)
    }

    /// Extruder index (into `Machine::extruders`) of an extruder position.
    pub fn global_value(&self, key: &str) -> Option<Value> {
        self.machine()?.get_value(StackId::Global, key)
    }

    pub fn global_f64(&self, key: &str, default: f64) -> f64 {
        self.global_value(key).and_then(|v| v.as_f64()).unwrap_or(default)
    }

    // ---------------------------------------------------------------------------------------
    // Profiles
    // ---------------------------------------------------------------------------------------

    pub fn set_variant(&mut self, position: usize, variant_name: &str) -> bool {
        let Some(i) = self.active else { return false };
        let ok = machine::set_variant_by_name(&mut self.machines[i], &self.tree, &position.to_string(), variant_name);
        self.refresh_scene();
        ok
    }

    pub fn set_material(&mut self, position: usize, base_file: &str) -> bool {
        let Some(i) = self.active else { return false };
        let ok = machine::set_material_by_id(&mut self.machines[i], &self.tree, &position.to_string(), base_file);
        self.refresh_scene();
        ok
    }

    pub fn set_quality(&mut self, quality_type: &str) -> bool {
        let Some(i) = self.active else { return false };
        let ok = machine::set_quality_type(&mut self.machines[i], &self.tree, quality_type);
        ok
    }

    pub fn set_intent(&mut self, category: &str) {
        if let Some(i) = self.active {
            machine::set_intent_by_category(&mut self.machines[i], &self.tree, category);
        }
    }

    pub fn set_extruder_enabled(&mut self, position: usize, enabled: bool) {
        if let Some(i) = self.active {
            machine::set_extruder_enabled(&mut self.machines[i], &self.tree, position, enabled);
        }
        self.refresh_scene();
    }

    /// Discard all user changes (`MachineManager.clearUserSettingAllCurrentStacks`).
    pub fn clear_user_settings(&mut self) {
        if let Some(m) = self.machine() {
            clear_container(m.global.user());
            for e in &m.extruders {
                clear_container(e.user());
            }
            m.clear_caches();
        }
        self.refresh_scene();
    }

    // ---------------------------------------------------------------------------------------
    // Settings
    // ---------------------------------------------------------------------------------------

    /// The stack the settings panel uses for a setting (SettingView's `containerStackId`).
    pub fn stack_for_setting(&self, key: &str) -> StackId {
        let Some(m) = self.machine() else { return StackId::Global };
        let settable = m.get_property(StackId::Global, key, "settable_per_extruder", None).map(|v| v.truthy()).unwrap_or(false);
        if !settable {
            return StackId::Global;
        }
        let limit = m.get_property(StackId::Global, key, "limit_to_extruder", None).and_then(|v| v.as_f64().or_else(|| v.py_str().parse().ok())).unwrap_or(-1.0);
        let list = m.extruder_list();
        if limit >= 0.0 {
            if let Some(&i) = list.get(limit as usize) {
                return StackId::Extruder(i);
            }
        }
        list.get(self.active_extruder).or(list.first()).map(|&i| StackId::Extruder(i)).unwrap_or(StackId::Global)
    }

    pub fn target_stack(&self, target: Target) -> Option<StackId> {
        let m = self.machine()?;
        match target {
            Target::Global => Some(StackId::Global),
            Target::Extruder(p) => m.extruder_list().get(p).map(|&i| StackId::Extruder(i)),
            Target::Object(id) => self.nodes.iter().position(|n| n.id == id).map(StackId::Object),
        }
    }

    /// Set a setting the way Cura's settings panel does (`SettingPropertyProvider.setPropertyValue`):
    /// a value equal to what the stack would give without it removes the user value instead.
    /// `value` is the text from the panel (`=...` makes a formula); booleans and option keys are
    /// stored typed like the QML controls do.
    pub fn set_setting(&mut self, target: Target, key: &str, value: &str) -> Result<(), String> {
        if let Target::Object(id) = target {
            let node = self.nodes.iter_mut().find(|n| n.id == id).ok_or("Unknown object")?;
            if let Some(slot) = node.settings.iter_mut().find(|(k, _)| k == key) {
                slot.1 = value.to_string();
            } else {
                node.settings.push((key.to_string(), value.to_string()));
            }
            self.refresh_scene();
            return Ok(());
        }
        let m = self.machine().ok_or("No printer")?;
        let def = m.global.definition().clone();
        let sd = def.find(key).ok_or_else(|| format!("Unknown setting {}", key))?;
        let ty = sd.setting_type();
        let s = match target {
            Target::Global => StackId::Global,
            Target::Extruder(p) => *m.extruder_list().get(p).map(|i| StackId::Extruder(*i)).as_ref().ok_or("Unknown extruder")?,
            Target::Object(_) => unreachable!(),
        };
        let stack = m.stack(s);
        let user = stack.user().clone();
        let new_value: InstValue = if value.trim().starts_with('=') {
            InstValue::from_str(value)
        } else {
            match ty.as_str() {
                "bool" => InstValue::Typed(Value::Bool(matches!(value, "true" | "True" | "1"))),
                "extruder" | "optional_extruder" => match value.trim().parse::<i64>() {
                    Ok(n) => InstValue::Typed(Value::Int(n)),
                    Err(_) => InstValue::Str(value.to_string()),
                },
                _ => InstValue::Str(value.to_string()),
            }
        };
        // `_remove_unused_value`: compare with the first deeper container that has a value.
        let mut deeper: Option<String> = None;
        for c in stack.containers.iter().skip(1) {
            let prop = match c {
                Container::Instance(i) => {
                    if i.is_empty {
                        None
                    } else {
                        let d = m.reg.definition(&i.definition);
                        i.get_value(key, d.as_deref())
                    }
                }
                Container::Definition(d) => d.get_property(key, "value"),
            };
            if let Some(p) = prop {
                let v = match p {
                    Prop::Value(v) => v,
                    Prop::Func(f) => m.call_function(&f, s, None, None).unwrap_or(Value::None),
                };
                deeper = Some(v.py_str());
                break;
            }
        }
        let new_str = match &new_value {
            InstValue::Str(t) => t.clone(),
            InstValue::Typed(v) => v.py_str(),
            InstValue::Func(f) => f.to_string(),
        };
        if deeper.as_deref() == Some(new_str.as_str()) {
            user.remove_value(key);
        } else {
            user.set_value(key, new_value);
        }
        m.clear_caches();
        if key == "machine_extruder_count" || key.ends_with("extruder_nr") {
            if let Some(i) = self.active {
                machine::correct_extruder_settings(&mut self.machines[i]);
            }
        }
        self.refresh_scene();
        Ok(())
    }

    /// Store a typed value (JSON text; strings starting with `=` become formulas) directly in a
    /// user container, without the settings panel's clean-up.
    pub fn set_raw(&mut self, target: Target, key: &str, json_text: &str) -> Result<(), String> {
        let j = curust_settings::json::parse(json_text).map_err(|e| format!("{}: {}", key, e))?;
        let v = match &j {
            Json::Str(t) if t.trim().starts_with('=') => InstValue::from_str(t),
            other => InstValue::Typed(json_to_value(other)),
        };
        if let Target::Object(id) = target {
            let node = self.nodes.iter_mut().find(|n| n.id == id).ok_or("Unknown object")?;
            node.settings.retain(|(k, _)| k != key);
            node.settings.push((key.to_string(), v.serialize()));
            self.refresh_scene();
            return Ok(());
        }
        let m = self.machine().ok_or("No printer")?;
        let s = self.target_stack(target).ok_or("Unknown target")?;
        m.stack(s).user().set_value(key, v);
        m.clear_caches();
        self.refresh_scene();
        Ok(())
    }

    pub fn reset_setting(&mut self, target: Target, key: &str) {
        match target {
            Target::Object(id) => {
                if let Some(n) = self.nodes.iter_mut().find(|n| n.id == id) {
                    n.settings.retain(|(k, _)| k != key);
                }
            }
            _ => {
                if let (Some(m), Some(s)) = (self.machine(), self.target_stack(target)) {
                    m.stack(s).user().remove_value(key);
                    m.clear_caches();
                }
            }
        }
        self.refresh_scene();
    }

    /// Machine settings (Cura's "Machine Settings" dialog) live in the definition-changes
    /// containers.
    pub fn set_machine_setting(&mut self, target: Target, key: &str, value: &str) -> Result<(), String> {
        let m = self.machine().ok_or("No printer")?;
        let s = self.target_stack(target).ok_or("Unknown target")?;
        let dc = m.stack(s).instance_at(index::DEFINITION_CHANGES).clone();
        if dc.is_empty {
            return Err("This printer has no machine settings container".into());
        }
        let ty = m.setting_definition(s, key).map(|d| d.setting_type()).ok_or_else(|| format!("Unknown setting {}", key))?;
        let v = match ty.as_str() {
            "bool" => InstValue::Typed(Value::Bool(matches!(value, "true" | "True" | "1"))),
            _ if value.trim().starts_with('=') => InstValue::from_str(value),
            _ => InstValue::Str(value.to_string()),
        };
        dc.set_value(key, v);
        m.clear_caches();
        if let Some(i) = self.active {
            if key == "machine_extruder_count" {
                let mm = &mut self.machines[i];
                machine::update_default_extruder(mm);
                machine::update_number_extruders_enabled(mm);
                machine::correct_extruder_settings(mm);
                machine::update_quality_with_material(mm, &self.tree);
            }
        }
        self.refresh_scene();
        Ok(())
    }

    // ---------------------------------------------------------------------------------------
    // Scene
    // ---------------------------------------------------------------------------------------

    pub fn machine_size(&self) -> (f64, f64, f64) {
        (self.global_f64("machine_width", 200.0), self.global_f64("machine_depth", 200.0), self.global_f64("machine_height", 200.0))
    }

    /// Load a model file into the scene (`CuraApplication._readMeshFinished`).
    pub fn add_model_data(&mut self, file_name: &str, data: &[u8]) -> Result<Vec<u64>, String> {
        let (w, d, _) = self.machine_size();
        let models = crate::mesh_io::load_model_data(file_name, data, w, d)?;
        self.add_loaded_models(file_name, models)
    }

    /// An image as a relief / lithophane (Cura's ImageReader).
    pub fn add_image_data(&mut self, file_name: &str, data: &[u8], opts: &crate::image_import::ImageOptions) -> Result<Vec<u64>, String> {
        let model = crate::image_import::image_model(file_name, data, opts)?;
        self.add_loaded_models(file_name, vec![model])
    }

    fn add_loaded_models(&mut self, file_name: &str, models: Vec<crate::mesh_io::LoadedModel>) -> Result<Vec<u64>, String> {
        let default_extruder = self.machine().map(|m| m.default_extruder_position.borrow().parse::<usize>().unwrap_or(0)).unwrap_or(0);
        let mut new_idx = Vec::new();
        let mut ids = Vec::new();
        let is_3mf = file_name.to_ascii_lowercase().ends_with(".3mf");
        for model in models {
            let id = self.next_node_id;
            self.next_node_id += 1;
            let mut settings = model.settings.clone();
            let mut extruder = default_extruder;
            if let Some(p) = settings.iter().position(|(k, _)| k == "extruder_nr") {
                extruder = settings[p].1.trim().parse().unwrap_or(default_extruder);
                settings.remove(p);
            }
            settings.retain(|(k, _)| self.machine().map(|m| m.global.definition().has_key(k)).unwrap_or(false));
            let mut lm = model;
            lm.settings = settings;
            let node = scene::node_from_model(id, lm, extruder.min(self.extruder_count() - 1), file_name);
            self.nodes.push(node);
            new_idx.push(self.nodes.len() - 1);
            ids.push(id);
        }
        if !is_3mf {
            let (w, d, _) = self.machine_size();
            scene::arrange(&mut self.nodes, &new_idx, w, d, 3.0);
            for &i in &new_idx {
                self.nodes[i].drop_to_plate();
            }
        }
        if self.base_name.is_empty() {
            let stem = std::path::Path::new(file_name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            self.base_name = stem;
        }
        self.refresh_scene();
        Ok(ids)
    }

    pub fn node_mut(&mut self, id: u64) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    pub fn remove_node(&mut self, id: u64) {
        // Children (support blockers) go with their model.
        let mut gone = vec![id];
        let mut k = 0;
        while k < gone.len() {
            let p = gone[k];
            gone.extend(self.nodes.iter().filter(|n| n.parent == Some(p)).map(|n| n.id));
            k += 1;
        }
        self.nodes.retain(|n| !gone.contains(&n.id));
        if self.nodes.is_empty() {
            self.base_name.clear();
        }
        self.refresh_scene();
    }

    pub fn clear_scene(&mut self) {
        self.nodes.clear();
        self.base_name.clear();
        self.refresh_scene();
    }

    pub fn duplicate_node(&mut self, id: u64, copies: usize) -> Vec<u64> {
        let Some(src) = self.nodes.iter().find(|n| n.id == id).cloned() else { return Vec::new() };
        let mut idx = Vec::new();
        let mut ids = Vec::new();
        let children: Vec<Node> = self.nodes.iter().filter(|n| n.parent == Some(id)).cloned().collect();
        for _ in 0..copies {
            let mut n = src.clone();
            n.id = self.next_node_id;
            self.next_node_id += 1;
            ids.push(n.id);
            let new_parent = n.id;
            self.nodes.push(n);
            idx.push(self.nodes.len() - 1);
            // `copy.deepcopy(node)` copies the children too.
            for c in &children {
                let mut c = c.clone();
                c.id = self.next_node_id;
                self.next_node_id += 1;
                c.parent = Some(new_parent);
                self.nodes.push(c);
            }
        }
        let (w, d, _) = self.machine_size();
        scene::arrange(&mut self.nodes, &idx, w, d, 3.0);
        self.refresh_scene();
        ids
    }

    /// Re-arrange all models on the build plate.
    pub fn arrange_all(&mut self) {
        let (w, d, _) = self.machine_size();
        let all: Vec<usize> = (0..self.nodes.len()).filter(|&i| self.nodes[i].parent.is_none()).collect();
        scene::arrange(&mut self.nodes, &all, w, d, 3.0);
        self.refresh_scene();
    }

    /// Node indices in scene order (`DepthFirstIterator`): each model followed by its children.
    pub fn dfs_order(&self) -> Vec<usize> {
        fn visit(s: &Session, parent: Option<u64>, out: &mut Vec<usize>) {
            for (i, n) in s.nodes.iter().enumerate() {
                if n.parent == parent {
                    out.push(i);
                    visit(s, Some(n.id), out);
                }
            }
        }
        let mut out = Vec::with_capacity(self.nodes.len());
        visit(self, None, &mut out);
        // Orphans (should not happen) last.
        for i in 0..self.nodes.len() {
            if !out.contains(&i) {
                out.push(i);
            }
        }
        out
    }

    /// `SceneNode.getAllChildren` of a node (children, then their children).
    pub fn all_children(&self, idx: usize) -> Vec<usize> {
        let id = self.nodes[idx].id;
        let direct: Vec<usize> = (0..self.nodes.len()).filter(|&i| self.nodes[i].parent == Some(id)).collect();
        let mut out = direct.clone();
        for c in direct {
            out.extend(self.all_children(c));
        }
        out
    }

    /// Update the world transformation of child nodes after their parents changed.
    fn update_child_transforms(&mut self) {
        for _ in 0..4 {
            let mut changed = false;
            for i in 0..self.nodes.len() {
                let (Some(pid), Some(local)) = (self.nodes[i].parent, self.nodes[i].local) else { continue };
                let Some(pw) = self.nodes.iter().find(|n| n.id == pid).map(|n| n.world()) else { continue };
                let w = scene::mat4_mul(&pw, &local);
                if self.nodes[i].world_cache != Some(w) {
                    self.nodes[i].world_cache = Some(w);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// Whether a node is a normal model (support blockers can only be added to those).
    pub fn is_normal_mesh(&self, idx: usize) -> bool {
        let n = &self.nodes[idx];
        !n.settings.iter().any(|(k, v)| matches!(k.as_str(), "support_mesh" | "infill_mesh" | "cutting_mesh" | "anti_overhang_mesh") && (v == "True" || v == "true"))
    }

    /// `SupportEraser._createEraserMesh`: a 10 mm cube "Eraser" with `anti_overhang_mesh`, a
    /// child of `parent_id`, centred on `position` (scene coordinates, Y up).
    pub fn add_support_blocker(&mut self, parent_id: u64, position: [f64; 3]) -> Result<u64, String> {
        let pi = self.nodes.iter().position(|n| n.id == parent_id).ok_or("Unknown model")?;
        if !self.is_normal_mesh(pi) {
            return Err("Support blockers can only be added to normal models".into());
        }
        // AddSceneNodeOperation (at the root, identity), SetParentOperation (keeps the world
        // transformation: local = parent⁻¹), TranslateOperation(set_position) in world space.
        let p = self.nodes[pi].world();
        let inv = scene::mat4_inverse(&p).ok_or("The model's transformation is singular")?;
        let mut local = scene::mat4_mul(&inv, &scene::identity());
        let w = scene::mat4_mul(&p, &local);
        let derived = [w[0][3], w[1][3], w[2][3]];
        let same = (0..3).all(|k| (derived[k] - position[k]).abs() < 1e-6);
        if !same {
            let d = [position[0] - derived[0], position[1] - derived[1], position[2] - derived[2]];
            let winv = scene::mat4_inverse(&w).ok_or("singular")?;
            local = scene::mat4_mul(&scene::mat4_mul(&scene::mat4_mul(&local, &winv), &scene::translation(d)), &w);
        }
        let id = self.next_node_id;
        self.next_node_id += 1;
        let world = scene::mat4_mul(&p, &local);
        self.nodes.push(Node {
            id,
            name: "Eraser".into(),
            mesh: std::sync::Arc::new(scene::MeshData::new(scene::eraser_cube(10.0))),
            position,
            orientation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            scale: [1.0; 3],
            extruder: 0,
            settings: vec![("anti_overhang_mesh".into(), "True".into())],
            source_file: String::new(),
            outside: false,
            parent: Some(parent_id),
            local: Some(local),
            world_cache: Some(world),
        });
        self.refresh_scene();
        Ok(id)
    }

    /// Move a child node to a world position (`TranslateOperation` in world space).
    pub fn move_child(&mut self, id: u64, position: [f64; 3]) {
        let Some(i) = self.nodes.iter().position(|n| n.id == id) else { return };
        let (Some(local), Some(w)) = (self.nodes[i].local, self.nodes[i].world_cache) else { return };
        let d = [position[0] - w[0][3], position[1] - w[1][3], position[2] - w[2][3]];
        if let Some(winv) = scene::mat4_inverse(&w) {
            self.nodes[i].local = Some(scene::mat4_mul(&scene::mat4_mul(&scene::mat4_mul(&local, &winv), &scene::translation(d)), &w));
        }
        self.refresh_scene();
    }

    /// `ObjectsModel._renameNodes`: models with the same name get "(n)" suffixes.
    fn rename_nodes(&mut self) {
        let parse = |name: &str| -> (String, u64) {
            if let Some(stripped) = name.strip_suffix(')') {
                if let Some(p) = stripped.rfind('(') {
                    let digits = &stripped[p + 1..];
                    if p > 0 && !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
                        if let Ok(n) = digits.parse::<u64>() {
                            return (stripped[..p].to_string(), n);
                        }
                    }
                }
            }
            (name.to_string(), 0)
        };
        let mut groups: Vec<(String, Vec<u64>, Vec<usize>)> = Vec::new();
        for i in self.dfs_order() {
            let n = &self.nodes[i];
            let (orig, idx) = parse(&n.name);
            let g = match groups.iter().position(|(o, _, _)| *o == orig) {
                Some(g) => g,
                None => {
                    groups.push((orig.clone(), Vec::new(), Vec::new()));
                    groups.len() - 1
                }
            };
            if groups[g].1.contains(&idx) {
                groups[g].2.push(i);
            } else {
                groups[g].1.push(idx);
            }
        }
        for (orig, used, rename) in groups {
            let mut current = 0;
            for i in rename {
                current += 1;
                while used.contains(&current) {
                    current += 1;
                }
                self.nodes[i].name = format!("{}({})", orig, current);
            }
        }
    }

    /// Rebuild the per-object stacks and the outside-build-volume flags after any change.
    pub fn refresh_scene(&mut self) {
        self.update_child_transforms();
        self.rename_nodes();
        let Some(ai) = self.active else { return };
        let def_id = self.machines[ai].global.definition().id.clone();
        let list = self.machines[ai].extruder_list();
        let mut stacks = Vec::with_capacity(self.nodes.len());
        for n in &self.nodes {
            let user = InstanceContainer::new(&format!("SettingOverrideInstanceContainer-{}", n.id), "", &def_id);
            for (k, v) in &n.settings {
                let iv = if v.trim().starts_with('=') {
                    InstValue::from_str(v)
                } else {
                    match v.as_str() {
                        "True" | "true" => InstValue::Typed(Value::Bool(true)),
                        "False" | "false" => InstValue::Typed(Value::Bool(false)),
                        _ => InstValue::Str(v.clone()),
                    }
                };
                user.set_value(k, iv);
            }
            let ext = list.get(n.extruder).or(list.first()).copied().unwrap_or(0);
            stacks.push(ObjectStack { user: Rc::new(user), extruder_position: ext });
        }
        self.machines[ai].object_stacks = stacks;
        let flags = crate::buildvolume::outside_flags(self);
        for (n, f) in self.nodes.iter_mut().zip(flags) {
            n.outside = f;
        }
    }

    // ---------------------------------------------------------------------------------------
    // Job name (`PrintInformation`)
    // ---------------------------------------------------------------------------------------

    /// `MachineManager.getAbbreviatedMachineName`.
    pub fn abbreviated_machine_name(&self) -> String {
        let Some(m) = self.machine() else { return String::new() };
        abbreviate(&m.global.definition().name)
    }

    pub fn job_name(&self) -> String {
        if let Some(n) = &self.user_job_name {
            return n.clone();
        }
        if self.base_name.is_empty() {
            return "Untitled".into();
        }
        if self.prefs.jobname_prefix {
            let abbr = self.abbreviated_machine_name();
            if self.base_name.starts_with(&format!("{}_", abbr)) {
                return self.base_name.clone();
            }
            return format!("{}_{}", abbr, self.base_name);
        }
        self.base_name.clone()
    }
}

fn copy_values(target: &InstanceContainer, source: &InstanceContainer) {
    for k in source.keys() {
        if let Some(v) = source.raw_value(&k) {
            target.set_value(&k, v);
        }
    }
}

/// `ContainerManager._performMerge`: move the values of `source` into `target`.
fn merge_into(target: &InstanceContainer, source: &InstanceContainer) {
    for k in source.keys() {
        if let Some(v) = source.raw_value(&k) {
            target.set_value(&k, v);
        }
        source.remove_value(&k);
    }
}

fn clear_container(c: &InstanceContainer) {
    for k in c.keys() {
        c.remove_value(&k);
    }
}

/// `getAbbreviatedMachineName`: words of `[\w']+`; "ultimaker" → UM, digits kept, words longer
/// than three characters reduced to their first (accent-stripped, upper-case) letter.
pub fn abbreviate(name: &str) -> String {
    let mut out = String::new();
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if word.is_empty() {
            return;
        }
        if word.to_lowercase() == "ultimaker" {
            out.push_str("UM");
        } else if word.chars().all(|c| c.is_ascii_digit()) {
            out.push_str(word);
        } else {
            let stripped: String = word.to_uppercase().chars().map(strip_accent).collect();
            if stripped.chars().count() > 3 {
                out.push(stripped.chars().next().unwrap());
            } else {
                out.push_str(&stripped);
            }
        }
        word.clear();
    };
    for c in name.chars() {
        if c.is_alphanumeric() || c == '_' || c == '\'' {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
        }
    }
    flush(&mut word, &mut out);
    out
}

fn strip_accent(c: char) -> char {
    match c {
        'À'..='Å' => 'A',
        'Ç' => 'C',
        'È'..='Ë' => 'E',
        'Ì'..='Ï' => 'I',
        'Ñ' => 'N',
        'Ò'..='Ö' => 'O',
        'Ù'..='Ü' => 'U',
        'Ý' => 'Y',
        _ => c,
    }
}

/// A JSON value for a setting value (for the web front-end).
pub fn value_to_json(v: &Value) -> Json {
    match v {
        Value::None => Json::Null,
        Value::Bool(b) => Json::Bool(*b),
        Value::Int(i) => Json::Int(*i),
        Value::Float(f) => {
            if f.is_finite() {
                Json::Float(*f)
            } else {
                Json::Str(v.py_str())
            }
        }
        Value::Str(s) => Json::Str(s.to_string()),
        Value::List(_) | Value::Tuple(_) => Json::Array(v.iter_items().unwrap_or_default().iter().map(value_to_json).collect()),
        other => Json::Str(other.py_str()),
    }
}

