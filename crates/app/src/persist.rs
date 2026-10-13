//! Saving and restoring the user's printers and preferences (`<config>/curust.json`).

use crate::resources::config_dir;
use crate::session::{new_quality_changes, CustomProfile, Prefs, Session};
use std::rc::Rc;
use curust_settings::instance::{InstValue, InstanceContainer};
use curust_settings::json::{self, Json};
use curust_settings::machine;
use curust_settings::stack::{index, Machine, StackId};

fn container_json(c: &InstanceContainer) -> Json {
    if c.is_empty {
        return Json::Object(Vec::new());
    }
    Json::Object(c.values.borrow().iter().map(|(k, v)| (k.clone(), Json::Str(v.serialize()))).collect())
}

fn restore_container(c: &InstanceContainer, j: Option<&Json>) {
    if c.is_empty {
        return;
    }
    if let Some(Json::Object(items)) = j {
        for (k, v) in items {
            if let Some(s) = v.as_str() {
                c.set_value(k, InstValue::from_str(s));
            }
        }
    }
}

/// A custom profile is saved as its containers in Cura's `.inst.cfg` format.
fn profile_json(p: &CustomProfile) -> Json {
    Json::Object(vec![
        ("name".into(), Json::Str(p.name.clone())),
        (
            "containers".into(),
            Json::Array(
                p.containers()
                    .map(|c| Json::Object(vec![("id".into(), Json::Str(c.id.clone())), ("cfg".into(), Json::Str(c.serialize()))]))
                    .collect(),
            ),
        ),
    ])
}

fn restore_profile_containers(list: &[Json]) -> Option<CustomProfile> {
    let mut global = None;
    let mut extruders = Vec::new();
    for c in list {
        let id = c.get("id")?.as_str()?;
        let cfg = c.get("cfg")?.as_str()?;
        let container = InstanceContainer::from_cfg(id, cfg).ok()?;
        match container.meta_str("position") {
            Some(p) if !p.is_empty() && p != "None" => extruders.push((p, Rc::new(container))),
            _ => global = Some(Rc::new(container)),
        }
    }
    Some(CustomProfile::from_containers(global?, extruders))
}

fn restore_profile(j: &Json) -> Option<CustomProfile> {
    if let Some(Json::Array(list)) = j.get("containers") {
        return restore_profile_containers(list);
    }
    // Version 2 files: values per container.
    let st = |k: &str| j.get(k).and_then(|v| v.as_str()).map(String::from);
    let (name, definition, quality_type) = (st("name")?, st("definition")?, st("quality_type")?);
    let intent = st("intent_category").unwrap_or_else(|| "default".into());
    let global = new_quality_changes(&st("global_id").unwrap_or_else(|| format!("{}_{}", definition, name)), &name, &definition, &quality_type, None, None);
    restore_container(&global, j.get("global"));
    let mut extruders = Vec::new();
    if let Some(Json::Array(a)) = j.get("extruders") {
        for e in a {
            let pos = e.get("position").and_then(|v| v.as_str()).unwrap_or("0").to_string();
            let id = e.get("id").and_then(|v| v.as_str()).map(String::from).unwrap_or_else(|| format!("{}_{}_{}", definition, pos, name));
            let eintent = e.get("intent_category").and_then(|v| v.as_str()).unwrap_or("default").to_string();
            let c = new_quality_changes(&id, &name, &definition, &quality_type, Some(&eintent), Some(&pos));
            restore_container(&c, e.get("values"));
            extruders.push((pos, Rc::new(c)));
        }
    }
    Some(CustomProfile { name, definition, quality_type, intent_category: intent, global: Rc::new(global), extruders })
}

fn machine_json(s: &Session, m: &Machine) -> Json {
    let q = m.global.container(index::QUALITY).meta_str("quality_type").unwrap_or_default();
    let mut extruders = Vec::new();
    for e in &m.extruders {
        let intent = e.container(index::INTENT).meta_str("intent_category").unwrap_or_else(|| "default".into());
        extruders.push(Json::Object(vec![
            ("position".into(), Json::Str(e.position().unwrap_or_default())),
            ("enabled".into(), Json::Bool(e.is_enabled())),
            ("variant".into(), Json::Str(e.container(index::VARIANT).name())),
            ("material".into(), Json::Str(e.container(index::MATERIAL).meta_str("base_file").unwrap_or_default())),
            ("intent".into(), Json::Str(intent)),
            ("definition_changes".into(), container_json(e.instance_at(index::DEFINITION_CHANGES))),
            ("user".into(), container_json(e.user())),
        ]));
    }
    Json::Object(vec![
        ("name".into(), Json::Str(m.global.name.clone())),
        ("definition".into(), Json::Str(m.global.definition().id.clone())),
        ("quality_type".into(), Json::Str(q)),
        (
            "custom_profile".into(),
            s.profiles.iter().find(|p| Rc::ptr_eq(&p.global, m.global.instance_at(index::QUALITY_CHANGES))).map(|p| Json::Str(p.name.clone())).unwrap_or(Json::Null),
        ),
        ("definition_changes".into(), container_json(m.global.instance_at(index::DEFINITION_CHANGES))),
        ("user".into(), container_json(m.global.user())),
        ("extruders".into(), Json::Array(extruders)),
        ("post_processing_scripts".into(), Json::Str(m.global.meta("post_processing_scripts").map(|v| v.py_str()).unwrap_or_default())),
        ("network_printer".into(), Json::Str(m.global.meta("curust_network_printer").map(|v| v.py_str()).unwrap_or_default())),
    ])
}

fn prefs_json(p: &Prefs) -> Json {
    Json::Object(vec![
        ("jobname_prefix".into(), Json::Bool(p.jobname_prefix)),
        ("visibility_preset".into(), Json::Str(p.visibility_preset.clone())),
        ("custom_visible".into(), Json::Array(p.custom_visible.iter().map(|s| Json::Str(s.clone())).collect())),
        ("auto_drop".into(), Json::Bool(p.auto_drop)),
        ("auto_slice".into(), Json::Bool(p.auto_slice)),
        ("currency".into(), Json::Str(p.currency.clone())),
        (
            "material_costs".into(),
            Json::Array(p.material_costs.iter().map(|(g, c, w)| Json::Array(vec![Json::Str(g.clone()), Json::Float(*c), Json::Float(*w)])).collect()),
        ),
    ])
}

/// Everything `save` writes, as JSON.
pub fn to_json(s: &Session) -> Json {
    Json::Object(vec![
        ("version".into(), Json::Int(2)),
        ("active".into(), s.active.map(|a| Json::Int(a as i64)).unwrap_or(Json::Null)),
        ("active_extruder".into(), Json::Int(s.active_extruder as i64)),
        ("machines".into(), Json::Array(s.machines.iter().map(|m| machine_json(s, m)).collect())),
        ("custom_profiles".into(), Json::Array(s.profiles.iter().map(profile_json).collect())),
        ("prefs".into(), prefs_json(&s.prefs)),
    ])
}

pub fn save(s: &Session) {
    let j = to_json(s);
    let dir = config_dir();
    let _ = std::fs::create_dir_all(&dir);
    let tmp = dir.join("curust.json.tmp");
    if std::fs::write(&tmp, j.to_string_pretty()).is_ok() {
        let _ = std::fs::rename(&tmp, dir.join("curust.json"));
    }
}

fn restore_machine(s: &mut Session, j: &Json) -> Result<(), String> {
    let def = j.get("definition").and_then(|v| v.as_str()).ok_or("missing definition")?;
    let name = j.get("name").and_then(|v| v.as_str()).unwrap_or(def).to_string();
    let idx = s.add_machine(def, Some(&name))?;
    {
        let m = &s.machines[idx];
        restore_container(m.global.instance_at(index::DEFINITION_CHANGES), j.get("definition_changes"));
        if let Some(Json::Array(exts)) = j.get("extruders") {
            for e in exts {
                let pos = e.get("position").and_then(|v| v.as_str()).unwrap_or("0");
                if let Some(i) = m.extruder_by_position(pos) {
                    restore_container(m.extruders[i].instance_at(index::DEFINITION_CHANGES), e.get("definition_changes"));
                }
            }
        }
        m.clear_caches();
    }
    {
        let mm = &mut s.machines[idx];
        machine::update_default_extruder(mm);
        machine::update_number_extruders_enabled(mm);
    }
    if let Some(Json::Array(exts)) = j.get("extruders") {
        for e in exts {
            let pos: usize = e.get("position").and_then(|v| v.as_str()).and_then(|p| p.parse().ok()).unwrap_or(0);
            if let Some(v) = e.get("variant").and_then(|v| v.as_str()) {
                s.set_variant(pos, v);
            }
            if let Some(v) = e.get("material").and_then(|v| v.as_str()) {
                s.set_material(pos, v);
            }
            if e.get("enabled").and_then(|v| v.as_bool()) == Some(false) {
                s.set_extruder_enabled(pos, false);
            }
        }
    }
    if let Some(q) = j.get("quality_type").and_then(|v| v.as_str()) {
        if !q.is_empty() && q != "not_supported" {
            s.set_quality(q);
        }
    }
    if let Some(Json::Array(exts)) = j.get("extruders") {
        if let Some(intent) = exts.iter().filter_map(|e| e.get("intent").and_then(|v| v.as_str())).find(|i| *i != "default") {
            s.set_intent(intent);
        }
    }
    if let Some(name) = j.get("custom_profile").and_then(|v| v.as_str()) {
        let def = s.quality_definition();
        if let Some(pi) = s.profiles.iter().position(|p| p.name == name && Some(&p.definition) == def.as_ref()) {
            let _ = s.activate_profile(pi);
        }
    }
    let m = &s.machines[idx];
    restore_container(m.global.user(), j.get("user"));
    if let Some(Json::Array(exts)) = j.get("extruders") {
        for e in exts {
            let pos = e.get("position").and_then(|v| v.as_str()).unwrap_or("0");
            if let Some(i) = m.extruder_by_position(pos) {
                restore_container(m.extruders[i].user(), e.get("user"));
            }
        }
    }
    m.clear_caches();
    if let Some(np) = j.get("network_printer").and_then(|v| v.as_str()) {
        if !np.is_empty() {
            s.machines[idx].global.set_meta("curust_network_printer", curust_settings::py::Value::Str(np.into()));
        }
    }
    if let Some(pp) = j.get("post_processing_scripts").and_then(|v| v.as_str()) {
        if !pp.is_empty() {
            s.machines[idx].global.set_meta("post_processing_scripts", curust_settings::py::Value::Str(pp.into()));
        }
    }
    let _ = StackId::Global;
    Ok(())
}

/// Load the saved printers and preferences. Printers whose definition no longer exists are
/// skipped.
pub fn load(s: &mut Session) {
    let path = config_dir().join("curust.json");
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    let Ok(j) = json::parse(&text) else { return };
    apply(s, &j);
}

/// Reload the container registry (after materials or resources were added, changed or
/// removed) and rebuild the printers and profiles on top of it. The scene stays.
pub fn reload(s: &mut Session) {
    let j = to_json(s);
    let roots = Session::resource_roots(&s.resources);
    s.reg = Rc::new(curust_settings::registry::Registry::load(&roots));
    s.tree = machine::ContainerTree::default();
    s.machines.clear();
    s.profiles.clear();
    s.active = None;
    apply(s, &j);
    save(s);
    s.refresh_scene();
}

/// Restore printers, profiles and preferences from `to_json` output.
pub fn apply(s: &mut Session, j: &Json) {
    if let Some(p) = j.get("prefs") {
        if let Some(b) = p.get("jobname_prefix").and_then(|v| v.as_bool()) {
            s.prefs.jobname_prefix = b;
        }
        if let Some(v) = p.get("visibility_preset").and_then(|v| v.as_str()) {
            s.prefs.visibility_preset = v.to_string();
        }
        if let Some(Json::Array(a)) = p.get("custom_visible") {
            s.prefs.custom_visible = a.iter().filter_map(|v| v.as_str().map(String::from)).collect();
        }
        if let Some(b) = p.get("auto_drop").and_then(|v| v.as_bool()) {
            s.prefs.auto_drop = b;
        }
        if let Some(b) = p.get("auto_slice").and_then(|v| v.as_bool()) {
            s.prefs.auto_slice = b;
        }
        if let Some(v) = p.get("currency").and_then(|v| v.as_str()) {
            s.prefs.currency = v.to_string();
        }
        if let Some(Json::Array(a)) = p.get("material_costs") {
            s.prefs.material_costs = a
                .iter()
                .filter_map(|e| {
                    let e = e.as_array()?;
                    Some((e.first()?.as_str()?.to_string(), e.get(1)?.as_f64()?, e.get(2)?.as_f64()?))
                })
                .collect();
        }
    }
    // Version 1 defaulted to the Basic setting visibility; Advanced is the default now.
    if j.get("version").and_then(|v| v.as_i64()).unwrap_or(1) < 2 && s.prefs.visibility_preset == "basic" {
        s.prefs.visibility_preset = "advanced".into();
    }
    if let Some(Json::Array(ps)) = j.get("custom_profiles") {
        s.profiles = ps.iter().filter_map(restore_profile).collect();
    }
    if let Some(Json::Array(ms)) = j.get("machines") {
        for m in ms {
            if let Err(e) = restore_machine(s, m) {
                eprintln!("Skipping a saved printer: {}", e);
            }
        }
    }
    let active = j.get("active").and_then(|v| v.as_i64()).unwrap_or(0) as usize;
    if active < s.machines.len() {
        s.set_active_machine(active);
    }
    if let Some(e) = j.get("active_extruder").and_then(|v| v.as_i64()) {
        s.active_extruder = (e.max(0) as usize).min(s.extruder_count() - 1);
    }
}
