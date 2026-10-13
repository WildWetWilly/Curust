//! `arachne/STHalfEdge`, `STHalfEdgeNode`, `SkeletalTrapezoidationEdge`, `SkeletalTrapezoidationJoint`
//! and `SkeletalTrapezoidationGraph`: the half-edge graph of the skeletal trapezoidation.
//!
//! The C++ graph stores its edges and nodes in `std::list`s and links them with raw pointers.
//! Here both lists are [`IndexList`]s (same iteration order, stable indices) and the pointers
//! are indices; `NIL` is the null pointer.

use crate::geometry::point2ll::{point_less, shorter_then, v_size, Point2LL};
use crate::utils::coord::Coord;
use crate::utils::index_list::{IndexList, NIL};
use crate::utils::linear_alg_2d;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeType {
    /// From the Voronoi diagram.
    Normal = 0,
    /// Introduced to the Voronoi diagram in order to make the gMAT.
    ExtraVd = 1,
    /// Introduced to the Voronoi diagram in order to make the gMAT.
    TransitionEnd = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Central {
    Unknown = -1,
    No = 0,
    Yes = 1,
}

/// `SkeletalTrapezoidationEdge`. The weak pointers to transition / junction storage are indices
/// into storage owned by the `SkeletalTrapezoidation` (or `NIL`).
#[derive(Clone, Debug)]
pub struct EdgeData {
    pub type_: EdgeType,
    pub is_central: Central,
    pub transitions: usize,
    pub transition_ends: usize,
    pub extrusion_junctions: usize,
}

impl EdgeData {
    pub fn new(type_: EdgeType) -> Self {
        EdgeData { type_, is_central: Central::Unknown, transitions: NIL, transition_ends: NIL, extrusion_junctions: NIL }
    }

    #[inline]
    pub fn is_central(&self) -> bool {
        self.is_central == Central::Yes
    }

    #[inline]
    pub fn set_is_central(&mut self, b: bool) {
        self.is_central = if b { Central::Yes } else { Central::No };
    }

    #[inline]
    pub fn central_is_set(&self) -> bool {
        self.is_central != Central::Unknown
    }
}

/// `SkeletalTrapezoidationJoint`.
#[derive(Clone, Debug)]
pub struct Joint {
    pub distance_to_boundary: Coord,
    pub bead_count: Coord,
    /// The distance near the skeleton to leave free because this joint is in the middle of a
    /// transition, as a fraction of the inner bead width of the bead at the higher transition.
    pub transition_ratio: f64,
    /// Index into the beading propagation storage, or `NIL`.
    pub beading: usize,
}

impl Default for Joint {
    fn default() -> Self {
        Joint { distance_to_boundary: -1, bead_count: -1, transition_ratio: 0.0, beading: NIL }
    }
}

#[derive(Clone, Debug)]
pub struct STEdge {
    pub data: EdgeData,
    pub twin: usize,
    pub next: usize,
    pub prev: usize,
    pub from: usize,
    pub to: usize,
}

impl STEdge {
    pub fn new(data: EdgeData) -> Self {
        STEdge { data, twin: NIL, next: NIL, prev: NIL, from: NIL, to: NIL }
    }
}

#[derive(Clone, Debug)]
pub struct STNode {
    pub data: Joint,
    pub p: Point2LL,
    pub incident_edge: usize,
}

impl STNode {
    pub fn new(data: Joint, p: Point2LL) -> Self {
        STNode { data, p, incident_edge: NIL }
    }
}

#[derive(Default, Debug)]
pub struct Graph {
    pub edges: IndexList<STEdge>,
    pub nodes: IndexList<STNode>,
}

impl Graph {
    #[inline]
    pub fn from_dtb(&self, e: usize) -> Coord {
        self.nodes[self.edges[e].from].data.distance_to_boundary
    }

    #[inline]
    pub fn to_dtb(&self, e: usize) -> Coord {
        self.nodes[self.edges[e].to].data.distance_to_boundary
    }

    #[inline]
    pub fn from_p(&self, e: usize) -> Point2LL {
        self.nodes[self.edges[e].from].p
    }

    #[inline]
    pub fn to_p(&self, e: usize) -> Point2LL {
        self.nodes[self.edges[e].to].p
    }

    #[inline]
    pub fn is_central(&self, e: usize) -> bool {
        self.edges[e].data.is_central()
    }

    /// `edge->twin_->next_`.
    #[inline]
    pub fn twin_next(&self, e: usize) -> usize {
        self.edges[self.edges[e].twin].next
    }

    // ---- STHalfEdge ---------------------------------------------------------------------------

    /// Check (recursively) whether there is any upward edge from the distance_to_boundary of the from of the edge.
    pub fn can_go_up(&self, e: usize, strict: bool) -> bool {
        let (to_d, from_d) = (self.to_dtb(e), self.from_dtb(e));
        if to_d > from_d {
            return true;
        }
        if to_d < from_d || strict {
            return false;
        }

        // Edge is between equidistant verts; recurse!
        let twin = self.edges[e].twin;
        let mut outgoing = self.edges[e].next;
        while outgoing != twin {
            if self.can_go_up(outgoing, false) {
                return true;
            }
            let ot = self.edges[outgoing].twin;
            if ot == NIL {
                return false;
            }
            if self.edges[ot].next == NIL {
                return true; // This point is on the boundary?! Should never occur
            }
            outgoing = self.edges[ot].next;
        }
        false
    }

    /// Check whether the edge goes from a lower to a higher distance_to_boundary.
    pub fn is_upward(&self, e: usize, strict: bool) -> bool {
        let (to_d, from_d) = (self.to_dtb(e), self.from_dtb(e));
        if to_d > from_d {
            return true;
        }
        if to_d < from_d || strict {
            return false;
        }

        // Equidistant edge case:
        let forward_up_dist = self.dist_to_go_up(e);
        let backward_up_dist = self.dist_to_go_up(self.edges[e].twin);
        if let (Some(f), Some(b)) = (forward_up_dist, backward_up_dist) {
            return f < b;
        }
        if forward_up_dist.is_some() {
            return true;
        }
        if backward_up_dist.is_some() {
            return false;
        }
        point_less(self.to_p(e), self.from_p(e)) // Arbitrary ordering, which returns the opposite for the twin edge
    }

    /// Calculate the traversed distance until we meet an upward edge.
    pub fn dist_to_go_up(&self, e: usize) -> Option<Coord> {
        let (to_d, from_d) = (self.to_dtb(e), self.from_dtb(e));
        if to_d > from_d {
            return Some(0);
        }
        if to_d < from_d {
            return None;
        }

        // Edge is between equidistant verts; recurse!
        let mut ret: Option<Coord> = None;
        let twin = self.edges[e].twin;
        let mut outgoing = self.edges[e].next;
        while outgoing != twin {
            let dist_to_up = self.dist_to_go_up(outgoing);
            if let Some(d) = dist_to_up {
                ret = Some(match ret {
                    Some(r) => {
                        if d < r {
                            d
                        } else {
                            r
                        }
                    }
                    None => d,
                });
            }
            let ot = self.edges[outgoing].twin;
            if ot == NIL {
                return None;
            }
            if self.edges[ot].next == NIL {
                return Some(0); // This point is on the boundary?! Should never occur
            }
            outgoing = self.edges[ot].next;
        }
        ret.map(|r| r + v_size(self.to_p(e) - self.from_p(e)))
    }

    pub fn get_next_unconnected(&self, e: usize) -> usize {
        let mut result = e;
        while self.edges[result].next != NIL {
            result = self.edges[result].next;
            if result == e {
                return NIL;
            }
        }
        self.edges[result].twin
    }

    // ---- STHalfEdgeNode -----------------------------------------------------------------------

    pub fn node_is_multi_intersection(&self, n: usize) -> bool {
        let mut odd_path_count = 0;
        let incident = self.nodes[n].incident_edge;
        let mut outgoing = incident;
        loop {
            if outgoing == NIL {
                // This is a node on the outside
                return false;
            }
            if self.edges[outgoing].data.is_central() {
                odd_path_count += 1;
            }
            outgoing = self.twin_next(outgoing);
            if outgoing == incident {
                break;
            }
        }
        odd_path_count > 2
    }

    pub fn node_is_central(&self, n: usize) -> bool {
        let incident = self.nodes[n].incident_edge;
        let mut edge = incident;
        loop {
            if self.edges[edge].data.is_central() {
                return true;
            }
            let t = self.edges[edge].twin;
            if t == NIL {
                return false;
            }
            edge = self.edges[t].next;
            if edge == incident {
                break;
            }
        }
        false
    }

    /// Check whether this node has a locally maximal distance_to_boundary.
    pub fn node_is_local_maximum(&self, n: usize, strict: bool) -> bool {
        if self.nodes[n].data.distance_to_boundary == 0 {
            return false;
        }
        let incident = self.nodes[n].incident_edge;
        let mut edge = incident;
        loop {
            if self.can_go_up(edge, strict) {
                return false;
            }
            let t = self.edges[edge].twin;
            if t == NIL {
                return false;
            }
            if self.edges[t].next == NIL {
                // This point is on the boundary
                return false;
            }
            edge = self.edges[t].next;
            if edge == incident {
                break;
            }
        }
        true
    }

    // ---- SkeletalTrapezoidationGraph ----------------------------------------------------------

    /// If an edge is too small, collapse it and its twin and fix the surrounding edges to ensure a
    /// consistent graph.
    pub fn collapse_small_edges(&mut self, snap_dist: Coord) {
        let should_collapse = |g: &Graph, a: usize, b: usize| shorter_then(g.nodes[a].p - g.nodes[b].p, snap_dist);

        // `safelyRemoveEdge`
        fn safely_remove_edge(g: &mut Graph, to_be_removed: usize, current_edge_it: &mut usize, edge_it_is_updated: &mut bool) {
            if *current_edge_it != NIL && to_be_removed == *current_edge_it {
                *current_edge_it = g.edges.erase(*current_edge_it);
                *edge_it_is_updated = true;
            } else if g.edges.is_alive(to_be_removed) {
                g.edges.erase(to_be_removed);
            }
        }

        let mut edge_it = self.edges.first();
        while edge_it != NIL {
            if self.edges[edge_it].prev != NIL {
                edge_it = self.edges.next(edge_it);
                continue;
            }

            let quad_start = edge_it;
            let mut quad_end = quad_start;
            while self.edges[quad_end].next != NIL {
                quad_end = self.edges[quad_end].next;
            }
            let quad_mid = if self.edges[quad_start].next == quad_end { NIL } else { self.edges[quad_start].next };

            let mut edge_it_is_updated = false;
            if quad_mid != NIL && should_collapse(self, self.edges[quad_mid].from, self.edges[quad_mid].to) {
                let quad_mid_twin = self.edges[quad_mid].twin;
                if quad_mid_twin == NIL {
                    continue; // Prevent accessing unallocated memory.
                }
                let mid_from = self.edges[quad_mid].from;
                let mut count = 0;
                let mut edge_from_3 = quad_end;
                while edge_from_3 != NIL && edge_from_3 != quad_mid_twin {
                    self.edges[edge_from_3].from = mid_from;
                    let t = self.edges[edge_from_3].twin;
                    self.edges[t].to = mid_from;
                    count += 1;
                    if count > 1000 {
                        break;
                    }
                    edge_from_3 = self.twin_next(edge_from_3);
                }

                // o-o > collapse top
                if self.nodes[mid_from].incident_edge == quad_mid {
                    if self.edges[quad_mid_twin].next != NIL {
                        self.nodes[mid_from].incident_edge = self.edges[quad_mid_twin].next;
                    } else {
                        let p = self.edges[quad_mid].prev;
                        self.nodes[mid_from].incident_edge = self.edges[p].twin;
                    }
                }

                let mid_to = self.edges[quad_mid].to;
                if self.nodes.is_alive(mid_to) {
                    self.nodes.erase(mid_to);
                }

                let (qp, qn) = (self.edges[quad_mid].prev, self.edges[quad_mid].next);
                self.edges[qp].next = qn;
                self.edges[qn].prev = qp;
                let (tp, tn) = (self.edges[quad_mid_twin].prev, self.edges[quad_mid_twin].next);
                self.edges[tn].prev = tp;
                self.edges[tp].next = tn;

                safely_remove_edge(self, quad_mid_twin, &mut edge_it, &mut edge_it_is_updated);
                safely_remove_edge(self, quad_mid, &mut edge_it, &mut edge_it_is_updated);
            }

            //  o-o
            //  | | > collapse sides
            //  o o
            if should_collapse(self, self.edges[quad_start].from, self.edges[quad_end].to)
                && should_collapse(self, self.edges[quad_start].to, self.edges[quad_end].from)
            {
                // Collapse start and end edges and remove whole cell
                let qs_twin = self.edges[quad_start].twin;
                let qe_twin = self.edges[quad_end].twin;
                let qe_to = self.edges[quad_end].to;
                self.edges[qs_twin].to = qe_to;
                self.nodes[qe_to].incident_edge = qe_twin;
                let qe_from = self.edges[quad_end].from;
                if self.nodes[qe_from].incident_edge == quad_end {
                    if self.edges[qe_twin].next != NIL {
                        self.nodes[qe_from].incident_edge = self.edges[qe_twin].next;
                    } else {
                        let p = self.edges[quad_end].prev;
                        self.nodes[qe_from].incident_edge = self.edges[p].twin;
                    }
                }
                let qs_from = self.edges[quad_start].from;
                if self.nodes.is_alive(qs_from) {
                    self.nodes.erase(qs_from);
                }

                self.edges[qs_twin].twin = qe_twin;
                self.edges[qe_twin].twin = qs_twin;
                safely_remove_edge(self, quad_start, &mut edge_it, &mut edge_it_is_updated);
                safely_remove_edge(self, quad_end, &mut edge_it, &mut edge_it_is_updated);
            }
            // If only one side had collapsable length then the cell on the other side of that edge has to collapse
            // if we would collapse that one edge then that would change the quad_start and/or quad_end of neighboring cells
            // this is to do with the constraint that !prev == !twin.next

            if !edge_it_is_updated {
                edge_it = self.edges.next(edge_it);
            }
        }
    }

    pub fn make_rib(&mut self, prev_edge: &mut usize, start_source_point: Point2LL, end_source_point: Point2LL) {
        let pe = *prev_edge;
        let pe_to = self.edges[pe].to;
        let p = linear_alg_2d::get_closest_on_line(self.nodes[pe_to].p, start_source_point, end_source_point);
        let dist = v_size(self.nodes[pe_to].p - p);
        self.nodes[pe_to].data.distance_to_boundary = dist;

        let node = self.nodes.push_front(STNode::new(Joint::default(), p));
        self.nodes[node].data.distance_to_boundary = 0;

        let forth_edge = self.edges.push_front(STEdge::new(EdgeData::new(EdgeType::ExtraVd)));
        let back_edge = self.edges.push_front(STEdge::new(EdgeData::new(EdgeType::ExtraVd)));

        self.edges[pe].next = forth_edge;
        self.edges[forth_edge].prev = pe;
        self.edges[forth_edge].from = pe_to;
        self.edges[forth_edge].to = node;
        self.edges[forth_edge].twin = back_edge;
        self.edges[back_edge].twin = forth_edge;
        self.edges[back_edge].from = node;
        self.edges[back_edge].to = pe_to;
        self.nodes[node].incident_edge = back_edge;

        *prev_edge = back_edge;
    }

    /// Returns the first and last edge of the edges replacing `edge` pointing to the same node.
    pub fn insert_rib(&mut self, edge: usize, mid_node: usize) -> (usize, usize) {
        let edge_before = self.edges[edge].prev;
        let edge_after = self.edges[edge].next;
        let node_before = self.edges[edge].from;
        let node_after = self.edges[edge].to;

        let p = self.nodes[mid_node].p;

        let source_segment = self.get_source(edge);
        let px = linear_alg_2d::get_closest_on_line_segment(p, source_segment.0, source_segment.1);
        let dist = v_size(p - px);
        self.nodes[mid_node].data.distance_to_boundary = dist;
        self.nodes[mid_node].data.transition_ratio = 0.0; // Both transition end should have rest = 0, because at the ends a whole number of beads fits without rest

        let source_node = self.nodes.push_back(STNode::new(Joint::default(), px));
        self.nodes[source_node].data.distance_to_boundary = 0;

        let first = edge;
        let second = self.edges.push_back(STEdge::new(EdgeData::new(EdgeType::Normal)));
        let outward_edge = self.edges.push_back(STEdge::new(EdgeData::new(EdgeType::TransitionEnd)));
        let inward_edge = self.edges.push_back(STEdge::new(EdgeData::new(EdgeType::TransitionEnd)));

        if edge_before != NIL {
            self.edges[edge_before].next = first;
        }
        self.edges[first].next = outward_edge;
        self.edges[outward_edge].next = NIL;
        self.edges[inward_edge].next = second;
        self.edges[second].next = edge_after;

        if edge_after != NIL {
            self.edges[edge_after].prev = second;
        }
        self.edges[second].prev = inward_edge;
        self.edges[inward_edge].prev = NIL;
        self.edges[outward_edge].prev = first;
        self.edges[first].prev = edge_before;

        self.edges[first].to = mid_node;
        self.edges[outward_edge].to = source_node;
        self.edges[inward_edge].to = mid_node;
        self.edges[second].to = node_after;

        self.edges[first].from = node_before;
        self.edges[outward_edge].from = mid_node;
        self.edges[inward_edge].from = source_node;
        self.edges[second].from = mid_node;

        self.nodes[node_before].incident_edge = first;
        self.nodes[mid_node].incident_edge = outward_edge;
        self.nodes[source_node].incident_edge = inward_edge;
        if edge_after != NIL {
            self.nodes[node_after].incident_edge = edge_after;
        }

        self.edges[first].data.set_is_central(true);
        self.edges[outward_edge].data.set_is_central(false); // TODO verify this is always the case.
        self.edges[inward_edge].data.set_is_central(false);
        self.edges[second].data.set_is_central(true);

        self.edges[outward_edge].twin = inward_edge;
        self.edges[inward_edge].twin = outward_edge;

        self.edges[first].twin = NIL; // we don't know these yet!
        self.edges[second].twin = NIL;

        (first, second)
    }

    /// Insert a node into the graph and connect it to the input polygon using ribs.
    /// Returns the last edge which replaced `edge`, which points to the same `to` node.
    pub fn insert_node(&mut self, edge: usize, mid: Point2LL, mide_node_bead_count: Coord) -> usize {
        let mut last_edge_replacing_input = edge;

        let mid_node = self.nodes.push_back(STNode::new(Joint::default(), mid));

        let twin = self.edges[last_edge_replacing_input].twin;
        self.edges[last_edge_replacing_input].twin = NIL;
        self.edges[twin].twin = NIL;
        let left_pair = self.insert_rib(last_edge_replacing_input, mid_node);
        let right_pair = self.insert_rib(twin, mid_node);
        let first_edge_replacing_input = left_pair.0;
        last_edge_replacing_input = left_pair.1;
        let first_edge_replacing_twin = right_pair.0;
        let last_edge_replacing_twin = right_pair.1;

        self.edges[first_edge_replacing_input].twin = last_edge_replacing_twin;
        self.edges[last_edge_replacing_twin].twin = first_edge_replacing_input;
        self.edges[last_edge_replacing_input].twin = first_edge_replacing_twin;
        self.edges[first_edge_replacing_twin].twin = last_edge_replacing_input;

        self.nodes[mid_node].data.bead_count = mide_node_bead_count;

        last_edge_replacing_input
    }

    fn get_source(&self, edge: usize) -> (Point2LL, Point2LL) {
        let mut from_edge = edge;
        while self.edges[from_edge].prev != NIL {
            from_edge = self.edges[from_edge].prev;
        }
        let mut to_edge = edge;
        while self.edges[to_edge].next != NIL {
            to_edge = self.edges[to_edge].next;
        }
        (self.from_p(from_edge), self.to_p(to_edge))
    }
}
