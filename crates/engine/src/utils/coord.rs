//! `utils/Coord_t.h`: integer coordinates (micrometres) and unit conversions.

/// `coord_t`: a coordinate in micrometres.
pub type Coord = i64;

pub const EPSILON: Coord = 5;

#[inline]
pub fn int2mm(n: Coord) -> f64 {
    n as f64 / 1000.0
}

#[inline]
pub fn int2mm2(n: Coord) -> f64 {
    n as f64 / 1_000_000.0
}

#[inline]
fn sign_half(n: f64) -> f64 {
    0.5 * (((n > 0.0) as i32 - (n < 0.0) as i32) as f64)
}

/// `MM2INT(n)`: millimetres to micrometres (rounded half away from zero).
#[inline]
pub fn mm2int(n: f64) -> Coord {
    (n * 1000.0 + sign_half(n)) as Coord
}

/// `MM2_2INT(n)`: square millimetres to square micrometres.
#[inline]
pub fn mm2_2int(n: f64) -> Coord {
    (n * 1_000_000.0 + sign_half(n)) as Coord
}

/// `MM3_2INT(n)`: cubic millimetres to cubic micrometres.
#[inline]
pub fn mm3_2int(n: f64) -> Coord {
    (n * 1_000_000_000.0 + sign_half(n)) as Coord
}

/// `cura::lerp(coord_t a, coord_t b, t)`.
#[inline]
pub fn lerp(a: Coord, b: Coord, t: f64) -> Coord {
    cppstd::llrint(cppstd::lerp(a as f64, b as f64, t))
}

#[inline]
pub fn fuzzy_is_zero(value: Coord) -> bool {
    value.abs() <= EPSILON
}

#[inline]
pub fn fuzzy_equal(a: Coord, b: Coord) -> bool {
    fuzzy_is_zero(b - a)
}

#[inline]
pub fn fuzzy_not_equal(a: Coord, b: Coord) -> bool {
    !fuzzy_equal(a, b)
}

#[inline]
pub fn fuzzy_is_greater(a: Coord, b: Coord) -> bool {
    a - b > EPSILON
}

#[inline]
pub fn fuzzy_is_greater_or_equal(a: Coord, b: Coord) -> bool {
    a > b - EPSILON
}

#[inline]
pub fn fuzzy_is_lesser(a: Coord, b: Coord) -> bool {
    b - a > EPSILON
}

#[inline]
pub fn fuzzy_is_lesser_or_equal(a: Coord, b: Coord) -> bool {
    b > a - EPSILON
}
