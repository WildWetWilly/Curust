//! `TreeSupport.h/.cpp`: tree support generation.
//!
//! Elements (`TreeSupportElement*` in C++) live in an [`ElemArena`] and are referred to by their
//! allocation index. See [`element`] for how the pointer-keyed containers are made deterministic.

pub mod element;
pub mod model_volumes;
pub mod settings;
pub mod tip_generator;
pub mod utils;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::atomic::Ordering;

use crate::geometry::lines_set::Shape;
use crate::geometry::point2ll::{pt, v_size, v_size2, Point2LL};
use crate::geometry::polyline::{Polygon, Polyline};
use crate::settings::enums::ESupportStructure;
use crate::settings::types::LayerIndex;
use crate::slice_data_storage::SliceDataStorage;
use crate::support::AreaSupport;
use crate::utils::aabb::AABB;
use crate::utils::coord::{Coord, EPSILON};
use crate::utils::math::round_up_divide;
use crate::utils::polygon_utils::PolygonUtils;
use clipper::JoinType;

use element::{AreaIncreaseSettings, ElemArena, ElemId, ElemKey, TreeSupportElement, UNSET};
use model_volumes::{TreeModelVolumes, FUDGE_LENGTH};
use settings::{base_circle, cmax, cmin, AvoidanceType, InterfacePreference, RestPreference, TreeSupportSettings, BASE_RADIUS};
use tip_generator::{TreeSupportTipGenerator, SUPPORT_TREE_ONLY_GRACIOUS_TO_MODEL};
use utils as tsu;

pub use settings::{HAS_TO_RELY_ON_MIN_XY_DIST_ONLY, SOME_MODEL_CONTAINS_THICK_ROOF};

/// `FakeRoofArea`.
#[derive(Clone)]
pub struct FakeRoofArea {
    pub area: Shape,
    pub line_distance: Coord,
    pub fractional: bool,
}

/// `PropertyAreas` (`std::map<TreeSupportElement, Shape>`): ordered by `TreeSupportElement::operator<`;
/// equivalent keys keep the first inserted key object.
type PropertyAreas = BTreeMap<ElemKey, (TreeSupportElement, Shape)>;
/// `PropertyAreasUnordered` (`std::unordered_map<TreeSupportElement, Shape>`). Its iteration order
/// never influences the result: it is only used for look-ups, or copied into maps that are ordered
/// or only looked up.
type PropertyAreasUnordered = HashMap<ElemKey, (TreeSupportElement, Shape)>;
type AabbMap = BTreeMap<ElemKey, (TreeSupportElement, AABB)>;

/// `std::thread::hardware_concurrency()`, overridable with `CURA_HW_CONCURRENCY` (the reference build
/// has the same hook). It decides how the influence areas are bucketed when merging and so changes
/// the result.
fn hardware_concurrency() -> usize {
    if let Ok(v) = std::env::var("CURA_HW_CONCURRENCY") {
        return v.trim().parse::<i32>().unwrap_or(0).max(0) as usize;
    }
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0)
}

fn lookup(map_u: Option<&PropertyAreasUnordered>, map_o: Option<&PropertyAreas>, key: &ElemKey) -> Option<Shape> {
    if let Some(m) = map_u {
        return m.get(key).map(|v| v.1.clone());
    }
    map_o.and_then(|m| m.get(key).map(|v| v.1.clone()))
}

/// Debug dumps matching the instrumented reference (`CURA_DUMP`).
fn dbg_dump_move_bounds(tag: &str, move_bounds: &[BTreeSet<ElemId>], arena: &ElemArena) {
    if !crate::debug_dump::enabled() {
        return;
    }
    let mut ss = String::new();
    for (layer, set) in move_bounds.iter().enumerate() {
        if set.is_empty() {
            continue;
        }
        ss.push_str(&format!("tree_{} mesh=0 layer={} n={}\n", tag, layer, set.len()));
        for &id in set {
            let e = &arena.elems[id];
            let l = &e.last_area_increase;
            ss.push_str(&format!(
                "  elem th={} tp={},{} np={},{} nh={} erh={} dtt={} bp={} grac={} res={},{} bri={} umin={} sroof={} dmu={} safe={} miss={} skipov={} par={} itmr={} lim={},{} last={},{},{},{},{},{} tips={} ovt={}\n",
                e.target_height,
                e.target_position.x,
                e.target_position.y,
                e.next_position.x,
                e.next_position.y,
                e.next_height,
                e.effective_radius_height,
                e.distance_to_top,
                e.to_buildplate as i32,
                e.to_model_gracious as i32,
                e.result_on_layer.x,
                e.result_on_layer.y,
                crate::utils::string::fmt_g_prec(e.buildplate_radius_increases, 17),
                e.use_min_xy_dist as i32,
                e.supports_roof as i32,
                e.dont_move_until,
                e.can_use_safe_radius as i32,
                e.missing_roof_layers,
                e.skip_ovalisation as i32,
                e.parents.len(),
                e.increased_to_model_radius,
                e.influence_area_limit_active as i32,
                e.influence_area_limit_range,
                l.type_ as i32,
                l.increase_speed,
                l.increase_radius as i32,
                l.no_error as i32,
                l.use_min_distance as i32,
                l.move_ as i32,
                e.all_tips.len(),
                e.additional_ovalization_targets.len()
            ));
            ss.push_str(&crate::debug_dump::shape_to_string(&arena.areas[id]));
        }
    }
    crate::debug_dump::write(&ss);
}

fn dbg_dump_layers(tag: &str, layers: &[Shape]) {
    if !crate::debug_dump::enabled() {
        return;
    }
    let mut ss = String::new();
    for (layer, shape) in layers.iter().enumerate() {
        if shape.is_empty() {
            continue;
        }
        ss.push_str(&format!("tree_{} mesh=0 layer={}\n{}", tag, layer, crate::debug_dump::shape_to_string(shape)));
    }
    crate::debug_dump::write(&ss);
}

pub struct TreeSupport {
    grouped_meshes: Vec<(TreeSupportSettings, Vec<usize>)>,
    additional_required_support_area: Vec<Shape>,
    fake_roof_areas: Vec<Vec<FakeRoofArea>>,
    volumes: Option<TreeModelVolumes>,
    config: Option<TreeSupportSettings>,
    arena: ElemArena,
}

impl TreeSupport {
    pub fn new(storage: &SliceDataStorage) -> Self {
        for mesh in &storage.meshes {
            let s = mesh.settings();
            if s.get_coord("support_roof_height") >= 2 * s.get_coord("layer_height") {
                SOME_MODEL_CONTAINS_THICK_ROOF.store(true, Ordering::Relaxed);
            }
            if s.get_coord("support_top_distance") == 0 || s.get_coord("support_bottom_distance") == 0 || s.get_coord("min_feature_size") < FUDGE_LENGTH * 2
            {
                HAS_TO_RELY_ON_MIN_XY_DIST_ONLY.store(true, Ordering::Relaxed);
            }
        }

        let mut grouped_meshes: Vec<(TreeSupportSettings, Vec<usize>)> = Vec::new();
        let mut largest_printed_mesh_idx = 0usize;
        for (mesh_idx, mesh) in storage.meshes.iter().enumerate() {
            let s = mesh.settings();
            let non_supportable_mesh = s.get_bool("infill_mesh") || s.get_bool("anti_overhang_mesh") || s.get_bool("support_mesh");
            if s.get_enum::<ESupportStructure>("support_structure") != ESupportStructure::TREE || !s.get_bool("support_enable") || non_supportable_mesh {
                continue;
            }
            let mut added = false;
            let next_settings = TreeSupportSettings::new(s);
            for grouped_mesh in grouped_meshes.iter_mut() {
                if next_settings.equals(&grouped_mesh.0) {
                    added = true;
                    grouped_mesh.1.push(mesh_idx);
                }
            }
            if !added {
                grouped_meshes.push((next_settings, vec![mesh_idx]));
            }
            let largest_z = storage.meshes[largest_printed_mesh_idx].layers.last().map(|l| l.print_z).unwrap_or(0);
            if largest_z < mesh.layers.last().map(|l| l.print_z).unwrap_or(0) {
                largest_printed_mesh_idx = mesh_idx;
            }
        }
        if !storage.meshes.is_empty() {
            let known_z: Vec<Coord> = storage.meshes[largest_printed_mesh_idx].layers.iter().map(|l| l.print_z).collect();
            for mesh in grouped_meshes.iter_mut() {
                mesh.0.set_actual_z(&known_z);
            }
        }

        TreeSupport {
            grouped_meshes,
            additional_required_support_area: Vec::new(),
            fake_roof_areas: (0..storage.support.support_layers.len()).map(|_| Vec::new()).collect(),
            volumes: None,
            config: None,
            arena: ElemArena::default(),
        }
    }

    fn volumes(&self) -> &TreeModelVolumes {
        self.volumes.as_ref().unwrap()
    }

    fn config(&self) -> &TreeSupportSettings {
        self.config.as_ref().unwrap()
    }

    /// `generateSupportAreas`.
    pub fn generate_support_areas(&mut self, storage: &mut SliceDataStorage) {
        if self.grouped_meshes.is_empty() {
            return;
        }
        if storage.support.cross_fill_provider.is_none() {
            AreaSupport::precompute_cross_infill_tree(storage);
        }

        let groups: Vec<(TreeSupportSettings, Vec<usize>)> = self.grouped_meshes.clone();
        for (counter, processing) in groups.iter().enumerate() {
            let n_layers = storage.support.support_layers.len();
            let mut move_bounds: Vec<BTreeSet<ElemId>> = vec![BTreeSet::new(); n_layers];
            self.additional_required_support_area = vec![Shape::default(); n_layers];
            crate::log_info!(
                "Processing support tree mesh group {} of {} containing {} meshes.",
                counter + 1,
                groups.len(),
                processing.1.len()
            );
            let mut exclude: Vec<Shape> = Vec::with_capacity(n_layers);
            for layer_idx in 0..n_layers {
                let sl = &storage.support.support_layers[layer_idx];
                let mut exlude_at_layer = Shape::default();
                exlude_at_layer.push_all(&sl.support_bottom);
                exlude_at_layer.push_all(&sl.support_roof);
                for part in &sl.support_infill_parts {
                    exlude_at_layer.push_all(&part.outline);
                }
                exclude.push(exlude_at_layer.union_polygons());
            }
            self.config = Some(processing.0.clone());
            let c = self.config().clone();
            self.volumes = Some(TreeModelVolumes::new(
                storage,
                c.maximum_move_distance,
                c.maximum_move_distance_slow,
                c.support_line_width / 2,
                processing.1[0],
                &exclude,
            ));
            self.arena = ElemArena::default();

            let max_required_layer = self.precalculate(storage, &processing.1);
            if max_required_layer < 0 {
                crate::log_info!(
                    "Support tree mesh group {} does not have any overhang. Skipping tree support generation for this support tree mesh group.",
                    counter + 1
                );
                continue;
            }

            for &mesh_idx in &processing.1 {
                self.generate_initial_areas(mesh_idx, &mut move_bounds, storage);
            }
            dbg_dump_move_bounds("tips", &move_bounds, &self.arena);
            self.create_layer_pathing(&mut move_bounds);
            dbg_dump_move_bounds("paths", &move_bounds, &self.arena);
            self.create_nodes_from_area(&mut move_bounds);
            dbg_dump_move_bounds("nodes", &move_bounds, &self.arena);
            self.draw_areas(&mut move_bounds, storage);
        }
        self.arena = ElemArena::default();
        self.volumes = None;
        storage.support.generated = true;
    }

    /// `precalculate`.
    fn precalculate(&mut self, storage: &SliceDataStorage, currently_processing_meshes: &[usize]) -> LayerIndex {
        let mut max_layer: LayerIndex = -1;
        for &mesh_idx in currently_processing_meshes {
            let mesh = &storage.meshes[mesh_idx];
            let s = mesh.settings();
            let layer_height = s.get_coord("layer_height");
            let z_distance_top = s.get_coord("support_top_distance");
            let z_distance_top_layers = round_up_divide(z_distance_top as u64, layer_height as u64) as usize + 1;
            if mesh.overhang_areas.len() <= z_distance_top_layers {
                continue;
            }
            for layer_idx in (1..mesh.overhang_areas.len() - z_distance_top_layers).rev() {
                if !mesh.overhang_areas[layer_idx + z_distance_top_layers].is_empty() {
                    if layer_idx as LayerIndex > max_layer {
                        max_layer = 1 + layer_idx as LayerIndex;
                    }
                    break;
                }
            }
        }
        if max_layer >= 0 {
            self.volumes.as_mut().unwrap().precalculate(max_layer);
        }
        max_layer
    }

    /// `generateInitialAreas`.
    fn generate_initial_areas(&mut self, mesh_idx: usize, move_bounds: &mut [BTreeSet<ElemId>], storage: &mut SliceDataStorage) {
        let volumes = self.volumes.as_ref().unwrap();
        let mesh = &storage.meshes[mesh_idx];
        let mut tip_gen = TreeSupportTipGenerator::new(mesh, volumes);
        tip_gen.generate_tips(
            &mut self.arena,
            &mut storage.support,
            mesh,
            move_bounds,
            &mut self.additional_required_support_area,
            &mut self.fake_roof_areas,
        );
    }

    /// `mergeHelper`.
    #[allow(clippy::too_many_arguments)]
    fn merge_helper(
        &mut self,
        reduced_aabb: &mut AabbMap,
        input_aabb: &AabbMap,
        to_bp_areas: &PropertyAreasUnordered,
        to_model_areas: &PropertyAreas,
        influence_areas: &PropertyAreas,
        insert_bp_areas: &mut PropertyAreasUnordered,
        insert_model_areas: &mut PropertyAreasUnordered,
        insert_influence: &mut PropertyAreasUnordered,
        erase: &mut Vec<TreeSupportElement>,
        layer_idx: LayerIndex,
    ) {
        let config = self.config.clone().unwrap();
        let first_merge_iteration = reduced_aabb.is_empty();
        let inputs: Vec<(ElemKey, TreeSupportElement, AABB)> = input_aabb.iter().map(|(k, v)| (*k, v.0.clone(), v.1)).collect();
        for (influence_key, influence_elem, influence_aabb) in inputs {
            let mut merged = false;
            let mut merge_result: Option<(ElemKey, TreeSupportElement, AABB)> = None;
            for (reduced_key, (reduced_elem, aabb)) in reduced_aabb.iter() {
                if !aabb.hit(&influence_aabb) {
                    continue;
                }
                if !first_merge_iteration && input_aabb.contains_key(reduced_key) {
                    break;
                }
                let merging_gracious_and_non_gracious = reduced_elem.to_model_gracious != influence_elem.to_model_gracious;
                let merging_to_bp = reduced_elem.to_buildplate && influence_elem.to_buildplate;
                let merging_min_and_regular_xy = reduced_elem.use_min_xy_dist != influence_elem.use_min_xy_dist;
                let merging_different_range_limits = reduced_elem.influence_area_limit_active
                    && influence_elem.influence_area_limit_active
                    && influence_elem.influence_area_limit_range != reduced_elem.influence_area_limit_range;
                let mut increased_to_model_radius: Coord = 0;
                let mut larger_to_model_dtt: usize = 0;

                if !merging_to_bp {
                    let infl_radius = config.get_radius(&influence_elem);
                    let redu_radius = config.get_radius(reduced_elem);
                    if reduced_elem.to_buildplate != influence_elem.to_buildplate {
                        if reduced_elem.to_buildplate {
                            if infl_radius < redu_radius {
                                increased_to_model_radius = influence_elem.increased_to_model_radius + redu_radius - infl_radius;
                            }
                        } else if infl_radius > redu_radius {
                            increased_to_model_radius = reduced_elem.increased_to_model_radius + infl_radius - redu_radius;
                        }
                    }
                    larger_to_model_dtt = cmax(influence_elem.distance_to_top, reduced_elem.distance_to_top);
                }

                if merging_min_and_regular_xy
                    || merging_gracious_and_non_gracious
                    || increased_to_model_radius > config.max_to_model_radius_increase
                    || (!merging_to_bp && larger_to_model_dtt < config.min_dtt_to_model && !reduced_elem.supports_roof && !influence_elem.supports_roof)
                    || merging_different_range_limits
                {
                    continue;
                }

                let (relevant_infl, relevant_redu) = if merging_to_bp {
                    (
                        to_bp_areas.get(&influence_key).map(|v| v.1.clone()).unwrap_or_default(),
                        lookup(Some(insert_bp_areas), None, reduced_key).or_else(|| lookup(Some(to_bp_areas), None, reduced_key)).unwrap_or_default(),
                    )
                } else {
                    (
                        to_model_areas.get(&influence_key).map(|v| v.1.clone()).unwrap_or_default(),
                        lookup(Some(insert_model_areas), None, reduced_key).or_else(|| lookup(None, Some(to_model_areas), reduced_key)).unwrap_or_default(),
                    )
                };

                let red_bigger = config.get_collision_radius(reduced_elem) > config.get_collision_radius(&influence_elem);
                let (smaller_rad, bigger_rad) = if red_bigger {
                    ((&influence_elem, relevant_infl), (reduced_elem, relevant_redu))
                } else {
                    ((reduced_elem, relevant_redu), (&influence_elem, relevant_infl))
                };
                let real_radius_delta = (config.get_radius(bigger_rad.0) - config.get_radius(smaller_rad.0)).abs();
                let smaller_collision_radius = config.get_collision_radius(smaller_rad.0);

                if !bigger_rad.0.can_use_safe_radius && smaller_rad.0.can_use_safe_radius {
                    continue;
                }
                let use_min_radius = bigger_rad.0.use_min_xy_dist && smaller_rad.0.use_min_xy_dist;

                let volumes = self.volumes.as_ref().unwrap();
                let small_collision = volumes.get_collision(smaller_collision_radius, layer_idx - 1, use_min_radius);
                let small_rad_increased_by_big_minus_small = tsu::safe_offset_inc(
                    &smaller_rad.1,
                    real_radius_delta,
                    &small_collision,
                    2 * (config.xy_distance + smaller_collision_radius - EPSILON),
                    0,
                    0,
                    config.support_line_distance / 2,
                    Some(&config.simplifier),
                );
                let intersect = small_rad_increased_by_big_minus_small.intersection(&bigger_rad.1);

                if intersect.area() > 1.0 {
                    if intersect.offset_default(-FUDGE_LENGTH / 2).area() <= 1.0 {
                        continue;
                    }
                    let mut new_pos = reduced_elem.next_position;
                    if !intersect.inside(new_pos, true) {
                        PolygonUtils::move_inside(&intersect, &mut new_pos, 0, i64::MAX);
                    }
                    if increased_to_model_radius == 0 {
                        increased_to_model_radius = cmax(reduced_elem.increased_to_model_radius, influence_elem.increased_to_model_radius);
                    }
                    let smaller_key = smaller_rad.0.key();
                    let bigger_key = bigger_rad.0.key();
                    let get_radius_fn = |dtt: usize, bp: f64| config.get_radius_dtt(dtt, bp);
                    let key = TreeSupportElement::merged(
                        reduced_elem,
                        &influence_elem,
                        layer_idx - 1,
                        new_pos,
                        increased_to_model_radius,
                        &get_radius_fn,
                        config.diameter_scale_bp_radius,
                        config.branch_radius,
                        config.diameter_angle_scale_factor,
                        &mut self.arena,
                    );
                    let volumes = self.volumes.as_ref().unwrap();

                    let get_intersect_influence = |insert_infl: &PropertyAreasUnordered, infl_areas: &PropertyAreas| -> Shape {
                        let infl_small = lookup(Some(insert_infl), None, &smaller_key).or_else(|| lookup(None, Some(infl_areas), &smaller_key)).unwrap_or_default();
                        let infl_big = lookup(Some(insert_infl), None, &bigger_key).or_else(|| lookup(None, Some(infl_areas), &bigger_key)).unwrap_or_default();
                        let small_rad_increased_by_big_minus_small_infl = tsu::safe_offset_inc(
                            &infl_small,
                            real_radius_delta,
                            &volumes.get_collision(smaller_collision_radius, layer_idx - 1, use_min_radius),
                            2 * (config.xy_distance + smaller_collision_radius - EPSILON),
                            0,
                            0,
                            config.support_line_distance / 2,
                            Some(&config.simplifier),
                        );
                        small_rad_increased_by_big_minus_small_infl.intersection(&infl_big)
                    };

                    let mut intersect_influence = tsu::safe_union(&intersect, &get_intersect_influence(insert_influence, influence_areas));
                    let mut intersect_to_model = Shape::default();
                    if merging_to_bp && config.support_rests_on_model {
                        intersect_to_model = get_intersect_influence(insert_model_areas, to_model_areas);
                        intersect_influence = tsu::safe_union(&intersect_influence, &intersect_to_model);
                    }

                    insert_bp_areas.remove(reduced_key);
                    insert_bp_areas.remove(&influence_key);
                    insert_model_areas.remove(reduced_key);
                    insert_model_areas.remove(&influence_key);
                    insert_influence.remove(reduced_key);
                    insert_influence.remove(&influence_key);

                    let k = key.key();
                    (if merging_to_bp { &mut *insert_bp_areas } else { &mut *insert_model_areas })
                        .entry(k)
                        .or_insert_with(|| (key.clone(), intersect.clone()));
                    if merging_to_bp && config.support_rests_on_model {
                        insert_model_areas.entry(k).or_insert_with(|| (key.clone(), intersect_to_model.clone()));
                    }
                    insert_influence.entry(k).or_insert_with(|| (key.clone(), intersect_influence.clone()));

                    erase.push(reduced_elem.clone());
                    erase.push(influence_elem.clone());
                    let merge = intersect
                        .union_polygons_other(&intersect_to_model)
                        .offset_join(config.get_radius(&key), JoinType::Round)
                        .difference(&volumes.get_collision(0, layer_idx - 1, false));
                    merge_result = Some((*reduced_key, key, AABB::from_shape(&merge)));
                    merged = true;
                    break;
                }
            }
            if let Some((reduced_key, key, aabb)) = merge_result {
                reduced_aabb.remove(&reduced_key);
                reduced_aabb.entry(key.key()).or_insert((key, aabb));
            }
            if !merged {
                reduced_aabb.entry(influence_key).and_modify(|v| v.1 = influence_aabb).or_insert((influence_elem, influence_aabb));
            }
        }
    }

    /// `mergeInfluenceAreas`.
    fn merge_influence_areas(
        &mut self,
        to_bp_areas: &mut PropertyAreasUnordered,
        to_model_areas: &mut PropertyAreas,
        influence_areas: &mut PropertyAreas,
        layer_idx: LayerIndex,
    ) {
        let input_size = influence_areas.len();
        let num_threads = cmax(1usize, hardware_concurrency());
        if input_size == 0 {
            return;
        }
        const MIN_ELEMENTS_PER_BUCKET: u64 = 2;
        let max_bucket_count = 2f64.powf((round_up_divide(input_size as u64, MIN_ELEMENTS_PER_BUCKET) as f64).ln().floor()) as usize;
        let bucket_count = cmin(max_bucket_count, num_threads) as i32;

        let mut buckets_area: Vec<PropertyAreas> = (0..2 * bucket_count).map(|_| PropertyAreas::new()).collect();
        let mut buckets_aabb: Vec<AabbMap> = (0..2 * bucket_count).map(|_| AabbMap::new()).collect();

        let mut position = 0usize;
        let mut counter = 0usize;
        let over_elements = input_size % bucket_count as usize;
        let elements_per_step = input_size / bucket_count as usize;
        for (k, v) in influence_areas.iter() {
            buckets_area[position * 2 + 1].entry(*k).or_insert_with(|| v.clone());
            counter += 1;
            if (counter == elements_per_step && position >= over_elements) || counter > elements_per_step {
                position += 1;
                counter = 0;
            }
        }

        let config = self.config().clone();
        for idx in 0..buckets_area.len() / 2 {
            let idx = idx * 2 + 1;
            for (k, (elem, shape)) in buckets_area[idx].iter() {
                let mut outer_support_wall_aabb = AABB::from_shape(shape);
                outer_support_wall_aabb.expand(config.get_radius(elem) as i32);
                buckets_aabb[idx].entry(*k).or_insert_with(|| (elem.clone(), outer_support_wall_aabb));
            }
        }

        while buckets_area.len() > 1 {
            let half = buckets_area.len() / 2;
            let mut insert_main: Vec<PropertyAreasUnordered> = (0..half).map(|_| HashMap::new()).collect();
            let mut insert_secondary: Vec<PropertyAreasUnordered> = (0..half).map(|_| HashMap::new()).collect();
            let mut insert_influence: Vec<PropertyAreasUnordered> = (0..half).map(|_| HashMap::new()).collect();
            let mut erase: Vec<Vec<TreeSupportElement>> = (0..half).map(|_| Vec::new()).collect();

            for bucket_pair in 0..half {
                let bucket_pair_idx = bucket_pair * 2;
                let (left, right) = buckets_aabb.split_at_mut(bucket_pair_idx + 1);
                let reduced = &mut left[bucket_pair_idx];
                let input = &right[0];
                self.merge_helper(
                    reduced,
                    input,
                    to_bp_areas,
                    to_model_areas,
                    influence_areas,
                    &mut insert_main[bucket_pair],
                    &mut insert_secondary[bucket_pair],
                    &mut insert_influence[bucket_pair],
                    &mut erase[bucket_pair],
                    layer_idx,
                );
                buckets_area[bucket_pair_idx + 1].clear();
                buckets_aabb[bucket_pair_idx + 1].clear();
            }

            for i in 0..half {
                for del in &erase[i] {
                    let k = del.key();
                    to_bp_areas.remove(&k);
                    to_model_areas.remove(&k);
                    influence_areas.remove(&k);
                }
                for (k, v) in insert_main[i].drain() {
                    to_bp_areas.entry(k).or_insert(v);
                }
                for (k, v) in insert_secondary[i].drain() {
                    to_model_areas.entry(k).or_insert(v);
                }
                for (k, v) in insert_influence[i].drain() {
                    influence_areas.entry(k).or_insert(v);
                }
            }

            buckets_area.retain(|x| !x.is_empty());
            buckets_aabb.retain(|x| !x.is_empty());
        }
    }

    /// `increaseSingleArea`.
    #[allow(clippy::too_many_arguments)]
    fn increase_single_area(
        &self,
        settings: AreaIncreaseSettings,
        layer_idx: LayerIndex,
        parent: ElemId,
        relevant_offset: &Shape,
        to_bp_data: &mut Shape,
        to_model_data: &mut Shape,
        increased: &mut Shape,
        overspeed: Coord,
        mergelayer: bool,
    ) -> Option<TreeSupportElement> {
        let config = self.config();
        let volumes = self.volumes();
        let parent_elem = &self.arena.elems[parent];
        let mut current_elem = TreeSupportElement::from_above(parent_elem, parent);
        let mut check_layer_data: Shape;
        if settings.increase_radius {
            current_elem.effective_radius_height += 1;
        }
        let mut radius = config.get_collision_radius(&current_elem);

        if settings.move_ {
            *increased = relevant_offset.clone();
            if overspeed > 0 {
                let safe_movement_distance = (if current_elem.use_min_xy_dist { config.xy_min_distance } else { config.xy_distance })
                    + if cmin(config.z_distance_top_layers, config.z_distance_bottom_layers) > 0 { config.min_feature_size } else { 0 };
                *increased = tsu::safe_offset_inc(
                    increased,
                    overspeed,
                    &volumes.get_wall_restriction(config.get_collision_radius(parent_elem), layer_idx, parent_elem.use_min_xy_dist),
                    safe_movement_distance,
                    safe_movement_distance + radius,
                    1,
                    config.support_line_distance / 2,
                    None,
                );
            }
            if settings.no_error && settings.move_ {
                *increased = config.simplifier.shape(increased);
            }
        } else {
            *increased = self.arena.areas[parent].clone();
        }

        if (mergelayer || current_elem.to_buildplate) && config.support_rest_preference == RestPreference::Buildplate {
            *to_bp_data = tsu::safe_union1(&increased.difference(&volumes.get_avoidance(radius, layer_idx - 1, settings.type_, false, settings.use_min_distance)));
            if !current_elem.to_buildplate && to_bp_data.area() > 1.0 {
                current_elem.to_buildplate = true;
                crate::log_debug!(
                    "Corrected taint leading to a wrong to model value on layer {} targeting {} with radius {}",
                    layer_idx - 1,
                    current_elem.target_height,
                    radius
                );
            }
        }
        if config.support_rests_on_model {
            if mergelayer || current_elem.to_model_gracious {
                *to_model_data =
                    tsu::safe_union1(&increased.difference(&volumes.get_avoidance(radius, layer_idx - 1, settings.type_, true, settings.use_min_distance)));
            }
            if !current_elem.to_model_gracious {
                if mergelayer && to_model_data.area() >= 1.0 {
                    current_elem.to_model_gracious = true;
                    crate::log_debug!(
                        "Corrected taint leading to a wrong non gracious value on layer {} targeting {} with radius {}",
                        layer_idx - 1,
                        current_elem.target_height,
                        radius
                    );
                } else {
                    *to_model_data = tsu::safe_union1(
                        &increased.difference(&volumes.get_avoidance(radius, layer_idx - 1, AvoidanceType::Collision, true, settings.use_min_distance)),
                    );
                }
            }
        }

        check_layer_data = if current_elem.to_buildplate { to_bp_data.clone() } else { to_model_data.clone() };

        if settings.increase_radius && check_layer_data.area() > 1.0 {
            // `validWithRadius` captures radius and current_elem by reference.
            let valid_with_radius = |next_radius: Coord, radius: Coord, current_elem: &TreeSupportElement, increased: &Shape| -> bool {
                if volumes.ceil_radius_md(next_radius, settings.use_min_distance) <= volumes.ceil_radius_md(radius, settings.use_min_distance) {
                    return true;
                }
                let mut to_bp_data_2 = Shape::default();
                if current_elem.to_buildplate {
                    to_bp_data_2 = increased
                        .difference(&volumes.get_avoidance(next_radius, layer_idx - 1, settings.type_, false, settings.use_min_distance))
                        .union_polygons();
                }
                let mut to_model_data_2 = Shape::default();
                if config.support_rests_on_model && !current_elem.to_buildplate {
                    to_model_data_2 = increased
                        .difference(&volumes.get_avoidance(
                            next_radius,
                            layer_idx - 1,
                            if current_elem.to_model_gracious { settings.type_ } else { AvoidanceType::Collision },
                            true,
                            settings.use_min_distance,
                        ))
                        .union_polygons();
                }
                let check_layer_data_2 = if current_elem.to_buildplate { to_bp_data_2 } else { to_model_data_2 };
                check_layer_data_2.area() > 1.0
            };
            let ceil_radius_before = volumes.ceil_radius_md(radius, settings.use_min_distance);

            if config.get_collision_radius(&current_elem) < config.increase_radius_until_radius
                && config.get_collision_radius(&current_elem) < config.get_radius(&current_elem)
            {
                let target_radius = cmin(config.get_radius(&current_elem), config.increase_radius_until_radius);
                let mut current_ceil_radius = volumes.get_radius_next_ceil(radius, settings.use_min_distance);
                while current_ceil_radius < target_radius
                    && valid_with_radius(volumes.get_radius_next_ceil(current_ceil_radius + 1, settings.use_min_distance), radius, &current_elem, increased)
                {
                    current_ceil_radius = volumes.get_radius_next_ceil(current_ceil_radius + 1, settings.use_min_distance);
                }
                let mut resulting_eff_dtt = current_elem.effective_radius_height;
                while resulting_eff_dtt + 1 < current_elem.distance_to_top
                    && config.get_radius_dtt(resulting_eff_dtt + 1, current_elem.buildplate_radius_increases) <= current_ceil_radius
                    && config.get_radius_dtt(resulting_eff_dtt + 1, current_elem.buildplate_radius_increases) <= config.get_radius(&current_elem)
                {
                    resulting_eff_dtt += 1;
                }
                current_elem.effective_radius_height = resulting_eff_dtt;

                if config.get_collision_radius(&current_elem) < config.increase_radius_until_radius
                    && config.get_collision_radius(&current_elem) < config.get_radius(&current_elem)
                {
                    if current_elem.to_buildplate {
                        let new_to_bp_data =
                            to_bp_data.difference(&volumes.get_collision(config.get_radius(&current_elem), layer_idx - 1, current_elem.use_min_xy_dist));
                        if new_to_bp_data.area() > EPSILON as f64 {
                            *to_bp_data = new_to_bp_data;
                        }
                    }
                    if config.support_rests_on_model && (!current_elem.to_buildplate || mergelayer) {
                        let new_to_model_data =
                            to_model_data.difference(&volumes.get_collision(config.get_radius(&current_elem), layer_idx - 1, current_elem.use_min_xy_dist));
                        if new_to_model_data.area() > EPSILON as f64 {
                            *to_model_data = new_to_model_data;
                        }
                    }
                }
            }
            radius = config.get_collision_radius(&current_elem);

            let foot_radius_increase = (config.branch_radius as f64 * cmax(config.diameter_scale_bp_radius - config.diameter_angle_scale_factor, 0.0)) as Coord;
            let planned_foot_increase = cmin(
                1.0,
                (config.recommended_min_radius(layer_idx - 1) - config.get_radius(&current_elem)) as f64 / foot_radius_increase as f64,
            );
            let increase_bp_foot = planned_foot_increase > 0.0
                && (current_elem.to_buildplate || (current_elem.to_model_gracious && config.support_rest_preference == RestPreference::Graceful));

            if increase_bp_foot
                && config.get_radius(&current_elem) >= config.branch_radius
                && config.get_radius(&current_elem) >= config.increase_radius_until_radius
                && valid_with_radius(
                    config.get_radius_dtt(current_elem.effective_radius_height, current_elem.buildplate_radius_increases + planned_foot_increase),
                    radius,
                    &current_elem,
                    increased,
                )
            {
                current_elem.buildplate_radius_increases += planned_foot_increase;
                radius = config.get_collision_radius(&current_elem);
            }

            if ceil_radius_before != volumes.ceil_radius_md(radius, settings.use_min_distance) {
                if current_elem.to_buildplate {
                    *to_bp_data =
                        tsu::safe_union1(&increased.difference(&volumes.get_avoidance(radius, layer_idx - 1, settings.type_, false, settings.use_min_distance)));
                }
                if config.support_rests_on_model && (!current_elem.to_buildplate || mergelayer) {
                    *to_model_data = tsu::safe_union1(&increased.difference(&volumes.get_avoidance(
                        radius,
                        layer_idx - 1,
                        if current_elem.to_model_gracious { settings.type_ } else { AvoidanceType::Collision },
                        true,
                        settings.use_min_distance,
                    )));
                }
                check_layer_data = if current_elem.to_buildplate { to_bp_data.clone() } else { to_model_data.clone() };
                if check_layer_data.area() < 1.0 {
                    crate::log_error!(
                        "Lost area by doing catch up from {} to radius {}",
                        ceil_radius_before,
                        volumes.ceil_radius_md(config.get_collision_radius(&current_elem), settings.use_min_distance)
                    );
                }
            }
        }

        if current_elem.influence_area_limit_active
            && !current_elem.use_min_xy_dist
            && check_layer_data.area() > 1.0
            && (current_elem.to_model_gracious || current_elem.distance_to_top <= config.min_dtt_to_model)
        {
            let max_radius_increase = cmax(
                (((config.branch_radius - config.min_radius) as u64) / config.tip_layers as u64) as Coord,
                ((config.branch_radius as f64 * config.diameter_angle_scale_factor)
                    + config.branch_radius as f64 * cmax(config.diameter_scale_bp_radius - config.diameter_angle_scale_factor, 0.0)) as Coord,
            );
            let mut limit_range_validated = false;
            *to_bp_data = tsu::safe_union1(to_bp_data);
            *to_model_data = tsu::safe_union1(to_model_data);
            while !limit_range_validated {
                if current_elem.to_buildplate {
                    let limited_to_bp = to_bp_data.intersection(&current_elem.influence_area_limit_area);
                    if limited_to_bp.area() > 1.0 {
                        *to_bp_data = limited_to_bp;
                        *to_model_data = to_model_data.intersection(&current_elem.influence_area_limit_area);
                        limit_range_validated = true;
                    }
                } else {
                    let limited_to_model_data = to_model_data.intersection(&current_elem.influence_area_limit_area);
                    if limited_to_model_data.area() > 1.0 {
                        *to_bp_data = to_bp_data.intersection(&current_elem.influence_area_limit_area);
                        *to_model_data = limited_to_model_data;
                        limit_range_validated = true;
                    }
                }
                if !limit_range_validated {
                    let reach_increase = cmax(current_elem.influence_area_limit_range / 4, config.maximum_move_distance + max_radius_increase);
                    current_elem.influence_area_limit_range += reach_increase;
                    current_elem.recreate_influence_limit_area();
                }
            }
        }

        if check_layer_data.area() > 1.0 {
            Some(current_elem)
        } else {
            None
        }
    }

    /// `increaseAreas`.
    #[allow(clippy::too_many_arguments)]
    fn increase_areas(
        &mut self,
        to_bp_areas: &mut PropertyAreasUnordered,
        to_model_areas: &mut PropertyAreas,
        influence_areas: &mut PropertyAreas,
        bypass_merge_areas: &mut Vec<ElemId>,
        last_layer: &[ElemId],
        layer_idx: LayerIndex,
        mergelayer: bool,
    ) {
        let config = self.config().clone();
        for &parent in last_layer {
            let parent_elem = self.arena.elems[parent].clone();
            let mut elem = TreeSupportElement::from_above(&parent_elem, parent);
            let wall_restriction =
                self.volumes().get_wall_restriction(config.get_collision_radius(&parent_elem), layer_idx, parent_elem.use_min_xy_dist);

            let mut to_bp_data = Shape::default();
            let mut to_model_data = Shape::default();
            let mut radius = config.get_collision_radius(&elem);

            let mut extra_speed: Coord = EPSILON;
            let mut extra_slow_speed: Coord = 0;
            let volumes = self.volumes();
            let ceiled_parent_radius = volumes.ceil_radius_md(config.get_collision_radius(&parent_elem), parent_elem.use_min_xy_dist);
            let projected_radius_increased = config.get_radius_dtt(parent_elem.effective_radius_height + 1, parent_elem.buildplate_radius_increases);
            let projected_radius_delta = projected_radius_increased - config.get_collision_radius(&parent_elem);

            let safe_movement_distance = (if elem.use_min_xy_dist { config.xy_min_distance } else { config.xy_distance })
                + if cmin(config.z_distance_top_layers, config.z_distance_bottom_layers) > 0 { config.min_feature_size } else { 0 };
            if ceiled_parent_radius == volumes.ceil_radius_md(projected_radius_increased, parent_elem.use_min_xy_dist)
                || projected_radius_increased < config.increase_radius_until_radius
            {
                extra_speed += projected_radius_delta;
            } else {
                extra_slow_speed += cmin(
                    projected_radius_delta,
                    (config.maximum_move_distance + extra_speed) - (config.maximum_move_distance_slow + extra_slow_speed),
                );
            }

            if config.layer_start_bp_radius > layer_idx
                && config.recommended_min_radius(layer_idx - 1) < config.get_radius_dtt(elem.effective_radius_height + 1, elem.buildplate_radius_increases)
            {
                if ceiled_parent_radius
                    == volumes.ceil_radius_md(
                        config.get_radius_dtt(parent_elem.effective_radius_height + 1, parent_elem.buildplate_radius_increases + 1.0),
                        parent_elem.use_min_xy_dist,
                    )
                {
                    extra_speed = (extra_speed as f64 + config.branch_radius as f64 * config.diameter_scale_bp_radius) as Coord;
                } else {
                    extra_slow_speed += cmin(
                        (config.branch_radius as f64 * config.diameter_scale_bp_radius) as Coord,
                        config.maximum_move_distance - (config.maximum_move_distance_slow + extra_slow_speed),
                    );
                }
            }

            let fast_speed = config.maximum_move_distance + extra_speed;
            let slow_speed = config.maximum_move_distance_slow + extra_speed + extra_slow_speed;

            let mut offset_slow = Shape::default();
            let mut offset_fast = Shape::default();
            let mut add = false;
            let mut bypass_merge = false;

            const INCREASE_RADIUS: bool = true;
            const NO_ERROR: bool = true;
            const USE_MIN_RADIUS: bool = true;
            const MOVE: bool = true;

            let mut order: VecDeque<AreaIncreaseSettings> = VecDeque::new();
            let insert_setting = |order: &mut VecDeque<AreaIncreaseSettings>, settings: AreaIncreaseSettings, back: bool| {
                if !order.iter().any(|s| *s == settings) {
                    if back {
                        order.push_back(settings);
                    } else {
                        order.push_front(settings);
                    }
                }
            };

            let parent_moved_slow = elem.last_area_increase.increase_speed < config.maximum_move_distance;
            let avoidance_speed_mismatch = parent_moved_slow && elem.last_area_increase.type_ != AvoidanceType::Slow;
            if elem.last_area_increase.move_
                && elem.last_area_increase.no_error
                && elem.can_use_safe_radius
                && !mergelayer
                && !avoidance_speed_mismatch
                && (elem.distance_to_top >= config.tip_layers || parent_moved_slow)
            {
                let slow_or_fast = if elem.last_area_increase.increase_speed < config.maximum_move_distance { slow_speed } else { fast_speed };
                let l = elem.last_area_increase;
                insert_setting(&mut order, AreaIncreaseSettings::new(l.type_, slow_or_fast, INCREASE_RADIUS, l.no_error, !USE_MIN_RADIUS, l.move_), true);
                insert_setting(&mut order, AreaIncreaseSettings::new(l.type_, slow_or_fast, !INCREASE_RADIUS, l.no_error, !USE_MIN_RADIUS, l.move_), true);
            }
            if !elem.can_use_safe_radius {
                insert_setting(&mut order, AreaIncreaseSettings::new(AvoidanceType::Slow, slow_speed, INCREASE_RADIUS, NO_ERROR, !USE_MIN_RADIUS, !MOVE), true);
                if (elem.distance_to_top as u64) < round_up_divide(config.tip_layers as u64, 2) {
                    insert_setting(&mut order, AreaIncreaseSettings::new(AvoidanceType::Fast, slow_speed, INCREASE_RADIUS, NO_ERROR, !USE_MIN_RADIUS, !MOVE), true);
                }
                insert_setting(&mut order, AreaIncreaseSettings::new(AvoidanceType::FastSafe, fast_speed, INCREASE_RADIUS, NO_ERROR, !USE_MIN_RADIUS, !MOVE), true);
                insert_setting(&mut order, AreaIncreaseSettings::new(AvoidanceType::FastSafe, fast_speed, !INCREASE_RADIUS, NO_ERROR, !USE_MIN_RADIUS, MOVE), true);
                insert_setting(&mut order, AreaIncreaseSettings::new(AvoidanceType::Fast, fast_speed, !INCREASE_RADIUS, NO_ERROR, !USE_MIN_RADIUS, MOVE), true);
            } else {
                insert_setting(&mut order, AreaIncreaseSettings::new(AvoidanceType::Slow, slow_speed, INCREASE_RADIUS, NO_ERROR, !USE_MIN_RADIUS, MOVE), true);
                insert_setting(&mut order, AreaIncreaseSettings::new(AvoidanceType::Slow, slow_speed, !INCREASE_RADIUS, NO_ERROR, !USE_MIN_RADIUS, MOVE), true);
                if elem.distance_to_top < config.tip_layers {
                    insert_setting(&mut order, AreaIncreaseSettings::new(AvoidanceType::FastSafe, slow_speed, INCREASE_RADIUS, NO_ERROR, !USE_MIN_RADIUS, MOVE), true);
                }
                insert_setting(&mut order, AreaIncreaseSettings::new(AvoidanceType::FastSafe, fast_speed, INCREASE_RADIUS, NO_ERROR, !USE_MIN_RADIUS, MOVE), true);
                insert_setting(&mut order, AreaIncreaseSettings::new(AvoidanceType::FastSafe, fast_speed, !INCREASE_RADIUS, NO_ERROR, !USE_MIN_RADIUS, MOVE), true);
            }

            if elem.use_min_xy_dist {
                let mut new_order: VecDeque<AreaIncreaseSettings> = VecDeque::new();
                for s in order.iter() {
                    new_order.push_back(*s);
                    new_order.push_back(AreaIncreaseSettings::new(s.type_, s.increase_speed, s.increase_radius, s.no_error, USE_MIN_RADIUS, s.move_));
                }
                order = new_order;
            }

            insert_setting(&mut order, AreaIncreaseSettings::new(AvoidanceType::Fast, fast_speed, !INCREASE_RADIUS, !NO_ERROR, elem.use_min_xy_dist, MOVE), true);

            let parent_area = &self.arena.areas[parent];
            if elem.to_buildplate
                || (elem.to_model_gracious && parent_area.intersection(&volumes.get_placeable_areas(radius, layer_idx)).is_empty())
                || (!elem.to_model_gracious && parent_area.intersection(&volumes.get_accumulated_placeable0(layer_idx)).is_empty())
            {
                insert_setting(
                    &mut order,
                    AreaIncreaseSettings::new(AvoidanceType::Fast, (fast_speed as f64 * 1.5) as Coord, !INCREASE_RADIUS, !NO_ERROR, elem.use_min_xy_dist, MOVE),
                    true,
                );
            }
            if elem.distance_to_top < elem.dont_move_until && elem.can_use_safe_radius {
                insert_setting(&mut order, AreaIncreaseSettings::new(AvoidanceType::Slow, 0, INCREASE_RADIUS, NO_ERROR, !USE_MIN_RADIUS, !MOVE), false);
            }

            let mut inc_wo_collision = Shape::default();
            let lhs = radius / safe_movement_distance
                - if (config.maximum_move_distance + extra_speed) < (radius + safe_movement_distance) { 1 } else { 0 };
            // coord_t compared against the uint64_t of round_up_divide: the coord_t converts to unsigned.
            let offset_independent_faster = (lhs as u64)
                > round_up_divide((extra_speed + extra_slow_speed + config.maximum_move_distance_slow) as u64, safe_movement_distance as u64);

            for settings in order.iter().copied() {
                if settings.move_ {
                    if offset_slow.is_empty() && (settings.increase_speed == slow_speed || !offset_independent_faster) {
                        offset_slow = tsu::safe_offset_inc(
                            parent_area,
                            extra_speed + extra_slow_speed + config.maximum_move_distance_slow,
                            &wall_restriction,
                            safe_movement_distance,
                            if offset_independent_faster { safe_movement_distance + radius } else { 0 },
                            2,
                            config.support_line_distance / 2,
                            Some(&config.simplifier),
                        )
                        .union_polygons();
                    }
                    if settings.increase_speed != slow_speed && offset_fast.is_empty() {
                        if offset_independent_faster {
                            offset_fast = tsu::safe_offset_inc(
                                parent_area,
                                extra_speed + config.maximum_move_distance,
                                &wall_restriction,
                                safe_movement_distance,
                                if offset_independent_faster { safe_movement_distance + radius } else { 0 },
                                1,
                                config.support_line_distance / 2,
                                Some(&config.simplifier),
                            )
                            .union_polygons();
                        } else {
                            let delta_slow_fast = config.maximum_move_distance - (config.maximum_move_distance_slow + extra_slow_speed);
                            offset_fast = tsu::safe_offset_inc(
                                &offset_slow,
                                delta_slow_fast,
                                &wall_restriction,
                                safe_movement_distance,
                                safe_movement_distance + radius,
                                if offset_independent_faster { 2 } else { 1 },
                                config.support_line_distance / 2,
                                Some(&config.simplifier),
                            )
                            .union_polygons();
                        }
                    }
                }
                let result;
                if !settings.no_error {
                    let lines_offset = tsu::to_polylines(parent_area).offset_default(EPSILON);
                    let base_error_area = parent_area.union_polygons_other(&lines_offset);
                    result = self.increase_single_area(
                        settings,
                        layer_idx,
                        parent,
                        &base_error_area,
                        &mut to_bp_data,
                        &mut to_model_data,
                        &mut inc_wo_collision,
                        settings.increase_speed,
                        mergelayer,
                    );
                    if fast_speed < settings.increase_speed {
                        crate::log_warn!(
                            "Influence area could not be increased! Data about the Influence area: Radius: {} at layer: {} NextTarget: {} Distance to top: {} Elephant foot increases {}  use_min_xy_dist {} to buildplate {} gracious {} safe {} until move {}",
                            radius,
                            layer_idx - 1,
                            elem.next_height,
                            elem.distance_to_top,
                            elem.buildplate_radius_increases,
                            elem.use_min_xy_dist,
                            elem.to_buildplate,
                            elem.to_model_gracious,
                            elem.can_use_safe_radius,
                            elem.dont_move_until
                        );
                    }
                } else {
                    result = self.increase_single_area(
                        settings,
                        layer_idx,
                        parent,
                        if settings.increase_speed == slow_speed { &offset_slow } else { &offset_fast },
                        &mut to_bp_data,
                        &mut to_model_data,
                        &mut inc_wo_collision,
                        cmax(settings.increase_speed - fast_speed, 0),
                        mergelayer,
                    );
                }

                if let Some(r) = result {
                    elem = r;
                    radius = config.get_collision_radius(&elem);
                    elem.last_area_increase = settings;
                    add = true;
                    bypass_merge = !settings.move_ || (settings.use_min_distance && elem.distance_to_top < config.tip_layers);
                    if settings.move_ {
                        elem.dont_move_until = 0;
                    } else {
                        elem.result_on_layer = parent_elem.result_on_layer;
                    }
                    elem.can_use_safe_radius = settings.type_ != AvoidanceType::Fast;
                    if !settings.use_min_distance {
                        elem.use_min_xy_dist = false;
                    }
                    if !settings.no_error && fast_speed < settings.increase_speed {
                        crate::log_warn!("Trying to keep area by moving faster than intended: Success.");
                    }
                    break;
                } else if !settings.no_error && fast_speed < settings.increase_speed {
                    crate::log_error!("Trying to keep area by moving faster than intended: FAILURE! Wrong branch likely!");
                }
            }

            if add {
                let max_influence_area = tsu::safe_union(
                    &inc_wo_collision.difference(&self.volumes().get_collision(radius, layer_idx - 1, elem.use_min_xy_dist)),
                    &tsu::safe_union(&to_bp_data, &to_model_data),
                );
                if bypass_merge {
                    let id = self.arena.alloc(TreeSupportElement::copy_for_new_area(&elem), max_influence_area);
                    bypass_merge_areas.push(id);
                } else {
                    let k = elem.key();
                    influence_areas.entry(k).or_insert_with(|| (elem.clone(), max_influence_area));
                    if elem.to_buildplate {
                        to_bp_areas.entry(k).or_insert_with(|| (elem.clone(), to_bp_data.clone()));
                    }
                    if config.support_rests_on_model {
                        to_model_areas.entry(k).or_insert_with(|| (elem.clone(), to_model_data.clone()));
                    }
                }
            } else {
                self.arena.elems[parent].result_on_layer = UNSET;
            }
        }
    }

    /// `createLayerPathing`.
    fn create_layer_pathing(&mut self, move_bounds: &mut [BTreeSet<ElemId>]) {
        let config = self.config().clone();
        let mut last_merge: LayerIndex = move_bounds.len() as LayerIndex;
        let mut new_element = false;
        let max_merge_every_x_layers = cmin(
            cmin(5000 / cmax(config.maximum_move_distance, 100), 1000 / cmax(config.maximum_move_distance_slow, 20)),
            3000 / config.layer_height,
        ) as usize;
        let mut merge_every_x_layers: usize = 1;

        for layer_idx in (1..move_bounds.len() as LayerIndex).rev() {
            let mut merge_this_layer = (last_merge - layer_idx) as usize >= merge_every_x_layers;
            if new_element {
                merge_this_layer = true;
                merge_every_x_layers = 1;
            }

            let mut influence_areas = PropertyAreas::new();
            let mut to_model_areas = PropertyAreas::new();
            let mut to_bp_areas = PropertyAreasUnordered::new();
            let mut bypass_merge_areas: Vec<ElemId> = Vec::new();

            let last_layer: Vec<ElemId> = move_bounds[layer_idx as usize].iter().copied().collect();
            self.increase_areas(
                &mut to_bp_areas,
                &mut to_model_areas,
                &mut influence_areas,
                &mut bypass_merge_areas,
                &last_layer,
                layer_idx,
                merge_this_layer,
            );

            if merge_this_layer {
                let count_before_merge = influence_areas.len();
                self.merge_influence_areas(&mut to_bp_areas, &mut to_model_areas, &mut influence_areas, layer_idx);
                last_merge = layer_idx;
                let reduced_by_merging = count_before_merge > influence_areas.len();
                if !reduced_by_merging && !new_element {
                    merge_every_x_layers = cmin(max_merge_every_x_layers, merge_every_x_layers + 1);
                }
            }

            new_element = !move_bounds[(layer_idx - 1) as usize].is_empty();

            for (_, (elem, area)) in influence_areas {
                let new_area = tsu::safe_union1(&area);
                let small = new_area.area() < 1.0;
                let parents = elem.parents.len();
                let to_bp = elem.to_buildplate;
                let id = self.arena.alloc(TreeSupportElement::copy_for_new_area(&elem), new_area);
                move_bounds[(layer_idx - 1) as usize].insert(id);
                if small {
                    crate::log_error!("Insert Error of Influence area on layer {}. Origin of {} areas. Was to bp {}", layer_idx - 1, parents, to_bp);
                }
            }
            for id in bypass_merge_areas {
                if self.arena.areas[id].area() < 1.0 {
                    crate::log_error!("Insert Error of Influence area bypass on layer {}.", layer_idx - 1);
                }
                move_bounds[(layer_idx - 1) as usize].insert(id);
            }
        }
    }

    /// `setPointsOnAreas`.
    fn set_points_on_areas(&mut self, elem: ElemId) {
        if self.arena.elems[elem].result_on_layer == UNSET {
            crate::log_error!("Uninitialized support element");
            return;
        }
        let parents = self.arena.elems[elem].parents.clone();
        for next_elem in parents {
            if self.arena.elems[next_elem].result_on_layer != UNSET {
                continue;
            }
            let mut from = self.arena.elems[elem].result_on_layer;
            if !self.arena.areas[next_elem].inside(from, true) {
                PolygonUtils::move_inside(&self.arena.areas[next_elem], &mut from, 0, i64::MAX);
            }
            self.arena.elems[next_elem].result_on_layer = from;
        }
    }

    /// `setToModelContact`.
    fn set_to_model_contact(&mut self, move_bounds: &mut [BTreeSet<ElemId>], first_elem: ElemId, layer_idx: LayerIndex) -> bool {
        let config = self.config().clone();
        if self.arena.elems[first_elem].to_model_gracious {
            let mut check = first_elem;
            let mut checked: Vec<ElemId> = Vec::new();
            let mut last_successfull_layer = layer_idx;
            let mut set = false;
            if config.support_rest_preference != RestPreference::Buildplate && layer_idx == 0 {
                set = true;
            }
            let mut valid_place_area = Shape::default();
            let mut layer_check = layer_idx;
            while self.arena.elems[check].next_height >= layer_check {
                let check_valid_place_area = self.arena.areas[check]
                    .intersection(&self.volumes().get_placeable_areas(config.get_collision_radius(&self.arena.elems[check]), layer_check));
                if !check_valid_place_area.is_empty() {
                    set = true;
                    last_successfull_layer = layer_check;
                    valid_place_area = check_valid_place_area;
                }
                checked.push(check);
                if self.arena.elems[check].parents.len() == 1 {
                    check = self.arena.elems[check].parents[0];
                } else {
                    break;
                }
                layer_check += 1;
            }

            if !set {
                if SUPPORT_TREE_ONLY_GRACIOUS_TO_MODEL {
                    crate::log_warn!("No valid placement found for to model gracious element on layer {}: REMOVING BRANCH", layer_idx);
                    let next_height = self.arena.elems[first_elem].next_height;
                    let mut layer = layer_idx;
                    while layer <= next_height {
                        move_bounds[layer as usize].remove(&checked[(layer - layer_idx) as usize]);
                        layer += 1;
                    }
                    return true;
                } else {
                    crate::log_warn!("No valid placement found for to model gracious element on layer {}", layer_idx);
                    self.arena.elems[first_elem].to_model_gracious = false;
                    return self.set_to_model_contact(move_bounds, first_elem, layer_idx);
                }
            }

            let mut layer = layer_idx + 1;
            while layer < last_successfull_layer - 1 {
                move_bounds[layer as usize].remove(&checked[(layer - layer_idx) as usize]);
                layer += 1;
            }

            if config.support_rest_preference != RestPreference::Buildplate && last_successfull_layer == 0 {
                return false;
            }

            let target = checked[(last_successfull_layer - layer_idx) as usize];
            let mut best = self.arena.elems[target].next_position;
            if !valid_place_area.inside(best, true) {
                PolygonUtils::move_inside(&valid_place_area, &mut best, 0, i64::MAX);
            }
            self.arena.elems[target].result_on_layer = best;
            crate::log_debug!("Added gracious Support On Model Point ({},{}). The current layer is {}", best.x, best.y, last_successfull_layer);
            last_successfull_layer != layer_idx
        } else {
            let fe = self.arena.elems[first_elem].clone();
            let mut best = fe.next_position;
            let cr = config.get_collision_radius(&fe);
            // Note: `getAvoidance(radius, layer, COLLISION, first_elem->use_min_xy_dist_)` passes use_min_xy_dist as `to_model`.
            let mut valid_place_area =
                self.arena.areas[first_elem].difference(&self.volumes().get_avoidance(cr, layer_idx, AvoidanceType::Collision, fe.use_min_xy_dist, false));
            if !valid_place_area.inside(best, true) {
                if !valid_place_area.is_empty() {
                    PolygonUtils::move_inside(&valid_place_area, &mut best, 0, i64::MAX);
                } else {
                    // Uninitialised in the C++ code; assumed false.
                    let mut found_partial_placement = false;
                    for radius_offset in [-cr, -cr / 2, 0] {
                        valid_place_area =
                            self.arena.areas[first_elem].intersection(&self.volumes().get_accumulated_placeable0(layer_idx).offset_default(radius_offset));
                        if !valid_place_area.is_empty() {
                            PolygonUtils::move_inside(&valid_place_area, &mut best, 0, i64::MAX);
                            crate::log_warn!(
                                "Not able to place branch fully on non support blocker at layer {} using offset {} for radius {}",
                                layer_idx,
                                radius_offset,
                                cr
                            );
                            found_partial_placement = true;
                            break;
                        }
                    }
                    if !found_partial_placement {
                        PolygonUtils::move_inside(&self.arena.areas[first_elem], &mut best, 0, i64::MAX);
                        crate::log_warn!("Not able to place branch on non support blocker at layer {}", layer_idx);
                    }
                }
            }
            self.arena.elems[first_elem].result_on_layer = best;
            self.arena.elems[first_elem].to_model_gracious = false;
            crate::log_debug!("Added NON gracious Support On Model Point ({},{}). The current layer is {}", best.x, best.y, layer_idx);
            false
        }
    }

    /// `createNodesFromArea`.
    fn create_nodes_from_area(&mut self, move_bounds: &mut [BTreeSet<ElemId>]) {
        let config = self.config().clone();
        let mut remove: Vec<ElemId> = Vec::new();
        let layer0: Vec<ElemId> = move_bounds[0].iter().copied().collect();
        for init in layer0 {
            let mut p = self.arena.elems[init].next_position;
            if !self.arena.areas[init].inside(p, true) {
                PolygonUtils::move_inside(&self.arena.areas[init], &mut p, 0, i64::MAX);
            }
            self.arena.elems[init].result_on_layer = p;
            self.set_points_on_areas(init);
            if config.support_rest_preference != RestPreference::Buildplate {
                if self.set_to_model_contact(move_bounds, init, 0) {
                    remove.push(init);
                } else {
                    self.arena.set_to_buildplate_for_all_parents(init, true);
                }
            }
        }
        for del in remove.drain(..) {
            move_bounds[0].remove(&del);
        }

        for layer_idx in 1..move_bounds.len() {
            let elems: Vec<ElemId> = move_bounds[layer_idx].iter().copied().collect();
            for elem in elems {
                let mut removed = false;
                if self.arena.elems[elem].result_on_layer == UNSET {
                    let e = &self.arena.elems[elem];
                    if e.to_buildplate || (!e.to_buildplate && e.distance_to_top < config.min_dtt_to_model && !e.supports_roof) {
                        if e.to_buildplate {
                            crate::log_error!(
                                "Uninitialized Influence area targeting ({},{}) at target_height: {} layer: {}",
                                e.target_position.x,
                                e.target_position.y,
                                e.target_height,
                                layer_idx
                            );
                        }
                        remove.push(elem);
                        for parent in self.arena.elems[elem].parents.clone() {
                            self.arena.elems[parent].result_on_layer = UNSET;
                        }
                        continue;
                    } else {
                        removed = self.set_to_model_contact(move_bounds, elem, layer_idx as LayerIndex);
                        if removed {
                            remove.push(elem);
                        }
                    }
                }
                if !removed {
                    self.set_points_on_areas(elem);
                }
            }
            for del in remove.drain(..) {
                move_bounds[layer_idx].remove(&del);
            }
        }
    }

    /// `generateBranchAreas`.
    fn generate_branch_areas(
        &self,
        linear_data: &[(LayerIndex, ElemId)],
        layer_tree_polygons: &mut [BTreeMap<ElemId, Shape>],
        inverse_tree_order: &HashMap<ElemId, ElemId>,
    ) {
        let config = self.config();
        let volumes = self.volumes();
        let branch_circle: Vec<Point2LL> =
            base_circle().iter().map(|v| pt(v.x * config.branch_radius / BASE_RADIUS, v.y * config.branch_radius / BASE_RADIUS)).collect();

        let mut linear_inserts: Vec<Shape> = Vec::with_capacity(linear_data.len());
        for &(layer, elem_id) in linear_data {
            let elem = &self.arena.elems[elem_id];
            let radius = config.get_radius(elem);
            let mut parent_uses_min = false;
            let child_elem = inverse_tree_order.get(&elem_id).copied();

            let mut movement_directions: Vec<(Point2LL, Coord)> = vec![(pt(0, 0), radius)];
            if !elem.skip_ovalisation {
                if let Some(child) = child_elem {
                    let movement = self.arena.elems[child].result_on_layer - elem.result_on_layer;
                    movement_directions.push((movement, radius));
                }
                for &parent in &elem.parents {
                    let pe = &self.arena.elems[parent];
                    let movement = pe.result_on_layer - elem.result_on_layer;
                    // `config.getRadius(parent)` is called with a pointer, which converts through the
                    // `TreeSupportElement(TreeSupportElement* element_above)` constructor (dtt + 1, no result).
                    let as_child = TreeSupportElement::from_above(pe, parent);
                    movement_directions.push((movement, cmax(config.get_radius(&as_child), config.support_line_width)));
                    parent_uses_min |= pe.use_min_xy_dist;
                }
                for &target in &elem.additional_ovalization_targets {
                    let movement = target - elem.result_on_layer;
                    movement_directions.push((movement, cmax(radius, config.support_line_width)));
                }
            }

            let collision_min = parent_uses_min || elem.use_min_xy_dist;
            let generate_area = |offset: Coord| -> Shape {
                let mut poly = Shape::default();
                for &(movement, mradius) in &movement_directions {
                    let used_scale = (mradius + offset) as f64 / (1.0 * config.branch_radius as f64);
                    let center_position = elem.result_on_layer + movement / 2;
                    let move_x = movement.x as f64 / (used_scale * config.branch_radius as f64);
                    let move_y = movement.y as f64 / (used_scale * config.branch_radius as f64);
                    let vsize_inv = 0.5 / (0.01 + (move_x * move_x + move_y * move_y).sqrt());
                    let matrix = [
                        used_scale * (1.0 + move_x * move_x * vsize_inv),
                        used_scale * (0.0 + move_x * move_y * vsize_inv),
                        used_scale * (0.0 + move_x * move_y * vsize_inv),
                        used_scale * (1.0 + move_y * move_y * vsize_inv),
                    ];
                    let mut circle = Vec::with_capacity(branch_circle.len());
                    for vertex in &branch_circle {
                        let v = pt(
                            (matrix[0] * vertex.x as f64 + matrix[1] * vertex.y as f64) as Coord,
                            (matrix[2] * vertex.x as f64 + matrix[3] * vertex.y as f64) as Coord,
                        );
                        circle.push(center_position + v);
                    }
                    poly.push_all_owned(Polygon::from_points(circle).offset_default(0));
                }
                poly.union_polygons()
                    .offset_default(cmin(FUDGE_LENGTH, config.support_line_width / 4))
                    .difference(&volumes.get_collision(0, layer, collision_min))
            };

            // `fast_relative_movement` is evaluated before generateArea updates max_speed_sqd, so it is
            // always false.
            let fast_relative_movement = false;
            let mut insert = generate_area(0);

            if fast_relative_movement || config.get_radius(elem) - config.get_collision_radius(elem) > config.support_line_width {
                let mut nozzle_path = insert.offset_default(-config.support_line_width / 2);
                if nozzle_path.split_into_parts(false).len() > 1 {
                    insert = generate_area(config.support_line_width / 2);
                    nozzle_path = insert.offset_default(-config.support_line_width / 2);
                    if nozzle_path.split_into_parts(false).len() > 1 {
                        let mut polygons_with_correct_center = Shape::default();
                        for part in nozzle_path.split_into_parts(false) {
                            if part.inside(elem.result_on_layer, true) {
                                polygons_with_correct_center = polygons_with_correct_center.union_polygons_other(&part);
                            } else {
                                let mut from = elem.result_on_layer;
                                PolygonUtils::move_inside(&part, &mut from, 0, i64::MAX);
                                if v_size2(elem.result_on_layer - from) < (FUDGE_LENGTH * FUDGE_LENGTH) / 4 {
                                    polygons_with_correct_center = polygons_with_correct_center.union_polygons_other(&part);
                                }
                            }
                        }
                        insert = polygons_with_correct_center.offset_default(config.support_line_width / 2).union_polygons();
                        insert = insert.difference(&volumes.get_collision(0, layer, collision_min)).union_polygons();
                    }
                }
            }
            linear_inserts.push(insert);
        }

        if crate::debug_dump::enabled() {
            let mut ss = String::new();
            for (i, &(layer, elem_id)) in linear_data.iter().enumerate() {
                let elem = &self.arena.elems[elem_id];
                let child = inverse_tree_order.get(&elem_id).copied();
                ss.push_str(&format!(
                    "tree_branch mesh=0 layer={} th={} tp={},{} res={},{} r={} child={}",
                    layer,
                    elem.target_height,
                    elem.target_position.x,
                    elem.target_position.y,
                    elem.result_on_layer.x,
                    elem.result_on_layer.y,
                    config.get_radius(elem),
                    child.is_some() as i32
                ));
                if let Some(c) = child {
                    let ce = &self.arena.elems[c];
                    ss.push_str(&format!(" cres={},{}", ce.result_on_layer.x, ce.result_on_layer.y));
                }
                for &p in &elem.parents {
                    let pe = &self.arena.elems[p];
                    ss.push_str(&format!(" pres={},{},{}", pe.result_on_layer.x, pe.result_on_layer.y, config.get_radius(pe)));
                }
                ss.push('\n');
                ss.push_str(&crate::debug_dump::shape_to_string(&linear_inserts[i]));
            }
            crate::debug_dump::write(&ss);
        }
        for (i, insert) in linear_inserts.into_iter().enumerate() {
            layer_tree_polygons[linear_data[i].0 as usize].entry(linear_data[i].1).or_insert(insert);
        }
    }

    /// `smoothBranchAreas`.
    fn smooth_branch_areas(&mut self, layer_tree_polygons: &mut [BTreeMap<ElemId, Shape>]) {
        let config = self.config().clone();
        let max_radius_change_per_layer = 1 + config.support_line_width / 2;
        let n = cmax(layer_tree_polygons.len(), 1) - 1;

        // Smooth upward.
        for layer_idx in 0..n {
            let processing: Vec<(ElemId, Shape)> = layer_tree_polygons[layer_idx].iter().map(|(k, v)| (*k, v.clone())).collect();
            let mut update_next: Vec<Vec<(ElemId, Shape)>> = vec![Vec::new(); processing.len()];
            for (processing_idx, (elem_id, area)) in processing.iter().enumerate() {
                let elem = &self.arena.elems[*elem_id];
                let mut max_outer_wall_distance: Coord = 0;
                let mut do_something = false;
                for &parent in &elem.parents {
                    let pe = &self.arena.elems[parent];
                    if config.get_radius(pe) != config.get_collision_radius(pe) {
                        do_something = true;
                        max_outer_wall_distance = cmax(
                            max_outer_wall_distance,
                            v_size(elem.result_on_layer - pe.result_on_layer) - (config.get_radius(elem) - config.get_radius(pe)),
                        );
                    }
                }
                max_outer_wall_distance += max_radius_change_per_layer;
                if do_something {
                    let max_allowed_area = area.offset_default(max_outer_wall_distance);
                    for &parent in &elem.parents {
                        let pe = &self.arena.elems[parent];
                        if config.get_radius(pe) != config.get_collision_radius(pe) {
                            let above = layer_tree_polygons[layer_idx + 1].entry(parent).or_default();
                            update_next[processing_idx].push((parent, above.intersection(&max_allowed_area)));
                        }
                    }
                }
            }
            for data_vector in update_next {
                for (k, v) in data_vector {
                    layer_tree_polygons[layer_idx + 1].insert(k, v);
                }
            }
        }

        // Smooth downwards.
        let mut updated_last_iteration: HashSet<ElemId> = HashSet::new();
        for layer_idx in (0..n).rev() {
            let processing: Vec<(ElemId, Shape)> = layer_tree_polygons[layer_idx].iter().map(|(k, v)| (*k, v.clone())).collect();
            let mut update_next: Vec<Option<(ElemId, Shape)>> = vec![None; processing.len()];
            for (processing_idx, (elem_id, area)) in processing.iter().enumerate() {
                let elem = &self.arena.elems[*elem_id];
                let mut do_something = false;
                let mut max_allowed_area = Shape::default();
                for &parent in &elem.parents {
                    let pe = &self.arena.elems[parent];
                    let max_outer_line_increase = max_radius_change_per_layer;
                    let mut result = layer_tree_polygons[layer_idx + 1].entry(parent).or_default().offset_default(max_outer_line_increase);
                    let direction = elem.result_on_layer - pe.result_on_layer;
                    for outer in result.iter_mut() {
                        for p in outer.iter_mut() {
                            *p += direction;
                        }
                    }
                    max_allowed_area.push_all_owned(result);
                    do_something =
                        do_something || updated_last_iteration.contains(&parent) || config.get_collision_radius(pe) != config.get_radius(pe);
                }
                if do_something {
                    let result = max_allowed_area.union_polygons().intersection(area);
                    if result.area() < area.area() {
                        update_next[processing_idx] = Some((*elem_id, result));
                    }
                }
            }
            updated_last_iteration.clear();
            for (k, v) in update_next.into_iter().flatten() {
                updated_last_iteration.insert(k);
                layer_tree_polygons[layer_idx].insert(k, v);
            }
        }
    }

    /// `dropNonGraciousAreas`.
    fn drop_non_gracious_areas(
        &self,
        layer_tree_polygons: &mut [BTreeMap<ElemId, Shape>],
        linear_data: &[(LayerIndex, ElemId)],
        dropped_down_areas: &mut [Vec<(LayerIndex, Shape)>],
        inverse_tree_order: &HashMap<ElemId, ElemId>,
    ) {
        let volumes = self.volumes();
        for (idx, &(layer, elem_id)) in linear_data.iter().enumerate() {
            let elem = &self.arena.elems[elem_id];
            let non_gracious_model_contact = !elem.to_model_gracious && !inverse_tree_order.contains_key(&elem_id) && layer > 0 && !elem.to_buildplate;
            if non_gracious_model_contact {
                let mut rest_support =
                    layer_tree_polygons[layer as usize].entry(elem_id).or_default().intersection(&volumes.get_accumulated_placeable0(layer));
                let mut counter: LayerIndex = 1;
                while rest_support.area() > 1.0 && counter < layer {
                    rest_support = rest_support.difference(&volumes.get_collision(0, layer - counter, false));
                    dropped_down_areas[idx].push((layer - counter, rest_support.clone()));
                    counter += 1;
                }
            }
        }
    }

    /// `filterFloatingLines`.
    fn filter_floating_lines(&self, support_layer_storage: &mut [Shape]) {
        let config = self.config();
        let closing_dist = config.support_line_width * config.support_wall_count as Coord;
        let open_close_distance = if config.fill_outline_gaps { config.min_feature_size / 2 - 5 } else { config.min_wall_line_width / 2 - 5 };
        let small_area_length = (config.support_line_width as f64 / 2.0) / 1000.0;

        let reverse_polygon = |poly: &mut Shape| {
            for p in poly.iter_mut() {
                p.reverse_points();
            }
        };

        let n = support_layer_storage.len();
        let mut support_holes: Vec<Shape> = vec![Shape::default(); n];
        for layer_idx in 0..n {
            let s = &mut support_layer_storage[layer_idx];
            *s = config
                .simplifier
                .shape(&PolygonUtils::union_many_small(&s.smooth(FUDGE_LENGTH as i32)))
                .offset_default(-open_close_distance)
                .offset_default(open_close_distance * 2)
                .offset_default(-open_close_distance);
            s.remove_small_areas(small_area_length * small_area_length, false);
            let parts = s.sort_by_nesting();
            if parts.len() <= 1 {
                continue;
            }
            let mut holes_original = Shape::default();
            for part in parts.iter().skip(1) {
                let mut area = part.clone();
                reverse_polygon(&mut area);
                holes_original.push_all_owned(area);
            }
            support_holes[layer_idx] = holes_original;
        }

        let mut holeparts: Vec<Vec<Shape>> = vec![Vec::new(); n];
        for layer_idx in 0..n {
            for hole in support_holes[layer_idx].split_into_parts(false) {
                holeparts[layer_idx].push(hole.into_shape());
            }
        }
        let mut hole_rest_map: Vec<BTreeMap<usize, Vec<usize>>> = vec![BTreeMap::new(); holeparts.len()];
        let mut holes_resting_outside: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); holeparts.len()];

        let volumes = self.volumes();
        for layer_idx in 1..n {
            if holeparts[layer_idx].is_empty() {
                continue;
            }
            let relevant_forbidden = volumes.get_collision(0, layer_idx as LayerIndex, true);
            let outer_walls = tsu::to_polylines(&support_layer_storage[layer_idx - 1].get_outside_polygons()).create_tube_shape(closing_dist, 0);
            for (idx, hole) in holeparts[layer_idx].iter().enumerate() {
                let mut hole_aabb = AABB::from_shape(hole);
                hole_aabb.expand(EPSILON as i32);
                if hole.len() > 1 {
                    holes_resting_outside[layer_idx].insert(idx);
                } else if !hole.intersection(&PolygonUtils::clip_polygon_with_aabb(&outer_walls, &hole_aabb)).is_empty() {
                    holes_resting_outside[layer_idx].insert(idx);
                } else if !hole
                    .intersection(&PolygonUtils::clip_polygon_with_aabb(&relevant_forbidden, &hole_aabb))
                    .offset_default(-config.xy_min_distance / 2)
                    .is_empty()
                {
                    holes_resting_outside[layer_idx].insert(idx);
                } else {
                    for (idx2, hole2) in holeparts[layer_idx - 1].iter().enumerate() {
                        if hole_aabb.hit(&AABB::from_shape(hole2)) && !hole.intersection(&PolygonUtils::clip_polygon_with_aabb(hole2, &hole_aabb)).is_empty() {
                            hole_rest_map[layer_idx].entry(idx).or_default().push(idx2);
                        }
                    }
                }
            }
        }

        let mut removed_holes_by_idx: HashSet<usize> = HashSet::new();
        let mut valid_holes: Vec<Shape> = vec![Shape::default(); support_holes.len()];
        for layer_idx in 1..support_holes.len() {
            let mut next_removed_holes_by_idx: HashSet<usize> = HashSet::new();
            for idx in 0..holeparts[layer_idx].len() {
                let mut found = false;
                if holes_resting_outside[layer_idx].contains(&idx) {
                    found = true;
                } else if let Some(rests) = hole_rest_map[layer_idx].get(&idx) {
                    for resting_idx in rests {
                        if !removed_holes_by_idx.contains(resting_idx) {
                            found = true;
                            break;
                        }
                    }
                }
                if !found {
                    next_removed_holes_by_idx.insert(idx);
                } else {
                    let hole = std::mem::take(&mut holeparts[layer_idx][idx]);
                    valid_holes[layer_idx].push_all_owned(hole);
                }
            }
            removed_holes_by_idx = next_removed_holes_by_idx;
        }

        for layer_idx in 1..n {
            if holeparts[layer_idx].is_empty() {
                continue;
            }
            support_layer_storage[layer_idx] = support_layer_storage[layer_idx].get_outside_polygons();
            reverse_polygon(&mut valid_holes[layer_idx]);
            support_layer_storage[layer_idx].push_all(&valid_holes[layer_idx]);
        }
    }

    /// `finalizeInterfaceAndSupportAreas`.
    fn finalize_interface_and_support_areas(
        &self,
        support_layer_storage: &mut [Shape],
        support_roof_storage: &[Shape],
        support_layer_storage_fractional: &[Shape],
        storage: &mut SliceDataStorage,
    ) {
        let config = self.config();
        let interface_pref = config.interface_preference;
        let n = support_layer_storage.len();
        let cfp = storage.support.cross_fill_provider.clone();
        for layer_idx in 0..n {
            let li = layer_idx as LayerIndex;
            let mut fake_roof_lines = Shape::default();
            for f_roof in &self.fake_roof_areas[layer_idx] {
                fake_roof_lines.push_all_owned(
                    tsu::generate_support_infill_lines(&f_roof.area, config, false, li, f_roof.line_distance, cfp.as_deref(), false, false)
                        .offset_default(config.support_line_width / 2),
                );
            }
            fake_roof_lines = fake_roof_lines.union_polygons();
            support_layer_storage[layer_idx] = support_layer_storage[layer_idx].difference(&fake_roof_lines);

            {
                let sl = &mut storage.support.support_layers[layer_idx];
                sl.support_roof = sl.support_roof.union_polygons_other(&support_roof_storage[layer_idx]);
            }
            let support_roof = storage.support.support_layers[layer_idx].support_roof.clone();
            if !support_roof.is_empty() && support_layer_storage[layer_idx].intersection(&support_roof).area() > 1.0 {
                match interface_pref {
                    InterfacePreference::InterfaceAreaOverwritesSupport => {
                        support_layer_storage[layer_idx] = support_layer_storage[layer_idx].difference(&support_roof);
                    }
                    InterfacePreference::SupportAreaOverwritesInterface => {
                        storage.support.support_layers[layer_idx].support_roof = support_roof.difference(&support_layer_storage[layer_idx]);
                    }
                    InterfacePreference::InterfaceLinesOverwriteSupport => {
                        let interface_lines =
                            tsu::generate_support_infill_lines(&support_roof, config, true, li, config.support_roof_line_distance, cfp.as_deref(), true, false)
                                .offset_default(config.support_roof_line_width / 2);
                        support_layer_storage[layer_idx] = support_layer_storage[layer_idx].difference(&interface_lines);
                    }
                    InterfacePreference::SupportLinesOverwriteInterface => {
                        let tree_lines = Shape::default().union_polygons_other(
                            &tsu::generate_support_infill_lines(
                                &support_layer_storage[layer_idx],
                                config,
                                false,
                                li,
                                config.support_line_distance,
                                cfp.as_deref(),
                                true,
                                false,
                            )
                            .offset_default(config.support_line_width / 2),
                        );
                        storage.support.support_layers[layer_idx].support_roof = support_roof.difference(&tree_lines);
                    }
                    InterfacePreference::Nothing => {}
                }
            }

            if config.support_bottom_layers > 0 && !support_layer_storage[layer_idx].is_empty() {
                let mut floor_layer = storage.support.support_layers[layer_idx].support_bottom.clone();
                let layer_outset = support_layer_storage[layer_idx]
                    .offset_default(config.support_bottom_offset)
                    .difference(&self.volumes().get_collision(0, li, false));
                let mut layers_below: usize = 0;
                while layers_below <= config.support_bottom_layers {
                    let sample_layer = cmax(0, (layer_idx as i32 - layers_below as i32) - config.z_distance_bottom_layers as i32) as usize;
                    floor_layer.push_all_owned(layer_outset.intersection(&storage.get_layer_outlines(sample_layer as LayerIndex, false, false, false, -1, true)));
                    if layers_below < config.support_bottom_layers {
                        layers_below = cmin(layers_below + 1, config.support_bottom_layers);
                    } else {
                        break;
                    }
                }
                floor_layer = floor_layer.union_polygons();
                let sl = &mut storage.support.support_layers[layer_idx];
                sl.support_bottom = sl.support_bottom.union_polygons_other(&floor_layer);
                support_layer_storage[layer_idx] = support_layer_storage[layer_idx].difference(&floor_layer.offset_default(10));
            }
        }

        for layer_idx in 0..n {
            let convert_every_part = true;
            let swc = config.support_wall_count as Coord;
            let fractional_support = if layer_idx > 0 {
                support_layer_storage_fractional[layer_idx].intersection(&support_layer_storage[layer_idx - 1])
            } else {
                support_layer_storage_fractional[layer_idx].clone()
            };
            let sl = &mut storage.support.support_layers[layer_idx];
            sl.fill_infill_parts_area(&support_layer_storage[layer_idx], config.support_line_width, swc, false, convert_every_part, 0);
            sl.fill_infill_parts_area(&fractional_support, config.support_line_width, swc, true, convert_every_part, 0);
            for fake_roof in &self.fake_roof_areas[layer_idx] {
                sl.fill_infill_parts_area(&fake_roof.area, config.support_line_width, 0, fake_roof.fractional, convert_every_part, fake_roof.line_distance);
            }
            if !sl.support_infill_parts.is_empty() || !sl.support_roof.is_empty() {
                storage.support.layer_nr_max_filled_layer = cmax(storage.support.layer_nr_max_filled_layer, layer_idx as i32);
            }
        }
    }

    /// `drawAreas`.
    fn draw_areas(&mut self, move_bounds: &mut [BTreeSet<ElemId>], storage: &mut SliceDataStorage) {
        let config = self.config().clone();
        let n = move_bounds.len();
        let mut support_layer_storage: Vec<Shape> = vec![Shape::default(); n];
        let mut support_layer_storage_fractional: Vec<Shape> = vec![Shape::default(); n];
        let mut support_roof_storage_fractional: Vec<Shape> = vec![Shape::default(); n];
        let mut support_roof_storage: Vec<Shape> = vec![Shape::default(); n];
        let mut inverse_tree_order: HashMap<ElemId, ElemId> = HashMap::new();
        let mut linear_data: Vec<(LayerIndex, ElemId)> = Vec::new();
        for layer_idx in 0..n {
            for &elem_id in move_bounds[layer_idx].iter() {
                let elem = &self.arena.elems[elem_id];
                let inv = inverse_tree_order.get(&elem_id).copied();
                if layer_idx > 0
                    && ((inv.is_none() && elem.target_height == layer_idx as LayerIndex && config.min_dtt_to_model > 0 && !elem.to_buildplate)
                        || (inv.is_some() && self.arena.elems[inv.unwrap()].result_on_layer == UNSET))
                {
                    continue;
                }
                for &par in &elem.parents {
                    if self.arena.elems[par].result_on_layer == UNSET {
                        continue;
                    }
                    inverse_tree_order.entry(par).or_insert(elem_id);
                }
                linear_data.push((layer_idx as LayerIndex, elem_id));
            }
        }

        let mut layer_tree_polygons: Vec<BTreeMap<ElemId, Shape>> = vec![BTreeMap::new(); n];
        self.generate_branch_areas(&linear_data, &mut layer_tree_polygons, &inverse_tree_order);
        self.smooth_branch_areas(&mut layer_tree_polygons);
        let mut dropped_down_areas: Vec<Vec<(LayerIndex, Shape)>> = vec![Vec::new(); linear_data.len()];
        self.drop_non_gracious_areas(&mut layer_tree_polygons, &linear_data, &mut dropped_down_areas, &inverse_tree_order);

        for areas in &dropped_down_areas {
            for (layer, shape) in areas {
                support_layer_storage[*layer as usize].push_all(shape);
            }
        }

        // Ensure all branch areas added as roof actually cause a roofline to generate.
        for layer_idx in 0..layer_tree_polygons.len() {
            let entries: Vec<ElemId> = layer_tree_polygons[layer_idx].keys().copied().collect();
            for elem_id in entries {
                let e = &self.arena.elems[elem_id];
                if e.missing_roof_layers > e.distance_to_top
                    && tsu::generate_support_infill_lines(
                        &layer_tree_polygons[layer_idx][&elem_id],
                        &config,
                        true,
                        layer_idx as LayerIndex,
                        config.support_roof_line_distance,
                        None,
                        true,
                        false,
                    )
                    .is_empty()
                {
                    // `to_disable_roofs` only ever reaches the element itself (the loop condition is
                    // evaluated on the element after resetting it).
                    self.arena.elems[elem_id].missing_roof_layers = 0;
                }
            }
        }

        for layer_idx in 0..layer_tree_polygons.len() {
            for (&elem_id, shape) in layer_tree_polygons[layer_idx].iter() {
                let e = &self.arena.elems[elem_id];
                if e.parents.is_empty()
                    && !e.supports_roof
                    && layer_idx + 1 < support_roof_storage_fractional.len()
                    && config.z_distance_top % config.layer_height > 0
                {
                    if e.missing_roof_layers > e.distance_to_top {
                        support_roof_storage_fractional[layer_idx + 1].push_all(shape);
                    } else {
                        support_layer_storage_fractional[layer_idx + 1].push_all(shape);
                    }
                }
                if e.missing_roof_layers > e.distance_to_top {
                    support_roof_storage[layer_idx].push_all(shape);
                } else {
                    support_layer_storage[layer_idx].push_all(shape);
                }
            }
            if layer_idx + 1 < support_roof_storage_fractional.len() {
                support_roof_storage_fractional[layer_idx + 1] = support_roof_storage_fractional[layer_idx + 1].union_polygons();
                support_layer_storage_fractional[layer_idx + 1] = support_layer_storage_fractional[layer_idx + 1].union_polygons();
            }
        }

        for layer_idx in 0..self.additional_required_support_area.len() {
            if support_layer_storage.len() > layer_idx {
                support_layer_storage[layer_idx].push_all(&self.additional_required_support_area[layer_idx]);
            }
        }

        dbg_dump_layers("prefilter", &support_layer_storage);
        dbg_dump_layers("roofstore", &support_roof_storage);
        self.filter_floating_lines(&mut support_layer_storage);
        dbg_dump_layers("postfilter", &support_layer_storage);
        self.finalize_interface_and_support_areas(&mut support_layer_storage, &support_roof_storage, &support_layer_storage_fractional, storage);
    }
}
