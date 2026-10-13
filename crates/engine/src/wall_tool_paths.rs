//! `WallToolPaths.h/.cpp`: generate the variable-width wall toolpaths of an outline (Arachne).

use crate::arachne::SkeletalTrapezoidation;
use crate::beading_strategy;
use crate::geometry::mended_shape::MendedShape;
use crate::geometry::Shape;
use crate::settings::settings::Settings;
use crate::settings::types::LayerIndex;
use crate::utils::coord::{mm2int, Coord};
use crate::utils::extrusion_line::VariableWidthLines;
use crate::utils::polyline_stitcher::stitch_extrusion_lines;
use crate::utils::section_type::SectionType;
use crate::utils::simplify::Simplify;
use clipper::PolyFillType;

pub struct WallToolPaths<'a> {
    /// A copy of the outline we are based on.
    outline: Shape,
    /// Optimal bead width of the outermost wall.
    bead_width_0: Coord,
    /// Optimal bead width of the inner walls.
    bead_width_x: Coord,
    /// The maximum number of walls to generate.
    inset_count: usize,
    /// How far to inset the outer wall. Should only be applied when printing the actual walls, not extra infill/skin/support walls.
    wall_0_inset: Coord,
    /// Whether to enable the "infill" of the thin walls.
    print_thin_walls: bool,
    /// The minimum size of the features that can be widened by the widening beading meta strategy.
    min_feature_size: Coord,
    /// The minimum bead size to use when widening thin model features with the widening beading meta strategy.
    min_bead_width: Coord,
    /// Were the toolpaths generated?
    toolpaths_generated: bool,
    /// The generated toolpaths, binned by inset_idx.
    toolpaths: Vec<VariableWidthLines>,
    /// The inner contour of the generated toolpaths.
    inner_contour: Shape,
    settings: &'a Settings,
    layer_idx: LayerIndex,
    section_type: SectionType,
}

impl<'a> WallToolPaths<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        outline: &Shape,
        bead_width_0: Coord,
        bead_width_x: Coord,
        inset_count: usize,
        wall_0_inset: Coord,
        settings: &'a Settings,
        layer_idx: LayerIndex,
        section_type: SectionType,
    ) -> Self {
        WallToolPaths {
            outline: outline.clone(),
            bead_width_0,
            bead_width_x,
            inset_count,
            wall_0_inset,
            print_thin_walls: settings.get_bool("fill_outline_gaps"),
            min_feature_size: settings.get_coord("min_feature_size"),
            min_bead_width: settings.get_coord("min_bead_width"),
            toolpaths_generated: false,
            toolpaths: Vec::new(),
            inner_contour: Shape::default(),
            settings,
            layer_idx,
            section_type,
        }
    }

    /// Single-width constructor (`WallToolPaths(outline, nominal_bead_width, ...)`).
    pub fn new_uniform(
        outline: &Shape,
        nominal_bead_width: Coord,
        inset_count: usize,
        wall_0_inset: Coord,
        settings: &'a Settings,
        layer_idx: LayerIndex,
        section_type: SectionType,
    ) -> Self {
        Self::new(outline, nominal_bead_width, nominal_bead_width, inset_count, wall_0_inset, settings, layer_idx, section_type)
    }

    /// Generates the Toolpaths.
    pub fn generate(&mut self) -> &Vec<VariableWidthLines> {
        let settings = self.settings;
        let prepared_outline = MendedShape::new(settings, self.section_type, &self.outline);
        if prepared_outline.get_shape().area() <= 0.0 {
            return &self.toolpaths;
        }

        let transitioning_angle = settings.get_angle_radians("wall_transition_angle");
        let discretization_step_size: Coord = mm2int(0.8);
        let wall_transition_length = settings.get_coord("wall_transition_length");

        // When to split the middle wall into two:
        let min_even_wall_line_width = settings.get_f64("min_even_wall_line_width");
        let wall_line_width_0 = settings.get_f64("wall_line_width_0");
        let wall_split_middle_threshold = max_f(1.0, min_f(99.0, 100.0 * (2.0 * min_even_wall_line_width - wall_line_width_0) / wall_line_width_0)) / 100.0;

        // When to add a new middle in between the innermost two walls:
        let min_odd_wall_line_width = settings.get_f64("min_odd_wall_line_width");
        let wall_line_width_x = settings.get_f64("wall_line_width_x");
        let wall_add_middle_threshold = max_f(1.0, min_f(99.0, 100.0 * min_odd_wall_line_width / wall_line_width_x)) / 100.0;

        let wall_distribution_count = settings.get_int("wall_distribution_count");
        let max_bead_count: usize = if self.inset_count < usize::MAX / 2 { 2 * self.inset_count } else { usize::MAX };
        let beading_strat = beading_strategy::make_strategy(
            self.bead_width_0,
            self.bead_width_x,
            wall_transition_length,
            transitioning_angle.value(),
            self.print_thin_walls,
            self.min_bead_width,
            self.min_feature_size,
            wall_split_middle_threshold,
            wall_add_middle_threshold,
            max_bead_count as Coord,
            self.wall_0_inset,
            wall_distribution_count,
            0.5,
        );
        let transition_filter_dist = settings.get_coord("wall_transition_filter_distance");
        let allowed_filter_deviation = settings.get_coord("wall_transition_filter_deviation");
        let mut wall_maker = SkeletalTrapezoidation::new(
            prepared_outline.get_shape(),
            &beading_strat,
            beading_strat.get_transitioning_angle(),
            discretization_step_size,
            transition_filter_dist,
            allowed_filter_deviation,
            wall_transition_length,
            self.layer_idx as i32,
            self.section_type,
        );
        wall_maker.generate_toolpaths(&mut self.toolpaths, false);

        Self::stitch_tool_paths(&mut self.toolpaths, settings);

        Self::remove_small_fill_lines(&mut self.toolpaths);

        Self::simplify_tool_paths(&mut self.toolpaths, settings);

        self.separate_out_inner_contour();

        Self::remove_empty_tool_paths(&mut self.toolpaths);
        self.toolpaths_generated = true;
        &self.toolpaths
    }

    /// Stitch the polylines together and form closed polygons.
    fn stitch_tool_paths(toolpaths: &mut [VariableWidthLines], settings: &Settings) {
        // In 0-width contours, junctions can cause up to 1-line-width gaps. Don't stitch more than 1 line width.
        let stitch_distance = settings.get_coord("wall_line_width_x") - 1;

        for wall_lines in toolpaths.iter_mut() {
            let mut stitched_polylines: VariableWidthLines = Vec::new();
            let mut closed_polygons: VariableWidthLines = Vec::new();
            stitch_extrusion_lines(wall_lines, &mut stitched_polylines, &mut closed_polygons, stitch_distance, 10);
            *wall_lines = stitched_polylines; // replace input toolpaths with stitched polylines

            for mut wall_polygon in closed_polygons {
                if wall_polygon.junctions.is_empty() {
                    continue;
                }
                wall_polygon.is_closed = true;
                wall_lines.push(wall_polygon); // add stitched polygons to result
            }
        }
    }

    /// Remove polylines shorter than half the smallest line width along that polyline, if that polyline isn't part of an outer wall.
    fn remove_small_fill_lines(toolpaths: &mut [VariableWidthLines]) {
        for inset in toolpaths.iter_mut() {
            let mut line_idx: usize = 0;
            while line_idx < inset.len() {
                let line = &inset[line_idx];
                if !line.is_outer_wall() {
                    let mut min_width = Coord::MAX;
                    for j in &line.junctions {
                        min_width = min_width.min(j.w);
                    }
                    if line.is_odd && !line.is_closed && line.shorter_than(min_width / 2) {
                        // remove line
                        inset.swap_remove(line_idx);
                        line_idx = line_idx.wrapping_sub(1); // reconsider the current position
                    }
                }
                line_idx = line_idx.wrapping_add(1);
            }
        }
    }

    /// Simplifies the variable-width toolpaths by calling the simplify on every line in the toolpath.
    fn simplify_tool_paths(toolpaths: &mut [VariableWidthLines], settings: &Settings) {
        let simplifier = Simplify::from_settings(settings);
        for toolpath in toolpaths.iter_mut() {
            let lines = std::mem::take(toolpath);
            *toolpath = lines
                .iter()
                .map(|line| {
                    let mut line_ = if line.is_closed { simplifier.extrusion_polygon(line) } else { simplifier.extrusion_polyline(line) };
                    if line_.is_closed && line_.junctions.len() >= 2 && line_.junctions[0] != *line_.junctions.last().unwrap() {
                        let front = line_.junctions[0];
                        line_.junctions.push(front);
                    }
                    line_
                })
                .filter(|line| !line.junctions.is_empty())
                .collect();
        }
    }

    /// Gets the toolpaths, if this called before `generate()` it will first generate the toolpaths.
    pub fn get_tool_paths(&mut self) -> &Vec<VariableWidthLines> {
        if !self.toolpaths_generated {
            return self.generate();
        }
        &self.toolpaths
    }

    /// Alternate 'get', for when the vector that'll be inserted in already exists.
    pub fn push_tool_paths(&mut self, paths: &mut Vec<VariableWidthLines>) {
        if !self.toolpaths_generated {
            self.generate();
        }
        paths.extend(self.toolpaths.iter().cloned());
    }

    /// Compute the inner contour of the walls. This contour indicates where the walled area ends
    /// and its infill begins.
    fn separate_out_inner_contour(&mut self) {
        // We'll remove all 0-width paths from the original toolpaths and store them separately as polygons.
        let mut actual_toolpaths: Vec<VariableWidthLines> = Vec::with_capacity(self.toolpaths.len());
        self.inner_contour.clear();
        for inset in &self.toolpaths {
            if inset.is_empty() {
                continue;
            }
            let mut is_contour = false;
            for line in inset {
                if let Some(j) = line.junctions.first() {
                    is_contour = j.w == 0;
                }
            }

            if is_contour {
                for line in inset {
                    if line.is_odd {
                        continue; // odd lines don't contribute to the contour
                    } else if line.is_closed {
                        // sometimes an very small even polygonal wall is not stitched into a polygon
                        self.inner_contour.push(line.to_polygon());
                    }
                }
            } else {
                actual_toolpaths.push(inset.clone());
            }
        }
        if !actual_toolpaths.is_empty() {
            self.toolpaths = actual_toolpaths; // Filtered out the 0-width paths.
        } else {
            self.toolpaths.clear();
        }

        // The output walls from the skeletal trapezoidation have no known winding order, especially if they are joined together from polylines.
        // To get a correct shape, we need to make the outside contour positive and any holes inside negative.
        // This can be done by applying the even-odd rule to the shape. This rule is not sensitive to the winding order of the polygon.
        self.inner_contour = self.inner_contour.process_even_odd(PolyFillType::EvenOdd);
    }

    /// Gets the inner contour of the area which is inside of the generated tool paths.
    pub fn get_inner_contour(&mut self) -> &Shape {
        if !self.toolpaths_generated && self.inset_count > 0 {
            self.generate();
        } else if self.inset_count == 0 {
            return &self.outline;
        }
        &self.inner_contour
    }

    /// Removes empty paths from the toolpaths. Returns true if there are no toolpaths.
    pub fn remove_empty_tool_paths(toolpaths: &mut Vec<VariableWidthLines>) -> bool {
        for toolpath in toolpaths.iter_mut() {
            toolpath.retain(|line| !line.junctions.is_empty());
        }
        toolpaths.retain(|lines| !lines.is_empty());
        toolpaths.is_empty()
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

#[inline]
fn min_f(a: f64, b: f64) -> f64 {
    if b < a {
        b
    } else {
        a
    }
}
