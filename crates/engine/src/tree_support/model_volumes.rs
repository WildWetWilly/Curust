//! `TreeModelVolumes.h/.cpp`: collision, avoidance, placeable and wall-restriction areas per
//! (radius, layer), computed lazily and cached.
//!
//! The C++ caches are `unordered_map`s guarded by mutexes; inserting never overwrites an existing
//! entry. Here they are `RefCell<HashMap>`s holding `Rc<Shape>` so lookups can hand out cheap
//! references while further (recursive) calculations add to the caches. All calculations run in the
//! order the single-threaded (`-m1`) reference runs them.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use cppstd::unordered::UnorderedSet;

use crate::geometry::lines_set::Shape;
use crate::settings::enums::{ESupportType, ESurfaceMode, SupportDistPriority};
use crate::settings::settings::Settings;
use crate::settings::types::LayerIndex;
use crate::slice_data_storage::{SliceDataStorage, SliceMeshStorage};
use crate::utils::coord::{mm2int, Coord, EPSILON};
use crate::utils::math::round_up_divide;
use crate::utils::simplify::Simplify;
use clipper::JoinType;

use super::settings::{cmax, cmin, AvoidanceType, RestPreference, TreeSupportSettings, HAS_TO_RELY_ON_MIN_XY_DIST_ONLY};

/// `FUDGE_LENGTH`.
pub const FUDGE_LENGTH: Coord = 50;

pub const SUPPORT_TREE_EXPONENTIAL_THRESHOLD: Coord = 1000;
pub const SUPPORT_TREE_EXPONENTIAL_FACTOR: f64 = 1.5;
pub const SUPPORT_TREE_PRE_EXPONENTIAL_STEPS: usize = 1;
pub const SUPPORT_TREE_COLLISION_RESOLUTION: Coord = 500;
pub const SUPPORT_TREE_AVOID_SUPPORT_BLOCKER: bool = true;

type RadiusLayerPair = (Coord, LayerIndex);
type Cache = RefCell<HashMap<RadiusLayerPair, Rc<Shape>>>;

/// `unordered_map::insert(range)`: never overwrites.
fn insert_all<K: std::hash::Hash + Eq, I: IntoIterator<Item = (K, Shape)>>(cache: &RefCell<HashMap<K, Rc<Shape>>>, data: I) {
    let mut c = cache.borrow_mut();
    for (k, v) in data {
        c.entry(k).or_insert_with(|| Rc::new(v));
    }
}

fn get_area(cache: &Cache, key: RadiusLayerPair) -> Option<Rc<Shape>> {
    cache.borrow().get(&key).cloned()
}

pub struct TreeModelVolumes {
    max_move: Coord,
    max_move_slow: Coord,
    min_offset_per_step: Coord,
    precalculated: bool,
    precalculation_finished: bool,
    current_outline_idx: usize,
    current_min_xy_dist: Coord,
    current_min_xy_dist_delta: Coord,
    max_layer_idx_without_blocker: LayerIndex,
    support_rests_on_model: bool,
    increase_until_radius: Coord,
    machine_border: Shape,
    machine_area: Shape,
    layer_outlines: Vec<(&'static Settings, Vec<Shape>)>,
    anti_overhang: Vec<Shape>,
    ignorable_radii: HashSet<Coord>,
    radius_0: Coord,
    support_rest_preference: RestPreference,
    collision_cache: Cache,
    collision_cache_holefree: Cache,
    accumulated_placeables_cache_radius_0: RefCell<HashMap<LayerIndex, Rc<Shape>>>,
    avoidance_cache_collision: Cache,
    avoidance_cache: Cache,
    avoidance_cache_slow: Cache,
    avoidance_cache_to_model: Cache,
    avoidance_cache_to_model_slow: Cache,
    placeable_areas_cache: Cache,
    avoidance_cache_hole: Cache,
    avoidance_cache_hole_to_model: Cache,
    wall_restrictions_cache: Cache,
    wall_restrictions_cache_min: Cache,
    simplifier: Simplify,
}

impl TreeModelVolumes {
    /// `TreeModelVolumes(storage, max_move, max_move_slow, min_offset_per_step, current_mesh_idx, ..., additional_excluded_areas)`.
    pub fn new(
        storage: &SliceDataStorage,
        max_move: Coord,
        max_move_slow: Coord,
        min_offset_per_step: Coord,
        current_mesh_idx: usize,
        additional_excluded_areas: &[Shape],
    ) -> Self {
        let machine_border_raw = storage.get_machine_border(-1);
        let n_layers = storage.support.support_layers.len();
        let mut anti_overhang = vec![Shape::default(); n_layers];
        let mut mesh_to_layeroutline_idx: HashMap<usize, usize> = HashMap::new();
        let mut layer_outlines: Vec<(&'static Settings, Vec<Shape>)> = Vec::new();

        let mut min_maximum_resolution = Coord::MAX;
        let mut min_maximum_deviation = Coord::MAX;
        let mut min_maximum_area_deviation = Coord::MAX;

        let mut support_rests_on_model = false;
        for (mesh_idx, mesh) in storage.meshes.iter().enumerate() {
            let mut added = false;
            for (idx, layer_outline) in layer_outlines.iter().enumerate() {
                if TreeSupportSettings::new(layer_outline.0).equals(&TreeSupportSettings::new(mesh.settings())) {
                    added = true;
                    mesh_to_layeroutline_idx.insert(mesh_idx, idx);
                }
            }
            if !added {
                mesh_to_layeroutline_idx.insert(mesh_idx, layer_outlines.len());
                layer_outlines.push((mesh.settings(), vec![Shape::default(); n_layers]));
            }
        }

        for data_pair in &layer_outlines {
            support_rests_on_model |= data_pair.0.get_enum::<ESupportType>("support_type") == ESupportType::EVERYWHERE;
            min_maximum_deviation = cmin(min_maximum_deviation, data_pair.0.get_coord("meshfix_maximum_deviation"));
            min_maximum_resolution = cmin(min_maximum_resolution, data_pair.0.get_coord("meshfix_maximum_resolution"));
            min_maximum_area_deviation = cmin(min_maximum_area_deviation, data_pair.0.get_coord("meshfix_maximum_extrusion_area_deviation"));
        }

        let current_outline_idx = *mesh_to_layeroutline_idx.entry(current_mesh_idx).or_insert(0);
        let config = TreeSupportSettings::new(layer_outlines[current_outline_idx].0);

        let (current_min_xy_dist, current_min_xy_dist_delta) = if config.support_overrides == SupportDistPriority::Z_OVERRIDES_XY {
            let mut d = config.xy_min_distance;
            if HAS_TO_RELY_ON_MIN_XY_DIST_ONLY.load(std::sync::atomic::Ordering::Relaxed) {
                d = cmax(d, FUDGE_LENGTH * 2);
            }
            (d, cmax(config.xy_distance - d, 0))
        } else {
            (config.xy_distance, 0)
        };
        let increase_until_radius = config.increase_radius_until_radius;

        // Retrieve all layer outlines.
        for (mesh_idx, mesh) in storage.meshes.iter().enumerate() {
            let outline_idx = mesh_to_layeroutline_idx[&mesh_idx];
            let count = layer_outlines[outline_idx].1.len() as LayerIndex;
            for layer_idx in 0..count {
                if mesh.layer_nr_max_filled_layer < layer_idx {
                    continue;
                }
                let outline = Self::extract_outline_from_mesh(mesh, layer_idx);
                layer_outlines[outline_idx].1[layer_idx as usize].push_all_owned(outline);
            }
        }
        for layer_outline in layer_outlines.iter_mut() {
            for layer_idx in 0..n_layers {
                layer_outline.1[layer_idx] = layer_outline.1[layer_idx].union_polygons();
            }
        }

        // Gather all excluded areas.
        for layer_idx in 0..n_layers {
            if layer_idx < additional_excluded_areas.len() {
                anti_overhang[layer_idx].push_all(&additional_excluded_areas[layer_idx]);
            }
            if SUPPORT_TREE_AVOID_SUPPORT_BLOCKER {
                anti_overhang[layer_idx].push_all(&storage.support.support_layers[layer_idx].anti_overhang);
            }
            if let Some(pt) = &storage.prime_tower {
                anti_overhang[layer_idx].push(pt.get_occupied_outline(layer_idx as LayerIndex).clone());
            }
            anti_overhang[layer_idx] = anti_overhang[layer_idx].union_polygons();
        }

        let mut max_layer_idx_without_blocker: LayerIndex = 0;
        while ((max_layer_idx_without_blocker + 1) as usize) < anti_overhang.len() {
            if !anti_overhang[(max_layer_idx_without_blocker + 1) as usize].is_empty() {
                break;
            }
            max_layer_idx_without_blocker += 1;
        }

        let radius_0 = config.get_radius_dtt(0, 0.0);
        let support_rest_preference = config.support_rest_preference;

        TreeModelVolumes {
            max_move: cmax(max_move.saturating_sub(2), 0),
            max_move_slow: cmax(max_move_slow.saturating_sub(2), 0),
            min_offset_per_step,
            precalculated: false,
            precalculation_finished: false,
            current_outline_idx,
            current_min_xy_dist,
            current_min_xy_dist_delta,
            max_layer_idx_without_blocker,
            support_rests_on_model,
            increase_until_radius,
            machine_border: Self::calculate_machine_border_collision(&machine_border_raw),
            machine_area: machine_border_raw,
            layer_outlines,
            anti_overhang,
            ignorable_radii: HashSet::new(),
            radius_0,
            support_rest_preference,
            collision_cache: Default::default(),
            collision_cache_holefree: Default::default(),
            accumulated_placeables_cache_radius_0: Default::default(),
            avoidance_cache_collision: Default::default(),
            avoidance_cache: Default::default(),
            avoidance_cache_slow: Default::default(),
            avoidance_cache_to_model: Default::default(),
            avoidance_cache_to_model_slow: Default::default(),
            placeable_areas_cache: Default::default(),
            avoidance_cache_hole: Default::default(),
            avoidance_cache_hole_to_model: Default::default(),
            wall_restrictions_cache: Default::default(),
            wall_restrictions_cache_min: Default::default(),
            simplifier: Simplify::new(min_maximum_resolution, min_maximum_deviation, min_maximum_area_deviation),
        }
    }

    /// `precalculate`.
    pub fn precalculate(&mut self, max_layer: Coord) {
        self.precalculated = true;
        let config = TreeSupportSettings::new(self.layer_outlines[self.current_outline_idx].0);

        let mut possible_tip_radiis: HashSet<Coord> = HashSet::new();
        for dtt in 0..=config.tip_layers {
            possible_tip_radiis.insert(self.ceil_radius(config.get_radius_dtt(dtt, 0.0)));
            possible_tip_radiis.insert(self.ceil_radius(config.get_radius_dtt(dtt, 0.0) + self.current_min_xy_dist_delta));
        }
        let mut radius_eval = self.ceil_radius(1);
        while radius_eval <= config.branch_radius {
            if !possible_tip_radiis.contains(&radius_eval) {
                self.ignorable_radii.insert(radius_eval);
            }
            radius_eval = self.ceil_radius(radius_eval + 1);
        }

        // `std::unordered_map<coord_t, LayerIndex> radius_until_layer`: the iteration order decides the
        // order in which the radii are processed, so it is emulated.
        let mut rul_keys: UnorderedSet<Coord> = UnorderedSet::new_std();
        let mut rul_vals: HashMap<Coord, LayerIndex> = HashMap::new();
        let set_if_absent = |keys: &mut UnorderedSet<Coord>, vals: &mut HashMap<Coord, LayerIndex>, k: Coord, v: LayerIndex| {
            if !keys.contains(&k) {
                keys.insert(k);
                vals.insert(k, v);
            }
        };
        for simulated_dtt in 0..=max_layer {
            let current_layer = max_layer - simulated_dtt;
            let max_regular_radius = self.ceil_radius(config.get_radius_dtt(simulated_dtt as usize, 0.0) + self.current_min_xy_dist_delta);
            let max_min_radius = self.ceil_radius(config.get_radius_dtt(simulated_dtt as usize, 0.0));
            let max_initial_layer_diameter_radius = self.ceil_radius(config.recommended_min_radius(current_layer) + self.current_min_xy_dist_delta);
            set_if_absent(&mut rul_keys, &mut rul_vals, max_regular_radius, current_layer);
            set_if_absent(&mut rul_keys, &mut rul_vals, max_min_radius, current_layer);
            set_if_absent(&mut rul_keys, &mut rul_vals, max_initial_layer_diameter_radius, current_layer);
        }
        let ordered = |keys: &UnorderedSet<Coord>, vals: &HashMap<Coord, LayerIndex>| -> Vec<RadiusLayerPair> {
            keys.iter().map(|k| (*k, vals[k])).collect()
        };
        let relevant_avoidance_radiis = ordered(&rul_keys, &rul_vals);
        let relevant_avoidance_radiis_to_model = relevant_avoidance_radiis.clone();

        let set = |keys: &mut UnorderedSet<Coord>, vals: &mut HashMap<Coord, LayerIndex>, k: Coord, v: LayerIndex| {
            keys.insert(k);
            vals.insert(k, v);
        };
        let k = self.ceil_radius_md(self.increase_until_radius, false);
        set(&mut rul_keys, &mut rul_vals, k, max_layer);
        set(&mut rul_keys, &mut rul_vals, 0, max_layer);
        if self.current_min_xy_dist_delta != 0 {
            set(&mut rul_keys, &mut rul_vals, self.current_min_xy_dist_delta, max_layer);
        }
        let relevant_collision_radiis = ordered(&rul_keys, &rul_vals);

        self.calculate_collision(&relevant_collision_radiis);

        let mut relevant_hole_collision_radiis = Vec::new();
        for &key in &relevant_avoidance_radiis {
            if key.0 < self.increase_until_radius + self.current_min_xy_dist_delta {
                relevant_hole_collision_radiis.push(key);
            }
        }
        self.calculate_collision_holefree(&relevant_hole_collision_radiis);

        if self.max_layer_idx_without_blocker < max_layer && self.support_rests_on_model {
            self.calculate_accumulated_placeable0(max_layer);
        }

        if self.support_rests_on_model {
            self.calculate_placeables(&relevant_avoidance_radiis_to_model);
        }
        if self.support_rest_preference == RestPreference::Buildplate {
            self.calculate_avoidance(&relevant_avoidance_radiis);
        }
        self.calculate_wall_restrictions(&relevant_avoidance_radiis);
        if self.support_rests_on_model {
            self.calculate_avoidance_to_model(&relevant_avoidance_radiis_to_model);
        }

        if self.max_layer_idx_without_blocker < max_layer && self.support_rests_on_model {
            self.calculate_collision_avoidance(&relevant_avoidance_radiis);
        }
        self.precalculation_finished = true;
    }

    /// `getCollision`.
    pub fn get_collision(&self, radius: Coord, layer_idx: LayerIndex, min_xy_dist: bool) -> Rc<Shape> {
        let orig_radius = radius;
        let mut radius = radius;
        if !min_xy_dist {
            radius += self.current_min_xy_dist_delta;
        }
        if orig_radius != 0 {
            radius = self.ceil_radius(radius);
        }
        let key = (radius, layer_idx);
        if let Some(r) = get_area(&self.collision_cache, key) {
            return r;
        }
        if self.precalculated {
            crate::log_warn!("Had to calculate collision at radius {} and layer {}, but precalculate was called. Performance may suffer!", key.0, key.1);
        }
        self.calculate_collision(&[key]);
        self.get_collision(orig_radius, layer_idx, min_xy_dist)
    }

    /// `getCollisionHolefree`.
    pub fn get_collision_holefree(&self, radius: Coord, layer_idx: LayerIndex, min_xy_dist: bool) -> Rc<Shape> {
        let orig_radius = radius;
        let mut radius = radius;
        if !min_xy_dist {
            radius += self.current_min_xy_dist_delta;
        }
        if radius >= self.increase_until_radius + self.current_min_xy_dist_delta {
            return self.get_collision(orig_radius, layer_idx, min_xy_dist);
        }
        let key = (radius, layer_idx);
        if let Some(r) = get_area(&self.collision_cache_holefree, key) {
            return r;
        }
        if self.precalculated {
            crate::log_warn!("Had to calculate collision holefree at radius {} and layer {}, but precalculate was called. Performance may suffer!", key.0, key.1);
        }
        self.calculate_collision_holefree(&[key]);
        self.get_collision_holefree(orig_radius, layer_idx, min_xy_dist)
    }

    /// `getAccumulatedPlaceable0`.
    pub fn get_accumulated_placeable0(&self, layer_idx: LayerIndex) -> Rc<Shape> {
        if let Some(r) = self.accumulated_placeables_cache_radius_0.borrow().get(&layer_idx) {
            return r.clone();
        }
        self.calculate_accumulated_placeable0(layer_idx);
        self.get_accumulated_placeable0(layer_idx)
    }

    /// `getAvoidance`.
    pub fn get_avoidance(&self, radius: Coord, layer_idx: LayerIndex, type_: AvoidanceType, to_model: bool, min_xy_dist: bool) -> Rc<Shape> {
        if layer_idx == 0 {
            return self.get_collision(radius, layer_idx, min_xy_dist);
        }
        let orig_radius = radius;
        let mut radius = radius + if min_xy_dist { 0 } else { self.current_min_xy_dist_delta };
        radius = self.ceil_radius(radius);
        let mut type_ = type_;
        if radius >= self.increase_until_radius + self.current_min_xy_dist_delta && type_ == AvoidanceType::FastSafe {
            type_ = AvoidanceType::Fast;
        }
        let key = (radius, layer_idx);
        let cache = match type_ {
            AvoidanceType::Fast => {
                if to_model {
                    &self.avoidance_cache_to_model
                } else {
                    &self.avoidance_cache
                }
            }
            AvoidanceType::Slow => {
                if to_model {
                    &self.avoidance_cache_to_model_slow
                } else {
                    &self.avoidance_cache_slow
                }
            }
            AvoidanceType::FastSafe => {
                if to_model {
                    &self.avoidance_cache_hole_to_model
                } else {
                    &self.avoidance_cache_hole
                }
            }
            AvoidanceType::Collision => {
                if layer_idx <= self.max_layer_idx_without_blocker {
                    return self.get_collision(radius, layer_idx, true);
                }
                &self.avoidance_cache_collision
            }
        };
        if let Some(r) = get_area(cache, key) {
            return r;
        }
        if self.precalculated {
            crate::log_warn!(
                "Had to calculate Avoidance (to model-bool: {}) at radius {} and layer {} and type {}, but precalculate was called. Performance may suffer!",
                to_model,
                key.0,
                key.1,
                type_ as i64
            );
        }
        if type_ == AvoidanceType::Collision {
            self.calculate_collision_avoidance(&[key]);
        } else if to_model {
            self.calculate_avoidance_to_model(&[key]);
        } else {
            self.calculate_avoidance(&[key]);
        }
        self.get_avoidance(orig_radius, layer_idx, type_, to_model, min_xy_dist)
    }

    /// `getPlaceableAreas`.
    pub fn get_placeable_areas(&self, radius: Coord, layer_idx: LayerIndex) -> Rc<Shape> {
        let orig_radius = radius;
        let radius = self.ceil_radius(radius);
        let key = (radius, layer_idx);
        if let Some(r) = get_area(&self.placeable_areas_cache, key) {
            return r;
        }
        if self.precalculated {
            crate::log_warn!("Had to calculate Placeable Areas at radius {} and layer {}, but precalculate was called. Performance may suffer!", radius, layer_idx);
        }
        if radius != 0 {
            self.calculate_placeables(&[key]);
        } else {
            self.get_collision(0, layer_idx, true);
        }
        self.get_placeable_areas(orig_radius, layer_idx)
    }

    /// `getWallRestriction`.
    pub fn get_wall_restriction(&self, radius: Coord, layer_idx: LayerIndex, min_xy_dist: bool) -> Rc<Shape> {
        if layer_idx == 0 {
            return self.get_collision(radius, layer_idx, min_xy_dist);
        }
        let orig_radius = radius;
        let min_xy_dist = min_xy_dist && self.current_min_xy_dist_delta > 0;
        let radius = self.ceil_radius(radius);
        let key = (radius, layer_idx);
        let cache = if min_xy_dist { &self.wall_restrictions_cache_min } else { &self.wall_restrictions_cache };
        if let Some(r) = get_area(cache, key) {
            return r;
        }
        if self.precalculated {
            crate::log_warn!("Had to calculate Wall restrictions at radius {} and layer {}, but precalculate was called. Performance may suffer!", key.0, key.1);
        }
        self.calculate_wall_restrictions(&[key]);
        self.get_wall_restriction(orig_radius, layer_idx, min_xy_dist)
    }

    /// `ceilRadius(radius, min_xy_dist)`.
    pub fn ceil_radius_md(&self, radius: Coord, min_xy_dist: bool) -> Coord {
        self.ceil_radius(radius + if min_xy_dist { 0 } else { self.current_min_xy_dist_delta })
    }

    /// `getRadiusNextCeil`.
    pub fn get_radius_next_ceil(&self, radius: Coord, min_xy_dist: bool) -> Coord {
        self.ceil_radius_md(radius, min_xy_dist) - if min_xy_dist { 0 } else { self.current_min_xy_dist_delta }
    }

    /// `extractOutlineFromMesh`.
    fn extract_outline_from_mesh(mesh: &SliceMeshStorage, layer_idx: LayerIndex) -> Shape {
        let settings = mesh.settings();
        let mut total = Shape::default();
        if settings.get_bool("infill_mesh") || settings.get_bool("anti_overhang_mesh") {
            return Shape::default();
        }
        let layer = &mesh.layers[layer_idx as usize];
        layer.get_outlines_into(&mut total, false);
        if settings.get_enum::<ESurfaceMode>("magic_mesh_surface_mode") != ESurfaceMode::NORMAL {
            total = total.union_polygons_other(&layer.open_polylines.offset_default(FUDGE_LENGTH * 2));
        }
        let maximum_resolution = settings.get_coord("meshfix_maximum_resolution");
        let maximum_deviation = settings.get_coord("meshfix_maximum_deviation");
        let maximum_area_deviation = settings.get_coord("meshfix_maximum_extrusion_area_deviation");
        Simplify::new(maximum_resolution, maximum_deviation, maximum_area_deviation).shape(&total)
    }

    /// `getMaxCalculatedLayer`.
    fn get_max_calculated_layer(radius: Coord, map: &Cache) -> LayerIndex {
        let map = map.borrow();
        let mut max_layer: LayerIndex = -1;
        if map.contains_key(&(radius, 1)) {
            max_layer = 1;
        }
        while map.contains_key(&(radius, max_layer + 1)) {
            max_layer += 1;
        }
        max_layer
    }

    /// `calculateCollision`.
    fn calculate_collision(&self, keys: &[RadiusLayerPair]) {
        for &key_i in keys {
            let radius = key_i.0;
            let mut data_outer: HashMap<LayerIndex, Shape> = HashMap::new();
            let mut data_placeable_outer: HashMap<LayerIndex, Shape> = HashMap::new();
            for outline_idx in 0..self.layer_outlines.len() {
                // Keyed by layer (the radius is the same for every entry).
                let mut data: HashMap<LayerIndex, Shape> = HashMap::new();
                let mut data_placeable: HashMap<LayerIndex, Shape> = HashMap::new();

                let s = self.layer_outlines[outline_idx].0;
                let outlines = &self.layer_outlines[outline_idx].1;
                let layer_height = s.get_coord("layer_height");
                let support_rests_on_this_model = s.get_enum::<ESupportType>("support_type") == ESupportType::EVERYWHERE;
                let z_distance_bottom = s.get_coord("support_bottom_distance");
                let z_distance_bottom_layers = round_up_divide(z_distance_bottom as u64, layer_height as u64) as usize;
                let z_distance_top_layers = round_up_divide(s.get_coord("support_top_distance") as u64, layer_height as u64) as Coord;
                let max_anti_overhang_layer = self.anti_overhang.len() as LayerIndex - 1;
                let max_required_layer = key_i.1 + cmax(1, z_distance_top_layers);
                let xy_distance = if outline_idx == self.current_outline_idx { self.current_min_xy_dist } else { s.get_coord("support_xy_distance") };
                let mut min_layer_bottom = Self::get_max_calculated_layer(radius, &self.collision_cache) - z_distance_bottom_layers as i64;
                if min_layer_bottom < 0 {
                    min_layer_bottom = 0;
                }
                for layer_idx in min_layer_bottom..=max_required_layer {
                    let mut collision_areas = self.machine_border.clone();
                    if (layer_idx as usize) < outlines.len() {
                        collision_areas.push_all(&outlines[layer_idx as usize]);
                    }
                    collision_areas = collision_areas.offset_default(radius + xy_distance);
                    data.entry(layer_idx).or_default().push_all_owned(collision_areas);
                }

                // Add layers below, to ensure correct support_bottom_distance. Also save placeable areas of radius 0.
                for layer_idx in (min_layer_bottom..=max_required_layer).rev() {
                    let mut layer_offset: usize = 1;
                    while layer_offset <= z_distance_bottom_layers && layer_idx - layer_offset as i64 > min_layer_bottom {
                        let below = data.entry(layer_idx - layer_offset as i64).or_default().clone();
                        data.entry(layer_idx).or_default().push_all_owned(below);
                        layer_offset += 1;
                    }
                    if (support_rests_on_this_model || self.precalculation_finished || !self.precalculated) && radius == 0 && layer_idx < 1 + key_i.1 {
                        let unioned = data.entry(layer_idx).or_default().union_polygons();
                        data.insert(layer_idx, unioned);
                        let mut above = data.entry(layer_idx + 1).or_default().clone();
                        above = above.union_polygons_other(&if max_anti_overhang_layer >= layer_idx + 1 {
                            self.anti_overhang[layer_idx as usize].clone()
                        } else {
                            Shape::default()
                        });
                        let placeable = data[&layer_idx].union_polygons().difference(&above);
                        let e = data_placeable.entry(layer_idx + 1).or_default();
                        *e = e.union_polygons_other(&placeable);
                    }
                }

                // Add collision layers above to ensure correct support_top_distance.
                for layer_idx in min_layer_bottom..=max_required_layer {
                    let mut layer_offset: Coord = 1;
                    while layer_offset <= z_distance_top_layers && layer_offset + layer_idx < cmin(outlines.len() as Coord, max_required_layer + 1) {
                        let required_range_x = (xy_distance as f64
                            - ((layer_offset as f64 - if z_distance_top_layers == 1 { 0.5 } else { 0.0 }) * xy_distance as f64
                                / z_distance_top_layers as f64)) as Coord;
                        let add = outlines[(layer_idx + layer_offset) as usize].offset_default(radius + required_range_x);
                        data.entry(layer_idx).or_default().push_all_owned(add);
                        layer_offset += 1;
                    }
                    let other = if max_anti_overhang_layer >= layer_idx { self.anti_overhang[layer_idx as usize].offset_default(radius) } else { Shape::default() };
                    let e = data.entry(layer_idx).or_default();
                    *e = e.union_polygons_other(&other);
                }

                for layer_idx in ((key_i.1 + 1)..=max_required_layer).rev() {
                    data.remove(&layer_idx);
                }

                for (k, v) in data {
                    let v = self.simplifier.shape(&v);
                    let e = data_outer.entry(k).or_default();
                    *e = e.union_polygons_other(&v);
                }
                if radius == 0 {
                    for (k, v) in data_placeable {
                        let v = self.simplifier.shape(&v);
                        let e = data_placeable_outer.entry(k).or_default();
                        *e = e.union_polygons_other(&v);
                    }
                }
            }

            insert_all(&self.collision_cache, data_outer.into_iter().map(|(l, s)| ((radius, l), s)));
            if radius == 0 {
                insert_all(&self.placeable_areas_cache, data_placeable_outer.into_iter().map(|(l, s)| ((radius, l), s)));
            }
        }
    }

    /// `calculateCollisionHolefree`.
    fn calculate_collision_holefree(&self, keys: &[RadiusLayerPair]) {
        let mut max_layer: LayerIndex = 0;
        for key in keys {
            max_layer = cmax(max_layer, key.1);
        }
        for layer_idx in 0..=max_layer {
            let mut data: Vec<(RadiusLayerPair, Shape)> = Vec::new();
            for key in keys {
                let radius = key.0;
                let increase_radius_ceil = self.ceil_radius_md(self.increase_until_radius, false) - self.ceil_radius_md(radius, true);
                let mut col = self
                    .get_collision(self.increase_until_radius, layer_idx, false)
                    .offset_join(EPSILON - increase_radius_ceil, JoinType::Round)
                    .union_polygons();
                col = self.simplifier.shape(&col);
                let k = (radius, layer_idx);
                if let Some(e) = data.iter_mut().find(|(kk, _)| *kk == k) {
                    e.1 = col;
                } else {
                    data.push((k, col));
                }
            }
            insert_all(&self.collision_cache_holefree, data);
        }
    }

    /// `calculateAccumulatedPlaceable0`.
    fn calculate_accumulated_placeable0(&self, max_layer: LayerIndex) {
        let mut start_layer: LayerIndex = -1;
        {
            let c = self.accumulated_placeables_cache_radius_0.borrow();
            while c.contains_key(&(start_layer + 1)) {
                start_layer += 1;
            }
        }
        start_layer = cmax(start_layer + 1, 1);
        if start_layer > max_layer {
            crate::log_debug!("Requested calculation for value already calculated ?");
            return;
        }
        let mut accumulated_placeable_0 = if start_layer == 1 {
            self.machine_area.clone()
        } else {
            self.get_accumulated_placeable0(start_layer - 1)
                .offset_default(FUDGE_LENGTH + (self.current_min_xy_dist + self.current_min_xy_dist_delta))
        };
        let mut data: Vec<(LayerIndex, Shape)> = vec![(-1, Shape::default()); (max_layer + 1) as usize];
        for layer in start_layer..=max_layer {
            accumulated_placeable_0 = accumulated_placeable_0
                .union_polygons_other(&self.get_placeable_areas(0, layer).offset_default(FUDGE_LENGTH))
                .difference(&self.anti_overhang[layer as usize]);
            accumulated_placeable_0 = self.simplifier.shape(&accumulated_placeable_0);
            data[layer as usize] = (layer, accumulated_placeable_0.clone());
        }
        let from = cmax(start_layer - 1, 1) as usize;
        for layer_idx in from..data.len() {
            data[layer_idx].1 = data[layer_idx].1.offset_default(-(self.current_min_xy_dist + self.current_min_xy_dist_delta));
        }
        insert_all(&self.accumulated_placeables_cache_radius_0, data);
    }

    /// `calculateCollisionAvoidance`.
    fn calculate_collision_avoidance(&self, keys: &[RadiusLayerPair]) {
        for &key in keys {
            let radius = key.0;
            let max_required_layer = key.1;
            let max_step_move = cmax(1.9 * radius as f64, self.current_min_xy_dist as f64 * 1.9) as Coord;
            let start_layer = 1 + cmax(Self::get_max_calculated_layer(radius, &self.avoidance_cache_collision), self.max_layer_idx_without_blocker);
            if start_layer > max_required_layer {
                continue;
            }
            let mut data: Vec<(RadiusLayerPair, Shape)> = vec![((radius, -1), Shape::default()); (max_required_layer + 1) as usize];
            let mut latest_avoidance: Shape = (*self.get_avoidance(radius, start_layer - 1, AvoidanceType::Collision, true, true)).clone();
            for layer in start_layer..=max_required_layer {
                let col = self.get_collision(radius, layer, true);
                latest_avoidance = self.safe_offset(&latest_avoidance, -self.max_move, JoinType::Round, -max_step_move, &col);
                let placeable0_radius_compensated =
                    self.get_accumulated_placeable0(layer).offset_join(-cmax(radius, self.increase_until_radius), JoinType::Round);
                latest_avoidance =
                    latest_avoidance.difference(&placeable0_radius_compensated).union_polygons_other(&self.get_collision(radius, layer, true));
                let next_latest_avoidance = self.simplifier.shape(&latest_avoidance);
                latest_avoidance = next_latest_avoidance.union_polygons_other(&latest_avoidance);
                data[layer as usize] = ((radius, layer), latest_avoidance.clone());
            }
            insert_all(&self.avoidance_cache_collision, data);
        }
    }

    /// `safeOffset`.
    fn safe_offset(&self, me: &Shape, distance: Coord, jt: JoinType, max_safe_step_distance: Coord, collision: &Shape) -> Shape {
        let steps = (distance / cmax(self.min_offset_per_step, max_safe_step_distance.abs())).unsigned_abs();
        let mut ret = me.clone();
        for _ in 0..steps {
            ret = ret.offset_join(max_safe_step_distance, jt).union_polygons_other(collision);
        }
        ret = ret.offset_join(distance % max_safe_step_distance, jt);
        ret.union_polygons_other(collision)
    }

    fn avoidance_cache_for(&self, slow: bool, holefree: bool, to_model: bool) -> &Cache {
        match (slow, holefree, to_model) {
            (true, _, false) => &self.avoidance_cache_slow,
            (false, true, false) => &self.avoidance_cache_hole,
            (false, false, false) => &self.avoidance_cache,
            (true, _, true) => &self.avoidance_cache_to_model_slow,
            (false, true, true) => &self.avoidance_cache_hole_to_model,
            (false, false, true) => &self.avoidance_cache_to_model,
        }
    }

    /// `calculateAvoidance`.
    fn calculate_avoidance(&self, keys: &[RadiusLayerPair]) {
        const ALL_TYPES: [AvoidanceType; 3] = [AvoidanceType::Slow, AvoidanceType::FastSafe, AvoidanceType::Fast];
        for iter_idx in 0..keys.len() * 3 {
            let key_idx = iter_idx / 3;
            let type_ = ALL_TYPES[iter_idx % 3];
            let slow = type_ == AvoidanceType::Slow;
            let holefree = type_ == AvoidanceType::FastSafe;
            let radius = keys[key_idx].0;
            let max_required_layer = keys[key_idx].1;
            if holefree && radius >= self.increase_until_radius + self.current_min_xy_dist_delta {
                continue;
            }
            let offset_speed = if slow { self.max_move_slow } else { self.max_move };
            let max_step_move = cmax(1.9 * radius as f64, self.current_min_xy_dist as f64 * 1.9) as Coord;
            let cache = self.avoidance_cache_for(slow, holefree, false);
            let mut start_layer = 1 + Self::get_max_calculated_layer(radius, cache);
            if start_layer > max_required_layer {
                crate::log_debug!("Requested calculation for value already calculated ?");
                continue;
            }
            start_layer = cmax(start_layer, 1);
            let mut data: Vec<(RadiusLayerPair, Shape)> = vec![((radius, -1), Shape::default()); (max_required_layer + 1) as usize];
            let mut latest_avoidance: Shape = (*self.get_avoidance(radius, start_layer - 1, type_, false, true)).clone();
            for layer in start_layer..=max_required_layer {
                let col = if (slow && radius < self.increase_until_radius + self.current_min_xy_dist_delta) || holefree {
                    self.get_collision_holefree(radius, layer, true)
                } else {
                    self.get_collision(radius, layer, true)
                };
                latest_avoidance = self.safe_offset(&latest_avoidance, -offset_speed, JoinType::Round, -max_step_move, &col);
                let next_latest_avoidance = self.simplifier.shape(&latest_avoidance);
                latest_avoidance = next_latest_avoidance.union_polygons_other(&latest_avoidance);
                data[layer as usize] = ((radius, layer), latest_avoidance.clone());
            }
            insert_all(cache, data);
        }
    }

    /// `calculatePlaceables`.
    fn calculate_placeables(&self, keys: &[RadiusLayerPair]) {
        for &key_in in keys {
            let radius = key_in.0;
            let max_required_layer = key_in.1;
            let mut data: Vec<(RadiusLayerPair, Shape)> = vec![((radius, -1), Shape::default()); (max_required_layer + 1) as usize];
            let mut start_layer = 1 + Self::get_max_calculated_layer(radius, &self.placeable_areas_cache);
            if start_layer > max_required_layer {
                crate::log_debug!("Requested calculation for value already calculated ?");
                continue;
            }
            if start_layer == 0 {
                data[0] = ((radius, 0), self.machine_border.difference(&self.get_collision(radius, 0, true)));
                start_layer = 1;
            }
            for layer in start_layer..=max_required_layer {
                let mut placeable: Shape = (*self.get_placeable_areas(0, layer)).clone();
                placeable = self.simplifier.shape(&placeable);
                placeable = placeable.offset_default(-(radius + (self.current_min_xy_dist + self.current_min_xy_dist_delta))).union_polygons();
                data[layer as usize] = ((radius, layer), placeable);
            }
            insert_all(&self.placeable_areas_cache, data);
        }
    }

    /// `calculateAvoidanceToModel`.
    fn calculate_avoidance_to_model(&self, keys: &[RadiusLayerPair]) {
        const ALL_TYPES: [AvoidanceType; 3] = [AvoidanceType::Slow, AvoidanceType::FastSafe, AvoidanceType::Fast];
        for iter_idx in 0..keys.len() * 3 {
            let key_idx = iter_idx / 3;
            let type_ = ALL_TYPES[iter_idx % 3];
            let slow = type_ == AvoidanceType::Slow;
            let holefree = type_ == AvoidanceType::FastSafe;
            let radius = keys[key_idx].0;
            let max_required_layer = keys[key_idx].1;
            if holefree && radius >= self.increase_until_radius + self.current_min_xy_dist_delta {
                continue;
            }
            let offset_speed = if slow { self.max_move_slow } else { self.max_move };
            let max_step_move = cmax(1.9 * radius as f64, self.current_min_xy_dist as f64 * 1.9) as Coord;
            let mut data: Vec<(RadiusLayerPair, Shape)> = vec![((radius, -1), Shape::default()); (max_required_layer + 1) as usize];
            let cache = self.avoidance_cache_for(slow, holefree, true);
            let mut start_layer = 1 + Self::get_max_calculated_layer(radius, cache);
            start_layer = cmax(start_layer, 1);
            if start_layer > max_required_layer {
                crate::log_debug!("Requested calculation for value already calculated or max_required_layer is 0?");
                continue;
            }
            self.get_placeable_areas(radius, max_required_layer);
            let mut latest_avoidance: Shape = (*self.get_avoidance(radius, start_layer - 1, type_, true, true)).clone();
            for layer in start_layer..=max_required_layer {
                let _ = self.get_collision(radius, layer, true);
                let col = if (slow && radius < self.increase_until_radius + self.current_min_xy_dist_delta) || holefree {
                    self.get_collision_holefree(radius, layer, true)
                } else {
                    self.get_collision(radius, layer, true)
                };
                latest_avoidance = self
                    .safe_offset(&latest_avoidance, -offset_speed, JoinType::Round, -max_step_move, &col)
                    .difference(&self.get_placeable_areas(radius, layer));
                let next_latest_avoidance = self.simplifier.shape(&latest_avoidance);
                latest_avoidance = next_latest_avoidance.union_polygons_other(&latest_avoidance);
                data[layer as usize] = ((radius, layer), latest_avoidance.clone());
            }
            insert_all(cache, data);
        }
    }

    /// `calculateWallRestrictions`.
    fn calculate_wall_restrictions(&self, keys: &[RadiusLayerPair]) {
        for &key_in in keys {
            let radius = key_in.0;
            let mut data: Vec<(RadiusLayerPair, Shape)> = Vec::new();
            let mut data_min: Vec<(RadiusLayerPair, Shape)> = Vec::new();
            let mut min_layer_bottom = Self::get_max_calculated_layer(radius, &self.wall_restrictions_cache);
            if min_layer_bottom < 1 {
                min_layer_bottom = 1;
            }
            for layer_idx in min_layer_bottom..=key_in.1 {
                let layer_idx_below = layer_idx - 1;
                let wall_restriction =
                    self.simplifier.shape(&self.get_collision(0, layer_idx, false).intersection(&self.get_collision(radius, layer_idx_below, true)));
                data.push(((radius, layer_idx), wall_restriction));
                if self.current_min_xy_dist_delta > 0 {
                    let wall_restriction_min =
                        self.simplifier.shape(&self.get_collision(0, layer_idx, true).intersection(&self.get_collision(radius, layer_idx_below, true)));
                    data_min.push(((radius, layer_idx), wall_restriction_min));
                }
            }
            insert_all(&self.wall_restrictions_cache, data);
            insert_all(&self.wall_restrictions_cache_min, data_min);
        }
    }

    /// `ceilRadius(radius)` (private overload).
    pub(crate) fn ceil_radius(&self, radius: Coord) -> Coord {
        if radius == 0 {
            return 0;
        }
        if radius <= self.radius_0 {
            return self.radius_0;
        }
        let mut exponential_result = (SUPPORT_TREE_EXPONENTIAL_THRESHOLD as f64 * SUPPORT_TREE_EXPONENTIAL_FACTOR) as Coord;
        let stepsize = (exponential_result - self.radius_0) / (SUPPORT_TREE_PRE_EXPONENTIAL_STEPS as Coord + 1);
        let mut result = self.radius_0;
        for _ in 0..SUPPORT_TREE_PRE_EXPONENTIAL_STEPS {
            result += stepsize;
            if result >= radius && !self.ignorable_radii.contains(&result) {
                return result;
            }
        }
        while exponential_result < radius || self.ignorable_radii.contains(&exponential_result) {
            exponential_result = cmax(
                (exponential_result as f64 * SUPPORT_TREE_EXPONENTIAL_FACTOR) as Coord,
                exponential_result + SUPPORT_TREE_COLLISION_RESOLUTION,
            );
        }
        exponential_result
    }

    /// `calculateMachineBorderCollision`.
    fn calculate_machine_border_collision(machine_border: &Shape) -> Shape {
        let machine_volume_border = machine_border.offset_default(mm2int(1000.0));
        machine_volume_border.difference(machine_border)
    }
}
