//! `WallsComputation.h/.cpp`: generate the walls (insets) of all parts of a layer.

use crate::geometry::SingleShape;
use crate::settings::settings::Settings;
use crate::settings::types::LayerIndex;
use crate::slice_data_storage::{SliceLayer, SliceLayerPart};
use crate::utils::coord::Coord;
use crate::utils::section_type::SectionType;
use crate::utils::simplify::Simplify;
use crate::wall_tool_paths::WallToolPaths;
use clipper::JoinType;

pub struct WallsComputation<'a> {
    settings: &'a Settings,
    layer_nr: LayerIndex,
}

impl<'a> WallsComputation<'a> {
    pub fn new(settings: &'a Settings, layer_nr: LayerIndex) -> Self {
        WallsComputation { settings, layer_nr }
    }

    /// Generates the walls / inner area for a single part.
    fn generate_walls_part(&self, part: &mut SliceLayerPart, section_type: SectionType) {
        let mut wall_count = self.settings.get_usize("wall_line_count");
        if wall_count == 0 {
            // Early out if no walls are to be generated
            part.print_outline = part.outline.0.clone();
            part.inner_area = part.outline.0.clone();
            return;
        }

        let spiralize = self.settings.get_bool("magic_spiralize");
        let alternate = (((self.layer_nr % 2) + 2) % 2) as usize;
        if spiralize && self.layer_nr < self.settings.get_usize("initial_bottom_layers") as LayerIndex && alternate == 1 {
            // Add extra insets every 2 layers when spiralizing. This makes bottoms of cups watertight.
            wall_count += 5;
        }
        if self.settings.get_bool("alternate_extra_perimeter") {
            wall_count += alternate;
        }

        let first_layer = self.layer_nr == 0;
        let line_width_0_factor =
            if first_layer { self.settings.get_extruder_train("wall_0_extruder_nr").settings.get_ratio("initial_layer_line_width_factor") } else { 1.0 };
        let line_width_0 = (self.settings.get_coord("wall_line_width_0") as f64 * line_width_0_factor) as Coord;
        let wall_0_inset = self.settings.get_coord("wall_0_inset");

        let line_width_x_factor =
            if first_layer { self.settings.get_extruder_train("wall_x_extruder_nr").settings.get_ratio("initial_layer_line_width_factor") } else { 1.0 };
        let line_width_x = (self.settings.get_coord("wall_line_width_x") as f64 * line_width_x_factor) as Coord;

        // When spiralizing, generate the spiral insets using simple offsets instead of generating toolpaths
        if spiralize {
            let recompute_outline_based_on_outer_wall = self.settings.get_bool("support_enable") && !self.settings.get_bool("fill_outline_gaps");

            self.generate_spiral_insets(part, line_width_0, wall_0_inset, recompute_outline_based_on_outer_wall);
            if self.layer_nr <= self.settings.get_usize("initial_bottom_layers") as LayerIndex {
                let mut wall_tool_paths =
                    WallToolPaths::new(&part.outline.0, line_width_0, line_width_x, wall_count, wall_0_inset, self.settings, self.layer_nr, section_type);
                part.wall_toolpaths = wall_tool_paths.get_tool_paths().clone();
                part.inner_area = wall_tool_paths.get_inner_contour().clone();
            }
        } else {
            let mut wall_tool_paths =
                WallToolPaths::new(&part.outline.0, line_width_0, line_width_x, wall_count, wall_0_inset, self.settings, self.layer_nr, section_type);
            part.wall_toolpaths = wall_tool_paths.get_tool_paths().clone();
            part.inner_area = wall_tool_paths.get_inner_contour().clone();
        }

        part.outline = SingleShape(Simplify::from_settings(self.settings).shape(&part.outline.0));
        part.print_outline = part.outline.0.clone();
    }

    /// Generates the walls / inner area for all parts in a layer.
    pub fn generate_walls(&self, layer: &mut SliceLayer, section: SectionType) {
        for part in layer.parts.iter_mut() {
            self.generate_walls_part(part, section);
        }

        // Remove the parts which did not generate a wall. As these parts are too small to print,
        //  and later code can now assume that there is always minimal 1 wall line.
        let check_wall_and_spiral = self.settings.get_usize("wall_line_count") >= 1 && !self.settings.get_bool("fill_outline_gaps");
        layer.parts.retain(|part| {
            !((check_wall_and_spiral && part.wall_toolpaths.is_empty() && part.spiral_wall.is_empty()) || part.outline.0.is_empty() || part.print_outline.is_empty())
        });
    }

    fn generate_spiral_insets(&self, part: &mut SliceLayerPart, line_width_0: Coord, wall_0_inset: Coord, recompute_outline_based_on_outer_wall: bool) {
        part.spiral_wall = part.outline.0.offset(-line_width_0 / 2 - wall_0_inset, JoinType::Miter, 1.2);

        // Optimize the wall. This prevents buffer underruns in the printer firmware, and reduces processing time in CuraEngine.
        let train_wall = self.settings.get_extruder_train("wall_0_extruder_nr");
        part.spiral_wall = Simplify::from_settings(&train_wall.settings).shape(&part.spiral_wall);
        part.spiral_wall.remove_degenerate_verts();
        if recompute_outline_based_on_outer_wall {
            part.print_outline = part.spiral_wall.offset(line_width_0 / 2, JoinType::Square, 1.2);
        } else {
            part.print_outline = part.outline.0.clone();
        }
    }
}

