//! The scene: models placed on the build plate, with Cura's conventions (Y up, origin at the
//! centre of the build plate, mesh data centred on its bounding box like Uranium's
//! `MeshFileHandler.readerRead`).

use crate::mesh_io::{mat_mul, LoadedModel};
use std::sync::Arc;

pub type Mat4 = [[f64; 4]; 4];

pub fn identity() -> Mat4 {
    [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]
}

/// Mesh data in local coordinates.
#[derive(Debug)]
pub struct MeshData {
    /// Three vertices per face.
    pub vertices: Vec<[f32; 3]>,
    pub min: [f32; 3],
    #[allow(dead_code)]
    pub max: [f32; 3],
}

impl MeshData {
    pub fn new(vertices: Vec<[f32; 3]>) -> MeshData {
        let (min, max) = bounds(&vertices);
        MeshData { vertices, min, max }
    }
}

fn bounds(v: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for p in v {
        for k in 0..3 {
            min[k] = min[k].min(p[k]);
            max[k] = max[k].max(p[k]);
        }
    }
    (min, max)
}

#[derive(Clone, Debug)]
pub struct Node {
    pub id: u64,
    pub name: String,
    pub mesh: Arc<MeshData>,
    /// Position (translation) in scene coordinates.
    pub position: [f64; 3],
    /// Rotation and scale (column vectors), applied before the translation.
    pub orientation: [[f64; 3]; 3],
    pub scale: [f64; 3],
    /// Index of the extruder (position) that prints this model.
    pub extruder: usize,
    /// Per-object settings: key → serialized value (`=formula` for formulas).
    pub settings: Vec<(String, String)>,
    #[allow(dead_code)]
    pub source_file: String,
    /// Whether the model is (partially) outside the printable volume.
    pub outside: bool,
    /// The parent model of a child node (a support blocker belongs to a model).
    pub parent: Option<u64>,
    /// A child's transformation relative to its parent (`SceneNode._transformation`).
    pub local: Option<Mat4>,
    /// A child's world transformation (parent world · local), kept up to date by the session.
    pub world_cache: Option<Mat4>,
}

impl Node {
    /// The world transformation (translation · rotation · scale).
    pub fn world(&self) -> Mat4 {
        if let Some(w) = self.world_cache {
            return w;
        }
        let r = &self.orientation;
        let s = &self.scale;
        let mut m = identity();
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] = r[i][j] * s[j];
            }
            m[i][3] = self.position[i];
        }
        m
    }

    /// World-space axis-aligned bounding box (min, max), in float64 like Uranium's
    /// `getConvexHullTransformedVertices` (vertices are transformed with the float32 matrix).
    pub fn world_bounds(&self) -> ([f64; 3], [f64; 3]) {
        let m = to_f32(&self.world());
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        for p in self.mesh.vertices.iter() {
            let (x, y, z) = (p[0] as f64, p[1] as f64, p[2] as f64);
            for k in 0..3 {
                let v = x * m[k][0] as f64 + y * m[k][1] as f64 + z * m[k][2] as f64 + m[k][3] as f64;
                min[k] = min[k].min(v);
                max[k] = max[k].max(v);
            }
        }
        (min, max)
    }

    /// Move the model so that it rests on the build plate.
    pub fn drop_to_plate(&mut self) {
        let (min, _) = self.world_bounds();
        if min[1].is_finite() {
            self.position[1] -= min[1];
        }
    }

    /// Vertices as `StartSliceJob` sends them to the engine: world space (float32 arithmetic),
    /// converted to Z up (`(x, -z, y)`).
    pub fn engine_vertices(&self) -> Vec<f32> {
        let w = to_f32(&self.world());
        let identity_rotation = (0..3).all(|i| (0..3).all(|j| w[i][j] == if i == j { 1.0 } else { 0.0 }));
        let mut out = Vec::with_capacity(self.mesh.vertices.len() * 3);
        for p in self.mesh.vertices.iter() {
            let mut v = [0f32; 3];
            for k in 0..3 {
                let r = if identity_rotation {
                    p[k]
                } else {
                    // numpy float32 `verts.dot(rot_scale)` (BLAS sgemm with fused multiply-adds).
                    p[2].mul_add(w[k][2], p[1].mul_add(w[k][1], p[0] * w[k][0]))
                };
                v[k] = r + w[k][3];
            }
            out.push(v[0]);
            out.push(-v[2]);
            out.push(v[1]);
        }
        out
    }

    pub fn face_count(&self) -> usize {
        self.mesh.vertices.len() / 3
    }
}

pub fn to_f32(m: &Mat4) -> [[f32; 4]; 4] {
    let mut r = [[0f32; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            r[i][j] = m[i][j] as f32;
        }
    }
    r
}

/// Rotation matrix around a scene axis (0 = x, 1 = y, 2 = z) by `degrees`.
pub fn rotation(axis: usize, degrees: f64) -> [[f64; 3]; 3] {
    let a = degrees.to_radians();
    // Exact values for multiples of 90 degrees.
    let (s, c) = {
        let q = degrees / 90.0;
        if (q - q.round()).abs() < 1e-12 {
            match (q.round() as i64).rem_euclid(4) {
                0 => (0.0, 1.0),
                1 => (1.0, 0.0),
                2 => (0.0, -1.0),
                _ => (-1.0, 0.0),
            }
        } else {
            (a.sin(), a.cos())
        }
    };
    match axis {
        0 => [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]],
        1 => [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]],
        _ => [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]],
    }
}

pub fn mul3(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    r
}

/// Build a node from a loaded model the way Cura does: centre the mesh data on its bounding box
/// (`setCenterPosition`), then lift the node so that it rests on the plate.
pub fn node_from_model(id: u64, model: LoadedModel, extruder: usize, source_file: &str) -> Node {
    let LoadedModel { name, vertices, transform, settings } = model;
    let (min, max) = bounds(&vertices);
    // AxisAlignedBox.center = min + (max - min) / 2 in float64.
    let center: [f64; 3] = std::array::from_fn(|k| min[k] as f64 + (max[k] as f64 - min[k] as f64) / 2.0);
    // transformVertices with a float32 translation: v + f32(-c) in float32.
    let neg_c: [f32; 3] = std::array::from_fn(|k| (-center[k]) as f32);
    let centred: Vec<[f32; 3]> = vertices.iter().map(|p| [p[0] + neg_c[0], p[1] + neg_c[1], p[2] + neg_c[2]]).collect();
    let mesh = Arc::new(MeshData::new(centred));
    let mut node = Node {
        id,
        name,
        mesh,
        position: [0.0; 3],
        orientation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        scale: [1.0; 3],
        extruder,
        settings,
        source_file: source_file.to_string(),
        outside: false,
        parent: None,
        local: None,
        world_cache: None,
    };
    match transform {
        Some(t) => {
            // The file's transformation applies to the uncentred mesh: compensate for the centring.
            let tc = [[1.0, 0.0, 0.0, center[0]], [0.0, 1.0, 0.0, center[1]], [0.0, 0.0, 1.0, center[2]], [0.0, 0.0, 0.0, 1.0]];
            let m = mat_mul(&t, &tc);
            node.position = [m[0][3], m[1][3], m[2][3]];
            // Split the 3x3 part into orientation (with scale folded in) and unit scale.
            let mut o = [[0.0; 3]; 3];
            for i in 0..3 {
                for j in 0..3 {
                    o[i][j] = m[i][j];
                }
            }
            node.orientation = o;
        }
        None => {
            // `center_y = node.getWorldPosition().y - node.getBoundingBox().bottom`
            let bottom = node.mesh.min[1] as f64;
            node.position[1] = 0.0 - bottom;
        }
    }
    node
}

/// `numpy.dot` of two 4x4 matrices (sequential sums).
pub fn mat4_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut r = [[0.0; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            let mut acc = 0.0;
            for k in 0..4 {
                acc += a[i][k] * b[k][j];
            }
            r[i][j] = acc;
        }
    }
    r
}

/// Matrix inverse (Gauss-Jordan with partial pivoting); `None` for singular matrices.
pub fn mat4_inverse(m: &Mat4) -> Option<Mat4> {
    let mut a = *m;
    let mut inv = identity();
    for c in 0..4 {
        let p = (c..4).max_by(|&x, &y| a[x][c].abs().partial_cmp(&a[y][c].abs()).unwrap())?;
        if a[p][c].abs() < 1e-300 {
            return None;
        }
        a.swap(c, p);
        inv.swap(c, p);
        let d = a[c][c];
        for j in 0..4 {
            a[c][j] /= d;
            inv[c][j] /= d;
        }
        for r in 0..4 {
            if r != c && a[r][c] != 0.0 {
                let f = a[r][c];
                for j in 0..4 {
                    a[r][j] -= f * a[c][j];
                    inv[r][j] -= f * inv[c][j];
                }
            }
        }
    }
    Some(inv)
}

pub fn translation(t: [f64; 3]) -> Mat4 {
    let mut m = identity();
    for k in 0..3 {
        m[k][3] = t[k];
    }
    m
}

/// The support blocker (`SupportEraser._createCube(10)`): a 10 mm cube, as the triangles
/// StartSliceJob sends (`numpy.take(verts, indices)`).
pub fn eraser_cube(size: f32) -> Vec<[f32; 3]> {
    let s = size / 2.0;
    let verts: [[f32; 3]; 24] = [
        [-s, -s, s], [-s, s, s], [s, s, s], [s, -s, s],
        [-s, s, -s], [-s, -s, -s], [s, -s, -s], [s, s, -s],
        [s, -s, -s], [-s, -s, -s], [-s, -s, s], [s, -s, s],
        [-s, s, -s], [s, s, -s], [s, s, s], [-s, s, s],
        [-s, -s, s], [-s, -s, -s], [-s, s, -s], [-s, s, s],
        [s, -s, -s], [s, -s, s], [s, s, s], [s, s, -s],
    ];
    let mut out = Vec::with_capacity(36);
    for i in (0..24).step_by(4) {
        for t in [[i, i + 2, i + 1], [i, i + 3, i + 2]] {
            for k in t {
                out.push(verts[k]);
            }
        }
    }
    out
}

/// Place new nodes next to the existing ones on the build plate: a simple spiral search over a
/// grid for a spot where the footprint (with a margin) does not overlap other models.
pub fn arrange(nodes: &mut [Node], to_place: &[usize], plate_w: f64, plate_d: f64, margin: f64) {
    let fixed: Vec<usize> = (0..nodes.len()).filter(|i| !to_place.contains(i) && nodes[*i].parent.is_none()).collect();
    let mut occupied: Vec<([f64; 3], [f64; 3])> = fixed.iter().map(|&i| nodes[i].world_bounds()).collect();
    for &i in to_place {
        let (min, max) = nodes[i].world_bounds();
        let half = [(max[0] - min[0]) / 2.0, (max[2] - min[2]) / 2.0];
        let offset = [nodes[i].position[0] - (min[0] + max[0]) / 2.0, nodes[i].position[2] - (min[2] + max[2]) / 2.0];
        let step = 2.0f64.max(((half[0] + half[1]) / 4.0).min(10.0));
        let fits = |cx: f64, cz: f64, occ: &[([f64; 3], [f64; 3])]| -> bool {
            for (a, b) in occ {
                if cx + half[0] + margin > a[0] && cx - half[0] - margin < b[0] && cz + half[1] + margin > a[2] && cz - half[1] - margin < b[2] {
                    return false;
                }
            }
            true
        };
        let inside = |cx: f64, cz: f64| cx - half[0] >= -plate_w / 2.0 && cx + half[0] <= plate_w / 2.0 && cz - half[1] >= -plate_d / 2.0 && cz + half[1] <= plate_d / 2.0;
        let mut best: Option<(f64, f64)> = None;
        let max_r = (plate_w.max(plate_d) / step).ceil() as i64 + 2;
        'search: for r in 0..=max_r {
            let mut candidates = Vec::new();
            for dx in -r..=r {
                for dz in -r..=r {
                    if dx.abs().max(dz.abs()) != r {
                        continue;
                    }
                    candidates.push((dx as f64 * step, dz as f64 * step));
                }
            }
            candidates.sort_by(|a, b| (a.0 * a.0 + a.1 * a.1).partial_cmp(&(b.0 * b.0 + b.1 * b.1)).unwrap());
            for (cx, cz) in candidates {
                if inside(cx, cz) && fits(cx, cz, &occupied) {
                    best = Some((cx, cz));
                    break 'search;
                }
            }
        }
        let (cx, cz) = best.unwrap_or((0.0, 0.0));
        nodes[i].position[0] = cx + offset[0];
        nodes[i].position[2] = cz + offset[1];
        occupied.push(nodes[i].world_bounds());
    }
}
