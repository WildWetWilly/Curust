//! `sliceDataStorage.h/.cpp`: all the data produced while slicing one mesh group.

use crate::application::current_slice;
use crate::geometry::lines_set::ClosedLinesSet;
use crate::geometry::mixed_lines_set::MixedLinesSet;
use crate::geometry::point2ll::Point2LL;
use crate::geometry::point3ll::Point3LL;
use crate::geometry::{OpenLinesSet, Shape, SingleShape};
use crate::infill::lightning_generator::LightningGenerator;
use crate::infill::sierpinski_fill_provider::SierpinskiFillProvider;
use crate::infill::sub_div_cube::SubDivCube;
use crate::mesh::Mesh;
use crate::prime_tower::PrimeTower;
use crate::raft::{Raft, RaftLayerType};
use crate::retraction_config::RetractionAndWipeConfig;
use crate::settings::enums::{BuildPlateShape, EPlatformAdhesion, ESurfaceMode};
use crate::settings::settings::{Settings, MAX_EXTRUDERS};
use crate::settings::types::{AngleDegrees, LayerIndex};
use crate::support_infill_part::SupportInfillPart;
use crate::texture_data_provider::TextureDataProvider;
use crate::top_surface::TopSurface;
use crate::utils::aabb::{AABB, AABB3D};
use crate::utils::coord::Coord;
use crate::utils::extrusion_line::VariableWidthLines;
use crate::utils::polygon_utils::PolygonUtils;
use clipper::{JoinType, PolyFillType};
use std::sync::Arc;

#[derive(Clone, Debug, Default)]
pub struct SkinPart {
    /// The skinOutline is the area which needs to be 100% filled to generate a proper top&bottom filling. It's filled by the "skin" module.
    pub outline: SingleShape,
    /// The skin can have perimeters so that the skin lines always start at a perimeter instead of in the middle of an infill cell.
    pub skin_fill: Shape,
    /// The roofing fill: the top most skin areas.
    pub roofing_fill: Shape,
    /// The flooring fill: the bottom most skin areas.
    pub flooring_fill: Shape,
}

/// The SliceLayerPart is a single enclosed printable area for a single layer.
#[derive(Clone, Debug, Default)]
pub struct SliceLayerPart {
    /// The boundaryBox is an axis-aligned boundary box which is used to quickly check for possible collision between different parts on different layers.
    pub boundary_box: AABB,
    /// The outline is the first member that is filled, and it's filled with polygons that match a cross section of the 3D model.
    pub outline: SingleShape,
    /// The print outline is the outline of the part as it will be printed (after compensation for wall overlap etc.)
    pub print_outline: Shape,
    /// The spiral wall used in spiralize mode
    pub spiral_wall: Shape,
    /// The area enclosed by the innermost wall of this part
    pub inner_area: Shape,
    /// The skin parts which are filled for 100% with lines and/or insets.
    pub skin_parts: Vec<SkinPart>,
    /// The wall toolpaths, binned by inset_idx.
    pub wall_toolpaths: Vec<VariableWidthLines>,
    /// The walls of the infill (for gradual infill support walls), binned by inset_idx.
    pub infill_wall_toolpaths: Vec<VariableWidthLines>,
    /// The top most surface (used for monotonic/ironing etc.)
    pub top_most_surface: Shape,
    pub bottom_most_surface: Shape,
    /// The infill area is the area which needs to be filled with sparse (0-99%) infill.
    pub infill_area: Shape,
    /// The infill area which belongs to this mesh and not to an infill mesh within.
    pub infill_area_own: Option<Shape>,
    /// The infill areas per density and per combine layer count.
    pub infill_area_per_combine_per_density: Vec<Vec<Shape>>,
}

impl SliceLayerPart {
    pub fn get_own_infill_area(&self) -> &Shape {
        match &self.infill_area_own {
            Some(s) => s,
            None => &self.infill_area,
        }
    }

    pub fn get_own_infill_area_mut(&mut self) -> &mut Shape {
        match &mut self.infill_area_own {
            Some(s) => s,
            None => &mut self.infill_area,
        }
    }

    pub fn has_wall_at_inset_index(&self, inset_idx: usize) -> bool {
        for lines in &self.wall_toolpaths {
            for line in lines {
                if line.inset_idx == inset_idx {
                    return true;
                }
            }
        }
        false
    }
}

/// The SliceLayer contains all the data for a single cross section of the 3D model.
#[derive(Default)]
pub struct SliceLayer {
    /// The height at which this layer needs to be printed. Can differ from sliceZ due to the raft.
    pub print_z: Coord,
    /// The thickness of this layer. Can be different when using variable layer heights.
    pub thickness: Coord,
    /// An array of LayerParts which contain the actual data. The parts are printed one at a time to minimize travel outside of the 3D model.
    pub parts: Vec<SliceLayerPart>,
    /// The open polylines (surface mode / slicing open meshes).
    pub open_polylines: OpenLinesSet,
    pub texture_data_provider: Option<Arc<TextureDataProvider>>,
    /// The parts of the model that are exposed at the very top of the model.
    pub top_surface: TopSurface,
    /// The parts of the model that are exposed at the bottom(s) of the model.
    pub bottom_surface: Shape,
}

impl SliceLayer {
    pub fn get_outlines(&self, external_polys_only: bool) -> Shape {
        let mut ret = Shape::default();
        self.get_outlines_into(&mut ret, external_polys_only);
        ret
    }

    pub fn get_outlines_into(&self, result: &mut Shape, external_polys_only: bool) {
        for part in &self.parts {
            if external_polys_only {
                result.push(part.outline.outer_polygon().clone());
            } else {
                result.push_all(&part.print_outline);
            }
        }
    }
}

/// Support structure for a single layer.
#[derive(Clone, Debug, Default)]
pub struct SupportLayer {
    /// a list of support infill parts
    pub support_infill_parts: Vec<SupportInfillPart>,
    /// Piece of support below the support and above the model. This must not overlap with any of the support_infill_parts or support_roof.
    pub support_bottom: Shape,
    /// Piece of support above the support and below the model. This must not overlap with any of the support_infill_parts or support_bottom.
    pub support_roof: Shape,
    /// If the support distance is not exactly a multiple of the layer height, the first part of support just underneath the model needs to be printed at a fraction of the layer height.
    pub support_fractional_roof: Shape,
    /// Areas from support meshes which should be supported by more support
    pub support_mesh_drop_down: Shape,
    /// Areas from support meshes which should NOT be supported by more support
    pub support_mesh: Shape,
    /// Areas where no support may be.
    pub anti_overhang: Shape,
}

impl SupportLayer {
    /// Exclude the given polygons from the support infill areas and update the SupportInfillParts.
    pub fn exclude_areas_from_support_infill_areas(&mut self, exclude_polygons: &Shape, exclude_polygons_boundary_box: &AABB) {
        // record the indexes that need to be removed and do that after
        let mut to_remove_part_indices: Vec<usize> = Vec::new(); // LIFO for removing

        let part_count_to_check = self.support_infill_parts.len() as u32; // note that support_infill_parts.size() changes during the loop
        for part_idx in 0..part_count_to_check as usize {
            let (bbox, line_width, inset_count) = {
                let p = &self.support_infill_parts[part_idx];
                (p.outline_boundary_box, p.support_line_width, p.inset_count_to_generate)
            };
            if !exclude_polygons_boundary_box.hit(&bbox) {
                continue;
            }

            let result_polygons = self.support_infill_parts[part_idx].outline.0.difference(exclude_polygons);

            // if no smaller parts get generated, we mark this part to be removed
            if result_polygons.is_empty() {
                to_remove_part_indices.push(part_idx);
                continue;
            }

            let smaller_support_islands = result_polygons.split_into_parts(false);

            if smaller_support_islands.is_empty() {
                // extra safety guard in case result_polygons consists of too small polygons which are automatically removed in splitIntoParts
                to_remove_part_indices.push(part_idx);
                continue;
            }

            // there are one or more smaller parts.
            // we first replace the current part with one of the smaller parts,
            // the rest we add to the support_infill_parts (but after part_count_to_check)
            self.support_infill_parts[part_idx].outline = smaller_support_islands[0].clone();

            for support_island_idx in 1..smaller_support_islands.len() {
                let smaller_island = &smaller_support_islands[support_island_idx];
                // `emplace_back(smaller_island, support_line_width_, inset_count_to_generate_)`: the inset count
                // binds to the `use_fractional_config` parameter.
                self.support_infill_parts.push(SupportInfillPart::new(smaller_island, line_width, inset_count != 0, 0, 0));
            }
        }

        // remove the ones that need to be removed (LIFO)
        while let Some(remove_idx) = to_remove_part_indices.pop() {
            if self.support_infill_parts.is_empty() {
                continue;
            }
            if remove_idx < self.support_infill_parts.len() - 1 {
                // move last element to the to-be-removed element so that we can erase the last one instead
                let last = self.support_infill_parts.pop().unwrap();
                self.support_infill_parts[remove_idx] = last;
            } else {
                self.support_infill_parts.pop();
            }
        }
    }

    /// Fill the support infill parts from a given area.
    pub fn fill_infill_parts_area(
        &mut self,
        area: &Shape,
        support_line_width: Coord,
        wall_line_count: Coord,
        use_fractional_config: bool,
        union_all: bool,
        custom_line_distance: Coord,
    ) {
        for island_outline in area.split_into_parts(union_all) {
            self.support_infill_parts.push(SupportInfillPart::new(
                &island_outline,
                support_line_width,
                use_fractional_config,
                wall_line_count as i32,
                custom_line_distance,
            ));
        }
    }

    /// Fill the support infill parts of a layer, separating the parts just below an overhang (fractional layers).
    pub fn fill_infill_parts(
        &mut self,
        layer_nr: LayerIndex,
        support_fill_per_layer: &[Shape],
        infill_layer_height: Coord,
        meshes: &[Box<SliceMeshStorage>],
        support_line_width: Coord,
        wall_line_count: Coord,
        _grow_layer_above: Coord,
        union_all: bool,
        custom_line_distance: Coord,
    ) {
        // Find the model exactly z-distance above the support layer.
        let mut overhang_z_dist_above = Shape::default();
        for mesh in meshes {
            let mesh_z_distance_top = mesh.settings().get_coord("support_top_distance");
            let overhang_layer_nr = (layer_nr + (mesh_z_distance_top / infill_layer_height) + 1) as usize;
            if overhang_layer_nr < mesh.overhang_areas.len() {
                overhang_z_dist_above.push_all(&mesh.overhang_areas[overhang_layer_nr]);
            }
        }
        overhang_z_dist_above = overhang_z_dist_above.union_polygons();

        // Split the support outline into areas that are directly under the overhang and areas that are not.
        let all_support_areas_in_layer = [
            support_fill_per_layer[layer_nr as usize].intersection(&overhang_z_dist_above),
            support_fill_per_layer[layer_nr as usize].difference(&overhang_z_dist_above),
        ];
        let mut use_fractional_config = true;
        for support_areas in &all_support_areas_in_layer {
            for island_outline in support_areas.split_into_parts(union_all) {
                self.support_infill_parts.push(SupportInfillPart::new(
                    &island_outline,
                    support_line_width,
                    use_fractional_config,
                    wall_line_count as i32,
                    custom_line_distance,
                ));
            }
            use_fractional_config = false;
        }
    }
}

pub struct SupportStorage {
    /// whether generateSupportGrid(.) has completed (successfully)
    pub generated: bool,
    /// the layer number of the last layer with support infill
    pub layer_nr_max_filled_layer: i32,
    /// a list of angle values which is cycled through to determine the infill angle of each layer
    pub support_infill_angles: Vec<AngleDegrees>,
    /// a list of angle values which is cycled through to determine the infill angle of each layer
    pub support_infill_angles_layer_0: Vec<AngleDegrees>,
    /// a list of angle values which is cycled through to determine the infill angle of each layer
    pub support_roof_angles: Vec<AngleDegrees>,
    /// a list of angle values which is cycled through to determine the infill angle of each layer
    pub support_bottom_angles: Vec<AngleDegrees>,
    pub support_layers: Vec<SupportLayer>,
    /// the fractal pattern for the cross (3d) filling pattern
    pub cross_fill_provider: Option<Arc<SierpinskiFillProvider>>,
}

impl Default for SupportStorage {
    fn default() -> Self {
        SupportStorage {
            generated: false,
            layer_nr_max_filled_layer: -1,
            support_infill_angles: Vec::new(),
            support_infill_angles_layer_0: Vec::new(),
            support_roof_angles: Vec::new(),
            support_bottom_angles: Vec::new(),
            support_layers: Vec::new(),
            cross_fill_provider: None,
        }
    }
}

pub struct SliceMeshStorage {
    /// The mesh whose settings are used (`Settings& settings`).
    mesh: *const Mesh,
    pub layers: Vec<SliceLayer>,
    pub mesh_name: String,

    /// the layer number of the uppermost layer with content (modified while infill meshes are processed)
    pub layer_nr_max_filled_layer: LayerIndex,

    /// a list of angle values which is cycled through to determine the infill angle of each layer
    pub infill_angles: Vec<AngleDegrees>,
    /// a list of angle values which is cycled through to determine the roofing angle of each layer
    pub roofing_angles: Vec<AngleDegrees>,
    /// a list of angle values which is cycled through to determine the flooring angle of each layer
    pub flooring_angles: Vec<AngleDegrees>,
    /// a list of angle values which is cycled through to determine the skin angle of each layer
    pub skin_angles: Vec<AngleDegrees>,
    /// For each layer the areas that are classified as overhang.
    pub overhang_areas: Vec<Shape>,
    /// For each layer the full overhang without the tangent of the overhang angle removed.
    pub full_overhang_areas: Vec<Shape>,
    /// For each layer, for each overhang "pocket", the points that need to be supported (for tree support).
    pub overhang_points: Vec<Vec<Shape>>,
    /// the mesh's bounding box
    pub bounding_box: AABB3D,

    /// the root of a tree of cubes for the cubic subdivision infill pattern
    pub base_subdiv_cube: Option<Arc<SubDivCube>>,
    /// the fractal pattern for the cross (3d) filling pattern
    pub cross_fill_provider: Option<Arc<SierpinskiFillProvider>>,
    /// Pre-computed tree data for the lightning infill pattern
    pub lightning_generator: Option<Arc<LightningGenerator>>,

    /// Per-mesh retraction and wipe settings.
    pub retraction_wipe_config: RetractionAndWipeConfig,
}

unsafe impl Send for SliceMeshStorage {}
unsafe impl Sync for SliceMeshStorage {}

impl SliceMeshStorage {
    pub fn new(mesh: &Mesh, slice_layer_count: usize) -> Self {
        SliceMeshStorage {
            mesh: mesh as *const Mesh,
            layers: (0..slice_layer_count).map(|_| SliceLayer::default()).collect(),
            mesh_name: mesh.mesh_name.clone(),
            layer_nr_max_filled_layer: 0,
            infill_angles: Vec::new(),
            roofing_angles: Vec::new(),
            flooring_angles: Vec::new(),
            skin_angles: Vec::new(),
            overhang_areas: Vec::new(),
            full_overhang_areas: Vec::new(),
            overhang_points: Vec::new(),
            bounding_box: mesh.get_aabb(),
            base_subdiv_cube: None,
            cross_fill_provider: None,
            lightning_generator: None,
            retraction_wipe_config: RetractionAndWipeConfig::default(),
        }
    }

    /// `SliceMeshStorage::settings` (a reference to the mesh's settings).
    #[inline]
    pub fn settings(&self) -> &'static Settings {
        // SAFETY: the mesh lives in the current mesh group for the whole slice.
        unsafe { &(*self.mesh).settings }
    }

    /// Mutable access to the mesh's settings (C++ `SliceMeshStorage::settings` is a non-const
    /// reference; `precomputeCrossInfillTree` assigns through it).
    ///
    /// # Safety
    /// Only call from sequential code while no other reference to these settings is being used.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn settings_mut(&self) -> &'static mut Settings {
        &mut (*(self.mesh as *mut Mesh)).settings
    }

    #[inline]
    pub fn mesh(&self) -> &'static Mesh {
        unsafe { &*self.mesh }
    }

    pub fn get_extruder_is_used(&self, extruder_nr: usize) -> bool {
        let settings = self.settings();
        if settings.get_bool("anti_overhang_mesh") || settings.get_bool("support_mesh") {
            // object is not printed as object, but as support.
            return false;
        }
        if settings.get_bool("magic_spiralize") {
            return settings.get_extruder_train("wall_0_extruder_nr").extruder_nr == extruder_nr;
        }
        if settings.get_enum::<ESurfaceMode>("magic_mesh_surface_mode") != ESurfaceMode::NORMAL
            && settings.get_extruder_train("wall_0_extruder_nr").extruder_nr == extruder_nr
        {
            return true;
        }
        if settings.get_usize("wall_line_count") > 0 && settings.get_extruder_train("wall_0_extruder_nr").extruder_nr == extruder_nr {
            return true;
        }
        if (settings.get_usize("wall_line_count") > 1 || settings.get_bool("alternate_extra_perimeter"))
            && settings.get_extruder_train("wall_x_extruder_nr").extruder_nr == extruder_nr
        {
            return true;
        }
        if settings.get_coord("infill_line_distance") > 0 && settings.get_extruder_train("infill_extruder_nr").extruder_nr == extruder_nr {
            return true;
        }
        if (settings.get_usize("top_layers") > 0 || settings.get_usize("bottom_layers") > 0)
            && settings.get_extruder_train("top_bottom_extruder_nr").extruder_nr == extruder_nr
        {
            return true;
        }
        let roofing_layer_count = settings.get_usize("roofing_layer_count").min(settings.get_usize("top_layers"));
        if roofing_layer_count > 0 && settings.get_extruder_train("roofing_extruder_nr").extruder_nr == extruder_nr {
            return true;
        }
        let flooring_layer_count = settings.get_usize("flooring_layer_count").min(settings.get_usize("bottom_layers"));
        if flooring_layer_count > 0 && settings.get_extruder_train("flooring_extruder_nr").extruder_nr == extruder_nr {
            return true;
        }
        false
    }

    pub fn get_extruder_is_used_on_layer(&self, extruder_nr: usize, layer_nr: LayerIndex) -> bool {
        if layer_nr < 0 || layer_nr >= self.layers.len() as i32 as LayerIndex {
            return false;
        }
        let settings = self.settings();
        if settings.get_bool("anti_overhang_mesh") || settings.get_bool("support_mesh") {
            // object is not printed as object, but as support.
            return false;
        }
        let layer = &self.layers[layer_nr as usize];
        if settings.get_extruder_train("wall_0_extruder_nr").extruder_nr == extruder_nr
            && (settings.get_usize("wall_line_count") > 0 || settings.get_usize("skin_outline_count") > 0)
        {
            for part in &layer.parts {
                if part.has_wall_at_inset_index(0) || !part.spiral_wall.is_empty() {
                    return true;
                }
            }
        }
        if settings.get_enum::<ESurfaceMode>("magic_mesh_surface_mode") != ESurfaceMode::NORMAL
            && settings.get_extruder_train("wall_0_extruder_nr").extruder_nr == extruder_nr
            && !layer.open_polylines.is_empty()
        {
            return true;
        }
        if (settings.get_usize("wall_line_count") > 1 || settings.get_bool("alternate_extra_perimeter"))
            && settings.get_extruder_train("wall_x_extruder_nr").extruder_nr == extruder_nr
        {
            for part in &layer.parts {
                if part.has_wall_at_inset_index(1) {
                    return true;
                }
            }
        }
        if settings.get_coord("infill_line_distance") > 0 && settings.get_extruder_train("infill_extruder_nr").extruder_nr == extruder_nr {
            for part in &layer.parts {
                if !part.get_own_infill_area().is_empty() {
                    return true;
                }
            }
        }
        if settings.get_extruder_train("top_bottom_extruder_nr").extruder_nr == extruder_nr {
            for part in &layer.parts {
                for skin_part in &part.skin_parts {
                    if !skin_part.skin_fill.is_empty() {
                        return true;
                    }
                }
            }
        }
        if settings.get_extruder_train("roofing_extruder_nr").extruder_nr == extruder_nr {
            for part in &layer.parts {
                for skin_part in &part.skin_parts {
                    if !skin_part.roofing_fill.is_empty() {
                        return true;
                    }
                }
            }
        }
        if settings.get_extruder_train("flooring_extruder_nr").extruder_nr == extruder_nr {
            for part in &layer.parts {
                for skin_part in &part.skin_parts {
                    if !skin_part.flooring_fill.is_empty() {
                        return true;
                    }
                }
            }
        }
        false
    }

    pub fn is_printed(&self) -> bool {
        let settings = self.settings();
        !settings.get_bool("infill_mesh") && !settings.get_bool("cutting_mesh") && !settings.get_bool("anti_overhang_mesh")
    }

    pub fn get_z_seam_hint(&self) -> Point2LL {
        let settings = self.settings();
        let mut pos = Point2LL::new(settings.get_coord("z_seam_x"), settings.get_coord("z_seam_y"));
        if settings.get_bool("z_seam_relative") {
            let middle: Point3LL = self.bounding_box.get_middle();
            pos += Point2LL::new(middle.x, middle.y);
        }
        pos
    }
}

pub struct SliceDataStorage {
    pub print_layer_count: usize,

    pub model_size: Point3LL,
    pub model_min: Point3LL,
    pub model_max: Point3LL,
    pub machine_size: AABB3D,
    pub meshes: Vec<Box<SliceMeshStorage>>,

    pub retraction_wipe_config_per_extruder: Vec<RetractionAndWipeConfig>,

    pub support: SupportStorage,

    /// Skirt and brim polygons per extruder, ordered from inner to outer polygons.
    pub skirt_brim: Vec<Vec<MixedLinesSet>>,
    /// brim lines for support, going from the edge of the support inward.
    pub support_brim: ClosedLinesSet,

    pub raft_base_outline: Shape,
    pub raft_interface_outline: Shape,
    pub raft_surface_outline: Shape,

    /// Used in multi-extrusion: the layer number beyond which all models are printed with the same extruder
    pub max_print_height_second_to_last_extruder: i32,
    /// For each extruder the highest layer number at which it is used.
    pub max_print_height_per_extruder: Vec<i32>,
    /// Ordered indices into max_print_height_per_extruder: back() will return the extruder number with the highest print height.
    pub max_print_height_order: Vec<usize>,

    /// the index of the seam vertex for each layer
    pub spiralize_seam_vertex_indices: Vec<i32>,
    /// the wall outline polygons for each layer
    pub spiralize_wall_outlines: Vec<*const Shape>,

    pub prime_tower: Option<Box<PrimeTower>>,

    /// oozeShield per layer
    pub ooze_shield: Vec<Shape>,
    /// polygons for the draft shield
    pub draft_protection_shield: Shape,
}

unsafe impl Send for SliceDataStorage {}
unsafe impl Sync for SliceDataStorage {}

impl SliceDataStorage {
    pub fn new() -> Self {
        let mesh_group_settings = &current_slice().scene.current_mesh_group().settings;
        let mut machine_max = Point3LL::new(
            mesh_group_settings.get_coord("machine_width"),
            mesh_group_settings.get_coord("machine_depth"),
            mesh_group_settings.get_coord("machine_height"),
        );
        let mut machine_min = Point3LL::new(0, 0, 0);
        if mesh_group_settings.get_bool("machine_center_is_zero") {
            machine_max = machine_max / 2;
            machine_min -= machine_max;
        }
        let mut machine_size = AABB3D::default();
        machine_size.include(machine_min);
        machine_size.include(machine_max);

        SliceDataStorage {
            print_layer_count: 0,
            model_size: Point3LL::default(),
            model_min: Point3LL::default(),
            model_max: Point3LL::default(),
            machine_size,
            meshes: Vec::new(),
            retraction_wipe_config_per_extruder: Self::initialize_retraction_and_wipe_configs(),
            support: SupportStorage::default(),
            skirt_brim: (0..MAX_EXTRUDERS).map(|_| Vec::new()).collect(),
            support_brim: ClosedLinesSet::default(),
            raft_base_outline: Shape::default(),
            raft_interface_outline: Shape::default(),
            raft_surface_outline: Shape::default(),
            max_print_height_second_to_last_extruder: -1,
            max_print_height_per_extruder: Vec::new(),
            max_print_height_order: Vec::new(),
            spiralize_seam_vertex_indices: Vec::new(),
            spiralize_wall_outlines: Vec::new(),
            prime_tower: None,
            ooze_shield: Vec::new(),
            draft_protection_shield: Shape::default(),
        }
    }

    fn initialize_retraction_and_wipe_configs() -> Vec<RetractionAndWipeConfig> {
        vec![RetractionAndWipeConfig::default(); current_slice().scene.extruders.len()]
    }

    /// Get all outlines within a given layer.
    pub fn get_layer_outlines(
        &self,
        layer_nr: LayerIndex,
        include_support: bool,
        include_prime_tower: bool,
        external_polys_only: bool,
        extruder_nr: i32,
        include_models: bool,
    ) -> Shape {
        let mesh_group_settings = &current_slice().scene.current_mesh_group().settings;

        let layer_type = Raft::get_layer_type(layer_nr);
        match layer_type {
            RaftLayerType::RaftBase | RaftLayerType::RaftInterface | RaftLayerType::RaftSurface => {
                let raft_outline: &Shape;
                let mut use_current_extruder_for_raft = extruder_nr == -1;

                match layer_type {
                    RaftLayerType::RaftBase => {
                        raft_outline = &self.raft_base_outline;
                        use_current_extruder_for_raft |= extruder_nr == mesh_group_settings.get_extruder_train("raft_base_extruder_nr").extruder_nr as i32;
                    }
                    RaftLayerType::RaftInterface => {
                        raft_outline = &self.raft_interface_outline;
                        use_current_extruder_for_raft |=
                            extruder_nr == mesh_group_settings.get_extruder_train("raft_interface_extruder_nr").extruder_nr as i32;
                    }
                    RaftLayerType::RaftSurface => {
                        raft_outline = &self.raft_surface_outline;
                        use_current_extruder_for_raft |= extruder_nr == mesh_group_settings.get_extruder_train("raft_surface_extruder_nr").extruder_nr as i32;
                    }
                    _ => unreachable!(),
                }

                if include_support && use_current_extruder_for_raft {
                    if external_polys_only {
                        let parts = raft_outline.split_into_parts(false);
                        let mut result = Shape::default();
                        for part in &parts {
                            result.push(part.outer_polygon().clone());
                        }
                        result
                    } else {
                        raft_outline.clone()
                    }
                } else {
                    Shape::default()
                }
            }
            RaftLayerType::Airgap | RaftLayerType::Model => {
                let mut total = Shape::default();
                if include_models && layer_nr >= 0 {
                    for mesh in &self.meshes {
                        let settings = mesh.settings();
                        if settings.get_bool("infill_mesh")
                            || settings.get_bool("anti_overhang_mesh")
                            || (extruder_nr != -1 && extruder_nr != settings.get_extruder_train("wall_0_extruder_nr").extruder_nr as i32)
                        {
                            continue;
                        }
                        let layer = &mesh.layers[layer_nr as usize];
                        layer.get_outlines_into(&mut total, external_polys_only);
                        if settings.get_enum::<ESurfaceMode>("magic_mesh_surface_mode") != ESurfaceMode::NORMAL {
                            total = total.union_polygons_with(&layer.open_polylines.offset(100, JoinType::Miter, 1.2), PolyFillType::NonZero);
                        }
                    }
                }
                if include_support
                    && (extruder_nr == -1 || extruder_nr == mesh_group_settings.get_extruder_train("support_infill_extruder_nr").extruder_nr as i32)
                {
                    let support_layer = &self.support.support_layers[0.max(layer_nr) as usize];
                    if self.support.generated {
                        for support_infill_part in &support_layer.support_infill_parts {
                            total.push_all(&support_infill_part.outline.0);
                        }
                        total.push_all(&support_layer.support_bottom);
                        total.push_all(&support_layer.support_roof);
                    }
                }
                if include_prime_tower {
                    if let Some(pt) = &self.prime_tower {
                        total.push(pt.get_occupied_outline(layer_nr).clone());
                    }
                }
                total
            }
        }
    }

    /// Get the axis-aligned bounding-box of the complete model (all meshes).
    pub fn get_model_bounding_box(&self) -> AABB3D {
        let mut bounding_box = AABB3D::default();
        for mesh in &self.meshes {
            bounding_box.include_aabb(&mesh.bounding_box);
        }
        bounding_box
    }

    /// Get the extruders used.
    pub fn get_extruders_used(&self) -> Vec<bool> {
        let scene = &current_slice().scene;
        let mut ret = vec![false; scene.extruders.len()];

        let mesh_group_settings = &scene.current_mesh_group().settings;
        let adhesion_type = mesh_group_settings.get_enum::<EPlatformAdhesion>("adhesion_type");
        if adhesion_type == EPlatformAdhesion::SKIRT || adhesion_type == EPlatformAdhesion::BRIM {
            for extruder_nr in 0..scene.extruders.len() {
                if !self.skirt_brim[extruder_nr].is_empty() {
                    ret[extruder_nr] = true;
                }
            }
            let skirt_brim_extruder_nr = mesh_group_settings.get_int("skirt_brim_extruder_nr");
            if skirt_brim_extruder_nr >= 0 {
                ret[skirt_brim_extruder_nr as usize] = true;
            }
        } else if adhesion_type == EPlatformAdhesion::RAFT {
            ret[mesh_group_settings.get_extruder_train("raft_base_extruder_nr").extruder_nr] = true;
            let num_interface_layers = mesh_group_settings.get_extruder_train("raft_interface_extruder_nr").settings.get_usize("raft_interface_layers");
            if num_interface_layers > 0 {
                ret[mesh_group_settings.get_extruder_train("raft_interface_extruder_nr").extruder_nr] = true;
            }
            let num_surface_layers = mesh_group_settings.get_extruder_train("raft_surface_extruder_nr").settings.get_usize("raft_surface_layers");
            if num_surface_layers > 0 {
                ret[mesh_group_settings.get_extruder_train("raft_surface_extruder_nr").extruder_nr] = true;
            }
        }

        // TODO: ooze shield, draft shield ..?

        // support
        // support is presupposed to be present...
        for mesh in &self.meshes {
            if mesh.settings().get_bool("support_enable") || mesh.settings().get_bool("support_mesh") {
                ret[mesh_group_settings.get_extruder_train("support_extruder_nr_layer_0").extruder_nr] = true;
                ret[mesh_group_settings.get_extruder_train("support_infill_extruder_nr").extruder_nr] = true;
                if mesh_group_settings.get_bool("support_roof_enable") {
                    ret[mesh_group_settings.get_extruder_train("support_roof_extruder_nr").extruder_nr] = true;
                }
                if mesh_group_settings.get_bool("support_bottom_enable") {
                    ret[mesh_group_settings.get_extruder_train("support_bottom_extruder_nr").extruder_nr] = true;
                }
            }
        }

        // all meshes are presupposed to actually have content
        for mesh in &self.meshes {
            for extruder_nr in 0..ret.len() {
                ret[extruder_nr] = ret[extruder_nr] || mesh.get_extruder_is_used(extruder_nr);
            }
        }
        ret
    }

    /// Get the extruders used on a particular layer.
    pub fn get_extruders_used_on_layer(&self, layer_nr: LayerIndex) -> Vec<bool> {
        let scene = &current_slice().scene;
        let extruders = &scene.extruders;
        let mut ret = vec![false; extruders.len()];

        let mesh_group_settings = &scene.current_mesh_group().settings;
        let adhesion_type = mesh_group_settings.get_enum::<EPlatformAdhesion>("adhesion_type");

        let mut include_adhesion = true;
        let mut include_helper_parts = true;
        let mut include_models = true;
        if layer_nr < 0 {
            include_models = false;
            if layer_nr < -(Raft::get_filler_layer_count() as LayerIndex) {
                include_helper_parts = false;
            }
        } else if layer_nr > 0 || adhesion_type == EPlatformAdhesion::RAFT {
            // only include adhesion only for layers where platform adhesion actually occurs
            // i.e. layers < 0 are for raft, layer 0 is for brim/skirt
            include_adhesion = false;
        }

        if include_adhesion {
            if layer_nr == 0 && (adhesion_type == EPlatformAdhesion::SKIRT || adhesion_type == EPlatformAdhesion::BRIM) {
                for extruder_nr in 0..extruders.len() {
                    if !self.skirt_brim[extruder_nr].is_empty() {
                        ret[extruder_nr] = true;
                    }
                }
            }
            if adhesion_type == EPlatformAdhesion::RAFT {
                let layer_type = Raft::get_layer_type(layer_nr);
                if layer_type == RaftLayerType::RaftBase {
                    ret[mesh_group_settings.get_extruder_train("raft_base_extruder_nr").extruder_nr] = true;
                    for extruder_nr in 0..extruders.len() {
                        if extruders[extruder_nr].settings.get_bool("prime_blob_enable") {
                            ret[extruder_nr] = true;
                        }
                    }
                } else if layer_type == RaftLayerType::RaftInterface {
                    ret[mesh_group_settings.get_extruder_train("raft_interface_extruder_nr").extruder_nr] = true;
                } else if layer_type == RaftLayerType::RaftSurface {
                    ret[mesh_group_settings.get_extruder_train("raft_surface_extruder_nr").extruder_nr] = true;
                }
            }
        }

        // TODO: ooze shield, draft shield ..?

        if include_helper_parts {
            // support
            if layer_nr < self.support.support_layers.len() as i32 as LayerIndex {
                let support_layer = &self.support.support_layers[0.max(layer_nr) as usize]; // Below layer 0, it's the same as layer 0 (even though it's not printed here).
                if layer_nr == 0 {
                    if !support_layer.support_infill_parts.is_empty() {
                        ret[mesh_group_settings.get_extruder_train("support_extruder_nr_layer_0").extruder_nr] = true;
                    }
                } else {
                    if !support_layer.support_infill_parts.is_empty() {
                        ret[mesh_group_settings.get_extruder_train("support_infill_extruder_nr").extruder_nr] = true;
                    }
                }
                if !support_layer.support_bottom.is_empty() {
                    ret[mesh_group_settings.get_extruder_train("support_bottom_extruder_nr").extruder_nr] = true;
                }
                if !support_layer.support_roof.is_empty() {
                    ret[mesh_group_settings.get_extruder_train("support_roof_extruder_nr").extruder_nr] = true;
                }
            }
        }

        if include_models {
            for mesh in &self.meshes {
                for extruder_nr in 0..ret.len() {
                    ret[extruder_nr] = ret[extruder_nr] || mesh.get_extruder_is_used_on_layer(extruder_nr, layer_nr);
                }
            }
        }

        ret
    }

    /// Gets whether prime blob is enabled for the given extruder number.
    pub fn get_extruder_prime_blob_enabled(&self, extruder_nr: usize) -> bool {
        let scene = &current_slice().scene;
        if extruder_nr >= scene.extruders.len() {
            return false;
        }
        scene.extruders[extruder_nr].settings.get_bool("prime_blob_enable")
    }

    /// Gets the border of the usable print area for this machine.
    pub fn get_machine_border(&self, checking_extruder_nr: i32) -> Shape {
        let scene = &current_slice().scene;
        let mesh_group_settings = &scene.current_mesh_group().settings;

        let mut border = self.get_raw_machine_border();

        let mut disallowed_areas = mesh_group_settings.get_shape("machine_disallowed_areas");
        disallowed_areas = disallowed_areas.union_polygons(); // union overlapping disallowed areas

        // The disallowed areas are always expressed in buildplate-centered coordinates
        if !mesh_group_settings.get_bool("machine_center_is_zero") {
            for poly in disallowed_areas.iter_mut() {
                for p in poly.points.iter_mut() {
                    *p = Point2LL::new(self.machine_size.max.x / 2 + p.x, self.machine_size.max.y / 2 - p.y);
                }
            }
        }

        let extruder_is_used = self.get_extruders_used();

        let prime_clearance: Coord = 6500; // MM2INT(6.5)
        for extruder_nr in 0..extruder_is_used.len() {
            if (checking_extruder_nr != -1 && extruder_nr as i32 != checking_extruder_nr) || !extruder_is_used[extruder_nr] {
                continue;
            }
            let extruder_settings = &scene.extruders[extruder_nr].settings;
            if !(extruder_settings.get_bool("prime_blob_enable") && mesh_group_settings.get_bool("extruder_prime_pos_abs")) {
                continue;
            }
            let mut prime_pos = Point2LL::new(extruder_settings.get_coord("extruder_prime_pos_x"), extruder_settings.get_coord("extruder_prime_pos_y"));
            if prime_pos == Point2LL::new(0, 0) {
                continue; // Ignore extruder prime position if it is not set.
            }
            let translation = Point2LL::new(extruder_settings.get_coord("machine_nozzle_offset_x"), extruder_settings.get_coord("machine_nozzle_offset_y"));
            prime_pos -= translation;
            let prime_polygon = PolygonUtils::make_disc(prime_pos, prime_clearance, 64);
            disallowed_areas = disallowed_areas.union_polygon_with(&prime_polygon, PolyFillType::NonZero);
        }

        let mut disallowed_all_extruders = Shape::default();
        let mut first = true;
        for extruder_nr in 0..extruder_is_used.len() {
            if (checking_extruder_nr != -1 && extruder_nr as i32 != checking_extruder_nr) || !extruder_is_used[extruder_nr] {
                continue;
            }
            let extruder_settings = &scene.extruders[extruder_nr].settings;
            let translation = Point2LL::new(extruder_settings.get_coord("machine_nozzle_offset_x"), extruder_settings.get_coord("machine_nozzle_offset_y"));
            let mut extruder_border = disallowed_areas.clone();
            extruder_border.translate(translation);
            if first {
                disallowed_all_extruders = extruder_border;
                first = false;
            } else {
                disallowed_all_extruders = disallowed_all_extruders.union_polygons_with(&extruder_border, PolyFillType::NonZero);
            }
        }
        disallowed_all_extruders = disallowed_all_extruders.process_even_odd(PolyFillType::NonZero); // prevent overlapping disallowed areas from XORing

        let mut border_all_extruders = border.clone(); // each extruders border areas must be limited to the global border, which is the union of all extruders borders
        if mesh_group_settings.has("nozzle_offsetting_for_disallowed_areas") && mesh_group_settings.get_bool("nozzle_offsetting_for_disallowed_areas") {
            for extruder_nr in 0..extruder_is_used.len() {
                if (checking_extruder_nr != -1 && extruder_nr as i32 != checking_extruder_nr) || !extruder_is_used[extruder_nr] {
                    continue;
                }
                let extruder_settings = &scene.extruders[extruder_nr].settings;
                let translation = Point2LL::new(extruder_settings.get_coord("machine_nozzle_offset_x"), extruder_settings.get_coord("machine_nozzle_offset_y"));
                for other_extruder_nr in 0..extruder_is_used.len() {
                    // NOTE: the other extruder doesn't have to be used. Since the global border also has to be taken into account.
                    if other_extruder_nr == extruder_nr {
                        continue;
                    }
                    let other_extruder_settings = &scene.extruders[other_extruder_nr].settings;
                    let other_translation =
                        Point2LL::new(other_extruder_settings.get_coord("machine_nozzle_offset_x"), other_extruder_settings.get_coord("machine_nozzle_offset_y"));
                    let mut translated_border = border.clone();
                    translated_border.translate(translation - other_translation);
                    border_all_extruders = border_all_extruders.intersection(&translated_border);
                }
            }
        }
        border = border_all_extruders.difference(&disallowed_all_extruders);
        border
    }

    /// Gets the border of the usable print area for this machine, without taking disallowed areas into account.
    pub fn get_raw_machine_border(&self) -> Shape {
        let mesh_group_settings = &current_slice().scene.current_mesh_group().settings;

        let mut border = Shape::default();
        let mut outline = crate::geometry::Polygon::default();
        match mesh_group_settings.get_enum::<BuildPlateShape>("machine_shape") {
            BuildPlateShape::ELLIPTIC => {
                // Construct an ellipse to approximate the build volume.
                let width = self.machine_size.max.x - self.machine_size.min.x;
                let depth = self.machine_size.max.y - self.machine_size.min.y;
                let circle_resolution: u32 = 50;
                for i in 0..circle_resolution {
                    let angle = std::f64::consts::PI * 2.0 * i as f64 / circle_resolution as f64;
                    let mid = self.machine_size.get_middle();
                    // `emplace_back(double, double)`: implicit truncation to cInt.
                    outline.points.push(Point2LL::new(
                        (mid.x as f64 + angle.cos() * width as f64 / 2.0) as i64,
                        (mid.y as f64 + angle.sin() * depth as f64 / 2.0) as i64,
                    ));
                }
            }
            _ => {
                outline = self.machine_size.flatten().to_polygon();
            }
        }
        border.push(outline);
        border
    }

    /// Initializes the prime tower if required.
    pub fn initialize_prime_tower(&mut self) {
        self.prime_tower = PrimeTower::create_prime_tower(self);
    }
}

impl Default for SliceDataStorage {
    fn default() -> Self {
        Self::new()
    }
}
