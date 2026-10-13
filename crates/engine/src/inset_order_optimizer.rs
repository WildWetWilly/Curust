//! `InsetOrderOptimizer.h/.cpp`: adds the walls of a part to a layer plan in an order that respects
//! the inset direction (inside-out / outside-in) and minimizes travel.

use crate::geometry::point2ll::{scale_f32, scale_i32, v_size, v_size2, Point2LL};
use crate::geometry::Shape;
use crate::layer_plan::LayerPlan;
use crate::path_order_optimizer::{PathOrderOptimizer, ZSeamConfig};
use crate::path_planning::GCodePathConfig;
use crate::settings::enums::{EZSeamType, InsetDirection};
use crate::settings::settings::Settings;
use crate::settings::types::LayerIndex;
use crate::utils::aabb::AABB;
use crate::utils::coord::Coord;
use crate::utils::extrusion_line::{ExtrusionJunction, ExtrusionLine, VariableWidthLines};
use crate::utils::linear_alg_2d as la;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use crate::texture_data_provider::TextureDataProvider;

/// The order requirements: pairs of indices into the list of walls, where the first must be printed
/// before the second (`std::unordered_multimap<const ExtrusionLine*, const ExtrusionLine*>`).
pub type OrderRequirements = Vec<(usize, usize)>;

pub struct InsetOrderOptimizer<'s> {
    settings: &'s Settings,
    extruder_nr: usize,
    inset_0_default_config: GCodePathConfig,
    inset_x_default_config: GCodePathConfig,
    inset_0_roofing_config: GCodePathConfig,
    inset_x_roofing_config: GCodePathConfig,
    inset_0_flooring_config: GCodePathConfig,
    inset_x_flooring_config: GCodePathConfig,
    inset_0_bridge_config: GCodePathConfig,
    inset_x_bridge_config: GCodePathConfig,
    retract_before_outer_wall: bool,
    wall_0_wipe_dist: Coord,
    wall_x_wipe_dist: Coord,
    wall_0_extruder_nr: usize,
    wall_x_extruder_nr: usize,
    z_seam_config: ZSeamConfig,
    paths: &'s [VariableWidthLines],
    layer_nr: LayerIndex,
    /// Center of the model (= all meshes) axis-aligned bounding-box.
    model_center_point: Point2LL,
    disallowed_areas_for_seams: Shape,
    scarf_seam: bool,
    smooth_speed: bool,
    overhang_areas: Shape,
    texture_data_provider: Option<Arc<TextureDataProvider>>,
}

/// The wall configurations of an `InsetOrderOptimizer` (outer/inner × default/roofing/flooring/bridge).
#[derive(Clone, Copy)]
pub struct WallConfigs {
    pub inset_0_default: GCodePathConfig,
    pub inset_x_default: GCodePathConfig,
    pub inset_0_roofing: GCodePathConfig,
    pub inset_x_roofing: GCodePathConfig,
    pub inset_0_flooring: GCodePathConfig,
    pub inset_x_flooring: GCodePathConfig,
    pub inset_0_bridge: GCodePathConfig,
    pub inset_x_bridge: GCodePathConfig,
}

impl WallConfigs {
    /// All eight configs the same.
    pub fn uniform(config: &GCodePathConfig) -> Self {
        WallConfigs {
            inset_0_default: *config,
            inset_x_default: *config,
            inset_0_roofing: *config,
            inset_x_roofing: *config,
            inset_0_flooring: *config,
            inset_x_flooring: *config,
            inset_0_bridge: *config,
            inset_x_bridge: *config,
        }
    }
}

impl<'s> InsetOrderOptimizer<'s> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        layer_nr: LayerIndex,
        settings: &'s Settings,
        extruder_nr: usize,
        configs: WallConfigs,
        retract_before_outer_wall: bool,
        wall_0_wipe_dist: Coord,
        wall_x_wipe_dist: Coord,
        wall_0_extruder_nr: usize,
        wall_x_extruder_nr: usize,
        z_seam_config: ZSeamConfig,
        paths: &'s [VariableWidthLines],
        model_center_point: Point2LL,
        disallowed_areas_for_seams: Shape,
        scarf_seam: bool,
        smooth_speed: bool,
        overhang_areas: Shape,
    ) -> Self {
        InsetOrderOptimizer {
            settings,
            extruder_nr,
            inset_0_default_config: configs.inset_0_default,
            inset_x_default_config: configs.inset_x_default,
            inset_0_roofing_config: configs.inset_0_roofing,
            inset_x_roofing_config: configs.inset_x_roofing,
            inset_0_flooring_config: configs.inset_0_flooring,
            inset_x_flooring_config: configs.inset_x_flooring,
            inset_0_bridge_config: configs.inset_0_bridge,
            inset_x_bridge_config: configs.inset_x_bridge,
            retract_before_outer_wall,
            wall_0_wipe_dist,
            wall_x_wipe_dist,
            wall_0_extruder_nr,
            wall_x_extruder_nr,
            z_seam_config,
            paths,
            layer_nr,
            model_center_point,
            disallowed_areas_for_seams,
            scarf_seam,
            smooth_speed,
            overhang_areas,
            texture_data_provider: None,
        }
    }

    /// The `texture_data_provider` constructor argument.
    pub fn with_texture_data_provider(mut self, texture_data_provider: Option<Arc<TextureDataProvider>>) -> Self {
        self.texture_data_provider = texture_data_provider;
        self
    }

    /// Adds the walls to the layer plan. Returns whether anything was added.
    pub fn add_to_layer(&mut self, gcode_layer: &mut LayerPlan) -> bool {
        // Settings & configs:
        let pack_by_inset = !self.settings.get_bool("optimize_wall_printing_order");
        let inset_direction: InsetDirection = self.settings.get_enum("inset_direction");
        let alternate_walls = self.settings.get_bool("material_alternate_walls");

        let outer_to_inner = inset_direction == InsetDirection::OUTSIDE_IN;
        let use_one_extruder = self.wall_0_extruder_nr == self.wall_x_extruder_nr;
        let current_extruder_is_wall_x = self.wall_x_extruder_nr == self.extruder_nr;

        let reverse = Self::should_reverse_path(use_one_extruder, current_extruder_is_wall_x, outer_to_inner);
        let use_shortest_for_inner_walls = outer_to_inner;
        let mut walls_to_be_added = self.get_walls_to_be_added(reverse, use_one_extruder);

        let order = if pack_by_inset { Self::get_inset_order(&walls_to_be_added, outer_to_inner) } else { Self::get_region_order(&walls_to_be_added, outer_to_inner) };

        let flow = 1.0;

        let mut added_something = false;

        let detect_loops = false;
        let group_outer_walls = self.settings.get_bool("group_outer_walls");

        // Insert the seam points first, since this modifies the walls.
        let mut force_starts: Vec<Option<usize>> = vec![None; walls_to_be_added.len()];
        let z_seam_on_vertex = self.settings.get_bool("z_seam_on_vertex");
        for (idx, line) in walls_to_be_added.iter_mut().enumerate() {
            if line.is_closed && !z_seam_on_vertex {
                // If the user indicated that we may deviate from the vertices for the seam, we can insert a seam point, if needed.
                force_starts[idx] = self.insert_seam_point(line, gcode_layer);
            }
        }

        // When we alternate walls, also alternate the direction at which the first wall starts in.
        // On even layers we start with normal direction, on odd layers with inverted direction.
        let mut order_optimizer: PathOrderOptimizer<usize> = PathOrderOptimizer::new(
            gcode_layer.get_last_planned_position_or_starting_position(),
            self.z_seam_config,
            detect_loops,
            None,
            reverse,
            &order,
            group_outer_walls,
            self.disallowed_areas_for_seams.clone(),
            use_shortest_for_inner_walls,
            self.overhang_areas.clone(),
        )
        .with_texture_data_provider(self.texture_data_provider.clone());

        for (idx, line) in walls_to_be_added.iter().enumerate() {
            let points: Vec<Point2LL> = line.junctions.iter().map(|j| j.p).collect();
            if line.is_closed {
                order_optimizer.add_polygon(idx, Arc::new(points), force_starts[idx], line.is_outer_wall());
            } else {
                order_optimizer.add_polyline(idx, Arc::new(points));
            }
        }

        order_optimizer.optimize(true);
        let ordered = std::mem::take(&mut order_optimizer.paths);
        drop(order_optimizer);

        for path in ordered.iter() {
            let line = &walls_to_be_added[path.vertices];
            if line.junctions.is_empty() {
                continue;
            }

            let is_outer_wall = line.inset_idx == 0; // or thin wall 'gap filler'
            let is_gap_filler = line.is_odd;
            let default_config = if is_outer_wall { self.inset_0_default_config } else { self.inset_x_default_config };
            let roofing_config = if is_outer_wall { self.inset_0_roofing_config } else { self.inset_x_roofing_config };
            let flooring_config = if is_outer_wall { self.inset_0_flooring_config } else { self.inset_x_flooring_config };
            let bridge_config = if is_outer_wall { self.inset_0_bridge_config } else { self.inset_x_bridge_config };
            let wipe_dist = if is_outer_wall && !is_gap_filler { self.wall_0_wipe_dist } else { self.wall_x_wipe_dist };
            let retract_before = if is_outer_wall { self.retract_before_outer_wall } else { false };
            let scarf_seam = self.scarf_seam && is_outer_wall;
            let smooth_speed = self.smooth_speed && is_outer_wall;

            let revert_inset = alternate_walls && (line.inset_idx % 2 != 0);
            let revert_layer = alternate_walls && (self.layer_nr % 2 != 0);
            let backwards = path.backwards != (revert_inset != revert_layer);
            let start_index = if backwards != path.backwards { line.junctions.len() - (path.start_vertex + 1) } else { path.start_vertex };
            let linked_path = !path.is_closed;

            gcode_layer.set_is_inside(true); // Going to print walls, which are always inside.
            gcode_layer.add_wall(
                line,
                start_index,
                self.settings,
                &default_config,
                &roofing_config,
                &flooring_config,
                &bridge_config,
                wipe_dist,
                flow,
                retract_before,
                path.is_closed,
                backwards,
                linked_path,
                scarf_seam,
                smooth_speed,
            );
            added_something = true;
        }
        added_something
    }

    /// Insert a seam point in the closed line if needed; returns the index of the start vertex.
    fn insert_seam_point(&self, closed_line: &mut ExtrusionLine, gcode_layer: &LayerPlan) -> Option<usize> {
        debug_assert!(closed_line.is_closed);
        debug_assert!(closed_line.junctions.len() >= 3);

        let mut request_point = match self.z_seam_config.type_ {
            EZSeamType::USER_SPECIFIED => self.z_seam_config.pos,
            EZSeamType::SHORTEST => gcode_layer.get_last_planned_position_or_starting_position(),
            _ => return None,
        };

        let n = closed_line.junctions.len();
        // Find the 'closest' point on the polygon to the request_point.
        let mut closest_point = Point2LL::default();
        let mut closest_junction_idx = 0usize;
        let mut closest_distance_sqd = Coord::MAX;
        let mut should_recalculate_closest = false;
        if self.z_seam_config.type_ == EZSeamType::USER_SPECIFIED {
            // For user-defined seams you usually don't _actually_ want the _closest_ point, per-se,
            // since you want the seam-line to be continuous in 3D space.
            let ray_origin = self.model_center_point;
            request_point = ray_origin + scale_i32(request_point - ray_origin, 10);

            for i in 0..n {
                let junction = closed_line.junctions[i];
                let next_junction = closed_line.junctions[(i + 1) % n];

                if let Some((t, _u)) = la::segment_segment_intersection(ray_origin, request_point, junction.p, next_junction.p) {
                    let intersection = ray_origin + scale_f32(request_point - ray_origin, t);
                    let distance_sqd = v_size2(request_point - intersection);
                    if distance_sqd < closest_distance_sqd {
                        closest_point = intersection;
                        closest_distance_sqd = distance_sqd;
                        closest_junction_idx = i;
                    }
                }
            }
        }
        if closest_distance_sqd >= Coord::MAX {
            // If it the method isn't 'user-defined', or the attempt to do user-defined above failed,
            // then just find the closest point on the polygon.
            for i in 0..n {
                let junction = closed_line.junctions[i];
                let next_junction = closed_line.junctions[(i + 1) % n];
                let distance_sqd = la::get_dist2_from_line_segment(junction.p, request_point, next_junction.p);
                if distance_sqd < closest_distance_sqd {
                    closest_distance_sqd = distance_sqd;
                    closest_junction_idx = i;
                }
            }
            should_recalculate_closest = true;
        }

        let start_pt = closed_line.junctions[closest_junction_idx];
        let end_pt = closed_line.junctions[(closest_junction_idx + 1) % n];
        if should_recalculate_closest {
            closest_point = la::get_closest_on_line_segment(request_point, start_pt.p, end_pt.p);
        }
        const SMALLEST_DIST_SQD: Coord = 25;
        if v_size2(closest_point - start_pt.p) <= SMALLEST_DIST_SQD {
            // If the closest point is very close to the start point, just use it instead.
            return Some(closest_junction_idx);
        }
        if v_size2(closest_point - end_pt.p) <= SMALLEST_DIST_SQD {
            // If the closest point is very close to the end point, just use it instead.
            return Some((closest_junction_idx + 1) % n);
        }

        let total_dist = v_size(end_pt.p - start_pt.p);
        let start_dist = v_size(closest_point - start_pt.p);
        let end_dist = v_size(closest_point - end_pt.p);
        let w = ((end_pt.w * end_dist) / total_dist) + ((start_pt.w * start_dist) / total_dist);

        closed_line.junctions.insert(closest_junction_idx + 1, ExtrusionJunction::new(closest_point, w, start_pt.perimeter_index as Coord));
        Some(closest_junction_idx + 1)
    }

    /// Get the order constraints of the walls, grouped per region.
    pub fn get_region_order(extrusion_lines: &[ExtrusionLine], outer_to_inner: bool) -> OrderRequirements {
        if extrusion_lines.is_empty() {
            return Vec::new();
        }

        // view on the extrusion lines, sorted by area
        let mut extrusion_lines_area: Vec<(usize, Coord)> = extrusion_lines
            .iter()
            .enumerate()
            .map(|(i, line)| {
                let poly = line.to_polygon();
                let mut aabb = AABB::default();
                aabb.include_points(&poly.points);
                (i, aabb.area())
            })
            .collect();
        cppstd::stable_sort_by(&mut extrusion_lines_area, |lhs, rhs| lhs.1 < rhs.1);
        let sorted_extrusion_lines: Vec<usize> = extrusion_lines_area.iter().map(|p| p.0).collect();

        // graph will contain the parent-child relationships between the extrusion lines
        // an edge is added for both the parent to child and child to parent relationship.
        // `equal_range` of an unordered_multimap yields the values in reverse insertion order with
        // libstdc++ and in insertion order with Microsoft's STL.
        let mut graph: Vec<Vec<usize>> = vec![Vec::new(); extrusion_lines.len()];
        let emplace = |graph: &mut Vec<Vec<usize>>, a: usize, b: usize| graph[a].push(b);
        // during the loop we maintain a list of invariant parents; these are the parents that we have found so far
        // (iterated in the order of their position in the input, see the determinism patch in NOTES.md)
        let mut invariant_outer_parents: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
        for &extrusion_line_idx in sorted_extrusion_lines.iter() {
            let extrusion_line = &extrusion_lines[extrusion_line_idx];
            // Create a polygon representing the inner area of the extrusion line; any
            // point inside this polygon is considered to the child of the extrusion line.
            let mut hole_polygons = Shape::default();
            if extrusion_line.is_closed {
                hole_polygons.push(extrusion_line.to_polygon());
            }

            if hole_polygons.is_empty() {
                invariant_outer_parents.insert(extrusion_line_idx);
                continue;
            }

            // go through all the invariant parents and see if they are inside the hole polygon
            // if they are, then that means we have found a child for this extrusion line
            let mut removed_parent_invariants: Vec<usize> = Vec::new();
            for &invariant_parent in invariant_outer_parents.iter() {
                if hole_polygons.inside(extrusion_lines[invariant_parent].junctions[0].p, false) {
                    // The root polygon is inside the location polygon. It is no longer a root in the graph we are building.
                    emplace(&mut graph, extrusion_line_idx, invariant_parent);
                    emplace(&mut graph, invariant_parent, extrusion_line_idx);
                    removed_parent_invariants.push(invariant_parent);
                }
            }
            for node in removed_parent_invariants {
                invariant_outer_parents.remove(&node);
            }

            // the current extrusion line is now an invariant parent
            invariant_outer_parents.insert(extrusion_line_idx);
        }

        let outer_walls: Vec<usize> = (0..extrusion_lines.len()).filter(|&i| extrusion_lines[i].is_outer_wall()).collect();

        let neighbours = |node: usize| -> Vec<usize> { crate::utils::sparse_grid::multimap_order(&graph[node]).copied().collect() };

        // find for each line the closest outer line, and store this in closest_outer_wall_line
        let mut closest_outer_wall_line: HashMap<usize, usize> = HashMap::new();
        let mut min_depth: HashMap<usize, u32> = HashMap::new();
        for &outer_wall in outer_walls.iter() {
            // actions::dfs_depth_state
            let mut visited: HashSet<usize> = HashSet::new();
            fn dfs_depth(
                node: usize,
                depth: u32,
                visited: &mut HashSet<usize>,
                neighbours: &dyn Fn(usize) -> Vec<usize>,
                handle: &mut dyn FnMut(usize, u32),
            ) {
                if visited.contains(&node) {
                    return;
                }
                visited.insert(node);
                handle(node, depth);
                for n in neighbours(node) {
                    dfs_depth(n, depth + 1, visited, neighbours, handle);
                }
            }
            let mut update_nodes = |current_line: usize, depth: u32| {
                if !min_depth.contains_key(&current_line) || depth < min_depth[&current_line] {
                    min_depth.insert(current_line, depth);
                    closest_outer_wall_line.insert(current_line, outer_wall);
                }
            };
            dfs_depth(outer_wall, 0, &mut visited, &neighbours, &mut update_nodes);
        }

        // for each of the outer walls, perform a dfs until we have found an extrusion line that is
        // _not_ closest to the current outer wall, then stop the dfs traversal for that branch.
        let mut order: OrderRequirements = Vec::new();
        for &outer_wall in outer_walls.iter() {
            // actions::dfs_parent_state
            let mut visited: HashSet<usize> = HashSet::new();
            fn dfs_parent(
                node: usize,
                parent: Option<usize>,
                visited: &mut HashSet<usize>,
                neighbours: &dyn Fn(usize) -> Vec<usize>,
                handle: &mut dyn FnMut(usize, Option<usize>),
            ) {
                if visited.contains(&node) {
                    return;
                }
                visited.insert(node);
                handle(node, parent);
                for n in neighbours(node) {
                    dfs_parent(n, Some(node), visited, neighbours, handle);
                }
            }
            let mut set_order_constraints = |current_line: usize, parent_line: Option<usize>| {
                // `closest_outer_wall_line[current_line]` default-inserts "nullptr" for unknown lines
                if closest_outer_wall_line.get(&current_line) == Some(&outer_wall) {
                    if let Some(parent_line) = parent_line {
                        // flip the key values if we want to print from inner to outer walls
                        if outer_to_inner {
                            order.push((parent_line, current_line));
                        } else {
                            order.push((current_line, parent_line));
                        }
                    }
                }
            };
            dfs_parent(outer_wall, None, &mut visited, &neighbours, &mut set_order_constraints);
        }

        order
    }

    /// Get the order constraints of the walls, grouped per inset.
    pub fn get_inset_order(input: &[ExtrusionLine], outer_to_inner: bool) -> OrderRequirements {
        let mut order: OrderRequirements = Vec::new();

        let mut walls_by_inset: Vec<Vec<usize>> = Vec::new();
        let mut fillers_by_inset: Vec<Vec<usize>> = Vec::new();

        for (idx, line) in input.iter().enumerate() {
            if line.is_odd {
                if line.inset_idx >= fillers_by_inset.len() {
                    fillers_by_inset.resize(line.inset_idx + 1, Vec::new());
                }
                fillers_by_inset[line.inset_idx].push(idx);
            } else {
                if line.inset_idx >= walls_by_inset.len() {
                    walls_by_inset.resize(line.inset_idx + 1, Vec::new());
                }
                walls_by_inset[line.inset_idx].push(idx);
            }
        }
        let mut inset_idx = 0;
        while inset_idx + 1 < walls_by_inset.len() {
            for &line in walls_by_inset[inset_idx].iter() {
                for &inner_line in walls_by_inset[inset_idx + 1].iter() {
                    let mut before = inner_line;
                    let mut after = line;
                    if outer_to_inner {
                        std::mem::swap(&mut before, &mut after);
                    }
                    order.push((before, after));
                }
            }
            inset_idx += 1;
        }
        for inset_idx in 1..fillers_by_inset.len() {
            for &line in fillers_by_inset[inset_idx].iter() {
                if inset_idx - 1 >= walls_by_inset.len() {
                    continue;
                }
                for &enclosing_wall in walls_by_inset[inset_idx - 1].iter() {
                    order.push((enclosing_wall, line));
                }
            }
        }

        order
    }

    fn should_reverse_path(use_one_extruder: bool, current_extruder_is_wall_x: bool, outer_to_inner: bool) -> bool {
        if use_one_extruder && current_extruder_is_wall_x {
            return !outer_to_inner;
        }
        current_extruder_is_wall_x
    }

    fn get_walls_to_be_added(&self, reverse: bool, use_one_extruder: bool) -> Vec<ExtrusionLine> {
        if self.paths.is_empty() {
            return Vec::new();
        }
        let selected: Vec<&VariableWidthLines> = if reverse {
            if use_one_extruder {
                self.paths.iter().rev().collect()
            } else {
                // rv::reverse | rv::drop_last(1)
                self.paths.iter().rev().take(self.paths.len() - 1).collect()
            }
        } else if use_one_extruder {
            self.paths.iter().collect()
        } else {
            self.paths.iter().take(1).collect()
        };
        selected.into_iter().flat_map(|lines| lines.iter()).filter(|line| !line.junctions.is_empty()).cloned().collect()
    }
}
