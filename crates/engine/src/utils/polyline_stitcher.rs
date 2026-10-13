//! `utils/PolylineStitcher.h` (+ `OpenPolylineStitcher`, `MixedPolylineStitcher`,
//! `ExtrusionLineStitcher`): connects polylines whose endpoints are close together.

use crate::geometry::lines_set::{ClosedLinesSet, OpenLinesSet, Shape};
use crate::geometry::point2ll::{v_size, v_size2, Point2LL};
use crate::geometry::polyline::{ClosedPolyline, OpenPolyline, Polygon, Polyline};
use crate::utils::coord::Coord;
use crate::utils::sparse_grid::SparsePointGrid;

/// A line type that can be stitched.
pub trait StitchLine: Clone {
    fn n_points(&self) -> usize;
    fn point_at(&self, i: usize) -> Point2LL;
    fn reverse_line(&mut self);
    /// `chain.length()` for this line type.
    fn line_length(&self) -> Coord;
    /// Append all vertices of `other` (in order, or reversed), optionally skipping the first one.
    fn append_from(&mut self, other: &Self, reversed: bool, skip_first: bool);
    /// `isOdd(line)`.
    fn is_odd_line(&self) -> bool;
    /// `canConnect(a, b)`.
    fn can_connect(&self, other: &Self) -> bool;
    /// `canReverse(ppi)` for the line at ppi.
    fn can_reverse(&self) -> bool;
}

impl StitchLine for OpenPolyline {
    fn n_points(&self) -> usize {
        self.len()
    }
    fn point_at(&self, i: usize) -> Point2LL {
        self[i]
    }
    fn reverse_line(&mut self) {
        self.points.reverse();
    }
    fn line_length(&self) -> Coord {
        self.length()
    }
    fn append_from(&mut self, other: &Self, reversed: bool, skip_first: bool) {
        let skip = skip_first as usize;
        if reversed {
            self.points.extend(other.points.iter().rev().skip(skip));
        } else {
            self.points.extend(other.points.iter().skip(skip));
        }
    }
    fn is_odd_line(&self) -> bool {
        false
    }
    fn can_connect(&self, _other: &Self) -> bool {
        true
    }
    fn can_reverse(&self) -> bool {
        true
    }
}

impl StitchLine for crate::utils::extrusion_line::ExtrusionLine {
    fn n_points(&self) -> usize {
        self.junctions.len()
    }
    fn point_at(&self, i: usize) -> Point2LL {
        self.junctions[i].p
    }
    fn reverse_line(&mut self) {
        self.reverse();
    }
    fn line_length(&self) -> Coord {
        self.length()
    }
    fn append_from(&mut self, other: &Self, reversed: bool, skip_first: bool) {
        let skip = skip_first as usize;
        if reversed {
            self.junctions.extend(other.junctions.iter().rev().skip(skip));
        } else {
            self.junctions.extend(other.junctions.iter().skip(skip));
        }
    }
    fn is_odd_line(&self) -> bool {
        self.is_odd
    }
    fn can_connect(&self, other: &Self) -> bool {
        self.is_odd == other.is_odd
    }
    fn can_reverse(&self) -> bool {
        self.is_odd
    }
}

/// `ExtrusionLineStitcher::stitch`.
pub fn stitch_extrusion_lines(
    lines: &[crate::utils::extrusion_line::ExtrusionLine],
    result_lines: &mut Vec<crate::utils::extrusion_line::ExtrusionLine>,
    result_polygons: &mut Vec<crate::utils::extrusion_line::ExtrusionLine>,
    max_stitch_distance: Coord,
    snap_distance: Coord,
) {
    stitch(lines, result_lines, |l| result_polygons.push(l), max_stitch_distance, snap_distance);
}

#[derive(Clone, Copy, Debug)]
struct Ppi {
    poly_idx: usize,
    point_idx: usize,
}

/// `PolylineStitcher::stitch`. Closed results are handed to `push_closed`.
pub fn stitch<L: StitchLine, F: FnMut(L)>(
    lines: &[L],
    result_lines: &mut Vec<L>,
    mut push_closed: F,
    max_stitch_distance: Coord,
    snap_distance: Coord,
) {
    if lines.is_empty() {
        return;
    }

    let locator = |ppi: &Ppi| lines[ppi.poly_idx].point_at(ppi.point_idx);
    let mut grid = SparsePointGrid::new(max_stitch_distance, lines.len() * 2, locator);

    // populate grid
    for (line_idx, line) in lines.iter().enumerate() {
        grid.insert(Ppi { poly_idx: line_idx, point_idx: 0 });
        grid.insert(Ppi { poly_idx: line_idx, point_idx: line.n_points().wrapping_sub(1) });
    }

    let mut processed = vec![false; lines.len()];

    for line_idx in 0..lines.len() {
        if processed[line_idx] {
            continue;
        }
        processed[line_idx] = true;
        let line = lines[line_idx].clone();
        let mut should_close = line.is_odd_line();

        let mut chain = line;
        let mut closest_is_closing_polygon = false;
        for go_in_reverse_direction in [false, true] {
            // first go in the unreversed direction, to connect chains that start at the end of the line
            if go_in_reverse_direction {
                // try extending chain in the other direction
                chain.reverse_line();
            }
            let mut chain_length = chain.line_length();

            loop {
                let from = chain.point_at(chain.n_points() - 1);

                let mut closest: Option<Ppi> = None;
                let mut closest_distance = Coord::MAX;
                grid.process_nearby(from, max_stitch_distance, |nearby: &Ppi| {
                    let mut is_closing_segment = false;
                    let nearby_p = lines[nearby.poly_idx].point_at(nearby.point_idx);
                    let mut dist = v_size(nearby_p - from);
                    if dist > max_stitch_distance {
                        return true; // keep looking
                    }
                    if v_size2(nearby_p - chain.point_at(0)) < snap_distance * snap_distance {
                        if chain_length + dist < 3 * max_stitch_distance // prevent closing of small poly, cause it might be able to continue making a larger polyline
                            || chain.n_points() <= 2
                        {
                            // don't make 2 vert polygons
                            return true; // look for a better next line
                        }
                        is_closing_segment = true;
                        if !should_close {
                            dist += 10; // prefer continuing polyline over closing a polygon; avoids closed zigzags from being printed separately
                            // continue to see if closing segment is also the closest
                            // there might be a segment smaller than [max_stitch_distance] which closes the polygon better
                        } else {
                            dist -= 10;
                            // Lower distance if closing is preferred
                        }
                    } else if processed[nearby.poly_idx] {
                        // it was already moved to output
                        return true; // keep looking for a connection
                    }
                    let mut nearby_would_be_reversed = nearby.point_idx != 0;
                    nearby_would_be_reversed = nearby_would_be_reversed != go_in_reverse_direction; // flip nearby_would_be_reversed when searching in the reverse direction
                    if !lines[nearby.poly_idx].can_reverse() && nearby_would_be_reversed {
                        // connecting the segment would reverse the polygon direction
                        return true; // keep looking for a connection
                    }
                    if !chain.can_connect(&lines[nearby.poly_idx]) {
                        return true; // keep looking for a connection
                    }
                    if dist < closest_distance {
                        closest_distance = dist;
                        closest = Some(*nearby);
                        closest_is_closing_polygon = is_closing_segment;
                    }
                    if dist < snap_distance {
                        // we have found a good enough next line
                        return false; // stop looking for alternatives
                    }
                    true // keep processing elements
                });

                let closest = match closest {
                    Some(c) if !closest_is_closing_polygon => c,
                    _ => break, // we couldn't find any next line
                };

                let closest_line = &lines[closest.poly_idx];
                let segment_dist = v_size(chain.point_at(chain.n_points() - 1) - closest_line.point_at(closest.point_idx));
                debug_assert!(segment_dist <= max_stitch_distance + 10);
                let old_size = chain.n_points();
                if closest.point_idx == 0 {
                    chain.append_from(closest_line, false, segment_dist < snap_distance);
                } else {
                    chain.append_from(closest_line, true, segment_dist < snap_distance);
                }
                for i in old_size..chain.n_points() {
                    chain_length += v_size(chain.point_at(i) - chain.point_at(i - 1));
                }
                should_close = should_close & !closest_line.is_odd_line(); // If we connect an even to an odd line, we should no longer try to close it.
                debug_assert!(!processed[closest.poly_idx]);
                processed[closest.poly_idx] = true;
            }

            if closest_is_closing_polygon {
                if go_in_reverse_direction {
                    // re-reverse chain to retain original direction
                    // NOTE: not sure if this code could ever be reached, since if a polygon can be closed that should be already possible in the forward direction
                    chain.reverse_line();
                }
                break; // don't consider reverse direction
            }
        }
        if closest_is_closing_polygon {
            push_closed(chain);
        } else {
            if !lines[line_idx].can_reverse() {
                // Since the original line is not reversible, its direction should be preserved.
                chain.reverse_line();
            }
            result_lines.push(chain);
        }
    }
}

/// `OpenPolylineStitcher::stitch(lines, result_lines, result_polygons: Shape, ...)`:
/// closed results become explicitly closed polygons.
pub fn stitch_open_into_shape(lines: &OpenLinesSet, result_lines: &mut OpenLinesSet, result_polygons: &mut Shape, max_stitch_distance: Coord, snap_distance: Coord) {
    stitch(
        &lines.lines,
        &mut result_lines.lines,
        |chain: OpenPolyline| result_polygons.push(Polygon::new(chain.points, true)),
        max_stitch_distance,
        snap_distance,
    );
}

/// `PolylineStitcher<OpenLinesSet, ClosedLinesSet, OpenPolyline, Point2LL>::stitch`.
pub fn stitch_open_into_closed(lines: &OpenLinesSet, result_lines: &mut OpenLinesSet, result_polygons: &mut ClosedLinesSet, max_stitch_distance: Coord, snap_distance: Coord) {
    stitch(
        &lines.lines,
        &mut result_lines.lines,
        |chain: OpenPolyline| result_polygons.push(ClosedPolyline::new(chain.points, true)),
        max_stitch_distance,
        snap_distance,
    );
}
