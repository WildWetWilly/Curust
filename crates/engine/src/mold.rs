//! `Mold.h/.cpp`: turn meshes into molds.

use crate::application::current_slice_mut;
use crate::geometry::Shape;
use crate::settings::types::LayerIndex;
use crate::slicer::Slicer;
use clipper::{JoinType, PolyFillType};

pub struct Mold;

impl Mold {
    /// Convert the polygons of all meshes with mold_enabled into molds.
    pub fn process(slicer_list: &mut [Slicer]) {
        let scene = &mut current_slice_mut().scene;
        {
            // check whether we even need to process molds
            let mut has_any_mold = false;
            for mesh_idx in 0..slicer_list.len() {
                let mesh = &mut scene.current_mesh_group_mut().meshes[mesh_idx];
                if mesh.settings.get_bool("mold_enabled") {
                    has_any_mold = true;
                    let w = mesh.settings.get_coord("mold_width");
                    mesh.expand_xy(w);
                }
            }
            if !has_any_mold {
                return;
            }
        }

        let mut layer_count: LayerIndex = 0;
        {
            // compute layer_count
            for slicer in slicer_list.iter() {
                let layer_count_here = slicer.layers.len() as LayerIndex;
                layer_count = layer_count.max(layer_count_here);
            }
        }

        let layer_height = scene.current_mesh_group().settings.get_coord("layer_height");
        let mut mold_outline_above_per_mesh: Vec<Shape> = vec![Shape::default(); slicer_list.len()]; // the outer outlines of the layer above without the original model(s) being cut out
        let mut layer_nr = layer_count as i32 - 1;
        while layer_nr >= 0 {
            let mut all_original_mold_outlines = Shape::default(); // outlines of all models for which to generate a mold (insides of all molds)

            // first generate outlines
            for mesh_idx in 0..slicer_list.len() {
                let mesh = &scene.current_mesh_group().meshes[mesh_idx];
                let slicer = &mut slicer_list[mesh_idx];
                if !mesh.settings.get_bool("mold_enabled") || layer_nr >= slicer.layers.len() as i32 {
                    continue;
                }
                let width = mesh.settings.get_coord("mold_width");
                let mut open_polyline_width = mesh.settings.get_coord("wall_line_width_0");
                if layer_nr == 0 {
                    let train_wall_0 = mesh.settings.get_extruder_train("wall_0_extruder_nr");
                    open_polyline_width = (open_polyline_width as f64 * train_wall_0.settings.get_ratio("initial_layer_line_width_factor")) as i64;
                }
                let angle = mesh.settings.get_angle_degrees("mold_angle");
                let roof_height = mesh.settings.get_coord("mold_roof_height");

                let inset = ((angle.value() / 180.0 * std::f64::consts::PI).tan() * layer_height as f64) as i64;
                let roof_layer_count = (roof_height / layer_height) as usize;

                let layer = &mut slicer.layers[layer_nr as usize];
                let model_outlines = layer
                    .polygons
                    .union_polygons_with(&layer.open_polylines.offset(open_polyline_width / 2, JoinType::Miter, 1.2), PolyFillType::NonZero);
                layer.open_polylines.clear();
                all_original_mold_outlines.push_all(&model_outlines);

                if angle.value() >= 90.0 {
                    layer.polygons = model_outlines.offset(width, JoinType::Round, 1.2);
                } else {
                    let mold_outline_above = &mold_outline_above_per_mesh[mesh_idx]; // the outside of the mold on the layer above
                    layer.polygons = mold_outline_above
                        .offset(-inset, JoinType::Miter, 1.2)
                        .union_polygons_with(&model_outlines.offset(width, JoinType::Round, 1.2), PolyFillType::NonZero);
                }

                // add roofs
                if roof_layer_count > 0 && layer_nr > 0 {
                    let layer_nr_below = 0i32.max((layer_nr as u32).wrapping_sub(roof_layer_count as u32) as i32);
                    let roofs = slicer.layers[layer_nr_below as usize].polygons.offset(width, JoinType::Round, 1.2); // TODO: don't compute offset twice!
                    let layer = &mut slicer.layers[layer_nr as usize];
                    layer.polygons = layer.polygons.union_polygons_with(&roofs, PolyFillType::NonZero);
                }

                mold_outline_above_per_mesh[mesh_idx] = slicer.layers[layer_nr as usize].polygons.clone();
            }
            all_original_mold_outlines = all_original_mold_outlines.union_polygons();

            // cut out molds from all objects after generating mold outlines for all objects so that molds won't overlap into the casting cutout of another mold

            // carve molds out of all other models
            for mesh_idx in 0..slicer_list.len() {
                let mesh = &scene.current_mesh_group().meshes[mesh_idx];
                if !mesh.settings.get_bool("mold_enabled") {
                    continue; // only cut original models out of all molds
                }
                let layer = &mut slicer_list[mesh_idx].layers[layer_nr as usize];
                layer.polygons = layer.polygons.difference(&all_original_mold_outlines);
            }
            layer_nr -= 1;
        }
    }
}
