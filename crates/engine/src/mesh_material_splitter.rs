//! `MeshMaterialSplitter.h/.cpp`: split meshes painted with per-extruder texture data into
//! cutting-mesh modifiers, one per extruder.
//!
//! The Boost containers the C++ code uses (`concurrent_flat_map/set`, an R-tree) decide the order
//! in which segments, polygons and faces are produced, so their layouts are emulated exactly. The
//! reference is compared single-threaded (its parallel algorithms run in order).

use std::collections::{BTreeMap, HashSet};

use cppstd::boost_foa::{FoaMap, FoaSet};
use cppstd::unordered::UnorderedSet;

use crate::application::current_slice;
use crate::geometry::lines_set::{OpenLinesSet, Shape};
use crate::geometry::point2ll::{pt, Point2LL};
use crate::geometry::point3ll::Point3LL;
use crate::geometry::points_f::{Point2F, Point3D};
use crate::geometry::polyline::{OpenPolyline, Polyline};
use crate::mesh::{Image, Mesh};
use crate::mesh_group::MeshGroup;
use crate::settings::enums::SlicingTolerance;
use crate::slicer::Slicer;
use crate::texture_data_provider::{texture_value, TextureDataMapping};
use crate::utils::aabb::AABB3D;
use crate::utils::coord::Coord;
use crate::utils::mesh_utils;
use crate::utils::simplify::Simplify;
use crate::utils::voxel_grid::{local_coordinates_hash, LocalCoordinates, SpatialLookup, VoxelGrid};

/// `MeshGeneratorData` (indexing the mesh group's meshes).
struct MeshGeneratorData {
    mesh_idx: usize,
    bounding_box: AABB3D,
    depth: Coord,
    resolution: Coord,
}

/// `ContourKey`: z | extruder << 16 (the fourth byte is always 0).
/// `boost::hash<uint8_t>` (the value itself).
fn u8_hash(k: &u8) -> u64 {
    *k as u64
}

fn contour_key(z: u16, extruder: u8) -> u32 {
    z as u32 | (extruder as u32) << 16
}

fn contour_key_hash(k: &u32) -> u64 {
    *k as u64
}

/// `makeVoxelGridFromTexture`.
fn make_voxel_grid_from_texture(
    mesh: &Mesh,
    texture: &Image,
    mapping: &TextureDataMapping,
    voxel_grid: &mut VoxelGrid,
    mesh_extruder_nr: u8,
) -> bool {
    let mut found_extruders: FoaSet<u8> = FoaSet::new(u8_hash);
    let active_extruders: HashSet<usize> = current_slice().scene.extruders.iter().map(|e| e.extruder_nr).collect();

    for face in &mesh.faces {
        let (Some(uv0), Some(uv1), Some(uv2)) = (face.uv_coordinates[0], face.uv_coordinates[1], face.uv_coordinates[2]) else {
            continue;
        };
        let face_uvs: [Point2F; 3] = [uv0, uv1, uv2];
        let triangle: [Point3LL; 3] = [
            mesh.vertices[face.vertex_index[0] as usize].p,
            mesh.vertices[face.vertex_index[1] as usize].p,
            mesh.vertices[face.vertex_index[2] as usize].p,
        ];
        for traversed_voxel in voxel_grid.get_traversed_voxels(&triangle) {
            let global_position = voxel_grid.to_global_coordinates(&traversed_voxel, true);
            let global_position_ll =
                Point3LL::new(cppstd::llrint(global_position.x), cppstd::llrint(global_position.y), cppstd::llrint(global_position.z));
            let barycentric = match mesh_utils::get_barycentric_coordinates(&global_position_ll, &triangle) {
                Some(b) if !(b.x < 0.0 || b.y < 0.0 || b.z < 0.0) => b,
                _ => continue,
            };
            let point_uv_coords = mesh_utils::get_uv_coordinates(&barycentric, &face_uvs);
            let (px, py) = texture.get_pixel_coordinates(&Point2F::new(point_uv_coords.x, point_uv_coords.y));
            let mut extruder_nr = texture_value(texture, mapping, px, py, "extruder");
            if extruder_nr.is_none() || !active_extruders.contains(&(extruder_nr.unwrap() as usize)) {
                extruder_nr = Some(mesh_extruder_nr as u32);
            }
            let e = extruder_nr.unwrap() as u8;
            voxel_grid.set_or_update_occupation(traversed_voxel, e);
            found_extruders.insert(e);
        }
    }

    if found_extruders.len() == 1 {
        let mut is_specific_extruder = true;
        for &e in found_extruders.iter() {
            is_specific_extruder = e != mesh_extruder_nr;
        }
        return is_specific_extruder;
    }
    true
}

/// `makeMeshesFromVoxelsGrid`.
fn make_meshes_from_voxels_grid(voxel_grid: &VoxelGrid, mesh_extruder_nr: u8) -> Vec<Mesh> {
    let mut marching_squares: FoaSet<LocalCoordinates> = FoaSet::new(local_coordinates_hash);
    for (p, &occupation) in voxel_grid.occupied_voxels() {
        if occupation != mesh_extruder_nr {
            marching_squares.insert(*p);
            if p.x > 0 {
                marching_squares.insert(LocalCoordinates::new(p.x - 1, p.y, p.z));
            }
            if p.y > 0 {
                marching_squares.insert(LocalCoordinates::new(p.x, p.y - 1, p.z));
            }
            if p.x > 0 && p.y > 0 {
                marching_squares.insert(LocalCoordinates::new(p.x - 1, p.y - 1, p.z));
            }
        }
    }

    let half_res_x = voxel_grid.get_resolution().x / 2.0;
    let half_res_y = voxel_grid.get_resolution().y / 2.0;
    let p = |x: f64, y: f64| -> Point2LL { pt(x as Coord, y as Coord) };
    let seg = |a: Point2LL, b: Point2LL| -> OpenPolyline { OpenPolyline::new(vec![a, b]) };
    let (hx, hy) = (half_res_x, half_res_y);
    let marching_segments: [Vec<OpenPolyline>; 16] = [
        vec![],
        vec![seg(p(0.0, hy), p(hx, 0.0))],
        vec![seg(p(-hx, 0.0), p(0.0, hy))],
        vec![seg(p(-hx, 0.0), p(hx, 0.0))],
        vec![seg(p(hx, 0.0), p(0.0, -hy))],
        vec![seg(p(0.0, hy), p(0.0, -hy))],
        vec![seg(p(-hx, 0.0), p(0.0, hy)), seg(p(hx, 0.0), p(0.0, -hy))],
        vec![seg(p(-hx, 0.0), p(0.0, -hy))],
        vec![seg(p(0.0, -hy), p(-hx, 0.0))],
        vec![seg(p(0.0, -hy), p(-hx, 0.0)), seg(p(0.0, hy), p(hx, 0.0))],
        vec![seg(p(0.0, -hy), p(0.0, hy))],
        vec![seg(p(0.0, -hy), p(hx, 0.0))],
        vec![seg(p(hx, 0.0), p(-hx, 0.0))],
        vec![seg(p(0.0, hy), p(-hx, 0.0))],
        vec![seg(p(hx, 0.0), p(0.0, hy))],
        vec![],
    ];

    struct Contour {
        segments: OpenLinesSet,
        polygons: Shape,
    }

    let position_delta_center = Point3D::new(half_res_x, half_res_y, 0.0);
    let mut raw_contours: FoaMap<u32, Contour> = FoaMap::new(contour_key_hash);
    for square_start in marching_squares.iter() {
        let x_plus1 = square_start.x as i32 + 1;
        let x_plus1_valid = x_plus1 <= u16::MAX as i32;
        let y_plus1 = square_start.y as i32 + 1;
        let y_plus1_valid = y_plus1 <= u16::MAX as i32;

        let mut filled_extruders: UnorderedSet<u8> = UnorderedSet::new_std();
        let mut occupation_bits = [0u8; 4];
        let mut add_occupied_extruder = |x: i32, y: i32, position_valid: bool, idx: usize| {
            if position_valid {
                if let Some(occupation) = voxel_grid.get_occupation(&LocalCoordinates::new(x as u16, y as u16, square_start.z)) {
                    filled_extruders.insert(occupation);
                    occupation_bits[idx] = occupation;
                    return;
                }
            }
            occupation_bits[idx] = mesh_extruder_nr;
        };
        add_occupied_extruder(x_plus1, y_plus1, x_plus1_valid && y_plus1_valid, 0);
        add_occupied_extruder(square_start.x as i32, y_plus1, y_plus1_valid, 1);
        add_occupied_extruder(x_plus1, square_start.y as i32, x_plus1_valid, 2);
        add_occupied_extruder(square_start.x as i32, square_start.y as i32, true, 3);

        if filled_extruders.len() < 2 {
            continue;
        }
        for &extruder in filled_extruders.iter() {
            if extruder == mesh_extruder_nr {
                continue;
            }
            let segments_index = (occupation_bits[0] == extruder) as usize
                + (((occupation_bits[1] == extruder) as usize) << 1)
                + (((occupation_bits[2] == extruder) as usize) << 2)
                + (((occupation_bits[3] == extruder) as usize) << 3);
            if marching_segments[segments_index].is_empty() {
                continue;
            }
            let center_position = voxel_grid.to_global_coordinates(square_start, true) + position_delta_center;
            let center_position_ll = pt(center_position.x as Coord, center_position.y as Coord);
            let mut translated_segments = OpenLinesSet::default();
            for line in &marching_segments[segments_index] {
                let mut l = line.clone();
                for point in l.points.iter_mut() {
                    *point += center_position_ll;
                }
                translated_segments.push(l);
            }
            let key = contour_key(square_start.z, extruder);
            let ts = translated_segments.clone();
            raw_contours.emplace_or_visit(key, Contour { segments: translated_segments, polygons: Shape::default() }, |c| c.segments.push_all(&ts));
        }
    }

    // Stitch the separate segments into closed polygons.
    for (_, contour) in raw_contours.iter_mut() {
        let mut result_lines = OpenLinesSet::default();
        crate::utils::polyline_stitcher::stitch_open_into_shape(
            &contour.segments,
            &mut result_lines,
            &mut contour.polygons,
            crate::utils::coord::mm2int(0.1),
            10,
        );
    }

    let r = voxel_grid.get_resolution();
    let min_distance = r.x.min(r.y).min(r.z) / 2.0;
    let simplifier = Simplify::new(min_distance as Coord, (min_distance / 2.0) as Coord, Coord::MAX);

    let mut meshes: BTreeMap<u8, Mesh> = BTreeMap::new();
    for (key, contour) in raw_contours.iter() {
        let z = (*key & 0xFFFF) as u16;
        let extruder = ((*key >> 16) & 0xFF) as u8;
        let z_low = voxel_grid.to_global_z(z, false) as Coord;
        let z_high = voxel_grid.to_global_z(z.wrapping_add(1), false) as Coord;
        const OFFSET_OVERLAPPING: Coord = 5;
        let simplified_polygons = simplifier.shape(&contour.polygons).offset_default(OFFSET_OVERLAPPING);
        for simplified_polygon in simplified_polygons.iter() {
            let mesh = meshes.entry(extruder).or_insert_with(|| {
                let mut mesh = Mesh::with_settings(&current_slice().scene.extruders[extruder as usize].settings);
                mesh.settings.add("cutting_mesh", "true");
                mesh.settings.add("extruder_nr", &extruder.to_string());
                mesh
            });
            for (start, end) in simplified_polygon.segments() {
                mesh.add_face(Point3LL::from_2d(start, z_low), Point3LL::from_2d(end, z_low), Point3LL::from_2d(end, z_high), None, None, None);
                mesh.add_face(Point3LL::from_2d(end, z_high), Point3LL::from_2d(start, z_high), Point3LL::from_2d(start, z_low), None, None, None);
            }
        }
    }
    meshes.into_values().collect()
}

/// `sliceMesh`: a slice of the mesh per Z plane of the voxel grid, unioned over ±2 planes.
fn slice_mesh(mesh: &Mesh, rasterized_mesh: &VoxelGrid) -> Vec<Shape> {
    let thickness = rasterized_mesh.get_resolution().z as Coord;
    let initial_layer_thickness = thickness;
    let mesh_bounding_box = mesh.get_aabb();
    let margin_below_mesh = rasterized_mesh.to_local_z(mesh_bounding_box.min.z.max(0) as f64) as usize;
    let slice_layer_count = (rasterized_mesh.to_local_z(mesh_bounding_box.max.z as f64) as usize).wrapping_sub(margin_below_mesh).wrapping_add(1);

    let res = rasterized_mesh.get_resolution();
    // INT2MM of a double: no truncation to coord_t.
    let xy_offset = (if res.x < res.y { res.y } else { res.x } * 2.0) / 1000.0;
    let xy_offset_str = format!("{:.6}", xy_offset);
    let mut sliced_mesh = mesh.clone();
    sliced_mesh.settings.add("xy_offset", &xy_offset_str);
    sliced_mesh.settings.add("xy_offset_layer_0", &xy_offset_str);
    sliced_mesh.settings.add("hole_xy_offset", "0");
    sliced_mesh.settings.add("hole_xy_offset_max_diameter", "0");

    let slicer = Slicer::new(&mut sliced_mesh, thickness, slice_layer_count, false, None, SlicingTolerance::INCLUSIVE, initial_layer_thickness);

    let mut slices = Vec::new();
    for layer_index in 0..rasterized_mesh.get_slices_count().z {
        let mut expanded_shape = Shape::default();
        for delta in -2i64..=2 {
            let union_layer_index = layer_index + delta - margin_below_mesh as i64;
            if union_layer_index >= 0 && (union_layer_index as usize) < slicer.layers.len() {
                expanded_shape = expanded_shape.union_polygons_other(&slicer.layers[union_layer_index as usize].polygons);
            }
        }
        slices.push(expanded_shape);
    }
    slices
}

/// `isInside`.
fn is_inside(voxel_grid: &VoxelGrid, position: &LocalCoordinates, sliced_mesh: &[Shape]) -> bool {
    let global_position = voxel_grid.to_global_coordinates(position, true);
    sliced_mesh[position.z as usize].inside(pt(global_position.x as Coord, global_position.y as Coord), true)
}

/// `findVoxelsToEvaluate`.
fn find_voxels_to_evaluate(voxel_grid: &VoxelGrid, previously_evaluated_voxels: &FoaSet<LocalCoordinates>) -> FoaSet<LocalCoordinates> {
    let mut voxels_to_evaluate = FoaSet::new(local_coordinates_hash);
    let mut voxels_considered = FoaSet::new(local_coordinates_hash);
    for previously_evaluated_voxel in previously_evaluated_voxels.iter() {
        for voxel_around in voxel_grid.get_voxels_around(previously_evaluated_voxel) {
            if voxels_considered.insert(voxel_around) && !voxel_grid.has_occupation(&voxel_around) {
                voxels_to_evaluate.insert(voxel_around);
            }
        }
    }
    voxels_to_evaluate
}

/// `evaluateVoxels`.
fn evaluate_voxels(
    voxel_grid: &mut VoxelGrid,
    voxels_to_evaluate: &FoaSet<LocalCoordinates>,
    texture_data: &SpatialLookup,
    sliced_mesh: &[Shape],
    depth_squared: Coord,
    mesh_extruder_nr: u8,
) {
    for voxel_to_evaluate in voxels_to_evaluate.iter() {
        let position = voxel_grid.to_global_coordinates(voxel_to_evaluate, true);
        if !is_inside(voxel_grid, voxel_to_evaluate, sliced_mesh) {
            voxel_grid.set_occupation(*voxel_to_evaluate, mesh_extruder_nr);
        } else {
            match texture_data.find_closest_occupation(&position) {
                Some(nearest) => {
                    let diff = position - nearest.position;
                    let new_occupation = if diff.v_size2() <= depth_squared as f64 { nearest.occupation } else { mesh_extruder_nr };
                    voxel_grid.set_occupation(*voxel_to_evaluate, new_occupation);
                }
                None => voxel_grid.set_occupation(*voxel_to_evaluate, mesh_extruder_nr),
            }
        }
    }
}

/// `findBoundaryVoxels`.
fn find_boundary_voxels(evaluated_voxels: &mut FoaSet<LocalCoordinates>, voxel_grid: &VoxelGrid) {
    evaluated_voxels.erase_if(|evaluated_voxel| {
        let mut has_various_voxels_around = false;
        let actual_occupation = voxel_grid.get_occupation(evaluated_voxel).unwrap();
        for voxel_around in voxel_grid.get_voxels_around(evaluated_voxel) {
            if let Some(around_occupation) = voxel_grid.get_occupation(&voxel_around) {
                if around_occupation != actual_occupation {
                    has_various_voxels_around = true;
                    break;
                }
            }
        }
        !has_various_voxels_around
    });
}

/// `propagateVoxels`.
fn propagate_voxels(
    voxel_grid: &mut VoxelGrid,
    evaluated_voxels: &mut FoaSet<LocalCoordinates>,
    sliced_mesh: &[Shape],
    texture_data: &SpatialLookup,
    depth_squared: Coord,
    mesh_extruder_nr: u8,
) {
    let mut iteration = 0usize;
    while !evaluated_voxels.is_empty() {
        crate::log_debug!("Finding voxels around {} voxels for iteration {}", evaluated_voxels.len(), iteration);
        *evaluated_voxels = find_voxels_to_evaluate(voxel_grid, evaluated_voxels);
        crate::log_debug!("Evaluating {} voxels", evaluated_voxels.len());
        evaluate_voxels(voxel_grid, evaluated_voxels, texture_data, sliced_mesh, depth_squared, mesh_extruder_nr);
        find_boundary_voxels(evaluated_voxels, voxel_grid);
        iteration += 1;
    }
}

/// `makeModifierMeshes`.
fn make_modifier_meshes(mesh: &Mesh, mesh_data: &MeshGeneratorData, texture: &Image, mapping: &TextureDataMapping) -> Vec<Mesh> {
    let mesh_extruder_nr = mesh.settings.get_usize("extruder_nr") as u8;
    let mut bounding_box = mesh_data.bounding_box;
    bounding_box.expand(mesh_data.resolution * 8);

    let mut voxel_grid = VoxelGrid::new(&bounding_box, mesh_data.resolution);
    if !make_voxel_grid_from_texture(mesh, texture, mapping, &mut voxel_grid, mesh_extruder_nr) {
        return Vec::new();
    }

    let texture_data = SpatialLookup::from_voxel_grid(&voxel_grid);
    let depth_squared = mesh_data.depth * mesh_data.depth;
    let sliced_mesh = slice_mesh(mesh, &voxel_grid);

    let mut previously_evaluated_voxels: FoaSet<LocalCoordinates> = FoaSet::new(local_coordinates_hash);
    for (p, &o) in voxel_grid.occupied_voxels() {
        if o != mesh_extruder_nr {
            previously_evaluated_voxels.insert(*p);
        }
    }

    propagate_voxels(&mut voxel_grid, &mut previously_evaluated_voxels, &sliced_mesh, &texture_data, depth_squared, mesh_extruder_nr);

    make_meshes_from_voxels_grid(&voxel_grid, mesh_extruder_nr)
}

/// `makeMaterialModifierMeshes`.
pub fn make_material_modifier_meshes(meshgroup: &mut MeshGroup) {
    let mut mesh_generation_data = Vec::new();
    for (mesh_idx, mesh) in meshgroup.meshes.iter().enumerate() {
        let has_extruder_painting = match (&mesh.texture, &mesh.texture_data_mapping) {
            (Some(_), Some(mapping)) => mapping.contains_key("extruder"),
            _ => false,
        };
        if !has_extruder_painting {
            continue;
        }
        let mut bounding_box = AABB3D::default();
        for vertex in &mesh.vertices {
            bounding_box.include(vertex.p);
        }
        mesh_generation_data.push(MeshGeneratorData {
            mesh_idx,
            bounding_box,
            depth: mesh.settings.get_coord("multi_material_paint_depth"),
            resolution: mesh.settings.get_coord("multi_material_paint_resolution"),
        });
    }

    let mut modifier_meshes = Vec::new();
    for mesh_data in &mesh_generation_data {
        let mesh = &meshgroup.meshes[mesh_data.mesh_idx];
        crate::log_info!("Start multi-material mesh generation for {}", mesh.mesh_name);
        let timer = std::time::Instant::now();
        let texture = mesh.texture.clone().unwrap();
        let mapping = mesh.texture_data_mapping.clone().unwrap();
        modifier_meshes.extend(make_modifier_meshes(mesh, mesh_data, &texture, &mapping));
        crate::log_info!("Multi-material mesh generation for {} took {} seconds", mesh.mesh_name, timer.elapsed().as_secs_f64());
    }

    meshgroup.meshes.extend(modifier_meshes);
}
