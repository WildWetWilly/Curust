//! `communication/CommandLine.h/.cpp`: slicing driven by command line arguments.

use super::Communication;
use cppstd::unordered::UnorderedSet;
use crate::application::{current_slice_mut, set_current_slice, start_thread_pool};
use crate::extruder_train::ExtruderTrain;
use crate::fff_processor::FffProcessor;
use crate::geometry::point3ll::Point3LL;
use crate::mesh_group::load_mesh_into_mesh_group;
use crate::print_feature::PrintFeatureType;
use crate::settings::settings::Settings;
use crate::settings::types::Velocity;
use crate::slice::Slice;
use crate::utils::coord::Coord;
use crate::utils::json::{self, JsonValue};
use std::path::{Path, PathBuf};

/// `container_setting_map`; kept as an ordered list of (key, settings) in document order.
pub type ContainerSettingMap = Vec<(String, Vec<(String, String)>)>;

pub struct CommandLine {
    arguments: Vec<String>,
    search_directories: Vec<PathBuf>,
    #[allow(dead_code)]
    last_shown_progress: u32,
}

fn split_paths(s: &str) -> Vec<PathBuf> {
    let sep = if cfg!(windows) { ';' } else { ':' };
    s.split(sep).filter(|p| !p.is_empty()).map(PathBuf::from).collect()
}

/// `jsonValue2Str`: convert a JSON value to the string representation used in the settings.
pub fn json_value_to_str(value: &JsonValue) -> Option<String> {
    match value {
        JsonValue::String(s) => Some(s.clone()),
        JsonValue::Bool(true) => Some("true".to_string()),
        JsonValue::Bool(false) => Some("false".to_string()),
        JsonValue::Int(_) | JsonValue::Uint(_) | JsonValue::Double(_) => Some(cppstd::to_string_f64(value.get_double())),
        JsonValue::Array(a) => {
            if a.is_empty() {
                return Some("[]".to_string());
            }
            // Note: like the C++ code, a failed conversion of an element leaves the previous text in place.
            let mut temp = json_value_to_str(&a[0]).unwrap_or_default();
            let mut converted = temp.clone();
            for next in &a[1..] {
                if let Some(t) = json_value_to_str(next) {
                    temp = t;
                }
                converted = converted + "," + &temp;
            }
            Some(format!("[{}]", converted))
        }
        _ => None,
    }
}

impl CommandLine {
    pub fn new(arguments: Vec<String>) -> Self {
        let mut search_directories = Vec::new();
        if let Ok(search_paths) = std::env::var("CURA_ENGINE_SEARCH_PATH") {
            if !search_paths.is_empty() {
                search_directories = split_paths(&search_paths);
            }
        }
        CommandLine { arguments, search_directories, last_shown_progress: 0 }
    }

    fn load_json_file(&mut self, json_filename: &Path, settings: *mut Settings, force_read_parent: bool, force_read_nondefault: bool) -> i32 {
        let data = match std::fs::read(json_filename) {
            Ok(d) => d,
            Err(_) => {
                crate::log_error!("Couldn't open JSON file: {}", json_filename.display());
                return 1;
            }
        };
        let json_document = match json::parse(&data) {
            Ok(d) => d,
            Err(e) => {
                crate::log_error!("Error parsing JSON (offset {}): {}", e.offset, e.message);
                return 2;
            }
        };
        let parent = json_filename.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        self.search_directories.push(parent);
        let dirs = self.search_directories.clone();
        self.load_json_document(&json_document, &dirs, settings, force_read_parent, force_read_nondefault)
    }

    fn load_json_document(
        &mut self,
        document: &JsonValue,
        search_directories: &[PathBuf],
        settings: *mut Settings,
        force_read_parent: bool,
        force_read_nondefault: bool,
    ) -> i32 {
        if let Some(JsonValue::String(inherits)) = document.get("inherits") {
            let parent_file = Self::find_definition_file(inherits, search_directories);
            if parent_file.is_empty() {
                crate::log_error!("Inherited JSON file: {} not found.", inherits);
                return 1;
            }
            let error_code = self.load_json_file(Path::new(&parent_file), settings, force_read_parent, force_read_nondefault);
            if error_code != 0 {
                return error_code;
            }
        }

        // Extruders defined from here, if any.
        // Note that this always puts the extruder settings in the slice of the current extruder. It doesn't keep the nested structure of the JSON files, if extruders would have their own sub-extruders.
        if let Some(metadata) = document.get("metadata") {
            if metadata.is_object() {
                if let Some(extruder_trains) = metadata.get("machine_extruder_trains") {
                    if extruder_trains.is_object() {
                        for (name, extruder_id) in extruder_trains.members() {
                            let extruder_nr = cppstd::atoi(name);
                            if extruder_nr < 0 {
                                continue;
                            }
                            let scene = &mut current_slice_mut().scene;
                            while scene.extruders.len() <= extruder_nr as usize {
                                let n = scene.extruders.len();
                                let parent: *const Settings = &scene.settings;
                                scene.extruders.push(ExtruderTrain::new(n, parent));
                            }
                            let extruder_definition_id = match extruder_id {
                                JsonValue::String(s) => s.clone(),
                                _ => continue,
                            };
                            let extruder_file = Self::find_definition_file(&extruder_definition_id, search_directories);
                            let extruder_settings: *mut Settings = &mut current_slice_mut().scene.extruders[extruder_nr as usize].settings;
                            self.load_json_file(Path::new(&extruder_file), extruder_settings, force_read_parent, force_read_nondefault);
                        }
                    }
                }
            }
        }

        if let Some(s) = document.get("settings") {
            if s.is_object() {
                Self::load_json_settings(s, settings, force_read_parent, force_read_nondefault);
            }
        }
        if let Some(s) = document.get("overrides") {
            if s.is_object() {
                Self::load_json_settings(s, settings, force_read_parent, force_read_nondefault);
            }
        }
        0
    }

    fn load_json_settings(element: &JsonValue, settings: *mut Settings, force_read_parent: bool, force_read_nondefault: bool) {
        // SAFETY: the settings object lives in the current slice.
        let settings = unsafe { &mut *settings };
        for (name, setting_object) in element.members() {
            if !setting_object.is_object() {
                crate::log_error!("JSON setting {} is not an object!", name);
                continue;
            }

            if let Some(children) = setting_object.get("children") {
                Self::load_json_settings(children, settings, force_read_parent, force_read_nondefault);
                if !force_read_parent {
                    continue;
                }
            }

            if !setting_object.has_member("default_value") && (!force_read_nondefault || !setting_object.has_member("value") || settings.has(name)) {
                if !setting_object.has_member("children") {
                    crate::log_warn!("JSON setting '{}' has no [default_]value!", name);
                }
                continue;
            }

            // If the setting has a default_value, use that. Otherwise, force_read_nondefault must be true, so use value.
            let json_value = setting_object.get("default_value").or_else(|| setting_object.get("value")).unwrap();
            let value_string = match json_value_to_str(json_value) {
                Some(v) => v,
                None => {
                    crate::log_warn!("Unrecognized data type in JSON setting {}", name);
                    continue;
                }
            };
            settings.add(name, &value_string);
        }
    }

    fn read_resolved_json_values_file(json_filename: &Path) -> Option<ContainerSettingMap> {
        let data = match std::fs::read(json_filename) {
            Ok(d) => d,
            Err(_) => {
                crate::log_error!("Couldn't open JSON file: {}", json_filename.display());
                return None;
            }
        };
        let json_document = match json::parse(&data) {
            Ok(d) => d,
            Err(e) => {
                crate::log_error!("Error parsing JSON (offset {}): {}", e.offset, e.message);
                return None;
            }
        };
        Self::read_resolved_json_values(&json_document)
    }

    fn read_resolved_json_values(document: &JsonValue) -> Option<ContainerSettingMap> {
        if !document.is_object() {
            return None;
        }

        let mut result: ContainerSettingMap = Vec::new();
        for (resolved_key, resolved_value_obj) in document.members() {
            let mut values: Vec<(String, String)> = Vec::new();
            for (name, value) in resolved_value_obj.members() {
                match json_value_to_str(value) {
                    Some(v) => {
                        // `std::unordered_map::emplace`: the first occurrence of a key wins.
                        if !values.iter().any(|(k, _)| k == name) {
                            values.push((name.clone(), v));
                        }
                    }
                    None => {
                        crate::log_warn!("Unrecognized data type in JSON setting {}", name);
                        continue;
                    }
                }
            }
            if !result.iter().any(|(k, _)| k == resolved_key) {
                result.push((resolved_key.clone(), values));
            }
        }
        Some(result)
    }

    fn find_definition_file(definition_id: &str, search_directories: &[PathBuf]) -> String {
        for search_directory in search_directories {
            let candidate = search_directory.join(format!("{}.def.json", definition_id));
            if candidate.exists() {
                return candidate.to_string_lossy().into_owned();
            }
        }
        crate::log_error!("Couldn't find definition file with ID: {}", definition_id);
        String::new()
    }

    fn fail(msg: std::fmt::Arguments) -> ! {
        crate::log_error!("{}", msg);
        std::process::exit(1);
    }
}

impl Communication for CommandLine {
    fn has_slice(&self) -> bool {
        !self.arguments.is_empty()
    }
    fn is_sequential(&self) -> bool {
        true // We have to receive the g-code in sequential order. Start g-code before the rest and so on.
    }
    fn send_progress(&self, _progress: f64) {}
    fn send_layer_complete(&mut self, _layer_nr: i64, _z: Coord, _thickness: Coord) {}
    fn send_line_to(&mut self, _feature_type: PrintFeatureType, _to: Point3LL, _line_width: Coord, _line_thickness: Coord, _velocity: Velocity) {}
    fn send_current_position(&mut self, _position: Point3LL) {}
    fn set_extruder_for_send(&mut self, _extruder: &ExtruderTrain) {}
    fn set_layer_for_send(&mut self, _layer_nr: i64) {}
    fn send_optimized_layer_data(&mut self) {}
    fn send_print_time_material_estimates(&self) {
        let time_estimates = FffProcessor::get_instance().get_total_print_time_per_feature();
        let sum: f64 = time_estimates.iter().map(|d| d.value()).sum();
        crate::log_info!("Total print time: {:.3}", sum);
    }
    fn begin_gcode(&mut self) {}
    fn flush_gcode(&mut self) {}
    fn send_gcode_prefix(&self, _prefix: &str) {}
    fn send_slice_uuid(&self, _slice_uuid: &str) {}
    fn send_finished_slicing(&self) {}

    fn slice_next(&mut self) {
        // Count the number of mesh groups to slice for.
        let mut num_mesh_groups: usize = 1;
        for argument in self.arguments.iter().skip(2) {
            if argument.starts_with("--next") {
                num_mesh_groups += 1;
            }
        }
        set_current_slice(Slice::new(num_mesh_groups));
        let slice = current_slice_mut();

        let mut mesh_group_index: usize = 0;
        let mut last_settings: *mut Settings = &mut slice.scene.settings;

        slice.scene.extruders.reserve(self.arguments.len() >> 1); // Allocate enough memory to prevent moves.
        let scene_settings: *const Settings = &slice.scene.settings;
        slice.scene.extruders.push(ExtruderTrain::new(0, scene_settings)); // Always have one extruder.
        let mut last_extruder: usize = 0;

        let mut force_read_parent = false;
        let mut force_read_nondefault = false;

        let mut argument_index: usize = 2;
        while argument_index < self.arguments.len() {
            let mut argument = self.arguments[argument_index].clone();
            let arg_bytes = argument.as_bytes();
            if arg_bytes.first() == Some(&b'-') {
                // Starts with "-".
                if arg_bytes.get(1) == Some(&b'-') {
                    // Starts with "--".
                    if argument.starts_with("--next") {
                        mesh_group_index += 1;
                        last_settings = &mut slice.scene.mesh_groups[mesh_group_index].settings;
                    } else if argument.starts_with("--force-read-parent") || argument.starts_with("--force_read_parent") {
                        crate::log_info!("From this point on, force the parser to read values of non-leaf settings, instead of skipping over them as is proper.");
                        force_read_parent = true;
                    } else if argument.starts_with("--force-read-nondefault") || argument.starts_with("--force_read_nondefault") {
                        crate::log_info!(
                            "From this point on, if 'default_value' is not available, force the parser to read 'value' (instead of dropping it) to fill the used setting-values."
                        );
                        force_read_nondefault = true;
                    } else if argument.starts_with("--end-force-read") || argument.starts_with("--end_force_read") {
                        crate::log_info!("From this point on, reset all force-XXX values to false (don't 'force read ___' anymore).");
                        force_read_parent = false;
                        force_read_nondefault = false;
                    } else if argument.starts_with("--progress_cb")
                        || argument.starts_with("--slice_info_cb")
                        || argument.starts_with("--gcode_header_cb")
                        || argument.starts_with("--engine_info_cb")
                    {
                        // Unused in standalone engine.
                        argument_index += 1;
                    } else {
                        crate::log_error!("Unknown option: {}", argument);
                    }
                } else {
                    // Starts with "-" but not with "--".
                    let opt = arg_bytes.get(1).copied().unwrap_or(0);
                    match opt {
                        b'v' => {
                            crate::logging::set_level(crate::logging::LEVEL_DEBUG);
                        }
                        b'm' => {
                            let threads = argument[2..].parse::<i32>().unwrap_or_else(|_| Self::fail(format_args!("Invalid thread count: {}", argument)));
                            start_thread_pool(threads.max(0) as usize);
                        }
                        b'p' => {
                            // enable progress logging
                        }
                        b'd' => {
                            argument_index += 1;
                            if argument_index >= self.arguments.len() {
                                Self::fail(format_args!("Missing definition search paths"));
                            }
                            argument = self.arguments[argument_index].clone();
                            self.search_directories = split_paths(&argument);
                        }
                        b'j' => {
                            argument_index += 1;
                            if argument_index >= self.arguments.len() {
                                Self::fail(format_args!("Missing JSON file with -j argument."));
                            }
                            argument = self.arguments[argument_index].clone();
                            if self.load_json_file(Path::new(&argument), last_settings, force_read_parent, force_read_nondefault) != 0 {
                                Self::fail(format_args!("Failed to load JSON file: {}", argument));
                            }

                            // If this was the global stack, create extruders for the machine_extruder_count setting.
                            if std::ptr::eq(last_settings, &slice.scene.settings) {
                                let extruder_count = slice.scene.settings.get_usize("machine_extruder_count");
                                while slice.scene.extruders.len() < extruder_count {
                                    let n = slice.scene.extruders.len();
                                    slice.scene.extruders.push(ExtruderTrain::new(n, scene_settings));
                                }
                            }
                            // If this was an extruder stack, make sure that the extruder_nr setting is correct.
                            if std::ptr::eq(last_settings, &slice.scene.extruders[last_extruder].settings) {
                                let nr = slice.scene.extruders[last_extruder].extruder_nr;
                                slice.scene.extruders[last_extruder].settings.add("extruder_nr", &nr.to_string());
                            }
                        }
                        b'e' => {
                            let extruder_nr = cppstd::stoul(&argument[2..]).unwrap_or_else(|| Self::fail(format_args!("Invalid extruder number: {}", argument))) as usize;
                            while slice.scene.extruders.len() <= extruder_nr {
                                // Make sure we have enough extruders up to the extruder_nr that the user wanted.
                                slice.scene.extruders.push(ExtruderTrain::new(extruder_nr, scene_settings));
                            }
                            last_settings = &mut slice.scene.extruders[extruder_nr].settings;
                            unsafe { (*last_settings).add("extruder_nr", &argument[2..]) };
                            last_extruder = extruder_nr;
                        }
                        b'l' => {
                            argument_index += 1;
                            if argument_index >= self.arguments.len() {
                                Self::fail(format_args!("Missing model file with -l argument."));
                            }
                            argument = self.arguments[argument_index].clone();

                            let transformation = unsafe { (*last_settings).get_matrix4x3d("mesh_rotation_matrix") }; // The transformation applied to the model when loaded.

                            let extruder_settings: *const Settings = &slice.scene.extruders[last_extruder].settings;
                            if !load_mesh_into_mesh_group(&mut slice.scene.mesh_groups[mesh_group_index], &argument, &transformation, unsafe { &*extruder_settings }) {
                                Self::fail(format_args!("Failed to load model: {}. (error number {})", argument, std::io::Error::last_os_error().raw_os_error().unwrap_or(0)));
                            } else {
                                last_settings = &mut slice.scene.mesh_groups[mesh_group_index].meshes.last_mut().unwrap().settings;
                            }
                        }
                        b'o' => {
                            argument_index += 1;
                            if argument_index >= self.arguments.len() {
                                Self::fail(format_args!("Missing output file with -o argument."));
                            }
                            argument = self.arguments[argument_index].clone();
                            if !FffProcessor::get_instance().set_target_file(&argument) {
                                Self::fail(format_args!("Failed to open {} for output.", argument));
                            }
                        }
                        b'g' => {
                            last_settings = &mut slice.scene.mesh_groups[mesh_group_index].settings;
                        }
                        b's' => {
                            // Parse the given setting and store it.
                            argument_index += 1;
                            if argument_index >= self.arguments.len() {
                                Self::fail(format_args!("Missing setting name and value with -s argument."));
                            }
                            argument = self.arguments[argument_index].clone();
                            let value_position = match argument.find('=') {
                                Some(p) => p,
                                None => Self::fail(format_args!("Missing value in setting argument: -s {}", argument)),
                            };
                            let key = &argument[..value_position];
                            let value = &argument[value_position + 1..];
                            unsafe { (*last_settings).add(key, value) };
                        }
                        b'r' => {
                            // Read the resolved settings file.
                            argument_index += 1;
                            if argument_index >= self.arguments.len() {
                                Self::fail(format_args!("Missing setting name and value with -r argument."));
                            }
                            argument = self.arguments[argument_index].clone();
                            let settings = match Self::read_resolved_json_values_file(Path::new(&argument)) {
                                Some(s) => s,
                                None => Self::fail(format_args!("Failed to load JSON file: {}", argument)),
                            };

                            let global_identifier = "global";
                            let extruder_identifier = "extruder.";
                            let limit_to_extruder_identifier = "limit_to_extruder";

                            let mut global_settings: Vec<(String, String)> = Vec::new();
                            let mut extruder_settings: Vec<(String, Vec<(String, String)>)> = Vec::new();
                            let mut model_settings: Vec<(String, Vec<(String, String)>)> = Vec::new();
                            let mut limit_to_extruder: Vec<(String, String)> = Vec::new();

                            // The C++ code keeps these in std::unordered_map<std::string, ...>, so the extruders
                            // and models are processed in libstdc++'s hash table order (which matters: models are
                            // loaded in that order, and an extruder key processed before a lower-numbered one
                            // creates the missing extruders with its own number).
                            let unordered_order = |keys: &mut dyn Iterator<Item = String>| -> Vec<String> {
                                let mut set: UnorderedSet<String> = UnorderedSet::new_std();
                                for k in keys {
                                    set.insert(k);
                                }
                                set.iter().cloned().collect()
                            };
                            let mut extruder_keys: Vec<String> = Vec::new();
                            let mut model_keys: Vec<String> = Vec::new();
                            for key in unordered_order(&mut settings.iter().map(|(k, _)| k.clone())) {
                                let values = settings.iter().find(|(k, _)| *k == key).unwrap().1.clone();
                                if key == global_identifier {
                                    global_settings = values;
                                } else if key.starts_with(extruder_identifier) {
                                    extruder_keys.push(key.clone());
                                    extruder_settings.push((key, values));
                                } else if key == limit_to_extruder_identifier {
                                    limit_to_extruder = values;
                                } else {
                                    model_keys.push(key.clone());
                                    model_settings.push((key, values));
                                }
                            }
                            let reorder = |list: &mut Vec<(String, Vec<(String, String)>)>, keys: Vec<String>| {
                                let order = unordered_order(&mut keys.into_iter());
                                list.sort_by_key(|(k, _)| order.iter().position(|o| o == k).unwrap());
                            };
                            reorder(&mut extruder_settings, extruder_keys);
                            reorder(&mut model_settings, model_keys);

                            for (setting_key, setting_value) in &global_settings {
                                slice.scene.settings.add(setting_key, setting_value);
                            }

                            for (key, values) in &extruder_settings {
                                let extruder_nr = cppstd::atoi(&key[extruder_identifier.len()..]);
                                while slice.scene.extruders.len() <= extruder_nr as usize {
                                    slice.scene.extruders.push(ExtruderTrain::new(extruder_nr as usize, scene_settings));
                                }
                                for (setting_key, setting_value) in values {
                                    slice.scene.extruders[extruder_nr as usize].settings.add(setting_key, setting_value);
                                }
                            }

                            for (model_name, values) in &model_settings {
                                for (setting_key, setting_value) in values {
                                    slice.scene.mesh_groups[mesh_group_index].settings.add(setting_key, setting_value);
                                }

                                let transformation = slice.scene.mesh_groups[mesh_group_index].settings.get_matrix4x3d("mesh_rotation_matrix");
                                let extruder_nr = slice.scene.mesh_groups[mesh_group_index].settings.get_usize("extruder_nr");
                                let extruder_settings: *const Settings = &slice.scene.extruders[extruder_nr].settings;
                                if !load_mesh_into_mesh_group(&mut slice.scene.mesh_groups[mesh_group_index], model_name, &transformation, unsafe { &*extruder_settings }) {
                                    Self::fail(format_args!("Failed to load model: {}. (error number {})", model_name, 0));
                                }
                            }

                            for (key, value) in &limit_to_extruder {
                                let extruder_nr = cppstd::atoi(&value[extruder_identifier.len().min(value.len())..]);
                                if extruder_nr >= 0 {
                                    slice.scene.limit_to_extruder.insert(key.clone(), extruder_nr as usize);
                                }
                            }
                        }
                        _ => {
                            crate::log_error!("Unknown option: -{}", opt as char);
                            crate::application::print_call();
                            crate::application::print_help();
                            std::process::exit(1);
                        }
                    }
                }
            } else {
                crate::log_error!("Unknown option: {}", argument);
                crate::application::print_call();
                crate::application::print_help();
                std::process::exit(1);
            }
            argument_index += 1;
        }

        self.arguments.clear(); // We've processed all arguments now.

        slice.scene.mesh_groups[mesh_group_index].finalize();
        // Start slicing.
        slice.compute();

        // Finalize the processor. This adds the end g-code and reports statistics.
        FffProcessor::get_instance().finalize();
    }
}
