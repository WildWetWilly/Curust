//! `infill/ZigzagConnectorProcessor.h/.cpp` and `infill/NoZigZagConnectorProcessor.h/.cpp`:
//! processing of the zigzag connectors of linear infill patterns.

use crate::geometry::matrix::PointMatrix;
use crate::geometry::point2ll::Point2LL;
use crate::geometry::{OpenLinesSet, OpenPolyline};
use crate::utils::coord::Coord;

/// A zigzag connector processor (`ZigzagConnectorProcessor`), or the no-op variant used for
/// non-zigzag infill (`NoZigZagConnectorProcessor`).
pub struct ZigzagConnectorProcessor<'a> {
    /// Whether this is the no-op `NoZigZagConnectorProcessor`.
    no_zigzag: bool,
    /// The rotation matrix used to enforce the infill angle
    rotation_matrix: &'a PointMatrix,
    /// Whether to include end pieces or not
    use_endpieces: bool,
    /// Whether the end pieces should be connected with the rest part of the infill
    connected_endpieces: bool,
    /// Whether to skip some zags
    skip_some_zags: bool,
    /// Skip 1 zag in every N zags
    zag_skip_count: i32,
    /// indicating whether we are still looking for the first connector or not
    is_first_connector: bool,
    /// scanline segment index of the first connector
    first_connector_end_scanline_index: i32,
    /// scanline segment index of the last connector
    last_connector_index: i32,
    /// The first connector, kept until the polygon is finished.
    first_connector: Vec<Point2LL>,
    /// The current connector.
    current_connector: Vec<Point2LL>,
}

impl<'a> ZigzagConnectorProcessor<'a> {
    pub fn new(rotation_matrix: &'a PointMatrix, use_endpieces: bool, connected_endpieces: bool, skip_some_zags: bool, zag_skip_count: i32) -> Self {
        ZigzagConnectorProcessor {
            no_zigzag: false,
            rotation_matrix,
            use_endpieces,
            connected_endpieces,
            skip_some_zags,
            zag_skip_count,
            is_first_connector: true,
            first_connector_end_scanline_index: 0,
            last_connector_index: 0,
            first_connector: Vec::new(),
            current_connector: Vec::new(),
        }
    }

    /// `NoZigZagConnectorProcessor`
    pub fn new_no_zigzag(rotation_matrix: &'a PointMatrix) -> Self {
        let mut ret = Self::new(rotation_matrix, false, false, false, 0);
        ret.no_zigzag = true;
        ret
    }

    pub fn register_vertex(&mut self, vertex: Point2LL) {
        if self.no_zigzag {
            return;
        }
        if self.is_first_connector {
            self.first_connector.push(vertex);
        } else {
            // it's yet unclear whether the polygon segment should be included, so we store it until we know
            self.current_connector.push(vertex);
        }
    }

    fn should_add_current_connector(&self, start_scanline_idx: i32, end_scanline_idx: i32) -> bool {
        let direction = end_scanline_idx - start_scanline_idx;
        let is_this_endpiece = start_scanline_idx == end_scanline_idx;
        let is_this_connection_even = start_scanline_idx % 2 == 0;
        let mut should_skip_this_connection = false;
        if self.skip_some_zags && self.zag_skip_count > 0 {
            if direction > 0 {
                should_skip_this_connection = start_scanline_idx % self.zag_skip_count == 0;
            } else {
                should_skip_this_connection = (start_scanline_idx - 1) % self.zag_skip_count == 0;
            }
        }

        (is_this_connection_even && !is_this_endpiece && !should_skip_this_connection) // normal connections that should be added
            || (self.use_endpieces && is_this_endpiece) // end piece if it is enabled;
    }

    fn handle_connector_too_close_to_segment(&self, scanline_x: Coord, min_distance_to_scanline: Coord) -> bool {
        if self.current_connector.is_empty() {
            return false;
        }
        !self.current_connector.iter().any(|point| (point.x - scanline_x).abs() >= min_distance_to_scanline)
    }

    pub fn register_scanline_segment_intersection(&mut self, intersection: Point2LL, scanline_index: i32, min_distance_to_scanline: Coord, result: &mut OpenLinesSet) {
        if self.no_zigzag {
            return;
        }
        if self.is_first_connector {
            // process as the first connector if we haven't found one yet
            // this will be processed with the last remaining piece at the end (when the polygon finishes)
            self.first_connector.push(intersection);
            self.first_connector_end_scanline_index = scanline_index;
            self.is_first_connector = false;
        } else {
            // add the current connector if needed
            if self.should_add_current_connector(self.last_connector_index, scanline_index)
                && !self.handle_connector_too_close_to_segment(intersection.x, min_distance_to_scanline)
            {
                let is_this_endpiece = scanline_index == self.last_connector_index;
                self.current_connector.push(intersection);
                let points = std::mem::take(&mut self.current_connector);
                self.add_zag_connector(&points, is_this_endpiece, result);
            }
        }

        // update state
        self.current_connector.clear(); // we're starting a new (odd) zigzag connector, so clear the old one
        self.current_connector.push(intersection);
        self.last_connector_index = scanline_index;
    }

    pub fn register_poly_finished(&mut self, result: &mut OpenLinesSet) {
        if self.no_zigzag {
            return;
        }
        let scanline_start_index = self.last_connector_index;
        let scanline_end_index = self.first_connector_end_scanline_index;
        let is_endpiece = self.is_first_connector || (!self.is_first_connector && scanline_start_index == scanline_end_index);

        // decides whether to add this zag according to the following rules
        if (is_endpiece && self.use_endpieces) || (!is_endpiece && self.should_add_current_connector(scanline_start_index, scanline_end_index)) {
            // for convenience, put every point in one vector
            let first = std::mem::take(&mut self.first_connector);
            self.current_connector.extend(first);

            let points = std::mem::take(&mut self.current_connector);
            self.add_zag_connector(&points, is_endpiece, result);
        }

        // reset member variables
        self.reset();
    }

    fn add_zag_connector(&self, points: &[Point2LL], is_endpiece: bool, result: &mut OpenLinesSet) {
        // don't include the last line yet
        if points.len() < 2 {
            return;
        }
        let mut polyline = OpenPolyline::new(points.to_vec());
        if is_endpiece && !self.connected_endpieces {
            polyline.points.pop();
        }
        self.add_polyline(polyline, result);
    }

    fn reset(&mut self) {
        self.is_first_connector = true;
        self.first_connector_end_scanline_index = 0;
        self.last_connector_index = 0;
        self.first_connector.clear();
        self.current_connector.clear();
    }

    fn add_polyline(&self, mut polyline: OpenPolyline, result: &mut OpenLinesSet) {
        for p in polyline.points.iter_mut() {
            *p = self.rotation_matrix.unapply(*p);
        }
        result.push(polyline);
    }
}
