//! `communication/Communication.h`: the channel to the front-end.

pub mod command_line;
pub mod job;

use crate::extruder_train::ExtruderTrain;
use crate::geometry::point3ll::Point3LL;
use crate::print_feature::PrintFeatureType;
use crate::settings::types::Velocity;
use crate::utils::coord::Coord;

pub trait Communication {
    /// Test if there are more slices to be queued.
    fn has_slice(&self) -> bool;
    /// If the communication with the front-end is sequential (the G-code is output in one go).
    fn is_sequential(&self) -> bool;
    fn send_progress(&self, progress: f64);
    fn send_layer_complete(&mut self, layer_nr: i64, z: Coord, thickness: Coord);
    fn send_line_to(&mut self, feature_type: PrintFeatureType, to: Point3LL, line_width: Coord, line_thickness: Coord, velocity: Velocity);
    fn send_current_position(&mut self, position: Point3LL);
    fn set_extruder_for_send(&mut self, extruder: &ExtruderTrain);
    fn set_layer_for_send(&mut self, layer_nr: i64);
    fn send_optimized_layer_data(&mut self);
    fn send_print_time_material_estimates(&self);
    fn begin_gcode(&mut self);
    fn flush_gcode(&mut self);
    fn send_gcode_prefix(&self, prefix: &str);
    fn send_slice_uuid(&self, slice_uuid: &str);
    fn send_finished_slicing(&self);
    fn slice_next(&mut self);
}
