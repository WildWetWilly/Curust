//! `RetractionConfig.h` and `WipeScriptConfig.h`.

use crate::settings::types::{Duration, Ratio, Velocity};
use crate::utils::coord::Coord;

/// The retraction configuration used in the G-code export.
#[derive(Clone, Copy, Debug, Default)]
pub struct RetractionConfig {
    /// The distance retracted (in mm)
    pub distance: f64,
    /// The ratio of retraction done during travel
    pub retract_during_travel: Ratio,
    /// Whether the retraction should keep going during a long travel
    pub keep_retracting_during_travel: bool,
    /// The ratio of priming done during travel
    pub prime_during_travel: Ratio,
    /// The speed with which to retract (in mm/s)
    pub speed: Velocity,
    /// The speed with which to unretract (in mm/s)
    pub prime_speed: Velocity,
    /// The amount of material primed after unretracting (in mm^3)
    pub prime_volume: f64,
    /// The amount with which to lift the head during a retraction-travel
    pub z_hop: Coord,
    pub retraction_min_travel_distance: Coord,
    /// in mm
    pub retraction_extrusion_window: f64,
    pub retraction_count_max: usize,
}

/// Settings for the wipe script.
#[derive(Clone, Copy, Debug, Default)]
pub struct WipeScriptConfig {
    pub retraction_enable: bool,
    pub retraction_config: RetractionConfig,
    pub pause: Duration,
    pub hop_enable: bool,
    pub hop_amount: Coord,
    pub hop_speed: Velocity,
    pub brush_pos_x: Coord,
    pub repeat_count: usize,
    pub move_distance: Coord,
    pub move_speed: Velocity,
    pub max_extrusion_mm3: f64,
    pub clean_between_layers: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RetractionAndWipeConfig {
    pub wipe_config: WipeScriptConfig,
    pub retraction_config: RetractionConfig,
    pub retraction_hop_after_extruder_switch: Coord,
    pub switch_extruder_extra_prime_amount: f64,
    pub extruder_switch_retraction_config: RetractionConfig,
}
