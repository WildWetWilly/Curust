//! `raft.h/.cpp`.

use crate::application::current_slice;
use crate::geometry::Shape;
use crate::settings::enums::EPlatformAdhesion;
use crate::settings::types::LayerIndex;
use crate::slice_data_storage::SliceDataStorage;
use crate::utils::coord::Coord;
use crate::utils::math::round_divide;
use clipper::JoinType;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RaftLayerType {
    RaftBase,
    RaftInterface,
    RaftSurface,
    Airgap,
    Model,
}

pub struct Raft;

impl Raft {
    pub fn generate(storage: &mut SliceDataStorage) {
        debug_assert!(storage.raft_base_outline.is_empty() && storage.raft_interface_outline.is_empty() && storage.raft_surface_outline.is_empty());
        let settings = &current_slice().scene.current_mesh_group().settings.get_extruder_train("raft_base_extruder_nr").settings;
        let include_support = true;
        let dont_include_prime_tower = false;
        let raft_base_margin = settings.get_coord("raft_base_margin");
        let raft_interface_margin = settings.get_coord("raft_interface_margin");
        let raft_surface_margin = settings.get_coord("raft_surface_margin");

        let outlines = storage.get_layer_outlines(0, include_support, dont_include_prime_tower, false, -1, true);
        storage.raft_base_outline = outlines.clone();
        storage.raft_surface_outline = outlines.clone();
        storage.raft_interface_outline = outlines;
        storage.raft_base_outline = storage.raft_base_outline.offset(raft_base_margin, JoinType::Round, 1.2);
        storage.raft_interface_outline = storage.raft_interface_outline.offset(raft_interface_margin, JoinType::Round, 1.2);
        storage.raft_surface_outline = storage.raft_surface_outline.offset(raft_surface_margin, JoinType::Round, 1.2);

        let shield_line_width_layer0 = settings.get_coord("skirt_brim_line_width");
        let max_raft_distance = raft_base_margin.max(raft_interface_margin).max(raft_surface_margin);
        if !storage.draft_protection_shield.is_empty() {
            let draft_shield_raft = storage
                .draft_protection_shield
                .offset_default(shield_line_width_layer0) // start half a line width outside shield
                .difference(&storage.draft_protection_shield.offset(-max_raft_distance - shield_line_width_layer0 / 2, JoinType::Round, 1.2)); // end distance inside shield
            storage.raft_base_outline = storage.raft_base_outline.union_polygons_with(&draft_shield_raft, clipper::PolyFillType::NonZero);
            storage.raft_surface_outline = storage.raft_surface_outline.union_polygons_with(&draft_shield_raft, clipper::PolyFillType::NonZero);
            storage.raft_interface_outline = storage.raft_interface_outline.union_polygons_with(&draft_shield_raft, clipper::PolyFillType::NonZero);
        }
        if !storage.ooze_shield.is_empty() && !storage.ooze_shield[0].is_empty() {
            let ooze_shield = &storage.ooze_shield[0];
            let ooze_shield_raft = ooze_shield
                .offset_default(shield_line_width_layer0) // start half a line width outside shield
                .difference(&ooze_shield.offset(-max_raft_distance - shield_line_width_layer0 / 2, JoinType::Round, 1.2)); // end distance inside shield
            storage.raft_base_outline = storage.raft_base_outline.union_polygons_with(&ooze_shield_raft, clipper::PolyFillType::NonZero);
            storage.raft_surface_outline = storage.raft_surface_outline.union_polygons_with(&ooze_shield_raft, clipper::PolyFillType::NonZero);
            storage.raft_interface_outline = storage.raft_interface_outline.union_polygons_with(&ooze_shield_raft, clipper::PolyFillType::NonZero);
        }

        let remove_inside_corners = |outline: &mut Shape, remove_inside_corners: bool, smoothing: Coord, line_width: Coord| {
            if remove_inside_corners {
                *outline = outline.union_polygons();
                let mut outline_parts = outline.union_polygons().split_into_parts(false);

                // Make the outline convex, but keep the number of parts the same.
                let mut nr_of_parts = outline_parts.len();
                loop {
                    outline.clear();
                    for part in outline_parts.iter_mut() {
                        part.0.make_convex();
                        outline.push_all(&part.0);
                    }
                    *outline = outline.union_polygons();
                    outline_parts = outline.split_into_parts(false);

                    let new_nr_of_parts = outline_parts.len();
                    if new_nr_of_parts > nr_of_parts {
                        crate::log_warn!("Error while removing inside corners from raft; merge operation increased the number of parts");
                        break;
                    }
                    if new_nr_of_parts == nr_of_parts {
                        break;
                    }
                    nr_of_parts = new_nr_of_parts;
                }
            } else {
                *outline = outline.offset(smoothing, JoinType::Round, 1.2).offset(-smoothing, JoinType::Round, 1.2);
                *outline = outline.offset(-line_width, JoinType::Round, 1.2).offset(line_width, JoinType::Round, 1.2);
            }
        };
        let nominal_raft_line_width = settings.get_coord("skirt_brim_line_width");
        remove_inside_corners(
            &mut storage.raft_base_outline,
            settings.get_bool("raft_base_remove_inside_corners"),
            settings.get_coord("raft_base_smoothing"),
            nominal_raft_line_width,
        );
        remove_inside_corners(
            &mut storage.raft_interface_outline,
            settings.get_bool("raft_interface_remove_inside_corners"),
            settings.get_coord("raft_interface_smoothing"),
            nominal_raft_line_width,
        );
        remove_inside_corners(
            &mut storage.raft_surface_outline,
            settings.get_bool("raft_surface_remove_inside_corners"),
            settings.get_coord("raft_surface_smoothing"),
            nominal_raft_line_width,
        );
    }

    pub fn get_total_thickness() -> Coord {
        let mesh_group_settings = &current_slice().scene.current_mesh_group().settings;
        let base_train = &mesh_group_settings.get_extruder_train("raft_base_extruder_nr").settings;
        let interface_train = &mesh_group_settings.get_extruder_train("raft_interface_extruder_nr").settings;
        let surface_train = &mesh_group_settings.get_extruder_train("raft_surface_extruder_nr").settings;
        // size_t * coord_t: unsigned 64-bit arithmetic, which wraps back to the same bits.
        base_train.get_coord("raft_base_thickness")
            + (interface_train.get_usize("raft_interface_layers") as i64).wrapping_mul(interface_train.get_coord("raft_interface_thickness"))
            + interface_train.get_coord("raft_interface_z_offset")
            + (surface_train.get_usize("raft_surface_layers") as i64).wrapping_mul(surface_train.get_coord("raft_surface_thickness"))
            + interface_train.get_coord("raft_surface_z_offset")
    }

    pub fn get_zdiff_between_raft_and_layer0() -> Coord {
        let mesh_group_settings = &current_slice().scene.current_mesh_group().settings;
        let train = mesh_group_settings.get_extruder_train("raft_surface_extruder_nr");
        if mesh_group_settings.get_enum::<EPlatformAdhesion>("adhesion_type") != EPlatformAdhesion::RAFT {
            return 0;
        }
        0i64.max(train.settings.get_coord("raft_airgap"))
    }

    pub fn get_filler_layer_count() -> usize {
        let normal_layer_height = current_slice().scene.current_mesh_group().settings.get_coord("layer_height");
        round_divide(Self::get_zdiff_between_raft_and_layer0() as u64, normal_layer_height as u64) as usize
    }

    pub fn get_filler_layer_height() -> Coord {
        let mesh_group_settings = &current_slice().scene.current_mesh_group().settings;
        if mesh_group_settings.get_enum::<EPlatformAdhesion>("adhesion_type") != EPlatformAdhesion::RAFT {
            return mesh_group_settings.get_coord("layer_height");
        }
        round_divide(Self::get_zdiff_between_raft_and_layer0() as u64, Self::get_filler_layer_count() as u64) as Coord
    }

    pub fn get_total_extra_layers() -> usize {
        Self::get_base_layers() + Self::get_interface_layers() + Self::get_surface_layers() + Self::get_filler_layer_count()
    }

    pub fn get_base_layers() -> usize {
        let mesh_group_settings = &current_slice().scene.current_mesh_group().settings;
        if mesh_group_settings.get_enum::<EPlatformAdhesion>("adhesion_type") != EPlatformAdhesion::RAFT {
            return 0;
        }
        1
    }

    pub fn get_interface_layers() -> usize {
        Self::get_layers_amount("raft_interface_extruder_nr", "raft_interface_layers")
    }

    pub fn get_surface_layers() -> usize {
        Self::get_layers_amount("raft_surface_extruder_nr", "raft_surface_layers")
    }

    pub fn get_layer_type(layer_index: LayerIndex) -> RaftLayerType {
        let airgap = Self::get_filler_layer_count() as LayerIndex;
        let interface_layers = Self::get_interface_layers() as LayerIndex;
        let surface_layers = Self::get_surface_layers() as LayerIndex;

        if layer_index < -(airgap + surface_layers + interface_layers) {
            return RaftLayerType::RaftBase;
        }
        if layer_index < -(airgap + surface_layers) {
            return RaftLayerType::RaftInterface;
        }
        if layer_index < -airgap {
            return RaftLayerType::RaftSurface;
        }
        if layer_index < 0 {
            return RaftLayerType::Airgap;
        }
        RaftLayerType::Model
    }

    fn get_layers_amount(extruder_nr_setting_name: &str, target_raft_section: &str) -> usize {
        let mesh_group_settings = &current_slice().scene.current_mesh_group().settings;
        if mesh_group_settings.get_enum::<EPlatformAdhesion>("adhesion_type") != EPlatformAdhesion::RAFT {
            return 0;
        }
        let train = mesh_group_settings.get_extruder_train(extruder_nr_setting_name);
        train.settings.get_usize(target_raft_section)
    }
}
