//! `utils/linearAlg2D.h`.

use crate::geometry::matrix::{Point3Matrix, PointMatrix};
use crate::geometry::point2ll::*;
use crate::utils::coord::Coord;
use std::f64::consts::PI;

#[inline]
pub fn point_lies_on_the_right_of_line(p: Point2LL, p0: Point2LL, p1: Point2LL) -> i16 {
    // no tests unless the segment p0-p1 is at least partly at, or to right of, p.X
    if p0.x.max(p1.x) >= p.x {
        let pd_y = p1.y - p0.y;
        if pd_y < 0 {
            // p0->p1 is 'falling'
            if p1.y <= p.y && p0.y > p.y {
                // candidate
                // dx > 0 if intersection is to right of p.X
                let dx = (p1.x - p0.x) * (p1.y - p.y) - (p1.x - p.x) * pd_y;
                if dx == 0 {
                    // includes p == p1
                    return 0;
                }
                if dx > 0 {
                    return 1;
                }
            }
        } else if p.y >= p0.y {
            if p.y < p1.y {
                // candidate for p0->p1 'rising' and includes p.Y
                // dx > 0 if intersection is to right of p.X
                let dx = (p1.x - p0.x) * (p.y - p0.y) - (p.x - p0.x) * pd_y;
                if dx == 0 {
                    // includes p == p0
                    return 0;
                }
                if dx > 0 {
                    return 1;
                }
            } else if p.y == p1.y {
                // some special cases here, points on border:
                // - p1 exactly matches p (might otherwise be missed)
                // - p0->p1 exactly horizontal, and includes p.
                // (we already tested std::max(p0.X,p1.X) >= p.X )
                if p.x == p1.x || (pd_y == 0 && p0.x.min(p1.x) <= p.x) {
                    return 0;
                }
            }
        }
    }
    -1
}

/// `lineLineIntersection(p1, p2, p3, p4, float& t, float& u)` (computed in single precision).
pub fn line_line_intersection_tu(p1: Point2LL, p2: Point2LL, p3: Point2LL, p4: Point2LL) -> Option<(f32, f32)> {
    let x1mx2 = (p1.x - p2.x) as f32;
    let x1mx3 = (p1.x - p3.x) as f32;
    let x3mx4 = (p3.x - p4.x) as f32;
    let y1my2 = (p1.y - p2.y) as f32;
    let y1my3 = (p1.y - p3.y) as f32;
    let y3my4 = (p3.y - p4.y) as f32;
    let mut t = x1mx3 * y3my4 - y1my3 * x3mx4;
    let mut u = x1mx3 * y1my2 - y1my3 * x1mx2;
    let div = x1mx2 * y3my4 - y1my2 * x3mx4;
    if div == 0.0f32 {
        return None;
    }
    t /= div;
    u /= div;
    Some((t, u))
}

/// Same as [`line_line_intersection_tu`] but also returns t/u in the failure case (C++ writes them
/// before checking the divisor).
pub fn segment_segment_intersection(p1: Point2LL, p2: Point2LL, p3: Point2LL, p4: Point2LL) -> Option<(f32, f32)> {
    match line_line_intersection_tu(p1, p2, p3, p4) {
        Some((t, u)) if t >= 0.0 && u >= 0.0 && t <= 1.0 && u <= 1.0 => Some((t, u)),
        _ => None,
    }
}

/// `lineLineIntersection(a, b, c, d, Point2LL& output)`.
pub fn line_line_intersection(a: Point2LL, b: Point2LL, c: Point2LL, d: Point2LL) -> Option<Point2LL> {
    let (t, _u) = line_line_intersection_tu(a, b, c, d)?;
    let result = a + scale_f32(b - a, t);
    if result.x.abs() > i32::MAX as i64 || result.y.abs() > i32::MAX as i64 {
        return None;
    }
    Some(result)
}

pub fn line_horizontal_line_intersection(p1: Point2LL, p2: Point2LL, line_y: Coord) -> Option<Coord> {
    if p1.y == p2.y {
        return None;
    }
    if p1.x == p2.x {
        return Some(p1.x);
    }
    if line_y == p1.y {
        return Some(p1.x);
    }
    if line_y == p2.y {
        return Some(p2.x);
    }
    let coeff_a = (p2.y - p1.y) as f64 / (p2.x - p1.x) as f64;
    let coeff_b = p1.y as f64 - coeff_a * p1.x as f64;
    Some(cppstd::llrint((line_y as f64 - coeff_b) / coeff_a))
}

pub fn segment_horizontal_line_intersection(p1: Point2LL, p2: Point2LL, line_y: Coord) -> Option<Coord> {
    let (min_y, max_y) = if p2.y < p1.y { (p2.y, p1.y) } else { (p1.y, p2.y) };
    if min_y > line_y || max_y < line_y {
        return None;
    }
    line_horizontal_line_intersection(p1, p2, line_y)
}

#[inline]
pub fn point_is_projected_beyond_line(from: Point2LL, a: Point2LL, b: Point2LL) -> i16 {
    let vec = b - a;
    let point_vec = from - a;
    let dot_prod = dot(point_vec, vec);
    if dot_prod < 0 {
        // point is projected before start of line
        return -1;
    }
    if dot_prod > v_size2(vec) {
        // point is projected after end of line
        return 1;
    }
    0
}

#[inline]
pub fn get_closest_on_line_segment(from: Point2LL, p0: Point2LL, p1: Point2LL) -> Point2LL {
    let direction = p1 - p0;
    let to_from = from - p0;
    let projected_x = dot(to_from, direction);
    let x_p0 = 0;
    let x_p1 = v_size2(direction);
    if x_p1 == 0 {
        // Line segment has length 0.
        return p0;
    }
    if projected_x <= x_p0 {
        // Projection is beyond p0.
        return p0;
    }
    if projected_x >= x_p1 {
        // Projection is beyond p1.
        return p1;
    }
    // Projection is between p0 and p1.
    // Return direction-normalised projection (projected_x / vSize(direction)) on direction vector.
    // projected_x / vSize(direction) is the length of the projection, so multiply by direction and divide by
    // vSize(direction) again to get the actual point.
    p0 + direction * projected_x / x_p1
}

#[inline]
pub fn get_closest_on_line(from: Point2LL, p0: Point2LL, p1: Point2LL) -> Point2LL {
    if p1 == p0 {
        return p0;
    }
    let direction = p1 - p0;
    let to_from = from - p0;
    let projected_x = dot(to_from, direction);
    p0 + direction * (projected_x / v_size(direction)) / v_size(direction)
}

pub fn get_closest_connection(a1: Point2LL, a2: Point2LL, b1: Point2LL, b2: Point2LL) -> (Point2LL, Point2LL) {
    let b1_on_a = get_closest_on_line_segment(b1, a1, a2);
    let b1_on_a_dist2 = v_size2(b1_on_a - b1);
    let b2_on_a = get_closest_on_line_segment(b2, a1, a2);
    let b2_on_a_dist2 = v_size2(b2_on_a - b2);
    let a1_on_b = get_closest_on_line_segment(a1, b1, b2);
    let a1_on_b_dist2 = v_size2(a1_on_b - a1);
    let a2_on_b = get_closest_on_line_segment(a1, b1, b2); // (sic: C++ uses a1 here)
    let a2_on_b_dist2 = v_size2(a2_on_b - a2);
    if b1_on_a_dist2 < b2_on_a_dist2 && b1_on_a_dist2 < a1_on_b_dist2 && b1_on_a_dist2 < a2_on_b_dist2 {
        (b1_on_a, b1)
    } else if b2_on_a_dist2 < a1_on_b_dist2 && b2_on_a_dist2 < a2_on_b_dist2 {
        (b2_on_a, b2)
    } else if a1_on_b_dist2 < a2_on_b_dist2 {
        (a1, a1_on_b)
    } else {
        (a2, a2_on_b)
    }
}

/// `getDist2FromLineSegment(a, b, c, b_is_beyond_ac)`: squared distance from b to the segment a-c.
/// Returns (distance squared, b_is_beyond_ac).
#[inline]
pub fn get_dist2_from_line_segment_beyond(a: Point2LL, b: Point2LL, c: Point2LL) -> (Coord, i16) {
    /*
     *     a,
     *     /|
     *    / |
     * b,/__|, x
     *   \  |
     *    \ |
     *     \|
     *      'c
     *
     * x = b projected on ac
     * ax = ab dot ac / vSize(ac)
     * xb = ab - ax
     * error = vSize(xb)
     */
    let ac = c - a;
    let ac_size = v_size(ac);
    let ab = b - a;
    if ac_size == 0 {
        let ab_dist2 = v_size2(ab);
        return (ab_dist2, 0);
    }
    let projected_x = dot(ab, ac);
    let ax_size = projected_x / ac_size;
    if ax_size < 0 {
        // b is 'before' segment ac
        return (v_size2(ab), -1);
    }
    if ax_size > ac_size {
        // b is 'after' segment ac
        return (v_size2(b - c), 1);
    }
    let ax = ac * ax_size / ac_size;
    let bx = ab - ax;
    (v_size2(bx), 0)
}

#[inline]
pub fn get_dist2_from_line_segment(a: Point2LL, b: Point2LL, c: Point2LL) -> Coord {
    get_dist2_from_line_segment_beyond(a, b, c).0
}

pub fn line_segments_are_closer_than(a: Point2LL, b: Point2LL, c: Point2LL, d: Point2LL, max_dist: Coord) -> bool {
    let max_dist2 = max_dist * max_dist;
    get_dist2_from_line_segment(a, c, b) <= max_dist2
        || get_dist2_from_line_segment(a, d, b) <= max_dist2
        || get_dist2_from_line_segment(c, a, d) <= max_dist2
        || get_dist2_from_line_segment(c, b, d) <= max_dist2
}

pub fn get_dist2_between_line_segments(a: Point2LL, b: Point2LL, c: Point2LL, d: Point2LL) -> Coord {
    get_dist2_from_line_segment(a, c, b).min(
        get_dist2_from_line_segment(a, d, b).min(get_dist2_from_line_segment(c, a, d).min(get_dist2_from_line_segment(c, b, d))),
    )
}

pub fn line_segments_collide(a_from_transformed: Point2LL, a_to_transformed: Point2LL, mut b_from_transformed: Point2LL, mut b_to_transformed: Point2LL) -> bool {
    if (b_from_transformed.y >= a_from_transformed.y && b_to_transformed.y <= a_from_transformed.y)
        || (b_to_transformed.y >= a_from_transformed.y && b_from_transformed.y <= a_from_transformed.y)
    {
        // line b intersects with the line through a
        if b_to_transformed.y == b_from_transformed.y {
            if b_to_transformed.x < b_from_transformed.x {
                std::mem::swap(&mut b_to_transformed.x, &mut b_from_transformed.x);
            }
            if b_from_transformed.x > a_to_transformed.x {
                return false;
            }
            if b_to_transformed.x < a_from_transformed.x {
                return false;
            }
            return true;
        } else {
            let x = b_from_transformed.x
                + (b_to_transformed.x - b_from_transformed.x) * (a_from_transformed.y - b_from_transformed.y) / (b_to_transformed.y - b_from_transformed.y);
            if x >= a_from_transformed.x && x <= a_to_transformed.x {
                return true;
            }
        }
    }
    false
}

/// Angle at b between a-b and b-c, measured counter clockwise ("left") in [0, 2*pi).
pub fn get_angle_left(a: Point2LL, b: Point2LL, c: Point2LL) -> f64 {
    let ba = a - b;
    let bc = c - b;
    let dott = dot(ba, bc); // dot product
    let det = ba.x * bc.y - ba.y * bc.x; // determinant
    if det == 0 {
        if (ba.x != 0 && (ba.x > 0) == (bc.x > 0)) || (ba.x == 0 && (ba.y > 0) == (bc.y > 0)) {
            return 0.0; // pointy bit
        } else {
            return PI; // straight bit
        }
    }
    let angle = -(det as f64).atan2(dott as f64); // from -pi to pi
    if angle >= 0.0 {
        angle
    } else {
        PI * 2.0 + angle
    }
}

#[inline]
pub fn point_is_left_of_line(p: Point2LL, a: Point2LL, b: Point2LL) -> Coord {
    (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x)
}

pub fn get_point_on_line_with_dist(p: Point2LL, a: Point2LL, b: Point2LL, dist: Coord) -> Option<Point2LL> {
    //         result
    //         v
    //   b<----r---a.......x
    //          '-.        :
    //              '-.    :
    //                  '-.p
    let ab = b - a;
    let ab_size = v_size(ab);
    let ap = p - a;
    let ax_size = if ab_size < 50 { dot(normal(ab, 1000), ap) / 1000 } else { dot(ab, ap) / ab_size };
    let ap_size2 = v_size2(ap);
    let px_size = ((0 as Coord).max(ap_size2 - ax_size * ax_size) as f64).sqrt() as Coord;
    if px_size > dist {
        return None;
    }
    let xr_size = ((dist * dist - px_size * px_size) as f64).sqrt() as Coord;
    if ax_size <= 0 {
        // x lies before ab
        let ar_size = xr_size + ax_size;
        if ar_size < 0 || ar_size > ab_size {
            // r lies outside of ab
            None
        } else {
            Some(a + normal(ab, ar_size))
        }
    } else if ax_size >= ab_size {
        // x lies after ab
        //         result
        //         v
        //   a-----r-------b............x
        //          '-.                 :
        //              '-.             :
        //                  '-.         :
        //                      '-.     :
        //                          '-. :
        //                              p
        let ar_size = ax_size - xr_size;
        if ar_size < 0 || ar_size > ab_size {
            // r lies outside of ab
            None
        } else {
            Some(a + normal(ab, ar_size))
        }
    } else {
        // x lies on ab
        //            result is either or
        //         v                       v
        //   a-----r-----------x-----------r----b
        //          '-.        :        .-'
        //              '-.    :    .-'
        //                  '-.p.-'
        //           or there is not result:
        //         v                       v
        //         r   a-------x---b       r
        //          '-.        :        .-'
        //              '-.    :    .-'
        //                  '-.p.-'
        // try r in both directions
        let ar1_size = ax_size - xr_size;
        if ar1_size >= 0 {
            return Some(a + normal(ab, ar1_size));
        }
        let ar2_size = ax_size + xr_size;
        if ar2_size < ab_size {
            return Some(a + normal(ab, ar2_size));
        }
        None
    }
}

pub fn get_dist2_from_line(p: Point2LL, a: Point2LL, b: Point2LL) -> Coord {
    let dist = get_dist_from_line(p, a, b);
    dist * dist
}

pub fn get_dist_from_line(p: Point2LL, a: Point2LL, b: Point2LL) -> Coord {
    //  x.......a------------b
    //  :
    //  :
    //  p
    // return px_size
    let vab = b - a;
    let vap = p - a;
    let ab_size = v_size(vab) as f64;
    if ab_size == 0.0 {
        // Line of 0 length. Assume it's a line perpendicular to the direction to p.
        return v_size(vap);
    }
    let area_times_two = ((p.x - b.x) * (p.y - a.y) + (a.x - p.x) * (p.y - b.y)).abs(); // Shoelace formula, factored
    (area_times_two as f64 / ab_size) as Coord
}

#[inline]
pub fn is_acute_corner(a: Point2LL, b: Point2LL, c: Point2LL) -> Coord {
    let ba = a - b;
    let bc = c - b;
    dot(ba, bc)
}

pub fn rotate_around(middle: Point2LL, rotation: f64) -> Point3Matrix {
    let rotation_matrix = PointMatrix::from_degrees(rotation);
    let rotation_matrix_homogeneous = Point3Matrix::from_point_matrix(&rotation_matrix);
    Point3Matrix::translate(middle).compose(&rotation_matrix_homogeneous).compose(&Point3Matrix::translate(-middle))
}

pub fn is_inside_corner(a: Point2LL, b: Point2LL, c: Point2LL, query_point: Point2LL) -> bool {
    /*
     Visualisation for the algorithm below:

                 query
                   |
                   |
                   |
     perp-----------b
                   / \       (note that the lines
                  /   \      AB and AC are normalized
                 /     \     to 10000 units length)
                a       c
     */
    const NORMAL_LENGTH: Coord = 10000; // Create a normal vector of reasonable length in order to reduce rounding error.
    let ba = normal(a - b, NORMAL_LENGTH);
    let bc = normal(c - b, NORMAL_LENGTH);
    let bq = query_point - b;
    let perpendicular = turn90_ccw(bq); // The query projects to this perpendicular to coordinate 0.
    let project_a_perpendicular = dot(ba, perpendicular); // Project vertex A on the perpendicular line.
    let project_c_perpendicular = dot(bc, perpendicular); // Project vertex C on the perpendicular line.
    if (project_a_perpendicular > 0) != (project_c_perpendicular > 0) {
        // Query is between A and C on the projection.
        project_a_perpendicular > 0 // Due to the winding order of corner ABC, this means that the query is inside.
    } else {
        // Beyond either A or C, but it could still be inside of the polygon.
        let project_a_parallel = dot(ba, bq); // Project not on the perpendicular, but on the original.
        let project_c_parallel = dot(bc, bq);

        // Either:
        //  * A is to the right of B (project_a_perpendicular > 0) and C is below A (project_c_parallel < project_a_parallel), or
        //  * A is to the left of B (project_a_perpendicular < 0) and C is above A (project_c_parallel > project_a_parallel).
        (project_c_parallel < project_a_parallel) == (project_a_perpendicular > 0)
    }
}

pub fn get_bisector_vector(intersect: Point2LL, a: Point2LL, b: Point2LL, vec_len: Coord) -> Point2LL {
    let a0 = a - intersect;
    let b0 = b - intersect;
    (((a0 * vec_len) / (1i64).max(v_size(a0))) + ((b0 * vec_len) / (1i64).max(v_size(b0)))) / 2
}
