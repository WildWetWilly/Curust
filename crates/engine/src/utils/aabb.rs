//! `utils/AABB.h` and `utils/AABB3D.h`.

use crate::geometry::lines_set::{OpenLinesSet, Shape};
use crate::geometry::point2ll::*;
use crate::geometry::point3ll::Point3LL;
use crate::geometry::polyline::{Polygon, Polyline};
use crate::settings::types::AngleRadians;
use crate::utils::coord::Coord;
use crate::utils::linear_alg_2d as la;

/// Axis-aligned bounding box in 2D.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AABB {
    pub min: Point2LL,
    pub max: Point2LL,
}

impl Default for AABB {
    fn default() -> Self {
        AABB { min: pt(POINT_MAX, POINT_MAX), max: pt(POINT_MIN, POINT_MIN) }
    }
}

impl AABB {
    pub fn new(min: Point2LL, max: Point2LL) -> Self {
        AABB { min, max }
    }

    pub fn from_points(points: &[Point2LL]) -> Self {
        let mut ret = AABB::default();
        for &p in points {
            ret.include(p);
        }
        ret
    }

    pub fn from_shape(shape: &Shape) -> Self {
        let mut ret = AABB::default();
        ret.calculate_shape(shape);
        ret
    }

    pub fn from_lines(lines: &OpenLinesSet) -> Self {
        let mut ret = AABB::default();
        ret.calculate_lines(lines);
        ret
    }

    pub fn calculate_shape(&mut self, shape: &Shape) {
        self.min = pt(POINT_MAX, POINT_MAX);
        self.max = pt(POINT_MIN, POINT_MIN);
        for poly in shape.iter() {
            for &p in poly.iter() {
                self.include(p);
            }
        }
    }

    pub fn calculate_lines(&mut self, lines: &OpenLinesSet) {
        self.min = pt(POINT_MAX, POINT_MAX);
        self.max = pt(POINT_MIN, POINT_MIN);
        for line in lines.iter() {
            for &p in line.iter() {
                self.include(p);
            }
        }
    }

    pub fn calculate_points(&mut self, poly: &[Point2LL]) {
        self.min = pt(POINT_MAX, POINT_MAX);
        self.max = pt(POINT_MIN, POINT_MIN);
        for &p in poly {
            self.include(p);
        }
    }

    pub fn get_middle(&self) -> Point2LL {
        (self.min + self.max) / 2
    }

    pub fn distance_squared(&self, p: Point2LL) -> Coord {
        let a = pt(self.max.x, self.min.y);
        let b = pt(self.min.x, self.max.y);
        let sign = if self.contains(p) { -1 } else { 1 };
        sign * la::get_dist2_from_line_segment(self.min, a, p)
            .min(la::get_dist2_from_line_segment(a, self.max, p))
            .min(la::get_dist2_from_line_segment(self.max, b, p))
            .min(la::get_dist2_from_line_segment(b, self.min, p))
    }

    pub fn distance_squared_aabb(&self, other: &AABB) -> Coord {
        [
            self.distance_squared(other.min),
            other.distance_squared(self.min),
            self.distance_squared(other.max),
            other.distance_squared(self.max),
            self.distance_squared(pt(other.max.x, other.min.y)),
            other.distance_squared(pt(self.max.x, self.min.y)),
            self.distance_squared(pt(other.min.x, other.max.y)),
            other.distance_squared(pt(self.min.x, self.max.y)),
        ]
        .into_iter()
        .min()
        .unwrap()
    }

    #[inline]
    pub fn contains(&self, point: Point2LL) -> bool {
        point.x >= self.min.x && point.x <= self.max.x && point.y >= self.min.y && point.y <= self.max.y
    }

    pub fn contains_aabb(&self, other: &AABB) -> bool {
        if self.area() < 0 {
            return false;
        }
        if other.area() < 0 {
            return true;
        }
        other.min.x >= self.min.x && other.max.x <= self.max.x && other.min.y >= self.min.y && other.max.y <= self.max.y
    }

    pub fn area(&self) -> Coord {
        if self.max.x < self.min.x || self.max.y < self.min.y {
            return -1;
        } // Do the unititialized check explicitly, so there aren't any problems with over/underflow and POINT_MAX/POINT_MIN.
        (self.max.x - self.min.x) * (self.max.y - self.min.y)
    }

    #[inline]
    pub fn hit(&self, other: &AABB) -> bool {
        if self.max.x < other.min.x {
            return false;
        }
        if self.min.x > other.max.x {
            return false;
        }
        if self.max.y < other.min.y {
            return false;
        }
        if self.min.y > other.max.y {
            return false;
        }
        true
    }

    #[inline]
    pub fn include(&mut self, point: Point2LL) {
        self.min.x = self.min.x.min(point.x);
        self.min.y = self.min.y.min(point.y);
        self.max.x = self.max.x.max(point.x);
        self.max.y = self.max.y.max(point.y);
    }

    pub fn include_points(&mut self, polygon: &[Point2LL]) {
        for &p in polygon {
            self.include(p);
        }
    }

    pub fn include_aabb(&mut self, other: &AABB) {
        // Note that this is different from including the min and max points, since when 'min > max' it's used to denote an negative/empty box.
        self.min.x = self.min.x.min(other.min.x);
        self.min.y = self.min.y.min(other.min.y);
        self.max.x = self.max.x.max(other.max.x);
        self.max.y = self.max.y.max(other.max.y);
    }

    pub fn expand(&mut self, dist: i32) {
        if self.min == pt(POINT_MAX, POINT_MAX) || self.max == pt(POINT_MIN, POINT_MIN) {
            return;
        }
        let dist = dist as Coord;
        self.min.x -= dist;
        self.min.y -= dist;
        self.max.x += dist;
        self.max.y += dist;
    }

    pub fn to_polygon(&self) -> Polygon {
        Polygon::new(vec![self.min, pt(self.max.x, self.min.y), self.max, pt(self.min.x, self.max.y)], false)
    }

    pub fn width(&self) -> Coord {
        self.max.x - self.min.x
    }

    pub fn height(&self) -> Coord {
        self.max.y - self.min.y
    }

    pub fn minimum_area_oriented_bounding_box(shape: &Shape) -> (AABB, AngleRadians) {
        if shape[0].len() < 2 {
            return (AABB::new(pt(0, 0), pt(0, 0)), AngleRadians::new(0.0));
        }

        let mut min_area = Coord::MAX;
        let mut best_angle = AngleRadians::new(0.0);
        let mut best_aabb = AABB::new(pt(0, 0), pt(0, 0));
        for (p1, p2) in shape[0].segments() {
            let x_hat = p2 - p1;
            let length = v_size(x_hat);
            if length < 5 {
                continue;
            }
            let y_hat = turn90_ccw(x_hat);
            let mut aabb = AABB::default();
            for &p in shape[0].iter() {
                let local_p = pt(dot(p, x_hat) / length, dot(p, y_hat) / length);
                aabb.include(local_p);
            }
            let area = aabb.area();
            if area < min_area {
                min_area = area;
                best_angle = angle_rad(x_hat);
                best_aabb = aabb;
            }
        }
        (best_aabb, best_angle)
    }
}

/// Axis-aligned bounding box in 3D.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AABB3D {
    pub min: Point3LL,
    pub max: Point3LL,
}

impl Default for AABB3D {
    fn default() -> Self {
        AABB3D {
            min: Point3LL::new(i32::MAX as i64, i32::MAX as i64, i32::MAX as i64),
            max: Point3LL::new(i32::MIN as i64, i32::MIN as i64, i32::MIN as i64),
        }
    }
}

impl AABB3D {
    pub fn new(min: Point3LL, max: Point3LL) -> Self {
        AABB3D { min, max }
    }

    pub fn get_middle(&self) -> Point3LL {
        (self.min + self.max) / 2
    }

    pub fn flatten(&self) -> AABB {
        AABB::new(pt(self.min.x, self.min.y), pt(self.max.x, self.max.y))
    }

    pub fn hit(&self, other: &AABB3D) -> bool {
        !(self.max.x < other.min.x
            || self.min.x > other.max.x
            || self.max.y < other.min.y
            || self.min.y > other.max.y
            || self.max.z < other.min.z
            || self.min.z > other.max.z)
    }

    pub fn include(&mut self, p: Point3LL) -> AABB3D {
        self.min.x = self.min.x.min(p.x);
        self.min.y = self.min.y.min(p.y);
        self.min.z = self.min.z.min(p.z);
        self.max.x = self.max.x.max(p.x);
        self.max.y = self.max.y.max(p.y);
        self.max.z = self.max.z.max(p.z);
        *self
    }

    pub fn include_aabb(&mut self, aabb: &AABB3D) -> AABB3D {
        self.min.x = self.min.x.min(aabb.min.x);
        self.min.y = self.min.y.min(aabb.min.y);
        self.min.z = self.min.z.min(aabb.min.z);
        self.max.x = self.max.x.max(aabb.max.x);
        self.max.y = self.max.y.max(aabb.max.y);
        self.max.z = self.max.z.max(aabb.max.z);
        *self
    }

    pub fn include_z(&mut self, z: Coord) -> AABB3D {
        self.min.z = self.min.z.min(z);
        self.max.z = self.max.z.max(z);
        *self
    }

    pub fn translate(&mut self, offset: Point3LL) -> AABB3D {
        self.min += offset;
        self.max += offset;
        *self
    }

    pub fn translate_2d(&mut self, offset: Point2LL) -> AABB3D {
        self.min += offset;
        self.max += offset;
        *self
    }

    pub fn expand(&mut self, outset: Coord) -> AABB3D {
        self.min -= Point3LL::new(outset, outset, outset);
        self.max += Point3LL::new(outset, outset, outset);
        *self
    }

    pub fn expand_xy(&mut self, outset: Coord) -> AABB3D {
        self.min -= Point3LL::new(outset, outset, 0);
        self.max += Point3LL::new(outset, outset, 0);
        *self
    }

    pub fn span_x(&self) -> Coord {
        self.max.x - self.min.x
    }
    pub fn span_y(&self) -> Coord {
        self.max.y - self.min.y
    }
    pub fn span_z(&self) -> Coord {
        self.max.z - self.min.z
    }
}
