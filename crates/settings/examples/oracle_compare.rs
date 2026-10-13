//! Run the jobs of an oracle job file (see `tools/oracle`) through the Rust settings port and print
//! the same JSON lines the oracle produces (without mesh data), for comparison.
//!
//! Usage: oracle_compare <resources-dir> <jobs.json>

use curust_settings::definition::json_to_value;
use curust_settings::instance::{InstValue, InstanceContainer};
use curust_settings::json::{self, Json};
use curust_settings::machine::{self, ContainerTree};
use curust_settings::registry::Registry;
use curust_settings::slice_message::{build_slice_settings, SceneObject, TimeTokens};
use curust_settings::stack::{index, ObjectStack};
use std::path::PathBuf;
use std::rc::Rc;

fn user_value(v: &Json) -> InstValue {
    match v {
        Json::Str(s) if s.starts_with('=') => InstValue::from_str(s),
        other => InstValue::Typed(json_to_value(other)),
    }
}

fn pairs(v: &[(String, String)]) -> Json {
    Json::Array(v.iter().map(|(k, v)| Json::Array(vec![Json::Str(k.clone()), Json::Str(v.clone())])).collect())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let resources = PathBuf::from(&args[1]);
    let jobs = json::parse(&std::fs::read_to_string(&args[2]).unwrap()).unwrap();
    let reg = Rc::new(Registry::load(&[resources]));
    let tree = ContainerTree::default();
    for (ji, job) in jobs.as_array().unwrap().iter().enumerate() {
        let name = job.get("name").and_then(|v| v.as_str()).map(String::from).unwrap_or_else(|| ji.to_string());
        let mut res: Vec<(String, Json)> = vec![("name".into(), Json::Str(name.clone()))];
        let machine_id = job.get("machine").unwrap().as_str().unwrap();
        let stack_name = format!("oracle_{}", ji);
        let mut m = match machine::create_machine(&reg, &tree, &stack_name, machine_id) {
            Ok(m) => m,
            Err(e) => {
                res.push(("error".into(), Json::Str(e)));
                println!("{}", Json::Object(res).to_string_compact());
                continue;
            }
        };
        let mut ext_items: Vec<(String, Json)> = job.get("extruders").and_then(|e| e.as_object()).cloned().unwrap_or_default();
        ext_items.sort_by(|a, b| a.0.cmp(&b.0));
        for (pos, ext) in &ext_items {
            if let Some(v) = ext.get("variant").and_then(|v| v.as_str()) {
                machine::set_variant_by_name(&mut m, &tree, pos, v);
            }
            if let Some(v) = ext.get("material").and_then(|v| v.as_str()) {
                machine::set_material_by_id(&mut m, &tree, pos, v);
            }
            if let Some(v) = ext.get("enabled").and_then(|v| v.as_bool()) {
                machine::set_extruder_enabled(&mut m, &tree, pos.parse().unwrap(), v);
            }
        }
        if let Some(q) = job.get("quality_type").and_then(|v| v.as_str()) {
            machine::set_quality_type(&mut m, &tree, q);
        }
        if let Some(i) = job.get("intent").and_then(|v| v.as_str()) {
            machine::set_intent_by_category(&mut m, &tree, i);
        }
        if let Some(Json::Object(g)) = job.get("global") {
            for (k, v) in g {
                m.global.user().set_value(k, user_value(v));
            }
        }
        for (pos, ext) in &ext_items {
            if let Some(Json::Object(u)) = ext.get("user") {
                let list = m.extruder_list();
                let i = list[pos.parse::<usize>().unwrap()];
                for (k, v) in u {
                    m.extruders[i].user().set_value(k, user_value(v));
                }
            }
        }
        m.clear_caches();
        let containers = Json::Object(vec![
            ("global".into(), Json::Array(m.global.containers.iter().map(|c| Json::Str(c.id().to_string())).collect())),
            (
                "extruders".into(),
                Json::Array(m.extruder_list().iter().map(|&i| Json::Array(m.extruders[i].containers.iter().map(|c| Json::Str(c.id().to_string())).collect())).collect()),
            ),
        ]);
        res.push(("containers".into(), containers));
        // Models: per-object stacks on the default (or requested) extruder.
        let mut objects = Vec::new();
        let default_pos: usize = m.default_extruder_position.borrow().parse().unwrap_or(0);
        if let Some(Json::Array(models)) = job.get("models") {
            for (mi, model) in models.iter().enumerate() {
                let pos = model.get("extruder").and_then(|v| v.as_i64()).map(|v| v as usize).unwrap_or(default_pos);
                let list = m.extruder_list();
                let ei = list[pos];
                let user = InstanceContainer::new(&format!("SettingOverrideInstanceContainer-{}", mi), "", &m.global.definition().id);
                if let Some(Json::Object(s)) = model.get("settings") {
                    for (k, v) in s {
                        user.set_value(k, user_value(v));
                    }
                }
                m.object_stacks.push(ObjectStack { user: Rc::new(user), extruder_position: ei });
                objects.push(SceneObject { stack: m.object_stacks.len() - 1, painted_extruders: None });
            }
        }
        let _ = index::USER;
        let s = build_slice_settings(&m, &objects, &TimeTokens::fixed());
        res.push(("global_settings".into(), pairs(&s.global_settings)));
        res.push(("limit_to_extruder".into(), Json::Array(s.limit_to_extruder.iter().map(|(k, v)| Json::Array(vec![Json::Str(k.clone()), Json::Int(*v)])).collect())));
        res.push((
            "extruders".into(),
            Json::Array(s.extruders.iter().map(|(id, st)| Json::Object(vec![("id".into(), Json::Int(*id)), ("settings".into(), pairs(st))])).collect()),
        ));
        res.push(("object_settings".into(), Json::Array(s.object_settings.iter().map(|o| pairs(o)).collect())));
        println!("{}", Json::Object(res).to_string_compact());
    }
}
