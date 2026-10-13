//! `TreeSupportTipGenerator.h/.cpp`: places the tips of the tree branches below overhangs and
//! computes the support roofs of tree support.

use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use crate::geometry::lines_set::{OpenLinesSet, Shape};
use crate::geometry::point2ll::{v_size2, Point2LL};
use crate::geometry::point3ll::Point3LL;
use crate::geometry::polyline::{OpenPolyline, Polygon, Polyline};
use crate::infill::sierpinski_fill_provider::SierpinskiFillProvider;
use crate::settings::enums::{EFillMethod, SupportDistPriority};
use crate::settings::types::{LayerIndex, TAU};
use crate::slice_data_storage::{SliceMeshStorage, SupportStorage};
use crate::utils::aabb::AABB3D;
use crate::utils::coord::{int2mm2, mm2_2int, Coord, EPSILON};
use crate::utils::math::{round_divide, round_up_divide};
use crate::utils::polygon_utils::{ClosestPoint, GivenDistPoint, PolygonUtils};
use clipper::JoinType;

use super::element::{ElemArena, ElemId, TreeSupportElement};
use super::model_volumes::{TreeModelVolumes, FUDGE_LENGTH};
use super::settings::{base_circle, cmax, cmin, AvoidanceType, RestPreference, TreeSupportSettings, BASE_RADIUS};
use super::utils as tsu;
use super::FakeRoofArea;

pub const SUPPORT_TREE_MINIMUM_FAKE_ROOF_AREA: f64 = 100.0;
pub const SUPPORT_TREE_MINIMUM_FAKE_ROOF_LAYERS: u64 = 1;
pub const SUPPORT_TREE_MINIMUM_ROOF_AREA_HARD_LIMIT: bool = false;
pub const SUPPORT_TREE_ONLY_GRACIOUS_TO_MODEL: bool = false;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineStatus {
    Invalid,
    ToModel,
    ToModelGracious,
    ToModelGraciousSafe,
    ToBp,
    ToBpSafe,
}

type LineInformation = Vec<(Point2LL, LineStatus)>;

pub struct TreeSupportTipGenerator<'a> {
    config: TreeSupportSettings,
    use_fake_roof: bool,
    volumes: &'a TreeModelVolumes,
    minimum_support_area: f64,
    minimum_roof_area: f64,
    support_roof_layers: usize,
    connect_length: Coord,
    support_tree_branch_distance: Coord,
    support_roof_line_distance: Coord,
    support_outset: Coord,
    roof_outset: Coord,
    force_tip_to_roof: bool,
    support_tree_limit_branch_reach: bool,
    support_tree_branch_reach_limit: Coord,
    z_distance_delta: usize,
    xy_overrides: bool,
    max_overhang_insert_lag: usize,
    tip_roof_size: f64,
    only_gracious: bool,
    force_minimum_roof_area: bool,
    support_supporting_branch_distance: Coord,
    cross_fill_provider: Option<Arc<SierpinskiFillProvider>>,
    already_inserted: Vec<HashSet<Point2LL>>,
    support_roof_drawn: Vec<Shape>,
    support_roof_drawn_fractional: Vec<Shape>,
    roof_tips_drawn: Vec<Shape>,
}

/// `splitLines`. Note that both results start with one empty line (`std::vector<LineInformation> keep(1)`).
fn split_lines(lines: Vec<LineInformation>, evaluate_point: &mut dyn FnMut((Point2LL, LineStatus)) -> bool) -> (Vec<LineInformation>, Vec<LineInformation>) {
    const KEEPING: bool = true;
    const FREEING: bool = false;
    let mut keep: Vec<LineInformation> = vec![Vec::new()];
    let mut set_free: Vec<LineInformation> = vec![Vec::new()];
    for line in &lines {
        let mut current = KEEPING;
        let mut resulting_line: LineInformation = Vec::new();
        for &me in line {
            if evaluate_point(me) == (current == FREEING) {
                if !resulting_line.is_empty() {
                    if current == KEEPING { &mut keep } else { &mut set_free }.push(std::mem::take(&mut resulting_line));
                }
                current = !current;
            }
            resulting_line.push(me);
        }
        if !resulting_line.is_empty() {
            if current == KEEPING { &mut keep } else { &mut set_free }.push(resulting_line);
        }
    }
    (keep, set_free)
}

fn convert_internal_to_lines(lines: &[LineInformation]) -> OpenLinesSet {
    let mut result = OpenLinesSet::default();
    for line in lines {
        result.push(OpenPolyline::new(line.iter().map(|p| p.0).collect()));
    }
    result
}

impl<'a> TreeSupportTipGenerator<'a> {
    pub fn new(mesh: &SliceMeshStorage, volumes: &'a TreeModelVolumes) -> Self {
        let s = mesh.settings();
        let mut config = TreeSupportSettings::new(s);
        let use_fake_roof = !s.get_bool("support_roof_enable");
        let minimum_support_area = s.get_f64("minimum_support_area");
        let minimum_roof_area =
            if !use_fake_roof { s.get_f64("minimum_roof_area") } else { cmax(SUPPORT_TREE_MINIMUM_FAKE_ROOF_AREA, minimum_support_area) };
        let support_roof_layers = (if s.get_bool("support_roof_enable") {
            round_divide(s.get_coord("support_roof_height") as u64, config.layer_height as u64)
        } else if use_fake_roof {
            SUPPORT_TREE_MINIMUM_FAKE_ROOF_LAYERS
        } else {
            0
        }) as usize;
        let top_rate = s.get_f64("support_tree_top_rate");
        let connect_length = ((config.support_line_width * 100) as f64 / top_rate
            + cmax(2.0 * config.min_radius as f64 - 1.0 * config.support_line_width as f64, 0.0)) as Coord;
        let pattern_mult: Coord = if config.support_pattern == EFillMethod::TRIANGLES {
            3
        } else if config.support_pattern == EFillMethod::GRID {
            2
        } else {
            1
        };
        let support_tree_branch_distance = pattern_mult * connect_length;
        let support_roof_line_distance = (if use_fake_roof {
            pattern_mult as f64 * ((config.support_line_width * 100) as f64 / top_rate)
        } else {
            s.get_coord("support_roof_line_distance") as f64
        }) as Coord;
        let support_outset: Coord = 0;
        let roof_outset = if use_fake_roof { support_outset } else { s.get_coord("support_roof_offset") };
        let force_tip_to_roof = ((config.min_radius * config.min_radius) as f64 * std::f64::consts::PI > minimum_roof_area * (1000.0 * 1000.0))
            && support_roof_layers != 0
            && !use_fake_roof;
        let support_tree_limit_branch_reach = s.get_bool("support_tree_limit_branch_reach");
        let support_tree_branch_reach_limit = if support_tree_limit_branch_reach { s.get_coord("support_tree_branch_reach_limit") } else { 0 };
        let z_distance_delta = cmin(config.z_distance_top_layers + 1, mesh.overhang_areas.len());
        let xy_overrides = config.support_overrides == SupportDistPriority::XY_OVERRIDES_Z;
        let tip_roof_size = if force_tip_to_roof { int2mm2(config.min_radius * config.min_radius) * std::f64::consts::PI } else { 0.0 };
        let force_minimum_roof_area = use_fake_roof || SUPPORT_TREE_MINIMUM_ROOF_AREA_HARD_LIMIT;
        let n = mesh.overhang_areas.len();

        let support_overhang_angle = s.get_angle_radians("support_angle").value();
        let max_overhang_speed: Coord =
            if support_overhang_angle < TAU / 4.0 { (support_overhang_angle.tan() * config.layer_height as f64) as Coord } else { Coord::MAX };
        let max_overhang_insert_lag = if max_overhang_speed < 2 {
            usize::MAX
        } else {
            cmax(round_up_divide(config.xy_distance as u64, (max_overhang_speed / 2) as u64) as usize, 2 * config.z_distance_top_layers)
        };

        let cross_fill_provider = Self::generate_cross_fill_provider(&config, mesh, support_tree_branch_distance, config.support_line_width);

        let known_z: Vec<Coord> = mesh.layers.iter().map(|l| l.print_z).collect();
        config.set_actual_z(&known_z);

        let mut dtt_when_tips_can_merge: Coord = 1;
        if (config.branch_radius as f64 * config.diameter_angle_scale_factor) < (2 * config.maximum_move_distance_slow) as f64 {
            while (2 * config.maximum_move_distance_slow * dtt_when_tips_can_merge - config.support_line_width)
                < config.get_radius_dtt(dtt_when_tips_can_merge as usize, 0.0)
            {
                dtt_when_tips_can_merge += 1;
            }
        } else {
            dtt_when_tips_can_merge = config.tip_layers as Coord;
        }
        let support_supporting_branch_distance =
            2 * config.get_radius_dtt(dtt_when_tips_can_merge as usize, 0.0) + config.support_line_width + FUDGE_LENGTH;

        TreeSupportTipGenerator {
            config,
            use_fake_roof,
            volumes,
            minimum_support_area,
            minimum_roof_area,
            support_roof_layers,
            connect_length,
            support_tree_branch_distance,
            support_roof_line_distance,
            support_outset,
            roof_outset,
            force_tip_to_roof,
            support_tree_limit_branch_reach,
            support_tree_branch_reach_limit,
            z_distance_delta,
            xy_overrides,
            max_overhang_insert_lag,
            tip_roof_size,
            only_gracious: SUPPORT_TREE_ONLY_GRACIOUS_TO_MODEL,
            force_minimum_roof_area,
            support_supporting_branch_distance,
            cross_fill_provider,
            already_inserted: vec![HashSet::new(); n],
            support_roof_drawn: vec![Shape::default(); n],
            support_roof_drawn_fractional: vec![Shape::default(); n],
            roof_tips_drawn: vec![Shape::default(); n],
        }
    }

    fn radius0(&self) -> Coord {
        self.config.get_radius_dtt(0, 0.0)
    }

    fn cfp(&self) -> Option<&SierpinskiFillProvider> {
        self.cross_fill_provider.as_deref()
    }

    /// `convertLinesToInternal`.
    fn convert_lines_to_internal(&self, polylines: &OpenLinesSet, layer_idx: LayerIndex) -> Vec<LineInformation> {
        let r0 = self.radius0();
        let v = self.volumes;
        let c = &self.config;
        let mut result = Vec::new();
        for line in polylines.iter() {
            let mut res_line: LineInformation = Vec::new();
            for &p in line.iter() {
                if c.support_rest_preference == RestPreference::Buildplate
                    && !v.get_avoidance(r0, layer_idx, AvoidanceType::FastSafe, false, !self.xy_overrides).inside(p, true)
                {
                    res_line.push((p, LineStatus::ToBpSafe));
                } else if c.support_rest_preference == RestPreference::Buildplate
                    && !v.get_avoidance(r0, layer_idx, AvoidanceType::Fast, false, !self.xy_overrides).inside(p, true)
                {
                    res_line.push((p, LineStatus::ToBp));
                } else if c.support_rests_on_model && !v.get_avoidance(r0, layer_idx, AvoidanceType::FastSafe, true, !self.xy_overrides).inside(p, true) {
                    res_line.push((p, LineStatus::ToModelGraciousSafe));
                } else if c.support_rests_on_model && !v.get_avoidance(r0, layer_idx, AvoidanceType::Fast, true, !self.xy_overrides).inside(p, true) {
                    res_line.push((p, LineStatus::ToModelGracious));
                } else if c.support_rests_on_model && !v.get_avoidance(r0, layer_idx, AvoidanceType::Collision, true, !self.xy_overrides).inside(p, true) {
                    res_line.push((p, LineStatus::ToModel));
                } else if !res_line.is_empty() {
                    result.push(std::mem::take(&mut res_line));
                }
            }
            if !res_line.is_empty() {
                result.push(res_line);
            }
        }
        result
    }

    /// `getEvaluatePointForNextLayerFunction(current_layer)` evaluated for one point.
    fn evaluate_point_for_next_layer(&self, current_layer: usize, p: (Point2LL, LineStatus)) -> bool {
        let r0 = self.radius0();
        let layer = current_layer.wrapping_sub(1) as LayerIndex;
        if self.config.support_rest_preference != RestPreference::Graceful
            && !self
                .volumes
                .get_avoidance(r0, layer, if p.1 == LineStatus::ToBpSafe { AvoidanceType::FastSafe } else { AvoidanceType::Fast }, false, !self.xy_overrides)
                .inside(p.0, true)
        {
            return true;
        }
        if self.config.support_rests_on_model && (p.1 != LineStatus::ToBp && p.1 != LineStatus::ToBpSafe) {
            if p.1 == LineStatus::ToModelGracious || p.1 == LineStatus::ToModelGraciousSafe {
                return !self
                    .volumes
                    .get_avoidance(
                        r0,
                        layer,
                        if p.1 == LineStatus::ToModelGraciousSafe { AvoidanceType::FastSafe } else { AvoidanceType::Fast },
                        true,
                        !self.xy_overrides,
                    )
                    .inside(p.0, true);
            } else {
                return !self.volumes.get_avoidance(r0, layer, AvoidanceType::Collision, true, !self.xy_overrides).inside(p.0, true);
            }
        }
        false
    }

    /// `ensureMaximumDistancePolyline`.
    fn ensure_maximum_distance_polyline(&self, input: &OpenLinesSet, distance: Coord, min_points: usize, enforce_distance: bool) -> OpenLinesSet {
        let mut result = OpenLinesSet::default();
        for part_in in input.iter() {
            if part_in.is_empty() {
                continue;
            }
            let mut part: OpenPolyline = part_in.clone();
            let length = part.length();
            let mut line = OpenPolyline::default();
            let mut current_distance = cmax(distance, FUDGE_LENGTH * 2);
            if length < 2 * distance && min_points <= 1 {
                let middle_point = ClosestPoint::<OpenPolyline>::new(part[0], 0, &part as *const OpenPolyline);
                let middle_point = PolygonUtils::walk(&middle_point, length / 2);
                line.push(middle_point.location);
            } else {
                let mut optimal_end_index = part.len() - 1;
                if part[0] == part[part.len() - 1] {
                    let mut optimal_start_index = 0usize;
                    let mut max_dist2_between_vertices: Coord = 0;
                    let n = part.len() - 1;
                    for idx in 0..n {
                        for inner_idx in 0..n {
                            let d = v_size2(part[idx] - part[inner_idx]);
                            if d > max_dist2_between_vertices {
                                optimal_start_index = idx;
                                optimal_end_index = inner_idx;
                                max_dist2_between_vertices = d;
                            }
                        }
                    }
                    part.points[..n].rotate_left(optimal_start_index);
                    let last = part.len() - 1;
                    part[last] = part[0];
                    optimal_end_index = (optimal_end_index.wrapping_sub(optimal_start_index).wrapping_add(part.len() - 1)) % (part.len() - 1);
                }

                while line.len() < min_points && current_distance >= FUDGE_LENGTH * 2 {
                    line.clear();
                    let mut current_point = part[0];
                    line.push(part[0]);
                    let should_add_endpoint = min_points > 1 || v_size2(part[0] - part[optimal_end_index]) > current_distance * current_distance;
                    let mut added_endpoint = !should_add_endpoint;
                    let mut current_index: usize = 0;
                    let mut next_point = GivenDistPoint::default();
                    let mut next_distance = current_distance;
                    while PolygonUtils::get_next_point_with_distance(current_point, next_distance, &part, current_index as i32, 0, &mut next_point)
                        && (next_point.pos as Coord) < part.len() as Coord - 1
                    {
                        if !added_endpoint && next_point.pos as i64 as usize >= optimal_end_index {
                            current_index = optimal_end_index;
                            current_point = part[optimal_end_index];
                            added_endpoint = true;
                            line.push(part[optimal_end_index]);
                            continue;
                        }
                        let mut min_distance_to_existing_point_sqd = Coord::MAX;
                        if enforce_distance {
                            for &p in line.iter() {
                                min_distance_to_existing_point_sqd = cmin(min_distance_to_existing_point_sqd, v_size2(p - next_point.location));
                            }
                        }
                        if !enforce_distance || min_distance_to_existing_point_sqd >= current_distance * current_distance {
                            line.push(next_point.location);
                            current_point = next_point.location;
                            current_index = next_point.pos as i64 as usize;
                            next_distance = current_distance;
                        } else {
                            if current_point == next_point.location {
                                crate::log_warn!(
                                    "Tree Support: Encountered a fixpoint in getNextPointWithDistance. This is expected to happen if the distance (currently {}) is smaller than 100",
                                    next_distance
                                );
                                if next_distance > 2 * current_distance {
                                    break;
                                }
                                next_distance += current_distance;
                                continue;
                            }
                            next_distance = cmax((current_distance as f64 - (min_distance_to_existing_point_sqd as f64).sqrt()) as Coord, FUDGE_LENGTH * 2);
                            current_point = next_point.location;
                            current_index = next_point.pos as i64 as usize;
                        }
                    }
                    if !added_endpoint {
                        line.push(part[optimal_end_index]);
                    }
                    current_distance = (current_distance as f64 * 0.9) as Coord;
                }
            }
            result.push(line);
        }
        result
    }

    /// `generateCrossFillProvider`.
    fn generate_cross_fill_provider(
        config: &TreeSupportSettings,
        mesh: &SliceMeshStorage,
        line_distance: Coord,
        line_width: Coord,
    ) -> Option<Arc<SierpinskiFillProvider>> {
        if config.support_pattern == EFillMethod::CROSS || config.support_pattern == EFillMethod::CROSS_3D {
            let s = mesh.settings();
            let mut aabb = AABB3D::default();
            if s.get_bool("infill_mesh") || s.get_bool("anti_overhang_mesh") {
                crate::log_warn!("Tree support tried to generate a CrossFillProvider for a non model mesh.");
                return None;
            }
            let aabb_expansion = s.get_coord("support_offset");
            let mut aabb_here = mesh.bounding_box;
            let a = aabb_here.min - Point3LL::new(-aabb_expansion, -aabb_expansion, 0);
            aabb_here.include(a);
            let b = aabb_here.max + Point3LL::new(-aabb_expansion, -aabb_expansion, 0);
            aabb_here.include(b);
            aabb.include_aabb(&aabb_here);
            let file = s.get_string("cross_support_density_image");
            if !file.is_empty() && std::fs::File::open(&file).is_ok() {
                return Some(Arc::new(SierpinskiFillProvider::new_with_image(&aabb, line_distance, line_width, &file)));
            }
            return Some(Arc::new(SierpinskiFillProvider::new(&aabb, line_distance, line_width)));
        }
        None
    }

    /// `dropOverhangAreas`.
    fn drop_overhang_areas(&self, mesh: &SliceMeshStorage, result: &mut [Shape], roof: bool) {
        let v = self.volumes;
        let c = &self.config;
        let zdd = self.z_distance_delta;
        for layer_idx in 1..(mesh.overhang_areas.len() - zdd) as LayerIndex {
            let lu = layer_idx as usize;
            if mesh.overhang_areas[lu + zdd].is_empty() || (result.len() as LayerIndex) < layer_idx {
                continue;
            }
            let relevant_forbidden = v.get_collision(if roof { 0 } else { self.radius0() }, layer_idx, !self.xy_overrides);
            let model_outline = v.get_collision(0, layer_idx, !self.xy_overrides).offset_join(-c.xy_min_distance, JoinType::Round);
            let outset = if roof { self.roof_outset } else { self.support_outset };
            let overhang_regular = tsu::safe_offset_inc(
                &mesh.overhang_areas[lu + zdd],
                outset,
                &relevant_forbidden,
                (c.min_radius as f64 * 1.75 + c.xy_min_distance as f64) as Coord,
                0,
                1,
                c.support_line_distance / 2,
                Some(&c.simplifier),
            );
            let mut remaining_overhang = mesh.overhang_areas[lu + zdd]
                .offset_default(outset)
                .difference(&overhang_regular)
                .intersection(&relevant_forbidden)
                .difference(&model_outline);
            let mut lag_ctr: usize = 1;
            while lag_ctr <= self.max_overhang_insert_lag && layer_idx - lag_ctr as LayerIndex >= 1 && !remaining_overhang.is_empty() {
                result[lu - lag_ctr].push_all(&remaining_overhang);
                let relevant_forbidden_below = v
                    .get_collision(if roof { 0 } else { self.radius0() }, layer_idx - lag_ctr as LayerIndex, !self.xy_overrides)
                    .offset_default(EPSILON);
                remaining_overhang = remaining_overhang.intersection(&relevant_forbidden_below).union_polygons().difference(&model_outline);
                lag_ctr += 1;
            }
        }
        for r in result.iter_mut() {
            *r = r.union_polygons();
        }
    }

    fn roof_avoidance_type(&self) -> AvoidanceType {
        if self.only_gracious || !self.config.support_rests_on_model {
            AvoidanceType::Fast
        } else {
            AvoidanceType::Collision
        }
    }

    /// `calculateRoofAreas`.
    fn calculate_roof_areas(&mut self, mesh: &SliceMeshStorage) {
        let n = mesh.overhang_areas.len();
        let mut potential_support_roofs = vec![Shape::default(); n];
        let mut dropped_overhangs = vec![Shape::default(); n];
        if self.xy_overrides {
            self.drop_overhang_areas(mesh, &mut dropped_overhangs, true);
        }
        let zdd = self.z_distance_delta;
        let r0 = self.radius0();
        let avo_type = self.roof_avoidance_type();
        let rests = self.config.support_rests_on_model;
        for layer_idx in 0..(n - zdd) as LayerIndex {
            let lu = layer_idx as usize;
            if mesh.overhang_areas[lu + zdd].is_empty() {
                continue;
            }
            let forbidden_here = self.volumes.get_avoidance(r0, layer_idx, avo_type, rests, !self.xy_overrides).offset_join(-r0, JoinType::Round);
            let mut full_overhang_area = tsu::safe_offset_inc(
                &mesh.full_overhang_areas[lu + zdd].union_polygons_other(&dropped_overhangs[lu]),
                self.roof_outset,
                &forbidden_here,
                self.config.support_line_width,
                0,
                1,
                self.config.support_line_distance / 2,
                Some(&self.config.simplifier),
            );
            let mut dtt_roof: LayerIndex = 0;
            while (dtt_roof as usize) < self.support_roof_layers && layer_idx - dtt_roof >= 1 {
                let forbidden_next =
                    self.volumes.get_avoidance(r0, layer_idx - (dtt_roof + 1), avo_type, rests, !self.xy_overrides).offset_join(-r0, JoinType::Round);
                full_overhang_area = full_overhang_area.difference(&forbidden_next);
                if self.force_minimum_roof_area {
                    full_overhang_area.remove_small_areas(self.minimum_roof_area, false);
                }
                if full_overhang_area.area() > EPSILON as f64 {
                    potential_support_roofs[(layer_idx - dtt_roof) as usize].push_all(&full_overhang_area);
                    if dtt_roof == 0 {
                        self.support_roof_drawn_fractional[lu].push_all(&full_overhang_area);
                    }
                } else {
                    break;
                }
                dtt_roof += 1;
            }
        }

        let xy_min = self.config.xy_min_distance;
        for psr in potential_support_roofs.iter_mut() {
            *psr = psr.union_polygons().offset_default(xy_min).union_polygons().offset_default(-xy_min).union_polygons_other(psr);
        }

        let mut additional_support_roofs = vec![Shape::default(); n];
        for layer_idx in 0..potential_support_roofs.len() as LayerIndex {
            let lu = layer_idx as usize;
            if potential_support_roofs[lu].is_empty() {
                continue;
            }
            let forbidden_here = self.volumes.get_avoidance(r0, layer_idx, avo_type, rests, !self.xy_overrides).offset_join(-r0, JoinType::Round);
            if !self.force_minimum_roof_area {
                let mut fuzzy_area = Shape::default();
                let from = -cmin(layer_idx, self.support_roof_layers as LayerIndex);
                let to = cmin(potential_support_roofs.len() as LayerIndex - layer_idx, self.support_roof_layers as LayerIndex + 1);
                for layer_offset in from..to {
                    fuzzy_area.push_all(&self.support_roof_drawn[(layer_idx + layer_offset) as usize]);
                    fuzzy_area.push_all(&potential_support_roofs[(layer_idx + layer_offset) as usize]);
                }
                fuzzy_area = fuzzy_area.union_polygons();
                fuzzy_area.remove_small_areas(cmax(self.minimum_roof_area, self.tip_roof_size), false);
                for potential_roof in potential_support_roofs[lu].difference(&forbidden_here).split_into_parts(false) {
                    if !potential_roof.intersection(&fuzzy_area).is_empty() {
                        additional_support_roofs[lu].push_all(&potential_roof);
                    }
                }
            } else {
                let mut valid_roof = potential_support_roofs[lu].difference(&forbidden_here);
                valid_roof.remove_small_areas(cmax(self.minimum_roof_area, self.tip_roof_size), false);
                additional_support_roofs[lu].push_all(&valid_roof);
            }
        }
        for (layer_idx, add) in additional_support_roofs.iter().enumerate() {
            self.support_roof_drawn[layer_idx] = self.support_roof_drawn[layer_idx].union_polygons_other(add);
        }
    }

    /// `addPointAsInfluenceArea`.
    #[allow(clippy::too_many_arguments)]
    fn add_point_as_influence_area(
        &mut self,
        arena: &mut ElemArena,
        move_bounds: &mut [BTreeSet<ElemId>],
        p: (Point2LL, LineStatus),
        dtt: usize,
        insert_layer: LayerIndex,
        dont_move_until: usize,
        roof: bool,
        skip_ovalisation: bool,
        additional_ovalization_targets: Vec<Point2LL>,
    ) {
        let to_bp = p.1 == LineStatus::ToBp || p.1 == LineStatus::ToBpSafe;
        let gracious = to_bp || p.1 == LineStatus::ToModelGracious || p.1 == LineStatus::ToModelGraciousSafe;
        let safe_radius = p.1 == LineStatus::ToBpSafe || p.1 == LineStatus::ToModelGraciousSafe;
        if !self.config.support_rests_on_model && !to_bp {
            crate::log_warn!("Tried to add an invalid support point");
            return;
        }
        let circle = Polygon::from_points(base_circle().iter().map(|&corner| p.0 + corner).collect());
        let area = circle.offset_default(0);
        let normalized = p.0 / ((self.config.min_radius + 1) / 10);
        let il = insert_layer as usize;
        if !self.already_inserted[il].contains(&normalized) {
            self.already_inserted[il].insert(normalized);
            let mut elem = TreeSupportElement::new(
                dtt as Coord,
                il,
                p.0,
                to_bp,
                gracious,
                !self.xy_overrides,
                dont_move_until,
                roof,
                safe_radius,
                !roof && self.force_tip_to_roof,
                skip_ovalisation,
                self.support_tree_limit_branch_reach,
                self.support_tree_branch_reach_limit,
            );
            elem.additional_ovalization_targets.extend(additional_ovalization_targets);
            let id = arena.alloc(elem, area);
            move_bounds[il].insert(id);
        }
    }

    fn roof_circle(&self, p: Point2LL) -> Polygon {
        let mult = cmax(self.config.min_radius / BASE_RADIUS, 1);
        Polygon::from_points(base_circle().iter().map(|&corner| p + corner * mult).collect())
    }

    /// `addLinesAsInfluenceAreas`.
    #[allow(clippy::too_many_arguments)]
    fn add_lines_as_influence_areas(
        &mut self,
        arena: &mut ElemArena,
        move_bounds: &mut [BTreeSet<ElemId>],
        mut lines: Vec<LineInformation>,
        roof_tip_layers: usize,
        insert_layer_idx: LayerIndex,
        supports_roof: bool,
        dont_move_until: usize,
        connect_points: bool,
    ) {
        let mut dtt_roof_tip: usize = 0;
        if self.config.support_roof_wall_count == 0 {
            dtt_roof_tip = 0;
            while dtt_roof_tip < roof_tip_layers && insert_layer_idx - dtt_roof_tip as LayerIndex >= 1 {
                let layer = insert_layer_idx - dtt_roof_tip as LayerIndex;
                let split = split_lines(lines, &mut |p| self.evaluate_point_for_next_layer(layer as usize, p));
                for line in &split.1 {
                    for &point_data in line {
                        self.add_point_as_influence_area(
                            arena,
                            move_bounds,
                            point_data,
                            0,
                            layer,
                            roof_tip_layers - dtt_roof_tip,
                            dtt_roof_tip != 0,
                            false,
                            Vec::new(),
                        );
                    }
                }
                let split = {
                    let this = &*self;
                    split_lines(split.0, &mut |p: (Point2LL, LineStatus)| {
                        let area = this.roof_circle(p.0).offset_default(0);
                        !tsu::generate_support_infill_lines(&area, &this.config, true, layer, this.support_roof_line_distance, this.cfp(), true, false)
                            .is_empty()
                    })
                };
                lines = split.0;
                for line in &split.1 {
                    for &point_data in line {
                        self.add_point_as_influence_area(
                            arena,
                            move_bounds,
                            point_data,
                            0,
                            layer,
                            roof_tip_layers - dtt_roof_tip,
                            dtt_roof_tip != 0,
                            false,
                            Vec::new(),
                        );
                    }
                }
                let mut added_roofs = Shape::default();
                for line in &lines {
                    for p in line {
                        added_roofs.push(self.roof_circle(p.0));
                    }
                }
                added_roofs = added_roofs.union_polygons();
                self.roof_tips_drawn[layer as usize].push_all(&added_roofs);
                if dtt_roof_tip == 0 {
                    self.support_roof_drawn_fractional[insert_layer_idx as usize].push_all(&added_roofs);
                }
                dtt_roof_tip += 1;
            }
        }

        for line in &lines {
            let disable_ovalization =
                !connect_points && self.config.min_radius < 3 * self.config.support_line_width && roof_tip_layers == 0 && dtt_roof_tip == 0;
            for (idx, &point_data) in line.iter().enumerate() {
                let mut additional_ovalization_targets = Vec::new();
                if connect_points {
                    if idx != 0 {
                        additional_ovalization_targets.push(line[idx - 1].0);
                    }
                    if idx != line.len() - 1 {
                        additional_ovalization_targets.push(line[idx + 1].0);
                    }
                }
                self.add_point_as_influence_area(
                    arena,
                    move_bounds,
                    point_data,
                    0,
                    insert_layer_idx - dtt_roof_tip as LayerIndex,
                    if dont_move_until > dtt_roof_tip { dont_move_until - dtt_roof_tip } else { 0 },
                    dtt_roof_tip != 0 || supports_roof,
                    disable_ovalization,
                    additional_ovalization_targets,
                );
            }
        }
    }

    /// `removeUselessAddedPoints`.
    fn remove_useless_added_points(
        &self,
        arena: &ElemArena,
        move_bounds: &mut [BTreeSet<ElemId>],
        support: &SupportStorage,
        additional_support_areas: &[Shape],
    ) {
        let r0 = self.radius0();
        for layer_idx in 0..move_bounds.len() {
            if layer_idx + 1 < support.support_layers.len() {
                let mut to_be_removed = Vec::new();
                let roof_on_layer_above = if self.use_fake_roof {
                    self.support_roof_drawn[layer_idx + 1].clone()
                } else {
                    support.support_layers[layer_idx + 1].support_roof.union_polygons_other(&additional_support_areas[layer_idx + 1])
                };
                let roof_on_layer = if self.use_fake_roof {
                    self.support_roof_drawn[layer_idx].clone()
                } else {
                    support.support_layers[layer_idx].support_roof.union_polygons_other(&additional_support_areas[layer_idx])
                };
                for &id in move_bounds[layer_idx].iter() {
                    let elem = &arena.elems[id];
                    if roof_on_layer.inside(elem.result_on_layer, false) {
                        to_be_removed.push(id);
                    } else if elem.supports_roof {
                        let mut from = elem.result_on_layer;
                        PolygonUtils::move_inside(&roof_on_layer_above, &mut from, 0, i64::MAX);
                        if roof_on_layer_above.is_empty()
                            || (!roof_on_layer_above.inside(elem.result_on_layer, false)
                                && v_size2(from - elem.result_on_layer) > r0 * r0 + FUDGE_LENGTH * FUDGE_LENGTH)
                        {
                            to_be_removed.push(id);
                            crate::log_warn!("Removing already placed tip that should have roof above it?");
                        }
                    }
                }
                for id in to_be_removed {
                    move_bounds[layer_idx].remove(&id);
                }
            }
        }
    }

    /// `generateTips`.
    pub fn generate_tips(
        &mut self,
        arena: &mut ElemArena,
        support: &mut SupportStorage,
        mesh: &SliceMeshStorage,
        move_bounds: &mut [BTreeSet<ElemId>],
        additional_support_areas: &mut [Shape],
        placed_fake_roof_areas: &mut [Vec<FakeRoofArea>],
    ) {
        let mut new_tips: Vec<BTreeSet<ElemId>> = vec![BTreeSet::new(); move_bounds.len()];
        let c_min_radius = self.config.min_radius;
        let slw = self.config.support_line_width;
        let circle_length_to_half_linewidth_change: Coord = if c_min_radius < slw {
            c_min_radius / 2
        } else {
            ((c_min_radius * c_min_radius - (c_min_radius - slw / 2) * (c_min_radius - slw / 2)) as f64).sqrt() as Coord
        };
        let extra_outset = cmax(0, c_min_radius - slw / 2) + if self.xy_overrides { 0 } else { slw / 2 };

        if self.support_roof_layers != 0 {
            self.calculate_roof_areas(mesh);
        }

        let zdd = self.z_distance_delta;
        let r0 = self.radius0();
        let avo_type = self.roof_avoidance_type();
        let rests = self.config.support_rests_on_model;
        for layer_idx in 1..(mesh.overhang_areas.len() - zdd) as LayerIndex {
            let lu = layer_idx as usize;
            if mesh.overhang_areas[lu + zdd].is_empty() && (lu + 1 >= self.support_roof_drawn.len() || self.support_roof_drawn[lu + 1].is_empty()) {
                continue;
            }
            let mut relevant_forbidden: Shape = (*self.volumes.get_avoidance(r0, layer_idx, avo_type, rests, !self.xy_overrides)).clone();
            relevant_forbidden = relevant_forbidden.offset_default(EPSILON).union_polygons();

            let generate_lines = |this: &Self, area: &Shape, roof: bool, generate_layer_idx: LayerIndex| -> OpenLinesSet {
                let upper_line_distance = this.support_supporting_branch_distance;
                let line_distance = cmax(if roof { this.support_roof_line_distance } else { this.support_tree_branch_distance }, upper_line_distance);
                tsu::generate_support_infill_lines(
                    area,
                    &this.config,
                    roof && !this.use_fake_roof,
                    generate_layer_idx,
                    line_distance,
                    this.cfp(),
                    roof && !this.use_fake_roof,
                    line_distance == upper_line_distance,
                )
            };

            let mut overhang_processing: Vec<(Shape, bool)> = Vec::new();
            let mut core_overhang = mesh.overhang_areas[lu + zdd].clone();
            if self.support_roof_layers != 0 && lu + 1 < self.support_roof_drawn.len() {
                core_overhang = core_overhang.difference(&self.support_roof_drawn[lu]);
                for roof_part in self.support_roof_drawn[lu + 1].difference(&self.support_roof_drawn[lu]).split_into_parts(true) {
                    overhang_processing.push((roof_part.into_shape(), true));
                }
            }

            let c = &self.config;
            let mut overhang_regular = tsu::safe_offset_inc(
                &core_overhang,
                self.support_outset,
                &relevant_forbidden,
                (c.min_radius as f64 * 1.75 + c.xy_min_distance as f64) as Coord,
                0,
                1,
                c.support_line_distance / 2,
                Some(&c.simplifier),
            );
            let mut remaining_overhang = core_overhang
                .offset_default(self.support_outset)
                .difference(&overhang_regular.offset_default((c.support_line_width as f64 * 0.5) as Coord))
                .intersection(&relevant_forbidden);

            let mut extra_total_offset_acc: Coord = 0;
            while extra_total_offset_acc + c.support_line_width / 8 < extra_outset {
                let offset_current_step = if extra_total_offset_acc + 2 * c.support_line_width > c.min_radius {
                    cmin(c.support_line_width / 8, extra_outset - extra_total_offset_acc)
                } else {
                    cmin(circle_length_to_half_linewidth_change, extra_outset - extra_total_offset_acc)
                };
                extra_total_offset_acc += offset_current_step;
                let col0 = self.volumes.get_collision(0, layer_idx, true);
                let overhang_offset = tsu::safe_offset_inc(
                    &overhang_regular,
                    (1.5 * extra_total_offset_acc as f64) as Coord,
                    &col0,
                    c.xy_min_distance + c.support_line_width,
                    0,
                    1,
                    c.support_line_distance / 2,
                    Some(&c.simplifier),
                );
                remaining_overhang = remaining_overhang
                    .difference(
                        &overhang_offset
                            .union_polygons_other(&self.support_roof_drawn[lu].offset_default((1.5 * extra_total_offset_acc as f64) as Coord)),
                    )
                    .union_polygons();
                let next_overhang = tsu::safe_offset_inc(
                    &remaining_overhang,
                    extra_total_offset_acc,
                    &col0,
                    c.xy_min_distance + c.support_line_width,
                    0,
                    1,
                    c.support_line_distance / 2,
                    Some(&c.simplifier),
                );
                overhang_regular = overhang_regular.union_polygons_other(&next_overhang.difference(&relevant_forbidden));
            }

            if self.xy_overrides {
                for remaining_overhang_part in remaining_overhang.split_into_parts(false) {
                    let remaining_overhang_part = remaining_overhang_part.into_shape();
                    if remaining_overhang_part.area() <= mm2_2int(self.minimum_support_area) as f64 {
                        continue;
                    }
                    let mut overhang_lines: Vec<LineInformation> = Vec::new();
                    let mut polylines =
                        self.ensure_maximum_distance_polyline(&generate_lines(self, &remaining_overhang_part, false, layer_idx), self.config.min_radius, 1, false);
                    if polylines.point_count() <= 3 {
                        polylines = self.ensure_maximum_distance_polyline(&tsu::to_polylines(&remaining_overhang_part), self.connect_length, 3, true);
                    }
                    for line in polylines.iter() {
                        overhang_lines.push(line.iter().map(|&p| (p, LineStatus::Invalid)).collect());
                    }
                    let mut lag_ctr: usize = 1;
                    while lag_ctr <= self.max_overhang_insert_lag && !overhang_lines.is_empty() && layer_idx - lag_ctr as LayerIndex >= 1 {
                        let below = layer_idx - lag_ctr as LayerIndex;
                        let relevant_forbidden_below = self.volumes.get_avoidance(r0, below, avo_type, rests, !self.xy_overrides);
                        if self.support_roof_layers != 0 {
                            let roof_below = &self.support_roof_drawn[below as usize];
                            overhang_lines = split_lines(overhang_lines, &mut |p| roof_below.inside(p.0, true)).1;
                        }
                        let split = split_lines(overhang_lines, &mut |p| relevant_forbidden_below.inside(p.0, true));
                        overhang_lines = split.0;
                        let fresh_valid_points = self.convert_lines_to_internal(&convert_internal_to_lines(&split.1), below);
                        self.add_lines_as_influence_areas(
                            arena,
                            &mut new_tips,
                            fresh_valid_points,
                            if self.force_tip_to_roof && lag_ctr <= self.support_roof_layers { self.support_roof_layers } else { 0 },
                            below,
                            false,
                            self.support_roof_layers,
                            false,
                        );
                        lag_ctr += 1;
                    }
                }
            }

            overhang_regular.remove_small_areas(self.minimum_support_area, false);
            for support_part in overhang_regular.split_into_parts(true) {
                overhang_processing.push((support_part.into_shape(), false));
            }

            for (overhang_outset, roof_allowed_for_this_part) in overhang_processing {
                let min_support_points = cmax(1, cmin(EPSILON, overhang_outset.length() / self.connect_length)) as usize;
                let mut only_lines = true;
                let distance = if !roof_allowed_for_this_part {
                    self.config.min_radius * 2
                } else if self.use_fake_roof {
                    self.support_supporting_branch_distance
                } else {
                    self.connect_length
                };
                let mut polylines = self.ensure_maximum_distance_polyline(
                    &generate_lines(self, &overhang_outset, roof_allowed_for_this_part, layer_idx + roof_allowed_for_this_part as LayerIndex),
                    distance,
                    1,
                    false,
                );
                if polylines.point_count() <= min_support_points {
                    only_lines = false;
                    let reduced_overhang_outset = overhang_outset.offset_default((-self.config.support_line_width as f64 / 2.2) as Coord);
                    if !reduced_overhang_outset.is_empty()
                        && overhang_outset
                            .difference(&reduced_overhang_outset.offset_default(cmax(self.config.support_line_width, self.connect_length)))
                            .area()
                            < 1.0
                    {
                        polylines =
                            self.ensure_maximum_distance_polyline(&tsu::to_polylines(&reduced_overhang_outset), self.connect_length, min_support_points, true);
                    } else {
                        polylines = self.ensure_maximum_distance_polyline(&tsu::to_polylines(&overhang_outset), self.connect_length, min_support_points, true);
                    }
                }
                if roof_allowed_for_this_part {
                    polylines = tsu::move_points_outside(&polylines, &relevant_forbidden, r0 + FUDGE_LENGTH / 2);
                }
                let mut overhang_lines = self.convert_lines_to_internal(&polylines, layer_idx);
                if overhang_lines.is_empty() {
                    let enlarged_overhang_outset =
                        overhang_outset.offset_join(r0 + FUDGE_LENGTH / 2, JoinType::Round).difference(&relevant_forbidden);
                    polylines =
                        self.ensure_maximum_distance_polyline(&tsu::to_polylines(&enlarged_overhang_outset), self.connect_length, min_support_points, true);
                    overhang_lines = self.convert_lines_to_internal(&polylines, layer_idx);
                    if !overhang_lines.is_empty() {
                        crate::log_debug!("Compensated for overhang area that had no valid tips. Now has a tip.");
                    } else {
                        crate::log_warn!("Overhang area has no valid tips! Was roof: {} On Layer: {}", roof_allowed_for_this_part, layer_idx);
                    }
                }
                let dont_move_for_layers = if self.support_roof_layers != 0 {
                    if self.force_tip_to_roof {
                        self.support_roof_layers
                    } else if roof_allowed_for_this_part {
                        0
                    } else {
                        self.support_roof_layers
                    }
                } else {
                    0
                };
                self.add_lines_as_influence_areas(
                    arena,
                    &mut new_tips,
                    overhang_lines,
                    if self.force_tip_to_roof { self.support_roof_layers } else { 0 },
                    layer_idx,
                    roof_allowed_for_this_part,
                    dont_move_for_layers,
                    only_lines,
                );
            }
        }

        let c = &self.config;
        for layer_idx in 0..self.support_roof_drawn.len() {
            if !self.use_fake_roof && c.support_wall_count > 0 && c.support_roof_wall_count == 0 {
                for roof_area in self.support_roof_drawn[layer_idx].union_polygons_other(&self.roof_tips_drawn[layer_idx]).split_into_parts(false) {
                    let roof_area = roof_area.into_shape();
                    if layer_idx < additional_support_areas.len()
                        && tsu::generate_support_infill_lines(
                            &roof_area,
                            c,
                            true,
                            layer_idx as LayerIndex,
                            self.support_roof_line_distance,
                            self.cfp(),
                            false,
                            false,
                        )
                        .is_empty()
                    {
                        additional_support_areas[layer_idx].push_all(&roof_area);
                    } else {
                        support.support_layers[layer_idx].support_roof.push_all(&roof_area);
                    }
                }
                additional_support_areas[layer_idx] = additional_support_areas[layer_idx].union_polygons();
                support.support_layers[layer_idx].support_roof = support.support_layers[layer_idx].support_roof.union_polygons();
            } else if self.use_fake_roof {
                placed_fake_roof_areas[layer_idx].push(FakeRoofArea {
                    area: self.support_roof_drawn[layer_idx].clone(),
                    line_distance: self.support_roof_line_distance,
                    fractional: false,
                });
                if c.z_distance_top % c.layer_height != 0 && layer_idx > 0 {
                    let all_roof_fractional = self.support_roof_drawn_fractional[layer_idx - 1].intersection(&self.support_roof_drawn[layer_idx - 1]);
                    placed_fake_roof_areas[layer_idx].push(FakeRoofArea {
                        area: all_roof_fractional,
                        line_distance: self.support_roof_line_distance,
                        fractional: true,
                    });
                }
            } else {
                if c.z_distance_top % c.layer_height != 0 && layer_idx > 0 {
                    let all_roof_below = self.support_roof_drawn[layer_idx - 1].union_polygons_other(&self.roof_tips_drawn[layer_idx - 1]);
                    let all_roof_fractional = self.support_roof_drawn_fractional[layer_idx - 1].intersection(&all_roof_below);
                    let sl = &mut support.support_layers[layer_idx];
                    sl.support_fractional_roof = sl.support_fractional_roof.union_polygons_other(&all_roof_fractional);
                    sl.support_roof.push_all(&all_roof_fractional);
                }
                let all_roof = self.support_roof_drawn[layer_idx].union_polygons_other(&self.roof_tips_drawn[layer_idx]);
                let sl = &mut support.support_layers[layer_idx];
                sl.support_roof = sl.support_roof.union_polygons_other(&all_roof);
            }
        }

        self.remove_useless_added_points(arena, &mut new_tips, support, additional_support_areas);

        for (layer_idx, tips_on_layer) in new_tips.into_iter().enumerate() {
            move_bounds[layer_idx].extend(tips_on_layer);
        }
    }
}
