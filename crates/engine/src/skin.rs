//! `skin.h/.cpp`: computation of the skin (top/bottom), roofing/flooring and infill areas.
//!
//! Like the C++ code this runs in parallel over layers: each instance only writes to the parts of
//! its own layer, while it reads the outlines of other layers. The mesh is therefore accessed
//! through a raw pointer, mirroring the C++ reference member.

use crate::geometry::Shape;
use crate::settings::enums::EFillMethod;
use crate::settings::types::{AngleRadians, LayerIndex};
use crate::slice_data_storage::{SkinPart, SliceDataStorage, SliceLayerPart, SliceMeshStorage};
use crate::utils::coord::Coord;
use crate::utils::extrusion_line::VariableWidthLines;
use crate::utils::math::round_divide;
use crate::utils::section_type::SectionType;
use crate::utils::simplify::Simplify;
use crate::wall_tool_paths::WallToolPaths;

const MIN_AREA_SIZE: f64 = 0.4 * 0.4;

pub struct SkinInfillAreaComputation {
    /// The index of the layer for which to generate the skins and infill.
    layer_nr: LayerIndex,
    /// The storage where the layer outline information (input) is stored and where the skin insets and fill areas (output) are stored.
    mesh: *mut SliceMeshStorage,
    /// The number of layers of bottom skin
    bottom_layer_count: usize,
    /// Whether to make bottom skin for the initial layer
    initial_bottom_layer_count: usize,
    /// The number of layers of top skin
    top_layer_count: usize,
    /// The line width of the skin.
    #[allow(dead_code)]
    skin_line_width: Coord,
    /// A heuristic which assumes there will be no small gaps between bottom and top skin with a z size smaller than the skin size itself
    no_small_gaps_heuristic: bool,
    /// Whether to process infill, i.e. whether there's a positive infill density or there are infill meshes modifying this mesh.
    process_infill: bool,
    /// The top skin removal width, to remove thin strips of skin along nearly-vertical walls.
    top_skin_preshrink: Coord,
    /// The bottom skin removal width, to remove thin strips of skin along nearly-vertical walls.
    bottom_skin_preshrink: Coord,
    /// The distance by which the top skins should be larger than the original top skins.
    top_skin_expand_distance: Coord,
    /// The distance by which the bottom skins should be larger than the original bottom skins.
    bottom_skin_expand_distance: Coord,
}

/// `infill_wall_toolpaths`-producing helper `Infill::generateWallToolPaths`.
#[allow(clippy::too_many_arguments)]
pub fn generate_infill_wall_tool_paths(
    toolpaths: &mut Vec<VariableWidthLines>,
    outer_contour: &Shape,
    wall_line_count: usize,
    line_width: Coord,
    settings: &crate::settings::settings::Settings,
    layer_idx: LayerIndex,
    section_type: SectionType,
) -> Shape {
    if wall_line_count > 0 {
        const WALL_0_INSET: Coord = 0; // Don't apply any outer wall inset for these. That's just for the outer wall.
        let mut wall_toolpaths = WallToolPaths::new_uniform(outer_contour, line_width, wall_line_count, WALL_0_INSET, settings, layer_idx, section_type);
        wall_toolpaths.push_tool_paths(toolpaths);
        wall_toolpaths.get_inner_contour().clone()
    } else {
        outer_contour.clone()
    }
}

impl SkinInfillAreaComputation {
    /// Compute the skin line width, which might be different for the first layer.
    fn get_skin_line_width(mesh: &SliceMeshStorage, layer_nr: LayerIndex) -> Coord {
        let mut skin_line_width = mesh.settings().get_coord("skin_line_width");
        if layer_nr == 0 {
            let train_skin = mesh.settings().get_extruder_train("top_bottom_extruder_nr");
            skin_line_width = (skin_line_width as f64 * train_skin.settings.get_ratio("initial_layer_line_width_factor")) as Coord;
        }
        skin_line_width
    }

    pub fn new(layer_nr: LayerIndex, mesh: &mut SliceMeshStorage, process_infill: bool) -> Self {
        let settings = mesh.settings();
        SkinInfillAreaComputation {
            layer_nr,
            bottom_layer_count: settings.get_usize("bottom_layers"),
            initial_bottom_layer_count: settings.get_usize("initial_bottom_layers"),
            top_layer_count: settings.get_usize("top_layers"),
            skin_line_width: Self::get_skin_line_width(mesh, layer_nr),
            no_small_gaps_heuristic: settings.get_bool("skin_no_small_gaps_heuristic"),
            process_infill,
            top_skin_preshrink: settings.get_coord("top_skin_preshrink"),
            bottom_skin_preshrink: settings.get_coord("bottom_skin_preshrink"),
            top_skin_expand_distance: settings.get_coord("top_skin_expand_distance"),
            bottom_skin_expand_distance: settings.get_coord("bottom_skin_expand_distance"),
            mesh: mesh as *mut SliceMeshStorage,
        }
    }

    #[inline]
    fn mesh(&self) -> &SliceMeshStorage {
        // SAFETY: the mesh outlives the computation; see the module documentation.
        unsafe { &*self.mesh }
    }

    #[inline]
    #[allow(clippy::mut_from_ref)]
    fn part_mut(&self, part_idx: usize) -> &mut SliceLayerPart {
        // SAFETY: only this computation writes to the parts of its own layer.
        unsafe { &mut (&mut (*self.mesh).layers)[self.layer_nr as usize].parts[part_idx] }
    }

    #[inline]
    fn part_ref(&self, part_idx: usize) -> &SliceLayerPart {
        &self.mesh().layers[self.layer_nr as usize].parts[part_idx]
    }

    /// Get the outlines of the parts on another layer which may overlap with the given part.
    fn get_outline_on_layer(&self, part_here: &SliceLayerPart, layer2_nr: LayerIndex) -> Shape {
        let mut result = Shape::default();
        let mesh = self.mesh();
        if layer2_nr >= mesh.layers.len() as LayerIndex {
            return result;
        }
        let layer2 = &mesh.layers[layer2_nr as usize];
        for part2 in &layer2.parts {
            if part_here.boundary_box.hit(&part2.boundary_box) {
                result.push_all(&part2.outline.0);
            }
        }
        result
    }

    /// Generate the skin areas and its insets.
    pub fn generate_skins_and_infill(&mut self) {
        self.generate_skin_and_infill_areas_layer();

        let n_parts = self.mesh().layers[self.layer_nr as usize].parts.len();
        for part_idx in 0..n_parts {
            self.generate_skin_roofing_flooring_fill(part_idx);

            self.generate_top_and_bottom_most_surfaces(part_idx);
        }
    }

    fn generate_skin_and_infill_areas_layer(&mut self) {
        if !self.process_infill && self.bottom_layer_count == 0 && self.top_layer_count == 0 {
            return;
        }

        let n_parts = self.mesh().layers[self.layer_nr as usize].parts.len();
        for part_idx in 0..n_parts {
            self.generate_skin_and_infill_areas(part_idx);
        }
    }

    fn generate_skin_and_infill_areas(&mut self, part_idx: usize) {
        let part = self.part_ref(part_idx);
        // Make a copy of the outline which we later intersect and union with the resized skins to ensure the resized skin isn't too large or removed completely.
        let mut top_skin = Shape::default();
        if self.top_layer_count > 0 {
            top_skin = part.inner_area.clone();
        }
        let mut bottom_skin = Shape::default();
        if self.bottom_layer_count > 0 || self.layer_nr < self.initial_bottom_layer_count as LayerIndex {
            bottom_skin = part.inner_area.clone();
        }

        self.calculate_bottom_skin(part, &mut bottom_skin);
        self.calculate_top_skin(part, &mut top_skin);

        let inner_area = part.inner_area.clone();
        self.apply_skin_expansion(&inner_area, &mut top_skin, &mut bottom_skin);

        // Now combine the resized top skin and bottom skin.
        let mut skin = top_skin.union_polygons_other(&bottom_skin);

        skin.remove_small_areas(MIN_AREA_SIZE, false);
        // Create infill area irrespective if the infill is to be generated or not(would be used for bridging).
        let infill_area = inner_area.difference(&skin);
        let skin_parts = skin.split_into_parts(false);
        let part = self.part_mut(part_idx);
        part.infill_area = infill_area;
        if self.process_infill {
            // process infill when infill density > 0
            // or when other infill meshes want to modify this infill
            Self::generate_infill(part);
        }

        for skin_area_part in skin_parts {
            if skin_area_part.is_empty() {
                continue;
            }
            part.skin_parts.push(SkinPart { outline: skin_area_part, ..Default::default() });
        }
    }

    fn calculate_bottom_skin(&self, part: &SliceLayerPart, downskin: &mut Shape) {
        if self.bottom_layer_count == 0 && self.initial_bottom_layer_count == 0 {
            return; // downskin remains empty
        }
        if self.layer_nr < self.initial_bottom_layer_count as LayerIndex {
            return; // don't subtract anything form the downskin
        }
        let bottom_check_start_layer_idx: LayerIndex = (0 as LayerIndex).max(self.layer_nr - self.bottom_layer_count as LayerIndex);
        let mut not_air = self.get_outline_on_layer(part, bottom_check_start_layer_idx);
        if !self.no_small_gaps_heuristic {
            for downskin_layer_nr in bottom_check_start_layer_idx + 1..self.layer_nr {
                not_air = not_air.intersection(&self.get_outline_on_layer(part, downskin_layer_nr));
            }
        }
        let min_infill_area = self.mesh().settings().get_f64("min_infill_area");
        if min_infill_area > 0.0 {
            not_air.remove_small_areas(min_infill_area, false);
        }
        *downskin = downskin.difference(&not_air); // skin overlaps with the walls
    }

    fn calculate_top_skin(&self, part: &SliceLayerPart, upskin: &mut Shape) {
        let n_layers = self.mesh().layers.len() as LayerIndex;
        if self.layer_nr > n_layers - self.top_layer_count as LayerIndex || self.top_layer_count == 0 {
            // If we're in the very top layers (less than top_layer_count from the top of the mesh) everything will be top skin anyway, so no need to generate infill. Just take the
            // original inner contour. If top_layer_count is 0, no need to calculate anything either.
            return;
        }

        let mut not_air = self.get_outline_on_layer(part, self.layer_nr + self.top_layer_count as LayerIndex);
        if !self.no_small_gaps_heuristic {
            for upskin_layer_nr in self.layer_nr + 1..self.layer_nr + self.top_layer_count as LayerIndex {
                not_air = not_air.intersection(&self.get_outline_on_layer(part, upskin_layer_nr));
            }
        }

        let min_infill_area = self.mesh().settings().get_f64("min_infill_area");
        if min_infill_area > 0.0 {
            not_air.remove_small_areas(min_infill_area, false);
        }

        *upskin = upskin.difference(&not_air); // skin overlaps with the walls
    }

    /// Apply skin expansion: expand skins into infill area where the skin is broad enough.
    fn apply_skin_expansion(&self, original_outline: &Shape, upskin: &mut Shape, downskin: &mut Shape) {
        let min_width = self.mesh().settings().get_coord("min_skin_width_for_expansion") / 2;

        // Expand some areas of the skin for Skin Expand Distance.
        if min_width > 0 {
            // This performs an opening operation by first insetting by the minimum width, then offsetting with the same width.
            // The expansion is only applied to that opened shape.
            if self.bottom_skin_expand_distance != 0 {
                let expanded = downskin.offset_default(-min_width).offset_default(min_width + self.bottom_skin_expand_distance);
                // And then re-joined with the original part that was not offset, to retain parts smaller than min_width.
                *downskin = downskin.union_polygons_other(&expanded);
            }
            if self.top_skin_expand_distance != 0 {
                let expanded = upskin.offset_default(-min_width).offset_default(min_width + self.top_skin_expand_distance);
                *upskin = upskin.union_polygons_other(&expanded);
            }
        } else {
            // No need to pay attention to minimum width. Just expand.
            if self.bottom_skin_expand_distance != 0 {
                *downskin = downskin.offset_default(self.bottom_skin_expand_distance);
            }
            if self.top_skin_expand_distance != 0 {
                *upskin = upskin.offset_default(self.top_skin_expand_distance);
            }
        }

        let mut should_bottom_be_clipped = self.bottom_skin_expand_distance > 0;
        let mut should_top_be_clipped = self.top_skin_expand_distance > 0;

        // Set miter limit for morphological open in order to fill in "narrow point" features of
        // polygons due to offsetting artifacts.
        const MITER_LIMIT: f64 = 10000000.0;
        // Remove thin pieces of support for Skin Removal Width.
        if self.bottom_skin_preshrink > 0 || (min_width == 0 && self.bottom_skin_expand_distance != 0) {
            *downskin = downskin
                .offset(-self.bottom_skin_preshrink / 2, clipper::JoinType::Miter, MITER_LIMIT)
                .offset(self.bottom_skin_preshrink / 2, clipper::JoinType::Miter, MITER_LIMIT)
                .intersection(downskin);
            should_bottom_be_clipped = true; // Rounding errors can lead to propagation of errors. This could mean that skin goes beyond the original outline
        }
        if self.top_skin_preshrink > 0 || (min_width == 0 && self.top_skin_expand_distance != 0) {
            *upskin = upskin
                .offset(-self.top_skin_preshrink / 2, clipper::JoinType::Miter, MITER_LIMIT)
                .offset(self.top_skin_preshrink / 2, clipper::JoinType::Miter, MITER_LIMIT);
            should_top_be_clipped = true; // Rounding errors can lead to propagation of errors. This could mean that skin goes beyond the original outline
        }

        if should_bottom_be_clipped {
            *downskin = downskin.intersection(original_outline);
        }
        if should_top_be_clipped {
            *upskin = upskin.intersection(original_outline);
        }
    }

    /// Generate infill everywhere where there wasn't any skin.
    fn generate_infill(part: &mut SliceLayerPart) {
        part.infill_area.remove_small_areas(MIN_AREA_SIZE, false);
    }

    /// Generate the roofing, flooring and skin fill areas of the skin parts.
    fn generate_skin_roofing_flooring_fill(&mut self, part_idx: usize) {
        let settings = self.mesh().settings();
        let roofing_layer_count = settings.get_usize("roofing_layer_count").min(settings.get_usize("top_layers"));
        let flooring_layer_count = settings.get_usize("flooring_layer_count").min(settings.get_usize("bottom_layers"));
        let skin_overlap = settings.get_coord("skin_overlap_mm");
        let roofing_expansion = settings.get_coord("roofing_expansion");

        const EPSILON: Coord = 5;
        let slice_data = SliceDataStorage::new();
        let build_plate = slice_data.get_raw_machine_border();

        let part = self.part_ref(part_idx);
        let filled_area_above = self.generate_filled_area_above(part, roofing_layer_count);
        let filled_area_below = match self.generate_filled_area_below(part, flooring_layer_count) {
            Some(s) => s,
            None => build_plate.offset_default(EPSILON),
        };

        let part = self.part_mut(part_idx);
        for skin_part in part.skin_parts.iter_mut() {
            let below_inside = skin_part.outline.0.intersection(&filled_area_below);
            let above_inside = skin_part.outline.0.intersection(&filled_area_above);

            skin_part.roofing_fill = below_inside.difference(&filled_area_above).offset_default(roofing_expansion).intersection(&below_inside);
            skin_part.flooring_fill = above_inside.difference(&filled_area_below);
            skin_part.skin_fill = skin_part.outline.0.difference(&skin_part.roofing_fill).intersection(&filled_area_below);

            // We remove offsets areas from roofing and flooring anywhere they overlap with skin_fill.
            // Otherwise, adjacent skin_fill and roofing/flooring would have doubled offset areas. Since they both offset into each other.
            skin_part.skin_fill = skin_part.skin_fill.offset_default(skin_overlap).difference(&skin_part.roofing_fill).difference(&skin_part.flooring_fill);
            skin_part.roofing_fill = skin_part.roofing_fill.offset_default(skin_overlap);
            skin_part.flooring_fill = skin_part.flooring_fill.offset_default(skin_overlap).difference(&skin_part.roofing_fill);
        }
    }

    fn generate_top_and_bottom_most_surfaces(&mut self, part_idx: usize) {
        let part = self.part_ref(part_idx);
        let outline_above = self.get_outline_on_layer(part, self.layer_nr + 1);
        let top_most_surface = part.outline.0.difference(&outline_above);

        let bottom_most_surface = if self.layer_nr > 0 {
            let outline_below = self.get_outline_on_layer(part, self.layer_nr + 1);
            part.outline.0.difference(&outline_below)
        } else {
            part.outline.0.clone()
        };
        let part = self.part_mut(part_idx);
        part.top_most_surface = top_most_surface;
        part.bottom_most_surface = bottom_most_surface;
    }

    /// Helper function to calculate the areas that are above the current layer (used for roofing).
    fn generate_filled_area_above(&self, part: &SliceLayerPart, roofing_layer_count: usize) -> Shape {
        let mut filled_area_above = self.get_outline_on_layer(part, self.layer_nr + roofing_layer_count as LayerIndex);
        if !self.no_small_gaps_heuristic {
            for layer_nr_above in self.layer_nr + 1..self.layer_nr + roofing_layer_count as LayerIndex {
                let outlines_above = self.get_outline_on_layer(part, layer_nr_above);
                filled_area_above = filled_area_above.intersection(&outlines_above);
            }
        }
        if self.layer_nr > 0 {
            // if the skin has air below it then cutting it into regions could cause a region
            // to be wholely or partly above air and it may not be printable so restrict
            // the regions that have air above (the visible regions) to not include any area that
            // has air below (fixes https://github.com/Ultimaker/Cura/issues/2656)

            // set air_below to the skin area for the current layer that has air below it
            let air_below = self.get_outline_on_layer(part, self.layer_nr).difference(&self.get_outline_on_layer(part, self.layer_nr - 1));

            if !air_below.is_empty() {
                // add the polygons that have air below to the no air above polygons
                filled_area_above = filled_area_above.union_polygons_other(&air_below);
            }
        }
        filled_area_above
    }

    /// Helper function to calculate the areas that are below the current layer (used for flooring).
    fn generate_filled_area_below(&self, part: &SliceLayerPart, flooring_layer_count: usize) -> Option<Shape> {
        if self.layer_nr < flooring_layer_count as LayerIndex {
            return None;
        }

        let lowest_flooring_layer = self.layer_nr - flooring_layer_count as LayerIndex;
        let mut filled_area_below = self.get_outline_on_layer(part, lowest_flooring_layer);

        if !self.no_small_gaps_heuristic {
            let next_lowest_flooring_layer = lowest_flooring_layer + 1;
            for layer_nr_below in next_lowest_flooring_layer..self.layer_nr {
                let outlines_below = self.get_outline_on_layer(part, layer_nr_below);
                filled_area_below = filled_area_below.intersection(&outlines_below);
            }
        }
        Some(filled_area_below)
    }

    /// Generate infill areas which cause support for the layers above (infill_support_enabled).
    pub fn generate_infill_support(mesh: &mut SliceMeshStorage) {
        let settings = mesh.settings();
        let layer_height = settings.get_coord("layer_height");
        let support_angle: AngleRadians = settings.get_angle_radians("infill_support_angle");
        let tan_angle = support_angle.value().tan() - 0.01; // The X/Y component of the support angle. 0.01 to make 90 degrees work too.
        let max_dist_from_lower_layer = (tan_angle * layer_height as f64) as Coord; // Maximum horizontal distance that can be bridged.

        let mut layer_idx = mesh.layers.len() as i64 - 2;
        while layer_idx >= 0 {
            let (lower, upper) = mesh.layers.split_at_mut(layer_idx as usize + 1);
            let layer = &mut lower[layer_idx as usize];
            let layer_above = &upper[0];

            let mut inside_above = Shape::default();
            let mut infill_above = Shape::default();
            for part_above in &layer_above.parts {
                inside_above.push_all(&part_above.infill_area);
                infill_above.push_all(part_above.get_own_infill_area());
            }

            for part in layer.parts.iter_mut() {
                let infill_area = &part.infill_area;
                if infill_area.is_empty() {
                    continue;
                }

                let unsupported = infill_area.offset_default(-max_dist_from_lower_layer);
                let basic_overhang = unsupported.difference(&inside_above);
                let overhang_extented = basic_overhang.offset_default(max_dist_from_lower_layer + 50); // +50 for easier joining with support from layer above
                let full_overhang = overhang_extented.difference(&inside_above);
                let infill_support = infill_above.union_polygons_other(&full_overhang);

                part.infill_area_own = Some(infill_support.intersection(part.get_own_infill_area()));
            }
            layer_idx -= 1;
        }
    }

    /// Limit the infill areas to places where they support internal overhangs, generating the infill areas per density.
    pub fn generate_gradual_infill(mesh: &mut SliceMeshStorage) {
        let settings = mesh.settings();
        // no early-out for this function; it needs to initialize the [infill_area_per_combine_per_density]
        let mut layer_skip_count: f64 = 8.0; // skip every so many layers as to ignore small gaps in the model making computation more easy
        if !settings.get_bool("skin_no_small_gaps_heuristic") {
            layer_skip_count = 1.0;
        }
        let gradual_infill_step_height = settings.get_coord("gradual_infill_step_height");
        // The difference in layer count between consecutive density infill areas
        let gradual_infill_step_layer_count = round_divide(gradual_infill_step_height as u64, settings.get_coord("layer_height") as u64) as usize;

        // make gradual_infill_step_height divisible by layer_skip_count
        let c = (gradual_infill_step_layer_count as f64 / layer_skip_count).ceil();
        let n_skip_steps_per_gradual_step = if 1.0 < c { c } else { 1.0 }; // only decrease layer_skip_count to make it a divisor of gradual_infill_step_layer_count
        layer_skip_count = gradual_infill_step_layer_count as f64 / n_skip_steps_per_gradual_step;
        let max_infill_steps = settings.get_usize("gradual_infill_steps");

        let mesh_min_layer = settings.get_usize("initial_bottom_layers") as LayerIndex;
        let mesh_max_layer = (mesh.layers.len().wrapping_sub(1).wrapping_sub(settings.get_usize("top_layers"))) as i64 as LayerIndex;

        let simplifier = Simplify::from_settings(&settings.get_extruder_train("infill_extruder_nr").settings);

        let infill_wall_count = settings.get_usize("infill_wall_line_count");
        let infill_wall_width = settings.get_coord("infill_line_width");
        let is_connected = settings.get_bool("zig_zaggify_infill") || settings.get_enum::<EFillMethod>("infill_pattern") == EFillMethod::ZIG_ZAG;
        let n_layers = mesh.layers.len();
        for layer_idx in 0..n_layers as LayerIndex {
            // loop also over layers which don't contain infill cause of bottom_ and top_layer to initialize their infill_area_per_combine_per_density
            let n_parts = mesh.layers[layer_idx as usize].parts.len();
            for part_idx in 0..n_parts {
                let mut infill_wall_toolpaths = std::mem::take(&mut mesh.layers[layer_idx as usize].parts[part_idx].infill_wall_toolpaths);
                let own = mesh.layers[layer_idx as usize].parts[part_idx].get_own_infill_area().clone();
                let infill_area = generate_infill_wall_tool_paths(
                    &mut infill_wall_toolpaths,
                    &own,
                    infill_wall_count,
                    infill_wall_width,
                    settings,
                    layer_idx,
                    SectionType::SKIN,
                );
                mesh.layers[layer_idx as usize].parts[part_idx].infill_wall_toolpaths = infill_wall_toolpaths;

                if infill_area.is_empty() || layer_idx < mesh_min_layer || layer_idx > mesh_max_layer {
                    // initialize infill_area_per_combine_per_density empty
                    let part = &mut mesh.layers[layer_idx as usize].parts[part_idx];
                    part.infill_area_per_combine_per_density.push(Vec::new()); // create a new infill_area_per_combine
                    part.infill_area_per_combine_per_density.last_mut().unwrap().push(Shape::default()); // put empty infill area in the newly constructed infill_area_per_combine
                    continue;
                }
                let part_bb = mesh.layers[layer_idx as usize].parts[part_idx].boundary_box;
                let mut less_dense_infill = infill_area.clone(); // one step less dense with each infill_step
                let mut sum_more_dense = Shape::default(); // NOTE: Only used for zig-zag or connected fills.
                let mut new_densities: Vec<Vec<Shape>> = Vec::new();
                for infill_step in 0..max_infill_steps {
                    let min_layer: LayerIndex =
                        layer_idx + (infill_step * gradual_infill_step_layer_count) as LayerIndex + (layer_skip_count as usize) as LayerIndex;
                    let max_layer: LayerIndex = layer_idx + ((infill_step + 1) * gradual_infill_step_layer_count) as LayerIndex;

                    let mut upper_layer_idx = min_layer as f64;
                    while upper_layer_idx <= max_layer as f64 {
                        if upper_layer_idx >= n_layers as f64 {
                            less_dense_infill.clear();
                            break;
                        }
                        let upper_layer = &mesh.layers[upper_layer_idx as usize];
                        let mut relevent_upper_polygons = Shape::default();
                        for upper_layer_part in &upper_layer.parts {
                            if !upper_layer_part.boundary_box.hit(&part_bb) {
                                continue;
                            }
                            relevent_upper_polygons.push_all(upper_layer_part.get_own_infill_area());
                        }
                        less_dense_infill = less_dense_infill.intersection(&relevent_upper_polygons);
                        upper_layer_idx += layer_skip_count;
                    }
                    if less_dense_infill.is_empty() {
                        break;
                    }
                    // add new infill_area_per_combine for the current density
                    let more_dense_infill = infill_area.difference(&less_dense_infill);
                    new_densities.push(vec![simplifier.shape(&more_dense_infill.difference(&sum_more_dense))]);
                    if is_connected {
                        sum_more_dense = sum_more_dense.union_polygons_other(&more_dense_infill);
                    }
                }
                new_densities.push(vec![simplifier.shape(&infill_area.difference(&sum_more_dense))]);
                let part = &mut mesh.layers[layer_idx as usize].parts[part_idx];
                part.infill_area_per_combine_per_density.extend(new_densities);
                part.infill_area_own = None; // clear infill_area_own, it's not needed any more.
            }
        }
    }

    /// Combine the infill areas of several layers into thicker infill layers (infill_sparse_thickness).
    pub fn combine_infill_layers(mesh: &mut SliceMeshStorage) {
        let settings = mesh.settings();
        if mesh.layers.is_empty() || mesh.layers.len() - 1 < settings.get_usize("top_layers") || settings.get_coord("infill_line_distance") == 0 {
            // No infill is even generated.
            return;
        }

        let layer_height = settings.get_coord("layer_height");
        // How many infill layers to combine to obtain the requested sparse thickness.
        let amount = (1u64).max(round_divide(settings.get_coord("infill_sparse_thickness") as u64, layer_height.max(1) as u64)) as usize;
        if amount <= 1 {
            // If we must combine 1 layer, nothing needs to be combined. Combining 0 layers is invalid.
            return;
        }

        // We need to round down the layer index we start at to the nearest divisible index. Otherwise we get some parts that have infill at divisible
        // layers and some at non-divisible layers. Those layers would then miss each other.
        let bottom_most_layers = settings.get_usize("initial_bottom_layers");
        let mut min_layer: LayerIndex = (bottom_most_layers + amount) as LayerIndex - 1;
        min_layer -= ((min_layer as u64) % amount as u64) as LayerIndex; // Round upwards to the nearest layer divisible by infill_sparse_combine.
        let mut max_layer: LayerIndex = mesh.layers.len() as LayerIndex - 1 - settings.get_usize("top_layers") as LayerIndex;
        max_layer -= ((max_layer as u64) % amount as u64) as LayerIndex; // Round downwards to the nearest layer divisible by infill_sparse_combine.
        let mut layer_idx = min_layer;
        while layer_idx <= max_layer {
            // Skip every few layers, but extrude more.
            for combine_count_here in 1..amount {
                if layer_idx < combine_count_here as LayerIndex {
                    break;
                }

                let lower_layer_idx = layer_idx - combine_count_here as LayerIndex;
                if lower_layer_idx < min_layer {
                    break;
                }
                let (lower, upper) = mesh.layers.split_at_mut(layer_idx as usize);
                let layer = &mut upper[0];
                let lower_layer = &mut lower[lower_layer_idx as usize];
                for part in layer.parts.iter_mut() {
                    let n_densities = part.infill_area_per_combine_per_density.len();
                    for density_idx in 0..n_densities {
                        // go over each density of gradual infill (these density areas overlap!)
                        let mut result = Shape::default();
                        for lower_layer_part in lower_layer.parts.iter_mut() {
                            if part.boundary_box.hit(&lower_layer_part.boundary_box) {
                                let infill_area_per_combine = &mut part.infill_area_per_combine_per_density[density_idx];
                                let intersection = infill_area_per_combine[combine_count_here - 1]
                                    .intersection(&lower_layer_part.infill_area)
                                    .offset_default(-200)
                                    .offset_default(200);
                                result.push_all(&intersection); // add area to be thickened
                                infill_area_per_combine[combine_count_here - 1] =
                                    infill_area_per_combine[combine_count_here - 1].difference(&intersection); // remove thickened area from less thick layer here
                                let mut max_lower_density_idx = density_idx;
                                // Generally: remove only from *same density* areas on layer below
                                // If there are no same density areas, then it's ok to print them anyway
                                // Don't remove other density areas
                                if density_idx == n_densities - 1 {
                                    // For the most dense areas on a given layer the density of that area is doubled.
                                    // This means that - if the lower layer has more densities -
                                    // all those lower density lines are included in the most dense of this layer.
                                    // We therefore compare the most dense are on this layer with all densities
                                    // of the lower layer with the same or higher density index
                                    max_lower_density_idx = lower_layer_part.infill_area_per_combine_per_density.len().wrapping_sub(1);
                                }
                                let mut lower_density_idx = density_idx;
                                while lower_density_idx <= max_lower_density_idx && lower_density_idx < lower_layer_part.infill_area_per_combine_per_density.len() {
                                    let lower_infill_area_per_combine = &mut lower_layer_part.infill_area_per_combine_per_density[lower_density_idx];
                                    lower_infill_area_per_combine[0] = lower_infill_area_per_combine[0].difference(&intersection); // remove thickened area from lower (single thickness) layer
                                    lower_density_idx += 1;
                                }
                            }
                        }

                        part.infill_area_per_combine_per_density[density_idx].push(result);
                    }
                }
            }
            layer_idx += amount as LayerIndex;
        }
    }
}
