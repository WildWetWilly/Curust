//! `TreeSupportEnums.h`, `TreeSupportBaseCircle.h` and `TreeSupportSettings.h`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use crate::geometry::point2ll::{pt, Point2LL};
use crate::geometry::polyline::Polygon;
use crate::settings::enums::{EFillMethod, ESupportType, SupportDistPriority};
use crate::settings::settings::Settings;
use crate::settings::types::{AngleDegrees, LayerIndex, TAU};
use crate::utils::coord::Coord;
use crate::utils::math::{round_divide, round_up_divide};
use crate::utils::simplify::Simplify;

use super::element::TreeSupportElement;

/// `InterfacePreference`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterfacePreference {
    InterfaceAreaOverwritesSupport,
    SupportAreaOverwritesInterface,
    InterfaceLinesOverwriteSupport,
    SupportLinesOverwriteInterface,
    Nothing,
}

/// `RestPreference`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestPreference {
    Buildplate,
    Graceful,
}

/// `AvoidanceType` (ordered as declared; `std::min` is taken over it when merging).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AvoidanceType {
    Slow,
    FastSafe,
    Fast,
    Collision,
}

/// `TreeSupportBaseCircle::base_radius`.
pub const BASE_RADIUS: Coord = 50;

/// `TreeSupportBaseCircle::getBaseCircle()`.
pub fn base_circle() -> &'static Polygon {
    static CIRCLE: OnceLock<Polygon> = OnceLock::new();
    CIRCLE.get_or_init(|| {
        const RESOLUTION: u64 = 12;
        let mut points = Vec::with_capacity(RESOLUTION as usize);
        for i in 0..RESOLUTION {
            // `const AngleRadians angle = static_cast<double>(i) / resolution * TAU;`
            let angle = crate::settings::types::AngleRadians::new(i as f64 / RESOLUTION as f64 * TAU).value();
            points.push(pt((angle.cos() * BASE_RADIUS as f64) as Coord, (angle.sin() * BASE_RADIUS as f64) as Coord));
        }
        Polygon::from_points(points)
    })
}

/// `TreeSupportSettings::some_model_contains_thick_roof` (static).
pub static SOME_MODEL_CONTAINS_THICK_ROOF: AtomicBool = AtomicBool::new(false);
/// `TreeSupportSettings::has_to_rely_on_min_xy_dist_only` (static).
pub static HAS_TO_RELY_ON_MIN_XY_DIST_ONLY: AtomicBool = AtomicBool::new(false);

/// `std::min` (returns the first argument when equal / unordered).
#[inline]
pub(crate) fn cmin<T: PartialOrd>(a: T, b: T) -> T {
    if b < a {
        b
    } else {
        a
    }
}

/// `std::max`.
#[inline]
pub(crate) fn cmax<T: PartialOrd>(a: T, b: T) -> T {
    if a < b {
        b
    } else {
        a
    }
}

/// `TreeSupportSettings`.
#[derive(Clone)]
pub struct TreeSupportSettings {
    angle: f64,
    angle_slow: f64,
    known_z: Vec<Coord>,
    pub support_line_width: Coord,
    pub layer_height: Coord,
    pub branch_radius: Coord,
    pub min_radius: Coord,
    pub max_radius: Coord,
    pub maximum_move_distance: Coord,
    pub maximum_move_distance_slow: Coord,
    pub support_bottom_layers: usize,
    pub tip_layers: usize,
    pub diameter_angle_scale_factor: f64,
    pub max_to_model_radius_increase: Coord,
    pub min_dtt_to_model: usize,
    pub increase_radius_until_radius: Coord,
    pub increase_radius_until_dtt: usize,
    pub support_rests_on_model: bool,
    pub support_rest_preference: RestPreference,
    pub xy_distance: Coord,
    pub bp_radius: Coord,
    pub layer_start_bp_radius: Coord,
    pub diameter_scale_bp_radius: f64,
    pub support_overrides: SupportDistPriority,
    pub xy_min_distance: Coord,
    pub z_distance_top: Coord,
    pub z_distance_top_layers: usize,
    pub z_distance_bottom_layers: usize,
    pub support_infill_angles: Vec<AngleDegrees>,
    pub support_roof_angles: Vec<AngleDegrees>,
    pub roof_pattern: EFillMethod,
    pub support_pattern: EFillMethod,
    pub support_roof_line_width: Coord,
    pub support_line_distance: Coord,
    pub support_bottom_offset: Coord,
    pub support_wall_count: i32,
    pub support_roof_wall_count: i32,
    pub zig_zaggify_support: bool,
    pub maximum_deviation: Coord,
    pub maximum_resolution: Coord,
    pub support_roof_line_distance: Coord,
    pub skip_some_zags: bool,
    pub zag_skip_count: usize,
    pub connect_zigzags: bool,
    pub interface_preference: InterfacePreference,
    pub settings: &'static Settings,
    pub min_feature_size: Coord,
    pub min_wall_line_width: Coord,
    pub fill_outline_gaps: bool,
    pub simplifier: Simplify,
}

impl TreeSupportSettings {
    pub fn new(s: &'static Settings) -> Self {
        let angle = s.get_angle_radians("support_tree_angle").value();
        let angle_slow = s.get_angle_radians("support_tree_angle_slow").value();
        let support_line_width = s.get_coord("support_line_width");
        let layer_height = s.get_coord("layer_height");
        let branch_radius = s.get_coord("support_tree_branch_diameter") / 2;
        let min_radius = s.get_coord("support_tree_tip_diameter") / 2;
        let max_radius = s.get_coord("support_tree_max_diameter") / 2;
        let maximum_move_distance = if angle < TAU / 4.0 { cppstd::llround(angle.tan() * layer_height as f64) } else { Coord::MAX };
        let maximum_move_distance_slow = if angle_slow < TAU / 4.0 { cppstd::llround(angle_slow.tan() * layer_height as f64) } else { Coord::MAX };
        let support_bottom_layers =
            if s.get_bool("support_bottom_enable") { round_divide(s.get_coord("support_bottom_height") as u64, layer_height as u64) as usize } else { 0 };
        let tip_layers = cmax(cmax((branch_radius - min_radius) / (support_line_width / 3), branch_radius / layer_height), 1i64) as usize;
        let diameter_angle_scale_factor = s.get_angle_radians("support_tree_branch_diameter_angle").value().sin() * layer_height as f64 / branch_radius as f64;
        let max_to_model_radius_increase = s.get_coord("support_tree_max_diameter_increase_by_merges_when_support_to_model") / 2;
        let min_dtt_to_model = round_up_divide(s.get_coord("support_tree_min_height_to_model") as u64, layer_height as u64) as usize;
        let increase_radius_until_radius = s.get_coord("support_tree_branch_diameter") / 2;
        // The conditional's common type is double.
        let increase_radius_until_dtt = (if increase_radius_until_radius <= branch_radius {
            (tip_layers as u64).wrapping_mul((increase_radius_until_radius / branch_radius) as u64) as f64
        } else {
            (increase_radius_until_radius - branch_radius) as f64 / (branch_radius as f64 * diameter_angle_scale_factor)
        }) as usize;
        let support_rests_on_model = s.get_enum::<ESupportType>("support_type") == ESupportType::EVERYWHERE;
        let support_rest_preference = if support_rests_on_model && s.get_str("support_tree_rest_preference") == "graceful" {
            RestPreference::Graceful
        } else {
            RestPreference::Buildplate
        };
        let mut xy_distance = s.get_coord("support_xy_distance");
        let bp_radius = s.get_coord("support_tree_bp_diameter") / 2;
        let diameter_scale_bp_radius = cmin(
            (0.7f64).sin() * layer_height as f64 / branch_radius as f64,
            1.0 / (branch_radius as f64 / (support_line_width as f64 / 2.0)),
        );
        let support_overrides = s.get_enum::<SupportDistPriority>("support_xy_overrides_z");
        let mut xy_min_distance =
            if support_overrides == SupportDistPriority::Z_OVERRIDES_XY { s.get_coord("support_xy_distance_overhang") } else { xy_distance };
        let z_distance_top = s.get_coord("support_top_distance");
        let z_distance_top_layers = round_up_divide(z_distance_top as u64, layer_height as u64) as usize;
        let z_distance_bottom_layers = round_up_divide(s.get_coord("support_bottom_distance") as u64, layer_height as u64) as usize;
        let mut support_infill_angles = s.get_vec_angle_degrees("support_infill_angles");
        let mut support_roof_angles = s.get_vec_angle_degrees("support_roof_angles");
        let roof_pattern = s.get_enum::<EFillMethod>("support_roof_pattern");
        let support_pattern = s.get_enum::<EFillMethod>("support_pattern");

        let layer_start_bp_radius = ((bp_radius - branch_radius) as f64 / (branch_radius as f64 * diameter_scale_bp_radius)) as Coord;

        if HAS_TO_RELY_ON_MIN_XY_DIST_ONLY.load(Ordering::Relaxed) {
            xy_min_distance = cmax(100, xy_min_distance);
        }
        xy_distance = cmax(xy_distance, xy_min_distance);

        let get_interface_angles = |angles: &mut Vec<AngleDegrees>, pattern: EFillMethod| {
            if angles.is_empty() {
                if pattern == EFillMethod::CONCENTRIC {
                    angles.push(AngleDegrees::new(0.0));
                } else if pattern == EFillMethod::TRIANGLES {
                    angles.push(AngleDegrees::new(90.0));
                } else {
                    if SOME_MODEL_CONTAINS_THICK_ROOF.load(Ordering::Relaxed) {
                        angles.push(AngleDegrees::new(45.0));
                        angles.push(AngleDegrees::new(135.0));
                    }
                    if angles.is_empty() {
                        angles.push(AngleDegrees::new(90.0));
                    }
                }
            }
        };
        if support_infill_angles.is_empty() {
            support_infill_angles.push(AngleDegrees::new(0.0));
        }
        get_interface_angles(&mut support_roof_angles, roof_pattern);
        let interface_preference = match s.get_str("support_interface_priority") {
            "support_area_overwrite_interface_area" => InterfacePreference::SupportAreaOverwritesInterface,
            "interface_area_overwrite_support_area" => InterfacePreference::InterfaceAreaOverwritesSupport,
            "support_lines_overwrite_interface_area" => InterfacePreference::SupportLinesOverwriteInterface,
            "interface_lines_overwrite_support_area" => InterfacePreference::InterfaceLinesOverwriteSupport,
            "nothing" => InterfacePreference::Nothing,
            other => panic!("unordered_map::at: unknown support_interface_priority {other}"),
        };

        TreeSupportSettings {
            angle,
            angle_slow,
            known_z: Vec::new(),
            support_line_width,
            layer_height,
            branch_radius,
            min_radius,
            max_radius,
            maximum_move_distance,
            maximum_move_distance_slow,
            support_bottom_layers,
            tip_layers,
            diameter_angle_scale_factor,
            max_to_model_radius_increase,
            min_dtt_to_model,
            increase_radius_until_radius,
            increase_radius_until_dtt,
            support_rests_on_model,
            support_rest_preference,
            xy_distance,
            bp_radius,
            layer_start_bp_radius,
            diameter_scale_bp_radius,
            support_overrides,
            xy_min_distance,
            z_distance_top,
            z_distance_top_layers,
            z_distance_bottom_layers,
            support_infill_angles,
            support_roof_angles,
            roof_pattern,
            support_pattern,
            support_roof_line_width: s.get_coord("support_roof_line_width"),
            support_line_distance: s.get_coord("support_line_distance"),
            support_bottom_offset: s.get_coord("support_bottom_offset"),
            support_wall_count: s.get_int("support_wall_count"),
            support_roof_wall_count: s.get_int("support_roof_wall_count"),
            zig_zaggify_support: s.get_bool("zig_zaggify_support"),
            maximum_deviation: s.get_coord("meshfix_maximum_deviation"),
            maximum_resolution: s.get_coord("meshfix_maximum_resolution"),
            support_roof_line_distance: s.get_coord("support_roof_line_distance"),
            skip_some_zags: s.get_bool("support_skip_some_zags"),
            zag_skip_count: s.get_usize("support_zag_skip_count"),
            connect_zigzags: s.get_bool("support_connect_zigzags"),
            interface_preference,
            settings: s,
            min_feature_size: s.get_coord("min_feature_size"),
            min_wall_line_width: s.get_coord("min_wall_line_width"),
            fill_outline_gaps: s.get_bool("fill_outline_gaps"),
            simplifier: Simplify::from_settings(s),
        }
    }

    /// `operator==`.
    pub fn equals(&self, other: &TreeSupportSettings) -> bool {
        let o = other;
        let s = self.settings;
        let os = other.settings;
        self.branch_radius == o.branch_radius
            && self.tip_layers == o.tip_layers
            && self.diameter_angle_scale_factor == o.diameter_angle_scale_factor
            && self.layer_start_bp_radius == o.layer_start_bp_radius
            && self.bp_radius == o.bp_radius
            && self.diameter_scale_bp_radius == o.diameter_scale_bp_radius
            && self.min_radius == o.min_radius
            && self.xy_min_distance == o.xy_min_distance
            && self.xy_distance - self.xy_min_distance == o.xy_distance - o.xy_min_distance
            && self.support_rests_on_model == o.support_rests_on_model
            && self.increase_radius_until_dtt == o.increase_radius_until_dtt
            && self.min_dtt_to_model == o.min_dtt_to_model
            && self.max_to_model_radius_increase == o.max_to_model_radius_increase
            && self.maximum_move_distance == o.maximum_move_distance
            && self.maximum_move_distance_slow == o.maximum_move_distance_slow
            && self.z_distance_bottom_layers == o.z_distance_bottom_layers
            && self.support_line_width == o.support_line_width
            && self.support_overrides == o.support_overrides
            && self.support_line_distance == o.support_line_distance
            && self.support_roof_line_width == o.support_roof_line_width
            && self.support_bottom_offset == o.support_bottom_offset
            && self.support_wall_count == o.support_wall_count
            && self.support_pattern == o.support_pattern
            && self.roof_pattern == o.roof_pattern
            && self.support_roof_angles == o.support_roof_angles
            && self.support_infill_angles == o.support_infill_angles
            && self.increase_radius_until_radius == o.increase_radius_until_radius
            && self.support_bottom_layers == o.support_bottom_layers
            && self.layer_height == o.layer_height
            && self.z_distance_top == o.z_distance_top
            && self.maximum_deviation == o.maximum_deviation
            && self.maximum_resolution == o.maximum_resolution
            && self.support_roof_line_distance == o.support_roof_line_distance
            && self.skip_some_zags == o.skip_some_zags
            && self.zag_skip_count == o.zag_skip_count
            && self.connect_zigzags == o.connect_zigzags
            && self.interface_preference == o.interface_preference
            && self.min_feature_size == o.min_feature_size
            && self.support_rest_preference == o.support_rest_preference
            && self.max_radius == o.max_radius
            && self.min_wall_line_width == o.min_wall_line_width
            && self.fill_outline_gaps == o.fill_outline_gaps
            && (self.interface_preference == InterfacePreference::InterfaceAreaOverwritesSupport
                || self.interface_preference == InterfacePreference::SupportAreaOverwritesInterface
                || (s.get_bool("fill_outline_gaps") == os.get_bool("fill_outline_gaps")
                    && s.get_coord("min_bead_width") == os.get_coord("min_bead_width")
                    && s.get_angle_radians("wall_transition_angle") == os.get_angle_radians("wall_transition_angle")
                    && s.get_coord("wall_transition_length") == os.get_coord("wall_transition_length")
                    && s.get_ratio("min_odd_wall_line_width") == os.get_ratio("min_odd_wall_line_width")
                    && s.get_ratio("min_even_wall_line_width") == os.get_ratio("min_even_wall_line_width")
                    && s.get_int("wall_distribution_count") == os.get_int("wall_distribution_count")
                    && s.get_coord("wall_transition_filter_distance") == os.get_coord("wall_transition_filter_distance")
                    && s.get_coord("wall_transition_filter_deviation") == os.get_coord("wall_transition_filter_deviation")
                    && s.get_coord("wall_line_width_x") == os.get_coord("wall_line_width_x")
                    && s.get_int("meshfix_maximum_extrusion_area_deviation") == os.get_int("meshfix_maximum_extrusion_area_deviation")))
    }

    /// `getEffectiveDTT`.
    #[inline]
    pub fn get_effective_dtt(&self, elem: &TreeSupportElement) -> usize {
        if elem.effective_radius_height < self.increase_radius_until_dtt {
            if elem.distance_to_top < self.increase_radius_until_dtt {
                elem.distance_to_top
            } else {
                self.increase_radius_until_dtt
            }
        } else {
            elem.effective_radius_height
        }
    }

    /// `getRadius(size_t distance_to_top, double buildplate_radius_increases = 0)`.
    pub fn get_radius_dtt(&self, distance_to_top: usize, buildplate_radius_increases: f64) -> Coord {
        let base: f64 = if distance_to_top <= self.tip_layers {
            // Unsigned arithmetic (coord_t * size_t), then converted to double by the conditional.
            let v = (self.min_radius as u64)
                .wrapping_add(((self.branch_radius - self.min_radius) as u64).wrapping_mul(distance_to_top as u64) / self.tip_layers as u64);
            v as f64
        } else {
            self.branch_radius as f64
                + ((self.branch_radius as u64).wrapping_mul((distance_to_top - self.tip_layers) as u64)) as f64 * self.diameter_angle_scale_factor
        };
        let uncapped = (base
            + self.branch_radius as f64 * buildplate_radius_increases * cmax(self.diameter_scale_bp_radius - self.diameter_angle_scale_factor, 0.0))
            as Coord;
        cmin(uncapped, self.max_radius)
    }

    /// `getRadius(const TreeSupportElement&)`.
    pub fn get_radius(&self, elem: &TreeSupportElement) -> Coord {
        self.get_radius_dtt(
            self.get_effective_dtt(elem),
            if (elem.is_result_on_layer_set() || !self.support_rests_on_model) && elem.to_buildplate { elem.buildplate_radius_increases } else { 0.0 },
        )
    }

    /// `getCollisionRadius`.
    pub fn get_collision_radius(&self, elem: &TreeSupportElement) -> Coord {
        self.get_radius_dtt(elem.effective_radius_height, elem.buildplate_radius_increases)
    }

    /// `recommendedMinRadius`.
    pub fn recommended_min_radius(&self, layer_idx: LayerIndex) -> Coord {
        let scale = (self.layer_start_bp_radius - layer_idx) as f64 * self.diameter_scale_bp_radius;
        if scale > 0.0 {
            cmin((self.branch_radius as f64 + self.branch_radius as f64 * scale) as Coord, self.max_radius)
        } else {
            0
        }
    }

    /// `getActualZ`.
    pub fn get_actual_z(&self, layer_idx: LayerIndex) -> Coord {
        let n = self.known_z.len() as LayerIndex;
        if layer_idx < n {
            self.known_z[layer_idx as usize]
        } else if (layer_idx - n) * self.layer_height + n != 0 {
            *self.known_z.last().unwrap_or(&0)
        } else {
            0
        }
    }

    pub fn set_actual_z(&mut self, z: &[Coord]) {
        self.known_z = z.to_vec();
    }

    #[allow(dead_code)]
    pub fn angles(&self) -> (f64, f64) {
        (self.angle, self.angle_slow)
    }
}

/// Point helper: `p + corner * k` for the base circle.
#[inline]
pub fn circle_around(p: Point2LL, mult: Coord) -> Polygon {
    let mut pts = Vec::with_capacity(base_circle().len());
    for corner in base_circle().iter() {
        pts.push(p + *corner * mult);
    }
    Polygon::from_points(pts)
}
