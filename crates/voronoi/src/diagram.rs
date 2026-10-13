//! Port of `boost/polygon/voronoi_diagram.hpp` (`voronoi_diagram<double>`).
//!
//! Pointers between cells, vertices and edges are replaced by indices into the three
//! containers; `NIL` stands for a null pointer.

use std::cell::Cell as StdCell;

use crate::ctypes::{ulp_cmp, UlpResult};
use crate::structures::{belongs, CircleEvent, GeometryCategory, SiteEvent, SourceCategory, NIL};

const BITS_SHIFT: usize = 0x5;
const BITS_MASK: usize = 0x1F;
const BIT_IS_LINEAR: usize = 0x1;
const BIT_IS_PRIMARY: usize = 0x2;

#[derive(Clone, Debug)]
pub struct VoronoiCell {
    source_index: usize,
    incident_edge: usize,
    color: StdCell<usize>,
}

impl VoronoiCell {
    fn new(source_index: usize, source_category: SourceCategory) -> Self {
        VoronoiCell { source_index, incident_edge: NIL, color: StdCell::new(source_category as usize) }
    }

    pub fn contains_point(&self) -> bool {
        belongs(self.source_category(), GeometryCategory::Point)
    }

    pub fn contains_segment(&self) -> bool {
        belongs(self.source_category(), GeometryCategory::Segment)
    }

    pub fn source_index(&self) -> usize {
        self.source_index
    }

    pub fn source_category(&self) -> SourceCategory {
        SourceCategory::from_bits(self.color.get() & BITS_MASK)
    }

    /// Degenerate cells don't have any incident edges.
    pub fn is_degenerate(&self) -> bool {
        self.incident_edge == NIL
    }

    /// Index of an incident edge, or `NIL`.
    pub fn incident_edge(&self) -> usize {
        self.incident_edge
    }

    pub fn color(&self) -> usize {
        self.color.get() >> BITS_SHIFT
    }

    pub fn set_color(&self, color: usize) {
        let c = self.color.get() & BITS_MASK;
        self.color.set(c | (color << BITS_SHIFT));
    }
}

#[derive(Clone, Debug)]
pub struct VoronoiVertex {
    x: f64,
    y: f64,
    incident_edge: usize,
    color: StdCell<usize>,
}

impl VoronoiVertex {
    fn new(x: f64, y: f64) -> Self {
        VoronoiVertex { x, y, incident_edge: NIL, color: StdCell::new(0) }
    }

    pub fn x(&self) -> f64 {
        self.x
    }

    pub fn y(&self) -> f64 {
        self.y
    }

    pub fn is_degenerate(&self) -> bool {
        self.incident_edge == NIL
    }

    pub fn incident_edge(&self) -> usize {
        self.incident_edge
    }

    pub fn color(&self) -> usize {
        self.color.get() >> BITS_SHIFT
    }

    pub fn set_color(&self, color: usize) {
        let c = self.color.get() & BITS_MASK;
        self.color.set(c | (color << BITS_SHIFT));
    }
}

#[derive(Clone, Debug)]
pub struct VoronoiEdge {
    cell: usize,
    vertex: usize,
    twin: usize,
    next: usize,
    prev: usize,
    color: StdCell<usize>,
}

impl VoronoiEdge {
    fn new(is_linear: bool, is_primary: bool) -> Self {
        let mut color = 0;
        if is_linear {
            color |= BIT_IS_LINEAR;
        }
        if is_primary {
            color |= BIT_IS_PRIMARY;
        }
        VoronoiEdge { cell: NIL, vertex: NIL, twin: NIL, next: NIL, prev: NIL, color: StdCell::new(color) }
    }

    pub fn cell(&self) -> usize {
        self.cell
    }

    pub fn vertex0(&self) -> usize {
        self.vertex
    }

    pub fn twin(&self) -> usize {
        self.twin
    }

    pub fn next(&self) -> usize {
        self.next
    }

    pub fn prev(&self) -> usize {
        self.prev
    }

    /// Returns true if the edge is linear (segment, ray, line).
    pub fn is_linear(&self) -> bool {
        self.color.get() & BIT_IS_LINEAR != 0
    }

    /// Returns true if the edge is curved (parabolic arc).
    pub fn is_curved(&self) -> bool {
        self.color.get() & BIT_IS_LINEAR == 0
    }

    /// Returns false if edge goes through the endpoint of the segment.
    pub fn is_primary(&self) -> bool {
        self.color.get() & BIT_IS_PRIMARY != 0
    }

    pub fn is_secondary(&self) -> bool {
        self.color.get() & BIT_IS_PRIMARY == 0
    }

    pub fn color(&self) -> usize {
        self.color.get() >> BITS_SHIFT
    }

    pub fn set_color(&self, color: usize) {
        let c = self.color.get() & BITS_MASK;
        self.color.set(c | (color << BITS_SHIFT));
    }
}

#[derive(Default, Debug)]
pub struct VoronoiDiagram {
    cells: Vec<VoronoiCell>,
    vertices: Vec<VoronoiVertex>,
    edges: Vec<VoronoiEdge>,
}

const VERTEX_EQUALITY_ULPS: u32 = 128;

fn vertex_equal(v1: &VoronoiVertex, v2: &VoronoiVertex) -> bool {
    ulp_cmp(v1.x, v2.x, VERTEX_EQUALITY_ULPS) == UlpResult::Equal && ulp_cmp(v1.y, v2.y, VERTEX_EQUALITY_ULPS) == UlpResult::Equal
}

impl VoronoiDiagram {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.cells.clear();
        self.vertices.clear();
        self.edges.clear();
    }

    pub fn cells(&self) -> &[VoronoiCell] {
        &self.cells
    }

    pub fn vertices(&self) -> &[VoronoiVertex] {
        &self.vertices
    }

    pub fn edges(&self) -> &[VoronoiEdge] {
        &self.edges
    }

    pub fn num_cells(&self) -> usize {
        self.cells.len()
    }

    pub fn num_edges(&self) -> usize {
        self.edges.len()
    }

    pub fn num_vertices(&self) -> usize {
        self.vertices.len()
    }

    #[inline]
    pub fn cell(&self, i: usize) -> &VoronoiCell {
        &self.cells[i]
    }

    #[inline]
    pub fn vertex(&self, i: usize) -> &VoronoiVertex {
        &self.vertices[i]
    }

    #[inline]
    pub fn edge(&self, i: usize) -> &VoronoiEdge {
        &self.edges[i]
    }

    /// `edge->vertex1()`.
    #[inline]
    pub fn vertex1(&self, e: usize) -> usize {
        self.edges[self.edges[e].twin].vertex
    }

    /// `edge->rot_next()`: the rotation next edge over the starting point of the half-edge.
    #[inline]
    pub fn rot_next(&self, e: usize) -> usize {
        self.edges[self.edges[e].prev].twin
    }

    /// `edge->rot_prev()`.
    #[inline]
    pub fn rot_prev(&self, e: usize) -> usize {
        self.edges[self.edges[e].twin].next
    }

    /// Returns true if the edge is finite (segment, parabolic arc).
    #[inline]
    pub fn is_finite(&self, e: usize) -> bool {
        self.edges[e].vertex != NIL && self.vertex1(e) != NIL
    }

    /// Returns true if the edge is infinite (ray, line).
    #[inline]
    pub fn is_infinite(&self, e: usize) -> bool {
        self.edges[e].vertex == NIL || self.vertex1(e) == NIL
    }

    pub(crate) fn reserve(&mut self, num_sites: usize) {
        self.cells.reserve(num_sites);
        self.vertices.reserve(num_sites << 1);
        self.edges.reserve((num_sites << 2) + (num_sites << 1));
    }

    pub(crate) fn process_single_site(&mut self, site: &SiteEvent) {
        self.cells.push(VoronoiCell::new(site.initial_index, site.source_category()));
    }

    /// Insert a new half-edge pair for the bisector of two sites (site event processing).
    pub(crate) fn insert_new_edge(&mut self, site1: &SiteEvent, site2: &SiteEvent) -> (usize, usize) {
        let site_index1 = site1.sorted_index;
        let site_index2 = site2.sorted_index;

        let is_linear = is_linear_edge(site1, site2);
        let is_primary = is_primary_edge(site1, site2);

        self.edges.push(VoronoiEdge::new(is_linear, is_primary));
        let edge1 = self.edges.len() - 1;
        self.edges.push(VoronoiEdge::new(is_linear, is_primary));
        let edge2 = self.edges.len() - 1;

        if self.cells.is_empty() {
            self.cells.push(VoronoiCell::new(site1.initial_index, site1.source_category()));
        }

        self.cells.push(VoronoiCell::new(site2.initial_index, site2.source_category()));

        self.edges[edge1].cell = site_index1;
        self.edges[edge2].cell = site_index2;

        self.edges[edge1].twin = edge2;
        self.edges[edge2].twin = edge1;

        (edge1, edge2)
    }

    /// Insert a new half-edge pair starting at the Voronoi vertex of a circle event.
    pub(crate) fn insert_new_edge_circle(
        &mut self,
        site1: &SiteEvent,
        site3: &SiteEvent,
        circle: &CircleEvent,
        edge12: usize,
        edge23: usize,
    ) -> (usize, usize) {
        self.vertices.push(VoronoiVertex::new(circle.x(), circle.y()));
        let new_vertex = self.vertices.len() - 1;

        self.edges[edge12].vertex = new_vertex;
        self.edges[edge23].vertex = new_vertex;

        let is_linear = is_linear_edge(site1, site3);
        let is_primary = is_primary_edge(site1, site3);

        self.edges.push(VoronoiEdge::new(is_linear, is_primary));
        let new_edge1 = self.edges.len() - 1;
        self.edges[new_edge1].cell = site1.sorted_index;

        self.edges.push(VoronoiEdge::new(is_linear, is_primary));
        let new_edge2 = self.edges.len() - 1;
        self.edges[new_edge2].cell = site3.sorted_index;

        self.edges[new_edge1].twin = new_edge2;
        self.edges[new_edge2].twin = new_edge1;

        self.edges[new_edge2].vertex = new_vertex;

        self.edges[edge12].prev = new_edge1;
        self.edges[new_edge1].next = edge12;
        let edge12_twin = self.edges[edge12].twin;
        self.edges[edge12_twin].next = edge23;
        self.edges[edge23].prev = edge12_twin;
        let edge23_twin = self.edges[edge23].twin;
        self.edges[edge23_twin].next = new_edge2;
        self.edges[new_edge2].prev = edge23_twin;

        (new_edge1, new_edge2)
    }

    pub(crate) fn build(&mut self) {
        // Remove degenerate edges.
        let mut last_edge = 0usize;
        let mut it = 0usize;
        while it < self.edges.len() {
            let v1 = self.edges[it].vertex;
            let v2 = self.vertex1(it);
            if v1 != NIL && v2 != NIL && vertex_equal(&self.vertices[v1], &self.vertices[v2]) {
                self.remove_edge(it);
            } else {
                if it != last_edge {
                    self.edges[last_edge] = self.edges[it].clone();
                    self.edges[last_edge + 1] = self.edges[it + 1].clone();
                    let e1 = last_edge;
                    let e2 = last_edge + 1;
                    self.edges[e1].twin = e2;
                    self.edges[e2].twin = e1;
                    if self.edges[e1].prev != NIL {
                        let p = self.edges[e1].prev;
                        self.edges[p].next = e1;
                        let n = self.edges[e2].next;
                        self.edges[n].prev = e2;
                    }
                    if self.edges[e2].prev != NIL {
                        let n = self.edges[e1].next;
                        self.edges[n].prev = e1;
                        let p = self.edges[e2].prev;
                        self.edges[p].next = e2;
                    }
                }
                last_edge += 2;
            }
            it += 2;
        }
        self.edges.truncate(last_edge);

        // Set up incident edge pointers for cells and vertices.
        for it in 0..self.edges.len() {
            let c = self.edges[it].cell;
            self.cells[c].incident_edge = it;
            let v = self.edges[it].vertex;
            if v != NIL {
                self.vertices[v].incident_edge = it;
            }
        }

        // Remove degenerate vertices.
        let mut last_vertex = 0usize;
        for it in 0..self.vertices.len() {
            if self.vertices[it].incident_edge != NIL {
                if it != last_vertex {
                    self.vertices[last_vertex] = self.vertices[it].clone();
                    let v = last_vertex;
                    let start = self.vertices[v].incident_edge;
                    let mut e = start;
                    loop {
                        self.edges[e].vertex = v;
                        e = self.rot_next(e);
                        if e == start {
                            break;
                        }
                    }
                }
                last_vertex += 1;
            }
        }
        self.vertices.truncate(last_vertex);

        // Set up next/prev pointers for infinite edges.
        if self.vertices.is_empty() {
            if !self.edges.is_empty() {
                // Update prev/next pointers for the line edges.
                let mut edge_it = 0usize;
                let mut edge1 = edge_it;
                self.edges[edge1].next = edge1;
                self.edges[edge1].prev = edge1;

                edge_it += 1;
                edge1 = edge_it;
                edge_it += 1;

                while edge_it != self.edges.len() {
                    let edge2 = edge_it;
                    edge_it += 1;

                    self.edges[edge1].next = edge2;
                    self.edges[edge1].prev = edge2;
                    self.edges[edge2].next = edge1;
                    self.edges[edge2].prev = edge1;

                    edge1 = edge_it;
                    edge_it += 1;
                }

                self.edges[edge1].next = edge1;
                self.edges[edge1].prev = edge1;
            }
        } else {
            // Update prev/next pointers for the ray edges.
            for cell_it in 0..self.cells.len() {
                if self.cells[cell_it].is_degenerate() {
                    continue;
                }
                // Move to the previous edge while it is possible in the CW direction.
                let incident = self.cells[cell_it].incident_edge;
                let mut left_edge = incident;
                while self.edges[left_edge].prev != NIL {
                    left_edge = self.edges[left_edge].prev;
                    // Terminate if this is not a boundary cell.
                    if left_edge == incident {
                        break;
                    }
                }

                if self.edges[left_edge].prev != NIL {
                    continue;
                }

                let mut right_edge = incident;
                while self.edges[right_edge].next != NIL {
                    right_edge = self.edges[right_edge].next;
                }
                self.edges[left_edge].prev = right_edge;
                self.edges[right_edge].next = left_edge;
            }
        }
    }

    /// Remove degenerate edge.
    fn remove_edge(&mut self, edge: usize) {
        // Update the endpoints of the incident edges to the second vertex.
        let vertex = self.edges[edge].vertex;
        let edge_twin = self.edges[edge].twin;
        let mut updated_edge = self.rot_next(edge_twin);
        while updated_edge != edge_twin {
            self.edges[updated_edge].vertex = vertex;
            updated_edge = self.rot_next(updated_edge);
        }

        let edge1 = edge;
        let edge2 = edge_twin;

        let edge1_rot_prev = self.rot_prev(edge1);
        let edge1_rot_next = self.rot_next(edge1);

        let edge2_rot_prev = self.rot_prev(edge2);
        let edge2_rot_next = self.rot_next(edge2);

        // Update prev/next pointers for the incident edges.
        let t = self.edges[edge1_rot_next].twin;
        self.edges[t].next = edge2_rot_prev;
        self.edges[edge2_rot_prev].prev = t;
        let t = self.edges[edge2_rot_next].twin;
        self.edges[edge1_rot_prev].prev = t;
        self.edges[t].next = edge1_rot_prev;
    }
}

fn is_primary_edge(site1: &SiteEvent, site2: &SiteEvent) -> bool {
    let flag1 = site1.is_segment();
    let flag2 = site2.is_segment();
    if flag1 && !flag2 {
        return site1.point0 != site2.point0 && site1.point1 != site2.point0;
    }
    if !flag1 && flag2 {
        return site2.point0 != site1.point0 && site2.point1 != site1.point0;
    }
    true
}

fn is_linear_edge(site1: &SiteEvent, site2: &SiteEvent) -> bool {
    if !is_primary_edge(site1, site2) {
        return true;
    }
    !(site1.is_segment() ^ site2.is_segment())
}
