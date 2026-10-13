//! `multiVolumes.h/.cpp`: interactions between overlapping meshes.

use crate::application::{current_slice, current_slice_mut};
use crate::geometry::{OpenLinesSet, Shape};
use crate::mesh::Mesh;
use crate::settings::enums::ESurfaceMode;
use crate::settings::types::LayerIndex;
use crate::slicer::Slicer;
use crate::utils::polyline_stitcher::stitch_open_into_shape;
use clipper::{JoinType, PolyFillType};
use std::collections::HashMap;

/// Carve all volumes out of each other according to their `infill_mesh_order`.
pub fn carve_multiple_volumes(volumes: &mut [Slicer]) {
    // Go trough all the volumes, and remove the previous volume outlines from our own outline, so we never have overlapped areas.
    let alternate_carve_order = current_slice().scene.current_mesh_group().settings.get_bool("alternate_carve_order");
    let mut ranked_volumes: Vec<usize> = (0..volumes.len()).collect();
    ranked_volumes.sort_by(|&a, &b| {
        let ka = volumes[a].mesh().settings.get_int("infill_mesh_order");
        let kb = volumes[b].mesh().settings.get_int("infill_mesh_order");
        ka.cmp(&kb)
    }); // stable sort
    for volume_1_idx in 1..volumes.len() {
        let v1 = ranked_volumes[volume_1_idx];
        {
            let m1 = volumes[v1].mesh();
            if m1.settings.get_bool("infill_mesh")
                || m1.settings.get_bool("anti_overhang_mesh")
                || m1.settings.get_bool("support_mesh")
                || m1.settings.get_enum::<ESurfaceMode>("magic_mesh_surface_mode") == ESurfaceMode::SURFACE
            {
                continue;
            }
        }
        for volume_2_idx in 0..volume_1_idx {
            let v2 = ranked_volumes[volume_2_idx];
            let (m1, m2) = (volumes[v1].mesh(), volumes[v2].mesh());
            if m2.settings.get_bool("infill_mesh")
                || m2.settings.get_bool("anti_overhang_mesh")
                || m2.settings.get_bool("support_mesh")
                || m2.settings.get_enum::<ESurfaceMode>("magic_mesh_surface_mode") == ESurfaceMode::SURFACE
            {
                continue;
            }
            if !m1.get_aabb().hit(&m2.get_aabb()) {
                continue;
            }
            let same_order = m1.settings.get_int("infill_mesh_order") == m2.settings.get_int("infill_mesh_order");
            for layer_nr in 0..volumes[v1].layers.len() {
                if alternate_carve_order && (layer_nr as LayerIndex) % 2 == 0 && same_order {
                    let p1 = volumes[v1].layers[layer_nr].polygons.clone();
                    let layer2 = &mut volumes[v2].layers[layer_nr];
                    layer2.polygons = layer2.polygons.difference(&p1);
                } else {
                    let p2 = volumes[v2].layers[layer_nr].polygons.clone();
                    let layer1 = &mut volumes[v1].layers[layer_nr];
                    layer1.polygons = layer1.polygons.difference(&p2);
                }
            }
        }
    }
}

/// Expand each layer a bit and then keep the extra overlapping parts that overlap with other volumes.
pub fn generate_multiple_volumes_overlap(volumes: &mut [Slicer]) {
    if volumes.len() < 2 {
        return;
    }

    let offset_to_merge_other_merged_volumes: i64 = 20;
    for vi in 0..volumes.len() {
        let mesh = volumes[vi].mesh();
        let fill_type = if mesh.settings.get_bool("meshfix_union_all") { PolyFillType::NonZero } else { PolyFillType::EvenOdd };
        let overlap = mesh.settings.get_coord("multiple_mesh_overlap");
        if mesh.settings.get_bool("infill_mesh") || mesh.settings.get_bool("anti_overhang_mesh") || mesh.settings.get_bool("support_mesh") || overlap == 0 {
            continue;
        }
        let mut aabb = mesh.get_aabb();
        aabb.expand_xy(overlap);
        for layer_nr in 0..volumes[vi].layers.len() {
            let mut all_other_volumes = Shape::default();
            for oi in 0..volumes.len() {
                let other_mesh = volumes[oi].mesh();
                if other_mesh.settings.get_bool("infill_mesh")
                    || other_mesh.settings.get_bool("anti_overhang_mesh")
                    || other_mesh.settings.get_bool("support_mesh")
                    || !other_mesh.get_aabb().hit(&aabb)
                    || oi == vi
                {
                    continue;
                }
                let other_volume_layer = &volumes[oi].layers[layer_nr];
                all_other_volumes = all_other_volumes
                    .union_polygons_with(&other_volume_layer.polygons.offset(offset_to_merge_other_merged_volumes, JoinType::Miter, 1.2), fill_type);
            }

            let volume_layer = &mut volumes[vi].layers[layer_nr];
            volume_layer.polygons = volume_layer
                .polygons
                .union_polygons_with(&all_other_volumes.intersection(&volume_layer.polygons.offset(overlap / 2, JoinType::Miter, 1.2)), fill_type);
        }
    }
}

pub struct MultiVolumes;

impl MultiVolumes {
    /// Carve cutting meshes from the other meshes and limit them to the other meshes.
    pub fn carve_cutting_meshes(volumes: &mut [Slicer]) {
        let meshes: &mut Vec<Mesh> = &mut current_slice_mut().scene.current_mesh_group_mut().meshes;
        let mut has_extruder_change_mesh = false;
        for cutting_mesh in meshes.iter() {
            has_extruder_change_mesh |= cutting_mesh.settings.get_bool("cutting_mesh") && cutting_mesh.settings.has("extruder_nr");
        }

        let mut layer_printable_mesh_unions: HashMap<LayerIndex, Shape> = HashMap::new();
        if has_extruder_change_mesh {
            // Calculate the union of all printable meshes for each layer
            for printable_mesh_idx in 0..meshes.len() {
                if !meshes[printable_mesh_idx].is_printed() {
                    continue;
                }
                let printable_mesh_volume = &volumes[printable_mesh_idx];
                for layer_nr in 0..printable_mesh_volume.layers.len() as LayerIndex {
                    let printable_mesh_area = &printable_mesh_volume.layers[layer_nr as usize].polygons;
                    if let Some(existing) = layer_printable_mesh_unions.get(&layer_nr) {
                        let u = existing.union_polygons_with(printable_mesh_area, PolyFillType::NonZero);
                        layer_printable_mesh_unions.insert(layer_nr, u);
                    } else {
                        layer_printable_mesh_unions.insert(layer_nr, printable_mesh_area.clone());
                    }
                }
            }
        }

        for carving_mesh_idx in 0..volumes.len() {
            if !meshes[carving_mesh_idx].settings.get_bool("cutting_mesh") {
                continue;
            }
            let is_extruder_change_mesh = meshes[carving_mesh_idx].settings.has("extruder_nr");
            let surface_mode = meshes[carving_mesh_idx].settings.get_enum::<ESurfaceMode>("magic_mesh_surface_mode");
            let surface_line_width = meshes[carving_mesh_idx].settings.get_coord("wall_line_width_0");
            let n_layers = volumes[carving_mesh_idx].layers.len();
            for layer_nr in 0..n_layers {
                let mut cutting_mesh_polygons = std::mem::take(&mut volumes[carving_mesh_idx].layers[layer_nr].polygons);
                let mut cutting_mesh_polylines = std::mem::take(&mut volumes[carving_mesh_idx].layers[layer_nr].open_polylines);
                let mut cutting_mesh_area_recomputed = Shape::default();
                // `cutting_mesh_area` points either to the recomputed area or to the polygons themselves.
                let mut area_is_polygons = false;
                {
                    if surface_mode == ESurfaceMode::BOTH {
                        cutting_mesh_area_recomputed = cutting_mesh_polygons
                            .union_polygons_with(&cutting_mesh_polylines.offset(surface_line_width / 2, JoinType::Miter, 1.2), PolyFillType::NonZero);
                    } else if surface_mode == ESurfaceMode::SURFACE {
                        // break up polygons into polylines
                        // they have to be polylines, because they might break up further when doing the cutting
                        for poly in cutting_mesh_polygons.iter_mut() {
                            let front = poly[0];
                            poly.points.push(front);
                            cutting_mesh_polylines.push(crate::geometry::OpenPolyline::new(poly.points.clone()));
                        }
                        cutting_mesh_polygons.clear();
                        cutting_mesh_area_recomputed = cutting_mesh_polylines.offset(surface_line_width / 2, JoinType::Miter, 1.2);
                    } else {
                        area_is_polygons = true;
                    }

                    if is_extruder_change_mesh {
                        // Limit the cutting mesh area to the printable meshes on this layer.
                        let area: &mut Shape = if area_is_polygons { &mut cutting_mesh_polygons } else { &mut cutting_mesh_area_recomputed };
                        if let Some(u) = layer_printable_mesh_unions.get(&(layer_nr as LayerIndex)) {
                            *area = area.intersection(u);
                        } else {
                            area.clear();
                        }
                    }
                }

                let mut new_outlines = Shape::default();
                let mut new_polylines = OpenLinesSet::default();
                for carved_mesh_idx in 0..volumes.len() {
                    let carved_mesh = &meshes[carved_mesh_idx];
                    // Do not apply cutting_mesh for meshes which have settings (cutting_mesh, anti_overhang_mesh, support_mesh).
                    if carved_mesh.settings.get_bool("cutting_mesh") || carved_mesh.settings.get_bool("anti_overhang_mesh") || carved_mesh.settings.get_bool("support_mesh") {
                        continue;
                    }
                    // (carved_mesh_idx != carving_mesh_idx because the carving mesh is a cutting mesh)
                    let carved_mesh_layer = &mut volumes[carved_mesh_idx].layers[layer_nr].polygons;

                    let intersection = cutting_mesh_polygons.intersection(carved_mesh_layer);
                    new_outlines.push_all_owned(intersection);
                    if surface_mode != ESurfaceMode::NORMAL {
                        // niet te geloven
                        new_polylines.push_all_owned(carved_mesh_layer.intersection_polylines(&cutting_mesh_polylines, true, 10, true));
                    }

                    let area: &Shape = if area_is_polygons { &cutting_mesh_polygons } else { &cutting_mesh_area_recomputed };
                    *carved_mesh_layer = carved_mesh_layer.difference(area);
                }
                cutting_mesh_polygons = new_outlines.union_polygons();
                if surface_mode != ESurfaceMode::NORMAL {
                    cutting_mesh_polylines.clear();
                    stitch_open_into_shape(&new_polylines, &mut cutting_mesh_polylines, &mut cutting_mesh_polygons, surface_line_width, 10);
                }
                volumes[carving_mesh_idx].layers[layer_nr].polygons = cutting_mesh_polygons;
                volumes[carving_mesh_idx].layers[layer_nr].open_polylines = cutting_mesh_polylines;
            }

            if is_extruder_change_mesh {
                // Since we are changing the extruder, the cutting mesh should not be considered a cutting mesh anymore.
                meshes[carving_mesh_idx].settings.remove("cutting_mesh");
            }
        }
    }
}
