//! `gradual_flow/FlowLimitedPath.h` and `gradual_flow/Processor.h`: limit the flow acceleration by
//! splitting paths into segments with gradually changing speed.

use crate::application::current_slice;
use crate::geometry::point3ll::Point3LL;
use crate::path_planning::GCodePath;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FlowState {
    Stable,
    Transition,
    Undefined,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Direction {
    Forward,
    Backward,
}

/// `FlowLimitedPath`. NOTE: like the C++ struct, `flow_` and `total_length` are computed only when
/// the struct is constructed; assigning `speed` later does not update them.
#[derive(Clone, Debug)]
struct FlowLimitedPath<'p> {
    original_gcode_path_data: &'p GCodePath,
    points: Vec<Point3LL>,
    /// um/s
    speed: f64,
    /// um/s
    flow_: f64,
    /// um
    total_length: f64,
}

impl<'p> FlowLimitedPath<'p> {
    fn new(original: &'p GCodePath, points: Vec<Point3LL>, speed: Option<f64>) -> Self {
        let mut p = FlowLimitedPath { original_gcode_path_data: original, points, speed: 0.0, flow_: 0.0, total_length: 0.0 };
        p.speed = speed.unwrap_or_else(|| p.target_speed());
        p.flow_ = p.extrusion_volume_per_mm() * p.speed;
        p.total_length = p.compute_total_length();
        p
    }

    /// um/s
    fn target_speed(&self) -> f64 {
        let o = self.original_gcode_path_data;
        o.config.speed_derivatives.speed * o.speed_factor * 1e3
    }

    fn is_travel(&self) -> bool {
        self.target_flow() <= 0.0
    }

    fn is_retract(&self) -> bool {
        self.original_gcode_path_data.retract
    }

    /// um^3/um
    fn extrusion_volume_per_mm(&self) -> f64 {
        let o = self.original_gcode_path_data;
        o.flow * o.config.line_width as f64 * o.config.layer_thickness as f64 * o.config.get_flow_ratio()
    }

    /// um^3/s
    fn flow(&self) -> f64 {
        self.flow_
    }

    /// um^3/s
    fn target_flow(&self) -> f64 {
        self.extrusion_volume_per_mm() * self.target_speed()
    }

    fn compute_total_length(&self) -> f64 {
        let mut path_length = 0.0;
        if let Some(&first) = self.points.first() {
            let mut last_point = first;
            for &point in &self.points[1..] {
                path_length += ((point.x - last_point.x) as f64).hypot((point.y - last_point.y) as f64);
                last_point = point;
            }
        }
        path_length
    }

    /// s
    fn total_duration(&self) -> f64 {
        self.total_length / self.speed
    }

    /// Splits either the beginning or the end of the path into a new path.
    fn partition(&self, partition_duration: f64, partition_speed: f64, direction: Direction) -> (FlowLimitedPath<'p>, Option<FlowLimitedPath<'p>>, f64) {
        let total_path_duration = self.total_length / partition_speed;
        if partition_duration >= total_path_duration {
            let remaining_partition_duration = partition_duration - total_path_duration;
            let gcode_path = FlowLimitedPath::new(self.original_gcode_path_data, self.points.clone(), Some(partition_speed));
            return (gcode_path, None, remaining_partition_duration);
        }

        let mut current_partition_duration = 0.0;
        let mut partition_index: usize = if direction == Direction::Forward { 0 } else { self.points.len().wrapping_sub(1) };
        let iteration_direction: isize = if direction == Direction::Forward { 1 } else { -1 };
        let mut prev_point = self.points[partition_index];

        loop {
            let next_index = partition_index.wrapping_add(iteration_direction as usize);
            let next_point = if next_index < self.points.len() { self.points[next_index] } else { prev_point };
            let segment_length = ((next_point.x - prev_point.x) as f64).hypot((next_point.y - prev_point.y) as f64);
            let segment_duration = segment_length / partition_speed;

            if current_partition_duration + segment_duration < partition_duration && next_index < self.points.len() {
                prev_point = next_point;
                current_partition_duration += segment_duration;
                partition_index = next_index;
            } else {
                let duration_left = partition_duration - current_partition_duration;
                let segment_ratio = duration_left / segment_duration;
                let partition_x = prev_point.x + ((next_point.x - prev_point.x) as f64 * segment_ratio) as i64;
                let partition_y = prev_point.y + ((next_point.y - prev_point.y) as f64 * segment_ratio) as i64;
                let partition_z = prev_point.z + ((next_point.z - prev_point.z) as f64 * segment_ratio) as i64;
                let partition_point = Point3LL::new(partition_x, partition_y, partition_z);

                let partition_point_index = if direction == Direction::Forward { partition_index + 1 } else { partition_index };

                // points left of the partition_index
                let mut left_points: Vec<Point3LL> = self.points[..partition_point_index].to_vec();
                left_points.push(partition_point);

                // points right of the partition_index
                let mut right_points = vec![partition_point];
                right_points.extend_from_slice(&self.points[partition_point_index..]);

                return match direction {
                    Direction::Forward => {
                        let partition_gcode_path = FlowLimitedPath::new(self.original_gcode_path_data, left_points, Some(partition_speed));
                        let remaining_gcode_path = FlowLimitedPath::new(self.original_gcode_path_data, right_points, Some(self.speed));
                        (partition_gcode_path, Some(remaining_gcode_path), 0.0)
                    }
                    Direction::Backward => {
                        let partition_gcode_path = FlowLimitedPath::new(self.original_gcode_path_data, right_points, Some(partition_speed));
                        let remaining_gcode_path = FlowLimitedPath::new(self.original_gcode_path_data, left_points, Some(self.speed));
                        (partition_gcode_path, Some(remaining_gcode_path), 0.0)
                    }
                };
            }
        }
    }

    fn to_classic_path(&self, include_first_point: bool) -> GCodePath {
        let mut output_path = self.original_gcode_path_data.clone();

        output_path.points.clear();
        let skip = if include_first_point { 0 } else { 1 };
        output_path.points.extend(self.points.iter().skip(skip).copied());

        output_path.config.speed_derivatives.speed = self.speed * 1e-3;

        output_path
    }
}

/// `GCodeState`
struct GCodeState {
    /// um^3/s
    current_flow: f64,
    /// um^3/s^2
    flow_acceleration: f64,
    /// um^3/s^2
    flow_deceleration: f64,
    /// s
    discretized_duration: f64,
    /// s
    discretized_duration_remaining: f64,
    /// um^3/s
    target_end_flow: f64,
    /// s
    reset_flow_duration: f64,
    flow_state: FlowState,
}

impl GCodeState {
    fn process_gcode_paths<'p>(&mut self, gcode_paths: &[FlowLimitedPath<'p>]) -> Vec<FlowLimitedPath<'p>> {
        // reset the discretized_duration_remaining
        self.discretized_duration_remaining = 0.0;

        let mut forward_pass_gcode_paths = Vec::new();
        for gcode_path in gcode_paths {
            forward_pass_gcode_paths.extend(self.process_gcode_path(gcode_path, Direction::Forward));
        }

        // reset the discretized_duration_remaining
        self.discretized_duration_remaining = 0.0;

        // set the current flow to the target end flow. When executing the backward pass we want to
        // start with this flow and gradually increase it to the target flow. However, if the
        // highest flow we can achieve is lower than this target flow we want to use that flow
        // instead.
        self.current_flow = if self.target_end_flow < self.current_flow { self.target_end_flow } else { self.current_flow };

        let mut backward_pass_gcode_paths: std::collections::VecDeque<FlowLimitedPath<'p>> = std::collections::VecDeque::new();
        for gcode_path in forward_pass_gcode_paths.iter().rev() {
            for path in self.process_gcode_path(gcode_path, Direction::Backward) {
                backward_pass_gcode_paths.push_front(path);
            }
        }

        backward_pass_gcode_paths.into_iter().collect()
    }

    /// Discretizes a path into multiple paths with a gradual increase in flow.
    fn process_gcode_path<'p>(&mut self, path: &FlowLimitedPath<'p>, direction: Direction) -> Vec<FlowLimitedPath<'p>> {
        if path.is_travel() {
            if path.is_retract() || path.total_duration() > self.reset_flow_duration {
                self.flow_state = FlowState::Undefined;
            }
            return vec![path.clone()];
        }

        // After a long travel move we want to reset the flow to the target end flow
        if self.flow_state == FlowState::Undefined && direction == Direction::Forward {
            self.current_flow = path.target_flow();
        }

        let target_flow = path.flow();
        if target_flow <= self.current_flow {
            self.current_flow = target_flow;
            self.discretized_duration_remaining = 0.0;
            self.flow_state = FlowState::Stable;
            return vec![path.clone()];
        }

        let extrusion_volume_per_mm = path.extrusion_volume_per_mm(); // um^3/um

        let mut discretized_paths = Vec::new();

        let mut remaining_path = path.clone();

        if self.discretized_duration_remaining > 0.0 {
            let discretized_segment_speed = self.current_flow / extrusion_volume_per_mm; // um^3/s / um^3/um = um/s
            let (partitioned_gcode_path, new_remaining_path, remaining_partition_duration) =
                path.partition(self.discretized_duration_remaining, discretized_segment_speed, direction);
            let v = self.discretized_duration_remaining - remaining_partition_duration;
            self.discretized_duration_remaining = if 0.0 < v { v } else { 0.0 };
            match new_remaining_path {
                Some(new_remaining_path) => {
                    remaining_path = new_remaining_path;
                    discretized_paths.push(partitioned_gcode_path);
                }
                None => {
                    self.flow_state = FlowState::Transition;
                    return vec![partitioned_gcode_path];
                }
            }
        }

        // while we have not reached the target flow, iteratively discretize the path
        // such that the new path has a duration of discretized_duration and with each
        // iteration an increased flow of flow_acceleration
        while self.current_flow < target_flow {
            let flow_delta = (if direction == Direction::Forward { self.flow_acceleration } else { self.flow_deceleration }) * self.discretized_duration;
            let candidate = self.current_flow + flow_delta;
            self.current_flow = if candidate < target_flow { candidate } else { target_flow };

            let segment_speed = self.current_flow / extrusion_volume_per_mm; // um^3/s / um^3/um = um/s

            if self.current_flow == target_flow {
                remaining_path.speed = segment_speed;
                let v = self.discretized_duration_remaining - remaining_path.total_duration();
                self.discretized_duration_remaining = if v < 0.0 { 0.0 } else { v };
                self.flow_state = if self.discretized_duration_remaining > 0.0 { FlowState::Transition } else { FlowState::Stable };
                discretized_paths.push(remaining_path);
                return discretized_paths;
            }

            let (partitioned_gcode_path, new_remaining_path, remaining_partition_duration) =
                remaining_path.partition(self.discretized_duration, segment_speed, direction);

            discretized_paths.push(partitioned_gcode_path);

            match new_remaining_path {
                Some(new_remaining_path) => remaining_path = new_remaining_path,
                None => {
                    self.flow_state = FlowState::Transition;
                    self.discretized_duration_remaining = remaining_partition_duration;
                    return discretized_paths;
                }
            }
        }
        discretized_paths.push(remaining_path);

        self.flow_state = if self.discretized_duration_remaining > 0.0 { FlowState::Transition } else { FlowState::Stable };

        discretized_paths
    }
}

/// `gradual_flow::Processor::process`
pub fn process(extruder_plan_paths: &mut Vec<GCodePath>, extruder_nr: usize, layer_nr: usize) {
    let extruder_settings = &current_slice().scene.extruders[extruder_nr].settings;
    if !extruder_settings.get_bool("gradual_flow_enabled") {
        return;
    }
    let new_paths = {
        // Convert the gcode paths to a format that suits our calculations more
        let mut gcode_paths: Vec<FlowLimitedPath> = Vec::new();
        for (i, path) in extruder_plan_paths.iter().enumerate() {
            if i == 0 {
                gcode_paths.push(FlowLimitedPath::new(path, path.points.clone(), None));
            } else {
                // The paths in Cura are a connected line string: a new path begins where the previous path ends.
                let mut points = Vec::with_capacity(path.points.len() + 1);
                if let Some(&last) = gcode_paths.last().and_then(|p| p.points.last()) {
                    points.push(last);
                }
                points.extend_from_slice(&path.points);
                gcode_paths.push(FlowLimitedPath::new(path, points, None));
            }
        }
        let first_non_zero_flow = gcode_paths.iter().map(|p| p.flow()).find(|&flow| flow != 0.0);
        let flow_limit = extruder_settings.get_f64(if layer_nr == 0 { "layer_0_max_flow_acceleration" } else { "max_flow_acceleration" }) * 1e9;
        let target_flow = first_non_zero_flow.unwrap_or(0.0);
        let mut state = GCodeState {
            current_flow: target_flow,
            flow_acceleration: flow_limit,
            flow_deceleration: flow_limit,
            discretized_duration: extruder_settings.get_f64("gradual_flow_discretisation_step_size"),
            discretized_duration_remaining: 0.0,
            // take the first path's target flow as the target flow, this might
            // not be correct, but it is safe to assume the target flow for the
            // next layer is the same as the target flow of the current layer
            target_end_flow: target_flow,
            reset_flow_duration: extruder_settings.get_f64("reset_flow_duration"),
            flow_state: FlowState::Undefined,
        };
        let limited_flow_acceleration_paths = state.process_gcode_paths(&gcode_paths);
        // Copy newly generated paths to actual plan
        limited_flow_acceleration_paths
            .iter()
            .enumerate()
            .map(|(index, gcode_path)| {
                // since the first point is added from the previous path in the initial conversion,
                // we should remove it here again (except for the first path)
                gcode_path.to_classic_path(index == 0)
            })
            .collect::<Vec<_>>()
    };
    *extruder_plan_paths = new_paths;
}
