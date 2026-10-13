//! `Simplify.h/.cpp`: reduce the vertex count of polygons/polylines within error bounds.

use crate::geometry::lines_set::CheckNonEmptyParam;
use crate::geometry::mixed_lines_set::{AnyPolyline, MixedLinesSet};
use crate::geometry::point2ll::{v_size, v_size2, Point2LL};
use crate::geometry::{ClosedPolyline, LinesSet, OpenPolyline, Polygon, Polyline, Shape};
use crate::settings::settings::Settings;
use crate::utils::coord::Coord;
use crate::utils::extrusion_line::{ExtrusionJunction, ExtrusionLine};
use crate::utils::linear_alg_2d;
use cppstd::PriorityQueue;
use std::sync::Arc;

const MIN_RESOLUTION: Coord = 5;

/// A shape that Simplify can operate on (`Polygonal` template parameter).
pub trait Polygonal: Clone {
    type V: Clone;
    fn create_empty(&self) -> Self;
    fn size(&self) -> usize;
    fn at(&self, i: usize) -> &Self::V;
    fn set_at(&mut self, i: usize, v: Self::V);
    fn append_vertex(&mut self, v: Self::V);
    fn position(v: &Self::V) -> Point2LL;
    fn create_intersection(before: &Self::V, intersection: Point2LL, after: &Self::V) -> Self::V;
    fn area_deviation(before: &Self::V, vertex: &Self::V, after: &Self::V) -> Coord;
}

macro_rules! impl_polygonal_for_polyline {
    ($t:ty) => {
        impl Polygonal for $t {
            type V = Point2LL;
            fn create_empty(&self) -> Self {
                <$t>::default()
            }
            fn size(&self) -> usize {
                self.points.len()
            }
            fn at(&self, i: usize) -> &Point2LL {
                &self.points[i]
            }
            fn set_at(&mut self, i: usize, v: Point2LL) {
                self.points[i] = v;
            }
            fn append_vertex(&mut self, v: Point2LL) {
                self.points.push(v);
            }
            fn position(v: &Point2LL) -> Point2LL {
                *v
            }
            fn create_intersection(_before: &Point2LL, intersection: Point2LL, _after: &Point2LL) -> Point2LL {
                intersection
            }
            fn area_deviation(_before: &Point2LL, _vertex: &Point2LL, _after: &Point2LL) -> Coord {
                0
            }
        }
    };
}

impl_polygonal_for_polyline!(Polygon);
impl_polygonal_for_polyline!(ClosedPolyline);
impl_polygonal_for_polyline!(OpenPolyline);

impl Polygonal for ExtrusionLine {
    type V = ExtrusionJunction;
    fn create_empty(&self) -> Self {
        let mut result = ExtrusionLine::new(self.inset_idx, self.is_odd, false);
        result.is_closed = self.is_closed;
        result
    }
    fn size(&self) -> usize {
        self.junctions.len()
    }
    fn at(&self, i: usize) -> &ExtrusionJunction {
        &self.junctions[i]
    }
    fn set_at(&mut self, i: usize, v: ExtrusionJunction) {
        self.junctions[i] = v;
    }
    fn append_vertex(&mut self, v: ExtrusionJunction) {
        self.junctions.push(v);
    }
    fn position(v: &ExtrusionJunction) -> Point2LL {
        v.p
    }
    fn create_intersection(before: &ExtrusionJunction, intersection: Point2LL, after: &ExtrusionJunction) -> ExtrusionJunction {
        ExtrusionJunction::new(intersection, (before.w + after.w) / 2, before.perimeter_index as Coord)
    }
    fn area_deviation(before: &ExtrusionJunction, vertex: &ExtrusionJunction, after: &ExtrusionJunction) -> Coord {
        // Area deviation of removing a junction: compare variable-width area before and after.
        let ab_length = v_size(*vertex - *before);
        let bc_length = v_size(*after - *vertex);
        let ac_length = v_size(*after - *before);
        if ab_length == 0 || ac_length == 0 || bc_length == 0 {
            return 0; // Either of the line segments is zero, so the deviation of one of the line segments doesn't matter (it's zero). Not simplifying is "fine".
        }
        let width_diff = (vertex.w - before.w).abs().max((after.w - vertex.w).abs());
        if width_diff > 1 {
            // Adjust the width only if there is a difference, or else the rounding errors may produce the wrong
            // weighted average value.
            let ab_weight = (before.w + vertex.w) / 2;
            let bc_weight = (vertex.w + after.w) / 2;
            let weighted_average_width = (ab_length * ab_weight + bc_length * bc_weight) / ac_length;
            (ab_weight - weighted_average_width).abs() * ab_length + (bc_weight - weighted_average_width).abs() * bc_length
        } else {
            if ab_length > bc_length {
                width_diff * bc_length
            } else {
                width_diff * ab_length
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Simplify {
    pub max_resolution: Coord,
    pub max_deviation: Coord,
    pub max_area_deviation: Coord,
}

impl Simplify {
    pub fn new(max_resolution: Coord, max_deviation: Coord, max_area_deviation: Coord) -> Self {
        Simplify { max_resolution, max_deviation, max_area_deviation }
    }

    pub fn from_settings(settings: &Settings) -> Self {
        Simplify {
            max_resolution: settings.get_coord("meshfix_maximum_resolution"),
            max_deviation: settings.get_coord("meshfix_maximum_deviation"),
            max_area_deviation: settings.get_usize("meshfix_maximum_extrusion_area_deviation") as Coord,
        }
    }

    pub fn shape(&self, polygons: &Shape) -> Shape {
        let mut result = Shape::default();
        for p in polygons.iter() {
            result.push_checked(self.polygon(p), CheckNonEmptyParam::OnlyIfNotEmpty);
        }
        result
    }

    pub fn polygon(&self, polygon: &Polygon) -> Polygon {
        self.simplify(polygon, true)
    }

    pub fn extrusion_polygon(&self, polygon: &ExtrusionLine) -> ExtrusionLine {
        self.simplify(polygon, true)
    }

    pub fn polylines<L: Polyline + SimplifyPolyline>(&self, polylines: &LinesSet<L>) -> LinesSet<L> {
        let mut result = LinesSet::<L>::default();
        for p in polylines.iter() {
            result.push_checked(p.simplify_with(self), CheckNonEmptyParam::OnlyIfNotEmpty);
        }
        result
    }

    pub fn mixed(&self, polylines: &MixedLinesSet) -> MixedLinesSet {
        let mut result = MixedLinesSet::default();
        for polyline_ptr in polylines.iter() {
            match &**polyline_ptr {
                AnyPolyline::Open(open) => result.lines.push(Arc::new(AnyPolyline::Open(self.open_polyline(open)))),
                // A Polygon is also a ClosedPolyline in C++ (dynamic_pointer_cast succeeds) and is pushed as a ClosedPolyline.
                AnyPolyline::Closed(closed) => result.lines.push(Arc::new(AnyPolyline::Closed(self.closed_polyline(closed)))),
                AnyPolyline::Polygon(poly) => {
                    let closed = poly.as_closed_polyline();
                    result.lines.push(Arc::new(AnyPolyline::Closed(self.closed_polyline(&closed))))
                }
            }
        }
        result
    }

    pub fn closed_polyline(&self, polyline: &ClosedPolyline) -> ClosedPolyline {
        self.simplify(polyline, polyline.explicitly_closed)
    }

    pub fn open_polyline(&self, polyline: &OpenPolyline) -> OpenPolyline {
        self.simplify(polyline, false)
    }

    pub fn extrusion_polyline(&self, polyline: &ExtrusionLine) -> ExtrusionLine {
        self.simplify(polyline, false)
    }

    fn next_not_deleted(mut index: usize, to_delete: &[bool]) -> usize {
        let size = to_delete.len();
        index = (index + 1) % size;
        while to_delete[index] {
            index = (index + 1) % size;
        }
        index
    }

    fn previous_not_deleted(mut index: usize, to_delete: &[bool]) -> usize {
        let size = to_delete.len();
        index = (index + size - 1) % size;
        while to_delete[index] {
            index = (index + size - 1) % size;
        }
        index
    }

    fn detect_small<P: Polygonal>(polygon: &P, min_size: usize) -> bool {
        if polygon.size() < min_size {
            // For polygon, 2 or fewer vertices is degenerate. Delete it. For polyline, 1 vertex is degenerate.
            return true;
        }
        if polygon.size() == min_size {
            let a = P::position(polygon.at(0));
            let b = P::position(polygon.at(1));
            let c = P::position(polygon.at(polygon.size() - 1));
            if v_size2(b - a).max(v_size2(c - a)).max(v_size2(c - b)) < MIN_RESOLUTION * MIN_RESOLUTION {
                // All vertices are really close to each other. Delete it.
                return true;
            }
        }
        false
    }

    pub fn simplify<P: Polygonal>(&self, polygon: &P, is_closed: bool) -> P {
        let min_size: usize = if is_closed { 3 } else { 2 };
        if Self::detect_small(polygon, min_size) {
            return polygon.create_empty();
        }
        if polygon.size() == min_size {
            // For polygon, don't reduce below 3. For polyline, not below 2.
            return polygon.clone();
        }

        let mut to_delete = vec![false; polygon.size()];
        // Comparator: the priority queue pops the *least important* vertex first (by importance, then by lowest index).
        let comparator = |a: &(usize, Coord), b: &(usize, Coord)| a.1 > b.1 || (a.1 == b.1 && a.0 > b.0);
        let mut by_importance = PriorityQueue::new(comparator);

        let mut result = polygon.clone(); // Make a copy so that we can also shift vertices.
        let size = polygon.size() as i64;
        let mut current_removed: i64 = -1;
        // `(polygon.size() - current_removed) > min_size` with size_t arithmetic.
        while ((size - current_removed) as u64) > min_size as u64 && current_removed != 0 {
            current_removed = 0;

            // Add the initial points.
            for i in 0..result.size() {
                if to_delete[i] {
                    continue;
                }
                let vertex_importance = self.importance(&result, &to_delete, i, is_closed);
                by_importance.push((i, vertex_importance));
            }

            // Iteratively remove the least important point until a threshold.
            while !by_importance.is_empty() && ((size - current_removed) as u64) > min_size as u64 {
                let vertex = by_importance.pop().unwrap();
                // The importance may have changed since this vertex was inserted. Re-compute it now.
                // If it doesn't change, it's safe to process.
                let vertex_importance = self.importance(&result, &to_delete, vertex.0, is_closed);
                if vertex_importance != vertex.1 {
                    by_importance.push((vertex.0, vertex_importance)); // Re-insert with updated importance.
                    continue;
                }

                if vertex_importance <= self.max_deviation * self.max_deviation {
                    current_removed += if self.remove(&mut result, &mut to_delete, vertex.0, vertex_importance, is_closed) { 1 } else { 0 };
                }
            }
        }

        // Now remove the marked vertices in one sweep.
        let mut filtered = polygon.create_empty();
        for i in 0..result.size() {
            if !to_delete[i] {
                filtered.append_vertex(result.at(i).clone());
            }
        }

        if Self::detect_small(&filtered, min_size) {
            return filtered.create_empty();
        }
        filtered
    }

    fn importance<P: Polygonal>(&self, polygon: &P, to_delete: &[bool], index: usize, is_closed: bool) -> Coord {
        let poly_size = polygon.size();
        if !is_closed && (index == 0 || index == poly_size - 1) {
            return Coord::MAX; // Endpoints of the polyline must always be retained.
        }
        // From here on out we can safely look at the vertex neighbors and assume it's a polygon. We won't go out of bounds of the polyline.

        let vertex = P::position(polygon.at(index));
        let before_index = Self::previous_not_deleted(index, to_delete);
        let after_index = Self::next_not_deleted(index, to_delete);

        let area_deviation = P::area_deviation(polygon.at(before_index), polygon.at(index), polygon.at(after_index));
        if area_deviation > self.max_area_deviation {
            // Removing this vertex would cause too much change in the area.
            return Coord::MAX;
        }

        let before = P::position(polygon.at(before_index));
        let after = P::position(polygon.at(after_index));
        let deviation2 = linear_alg_2d::get_dist2_from_line(vertex, before, after);
        if deviation2 <= MIN_RESOLUTION * MIN_RESOLUTION {
            // Deviation so small that it's always desired to remove them.
            return deviation2;
        }
        if v_size2(before - vertex) > self.max_resolution * self.max_resolution
            && v_size2(after - vertex) > self.max_resolution * self.max_resolution
        {
            return Coord::MAX; // Long line segments, no need to remove this one.
        }
        deviation2
    }

    fn remove<P: Polygonal>(&self, polygon: &mut P, to_delete: &mut [bool], vertex: usize, deviation2: Coord, is_closed: bool) -> bool {
        if deviation2 <= MIN_RESOLUTION * MIN_RESOLUTION {
            // At less than the minimum resolution we're always allowed to delete the vertex.
            // Even if the adjacent line segments are very long.
            to_delete[vertex] = true;
            return true;
        }

        let before = Self::previous_not_deleted(vertex, to_delete);
        let after = Self::next_not_deleted(vertex, to_delete);
        let vertex_position = P::position(polygon.at(vertex));
        let before_position = P::position(polygon.at(before));
        let after_position = P::position(polygon.at(after));
        let length2_before = v_size2(vertex_position - before_position);
        let length2_after = v_size2(vertex_position - after_position);

        if length2_before <= self.max_resolution * self.max_resolution && length2_after <= self.max_resolution * self.max_resolution {
            // Both adjacent line segments are short.
            // Removing this vertex does little harm. No long lines will be shifted.
            to_delete[vertex] = true;
            return true;
        }

        // Otherwise, one edge next to this vertex is longer than max_resolution. The other is shorter.
        // In this case we want to remove the short edge by replacing it with a vertex where the two surrounding edges intersect.
        // Find the two line segments surrounding the short edge here ("before" and "after" edges).
        let (before_from, before_to, after_from, after_to);
        if length2_before <= length2_after {
            // Before is the shorter line.
            if !is_closed && before == 0 {
                // No edge before the short edge.
                return false; // Edge cannot be deleted without shifting a long edge. Don't remove anything.
            }
            let before_before = Self::previous_not_deleted(before, to_delete);
            before_from = P::position(polygon.at(before_before));
            before_to = P::position(polygon.at(before));
            after_from = P::position(polygon.at(vertex));
            after_to = P::position(polygon.at(after));
        } else {
            if !is_closed && after == polygon.size() - 1 {
                // No edge after the short edge.
                return false; // Edge cannot be deleted without shifting a long edge. Don't remove anything.
            }
            let after_after = Self::next_not_deleted(after, to_delete);
            before_from = P::position(polygon.at(before));
            before_to = P::position(polygon.at(vertex));
            after_from = P::position(polygon.at(after));
            after_to = P::position(polygon.at(after_after));
        }
        let intersection = match linear_alg_2d::line_line_intersection(before_from, before_to, after_from, after_to) {
            Some(p) => p,
            None => return false, // Lines are parallel to each other. No intersection.
        };

        let intersection_deviation = linear_alg_2d::get_dist2_from_line_segment(before_to, intersection, after_from);
        if intersection_deviation <= self.max_deviation * self.max_deviation {
            // Intersection point doesn't deviate too much. Use it!
            to_delete[vertex] = true;
            let idx = if length2_before <= length2_after { before } else { after };
            let new_v = P::create_intersection(polygon.at(before), intersection, polygon.at(after));
            polygon.set_at(idx, new_v);
            return true;
        }
        false
    }
}

/// Dispatch of `Simplify::polyline` overloads for the line types stored in a `LinesSet`.
pub trait SimplifyPolyline: Sized {
    fn simplify_with(&self, s: &Simplify) -> Self;
}

impl SimplifyPolyline for OpenPolyline {
    fn simplify_with(&self, s: &Simplify) -> Self {
        s.open_polyline(self)
    }
}

impl SimplifyPolyline for ClosedPolyline {
    fn simplify_with(&self, s: &Simplify) -> Self {
        s.closed_polyline(self)
    }
}
