//! `geometry/Point3LL.h`: 3D integer points (micrometres).

use super::point2ll::{pt, Point2LL};
use crate::utils::coord::{self, int2mm, Coord};
use std::ops::{Add, AddAssign, Div, Index, IndexMut, Mul, Neg, Rem, Sub, SubAssign};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Point3LL {
    pub x: Coord,
    pub y: Coord,
    pub z: Coord,
}

pub const NO_POINT3: Point3LL = Point3LL { x: i64::MIN, y: i64::MIN, z: i64::MIN };

impl Point3LL {
    #[inline]
    pub const fn new(x: Coord, y: Coord, z: Coord) -> Self {
        Point3LL { x, y, z }
    }

    #[inline]
    pub fn from_2d(p: Point2LL, z: Coord) -> Self {
        Point3LL { x: p.x, y: p.y, z }
    }

    /// `operator*(T)` with T floating point: rounds with `llround` (half away from zero).
    #[inline]
    pub fn scale_f64(&self, i: f64) -> Point3LL {
        Point3LL::new(cppstd::llround(self.x as f64 * i), cppstd::llround(self.y as f64 * i), cppstd::llround(self.z as f64 * i))
    }

    pub fn max(&self) -> Coord {
        if self.x > self.y && self.x > self.z {
            return self.x;
        }
        if self.y > self.z {
            return self.y;
        }
        self.z
    }

    pub fn test_length(&self, len: Coord) -> bool {
        if self.x > len || self.x < -len {
            return false;
        }
        if self.y > len || self.y < -len {
            return false;
        }
        if self.z > len || self.z < -len {
            return false;
        }
        self.v_size2() <= len * len
    }

    #[inline]
    pub fn v_size2(&self) -> Coord {
        self.x.wrapping_mul(self.x).wrapping_add(self.y.wrapping_mul(self.y)).wrapping_add(self.z.wrapping_mul(self.z))
    }

    #[inline]
    pub fn v_size2f(&self) -> f64 {
        self.x as f64 * self.x as f64 + self.y as f64 * self.y as f64 + self.z as f64 * self.z as f64
    }

    #[inline]
    pub fn v_size(&self) -> Coord {
        cppstd::llrint((self.v_size2() as f64).sqrt())
    }

    pub fn v_size_mm(&self) -> f64 {
        let fx = int2mm(self.x);
        let fy = int2mm(self.y);
        let fz = int2mm(self.z);
        (fx * fx + fy * fy + fz * fz).sqrt()
    }

    #[inline]
    pub fn dot(&self, p: &Point3LL) -> Coord {
        self.x * p.x + self.y * p.y + self.z * p.z
    }

    #[inline]
    pub fn to_point2ll(&self) -> Point2LL {
        pt(self.x, self.y)
    }

    pub fn resized(&self, length: Coord) -> Point3LL {
        let actual_length = self.v_size();
        if actual_length < 1 {
            return Point3LL::new(length, 0, 0);
        }
        (*self * length) / actual_length
    }

    /// Element-wise multiplication (`operator*(const Point3LL&)`).
    #[inline]
    pub fn mul_elem(&self, p: &Point3LL) -> Point3LL {
        Point3LL::new(self.x * p.x, self.y * p.y, self.z * p.z)
    }

    /// Element-wise division (`operator/(const Point3LL&)`).
    #[inline]
    pub fn div_elem(&self, p: &Point3LL) -> Point3LL {
        Point3LL::new(self.x / p.x, self.y / p.y, self.z / p.z)
    }
}

impl Add for Point3LL {
    type Output = Point3LL;
    #[inline]
    fn add(self, p: Point3LL) -> Point3LL {
        Point3LL::new(self.x + p.x, self.y + p.y, self.z + p.z)
    }
}
impl Sub for Point3LL {
    type Output = Point3LL;
    #[inline]
    fn sub(self, p: Point3LL) -> Point3LL {
        Point3LL::new(self.x - p.x, self.y - p.y, self.z - p.z)
    }
}
impl Neg for Point3LL {
    type Output = Point3LL;
    #[inline]
    fn neg(self) -> Point3LL {
        Point3LL::new(-self.x, -self.y, -self.z)
    }
}
/// Integer scaling (`operator*(T)` with integral T: llround of an integer is exact).
impl Mul<Coord> for Point3LL {
    type Output = Point3LL;
    #[inline]
    fn mul(self, i: Coord) -> Point3LL {
        Point3LL::new(self.x * i, self.y * i, self.z * i)
    }
}
impl Div<Coord> for Point3LL {
    type Output = Point3LL;
    #[inline]
    fn div(self, i: Coord) -> Point3LL {
        Point3LL::new(self.x / i, self.y / i, self.z / i)
    }
}
impl Rem<Coord> for Point3LL {
    type Output = Point3LL;
    #[inline]
    fn rem(self, i: Coord) -> Point3LL {
        Point3LL::new(self.x % i, self.y % i, self.z % i)
    }
}
impl AddAssign for Point3LL {
    #[inline]
    fn add_assign(&mut self, p: Point3LL) {
        self.x += p.x;
        self.y += p.y;
        self.z += p.z;
    }
}
impl SubAssign for Point3LL {
    #[inline]
    fn sub_assign(&mut self, p: Point3LL) {
        self.x -= p.x;
        self.y -= p.y;
        self.z -= p.z;
    }
}
/// `Point3LL + Point2LL` keeps z.
impl Add<Point2LL> for Point3LL {
    type Output = Point3LL;
    #[inline]
    fn add(self, p: Point2LL) -> Point3LL {
        Point3LL::new(self.x + p.x, self.y + p.y, self.z)
    }
}
impl Sub<Point2LL> for Point3LL {
    type Output = Point3LL;
    #[inline]
    fn sub(self, p: Point2LL) -> Point3LL {
        Point3LL::new(self.x - p.x, self.y - p.y, self.z)
    }
}
impl AddAssign<Point2LL> for Point3LL {
    #[inline]
    fn add_assign(&mut self, p: Point2LL) {
        self.x += p.x;
        self.y += p.y;
    }
}
impl SubAssign<Point2LL> for Point3LL {
    #[inline]
    fn sub_assign(&mut self, p: Point2LL) {
        self.x -= p.x;
        self.y -= p.y;
    }
}
impl Index<usize> for Point3LL {
    type Output = Coord;
    fn index(&self, i: usize) -> &Coord {
        match i {
            0 => &self.x,
            1 => &self.y,
            _ => &self.z,
        }
    }
}
impl IndexMut<usize> for Point3LL {
    fn index_mut(&mut self, i: usize) -> &mut Coord {
        match i {
            0 => &mut self.x,
            1 => &mut self.y,
            _ => &mut self.z,
        }
    }
}

/// `Point2LL + Point3LL` returns a 2D point.
#[inline]
pub fn add_2d_3d(p2: Point2LL, p3: &Point3LL) -> Point2LL {
    pt(p3.x + p2.x, p3.y + p2.y)
}

/// `Point2LL - Point3LL` returns a 2D point.
#[inline]
pub fn sub_2d_3d(p2: Point2LL, p3: &Point3LL) -> Point2LL {
    pt(p2.x - p3.x, p2.y - p3.y)
}

#[inline]
pub fn lerp(a: &Point3LL, b: &Point3LL, t: f64) -> Point3LL {
    Point3LL::new(coord::lerp(a.x, b.x, t), coord::lerp(a.y, b.y, t), coord::lerp(a.z, b.z, t))
}

/// `std::hash<Point3LL>` as in CuraEngine.
pub fn cpp_hash(p: &Point3LL) -> u64 {
    let prime: i32 = 31;
    let mut result: i32 = 89;
    result = ((result.wrapping_mul(prime) as i64).wrapping_add(p.x)) as i32;
    result = ((result.wrapping_mul(prime) as i64).wrapping_add(p.y)) as i32;
    result = ((result.wrapping_mul(prime) as i64).wrapping_add(p.z)) as i32;
    result as i64 as u64
}
