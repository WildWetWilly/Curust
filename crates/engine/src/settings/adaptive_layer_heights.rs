//! `AdaptiveLayerHeights.h/.cpp`: variable layer heights based on the slope of the model surface.

use crate::application::current_slice;
use crate::geometry::points_f::Point3D;
use crate::mesh_group::MeshGroup;
use crate::settings::enums::SlicingTolerance;
use crate::settings::types::AngleRadians;
use crate::utils::coord::{mm2int, Coord};

#[derive(Clone, Copy, Debug)]
pub struct AdaptiveLayer {
    /// Height of the layer in microns.
    pub layer_height: Coord,
    /// The absolute z position of the layer.
    pub z_position: Coord,
    /// Temperature to use for this layer.
    pub temperature: i32,
}

impl AdaptiveLayer {
    pub fn new(layer_height: Coord) -> Self {
        AdaptiveLayer { layer_height, z_position: 0, temperature: 0 }
    }
}

pub struct AdaptiveLayerHeights {
    layers: Vec<AdaptiveLayer>,
    allowed_layer_heights: Vec<Coord>,
    base_layer_height: Coord,
    max_variation: Coord,
    step_size: Coord,
    threshold: Coord,
    face_slopes: Vec<f64>,
    face_min_z_values: Vec<i32>,
    face_max_z_values: Vec<i32>,
}

impl AdaptiveLayerHeights {
    pub fn new(base_layer_height: Coord, variation: Coord, step_size: Coord, threshold: Coord, meshgroup: &MeshGroup) -> Self {
        let mut a = AdaptiveLayerHeights {
            layers: Vec::new(),
            allowed_layer_heights: Vec::new(),
            base_layer_height,
            max_variation: variation,
            step_size,
            threshold,
            face_slopes: Vec::new(),
            face_min_z_values: Vec::new(),
            face_max_z_values: Vec::new(),
        };
        a.calculate_allowed_layer_heights();
        a.calculate_mesh_triangle_slopes();
        a.calculate_layers(meshgroup);
        a
    }

    pub fn get_layer_count(&self) -> usize {
        self.layers.len()
    }

    pub fn get_layers(&self) -> &Vec<AdaptiveLayer> {
        &self.layers
    }

    fn calculate_allowed_layer_heights(&mut self) {
        // calculate the allowed layer heights from variation and step size
        // note: the order is from thickest to thinnest height!
        let mut allowed_layer_height = self.base_layer_height + self.max_variation;
        while allowed_layer_height >= self.base_layer_height - self.max_variation {
            // we should only consider using layer_heights that are > 0
            if allowed_layer_height <= 0 {
                break;
            }
            self.allowed_layer_heights.push(allowed_layer_height);
            allowed_layer_height -= self.step_size;
        }
    }

    fn calculate_layers(&mut self, meshgroup: &MeshGroup) {
        let mut minimum_layer_height = self.allowed_layer_heights[0];
        for &h in &self.allowed_layer_heights[1..] {
            if h < minimum_layer_height {
                minimum_layer_height = h;
            }
        }
        let mesh_group_settings = &current_slice().scene.current_mesh_group().settings;
        let slicing_tolerance = mesh_group_settings.get_enum::<SlicingTolerance>("slicing_tolerance");
        let mut triangles_of_interest: Vec<usize> = Vec::new();
        let model_max_z = meshgroup.max().z;
        let mut z_level: Coord = 0;
        let mut previous_layer_height: Coord;

        // the first layer has it's own independent height set, so we always add that
        let initial_layer_height = mesh_group_settings.get_coord("layer_height_0");
        z_level += initial_layer_height;

        let mut adaptive_layer = AdaptiveLayer::new(initial_layer_height);
        adaptive_layer.z_position = z_level;
        previous_layer_height = adaptive_layer.layer_height;
        self.layers.push(adaptive_layer);

        // loop while triangles are found
        while z_level <= model_max_z || self.layers.len() < 2 {
            let mut global_min_slope = f64::MAX;
            // loop over all allowed layer heights starting with the largest
            let mut has_added_layer = false;
            for li in 0..self.allowed_layer_heights.len() {
                let layer_height = self.allowed_layer_heights[li];
                // use lower and upper bounds to filter on triangles that are interesting for this potential layer
                let lower_bound = z_level;
                // if slicing tolerance "middle" is used, a layer is defined by the middle of the triangles
                let upper_bound = z_level + if slicing_tolerance == SlicingTolerance::MIDDLE { layer_height / 2 } else { layer_height };

                if layer_height == self.allowed_layer_heights[0] {
                    // this is the max layer thickness, search through all of the triangles in the mesh to find those
                    // that intersect with a layer this thick
                    triangles_of_interest.clear();

                    for i in 0..self.face_min_z_values.len() {
                        if self.face_min_z_values[i] as Coord <= upper_bound && self.face_max_z_values[i] as Coord >= lower_bound {
                            triangles_of_interest.push(i);
                        }
                    }
                } else {
                    // this is a reduced thickness layer, just search those triangles that intersected with the layer
                    // in the previous iteration
                    let last_triangles_of_interest = triangles_of_interest.clone();

                    triangles_of_interest.clear();

                    for &i in &last_triangles_of_interest {
                        if self.face_min_z_values[i] as Coord <= upper_bound {
                            triangles_of_interest.push(i);
                        }
                    }
                }

                // when there not interesting triangles in this potential layer go to the next one
                if triangles_of_interest.is_empty() {
                    break;
                }

                // find the minimum slope of all the interesting triangles
                let mut minimum_slope = f64::MAX;
                for &triangle_index in &triangles_of_interest {
                    let slope = self.face_slopes[triangle_index];
                    if minimum_slope > slope {
                        minimum_slope = slope;
                    }
                }
                if global_min_slope > minimum_slope {
                    global_min_slope = minimum_slope;
                }

                // check if the maximum step size has been exceeded depending on layer height direction
                let mut has_exceeded_step_size = false;
                if previous_layer_height > layer_height && previous_layer_height - layer_height > self.step_size {
                    has_exceeded_step_size = true;
                } else if layer_height - previous_layer_height > self.step_size && layer_height > minimum_layer_height {
                    continue;
                }

                // we add the layer in the following cases:
                // 1) the layer angle is below the threshold and the layer height difference with the previous layer is the maximum allowed step size
                // 2) the layer height is the smallest it is allowed
                // 3) the layer is a flat surface (we can't divide by 0)
                let minimum_slope_tan = minimum_slope.tan();
                if minimum_slope_tan == 0.0
                    || (layer_height as f64 / minimum_slope_tan) <= self.threshold as f64
                    || layer_height == minimum_layer_height
                    || has_exceeded_step_size
                {
                    z_level += layer_height;
                    let mut adaptive_layer_add = AdaptiveLayer::new(layer_height);
                    adaptive_layer_add.z_position = z_level;
                    previous_layer_height = adaptive_layer_add.layer_height;
                    self.layers.push(adaptive_layer_add);
                    has_added_layer = true;
                    break;
                }
            }
            let _ = global_min_slope;

            // this means we cannot find a layer height that has an angle lower than the threshold.
            // in this case, we use the layer height with the lowest
            if !has_added_layer {
                let min_layer_height = *self.allowed_layer_heights.last().unwrap();
                let mut minimum_adaptive_layer = AdaptiveLayer::new(min_layer_height);
                z_level += min_layer_height;
                minimum_adaptive_layer.z_position = z_level;
                previous_layer_height = min_layer_height;
                self.layers.push(minimum_adaptive_layer);
            }
        }
    }

    fn calculate_mesh_triangle_slopes(&mut self) {
        // loop over all mesh faces (triangles) and find their slopes
        for mesh in &current_slice().scene.current_mesh_group().meshes {
            // Skip meshes that are not printable
            if mesh.settings.get_bool("infill_mesh") || mesh.settings.get_bool("cutting_mesh") || mesh.settings.get_bool("anti_overhang_mesh") {
                continue;
            }

            for face in &mesh.faces {
                let v0 = &mesh.vertices[face.vertex_index[0] as usize];
                let v1 = &mesh.vertices[face.vertex_index[1] as usize];
                let v2 = &mesh.vertices[face.vertex_index[2] as usize];

                let p0 = Point3D::from_3ll(&v0.p);
                let p1 = Point3D::from_3ll(&v1.p);
                let p2 = Point3D::from_3ll(&v2.p);

                let mut min_z = p0.z;
                min_z = min_z.min(p1.z);
                min_z = min_z.min(p2.z);
                let mut max_z = p0.z;
                max_z = max_z.max(p1.z);
                max_z = max_z.max(p2.z);

                // calculate the angle of this triangle in the z direction
                let n = (p1 - p0).cross(&(p2 - p0));
                let normal = n.normalized();
                let mut z_angle = AngleRadians::new(normal.z.abs().acos());

                // prevent flat surfaces from influencing the algorithm
                if z_angle.value() == 0.0 {
                    z_angle = AngleRadians::new(std::f64::consts::PI);
                }

                self.face_min_z_values.push(mm2int(min_z) as i32);
                self.face_max_z_values.push(mm2int(max_z) as i32);
                self.face_slopes.push(z_angle.value());
            }
        }
    }
}
