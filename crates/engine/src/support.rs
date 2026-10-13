//! `support.h/.cpp`: area based ("normal") support generation.

use std::collections::BTreeMap;

use crate::application::current_slice;
use crate::geometry::point2ll::{pt, v_size2, Point2LL};
use crate::geometry::point3ll::Point3LL;
use crate::geometry::{Polygon, Shape, SingleShape};
use crate::infill::sierpinski_fill_provider::SierpinskiFillProvider;
use crate::settings::enums::{BuildPlateShape, EFillMethod, EPlatformAdhesion, ESupportStructure, ESupportType, SupportDistPriority};
use crate::settings::settings::Settings;
use crate::settings::types::{bounded_tan, AngleRadians, LayerIndex};
use crate::skin::generate_infill_wall_tool_paths;
use crate::slice_data_storage::{SliceDataStorage, SliceMeshStorage};
use crate::slicer::Slicer;
use crate::utils::aabb::AABB3D;
use crate::utils::coord::{mm2int, Coord};
use crate::utils::linear_alg_2d;
use crate::utils::math::{round_divide, round_up_divide};
use crate::utils::section_type::SectionType;
use crate::utils::simplify::Simplify;
use crate::utils::thread_pool::{parallel_for, UnsafeSlice};
use clipper::JoinType;
use std::sync::Arc;

pub struct AreaSupport;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ModifierType {
    AntiOverhang,
    SupportDropDown,
    SupportVanilla,
}

/// `cura::parallel_for(first, last, body, chunk_size_factor)`: blocks of `chunk_size_factor`
/// consecutive iterations (starting at `first`) are executed in order; blocks run in parallel.
fn parallel_for_chunked<F: Fn(usize) + Sync>(first: usize, last: usize, chunk: usize, f: F) {
    if last <= first {
        return;
    }
    let chunk = chunk.max(1);
    let n_blocks = (last - first).div_ceil(chunk);
    parallel_for(0, n_blocks, |b| {
        let start = first + b * chunk;
        let end = (start + chunk).min(last);
        for i in start..end {
            f(i);
        }
    });
}

impl AreaSupport {
    /// Store the support (or anti-overhang) areas of a support modifier mesh in the support storage.
    /// Returns whether the mesh is a support modifier mesh.
    pub fn handle_support_modifier_mesh(storage: &mut SliceDataStorage, mesh_settings: &Settings, slicer: &Slicer) -> bool {
        if !mesh_settings.get_bool("anti_overhang_mesh") && !mesh_settings.get_bool("support_mesh") {
            return false;
        }
        let modifier_type = if mesh_settings.get_bool("anti_overhang_mesh") {
            ModifierType::AntiOverhang
        } else if mesh_settings.get_bool("support_mesh_drop_down") {
            ModifierType::SupportDropDown
        } else {
            ModifierType::SupportVanilla
        };
        for layer_nr in 0..slicer.layers.len() as LayerIndex {
            let support_layer = &mut storage.support.support_layers[layer_nr as usize];
            let slicer_layer = &slicer.layers[layer_nr as usize];
            match modifier_type {
                ModifierType::AntiOverhang => support_layer.anti_overhang.push_all(&slicer_layer.polygons),
                ModifierType::SupportDropDown => support_layer.support_mesh_drop_down.push_all(&slicer_layer.polygons),
                ModifierType::SupportVanilla => support_layer.support_mesh.push_all(&slicer_layer.polygons),
            }
        }
        true
    }

    /// Split the global support areas into parts for gradual support infill generation.
    fn split_global_support_areas_into_support_infill_parts(storage: &mut SliceDataStorage, global_support_areas_per_layer: &[Shape], total_layer_count: u32) {
        if total_layer_count == 0 {
            return;
        }

        let min_layer: usize = 0;
        let max_layer: usize = total_layer_count as usize - 1;

        let mesh_group_settings = &current_slice().scene.current_mesh_group().settings;
        let infill_extruder = mesh_group_settings.get_extruder_train("support_infill_extruder_nr");
        let support_pattern = infill_extruder.settings.get_enum::<EFillMethod>("support_pattern");
        let support_line_width = infill_extruder.settings.get_coord("support_line_width");

        // The wall line count is used for calculating insets, and we generate support infill patterns within the insets
        let wall_line_count = infill_extruder.settings.get_usize("support_wall_count");

        // Generate separate support islands
        for layer_nr in 0..(total_layer_count as LayerIndex - 1) {
            let mut wall_line_count_this_layer = wall_line_count as u32;
            if layer_nr == 0 && (support_pattern == EFillMethod::LINES || support_pattern == EFillMethod::ZIG_ZAG) {
                // The first layer will be printed with a grid pattern
                wall_line_count_this_layer += 1;
            }

            let global_support_areas = &global_support_areas_per_layer[layer_nr as usize];
            if global_support_areas.is_empty() || (layer_nr as usize) < min_layer || layer_nr as usize > max_layer {
                // Initialize support_infill_parts empty
                storage.support.support_layers[layer_nr as usize].support_infill_parts.clear();
                continue;
            }

            let mut support_line_width_here = support_line_width;
            if layer_nr == 0 && mesh_group_settings.get_enum::<EPlatformAdhesion>("adhesion_type") != EPlatformAdhesion::RAFT {
                support_line_width_here = (support_line_width_here as f64 * infill_extruder.settings.get_ratio("initial_layer_line_width_factor")) as Coord;
            }
            // We don't generate insets and infill area for the parts yet because later the skirt/brim and prime
            // tower will remove themselves from the support, so the outlines of the parts can be changed.
            let layer_height = infill_extruder.settings.get_coord("layer_height");
            let (support, meshes) = (&mut storage.support, &storage.meshes);
            support.support_layers[layer_nr as usize].fill_infill_parts(
                layer_nr,
                global_support_areas_per_layer,
                layer_height,
                meshes,
                support_line_width_here,
                wall_line_count_this_layer as Coord,
                0,
                false,
                0,
            );
        }
    }

    /// Generate gradual support, combine support infill layers and clean up.
    pub fn generate_support_infill_features(storage: &mut SliceDataStorage) {
        Self::generate_gradual_support(storage);

        // combine support infill layers
        Self::combine_support_infill_layers(storage);

        Self::cleanup(storage);
    }

    fn generate_gradual_support(storage: &mut SliceDataStorage) {
        let mesh_group_settings = &current_slice().scene.current_mesh_group().settings;
        let total_layer_count = storage.print_layer_count as usize;
        let infill_extruder = mesh_group_settings.get_extruder_train("support_infill_extruder_nr");
        let gradual_support_step_height = infill_extruder.settings.get_coord("gradual_support_infill_step_height");
        let max_density_steps = infill_extruder.settings.get_usize("gradual_support_infill_steps");

        let wall_width = infill_extruder.settings.get_coord("support_line_width");
        let is_connected =
            infill_extruder.settings.get_bool("zig_zaggify_infill") || infill_extruder.settings.get_enum::<EFillMethod>("infill_pattern") == EFillMethod::ZIG_ZAG;
        let simplifier = Simplify::from_settings(&infill_extruder.settings);

        // no early-out for this function; it needs to initialize the [infill_area_per_combine_per_density]
        let mut layer_skip_count: f64 = 8.0; // skip every so many layers as to ignore small gaps in the model making computation more easy
        // The difference in layer count between consecutive density infill areas.
        let gradual_support_step_layer_count =
            round_divide(gradual_support_step_height as u64, mesh_group_settings.get_coord("layer_height") as u64) as usize;

        // make gradual_support_step_height divisable by layer_skip_count
        let c = (gradual_support_step_layer_count as f64 / layer_skip_count).ceil();
        let n_skip_steps_per_gradual_step = if 1.0 < c { c } else { 1.0 }; // only decrease layer_skip_count to make it a divisor of gradual_support_step_layer_count
        layer_skip_count = gradual_support_step_layer_count as f64 / n_skip_steps_per_gradual_step;

        let min_layer: LayerIndex = 0;
        let max_layer: LayerIndex = total_layer_count as LayerIndex - 1;

        // compute different density areas for each support island
        let mut layer_nr: LayerIndex = 0;
        while layer_nr < total_layer_count as LayerIndex - 1 {
            if layer_nr < min_layer || layer_nr > max_layer {
                layer_nr += 1;
                continue;
            }

            // generate separate support islands and calculate density areas for each island
            let n_parts = storage.support.support_layers[layer_nr as usize].support_infill_parts.len();
            for part_idx in 0..n_parts {
                let original_area = storage.support.support_layers[layer_nr as usize].support_infill_parts[part_idx].get_infill_area().clone();
                if original_area.is_empty() {
                    continue;
                }
                // NOTE: This both generates the walls _and_ returns the _actual_ infill area (the one _without_ walls) for use in the rest of the method.
                let inset_count = storage.support.support_layers[layer_nr as usize].support_infill_parts[part_idx].inset_count_to_generate;
                let mut wall_toolpaths =
                    std::mem::take(&mut storage.support.support_layers[layer_nr as usize].support_infill_parts[part_idx].wall_toolpaths);
                let infill_area = generate_infill_wall_tool_paths(
                    &mut wall_toolpaths,
                    &original_area,
                    inset_count as usize,
                    wall_width,
                    &infill_extruder.settings,
                    layer_nr,
                    SectionType::SUPPORT,
                );
                let support_infill_part = &mut storage.support.support_layers[layer_nr as usize].support_infill_parts[part_idx];
                support_infill_part.wall_toolpaths = wall_toolpaths;
                let this_part_boundary_box = support_infill_part.outline_boundary_box;
                let this_outline_empty = support_infill_part.outline.0.is_empty();

                // calculate density areas for this island
                let mut less_dense_support = infill_area.clone(); // one step less dense with each density_step
                let mut sum_more_dense = Shape::default(); // NOTE: Only used for zig-zag or connected fills.
                let mut new_densities: Vec<Vec<Shape>> = Vec::new();
                for density_step in 0..max_density_steps {
                    let actual_min_layer: LayerIndex =
                        layer_nr + (density_step * gradual_support_step_layer_count) as LayerIndex + layer_skip_count as LayerIndex;
                    let actual_max_layer: LayerIndex = layer_nr + ((density_step + 1) * gradual_support_step_layer_count) as LayerIndex;

                    let mut upper_layer_idx = actual_min_layer as f64;
                    while upper_layer_idx <= actual_max_layer as f64 {
                        if (upper_layer_idx as u32) >= total_layer_count as u32 {
                            less_dense_support.clear();
                            break;
                        }

                        // compute intersections with relevant upper parts
                        let upper_infill_parts = &storage.support.support_layers[upper_layer_idx as usize].support_infill_parts;
                        let mut relevant_upper_polygons = Shape::default();
                        for upper_part in upper_infill_parts {
                            if this_outline_empty {
                                continue;
                            }

                            // we compute intersection based on support infill areas
                            let upper_part_boundary_box = &upper_part.outline_boundary_box;
                            // Here we are comparing the **outlines** of the infill areas
                            if upper_part_boundary_box.hit(&this_part_boundary_box) {
                                relevant_upper_polygons.push_all(&upper_part.outline.0);
                            }
                        }

                        less_dense_support = less_dense_support.intersection(&relevant_upper_polygons);
                        upper_layer_idx += layer_skip_count;
                    }
                    if less_dense_support.is_empty() {
                        break;
                    }

                    // add new infill_area_per_combine_per_density for the current density
                    let more_dense_support = infill_area.difference(&less_dense_support);
                    new_densities.push(vec![simplifier.shape(&more_dense_support.difference(&sum_more_dense))]);
                    if is_connected {
                        sum_more_dense = sum_more_dense.union_polygons_other(&more_dense_support);
                    }
                }

                new_densities.push(vec![simplifier.shape(&infill_area.difference(&sum_more_dense))]);
                let support_infill_part = &mut storage.support.support_layers[layer_nr as usize].support_infill_parts[part_idx];
                support_infill_part.infill_area_per_combine_per_density.extend(new_densities);
            }
            layer_nr += 1;
        }
    }

    fn combine_support_infill_layers(storage: &mut SliceDataStorage) {
        let mesh_group_settings = &current_slice().scene.current_mesh_group().settings;
        let total_layer_count = storage.print_layer_count as u32;
        let layer_height = mesh_group_settings.get_coord("layer_height");
        // How many support infill layers to combine to obtain the requested sparse thickness.
        let infill_extruder = mesh_group_settings.get_extruder_train("support_infill_extruder_nr");
        let combine_layers_amount = (1u64)
            .max(round_divide(infill_extruder.settings.get_coord("support_infill_sparse_thickness") as u64, layer_height.max(1) as u64))
            as usize;
        if combine_layers_amount <= 1 {
            return;
        }

        // We need to round down the layer index we start at to the nearest divisible index.
        let mut min_layer: usize = combine_layers_amount - 1;
        min_layer -= min_layer % combine_layers_amount; // Round upwards to the nearest layer divisible by infill_sparse_combine.
        let n_support_layers = storage.support.support_layers.len();
        let mut max_layer: usize = if (total_layer_count as usize) < n_support_layers { total_layer_count as usize } else { n_support_layers };
        max_layer = max_layer.wrapping_sub(1);
        max_layer -= max_layer % combine_layers_amount; // Round downwards to the nearest layer divisible by infill_sparse_combine.

        let mut layer_idx = min_layer;
        while layer_idx <= max_layer {
            // Skip every few layers, but extrude more.
            if layer_idx >= n_support_layers {
                break;
            }

            for combine_count_here in 1..combine_layers_amount as u32 {
                if layer_idx < combine_count_here as usize {
                    break;
                }

                let lower_layer_idx = layer_idx - combine_count_here as usize;
                if lower_layer_idx < min_layer {
                    break;
                }
                let (lower, upper) = storage.support.support_layers.split_at_mut(layer_idx);
                let layer = &mut upper[0];
                let lower_layer = &mut lower[lower_layer_idx];

                for part in layer.support_infill_parts.iter_mut() {
                    if part.get_infill_area().is_empty() {
                        continue;
                    }
                    let n_densities = part.infill_area_per_combine_per_density.len();
                    for density_idx in 0..n_densities {
                        // go over each density of gradual infill (these density areas overlap!)
                        let mut result = Shape::default();
                        for lower_layer_part in lower_layer.support_infill_parts.iter_mut() {
                            if !part.outline_boundary_box.hit(&lower_layer_part.outline_boundary_box) {
                                continue;
                            }

                            let infill_area_per_combine = &mut part.infill_area_per_combine_per_density[density_idx];
                            let intersection = infill_area_per_combine[combine_count_here as usize - 1]
                                .intersection(lower_layer_part.get_infill_area())
                                .offset_default(-200)
                                .offset_default(200);
                            if intersection.is_empty() {
                                continue;
                            }

                            result.push_all(&intersection); // add area to be thickened
                            infill_area_per_combine[combine_count_here as usize - 1] =
                                infill_area_per_combine[combine_count_here as usize - 1].difference(&intersection); // remove thickened area from less thick layer here

                            let mut max_lower_density_idx = density_idx as u32;
                            // Generally: remove only from *same density* areas on layer below
                            // If there are no same density areas, then it's ok to print them anyway
                            // Don't remove other density areas
                            if density_idx == n_densities - 1 {
                                // For the most dense areas on a given layer the density of that area is doubled.
                                max_lower_density_idx = (lower_layer_part.infill_area_per_combine_per_density.len() as u32).wrapping_sub(1);
                            }
                            let mut lower_density_idx = density_idx as u32;
                            while lower_density_idx <= max_lower_density_idx
                                && (lower_density_idx as usize) < lower_layer_part.infill_area_per_combine_per_density.len()
                            {
                                let lower_infill_area_per_combine = &mut lower_layer_part.infill_area_per_combine_per_density[lower_density_idx as usize];
                                lower_infill_area_per_combine[0] = lower_infill_area_per_combine[0].difference(&intersection); // remove thickened area from lower (single thickness) layer
                                lower_density_idx += 1;
                            }
                        }

                        part.infill_area_per_combine_per_density[density_idx].push(result);
                    }
                }
            }
            layer_idx += combine_layers_amount;
        }
    }

    /// Remove support infill parts which are too small to print.
    fn cleanup(storage: &mut SliceDataStorage) {
        let support_line_width = current_slice().scene.current_mesh_group().settings.get_coord("support_line_width");
        for layer in storage.support.support_layers.iter_mut() {
            let mut part_idx: usize = 0;
            while part_idx < layer.support_infill_parts.len() {
                let part = &layer.support_infill_parts[part_idx];
                let mut can_be_removed = true;
                if part.inset_count_to_generate > 0 {
                    can_be_removed = false;
                } else {
                    'outer: for infill_area_per_combine_this_density in &part.infill_area_per_combine_per_density {
                        for infill_area_this_combine_this_density in infill_area_per_combine_this_density {
                            // remove small areas which were introduced by rounding errors in comparing the same area on two consecutive layer
                            if !infill_area_this_combine_this_density.is_empty()
                                && infill_area_this_combine_this_density.area() > (support_line_width * support_line_width) as f64
                            {
                                can_be_removed = false;
                                break 'outer;
                            }
                        }
                    }
                }
                if can_be_removed {
                    layer.support_infill_parts.swap_remove(part_idx);
                    part_idx = part_idx.wrapping_sub(1);
                }
                part_idx = part_idx.wrapping_add(1);
            }
        }
    }

    /// Join current support layer with the support of the layer above (conical support / join distance).
    fn join(storage: &SliceDataStorage, support_layer_up: &Shape, support_layer_this: &Shape) -> Shape {
        let joined: Shape;

        let infill_settings = &current_slice().scene.current_mesh_group().settings.get_extruder_train("support_infill_extruder_nr").settings;
        let conical_support_angle = infill_settings.get_angle_radians("support_conical_angle");
        let layer_thickness = infill_settings.get_coord("layer_height");
        let conical_support_offset: Coord = if conical_support_angle.value() > 0.0 {
            // outward ==> wider base than overhang
            (-bounded_tan(conical_support_angle) * layer_thickness as f64) as Coord
        } else {
            // inward ==> smaller base than overhang
            (bounded_tan(AngleRadians::new(-conical_support_angle.value())) * layer_thickness as f64) as Coord
        };
        let conical_support = infill_settings.get_bool("support_conical_enabled") && conical_support_angle.value() != 0.0;
        if conical_support {
            let mesh_group_settings = &current_slice().scene.current_mesh_group().settings;
            // Don't go outside the build volume.
            let mut machine_volume_border = Shape::default();
            match mesh_group_settings.get_enum::<BuildPlateShape>("machine_shape") {
                BuildPlateShape::ELLIPTIC => {
                    // Construct an ellipse to approximate the build volume.
                    let width = storage.machine_size.max.x - storage.machine_size.min.x;
                    let depth = storage.machine_size.max.y - storage.machine_size.min.y;
                    let mut border_circle = Polygon::default();
                    const CIRCLE_RESOLUTION: u32 = 50;
                    for i in 0..CIRCLE_RESOLUTION {
                        let angle = AngleRadians::new(std::f64::consts::TAU * i as f64 / CIRCLE_RESOLUTION as f64);
                        let machine_middle = storage.machine_size.get_middle();
                        let x = (machine_middle.x as f64 + angle.value().cos() * width as f64 / 2.0) as Coord;
                        let y = (machine_middle.y as f64 + angle.value().sin() * depth as f64 / 2.0) as Coord;
                        border_circle.points.push(pt(x, y));
                    }
                    machine_volume_border.push(border_circle);
                }
                _ => {
                    machine_volume_border.push(storage.machine_size.flatten().to_polygon());
                }
            }
            let mut adhesion_size: Coord = 0; // Make sure there is enough room for the platform adhesion around support.
            let mut extra_skirt_line_width: Coord = 0;
            let is_extruder_used = storage.get_extruders_used();
            for extruder_nr in 0..current_slice().scene.extruders.len() {
                if !is_extruder_used[extruder_nr] {
                    // Unused extruders and the primary adhesion extruder don't generate an extra skirt line.
                    continue;
                }
                let other_extruder = &current_slice().scene.extruders[extruder_nr];
                extra_skirt_line_width += (other_extruder.settings.get_coord("skirt_brim_line_width") as f64
                    * other_extruder.settings.get_ratio("initial_layer_line_width_factor")) as Coord;
            }
            let skirt_brim_extruders = mesh_group_settings.get_extruder_trains("skirt_brim_extruder_nr");
            let mut adhesion_width_str = "brim_width";
            let mut adhesion_line_count_str = "brim_line_count";
            match mesh_group_settings.get_enum::<EPlatformAdhesion>("adhesion_type") {
                EPlatformAdhesion::SKIRT | EPlatformAdhesion::BRIM => {
                    if mesh_group_settings.get_enum::<EPlatformAdhesion>("adhesion_type") == EPlatformAdhesion::SKIRT {
                        adhesion_width_str = "skirt_gap";
                        adhesion_line_count_str = "skirt_line_count";
                    }
                    for &e in &skirt_brim_extruders {
                        let skirt_brim_extruder = &current_slice().scene.extruders[e];
                        let s = &skirt_brim_extruder.settings;
                        let v = s.get_coord(adhesion_width_str) as f64
                            + (s.get_coord("skirt_brim_line_width") as f64
                                * (s.get_usize(adhesion_line_count_str).wrapping_sub(1)) as f64) // - 1 because the line is also included in extra_skirt_line_width
                                * s.get_ratio("initial_layer_line_width_factor")
                            + extra_skirt_line_width as f64;
                        adhesion_size = adhesion_size.max(v as Coord);
                    }
                }
                EPlatformAdhesion::RAFT => {
                    adhesion_size = mesh_group_settings
                        .get_extruder_train("raft_base_extruder_nr")
                        .settings
                        .get_coord("raft_base_margin")
                        .max(mesh_group_settings.get_extruder_train("raft_interface_extruder_nr").settings.get_coord("raft_interface_margin"))
                        .max(mesh_group_settings.get_extruder_train("raft_surface_extruder_nr").settings.get_coord("raft_surface_margin"));
                }
                EPlatformAdhesion::NONE => {
                    adhesion_size = 0;
                }
                _ => {
                    crate::log_info!("Unknown platform adhesion type! Please implement the width of the platform adhesion here.");
                }
            }
            machine_volume_border = machine_volume_border.offset_default(-adhesion_size);

            let conical_smallest_breadth = infill_settings.get_coord("support_conical_min_width");
            let insetted = support_layer_up.offset_default(-conical_smallest_breadth / 2);
            let small_parts = support_layer_up.difference(&insetted.offset_default(conical_smallest_breadth / 2 + 20));
            joined = support_layer_this
                .union_polygons_other(&support_layer_up.offset_default(conical_support_offset))
                .union_polygons_other(&small_parts)
                .intersection(&machine_volume_border);
        } else {
            joined = support_layer_this.union_polygons_other(support_layer_up);
        }

        // join different parts
        let mut joined = joined;
        let join_distance = infill_settings.get_coord("support_join_distance");
        if join_distance > 0 {
            // first offset the layer a little inwards; this way tiny area's will not be joined
            // (narrow areas; ergo small areas will be removed in a later step using this same offset)
            // this inwards offset is later reversed by increasing the outwards offset
            let min_even_wall_line_width = infill_settings.get_coord("min_even_wall_line_width");
            let half_min_feature_width = min_even_wall_line_width + 10;

            joined = joined
                .offset_default(-half_min_feature_width)
                .offset_join(join_distance + half_min_feature_width, JoinType::Round)
                .offset_join(-join_distance, JoinType::Round)
                .union_polygons_other(&joined);
        }

        let simplify = Simplify::from_settings(infill_settings);
        simplify.shape(&joined)
    }

    /// Generate the overhang areas for all meshes.
    pub fn generate_overhang_areas(storage: &mut SliceDataStorage) {
        for mesh_idx in 0..storage.meshes.len() {
            let settings = storage.meshes[mesh_idx].settings();
            if settings.get_bool("infill_mesh") || settings.get_bool("anti_overhang_mesh") {
                continue;
            }
            // it actually also initializes some buffers that are needed in generateSupport
            Self::generate_overhang_areas_for_mesh(storage, mesh_idx);
        }
    }

    /// Generate the support areas of all meshes.
    pub fn generate_support_areas(storage: &mut SliceDataStorage) {
        let mut global_support_areas_per_layer: Vec<Shape> = vec![Shape::default(); storage.print_layer_count as usize];

        let mut max_layer_nr_support_mesh_filled: i32 = storage.support.support_layers.len() as i32 - 1;
        while max_layer_nr_support_mesh_filled >= 0 {
            let support_layer = &storage.support.support_layers[max_layer_nr_support_mesh_filled as usize];
            if !support_layer.support_mesh.is_empty() || !support_layer.support_mesh_drop_down.is_empty() {
                break;
            }
            max_layer_nr_support_mesh_filled -= 1;
        }
        storage.support.layer_nr_max_filled_layer = max_layer_nr_support_mesh_filled;
        for layer_nr in 0..max_layer_nr_support_mesh_filled.max(0) as usize {
            let support_layer = &mut storage.support.support_layers[layer_nr];
            support_layer.anti_overhang = support_layer.anti_overhang.union_polygons();
            support_layer.support_mesh_drop_down = support_layer.support_mesh_drop_down.union_polygons();
            support_layer.support_mesh = support_layer.support_mesh.union_polygons();
        }

        // initialization of supportAreasPerLayer
        if storage.print_layer_count as usize > storage.support.support_layers.len() {
            // there might already be anti_overhang_area data or support mesh data in the supportLayers
            storage.support.support_layers.resize_with(storage.print_layer_count as usize, Default::default);
        }

        // generate support areas
        let mut support_meshes_drop_down_handled = false;
        let mut support_meshes_handled = false;
        let mesh_group_settings = &current_slice().scene.current_mesh_group().settings;
        for mesh_idx in 0..storage.meshes.len() {
            let mesh_settings = storage.meshes[mesh_idx].settings();
            if mesh_settings.get_bool("infill_mesh") || mesh_settings.get_bool("anti_overhang_mesh") {
                continue;
            }
            let mut infill_settings: &Settings = mesh_settings;
            let mut roof_settings: &Settings = mesh_settings;
            let mut bottom_settings: &Settings = mesh_settings;
            if mesh_settings.get_bool("support_mesh") {
                if (mesh_settings.get_bool("support_mesh_drop_down") && support_meshes_drop_down_handled)
                    || (!mesh_settings.get_bool("support_mesh_drop_down") && support_meshes_handled)
                {
                    // handle all support_mesh and support_mesh_drop_down areas only once
                    continue;
                }
                // use extruder train settings rather than the per-object settings of the first support mesh encountered.
                infill_settings = &mesh_group_settings.get_extruder_train("support_infill_extruder_nr").settings;
                roof_settings = &mesh_group_settings.get_extruder_train("support_roof_extruder_nr").settings;
                bottom_settings = &mesh_group_settings.get_extruder_train("support_bottom_extruder_nr").settings;
                if mesh_settings.get_bool("support_mesh_drop_down") {
                    support_meshes_drop_down_handled = true;
                } else {
                    support_meshes_handled = true;
                }
            }
            let mut mesh_support_areas_per_layer: Vec<Shape> = vec![Shape::default(); storage.print_layer_count as usize];

            let layer_count = storage.print_layer_count as usize;
            Self::generate_support_areas_for_mesh(
                storage,
                infill_settings,
                roof_settings,
                bottom_settings,
                mesh_idx,
                layer_count,
                &mut mesh_support_areas_per_layer,
            );
            for layer_idx in 0..storage.print_layer_count as usize {
                global_support_areas_per_layer[layer_idx].push_all(&mesh_support_areas_per_layer[layer_idx]);
            }
        }

        for support_areas in global_support_areas_per_layer.iter_mut() {
            *support_areas = support_areas.union_polygons();
        }

        // handle support interface
        for mesh_idx in 0..storage.meshes.len() {
            let settings = storage.meshes[mesh_idx].settings();
            if settings.get_bool("infill_mesh") || settings.get_bool("anti_overhang_mesh") {
                continue;
            }

            if settings.get_bool("support_roof_enable") {
                Self::generate_support_roof(storage, mesh_idx, &mut global_support_areas_per_layer);
            }
            if settings.get_bool("support_bottom_enable") {
                Self::generate_support_bottom(storage, mesh_idx, &mut global_support_areas_per_layer);
            }
        }

        // split the global support areas into parts for later gradual support infill generation
        let print_layer_count = storage.print_layer_count as u32;
        Self::split_global_support_areas_into_support_infill_parts(storage, &global_support_areas_per_layer, print_layer_count);
        Self::precompute_cross_infill_tree(storage);
    }

    pub(crate) fn precompute_cross_infill_tree(storage: &mut SliceDataStorage) {
        let mesh_group_settings = &current_slice().scene.current_mesh_group().settings;
        let infill_extruder = mesh_group_settings.get_extruder_train("support_infill_extruder_nr");
        let support_pattern = infill_extruder.settings.get_enum::<EFillMethod>("support_pattern");
        if (support_pattern == EFillMethod::CROSS || support_pattern == EFillMethod::CROSS_3D) && infill_extruder.settings.get_coord("support_line_distance") > 0 {
            let mut aabb = AABB3D::default();
            for mesh in &storage.meshes {
                let settings = mesh.settings();
                if settings.get_bool("infill_mesh") || settings.get_bool("anti_overhang_mesh") {
                    continue;
                }
                if settings.get_bool("support_mesh") {
                    // use extruder train settings rather than the per-object settings of the first support mesh encountered.
                    // because all support meshes are processed at the same time it doesn't make sense to use the per-object settings of the first support mesh encountered.
                    // instead we must use the support extruder settings, which is the settings base common to all support meshes.
                    // NOTE: in C++ `infill_settings` is a `Settings&` bound to the mesh settings, so this assignment
                    // overwrites the mesh's settings with a copy of the extruder settings. Mirror that.
                    let extruder_settings = mesh_group_settings.get_extruder_train("support_infill_extruder_nr").settings.clone();
                    unsafe {
                        *mesh.settings_mut() = extruder_settings;
                    }
                }
                let infill_settings: &Settings = mesh.settings();
                let aabb_expansion = infill_settings.get_coord("support_offset");
                let mut aabb_here = mesh.bounding_box;
                let min_here = aabb_here.min - Point3LL::new(-aabb_expansion, -aabb_expansion, 0);
                aabb_here.include(min_here);
                let max_here = aabb_here.max + Point3LL::new(-aabb_expansion, -aabb_expansion, 0);
                aabb_here.include(max_here);
                aabb.include_aabb(&aabb_here);
            }

            let cross_subdisivion_spec_image_file = infill_extruder.settings.get_string("cross_support_density_image");
            let good = !cross_subdisivion_spec_image_file.is_empty() && std::fs::File::open(&cross_subdisivion_spec_image_file).is_ok();
            if good {
                storage.support.cross_fill_provider = Some(Arc::new(SierpinskiFillProvider::new_with_image(
                    &aabb,
                    infill_extruder.settings.get_coord("support_line_distance"),
                    infill_extruder.settings.get_coord("support_line_width"),
                    &cross_subdisivion_spec_image_file,
                )));
            } else {
                if !cross_subdisivion_spec_image_file.is_empty() {
                    crate::log_error!("Cannot find density image: {}.", cross_subdisivion_spec_image_file);
                }
                storage.support.cross_fill_provider = Some(Arc::new(SierpinskiFillProvider::new(
                    &aabb,
                    infill_extruder.settings.get_coord("support_line_distance"),
                    infill_extruder.settings.get_coord("support_line_width"),
                )));
            }
        }
    }

    fn generate_overhang_areas_for_mesh(storage: &mut SliceDataStorage, mesh_idx: usize) {
        let settings = storage.meshes[mesh_idx].settings();
        if !settings.get_bool("support_enable") && !settings.get_bool("support_mesh") {
            return;
        }

        let print_layer_count = storage.print_layer_count as usize;
        // Fill the overhang areas with emptiness first, even if it's a support mesh, so that we can request the areas.
        {
            let mesh = &mut storage.meshes[mesh_idx];
            mesh.full_overhang_areas.resize_with(print_layer_count, Default::default);
            for _ in 0..print_layer_count {
                mesh.overhang_areas.push(Shape::default());
            }
        }

        // Don't generate overhang areas for support meshes. They are dropped down automatically if desired.
        let is_support_modifier = settings.get_bool("support_mesh");
        if is_support_modifier {
            return;
        }

        // Don't generate overhang areas if the Z distance is higher than the objects we're generating support for.
        let layer_height = current_slice().scene.current_mesh_group().settings.get_coord("layer_height");
        let z_distance_top = settings.get_coord("support_top_distance");
        let z_distance_top_layers = (z_distance_top / layer_height) as usize + 1;
        if z_distance_top_layers + 1 > print_layer_count {
            return;
        }

        // Generate points and lines of overhang (for corners pointing downwards, since they don't have an area to support but still need supporting).
        let max_supported_diameter = settings.get_coord("support_tower_maximum_supported_diameter");
        let use_towers = settings.get_bool("support_use_towers") && max_supported_diameter > 0;
        if use_towers {
            Self::detect_overhang_points(storage, mesh_idx);
        }

        // Generate the actual areas and store them in the mesh.
        let mut overhang_areas = std::mem::take(&mut storage.meshes[mesh_idx].overhang_areas);
        let mut full_overhang_areas = std::mem::take(&mut storage.meshes[mesh_idx].full_overhang_areas);
        {
            let oa = UnsafeSlice::new(&mut overhang_areas);
            let foa = UnsafeSlice::new(&mut full_overhang_areas);
            let storage_ref: &SliceDataStorage = storage;
            let mesh: &SliceMeshStorage = &storage_ref.meshes[mesh_idx];
            parallel_for(1, print_layer_count, |layer_idx| {
                let (basic, full) = Self::compute_basic_and_full_overhang(storage_ref, mesh, layer_idx as LayerIndex);
                // SAFETY: each layer index is written by exactly one iteration.
                unsafe {
                    *oa.get_mut(layer_idx) = basic; // Store the results.
                    *foa.get_mut(layer_idx) = full;
                }
            });
        }
        storage.meshes[mesh_idx].overhang_areas = overhang_areas;
        storage.meshes[mesh_idx].full_overhang_areas = full_overhang_areas;
    }

    /// Generate the X/Y disallowed area with a varying X/Y distance depending on the slope of the model.
    fn generate_varying_xy_disallowed_area(storage: &SliceMeshStorage, layer_idx: LayerIndex) -> Shape {
        let mesh_group_settings = &current_slice().scene.current_mesh_group().settings;
        let simplify = Simplify::from_settings(mesh_group_settings);
        let layer_thickness = mesh_group_settings.get_coord("layer_height");
        let support_distance_top = mesh_group_settings.get_coord("support_top_distance") as f64;
        let support_distance_bot = mesh_group_settings.get_coord("support_bottom_distance") as f64;
        let overhang_angle = mesh_group_settings.get_angle_radians("support_angle");
        let xy_distance = mesh_group_settings.get_coord("support_xy_distance") as f64;

        const CLOSE_DIST: Coord = 20;

        let layer_current = simplify.shape(&storage.layers[layer_idx as usize].get_outlines(false).offset_default(-CLOSE_DIST).offset_default(CLOSE_DIST));

        struct ZDeltaPoly {
            support_distance: f64,
            delta_z: f64,
            layer_delta: Shape,
        }

        let mut z_distances_layer_deltas: Vec<ZDeltaPoly> = Vec::new();

        // We only use two compare-layers for the slope calculation.
        const LAYER_INDEX_OFFSET: usize = 1;

        let layer_idx_below: LayerIndex = (layer_idx - LAYER_INDEX_OFFSET as LayerIndex).max(0);
        if layer_idx_below != layer_idx {
            let layer_below =
                simplify.shape(&storage.layers[layer_idx_below as usize].get_outlines(false).offset_default(-CLOSE_DIST).offset_default(CLOSE_DIST));
            z_distances_layer_deltas.push(ZDeltaPoly {
                support_distance: support_distance_bot,
                delta_z: -((LAYER_INDEX_OFFSET as Coord * layer_thickness) as f64),
                layer_delta: layer_below,
            });
        }

        let layer_idx_above: LayerIndex = (layer_idx + LAYER_INDEX_OFFSET as LayerIndex).min(storage.layers.len() as LayerIndex - 1);
        if layer_idx_above != layer_idx {
            let layer_above =
                simplify.shape(&storage.layers[layer_idx_above as usize].get_outlines(false).offset_default(-CLOSE_DIST).offset_default(CLOSE_DIST));
            z_distances_layer_deltas.push(ZDeltaPoly {
                support_distance: support_distance_top,
                delta_z: (LAYER_INDEX_OFFSET as Coord * layer_thickness) as f64,
                layer_delta: layer_above,
            });
        }

        // Initialize the offset_dist_at_point map with all the points in the current layer.
        let mut offset_dist_at_point: BTreeMap<(u32, u32), (usize, f64)> = BTreeMap::new();
        for (current_poly_idx, current_poly) in layer_current.iter().enumerate() {
            for (current_point_idx, _) in current_poly.points.iter().enumerate() {
                offset_dist_at_point.entry((current_poly_idx as u32, current_point_idx as u32)).or_insert((0, 0.0));
            }
        }

        for z_delta_poly in &z_distances_layer_deltas {
            let support_distance = z_delta_poly.support_distance;
            let delta_z = z_delta_poly.delta_z;
            let layer_delta = &z_delta_poly.layer_delta;
            let xy_distance_natural = support_distance * bounded_tan(overhang_angle);

            for (current_poly_idx, current_poly) in layer_current.iter().enumerate() {
                for (current_point_idx, &current_point) in current_poly.points.iter().enumerate() {
                    let mut min_dist2 = Coord::MAX;

                    for delta_poly in layer_delta.iter() {
                        let n = delta_poly.points.len();
                        // wrap around to make sure all line segments are included
                        let total = if n == 0 { 0 } else { n + 1 };
                        for w in 0..total.saturating_sub(1) {
                            let delta_point = delta_poly.points[w % n];
                            let delta_point_next = delta_poly.points[(w + 1) % n];
                            let dist2 = linear_alg_2d::get_dist2_from_line_segment(delta_point, current_point, delta_point_next);
                            min_dist2 = if dist2 < min_dist2 { dist2 } else { min_dist2 };
                        }
                    }

                    let min_dist = (min_dist2 as f64).sqrt();
                    let slope = min_dist / delta_z;
                    let wall_angle = slope.abs().atan();
                    let r = wall_angle / overhang_angle.value();
                    let r = if r < 1.0 { r } else { 1.0 };
                    let ratio = if 0.0 < r { r } else { 0.0 };
                    let xy_distance_varying = cppstd::lerp(xy_distance, xy_distance_natural, ratio);

                    let key = (current_poly_idx as u32, current_point_idx as u32);
                    let (n, commutative_offset) = offset_dist_at_point[&key];
                    offset_dist_at_point.insert(key, (n + 1, commutative_offset + xy_distance_varying));
                }
            }
        }

        let mut varying_offsets: Vec<Coord> = Vec::new();

        for (current_poly_idx, current_poly) in layer_current.iter().enumerate() {
            for (current_point_idx, _) in current_poly.points.iter().enumerate() {
                let (n, commutative_offset) = offset_dist_at_point[&(current_poly_idx as u32, current_point_idx as u32)];

                let offset_dist = if n == 0 {
                    // the walls at vertex $p$ are vertical, so the xy_distance is taken at vertex $p$.
                    xy_distance
                } else {
                    // Take average of all dists generated for vertex $p$.
                    commutative_offset / n as f64
                };

                varying_offsets.push(offset_dist as Coord);
            }
        }

        let smooth_dist = xy_distance / 2.0;
        // offset using the varying offset distances we calculated previously, then close to smooth the boundary.
        layer_current.offset_multi(&varying_offsets).offset_default(smooth_dist as Coord).offset_default(-smooth_dist as Coord)
    }

    /// Generate support polygons over all layers for one mesh.
    #[allow(clippy::too_many_arguments)]
    fn generate_support_areas_for_mesh(
        storage: &mut SliceDataStorage,
        infill_settings: &Settings,
        roof_settings: &Settings,
        bottom_settings: &Settings,
        mesh_idx: usize,
        layer_count: usize,
        support_areas: &mut [Shape],
    ) {
        let mesh_settings = storage.meshes[mesh_idx].settings();

        let support_structure = mesh_settings.get_enum::<ESupportStructure>("support_structure");
        // whether this mesh has empty SliceMeshStorage and this function is now called to only generate support for all support meshes
        let is_support_mesh_place_holder = mesh_settings.get_bool("support_mesh");
        if (!mesh_settings.get_bool("support_enable") || support_structure != ESupportStructure::NORMAL) && !is_support_mesh_place_holder {
            return;
        }
        let mesh_group_settings = &current_slice().scene.current_mesh_group().settings;
        let support_type = mesh_group_settings.get_enum::<ESupportType>("support_type");
        if support_type == ESupportType::NONE && !is_support_mesh_place_holder {
            return;
        }

        // early out
        let layer_thickness = mesh_group_settings.get_coord("layer_height");
        let z_distance_top = (if mesh_settings.get_bool("support_roof_enable") { roof_settings } else { infill_settings }).get_coord("support_top_distance");
        let layer_z_distance_top = (z_distance_top / layer_thickness) as usize + 1;
        if layer_z_distance_top + 1 > layer_count {
            return;
        }

        {
            let mesh = &storage.meshes[mesh_idx];
            if !mesh_settings.get_bool("support_mesh")
                && mesh.overhang_areas.iter().all(|a| a.is_empty())
                && mesh.full_overhang_areas.iter().all(|a| a.is_empty())
            {
                // Mesh has no overhang, skip support generation
                return;
            }
        }

        // Compute the areas that are disallowed by the X/Y distance.
        let mut xy_disallowed_per_layer: Vec<Shape> = vec![Shape::default(); layer_count];
        let mut sloped_areas_per_layer: Vec<Shape> = vec![Shape::default(); layer_count];
        sloped_areas_per_layer[0] = Shape::default();
        // simplified processing for bottom layer - just ensure support clears part by XY distance
        let xy_distance = infill_settings.get_coord("support_xy_distance");
        let xy_distance_overhang = infill_settings.get_coord("support_xy_distance_overhang");
        // whether to use a different xy distance at overhangs
        let use_xy_distance_overhang = infill_settings.get_enum::<SupportDistPriority>("support_xy_overrides_z") == SupportDistPriority::Z_OVERRIDES_XY;
        const NO_SUPPORT: bool = false;
        const NO_PRIME_TOWER: bool = false;
        let support_line_width = mesh_group_settings.get_extruder_train("support_infill_extruder_nr").settings.get_coord("support_line_width");
        let sloped_areas_angle = mesh_settings.get_angle_radians("support_bottom_stair_step_min_slope").value();
        let sloped_area_detection_width: Coord = 10 + ((layer_thickness as f64 / sloped_areas_angle.tan()) as Coord) / 2;
        let minimum_support_area = mesh_settings.get_f64("minimum_support_area");
        let min_even_wall_line_width = mesh_settings.get_coord("min_even_wall_line_width");
        xy_disallowed_per_layer[0] = storage.get_layer_outlines(0, NO_SUPPORT, NO_PRIME_TOWER, false, -1, true).offset_default(xy_distance);

        // The maximum width of an odd wall = 2 * minimum even wall width.
        let half_min_feature_width = min_even_wall_line_width + 10;

        {
            let storage_ref: &SliceDataStorage = storage;
            let mesh: &SliceMeshStorage = &storage_ref.meshes[mesh_idx];
            let sloped = UnsafeSlice::new(&mut sloped_areas_per_layer);
            let xy_dis = UnsafeSlice::new(&mut xy_disallowed_per_layer);
            parallel_for(1, layer_count, |layer_idx| {
                let outlines = storage_ref.get_layer_outlines(layer_idx as LayerIndex, NO_SUPPORT, NO_PRIME_TOWER, false, -1, true);

                // Build sloped areas. We need this for the stair-stepping later on.
                let sloped_here = storage_ref
                    .get_layer_outlines(layer_idx as LayerIndex - 1, NO_SUPPORT, NO_PRIME_TOWER, false, -1, true)
                    .create_tube_shape(sloped_area_detection_width, 10)
                    .intersection(&outlines.create_tube_shape(10, sloped_area_detection_width))
                    .offset_default(-10)
                    .offset_default(10 + sloped_area_detection_width);
                // SAFETY: each layer index is written by exactly one iteration.
                unsafe {
                    *sloped.get_mut(layer_idx) = sloped_here;
                }

                if !is_support_mesh_place_holder {
                    // don't compute overhang for support meshes
                    if use_xy_distance_overhang {
                        // Z overrides XY distance.
                        let minimum_xy_disallowed_areas = mesh.layers[layer_idx].get_outlines(false).offset_default(xy_distance_overhang);
                        let varying_xy_disallowed_areas = Self::generate_varying_xy_disallowed_area(mesh, layer_idx as LayerIndex);
                        unsafe {
                            *xy_dis.get_mut(layer_idx) = minimum_xy_disallowed_areas.union_polygons_other(&varying_xy_disallowed_areas);
                        }
                    }
                }
                if is_support_mesh_place_holder || !use_xy_distance_overhang {
                    unsafe {
                        *xy_dis.get_mut(layer_idx) = outlines.offset_default(xy_distance);
                    }
                }
            });
        }

        let mut tower_roofs: Vec<Shape> = Vec::new();
        let mut stair_removal = Shape::default(); // polygons to subtract from support because of stair-stepping

        let is_support_mesh_nondrop_place_holder = is_support_mesh_place_holder && !mesh_settings.get_bool("support_mesh_drop_down");
        let is_support_mesh_drop_down_place_holder = is_support_mesh_place_holder && mesh_settings.get_bool("support_mesh_drop_down");

        let bottom_stair_step_width = (0 as Coord).max(mesh_settings.get_coord("support_bottom_stair_step_width"));
        let extension_offset = infill_settings.get_coord("support_offset");

        let max_tower_supported_diameter = infill_settings.get_coord("support_tower_maximum_supported_diameter");
        let use_towers = infill_settings.get_bool("support_use_towers") && max_tower_supported_diameter > 0;

        let z_distance_bottom = (if mesh_settings.get_bool("support_bottom_enable") { bottom_settings } else { infill_settings }).get_coord("support_bottom_distance");
        let bottom_empty_layer_count = round_up_divide(z_distance_bottom as u64, layer_thickness as u64) as usize; // number of empty layers between support and model
        let bottom_stair_step_height = (0 as Coord).max(mesh_settings.get_coord("support_bottom_stair_step_height"));
        // the difference in layers between two stair steps. One is normal support (not stair-like)
        let bottom_stair_step_layer_count = (bottom_stair_step_height / layer_thickness) as usize + 1;

        // Post-process the sloped areas's. (Skip if no stair-stepping anyway.)
        if bottom_stair_step_layer_count > 1 {
            let sloped = UnsafeSlice::new(&mut sloped_areas_per_layer);
            parallel_for_chunked(1, layer_count, bottom_stair_step_layer_count, |layer_idx| {
                // Add the sloped areas together for each stair of the stair stepping.
                // Start a new stair every modulo bottom_stair_step_layer_count steps.
                if layer_idx % bottom_stair_step_layer_count != 1 {
                    // SAFETY: within a chunk the iterations are sequential; chunks are disjoint.
                    unsafe {
                        let prev = sloped.get(layer_idx - 1).clone();
                        let here = sloped.get_mut(layer_idx);
                        *here = here.union_polygons_other(&prev);
                    }
                }
            });
        }

        let mut layer_idx: usize = layer_count - 1 - layer_z_distance_top;
        while layer_idx != usize::MAX {
            let mut layer_this = storage.meshes[mesh_idx].full_overhang_areas[layer_idx + layer_z_distance_top].clone();

            if extension_offset != 0 && !is_support_mesh_place_holder {
                // To avoid that the support is folding around the model, the support horizontal expansion should not cause
                // the support to grow towards the model.
                let mut model_outline = storage.get_layer_outlines(layer_idx as LayerIndex, NO_SUPPORT, NO_PRIME_TOWER, false, -1, true);
                let offset_per_step = support_line_width / 2;

                // perform a small offset we don't enlarge small features of the support
                let mut horizontal_expansion = layer_this.clone();
                let mut offset_cumulative: Coord = 0;
                while offset_cumulative <= extension_offset {
                    horizontal_expansion = horizontal_expansion.offset_default(offset_per_step);
                    model_outline = model_outline.difference(&horizontal_expansion);
                    model_outline = model_outline.offset_default(offset_per_step);
                    horizontal_expansion = horizontal_expansion.difference(&model_outline);
                    offset_cumulative += offset_per_step;
                }
                layer_this = layer_this.union_polygons_other(&horizontal_expansion);
            }

            if use_towers && !is_support_mesh_place_holder {
                // handle straight walls
                Self::handle_wall_struts(infill_settings, &mut layer_this);
                // handle towers
                let overhang_points = &mut storage.meshes[mesh_idx].overhang_points;
                Self::handle_towers(
                    infill_settings,
                    &xy_disallowed_per_layer[layer_idx],
                    &mut layer_this,
                    &mut tower_roofs,
                    overhang_points,
                    layer_idx as LayerIndex,
                    layer_count,
                );
            }

            if layer_idx + 1 < layer_count {
                // join with support from layer up
                let empty = Shape::default();
                let mut layer_above: &Shape = if layer_idx < support_areas.len() { &support_areas[layer_idx + 1] } else { &empty };
                let model_mesh_on_layer = if layer_idx > 0 && !is_support_mesh_nondrop_place_holder {
                    storage.get_layer_outlines(layer_idx as LayerIndex, NO_SUPPORT, NO_PRIME_TOWER, false, -1, true)
                } else {
                    Shape::default()
                };
                if is_support_mesh_nondrop_place_holder {
                    layer_above = &empty;
                    layer_this = layer_this.union_polygons_other(&storage.support.support_layers[layer_idx].support_mesh);
                }
                layer_this = Self::join(storage, layer_above, &layer_this).difference(&model_mesh_on_layer);
            }

            // make towers for small support
            if use_towers {
                for poly in layer_this.split_into_parts(false) {
                    let polygon_part =
                        poly.0.difference(&xy_disallowed_per_layer[layer_idx]).offset_default(-half_min_feature_width).offset_default(half_min_feature_width);

                    let part_area = polygon_part.area() as i64;
                    if part_area == 0 || part_area > max_tower_supported_diameter * max_tower_supported_diameter {
                        continue;
                    }

                    const TOWER_TOP_LAYER_COUNT: usize = 6; // number of layers after which to conclude that a tiny support area needs a tower
                    if layer_idx < layer_count.wrapping_sub(TOWER_TOP_LAYER_COUNT) && layer_idx >= TOWER_TOP_LAYER_COUNT + bottom_empty_layer_count {
                        let mut tiny_tower_here = Shape::default();
                        tiny_tower_here.push_all(&polygon_part);
                        tower_roofs.push(tiny_tower_here);
                    }
                }
            }

            if is_support_mesh_drop_down_place_holder && !storage.support.support_layers[layer_idx].support_mesh_drop_down.is_empty() {
                // handle support mesh which should be supported by more support
                layer_this = layer_this.union_polygons_other(&storage.support.support_layers[layer_idx].support_mesh_drop_down);
            }

            // Move up from model, handle stair-stepping.
            Self::move_up_from_model(
                storage,
                &mut stair_removal,
                &sloped_areas_per_layer[layer_idx],
                &mut layer_this,
                layer_idx,
                bottom_empty_layer_count,
                bottom_stair_step_layer_count,
                bottom_stair_step_width,
            );

            support_areas[layer_idx] = layer_this;
            layer_idx = layer_idx.wrapping_sub(1);
        }

        // Removal of the x/y distance needs to be outside the main loop
        for (support_layer, xy_disallowed_area) in support_areas.iter_mut().zip(xy_disallowed_per_layer.iter()) {
            // inset using X/Y distance
            if !support_layer.is_empty() && !xy_disallowed_area.is_empty() {
                *support_layer = support_layer.difference(xy_disallowed_area);
            }

            // Perform close operation to remove areas from support area that are unprintable
            *support_layer = support_layer.offset_default(-half_min_feature_width).offset_default(half_min_feature_width);

            // remove areas smaller than the minimum support area
            support_layer.remove_small_areas(minimum_support_area, false);
        }

        // do stuff for when support on buildplate only
        if support_type == ESupportType::PLATFORM_ONLY {
            let mut touching_buildplate = support_areas[0].clone(); // TODO: not working for conical support!
            let conical_support_angle = infill_settings.get_angle_radians("support_conical_angle");
            let conical_support_offset: Coord = if conical_support_angle.value() > 0.0 {
                // outward ==> wider base than overhang
                (-bounded_tan(conical_support_angle) * layer_thickness as f64) as Coord
            } else {
                // inward ==> smaller base than overhang
                (bounded_tan(AngleRadians::new(-conical_support_angle.value())) * layer_thickness as f64) as Coord
            };
            let conical_support = infill_settings.get_bool("support_conical_enabled") && conical_support_angle.value() != 0.0;
            for layer_idx in 1..storage.support.support_layers.len() {
                let layer = &support_areas[layer_idx];

                if conical_support {
                    // with conical support the next layer is allowed to be larger than the previous
                    touching_buildplate = touching_buildplate.offset(conical_support_offset.abs() + 10, JoinType::Miter, 10.0);
                }

                touching_buildplate = layer.intersection(&touching_buildplate); // from bottom to top, support areas can only decrease!

                support_areas[layer_idx] = touching_buildplate.clone();
            }
        }

        // Enforce top Z distance.
        if layer_z_distance_top > 1 {
            // this is performed after the main support generation loop above, because it affects the joining of polygons
            let max_checking_layer_idx =
                0i32.max((storage.support.support_layers.len() as i32).min(layer_count as i32 - (layer_z_distance_top as i32 - 1)));

            let sa = UnsafeSlice::new(support_areas);
            let storage_ref: &SliceDataStorage = storage;
            parallel_for(0, max_checking_layer_idx as usize, |layer_idx| {
                const NO_SUPPORT_HERE: bool = false;
                const NO_PRIME_TOWER_HERE: bool = false;
                let outlines = storage_ref.get_layer_outlines(
                    (layer_idx + layer_z_distance_top - 1) as LayerIndex,
                    NO_SUPPORT_HERE,
                    NO_PRIME_TOWER_HERE,
                    false,
                    -1,
                    true,
                );
                // SAFETY: each layer index is written by exactly one iteration.
                unsafe {
                    let s = sa.get_mut(layer_idx);
                    *s = s.difference(&outlines);
                }
            });
        }

        // Procedure to remove floating support
        for layer_idx in 1..layer_count.saturating_sub(1) {
            if !support_areas[layer_idx].is_empty() {
                let surrounding_layer = support_areas[layer_idx + 1].union_polygons_other(&support_areas[layer_idx - 1]);
                support_areas[layer_idx] = support_areas[layer_idx].intersection(&surrounding_layer);
            }
        }

        let stop = (-1i32).max(storage.support.layer_nr_max_filled_layer) as usize;
        let mut layer_idx = support_areas.len().wrapping_sub(1);
        while layer_idx != stop {
            if !support_areas[layer_idx].is_empty() {
                storage.support.layer_nr_max_filled_layer = layer_idx as i32;
                break;
            }
            layer_idx = layer_idx.wrapping_sub(1);
        }

        storage.support.generated = true;
    }

    /// Remove the support areas directly above the model (bottom distance) and handle stair-stepping.
    #[allow(clippy::too_many_arguments)]
    fn move_up_from_model(
        storage: &SliceDataStorage,
        stair_removal: &mut Shape,
        sloped_areas: &Shape,
        support_areas: &mut Shape,
        layer_idx: usize,
        bottom_empty_layer_count: usize,
        bottom_stair_step_layer_count: usize,
        support_bottom_stair_step_width: Coord,
    ) {
        if layer_idx < bottom_empty_layer_count {
            return;
        }
        if bottom_empty_layer_count == 0 && bottom_stair_step_layer_count <= 1 {
            return;
        }

        let bottom_layer_nr = layer_idx - bottom_empty_layer_count;
        const NO_SUPPORT: bool = false;
        const NO_PRIME_TOWER: bool = false;
        let bottom_outline = storage.get_layer_outlines(bottom_layer_nr as LayerIndex, NO_SUPPORT, NO_PRIME_TOWER, false, -1, true);

        let to_be_removed: Shape;
        if bottom_stair_step_layer_count <= 1 {
            to_be_removed = bottom_outline;
        } else {
            to_be_removed = stair_removal.union_polygons_other(&bottom_outline);
            if layer_idx % bottom_stair_step_layer_count == 0 {
                // update stairs for next step
                let supporting_bottom =
                    storage.get_layer_outlines(bottom_layer_nr.wrapping_sub(1) as i64 as LayerIndex, NO_SUPPORT, NO_PRIME_TOWER, false, -1, true);
                let allowed_step_width = supporting_bottom.offset_default(support_bottom_stair_step_width).intersection(sloped_areas);

                let step_bottom_layer_nr = bottom_layer_nr as i64 - bottom_stair_step_layer_count as i64 + 1;
                if step_bottom_layer_nr >= 0 {
                    let step_bottom_outline = storage.get_layer_outlines(step_bottom_layer_nr, NO_SUPPORT, NO_PRIME_TOWER, false, -1, true);
                    *stair_removal = step_bottom_outline.intersection(&allowed_step_width);
                } else {
                    *stair_removal = allowed_step_width;
                }
            }
        }
        *support_areas = support_areas.difference(&to_be_removed);
    }

    /// Compute the basic overhang (without the tangent offset) and the full overhang of a layer.
    pub fn compute_basic_and_full_overhang(storage: &SliceDataStorage, mesh: &SliceMeshStorage, layer_idx: LayerIndex) -> (Shape, Shape) {
        let outlines = mesh.layers[layer_idx as usize].get_outlines(false);
        const NO_SUPPORT: bool = false;
        const NO_PRIME_TOWER: bool = false;

        const SMOOTH_HEIGHT: f64 = 0.4; // mm
        let layers_below: LayerIndex = cppstd::round(SMOOTH_HEIGHT / mesh.settings().get_f64("layer_height")) as LayerIndex;

        let layer_height = mesh.settings().get_coord("layer_height");
        let support_angle = mesh.settings().get_angle_radians("support_angle");
        let tan_angle = bounded_tan(support_angle); // The X/Y component of the support angle
        // overhang areas protruding less then `max_dist_from_lower_layer` don't need support
        let max_dist_from_lower_layer = (tan_angle * layer_height as f64) as Coord; // Maximum horizontal distance that can be bridged.

        // To avoids generating support for textures on vertical surfaces, a moving average is taken over smooth_height.
        let mut outlines_below = storage.get_layer_outlines(layer_idx - 1, NO_SUPPORT, NO_PRIME_TOWER, false, -1, true).offset_default(max_dist_from_lower_layer);
        let mut layer_idx_offset: i32 = 2;
        while layer_idx - layer_idx_offset as LayerIndex >= 0 && layer_idx_offset as LayerIndex <= layers_below {
            let outlines_below_ = storage
                .get_layer_outlines(layer_idx - layer_idx_offset as LayerIndex, NO_SUPPORT, NO_PRIME_TOWER, false, -1, true)
                .offset_default(max_dist_from_lower_layer * layer_idx_offset as Coord);
            outlines_below = outlines_below.union_polygons_other(&outlines_below_);
            layer_idx_offset += 1;
        }

        let mut basic_overhang = outlines.difference(&outlines_below);

        let support_layer = &storage.support.support_layers[layer_idx as usize];
        if !support_layer.anti_overhang.is_empty() {
            // Merge anti overhang into one polygon, otherwise overlapping polygons will create opposite effect.
            let merged_polygons = support_layer.anti_overhang.union_polygons();

            basic_overhang = basic_overhang.difference(&merged_polygons);
        }

        // +0.1mm for easier joining with support from layer above
        let overhang_extended = basic_overhang.offset_default(max_dist_from_lower_layer * layers_below as Coord + mm2int(0.1));
        let full_overhang = overhang_extended.intersection(&outlines);

        (basic_overhang, full_overhang)
    }

    /// Detect the overhang points of a mesh (small parts without anything below, for towers).
    fn detect_overhang_points(storage: &mut SliceDataStorage, mesh_idx: usize) {
        let settings = storage.meshes[mesh_idx].settings();
        let max_tower_supported_diameter = settings.get_coord("support_tower_maximum_supported_diameter");
        let max_tower_supported_area = max_tower_supported_diameter * max_tower_supported_diameter;

        let print_layer_count = storage.print_layer_count as usize;
        storage.meshes[mesh_idx].overhang_points.resize_with(print_layer_count, Vec::new);

        for layer_idx in 1..print_layer_count {
            let mut new_points: Vec<Shape> = Vec::new();
            {
                let mesh = &storage.meshes[mesh_idx];
                let layer = &mesh.layers[layer_idx];
                let layer_below = &mesh.layers[layer_idx - 1];

                for part in &layer.parts {
                    if part.outline.0.is_empty() {
                        continue;
                    }
                    if part.outline.outer_polygon().area() >= max_tower_supported_area as f64 {
                        // area is too big for support towers, should be supported by normal overhang detection
                        continue;
                    }

                    let has_support_below = !layer_below.get_outlines(false).intersection(&part.outline.0).is_empty();
                    if has_support_below {
                        continue;
                    }

                    let overhang = part.outline.0.difference(&storage.support.support_layers[layer_idx].anti_overhang);
                    if !overhang.is_empty() {
                        new_points.push(overhang);
                    }
                }
            }
            storage.meshes[mesh_idx].overhang_points[layer_idx].extend(new_points);
        }
    }

    /// Adds tower pieces to the current support layer.
    fn handle_towers(
        settings: &Settings,
        xy_disallowed_area: &Shape,
        support_layer_this: &mut Shape,
        tower_roofs: &mut Vec<Shape>,
        overhang_points: &mut [Vec<Shape>],
        layer_idx: LayerIndex,
        layer_count: usize,
    ) {
        let layer_overhang_point = layer_idx + 1; // Start tower 1 layer below overhang point.
        if layer_overhang_point >= layer_count as LayerIndex - 1 {
            return;
        }
        // handle new tower rooftops
        if !overhang_points[layer_overhang_point as usize].is_empty() {
            // make sure we have the lowest point (make polys empty if they have small parts below)
            if layer_overhang_point < layer_count as LayerIndex && !overhang_points[layer_overhang_point as usize - 1].is_empty() {
                let max_tower_supported_diameter = settings.get_coord("support_tower_maximum_supported_diameter");
                let (below, here) = overhang_points.split_at_mut(layer_overhang_point as usize);
                let overhang_points_below = &below[layer_overhang_point as usize - 1];
                for poly_here in here[0].iter_mut() {
                    for poly_below in overhang_points_below {
                        *poly_here = poly_here.difference(&poly_below.offset_default(max_tower_supported_diameter * 2));
                    }
                }
            }
            for poly in &overhang_points[layer_overhang_point as usize] {
                if !poly.is_empty() {
                    tower_roofs.push(poly.clone());
                }
            }
        }

        // make tower roofs
        let layer_thickness = settings.get_coord("layer_height");
        let tower_roof_angle = settings.get_angle_radians("support_tower_roof_angle");
        let tower_diameter = settings.get_coord("support_tower_diameter");
        let support_line_width = settings.get_coord("support_line_width");

        let tower_roof_expansion_distance: Coord = if tower_roof_angle.value() == 0.0 {
            // maximum expansion distance needed to reach a tower_diameter^2 tower area
            tower_diameter / 2
        } else {
            let tan_tower_roof_angle = bounded_tan(tower_roof_angle);
            (layer_thickness as f64 / tan_tower_roof_angle) as Coord
        };

        for tower_roof in tower_roofs.iter_mut() {
            if tower_roof.is_empty() {
                continue;
            }
            *support_layer_this = support_layer_this.union_polygons_other(tower_roof);

            if tower_roof.area() < (tower_diameter * tower_diameter) as f64 {
                let mut model_outline = xy_disallowed_area.clone();

                // Rather than offsetting the tower with tower_roof_expansion_distance we do this step wise
                let offset_per_step = (support_line_width / 2).max(tower_roof_expansion_distance);
                let mut offset_cumulative: Coord = 0;
                while offset_cumulative <= tower_roof_expansion_distance {
                    *tower_roof = tower_roof.offset_join(offset_per_step, JoinType::Round);
                    model_outline = model_outline.difference(tower_roof);
                    model_outline = model_outline.offset_join(offset_per_step, JoinType::Round);
                    *tower_roof = tower_roof.difference(&model_outline);

                    if tower_roof.is_empty() {
                        // guard against the tower_roof getting empty
                        break;
                    }
                    if tower_roof.area() >= (tower_diameter * tower_diameter) as f64 {
                        // the desired size of the roof tower is reached, add the support tower to the
                        // current layer and clear the support tower itself
                        *support_layer_this = support_layer_this.union_polygons_other(tower_roof);
                        tower_roof.clear();
                        break;
                    }
                    offset_cumulative += offset_per_step;
                }
            } else {
                tower_roof.clear();
            }
        }
    }

    /// Adds struts (towers against a wall) to the current layer.
    fn handle_wall_struts(settings: &Settings, support_layer_this: &mut Shape) {
        let max_tower_supported_diameter = settings.get_coord("support_tower_maximum_supported_diameter");
        let tower_diameter = settings.get_coord("support_tower_diameter");
        let mut p: usize = 0;
        while p < support_layer_this.len() {
            let poly = support_layer_this[p].clone();
            if poly.len() < 6 {
                // might be a single wall
                let mut best: i32 = -1;
                let mut best_length2: i32 = -1;
                for i in 0..poly.len() {
                    let length2 = v_size2(poly[i] - poly[(i + 1) % poly.len()]) as i32;
                    if length2 > best_length2 {
                        best = i as i32;
                        best_length2 = length2;
                    }
                }

                if (best_length2 as Coord) < max_tower_supported_diameter * max_tower_supported_diameter {
                    break; // this is a small area, not a wall!
                }

                // an estimate of the width of the area
                let area = poly.area();
                let width = (area * area / best_length2 as f64).sqrt() as i32; // sqrt (a^2 / l^2) instead of a / sqrt(l^2)

                // add square tower (strut) in the middle of the wall
                if (width as Coord) < max_tower_supported_diameter {
                    let best = best as usize;
                    let mid = (poly[best] + poly[(best + 1) % poly.len()]) / 2;
                    let mut struts = Shape::default();
                    let mut strut = Polygon::default();
                    strut.points.push(mid + pt(tower_diameter / 2, tower_diameter / 2));
                    strut.points.push(mid + pt(-tower_diameter / 2, tower_diameter / 2));
                    strut.points.push(mid + pt(-tower_diameter / 2, -tower_diameter / 2));
                    strut.points.push(mid + pt(tower_diameter / 2, -tower_diameter / 2));
                    struts.push(strut);
                    *support_layer_this = support_layer_this.union_polygons_other(&struts);
                }
            }
            p += 1;
        }
    }

    /// Generate the support bottoms (interface between support and model below).
    fn generate_support_bottom(storage: &mut SliceDataStorage, mesh_idx: usize, global_support_areas_per_layer: &mut [Shape]) {
        let mesh_group_settings = &current_slice().scene.current_mesh_group().settings;
        let mesh_settings = storage.meshes[mesh_idx].settings();
        let layer_height = mesh_group_settings.get_coord("layer_height");
        let bottom_layer_count = round_divide(mesh_settings.get_coord("support_bottom_height") as u64, layer_height as u64) as usize; // Number of layers in support bottom.
        if bottom_layer_count == 0 {
            return;
        }
        // Number of layers between support bottom and model.
        let z_distance_bottom = round_up_divide(mesh_settings.get_coord("support_bottom_distance") as u64, layer_height as u64) as Coord;
        let bottom_line_width = mesh_group_settings.get_extruder_train("support_bottom_extruder_nr").settings.get_coord("support_bottom_line_width");
        let bottom_outline_offset = mesh_group_settings.get_extruder_train("support_bottom_extruder_nr").settings.get_coord("support_bottom_offset");

        let minimum_bottom_area = mesh_settings.get_f64("minimum_bottom_area");

        let mut layer_idx: LayerIndex = storage.support.support_layers.len() as LayerIndex - 1;
        while layer_idx >= z_distance_bottom {
            let bottom_layer_idx_below = 0.max(layer_idx as i32 - bottom_layer_count as i32 - z_distance_bottom as i32) as u32;
            let mut mesh_outlines = Shape::default();
            let mut layer_idx_below = bottom_layer_idx_below as LayerIndex;
            while layer_idx_below < layer_idx - z_distance_bottom + 1 {
                mesh_outlines.push_all(&storage.meshes[mesh_idx].layers[layer_idx_below as usize].get_outlines(false));
                layer_idx_below += 1;
            }
            let mut bottoms = Shape::default();
            Self::generate_support_interface_layer(
                &mut global_support_areas_per_layer[layer_idx as usize],
                &mesh_outlines,
                bottom_line_width,
                bottom_outline_offset,
                minimum_bottom_area,
                &mut bottoms,
            );
            storage.support.support_layers[layer_idx as usize].support_bottom.push_all(&bottoms);
            layer_idx -= 1;
        }
    }

    /// Generate the support roofs (interface between support and model above).
    fn generate_support_roof(storage: &mut SliceDataStorage, mesh_idx: usize, global_support_areas_per_layer: &mut [Shape]) {
        let mesh_group_settings = &current_slice().scene.current_mesh_group().settings;
        let mesh_settings = storage.meshes[mesh_idx].settings();
        let layer_height = mesh_group_settings.get_coord("layer_height");
        let roof_layer_count = round_divide(mesh_settings.get_coord("support_roof_height") as u64, layer_height as u64) as usize; // Number of layers in support roof.
        if roof_layer_count == 0 {
            return;
        }
        let support_top_distance = mesh_settings.get_coord("support_top_distance");
        let z_distance_top = round_up_divide(support_top_distance as u64, layer_height as u64) as Coord; // Number of layers between support roof and model.
        let roof_line_width = mesh_group_settings.get_extruder_train("support_roof_extruder_nr").settings.get_coord("support_roof_line_width");
        let roof_outline_offset = mesh_group_settings.get_extruder_train("support_roof_extruder_nr").settings.get_coord("support_roof_offset");

        let minimum_roof_area = mesh_settings.get_f64("minimum_roof_area");

        let n_support_layers = storage.support.support_layers.len();
        let mut layer_idx: LayerIndex = (n_support_layers as i64 - z_distance_top) as i32 as LayerIndex - 1;
        while layer_idx >= 0 {
            // Maximum layer of the model that generates support roof.
            let top_layer_idx_above: LayerIndex = (n_support_layers as LayerIndex - 1).min(layer_idx + roof_layer_count as LayerIndex + z_distance_top);
            let mut mesh_outlines = Shape::default();
            let mut layer_idx_above = top_layer_idx_above;
            while layer_idx_above > layer_idx + z_distance_top - 1 {
                mesh_outlines.push_all(&storage.meshes[mesh_idx].layers[layer_idx_above as usize].get_outlines(false));
                layer_idx_above -= 1;
            }
            let mut roofs = Shape::default();
            Self::generate_support_interface_layer(
                &mut global_support_areas_per_layer[layer_idx as usize],
                &mesh_outlines,
                roof_line_width,
                roof_outline_offset,
                minimum_roof_area,
                &mut roofs,
            );
            storage.support.support_layers[layer_idx as usize].support_roof.push_all(&roofs);
            if layer_idx > 0 && (layer_idx as usize) < n_support_layers - 1 && support_top_distance % layer_height != 0 {
                let fractional = roofs.difference(&storage.support.support_layers[layer_idx as usize + 1].support_roof);
                storage.support.support_layers[layer_idx as usize].support_fractional_roof.push_all(&fractional);
            }
            layer_idx -= 1;
        }

        // Remove support in between the support roof and the model. Subtracts the roof polygons from the support polygons on the layers above it.
        let end = n_support_layers.saturating_sub(z_distance_top as usize);
        for layer_idx in 1..end {
            let support_layer = &storage.support.support_layers[layer_idx];
            if support_layer.support_roof.is_empty() {
                continue;
            }

            let lower = layer_idx as i32;
            let upper = (layer_idx as i32 + roof_layer_count as i32 + z_distance_top as i32 + 5).min(global_support_areas_per_layer.len() as i32 - 1);
            let mut i = lower;
            while i < upper {
                global_support_areas_per_layer[i as usize] = global_support_areas_per_layer[i as usize].difference(&support_layer.support_roof);
                i += 1;
            }
        }
    }

    /// Generate a single layer of support interface.
    fn generate_support_interface_layer(
        support_areas: &mut Shape,
        colliding_mesh_outlines: &Shape,
        safety_offset: Coord,
        outline_offset: Coord,
        minimum_interface_area: f64,
        interface_polygons: &mut Shape,
    ) {
        let model = colliding_mesh_outlines.union_polygons();
        *interface_polygons = support_areas.offset_default(safety_offset / 2).intersection(&model);
        *interface_polygons = interface_polygons.offset_default(safety_offset).intersection(support_areas); // Make sure we don't generate any models that are not printable.
        if outline_offset != 0 {
            *interface_polygons = interface_polygons.offset_default(outline_offset);
            if outline_offset > 0 {
                // The interface might exceed the area of the normal support.
                *interface_polygons = interface_polygons.intersection(support_areas);
            }
        }
        if minimum_interface_area > 0.0 {
            interface_polygons.remove_small_areas(minimum_interface_area, false);
        }
        *support_areas = support_areas.difference(interface_polygons);
    }
}

#[allow(dead_code)]
fn _unused(_: SingleShape, _: Point2LL) {}
