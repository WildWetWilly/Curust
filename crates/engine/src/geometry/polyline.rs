//! `geometry/PointsSet.h`, `Polyline.h`, `OpenPolyline.h`, `ClosedPolyline.h`, `Polygon.h`.
//!
//! The C++ class hierarchy `PointsSet -> Polyline -> {OpenPolyline, ClosedPolyline -> Polygon}`
//! is represented by three concrete structs that dereference to `Vec<Point2LL>` and share the
//! behaviour of the abstract `Polyline` base through the [`Polyline`] trait.

use super::lines_set::{OpenLinesSet, Shape};
use super::matrix::{Point3Matrix, PointMatrix};
use super::point2ll::*;
use crate::settings::types::{AngleDegrees, AngleRadians};
use crate::utils::coord::{int2mm, Coord};
use crate::utils::linear_alg_2d as la;
use crate::utils::list_poly_it::{insert_point_non_duplicate, ListPolyIt, ListPolygon};
use clipper::{ClipType, Clipper, ClipperOffset, EndType, JoinType, PolyFillType, PolyType};
use std::f64::consts::PI;
use std::ops::{Deref, DerefMut};

pub type Path = Vec<Point2LL>;

/// Common behaviour of all polyline types (`cura::Polyline`).
pub trait Polyline: Clone + Default + std::fmt::Debug + Deref<Target = Vec<Point2LL>> + DerefMut + Send + Sync + 'static {
    /// True only for `Polygon` (decides whether it is added to Clipper as a closed path).
    const IS_POLYGON: bool;
    /// True only for `OpenPolyline` (used where C++ does `dynamic_cast<OpenPolyline*>`).
    const IS_OPEN: bool;

    fn has_closing_segment(&self) -> bool;
    fn segments_count(&self) -> usize;
    fn is_valid(&self) -> bool;

    /// Construct from raw points, as `LineType(std::move(path))` / `emplace_back(path)` would
    /// (closed types are not explicitly closed).
    fn from_path(path: Path) -> Self;

    fn points(&self) -> &Vec<Point2LL> {
        self
    }
    fn points_mut(&mut self) -> &mut Vec<Point2LL> {
        self
    }
    fn set_points(&mut self, points: Path) {
        *self.points_mut() = points;
    }

    /// Number of segments visited when iterating `beginSegments()..endSegments()` (note that a
    /// single-point line yields one degenerate segment, as in C++).
    #[inline]
    fn segment_iter_count(&self) -> usize {
        let n = self.len();
        if self.has_closing_segment() {
            n
        } else if n > 1 {
            n - 1
        } else {
            n
        }
    }

    /// Segment `i` as visited by the C++ SegmentIterator.
    #[inline]
    fn segment(&self, i: usize) -> (Point2LL, Point2LL) {
        let n = self.len();
        if i == n - 1 {
            (self[i], self[0])
        } else {
            (self[i], self[i + 1])
        }
    }

    fn segments(&self) -> SegmentIter<'_> {
        SegmentIter { points: self.points(), idx: 0, count: self.segment_iter_count() }
    }

    fn split_into_segments_into(&self, result: &mut OpenLinesSet) {
        result.reserve(self.segments_count());
        for (s, e) in self.segments() {
            result.push(OpenPolyline::new(vec![s, e]));
        }
    }

    fn split_into_segments(&self) -> OpenLinesSet {
        let mut result = OpenLinesSet::default();
        self.split_into_segments_into(&mut result);
        result
    }

    fn orientation(&self) -> bool {
        clipper::orientation(self)
    }

    fn length(&self) -> Coord {
        // std::accumulate with an `int` initial value: the running total is an int.
        let mut total: i32 = 0;
        for (s, e) in self.segments() {
            total = (total as i64 + v_size(e - s)) as i32;
        }
        total as Coord
    }

    fn shorter_than(&self, check_length: Coord) -> bool {
        let mut length: Coord = 0;
        for (s, e) in self.segments() {
            length += v_size(e - s);
            if length >= check_length {
                return false;
            }
        }
        true
    }

    fn push_back_if_forming_segment(&mut self, point: Point2LL) -> bool {
        if self.is_empty() || !fuzzy_equal(point, *self.last().unwrap()) {
            self.push(point);
            return true;
        }
        false
    }

    fn reverse_points(&mut self) {
        self.points_mut().reverse();
    }

    fn translate(&mut self, translation: Point2LL) {
        for p in self.points_mut().iter_mut() {
            *p += translation;
        }
    }

    fn apply_matrix(&mut self, matrix: &PointMatrix) {
        for p in self.points_mut().iter_mut() {
            *p = matrix.apply(*p);
        }
    }

    fn apply_matrix3(&mut self, matrix: &Point3Matrix) {
        for p in self.points_mut().iter_mut() {
            *p = matrix.apply(*p);
        }
    }

    /// `Polyline::removeColinearEdges`.
    fn remove_colinear_edges(&mut self, max_deviation_angle: AngleRadians) {
        let max_dev = max_deviation_angle.value();
        // TODO: Can be made more efficient (for example, use pointer-types for process-/skip-indices, so we can swap them without copy).
        loop {
            let mut num_removed_in_iteration = 0usize;
            // Note: this is the same as the C++ outer do/while.
            let mut process_indices: Vec<bool> = vec![true; self.len()];
            let mut go = true;
            while go {
                go = false;
                let rpath = self.points().clone();
                let pathlen = rpath.len();
                if pathlen <= 3 {
                    return;
                }
                let mut skip_indices: Vec<bool> = vec![false; self.len()];
                let mut new_path: Vec<Point2LL> = Vec::new();
                let mut point_idx = 0usize;
                while point_idx < pathlen {
                    // Don't iterate directly over process-indices, but do it this way, because there are points _in_ process-indices that should nonetheless be skipped:
                    if !process_indices[point_idx] {
                        new_path.push(rpath[point_idx]);
                        point_idx += 1;
                        continue;
                    }
                    // Should skip the last point for this iteration if the old first was removed (which can be seen from the fact that the new first was skipped):
                    if point_idx == pathlen - 1 && skip_indices[0] {
                        skip_indices[new_path.len()] = true;
                        go = true;
                        new_path.push(rpath[point_idx]);
                        break;
                    }
                    let prev = rpath[(point_idx + pathlen - 1) % pathlen];
                    let p = rpath[point_idx];
                    let next = rpath[(point_idx + 1) % pathlen];
                    let mut angle = la::get_angle_left(prev, p, next); // [0 : 2 * pi]
                    if angle >= PI {
                        angle -= PI;
                    } // map [pi : 2 * pi] to [0 : pi]
                    // Check if the angle is within limits for the point to 'make sense', given the maximum deviation.
                    // If the angle indicates near-parallel segments ignore the point 'pt'
                    if angle > max_dev && angle < PI - max_dev {
                        new_path.push(p);
                    } else if point_idx != pathlen - 1 {
                        // Skip the next point, since the current one was removed:
                        skip_indices[new_path.len()] = true;
                        go = true;
                        new_path.push(next);
                        point_idx += 1;
                    }
                    point_idx += 1;
                }
                self.set_points(new_path);
                num_removed_in_iteration += pathlen - self.len();
                process_indices.clear();
                process_indices.extend_from_slice(&skip_indices);
            }
            if num_removed_in_iteration == 0 {
                break;
            }
        }
    }
}

/// Iterator over the segments of a polyline (`SegmentIterator`).
pub struct SegmentIter<'a> {
    points: &'a [Point2LL],
    idx: usize,
    count: usize,
}

impl<'a> Iterator for SegmentIter<'a> {
    type Item = (Point2LL, Point2LL);
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.idx >= self.count {
            return None;
        }
        let i = self.idx;
        self.idx += 1;
        let n = self.points.len();
        Some(if i == n - 1 { (self.points[i], self.points[0]) } else { (self.points[i], self.points[i + 1]) })
    }
}

macro_rules! impl_points_deref {
    ($t:ty) => {
        impl Deref for $t {
            type Target = Vec<Point2LL>;
            #[inline]
            fn deref(&self) -> &Vec<Point2LL> {
                &self.points
            }
        }
        impl DerefMut for $t {
            #[inline]
            fn deref_mut(&mut self) -> &mut Vec<Point2LL> {
                &mut self.points
            }
        }
    };
}

/// A polyline that is not closed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenPolyline {
    pub points: Path,
}

impl OpenPolyline {
    pub fn new(points: Path) -> Self {
        OpenPolyline { points }
    }
}

impl_points_deref!(OpenPolyline);

impl Polyline for OpenPolyline {
    const IS_POLYGON: bool = false;
    const IS_OPEN: bool = true;
    #[inline]
    fn has_closing_segment(&self) -> bool {
        false
    }
    #[inline]
    fn segments_count(&self) -> usize {
        if self.len() > 1 {
            self.len() - 1
        } else {
            0
        }
    }
    #[inline]
    fn is_valid(&self) -> bool {
        self.len() >= 2
    }
    fn from_path(path: Path) -> Self {
        OpenPolyline { points: path }
    }
}

/// A closed polyline. It is "explicitly closed" when the last point repeats the first one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClosedPolyline {
    pub points: Path,
    pub explicitly_closed: bool,
}

impl_points_deref!(ClosedPolyline);

/// Shared implementation of `ClosedPolyline` methods for ClosedPolyline and Polygon.
pub trait ClosedLine: Polyline {
    fn is_explicitly_closed(&self) -> bool;
    fn set_explicitly_closed(&mut self, explicitly_closed: bool);

    /// `ClosedPolyline::inside(p, border_result)`.
    fn inside(&self, p: Point2LL, border_result: bool) -> bool {
        let res = clipper::point_in_polygon(p, self);
        if res == -1 {
            return border_result;
        }
        res == 1
    }

    /// `ClosedPolyline::inside(const Path& polygon)`: all points (non-zero result) inside polygon.
    fn inside_path(&self, polygon: &[Point2LL]) -> bool {
        self.iter().all(|p| clipper::point_in_polygon(*p, polygon) != 0)
    }

    fn to_pseudo_open_polyline(&self) -> OpenPolyline {
        let mut open_polyline = OpenPolyline::new(self.points().clone());
        if self.has_closing_segment() {
            let f = open_polyline[0];
            open_polyline.push(f);
        }
        open_polyline
    }

    fn shift_vertices_to_start_point(&mut self, start_index: usize) {
        if !self.is_valid() || start_index == 0 || start_index >= self.len() {
            return;
        }
        let explicitly_closed = self.is_explicitly_closed();
        if explicitly_closed {
            self.points_mut().pop();
        }
        self.points_mut().rotate_left(start_index);
        if explicitly_closed {
            let f = self[0];
            self.push(f);
        }
    }
}

#[inline]
fn closed_segments_count(len: usize, explicitly_closed: bool) -> usize {
    if explicitly_closed {
        if len >= 3 {
            len - 1
        } else {
            0
        }
    } else if len >= 2 {
        len
    } else {
        0
    }
}

impl ClosedPolyline {
    pub fn new(points: Path, explicitly_closed: bool) -> Self {
        ClosedPolyline { points, explicitly_closed }
    }
}

impl Polyline for ClosedPolyline {
    const IS_POLYGON: bool = false;
    const IS_OPEN: bool = false;
    #[inline]
    fn has_closing_segment(&self) -> bool {
        !self.explicitly_closed
    }
    #[inline]
    fn segments_count(&self) -> usize {
        closed_segments_count(self.len(), self.explicitly_closed)
    }
    #[inline]
    fn is_valid(&self) -> bool {
        self.len() >= if self.explicitly_closed { 4 } else { 3 }
    }
    fn from_path(path: Path) -> Self {
        ClosedPolyline { points: path, explicitly_closed: false }
    }
}

impl ClosedLine for ClosedPolyline {
    fn is_explicitly_closed(&self) -> bool {
        self.explicitly_closed
    }
    fn set_explicitly_closed(&mut self, explicitly_closed: bool) {
        self.explicitly_closed = explicitly_closed;
    }
}

/// A closed polyline that encloses an area.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Polygon {
    pub points: Path,
    pub explicitly_closed: bool,
}

impl_points_deref!(Polygon);

impl Polyline for Polygon {
    const IS_POLYGON: bool = true;
    const IS_OPEN: bool = false;
    #[inline]
    fn has_closing_segment(&self) -> bool {
        !self.explicitly_closed
    }
    #[inline]
    fn segments_count(&self) -> usize {
        closed_segments_count(self.len(), self.explicitly_closed)
    }
    #[inline]
    fn is_valid(&self) -> bool {
        self.len() >= if self.explicitly_closed { 4 } else { 3 }
    }
    fn from_path(path: Path) -> Self {
        Polygon { points: path, explicitly_closed: false }
    }
}

impl ClosedLine for Polygon {
    fn is_explicitly_closed(&self) -> bool {
        self.explicitly_closed
    }
    fn set_explicitly_closed(&mut self, explicitly_closed: bool) {
        self.explicitly_closed = explicitly_closed;
    }
}

impl From<ClosedPolyline> for Polygon {
    fn from(c: ClosedPolyline) -> Self {
        Polygon { points: c.points, explicitly_closed: c.explicitly_closed }
    }
}

impl Polygon {
    pub fn new(points: Path, explicitly_closed: bool) -> Self {
        Polygon { points, explicitly_closed }
    }

    /// A non-explicitly closed polygon from points.
    pub fn from_points(points: Path) -> Self {
        Polygon { points, explicitly_closed: false }
    }

    pub fn as_closed_polyline(&self) -> ClosedPolyline {
        ClosedPolyline { points: self.points.clone(), explicitly_closed: self.explicitly_closed }
    }

    #[inline]
    pub fn area(&self) -> f64 {
        clipper::area(self)
    }

    pub fn intersection(&self, other: &Polygon) -> Shape {
        let mut ret_paths = Vec::new();
        let mut clipper = Clipper::new(0);
        clipper.add_path(self, PolyType::Subject, true);
        clipper.add_path(other, PolyType::Clip, true);
        clipper.execute(ClipType::Intersection, &mut ret_paths, PolyFillType::EvenOdd, PolyFillType::EvenOdd);
        Shape::from_paths(ret_paths)
    }

    pub fn center_of_mass(&self) -> Point2LL {
        if !self.is_empty() {
            let mut p0 = self[0];
            if self.len() > 1 {
                let mut x = 0.0f64;
                let mut y = 0.0f64;
                for n in 1..=self.len() {
                    let p1 = self[n % self.len()];
                    let second_factor = ((p0.x * p1.y) - (p1.x * p0.y)) as f64;
                    x += (p0.x + p1.x) as f64 * second_factor;
                    y += (p0.y + p1.y) as f64 * second_factor;
                    p0 = p1;
                }
                let current_area = self.area();
                x = x / 6.0 / current_area;
                y = y / 6.0 / current_area;
                return pt(cppstd::llrint(x), cppstd::llrint(y));
            }
            return p0;
        }
        Point2LL::default()
    }

    /// `Polygon::offset(int distance, join_type = jtMiter, miter_limit = 1.2)`.
    pub fn offset(&self, distance: i32, join_type: JoinType, miter_limit: f64) -> Shape {
        if distance == 0 {
            return Shape::from_polygon(self.clone());
        }
        let mut ret = Vec::new();
        let mut clipper = ClipperOffset::new(miter_limit, 10.0);
        clipper.add_path(self, join_type, EndType::ClosedPolygon);
        clipper.miter_limit = miter_limit;
        clipper.execute(&mut ret, distance as f64);
        Shape::from_paths(ret)
    }

    pub fn offset_default(&self, distance: i32) -> Shape {
        self.offset(distance, JoinType::Miter, 1.2)
    }

    pub fn smooth2(&self, remove_length: i32, result: &mut Polygon) {
        if !self.is_empty() {
            result.push(self[0]);
        }
        let remove_length = remove_length as Coord;
        let mut poly_idx = 1usize;
        while poly_idx < self.len() {
            let last = self[poly_idx - 1];
            let now = self[poly_idx];
            let next = self[(poly_idx + 1) % self.len()];
            if shorter_then(last - now, remove_length) && shorter_then(now - next, remove_length) {
                poly_idx += 1; // skip the next line piece (dont escalate the removal of edges)
                if poly_idx < self.len() {
                    result.push(self[poly_idx]);
                }
            } else {
                result.push(self[poly_idx]);
            }
            poly_idx += 1;
        }
    }

    pub fn smooth(&self, remove_length: i32, result: &mut Polygon) {
        // a typical zigzag with the middle part to be removed by removing (1) :
        //
        //               3
        //               ^
        //               |
        //               |
        // inside        |     outside
        //          1--->2
        //          ^
        //          |
        //          |
        //          |
        //          0
        if !self.is_empty() {
            result.push(self[0]);
        }

        let remove_length = remove_length as i64;
        let is_zigzag = |v02_size: i64, v12_size: i64, v13_size: i64, dot1: i64, dot2: i64| -> bool {
            if v12_size > remove_length {
                // v12 or v13 is too long
                return false;
            }
            let p1_is_left_of_v02 = dot1 < 0;
            if !p1_is_left_of_v02 {
                // removing p1 wouldn't smooth outward
                return false;
            }
            let p2_is_left_of_v13 = dot2 > 0;
            if p2_is_left_of_v13 {
                // l0123 doesn't constitute a zigzag ''|,,
                return false;
            }
            if -dot1 <= v02_size * v12_size / 2 {
                // angle at p1 isn't sharp enough
                return false;
            }
            if -dot2 <= v13_size * v12_size / 2 {
                // angle at p2 isn't sharp enough
                return false;
            }
            true
        };
        let mut v02 = self[2] - self[0];
        let mut v02_t = turn90_ccw(v02);
        let mut v02_size = v_size(v02);
        let mut force_push = false;
        for poly_idx in 1..self.len() {
            let p1 = self[poly_idx];
            let p2 = self[(poly_idx + 1) % self.len()];
            let p3 = self[(poly_idx + 2) % self.len()];
            // v02 computed in last iteration
            // v02_size as well
            let v12 = p2 - p1;
            let v12_size = v_size(v12);
            let v13 = p3 - p1;
            let v13_size = v_size(v13);

            // v02T computed in last iteration
            let dot1 = dot(v02_t, v12);
            let v13_t = turn90_ccw(v13);
            let dot2 = dot(v13_t, v12);
            let push_point = force_push || !is_zigzag(v02_size, v12_size, v13_size, dot1, dot2);
            force_push = false;
            if push_point {
                result.push(p1);
            } else {
                // do not add the current one to the result
                force_push = true; // ensure the next point is added; it cannot also be a zigzag
            }
            v02_t = v13_t;
            v02 = v13;
            v02_size = v13_size;
        }
        let _ = v02;
    }

    pub fn smooth_outward(&self, min_angle: AngleDegrees, shortcut_length: i32, result: &mut Polygon) {
        // example of smoothed out corner:
        //
        //               6
        //               ^
        //               |
        // inside        |     outside
        //         2>3>4>5
        //         ^    /                   .
        //         |   /                    .
        //         1  /                     .
        //         ^ /                      .
        //         |/                       .
        //         |
        //         |
        //         0

        let shortcut_length2: i32 = shortcut_length.wrapping_mul(shortcut_length);
        let cos_min_angle = (min_angle.value() / 180.0 * PI).cos();

        let mut poly = ListPolygon::from_polygon(self);
        {
            // remove duplicate vertices
            let mut p1_it = poly.begin();
            loop {
                let next = poly.next(p1_it);
                if v_size2(poly.p(p1_it) - poly.p(next)) < 100 {
                    poly.remove(p1_it);
                }
                p1_it = next;
                if p1_it == poly.begin() {
                    break;
                }
            }
        }

        let mut p1_it = poly.begin();
        loop {
            let p1 = poly.p(p1_it);
            let mut p0_it = poly.prev(p1_it);
            let mut p2_it = poly.next(p1_it);
            let p0 = poly.p(p0_it);
            let p2 = poly.p(p2_it);

            let v10 = p0 - p1;
            let v12 = p2 - p1;
            // INT2MM(INT2MM(x)): the inner result is a double, divided again by 1000.
            let cos_angle = (dot(v10, v12) as f64 / 1000.0 / 1000.0) / v_size_mm(v10) / v_size_mm(v12);
            let is_left_angle = la::point_is_left_of_line(p1, p0, p2) > 0;
            if cos_angle > cos_min_angle && is_left_angle {
                // angle is so sharp that it can be removed
                let v02 = poly.p(p2_it) - poly.p(p0_it);
                if v_size2(v02) >= shortcut_length2 as i64 {
                    Self::smooth_corner_simple(&mut poly, p0, p1, p2, p0_it, p1_it, p2_it, v10, v12, v02, shortcut_length as i64, cos_angle);
                } else {
                    let remove_poly = Self::smooth_corner_complex(&mut poly, p1, &mut p0_it, &mut p2_it, shortcut_length as i64); // edits p0_it and p2_it!
                    if remove_poly {
                        // don't convert ListPolygon into result
                        return;
                    }
                }
                // update:
                p1_it = p2_it; // next point to consider for whether it's an internal corner
            } else {
                p1_it = poly.next(p1_it);
            }
            if p1_it == poly.begin() {
                break;
            }
        }

        poly.to_polygon(result);
    }

    #[allow(clippy::too_many_arguments)]
    fn smooth_corner_simple(
        poly: &mut ListPolygon,
        p0: Point2LL,
        p1: Point2LL,
        p2: Point2LL,
        p0_it: ListPolyIt,
        p1_it: ListPolyIt,
        p2_it: ListPolyIt,
        v10: Point2LL,
        v12: Point2LL,
        v02: Point2LL,
        shortcut_length: i64,
        cos_angle: f64,
    ) {
        //  1----b---->2
        //  ^   /
        //  |  /
        //  | /
        //  |/
        //  |a
        //  |
        //  0
        // ideally a1_size == b1_size
        if v_size2(v02) <= shortcut_length * (shortcut_length + 10) // v02 is approximately shortcut length
            || (cos_angle > 0.9999 && la::get_dist2_from_line(p2, p0, p1) < 20 * 20)
        // p1 is degenerate
        {
            // handle this separately to avoid rounding problems below in the getPointOnLineWithDist function
            // p0_it and p2_it are already correct
            poly.remove(p1_it);
        } else {
            // compute the distance a1 == b1 to get vSize(ab)==shortcut_length with the given angle between v10 and v12
            //       1
            //      /|\                .
            //     / | \               .
            //    /  |  \              .
            //   /   |   \             .
            // a/____|____\b           .
            //       m
            // use trigonometry on the right-angled triangle am1
            let a1m_angle = cos_angle.acos() / 2.0;
            let a1_size = (shortcut_length as f64 / 2.0 / a1m_angle.sin()) as i64;
            if a1_size * a1_size < v_size2(v10) && a1_size * a1_size < v_size2(v12) {
                let a = p1 + normal(v10, a1_size);
                let b = p1 + normal(v12, a1_size);
                insert_point_non_duplicate(poly, p0_it, p1_it, a);
                insert_point_non_duplicate(poly, p1_it, p2_it, b);
                poly.remove(p1_it);
            } else if v_size2(v12) < v_size2(v10) {
                //     b
                //  1->2
                //  ^  |
                //  |  |
                //  |  |
                //  |  |
                //  |a/
                //  |/
                //  0
                let b = poly.p(p2_it);
                if let Some(a) = la::get_point_on_line_with_dist(b, p1, p0, shortcut_length) {
                    insert_point_non_duplicate(poly, p0_it, p1_it, a);
                }
                poly.remove(p1_it);
            } else {
                //     1---------b----------2
                //     ^       ,-'
                //     |   ,-'
                //     |,-'
                //     a
                //     |
                //     0
                let a = poly.p(p0_it);
                if let Some(b) = la::get_point_on_line_with_dist(a, p1, p2, shortcut_length) {
                    insert_point_non_duplicate(poly, p1_it, p2_it, b);
                }
                poly.remove(p1_it);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn smooth_outward_step(
        poly: &ListPolygon,
        p1: Point2LL,
        shortcut_length2: i64,
        p0_it: &mut ListPolyIt,
        p2_it: &mut ListPolyIt,
        forward_is_blocked: &mut bool,
        backward_is_blocked: &mut bool,
        forward_is_too_far: &mut bool,
        backward_is_too_far: &mut bool,
    ) {
        let forward_has_converged = *forward_is_blocked || *forward_is_too_far;
        let backward_has_converged = *backward_is_blocked || *backward_is_too_far;
        let p0 = poly.p(*p0_it);
        let p2 = poly.p(*p2_it);
        let walk_forward = !forward_has_converged && (backward_has_converged || (v_size2(p2 - p1) < v_size2(p0 - p1))); // walk a step forward

        if walk_forward {
            let p2_2_it = poly.next(*p2_it);
            let p2_2 = poly.p(p2_2_it);
            let p2_is_left = la::point_is_left_of_line(p2, p0, p2_2) >= 0;
            if !p2_is_left {
                *forward_is_blocked = true;
                return;
            }

            let v02_2 = p2_2 - poly.p(*p0_it);
            if v_size2(v02_2) > shortcut_length2 {
                *forward_is_too_far = true;
                return;
            }

            *p2_it = p2_2_it; // make one step in the forward direction
            *backward_is_blocked = false; // invalidate data about backward walking
            *backward_is_too_far = false;
            return;
        }
        let p0_2_it = poly.prev(*p0_it);
        let p0_2 = poly.p(p0_2_it);
        let p0_is_left = la::point_is_left_of_line(p0, p0_2, p2) >= 0;
        if !p0_is_left {
            *backward_is_blocked = true;
            return;
        }

        let v02_2 = poly.p(*p2_it) - p0_2;
        if v_size2(v02_2) > shortcut_length2 {
            *backward_is_too_far = true;
            return;
        }

        *p0_it = p0_2_it; // make one step in the backward direction
        *forward_is_blocked = false; // invalidate data about forward walking
        *forward_is_too_far = false;
    }

    fn smooth_corner_complex(poly: &mut ListPolygon, p1: Point2LL, p0_it: &mut ListPolyIt, p2_it: &mut ListPolyIt, shortcut_length: i64) -> bool {
        // walk away from the corner until the shortcut > shortcut_length or it would smooth a piece inward
        // - walk in both directions untill shortcut > shortcut_length
        // - stop walking in one direction if it would otherwise cut off a corner in that direction
        // - same in the other direction
        // - stop if both are cut off
        // walk by updating p0_it and p2_it
        let shortcut_length2 = shortcut_length * shortcut_length;
        let mut forward_is_blocked = false;
        let mut forward_is_too_far = false;
        let mut backward_is_blocked = false;
        let mut backward_is_too_far = false;
        loop {
            let forward_has_converged = forward_is_blocked || forward_is_too_far;
            let backward_has_converged = backward_is_blocked || backward_is_too_far;
            if forward_has_converged && backward_has_converged {
                if forward_is_too_far && backward_is_too_far && v_size2(poly.p(poly.prev(*p0_it)) - poly.p(poly.next(*p2_it))) < shortcut_length2 {
                    //         o
                    //       /   \                                                  .
                    //      o     o
                    //      |     |
                    //      \     /                                                 .
                    //       |   |
                    //       \   /                                                  .
                    //        | |
                    //        o o
                    *p0_it = poly.prev(*p0_it);
                    *p2_it = poly.next(*p2_it);
                    forward_is_too_far = false; // invalidate data
                    backward_is_too_far = false; // invalidate data
                    continue;
                } else {
                    break;
                }
            }
            Self::smooth_outward_step(
                poly,
                p1,
                shortcut_length2,
                p0_it,
                p2_it,
                &mut forward_is_blocked,
                &mut backward_is_blocked,
                &mut forward_is_too_far,
                &mut backward_is_too_far,
            );
            if poly.prev(*p0_it) == *p2_it || *p0_it == *p2_it {
                // stop if we went all the way around the polygon
                // this should only be the case for hole polygons (?)
                if forward_is_too_far && backward_is_too_far {
                    // in case p0_it.prev() == p2_it :
                    //     -                                                       .
                    //    /   \                                                    .
                    //    |   |
                    //    \   /                                                    .
                    //     | |
                    //     o o
                    // in case p0_it == p2_it :
                    //      -                                                       .
                    //     / \                                                      .
                    //    | |
                    //     |
                    break;
                } else {
                    // this whole polygon can be removed
                    return true;
                }
            }
        }

        let v02 = poly.p(*p2_it) - poly.p(*p0_it);
        let v02_size2 = v_size2(v02);
        // set the following:
        // p0_it = start point of line
        // p2_it = end point of line
        if (v02_size2 - shortcut_length2).abs() < shortcut_length * 10 {
            // i.e. if (size2 < l * (l+10) && size2 > l * (l-10))
            // v02 is approximately shortcut length
            // handle this separately to avoid rounding problems below in the getPointOnLineWithDist function
            // p0_it and p2_it are already correct
        } else if !backward_is_blocked && !forward_is_blocked {
            // introduce two new points
            //  1----b---->2
            //  ^   /
            //  |  /
            //  | /
            //  |/
            //  |a
            //  |
            //  0
            let v02_size = (v02_size2 as f64).sqrt() as i64;

            let p0_2_it = poly.prev(*p0_it);
            let p2_2_it = poly.next(*p2_it);
            let p2_2 = poly.p(p2_2_it);
            let p0_2 = poly.p(p0_2_it);
            let v02_2 = p0_2 - p2_2;
            let v02_2_size = v_size(v02_2);
            let progress = (1.0f64).min(int2mm(shortcut_length - v02_size) / int2mm(v02_2_size - v02_size)); // account for rounding error when v02_2_size is approx equal to v02_size
            let new_p0 = poly.p(*p0_it) + scale_f64(p0_2 - poly.p(*p0_it), progress);
            *p0_it = insert_point_non_duplicate(poly, p0_2_it, *p0_it, new_p0);
            let new_p2 = poly.p(*p2_it) + scale_f64(p2_2 - poly.p(*p2_it), progress);
            *p2_it = insert_point_non_duplicate(poly, *p2_it, p2_2_it, new_p2);
        } else if !backward_is_blocked {
            // forward is blocked, back is open
            //     |
            //  1->b
            //  ^  :
            //  | /
            //  0 :
            //  ^ :
            //  |/
            //  |a
            //  |
            //  0_2
            let p0_2_it = poly.prev(*p0_it);
            let p0 = poly.p(*p0_it);
            let p0_2 = poly.p(p0_2_it);
            let p2 = poly.p(*p2_it);
            if let Some(new_p0) = la::get_point_on_line_with_dist(p2, p0, p0_2, shortcut_length) {
                *p0_it = insert_point_non_duplicate(poly, p0_2_it, *p0_it, new_p0);
            } else {
                // shortcut cannot be of the desired length
                // ignore the area => just shortcut from p0 to p2
                if v_size(p2 - p0_2) < v_size2(p2 - p0) {
                    *p0_it = p0_2_it;
                }
            }
        } else if !forward_is_blocked {
            // backward is blocked, front is open
            //  1----2----b----------->2_2
            //  ^      ,-'
            //  |   ,-'
            //--a-'
            //  |
            //  0
            let p2_2_it = poly.next(*p2_it);
            let p0 = poly.p(*p0_it);
            let p2 = poly.p(*p2_it);
            let p2_2 = poly.p(p2_2_it);
            if let Some(new_p2) = la::get_point_on_line_with_dist(p0, p2, p2_2, shortcut_length) {
                *p2_it = insert_point_non_duplicate(poly, *p2_it, p2_2_it, new_p2);
            } else {
                // shortcut cannot be of the desired length
                // ignore the area => just shortcut from p0 to p2
                if v_size(p2_2 - p0) < v_size2(p2 - p0) {
                    *p2_it = p2_2_it;
                }
            }
        } else {
            // both are blocked
            //  1----2->b
            //  ^  ,-'
            //  | /
            //  |/
            //  a
            //  |
            //  0
            //
            // just shortcut from p0 to p2
        }
        while poly.next(*p0_it) != *p2_it {
            let n = poly.next(*p0_it);
            poly.remove(n);
        }
        false
    }
}

/// `PointsSet`: a plain list of points (used e.g. as a convex hull container).
pub type PointsSet = Vec<Point2LL>;

/// Helper: `AngleRadians` from the C++ default argument `AngleRadians(0.0005)`.
pub fn default_colinear_angle() -> AngleRadians {
    AngleRadians::new(0.0005)
}
