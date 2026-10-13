//! Lightning infill: `infill/LightningGenerator`, `LightningLayer`, `LightningTreeNode` and
//! `LightningDistanceField`.
//!
//! The C++ trees are `shared_ptr` nodes with `weak_ptr` parents. Here every layer owns an arena of
//! nodes and the trees refer to nodes by index. Trees are deep-copied from one layer to the next, so
//! nodes are never shared between layers. Nodes the C++ code destroys simply become unreachable;
//! no weak reference to a destroyed node is ever dereferenced in the C++ code, so this is
//! equivalent.

use std::collections::HashMap;

use crate::geometry::point2ll::{cpp_hash, normal, shorter_then, turn90_ccw, v_size, v_size2};
use crate::geometry::{OpenLinesSet, OpenPolyline, Point2LL, Shape};
use crate::slice_data_storage::SliceMeshStorage;
use crate::utils::coord::Coord;
use crate::utils::linear_alg_2d;
use crate::utils::polygon_utils::{ClosestPointPolygon, LocToLineGrid, PolygonUtils, NO_PENALTY};
use crate::utils::polygons_point_index::PolygonsPointIndex;
use crate::utils::sparse_grid::{GridPoint, SparsePointGridInclusive, SquareGrid};

/// `locator_cell_size`
pub const LOCATOR_CELL_SIZE: Coord = 4000;

type NodeId = usize;
type SparseLightningTreeNodeGrid = SparsePointGridInclusive<NodeId>;

#[inline]
fn mul_div(p: Point2LL, mul: Coord, div: Coord) -> Point2LL {
    // `Point2LL * coord_t / coord_t` (exact integer arithmetic).
    Point2LL::new(p.x * mul / div, p.y * mul / div)
}

/// `LightningTreeNode`
#[derive(Clone, Debug)]
struct Node {
    is_root: bool,
    p: Point2LL,
    parent: Option<NodeId>,
    children: Vec<NodeId>,
    /// The last known grounding location, see `getLastGroundingLocation()`.
    last_grounding_location: Option<Point2LL>,
}

#[derive(Clone, Copy, Debug)]
struct RectilinearJunction {
    /// rectilinear distance along the tree from the last junction above to the junction below
    total_recti_dist: Coord,
    /// junction location below
    junction_loc: Point2LL,
}

/// `GroundingLocation`
struct GroundingLocation {
    /// not null if the grounding location is on a tree
    tree_node: Option<NodeId>,
    /// in case the grounding location is on the boundary
    boundary_location: Option<ClosestPointPolygon>,
}

/// A layer of the lightning fill. Contains the trees to be printed and propagated to the next layer below.
#[derive(Default)]
pub struct LightningLayer {
    nodes: Vec<Node>,
    tree_roots: Vec<NodeId>,
}

impl LightningLayer {
    // ----- LightningTreeNode -----

    fn create(&mut self, p: Point2LL, last_grounding_location: Option<Point2LL>) -> NodeId {
        self.nodes.push(Node { is_root: true, p, parent: None, children: Vec::new(), last_grounding_location });
        self.nodes.len() - 1
    }

    fn add_child_pt(&mut self, n: NodeId, child_loc: Point2LL) -> NodeId {
        let child = self.create(child_loc, None);
        self.add_child(n, child)
    }

    fn add_child(&mut self, n: NodeId, new_child: NodeId) -> NodeId {
        self.nodes[n].children.push(new_child);
        self.nodes[new_child].parent = Some(n);
        self.nodes[new_child].is_root = false;
        new_child
    }

    fn node_weighted_distance(&self, n: NodeId, unsupported_location: Point2LL, supporting_radius: Coord) -> Coord {
        const MIN_VALENCE_FOR_BOOST: usize = 0;
        const MAX_VALENCE_FOR_BOOST: usize = 4;
        const VALENCE_BOOST_MULTIPLIER: Coord = 4;

        let node = &self.nodes[n];
        let valence = (!node.is_root) as usize + node.children.len();
        let valence_boost = if MIN_VALENCE_FOR_BOOST < valence && valence < MAX_VALENCE_FOR_BOOST { VALENCE_BOOST_MULTIPLIER * supporting_radius } else { 0 };
        let dist_here = v_size(node.p - unsupported_location);
        dist_here - valence_boost
    }

    fn has_offspring(&self, n: NodeId, to_be_checked: NodeId) -> bool {
        if to_be_checked == n {
            return true;
        }
        self.nodes[n].children.iter().any(|&child| self.has_offspring(child, to_be_checked))
    }

    /// Depth-first, pre-order.
    fn visit_nodes(&self, n: NodeId, visitor: &mut dyn FnMut(NodeId)) {
        visitor(n);
        for &child in &self.nodes[n].children {
            self.visit_nodes(child, visitor);
        }
    }

    /// Copy the tree rooted at `n` in `src` into this layer.
    fn deep_copy_from(&mut self, src: &LightningLayer, n: NodeId) -> NodeId {
        let src_node = &src.nodes[n];
        let local_root = self.create(src_node.p, None);
        self.nodes[local_root].is_root = src_node.is_root;
        if src_node.is_root {
            self.nodes[local_root].last_grounding_location = Some(src_node.last_grounding_location.unwrap_or(src_node.p));
        }
        self.nodes[local_root].children.reserve(src_node.children.len());
        for &child_src in &src_node.children {
            let child = self.deep_copy_from(src, child_src);
            self.nodes[child].parent = Some(local_root);
            self.nodes[local_root].children.push(child);
        }
        local_root
    }

    fn reroot(&mut self, n: NodeId, new_parent: Option<NodeId>) {
        if !self.nodes[n].is_root {
            let old_parent = self.nodes[n].parent.expect("non-root node without parent");
            self.reroot(old_parent, Some(n));
            self.nodes[n].children.push(old_parent);
        }

        match new_parent {
            Some(new_parent) => {
                self.nodes[n].children.retain(|&c| c != new_parent);
                self.nodes[n].is_root = false;
                self.nodes[n].parent = Some(new_parent);
            }
            None => {
                self.nodes[n].is_root = true;
                self.nodes[n].parent = None;
            }
        }
    }

    fn closest_node(&self, n: NodeId, loc: Point2LL) -> NodeId {
        let mut result = n;
        let mut closest_dist2 = v_size2(self.nodes[n].p - loc);

        for &child in &self.nodes[n].children {
            let candidate_node = self.closest_node(child, loc);
            let child_dist2 = v_size2(self.nodes[candidate_node].p - loc);
            if child_dist2 < closest_dist2 {
                closest_dist2 = child_dist2;
                result = candidate_node;
            }
        }
        result
    }

    /// Reconnect trees from the layer above to the new outlines of the lower layer.
    fn realign(&mut self, n: NodeId, outlines: &Shape, outline_locator: &LocToLineGrid, rerooted_parts: &mut Vec<NodeId>) -> bool {
        if outlines.is_empty() {
            return false;
        }

        let p = self.nodes[n].p;
        if outlines.inside(p, true) {
            // Only keep children that have an unbroken connection to here, realign will put the rest in rerooted parts due to recursion:
            let mut coll = Point2LL::new(0, 0);
            let mut reground_me = false;
            let children = std::mem::take(&mut self.nodes[n].children);
            let mut kept = Vec::with_capacity(children.len());
            for child in children {
                let mut connect_branch = self.realign(child, outlines, outline_locator, rerooted_parts);
                if connect_branch
                    && PolygonUtils::line_segment_polygons_intersection(self.nodes[child].p, p, outlines, outline_locator, &mut coll, outline_locator.cell_size() * 2)
                {
                    self.nodes[child].last_grounding_location = None;
                    self.nodes[child].parent = None;
                    self.nodes[child].is_root = true;
                    rerooted_parts.push(child);

                    reground_me = true;
                    connect_branch = false;
                }
                if connect_branch {
                    kept.push(child);
                }
            }
            self.nodes[n].children = kept;
            if reground_me {
                self.nodes[n].last_grounding_location = None;
            }
            return true;
        }

        // 'Lift' any decendants out of this tree:
        let children = self.nodes[n].children.clone();
        for child in children {
            if self.realign(child, outlines, outline_locator, rerooted_parts) {
                self.nodes[child].last_grounding_location = Some(p);
                self.nodes[child].parent = None;
                self.nodes[child].is_root = true;
                rerooted_parts.push(child);
            }
        }
        self.nodes[n].children.clear();

        false
    }

    fn straighten(&mut self, n: NodeId, magnitude: Coord, max_remove_colinear_dist: Coord) {
        let p = self.nodes[n].p;
        self.straighten_rec(n, magnitude, p, 0, max_remove_colinear_dist * max_remove_colinear_dist);
    }

    fn straighten_rec(&mut self, n: NodeId, magnitude: Coord, junction_above: Point2LL, accumulated_dist: Coord, max_remove_colinear_dist2: Coord) -> RectilinearJunction {
        const JUNCTION_MAGNITUDE_FACTOR_NUMERATOR: Coord = 3;
        const JUNCTION_MAGNITUDE_FACTOR_DENOMINATOR: Coord = 4;

        let junction_magnitude = magnitude * JUNCTION_MAGNITUDE_FACTOR_NUMERATOR / JUNCTION_MAGNITUDE_FACTOR_DENOMINATOR;
        if self.nodes[n].children.len() == 1 {
            let child_p = self.nodes[n].children[0];
            let child_dist = v_size(self.nodes[n].p - self.nodes[child_p].p);
            let junction_below = self.straighten_rec(child_p, magnitude, junction_above, accumulated_dist + child_dist, max_remove_colinear_dist2);
            let total_dist_to_junction_below = junction_below.total_recti_dist;
            let a = junction_above;
            let b = junction_below.junction_loc;
            if a != b {
                // should always be true!
                let ab = b - a;
                let denominator = if 1 < total_dist_to_junction_below { total_dist_to_junction_below } else { 1 };
                let destination = a + mul_div(ab, accumulated_dist, denominator);
                let p = self.nodes[n].p;
                if shorter_then(destination - p, magnitude) {
                    self.nodes[n].p = destination;
                } else {
                    self.nodes[n].p = p + normal(destination - p, magnitude);
                }
            }
            {
                // remove nodes on linear segments
                const CLOSE_ENOUGH: Coord = 10;

                let child_p = self.nodes[n].children[0]; // recursive call to straighten might have removed the child
                if let Some(parent_node) = self.nodes[n].parent {
                    let parent_p = self.nodes[parent_node].p;
                    let child_loc = self.nodes[child_p].p;
                    if v_size2(child_loc - parent_p) < max_remove_colinear_dist2
                        && linear_alg_2d::get_dist2_from_line_segment(parent_p, self.nodes[n].p, child_loc) < CLOSE_ENOUGH
                    {
                        self.nodes[child_p].parent = self.nodes[n].parent;
                        for sibling in self.nodes[parent_node].children.iter_mut() {
                            // find this node among siblings
                            if *sibling == n {
                                *sibling = child_p; // replace this node by child
                                break;
                            }
                        }
                    }
                }
            }
            junction_below
        } else {
            const WEIGHT: Coord = 1000;
            let p = self.nodes[n].p;
            let mut junction_moving_dir = normal(junction_above - p, WEIGHT);
            let mut prevent_junction_moving = false;
            for i in 0..self.nodes[n].children.len() {
                let child_p = self.nodes[n].children[i];
                let child_dist = v_size(p - self.nodes[child_p].p);
                let below = self.straighten_rec(child_p, magnitude, p, child_dist, max_remove_colinear_dist2);

                junction_moving_dir = junction_moving_dir + normal(below.junction_loc - p, WEIGHT);
                if below.total_recti_dist < magnitude {
                    // prevent flipflopping in branches due to straightening and junction moving clashing
                    prevent_junction_moving = true;
                }
            }
            if junction_moving_dir != Point2LL::new(0, 0) && !self.nodes[n].children.is_empty() && !self.nodes[n].is_root && !prevent_junction_moving {
                let junction_moving_dir_len = v_size(junction_moving_dir);
                if junction_moving_dir_len > junction_magnitude {
                    junction_moving_dir = mul_div(junction_moving_dir, junction_magnitude, junction_moving_dir_len);
                }
                self.nodes[n].p = self.nodes[n].p + junction_moving_dir;
            }
            RectilinearJunction { total_recti_dist: accumulated_dist, junction_loc: self.nodes[n].p }
        }
    }

    /// Prune the tree from the extremeties (leaf-nodes) until the pruning distance is reached.
    fn prune(&mut self, n: NodeId, pruning_distance: Coord) -> Coord {
        if pruning_distance <= 0 {
            return 0;
        }

        let mut max_distance_pruned: Coord = 0;
        let mut i = 0;
        while i < self.nodes[n].children.len() {
            let child = self.nodes[n].children[i];
            let dist_pruned_child = self.prune(child, pruning_distance);
            if dist_pruned_child >= pruning_distance {
                // pruning is finished for child; dont modify further
                max_distance_pruned = if max_distance_pruned < dist_pruned_child { dist_pruned_child } else { max_distance_pruned };
                i += 1;
            } else {
                let a = self.nodes[n].p;
                let b = self.nodes[child].p;
                let ba = a - b;
                let ab_len = v_size(ba);
                if dist_pruned_child + ab_len <= pruning_distance {
                    // we're still in the process of pruning
                    let v = dist_pruned_child + ab_len;
                    max_distance_pruned = if max_distance_pruned < v { v } else { max_distance_pruned };
                    self.nodes[n].children.remove(i);
                } else {
                    // pruning stops in between this node and the child
                    let new_loc = b + normal(ba, pruning_distance - dist_pruned_child);
                    max_distance_pruned = if max_distance_pruned < pruning_distance { pruning_distance } else { max_distance_pruned };
                    self.nodes[child].p = new_loc;
                    i += 1;
                }
            }
        }
        max_distance_pruned
    }

    fn convert_to_polylines_node(&self, n: NodeId, output: &mut OpenLinesSet, line_width: Coord) {
        let mut result = OpenLinesSet::default();
        result.push(OpenPolyline::default());
        self.convert_to_polylines_rec(n, 0, &mut result);
        Self::remove_junction_overlap(&mut result, line_width);
        output.push_all_owned(result);
    }

    fn convert_to_polylines_rec(&self, n: NodeId, long_line_idx: usize, output: &mut OpenLinesSet) {
        let node = &self.nodes[n];
        if node.children.is_empty() {
            output[long_line_idx].push(node.p);
            return;
        }
        let first_child_idx = (cppstd::crand::rand() as usize) % node.children.len();
        self.convert_to_polylines_rec(node.children[first_child_idx], long_line_idx, output);
        output[long_line_idx].push(node.p);

        for idx_offset in 1..node.children.len() {
            let child_idx = (first_child_idx + idx_offset) % node.children.len();
            output.push(OpenPolyline::default());
            let child_line_idx = output.len() - 1;
            self.convert_to_polylines_rec(node.children[child_idx], child_line_idx, output);
            output[child_line_idx].push(node.p);
        }
    }

    fn remove_junction_overlap(result_lines: &mut OpenLinesSet, line_width: Coord) {
        let reduction = line_width / 2;
        let mut i = 0;
        while i < result_lines.len() {
            if result_lines[i].len() <= 1 {
                result_lines.lines.swap_remove(i);
                continue;
            }

            let polyline = &mut result_lines[i];
            let mut to_be_reduced = reduction;
            let mut a = polyline[polyline.len() - 1];
            let mut point_idx = polyline.len() as i64 - 2;
            while point_idx >= 0 {
                let b = polyline[point_idx as usize];
                let ab = b - a;
                let ab_len = v_size(ab);
                if ab_len >= to_be_reduced {
                    let last = polyline.len() - 1;
                    polyline[last] = a + mul_div(ab, to_be_reduced, ab_len);
                    break;
                } else {
                    to_be_reduced -= ab_len;
                    polyline.pop();
                }
                a = b;
                point_idx -= 1;
            }

            if result_lines[i].len() <= 1 {
                result_lines.lines.swap_remove(i);
            } else {
                i += 1;
            }
        }
    }

    // ----- LightningLayer -----

    fn get_weighted_distance(boundary_loc: Point2LL, unsupported_location: Point2LL) -> Coord {
        v_size(boundary_loc - unsupported_location)
    }

    fn grounding_p(&self, g: &GroundingLocation) -> Point2LL {
        match g.tree_node {
            Some(n) => self.nodes[n].p,
            None => g.boundary_location.as_ref().expect("grounding location").p(),
        }
    }

    fn fill_locator(&self, tree_node_locator: &mut SparseLightningTreeNodeGrid) {
        for &tree in &self.tree_roots {
            self.visit_nodes(tree, &mut |node| tree_node_locator.insert(self.nodes[node].p, node));
        }
    }

    fn generate_new_trees(
        &mut self,
        current_overhang: &Shape,
        current_outlines: &Shape,
        outlines_locator: &LocToLineGrid,
        supporting_radius: Coord,
        wall_supporting_radius: Coord,
    ) {
        let mut distance_field = LightningDistanceField::new(supporting_radius, current_outlines, current_overhang);

        let mut tree_node_locator = SparseLightningTreeNodeGrid::new(LOCATOR_CELL_SIZE, 0);
        self.fill_locator(&mut tree_node_locator);

        // Until no more points need to be added to support all:
        // Determine next point from tree/outline areas via distance-field
        while let Some(unsupported_location) = distance_field.try_get_next_point() {
            let grounding_loc = self.get_best_grounding_location(
                unsupported_location,
                current_outlines,
                outlines_locator,
                supporting_radius,
                wall_supporting_radius,
                &tree_node_locator,
                None,
            );

            let (new_child, new_parent) = self.attach(unsupported_location, &grounding_loc);
            tree_node_locator.insert(self.nodes[new_child].p, new_child);
            if let Some(new_parent) = new_parent {
                tree_node_locator.insert(self.nodes[new_parent].p, new_parent);
            }

            // update distance field
            distance_field.update(self.grounding_p(&grounding_loc), unsupported_location);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn get_best_grounding_location(
        &self,
        unsupported_location: Point2LL,
        current_outlines: &Shape,
        outline_locator: &LocToLineGrid,
        supporting_radius: Coord,
        wall_supporting_radius: Coord,
        tree_node_locator: &SparseLightningTreeNodeGrid,
        exclude_tree: Option<NodeId>,
    ) -> GroundingLocation {
        let cpp = PolygonUtils::find_closest(unsupported_location, current_outlines, NO_PENALTY);
        let node_location = cpp.p();
        let within_dist = v_size(node_location - unsupported_location);

        let mut dummy = PolygonsPointIndex::default();

        let mut sub_tree: Option<NodeId> = None;
        let mut current_dist = Self::get_weighted_distance(node_location, unsupported_location);
        if current_dist >= wall_supporting_radius {
            // Only reconnect tree roots to other trees if they are not already close to the outlines.
            let radius = if within_dist < current_dist { within_dist } else { current_dist };
            let candidate_trees = tree_node_locator.get_nearby_vals(unsupported_location, radius);
            for candidate_sub_tree in candidate_trees {
                if Some(candidate_sub_tree) != exclude_tree
                    && !(exclude_tree.is_some() && self.has_offspring(exclude_tree.unwrap(), candidate_sub_tree))
                    && !PolygonUtils::polygon_collides_with_line_segment_grid(unsupported_location, self.nodes[candidate_sub_tree].p, outline_locator, Some(&mut dummy))
                {
                    let candidate_dist = self.node_weighted_distance(candidate_sub_tree, unsupported_location, supporting_radius);
                    if candidate_dist < current_dist {
                        current_dist = candidate_dist;
                        sub_tree = Some(candidate_sub_tree);
                    }
                }
            }
        }

        match sub_tree {
            None => GroundingLocation { tree_node: None, boundary_location: Some(cpp) },
            Some(t) => GroundingLocation { tree_node: Some(t), boundary_location: None },
        }
    }

    /// Returns (new_child, new_root).
    fn attach(&mut self, unsupported_location: Point2LL, grounding_loc: &GroundingLocation) -> (NodeId, Option<NodeId>) {
        // Update trees & distance fields.
        if grounding_loc.boundary_location.is_some() {
            let gp = self.grounding_p(grounding_loc);
            let new_root = self.create(gp, Some(gp));
            let new_child = self.add_child_pt(new_root, unsupported_location);
            self.tree_roots.push(new_root);
            (new_child, Some(new_root))
        } else {
            let new_child = self.add_child_pt(grounding_loc.tree_node.unwrap(), unsupported_location);
            (new_child, None)
        }
    }

    fn reconnect_roots(
        &mut self,
        to_be_reconnected_tree_roots: &[NodeId],
        current_outlines: &Shape,
        outline_locator: &LocToLineGrid,
        supporting_radius: Coord,
        wall_supporting_radius: Coord,
    ) {
        const TREE_CONNECTING_IGNORE_OFFSET: Coord = 100;

        let mut tree_node_locator = SparseLightningTreeNodeGrid::new(LOCATOR_CELL_SIZE, 0);
        self.fill_locator(&mut tree_node_locator);

        let within_max_dist = outline_locator.cell_size() * 2;
        for &root_ptr in to_be_reconnected_tree_roots {
            let old_root_it = self.tree_roots.iter().position(|&r| r == root_ptr).expect("tree root to reconnect");

            if let Some(ground_loc) = self.nodes[root_ptr].last_grounding_location {
                if ground_loc != self.nodes[root_ptr].p {
                    let mut new_root_pt = Point2LL::new(0, 0);
                    if PolygonUtils::line_segment_polygons_intersection(
                        self.nodes[root_ptr].p,
                        ground_loc,
                        current_outlines,
                        outline_locator,
                        &mut new_root_pt,
                        within_max_dist,
                    ) {
                        let new_root = self.create(new_root_pt, Some(new_root_pt));
                        self.add_child(root_ptr, new_root);
                        self.reroot(new_root, None);

                        tree_node_locator.insert(self.nodes[new_root].p, new_root);
                        self.tree_roots[old_root_it] = new_root; // replace old root with new root
                        continue;
                    }
                }
            }

            // Ideally, the boundary size in which the valence rule is ignored would be configurable.
            let tree_connecting_ignore_width = wall_supporting_radius - TREE_CONNECTING_IGNORE_OFFSET;
            let ground = self.get_best_grounding_location(
                self.nodes[root_ptr].p,
                current_outlines,
                outline_locator,
                supporting_radius,
                tree_connecting_ignore_width,
                &tree_node_locator,
                Some(root_ptr),
            );
            if let Some(boundary_location) = &ground.boundary_location {
                if boundary_location.p() == self.nodes[root_ptr].p {
                    continue; // Already on the boundary.
                }

                let gp = self.grounding_p(&ground);
                let new_root = self.create(gp, Some(gp));
                let attach_ptr = self.closest_node(root_ptr, self.nodes[new_root].p);
                self.reroot(attach_ptr, None);

                self.add_child(new_root, attach_ptr);
                tree_node_locator.insert(self.nodes[new_root].p, new_root);

                self.tree_roots[old_root_it] = new_root; // replace old root with new root
            } else {
                let tree_node = ground.tree_node.expect("grounding tree node");
                let attach_ptr = self.closest_node(root_ptr, self.nodes[tree_node].p);
                self.reroot(attach_ptr, None);

                self.add_child(tree_node, attach_ptr);

                // remove old root
                self.tree_roots.swap_remove(old_root_it);
            }
        }
    }

    /// `convertToLines`
    pub fn convert_to_lines(&self, limit_to_outline: &Shape, line_width: Coord) -> OpenLinesSet {
        let mut result_lines = OpenLinesSet::default();
        if self.tree_roots.is_empty() {
            return result_lines;
        }

        for &tree in &self.tree_roots {
            self.convert_to_polylines_node(tree, &mut result_lines, line_width);
        }
        limit_to_outline.intersection_polylines(&result_lines, true, 10, true)
    }
}

/// `LightningDistanceField`: keeps track of which overhang locations still need support.
struct LightningDistanceField {
    grid: SquareGrid,
    supporting_radius: Coord,
    /// The `std::list<UnsupCell>` in sorted order; erased cells are marked dead.
    cells: Vec<(Point2LL, Coord)>,
    alive: Vec<bool>,
    head: usize,
    /// `unordered_map<GridPoint, list::iterator>`
    unsupported_points_grid: HashMap<GridPoint, usize>,
}

impl LightningDistanceField {
    /// The cell-size should be small compared to the radius, but not so small as to be inefficient.
    const RADIUS_PER_CELL_SIZE: Coord = 6;

    fn new(radius: Coord, current_outline: &Shape, current_overhang: &Shape) -> Self {
        let cell_size = radius / Self::RADIUS_PER_CELL_SIZE;
        let grid = SquareGrid::new(cell_size);

        let regular_dots = PolygonUtils::spread_dots_area(current_overhang, cell_size);
        let mut unsupported_points = Vec::with_capacity(regular_dots.len());
        for p in regular_dots {
            let cpp = PolygonUtils::find_closest(p, current_outline, NO_PENALTY);
            let dist_to_boundary = v_size(p - cpp.p());
            unsupported_points.push((p, dist_to_boundary));
        }
        const PRIME_FOR_HASH: u64 = 191;
        let unsupported_points = cppstd::list_sort_by(unsupported_points, |a, b| {
            if (b.1 - a.1).abs() > radius {
                a.1 < b.1
            } else {
                (cpp_hash(a.0) % PRIME_FOR_HASH) < (cpp_hash(b.0) % PRIME_FOR_HASH)
            }
        });
        let mut unsupported_points_grid = HashMap::new();
        for (i, cell) in unsupported_points.iter().enumerate() {
            unsupported_points_grid.entry(grid.to_grid_point(cell.0)).or_insert(i);
        }
        let n = unsupported_points.len();
        LightningDistanceField { grid, supporting_radius: radius, cells: unsupported_points, alive: vec![true; n], head: 0, unsupported_points_grid }
    }

    fn try_get_next_point(&mut self) -> Option<Point2LL> {
        while self.head < self.cells.len() && !self.alive[self.head] {
            self.head += 1;
        }
        if self.head < self.cells.len() {
            Some(self.cells[self.head].0)
        } else {
            None
        }
    }

    fn update(&mut self, to_node: Point2LL, added_leaf: Point2LL) {
        let supporting_radius = self.supporting_radius;
        let cells = &self.cells;
        let alive = &mut self.alive;
        let unsupported_points_grid = &mut self.unsupported_points_grid;
        let mut process_func = |grid_loc: GridPoint| -> bool {
            if let Some(&idx) = unsupported_points_grid.get(&grid_loc) {
                if shorter_then(cells[idx].0 - added_leaf, supporting_radius) {
                    alive[idx] = false;
                    unsupported_points_grid.remove(&grid_loc);
                }
            }
            true
        };
        let a = to_node;
        let b = added_leaf;
        let ab = b - a;
        let ab_t = turn90_ccw(ab);
        let extent = normal(ab_t, supporting_radius);
        let grid = self.grid;
        // NOTE: like the C++ code, the grid point `p` is used as a location for the inner line.
        grid.process_line_cells((a + extent, a - extent), |p: GridPoint| {
            grid.process_line_cells((p, p + ab), &mut process_func);
            true
        });
        grid.process_nearby(added_leaf, supporting_radius, &mut process_func);
    }
}

/// `LightningGenerator` ("Just like Nicola used to make!")
pub struct LightningGenerator {
    lightning_layers: Vec<LightningLayer>,
}

impl LightningGenerator {
    pub fn new(mesh: &SliceMeshStorage) -> Self {
        let infill_extruder = mesh.settings().get_extruder_train("infill_extruder_nr");
        // Note: There's not going to be a layer below the first one, so the 'initial layer height' doesn't have to be taken into account.
        let layer_thickness = infill_extruder.settings.get_coord("layer_height");

        let line_distance = infill_extruder.settings.get_coord("infill_line_distance");
        let line_width = infill_extruder.settings.get_coord("infill_line_width");
        let supporting_radius = (if line_distance < line_width { line_width } else { line_distance }) / 2;
        let tan_of = |key: &str| f64::tan(infill_extruder.settings.get_angle_radians(key).value());
        let wall_supporting_radius = (layer_thickness as f64 * tan_of("lightning_infill_overhang_angle")) as Coord;
        let prune_length = (layer_thickness as f64 * tan_of("lightning_infill_prune_angle")) as Coord;
        let straightening_max_distance = (layer_thickness as f64 * tan_of("lightning_infill_straightening_angle")) as Coord;

        let overhang_per_layer = Self::generate_initial_internal_overhangs(mesh, wall_supporting_radius);
        let mut generator = LightningGenerator { lightning_layers: Vec::new() };
        generator.generate_trees(mesh, &overhang_per_layer, supporting_radius, wall_supporting_radius, prune_length, straightening_max_distance);
        generator
    }

    /// Calculate the overhangs above the infill areas that need to be supported by infill.
    fn generate_initial_internal_overhangs(mesh: &SliceMeshStorage, wall_supporting_radius: Coord) -> Vec<Shape> {
        let mut overhang_per_layer = vec![Shape::default(); mesh.layers.len()];
        let infill_wall_line_count = mesh.settings().get_usize("infill_wall_line_count") as Coord;
        let infill_line_width = mesh.settings().get_coord("infill_line_width");
        let infill_wall_offset = -infill_wall_line_count * infill_line_width;

        let mut infill_area_above = Shape::default();
        // Iterate from top to bottom, to subtract the overhang areas above from the overhang areas on the layer below, to get only overhang in the top layer where it is overhanging.
        for layer_nr in (0..mesh.layers.len()).rev() {
            let current_layer = &mesh.layers[layer_nr];
            let mut infill_area_here = Shape::default();
            for part in &current_layer.parts {
                infill_area_here.push_all_owned(part.get_own_infill_area().offset_default(infill_wall_offset));
            }

            // Remove the part of the infill area that is already supported by the walls.
            let overhang = infill_area_here.offset_default(-wall_supporting_radius).difference(&infill_area_above);

            overhang_per_layer[layer_nr] = overhang;
            infill_area_above = infill_area_here;
        }
        overhang_per_layer
    }

    /// Get a tree of paths generated for a certain layer of the mesh.
    pub fn get_trees_for_layer(&self, layer_id: usize) -> &LightningLayer {
        &self.lightning_layers[layer_id]
    }

    /// Calculate the tree structure of all layers.
    fn generate_trees(
        &mut self,
        mesh: &SliceMeshStorage,
        overhang_per_layer: &[Shape],
        supporting_radius: Coord,
        wall_supporting_radius: Coord,
        prune_length: Coord,
        straightening_max_distance: Coord,
    ) {
        self.lightning_layers.resize_with(mesh.layers.len(), LightningLayer::default);
        let infill_wall_line_count = mesh.settings().get_usize("infill_wall_line_count") as Coord;
        let infill_line_width = mesh.settings().get_coord("infill_line_width");
        let infill_wall_offset = -infill_wall_line_count * infill_line_width;

        let mut infill_outlines: Vec<Shape> = vec![Shape::default(); mesh.layers.len()];

        // For-each layer from top to bottom:
        for layer_id in (0..mesh.layers.len()).rev() {
            for part in &mesh.layers[layer_id].parts {
                infill_outlines[layer_id].push_all_owned(part.get_own_infill_area().offset_default(infill_wall_offset));
            }
        }
        if mesh.layers.is_empty() {
            return;
        }

        // For various operations its beneficial to quickly locate nearby features on the polygon:
        let top_layer_id = mesh.layers.len() - 1;
        let mut outlines_locator_ptr = PolygonUtils::create_loc_to_line_grid(&infill_outlines[top_layer_id], LOCATOR_CELL_SIZE as i32);

        // For-each layer from top to bottom:
        for layer_id in (0..=top_layer_id).rev() {
            let current_outlines = &infill_outlines[layer_id];
            {
                let current_lightning_layer = &mut self.lightning_layers[layer_id];
                let outlines_locator = &*outlines_locator_ptr;

                // register all trees propagated from the previous layer as to-be-reconnected
                let to_be_reconnected_tree_roots = current_lightning_layer.tree_roots.clone();

                current_lightning_layer.generate_new_trees(&overhang_per_layer[layer_id], current_outlines, outlines_locator, supporting_radius, wall_supporting_radius);

                current_lightning_layer.reconnect_roots(&to_be_reconnected_tree_roots, current_outlines, outlines_locator, supporting_radius, wall_supporting_radius);
            }

            // Initialize trees for next lower layer from the current one.
            if layer_id == 0 {
                return;
            }
            let below_outlines = &infill_outlines[layer_id - 1];
            outlines_locator_ptr = PolygonUtils::create_loc_to_line_grid(below_outlines, LOCATOR_CELL_SIZE as i32);
            let below_outlines_locator = &*outlines_locator_ptr;

            let (lower, upper) = self.lightning_layers.split_at_mut(layer_id);
            let current_lightning_layer = &upper[0];
            let lower_layer = &mut lower[layer_id - 1];
            let mut lower_trees = std::mem::take(&mut lower_layer.tree_roots);
            for &tree in &current_lightning_layer.tree_roots {
                // propagateToNextLayer
                let tree_below = lower_layer.deep_copy_from(current_lightning_layer, tree);

                lower_layer.prune(tree_below, prune_length);
                lower_layer.straighten(tree_below, straightening_max_distance, LOCATOR_CELL_SIZE / 2);
                if lower_layer.realign(tree_below, below_outlines, below_outlines_locator, &mut lower_trees) {
                    lower_trees.push(tree_below);
                }
            }
            lower_layer.tree_roots = lower_trees;
        }
    }
}
