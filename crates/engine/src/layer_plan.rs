//! `LayerPlan.h/.cpp`: the moves planned for a layer, with combing, retraction decisions, time
//! estimates and the final translation of the plan into G-code.

use crate::application::{communication, current_slice};
use crate::bridge::wall_segment_uses_bridging;
use crate::extruder_plan::ExtruderPlan;
use crate::extruder_train::ExtruderTrain;
use crate::gcode_export::{GCodeExport, TravelAntiOozing, ZHopAntiOozing};
use crate::geometry::mended_shape::MendedShape;
use crate::geometry::mixed_lines_set::{AnyPolyline, MixedLinesSet};
use crate::geometry::point2ll::{lerp as lerp2, normal, pt, scale_f64, shorter_then, v_size, v_size2, v_size_mm, Point2LL};
use crate::geometry::point3ll::{lerp as lerp3, sub_2d_3d, Point3LL};
use crate::geometry::{ClosedPolyline, OpenLinesSet, OpenPolyline, Polygon, Polyline, Shape};
use crate::path_adapter::PathAdapter;
use crate::path_order_monotonic::PathOrderMonotonic;
use crate::path_order_optimizer::{PathOrderOptimizer, PathOrdering, ZSeamConfig};
use crate::path_planning::comb::Comb;
use crate::path_planning::line_polygons_crossings::CombPaths;
use crate::path_planning::{FanSpeedLayerTimeSettings, GCodePath, GCodePathConfig, SpaceFillType};
use crate::print_feature::PrintFeatureType;
use crate::raft::{Raft, RaftLayerType};
use crate::retraction_config::RetractionAndWipeConfig;
use crate::settings::enums::CombingMode;
use crate::settings::path_config_storage::PathConfigStorage;
use crate::settings::settings::{Settings, MAX_EXTRUDERS};
use crate::settings::types::{Acceleration, AngleRadians, Duration, LayerIndex, Ratio, Velocity};
use crate::slice_data_storage::{SliceDataStorage, SliceLayerPart};
use crate::utils::aabb::AABB;
use crate::utils::coord::{int2mm, mm2int, Coord};
use crate::utils::extrusion_line::{ExtrusionJunction, ExtrusionLine};
use crate::utils::linear_alg_2d as la;
use crate::utils::math::{round_divide, round_up_divide};
use crate::utils::polygon_utils::{PolygonUtils, NO_PENALTY};
use crate::utils::simplify::Simplify;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;
use crate::texture_data_provider::TextureDataProvider;

/// in uM. Generated lines shorter than this may be discarded
const MINIMUM_LINE_LENGTH: Coord = 5;
const MINIMUM_SQUARED_LINE_LENGTH: Coord = MINIMUM_LINE_LENGTH * MINIMUM_LINE_LENGTH;

/// limit to accumulated "volume" of non-bridge lines which is proportional to distance x extrusion rate
const MAX_NON_BRIDGE_LINE_VOLUME: f64 = 100000.0; // MM2INT(100)

/// `LayerPlan::OverhangMask`
#[derive(Clone, Debug, Default)]
pub struct OverhangMask {
    pub supported_region: Shape,
    pub speed_ratio: Ratio,
}

/// Indicates how coasting should be processed on the given path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum ApplyCoasting {
    #[default]
    NoCoasting,
    CoastEntirePath,
    PartialCoasting,
}

#[derive(Clone, Copy, Debug, Default)]
struct PathCoasting {
    apply_coasting: ApplyCoasting,
    coasting_start_index: usize,
    coasting_start_pos: Point3LL,
}

#[derive(Clone, Copy, Debug, Default)]
struct TravelDurations {
    z_hop: Duration,
    travel: Duration,
}

#[derive(Clone, Copy, Debug)]
struct AntiOozeSettings {
    distance: f64,
    speed: Velocity,
    during_travel_ratio: Ratio,
}

#[derive(Clone, Copy, Debug, Default)]
struct AntiOozeIntermediateAmounts {
    actual_during_travel_ratio: Ratio,
    total_expected_duration: Duration,
    expected_duration_during_travel: Duration,
    expected_amount_during_travel: f64,
    actual_amount_during_travel: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TravelRetractionState {
    None,
    Retracting,
    Travelling,
    Priming,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CombBoundary {
    Minimum,
    Preferred,
}

/// A reference to a line of any type, replacing `const Polyline*` with its dynamic type.
#[derive(Clone, Copy)]
pub enum LineRef<'l> {
    Open(&'l OpenPolyline),
    Closed(&'l ClosedPolyline),
    Polygon(&'l Polygon),
}

impl<'l> LineRef<'l> {
    pub fn points(&self) -> &'l Vec<Point2LL> {
        match self {
            LineRef::Open(l) => &l.points,
            LineRef::Closed(l) => &l.points,
            LineRef::Polygon(l) => &l.points,
        }
    }
    fn is_valid(&self) -> bool {
        match self {
            LineRef::Open(l) => l.is_valid(),
            LineRef::Closed(l) => l.is_valid(),
            LineRef::Polygon(l) => l.is_valid(),
        }
    }
    fn length(&self) -> Coord {
        match self {
            LineRef::Open(l) => l.length(),
            LineRef::Closed(l) => l.length(),
            LineRef::Polygon(l) => l.length(),
        }
    }
    fn is_open(&self) -> bool {
        matches!(self, LineRef::Open(_))
    }
}

/// Optional arguments of `addLinesByOptimizer`.
#[derive(Clone, Copy)]
pub struct LinesOpts<'r> {
    pub enable_travel_optimization: bool,
    pub wipe_dist: Coord,
    pub flow_ratio: Ratio,
    pub near_start_location: Option<Point2LL>,
    pub fan_speed: f64,
    pub reverse_print_direction: bool,
    pub order_requirements: &'r [(usize, usize)],
    pub extra_inwards_start_move_length: Coord,
    pub extra_inwards_end_move_length: Coord,
    pub extra_inwards_move_contour: Option<&'r MendedShape>,
}

impl Default for LinesOpts<'_> {
    fn default() -> Self {
        LinesOpts {
            enable_travel_optimization: false,
            wipe_dist: 0,
            flow_ratio: 1.0,
            near_start_location: None,
            fan_speed: GCodePathConfig::FAN_SPEED_DEFAULT,
            reverse_print_direction: false,
            order_requirements: &[],
            extra_inwards_start_move_length: 0,
            extra_inwards_end_move_length: 0,
            extra_inwards_move_contour: None,
        }
    }
}

/// Optional arguments of `addPolygonsByOptimizer`.
#[derive(Clone, Copy)]
pub struct PolygonsOpts {
    pub z_seam_config: ZSeamConfig,
    pub wall_0_wipe_dist: Coord,
    pub spiralize: bool,
    pub flow_ratio: Ratio,
    pub always_retract: bool,
    pub reverse_order: bool,
    pub start_near_location: Option<Point2LL>,
    pub scarf_seam: bool,
    pub smooth_speed: bool,
}

impl Default for PolygonsOpts {
    fn default() -> Self {
        PolygonsOpts {
            z_seam_config: ZSeamConfig::default(),
            wall_0_wipe_dist: 0,
            spiralize: false,
            flow_ratio: 1.0,
            always_retract: false,
            reverse_order: false,
            start_near_location: None,
            scarf_seam: false,
            smooth_speed: false,
        }
    }
}

/// The parameters given to an `AddExtrusionSegmentFunction`.
#[derive(Clone, Copy)]
struct SegmentArgs {
    segment_index: usize,
    segment_start_ratio: Ratio,
    segment_end_ratio: Ratio,
    start: Point3LL,
    end: Point3LL,
    speed_factor: Ratio,
    flow_ratio: Ratio,
    line_width_ratio: Ratio,
    distance_to_bridge_start: Coord,
}

pub struct LayerPlan<'a> {
    /// The line configs for this layer for each feature type
    pub configs_storage: PathConfigStorage,
    pub z: Coord,
    pub final_travel_z: Coord,

    storage: &'a SliceDataStorage,
    pub(crate) layer_nr: LayerIndex,
    is_initial_layer: bool,
    layer_type: RaftLayerType,
    layer_thickness: Coord,

    layer_start_pos_per_extruder: Vec<Point3LL>,
    has_prime_tower_planned_per_extruder: Vec<bool>,
    pub(crate) last_planned_position: Option<Point3LL>,

    /// The mesh of the last planned move (index into `storage.meshes`).
    current_mesh: Option<usize>,

    skirt_brim_is_processed: [bool; MAX_EXTRUDERS],

    /// should always contain at least one ExtruderPlan
    pub(crate) extruder_plans: Vec<ExtruderPlan>,

    last_extruder_previous_layer: usize,
    /// The extruder for which a move has most recently been planned.
    last_planned_extruder: usize,

    first_travel_destination: Option<Point2LL>,
    first_travel_destination_is_inside: bool,
    pub(crate) first_extrusion_acc_jerk: Option<(Acceleration, Velocity)>,
    pub(crate) next_layer_acc_jerk: Option<(Acceleration, Velocity)>,
    was_inside: bool,
    is_inside: bool,
    comb_boundary_minimum: Shape,
    comb_boundary_preferred: Shape,
    comb: Option<Box<Comb>>,
    #[allow(dead_code)]
    comb_move_inside_distance: Coord,
    bridge_wall_mask: Shape,
    bridge_wall_mask_bb: AABB,
    overhang_masks: Vec<OverhangMask>,
    seam_overhang_mask: Shape,
    roofing_mask: Shape,
    flooring_mask: Shape,

    currently_overhanging: bool,
    current_overhang_length: Coord,
    max_overhang_length: Coord,

    min_layer_time_used: bool,

    /// Infill lines generated for this layer, per mesh index
    infill_lines: BTreeMap<usize, MixedLinesSet>,

    fan_speed_layer_time_settings_per_extruder: Vec<FanSpeedLayerTimeSettings>,
}

#[inline]
fn extruders() -> &'static [ExtruderTrain] {
    &current_slice().scene.extruders
}

#[inline]
fn mesh_group_settings() -> &'static Settings {
    &current_slice().scene.current_mesh_group().settings
}

#[inline]
fn max_c(a: Coord, b: Coord) -> Coord {
    if a < b {
        b
    } else {
        a
    }
}

#[inline]
fn min_f(a: f64, b: f64) -> f64 {
    if b < a {
        b
    } else {
        a
    }
}

#[inline]
fn max_f(a: f64, b: f64) -> f64 {
    if a < b {
        b
    } else {
        a
    }
}

/// `Point3LL * float` (`std::llround(static_cast<float>(x) * i)`).
#[inline]
fn scale3_f32(p: Point3LL, i: f32) -> Point3LL {
    Point3LL::new(((p.x as f32) * i).round() as Coord, ((p.y as f32) * i).round() as Coord, ((p.z as f32) * i).round() as Coord)
}

/// The `point_index` lambda: an `int` index brought into `[0, size)`.
#[inline]
fn wrap_index(base_index: i32, size: usize) -> usize {
    let mut index = base_index;
    while index < 0 {
        index = index.wrapping_add(size as i32);
    }
    (index as usize) % size
}

impl<'a> LayerPlan<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        storage: &'a SliceDataStorage,
        layer_nr: LayerIndex,
        z: Coord,
        layer_thickness: Coord,
        start_extruder: usize,
        fan_speed_layer_time_settings_per_extruder: &[FanSpeedLayerTimeSettings],
        comb_boundary_offset: Coord,
        comb_move_inside_distance: Coord,
        travel_avoid_distance: Coord,
    ) -> Self {
        let configs_storage = PathConfigStorage::new(storage, layer_nr, layer_thickness);
        let layer_type = Raft::get_layer_type(layer_nr);
        let mut plan = LayerPlan {
            configs_storage,
            z,
            final_travel_z: z,
            storage,
            layer_nr,
            is_initial_layer: layer_nr == -(Raft::get_total_extra_layers() as LayerIndex),
            layer_type,
            layer_thickness,
            layer_start_pos_per_extruder: Vec::new(),
            has_prime_tower_planned_per_extruder: vec![false; extruders().len()],
            last_planned_position: None,
            current_mesh: None,
            skirt_brim_is_processed: [false; MAX_EXTRUDERS],
            extruder_plans: Vec::new(),
            last_extruder_previous_layer: start_extruder,
            last_planned_extruder: start_extruder,
            first_travel_destination: None,
            first_travel_destination_is_inside: false,
            first_extrusion_acc_jerk: None,
            next_layer_acc_jerk: None,
            was_inside: true,
            is_inside: false,
            comb_boundary_minimum: Shape::default(),
            comb_boundary_preferred: Shape::default(),
            comb: None,
            comb_move_inside_distance,
            bridge_wall_mask: Shape::default(),
            bridge_wall_mask_bb: AABB::default(),
            overhang_masks: Vec::new(),
            seam_overhang_mask: Shape::default(),
            roofing_mask: Shape::default(),
            flooring_mask: Shape::default(),
            currently_overhanging: false,
            current_overhang_length: 0,
            max_overhang_length: 0,
            min_layer_time_used: false,
            infill_lines: BTreeMap::new(),
            fan_speed_layer_time_settings_per_extruder: fan_speed_layer_time_settings_per_extruder.to_vec(),
        };
        plan.comb_boundary_minimum = plan.compute_comb_boundary(CombBoundary::Minimum);
        plan.comb_boundary_preferred = plan.compute_comb_boundary(CombBoundary::Preferred);

        let current_extruder = start_extruder;
        let local_settings = mesh_group_settings();
        if local_settings.get_enum::<CombingMode>("retraction_combing") != CombingMode::OFF && local_settings.get_coord("retraction_combing_avoid_distance") > 0 {
            plan.comb = Some(Box::new(Comb::new(
                storage,
                layer_nr,
                &plan.comb_boundary_minimum,
                &plan.comb_boundary_preferred,
                comb_boundary_offset,
                travel_avoid_distance,
                comb_move_inside_distance,
            )));
        }
        for extruder in extruders() {
            plan.layer_start_pos_per_extruder.push(Point3LL::new(extruder.settings.get_coord("layer_start_x"), extruder.settings.get_coord("layer_start_y"), 0));
        }
        let is_raft_layer = matches!(layer_type, RaftLayerType::RaftBase | RaftLayerType::RaftInterface | RaftLayerType::RaftSurface);
        plan.extruder_plans.push(ExtruderPlan::new(
            current_extruder,
            layer_nr,
            plan.is_initial_layer,
            is_raft_layer,
            layer_thickness,
            fan_speed_layer_time_settings_per_extruder[current_extruder],
            storage.retraction_wipe_config_per_extruder[current_extruder].retraction_config,
        ));
        plan
    }

    pub fn storage(&self) -> &'a SliceDataStorage {
        self.storage
    }

    /// Either create a new path with the given config or return the last path if it already had that config.
    #[allow(clippy::too_many_arguments)]
    fn get_latest_path_with_config(
        &mut self,
        config: &GCodePathConfig,
        space_fill_type: SpaceFillType,
        z_offset: Coord,
        flow: Ratio,
        width_factor: Ratio,
        spiralize: bool,
        speed_factor: Ratio,
    ) -> usize {
        let current_mesh = self.current_mesh;
        let paths = &mut self.extruder_plans.last_mut().unwrap().paths;
        if let Some(back) = paths.last() {
            if back.config == *config
                && !back.done
                && back.flow == flow
                && back.width_factor == width_factor
                && back.speed_factor == speed_factor
                && back.z_offset == z_offset
                && back.mesh == current_mesh
            {
                return paths.len() - 1;
            }
        }
        paths.push(GCodePath {
            z_offset,
            config: *config,
            mesh: current_mesh,
            space_fill_type,
            flow,
            width_factor,
            spiralize,
            speed_factor,
            ..Default::default()
        });
        paths.len() - 1
    }

    /// Access a path of the last extruder plan.
    pub fn path_mut(&mut self, idx: usize) -> &mut GCodePath {
        &mut self.extruder_plans.last_mut().unwrap().paths[idx]
    }

    pub fn get_comb_boundary_inside(&self) -> &Shape {
        &self.comb_boundary_preferred
    }

    /// Force LayerPlan::getLatestPathWithConfig to return a new path.
    pub fn force_new_path_start(&mut self) {
        let paths = &mut self.extruder_plans.last_mut().unwrap().paths;
        if let Some(last) = paths.last_mut() {
            last.done = true;
        }
    }

    pub fn get_last_planned_extruder_train(&self) -> &'static ExtruderTrain {
        &extruders()[self.last_planned_extruder]
    }

    fn compute_comb_boundary(&self, boundary_type: CombBoundary) -> Shape {
        let mut comb_boundary = Shape::default();
        let mesh_combing_mode: CombingMode = mesh_group_settings().get_enum("retraction_combing");
        if mesh_combing_mode != CombingMode::OFF && (self.layer_nr >= 0 || mesh_combing_mode != CombingMode::NO_SKIN) {
            match self.layer_type {
                RaftLayerType::RaftBase => comb_boundary = self.storage.raft_base_outline.offset_default(100),
                RaftLayerType::RaftInterface => comb_boundary = self.storage.raft_interface_outline.offset_default(100),
                RaftLayerType::RaftSurface => comb_boundary = self.storage.raft_surface_outline.offset_default(100),
                RaftLayerType::Airgap => {}
                RaftLayerType::Model => {
                    for mesh in self.storage.meshes.iter() {
                        let layer = &mesh.layers[self.layer_nr as usize];
                        let settings = mesh.settings();
                        if settings.get_bool("infill_mesh") || settings.get_bool("anti_overhang_mesh") {
                            continue;
                        }

                        const EXTRA_OFFSET: Coord = 10;
                        let offset = match boundary_type {
                            CombBoundary::Minimum => -(settings.get_coord("machine_nozzle_size") / 2 + settings.get_coord("wall_line_width_0") / 2 + EXTRA_OFFSET),
                            CombBoundary::Preferred => {
                                -(settings.get_coord("retraction_combing_avoid_distance") + settings.get_coord("wall_line_width_0") / 2 + EXTRA_OFFSET)
                            }
                        };

                        let combing_mode: CombingMode = settings.get_enum("retraction_combing");
                        for part in layer.parts.iter() {
                            let mut part_combing_boundary;
                            if combing_mode == CombingMode::INFILL {
                                part_combing_boundary = part.infill_area.clone();
                            } else {
                                part_combing_boundary = part.outline.offset_default(10).offset_default(offset - 10);
                                if combing_mode == CombingMode::NO_SKIN {
                                    part_combing_boundary = part_combing_boundary.difference(&part.inner_area.difference(&part.infill_area));
                                } else if combing_mode == CombingMode::NO_OUTER_SURFACES {
                                    for outer_surface_part in layer.parts.iter() {
                                        part_combing_boundary = part_combing_boundary.difference(&outer_surface_part.top_most_surface);
                                        part_combing_boundary = part_combing_boundary.difference(&outer_surface_part.bottom_most_surface);
                                    }
                                }
                            }
                            comb_boundary.push_all_owned(part_combing_boundary);
                        }
                    }
                }
            }
        }
        comb_boundary
    }

    pub fn set_is_inside(&mut self, is_inside: bool) {
        self.is_inside = is_inside;
    }

    fn is_raft_layer(&self) -> bool {
        matches!(self.layer_type, RaftLayerType::RaftBase | RaftLayerType::RaftInterface | RaftLayerType::RaftSurface)
    }

    /// Plan a switch to a new extruder. Returns whether the extruder has changed.
    pub fn set_extruder(&mut self, extruder_nr: usize) -> bool {
        if extruder_nr == self.get_extruder() {
            return false;
        }
        self.set_is_inside(false);
        {
            // handle end position of the prev extruder
            let extruder = self.get_last_planned_extruder_train();
            let end_pos_absolute = extruder.settings.get_bool("machine_extruder_end_pos_abs");
            let mut end_pos = pt(extruder.settings.get_coord("machine_extruder_end_pos_x"), extruder.settings.get_coord("machine_extruder_end_pos_y"));
            if !end_pos_absolute {
                end_pos = end_pos + self.get_last_planned_position_or_starting_position();
            } else {
                let extruder_offset = pt(extruder.settings.get_coord("machine_nozzle_offset_x"), extruder.settings.get_coord("machine_nozzle_offset_y"));
                end_pos = end_pos + extruder_offset;
            }
            if end_pos_absolute || self.last_planned_position.is_some() {
                let path = self.add_travel(end_pos, false, 0);
                self.path_mut(path).retract_for_nozzle_switch = true;
            }
        }
        {
            let back = self.extruder_plans.last_mut().unwrap();
            if back.paths.is_empty() && back.inserts.is_empty() {
                // first extruder plan in a layer might be empty, cause it is made with the last extruder planned in the previous layer
                back.extruder_nr = extruder_nr;
            }
        }

        let is_raft_layer = self.is_raft_layer();
        self.extruder_plans.push(ExtruderPlan::new(
            extruder_nr,
            self.layer_nr,
            self.is_initial_layer,
            is_raft_layer,
            self.layer_thickness,
            self.fan_speed_layer_time_settings_per_extruder[extruder_nr],
            self.storage.retraction_wipe_config_per_extruder[extruder_nr].retraction_config,
        ));
        self.last_planned_extruder = extruder_nr;

        {
            // handle starting pos of the new extruder
            let extruder = self.get_last_planned_extruder_train();
            let start_pos_absolute = extruder.settings.get_bool("machine_extruder_start_pos_abs");
            let mut start_pos = Point3LL::new(extruder.settings.get_coord("machine_extruder_start_pos_x"), extruder.settings.get_coord("machine_extruder_start_pos_y"), 0);
            if !start_pos_absolute {
                start_pos += self.get_last_planned_position_or_starting_position();
            } else {
                let extruder_offset = Point3LL::new(extruder.settings.get_coord("machine_nozzle_offset_x"), extruder.settings.get_coord("machine_nozzle_offset_y"), 0);
                start_pos += extruder_offset;
            }
            if start_pos_absolute || self.last_planned_position.is_some() {
                self.last_planned_position = Some(start_pos);
            }
        }
        true
    }

    /// Track the currently printing mesh (index into `storage.meshes`).
    pub fn set_mesh(&mut self, mesh: Option<usize>) {
        self.current_mesh = mesh;
    }

    /// Add a travel move to the layer plan to move inside the current layer part by a given distance.
    pub fn move_inside_comb_boundary(&mut self, distance: Coord, part: Option<&SliceLayerPart>, path: Option<usize>) {
        const MAX_DIST2: Coord = 2000 * 2000;
        let mut p = self.get_last_planned_position_or_starting_position();
        if PolygonUtils::move_inside(&self.comb_boundary_preferred, &mut p, distance as i32, MAX_DIST2) != crate::geometry::lines_set::NO_INDEX {
            // Move inside again, so we move out of tight 90deg corners
            PolygonUtils::move_inside(&self.comb_boundary_preferred, &mut p, distance as i32, MAX_DIST2);
            if self.comb_boundary_preferred.inside(p, false) && part.map_or(true, |part| part.outline.inside(p, false)) {
                self.add_travel_simple(p, path);
                // Make sure the that any retraction happens after this move, not before it by starting a new move path.
                self.force_new_path_start();
            }
        }
    }

    pub fn get_prime_tower_is_planned(&self, extruder_nr: usize) -> bool {
        self.has_prime_tower_planned_per_extruder[extruder_nr]
    }

    pub fn set_prime_tower_is_planned(&mut self, extruder_nr: usize) {
        self.has_prime_tower_planned_per_extruder[extruder_nr] = true;
    }

    pub fn get_first_travel_destination_state(&self) -> Option<(Point2LL, bool)> {
        self.first_travel_destination.map(|d| (d, self.first_travel_destination_is_inside))
    }

    /// Travel to a certain point, with all of the procedures necessary to do so.
    /// Returns the index of the travel path in the last extruder plan.
    pub fn add_travel(&mut self, p: Point2LL, force_retract: bool, z_offset: Coord) -> usize {
        let travel_config = self.configs_storage.travel_config_per_extruder[self.get_extruder()];

        let retraction_config = match self.current_mesh {
            Some(m) => self.storage.meshes[m].retraction_wipe_config.retraction_config,
            None => self.storage.retraction_wipe_config_per_extruder[self.get_extruder()].retraction_config,
        };

        let path = self.get_latest_path_with_config(&travel_config, SpaceFillType::None, z_offset, 1.0, 1.0, false, 1.0);

        let mut combed = false;

        let extruder = self.get_last_planned_extruder_train();
        let mesh_or_extruder_settings: &Settings = match self.current_mesh {
            Some(m) => self.storage.meshes[m].settings(),
            None => &extruder.settings,
        };

        let is_first_travel_of_extruder_after_switch =
            self.extruder_plans.last().unwrap().paths.len() == 1 && (self.extruder_plans.len() > 1 || self.last_extruder_previous_layer != self.get_extruder());
        let mut bypass_combing = is_first_travel_of_extruder_after_switch && mesh_or_extruder_settings.get_bool("retraction_hop_after_extruder_switch");

        let is_first_travel_of_layer = self.last_planned_position.is_none();
        let retraction_enable = mesh_or_extruder_settings.get_bool("retraction_enable");
        if is_first_travel_of_layer {
            bypass_combing = true; // first travel move is bogus; it is added after this and the previous layer have been planned in LayerPlanBuffer::addConnectingTravelMove
            self.first_travel_destination = Some(p);
            self.first_travel_destination_is_inside = self.is_inside;
            if self.layer_nr == 0 && retraction_enable && mesh_or_extruder_settings.get_bool("retraction_hop_enabled") {
                let path = self.path_mut(path);
                path.retract = true;
                path.perform_z_hop = true;
            }
            self.force_new_path_start(); // force a new travel path after this first bogus move
        } else if force_retract
            && self.last_planned_position.is_some()
            && !shorter_then(self.last_planned_position.unwrap().to_point2ll() - p, retraction_config.retraction_min_travel_distance)
        {
            // path is not shorter than min travel distance, force a retraction
            let has_comb = self.comb.is_some();
            let hop = mesh_or_extruder_settings.get_bool("retraction_hop_enabled");
            let path = self.path_mut(path);
            path.retract = true;
            if !has_comb {
                path.perform_z_hop = hop;
            }
        }

        if self.comb.is_some() && !bypass_combing {
            let mut comb_paths = CombPaths::default();

            // Divide by 2 to get the radius
            // Multiply by 2 because if two lines start and end points places very close then will be applied combing with retractions. (Ex: for brim)
            let max_distance_ignored = mesh_or_extruder_settings.get_coord("machine_nozzle_tip_outer_diameter") / 2 * 2;

            let mut unretract_before_last_travel_move = false; // Decided when calculating the combing
            let mut do_retracted_combing_move = false; // Decided when calculating the combing
            let perform_z_hops = mesh_or_extruder_settings.get_bool("retraction_hop_enabled");
            let perform_z_hops_only_when_collides = mesh_or_extruder_settings.get_bool("retraction_hop_only_when_collides");
            let start = self.last_planned_position.unwrap().to_point2ll();
            let was_inside = self.was_inside;
            let is_inside = self.is_inside;
            combed = self.comb.as_mut().unwrap().calc(
                perform_z_hops,
                perform_z_hops_only_when_collides,
                extruder,
                start,
                p,
                &mut comb_paths,
                was_inside,
                is_inside,
                max_distance_ignored,
                &mut unretract_before_last_travel_move,
                &mut do_retracted_combing_move,
            );
            if combed {
                let mut retract = self.path_mut(path).retract || ((comb_paths.len() > 1 || do_retracted_combing_move) && retraction_enable);
                if !retract {
                    // check whether we want to retract
                    if comb_paths.through_air {
                        retract = retraction_enable;
                    } else {
                        for comb_path in comb_paths.iter() {
                            // retract when path moves through a boundary
                            if comb_path.cross_boundary {
                                retract = retraction_enable;
                                break;
                            }
                        }
                    }
                }

                let maximum_travel_resolution = mesh_or_extruder_settings.get_coord("meshfix_maximum_travel_resolution");
                let mut distance: Coord = 0;
                let mut last_point = match self.last_planned_position {
                    Some(l) => l.to_point2ll(),
                    None => pt(0, 0),
                };
                let retract_threshold = mesh_or_extruder_settings.get_coord("retraction_combing_max_distance");
                let path_ref = self.path_mut(path);
                for comb_path in comb_paths.iter() {
                    // add all comb paths (don't do anything special for paths which are moving through air)
                    if comb_path.is_empty() {
                        continue;
                    }
                    for &comb_point in comb_path.iter() {
                        if path_ref.points.is_empty()
                            || (*path_ref.points.last().unwrap() - Point3LL::from_2d(comb_point, 0)).v_size2() > maximum_travel_resolution * maximum_travel_resolution
                        {
                            path_ref.points.push(Point3LL::from_2d(comb_point, 0));
                            distance += v_size(last_point - comb_point);
                            last_point = comb_point;
                        }
                    }
                    distance += v_size(last_point - p);
                    path_ref.retract = retract || (retract_threshold > 0 && distance > retract_threshold && retraction_enable);
                    // don't perform a z-hop
                }
                // Whether to unretract before the last travel move of the travel path, which comes before the wall to be printed.
                path_ref.unretract_before_last_travel_move = path_ref.retract && unretract_before_last_travel_move;
            }
        }

        // CURA-6675:
        // Retraction Minimal Travel Distance should work for all travel moves. If the travel move is shorter than the
        // Retraction Minimal Travel Distance, retraction should be disabled.
        if !is_first_travel_of_layer
            && self.last_planned_position.is_some()
            && shorter_then(self.last_planned_position.unwrap().to_point2ll() - p, retraction_config.retraction_min_travel_distance)
        {
            let path = self.path_mut(path);
            path.retract = false;
            path.perform_z_hop = false;
        }

        // no combing? retract only when path is not shorter than minimum travel distance
        if !combed
            && !is_first_travel_of_layer
            && self.last_planned_position.is_some()
            && !shorter_then(self.last_planned_position.unwrap().to_point2ll() - p, retraction_config.retraction_min_travel_distance)
        {
            if self.was_inside {
                // when the previous location was from printing something which is considered inside (not support or prime tower etc)
                // then move inside the printed part, so that we don't ooze on the outer wall while retraction, but on the inside of the print.
                let mut innermost_wall_line_width =
                    mesh_or_extruder_settings.get_coord(if mesh_or_extruder_settings.get_usize("wall_line_count") > 1 { "wall_line_width_x" } else { "wall_line_width_0" });
                if self.layer_nr == 0 {
                    innermost_wall_line_width = (innermost_wall_line_width as f64 * mesh_or_extruder_settings.get_ratio("initial_layer_line_width_factor")) as Coord;
                }
                self.move_inside_comb_boundary(innermost_wall_line_width, None, Some(path));
            }
            let hop = retraction_enable && mesh_or_extruder_settings.get_bool("retraction_hop_enabled");
            let path = self.path_mut(path);
            path.retract = retraction_enable;
            path.perform_z_hop = hop;
        }

        // must start new travel path as retraction can be enabled or not depending on path length, etc.
        self.force_new_path_start();

        let ret = self.add_travel_simple(p, Some(path));
        self.was_inside = self.is_inside;
        ret
    }

    /// Add a travel path to a certain point and retract if needed. No combing is performed.
    pub fn add_travel_simple(&mut self, p: Point2LL, path: Option<usize>) -> usize {
        let is_first_travel_of_layer = self.last_planned_position.is_none();
        if is_first_travel_of_layer {
            // spiralize calls addTravel_simple directly as the first travel move in a layer
            self.first_travel_destination = Some(p);
            self.first_travel_destination_is_inside = self.is_inside;
        }
        let path = match path {
            Some(path) => path,
            None => {
                let config = self.configs_storage.travel_config_per_extruder[self.get_extruder()];
                self.get_latest_path_with_config(&config, SpaceFillType::None, 0, 1.0, 1.0, false, 1.0)
            }
        };
        self.path_mut(path).points.push(Point3LL::from_2d(p, 0));
        self.last_planned_position = Some(Point3LL::from_2d(p, 0));
        path
    }

    /// Plan a prime blob at the current location.
    pub fn plan_prime(&mut self, prime_blob_wipe_length: f64) {
        self.force_new_path_start();
        let target = self.get_last_planned_position_or_starting_position() + pt(0, mm2int(prime_blob_wipe_length));
        let prime_travel = self.add_travel_simple(target, None);
        let path = self.path_mut(prime_travel);
        path.retract = false;
        path.perform_z_hop = false;
        path.perform_prime = true;
        self.force_new_path_start();
    }

    pub fn set_generated_infill_lines(&mut self, mesh_idx: usize, infill_lines: &MixedLinesSet) {
        self.infill_lines.entry(mesh_idx).or_default().push_mixed(infill_lines);
    }

    pub fn get_generated_infill_lines(&self, mesh_idx: usize) -> MixedLinesSet {
        match self.infill_lines.get(&mesh_idx) {
            Some(l) => l.clone(),
            None => MixedLinesSet::default(),
        }
    }

    /// Add an extrusion move to a certain point, optionally with a different flow than the one in the `config`.
    #[allow(clippy::too_many_arguments)]
    pub fn add_extrusion_move(
        &mut self,
        p: Point3LL,
        config: &GCodePathConfig,
        space_fill_type: SpaceFillType,
        flow: Ratio,
        width_factor: Ratio,
        spiralize: bool,
        speed_factor: Ratio,
        fan_speed: f64,
        travel_to_z: bool,
    ) {
        let path = self.get_latest_path_with_config(config, space_fill_type, config.z_offset, flow, width_factor, spiralize, speed_factor);
        let path = self.path_mut(path);
        path.points.push(p);
        path.set_fan_speed(fan_speed);
        path.travel_to_z = travel_to_z;
        let acc_jerk = (path.config.get_acceleration(), path.config.get_jerk());
        if self.first_extrusion_acc_jerk.is_none() {
            self.first_extrusion_acc_jerk = Some(acc_jerk);
        }
        self.last_planned_position = Some(p);
    }

    /// `addExtrusionMove` with the default arguments after `flow`.
    pub fn add_extrusion_move_simple(&mut self, p: Point3LL, config: &GCodePathConfig, space_fill_type: SpaceFillType, flow: Ratio) {
        self.add_extrusion_move(p, config, space_fill_type, flow, 1.0, false, 1.0, GCodePathConfig::FAN_SPEED_DEFAULT, true);
    }

    fn update_is_overhanging(&mut self, target: Point3LL, current_position: Option<Point3LL>, is_overhanging: bool) {
        if is_overhanging != self.currently_overhanging {
            self.max_overhang_length = max_c(self.current_overhang_length, self.max_overhang_length);
            self.current_overhang_length = 0;
        }

        if is_overhanging {
            if let Some(cp) = current_position {
                self.current_overhang_length += (target - cp).v_size();
            }
        }

        self.currently_overhanging = is_overhanging;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_extrusion_move_with_gradual_overhang(
        &mut self,
        p: Point3LL,
        config: &GCodePathConfig,
        space_fill_type: SpaceFillType,
        flow: Ratio,
        width_factor: Ratio,
        spiralize: bool,
        speed_factor: Ratio,
        fan_speed: f64,
        travel_to_z: bool,
    ) {
        let add_extrusion_move = |this: &mut Self, target: Point3LL, speed_region_index: Option<usize>| {
            let overhang_speed_factor = match speed_region_index {
                Some(i) => this.overhang_masks[i].speed_ratio,
                None => 1.0,
            };
            this.add_extrusion_move(target, config, space_fill_type, flow, width_factor, spiralize, speed_factor * overhang_speed_factor, fan_speed, travel_to_z);
        };

        if self.overhang_masks.is_empty() || self.last_planned_position.is_none() {
            // Unable to apply gradual overhanging (probably just disabled), just add the basic extrusion move
            let lpp = self.last_planned_position;
            self.update_is_overhanging(p, lpp, false);
            add_extrusion_move(self, p, None);
            return;
        }

        // First, find the speed region where the segment starts
        let start = self.last_planned_position.unwrap();
        let start_flat = start.to_point2ll();
        let mut actual_speed_region_index = self.overhang_masks.len() - 1; // Default to last region, which is infinity and beyond
        for (index, overhang_region) in self.overhang_masks[..self.overhang_masks.len() - 1].iter().enumerate() {
            if overhang_region.supported_region.inside(start_flat, true) {
                actual_speed_region_index = index;
                break;
            }
        }

        // Pre-calculate the intersections of the segment with all regions (except last one, you cannot intersect an infinite plane)
        let end = p;
        let end_flat = end.to_point2ll();
        let vector = end - start;
        let mut speed_regions_intersections: Vec<Vec<f32>> = Vec::with_capacity(self.overhang_masks.len() - 1);
        for overhang_region in self.overhang_masks[..self.overhang_masks.len() - 1].iter() {
            let mut intersections = overhang_region.supported_region.intersections_with_segment(start_flat, end_flat);
            cppstd::stable_sort_by(&mut intersections, |a, b| a < b);
            speed_regions_intersections.push(intersections);
        }

        let remove_previous_intersections = |speed_regions_intersections: &mut Vec<Vec<f32>>, current_intersection: f32| {
            for intersections in speed_regions_intersections.iter_mut() {
                let pos = intersections.iter().position(|&next| next > current_intersection).unwrap_or(intersections.len());
                intersections.drain(0..pos);
            }
        };

        #[derive(Clone, Copy)]
        struct SegmentExtrusionMove {
            position: Point3LL,
            speed_region_index: usize,
        }

        let mut extrusion_moves: Vec<SegmentExtrusionMove> = Vec::new();

        // Now move along segment and split it where we cross speed regions
        loop {
            let first_intersection_current_region: Option<f32> = if actual_speed_region_index < speed_regions_intersections.len() {
                speed_regions_intersections[actual_speed_region_index].first().copied()
            } else {
                None
            };
            let first_intersection_inner_region: Option<f32> =
                if actual_speed_region_index > 0 { speed_regions_intersections[actual_speed_region_index - 1].first().copied() } else { None };

            if first_intersection_current_region.is_some() || first_intersection_inner_region.is_some() {
                let intersection_parameter;
                let next_speed_region_index;

                if first_intersection_current_region.is_some()
                    && (first_intersection_inner_region.is_none() || first_intersection_inner_region.unwrap() > first_intersection_current_region.unwrap())
                {
                    // We crossed the border of the current region, which means we are getting out of it to an outer region
                    intersection_parameter = first_intersection_current_region.unwrap();
                    next_speed_region_index = actual_speed_region_index + 1;
                } else {
                    // We crossed the border of the inner region, which means we are getting inside of it
                    intersection_parameter = first_intersection_inner_region.unwrap();
                    next_speed_region_index = actual_speed_region_index - 1;
                }

                // Move to intersection at current region speed
                let split_position = start + scale3_f32(vector, intersection_parameter);
                extrusion_moves.push(SegmentExtrusionMove { position: split_position, speed_region_index: actual_speed_region_index });

                // Prepare for next move in different region
                actual_speed_region_index = next_speed_region_index;
                remove_previous_intersections(&mut speed_regions_intersections, intersection_parameter);
            } else {
                // We cross no border, which means we can reach the end of the segment within the current speed region, so we are done
                extrusion_moves.push(SegmentExtrusionMove { position: p, speed_region_index: actual_speed_region_index });
                break;
            }
        }

        // Filter out micro-segments
        let mut extrusion_moves_filtered: Vec<SegmentExtrusionMove> = Vec::with_capacity(extrusion_moves.len());
        let mut current_position = start;
        for extrusion_move in extrusion_moves[..extrusion_moves.len() - 1].iter() {
            if (extrusion_move.position - current_position).v_size2() >= MINIMUM_SQUARED_LINE_LENGTH {
                extrusion_moves_filtered.push(*extrusion_move);
            }
            current_position = extrusion_move.position;
        }

        let last_move = *extrusion_moves.last().unwrap();
        if extrusion_moves_filtered.is_empty() || (last_move.position - current_position).v_size2() >= MINIMUM_SQUARED_LINE_LENGTH {
            extrusion_moves_filtered.push(last_move);
        } else {
            extrusion_moves_filtered.last_mut().unwrap().position = last_move.position;
        }

        // Calculate max consecutive overhanging segment length
        current_position = start;
        for extrusion_move in extrusion_moves_filtered.iter() {
            let is_overhanging = extrusion_move.speed_region_index > 0;
            self.update_is_overhanging(extrusion_move.position, Some(current_position), is_overhanging);
            current_position = extrusion_move.position;
        }

        // Merge consecutive sub-segments that in the end have the same speed
        let mut extrusion_moves_merged: Vec<SegmentExtrusionMove> = Vec::with_capacity(extrusion_moves_filtered.len());
        extrusion_moves_merged.push(extrusion_moves_filtered[0]);

        for extrusion_move in extrusion_moves_filtered[1..].iter() {
            let previous_speed_factor = self.overhang_masks[extrusion_moves_merged.last().unwrap().speed_region_index].speed_ratio;
            let next_speed_factor = self.overhang_masks[extrusion_move.speed_region_index].speed_ratio;

            if next_speed_factor == previous_speed_factor {
                extrusion_moves_merged.last_mut().unwrap().position = extrusion_move.position;
            } else {
                extrusion_moves_merged.push(*extrusion_move);
            }
        }

        // Finally, add extrusion moves
        for extrusion_move in extrusion_moves_merged.iter() {
            add_extrusion_move(self, extrusion_move.position, Some(extrusion_move.speed_region_index));
        }
    }

    fn add_wipe_travel(&mut self, path: &PathAdapter, wipe_distance: Coord, backwards: bool, start_index: usize, last_path_position: Point2LL) {
        if path.size() >= 2 && wipe_distance > 0 {
            let direction: i64 = if backwards { -1 } else { 1 };
            let mut p0 = last_path_position;
            let mut distance_traversed: i32 = 0;
            let mut index = start_index;
            let size = path.size();
            while (distance_traversed as Coord) < wipe_distance {
                index = ((index as u64).wrapping_add(direction as u64).wrapping_add(size as u64) % size as u64) as usize;
                if index == start_index && distance_traversed == 0 {
                    // Wall has a total circumference of 0. This loop would never end.
                    break;
                }

                let p1 = path.point_at(index);
                let p0p1_dist = v_size(p1 - p0) as i32;
                if (distance_traversed as Coord) + (p0p1_dist as Coord) >= wipe_distance {
                    let vector = p1 - p0;
                    let half_way = p0 + normal(vector, wipe_distance - distance_traversed as Coord);
                    self.add_travel_simple(half_way, None);
                } else {
                    self.add_travel_simple(p1, None);
                }

                distance_traversed = distance_traversed.wrapping_add(p0p1_dist);
                p0 = p1;
            }

            self.force_new_path_start();
        }
    }

    /// Add polygon to the gcode starting at vertex `start_idx`.
    #[allow(clippy::too_many_arguments)]
    pub fn add_polygon(
        &mut self,
        polygon: &[Point2LL],
        start_idx: i32,
        backwards: bool,
        settings: &Settings,
        config: &GCodePathConfig,
        wall_0_wipe_dist: Coord,
        spiralize: bool,
        flow_ratio: Ratio,
        always_retract: bool,
        scarf_seam: bool,
        smooth_speed: bool,
    ) {
        let is_closed = true;
        let is_candidate_small_feature = false;
        let path_adapter = PathAdapter::Polygon(polygon, config.get_line_width());

        let p0 = polygon[start_idx as usize];
        self.add_travel(p0, always_retract, config.z_offset);

        let config_copy = *config;
        let add_wall_result = self.add_wall_with_scarf_seam(
            &path_adapter,
            start_idx as usize,
            settings,
            config,
            flow_ratio,
            always_retract,
            is_closed,
            backwards,
            is_candidate_small_feature,
            scarf_seam,
            smooth_speed,
            &mut |this: &mut LayerPlan, _wall: &PathAdapter, args: SegmentArgs| {
                let fan_speed = GCodePathConfig::FAN_SPEED_DEFAULT;
                let travel_to_z = false;
                this.add_extrusion_move(
                    args.end,
                    &config_copy,
                    SpaceFillType::Polygons,
                    args.flow_ratio,
                    args.line_width_ratio,
                    spiralize,
                    args.speed_factor,
                    fan_speed,
                    travel_to_z,
                );
            },
        );

        if polygon.len() > 2 {
            self.add_wipe_travel(&path_adapter, wall_0_wipe_dist, backwards, add_wall_result.0, add_wall_result.1);
        } else {
            log_warn!("line added as polygon! (LayerPlan)");
        }
    }

    /// Add polygons to the gcode with optimized order.
    pub fn add_polygons_by_optimizer(
        &mut self,
        polygons: &Shape,
        config: &GCodePathConfig,
        settings: &Settings,
        opts: PolygonsOpts,
    ) {
        self.add_polygons_by_optimizer_textured(polygons, config, settings, opts, None);
    }

    /// `addPolygonsByOptimizer` with its `texture_data_provider` argument.
    pub fn add_polygons_by_optimizer_textured(
        &mut self,
        polygons: &Shape,
        config: &GCodePathConfig,
        settings: &Settings,
        opts: PolygonsOpts,
        texture_data_provider: Option<Arc<TextureDataProvider>>,
    ) {
        if polygons.is_empty() {
            return;
        }

        let detect_loops = false;
        let reverse_direction = false;
        let group_outer_walls = false;
        let use_shortest_for_inner_walls = false;
        let mut order_optimizer: PathOrderOptimizer<usize> = PathOrderOptimizer::new(
            opts.start_near_location.unwrap_or_else(|| self.get_last_planned_position_or_starting_position()),
            opts.z_seam_config,
            detect_loops,
            None,
            reverse_direction,
            &[],
            group_outer_walls,
            Shape::default(),
            use_shortest_for_inner_walls,
            Shape::default(),
        )
        .with_texture_data_provider(texture_data_provider);
        for (poly_idx, polygon) in polygons.iter().enumerate() {
            order_optimizer.add_polygon(poly_idx, Arc::new(polygon.points.clone()), None, false);
        }
        order_optimizer.optimize(true);

        self.add_polygons_in_given_order(
            &order_optimizer.paths,
            config,
            settings,
            opts.wall_0_wipe_dist,
            opts.spiralize,
            opts.flow_ratio,
            opts.always_retract,
            opts.reverse_order,
            opts.scarf_seam,
            opts.smooth_speed,
        );
    }

    /// Adds infill polygons to the gcode with optimized order.
    pub fn add_infill_polygons_by_optimizer(
        &mut self,
        polygons: &Shape,
        remaining_lines: &mut OpenLinesSet,
        config: &GCodePathConfig,
        settings: &Settings,
        add_extra_inwards_move: bool,
        near_start_location: Option<Point2LL>,
    ) {
        if polygons.is_empty() {
            return;
        }

        let mut order_optimizer: PathOrderOptimizer<usize> = PathOrderOptimizer::simple(
            near_start_location.unwrap_or_else(|| self.get_last_planned_position_or_starting_position()),
            ZSeamConfig::default(),
            false,
            None,
            false,
        );
        for (poly_idx, polygon) in polygons.iter().enumerate() {
            order_optimizer.add_polygon(poly_idx, Arc::new(polygon.points.clone()), None, false);
        }
        order_optimizer.optimize(true);

        if !add_extra_inwards_move {
            let force_comb_retract = false;
            let first = &order_optimizer.paths[0];
            let start = first.converted[first.start_vertex];
            self.add_travel(start, force_comb_retract, 0);
            self.add_polygons_in_given_order(&order_optimizer.paths, config, settings, 0, false, 1.0, false, false, false, false);
            return;
        }

        // In order to add the inwards moves, we will have to un-close the polygons to open lines
        for ordered_polygon in order_optimizer.paths.iter() {
            let polygon = &polygons[ordered_polygon.vertices];
            let start_index = ordered_polygon.start_vertex;

            let mut split_polygon = ClosedPolyline::new(polygon.points.clone(), polygon.explicitly_closed);
            crate::geometry::ClosedLine::shift_vertices_to_start_point(&mut split_polygon, start_index);
            remaining_lines.push(crate::geometry::ClosedLine::to_pseudo_open_polyline(&split_polygon));
        }
    }

    /// Add a single line that is part of a wall to the gcode.
    #[allow(clippy::too_many_arguments)]
    fn add_wall_line(
        &mut self,
        wall: &PathAdapter,
        segment_index: usize,
        segment_start_ratio: Ratio,
        segment_end_ratio: Ratio,
        p0: Point3LL,
        p1: Point3LL,
        settings: &Settings,
        default_config: &GCodePathConfig,
        roofing_config: &GCodePathConfig,
        flooring_config: &GCodePathConfig,
        bridge_config: &GCodePathConfig,
        flow: f64,
        width_factor: Ratio,
        non_bridge_line_volume: &mut f64,
        mut speed_factor: Ratio,
        mut distance_to_bridge_start: f64,
        travel_to_z: bool,
    ) {
        const MIN_LINE_LEN: Coord = 5; // we ignore lines less than 5um long
        const MIN_LINE_LEN_SQUARED: Coord = MIN_LINE_LEN * MIN_LINE_LEN;
        const ACCELERATION_SEGMENT_LEN: f64 = 1000.0; // accelerate using segments of this length
        const ACCELERATION_FACTOR: f64 = 0.75; // must be < 1, the larger the value, the slower the acceleration
        let spiralize = false;

        let min_bridge_line_len = max_c(MIN_LINE_LEN, settings.get_coord("bridge_wall_min_length"));
        let bridge_wall_coast = settings.get_ratio("bridge_wall_coast");

        let mut cur_point = p0;

        // helper function to add a single non-bridge line
        let add_non_bridge_line = |this: &mut Self,
                                       line_end: Point3LL,
                                       cur_point: &mut Point3LL,
                                       speed_factor: &mut Ratio,
                                       distance_to_bridge_start: &mut f64,
                                       non_bridge_line_volume: &mut f64| {
            let mut distance_to_line_end = (*cur_point - line_end).v_size();

            while distance_to_line_end > MIN_LINE_LEN {
                // if we are accelerating after a bridge line, the segment length is less than the whole line length
                let mut segment_end = if *speed_factor == 1.0 || (distance_to_line_end as f64) < ACCELERATION_SEGMENT_LEN {
                    line_end
                } else {
                    *cur_point + (line_end - *cur_point).scale_f64(ACCELERATION_SEGMENT_LEN) / distance_to_line_end
                };

                // flow required for the next line segment - when accelerating after a bridge segment, the flow is increased in inverse proportion to the speed_factor
                let segment_flow = if *speed_factor > 1.0 { flow * (1.0 / *speed_factor) } else { flow };

                // if a bridge is present in this wall, this particular segment may need to be partially or wholely coasted
                if *distance_to_bridge_start > 0.0 {
                    let speed_flow_factor: Ratio =
                        (bridge_config.get_speed() * bridge_config.get_flow_ratio()) / (default_config.get_speed() * default_config.get_flow_ratio());

                    // coast distance is proportional to distance, speed and flow of non-bridge segments just printed and is throttled by speed_flow_factor
                    let coast_dist = min_f(*non_bridge_line_volume, MAX_NON_BRIDGE_LINE_VOLUME) * (1.0 - speed_flow_factor) * bridge_wall_coast / 40.0;

                    if (*distance_to_bridge_start - distance_to_line_end as f64) <= coast_dist {
                        // coast takes precedence over acceleration
                        segment_end = line_end;
                    }

                    let len = (*cur_point - segment_end).v_size();
                    if coast_dist > 0.0 && ((*distance_to_bridge_start - len as f64) <= coast_dist) {
                        if (len as f64 - coast_dist) > MIN_LINE_LEN as f64 {
                            // segment is longer than coast distance so extrude using non-bridge config to start of coast
                            let target = segment_end + (*cur_point - segment_end).scale_f64(coast_dist) / len;
                            this.add_extrusion_move(
                                target,
                                default_config,
                                SpaceFillType::Polygons,
                                segment_flow,
                                width_factor,
                                spiralize,
                                *speed_factor,
                                GCodePathConfig::FAN_SPEED_DEFAULT,
                                travel_to_z,
                            );
                        }
                        // then coast to start of bridge segment
                        let no_flow = 0.0; // Coasting has no flow rate.
                        this.add_extrusion_move(
                            segment_end,
                            default_config,
                            SpaceFillType::Polygons,
                            no_flow,
                            width_factor,
                            spiralize,
                            *speed_factor,
                            GCodePathConfig::FAN_SPEED_DEFAULT,
                            true,
                        );
                    } else {
                        // no coasting required, just normal segment using non-bridge config
                        this.add_extrusion_move_with_gradual_overhang(
                            segment_end,
                            default_config,
                            SpaceFillType::Polygons,
                            segment_flow,
                            width_factor,
                            spiralize,
                            *speed_factor,
                            GCodePathConfig::FAN_SPEED_DEFAULT,
                            travel_to_z,
                        );
                    }

                    *distance_to_bridge_start -= len as f64;
                } else {
                    // no coasting required, just normal segment using non-bridge config
                    this.add_extrusion_move_with_gradual_overhang(
                        segment_end,
                        default_config,
                        SpaceFillType::Polygons,
                        segment_flow,
                        width_factor,
                        spiralize,
                        *speed_factor,
                        GCodePathConfig::FAN_SPEED_DEFAULT,
                        travel_to_z,
                    );
                }
                *non_bridge_line_volume += (*cur_point - segment_end).v_size() as f64 * segment_flow * width_factor * *speed_factor * default_config.get_speed();
                *cur_point = segment_end;
                *speed_factor = 1.0 - (1.0 - *speed_factor) * ACCELERATION_FACTOR;
                if *speed_factor >= 0.9 {
                    *speed_factor = 1.0;
                }
                distance_to_line_end = (*cur_point - line_end).v_size();
            }
        };

        let use_skin_config = |mask: &Shape, config: &GCodePathConfig| -> bool {
            if *config == *default_config {
                // if the roofing config and normal config are the same any way there is no need to check
                // what part of the line segment will be printed with what config.
                return false;
            }
            PolygonUtils::polygons_collide_with_line_segment(mask, p0.to_point2ll(), p1.to_point2ll()) || mask.inside(p1.to_point2ll(), true)
        };

        let add_skin_extrusion = |this: &mut Self, mask: &Shape, config: &GCodePathConfig| {
            // The line segment is wholly or partially in the skin area. The line is intersected
            // with the skin area into line segments. Each line segment left in this intersection
            // will be printed using the skin config, all removed segments will be printed using
            // the default_config.
            let mut line_polys = OpenLinesSet::default();
            line_polys.add_segment(p0.to_point2ll(), p1.to_point2ll());
            let restitch = false; // only a single line doesn't need stitching
            let mut skin_line_segments = mask.intersection_polylines(&line_polys, restitch, 10, true);

            if skin_line_segments.is_empty() {
                this.add_extrusion_move(p1, default_config, SpaceFillType::Polygons, flow, width_factor, spiralize, 1.0, GCodePathConfig::FAN_SPEED_DEFAULT, travel_to_z);
            } else {
                // reorder all the line segments so all lines start at p0 and end at p1
                for line_poly in skin_line_segments.iter_mut() {
                    let line_p0 = line_poly[0];
                    let line_p1 = *line_poly.last().unwrap();
                    if v_size2(sub_2d_3d(line_p1, &p0)) < v_size2(sub_2d_3d(line_p0, &p0)) {
                        line_poly.reverse();
                    }
                }
                cppstd::stable_sort_by(skin_line_segments.get_lines_mut(), |a, b| v_size2(sub_2d_3d(a[0], &p0)) < v_size2(sub_2d_3d(b[0], &p0)));

                // add intersected line segments, alternating between roofing and default_config
                for line_poly in skin_line_segments.iter() {
                    // This is only relevant for the very fist iteration of the loop
                    // if the start of the line segment is not at minimum distance from p0
                    if v_size2(sub_2d_3d(line_poly[0], &p0)) > MIN_LINE_LEN_SQUARED {
                        this.add_extrusion_move(
                            Point3LL::from_2d(line_poly[0], 0),
                            default_config,
                            SpaceFillType::Polygons,
                            flow,
                            width_factor,
                            spiralize,
                            1.0,
                            GCodePathConfig::FAN_SPEED_DEFAULT,
                            travel_to_z,
                        );
                    }

                    this.add_extrusion_move(
                        Point3LL::from_2d(*line_poly.last().unwrap(), 0),
                        config,
                        SpaceFillType::Polygons,
                        flow,
                        width_factor,
                        spiralize,
                        1.0,
                        GCodePathConfig::FAN_SPEED_DEFAULT,
                        travel_to_z,
                    );
                }

                // if the last point is not yet at a minimum distance from p1 then add a move to p1
                if v_size2(sub_2d_3d(*skin_line_segments.last().unwrap().last().unwrap(), &p1)) > MIN_LINE_LEN_SQUARED {
                    this.add_extrusion_move(p1, default_config, SpaceFillType::Polygons, flow, width_factor, spiralize, 1.0, GCodePathConfig::FAN_SPEED_DEFAULT, travel_to_z);
                }
            }
        };

        let roofing_mask = std::mem::take(&mut self.roofing_mask);
        let flooring_mask = std::mem::take(&mut self.flooring_mask);
        let bridge_wall_mask = std::mem::take(&mut self.bridge_wall_mask);

        if use_skin_config(&roofing_mask, roofing_config) {
            add_skin_extrusion(self, &roofing_mask, roofing_config);
        } else if bridge_wall_mask.is_empty() {
            // no bridges required
            self.add_extrusion_move_with_gradual_overhang(
                p1,
                default_config,
                SpaceFillType::Polygons,
                flow,
                width_factor,
                spiralize,
                speed_factor,
                GCodePathConfig::FAN_SPEED_DEFAULT,
                travel_to_z,
            );
        } else {
            let bridging_subsegments = wall_segment_uses_bridging(
                &self.bridge_wall_mask_bb,
                &bridge_wall_mask,
                wall,
                segment_index,
                segment_start_ratio,
                segment_end_ratio,
                min_bridge_line_len,
                default_config.line_width,
            );
            if !bridging_subsegments.is_empty() {
                // the line crosses the boundary between supported and non-supported regions so one or more bridges are required
                for &(start_ratio, end_ratio) in bridging_subsegments.iter() {
                    let bridging_subsegment_p0 = lerp3(&p0, &p1, start_ratio);
                    let bridging_subsegment_p1 = lerp3(&p0, &p1, end_ratio);

                    // Temporarily restore the masks while adding the non bridge line (it may need the overhang masks only)
                    add_non_bridge_line(self, bridging_subsegment_p0, &mut cur_point, &mut speed_factor, &mut distance_to_bridge_start, non_bridge_line_volume);
                    self.add_extrusion_move(
                        bridging_subsegment_p1,
                        bridge_config,
                        SpaceFillType::Polygons,
                        flow,
                        width_factor,
                        spiralize,
                        1.0,
                        GCodePathConfig::FAN_SPEED_DEFAULT,
                        travel_to_z,
                    );

                    *non_bridge_line_volume = 0.0;
                    cur_point = bridging_subsegment_p1;

                    // after a bridge segment, start slow and accelerate to avoid under-extrusion due to extruder lag
                    speed_factor = max_f(min_f(bridge_config.get_speed() / default_config.get_speed(), 1.0), 0.5);
                }

                add_non_bridge_line(self, p1, &mut cur_point, &mut speed_factor, &mut distance_to_bridge_start, non_bridge_line_volume);
            } else if use_skin_config(&flooring_mask, flooring_config) {
                add_skin_extrusion(self, &flooring_mask, flooring_config);
            } else {
                // no part of the line is above air or the line is too short to print as a bridge line
                add_non_bridge_line(self, p1, &mut cur_point, &mut speed_factor, &mut distance_to_bridge_start, non_bridge_line_volume);
            }
        }

        self.roofing_mask = roofing_mask;
        self.flooring_mask = flooring_mask;
        self.bridge_wall_mask = bridge_wall_mask;
    }

    /// Add a wall (polygon) to the g-code starting at vertex `start_idx`.
    #[allow(clippy::too_many_arguments)]
    pub fn add_wall_polygon(
        &mut self,
        wall: &[Point2LL],
        start_idx: usize,
        settings: &Settings,
        default_config: &GCodePathConfig,
        roofing_config: &GCodePathConfig,
        flooring_config: &GCodePathConfig,
        bridge_config: &GCodePathConfig,
        wall_0_wipe_dist: Coord,
        flow_ratio: f64,
        always_retract: bool,
    ) {
        if wall.len() < 3 {
            log_warn!("Point, or line added as (polygon) wall sequence! (LayerPlan)");
        }

        let dummy_perimeter_id = 0;
        let nominal_line_width = default_config.get_line_width();

        let mut ewall = ExtrusionLine::default();
        for &p in wall {
            ewall.junctions.push(ExtrusionJunction::new(p, nominal_line_width, dummy_perimeter_id));
        }
        ewall.junctions.push(ExtrusionJunction::new(wall[0], nominal_line_width, dummy_perimeter_id));
        let is_closed = true;
        let is_reversed = false;
        let is_linked_path = false;
        self.add_wall(
            &ewall,
            start_idx,
            settings,
            default_config,
            roofing_config,
            flooring_config,
            bridge_config,
            wall_0_wipe_dist,
            flow_ratio,
            always_retract,
            is_closed,
            is_reversed,
            is_linked_path,
            false,
            false,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn add_split_wall(
        &mut self,
        wall: &PathAdapter,
        wall_length: Coord,
        start_idx: usize,
        max_index: usize,
        direction: i32,
        default_config: &GCodePathConfig,
        always_retract: bool,
        is_small_feature: bool,
        small_feature_speed_factor: Ratio,
        max_area_deviation: Coord,
        max_resolution: Coord,
        flow_ratio: f64,
        nominal_line_width: Coord,
        min_bridge_line_len: Coord,
        scarf_seam_length: Coord,
        _scarf_seam_start_ratio: Ratio,
        scarf_split_distance: Coord,
        scarf_max_z_offset: Coord,
        speed_split_distance: Coord,
        start_speed_ratio: Ratio,
        accelerate_length: Coord,
        end_speed_ratio: Ratio,
        decelerate_length: Coord,
        is_scarf_closure: bool,
        compute_distance_to_bridge_start: bool,
        func_add_segment: &mut dyn FnMut(&mut LayerPlan<'a>, &PathAdapter, SegmentArgs),
    ) -> (usize, Point2LL) {
        let mut distance_to_bridge_start: Option<Coord> = None; // will be updated before each line is processed
        let mut p0 = wall.point_at(start_idx);
        let mut w0 = wall.line_width_at(start_idx);
        let mut first_line = !is_scarf_closure;
        let mut first_split = !is_scarf_closure;
        let mut split_origin = Point3LL::from_2d(p0, 0);
        if !is_scarf_closure && scarf_seam_length > 0 {
            split_origin.z = scarf_max_z_offset;
        }

        let mut wall_processed_distance: Coord = 0; // This will grow while we travel along the wall, to the total wall length
        let mut accelerate_factor_origin = 0.0; // Interpolation factor at the current point for the acceleration
        let mut decelerate_factor_origin = 0.0; // Interpolation factor at the current point for the deceleration
        let start_decelerate_position = wall_length - decelerate_length;
        let mut split_destination = Point3LL::from_2d(p0, 0);
        let mut previous_point_index = start_idx;
        let mut keep_processing = true;

        let size = wall.size() as u64;
        let dir = direction as i64 as u64;

        let mut point_idx: usize = 1;
        while point_idx < max_index && keep_processing {
            let actual_point_index = (size.wrapping_add(start_idx as u64).wrapping_add((point_idx as u64).wrapping_mul(dir)) % size) as usize;
            previous_point_index = (size.wrapping_add(start_idx as u64).wrapping_add(((point_idx - 1) as u64).wrapping_mul(dir)) % size) as usize;
            let p1 = wall.point_at(actual_point_index);
            let w1 = wall.line_width_at(actual_point_index);
            let mut segment_processed_distance: Coord = 0;

            if let Some(extrusion_line) = wall.extrusion_line() {
                // The bridging functionality has not been designed to work with anything else than ExtrusionLine objects
                if compute_distance_to_bridge_start && !self.bridge_wall_mask.is_empty() && distance_to_bridge_start.is_none() {
                    let current_index = (size.wrapping_add(start_idx as u64).wrapping_add((point_idx as u64).wrapping_mul(dir)).wrapping_sub(1) % size) as usize;
                    distance_to_bridge_start = Some(self.compute_distance_to_bridge_start(extrusion_line, current_index, min_bridge_line_len, direction));
                }
            }

            if first_line {
                self.add_travel(p0, always_retract, 0);
                first_line = false;
            }

            /*
            If the line has variable width, break it up into pieces with the
            following constraints:
            - Each piece must be smaller than the Maximum Resolution setting.
            - The difference between the trapezoidal shape of the line and the
              rectangular shape of the line may not exceed the Maximum Extrusion
              Area Deviation setting, unless required by the first constraint.
            */
            let delta_line_width = w1 - w0;
            let line_vector = p1 - p0;
            let line_length = v_size(line_vector);
            let line_area_deviation = delta_line_width.abs() * line_length / 4;
            let pieces_limit_deviation = round_up_divide(line_area_deviation as u64, max_area_deviation as u64) as usize; // How many pieces we'd need to stay beneath the max area deviation.
            let pieces_limit_resolution = (line_length / max_resolution) as usize; // Round down this time, to not exceed the maximum resolution.
            let pieces = 1usize.max(pieces_limit_deviation.min(pieces_limit_resolution)); // Resolution overrides deviation, if resolution is a constraint.
            let piece_length = round_divide(line_length as u64, pieces as u64) as Coord;

            let mut piece = 0usize;
            while piece < pieces && keep_processing {
                let average_progress = (piece as f64 + 0.5) / pieces as f64; // How far along this line to sample the line width in the middle of this piece.
                // Round the line_width value to overcome floating point rounding issues
                let line_width = cppstd::llrint(w0 as f64 + average_progress * delta_line_width as f64);
                let line_width_ratio: Ratio = line_width as f64 / nominal_line_width as f64;
                let destination = p0 + normal(line_vector, piece_length.wrapping_mul((piece + 1) as Coord));
                if is_small_feature && !is_scarf_closure {
                    let spiralize = false;
                    self.add_extrusion_move(
                        Point3LL::from_2d(destination, 0),
                        default_config,
                        SpaceFillType::Polygons,
                        flow_ratio,
                        line_width_ratio,
                        spiralize,
                        small_feature_speed_factor,
                        GCodePathConfig::FAN_SPEED_DEFAULT,
                        true,
                    );
                } else {
                    let mut piece_remaining_distance = piece_length;

                    // Cut piece into smaller parts for scarf seam and acceleration/deceleration
                    while piece_remaining_distance > 0 && keep_processing {
                        // Make a list of all the possible incoming positions where we would eventually want to stop next
                        let mut split_positions: Vec<Coord> = vec![wall_processed_distance + piece_remaining_distance];

                        let process_scarf = wall_processed_distance < scarf_seam_length;
                        if process_scarf {
                            split_positions.push(scarf_seam_length);
                            split_positions.push(wall_processed_distance + scarf_split_distance);
                        }

                        let process_acceleration = !is_scarf_closure && wall_processed_distance < accelerate_length;
                        if process_acceleration {
                            split_positions.push(accelerate_length);
                            split_positions.push(wall_processed_distance + speed_split_distance);
                        }

                        let mut deceleration_started = false;
                        if !is_scarf_closure && decelerate_length > 0 {
                            deceleration_started = wall_processed_distance >= start_decelerate_position;
                            if deceleration_started {
                                split_positions.push(wall_processed_distance + speed_split_distance);
                            } else {
                                split_positions.push(start_decelerate_position);
                            }
                        }

                        // Now take the closest position candidate and make a sub-segment to it
                        let destination_position = *split_positions.iter().min().unwrap();
                        let length_to_process = destination_position - wall_processed_distance;
                        let destination_factor = (segment_processed_distance + length_to_process) as f64 / line_length as f64;
                        split_destination = Point3LL::from_2d(lerp2(p0, p1, destination_factor), 0);

                        if process_scarf {
                            // Calculate scarf interpolation factor on the destination point
                            let scarf_factor_destination = destination_position as f64 / scarf_seam_length as f64;

                            // Interpolate Z offset according to interpolation factor
                            if !is_scarf_closure {
                                split_destination.z = cppstd::llrint(cppstd::lerp(scarf_max_z_offset as f64, 0.0, scarf_factor_destination));
                            }

                            if first_split {
                                // Manually add a Z-only travel move to set the nozzle at the height of the first point
                                self.add_travel(p0, always_retract, split_origin.z);
                                first_split = false;
                            }
                        }

                        let mut accelerate_speed_factor: Ratio = 1.0;
                        let mut accelerate_factor_destination = 1.0; // Out of range, acceleration is done => 1.0
                        if process_acceleration {
                            accelerate_factor_destination = destination_position as f64 / accelerate_length as f64;
                            let accelerate_factor_average = (accelerate_factor_origin + accelerate_factor_destination) / 2.0;
                            accelerate_speed_factor = cppstd::lerp(start_speed_ratio, 1.0, accelerate_factor_average);
                        }

                        let mut decelerate_speed_factor: Ratio = if is_scarf_closure { end_speed_ratio } else { 1.0 };
                        let mut decelerate_factor_destination = 0.0; // Out of range, deceleration is not started => 0.0
                        if deceleration_started {
                            decelerate_factor_destination = 1.0 - ((wall_length - destination_position) as f64 / decelerate_length as f64);
                            let decelerate_factor_average = (decelerate_factor_origin + decelerate_factor_destination) / 2.0;
                            decelerate_speed_factor = cppstd::lerp(1.0, end_speed_ratio, decelerate_factor_average);
                        }

                        // now add the (sub-)segment
                        let args = SegmentArgs {
                            segment_index: wrap_index((actual_point_index as u64).wrapping_sub(1) as i32, wall.size()),
                            segment_start_ratio: (segment_processed_distance as f32 / line_length as f32) as f64,
                            segment_end_ratio: ((segment_processed_distance + length_to_process) as f32 / line_length as f32) as f64,
                            start: split_origin,
                            end: split_destination,
                            speed_factor: accelerate_speed_factor * decelerate_speed_factor,
                            flow_ratio,
                            line_width_ratio,
                            distance_to_bridge_start: distance_to_bridge_start.unwrap_or(0),
                        };
                        func_add_segment(self, wall, args);

                        wall_processed_distance = destination_position;
                        segment_processed_distance += length_to_process;
                        piece_remaining_distance -= length_to_process;
                        split_origin = split_destination;
                        accelerate_factor_origin = accelerate_factor_destination;
                        decelerate_factor_origin = decelerate_factor_destination;

                        if is_scarf_closure {
                            keep_processing = wall_processed_distance < scarf_seam_length;
                        }
                    }
                }
                piece += 1;
            }

            if let Some(d) = distance_to_bridge_start {
                if d > 0 {
                    if segment_processed_distance < d {
                        distance_to_bridge_start = Some(d - segment_processed_distance);
                    } else {
                        distance_to_bridge_start = None;
                    }
                }
            }

            p0 = p1;
            w0 = w1;
            point_idx += 1;
        }

        (previous_point_index, split_destination.to_point2ll())
    }

    /// Pre-calculates the coasting to be applied on the paths.
    fn calculate_paths_coasting(&self, extruder_settings: &Settings, paths: &[GCodePath], current_position: Point3LL) -> Vec<PathCoasting> {
        let mut path_coastings: Vec<PathCoasting> = vec![PathCoasting::default(); paths.len()];

        if extruder_settings.get_bool("coasting_enable") {
            let coasting_volume = extruder_settings.get_f64("coasting_volume");
            let coasting_min_volume = extruder_settings.get_f64("coasting_min_volume");

            // Chunk the reversed, enumerated paths by `(!a.isTravelPath()) || b.isTravelPath()` on adjacent elements.
            let reversed: Vec<usize> = (0..paths.len()).rev().collect();
            let mut chunks: Vec<Vec<usize>> = Vec::new();
            for &idx in reversed.iter() {
                match chunks.last_mut() {
                    Some(chunk) => {
                        let a = *chunk.last().unwrap();
                        if !paths[a].is_travel_path() || paths[idx].is_travel_path() {
                            chunk.push(idx);
                        } else {
                            chunks.push(vec![idx]);
                        }
                    }
                    None => chunks.push(vec![idx]),
                }
            }

            for reversed_chunk in chunks.iter() {
                if reversed_chunk.is_empty() {
                    continue;
                }

                let type_ = paths[reversed_chunk[0]].config.get_print_feature_type();
                if type_ != PrintFeatureType::OuterWall && type_ != PrintFeatureType::InnerWall {
                    continue;
                }

                let mut accumulated_volume = 0.0;
                let mut chunk_coasting_point_reached = false;
                let mut chunk_min_volume_reached = false;

                for &path_idx in reversed_chunk.iter() {
                    let path = &paths[path_idx];
                    if path.is_travel_path() {
                        continue;
                    }

                    let path_extrusion_per_length = path.config.extrusion_mm3_per_mm * path.flow;

                    for point_idx in (0..path.points.len()).rev() {
                        let point = path.points[point_idx];
                        let previous_point = if point_idx > 0 {
                            path.points[point_idx - 1]
                        } else if path_idx > 0 {
                            *paths[path_idx - 1].points.last().unwrap()
                        } else {
                            current_position
                        };

                        let segment_length = int2mm((point - previous_point).v_size());
                        let segment_volume = segment_length * path_extrusion_per_length;

                        accumulated_volume += segment_volume;

                        let path_coasting = &mut path_coastings[path_idx];
                        if !chunk_coasting_point_reached && path_coasting.apply_coasting == ApplyCoasting::NoCoasting {
                            if accumulated_volume >= coasting_volume {
                                let start_pos_factor = (accumulated_volume - coasting_volume) / segment_volume;
                                let coasting_start_pos = lerp3(&previous_point, &point, start_pos_factor);
                                *path_coasting = PathCoasting { apply_coasting: ApplyCoasting::PartialCoasting, coasting_start_index: point_idx, coasting_start_pos };
                                chunk_coasting_point_reached = true;
                            } else if point_idx == 0 {
                                // End of path reached (reverse iteration), coasting not fulfilled
                                path_coasting.apply_coasting = ApplyCoasting::CoastEntirePath;
                            }
                        }

                        if accumulated_volume >= coasting_min_volume {
                            chunk_min_volume_reached = true;
                        }

                        if chunk_min_volume_reached && chunk_coasting_point_reached {
                            break;
                        }
                    }

                    if chunk_min_volume_reached && chunk_coasting_point_reached {
                        break;
                    }
                }

                if !chunk_min_volume_reached {
                    // It turns out this chunk doesn't fit the minimum requirements for coasting, so skip it
                    for &path_idx in reversed_chunk.iter() {
                        path_coastings[path_idx].apply_coasting = ApplyCoasting::NoCoasting;
                    }
                }
            }
        }

        path_coastings
    }

    /// Helper function to calculate the distance from the start of the current wall line to the first bridge segment.
    fn compute_distance_to_bridge_start(&self, wall: &ExtrusionLine, current_index: usize, min_bridge_line_len: Coord, direction: i32) -> Coord {
        let mut distance_to_bridge_start: Coord = 0;
        let size = wall.junctions.len();

        if !self.bridge_wall_mask.is_empty() {
            let iterations = if wall.is_closed { size } else if direction > 0 { size - current_index } else { current_index };

            // there is air below the part so iterate through the lines that have not yet been output accumulating the total distance to the first bridge segment
            for iteration in 0..iterations {
                let base_index = (current_index as u64).wrapping_add((iteration as u64).wrapping_mul(direction as i64 as u64));
                let p0 = wall.junctions[wrap_index(base_index as i32, size)];
                let p1 = wall.junctions[wrap_index(base_index.wrapping_add(direction as i64 as u64) as i32, size)];

                if self.bridge_wall_mask_bb.hit(&AABB::from_points(&[p0.p, p1.p])) && PolygonUtils::polygons_collide_with_line_segment(&self.bridge_wall_mask, p0.p, p1.p) {
                    let restitch = false; // only a single line doesn't need stitching
                    let segments = OpenLinesSet::from_line(OpenPolyline::new(vec![p0.p, p1.p]));
                    let intersections_with_bridge_mask = self.bridge_wall_mask.intersection_polylines(&segments, restitch, 10, true);

                    // the line crosses the boundary between supported and non-supported regions so it will contain one or more bridge segments
                    // First, replace the intersection segments in the proper direction, and sort them by order along the main segment
                    // (the C++ comparator mutates its arguments, which we mimic with interior mutability)
                    let mut cells: Vec<RefCell<OpenPolyline>> = intersections_with_bridge_mask.get_lines().iter().cloned().map(RefCell::new).collect();
                    let p0p = p0.p;
                    cppstd::sort_by(&mut cells, |s0, s1| {
                        let compare = |segment0: &mut OpenPolyline, segment1: &mut OpenPolyline| -> bool {
                            let distance_s0_p0 = v_size2(segment0[0] - p0p);
                            let distance_s0_p1 = v_size2(segment0[1] - p0p);
                            let distance_s1_p0 = v_size2(segment1[0] - p0p);
                            let distance_s1_p1 = v_size2(segment1[1] - p0p);
                            if distance_s0_p1 < distance_s0_p0 {
                                segment0.points.swap(0, 1);
                            }
                            if distance_s1_p1 < distance_s1_p0 {
                                segment1.points.swap(0, 1);
                            }
                            distance_s0_p0 < distance_s1_p0
                        };
                        if std::ptr::eq(s0, s1) {
                            let mut a = s0.borrow_mut();
                            let mut b = a.clone();
                            let r = compare(&mut a, &mut b);
                            *a = b; // the second swap wins in C++ (same object)
                            r
                        } else {
                            compare(&mut s0.borrow_mut(), &mut s1.borrow_mut())
                        }
                    });

                    // Now loop over the segments and try to find one that is long enough
                    for cell in cells.iter() {
                        let intersection_segment = cell.borrow();
                        let b0 = intersection_segment[0];
                        let b1 = intersection_segment[1];

                        let bridge_line_len = v_size(b1 - b0) as f64;
                        if bridge_line_len >= min_bridge_line_len as f64 {
                            // job done, we have found the first bridge line
                            return distance_to_bridge_start + v_size(b0 - p0.p);
                        }
                    }

                    // None of the intersection segments was long enough to be considered relevant, so just ignore the segment
                    distance_to_bridge_start += v_size(p1.p - p0.p);
                } else if !self.bridge_wall_mask.inside(p0.p, true) {
                    // none of the line is over air
                    distance_to_bridge_start += v_size(p1.p - p0.p);
                }
            }

            // we have got all the way to the end of the wall without finding a bridge segment so disable coasting by setting distance_to_bridge_start back to 0
            distance_to_bridge_start = 0;
        }

        distance_to_bridge_start
    }

    #[allow(clippy::too_many_arguments)]
    fn add_wall_with_scarf_seam(
        &mut self,
        wall: &PathAdapter,
        start_idx: usize,
        settings: &Settings,
        default_config: &GCodePathConfig,
        flow_ratio: f64,
        always_retract: bool,
        is_closed: bool,
        is_reversed: bool,
        is_candidate_small_feature: bool,
        scarf_seam: bool,
        smooth_speed: bool,
        func_add_segment: &mut dyn FnMut(&mut LayerPlan<'a>, &PathAdapter, SegmentArgs),
    ) -> (usize, Point2LL) {
        if wall.empty() {
            return (start_idx, Point2LL::default());
        }

        let actual_scarf_seam = scarf_seam && is_closed && self.layer_nr > 0;

        let min_bridge_line_len = settings.get_coord("bridge_wall_min_length");

        let nominal_line_width = default_config.get_line_width();

        let wall_length = wall.length();
        let small_feature_max_length = settings.get_coord("small_feature_max_length");
        let is_small_feature = (small_feature_max_length > 0) && (self.layer_nr == 0 || is_candidate_small_feature) && wall_length < small_feature_max_length;
        let min_speed = self.fan_speed_layer_time_settings_per_extruder[self.get_last_planned_extruder_train().extruder_nr].cool_min_speed;
        let mut small_feature_speed_factor = settings.get_ratio(if self.layer_nr == 0 { "small_feature_speed_factor_0" } else { "small_feature_speed_factor" });
        small_feature_speed_factor = max_f(small_feature_speed_factor, min_speed / default_config.get_speed());
        let max_area_deviation = settings.get_int("meshfix_maximum_extrusion_area_deviation").max(1) as Coord; // Square micrometres!
        let max_resolution = max_c(settings.get_coord("meshfix_maximum_resolution"), 1);
        let direction = if is_reversed { -1 } else { 1 };
        let max_index = if is_closed { wall.size() + 1 } else { wall.size() };

        let scarf_seam_length = {
            let s = if actual_scarf_seam { settings.get_coord("scarf_joint_seam_length") } else { 0 };
            if s < wall_length {
                s
            } else {
                wall_length
            }
        };
        let scarf_seam_start_ratio = if actual_scarf_seam { settings.get_ratio("scarf_joint_seam_start_height_ratio") } else { 1.0 };
        let scarf_split_distance = settings.get_coord("scarf_split_distance");
        let scarf_max_z_offset = (-(1.0 - scarf_seam_start_ratio) * self.layer_thickness as f64) as Coord;

        let top_speed = default_config.get_speed();
        let speed_split_distance = settings.get_coord("wall_0_speed_split_distance"); // mm
        let start_speed_ratio = if smooth_speed { settings.get_ratio("wall_0_start_speed_ratio") } else { 1.0 };
        let acceleration = settings.get_int("wall_0_acceleration"); // mm/s²
        let start_speed = top_speed * start_speed_ratio; // mm/s
        let accelerate_length = if smooth_speed && start_speed_ratio < 1.0 {
            mm2int((top_speed * top_speed - start_speed * start_speed) / (2.0 * acceleration as f64))
        } else {
            0
        }; // µm

        let end_speed_ratio = if smooth_speed { settings.get_ratio("wall_0_end_speed_ratio") } else { 1.0 };
        let deceleration = settings.get_int("wall_0_deceleration"); // mm/s²
        let end_speed = top_speed * end_speed_ratio; // mm/s
        let decelerate_length = if smooth_speed && end_speed_ratio < 1.0 {
            mm2int((top_speed * top_speed - end_speed * end_speed) / (2.0 * deceleration as f64))
        } else {
            0
        }; // µm

        let add_split_wall_pass = |this: &mut Self, is_scarf_closure: bool, func: &mut dyn FnMut(&mut LayerPlan<'a>, &PathAdapter, SegmentArgs)| -> (usize, Point2LL) {
            let compute_distance_to_bridge_start = true;
            this.add_split_wall(
                wall,
                wall_length,
                start_idx,
                max_index,
                direction,
                default_config,
                always_retract,
                is_small_feature,
                small_feature_speed_factor,
                max_area_deviation,
                max_resolution,
                flow_ratio,
                nominal_line_width,
                min_bridge_line_len,
                scarf_seam_length,
                scarf_seam_start_ratio,
                scarf_split_distance,
                scarf_max_z_offset,
                speed_split_distance,
                start_speed_ratio,
                accelerate_length,
                end_speed_ratio,
                decelerate_length,
                is_scarf_closure,
                compute_distance_to_bridge_start,
                func,
            )
        };

        // First pass to add the wall with the scarf beginning and acceleration
        let mut result = add_split_wall_pass(self, false, func_add_segment);

        if scarf_seam_length > 0 {
            // Second pass to add the scarf closure
            result = add_split_wall_pass(self, true, func_add_segment);
        }

        result
    }

    /// Add a wall to the g-code starting at vertex `start_idx`.
    #[allow(clippy::too_many_arguments)]
    pub fn add_wall(
        &mut self,
        wall: &ExtrusionLine,
        start_idx: usize,
        settings: &Settings,
        default_config: &GCodePathConfig,
        roofing_config: &GCodePathConfig,
        flooring_config: &GCodePathConfig,
        bridge_config: &GCodePathConfig,
        wall_0_wipe_dist: Coord,
        flow_ratio: f64,
        always_retract: bool,
        is_closed: bool,
        is_reversed: bool,
        is_linked_path: bool,
        scarf_seam: bool,
        smooth_speed: bool,
    ) {
        if wall.junctions.is_empty() {
            return;
        }

        let mut non_bridge_line_volume = MAX_NON_BRIDGE_LINE_VOLUME; // assume extruder is fully pressurised before first non-bridge line is output
        let path_adapter = PathAdapter::Extrusion(wall);

        let default_c = *default_config;
        let roofing_c = *roofing_config;
        let flooring_c = *flooring_config;
        let bridge_c = *bridge_config;
        let add_wall_result = self.add_wall_with_scarf_seam(
            &path_adapter,
            start_idx,
            settings,
            default_config,
            flow_ratio,
            always_retract,
            is_closed,
            is_reversed,
            wall.inset_idx == 0,
            scarf_seam,
            smooth_speed,
            &mut |this: &mut LayerPlan, wall: &PathAdapter, args: SegmentArgs| {
                let travel_to_z = false;
                this.add_wall_line(
                    wall,
                    args.segment_index,
                    args.segment_start_ratio,
                    args.segment_end_ratio,
                    args.start,
                    args.end,
                    settings,
                    &default_c,
                    &roofing_c,
                    &flooring_c,
                    &bridge_c,
                    args.flow_ratio,
                    args.line_width_ratio,
                    &mut non_bridge_line_volume,
                    args.speed_factor,
                    args.distance_to_bridge_start as f64,
                    travel_to_z,
                );
            },
        );

        if wall.junctions.len() >= 2 {
            if !is_linked_path {
                self.add_wipe_travel(&path_adapter, wall_0_wipe_dist, is_reversed, add_wall_result.0, add_wall_result.1);
            }
        } else {
            log_warn!("Point added as wall sequence! (LayerPlan)");
        }
    }

    /// Add an infill wall to the g-code.
    pub fn add_infill_wall(&mut self, wall: &ExtrusionLine, path_config: &GCodePathConfig, force_retract: bool) {
        assert!(!wall.junctions.is_empty(), "All empty walls should have been filtered at this stage");
        let junction = wall.junctions[0];
        self.add_travel(junction.p, force_retract, 0);

        for junction_n in wall.junctions.iter() {
            let width_factor: Ratio = junction_n.w as f64 / path_config.get_line_width() as f64;
            let space_fill_type = SpaceFillType::Polygons;
            let flow = 1.0;
            self.add_extrusion_move(Point3LL::from_2d(junction_n.p, 0), path_config, space_fill_type, flow, width_factor, false, 1.0, GCodePathConfig::FAN_SPEED_DEFAULT, true);
        }
    }

    /// Add walls (polygons) to the gcode with optimized order.
    #[allow(clippy::too_many_arguments)]
    pub fn add_walls(
        &mut self,
        walls: &Shape,
        settings: &Settings,
        default_config: &GCodePathConfig,
        roofing_config: &GCodePathConfig,
        flooring_config: &GCodePathConfig,
        bridge_config: &GCodePathConfig,
        z_seam_config: ZSeamConfig,
        wall_0_wipe_dist: Coord,
        flow_ratio: f64,
        always_retract: bool,
    ) {
        let mut order_optimizer: PathOrderOptimizer<usize> =
            PathOrderOptimizer::simple(self.get_last_planned_position_or_starting_position(), z_seam_config, false, None, false);
        for (idx, polygon) in walls.iter().enumerate() {
            order_optimizer.add_polygon(idx, Arc::new(polygon.points.clone()), None, false);
        }
        order_optimizer.optimize(true);
        for path in order_optimizer.paths.iter() {
            self.add_wall_polygon(
                &walls[path.vertices].points,
                path.start_vertex,
                settings,
                default_config,
                roofing_config,
                flooring_config,
                bridge_config,
                wall_0_wipe_dist,
                flow_ratio,
                always_retract,
            );
        }
    }

    /// Add lines (open polylines) to the gcode with optimized order.
    pub fn add_lines_by_optimizer_open(&mut self, lines: &OpenLinesSet, config: &GCodePathConfig, space_fill_type: SpaceFillType, opts: LinesOpts) {
        let refs: Vec<LineRef> = lines.iter().map(LineRef::Open).collect();
        self.add_lines_by_optimizer_impl(&refs, config, space_fill_type, opts, true);
    }

    /// Add lines (closed polylines) to the gcode with optimized order.
    pub fn add_lines_by_optimizer_closed(&mut self, lines: &crate::geometry::ClosedLinesSet, config: &GCodePathConfig, space_fill_type: SpaceFillType, opts: LinesOpts) {
        let refs: Vec<LineRef> = lines.iter().map(LineRef::Closed).collect();
        self.add_lines_by_optimizer_impl(&refs, config, space_fill_type, opts, true);
    }

    /// Add lines of mixed types to the gcode with optimized order.
    pub fn add_lines_by_optimizer_mixed(&mut self, lines: &MixedLinesSet, config: &GCodePathConfig, space_fill_type: SpaceFillType, opts: LinesOpts) {
        let refs: Vec<LineRef> = lines
            .iter()
            .map(|l| match &**l {
                AnyPolyline::Open(o) => LineRef::Open(o),
                AnyPolyline::Closed(c) => LineRef::Closed(c),
                AnyPolyline::Polygon(p) => LineRef::Polygon(p),
            })
            .collect();
        let opts = LinesOpts { extra_inwards_start_move_length: 0, extra_inwards_end_move_length: 0, extra_inwards_move_contour: None, ..opts };
        self.add_lines_by_optimizer_impl(&refs, config, space_fill_type, opts, false);
    }

    fn add_lines_by_optimizer_impl(&mut self, lines: &[LineRef], config: &GCodePathConfig, space_fill_type: SpaceFillType, opts: LinesOpts, detect_loops: bool) {
        let mut boundary = Shape::default();
        if opts.enable_travel_optimization && !self.comb_boundary_minimum.is_empty() {
            // use the combing boundary inflated so that all infill lines are inside the boundary
            let mut dist: i32 = 0;
            if self.layer_nr >= 0 {
                // determine how much the skin/infill lines overlap the combing boundary
                for mesh in self.storage.meshes.iter() {
                    let s = mesh.settings();
                    let overlap = max_c(s.get_coord("skin_overlap_mm"), s.get_coord("infill_overlap_mm"));
                    if overlap > dist as Coord {
                        dist = overlap as i32;
                    }
                }
                dist += 100; // ensure boundary is slightly outside all skin/infill lines
            }
            boundary.push_all_owned(self.comb_boundary_minimum.offset_default(dist as Coord));
            // simplify boundary to cut down processing time
            boundary = Simplify::new(100, 100, 0).shape(&boundary);
        }
        let mut order_optimizer: PathOrderOptimizer<usize> = PathOrderOptimizer::new(
            opts.near_start_location.unwrap_or_else(|| self.get_last_planned_position_or_starting_position()),
            ZSeamConfig::default(),
            detect_loops,
            Some(&boundary),
            opts.reverse_print_direction,
            opts.order_requirements,
            false,
            Shape::default(),
            false,
            Shape::default(),
        );
        for (idx, line) in lines.iter().enumerate() {
            if line.is_open() {
                order_optimizer.add_polyline(idx, Arc::new(line.points().clone()));
            } else {
                order_optimizer.add_polygon(idx, Arc::new(line.points().clone()), None, false);
            }
        }
        order_optimizer.optimize(true);
        let paths = std::mem::take(&mut order_optimizer.paths);
        drop(order_optimizer);

        self.add_lines_in_given_order(
            &paths,
            lines,
            config,
            space_fill_type,
            opts.wipe_dist,
            opts.flow_ratio,
            opts.fan_speed,
            opts.extra_inwards_start_move_length,
            opts.extra_inwards_end_move_length,
            opts.extra_inwards_move_contour,
        );
    }

    /// `makeInwardsMove`: a move from `start_point` going inwards along the skeletal trapezoidation,
    /// away from the outline, with the given length.
    fn make_inwards_move(graph: &crate::arachne::graph::Graph, start_point: Point2LL, move_inwards_length: Coord) -> Vec<Point2LL> {
        use crate::geometry::point2ll::{dot, v_size2f};
        use crate::utils::index_list::NIL;
        // Find the trapezoidal that the start point belongs to
        let mut trapezoidal_start: Option<usize> = None;
        let mut trapezoidal_segments: u8 = 0;
        let mut projection_ratio_on_outer_segment = 0.0;
        let mut distance_to_closest_segment_squared: Option<Coord> = None;
        let mut e = graph.edges.first();
        while e != NIL {
            let start_edge = &graph.edges[e];
            if start_edge.prev != NIL {
                // This is not a starting edge, skip
                e = graph.edges.next(e);
                continue;
            }

            let mut current_trapezoidal_segments: u8 = 1;
            let mut end_edge = e;
            while graph.edges[end_edge].next != NIL {
                current_trapezoidal_segments = current_trapezoidal_segments.wrapping_add(1);
                end_edge = graph.edges[end_edge].next;
            }

            let outer_segment_p0 = graph.nodes[start_edge.from].p;
            let outer_segment_p1 = graph.nodes[graph.edges[end_edge].to].p;

            let projection_distance_squared;
            let projection_ratio;
            if outer_segment_p1 == outer_segment_p0 {
                projection_distance_squared = v_size2(start_point - outer_segment_p0);
                projection_ratio = 0.5; // If using this segment for subsequent opposite projection, use edge center
            } else {
                let p0_p1 = outer_segment_p1 - outer_segment_p0;
                let p0_start = start_point - outer_segment_p0;
                projection_ratio = dot(p0_start, p0_p1) as f64 / v_size2f(p0_p1);

                if projection_ratio < 0.0 {
                    projection_distance_squared = v_size2(start_point - outer_segment_p0);
                } else if projection_ratio > 1.0 {
                    projection_distance_squared = v_size2(start_point - outer_segment_p1);
                } else {
                    let projected = lerp2(outer_segment_p0, outer_segment_p1, projection_ratio);
                    projection_distance_squared = v_size2(start_point - projected);
                }
            }

            if distance_to_closest_segment_squared.is_none() || projection_distance_squared < distance_to_closest_segment_squared.unwrap() {
                trapezoidal_start = Some(e);
                trapezoidal_segments = current_trapezoidal_segments;
                projection_ratio_on_outer_segment = projection_ratio;
                distance_to_closest_segment_squared = Some(projection_distance_squared);
            }
            e = graph.edges.next(e);
        }

        let Some(trapezoidal_start) = trapezoidal_start else {
            // Could not find the trapezoid this point belongs to
            return Vec::new();
        };

        let mut remaining_inwards_length = (move_inwards_length as f64 - (distance_to_closest_segment_squared.unwrap() as f64).sqrt()) as Coord;
        if remaining_inwards_length <= 0 {
            // Start point is already far enough from the outside, no need to add an extra move
            return Vec::new();
        }

        let ts = &graph.edges[trapezoidal_start];
        // Project the start point on the edge that is at the opposite in the trapezoidal, i.e. going inwards the contour
        let opposite_projection = if trapezoidal_segments == 2 {
            // Trapezoidal is reduced to a triangle, inwards direction point towards the opposite vertex
            graph.nodes[ts.to].p
        } else {
            // Trapezoidal is a quadrilateral, project point on opposite edge
            lerp2(graph.nodes[ts.to].p, graph.nodes[graph.edges[ts.next].to].p, projection_ratio_on_outer_segment)
        };

        let mut inwards_move: Vec<Point2LL> = Vec::new();
        const IS_UPWARD_STRICT: bool = true;
        let mut next_start_node: Option<usize> = None;
        let start_to_opposite = opposite_projection - start_point;
        let distance_to_opposite_vertex = v_size(start_to_opposite);

        let normalized = |p: Point2LL| -> (f64, f64) {
            let (x, y) = (p.x as f64, p.y as f64);
            let size = (x * x + y * y).sqrt();
            if crate::utils::math::is_zero(size) {
                panic!("bad optional access: zero length direction in makeInwardsMove");
            }
            (x / size, y / size)
        };
        let to_point2ll = |x: f64, y: f64| pt(cppstd::llrint(x), cppstd::llrint(y));

        let add_edge = |edge_move_up: usize, start_position: Point2LL, remaining_inwards_length: &mut Coord, inwards_move: &mut Vec<Point2LL>| {
            let edge = &graph.edges[edge_move_up];
            let start_to_edge_end = graph.nodes[edge.to].p - start_position;
            let distance_to_edge_end = v_size(start_to_edge_end);
            let add_distance = if *remaining_inwards_length < distance_to_edge_end { *remaining_inwards_length } else { distance_to_edge_end };
            let (dx, dy) = normalized(graph.nodes[edge.to].p - graph.nodes[edge.from].p);

            inwards_move.push(start_position + to_point2ll(dx * add_distance as f64, dy * add_distance as f64));
            *remaining_inwards_length -= add_distance;
        };

        if distance_to_opposite_vertex > remaining_inwards_length {
            // Trapezoidal is long enough to contain the full inwards move, stop now
            let (dx, dy) = normalized(start_to_opposite);
            inwards_move.push(start_point + to_point2ll(dx * remaining_inwards_length as f64, dy * remaining_inwards_length as f64));
        } else {
            remaining_inwards_length -= distance_to_opposite_vertex;
            inwards_move.push(opposite_projection);
            if trapezoidal_segments == 2 {
                // Going to the opposite vertex doesn't cover the whole distance, go on
                next_start_node = Some(ts.to);
            } else {
                // Now we have reached the opposite segment, keep going inwards by following it
                let mut edge_move_up: Option<usize> = None;
                if graph.is_upward(ts.next, IS_UPWARD_STRICT) {
                    edge_move_up = Some(ts.next);
                } else if graph.is_upward(graph.edges[ts.next].twin, IS_UPWARD_STRICT) {
                    edge_move_up = Some(graph.edges[ts.next].twin);
                }

                if let Some(edge_move_up) = edge_move_up {
                    add_edge(edge_move_up, opposite_projection, &mut remaining_inwards_length, &mut inwards_move);

                    if remaining_inwards_length > 0 {
                        // Remaining part of the edge doesn't cover the whole distance, go on
                        next_start_node = Some(graph.edges[edge_move_up].to);
                    }
                }
            }
        }

        let Some(mut next_start_node) = next_start_node else {
            // We cannot ge further inside, stop here
            return inwards_move;
        };

        // Keep following skeleton segments going upwards (further away from the walls) until we have covered the desired length
        while remaining_inwards_length > 0 {
            // Find an edge starting from the current node that goes upwards
            let mut found = None;
            let mut e = graph.edges.first();
            while e != NIL {
                if graph.edges[e].from == next_start_node && graph.is_upward(e, IS_UPWARD_STRICT) {
                    found = Some(e);
                    break;
                }
                e = graph.edges.next(e);
            }
            let Some(edge_move_up) = found else {
                // We cannot move any upper
                break;
            };

            add_edge(edge_move_up, graph.nodes[graph.edges[edge_move_up].from].p, &mut remaining_inwards_length, &mut inwards_move);
            next_start_node = graph.edges[edge_move_up].to;
        }

        inwards_move
    }

    #[allow(clippy::too_many_arguments)]
    fn add_lines_in_given_order(
        &mut self,
        paths: &[PathOrdering<usize>],
        lines: &[LineRef],
        config: &GCodePathConfig,
        space_fill_type: SpaceFillType,
        wipe_dist: Coord,
        flow_ratio: Ratio,
        fan_speed: f64,
        extra_inwards_start_move_length: Coord,
        extra_inwards_end_move_length: Coord,
        extra_inwards_move_contour: Option<&MendedShape>,
    ) {
        let half_line_width = config.get_line_width() / 2;
        let line_width_2 = half_line_width * half_line_width;

        let inwards = extra_inwards_start_move_length > 0 || extra_inwards_end_move_length > 0;
        let beading_strategy;
        let trapezoidation = if inwards {
            beading_strategy = crate::beading_strategy::make_strategy(
                mm2int(0.5),
                mm2int(0.5),
                mm2int(0.4),
                std::f64::consts::PI / 4.0,
                false,
                0,
                0,
                0.5,
                0.5,
                0,
                0,
                2,
                0.5,
            );
            const DISCRETIZATION_STEP_SIZE: Coord = 800;
            let contour = extra_inwards_move_contour.expect("contour for extra inwards moves");
            Some(crate::arachne::SkeletalTrapezoidation::new(
                contour.get_shape(),
                &beading_strategy,
                AngleRadians::new(0.0),
                DISCRETIZATION_STEP_SIZE,
                0,
                0,
                0,
                0,
                crate::utils::section_type::SectionType::INFILL,
            ))
        } else {
            None
        };

        for order_idx in 0..paths.len() {
            let path = &paths[order_idx];
            let raw_polyline = lines[path.vertices];

            if !raw_polyline.is_valid() {
                continue;
            }

            let mut start_idx = path.start_vertex;
            let raw_points = raw_polyline.points();
            let mut start = raw_points[start_idx];

            let mut expanded_polyline: Option<Vec<Point2LL>> = None;
            if let Some(trapezoidation) = &trapezoidation {
                let graph = &trapezoidation.graph;
                let mut start_inwards_move =
                    if extra_inwards_start_move_length > 0 { Self::make_inwards_move(graph, start, extra_inwards_start_move_length) } else { Vec::new() };
                let end = raw_points[if path.is_closed { start_idx } else if start_idx == 0 { raw_points.len() - 1 } else { 0 }];

                let mut end_inwards_move = Vec::new();
                if extra_inwards_end_move_length == extra_inwards_start_move_length && end == start {
                    end_inwards_move = start_inwards_move.clone();
                } else if extra_inwards_end_move_length > 0 {
                    end_inwards_move = Self::make_inwards_move(graph, end, extra_inwards_end_move_length);
                }

                let mut expanded = Vec::with_capacity(raw_points.len() + 2 * start_inwards_move.len());
                if path.is_closed {
                    start_inwards_move.reverse();
                    expanded.extend_from_slice(&raw_points[..start_idx]);
                    expanded.extend_from_slice(&end_inwards_move);
                    expanded.extend_from_slice(&start_inwards_move);
                    expanded.extend_from_slice(&raw_points[start_idx..]);

                    start_idx += end_inwards_move.len();
                } else if start_idx == 0 {
                    start_inwards_move.reverse();
                    expanded.extend_from_slice(&start_inwards_move);
                    expanded.extend_from_slice(raw_points);
                    expanded.extend_from_slice(&end_inwards_move);
                } else {
                    end_inwards_move.reverse();
                    expanded.extend_from_slice(&end_inwards_move);
                    expanded.extend_from_slice(raw_points);
                    expanded.extend_from_slice(&start_inwards_move);
                    start_idx = expanded.len() - 1;
                }
                start = expanded[start_idx];
                expanded_polyline = Some(expanded);
            }
            let polyline_points: &[Point2LL] = match &expanded_polyline {
                Some(e) => e,
                None => raw_points,
            };

            if v_size2(self.get_last_planned_position_or_starting_position() - start) < line_width_2 {
                // Instead of doing a small travel that is shorter than the line width (which is generally done at pretty high jerk & move) do a
                // "fake" extrusion move
                let flow = 0.0;
                self.add_extrusion_move(Point3LL::from_2d(start, 0), config, space_fill_type, flow, 1.0, false, 1.0, fan_speed, true);
            } else {
                self.add_travel(start, false, config.z_offset);
            }

            let n = polyline_points.len();
            let mut p0 = start;
            for idx in 0..n {
                let point_idx = if path.is_closed {
                    (start_idx + idx + 1) % n
                } else if start_idx == 0 {
                    idx
                } else {
                    start_idx - idx
                };
                let p1 = polyline_points[point_idx];

                // ignore line segments that are less than 5uM long
                if v_size2(p1 - p0) >= MINIMUM_SQUARED_LINE_LENGTH {
                    self.add_extrusion_move(Point3LL::from_2d(p1, 0), config, space_fill_type, flow_ratio, 1.0, false, 1.0, fan_speed, true);
                    p0 = p1;
                }
            }

            let p1 = polyline_points[if start_idx == 0 { n - 1 } else { 0 }];
            let p0 = if n <= 1 { p1 } else { polyline_points[if start_idx == 0 { n - 2 } else { 1 }] };
            let polyline_length = match &expanded_polyline {
                Some(e) => OpenPolyline::new(e.clone()).length(),
                None => raw_polyline.length(),
            };

            // Wipe
            if wipe_dist != 0 {
                let mut wipe = true;
                let line_width = config.get_line_width() as i32;

                // Don't wipe if current extrusion is too small
                if polyline_length <= (line_width * 2) as Coord {
                    wipe = false;
                }

                // Don't wipe if next starting point is very near
                if wipe && (order_idx < paths.len() - 1) {
                    let next_path = &paths[order_idx + 1];
                    let next_p0 = next_path.converted[next_path.start_vertex];
                    if v_size2(next_p0 - p1) <= (line_width * line_width * 4) as Coord {
                        wipe = false;
                    }
                }

                if wipe {
                    let flow = 0.0;
                    self.add_extrusion_move(Point3LL::from_2d(p1 + normal(p1 - p0, wipe_dist), 0), config, space_fill_type, flow, 1.0, false, 1.0, fan_speed, true);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn add_polygons_in_given_order(
        &mut self,
        polygons: &[PathOrdering<usize>],
        config: &GCodePathConfig,
        settings: &Settings,
        wall_0_wipe_dist: Coord,
        spiralize: bool,
        flow_ratio: Ratio,
        always_retract: bool,
        reverse_order: bool,
        scarf_seam: bool,
        smooth_speed: bool,
    ) {
        let add = |this: &mut Self, path: &PathOrdering<usize>| {
            this.add_polygon(
                &path.converted,
                path.start_vertex as i32,
                path.backwards,
                settings,
                config,
                wall_0_wipe_dist,
                spiralize,
                flow_ratio,
                always_retract,
                scarf_seam,
                smooth_speed,
            );
        };
        if !reverse_order {
            for path in polygons.iter() {
                add(self, path);
            }
        } else {
            for path in polygons.iter().rev() {
                add(self, path);
            }
        }
    }

    /// Add lines to the g-code with monotonic order.
    #[allow(clippy::too_many_arguments)]
    pub fn add_lines_monotonic(
        &mut self,
        area: &Shape,
        lines: &OpenLinesSet,
        config: &GCodePathConfig,
        space_fill_type: SpaceFillType,
        monotonic_direction: AngleRadians,
        max_adjacent_distance: Coord,
        exclude_distance: Coord,
        wipe_dist: Coord,
        flow_ratio: Ratio,
        fan_speed: f64,
        interlaced: bool,
    ) {
        let exclude_areas = area.create_tube_shape(exclude_distance, exclude_distance);
        let exclude_dist2 = exclude_distance * exclude_distance;
        let last_position = self.get_last_planned_position_or_starting_position();

        // First lay all adjacent lines next to each other, to have a sensible input to the monotonic part of the algorithm.
        let mut line_order: PathOrderOptimizer<usize> = PathOrderOptimizer::simple(last_position, ZSeamConfig::default(), false, None, false);
        for (idx, line) in lines.iter().enumerate() {
            line_order.add_polyline(idx, Arc::new(line.points.clone()));
        }
        line_order.optimize(true);

        let is_inside_exclusion = |path: &OpenPolyline| -> bool {
            v_size2(path[1] - path[0]) < exclude_dist2 && exclude_areas.inside(pt((path[0].x + path[1].x) / 2, (path[0].y + path[1].y) / 2), false)
        };

        // Order monotonically, except for line-segments which stay in the excluded areas (read: close to the walls) consecutively.
        let mut order: PathOrderMonotonic<usize> = PathOrderMonotonic::new(monotonic_direction, max_adjacent_distance, last_position, interlaced);
        let mut left_over = OpenLinesSet::default();
        let mut last_would_have_been_excluded = false;
        let n = line_order.paths.len();
        for line_idx in 0..n {
            let polyline = &lines[line_order.paths[line_idx].vertices];
            if !polyline.is_valid() {
                continue;
            }

            let inside_exclusion = is_inside_exclusion(polyline);
            let next_would_have_been_included = inside_exclusion && (line_idx < n - 1 && is_inside_exclusion(&lines[line_order.paths[line_idx + 1].vertices]));
            if inside_exclusion && last_would_have_been_excluded && next_would_have_been_included {
                left_over.push(polyline.clone());
            } else {
                order.add_polyline(line_order.paths[line_idx].vertices, Arc::new(polyline.points.clone()));
            }
            last_would_have_been_excluded = inside_exclusion;
        }
        order.optimize();

        // Read out and process the monotonically ordered lines.
        let refs: Vec<LineRef> = lines.iter().map(LineRef::Open).collect();
        let ordered_paths = std::mem::take(&mut order.paths);
        self.add_lines_in_given_order(&ordered_paths, &refs, config, space_fill_type, wipe_dist, flow_ratio, fan_speed, 0, 0, None);

        // Add all lines in the excluded areas the 'normal' way.
        let near = self.get_last_planned_position_or_starting_position();
        self.add_lines_by_optimizer_open(
            &left_over,
            config,
            space_fill_type,
            LinesOpts { enable_travel_optimization: true, wipe_dist, flow_ratio, near_start_location: Some(near), fan_speed, ..Default::default() },
        );
    }

    /// Add a spiralized slice of wall that is interpolated in X/Y between `last_wall` and `wall`.
    #[allow(clippy::too_many_arguments)]
    pub fn spiralize_wall_slice(
        &mut self,
        config: &GCodePathConfig,
        wall: &Polygon,
        last_wall: &Polygon,
        seam_vertex_idx: i32,
        last_seam_vertex_idx: i32,
        is_top_layer: bool,
        is_bottom_layer: bool,
    ) {
        let smooth_contours = mesh_group_settings().get_bool("smooth_spiralized_contours");
        let spiralize = true; // In addExtrusionMove calls, enable spiralize and use nominal line width.
        let width_factor = 1.0;

        // once we are into the spiral we always start at the end point of the last layer (if any)
        let origin = if last_seam_vertex_idx >= 0 && !is_bottom_layer { last_wall[last_seam_vertex_idx as usize] } else { wall[seam_vertex_idx as usize] };
        // NOTE: this used to use addTravel_simple() but if support is being generated then combed travel is required to avoid
        // the nozzle crossing the model on its return from printing the support.
        self.add_travel(origin, false, 0);

        if !smooth_contours && last_seam_vertex_idx >= 0 {
            // when not smoothing, we get to the (unchanged) outline for this layer as quickly as possible
            let join_first_wall_at =
                la::get_closest_on_line_segment(origin, wall[(seam_vertex_idx as usize) % wall.len()], wall[(seam_vertex_idx as usize + 1) % wall.len()]);
            if v_size(join_first_wall_at - origin) > 10 {
                let flow = 1.0;
                self.add_extrusion_move(Point3LL::from_2d(join_first_wall_at, 0), config, SpaceFillType::Polygons, flow, width_factor, spiralize, 1.0, GCodePathConfig::FAN_SPEED_DEFAULT, true);
            }
        }

        let n_points = wall.len() as i32;
        let mut last_wall_polygons = Shape::default();
        last_wall_polygons.push(last_wall.clone());
        let max_dist2 = (config.get_line_width() as i32).wrapping_mul(config.get_line_width() as i32).wrapping_mul(4); // (2 * lineWidth)^2;

        let mut total_length = 0.0; // determine the length of the complete wall
        let mut p0 = origin;
        for wall_point_idx in 1..=n_points {
            let p1 = wall[((seam_vertex_idx + wall_point_idx) % n_points) as usize];
            total_length += v_size_mm(p1 - p0);
            p0 = p1;
        }

        if total_length == 0.0 {
            // nothing to do
            return;
        }

        let min_bottom_layer_flow = 0.25; // start the bottom spiral at this flow rate
        let min_top_layer_flow = 0.25; // lowest allowed flow while tapering the last spiral

        let mut speed_factor = 1.0; // may be reduced when printing the top layer so as to avoid a jump in extrusion rate as the layer starts

        if is_top_layer {
            // HACK ALERT - the last layer is approx 50% longer than the previous layer so it should take longer to print
            let layer_time_settings = self.extruder_plans.last().unwrap().fan_speed_layer_time_settings;
            let min_time = layer_time_settings.cool_min_layer_time.value();
            let normal_layer_time = total_length / config.get_speed();

            // would this layer's speed normally get reduced to satisfy the min layer time?
            if normal_layer_time < min_time {
                // yes, so the extended version will not get slowed down so much and we want to compensate for that
                let extended_layer_time = (total_length * (2.0 - min_top_layer_flow)) / config.get_speed();

                // modify the speed factor to cancel out the speed increase that would normally happen due to the longer layer time
                speed_factor = normal_layer_time / min_f(extended_layer_time, min_time);
            }
        }

        // extrude to the points following the seam vertex
        // the last point is the seam vertex as the polygon is a loop
        let mut wall_length = 0.0;
        p0 = origin;
        for wall_point_idx in 1..=n_points {
            // p is a point from the current wall polygon
            let p = wall[((seam_vertex_idx + wall_point_idx) % n_points) as usize];
            wall_length += v_size_mm(p - p0);
            p0 = p;

            let flow = if is_bottom_layer { min_bottom_layer_flow + ((1.0 - min_bottom_layer_flow) * wall_length / total_length) } else { 1.0 };

            // if required, use interpolation to smooth the x/y coordinates between layers but not for the first spiralized layer
            // as that lies directly on top of a non-spiralized wall with exactly the same outline and not for the last point in each layer
            // because we want that to be numerically exactly the same as the starting point on the next layer (not subject to any rounding)
            if smooth_contours && !is_bottom_layer && wall_point_idx < n_points {
                // now find the point on the last wall that is closest to p
                let cpp = PolygonUtils::find_closest(p, &last_wall_polygons, NO_PENALTY);

                // if we found a point and it's not further away than max_dist2, use it
                if cpp.is_valid() && v_size2(cpp.location - p) <= max_dist2 as Coord {
                    // interpolate between cpp.location and p depending on how far we have progressed along wall
                    let target = cpp.location + scale_f64(p - cpp.location, wall_length / total_length);
                    self.add_extrusion_move(Point3LL::from_2d(target, 0), config, SpaceFillType::Polygons, flow, width_factor, spiralize, speed_factor, GCodePathConfig::FAN_SPEED_DEFAULT, true);
                } else {
                    // no point in the last wall was found close enough to the current wall point so don't interpolate
                    self.add_extrusion_move(Point3LL::from_2d(p, 0), config, SpaceFillType::Polygons, flow, width_factor, spiralize, speed_factor, GCodePathConfig::FAN_SPEED_DEFAULT, true);
                }
            } else {
                // no smoothing, use point verbatim
                self.add_extrusion_move(Point3LL::from_2d(p, 0), config, SpaceFillType::Polygons, flow, width_factor, spiralize, speed_factor, GCodePathConfig::FAN_SPEED_DEFAULT, true);
            }
        }

        if is_top_layer {
            // add the tapering spiral
            let min_spiral_coast_dist = 10.0; // mm
            let mut distance_coasted = 0.0;
            wall_length = 0.0;
            let mut wall_point_idx = 1;
            while wall_point_idx <= n_points && distance_coasted < min_spiral_coast_dist {
                let p = wall[((seam_vertex_idx + wall_point_idx) % n_points) as usize];
                let seg_length = v_size_mm(p - p0);
                wall_length += seg_length;
                p0 = p;
                // flow is reduced in step with the distance travelled so the wall width should remain roughly constant
                let mut flow = 1.0 - (wall_length / total_length);
                if flow < min_top_layer_flow {
                    flow = 0.0;
                    distance_coasted += seg_length;
                }
                // reduce number of paths created when polygon has many points by limiting precision of flow
                let no_spiralize = false;
                let fan_speed = GCodePathConfig::FAN_SPEED_DEFAULT;
                let travel_to_z = false;
                self.add_extrusion_move(
                    Point3LL::from_2d(p, (self.layer_thickness as f64 / 2.0) as Coord),
                    config,
                    SpaceFillType::Polygons,
                    ((flow * 20.0) as i32) as f64 / 20.0,
                    width_factor,
                    no_spiralize,
                    speed_factor,
                    fan_speed,
                    travel_to_z,
                );
                wall_point_idx += 1;
            }
        }
    }

    /// Applying speed corrections for minimal layer times and determine the fanSpeed.
    pub fn process_fan_speed_and_minimal_layer_time(&mut self, mut starting_position: Point2LL) {
        // the minimum layer time behaviour is only applied to the last extruder.
        let mut last_extruder_nr = self.extruder_plans[0].extruder_nr;
        for plan in self.extruder_plans.iter().skip(1) {
            if last_extruder_nr < plan.extruder_nr {
                last_extruder_nr = plan.extruder_nr;
            }
        }
        let mut last_extruder_idx: usize = 0;
        let mut other_extr_plan_time = 0.0;
        let mut maximum_cool_min_layer_time = Duration::default();

        let max_overhang_length = self.max_overhang_length;
        for extr_plan_idx in 0..self.extruder_plans.len() {
            let extruder_plan = &mut self.extruder_plans[extr_plan_idx];

            // Precalculate the time estimates. Don't call this function twice, since it is works cumulative.
            extruder_plan.compute_naive_time_estimates(starting_position);
            if extruder_plan.extruder_nr == last_extruder_nr {
                last_extruder_idx = extr_plan_idx;
            } else {
                other_extr_plan_time += extruder_plan.estimates.get_total_time();
            }

            let settings = &extruder_plan.fan_speed_layer_time_settings;
            let apply_minimum_layer_time_overhang = max_overhang_length > settings.cool_min_layer_time_overhang_min_segment_length;
            let t = if apply_minimum_layer_time_overhang { settings.cool_min_layer_time_overhang } else { settings.cool_min_layer_time };
            if maximum_cool_min_layer_time < t {
                maximum_cool_min_layer_time = t;
            }

            // Modify fan speeds for the first layer(s)
            extruder_plan.process_fan_speed_for_first_layers();

            if let Some(last_path) = extruder_plan.paths.last() {
                if let Some(last_point) = last_path.points.last() {
                    starting_position = last_point.to_point2ll();
                }
            }
        }

        // apply minimum layer time behaviour
        let last_extruder_plan = &mut self.extruder_plans[last_extruder_idx];
        self.min_layer_time_used |= last_extruder_plan.force_minimal_layer_time(maximum_cool_min_layer_time.value(), other_extr_plan_time);
        last_extruder_plan.process_fan_speed_for_minimal_layer_time(maximum_cool_min_layer_time, other_extr_plan_time);
    }

    pub fn override_fan_speeds(&mut self, speed: f64) {
        for extruder_plan in self.extruder_plans.iter_mut() {
            extruder_plan.set_fan_speed(speed);
        }
    }

    /// Whether the current retracted path is to be an extruder switch retraction.
    pub fn make_retract_switch_retract(&self, extruder_plan_idx: usize, path_idx: usize) -> bool {
        let paths = &self.extruder_plans[extruder_plan_idx].paths;
        for path in paths.iter().skip(path_idx + 1) {
            if path.get_extrusion_mm3_per_mm() > 0.0 {
                return false;
            }
        }

        if self.extruder_plans.len() <= extruder_plan_idx + 1 {
            return false;
        }

        self.extruder_plans[extruder_plan_idx + 1].extruder_nr != self.extruder_plans[extruder_plan_idx].extruder_nr
    }

    /// If enabled, apply the modify plugin to the layer-plan. Without plugins the paths are unchanged,
    /// but the bookkeeping around the plugin slot is still performed.
    pub fn apply_modify_plugin(&mut self) {
        let mut handled_initial_travel = false;
        for plan_idx in 0..self.extruder_plans.len() {
            // Check if the plugin changed first_travel_destination and update it accordingly if it has
            if !handled_initial_travel {
                let mut new_destination: Option<Point2LL> = None;
                for path in self.extruder_plans[plan_idx].paths.iter() {
                    if path.is_travel_path() && !path.points.is_empty() {
                        let front = path.points[0];
                        let differs = match self.first_travel_destination {
                            None => true,
                            Some(d) => front != Point3LL::from_2d(d, 0),
                        };
                        if differs {
                            new_destination = Some(front.to_point2ll());
                        }
                        handled_initial_travel = true;
                        break;
                    }
                }
                if let Some(d) = new_destination {
                    self.first_travel_destination = Some(d);
                }
            }

            let removed_count = {
                let paths = &mut self.extruder_plans[plan_idx].paths;
                let before = paths.len();
                paths.retain(|path| !path.points.is_empty());
                before - paths.len()
            };
            if removed_count > 0 {
                log_warn!("Removed {} empty paths after plugin slot GCODE_PATHS_MODIFY was executed", removed_count);
            }
            // Ensure that the output is at least valid enough to not cause crashes.
            if self.extruder_plans[plan_idx].paths.is_empty() {
                let config = self.configs_storage.travel_config_per_extruder[self.get_extruder()];
                let reinstated_path = self.get_latest_path_with_config(&config, SpaceFillType::None, 0, 1.0, 1.0, false, 1.0);
                let dest = self.first_travel_destination.unwrap_or_else(|| self.get_last_planned_position_or_starting_position());
                self.add_travel_simple(dest, Some(reinstated_path));
            }
        }
    }

    pub fn apply_back_pressure_compensation(&mut self) {
        for extruder_plan in self.extruder_plans.iter_mut() {
            let back_pressure_compensation = extruders()[extruder_plan.extruder_nr].settings.get_ratio("speed_equalize_flow_width_factor");
            if back_pressure_compensation != 0.0 {
                extruder_plan.apply_back_pressure_compensation(back_pressure_compensation);
            }
        }
    }

    pub fn apply_gradual_flow(&mut self) {
        let layer_nr = self.layer_nr as i64 as usize;
        for extruder_plan in self.extruder_plans.iter_mut() {
            crate::gradual_flow::process(&mut extruder_plan.paths, extruder_plan.extruder_nr, layer_nr);
        }
    }

    pub fn find_first_printed_mesh(&self) -> Option<usize> {
        for extruder_plan in &self.extruder_plans {
            if let Some(mesh) = extruder_plan.find_first_printed_mesh() {
                return Some(mesh);
            }
        }
        None
    }

    pub fn get_layer_nr(&self) -> LayerIndex {
        self.layer_nr
    }

    /// Get the last planned position, or if no position has been planned yet, the user specified layer start position.
    pub fn get_last_planned_position_or_starting_position(&self) -> Point2LL {
        self.last_planned_position.unwrap_or(self.layer_start_pos_per_extruder[self.get_extruder()]).to_point2ll()
    }

    pub fn get_is_inside_mesh(&self) -> bool {
        self.was_inside
    }

    pub fn get_skirt_brim_is_planned(&self, extruder_nr: usize) -> bool {
        self.skirt_brim_is_processed[extruder_nr]
    }

    pub fn set_skirt_brim_is_planned(&mut self, extruder_nr: usize) {
        self.skirt_brim_is_processed[extruder_nr] = true;
    }

    pub fn get_extruder(&self) -> usize {
        self.extruder_plans.last().unwrap().extruder_nr
    }

    pub fn set_bridge_wall_mask(&mut self, polys: &Shape) {
        self.bridge_wall_mask = polys.clone();
        self.bridge_wall_mask_bb = AABB::from_shape(polys);
    }

    pub fn set_overhang_masks(&mut self, masks: &[OverhangMask]) {
        self.overhang_masks = masks.to_vec();
    }

    pub fn set_seam_overhang_mask(&mut self, polys: &Shape) {
        self.seam_overhang_mask = polys.clone();
    }

    pub fn get_seam_overhang_mask(&self) -> &Shape {
        &self.seam_overhang_mask
    }

    pub fn set_roofing_mask(&mut self, polys: &Shape) {
        self.roofing_mask = polys.clone();
    }

    pub fn set_flooring_mask(&mut self, shape: &Shape) {
        self.flooring_mask = shape.clone();
    }

    // ------------------------------------------------------------------------------------------
    // Writing G-code
    // ------------------------------------------------------------------------------------------

    fn compute_travel_durations(gcode: &GCodeExport, extruder: &ExtruderTrain, path: &GCodePath, z_hop_height: Coord) -> TravelDurations {
        let mut travel_durations = TravelDurations::default();

        if z_hop_height > 0 {
            let z_hop_speed = extruder.settings.get_velocity("speed_z_hop");
            travel_durations.z_hop = Duration::new((z_hop_height as f64 / z_hop_speed) / 1000.0);
        }

        if !path.points.is_empty() {
            let travel_speed = path.config.get_speed();
            let mut start_position = gcode.get_position().to_point2ll();
            let mut travel_distance: i32 = 0; // ranges::accumulate with an `int` initial value
            for travel_point in path.points.iter() {
                let travel_point_2d = travel_point.to_point2ll();
                let travel_segment_length = v_size(travel_point_2d - start_position);
                start_position = travel_point_2d;
                travel_distance = ((travel_distance as Coord) + travel_segment_length) as i32;
            }
            travel_durations.travel = Duration::new((travel_distance as f64 / travel_speed) / 1000.0);
        }

        travel_durations
    }

    fn compute_anti_ooze_amounts(
        gcode: &GCodeExport,
        extruder: &ExtruderTrain,
        path: &GCodePath,
        z_hop_height: Coord,
        retraction_config: &RetractionAndWipeConfig,
        retraction_amounts: &mut Option<TravelAntiOozing>,
        priming_amounts: &mut Option<TravelAntiOozing>,
    ) {
        // First compute the actual durations of the travel/z-hop move, as the retraction/prime will have to fit within the travel move without changing this
        let travel_durations = Self::compute_travel_durations(gcode, extruder, path, if path.perform_z_hop { z_hop_height } else { 0 });

        let rc = &retraction_config.retraction_config;
        let retract_settings = AntiOozeSettings { distance: rc.distance, speed: rc.speed, during_travel_ratio: rc.retract_during_travel };
        let prime_settings = AntiOozeSettings { distance: rc.distance + gcode.mm3_to_e(rc.prime_volume), speed: rc.prime_speed, during_travel_ratio: rc.prime_during_travel };

        let compute_base_anti_ooze_amounts = |settings: &AntiOozeSettings, intermediate_amounts: &mut AntiOozeIntermediateAmounts, amounts: &mut Option<TravelAntiOozing>| {
            intermediate_amounts.actual_during_travel_ratio = if gcode.machine_handles_retraction() { 0.0 } else { settings.during_travel_ratio };
            intermediate_amounts.total_expected_duration = Duration::new(settings.distance / settings.speed);
            let expected_duration_during_zhop_and_travel = Duration::new(intermediate_amounts.total_expected_duration.value() * intermediate_amounts.actual_during_travel_ratio);
            let diff = expected_duration_during_zhop_and_travel - travel_durations.z_hop;
            intermediate_amounts.expected_duration_during_travel = if Duration::new(0.0) < diff { diff } else { Duration::new(0.0) };
            intermediate_amounts.expected_amount_during_travel = intermediate_amounts.expected_duration_during_travel.value() * settings.speed;

            let mut a = TravelAntiOozing::default();
            let expected_distance_during_zhop_and_travel = settings.distance * intermediate_amounts.actual_during_travel_ratio;
            a.amount_while_still = settings.distance - expected_distance_during_zhop_and_travel;

            let mut actual_distance_during_zhop = 0.0;
            if travel_durations.z_hop > Duration::new(0.0) {
                let possible_distance_during_zhop = settings.speed * travel_durations.z_hop.value();
                actual_distance_during_zhop = min_f(expected_distance_during_zhop_and_travel, possible_distance_during_zhop);
                a.z_hop.ratio = actual_distance_during_zhop / possible_distance_during_zhop;
            }
            a.z_hop.amount = a.amount_while_still + actual_distance_during_zhop;
            *amounts = Some(a);
        };

        // Compute the expected retraction/priming distance and duration during travel/z-hop
        let mut intermediate_amounts_retract = AntiOozeIntermediateAmounts::default();
        let mut intermediate_amounts_prime = AntiOozeIntermediateAmounts::default();
        let mut retract_speed = retract_settings.speed;

        compute_base_anti_ooze_amounts(&retract_settings, &mut intermediate_amounts_retract, retraction_amounts);
        compute_base_anti_ooze_amounts(&prime_settings, &mut intermediate_amounts_prime, priming_amounts);

        // Now check whether we actually have enough time during z-hop + travel to fit the retraction and priming
        let total_anti_ooze_expected_duration_during_travel = intermediate_amounts_retract.expected_duration_during_travel + intermediate_amounts_prime.expected_duration_during_travel;

        let free_time_during_travel = travel_durations.travel.value() - total_anti_ooze_expected_duration_during_travel.value();
        let retraction = retraction_amounts.as_mut().unwrap();
        let priming = priming_amounts.as_mut().unwrap();
        if free_time_during_travel < 0.0 {
            // We won't have enough time to perform retraction and priming during travel, so we will have to retract less
            let reduction_ratio: Ratio = -(free_time_during_travel / total_anti_ooze_expected_duration_during_travel.value());
            let retraction_reduction_amount = intermediate_amounts_retract.expected_amount_during_travel * reduction_ratio;
            intermediate_amounts_retract.actual_amount_during_travel = intermediate_amounts_retract.expected_amount_during_travel - retraction_reduction_amount;
            let prime_reduction_amount = intermediate_amounts_prime.expected_amount_during_travel * reduction_ratio;
            intermediate_amounts_prime.actual_amount_during_travel = intermediate_amounts_prime.expected_amount_during_travel - prime_reduction_amount;

            // Whatever the settings, what matters is that this ratio always remains while we reduce the amounts globally, so reduce the stationary steps if required
            let expected_prime_retract_ratio = prime_settings.distance / retract_settings.distance;
            let reduced_retract_distance = retract_settings.distance - retraction_reduction_amount;
            let reduced_prime_distance = prime_settings.distance - prime_reduction_amount;
            let reduced_prime_ratio = reduced_prime_distance / reduced_retract_distance;
            if reduced_prime_ratio < expected_prime_retract_ratio {
                // We can't prime enough to respect the ratio, so we will have to retract less
                let expected_reduced_retract_distance = reduced_prime_distance / expected_prime_retract_ratio;
                let stationary_retract_reduction = reduced_retract_distance - expected_reduced_retract_distance;
                retraction.amount_while_still -= stationary_retract_reduction;
                retraction.z_hop.amount -= stationary_retract_reduction;
            } else if reduced_prime_ratio > expected_prime_retract_ratio {
                // We can't retract enough to respect the ratio, so we will have to prime less
                let expected_reduced_prime_distance = reduced_retract_distance * expected_prime_retract_ratio;
                let stationary_prime_reduction = reduced_prime_distance - expected_reduced_prime_distance;
                priming.amount_while_still -= stationary_prime_reduction;
                priming.z_hop.amount -= stationary_prime_reduction;
            }
        } else {
            intermediate_amounts_retract.actual_amount_during_travel = intermediate_amounts_retract.expected_amount_during_travel;
            intermediate_amounts_prime.actual_amount_during_travel = intermediate_amounts_prime.expected_amount_during_travel;

            if rc.keep_retracting_during_travel && free_time_during_travel > 0.0 {
                // We have more than enough time to do the full retract/unretract, so reduce retraction speed so that is lasts during the whole travel
                let longer_retraction_duration = intermediate_amounts_retract.expected_duration_during_travel.value() + free_time_during_travel;
                retract_speed = intermediate_amounts_retract.actual_amount_during_travel / longer_retraction_duration;
            }
        }

        retraction.amount_while_travel = retraction.z_hop.amount + intermediate_amounts_retract.actual_amount_during_travel;
        priming.amount_while_travel = priming.z_hop.amount + intermediate_amounts_prime.actual_amount_during_travel;

        Self::compute_anti_ooze_travel_split(gcode, path, retract_speed, intermediate_amounts_retract.actual_amount_during_travel, false, retraction);
        Self::compute_anti_ooze_travel_split(gcode, path, prime_settings.speed, intermediate_amounts_prime.actual_amount_during_travel, true, priming);

        // Now adjust the amounts of the prime to match the reached amounts of the retract
        let prime_delta = retraction.amount_while_travel - priming.amount_while_travel;
        priming.amount_while_travel += prime_delta;
        priming.z_hop.amount += prime_delta;
        priming.amount_while_still += prime_delta;
        for amount_by_segment in priming.amount_by_segment.iter_mut() {
            *amount_by_segment += prime_delta;
        }
    }

    fn compute_anti_ooze_travel_split(gcode: &GCodeExport, path: &GCodePath, speed: Velocity, amount_during_travel: f64, reversed: bool, anti_oozing: &mut TravelAntiOozing) {
        // Now we are going to iterate over all the points of the travel move, including the start position, so create a temporary list and fill it appropriately
        let start_position = gcode.get_position().to_point2ll();
        let mut points: Vec<Point3LL> = Vec::with_capacity(path.points.len() + 1);
        if reversed {
            points.extend(path.points.iter().rev().copied());
            points.push(Point3LL::from_2d(start_position, 0));
        } else {
            points.push(Point3LL::from_2d(start_position, 0));
            points.extend(path.points.iter().copied());
        }

        // Now loop over the segments of the travel move to find when and where the retraction/prime should stop/start
        let duration_during_travel = Duration::new(amount_during_travel / speed);
        let mut travel_duration = Duration::new(0.0);
        if points.len() < 2 {
            return;
        }
        for index in 0..points.len() - 1 {
            let segment_start = points[index].to_point2ll();
            let segment_end = points[index + 1].to_point2ll();

            let segment_duration = Duration::new((v_size(segment_end - segment_start) as f64 / path.config.get_speed()) / 1000.0);
            if (travel_duration + segment_duration) >= (duration_during_travel - Duration::new(0.001)) || index == points.len() - 2 {
                // Retraction/prime ends/starts on this segment, so calculate the intermediate position and final/start amount
                if segment_duration > Duration::new(0.001) {
                    let segment_ratio = (duration_during_travel - travel_duration).value() / segment_duration.value();
                    anti_oozing.segment_split_position = lerp2(segment_start, segment_end, segment_ratio);
                } else {
                    anti_oozing.segment_split_position = segment_start;
                }

                if reversed {
                    anti_oozing.amount_by_segment.insert(0, anti_oozing.z_hop.amount);
                } else {
                    anti_oozing.amount_by_segment.push(anti_oozing.amount_while_travel);
                }
                break;
            }

            // This segment fully contains the retraction/prime, set the proper intermediate amount and keep looping
            anti_oozing
                .amount_by_segment
                .push(cppstd::lerp(anti_oozing.z_hop.amount, anti_oozing.amount_while_travel, (travel_duration + segment_duration).value() / duration_during_travel.value()));
            travel_duration += segment_duration;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn write_travel_segment(
        &self,
        travel_retraction_state: &mut TravelRetractionState,
        gcode: &mut GCodeExport,
        path: &GCodePath,
        retraction_amounts: &Option<TravelAntiOozing>,
        priming_amounts: &Option<TravelAntiOozing>,
        speed: Velocity,
        point_index: usize,
    ) {
        match *travel_retraction_state {
            TravelRetractionState::None => {
                self.write_travel_relative_z(gcode, path.points[point_index], speed, path.z_offset, None);
            }
            TravelRetractionState::Retracting => {
                let r = retraction_amounts.as_ref().unwrap();
                if point_index == r.amount_by_segment.len().wrapping_sub(1) {
                    // This is the segment at which we should stop retracting
                    *travel_retraction_state = TravelRetractionState::Travelling;

                    // We have to split the segment in two parts, one with retraction and one without
                    self.write_travel_relative_z(gcode, Point3LL::from_2d(r.segment_split_position, 0), speed, path.z_offset, Some(r.amount_by_segment[point_index]));
                    self.write_travel_segment(travel_retraction_state, gcode, path, retraction_amounts, priming_amounts, speed, point_index);
                } else if point_index < r.amount_by_segment.len() {
                    self.write_travel_relative_z(gcode, path.points[point_index], speed, path.z_offset, Some(r.amount_by_segment[point_index]));
                } else {
                    debug_assert!(false, "The list of amounts by segment must be empty, which should not happen");
                }
            }
            TravelRetractionState::Travelling => {
                let p = priming_amounts.as_ref().unwrap();
                let point_index_reversed = path.points.len() - 1 - point_index;
                if point_index_reversed == p.amount_by_segment.len().wrapping_sub(1) {
                    // This is the segment at which we should start priming
                    *travel_retraction_state = TravelRetractionState::Priming;

                    self.write_travel_relative_z(gcode, Point3LL::from_2d(p.segment_split_position, 0), speed, path.z_offset, None);
                    self.write_travel_segment(travel_retraction_state, gcode, path, retraction_amounts, priming_amounts, speed, point_index);
                } else {
                    // Just keep travelling without retracting/priming for now
                    self.write_travel_relative_z(gcode, path.points[point_index], speed, path.z_offset, None);
                }
            }
            TravelRetractionState::Priming => {
                let p = priming_amounts.as_ref().unwrap();
                let point_index_reversed = path.points.len() - 1 - point_index;
                if point_index_reversed < p.amount_by_segment.len() {
                    self.write_travel_relative_z(gcode, path.points[point_index], speed, path.z_offset, Some(p.amount_by_segment[point_index_reversed]));
                } else {
                    debug_assert!(false, "Missing priming amount for segment");
                }
            }
        }
    }

    fn send_line_to(&self, path: &GCodePath, position: Point3LL, extrude_speed: f64, line_thickness: Option<Coord>) {
        communication().send_line_to(
            path.config.type_,
            position + Point3LL::new(0, 0, self.z + path.z_offset),
            path.get_line_width_for_layer_view(),
            line_thickness.unwrap_or(path.config.get_layer_thickness() + path.z_offset + position.z),
            extrude_speed,
        );
    }

    fn write_travel_relative_z(&self, gcode: &mut GCodeExport, position: Point3LL, speed: Velocity, path_z_offset: Coord, retract_distance: Option<f64>) {
        gcode.write_travel(position + Point3LL::new(0, 0, self.z + path_z_offset), speed, retract_distance);
    }

    #[allow(clippy::too_many_arguments)]
    fn write_extrusion_relative_z(
        &self,
        gcode: &mut GCodeExport,
        position: Point3LL,
        speed: Velocity,
        path_z_offset: Coord,
        extrusion_mm3_per_mm: f64,
        feature: PrintFeatureType,
        update_extrusion_offset: bool,
    ) {
        let thickness_factor: Ratio;
        let z_offset_start = gcode.get_position_z() - self.z;
        if z_offset_start != 0 || path_z_offset != 0 {
            // Make average flow according to Z offset, because it can't be different at start and end positions
            let thickness_factor_start = (1.0 + (z_offset_start as f64 / self.layer_thickness as f64)).clamp(0.0, 1.0);
            let thickness_factor_end = (1.0 + (path_z_offset as f64 / self.layer_thickness as f64)).clamp(0.0, 1.0);
            thickness_factor = (thickness_factor_start + thickness_factor_end) / 2.0;
        } else {
            thickness_factor = 1.0;
        }

        gcode.write_extrusion(position + Point3LL::new(0, 0, self.z + path_z_offset), speed, extrusion_mm3_per_mm * thickness_factor, feature, update_extrusion_offset);
    }

    /// Writes a path to GCode and performs coasting, or returns false if it did nothing.
    fn write_path_with_coasting(
        &mut self,
        gcode: &mut GCodeExport,
        extruder_plan_idx: usize,
        path_idx: usize,
        insert_temp_on_time: &mut dyn FnMut(&mut ExtruderPlan, &mut GCodeExport, f64, usize),
        path_coasting: &PathCoasting,
    ) -> bool {
        if path_coasting.apply_coasting == ApplyCoasting::NoCoasting {
            return false;
        }

        let coasting_start_index;
        let mut previous_position = gcode.get_position();
        let extruder_nr = self.extruder_plans[extruder_plan_idx].extruder_nr;
        let extruder = &extruders()[extruder_nr];
        // The path is not modified while writing; take a copy to allow mutable access to the extruder plan (for the inserts).
        let path = self.extruder_plans[extruder_plan_idx].paths[path_idx].clone();

        if path_coasting.apply_coasting == ApplyCoasting::CoastEntirePath {
            coasting_start_index = 0;
        } else {
            let extrude_speed = path.config.get_speed() * path.speed_factor * path.speed_back_pressure_factor;
            coasting_start_index = path_coasting.coasting_start_index;

            // write normal extrude path, followed by split extrusion to coasting starting point
            for point_idx in 0..coasting_start_index {
                let (_, time) = self.extruder_plans[extruder_plan_idx].get_point_to_point_time(&previous_position, &path.points[point_idx], &path);
                insert_temp_on_time(&mut self.extruder_plans[extruder_plan_idx], gcode, time, path_idx);

                self.write_extrusion_relative_z(gcode, path.points[point_idx], extrude_speed, path.z_offset, path.get_extrusion_mm3_per_mm(), path.config.type_, false);
                self.send_line_to(&path, path.points[point_idx], extrude_speed, None);

                previous_position = path.points[point_idx];
            }

            self.write_extrusion_relative_z(gcode, path_coasting.coasting_start_pos, extrude_speed, path.z_offset, path.get_extrusion_mm3_per_mm(), path.config.type_, false);
            self.send_line_to(&path, path_coasting.coasting_start_pos, extrude_speed, None);
        }

        // write coasting path
        for point_idx in coasting_start_index..path.points.len() {
            let (_, time) = self.extruder_plans[extruder_plan_idx].get_point_to_point_time(&previous_position, &path.points[point_idx], &path);
            insert_temp_on_time(&mut self.extruder_plans[extruder_plan_idx], gcode, time, path_idx);

            let coasting_speed_modifier = extruder.settings.get_ratio("coasting_speed");
            let speed: Velocity = coasting_speed_modifier * path.config.get_speed();
            self.write_travel_relative_z(gcode, path.points[point_idx], speed, path.z_offset, None);

            previous_position = path.points[point_idx];
        }

        true
    }

    /// Write the planned paths to gcode.
    pub fn write_gcode(&mut self, gcode: &mut GCodeExport) {
        let comm = communication();
        comm.set_layer_for_send(self.layer_nr);
        comm.send_current_position(gcode.get_position());
        gcode.set_layer_nr(self.layer_nr);

        gcode.write_layer_comment(self.layer_nr);
        if self.min_layer_time_used {
            gcode.write_comment("note -- min layer time used");
        }

        // flow-rate compensation
        let mesh_group_settings = mesh_group_settings();
        gcode.set_flow_rate_extrusion_settings(mesh_group_settings.get_f64("flow_rate_max_extrusion_offset"), mesh_group_settings.get_ratio("flow_rate_extrusion_offset_factor")); // Offset is in mm.

        let layer_1: LayerIndex = 1 - Raft::get_total_extra_layers() as LayerIndex;
        if self.layer_nr == layer_1 && mesh_group_settings.get_bool("machine_heated_bed") {
            let wait = false;
            gcode.write_bed_temperature_command(mesh_group_settings.get_temperature("material_bed_temperature"), wait);
        }
        if mesh_group_settings.get_usize("build_volume_fan_nr") != 0 {
            // The machine has a build volume fan.
            if self.layer_nr == mesh_group_settings.get_usize("build_fan_full_layer") as LayerIndex {
                let fan_speed = mesh_group_settings.get_ratio("build_volume_fan_speed") * 100.0;
                gcode.write_specific_fan_command(fan_speed, mesh_group_settings.get_usize("build_volume_fan_nr"));
            }
        }

        gcode.set_z(self.z as i32);

        let mut last_extrusion_config: Option<GCodePathConfig> = None; // used to check whether we need to insert a TYPE comment in the gcode.

        let mut extruder_nr = gcode.get_extruder_nr();
        let acceleration_enabled = mesh_group_settings.get_bool("acceleration_enabled");
        let acceleration_travel_enabled = mesh_group_settings.get_bool("acceleration_travel_enabled");
        let jerk_enabled = mesh_group_settings.get_bool("jerk_enabled");
        let jerk_travel_enabled = mesh_group_settings.get_bool("jerk_travel_enabled");
        let mut current_mesh: Option<usize> = None;
        let storage = self.storage;

        let get_retraction_config = |extruder_nr: usize, mesh: Option<usize>| -> Option<&'a RetractionAndWipeConfig> {
            match mesh {
                Some(m) => {
                    let mesh = &storage.meshes[m];
                    if extruder_nr == mesh.settings().get_usize("extruder_nr") {
                        Some(&mesh.retraction_wipe_config)
                    } else {
                        // We are printing a part of a mesh with a different extruder, use this extruder settings instead (mesh-specific settings will be ignored)
                        Some(&storage.retraction_wipe_config_per_extruder[extruder_nr])
                    }
                }
                None => None,
            }
        };

        for extruder_plan_idx in 0..self.extruder_plans.len() {
            let plan_extruder_nr = self.extruder_plans[extruder_plan_idx].extruder_nr;
            let mut retraction_config: &RetractionAndWipeConfig =
                get_retraction_config(extruder_nr, current_mesh).unwrap_or(&storage.retraction_wipe_config_per_extruder[plan_extruder_nr]);
            let mut z_hop_height = retraction_config.retraction_config.z_hop;

            if extruder_nr != plan_extruder_nr {
                let prev_extruder = extruder_nr;
                extruder_nr = plan_extruder_nr;

                gcode.reset_last_e_value_after_wipe(prev_extruder);
                gcode.write_prepare_fans_for_nozzle_switch();

                let prev_retraction_config = &storage.retraction_wipe_config_per_extruder[prev_extruder];
                if prev_retraction_config.retraction_hop_after_extruder_switch != 0 {
                    z_hop_height = prev_retraction_config.extruder_switch_retraction_config.z_hop;
                    gcode.switch_extruder(extruder_nr, &prev_retraction_config.extruder_switch_retraction_config, z_hop_height);
                } else {
                    gcode.switch_extruder(extruder_nr, &prev_retraction_config.extruder_switch_retraction_config, 0);
                }

                let required_start_temperature = self.extruder_plans[extruder_plan_idx].required_start_temperature;
                {
                    // require printing temperature to be met
                    let wait = true;
                    gcode.write_temperature_command(extruder_nr, required_start_temperature, wait, false);
                }

                if let Some(standby) = self.extruder_plans[extruder_plan_idx].prev_extruder_standby_temp {
                    // turn off previous extruder
                    let wait = false;
                    let mut prev_extruder_temp = standby;
                    let prev_layer_nr = if extruder_plan_idx == 0 { self.layer_nr - 1 } else { self.layer_nr };
                    if prev_layer_nr == storage.max_print_height_per_extruder[prev_extruder] as LayerIndex {
                        prev_extruder_temp = 0.0; // TODO ? should there be a setting for extruder_off_temperature ?
                    }
                    gcode.write_temperature_command(prev_extruder, prev_extruder_temp, wait, false);
                }

                {
                    // require printing temperature to be met
                    let wait = true;
                    gcode.write_temperature_command(extruder_nr, required_start_temperature, wait, false);
                }

                let extra_prime_amount = if retraction_config.retraction_config.distance != 0.0 { retraction_config.switch_extruder_extra_prime_amount } else { 0.0 };
                gcode.add_extra_prime_amount(extra_prime_amount);
            } else if extruder_plan_idx == 0 {
                let wipe_config = &storage.retraction_wipe_config_per_extruder[plan_extruder_nr].wipe_config;
                if wipe_config.clean_between_layers && gcode.get_extruded_volume_after_last_wipe(extruder_nr) > wipe_config.max_extrusion_mm3 {
                    gcode.insert_wipe_script(wipe_config);
                    gcode.reset_last_e_value_after_wipe(extruder_nr);
                } else if self.layer_nr != 0 && extruders()[extruder_nr].settings.get_bool("retract_at_layer_change") {
                    // only do the retract if the paths are not spiralized
                    if !mesh_group_settings.get_bool("magic_spiralize") {
                        gcode.write_retraction(&retraction_config.retraction_config, false, false, None);
                    }
                }
            }
            // Fan speed may already be set by a plugin. Prevents two fan speed commands without move in between.
            {
                let plan = &self.extruder_plans[extruder_plan_idx];
                if !plan.paths.is_empty() && plan.paths[0].fan_speed == -1.0 {
                    gcode.write_prepare_fans_for_extrusion(plan.get_fan_speed());
                }
            }

            self.extruder_plans[extruder_plan_idx].sort_inserts();

            let extruder = &extruders()[extruder_nr];

            let mut update_extrusion_offset;

            // Time in seconds since the start of the current path.
            let cumulative_path_time = std::cell::Cell::new(0.0f64);
            let mut insert_temp_on_time = |plan: &mut ExtruderPlan, gcode: &mut GCodeExport, to_add: f64, path_idx: usize| {
                cumulative_path_time.set(cumulative_path_time.get() + to_add);
                plan.handle_inserts(path_idx, gcode, cumulative_path_time.get());
            };

            let coasting_per_path = self.calculate_paths_coasting(&extruder.settings, &self.extruder_plans[extruder_plan_idx].paths, gcode.get_position());

            let n_paths = self.extruder_plans[extruder_plan_idx].paths.len();
            let mut path_idx = 0usize;
            while path_idx < n_paths {
                self.extruder_plans[extruder_plan_idx].handle_inserts(path_idx, gcode, f64::INFINITY);
                cumulative_path_time.set(0.0); // reset to 0 for current path.

                let path = self.extruder_plans[extruder_plan_idx].paths[path_idx].clone();
                debug_assert!(!path.points.is_empty());

                // If travel paths have a non default fan speed for some reason set it as fan speed as such modification could be made by a plugin.
                if !path.is_travel_path() || path.fan_speed >= 0.0 {
                    let path_fan_speed = path.get_fan_speed();
                    let plan_fan = self.extruder_plans[extruder_plan_idx].get_fan_speed();
                    gcode.write_fan_command(if path_fan_speed != GCodePathConfig::FAN_SPEED_DEFAULT { path_fan_speed } else { plan_fan }, None);
                }

                if path.perform_prime {
                    gcode.write_prime_train(extruder.settings.get_velocity("speed_travel"));
                    // Don't update cumulative path time, as ComputeNaiveTimeEstimates also doesn't.
                    gcode.write_retraction(&retraction_config.retraction_config, false, false, None);
                }

                if self.z > 0 {
                    gcode.set_z((self.z + path.z_offset) as i32);
                }

                if !path.retract
                    && path.config.is_travel_path()
                    && path.points.len() == 1
                    && path.points[0] == Point3LL::from_2d(gcode.get_position_xy(), 0)
                    && (self.z + path.z_offset) == gcode.get_position_z()
                {
                    // ignore travel moves to the current location to avoid needless change of acceleration/jerk
                    path_idx += 1;
                    continue;
                }

                // In some cases we want to find the next non-travel move.
                let mut next_extrusion_idx = path_idx + 1;
                {
                    let paths = &self.extruder_plans[extruder_plan_idx].paths;
                    if (acceleration_enabled && !acceleration_travel_enabled) || (jerk_enabled && !jerk_travel_enabled) {
                        while next_extrusion_idx < paths.len() && paths[next_extrusion_idx].config.is_travel_path() {
                            next_extrusion_idx += 1;
                        }
                    }

                    if acceleration_enabled {
                        if path.config.is_travel_path() {
                            if acceleration_travel_enabled {
                                gcode.write_travel_acceleration(path.config.get_acceleration());
                            } else {
                                // Use the acceleration of the first non-travel move *after* the travel.
                                if next_extrusion_idx >= paths.len() {
                                    // Only travel moves for the remainder of the layer.
                                    if let Some((acc, _)) = self.next_layer_acc_jerk {
                                        gcode.write_travel_acceleration(acc);
                                    } // If the next layer has no extruded move, just keep the old acceleration. Should be very rare to have an empty layer.
                                } else {
                                    gcode.write_travel_acceleration(paths[next_extrusion_idx].config.get_acceleration());
                                }
                            }
                        } else {
                            gcode.write_print_acceleration(path.config.get_acceleration());
                        }
                    }
                    if jerk_enabled {
                        if jerk_travel_enabled {
                            gcode.write_jerk(path.config.get_jerk());
                        } else {
                            // Use the jerk of the first non-travel move *after* the travel.
                            if next_extrusion_idx >= paths.len() {
                                // Only travel moves for the remainder of the layer.
                                if let Some((_, jerk)) = self.next_layer_acc_jerk {
                                    gcode.write_jerk(jerk);
                                } // If the next layer has no extruded move, just keep the old jerk. Should be very rare to have an empty layer.
                            } else {
                                gcode.write_jerk(paths[next_extrusion_idx].config.get_jerk());
                            }
                        }
                    }
                }

                let mut retraction_amounts: Option<TravelAntiOozing> = None;
                let mut priming_amounts: Option<TravelAntiOozing> = None;

                if path.retract {
                    retraction_config = get_retraction_config(extruder_nr, path.mesh).unwrap_or(retraction_config);

                    if !path.retract_for_nozzle_switch {
                        // Compute how much of the retract/prime we will have to process during stationary/Zhop/travel
                        Self::compute_anti_ooze_amounts(gcode, extruder, &path, z_hop_height, retraction_config, &mut retraction_amounts, &mut priming_amounts);
                    }

                    if !gcode.write_retraction(&retraction_config.retraction_config, false, false, retraction_amounts.as_ref().map(|r| r.amount_while_still)) {
                        // Retraction was canceled because of limitations, so also cancel retraction/priming during travel
                        retraction_amounts = None;
                        priming_amounts = None;
                    }

                    if path.retract_for_nozzle_switch {
                        let force = true;
                        let extruder_switch = true;
                        gcode.write_retraction(&retraction_config.extruder_switch_retraction_config, force, extruder_switch, None);
                    }
                    let retract_time = self.extruder_plans[extruder_plan_idx].get_retract_time(&path);
                    insert_temp_on_time(&mut self.extruder_plans[extruder_plan_idx], gcode, retract_time, path_idx);
                    if path.perform_z_hop {
                        gcode.write_zhop_start(
                            z_hop_height,
                            0.0,
                            retraction_amounts.as_ref().map(|r| r.z_hop.amount),
                            match &retraction_amounts {
                                Some(r) => r.z_hop.ratio,
                                None => 0.0,
                            },
                        );
                        z_hop_height = retraction_config.retraction_config.z_hop; // back to normal z hop
                    } else {
                        gcode.write_zhop_end(0.0, 0, None, 0.0);

                        if self.z > 0 && path.z_offset != 0 {
                            gcode.set_z((self.z + path.z_offset) as i32);
                        }
                    }
                }
                let extruder_changed = match &last_extrusion_config {
                    None => true,
                    Some(c) => c.type_ != path.config.type_,
                };
                if !path.config.is_travel_path() && extruder_changed {
                    gcode.write_type_comment(path.config.type_);
                    if path.config.is_bridge_path() {
                        gcode.write_comment("BRIDGE");
                    }
                    last_extrusion_config = Some(path.config);
                    update_extrusion_offset = true;
                } else {
                    update_extrusion_offset = false;
                }

                let mut speed = path.config.get_speed();

                // for some movements such as prime tower purge, the speed may get changed by this factor
                speed *= path.speed_factor;

                // This seems to be the best location to place this, but still not ideal.
                if path.mesh != current_mesh {
                    current_mesh = path.mesh;
                    let name = match current_mesh {
                        Some(m) => storage.meshes[m].mesh_name.as_str(),
                        None => "NONMESH",
                    };
                    gcode.write_comment(&format!("MESH:{}", name));
                }

                if !path.spiralize
                    && path.travel_to_z
                    && (!path.retract || !path.perform_z_hop)
                    && (self.z + path.z_offset + path.points[0].z != gcode.get_position_z())
                    && (path_idx > 0 || self.layer_nr > 0)
                {
                    // First move to desired height to then make a plain horizontal move
                    let pos = gcode.get_position();
                    gcode.write_travel(Point3LL::new(pos.x, pos.y, self.z + path.z_offset + path.points[0].z), speed, None);
                }

                if path.config.is_travel_path() {
                    // early comp for travel paths, which are handled more simply
                    let mut path = path;
                    if !path.perform_z_hop && self.final_travel_z != self.z && extruder_plan_idx == (self.extruder_plans.len() - 1) && path_idx == (n_paths - 1) {
                        // Before the final travel, move up to the next layer height, on the current spot, with a sensible speed.
                        let mut current_position = gcode.get_position();
                        current_position.z = self.final_travel_z;
                        gcode.write_travel(current_position, extruder.settings.get_velocity("speed_z_hop"), None);

                        // Prevent the final travel(s) from resetting to the 'previous' layer height.
                        path.z_offset = self.final_travel_z - self.z;
                        self.extruder_plans[extruder_plan_idx].paths[path_idx].z_offset = path.z_offset;
                        gcode.set_z(self.final_travel_z as i32);
                    }

                    let mut travel_retraction_state = TravelRetractionState::None;
                    if let Some(r) = &retraction_amounts {
                        if r.amount_while_travel > 0.0 {
                            travel_retraction_state = TravelRetractionState::Retracting;
                        }
                    }

                    let mut point_idx = 0;
                    while point_idx + 1 < path.points.len() {
                        self.write_travel_segment(&mut travel_retraction_state, gcode, &path, &retraction_amounts, &priming_amounts, speed, point_idx);
                        point_idx += 1;
                    }
                    if path.unretract_before_last_travel_move && self.final_travel_z == self.z {
                        // We need to unretract before the last travel move of the path if the next path is an outer wall.
                        gcode.write_unretraction_and_prime();
                    }
                    if !path.points.is_empty() {
                        self.write_travel_segment(&mut travel_retraction_state, gcode, &path, &retraction_amounts, &priming_amounts, speed, path.points.len() - 1);
                    }

                    if let Some(p) = &priming_amounts {
                        if p.z_hop.amount > 0.0 {
                            // The given amount is the one we want at the end of the zhop, thus it is actually the one at the start of the still prime
                            gcode.set_z_hop_prime_leftover(ZHopAntiOozing { amount: p.amount_while_still, ratio: p.z_hop.ratio });
                        }
                    }

                    path_idx += 1;
                    continue;
                }

                let spiralize = path.spiralize;
                if !spiralize {
                    // normal (extrusion) move (with coasting)
                    let mut coasting = extruder.settings.get_bool("coasting_enable");
                    if coasting {
                        coasting = self.write_path_with_coasting(gcode, extruder_plan_idx, path_idx, &mut insert_temp_on_time, &coasting_per_path[path_idx]);
                    }
                    if !coasting {
                        // normal path to gcode algorithm
                        let mut prev_point = gcode.get_position();
                        for &pt_ in path.points.iter() {
                            let (_, time) = self.extruder_plans[extruder_plan_idx].get_point_to_point_time(&prev_point, &pt_, &path);
                            insert_temp_on_time(&mut self.extruder_plans[extruder_plan_idx], gcode, time, path_idx);

                            let extrude_speed = speed * path.speed_back_pressure_factor;
                            self.write_extrusion_relative_z(gcode, pt_, extrude_speed, path.z_offset, path.get_extrusion_mm3_per_mm(), path.config.type_, update_extrusion_offset);
                            self.send_line_to(&path, pt_, extrude_speed, None);

                            prev_point = pt_;
                        }
                    }
                } else {
                    // SPIRALIZE
                    // If we need to spiralize then raise the head slowly by 1 layer as this path progresses.
                    let mut total_length = 0.0;
                    let mut p0 = gcode.get_position_xy();
                    {
                        let paths = &self.extruder_plans[extruder_plan_idx].paths;
                        let mut _path_idx = path_idx;
                        while _path_idx < paths.len() && !paths[_path_idx].is_travel_path() {
                            let _path = &paths[_path_idx];
                            for point in _path.points.iter() {
                                let p1 = point.to_point2ll();
                                total_length += v_size_mm(p0 - p1);
                                p0 = p1;
                            }
                            _path_idx += 1;
                        }
                    }

                    let mut length = 0.0;
                    p0 = gcode.get_position_xy();
                    let path_z_offset = path.z_offset;
                    let layer_thickness = self.layer_thickness;
                    let mut write_spiral_path = |this: &Self, gcode: &mut GCodeExport, spiral_path: &GCodePath, end_layer: bool| {
                        for p1 in spiral_path.points.iter() {
                            let p1_2d = p1.to_point2ll();
                            length += v_size_mm(p0 - p1_2d);
                            p0 = p1_2d;

                            let z_offset: Coord = if end_layer { layer_thickness / 2 } else { cppstd::round(layer_thickness as f64 * length / total_length) as Coord };
                            let extrude_speed = speed * spiral_path.speed_back_pressure_factor;
                            this.write_extrusion_relative_z(
                                gcode,
                                *p1,
                                extrude_speed,
                                path_z_offset + z_offset,
                                spiral_path.get_extrusion_mm3_per_mm(),
                                spiral_path.config.type_,
                                update_extrusion_offset,
                            );
                            this.send_line_to(spiral_path, Point3LL::new(p1.x, p1.y, z_offset), extrude_speed, Some(layer_thickness));
                        }
                    };

                    while path_idx < n_paths && self.extruder_plans[extruder_plan_idx].paths[path_idx].spiralize {
                        // handle all consecutive spiralized paths > CHANGES path_idx!
                        let not_end_layer = false;
                        let sp = self.extruder_plans[extruder_plan_idx].paths[path_idx].clone();
                        write_spiral_path(self, gcode, &sp, not_end_layer);
                        path_idx += 1;
                    }

                    if path_idx < n_paths {
                        // Handle last path & exit.
                        let end_layer = true;
                        while path_idx < n_paths {
                            let sp = self.extruder_plans[extruder_plan_idx].paths[path_idx].clone();
                            write_spiral_path(self, gcode, &sp, end_layer);
                            path_idx += 1;
                        }
                        break;
                    }
                    path_idx -= 1; // the last path_idx didnt spiralize, so it's not part of the current spiralize path
                }
                path_idx += 1;
            } // paths for this extruder

            if extruder.settings.get_bool("cool_lift_head") && self.extruder_plans[extruder_plan_idx].extra_time > 0.0 {
                gcode.write_comment("Small layer, adding delay");
                let actual_retraction_config = match current_mesh {
                    Some(m) => &storage.meshes[m].retraction_wipe_config,
                    None => &storage.retraction_wipe_config_per_extruder[gcode.get_extruder_nr()],
                };
                gcode.write_retraction(&actual_retraction_config.retraction_config, false, false, None);
                if extruder_plan_idx == self.extruder_plans.len() - 1 || !extruder.settings.get_bool("machine_extruder_end_pos_abs") {
                    // only do the z-hop if it's the last extruder plan; otherwise it's already at the switching bay area
                    // or do it anyway when we switch extruder in-place
                    gcode.write_zhop_start(3000, 0.0, None, 0.0);
                }
                gcode.write_delay(Duration::new(self.extruder_plans[extruder_plan_idx].extra_time));
            }

            self.extruder_plans[extruder_plan_idx].handle_all_remaining_inserts(gcode);
        } // extruder plans

        communication().send_layer_complete(self.layer_nr, self.z, self.layer_thickness);
        gcode.update_total_print_time();
    }
}
