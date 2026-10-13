//! `BeadingStrategy/*`: how many beads (wall lines) of which width fill a given thickness.
//!
//! The C++ code is a chain of decorators with virtual methods; methods a decorator does not
//! override fall back to the `BeadingStrategy` base implementation, evaluated with the
//! decorator's own (copied) parameters and virtual functions. This port keeps exactly that
//! dispatch: every node of the chain carries the base fields plus a `Kind`.

use crate::settings::types::{AngleRadians, Ratio};
use crate::utils::coord::Coord;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Beading {
    pub total_thickness: Coord,
    /// The line width of each bead from the outer inset inward.
    pub bead_widths: Vec<Coord>,
    /// The distance of the toolpath location of each bead from the outline.
    pub toolpath_locations: Vec<Coord>,
    /// The distance not covered by any bead; gap area.
    pub left_over: Coord,
}

#[derive(Debug)]
enum Kind {
    Distributed { one_over_distribution_radius_squared: f64 },
    Redistribute { parent: Box<BeadingStrategy>, optimal_width_outer: Coord, minimum_variable_line_ratio: Ratio },
    Widening { parent: Box<BeadingStrategy>, min_input_width: Coord, min_output_width: Coord },
    OuterWallInset { parent: Box<BeadingStrategy>, outer_wall_offset: Coord },
    Limited { max_bead_count: Coord, parent: Box<BeadingStrategy> },
}

#[derive(Debug)]
pub struct BeadingStrategy {
    pub name: String,
    optimal_width: Coord,
    wall_split_middle_threshold: Ratio,
    wall_add_middle_threshold: Ratio,
    default_transition_length: Coord,
    transitioning_angle: AngleRadians,
    kind: Kind,
}

pub type BeadingStrategyPtr = Box<BeadingStrategy>;

impl BeadingStrategy {
    /// Copy of the base fields (`BeadingStrategy(const BeadingStrategy& other)`).
    fn base_copy(other: &BeadingStrategy, kind: Kind) -> Self {
        BeadingStrategy {
            name: other.name.clone(),
            optimal_width: other.optimal_width,
            wall_split_middle_threshold: other.wall_split_middle_threshold,
            wall_add_middle_threshold: other.wall_add_middle_threshold,
            default_transition_length: other.default_transition_length,
            transitioning_angle: other.transitioning_angle,
            kind,
        }
    }

    pub fn new_distributed(
        optimal_width: Coord,
        default_transition_length: Coord,
        transitioning_angle: AngleRadians,
        wall_split_middle_threshold: Ratio,
        wall_add_middle_threshold: Ratio,
        distribution_radius: i32,
    ) -> Self {
        let one_over = if distribution_radius >= 2 {
            let d = (distribution_radius - 1) as f32;
            (1.0f32 / d * 1.0f32 / d) as f64
        } else {
            (1.0f32 / 1.0 * 1.0f32 / 1.0) as f64
        };
        BeadingStrategy {
            name: "DistributedBeadingStrategy".into(),
            optimal_width,
            wall_split_middle_threshold,
            wall_add_middle_threshold,
            default_transition_length,
            // BeadingStrategy(..., double transitioning_angle) stores it in an AngleRadians.
            transitioning_angle: AngleRadians::new(transitioning_angle.value()),
            kind: Kind::Distributed { one_over_distribution_radius_squared: one_over },
        }
    }

    pub fn new_redistribute(optimal_width_outer: Coord, minimum_variable_line_ratio: Ratio, parent: BeadingStrategyPtr) -> Self {
        let mut s = Self::base_copy(&parent, Kind::Distributed { one_over_distribution_radius_squared: 0.0 });
        s.name = "RedistributeBeadingStrategy".into();
        s.kind = Kind::Redistribute { parent, optimal_width_outer, minimum_variable_line_ratio };
        s
    }

    pub fn new_widening(parent: BeadingStrategyPtr, min_input_width: Coord, min_output_width: Coord) -> Self {
        let mut s = Self::base_copy(&parent, Kind::Distributed { one_over_distribution_radius_squared: 0.0 });
        s.kind = Kind::Widening { parent, min_input_width, min_output_width };
        s
    }

    pub fn new_outer_wall_inset(outer_wall_offset: Coord, parent: BeadingStrategyPtr) -> Self {
        let mut s = Self::base_copy(&parent, Kind::Distributed { one_over_distribution_radius_squared: 0.0 });
        s.name = "OuterWallOfsetBeadingStrategy".into();
        s.kind = Kind::OuterWallInset { parent, outer_wall_offset };
        s
    }

    pub fn new_limited(max_bead_count: Coord, parent: BeadingStrategyPtr) -> Self {
        let mut s = Self::base_copy(&parent, Kind::Distributed { one_over_distribution_radius_squared: 0.0 });
        s.kind = Kind::Limited { max_bead_count, parent };
        s
    }

    pub fn get_optimal_width(&self) -> Coord {
        self.optimal_width
    }

    pub fn get_split_middle_threshold(&self) -> Ratio {
        self.wall_split_middle_threshold
    }

    pub fn get_add_middle_threshold(&self) -> Ratio {
        self.wall_add_middle_threshold
    }

    pub fn get_default_transition_length(&self) -> Coord {
        self.default_transition_length
    }

    pub fn get_transitioning_angle(&self) -> AngleRadians {
        self.transitioning_angle
    }

    // ---- base implementations -------------------------------------------------------------

    fn base_get_transitioning_length(&self, lower_bead_count: Coord) -> Coord {
        if lower_bead_count == 0 {
            return 10;
        }
        self.default_transition_length
    }

    fn base_get_transition_anchor_pos(&self, lower_bead_count: Coord) -> f64 {
        let lower_optimum = self.get_optimal_thickness(lower_bead_count);
        let transition_point = self.get_transition_thickness(lower_bead_count);
        let upper_optimum = self.get_optimal_thickness(lower_bead_count + 1);
        1.0 - (transition_point - lower_optimum) as f64 / (upper_optimum - lower_optimum) as f64
    }

    fn base_get_optimal_thickness(&self, bead_count: Coord) -> Coord {
        self.optimal_width * bead_count
    }

    fn base_get_transition_thickness(&self, lower_bead_count: Coord) -> Coord {
        let lower_ideal_width = self.get_optimal_thickness(lower_bead_count);
        let higher_ideal_width = self.get_optimal_thickness(lower_bead_count + 1);
        let threshold = if lower_bead_count % 2 == 1 { self.wall_split_middle_threshold } else { self.wall_add_middle_threshold };
        (lower_ideal_width as f64 + threshold * (higher_ideal_width - lower_ideal_width) as f64) as Coord
    }

    // ---- virtual methods --------------------------------------------------------------------

    pub fn compute(&self, thickness: Coord, bead_count: Coord) -> Beading {
        match &self.kind {
            Kind::Distributed { one_over_distribution_radius_squared } => {
                self.distributed_compute(*one_over_distribution_radius_squared, thickness, bead_count)
            }
            Kind::Redistribute { parent, optimal_width_outer, minimum_variable_line_ratio } => {
                redistribute_compute(parent, *optimal_width_outer, *minimum_variable_line_ratio, thickness, bead_count)
            }
            Kind::Widening { parent, min_input_width, min_output_width } => {
                if thickness < self.optimal_width {
                    let mut ret = Beading { total_thickness: thickness, ..Default::default() };
                    if thickness >= *min_input_width {
                        ret.bead_widths.push(thickness.max(*min_output_width));
                        ret.toolpath_locations.push(thickness / 2);
                    } else {
                        ret.left_over = thickness;
                    }
                    ret
                } else {
                    parent.compute(thickness, bead_count)
                }
            }
            Kind::OuterWallInset { parent, outer_wall_offset } => {
                let mut ret = parent.compute(thickness, bead_count);
                let bead_count = ret.bead_widths.iter().filter(|&&w| w > 0).count() as Coord;
                if bead_count < 2 {
                    return ret;
                }
                ret.toolpath_locations[0] = (ret.toolpath_locations[0] + outer_wall_offset).min(thickness / 2);
                ret
            }
            Kind::Limited { max_bead_count, parent } => limited_compute(*max_bead_count, parent, thickness, bead_count),
        }
    }

    pub fn get_optimal_thickness(&self, bead_count: Coord) -> Coord {
        match &self.kind {
            Kind::Distributed { .. } => self.base_get_optimal_thickness(bead_count),
            Kind::Redistribute { parent, optimal_width_outer, .. } => {
                let inner_bead_count = (0 as Coord).max(bead_count - 2);
                let outer_bead_count = bead_count - inner_bead_count;
                parent.get_optimal_thickness(inner_bead_count) + optimal_width_outer * outer_bead_count
            }
            Kind::Widening { parent, .. } => parent.get_optimal_thickness(bead_count),
            Kind::OuterWallInset { parent, .. } => parent.get_optimal_thickness(bead_count),
            Kind::Limited { max_bead_count, parent } => {
                if bead_count <= *max_bead_count {
                    return parent.get_optimal_thickness(bead_count);
                }
                10000000 // 10 meter
            }
        }
    }

    pub fn get_transition_thickness(&self, lower_bead_count: Coord) -> Coord {
        match &self.kind {
            Kind::Distributed { .. } => self.base_get_transition_thickness(lower_bead_count),
            Kind::Redistribute { parent, optimal_width_outer, minimum_variable_line_ratio } => match lower_bead_count {
                0 => (minimum_variable_line_ratio * *optimal_width_outer as f64) as Coord,
                1 => ((1.0 + parent.get_split_middle_threshold()) * *optimal_width_outer as f64) as Coord,
                _ => parent.get_transition_thickness(lower_bead_count - 2) + 2 * optimal_width_outer,
            },
            Kind::Widening { parent, min_input_width, .. } => {
                if lower_bead_count == 0 {
                    *min_input_width
                } else {
                    parent.get_transition_thickness(lower_bead_count)
                }
            }
            Kind::OuterWallInset { parent, .. } => parent.get_transition_thickness(lower_bead_count),
            Kind::Limited { max_bead_count, parent } => {
                if lower_bead_count < *max_bead_count {
                    return parent.get_transition_thickness(lower_bead_count);
                }
                if lower_bead_count == *max_bead_count {
                    return parent.get_optimal_thickness(lower_bead_count + 1) - 10;
                }
                9000000 // 9 meter
            }
        }
    }

    pub fn get_optimal_bead_count(&self, thickness: Coord) -> Coord {
        match &self.kind {
            Kind::Distributed { .. } => {
                let naive_count = thickness / self.optimal_width; // How many lines we can fit in for sure.
                let remainder = thickness - naive_count * self.optimal_width; // Space left after fitting that many lines.
                let threshold = if naive_count % 2 == 1 { self.wall_split_middle_threshold } else { self.wall_add_middle_threshold };
                let minimum_line_width = (self.optimal_width as f64 * threshold) as Coord;
                naive_count + (remainder >= minimum_line_width) as Coord // If there's enough space, fit an extra one.
            }
            Kind::Redistribute { parent, optimal_width_outer, minimum_variable_line_ratio } => {
                if (thickness as f64) < minimum_variable_line_ratio * *optimal_width_outer as f64 {
                    return 0;
                }
                if thickness <= 2 * optimal_width_outer {
                    return if thickness as f64 > (1.0 + parent.get_split_middle_threshold()) * *optimal_width_outer as f64 { 2 } else { 1 };
                }
                parent.get_optimal_bead_count(thickness - 2 * optimal_width_outer) + 2
            }
            Kind::Widening { parent, min_input_width, .. } => {
                if thickness < *min_input_width {
                    return 0;
                }
                let ret = parent.get_optimal_bead_count(thickness);
                if thickness >= *min_input_width && ret < 1 {
                    return 1;
                }
                ret
            }
            Kind::OuterWallInset { parent, .. } => parent.get_optimal_bead_count(thickness),
            Kind::Limited { max_bead_count, parent } => {
                let parent_bead_count = parent.get_optimal_bead_count(thickness);
                if parent_bead_count <= *max_bead_count {
                    parent.get_optimal_bead_count(thickness)
                } else if parent_bead_count == max_bead_count + 1 {
                    if thickness < parent.get_optimal_thickness(max_bead_count + 1) - 10 {
                        *max_bead_count
                    } else {
                        max_bead_count + 1
                    }
                } else {
                    max_bead_count + 1
                }
            }
        }
    }

    pub fn get_transitioning_length(&self, lower_bead_count: Coord) -> Coord {
        match &self.kind {
            Kind::Distributed { .. } => self.base_get_transitioning_length(lower_bead_count),
            Kind::Redistribute { parent, .. }
            | Kind::Widening { parent, .. }
            | Kind::OuterWallInset { parent, .. }
            | Kind::Limited { parent, .. } => parent.get_transitioning_length(lower_bead_count),
        }
    }

    pub fn get_transition_anchor_pos(&self, lower_bead_count: Coord) -> f64 {
        match &self.kind {
            Kind::Distributed { .. } | Kind::OuterWallInset { .. } => self.base_get_transition_anchor_pos(lower_bead_count),
            Kind::Redistribute { parent, .. } | Kind::Widening { parent, .. } | Kind::Limited { parent, .. } => {
                parent.get_transition_anchor_pos(lower_bead_count)
            }
        }
    }

    pub fn get_nonlinear_thicknesses(&self, lower_bead_count: Coord) -> Vec<Coord> {
        match &self.kind {
            Kind::Widening { parent, min_output_width, .. } => {
                let mut ret = vec![*min_output_width];
                ret.extend(parent.get_nonlinear_thicknesses(lower_bead_count));
                ret
            }
            _ => Vec::new(),
        }
    }

    fn distributed_compute(&self, one_over_distribution_radius_squared: f64, thickness: Coord, bead_count: Coord) -> Beading {
        let mut ret = Beading { total_thickness: thickness, ..Default::default() };
        if bead_count > 2 {
            let to_be_divided = thickness - bead_count * self.optimal_width;
            let middle = (bead_count - 1) as f64 / 2.0;

            let get_weight = |bead_idx: Coord| -> f64 {
                let dev_from_middle = bead_idx as f64 - middle;
                let w = 1.0 - one_over_distribution_radius_squared * dev_from_middle * dev_from_middle;
                if 0.0 < w {
                    w
                } else {
                    0.0
                }
            };

            let weights: Vec<f64> = (0..bead_count).map(get_weight).collect();
            let mut total_weight = 0.0;
            for w in &weights {
                total_weight += w;
            }
            for bead_idx in 0..bead_count {
                let weight_fraction = weights[bead_idx as usize] / total_weight;
                let splitup_left_over_weight = (to_be_divided as f64 * weight_fraction) as Coord;
                let width = self.optimal_width + splitup_left_over_weight;
                if bead_idx == 0 {
                    ret.toolpath_locations.push(width / 2);
                } else {
                    let loc = ret.toolpath_locations.last().unwrap() + (ret.bead_widths.last().unwrap() + width) / 2;
                    ret.toolpath_locations.push(loc);
                }
                ret.bead_widths.push(width);
            }
            ret.left_over = 0;
        } else if bead_count == 2 {
            let outer_width = thickness / 2;
            ret.bead_widths.push(outer_width);
            ret.bead_widths.push(outer_width);
            ret.toolpath_locations.push(outer_width / 2);
            ret.toolpath_locations.push(thickness - outer_width / 2);
            ret.left_over = 0;
        } else if bead_count == 1 {
            let outer_width = thickness;
            ret.bead_widths.push(outer_width);
            ret.toolpath_locations.push(outer_width / 2);
            ret.left_over = 0;
        } else {
            ret.left_over = thickness;
        }
        ret
    }
}

fn redistribute_compute(
    parent: &BeadingStrategy,
    optimal_width_outer: Coord,
    minimum_variable_line_ratio: Ratio,
    thickness: Coord,
    bead_count: Coord,
) -> Beading {
    let mut ret = Beading::default();

    // Take care of all situations in which no lines are actually produced:
    if bead_count == 0 || (thickness as f64) < minimum_variable_line_ratio * optimal_width_outer as f64 {
        ret.left_over = thickness;
        ret.total_thickness = thickness;
        return ret;
    }

    // Compute the beadings of the inner walls, if any:
    let inner_bead_count = bead_count - 2;
    let inner_thickness = thickness - 2 * optimal_width_outer;
    if inner_bead_count > 0 && inner_thickness > 0 {
        ret = parent.compute(inner_thickness, inner_bead_count);
        for toolpath_location in ret.toolpath_locations.iter_mut() {
            *toolpath_location += optimal_width_outer;
        }
    }

    // Insert the outer wall(s) around the previously computed inner wall(s), which may be empty:
    let actual_outer_thickness = if bead_count > 2 { (thickness / 2).min(optimal_width_outer) } else { thickness / bead_count };
    ret.bead_widths.insert(0, actual_outer_thickness);
    ret.toolpath_locations.insert(0, actual_outer_thickness / 2);
    if bead_count > 1 {
        ret.bead_widths.push(actual_outer_thickness);
        ret.toolpath_locations.push(thickness - actual_outer_thickness / 2);
    }

    // Ensure correct total and left over thickness.
    ret.total_thickness = thickness;
    ret.left_over = thickness - ret.bead_widths.iter().sum::<Coord>();
    ret
}

fn limited_compute(max_bead_count: Coord, parent: &BeadingStrategy, thickness: Coord, mut bead_count: Coord) -> Beading {
    if bead_count <= max_bead_count {
        let mut ret = parent.compute(thickness, bead_count);
        bead_count = ret.toolpath_locations.len() as Coord;

        if bead_count % 2 == 0 && bead_count == max_bead_count {
            let mid = (max_bead_count / 2) as usize;
            let innermost_toolpath_location = ret.toolpath_locations[mid - 1];
            let innermost_toolpath_width = ret.bead_widths[mid - 1];
            ret.toolpath_locations.insert(mid, innermost_toolpath_location + innermost_toolpath_width / 2);
            ret.bead_widths.insert(mid, 0);
        }
        return ret;
    }

    let optimal_thickness = parent.get_optimal_thickness(max_bead_count);
    let mut ret = parent.compute(optimal_thickness, max_bead_count);
    bead_count = ret.toolpath_locations.len() as Coord;
    ret.left_over += thickness - ret.total_thickness;
    ret.total_thickness = thickness;

    // Enforce symmetry
    if bead_count % 2 == 1 {
        ret.toolpath_locations[(bead_count / 2) as usize] = thickness / 2;
        ret.bead_widths[(bead_count / 2) as usize] = thickness - optimal_thickness;
    }
    for bead_idx in 0..(bead_count + 1) / 2 {
        ret.toolpath_locations[(bead_count - 1 - bead_idx) as usize] = thickness - ret.toolpath_locations[bead_idx as usize];
    }

    // Create a "fake" inner wall with 0 width to indicate the edge of the walled area.
    let mid = (max_bead_count / 2) as usize;
    let mut innermost_toolpath_location = ret.toolpath_locations[mid - 1];
    let mut innermost_toolpath_width = ret.bead_widths[mid - 1];
    ret.toolpath_locations.insert(mid, innermost_toolpath_location + innermost_toolpath_width / 2);
    ret.bead_widths.insert(mid, 0);

    // Symmetry on both sides. Symmetry is guaranteed since this code is stopped early if the bead_count <= max_bead_count, and never reaches this point then.
    let opposite_bead = (bead_count - (max_bead_count / 2 - 1)) as usize;
    innermost_toolpath_location = ret.toolpath_locations[opposite_bead];
    innermost_toolpath_width = ret.bead_widths[opposite_bead];
    ret.toolpath_locations.insert(opposite_bead, innermost_toolpath_location - innermost_toolpath_width / 2);
    ret.bead_widths.insert(opposite_bead, 0);

    ret
}

/// `BeadingStrategyFactory::makeStrategy`.
#[allow(clippy::too_many_arguments)]
pub fn make_strategy(
    preferred_bead_width_outer: Coord,
    preferred_bead_width_inner: Coord,
    preferred_transition_length: Coord,
    transitioning_angle: f64,
    print_thin_walls: bool,
    min_bead_width: Coord,
    min_feature_size: Coord,
    wall_split_middle_threshold: Ratio,
    wall_add_middle_threshold: Ratio,
    max_bead_count: Coord,
    outer_wall_offset: Coord,
    inward_distributed_center_wall_count: i32,
    minimum_variable_line_ratio: Ratio,
) -> BeadingStrategyPtr {
    let mut ret: BeadingStrategyPtr = Box::new(BeadingStrategy::new_distributed(
        preferred_bead_width_inner,
        preferred_transition_length,
        AngleRadians::new(transitioning_angle),
        wall_split_middle_threshold,
        wall_add_middle_threshold,
        inward_distributed_center_wall_count,
    ));
    ret = Box::new(BeadingStrategy::new_redistribute(preferred_bead_width_outer, minimum_variable_line_ratio, ret));
    if print_thin_walls {
        ret = Box::new(BeadingStrategy::new_widening(ret, min_feature_size, min_bead_width));
    }
    if outer_wall_offset > 0 {
        ret = Box::new(BeadingStrategy::new_outer_wall_inset(outer_wall_offset, ret));
    }
    ret = Box::new(BeadingStrategy::new_limited(max_bead_count, ret));
    ret
}
