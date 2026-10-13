//! `Slice.h` and `Scene.h`.

use crate::extruder_train::ExtruderTrain;
use crate::mesh_group::MeshGroup;
use crate::settings::settings::Settings;
use std::collections::HashMap;

/// All data needed for one slice: global settings, extruders and mesh groups.
pub struct Scene {
    pub settings: Settings,
    /// Setting key -> extruder number the setting is limited to.
    pub limit_to_extruder: HashMap<String, usize>,
    pub mesh_groups: Vec<MeshGroup>,
    pub extruders: Vec<ExtruderTrain>,
    pub current_mesh_group: usize,
}

impl Scene {
    fn new(num_mesh_groups: usize) -> Self {
        Scene {
            settings: Settings::new(),
            limit_to_extruder: HashMap::new(),
            mesh_groups: (0..num_mesh_groups).map(|_| MeshGroup::default()).collect(),
            extruders: Vec::new(),
            current_mesh_group: 0,
        }
    }

    /// Must be called once the scene is at its final address (inside the boxed slice).
    fn link_parents(&mut self) {
        let p: *const Settings = &self.settings;
        for mesh_group in self.mesh_groups.iter_mut() {
            mesh_group.settings.set_parent(p);
        }
    }

    pub fn current_mesh_group(&self) -> &MeshGroup {
        &self.mesh_groups[self.current_mesh_group]
    }

    pub fn current_mesh_group_mut(&mut self) -> &mut MeshGroup {
        &mut self.mesh_groups[self.current_mesh_group]
    }

    /// `Scene::processMeshGroup`: generate the G-code of one mesh group.
    pub fn process_mesh_group(mesh_group_idx: usize) {
        use crate::fff_processor::FffProcessor;
        let timer = std::time::Instant::now();
        let mesh_group = &crate::application::current_slice().scene.mesh_groups[mesh_group_idx];
        let mut empty = true;
        for mesh in &mesh_group.meshes {
            if !mesh.settings.get_bool("infill_mesh") && !mesh.settings.get_bool("anti_overhang_mesh") {
                empty = false;
                break;
            }
        }
        if empty {
            crate::progress::message_progress(crate::progress::Stage::Finish, 1, 1);
            crate::log_info!("Total time elapsed {:.3}s", timer.elapsed().as_secs_f64());
            return;
        }

        let fff_processor = FffProcessor::get_instance();
        let mut storage = crate::slice_data_storage::SliceDataStorage::new();
        if !fff_processor.polygon_generator.generate_areas(&mut storage) {
            return;
        }

        fff_processor.gcode_writer.write_gcode(&mut storage);

        crate::progress::message_progress(crate::progress::Stage::Finish, 1, 1);
        crate::application::communication().flush_gcode();
        crate::application::communication().send_optimized_layer_data();
        crate::log_info!("Total time elapsed {:.3}s\n", timer.elapsed().as_secs_f64());
    }

    pub fn get_all_settings_string(&self) -> String {
        let mut output = String::new();
        output.push_str(&self.settings.get_all_settings_string());
        for (extruder_nr, extruder) in self.extruders.iter().enumerate() {
            output.push_str(&format!(" -e{}{}", extruder_nr, extruder.settings.get_all_settings_string()));
        }
        for (mesh_group_index, mesh_group) in self.mesh_groups.iter().enumerate() {
            if mesh_group_index == 0 {
                output.push_str(" -g");
            } else {
                output.push_str(" --next");
            }
            output.push_str(&mesh_group.settings.get_all_settings_string());
            for (mesh_index, mesh) in mesh_group.meshes.iter().enumerate() {
                output.push_str(&format!(
                    " -e{} -l \"{}\"{}",
                    mesh.settings.get_usize("extruder_nr"),
                    mesh_index,
                    mesh.settings.get_all_settings_string()
                ));
            }
        }
        output.push('\n');
        output
    }
}

pub struct Slice {
    pub scene: Scene,
}

impl Slice {
    /// `Slice::compute()`: slice all mesh groups.
    pub fn compute(&mut self) {
        crate::log_info!("All settings: {}", self.scene.get_all_settings_string());
        for mesh_group_idx in 0..self.scene.mesh_groups.len() {
            self.scene.current_mesh_group = mesh_group_idx;
            let parent: *const Settings = &self.scene.mesh_groups[mesh_group_idx].settings;
            for extruder in self.scene.extruders.iter_mut() {
                extruder.settings.set_parent(parent);
            }
            Scene::process_mesh_group(mesh_group_idx);
        }
    }

    pub fn new(num_mesh_groups: usize) -> Box<Slice> {
        let mut slice = Box::new(Slice { scene: Scene::new(num_mesh_groups) });
        slice.scene.link_parents();
        slice
    }

    pub fn reset(&mut self) {
        self.scene.extruders.clear();
        self.scene.mesh_groups.clear();
        self.scene.settings = Settings::new();
    }
}
