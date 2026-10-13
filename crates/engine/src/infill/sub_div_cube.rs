//! `infill/SubDivCube.h/.cpp`: the cubic subdivision infill octree.
//!
//! The C++ class keeps its per-recursion properties and rotation matrices in static members. They
//! are never reset, so they persist between meshes (and `cube_properties_per_recursion_step_` keeps
//! growing). This port keeps the same process-wide state.

use std::sync::{Arc, Mutex};

use crate::geometry::matrix::{Point3Matrix, PointMatrix};
use crate::geometry::point2ll::v_size2;
use crate::geometry::point3ll::Point3LL;
use crate::geometry::{OpenLinesSet, Point2LL, Shape};
use crate::settings::types::{AngleDegrees, LayerIndex};
use crate::slice_data_storage::SliceMeshStorage;
use crate::utils::coord::Coord;
use crate::utils::polygon_utils::{PolygonUtils, NO_PENALTY};

const ONE_OVER_SQRT_2: f64 = 0.7071067811865475244008443621048490392848359376884740; // 1 / sqrt(2)
const ONE_OVER_SQRT_3: f64 = 0.577350269189625764509148780501957455647601751270126876018; // 1 / sqrt(3)
const ONE_OVER_SQRT_6: f64 = 0.408248290463863016366214012450981898660991246776111688072; // 1 / sqrt(6)
const SQRT_TWO_THIRD: f64 = 0.816496580927726032732428024901963797321982493552223376144; // sqrt(2 / 3)
const SQRT_THREE_FOURTHS: f64 = 0.86602540378443864676372317;

#[derive(Clone, Copy, Debug, Default)]
struct CubeProperties {
    /// side length of cubes
    side_length: Coord,
    /// height of cubes based. This is the distance from one point of a cube to its 3d opposite.
    height: Coord,
    /// square cut across lengths. This is the diagonal distance across a face of the cube.
    square_height: Coord,
    /// maximum draw z differences. This is the maximum difference in z at which lines need to be drawn.
    max_draw_z_diff: Coord,
    /// maximum line offsets. This is the maximum distance at which subdivision lines should be drawn from the 2d cube center.
    max_line_offset: Coord,
}

/// The static members of the C++ class.
struct Statics {
    cube_properties_per_recursion_step: Vec<CubeProperties>,
    radius_addition: Coord,
    rotation_matrix: Point3Matrix,
    infill_rotation_matrix: PointMatrix,
}

static STATICS: Mutex<Statics> = Mutex::new(Statics {
    cube_properties_per_recursion_step: Vec::new(),
    radius_addition: 0,
    rotation_matrix: Point3Matrix { matrix: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] },
    infill_rotation_matrix: PointMatrix { matrix: [1.0, 0.0, 0.0, 1.0] },
});

fn statics() -> std::sync::MutexGuard<'static, Statics> {
    STATICS.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct SubDivCube {
    /// the recursion depth of the cube (0 is most recursed)
    depth: usize,
    /// center location of the cube in absolute coordinates
    center: Point3LL,
    /// this cube's eight octree children
    children: [Option<Box<SubDivCube>>; 8],
}

impl SubDivCube {
    /// Precompute the octree of subdivided cubes.
    pub fn precompute_octree(mesh: &mut SliceMeshStorage, infill_origin: Point2LL) {
        let settings = mesh.settings();
        let curr_recursion_depth;
        {
            let mut st = statics();
            st.radius_addition = settings.get_coord("sub_div_rad_add");

            // if infill_angles is not empty use the first value, otherwise use 0
            let infill_angles = settings.get_vec_angle_degrees("infill_angles");
            let infill_angle = if !infill_angles.is_empty() { infill_angles[0] } else { AngleDegrees::new(0.0) };

            let machine_height = settings.get_coord("machine_height");
            let half_depth = settings.get_coord("machine_depth") / 2;
            let half_width = settings.get_coord("machine_width") / 2;
            let furthest_dist_from_origin = ((machine_height * machine_height + half_depth * half_depth + half_width * half_width) as f64).sqrt() as Coord;
            let max_side_length = furthest_dist_from_origin * 2;

            let mut depth = 0usize;
            let infill_line_distance = settings.get_coord("infill_line_distance");
            if infill_line_distance > 0 {
                let mut curr_side_length = infill_line_distance * 2;
                while curr_side_length < max_side_length * 2 {
                    st.cube_properties_per_recursion_step.push(CubeProperties {
                        side_length: curr_side_length,
                        height: (3f64.sqrt() * curr_side_length as f64) as Coord,
                        square_height: (2f64.sqrt() * curr_side_length as f64) as Coord,
                        max_draw_z_diff: (ONE_OVER_SQRT_3 * curr_side_length as f64) as Coord,
                        max_line_offset: (ONE_OVER_SQRT_6 * curr_side_length as f64) as Coord,
                    });
                    depth += 1;
                    curr_side_length *= 2;
                }
            }
            curr_recursion_depth = depth;

            // rotation matrix to get from axis aligned cubes to cubes standing on their tip
            // The Z axis is transformed to go in positive Y direction
            let tilt = Point3Matrix {
                matrix: [
                    -ONE_OVER_SQRT_2,
                    ONE_OVER_SQRT_2,
                    0.0,
                    -ONE_OVER_SQRT_6,
                    -ONE_OVER_SQRT_6,
                    SQRT_TWO_THIRD,
                    ONE_OVER_SQRT_3,
                    ONE_OVER_SQRT_3,
                    ONE_OVER_SQRT_3,
                ],
            };

            st.infill_rotation_matrix = PointMatrix::from_angle_degrees(infill_angle);
            let infill_angle_mat = Point3Matrix::from_point_matrix(&st.infill_rotation_matrix);
            st.rotation_matrix = infill_angle_mat.compose(&tilt);
        }
        let center = Point3LL::new(infill_origin.x, infill_origin.y, 0);

        let cube = SubDivCube::new(mesh, center, curr_recursion_depth.wrapping_sub(1));
        mesh.base_subdiv_cube = Some(Arc::new(cube));
    }

    /// Recursively creates the octree.
    fn new(mesh: &SliceMeshStorage, center: Point3LL, depth: usize) -> SubDivCube {
        let mut cube = SubDivCube { depth, center, children: Default::default() };
        if depth == 0 {
            // lowest layer, no need for subdivision, exit.
            return cube;
        }
        let (cube_properties, radius_addition, rotation_matrix) = {
            let st = statics();
            if depth >= st.cube_properties_per_recursion_step.len() {
                // Depth is out of bounds of what we pre-computed.
                return cube;
            }
            (st.cube_properties_per_recursion_step[depth], st.radius_addition, st.rotation_matrix)
        };

        let radius = (cube_properties.height as f64 / 4.0 + radius_addition as f64) as Coord;

        let mut child_nr = 0;
        let rel_child_centers = [
            Point3LL::new(1, 1, 1),    // top
            Point3LL::new(-1, 1, 1),   // top three
            Point3LL::new(1, -1, 1),
            Point3LL::new(1, 1, -1),
            Point3LL::new(-1, -1, -1), // bottom
            Point3LL::new(1, -1, -1),  // bottom three
            Point3LL::new(-1, 1, -1),
            Point3LL::new(-1, -1, 1),
        ];
        let quarter = (cube_properties.side_length / 4) as i32;
        for rel_child_center in rel_child_centers {
            // Point3LL * int32_t: the multiplication is done in 32 bits.
            let scaled = Point3LL::new(
                (rel_child_center.x as i32).wrapping_mul(quarter) as Coord,
                (rel_child_center.y as i32).wrapping_mul(quarter) as Coord,
                (rel_child_center.z as i32).wrapping_mul(quarter) as Coord,
            );
            let child_center = center + rotation_matrix.apply_3d(&scaled);
            if Self::is_valid_subdivision(mesh, &child_center, radius) {
                cube.children[child_nr] = Some(Box::new(SubDivCube::new(mesh, child_center, depth - 1)));
                child_nr += 1;
            }
        }
        cube
    }

    /// Generates the lines of subdivision of the specific cube at the specific layer.
    pub fn generate_subdivision_lines(&self, z: Coord, result: &mut OpenLinesSet) {
        let st = statics();
        if st.cube_properties_per_recursion_step.is_empty() {
            // Infill is set to 0%.
            return;
        }
        let mut directional_line_groups: [OpenLinesSet; 3] = Default::default();

        self.generate_subdivision_lines_rec(z, &mut directional_line_groups, &st);

        for line_group in &directional_line_groups {
            for line in line_group.iter() {
                result.add_segment(line[0], line[1]);
            }
        }
    }

    fn generate_subdivision_lines_rec(&self, z: Coord, directional_line_groups: &mut [OpenLinesSet; 3], st: &Statics) {
        let cube_properties = st.cube_properties_per_recursion_step[self.depth];

        let z_diff = (z - self.center.z).abs(); // the difference between the cube center and the target layer.
        if z_diff > cube_properties.height / 2 {
            // this cube does not touch the target layer. Early exit.
            return;
        }
        if z_diff < cube_properties.max_draw_z_diff {
            // this cube has lines that need to be drawn.
            let mut relative_a = Point2LL::new(0, 0); // relative coordinates of line endpoints around cube center
            let mut relative_b = Point2LL::new(0, 0);
            relative_a.x = (cube_properties.square_height / 2) * (cube_properties.max_draw_z_diff - z_diff) / cube_properties.max_draw_z_diff;
            relative_b.x = -relative_a.x;
            relative_a.y = (cube_properties.max_line_offset as f64 - ((z - (self.center.z - cube_properties.max_draw_z_diff)) as f64 * ONE_OVER_SQRT_2)) as Coord;
            relative_b.y = relative_a.y;
            relative_a = st.infill_rotation_matrix.apply(relative_a);
            relative_b = st.infill_rotation_matrix.apply(relative_b);
            for (dir_idx, group) in directional_line_groups.iter_mut().enumerate() {
                // draw the line, then rotate 120 degrees.
                let a = Point2LL::new(self.center.x + relative_a.x, self.center.y + relative_a.y);
                let b = Point2LL::new(self.center.x + relative_b.x, self.center.y + relative_b.y);
                Self::add_line_and_combine(group, a, b);
                if dir_idx < 2 {
                    Self::rotate_point_120(&mut relative_a);
                    Self::rotate_point_120(&mut relative_b);
                }
            }
        }
        for child in self.children.iter().flatten() {
            // draws the eight children
            child.generate_subdivision_lines_rec(z, directional_line_groups, st);
        }
    }

    /// Determines if a described theoretical cube should be subdivided based on if a sphere that encloses the cube touches the infill mesh.
    fn is_valid_subdivision(mesh: &SliceMeshStorage, center: &Point3LL, radius: Coord) -> bool {
        let mut distance2: Coord = 0;
        let mut inside_somewhere = false;
        let mut outside_somewhere = false;
        let layer_height = mesh.settings().get_coord("layer_height");
        let bottom_layer = ((center.z - radius) / layer_height) as i32;
        let top_layer = ((center.z + radius) / layer_height) as i32;
        let mut test_layer = bottom_layer;
        while test_layer <= top_layer {
            // steps of three. Low-hanging speed gain.
            let part_dist = (test_layer as Coord * layer_height - center.z) as f64 / radius as f64;
            let sphere_slice_radius2 = ((radius * radius) as f64 * (1.0 - (part_dist * part_dist))) as Coord;
            let loc = Point2LL::new(center.x, center.y);

            let inside = Self::distance_from_point_to_mesh(mesh, test_layer as LayerIndex, loc, &mut distance2);
            if inside == 1 {
                inside_somewhere = true;
            } else {
                outside_somewhere = true;
            }
            if outside_somewhere && inside_somewhere {
                return true;
            }
            if inside != 2 && distance2 < sphere_slice_radius2 {
                return true;
            }
            test_layer += 3;
        }
        false
    }

    /// Finds the distance to the infill border at the specified layer from the specified point.
    /// Returns 0: outside, 1: inside, 2: boundary does not exist at specified layer.
    fn distance_from_point_to_mesh(mesh: &SliceMeshStorage, layer_nr: LayerIndex, location: Point2LL, distance2: &mut Coord) -> Coord {
        if layer_nr < 0 || (layer_nr as u32 as usize) >= mesh.layers.len() {
            // this layer is outside of valid range
            return 2;
        }
        let mut collide = Shape::default();
        for part in &mesh.layers[layer_nr as usize].parts {
            collide.push_all(&part.infill_area);
        }

        let mut centerpoint = location;
        let inside = collide.inside(centerpoint, false);
        let border_point = PolygonUtils::move_inside2(&collide, &mut centerpoint, 0, i64::MAX, None, None, NO_PENALTY);
        let diff = border_point.location - location;
        *distance2 = v_size2(diff);
        if inside {
            return 1;
        }
        0
    }

    /// Rotates a point 120 degrees about the origin.
    fn rotate_point_120(target: &mut Point2LL) {
        let x = (-0.5 * target.x as f64 - SQRT_THREE_FOURTHS * target.y as f64) as Coord;
        target.y = (-0.5 * target.y as f64 + SQRT_THREE_FOURTHS * target.x as f64) as Coord;
        target.x = x;
    }

    /// Adds the defined line to the specified polygons, combining line segments with touching ends.
    fn add_line_and_combine(group: &mut OpenLinesSet, mut from: Point2LL, mut to: Point2LL) {
        let epsilon: Coord = 10; // the smallest distance of two points which are viewed as coincident (dist > 0 due to rounding errors)
        let mut idx: usize = 0;
        while idx < group.len() {
            if (from.x - group[idx][1].x).abs() < epsilon && (from.y - group[idx][1].y).abs() < epsilon {
                from = group[idx][0];
                group.remove_at(idx);
                idx = idx.wrapping_sub(1).wrapping_add(1);
                continue;
            }
            if (to.x - group[idx][0].x).abs() < epsilon && (to.y - group[idx][0].y).abs() < epsilon {
                to = group[idx][1];
                group.remove_at(idx);
                idx = idx.wrapping_sub(1).wrapping_add(1);
                continue;
            }
            idx += 1;
        }
        group.add_segment(from, to);
    }
}
