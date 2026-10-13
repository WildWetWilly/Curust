//! `pathPlanning/LinePolygonsCrossings.h/.cpp` (+ `CombPath.h`, `CombPaths.h`): generating a combing
//! move from point a to point b, avoiding collision with other parts when moving through air.

use crate::geometry::matrix::PointMatrix;
use crate::geometry::point2ll::{scale_f64, shorter_then, v_size, Point2LL};
use crate::geometry::Shape;
use crate::utils::coord::Coord;
use crate::utils::polygon_utils::{LocToLineGrid, PolygonUtils};

/// `CombPath`: a single path either inside or outside the parts.
#[derive(Clone, Debug, Default)]
pub struct CombPath {
    pub points: Vec<Point2LL>,
    /// Whether the path crosses a boundary.
    pub cross_boundary: bool,
}

impl std::ops::Deref for CombPath {
    type Target = Vec<Point2LL>;
    fn deref(&self) -> &Vec<Point2LL> {
        &self.points
    }
}
impl std::ops::DerefMut for CombPath {
    fn deref_mut(&mut self) -> &mut Vec<Point2LL> {
        &mut self.points
    }
}

/// `CombPaths`: a list of paths alternating between inside a part and outside a part.
#[derive(Clone, Debug, Default)]
pub struct CombPaths {
    pub paths: Vec<CombPath>,
    /// Whether the path is one which moves through air.
    pub through_air: bool,
}

impl std::ops::Deref for CombPaths {
    type Target = Vec<CombPath>;
    fn deref(&self) -> &Vec<CombPath> {
        &self.paths
    }
}
impl std::ops::DerefMut for CombPaths {
    fn deref_mut(&mut self) -> &mut Vec<CombPath> {
        &mut self.paths
    }
}

/// A single point where a polygon crosses the scanline.
#[derive(Clone, Copy, Debug)]
struct Crossing {
    /// The index of the polygon which crosses the scanline
    poly_idx: usize,
    /// x coordinate of crossings between the polygon and the scanline.
    x: Coord,
    /// The index of the first point of the line segment which crosses the scanline
    point_idx: usize,
}

pub struct LinePolygonsCrossings<'a> {
    /// All crossings of polygons in the boundary with the scanline.
    crossings: Vec<Crossing>,
    /// The boundary not to cross during combing.
    boundary: &'a Shape,
    /// Mapping from locations to line segments of the boundary
    loc_to_line_grid: &'a LocToLineGrid,
    start_point: Point2LL,
    end_point: Point2LL,
    /// The distance used to move outside or inside so that a boundary point doesn't intersect with the boundary anymore.
    dist_to_move_boundary_point_outside: i64,
    transformation_matrix: PointMatrix,
    transformed_start_point: Point2LL,
    transformed_end_point: Point2LL,
}

impl<'a> LinePolygonsCrossings<'a> {
    fn new(boundary: &'a Shape, loc_to_line_grid: &'a LocToLineGrid, start: Point2LL, end: Point2LL, dist_to_move_boundary_point_outside: i64) -> Self {
        LinePolygonsCrossings {
            crossings: Vec::new(),
            boundary,
            loc_to_line_grid,
            start_point: start,
            end_point: end,
            dist_to_move_boundary_point_outside,
            transformation_matrix: PointMatrix::default(),
            transformed_start_point: Point2LL::default(),
            transformed_end_point: Point2LL::default(),
        }
    }

    /// The main function of this class: calculate one combing path within the boundary.
    #[allow(clippy::too_many_arguments)]
    pub fn comb(
        boundary: &Shape,
        loc_to_line_grid: &LocToLineGrid,
        start_point: Point2LL,
        end_point: Point2LL,
        comb_path: &mut CombPath,
        dist_to_move_boundary_point_outside: i64,
        max_comb_distance_ignored: i64,
        fail_on_unavoidable_obstacles: bool,
    ) -> bool {
        let mut lpc = LinePolygonsCrossings::new(boundary, loc_to_line_grid, start_point, end_point, dist_to_move_boundary_point_outside);
        lpc.generate_combing_path(comb_path, max_comb_distance_ignored, fail_on_unavoidable_obstacles)
    }

    fn calc_scanline_crossings(&mut self, fail_on_unavoidable_obstacles: bool) -> bool {
        for poly_idx in 0..self.boundary.len() {
            let poly = &self.boundary[poly_idx];
            let mut p0 = self.transformation_matrix.apply(poly[poly.len().wrapping_sub(1)]);
            for point_idx in 0..poly.len() {
                let p1 = self.transformation_matrix.apply(poly[point_idx]);
                let ts = self.transformed_start_point;
                if (p0.y >= ts.y && p1.y <= ts.y) || (p1.y >= ts.y && p0.y <= ts.y) {
                    // if line segment crosses the line through the transformed start and end point (aka scanline)
                    if p1.y == p0.y {
                        // Line segment is parallel with the scanline. That means that both endpoints lie on the scanline, so they will have intersected with the adjacent line.
                        p0 = p1;
                        continue;
                    }
                    let x = p0.x + (p1.x - p0.x) * (ts.y - p0.y) / (p1.y - p0.y); // intersection point between line segment and the scanline

                    if x >= ts.x && x <= self.transformed_end_point.x && !((p1.y == ts.y && p1.y < p0.y) || (p0.y == ts.y && p0.y < p1.y)) {
                        // perform edge case only for line segments on and below the scanline, not for line segments on and above.
                        // \/ will be no crossings and /\ two, but most importantly | will be one crossing.
                        self.crossings.push(Crossing { poly_idx, x, point_idx });
                    }
                }
                p0 = p1;
            }

            if fail_on_unavoidable_obstacles && self.crossings.len() % 2 == 1 {
                // if start area and end area are not the same
                return false;
            }
        }
        // order crossings by increasing x
        self.crossings.sort_by(|a, b| a.x.cmp(&b.x)); // stable
        true
    }

    fn line_segment_collides_with_boundary(&mut self) -> bool {
        let diff = self.end_point - self.start_point;

        self.transformation_matrix = PointMatrix::from_point(diff);
        self.transformed_start_point = self.transformation_matrix.apply(self.start_point);
        self.transformed_end_point = self.transformation_matrix.apply(self.end_point);

        let ts = self.transformed_start_point;
        for poly in self.boundary.iter() {
            let mut p0 = self.transformation_matrix.apply(*poly.last().unwrap());
            for p1_ in poly.iter() {
                let p1 = self.transformation_matrix.apply(*p1_);
                // when the boundary just touches the line don't disambiguate between the boundary moving on to actually cross the line
                // and the boundary bouncing back, resulting in not a real collision - to keep the algorithm simple.
                if p1.y != p0.y && ((p0.y >= ts.y && p1.y <= ts.y) || (p1.y >= ts.y && p0.y <= ts.y)) {
                    let x = p0.x + (p1.x - p0.x) * (ts.y - p0.y) / (p1.y - p0.y);

                    if x > ts.x && x < self.transformed_end_point.x {
                        return true;
                    }
                }
                p0 = p1;
            }
        }

        false
    }

    fn generate_combing_path(&mut self, comb_path: &mut CombPath, max_comb_distance_ignored: i64, fail_on_unavoidable_obstacles: bool) -> bool {
        if shorter_then(self.end_point - self.start_point, max_comb_distance_ignored) || !self.line_segment_collides_with_boundary() {
            // We're not crossing any boundaries. So skip the comb generation.
            comb_path.push(self.start_point);
            comb_path.push(self.end_point);
            return true;
        }

        let success = self.calc_scanline_crossings(fail_on_unavoidable_obstacles);
        if !success {
            return false;
        }

        let mut basic_path = CombPath::default();
        self.generate_basic_combing_path(&mut basic_path);
        self.optimize_path(&mut basic_path, comb_path);
        true
    }

    fn generate_basic_combing_path(&self, comb_path: &mut CombPath) {
        // crossings are ordered by increasing x
        let mut i = 0usize;
        while i < self.crossings.len() {
            // find the next crossing that belongs to the same polygon
            let mut j = i + 1;
            while j < self.crossings.len() {
                if self.crossings[i].poly_idx == self.crossings[j].poly_idx {
                    // comb between the two crossings
                    self.generate_basic_combing_path_between(&self.crossings[i], &self.crossings[j], comb_path);
                    // update outer loop variable to skip any crossings on other polygons
                    i = j;
                    break;
                }
                j += 1;
            }
            i += 1;
        }
        comb_path.push(self.end_point);
    }

    fn generate_basic_combing_path_between(&self, min: &Crossing, max: &Crossing, comb_path: &mut CombPath) {
        // minimise the path length by measuring the length of both paths around the polygon so we can determine the shorter path

        let poly = &self.boundary[min.poly_idx];
        let n = poly.len();
        comb_path.push(self.transformation_matrix.unapply(Point2LL::new(min.x - self.dist_to_move_boundary_point_outside.abs(), self.transformed_start_point.y)));

        // follow the path in the same direction as the winding order of the boundary polygon
        let mut fwd_points: Vec<Point2LL> = Vec::new();
        let mut prev = *comb_path.last().unwrap();
        let mut fwd_len: Coord = 0;
        let mut point_idx = min.point_idx as u32;
        while point_idx != max.point_idx as u32 {
            let p = PolygonUtils::get_boundary_point_with_offset(&poly.points, point_idx, self.dist_to_move_boundary_point_outside);
            fwd_points.push(p);
            fwd_len += v_size(p - prev);
            prev = p;
            point_idx = if (point_idx as usize) < n - 1 { point_idx + 1 } else { 0 };
        }

        let last = self.transformation_matrix.unapply(Point2LL::new(max.x + self.dist_to_move_boundary_point_outside.abs(), self.transformed_start_point.y));

        if let Some(b) = fwd_points.last() {
            fwd_len += v_size(last - *b);
        }

        // follow the path in the opposite direction of the winding order of the boundary polygon
        let mut rev_points: Vec<Point2LL> = Vec::new();
        prev = *comb_path.last().unwrap();
        let mut rev_len: Coord = 0;
        let min_idx: u32 = if min.point_idx == 0 { (n - 1) as u32 } else { (min.point_idx - 1) as u32 };
        let max_idx: u32 = if max.point_idx == 0 { (n - 1) as u32 } else { (max.point_idx - 1) as u32 };
        let mut point_idx = min_idx;
        while point_idx != max_idx {
            let p = PolygonUtils::get_boundary_point_with_offset(&poly.points, point_idx, self.dist_to_move_boundary_point_outside);
            rev_points.push(p);
            rev_len += v_size(p - prev);
            prev = p;
            if rev_len > fwd_len {
                // this path is already longer than the forward path so there's no point in carrying on
                break;
            }
            point_idx = if point_idx > 0 { point_idx - 1 } else { (n - 1) as u32 };
        }

        if let Some(b) = rev_points.last() {
            rev_len += v_size(last - *b);
        }

        // use the points from the shortest path
        for p in if fwd_len < rev_len { &fwd_points } else { &rev_points } {
            comb_path.push(*p);
        }
        comb_path.push(last);
    }

    fn collides(&self, a: Point2LL, b: Point2LL) -> bool {
        PolygonUtils::polygon_collides_with_line_segment_grid(a, b, self.loc_to_line_grid, None)
    }

    fn optimize_path(&self, comb_path: &mut CombPath, optimized_comb_path: &mut CombPath) -> bool {
        optimized_comb_path.push(self.start_point);
        for point_idx in 1..comb_path.len() {
            if comb_path[point_idx] == comb_path[point_idx - 1] {
                // Two points are the same. Skip the second.
                continue;
            }
            let current_point = *optimized_comb_path.last().unwrap();
            if self.collides(current_point, comb_path[point_idx]) {
                if self.collides(current_point, comb_path[point_idx - 1]) {
                    comb_path.cross_boundary = true;
                }
                // before we add this point, can we discard some of the previous points and still avoid clashing with the combing boundary?
                let max_short_circuit_len: i32 = 1 << 3; // test distances of 8, 4, 2, 1
                let mut n = max_short_circuit_len.min(optimized_comb_path.len() as i32) as usize;
                while n > 0 {
                    if optimized_comb_path.len() > n && !self.collides(optimized_comb_path[optimized_comb_path.len() - n - 1], comb_path[point_idx - 1]) {
                        // we can remove n points from the path without it clashing with the combing boundary
                        for _ in 0..n {
                            optimized_comb_path.pop();
                        }
                        break;
                    }
                    n >>= 1;
                }
                optimized_comb_path.push(comb_path[point_idx - 1]);

                if point_idx == 1 {
                    // we have just inserted the first point in the path that touches the boundary
                    // and as the first line in the comb is pointing from the start point towards the
                    // end point, we can maybe save some travel by replacing this first point with another
                    // that also lies on the second path of the comb.
                    let mut p = *optimized_comb_path.last().unwrap();
                    for frac in [0.9, 0.9, 0.7, 0.5] {
                        // slide p towards the second point in the comb path
                        p = comb_path[1] + scale_f64(p - comb_path[1], frac);
                        if !self.collides(self.start_point, p) {
                            // using the new corner doesn't cause a conflict
                            *optimized_comb_path.last_mut().unwrap() = p;
                        } else {
                            // quit loop and keep what we know to be good
                            break;
                        }
                    }
                }
            } else {
                // : dont add the newest point
                while optimized_comb_path.len() > 1 {
                    if self.collides(optimized_comb_path[optimized_comb_path.len() - 2], comb_path[point_idx]) {
                        break;
                    } else {
                        optimized_comb_path.pop();
                    }
                }
            }
        }

        if optimized_comb_path.len() > 1 {
            let n = optimized_comb_path.len();
            // the penultimate corner may be deleted if the resulting path doesn't conflict with the boundary
            if !self.collides(optimized_comb_path[n - 2], *comb_path.last().unwrap()) {
                optimized_comb_path.pop();
            } else {
                // that wasn't possible so try and move the penultimate corner without conficting with the boundary
                // in exactly the same way as we did at the start of the path
                for frac in [0.9, 0.9, 0.7, 0.5] {
                    // make a new point between the penultimate corner and the corner before that
                    let p = optimized_comb_path[n - 2] + scale_f64(optimized_comb_path[n - 1] - optimized_comb_path[n - 2], frac);
                    if !self.collides(p, *comb_path.last().unwrap()) {
                        // using the new corner doesn't cause a conflict
                        optimized_comb_path[n - 1] = p;
                    } else {
                        // quit loop and keep what we know to be good
                        break;
                    }
                }
            }
        }

        optimized_comb_path.push(*comb_path.last().unwrap());
        true
    }
}
