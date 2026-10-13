//! `geometry/PointMatrix.h`, `geometry/Point3Matrix.h` and `utils/Matrix4x3D.h`.

use super::point2ll::{pt, Point2LL};
use super::point3ll::Point3LL;
use super::points_f::Point3D;
use crate::settings::types::{AngleDegrees, AngleRadians, Ratio};
use crate::utils::coord::mm2int;
use std::f64::consts::PI;

/// 2x2 rotation/scale matrix.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointMatrix {
    pub matrix: [f64; 4],
}

impl Default for PointMatrix {
    fn default() -> Self {
        PointMatrix { matrix: [1.0, 0.0, 0.0, 1.0] }
    }
}

impl PointMatrix {
    /// `PointMatrix(double rotation)` with rotation in degrees.
    pub fn from_degrees(rotation: f64) -> Self {
        let rotation = rotation / 180.0 * PI;
        let m0 = rotation.cos();
        let m1 = -rotation.sin();
        PointMatrix { matrix: [m0, m1, -m1, m0] }
    }

    pub fn from_angle_degrees(rotation: AngleDegrees) -> Self {
        Self::from_degrees(rotation.value())
    }

    pub fn from_angle_radians(rotation: AngleRadians) -> Self {
        Self::from_degrees(AngleDegrees::from(rotation).value())
    }

    /// `PointMatrix(const Point2LL& p)`: rotation that aligns p with the x axis.
    pub fn from_point(p: Point2LL) -> Self {
        let mut m0 = p.x as f64;
        let mut m1 = p.y as f64;
        let f = (m0 * m0 + m1 * m1).sqrt();
        m0 /= f;
        m1 /= f;
        PointMatrix { matrix: [m0, m1, -m1, m0] }
    }

    pub fn scale(s: f64) -> Self {
        PointMatrix { matrix: [s, 0.0, 0.0, s] }
    }

    #[inline]
    pub fn apply(&self, p: Point2LL) -> Point2LL {
        let x = p.x as f64;
        let y = p.y as f64;
        pt(cppstd::llrint(x * self.matrix[0] + y * self.matrix[1]), cppstd::llrint(x * self.matrix[2] + y * self.matrix[3]))
    }

    #[inline]
    pub fn unapply(&self, p: Point2LL) -> Point2LL {
        let x = p.x as f64;
        let y = p.y as f64;
        pt(cppstd::llrint(x * self.matrix[0] + y * self.matrix[2]), cppstd::llrint(x * self.matrix[1] + y * self.matrix[3]))
    }

    pub fn inverse(&self) -> PointMatrix {
        let m = &self.matrix;
        let det = m[0] * m[3] - m[1] * m[2];
        PointMatrix { matrix: [m[3] / det, -m[1] / det, -m[2] / det, m[0] / det] }
    }
}

/// 3x3 homogeneous matrix.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point3Matrix {
    pub matrix: [f64; 9],
}

impl Default for Point3Matrix {
    fn default() -> Self {
        Point3Matrix { matrix: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] }
    }
}

impl Point3Matrix {
    pub fn from_point_matrix(b: &PointMatrix) -> Self {
        Point3Matrix { matrix: [b.matrix[0], b.matrix[1], 0.0, b.matrix[2], b.matrix[3], 0.0, 0.0, 0.0, 1.0] }
    }

    pub fn apply_3d(&self, p: &Point3LL) -> Point3LL {
        let x = p.x as f64;
        let y = p.y as f64;
        let z = p.z as f64;
        let m = &self.matrix;
        Point3LL::new(
            cppstd::llrint(x * m[0] + y * m[1] + z * m[2]),
            cppstd::llrint(x * m[3] + y * m[4] + z * m[5]),
            cppstd::llrint(x * m[6] + y * m[7] + z * m[8]),
        )
    }

    pub fn apply(&self, p: Point2LL) -> Point2LL {
        let result = self.apply_3d(&Point3LL::new(p.x, p.y, 1));
        pt(result.x / result.z, result.y / result.z)
    }

    pub fn translate(p: Point2LL) -> Self {
        let mut ret = Point3Matrix::default();
        ret.matrix[2] = p.x as f64;
        ret.matrix[5] = p.y as f64;
        ret
    }

    pub fn compose(&self, b: &Point3Matrix) -> Point3Matrix {
        let mut ret = Point3Matrix::default();
        for outx in 0..3 {
            for outy in 0..3 {
                ret.matrix[outy * 3 + outx] = 0.0;
                for inn in 0..3 {
                    ret.matrix[outy * 3 + outx] += self.matrix[outy * 3 + inn] * b.matrix[inn * 3 + outx];
                }
            }
        }
        ret
    }
}

/// `utils/Matrix4x3D.h`: affine transformation for mesh loading.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix4x3D {
    pub m: [[f64; 3]; 4],
}

impl Default for Matrix4x3D {
    fn default() -> Self {
        Matrix4x3D { m: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0, 0.0]] }
    }
}

impl Matrix4x3D {
    pub fn scale(scale: Ratio, origin: Point3LL) -> Self {
        Self::scale_xyz(scale, scale, scale, origin)
    }

    pub fn scale_xyz(scale_x: Ratio, scale_y: Ratio, scale_z: Ratio, origin: Point3LL) -> Self {
        let mut result = Matrix4x3D::default();
        result.m[0][0] = scale_x;
        result.m[1][1] = scale_y;
        result.m[2][2] = scale_z;
        result.m[3][0] = (scale_x - 1.0) * -(origin.x as f64);
        result.m[3][1] = (scale_y - 1.0) * -(origin.y as f64);
        result.m[3][2] = (scale_z - 1.0) * -(origin.z as f64);
        result
    }

    /// `apply(const Point3D&)`: millimetres in, micrometres out.
    pub fn apply_3d(&self, p: &Point3D) -> Point3LL {
        let m = &self.m;
        Point3LL::new(
            mm2int(p.x * m[0][0] + p.y * m[1][0] + p.z * m[2][0] + m[3][0]),
            mm2int(p.x * m[0][1] + p.y * m[1][1] + p.z * m[2][1] + m[3][1]),
            mm2int(p.x * m[0][2] + p.y * m[1][2] + p.z * m[2][2] + m[3][2]),
        )
    }

    /// `apply(const Point3LL&)`: the double result is truncated to coord_t.
    pub fn apply_3ll(&self, p: &Point3LL) -> Point3LL {
        let m = &self.m;
        let (x, y, z) = (p.x as f64, p.y as f64, p.z as f64);
        Point3LL::new(
            (m[0][0] * x + m[1][0] * y + m[2][0] * z + m[3][0]) as i64,
            (m[0][1] * x + m[1][1] * y + m[2][1] * z + m[3][1]) as i64,
            (m[0][2] * x + m[1][2] * y + m[2][2] * z + m[3][2]) as i64,
        )
    }
}
