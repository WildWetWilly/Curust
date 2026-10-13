//! Bridge to the mapbox wagyu port used by `Shape::removeNearSelfIntersections`.

use super::lines_set::{default_max_deviation_angle, Shape};
use super::point2ll::Point2LL;
use super::polyline::Polygon;

/// `Shape::removeNearSelfIntersections()`: snap-round the shape on a 4 micron grid with wagyu (even-odd
/// union), which removes near self intersections, then scale back and clean up with Clipper.
pub fn remove_near_self_intersections(shape: &Shape) -> Shape {
    let mut w = wagyu::Wagyu::new();

    for polygon in shape.split_into_parts(false) {
        for path in polygon.iter() {
            let ring: Vec<wagyu::P> = path.iter().map(|p| wagyu::P::new(p.x / 4, p.y / 4)).collect();
            w.add_ring(&ring, wagyu::PolygonType::Subject);
        }
    }

    let mut sln = wagyu::MultiPolygon::new();
    w.execute(wagyu::ClipType::Union, &mut sln, wagyu::FillType::EvenOdd, wagyu::FillType::EvenOdd);

    let mut polys = Shape::default();
    for poly in sln {
        for mut ring in poly {
            ring.pop();
            let points = ring.into_iter().map(|p| Point2LL::new(p.x * 4, p.y * 4)).collect();
            polys.push(Polygon::new(points, false));
        }
    }
    let mut polys = polys.union_polygons();
    polys.remove_colinear_edges(default_max_deviation_angle());
    polys
}
