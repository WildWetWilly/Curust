use curust_settings::machine::{self, ContainerTree};
use curust_settings::registry::Registry;
use curust_settings::stack::index;
use std::path::PathBuf;
use std::rc::Rc;
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let reg = Rc::new(Registry::load(&[PathBuf::from(&args[1])]));
    let tree = ContainerTree::default();
    let node = tree.machine(&reg, &args[2]);
    println!("has_machine_quality={} quality_def={} variants={:?}", node.has_machine_quality, node.quality_definition, node.variants.items.iter().map(|(k, v)| (k.clone(), v.container_id.clone(), v.materials.len())).collect::<Vec<_>>());
    for (k, v) in &node.variants.items {
        if let Some(mn) = v.materials.get("generic_pla_175").or_else(|| v.materials.get("generic_pla")) {
            println!(" variant {} material {} qualities {:?}", k, mn.container_id, mn.qualities.items.iter().map(|(k, q)| (k.clone(), q.quality_type.clone())).collect::<Vec<_>>());
        }
    }
    println!("global qualities {:?}", node.global_qualities.items.iter().map(|(k, q)| (k.clone(), q.container_id.clone())).collect::<Vec<_>>());
    let m = machine::create_machine(&reg, &tree, "x", &args[2]).unwrap();
    for i in m.extruder_list() {
        println!("extruder {}: {:?}", i, m.extruders[i].containers.iter().map(|c| c.id().to_string()).collect::<Vec<_>>());
        println!("  quality {}", m.extruders[i].container(index::QUALITY).id());
    }
}
