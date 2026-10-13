//! Material profiles (`*.xml.fdm_material`), as Cura's `XmlMaterialProfile` reads them.

use crate::instance::{InstValue, InstanceContainer};
use crate::py::Value;
use crate::registry::{MetaEntry, Registry};
use crate::xml::{self, Element};
use std::rc::Rc;

const UM: &str = "http://www.ultimaker.com/material";
const CURA: &str = "http://www.ultimaker.com/cura";

const SETTING_MAP: &[(&str, &str)] = &[
    ("print temperature", "default_material_print_temperature"),
    ("heated bed temperature", "default_material_bed_temperature"),
    ("standby temperature", "material_standby_temperature"),
    ("processing temperature graph", "material_flow_temp_graph"),
    ("print cooling", "cool_fan_speed"),
    ("retraction amount", "retraction_amount"),
    ("retraction speed", "retraction_speed"),
    ("adhesion tendency", "material_adhesion_tendency"),
    ("surface energy", "material_surface_energy"),
    ("build volume temperature", "build_volume_temperature"),
    ("anti ooze retract position", "material_anti_ooze_retracted_position"),
    ("anti ooze retract speed", "material_anti_ooze_retraction_speed"),
    ("break preparation position", "material_break_preparation_retracted_position"),
    ("break preparation speed", "material_break_preparation_speed"),
    ("break preparation temperature", "material_break_preparation_temperature"),
    ("break position", "material_break_retracted_position"),
    ("flush purge speed", "material_flush_purge_speed"),
    ("flush purge length", "material_flush_purge_length"),
    ("end of filament purge speed", "material_end_of_filament_purge_speed"),
    ("end of filament purge length", "material_end_of_filament_purge_length"),
    ("maximum park duration", "material_maximum_park_duration"),
    ("no load move factor", "material_no_load_move_factor"),
    ("break speed", "material_break_speed"),
    ("break temperature", "material_break_temperature"),
];

fn mapped(key: &str) -> Option<&'static str> {
    SETTING_MAP.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

fn text(e: &Element) -> String {
    e.text.clone().unwrap_or_default()
}

fn parse_compatible(v: &str) -> bool {
    v == "yes" || v == "unknown"
}

/// `xmlVersionToSettingVersion`.
fn xml_version_to_setting_version(v: &str) -> i64 {
    // `{"1.3": CuraApplication.SettingVersion}` (26 in Cura 5.12).
    match v {
        "1.3" => 26,
        _ => 0,
    }
}

/// `getPossibleDefinitionIDsFromName` (in a fixed order; Cura iterates a set).
pub fn possible_definition_ids(name: &str) -> Vec<String> {
    let lower = name.to_lowercase();
    let mut merged: Vec<String> = Vec::new();
    for part in lower.split(' ') {
        if part.is_empty() {
            continue;
        }
        if merged.is_empty() {
            merged.push(part.to_string());
            continue;
        }
        if part.chars().all(|c| c.is_ascii_digit()) {
            let last = merged.last_mut().unwrap();
            last.push_str(part);
        } else {
            merged.push(part.to_string());
        }
    }
    let mut ids = vec![lower.replace(' ', ""), lower.replace(' ', "_"), merged.join("_"), name.replace(' ', ""), name.replace(' ', "_")];
    let mut seen = std::collections::HashSet::new();
    ids.retain(|i| seen.insert(i.clone()));
    ids
}

/// The machine definition ids a `<machine_identifier product=...>` stands for.
pub fn machine_ids_for(product: &str, reg: &Registry) -> Vec<String> {
    match reg.product_id_map.get(product) {
        Some(v) if !v.is_empty() => v.clone(),
        _ => possible_definition_ids(product),
    }
}

/// The metadata entries `XmlMaterialProfile.deserializeMetadata` produces:
/// (id, machine definition id, hotend name, buildplate name, metadata).
#[allow(clippy::type_complexity)]
pub fn deserialize_metadata(xml_text: &str, container_id: &str, reg: &Registry) -> Vec<(String, Option<String>, Option<String>, Option<String>, Vec<(String, Value)>)> {
    let Ok(data) = xml::parse(xml_text) else { return Vec::new() };
    let mut out = Vec::new();
    let mut base: Vec<(String, Value)> = Vec::new();
    let set = |m: &mut Vec<(String, Value)>, k: &str, v: Value| {
        if let Some(slot) = m.iter_mut().find(|(x, _)| x == k) {
            slot.1 = v;
        } else {
            m.push((k.to_string(), v));
        }
    };
    set(&mut base, "type", Value::str("material"));
    set(&mut base, "status", Value::str("unknown"));
    set(&mut base, "container_type", Value::str("XmlMaterialProfile"));
    set(&mut base, "id", Value::str(container_id));
    set(&mut base, "base_file", Value::str(container_id));
    let version = data.attr("version").unwrap_or("1.2");
    set(&mut base, "setting_version", Value::Int(xml_version_to_setting_version(version)));
    if let Some(md) = data.find(UM, "metadata") {
        for entry in &md.children {
            let tag = entry.local_name();
            if tag == "name" {
                let brand = entry.find(UM, "brand");
                let material = entry.find(UM, "material");
                let color = entry.find(UM, "color");
                let label = entry.find(UM, "label");
                if let Some(l) = label.and_then(|l| l.text.clone()) {
                    set(&mut base, "name", Value::str(&l));
                } else if let (Some(m), Some(c)) = (material, color) {
                    set(&mut base, "name", Value::str(&profile_name(m.text.as_deref(), c.text.as_deref())));
                } else {
                    set(&mut base, "name", Value::str("Unknown Material"));
                }
                set(&mut base, "brand", Value::str(brand.and_then(|b| b.text.as_deref()).unwrap_or("Unknown Brand")));
                set(&mut base, "material", Value::str(material.and_then(|b| b.text.as_deref()).unwrap_or("Unknown Type")));
                set(&mut base, "color_name", Value::str(color.and_then(|b| b.text.as_deref()).unwrap_or("Unknown Color")));
                continue;
            }
            if tag == "setting_version" {
                continue;
            }
            set(&mut base, tag, entry.text.as_deref().map(Value::str).unwrap_or(Value::None));
        }
    }
    if !base.iter().any(|(k, _)| k == "description") {
        set(&mut base, "description", Value::str(""));
    }
    if !base.iter().any(|(k, _)| k == "adhesion_info") {
        set(&mut base, "adhesion_info", Value::str(""));
    }
    let mut props: Vec<(Value, Value)> = Vec::new();
    let mut diameter = "2.85".to_string();
    if let Some(p) = data.find(UM, "properties") {
        for e in &p.children {
            let tag = e.local_name().to_string();
            if tag == "diameter" {
                diameter = text(e);
            }
            props.push((Value::str(&tag), e.text.as_deref().map(Value::str).unwrap_or(Value::None)));
        }
    }
    let approx = diameter.trim().parse::<f64>().map(|d| crate::py::builtins::py_round(&Value::Float(d), None).map(|v| v.py_str()).unwrap_or_default()).unwrap_or_default();
    set(&mut base, "approximate_diameter", Value::str(&approx));
    set(&mut base, "properties", Value::Dict(Rc::new(props)));
    set(&mut base, "definition", Value::str("fdmprinter"));
    let settings = data.find(UM, "settings");
    let common_compat = settings
        .and_then(|s| s.find_all(UM, "setting").find(|e| e.attr("key") == Some("hardware compatible")))
        .map(|e| parse_compatible(&text(e)))
        .unwrap_or(true);
    set(&mut base, "compatible", Value::Bool(common_compat));
    out.push((container_id.to_string(), None, None, None, base.clone()));

    let Some(settings) = settings else { return out };
    for machine in settings.find_all(UM, "machine") {
        let mut machine_compat = common_compat;
        for e in machine.find_all(UM, "setting").filter(|e| e.attr("key") == Some("hardware compatible")) {
            if let Some(t) = &e.text {
                machine_compat = parse_compatible(t);
            }
        }
        for identifier in machine.find_all(UM, "machine_identifier") {
            let product = identifier.attr("product").unwrap_or("");
            for machine_id in machine_ids_for(product, reg) {
                let defs = reg.find_definitions(&[("id", &machine_id)]);
                let Some(def) = defs.first() else { continue };
                let manufacturer = identifier.attr("manufacturer").map(|s| s.to_string()).or_else(|| def.get_str("manufacturer")).unwrap_or_else(|| "Unknown".into());
                let mut m = base.clone();
                set(&mut m, "id", Value::str(&format!("{}_{}", container_id, machine_id)));
                set(&mut m, "compatible", Value::Bool(machine_compat));
                set(&mut m, "machine_manufacturer", Value::str(&manufacturer));
                set(&mut m, "definition", Value::str(&machine_id));
                out.push((format!("{}_{}", container_id, machine_id), Some(machine_id.clone()), None, None, m));

                // Buildplate compatibility maps (machine level).
                let mut bp_compat: Vec<(Value, Value)> = Vec::new();
                let mut bp_rec: Vec<(Value, Value)> = Vec::new();
                for bp in machine.find_all(UM, "buildplate") {
                    let Some(bp_id) = bp.attr("id") else { continue };
                    let found = !reg.find_instances(&[("id", bp_id)]).is_empty() || !reg.find_instances(&[("definition", &machine_id), ("name", bp_id)]).is_empty();
                    if !found {
                        continue;
                    }
                    let (mut c, mut r) = (true, true);
                    for e in bp.find_all(UM, "setting") {
                        if let Some(t) = &e.text {
                            match e.attr("key") {
                                Some("hardware compatible") => c = parse_compatible(t),
                                Some("hardware recommended") => r = parse_compatible(t),
                                _ => {}
                            }
                        }
                    }
                    bp_compat.push((Value::str(bp_id), Value::Bool(c)));
                    bp_rec.push((Value::str(bp_id), Value::Bool(r)));
                }

                for hotend in machine.find_all(UM, "hotend") {
                    let Some(hotend_name) = hotend.attr("id") else { continue };
                    let mut hotend_compat = machine_compat;
                    for e in hotend.find_all(UM, "setting").filter(|e| e.attr("key") == Some("hardware compatible")) {
                        if let Some(t) = &e.text {
                            hotend_compat = parse_compatible(t);
                        }
                    }
                    let hid = format!("{}_{}_{}", container_id, machine_id, hotend_name.replace(' ', "_"));
                    let mut h = base.clone();
                    set(&mut h, "variant_name", Value::str(hotend_name));
                    set(&mut h, "compatible", Value::Bool(hotend_compat));
                    set(&mut h, "machine_manufacturer", Value::str(&manufacturer));
                    set(&mut h, "id", Value::str(&hid));
                    set(&mut h, "definition", Value::str(&machine_id));
                    if !bp_compat.is_empty() {
                        set(&mut h, "buildplate_compatible", Value::Dict(Rc::new(bp_compat.clone())));
                        set(&mut h, "buildplate_recommended", Value::Dict(Rc::new(bp_rec.clone())));
                    }
                    out.push((hid.clone(), Some(machine_id.clone()), Some(hotend_name.to_string()), None, h.clone()));
                    for bp in hotend.find_all(UM, "buildplate") {
                        let Some(bp_name) = bp.attr("id") else { continue };
                        let (_, unmapped) = settings_for_node(bp);
                        let c = unmapped.iter().find(|(k, _)| k == "hardware compatible").map(|(_, v)| Value::Bool(*v)).unwrap_or_else(|| Value::Dict(Rc::new(bp_compat.clone())));
                        let r = unmapped.iter().find(|(k, _)| k == "hardware recommended").map(|(_, v)| Value::Bool(*v)).unwrap_or_else(|| Value::Dict(Rc::new(bp_rec.clone())));
                        let bid = format!("{}_{}", hid, bp_name.replace(' ', "_"));
                        let mut b = h.clone();
                        set(&mut b, "id", Value::str(&bid));
                        set(&mut b, "buildplate_name", Value::str(bp_name));
                        set(&mut b, "compatible", c.clone());
                        set(&mut b, "buildplate_compatible", c);
                        set(&mut b, "buildplate_recommended", r);
                        out.push((bid, Some(machine_id.clone()), Some(hotend_name.to_string()), Some(bp_name.to_string()), b));
                    }
                }
                break;
            }
        }
    }
    out
}

fn profile_name(material: Option<&str>, color: Option<&str>) -> String {
    let Some(m) = material else { return "Unknown Material".into() };
    match color {
        Some(c) if c != "Generic" => format!("{} {}", c, m),
        _ => m.to_string(),
    }
}

/// `_getSettingsDictForNode`: mapped settings (with Cura-namespace settings) and unmapped
/// compatibility flags.
fn settings_for_node(node: &Element) -> (Vec<(String, InstValue)>, Vec<(String, bool)>) {
    let mut mapped_out: Vec<(String, InstValue)> = Vec::new();
    let mut unmapped: Vec<(String, bool)> = Vec::new();
    let put = |m: &mut Vec<(String, InstValue)>, k: &str, v: InstValue| {
        if let Some(slot) = m.iter_mut().find(|(x, _)| x == k) {
            slot.1 = v;
        } else {
            m.push((k.to_string(), v));
        }
    };
    for e in node.find_all(UM, "setting") {
        let key = e.attr("key").unwrap_or("");
        if let Some(m) = mapped(key) {
            put(&mut mapped_out, m, setting_text_value(key, e));
        } else if key == "hardware compatible" || key == "hardware recommended" {
            unmapped.push((key.to_string(), parse_compatible(&text(e))));
        }
    }
    for e in node.find_all(CURA, "setting") {
        put(&mut mapped_out, e.attr("key").unwrap_or(""), cura_value(e));
    }
    (mapped_out, unmapped)
}

fn setting_text_value(key: &str, e: &Element) -> InstValue {
    if key == "processing temperature graph" {
        let mut pts = Vec::new();
        for p in e.find_all(UM, "point") {
            let flow = p.attr("flow").and_then(|s| s.trim().parse::<f64>().ok()).unwrap_or(0.0);
            let temp = p.attr("temperature").and_then(|s| s.trim().parse::<f64>().ok()).unwrap_or(0.0);
            pts.push(Value::list(vec![Value::Float(flow), Value::Float(temp)]));
        }
        return InstValue::Str(Value::list(pts).py_str());
    }
    InstValue::Str(text(e))
}

fn cura_value(e: &Element) -> InstValue {
    let v = text(e);
    match v.to_lowercase().as_str() {
        "yes" => InstValue::Typed(Value::Bool(true)),
        "no" => InstValue::Typed(Value::Bool(false)),
        _ => InstValue::Str(v),
    }
}

/// Build the container for a material metadata entry (`XmlMaterialProfile.deserialize`).
pub fn build_container(
    text_xml: &str,
    entry: &MetaEntry,
    machine: Option<&str>,
    hotend: Option<&str>,
    product_map: &std::collections::HashMap<String, Vec<String>>,
) -> Option<InstanceContainer> {
    let data = xml::parse(text_xml).ok()?;
    let mut values: Vec<(String, InstValue)> = Vec::new();
    let put = |m: &mut Vec<(String, InstValue)>, k: &str, v: InstValue| {
        if let Some(slot) = m.iter_mut().find(|(x, _)| x == k) {
            slot.1 = v;
        } else {
            m.push((k.to_string(), v));
        }
    };
    // Metadata-derived values.
    let mut meta_seen: Vec<(String, String)> = Vec::new();
    if let Some(md) = data.find(UM, "metadata") {
        for e in &md.children {
            let tag = e.local_name();
            if tag == "name" {
                let brand = e.find(UM, "brand").and_then(|b| b.text.clone()).unwrap_or_else(|| "Unknown Brand".into());
                let material = e.find(UM, "material").and_then(|b| b.text.clone()).unwrap_or_else(|| "Unknown Type".into());
                meta_seen.retain(|(k, _)| k != "brand" && k != "material");
                meta_seen.push(("brand".into(), brand));
                meta_seen.push(("material".into(), material));
                continue;
            }
            if tag == "setting_version" {
                continue;
            }
            meta_seen.retain(|(k, _)| k != tag);
            meta_seen.push((tag.to_string(), text(e)));
            for (k, v) in &meta_seen {
                let target = match k.as_str() {
                    "GUID" => "material_guid",
                    "material" => "material_type",
                    "brand" => "material_brand",
                    _ => continue,
                };
                put(&mut values, target, InstValue::Str(v.clone()));
            }
        }
    }
    if let Some(p) = data.find(UM, "properties") {
        for e in &p.children {
            if e.local_name() == "diameter" {
                put(&mut values, "material_diameter", InstValue::Str(text(e)));
            }
        }
    }
    let settings = data.find(UM, "settings");
    if let Some(s) = settings {
        for e in s.find_all(UM, "setting") {
            let key = e.attr("key").unwrap_or("");
            if let Some(m) = mapped(key) {
                put(&mut values, m, setting_text_value(key, e));
            }
        }
        for e in s.find_all(CURA, "setting") {
            put(&mut values, e.attr("key").unwrap_or(""), cura_value(e));
        }
    }
    if let (Some(machine_id), Some(s)) = (machine, settings) {
        // Find the <machine> element whose identifier maps to this machine.
        let mut machine_el = None;
        'outer: for m in s.find_all(UM, "machine") {
            for ident in m.find_all(UM, "machine_identifier") {
                let product = ident.attr("product").unwrap_or("");
                let ids = match product_map.get(product) {
                    Some(v) if !v.is_empty() => v.clone(),
                    _ => possible_definition_ids(product),
                };
                if ids.iter().any(|i| i == machine_id) {
                    machine_el = Some(m);
                    break 'outer;
                }
            }
        }
        if let Some(m) = machine_el {
            for e in m.find_all(UM, "setting") {
                let key = e.attr("key").unwrap_or("");
                if let Some(mk) = mapped(key) {
                    put(&mut values, mk, setting_text_value(key, e));
                }
            }
            for e in m.find_all(CURA, "setting") {
                put(&mut values, e.attr("key").unwrap_or(""), cura_value(e));
            }
            if let Some(hname) = hotend {
                if let Some(h) = m.find_all(UM, "hotend").find(|h| h.attr("id") == Some(hname)) {
                    let (hm, _) = settings_for_node(h);
                    for (k, v) in hm {
                        put(&mut values, &k, v);
                    }
                }
            }
        }
    }
    let definition = entry.get_str("definition").unwrap_or_else(|| "fdmprinter".into());
    let mut c = InstanceContainer::new(&entry.id, &entry.get_str("name").unwrap_or_default(), &definition);
    c.metadata = entry.meta.clone();
    *c.values.borrow_mut() = values;
    Some(c)
}

