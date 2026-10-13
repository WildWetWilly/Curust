//! `layerPart.h/.cpp`: split the sliced polygons of each layer into connected parts.

use crate::geometry::SingleShape;
use crate::settings::enums::ESurfaceMode;
use crate::settings::settings::Settings;
use crate::settings::types::LayerIndex;
use crate::slice_data_storage::{SliceLayer, SliceLayerPart, SliceMeshStorage};
use crate::slicer::{Slicer, SlicerLayer};
use crate::utils::polyline_stitcher::stitch_open_into_shape;
use crate::utils::simplify::Simplify;
use crate::utils::thread_pool::{parallel_for, UnsafeSlice};
use crate::geometry::Polyline;

/// Split a layer into parts.
pub fn create_layer_with_parts(settings: &Settings, storage_layer: &mut SliceLayer, layer: &mut SlicerLayer) {
    stitch_open_into_shape(&layer.open_polylines, &mut storage_layer.open_polylines, &mut layer.polygons, settings.get_coord("wall_line_width_0"), 10);

    storage_layer.open_polylines = Simplify::from_settings(settings).polylines(&storage_layer.open_polylines);

    let union_all_remove_holes = settings.get_bool("meshfix_union_all_remove_holes");
    if union_all_remove_holes {
        for i in 0..layer.polygons.len() {
            if layer.polygons[i].orientation() {
                layer.polygons[i].points.reverse();
            }
        }
    }

    let mut result: Vec<SingleShape>;
    let union_layers = settings.get_bool("meshfix_union_all");
    let surface_only = settings.get_enum::<ESurfaceMode>("magic_mesh_surface_mode");
    if surface_only == ESurfaceMode::SURFACE && !union_layers {
        // Don't do anything with overlapping areas; no union nor xor
        result = Vec::with_capacity(layer.polygons.len());
        for poly in layer.polygons.iter() {
            if poly.is_empty() {
                continue;
            }
            let mut part = SingleShape::default();
            part.0.push(poly.clone());
            result.push(part);
        }
    } else {
        result = layer.polygons.split_into_parts(union_layers || union_all_remove_holes);
    }

    for part in result {
        storage_layer.parts.push(SliceLayerPart::default());
        if part.0.is_empty() {
            continue;
        }
        let back = storage_layer.parts.last_mut().unwrap();
        back.outline = part;
        back.boundary_box.calculate_shape(&back.outline.0);
        if back.outline.0.is_empty() {
            storage_layer.parts.pop();
        }
    }
}

/// Split all layers of a mesh into parts.
pub fn create_layer_parts(mesh: &mut SliceMeshStorage, slicer: &mut Slicer) {
    let total_layers = slicer.layers.len();
    debug_assert!(mesh.layers.len() == total_layers);

    let settings = mesh.settings();
    {
        let mesh_layers = UnsafeSlice::new(mesh.layers.as_mut_slice());
        let slicer_layers = UnsafeSlice::new(slicer.layers.as_mut_slice());
        parallel_for(0, total_layers, |layer_nr| {
            // SAFETY: each index is processed exactly once.
            let layer_storage = unsafe { mesh_layers.get_mut(layer_nr) };
            let slice_layer = unsafe { slicer_layers.get_mut(layer_nr) };
            create_layer_with_parts(settings, layer_storage, slice_layer);
        });
    }

    let mut layer_nr = total_layers as LayerIndex - 1;
    while layer_nr >= 0 {
        let layer_storage = &mesh.layers[layer_nr as usize];
        if !layer_storage.parts.is_empty()
            || (settings.get_enum::<ESurfaceMode>("magic_mesh_surface_mode") != ESurfaceMode::NORMAL && !layer_storage.open_polylines.is_empty())
        {
            mesh.layer_nr_max_filled_layer = layer_nr; // last set by the highest non-empty layer
            break;
        }
        layer_nr -= 1;
    }
}

