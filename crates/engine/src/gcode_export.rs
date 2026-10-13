//! `gcodeExport.h/.cpp`: the `GCodeExport` class writes the actual G-code. This is the only class
//! that knows how G-code looks and feels. Any customizations on G-code flavors are done in this class.

use crate::application::{communication, current_slice};
use crate::geometry::point3ll::Point3LL;
use crate::geometry::Point2LL;
use crate::print_feature::PrintFeatureType;
use crate::retraction_config::{RetractionConfig, WipeScriptConfig};
use crate::settings::enums::{CoolDuringExtruderSwitch, EGCodeFlavor};
use crate::settings::settings::MAX_EXTRUDERS;
use crate::settings::types::{Acceleration, Duration, LayerIndex, Ratio, Temperature, Velocity};
use crate::slice_data_storage::SliceDataStorage;
use crate::time_estimate::{Position, TimeEstimateCalculator};
use crate::utils::aabb::AABB3D;
use crate::utils::coord::{int2mm, mm2int, Coord};
use crate::utils::string::{fmt_fixed, fmt_g, MMtoStream, PrecisionedDouble};
use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write as _;
use std::io::Write;

/// `ZHopAntiOozing` (TravelAntiOozing.h)
#[derive(Clone, Copy, Debug, Default)]
pub struct ZHopAntiOozing {
    /// The absolute amount of retracted material (in mm) to be reached while the nozzle is moved up/down
    pub amount: f64,
    /// The ratio of the full z-hop move that contains a retract/prime
    pub ratio: Ratio,
}

/// `TravelAntiOozing` (TravelAntiOozing.h)
#[derive(Clone, Debug, Default)]
pub struct TravelAntiOozing {
    /// The absolute amount of retracted material (in mm) to be reached while the nozzle is still
    pub amount_while_still: f64,
    /// The amount and ratio of retracted material to be processed during z-hop move
    pub z_hop: ZHopAntiOozing,
    /// The absolute amount of retracted material (in mm) to be reached while the nozzle is traveling
    pub amount_while_travel: f64,
    /// The intermediate position on the last/first segment that contains a retract/prime, where it should actually stop/start
    pub segment_split_position: Point2LL,
    /// For each intermediate segment containing a retraction/prime, this is the absolute amount to be reached at the end of the segment
    pub amount_by_segment: Vec<f64>,
}

#[derive(Clone, Debug, Default)]
struct ExtruderTrainAttributes {
    /// Whether this extruder has currently already been primed in this print
    is_primed: bool,
    /// Whether this extruder train is actually used during the printing of all meshgroups
    is_used: bool,
    extruder_character: char,
    /// in mm^2 for non-volumetric, cylindrical filament
    filament_area: f64,
    /// total filament used per extruder in mm^3
    total_filament: f64,
    current_temperature: Temperature,
    /// Whether the most recent temperature command has been a heat-and-wait command (M109) or not (M104).
    waited_for_temperature: bool,
    /// Temperature this nozzle needs to be at the start of the print.
    initial_temp: Temperature,
    /// The current retracted amount (in mm or mm^3), or zero(i.e. false) if it is not currently retracted
    retraction_e_amount_current: f64,
    /// The retraction_e_amount_current value at E0
    retraction_e_amount_at_e_start: f64,
    /// Amount of material (in mm^3) to be primed after an unretration (due to oozing and/or coasting)
    prime_volume: f64,
    /// The last prime speed (in mm/s) of the to-be-primed amount
    last_retraction_prime_speed: Velocity,
    /// The current material amount extruded since last wipe
    last_e_value_after_wipe: f64,
    /// nozzle print cooling fan number
    fan_number: usize,
    /// Cache of setting machine_nozzle_offset_[xy]
    nozzle_offset: Point2LL,
    /// Cache of setting machine_firmware_retract
    machine_firmware_retract: bool,
    /// in mm^3
    extruded_volume_at_previous_n_retractions: VecDeque<f64>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RetractionAmounts {
    /// The previous absolute retraction amount
    pub old_e: f64,
    /// The new absolute retraction amount
    pub new_e: f64,
    /// The difference between the new and previous amount, which is to be processed. Positive means retraction.
    pub diff_e: f64,
}

impl RetractionAmounts {
    pub fn has_retraction(&self) -> bool {
        const THRESHOLD: f64 = 1e-6;
        self.diff_e.abs() >= THRESHOLD
    }
}

/// `transliterate`: for now, just replace all non-ascii characters with '?'.
pub fn transliterate(text: &str) -> String {
    text.bytes().map(|c| if c < 0x80 { c as char } else { '?' }).collect()
}

/// The build date written into Griffin headers (`Date::getDate()`, the compile date in C++).
fn build_date_dashed() -> &'static str {
    option_env!("CURUST_BUILD_DATE").unwrap_or("2026-03-05")
}

pub struct GCodeExport {
    extruder_attr: Vec<ExtruderTrainAttributes>,
    use_extruder_offset_to_offset_coords: bool,
    machine_name: String,
    /// The UUID of the current slice.
    slice_uuid: String,
    /// Buffered output; flushed to `sink`.
    out: String,
    sink: Box<dyn Write>,
    new_line: &'static str,

    /// The last E value written to gcode (in mm or mm^3)
    current_e_value: f64,

    // flow-rate compensation
    /// Offset to compensate for flow rate (mm or mm^3)
    current_e_offset: f64,
    /// 0 to turn it off, normally 4
    max_extrusion_offset: f64,
    /// default 1
    extrusion_offset_factor: f64,

    /// The last build plate coordinates written to gcode
    current_position: Point3LL,
    /// The current speed (F values / 60) in mm/s
    current_speed: Velocity,
    /// The current acceleration (in mm/s^2) used for print moves
    current_print_acceleration: Acceleration,
    /// The current acceleration (in mm/s^2) used for travel moves for those gcode flavors that have separate print and travel accelerations
    current_travel_acceleration: Acceleration,
    /// The current jerk in the XY direction (in mm/s^3)
    current_jerk: Velocity,

    /// The bounding box of all g-code.
    total_bounding_box: AABB3D,

    /// The z position to be used on the next xy move, if the head wasn't in the correct z position yet.
    current_layer_z: Coord,
    /// The amount by which the print head is currently z hopped, or zero if it is not z hopped.
    is_z_hopped: Coord,
    /// The leftover priming from a previous travel move that should be processed while z-hopping down
    z_hop_prime_leftover: Option<ZHopAntiOozing>,

    current_extruder: usize,
    /// Current fan speed, by fan index. No value means the speed has never been set yet.
    current_fans_speeds: BTreeMap<usize, f64>,
    fans_count: usize,
    flavor: EGCodeFlavor,

    /// The total estimated print time in seconds for each feature
    total_print_times: Vec<Duration>,
    estimate_calculator: TimeEstimateCalculator,

    /// for sending travel data
    layer_nr: LayerIndex,

    is_volumetric: bool,
    /// whether to use relative extrusion distances rather than absolute
    relative_extrusion: bool,
    /// whether to write the active tool after sending commands to inactive tool
    always_write_active_tool: bool,

    /// bed temperature at the beginning of the print.
    initial_bed_temp: Temperature,
    /// Current build plate temperature.
    bed_temperature: Temperature,
    /// build volume temperature
    build_volume_temperature: Temperature,
    /// does the machine have the ability to control/stabilize build-volume-temperature
    machine_heated_build_volume: bool,
    /// if the print process reporting is enabled
    ppr_enable: bool,
}

const OUTPUT_FLUSH_SIZE: usize = 1 << 20;

impl Default for GCodeExport {
    fn default() -> Self {
        Self::new()
    }
}

impl GCodeExport {
    pub fn new() -> Self {
        let mut ret = GCodeExport {
            extruder_attr: vec![ExtruderTrainAttributes::default(); MAX_EXTRUDERS],
            use_extruder_offset_to_offset_coords: false,
            machine_name: String::new(),
            slice_uuid: String::new(),
            out: String::new(),
            sink: Box::new(std::io::stdout()),
            new_line: "\n",
            current_e_value: 0.0,
            current_e_offset: 0.0,
            max_extrusion_offset: 0.0,
            extrusion_offset_factor: 0.0,
            current_position: Point3LL::new(0, 0, mm2int(20.0)),
            current_speed: 1.0,
            current_print_acceleration: -1.0,
            current_travel_acceleration: -1.0,
            current_jerk: -1.0,
            total_bounding_box: AABB3D::default(),
            current_layer_z: 0,
            is_z_hopped: 0,
            z_hop_prime_leftover: None,
            current_extruder: 0,
            current_fans_speeds: BTreeMap::new(),
            fans_count: 0,
            flavor: EGCodeFlavor::MARLIN,
            total_print_times: vec![Duration::new(0.0); PrintFeatureType::NumPrintFeatureTypes as usize],
            estimate_calculator: TimeEstimateCalculator::new(),
            layer_nr: 0,
            is_volumetric: false,
            relative_extrusion: false,
            always_write_active_tool: false,
            initial_bed_temp: 0.0,
            bed_temperature: 0.0,
            build_volume_temperature: 0.0,
            machine_heated_build_volume: false,
            ppr_enable: false,
        };
        ret.set_flavor(EGCodeFlavor::MARLIN);
        ret
    }

    // ---------------------------------------------------------------------------------------------
    // output helpers

    #[inline]
    fn w(&mut self, s: &str) {
        self.out.push_str(s);
    }

    #[inline]
    fn nl(&mut self) {
        self.out.push_str(self.new_line);
        if self.out.len() > OUTPUT_FLUSH_SIZE {
            self.flush_buffer();
        }
    }

    fn flush_buffer(&mut self) {
        if !self.out.is_empty() {
            let _ = self.sink.write_all(self.out.as_bytes());
            self.out.clear();
        }
    }

    /// `setOutputStream`
    pub fn set_output_stream(&mut self, stream: Box<dyn Write>) {
        self.flush_buffer();
        self.sink = stream;
    }

    /// `flushOutputStream`
    pub fn flush_output_stream(&mut self) {
        self.flush_buffer();
        let _ = self.sink.flush();
    }

    // ---------------------------------------------------------------------------------------------

    fn e_to_mm(&self, e: f64) -> f64 {
        if self.is_volumetric {
            e / self.extruder_attr[self.current_extruder].filament_area
        } else {
            e
        }
    }

    pub fn mm3_to_e(&self, mm3: f64) -> f64 {
        if self.is_volumetric {
            mm3
        } else {
            mm3 / self.extruder_attr[self.current_extruder].filament_area
        }
    }

    fn mm_to_e(&self, mm: f64) -> f64 {
        if self.is_volumetric {
            mm * self.extruder_attr[self.current_extruder].filament_area
        } else {
            mm
        }
    }

    fn e_to_mm3(&self, e: f64, extruder: usize) -> f64 {
        if self.is_volumetric {
            e
        } else {
            e * self.extruder_attr[extruder].filament_area
        }
    }

    pub fn pre_setup(&mut self, start_extruder: usize) {
        self.current_extruder = start_extruder;

        let scene = &current_slice().scene;
        let mesh_group = scene.current_mesh_group();
        self.set_flavor(mesh_group.settings.get_enum::<EGCodeFlavor>("machine_gcode_flavor"));
        self.use_extruder_offset_to_offset_coords = mesh_group.settings.get_bool("machine_use_extruder_offset_to_offset_coords");
        let extruder_count = scene.extruders.len();
        self.ppr_enable = mesh_group.settings.get_bool("ppr_enable");

        for extruder_nr in 0..extruder_count {
            let train = &scene.extruders[extruder_nr];
            self.set_filament_diameter(extruder_nr, train.settings.get_coord("material_diameter"));

            self.extruder_attr[extruder_nr].last_retraction_prime_speed = train.settings.get_velocity("retraction_prime_speed"); // the alternative would be switch_extruder_prime_speed, but dual extrusion might not even be configured...
            self.extruder_attr[extruder_nr].fan_number = train.settings.get_usize("machine_extruder_cooling_fan_number");
            self.fans_count = self.fans_count.max(self.extruder_attr[extruder_nr].fan_number + 1);

            // Cache some settings that we use frequently.
            let extruder_settings = &scene.extruders[extruder_nr].settings;
            if self.use_extruder_offset_to_offset_coords {
                self.extruder_attr[extruder_nr].nozzle_offset =
                    Point2LL::new(extruder_settings.get_coord("machine_nozzle_offset_x"), extruder_settings.get_coord("machine_nozzle_offset_y"));
            } else {
                self.extruder_attr[extruder_nr].nozzle_offset = Point2LL::new(0, 0);
            }
            self.extruder_attr[extruder_nr].machine_firmware_retract = extruder_settings.get_bool("machine_firmware_retract");
        }

        self.machine_name = mesh_group.settings.get_string("machine_name");

        self.relative_extrusion = mesh_group.settings.get_bool("relative_extrusion");
        self.always_write_active_tool = mesh_group.settings.get_bool("machine_always_write_active_tool");

        if self.flavor == EGCodeFlavor::BFB {
            self.new_line = "\r\n";
        } else {
            self.new_line = "\n";
        }

        self.estimate_calculator.set_firmware_defaults(&mesh_group.settings);

        if scene.current_mesh_group == 0 {
            if !mesh_group.settings.get_bool("material_bed_temp_prepend") {
                // Current bed temperature is the one of the first layer (has already been set in header)
                self.bed_temperature = mesh_group.settings.get_temperature("material_bed_temperature_layer_0");
            } else {
                // Bed temperature has not been set yet
            }
        } else {
            // Current bed temperature is the one of the previous group
            self.bed_temperature = scene.mesh_groups[scene.current_mesh_group - 1].settings.get_temperature("material_bed_temperature");
        }
    }

    pub fn set_initial_and_build_volume_temps(&mut self, start_extruder_nr: usize) {
        let scene = &current_slice().scene;
        let extruder_count = scene.extruders.len();
        for extruder_nr in 0..extruder_count {
            let train = &scene.extruders[extruder_nr];

            let print_temp_0 = train.settings.get_temperature("material_print_temperature_layer_0");
            let print_temp_here = if print_temp_0 != 0.0 { print_temp_0 } else { train.settings.get_temperature("material_print_temperature") };
            let temp = if extruder_nr == start_extruder_nr { print_temp_here } else { train.settings.get_temperature("material_standby_temperature") };
            self.set_initial_temp(extruder_nr, temp);
        }

        self.initial_bed_temp = scene.current_mesh_group().settings.get_temperature("material_bed_temperature_layer_0");
        self.machine_heated_build_volume = scene.current_mesh_group().settings.get_bool("machine_heated_build_volume");
        self.build_volume_temperature =
            if self.machine_heated_build_volume { scene.current_mesh_group().settings.get_temperature("build_volume_temperature") } else { 0.0 };
    }

    pub fn set_initial_temp(&mut self, extruder_nr: usize, temp: f64) {
        self.extruder_attr[extruder_nr].initial_temp = temp;
        if self.flavor == EGCodeFlavor::GRIFFIN || self.flavor == EGCodeFlavor::CHEETAH || self.flavor == EGCodeFlavor::ULTIGCODE {
            self.extruder_attr[extruder_nr].current_temperature = temp;
        }
    }

    pub fn flavor_to_string(flavor: EGCodeFlavor) -> &'static str {
        match flavor {
            EGCodeFlavor::BFB => "BFB",
            EGCodeFlavor::MACH3 => "Mach3",
            EGCodeFlavor::MAKERBOT => "Makerbot",
            EGCodeFlavor::ULTIGCODE => "UltiGCode",
            EGCodeFlavor::MARLIN_VOLUMATRIC => "Marlin(Volumetric)",
            EGCodeFlavor::GRIFFIN => "Griffin",
            EGCodeFlavor::CHEETAH => "Cheetah",
            EGCodeFlavor::REPETIER => "Repetier",
            EGCodeFlavor::REPRAP => "RepRap",
            _ => "Marlin",
        }
    }

    /// Get the gcode file header (e.g. ";FLAVOR:UltiGCode\n")
    pub fn get_file_header(&mut self, extruder_is_used: &[bool], print_time: Option<Duration>, filament_used: &[f64], mat_ids: &[String]) -> String {
        let mut prefix = String::new();
        let nl = self.new_line;

        let scene = &current_slice().scene;
        let extruder_count = scene.extruders.len();
        match self.flavor {
            EGCodeFlavor::GRIFFIN | EGCodeFlavor::CHEETAH => {
                let _ = write!(prefix, ";START_OF_HEADER{}", nl);
                let _ = write!(prefix, ";HEADER_VERSION:0.1{}", nl);
                let _ = write!(prefix, ";FLAVOR:{}{}", Self::flavor_to_string(self.flavor), nl);
                let _ = write!(prefix, ";GENERATOR.NAME:Cura_SteamEngine{}", nl);
                let _ = write!(prefix, ";GENERATOR.VERSION:{}{}", crate::settings::settings::CURA_ENGINE_VERSION, nl);
                let _ = write!(prefix, ";GENERATOR.BUILD_DATE:{}{}", build_date_dashed(), nl);
                let _ = write!(prefix, ";TARGET_MACHINE.NAME:{}{}", transliterate(&self.machine_name), nl);

                for extr_nr in 0..extruder_count {
                    if !extruder_is_used[extr_nr] {
                        continue;
                    }
                    let _ = write!(prefix, ";EXTRUDER_TRAIN.{}.INITIAL_TEMPERATURE:{}{}", extr_nr, fmt_g(self.extruder_attr[extr_nr].initial_temp), nl);
                    if filament_used.len() == extruder_count {
                        let _ = write!(prefix, ";EXTRUDER_TRAIN.{}.MATERIAL.VOLUME_USED:{}{}", extr_nr, filament_used[extr_nr] as i32, nl);
                    }
                    if mat_ids.len() == extruder_count && !mat_ids[extr_nr].is_empty() {
                        let _ = write!(prefix, ";EXTRUDER_TRAIN.{}.MATERIAL.GUID:{}{}", extr_nr, mat_ids[extr_nr], nl);
                    }
                    let extruder_settings = &scene.extruders[extr_nr].settings;
                    let _ = write!(prefix, ";EXTRUDER_TRAIN.{}.NOZZLE.DIAMETER:{}{}", extr_nr, fmt_g(extruder_settings.get_f64("machine_nozzle_size")), nl);
                    let _ = write!(prefix, ";EXTRUDER_TRAIN.{}.NOZZLE.NAME:{}{}", extr_nr, extruder_settings.get_string("machine_nozzle_id"), nl);
                }
                let _ = write!(prefix, ";BUILD_PLATE.INITIAL_TEMPERATURE:{}{}", fmt_g(self.initial_bed_temp), nl);

                if self.machine_heated_build_volume {
                    let _ = write!(prefix, ";BUILD_VOLUME.TEMPERATURE:{}{}", fmt_g(self.build_volume_temperature), nl);
                }

                if let Some(print_time) = print_time {
                    let _ = write!(prefix, ";PRINT.TIME:{}{}", print_time.value() as i32, nl);
                }

                let _ = write!(prefix, ";PRINT.GROUPS:{}{}", scene.mesh_groups.len(), nl);

                if self.total_bounding_box.min.x > self.total_bounding_box.max.x {
                    // We haven't encountered any movement (yet). This probably means we're command-line slicing.
                    // Put some small default in there.
                    self.total_bounding_box.min = Point3LL::new(0, 0, 0);
                    self.total_bounding_box.max = Point3LL::new(10, 10, 10);
                }
                let bb = self.total_bounding_box;
                let _ = write!(prefix, ";PRINT.SIZE.MIN.X:{}{}", fmt_g(int2mm(bb.min.x)), nl);
                let _ = write!(prefix, ";PRINT.SIZE.MIN.Y:{}{}", fmt_g(int2mm(bb.min.y)), nl);
                let _ = write!(prefix, ";PRINT.SIZE.MIN.Z:{}{}", fmt_g(int2mm(bb.min.z)), nl);
                let _ = write!(prefix, ";PRINT.SIZE.MAX.X:{}{}", fmt_g(int2mm(bb.max.x)), nl);
                let _ = write!(prefix, ";PRINT.SIZE.MAX.Y:{}{}", fmt_g(int2mm(bb.max.y)), nl);
                let _ = write!(prefix, ";PRINT.SIZE.MAX.Z:{}{}", fmt_g(int2mm(bb.max.z)), nl);
                let _ = write!(prefix, ";SLICE_UUID:{}{}", self.slice_uuid, nl);

                if self.ppr_enable {
                    let _ = write!(prefix, ";PPR_ENABLE:TRUE{}", nl);

                    for extr_nr in 0..extruder_count {
                        if !extruder_is_used[extr_nr] {
                            continue;
                        }
                        let extruder_settings = &scene.extruders[extr_nr].settings;
                        let _ = write!(prefix, ";EXTRUDER_TRAIN.{}.PPR_FLOW_WARNING:{}{}", extr_nr, fmt_g(extruder_settings.get_f64("flow_warn_limit")), nl);
                        let _ = write!(prefix, ";EXTRUDER_TRAIN.{}.PPR_FLOW_LIMIT:{}{}", extr_nr, fmt_g(extruder_settings.get_f64("flow_anomaly_limit")), nl);
                        let _ = write!(
                            prefix,
                            ";EXTRUDER_TRAIN.{}.PPR_PRINTING_TEMPERATURE_WARNING:{}{}",
                            extr_nr,
                            fmt_g(extruder_settings.get_f64("print_temp_warn_limit")),
                            nl
                        );
                        let _ = write!(
                            prefix,
                            ";EXTRUDER_TRAIN.{}.PPR_PRINTING_TEMPERATURE_LIMIT:{}{}",
                            extr_nr,
                            fmt_g(extruder_settings.get_f64("print_temp_anomaly_limit")),
                            nl
                        );
                    }
                    let _ = write!(prefix, ";PPR_BUILD_VOLUME_TEMPERATURE_WARNING:{}{}", fmt_g(scene.extruders[0].settings.get_f64("bv_temp_warn_limit")), nl);
                    let _ = write!(prefix, ";PPR_BUILD_VOLUME_TEMPERATURE_LIMIT:{}{}", fmt_g(scene.extruders[0].settings.get_f64("bv_temp_anomaly_limit")), nl);
                }
                let _ = write!(prefix, ";END_OF_HEADER{}", nl);
            }
            _ => {
                let _ = write!(prefix, ";FLAVOR:{}{}", Self::flavor_to_string(self.flavor), nl);
                let _ = write!(prefix, ";TIME:{}{}", print_time.map(|t| t.value() as i32).unwrap_or(6666), nl);
                if self.flavor == EGCodeFlavor::ULTIGCODE {
                    let _ = write!(prefix, ";MATERIAL:{}{}", if !filament_used.is_empty() { filament_used[0] as i32 } else { 6666 }, nl);
                    let _ = write!(prefix, ";MATERIAL2:{}{}", if filament_used.len() >= 2 { filament_used[1] as i32 } else { 0 }, nl);

                    let _ = write!(prefix, ";NOZZLE_DIAMETER:{}{}", fmt_g(scene.extruders[0].settings.get_f64("machine_nozzle_size")), nl);
                } else if self.flavor == EGCodeFlavor::REPRAP || self.flavor == EGCodeFlavor::MARLIN || self.flavor == EGCodeFlavor::MARLIN_VOLUMATRIC {
                    prefix.push_str(";Filament used: ");
                    if !filament_used.is_empty() {
                        for (i, used) in filament_used.iter().enumerate() {
                            if i > 0 {
                                prefix.push_str(", ");
                            }
                            if self.flavor != EGCodeFlavor::MARLIN_VOLUMATRIC {
                                let _ = write!(prefix, "{}m", fmt_g(used / (1000.0 * self.extruder_attr[i].filament_area)));
                            } else {
                                // Use volumetric filament used.
                                let _ = write!(prefix, "{}mm3", fmt_g(*used));
                            }
                        }
                    } else {
                        prefix.push_str("0m");
                    }
                    prefix.push_str(nl);
                    let _ = write!(prefix, ";Layer height: {}{}", fmt_g(scene.current_mesh_group().settings.get_f64("layer_height")), nl);
                }
                let bb = self.total_bounding_box;
                let _ = write!(prefix, ";MINX:{}{}", fmt_g(int2mm(bb.min.x)), nl);
                let _ = write!(prefix, ";MINY:{}{}", fmt_g(int2mm(bb.min.y)), nl);
                let _ = write!(prefix, ";MINZ:{}{}", fmt_g(int2mm(bb.min.z)), nl);
                let _ = write!(prefix, ";MAXX:{}{}", fmt_g(int2mm(bb.max.x)), nl);
                let _ = write!(prefix, ";MAXY:{}{}", fmt_g(int2mm(bb.max.y)), nl);
                let _ = write!(prefix, ";MAXZ:{}{}", fmt_g(int2mm(bb.max.z)), nl);
                let _ = write!(prefix, ";TARGET_MACHINE.NAME:{}{}", transliterate(&self.machine_name), nl);
            }
        }

        prefix
    }

    pub fn set_slice_uuid(&mut self, slice_uuid: &str) {
        self.slice_uuid = slice_uuid.to_string();
    }

    pub fn set_layer_nr(&mut self, layer_nr: LayerIndex) {
        self.layer_nr = layer_nr;
    }

    /// return whether the extruder has been used throughout printing all meshgroup up till now
    pub fn get_extruder_is_used(&self, extruder_nr: usize) -> bool {
        debug_assert!(extruder_nr < MAX_EXTRUDERS);
        self.extruder_attr[extruder_nr].is_used
    }

    pub fn get_gcode_pos(&self, x: Coord, y: Coord, extruder_train: usize) -> Point2LL {
        Point2LL::new(x, y) - self.get_nozzle_offset(extruder_train)
    }

    pub fn get_nozzle_offset(&self, extruder_train: usize) -> Point2LL {
        self.extruder_attr[extruder_train].nozzle_offset
    }

    pub fn set_flavor(&mut self, flavor: EGCodeFlavor) {
        self.flavor = flavor;
        if flavor == EGCodeFlavor::MACH3 {
            for n in 0..MAX_EXTRUDERS {
                self.extruder_attr[n].extruder_character = (b'A' + n as u8) as char;
            }
        } else {
            for n in 0..MAX_EXTRUDERS {
                self.extruder_attr[n].extruder_character = 'E';
            }
        }
        self.is_volumetric = flavor == EGCodeFlavor::ULTIGCODE || flavor == EGCodeFlavor::MARLIN_VOLUMATRIC;
    }

    pub fn get_flavor(&self) -> EGCodeFlavor {
        self.flavor
    }

    pub fn set_z(&mut self, z: i32) {
        self.current_layer_z = z as Coord;
    }

    pub fn set_flow_rate_extrusion_settings(&mut self, max_extrusion_offset: f64, extrusion_offset_factor: f64) {
        self.max_extrusion_offset = max_extrusion_offset;
        self.extrusion_offset_factor = extrusion_offset_factor;
    }

    /// Add extra amount of material to be primed after an unretraction.
    pub fn add_extra_prime_amount(&mut self, extra_prime_volume: f64) {
        self.extruder_attr[self.current_extruder].prime_volume += extra_prime_volume;
    }

    pub fn get_position(&self) -> Point3LL {
        self.current_position
    }

    pub fn get_position_xy(&self) -> Point2LL {
        self.current_position.to_point2ll()
    }

    pub fn get_position_z(&self) -> Coord {
        self.current_position.z
    }

    pub fn get_extruder_nr(&self) -> usize {
        self.current_extruder
    }

    pub fn set_filament_diameter(&mut self, extruder: usize, diameter: Coord) {
        let r = int2mm(diameter) / 2.0;
        let area = std::f64::consts::PI * r * r;
        self.extruder_attr[extruder].filament_area = area;
    }

    pub fn get_current_extruded_volume(&self) -> f64 {
        let attr = &self.extruder_attr[self.current_extruder];
        let mut extrusion_amount = self.current_e_value;
        if !attr.machine_firmware_retract {
            // no E values are changed to perform a retraction
            extrusion_amount -= attr.retraction_e_amount_at_e_start; // subtract the increment in E which was used for the first unretraction instead of extrusion
            extrusion_amount += attr.retraction_e_amount_current; // add the decrement in E which the filament is behind on extrusion due to the last retraction
        }
        if self.is_volumetric {
            extrusion_amount
        } else {
            extrusion_amount * attr.filament_area
        }
    }

    /// Get the total extruded volume for a specific extruder in mm^3
    pub fn get_total_filament_used(&self, extruder_nr: usize) -> f64 {
        if extruder_nr == self.current_extruder {
            return self.extruder_attr[extruder_nr].total_filament + self.get_current_extruded_volume();
        }
        self.extruder_attr[extruder_nr].total_filament
    }

    /// Get the total estimated print time in seconds for each feature
    pub fn get_total_print_time_per_feature(&self) -> Vec<Duration> {
        self.total_print_times.clone()
    }

    /// Get the total print time in seconds for the complete print
    pub fn get_sum_total_print_times(&self) -> f64 {
        let mut sum = 0.0;
        for item in &self.total_print_times {
            sum += item.value();
        }
        sum
    }

    pub fn update_total_print_time(&mut self) {
        let estimates = self.estimate_calculator.calculate();
        for (i, e) in estimates.iter().enumerate() {
            self.total_print_times[i] += *e;
        }
        self.estimate_calculator.reset();
        let sum = self.get_sum_total_print_times();
        self.write_time_comment(Duration::new(sum));
    }

    pub fn reset_total_print_time_and_filament(&mut self) {
        for t in self.total_print_times.iter_mut() {
            *t = Duration::new(0.0);
        }
        for e in 0..MAX_EXTRUDERS {
            self.extruder_attr[e].total_filament = 0.0;
            self.extruder_attr[e].current_temperature = 0.0;
            self.extruder_attr[e].waited_for_temperature = false;
        }
        self.current_e_value = 0.0;
        self.estimate_calculator.reset();
    }

    pub fn write_comment(&mut self, unsanitized_comment: &str) {
        let comment = transliterate(unsanitized_comment);

        self.w(";");
        for c in comment.chars() {
            if c == '\n' {
                let nl = self.new_line;
                self.w(nl);
                self.w(";");
            } else {
                self.out.push(c);
            }
        }
        self.nl();
    }

    pub fn write_type_comment(&mut self, type_: PrintFeatureType) {
        let s = match type_ {
            PrintFeatureType::OuterWall => ";TYPE:WALL-OUTER",
            PrintFeatureType::InnerWall => ";TYPE:WALL-INNER",
            PrintFeatureType::Skin => ";TYPE:SKIN",
            PrintFeatureType::Support => ";TYPE:SUPPORT",
            PrintFeatureType::SkirtBrim => ";TYPE:SKIRT",
            PrintFeatureType::Infill => ";TYPE:FILL",
            PrintFeatureType::SupportInfill => ";TYPE:SUPPORT",
            PrintFeatureType::SupportInterface => ";TYPE:SUPPORT-INTERFACE",
            PrintFeatureType::PrimeTower => ";TYPE:PRIME-TOWER",
            _ => return, // do nothing
        };
        self.w(s);
        self.nl();
    }

    /// Write an extrusion mode command if appropriate. (M83 for relative mode, M82 for absolute mode)
    pub fn write_extrusion_mode(&mut self, set_relative_extrusion_mode: bool) {
        if set_relative_extrusion_mode {
            self.w("M83 ;relative extrusion mode");
        } else {
            self.w("M82 ;absolute extrusion mode");
        }
        self.nl();
    }

    pub fn reset_extrusion_mode(&mut self) {
        let set_relative_extrusion_mode = self.get_flavor() == EGCodeFlavor::REPRAP;
        self.write_extrusion_mode(set_relative_extrusion_mode);
    }

    pub fn write_time_comment(&mut self, time: Duration) {
        let s = format!(";TIME_ELAPSED:{}", fmt_fixed(time.value()));
        self.w(&s);
        self.nl();
    }

    pub fn write_layer_comment(&mut self, layer_nr: LayerIndex) {
        let s = format!(";LAYER:{}", layer_nr);
        self.w(&s);
        self.nl();
    }

    pub fn write_layer_count_comment(&mut self, layer_count: usize) {
        let s = format!(";LAYER_COUNT:{}", layer_count);
        self.w(&s);
        self.nl();
    }

    pub fn write_line(&mut self, line: &str) {
        self.w(line);
        self.nl();
    }

    /// Reset the current_e_value to prevent too high E values.
    pub fn reset_extrusion_value(&mut self) {
        if !self.relative_extrusion {
            let s = format!("G92 {}0", self.extruder_attr[self.current_extruder].extruder_character);
            self.w(&s);
            self.nl();
        }
        let current_extruded_volume = self.get_current_extruded_volume();
        let attr = &mut self.extruder_attr[self.current_extruder];
        attr.total_filament += current_extruded_volume;
        for extruded_volume_at_retraction in attr.extruded_volume_at_previous_n_retractions.iter_mut() {
            // update the extruded_volume_at_previous_n_retractions only of the current extruder, since other extruders don't extrude the current volume
            *extruded_volume_at_retraction -= current_extruded_volume;
        }
        self.current_e_value = 0.0;
        attr.retraction_e_amount_at_e_start = attr.retraction_e_amount_current;
    }

    /// Start the new_extruder: write the starting gcode etc.
    pub fn initialize_extruder_trains(&mut self, storage: &SliceDataStorage, start_extruder_nr: usize) -> bool {
        let mut should_prime_extruder = true;
        let mesh_group_settings = &current_slice().scene.current_mesh_group().settings;

        if communication().is_sequential() {
            // If we must output the g-code sequentially, we must already place the g-code header here even if we don't know
            // the exact time/material usages yet.
            let prefix = self.get_file_header(&storage.get_extruders_used(), None, &[], &[]);
            self.write_code(&prefix);
        }

        self.write_comment(&format!("Generated with Cura_SteamEngine {}", crate::settings::settings::CURA_ENGINE_VERSION));

        if mesh_group_settings.get_bool("machine_start_gcode_first") {
            self.write_code(&mesh_group_settings.get_string("machine_start_gcode"));
        }

        if self.get_flavor() == EGCodeFlavor::GRIFFIN || self.get_flavor() == EGCodeFlavor::CHEETAH {
            let s = format!("T{}", start_extruder_nr);
            self.write_line(&s);
        } else {
            self.process_initial_layer_temperature(storage, start_extruder_nr);
        }

        if !mesh_group_settings.get_bool("machine_start_gcode_first") {
            self.write_code(&mesh_group_settings.get_string("machine_start_gcode"));
        }
        self.write_extrusion_mode(false); // ensure absolute extrusion mode is set before the start gcode

        // in case of shared nozzle assume that the machine-start gcode reset the extruders as per machine description
        if current_slice().scene.settings.get_bool("machine_extruders_share_nozzle") {
            for train in &current_slice().scene.extruders {
                self.reset_extruder_to_primed(train.extruder_nr, train.settings.get_f64("machine_extruders_shared_nozzle_initial_retraction"));
            }
        }

        if mesh_group_settings.get_bool("machine_heated_build_volume") {
            self.write_build_volume_temperature_command(mesh_group_settings.get_temperature("build_volume_temperature"), false);
        }

        communication().send_current_position(Point3LL::from_2d(self.get_position_xy(), 0));
        self.start_extruder(start_extruder_nr);

        if self.get_flavor() == EGCodeFlavor::BFB {
            self.write_comment("enable auto-retraction");
            let s = format!(
                "M227 S{} P{}",
                mesh_group_settings.get_coord("retraction_amount") * 2560 / 1000,
                mesh_group_settings.get_coord("retraction_amount") * 2560 / 1000
            );
            self.write_line(&s);
        } else if self.get_flavor() == EGCodeFlavor::GRIFFIN || self.get_flavor() == EGCodeFlavor::CHEETAH {
            // initialize extruder trains
            let train = &current_slice().scene.extruders[start_extruder_nr];
            self.process_initial_layer_temperature(storage, start_extruder_nr);
            self.write_prime_train(train.settings.get_velocity("speed_travel"));
            should_prime_extruder = false;
            let retraction_config = storage.retraction_wipe_config_per_extruder[start_extruder_nr].retraction_config;
            self.write_retraction(&retraction_config, false, false, None);
        }

        if mesh_group_settings.get_bool("relative_extrusion") {
            self.write_extrusion_mode(true);
        }
        if self.get_flavor() != EGCodeFlavor::GRIFFIN && self.get_flavor() != EGCodeFlavor::CHEETAH && mesh_group_settings.get_bool("retraction_enable") {
            // ensure extruder is zeroed
            self.reset_extrusion_value();

            // retract before first travel move
            let retraction_config = storage.retraction_wipe_config_per_extruder[start_extruder_nr].retraction_config;
            self.write_retraction(&retraction_config, false, false, None);
        }

        should_prime_extruder
    }

    fn process_initial_layer_bed_temperature(&mut self) {
        let scene = &current_slice().scene;
        let heated = scene.current_mesh_group().settings.get_bool("machine_heated_bed");
        let bed_temp = scene.current_mesh_group().settings.get_temperature("material_bed_temperature_layer_0");
        if heated && bed_temp != 0.0 {
            self.write_bed_temperature_command(bed_temp, scene.current_mesh_group().settings.get_bool("material_bed_temp_wait"));
        }
    }

    fn process_initial_layer_extruders_temperatures(&mut self, storage: &SliceDataStorage, wait_start_extruder: bool, start_extruder_nr: usize) {
        let scene = &current_slice().scene;
        let material_print_temp_prepend = scene.current_mesh_group().settings.get_bool("material_print_temp_prepend");
        let material_print_temp_wait = scene.current_mesh_group().settings.get_bool("material_print_temp_wait");

        if !material_print_temp_prepend && scene.current_mesh_group == 0 {
            // Nozzle initial temperatures are handled by start GCode, ignore
            return;
        }

        struct ExtruderInitialize {
            nr: usize,
            temperature: Temperature,
        }

        let mut all_extruders: Vec<ExtruderInitialize> = Vec::new();
        let extruders_used = storage.get_extruders_used();
        for (extruder_nr, used) in extruders_used.iter().enumerate() {
            if *used {
                let train = &scene.extruders[extruder_nr];
                let extruder_temp = if extruder_nr == start_extruder_nr {
                    let print_temp_0 = train.settings.get_temperature("material_print_temperature_layer_0");
                    if print_temp_0 != 0.0 {
                        print_temp_0
                    } else {
                        train.settings.get_temperature("material_print_temperature")
                    }
                } else {
                    train.settings.get_temperature("material_standby_temperature")
                };

                all_extruders.push(ExtruderInitialize { nr: extruder_nr, temperature: extruder_temp });
            }
        }

        // First set all the required temperatures at once, but without waiting so that all heaters start heating right now
        for extruder in &all_extruders {
            self.write_temperature_command(extruder.nr, extruder.temperature, false, true);
        }

        // Now wait for all the required temperatures one after the other
        for extruder in &all_extruders {
            if material_print_temp_wait || (extruder.nr == start_extruder_nr && wait_start_extruder) {
                self.write_temperature_command(extruder.nr, extruder.temperature, true, true);
            }
        }
    }

    fn write_raw_retract(&mut self, retraction_amounts: &RetractionAmounts) {
        self.current_e_value += retraction_amounts.diff_e;
        let output_e = if self.relative_extrusion { retraction_amounts.diff_e } else { self.current_e_value };
        let s = format!(" {}{}", self.extruder_attr[self.current_extruder].extruder_character, PrecisionedDouble::new(5, output_e));
        self.w(&s);
        self.extruder_attr[self.current_extruder].retraction_e_amount_current = retraction_amounts.new_e;
    }

    fn compute_retraction_amounts(&self, extruder_attributes: &ExtruderTrainAttributes, distance: f64) -> RetractionAmounts {
        let old_e = extruder_attributes.retraction_e_amount_current;
        let new_e = self.mm_to_e(distance);
        RetractionAmounts { old_e, new_e, diff_e: old_e - new_e }
    }

    /// Write the initial temperatures (bed and nozzles).
    pub fn process_initial_layer_temperature(&mut self, storage: &SliceDataStorage, start_extruder_nr: usize) {
        let scene = &current_slice().scene;
        let mut wait_start_extruder = false;
        let extruders_used = storage.get_extruders_used();
        let used_extruders = extruders_used.iter().filter(|u| **u).count();

        match self.get_flavor() {
            EGCodeFlavor::ULTIGCODE => return,
            EGCodeFlavor::GRIFFIN | EGCodeFlavor::CHEETAH => {
                wait_start_extruder = true;
            }
            _ => {
                if used_extruders > 1 || self.get_flavor() == EGCodeFlavor::REPRAP || !extruders_used[0] {
                    let s = format!("T{}", start_extruder_nr);
                    self.write_line(&s);
                }
            }
        }

        if scene.settings.get_usize("build_volume_fan_nr") != 0 {
            let fan_speed = scene.settings.get_ratio("build_volume_fan_speed_0") * 100.0;
            self.write_specific_fan_command(fan_speed, scene.settings.get_usize("build_volume_fan_nr"));
        }

        self.process_initial_layer_bed_temperature();
        self.process_initial_layer_extruders_temperatures(storage, wait_start_extruder, start_extruder_nr);
    }

    /// Whether the flavor needs a prime blob.
    pub fn need_prime_blob(&self) -> bool {
        matches!(self.get_flavor(), EGCodeFlavor::GRIFFIN | EGCodeFlavor::CHEETAH)
    }

    pub fn write_delay(&mut self, time_amount: Duration) {
        let s = format!("G4 P{}", (time_amount.value() * 1000.0) as i32);
        self.w(&s);
        self.nl();
        self.estimate_calculator.add_time(time_amount);
    }

    /// Coordinates are build plate coordinates, which might be offsetted when extruder offsets are encoded in the gcode.
    pub fn write_travel_2d(&mut self, p: Point2LL, speed: Velocity) {
        self.write_travel(Point3LL::new(p.x, p.y, self.current_layer_z), speed, None);
    }

    pub fn write_extrusion_2d(&mut self, p: Point2LL, speed: Velocity, extrusion_mm3_per_mm: f64, feature: PrintFeatureType, update_extrusion_offset: bool) {
        self.write_extrusion(Point3LL::new(p.x, p.y, self.current_layer_z), speed, extrusion_mm3_per_mm, feature, update_extrusion_offset);
    }

    pub fn write_travel(&mut self, p: Point3LL, speed: Velocity, retract_distance: Option<f64>) {
        if self.flavor == EGCodeFlavor::BFB {
            self.write_move_bfb(p.x as i32, p.y as i32, (p.z + self.is_z_hopped) as i32, speed, 0.0, PrintFeatureType::MoveUnretracted);
            return;
        }
        self.write_travel_xyz(p.x, p.y, p.z + self.is_z_hopped, speed, retract_distance);
    }

    pub fn write_extrusion(&mut self, p: Point3LL, speed: Velocity, extrusion_mm3_per_mm: f64, feature: PrintFeatureType, update_extrusion_offset: bool) {
        if self.flavor == EGCodeFlavor::BFB {
            self.write_move_bfb(p.x as i32, p.y as i32, p.z as i32, speed, extrusion_mm3_per_mm, feature);
            return;
        }
        self.write_extrusion_xyz(p.x, p.y, p.z, speed, extrusion_mm3_per_mm, feature, update_extrusion_offset);
    }

    fn write_move_bfb(&mut self, x: i32, y: i32, z: i32, speed: Velocity, extrusion_mm3_per_mm: f64, feature: PrintFeatureType) {
        if extrusion_mm3_per_mm.is_infinite() {
            crate::log_error!("Extrusion rate is infinite!");
            std::process::exit(1);
        }
        if extrusion_mm3_per_mm.is_nan() {
            crate::log_error!("Extrusion rate is not a number!");
            std::process::exit(1);
        }

        let extrusion_per_mm = self.mm3_to_e(extrusion_mm3_per_mm);

        let gcode_pos = self.get_gcode_pos(x as Coord, y as Coord, self.current_extruder);

        // For Bits From Bytes machines, we need to handle this completely differently. As they do not use E values but RPM values.
        let mut fspeed = speed * 60.0;
        let mut rpm = extrusion_per_mm * speed * 60.0;
        let mm_per_rpm = 4.0; // All BFB machines have 4mm per RPM extrusion.
        rpm /= mm_per_rpm;
        if rpm > 0.0 {
            if self.extruder_attr[self.current_extruder].retraction_e_amount_current != 0.0 {
                if self.current_speed != rpm {
                    let s = format!("M108 S{}", PrecisionedDouble::new(1, rpm));
                    self.w(&s);
                    self.nl();
                    self.current_speed = rpm;
                }
                // Add M101 or M201 to enable the proper extruder.
                let s = format!("M{}", ((self.current_extruder + 1) * 100 + 1) as i32);
                self.w(&s);
                self.nl();
                self.extruder_attr[self.current_extruder].retraction_e_amount_current = 0.0;
            }
            // Fix the speed by the actual RPM we are asking, because of rounding errors we cannot get all RPM values, but we have a lot more resolution in the feedrate value.
            //  (Trick copied from KISSlicer, thanks Jonathan)
            fspeed *= rpm / (((rpm * 100.0) as f32).round() as f64 / 100.0);

            // Increase the extrusion amount to calculate the amount of filament used.
            let diff = Point3LL::new(x as Coord, y as Coord, z as Coord) - self.get_position();

            self.current_e_value += extrusion_per_mm * diff.v_size_mm();
        } else {
            // If we are not extruding, check if we still need to disable the extruder. This causes a retraction due to auto-retraction.
            if self.extruder_attr[self.current_extruder].retraction_e_amount_current == 0.0 {
                self.w("M103");
                self.nl();
                self.extruder_attr[self.current_extruder].retraction_e_amount_current = 1.0; // 1.0 used as stub; BFB doesn't use the actual retraction amount; it performs retraction on the firmware automatically
            }
        }
        let s = format!("G1 X{} Y{} Z{} F{}", MMtoStream(gcode_pos.x), MMtoStream(gcode_pos.y), MMtoStream(z as Coord), PrecisionedDouble::new(1, fspeed));
        self.w(&s);
        self.nl();

        self.current_position = Point3LL::new(x as Coord, y as Coord, z as Coord);
        let pos = Position::new(
            int2mm(self.current_position.x),
            int2mm(self.current_position.y),
            int2mm(self.current_position.z),
            self.e_to_mm(self.current_e_value),
        );
        self.estimate_calculator.plan(pos, speed, feature);
    }

    fn write_travel_xyz(&mut self, x: Coord, y: Coord, z: Coord, speed: Velocity, retract_distance: Option<f64>) {
        let extruder_attr = self.extruder_attr[self.current_extruder].clone();
        let retraction_amounts = retract_distance.map(|d| self.compute_retraction_amounts(&extruder_attr, d));

        if self.current_position.x == x
            && self.current_position.y == y
            && self.current_position.z == z
            && (retraction_amounts.is_none() || !retraction_amounts.unwrap().has_retraction())
        {
            return;
        }

        let travel_move_type = Self::send_travel(Point3LL::new(x, y, z), speed, &extruder_attr, &retraction_amounts);

        self.w("G0");
        let e = self.current_e_value;
        self.write_fxyze(speed, x, y, z, e, travel_move_type, retraction_amounts);
    }

    #[allow(clippy::too_many_arguments)]
    fn write_extrusion_xyz(
        &mut self,
        x: Coord,
        y: Coord,
        z: Coord,
        speed: Velocity,
        extrusion_mm3_per_mm: f64,
        feature: PrintFeatureType,
        update_extrusion_offset: bool,
    ) {
        if self.current_position.x == x && self.current_position.y == y && self.current_position.z == z {
            return;
        }

        let extrusion_per_mm = self.mm3_to_e(extrusion_mm3_per_mm);

        if self.is_z_hopped > 0 {
            self.write_zhop_end(0.0, 0, None, 0.0);
        }

        let diff = Point3LL::new(x, y, z) - self.current_position;
        let diff_length = diff.v_size_mm();

        self.write_unretraction_and_prime();

        // flow rate compensation
        let mut extrusion_offset = 0.0;
        if diff_length != 0.0 {
            extrusion_offset = speed * extrusion_mm3_per_mm * self.extrusion_offset_factor;
            if extrusion_offset > self.max_extrusion_offset {
                extrusion_offset = self.max_extrusion_offset;
            }
        }
        // write new value of extrusion_offset, which will be remembered.
        if update_extrusion_offset && extrusion_offset != self.current_e_offset {
            self.current_e_offset = extrusion_offset;
            let s = format!(";FLOW_RATE_COMPENSATED_OFFSET = {}", fmt_fixed(self.current_e_offset));
            self.w(&s);
            self.nl();
        }

        self.extruder_attr[self.current_extruder].last_e_value_after_wipe += extrusion_per_mm * diff_length;
        let new_e_value = self.current_e_value + extrusion_per_mm * diff_length;

        self.w("G1");
        self.write_fxyze(speed, x, y, z, new_e_value, feature, None);
    }

    #[allow(clippy::too_many_arguments)]
    fn write_fxyze(&mut self, speed: Velocity, x: Coord, y: Coord, z: Coord, e: f64, feature: PrintFeatureType, retraction_amounts: Option<RetractionAmounts>) {
        if self.current_speed != speed {
            let s = format!(" F{}", PrecisionedDouble::new(1, speed * 60.0));
            self.w(&s);
            self.current_speed = speed;
        }

        let gcode_pos = self.get_gcode_pos(x, y, self.current_extruder);
        self.total_bounding_box.include(Point3LL::new(gcode_pos.x, gcode_pos.y, z));

        // This is done intentionally to include both X and Y in case of a change in either of them.
        if x != self.current_position.x || y != self.current_position.y {
            let s = format!(" X{} Y{}", MMtoStream(gcode_pos.x), MMtoStream(gcode_pos.y));
            self.w(&s);
        }

        if z != self.current_position.z {
            let s = format!(" Z{}", MMtoStream(z));
            self.w(&s);
        }

        if let Some(ra) = retraction_amounts {
            if ra.has_retraction() {
                self.write_raw_retract(&ra);
            }
        } else if e + self.current_e_offset != self.current_e_value {
            let output_e = if self.relative_extrusion { e + self.current_e_offset - self.current_e_value } else { e + self.current_e_offset };
            let s = format!(" {}{}", self.extruder_attr[self.current_extruder].extruder_character, PrecisionedDouble::new(5, output_e));
            self.w(&s);
            self.current_e_value = e;
        }
        self.nl();

        self.current_position = Point3LL::new(x, y, z);
        let pos = Position::new(int2mm(x), int2mm(y), int2mm(z), self.e_to_mm(e));
        self.estimate_calculator.plan(pos, speed, feature);
    }

    fn plan_current_position(&mut self, speed: Velocity, feature: PrintFeatureType, e: f64) {
        let pos = Position::new(int2mm(self.current_position.x), int2mm(self.current_position.y), int2mm(self.current_position.z), self.e_to_mm(e));
        self.estimate_calculator.plan(pos, speed, feature);
    }

    /// Write the command for priming the current extruder.
    pub fn write_unretraction_and_prime(&mut self) {
        let ce = self.current_extruder;
        let prime_volume = self.extruder_attr[ce].prime_volume;
        let prime_volume_e = self.mm3_to_e(prime_volume);
        self.current_e_value += prime_volume_e;
        if self.extruder_attr[ce].retraction_e_amount_current != 0.0 {
            if self.extruder_attr[ce].machine_firmware_retract {
                // note that BFB is handled differently
                self.w("G11");
                self.nl();
                // Assume default UM2 retraction settings.
                if prime_volume != 0.0 {
                    let output_e = if self.relative_extrusion { prime_volume_e } else { self.current_e_value };
                    let s = format!(
                        "G1 F{} {}{}",
                        PrecisionedDouble::new(1, self.extruder_attr[ce].last_retraction_prime_speed * 60.0),
                        self.extruder_attr[ce].extruder_character,
                        PrecisionedDouble::new(5, output_e)
                    );
                    self.w(&s);
                    self.nl();
                    self.current_speed = self.extruder_attr[ce].last_retraction_prime_speed;
                }
                let e = self.current_e_value;
                self.plan_current_position(25.0, PrintFeatureType::StationaryRetractUnretract, e);
            } else {
                self.current_e_value += self.extruder_attr[ce].retraction_e_amount_current;
                let output_e = if self.relative_extrusion { self.extruder_attr[ce].retraction_e_amount_current + prime_volume_e } else { self.current_e_value };
                let s = format!(
                    "G1 F{} {}{}",
                    PrecisionedDouble::new(1, self.extruder_attr[ce].last_retraction_prime_speed * 60.0),
                    self.extruder_attr[ce].extruder_character,
                    PrecisionedDouble::new(5, output_e)
                );
                self.w(&s);
                self.nl();
                self.current_speed = self.extruder_attr[ce].last_retraction_prime_speed;
                let (speed, e) = (self.current_speed, self.current_e_value);
                self.plan_current_position(speed, PrintFeatureType::StationaryRetractUnretract, e);
            }
        } else if prime_volume != 0.0 {
            let output_e = if self.relative_extrusion { prime_volume_e } else { self.current_e_value };
            let s = format!(
                "G1 F{} {}{}",
                PrecisionedDouble::new(1, self.extruder_attr[ce].last_retraction_prime_speed * 60.0),
                self.extruder_attr[ce].extruder_character,
                PrecisionedDouble::new(5, output_e)
            );
            self.w(&s);
            self.nl();
            self.current_speed = self.extruder_attr[ce].last_retraction_prime_speed;
            let (speed, e) = (self.current_speed, self.current_e_value);
            self.plan_current_position(speed, PrintFeatureType::StationaryRetractUnretract, e);
        }
        self.extruder_attr[ce].prime_volume = 0.0;

        if self.get_current_extruded_volume() > 10000.0 && self.flavor != EGCodeFlavor::BFB && self.flavor != EGCodeFlavor::MAKERBOT {
            // According to https://github.com/Ultimaker/CuraEngine/issues/14 having more then 21m of extrusion causes inaccuracies. So reset it
            // every 10m, just to be sure.
            self.reset_extrusion_value();
        }
        if self.extruder_attr[ce].retraction_e_amount_current != 0.0 {
            self.extruder_attr[ce].retraction_e_amount_current = 0.0;
        }
    }

    /// Write a retraction, returning whether a retraction was performed (or skipped because of the firmware).
    pub fn write_retraction(&mut self, config: &RetractionConfig, force: bool, extruder_switch: bool, retract_distance: Option<f64>) -> bool {
        let ce = self.current_extruder;

        if self.flavor == EGCodeFlavor::BFB {
            // BitsFromBytes does automatic retraction.
            if extruder_switch {
                if self.extruder_attr[ce].retraction_e_amount_current == 0.0 {
                    self.w("M103");
                    self.nl();
                }
                self.extruder_attr[ce].retraction_e_amount_current = 1.0; // 1.0 is a stub; BFB doesn't use the actual retracted amount; retraction is performed by firmware
            }
            return true;
        }

        {
            // handle retraction limitation
            let current_extruded_volume = self.get_current_extruded_volume();
            let filament_area = self.extruder_attr[ce].filament_area;
            let extruded_volume_at_previous_n_retractions = &mut self.extruder_attr[ce].extruded_volume_at_previous_n_retractions;
            while extruded_volume_at_previous_n_retractions.len() > config.retraction_count_max && !extruded_volume_at_previous_n_retractions.is_empty() {
                // extruder switch could have introduced data which falls outside the retraction window
                // also the retraction_count_max could have changed between the last retraction and this
                extruded_volume_at_previous_n_retractions.pop_back();
            }
            if !force && config.retraction_count_max == 0 {
                return false;
            }
            if !force
                && extruded_volume_at_previous_n_retractions.len() == config.retraction_count_max
                && current_extruded_volume < extruded_volume_at_previous_n_retractions.back().copied().unwrap_or(0.0) + config.retraction_extrusion_window * filament_area
            {
                return false;
            }
            extruded_volume_at_previous_n_retractions.push_front(current_extruded_volume);
            if extruded_volume_at_previous_n_retractions.len() == config.retraction_count_max + 1 {
                extruded_volume_at_previous_n_retractions.pop_back();
            }
        }

        let attr = self.extruder_attr[ce].clone();
        let retraction_amounts = self.compute_retraction_amounts(&attr, retract_distance.unwrap_or(config.distance));
        if !retraction_amounts.has_retraction() {
            return true;
        }

        if attr.machine_firmware_retract {
            if extruder_switch && attr.retraction_e_amount_current != 0.0 {
                return true;
            }
            self.w("G10");
            if extruder_switch && self.flavor == EGCodeFlavor::REPETIER {
                self.w(" S1");
            }
            self.nl();
            // Assume default UM2 retraction settings.
            let e = self.current_e_value + retraction_amounts.diff_e;
            self.plan_current_position(25.0, PrintFeatureType::StationaryRetractUnretract, e); // TODO: hardcoded values!

            self.extruder_attr[ce].retraction_e_amount_current = retraction_amounts.new_e; // suppose that for UM2 the retraction amount in the firmware is equal to the provided amount
        } else {
            let speed = if retraction_amounts.diff_e < 0.0 { config.speed } else { attr.last_retraction_prime_speed };
            let s = format!("G1 F{}", PrecisionedDouble::new(1, speed * 60.0));
            self.w(&s);
            self.write_raw_retract(&retraction_amounts);
            self.nl();
            self.current_speed = speed;
            let e = self.current_e_value;
            self.plan_current_position(speed, PrintFeatureType::StationaryRetractUnretract, e);
            self.extruder_attr[ce].last_retraction_prime_speed = config.prime_speed;
        }

        self.extruder_attr[ce].prime_volume += config.prime_volume;

        true
    }

    /// Start a z hop with the given `hop_height`.
    pub fn write_zhop_start(&mut self, hop_height: Coord, speed: Velocity, retract_distance: Option<f64>, retract_ratio: Ratio) {
        if retract_distance.is_some() && retract_ratio > 0.0 && retract_ratio < 1.0 {
            // We have to split the z-hop move in two sub-moves, one with retraction and one without
            self.write_zhop_start((hop_height as f64 * retract_ratio) as Coord, speed, retract_distance, 0.0);
            self.write_zhop_start(hop_height, speed, retract_distance, 0.0);
            return;
        }

        if hop_height > 0 {
            self.write_zhop(speed, hop_height, retract_distance);
        }
    }

    /// End a z hop: go back to the layer height.
    pub fn write_zhop_end(&mut self, speed: Velocity, height: Coord, prime_distance: Option<f64>, prime_ratio: Ratio) {
        if let Some(z_hop_prime_leftover) = self.z_hop_prime_leftover.take() {
            self.write_zhop_end(speed, height, Some(z_hop_prime_leftover.amount), z_hop_prime_leftover.ratio);
        }

        if self.is_z_hopped != 0 {
            if prime_distance.is_some() && prime_ratio > 0.0 && prime_ratio < 1.0 {
                // We have to split the z-hop move in two sub-moves, one without prime and one with
                let current_retraction = self.extruder_attr[self.current_extruder].retraction_e_amount_current;
                self.write_zhop_end(speed, (self.is_z_hopped as f64 * prime_ratio) as Coord, Some(current_retraction), 0.0);
                self.write_zhop_end(speed, 0, prime_distance, 0.0);
                return;
            }

            self.write_zhop(speed, height, prime_distance);
            self.current_position.z = self.current_layer_z;
        }
    }

    fn write_zhop(&mut self, mut speed: Velocity, height: Coord, retract_distance: Option<f64>) {
        if speed == 0.0 {
            let extruder = &current_slice().scene.extruders[self.current_extruder];
            speed = extruder.settings.get_velocity("speed_z_hop");
        }

        let extruder_attr = self.extruder_attr[self.current_extruder].clone();
        let mut retraction_amounts = RetractionAmounts::default();
        if let Some(d) = retract_distance {
            retraction_amounts = self.compute_retraction_amounts(&extruder_attr, d);
        }

        self.is_z_hopped = height;
        let target_z = self.current_layer_z + self.is_z_hopped;

        let travel_move_type =
            Self::send_travel(Point3LL::new(self.current_position.x, self.current_position.y, target_z), speed, &extruder_attr, &Some(retraction_amounts));

        self.w("G1");
        let (x, y, e) = (self.current_position.x, self.current_position.y, self.current_e_value);
        self.write_fxyze(speed, x, y, target_z, e, travel_move_type, Some(retraction_amounts));

        debug_assert!(speed > 0.0, "Z hop speed should be positive.");
        self.total_bounding_box.include_z(target_z);
    }

    fn send_travel(p: Point3LL, speed: Velocity, extruder_attr: &ExtruderTrainAttributes, retraction_amounts: &Option<RetractionAmounts>) -> PrintFeatureType {
        let travel_move_type = match retraction_amounts {
            Some(ra) if ra.has_retraction() => {
                if ra.diff_e < 0.0 {
                    PrintFeatureType::MoveWhileRetracting
                } else {
                    PrintFeatureType::MoveWhileUnretracting
                }
            }
            _ => {
                if extruder_attr.retraction_e_amount_current > 0.0 {
                    PrintFeatureType::MoveRetracted
                } else {
                    PrintFeatureType::MoveUnretracted
                }
            }
        };
        let display_width: Coord = if extruder_attr.retraction_e_amount_current != 0.0 { mm2int(0.2) } else { mm2int(0.1) };
        let layer_height = current_slice().scene.current_mesh_group().settings.get_f64("layer_height");
        communication().send_line_to(travel_move_type, p, display_width, layer_height as Coord, speed);

        travel_move_type
    }

    /// Start the new_extruder.
    pub fn start_extruder(&mut self, new_extruder: usize) {
        let extruder_settings = &current_slice().scene.extruders[new_extruder].settings;
        let prestart_code = extruder_settings.get_string("machine_extruder_prestart_code");
        let start_code = extruder_settings.get_string("machine_extruder_start_code");
        let start_code_duration = extruder_settings.get_duration("machine_extruder_start_code_duration");
        let extruder_change_duration = extruder_settings.get_duration("machine_extruder_change_duration");

        // Be nice to be able to calculate the extruder change time verses time
        // to heat and run this so it's run before the change call. **Future note**
        if !prestart_code.is_empty() {
            self.write_code_with_absolute_extrusion(&prestart_code);
        }

        self.extruder_attr[new_extruder].is_used = true;
        if new_extruder != self.current_extruder {
            // wouldn't be the case on the very first extruder start if it's extruder 0
            let s = if self.flavor == EGCodeFlavor::MAKERBOT { format!("M135 T{}", new_extruder) } else { format!("T{}", new_extruder) };
            self.w(&s);
            self.nl();
            // Only add time is we are actually changing extruders
            self.estimate_calculator.add_time(extruder_change_duration);
        }

        self.estimate_calculator.add_time(start_code_duration);
        let offset = self.get_nozzle_offset(new_extruder) - self.get_nozzle_offset(self.current_extruder);
        self.current_position += offset; // Change current position to new extruder coordinates
        self.current_extruder = new_extruder;

        debug_assert!(self.get_current_extruded_volume() == 0.0, "Just after an extruder switch we haven't extruded anything yet!");
        self.reset_extrusion_value(); // zero the E value on the new extruder, just to be sure

        if !start_code.is_empty() {
            self.write_code_with_absolute_extrusion(&start_code);
        }

        communication().set_extruder_for_send(&current_slice().scene.extruders[new_extruder]);
        communication().send_current_position(Point3LL::from_2d(self.get_position_xy(), 0));

        // Change the Z position so it gets re-written again. We do not know if the switch code modified the Z position.
        self.current_position.z += 1;
    }

    /// Switch to the new_extruder.
    pub fn switch_extruder(&mut self, new_extruder: usize, retraction_config_old_extruder: &RetractionConfig, perform_z_hop: Coord) {
        if self.current_extruder == new_extruder {
            return;
        }

        let old_extruder_settings = &current_slice().scene.extruders[self.current_extruder].settings;
        if old_extruder_settings.get_bool("retraction_enable") {
            let force = true;
            let extruder_switch = true;
            self.write_retraction(retraction_config_old_extruder, force, extruder_switch, None);
        }

        if perform_z_hop > 0 {
            self.write_zhop_start(perform_z_hop, 0.0, None, 0.0);
        }

        self.reset_extrusion_value(); // zero the E value on the old extruder, so that the current_e_value is registered on the old extruder

        let end_code = old_extruder_settings.get_string("machine_extruder_end_code");

        if !end_code.is_empty() {
            self.write_code_with_absolute_extrusion(&end_code);
        }

        let end_code_duration = old_extruder_settings.get_duration("machine_extruder_end_code_duration");
        self.estimate_calculator.add_time(end_code_duration);

        self.start_extruder(new_extruder);
    }

    pub fn write_code(&mut self, s: &str) {
        self.w(s);
        self.nl();
    }

    /// Write a G-code snippet while making sure absolute extrusion is active.
    pub fn write_code_with_absolute_extrusion(&mut self, s: &str) {
        if self.relative_extrusion {
            self.write_extrusion_mode(false); // ensure absolute extrusion mode is set before the gcode
        }

        self.write_code(s);

        if self.relative_extrusion {
            self.write_extrusion_mode(true); // restore relative extrusion mode
        }
    }

    pub fn reset_extruder_to_primed(&mut self, extruder: usize, initial_retraction: f64) {
        self.extruder_attr[extruder].is_primed = true;
        self.extruder_attr[extruder].retraction_e_amount_current = initial_retraction;
    }

    /// Write the gcode for priming the current extruder train so that it can be used.
    pub fn write_prime_train(&mut self, travel_speed: Velocity) {
        if self.extruder_attr[self.current_extruder].is_primed {
            // extruder is already primed once!
            return;
        }
        let extruder_settings = &current_slice().scene.extruders[self.current_extruder].settings;
        if extruder_settings.get_bool("prime_blob_enable") {
            // only move to prime position if we do a blob/poop
            let mut prime_pos = Point3LL::new(
                extruder_settings.get_coord("extruder_prime_pos_x"),
                extruder_settings.get_coord("extruder_prime_pos_y"),
                extruder_settings.get_coord("extruder_prime_pos_z"),
            );
            if !extruder_settings.get_bool("extruder_prime_pos_abs") {
                // currentPosition.z can be already z hopped
                prime_pos += Point3LL::new(self.current_position.x, self.current_position.y, self.current_layer_z);
            }
            self.write_travel(prime_pos, travel_speed, None);
        }

        if self.flavor == EGCodeFlavor::GRIFFIN || self.flavor == EGCodeFlavor::CHEETAH {
            let mut should_correct_z = false;

            let mut command = String::from("G280");
            if !extruder_settings.get_bool("prime_blob_enable") {
                command.push_str(" S1"); // use S1 to disable prime blob
                should_correct_z = true;
            }
            self.w(&command);
            self.nl();

            // There was an issue with the S1 strategy parameter, where it would only change the material-position,
            //   as opposed to 'be a prime-blob maneuvre without actually printing the prime blob', as we assumed here.
            if should_correct_z {
                // Can't output via 'writeTravel', since if this is needed, the value saved for 'current height' will not be correct.
                // For similar reasons, this isn't written to the front-end via command-socket.
                let s = format!("G0 Z{}", MMtoStream(self.get_position_z()));
                self.w(&s);
                self.nl();
            }
        } else {
            // there is no prime gcode for other firmware versions...
        }

        self.extruder_attr[self.current_extruder].is_primed = true;
    }

    /// Set the fan speed (0-100) of the fan of `extruder` (or the current one).
    pub fn write_fan_command(&mut self, speed: f64, extruder: Option<usize>) {
        let extruder_set_fan = extruder.unwrap_or(self.current_extruder);
        let fan_number = self.extruder_attr[extruder_set_fan].fan_number;

        self.write_specific_fan_command(speed, fan_number);
    }

    pub fn write_specific_fan_command(&mut self, speed: f64, fan_number: usize) {
        let current_fan_speed = self.current_fans_speeds.get(&fan_number).copied();

        if self.flavor == EGCodeFlavor::MAKERBOT {
            // Makerbot cannot PWM the fan speed, only turn it on or off
            let mut write_value = true;
            let new_on = speed >= 50.0;
            if let Some(cur) = current_fan_speed {
                let old_on = cur >= 50.0;
                write_value = new_on != old_on;
            }

            if write_value {
                if new_on {
                    self.w("M126 T0");
                } else {
                    self.w("M127 T0");
                }
                self.nl();
            }
        } else {
            let should_scale_zero_to_one = current_slice().scene.settings.get_bool("machine_scale_fan_speed_zero_to_one");
            let scale_zero_to_one_optional = |value: f64| -> PrecisionedDouble {
                PrecisionedDouble::new(if should_scale_zero_to_one { 2 } else { 1 }, (if should_scale_zero_to_one { value } else { value * 255.0 }) / 100.0)
            };
            let mut write_value = true;
            let num_new_val = scale_zero_to_one_optional(speed);
            let new_value_str = num_new_val.to_string();
            if let Some(cur) = current_fan_speed {
                let old_value = scale_zero_to_one_optional(cur).to_string();
                write_value = new_value_str != old_value;
            }

            if write_value {
                if num_new_val.would_write_zero() {
                    // Turn off when the fan value is zero.
                    self.w("M107");
                } else {
                    let s = format!("M106 S{}", new_value_str);
                    self.w(&s);
                }

                if fan_number != 0 {
                    let s = format!(" P{}", fan_number);
                    self.w(&s);
                }

                self.nl();
            }
        }

        self.current_fans_speeds.insert(fan_number, speed);
    }

    /// Write cooling fan speeds before proceeding an extruder switch
    pub fn write_prepare_fans_for_nozzle_switch(&mut self) {
        let settings = &current_slice().scene.settings;
        let cool_during_switch = settings.get_enum::<CoolDuringExtruderSwitch>("cool_during_extruder_switch");

        if cool_during_switch != CoolDuringExtruderSwitch::UNCHANGED {
            let current_extruder_fan_number = self.extruder_attr[self.current_extruder].fan_number;

            for fan_number in 0..self.fans_count {
                let fan_speed = if cool_during_switch == CoolDuringExtruderSwitch::ALL_FANS || fan_number == current_extruder_fan_number { 100.0 } else { 0.0 };

                self.write_specific_fan_command(fan_speed, fan_number);
            }
        }
    }

    /// Set the fans speed before an extrusion move, including the other fans if required.
    pub fn write_prepare_fans_for_extrusion(&mut self, current_extruder_new_speed: f64) {
        let settings = &current_slice().scene.settings;
        let cool_during_switch = settings.get_enum::<CoolDuringExtruderSwitch>("cool_during_extruder_switch");
        let current_extruder_fan_number = self.extruder_attr[self.current_extruder].fan_number;

        for fan_number in 0..self.fans_count {
            let new_fan_speed = if fan_number == current_extruder_fan_number {
                current_extruder_new_speed
            } else if cool_during_switch == CoolDuringExtruderSwitch::UNCHANGED {
                continue;
            } else {
                0.0
            };
            self.write_specific_fan_command(new_fan_speed, fan_number);
        }
    }

    /// Write a temperature command for the given extruder.
    pub fn write_temperature_command(&mut self, extruder: usize, temperature: Temperature, wait: bool, force_write_on_equal: bool) {
        let extruder_train = &current_slice().scene.extruders[extruder];

        if !extruder_train.settings.get_bool("machine_nozzle_temp_enabled") {
            return;
        }

        if extruder_train.settings.get_bool("machine_extruders_share_heater") {
            // extruders share a single heater
            if extruder != self.current_extruder {
                // ignore all changes to the non-current extruder
                return;
            }

            // sync all extruders with the change to the current extruder
            let extruder_count = current_slice().scene.extruders.len();

            for extruder_nr in 0..extruder_count {
                if extruder_nr != extruder {
                    // only reset the other extruders' waited_for_temperature state when the new temperature
                    // is greater than the old temperature
                    if wait || temperature > self.extruder_attr[extruder_nr].current_temperature {
                        self.extruder_attr[extruder_nr].waited_for_temperature = wait;
                    }
                    self.extruder_attr[extruder_nr].current_temperature = temperature;
                }
            }
        }

        if (!wait || self.extruder_attr[extruder].waited_for_temperature)
            && !force_write_on_equal
            && self.extruder_attr[extruder].current_temperature == temperature
        {
            return;
        }

        if wait && self.flavor != EGCodeFlavor::MAKERBOT {
            if self.flavor == EGCodeFlavor::MARLIN {
                self.w("M105"); // get temperatures from the last update, the M109 will not let get the target temperature
                self.nl();
            }
            self.w("M109");
            self.extruder_attr[extruder].waited_for_temperature = true;
        } else {
            self.w("M104");
            self.extruder_attr[extruder].waited_for_temperature = false;
        }
        if extruder != self.current_extruder {
            let s = format!(" T{}", extruder);
            self.w(&s);
        }
        let s = format!(" S{}", PrecisionedDouble::new(1, temperature));
        self.w(&s);
        self.nl();
        if extruder != self.current_extruder && self.always_write_active_tool {
            // Some firmwares (ie Smoothieware) change tools every time a "T" command is read - even on a M104 line, so we need to switch back to the active tool.
            let s = format!("T{}", self.current_extruder);
            self.w(&s);
            self.nl();
        }
        if wait && self.flavor == EGCodeFlavor::MAKERBOT {
            // Makerbot doesn't use M109 for heat-and-wait. Instead, use M104 and then wait using M116.
            self.w("M116");
            self.nl();
        }
        self.extruder_attr[extruder].current_temperature = temperature;
    }

    pub fn write_bed_temperature_command(&mut self, temperature: Temperature, wait: bool) {
        if self.flavor == EGCodeFlavor::ULTIGCODE {
            // The UM2 family doesn't support temperature commands (they are fixed in the firmware)
            return;
        }

        if self.bed_temperature != temperature {
            // Not already at the desired temperature.
            if wait {
                if self.flavor == EGCodeFlavor::MARLIN {
                    let s = format!("M140 S{}", PrecisionedDouble::new(1, temperature)); // set the temperature, it will be used as target temperature from M105
                    self.w(&s);
                    self.nl();
                    self.w("M105");
                    self.nl();
                }
                self.w("M190 S");
            } else {
                self.w("M140 S");
            }

            let s = PrecisionedDouble::new(1, temperature).to_string();
            self.w(&s);
            self.nl();

            self.bed_temperature = temperature;
        }
    }

    pub fn write_build_volume_temperature_command(&mut self, temperature: Temperature, wait: bool) {
        if self.flavor == EGCodeFlavor::ULTIGCODE || self.flavor == EGCodeFlavor::GRIFFIN || self.flavor == EGCodeFlavor::CHEETAH {
            // Ultimaker printers don't support build volume temperature commands.
            return;
        }
        if wait {
            self.w("M191 S");
        } else {
            self.w("M141 S");
        }
        let s = PrecisionedDouble::new(1, temperature).to_string();
        self.w(&s);
        self.nl();
    }

    /// Write the command for setting the acceleration for print moves to a specific value
    pub fn write_print_acceleration(&mut self, acceleration: Acceleration) {
        match self.get_flavor() {
            EGCodeFlavor::REPETIER => {
                if self.current_print_acceleration != acceleration {
                    let s = format!("M201 X{} Y{}", PrecisionedDouble::new(0, acceleration), PrecisionedDouble::new(0, acceleration));
                    self.w(&s);
                    self.nl();
                }
            }
            EGCodeFlavor::REPRAP => {
                if self.current_print_acceleration != acceleration {
                    let s = format!("M204 P{}", PrecisionedDouble::new(0, acceleration));
                    self.w(&s);
                    self.nl();
                }
            }
            _ => {
                // MARLIN, etc. only have one acceleration for both print and travel
                if self.current_print_acceleration != acceleration {
                    let s = format!("M204 S{}", PrecisionedDouble::new(0, acceleration));
                    self.w(&s);
                    self.nl();
                }
            }
        }
        self.current_print_acceleration = acceleration;
        self.estimate_calculator.set_acceleration(acceleration);
    }

    /// Write the command for setting the acceleration for travel moves to a specific value
    pub fn write_travel_acceleration(&mut self, acceleration: Acceleration) {
        match self.get_flavor() {
            EGCodeFlavor::REPETIER => {
                if self.current_travel_acceleration != acceleration {
                    let s = format!("M202 X{} Y{}", PrecisionedDouble::new(0, acceleration), PrecisionedDouble::new(0, acceleration));
                    self.w(&s);
                    self.nl();
                }
            }
            EGCodeFlavor::REPRAP => {
                if self.current_travel_acceleration != acceleration {
                    let s = format!("M204 T{}", PrecisionedDouble::new(0, acceleration));
                    self.w(&s);
                    self.nl();
                }
            }
            _ => {
                // MARLIN, etc. only have one acceleration for both print and travel
                self.write_print_acceleration(acceleration);
            }
        }
        self.current_travel_acceleration = acceleration;
        self.estimate_calculator.set_acceleration(acceleration);
    }

    /// Write the command for setting the jerk to a specific value
    pub fn write_jerk(&mut self, jerk: Velocity) {
        if self.current_jerk != jerk {
            let s = match self.get_flavor() {
                EGCodeFlavor::REPETIER => format!("M207 X{}", PrecisionedDouble::new(2, jerk)),
                EGCodeFlavor::REPRAP => format!("M566 X{} Y{}", PrecisionedDouble::new(2, jerk * 60.0), PrecisionedDouble::new(2, jerk * 60.0)),
                EGCodeFlavor::CHEETAH => format!("M215 X{} Y{}", PrecisionedDouble::new(2, jerk * 1000.0), PrecisionedDouble::new(2, jerk * 1000.0)),
                _ => format!("M205 X{} Y{}", PrecisionedDouble::new(2, jerk), PrecisionedDouble::new(2, jerk)),
            };
            self.w(&s);
            self.nl();
            self.current_jerk = jerk;

            if self.get_flavor() == EGCodeFlavor::CHEETAH {
                self.estimate_calculator.set_max_xy_jerk(jerk / 200.0);
            } else {
                self.estimate_calculator.set_max_xy_jerk(jerk);
            }
        }
    }

    /// Finish the gcode: set fan to zero, write the end code and flush all gcode.
    pub fn finalize(&mut self, end_code: &str) {
        self.write_fan_command(0.0, None);
        self.write_code(end_code);
        let print_time = self.get_sum_total_print_times() as i64;
        let mat_0 = self.get_total_filament_used(0) as i32;
        crate::log_info!("Print time (s): {}", print_time);
        crate::log_info!("Print time (hr|min|s): {}h {}m {}s", (print_time / 60 / 60) as i32, ((print_time / 60) % 60) as i32, (print_time % 60) as i32);
        crate::log_info!("Filament (mm^3): {}", mat_0);
        for n in 1..MAX_EXTRUDERS {
            if self.get_total_filament_used(n) > 0.0 {
                crate::log_info!("Filament {}: {}", n + 1, self.get_total_filament_used(n) as i32);
            }
        }
        self.flush_output_stream();
    }

    /// Get the amount of extruded material in mm^3 since the last wipe.
    pub fn get_extruded_volume_after_last_wipe(&self, extruder: usize) -> f64 {
        self.e_to_mm3(self.extruder_attr[extruder].last_e_value_after_wipe, extruder)
    }

    pub fn reset_last_e_value_after_wipe(&mut self, extruder: usize) {
        self.extruder_attr[extruder].last_e_value_after_wipe = 0.0;
    }

    /// Generate g-code for wiping the current nozzle.
    pub fn insert_wipe_script(&mut self, wipe_config: &WipeScriptConfig) {
        let prev_position = self.current_position;
        self.write_comment("WIPE_SCRIPT_BEGIN");

        if wipe_config.retraction_enable {
            self.write_retraction(&wipe_config.retraction_config, false, false, None);
        }

        if wipe_config.hop_enable {
            self.write_zhop_start(wipe_config.hop_amount, wipe_config.hop_speed, None, 0.0);
        }

        self.write_travel_2d(Point2LL::new(wipe_config.brush_pos_x, self.current_position.y), wipe_config.move_speed);
        for i in 0..wipe_config.repeat_count {
            let x = self.current_position.x + if i % 2 != 0 { -wipe_config.move_distance } else { wipe_config.move_distance };
            self.write_travel_2d(Point2LL::new(x, self.current_position.y), wipe_config.move_speed);
        }

        self.write_travel(prev_position, wipe_config.move_speed, None);

        if wipe_config.hop_enable {
            self.write_zhop_end(wipe_config.hop_speed, 0, None, 0.0);
        }

        if wipe_config.retraction_enable {
            self.write_unretraction_and_prime();
        }

        if wipe_config.pause.value() > 0.0 {
            self.write_delay(wipe_config.pause);
        }

        self.write_comment("WIPE_SCRIPT_END");
    }

    pub fn set_z_hop_prime_leftover(&mut self, z_hop_prime_leftover: ZHopAntiOozing) {
        self.z_hop_prime_leftover = Some(z_hop_prime_leftover);
    }

    /// Whether the machine handles retractions itself (firmware retraction or BFB).
    pub fn machine_handles_retraction(&self) -> bool {
        self.extruder_attr[0].machine_firmware_retract || self.flavor == EGCodeFlavor::BFB
    }

    /// The current z position of the layer (without z hop).
    pub fn current_layer_z(&self) -> Coord {
        self.current_layer_z
    }

    pub fn is_z_hopped(&self) -> Coord {
        self.is_z_hopped
    }

    pub fn current_extruder_retraction_e_amount(&self) -> f64 {
        self.extruder_attr[self.current_extruder].retraction_e_amount_current
    }
}
