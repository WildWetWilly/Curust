//! `PathOrderOptimizer.h`, `path_ordering.h/.cpp` and `settings/ZSeamConfig.h`: optimizing the order in
//! which things are printed by minimizing the distance traveled between different items to be
//! printed, and choosing a starting point and direction for each item.
//!
//! The C++ template parameter `Path` (a pointer to the original data) is represented by a generic
//! copyable identifier `P`. The vertex data of each path is supplied when adding it.

use crate::geometry::point2ll::{v_size, v_size2, Point2LL};
use crate::geometry::Shape;
use crate::path_planning::line_polygons_crossings::{CombPath, LinePolygonsCrossings};
use crate::settings::enums::{EZSeamCornerPrefType, EZSeamType};
use crate::texture_data_provider::TextureDataProvider;
use crate::utils::aabb::AABB;
use crate::utils::coord::Coord;
use crate::utils::polygon_utils::{LocToLineGrid, PolygonUtils};
use crate::utils::scoring::{
    BestElementFinder, CornerScoringCriterion, CriteriaPass, DistanceScoringCriterion, DistanceType, ExclusionAreaScoringCriterion, ScoringCriterion,
    TextureScoringCriterion, WeighedCriterion,
};
use crate::utils::sparse_grid::SparsePointGridInclusive;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::Hash;
use std::sync::Arc;

/// `ZSeamConfig`: helper class that encapsulates various criteria that define the location of the
/// z-seam.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZSeamConfig {
    /// Strategy to place the seam (user-specified, shortest distance, sharpest corner, etc.).
    pub type_: EZSeamType,
    /// When using a user-specified position for the seam, this is the position that the user specified.
    pub pos: Point2LL,
    /// Corner preference type, if using the sharpest corner strategy.
    pub corner_pref: EZSeamCornerPrefType,
    /// Prevent 'smoothed out' corners (corners that are spread over multiple, very close together vertices),
    /// by simplifying the polygon that the corners are detected on by this ammount.
    pub simplify_curvature: Coord,
}

impl Default for ZSeamConfig {
    fn default() -> Self {
        ZSeamConfig::new(EZSeamType::SHORTEST, Point2LL::new(0, 0), EZSeamCornerPrefType::Z_SEAM_CORNER_PREF_INNER, 0)
    }
}

impl ZSeamConfig {
    pub fn new(type_: EZSeamType, pos: Point2LL, corner_pref: EZSeamCornerPrefType, simplify_curvature: Coord) -> Self {
        ZSeamConfig { type_, pos, corner_pref, simplify_curvature }
    }

    pub fn with_type(type_: EZSeamType) -> Self {
        ZSeamConfig { type_, ..Default::default() }
    }
}

/// `PathOrdering<PathType>`: a path which has been optimised, the output of the ordering.
#[derive(Clone, Debug)]
pub struct PathOrdering<P> {
    /// The original path (identifier).
    pub vertices: P,
    /// Vertex data, converted into a list of points so that the orderer knows how to deal with this data.
    pub converted: Arc<Vec<Point2LL>>,
    /// Which vertex along the path to start printing with.
    pub start_vertex: usize,
    /// Whether the path should be closed at the ends or not.
    pub is_closed: bool,
    /// Whether the path should be traversed in backwards direction.
    pub backwards: bool,
    /// Force the start point of the path to be at a specific location.
    pub force_start_index: Option<usize>,
    /// The start point calculation strategy to be used for this path
    pub seam_config: ZSeamConfig,
    /// Indicates whether this path is an outer (or inner) wall
    pub is_outer_wall: bool,
}

impl<P> PathOrdering<P> {
    pub fn new(vertices: P, converted: Arc<Vec<Point2LL>>, is_closed: bool, start_vertex: usize, backwards: bool) -> Self {
        PathOrdering {
            vertices,
            converted,
            start_vertex,
            is_closed,
            backwards,
            force_start_index: None,
            seam_config: ZSeamConfig::default(),
            is_outer_wall: false,
        }
    }

    /// The position where printing this path ends.
    fn end_position(&self) -> Point2LL {
        if self.is_closed {
            self.converted[self.start_vertex] // We end where we started.
        } else {
            // Pick the other end from where we started.
            if self.start_vertex == 0 {
                *self.converted.last().unwrap()
            } else {
                self.converted[0]
            }
        }
    }
}

/// `PathOrderOptimizer<Path>`
pub struct PathOrderOptimizer<'a, P: Copy + Eq + Hash> {
    /// Areas defined here are not allowed to have the start the prints
    pub disallowed_area_for_seams: Shape,
    /// After optimizing, this contains the paths that need to be printed in the correct order.
    pub paths: Vec<PathOrdering<P>>,
    /// Maps the path implementation to its orderable path index
    vertices_to_paths: HashMap<P, usize>,
    /// The location where the nozzle is assumed to start from before printing these parts.
    pub start_point: Point2LL,
    /// Seam settings.
    pub seam_config: ZSeamConfig,

    /// Bucket grid to store the locations of the combing boundary.
    combing_grid: Option<Box<LocToLineGrid>>,
    /// Boundary to avoid when making travel moves.
    combing_boundary: Option<&'a Shape>,
    /// Whether to check polylines to see if they are closed, before optimizing.
    detect_loops: bool,
    /// Whether to reverse the ordering completely.
    reverse_direction: bool,
    /// Whether to print all outer walls in a group, one after another.
    group_outer_walls: bool,
    /// Order requirements on the paths. For each pair the first needs to be printed before the second.
    order_requirements: &'a [(P, P)],
    /// If true, we will compute the seam position of inner walls using a "shortest" seam configs.
    use_shortest_for_inner_walls: bool,
    /// Contains the overhang areas, where we would prefer not to place the start locations of walls
    overhang_areas: Shape,
    /// Painted texture data, used to place the seams according to the "seam" paint feature.
    texture_data_provider: Option<Arc<TextureDataProvider>>,
}

/// If `detect_loops` is enabled, endpoints of polylines that are closer than this distance together
/// will be considered to be coincident, closing that polyline into a polygon.
const COINCIDENT_POINT_DISTANCE: Coord = 10;

impl<'a, P: Copy + Eq + Hash> PathOrderOptimizer<'a, P> {
    /// Construct a new optimizer. This doesn't actually optimize the order yet.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        start_point: Point2LL,
        seam_config: ZSeamConfig,
        detect_loops: bool,
        combing_boundary: Option<&'a Shape>,
        reverse_direction: bool,
        order_requirements: &'a [(P, P)],
        group_outer_walls: bool,
        disallowed_areas_for_seams: Shape,
        use_shortest_for_inner_walls: bool,
        overhang_areas: Shape,
    ) -> Self {
        PathOrderOptimizer {
            disallowed_area_for_seams: disallowed_areas_for_seams,
            paths: Vec::new(),
            vertices_to_paths: HashMap::new(),
            start_point,
            seam_config,
            combing_grid: None,
            combing_boundary: combing_boundary.filter(|b| !b.is_empty()),
            detect_loops,
            reverse_direction,
            group_outer_walls,
            order_requirements,
            use_shortest_for_inner_walls,
            overhang_areas,
            texture_data_provider: None,
        }
    }

    /// The `texture_data_provider` constructor argument.
    pub fn with_texture_data_provider(mut self, texture_data_provider: Option<Arc<TextureDataProvider>>) -> Self {
        self.texture_data_provider = texture_data_provider;
        self
    }

    /// Shorthand for the common constructor arguments (no requirements, no seam exclusions).
    pub fn simple(start_point: Point2LL, seam_config: ZSeamConfig, detect_loops: bool, combing_boundary: Option<&'a Shape>, reverse_direction: bool) -> Self {
        Self::new(start_point, seam_config, detect_loops, combing_boundary, reverse_direction, &[], false, Shape::default(), false, Shape::default())
    }

    /// Add a new polygon to be optimized.
    pub fn add_polygon(&mut self, polygon: P, vertices: Arc<Vec<Point2LL>>, force_start_index: Option<usize>, is_outer_wall: bool) {
        let is_closed = true;
        let mut p = PathOrdering::new(polygon, vertices, is_closed, 0, false);
        p.force_start_index = force_start_index;
        p.is_outer_wall = is_outer_wall;
        self.paths.push(p);
    }

    /// Add a new polyline to be optimized.
    pub fn add_polyline(&mut self, polyline: P, vertices: Arc<Vec<Point2LL>>) {
        let is_closed = false;
        self.paths.push(PathOrdering::new(polyline, vertices, is_closed, 0, false));
    }

    /// Perform the calculations to optimize the order of the parts.
    pub fn optimize(&mut self, precompute_start: bool) {
        if self.paths.is_empty() {
            return;
        }

        // Get the vertex data and store it in the paths.
        for (i, path) in self.paths.iter().enumerate() {
            self.vertices_to_paths.entry(path.vertices).or_insert(i);
        }

        // If necessary, check polylines to see if they are actually polygons.
        if self.detect_loops {
            for path in self.paths.iter_mut() {
                if !path.is_closed {
                    // If we want to detect chains, first check if some of the polylines are secretly polygons.
                    path.is_closed = Self::is_looping_polyline(path); // If it is, we'll set the seam position correctly later.
                }
            }
        }

        // Set actual used start point calculation strategy for each path
        for path in self.paths.iter_mut() {
            if self.use_shortest_for_inner_walls && !path.is_outer_wall {
                path.seam_config = ZSeamConfig::with_type(EZSeamType::SHORTEST);
                path.force_start_index = None;
            } else {
                path.seam_config = self.seam_config;
            }
        }

        // Add all vertices to a bucket grid so that we can find nearby endpoints quickly.
        let snap_radius: Coord = 10; // 0.01mm grid cells. Chaining only needs to consider polylines which are next to each other.
        let mut line_bucket_grid: SparsePointGridInclusive<usize> = SparsePointGridInclusive::new(snap_radius, 0);
        for (i, path) in self.paths.iter().enumerate() {
            if path.converted.is_empty() {
                continue;
            }
            if path.is_closed {
                for point in path.converted.iter() {
                    line_bucket_grid.insert(*point, i); // Store by index so that we can also mark them down in the `picked` vector.
                }
            } else {
                // For polylines, only insert the endpoints. Those are the only places we can start from so the only relevant vertices to be near to.
                line_bucket_grid.insert(path.converted[0], i);
                line_bucket_grid.insert(*path.converted.last().unwrap(), i);
            }
        }

        // For some Z seam types the start position can be pre-computed.
        // This is faster since we don't need to re-compute the start position at each step then.
        if precompute_start {
            for i in 0..self.paths.len() {
                let t = self.paths[i].seam_config.type_;
                if t == EZSeamType::RANDOM || t == EZSeamType::USER_SPECIFIED || t == EZSeamType::SHARPEST_CORNER {
                    if !self.paths[i].is_closed || self.paths[i].converted.is_empty() {
                        continue; // Can't pre-compute the seam for open polylines since they're at the endpoint nearest to the current position.
                    }
                    let pos = self.paths[i].seam_config.pos;
                    self.paths[i].start_vertex = self.find_start_location(i, pos);
                }
            }
        }

        let optimized_order = if self.order_requirements.is_empty() {
            self.get_optimized_order(&line_bucket_grid, snap_radius)
        } else {
            self.get_optimizer_order_with_constraints()
        };

        if self.reverse_direction && self.order_requirements.is_empty() {
            self.paths = Self::reverse_order_paths(optimized_order); // Reverse-insert the optimized order, to invert the ordering.
        } else {
            self.paths = optimized_order;
        }

        self.combing_grid = None;
    }

    fn get_optimized_order(&mut self, line_bucket_grid: &SparsePointGridInclusive<usize>, snap_radius: Coord) -> Vec<PathOrdering<P>> {
        let mut optimized_order: Vec<PathOrdering<P>> = Vec::with_capacity(self.paths.len());

        let mut current_position = self.start_point;

        let mut picked = vec![false; self.paths.len()]; // Fixed size boolean flag for whether each path is already in the optimized vector.

        while optimized_order.len() < self.paths.len() {
            // Use bucket grid to find paths within snap_radius
            let nearby_candidates = line_bucket_grid.get_nearby_vals(current_position, snap_radius);

            let mut available_candidates: Vec<usize> = Vec::with_capacity(nearby_candidates.len());
            for candidate in nearby_candidates {
                if !picked[candidate] {
                    available_candidates.push(candidate);
                }
            }

            if available_candidates.is_empty() {
                // We need to broaden our search through all candidates
                for (i, p) in picked.iter().enumerate() {
                    if !p {
                        available_candidates.push(i);
                    }
                }
            }

            let best_path = self.find_closest_path(current_position, &available_candidates).expect("no candidate path");
            optimized_order.push(self.paths[best_path].clone());
            picked[best_path] = true;

            if !self.paths[best_path].converted.is_empty() {
                // If all paths were empty, the best path is still empty. We don't upate the current position then.
                current_position = self.paths[best_path].end_position();
            }
        }

        optimized_order
    }

    fn get_optimizer_order_with_constraints(&mut self) -> Vec<PathOrdering<P>> {
        let mut optimized_order: Vec<PathOrdering<P>> = Vec::new();
        let n = self.paths.len();

        // Map the path identifiers to node ids (the index of the first path with that identifier).
        let node_of = |vertices_to_paths: &HashMap<P, usize>, p: &P| -> usize { *vertices_to_paths.get(p).expect("order requirement on unknown path") };
        let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (u, v) in self.order_requirements.iter() {
            adjacency[node_of(&self.vertices_to_paths, u)].push(node_of(&self.vertices_to_paths, v));
        }

        // initialize the roots set with all possible nodes
        let mut roots: BTreeSet<usize> = BTreeSet::new();
        let mut leaves: BTreeSet<usize> = BTreeSet::new();
        let mut num_incoming_edges: Vec<usize> = vec![0; n];
        for path in &self.paths {
            let node = self.vertices_to_paths[&path.vertices];
            roots.insert(node);
            leaves.insert(node);
        }

        // remove all edges from roots with an incoming edge
        // result is a set of nodes that have no incoming edges; these are by definition the roots
        for (u, v) in self.order_requirements.iter() {
            let un = node_of(&self.vertices_to_paths, u);
            let vn = node_of(&self.vertices_to_paths, v);
            roots.remove(&vn);
            leaves.remove(&un);
            num_incoming_edges[vn] += 1;
        }

        // We used a shared visited set between runs of dfs. This is for the case when we reverse the ordering tree.
        // In this case two roots can share the same children nodes, but we don't want to print them twice.
        let mut visited: HashSet<usize> = HashSet::new();
        let mut current_position = self.start_point;

        let mut state = ConstraintState { adjacency: &adjacency, num_incoming_edges: &mut num_incoming_edges, optimized_order: &mut optimized_order, current_position: &mut current_position };

        if self.group_outer_walls {
            if self.reverse_direction {
                // When the order is reverse the leaves of the order requirement are the outer walls
                let mut outer_walls = leaves.clone();

                // We don't want to add the outerwalls until after we finish all inner walls. Adding them to visited stops dfs from visiting these nodes.
                visited.extend(outer_walls.iter().copied());

                // Add all inner walls
                while !roots.is_empty() {
                    let root = self.find_closest_path_vertices(*state.current_position, &roots);
                    roots.remove(&root);
                    self.dfs(root, &mut visited, &mut state);
                }

                // Add all outer walls
                while !outer_walls.is_empty() {
                    let wall = self.find_closest_path_vertices(*state.current_position, &outer_walls);
                    outer_walls.remove(&wall);
                    self.handle_node(wall, &mut state);
                }
            } else {
                // The roots are the outer walls when reverse is false.
                let mut outer_walls = roots.clone();

                // Add all outer walls
                let mut root_neighbours: BTreeSet<usize> = BTreeSet::new();
                while !outer_walls.is_empty() {
                    let outer_wall = self.find_closest_path_vertices(*state.current_position, &outer_walls);
                    outer_walls.remove(&outer_wall);

                    self.handle_node(outer_wall, &mut state);
                    visited.insert(outer_wall);
                    for path in self.get_neighbours(outer_wall, &state) {
                        root_neighbours.insert(path);
                    }
                }

                // Add all inner walls
                while !root_neighbours.is_empty() {
                    let root_neighbour = self.find_closest_path_vertices(*state.current_position, &root_neighbours);
                    root_neighbours.remove(&root_neighbour);
                    self.dfs(root_neighbour, &mut visited, &mut state);
                }
            }
        } else {
            while !roots.is_empty() {
                let root = self.find_closest_path_vertices(*state.current_position, &roots);
                roots.remove(&root);

                self.dfs(root, &mut visited, &mut state);
            }
        }

        optimized_order
    }

    /// `actions::dfs` with the custom neighbour ordering of the optimizer.
    fn dfs(&mut self, current_node: usize, visited: &mut HashSet<usize>, state: &mut ConstraintState<P>) {
        if visited.contains(&current_node) {
            return;
        }
        visited.insert(current_node);

        self.handle_node(current_node, state);

        for neighbour in self.get_neighbours(current_node, state) {
            self.dfs(neighbour, visited, state);
        }
    }

    fn handle_node(&self, current_node: usize, state: &mut ConstraintState<P>) {
        let path = &self.paths[current_node];
        *state.current_position = path.end_position();

        // Add to optimized order
        state.optimized_order.push(path.clone());

        // update incoming edges of neighbours since this path is handled
        for &neighbour in &state.adjacency[current_node] {
            state.num_incoming_edges[neighbour] -= 1;
        }
    }

    fn get_neighbours(&mut self, current_node: usize, state: &ConstraintState<P>) -> Vec<usize> {
        let mut order: Vec<usize> = Vec::new(); // Output order to traverse neighbors

        let mut candidates: BTreeSet<usize> = BTreeSet::new();
        for &neighbour in &state.adjacency[current_node] {
            // we only want to visit nodes that have no incoming edges, this is for the situation where we
            // are printing paths from inner to outer.
            if state.num_incoming_edges[neighbour] == 0 {
                candidates.insert(neighbour);
            }
        }

        let mut local_current_position = *state.current_position;
        while !candidates.is_empty() {
            let best_candidate = self.find_closest_path_vertices(local_current_position, &candidates);

            candidates.remove(&best_candidate);
            order.push(best_candidate);

            // update local_current_position
            local_current_position = self.paths[best_candidate].end_position();
        }

        order
    }

    fn reverse_order_paths(paths_order_paths: Vec<PathOrdering<P>>) -> Vec<PathOrdering<P>> {
        let mut reversed: Vec<PathOrdering<P>> = Vec::with_capacity(paths_order_paths.len());
        for path in paths_order_paths.into_iter().rev() {
            let mut p = path;
            p.backwards = !p.backwards;
            if !p.is_closed {
                p.start_vertex = p.converted.len() - 1 - p.start_vertex;
            }
            reversed.push(p);
        }
        reversed
    }

    /// `findClosestPathVertices`. The candidates are visited in order of their index in `paths`
    /// (see the determinism note in NOTES.md).
    fn find_closest_path_vertices(&mut self, start_position: Point2LL, candidate_paths: &BTreeSet<usize>) -> usize {
        let candidates: Vec<usize> = candidate_paths.iter().copied().collect();
        let best_candidate = self.find_closest_path(start_position, &candidates).expect("no candidate path");
        self.vertices_to_paths[&self.paths[best_candidate].vertices]
    }

    fn find_closest_path(&mut self, start_position: Point2LL, candidate_paths: &[usize]) -> Option<usize> {
        let mut best_distance2 = Coord::MAX;
        let mut best_candidate: Option<usize> = None;

        for &i in candidate_paths {
            if self.paths[i].converted.is_empty() {
                // No vertices in the path. Can't find the start position then or really plan it in. Put that at the end.
                if best_distance2 == Coord::MAX {
                    best_candidate = Some(i);
                }
                continue;
            }

            let t = self.paths[i].seam_config.type_;
            let precompute_start = t == EZSeamType::RANDOM || t == EZSeamType::USER_SPECIFIED || t == EZSeamType::SHARPEST_CORNER;
            if !self.paths[i].is_closed || !precompute_start {
                // Find the start location unless we've already precomputed it.
                let sv = self.find_start_location(i, start_position);
                self.paths[i].start_vertex = sv;
                if !self.paths[i].is_closed {
                    // Open polylines start at vertex 0 or vertex N-1. Indicate that they should be reversed if they start at N-1.
                    self.paths[i].backwards = sv > 0;
                }
            }
            let candidate_position = self.paths[i].converted[self.paths[i].start_vertex];
            let mut distance2 = Self::get_direct_distance(start_position, candidate_position);
            if distance2 < best_distance2 && self.combing_boundary.is_some() {
                // If direct distance is longer than best combing distance, the combing distance can never be better, so only compute combing if necessary.
                distance2 = self.get_combing_distance(start_position, candidate_position);
            }
            if distance2 < best_distance2 {
                // Closer than the best candidate so far.
                best_candidate = Some(i);
                best_distance2 = distance2;
            }
        }

        best_candidate
    }

    /// Analyze the positions in a path and determine the next optimal position based on a proximity criterion.
    fn path_if_zseam_is_in_disallowed_area(&self, mut best_pos: usize, path: &PathOrdering<P>, mut number_of_paths_analysed: usize) -> usize {
        let path_size = path.converted.len();
        if path_size > number_of_paths_analysed {
            if !self.disallowed_area_for_seams.is_empty() {
                let current_candidate = path.converted[best_pos];
                if self.disallowed_area_for_seams.inside(current_candidate, true) {
                    let next_best_position = if path_size > best_pos + 1 { best_pos + 1 } else { 0 };
                    number_of_paths_analysed += 1;
                    best_pos = self.path_if_zseam_is_in_disallowed_area(next_best_position, path, number_of_paths_analysed);
                }
            }
        } else {
            crate::log_warn!("No start path found for support z seam distance");
            // We can also calculate the best point to start at this point.
            // This usually happens when the distance of support seam from model is bigger than the whole support wall points.
        }
        best_pos
    }

    /// Find the vertex which will be the starting point of printing a polygon or polyline.
    fn find_start_location(&mut self, path_idx: usize, target_pos: Point2LL) -> usize {
        let converted = self.paths[path_idx].converted.clone();
        if !self.paths[path_idx].is_closed {
            // For polylines, the seam settings are not applicable. Simply choose the position closest to target_pos then.
            let back = *converted.last().unwrap();
            let front = converted[0];
            let back_distance =
                if self.combing_boundary.is_none() { Self::get_direct_distance(back, target_pos) } else { self.get_combing_distance(back, target_pos) };
            if back_distance < Self::get_direct_distance(front, target_pos) || (self.combing_boundary.is_some() && back_distance < self.get_combing_distance(front, target_pos)) {
                // Lazy or: Only compute combing distance if direct distance is closer.
                return converted.len() - 1; // Back end is closer.
            }
            return 0; // Front end is closer.
        }

        // Rest of the function only deals with (closed) polygons. We need to be able to find the seam location of those polygons.
        let points: &[Point2LL] = &converted;
        let path = &self.paths[path_idx];

        // ########## Step 1: define the main criteria to be applied and their weights
        let mut best_candidate_finder = BestElementFinder::new();
        let mut main_criteria_pass = CriteriaPass { criteria: Vec::new(), outsider_delta_threshold: 0.05 };

        let texture_data_provider = self.texture_data_provider.clone();
        if let Some(provider) = texture_data_provider.as_deref() {
            main_criteria_pass
                .criteria
                .push(WeighedCriterion { criterion: ScoringCriterion::Texture(TextureScoringCriterion::new(points, provider, "seam")), weight: 5.0 });
        }

        let mut main_criterion: Option<ScoringCriterion> = None;

        if let Some(force) = path.force_start_index {
            // Handles EZSeamType::USER_SPECIFIED with "seam_on_vertex" disabled
            // Use a much smaller distance divider because we want points around the forced points to be filtered out very easily
            let distance_divider = 1.0;
            main_criterion = Some(ScoringCriterion::Distance(DistanceScoringCriterion::new(points, points[force], DistanceType::Euclidian, distance_divider)));
        } else if path.seam_config.type_ == EZSeamType::SHORTEST || path.seam_config.type_ == EZSeamType::USER_SPECIFIED {
            main_criterion = Some(ScoringCriterion::Distance(DistanceScoringCriterion::with_defaults(points, target_pos)));
        } else if path.seam_config.type_ == EZSeamType::SHARPEST_CORNER && path.seam_config.corner_pref != EZSeamCornerPrefType::PLUGIN {
            main_criterion = Some(ScoringCriterion::Corner(CornerScoringCriterion::new(points, path.seam_config.corner_pref)));
        } else if path.seam_config.type_ == EZSeamType::RANDOM {
            main_criterion = Some(ScoringCriterion::Random);
        }

        if let Some(c) = main_criterion {
            main_criteria_pass.criteria.push(WeighedCriterion { criterion: c, weight: 1.0 });
        } else {
            crate::log_warn!("Missing main criterion calculator");
        }

        // Second criterion with higher weight to avoid overhanging areas
        if !self.overhang_areas.is_empty() {
            main_criteria_pass
                .criteria
                .push(WeighedCriterion { criterion: ScoringCriterion::ExclusionArea(ExclusionAreaScoringCriterion::new(points, &self.overhang_areas)), weight: 2.0 });
        }

        best_candidate_finder.append_criteria_pass(main_criteria_pass);

        // ########## Step 2: add fallback passes for criteria with very similar scores (e.g. corner on a cylinder)
        if path.seam_config.type_ == EZSeamType::SHARPEST_CORNER {
            // NOTE: in C++ the fallback criteria keep a reference to the bounding box, which goes out of
            // scope before the criteria are evaluated; the (still intact) value is used here.
            let path_bounding_box = AABB::from_points(points);

            {
                // First fallback strategy is to take points on the back-most position
                let fallback_criterion = ScoringCriterion::Distance(DistanceScoringCriterion::new(points, path_bounding_box.max, DistanceType::YOnly, 20.0));
                let outsider_delta_threshold = 0.01;
                best_candidate_finder.append_single_criterion_pass(fallback_criterion, outsider_delta_threshold);
            }

            {
                // Second fallback strategy, in case we still have multiple points that are aligned on Y (e.g. cube), take the right-most point
                let fallback_criterion = ScoringCriterion::Distance(DistanceScoringCriterion::new(points, path_bounding_box.max, DistanceType::XOnly, 20.0));
                best_candidate_finder.append_single_criterion_pass(fallback_criterion, 0.0);
            }
        }

        // ########## Step 3: apply the criteria to find the vertex with the best global score
        let mut best_i = best_candidate_finder.find_best_element(points.len());

        if !self.disallowed_area_for_seams.is_empty() {
            best_i = Some(self.path_if_zseam_is_in_disallowed_area(best_i.unwrap_or(0), path, 0));
        }
        best_i.unwrap_or(0)
    }

    /// Calculate the direct Euclidean distance to move from one point to another.
    fn get_direct_distance(a: Point2LL, b: Point2LL) -> Coord {
        v_size2(a - b)
    }

    /// Calculate the distance that one would have to travel to move from A to B while avoiding collisions with the combing boundary.
    fn get_combing_distance(&mut self, a: Point2LL, b: Point2LL) -> Coord {
        let combing_boundary = self.combing_boundary.unwrap();
        if !PolygonUtils::polygons_collide_with_line_segment(combing_boundary, a, b) {
            return Self::get_direct_distance(a, b); // No collision with any line. Just compute the direct distance then.
        }
        if self.paths.len() > 100 {
            // If we have many paths to optimize the order for, this combing calculation can become very expensive.
            // Instead, penalize travels that hit the combing boundary with a static factor.
            return Self::get_direct_distance(a, b) * 5;
        }

        if self.combing_grid.is_none() {
            let grid_size: Coord = 2000; // 2mm grid cells. Smaller will use more memory, but reduce chance of unnecessary collision checks.
            self.combing_grid = Some(PolygonUtils::create_loc_to_line_grid(combing_boundary, grid_size as i32));
        }

        let mut comb_path = CombPath::default(); // Output variable.
        let rounding_error: Coord = -25;
        let tiny_travel_threshold: Coord = 0;
        let fail_on_unavoidable_obstacles = false;
        LinePolygonsCrossings::comb(
            combing_boundary,
            self.combing_grid.as_ref().unwrap(),
            a,
            b,
            &mut comb_path,
            rounding_error,
            tiny_travel_threshold,
            fail_on_unavoidable_obstacles,
        );

        let mut sum: Coord = 0;
        let mut last_point = a;
        for point in comb_path.iter() {
            sum += v_size(*point - last_point);
            last_point = *point;
        }
        sum * sum // Squared distance, for fair comparison with direct distance.
    }

    fn is_looping_polyline(path: &PathOrdering<P>) -> bool {
        if path.converted.is_empty() {
            return false;
        }
        v_size2(*path.converted.last().unwrap() - path.converted[0]) < COINCIDENT_POINT_DISTANCE * COINCIDENT_POINT_DISTANCE
    }
}

struct ConstraintState<'s, P> {
    adjacency: &'s Vec<Vec<usize>>,
    num_incoming_edges: &'s mut Vec<usize>,
    optimized_order: &'s mut Vec<PathOrdering<P>>,
    current_position: &'s mut Point2LL,
}
