//! Port of `boost/polygon/detail/voronoi_predicates.hpp` for `voronoi_ctype_traits<int32>`.
#![allow(non_snake_case)]
#![allow(clippy::too_many_arguments)]

use crate::ctypes::{ulp_cmp, BigInt, Efpt, UlpResult};
use crate::robust_fpt::{RobustDif, RobustFpt, RobustSqrtExpr as SE};
use crate::structures::{BeachLineKey, CircleEvent, Point2d, SiteEvent};

pub const ULPS: u32 = 64;
pub const ULPSX2: u32 = 128;
const ULPS_F: f64 = 64.0;

type RF = RobustFpt;

#[inline]
fn rf(v: f64, e: f64) -> RF {
    RobustFpt::with_error(v, e)
}

#[inline]
fn f(v: i32) -> f64 {
    v as f64
}

#[inline]
fn l(v: i32) -> i64 {
    v as i64
}

#[inline]
fn bi(v: i64) -> BigInt {
    BigInt::from_i64(v)
}

#[inline]
pub fn is_vertical_pts(p1: &Point2d, p2: &Point2d) -> bool {
    p1.x == p2.x
}

#[inline]
pub fn is_vertical(site: &SiteEvent) -> bool {
    is_vertical_pts(&site.point0, &site.point1)
}

/// Robust cross product a1 * b2 - b1 * a2 with 1 EPS relative error.
pub fn robust_cross_product(a1_: i64, b1_: i64, a2_: i64, b2_: i64) -> f64 {
    let a1 = (if a1_ < 0 { a1_.wrapping_neg() } else { a1_ }) as u64;
    let b1 = (if b1_ < 0 { b1_.wrapping_neg() } else { b1_ }) as u64;
    let a2 = (if a2_ < 0 { a2_.wrapping_neg() } else { a2_ }) as u64;
    let b2 = (if b2_ < 0 { b2_.wrapping_neg() } else { b2_ }) as u64;
    let l = a1.wrapping_mul(b2);
    let r = b1.wrapping_mul(a2);
    if (a1_ < 0) ^ (b2_ < 0) {
        if (a2_ < 0) ^ (b1_ < 0) {
            if l > r {
                -((l - r) as f64)
            } else {
                (r - l) as f64
            }
        } else {
            -(l.wrapping_add(r) as f64)
        }
    } else if (a2_ < 0) ^ (b1_ < 0) {
        l.wrapping_add(r) as f64
    } else if l < r {
        -((r - l) as f64)
    } else {
        (l - r) as f64
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    Right = -1,
    Collinear = 0,
    Left = 1,
}

#[inline]
pub fn ot_eval_f(value: f64) -> Orientation {
    if value == 0.0 {
        return Orientation::Collinear;
    }
    if value < 0.0 {
        Orientation::Right
    } else {
        Orientation::Left
    }
}

#[inline]
pub fn ot_eval_rf(value: &RF) -> Orientation {
    ot_eval_f(value.fpv())
}

#[inline]
pub fn ot_eval4(dif_x1: i64, dif_y1: i64, dif_x2: i64, dif_y2: i64) -> Orientation {
    ot_eval_f(robust_cross_product(dif_x1, dif_y1, dif_x2, dif_y2))
}

pub fn ot_eval_pts(point1: &Point2d, point2: &Point2d, point3: &Point2d) -> Orientation {
    let dx1 = l(point1.x) - l(point2.x);
    let dx2 = l(point2.x) - l(point3.x);
    let dy1 = l(point1.y) - l(point2.y);
    let dy2 = l(point2.y) - l(point3.y);
    ot_eval_f(robust_cross_product(dx1, dy1, dx2, dy2))
}

#[inline]
pub fn point_comparison(lhs: &Point2d, rhs: &Point2d) -> bool {
    if lhs.x == rhs.x {
        return lhs.y < rhs.y;
    }
    lhs.x < rhs.x
}

/// `event_comparison_predicate(site, site)`.
pub fn event_less_sites(lhs: &SiteEvent, rhs: &SiteEvent) -> bool {
    if lhs.x0() != rhs.x0() {
        return lhs.x0() < rhs.x0();
    }
    if !lhs.is_segment() {
        if !rhs.is_segment() {
            return lhs.y0() < rhs.y0();
        }
        if is_vertical(rhs) {
            return lhs.y0() <= rhs.y0();
        }
        true
    } else {
        if is_vertical(rhs) {
            if is_vertical(lhs) {
                return lhs.y0() < rhs.y0();
            }
            return false;
        }
        if is_vertical(lhs) {
            return true;
        }
        if lhs.y0() != rhs.y0() {
            return lhs.y0() < rhs.y0();
        }
        ot_eval_pts(&lhs.point1, &lhs.point0, &rhs.point1) == Orientation::Left
    }
}

/// `event_comparison_predicate(site, circle)`.
#[inline]
pub fn event_less_site_circle(lhs: &SiteEvent, rhs: &CircleEvent) -> bool {
    ulp_cmp(f(lhs.x0()), rhs.lower_x(), ULPS) == UlpResult::Less
}

/// `event_comparison_predicate(circle, site)`.
#[inline]
pub fn event_less_circle_site(lhs: &CircleEvent, rhs: &SiteEvent) -> bool {
    ulp_cmp(lhs.lower_x(), f(rhs.x0()), ULPS) == UlpResult::Less
}

/// `event_comparison_predicate(circle, circle)`.
#[inline]
pub fn event_less_circles(lhs: &CircleEvent, rhs: &CircleEvent) -> bool {
    if lhs.lower_x() != rhs.lower_x() {
        return lhs.lower_x() < rhs.lower_x();
    }
    lhs.y() < rhs.y()
}

// ---------------------------------------------------------------------------------------------
// distance_predicate
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum PredicateResult {
    Less,
    Undefined,
    More,
}

/// Returns true if a horizontal line going through a new site intersects right arc at first.
pub fn distance_predicate(left_site: &SiteEvent, right_site: &SiteEvent, new_point: &Point2d) -> bool {
    if !left_site.is_segment() {
        if !right_site.is_segment() {
            dp_pp(left_site, right_site, new_point)
        } else {
            dp_ps(left_site, right_site, new_point, false)
        }
    } else if !right_site.is_segment() {
        dp_ps(right_site, left_site, new_point, true)
    } else {
        dp_ss(left_site, right_site, new_point)
    }
}

fn dp_pp(left_site: &SiteEvent, right_site: &SiteEvent, new_point: &Point2d) -> bool {
    let left_point = &left_site.point0;
    let right_point = &right_site.point0;
    if left_point.x > right_point.x {
        if new_point.y <= left_point.y {
            return false;
        }
    } else if left_point.x < right_point.x {
        if new_point.y >= right_point.y {
            return true;
        }
    } else {
        return l(left_point.y) + l(right_point.y) < l(new_point.y) * 2;
    }
    let dist1 = find_distance_to_point_arc(left_site, new_point);
    let dist2 = find_distance_to_point_arc(right_site, new_point);
    dist1 < dist2
}

fn dp_ps(left_site: &SiteEvent, right_site: &SiteEvent, new_point: &Point2d, reverse_order: bool) -> bool {
    let fast_res = fast_ps(left_site, right_site, new_point, reverse_order);
    if fast_res != PredicateResult::Undefined {
        return fast_res == PredicateResult::Less;
    }
    let dist1 = find_distance_to_point_arc(left_site, new_point);
    let dist2 = find_distance_to_segment_arc(right_site, new_point);
    reverse_order ^ (dist1 < dist2)
}

fn dp_ss(left_site: &SiteEvent, right_site: &SiteEvent, new_point: &Point2d) -> bool {
    if left_site.sorted_index == right_site.sorted_index {
        return ot_eval_pts(&left_site.point0, &left_site.point1, new_point) == Orientation::Left;
    }
    let dist1 = find_distance_to_segment_arc(left_site, new_point);
    let dist2 = find_distance_to_segment_arc(right_site, new_point);
    dist1 < dist2
}

fn find_distance_to_point_arc(site: &SiteEvent, point: &Point2d) -> f64 {
    let dx = f(site.x()) - f(point.x);
    let dy = f(site.y()) - f(point.y);
    (dx * dx + dy * dy) / (2.0 * dx)
}

fn find_distance_to_segment_arc(site: &SiteEvent, point: &Point2d) -> f64 {
    if is_vertical(site) {
        (f(site.x()) - f(point.x)) * 0.5
    } else {
        let segment0 = &site.point0;
        let segment1 = &site.point1;
        let a1 = f(segment1.x) - f(segment0.x);
        let b1 = f(segment1.y) - f(segment0.y);
        let mut k = (a1 * a1 + b1 * b1).sqrt();
        if !(b1 < 0.0) {
            k = 1.0 / (b1 + k);
        } else {
            k = (k - b1) / (a1 * a1);
        }
        k * robust_cross_product(
            l(segment1.x) - l(segment0.x),
            l(segment1.y) - l(segment0.y),
            l(point.x) - l(segment0.x),
            l(point.y) - l(segment0.y),
        )
    }
}

fn fast_ps(left_site: &SiteEvent, right_site: &SiteEvent, new_point: &Point2d, reverse_order: bool) -> PredicateResult {
    let site_point = &left_site.point0;
    let segment_start = &right_site.point0;
    let segment_end = &right_site.point1;

    if ot_eval_pts(segment_start, segment_end, new_point) != Orientation::Right {
        return if !right_site.is_inverse() { PredicateResult::Less } else { PredicateResult::More };
    }

    let dif_x = f(new_point.x) - f(site_point.x);
    let dif_y = f(new_point.y) - f(site_point.y);
    let a = f(segment_end.x) - f(segment_start.x);
    let b = f(segment_end.y) - f(segment_start.y);

    if is_vertical(right_site) {
        if new_point.y < site_point.y && !reverse_order {
            return PredicateResult::More;
        } else if new_point.y > site_point.y && reverse_order {
            return PredicateResult::Less;
        }
        return PredicateResult::Undefined;
    } else {
        let orientation = ot_eval4(
            l(segment_end.x) - l(segment_start.x),
            l(segment_end.y) - l(segment_start.y),
            l(new_point.x) - l(site_point.x),
            l(new_point.y) - l(site_point.y),
        );
        if orientation == Orientation::Left {
            if !right_site.is_inverse() {
                return if reverse_order { PredicateResult::Less } else { PredicateResult::Undefined };
            }
            return if reverse_order { PredicateResult::Undefined } else { PredicateResult::More };
        }
    }

    let fast_left_expr = a * (dif_y + dif_x) * (dif_y - dif_x);
    let fast_right_expr = (2.0 * b) * dif_x * dif_y;
    let expr_cmp = ulp_cmp(fast_left_expr, fast_right_expr, 4);
    if expr_cmp != UlpResult::Equal {
        if (expr_cmp == UlpResult::More) ^ reverse_order {
            return if reverse_order { PredicateResult::Less } else { PredicateResult::More };
        }
        return PredicateResult::Undefined;
    }
    PredicateResult::Undefined
}

// ---------------------------------------------------------------------------------------------
// node_comparison_predicate
// ---------------------------------------------------------------------------------------------

#[inline]
fn get_comparison_site(node: &BeachLineKey) -> &SiteEvent {
    if node.left_site.sorted_index > node.right_site.sorted_index {
        return &node.left_site;
    }
    &node.right_site
}

#[inline]
fn get_comparison_point(site: &SiteEvent) -> &Point2d {
    if point_comparison(&site.point0, &site.point1) {
        &site.point0
    } else {
        &site.point1
    }
}

fn get_comparison_y(node: &BeachLineKey, is_new_node: bool) -> (i32, i32) {
    if node.left_site.sorted_index == node.right_site.sorted_index {
        return (node.left_site.y0(), 0);
    }
    if node.left_site.sorted_index > node.right_site.sorted_index {
        if !is_new_node && node.left_site.is_segment() && is_vertical(&node.left_site) {
            return (node.left_site.y0(), 1);
        }
        return (node.left_site.y1(), 1);
    }
    (node.right_site.y0(), -1)
}

/// `node_comparison_predicate::operator()`: compares beach line nodes by the y coordinate of the
/// intersection point of their arcs at the sweep line.
pub fn node_less(node1: &BeachLineKey, node2: &BeachLineKey) -> bool {
    let site1 = get_comparison_site(node1);
    let site2 = get_comparison_site(node2);
    let point1 = get_comparison_point(site1);
    let point2 = get_comparison_point(site2);

    if point1.x < point2.x {
        distance_predicate(&node1.left_site, &node1.right_site, point2)
    } else if point1.x > point2.x {
        !distance_predicate(&node2.left_site, &node2.right_site, point1)
    } else if site1.sorted_index == site2.sorted_index {
        get_comparison_y(node1, true) < get_comparison_y(node2, true)
    } else if site1.sorted_index < site2.sorted_index {
        let y1 = get_comparison_y(node1, false);
        let y2 = get_comparison_y(node2, true);
        if y1.0 != y2.0 {
            return y1.0 < y2.0;
        }
        if !site1.is_segment() {
            y1.1 < 0
        } else {
            false
        }
    } else {
        let y1 = get_comparison_y(node1, true);
        let y2 = get_comparison_y(node2, false);
        if y1.0 != y2.0 {
            return y1.0 < y2.0;
        }
        if !site2.is_segment() {
            y2.1 > 0
        } else {
            true
        }
    }
}

// ---------------------------------------------------------------------------------------------
// circle_existence_predicate
// ---------------------------------------------------------------------------------------------

fn cep_ppp(site1: &SiteEvent, site2: &SiteEvent, site3: &SiteEvent) -> bool {
    ot_eval_pts(&site1.point0, &site2.point0, &site3.point0) == Orientation::Right
}

fn cep_pps(site1: &SiteEvent, site2: &SiteEvent, site3: &SiteEvent, segment_index: i32) -> bool {
    if segment_index != 2 {
        let orient1 = ot_eval_pts(&site1.point0, &site2.point0, &site3.point0);
        let orient2 = ot_eval_pts(&site1.point0, &site2.point0, &site3.point1);
        if segment_index == 1 && site1.x0() >= site2.x0() {
            if orient1 != Orientation::Right {
                return false;
            }
        } else if segment_index == 3 && site2.x0() >= site1.x0() {
            if orient2 != Orientation::Right {
                return false;
            }
        } else if orient1 != Orientation::Right && orient2 != Orientation::Right {
            return false;
        }
    } else {
        return site3.point0 != site1.point0 || site3.point1 != site2.point0;
    }
    true
}

fn cep_pss(site1: &SiteEvent, site2: &SiteEvent, site3: &SiteEvent, point_index: i32) -> bool {
    if site2.sorted_index == site3.sorted_index {
        return false;
    }
    if point_index == 2 {
        if !site2.is_inverse() && site3.is_inverse() {
            return false;
        }
        if site2.is_inverse() == site3.is_inverse()
            && ot_eval_pts(&site2.point0, &site1.point0, &site3.point1) != Orientation::Right
        {
            return false;
        }
    }
    true
}

fn cep_sss(site1: &SiteEvent, site2: &SiteEvent, site3: &SiteEvent) -> bool {
    site1.sorted_index != site2.sorted_index && site2.sorted_index != site3.sorted_index
}

// ---------------------------------------------------------------------------------------------
// mp_circle_formation_functor (exact recomputation with big integers)
// ---------------------------------------------------------------------------------------------

fn empty4() -> [BigInt; 4] {
    [BigInt::default(), BigInt::default(), BigInt::default(), BigInt::default()]
}

fn mp_ppp(
    site1: &SiteEvent,
    site2: &SiteEvent,
    site3: &SiteEvent,
    circle: &mut CircleEvent,
    recompute_c_x: bool,
    recompute_c_y: bool,
    recompute_lower_x: bool,
) {
    let dif_x = [bi(l(site1.x()) - l(site2.x())), bi(l(site2.x()) - l(site3.x())), bi(l(site1.x()) - l(site3.x()))];
    let dif_y = [bi(l(site1.y()) - l(site2.y())), bi(l(site2.y()) - l(site3.y())), bi(l(site1.y()) - l(site3.y()))];
    let sum_x = [bi(l(site1.x()) + l(site2.x())), bi(l(site2.x()) + l(site3.x()))];
    let sum_y = [bi(l(site1.y()) + l(site2.y())), bi(l(site2.y()) + l(site3.y()))];
    let inv_denom = 0.5 / (&dif_x[0] * &dif_y[1] - &dif_x[1] * &dif_y[0]).d();
    let numer1 = &dif_x[0] * &sum_x[0] + &dif_y[0] * &sum_y[0];
    let numer2 = &dif_x[1] * &sum_x[1] + &dif_y[1] * &sum_y[1];

    if recompute_c_x || recompute_lower_x {
        let c_x = &numer1 * &dif_y[1] - &numer2 * &dif_y[0];
        circle.center_x = c_x.d() * inv_denom;

        if recompute_lower_x {
            // Evaluate radius of the circle.
            let sqr_r = (&dif_x[0] * &dif_x[0] + &dif_y[0] * &dif_y[0])
                * (&dif_x[1] * &dif_x[1] + &dif_y[1] * &dif_y[1])
                * (&dif_x[2] * &dif_x[2] + &dif_y[2] * &dif_y[2]);
            let r = sqr_r.d().sqrt();

            // If c_x >= 0 then lower_x = c_x + r,
            // else lower_x = (c_x * c_x - r * r) / (c_x - r).
            // To guarantee epsilon relative error.
            if !(circle.x() < 0.0) {
                if !(inv_denom < 0.0) {
                    circle.lower_x = circle.x() + r * inv_denom;
                } else {
                    circle.lower_x = circle.x() - r * inv_denom;
                }
            } else {
                let numer = &c_x * &c_x - &sqr_r;
                let lower_x = numer.d() * inv_denom / (c_x.d() + r);
                circle.lower_x = lower_x;
            }
        }
    }

    if recompute_c_y {
        let c_y = &numer2 * &dif_x[0] - &numer1 * &dif_x[1];
        circle.center_y = c_y.d() * inv_denom;
    }
}

fn mp_pps(
    site1: &SiteEvent,
    site2: &SiteEvent,
    site3: &SiteEvent,
    segment_index: i32,
    c_event: &mut CircleEvent,
    recompute_c_x: bool,
    recompute_c_y: bool,
    recompute_lower_x: bool,
) {
    let mut cA = empty4();
    let mut cB = empty4();
    let line_a = bi(l(site3.y1()) - l(site3.y0()));
    let line_b = bi(l(site3.x0()) - l(site3.x1()));
    let segm_len = &line_a * &line_a + &line_b * &line_b;
    let vec_x = bi(l(site2.y()) - l(site1.y()));
    let vec_y = bi(l(site1.x()) - l(site2.x()));
    let sum_x = bi(l(site1.x()) + l(site2.x()));
    let sum_y = bi(l(site1.y()) + l(site2.y()));
    let teta = &line_a * &vec_x + &line_b * &vec_y;
    let mut denom = &vec_x * &line_b - &vec_y * &line_a;

    let mut dif0 = bi(l(site3.y1()) - l(site1.y()));
    let mut dif1 = bi(l(site1.x()) - l(site3.x1()));
    let A = &line_a * &dif1 - &line_b * &dif0;
    dif0 = bi(l(site3.y1()) - l(site2.y()));
    dif1 = bi(l(site2.x()) - l(site3.x1()));
    let B = &line_a * &dif1 - &line_b * &dif0;
    let sum_AB = &A + &B;

    if denom.is_zero() {
        let numer = &teta * &teta - &sum_AB * &sum_AB;
        denom = &teta * &sum_AB;
        cA[0] = (&denom * &sum_x).mul_i32(2) + &numer * &vec_x;
        cB[0] = segm_len.clone();
        cA[1] = (&denom * &sum_AB).mul_i32(2) + &numer * &teta;
        cB[1] = BigInt::from_i32(1);
        cA[2] = (&denom * &sum_y).mul_i32(2) + &numer * &vec_y;
        let inv_denom = 1.0 / denom.d();
        if recompute_c_x {
            c_event.center_x = 0.25 * cA[0].d() * inv_denom;
        }
        if recompute_c_y {
            c_event.center_y = 0.25 * cA[2].d() * inv_denom;
        }
        if recompute_lower_x {
            c_event.lower_x = 0.25 * SE::eval2(&cA, &cB).d() * inv_denom / segm_len.d().sqrt();
        }
        return;
    }

    let det = ((&teta * &teta + &denom * &denom) * &A * &B).mul_i32(4);
    let mut inv_denom_sqr = 1.0 / denom.d();
    inv_denom_sqr *= inv_denom_sqr;

    if recompute_c_x || recompute_lower_x {
        cA[0] = &sum_x * &denom * &denom + &teta * &sum_AB * &vec_x;
        cB[0] = BigInt::from_i32(1);
        cA[1] = if segment_index == 2 { -&vec_x } else { vec_x.clone() };
        cB[1] = det.clone();
        if recompute_c_x {
            c_event.center_x = 0.5 * SE::eval2(&cA, &cB).d() * inv_denom_sqr;
        }
    }

    if recompute_c_y || recompute_lower_x {
        cA[2] = &sum_y * &denom * &denom + &teta * &sum_AB * &vec_y;
        cB[2] = BigInt::from_i32(1);
        cA[3] = if segment_index == 2 { -&vec_y } else { vec_y.clone() };
        cB[3] = det.clone();
        if recompute_c_y {
            c_event.center_y = 0.5 * SE::eval2(&cA[2..], &cB[2..]).d() * inv_denom_sqr;
        }
    }

    if recompute_lower_x {
        cB[0] = &cB[0] * &segm_len;
        cB[1] = &cB[1] * &segm_len;
        cA[2] = &sum_AB * &(&denom * &denom + &teta * &teta);
        cB[2] = BigInt::from_i32(1);
        cA[3] = if segment_index == 2 { -&teta } else { teta.clone() };
        cB[3] = det.clone();
        c_event.lower_x = 0.5 * SE::eval4(&cA, &cB).d() * inv_denom_sqr / segm_len.d().sqrt();
    }
}

fn mp_pss(
    site1: &SiteEvent,
    site2: &SiteEvent,
    site3: &SiteEvent,
    point_index: i32,
    c_event: &mut CircleEvent,
    recompute_c_x: bool,
    recompute_c_y: bool,
    recompute_lower_x: bool,
) {
    let mut cA = empty4();
    let mut cB = empty4();
    let segm_start1 = site2.point1;
    let segm_end1 = site2.point0;
    let segm_start2 = site3.point0;
    let segm_end2 = site3.point1;
    let a0 = bi(l(segm_end1.x) - l(segm_start1.x));
    let b0 = bi(l(segm_end1.y) - l(segm_start1.y));
    let a1 = bi(l(segm_end2.x) - l(segm_start2.x));
    let b1 = bi(l(segm_end2.y) - l(segm_start2.y));
    let orientation = &a1 * &b0 - &a0 * &b1;
    if orientation.is_zero() {
        let denom = 2.0 * (&a0 * &a0 + &b0 * &b0).d();
        let c0 = &b0 * (l(segm_start2.x) - l(segm_start1.x)) - &a0 * (l(segm_start2.y) - l(segm_start1.y));
        let dx = &a0 * (l(site1.y()) - l(segm_start1.y)) - &b0 * (l(site1.x()) - l(segm_start1.x));
        let dy = &b0 * (l(site1.x()) - l(segm_start2.x)) - &a0 * (l(site1.y()) - l(segm_start2.y));
        cB[0] = &dx * &dy;
        cB[1] = BigInt::from_i32(1);

        if recompute_c_y {
            cA[0] = b0.mul_i32(if point_index == 2 { 2 } else { -2 });
            cA[1] = &a0 * &a0 * (l(segm_start1.y) + l(segm_start2.y))
                - &a0 * &b0 * (l(segm_start1.x) + l(segm_start2.x) - l(site1.x()) * 2)
                + &b0 * &b0 * (l(site1.y()) * 2);
            let c_y = SE::eval2(&cA, &cB).d();
            c_event.center_y = c_y / denom;
        }

        if recompute_c_x || recompute_lower_x {
            cA[0] = a0.mul_i32(if point_index == 2 { 2 } else { -2 });
            cA[1] = &b0 * &b0 * (l(segm_start1.x) + l(segm_start2.x))
                - &a0 * &b0 * (l(segm_start1.y) + l(segm_start2.y) - l(site1.y()) * 2)
                + &a0 * &a0 * (l(site1.x()) * 2);

            if recompute_c_x {
                let c_x = SE::eval2(&cA, &cB).d();
                c_event.center_x = c_x / denom;
            }

            if recompute_lower_x {
                cA[2] = if c0.is_neg() { -&c0 } else { c0.clone() };
                cB[2] = &a0 * &a0 + &b0 * &b0;
                let lower_x = SE::eval3(&cA, &cB).d();
                c_event.lower_x = lower_x / denom;
            }
        }
        return;
    }
    let c0 = &b0 * l(segm_end1.x) - &a0 * l(segm_end1.y);
    let c1 = &a1 * l(segm_end2.y) - &b1 * l(segm_end2.x);
    let ix = &a0 * &c1 + &a1 * &c0;
    let iy = &b0 * &c1 + &b1 * &c0;
    let dx = &ix - &orientation * l(site1.x());
    let dy = &iy - &orientation * l(site1.y());
    if dx.is_zero() && dy.is_zero() {
        let denom = orientation.d();
        let c_x = ix.d() / denom;
        let c_y = iy.d() / denom;
        *c_event = CircleEvent::new(c_x, c_y, c_x);
        return;
    }

    let sign = BigInt::from_i32((if point_index == 2 { 1 } else { -1 }) * (if orientation.is_neg() { 1 } else { -1 }));
    cA[0] = &a1 * &(-&dx) + &b1 * &(-&dy);
    cA[1] = &a0 * &(-&dx) + &b0 * &(-&dy);
    cA[2] = sign.clone();
    cA[3] = BigInt::from_i32(0);
    cB[0] = &a0 * &a0 + &b0 * &b0;
    cB[1] = &a1 * &a1 + &b1 * &b1;
    cB[2] = &a0 * &a1 + &b0 * &b1;
    cB[3] = ((&a0 * &dy - &b0 * &dx) * (&a1 * &dy - &b1 * &dx)).mul_i32(-2);
    let temp = sqrt_expr_evaluator_pss4(&cA, &cB).d();
    let denom = temp * orientation.d();

    if recompute_c_y {
        cA[0] = &b1 * &(&dx * &dx + &dy * &dy) - &iy * &(&dx * &a1 + &dy * &b1);
        cA[1] = &b0 * &(&dx * &dx + &dy * &dy) - &iy * &(&dx * &a0 + &dy * &b0);
        cA[2] = &iy * &sign;
        let cy = sqrt_expr_evaluator_pss4(&cA, &cB).d();
        c_event.center_y = cy / denom;
    }

    if recompute_c_x || recompute_lower_x {
        cA[0] = &a1 * &(&dx * &dx + &dy * &dy) - &ix * &(&dx * &a1 + &dy * &b1);
        cA[1] = &a0 * &(&dx * &dx + &dy * &dy) - &ix * &(&dx * &a0 + &dy * &b0);
        cA[2] = &ix * &sign;

        if recompute_c_x {
            let cx = sqrt_expr_evaluator_pss4(&cA, &cB).d();
            c_event.center_x = cx / denom;
        }

        if recompute_lower_x {
            cA[3] = (&orientation * &(&dx * &dx + &dy * &dy)).mul_i32(if temp < 0.0 { -1 } else { 1 });
            let lower_x = sqrt_expr_evaluator_pss4(&cA, &cB).d();
            c_event.lower_x = lower_x / denom;
        }
    }
}

fn mp_sss(
    site1: &SiteEvent,
    site2: &SiteEvent,
    site3: &SiteEvent,
    c_event: &mut CircleEvent,
    recompute_c_x: bool,
    recompute_c_y: bool,
    recompute_lower_x: bool,
) {
    let mut cA = empty4();
    let mut cB = empty4();
    let sites = [site1, site2, site3];
    let a: Vec<BigInt> = sites.iter().map(|s| bi(l(s.x1()) - l(s.x0()))).collect();
    let b: Vec<BigInt> = sites.iter().map(|s| bi(l(s.y1()) - l(s.y0()))).collect();
    let c: Vec<BigInt> = sites
        .iter()
        .map(|s| bi((l(s.x0()).wrapping_mul(l(s.y1()))).wrapping_sub(l(s.y0()).wrapping_mul(l(s.x1())))))
        .collect();

    for i in 0..3 {
        cB[i] = &a[i] * &a[i] + &b[i] * &b[i];
    }

    for i in 0..3 {
        let j = (i + 1) % 3;
        let k = (i + 2) % 3;
        cA[i] = &a[j] * &b[k] - &a[k] * &b[j];
    }
    let denom = SE::eval3(&cA, &cB).d();

    if recompute_c_y {
        for i in 0..3 {
            let j = (i + 1) % 3;
            let k = (i + 2) % 3;
            cA[i] = &b[j] * &c[k] - &b[k] * &c[j];
        }
        let c_y = SE::eval3(&cA, &cB).d();
        c_event.center_y = c_y / denom;
    }

    if recompute_c_x || recompute_lower_x {
        cA[3] = BigInt::from_i32(0);
        for i in 0..3 {
            let j = (i + 1) % 3;
            let k = (i + 2) % 3;
            cA[i] = &a[j] * &c[k] - &a[k] * &c[j];
            if recompute_lower_x {
                cA[3] = &cA[3] + &(&cA[i] * &b[i]);
            }
        }

        if recompute_c_x {
            let c_x = SE::eval3(&cA, &cB).d();
            c_event.center_x = c_x / denom;
        }

        if recompute_lower_x {
            cB[3] = BigInt::from_i32(1);
            let lower_x = SE::eval4(&cA, &cB).d();
            c_event.lower_x = lower_x / denom;
        }
    }
}

/// Evaluates A[3] + A[0] * sqrt(B[0]) + A[1] * sqrt(B[1]) + A[2] * sqrt(B[3] * (sqrt(B[0] * B[1]) + B[2])).
fn sqrt_expr_evaluator_pss4(A: &[BigInt], B: &[BigInt]) -> Efpt {
    let mut cA = empty4();
    let mut cB = empty4();
    if A[3].is_zero() {
        let lh = SE::eval2(A, B);
        cA[0] = BigInt::from_i32(1);
        cB[0] = &B[0] * &B[1];
        cA[1] = B[2].clone();
        cB[1] = BigInt::from_i32(1);
        let rh = SE::eval1(&A[2..], &B[3..]) * SE::eval2(&cA, &cB).sqrt();
        if (!lh.is_neg() && !rh.is_neg()) || (!lh.is_pos() && !rh.is_pos()) {
            return lh + rh;
        }
        cA[0] = &A[0] * &A[0] * &B[0] + &A[1] * &A[1] * &B[1] - &A[2] * &A[2] * &B[3] * &B[2];
        cB[0] = BigInt::from_i32(1);
        cA[1] = (&A[0] * &A[1]).mul_i32(2) - &A[2] * &A[2] * &B[3];
        cB[1] = &B[0] * &B[1];
        let numer = SE::eval2(&cA, &cB);
        return numer / (lh - rh);
    }
    cA[0] = BigInt::from_i32(1);
    cB[0] = &B[0] * &B[1];
    cA[1] = B[2].clone();
    cB[1] = BigInt::from_i32(1);
    let rh = SE::eval1(&A[2..], &B[3..]) * SE::eval2(&cA, &cB).sqrt();
    cA[0] = A[0].clone();
    cB[0] = B[0].clone();
    cA[1] = A[1].clone();
    cB[1] = B[1].clone();
    cA[2] = A[3].clone();
    cB[2] = BigInt::from_i32(1);
    let lh = SE::eval3(&cA, &cB);
    if (!lh.is_neg() && !rh.is_neg()) || (!lh.is_pos() && !rh.is_pos()) {
        return lh + rh;
    }
    cA[0] = (&A[3] * &A[0]).mul_i32(2);
    cA[1] = (&A[3] * &A[1]).mul_i32(2);
    cA[2] = &A[0] * &A[0] * &B[0] + &A[1] * &A[1] * &B[1] + &A[3] * &A[3] - &A[2] * &A[2] * &B[2] * &B[3];
    cA[3] = (&A[0] * &A[1]).mul_i32(2) - &A[2] * &A[2] * &B[3];
    cB[3] = &B[0] * &B[1];
    let numer = sqrt_expr_evaluator_pss3(&cA, &cB);
    numer / (lh - rh)
}

/// Evaluates A[0] * sqrt(B[0]) + A[1] * sqrt(B[1]) + A[2] + A[3] * sqrt(B[0] * B[1]); B[3] = B[0] * B[1].
fn sqrt_expr_evaluator_pss3(A: &[BigInt], B: &[BigInt]) -> Efpt {
    let mut cA = [BigInt::default(), BigInt::default()];
    let mut cB = [BigInt::default(), BigInt::default()];
    let lh = SE::eval2(A, B);
    let rh = SE::eval2(&A[2..], &B[2..]);
    if (!lh.is_neg() && !rh.is_neg()) || (!lh.is_pos() && !rh.is_pos()) {
        return lh + rh;
    }
    cA[0] = &A[0] * &A[0] * &B[0] + &A[1] * &A[1] * &B[1] - &A[2] * &A[2] - &A[3] * &A[3] * &B[0] * &B[1];
    cB[0] = BigInt::from_i32(1);
    cA[1] = (&A[0] * &A[1] - &A[2] * &A[3]).mul_i32(2);
    cB[1] = B[3].clone();
    let numer = SE::eval2(&cA, &cB);
    numer / (lh - rh)
}

// ---------------------------------------------------------------------------------------------
// lazy_circle_formation_functor
// ---------------------------------------------------------------------------------------------

fn lazy_ppp(site1: &SiteEvent, site2: &SiteEvent, site3: &SiteEvent, c_event: &mut CircleEvent) {
    let dif_x1 = f(site1.x()) - f(site2.x());
    let dif_x2 = f(site2.x()) - f(site3.x());
    let dif_y1 = f(site1.y()) - f(site2.y());
    let dif_y2 = f(site2.y()) - f(site3.y());
    let orientation = robust_cross_product(
        l(site1.x()) - l(site2.x()),
        l(site2.x()) - l(site3.x()),
        l(site1.y()) - l(site2.y()),
        l(site2.y()) - l(site3.y()),
    );
    let inv_orientation = rf(0.5 / orientation, 2.0);
    let sum_x1 = f(site1.x()) + f(site2.x());
    let sum_x2 = f(site2.x()) + f(site3.x());
    let sum_y1 = f(site1.y()) + f(site2.y());
    let sum_y2 = f(site2.y()) + f(site3.y());
    let dif_x3 = f(site1.x()) - f(site3.x());
    let dif_y3 = f(site1.y()) - f(site3.y());
    let mut c_x = RobustDif::new();
    let mut c_y = RobustDif::new();
    c_x.add_val(rf(dif_x1 * sum_x1 * dif_y2, 2.0));
    c_x.add_val(rf(dif_y1 * sum_y1 * dif_y2, 2.0));
    c_x.sub_val(rf(dif_x2 * sum_x2 * dif_y1, 2.0));
    c_x.sub_val(rf(dif_y2 * sum_y2 * dif_y1, 2.0));
    c_y.add_val(rf(dif_x2 * sum_x2 * dif_x1, 2.0));
    c_y.add_val(rf(dif_y2 * sum_y2 * dif_x1, 2.0));
    c_y.sub_val(rf(dif_x1 * sum_x1 * dif_x2, 2.0));
    c_y.sub_val(rf(dif_y1 * sum_y1 * dif_x2, 2.0));
    let mut lower_x = c_x;
    lower_x.sub_val(rf(
        ((dif_x1 * dif_x1 + dif_y1 * dif_y1) * (dif_x2 * dif_x2 + dif_y2 * dif_y2) * (dif_x3 * dif_x3 + dif_y3 * dif_y3)).sqrt(),
        5.0,
    ));
    *c_event = CircleEvent::new(
        c_x.dif().fpv() * inv_orientation.fpv(),
        c_y.dif().fpv() * inv_orientation.fpv(),
        lower_x.dif().fpv() * inv_orientation.fpv(),
    );
    let recompute_c_x = c_x.dif().ulp() > ULPS_F;
    let recompute_c_y = c_y.dif().ulp() > ULPS_F;
    let recompute_lower_x = lower_x.dif().ulp() > ULPS_F;
    if recompute_c_x || recompute_c_y || recompute_lower_x {
        mp_ppp(site1, site2, site3, c_event, recompute_c_x, recompute_c_y, recompute_lower_x);
    }
}

fn lazy_pps(site1: &SiteEvent, site2: &SiteEvent, site3: &SiteEvent, segment_index: i32, c_event: &mut CircleEvent) {
    let line_a = f(site3.y1()) - f(site3.y0());
    let line_b = f(site3.x0()) - f(site3.x1());
    let vec_x = f(site2.y()) - f(site1.y());
    let vec_y = f(site1.x()) - f(site2.x());
    let teta = rf(
        robust_cross_product(
            l(site3.y1()) - l(site3.y0()),
            l(site3.x0()) - l(site3.x1()),
            l(site2.x()) - l(site1.x()),
            l(site2.y()) - l(site1.y()),
        ),
        1.0,
    );
    let A = rf(
        robust_cross_product(
            l(site3.y0()) - l(site3.y1()),
            l(site3.x0()) - l(site3.x1()),
            l(site3.y1()) - l(site1.y()),
            l(site3.x1()) - l(site1.x()),
        ),
        1.0,
    );
    let B = rf(
        robust_cross_product(
            l(site3.y0()) - l(site3.y1()),
            l(site3.x0()) - l(site3.x1()),
            l(site3.y1()) - l(site2.y()),
            l(site3.x1()) - l(site2.x()),
        ),
        1.0,
    );
    let denom = rf(
        robust_cross_product(
            l(site1.y()) - l(site2.y()),
            l(site1.x()) - l(site2.x()),
            l(site3.y1()) - l(site3.y0()),
            l(site3.x1()) - l(site3.x0()),
        ),
        1.0,
    );
    let inv_segm_len = rf(1.0 / (line_a * line_a + line_b * line_b).sqrt(), 3.0);
    let mut t = RobustDif::new();
    if ot_eval_rf(&denom) == Orientation::Collinear {
        t.add_val(teta / (RF::new(8.0) * A));
        t.sub_val(A / (RF::new(2.0) * teta));
    } else {
        let det = ((teta * teta + denom * denom) * A * B).sqrt();
        if segment_index == 2 {
            t.sub_val(det / (denom * denom));
        } else {
            t.add_val(det / (denom * denom));
        }
        t.add_val(teta * (A + B) / (RF::new(2.0) * denom * denom));
    }
    let mut c_x = RobustDif::new();
    let mut c_y = RobustDif::new();
    c_x.add_val(RF::new(0.5 * (f(site1.x()) + f(site2.x()))));
    c_x.add_dif(&RobustDif::val_times(RF::new(vec_x), &t));
    c_y.add_val(RF::new(0.5 * (f(site1.y()) + f(site2.y()))));
    c_y.add_dif(&RobustDif::val_times(RF::new(vec_y), &t));
    let mut r = RobustDif::new();
    let mut lower_x = c_x;
    r.sub_val(RF::new(line_a) * RF::new(f(site3.x0())));
    r.sub_val(RF::new(line_b) * RF::new(f(site3.y0())));
    r.add_dif(&RobustDif::val_times(RF::new(line_a), &c_x));
    r.add_dif(&RobustDif::val_times(RF::new(line_b), &c_y));
    if r.pos().fpv() < r.neg_sum().fpv() {
        r = r.negate();
    }
    lower_x.add_dif(&r.times_val(inv_segm_len));
    *c_event = CircleEvent::new(c_x.dif().fpv(), c_y.dif().fpv(), lower_x.dif().fpv());
    let recompute_c_x = c_x.dif().ulp() > ULPS_F;
    let recompute_c_y = c_y.dif().ulp() > ULPS_F;
    let recompute_lower_x = lower_x.dif().ulp() > ULPS_F;
    if recompute_c_x || recompute_c_y || recompute_lower_x {
        mp_pps(site1, site2, site3, segment_index, c_event, recompute_c_x, recompute_c_y, recompute_lower_x);
    }
}

fn lazy_pss(site1: &SiteEvent, site2: &SiteEvent, site3: &SiteEvent, point_index: i32, c_event: &mut CircleEvent) {
    let segm_start1 = site2.point1;
    let segm_end1 = site2.point0;
    let segm_start2 = site3.point0;
    let segm_end2 = site3.point1;
    let a1 = f(segm_end1.x) - f(segm_start1.x);
    let b1 = f(segm_end1.y) - f(segm_start1.y);
    let a2 = f(segm_end2.x) - f(segm_start2.x);
    let b2 = f(segm_end2.y) - f(segm_start2.y);
    let recompute_c_x;
    let recompute_c_y;
    let recompute_lower_x;
    let orientation = rf(
        robust_cross_product(
            l(segm_end1.y) - l(segm_start1.y),
            l(segm_end1.x) - l(segm_start1.x),
            l(segm_end2.y) - l(segm_start2.y),
            l(segm_end2.x) - l(segm_start2.x),
        ),
        1.0,
    );
    if ot_eval_rf(&orientation) == Orientation::Collinear {
        let a = rf(a1 * a1 + b1 * b1, 2.0);
        let c = rf(
            robust_cross_product(
                l(segm_end1.y) - l(segm_start1.y),
                l(segm_end1.x) - l(segm_start1.x),
                l(segm_start2.y) - l(segm_start1.y),
                l(segm_start2.x) - l(segm_start1.x),
            ),
            1.0,
        );
        let det = rf(
            robust_cross_product(
                l(segm_end1.x) - l(segm_start1.x),
                l(segm_end1.y) - l(segm_start1.y),
                l(site1.x()) - l(segm_start1.x),
                l(site1.y()) - l(segm_start1.y),
            ) * robust_cross_product(
                l(segm_end1.y) - l(segm_start1.y),
                l(segm_end1.x) - l(segm_start1.x),
                l(site1.y()) - l(segm_start2.y),
                l(site1.x()) - l(segm_start2.x),
            ),
            3.0,
        );
        let mut t = RobustDif::new();
        t.sub_val(RF::new(a1) * RF::new((f(segm_start1.x) + f(segm_start2.x)) * 0.5 - f(site1.x())));
        t.sub_val(RF::new(b1) * RF::new((f(segm_start1.y) + f(segm_start2.y)) * 0.5 - f(site1.y())));
        if point_index == 2 {
            t.add_val(det.sqrt());
        } else {
            t.sub_val(det.sqrt());
        }
        t.div_val(a);
        let mut c_x = RobustDif::new();
        let mut c_y = RobustDif::new();
        c_x.add_val(RF::new(0.5 * (f(segm_start1.x) + f(segm_start2.x))));
        c_x.add_dif(&RobustDif::val_times(RF::new(a1), &t));
        c_y.add_val(RF::new(0.5 * (f(segm_start1.y) + f(segm_start2.y))));
        c_y.add_dif(&RobustDif::val_times(RF::new(b1), &t));
        let mut lower_x = c_x;
        if c.is_neg() {
            lower_x.sub_val(RF::new(0.5) * c / a.sqrt());
        } else {
            lower_x.add_val(RF::new(0.5) * c / a.sqrt());
        }
        recompute_c_x = c_x.dif().ulp() > ULPS_F;
        recompute_c_y = c_y.dif().ulp() > ULPS_F;
        recompute_lower_x = lower_x.dif().ulp() > ULPS_F;
        *c_event = CircleEvent::new(c_x.dif().fpv(), c_y.dif().fpv(), lower_x.dif().fpv());
    } else {
        let sqr_sum1 = rf((a1 * a1 + b1 * b1).sqrt(), 2.0);
        let sqr_sum2 = rf((a2 * a2 + b2 * b2).sqrt(), 2.0);
        let mut a = rf(
            robust_cross_product(
                l(segm_end1.x) - l(segm_start1.x),
                l(segm_end1.y) - l(segm_start1.y),
                l(segm_start2.y) - l(segm_end2.y),
                l(segm_end2.x) - l(segm_start2.x),
            ),
            1.0,
        );
        if !a.is_neg() {
            a.add_assign(&(sqr_sum1 * sqr_sum2));
        } else {
            a = (orientation * orientation) / (sqr_sum1 * sqr_sum2 - a);
        }
        let or1 = rf(
            robust_cross_product(
                l(segm_end1.y) - l(segm_start1.y),
                l(segm_end1.x) - l(segm_start1.x),
                l(segm_end1.y) - l(site1.y()),
                l(segm_end1.x) - l(site1.x()),
            ),
            1.0,
        );
        let or2 = rf(
            robust_cross_product(
                l(segm_end2.x) - l(segm_start2.x),
                l(segm_end2.y) - l(segm_start2.y),
                l(segm_end2.x) - l(site1.x()),
                l(segm_end2.y) - l(site1.y()),
            ),
            1.0,
        );
        let det = RF::new(2.0) * a * or1 * or2;
        let c1 = rf(
            robust_cross_product(
                l(segm_end1.y) - l(segm_start1.y),
                l(segm_end1.x) - l(segm_start1.x),
                l(segm_end1.y),
                l(segm_end1.x),
            ),
            1.0,
        );
        let c2 = rf(
            robust_cross_product(
                l(segm_end2.x) - l(segm_start2.x),
                l(segm_end2.y) - l(segm_start2.y),
                l(segm_end2.x),
                l(segm_end2.y),
            ),
            1.0,
        );
        let inv_orientation = RF::new(1.0) / orientation;
        let mut t = RobustDif::new();
        let mut b = RobustDif::new();
        let mut ix = RobustDif::new();
        let mut iy = RobustDif::new();

        ix.add_val(RF::new(a2) * c1 * inv_orientation);
        ix.add_val(RF::new(a1) * c2 * inv_orientation);
        iy.add_val(RF::new(b1) * c2 * inv_orientation);
        iy.add_val(RF::new(b2) * c1 * inv_orientation);

        b.add_dif(&ix.times_val(RF::new(a1) * sqr_sum2));
        b.add_dif(&ix.times_val(RF::new(a2) * sqr_sum1));
        b.add_dif(&iy.times_val(RF::new(b1) * sqr_sum2));
        b.add_dif(&iy.times_val(RF::new(b2) * sqr_sum1));
        b.sub_val(
            sqr_sum1
                * rf(
                    robust_cross_product(
                        l(segm_end2.x) - l(segm_start2.x),
                        l(segm_end2.y) - l(segm_start2.y),
                        l(site1.y().wrapping_neg()),
                        l(site1.x()),
                    ),
                    1.0,
                ),
        );
        b.sub_val(
            sqr_sum2
                * rf(
                    robust_cross_product(
                        l(segm_end1.x) - l(segm_start1.x),
                        l(segm_end1.y) - l(segm_start1.y),
                        l(site1.y().wrapping_neg()),
                        l(site1.x()),
                    ),
                    1.0,
                ),
        );
        t.sub_dif(&b);
        if point_index == 2 {
            t.add_val(det.sqrt());
        } else {
            t.sub_val(det.sqrt());
        }
        t.div_val(a * a);
        let mut c_x = ix;
        let mut c_y = iy;
        c_x.add_dif(&t.times_val(RF::new(a1) * sqr_sum2));
        c_x.add_dif(&t.times_val(RF::new(a2) * sqr_sum1));
        c_y.add_dif(&t.times_val(RF::new(b1) * sqr_sum2));
        c_y.add_dif(&t.times_val(RF::new(b2) * sqr_sum1));
        if t.pos().fpv() < t.neg_sum().fpv() {
            t = t.negate();
        }
        let mut lower_x = c_x;
        if orientation.is_neg() {
            lower_x.sub_dif(&t.times_val(orientation));
        } else {
            lower_x.add_dif(&t.times_val(orientation));
        }
        recompute_c_x = c_x.dif().ulp() > ULPS_F;
        recompute_c_y = c_y.dif().ulp() > ULPS_F;
        recompute_lower_x = lower_x.dif().ulp() > ULPS_F;
        *c_event = CircleEvent::new(c_x.dif().fpv(), c_y.dif().fpv(), lower_x.dif().fpv());
    }
    if recompute_c_x || recompute_c_y || recompute_lower_x {
        mp_pss(site1, site2, site3, point_index, c_event, recompute_c_x, recompute_c_y, recompute_lower_x);
    }
}

fn lazy_sss(site1: &SiteEvent, site2: &SiteEvent, site3: &SiteEvent, c_event: &mut CircleEvent) {
    let a1 = RF::new(f(site1.x1()) - f(site1.x0()));
    let b1 = RF::new(f(site1.y1()) - f(site1.y0()));
    let c1 = rf(robust_cross_product(l(site1.x0()), l(site1.y0()), l(site1.x1()), l(site1.y1())), 1.0);

    let a2 = RF::new(f(site2.x1()) - f(site2.x0()));
    let b2 = RF::new(f(site2.y1()) - f(site2.y0()));
    let c2 = rf(robust_cross_product(l(site2.x0()), l(site2.y0()), l(site2.x1()), l(site2.y1())), 1.0);

    let a3 = RF::new(f(site3.x1()) - f(site3.x0()));
    let b3 = RF::new(f(site3.y1()) - f(site3.y0()));
    let c3 = rf(robust_cross_product(l(site3.x0()), l(site3.y0()), l(site3.x1()), l(site3.y1())), 1.0);

    let len1 = (a1 * a1 + b1 * b1).sqrt();
    let len2 = (a2 * a2 + b2 * b2).sqrt();
    let len3 = (a3 * a3 + b3 * b3).sqrt();
    let cross_12 = rf(
        robust_cross_product(
            l(site1.x1()) - l(site1.x0()),
            l(site1.y1()) - l(site1.y0()),
            l(site2.x1()) - l(site2.x0()),
            l(site2.y1()) - l(site2.y0()),
        ),
        1.0,
    );
    let cross_23 = rf(
        robust_cross_product(
            l(site2.x1()) - l(site2.x0()),
            l(site2.y1()) - l(site2.y0()),
            l(site3.x1()) - l(site3.x0()),
            l(site3.y1()) - l(site3.y0()),
        ),
        1.0,
    );
    let cross_31 = rf(
        robust_cross_product(
            l(site3.x1()) - l(site3.x0()),
            l(site3.y1()) - l(site3.y0()),
            l(site1.x1()) - l(site1.x0()),
            l(site1.y1()) - l(site1.y0()),
        ),
        1.0,
    );

    // denom = cross_12 * len3 + cross_23 * len1 + cross_31 * len2.
    let mut denom = RobustDif::new();
    denom.add_val(cross_12 * len3);
    denom.add_val(cross_23 * len1);
    denom.add_val(cross_31 * len2);

    // denom * r = (b2 * c_x - a2 * c_y - c2 * denom) / len2.
    let mut r = RobustDif::new();
    r.sub_val(cross_12 * c3);
    r.sub_val(cross_23 * c1);
    r.sub_val(cross_31 * c2);

    let mut c_x = RobustDif::new();
    c_x.add_val(a1 * c2 * len3);
    c_x.sub_val(a2 * c1 * len3);
    c_x.add_val(a2 * c3 * len1);
    c_x.sub_val(a3 * c2 * len1);
    c_x.add_val(a3 * c1 * len2);
    c_x.sub_val(a1 * c3 * len2);

    let mut c_y = RobustDif::new();
    c_y.add_val(b1 * c2 * len3);
    c_y.sub_val(b2 * c1 * len3);
    c_y.add_val(b2 * c3 * len1);
    c_y.sub_val(b3 * c2 * len1);
    c_y.add_val(b3 * c1 * len2);
    c_y.sub_val(b1 * c3 * len2);

    let lower_x = c_x.plus(&r);

    let denom_dif = denom.dif();
    let c_x_dif = c_x.dif() / denom_dif;
    let c_y_dif = c_y.dif() / denom_dif;
    let lower_x_dif = lower_x.dif() / denom_dif;

    let recompute_c_x = c_x_dif.ulp() > ULPS_F;
    let recompute_c_y = c_y_dif.ulp() > ULPS_F;
    let recompute_lower_x = lower_x_dif.ulp() > ULPS_F;
    *c_event = CircleEvent::new(c_x_dif.fpv(), c_y_dif.fpv(), lower_x_dif.fpv());
    if recompute_c_x || recompute_c_y || recompute_lower_x {
        mp_sss(site1, site2, site3, c_event, recompute_c_x, recompute_c_y, recompute_lower_x);
    }
}

// ---------------------------------------------------------------------------------------------
// circle_formation_predicate
// ---------------------------------------------------------------------------------------------

fn lies_outside_vertical_segment(c: &CircleEvent, s: &SiteEvent) -> bool {
    if !s.is_segment() || !is_vertical(s) {
        return false;
    }
    let y0 = f(if s.is_inverse() { s.y1() } else { s.y0() });
    let y1 = f(if s.is_inverse() { s.y0() } else { s.y1() });
    ulp_cmp(c.y(), y0, ULPS) == UlpResult::Less || ulp_cmp(c.y(), y1, ULPS) == UlpResult::More
}

/// Creates a circle event from the given three sites. Returns true if the circle event exists.
pub fn circle_formation_predicate(site1: &SiteEvent, site2: &SiteEvent, site3: &SiteEvent, circle: &mut CircleEvent) -> bool {
    if !site1.is_segment() {
        if !site2.is_segment() {
            if !site3.is_segment() {
                // (point, point, point) sites.
                if !cep_ppp(site1, site2, site3) {
                    return false;
                }
                lazy_ppp(site1, site2, site3, circle);
            } else {
                // (point, point, segment) sites.
                if !cep_pps(site1, site2, site3, 3) {
                    return false;
                }
                lazy_pps(site1, site2, site3, 3, circle);
            }
        } else if !site3.is_segment() {
            // (point, segment, point) sites.
            if !cep_pps(site1, site3, site2, 2) {
                return false;
            }
            lazy_pps(site1, site3, site2, 2, circle);
        } else {
            // (point, segment, segment) sites.
            if !cep_pss(site1, site2, site3, 1) {
                return false;
            }
            lazy_pss(site1, site2, site3, 1, circle);
        }
    } else if !site2.is_segment() {
        if !site3.is_segment() {
            // (segment, point, point) sites.
            if !cep_pps(site2, site3, site1, 1) {
                return false;
            }
            lazy_pps(site2, site3, site1, 1, circle);
        } else {
            // (segment, point, segment) sites.
            if !cep_pss(site2, site1, site3, 2) {
                return false;
            }
            lazy_pss(site2, site1, site3, 2, circle);
        }
    } else if !site3.is_segment() {
        // (segment, segment, point) sites.
        if !cep_pss(site3, site1, site2, 3) {
            return false;
        }
        lazy_pss(site3, site1, site2, 3, circle);
    } else {
        // (segment, segment, segment) sites.
        if !cep_sss(site1, site2, site3) {
            return false;
        }
        lazy_sss(site1, site2, site3, circle);
    }

    if lies_outside_vertical_segment(circle, site1)
        || lies_outside_vertical_segment(circle, site2)
        || lies_outside_vertical_segment(circle, site3)
    {
        return false;
    }
    true
}
