//! `pathPlanning/*` and `GCodePathConfig`: the planned paths of a layer and their configurations.

pub mod comb;
pub mod line_polygons_crossings;

use crate::geometry::point3ll::Point3LL;
use crate::print_feature::PrintFeatureType;
use crate::settings::types::{Acceleration, LayerIndex, Ratio, Velocity};
use crate::utils::coord::{int2mm, Coord};

/// `SpaceFillType`: the strategies with which an area can be occupied with filament.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, PartialOrd, Ord)]
pub enum SpaceFillType {
    #[default]
    None,
    Polygons,
    PolyLines,
    Lines,
}

/// `SpeedDerivatives`
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
pub struct SpeedDerivatives {
    /// movement speed (mm/s)
    pub speed: Velocity,
    /// acceleration of head movement (mm/s^2)
    pub acceleration: Acceleration,
    /// jerk of the head movement (around stand still) as instantaneous speed change (mm/s)
    pub jerk: Velocity,
}

#[inline]
fn min_f(a: f64, b: f64) -> f64 {
    if b < a {
        b
    } else {
        a
    }
}

impl SpeedDerivatives {
    pub fn new(speed: Velocity, acceleration: Acceleration, jerk: Velocity) -> Self {
        SpeedDerivatives { speed, acceleration, jerk }
    }

    /// Set the speed to somewhere between the speed of `first_layer_config` and the iconic speed.
    pub fn smooth_speed(&mut self, first_layer_config: &SpeedDerivatives, layer_nr: LayerIndex, max_speed_layer_nr: LayerIndex) {
        let max_speed_layer = max_speed_layer_nr as f64;
        let first_layer_speed = min_f(self.speed, first_layer_config.speed);
        let first_layer_acceleration = min_f(self.acceleration, first_layer_config.acceleration);
        let first_layer_jerk = min_f(self.jerk, first_layer_config.jerk);
        let l = layer_nr as f64;
        self.speed = (self.speed * l) / max_speed_layer + (first_layer_speed * (max_speed_layer - l) / max_speed_layer);
        self.acceleration = (self.acceleration * l) / max_speed_layer + (first_layer_acceleration * (max_speed_layer - l) / max_speed_layer);
        self.jerk = (self.jerk * l) / max_speed_layer + (first_layer_jerk * (max_speed_layer - l) / max_speed_layer);
    }
}

/// `GCodePathConfig`: the settings needed to generate the G-code of a path (line width, speed, etc.).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GCodePathConfig {
    /// vertical offset from 'full' layer height, to be applied to the whole path
    pub z_offset: Coord,
    /// name of the feature type
    pub type_: PrintFeatureType,
    /// width of the line extruded
    pub line_width: Coord,
    /// current layer height in micron
    pub layer_thickness: Coord,
    /// extrusion flow modifier.
    pub flow: Ratio,
    /// The speed settings (and acceleration and jerk) of the extruded line.
    pub speed_derivatives: SpeedDerivatives,
    /// whether current config is used when bridging
    pub is_bridge_path: bool,
    /// fan speed override for this path, value should be within range 0-100 (inclusive) and ignored otherwise
    pub fan_speed: f64,
    /// current mm^3 filament moved per mm line traversed
    pub extrusion_mm3_per_mm: f64,
}

impl Default for GCodePathConfig {
    fn default() -> Self {
        GCodePathConfig::new(PrintFeatureType::NoneType, 0, 0, 0.0, SpeedDerivatives::default())
    }
}

impl GCodePathConfig {
    pub const FAN_SPEED_DEFAULT: f64 = -1.0;

    /// Designated-initializer style constructor; `extrusion_mm3_per_mm` is computed from the other fields
    /// like the default member initializer in C++.
    pub fn new(type_: PrintFeatureType, line_width: Coord, layer_thickness: Coord, flow: Ratio, speed_derivatives: SpeedDerivatives) -> Self {
        Self::new_full(type_, line_width, layer_thickness, flow, speed_derivatives, false, Self::FAN_SPEED_DEFAULT)
    }

    pub fn new_full(
        type_: PrintFeatureType,
        line_width: Coord,
        layer_thickness: Coord,
        flow: Ratio,
        speed_derivatives: SpeedDerivatives,
        is_bridge_path: bool,
        fan_speed: f64,
    ) -> Self {
        GCodePathConfig {
            z_offset: 0,
            type_,
            line_width,
            layer_thickness,
            flow,
            speed_derivatives,
            is_bridge_path,
            fan_speed,
            extrusion_mm3_per_mm: int2mm(line_width) * int2mm(layer_thickness) * flow,
        }
    }

    pub fn get_extrusion_mm3_per_mm(&self) -> f64 {
        self.extrusion_mm3_per_mm
    }
    pub fn get_speed(&self) -> Velocity {
        self.speed_derivatives.speed
    }
    pub fn get_acceleration(&self) -> Acceleration {
        self.speed_derivatives.acceleration
    }
    pub fn get_jerk(&self) -> Velocity {
        self.speed_derivatives.jerk
    }
    pub fn get_line_width(&self) -> Coord {
        self.line_width
    }
    pub fn get_layer_thickness(&self) -> Coord {
        self.layer_thickness
    }
    pub fn get_print_feature_type(&self) -> PrintFeatureType {
        self.type_
    }
    pub fn is_travel_path(&self) -> bool {
        self.line_width == 0
    }
    pub fn is_bridge_path(&self) -> bool {
        self.is_bridge_path
    }
    pub fn get_fan_speed(&self) -> f64 {
        self.fan_speed
    }
    pub fn get_flow_ratio(&self) -> Ratio {
        self.flow
    }
}

/// `TimeMaterialEstimates`
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
pub struct TimeMaterialEstimates {
    /// Time in seconds occupied by extrusion
    pub extrude_time: f64,
    /// Time in seconds occupied by non-retracted travel (non-extrusion)
    pub unretracted_travel_time: f64,
    /// Time in seconds occupied by retracted travel (non-extrusion)
    pub retracted_travel_time: f64,
    /// Material used (in mm^3)
    pub material: f64,
    /// Time in seconds occupied by extrusion assuming paths are printed at slowest path speed, usually the outer wall speed
    pub extrude_time_at_slowest_path_speed: f64,
    /// Time in seconds occupied by extrusion assuming paths are printed at the user specified Minimum Speed
    pub extrude_time_at_minimum_speed: f64,
}

impl std::ops::AddAssign for TimeMaterialEstimates {
    fn add_assign(&mut self, other: TimeMaterialEstimates) {
        self.extrude_time += other.extrude_time;
        self.extrude_time_at_slowest_path_speed += other.extrude_time_at_slowest_path_speed;
        self.extrude_time_at_minimum_speed += other.extrude_time_at_minimum_speed;
        self.unretracted_travel_time += other.unretracted_travel_time;
        self.retracted_travel_time += other.retracted_travel_time;
        self.material += other.material;
    }
}

impl std::ops::SubAssign for TimeMaterialEstimates {
    fn sub_assign(&mut self, other: TimeMaterialEstimates) {
        self.extrude_time -= other.extrude_time;
        self.extrude_time_at_slowest_path_speed -= other.extrude_time_at_slowest_path_speed;
        self.extrude_time_at_minimum_speed -= other.extrude_time_at_minimum_speed;
        self.unretracted_travel_time -= other.unretracted_travel_time;
        self.retracted_travel_time -= other.retracted_travel_time;
        self.material -= other.material;
    }
}

impl std::ops::Add for TimeMaterialEstimates {
    type Output = TimeMaterialEstimates;
    fn add(self, other: TimeMaterialEstimates) -> TimeMaterialEstimates {
        TimeMaterialEstimates {
            extrude_time: self.extrude_time + other.extrude_time,
            unretracted_travel_time: self.unretracted_travel_time + other.unretracted_travel_time,
            retracted_travel_time: self.retracted_travel_time + other.retracted_travel_time,
            material: self.material + other.material,
            extrude_time_at_slowest_path_speed: self.extrude_time_at_slowest_path_speed + other.extrude_time_at_slowest_path_speed,
            extrude_time_at_minimum_speed: self.extrude_time_at_minimum_speed + other.extrude_time_at_minimum_speed,
        }
    }
}

impl std::ops::Sub for TimeMaterialEstimates {
    type Output = TimeMaterialEstimates;
    fn sub(self, other: TimeMaterialEstimates) -> TimeMaterialEstimates {
        TimeMaterialEstimates {
            extrude_time: self.extrude_time - other.extrude_time,
            unretracted_travel_time: self.unretracted_travel_time - other.unretracted_travel_time,
            retracted_travel_time: self.retracted_travel_time - other.retracted_travel_time,
            material: self.material - other.material,
            extrude_time_at_slowest_path_speed: self.extrude_time_at_slowest_path_speed - other.extrude_time_at_slowest_path_speed,
            extrude_time_at_minimum_speed: self.extrude_time_at_minimum_speed - other.extrude_time_at_minimum_speed,
        }
    }
}

impl TimeMaterialEstimates {
    pub fn reset(&mut self) {
        *self = TimeMaterialEstimates::default();
    }
    pub fn get_total_time(&self) -> f64 {
        self.extrude_time + self.unretracted_travel_time + self.retracted_travel_time
    }
    pub fn get_total_unretracted_time(&self) -> f64 {
        self.extrude_time + self.unretracted_travel_time
    }
    pub fn get_travel_time(&self) -> f64 {
        self.retracted_travel_time + self.unretracted_travel_time
    }
}

/// `GCodePath`: a list of points with the same config (feature type, speed, line width, etc.).
#[derive(Clone, Debug)]
pub struct GCodePath {
    /// Actual vertical offset from 'full' layer height, applied to the whole path (can be different from the one in the config)
    pub z_offset: Coord,
    /// The configuration settings of the path.
    pub config: GCodePathConfig,
    /// Which mesh this path belongs to (index into `storage.meshes`), if any.
    pub mesh: Option<usize>,
    /// The type of space filling of which this path is a part
    pub space_fill_type: SpaceFillType,
    /// A type-independent flow configuration
    pub flow: Ratio,
    /// Adjustment to the line width. Similar to flow, but causes the speed_back_pressure_factor to be adjusted.
    pub width_factor: Ratio,
    /// Whether to gradually increment the z position during the printing of this path.
    pub spiralize: bool,
    /// A speed factor that is multiplied with the travel speed.
    pub speed_factor: Ratio,
    /// The factor the (non-travel) speed should be multiplied with as a consequence of back pressure compensation.
    pub speed_back_pressure_factor: Ratio,
    /// Whether the path is a move path preceded by a retraction move; whether the path is a retracted move path.
    pub retract: bool,
    /// Also retract to prepare for a nozzle switch
    pub retract_for_nozzle_switch: bool,
    /// Whether the last move of the path should be preceded by an unretraction.
    pub unretract_before_last_travel_move: bool,
    /// Whether to perform a z_hop in this path, which is assumed to be a travel path.
    pub perform_z_hop: bool,
    /// Whether this path is preceded by a prime (blob)
    pub perform_prime: bool,
    /// The points constituting this path. The Z coordinate is an offset relative to the actual layer height.
    pub points: Vec<Point3LL>,
    /// Path is finished, no more moves should be added, and a new path should be started instead of any appending done to this one.
    pub done: bool,
    /// fan speed override for this path, value should be within range 0-100 (inclusive) and ignored otherwise
    pub fan_speed: f64,
    /// Naive time and material estimates
    pub estimates: TimeMaterialEstimates,
    /// Indicates whether we should add a travel move to the Z height of the first point before processing the path
    pub travel_to_z: bool,
}

impl Default for GCodePath {
    fn default() -> Self {
        GCodePath {
            z_offset: 0,
            config: GCodePathConfig::default(),
            mesh: None,
            space_fill_type: SpaceFillType::None,
            flow: 0.0,
            width_factor: 0.0,
            spiralize: false,
            speed_factor: 1.0,
            speed_back_pressure_factor: 1.0,
            retract: false,
            retract_for_nozzle_switch: false,
            unretract_before_last_travel_move: false,
            perform_z_hop: false,
            perform_prime: false,
            points: Vec::new(),
            done: false,
            fan_speed: GCodePathConfig::FAN_SPEED_DEFAULT,
            estimates: TimeMaterialEstimates::default(),
            travel_to_z: true,
        }
    }
}

impl GCodePath {
    /// Whether this config is the config of a travel path.
    pub fn is_travel_path(&self) -> bool {
        self.config.is_travel_path()
    }

    /// Get the material flow in mm^3 per mm traversed.
    pub fn get_extrusion_mm3_per_mm(&self) -> f64 {
        self.flow * self.width_factor * self.config.get_extrusion_mm3_per_mm()
    }

    /// Get the actual line width (modulated by the flow)
    pub fn get_line_width_for_layer_view(&self) -> Coord {
        (self.flow * self.width_factor * self.config.get_line_width() as f64 * self.config.get_flow_ratio()) as Coord
    }

    pub fn set_fan_speed(&mut self, fanspeed: f64) {
        self.fan_speed = fanspeed;
    }

    pub fn get_fan_speed(&self) -> f64 {
        if self.fan_speed >= 0.0 && self.fan_speed <= 100.0 {
            self.fan_speed
        } else {
            self.config.get_fan_speed()
        }
    }
}

/// `NozzleTempInsert`: a gcode command to insert in between paths.
#[derive(Clone, Copy, Debug, Default)]
pub struct NozzleTempInsert {
    /// The path before which to insert this command
    pub path_idx: usize,
    /// The extruder for which to set the temp
    pub extruder: usize,
    /// The temperature of the temperature command to insert
    pub temperature: f64,
    /// Whether to wait for the temperature to be reached
    pub wait: bool,
    /// The time after the start of the path, before which to insert the command
    pub time_after_path_start: f64,
}

impl NozzleTempInsert {
    /// Write the temperature command at the current position in the gcode.
    pub fn write(&self, gcode: &mut crate::gcode_export::GCodeExport) {
        gcode.write_temperature_command(self.extruder, self.temperature, self.wait, false);
    }

    /// `operator<` derived from the C++ `operator<=>` (a partial ordering).
    pub fn less(&self, other: &NozzleTempInsert) -> bool {
        if self.path_idx == other.path_idx {
            return self.time_after_path_start < other.time_after_path_start;
        }
        self.path_idx < other.path_idx
    }
}

/// `FanSpeedLayerTimeSettings`
#[derive(Clone, Copy, Debug, Default)]
pub struct FanSpeedLayerTimeSettings {
    pub cool_min_layer_time: crate::settings::types::Duration,
    pub cool_min_layer_time_overhang: crate::settings::types::Duration,
    pub cool_min_layer_time_overhang_min_segment_length: Coord,
    pub cool_min_layer_time_fan_speed_max: crate::settings::types::Duration,
    pub cool_fan_speed_0: f64,
    pub cool_fan_speed_min: f64,
    pub cool_fan_speed_max: f64,
    pub cool_min_speed: Velocity,
    pub cool_fan_full_layer: LayerIndex,
}
