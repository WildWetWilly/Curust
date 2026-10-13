//! `timeEstimate.h/.cpp`: the `TimeEstimateCalculator` generates an estimate of printing time
//! calculated with acceleration in mind. Some of this code has been adapted from the Marlin sources.

use crate::print_feature::PrintFeatureType;
use crate::settings::settings::Settings;
use crate::settings::types::{Acceleration, Duration, Ratio, Velocity};

pub const NUM_AXIS: usize = 4;
pub const X_AXIS: usize = 0;
pub const Y_AXIS: usize = 1;
pub const Z_AXIS: usize = 2;
pub const E_AXIS: usize = 3;

const MINIMUM_PLANNER_SPEED: f64 = 0.05; // mm/sec

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Position {
    pub axis: [f64; NUM_AXIS],
}

impl Position {
    pub fn new(x: f64, y: f64, z: f64, e: f64) -> Self {
        Position { axis: [x, y, z, e] }
    }
}

impl std::ops::Index<usize> for Position {
    type Output = f64;
    fn index(&self, n: usize) -> &f64 {
        &self.axis[n]
    }
}
impl std::ops::IndexMut<usize> for Position {
    fn index_mut(&mut self, n: usize) -> &mut f64 {
        &mut self.axis[n]
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Block {
    pub recalculate_flag: bool,
    pub accelerate_until: f64,
    pub decelerate_after: f64,
    pub initial_feedrate: Velocity,
    pub final_feedrate: Velocity,
    pub entry_speed: Velocity,
    pub max_entry_speed: Velocity,
    pub nominal_length_flag: bool,
    pub nominal_feedrate: Velocity,
    pub max_travel: f64,
    pub distance: f64,
    pub acceleration: Acceleration,
    pub delta: Position,
    pub abs_delta: Position,
    pub feature: PrintFeatureType,
}

impl Default for Block {
    fn default() -> Self {
        // memset(&block, 0, sizeof(block))
        Block {
            recalculate_flag: false,
            accelerate_until: 0.0,
            decelerate_after: 0.0,
            initial_feedrate: 0.0,
            final_feedrate: 0.0,
            entry_speed: 0.0,
            max_entry_speed: 0.0,
            nominal_length_flag: false,
            nominal_feedrate: 0.0,
            max_travel: 0.0,
            distance: 0.0,
            acceleration: 0.0,
            delta: Position::default(),
            abs_delta: Position::default(),
            feature: PrintFeatureType::NoneType,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TimeEstimateCalculator {
    max_feedrate: [Velocity; NUM_AXIS],
    minimumfeedrate: Velocity,
    acceleration: Acceleration,
    max_acceleration: [Acceleration; NUM_AXIS],
    max_xy_jerk: Velocity,
    max_z_jerk: Velocity,
    max_e_jerk: Velocity,
    extra_time: Duration,

    previous_feedrate: Position,
    previous_nominal_feedrate: Velocity,

    current_position: Position,

    blocks: Vec<Block>,
}

impl Default for TimeEstimateCalculator {
    fn default() -> Self {
        TimeEstimateCalculator {
            max_feedrate: [600.0, 600.0, 40.0, 25.0], // mm/s
            minimumfeedrate: 0.01,
            acceleration: 3000.0,
            max_acceleration: [9000.0, 9000.0, 100.0, 10000.0],
            max_xy_jerk: 20.0,
            max_z_jerk: 0.4,
            max_e_jerk: 5.0,
            extra_time: Duration::new(0.0),
            previous_feedrate: Position::default(),
            previous_nominal_feedrate: 0.0,
            current_position: Position::default(),
            blocks: Vec::new(),
        }
    }
}

#[inline]
fn square(a: f64) -> f64 {
    a * a
}

/// Calculates the maximum allowable speed at this point when you must be able to reach target_velocity using the
/// acceleration within the allotted distance.
#[inline]
fn max_allowable_speed(acceleration: Acceleration, target_velocity: Velocity, distance: f64) -> Velocity {
    (target_velocity * target_velocity - 2.0 * acceleration * distance).sqrt()
}

/// Calculates the distance (not time) it takes to accelerate from initial_rate to target_rate using the given acceleration:
#[inline]
fn estimate_acceleration_distance(initial_rate: Velocity, target_rate: Velocity, acceleration: Acceleration) -> f64 {
    if acceleration == 0.0 {
        return 0.0;
    }
    (square(target_rate) - square(initial_rate)) / (2.0 * acceleration)
}

/// This function gives you the point at which you must start braking (at the rate of -acceleration) if
/// you started at speed initial_rate and accelerated until this point and want to end at the final_rate after
/// a total travel of distance.
#[inline]
fn intersection_distance(initial_rate: Velocity, final_rate: Velocity, acceleration: Acceleration, distance: f64) -> f64 {
    if acceleration == 0.0 {
        return 0.0;
    }
    (2.0 * acceleration * distance - square(initial_rate) + square(final_rate)) / (4.0 * acceleration)
}

/// This function gives the time it needs to accelerate from an initial speed to reach a final distance.
#[inline]
fn acceleration_time_from_distance(initial_feedrate: Velocity, distance: Velocity, acceleration: Acceleration) -> f64 {
    let mut discriminant = square(initial_feedrate) - 2.0 * acceleration * -distance;
    // If discriminant is negative, we're moving in the wrong direction.
    // Making the discriminant 0 then gives the extremum of the parabola instead of the intersection.
    discriminant = if 0.0 < discriminant { discriminant } else { 0.0 };
    (-initial_feedrate + discriminant.sqrt()) / acceleration
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

impl TimeEstimateCalculator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the movement configuration of the firmware.
    pub fn set_firmware_defaults(&mut self, settings: &Settings) {
        self.max_feedrate[X_AXIS] = settings.get_velocity("machine_max_feedrate_x");
        self.max_feedrate[Y_AXIS] = settings.get_velocity("machine_max_feedrate_y");
        self.max_feedrate[Z_AXIS] = settings.get_velocity("machine_max_feedrate_z");
        self.max_feedrate[E_AXIS] = settings.get_velocity("machine_max_feedrate_e");
        self.max_acceleration[X_AXIS] = settings.get_acceleration("machine_max_acceleration_x");
        self.max_acceleration[Y_AXIS] = settings.get_acceleration("machine_max_acceleration_y");
        self.max_acceleration[Z_AXIS] = settings.get_acceleration("machine_max_acceleration_z");
        self.max_acceleration[E_AXIS] = settings.get_acceleration("machine_max_acceleration_e");
        self.max_xy_jerk = settings.get_velocity("machine_max_jerk_xy");
        self.max_z_jerk = settings.get_velocity("machine_max_jerk_z");
        self.max_e_jerk = settings.get_velocity("machine_max_jerk_e");
        self.minimumfeedrate = settings.get_velocity("machine_minimum_feedrate");
        self.acceleration = settings.get_acceleration("machine_acceleration");
    }

    pub fn set_position(&mut self, new_pos: Position) {
        self.current_position = new_pos;
    }

    pub fn add_time(&mut self, time: Duration) {
        self.extra_time += time;
    }

    /// Set the default acceleration to `acc`
    pub fn set_acceleration(&mut self, acc: Acceleration) {
        self.acceleration = acc;
    }

    /// Set the max xy jerk to `jerk`
    pub fn set_max_xy_jerk(&mut self, jerk: Velocity) {
        self.max_xy_jerk = jerk;
    }

    pub fn reset(&mut self) {
        self.extra_time = Duration::new(0.0);
        self.blocks.clear();
    }

    fn calculate_trapezoid_for_block(block: &mut Block, entry_factor: Ratio, exit_factor: Ratio) {
        let initial_feedrate = block.nominal_feedrate * entry_factor;
        let final_feedrate = block.nominal_feedrate * exit_factor;

        let mut accelerate_distance = estimate_acceleration_distance(initial_feedrate, block.nominal_feedrate, block.acceleration);
        let decelerate_distance = estimate_acceleration_distance(block.nominal_feedrate, final_feedrate, -block.acceleration);

        // Calculate the size of Plateau of Nominal Rate.
        let mut plateau_distance = block.distance - accelerate_distance - decelerate_distance;

        // Is the Plateau of Nominal Rate smaller than nothing? That means no cruising, and we will
        // have to use intersection_distance() to calculate when to abort acceleration and start braking
        // in order to reach the final_rate exactly at the end of this block.
        if plateau_distance < 0.0 {
            accelerate_distance = intersection_distance(initial_feedrate, final_feedrate, block.acceleration, block.distance);
            accelerate_distance = max_f(accelerate_distance, 0.0); // Check limits due to numerical round-off
            accelerate_distance = min_f(accelerate_distance, block.distance);
            plateau_distance = 0.0;
        }

        block.accelerate_until = accelerate_distance;
        block.decelerate_after = accelerate_distance + plateau_distance;
        block.initial_feedrate = initial_feedrate;
        block.final_feedrate = final_feedrate;
    }

    pub fn plan(&mut self, new_pos: Position, mut feedrate: Velocity, feature: PrintFeatureType) {
        let mut block = Block { feature, ..Default::default() };

        for n in 0..NUM_AXIS {
            block.delta[n] = new_pos[n] - self.current_position[n];
            block.abs_delta[n] = block.delta[n].abs();
            block.max_travel = max_f(block.max_travel, block.abs_delta[n]);
        }
        if block.max_travel <= 0.0 {
            return;
        }
        if feedrate < self.minimumfeedrate {
            feedrate = self.minimumfeedrate;
        }
        // NOTE: sqrtf (single precision) in the C++ code.
        block.distance = ((square(block.abs_delta[0]) + square(block.abs_delta[1]) + square(block.abs_delta[2])) as f32).sqrt() as f64;
        if block.distance == 0.0 {
            block.distance = block.abs_delta[3];
        }
        block.nominal_feedrate = feedrate;

        let mut current_feedrate = Position::default();
        let mut current_abs_feedrate = Position::default();
        let mut feedrate_factor: Ratio = 1.0;
        for n in 0..NUM_AXIS {
            current_feedrate[n] = (block.delta[n] * feedrate) / block.distance;
            current_abs_feedrate[n] = current_feedrate[n].abs();
            if current_abs_feedrate[n] > self.max_feedrate[n] {
                feedrate_factor = min_f(feedrate_factor, self.max_feedrate[n] / current_abs_feedrate[n]);
            }
        }
        // TODO: XY_FREQUENCY_LIMIT

        if feedrate_factor < 1.0 {
            for n in 0..NUM_AXIS {
                current_feedrate[n] *= feedrate_factor;
                current_abs_feedrate[n] *= feedrate_factor;
            }
            block.nominal_feedrate *= feedrate_factor;
        }

        block.acceleration = self.acceleration;
        for n in 0..NUM_AXIS {
            if block.acceleration * (block.abs_delta[n] / block.distance) > self.max_acceleration[n] {
                block.acceleration = self.max_acceleration[n];
            }
        }

        let mut vmax_junction: Velocity = self.max_xy_jerk / 2.0;
        let mut vmax_junction_factor: Ratio = 1.0;
        if current_abs_feedrate[Z_AXIS] > self.max_z_jerk / 2.0 {
            vmax_junction = min_f(vmax_junction, self.max_z_jerk / 2.0);
        }
        if current_abs_feedrate[E_AXIS] > self.max_e_jerk / 2.0 {
            vmax_junction = min_f(vmax_junction, self.max_e_jerk / 2.0);
        }
        vmax_junction = min_f(vmax_junction, block.nominal_feedrate);
        let safe_speed = vmax_junction;

        if !self.blocks.is_empty() && self.previous_nominal_feedrate > 0.0001 {
            let xy_jerk = (square(current_feedrate[X_AXIS] - self.previous_feedrate[X_AXIS]) + square(current_feedrate[Y_AXIS] - self.previous_feedrate[Y_AXIS])).sqrt();
            vmax_junction = block.nominal_feedrate;
            if xy_jerk > self.max_xy_jerk {
                vmax_junction_factor = self.max_xy_jerk / xy_jerk;
            }
            let z_jerk = (current_feedrate[Z_AXIS] - self.previous_feedrate[Z_AXIS]).abs();
            if z_jerk > self.max_z_jerk {
                vmax_junction_factor = min_f(vmax_junction_factor, self.max_z_jerk / z_jerk);
            }
            let e_jerk = (current_feedrate[E_AXIS] - self.previous_feedrate[E_AXIS]).abs();
            if e_jerk > self.max_e_jerk {
                vmax_junction_factor = min_f(vmax_junction_factor, self.max_e_jerk / e_jerk);
            }
            vmax_junction = min_f(self.previous_nominal_feedrate, vmax_junction * vmax_junction_factor); // Limit speed to max previous speed
        }

        block.max_entry_speed = vmax_junction;

        let v_allowable = max_allowable_speed(-block.acceleration, MINIMUM_PLANNER_SPEED, block.distance);
        block.entry_speed = min_f(vmax_junction, v_allowable);
        block.nominal_length_flag = block.nominal_feedrate <= v_allowable;
        block.recalculate_flag = true; // Always calculate trapezoid for new block

        self.previous_feedrate = current_feedrate;
        self.previous_nominal_feedrate = block.nominal_feedrate;

        self.current_position = new_pos;

        let entry_factor = block.entry_speed / block.nominal_feedrate;
        let exit_factor = safe_speed / block.nominal_feedrate;
        Self::calculate_trapezoid_for_block(&mut block, entry_factor, exit_factor);

        self.blocks.push(block);
    }

    pub fn calculate(&mut self) -> Vec<Duration> {
        self.reverse_pass();
        self.forward_pass();
        self.recalculate_trapezoids();

        let mut totals = vec![Duration::new(0.0); PrintFeatureType::NumPrintFeatureTypes as usize];
        totals[PrintFeatureType::NoneType as usize] = self.extra_time; // Extra time (pause for minimum layer time, etc) is marked as NoneType
        for block in &self.blocks {
            let plateau_distance = block.decelerate_after - block.accelerate_until;
            let f = block.feature as usize;

            totals[f] += Duration::new(acceleration_time_from_distance(block.initial_feedrate, block.accelerate_until, block.acceleration));
            totals[f] += Duration::new(plateau_distance / block.nominal_feedrate);
            totals[f] += Duration::new(acceleration_time_from_distance(block.final_feedrate, block.distance - block.decelerate_after, block.acceleration));
        }
        totals
    }

    fn planner_reverse_pass_kernel(current: &mut Block, next: &Block) {
        // If entry speed is already at the maximum entry speed, no need to recheck. Block is cruising.
        // If not, block in state of acceleration or deceleration. Reset entry speed to maximum and
        // check for maximum allowable speed reductions to ensure maximum possible planned speed.
        if current.entry_speed != current.max_entry_speed {
            // If nominal length true, max junction speed is guaranteed to be reached. Only compute
            // for max allowable speed if block is decelerating and nominal length is false.
            if !current.nominal_length_flag && current.max_entry_speed > next.entry_speed {
                current.entry_speed = min_f(current.max_entry_speed, max_allowable_speed(-current.acceleration, next.entry_speed, current.distance));
            } else {
                current.entry_speed = current.max_entry_speed;
            }
            current.recalculate_flag = true;
        }
    }

    fn reverse_pass(&mut self) {
        // block[0] = blocks[n], block[1] = blocks[n+1], block[2] = blocks[n+2]; kernel(previous=block[0], current=block[1], next=block[2])
        let n_blocks = self.blocks.len();
        if n_blocks < 3 {
            // The kernel needs both a current and a next block, i.e. n + 2 < size.
            if n_blocks < 2 {
                return;
            }
        }
        // Iterations happen for n from size-1 down to 0; the kernel acts on (blocks[n+1], blocks[n+2]) when both exist.
        let mut n = n_blocks;
        while n > 0 {
            n -= 1;
            if n + 2 < n_blocks {
                let (left, right) = self.blocks.split_at_mut(n + 2);
                Self::planner_reverse_pass_kernel(&mut left[n + 1], &right[0]);
            }
        }
    }

    fn planner_forward_pass_kernel(previous: &Block, current: &mut Block) {
        // If the previous block is an acceleration block, but it is not long enough to complete the
        // full speed change within the block, we need to adjust the entry speed accordingly. Entry
        // speeds have already been reset, maximized, and reverse planned by reverse planner.
        // If nominal length is true, max junction speed is guaranteed to be reached. No need to recheck.
        if !previous.nominal_length_flag && previous.entry_speed < current.entry_speed {
            let entry_speed = min_f(current.entry_speed, max_allowable_speed(-previous.acceleration, previous.entry_speed, previous.distance));

            // Check for junction speed change
            if current.entry_speed != entry_speed {
                current.entry_speed = entry_speed;
                current.recalculate_flag = true;
            }
        }
    }

    fn forward_pass(&mut self) {
        // for n: block = [blocks[n-2], blocks[n-1], blocks[n]]; kernel(previous=blocks[n-2], current=blocks[n-1])
        // and finally kernel(blocks[size-2], blocks[size-1]).
        let n_blocks = self.blocks.len();
        for n in 0..n_blocks {
            if n >= 2 {
                let (left, right) = self.blocks.split_at_mut(n - 1);
                Self::planner_forward_pass_kernel(&left[n - 2], &mut right[0]);
            }
        }
        if n_blocks >= 2 {
            let (left, right) = self.blocks.split_at_mut(n_blocks - 1);
            Self::planner_forward_pass_kernel(&left[n_blocks - 2], &mut right[0]);
        }
    }

    fn recalculate_trapezoids(&mut self) {
        let n_blocks = self.blocks.len();
        for n in 1..n_blocks {
            // current = blocks[n-1], next = blocks[n]
            let next_recalc = self.blocks[n].recalculate_flag;
            let next_entry = self.blocks[n].entry_speed;
            let current = &mut self.blocks[n - 1];
            // Recalculate if current block entry or exit junction speed has changed.
            if current.recalculate_flag || next_recalc {
                // NOTE: Entry and exit factors always > 0 by all previous logic operations.
                let entry_factor = current.entry_speed / current.nominal_feedrate;
                let exit_factor = next_entry / current.nominal_feedrate;
                Self::calculate_trapezoid_for_block(current, entry_factor, exit_factor);
                current.recalculate_flag = false; // Reset current only to ensure next trapezoid is computed
            }
        }
        // Last/newest block in buffer. Exit speed is set with MINIMUM_PLANNER_SPEED. Always recalculated.
        if n_blocks > 0 {
            let next = &mut self.blocks[n_blocks - 1];
            let entry_factor = next.entry_speed / next.nominal_feedrate;
            let exit_factor = MINIMUM_PLANNER_SPEED / next.nominal_feedrate;
            Self::calculate_trapezoid_for_block(next, entry_factor, exit_factor);
            next.recalculate_flag = false;
        }
    }
}
