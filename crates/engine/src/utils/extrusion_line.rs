//! `ExtrusionJunction.h`, `ExtrusionLine.h`, `ExtrusionSegment.h`: variable-width extrusion paths.

use crate::geometry::point2ll::{v_size, Point2LL};
use crate::geometry::{Polygon, Shape};
use crate::utils::coord::Coord;
use std::f64::consts::PI;

/// A vertex of a variable-width path: location, width and the inset it belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExtrusionJunction {
    pub p: Point2LL,
    pub w: Coord,
    pub perimeter_index: usize,
}

impl ExtrusionJunction {
    pub fn new(p: Point2LL, w: Coord, perimeter_index: Coord) -> Self {
        ExtrusionJunction { p, w, perimeter_index: perimeter_index as usize }
    }
}

impl std::ops::Sub for ExtrusionJunction {
    type Output = Point2LL;
    fn sub(self, b: ExtrusionJunction) -> Point2LL {
        self.p - b.p
    }
}

pub type LineJunctions = Vec<ExtrusionJunction>;

/// A sequence of extrusion junctions forming one (possibly closed) variable-width path.
#[derive(Clone, Debug, PartialEq)]
pub struct ExtrusionLine {
    pub inset_idx: usize,
    pub is_odd: bool,
    pub is_closed: bool,
    pub junctions: Vec<ExtrusionJunction>,
}

impl Default for ExtrusionLine {
    fn default() -> Self {
        ExtrusionLine::new(usize::MAX, false, false)
    }
}

impl std::ops::Deref for ExtrusionLine {
    type Target = Vec<ExtrusionJunction>;
    fn deref(&self) -> &Self::Target {
        &self.junctions
    }
}

impl std::ops::DerefMut for ExtrusionLine {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.junctions
    }
}

impl ExtrusionLine {
    pub fn new(inset_idx: usize, is_odd: bool, is_closed: bool) -> Self {
        ExtrusionLine { inset_idx, is_odd, is_closed, junctions: Vec::new() }
    }

    pub fn is_outer_wall(&self) -> bool {
        self.inset_idx == 0
    }

    pub fn length(&self) -> Coord {
        if self.junctions.is_empty() {
            return 0;
        }
        let mut len: Coord = 0;
        let mut prev = self.junctions[0];
        for next in &self.junctions {
            len += v_size(next.p - prev.p);
            prev = *next;
        }
        if self.is_closed {
            len += v_size(self.junctions[0].p - self.junctions[self.junctions.len() - 1].p);
        }
        len
    }

    pub fn to_polygon(&self) -> Polygon {
        let mut ret = Polygon::default();
        for j in &self.junctions {
            ret.points.push(j.p);
        }
        ret
    }

    pub fn get_minimal_width(&self) -> Coord {
        let mut best = 0usize;
        for i in 1..self.junctions.len() {
            if self.junctions[i].w < self.junctions[best].w {
                best = i;
            }
        }
        self.junctions[best].w
    }

    pub fn shorter_than(&self, check_length: Coord) -> bool {
        let mut p0 = *self.junctions.last().unwrap();
        let mut length: i64 = 0;
        for p1 in &self.junctions {
            length += v_size(p0 - *p1);
            if length >= check_length {
                return false;
            }
            p0 = *p1;
        }
        true
    }

    pub fn remove(&mut self, index: usize) {
        self.junctions.remove(index);
    }

    pub fn insert(&mut self, index: usize, p: ExtrusionJunction) {
        self.junctions.insert(index, p);
    }

    pub fn reverse(&mut self) {
        self.junctions.reverse();
    }
}

pub type VariableWidthLines = Vec<ExtrusionLine>;

/// `ExtrusionSegment`: a single variable-width segment between two junctions.
#[derive(Clone, Copy, Debug)]
pub struct ExtrusionSegment {
    pub from: ExtrusionJunction,
    pub to: ExtrusionJunction,
    pub is_odd: bool,
    pub is_reduced: bool,
}

const A_STEP: f64 = 15.0 / 180.0 * PI;

#[inline]
fn pd(x: f64, y: f64) -> Point2LL {
    // `Point2LL(double, double)`: implicit truncation to cInt.
    Point2LL::new(x as i64, y as i64)
}

impl ExtrusionSegment {
    pub fn new(from: ExtrusionJunction, to: ExtrusionJunction, is_odd: bool, is_reduced: bool) -> Self {
        ExtrusionSegment { from, to, is_odd, is_reduced }
    }

    pub fn to_shape(&self) -> Shape {
        self.to_shape_reduced(self.is_reduced)
    }

    pub fn to_shape_reduced(&self, reduced: bool) -> Shape {
        let mut ret = Shape::default();
        let vec = self.to.p - self.from.p;
        let vec_length = v_size(vec);

        if vec_length <= 0 {
            // Don't even output the endcaps.
            return ret;
        }

        let mut poly = Polygon::default();
        let delta_r = 0.5 * ((self.from.w - self.to.w).abs() as f64);
        let vec_length_fixed = delta_r.max(vec_length as f64);
        let mut alpha: f32 = (delta_r / vec_length_fixed).acos() as f32;
        if self.to.w > self.from.w {
            alpha = (PI - alpha as f64) as f32;
        }
        if (alpha as f64) <= -PI || (alpha as f64) >= PI {
            crate::log_warn!("Line joint slope is out of bounds (should be between -pi and +pi): {}", alpha);
        }
        let alpha = alpha as f64;

        let mut dir = (vec.y as f64 / vec.x as f64).atan();
        if vec.x < 0 {
            dir += PI;
        }

        // Draw the endcap on the "from" vertex's end.
        {
            let fw2 = (self.from.w / 2) as f64;
            poly.points.push(self.from.p + pd(fw2 * (alpha + dir).cos(), fw2 * (alpha + dir).sin()));

            let mut start_a = 2.0 * PI;
            while start_a > alpha + dir {
                start_a -= A_STEP;
            }
            start_a += A_STEP;

            let mut end_a = -2.0 * PI;
            while end_a < 2.0 * PI - alpha + dir {
                end_a += A_STEP;
            }

            let mut a = start_a;
            while a <= end_a {
                poly.points.push(self.from.p + pd(fw2 * a.cos(), fw2 * a.sin()));
                a += A_STEP;
            }

            // Draw the endcap.
            poly.points.push(self.from.p + pd(fw2 * (2.0 * PI - alpha + dir).cos(), fw2 * (2.0 * PI - alpha + dir).sin()));
        }

        // Draw the endcap on the "to" vertex's end.
        {
            let tw2 = (self.to.w / 2) as f64;
            poly.points.push(self.to.p + pd(tw2 * (2.0 * PI - alpha + dir).cos(), tw2 * (2.0 * PI - alpha + dir).sin()));

            let mut start_a = 2.0 * PI;
            while start_a > alpha + dir {
                start_a -= A_STEP;
            }
            if reduced {
                // If reduced, we'll go the other way around the circle, drawing the other half.
                // The rounding at the ends works slightly different then.
                start_a += A_STEP;
            }

            let mut end_a = -2.0 * PI;
            while end_a < 2.0 * PI - alpha + dir {
                end_a += A_STEP;
            }
            if reduced {
                end_a -= A_STEP;
            } else {
                end_a -= 2.0 * PI;
            }

            // Draw the endcap.
            if reduced {
                let mut a = end_a;
                while a >= start_a {
                    poly.points.push(self.to.p + pd(tw2 * a.cos(), tw2 * a.sin()));
                    a -= A_STEP;
                }
            } else {
                let mut a = end_a;
                while a <= start_a {
                    poly.points.push(self.to.p + pd(tw2 * a.cos(), tw2 * a.sin()));
                    a += A_STEP;
                }
            }

            poly.points.push(self.to.p + pd(tw2 * (alpha + dir).cos(), tw2 * (alpha + dir).sin()));
        }
        ret.lines.push(poly);
        ret
    }

    pub fn discretize(&mut self, step_size: Coord) -> Vec<ExtrusionSegment> {
        let a = self.from.p;
        let b = self.to.p;
        let ab = b - a;
        let ab_length = v_size(ab);
        let step_count = 1i64.max((ab_length + step_size / 2) / step_size);
        let mut discretized: Vec<ExtrusionSegment> = Vec::new();
        for step in 0..step_count {
            let mid = ExtrusionJunction::new(
                a + ab * (step + 1) / step_count,
                self.from.w + (self.to.w - self.from.w) * (step + 1) / step_count,
                self.from.perimeter_index as Coord,
            );
            discretized.push(ExtrusionSegment::new(self.from, mid, self.is_odd, true));
            self.from = mid;
        }
        discretized.last_mut().unwrap().is_reduced = self.is_reduced;
        discretized
    }
}
