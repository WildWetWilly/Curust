//! `PathOrder.h`, `PathOrderMonotonic.h/.cpp`: ordering lines in a monotonic direction (used for
//! monotonic top/bottom skin and ironing).

use crate::geometry::point2ll::{v_size2, Point2LL};
use crate::geometry::points_f::Point2D;
use crate::path_order_optimizer::PathOrdering;
use crate::settings::types::AngleRadians;
use crate::utils::coord::Coord;
use crate::utils::sparse_grid::{SparsePointGridInclusive, SparsePointGridInclusiveElem};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;

/// Endpoints of polylines that are closer than this distance together will be considered to be coincident.
const COINCIDENT_POINT_DISTANCE: Coord = 10;

/// Divide by a precision factor before doing the rounding, so that we are sure that aligned lines will end up in the same bucket
const PRECISION_FACTOR: f64 = 10.0;

pub struct PathOrderMonotonic<P: Copy> {
    /// The paths in the order in which they should be printed (after optimizing).
    pub paths: Vec<PathOrdering<P>>,
    /// The location where the nozzle is assumed to start from before printing these parts.
    pub start_point: Point2LL,
    /// The direction in which to print montonically, encoded as vector of length 1.
    monotonic_vector: Point2D,
    /// Maximum distance at which lines are considered to be adjacent.
    max_adjacent_distance: Coord,
    /// Indicates whether the lines should be printed in 2 passes interlaced
    interlaced: bool,
}

#[inline]
fn project_to_vector(point: Point2LL, direction: &Point2D) -> f64 {
    Point2D::dot(&Point2D::new(point.x as f64, point.y as f64), direction)
}

#[inline]
fn min_f(a: f64, b: f64) -> f64 {
    if b < a {
        b
    } else {
        a
    }
}
#[inline]
fn max_f(a: f64, b: f64) -> f64 {
    if a < b {
        b
    } else {
        a
    }
}

impl<P: Copy> PathOrderMonotonic<P> {
    pub fn new(monotonic_direction: AngleRadians, max_adjacent_distance: Coord, start_point: Point2LL, interlaced: bool) -> Self {
        let d = monotonic_direction.value();
        PathOrderMonotonic {
            paths: Vec::new(),
            start_point,
            // The monotonic vector needs to rotate clockwise instead of counter-clockwise, the same as how the infill patterns are generated.
            monotonic_vector: Point2D::new(-d.cos(), d.sin()),
            max_adjacent_distance,
            interlaced,
        }
    }

    pub fn add_polygon(&mut self, polygon: P, vertices: Arc<Vec<Point2LL>>) {
        self.paths.push(PathOrdering::new(polygon, vertices, true, 0, false));
    }

    pub fn add_polyline(&mut self, polyline: P, vertices: Arc<Vec<Point2LL>>) {
        self.paths.push(PathOrdering::new(polyline, vertices, false, 0, false));
    }

    fn project_to_monotonic_vector(&self, point: Point2LL) -> f64 {
        project_to_vector(point, &self.monotonic_vector)
    }

    /// `PathOrder::detectLoops`
    fn detect_loops(&mut self) {
        for path in self.paths.iter_mut() {
            if path.is_closed {
                // Already a polygon. No need to detect loops.
                continue;
            }
            if path.converted.len() < 3 {
                // Not enough vertices to really be a closed loop.
                continue;
            }
            if v_size2(*path.converted.last().unwrap() - path.converted[0]) < COINCIDENT_POINT_DISTANCE * COINCIDENT_POINT_DISTANCE {
                // Endpoints are really close to one another. Consider it a closed loop.
                path.is_closed = true;
            }
        }
    }

    pub fn optimize(&mut self) {
        if self.paths.is_empty() {
            return;
        }

        let mut reordered: Vec<PathOrdering<P>> = Vec::with_capacity(self.paths.len()); // To store the result in.

        // First print all the looping polygons, if there are any.
        let mut polylines: Vec<usize> = Vec::new(); // Also find all polylines and store them in a vector that we can sort in-place without making copies all the time.
        self.detect_loops(); // Always filter out loops. We don't specifically want to print those in monotonic order.
        for i in 0..self.paths.len() {
            if self.paths[i].is_closed || self.paths[i].converted.len() <= 1 {
                reordered.push(self.paths[i].clone());
            } else {
                polylines.push(i);
                // Assign an invalid starting vertex to indicate we don't know the starting point yet.
                self.paths[i].start_vertex = self.paths[i].converted.len();
            }
        }

        // Sort the polylines by their projection on the monotonic vector. This helps find adjacent lines quickly.
        {
            let proj = |s: &Self, i: usize| -> f64 {
                let a_start = s.project_to_monotonic_vector(s.paths[i].converted[0]);
                let a_end = s.project_to_monotonic_vector(*s.paths[i].converted.last().unwrap());
                min_f(a_start, a_end) // The projection of a path is the endpoint furthest back of the two endpoints.
            };
            let keys: HashMap<usize, f64> = polylines.iter().map(|&i| (i, proj(self, i))).collect();
            polylines.sort_by(|a, b| {
                let (ka, kb) = (keys[a], keys[b]);
                if ka < kb {
                    std::cmp::Ordering::Less
                } else if kb < ka {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            });
        }

        if self.interlaced {
            // Separate the lines in two sets by adjacency
            let mut lines_pass1: Vec<usize> = Vec::with_capacity(polylines.len() / 2);
            let mut lines_pass2: Vec<usize> = Vec::with_capacity(polylines.len() / 2);

            let mut current_pass: Option<u8> = None;
            let mut current_projection = 0.0;

            for &path in &polylines {
                let path_projection = self.project_to_monotonic_vector(self.paths[path].converted[0]);
                if current_pass.is_none() {
                    current_pass = Some(1);
                    current_projection = path_projection;
                } else if (path_projection - current_projection) > (self.max_adjacent_distance / 2) as f64 {
                    current_pass = if current_pass == Some(1) { Some(2) } else { Some(1) };
                    current_projection = path_projection;
                }

                if current_pass == Some(1) {
                    lines_pass1.push(path);
                } else {
                    lines_pass2.push(path);
                }
            }

            // Process the 2 sets independently and concatenate them to get the final 2-pass monotonic order
            let ordered_lines_pass1 = self.make_ordered_path(&lines_pass1, self.max_adjacent_distance * 2);
            let ordered_lines_pass2 = self.make_ordered_path(&lines_pass2, self.max_adjacent_distance * 2);

            let rest = std::mem::take(&mut reordered);
            reordered.extend(ordered_lines_pass1);
            reordered.extend(rest);
            let rest = std::mem::take(&mut reordered);
            reordered.extend(ordered_lines_pass2);
            reordered.extend(rest);
        } else {
            let lines_ordered = self.make_ordered_path(&polylines, self.max_adjacent_distance);
            let rest = std::mem::take(&mut reordered);
            reordered.extend(lines_ordered);
            reordered.extend(rest);
        }

        self.paths = reordered; // Store the resulting list in the main paths.
    }

    fn set_start_vertex(path: &mut PathOrdering<P>, backwards: bool) {
        if backwards {
            path.start_vertex = path.converted.len() - 1;
            path.backwards = true;
        } else {
            path.start_vertex = 0;
            path.backwards = false;
        }
    }

    fn optimize_closest_start_point(path: &mut PathOrdering<P>, current_pos: &mut Point2LL) {
        if path.start_vertex == path.converted.len() {
            let dist_start = v_size2(*current_pos - path.converted[0]);
            let dist_end = v_size2(*current_pos - *path.converted.last().unwrap());
            Self::set_start_vertex(path, dist_start >= dist_end);
        }
        *current_pos = path.converted[path.converted.len() - 1 - path.start_vertex]; // Opposite of the start vertex.
    }

    fn get_farthest_endpoint(polyline: &PathOrdering<P>, point: Point2LL) -> usize {
        let front_dist = v_size2(polyline.converted[0] - point);
        let back_dist = v_size2(*polyline.converted.last().unwrap() - point);
        if front_dist < back_dist {
            polyline.converted.len() - 1
        } else {
            0
        }
    }

    fn can_connect_to_polyline(&self, nearby_endpoint: Point2LL, found_path: &SparsePointGridInclusiveElem<usize>) -> bool {
        let p = &self.paths[found_path.val];
        p.start_vertex == p.converted.len() // Don't find any line already in the string.
            && v_size2(found_path.point - nearby_endpoint) < COINCIDENT_POINT_DISTANCE * COINCIDENT_POINT_DISTANCE // And only find close lines.
    }

    fn find_close(&self, grid: &SparsePointGridInclusive<usize>, endpoint: Point2LL) -> Option<SparsePointGridInclusiveElem<usize>> {
        let nearby = grid.get_nearby(endpoint, COINCIDENT_POINT_DISTANCE);
        nearby.into_iter().find(|found| self.can_connect_to_polyline(endpoint, found))
    }

    /// For a given path, find the polylines that are connected end-to-end with it.
    fn find_polyline_string(&mut self, polyline: usize, line_bucket_grid: &SparsePointGridInclusive<usize>) -> VecDeque<usize> {
        let mut result: VecDeque<usize> = VecDeque::new();
        if self.paths[polyline].converted.is_empty() {
            return result;
        }

        // Find the two endpoints of the polyline string, on either side.
        result.push_back(polyline);
        self.paths[polyline].start_vertex = 0;
        let mut first_endpoint = self.paths[polyline].converted[0];
        let mut last_endpoint = *self.paths[polyline].converted.last().unwrap();
        let mut close_line_before = self.find_close(line_bucket_grid, first_endpoint);
        let mut close_line_after = self.find_close(line_bucket_grid, last_endpoint);

        while let Some(found) = close_line_before {
            let first = found.val;
            result.push_front(first); // Store this one in the sequence. It's a good one.
            let farthest_vertex = Self::get_farthest_endpoint(&self.paths[first], found.point); // Get to the opposite side.
            self.paths[first].start_vertex = farthest_vertex;
            self.paths[first].backwards = farthest_vertex != 0;
            first_endpoint = self.paths[first].converted[farthest_vertex];
            close_line_before = self.find_close(line_bucket_grid, first_endpoint);
        }
        while let Some(found) = close_line_after {
            let last = found.val;
            result.push_back(last);
            let farthest_vertex = Self::get_farthest_endpoint(&self.paths[last], found.point); // Get to the opposite side.
            self.paths[last].start_vertex = if farthest_vertex == 0 { self.paths[last].converted.len() - 1 } else { 0 };
            self.paths[last].backwards = farthest_vertex != 0;
            last_endpoint = self.paths[last].converted[farthest_vertex];
            close_line_after = self.find_close(line_bucket_grid, last_endpoint);
        }

        // Figure out which of the two endpoints to start with: The one monotonically earliest.
        let first_projection = self.project_to_monotonic_vector(first_endpoint);
        let last_projection = self.project_to_monotonic_vector(last_endpoint);
        // If the last endpoint should be printed first (unlikely due to monotonic start, but possible), flip the whole polyline!
        if last_projection < first_projection {
            result.make_contiguous().reverse();
            for &path in result.iter() {
                // Also reverse their start_vertex.
                let p = &mut self.paths[path];
                p.start_vertex = if p.start_vertex == 0 { p.converted.len() - 1 } else { 0 };
                p.backwards = !p.backwards;
            }
        }

        if result.len() == 1 {
            let p = &mut self.paths[result[0]];
            p.start_vertex = p.converted.len(); // Reset start vertex as "unknown" again if it's not a string of polylines.
        }
        result
    }

    /// Get the lines that are adjacent to the given line (later in the monotonic order).
    fn get_overlapping_lines(&self, polyline_pos: usize, perpendicular: &Point2D, polylines: &[usize], max_adjacent_distance: Coord) -> Vec<usize> {
        let me = &self.paths[polylines[polyline_pos]];
        let mad = max_adjacent_distance as f64;
        // How far this extends in the monotonic direction, to make sure we only go up to max_adjacent_distance in that direction.
        let start_monotonic = self.project_to_monotonic_vector(me.converted[0]);
        let end_monotonic = self.project_to_monotonic_vector(*me.converted.last().unwrap());
        let my_farthest_monotonic = max_f(start_monotonic, end_monotonic);
        let my_closest_monotonic = min_f(start_monotonic, end_monotonic);
        let my_farthest_monotonic_padded = my_farthest_monotonic + mad;
        let my_closest_monotonic_padded = my_closest_monotonic - mad;
        // How far this line reaches in the perpendicular direction -- the range at which the line overlaps other lines.
        let my_start = project_to_vector(me.converted[0], perpendicular);
        let my_end = project_to_vector(*me.converted.last().unwrap(), perpendicular);
        let my_farthest = max_f(my_start, my_end);
        let my_closest = min_f(my_start, my_end);
        let my_farthest_padded = my_farthest + mad;
        let my_closest_padded = my_closest - mad;

        let mut overlapping_lines = Vec::new();
        for &overlapping_line in &polylines[polyline_pos + 1..] {
            let them = &self.paths[overlapping_line];
            // Don't go beyond the maximum adjacent distance.
            let start_their_projection = self.project_to_monotonic_vector(them.converted[0]);
            let end_their_projection = self.project_to_monotonic_vector(*them.converted.last().unwrap());
            let their_farthest_projection = max_f(start_their_projection, end_their_projection);
            let their_closest_projection = min_f(start_their_projection, end_their_projection);
            // Multiply by the length of the vector since we need to compare actual distances here.
            if their_closest_projection > my_farthest_monotonic_padded || my_closest_monotonic_padded > their_farthest_projection {
                break; // Too far. This line and all subsequent lines are not adjacent anymore, even though they might be side-by-side.
            }

            // Does this one overlap?
            let their_start = project_to_vector(them.converted[0], perpendicular);
            let their_end = project_to_vector(*them.converted.last().unwrap(), perpendicular);
            let their_farthest = max_f(their_start, their_end);
            let their_closest = min_f(their_start, their_end);
            if (my_closest_padded >= their_closest && my_closest_padded <= their_farthest)
                || (my_farthest_padded >= their_closest && my_farthest_padded <= their_farthest)
                || (their_closest >= my_closest_padded && their_farthest <= my_farthest_padded)
            {
                overlapping_lines.push(overlapping_line);
            }
        }

        overlapping_lines
    }

    /// Order the given polylines (indices into `paths`) monotonically.
    fn make_ordered_path(&mut self, polylines: &[usize], max_adjacent_distance: Coord) -> Vec<PathOrdering<P>> {
        let mut reordered: Vec<PathOrdering<P>> = Vec::with_capacity(polylines.len());

        // Create a bucket grid to be able to find adjacent lines quickly.
        let mut line_bucket_grid: SparsePointGridInclusive<usize> = SparsePointGridInclusive::new(2000, 0); // Grid size of 2mm.
        for &polyline in polylines {
            let p = &self.paths[polyline];
            if !p.converted.is_empty() {
                line_bucket_grid.insert(p.converted[0], polyline);
                line_bucket_grid.insert(*p.converted.last().unwrap(), polyline);
            }
        }

        let perpendicular = self.monotonic_vector.rotated90_ccw(); // To project on to detect adjacent lines.

        let mut connected_lines: HashSet<usize> = HashSet::new(); // Lines that are reachable from one of the starting lines through its connections.
        let mut starting_lines: HashSet<usize> = HashSet::new(); // Starting points of a linearly connected segment.
        let mut connections: HashMap<usize, usize> = HashMap::new(); // For each polyline, which polyline it overlaps with, closest in the projected order.

        for polyline_pos in 0..polylines.len() {
            let polyline = polylines[polyline_pos];
            if connections.contains_key(&polyline) {
                // Already connected this one through a polyline.
                continue;
            }
            // First find out if this polyline is part of a string of polylines.
            let polystring = self.find_polyline_string(polyline, &line_bucket_grid);

            // If we're part of a string of polylines, connect up the whole string and mark all of them as being connected.
            if polystring.len() > 1 {
                starting_lines.insert(polystring[0]);
                for i in 0..polystring.len() - 1 {
                    // Iterate over every pair of adjacent polylines in the string (so skip the last one)!
                    connections.insert(polystring[i], polystring[i + 1]);
                    connected_lines.insert(polystring[i + 1]);

                    // Even though we chain polylines, we still want to find lines that they overlap with.
                    let pos = polylines.iter().position(|&p| p == polystring[i]).unwrap();
                    let overlapping_lines = self.get_overlapping_lines(pos, &perpendicular, polylines, max_adjacent_distance);
                    for overlapping_line in overlapping_lines {
                        if !polystring.contains(&overlapping_line) {
                            // Mark all overlapping lines not part of the string as possible starting points.
                            starting_lines.insert(overlapping_line);
                            starting_lines.insert(polystring[i + 1]); // Also be able to re-start from this point in the string.
                        }
                    }
                }
            } else {
                // Not a string of polylines, but simply adjacent line segments.
                if !connected_lines.contains(&polyline) {
                    // Nothing connects to this line yet.
                    starting_lines.insert(polyline); // This is a starting point then.
                }
                let overlapping_lines = self.get_overlapping_lines(polyline_pos, &perpendicular, polylines, max_adjacent_distance);
                if overlapping_lines.len() == 1 {
                    // If we're not a string of polylines, but adjacent to only one other polyline, create a sequence of polylines.
                    connections.insert(polyline, overlapping_lines[0]);
                    if connected_lines.contains(&overlapping_lines[0]) {
                        // This line was already connected to.
                        starting_lines.insert(overlapping_lines[0]); // Multiple lines connect to it, so we must be able to start there.
                    } else {
                        connected_lines.insert(overlapping_lines[0]);
                    }
                } else {
                    // Either 0 (the for loop terminates immediately) or multiple overlapping lines. For multiple lines we need to mark all of them a starting position.
                    for overlapping_line in overlapping_lines {
                        starting_lines.insert(overlapping_line);
                    }
                }
            }
        }

        let line_projections = |s: &Self, path: usize| -> (Coord, Coord) {
            let p = &s.paths[path];
            let start_projection = (project_to_vector(p.converted[0], &s.monotonic_vector) / PRECISION_FACTOR).round() as Coord;
            let end_projection = (project_to_vector(*p.converted.last().unwrap(), &s.monotonic_vector) / PRECISION_FACTOR).round() as Coord;
            if end_projection < start_projection {
                (end_projection, start_projection)
            } else {
                (start_projection, end_projection)
            }
        };

        // Pre-order lines in a multi-map, so that aligned lines will end up in the same bucket and we can process them in a row.
        // The starting lines are inserted in the order of their index (see NOTES.md: determinism).
        let mut sorted_starting_lines: Vec<usize> = starting_lines.iter().copied().collect();
        sorted_starting_lines.sort_unstable();
        let mut pre_ordered_lines: BTreeMap<(Coord, Coord), Vec<usize>> = BTreeMap::new();
        for starting_line in sorted_starting_lines {
            pre_ordered_lines.entry(line_projections(self, starting_line)).or_default().push(starting_line);
        }

        // Now order the lines, by finding the closest line from the current position in the current bucket (row)
        let mut current_pos = self.start_point;
        while let Some((&first_projection_key, _)) = pre_ordered_lines.iter().next() {
            let lines_on_row = pre_ordered_lines.get_mut(&first_projection_key).unwrap();

            let mut closest_distance = Coord::MAX;
            let mut closest_next_line_pos: Option<usize> = None;
            let mut closest_backwards = false;
            for (pos, &path) in lines_on_row.iter().enumerate() {
                let p = &self.paths[path];
                let dist_start = v_size2(current_pos - p.converted[0]);
                let dist_end = v_size2(current_pos - *p.converted.last().unwrap());
                if dist_start < closest_distance {
                    closest_next_line_pos = Some(pos);
                    closest_distance = dist_start;
                    closest_backwards = false;
                }
                if dist_end < closest_distance {
                    closest_next_line_pos = Some(pos);
                    closest_distance = dist_end;
                    closest_backwards = true;
                }
            }

            let closest_pos = closest_next_line_pos.expect("closest line on row");
            let closest_path = lines_on_row.remove(closest_pos);
            if lines_on_row.is_empty() {
                pre_ordered_lines.remove(&first_projection_key);
            }
            Self::set_start_vertex(&mut self.paths[closest_path], closest_backwards);
            {
                let p = &self.paths[closest_path];
                current_pos = p.converted[p.converted.len() - 1 - p.start_vertex]; // Opposite of the start vertex.
            }
            reordered.push(self.paths[closest_path].clone());

            // Now add the (adjacent) lines to be processed together with this one
            let mut connection = connections.get(&closest_path).map(|v| (closest_path, *v));
            let mut checked_connections: HashMap<usize, usize> = HashMap::new(); // Which connections have already been iterated over
            let mut checked_connection = checked_connections.get(&closest_path).copied();

            while let Some((conn_first, conn_second)) = connection {
                // Stop if the sequence ends or if we hit another starting point
                // or if we have already checked the connection (to avoid falling into a cyclical connection)
                if starting_lines.contains(&conn_second) || checked_connection == Some(conn_second) {
                    break;
                }
                checked_connections.entry(conn_first).or_insert(conn_second);
                let line = conn_second;
                Self::optimize_closest_start_point(&mut self.paths[line], &mut current_pos);
                reordered.push(self.paths[line].clone()); // Plan this line in, to be printed next!
                connection = connections.get(&line).map(|v| (line, *v));
                checked_connection = checked_connections.get(&line).copied();
            }
        }

        reordered
    }
}
