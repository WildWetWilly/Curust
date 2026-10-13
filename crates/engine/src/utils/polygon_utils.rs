//! `polygonUtils.h/.cpp`: assorted polygon algorithms.

use crate::geometry::lines_set::NO_INDEX;
use crate::geometry::matrix::PointMatrix;
use crate::geometry::point2ll::{dot, normal, turn90_ccw, v_size, v_size2, Point2LL};
use crate::geometry::polyline::ClosedLine;
use crate::geometry::{ClosedLinesSet, ClosedPolyline, OpenPolyline, Polygon, Polyline, Shape};
use crate::geometry::OpenLinesSet;
use crate::settings::types::AngleRadians;
use crate::utils::aabb::AABB;
use crate::utils::coord::Coord;
use crate::utils::linear_alg_2d as la;
use crate::utils::polygons_point_index::{polygons_point_index_segment_locator, PolygonsPointIndex};
use crate::utils::sparse_grid::SparseLineGrid;
use clipper::{JoinType, PolyFillType};

/// `ClosestPoint<LineType>`.
pub struct ClosestPoint<L> {
    /// Result location
    pub location: Point2LL,
    /// Polygon in which the result was found (or nullptr if no result was found)
    pub poly: *const L,
    /// The index of the polygon in some Shape where ClosestPoint::poly can be found
    pub poly_idx: usize,
    /// Index to the first point in the polygon of the line segment on which the result was found
    pub point_idx: usize,
}

unsafe impl<L> Send for ClosestPoint<L> {}
unsafe impl<L> Sync for ClosestPoint<L> {}

impl<L> Clone for ClosestPoint<L> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<L> Copy for ClosestPoint<L> {}

impl<L> std::fmt::Debug for ClosestPoint<L> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ClosestPoint({:?}, {:p}, {}, {})", self.location, self.poly, self.poly_idx, self.point_idx)
    }
}

impl<L> Default for ClosestPoint<L> {
    fn default() -> Self {
        ClosestPoint { location: Point2LL::default(), poly: std::ptr::null(), poly_idx: NO_INDEX, point_idx: NO_INDEX }
    }
}

impl<L> PartialEq for ClosestPoint<L> {
    fn eq(&self, rhs: &Self) -> bool {
        self.poly == rhs.poly && self.point_idx == rhs.point_idx && self.location == rhs.location
    }
}

impl<L> ClosestPoint<L> {
    pub fn new(p: Point2LL, pos: usize, poly: *const L) -> Self {
        ClosestPoint { location: p, poly, poly_idx: NO_INDEX, point_idx: pos }
    }
    pub fn with_poly_idx(p: Point2LL, pos: usize, poly: *const L, poly_idx: usize) -> Self {
        ClosestPoint { location: p, poly, poly_idx, point_idx: pos }
    }
    pub fn invalid_with_poly(poly: *const L) -> Self {
        ClosestPoint { location: Point2LL::default(), poly, poly_idx: NO_INDEX, point_idx: NO_INDEX }
    }
    pub fn p(&self) -> Point2LL {
        self.location
    }
    pub fn is_valid(&self) -> bool {
        self.point_idx != NO_INDEX
    }
    /// # Safety
    /// The referenced polygon must still be alive.
    pub fn poly(&self) -> &L {
        unsafe { &*self.poly }
    }
}

pub type ClosestPointPolygon = ClosestPoint<Polygon>;

#[derive(Clone, Copy, Debug, Default)]
pub struct GivenDistPoint {
    pub location: Point2LL,
    pub pos: i32,
}

pub type LocToLineGrid = SparseLineGrid<PolygonsPointIndex, fn(&PolygonsPointIndex) -> (Point2LL, Point2LL)>;

pub type PenaltyFn<'a> = &'a dyn Fn(Point2LL) -> i32;

pub fn no_penalty_function(_p: Point2LL) -> i32 {
    0
}

pub const NO_PENALTY: PenaltyFn<'static> = &no_penalty_function;

#[inline]
fn int_sq(d: i32) -> i64 {
    // `distance * distance` with `int` operands.
    d.wrapping_mul(d) as i64
}

pub struct PolygonUtils;

impl PolygonUtils {
    pub fn spread_dots_area(polygons: &Shape, grid_size: Coord) -> Vec<Point2LL> {
        Self::spread_dots_area_pt(polygons, Point2LL::new(grid_size, grid_size))
    }

    pub fn spread_dots_area_pt(polygons: &Shape, grid_size: Point2LL) -> Vec<Point2LL> {
        use crate::infill::infill_gen::{Infill, InfillExtra};
        use crate::settings::enums::EFillMethod;
        use crate::settings::settings::Settings;
        use crate::settings::types::AngleDegrees;
        use crate::utils::section_type::SectionType;
        let mut dummy_toolpaths = Vec::new();
        let dummy_settings = Settings::new();
        let mut infill_gen =
            Infill::new(EFillMethod::LINES, false, false, polygons.clone(), 0, grid_size.x, 0, 1, AngleDegrees::new(0.0), 0, 0, 0, 0, InfillExtra::default());
        let mut result_polygons = Shape::default();
        let mut result_lines = OpenLinesSet::default();
        // FIXME (upstream): make sure the proper layer nr is used
        infill_gen.generate(&mut dummy_toolpaths, &mut result_polygons, &mut result_lines, &dummy_settings, 0, SectionType::DOTS, None, None, None, &Shape::default());
        let mut result = Vec::new();
        for line in result_lines.iter() {
            debug_assert!(line.len() == 2);
            let mut a = line[0];
            let mut b = line[1];
            if a.y > b.y {
                std::mem::swap(&mut a, &mut b);
            }
            let mut y = a.y - (a.y % grid_size.y) - grid_size.y;
            while y < b.y {
                if y >= a.y {
                    result.push(Point2LL::new(a.x, y));
                }
                y += grid_size.y;
            }
        }
        result
    }

    pub fn line_segment_polygons_intersection(
        a: Point2LL,
        b: Point2LL,
        current_outlines: &Shape,
        outline_locator: &LocToLineGrid,
        result: &mut Point2LL,
        within_max_dist: Coord,
    ) -> bool {
        let within_max_dist2 = within_max_dist * within_max_dist;

        let mut closest_dist2 = within_max_dist2;

        let process_on_intersect = |p_start: Point2LL, p_end: Point2LL, closest_dist2: &mut Coord, result: &mut Point2LL| {
            // Validate that the intersection is on the line segments.
            if let Some(coll) = la::line_line_intersection(a, b, p_start, p_end) {
                if la::point_is_projected_beyond_line(coll, p_start, p_end) == 0 && la::point_is_projected_beyond_line(coll, a, b) == 0 {
                    let dist2 = v_size2(b - coll);
                    if dist2 < *closest_dist2 {
                        *closest_dist2 = dist2;
                        *result = coll;
                    }
                }
            }
        };

        let nearby = outline_locator.get_nearby(b, within_max_dist);
        if !nearby.is_empty() {
            for pp_idx in &nearby {
                process_on_intersect(pp_idx.p(), pp_idx.next().p(), &mut closest_dist2, result);
            }
            if closest_dist2 < within_max_dist2 {
                return true;
            }
        }

        for poly in current_outlines.iter() {
            let poly_size = poly.len();
            for i_segment_start in 0..poly_size {
                let i_segment_end = (i_segment_start + 1) % poly_size;
                process_on_intersect(poly[i_segment_start], poly[i_segment_end], &mut closest_dist2, result);
            }
        }

        closest_dist2 < within_max_dist2
    }

    pub fn get_vertex_inward_normal(poly: &[Point2LL], point_idx: u32) -> Point2LL {
        let p1 = poly[point_idx as usize];

        let mut p0_idx: i32 = point_idx as i32 - 1;
        while p0_idx as u32 != point_idx {
            if p0_idx == -1 {
                p0_idx = poly.len() as i32 - 1;
            }
            if poly[p0_idx as usize] != p1 {
                break;
            }
            p0_idx -= 1;
        }
        let p0 = poly[p0_idx as usize];

        let mut p2_idx: u32 = point_idx + 1;
        while p2_idx != point_idx {
            if p2_idx as usize == poly.len() {
                p2_idx = 0;
            }
            if poly[p2_idx as usize] != p1 {
                break;
            }
            p2_idx += 1;
        }
        let p2 = poly[p2_idx as usize];

        let off0 = turn90_ccw(normal(p1 - p0, 10000)); // 10.0 for some precision
        let off1 = turn90_ccw(normal(p2 - p1, 10000)); // 10.0 for some precision
        off0 + off1
    }

    pub fn get_boundary_point_with_offset(poly: &[Point2LL], point_idx: u32, offset: i64) -> Point2LL {
        poly[point_idx as usize] + normal(Self::get_vertex_inward_normal(poly, point_idx), -offset)
    }

    pub fn move_outside(polygons: &Shape, from: &mut Point2LL, distance: i32, max_dist2: i64) -> u32 {
        Self::move_inside(polygons, from, -distance, max_dist2) as u32
    }

    pub fn move_inside2(
        polygons: &Shape,
        from: &mut Point2LL,
        distance: i32,
        max_dist2: i64,
        loc_to_line_polygons: Option<&Shape>,
        loc_to_line_grid: Option<&LocToLineGrid>,
        penalty_function: PenaltyFn,
    ) -> ClosestPointPolygon {
        let mut closest_polygon_point: Option<ClosestPointPolygon> = None;
        if let Some(grid) = loc_to_line_grid {
            closest_polygon_point = Self::find_close(*from, loc_to_line_polygons.unwrap(), grid, penalty_function);
        }
        if closest_polygon_point.is_none() {
            closest_polygon_point = Some(Self::find_closest(*from, polygons, penalty_function));
        }
        Self::move_inside2_impl(&closest_polygon_point.unwrap(), distance, from, max_dist2)
    }

    pub fn move_inside2_polygon(
        loc_to_line_polygons: &Shape,
        polygon: &Polygon,
        from: &mut Point2LL,
        distance: i32,
        max_dist2: i64,
        loc_to_line_grid: Option<&LocToLineGrid>,
        penalty_function: PenaltyFn,
    ) -> ClosestPointPolygon {
        let mut closest_polygon_point: Option<ClosestPointPolygon> = None;
        if let Some(grid) = loc_to_line_grid {
            closest_polygon_point = Self::find_close(*from, loc_to_line_polygons, grid, penalty_function);
        }
        if closest_polygon_point.is_none() {
            closest_polygon_point = Some(Self::find_closest_polygon(*from, polygon, penalty_function));
        }
        Self::move_inside2_impl(&closest_polygon_point.unwrap(), distance, from, max_dist2)
    }

    fn move_inside2_impl(closest_polygon_point: &ClosestPointPolygon, distance: i32, from: &mut Point2LL, max_dist2: i64) -> ClosestPointPolygon {
        if !closest_polygon_point.is_valid() {
            return ClosestPointPolygon::default(); // stub with invalid indices to signify we haven't found any
        }
        let v_boundary_from = *from - closest_polygon_point.location;
        let result = Self::move_inside_cpp(closest_polygon_point, distance);
        let v_boundary_result = result - closest_polygon_point.location;
        if dot(v_boundary_result, v_boundary_from) > 0 {
            // point is already on the right side of the boundary
            if v_size2(v_boundary_from) > int_sq(distance) {
                // move inside/outside wasn't needed
                *closest_polygon_point
            } else {
                *from = result;
                *closest_polygon_point
            }
        } else {
            if v_size2(v_boundary_from) > max_dist2 {
                ClosestPointPolygon::invalid_with_poly(closest_polygon_point.poly) // poly_idx is the only field we don't know!
            } else {
                *from = result;
                *closest_polygon_point
            }
        }
    }

    /// `moveInside(const Shape&, Point2LL&, int distance, int64_t max_dist2)`.
    pub fn move_inside(polygons: &Shape, from: &mut Point2LL, distance: i32, max_dist2: i64) -> usize {
        let mut ret = *from;
        let mut best_dist2 = i64::MAX;
        let mut best_poly = NO_INDEX;
        let mut is_already_on_correct_side_of_boundary = false; // whether [from] is already on the right side of the boundary
        for poly_idx in 0..polygons.len() {
            let poly = &polygons[poly_idx];
            if poly.len() < 2 {
                continue;
            }
            let mut p0 = poly[poly.len() - 2];
            let mut p1 = *poly.last().unwrap();
            // because we compare with vSize2 here (no division by zero), we also need to compare by vSize2 inside the loop
            // to avoid integer rounding edge cases
            let mut projected_p_beyond_prev_segment = dot(p1 - p0, *from - p0) >= v_size2(p1 - p0);
            for &p2 in poly.iter() {
                // X = A + Normal(B-A) * (((B-A) dot (P-A)) / VSize(B-A));
                //   = A +       (B-A) *  ((B-A) dot (P-A)) / VSize2(B-A);
                // X = P projected on AB
                let a = p1;
                let b = p2;
                let p = *from;
                let ab = b - a;
                let ap = p - a;
                let ab_length2 = v_size2(ab);
                if ab_length2 <= 0 {
                    // A = B, i.e. the input polygon had two adjacent points on top of each other.
                    p1 = p2; // Skip only one of the points.
                    continue;
                }
                let dot_prod = dot(ab, ap);
                if dot_prod <= 0 {
                    // x is projected to before ab
                    if projected_p_beyond_prev_segment {
                        //  case which looks like:   > .
                        projected_p_beyond_prev_segment = false;
                        let x = p1;

                        let dist2 = v_size2(x - p);
                        if dist2 < best_dist2 {
                            best_dist2 = dist2;
                            best_poly = poly_idx;
                            if distance == 0 {
                                ret = x;
                            } else {
                                // inward direction irrespective of sign of [distance]
                                let inward_dir = turn90_ccw(normal(ab, 10000) + normal(p1 - p0, 10000));
                                // MM2INT(10.0) to retain precision for the eventual normalization
                                ret = x + normal(inward_dir, distance as i64);
                                is_already_on_correct_side_of_boundary = dot(inward_dir, p - x).wrapping_mul(distance as i64) >= 0;
                            }
                        }
                    } else {
                        projected_p_beyond_prev_segment = false;
                        p0 = p1;
                        p1 = p2;
                        continue;
                    }
                } else if dot_prod >= ab_length2 {
                    // x is projected to beyond ab
                    projected_p_beyond_prev_segment = true;
                    p0 = p1;
                    p1 = p2;
                    continue;
                } else {
                    // x is projected to a point properly on the line segment (not onto a vertex). The case which looks like | .
                    projected_p_beyond_prev_segment = false;
                    let x = a + ab * dot_prod / ab_length2;

                    let dist2 = v_size2(p - x);
                    if dist2 < best_dist2 {
                        best_dist2 = dist2;
                        best_poly = poly_idx;
                        if distance == 0 {
                            ret = x;
                        } else {
                            // inward or outward depending on the sign of [distance]
                            let inward_dir = turn90_ccw(normal(ab, distance as i64));
                            ret = x + inward_dir;
                            is_already_on_correct_side_of_boundary = dot(inward_dir, p - x) >= 0;
                        }
                    }
                }
                p0 = p1;
                p1 = p2;
            }
        }
        if is_already_on_correct_side_of_boundary {
            // when the best point is already inside and we're moving inside, or when the best point is already outside and we're moving outside
            if best_dist2 < int_sq(distance) {
                *from = ret;
            }
            best_poly
        } else if best_dist2 < max_dist2 {
            *from = ret;
            best_poly
        } else {
            NO_INDEX
        }
    }

    /// `moveInside(const ClosedPolyline&, Point2LL&, int distance, int64_t max_dist2)`.
    pub fn move_inside_polygon(polygon: &[Point2LL], from: &mut Point2LL, distance: i32, max_dist2: i64) -> u32 {
        let mut ret = *from;
        let mut best_dist2 = i64::MAX;
        let mut is_already_on_correct_side_of_boundary = false; // whether [from] is already on the right side of the boundary

        if polygon.len() < 2 {
            return 0;
        }
        let mut p0 = polygon[polygon.len() - 2];
        let mut p1 = *polygon.last().unwrap();
        // because we compare with vSize2 here (no division by zero), we also need to compare by vSize2 inside the loop
        // to avoid integer rounding edge cases
        let mut projected_p_beyond_prev_segment = dot(p1 - p0, *from - p0) >= v_size2(p1 - p0);
        for &p2 in polygon.iter() {
            let a = p1;
            let b = p2;
            let p = *from;
            let ab = b - a;
            let ap = p - a;
            let ab_length2 = v_size2(ab);
            if ab_length2 <= 0 {
                // A = B, i.e. the input polygon had two adjacent points on top of each other.
                p1 = p2; // Skip only one of the points.
                continue;
            }
            let dot_prod = dot(ab, ap);
            if dot_prod <= 0 {
                // x is projected to before ab
                if projected_p_beyond_prev_segment {
                    //  case which looks like:   > .
                    projected_p_beyond_prev_segment = false;
                    let x = p1;

                    let dist2 = v_size2(x - p);
                    if dist2 < best_dist2 {
                        best_dist2 = dist2;
                        if distance == 0 {
                            ret = x;
                        } else {
                            // inward direction irrespective of sign of [distance]
                            let inward_dir = turn90_ccw(normal(ab, 10000) + normal(p1 - p0, 10000));
                            // MM2INT(10.0) to retain precision for the eventual normalization
                            ret = x + normal(inward_dir, distance as i64);
                            is_already_on_correct_side_of_boundary = dot(inward_dir, p - x).wrapping_mul(distance as i64) >= 0;
                        }
                    }
                } else {
                    projected_p_beyond_prev_segment = false;
                    p0 = p1;
                    p1 = p2;
                    continue;
                }
            } else if dot_prod >= ab_length2 {
                // x is projected to beyond ab
                projected_p_beyond_prev_segment = true;
                p0 = p1;
                p1 = p2;
                continue;
            } else {
                // x is projected to a point properly on the line segment (not onto a vertex). The case which looks like | .
                projected_p_beyond_prev_segment = false;
                let x = a + ab * dot_prod / ab_length2;

                let dist2 = v_size2(p - x);
                if dist2 < best_dist2 {
                    best_dist2 = dist2;
                    if distance == 0 {
                        ret = x;
                    } else {
                        // inward or outward depending on the sign of [distance]
                        let inward_dir = turn90_ccw(normal(ab, distance as i64));
                        ret = x + inward_dir;
                        is_already_on_correct_side_of_boundary = dot(inward_dir, p - x) >= 0;
                    }
                }
            }
            p0 = p1;
            p1 = p2;
        }
        if is_already_on_correct_side_of_boundary {
            // when the best point is already inside and we're moving inside, or when the best point is already outside and we're moving outside
            if best_dist2 < int_sq(distance) {
                *from = ret;
            }
        } else if best_dist2 < max_dist2 {
            *from = ret;
        }
        0
    }

    pub fn move_outside_cpp(cpp: &ClosestPointPolygon, distance: i32) -> Point2LL {
        Self::move_inside_cpp(cpp, -distance)
    }

    /// `moveInside(const ClosestPointPolygon& cpp, const int distance)`.
    pub fn move_inside_cpp(cpp: &ClosestPointPolygon, distance: i32) -> Point2LL {
        if !cpp.is_valid() {
            return crate::geometry::point2ll::NO_POINT;
        }
        if distance == 0 {
            // the point which is assumed to be on the boundary doesn't have to be moved
            return cpp.location;
        }
        let poly = &cpp.poly().points;
        let point_idx = cpp.point_idx as u32;
        let on_boundary = cpp.location;

        let p1 = poly[point_idx as usize];
        let mut p2_idx: u32 = point_idx + 1;
        while p2_idx != point_idx {
            if p2_idx as usize == poly.len() {
                p2_idx = 0;
            }
            if poly[p2_idx as usize] != p1 {
                break;
            }
            p2_idx += 1;
        }
        let p2 = poly[p2_idx as usize];

        if on_boundary == p1 {
            Self::get_boundary_point_with_offset(poly, point_idx, -(distance as i64))
        } else if on_boundary == p2 {
            Self::get_boundary_point_with_offset(poly, p2_idx, -(distance as i64))
        } else {
            let x = on_boundary; // on_boundary is already projected on p1-p2

            let inward_dir = turn90_ccw(normal(p2 - p1, distance as i64));
            x + inward_dir
        }
    }

    pub fn ensure_inside_or_outside(
        polygons: &Shape,
        from: &mut Point2LL,
        preferred_dist_inside: i32,
        max_dist2: i64,
        loc_to_line_polygons: Option<&Shape>,
        loc_to_line_grid: Option<&LocToLineGrid>,
        penalty_function: PenaltyFn,
    ) -> ClosestPointPolygon {
        let closest_polygon_point =
            Self::move_inside2(polygons, from, preferred_dist_inside, max_dist2, loc_to_line_polygons, loc_to_line_grid, penalty_function);
        Self::ensure_inside_or_outside_cpp(
            polygons,
            from,
            &closest_polygon_point,
            preferred_dist_inside,
            loc_to_line_polygons,
            loc_to_line_grid,
            penalty_function,
        )
    }

    pub fn ensure_inside_or_outside_cpp(
        polygons: &Shape,
        from: &mut Point2LL,
        closest_polygon_point: &ClosestPointPolygon,
        preferred_dist_inside: i32,
        loc_to_line_polygons: Option<&Shape>,
        loc_to_line_grid: Option<&LocToLineGrid>,
        penalty_function: PenaltyFn,
    ) -> ClosestPointPolygon {
        if !closest_polygon_point.is_valid() {
            return ClosestPointPolygon::default(); // we couldn't move inside
        }
        let closest_poly = closest_polygon_point.poly();
        let is_outside_boundary = closest_poly.orientation();

        {
            let is_inside = closest_poly.inside(*from, false) == is_outside_boundary; // inside a hole is outside the part
            if is_inside == (preferred_dist_inside > 0) {
                // we ended up on the right side of the polygon
                // assume we didn't overshoot another polygon in [polygons]
                return *closest_polygon_point;
            }
        }

        // try once more with half the preferred distance inside
        {
            let max_dist2_here = i64::MAX; // we already concluded we are close enough to the closest_poly when we obtained the closest_polygon_point
            let empty = Shape::default();
            Self::move_inside2_polygon(
                loc_to_line_polygons.unwrap_or(&empty),
                closest_poly,
                from,
                preferred_dist_inside / 2,
                max_dist2_here,
                loc_to_line_grid,
                penalty_function,
            );
            let is_inside = closest_poly.inside(*from, false) == is_outside_boundary; // inside a hole is outside the part
            if is_inside == (preferred_dist_inside > 0) {
                // we ended up on the right side of the polygon
                // assume we didn't overshoot another polygon in [polygons]
                return *closest_polygon_point;
            }
        }

        // if above fails, we perform an offset and sit directly on the offsetted polygon (and keep the result from the above moveInside)
        // The offset is performed on the closest reference polygon in order to save computation time
        {
            let offset: Coord = if is_outside_boundary { -(preferred_dist_inside as Coord) } else { preferred_dist_inside as Coord }; // perform inset on outer boundary and outset on holes
            let insetted = closest_poly.offset((offset / 2) as i32, JoinType::Miter, 1.2); // perform less inset, because chances are (thin parts of) the polygon will disappear, given that moveInside did an overshoot
            if insetted.is_empty() {
                return ClosestPointPolygon::default(); // we couldn't move inside
            }
            let mut inside = Self::find_closest(*from, &insetted, penalty_function);
            if inside.is_valid() {
                let is_inside = polygons.inside(inside.location, false);
                if is_inside != (preferred_dist_inside > 0) {
                    // Insetting from the reference polygon ended up outside another polygon.
                    // Perform an offset on all polygons instead.
                    let all_insetted = polygons.offset(-(preferred_dist_inside as Coord), JoinType::Miter, 1.2);
                    let overall_inside = Self::find_closest(*from, &all_insetted, penalty_function);
                    let overall_is_inside = polygons.inside(overall_inside.location, false);
                    if overall_is_inside != (preferred_dist_inside > 0) {
                        return ClosestPointPolygon::default();
                    }
                    inside = overall_inside;
                }
                *from = inside.location;
            } // otherwise we just return the closest polygon point without modifying the from location
            *closest_polygon_point // don't return a ClosestPoint with a reference to the above local polygons variable
        }
    }

    pub fn walk_to_nearest_smallest_connection(poly1_result: &mut ClosestPointPolygon, poly2_result: &mut ClosestPointPolygon) {
        if !poly1_result.is_valid() || !poly2_result.is_valid() {
            return;
        }
        let poly1 = poly1_result.poly;
        let poly2 = poly2_result.poly;
        let poly1_idx = poly1_result.poly_idx;
        let poly2_idx = poly2_result.poly_idx;
        if poly1_result.point_idx == NO_INDEX || poly2_result.point_idx == NO_INDEX {
            return;
        }
        // SAFETY: the polygons are alive for the duration of the call.
        let (poly1, poly2) = unsafe { (&*poly1, &*poly2) };

        let equilibirum_limit = 100; // hard coded value
        for _loop_counter in 0..equilibirum_limit {
            let pos1_before = poly1_result.point_idx as u32;
            *poly1_result = Self::find_nearest_closest(poly2_result.location, poly1, poly1_result.point_idx as i32);
            let pos2_before = poly2_result.point_idx as u32;
            *poly2_result = Self::find_nearest_closest(poly1_result.location, poly2, poly2_result.point_idx as i32);

            if poly1_result.point_idx as u32 == pos1_before && poly2_result.point_idx as u32 == pos2_before {
                break;
            }
        }

        // check surrounding verts in order to prevent local optima like the following:
        // o      o
        // \.....|
        //  \_.-'|
        //   \---|
        //    \-'|
        //     o      o >> should find connection here
        let mut best_distance2 = v_size2(poly1_result.p() - poly2_result.p());
        let mut check_neighboring_vert =
            |from_poly: &Polygon, to_poly: &Polygon, from_poly_result: &mut ClosestPointPolygon, to_poly_result: &mut ClosestPointPolygon, vertex_after: bool| {
                let after_poly2_result = to_poly[(to_poly_result.point_idx + vertex_after as usize) % to_poly.len()];
                let poly1_after_poly2_result = Self::find_nearest_closest(after_poly2_result, from_poly, from_poly_result.point_idx as i32);
                let poly1_after_poly2_result_dist2 = v_size2(poly1_after_poly2_result.p() - after_poly2_result);
                if poly1_after_poly2_result_dist2 < best_distance2 {
                    *from_poly_result = poly1_after_poly2_result;
                    to_poly_result.location = after_poly2_result;
                    best_distance2 = poly1_after_poly2_result_dist2;
                }
            };
        check_neighboring_vert(poly1, poly2, poly1_result, poly2_result, false);
        check_neighboring_vert(poly1, poly2, poly1_result, poly2_result, true);
        check_neighboring_vert(poly2, poly1, poly2_result, poly1_result, false);
        check_neighboring_vert(poly2, poly1, poly2_result, poly1_result, true);

        poly1_result.poly_idx = poly1_idx;
        poly2_result.poly_idx = poly2_idx;
    }

    pub fn find_nearest_closest(from: Point2LL, polygon: &Polygon, start_idx: i32) -> ClosestPointPolygon {
        let forth = Self::find_nearest_closest_dir(from, polygon, start_idx, 1);
        if !forth.is_valid() {
            return forth; // stop computation
        }
        let back = Self::find_nearest_closest_dir(from, polygon, start_idx, -1);
        debug_assert!(back.is_valid());
        if v_size2(forth.location - from) < v_size2(back.location - from) {
            forth
        } else {
            back
        }
    }

    pub fn find_nearest_closest_dir(from: Point2LL, polygon: &Polygon, start_idx: i32, direction: i32) -> ClosestPointPolygon {
        if polygon.is_empty() {
            return ClosestPointPolygon::invalid_with_poly(polygon);
        }
        let a_point = polygon[0];
        let mut best = a_point;

        let mut closest_dist = v_size2(from - best);
        let mut best_pos: i32 = 0;

        let poly_size = polygon.len() as u64;
        for p in 0..poly_size {
            // size_t arithmetic, as in C++.
            let p1_idx = (poly_size.wrapping_add((direction as i64 as u64).wrapping_mul(p)).wrapping_add(start_idx as i64 as u64) % poly_size) as i32;
            let p2_idx = (poly_size.wrapping_add((direction as i64 as u64).wrapping_mul(p + 1)).wrapping_add(start_idx as i64 as u64) % poly_size) as i32;
            let p1 = polygon[p1_idx as usize];
            let p2 = polygon[p2_idx as usize];

            let closest_here = la::get_closest_on_line_segment(from, p1, p2);
            let dist = v_size2(from - closest_here);
            if dist < closest_dist {
                best = closest_here;
                closest_dist = dist;
                best_pos = if direction > 0 { p1_idx } else { p2_idx };
            } else {
                return ClosestPointPolygon::new(best, best_pos as usize, polygon);
            }
        }

        ClosestPointPolygon::new(best, best_pos as usize, polygon)
    }

    pub fn find_closest(from: Point2LL, polygons: &Shape, penalty_function: PenaltyFn) -> ClosestPointPolygon {
        let none = ClosestPointPolygon::default();

        if polygons.is_empty() {
            return none;
        }
        let mut any_polygon: &Polygon = &polygons[0];
        let mut any_poly_idx: u32 = 0;
        while (any_poly_idx as usize) < polygons.len() {
            if !polygons[any_poly_idx as usize].is_empty() {
                any_polygon = &polygons[any_poly_idx as usize];
                break;
            }
            any_poly_idx += 1;
        }
        if any_polygon.is_empty() {
            return none;
        }
        let mut best = ClosestPointPolygon::with_poly_idx(any_polygon[0], 0, any_polygon, any_poly_idx as usize);

        let mut closest_dist2_score = v_size2(from - best.location) + penalty_function(best.location) as i64;

        for ply in 0..polygons.len() {
            let poly = &polygons[ply];
            if poly.is_empty() {
                continue;
            }
            let mut closest_here = Self::find_closest_polygon(from, poly, penalty_function);
            if !closest_here.is_valid() {
                continue;
            }
            let dist2_score = v_size2(from - closest_here.location) + penalty_function(closest_here.location) as i64;
            if dist2_score < closest_dist2_score {
                closest_here.poly_idx = ply;
                best = closest_here;
                closest_dist2_score = dist2_score;
            }
        }

        best
    }

    pub fn find_closest_polygon(from: Point2LL, polygon: &Polygon, penalty_function: PenaltyFn) -> ClosestPointPolygon {
        if polygon.is_empty() {
            return ClosestPointPolygon::invalid_with_poly(polygon);
        }
        let a_point = polygon[0];
        let mut best = a_point;

        let mut closest_dist2_score = v_size2(from - best) + penalty_function(best) as i64;
        let mut best_pos: i32 = 0;

        for p in 0..polygon.len() as u32 {
            let p1 = polygon[p as usize];

            let mut p2_idx = p + 1;
            if p2_idx as usize >= polygon.len() {
                p2_idx = 0;
            }
            let p2 = polygon[p2_idx as usize];

            let closest_here = la::get_closest_on_line_segment(from, p1, p2);
            let dist2_score = v_size2(from - closest_here) + penalty_function(closest_here) as i64;
            if dist2_score < closest_dist2_score {
                best = closest_here;
                closest_dist2_score = dist2_score;
                best_pos = p as i32;
            }
        }

        ClosestPointPolygon::new(best, best_pos as usize, polygon)
    }

    pub fn find_nearest_vert(from: Point2LL, polys: &Shape) -> PolygonsPointIndex {
        let mut best_dist2 = Coord::MAX;
        let mut closest_vert = PolygonsPointIndex::default();
        for poly_idx in 0..polys.len() {
            let poly = &polys[poly_idx];
            for point_idx in 0..poly.len() {
                let dist2 = v_size2(poly[point_idx] - from);
                if dist2 < best_dist2 {
                    best_dist2 = dist2;
                    closest_vert = PolygonsPointIndex::new(polys, poly_idx, point_idx);
                }
            }
        }
        closest_vert
    }

    pub fn find_nearest_vert_polygon(from: Point2LL, poly: &[Point2LL]) -> u32 {
        let mut best_dist2 = i64::MAX;
        let mut closest_vert_idx: u32 = u32::MAX;
        for point_idx in 0..poly.len() as u32 {
            let dist2 = v_size2(poly[point_idx as usize] - from);
            if dist2 < best_dist2 {
                best_dist2 = dist2;
                closest_vert_idx = point_idx;
            }
        }
        closest_vert_idx
    }

    pub fn create_loc_to_line_grid(polygons: &Shape, square_size: i32) -> Box<LocToLineGrid> {
        let mut n_points: u32 = 0;
        for poly in polygons.iter() {
            n_points = n_points.wrapping_add(poly.len() as u32);
        }

        let locator: fn(&PolygonsPointIndex) -> (Point2LL, Point2LL) = polygons_point_index_segment_locator;
        let mut ret = Box::new(LocToLineGrid::new(square_size as Coord, n_points as usize, locator));

        for poly_idx in 0..polygons.len() {
            let poly = &polygons[poly_idx];
            for point_idx in 0..poly.len() {
                ret.insert(PolygonsPointIndex::new(polygons, poly_idx, point_idx));
            }
        }
        ret
    }

    /// The current implementation can check the same line segment multiple times,
    /// since the same line segment can occur in multiple cells if it it longer than the cell size of the SparsePointGridInclusive.
    ///
    /// We could skip the duplication by keeping a vector of vectors of bools.
    pub fn find_close(from: Point2LL, polygons: &Shape, loc_to_line: &LocToLineGrid, penalty_function: PenaltyFn) -> Option<ClosestPointPolygon> {
        let near_lines = loc_to_line.get_nearby(from, loc_to_line.base.cell_size());

        let mut best = Point2LL::new(0, 0);

        let mut closest_dist2_score = i64::MAX;
        let mut best_point_poly_idx = PolygonsPointIndex::new(std::ptr::null(), NO_INDEX, NO_INDEX);
        for point_poly_index in &near_lines {
            let poly = &polygons[point_poly_index.poly_idx];
            let p1 = poly[point_poly_index.point_idx];
            let p2 = poly[(point_poly_index.point_idx + 1) % poly.len()];

            let closest_here = la::get_closest_on_line_segment(from, p1, p2);
            let dist2_score = v_size2(from - closest_here) + penalty_function(closest_here) as i64;
            if dist2_score < closest_dist2_score {
                best = closest_here;
                closest_dist2_score = dist2_score;
                best_point_poly_idx = *point_poly_index;
            }
        }
        if best_point_poly_idx.poly_idx == NO_INDEX {
            None
        } else {
            Some(ClosestPointPolygon::with_poly_idx(
                best,
                best_point_poly_idx.point_idx,
                &polygons[best_point_poly_idx.poly_idx],
                best_point_poly_idx.poly_idx,
            ))
        }
    }

    pub fn find_close_polygon(
        from: &Polygon,
        destination: &Shape,
        destination_loc_to_line: &LocToLineGrid,
        penalty_function: PenaltyFn,
    ) -> Vec<(ClosestPointPolygon, ClosestPointPolygon)> {
        let mut ret = Vec::new();
        let mut p0_idx: i32 = from.len() as i32 - 1;
        let mut p0 = from[p0_idx as usize];
        let grid_size: i32 = destination_loc_to_line.base.cell_size() as i32;
        for p1_idx in 0..from.len() as u32 {
            let p1 = from[p1_idx as usize];
            let mut best_here = Self::find_close(p1, destination, destination_loc_to_line, penalty_function);
            if let Some(b) = best_here {
                ret.push((ClosestPointPolygon::new(p1, p1_idx as usize, from), b));
            }
            let p0p1 = p1 - p0;
            let mut dist_to_p1: i32 = v_size(p0p1) as i32;
            let mut middle_point_nr: u32 = 1;
            while dist_to_p1 > grid_size.wrapping_mul(2) {
                let x = p0 + normal(p0p1, middle_point_nr.wrapping_mul(grid_size as u32) as i64);
                dist_to_p1 -= grid_size;

                best_here = Self::find_close(x, destination, destination_loc_to_line, penalty_function);
                if let Some(b) = best_here {
                    ret.push((ClosestPointPolygon::new(x, p0_idx as usize, from), b));
                }
                middle_point_nr += 1;
            }
            p0 = p1;
            p0_idx = p1_idx as i32;
        }
        ret
    }

    pub fn get_next_point_with_distance(from: Point2LL, dist: i64, poly: &OpenPolyline, start_idx: i32, poly_start_idx: i32, result: &mut GivenDistPoint) -> bool {
        let mut prev_poly_point = poly[((start_idx + poly_start_idx) as usize) % poly.len()];

        let mut prev_idx = start_idx as u32;
        while (prev_idx as usize) < poly.len() {
            let next_idx = ((prev_idx as usize + 1 + poly_start_idx as usize) % poly.len()) as i32; // last checked segment is between last point in poly and poly[0]...
            let next_poly_point = poly[next_idx as usize];
            if !crate::geometry::point2ll::shorter_then(next_poly_point - from, dist) {
                //
                //                 x    r
                //      p.---------+---+------------.n
                //                L|  /
                //                 | / dist
                //                 |/
                //                f.
                //
                // f=from
                // p=prev_poly_point
                // n=next_poly_point
                // x= f projected on pn
                // r=result point at distance [dist] from f

                let pn = next_poly_point - prev_poly_point;

                if crate::geometry::point2ll::shorter_then(pn, 100) {
                    // when precision is limited
                    let middle = (next_poly_point + prev_poly_point) / 2;
                    let dist_to_middle = v_size(from - middle);
                    if dist_to_middle - dist < 100 && dist_to_middle - dist > -100 {
                        result.location = middle;
                        result.pos = prev_idx as i32;
                        return true;
                    } else {
                        prev_poly_point = next_poly_point;
                        prev_idx += 1;
                        continue;
                    }
                }

                let pf = from - prev_poly_point;
                let px = pn * (dot(pf, pn) / v_size(pn)) / v_size(pn);
                let xf = pf - px;

                if !crate::geometry::point2ll::shorter_then(xf, dist) {
                    // line lies wholly further than pn
                    prev_poly_point = next_poly_point;
                    prev_idx += 1;
                    continue;
                }

                let xr_dist = ((dist * dist - v_size2(xf)) as f64).sqrt() as i64; // inverse Pythagoras

                if v_size(pn - px) - xr_dist < 1 {
                    // r lies beyond n
                    prev_poly_point = next_poly_point;
                    prev_idx += 1;
                    continue;
                }

                let xr = pn * xr_dist / v_size(pn);
                let pr = px + xr;

                result.location = prev_poly_point + pr;
                result.pos = prev_idx as i32;
                return true;
            }
            prev_poly_point = next_poly_point;
            prev_idx += 1;
        }
        false
    }

    /// `walk(const ClosestPoint<LineType>& from, coord_t distance)`.
    pub fn walk<L: Polyline>(from: &ClosestPoint<L>, mut distance: Coord) -> ClosestPoint<L> {
        let poly = from.poly();
        let mut last_vertex = from.p();
        let mut next_vertex;
        let mut last_point_idx = from.point_idx;
        let mut point_idx = from.point_idx + 1;
        loop {
            if point_idx == poly.points().len() {
                point_idx = 0;
            }
            next_vertex = poly.points()[point_idx];
            distance -= v_size(last_vertex - next_vertex);
            if distance <= 0 {
                break;
            }
            last_vertex = next_vertex;
            last_point_idx = point_idx;
            point_idx += 1;
        }
        let result = next_vertex + normal(last_vertex - next_vertex, -distance);
        ClosestPoint::with_poly_idx(result, last_point_idx, from.poly, from.poly_idx)
    }

    pub fn get_next_parallel_intersection(start: &ClosestPointPolygon, line_to: Point2LL, dist: Coord, forward: bool) -> Option<ClosestPointPolygon> {
        // <--o--t-----y----< poly 1
        //       :     :
        // >---o :====>:shift
        //      \:     :
        //       s     x
        //        \    :
        //         \   :
        //          o--r----->  poly 2
        //  s=start
        //  r=result
        //  t=line_to

        let poly = start.poly();
        let s = start.p();
        let t = line_to;

        let st = t - s;
        let shift = normal(turn90_ccw(st), dist);

        let mut prev_vert = s;
        let mut prev_projected: Coord = 0;
        for next_point_nr in 0..poly.len() as u32 {
            let next_point_idx: u32 = if forward {
                ((start.point_idx + 1 + next_point_nr as usize) % poly.len()) as u32
            } else {
                ((start.point_idx.wrapping_sub(next_point_nr as usize).wrapping_add(poly.len())) % poly.len()) as u32
            };
            let next_vert = poly[next_point_idx as usize];
            let so = next_vert - s;
            let projected = dot(shift, so) / dist;
            if projected.abs() > dist {
                // segment crosses the line through xy (or the one on the other side of st)
                let segment_vector = next_vert - prev_vert;
                let segment_length = v_size(segment_vector);
                let projected_segment_length = (projected - prev_projected).abs();
                let sign: i16 = if projected > 0 { 1 } else { -1 };
                let projected_inter_segment_length = dist - sign as i64 * prev_projected; // add the prev_projected to dist if it is projected to the other side of the input line than where the intersection occurs.
                let inter_segment_length = segment_length * projected_inter_segment_length / projected_segment_length;
                let intersection = prev_vert + normal(next_vert - prev_vert, inter_segment_length);

                let mut vert_before_idx = next_point_idx as usize;
                if forward {
                    vert_before_idx = if next_point_idx > 0 { vert_before_idx - 1 } else { poly.len() - 1 };
                }
                debug_assert!(vert_before_idx < poly.len());
                return Some(ClosestPointPolygon::new(intersection, vert_before_idx, poly));
            }

            prev_vert = next_vert;
            prev_projected = projected;
        }

        None
    }

    pub fn polygon_collides_with_line_segment_grid(from: Point2LL, to: Point2LL, loc_to_line: &LocToLineGrid, collision_result: Option<&mut PolygonsPointIndex>) -> bool {
        let mut ret = false;
        let diff = to - from;
        if v_size2(diff) < 2 {
            // transformation matrix would fail
            return false;
        }

        let transformation_matrix = PointMatrix::from_point(diff);
        let transformed_from = transformation_matrix.apply(from);
        let transformed_to = transformation_matrix.apply(to);

        let mut result = PolygonsPointIndex::default();

        loc_to_line.process_line((from, to), |line_start: &PolygonsPointIndex| {
            let p0 = transformation_matrix.apply(line_start.p());
            let p1 = transformation_matrix.apply(line_start.next().p());

            if la::line_segments_collide(transformed_from, transformed_to, p0, p1) {
                result = *line_start;
                ret = true;
                return false;
            }
            true
        });
        if let Some(cr) = collision_result {
            *cr = result;
        }
        ret
    }

    pub fn polygon_collides_with_line_segment_transformed(
        poly: &[Point2LL],
        transformed_start_point: Point2LL,
        transformed_end_point: Point2LL,
        transformation_matrix: &PointMatrix,
    ) -> bool {
        let mut p0 = transformation_matrix.apply(*poly.last().unwrap());
        for &p1_ in poly {
            let p1 = transformation_matrix.apply(p1_);
            if la::line_segments_collide(transformed_start_point, transformed_end_point, p0, p1) {
                return true;
            }
            p0 = p1;
        }
        false
    }

    pub fn polygon_collides_with_line_segment(poly: &[Point2LL], start_point: Point2LL, end_point: Point2LL) -> bool {
        let diff = end_point - start_point;

        let transformation_matrix = PointMatrix::from_point(diff);
        let transformed_start_point = transformation_matrix.apply(start_point);
        let transformed_end_point = transformation_matrix.apply(end_point);

        Self::polygon_collides_with_line_segment_transformed(poly, transformed_start_point, transformed_end_point, &transformation_matrix)
    }

    pub fn polygons_collide_with_line_segment_transformed(
        polys: &Shape,
        transformed_start_point: Point2LL,
        transformed_end_point: Point2LL,
        transformation_matrix: &PointMatrix,
    ) -> bool {
        for poly in polys.iter() {
            if poly.is_empty() {
                continue;
            }
            if Self::polygon_collides_with_line_segment_transformed(poly, transformed_start_point, transformed_end_point, transformation_matrix) {
                return true;
            }
        }
        false
    }

    pub fn polygons_collide_with_line_segment(polys: &Shape, start_point: Point2LL, end_point: Point2LL) -> bool {
        if end_point == start_point {
            return false; // Zero-length line segments never collide.
        }
        let diff = end_point - start_point;

        let transformation_matrix = PointMatrix::from_point(diff);
        let transformed_start_point = transformation_matrix.apply(start_point);
        let transformed_end_point = transformation_matrix.apply(end_point);

        Self::polygons_collide_with_line_segment_transformed(polys, transformed_start_point, transformed_end_point, &transformation_matrix)
    }

    pub fn polygons_intersect(poly_a: &Polygon, poly_b: &Polygon) -> bool {
        let bba = AABB::from_points(poly_a);
        let bbb = AABB::from_points(poly_b);
        bba.hit(&bbb) && !poly_a.intersection(poly_b).is_empty()
    }

    pub fn polygon_outlines_adjacent(inner_poly: &Polygon, outer_poly: &Polygon, max_gap: Coord) -> bool {
        // Heuristic check if their AABBs are near first.
        let mut inner_aabb = AABB::from_points(inner_poly);
        let outer_aabb = AABB::from_points(outer_poly);
        inner_aabb.max += Point2LL::new(max_gap, max_gap); // Expand one of them by way of a "distance" by checking intersection with the expanded rectangle.
        inner_aabb.min -= Point2LL::new(max_gap, max_gap);
        if !inner_aabb.hit(&outer_aabb) {
            return false;
        }

        // Heuristic says they are near, now check for real.
        let max_gap2 = max_gap * max_gap;
        let outer_poly_size = outer_poly.len() as u32;
        for line_index in 0..outer_poly_size {
            let lp0 = outer_poly[line_index as usize];
            let lp1 = outer_poly[((line_index + 1) % outer_poly_size) as usize];
            for &inner_poly_point in inner_poly.iter() {
                if la::get_dist2_from_line_segment(lp0, inner_poly_point, lp1) < max_gap2 {
                    return true;
                }
            }
        }
        false
    }

    pub fn find_adjacent_polygons(adjacent_poly_indices: &mut Vec<u32>, poly: &Polygon, possible_adjacent_polys: &[&Polygon], max_gap: Coord) {
        // given a polygon, and a vector of polygons, return a vector containing the indices of the polygons that are adjacent to the given polygon
        for poly_idx in 0..possible_adjacent_polys.len() {
            if Self::polygon_outlines_adjacent(poly, possible_adjacent_polys[poly_idx], max_gap)
                || Self::polygon_outlines_adjacent(possible_adjacent_polys[poly_idx], poly, max_gap)
            {
                adjacent_poly_indices.push(poly_idx as u32);
            }
        }
    }

    pub fn relative_hamming_distance(poly_a: &Shape, poly_b: &Shape) -> f64 {
        let area_a = poly_a.area().abs();
        let area_b = poly_b.area().abs();
        let total_area = area_a + area_b;

        // If the total area is 0.0, we'd get a division by zero. Instead, only return 0.0 if they are exactly equal.
        let borders_allowed = true;
        if total_area == 0.0 {
            for polygon_a in poly_a.iter() {
                for &point in polygon_a.iter() {
                    if !poly_b.inside(point, borders_allowed) {
                        return 1.0;
                    }
                }
            }
            for polygon_b in poly_b.iter() {
                for &point in polygon_b.iter() {
                    if !poly_a.inside(point, borders_allowed) {
                        return 1.0;
                    }
                }
            }
            return 0.0; // All points are inside the other polygon, regardless of where the vertices are along the line.
        }

        let symmetric_difference = poly_a.xor_polygons(poly_b, PolyFillType::EvenOdd);
        let hamming_distance = symmetric_difference.area();
        hamming_distance / total_area
    }

    pub fn make_disc(mid: Point2LL, radius: Coord, steps: usize) -> Polygon {
        let pts = Self::make_circle_points(mid, radius, steps);
        Polygon::new(pts, false)
    }

    /// `makeCircle<ClosedPolyline>`.
    pub fn make_circle(mid: Point2LL, radius: Coord, segments: usize) -> ClosedPolyline {
        ClosedPolyline::new(Self::make_circle_points(mid, radius, segments), false)
    }

    /// The points of `makeCircle` (without the optional explicit closing point).
    pub fn make_circle_points(mid: Point2LL, radius: Coord, segments: usize) -> Vec<Point2LL> {
        let mut circle = Vec::with_capacity(segments);
        let step_angle = AngleRadians::new((std::f64::consts::PI * 2.0) / segments as f64);
        for step in 0..segments {
            let angle = AngleRadians::new(step as f64 * step_angle.value());
            circle.push(Self::make_circle_point(mid, radius, angle));
        }
        circle
    }

    pub fn make_circle_point(mid: Point2LL, radius: Coord, angle: AngleRadians) -> Point2LL {
        mid + Point2LL::new(cppstd::llrint(radius as f64 * angle.value().cos()), cppstd::llrint(radius as f64 * angle.value().sin()))
    }

    pub fn make_wheel(mid: Point2LL, inner_radius: Coord, outer_radius: Coord, semi_nb_spokes: usize, arc_angle_resolution: usize) -> ClosedPolyline {
        let mut wheel = ClosedPolyline::default();

        let nb_spokes = semi_nb_spokes * 2;
        let spoke_angle_step = crate::settings::types::TAU / nb_spokes as f64;
        let arc_angle_step = spoke_angle_step / arc_angle_resolution as f64;

        for spoke in 0..nb_spokes {
            let spoke_angle = spoke as f64 * spoke_angle_step;
            let radius = if spoke % 2 == 0 { inner_radius } else { outer_radius };

            for arc_part in 0..=arc_angle_resolution {
                let angle = spoke_angle + arc_part as f64 * arc_angle_step;
                wheel.points.push(Self::make_circle_point(mid, radius, AngleRadians::new(angle)));
            }
        }

        wheel
    }

    pub fn connect(input: &Shape) -> Shape {
        let mut ret = Shape::default();
        let mut parts = input.split_into_parts(true);
        for part in parts.iter_mut() {
            let n_holes = part.0.len();
            for hole_idx in 1..n_holes {
                let (outline_slice, rest) = part.0.lines.split_at_mut(1);
                let outline = &mut outline_slice[0];
                let hole = &mut rest[hole_idx - 1];
                let hole_point = hole[0];
                hole.points.push(hole_point);
                // find where the scanline passes the outline
                let mut best_segment_to_idx: usize = 0;
                let mut best_dist = Coord::MAX;
                let mut best_intersection_point = *outline.last().unwrap();

                let mut prev = *outline.last().unwrap();
                for point_idx in 0..outline.len() {
                    let here = outline[point_idx];
                    if here.y > hole_point.y && prev.y <= hole_point.y && here.y != prev.y {
                        let intersection_point = prev + (here - prev) * (hole_point.y - prev.y) / (here.y - prev.y);
                        let dist = hole_point.x - intersection_point.x;
                        if dist > 0 && dist < best_dist {
                            best_dist = dist;
                            best_segment_to_idx = point_idx;
                            best_intersection_point = intersection_point;
                        }
                    }
                    prev = here;
                }
                outline.points.splice(best_segment_to_idx..best_segment_to_idx, [best_intersection_point, best_intersection_point]);
                let hole_pts = hole.points.clone();
                outline.points.splice(best_segment_to_idx + 1..best_segment_to_idx + 1, hole_pts);
            }
            ret.push(part.0[0].clone());
        }
        ret
    }

    pub fn fix_self_intersections(epsilon: Coord, polygon: &mut Shape) {
        if epsilon < 1 {
            polygon.simplify_strict(PolyFillType::EvenOdd);
            return;
        }

        let half_epsilon = 10i64.max((epsilon + 1) / 2);

        // Points too close to line segments should be moved a little away from those line segments, but less than epsilon,
        //   so at least half-epsilon distance between points can still be guaranteed.
        let grid_size: Coord = 2000;
        let query_grid = Self::create_loc_to_line_grid(polygon, grid_size as i32);

        let move_dist = half_epsilon - 2;
        let half_epsilon_sqrd = half_epsilon * half_epsilon;

        let n = polygon.len();
        for poly_idx in 0..n {
            let pathlen = polygon[poly_idx].len();
            for point_idx in 0..pathlen {
                let mut pt = polygon[poly_idx][point_idx];
                for line in query_grid.get_nearby(pt, epsilon * 2) {
                    let line_next_idx = (line.point_idx + 1) % polygon[line.poly_idx].len();
                    if poly_idx == line.poly_idx && (point_idx == line.point_idx || point_idx == line_next_idx) {
                        continue;
                    }

                    let a = polygon[line.poly_idx][line.point_idx];
                    let b = polygon[line.poly_idx][line_next_idx];

                    if half_epsilon_sqrd >= v_size2(pt - la::get_closest_on_line_segment(pt, a, b)) {
                        let other = polygon[poly_idx][(point_idx + 1) % pathlen];
                        let vec = if la::point_is_left_of_line(other, a, b) > 0 { b - a } else { a - b };
                        let len = v_size(vec).max(1);
                        pt.x += (-vec.y * move_dist) / len;
                        pt.y += (vec.x * move_dist) / len;
                        polygon.lines[poly_idx].points[point_idx] = pt;
                    }
                }
            }
        }

        polygon.simplify_strict(PolyFillType::EvenOdd);
    }

    pub fn union_many_small(polygon: &Shape) -> Shape {
        if polygon.len() < 8 {
            return polygon.union_polygons();
        }

        let mut a = Shape::default();
        let mut b = Shape::default();
        for (i, path) in polygon.iter().enumerate() {
            if i % 2 == 0 {
                b.push(path.clone());
            } else {
                a.push(path.clone());
            }
        }
        Self::union_many_small(&a).union_polygons_with(&Self::union_many_small(&b), PolyFillType::NonZero)
    }

    pub fn clip_polygon_with_aabb(src: &Shape, aabb: &AABB) -> Shape {
        let mut out = Shape::default();
        for path in src.iter() {
            let mut poly = Polygon::default();

            let cnt = path.len();
            if cnt < 3 {
                return Shape::default();
            }

            const LEFT: i32 = 1;
            const RIGHT: i32 = 2;
            const TOP: i32 = 4;
            const BOTTOM: i32 = 8;

            let sides = |p: Point2LL| -> i32 {
                (p.x < aabb.min.x) as i32 * LEFT + (p.x > aabb.max.x) as i32 * RIGHT + (p.y < aabb.min.y) as i32 * BOTTOM + (p.y > aabb.max.y) as i32 * TOP
            };

            let mut sides_prev = sides(*path.last().unwrap());
            let mut sides_this = sides(path[0]);
            let last = cnt - 1;

            for i in 0..last {
                let sides_next = sides(path[i + 1]);
                if sides_this == 0 // This point is inside. Take it.
                    || (sides_prev & sides_this & sides_next) == 0
                // This point is outside. Take it only if the prev/next are not on the same side.
                {
                    poly.points.push(path[i]);
                    sides_prev = sides_this;
                }
                sides_this = sides_next;
            }

            // Repeat the above for the last point.
            if !poly.is_empty() {
                let sides_next = sides(poly[0]);
                if sides_this == 0 // The last point is inside. Take it.
                    || (sides_prev & sides_this & sides_next) == 0
                // The last point is outside and not inside the same sector as prev/next.
                {
                    poly.points.push(*path.last().unwrap());
                }
            }

            if !poly.is_empty() {
                out.push(poly);
            }
        }
        out
    }

    pub fn generate_circular_outset(center: Point2LL, inner_radius: Coord, outer_radius: Coord, line_width: Coord, circle_definition: usize) -> (ClosedLinesSet, Coord) {
        let mut outset = ClosedLinesSet::default();
        let semi_line_width = line_width / 2;
        let mut radius = inner_radius + semi_line_width;

        while radius + semi_line_width <= outer_radius {
            outset.push(Self::make_circle(center, radius, circle_definition));
            radius += line_width;
        }

        (outset, radius - semi_line_width)
    }

    pub fn generate_circular_inset(center: Point2LL, outer_radius: Coord, line_width: Coord, circle_definition: usize) -> ClosedLinesSet {
        let mut inset = ClosedLinesSet::default();
        let semi_line_width = line_width / 2;
        let mut radius = outer_radius - semi_line_width;

        while radius - semi_line_width >= line_width {
            inset.push(Self::make_circle(center, radius, circle_definition));
            radius -= line_width;
        }

        inset
    }
}
