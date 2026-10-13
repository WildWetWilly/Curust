//! `utils/PolygonConnector.h/.cpp`: connects polygons (or closed variable-width paths) into fewer
//! polygons by bridging adjacent ones.
//!
//! The C++ `PolygonConnection` keeps pointers to the polygons; here the source polygon is always the
//! one being connected (`current` in `connectGroup`) and the destination is identified by its index
//! in the candidate list.

use crate::geometry::point2ll::{cross, scale_f64, v_size, v_size2};
use crate::geometry::polyline::Polygon;
use crate::geometry::{Point2LL, Shape};
use crate::utils::coord::Coord;
use crate::utils::extrusion_line::{ExtrusionJunction, ExtrusionLine, VariableWidthLines};
use crate::utils::linear_alg_2d;
use crate::utils::math::round_divide;

/// The maximum allowed gap between lines that get connected, in multiples of the local line width.
const MAX_GAP: f64 = 0.5;

/// Abstraction over the two polygonal types the C++ template is instantiated with.
trait Polygonal: Clone {
    type Vertex: Clone;
    fn size(&self) -> usize;
    fn vertex(&self, i: usize) -> &Self::Vertex;
    fn position(v: &Self::Vertex) -> Point2LL;
    fn width(v: &Self::Vertex, line_width: Coord) -> Coord;
    fn add_vertex_pw(&mut self, position: Point2LL, width: Coord);
    fn add_vertex(&mut self, v: &Self::Vertex);
    fn is_closed(&self) -> bool;
    fn create_empty() -> Self;
    fn front(&self) -> &Self::Vertex {
        self.vertex(0)
    }
    fn back(&self) -> &Self::Vertex {
        self.vertex(self.size() - 1)
    }
}

impl Polygonal for Polygon {
    type Vertex = Point2LL;
    fn size(&self) -> usize {
        self.points.len()
    }
    fn vertex(&self, i: usize) -> &Point2LL {
        &self.points[i]
    }
    fn position(v: &Point2LL) -> Point2LL {
        *v
    }
    fn width(_v: &Point2LL, line_width: Coord) -> Coord {
        line_width
    }
    fn add_vertex_pw(&mut self, position: Point2LL, _width: Coord) {
        self.points.push(position);
    }
    fn add_vertex(&mut self, v: &Point2LL) {
        self.points.push(*v);
    }
    fn is_closed(&self) -> bool {
        true
    }
    fn create_empty() -> Self {
        Polygon::default()
    }
}

impl Polygonal for ExtrusionLine {
    type Vertex = ExtrusionJunction;
    fn size(&self) -> usize {
        self.junctions.len()
    }
    fn vertex(&self, i: usize) -> &ExtrusionJunction {
        &self.junctions[i]
    }
    fn position(v: &ExtrusionJunction) -> Point2LL {
        v.p
    }
    fn width(v: &ExtrusionJunction, _line_width: Coord) -> Coord {
        v.w
    }
    fn add_vertex_pw(&mut self, position: Point2LL, width: Coord) {
        // Perimeter indices don't make sense any more once perimeters are merged. Use 1 as placeholder, being the first "normal" wall.
        self.junctions.push(ExtrusionJunction::new(position, width, 1));
    }
    fn add_vertex(&mut self, v: &ExtrusionJunction) {
        self.junctions.push(v.clone());
    }
    fn is_closed(&self) -> bool {
        v_size2(self.junctions[0].p - self.junctions[self.junctions.len() - 1].p) < 25
    }
    fn create_empty() -> Self {
        // inset_index 1 instead of maximum int: connected polys are not specific to any inset.
        ExtrusionLine::new(1, false, true)
    }
}

/// Which polygon a connection endpoint refers to: the polygon being connected, or one of the candidates.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PolyRef {
    From,
    To(usize),
}

#[derive(Clone, Copy, Debug)]
struct PolygonConnection {
    from_poly: PolyRef,
    from_segment: usize,
    from_point: Point2LL,
    to_poly: PolyRef,
    to_segment: usize,
    to_point: Point2LL,
}

#[derive(Clone, Copy, Debug)]
struct PolygonBridge {
    a: PolygonConnection,
    b: PolygonConnection,
}

pub struct PolygonConnector {
    line_width: Coord,
    input_polygons: Vec<Polygon>,
    input_paths: Vec<ExtrusionLine>,
}

impl PolygonConnector {
    pub fn new(line_width: Coord) -> Self {
        PolygonConnector { line_width, input_polygons: Vec::new(), input_paths: Vec::new() }
    }

    pub fn add_polygons(&mut self, input: &Shape) {
        for poly in input.iter() {
            self.input_polygons.push(poly.clone());
        }
    }

    pub fn add_paths(&mut self, input: &[VariableWidthLines]) {
        for lines in input {
            for line in lines {
                self.input_paths.push(line.clone());
            }
        }
    }

    pub fn connect(&mut self, output_polygons: &mut Shape, output_paths: &mut Vec<VariableWidthLines>) {
        let mut input_polygons = std::mem::take(&mut self.input_polygons);
        let result_polygons = self.connect_group(&mut input_polygons);
        for polygon in result_polygons {
            output_polygons.push(polygon);
        }

        let mut input_paths = std::mem::take(&mut self.input_paths);
        let result_paths = self.connect_group(&mut input_paths);
        output_paths.push(result_paths);
    }

    /// `PolygonConnector connector(line_width); connector.add(polygons); connector.add(paths); connector.connect(...)`.
    pub fn connect_all(line_width: Coord, polygons: &Shape, paths: &[VariableWidthLines]) -> (Shape, Vec<VariableWidthLines>) {
        let mut connector = PolygonConnector::new(line_width);
        connector.add_polygons(polygons);
        connector.add_paths(paths);
        let mut connected_polygons = Shape::default();
        let mut connected_paths = Vec::new();
        connector.connect(&mut connected_polygons, &mut connected_paths);
        (connected_polygons, connected_paths)
    }

    fn connect_group<P: Polygonal>(&self, to_connect: &mut Vec<P>) -> Vec<P> {
        let mut result = Vec::new();
        while !to_connect.is_empty() {
            if to_connect.len() == 1 {
                // Nothing to connect it to any more.
                result.push(to_connect[0].clone());
                break;
            }
            let current = to_connect.pop().unwrap();

            if !current.is_closed() {
                // Only bridge closed contours.
                result.push(current);
                continue;
            }
            match self.get_bridge(&current, to_connect) {
                Some(bridge) => {
                    // Connect the polygons, and store the result in the to_poly.
                    let PolyRef::To(to_idx) = bridge.a.to_poly else { unreachable!() };
                    let merged = self.connect_polygons_along_bridge(&bridge, &current, &to_connect[to_idx]);
                    to_connect[to_idx] = merged;
                    // Don't store the current polygon. It has just been merged into the other one.
                }
                None => {
                    // Can't connect this to anything. Leave it as-is.
                    result.push(current);
                }
            }
        }
        result
    }

    fn poly<'p, P: Polygonal>(r: PolyRef, from_poly: &'p P, to_polygons: &'p [P]) -> &'p P {
        match r {
            PolyRef::From => from_poly,
            PolyRef::To(i) => &to_polygons[i],
        }
    }

    fn interpolate_width<P: Polygonal>(&self, position: Point2LL, a: &P::Vertex, b: &P::Vertex) -> Coord {
        let total_length = v_size(P::position(a) - P::position(b));
        if total_length == 0 {
            // Prevent division by 0 when the vertices are on top of each other.
            return P::width(a, self.line_width);
        }
        let position_along_length = v_size(position - P::position(a));
        (round_divide((P::width(b, self.line_width) * position_along_length) as u64, total_length as u64)
            .wrapping_add(round_divide((P::width(a, self.line_width) * (total_length - position_along_length)) as u64, total_length as u64))) as Coord
    }

    fn get_space<P: Polygonal>(&self, connection: &PolygonConnection, from_poly: &P, to_polygons: &[P]) -> Coord {
        let fp = Self::poly(connection.from_poly, from_poly, to_polygons);
        let tp = Self::poly(connection.to_poly, from_poly, to_polygons);
        let from_width = self.interpolate_width::<P>(connection.from_point, fp.vertex(connection.from_segment), fp.vertex((connection.from_segment + 1) % fp.size()));
        let to_width = self.interpolate_width::<P>(connection.to_point, tp.vertex(connection.to_segment), tp.vertex((connection.to_segment + 1) % tp.size()));
        v_size(connection.to_point - connection.from_point) - from_width / 2 - to_width / 2
    }

    fn find_connection<P: Polygonal>(&self, from_poly: &P, to_polygons: &[P]) -> Option<PolygonBridge> {
        let max_gap_dist = self.line_width as f64 * MAX_GAP;
        // Optimise for finding the best connection.
        let mut best_distance: Coord = max_gap_dist as Coord; // Allow up to the max_gap.
        let mut best_connection: Option<PolygonConnection> = None;
        let mut best_second_connection: Option<PolygonConnection> = None;

        // The smallest connection will be from one of the vertices. So go through all of the vertices to find the closest place where they approach.
        for poly_index in 0..to_polygons.len() {
            let to_poly = &to_polygons[poly_index];
            if !to_poly.is_closed() {
                continue;
            }
            for to_index in 0..to_poly.size() {
                let to_v1 = to_poly.vertex(to_index);
                let to_v2 = to_poly.vertex((to_index + 1) % to_poly.size());
                let to_pos1 = P::position(to_v1);
                let to_width1 = P::width(to_v1, self.line_width);
                let to_pos2 = P::position(to_v2);
                let to_width2 = P::width(to_v2, self.line_width);
                let smallest_to_width = if to_width2 < to_width1 { to_width2 } else { to_width1 };

                for from_index in 0..from_poly.size() {
                    let from_v1 = from_poly.vertex(from_index);
                    let from_v2 = from_poly.vertex((from_index + 1) % from_poly.size());
                    let from_pos1 = P::position(from_v1);
                    let from_width1 = P::width(from_v1, self.line_width);
                    let from_pos2 = P::position(from_v2);
                    let from_width2 = P::width(from_v2, self.line_width);
                    let smallest_from_width = if from_width2 < from_width1 { from_width2 } else { from_width1 };

                    // Try a naive distance first. Faster to compute, but it may estimate the distance too small.
                    let mut naive_dist = linear_alg_2d::get_dist_from_line(from_pos1, to_pos1, to_pos2);
                    if ((naive_dist - from_width1 - smallest_to_width) as f64) < max_gap_dist {
                        let closest_point = linear_alg_2d::get_closest_on_line_segment(from_pos1, to_pos1, to_pos2);
                        if closest_point == to_pos2 {
                            // The last endpoint of a vertex is considered to be part of the next segment. Let that one handle it.
                            continue;
                        }
                        let width_at_closest = self.interpolate_width::<P>(closest_point, to_v1, to_v2);
                        let distance = v_size(closest_point - from_pos1) - from_width1 - width_at_closest; // Actual, accurate distance to the other polygon.
                        if distance < best_distance {
                            let first_connection = PolygonConnection {
                                from_poly: PolyRef::From,
                                from_segment: from_index,
                                from_point: from_pos1,
                                to_poly: PolyRef::To(poly_index),
                                to_segment: to_index,
                                to_point: closest_point,
                            };
                            let second_connection = self.get_second_connection(&first_connection, (width_at_closest + from_width1) / 2, from_poly, to_polygons);
                            if second_connection.is_some() {
                                // Second connection is also valid.
                                best_distance = distance;
                                best_connection = Some(first_connection);
                                best_second_connection = second_connection;
                            }
                        }
                    }

                    // Also try the other way around: From the line segment of the from_poly to a vertex in the to_polygons.
                    naive_dist = linear_alg_2d::get_dist_from_line(to_pos1, from_pos1, from_pos2);
                    if ((naive_dist - smallest_from_width - to_width1) as f64) < max_gap_dist {
                        let closest_point = linear_alg_2d::get_closest_on_line_segment(to_pos1, from_pos1, from_pos2);
                        if closest_point == from_pos2 {
                            // The last endpoint of a vertex is considered to be part of the next segment. Let that one handle it.
                            continue;
                        }
                        let width_at_closest = self.interpolate_width::<P>(closest_point, from_v1, from_v2);
                        let distance = v_size(closest_point - to_pos1) - width_at_closest - to_width1; // Actual, accurate distance.
                        if distance < best_distance {
                            let first_connection = PolygonConnection {
                                from_poly: PolyRef::From,
                                from_segment: from_index,
                                from_point: closest_point,
                                to_poly: PolyRef::To(poly_index),
                                to_segment: to_index,
                                to_point: to_pos1,
                            };
                            let second_connection = self.get_second_connection(&first_connection, (to_width1 + width_at_closest) / 2, from_poly, to_polygons);
                            if second_connection.is_some() {
                                // Second connection is also valid.
                                best_distance = distance;
                                best_connection = Some(first_connection);
                                best_second_connection = second_connection;
                            }
                        }
                    }
                }
            }
        }

        best_connection.map(|a| PolygonBridge { a, b: best_second_connection.unwrap() })
    }

    fn get_bridge<P: Polygonal>(&self, from_poly: &P, to_polygons: &[P]) -> Option<PolygonBridge> {
        let mut connection = self.find_connection(from_poly, to_polygons)?; // No connection: no bridge.

        // Ensure that B is always the right connection and A the left.
        if linear_alg_2d::point_is_left_of_line(connection.b.from_point, connection.a.from_point, connection.a.to_point) > 0 {
            std::mem::swap(&mut connection.a, &mut connection.b);
        }
        Some(connection)
    }

    /// Walk along a polygon to find the first point that is exactly `distance` away from a given line.
    fn walk_until_distance_from_line<P: Polygonal>(
        poly: &P,
        start_index: usize,
        distance: Coord,
        line_a: Point2LL,
        line_b: Point2LL,
        direction: i16,
    ) -> Option<(Point2LL, usize)> {
        let poly_size = poly.size();
        let line_magnitude = v_size(line_b - line_a); // Pre-compute, used for line distance calculation.
        if line_magnitude == 0 {
            return None; // Line doesn't have a direction, so we can't be on any one side of it.
        }
        let dir = direction as i64 as u64 as usize;

        let mut index = start_index.wrapping_add(dir).wrapping_add(poly_size) % poly_size;
        while index != start_index {
            let vertex_pos = P::position(poly.vertex(index));
            let vertex_distance = cross(line_a - line_b, line_a - vertex_pos) / line_magnitude; // Signed distance!
            if vertex_distance.abs() >= distance {
                // Further away from the line than the threshold.
                // Interpolate over that last line segment to find the point at exactly the right distance.
                let previous_index = index.wrapping_sub(dir).wrapping_add(poly_size) % poly_size;
                let previous_pos = P::position(poly.vertex(previous_index));
                let previous_distance = cross(line_a - line_b, line_a - previous_pos) / line_magnitude;
                if previous_distance == vertex_distance {
                    // 0-length line segment, or parallel to line.
                    index = index.wrapping_add(dir).wrapping_add(poly_size) % poly_size;
                    continue;
                }
                let interpolation_pos = (distance - previous_distance) as f64 / (vertex_distance - previous_distance) as f64;
                let interpolation_neg = (-distance - previous_distance) as f64 / (vertex_distance - previous_distance) as f64;
                let interpolation = if interpolation_pos >= 0.0 && interpolation_pos < 1.0 {
                    interpolation_pos
                } else if interpolation_neg >= 0.0 && interpolation_neg < 1.0 {
                    interpolation_neg
                } else {
                    index = index.wrapping_add(dir).wrapping_add(poly_size) % poly_size;
                    continue;
                };
                let interpolated_point = previous_pos + scale_f64(vertex_pos - previous_pos, interpolation);
                // Choose the "earlier" index of the two, regardless of direction.
                return Some((interpolated_point, if direction == 1 { previous_index } else { index }));
            }
            index = index.wrapping_add(dir).wrapping_add(poly_size) % poly_size;
        }
        None // None of the vertices were far enough away from the line.
    }

    /// Get a connection parallel to a given `first` connection at an orthogonal distance from it.
    fn get_second_connection<P: Polygonal>(&self, first: &PolygonConnection, adjacent_distance: Coord, from_poly: &P, to_polygons: &[P]) -> Option<PolygonConnection> {
        let mut result: Option<PolygonConnection> = None;
        let mut best_connection_length = Coord::MAX;
        let fp = Self::poly(first.from_poly, from_poly, to_polygons);
        let tp = Self::poly(first.to_poly, from_poly, to_polygons);

        // Find the four intersections, on both sides of the initial connection, and on both polygons.
        let from_forward_intersection = Self::walk_until_distance_from_line(fp, first.from_segment, adjacent_distance, first.from_point, first.to_point, 1);
        let from_backward_intersection = Self::walk_until_distance_from_line(fp, first.from_segment, adjacent_distance, first.from_point, first.to_point, -1);
        let to_forward_intersection = Self::walk_until_distance_from_line(tp, first.to_segment, adjacent_distance, first.from_point, first.to_point, 1);
        let to_backward_intersection = Self::walk_until_distance_from_line(tp, first.to_segment, adjacent_distance, first.from_point, first.to_point, -1);

        for from_intersection in [from_forward_intersection, from_backward_intersection] {
            let Some(from_intersection) = from_intersection else {
                continue;
            };
            // Find the shortest of the connections in the to_poly.
            let original_side = linear_alg_2d::point_is_left_of_line(first.to_point, first.from_point, from_intersection.0) > 0;
            for to_intersection in [to_forward_intersection, to_backward_intersection] {
                let Some(to_intersection) = to_intersection else {
                    continue;
                };
                let current_side = linear_alg_2d::point_is_left_of_line(to_intersection.0, first.from_point, from_intersection.0) > 0;
                if original_side != current_side {
                    continue;
                }
                let connection = PolygonConnection {
                    from_poly: first.from_poly,
                    from_segment: from_intersection.1,
                    from_point: from_intersection.0,
                    to_poly: first.to_poly,
                    to_segment: to_intersection.1,
                    to_point: to_intersection.0,
                };
                let connection_length = self.get_space(&connection, from_poly, to_polygons);
                if (connection_length as f64) < MAX_GAP * self.line_width as f64 && connection_length < best_connection_length {
                    // Connection is allowed.
                    result = Some(connection);
                    best_connection_length = connection_length;
                }
            }
        }
        result
    }

    /// Build the polygon that goes from the from-polygon over the bridge into the to-polygon and back.
    fn connect_polygons_along_bridge<P: Polygonal>(&self, bridge: &PolygonBridge, from_poly: &P, to_poly: &P) -> P {
        // We'll traverse the following path:
        //
        //  <<<<<<X......X<<<<<<< to_poly
        //        ^      v
        //        ^      v
        //        ^ a  b v bridge
        //        ^      v
        //  >>>>>>X......X>>>>>>> from_poly
        //
        // To do this, from_poly and to_poly might need to be traversed in reverse order.
        let mut ret = P::create_empty(); // Create a temporary result that we'll move into the result.
        let (a, b) = (&bridge.a, &bridge.b);

        let from_size = from_poly.size();
        // Add the from-endpoint of B.
        let b_from_width = self.interpolate_width::<P>(b.from_point, from_poly.vertex(b.from_segment), from_poly.vertex((b.from_segment + 1) % from_size));
        ret.add_vertex_pw(b.from_point, b_from_width);

        // Add the from-polygonal from B to A.
        let mut forwards: usize = if a.from_segment == b.from_segment {
            // If we start and end on the same segment, iterate in the direction from A to B.
            let vertex = P::position(from_poly.vertex(b.from_segment)); // Same vertex for A and B.
            let next_vertex = P::position(from_poly.vertex((b.from_segment + 1) % from_size));
            let direction = next_vertex - vertex; // Direction we'd go into when forward iterating.
            let a_to_b = b.from_point - a.from_point;
            (v_size2(direction - a_to_b) < v_size2(-direction - a_to_b)) as usize
        } else {
            // If not the same segment, traverse in whichever direction is the long way around.
            (((b.from_segment + from_size - a.from_segment) % from_size) < ((a.from_segment + from_size - b.from_segment) % from_size)) as usize
        };
        let step = |i: usize, size: usize, forwards: usize| i.wrapping_add(2 * forwards).wrapping_sub(1).wrapping_add(size) % size;
        let first_segment = if forwards != 0 { (b.from_segment + 1) % from_size } else { (b.from_segment + from_size) % from_size };
        let mut last_segment = a.from_segment;
        if first_segment == last_segment {
            last_segment = last_segment.wrapping_add(from_size).wrapping_sub(2 * forwards).wrapping_add(1) % from_size;
        }
        let mut i = first_segment;
        let stop = last_segment.wrapping_add(from_size).wrapping_add(2 * forwards).wrapping_sub(1) % from_size;
        loop {
            // Since we might start and end on the same segment, do a do_while loop to iterate at least once.
            ret.add_vertex(from_poly.vertex(i));
            i = step(i, from_size, forwards);
            if i == stop {
                break;
            }
        }

        // Add the from-endpoint of A.
        let a_from_width = self.interpolate_width::<P>(a.from_point, from_poly.vertex(a.from_segment), from_poly.vertex((a.from_segment + 1) % from_size));
        ret.add_vertex_pw(a.from_point, a_from_width);

        let to_size = to_poly.size();
        // Add the to-endpoint of A.
        let a_to_width = self.interpolate_width::<P>(a.to_point, to_poly.vertex(a.to_segment), to_poly.vertex((a.to_segment + 1) % to_size));
        ret.add_vertex_pw(a.to_point, a_to_width);

        // Add the to_polygonal from A to B.
        forwards = if a.to_segment == b.to_segment {
            let vertex = P::position(to_poly.vertex(b.to_segment)); // Same vertex for A and B.
            let next_vertex = P::position(to_poly.vertex((b.to_segment + 1) % to_size));
            let direction = next_vertex - vertex;
            let a_to_b = b.to_point - a.to_point;
            (v_size2(direction - a_to_b) > v_size2(-direction - a_to_b)) as usize
        } else {
            (((a.to_segment + to_size - b.to_segment) % to_size) < ((b.to_segment + to_size - a.to_segment) % to_size)) as usize
        };
        let first_segment = if forwards != 0 { (a.to_segment + 1) % to_size } else { a.to_segment };
        let end_segment = if forwards != 0 { (b.to_segment + 1) % to_size } else { b.to_segment };
        i = first_segment;
        loop {
            ret.add_vertex(to_poly.vertex(i));
            i = step(i, to_size, forwards);
            if i == end_segment {
                break;
            }
        }

        // Add the to-endpoint of B.
        let b_to_width = self.interpolate_width::<P>(b.to_point, to_poly.vertex(b.to_segment), to_poly.vertex((b.to_segment + 1) % to_size));
        ret.add_vertex_pw(b.to_point, b_to_width);

        if P::position(ret.back()) != P::position(ret.front()) {
            let front = ret.front().clone();
            ret.add_vertex_pw(P::position(&front), P::width(&front, self.line_width));
        }

        ret // Override the result with the new combined shape.
    }
}
