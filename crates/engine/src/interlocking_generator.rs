//! `InterlockingGenerator.h/.cpp` and `utils/VoxelUtils.h/.cpp`: generate an interlocking
//! (dovetail-like beam) structure where meshes printed with different extruders touch.
//!
//! The C++ code keeps voxel cells in `std::unordered_set<GridPoint3>`; their iteration order
//! affects the order of polygons handed to Clipper, so the libstdc++ hash table is emulated
//! (`cppstd::unordered::UnorderedSet`).

use cppstd::unordered::UnorderedSet;

use crate::application::current_slice;
use crate::geometry::matrix::PointMatrix;
use crate::geometry::point3ll::Point3LL;
use crate::geometry::{Point2LL, Polygon, Shape};
use crate::mesh::Mesh;
use crate::slicer::Slicer;
use crate::utils::coord::Coord;
use crate::utils::polygon_utils::PolygonUtils;

type GridPoint3 = Point3LL;
type CellSet = UnorderedSet<GridPoint3>;

/// `std::hash<cura::Point3LL>`
fn hash_point3(pp: &Point3LL) -> u64 {
    let prime: i32 = 31;
    let mut result: i32 = 89;
    result = ((result.wrapping_mul(prime) as i64).wrapping_add(pp.x)) as i32;
    result = ((result.wrapping_mul(prime) as i64).wrapping_add(pp.y)) as i32;
    result = ((result.wrapping_mul(prime) as i64).wrapping_add(pp.z)) as i32;
    result as i64 as u64
}

fn new_cell_set() -> CellSet {
    UnorderedSet::new(hash_point3)
}

#[inline]
fn comp(p: &Point3LL, dim: usize) -> Coord {
    match dim {
        0 => p.x,
        1 => p.y,
        _ => p.z,
    }
}

#[inline]
fn comp_mut(p: &mut Point3LL, dim: usize) -> &mut Coord {
    match dim {
        0 => &mut p.x,
        1 => &mut p.y,
        _ => &mut p.z,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(dead_code)]
enum KernelType {
    Cube,
    Diamond,
    Prism,
}

/// `DilationKernel`: the relative positions of the cells around a reference cell to dilate to.
struct DilationKernel {
    /// Size of the kernel in number of voxel cells
    kernel_size: GridPoint3,
    relative_cells: Vec<GridPoint3>,
}

impl DilationKernel {
    fn new(kernel_size: GridPoint3, kernel_type: KernelType) -> Self {
        // multiplier for division to avoid rounding and to avoid use of floating point numbers
        let mult = kernel_size.x * kernel_size.y * kernel_size.z;
        let mut relative_cells = Vec::new();
        let half_kernel = Point3LL::new(kernel_size.x / 2, kernel_size.y / 2, kernel_size.z / 2);

        let start = -half_kernel;
        let end = kernel_size - half_kernel;
        for x in start.x..end.x {
            for y in start.y..end.y {
                for z in start.z..end.z {
                    let current = Point3LL::new(x, y, z);
                    if kernel_type != KernelType::Cube {
                        let mut limit = Point3LL::new(
                            if x < 0 { start.x } else { end.x - 1 },
                            if y < 0 { start.y } else { end.y - 1 },
                            if z < 0 { start.z } else { end.z - 1 },
                        );
                        if limit.x == 0 {
                            limit.x = 1;
                        }
                        if limit.y == 0 {
                            limit.y = 1;
                        }
                        if limit.z == 0 {
                            limit.z = 1;
                        }
                        let rel_dists = Point3LL::new(mult * current.x / limit.x, mult * current.y / limit.y, mult * current.z / limit.z);
                        if (kernel_type == KernelType::Diamond && rel_dists.x + rel_dists.y + rel_dists.z > mult)
                            || (kernel_type == KernelType::Prism && rel_dists.x + rel_dists.y > mult)
                        {
                            continue; // don't consider this cell
                        }
                    }
                    relative_cells.push(Point3LL::new(x, y, z));
                }
            }
        }
        DilationKernel { kernel_size, relative_cells }
    }
}

/// `VoxelUtils`: walking over a 3D voxel grid.
struct VoxelUtils {
    cell_size: Point3LL,
}

impl VoxelUtils {
    fn to_grid_point(&self, point: &Point3LL) -> GridPoint3 {
        Point3LL::new(self.to_grid_coord(point.x, 0), self.to_grid_coord(point.y, 1), self.to_grid_coord(point.z, 2))
    }

    fn to_grid_coord(&self, coord: Coord, dim: usize) -> Coord {
        coord / comp(&self.cell_size, dim) - (coord < 0) as Coord
    }

    fn to_lower_corner(&self, location: &GridPoint3) -> Point3LL {
        Point3LL::new(self.to_lower_coord(location.x, 0), self.to_lower_coord(location.y, 1), self.to_lower_coord(location.z, 2))
    }

    fn to_lower_coord(&self, grid_coord: Coord, dim: usize) -> Coord {
        grid_coord * comp(&self.cell_size, dim)
    }

    /// A rectangular polygon equal to the cross section of a voxel cell.
    fn to_polygon(&self, p: &GridPoint3) -> Polygon {
        let c = self.to_lower_corner(p);
        Polygon::new(
            vec![
                Point2LL::new(c.x, c.y),
                Point2LL::new(c.x + self.cell_size.x, c.y),
                Point2LL::new(c.x + self.cell_size.x, c.y + self.cell_size.y),
                Point2LL::new(c.x, c.y + self.cell_size.y),
            ],
            false,
        )
    }

    /// Process voxels which a line segment crosses.
    fn walk_line(&self, start: Point3LL, end: Point3LL, process_cell_func: &mut dyn FnMut(GridPoint3) -> bool) -> bool {
        let diff = end - start;

        let start_cell = self.to_grid_point(&start);
        let end_cell = self.to_grid_point(&end);
        if start_cell == end_cell {
            return process_cell_func(start_cell);
        }

        let mut current_cell = start_cell;
        loop {
            if !process_cell_func(current_cell) {
                return false;
            }

            let mut stepping_dim: i32 = -1; // dimension in which the line next exits the current cell
            let mut percentage_along_line = f64::MAX;
            for dim in 0..3 {
                let d = comp(&diff, dim);
                if d == 0 {
                    continue;
                }
                let crossing_boundary = self.to_lower_coord(comp(&current_cell, dim), dim) + (d > 0) as Coord * comp(&self.cell_size, dim);
                let percentage_along_line_here = (crossing_boundary - comp(&start, dim)) as f64 / d as f64;
                if percentage_along_line_here < percentage_along_line {
                    percentage_along_line = percentage_along_line_here;
                    stepping_dim = dim as i32;
                }
            }
            if percentage_along_line > 1.0 {
                // next cell is beyond the end
                return true;
            }
            let sd = stepping_dim as usize;
            *comp_mut(&mut current_cell, sd) += if comp(&diff, sd) > 0 { 1 } else { -1 };
        }
    }

    fn walk_polygons(&self, polys: &Shape, z: Coord, process_cell_func: &mut dyn FnMut(GridPoint3) -> bool) -> bool {
        for poly in polys.iter() {
            let Some(&back) = poly.points.last() else {
                continue;
            };
            let mut last = back;
            for &p in poly.points.iter() {
                if !self.walk_line(Point3LL::new(last.x, last.y, z), Point3LL::new(p.x, p.y, z), process_cell_func) {
                    return false;
                }
                last = p;
            }
        }
        true
    }

    fn walk_dilated_polygons(&self, polys: &Shape, z: Coord, kernel: &DilationKernel, process_cell_func: &mut dyn FnMut(GridPoint3) -> bool) -> bool {
        let mut translated = polys.clone();
        let translation = self.dilation_translation(kernel);
        if translation.x != 0 && translation.y != 0 {
            translated.translate(Point2LL::new(translation.x, translation.y));
        }
        let mut dilated = |loc: GridPoint3| Self::dilate(kernel, loc, process_cell_func);
        self.walk_polygons(&translated, z + translation.z, &mut dilated)
    }

    /// `(Point3LL(1, 1, 1) - kernel.kernel_size_ % 2) * cell_size_ / 2`
    fn dilation_translation(&self, kernel: &DilationKernel) -> Point3LL {
        let k = &kernel.kernel_size;
        let odd = Point3LL::new(1 - k.x % 2, 1 - k.y % 2, 1 - k.z % 2);
        Point3LL::new(odd.x * self.cell_size.x / 2, odd.y * self.cell_size.y / 2, odd.z * self.cell_size.z / 2)
    }

    /// `_walkAreas`: the polygons are assumed to be translated by half the cell size in xy already.
    fn walk_areas_inner(&self, polys: &Shape, z: Coord, process_cell_func: &mut dyn FnMut(GridPoint3) -> bool) -> bool {
        let skin_points = PolygonUtils::spread_dots_area_pt(polys, Point2LL::new(self.cell_size.x, self.cell_size.y));
        for p in skin_points {
            if !process_cell_func(self.to_grid_point(&Point3LL::new(p.x + self.cell_size.x / 2, p.y + self.cell_size.y / 2, z))) {
                return false;
            }
        }
        true
    }

    fn walk_dilated_areas(&self, polys: &Shape, z: Coord, kernel: &DilationKernel, process_cell_func: &mut dyn FnMut(GridPoint3) -> bool) -> bool {
        let mut translated = polys.clone();
        // offset half a cell when using an even kernel, and half a cell so that the dots of spreadDotsArea are centered on the middle of the cell
        let t = self.dilation_translation(kernel);
        let translation = Point3LL::new(t.x - self.cell_size.x / 2, t.y - self.cell_size.y / 2, t.z - self.cell_size.z / 2);
        if translation.x != 0 && translation.y != 0 {
            translated.translate(Point2LL::new(translation.x, translation.y));
        }
        let mut dilated = |loc: GridPoint3| Self::dilate(kernel, loc, process_cell_func);
        self.walk_areas_inner(&translated, z + translation.z, &mut dilated)
    }

    fn dilate(kernel: &DilationKernel, loc: GridPoint3, process_cell_func: &mut dyn FnMut(GridPoint3) -> bool) -> bool {
        for rel in &kernel.relative_cells {
            if !process_cell_func(loc + *rel) {
                return false;
            }
        }
        true
    }
}

pub struct InterlockingGenerator<'s> {
    mesh_a: &'s mut Slicer,
    mesh_b: &'s mut Slicer,
    beam_width_a: Coord,
    beam_width_b: Coord,
    vu: VoxelUtils,
    rotation: PointMatrix,
    cell_size: Point3LL,
    beam_layer_count: Coord,
    interface_dilation: DilationKernel,
    air_dilation: DilationKernel,
    /// Whether to fully remove all of the interlocking cells which would be visible on the outside.
    air_filtering: bool,
}

/// Distance between models to be considered next to each other so that an interlocking structure will be generated there
const IGNORED_GAP: Coord = 100;

fn mesh_of(slicer: &Slicer) -> &Mesh {
    unsafe { &*slicer.mesh }
}

impl<'s> InterlockingGenerator<'s> {
    /// Generate an interlocking structure between each two adjacent meshes printed with different extruders.
    pub fn generate_interlocking_structure(volumes: &mut [Slicer]) {
        let global_settings = &current_slice().scene.current_mesh_group().settings;
        let rotation = PointMatrix::from_angle_degrees(global_settings.get_angle_degrees("interlocking_orientation"));
        let beam_layer_count = global_settings.get_int("interlocking_beam_layer_count") as Coord;
        let interface_depth = global_settings.get_int("interlocking_depth") as Coord;
        let boundary_avoidance = global_settings.get_int("interlocking_boundary_avoidance") as Coord;

        for mesh_a_idx in 0..volumes.len() {
            let extruder_nr_a = mesh_of(&volumes[mesh_a_idx]).settings.get_extruder_train("wall_0_extruder_nr").extruder_nr;
            for mesh_b_idx in mesh_a_idx + 1..volumes.len() {
                let extruder_nr_b = mesh_of(&volumes[mesh_b_idx]).settings.get_extruder_train("wall_0_extruder_nr").extruder_nr;

                {
                    let a = mesh_of(&volumes[mesh_a_idx]);
                    let b = mesh_of(&volumes[mesh_b_idx]);
                    if !a.can_interlock() || !b.can_interlock() {
                        continue;
                    }

                    let mut aabb_a = a.get_aabb();
                    if extruder_nr_a == extruder_nr_b || !aabb_a.expand(IGNORED_GAP).hit(&b.get_aabb()) {
                        // early out for when meshes don't share any overlap in their bounding box
                        continue;
                    }
                }

                let beam_width_a = mesh_of(&volumes[mesh_a_idx]).settings.get_coord("interlocking_beam_width");
                let beam_width_b = mesh_of(&volumes[mesh_b_idx]).settings.get_coord("interlocking_beam_width");

                let interface_dilation = DilationKernel::new(Point3LL::new(interface_depth, interface_depth, interface_depth), KernelType::Prism);

                let air_filtering = boundary_avoidance > 0;
                let air_dilation = DilationKernel::new(Point3LL::new(boundary_avoidance, boundary_avoidance, boundary_avoidance), KernelType::Prism);

                let cell_width = beam_width_a + beam_width_b;
                let cell_size = Point3LL::new(cell_width, cell_width, 2 * beam_layer_count);

                let (left, right) = volumes.split_at_mut(mesh_b_idx);
                let gen = InterlockingGenerator {
                    mesh_a: &mut left[mesh_a_idx],
                    mesh_b: &mut right[0],
                    beam_width_a,
                    beam_width_b,
                    vu: VoxelUtils { cell_size },
                    rotation,
                    cell_size,
                    beam_layer_count,
                    interface_dilation,
                    air_dilation,
                    air_filtering,
                };
                gen.generate();
            }
        }
    }

    fn generate(mut self) {
        let mut voxels_per_mesh = self.get_shell_voxels(&self.interface_dilation);
        let mut has_all_meshes = voxels_per_mesh.pop().unwrap();
        let mut has_any_mesh = voxels_per_mesh.pop().unwrap();
        has_any_mesh.merge(&mut has_all_meshes); // perform union and intersection simultaneously

        let layer_regions = self.compute_unioned_volume_regions();

        if self.air_filtering {
            let mut air_cells = new_cell_set();
            self.add_boundary_cells(&layer_regions, &self.air_dilation, &mut air_cells);

            for p in air_cells.iter() {
                has_all_meshes.remove(p);
            }

            self.handle_thin_areas(&has_all_meshes);
        }

        self.apply_microstructure_to_outlines(&has_all_meshes, &layer_regions);
    }

    /// Given two polygons, return the parts that border on air, and grow 'perpendicular' up to 'detect' distance.
    fn grow_border_areas_perpendicular(&self, a: &Shape, b: &Shape, detect: Coord) -> (Shape, Shape) {
        let la = mesh_of(self.mesh_a).settings.get_coord("min_wall_line_width");
        let lb = mesh_of(self.mesh_b).settings.get_coord("min_wall_line_width");
        let min_line = if lb < la { lb } else { la };

        let total_shrunk = a.offset_default(min_line).union_polygons_other(&b.offset_default(min_line)).offset_default(2 * -min_line);

        let mut from_border_a = a.difference(&total_shrunk);
        let mut from_border_b = b.difference(&total_shrunk);

        let mut i: Coord = 0;
        while i < (detect / min_line) + 2 {
            let temp_a = from_border_a.offset_default(min_line);
            let temp_b = from_border_b.offset_default(min_line);
            from_border_a = temp_a.difference(&temp_b);
            from_border_b = temp_b.difference(&temp_a);
            i += 1;
        }

        (from_border_a, from_border_b)
    }

    /// Special handling for thin strips of material.
    fn handle_thin_areas(&mut self, has_all_meshes: &CellSet) {
        let global_settings = &current_slice().scene.current_mesh_group().settings;
        let boundary_avoidance = global_settings.get_int("interlocking_boundary_avoidance") as Coord;

        let number_of_beams_detect = boundary_avoidance;
        let number_of_beams_expand = boundary_avoidance - 1;
        const ROUNDING_ERRORS: Coord = 5;

        let max_beam_width = if self.beam_width_a < self.beam_width_b { self.beam_width_b } else { self.beam_width_a };
        let detect = (max_beam_width * number_of_beams_detect) + ROUNDING_ERRORS;
        let expand = (max_beam_width * number_of_beams_expand) + ROUNDING_ERRORS;
        let lwa = mesh_of(self.mesh_a).settings.get_coord("line_width");
        let lwb = mesh_of(self.mesh_b).settings.get_coord("line_width");
        let close_gaps = (if lwb < lwa { lwb } else { lwa }) / 4;

        // Make an inclusionary polygon, to only actually handle thin areas near actual microstructures (so not in skin for example).
        let n_layers = self.mesh_a.layers.len().min(self.mesh_b.layers.len());
        let mut near_interlock_per_layer: Vec<Shape> = vec![Shape::default(); n_layers];
        for cell in has_all_meshes.iter() {
            let bottom_corner = self.vu.to_lower_corner(cell);
            let mut layer_nr = bottom_corner.z;
            while layer_nr < bottom_corner.z + self.cell_size.z && layer_nr < n_layers as Coord {
                if layer_nr >= 0 {
                    near_interlock_per_layer[layer_nr as usize].push(self.vu.to_polygon(cell));
                }
                layer_nr += 1;
            }
        }
        let unrotate = self.rotation.inverse();
        for near_interlock in near_interlock_per_layer.iter_mut() {
            *near_interlock = near_interlock.offset_default(ROUNDING_ERRORS).offset_default(-ROUNDING_ERRORS).union_polygons().offset_default(detect);
            near_interlock.apply_matrix(&unrotate);
        }

        // Only alter layers when they are present in both meshes, zip should take care if that.
        for layer_nr in 0..n_layers {
            let polys_a = self.mesh_a.layers[layer_nr].polygons.clone();
            let polys_b = self.mesh_b.layers[layer_nr].polygons.clone();

            let (from_border_a, from_border_b) = self.grow_border_areas_perpendicular(&polys_a, &polys_b, detect);

            // Get the areas of each mesh that are _not_ thin (large), by performing a morphological open.
            let large_a = polys_a.offset_default(-detect).offset_default(detect);
            let large_b = polys_b.offset_default(-detect).offset_default(detect);

            // Derive the area that the thin areas need to expand into (so the added areas to the thin strips) from the information we already have.
            let thin_expansion_a = large_b
                .intersection(&polys_a.difference(&large_a).offset_default(expand))
                .intersection(&near_interlock_per_layer[layer_nr])
                .intersection(&from_border_a)
                .offset_default(ROUNDING_ERRORS);
            let thin_expansion_b = large_a
                .intersection(&polys_b.difference(&large_b).offset_default(expand))
                .intersection(&near_interlock_per_layer[layer_nr])
                .intersection(&from_border_b)
                .offset_default(ROUNDING_ERRORS);

            // Expanded thin areas of the opposing polygon should 'eat into' the larger areas of the polygon,
            // and conversely, add the expansions to their own thin areas.
            self.mesh_a.layers[layer_nr].polygons = polys_a
                .union_polygons_other(&thin_expansion_a)
                .difference(&thin_expansion_b)
                .offset_default(close_gaps)
                .offset_default(-close_gaps);
            self.mesh_b.layers[layer_nr].polygons = polys_b
                .union_polygons_other(&thin_expansion_b)
                .difference(&thin_expansion_a)
                .offset_default(close_gaps)
                .offset_default(-close_gaps);
        }
    }

    /// Get the voxels bordering the areas of each mesh, dilated by the kernel.
    fn get_shell_voxels(&self, kernel: &DilationKernel) -> Vec<CellSet> {
        let mut voxels_per_mesh = vec![new_cell_set(), new_cell_set()];

        // mark all cells which contain some boundary
        for (mesh_idx, mesh_voxels) in voxels_per_mesh.iter_mut().enumerate() {
            let mesh: &Slicer = if mesh_idx == 0 { self.mesh_a } else { self.mesh_b };

            let rotated_polygons_per_layer: Vec<Shape> = mesh
                .layers
                .iter()
                .map(|layer| {
                    let mut s = layer.polygons.clone();
                    s.apply_matrix(&self.rotation);
                    s
                })
                .collect();

            self.add_boundary_cells(&rotated_polygons_per_layer, kernel, mesh_voxels);
        }

        voxels_per_mesh
    }

    /// Add the cells crossed by the outlines and the skin areas, dilated by the kernel.
    fn add_boundary_cells(&self, layers: &[Shape], kernel: &DilationKernel, cells: &mut CellSet) {
        let mut voxel_emplacer = |p: GridPoint3| {
            cells.insert(p);
            true
        };

        for layer_nr in 0..layers.len() {
            let z = layer_nr as Coord;
            self.vu.walk_dilated_polygons(&layers[layer_nr], z, kernel, &mut voxel_emplacer);
            let mut skin = layers[layer_nr].clone();
            if layer_nr > 0 {
                skin = skin.xor_polygons(&layers[layer_nr - 1], clipper::PolyFillType::EvenOdd);
            }
            // remove superfluous small areas, which would anyway be included because of walkPolygons
            skin = skin.offset_default(-self.cell_size.x / 2).offset_default(self.cell_size.x / 2);
            self.vu.walk_dilated_areas(&skin, z, kernel, &mut voxel_emplacer);
        }
    }

    /// The union of both meshes, per layer (plus a ghost layer on top), rotated.
    fn compute_unioned_volume_regions(&self) -> Vec<Shape> {
        // introduce ghost layer on top for correct skin computation of topmost layer.
        let max_layer_count = self.mesh_a.layers.len().max(self.mesh_b.layers.len()) + 1;
        let mut layer_regions = vec![Shape::default(); max_layer_count];

        for (layer_nr, layer_region) in layer_regions.iter_mut().enumerate() {
            for mesh in [&*self.mesh_a, &*self.mesh_b] {
                if layer_nr >= mesh.layers.len() {
                    break;
                }
                layer_region.push_all(&mesh.layers[layer_nr].polygons);
            }
            // Morphological close to merge meshes into single volume
            *layer_region = layer_region.offset_default(IGNORED_GAP).offset_default(-IGNORED_GAP);
            layer_region.apply_matrix(&self.rotation);
        }
        layer_regions
    }

    /// The areas of each mesh within a cell, for even and odd beam layers.
    fn generate_microstructure(&self) -> Vec<Vec<Shape>> {
        let mut cell_area_per_mesh_per_layer: Vec<Vec<Shape>> = vec![vec![Shape::default(), Shape::default()]];
        let beam_w_sum = self.beam_width_a + self.beam_width_b;
        let middle = self.cell_size.x * self.beam_width_a / beam_w_sum;
        let width = [middle, self.cell_size.x - middle];
        for mesh_idx in 0..2usize {
            let offset = Point2LL::new(if mesh_idx != 0 { middle } else { 0 }, 0);
            let area_size = Point2LL::new(width[mesh_idx], self.cell_size.y);

            cell_area_per_mesh_per_layer[0][mesh_idx].push(Polygon::new(
                vec![offset, offset + Point2LL::new(area_size.x, 0), offset + area_size, offset + Point2LL::new(0, area_size.y)],
                false,
            ));
        }
        let mut swapped = cell_area_per_mesh_per_layer[0].clone();
        for polys in swapped.iter_mut() {
            for poly in polys.lines.iter_mut() {
                for p in poly.points.iter_mut() {
                    std::mem::swap(&mut p.x, &mut p.y);
                }
            }
        }
        cell_area_per_mesh_per_layer.push(swapped);
        cell_area_per_mesh_per_layer
    }

    /// Change the outlines of the meshes so that they interlock.
    fn apply_microstructure_to_outlines(&mut self, cells: &CellSet, layer_regions: &[Shape]) {
        let cell_area_per_mesh_per_layer = self.generate_microstructure();

        let unapply_rotation = self.rotation.inverse();
        let max_layer_count = self.mesh_a.layers.len().max(self.mesh_b.layers.len());

        let blc = self.beam_layer_count;
        // Every `beam_layer_count` number of layers are combined to an interlocking beam layer
        let num_interlocking_layers = (max_layer_count + blc as usize - 1) / blc as usize;
        let mut structure_per_layer: [Vec<Shape>; 2] = [vec![Shape::default(); num_interlocking_layers], vec![Shape::default(); num_interlocking_layers]];

        // Only compute cell structure for half the layers, because since our beams are two layers high, every odd layer of the structure will be the same as the layer below.
        for grid_loc in cells.iter() {
            let bottom_corner = self.vu.to_lower_corner(grid_loc);
            for (mesh_idx, structure) in structure_per_layer.iter_mut().enumerate() {
                let mut layer_nr = bottom_corner.z;
                while layer_nr < bottom_corner.z + self.cell_size.z && layer_nr < max_layer_count as Coord {
                    let q = layer_nr / blc;
                    let mut areas_here = cell_area_per_mesh_per_layer[((q as u64) % cell_area_per_mesh_per_layer.len() as u64) as usize][mesh_idx].clone();
                    areas_here.translate(Point2LL::new(bottom_corner.x, bottom_corner.y));
                    if q >= 0 && (q as usize) < structure.len() {
                        structure[q as usize].push_all_owned(areas_here);
                    }
                    layer_nr += blc;
                }
            }
        }

        for structure in structure_per_layer.iter_mut() {
            for layer_structure in structure.iter_mut() {
                *layer_structure = layer_structure.union_polygons();
                layer_structure.apply_matrix(&unapply_rotation);
            }
        }

        for mesh_idx in 0..2usize {
            for layer_nr in 0..max_layer_count {
                let mesh: &mut Slicer = if mesh_idx == 0 { &mut *self.mesh_a } else { &mut *self.mesh_b };
                if layer_nr >= mesh.layers.len() {
                    break;
                }

                let mut layer_outlines = layer_regions[layer_nr].clone();
                layer_outlines.apply_matrix(&unapply_rotation);

                let areas_here = structure_per_layer[mesh_idx][layer_nr / blc as usize].intersection(&layer_outlines);
                let areas_other = &structure_per_layer[1 - mesh_idx][layer_nr / blc as usize];

                let layer = &mut mesh.layers[layer_nr];
                layer.polygons = layer
                    .polygons
                    .difference(areas_other) // reduce layer areas inward with beams from other mesh
                    .union_polygons_other(&areas_here); // extend layer areas outward with newly added beams
            }
        }
    }
}
