use curust_settings::machine::{self, ContainerTree};
use curust_settings::registry::Registry;
use curust_settings::stack::StackId;
use std::path::PathBuf;
use std::rc::Rc;
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let reg = Rc::new(Registry::load(&[PathBuf::from(&args[1])]));
    let tree = ContainerTree::default();
    let m = machine::create_machine(&reg, &tree, "x", &args[2]).unwrap();
    for key in &args[3..] {
        for p in ["value", "minimum_value", "maximum_value", "minimum_value_warning", "maximum_value_warning", "warning_value", "error_value"] {
            println!("{} {} = {:?}", key, p, m.get_property(StackId::Global, key, p, None));
        }
        println!("state {:?}", m.validation_state(StackId::Global, key));
    }
}
