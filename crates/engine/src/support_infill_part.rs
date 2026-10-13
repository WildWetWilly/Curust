//! `SupportInfillPart.h/.cpp`.

use crate::geometry::{Shape, SingleShape};
use crate::utils::aabb::AABB;
use crate::utils::coord::Coord;
use crate::utils::extrusion_line::VariableWidthLines;

/// A part of the support infill area of a layer, with its walls and infill areas.
#[derive(Clone, Debug)]
pub struct SupportInfillPart {
    /// the outline of the support infill area
    pub outline: SingleShape,
    /// the boundary box of the infill area
    pub outline_boundary_box: AABB,
    /// The support line width
    pub support_line_width: Coord,
    /// The number of insets need to be generated from the outline. This is not the actual insets that will be generated.
    pub inset_count_to_generate: i32,
    /// a list of separated sub-areas which requires different infill densities and combined thicknesses
    ///   for infill_areas[x][n], x means the density level and n means the thickness
    pub infill_area_per_combine_per_density: Vec<Vec<Shape>>,
    /// Any walls go here, not in the areas, where they could be combined vertically (don't combine walls). Binned by inset_idx.
    pub wall_toolpaths: Vec<VariableWidthLines>,
    /// The distance between support infill lines. 0 means use the default line distance instead.
    pub custom_line_distance: Coord,
    /// Whether to use the config for fractional layers (for the top of the support).
    pub use_fractional_config: bool,
}

impl SupportInfillPart {
    pub fn new(outline: &SingleShape, support_line_width: Coord, use_fractional_config: bool, inset_count_to_generate: i32, custom_line_distance: Coord) -> Self {
        SupportInfillPart {
            outline: outline.clone(),
            outline_boundary_box: AABB::from_shape(&outline.0),
            support_line_width,
            inset_count_to_generate,
            infill_area_per_combine_per_density: Vec::new(),
            wall_toolpaths: Vec::new(),
            custom_line_distance,
            use_fractional_config,
        }
    }

    pub fn get_infill_area(&self) -> &Shape {
        // if there is no wall, we use the original outline as the infill area
        &self.outline.0
    }
}
