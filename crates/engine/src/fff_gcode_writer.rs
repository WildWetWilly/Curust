//! `FffGcodeWriter.h/.cpp`: the second half of the pipeline, turning the sliced areas into layer
//! plans and finally G-code.

use crate::application::{communication, current_slice};
use crate::bridge::{bridge_angle, make_bridge_over_infill_printable};
use crate::extruder_train::ExtruderTrain;
use crate::gcode_export::GCodeExport;
use crate::geometry::matrix::PointMatrix;
use crate::geometry::mixed_lines_set::{AnyPolyline, MixedLinesSet, PolylinePtr};
use crate::geometry::point2ll::{pt, v_size, v_size2, Point2LL};
use crate::geometry::point3ll::Point3LL;
use crate::geometry::{OpenLinesSet, Polyline, Shape};
use crate::infill::{Infill, InfillExtra, InfillZigzag};
use crate::inset_order_optimizer::{InsetOrderOptimizer, OrderRequirements, WallConfigs};
use crate::layer_plan::{LayerPlan, LinesOpts, OverhangMask, PolygonsOpts};
use crate::layer_plan_buffer::LayerPlanBuffer;
use crate::path_order_optimizer::{PathOrderOptimizer, ZSeamConfig};
use crate::path_planning::{FanSpeedLayerTimeSettings, GCodePathConfig, SpaceFillType};
use crate::raft::{Raft, RaftLayerType};
use crate::retraction_config::RetractionAndWipeConfig;
use crate::settings::enums::{
    DraftShieldHeightLimitation, EFillMethod, EPlatformAdhesion, ESupportStructure, ESurfaceMode, EZSeamCornerPrefType, EZSeamType,
};
use crate::settings::path_config_storage::MeshPathConfigs;
use crate::settings::settings::{Settings, MAX_EXTRUDERS};
use crate::settings::types::{AngleDegrees, AngleRadians, Duration, LayerIndex, Ratio};
use crate::slice_data_storage::{SkinPart, SliceDataStorage, SliceLayer, SliceLayerPart, SliceMeshStorage, SupportLayer, SupportStorage};
use crate::utils::aabb::AABB;
use crate::utils::coord::{int2mm, mm2int, Coord};
use crate::utils::extrusion_line::VariableWidthLines;
use crate::utils::linear_alg_2d as la;
use crate::utils::math::round_divide;
use crate::utils::polygon_utils::PolygonUtils;
use crate::utils::section_type::SectionType;
use crate::utils::simplify::Simplify;
use crate::utils::sparse_grid::SparsePointGridInclusive;
use std::fs::File;
use std::io::BufWriter;
use std::sync::Arc;

pub use crate::extruder_use::{ExtruderPrime, ExtruderUse};
pub use crate::utils::layer_vector::LayerVector;

/// `LinesOrderingMethod`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinesOrderingMethod {
    Basic,
    Monotonic,
    Interlaced,
}

/// `RoofingFlooringSettingsNames`
struct RoofingFlooringSettingsNames {
    extruder_nr: &'static str,
    pattern: &'static str,
    monotonic: &'static str,
}

const ROOFING_SETTINGS_NAMES: RoofingFlooringSettingsNames = RoofingFlooringSettingsNames { extruder_nr: "roofing_extruder_nr", pattern: "roofing_pattern", monotonic: "roofing_monotonic" };
const FLOORING_SETTINGS_NAMES: RoofingFlooringSettingsNames =
    RoofingFlooringSettingsNames { extruder_nr: "flooring_extruder_nr", pattern: "flooring_pattern", monotonic: "flooring_monotonic" };

#[inline]
fn scene() -> &'static crate::slice::Scene {
    &current_slice().scene
}

#[inline]
fn mesh_group_settings() -> &'static Settings {
    &current_slice().scene.current_mesh_group().settings
}

#[inline]
fn extruder(nr: usize) -> &'static ExtruderTrain {
    &current_slice().scene.extruders[nr]
}

#[inline]
fn max_c(a: Coord, b: Coord) -> Coord {
    if a < b {
        b
    } else {
        a
    }
}

/// The infill generator with the parameters of the long C++ constructor.
#[allow(clippy::too_many_arguments)]
pub(crate) fn make_infill(
    pattern: EFillMethod,
    zig_zaggify: bool,
    connect_polygons: bool,
    in_outline: Shape,
    line_width: Coord,
    line_distance: Coord,
    overlap: Coord,
    multiplier: usize,
    fill_angle: AngleDegrees,
    z: Coord,
    shift: Coord,
    max_resolution: Coord,
    max_deviation: Coord,
    wall_line_count: usize,
    small_area_width: Coord,
    infill_origin: Point2LL,
    skip_line_stitching: bool,
    fill_gaps: bool,
    connected_zigzags: bool,
    use_endpieces: bool,
    skip_some_zags: bool,
    zag_skip_count: usize,
    pocket_size: Coord,
) -> Infill {
    Infill::new(
        pattern,
        zig_zaggify,
        connect_polygons,
        in_outline,
        line_width,
        line_distance,
        overlap,
        multiplier,
        fill_angle,
        z,
        shift,
        max_resolution,
        max_deviation,
        InfillExtra {
            wall_line_count,
            small_area_width,
            infill_origin,
            skip_line_stitching,
            zigzag: Some(InfillZigzag { fill_gaps, connected_zigzags, use_endpieces, skip_some_zags, zag_skip_count, pocket_size }),
        },
    )
}

/// A random UUID, like `boost::uuids::random_generator`. For testing, `CURA_FIXED_UUID` overrides it
/// (the reference build has the same hook).
fn make_instance_uuid() -> String {
    if let Ok(fixed) = std::env::var("CURA_FIXED_UUID") {
        return fixed;
    }
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let mut state = (nanos as u64) ^ ((nanos >> 64) as u64) ^ (std::process::id() as u64).rotate_left(32) ^ 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        // splitmix64
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    let mut bytes = [0u8; 16];
    for chunk in bytes.chunks_mut(8) {
        chunk.copy_from_slice(&next().to_le_bytes());
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: Vec<String> = bytes.iter().map(|b| format!("{:02x}", b)).collect();
    format!("{}-{}-{}-{}-{}", hex[0..4].concat(), hex[4..6].concat(), hex[6..8].concat(), hex[8..10].concat(), hex[10..16].concat())
}

pub struct FffGcodeWriter {
    /// The maximal height of all previously sliced meshgroups, used to avoid collision when moving to the next meshgroup to print.
    max_object_height: Coord,
    pub gcode: GCodeExport,
    extruder_order_per_layer: LayerVector<Vec<ExtruderUse>>,
    /// For each extruder, the order of the meshes (first element is first mesh to be printed)
    mesh_order_per_extruder: Vec<Vec<usize>>,
    /// For each extruder the layer at which it is primed.
    extruder_prime_layer_nr: [LayerIndex; MAX_EXTRUDERS],
    /// The settings used relating to minimal layer time
    fan_speed_layer_time_settings_per_extruder: Vec<FanSpeedLayerTimeSettings>,
    /// The UUID of the current slice.
    slice_uuid: String,
    /// The persistent state of the `LayerPlanBuffer` (which in C++ is a member of the writer).
    extruder_used_in_meshgroup: Vec<bool>,
}

impl Default for FffGcodeWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl FffGcodeWriter {
    pub fn new() -> Self {
        FffGcodeWriter {
            max_object_height: 0,
            gcode: GCodeExport::new(),
            extruder_order_per_layer: LayerVector::default(),
            mesh_order_per_extruder: Vec::new(),
            // initialize all as max layer_nr, so that they get updated to the lowest layer on which they are used.
            extruder_prime_layer_nr: [i32::MAX as LayerIndex; MAX_EXTRUDERS],
            fan_speed_layer_time_settings_per_extruder: Vec::new(),
            slice_uuid: make_instance_uuid(),
            extruder_used_in_meshgroup: vec![false; MAX_EXTRUDERS],
        }
    }

    pub fn set_target_file(&mut self, filename: &str) -> bool {
        match File::create(filename) {
            Ok(f) => {
                // A text-mode std::ofstream: "\r\n" line endings on Windows.
                self.gcode.set_output_stream(Box::new(crate::utils::text_mode::TextModeWriter::new(BufWriter::new(f))));
                true
            }
            Err(_) => false,
        }
    }

    pub fn set_target_stream(&mut self, stream: Box<dyn std::io::Write>) {
        self.gcode.set_output_stream(stream);
    }

    /// Push buffered G-code into the target stream.
    pub fn flush_target_stream(&mut self) {
        self.gcode.flush_output_stream();
    }

    pub fn get_extruder_actual_use(&self, extruder_nr: usize) -> bool {
        self.gcode.get_extruder_is_used(extruder_nr)
    }

    pub fn get_total_filament_used(&self, extruder_nr: usize) -> f64 {
        self.gcode.get_total_filament_used(extruder_nr)
    }

    pub fn get_total_print_time_per_feature(&self) -> Vec<Duration> {
        self.gcode.get_total_print_time_per_feature()
    }

    /// Write all the gcode for the current meshgroup.
    pub fn write_gcode(&mut self, storage: &mut SliceDataStorage) {
        let start_extruder_nr = self.get_start_extruder(storage);
        self.gcode.pre_setup(start_extruder_nr);
        let uuid = self.slice_uuid.clone();
        self.gcode.set_slice_uuid(&uuid);

        let first_mesh_group = scene().current_mesh_group == 0;
        if first_mesh_group {
            self.gcode.reset_total_print_time_and_filament();
            self.gcode.set_initial_and_build_volume_temps(start_extruder_nr);
        }

        communication().begin_gcode();

        self.set_config_fan_speed_layer_time();

        Self::set_config_retraction_and_wipe(storage);

        if first_mesh_group {
            let should_prime_extruder = self.gcode.initialize_extruder_trains(storage, start_extruder_nr);

            if !should_prime_extruder {
                // set to most negative number so that layer processing never primes this extruder anymore.
                self.extruder_prime_layer_nr[start_extruder_nr] = i32::MIN as LayerIndex;
            }
        } else {
            self.process_next_mesh_group_code(storage);
        }

        let mut total_layers: usize = 0;
        for mesh in storage.meshes.iter_mut() {
            let mut mesh_layer_num = mesh.layers.len();

            // calculation of _actual_ number of layers in loop.
            while mesh_layer_num > 0 && mesh.layers[mesh_layer_num - 1].get_outlines(false).is_empty() {
                mesh_layer_num -= 1;
            }

            total_layers = total_layers.max(mesh_layer_num);

            Self::set_infill_and_skin_angles(mesh);
        }

        Self::set_support_angles(storage);

        self.gcode.write_layer_count_comment(total_layers);

        {
            // calculate the mesh order for each extruder
            let extruder_count = scene().extruders.len();
            self.mesh_order_per_extruder.clear(); // Might be not empty in case of sequential printing.
            for extruder_nr in 0..extruder_count {
                let order = self.calculate_mesh_order(storage, extruder_nr);
                self.mesh_order_per_extruder.push(order);
            }
        }
        let extruder_settings = &extruder(self.gcode.get_extruder_nr()).settings;
        // in case the prime blob is enabled the brim already starts from the closest start position which is blob location
        // also in case of one at a time printing the first move of every object shouldn't be start position of machine
        if !extruder_settings.get_bool("prime_blob_enable") && extruder_settings.get_str("print_sequence") != "one_at_a_time" {
            // Setting first travel move of the first extruder to the machine start position
            let p = Point3LL::new(
                extruder_settings.get_coord("machine_extruder_start_pos_x"),
                extruder_settings.get_coord("machine_extruder_start_pos_y"),
                self.gcode.get_position_z(),
            );
            self.gcode.write_travel(p, extruder_settings.get_velocity("speed_travel"), None);
        }

        self.calculate_extruder_order_per_layer(storage);
        self.calculate_prime_layer_per_extruder(storage);

        if mesh_group_settings().get_bool("magic_spiralize") {
            self.find_layer_seams_for_spiralize(storage, total_layers);
        }

        let storage: &SliceDataStorage = storage;
        let mut layer_plan_buffer = LayerPlanBuffer::with_extruder_used_in_meshgroup(std::mem::take(&mut self.extruder_used_in_meshgroup));

        let mut process_layer_starting_layer_nr: LayerIndex = 0;
        let has_raft = mesh_group_settings().get_enum::<EPlatformAdhesion>("adhesion_type") == EPlatformAdhesion::RAFT;
        if has_raft {
            self.process_raft(storage, &mut layer_plan_buffer);
            // process filler layers to fill the airgap with helper object (support etc) so that they stick better to the raft.
            // only process the filler layers if there is anything to print in them.
            for extruder_is_used_in_filler_layers in storage.get_extruders_used_on_layer(-1) {
                if extruder_is_used_in_filler_layers {
                    process_layer_starting_layer_nr = -(Raft::get_filler_layer_count() as LayerIndex);
                    break;
                }
            }
        }

        let mut layer_nr = process_layer_starting_layer_nr;
        while layer_nr < total_layers as LayerIndex {
            let layer_plan = self.process_layer(storage, layer_nr, total_layers, &layer_plan_buffer);
            layer_plan_buffer.handle(layer_plan, &mut self.gcode);
            crate::progress::message_progress(crate::progress::Stage::Export, layer_nr.max(0) as i64 + 1, total_layers as i64);
            layer_nr += 1;
        }

        layer_plan_buffer.flush(&mut self.gcode);
        self.extruder_used_in_meshgroup = layer_plan_buffer.into_extruder_used_in_meshgroup();

        // Store the object height for when we are printing multiple objects, as we need to clear every one of them when moving to the next position.
        self.max_object_height = self.max_object_height.max(storage.model_max.z);

        let force = true;
        let rc = storage.retraction_wipe_config_per_extruder[self.gcode.get_extruder_nr()].retraction_config;
        self.gcode.write_retraction(&rc, force, false, None); // retract after finishing each meshgroup
    }

    fn find_spiralized_layer_seam_vertex_index(storage: &SliceDataStorage, mesh: &SliceMeshStorage, layer_nr: usize, last_layer_nr: i32) -> u32 {
        let layer = &mesh.layers[layer_nr];

        // last_layer_nr will be < 0 until we have processed the first non-empty layer
        if last_layer_nr < 0 {
            // If the user has specified a z-seam location, use the vertex closest to that location for the seam vertex
            // in the first layer that has a part with insets.
            let mut seam_pos = pt(0, 0);
            if mesh.settings().get_enum::<EZSeamType>("z_seam_type") == EZSeamType::USER_SPECIFIED {
                seam_pos = mesh.get_z_seam_hint();
            }
            let mut first_wall = Shape::default();
            first_wall.push(layer.parts[0].spiral_wall[0].clone());
            PolygonUtils::find_closest_polygon(seam_pos, &layer.parts[0].spiral_wall[0], crate::utils::polygon_utils::NO_PENALTY).point_idx as u32
        } else {
            // note that the code below doesn't assume that last_layer_nr is one less than layer_nr but the print is going
            // to come out pretty weird if that isn't true as it implies that there are empty layers

            let last_wall = unsafe { &(&*storage.spiralize_wall_outlines[last_layer_nr as usize])[0] };
            // Even though this is just one (contiguous) part, the spiralize wall may still be multiple parts if the part is somewhere thinner than 1 line width.
            let wall = &layer.parts[0].spiral_wall[0];
            let n_points = wall.len();
            let last_wall_seam_vertex = last_wall[storage.spiralize_seam_vertex_indices[last_layer_nr as usize] as usize];

            // seam_vertex_idx is going to be the index of the seam vertex in the current wall polygon
            // initially we choose the vertex that is closest to the seam vertex in the last spiralized layer processed
            let mut seam_vertex_idx = PolygonUtils::find_nearest_vert_polygon(last_wall_seam_vertex, wall) as i32;

            // now we check that the vertex following the seam vertex is to the left of the seam vertex in the last layer
            // and if it isn't, we move forward
            if v_size(last_wall_seam_vertex - wall[seam_vertex_idx as usize]) >= mesh.settings().get_coord("meshfix_maximum_resolution") {
                // get the inward normal of the last layer seam vertex
                let last_wall_seam_vertex_inward_normal =
                    PolygonUtils::get_vertex_inward_normal(last_wall, storage.spiralize_seam_vertex_indices[last_layer_nr as usize] as u32);

                // create a vector from the normal so that we can then test the vertex following the candidate seam vertex to make sure it is on the correct side
                let last_wall_seam_vertex_vector = last_wall_seam_vertex + last_wall_seam_vertex_inward_normal;

                // now test the vertex following the candidate seam vertex and if it lies to the left of the vector, it's good to use
                let a = la::get_angle_left(last_wall_seam_vertex_vector, last_wall_seam_vertex, wall[((seam_vertex_idx + 1) as usize) % n_points]);

                if a <= 0.0 || a >= std::f64::consts::PI {
                    // the vertex was not on the left of the vector so move the seam vertex on
                    seam_vertex_idx = ((seam_vertex_idx as usize + 1) % n_points) as i32;
                }
            }

            seam_vertex_idx as u32
        }
    }

    fn find_layer_seams_for_spiralize(&self, storage: &mut SliceDataStorage, total_layers: usize) {
        // The spiral has to continue on in an anti-clockwise direction from where the last layer finished, it can't jump backwards

        // we track the seam position for each layer and ensure that the seam position for next layer continues in the right direction

        storage.spiralize_wall_outlines = vec![std::ptr::null(); total_layers]; // default is no information available
        storage.spiralize_seam_vertex_indices = vec![0; total_layers];

        let mut last_layer_nr: i32 = -1; // layer number of the last non-empty layer processed (for any extruder or mesh)

        for layer_nr in 0..total_layers {
            let mut done_this_layer = false;

            // iterate through extruders until we find a mesh that has a part with insets
            let extruder_order = self.extruder_order_per_layer.get(layer_nr as LayerIndex);
            let mut extruder_idx = 0;
            while !done_this_layer && extruder_idx < extruder_order.len() {
                let extruder_nr = extruder_order[extruder_idx].extruder_nr;
                // iterate through this extruder's meshes until we find a part with insets
                let mesh_order = &self.mesh_order_per_extruder[extruder_nr];
                for &mesh_idx in mesh_order.iter() {
                    let mesh = &storage.meshes[mesh_idx];
                    // if this mesh has layer data for this layer process it
                    if !done_this_layer && mesh.layers.len() > layer_nr {
                        let layer = &mesh.layers[layer_nr];
                        // if the first part in the layer (if any) has insets, process it
                        if !layer.parts.is_empty() && !layer.parts[0].spiral_wall.is_empty() {
                            // save the seam vertex index for this layer as we need it to determine the seam vertex index for the next layer
                            let idx = Self::find_spiralized_layer_seam_vertex_index(storage, mesh, layer_nr, last_layer_nr);
                            let outline_ptr = &layer.parts[0].spiral_wall as *const Shape;
                            storage.spiralize_seam_vertex_indices[layer_nr] = idx as i32;
                            // save the wall outline for this layer so it can be used in the spiralize interpolation calculation
                            storage.spiralize_wall_outlines[layer_nr] = outline_ptr;
                            last_layer_nr = layer_nr as i32;
                            // ignore any further meshes/extruders for this layer
                            done_this_layer = true;
                        }
                    }
                }
                extruder_idx += 1;
            }
        }
    }

    fn set_config_fan_speed_layer_time(&mut self) {
        for train in scene().extruders.iter() {
            let s = &train.settings;
            let mut f = FanSpeedLayerTimeSettings {
                cool_min_layer_time: s.get_duration("cool_min_layer_time"),
                cool_min_layer_time_overhang: s.get_duration("cool_min_layer_time_overhang"),
                cool_min_layer_time_overhang_min_segment_length: s.get_coord("cool_min_layer_time_overhang_min_segment_length"),
                cool_min_layer_time_fan_speed_max: s.get_duration("cool_min_layer_time_fan_speed_max"),
                cool_fan_speed_0: s.get_ratio("cool_fan_speed_0") * 100.0,
                cool_fan_speed_min: s.get_ratio("cool_fan_speed_min") * 100.0,
                cool_fan_speed_max: s.get_ratio("cool_fan_speed_max") * 100.0,
                cool_min_speed: s.get_velocity("cool_min_speed"),
                cool_fan_full_layer: s.get_layer_index("cool_fan_full_layer"),
            };
            if !s.get_bool("cool_fan_enabled") {
                f.cool_fan_speed_0 = 0.0;
                f.cool_fan_speed_min = 0.0;
                f.cool_fan_speed_max = 0.0;
            }
            self.fan_speed_layer_time_settings_per_extruder.push(f);
        }
    }

    fn retraction_and_wipe_config_from_settings(settings: &Settings, config: &mut RetractionAndWipeConfig) {
        let retraction_config = &mut config.retraction_config;
        retraction_config.distance = if settings.get_bool("retraction_enable") { settings.get_f64("retraction_amount") } else { 0.0 }; // Retraction distance in mm.
        retraction_config.retract_during_travel = settings.get_ratio("retraction_during_travel_ratio");
        retraction_config.keep_retracting_during_travel = settings.get_bool("keep_retracting_during_travel");
        retraction_config.prime_during_travel = settings.get_ratio("prime_during_travel_ratio");
        retraction_config.prime_volume = settings.get_f64("retraction_extra_prime_amount"); // Extra prime volume in mm^3.
        retraction_config.speed = settings.get_velocity("retraction_retract_speed");
        retraction_config.prime_speed = settings.get_velocity("retraction_prime_speed");
        retraction_config.z_hop = settings.get_coord("retraction_hop");
        retraction_config.retraction_min_travel_distance = settings.get_coord("retraction_min_travel");
        retraction_config.retraction_extrusion_window = settings.get_f64("retraction_extrusion_window"); // Window to count retractions in in mm of extruded filament.
        retraction_config.retraction_count_max = settings.get_usize("retraction_count_max");

        config.retraction_hop_after_extruder_switch = settings.get_bool("retraction_hop_after_extruder_switch") as Coord;
        config.switch_extruder_extra_prime_amount = settings.get_f64("switch_extruder_extra_prime_amount");
        let switch_retraction_config = &mut config.extruder_switch_retraction_config;
        switch_retraction_config.distance = settings.get_f64("switch_extruder_retraction_amount"); // Retraction distance in mm.
        switch_retraction_config.prime_volume = 0.0;
        switch_retraction_config.speed = settings.get_velocity("switch_extruder_retraction_speed");
        switch_retraction_config.prime_speed = settings.get_velocity("switch_extruder_prime_speed");
        switch_retraction_config.z_hop = settings.get_coord("retraction_hop_after_extruder_switch_height");
        switch_retraction_config.retraction_min_travel_distance = 0; // No limitation on travel distance for an extruder switch retract.
        switch_retraction_config.retraction_extrusion_window = 99999.9; // So that extruder switch retractions won't affect the retraction buffer.
        switch_retraction_config.retraction_count_max = 9999999; // Extruder switch retraction is never limited.

        let wipe_config = &mut config.wipe_config;

        wipe_config.retraction_enable = settings.get_bool("wipe_retraction_enable");
        wipe_config.retraction_config.distance = settings.get_f64("wipe_retraction_amount");
        wipe_config.retraction_config.speed = settings.get_velocity("wipe_retraction_retract_speed");
        wipe_config.retraction_config.prime_speed = settings.get_velocity("wipe_retraction_prime_speed");
        wipe_config.retraction_config.prime_volume = settings.get_f64("wipe_retraction_extra_prime_amount");
        wipe_config.retraction_config.retraction_min_travel_distance = 0;
        wipe_config.retraction_config.retraction_extrusion_window = f64::MAX;
        wipe_config.retraction_config.retraction_count_max = usize::MAX;

        wipe_config.pause = settings.get_duration("wipe_pause");

        wipe_config.hop_enable = settings.get_bool("wipe_hop_enable");
        wipe_config.hop_amount = settings.get_coord("wipe_hop_amount");
        wipe_config.hop_speed = settings.get_velocity("wipe_hop_speed");

        wipe_config.brush_pos_x = settings.get_coord("wipe_brush_pos_x");
        wipe_config.repeat_count = settings.get_usize("wipe_repeat_count");
        wipe_config.move_distance = settings.get_coord("wipe_move_distance");
        wipe_config.move_speed = settings.get_velocity("speed_travel");
        wipe_config.max_extrusion_mm3 = settings.get_f64("max_extrusion_before_wipe");
        wipe_config.clean_between_layers = settings.get_bool("clean_between_layers");
    }

    fn set_config_retraction_and_wipe(storage: &mut SliceDataStorage) {
        for (extruder_index, train) in scene().extruders.iter().enumerate() {
            Self::retraction_and_wipe_config_from_settings(&train.settings, &mut storage.retraction_wipe_config_per_extruder[extruder_index]);
        }
        for mesh in storage.meshes.iter_mut() {
            let settings = mesh.settings();
            Self::retraction_and_wipe_config_from_settings(settings, &mut mesh.retraction_wipe_config);
        }
    }

    fn get_start_extruder(&self, storage: &SliceDataStorage) -> usize {
        let mesh_group_settings = mesh_group_settings();
        let adhesion_type: EPlatformAdhesion = mesh_group_settings.get_enum("adhesion_type");
        let skirt_brim_extruder_nr = mesh_group_settings.get_int("skirt_brim_extruder_nr");
        let skirt_brim_extruder: Option<&ExtruderTrain> = if skirt_brim_extruder_nr < 0 { None } else { Some(mesh_group_settings.get_extruder_train("skirt_brim_extruder_nr")) };

        let mut start_extruder_nr: usize = 0;
        if adhesion_type == EPlatformAdhesion::SKIRT
            && skirt_brim_extruder.is_some_and(|e| e.settings.get_int("skirt_line_count") > 0 || e.settings.get_coord("skirt_brim_minimal_length") > 0)
        {
            start_extruder_nr = skirt_brim_extruder.unwrap().extruder_nr;
        } else if (adhesion_type == EPlatformAdhesion::BRIM || mesh_group_settings.get_bool("prime_tower_brim_enable"))
            && skirt_brim_extruder.is_some_and(|e| e.settings.get_int("brim_line_count") > 0 || e.settings.get_coord("skirt_brim_minimal_length") > 0)
        {
            start_extruder_nr = skirt_brim_extruder.unwrap().extruder_nr;
        } else if adhesion_type == EPlatformAdhesion::RAFT && skirt_brim_extruder.is_some() {
            start_extruder_nr = mesh_group_settings.get_extruder_train("raft_base_extruder_nr").extruder_nr;
        } else {
            // No adhesion.
            if mesh_group_settings.get_bool("support_enable") && mesh_group_settings.get_bool("support_brim_enable") {
                start_extruder_nr = mesh_group_settings.get_extruder_train("support_infill_extruder_nr").extruder_nr;
            } else {
                let extruder_is_used = storage.get_extruders_used();
                for (extruder_nr, &used) in extruder_is_used.iter().enumerate() {
                    start_extruder_nr = extruder_nr;
                    if used {
                        break;
                    }
                }
            }
        }
        debug_assert!(start_extruder_nr < scene().extruders.len(), "start_extruder_nr must be a valid extruder");
        start_extruder_nr
    }

    fn set_infill_and_skin_angles(mesh: &mut SliceMeshStorage) {
        let settings = mesh.settings();
        if mesh.infill_angles.is_empty() {
            mesh.infill_angles = settings.get_vec_angle_degrees("infill_angles");
            if mesh.infill_angles.is_empty() {
                // user has not specified any infill angles so use defaults
                let infill_pattern: EFillMethod = settings.get_enum("infill_pattern");
                if infill_pattern == EFillMethod::CROSS || infill_pattern == EFillMethod::CROSS_3D {
                    mesh.infill_angles.push(AngleDegrees::new(22.0)); // put most infill lines in between 45 and 0 degrees
                } else {
                    mesh.infill_angles.push(AngleDegrees::new(45.0)); // generally all infill patterns use 45 degrees
                    if infill_pattern == EFillMethod::LINES || infill_pattern == EFillMethod::ZIG_ZAG {
                        // lines and zig zag patterns default to also using 135 degrees
                        mesh.infill_angles.push(AngleDegrees::new(135.0));
                    }
                }
            }
        }

        let defaults = |angles: &mut Vec<AngleDegrees>, key: &str| {
            if angles.is_empty() {
                *angles = settings.get_vec_angle_degrees(key);
                if angles.is_empty() {
                    // user has not specified any infill angles so use defaults
                    angles.push(AngleDegrees::new(45.0));
                    angles.push(AngleDegrees::new(135.0));
                }
            }
        };
        defaults(&mut mesh.roofing_angles, "roofing_angles");
        defaults(&mut mesh.flooring_angles, "flooring_angles");
        defaults(&mut mesh.skin_angles, "skin_angles");
    }

    fn set_support_angles(storage: &mut SliceDataStorage) {
        let mesh_group_settings = mesh_group_settings();
        let support_infill_extruder = mesh_group_settings.get_extruder_train("support_infill_extruder_nr");
        storage.support.support_infill_angles = support_infill_extruder.settings.get_vec_angle_degrees("support_infill_angles");
        if storage.support.support_infill_angles.is_empty() {
            storage.support.support_infill_angles.push(AngleDegrees::new(0.0));
        }

        let support_extruder_nr_layer_0 = mesh_group_settings.get_extruder_train("support_extruder_nr_layer_0");
        storage.support.support_infill_angles_layer_0 = support_extruder_nr_layer_0.settings.get_vec_angle_degrees("support_infill_angles");
        if storage.support.support_infill_angles_layer_0.is_empty() {
            storage.support.support_infill_angles_layer_0.push(AngleDegrees::new(0.0));
        }

        let get_interface_angles = |storage: &SliceDataStorage, extruder: &ExtruderTrain, interface_angles_setting: &str, pattern: EFillMethod, interface_height_setting: &str| {
            let mut angles = extruder.settings.get_vec_angle_degrees(interface_angles_setting);
            if angles.is_empty() {
                if pattern == EFillMethod::CONCENTRIC {
                    angles.push(AngleDegrees::new(0.0)); // Concentric has no rotation.
                } else if pattern == EFillMethod::TRIANGLES {
                    angles.push(AngleDegrees::new(90.0)); // Triangular support interface shouldn't alternate every layer.
                } else {
                    for mesh in storage.meshes.iter() {
                        if mesh.settings().get_coord(interface_height_setting) >= 2 * mesh_group_settings.get_coord("layer_height") {
                            // Some roofs are quite thick.
                            // Alternate between the two kinds of diagonal: / and \ .
                            angles.push(AngleDegrees::new(45.0));
                            angles.push(AngleDegrees::new(135.0));
                        }
                    }
                    if angles.is_empty() {
                        angles.push(AngleDegrees::new(90.0)); // Perpendicular to support lines.
                    }
                }
            }
            angles
        };

        let roof_extruder = mesh_group_settings.get_extruder_train("support_roof_extruder_nr");
        storage.support.support_roof_angles =
            get_interface_angles(storage, roof_extruder, "support_roof_angles", roof_extruder.settings.get_enum("support_roof_pattern"), "support_roof_height");

        let bottom_extruder = mesh_group_settings.get_extruder_train("support_bottom_extruder_nr");
        storage.support.support_bottom_angles =
            get_interface_angles(storage, bottom_extruder, "support_bottom_angles", bottom_extruder.settings.get_enum("support_bottom_pattern"), "support_bottom_height");
    }

    fn process_next_mesh_group_code(&mut self, storage: &SliceDataStorage) {
        self.gcode.write_fan_command(0.0, None);
        self.gcode.set_z((self.max_object_height + 5000) as i32);

        communication().send_current_position(Point3LL::from_2d(self.gcode.get_position_xy(), 0));
        let speed_travel = extruder(self.gcode.get_extruder_nr()).settings.get_velocity("speed_travel");
        let xy = self.gcode.get_position_xy();
        self.gcode.write_travel_2d(xy, speed_travel);
        let start_pos = pt(storage.model_min.x, storage.model_min.y);
        self.gcode.write_travel_2d(start_pos, speed_travel);

        let nr = self.gcode.get_extruder_nr();
        self.gcode.process_initial_layer_temperature(storage, nr);
    }

    fn raft_fan_settings(&self, fan_speed: f64) -> Vec<FanSpeedLayerTimeSettings> {
        let mut v = self.fan_speed_layer_time_settings_per_extruder.clone(); // copy so that we change only the local copy
        for f in v.iter_mut() {
            f.cool_fan_speed_min = fan_speed;
            f.cool_fan_speed_0 = fan_speed; // ignore initial layer fan speed stuff
        }
        v
    }

    fn process_raft<'a>(&mut self, storage: &'a SliceDataStorage, layer_plan_buffer: &mut LayerPlanBuffer<'a>) {
        let mesh_group_settings = mesh_group_settings();
        let base_extruder_nr = mesh_group_settings.get_extruder_train("raft_base_extruder_nr").extruder_nr;
        let interface_extruder_nr = mesh_group_settings.get_extruder_train("raft_interface_extruder_nr").extruder_nr;
        let surface_extruder_nr = mesh_group_settings.get_extruder_train("raft_surface_extruder_nr").extruder_nr;

        let mut z: Coord = 0;
        let initial_raft_layer_nr: LayerIndex = -(Raft::get_total_extra_layers() as LayerIndex);
        let interface_settings = &mesh_group_settings.get_extruder_train("raft_interface_extruder_nr").settings;
        let num_interface_layers = interface_settings.get_usize("raft_interface_layers");
        let surface_settings = &mesh_group_settings.get_extruder_train("raft_surface_extruder_nr").settings;
        let num_surface_layers = surface_settings.get_usize("raft_surface_layers");

        // some infill config for all lines infill generation below
        let infill_multiplier = 1; // rafts use single lines
        let extra_infill_shift = 0;
        let fill_gaps = true;
        let retract_before_outer_wall = false;
        let wipe_dist: Coord = 0;

        let mut raft_polygons = Shape::default();
        let mut last_planned_position: Option<Point2LL> = None;

        let mut current_extruder_nr = base_extruder_nr;
        let model_center = storage.get_model_bounding_box().flatten().get_middle();

        {
            // raft base layer
            let base_settings = &mesh_group_settings.get_extruder_train("raft_base_extruder_nr").settings;
            let layer_nr = initial_raft_layer_nr;
            let layer_height = base_settings.get_coord("raft_base_thickness");
            z += layer_height;
            let comb_offset = max_c(base_settings.get_coord("raft_base_line_spacing"), base_settings.get_coord("raft_base_line_width"));

            let fan = self.raft_fan_settings(base_settings.get_ratio("raft_base_fan_speed") * 100.0);

            let line_width = base_settings.get_coord("raft_base_line_width");
            let avoid_distance = base_settings.get_coord("travel_avoid_distance");
            let mut gcode_layer = Box::new(LayerPlan::new(storage, layer_nr, z, layer_height, base_extruder_nr, &fan, comb_offset, line_width, avoid_distance));
            gcode_layer.set_is_inside(true);

            communication().send_layer_complete(layer_nr, z, layer_height);

            let mut raft_lines = OpenLinesSet::default();
            let fill_angle = AngleDegrees::new(if (num_surface_layers + num_interface_layers) % 2 != 0 { 45.0 } else { 135.0 }); // 90 degrees rotated from the interface layer.
            let zig_zaggify_infill = false;
            let connect_polygons = true; // causes less jerks, so better adhesion

            let wall_line_count = base_settings.get_usize("raft_base_wall_count");
            let small_area_width = 0; // A raft never has a small region due to the large horizontal expansion.
            let line_spacing = base_settings.get_coord("raft_base_line_spacing");
            let infill_overlap = base_settings.get_coord("raft_base_infill_overlap_mm");
            let line_spacing_prime_tower = base_settings.get_coord("prime_tower_raft_base_line_spacing");
            let infill_origin = Point2LL::default();
            let skip_stitching = false;
            let connected_zigzags = false;
            let use_endpieces = true;
            let skip_some_zags = false;
            let zag_skip_count = 0;
            let pocket_size = 0;
            let max_resolution = base_settings.get_coord("meshfix_maximum_resolution");
            let max_deviation = base_settings.get_coord("meshfix_maximum_deviation");

            struct ParameterizedRaftPath {
                line_spacing: Coord,
                outline: Shape,
            }

            let mut raft_outline_paths: Vec<ParameterizedRaftPath> = vec![ParameterizedRaftPath { line_spacing, outline: storage.raft_base_outline.clone() }];
            if let Some(prime_tower) = storage.prime_tower.as_ref() {
                let raft_outline_prime_tower = Shape::from_polygon(prime_tower.get_extrusion_outline(layer_nr).clone());
                if line_spacing_prime_tower == line_spacing {
                    // Base layer is shared with prime tower base
                    raft_outline_paths[0].outline = raft_outline_paths[0].outline.union_polygons_other(&raft_outline_prime_tower);
                } else {
                    // Prime tower has a different line spacing, print them separately
                    raft_outline_paths[0].outline = raft_outline_paths[0].outline.difference(&raft_outline_prime_tower);
                    raft_outline_paths.push(ParameterizedRaftPath { line_spacing: line_spacing_prime_tower, outline: raft_outline_prime_tower });
                }
            }

            for raft_outline_path in raft_outline_paths.iter() {
                let config = gcode_layer.configs_storage.raft_base_config;
                let mut infill_comp = make_infill(
                    EFillMethod::LINES,
                    zig_zaggify_infill,
                    connect_polygons,
                    raft_outline_path.outline.clone(),
                    config.get_line_width(),
                    raft_outline_path.line_spacing,
                    infill_overlap,
                    infill_multiplier,
                    fill_angle,
                    z,
                    extra_infill_shift,
                    max_resolution,
                    max_deviation,
                    wall_line_count,
                    small_area_width,
                    infill_origin,
                    skip_stitching,
                    fill_gaps,
                    connected_zigzags,
                    use_endpieces,
                    skip_some_zags,
                    zag_skip_count,
                    pocket_size,
                );
                let mut raft_paths: Vec<VariableWidthLines> = Vec::new();
                infill_comp.generate(&mut raft_paths, &mut raft_polygons, &mut raft_lines, base_settings, layer_nr as i32, SectionType::ADHESION, None, None, None, &Shape::default());
                if !raft_paths.is_empty() {
                    let z_seam_config = ZSeamConfig::new(EZSeamType::SHORTEST, gcode_layer.get_last_planned_position_or_starting_position(), EZSeamCornerPrefType::Z_SEAM_CORNER_PREF_INNER, 0);
                    let mut wall_orderer = InsetOrderOptimizer::new(
                        gcode_layer.get_layer_nr(),
                        base_settings,
                        base_extruder_nr,
                        WallConfigs::uniform(&config),
                        retract_before_outer_wall,
                        wipe_dist,
                        wipe_dist,
                        base_extruder_nr,
                        base_extruder_nr,
                        z_seam_config,
                        &raft_paths,
                        model_center,
                        Shape::default(),
                        false,
                        false,
                        Shape::default(),
                    );
                    wall_orderer.add_to_layer(&mut gcode_layer);
                }

                gcode_layer.add_lines_by_optimizer_open(
                    &raft_lines,
                    &config,
                    SpaceFillType::Lines,
                    LinesOpts { near_start_location: last_planned_position, ..Default::default() },
                );
                last_planned_position = Some(gcode_layer.get_last_planned_position_or_starting_position());
                gcode_layer.add_polygons_by_optimizer(
                    &raft_polygons,
                    &config,
                    mesh_group_settings,
                    PolygonsOpts { start_near_location: last_planned_position, ..Default::default() },
                );
                raft_polygons.clear();
                raft_lines.clear();
                last_planned_position = Some(gcode_layer.get_last_planned_position_or_starting_position());
            }

            self.end_raft_layer(storage, &mut gcode_layer, layer_nr, &mut current_extruder_nr, false);

            layer_plan_buffer.handle(gcode_layer, &mut self.gcode);
        }

        let interface_layer_height = interface_settings.get_coord("raft_interface_thickness");
        let interface_line_spacing = interface_settings.get_coord("raft_interface_line_spacing");
        let interface_fan_speed = interface_settings.get_ratio("raft_interface_fan_speed");
        let interface_line_width = interface_settings.get_coord("raft_interface_line_width");
        let interface_infill_overlap = interface_settings.get_coord("raft_interface_infill_overlap_mm");
        let interface_avoid_distance = interface_settings.get_coord("travel_avoid_distance");
        let interface_max_resolution = interface_settings.get_coord("meshfix_maximum_resolution");
        let interface_max_deviation = interface_settings.get_coord("meshfix_maximum_deviation");
        let raft_interface_z_offset = interface_settings.get_coord("raft_interface_z_offset");

        z += raft_interface_z_offset;

        for raft_interface_layer in 1..=(num_interface_layers as LayerIndex) {
            // raft interface layer
            let layer_nr = initial_raft_layer_nr + raft_interface_layer;
            z += interface_layer_height;

            let fan = self.raft_fan_settings(interface_fan_speed * 100.0);

            let comb_offset = max_c(interface_line_spacing, interface_line_width);
            let mut gcode_layer = Box::new(LayerPlan::new(
                storage,
                layer_nr,
                z,
                interface_layer_height,
                current_extruder_nr,
                &fan,
                comb_offset,
                interface_line_width,
                interface_avoid_distance,
            ));

            gcode_layer.set_is_inside(true);

            self.start_raft_layer(storage, &mut gcode_layer, layer_nr, interface_extruder_nr, &mut current_extruder_nr);

            communication().send_layer_complete(layer_nr, z, interface_layer_height);

            let config = gcode_layer.configs_storage.raft_interface_config;
            let small_offset = config.get_line_width() / 2; // Do this manually because of micron-movement created in corners when insetting a polygon that was offset with round joint type.
            let mut raft_outline_path = storage.raft_interface_outline.offset_default(-small_offset);
            raft_outline_path = Simplify::from_settings(interface_settings).shape(&raft_outline_path); // Remove those micron-movements.
            let infill_outline_width = config.get_line_width();
            let mut raft_lines = OpenLinesSet::default();
            let fill_angle = AngleDegrees::new(if ((num_surface_layers + num_interface_layers) as LayerIndex - raft_interface_layer) % 2 != 0 { 45.0 } else { 135.0 }); // 90 degrees rotated from the first top layer.
            let zig_zaggify_infill = true;
            let connect_polygons = true; // why not?

            let wall_line_count = interface_settings.get_usize("raft_interface_wall_count");
            let small_area_width = 0; // A raft never has a small region due to the large horizontal expansion.
            let infill_origin = Point2LL::default();
            let skip_stitching = false;
            let connected_zigzags = false;
            let use_endpieces = true;
            let skip_some_zags = false;
            let zag_skip_count = 0;
            let pocket_size = 0;

            if let Some(prime_tower) = storage.prime_tower.as_ref() {
                // Interface layer excludes prime tower base
                raft_outline_path = raft_outline_path.difference(&Shape::from_polygon(prime_tower.get_extrusion_outline(layer_nr).clone()));
            }

            let mut infill_comp = make_infill(
                EFillMethod::ZIG_ZAG,
                zig_zaggify_infill,
                connect_polygons,
                raft_outline_path,
                infill_outline_width,
                interface_line_spacing,
                interface_infill_overlap,
                infill_multiplier,
                fill_angle,
                z,
                extra_infill_shift,
                interface_max_resolution,
                interface_max_deviation,
                wall_line_count,
                small_area_width,
                infill_origin,
                skip_stitching,
                fill_gaps,
                connected_zigzags,
                use_endpieces,
                skip_some_zags,
                zag_skip_count,
                pocket_size,
            );
            let mut raft_paths: Vec<VariableWidthLines> = Vec::new();
            infill_comp.generate(&mut raft_paths, &mut raft_polygons, &mut raft_lines, interface_settings, layer_nr as i32, SectionType::ADHESION, None, None, None, &Shape::default());
            if !raft_paths.is_empty() {
                let z_seam_config = ZSeamConfig::new(EZSeamType::SHORTEST, gcode_layer.get_last_planned_position_or_starting_position(), EZSeamCornerPrefType::Z_SEAM_CORNER_PREF_INNER, 0);
                let mut wall_orderer = InsetOrderOptimizer::new(
                    gcode_layer.get_layer_nr(),
                    interface_settings,
                    interface_extruder_nr,
                    WallConfigs::uniform(&config),
                    retract_before_outer_wall,
                    wipe_dist,
                    wipe_dist,
                    interface_extruder_nr,
                    interface_extruder_nr,
                    z_seam_config,
                    &raft_paths,
                    model_center,
                    Shape::default(),
                    false,
                    false,
                    Shape::default(),
                );
                wall_orderer.add_to_layer(&mut gcode_layer);
            }

            gcode_layer.add_lines_by_optimizer_open(
                &raft_lines,
                &config,
                SpaceFillType::Lines,
                LinesOpts { near_start_location: last_planned_position, ..Default::default() },
            );
            last_planned_position = Some(gcode_layer.get_last_planned_position_or_starting_position());
            gcode_layer.add_polygons_by_optimizer(
                &raft_polygons,
                &config,
                mesh_group_settings,
                PolygonsOpts { start_near_location: last_planned_position, ..Default::default() },
            );

            raft_polygons.clear();
            raft_lines.clear();

            self.end_raft_layer(storage, &mut gcode_layer, layer_nr, &mut current_extruder_nr, true);

            last_planned_position = Some(gcode_layer.get_last_planned_position_or_starting_position());
            layer_plan_buffer.handle(gcode_layer, &mut self.gcode);
        }

        let surface_layer_height = surface_settings.get_coord("raft_surface_thickness");
        let surface_line_spacing = surface_settings.get_coord("raft_surface_line_spacing");
        let surface_max_resolution = surface_settings.get_coord("meshfix_maximum_resolution");
        let surface_max_deviation = surface_settings.get_coord("meshfix_maximum_deviation");
        let surface_line_width = surface_settings.get_coord("raft_surface_line_width");
        let surface_infill_overlap = surface_settings.get_coord("raft_surface_infill_overlap_mm");
        let surface_avoid_distance = surface_settings.get_coord("travel_avoid_distance");
        let surface_fan_speed = surface_settings.get_ratio("raft_surface_fan_speed");
        let surface_monotonic = surface_settings.get_bool("raft_surface_monotonic");
        let raft_surface_z_offset = interface_settings.get_coord("raft_surface_z_offset");

        z += raft_surface_z_offset;

        for raft_surface_layer in 1..=(num_surface_layers as LayerIndex) {
            // raft surface layers
            let layer_nr = initial_raft_layer_nr + 1 + num_interface_layers as LayerIndex + raft_surface_layer - 1; // +1: 1 base layer
            z += surface_layer_height;

            let fan = self.raft_fan_settings(surface_fan_speed * 100.0);

            let comb_offset = max_c(surface_line_spacing, surface_line_width);
            let mut gcode_layer = Box::new(LayerPlan::new(
                storage,
                layer_nr,
                z,
                surface_layer_height,
                current_extruder_nr,
                &fan,
                comb_offset,
                surface_line_width,
                surface_avoid_distance,
            ));

            gcode_layer.set_is_inside(true);

            // make sure that we are using the correct extruder to print raft
            self.start_raft_layer(storage, &mut gcode_layer, layer_nr, surface_extruder_nr, &mut current_extruder_nr);

            communication().send_layer_complete(layer_nr, z, surface_layer_height);

            let small_offset = gcode_layer.configs_storage.raft_interface_config.get_line_width() / 2; // Do this manually because of micron-movement created in corners when insetting a polygon that was offset with round joint type.
            let mut raft_outline_path = storage.raft_surface_outline.offset_default(-small_offset);
            raft_outline_path = Simplify::from_settings(interface_settings).shape(&raft_outline_path); // Remove those micron-movements.
            let config = gcode_layer.configs_storage.raft_surface_config;
            let infill_outline_width = config.get_line_width();
            let mut raft_lines = OpenLinesSet::default();
            let fill_angle = AngleDegrees::new(if (num_surface_layers as LayerIndex - raft_surface_layer) % 2 != 0 { 45.0 } else { 135.0 }); // Alternate between -45 and +45 degrees, ending up 90 degrees rotated from the default skin angle.
            let zig_zaggify_infill = true;

            let wall_line_count = surface_settings.get_usize("raft_surface_wall_count");
            let small_area_width = 0; // A raft never has a small region due to the large horizontal expansion.
            let infill_origin = Point2LL::default();
            let monotonic = raft_surface_layer == num_surface_layers as LayerIndex && surface_monotonic;
            let skip_stitching = monotonic;
            let connected_zigzags = false;
            let connect_polygons = false; // midway connections between polygons can make the surface less smooth
            let use_endpieces = true;
            let skip_some_zags = false;
            let zag_skip_count = 0;
            let pocket_size = 0;

            if let Some(prime_tower) = storage.prime_tower.as_ref() {
                // Surface layers exclude prime tower base
                raft_outline_path = raft_outline_path.difference(&Shape::from_polygon(prime_tower.get_extrusion_outline(layer_nr).clone()));
            }

            for raft_island in raft_outline_path.split_into_parts(false) {
                let raft_island: Shape = raft_island.0;
                let mut infill_comp = make_infill(
                    EFillMethod::ZIG_ZAG,
                    zig_zaggify_infill,
                    connect_polygons,
                    raft_island.clone(),
                    infill_outline_width,
                    surface_line_spacing,
                    surface_infill_overlap,
                    infill_multiplier,
                    fill_angle,
                    z,
                    extra_infill_shift,
                    surface_max_resolution,
                    surface_max_deviation,
                    wall_line_count,
                    small_area_width,
                    infill_origin,
                    skip_stitching,
                    fill_gaps,
                    connected_zigzags,
                    use_endpieces,
                    skip_some_zags,
                    zag_skip_count,
                    pocket_size,
                );

                let mut raft_paths: Vec<VariableWidthLines> = Vec::new();
                infill_comp.generate(&mut raft_paths, &mut raft_polygons, &mut raft_lines, surface_settings, layer_nr as i32, SectionType::ADHESION, None, None, None, &Shape::default());

                if !raft_paths.is_empty() {
                    let z_seam_config = ZSeamConfig::new(EZSeamType::SHORTEST, gcode_layer.get_last_planned_position_or_starting_position(), EZSeamCornerPrefType::Z_SEAM_CORNER_PREF_INNER, 0);
                    let mut wall_orderer = InsetOrderOptimizer::new(
                        gcode_layer.get_layer_nr(),
                        surface_settings,
                        surface_extruder_nr,
                        WallConfigs::uniform(&config),
                        retract_before_outer_wall,
                        wipe_dist,
                        wipe_dist,
                        surface_extruder_nr,
                        surface_extruder_nr,
                        z_seam_config,
                        &raft_paths,
                        model_center,
                        Shape::default(),
                        false,
                        false,
                        Shape::default(),
                    );
                    wall_orderer.add_to_layer(&mut gcode_layer);
                }

                if monotonic {
                    let monotonic_direction = AngleRadians::from(fill_angle);
                    let space_fill_type = SpaceFillType::PolyLines;
                    let max_adjacent_distance = surface_line_spacing;

                    gcode_layer.add_lines_monotonic(
                        &raft_island,
                        &raft_lines,
                        &config,
                        space_fill_type,
                        monotonic_direction,
                        max_adjacent_distance,
                        0,
                        0,
                        1.0,
                        GCodePathConfig::FAN_SPEED_DEFAULT,
                        false,
                    );
                } else {
                    gcode_layer.add_lines_by_optimizer_open(
                        &raft_lines,
                        &config,
                        SpaceFillType::Lines,
                        LinesOpts { near_start_location: last_planned_position, ..Default::default() },
                    );
                    last_planned_position = Some(gcode_layer.get_last_planned_position_or_starting_position());
                    gcode_layer.add_polygons_by_optimizer(
                        &raft_polygons,
                        &config,
                        mesh_group_settings,
                        PolygonsOpts { start_near_location: last_planned_position, ..Default::default() },
                    );
                    last_planned_position = Some(gcode_layer.get_last_planned_position_or_starting_position());
                }

                raft_polygons.clear();
                raft_lines.clear();
            }

            self.end_raft_layer(storage, &mut gcode_layer, layer_nr, &mut current_extruder_nr, true);

            layer_plan_buffer.handle(gcode_layer, &mut self.gcode);
        }
    }

    fn start_raft_layer(&self, storage: &SliceDataStorage, gcode_layer: &mut LayerPlan, _layer_nr: LayerIndex, layer_extruder: usize, current_extruder: &mut usize) {
        // If required, fill prime tower with previous extruder
        self.set_extruder_add_prime(storage, gcode_layer, *current_extruder, true);

        if *current_extruder != layer_extruder {
            // Switch to new extruder and prime
            self.set_extruder_add_prime(storage, gcode_layer, layer_extruder, true);
            *current_extruder = layer_extruder;
        }
    }

    fn end_raft_layer(&self, storage: &SliceDataStorage, gcode_layer: &mut LayerPlan, layer_nr: LayerIndex, current_extruder: &mut usize, append_to_prime_tower: bool) {
        // If required, fill prime tower with current extruder
        self.set_extruder_add_prime(storage, gcode_layer, *current_extruder, append_to_prime_tower);

        // If required, fill prime tower for other extruders
        for extruder_use in self.extruder_order_per_layer.get(layer_nr) {
            if !append_to_prime_tower || (!gcode_layer.get_prime_tower_is_planned(extruder_use.extruder_nr) && extruder_use.prime != ExtruderPrime::None) {
                self.set_extruder_add_prime(storage, gcode_layer, extruder_use.extruder_nr, append_to_prime_tower);
                *current_extruder = extruder_use.extruder_nr;
            }
        }
    }

    fn process_layer<'a>(&self, storage: &'a SliceDataStorage, layer_nr: LayerIndex, _total_layers: usize, layer_plan_buffer: &LayerPlanBuffer<'a>) -> Box<LayerPlan<'a>> {
        log_debug!("GcodeWriter processing layer {} of {}", layer_nr, _total_layers);

        let mesh_group_settings = mesh_group_settings();
        let mut layer_thickness = mesh_group_settings.get_coord("layer_height");
        let z: Coord;
        let mut include_helper_parts = true;
        if layer_nr < 0 {
            let filler_layer_count = Raft::get_filler_layer_count() as i32;
            layer_thickness = Raft::get_filler_layer_height();
            z = Raft::get_total_thickness() + (filler_layer_count as LayerIndex + layer_nr + 1) * layer_thickness;
        } else {
            let mut zz = storage.meshes[0].layers[layer_nr as usize].print_z; // stub default
            // find printZ of first actual printed mesh
            for mesh in storage.meshes.iter() {
                let s = mesh.settings();
                if layer_nr >= mesh.layers.len() as LayerIndex || s.get_bool("support_mesh") || s.get_bool("anti_overhang_mesh") || s.get_bool("cutting_mesh") || s.get_bool("infill_mesh") {
                    continue;
                }
                zz = mesh.layers[layer_nr as usize].print_z;
                layer_thickness = mesh.layers[layer_nr as usize].thickness;
                break;
            }
            z = zz;

            if layer_nr < 0 && mesh_group_settings.get_enum::<EPlatformAdhesion>("adhesion_type") == EPlatformAdhesion::RAFT {
                include_helper_parts = false;
            }
        }

        let scene = scene();

        let mut comb_offset_from_outlines: Coord = 0;
        let mut avoid_distance: Coord = 0; // minimal avoid distance is zero
        let extruder_is_used = storage.get_extruders_used();
        for extruder_nr in 0..scene.extruders.len() {
            if extruder_is_used[extruder_nr] {
                let train = &scene.extruders[extruder_nr];

                if train.settings.get_bool("travel_avoid_other_parts") {
                    avoid_distance = max_c(avoid_distance, train.settings.get_coord("travel_avoid_distance"));
                }

                comb_offset_from_outlines = max_c(comb_offset_from_outlines, train.settings.get_coord("retraction_combing_avoid_distance"));
            }
        }

        let mut max_inner_wall_width: Coord = 0;
        for mesh in storage.meshes.iter() {
            let s = mesh.settings();
            let mut mesh_inner_wall_width = s.get_coord(if s.get_usize("wall_line_count") > 1 { "wall_line_width_x" } else { "wall_line_width_0" });
            if layer_nr == 0 {
                let train = s.get_extruder_train(if s.get_usize("wall_line_count") > 1 { "wall_0_extruder_nr" } else { "wall_x_extruder_nr" });
                mesh_inner_wall_width = (mesh_inner_wall_width as f64 * train.settings.get_ratio("initial_layer_line_width_factor")) as Coord;
            }
            max_inner_wall_width = max_c(max_inner_wall_width, mesh_inner_wall_width);
        }
        let _ = max_inner_wall_width;

        let first_extruder = self.find_used_extruder_index(storage, layer_nr, false);

        let extruder_order = self.extruder_order_per_layer.get(layer_nr);

        let first_outer_wall_line_width = scene.extruders[first_extruder].settings.get_coord("wall_line_width_0");
        let mut gcode_layer = Box::new(LayerPlan::new(
            storage,
            layer_nr,
            z,
            layer_thickness,
            first_extruder,
            &self.fan_speed_layer_time_settings_per_extruder,
            comb_offset_from_outlines,
            first_outer_wall_line_width,
            avoid_distance,
        ));

        if include_helper_parts {
            // process the skirt or the brim of the starting extruder.
            let extruder_nr = gcode_layer.get_extruder();
            if !storage.skirt_brim[extruder_nr].is_empty() {
                self.process_skirt_brim(storage, &mut gcode_layer, extruder_nr, layer_nr);
            }

            // handle shield(s) first in a layer so that chances are higher that the other nozzle is wiped (for the ooze shield)
            self.process_ooze_shield(storage, &mut gcode_layer);

            self.process_draft_shield(storage, &mut gcode_layer);
        }

        let support_roof_extruder_nr = mesh_group_settings.get_extruder_train("support_roof_extruder_nr").extruder_nr;
        let support_bottom_extruder_nr = mesh_group_settings.get_extruder_train("support_bottom_extruder_nr").extruder_nr;
        let support_infill_extruder_nr = if layer_nr <= 0 {
            mesh_group_settings.get_extruder_train("support_extruder_nr_layer_0").extruder_nr
        } else {
            mesh_group_settings.get_extruder_train("support_infill_extruder_nr").extruder_nr
        };

        for extruder_use in extruder_order.iter() {
            let extruder_nr = extruder_use.extruder_nr;

            // Set extruder (if needed) and prime (if needed)
            self.set_extruder_add_prime(storage, &mut gcode_layer, extruder_nr, true);

            if include_helper_parts && (extruder_nr == support_infill_extruder_nr || extruder_nr == support_roof_extruder_nr || extruder_nr == support_bottom_extruder_nr) {
                self.add_support_to_gcode(storage, &mut gcode_layer, extruder_nr);
            }
            if layer_nr >= 0 {
                let mesh_order = &self.mesh_order_per_extruder[extruder_nr];
                for &mesh_idx in mesh_order.iter() {
                    let mesh = &*storage.meshes[mesh_idx];
                    let mesh_config = gcode_layer.configs_storage.mesh_configs[mesh_idx].clone();
                    if mesh.settings().get_enum::<ESurfaceMode>("magic_mesh_surface_mode") == ESurfaceMode::SURFACE
                        && extruder_nr == mesh.settings().get_extruder_train("wall_0_extruder_nr").extruder_nr
                    // mesh surface mode should always only be printed with the outer wall extruder!
                    {
                        self.add_mesh_layer_to_gcode_mesh_surface_mode(mesh, &mesh_config, &mut gcode_layer);
                    } else {
                        self.add_mesh_layer_to_gcode(storage, mesh_idx, extruder_nr, &mesh_config, &mut gcode_layer, layer_plan_buffer);
                    }
                }
            }
        }

        gcode_layer.apply_gradual_flow();

        gcode_layer.apply_modify_plugin();

        gcode_layer.apply_back_pressure_compensation();

        gcode_layer
    }

    fn get_extruder_need_prime_blob_during_first_layer(&self, storage: &SliceDataStorage, extruder_nr: usize) -> bool {
        let mut need_prime_blob = self.gcode.need_prime_blob();

        // check the settings if the prime blob is disabled
        if need_prime_blob {
            let used_extruders = storage.get_extruders_used();
            let is_extruder_used_overall = extruder_nr < used_extruders.len() && used_extruders[extruder_nr];
            let extruder_prime_blob_enabled = storage.get_extruder_prime_blob_enabled(extruder_nr);

            need_prime_blob = is_extruder_used_overall && extruder_prime_blob_enabled;
        }

        need_prime_blob
    }

    fn process_skirt_brim(&self, storage: &SliceDataStorage, gcode_layer: &mut LayerPlan, extruder_nr: usize, layer_nr: LayerIndex) {
        let train = extruder(extruder_nr);
        let skirt_height = train.settings.get_int("skirt_height");
        let is_skirt = train.settings.get_enum::<EPlatformAdhesion>("adhesion_type") == EPlatformAdhesion::SKIRT;
        // only create a multilayer SkirtBrim for a skirt for the height of skirt_height
        if layer_nr != 0 && (layer_nr >= skirt_height as LayerIndex || !is_skirt) {
            return;
        }
        if gcode_layer.get_skirt_brim_is_planned(extruder_nr) {
            return;
        }
        gcode_layer.set_skirt_brim_is_planned(extruder_nr);

        let original_skirt_brim = &storage.skirt_brim[extruder_nr];
        if original_skirt_brim.is_empty() {
            return;
        }

        // Start brim close to the prime location
        let start_close_to = if train.settings.get_bool("prime_blob_enable") {
            let prime_pos_is_abs = train.settings.get_bool("extruder_prime_pos_abs");
            let prime_pos = pt(train.settings.get_coord("extruder_prime_pos_x"), train.settings.get_coord("extruder_prime_pos_y"));
            if prime_pos_is_abs {
                prime_pos
            } else {
                gcode_layer.get_last_planned_position_or_starting_position() + prime_pos
            }
        } else {
            gcode_layer.get_last_planned_position_or_starting_position()
        };

        // figure out order requirements
        let mut all_brim_lines = MixedLinesSet::default();

        let line_w = (train.settings.get_coord("skirt_brim_line_width") as f64 * train.settings.get_ratio("initial_layer_line_width_factor")) as Coord;
        let searching_radius = line_w * 2;
        // (inset index, index of the line in all_brim_lines)
        let mut grid: SparsePointGridInclusive<(usize, usize)> = SparsePointGridInclusive::new(searching_radius, 0);

        for (inset_idx, offset) in storage.skirt_brim[extruder_nr].iter().enumerate() {
            for line in offset.iter() {
                if segments_count(line) > 0 {
                    let line_idx = all_brim_lines.len();
                    all_brim_lines.push(line.clone());
                    for &p in line.points().iter() {
                        grid.insert(p, (inset_idx, line_idx));
                    }
                }
            }

            // For layer_nr != 0 add only the innermost brim line (which is only the case if skirt_height > 1)
            if layer_nr != 0 {
                break;
            }
        }

        let smart_brim_ordering = train.settings.get_bool("brim_smart_ordering") && train.settings.get_enum::<EPlatformAdhesion>("adhesion_type") == EPlatformAdhesion::BRIM;
        let mut order_requirements: OrderRequirements = Vec::new();
        for cell in grid.base.grid.values() {
            for elem in cell.iter() {
                let here = elem.val;
                let loc_here = elem.point;
                let nearby_verts = grid.get_nearby_vals(loc_here, searching_radius);
                for nearby in nearby_verts {
                    if nearby.1 == here.1 || nearby.0 == here.0 {
                        continue;
                    }

                    let (lower_inset, higher_inset) = if here.0 < nearby.0 { (here, nearby) } else { (nearby, here) };

                    if smart_brim_ordering {
                        // apply "smart brim ordering" by swapping innermost and second innermost brim lines
                        // The "order requirements" tree should look like: n -> n-1 -> ... -> 3 -> 2 -> 0 -> 1
                        if lower_inset.0 == 0 && higher_inset.0 == 1 {
                            order_requirements.push((lower_inset.1, higher_inset.1));
                            continue;
                        } else if lower_inset.0 == 0 && higher_inset.0 == 2 {
                            order_requirements.push((higher_inset.1, lower_inset.1));
                            continue;
                        } else if lower_inset.0 == 1 && higher_inset.0 == 2 {
                            // not directly adjacent
                            continue;
                        }
                    }

                    if higher_inset.0 > lower_inset.0 + 1 {
                        // not directly adjacent
                        continue;
                    }

                    order_requirements.push((higher_inset.1, lower_inset.1));
                }
            }
        }
        // `unordered_multimap` iteration order does not matter for the optimizer, but sort for reproducibility.
        order_requirements.sort();

        let enable_travel_optimization = true; // Use the combing outline while deciding in which order to print the lines. Can't hurt for only one layer.
        let config = gcode_layer.configs_storage.skirt_brim_config_per_extruder[extruder_nr];

        if !all_brim_lines.is_empty() {
            gcode_layer.add_lines_by_optimizer_mixed(
                &all_brim_lines,
                &config,
                SpaceFillType::PolyLines,
                LinesOpts {
                    enable_travel_optimization,
                    near_start_location: Some(start_close_to),
                    order_requirements: if layer_nr == 0 { &order_requirements } else { &[] },
                    ..Default::default()
                },
            );
        }

        // Add the support brim after the skirt_brim to gcode_layer
        // Support brim is only added in layer 0
        // For support brim we don't care about the order, because support doesn't need to be accurate.
        if layer_nr == 0 && extruder_nr == mesh_group_settings().get_extruder_train("support_extruder_nr_layer_0").extruder_nr {
            gcode_layer.add_lines_by_optimizer_closed(
                &storage.support_brim,
                &config,
                SpaceFillType::PolyLines,
                LinesOpts { enable_travel_optimization, near_start_location: Some(start_close_to), ..Default::default() },
            );
        }
    }

    fn process_ooze_shield(&self, storage: &SliceDataStorage, gcode_layer: &mut LayerPlan) {
        let layer_nr = gcode_layer.get_layer_nr().max(0);
        let mesh_group_settings = mesh_group_settings();
        if layer_nr == 0 && mesh_group_settings.get_enum::<EPlatformAdhesion>("adhesion_type") == EPlatformAdhesion::BRIM {
            return; // ooze shield already generated by brim
        }
        if !storage.ooze_shield.is_empty() && (layer_nr as usize) < storage.ooze_shield.len() {
            let config = gcode_layer.configs_storage.skirt_brim_config_per_extruder[0];
            gcode_layer.add_polygons_by_optimizer(&storage.ooze_shield[layer_nr as usize], &config, mesh_group_settings, PolygonsOpts::default());
        }
    }

    fn process_draft_shield(&self, storage: &SliceDataStorage, gcode_layer: &mut LayerPlan) {
        let mesh_group_settings = mesh_group_settings();
        let layer_nr = gcode_layer.get_layer_nr().max(0);
        if storage.draft_protection_shield.is_empty() {
            return;
        }
        if !mesh_group_settings.get_bool("draft_shield_enabled") {
            return;
        }
        if layer_nr == 0 && mesh_group_settings.get_enum::<EPlatformAdhesion>("adhesion_type") == EPlatformAdhesion::BRIM {
            return; // draft shield already generated by brim
        }

        if mesh_group_settings.get_enum::<DraftShieldHeightLimitation>("draft_shield_height_limitation") == DraftShieldHeightLimitation::LIMITED {
            let draft_shield_height = mesh_group_settings.get_coord("draft_shield_height");
            let layer_height_0 = mesh_group_settings.get_coord("layer_height_0");
            let layer_height = mesh_group_settings.get_coord("layer_height");
            let max_screen_layer = (draft_shield_height - layer_height_0) / layer_height + 1;
            if layer_nr > max_screen_layer {
                return;
            }
        }

        let config = gcode_layer.configs_storage.skirt_brim_config_per_extruder[0];
        gcode_layer.add_polygons_by_optimizer(&storage.draft_protection_shield, &config, mesh_group_settings, PolygonsOpts::default());
    }

    fn calculate_extruder_order_per_layer(&mut self, storage: &mut SliceDataStorage) {
        // set the initial extruder of this meshgroup
        let start_extruder = if scene().current_mesh_group == 0 {
            // first meshgroup
            self.get_start_extruder(storage)
        } else {
            self.gcode.get_extruder_nr()
        };
        let mut last_extruder = start_extruder;

        self.extruder_order_per_layer.init(true, storage.print_layer_count);

        let extruders_used = storage.get_extruders_used();
        let mut layer_nr = -(Raft::get_total_extra_layers() as LayerIndex);
        while layer_nr < storage.print_layer_count as LayerIndex {
            let extruder_order = self.get_used_extruders_on_layer(storage, last_extruder, layer_nr, &extruders_used);
            if let Some(last) = extruder_order.last() {
                last_extruder = last.extruder_nr;
            }
            self.extruder_order_per_layer.push(extruder_order);
            layer_nr += 1;
        }

        if let Some(mut prime_tower) = storage.prime_tower.take() {
            prime_tower.process_extruders_use(&mut self.extruder_order_per_layer, start_extruder);
            storage.prime_tower = Some(prime_tower);
        }
    }

    fn calculate_prime_layer_per_extruder(&mut self, storage: &SliceDataStorage) {
        let first_print_layer = -(Raft::get_total_extra_layers() as LayerIndex);
        for extruder_nr in 0..MAX_EXTRUDERS {
            if self.get_extruder_need_prime_blob_during_first_layer(storage, extruder_nr) {
                // Extruders requiring a prime blob have to be primed at first layer
                self.extruder_prime_layer_nr[extruder_nr] = self.extruder_prime_layer_nr[extruder_nr].min(first_print_layer);
            }
        }

        let mut layer_nr = first_print_layer;
        while layer_nr < storage.print_layer_count as LayerIndex {
            let used_extruders = storage.get_extruders_used_on_layer(layer_nr);
            for (extruder_nr, &used) in used_extruders.iter().enumerate() {
                if used {
                    self.extruder_prime_layer_nr[extruder_nr] = self.extruder_prime_layer_nr[extruder_nr].min(layer_nr);
                }
            }
            layer_nr += 1;
        }
    }

    fn get_used_extruders_on_layer(&self, storage: &SliceDataStorage, start_extruder: usize, layer_nr: LayerIndex, global_extruders_used: &[bool]) -> Vec<ExtruderUse> {
        let mesh_group_settings = mesh_group_settings();
        let extruder_count = global_extruders_used.len();
        let mut ret: Vec<ExtruderUse> = Vec::new();
        let mut extruder_is_used_on_this_layer = storage.get_extruders_used_on_layer(layer_nr);
        let raft_base_layer_nr = -(Raft::get_total_extra_layers() as LayerIndex);
        let layer_type = Raft::get_layer_type(layer_nr);

        if layer_type == RaftLayerType::RaftBase {
            // Raft base layers area treated apart because they don't have a proper prime tower
            let raft_base_extruder_nr = mesh_group_settings.get_extruder_train("raft_base_extruder_nr").extruder_nr;
            ret.push(ExtruderUse { extruder_nr: raft_base_extruder_nr, prime: ExtruderPrime::None });

            // check if we need prime blob on the first layer
            if layer_nr == raft_base_layer_nr {
                for extruder_nr in 0..extruder_is_used_on_this_layer.len() {
                    if extruder_nr != raft_base_extruder_nr && self.get_extruder_need_prime_blob_during_first_layer(storage, extruder_nr) {
                        ret.push(ExtruderUse { extruder_nr, prime: ExtruderPrime::None });
                    }
                }
            }

            return ret;
        }

        // check if we are on the first layer
        if layer_nr == raft_base_layer_nr {
            // check if we need prime blob on the first layer
            for used_idx in 0..extruder_is_used_on_this_layer.len() {
                if self.get_extruder_need_prime_blob_during_first_layer(storage, used_idx) {
                    extruder_is_used_on_this_layer[used_idx] = true;
                }
            }
        }

        // Make a temp list with the potential ordered extruders
        let mut ordered_extruders: Vec<usize> = vec![start_extruder];
        for extruder_nr in 0..extruder_count {
            if extruder_nr != start_extruder && global_extruders_used[extruder_nr] {
                ordered_extruders.push(extruder_nr);
            }
        }

        // Now check whether extruders should really be used, and how
        let mut last_extruder = start_extruder;
        for &extruder_nr in ordered_extruders.iter() {
            let mut prime = ExtruderPrime::None;

            if let Some(prime_tower) = storage.prime_tower.as_ref() {
                prime = prime_tower.get_extruder_prime(&extruder_is_used_on_this_layer, extruder_nr, last_extruder, storage, layer_nr);
            }

            if extruder_is_used_on_this_layer[extruder_nr] || prime != ExtruderPrime::None {
                ret.push(ExtruderUse { extruder_nr, prime });
                last_extruder = extruder_nr;
            }
        }

        debug_assert!(ret.len() <= extruder_count, "Not more extruders may be planned in a layer than there are extruders!");
        ret
    }

    fn calculate_mesh_order(&self, storage: &SliceDataStorage, extruder_nr: usize) -> Vec<usize> {
        // OrderOptimizer<size_t>
        let mut items: Vec<(Point2LL, usize)> = Vec::new();

        let mesh_group = scene().current_mesh_group();
        for mesh_idx in 0..storage.meshes.len() {
            let mesh = &storage.meshes[mesh_idx];
            if mesh.get_extruder_is_used(extruder_nr) {
                let mesh_data = &mesh_group.meshes[mesh_idx];
                let aabb = mesh_data.get_aabb();
                let middle = (aabb.min + aabb.max) / 2;
                items.push((pt(middle.x, middle.y), mesh_idx));
            }
        }
        let train = extruder(extruder_nr);
        let layer_start_position = pt(train.settings.get_coord("layer_start_x"), train.settings.get_coord("layer_start_y"));

        // OrderOptimizer::optimize
        let mut order: Vec<usize> = Vec::new();
        let mut item_idx_list: Vec<usize> = (0..items.len()).collect();
        let mut last_item_position = layer_start_position;
        while !item_idx_list.is_empty() {
            let mut shortest_distance = Coord::MAX;
            let mut shortest_distance_item_idx = usize::MAX;
            let mut idx_in_list = usize::MAX;
            for (idx, &item_idx) in item_idx_list.iter().enumerate() {
                let position = items[item_idx].0;
                let distance = v_size(position - last_item_position);
                if distance < shortest_distance {
                    shortest_distance = distance;
                    shortest_distance_item_idx = item_idx;
                    idx_in_list = idx;
                }
            }
            order.push(shortest_distance_item_idx);
            last_item_position = items[shortest_distance_item_idx].0;
            item_idx_list.remove(idx_in_list);
        }

        order.iter().map(|&i| items[i].1).collect()
    }

    fn add_mesh_layer_to_gcode_mesh_surface_mode(&self, mesh: &SliceMeshStorage, mesh_config: &MeshPathConfigs, gcode_layer: &mut LayerPlan) {
        if gcode_layer.get_layer_nr() > mesh.layer_nr_max_filled_layer {
            return;
        }

        let settings = mesh.settings();
        if settings.get_bool("anti_overhang_mesh") || settings.get_bool("support_mesh") {
            return;
        }

        let layer = &mesh.layers[gcode_layer.get_layer_nr() as usize];

        let mut polygons = Shape::default();
        for part in layer.parts.iter() {
            if !part.outline.is_empty() {
                polygons.push_all(&part.outline.0);
            }
        }

        polygons = Simplify::from_settings(settings).shape(&polygons);

        let z_seam_config = ZSeamConfig::new(settings.get_enum("z_seam_type"), mesh.get_z_seam_hint(), settings.get_enum("z_seam_corner"), settings.get_coord("wall_line_width_0") * 2);
        let spiralize = mesh_group_settings().get_bool("magic_spiralize");

        gcode_layer.add_polygons_by_optimizer_textured(
            &polygons,
            &mesh_config.inset0_config,
            settings,
            PolygonsOpts {
                z_seam_config,
                wall_0_wipe_dist: settings.get_coord("wall_0_wipe_dist"),
                spiralize,
                flow_ratio: 1.0,
                always_retract: false,
                reverse_order: false,
                start_near_location: None,
                scarf_seam: true,
                smooth_speed: true,
            },
            layer.texture_data_provider.clone(),
        );

        self.add_mesh_open_poly_lines_to_gcode(mesh, mesh_config, gcode_layer);
    }

    fn add_mesh_open_poly_lines_to_gcode(&self, mesh: &SliceMeshStorage, mesh_config: &MeshPathConfigs, gcode_layer: &mut LayerPlan) {
        let layer = &mesh.layers[gcode_layer.get_layer_nr() as usize];

        gcode_layer.add_lines_by_optimizer_open(&layer.open_polylines, &mesh_config.inset0_config, SpaceFillType::PolyLines, LinesOpts::default());
    }

    fn add_mesh_layer_to_gcode<'a>(
        &self,
        storage: &'a SliceDataStorage,
        mesh_idx: usize,
        extruder_nr: usize,
        mesh_config: &MeshPathConfigs,
        gcode_layer: &mut LayerPlan<'a>,
        layer_plan_buffer: &LayerPlanBuffer<'a>,
    ) {
        let mesh = &*storage.meshes[mesh_idx];
        if gcode_layer.get_layer_nr() > mesh.layer_nr_max_filled_layer {
            return;
        }

        let settings = mesh.settings();
        if settings.get_bool("anti_overhang_mesh") || settings.get_bool("support_mesh") {
            return;
        }

        let layer = &mesh.layers[gcode_layer.get_layer_nr() as usize];

        if layer.parts.is_empty() {
            return;
        }

        gcode_layer.set_mesh(Some(mesh_idx));

        let mut z_seam_config = ZSeamConfig::default();
        if mesh.is_printed() {
            //"normal" meshes with walls, skin, infill, etc. get the traditional part ordering based on the z-seam settings.
            z_seam_config = ZSeamConfig::new(settings.get_enum("z_seam_type"), mesh.get_z_seam_hint(), settings.get_enum("z_seam_corner"), settings.get_coord("wall_line_width_0") * 2);
        }
        let mut part_order_optimizer: PathOrderOptimizer<usize> =
            PathOrderOptimizer::simple(gcode_layer.get_last_planned_position_or_starting_position(), z_seam_config, false, None, false);
        for (part_idx, part) in layer.parts.iter().enumerate() {
            if part.outline.is_empty() {
                continue;
            }
            part_order_optimizer.add_polygon(part_idx, Arc::new(part.outline.outer_polygon().points.clone()), None, false);
        }

        part_order_optimizer.optimize(false);
        let ordered: Vec<usize> = part_order_optimizer.paths.iter().map(|p| p.vertices).collect();
        drop(part_order_optimizer);

        for part_idx in ordered {
            self.add_mesh_part_to_gcode(storage, mesh_idx, extruder_nr, mesh_config, part_idx, gcode_layer, layer_plan_buffer);
        }

        let extruder_identifier = if settings.get_usize("roofing_layer_count") > 0 { "roofing_extruder_nr" } else { "top_bottom_extruder_nr" };
        if extruder_nr == settings.get_extruder_train(extruder_identifier).extruder_nr {
            self.process_ironing(storage, mesh, layer, &mesh_config.ironing_config, gcode_layer);
        }
        if settings.get_enum::<ESurfaceMode>("magic_mesh_surface_mode") != ESurfaceMode::NORMAL && extruder_nr == settings.get_extruder_train("wall_0_extruder_nr").extruder_nr {
            self.add_mesh_open_poly_lines_to_gcode(mesh, mesh_config, gcode_layer);
        }
        gcode_layer.set_mesh(None);
    }

    #[allow(clippy::too_many_arguments)]
    fn add_mesh_part_to_gcode<'a>(
        &self,
        storage: &'a SliceDataStorage,
        mesh_idx: usize,
        extruder_nr: usize,
        mesh_config: &MeshPathConfigs,
        part_idx: usize,
        gcode_layer: &mut LayerPlan<'a>,
        layer_plan_buffer: &LayerPlanBuffer<'a>,
    ) {
        let mesh = &*storage.meshes[mesh_idx];
        let settings = mesh.settings();
        let part = &mesh.layers[gcode_layer.get_layer_nr() as usize].parts[part_idx];
        let mesh_group_settings = mesh_group_settings();

        // processInsets may modify the skin parts of the part (to register bridging areas); work on a copy of them.
        let mut skin_parts: Vec<SkinPart> = part.skin_parts.clone();

        let mut added_something = false;

        if settings.get_bool("infill_before_walls") {
            added_something |= self.process_infill(storage, gcode_layer, mesh, mesh_idx, extruder_nr, mesh_config, part, layer_plan_buffer);
        }

        added_something |= self.process_insets(storage, gcode_layer, mesh, extruder_nr, mesh_config, part, part_idx, &mut skin_parts);

        if !settings.get_bool("infill_before_walls") {
            added_something |= self.process_infill(storage, gcode_layer, mesh, mesh_idx, extruder_nr, mesh_config, part, layer_plan_buffer);
        }

        added_something |= self.process_skin(storage, gcode_layer, mesh, extruder_nr, mesh_config, &skin_parts);

        // After a layer part, make sure the nozzle is inside the comb boundary, so we do not retract on the perimeter.
        if added_something && (!mesh_group_settings.get_bool("magic_spiralize") || gcode_layer.get_layer_nr() < settings.get_usize("initial_bottom_layers") as LayerIndex) {
            let mut innermost_wall_line_width = settings.get_coord(if settings.get_usize("wall_line_count") > 1 { "wall_line_width_x" } else { "wall_line_width_0" });
            if gcode_layer.get_layer_nr() == 0 {
                innermost_wall_line_width = (innermost_wall_line_width as f64 * settings.get_ratio("initial_layer_line_width_factor")) as Coord;
            }
            gcode_layer.move_inside_comb_boundary(innermost_wall_line_width, Some(part), None);
        }

        gcode_layer.set_is_inside(false);
    }

    #[allow(clippy::too_many_arguments)]
    fn process_infill<'a>(
        &self,
        storage: &'a SliceDataStorage,
        gcode_layer: &mut LayerPlan<'a>,
        mesh: &SliceMeshStorage,
        mesh_idx: usize,
        extruder_nr: usize,
        mesh_config: &MeshPathConfigs,
        part: &SliceLayerPart,
        layer_plan_buffer: &LayerPlanBuffer<'a>,
    ) -> bool {
        if extruder_nr != mesh.settings().get_extruder_train("infill_extruder_nr").extruder_nr {
            return false;
        }

        let infill_start_move_inwards_length = mesh.settings().get_coord("infill_start_move_inwards_length");
        let infill_end_move_inwards_length = mesh.settings().get_coord("infill_end_move_inwards_length");

        let mut added_something =
            self.process_multi_layer_infill(gcode_layer, mesh, extruder_nr, mesh_config, part, infill_start_move_inwards_length, infill_end_move_inwards_length);
        added_something |= self.process_single_layer_infill(
            storage,
            gcode_layer,
            mesh,
            mesh_idx,
            extruder_nr,
            mesh_config,
            part,
            infill_start_move_inwards_length,
            infill_end_move_inwards_length,
            layer_plan_buffer,
        );
        added_something
    }

    fn infill_angle_for_layer(mesh: &SliceMeshStorage, layer_nr: LayerIndex) -> AngleDegrees {
        let mut infill_angle = AngleDegrees::new(45.0); // Original default. This will get updated to an element from mesh->infill_angles.
        if !mesh.infill_angles.is_empty() {
            let s = mesh.settings();
            let combined_infill_layers = 1u64.max(round_divide(s.get_coord("infill_sparse_thickness") as u64, max_c(s.get_coord("layer_height"), 1) as u64));
            let idx = ((layer_nr / combined_infill_layers as LayerIndex) as u64) % mesh.infill_angles.len() as u64;
            infill_angle = mesh.infill_angles[idx as usize];
        }
        infill_angle
    }

    fn lightning_layer(mesh: &SliceMeshStorage, layer_nr: LayerIndex) -> Option<&crate::infill::lightning_generator::LightningLayer> {
        mesh.lightning_generator.as_ref().map(|g| g.get_trees_for_layer(layer_nr as usize))
    }

    #[allow(clippy::too_many_arguments)]
    fn process_multi_layer_infill(
        &self,
        gcode_layer: &mut LayerPlan,
        mesh: &SliceMeshStorage,
        extruder_nr: usize,
        mesh_config: &MeshPathConfigs,
        part: &SliceLayerPart,
        start_move_inwards_length: Coord,
        end_move_inwards_length: Coord,
    ) -> bool {
        let settings = mesh.settings();
        if extruder_nr != settings.get_extruder_train("infill_extruder_nr").extruder_nr {
            return false;
        }
        let infill_line_distance = settings.get_coord("infill_line_distance");
        if infill_line_distance <= 0 {
            return false;
        }
        let max_resolution = settings.get_coord("meshfix_maximum_resolution");
        let max_deviation = settings.get_coord("meshfix_maximum_deviation");
        let infill_angle = Self::infill_angle_for_layer(mesh, gcode_layer.get_layer_nr());
        let mesh_middle = mesh.bounding_box.get_middle();
        let infill_origin = pt(mesh_middle.x + settings.get_coord("infill_offset_x"), mesh_middle.y + settings.get_coord("infill_offset_y"));

        // Print the thicker infill lines first. (double or more layer thickness, infill combined with previous layers)
        let mut added_something = false;
        for combine_idx in 1..part.infill_area_per_combine_per_density[0].len() {
            let infill_line_width = mesh_config.infill_config[combine_idx].get_line_width();
            let infill_pattern: EFillMethod = settings.get_enum("infill_pattern");
            let zig_zaggify_infill = settings.get_bool("zig_zaggify_infill") || infill_pattern == EFillMethod::ZIG_ZAG;
            let connect_polygons = settings.get_bool("connect_infill_polygons");
            let infill_multiplier = settings.get_usize("infill_multiplier");
            let mut infill_polygons = Shape::default();
            let mut infill_lines = OpenLinesSet::default();
            let mut infill_paths: Vec<VariableWidthLines> = part.infill_wall_toolpaths.clone();
            let mut infill_inner_contour = Shape::default();
            let mut density_idx = part.infill_area_per_combine_per_density.len().wrapping_sub(1);
            while density_idx as i32 >= 0 {
                // combine different density infill areas (for gradual infill)
                let density_factor: usize = 2 << density_idx; // == pow(2, density_idx + 1)
                let mut infill_line_distance_here = infill_line_distance * density_factor as Coord; // the highest density infill combines with the next to create a grid with density_factor 1
                let infill_shift = infill_line_distance_here / 2;
                if density_idx == part.infill_area_per_combine_per_density.len() - 1 || infill_pattern == EFillMethod::CROSS || infill_pattern == EFillMethod::CROSS_3D {
                    infill_line_distance_here /= 2;
                }

                let wall_line_count = 0; // wall toolpaths are when gradual infill areas are determined
                let small_area_width = 0;
                let infill_overlap = settings.get_coord("infill_overlap_mm");
                let skip_stitching = false;
                let connected_zigzags = false;
                let use_endpieces = true;
                let skip_some_zags = false;
                let zag_skip_count = 0;
                let fill_gaps = density_idx == 0; // Only fill gaps for the lowest density.

                let lightning_layer = Self::lightning_layer(mesh, gcode_layer.get_layer_nr());
                let mut infill_comp = make_infill(
                    infill_pattern,
                    zig_zaggify_infill,
                    connect_polygons,
                    part.infill_area_per_combine_per_density[density_idx][combine_idx].clone(),
                    infill_line_width,
                    infill_line_distance_here,
                    infill_overlap,
                    infill_multiplier,
                    infill_angle,
                    gcode_layer.z,
                    infill_shift,
                    max_resolution,
                    max_deviation,
                    wall_line_count,
                    small_area_width,
                    infill_origin,
                    skip_stitching,
                    fill_gaps,
                    connected_zigzags,
                    use_endpieces,
                    skip_some_zags,
                    zag_skip_count,
                    settings.get_coord("cross_infill_pocket_size"),
                );
                infill_comp.generate(
                    &mut infill_paths,
                    &mut infill_polygons,
                    &mut infill_lines,
                    settings,
                    gcode_layer.get_layer_nr() as i32,
                    SectionType::INFILL,
                    mesh.cross_fill_provider.as_deref(),
                    lightning_layer,
                    Some(mesh),
                    &Shape::default(),
                );
                if start_move_inwards_length > 0 || end_move_inwards_length > 0 {
                    infill_inner_contour = infill_inner_contour.union_polygons_other(infill_comp.get_inner_contour());
                }
                density_idx = density_idx.wrapping_sub(1);
            }
            if !infill_lines.is_empty() || !infill_polygons.is_empty() {
                added_something = true;
                gcode_layer.set_is_inside(true); // going to print stuff inside print object

                if !infill_polygons.is_empty() {
                    gcode_layer.add_infill_polygons_by_optimizer(
                        &infill_polygons,
                        &mut infill_lines,
                        &mesh_config.infill_config[combine_idx],
                        settings,
                        start_move_inwards_length > 0 || end_move_inwards_length > 0,
                        None,
                    );
                }

                if !infill_lines.is_empty() {
                    let mut near_start_location: Option<Point2LL> = None;
                    if settings.get_bool("infill_randomize_start_location") {
                        cppstd::crand::srand(gcode_layer.get_layer_nr() as u32);
                        near_start_location = Some(infill_lines[(cppstd::crand::rand() as usize) % infill_lines.len()][0]);
                    }

                    let enable_travel_optimization = settings.get_bool("infill_enable_travel_optimization");
                    let mended = if start_move_inwards_length > 0 || end_move_inwards_length > 0 {
                        Some(crate::geometry::mended_shape::MendedShape::new(settings, SectionType::INFILL, &infill_inner_contour))
                    } else {
                        None
                    };
                    gcode_layer.add_lines_by_optimizer_open(
                        &infill_lines,
                        &mesh_config.infill_config[combine_idx],
                        if zig_zaggify_infill { SpaceFillType::PolyLines } else { SpaceFillType::Lines },
                        LinesOpts {
                            enable_travel_optimization,
                            near_start_location,
                            extra_inwards_start_move_length: start_move_inwards_length,
                            extra_inwards_end_move_length: end_move_inwards_length,
                            extra_inwards_move_contour: mended.as_ref(),
                            ..Default::default()
                        },
                    );
                }
            }
        }
        added_something
    }

    #[allow(clippy::too_many_arguments)]
    fn process_single_layer_infill<'a>(
        &self,
        _storage: &'a SliceDataStorage,
        gcode_layer: &mut LayerPlan<'a>,
        mesh: &SliceMeshStorage,
        mesh_idx: usize,
        extruder_nr: usize,
        mesh_config: &MeshPathConfigs,
        part: &SliceLayerPart,
        start_move_inwards_length: Coord,
        end_move_inwards_length: Coord,
        layer_plan_buffer: &LayerPlanBuffer<'a>,
    ) -> bool {
        let settings = mesh.settings();
        if extruder_nr != settings.get_extruder_train("infill_extruder_nr").extruder_nr {
            return false;
        }
        let infill_line_distance = settings.get_coord("infill_line_distance");
        if infill_line_distance == 0 || part.infill_area_per_combine_per_density[0].is_empty() {
            return false;
        }
        let mut added_something = false;
        let infill_line_width = mesh_config.infill_config[0].get_line_width();

        // Combine the 1 layer thick infill with the top/bottom skin and print that as one thing.
        let mut infill_polygons = Shape::default();
        let mut wall_tool_paths: Vec<Vec<VariableWidthLines>> = Vec::new(); // All wall toolpaths binned by inset_idx (inner) and by density_idx (outer)
        let mut infill_lines = OpenLinesSet::default();
        let mut skin_support_lines = OpenLinesSet::default();
        let mut skin_support_polygons = Shape::default();

        let pattern: EFillMethod = settings.get_enum("infill_pattern");
        let zig_zaggify_infill = settings.get_bool("zig_zaggify_infill") || pattern == EFillMethod::ZIG_ZAG;
        let connect_polygons = settings.get_bool("connect_infill_polygons");
        let infill_overlap = settings.get_coord("infill_overlap_mm");
        let infill_multiplier = settings.get_usize("infill_multiplier");
        let wall_line_count = settings.get_usize("infill_wall_line_count");
        let last_idx = part.infill_area_per_combine_per_density.len() - 1;
        let max_resolution = settings.get_coord("meshfix_maximum_resolution");
        let max_deviation = settings.get_coord("meshfix_maximum_deviation");
        let skin_support_density = settings.get_ratio("skin_support_density");
        let skin_support_line_distance: Coord = if skin_support_density > 0.0 { (infill_line_width as f64 / skin_support_density) as Coord } else { 0 };
        let infill_angle = Self::infill_angle_for_layer(mesh, gcode_layer.get_layer_nr());
        let mesh_middle = mesh.bounding_box.get_middle();
        let infill_origin = pt(mesh_middle.x + settings.get_coord("infill_offset_x"), mesh_middle.y + settings.get_coord("infill_offset_y"));

        let get_cut_offset = |zig_zaggify: bool, line_width: Coord, line_count: usize| -> Coord {
            if zig_zaggify {
                return -line_width / 2 - line_count as Coord * line_width - 5;
            }
            -(line_count as Coord) * line_width
        };

        let mut sparse_in_outline = part.infill_area_per_combine_per_density[last_idx][0].clone();

        // if infill walls are required below the boundaries of skin regions above, partition the infill along the
        // boundary edge
        let mut infill_below_skin = Shape::default();
        let mut infill_not_below_skin = Shape::default();
        let mut skin_support_angle = AngleDegrees::default();
        if gcode_layer.get_layer_nr() > 0 {
            Self::partition_infill_by_skin_above(&mut infill_below_skin, &mut infill_not_below_skin, gcode_layer, mesh, part, infill_line_width);
        }

        if !infill_below_skin.is_empty() {
            let infill_wall_line_count = settings.get_usize("infill_wall_line_count") as Coord;
            let infill_wall_offset = -infill_wall_line_count * infill_line_width;
            let infill_contour = part.infill_area.offset_default(-(infill_line_width / 2) + infill_overlap + infill_wall_offset);
            let completed_layer_below =
                layer_plan_buffer.get_completed_layer_plan(gcode_layer.get_layer_nr() - 1).expect("the layer plan below must be in the layer plan buffer");
            let (below, angle) =
                make_bridge_over_infill_printable(&infill_contour, &infill_below_skin, mesh, completed_layer_below, mesh_idx, gcode_layer.get_layer_nr() as u32);
            infill_below_skin = below;
            skin_support_angle = angle;
            infill_not_below_skin = infill_not_below_skin.difference(&infill_below_skin);
        }

        let pocket_size = settings.get_coord("cross_infill_pocket_size");
        let skip_stitching = false;
        let connected_zigzags = false;
        let use_endpieces = part.infill_area_per_combine_per_density.len() == 1; // Only use endpieces when not using gradual infill, since they will then overlap.
        let skip_some_zags = false;
        let zag_skip_count = 0;
        let mut infill_inner_contour = Shape::default();

        let mut density_idx = last_idx;
        while density_idx as i32 >= 0 {
            // Only process dense areas when they're initialized
            if part.infill_area_per_combine_per_density[density_idx][0].is_empty() {
                density_idx = density_idx.wrapping_sub(1);
                continue;
            }

            let mut infill_lines_here = OpenLinesSet::default();
            let infill_polygons_here = Shape::default();

            // the highest density infill combines with the next to create a grid with density_factor 1
            let mut infill_line_distance_here: i32 = (infill_line_distance << (density_idx + 1)) as i32;
            let infill_shift: i32 = infill_line_distance_here / 2;

            // All of that doesn't hold for the Cross patterns; they should just always be multiplied by 2.
            if density_idx == part.infill_area_per_combine_per_density.len() - 1 || pattern == EFillMethod::CROSS || pattern == EFillMethod::CROSS_3D {
                infill_line_distance_here /= 2;
            }

            let mut in_outline = part.infill_area_per_combine_per_density[density_idx][0].clone();

            let lightning_layer = Self::lightning_layer(mesh, gcode_layer.get_layer_nr());

            let fill_gaps = density_idx == 0; // Only fill gaps in the lowest infill density pattern.
            if !infill_below_skin.is_empty() {
                // infill region with skin above has to be printed bridge-like
                let skin_support_pattern = EFillMethod::LINES;
                let skin_support_zig_zaggify = false;
                let skin_support_connect_polygons = false;
                let skin_support_infill_multiplier = 1;
                let min_skin_support_wall_count = 0;
                let skin_support_wall_count = 0;
                let small_area_width = 0;
                wall_tool_paths.push(Vec::new());
                let skin_support_overlap = 0;
                let skin_support_infill_shift = 0;
                let mut infill_comp = make_infill(
                    skin_support_pattern,
                    skin_support_zig_zaggify,
                    skin_support_connect_polygons,
                    infill_below_skin.clone(),
                    infill_line_width,
                    skin_support_line_distance,
                    skin_support_overlap,
                    skin_support_infill_multiplier,
                    skin_support_angle,
                    gcode_layer.z,
                    skin_support_infill_shift,
                    max_resolution,
                    max_deviation,
                    skin_support_wall_count,
                    small_area_width,
                    infill_origin,
                    skip_stitching,
                    fill_gaps,
                    connected_zigzags,
                    use_endpieces,
                    skip_some_zags,
                    zag_skip_count,
                    pocket_size,
                );
                infill_comp.generate(
                    wall_tool_paths.last_mut().unwrap(),
                    &mut skin_support_polygons,
                    &mut skin_support_lines,
                    settings,
                    gcode_layer.get_layer_nr() as i32,
                    SectionType::INFILL,
                    mesh.cross_fill_provider.as_deref(),
                    lightning_layer,
                    Some(mesh),
                    &Shape::default(),
                );
                if density_idx < last_idx {
                    let cut_offset = get_cut_offset(skin_support_zig_zaggify, infill_line_width, min_skin_support_wall_count);
                    let tool = infill_below_skin.offset_default(cut_offset as i32 as Coord);
                    infill_lines_here = tool.intersection_polylines(&infill_lines_here, true, 10, true);
                }
                skin_support_lines.push_all(&infill_lines_here);

                // normal processing for the infill that isn't below skin
                in_outline = infill_not_below_skin.clone();
                if density_idx == last_idx {
                    sparse_in_outline = infill_not_below_skin.clone();
                }
            }

            let circumference = in_outline.length();
            // Originally an area of 0.4*0.4*2 (2 line width squares) was found to be a good threshold for removal.
            // However we found that this doesn't scale well with polygons with larger circumference (https://github.com/Ultimaker/Cura/issues/3992).
            // Given that the original test worked for approximately 2x2cm models, this scaling by circumference should make it work for any size.
            const MINIMUM_SMALL_AREA_FACTOR: f64 = 0.4 * 0.4 / 40000.0;
            let minimum_small_area = MINIMUM_SMALL_AREA_FACTOR * circumference as f64;

            // This is only for density infill, because after generating the infill might appear unnecessary infill on walls
            // especially on vertical surfaces
            in_outline.remove_small_areas(minimum_small_area, false);

            let wall_line_count_here = 0; // Wall toolpaths were generated in generateGradualInfill for the sparsest density, denser parts don't have walls by default
            let small_area_width = 0;

            wall_tool_paths.push(Vec::new());
            let mut infill_comp = make_infill(
                pattern,
                zig_zaggify_infill,
                connect_polygons,
                in_outline,
                infill_line_width,
                infill_line_distance_here as Coord,
                infill_overlap,
                infill_multiplier,
                infill_angle,
                gcode_layer.z,
                infill_shift as Coord,
                max_resolution,
                max_deviation,
                wall_line_count_here,
                small_area_width,
                infill_origin,
                skip_stitching,
                fill_gaps,
                connected_zigzags,
                use_endpieces,
                skip_some_zags,
                zag_skip_count,
                pocket_size,
            );
            infill_comp.generate(
                wall_tool_paths.last_mut().unwrap(),
                &mut infill_polygons,
                &mut infill_lines,
                settings,
                gcode_layer.get_layer_nr() as i32,
                SectionType::INFILL,
                mesh.cross_fill_provider.as_deref(),
                lightning_layer,
                Some(mesh),
                &Shape::default(),
            );
            if density_idx < last_idx {
                let cut_offset = get_cut_offset(zig_zaggify_infill, infill_line_width, wall_line_count);
                let tool = sparse_in_outline.offset_default(cut_offset as i32 as Coord);
                infill_lines_here = tool.intersection_polylines(&infill_lines_here, true, 10, true);
            }
            infill_lines.push_all(&infill_lines_here);
            infill_polygons.push_all(&infill_polygons_here);

            if start_move_inwards_length > 0 || end_move_inwards_length > 0 {
                infill_inner_contour = infill_inner_contour.union_polygons_other(infill_comp.get_inner_contour());
            }
            density_idx = density_idx.wrapping_sub(1);
        }

        wall_tool_paths.push(part.infill_wall_toolpaths.clone()); // The extra infill walls were generated separately. Add these too.

        let walls_generated = wall_tool_paths.iter().any(|tp| !(tp.is_empty() || tp.iter().all(|vwl| vwl.is_empty())));
        if !infill_lines.is_empty() || !infill_polygons.is_empty() || !skin_support_lines.is_empty() || !skin_support_polygons.is_empty() || walls_generated {
            added_something = true;
            gcode_layer.set_is_inside(true); // going to print stuff inside print object
            let mut near_start_location: Option<Point2LL> = None;
            if settings.get_bool("infill_randomize_start_location") {
                cppstd::crand::srand(gcode_layer.get_layer_nr() as u32);
                if !infill_lines.is_empty() {
                    near_start_location = Some(infill_lines[(cppstd::crand::rand() as usize) % infill_lines.len()][0]);
                } else if !infill_polygons.is_empty() {
                    let start_poly = &infill_polygons[(cppstd::crand::rand() as usize) % infill_polygons.len()];
                    near_start_location = Some(start_poly[(cppstd::crand::rand() as usize) % start_poly.len()]);
                } else {
                    // So walls_generated must be true.
                    let mut start_paths = &wall_tool_paths[(cppstd::crand::rand() as usize) % wall_tool_paths.len()];
                    while start_paths.is_empty() || start_paths[0].is_empty() {
                        // We know for sure (because walls_generated) that one of them is not empty. So randomise until we hit it.
                        start_paths = &wall_tool_paths[(cppstd::crand::rand() as usize) % wall_tool_paths.len()];
                    }
                    near_start_location = Some(start_paths[0][0].junctions[0].p);
                }
            }
            if walls_generated {
                for tool_paths in wall_tool_paths.iter() {
                    let retract_before_outer_wall = false;
                    let wipe_dist = 0;
                    let z_seam_config = ZSeamConfig::new(
                        settings.get_enum("z_seam_type"),
                        mesh.get_z_seam_hint(),
                        settings.get_enum("z_seam_corner"),
                        mesh_config.infill_config[0].get_line_width() * 2,
                    );
                    let mut wall_orderer = InsetOrderOptimizer::new(
                        gcode_layer.get_layer_nr(),
                        settings,
                        extruder_nr,
                        WallConfigs::uniform(&mesh_config.infill_config[0]),
                        retract_before_outer_wall,
                        wipe_dist,
                        wipe_dist,
                        extruder_nr,
                        extruder_nr,
                        z_seam_config,
                        tool_paths,
                        mesh.bounding_box.flatten().get_middle(),
                        Shape::default(),
                        false,
                        false,
                        Shape::default(),
                    );
                    added_something |= wall_orderer.add_to_layer(gcode_layer);
                }
            }
            if !infill_polygons.is_empty() {
                gcode_layer.add_infill_polygons_by_optimizer(
                    &infill_polygons,
                    &mut infill_lines,
                    &mesh_config.infill_config[0],
                    settings,
                    start_move_inwards_length > 0 || end_move_inwards_length > 0,
                    near_start_location,
                );
            }

            let (space_fill_type, wipe_dist) = if matches!(
                pattern,
                EFillMethod::GRID
                    | EFillMethod::LINES
                    | EFillMethod::TRIANGLES
                    | EFillMethod::CUBIC
                    | EFillMethod::TETRAHEDRAL
                    | EFillMethod::QUARTER_CUBIC
                    | EFillMethod::CUBICSUBDIV
                    | EFillMethod::LIGHTNING
            ) {
                (SpaceFillType::Lines, settings.get_coord("infill_wipe_dist"))
            } else {
                (if pattern == EFillMethod::ZIG_ZAG { SpaceFillType::PolyLines } else { SpaceFillType::Lines }, 0)
            };

            let enable_travel_optimization = settings.get_bool("infill_enable_travel_optimization");

            let mended = if start_move_inwards_length > 0 || end_move_inwards_length > 0 {
                Some(crate::geometry::mended_shape::MendedShape::new(mesh.settings(), SectionType::INFILL, &infill_inner_contour))
            } else {
                None
            };
            gcode_layer.add_lines_by_optimizer_open(
                &infill_lines,
                &mesh_config.infill_config[0],
                space_fill_type,
                LinesOpts {
                    enable_travel_optimization,
                    wipe_dist,
                    near_start_location,
                    extra_inwards_start_move_length: start_move_inwards_length,
                    extra_inwards_end_move_length: end_move_inwards_length,
                    extra_inwards_move_contour: mended.as_ref(),
                    ..Default::default()
                },
            );

            if !skin_support_polygons.is_empty() {
                let extra_inwards_move = false;
                gcode_layer.add_infill_polygons_by_optimizer(
                    &skin_support_polygons,
                    &mut skin_support_lines,
                    &mesh_config.skin_support_config,
                    settings,
                    extra_inwards_move,
                    near_start_location,
                );
            }

            let skin_support_fan_speed = settings.get_f64("skin_support_fan_speed");
            let skin_support_space_fill_type = SpaceFillType::Lines;
            let skin_support_wipe_dist = 0;
            let skin_support_interlace_lines = settings.get_bool("skin_support_interlace_lines");
            if skin_support_interlace_lines {
                let max_adjacent_distance = (skin_support_line_distance as f64 * 1.1) as Coord;
                let exclude_distance = 0;
                let skin_support_interlaced = true;
                gcode_layer.add_lines_monotonic(
                    &infill_below_skin,
                    &skin_support_lines,
                    &mesh_config.skin_support_config,
                    skin_support_space_fill_type,
                    AngleRadians::from(skin_support_angle),
                    max_adjacent_distance,
                    exclude_distance,
                    skin_support_wipe_dist,
                    1.0,
                    skin_support_fan_speed,
                    skin_support_interlaced,
                );
            } else {
                gcode_layer.add_lines_by_optimizer_open(
                    &skin_support_lines,
                    &mesh_config.skin_support_config,
                    skin_support_space_fill_type,
                    LinesOpts { enable_travel_optimization, wipe_dist: skin_support_wipe_dist, near_start_location, fan_speed: skin_support_fan_speed, ..Default::default() },
                );
            }

            let mut all_infill_lines = MixedLinesSet::default();
            all_infill_lines.push_open_set(infill_lines);
            all_infill_lines.push_shape(infill_polygons);
            gcode_layer.set_generated_infill_lines(mesh_idx, &all_infill_lines);
        }

        added_something
    }

    fn partition_infill_by_skin_above(
        infill_below_skin: &mut Shape,
        infill_not_below_skin: &mut Shape,
        gcode_layer: &LayerPlan,
        mesh: &SliceMeshStorage,
        part: &SliceLayerPart,
        infill_line_width: Coord,
    ) {
        const TINY_INFILL_OFFSET: Coord = 20;
        let skin_support = mesh.settings().get_bool("skin_support");

        if !skin_support {
            return;
        }

        let mut skin_above_combined = Shape::default(); // skin regions on the layers above combined with small gaps between

        let skin_layer_nr = (gcode_layer.get_layer_nr() + 1) as usize;
        if skin_layer_nr < mesh.layers.len() {
            for part_i in mesh.layers[skin_layer_nr].parts.iter() {
                for skin_part in part_i.skin_parts.iter() {
                    // Limit considered areas to the ones that should have infill underneath at the current layer.
                    let relevant_outline = skin_part.outline.0.intersection(part.get_own_infill_area());

                    if !skin_above_combined.is_empty() {
                        // does this skin part overlap with any of the skin parts on the layers above?
                        let overlap = skin_above_combined.intersection(&relevant_outline);
                        if !overlap.is_empty() {
                            // yes, it overlaps, need to leave a gap between this skin part and the others
                            skin_above_combined = skin_above_combined.difference(&relevant_outline.offset_default(TINY_INFILL_OFFSET));
                            skin_above_combined.push_all(&relevant_outline);
                        } else {
                            // no overlap
                            skin_above_combined.push_all(&relevant_outline);
                        }
                    } else {
                        // this is the first skin region we have looked at
                        skin_above_combined.push_all(&relevant_outline);
                    }
                }
            }
        }

        // the shrink/expand here is to remove regions of infill below skin that are narrower than the width of the infill walls otherwise the infill walls could merge and form
        // a bump
        let sparse = &part.infill_area_per_combine_per_density.last().unwrap()[0];
        *infill_below_skin = skin_above_combined.intersection(sparse).offset_default(-infill_line_width).offset_default(infill_line_width);

        let remove_small_holes_from_infill_below_skin = true;
        let min_area_multiplier = 25.0;
        let min_area = int2mm(infill_line_width) * int2mm(infill_line_width) * min_area_multiplier;
        infill_below_skin.remove_small_areas(min_area, remove_small_holes_from_infill_below_skin);

        // there is infill below skin, is there also infill that isn't below skin?
        *infill_not_below_skin = sparse.difference(infill_below_skin);
        infill_not_below_skin.remove_small_areas(min_area, false);
    }

    fn find_used_extruder_index(&self, storage: &SliceDataStorage, layer_nr: LayerIndex, last: bool) -> usize {
        let extruder_use = self.extruder_order_per_layer.get(layer_nr);

        if !extruder_use.is_empty() {
            if last {
                extruder_use.last().unwrap().extruder_nr
            } else {
                extruder_use[0].extruder_nr
            }
        } else if layer_nr <= -(Raft::get_total_extra_layers() as LayerIndex) {
            // Asking for extruder use below first layer, give first extruder
            self.get_start_extruder(storage)
        } else {
            // Asking for extruder on an empty layer, get the last one from layer below
            self.find_used_extruder_index(storage, layer_nr - 1, true)
        }
    }

    fn process_spiralized_wall(&self, storage: &SliceDataStorage, gcode_layer: &mut LayerPlan, mesh_config: &MeshPathConfigs, part: &SliceLayerPart, mesh: &SliceMeshStorage) {
        if part.spiral_wall.is_empty() {
            // wall doesn't have usable outline
            return;
        }
        let mut last_wall_outline = &part.spiral_wall[0]; // default to current wall outline
        let mut last_seam_vertex_idx: i32 = -1; // last layer seam vertex index
        let layer_nr = gcode_layer.get_layer_nr() as i32;
        if layer_nr > 0 {
            let prev = storage.spiralize_wall_outlines[(layer_nr - 1) as usize];
            if !prev.is_null() {
                // use the wall outline from the previous layer
                last_wall_outline = unsafe { &(&*prev)[0] };
                // and the seam vertex index pre-computed for that layer
                last_seam_vertex_idx = storage.spiralize_seam_vertex_indices[(layer_nr - 1) as usize];
            }
        }
        let is_bottom_layer = layer_nr as LayerIndex == mesh.settings().get_layer_index("initial_bottom_layers");
        let is_top_layer = layer_nr as usize == storage.spiralize_wall_outlines.len() - 1 || storage.spiralize_wall_outlines[(layer_nr + 1) as usize].is_null();
        let seam_vertex_idx = storage.spiralize_seam_vertex_indices[layer_nr as usize]; // use pre-computed seam vertex index for current layer
        // output a wall slice that is interpolated between the last and current walls
        for wall_outline in part.spiral_wall.iter() {
            gcode_layer.spiralize_wall_slice(&mesh_config.inset0_config, wall_outline, last_wall_outline, seam_vertex_idx, last_seam_vertex_idx, is_top_layer, is_bottom_layer);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn process_insets(
        &self,
        storage: &SliceDataStorage,
        gcode_layer: &mut LayerPlan,
        mesh: &SliceMeshStorage,
        extruder_nr: usize,
        mesh_config: &MeshPathConfigs,
        part: &SliceLayerPart,
        part_idx: usize,
        skin_parts: &mut [SkinPart],
    ) -> bool {
        let settings = mesh.settings();
        let mut added_something = false;
        if extruder_nr != settings.get_extruder_train("wall_0_extruder_nr").extruder_nr && extruder_nr != settings.get_extruder_train("wall_x_extruder_nr").extruder_nr {
            return added_something;
        }
        if settings.get_usize("wall_line_count") == 0 {
            return added_something;
        }

        let mut spiralize = false;
        if mesh_group_settings().get_bool("magic_spiralize") {
            let initial_bottom_layers = settings.get_usize("initial_bottom_layers") as LayerIndex;
            let layer_nr = gcode_layer.get_layer_nr();
            if (layer_nr < initial_bottom_layers && part.wall_toolpaths.is_empty()) // The bottom layers in spiralize mode are generated using the variable width paths
                || (layer_nr >= initial_bottom_layers && part.spiral_wall.is_empty())
            // The rest of the layers in spiralize mode are using the spiral wall
            {
                // nothing to do
                return false;
            }
            if gcode_layer.get_layer_nr() >= initial_bottom_layers {
                spiralize = true;
            }
            if spiralize && gcode_layer.get_layer_nr() == initial_bottom_layers && extruder_nr == settings.get_extruder_train("wall_0_extruder_nr").extruder_nr {
                // on the last normal layer first make the outer wall normally and then start a second outer wall from the same hight, but gradually moving upward
                added_something = true;
                gcode_layer.set_is_inside(true); // going to print stuff inside print object
                // start this first wall at the same vertex the spiral starts
                let spiral_inset = &part.spiral_wall[0];
                let spiral_start_vertex = storage.spiralize_seam_vertex_indices[initial_bottom_layers as usize] as usize;
                if spiral_start_vertex < spiral_inset.len() {
                    gcode_layer.add_travel(spiral_inset[spiral_start_vertex], false, 0);
                }
                let wall_0_wipe_dist = 0;
                gcode_layer.add_polygons_by_optimizer(&part.spiral_wall, &mesh_config.inset0_config, settings, PolygonsOpts { wall_0_wipe_dist, ..Default::default() });
            }
        }
        // for non-spiralized layers, determine the shape of the unsupported areas below this part
        if !spiralize && gcode_layer.get_layer_nr() > 0 {
            // accumulate the outlines of all of the parts that are on the layer below

            let mut outlines_below = Shape::default();
            let boundary_box = AABB::from_shape(&part.outline);
            for m in storage.meshes.iter() {
                if m.is_printed() {
                    for prev_layer_part in m.layers[(gcode_layer.get_layer_nr() - 1) as usize].parts.iter() {
                        if boundary_box.hit(&prev_layer_part.boundary_box) {
                            outlines_below.push_all(&prev_layer_part.outline.0);
                        }
                    }
                }
            }

            let layer_height = mesh_config.inset0_config.get_layer_thickness();

            // if support is enabled, add the support outlines also so we don't generate bridges over support

            if mesh_group_settings().get_bool("support_enable") {
                let z_distance_top = settings.get_coord("support_top_distance");
                let z_distance_top_layers = (z_distance_top / layer_height) as usize + 1;
                let support_layer_nr = (gcode_layer.get_layer_nr() - z_distance_top_layers as LayerIndex) as i32;

                if support_layer_nr > 0 {
                    let support_layer = &storage.support.support_layers[support_layer_nr as usize];

                    if !support_layer.support_roof.is_empty() {
                        let support_roof_bb = AABB::from_shape(&support_layer.support_roof);
                        if boundary_box.hit(&support_roof_bb) {
                            outlines_below.push_all(&support_layer.support_roof);
                        }
                    } else {
                        for support_part in support_layer.support_infill_parts.iter() {
                            let support_part_bb = AABB::from_shape(support_part.get_infill_area());
                            if boundary_box.hit(&support_part_bb) {
                                outlines_below.push_all(support_part.get_infill_area());
                            }
                        }
                    }
                }
            }

            let half_outer_wall_width = (mesh_config.inset0_config.get_line_width() / 2) as i32 as Coord;

            // remove those parts of the layer below that are narrower than a wall line width as they will not be printed

            outlines_below = outlines_below.offset_default(-half_outer_wall_width).offset_default(half_outer_wall_width);

            if settings.get_bool("bridge_settings_enabled") {
                // max_air_gap is the max allowed width of the unsupported region below the wall line
                // if the unsupported region is wider than max_air_gap, the wall line will be printed using bridge settings

                let max_air_gap = half_outer_wall_width;

                // subtract the outlines of the parts below this part to give the shapes of the unsupported regions and then
                // shrink those shapes so that any that are narrower than two times max_air_gap will be removed

                let compressed_air = part.outline.0.difference(&outlines_below).offset_default(-max_air_gap);

                // now expand the air regions by the same amount as they were shrunk (completing the morphological opening operation)
                // also, if the bridge-flow is light enough, compensate for the fact that the wall-vertices aren't exactly on the outline

                let compensate_outline_distance = if mesh_config.bridge_inset0_config.flow < 1.0 { half_outer_wall_width } else { 0 };
                let bridge_mask = compressed_air.offset_default(max_air_gap + compensate_outline_distance);
                gcode_layer.set_bridge_wall_mask(&bridge_mask);

                let skin_overlap = settings.get_coord("skin_overlap_mm");

                // Override flooring/skin areas to register bridging areas to be treated as normal skin
                for skin_part in skin_parts.iter_mut() {
                    let moved_area = skin_part.flooring_fill.intersection(&bridge_mask).offset_default(10);
                    skin_part.skin_fill = skin_part.skin_fill.union_polygons_other(&moved_area);

                    // make sure that
                    // - skin_fill (bridging) and flooring/roofing areas are distinct areas
                    // - skin overlap is reapplied on roofing and flooring areas
                    // - skin doesn't grow beyond its original area
                    skin_part.flooring_fill = skin_part
                        .flooring_fill
                        .difference(&skin_part.skin_fill)
                        .offset_default(-10)
                        .offset_default(skin_overlap + 10)
                        .intersection(&skin_part.flooring_fill);
                    skin_part.roofing_fill =
                        skin_part.roofing_fill.difference(&skin_part.skin_fill).offset_default(-10).offset_default(skin_overlap + 10).intersection(&skin_part.roofing_fill);
                }
            } else {
                // clear to disable use of bridging settings
                gcode_layer.set_bridge_wall_mask(&Shape::default());
            }

            let fully_supported_region = outlines_below.offset_default(-half_outer_wall_width);
            let part_print_region = part.outline.offset_default(-half_outer_wall_width);

            let get_supported_region = |overhang_angle: AngleDegrees| -> Shape {
                // the overhang mask is set to the area of the current part's outline minus the region that is considered to be supported
                if overhang_angle.value() < 90.0 {
                    let overhang_width = (layer_height as f64 * AngleRadians::from(overhang_angle).value().tan()) as Coord;
                    return fully_supported_region.offset_default(overhang_width + 10);
                }

                Shape::default()
            };

            // Build supported regions for all the overhang speeds. For a visual explanation of the result, see doc/gradual_overhang_speed.svg
            let mut overhang_masks: Vec<OverhangMask> = Vec::new();
            let overhang_speed_factors = settings.get_vec_ratio("wall_overhang_speed_factors");
            let overhang_angles_count = overhang_speed_factors.len();
            let wall_overhang_angle = settings.get_angle_degrees("wall_overhang_angle");
            if overhang_angles_count > 0 && wall_overhang_angle.value() < 90.0 {
                #[derive(Clone, Copy)]
                struct SpeedRegion {
                    overhang_angle: AngleDegrees,
                    speed_factor: Ratio,
                    chunk: bool,
                }

                // Create raw speed regions
                let overhang_step = AngleDegrees::new((90.0 - wall_overhang_angle.value()) / overhang_angles_count as f64);
                let mut speed_regions: Vec<SpeedRegion> = Vec::with_capacity(overhang_angles_count + 2);

                let dont_chunk_first = false; // Never merge internal region in order to detect actual overhanging
                speed_regions.push(SpeedRegion { overhang_angle: wall_overhang_angle, speed_factor: 1.0, chunk: dont_chunk_first }); // Initial internal region, always 100% speed factor

                for angle_index in 1..=overhang_angles_count {
                    let actual_wall_overhang_angle = wall_overhang_angle + AngleDegrees::new(angle_index as f64 * overhang_step.value());
                    let speed_factor = overhang_speed_factors[angle_index - 1];

                    speed_regions.push(SpeedRegion { overhang_angle: actual_wall_overhang_angle, speed_factor, chunk: true });
                }

                speed_regions.push(SpeedRegion { overhang_angle: AngleDegrees::new(90.0), speed_factor: *overhang_speed_factors.last().unwrap(), chunk: true }); // Final "everything else" speed region

                // Now merge regions that have similar speed factors (saves calculations and avoid generating micro-segments)
                let mut merged_regions: Vec<Vec<SpeedRegion>> = Vec::new();
                for region in speed_regions.iter() {
                    match merged_regions.last_mut() {
                        Some(chunk) => {
                            let a = *chunk.last().unwrap();
                            if a.chunk && region.chunk && a.speed_factor == region.speed_factor {
                                chunk.push(*region);
                            } else {
                                merged_regions.push(vec![*region]);
                            }
                        }
                        None => merged_regions.push(vec![*region]),
                    }
                }

                // If finally necessary, add actual calculated speed regions
                if merged_regions.len() > 1 {
                    for regions in merged_regions.iter() {
                        let last_region = regions.last().unwrap();
                        overhang_masks.push(OverhangMask { supported_region: get_supported_region(last_region.overhang_angle), speed_ratio: last_region.speed_factor });
                    }
                }
            }
            gcode_layer.set_overhang_masks(&overhang_masks);

            // the seam overhang mask is set to the area of the current part's outline minus the region that is considered to be supported,
            // which will then be empty if everything is considered supported i.r.t. the angle
            let seam_overhang_angle = settings.get_angle_degrees("seam_overhang_angle");
            if seam_overhang_angle.value() < 90.0 {
                let supported_region_seam = get_supported_region(seam_overhang_angle);
                gcode_layer.set_seam_overhang_mask(&part_print_region.difference(&supported_region_seam).offset_default(10));
            } else {
                gcode_layer.set_seam_overhang_mask(&Shape::default());
            }

            let wall_line_width_0 = settings.get_coord("wall_line_width_0");

            let roofing_mask = {
                let roofing_layer_count = settings.get_usize("roofing_layer_count").min(settings.get_usize("top_layers"));

                let mut roofing_mask = storage.get_machine_border(settings.get_extruder_train("wall_0_extruder_nr").extruder_nr as i32);

                if gcode_layer.get_layer_nr() + roofing_layer_count as LayerIndex >= mesh.layers.len() as LayerIndex {
                    roofing_mask
                } else {
                    for layer_part in mesh.layers[(gcode_layer.get_layer_nr() + roofing_layer_count as LayerIndex) as usize].parts.iter() {
                        if boundary_box.hit(&layer_part.boundary_box) {
                            roofing_mask = roofing_mask.difference(&layer_part.outline.offset_default(-wall_line_width_0 / 4));
                        }
                    }
                    roofing_mask
                }
            };

            gcode_layer.set_roofing_mask(&roofing_mask);

            let flooring_mask = {
                let flooring_layer_count = settings.get_usize("flooring_layer_count").min(settings.get_usize("bottom_layers"));

                let mut flooring_mask = storage.get_machine_border(settings.get_extruder_train("wall_0_extruder_nr").extruder_nr as i32);

                if gcode_layer.get_layer_nr() - (flooring_layer_count as LayerIndex) < 0 {
                    flooring_mask
                } else {
                    for layer_part in mesh.layers[(gcode_layer.get_layer_nr() - flooring_layer_count as LayerIndex) as usize].parts.iter() {
                        if boundary_box.hit(&layer_part.boundary_box) {
                            flooring_mask = flooring_mask.difference(&layer_part.outline.offset_default(-wall_line_width_0 / 4));
                        }
                    }
                    flooring_mask
                }
            };

            gcode_layer.set_flooring_mask(&flooring_mask);
        } else {
            // clear to disable use of bridging settings
            gcode_layer.set_bridge_wall_mask(&Shape::default());
            // clear to disable overhang detection
            gcode_layer.set_overhang_masks(&[]);
            // clear to disable overhang detection
            gcode_layer.set_seam_overhang_mask(&Shape::default());
            // clear to disable use of roofing settings
            gcode_layer.set_roofing_mask(&Shape::default());
            // clear to disable use of flooring settings
            gcode_layer.set_flooring_mask(&Shape::default());
        }

        if spiralize && extruder_nr == settings.get_extruder_train("wall_0_extruder_nr").extruder_nr && !part.spiral_wall.is_empty() {
            added_something = true;
            gcode_layer.set_is_inside(true); // going to print stuff inside print object

            // Only spiralize the first part in the mesh, any other parts will be printed using the normal, non-spiralize codepath.
            if part_idx == 0 {
                self.process_spiralized_wall(storage, gcode_layer, mesh_config, part, mesh);
            } else {
                // Print the spiral walls of other parts as single walls without Z gradient.
                let c = mesh_config.inset0_config;
                gcode_layer.add_walls(&part.spiral_wall, settings, &c, &c, &c, &c, ZSeamConfig::default(), 0, 1.0, false);
            }
        } else {
            // Main case: Optimize the insets with the InsetOrderOptimizer.
            let wall_x_wipe_dist = 0;
            let z_seam_config =
                ZSeamConfig::new(settings.get_enum("z_seam_type"), mesh.get_z_seam_hint(), settings.get_enum("z_seam_corner"), settings.get_coord("wall_line_width_0") * 2);
            let disallowed_areas_for_seams = Shape::default();
            let scarf_seam = true;
            let smooth_speed = true;
            let mut wall_orderer = InsetOrderOptimizer::new(
                gcode_layer.get_layer_nr(),
                settings,
                extruder_nr,
                WallConfigs {
                    inset_0_default: mesh_config.inset0_config,
                    inset_x_default: mesh_config.inset_x_config,
                    inset_0_roofing: mesh_config.inset0_roofing_config,
                    inset_x_roofing: mesh_config.inset_x_roofing_config,
                    inset_0_flooring: mesh_config.inset0_flooring_config,
                    inset_x_flooring: mesh_config.inset_x_flooring_config,
                    inset_0_bridge: mesh_config.bridge_inset0_config,
                    inset_x_bridge: mesh_config.bridge_inset_x_config,
                },
                settings.get_bool("travel_retract_before_outer_wall"),
                settings.get_coord("wall_0_wipe_dist"),
                wall_x_wipe_dist,
                settings.get_extruder_train("wall_0_extruder_nr").extruder_nr,
                settings.get_extruder_train("wall_x_extruder_nr").extruder_nr,
                z_seam_config,
                &part.wall_toolpaths,
                mesh.bounding_box.flatten().get_middle(),
                disallowed_areas_for_seams,
                scarf_seam,
                smooth_speed,
                gcode_layer.get_seam_overhang_mask().clone(),
            )
            .with_texture_data_provider(mesh.layers[gcode_layer.get_layer_nr() as usize].texture_data_provider.clone());
            added_something |= wall_orderer.add_to_layer(gcode_layer);
        }

        // clear overhang masks for any successive part on the layer
        gcode_layer.set_overhang_masks(&[]);

        added_something
    }

    fn get_seam_avoiding_location(filling_part: &Shape, filling_angle: i32, last_position: Point2LL) -> Option<Point2LL> {
        if filling_part.is_empty() {
            return None;
        }
        // start with the BB of the outline
        let skin_part_bb = AABB::from_shape(filling_part);
        let rot = PointMatrix::from_degrees(((-filling_angle + 90) % 360) as f64); // create a matrix to rotate a vector so that it is normal to the skin angle
        let bb_middle = skin_part_bb.get_middle();
        // create a vector from the middle of the BB whose length is such that it can be rotated
        // around the middle of the BB and the end will always be a long way outside of the part's outline
        // and rotate the vector so that it is normal to the skin angle
        let vec = rot.apply(pt(0, v_size(skin_part_bb.max - bb_middle) * 100));
        // find the vertex in the outline that is closest to the end of the rotated vector
        let pa = PolygonUtils::find_nearest_vert(bb_middle + vec, filling_part);
        // and find another outline vertex, this time using the vector + 180 deg
        let pb = PolygonUtils::find_nearest_vert(bb_middle - vec, filling_part);
        if !pa.initialized() || !pb.initialized() {
            return None;
        }
        // now go to whichever of those vertices that is closest to where we are now
        if v_size2(pa.p() - last_position) < v_size2(pb.p() - last_position) {
            Some(pa.p())
        } else {
            Some(pb.p())
        }
    }

    fn process_skin(&self, storage: &SliceDataStorage, gcode_layer: &mut LayerPlan, mesh: &SliceMeshStorage, extruder_nr: usize, mesh_config: &MeshPathConfigs, skin_parts: &[SkinPart]) -> bool {
        let settings = mesh.settings();
        let top_bottom_extruder_nr = settings.get_extruder_train("top_bottom_extruder_nr").extruder_nr;
        let roofing_extruder_nr = settings.get_extruder_train("roofing_extruder_nr").extruder_nr;
        let flooring_extruder_nr = settings.get_extruder_train("flooring_extruder_nr").extruder_nr;
        let wall_0_extruder_nr = settings.get_extruder_train("wall_0_extruder_nr").extruder_nr;
        let roofing_layer_count = settings.get_usize("roofing_layer_count").min(settings.get_usize("top_layers"));
        let flooring_layer_count = settings.get_usize("flooring_layer_count").min(settings.get_usize("bottom_layers"));
        if extruder_nr != top_bottom_extruder_nr
            && extruder_nr != wall_0_extruder_nr
            && (extruder_nr != roofing_extruder_nr || roofing_layer_count == 0)
            && (extruder_nr != flooring_extruder_nr || flooring_layer_count == 0)
        {
            return false;
        }
        let mut added_something = false;

        let mut part_order_optimizer: PathOrderOptimizer<usize> =
            PathOrderOptimizer::simple(gcode_layer.get_last_planned_position_or_starting_position(), ZSeamConfig::default(), false, None, false);
        for (idx, skin_part) in skin_parts.iter().enumerate() {
            part_order_optimizer.add_polygon(idx, Arc::new(skin_part.outline.outer_polygon().points.clone()), None, false);
        }
        part_order_optimizer.optimize(true);
        let ordered: Vec<usize> = part_order_optimizer.paths.iter().map(|p| p.vertices).collect();
        drop(part_order_optimizer);

        for idx in ordered {
            added_something |= self.process_skin_part(storage, gcode_layer, mesh, extruder_nr, mesh_config, &skin_parts[idx]);
        }

        added_something
    }

    fn process_skin_part(&self, storage: &SliceDataStorage, gcode_layer: &mut LayerPlan, mesh: &SliceMeshStorage, extruder_nr: usize, mesh_config: &MeshPathConfigs, skin_part: &SkinPart) -> bool {
        let mut added_something = false;
        self.process_roofing_flooring(
            storage,
            gcode_layer,
            mesh,
            extruder_nr,
            &ROOFING_SETTINGS_NAMES,
            &skin_part.roofing_fill,
            &mesh_config.roofing_config,
            &mesh.roofing_angles,
            &mut added_something,
        );
        self.process_roofing_flooring(
            storage,
            gcode_layer,
            mesh,
            extruder_nr,
            &FLOORING_SETTINGS_NAMES,
            &skin_part.flooring_fill,
            &mesh_config.flooring_config,
            &mesh.flooring_angles,
            &mut added_something,
        );
        self.process_top_bottom(storage, gcode_layer, mesh, extruder_nr, mesh_config, skin_part, &mut added_something);
        added_something
    }

    #[allow(clippy::too_many_arguments)]
    fn process_roofing_flooring(
        &self,
        storage: &SliceDataStorage,
        gcode_layer: &mut LayerPlan,
        mesh: &SliceMeshStorage,
        extruder_nr: usize,
        settings_names: &RoofingFlooringSettingsNames,
        fill: &Shape,
        config: &GCodePathConfig,
        angles: &[AngleDegrees],
        added_something: &mut bool,
    ) {
        let settings = mesh.settings();
        let skin_extruder_nr = settings.get_extruder_train(settings_names.extruder_nr).extruder_nr;
        if extruder_nr != skin_extruder_nr {
            return;
        }

        let pattern: EFillMethod = settings.get_enum(settings_names.pattern);
        let mut roofing_angle = AngleDegrees::new(45.0);
        if !angles.is_empty() {
            roofing_angle = angles[((gcode_layer.get_layer_nr() as u64) % angles.len() as u64) as usize];
        }

        let skin_density = 1.0;
        let skin_overlap = 0; // skinfill already expanded over the roofing areas; don't overlap with perimeters
        let ordering = if settings.get_bool(settings_names.monotonic) { LinesOrderingMethod::Monotonic } else { LinesOrderingMethod::Basic };
        let is_roofing_flooring = true;
        self.process_skin_print_feature(
            storage,
            gcode_layer,
            mesh,
            extruder_nr,
            fill,
            config,
            pattern,
            roofing_angle,
            skin_overlap,
            skin_density,
            ordering,
            is_roofing_flooring,
            added_something,
            GCodePathConfig::FAN_SPEED_DEFAULT,
            None,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn process_top_bottom<'s>(
        &self,
        storage: &'s SliceDataStorage,
        gcode_layer: &mut LayerPlan,
        mesh: &SliceMeshStorage,
        extruder_nr: usize,
        mesh_config: &MeshPathConfigs,
        skin_part: &SkinPart,
        added_something: &mut bool,
    ) {
        if skin_part.skin_fill.is_empty() {
            return; // bridgeAngle requires a non-empty skin_fill.
        }
        let settings = mesh.settings();
        let top_bottom_extruder_nr = settings.get_extruder_train("top_bottom_extruder_nr").extruder_nr;
        if extruder_nr != top_bottom_extruder_nr {
            return;
        }
        let mesh_group_settings = mesh_group_settings();

        let layer_nr = gcode_layer.get_layer_nr() as usize;

        let mut pattern: EFillMethod = if layer_nr == 0 { settings.get_enum("top_bottom_pattern_0") } else { settings.get_enum("top_bottom_pattern") };

        let mut skin_angle = AngleDegrees::new(45.0);
        if !mesh.skin_angles.is_empty() {
            skin_angle = mesh.skin_angles[layer_nr % mesh.skin_angles.len()];
        }

        // generate skin_polygons and skin_lines
        let mut skin_config: &GCodePathConfig = &mesh_config.skin_config;
        let mut skin_is_default_config = true;
        let mut skin_density: Ratio = 1.0;
        let skin_overlap = 0; // Skin overlap offset is applied in skin.cpp more overlap might be beneficial for curved bridges, but makes it worse in general.
        let bridge_settings_enabled = settings.get_bool("bridge_settings_enabled");
        let bridge_enable_more_layers = bridge_settings_enabled && settings.get_bool("bridge_enable_more_layers");
        let support_threshold = if bridge_settings_enabled { settings.get_ratio("bridge_skin_support_threshold") } else { 0.0 };
        let bottom_layers = settings.get_usize("bottom_layers");
        let mut forced_small_area_width: Option<Coord> = None;

        // if support is enabled, consider the support outlines so we don't generate bridges over support

        let mut support_layer_nr: i32 = -1;
        let mut support_layer: Option<&'s SupportLayer> = None;

        if mesh_group_settings.get_bool("support_enable") {
            let layer_height = mesh_config.inset0_config.get_layer_thickness();
            let z_distance_top = settings.get_coord("support_top_distance");
            let z_distance_top_layers = (z_distance_top / layer_height) as usize + 1;
            support_layer_nr = layer_nr.wrapping_sub(z_distance_top_layers) as i32;
        }

        // helper function that detects skin regions that have no support and modifies their print settings (config, line angle, density, etc.)
        let mut handle_bridge_skin = |bridge_layer: i32,
                                      config: &'_ GCodePathConfig,
                                      density: f64,
                                      support_layer: &mut Option<&'s SupportLayer>,
                                      skin_angle: &mut AngleDegrees,
                                      pattern: &mut EFillMethod,
                                      skin_density: &mut Ratio,
                                      skin_config: &mut GCodePathConfig,
                                      is_default: &mut bool|
         -> bool {
            if support_layer_nr >= (bridge_layer - 1) {
                *support_layer = Some(&storage.support.support_layers[(support_layer_nr - (bridge_layer - 1)) as usize]);
            }

            let mut supported_skin_part_regions = Shape::default();

            let bridge_angle = bridge_angle(mesh, &skin_part.skin_fill, storage, layer_nr as u32, bridge_layer as u32, *support_layer, &mut supported_skin_part_regions);

            if bridge_angle.is_some() || (support_threshold > 0.0 && (supported_skin_part_regions.area() / (skin_part.skin_fill.area() + 1.0) < support_threshold)) {
                if let Some(angle) = bridge_angle {
                    match bridge_layer {
                        2 => {
                            if bottom_layers > 2 {
                                // orientate second bridge skin at +45 deg to first
                                *skin_angle = angle + AngleDegrees::new(45.0);
                            } else {
                                // orientate second bridge skin at 90 deg to first
                                *skin_angle = angle + AngleDegrees::new(90.0);
                            }
                        }
                        3 => {
                            // orientate third bridge skin at 135 (same result as -45) deg to first
                            *skin_angle = angle + AngleDegrees::new(135.0);
                        }
                        _ => {
                            *skin_angle = angle;
                        }
                    }
                }
                forced_small_area_width = Some(0);
                *pattern = EFillMethod::LINES; // force lines pattern when bridging
                if bridge_settings_enabled {
                    *skin_config = *config;
                    *is_default = false;
                    *skin_density = density;
                }
                return true;
            }

            false
        };

        let mut chosen_config: GCodePathConfig = *skin_config;
        let mut is_bridge_skin = false;
        if layer_nr > 0 {
            is_bridge_skin = handle_bridge_skin(
                1,
                &mesh_config.bridge_skin_config,
                settings.get_ratio("bridge_skin_density"),
                &mut support_layer,
                &mut skin_angle,
                &mut pattern,
                &mut skin_density,
                &mut chosen_config,
                &mut skin_is_default_config,
            );
        }
        if bridge_enable_more_layers && !is_bridge_skin && layer_nr > 1 && bottom_layers > 1 {
            is_bridge_skin = handle_bridge_skin(
                2,
                &mesh_config.bridge_skin_config2,
                settings.get_ratio("bridge_skin_density_2"),
                &mut support_layer,
                &mut skin_angle,
                &mut pattern,
                &mut skin_density,
                &mut chosen_config,
                &mut skin_is_default_config,
            );

            if !is_bridge_skin && layer_nr > 2 && bottom_layers > 2 {
                is_bridge_skin = handle_bridge_skin(
                    3,
                    &mesh_config.bridge_skin_config3,
                    settings.get_ratio("bridge_skin_density_3"),
                    &mut support_layer,
                    &mut skin_angle,
                    &mut pattern,
                    &mut skin_density,
                    &mut chosen_config,
                    &mut skin_is_default_config,
                );
            }
        }
        skin_config = &chosen_config;

        let mut fan_speed = GCodePathConfig::FAN_SPEED_DEFAULT;

        if layer_nr > 0 && skin_is_default_config && support_layer_nr >= 0 && settings.get_bool("support_fan_enable") {
            // skin isn't a bridge but is it above support and we need to modify the fan speed?

            let skin_bb = AABB::from_shape(&skin_part.skin_fill);

            let support_layer = &storage.support.support_layers[support_layer_nr as usize];

            let mut supported = false;

            if !support_layer.support_roof.is_empty() {
                let support_roof_bb = AABB::from_shape(&support_layer.support_roof);
                if skin_bb.hit(&support_roof_bb) {
                    supported = !skin_part.skin_fill.intersection(&support_layer.support_roof).is_empty();
                }
            } else {
                for support_part in support_layer.support_infill_parts.iter() {
                    let support_part_bb = AABB::from_shape(support_part.get_infill_area());
                    if skin_bb.hit(&support_part_bb) {
                        supported = !skin_part.skin_fill.intersection(support_part.get_infill_area()).is_empty();

                        if supported {
                            break;
                        }
                    }
                }
            }

            if supported {
                fan_speed = settings.get_ratio("support_supported_skin_fan_speed") * 100.0;
            }
        }

        let ordering = if is_bridge_skin {
            if settings.get_bool("bridge_interlace_lines") {
                LinesOrderingMethod::Interlaced
            } else {
                LinesOrderingMethod::Basic
            }
        } else if settings.get_bool("skin_monotonic") {
            LinesOrderingMethod::Monotonic
        } else {
            LinesOrderingMethod::Basic
        };

        let is_roofing_flooring = false;
        self.process_skin_print_feature(
            storage,
            gcode_layer,
            mesh,
            extruder_nr,
            &skin_part.skin_fill,
            skin_config,
            pattern,
            skin_angle,
            skin_overlap,
            skin_density,
            ordering,
            is_roofing_flooring,
            added_something,
            fan_speed,
            forced_small_area_width,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn process_skin_print_feature(
        &self,
        _storage: &SliceDataStorage,
        gcode_layer: &mut LayerPlan,
        mesh: &SliceMeshStorage,
        extruder_nr: usize,
        area: &Shape,
        config: &GCodePathConfig,
        pattern: EFillMethod,
        skin_angle: AngleDegrees,
        skin_overlap: Coord,
        skin_density: Ratio,
        ordering: LinesOrderingMethod,
        is_roofing_flooring: bool,
        added_something: &mut bool,
        fan_speed: f64,
        forced_small_area_width: Option<Coord>,
    ) {
        let settings = mesh.settings();
        let mut skin_polygons = Shape::default();
        let mut skin_lines = OpenLinesSet::default();
        let mut skin_paths: Vec<VariableWidthLines> = Vec::new();

        let infill_multiplier = 1;
        let extra_infill_shift = 0;
        let wall_line_count = settings.get_usize("skin_outline_count");
        let zig_zaggify_infill = pattern == EFillMethod::ZIG_ZAG;
        let connect_polygons = settings.get_bool("connect_skin_polygons");
        let max_resolution = settings.get_coord("meshfix_maximum_resolution");
        let max_deviation = settings.get_coord("meshfix_maximum_deviation");
        let infill_origin = Point2LL::default();
        let skip_line_stitching = ordering == LinesOrderingMethod::Monotonic || ordering == LinesOrderingMethod::Interlaced;
        let fill_gaps = true;
        let connected_zigzags = false;
        let use_endpieces = true;
        let skip_some_zags = false;
        let zag_skip_count = 0;
        let pocket_size = 0;
        let small_areas_on_surface = settings.get_bool("small_skin_on_surface");
        let line_width = config.get_line_width();
        let small_area_width =
            forced_small_area_width.unwrap_or(if small_areas_on_surface || !is_roofing_flooring { settings.get_coord("small_skin_width") } else { line_width / 4 });
        let current_layer = &mesh.layers[gcode_layer.get_layer_nr() as usize];
        let exposed_to_air = current_layer.top_surface.areas.union_polygons_other(&current_layer.bottom_surface);

        let mut infill_comp = make_infill(
            pattern,
            zig_zaggify_infill,
            connect_polygons,
            area.clone(),
            config.get_line_width(),
            (config.get_line_width() as f64 / skin_density) as Coord,
            skin_overlap,
            infill_multiplier,
            skin_angle,
            gcode_layer.z,
            extra_infill_shift,
            max_resolution,
            max_deviation,
            wall_line_count,
            small_area_width,
            infill_origin,
            skip_line_stitching,
            fill_gaps,
            connected_zigzags,
            use_endpieces,
            skip_some_zags,
            zag_skip_count,
            pocket_size,
        );
        infill_comp.generate(
            &mut skin_paths,
            &mut skin_polygons,
            &mut skin_lines,
            settings,
            gcode_layer.get_layer_nr() as i32,
            SectionType::SKIN,
            None,
            None,
            None,
            &(if small_areas_on_surface { Shape::default() } else { exposed_to_air }),
        );

        // add paths
        if !skin_polygons.is_empty() || !skin_lines.is_empty() || !skin_paths.is_empty() {
            *added_something = true;
            gcode_layer.set_is_inside(true); // going to print stuff inside print object
            if !skin_paths.is_empty() {
                // Add skin-walls a.k.a. skin-perimeters, skin-insets.
                let retract_before_outer_wall = false;
                let wipe_dist = 0;
                let z_seam_config = ZSeamConfig::new(settings.get_enum("z_seam_type"), mesh.get_z_seam_hint(), settings.get_enum("z_seam_corner"), config.get_line_width() * 2);
                let mut wall_orderer = InsetOrderOptimizer::new(
                    gcode_layer.get_layer_nr(),
                    settings,
                    extruder_nr,
                    WallConfigs::uniform(config),
                    retract_before_outer_wall,
                    wipe_dist,
                    wipe_dist,
                    extruder_nr,
                    extruder_nr,
                    z_seam_config,
                    &skin_paths,
                    mesh.bounding_box.flatten().get_middle(),
                    Shape::default(),
                    false,
                    false,
                    Shape::default(),
                );
                *added_something |= wall_orderer.add_to_layer(gcode_layer);
            }
            if !skin_polygons.is_empty() {
                let force_comb_retract = false;
                gcode_layer.add_travel(skin_polygons[0][0], force_comb_retract, 0);
                gcode_layer.add_polygons_by_optimizer(&skin_polygons, config, settings, PolygonsOpts::default());
            }

            if ordering == LinesOrderingMethod::Monotonic || ordering == LinesOrderingMethod::Interlaced {
                let exclude_distance = (config.get_line_width() as f64 * 0.8) as Coord;

                let interlaced = ordering == LinesOrderingMethod::Interlaced;
                let monotonic_direction = AngleRadians::from(skin_angle);
                let flow = 1.0;
                let max_adjacent_distance = (infill_comp.get_line_distance() as f64 * 1.1) as Coord; // Lines are considered adjacent if they are 1 line width apart, with 10% extra play.
                if matches!(
                    pattern,
                    EFillMethod::GRID
                        | EFillMethod::LINES
                        | EFillMethod::TRIANGLES
                        | EFillMethod::CUBIC
                        | EFillMethod::TETRAHEDRAL
                        | EFillMethod::QUARTER_CUBIC
                        | EFillMethod::CUBICSUBDIV
                        | EFillMethod::LIGHTNING
                ) {
                    gcode_layer.add_lines_monotonic(
                        area,
                        &skin_lines,
                        config,
                        SpaceFillType::Lines,
                        monotonic_direction,
                        max_adjacent_distance,
                        exclude_distance,
                        settings.get_coord("infill_wipe_dist"),
                        flow,
                        fan_speed,
                        interlaced,
                    );
                } else {
                    let space_fill_type = if pattern == EFillMethod::ZIG_ZAG { SpaceFillType::PolyLines } else { SpaceFillType::Lines };
                    let wipe_dist = 0;
                    gcode_layer.add_lines_monotonic(area, &skin_lines, config, space_fill_type, monotonic_direction, max_adjacent_distance, exclude_distance, wipe_dist, flow, fan_speed, interlaced);
                }
            } else {
                let mut near_start_location: Option<Point2LL> = None;
                let actual_pattern: EFillMethod = if gcode_layer.get_layer_nr() == 0 { settings.get_enum("top_bottom_pattern_0") } else { settings.get_enum("top_bottom_pattern") };
                if actual_pattern == EFillMethod::LINES || actual_pattern == EFillMethod::ZIG_ZAG {
                    // update near_start_location to a location which tries to avoid seams in skin
                    near_start_location = Self::get_seam_avoiding_location(area, skin_angle.value() as i32, gcode_layer.get_last_planned_position_or_starting_position());
                }

                let enable_travel_optimization = false;
                let flow = 1.0;
                if matches!(
                    actual_pattern,
                    EFillMethod::GRID
                        | EFillMethod::LINES
                        | EFillMethod::TRIANGLES
                        | EFillMethod::CUBIC
                        | EFillMethod::TETRAHEDRAL
                        | EFillMethod::QUARTER_CUBIC
                        | EFillMethod::CUBICSUBDIV
                        | EFillMethod::LIGHTNING
                ) {
                    gcode_layer.add_lines_by_optimizer_open(
                        &skin_lines,
                        config,
                        SpaceFillType::Lines,
                        LinesOpts {
                            enable_travel_optimization,
                            wipe_dist: settings.get_coord("infill_wipe_dist"),
                            flow_ratio: flow,
                            near_start_location,
                            fan_speed,
                            ..Default::default()
                        },
                    );
                } else {
                    let space_fill_type = if actual_pattern == EFillMethod::ZIG_ZAG { SpaceFillType::PolyLines } else { SpaceFillType::Lines };
                    let wipe_dist = 0;
                    gcode_layer.add_lines_by_optimizer_open(
                        &skin_lines,
                        config,
                        space_fill_type,
                        LinesOpts { enable_travel_optimization, wipe_dist, flow_ratio: flow, near_start_location, fan_speed, ..Default::default() },
                    );
                }
            }
        }
    }

    fn process_ironing(&self, storage: &SliceDataStorage, mesh: &SliceMeshStorage, layer: &SliceLayer, line_config: &GCodePathConfig, gcode_layer: &mut LayerPlan) -> bool {
        let mut added_something = false;
        let settings = mesh.settings();
        let ironing_enabled = settings.get_bool("ironing_enabled");
        let ironing_only_highest_layer = settings.get_bool("ironing_only_highest_layer");
        if ironing_enabled && (!ironing_only_highest_layer || mesh.layer_nr_max_filled_layer == gcode_layer.get_layer_nr()) {
            // Since we are ironing after all the parts are completed, it believes that it is outside.
            // But the truth is that we are inside a part, so we need to change it before we do the ironing
            // See CURA-8615
            gcode_layer.set_is_inside(true);
            added_something |= Self::top_surface_ironing(&layer.top_surface.areas, storage, mesh, line_config, gcode_layer);
            gcode_layer.set_is_inside(false);
        }
        added_something
    }

    /// `TopSurface::ironing`
    fn top_surface_ironing(areas: &Shape, storage: &SliceDataStorage, mesh: &SliceMeshStorage, line_config: &GCodePathConfig, layer: &mut LayerPlan) -> bool {
        if areas.is_empty() {
            return false; // Nothing to do.
        }
        let settings = mesh.settings();
        // Generate the lines to cover the surface.
        let extruder_nr = settings.get_extruder_train("top_bottom_extruder_nr").extruder_nr;
        let pattern: EFillMethod = settings.get_enum("ironing_pattern");
        let zig_zaggify_infill = pattern == EFillMethod::ZIG_ZAG;
        let connect_polygons = false; // midway connections can make the surface less smooth
        let line_spacing = settings.get_coord("ironing_line_spacing");
        let line_width = line_config.get_line_width();
        let roofing_layer_count = settings.get_usize("roofing_layer_count").min(settings.get_usize("top_layers"));
        let top_most_skin_angles = if roofing_layer_count > 0 { &mesh.roofing_angles } else { &mesh.skin_angles };
        let direction =
            top_most_skin_angles[((layer.get_layer_nr() as u64) % top_most_skin_angles.len() as u64) as usize] + AngleDegrees::new(90.0); // Always perpendicular to the skin lines.
        let infill_overlap = 0;
        let infill_multiplier = 1;
        let shift = 0;
        let max_resolution = settings.get_coord("meshfix_maximum_resolution");
        let max_deviation = settings.get_coord("meshfix_maximum_deviation");
        let ironing_flow = settings.get_ratio("ironing_flow");
        let enforce_monotonic_order = settings.get_bool("ironing_monotonic");
        let wall_line_count = 0;
        let small_area_width = 0; // This shouldn't be on for ironing.
        let infill_origin = Point2LL::default();
        let skip_line_stitching = enforce_monotonic_order;

        let mut ironing_inset: Coord = -settings.get_coord("ironing_inset");
        if pattern == EFillMethod::ZIG_ZAG {
            // Compensate for the outline_offset decrease that takes place when using the infill generator to generate ironing with the zigzag pattern
            let width_scale: Ratio = settings.get_coord("layer_height") as f64 / settings.get_coord("infill_sparse_thickness") as f64;
            ironing_inset = (ironing_inset as f64 + width_scale * line_width as f64 / 2.0) as Coord;
            // Align the edge of the ironing line with the edge of the outer wall
            ironing_inset = (ironing_inset as f64 - ironing_flow * line_width as f64 / 2.0) as Coord;
        } else if pattern == EFillMethod::CONCENTRIC {
            // Counteract the outline_offset increase that takes place when using the infill generator to generate ironing with the concentric pattern
            ironing_inset += line_spacing - line_width / 2;
            // Align the edge of the ironing line with the edge of the outer wall
            ironing_inset = (ironing_inset as f64 - ironing_flow * line_width as f64 / 2.0) as Coord;
        }
        let ironed_areas = areas.offset_default(ironing_inset);

        let mut infill_generator = Infill::new(
            pattern,
            zig_zaggify_infill,
            connect_polygons,
            ironed_areas.clone(),
            line_width,
            line_spacing,
            infill_overlap,
            infill_multiplier,
            direction,
            layer.z - 10,
            shift,
            max_resolution,
            max_deviation,
            InfillExtra { wall_line_count, small_area_width, infill_origin, skip_line_stitching, zigzag: None },
        );
        let mut ironing_paths: Vec<VariableWidthLines> = Vec::new();
        let mut ironing_polygons = Shape::default();
        let mut ironing_lines = OpenLinesSet::default();
        infill_generator.generate(
            &mut ironing_paths,
            &mut ironing_polygons,
            &mut ironing_lines,
            settings,
            layer.get_layer_nr() as i32,
            SectionType::IRONING,
            None,
            None,
            None,
            &Shape::default(),
        );

        if ironing_polygons.is_empty() && ironing_lines.is_empty() && ironing_paths.is_empty() {
            return false; // Nothing to do.
        }

        let mut added = false;
        if !ironing_polygons.is_empty() {
            let force_comb_retract = false;
            layer.add_travel(ironing_polygons[0][0], force_comb_retract, 0);
            layer.add_polygons_by_optimizer(&ironing_polygons, line_config, settings, PolygonsOpts::default());
            added = true;
        }
        if !ironing_lines.is_empty() {
            if pattern == EFillMethod::LINES || pattern == EFillMethod::ZIG_ZAG {
                // Move to a corner of the area that is perpendicular to the ironing lines, to reduce the number of seams.
                let bounding_box = AABB::from_shape(&ironed_areas);
                let rotate = PointMatrix::from_degrees(-direction.value() + 90.0);
                let center = bounding_box.get_middle();
                let far_away = rotate.apply(pt(0, v_size(bounding_box.max - center) * 100)); // Some direction very far away in the direction perpendicular to the ironing lines, relative to the centre.
                // Two options to start, both perpendicular to the ironing lines. Which is closer?
                let front_side = PolygonUtils::find_nearest_vert(center + far_away, &ironed_areas).p();
                let back_side = PolygonUtils::find_nearest_vert(center - far_away, &ironed_areas).p();
                let last = layer.get_last_planned_position_or_starting_position();
                if v_size2(last - front_side) < v_size2(last - back_side) {
                    layer.add_travel(front_side, false, 0);
                } else {
                    layer.add_travel(back_side, false, 0);
                }
            }

            if !enforce_monotonic_order {
                layer.add_lines_by_optimizer_open(&ironing_lines, line_config, SpaceFillType::PolyLines, LinesOpts::default());
            } else {
                let max_adjacent_distance = (line_spacing as f64 * 1.1) as Coord; // Lines are considered adjacent - meaning they need to be printed in monotonic order - if spaced 1 line apart, with 10% extra play.
                layer.add_lines_monotonic(
                    &Shape::default(),
                    &ironing_lines,
                    line_config,
                    SpaceFillType::PolyLines,
                    AngleRadians::from(direction),
                    max_adjacent_distance,
                    0,
                    0,
                    1.0,
                    GCodePathConfig::FAN_SPEED_DEFAULT,
                    false,
                );
            }
            added = true;
        }
        if !ironing_paths.is_empty() {
            let retract_before_outer_wall = false;
            let wipe_dist = 0;
            let z_seam_config = ZSeamConfig::new(EZSeamType::SHORTEST, layer.get_last_planned_position_or_starting_position(), EZSeamCornerPrefType::Z_SEAM_CORNER_PREF_INNER, 0);
            let mut wall_orderer = InsetOrderOptimizer::new(
                layer.get_layer_nr(),
                settings,
                extruder_nr,
                WallConfigs::uniform(line_config),
                retract_before_outer_wall,
                wipe_dist,
                wipe_dist,
                extruder_nr,
                extruder_nr,
                z_seam_config,
                &ironing_paths,
                storage.get_model_bounding_box().flatten().get_middle(),
                Shape::default(),
                false,
                false,
                Shape::default(),
            );
            wall_orderer.add_to_layer(layer);
            added = true;
        }
        added
    }

    fn add_support_to_gcode(&self, storage: &SliceDataStorage, gcode_layer: &mut LayerPlan, extruder_nr: usize) -> bool {
        let mut support_added = false;
        if !storage.support.generated || gcode_layer.get_layer_nr() > storage.support.layer_nr_max_filled_layer as LayerIndex {
            return support_added;
        }

        let mesh_group_settings = mesh_group_settings();
        let support_roof_extruder_nr = mesh_group_settings.get_extruder_train("support_roof_extruder_nr").extruder_nr;
        let support_bottom_extruder_nr = mesh_group_settings.get_extruder_train("support_bottom_extruder_nr").extruder_nr;
        let support_infill_extruder_nr = if gcode_layer.get_layer_nr() <= 0 {
            mesh_group_settings.get_extruder_train("support_extruder_nr_layer_0").extruder_nr
        } else {
            mesh_group_settings.get_extruder_train("support_infill_extruder_nr").extruder_nr
        };

        let support_layer = &storage.support.support_layers[gcode_layer.get_layer_nr().max(0) as usize];
        if support_layer.support_bottom.is_empty() && support_layer.support_roof.is_empty() && support_layer.support_infill_parts.is_empty() {
            return support_added;
        }

        if extruder_nr == support_roof_extruder_nr {
            let config = gcode_layer.configs_storage.support_fractional_roof_config;
            support_added |= self.add_support_roofs_to_gcode(storage, &support_layer.support_fractional_roof, &config, gcode_layer);
        }
        if extruder_nr == support_infill_extruder_nr {
            support_added |= self.process_support_infill(storage, gcode_layer);
        }
        if extruder_nr == support_roof_extruder_nr {
            let config = gcode_layer.configs_storage.support_roof_config;
            support_added |= self.add_support_roofs_to_gcode(storage, &support_layer.support_roof.difference(&support_layer.support_fractional_roof), &config, gcode_layer);
        }
        if extruder_nr == support_bottom_extruder_nr {
            support_added |= self.add_support_bottoms_to_gcode(storage, gcode_layer);
        }
        support_added
    }

    fn process_support_infill(&self, storage: &SliceDataStorage, gcode_layer: &mut LayerPlan) -> bool {
        let mut added_something = false;
        let support_layer = &storage.support.support_layers[gcode_layer.get_layer_nr().max(0) as usize]; // account for negative layer numbers for raft filler layers

        if gcode_layer.get_layer_nr() > storage.support.layer_nr_max_filled_layer as LayerIndex || support_layer.support_infill_parts.is_empty() {
            return added_something;
        }

        let mesh_group_settings = mesh_group_settings();
        let extruder_nr = if gcode_layer.get_layer_nr() <= 0 {
            mesh_group_settings.get_extruder_train("support_extruder_nr_layer_0").extruder_nr
        } else {
            mesh_group_settings.get_extruder_train("support_infill_extruder_nr").extruder_nr
        };
        let infill_extruder = extruder(extruder_nr);
        let es = &infill_extruder.settings;

        let mut default_support_line_distance = es.get_coord("support_line_distance");

        // To improve adhesion for the "support initial layer" the first layer might have different properties
        if gcode_layer.get_layer_nr() == 0 {
            default_support_line_distance = es.get_coord("support_initial_layer_line_distance");
        }

        let default_support_infill_overlap = es.get_coord("infill_overlap_mm");

        // Helper to get the support infill angle
        let get_support_infill_angle = |support_storage: &SupportStorage, layer_nr: i32| -> AngleDegrees {
            if layer_nr <= 0 {
                // handle negative layer numbers
                let divisor = support_storage.support_infill_angles_layer_0.len();
                let index = (((layer_nr as i64 as u64).wrapping_rem(divisor as u64)).wrapping_add(divisor as u64)) % divisor as u64;
                return support_storage.support_infill_angles_layer_0[index as usize];
            }
            support_storage.support_infill_angles[(layer_nr as usize) % support_storage.support_infill_angles.len()]
        };
        let support_infill_angle = get_support_infill_angle(&storage.support, gcode_layer.get_layer_nr() as i32);

        let infill_multiplier = 1; // there is no frontend setting for this (yet)
        let mut infill_density_multiplier: usize = 1;
        if gcode_layer.get_layer_nr() <= 0 {
            infill_density_multiplier = es.get_usize("support_infill_density_multiplier_initial_layer");
        }

        let wall_line_count = es.get_usize("support_wall_count");
        let max_resolution = es.get_coord("meshfix_maximum_resolution");
        let max_deviation = es.get_coord("meshfix_maximum_deviation");
        let mut default_support_line_width = es.get_coord("support_line_width");
        if gcode_layer.get_layer_nr() == 0 && mesh_group_settings.get_enum::<EPlatformAdhesion>("adhesion_type") != EPlatformAdhesion::RAFT {
            default_support_line_width = (default_support_line_width as f64 * es.get_ratio("initial_layer_line_width_factor")) as Coord;
        }

        // Helper to get the support pattern
        let get_support_pattern = |pattern: EFillMethod, layer_nr: i32| -> EFillMethod {
            if layer_nr <= 0 && (pattern == EFillMethod::LINES || pattern == EFillMethod::ZIG_ZAG) {
                return EFillMethod::GRID;
            }
            pattern
        };
        let support_pattern = get_support_pattern(es.get_enum("support_pattern"), gcode_layer.get_layer_nr() as i32);

        let zig_zaggify_infill = es.get_bool("zig_zaggify_support");
        let skip_some_zags = es.get_bool("support_skip_some_zags");
        let zag_skip_count = es.get_usize("support_zag_skip_count");

        // create a list of outlines and use PathOrderOptimizer to optimize the travel move
        let start = gcode_layer.get_last_planned_position_or_starting_position();
        let mut island_order_optimizer_initial: PathOrderOptimizer<usize> = PathOrderOptimizer::simple(start, ZSeamConfig::default(), false, None, false);
        let mut island_order_optimizer: PathOrderOptimizer<usize> = PathOrderOptimizer::simple(start, ZSeamConfig::default(), false, None, false);
        for (idx, part) in support_layer.support_infill_parts.iter().enumerate() {
            let opt = if part.use_fractional_config { &mut island_order_optimizer_initial } else { &mut island_order_optimizer };
            opt.add_polygon(idx, Arc::new(part.outline.outer_polygon().points.clone()), None, false);
        }
        island_order_optimizer_initial.optimize(true);
        island_order_optimizer.optimize(true);
        let ordered: Vec<usize> = island_order_optimizer_initial.paths.iter().chain(island_order_optimizer.paths.iter()).map(|p| p.vertices).collect();
        drop(island_order_optimizer_initial);
        drop(island_order_optimizer);

        let support_connect_zigzags = es.get_bool("support_connect_zigzags");
        let support_structure: ESupportStructure = es.get_enum("support_structure");
        let infill_origin = Point2LL::default();

        let use_endpieces = true;
        let pocket_size = 0;
        let connect_polygons = false; // polygons are too distant to connect for sparse support
        let mut need_travel_to_end_of_last_spiral = true;

        // Print the thicker infill lines first. (double or more layer thickness, infill combined with previous layers)
        for part_idx in ordered {
            let part = &support_layer.support_infill_parts[part_idx];
            let configs: Vec<GCodePathConfig> =
                if part.use_fractional_config { gcode_layer.configs_storage.support_fractional_infill_config.clone() } else { gcode_layer.configs_storage.support_infill_config.clone() };

            // always process the wall overlap if walls are generated
            let current_support_infill_overlap: i32 = if part.inset_count_to_generate > 0 { default_support_infill_overlap as i32 } else { 0 };

            // The support infill walls were generated separately, first. Always add them, regardless of how many densities we have.
            let wall_toolpaths: &Vec<VariableWidthLines> = &part.wall_toolpaths;

            if !wall_toolpaths.is_empty() {
                let config = configs[0];
                let retract_before_outer_wall = false;
                let wipe_dist = 0;
                let layer_nr = gcode_layer.get_layer_nr();
                let z_seam_config = ZSeamConfig::new(EZSeamType::SHORTEST, gcode_layer.get_last_planned_position_or_starting_position(), EZSeamCornerPrefType::Z_SEAM_CORNER_PREF_INNER, 0);
                let mut disallowed_area_for_seams = Shape::default();
                if es.get_bool("support_z_seam_away_from_model") && layer_nr >= 0 {
                    for mesh in storage.meshes.iter() {
                        for part in mesh.layers[layer_nr as usize].parts.iter() {
                            disallowed_area_for_seams.push_all(&part.print_outline);
                        }
                    }
                    if !disallowed_area_for_seams.is_empty() {
                        let min_distance = es.get_coord("support_z_seam_min_distance");
                        disallowed_area_for_seams = disallowed_area_for_seams.offset_join(min_distance, clipper::JoinType::Round);
                    }
                }

                let mut wall_orderer = InsetOrderOptimizer::new(
                    gcode_layer.get_layer_nr(),
                    es,
                    extruder_nr,
                    WallConfigs::uniform(&config),
                    retract_before_outer_wall,
                    wipe_dist,
                    wipe_dist,
                    extruder_nr,
                    extruder_nr,
                    z_seam_config,
                    wall_toolpaths,
                    storage.get_model_bounding_box().flatten().get_middle(),
                    disallowed_area_for_seams,
                    false,
                    false,
                    Shape::default(),
                );
                added_something |= wall_orderer.add_to_layer(gcode_layer);
            }

            if (default_support_line_distance <= 0 && support_structure != ESupportStructure::TREE) || part.infill_area_per_combine_per_density.is_empty() {
                continue;
            }

            for combine_idx in 0..part.infill_area_per_combine_per_density[0].len() {
                let support_line_width = default_support_line_width * (combine_idx as Coord + 1);

                let mut support_polygons = Shape::default();
                let mut wall_toolpaths_here: Vec<VariableWidthLines> = Vec::new();
                let mut support_lines = OpenLinesSet::default();
                let max_density_idx = part.infill_area_per_combine_per_density.len() - 1;
                let mut density_idx = max_density_idx;
                while density_idx.wrapping_add(1) > 0 {
                    if combine_idx >= part.infill_area_per_combine_per_density[density_idx].len() {
                        density_idx = density_idx.wrapping_sub(1);
                        continue;
                    }

                    let density_factor: u32 = 2 << density_idx; // == pow(2, density_idx + 1)
                    let mut support_line_distance_here: Coord =
                        if part.custom_line_distance > 0 { part.custom_line_distance } else { default_support_line_distance * density_factor as Coord }; // the highest density infill combines with the next to create a grid with density_factor 1
                    if support_line_distance_here != 0 && infill_density_multiplier > 1 {
                        support_line_distance_here /= 1 << (infill_density_multiplier - 1);
                        support_line_distance_here = max_c(support_line_distance_here, support_line_width);
                    }
                    let support_shift: i32 = (support_line_distance_here / 2) as i32;
                    if part.custom_line_distance == 0 && (density_idx == max_density_idx || support_pattern == EFillMethod::CROSS || support_pattern == EFillMethod::CROSS_3D) {
                        support_line_distance_here /= 2;
                    }
                    let area = Simplify::from_settings(es).shape(&part.infill_area_per_combine_per_density[density_idx][combine_idx]);

                    let wall_count = 0; // Walls are generated somewhere else, so their layers aren't vertically combined.
                    let small_area_width = 0;
                    let skip_stitching = false;
                    let fill_gaps = density_idx == 0; // Only fill gaps for one of the densities.
                    let overlap = (current_support_infill_overlap as Coord)
                        .wrapping_sub(if density_idx == max_density_idx { 0 } else { (wall_line_count as Coord).wrapping_mul(support_line_width) });
                    let mut infill_comp = make_infill(
                        support_pattern,
                        zig_zaggify_infill,
                        connect_polygons,
                        area,
                        support_line_width,
                        support_line_distance_here,
                        overlap,
                        infill_multiplier,
                        support_infill_angle,
                        gcode_layer.z + configs[combine_idx].z_offset,
                        support_shift as Coord,
                        max_resolution,
                        max_deviation,
                        wall_count,
                        small_area_width,
                        infill_origin,
                        skip_stitching,
                        fill_gaps,
                        support_connect_zigzags,
                        use_endpieces,
                        skip_some_zags,
                        zag_skip_count,
                        pocket_size,
                    );
                    infill_comp.generate(
                        &mut wall_toolpaths_here,
                        &mut support_polygons,
                        &mut support_lines,
                        es,
                        gcode_layer.get_layer_nr() as i32,
                        SectionType::SUPPORT,
                        storage.support.cross_fill_provider.as_deref(),
                        None,
                        None,
                        &Shape::default(),
                    );
                    density_idx = density_idx.wrapping_sub(1);
                }

                if need_travel_to_end_of_last_spiral && es.get_bool("magic_spiralize") {
                    if !wall_toolpaths.is_empty() || !support_polygons.is_empty() || !support_lines.is_empty() {
                        let layer_nr = gcode_layer.get_layer_nr() as i32;
                        if layer_nr > es.get_usize("initial_bottom_layers") as i32 {
                            // bit of subtlety here... support is being used on a spiralized model and to ensure the travel move from the end of the last spiral
                            // to the start of the support does not go through the model we have to tell the slicer what the current location of the nozzle is
                            // by adding a travel move to the end vertex of the last spiral.
                            let last_wall_outline = storage.spiralize_wall_outlines[(layer_nr - 1) as usize];
                            if !last_wall_outline.is_null() {
                                let p = unsafe { (&*last_wall_outline)[0][storage.spiralize_seam_vertex_indices[(layer_nr - 1) as usize] as usize] };
                                gcode_layer.add_travel(p, false, 0);
                                need_travel_to_end_of_last_spiral = false;
                            }
                        }
                    }
                }

                gcode_layer.set_is_inside(false); // going to print stuff outside print object, i.e. support

                let alternate_inset_direction = es.get_bool("material_alternate_walls");
                let alternate_layer_print_direction = alternate_inset_direction && gcode_layer.get_layer_nr() % 2 == 1;

                if !support_polygons.is_empty() {
                    let force_comb_retract = false;
                    gcode_layer.add_travel(support_polygons[0][0], force_comb_retract, 0);

                    gcode_layer.add_polygons_by_optimizer(
                        &support_polygons,
                        &configs[combine_idx],
                        mesh_group_settings,
                        PolygonsOpts { reverse_order: alternate_layer_print_direction, ..Default::default() },
                    );
                    added_something = true;
                }

                if !support_lines.is_empty() {
                    gcode_layer.add_lines_by_optimizer_open(
                        &support_lines,
                        &configs[combine_idx],
                        if support_pattern == EFillMethod::ZIG_ZAG { SpaceFillType::PolyLines } else { SpaceFillType::Lines },
                        LinesOpts { reverse_print_direction: alternate_layer_print_direction, ..Default::default() },
                    );

                    added_something = true;
                }

                // If we're printing with a support wall, that support wall generates gap filling as well.
                // If not, the pattern may still generate gap filling (if it's connected infill or zigzag). We still want to print those.
                if wall_line_count == 0 || !wall_toolpaths_here.is_empty() {
                    let config = configs[0];
                    let retract_before_outer_wall = false;
                    let wipe_dist = 0;
                    let simplify_curvature = 0;
                    let z_seam_config =
                        ZSeamConfig::new(EZSeamType::SHORTEST, gcode_layer.get_last_planned_position_or_starting_position(), EZSeamCornerPrefType::Z_SEAM_CORNER_PREF_INNER, simplify_curvature);
                    let mut wall_orderer = InsetOrderOptimizer::new(
                        gcode_layer.get_layer_nr(),
                        es,
                        extruder_nr,
                        WallConfigs::uniform(&config),
                        retract_before_outer_wall,
                        wipe_dist,
                        wipe_dist,
                        extruder_nr,
                        extruder_nr,
                        z_seam_config,
                        &wall_toolpaths_here,
                        storage.get_model_bounding_box().flatten().get_middle(),
                        Shape::default(),
                        false,
                        false,
                        Shape::default(),
                    );
                    added_something |= wall_orderer.add_to_layer(gcode_layer);
                }
            }
        }

        added_something
    }

    fn add_support_roofs_to_gcode(&self, storage: &SliceDataStorage, support_roof_outlines: &Shape, current_roof_config: &GCodePathConfig, gcode_layer: &mut LayerPlan) -> bool {
        let support_layer = &storage.support.support_layers[gcode_layer.get_layer_nr().max(0) as usize];

        if !storage.support.generated || gcode_layer.get_layer_nr() > storage.support.layer_nr_max_filled_layer as LayerIndex || support_layer.support_roof.is_empty() {
            return false; // No need to generate support roof if there's no support.
        }

        let roof_extruder_nr = mesh_group_settings().get_extruder_train("support_roof_extruder_nr").extruder_nr;
        let roof_extruder = extruder(roof_extruder_nr);
        let rs = &roof_extruder.settings;

        let pattern: EFillMethod = rs.get_enum("support_roof_pattern");
        let mut fill_angle = AngleDegrees::new(0.0);
        if !storage.support.support_roof_angles.is_empty() {
            // handle negative layer numbers
            let divisor = storage.support.support_roof_angles.len() as i32;
            let index = (((gcode_layer.get_layer_nr() % divisor as LayerIndex) as i32) + divisor) % divisor;
            fill_angle = storage.support.support_roof_angles[index as usize];
        }
        let zig_zaggify_infill = pattern == EFillMethod::ZIG_ZAG;
        let connect_polygons = false; // connections might happen in mid air in between the infill lines
        let support_roof_overlap = 0; // the roofs should never be expanded outwards
        let infill_multiplier = 1;
        let extra_infill_shift = 0;
        let wall_line_count = rs.get_usize("support_roof_wall_count");
        let small_area_width = rs.get_coord("min_even_wall_line_width") * 2; // Maximum width of a region that can still be filled with one wall.
        let infill_origin = Point2LL::default();
        let skip_stitching = false;
        let fill_gaps = true;
        let use_endpieces = true;
        let connected_zigzags = false;
        let skip_some_zags = false;
        let zag_skip_count = 0;
        let pocket_size = 0;
        let max_resolution = rs.get_coord("meshfix_maximum_resolution");
        let max_deviation = rs.get_coord("meshfix_maximum_deviation");

        let mut support_roof_line_distance = rs.get_coord("support_roof_line_distance");
        let support_roof_line_width = rs.get_coord("support_roof_line_width");
        if gcode_layer.get_layer_nr() == 0 && support_roof_line_distance < 2 * support_roof_line_width {
            // if roof is dense
            support_roof_line_distance = (support_roof_line_distance as f64 * rs.get_ratio("initial_layer_line_width_factor")) as Coord;
        }

        let mut infill_outline = support_roof_outlines.clone();
        let mut wall = Shape::default();
        // make sure there is a wall if this is on the first layer
        if gcode_layer.get_layer_nr() == 0 {
            wall = support_roof_outlines.offset_default(-support_roof_line_width / 2);
            infill_outline = wall.offset_default(-support_roof_line_width / 2);
        }
        infill_outline = Simplify::from_settings(rs).shape(&infill_outline);

        let mut roof_computation = make_infill(
            pattern,
            zig_zaggify_infill,
            connect_polygons,
            infill_outline,
            current_roof_config.get_line_width(),
            support_roof_line_distance,
            support_roof_overlap,
            infill_multiplier,
            fill_angle,
            gcode_layer.z + current_roof_config.z_offset,
            extra_infill_shift,
            max_resolution,
            max_deviation,
            wall_line_count,
            small_area_width,
            infill_origin,
            skip_stitching,
            fill_gaps,
            connected_zigzags,
            use_endpieces,
            skip_some_zags,
            zag_skip_count,
            pocket_size,
        );
        let mut roof_polygons = Shape::default();
        let mut roof_paths: Vec<VariableWidthLines> = Vec::new();
        let mut roof_lines = OpenLinesSet::default();
        roof_computation.generate(&mut roof_paths, &mut roof_polygons, &mut roof_lines, rs, gcode_layer.get_layer_nr() as i32, SectionType::SUPPORT, None, None, None, &Shape::default());
        if (gcode_layer.get_layer_nr() == 0 && wall.is_empty()) || (gcode_layer.get_layer_nr() > 0 && roof_paths.is_empty() && roof_polygons.is_empty() && roof_lines.is_empty()) {
            return false; // We didn't create any support roof.
        }
        gcode_layer.set_is_inside(false); // going to print stuff outside print object, i.e. support
        if gcode_layer.get_layer_nr() == 0 {
            gcode_layer.add_polygons_by_optimizer(&wall, current_roof_config, rs, PolygonsOpts::default());
        }
        if !roof_polygons.is_empty() {
            let force_comb_retract = false;
            gcode_layer.add_travel(roof_polygons[0][0], force_comb_retract, 0);
            gcode_layer.add_polygons_by_optimizer(&roof_polygons, current_roof_config, rs, PolygonsOpts::default());
        }
        if !roof_paths.is_empty() {
            let retract_before_outer_wall = false;
            let wipe_dist = 0;
            let z_seam_config = ZSeamConfig::new(EZSeamType::SHORTEST, gcode_layer.get_last_planned_position_or_starting_position(), EZSeamCornerPrefType::Z_SEAM_CORNER_PREF_INNER, 0);

            let mut wall_orderer = InsetOrderOptimizer::new(
                gcode_layer.get_layer_nr(),
                rs,
                roof_extruder_nr,
                WallConfigs::uniform(current_roof_config),
                retract_before_outer_wall,
                wipe_dist,
                wipe_dist,
                roof_extruder_nr,
                roof_extruder_nr,
                z_seam_config,
                &roof_paths,
                storage.get_model_bounding_box().flatten().get_middle(),
                Shape::default(),
                false,
                false,
                Shape::default(),
            );
            wall_orderer.add_to_layer(gcode_layer);
        }
        gcode_layer.add_lines_by_optimizer_open(
            &roof_lines,
            current_roof_config,
            if pattern == EFillMethod::ZIG_ZAG { SpaceFillType::PolyLines } else { SpaceFillType::Lines },
            LinesOpts::default(),
        );
        true
    }

    fn add_support_bottoms_to_gcode(&self, storage: &SliceDataStorage, gcode_layer: &mut LayerPlan) -> bool {
        let support_layer = &storage.support.support_layers[gcode_layer.get_layer_nr().max(0) as usize];

        if !storage.support.generated || gcode_layer.get_layer_nr() > storage.support.layer_nr_max_filled_layer as LayerIndex || support_layer.support_bottom.is_empty() {
            return false; // No need to generate support bottoms if there's no support.
        }

        let bottom_extruder_nr = mesh_group_settings().get_extruder_train("support_bottom_extruder_nr").extruder_nr;
        let bottom_extruder = extruder(bottom_extruder_nr);
        let bs = &bottom_extruder.settings;

        let pattern: EFillMethod = bs.get_enum("support_bottom_pattern");
        let mut fill_angle = AngleDegrees::new(0.0);
        if !storage.support.support_bottom_angles.is_empty() {
            // handle negative layer numbers
            let divisor = storage.support.support_bottom_angles.len() as i32;
            let index = (((gcode_layer.get_layer_nr() % divisor as LayerIndex) as i32) + divisor) % divisor;
            fill_angle = storage.support.support_bottom_angles[index as usize];
        }
        let zig_zaggify_infill = pattern == EFillMethod::ZIG_ZAG;
        let connect_polygons = false; // Keep the same as roof, also does make a bit less sense when support infill is < 100% or support walls are set to > 0.
        let support_bottom_overlap = 0; // the bottoms should never be expanded outwards
        let infill_multiplier = 1;
        let extra_infill_shift = 0;
        let wall_line_count = bs.get_usize("support_bottom_wall_count");
        let small_area_width = bs.get_coord("min_even_wall_line_width") * 2; // Maximum width of a region that can still be filled with one wall.

        let infill_origin = Point2LL::default();
        let skip_stitching = false;
        let fill_gaps = true;
        let use_endpieces = true;
        let connected_zigzags = false;
        let skip_some_zags = false;
        let zag_skip_count = 0;
        let pocket_size = 0;
        let max_resolution = bs.get_coord("meshfix_maximum_resolution");
        let max_deviation = bs.get_coord("meshfix_maximum_deviation");

        let support_bottom_line_distance = bs.get_coord("support_bottom_line_distance"); // note: no need to apply initial line width factor; support bottoms cannot exist on the first layer
        let config = gcode_layer.configs_storage.support_bottom_config;
        let mut bottom_computation = make_infill(
            pattern,
            zig_zaggify_infill,
            connect_polygons,
            support_layer.support_bottom.clone(),
            config.get_line_width(),
            support_bottom_line_distance,
            support_bottom_overlap,
            infill_multiplier,
            fill_angle,
            gcode_layer.z,
            extra_infill_shift,
            max_resolution,
            max_deviation,
            wall_line_count,
            small_area_width,
            infill_origin,
            skip_stitching,
            fill_gaps,
            connected_zigzags,
            use_endpieces,
            skip_some_zags,
            zag_skip_count,
            pocket_size,
        );
        let mut bottom_polygons = Shape::default();
        let mut bottom_paths: Vec<VariableWidthLines> = Vec::new();
        let mut bottom_lines = OpenLinesSet::default();
        bottom_computation.generate(&mut bottom_paths, &mut bottom_polygons, &mut bottom_lines, bs, gcode_layer.get_layer_nr() as i32, SectionType::SUPPORT, None, None, None, &Shape::default());
        if bottom_paths.is_empty() && bottom_polygons.is_empty() && bottom_lines.is_empty() {
            return false;
        }
        gcode_layer.set_is_inside(false); // going to print stuff outside print object, i.e. support
        if !bottom_polygons.is_empty() {
            let force_comb_retract = false;
            gcode_layer.add_travel(bottom_polygons[0][0], force_comb_retract, 0);
            gcode_layer.add_polygons_by_optimizer(&bottom_polygons, &config, bs, PolygonsOpts::default());
        }
        if !bottom_paths.is_empty() {
            let retract_before_outer_wall = false;
            let wipe_dist = 0;
            let z_seam_config = ZSeamConfig::new(EZSeamType::SHORTEST, gcode_layer.get_last_planned_position_or_starting_position(), EZSeamCornerPrefType::Z_SEAM_CORNER_PREF_INNER, 0);

            let mut wall_orderer = InsetOrderOptimizer::new(
                gcode_layer.get_layer_nr(),
                bs,
                bottom_extruder_nr,
                WallConfigs::uniform(&config),
                retract_before_outer_wall,
                wipe_dist,
                wipe_dist,
                bottom_extruder_nr,
                bottom_extruder_nr,
                z_seam_config,
                &bottom_paths,
                storage.get_model_bounding_box().flatten().get_middle(),
                Shape::default(),
                false,
                false,
                Shape::default(),
            );
            wall_orderer.add_to_layer(gcode_layer);
        }
        gcode_layer.add_lines_by_optimizer_open(
            &bottom_lines,
            &config,
            if pattern == EFillMethod::ZIG_ZAG { SpaceFillType::PolyLines } else { SpaceFillType::Lines },
            LinesOpts::default(),
        );
        true
    }

    fn set_extruder_add_prime(&self, storage: &SliceDataStorage, gcode_layer: &mut LayerPlan, extruder_nr: usize, append_to_prime_tower: bool) {
        let previous_extruder = gcode_layer.get_extruder();
        let extruder_changed = gcode_layer.set_extruder(extruder_nr);

        if extruder_changed {
            if self.extruder_prime_layer_nr[extruder_nr] == gcode_layer.get_layer_nr() {
                let train = extruder(extruder_nr);

                // We always prime an extruder, but whether it will be a prime blob/poop depends on if prime blob is enabled.
                // This is decided in GCodeExport::writePrimeTrain().
                if train.settings.get_bool("prime_blob_enable") {
                    // Don't travel to the prime-blob position if not enabled though.
                    let prime_pos_is_abs = train.settings.get_bool("extruder_prime_pos_abs");
                    let prime_pos = pt(train.settings.get_coord("extruder_prime_pos_x"), train.settings.get_coord("extruder_prime_pos_y"));
                    let target = if prime_pos_is_abs { prime_pos } else { gcode_layer.get_last_planned_position_or_starting_position() + prime_pos };
                    gcode_layer.add_travel(target, false, 0);
                    gcode_layer.plan_prime(10.0);
                } else {
                    // Otherwise still prime, but don't do any other travels.
                    gcode_layer.plan_prime(0.0);
                }
            }

            if !gcode_layer.get_skirt_brim_is_planned(extruder_nr) {
                let layer_nr = gcode_layer.get_layer_nr();
                self.process_skirt_brim(storage, gcode_layer, extruder_nr, layer_nr);
            }
        }

        if append_to_prime_tower {
            self.add_prime_tower(storage, gcode_layer, previous_extruder);
        }
    }

    fn add_prime_tower(&self, storage: &SliceDataStorage, gcode_layer: &mut LayerPlan, prev_extruder: usize) {
        let Some(prime_tower) = storage.prime_tower.as_ref() else {
            return;
        };

        let layer_nr = gcode_layer.get_layer_nr();
        let extruder_order = self.extruder_order_per_layer.get(layer_nr);
        let new_extruder = gcode_layer.get_extruder();
        prime_tower.add_to_gcode(storage, gcode_layer, &extruder_order, prev_extruder, new_extruder);
    }

    /// Finish the G-code: end code, header, temperatures.
    pub fn finalize(&mut self) {
        let mesh_group_settings = mesh_group_settings();

        // In one-at-a-time mode, raise the nozzle to avoid collision with already printed objects during final travel
        if mesh_group_settings.get_str("print_sequence") == "one_at_a_time" {
            self.gcode.set_z((self.max_object_height + mm2int(5.0)) as i32);
            communication().send_current_position(self.gcode.get_position());
            let speed = extruder(self.gcode.get_extruder_nr()).settings.get_velocity("speed_travel");
            let xy = self.gcode.get_position_xy();
            self.gcode.write_travel_2d(xy, speed);
        }
        // Write the current extruder's end G-code
        let scene = scene();
        let extruder_settings = &scene.extruders[self.gcode.get_extruder_nr()].settings;
        let extruder_end_code = extruder_settings.get_string("machine_extruder_end_code");

        if !extruder_end_code.is_empty() {
            self.gcode.write_code_with_absolute_extrusion(&extruder_end_code);
        }

        if mesh_group_settings.get_bool("machine_heated_bed") {
            self.gcode.write_bed_temperature_command(0.0, false); // Cool down the bed (M140).
            // Nozzles are cooled down automatically after the last time they are used (which might be earlier than the end of the print).
        }
        if mesh_group_settings.get_bool("machine_heated_build_volume") && mesh_group_settings.get_temperature("build_volume_temperature") != 0.0 {
            self.gcode.write_build_volume_temperature_command(0.0, false); // Cool down the build volume.
        }

        let print_time = Duration::new(self.gcode.get_sum_total_print_times());
        let mut filament_used: Vec<f64> = Vec::new();
        let mut material_ids: Vec<String> = Vec::new();
        let mut extruder_is_used: Vec<bool> = Vec::new();
        for extruder_nr in 0..scene.extruders.len() {
            filament_used.push(self.gcode.get_total_filament_used(extruder_nr));
            material_ids.push(scene.extruders[extruder_nr].settings.get_string("material_guid"));
            extruder_is_used.push(self.gcode.get_extruder_is_used(extruder_nr));
        }
        let prefix = self.gcode.get_file_header(&extruder_is_used, Some(print_time), &filament_used, &material_ids);
        if !communication().is_sequential() {
            communication().send_gcode_prefix(&prefix);
            communication().send_slice_uuid(&self.slice_uuid);
        } else {
            log_info!("Gcode header after slicing: {}", prefix);
        }
        if mesh_group_settings.get_bool("acceleration_enabled") {
            self.gcode.write_print_acceleration(mesh_group_settings.get_acceleration("machine_acceleration"));
            self.gcode.write_travel_acceleration(mesh_group_settings.get_acceleration("machine_acceleration"));
        }
        if mesh_group_settings.get_bool("jerk_enabled") {
            self.gcode.write_jerk(mesh_group_settings.get_velocity("machine_max_jerk_xy"));
        }

        let end_gcode = mesh_group_settings.get_string("machine_end_gcode");

        if !end_gcode.is_empty() && mesh_group_settings.get_bool("relative_extrusion") {
            self.gcode.write_extrusion_mode(false); // ensure absolute extrusion mode is set before the end gcode
        }
        self.gcode.finalize(&end_gcode);

        // set extrusion mode back to "normal"
        self.gcode.reset_extrusion_mode();

        for e in 0..scene.extruders.len() {
            self.gcode.write_temperature_command(e, 0.0, false, false);
        }

        self.gcode.write_comment("End of Gcode");
        self.gcode.flush_output_stream();
    }
}

/// `Polyline::segmentsCount()` for a polyline of any type.
fn segments_count(line: &PolylinePtr) -> usize {
    match &**line {
        AnyPolyline::Open(l) => l.segments_count(),
        AnyPolyline::Closed(l) => l.segments_count(),
        AnyPolyline::Polygon(l) => l.segments_count(),
    }
}
