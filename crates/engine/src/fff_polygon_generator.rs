//! `FffPolygonGenerator.h/.cpp`: the first half of the slicing pipeline, from meshes to the
//! areas (walls, skins, infill, support, adhesion) of every layer.

use crate::application::{current_slice, current_slice_mut};
use crate::conical_overhang::ConicalOverhang;
use crate::debug_dump;
use crate::geometry::point2ll::{normal, turn90_ccw, v_size, Point2LL};
use crate::geometry::{OpenLinesSet, Shape, SingleShape};
use crate::infill::lightning_generator::LightningGenerator;
use crate::infill::sierpinski_fill_provider::SierpinskiFillProvider;
use crate::infill::sub_div_cube::SubDivCube;
use crate::interlocking_generator::InterlockingGenerator;
use crate::layer_part::create_layer_parts;
use crate::mesh_group::MeshGroup;
use crate::mesh_material_splitter;
use crate::mold::Mold;
use crate::multi_volumes::{carve_multiple_volumes, generate_multiple_volumes_overlap, MultiVolumes};
use crate::raft::Raft;
use crate::settings::adaptive_layer_heights::AdaptiveLayerHeights;
use crate::settings::enums::{DraftShieldHeightLimitation, EFillMethod, EPlatformAdhesion, ESurfaceMode, SlicingTolerance};
use crate::settings::types::LayerIndex;
use crate::skin::SkinInfillAreaComputation;
use crate::skirt_brim::SkirtBrim;
use crate::slice_data_storage::{SliceDataStorage, SliceLayerPart, SliceMeshStorage};
use crate::slicer::Slicer;
use crate::support::AreaSupport;
use crate::texture_data_provider::TextureDataProvider;
use crate::tree_support::TreeSupport;
use crate::utils::algorithm::order;
use crate::utils::coord::Coord;
use crate::utils::extrusion_line::{ExtrusionJunction, ExtrusionLine, VariableWidthLines};
use crate::utils::math::{ceil_divide_signed, floor_divide_signed, round_divide_signed};
use crate::utils::section_type::SectionType;
use crate::utils::simplify::Simplify;
use crate::utils::thread_pool::{parallel_for, UnsafeSlice};
use crate::walls_computation::WallsComputation;
use clipper::JoinType;
use std::sync::Arc;

/// Development aid: `CURA_STOP_AFTER=<stage>` stops processing after the given stage so that the
/// intermediate results can be compared (see `debug_dump`).
fn stop_after(stage: &str) -> bool {
    std::env::var("CURA_STOP_AFTER").map(|v| v == stage).unwrap_or(false)
}

#[derive(Default)]
pub struct FffPolygonGenerator;

impl FffPolygonGenerator {
    /// Slice the meshes of the current mesh group and generate all the areas.
    pub fn generate_areas(&mut self, storage: &mut SliceDataStorage) -> bool {
        let meshgroup: &mut MeshGroup = current_slice_mut().scene.current_mesh_group_mut();
        mesh_material_splitter::make_material_modifier_meshes(meshgroup);

        if !self.slice_model(storage) {
            return false;
        }

        if stop_after("slice") {
            return false;
        }

        self.slices2polygons(storage);

        if stop_after("walls") || stop_after("skin") || stop_after("support") || stop_after("adhesion") || stop_after("suppinfill") {
            return false;
        }

        true
    }

    fn get_draft_shield_layer_count(&self, total_layers: usize) -> usize {
        let mesh_group_settings = &current_slice().scene.current_mesh_group().settings;
        if !mesh_group_settings.get_bool("draft_shield_enabled") {
            return 0;
        }
        match mesh_group_settings.get_enum::<DraftShieldHeightLimitation>("draft_shield_height_limitation") {
            DraftShieldHeightLimitation::FULL => total_layers,
            DraftShieldHeightLimitation::LIMITED => 0i64.max(
                (mesh_group_settings.get_coord("draft_shield_height") - mesh_group_settings.get_coord("layer_height_0")) / mesh_group_settings.get_coord("layer_height") + 1,
            ) as usize,
            #[allow(unreachable_patterns)]
            _ => {
                crate::log_warn!("A draft shield height limitation option was added without implementing the new option in getDraftShieldLayerCount.");
                total_layers
            }
        }
    }

    fn slice_model(&mut self, storage: &mut SliceDataStorage) -> bool {
        let scene = &mut current_slice_mut().scene;
        {
            let meshgroup = scene.current_mesh_group();
            storage.model_min = meshgroup.min();
            storage.model_max = meshgroup.max();
            storage.model_size = storage.model_max - storage.model_min;
        }

        crate::log_info!("Slicing model...");

        let mesh_group_settings = &current_slice().scene.current_mesh_group().settings;

        // Setup the variable layer height
        let mut slice_layer_count: i32 = 0;

        // Initial layer height of zero is not allowed. Negative layer height is nonsense.
        let initial_layer_thickness = mesh_group_settings.get_coord("layer_height_0");
        if initial_layer_thickness <= 0 {
            crate::log_error!("Initial layer height {} is disallowed.", initial_layer_thickness);
            return false;
        }

        // Normal layer height of zero is not allowed. Negative layer height is nonsense.
        let layer_thickness = mesh_group_settings.get_coord("layer_height");
        if layer_thickness <= 0 {
            crate::log_error!("Layer height {} is disallowed.\n", layer_thickness);
            return false;
        }

        // variable layers
        let mut adaptive_layer_heights: Option<AdaptiveLayerHeights> = None;
        let use_variable_layer_heights = mesh_group_settings.get_bool("adaptive_layer_height_enabled");

        if use_variable_layer_heights {
            // Calculate adaptive layer heights
            let variable_layer_height_max_variation = mesh_group_settings.get_coord("adaptive_layer_height_variation");
            let variable_layer_height_variation_step = mesh_group_settings.get_coord("adaptive_layer_height_variation_step");
            let adaptive_threshold = mesh_group_settings.get_coord("adaptive_layer_height_threshold");
            let alh = AdaptiveLayerHeights::new(
                layer_thickness,
                variable_layer_height_max_variation,
                variable_layer_height_variation_step,
                adaptive_threshold,
                scene.current_mesh_group(),
            );

            // Get the amount of layers
            slice_layer_count = alh.get_layer_count() as i32;
            adaptive_layer_heights = Some(alh);
        } else {
            // Find highest layer count according to each mesh's settings.
            for mesh in &scene.current_mesh_group().meshes {
                if !mesh.is_printed() {
                    continue;
                }
                let mesh_height = mesh.max().z;
                match mesh.settings.get_enum::<SlicingTolerance>("slicing_tolerance") {
                    SlicingTolerance::MIDDLE => {
                        if storage.model_max.z < initial_layer_thickness {
                            slice_layer_count = slice_layer_count.max(if mesh_height > initial_layer_thickness / 2 { 1 } else { 0 }); // One layer if higher than half initial layer height.
                        } else {
                            slice_layer_count =
                                slice_layer_count.max((round_divide_signed(mesh_height - initial_layer_thickness, layer_thickness) + 1) as i32);
                        }
                    }
                    SlicingTolerance::EXCLUSIVE => {
                        let mut new_slice_layer_count: i32 = 0;
                        if mesh_height >= initial_layer_thickness {
                            // If less than the initial layer thickness, leave it at 0.
                            new_slice_layer_count = (floor_divide_signed(mesh_height - 1 - initial_layer_thickness, layer_thickness) + 1) as i32;
                        }
                        if new_slice_layer_count > 0 {
                            // If there is at least one layer already, then add one more.
                            new_slice_layer_count += 1;
                        }
                        slice_layer_count = slice_layer_count.max(new_slice_layer_count);
                    }
                    SlicingTolerance::INCLUSIVE => {
                        if mesh_height < initial_layer_thickness {
                            slice_layer_count = slice_layer_count.max(if mesh_height > 0 { 1 } else { 0 }); // If less than the initial layer height, it always has 1 layer unless the height is truly zero.
                        } else {
                            slice_layer_count = slice_layer_count.max((ceil_divide_signed(mesh_height - initial_layer_thickness, layer_thickness) + 1) as i32);
                        }
                    }
                    #[allow(unreachable_patterns)]
                    _ => {
                        crate::log_error!("Unknown slicing tolerance. Did you forget to add a case here?");
                        return false;
                    }
                }
            }
        }

        // Model is shallower than layer_height_0, so not even the first layer is sliced. Return an empty model then.
        if slice_layer_count <= 0 {
            return true; // This is NOT an error state!
        }

        let mut slicer_list: Vec<Slicer> = Vec::new();
        let n_meshes = scene.current_mesh_group().meshes.len();
        for mesh_idx in 0..n_meshes {
            // Check if adaptive layers is populated to prevent accessing a method on NULL
            let adaptive_layer_height_values = adaptive_layer_heights.as_ref().map(|a| a.get_layers());

            let mesh = &mut scene.current_mesh_group_mut().meshes[mesh_idx];
            let slicing_tolerance = mesh.settings.get_enum::<SlicingTolerance>("slicing_tolerance");
            let slicer = Slicer::new(
                mesh,
                layer_thickness,
                slice_layer_count as usize,
                use_variable_layer_heights,
                adaptive_layer_height_values,
                slicing_tolerance,
                initial_layer_thickness,
            );

            slicer_list.push(slicer);
            crate::progress::message_progress(crate::progress::Stage::Slicing, mesh_idx as i64 + 1, n_meshes as i64);
        }

        scene.current_mesh_group_mut().clear();

        Mold::process(&mut slicer_list);

        for mesh_idx in 0..slicer_list.len() {
            let mesh = &current_slice().scene.current_mesh_group().meshes[mesh_idx];
            if mesh.settings.get_bool("conical_overhang_enabled") && !mesh.settings.get_bool("anti_overhang_mesh") {
                ConicalOverhang::apply(&mut slicer_list[mesh_idx], mesh);
            }
        }

        MultiVolumes::carve_cutting_meshes(&mut slicer_list);

        if current_slice().scene.current_mesh_group().settings.get_bool("carve_multiple_volumes") {
            carve_multiple_volumes(&mut slicer_list);
        }

        generate_multiple_volumes_overlap(&mut slicer_list);

        if current_slice().scene.current_mesh_group().settings.get_bool("interlocking_enable") {
            InterlockingGenerator::generate_interlocking_structure(&mut slicer_list);
        }

        storage.print_layer_count = 0;
        for mesh_idx in 0..slicer_list.len() {
            let mesh = &current_slice().scene.current_mesh_group().meshes[mesh_idx];
            let slicer = &slicer_list[mesh_idx];
            if !mesh.settings.get_bool("anti_overhang_mesh") && !mesh.settings.get_bool("infill_mesh") && !mesh.settings.get_bool("cutting_mesh") {
                storage.print_layer_count = storage.print_layer_count.max(slicer.layers.len());
            }
        }
        storage.support.support_layers.resize_with(storage.print_layer_count, Default::default);

        storage.meshes.reserve(slicer_list.len()); // causes there to be no resize in meshes so that the pointers in sliceMeshStorage._config to retraction_config don't get invalidated.
        for mesh_idx in 0..slicer_list.len() {
            let slicer = &mut slicer_list[mesh_idx];
            let mesh = &current_slice().scene.current_mesh_group().meshes[mesh_idx];

            // always make a new SliceMeshStorage, so that they have the same ordering / indexing as meshgroup.meshes
            storage.meshes.push(Box::new(SliceMeshStorage::new(mesh, slicer.layers.len())));
            let is_support_modifier = AreaSupport::handle_support_modifier_mesh(storage, &mesh.settings, slicer);
            if !is_support_modifier {
                let mesh_storage = storage.meshes.last_mut().unwrap();
                create_layer_parts(mesh_storage, slicer);
            }

            // Do not add and process support _modifier_ meshes further, and ONLY skip support _modifiers_. They have been
            // processed in AreaSupport::handleSupportModifierMesh(), but other helper meshes such as infill meshes are
            // processed in a later stage, except for support mesh itself, so an exception is made for that.
            if is_support_modifier && !mesh.settings.get_bool("support_mesh") {
                storage.meshes.pop();
                continue;
            }

            // check one if raft offset is needed
            let has_raft = mesh_group_settings.get_enum::<EPlatformAdhesion>("adhesion_type") == EPlatformAdhesion::RAFT;

            // calculate the height at which each layer is actually printed (printZ)
            let mesh_storage = storage.meshes.last_mut().unwrap();
            for layer_nr in 0..mesh_storage.layers.len() {
                let slicer_layer = &slicer.layers[layer_nr];
                let layer = &mut mesh_storage.layers[layer_nr];

                if let (Some(uv), Some(texture), Some(mapping)) = (&slicer_layer.sliced_uv_coordinates, &mesh.texture, &mesh.texture_data_mapping) {
                    layer.texture_data_provider = Some(Arc::new(TextureDataProvider::new(uv.clone(), texture.clone(), mapping.clone())));
                }

                if use_variable_layer_heights {
                    let al = &adaptive_layer_heights.as_ref().unwrap().get_layers()[layer_nr];
                    layer.print_z = al.z_position;
                    layer.thickness = al.layer_height;
                } else {
                    layer.print_z = initial_layer_thickness + (layer_nr as Coord * layer_thickness);

                    if layer_nr == 0 {
                        layer.thickness = initial_layer_thickness;
                    } else {
                        layer.thickness = layer_thickness;
                    }
                }

                // add the raft offset to each layer
                if has_raft {
                    let train = mesh_group_settings.get_extruder_train("raft_surface_extruder_nr");
                    layer.print_z +=
                        Raft::get_total_thickness() + train.settings.get_coord("raft_airgap") - train.settings.get_coord("layer_0_z_overlap"); // shift all layers (except 0) down

                    if layer_nr == 0 {
                        layer.print_z += train.settings.get_coord("layer_0_z_overlap"); // undo shifting down of first layer
                    }
                }
            }
        }

        if debug_dump::enabled() {
            for (mesh_idx, mesh) in storage.meshes.iter().enumerate() {
                for (layer_nr, layer) in mesh.layers.iter().enumerate() {
                    let mut s = format!("parts mesh={} layer={} z={} thickness={} nparts={}\n", mesh_idx, layer_nr, layer.print_z, layer.thickness, layer.parts.len());
                    for part in &layer.parts {
                        s.push_str(&debug_dump::shape_to_string(&part.outline.0));
                    }
                    debug_dump::write(&s);
                }
            }
        }

        true
    }

    fn slices2polygons(&mut self, storage: &mut SliceDataStorage) {
        // compute layer count and remove first empty layers
        // there is no separate progress stage for removeEmptyFisrtLayer (TODO)
        let mut _slice_layer_count: u32 = 0;
        for mesh in &storage.meshes {
            if !mesh.settings().get_bool("infill_mesh") && !mesh.settings().get_bool("anti_overhang_mesh") {
                _slice_layer_count = _slice_layer_count.max(mesh.layers.len() as u32);
            }
        }

        // handle meshes
        let mut mesh_order: Vec<usize> = Vec::new();
        {
            // compute mesh order (std::multimap: sorted by key, equal keys in insertion order)
            let mut order_to_mesh_indices: Vec<(i32, usize)> = Vec::new();
            for mesh_idx in 0..storage.meshes.len() {
                order_to_mesh_indices.push((storage.meshes[mesh_idx].settings().get_int("infill_mesh_order"), mesh_idx));
            }
            order_to_mesh_indices.sort_by_key(|a| a.0); // stable
            for (_, idx) in order_to_mesh_indices {
                mesh_order.push(idx);
            }
        }
        for mesh_order_idx in 0..mesh_order.len() {
            self.process_basic_walls_skin_infill(storage, mesh_order_idx, &mesh_order);
            crate::progress::message_progress(crate::progress::Stage::InsetSkin, mesh_order_idx as i64 + 1, storage.meshes.len() as i64);
        }
        if stop_after("walls") || stop_after("skin") {
            return;
        }

        let mesh_group_settings = &current_slice().scene.current_mesh_group().settings;
        let has_support = mesh_group_settings.get_bool("support_enable") || mesh_group_settings.get_bool("support_mesh");
        let remove_empty_first_layers = mesh_group_settings.get_bool("remove_empty_first_layers") && !has_support;
        if remove_empty_first_layers {
            let mut total = storage.print_layer_count;
            self.remove_empty_first_layers(storage, &mut total);
            storage.print_layer_count = total;
        }
        if storage.print_layer_count == 0 {
            crate::log_warn!("Stopping process because there are no non-empty layers.");
            return;
        }

        AreaSupport::generate_overhang_areas(storage);
        AreaSupport::generate_support_areas(storage);
        let mut tree_support_generator = TreeSupport::new(storage);
        tree_support_generator.generate_support_areas(storage);
        debug_dump::dump_support(storage, "support", false);
        if stop_after("support") {
            return;
        }

        self.compute_print_height_statistics(storage);

        // handle helpers
        storage.initialize_prime_tower();

        crate::log_debug!("Processing ooze shield");
        self.process_ooze_shield(storage);

        crate::log_debug!("Processing draft shield");
        self.process_draft_shield(storage);

        // This catches a special case in which the models are in the air, and then
        // the adhesion mustn't be calculated.
        if !self.is_empty_layer(storage, 0) || storage.prime_tower.is_some() {
            crate::log_debug!("Processing platform adhesion");
            self.process_platform_adhesion(storage);
        }
        debug_dump::dump_adhesion(storage);
        if stop_after("adhesion") {
            return;
        }

        crate::log_debug!("Meshes post-processing");
        // meshes post processing
        for mesh in storage.meshes.iter_mut() {
            self.process_derived_walls_skin_infill(mesh);
        }

        crate::log_debug!("Processing gradual support");
        // generate gradual support
        AreaSupport::generate_support_infill_features(storage);
        debug_dump::dump_support(storage, "suppinfill", true);
    }

    fn process_basic_walls_skin_infill(&mut self, storage: &mut SliceDataStorage, mesh_order_idx: usize, mesh_order: &[usize]) {
        let mesh_idx = mesh_order[mesh_order_idx];
        let mesh_layer_count = storage.meshes[mesh_idx].layers.len();
        if storage.meshes[mesh_idx].settings().get_bool("infill_mesh") {
            self.process_infill_mesh(storage, mesh_order_idx, mesh_order);
        }

        // walls
        {
            let mesh: &mut SliceMeshStorage = &mut storage.meshes[mesh_idx];
            let settings = mesh.settings();
            let layers = UnsafeSlice::new(mesh.layers.as_mut_slice());
            parallel_for(0, mesh_layer_count, |layer_number| {
                crate::log_debug!("Processing insets for layer {} of {}", layer_number, mesh_layer_count);
                // SAFETY: each layer is processed by exactly one iteration.
                let layer = unsafe { layers.get_mut(layer_number) };
                Self::process_walls(settings, layer, layer_number);
            });
        }

        if debug_dump::enabled() {
            let mesh = &storage.meshes[mesh_idx];
            for (layer_nr, layer) in mesh.layers.iter().enumerate() {
                let mut s = format!("walls mesh={} layer={} nparts={}\n", mesh_idx, layer_nr, layer.parts.len());
                for part in &layer.parts {
                    s.push_str(" outline\n");
                    s.push_str(&debug_dump::shape_to_string(&part.outline.0));
                    s.push_str(" inner_area\n");
                    s.push_str(&debug_dump::shape_to_string(&part.inner_area));
                    s.push_str(" toolpaths\n");
                    s.push_str(&debug_dump::toolpaths_to_string(&part.wall_toolpaths));
                }
                debug_dump::write(&s);
            }
        }
        if stop_after("walls") {
            return;
        }

        let mut process_infill = storage.meshes[mesh_idx].settings().get_coord("infill_line_distance") > 0;
        if !process_infill {
            // do process infill anyway if it's modified by modifier meshes
            let scene = &current_slice().scene;
            for other_mesh_order_idx in mesh_order_idx + 1..mesh_order.len() {
                let other_mesh_idx = mesh_order[other_mesh_order_idx];
                let other_mesh = &storage.meshes[other_mesh_idx];
                if other_mesh.settings().get_bool("infill_mesh") {
                    let aabb = scene.current_mesh_group().meshes[mesh_idx].get_aabb();
                    let other_aabb = scene.current_mesh_group().meshes[other_mesh_idx].get_aabb();
                    if aabb.hit(&other_aabb) {
                        process_infill = true;
                    }
                }
            }
        }

        // skin & infill
        let mesh_group_settings = &current_slice().scene.current_mesh_group().settings;
        let magic_spiralize = mesh_group_settings.get_bool("magic_spiralize");
        let mut mesh_max_initial_bottom_layer_count: usize = 0;
        if magic_spiralize {
            mesh_max_initial_bottom_layer_count = mesh_max_initial_bottom_layer_count.max(storage.meshes[mesh_idx].settings().get_usize("initial_bottom_layers"));
        }

        {
            let mesh: &mut SliceMeshStorage = &mut storage.meshes[mesh_idx];
            let mesh_ptr = crate::utils::thread_pool::SharedMut::new(mesh);
            parallel_for(0, mesh_layer_count, |layer_number| {
                crate::log_debug!("Processing skins and infill layer {} of {}", layer_number, mesh_layer_count);
                if !magic_spiralize || layer_number < mesh_max_initial_bottom_layer_count {
                    // Only generate up/downskin and infill for the first X layers when spiralize is choosen.
                    // SAFETY: SkinInfillAreaComputation only writes to the given layer (and reads other layers' outlines).
                    let mesh = unsafe { mesh_ptr.get() };
                    Self::process_skins_and_infill(mesh, layer_number as LayerIndex, process_infill);
                }
            });
        }

        if debug_dump::enabled() {
            let mesh = &storage.meshes[mesh_idx];
            for (layer_nr, layer) in mesh.layers.iter().enumerate() {
                let mut s = format!("skin mesh={} layer={} nparts={}\n", mesh_idx, layer_nr, layer.parts.len());
                for part in &layer.parts {
                    s.push_str(" infill_area\n");
                    s.push_str(&debug_dump::shape_to_string(&part.infill_area));
                    s.push_str(&format!(" infill_area_own {}\n", part.infill_area_own.is_some() as i32));
                    if let Some(own) = &part.infill_area_own {
                        s.push_str(&debug_dump::shape_to_string(own));
                    }
                    s.push_str(" top_most_surface\n");
                    s.push_str(&debug_dump::shape_to_string(&part.top_most_surface));
                    s.push_str(" bottom_most_surface\n");
                    s.push_str(&debug_dump::shape_to_string(&part.bottom_most_surface));
                    s.push_str(&format!(" nskin={}\n", part.skin_parts.len()));
                    for sp in &part.skin_parts {
                        s.push_str(" skin outline\n");
                        s.push_str(&debug_dump::shape_to_string(&sp.outline.0));
                        s.push_str(" skin_fill\n");
                        s.push_str(&debug_dump::shape_to_string(&sp.skin_fill));
                        s.push_str(" roofing_fill\n");
                        s.push_str(&debug_dump::shape_to_string(&sp.roofing_fill));
                        s.push_str(" flooring_fill\n");
                        s.push_str(&debug_dump::shape_to_string(&sp.flooring_fill));
                    }
                }
                s.push_str(" top_surface\n");
                s.push_str(&debug_dump::shape_to_string(&layer.top_surface.areas));
                s.push_str(" bottom_surface\n");
                s.push_str(&debug_dump::shape_to_string(&layer.bottom_surface));
                debug_dump::write(&s);
            }
        }
    }

    fn process_infill_mesh(&mut self, storage: &mut SliceDataStorage, mesh_order_idx: usize, mesh_order: &[usize]) {
        let mesh_idx = mesh_order[mesh_order_idx];
        let settings = storage.meshes[mesh_idx].settings();
        let surface_line_width = settings.get_coord("wall_line_width_0");
        let surface_mode = settings.get_enum::<ESurfaceMode>("magic_mesh_surface_mode");

        storage.meshes[mesh_idx].layer_nr_max_filled_layer = -1;
        let n_layers = storage.meshes[mesh_idx].layers.len() as LayerIndex;
        for layer_idx in 0..n_layers {
            // Take the layer out so that other meshes can be borrowed mutably at the same time.
            let mut layer = std::mem::take(&mut storage.meshes[mesh_idx].layers[layer_idx as usize]);
            if surface_mode == ESurfaceMode::SURFACE {
                for part in &layer.parts {
                    for poly in part.outline.0.iter() {
                        layer.open_polylines.push(crate::geometry::polyline::ClosedLine::to_pseudo_open_polyline(poly));
                    }
                }
                layer.parts.clear();
            }

            let mut new_parts: Vec<SingleShape> = Vec::new();
            let mut new_polylines = OpenLinesSet::default();

            for &other_mesh_idx in mesh_order {
                // limit the infill mesh's outline to within the infill of all meshes with lower order
                if other_mesh_idx == mesh_idx {
                    break; // all previous meshes have been processed
                }
                let other_mesh = &mut storage.meshes[other_mesh_idx];
                if layer_idx >= other_mesh.layers.len() as LayerIndex {
                    // there can be no interaction between the infill mesh and this other non-infill mesh
                    continue;
                }

                let other_layer = &mut other_mesh.layers[layer_idx as usize];

                for other_part in other_layer.parts.iter_mut() {
                    if surface_mode != ESurfaceMode::SURFACE {
                        for part in &layer.parts {
                            if !part.boundary_box.hit(&other_part.boundary_box) {
                                // early out for optimization
                                continue;
                            }

                            let new_outline = part.outline.0.intersection(other_part.get_own_infill_area());
                            if new_outline.len() == 1 {
                                // we don't have to call splitIntoParts, because a single polygon can only be a single part
                                let mut outline_part_here = SingleShape::default();
                                outline_part_here.0.push(new_outline[0].clone());
                                new_parts.push(outline_part_here);
                            } else if new_outline.len() > 1 {
                                // we don't know whether it's a multitude of parts because of newly introduced holes, or because the polygon has been split up
                                let new_parts_here = new_outline.split_into_parts(false);
                                for new_part_here in new_parts_here {
                                    new_parts.push(new_part_here);
                                }
                            }
                            // change the infill area of the non-infill mesh which is to be filled with e.g. lines
                            other_part.infill_area_own = Some(other_part.get_own_infill_area().difference(&part.outline.0));
                            // note: don't change the part.infill_area, because we change the structure of that area, while the basic area in which infill is printed remains the same
                            //       the infill area remains the same for combining the infill of various layers
                        }
                    }
                    if surface_mode != ESurfaceMode::NORMAL {
                        let own_infill_area = other_part.get_own_infill_area().clone();
                        let cut_lines = own_infill_area.intersection_polylines(&layer.open_polylines, true, 10, true);
                        new_polylines.push_all_owned(cut_lines);
                        // NOTE: closed polygons will be represented as polylines, which will be closed automatically in the PathOrderOptimizer
                        if !own_infill_area.is_empty() {
                            other_part.infill_area_own =
                                Some(own_infill_area.difference(&layer.open_polylines.offset(surface_line_width / 2, JoinType::Miter, 1.2)));
                        }
                    }
                }
            }

            layer.parts.clear();
            for part in new_parts {
                if part.0.is_empty() {
                    continue;
                }
                let mut p = SliceLayerPart::default();
                p.boundary_box.calculate_shape(&part.0);
                p.outline = part;
                layer.parts.push(p);
            }

            if surface_mode != ESurfaceMode::NORMAL {
                layer.open_polylines = new_polylines;
            }

            let mesh = &mut storage.meshes[mesh_idx];
            if !layer.parts.is_empty() || (surface_mode != ESurfaceMode::NORMAL && !layer.open_polylines.is_empty()) {
                mesh.layer_nr_max_filled_layer = layer_idx; // last set by the highest non-empty layer
            }
            mesh.layers[layer_idx as usize] = layer;
        }
    }

    fn process_derived_walls_skin_infill(&mut self, mesh: &mut SliceMeshStorage) {
        let settings = mesh.settings();
        if settings.get_bool("infill_support_enabled") {
            // create gradual infill areas
            SkinInfillAreaComputation::generate_infill_support(mesh);
        }

        // create gradual infill areas
        SkinInfillAreaComputation::generate_gradual_infill(mesh);

        // SubDivCube Pre-compute Octree
        if settings.get_coord("infill_line_distance") > 0 && settings.get_enum::<EFillMethod>("infill_pattern") == EFillMethod::CUBICSUBDIV {
            let mesh_middle = mesh.bounding_box.get_middle();
            let infill_origin =
                Point2LL::new(mesh_middle.x + settings.get_coord("infill_offset_x"), mesh_middle.y + settings.get_coord("infill_offset_y"));
            SubDivCube::precompute_octree(mesh, infill_origin);
        }

        // Pre-compute Cross Fractal
        if settings.get_coord("infill_line_distance") > 0
            && (settings.get_enum::<EFillMethod>("infill_pattern") == EFillMethod::CROSS || settings.get_enum::<EFillMethod>("infill_pattern") == EFillMethod::CROSS_3D)
        {
            let cross_subdivision_spec_image_file = settings.get_string("cross_infill_density_image");
            let file_good = !cross_subdivision_spec_image_file.is_empty() && std::fs::File::open(&cross_subdivision_spec_image_file).is_ok();
            if file_good {
                mesh.cross_fill_provider = Some(Arc::new(SierpinskiFillProvider::new_with_image(
                    &mesh.bounding_box,
                    settings.get_coord("infill_line_distance"),
                    settings.get_coord("infill_line_width"),
                    &cross_subdivision_spec_image_file,
                )));
            } else {
                if !cross_subdivision_spec_image_file.is_empty() && cross_subdivision_spec_image_file != " " {
                    crate::log_error!("Cannot find density image: {}.", cross_subdivision_spec_image_file);
                }
                mesh.cross_fill_provider = Some(Arc::new(SierpinskiFillProvider::new(
                    &mesh.bounding_box,
                    settings.get_coord("infill_line_distance"),
                    settings.get_coord("infill_line_width"),
                )));
            }
        }

        // Pre-compute lightning fill (aka minfill, aka ribbed support vaults)
        if settings.get_coord("infill_line_distance") > 0 && settings.get_enum::<EFillMethod>("infill_pattern") == EFillMethod::LIGHTNING {
            // TODO: Make all of these into new type pointers (but the cross fill things need to happen too then, otherwise it'd just look inconsistent).
            mesh.lightning_generator = Some(Arc::new(LightningGenerator::new(mesh)));
        }

        // combine infill
        SkinInfillAreaComputation::combine_infill_layers(mesh);

        // fuzzy skin
        if settings.get_bool("magic_fuzzy_skin_enabled") && !settings.get_bool("interlocking_enable") {
            self.process_fuzzy_walls(mesh);
        }
    }

    fn process_walls(settings: &crate::settings::settings::Settings, layer: &mut crate::slice_data_storage::SliceLayer, layer_nr: usize) {
        let walls_computation = WallsComputation::new(settings, layer_nr as LayerIndex);
        walls_computation.generate_walls(layer, SectionType::WALL);
    }

    fn is_empty_layer(&self, storage: &SliceDataStorage, layer_idx: LayerIndex) -> bool {
        if storage.support.generated && (layer_idx as usize) < storage.support.support_layers.len() {
            let support_layer = &storage.support.support_layers[layer_idx as usize];
            if !support_layer.support_infill_parts.is_empty() || !support_layer.support_bottom.is_empty() || !support_layer.support_roof.is_empty() {
                return false;
            }
        }
        for mesh in &storage.meshes {
            if layer_idx as usize >= mesh.layers.len() {
                continue;
            }
            let layer = &mesh.layers[layer_idx as usize];
            if mesh.settings().get_enum::<ESurfaceMode>("magic_mesh_surface_mode") != ESurfaceMode::NORMAL && !layer.open_polylines.is_empty() {
                return false;
            }
            for part in &layer.parts {
                if !part.print_outline.is_empty() {
                    return false;
                }
            }
        }
        true
    }

    fn remove_empty_first_layers(&mut self, storage: &mut SliceDataStorage, total_layers: &mut usize) {
        let mut n_empty_first_layers: usize = 0;
        let mut hightest_empty_layer: Coord = 0;
        for layer_idx in 0..*total_layers {
            if self.is_empty_layer(storage, layer_idx as LayerIndex) {
                n_empty_first_layers += 1;
                let mut layer_highest_z: Coord = 0;
                for mesh in &storage.meshes {
                    layer_highest_z = if layer_idx >= mesh.layers.len() { layer_highest_z } else { layer_highest_z.max(mesh.layers[layer_idx].print_z) };
                }
                hightest_empty_layer = hightest_empty_layer.max(layer_highest_z);
            } else {
                break;
            }
        }

        if n_empty_first_layers > 0 {
            crate::log_info!("Removing {} layers because they are empty", n_empty_first_layers);
            for mesh in storage.meshes.iter_mut() {
                let layers = &mut mesh.layers;
                if layers.len() > n_empty_first_layers {
                    // transfer initial layer thickness to new initial layer
                    layers[n_empty_first_layers].thickness = layers[0].thickness;
                }
                layers.drain(0..n_empty_first_layers.min(layers.len()));
                for layer in layers.iter_mut() {
                    layer.print_z -= hightest_empty_layer;
                }
                mesh.layer_nr_max_filled_layer -= n_empty_first_layers as LayerIndex;
            }
            *total_layers -= n_empty_first_layers;
            storage.support.layer_nr_max_filled_layer -= n_empty_first_layers as i32;
            let support_layers = &mut storage.support.support_layers;
            support_layers.drain(0..n_empty_first_layers);
        }
    }

    fn process_skins_and_infill(mesh: &mut SliceMeshStorage, layer_nr: LayerIndex, process_infill: bool) {
        let settings = mesh.settings();
        if settings.get_enum::<ESurfaceMode>("magic_mesh_surface_mode") == ESurfaceMode::SURFACE {
            return;
        }

        let mut skin_infill_area_computation = SkinInfillAreaComputation::new(layer_nr, mesh, process_infill);
        skin_infill_area_computation.generate_skins_and_infill();

        if ((settings.get_bool("ironing_enabled") && !settings.get_bool("ironing_only_highest_layer")) || mesh.layer_nr_max_filled_layer == layer_nr)
            || !settings.get_bool("small_skin_on_surface")
        {
            // Generate the top surface to iron over.
            let mut top_surface = std::mem::take(&mut mesh.layers[layer_nr as usize].top_surface);
            top_surface.set_areas_from_mesh_and_layer_number(mesh, layer_nr as usize);
            mesh.layers[layer_nr as usize].top_surface = top_surface;
        }

        if layer_nr >= 0 && !settings.get_bool("small_skin_on_surface") {
            // Generate the bottom surface.
            let mut bottom_surface = mesh.layers[layer_nr as usize].get_outlines(false);
            if layer_nr > 0 {
                bottom_surface = bottom_surface.difference(&mesh.layers[layer_nr as usize - 1].get_outlines(false));
            }
            mesh.layers[layer_nr as usize].bottom_surface = bottom_surface;
        }
    }

    fn compute_print_height_statistics(&mut self, storage: &mut SliceDataStorage) {
        let extruder_count = current_slice().scene.extruders.len();

        debug_assert!(storage.max_print_height_per_extruder.is_empty(), "storage.max_print_height_per_extruder shouldn't have been initialized yet!");
        let raft_layers = Raft::get_total_extra_layers() as i32;
        storage.max_print_height_per_extruder = vec![-(raft_layers + 1); extruder_count]; // Initialize all as -1 (or lower in case of raft).
        {
            // compute max_object_height_per_extruder
            // Height of the meshes themselves.
            for mesh in &storage.meshes {
                if mesh.settings().get_bool("anti_overhang_mesh") || mesh.settings().get_bool("support_mesh") {
                    continue; // Special type of mesh that doesn't get printed.
                }
                for extruder_nr in 0..extruder_count {
                    let mut layer_nr = mesh.layers.len() as LayerIndex - 1;
                    while layer_nr > storage.max_print_height_per_extruder[extruder_nr] as LayerIndex {
                        if mesh.get_extruder_is_used_on_layer(extruder_nr, layer_nr) {
                            debug_assert!(storage.max_print_height_per_extruder[extruder_nr] as LayerIndex <= layer_nr);
                            storage.max_print_height_per_extruder[extruder_nr] = layer_nr as i32;
                        }
                        layer_nr -= 1;
                    }
                }
            }

            // Height of where the support reaches.
            let scene = &current_slice().scene;
            let mesh_group_settings = &scene.current_mesh_group().settings;
            let max_filled = storage.support.layer_nr_max_filled_layer;
            let support_infill_extruder_nr = mesh_group_settings.get_extruder_train("support_infill_extruder_nr").extruder_nr; // TODO: Support extruder should be configurable per object.
            storage.max_print_height_per_extruder[support_infill_extruder_nr] = storage.max_print_height_per_extruder[support_infill_extruder_nr].max(max_filled);
            let support_roof_extruder_nr = mesh_group_settings.get_extruder_train("support_roof_extruder_nr").extruder_nr; // TODO: Support roof extruder should be configurable per object.
            storage.max_print_height_per_extruder[support_roof_extruder_nr] = storage.max_print_height_per_extruder[support_roof_extruder_nr].max(max_filled);
            let support_bottom_extruder_nr = mesh_group_settings.get_extruder_train("support_bottom_extruder_nr").extruder_nr; // TODO: Support bottom extruder should be configurable per object.
            storage.max_print_height_per_extruder[support_bottom_extruder_nr] = storage.max_print_height_per_extruder[support_bottom_extruder_nr].max(max_filled);

            // Height of where the platform adhesion reaches.
            let adhesion_type = mesh_group_settings.get_enum::<EPlatformAdhesion>("adhesion_type");
            match adhesion_type {
                EPlatformAdhesion::SKIRT | EPlatformAdhesion::BRIM => {
                    let skirt_brim_extruder_trains = mesh_group_settings.get_extruder_trains("skirt_brim_extruder_nr");
                    for skirt_brim_extruder_nr in skirt_brim_extruder_trains {
                        storage.max_print_height_per_extruder[skirt_brim_extruder_nr] = 0.max(storage.max_print_height_per_extruder[skirt_brim_extruder_nr]);
                    }
                }
                EPlatformAdhesion::RAFT => {
                    let base_extruder_nr = mesh_group_settings.get_extruder_train("raft_base_extruder_nr").extruder_nr;
                    storage.max_print_height_per_extruder[base_extruder_nr] = (-raft_layers).max(storage.max_print_height_per_extruder[base_extruder_nr]);
                    let interface_extruder_nr = mesh_group_settings.get_extruder_train("raft_interface_extruder_nr").extruder_nr;
                    storage.max_print_height_per_extruder[interface_extruder_nr] = (-raft_layers + 1).max(storage.max_print_height_per_extruder[interface_extruder_nr]);
                    let surface_extruder_nr = mesh_group_settings.get_extruder_train("raft_surface_extruder_nr").extruder_nr;
                    storage.max_print_height_per_extruder[surface_extruder_nr] = (-1).max(storage.max_print_height_per_extruder[surface_extruder_nr]);
                }
                _ => {}
            }
        }

        storage.max_print_height_order = order(&storage.max_print_height_per_extruder);
        if extruder_count >= 2 {
            let second_highest_extruder = storage.max_print_height_order[extruder_count - 2];
            storage.max_print_height_second_to_last_extruder = storage.max_print_height_per_extruder[second_highest_extruder];
        } else {
            storage.max_print_height_second_to_last_extruder = -(raft_layers + 1);
        }
    }

    fn process_ooze_shield(&mut self, storage: &mut SliceDataStorage) {
        let mesh_group_settings = &current_slice().scene.current_mesh_group().settings;
        if !mesh_group_settings.get_bool("ooze_shield_enabled") {
            return;
        }

        let ooze_shield_dist = mesh_group_settings.get_coord("ooze_shield_dist");

        let mut layer_nr: i32 = 0;
        while layer_nr <= storage.max_print_height_second_to_last_extruder {
            let around_support = true;
            let around_prime_tower = false;
            let s = storage
                .get_layer_outlines(layer_nr as LayerIndex, around_support, around_prime_tower, false, -1, true)
                .offset(ooze_shield_dist, JoinType::Round, 1.2)
                .get_outside_polygons();
            storage.ooze_shield.push(s);
            layer_nr += 1;
        }

        let angle = mesh_group_settings.get_angle_degrees("ooze_shield_angle");
        if angle.value() <= 89.0 {
            let allowed_angle_offset =
                (mesh_group_settings.get_angle_radians("ooze_shield_angle").value().tan() * mesh_group_settings.get_coord("layer_height") as f64) as Coord; // Allow for a 60deg angle in the oozeShield.
            let mut layer_nr: LayerIndex = 1;
            while layer_nr <= storage.max_print_height_second_to_last_extruder as LayerIndex {
                let below = storage.ooze_shield[layer_nr as usize - 1].offset(-allowed_angle_offset, JoinType::Miter, 1.2);
                storage.ooze_shield[layer_nr as usize] = storage.ooze_shield[layer_nr as usize].union_polygons_with(&below, clipper::PolyFillType::NonZero);
                layer_nr += 1;
            }
            let mut layer_nr = storage.max_print_height_second_to_last_extruder as LayerIndex;
            while layer_nr > 0 {
                let above = storage.ooze_shield[layer_nr as usize].offset(-allowed_angle_offset, JoinType::Miter, 1.2);
                storage.ooze_shield[layer_nr as usize - 1] = storage.ooze_shield[layer_nr as usize - 1].union_polygons_with(&above, clipper::PolyFillType::NonZero);
                layer_nr -= 1;
            }
        }

        let largest_printed_area = 1.0; // TODO: make var a parameter, and perhaps even a setting?
        let mut layer_nr: LayerIndex = 0;
        while layer_nr <= storage.max_print_height_second_to_last_extruder as LayerIndex {
            storage.ooze_shield[layer_nr as usize].remove_small_areas(largest_printed_area, false);
            layer_nr += 1;
        }
        if storage.prime_tower.is_some() {
            let max_line_width = Self::max_used_skirt_brim_line_width(storage);
            let mut layer_nr: LayerIndex = 0;
            while layer_nr <= storage.max_print_height_second_to_last_extruder as LayerIndex {
                let occupied = storage.prime_tower.as_ref().unwrap().get_occupied_outline(layer_nr).offset((max_line_width / 2) as i32, JoinType::Miter, 1.2);
                storage.ooze_shield[layer_nr as usize] = storage.ooze_shield[layer_nr as usize].difference(&occupied);
                layer_nr += 1;
            }
        }
    }

    fn max_used_skirt_brim_line_width(storage: &SliceDataStorage) -> Coord {
        // TODO: this is the same code as in processDraftShield
        let mut max_line_width: Coord = 0;
        let extruder_is_used = storage.get_extruders_used();
        let extruders = &current_slice().scene.extruders;
        for extruder_nr in 0..extruders.len() {
            if !extruder_is_used[extruder_nr] {
                continue;
            }
            max_line_width = max_line_width.max(extruders[extruder_nr].settings.get_coord("skirt_brim_line_width"));
        }
        max_line_width
    }

    fn process_draft_shield(&mut self, storage: &mut SliceDataStorage) {
        let draft_shield_layers = self.get_draft_shield_layer_count(storage.print_layer_count);
        if draft_shield_layers == 0 {
            return;
        }
        let mesh_group_settings = &current_slice().scene.current_mesh_group().settings;
        let layer_height = mesh_group_settings.get_coord("layer_height");

        let layer_skip: LayerIndex = 500 / layer_height + 1;

        let mut draft_shield = std::mem::take(&mut storage.draft_protection_shield);
        let mut layer_nr: LayerIndex = 0;
        while layer_nr < storage.print_layer_count as LayerIndex && layer_nr < draft_shield_layers as LayerIndex {
            let around_support = true;
            let around_prime_tower = false;
            draft_shield = draft_shield.union_polygons_with(
                &storage.get_layer_outlines(layer_nr, around_support, around_prime_tower, false, -1, true),
                clipper::PolyFillType::NonZero,
            );
            layer_nr += layer_skip;
        }

        let draft_shield_dist = mesh_group_settings.get_coord("draft_shield_dist");
        storage.draft_protection_shield = draft_shield.approx_convex_hull(draft_shield_dist as i32);

        // Extra offset has rounded joints, so simplify again.
        let mut maximum_resolution: Coord = 0; // Draft shield is printed with every extruder, so resolve with the max() or min() of them to meet the requirements of all extruders.
        let mut maximum_deviation: Coord = Coord::MAX;
        for extruder in &current_slice().scene.extruders {
            maximum_resolution = maximum_resolution.max(extruder.settings.get_coord("meshfix_maximum_resolution"));
            maximum_deviation = maximum_deviation.min(extruder.settings.get_coord("meshfix_maximum_deviation"));
        }
        storage.draft_protection_shield = Simplify::new(maximum_resolution, maximum_deviation, 0).shape(&storage.draft_protection_shield);
        if storage.prime_tower.is_some() {
            let max_line_width = Self::max_used_skirt_brim_line_width(storage);
            let ground = storage.prime_tower.as_ref().unwrap().get_occupied_ground_outline().offset((max_line_width / 2) as i32, JoinType::Miter, 1.2);
            storage.draft_protection_shield = storage.draft_protection_shield.difference(&ground);
        }
    }

    fn process_platform_adhesion(&mut self, storage: &mut SliceDataStorage) {
        let mesh_group_settings = &current_slice().scene.current_mesh_group().settings;
        let adhesion_type = mesh_group_settings.get_enum::<EPlatformAdhesion>("adhesion_type");

        if adhesion_type == EPlatformAdhesion::RAFT {
            Raft::generate(storage);
            return;
        }

        let mut skirt_brim = SkirtBrim::new(storage);
        if adhesion_type != EPlatformAdhesion::NONE {
            skirt_brim.generate(storage);
        }

        if mesh_group_settings.get_bool("support_brim_enable") {
            skirt_brim.generate_support_brim(storage);
        }
    }

    fn process_fuzzy_walls(&mut self, mesh: &mut SliceMeshStorage) {
        let settings = mesh.settings();
        if settings.get_usize("wall_line_count") == 0 {
            return;
        }
        let line_width = settings.get_coord("line_width");
        let apply_outside_only = settings.get_bool("magic_fuzzy_skin_outside_only");
        let fuzziness = settings.get_coord("magic_fuzzy_skin_thickness");
        let avg_dist_between_points = settings.get_coord("magic_fuzzy_skin_point_dist");
        let min_dist_between_points = avg_dist_between_points * 3 / 4; // hardcoded: the point distance may vary between 3/4 and 5/4 the supplied value
        let range_random_point_dist = avg_dist_between_points / 2;
        let start_layer_nr: u32 = if settings.get_enum::<EPlatformAdhesion>("adhesion_type") == EPlatformAdhesion::BRIM { 1 } else { 0 }; // don't make fuzzy skin on first layer if there's a brim
        let rand = || cppstd::crand::rand() as i64;

        let mut layer_nr = start_layer_nr as LayerIndex;
        while layer_nr < mesh.layers.len() as LayerIndex {
            let layer = &mut mesh.layers[layer_nr as usize];
            for part in layer.parts.iter_mut() {
                let mut result_paths: Vec<VariableWidthLines> = Vec::new();
                let mut hole_area = Shape::default();
                let mut use_hole_area = false;
                for toolpath in &part.wall_toolpaths {
                    if toolpath[0].inset_idx != 0 {
                        result_paths.push(toolpath.clone());
                        continue;
                    }

                    let mut result_lines: VariableWidthLines = Vec::new();
                    if apply_outside_only {
                        hole_area = part.print_outline.get_outside_polygons().offset(-line_width, JoinType::Miter, 1.2);
                        use_hole_area = true;
                    }
                    for line in toolpath {
                        if apply_outside_only && use_hole_area && line.junctions.iter().any(|j| hole_area.inside(j.p, false)) {
                            result_lines.push(line.clone());
                            continue;
                        }
                        let mut result = ExtrusionLine::new(line.inset_idx, line.is_odd, line.is_closed);

                        // generate points in between p0 and p1
                        let mut dist_left_over: i64 = (min_dist_between_points / 4) + rand() % (min_dist_between_points / 4); // the distance to be traversed on the line before making the first new point
                        let mut p0 = line.junctions[0];
                        for p1 in &line.junctions {
                            if p0.p == p1.p {
                                // avoid seams
                                result.junctions.push(ExtrusionJunction::new(p1.p, p1.w, p1.perimeter_index as Coord));
                                continue;
                            }

                            // 'a' is the (next) new point between p0 and p1
                            let p0p1 = p1.p - p0.p;
                            let p0p1_size = v_size(p0p1);
                            let mut p0pa_dist = dist_left_over;
                            if p0pa_dist >= p0p1_size {
                                let p = p1.p - (p0p1 / 2);
                                let width = (p1.w * v_size(p1.p - p) + p0.w * v_size(p0.p - p)) / p0p1_size;
                                result.junctions.push(ExtrusionJunction::new(p, width as f64 as Coord, p1.perimeter_index as Coord));
                            }
                            while p0pa_dist < p0p1_size {
                                let r: i32 = (rand() % (fuzziness * 2) - fuzziness) as i32;
                                let perp_to_p0p1 = turn90_ccw(p0p1);
                                let fuzz = normal(perp_to_p0p1, r as Coord);
                                let pa = p0.p + normal(p0p1, p0pa_dist);
                                let width = (p1.w * v_size(p1.p - pa) + p0.w * v_size(p0.p - pa)) / p0p1_size;
                                result.junctions.push(ExtrusionJunction::new(pa + fuzz, width as f64 as Coord, p1.perimeter_index as Coord));
                                p0pa_dist += min_dist_between_points + rand() % range_random_point_dist;
                            }
                            // p0pa_dist > p0p1_size now because we broke out of the for-loop
                            dist_left_over = p0pa_dist - p0p1_size;

                            p0 = *p1;
                        }
                        while result.junctions.len() < 3 {
                            let mut point_idx = line.junctions.len() - 2;
                            let j = line.junctions[point_idx];
                            result.junctions.push(ExtrusionJunction::new(j.p, j.w, j.perimeter_index as Coord));
                            if point_idx == 0 {
                                break;
                            }
                            point_idx -= 1;
                            let _ = point_idx;
                        }
                        if result.junctions.len() < 3 {
                            result.junctions.clear();
                            for p in &line.junctions {
                                result.junctions.push(ExtrusionJunction::new(p.p, p.w, p.perimeter_index as Coord));
                            }
                        }
                        if line.junctions.last().unwrap().p == line.junctions[0].p {
                            // keep polygons closed
                            let front = result.junctions[0].p;
                            result.junctions.last_mut().unwrap().p = front;
                        }
                        result_lines.push(result);
                    }
                    result_paths.push(result_lines);
                }
                part.wall_toolpaths = result_paths;
            }
            layer_nr += 1;
        }
    }
}
