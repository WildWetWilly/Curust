//! `infill/AbstractLinesInfill`, `infill/GyroidInfill` and `infill/RegularNGonalInfill`: infill
//! patterns generated as parallel (curvy) lines which are then fitted into the area.

use crate::geometry::matrix::PointMatrix;
use crate::geometry::point2ll::{v_size2, v_size2f, Point2LL};
use crate::geometry::{OpenLinesSet, OpenPolyline, Polyline, Shape};
use crate::settings::types::AngleDegrees;
use crate::utils::aabb::AABB;
use crate::utils::coord::Coord;
use crate::utils::linear_alg_2d as la;
use crate::utils::polyline_stitcher;
use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegularNGonType {
    Hexagon,
    Octagon,
}

/// The `AbstractLinesInfill` implementations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbstractLinesInfill {
    Gyroid,
    RegularNGonal(RegularNGonType),
}

/// Helper structure that contain data about the split lines to be joined
#[derive(Clone, Copy, Debug)]
struct SplitLines {
    /// The start and end positions of the intersected polyline
    chain: [Point2LL; 2],
    /// The ID of the original polyline this one is a part from
    column_id: usize,
}

#[derive(Clone, Copy, Debug)]
struct SegmentsPattern {
    segments: [Point2LL; 4],
    delta_x: Coord,
}

impl AbstractLinesInfill {
    /// Generate the infill pattern without the infill_multiplier functionality
    #[allow(clippy::too_many_arguments)]
    pub fn generate_infill(
        &self,
        result_polylines: &mut OpenLinesSet,
        result_polygons: &mut Shape,
        zig_zaggify: bool,
        line_distance: Coord,
        in_outline: &Shape,
        z: Coord,
        line_width: Coord,
        rotation: AngleDegrees,
    ) {
        let mut rotated_outline = in_outline.clone();
        let mut rotation_matrix = PointMatrix::from_degrees(rotation.value());
        if rotation.value() != 0.0 {
            rotated_outline.apply_matrix(&rotation_matrix);
        }
        let raw_lines = self.generate_parallel_lines(line_distance, &AABB::from_shape(&rotated_outline), z, line_width);
        let fit_lines = Self::fit_lines(&raw_lines, zig_zaggify, &rotated_outline);
        polyline_stitcher::stitch_open_into_shape(&fit_lines, result_polylines, result_polygons, line_width, 10);
        if rotation.value() != 0.0 {
            rotation_matrix = rotation_matrix.inverse();
            result_polylines.apply_matrix(&rotation_matrix);
            result_polygons.apply_matrix(&rotation_matrix);
        }
    }

    fn generate_parallel_lines(&self, line_distance: Coord, bounding_box: &AABB, z: Coord, line_width: Coord) -> OpenLinesSet {
        match self {
            AbstractLinesInfill::Gyroid => gyroid_parallel_lines(line_distance, bounding_box, z),
            AbstractLinesInfill::RegularNGonal(t) => ngonal_parallel_lines(*t, line_distance, bounding_box, line_width),
        }
    }

    /// Joins alternate chain ends to make a chain of chains.
    fn zig_zaggify(split_lines: &[SplitLines], in_outline: &Shape) -> OpenLinesSet {
        let mut result = OpenLinesSet::default();
        let mut chain_ends_remaining: i32 = (split_lines.len() * 2) as i32;
        let mut connected_to: Vec<[Option<usize>; 2]> = vec![[None, None]; split_lines.len()];

        for outline_poly in in_outline.iter() {
            let mut connector_points: Vec<Point2LL> = Vec::new(); // the points that make up a connector line

            // we need to remember the first chain processed and the path to it from the first outline point
            // so that later we can possibly connect to it from the last chain processed
            let mut first_chain_chain_index: Option<usize> = None;
            let mut path_to_first_chain: Vec<Point2LL> = Vec::new();

            let mut drawing = false; // true when a connector line is being (potentially) created

            // keep track of the chain+point that a connector line started at
            let mut connector_start_chain_index: Option<usize> = None;
            let mut connector_start_point_index: Option<usize> = None;

            let mut cur_point = Point2LL::default(); // current point of interest - either an outline point or a chain end

            // go round all of the region's outline and find the chain ends that meet it
            // quit the loop early if we have seen all the chain ends and are not currently drawing a connector
            let mut outline_point_index = 0usize;
            while (chain_ends_remaining > 0 || drawing) && outline_point_index < outline_poly.len() {
                let op0 = outline_poly[outline_point_index];
                let op1 = outline_poly[(outline_point_index + 1) % outline_poly.len()];
                let mut points_on_outline_chain_index: Vec<usize> = Vec::new();
                let mut points_on_outline_point_index: Vec<usize> = Vec::new();

                // collect the chain ends that meet this segment of the outline
                for (chain_index, split_line) in split_lines.iter().enumerate() {
                    for (point_index, point) in split_line.chain.iter().enumerate() {
                        // don't include chain ends that are close to the segment but are beyond the segment ends
                        let (dist2, beyond) = la::get_dist2_from_line_segment_beyond(op0, *point, op1);
                        if dist2 < 10 && beyond == 0 {
                            points_on_outline_point_index.push(point_index);
                            points_on_outline_chain_index.push(chain_index);
                        }
                    }
                }

                if outline_point_index == 0 || v_size2(op0 - cur_point) > 100 {
                    // this is either the first outline point or it is another outline point that is not too close to cur_point
                    if first_chain_chain_index.is_none() {
                        // include the outline point in the path to the first chain
                        path_to_first_chain.push(op0);
                    }

                    cur_point = op0;
                    if drawing {
                        // include the start point of this outline segment in the connector
                        connector_points.push(op0);
                    }
                }

                // iterate through each of the chain ends that meet the current outline segment
                while !points_on_outline_chain_index.is_empty() {
                    // find the nearest chain end to the current point
                    let mut nearest_point_index = 0usize;
                    let mut nearest_point_dist2 = f32::INFINITY as f64;
                    for pi in 0..points_on_outline_chain_index.len() {
                        let dist2 = v_size2f(split_lines[points_on_outline_chain_index[pi]].chain[points_on_outline_point_index[pi]] - cur_point);
                        if dist2 < nearest_point_dist2 {
                            nearest_point_dist2 = dist2;
                            nearest_point_index = pi;
                        }
                    }
                    let point_index = points_on_outline_point_index[nearest_point_index];
                    let chain_index = points_on_outline_chain_index[nearest_point_index];

                    // make the chain end the current point and add it to the connector line
                    cur_point = split_lines[chain_index].chain[point_index];

                    if drawing && !connector_points.is_empty() && v_size2(cur_point - *connector_points.last().unwrap()) < 100 {
                        // this chain end will be too close to the last connector point so throw away the last connector point
                        connector_points.pop();
                    }
                    connector_points.push(cur_point);

                    if first_chain_chain_index.is_none() {
                        // this is the first chain to be processed, remember it
                        first_chain_chain_index = Some(chain_index);
                        path_to_first_chain.push(cur_point);
                    }

                    if drawing {
                        // add the connector line segments but only if
                        //  1 - the start/end points are not the opposite ends of the same chain
                        //  2 - the other end of the current chain is not connected to the chain the connector line is coming from
                        if Some(chain_index) != connector_start_chain_index && connected_to[chain_index][(point_index + 1) % 2] != connector_start_chain_index {
                            result.push(OpenPolyline::new(connector_points.clone()));
                            drawing = false;
                            connector_points.clear();
                            // remember the connection
                            connected_to[chain_index][point_index] = connector_start_chain_index;
                            connected_to[connector_start_chain_index.unwrap()][connector_start_point_index.unwrap()] = Some(chain_index);
                        } else {
                            // start a new connector from the current location
                            connector_points.clear();
                            connector_points.push(cur_point);

                            // remember the chain+point that the connector started from
                            connector_start_chain_index = Some(chain_index);
                            connector_start_point_index = Some(point_index);
                        }
                    } else {
                        // we have just jumped a gap so now we want to start drawing again
                        drawing = true;

                        // if this connector is the first to be created or we are not connecting chains from the same row/column,
                        // remember the chain+point that this connector is starting from
                        if connector_start_chain_index.is_none() || split_lines[chain_index].column_id != split_lines[connector_start_chain_index.unwrap()].column_id {
                            connector_start_chain_index = Some(chain_index);
                            connector_start_point_index = Some(point_index);
                        }
                    }

                    // done with this chain end
                    points_on_outline_chain_index.remove(nearest_point_index);
                    points_on_outline_point_index.remove(nearest_point_index);

                    // decrement total amount of work to do
                    chain_ends_remaining -= 1;
                }
                outline_point_index += 1;
            }

            // we have now visited all the points in the outline, if a connector was (potentially) being drawn
            // check whether the first chain is already connected to the last chain and, if not, draw the
            // connector between
            if drawing
                && first_chain_chain_index.is_some()
                && first_chain_chain_index != connector_start_chain_index
                && connected_to[first_chain_chain_index.unwrap()][0] != connector_start_chain_index
                && connected_to[first_chain_chain_index.unwrap()][1] != connector_start_chain_index
            {
                // output the connector line segments from the last chain to the first point in the outline
                connector_points.push(outline_poly[0]);
                result.push(OpenPolyline::new(connector_points.clone()));
                // output the connector line segments from the first point in the outline to the first chain
                result.push(OpenPolyline::new(path_to_first_chain.clone()));
            }

            if chain_ends_remaining < 1 {
                break;
            }
        }

        result
    }

    fn fit_lines(raw_lines: &OpenLinesSet, zig_zaggify: bool, in_outline: &Shape) -> OpenLinesSet {
        let mut result = OpenLinesSet::default();
        let mut split_lines: Vec<SplitLines> = Vec::new();

        for (column_id, raw_line) in raw_lines.iter().enumerate() {
            if !raw_line.is_valid() {
                continue;
            }
            let restitch = false;
            let max_stitching_distance: Coord = 0;
            let split_into_segments = false;
            let single = OpenLinesSet::from_line(raw_line.clone());
            let intersected_lines = in_outline.intersection_polylines(&single, restitch, max_stitching_distance, split_into_segments);
            for intersected_line in intersected_lines.iter() {
                result.push(intersected_line.clone());
                if zig_zaggify {
                    split_lines.push(SplitLines { chain: [intersected_line[0], *intersected_line.last().unwrap()], column_id });
                }
            }
        }

        if !split_lines.is_empty() {
            result.push_all_owned(Self::zig_zaggify(&split_lines, in_outline));
        }

        result
    }
}

/// `GyroidInfill::generateParallelLines`: infill based on the gyroid equation
/// sin_x * cos_y + sin_y * cos_z + sin_z * cos_x = 0
fn gyroid_parallel_lines(line_distance: Coord, bounding_box: &AABB, z: Coord) -> OpenLinesSet {
    let mut pitch: i32 = (line_distance as f64 * 2.41) as i32; // this produces similar density to the "line" infill pattern
    let mut num_steps: i32 = 4;
    let mut step: i32 = pitch / num_steps;
    while step > 500 && num_steps < 16 {
        num_steps *= 2;
        step = pitch / num_steps;
    }
    pitch = step * num_steps; // recalculate to avoid precision errors
    let z_rads = 2.0 * PI * z as f64 / pitch as f64;
    let cos_z = z_rads.cos();
    let sin_z = z_rads.sin();
    let mut odd_line_coords: Vec<Coord> = Vec::new();
    let mut even_line_coords: Vec<Coord> = Vec::new();
    let mut result = OpenLinesSet::default();
    let pitch_c = pitch as Coord;
    let step_c = step as Coord;

    if sin_z.abs() <= cos_z.abs() {
        // "vertical" lines
        let phase_offset = (if cos_z < 0.0 { PI } else { 0.0 }) + PI;
        let mut y: Coord = 0;
        while y < pitch_c {
            let y_rads = 2.0 * PI * y as f64 / pitch as f64;
            let a = cos_z;
            let b = (y_rads + phase_offset).sin();
            let odd_c = sin_z * (y_rads + phase_offset).cos();
            let even_c = sin_z * (y_rads + phase_offset + PI).cos();
            let h = (a * a + b * b).sqrt();
            let odd_x_rads = (if h != 0.0 { (odd_c / h).asin() + (b / h).asin() } else { 0.0 }) - PI / 2.0;
            let even_x_rads = (if h != 0.0 { (even_c / h).asin() + (b / h).asin() } else { 0.0 }) - PI / 2.0;
            odd_line_coords.push((odd_x_rads / PI * pitch as f64) as Coord);
            even_line_coords.push((even_x_rads / PI * pitch as f64) as Coord);
            y += step_c;
        }
        let num_coords = odd_line_coords.len() as u32;
        let mut num_columns: u32 = 0;
        let mut x: Coord = ((((bounding_box.min.x / pitch_c) as f64).floor() - 2.25) * pitch as f64) as Coord;
        while x <= bounding_box.max.x + pitch_c / 2 {
            let mut line = OpenPolyline::default();
            let mut y: Coord = ((((bounding_box.min.y / pitch_c) as f64).floor() - 1.0) * pitch as f64) as Coord;
            while y <= bounding_box.max.y + pitch_c {
                for i in 0..num_coords {
                    let c = if num_columns & 1 != 0 { odd_line_coords[i as usize] } else { even_line_coords[i as usize] };
                    line.points.push(Point2LL::new(x + c / 2 + pitch_c, y + (i.wrapping_mul(step as u32)) as Coord));
                }
                y += pitch_c;
            }
            result.push(line);
            num_columns += 1;
            x += pitch_c / 2;
        }
    } else {
        // "horizontal" lines
        let phase_offset = if sin_z < 0.0 { PI } else { 0.0 };
        let mut x: Coord = 0;
        while x < pitch_c {
            let x_rads = 2.0 * PI * x as f64 / pitch as f64;
            let a = sin_z;
            let b = (x_rads + phase_offset).cos();
            let odd_c = cos_z * (x_rads + phase_offset + PI).sin();
            let even_c = cos_z * (x_rads + phase_offset).sin();
            let h = (a * a + b * b).sqrt();
            let odd_y_rads = (if h != 0.0 { (odd_c / h).asin() + (b / h).asin() } else { 0.0 }) + PI / 2.0;
            let even_y_rads = (if h != 0.0 { (even_c / h).asin() + (b / h).asin() } else { 0.0 }) + PI / 2.0;
            odd_line_coords.push((odd_y_rads / PI * pitch as f64) as Coord);
            even_line_coords.push((even_y_rads / PI * pitch as f64) as Coord);
            x += step_c;
        }
        let num_coords = odd_line_coords.len() as u32;
        let mut num_rows: u32 = 0;
        let mut y: Coord = ((((bounding_box.min.y / pitch_c) as f64).floor() - 1.0) * pitch as f64) as Coord;
        while y <= bounding_box.max.y + pitch_c / 2 {
            let mut line = OpenPolyline::default();
            let mut x: Coord = ((((bounding_box.min.x / pitch_c) as f64).floor() - 1.0) * pitch as f64) as Coord;
            while x <= bounding_box.max.x + pitch_c {
                for i in 0..num_coords {
                    let c = if num_rows & 1 != 0 { odd_line_coords[i as usize] } else { even_line_coords[i as usize] };
                    line.points.push(Point2LL::new(x + (i.wrapping_mul(step as u32)) as Coord, y + c / 2));
                }
                x += pitch_c;
            }
            result.push(line);
            num_rows += 1;
            y += pitch_c / 2;
        }
    }

    result
}

fn to_point2ll(x: f64, y: f64) -> Point2LL {
    Point2LL::new(cppstd::llrint(x), cppstd::llrint(y))
}

fn generate_patterns(segment_up_length: f64, segment_right_x: f64, segment_right_y: f64, absolute_delta_x: Coord) -> [SegmentsPattern; 2] {
    let segment_up = to_point2ll(0.0, segment_up_length);
    let segment_right = to_point2ll(segment_right_x, segment_right_y);
    let segment_left = Point2LL::new(-segment_right.x, segment_right.y);

    [
        SegmentsPattern { segments: [segment_up, segment_left, segment_up, segment_right], delta_x: -absolute_delta_x },
        SegmentsPattern { segments: [segment_up, segment_right, segment_up, segment_left], delta_x: absolute_delta_x },
    ]
}

fn generate_regular_ngonal_lines(bounding_box: &AABB, patterns: &[SegmentsPattern; 2], delta_y: Coord, pattern_width: Coord, pattern_height: Coord) -> OpenLinesSet {
    if pattern_width <= 0 || pattern_height <= 0 {
        return OpenLinesSet::default();
    }

    let start_col = (bounding_box.min.x / pattern_width - 1) as i32;
    let end_col = ((bounding_box.max.x / pattern_width) + 1) as i32;
    let start_row = (bounding_box.min.y / pattern_height - 1) as i32;
    let end_row = ((bounding_box.max.y / pattern_height) + 1) as i32;

    let mut raw_lines = OpenLinesSet::default();
    for col in start_col..=end_col {
        for pattern in patterns {
            let mut raw_line = OpenPolyline::default();

            let mut current_position = Point2LL::new(pattern.delta_x + col as Coord * pattern_width, delta_y + start_row as Coord * pattern_height);
            raw_line.points.push(current_position);

            for _row in start_row..=end_row {
                for segment in &pattern.segments {
                    current_position += *segment;
                    raw_line.points.push(current_position);
                }
            }

            raw_lines.push(raw_line);
        }
    }

    raw_lines
}

/// `RegularNGonalInfill::generateParallelLines`
fn ngonal_parallel_lines(type_: RegularNGonType, line_distance: Coord, bounding_box: &AABB, line_width: Coord) -> OpenLinesSet {
    let patterns;
    let delta_y: Coord;
    let pattern_width: Coord;
    let pattern_height: Coord;

    let circumradius = line_distance as f64 / (PI / 6.0).cos();

    match type_ {
        RegularNGonType::Octagon => {
            let segment_length = ((PI / 8.0).cos() * circumradius) / 2.0;
            let segment_right_side = segment_length / 2.0f64.sqrt();
            patterns = generate_patterns(segment_length, segment_right_side, segment_right_side, cppstd::llrint(segment_length / 2.0));
            delta_y = (-(segment_length * 1.5 + segment_right_side)) as Coord;
            pattern_width = (2.0 * segment_length + 2.0 * segment_right_side) as Coord;
            pattern_height = pattern_width;
        }
        RegularNGonType::Hexagon => {
            // Theoretical dimensions of the right segment
            let segment_right_full_x = circumradius * (PI / 6.0).cos();
            let segment_right_full_y = circumradius * (PI / 6.0).sin();

            // Actual dimensions of the right segment, applied with the line_width reduction so that adjacent hexagons don't have overlapping lines
            let segment_right_x = segment_right_full_x - line_width as f64;
            let segment_right_y = segment_right_x * (PI / 6.0).tan();
            let segment_up_length = circumradius + (segment_right_full_y - segment_right_y);

            patterns = generate_patterns(segment_up_length, segment_right_x, segment_right_y, line_width / 2);
            delta_y = (-(segment_up_length * 1.5 + segment_right_y)) as Coord;
            pattern_width = line_distance * 2;
            pattern_height = (segment_up_length * 2.0 + segment_right_y * 2.0) as Coord;
        }
    }

    generate_regular_ngonal_lines(bounding_box, &patterns, delta_y, pattern_width, pattern_height)
}
