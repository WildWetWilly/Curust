//! `ExtruderPlan.h/.cpp` and the `ExtruderPlan` methods of `LayerPlan.cpp`: the paths planned for a
//! single extruder on a single layer.

use crate::gcode_export::GCodeExport;
use crate::geometry::point2ll::Point2LL;
use crate::geometry::point3ll::Point3LL;
use crate::path_planning::{FanSpeedLayerTimeSettings, GCodePath, NozzleTempInsert, TimeMaterialEstimates};
use crate::retraction_config::RetractionConfig;
use crate::settings::types::{Duration, LayerIndex, Ratio};
use crate::utils::coord::{int2mm, Coord};
use std::collections::VecDeque;

/// An entry of the list of nozzle temperature inserts. The id replaces the `std::list` iterator
/// identity used to erase a specific insert.
#[derive(Clone, Copy, Debug)]
pub struct InsertEntry {
    pub id: u64,
    pub insert: NozzleTempInsert,
}

#[derive(Clone, Debug)]
pub struct ExtruderPlan {
    /// The extruder used for this paths in the current plan.
    pub extruder_nr: usize,
    pub(crate) layer_nr: LayerIndex,
    pub(crate) is_initial_layer: bool,
    pub(crate) is_raft_layer: bool,
    pub(crate) layer_thickness: Coord,
    pub(crate) fan_speed_layer_time_settings: FanSpeedLayerTimeSettings,
    pub(crate) retraction_config: RetractionConfig,
    /// The paths planned for this extruder
    pub paths: Vec<GCodePath>,
    /// The nozzle temperature command inserts, to be inserted in between segments
    pub(crate) inserts: VecDeque<InsertEntry>,
    next_insert_id: u64,
    pub(crate) heated_pre_travel_time: f64,
    pub(crate) required_start_temperature: f64,
    pub(crate) extrusion_temperature: Option<f64>,
    /// The command to heat from the printing temperature of this extruder plan to the printing
    /// temperature of the next extruder plan (if it has the same extruder).
    pub(crate) extrusion_temperature_command: Option<u64>,
    pub(crate) prev_extruder_standby_temp: Option<f64>,
    pub(crate) estimates: TimeMaterialEstimates,
    pub(crate) slowest_path_speed: f64,
    pub(crate) extra_time: f64,
    pub(crate) fan_speed: f64,
    pub(crate) temperature_factor: f64,
}

impl ExtruderPlan {
    pub fn new(
        extruder: usize,
        layer_nr: LayerIndex,
        is_initial_layer: bool,
        is_raft_layer: bool,
        layer_thickness: Coord,
        fan_speed_layer_time_settings: FanSpeedLayerTimeSettings,
        retraction_config: RetractionConfig,
    ) -> Self {
        ExtruderPlan {
            extruder_nr: extruder,
            layer_nr,
            is_initial_layer,
            is_raft_layer,
            layer_thickness,
            fan_speed_layer_time_settings,
            retraction_config,
            paths: Vec::new(),
            inserts: VecDeque::new(),
            next_insert_id: 0,
            heated_pre_travel_time: 0.0,
            required_start_temperature: -1.0,
            extrusion_temperature: None,
            extrusion_temperature_command: None,
            prev_extruder_standby_temp: None,
            estimates: TimeMaterialEstimates::default(),
            slowest_path_speed: 0.0,
            extra_time: 0.0,
            fan_speed: 0.0,
            temperature_factor: 0.0,
        }
    }

    pub fn insert_command(&mut self, insert: NozzleTempInsert) {
        let id = self.next_insert_id;
        self.next_insert_id += 1;
        self.inserts.push_back(InsertEntry { id, insert });
    }

    /// The id of the last insert (`--inserts_.end()`).
    pub(crate) fn last_insert_id(&self) -> Option<u64> {
        self.inserts.back().map(|e| e.id)
    }

    /// `inserts_.erase(iterator)`
    pub(crate) fn erase_insert(&mut self, id: u64) {
        if let Some(pos) = self.inserts.iter().position(|e| e.id == id) {
            self.inserts.remove(pos);
        }
    }

    /// `inserts_.sort()`: a stable sort using `NozzleTempInsert::operator<`.
    pub(crate) fn sort_inserts(&mut self) {
        let mut v: Vec<InsertEntry> = self.inserts.drain(..).collect();
        v.sort_by(|a, b| {
            if a.insert.less(&b.insert) {
                std::cmp::Ordering::Less
            } else if b.insert.less(&a.insert) {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        });
        self.inserts = v.into();
    }

    /// Handle the inserts to be inserted before this path_idx.
    pub fn handle_inserts(&mut self, path_idx: usize, gcode: &mut GCodeExport, cumulative_path_time: f64) {
        while let Some(front) = self.inserts.front() {
            if path_idx >= front.insert.path_idx && front.insert.time_after_path_start < cumulative_path_time {
                front.insert.write(gcode);
                self.inserts.pop_front();
            } else {
                break;
            }
        }
    }

    pub fn handle_all_remaining_inserts(&mut self, gcode: &mut GCodeExport) {
        while let Some(front) = self.inserts.pop_front() {
            front.insert.write(gcode);
        }
    }

    pub fn set_fan_speed(&mut self, fan_speed: f64) {
        self.fan_speed = fan_speed;
    }

    pub fn get_fan_speed(&self) -> f64 {
        self.fan_speed
    }

    pub fn apply_back_pressure_compensation(&mut self, back_pressure_compensation: Ratio) {
        const EPSILON_SPEED_FACTOR: f64 = 0.001;
        for path in self.paths.iter_mut() {
            let nominal_width_for_path = path.config.get_line_width() as f64;
            if path.width_factor <= 0.0 || nominal_width_for_path <= 0.0 || path.config.is_travel_path() || path.config.is_bridge_path() {
                continue;
            }
            let line_width_for_path = path.width_factor * nominal_width_for_path;
            let v = 1.0 + (nominal_width_for_path / line_width_for_path - 1.0) * back_pressure_compensation;
            path.speed_back_pressure_factor = if EPSILON_SPEED_FACTOR < v { v } else { EPSILON_SPEED_FACTOR };
        }
    }

    pub fn find_first_printed_mesh(&self) -> Option<usize> {
        for path in &self.paths {
            if path.mesh.is_some() {
                return path.mesh;
            }
        }
        None
    }

    /// Applying speed corrections for minimal layer times.
    pub(crate) fn force_minimal_layer_time(&mut self, min_time: f64, time_other_extr_plans: f64) -> bool {
        let minimal_speed = self.fan_speed_layer_time_settings.cool_min_speed;
        let travel_time = self.estimates.get_travel_time();
        let extrude_time = self.estimates.extrude_time;

        let total_time = travel_time + extrude_time + time_other_extr_plans;
        const EPSILON: f64 = 0.01;

        let mut total_extrude_time_at_minimum_speed = 0.0;
        let mut total_extrude_time_at_slowest_speed = 0.0;
        for path in &self.paths {
            total_extrude_time_at_minimum_speed += path.estimates.extrude_time_at_minimum_speed;
            total_extrude_time_at_slowest_speed += path.estimates.extrude_time_at_slowest_path_speed;
        }

        if total_time < min_time - EPSILON && extrude_time > 0.0 {
            let min_extrude_time = min_time - (total_time - extrude_time);

            enum SlowDown {
                Target(f64),
                Interpolate(f64),
            }
            let slow_down;

            if min_extrude_time >= total_extrude_time_at_minimum_speed {
                // Even at cool min speed extrusion is not taken enough time. So speed is set to cool min speed.
                slow_down = SlowDown::Target(minimal_speed);
                self.temperature_factor = 1.0;

                self.estimates.extrude_time = total_extrude_time_at_minimum_speed;
                if min_time - total_extrude_time_at_minimum_speed - travel_time > EPSILON {
                    self.extra_time = min_time - total_extrude_time_at_minimum_speed - travel_time;
                }
            } else if min_extrude_time >= total_extrude_time_at_slowest_speed && (total_extrude_time_at_minimum_speed - total_extrude_time_at_slowest_speed).abs() >= EPSILON {
                // Slowing down to the slowest path speed is not sufficient, need to slow down further to the minimum speed.
                let factor = (1.0 / total_extrude_time_at_minimum_speed - 1.0 / min_extrude_time)
                    / (1.0 / total_extrude_time_at_minimum_speed - 1.0 / total_extrude_time_at_slowest_speed);
                slow_down = SlowDown::Target(minimal_speed * (1.0 - factor) + self.slowest_path_speed * factor);
                self.temperature_factor = 1.0 - factor;

                self.estimates.extrude_time = min_extrude_time;
            } else {
                // Slowing down to the slowest_speed is sufficient to respect the minimum layer time.
                let factor = (1.0 / total_extrude_time_at_slowest_speed - 1.0 / min_extrude_time) / (1.0 / total_extrude_time_at_slowest_speed - 1.0 / extrude_time);
                slow_down = SlowDown::Interpolate(factor);

                self.estimates.extrude_time = min_extrude_time;
            }

            let slowest_path_speed = self.slowest_path_speed;
            for path in self.paths.iter_mut() {
                if path.is_travel_path() {
                    continue;
                }
                let path_speed = path.config.get_speed() * path.speed_factor;
                let slow_down_factor: Ratio = match slow_down {
                    SlowDown::Target(target_speed) => min_f(target_speed / path_speed, 1.0),
                    SlowDown::Interpolate(factor) => {
                        let actual_target_speed = slowest_path_speed * (1.0 - factor) + path_speed * factor;
                        min_f(actual_target_speed / path_speed, 1.0)
                    }
                };
                path.speed_factor *= slow_down_factor;
                path.estimates.extrude_time /= slow_down_factor;
            }

            return true;
        }
        false
    }

    pub(crate) fn get_retract_time(&self, path: &GCodePath) -> f64 {
        self.retraction_config.distance / (if path.retract { self.retraction_config.speed } else { self.retraction_config.prime_speed })
    }

    pub(crate) fn get_point_to_point_time(&self, p0: &Point3LL, p1: &Point3LL, path: &GCodePath) -> (f64, f64) {
        let length = (*p0 - *p1).v_size_mm();
        (length, length / (path.config.get_speed() * path.speed_factor))
    }

    pub(crate) fn compute_naive_time_estimates(&mut self, starting_position: Point2LL) -> TimeMaterialEstimates {
        let mut p0 = Point3LL::from_2d(starting_position, 0);

        let min_path_speed = self.fan_speed_layer_time_settings.cool_min_speed;
        let mut slowest = f64::MAX;
        for path in &self.paths {
            if !path.is_travel_path() {
                slowest = min_f(slowest, path.config.get_speed() * path.speed_factor);
            }
        }
        self.slowest_path_speed = slowest;

        let mut was_retracted = false; // wrong assumption; won't matter that much. (TODO)
        let _ = &mut was_retracted;
        let layer_thickness = self.layer_thickness;
        let retraction_config = self.retraction_config;
        for path in self.paths.iter_mut() {
            let mut is_extrusion_path = false;

            path.estimates.extrude_time_at_minimum_speed = 0.0;
            path.estimates.extrude_time_at_slowest_path_speed = 0.0;

            #[derive(Clone, Copy)]
            enum Target {
                Extrude,
                Retracted,
                Unretracted,
            }
            let target;
            if !path.is_travel_path() {
                is_extrusion_path = true;
                target = Target::Extrude;
            } else {
                target = if path.retract { Target::Retracted } else { Target::Unretracted };
                if path.retract != was_retracted {
                    let retract_unretract_time = if path.retract {
                        retraction_config.distance / retraction_config.speed
                    } else {
                        retraction_config.distance / retraction_config.prime_speed
                    };
                    path.estimates.retracted_travel_time += 0.5 * retract_unretract_time;
                    path.estimates.unretracted_travel_time += 0.5 * retract_unretract_time;
                }
            }
            let line_width = path.config.get_line_width();
            let speed = path.config.get_speed() * path.speed_factor;
            for i in 0..path.points.len() {
                let p1 = path.points[i];
                let length = (p0 - p1).v_size_mm();
                if is_extrusion_path {
                    if length > 0.0 {
                        path.estimates.extrude_time_at_minimum_speed += length / min_path_speed;
                        path.estimates.extrude_time_at_slowest_path_speed += length / self.slowest_path_speed;
                    }
                    path.estimates.material += length * int2mm(layer_thickness) * int2mm(line_width);
                }
                let this_time = length / speed;
                match target {
                    Target::Extrude => path.estimates.extrude_time += this_time,
                    Target::Retracted => path.estimates.retracted_travel_time += this_time,
                    Target::Unretracted => path.estimates.unretracted_travel_time += this_time,
                }
                p0 = p1;
            }
            self.estimates += path.estimates;
        }
        self.estimates
    }

    pub(crate) fn process_fan_speed_for_minimal_layer_time(&mut self, min_time: Duration, time_other_extr_plans: f64) {
        let total_layer_time = self.estimates.get_total_time() + time_other_extr_plans;
        let layer_time_diff = (self.fan_speed_layer_time_settings.cool_min_layer_time_fan_speed_max - min_time).value();
        let fraction_of_slope = if layer_time_diff != 0.0 { ((total_layer_time - min_time.value()) / layer_time_diff).clamp(0.0, 1.0) } else { 1.0 };
        self.fan_speed = cppstd::lerp(self.fan_speed_layer_time_settings.cool_fan_speed_max, self.fan_speed, fraction_of_slope);
    }

    pub(crate) fn process_fan_speed_for_first_layers(&mut self) {
        let s = &self.fan_speed_layer_time_settings;
        self.fan_speed = s.cool_fan_speed_min;
        if self.layer_nr < s.cool_fan_full_layer && s.cool_fan_full_layer > 0 && !self.is_raft_layer {
            self.fan_speed = s.cool_fan_speed_0 + (self.fan_speed - s.cool_fan_speed_0) * (self.layer_nr.max(0) as f64) / (s.cool_fan_full_layer as f64);
        }
    }
}

/// `std::min(a, b)` for doubles: `(b < a) ? b : a`.
#[inline]
fn min_f(a: f64, b: f64) -> f64 {
    if b < a {
        b
    } else {
        a
    }
}
