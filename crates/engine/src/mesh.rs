//! `mesh.h`: a triangle mesh with vertex melding and face connectivity.

use crate::geometry::point3ll::Point3LL;
use crate::geometry::points_f::{Point2F, Point3D};
use crate::geometry::matrix::Matrix4x3D;
use crate::settings::settings::Settings;
use crate::utils::aabb::AABB3D;
use crate::utils::coord::Coord;
use std::cell::Cell;
use std::collections::HashMap;
use std::f64::consts::PI;

/// `vertex_meld_distance = MM2INT(0.03)`.
pub const VERTEX_MELD_DISTANCE: Coord = 30;

#[inline]
fn point_hash(p: &Point3LL) -> u32 {
    let d = VERTEX_MELD_DISTANCE;
    (((p.x + d / 2) / d) ^ (((p.y + d / 2) / d) << 10) ^ (((p.z + d / 2) / d) << 20)) as u32
}

#[derive(Clone, Debug)]
pub struct MeshVertex {
    pub p: Point3LL,
    /// Faces that use this vertex.
    pub connected_faces: Vec<u32>,
}

impl MeshVertex {
    pub fn new(p: Point3LL) -> Self {
        MeshVertex { p, connected_faces: Vec::with_capacity(8) }
    }
}

#[derive(Clone, Debug)]
pub struct MeshFace {
    pub vertex_index: [i32; 3],
    /// Index of the face connected across each edge (-1 if none).
    pub connected_face_index: [i32; 3],
    pub uv_coordinates: [Option<Point2F>; 3],
}

impl Default for MeshFace {
    fn default() -> Self {
        // `int vertex_index_[3] = { -1 };`
        MeshFace { vertex_index: [-1, 0, 0], connected_face_index: [0; 3], uv_coordinates: [None; 3] }
    }
}

/// An RGB(A) image used as texture (`Image` in mesh.h).
#[derive(Clone, Debug, Default)]
pub struct Image {
    pub data: Vec<u8>,
    pub width: usize,
    pub height: usize,
    pub bytes_per_pixel: usize,
    pub bytes_per_row: usize,
}

impl Image {
    pub fn new(width: usize, height: usize, bytes_per_pixel: usize, data: Vec<u8>) -> Self {
        Image { data, width, height, bytes_per_pixel, bytes_per_row: bytes_per_pixel * width }
    }

    pub fn get_pixel(&self, x: usize, y: usize) -> u32 {
        let mut result = 0u32;
        let index = y * self.bytes_per_row + x * self.bytes_per_pixel;
        for i in 0..self.bytes_per_pixel {
            result |= (self.data[index + i] as u32) << ((self.bytes_per_pixel - i - 1) * 8);
        }
        result
    }

    pub fn get_pixel_coordinates(&self, uv: &Point2F) -> (usize, usize) {
        // static_cast<size_t>(float) as compiled for x86-64: negative values below -1 wrap to huge
        // values (and are then clamped to the last pixel).
        fn float_to_size_t(f: f32) -> usize {
            const TWO63: f32 = 9.223372e18;
            if f.is_nan() {
                1usize << 63
            } else if f >= TWO63 {
                (((f - TWO63) as i64) as u64 ^ (1u64 << 63)) as usize
            } else {
                (f as i64) as u64 as usize
            }
        }
        let x = float_to_size_t(uv.x * self.width as f32);
        let y = float_to_size_t(uv.y * self.height as f32);
        (x.clamp(0, self.width - 1), y.clamp(0, self.height - 1))
    }

    pub fn get_pixel_uv(&self, uv: &Point2F) -> u32 {
        let (x, y) = self.get_pixel_coordinates(uv);
        self.get_pixel(x, y)
    }
}

#[derive(Debug, Clone)]
pub struct Mesh {
    vertex_hash_map: HashMap<u32, Vec<u32>>,
    aabb: AABB3D,
    pub vertices: Vec<MeshVertex>,
    pub faces: Vec<MeshFace>,
    pub settings: Settings,
    pub mesh_name: String,
    pub texture: Option<std::sync::Arc<Image>>,
    pub texture_data_mapping: Option<std::sync::Arc<crate::texture_data_provider::TextureDataMapping>>,
    has_disconnected_faces: Cell<bool>,
    has_overlapping_faces: Cell<bool>,
}

unsafe impl Sync for Mesh {}

impl Default for Mesh {
    fn default() -> Self {
        Mesh {
            vertex_hash_map: HashMap::new(),
            aabb: AABB3D::default(),
            vertices: Vec::new(),
            faces: Vec::new(),
            settings: Settings::new(),
            mesh_name: String::new(),
            texture: None,
            texture_data_mapping: None,
            has_disconnected_faces: Cell::new(false),
            has_overlapping_faces: Cell::new(false),
        }
    }
}

impl Mesh {
    /// `Mesh(Settings& parent)`: note that this *copies* the given settings (including its parent).
    pub fn with_settings(parent: &Settings) -> Self {
        Mesh { settings: parent.clone(), ..Default::default() }
    }

    pub fn add_face(&mut self, v0: Point3LL, v1: Point3LL, v2: Point3LL, uv0: Option<Point2F>, uv1: Option<Point2F>, uv2: Option<Point2F>) {
        let vi0 = self.find_index_of_vertex(v0);
        let vi1 = self.find_index_of_vertex(v1);
        let vi2 = self.find_index_of_vertex(v2);
        if vi0 == vi1 || vi1 == vi2 || vi0 == vi2 {
            return; // the face has two vertices which get assigned the same location. Don't add the face.
        }

        let idx = self.faces.len() as u32; // index of face to be added
        let mut face = MeshFace::default();
        face.vertex_index = [vi0, vi1, vi2];
        face.uv_coordinates = [uv0, uv1, uv2];
        self.faces.push(face);
        self.vertices[vi0 as usize].connected_faces.push(idx);
        self.vertices[vi1 as usize].connected_faces.push(idx);
        self.vertices[vi2 as usize].connected_faces.push(idx);
    }

    pub fn clear(&mut self) {
        self.faces.clear();
        self.vertices.clear();
        self.vertex_hash_map.clear();
    }

    pub fn finish(&mut self) {
        // Finish up the mesh, clear the vertex_hash_map, as it's no longer needed from this point on and uses quite a bit of memory.
        self.vertex_hash_map.clear();

        // For each face, store which other face is connected with it.
        for i in 0..self.faces.len() {
            let vi = self.faces[i].vertex_index;
            let c0 = self.get_face_idx_with_points(vi[0], vi[1], i as i32, vi[2]); // connected face index on the edge v0-v1
            let c1 = self.get_face_idx_with_points(vi[1], vi[2], i as i32, vi[0]); // connected face index on the edge v1-v2
            let c2 = self.get_face_idx_with_points(vi[2], vi[0], i as i32, vi[1]); // connected face index on the edge v2-v0
            self.faces[i].connected_face_index = [c0, c1, c2];
        }
    }

    pub fn min(&self) -> Point3LL {
        self.aabb.min
    }

    pub fn max(&self) -> Point3LL {
        self.aabb.max
    }

    pub fn get_aabb(&self) -> AABB3D {
        self.aabb
    }

    pub fn expand_xy(&mut self, offset: i64) {
        if offset != 0 {
            self.aabb.expand_xy(offset);
        }
    }

    pub fn translate(&mut self, offset: Point3LL) {
        if offset == Point3LL::new(0, 0, 0) {
            return;
        }
        for v in self.vertices.iter_mut() {
            v.p += offset;
        }
        self.aabb.translate(offset);
    }

    pub fn transform(&mut self, transformation: &Matrix4x3D) {
        for v in self.vertices.iter_mut() {
            v.p = transformation.apply_3ll(&v.p);
        }
        self.aabb.min = transformation.apply_3ll(&self.aabb.min);
        self.aabb.max = transformation.apply_3ll(&self.aabb.max);
    }

    /// Whether this mesh produces any printed output (not an infill/cutting/anti-overhang mesh).
    pub fn is_printed(&self) -> bool {
        !self.settings.get_bool("infill_mesh") && !self.settings.get_bool("cutting_mesh") && !self.settings.get_bool("anti_overhang_mesh")
    }

    pub fn can_interlock(&self) -> bool {
        !self.settings.get_bool("infill_mesh") && !self.settings.get_bool("anti_overhang_mesh")
    }

    fn find_index_of_vertex(&mut self, v: Point3LL) -> i32 {
        let hash = point_hash(&v);
        let bucket = self.vertex_hash_map.entry(hash).or_default();
        for &idx in bucket.iter() {
            if (self.vertices[idx as usize].p - v).test_length(VERTEX_MELD_DISTANCE) {
                return idx as i32;
            }
        }
        bucket.push(self.vertices.len() as u32);
        self.vertices.push(MeshVertex::new(v));
        self.aabb.include(v);
        self.vertices.len() as i32 - 1
    }

    /// Returns the index of the 'other' face connected to the edge between vertices with indices
    /// idx0 and idx1. In case more than two faces are connected via the same edge, the next face
    /// in a counter-clockwise ordering (looking from idx1 to idx0) is returned.
    fn get_face_idx_with_points(&self, idx0: i32, idx1: i32, not_face_idx: i32, not_face_vertex_idx: i32) -> i32 {
        let mut candidate_faces: Vec<i32> = Vec::new(); // in case more than two faces meet at an edge, multiple candidates are generated
        for &f in &self.vertices[idx0 as usize].connected_faces {
            let f = f as i32;
            if f == not_face_idx {
                continue;
            }
            let face = &self.faces[f as usize];
            if face.vertex_index[0] == idx1 // && faces[f].vertex_index[1] == idx0 // next face should have the right direction!
                || face.vertex_index[1] == idx1 // && faces[f].vertex_index[2] == idx0
                || face.vertex_index[2] == idx1
            // && faces[f].vertex_index[0] == idx0
            {
                candidate_faces.push(f);
            }
        }

        if candidate_faces.is_empty() {
            if !self.has_disconnected_faces.get() {
                // Mesh has disconnected faces. Only log the warning once per mesh.
                crate::log_warn!("Mesh has disconnected faces!");
            }
            self.has_disconnected_faces.set(true);
            return -1;
        }
        if candidate_faces.len() == 1 {
            return candidate_faces[0];
        }

        if candidate_faces.len() % 2 == 0 {
            if !self.has_disconnected_faces.get() {
                crate::log_warn!("Mesh has disconnected faces!");
            }
            self.has_disconnected_faces.set(true);
        }

        // Find the face with the smallest angle (counter clockwise), seen from the edge vector
        let vertices = &self.vertices;
        let vn = Point3D::from_3ll(&(vertices[idx1 as usize].p - vertices[idx0 as usize].p));
        let n = vn / vn.v_size(); // the normal of the plane in which all normals of faces connected to the edge lie => the normalized edge vector
        let v0 = Point3D::from_3ll(&(vertices[idx1 as usize].p - vertices[idx0 as usize].p)); // the edge vector
        let n0 = Point3D::from_3ll(&(vertices[not_face_vertex_idx as usize].p - vertices[idx0 as usize].p)).cross(&v0);

        let mut smallest_angle = 1000.0f64; // more then 2 PI (impossible angle)
        let mut best_idx = -1;

        for &candidate_face in &candidate_faces {
            let mut candidate_vertex = 0usize;
            {
                // find third vertex belonging to the face (besides idx0 and idx1)
                while candidate_vertex < 3 {
                    let vi = self.faces[candidate_face as usize].vertex_index[candidate_vertex];
                    if vi != idx0 && vi != idx1 {
                        break;
                    }
                    candidate_vertex += 1;
                }
            }

            let v1 = Point3D::from_3ll(&(vertices[self.faces[candidate_face as usize].vertex_index[candidate_vertex] as usize].p - vertices[idx0 as usize].p));
            let n1 = v0.cross(&v1);

            let dot = n0.dot(&n1);
            let det = n.dot(&n0.cross(&n1));
            let mut angle = det.atan2(dot);
            if angle < 0.0 {
                angle += 2.0 * PI; // 0 <= angle < 2* M_PI
            }

            if angle == 0.0 {
                if !self.has_overlapping_faces.get() {
                    // Mesh has overlapping faces. Only log the warning once per mesh.
                    crate::log_warn!("Mesh has overlapping faces!");
                }
                self.has_overlapping_faces.set(true);
            }
            if angle < smallest_angle {
                smallest_angle = angle;
                best_idx = candidate_face;
            }
        }
        if best_idx < 0 {
            if !self.has_disconnected_faces.get() {
                crate::log_warn!("Mesh has disconnected faces!");
            }
            self.has_disconnected_faces.set(true);
        }
        best_idx
    }
}
