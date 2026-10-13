//! Floating point helper points: `utils/Point3D.h`, `utils/Point3F.h`, `utils/Point2F.h` and
//! `geometry/Point2D.h`.

use super::point3ll::Point3LL;
use crate::utils::coord::mm2int;
use crate::utils::math::is_zero;
use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

/// Double-precision 3D point (millimetres by default).
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
pub struct Point3D {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Point3D {
    #[inline]
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Point3D { x, y, z }
    }

    /// `Point3D(const Point3LL&, scale = 0.001)`.
    #[inline]
    pub fn from_3ll(p: &Point3LL) -> Self {
        Self::from_3ll_scaled(p, 0.001)
    }

    #[inline]
    pub fn from_3ll_scaled(p: &Point3LL, scale: f64) -> Self {
        Point3D::new(p.x as f64 * scale, p.y as f64 * scale, p.z as f64 * scale)
    }

    pub fn max(&self) -> f64 {
        if self.x > self.y && self.x > self.z {
            return self.x;
        }
        if self.y > self.z {
            return self.y;
        }
        self.z
    }

    pub fn test_length(&self, len: f64) -> bool {
        self.v_size2() <= len * len
    }

    #[inline]
    pub fn v_size2(&self) -> f64 {
        self.x * self.x + self.y * self.y + self.z * self.z
    }

    #[inline]
    pub fn v_size(&self) -> f64 {
        self.v_size2().sqrt()
    }

    #[inline]
    pub fn normalized(&self) -> Point3D {
        *self / self.v_size()
    }

    #[inline]
    pub fn cross(&self, p: &Point3D) -> Point3D {
        Point3D::new(self.y * p.z - self.z * p.y, self.z * p.x - self.x * p.z, self.x * p.y - self.y * p.x)
    }

    pub fn cross_3ll(a: &Point3LL, b: &Point3LL) -> Point3D {
        Point3D::from_3ll(a).cross(&Point3D::from_3ll(b))
    }

    #[inline]
    pub fn dot(&self, p: &Point3D) -> f64 {
        self.x * p.x + self.y * p.y + self.z * p.z
    }

    pub fn to_point3(&self) -> Point3LL {
        Point3LL::new(mm2int(self.x), mm2int(self.y), mm2int(self.z))
    }
}

impl Add for Point3D {
    type Output = Point3D;
    fn add(self, o: Point3D) -> Point3D {
        Point3D::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl Sub for Point3D {
    type Output = Point3D;
    fn sub(self, o: Point3D) -> Point3D {
        Point3D::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl Mul<f64> for Point3D {
    type Output = Point3D;
    fn mul(self, f: f64) -> Point3D {
        Point3D::new(self.x * f, self.y * f, self.z * f)
    }
}
impl Div<f64> for Point3D {
    type Output = Point3D;
    fn div(self, d: f64) -> Point3D {
        Point3D::new(self.x / d, self.y / d, self.z / d)
    }
}
impl AddAssign for Point3D {
    fn add_assign(&mut self, o: Point3D) {
        self.x += o.x;
        self.y += o.y;
        self.z += o.z;
    }
}
impl SubAssign for Point3D {
    fn sub_assign(&mut self, o: Point3D) {
        self.x -= o.x;
        self.y -= o.y;
        self.z -= o.z;
    }
}

/// Single precision 3D point used during model loading (millimetres).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point3F {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Point3F {
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Point3F { x, y, z }
    }
    pub fn to_point3d(&self) -> Point3D {
        Point3D::new(self.x as f64, self.y as f64, self.z as f64)
    }
}

/// Single precision 2D point (`utils/Point2F.h`).
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
pub struct Point2F {
    pub x: f32,
    pub y: f32,
}

impl Point2F {
    pub const fn new(x: f32, y: f32) -> Self {
        Point2F { x, y }
    }
    pub fn v_size2(&self) -> f64 {
        (self.x * self.x + self.y * self.y) as f64
    }
    pub fn v_size(&self) -> f64 {
        self.v_size2().sqrt()
    }
    pub fn v_normalized(&self) -> Point2F {
        let s = self.v_size();
        Point2F::new((self.x as f64 / s) as f32, (self.y as f64 / s) as f32)
    }
}

pub fn lerp_2f(a: &Point2F, b: &Point2F, t: f32) -> Point2F {
    Point2F::new(cppstd::lerpf(a.x, b.x, t), cppstd::lerpf(a.y, b.y, t))
}

/// Double precision 2D point (`geometry/Point2D.h`).
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
pub struct Point2D {
    pub x: f64,
    pub y: f64,
}

impl Point2D {
    pub const fn new(x: f64, y: f64) -> Self {
        Point2D { x, y }
    }
    pub fn v_size2(&self) -> f64 {
        self.x * self.x + self.y * self.y
    }
    pub fn v_size(&self) -> f64 {
        self.v_size2().sqrt()
    }
    pub fn v_normalized(&self) -> Option<Point2D> {
        let size = self.v_size();
        if is_zero(size) {
            return None;
        }
        Some(*self / size)
    }
    pub fn rotated90_ccw(&self) -> Point2D {
        Point2D::new(-self.y, self.x)
    }
    pub fn dot(p0: &Point2D, p1: &Point2D) -> f64 {
        p0.x * p1.x + p0.y * p1.y
    }
}

impl Add for Point2D {
    type Output = Point2D;
    fn add(self, o: Point2D) -> Point2D {
        Point2D::new(self.x + o.x, self.y + o.y)
    }
}
impl Sub for Point2D {
    type Output = Point2D;
    fn sub(self, o: Point2D) -> Point2D {
        Point2D::new(self.x - o.x, self.y - o.y)
    }
}
impl Neg for Point2D {
    type Output = Point2D;
    fn neg(self) -> Point2D {
        Point2D::new(-self.x, -self.y)
    }
}
impl Mul<f64> for Point2D {
    type Output = Point2D;
    fn mul(self, f: f64) -> Point2D {
        Point2D::new(self.x * f, self.y * f)
    }
}
impl Div<f64> for Point2D {
    type Output = Point2D;
    fn div(self, f: f64) -> Point2D {
        Point2D::new(self.x / f, self.y / f)
    }
}

pub fn lerp_2d(a: &Point2D, b: &Point2D, t: f64) -> Point2D {
    Point2D::new(cppstd::lerp(a.x, b.x, t), cppstd::lerp(a.y, b.y, t))
}
