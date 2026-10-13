//! `geometry/Point2LL.h`: 2D integer points (micrometres) and vector helpers.
//!
//! `Point2LL` is ClipperLib's `IntPoint`. In C++, `point * double` rounds with `llrint` in double
//! precision, `point * float` in single precision and `point * integer` is an exact integer
//! multiplication; these are exposed as [`scale_f64`], [`scale_f32`] and the `*` operator.

pub use clipper::IntPoint as Point2LL;

use crate::settings::types::{AngleDegrees, AngleRadians};
use crate::utils::coord::{self, int2mm, Coord};

pub const POINT_MIN: Coord = i64::MIN;
pub const POINT_MAX: Coord = i64::MAX;

/// Placeholder "no point" value.
pub const NO_POINT: Point2LL = Point2LL { x: i64::MIN, y: i64::MIN };

#[inline]
pub const fn pt(x: Coord, y: Coord) -> Point2LL {
    Point2LL { x, y }
}

#[inline]
pub fn v_size2(p: Point2LL) -> Coord {
    p.x.wrapping_mul(p.x).wrapping_add(p.y.wrapping_mul(p.y))
}

#[inline]
pub fn v_size2f(p: Point2LL) -> f64 {
    p.x as f64 * p.x as f64 + p.y as f64 * p.y as f64
}

/// `shorterThen`/`shorterThan`.
#[inline]
pub fn shorter_then(p: Point2LL, len: Coord) -> bool {
    if p.x > len || p.x < -len {
        return false;
    }
    if p.y > len || p.y < -len {
        return false;
    }
    v_size2(p) <= len * len
}

#[inline]
pub fn shorter_than(p: Point2LL, len: Coord) -> bool {
    shorter_then(p, len)
}

#[inline]
pub fn v_size(p: Point2LL) -> Coord {
    cppstd::llrint((v_size2(p) as f64).sqrt())
}

/// `vSizef` (note: returns coord_t in C++, i.e. truncated).
#[inline]
pub fn v_sizef(p: Point2LL) -> Coord {
    v_size2f(p).sqrt() as Coord
}

#[inline]
pub fn v_size_mm(p: Point2LL) -> f64 {
    let fx = int2mm(p.x);
    let fy = int2mm(p.y);
    (fx * fx + fy * fy).sqrt()
}

#[inline]
pub fn normal(p: Point2LL, length: Coord) -> Point2LL {
    let len = v_size(p);
    if len < 1 {
        return pt(length, 0);
    }
    p * length / len
}

#[inline]
pub fn turn90_ccw(p: Point2LL) -> Point2LL {
    pt(-p.y, p.x)
}

#[inline]
pub fn rotate(p: Point2LL, angle: f64) -> Point2LL {
    let c = angle.cos();
    let s = angle.sin();
    let x = p.x as f64;
    let y = p.y as f64;
    pt(cppstd::llrint(c * x - s * y), cppstd::llrint(s * x + c * y))
}

#[inline]
pub fn dot(a: Point2LL, b: Point2LL) -> Coord {
    a.x.wrapping_mul(b.x).wrapping_add(a.y.wrapping_mul(b.y))
}

#[inline]
pub fn cross(a: Point2LL, b: Point2LL) -> Coord {
    a.x.wrapping_mul(b.y).wrapping_sub(a.y.wrapping_mul(b.x))
}

#[inline]
pub fn angle_rad(p: Point2LL) -> AngleRadians {
    AngleRadians::new((p.y as f64).atan2(p.x as f64))
}

#[inline]
pub fn angle_deg(p: Point2LL) -> AngleDegrees {
    AngleDegrees::from(angle_rad(p))
}

/// `point * double` (rounded with llrint in double precision).
#[inline]
pub fn scale_f64(p: Point2LL, f: f64) -> Point2LL {
    pt(cppstd::llrint(p.x as f64 * f), cppstd::llrint(p.y as f64 * f))
}

/// `point * float` (computed and rounded in single precision).
#[inline]
pub fn scale_f32(p: Point2LL, f: f32) -> Point2LL {
    pt(cppstd::llrintf(p.x as f32 * f), cppstd::llrintf(p.y as f32 * f))
}

/// `point * int` where the C++ template is instantiated with `int`: the coordinates are first
/// truncated to 32 bit.
#[inline]
pub fn scale_i32(p: Point2LL, i: i32) -> Point2LL {
    pt(((p.x as i32).wrapping_mul(i)) as i64, ((p.y as i32).wrapping_mul(i)) as i64)
}

/// `operator/(Point2LL, Point2LL)`: element-wise division.
#[inline]
pub fn div_elementwise(a: Point2LL, b: Point2LL) -> Point2LL {
    pt(a.x / b.x, a.y / b.y)
}

#[inline]
pub fn lerp(a: Point2LL, b: Point2LL, t: f64) -> Point2LL {
    pt(coord::lerp(a.x, b.x, t), coord::lerp(a.y, b.y, t))
}

#[inline]
pub fn fuzzy_equal(a: Point2LL, b: Point2LL) -> bool {
    coord::fuzzy_equal(a.x, b.x) && coord::fuzzy_equal(a.y, b.y)
}

/// `std::hash<Point2LL>` as implemented in CuraEngine (used where unordered container
/// iteration order matters).
#[inline]
pub fn cpp_hash(p: Point2LL) -> u64 {
    let prime: i32 = 31;
    let mut result: i32 = 89;
    result = ((result.wrapping_mul(prime) as i64).wrapping_add(p.x)) as i32;
    result = ((result.wrapping_mul(prime) as i64).wrapping_add(p.y)) as i32;
    result as i64 as u64
}

/// `ClipperLib::operator<(IntPoint, IntPoint)` as defined by CuraEngine (lexicographic).
#[inline]
pub fn point_less(lhs: Point2LL, rhs: Point2LL) -> bool {
    lhs.x < rhs.x || (lhs.x == rhs.x && lhs.y < rhs.y)
}
