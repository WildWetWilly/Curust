//! `PrimeTower/PrimeTower.h/.cpp`, `PrimeTowerNormal` and `PrimeTowerInterleaved`.

use std::collections::BTreeMap;

use crate::application::current_slice;
use crate::extruder_use::{ExtruderPrime, ExtruderUse};
use crate::geometry::{ClosedLinesSet, Point2LL, Polygon, Shape};
use crate::layer_plan::{LayerPlan, LinesOpts};
use crate::path_planning::SpaceFillType;
use crate::raft::Raft;
use crate::settings::enums::PrimeTowerMode;
use crate::settings::types::{AngleRadians, LayerIndex};
use crate::slice_data_storage::SliceDataStorage;
use crate::utils::aabb::AABB;
use crate::utils::coord::Coord;
use crate::utils::layer_vector::LayerVector;
use crate::utils::polygon_utils::PolygonUtils;

#[derive(Clone, Debug, Default)]
struct OccupiedOutline {
    outline: Polygon,
    outer_radius: Coord,
}

#[derive(Clone, Debug, Default)]
struct ExtruderToolPaths {
    extruder_nr: usize,
    toolpaths: ClosedLinesSet,
    outer_radius: Coord,
    inner_radius: Coord,
}

/// The prime tower variant (`PrimeTowerNormal` / `PrimeTowerInterleaved`).
enum Mode {
    Normal { used_extruders: Vec<usize> },
    Interleaved,
}

/// The number of vertices in each circle.
const CIRCLE_DEFINITION: usize = 32;
/// The number of segments in each arc of a wheel
const ARC_DEFINITION: usize = 4;
/// The required size of the wipe locations
const NUMBER_OF_PRIME_TOWER_START_LOCATIONS: usize = 21;

pub struct PrimeTower {
    mode: Mode,
    /// The middle of the prime tower
    middle: Point2LL,
    /// Location to post-wipe the unused nozzle off on
    post_wipe_point: Point2LL,
    /// The toolpaths to be printed, per layer, per extruder
    toolpaths: BTreeMap<LayerIndex, Vec<ExtruderToolPaths>>,
    /// The outline of the prime tower, not including the base
    outer_poly: OccupiedOutline,
    /// This is the exact outline of the extrusions lines of each layer, for layers having extra width for the base
    base_extrusion_outline: LayerVector<Polygon>,
    /// This is the approximate outline of the area filled at each layer, for layers having extra width for the base
    base_occupied_outline: LayerVector<OccupiedOutline>,
}

fn start_locations_step() -> f64 {
    AngleRadians::new((std::f64::consts::PI * 2.0) / NUMBER_OF_PRIME_TOWER_START_LOCATIONS as f64).value()
}

impl PrimeTower {
    fn new(mode: Mode) -> Self {
        let scene = &current_slice().scene;
        let mesh_group_settings = &scene.current_mesh_group().settings;
        let tower_radius = mesh_group_settings.get_coord("prime_tower_size") / 2;
        let x = mesh_group_settings.get_coord("prime_tower_position_x");
        let y = mesh_group_settings.get_coord("prime_tower_position_y");
        let layer_height = mesh_group_settings.get_coord("layer_height");
        let base_enabled = mesh_group_settings.get_bool("prime_tower_brim_enable");
        let base_extra_radius = scene.settings.get_coord("prime_tower_base_size");
        let base_height = scene.settings.get_coord("prime_tower_base_height");
        let base_curve_magnitude = mesh_group_settings.get_f64("prime_tower_base_curve_magnitude");

        let middle = Point2LL::new(x - tower_radius, y + tower_radius);
        let outer_poly = OccupiedOutline { outline: PolygonUtils::make_disc(middle, tower_radius, CIRCLE_DEFINITION), outer_radius: tower_radius };
        let post_wipe_point = Point2LL::new(x - tower_radius, y + tower_radius);

        let mut base_occupied_outline = LayerVector::default();
        // Generate the base outline
        if base_enabled && base_extra_radius > 0 && base_height > 0 {
            base_occupied_outline.init(true, 0);

            let mut z: Coord = 0;
            while z < base_height {
                let brim_radius_factor = (1.0 - z as f64 / base_height as f64).powf(base_curve_magnitude);
                let extra_radius = cppstd::llrint(base_extra_radius as f64 * brim_radius_factor);
                let total_radius = tower_radius + extra_radius;
                base_occupied_outline
                    .push(OccupiedOutline { outline: PolygonUtils::make_disc(middle, total_radius, CIRCLE_DEFINITION), outer_radius: total_radius });
                z += layer_height;
            }
        }

        PrimeTower {
            mode,
            middle,
            post_wipe_point,
            toolpaths: BTreeMap::new(),
            outer_poly,
            base_extrusion_outline: LayerVector::default(),
            base_occupied_outline,
        }
    }

    /// `createPrimeTower`
    pub fn create_prime_tower(storage: &mut SliceDataStorage) -> Option<Box<PrimeTower>> {
        let settings = &current_slice().scene.current_mesh_group().settings;
        let raft_total_extra_layers = Raft::get_total_extra_layers();
        let extruders_used = storage.get_extruders_used();

        let used_extruders_nrs: Vec<usize> = (0..extruders_used.len()).filter(|&i| extruders_used[i]).collect();

        let mut prime_tower = None;
        if used_extruders_nrs.len() > 1
            && settings.get_bool("prime_tower_enable")
            && settings.get_coord("prime_tower_min_volume") > 10
            && settings.get_coord("prime_tower_size") > 10
            && storage.max_print_height_second_to_last_extruder >= -(raft_total_extra_layers as i32)
        {
            let method: PrimeTowerMode = settings.get_enum("prime_tower_mode");
            prime_tower = Some(Box::new(match method {
                PrimeTowerMode::NORMAL => PrimeTower::new(Mode::Normal { used_extruders: used_extruders_nrs }),
                PrimeTowerMode::INTERLEAVED => PrimeTower::new(Mode::Interleaved),
            }));
        }

        if let Some(pt) = &prime_tower {
            pt.subtract_from_support(storage);
        }

        prime_tower
    }

    fn subtract_from_support(&self, storage: &mut SliceDataStorage) {
        let max_layer = storage.max_print_height_second_to_last_extruder + 1;
        let mut layer = 0usize;
        while (layer as i32) <= max_layer && layer < storage.support.support_layers.len() {
            let outside_polygon = self.get_occupied_outline(layer as LayerIndex);
            let outside_polygon_boundary_box = AABB::from_points(&outside_polygon.points);
            let mut shape = Shape::default();
            shape.push(outside_polygon.clone());
            // take the differences of the support infill parts and the prime tower area
            storage.support.support_layers[layer].exclude_areas_from_support_infill_areas(&shape, &outside_polygon_boundary_box);
            layer += 1;
        }
    }

    /// `getOccupiedOutline`: the outline of the area occupied by the prime tower on a layer.
    pub fn get_occupied_outline(&self, layer_nr: LayerIndex) -> &Polygon {
        match self.base_occupied_outline.index_at(layer_nr) {
            Some(i) => &self.base_occupied_outline.at_index(i).outline,
            None => &self.outer_poly.outline,
        }
    }

    /// `getOccupiedGroundOutline`
    pub fn get_occupied_ground_outline(&self) -> &Polygon {
        if !self.base_extrusion_outline.is_empty() {
            self.base_extrusion_outline.front()
        } else {
            &self.outer_poly.outline
        }
    }

    /// `getExtrusionOutline`
    pub fn get_extrusion_outline(&self, layer_nr: LayerIndex) -> &Polygon {
        match self.base_extrusion_outline.index_at(layer_nr) {
            Some(i) => self.base_extrusion_outline.at_index(i),
            None => &self.outer_poly.outline,
        }
    }

    fn extruder_requires_prime(extruder_is_used_on_this_layer: &[bool], extruder_nr: usize, last_extruder: usize) -> bool {
        extruder_is_used_on_this_layer[extruder_nr] && extruder_nr != last_extruder
    }

    /// `getExtruderPrime`
    pub fn get_extruder_prime(
        &self,
        extruder_is_used_on_this_layer: &[bool],
        extruder_nr: usize,
        last_extruder: usize,
        storage: &SliceDataStorage,
        layer_nr: LayerIndex,
    ) -> ExtruderPrime {
        match self.mode {
            Mode::Normal { .. } => {
                if Self::extruder_requires_prime(extruder_is_used_on_this_layer, extruder_nr, last_extruder) {
                    ExtruderPrime::Prime
                } else if layer_nr <= storage.max_print_height_second_to_last_extruder as LayerIndex {
                    ExtruderPrime::Support
                } else {
                    ExtruderPrime::None
                }
            }
            Mode::Interleaved => {
                // For now, just calculate prime or not. Support extrusion requires the whole extruders list to be
                // calculated, and will be processed later.
                if Self::extruder_requires_prime(extruder_is_used_on_this_layer, extruder_nr, last_extruder) {
                    ExtruderPrime::Prime
                } else {
                    ExtruderPrime::None
                }
            }
        }
    }

    /// `processExtrudersUse`
    pub fn process_extruders_use(&mut self, extruders_use: &mut LayerVector<Vec<ExtruderUse>>, start_extruder: usize) {
        self.polish_extruders_uses(extruders_use, start_extruder);
        self.toolpaths = self.generate_tool_paths(extruders_use);
        self.generate_base();
        self.generate_first_layer_inset();
    }

    fn polish_extruders_uses(&self, extruders_use: &mut LayerVector<Vec<ExtruderUse>>, start_extruder: usize) {
        if let Mode::Interleaved = self.mode {
            let mut last_used_extruder = start_extruder;

            // Loop through the extruders uses from bottom to top to find the last used extruder at each layer, and make sure we always have some support to print
            for extruders_use_at_layer in extruders_use.iter_mut() {
                // Make sure we always have something to print
                if extruders_use_at_layer.is_empty() {
                    extruders_use_at_layer.push(ExtruderUse { extruder_nr: last_used_extruder, prime: ExtruderPrime::Support });
                } else if extruders_use_at_layer.iter().all(|u| u.prime == ExtruderPrime::None) {
                    extruders_use_at_layer.last_mut().unwrap().prime = ExtruderPrime::Support;
                }

                last_used_extruder = extruders_use_at_layer.last().unwrap().extruder_nr;
            }
        }
    }

    fn generate_tool_paths(&self, extruders_use: &LayerVector<Vec<ExtruderUse>>) -> BTreeMap<LayerIndex, Vec<ExtruderToolPaths>> {
        match &self.mode {
            Mode::Normal { used_extruders } => self.generate_tool_paths_normal(used_extruders, extruders_use),
            Mode::Interleaved => self.generate_tool_paths_interleaved(extruders_use),
        }
    }

    fn generate_tool_paths_normal(&self, used_extruders: &[usize], extruders_use: &LayerVector<Vec<ExtruderUse>>) -> BTreeMap<LayerIndex, Vec<ExtruderToolPaths>> {
        let scene = &current_slice().scene;
        let mesh_group_settings = &scene.current_mesh_group().settings;
        let tower_radius = mesh_group_settings.get_coord("prime_tower_size") / 2;
        let mut toolpaths = BTreeMap::new();

        // First take all the used extruders numbers, unsorted
        let mut extruder_order: Vec<usize> = used_extruders.to_vec();

        // Then sort from high adhesion to low adhesion. This will give us the outside to inside extruder processing order.
        cppstd::stable_sort_by(&mut extruder_order, |&a, &b| {
            let adhesion_a = scene.extruders[a].settings.get_ratio("material_adhesion_tendency");
            let adhesion_b = scene.extruders[b].settings.get_ratio("material_adhesion_tendency");
            adhesion_a < adhesion_b
        });

        // For each extruder, generate the prime and support patterns, which will always be the same across layers
        let mut current_radius = tower_radius;
        let mut extruders_prime_toolpaths: BTreeMap<usize, ExtruderToolPaths> = BTreeMap::new();
        let mut extruders_support_toolpaths: BTreeMap<usize, ExtruderToolPaths> = BTreeMap::new();
        for &extruder_nr in &extruder_order {
            let (prime_paths, inner_radius) = self.generate_prime_toolpaths(extruder_nr, current_radius);
            let extruder_prime_toolpaths = ExtruderToolPaths { extruder_nr, toolpaths: prime_paths, outer_radius: current_radius, inner_radius };

            let mut extruder_support_toolpaths = extruder_prime_toolpaths.clone();
            extruder_support_toolpaths.toolpaths = self.generate_support_toolpaths(extruder_nr, current_radius, extruder_prime_toolpaths.inner_radius);
            extruders_support_toolpaths.insert(extruder_nr, extruder_support_toolpaths);

            current_radius = extruder_prime_toolpaths.inner_radius;
            extruders_prime_toolpaths.insert(extruder_nr, extruder_prime_toolpaths);
        }

        let position = |nr: usize| extruder_order.iter().position(|&e| e == nr).unwrap_or(extruder_order.len());

        // Now fill the extruders toolpaths according to their use
        for index in 0..extruders_use.len() {
            let layer_nr = extruders_use.layer_of(index);
            let mut extruders_use_at_layer = extruders_use.at_index(index).clone();

            // Sort to fit the global order, in order to insert the toolpaths in outside to inside order
            cppstd::stable_sort_by(&mut extruders_use_at_layer, |u1, u2| position(u1.extruder_nr) < position(u2.extruder_nr));

            // Now put the proper toolpaths for each extruder use
            let mut toolpaths_at_layer = Vec::new();
            for extruder_use in &extruders_use_at_layer {
                match extruder_use.prime {
                    ExtruderPrime::None => {}
                    ExtruderPrime::Prime => {
                        toolpaths_at_layer.push(extruders_prime_toolpaths.entry(extruder_use.extruder_nr).or_default().clone());
                    }
                    ExtruderPrime::Support => {
                        toolpaths_at_layer.push(extruders_support_toolpaths.entry(extruder_use.extruder_nr).or_default().clone());
                    }
                }
            }

            toolpaths.insert(layer_nr, toolpaths_at_layer);
        }

        toolpaths
    }

    fn generate_tool_paths_interleaved(&self, extruders_use: &LayerVector<Vec<ExtruderUse>>) -> BTreeMap<LayerIndex, Vec<ExtruderToolPaths>> {
        let mesh_group_settings = &current_slice().scene.current_mesh_group().settings;
        let tower_radius = mesh_group_settings.get_coord("prime_tower_size") / 2;
        let min_shell_thickness = mesh_group_settings.get_coord("prime_tower_min_shell_thickness");
        let mut shell_thickness: Coord = 0;
        let mut toolpaths = BTreeMap::new();

        // Loop from top bo bottom, so that the required support increases with what is actually required
        for index in (0..extruders_use.len()).rev() {
            let layer_nr = extruders_use.layer_of(index);
            let extruders_use_at_layer = extruders_use.get_ref(layer_nr);
            let mut toolpaths_at_layer: Vec<ExtruderToolPaths> = Vec::new();
            let mut last_extruder_support = 0usize;

            // Generate actual priming patterns
            let mut prime_next_outer_radius = tower_radius;
            for extruder_use in extruders_use_at_layer {
                if extruder_use.prime == ExtruderPrime::Prime {
                    let (prime_paths, inner_radius) = self.generate_prime_toolpaths(extruder_use.extruder_nr, prime_next_outer_radius);
                    toolpaths_at_layer.push(ExtruderToolPaths {
                        extruder_nr: extruder_use.extruder_nr,
                        toolpaths: prime_paths,
                        outer_radius: prime_next_outer_radius,
                        inner_radius,
                    });

                    prime_next_outer_radius = inner_radius;
                } else if extruder_use.prime == ExtruderPrime::Support {
                    last_extruder_support = extruder_use.extruder_nr;
                }
            }

            // Increase shell thickness if required
            let layer_prime_thickness = tower_radius - prime_next_outer_radius;
            shell_thickness = if shell_thickness < layer_prime_thickness { layer_prime_thickness } else { shell_thickness };

            if shell_thickness > 0 {
                shell_thickness = if shell_thickness < min_shell_thickness { min_shell_thickness } else { shell_thickness };

                // Generate extra inner support if required
                let inner_support_radius = tower_radius - shell_thickness;
                if inner_support_radius < prime_next_outer_radius {
                    if toolpaths_at_layer.is_empty() {
                        toolpaths_at_layer.push(ExtruderToolPaths {
                            extruder_nr: last_extruder_support,
                            toolpaths: ClosedLinesSet::default(),
                            outer_radius: prime_next_outer_radius,
                            inner_radius: inner_support_radius,
                        });
                    }

                    let last_extruder_nr = toolpaths_at_layer.last().unwrap().extruder_nr;
                    let support_toolpaths = self.generate_support_toolpaths(last_extruder_nr, prime_next_outer_radius, inner_support_radius);
                    let last_extruder_toolpaths = toolpaths_at_layer.last_mut().unwrap();
                    last_extruder_toolpaths.toolpaths.push_all_owned(support_toolpaths);
                    last_extruder_toolpaths.outer_radius = prime_next_outer_radius;
                    last_extruder_toolpaths.inner_radius = inner_support_radius;
                }

                toolpaths.insert(layer_nr, toolpaths_at_layer);
            }
        }

        toolpaths
    }

    /// Generates the extra annuli around the first layers of the prime tower which help make it stronger
    fn generate_base(&mut self) {
        let scene = &current_slice().scene;
        let mesh_group_settings = &scene.current_mesh_group().settings;
        let base_enabled = mesh_group_settings.get_bool("prime_tower_brim_enable");
        let base_extra_radius = scene.settings.get_coord("prime_tower_base_size");
        let base_height = scene.settings.get_coord("prime_tower_base_height");

        if base_enabled && base_extra_radius > 0 && base_height > 0 {
            self.base_extrusion_outline.init(true, 0);

            // Generate the base outside extra annuli for the first extruder of each layer
            let middle = self.middle;
            let base_count = self.base_occupied_outline.len();
            for (idx, (_, toolpaths_at_this_layer)) in self.toolpaths.iter_mut().enumerate() {
                if idx >= base_count {
                    break;
                }
                if !toolpaths_at_this_layer.is_empty() {
                    let base_outline_at_this_layer = self.base_occupied_outline.at_index(idx);
                    let first_extruder_toolpaths = &mut toolpaths_at_this_layer[0];
                    let extruder_nr = first_extruder_toolpaths.extruder_nr;
                    let line_width = scene.extruders[extruder_nr].settings.get_coord("prime_tower_line_width");

                    let (outset, outset_radius) = PolygonUtils::generate_circular_outset(
                        middle,
                        first_extruder_toolpaths.outer_radius,
                        base_outline_at_this_layer.outer_radius,
                        line_width,
                        CIRCLE_DEFINITION,
                    );
                    first_extruder_toolpaths.toolpaths.push_all_owned(outset);

                    self.base_extrusion_outline.push(PolygonUtils::make_disc(middle, outset_radius, CIRCLE_DEFINITION));
                }
            }
        }
    }

    /// Generates the extra inset used for better adhesion at the first layer
    fn generate_first_layer_inset(&mut self) {
        // Generate the base inside extra disc for the last extruder of the first layer
        let middle = self.middle;
        if let Some((_, toolpaths_first_layer)) = self.toolpaths.iter_mut().next() {
            if let Some(last_extruder_toolpaths) = toolpaths_first_layer.last_mut() {
                let scene = &current_slice().scene;
                let extruder_nr = last_extruder_toolpaths.extruder_nr;
                let line_width = scene.extruders[extruder_nr].settings.get_coord("prime_tower_line_width");
                let pattern = PolygonUtils::generate_circular_inset(middle, last_extruder_toolpaths.inner_radius, line_width, CIRCLE_DEFINITION);
                last_extruder_toolpaths.toolpaths.push_all_owned(pattern);
            }
        }
    }

    /// Generate the concentric circles to prime the given extruder, starting from the outer radius.
    fn generate_prime_toolpaths(&self, extruder_nr: usize, outer_radius: Coord) -> (ClosedLinesSet, Coord) {
        let scene = &current_slice().scene;
        let mesh_group_settings = &scene.current_mesh_group().settings;
        let layer_height = mesh_group_settings.get_coord("layer_height");
        let extruder_settings = &scene.extruders[extruder_nr].settings;
        let line_width = extruder_settings.get_coord("prime_tower_line_width");
        let required_volume = extruder_settings.get_f64("prime_tower_min_volume") * 1000000000.0;
        let flow = extruder_settings.get_ratio("prime_tower_flow");
        let semi_line_width = line_width / 2;

        let mut current_volume = 0.0;
        let mut current_outer_radius = outer_radius - semi_line_width;
        let mut toolpaths = ClosedLinesSet::default();
        while current_volume < required_volume && current_outer_radius >= semi_line_width {
            let circle = PolygonUtils::make_circle(self.middle, current_outer_radius, CIRCLE_DEFINITION);
            current_volume += (crate::geometry::Polyline::length(&circle) * line_width * layer_height) as f64 * flow;
            toolpaths.push(circle);
            current_outer_radius -= line_width;
        }

        (toolpaths, current_outer_radius + semi_line_width)
    }

    /// Generate the sparse pattern supporting the upper layers of the prime tower.
    fn generate_support_toolpaths(&self, extruder_nr: usize, outer_radius: Coord, inner_radius: Coord) -> ClosedLinesSet {
        let extruder_settings = &current_slice().scene.extruders[extruder_nr].settings;
        let max_bridging_distance = extruder_settings.get_coord("prime_tower_max_bridging_distance") as f64;
        let line_width = extruder_settings.get_coord("prime_tower_line_width");
        let radius_delta = outer_radius - inner_radius;
        let semi_line_width = line_width / 2;

        let mut toolpaths = ClosedLinesSet::default();

        // Split annuli according to max bridging distance
        let nb_annuli = (radius_delta as f64 / max_bridging_distance).ceil() as Coord;
        if nb_annuli > 0 {
            let actual_radius_step = radius_delta / nb_annuli;

            for i in 0..nb_annuli {
                let annulus_inner_radius = (inner_radius + i * actual_radius_step) + semi_line_width;
                let annulus_outer_radius = (inner_radius + (i + 1) * actual_radius_step) - semi_line_width;

                let semi_nb_spokes = ((std::f64::consts::PI * annulus_outer_radius as f64) / max_bridging_distance).ceil() as usize;

                toolpaths.push(PolygonUtils::make_wheel(self.middle, annulus_inner_radius, annulus_outer_radius, semi_nb_spokes, ARC_DEFINITION));
            }
        }

        toolpaths
    }

    /// `addToGcode`: add the prime tower toolpaths of the new extruder to the layer.
    pub fn add_to_gcode(
        &self,
        storage: &SliceDataStorage,
        gcode_layer: &mut LayerPlan,
        required_extruder_prime: &[ExtruderUse],
        prev_extruder_nr: usize,
        new_extruder_nr: usize,
    ) {
        if gcode_layer.get_prime_tower_is_planned(new_extruder_nr) {
            // don't print the prime tower if it has been printed already with this extruder.
            return;
        }

        let layer_nr = gcode_layer.get_layer_nr();
        if layer_nr > (storage.max_print_height_second_to_last_extruder + 1) as LayerIndex {
            return;
        }

        let scene = &current_slice().scene;
        let mut post_wipe = scene.extruders[prev_extruder_nr].settings.get_bool("prime_tower_wipe_enabled");

        // Do not wipe on the first layer, we will generate non-hollow prime tower there for better bed adhesion.
        if prev_extruder_nr == new_extruder_nr || layer_nr == 0 {
            post_wipe = false;
        }

        if !required_extruder_prime.iter().any(|u| u.extruder_nr == new_extruder_nr) {
            // Extruder is not used on this layer
            return;
        }

        let toolpaths = self.toolpaths.get(&layer_nr).and_then(|at_layer| at_layer.iter().find(|t| t.extruder_nr == new_extruder_nr)).map(|t| &t.toolpaths);

        if let Some(toolpaths) = toolpaths {
            if !toolpaths.is_empty() {
                self.goto_start_location(gcode_layer, new_extruder_nr);

                let config = gcode_layer.configs_storage.prime_tower_config_per_extruder[new_extruder_nr];
                gcode_layer.add_lines_by_optimizer_closed(toolpaths, &config, SpaceFillType::PolyLines, LinesOpts::default());
            }
        }

        gcode_layer.set_prime_tower_is_planned(new_extruder_nr);

        // post-wipe:
        if post_wipe {
            // Make sure we wipe the old extruder on the prime tower.
            let previous_settings = &scene.extruders[prev_extruder_nr].settings;
            let previous_nozzle_offset = Point2LL::new(previous_settings.get_coord("machine_nozzle_offset_x"), previous_settings.get_coord("machine_nozzle_offset_y"));
            let new_settings = &scene.extruders[new_extruder_nr].settings;
            let new_nozzle_offset = Point2LL::new(new_settings.get_coord("machine_nozzle_offset_x"), new_settings.get_coord("machine_nozzle_offset_y"));
            gcode_layer.add_travel(self.post_wipe_point - previous_nozzle_offset + new_nozzle_offset, false, 0);
        }
    }

    /// Travel to a start location on the outside of the prime tower, rotating per layer.
    fn goto_start_location(&self, gcode_layer: &mut LayerPlan, extruder_nr: usize) {
        let mut layer_nr = gcode_layer.get_layer_nr();
        if layer_nr != -(Raft::get_total_extra_layers() as LayerIndex) {
            let mut wipe_radius = match self.base_occupied_outline.index_at(layer_nr) {
                Some(i) => self.base_occupied_outline.at_index(i).outer_radius,
                None => self.outer_poly.outer_radius,
            };

            let train = &current_slice().scene.extruders[extruder_nr];
            wipe_radius += train.settings.get_coord("machine_nozzle_size") * 2;

            // Layer number may be negative, make it positive (or null) before using modulo operator
            while layer_nr < 0 {
                layer_nr += NUMBER_OF_PRIME_TOWER_START_LOCATIONS as LayerIndex;
            }

            let current_start_location_idx = ((extruder_nr + 1) * layer_nr as usize) % NUMBER_OF_PRIME_TOWER_START_LOCATIONS;
            let angle = AngleRadians::new(start_locations_step() * current_start_location_idx as f64);
            let prime_start = PolygonUtils::make_circle_point(self.middle, wipe_radius, angle);

            gcode_layer.add_travel(prime_start, false, 0);
        }
    }
}
