//! `geometry/LinesSet.h`, `Shape.h`, `SingleShape.h`, `PartsView.h`.

use super::point2ll::*;
use super::polyline::*;
use super::matrix::{Point3Matrix, PointMatrix};
use crate::settings::types::{AngleDegrees, AngleRadians};
use crate::utils::coord::Coord;
use crate::utils::linear_alg_2d as la;
use crate::utils::polyline_stitcher;
use clipper::{ClipType, Clipper, ClipperOffset, EndType, JoinType, Paths, PolyFillType, PolyTree, PolyType};
use std::collections::HashSet;
use std::ops::{Deref, DerefMut};

pub const NO_INDEX: usize = usize::MAX;

/// `CheckNonEmptyParam`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckNonEmptyParam {
    OnlyIfValid,
    OnlyIfNotEmpty,
    EvenIfEmpty,
}

/// A set of lines of the same type (`LinesSet<LineType>`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinesSet<L> {
    pub lines: Vec<L>,
}

impl<L> Default for LinesSet<L> {
    fn default() -> Self {
        LinesSet { lines: Vec::new() }
    }
}

impl<L> Deref for LinesSet<L> {
    type Target = Vec<L>;
    #[inline]
    fn deref(&self) -> &Vec<L> {
        &self.lines
    }
}

impl<L> DerefMut for LinesSet<L> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Vec<L> {
        &mut self.lines
    }
}

impl<'a, L> IntoIterator for &'a LinesSet<L> {
    type Item = &'a L;
    type IntoIter = std::slice::Iter<'a, L>;
    fn into_iter(self) -> Self::IntoIter {
        self.lines.iter()
    }
}

impl<'a, L> IntoIterator for &'a mut LinesSet<L> {
    type Item = &'a mut L;
    type IntoIter = std::slice::IterMut<'a, L>;
    fn into_iter(self) -> Self::IntoIter {
        self.lines.iter_mut()
    }
}

impl<L> IntoIterator for LinesSet<L> {
    type Item = L;
    type IntoIter = std::vec::IntoIter<L>;
    fn into_iter(self) -> Self::IntoIter {
        self.lines.into_iter()
    }
}

impl<L> FromIterator<L> for LinesSet<L> {
    fn from_iter<I: IntoIterator<Item = L>>(iter: I) -> Self {
        LinesSet { lines: iter.into_iter().collect() }
    }
}

pub type OpenLinesSet = LinesSet<OpenPolyline>;
pub type ClosedLinesSet = LinesSet<ClosedPolyline>;
/// A set of polygons, i.e. an area with holes (`cura::Shape`).
pub type Shape = LinesSet<Polygon>;

impl<L: Polyline> LinesSet<L> {
    pub fn new() -> Self {
        LinesSet { lines: Vec::new() }
    }

    pub fn from_lines(lines: Vec<L>) -> Self {
        LinesSet { lines }
    }

    pub fn from_line(line: L) -> Self {
        LinesSet { lines: vec![line] }
    }

    /// `LinesSet(ClipperLib::Paths&&)` / `Shape(ClipperLib::Paths&&)`: lines are not explicitly closed.
    pub fn from_paths(paths: Paths) -> Self {
        LinesSet { lines: paths.into_iter().map(L::from_path).collect() }
    }

    pub fn get_lines(&self) -> &Vec<L> {
        &self.lines
    }

    pub fn get_lines_mut(&mut self) -> &mut Vec<L> {
        &mut self.lines
    }

    pub fn set_lines(&mut self, lines: Vec<L>) {
        self.lines = lines;
    }

    fn check_add(line: &L, check_non_empty: CheckNonEmptyParam) -> bool {
        match check_non_empty {
            CheckNonEmptyParam::EvenIfEmpty => true,
            CheckNonEmptyParam::OnlyIfNotEmpty => !line.is_empty(),
            CheckNonEmptyParam::OnlyIfValid => line.is_valid(),
        }
    }

    /// `push_back(line, check_non_empty)`.
    pub fn push_checked(&mut self, line: L, check_non_empty: CheckNonEmptyParam) {
        if Self::check_add(&line, check_non_empty) {
            self.lines.push(line);
        }
    }

    /// `push_back(const LinesSet& other)`.
    pub fn push_all(&mut self, other: &LinesSet<L>) {
        self.lines.extend_from_slice(&other.lines);
    }

    /// `push_back(LinesSet&& other)`.
    pub fn push_all_owned(&mut self, other: LinesSet<L>) {
        self.lines.extend(other.lines);
    }

    /// `newLine()`.
    pub fn new_line(&mut self) -> &mut L {
        self.lines.push(L::default());
        self.lines.last_mut().unwrap()
    }

    /// `emplace_back(path)` for a raw path.
    pub fn emplace_back_path(&mut self, path: Path) {
        self.lines.push(L::from_path(path));
    }

    pub fn point_count(&self) -> usize {
        self.lines.iter().map(|l| l.len()).sum()
    }

    /// `removeAt(index)`: swap with the last element and remove.
    pub fn remove_at(&mut self, index: usize) {
        if self.lines.len() == 1 {
            self.lines.clear();
        } else if self.lines.len() > 1 {
            debug_assert!(index < self.lines.len());
            self.lines.swap_remove(index);
        }
    }

    pub fn split_into_segments_into(&self, result: &mut OpenLinesSet) {
        for line in &self.lines {
            line.split_into_segments_into(result);
        }
    }

    pub fn split_into_segments(&self) -> OpenLinesSet {
        let mut result = OpenLinesSet::default();
        for line in &self.lines {
            line.split_into_segments_into(&mut result);
        }
        result
    }

    /// `length()` (accumulated as `long long`).
    pub fn length(&self) -> Coord {
        self.lines.iter().fold(0i64, |total, line| total + line.length())
    }

    pub fn translate(&mut self, delta: Point2LL) {
        if delta.x != 0 || delta.y != 0 {
            for line in self.lines.iter_mut() {
                line.translate(delta);
            }
        }
    }

    pub fn apply_matrix(&mut self, matrix: &PointMatrix) {
        for line in self.lines.iter_mut() {
            line.apply_matrix(matrix);
        }
    }

    pub fn apply_matrix3(&mut self, matrix: &Point3Matrix) {
        for line in self.lines.iter_mut() {
            line.apply_matrix3(matrix);
        }
    }

    /// `addPaths(Clipper&, PolyType)`: polygons are added closed, other line types open.
    pub fn add_paths(&self, clipper: &mut Clipper, poly_typ: PolyType) {
        for line in &self.lines {
            clipper.add_path(line, poly_typ, L::IS_POLYGON);
        }
    }

    /// `addPaths(ClipperOffset&, JoinType, EndType)`.
    pub fn add_paths_offset(&self, clipper: &mut ClipperOffset, join_type: JoinType, end_type: EndType) {
        for line in &self.lines {
            clipper.add_path(line, join_type, end_type);
        }
    }

    /// `removeDegenerateVerts()`.
    pub fn remove_degenerate_verts(&mut self) {
        let mut poly_idx: usize = 0;
        while poly_idx < self.lines.len() {
            let for_polyline = L::IS_OPEN;
            let poly = &self.lines[poly_idx];
            let mut result: Path = Vec::new();

            let is_degenerate = |last: Point2LL, now: Point2LL, next: Point2LL| -> bool {
                let last_line = now - last;
                let next_line = next - now;
                dot(last_line, next_line) == -v_size(last_line) * v_size(next_line)
            };

            // With polylines, skip the first and last vertex.
            let start_vertex = if for_polyline { 1 } else { 0 };
            let end_vertex = if for_polyline { poly.len().wrapping_sub(1) } else { poly.len() };
            for i in 0..start_vertex.min(poly.len()) {
                result.push(poly[i]); // Add everything before the start vertex.
            }

            let mut is_changed = false;
            let mut idx = start_vertex;
            while idx < end_vertex {
                let last = if result.is_empty() { *poly.last().unwrap() } else { *result.last().unwrap() };
                if idx + 1 >= poly.len() && result.is_empty() {
                    break;
                }
                let next = if idx + 1 >= poly.len() { result[0] } else { poly[idx + 1] };
                if is_degenerate(last, poly[idx], next) {
                    // lines are in the opposite direction
                    // don't add vert to the result
                    is_changed = true;
                    while result.len() > 1 && is_degenerate(result[result.len() - 2], *result.last().unwrap(), next) {
                        result.pop();
                    }
                } else {
                    result.push(poly[idx]);
                }
                idx += 1;
            }

            let mut i = end_vertex;
            while i < poly.len() {
                result.push(poly[i]); // Add everything after the end vertex.
                i += 1;
            }

            if is_changed {
                if for_polyline || result.len() > 2 {
                    self.lines[poly_idx].set_points(result);
                } else {
                    self.remove_at(poly_idx);
                    poly_idx = poly_idx.wrapping_sub(1); // effectively the next iteration has the same poly_idx (referring to a new poly which is not yet processed)
                }
            }
            poly_idx = poly_idx.wrapping_add(1);
        }
    }

    /// `createTubeShape(inner_offset, outer_offset)`.
    pub fn create_tube_shape(&self, inner_offset: Coord, outer_offset: Coord) -> Shape
    where
        Self: Offsettable,
    {
        self.offset_lines(outer_offset, JoinType::Miter, 1.2).difference(&self.offset_lines(-inner_offset, JoinType::Miter, 1.2))
    }
}

/// Dispatch of `LinesSet<T>::offset` to the right specialisation.
pub trait Offsettable {
    fn offset_lines(&self, distance: Coord, join_type: JoinType, miter_limit: f64) -> Shape;
}

impl Offsettable for OpenLinesSet {
    fn offset_lines(&self, distance: Coord, join_type: JoinType, miter_limit: f64) -> Shape {
        self.offset(distance, join_type, miter_limit)
    }
}
impl Offsettable for ClosedLinesSet {
    fn offset_lines(&self, distance: Coord, join_type: JoinType, miter_limit: f64) -> Shape {
        self.offset(distance, join_type, miter_limit)
    }
}
impl Offsettable for Shape {
    fn offset_lines(&self, distance: Coord, join_type: JoinType, miter_limit: f64) -> Shape {
        self.offset(distance, join_type, miter_limit)
    }
}

impl OpenLinesSet {
    pub fn add_segment(&mut self, from: Point2LL, to: Point2LL) {
        self.lines.push(OpenPolyline::new(vec![from, to]));
    }

    /// `OpenLinesSet::offset`: open lines get square (miter) or round ends.
    pub fn offset(&self, distance: Coord, join_type: JoinType, miter_limit: f64) -> Shape {
        if self.is_empty() || distance == 0 {
            return Shape::default();
        }
        let mut clipper = ClipperOffset::new(miter_limit, 10.0);
        let end_type = if join_type == JoinType::Miter { EndType::OpenSquare } else { EndType::OpenRound };
        self.add_paths_offset(&mut clipper, join_type, end_type);
        clipper.miter_limit = miter_limit;
        let mut result_paths = Vec::new();
        clipper.execute(&mut result_paths, distance as f64);
        Shape::from_paths(result_paths)
    }

    pub fn offset_default(&self, distance: Coord) -> Shape {
        self.offset(distance, JoinType::Miter, 1.2)
    }

    /// `OpenLinesSet::difference(const Shape&)`.
    pub fn difference(&self, other: &Shape) -> OpenLinesSet {
        if self.is_empty() {
            return OpenLinesSet::default();
        }
        if other.is_empty() {
            return self.clone();
        }
        let mut ret = PolyTree::new();
        let mut clipper = Clipper::new(0);
        self.add_paths(&mut clipper, PolyType::Subject);
        other.add_paths(&mut clipper, PolyType::Clip);
        clipper.execute_tree(ClipType::Difference, &mut ret, PolyFillType::EvenOdd, PolyFillType::EvenOdd);
        OpenLinesSet::from_paths(clipper::open_paths_from_poly_tree(&ret))
    }
}

impl ClosedLinesSet {
    /// `ClosedLinesSet::offset`: closed lines are offset as `etClosedLine`.
    pub fn offset(&self, distance: Coord, join_type: JoinType, miter_limit: f64) -> Shape {
        if self.is_empty() {
            return Shape::default();
        }
        if distance == 0 {
            let mut result = Shape::default();
            for line in &self.lines {
                result.push(Polygon::new(line.points.clone(), line.explicitly_closed));
            }
            return result;
        }
        let mut ret = Vec::new();
        let mut clipper = ClipperOffset::new(miter_limit, 10.0);
        self.add_paths_offset(&mut clipper, join_type, EndType::ClosedLine);
        clipper.miter_limit = miter_limit;
        clipper.execute(&mut ret, distance as f64);
        Shape::from_paths(ret)
    }
}

/// Visit the children of a PolyTree node (`None` = root).
fn tree_childs(tree: &PolyTree, node: Option<usize>) -> Vec<usize> {
    match node {
        None => tree.childs.clone(),
        Some(n) => tree.all_nodes[n].childs.clone(),
    }
}

impl Shape {
    /// `Shape(const Polygon&)`.
    pub fn from_polygon(polygon: Polygon) -> Shape {
        LinesSet { lines: vec![polygon] }
    }

    /// `emplace_back(ClipperLib::Paths&&, explicitely_closed = false)`.
    pub fn emplace_back_paths(&mut self, paths: Paths) {
        self.reserve(paths.len());
        for p in paths {
            self.push(Polygon::new(p, false));
        }
    }

    pub fn approx_convex_hull(&self, extra_outset: i32) -> Shape {
        const OVERSHOOT: i32 = 100_000; // MM2INT(100) - 10cm (hard-coded value).

        let mut convex_hull = Shape::default();
        // Perform the offset for each polygon one at a time.
        // This is necessary because the polygons may overlap, in which case the offset could end up in an infinite loop.
        // See http://www.angusj.com/delphi/clipper/documentation/Docs/Units/ClipperLib/Classes/ClipperOffset/_Body.htm
        for polygon in &self.lines {
            let mut offset_result = Vec::new();
            let mut offsetter = ClipperOffset::new(1.2, 10.0);
            offsetter.add_path(polygon, JoinType::Round, EndType::ClosedPolygon);
            offsetter.execute(&mut offset_result, OVERSHOOT as f64);
            convex_hull.emplace_back_paths(offset_result);
        }

        convex_hull.union_polygons().offset((-OVERSHOOT + extra_outset) as Coord, JoinType::Round, 1.2)
    }

    pub fn make_convex(&mut self) {
        // early out if there is nothing to do
        if self.is_empty() {
            return;
        }

        // Andrew’s Monotone Chain Convex Hull Algorithm
        let mut points: Vec<Point2LL> = Vec::new();
        for poly in &self.lines {
            points.extend_from_slice(poly);
        }

        let mut convexified = Polygon::default();

        let make_sorted_poly_convex = |poly: &Vec<Point2LL>, convexified: &mut Polygon| {
            convexified.push(poly[0]);

            for window in poly.windows(2) {
                let current = window[0];
                let after = window[1];

                if la::point_is_left_of_line(current, *convexified.last().unwrap(), after) < 0 {
                    // Track backwards to make sure we haven't been in a concave pocket for multiple vertices already.
                    while convexified.len() >= 2
                        && (la::point_is_left_of_line(*convexified.last().unwrap(), convexified[convexified.len() - 2], current) >= 0
                            || la::point_is_left_of_line(*convexified.last().unwrap(), convexified[convexified.len() - 2], convexified[0]) > 0)
                    {
                        convexified.pop();
                    }
                    convexified.push(current);
                }
            }
        };

        points.sort_by(|a, b| if a.x == b.x { a.y.cmp(&b.y) } else { a.x.cmp(&b.x) });
        make_sorted_poly_convex(&points, &mut convexified);
        points.reverse();
        make_sorted_poly_convex(&points, &mut convexified);

        self.lines = vec![convexified];
    }

    pub fn difference(&self, other: &Shape) -> Shape {
        if self.is_empty() {
            return Shape::default();
        }
        if other.is_empty() {
            return self.clone();
        }
        let mut ret = Vec::new();
        let mut clipper = Clipper::new(0);
        self.add_paths(&mut clipper, PolyType::Subject);
        other.add_paths(&mut clipper, PolyType::Clip);
        clipper.execute(ClipType::Difference, &mut ret, PolyFillType::EvenOdd, PolyFillType::EvenOdd);
        Shape::from_paths(ret)
    }

    pub fn difference_polygon(&self, other: &Polygon) -> Shape {
        if self.is_empty() {
            return Shape::default();
        }
        if other.is_empty() {
            return self.clone();
        }
        let mut ret = Vec::new();
        let mut clipper = Clipper::new(0);
        self.add_paths(&mut clipper, PolyType::Subject);
        clipper.add_path(other, PolyType::Clip, true);
        clipper.execute(ClipType::Difference, &mut ret, PolyFillType::EvenOdd, PolyFillType::EvenOdd);
        Shape::from_paths(ret)
    }

    pub fn union_polygons_with(&self, other: &Shape, fill_type: PolyFillType) -> Shape {
        if self.is_empty() && other.is_empty() {
            return Shape::default();
        }
        let mut ret = Vec::new();
        let mut clipper = Clipper::new(0);
        self.add_paths(&mut clipper, PolyType::Subject);
        other.add_paths(&mut clipper, PolyType::Subject);
        clipper.execute(ClipType::Union, &mut ret, fill_type, fill_type);
        Shape::from_paths(ret)
    }

    pub fn union_polygons_other(&self, other: &Shape) -> Shape {
        self.union_polygons_with(other, PolyFillType::NonZero)
    }

    pub fn union_polygon_with(&self, polygon: &Polygon, fill_type: PolyFillType) -> Shape {
        if self.is_empty() && polygon.is_empty() {
            return Shape::default();
        }
        let mut ret = Vec::new();
        let mut clipper = Clipper::new(0);
        self.add_paths(&mut clipper, PolyType::Subject);
        clipper.add_path(polygon, PolyType::Subject, true);
        clipper.execute(ClipType::Union, &mut ret, fill_type, fill_type);
        Shape::from_paths(ret)
    }

    /// `unionPolygons()`: union of all polygons in this shape (non-zero fill).
    pub fn union_polygons(&self) -> Shape {
        self.union_polygons_with(&Shape::default(), PolyFillType::NonZero)
    }

    pub fn intersection(&self, other: &Shape) -> Shape {
        if self.is_empty() || other.is_empty() {
            return Shape::default();
        }
        let mut ret = Vec::new();
        let mut clipper = Clipper::new(0);
        self.add_paths(&mut clipper, PolyType::Subject);
        other.add_paths(&mut clipper, PolyType::Clip);
        clipper.execute(ClipType::Intersection, &mut ret, PolyFillType::EvenOdd, PolyFillType::EvenOdd);
        Shape::from_paths(ret)
    }

    /// `Shape::offset(distance, join_type = jtMiter, miter_limit = 1.2)`.
    pub fn offset(&self, distance: Coord, join_type: JoinType, miter_limit: f64) -> Shape {
        if self.is_empty() {
            return Shape::default();
        }
        if distance == 0 {
            return self.clone();
        }
        let mut ret = Vec::new();
        let mut clipper = ClipperOffset::new(miter_limit, 10.0);
        self.union_polygons().add_paths_offset(&mut clipper, join_type, EndType::ClosedPolygon);
        clipper.miter_limit = miter_limit;
        clipper.execute(&mut ret, distance as f64);
        Shape::from_paths(ret)
    }

    /// `offset(distance)` with the default miter join.
    pub fn offset_default(&self, distance: Coord) -> Shape {
        self.offset(distance, JoinType::Miter, 1.2)
    }

    /// `offset(distance, join_type)` with the default miter limit.
    pub fn offset_join(&self, distance: Coord, join_type: JoinType) -> Shape {
        self.offset(distance, join_type, 1.2)
    }

    /// `Shape::intersection(const LinesSet<LineType>& polylines, restitch, max_stitch_distance, split_into_segments)`.
    pub fn intersection_polylines<L: Polyline>(
        &self,
        polylines: &LinesSet<L>,
        restitch: bool,
        max_stitch_distance: Coord,
        split_into_segments: bool,
    ) -> OpenLinesSet {
        if self.is_empty() || polylines.is_empty() {
            return OpenLinesSet::default();
        }

        let mut result = PolyTree::new();
        let mut clipper = Clipper::new(0);

        if split_into_segments {
            polylines.split_into_segments().add_paths(&mut clipper, PolyType::Subject);
        } else {
            polylines.add_paths(&mut clipper, PolyType::Subject);
        }
        self.add_paths(&mut clipper, PolyType::Clip);
        clipper.execute_tree(ClipType::Intersection, &mut result, PolyFillType::EvenOdd, PolyFillType::EvenOdd);
        let result_paths = clipper::open_paths_from_poly_tree(&result);

        let mut result_lines = OpenLinesSet::from_paths(result_paths);

        if restitch {
            let mut result_open_lines = OpenLinesSet::default();
            let mut result_closed_lines = Shape::default();

            const SNAP_DISTANCE: Coord = 10;
            polyline_stitcher::stitch_open_into_shape(&result_lines, &mut result_open_lines, &mut result_closed_lines, max_stitch_distance, SNAP_DISTANCE);

            result_lines = result_open_lines;
            // if open polylines got stitched into closed polylines, split them back up into open polylines again, because the result only admits open polylines
            for closed_line in result_closed_lines.lines.iter_mut() {
                if !closed_line.is_empty() {
                    if closed_line.len() > 2 {
                        let f = closed_line[0];
                        closed_line.push(f);
                    }
                    result_lines.push(OpenPolyline::new(std::mem::take(&mut closed_line.points)));
                }
            }
        }

        result_lines
    }

    pub fn xor_polygons(&self, other: &Shape, pft: PolyFillType) -> Shape {
        if self.is_empty() {
            return other.clone();
        }
        if other.is_empty() {
            return self.clone();
        }
        let mut ret = Vec::new();
        let mut clipper = Clipper::new(0);
        self.add_paths(&mut clipper, PolyType::Subject);
        other.add_paths(&mut clipper, PolyType::Clip);
        clipper.execute(ClipType::Xor, &mut ret, pft, pft);
        Shape::from_paths(ret)
    }

    pub fn execute(&self, pft: PolyFillType) -> Shape {
        let mut ret = Vec::new();
        let mut clipper = Clipper::new(0);
        self.add_paths(&mut clipper, PolyType::Subject);
        clipper.execute(ClipType::Xor, &mut ret, pft, pft);
        Shape::from_paths(ret)
    }

    pub fn inside(&self, p: Point2LL, border_result: bool) -> bool {
        let mut poly_count_inside = 0;
        for poly in &self.lines {
            let is_inside_this_poly = clipper::point_in_polygon(p, poly);
            if is_inside_this_poly == -1 {
                return border_result;
            }
            poly_count_inside += is_inside_this_poly;
        }
        (poly_count_inside % 2) == 1
    }

    pub fn find_inside(&self, p: Point2LL, border_result: bool) -> usize {
        if self.is_empty() {
            return NO_INDEX;
        }

        // NOTE: Keep these vectors fixed-size, they replace an (non-standard, sized at runtime) arrays.
        let mut min_x: Vec<i64> = vec![i64::MAX; self.len()];
        let mut crossings: Vec<i64> = vec![0; self.len()];

        for poly_idx in 0..self.len() {
            let poly = &self[poly_idx];
            let mut p0 = *poly.last().unwrap();
            for &p1 in poly.iter() {
                let comp = la::point_lies_on_the_right_of_line(p, p0, p1);
                if comp == 1 {
                    crossings[poly_idx] += 1;
                    let x;
                    if p1.y == p0.y {
                        x = p0.x;
                    } else {
                        x = p0.x + (p1.x - p0.x) * (p.y - p0.y) / (p1.y - p0.y);
                    }
                    if x < min_x[poly_idx] {
                        min_x[poly_idx] = x;
                    }
                } else if border_result && comp == 0 {
                    return poly_idx;
                }
                p0 = p1;
            }
        }

        let mut min_x_uneven = i64::MAX;
        let mut ret = NO_INDEX;
        let mut n_unevens = 0usize;
        for array_idx in 0..self.len() {
            if crossings[array_idx] % 2 == 1 {
                n_unevens += 1;
                if min_x[array_idx] < min_x_uneven {
                    min_x_uneven = min_x[array_idx];
                    ret = array_idx;
                }
            }
        }
        if n_unevens % 2 == 0 {
            ret = NO_INDEX;
        }
        ret
    }

    pub fn area(&self) -> f64 {
        self.lines.iter().fold(0.0, |total, poly| total + poly.area())
    }

    pub fn smooth(&self, remove_length: i32) -> Shape {
        let mut ret = Shape::default();
        for poly in &self.lines {
            if poly.len() < 3 {
                continue;
            }
            if poly.len() == 3 {
                ret.push(poly.clone());
                continue;
            }
            let mut out = Polygon::default();
            poly.smooth(remove_length, &mut out);
            ret.push(out);
            let back = ret.last_mut().unwrap();
            if back.len() < 3 {
                let n = back.len();
                back.points.truncate(n.saturating_sub(1));
            }
        }
        ret
    }

    pub fn smooth_outward(&self, max_angle: AngleDegrees, shortcut_length: i32) -> Shape {
        let mut ret = Shape::default();
        for poly in &self.lines {
            if poly.len() < 3 {
                continue;
            }
            if poly.len() == 3 {
                ret.push(poly.clone());
                continue;
            }
            let mut out = Polygon::default();
            poly.smooth_outward(max_angle, shortcut_length, &mut out);
            ret.push(out);
            if ret.last().unwrap().len() < 3 {
                let n = ret.len();
                ret.truncate(n - 1);
            }
        }
        ret
    }

    pub fn smooth2(&self, remove_length: i32, min_area: i32) -> Shape {
        let mut ret = Shape::default();
        for poly in &self.lines {
            if poly.is_empty() {
                continue;
            }
            if poly.area() < min_area as f64 || poly.len() <= 5 {
                // when optimally removing, a poly with 5 pieces results in a triangle. Smaller polys dont have area!
                ret.push(poly.clone());
                continue;
            }
            if poly.len() < 4 {
                ret.push(poly.clone());
            } else {
                let mut out = Polygon::default();
                poly.smooth2(remove_length, &mut out);
                ret.push(out);
            }
        }
        ret
    }

    pub fn remove_colinear_edges(&mut self, max_deviation_angle: AngleRadians) {
        let mut p: usize = 0;
        while p < self.len() {
            self.lines[p].remove_colinear_edges(max_deviation_angle);
            if self.lines[p].len() < 3 {
                self.remove_at(p);
                p = p.wrapping_sub(1);
            }
            p = p.wrapping_add(1);
        }
    }

    pub fn get_outside_polygons(&self) -> Shape {
        if self.is_empty() {
            return Shape::default();
        }
        if self.len() == 1 {
            return self.clone();
        }

        let mut ret = Shape::default();
        let mut clipper = Clipper::new(0);
        let mut poly_tree = PolyTree::new();
        self.add_paths(&mut clipper, PolyType::Subject);
        clipper.execute_tree(ClipType::Union, &mut poly_tree, PolyFillType::EvenOdd, PolyFillType::EvenOdd);

        for &child in &poly_tree.childs {
            ret.push(Polygon::new(std::mem::take(&mut poly_tree.all_nodes[child].contour), false));
        }
        ret
    }

    /// `splitIntoParts(union_all = false)`.
    pub fn split_into_parts(&self, union_all: bool) -> Vec<SingleShape> {
        let mut ret = Vec::new();
        let mut clipper = Clipper::new(0);
        let mut result_poly_tree = PolyTree::new();
        self.add_paths(&mut clipper, PolyType::Subject);
        if union_all {
            clipper.execute_tree(ClipType::Union, &mut result_poly_tree, PolyFillType::NonZero, PolyFillType::NonZero);
        } else {
            clipper.execute_tree(ClipType::Union, &mut result_poly_tree, PolyFillType::EvenOdd, PolyFillType::EvenOdd);
        }

        Self::split_into_parts_process_poly_tree_node(&mut result_poly_tree, None, &mut ret);
        ret
    }

    fn split_into_parts_process_poly_tree_node(tree: &mut PolyTree, node: Option<usize>, ret: &mut Vec<SingleShape>) {
        for child in tree_childs(tree, node) {
            let mut part = SingleShape::default();
            part.push(Polygon::new(std::mem::take(&mut tree.all_nodes[child].contour), false));
            for i in tree_childs(tree, Some(child)) {
                part.push(Polygon::new(std::mem::take(&mut tree.all_nodes[i].contour), false));
                Self::split_into_parts_process_poly_tree_node(tree, Some(i), ret);
            }
            ret.push(part);
        }
    }

    pub fn sort_by_nesting(&self) -> Vec<Shape> {
        let mut ret: Vec<Shape> = Vec::new();
        let mut clipper = Clipper::new(0);
        let mut result_poly_tree = PolyTree::new();
        self.add_paths(&mut clipper, PolyType::Subject);
        clipper.execute_tree(ClipType::Union, &mut result_poly_tree, PolyFillType::EvenOdd, PolyFillType::EvenOdd);

        Self::sort_by_nesting_process_poly_tree_node(&mut result_poly_tree, None, 0, &mut ret);
        ret
    }

    fn sort_by_nesting_process_poly_tree_node(tree: &mut PolyTree, node: Option<usize>, nesting_idx: usize, ret: &mut Vec<Shape>) {
        for child in tree_childs(tree, node) {
            if nesting_idx >= ret.len() {
                ret.resize_with(nesting_idx + 1, Shape::default);
            }
            ret[nesting_idx].push(Polygon::new(std::mem::take(&mut tree.all_nodes[child].contour), false));
            Self::sort_by_nesting_process_poly_tree_node(tree, Some(child), nesting_idx + 1, ret);
        }
    }

    /// `splitIntoPartsView(union_all = false)`: reorders this shape and returns the part indices.
    pub fn split_into_parts_view(&mut self, union_all: bool) -> PartsView {
        let mut reordered = Shape::default();
        let mut parts_view = PartsView::default();

        let mut clipper = Clipper::new(0);
        let mut result_poly_tree = PolyTree::new();
        self.add_paths(&mut clipper, PolyType::Subject);
        if union_all {
            clipper.execute_tree(ClipType::Union, &mut result_poly_tree, PolyFillType::NonZero, PolyFillType::NonZero);
        } else {
            clipper.execute_tree(ClipType::Union, &mut result_poly_tree, PolyFillType::EvenOdd, PolyFillType::EvenOdd);
        }

        Self::split_into_parts_view_process_poly_tree_node(&mut parts_view, &mut reordered, &mut result_poly_tree, None);

        *self = reordered;
        parts_view
    }

    fn split_into_parts_view_process_poly_tree_node(parts_view: &mut PartsView, reordered: &mut Shape, tree: &mut PolyTree, node: Option<usize>) {
        for child in tree_childs(tree, node) {
            parts_view.parts.push(Vec::new());
            let pos = parts_view.parts.len() - 1;
            parts_view.parts[pos].push(reordered.len());
            reordered.push(Polygon::new(std::mem::take(&mut tree.all_nodes[child].contour), false));
            for i in tree_childs(tree, Some(child)) {
                parts_view.parts[pos].push(reordered.len());
                reordered.push(Polygon::new(std::mem::take(&mut tree.all_nodes[i].contour), false));
                Self::split_into_parts_view_process_poly_tree_node(parts_view, reordered, tree, Some(i));
            }
        }
    }

    pub fn remove_small_areas(&mut self, min_area_size: f64, remove_holes: bool) {
        let mut new_end = self.len();
        if remove_holes {
            let mut it = 0;
            while it < new_end {
                // All polygons smaller than target are removed by replacing them with a polygon from the back of the vector.
                if (self[it].area() / 1_000_000.0).abs() < min_area_size {
                    new_end -= 1;
                    let moved = std::mem::take(&mut self.lines[new_end]);
                    self.lines[it] = moved;
                    continue; // Don't increment the iterator such that the polygon just swapped in is checked next.
                }
                it += 1;
            }
        } else {
            // For each polygon, computes the signed area, move small outlines to the end of the vector and keep a reference to small holes
            let mut small_holes: Vec<usize> = Vec::new();
            let mut it = 0;
            while it < new_end {
                let area = self[it].area() / 1_000_000.0;
                if area.abs() < min_area_size {
                    if area >= 0.0 {
                        new_end -= 1;
                        if it < new_end {
                            self.lines.swap(new_end, it);
                            continue; // Don't increment the iterator such that the polygon just swapped in is checked next.
                        }
                        // Don't run off the end if the last polygon is the one to be removed
                        break;
                    }
                    small_holes.push(it);
                }
                it += 1;
            }
            // Removes small holes that have their outline removed
            let removed_outlines_start = new_end;
            let end = self.len();
            for &hole_it in small_holes.iter().rev() {
                for outline_it in removed_outlines_start..end {
                    let hole_front = self.lines[hole_it][0];
                    if self.lines[outline_it].inside(hole_front, false) {
                        new_end -= 1;
                        if hole_it == new_end {
                            // self-move-assignment leaves the vector empty in libstdc++; Microsoft's
                            // vector checks for self-assignment and keeps it.
                            if !cppstd::flavor::msvc() {
                                self.lines[hole_it].points.clear();
                            }
                        } else {
                            let moved = std::mem::take(&mut self.lines[new_end]);
                            self.lines[hole_it] = moved;
                        }
                        break;
                    }
                }
            }
        }
        self.lines.truncate(new_end);
    }

    pub fn remove_polygon(&self, to_be_removed: &Shape, same_distance: i32) -> Shape {
        let mut result = Shape::default();
        let same2 = (same_distance.wrapping_mul(same_distance)) as Coord;
        for poly_keep_idx in 0..self.len() {
            let poly_keep = &self[poly_keep_idx];
            let mut should_be_removed = false;
            if !poly_keep.is_empty() {
                for poly_rem in &to_be_removed.lines {
                    if poly_rem.len() != poly_keep.len() || poly_rem.is_empty() {
                        continue;
                    }

                    // find closest point, supposing this point aligns the two shapes in the best way
                    let mut closest_point_idx = 0usize;
                    let mut smallest_dist2: Coord = -1;
                    for point_rem_idx in 0..poly_rem.len() {
                        let dist2 = v_size2(poly_rem[point_rem_idx] - poly_keep[0]);
                        if dist2 < smallest_dist2 || smallest_dist2 < 0 {
                            smallest_dist2 = dist2;
                            closest_point_idx = point_rem_idx;
                        }
                    }
                    let mut poly_rem_is_poly_keep = true;
                    // compare the two polygons on all points
                    if smallest_dist2 > same2 {
                        continue;
                    }
                    for point_idx in 0..poly_rem.len() {
                        let dist2 = v_size2(poly_rem[(closest_point_idx + point_idx) % poly_rem.len()] - poly_keep[point_idx]);
                        if dist2 > same2 {
                            poly_rem_is_poly_keep = false;
                            break;
                        }
                    }
                    if poly_rem_is_poly_keep {
                        should_be_removed = true;
                        break;
                    }
                }
            }
            if !should_be_removed {
                result.push(poly_keep.clone());
            }
        }
        result
    }

    pub fn process_even_odd(&self, poly_fill_type: PolyFillType) -> Shape {
        let mut ret = Vec::new();
        let mut clipper = Clipper::new(0);
        self.add_paths(&mut clipper, PolyType::Subject);
        clipper.execute(ClipType::Union, &mut ret, poly_fill_type, poly_fill_type);
        Shape::from_paths(ret)
    }

    pub fn ensure_manifold(&mut self) {
        let mut duplicate_locations: Vec<Point2LL> = Vec::new();
        let mut poly_locations: HashSet<Point2LL> = HashSet::new();
        for poly in &self.lines {
            for &p in poly.iter() {
                if poly_locations.contains(&p) {
                    duplicate_locations.push(p);
                }
                poly_locations.insert(p);
            }
        }
        let mut removal_dots = Shape::default();
        for &p in &duplicate_locations {
            let dot = removal_dots.new_line();
            dot.push(p + pt(0, 5));
            dot.push(p + pt(5, 0));
            dot.push(p + pt(0, -5));
            dot.push(p + pt(-5, 0));
        }
        if !removal_dots.is_empty() {
            *self = self.difference(&removal_dots);
        }
    }

    pub fn offset_multi(&self, offset_dists: &[Coord]) -> Shape {
        // we need as many offset-dists as points
        debug_assert_eq!(self.point_count(), offset_dists.len());

        let mut ret: Paths = Vec::new();
        let mut i = 0usize;
        for poly_line in self.lines.iter().filter(|path| !path.is_empty()) {
            let mut ret_poly_line = Polygon::default();

            let mut prev_p = *poly_line.last().unwrap();
            let mut prev_dist = offset_dists[i + poly_line.len() - 1];

            for &p in poly_line.iter() {
                let offset_dist = offset_dists[i];

                let vec_dir = prev_p - p;

                const MIN_VEC_LEN: Coord = 10;
                if v_size2(vec_dir) > MIN_VEC_LEN * MIN_VEC_LEN {
                    let offset_p1 = turn90_ccw(normal(vec_dir, prev_dist));
                    let offset_p2 = turn90_ccw(normal(vec_dir, offset_dist));

                    ret_poly_line.push(prev_p + offset_p1);
                    ret_poly_line.push(p + offset_p2);
                }

                prev_p = p;
                prev_dist = offset_dist;
                i += 1;
            }

            ret.push(ret_poly_line.points);
        }

        let ret = clipper::simplify_polygons(&ret, PolyFillType::Positive);
        Shape::from_paths(ret)
    }

    /// `removeNearSelfIntersections()` using mapbox wagyu.
    pub fn remove_near_self_intersections(&self) -> Shape {
        crate::geometry::wagyu_bridge::remove_near_self_intersections(self)
    }

    /// `Shape::simplify(fill_type)`: make the polygons strictly simple.
    pub fn simplify_strict(&mut self, fill_type: PolyFillType) {
        if self.is_empty() {
            return;
        }
        let mut clipper = Clipper::new(0);
        let mut ret = Vec::new();
        clipper.set_strictly_simple(true);
        self.add_paths(&mut clipper, PolyType::Subject);
        clipper.execute(ClipType::Union, &mut ret, fill_type, fill_type);
        self.lines.resize_with(ret.len(), Polygon::default);
        for (i, path) in ret.into_iter().enumerate() {
            let polygon = &mut self.lines[i];
            polygon.explicitly_closed = false;
            polygon.points = path;
        }
    }

    pub fn intersections_with_segment(&self, start: Point2LL, end: Point2LL) -> Vec<f32> {
        let mut result = Vec::new();
        for polygon in &self.lines {
            for (s, e) in polygon.segments() {
                if let Some((t, _u)) = la::segment_segment_intersection(start, end, s, e) {
                    result.push(t);
                }
            }
        }
        result
    }
}

/// A single area: an outer polygon and its holes (`cura::SingleShape`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SingleShape(pub Shape);

impl Deref for SingleShape {
    type Target = Shape;
    fn deref(&self) -> &Shape {
        &self.0
    }
}

impl DerefMut for SingleShape {
    fn deref_mut(&mut self) -> &mut Shape {
        &mut self.0
    }
}

impl From<Shape> for SingleShape {
    fn from(s: Shape) -> Self {
        SingleShape(s)
    }
}

impl SingleShape {
    pub fn outer_polygon(&self) -> &Polygon {
        &self.0[0]
    }

    pub fn outer_polygon_mut(&mut self) -> &mut Polygon {
        &mut self.0[0]
    }

    /// `SingleShape::inside`: inside the outline and not inside any hole.
    pub fn inside(&self, p: Point2LL, border_result: bool) -> bool {
        if self.0.is_empty() {
            return false;
        }
        if !self.0[0].inside(p, border_result) {
            return false;
        }
        for n in 1..self.0.len() {
            if self.0[n].inside(p, border_result) {
                return false;
            }
        }
        true
    }

    pub fn into_shape(self) -> Shape {
        self.0
    }
}

/// `PartsView`: indices of the polygons in a shape, grouped per part (first = outline).
#[derive(Clone, Debug, Default)]
pub struct PartsView {
    pub parts: Vec<Vec<usize>>,
}

impl Deref for PartsView {
    type Target = Vec<Vec<usize>>;
    fn deref(&self) -> &Vec<Vec<usize>> {
        &self.parts
    }
}

impl PartsView {
    pub fn get_part_containing(&self, poly_idx: usize, boundary_poly_idx: Option<&mut usize>) -> usize {
        for (part_idx_now, part_view) in self.parts.iter().enumerate() {
            if part_view.is_empty() {
                continue;
            }
            if part_view.contains(&poly_idx) {
                if let Some(b) = boundary_poly_idx {
                    *b = part_view[0];
                }
                return part_idx_now;
            }
        }
        NO_INDEX
    }

    pub fn assemble_part(&self, polygons: &Shape, part_idx: usize) -> SingleShape {
        let mut ret = SingleShape::default();
        if part_idx != NO_INDEX {
            for &poly_idx_ff in &self.parts[part_idx] {
                ret.push(polygons[poly_idx_ff].clone());
            }
        }
        ret
    }

    pub fn assemble_part_containing(&self, polygons: &Shape, poly_idx: usize, boundary_poly_idx: Option<&mut usize>) -> SingleShape {
        let part_idx = self.get_part_containing(poly_idx, boundary_poly_idx);
        if part_idx != NO_INDEX {
            return self.assemble_part(polygons, part_idx);
        }
        SingleShape::default()
    }
}

/// Default deviation angle of `Shape::removeColinearEdges`.
pub fn default_max_deviation_angle() -> AngleRadians {
    AngleRadians::new(0.0005)
}
