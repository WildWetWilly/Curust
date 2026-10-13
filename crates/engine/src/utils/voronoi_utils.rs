//! `utils/VoronoiUtils.h/.cpp`: helpers to relate Voronoi cells to the input polygons.

use voronoi::{SourceCategory, VoronoiDiagram, NIL};

use crate::geometry::matrix::PointMatrix;
use crate::geometry::point2ll::{dot, turn90_ccw, v_size, Point2LL};
use crate::utils::coord::Coord;
use crate::utils::linear_alg_2d;
use crate::utils::polygons_point_index::{PolygonsPointIndex, PolygonsSegmentIndex};

pub type Segment = PolygonsSegmentIndex;

/// `VoronoiUtils::p(vertex)`.
#[inline]
pub fn p(vd: &VoronoiDiagram, vertex: usize) -> Point2LL {
    assert!(vertex != NIL, "dereferencing a null Voronoi vertex");
    let v = vd.vertex(vertex);
    Point2LL::new(cppstd::llrint(v.x()), cppstd::llrint(v.y()))
}

pub fn get_source_point(vd: &VoronoiDiagram, cell: usize, points: &[Point2LL], segments: &[Segment]) -> Point2LL {
    let cell = vd.cell(cell);
    match cell.source_category() {
        SourceCategory::SinglePoint => points[cell.source_index()],
        SourceCategory::SegmentStartPoint => segments[cell.source_index() - points.len()].to(),
        SourceCategory::SegmentEndPoint => segments[cell.source_index() - points.len()].from(),
        _ => points[cell.source_index()],
    }
}

pub fn get_source_point_index(vd: &VoronoiDiagram, cell: usize, points: &[Point2LL], segments: &[Segment]) -> PolygonsPointIndex {
    let cell = vd.cell(cell);
    match cell.source_category() {
        SourceCategory::SegmentStartPoint => {
            let mut ret = segments[cell.source_index() - points.len()];
            ret.inc();
            ret
        }
        SourceCategory::SegmentEndPoint => segments[cell.source_index() - points.len()],
        _ => {
            let mut ret = segments[cell.source_index() - points.len()];
            ret.inc();
            ret
        }
    }
}

pub fn get_source_segment<'a>(vd: &VoronoiDiagram, cell: usize, points: &[Point2LL], segments: &'a [Segment]) -> &'a Segment {
    &segments[vd.cell(cell).source_index() - points.len()]
}

/// `VoronoiUtils::discretizeParabola`.
pub fn discretize_parabola(
    p: Point2LL,
    segment: &Segment,
    s: Point2LL,
    e: Point2LL,
    approximate_step_size: Coord,
    transitioning_angle: f64,
) -> Vec<Point2LL> {
    let mut discretized = Vec::new();
    // x is distance of point projected on the segment ab
    // xx is point projected on the segment ab
    let a = segment.from();
    let b = segment.to();
    let ab = b - a;
    let as_ = s - a;
    let ae = e - a;
    let ab_size = v_size(ab);
    let sx = dot(as_, ab) / ab_size;
    let ex = dot(ae, ab) / ab_size;
    let sxex = ex - sx;

    let ap = p - a;
    let px = dot(ap, ab) / ab_size;

    let pxx = linear_alg_2d::get_closest_on_line(p, a, b);
    let ppxx = pxx - p;
    let d = v_size(ppxx);
    let rot = PointMatrix::from_point(turn90_ccw(ppxx));

    if d == 0 {
        discretized.push(s);
        discretized.push(e);
        return discretized;
    }

    let marking_bound = (transitioning_angle * 0.5).atan();
    let mut msx = (-marking_bound * d as f64) as Coord; // projected marking_start
    let mut mex = (marking_bound * d as f64) as Coord; // projected marking_end
    let marking_start_end_h = msx * msx / (2 * d) + d / 2;
    let mut marking_start = rot.unapply(Point2LL::new(msx, marking_start_end_h)) + pxx;
    let mut marking_end = rot.unapply(Point2LL::new(mex, marking_start_end_h)) + pxx;
    let dir: Coord = if sx > ex { -1 } else { 1 };
    if dir < 0 {
        std::mem::swap(&mut marking_start, &mut marking_end);
        std::mem::swap(&mut msx, &mut mex);
    }

    let mut add_marking_start = msx * dir > (sx - px) * dir && msx * dir < (ex - px) * dir;
    let mut add_marking_end = mex * dir > (sx - px) * dir && mex * dir < (ex - px) * dir;

    let apex = rot.unapply(Point2LL::new(0, d / 2)) + pxx;
    // Only at the apex point if the projected start and end points
    // are more than 10 microns away from the projected apex
    let mut add_apex = (sx - px) * dir < -10 && (ex - px) * dir > 10;

    let step_count = ((ex - sx).abs() as f64 / approximate_step_size as f64 + 0.5) as Coord;

    discretized.push(s);
    for step in 1..step_count {
        let x = sx + sxex * step / step_count - px;
        let y = x * x / (2 * d) + d / 2;

        if add_marking_start && msx * dir < x * dir {
            discretized.push(marking_start);
            add_marking_start = false;
        }
        if add_apex && x * dir > 0 {
            discretized.push(apex);
            add_apex = false; // only add the apex just before the
        }
        if add_marking_end && mex * dir < x * dir {
            discretized.push(marking_end);
            add_marking_end = false;
        }
        let result = rot.unapply(Point2LL::new(x, y)) + pxx;
        discretized.push(result);
    }
    if add_apex {
        discretized.push(apex);
    }
    if add_marking_end {
        discretized.push(marking_end);
    }
    discretized.push(e);
    discretized
}
