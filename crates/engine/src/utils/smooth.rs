//! `utils/actions/smooth.h`: "fluid motion" smoothing of polygons, shifting the end points of very
//! short segments outward when the corner around them is not smooth.

use crate::geometry::point2ll::Point2LL;
use crate::settings::settings::Settings;
use crate::utils::coord::Coord;

#[derive(Clone, Copy, Debug)]
pub struct Smooth {
    fluid_motion_shift_distance: Coord,
    fluid_motion_small_distance: Coord,
    fluid_motion_angle: f64,
}

#[inline]
fn magnitude(x: Coord, y: Coord) -> f64 {
    (x as f64).hypot(y as f64)
}

#[inline]
fn dist(p0: Point2LL, p1: Point2LL) -> f64 {
    magnitude(p1.x - p0.x, p1.y - p0.y)
}

#[inline]
fn shift_point_towards(p0: Point2LL, p1: Point2LL, move_distance: f64, p0p1_distance: f64) -> Point2LL {
    let shift_distance = move_distance / p0p1_distance;
    Point2LL::new(
        p0.x + ((p1.x - p0.x) as f64 * shift_distance) as Coord,
        p0.y + ((p1.y - p0.y) as f64 * shift_distance) as Coord,
    )
}

/// cosine of the angle between the vectors AB and CD
#[inline]
fn cos_angle4(a: Point2LL, b: Point2LL, c: Point2LL, d: Point2LL, ab_magnitude: f64, cd_magnitude: f64) -> f64 {
    let vector_ab = Point2LL::new(b.x - a.x, b.y - a.y);
    let vector_cd = Point2LL::new(d.x - c.x, d.y - c.y);
    if ab_magnitude <= f64::EPSILON || cd_magnitude <= f64::EPSILON {
        return 0.0;
    }
    let dot = vector_ab.x.wrapping_mul(vector_cd.x).wrapping_add(vector_ab.y.wrapping_mul(vector_cd.y));
    dot as f64 / (ab_magnitude * cd_magnitude)
}

impl Smooth {
    pub fn from_settings(settings: &Settings) -> Self {
        Smooth {
            fluid_motion_shift_distance: settings.get_coord("meshfix_fluid_motion_shift_distance"),
            fluid_motion_small_distance: settings.get_coord("meshfix_fluid_motion_small_distance"),
            fluid_motion_angle: settings.get_angle_radians("meshfix_fluid_motion_angle").value(),
        }
    }

    pub fn new(fluid_motion_shift_distance: Coord, fluid_motion_small_distance: Coord, fluid_motion_angle: f64) -> Self {
        Smooth { fluid_motion_shift_distance, fluid_motion_small_distance, fluid_motion_angle }
    }

    fn is_smooth(&self, a: Point2LL, b: Point2LL, c: Point2LL, d: Point2LL, fluid_motion_angle: f64, dist_ab: f64, dist_bc: f64, dist_cd: f64) -> bool {
        // Move points A and D, so they are both at equal distance from B and C.
        // Points B, C are in a "fluid motion" with points A, D if vectors [A_,D_] and [B,C] are
        // oriented within a certain angle.
        const SHIFT_DISTANCE: f64 = 300.0;
        let a_ = shift_point_towards(b, a, SHIFT_DISTANCE, dist_ab);
        let d_ = shift_point_towards(c, d, SHIFT_DISTANCE, dist_cd);

        let cos_angle_fluid = cos_angle4(a_, d_, b, c, dist(a_, d_), dist_bc);
        let cos_angle_abc = cos_angle4(a_, b, b, c, SHIFT_DISTANCE, dist_bc);
        let cos_angle_bcd = cos_angle4(b, c, c, d_, dist_bc, SHIFT_DISTANCE);

        // The motion is fluid if either of the marker angles is smaller than the max angle
        cos_angle_fluid >= fluid_motion_angle || cos_angle_abc >= fluid_motion_angle || cos_angle_bcd >= fluid_motion_angle
    }

    /// Apply the smoothing to a closed path, returning the smoothed copy.
    pub fn apply(&self, rng: &[Point2LL]) -> Vec<Point2LL> {
        let size = rng.len() as i64 - 1;
        if size < 4 {
            return rng.to_vec();
        }

        let fluid_motion_shift_distance3 = 3 * self.fluid_motion_shift_distance;
        let cos_fluid_motion_angle = self.fluid_motion_angle.cos();

        let mut tmp = rng.to_vec(); // We don't want to shift the points of the in-going range, therefore we create a temporary copy
        let n = tmp.len();
        // windows = [back, tmp..., tmp[0..4]] (+ the element just past the end, tmp[4]); window i covers
        // tmp indices (i-1, i, i+1, i+2) modulo n.
        for i in 0..n + 3 {
            let ia = (i + n - 1) % n;
            let ib = i % n;
            let ic = (i + 1) % n;
            let id = (i + 2) % n;
            let (a, b, c, d) = (tmp[ia], tmp[ib], tmp[ic], tmp[id]);

            let magnitude_ab = dist(a, b);
            let magnitude_bc = dist(b, c);
            let magnitude_cd = dist(c, d);

            if magnitude_bc > self.fluid_motion_small_distance as f64 {
                continue;
            }

            // only if segments ab and cd are long enough, we can shift b and c
            // 3 * fluid_motion_shift_distance is the minimum length of the segments ab and cd
            // as this allows us to shift both ends with the shift distance and still have some room to spare
            if magnitude_ab < fluid_motion_shift_distance3 as f64 || magnitude_cd < fluid_motion_shift_distance3 as f64 {
                continue;
            }

            if !self.is_smooth(a, b, c, d, cos_fluid_motion_angle, magnitude_ab, magnitude_bc, magnitude_cd) {
                tmp[ib] = shift_point_towards(b, a, self.fluid_motion_shift_distance as f64, magnitude_ab);
                tmp[ic] = shift_point_towards(c, d, self.fluid_motion_shift_distance as f64, magnitude_cd);
            }
        }

        tmp
    }
}
