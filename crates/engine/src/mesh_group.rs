//! `MeshGroup.h`: a group of meshes printed together, plus model file loading (STL, OBJ).

use crate::geometry::matrix::Matrix4x3D;
use crate::geometry::point3ll::Point3LL;
use crate::geometry::points_f::{Point2F, Point3D, Point3F};
use crate::mesh::Mesh;
use crate::settings::settings::Settings;
use std::fs;
use std::io::Read;

#[derive(Debug, Default)]
pub struct MeshGroup {
    pub meshes: Vec<Mesh>,
    pub settings: Settings,
}

impl MeshGroup {
    /// Minimum of all printed meshes.
    pub fn min(&self) -> Point3LL {
        if self.meshes.is_empty() {
            return Point3LL::new(0, 0, 0);
        }
        let mut ret = Point3LL::new(i64::MAX, i64::MAX, i64::MAX);
        for mesh in &self.meshes {
            if mesh.settings.get_bool("infill_mesh") || mesh.settings.get_bool("cutting_mesh") || mesh.settings.get_bool("anti_overhang_mesh") {
                // Don't count pieces that are not printed.
                continue;
            }
            let v = mesh.min();
            ret.x = ret.x.min(v.x);
            ret.y = ret.y.min(v.y);
            ret.z = ret.z.min(v.z);
        }
        ret
    }

    /// Maximum of all printed meshes.
    pub fn max(&self) -> Point3LL {
        if self.meshes.is_empty() {
            return Point3LL::new(0, 0, 0);
        }
        let mut ret = Point3LL::new(i32::MIN as i64, i32::MIN as i64, i32::MIN as i64);
        for mesh in &self.meshes {
            if mesh.settings.get_bool("infill_mesh") || mesh.settings.get_bool("cutting_mesh") || mesh.settings.get_bool("anti_overhang_mesh") {
                // Don't count pieces that are not printed.
                continue;
            }
            let v = mesh.max();
            ret.x = ret.x.max(v.x);
            ret.y = ret.y.max(v.y);
            ret.z = ret.z.max(v.z);
        }
        ret
    }

    pub fn clear(&mut self) {
        for m in self.meshes.iter_mut() {
            m.clear();
        }
    }

    /// Place the meshes on the build plate and apply the material shrinkage compensation.
    pub fn finalize(&mut self) {
        // If the machine settings have been supplied, offset the given position vertices to the center of vertices (0,0,0) is at the bed center.
        let mut meshgroup_offset = Point3LL::new(0, 0, 0);
        if !self.settings.get_bool("machine_center_is_zero") {
            meshgroup_offset.x = self.settings.get_coord("machine_width") / 2;
            meshgroup_offset.y = self.settings.get_coord("machine_depth") / 2;
        }

        // If a mesh position was given, put the mesh at this position in 3D space.
        for mesh in self.meshes.iter_mut() {
            let mut mesh_offset = Point3LL::new(
                mesh.settings.get_coord("mesh_position_x"),
                mesh.settings.get_coord("mesh_position_y"),
                mesh.settings.get_coord("mesh_position_z"),
            );
            if mesh.settings.get_bool("center_object") {
                let object_min = mesh.min();
                let object_max = mesh.max();
                let object_size = object_max - object_min;
                mesh_offset += Point3LL::new(-object_min.x - object_size.x / 2, -object_min.y - object_size.y / 2, -object_min.z);
            }
            mesh.translate(mesh_offset + meshgroup_offset);
        }
        self.scale_from_bottom(self.settings.get_ratio("material_shrinkage_percentage_xy"), self.settings.get_ratio("material_shrinkage_percentage_z"));
    }

    /// Scale around the bottom-center of the mesh group (shrinkage compensation).
    pub fn scale_from_bottom(&mut self, factor_xy: f64, factor_z: f64) {
        let center = (self.max() + self.min()) / 2;
        let origin = Point3LL::new(center.x, center.y, 0);

        let transformation = Matrix4x3D::scale_xyz(factor_xy, factor_xy, factor_z, origin);
        for mesh in self.meshes.iter_mut() {
            mesh.transform(&transformation);
        }
    }
}

/// Read a line terminated by '\n' or '\r' (`fgets_` in MeshGroup.cpp).
fn split_lines(data: &[u8]) -> impl Iterator<Item = &[u8]> {
    data.split(|&c| c == b'\n' || c == b'\r')
}

/// Parse a float like `%f` in scanf: skips leading whitespace, returns value and rest.
fn scan_float(s: &str) -> Option<(f32, &str)> {
    let t = s.trim_start();
    let (v, consumed) = cppstd::strtod_prefix(t);
    if consumed == 0 {
        return None;
    }
    // %f converts with strtof (single precision rounding of the decimal value).
    let text = &t[..consumed];
    let fv = text.trim().parse::<f32>().unwrap_or(v as f32);
    Some((fv, &t[consumed..]))
}

fn load_mesh_stl_ascii(mesh: &mut Mesh, data: &[u8], matrix: &Matrix4x3D) -> bool {
    let mut n = 0;
    let mut v0 = Point3LL::new(0, 0, 0);
    let mut v1 = Point3LL::new(0, 0, 0);
    for line in split_lines(data) {
        let line = String::from_utf8_lossy(line);
        // sscanf(buffer, " vertex %f %f %f", ...)
        let t = line.trim_start();
        let Some(rest) = t.strip_prefix("vertex") else {
            continue;
        };
        let Some((x, rest)) = scan_float(rest) else { continue };
        let Some((y, rest)) = scan_float(rest) else { continue };
        let Some((z, _)) = scan_float(rest) else { continue };
        let vertex = Point3F::new(x, y, z);
        n += 1;
        match n {
            1 => v0 = matrix.apply_3d(&vertex.to_point3d()),
            2 => v1 = matrix.apply_3d(&vertex.to_point3d()),
            3 => {
                let v2 = matrix.apply_3d(&vertex.to_point3d());
                mesh.add_face(v0, v1, v2, None, None, None);
                n = 0;
            }
            _ => {}
        }
    }
    mesh.finish();
    true
}

fn load_mesh_stl_binary(mesh: &mut Mesh, data: &[u8], matrix: &Matrix4x3D, uv_coordinates: Option<&[Point2F]>) -> bool {
    let file_size = data.len() as i64;
    // HACK: Get the file size from the file itself rather than the header.
    let face_count = ((file_size - 80 - 4) / 50).max(0) as usize;
    if data.len() < 84 {
        return false;
    }
    let reported_face_count = u32::from_le_bytes([data[80], data[81], data[82], data[83]]);
    if reported_face_count as usize != face_count {
        crate::log_warn!(
            "Face count reported by file ({}) is not equal to actual face count ({}). File could be corrupt!",
            reported_face_count,
            face_count
        );
    }

    mesh.faces.reserve(face_count);
    mesh.vertices.reserve(face_count);
    let mut vertex_index = 0usize;
    for i in 0..face_count {
        let off = 84 + i * 50;
        if off + 50 > data.len() {
            return false;
        }
        let f = |k: usize| -> f32 {
            let o = off + 12 + k * 4;
            f32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]])
        };
        // Skip the normal (first 3 floats).
        let v0 = matrix.apply_3d(&Point3F::new(f(0), f(1), f(2)).to_point3d());
        let v1 = matrix.apply_3d(&Point3F::new(f(3), f(4), f(5)).to_point3d());
        let v2 = matrix.apply_3d(&Point3F::new(f(6), f(7), f(8)).to_point3d());

        match uv_coordinates {
            Some(uv) if vertex_index + 2 < uv.len() => {
                let uv0 = Some(uv[vertex_index]);
                let uv1 = Some(uv[vertex_index + 1]);
                let uv2 = Some(uv[vertex_index + 2]);
                vertex_index += 3;
                mesh.add_face(v0, v1, v2, uv0, uv1, uv2);
            }
            _ => mesh.add_face(v0, v1, v2, None, None, None),
        }
    }
    mesh.finish();
    true
}

fn is_ascii_stl(data: &[u8]) -> bool {
    let mut i = 0;
    while i < data.len() && (data[i] as char).is_ascii_whitespace() {
        i += 1;
    }
    if i + 5 > data.len() {
        return false;
    }
    data[i..i + 5].eq_ignore_ascii_case(b"solid")
}

pub fn load_mesh_stl(mesh: &mut Mesh, filename: &str, matrix: &Matrix4x3D) -> bool {
    let Ok(data) = fs::read(filename) else {
        return false;
    };
    mesh.mesh_name = filename.to_string();
    if data.len() < 6 {
        return false;
    }
    if is_ascii_stl(&data) {
        let load_success = load_mesh_stl_ascii(mesh, &data, matrix);
        if !load_success {
            return false;
        }
        // This logic is used to handle the case where the file starts with "solid" but is a binary file.
        if mesh.faces.is_empty() {
            mesh.clear();
            return load_mesh_stl_binary(mesh, &data, matrix, None);
        }
        return true;
    }
    load_mesh_stl_binary(mesh, &data, matrix, None)
}

fn load_uv_coordinates_from_file(uv_filename: &str) -> Option<Vec<Point2F>> {
    let mut f = fs::File::open(uv_filename).ok()?;
    let mut data = Vec::new();
    f.read_to_end(&mut data).ok()?;
    if data.len() < 4 {
        crate::log_warn!("Failed to read vertex count from UV file: {}", uv_filename);
        return None;
    }
    let vertex_count = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
    if data.len() < 4 + vertex_count * 8 {
        crate::log_warn!("Failed to read UV coordinates from file: {}", uv_filename);
        return None;
    }
    let mut uv = Vec::with_capacity(vertex_count);
    for i in 0..vertex_count {
        let o = 4 + i * 8;
        let u = f32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
        let v = f32::from_le_bytes([data[o + 4], data[o + 5], data[o + 6], data[o + 7]]);
        uv.push(Point2F::new(u, v));
    }
    crate::log_info!("Loaded {} UV coordinates from: {}", vertex_count, uv_filename);
    Some(uv)
}

/// `loadMeshOBJ`.
pub fn load_mesh_obj(mesh: &mut Mesh, filename: &str, matrix: &Matrix4x3D) -> bool {
    let Ok(text) = fs::read_to_string(filename) else {
        crate::log_error!("Could not open OBJ file: {}", filename);
        return false;
    };
    let mut vertices: Vec<Point3LL> = Vec::new();
    let mut uv_coordinates: Vec<Point2F> = Vec::new();

    let parse_floats = |payload: &str, n: usize| -> Option<Vec<f32>> {
        let parts: Vec<&str> = payload.split_whitespace().collect();
        if parts.len() != n {
            return None;
        }
        parts.iter().map(|p| p.parse::<f32>().ok()).collect()
    };

    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        let (ident, payload) = match line.split_once(|c: char| c.is_whitespace()) {
            Some((a, b)) => (a, b.trim_start()),
            None => continue,
        };
        match ident {
            "v" => {
                if let Some(v) = parse_floats(payload, 3) {
                    vertices.push(matrix.apply_3d(&Point3D::new(v[0] as f64, v[1] as f64, v[2] as f64)));
                }
            }
            "vt" => {
                if let Some(v) = parse_floats(payload, 2) {
                    uv_coordinates.push(Point2F::new(v[0], v[1]));
                }
            }
            "f" => {
                let mut idx: Vec<(usize, Option<usize>)> = Vec::new();
                for tok in payload.split_whitespace() {
                    let mut parts = tok.split('/');
                    let Some(Ok(vi)) = parts.next().map(|s| s.parse::<usize>()) else { continue };
                    let uvi = parts.next().and_then(|s| if s.is_empty() { None } else { s.parse::<usize>().ok() });
                    idx.push((vi.wrapping_sub(1), uvi.map(|u| u.wrapping_sub(1))));
                }
                let get_uv = |i: Option<usize>| i.and_then(|i| uv_coordinates.get(i).copied());
                if idx.len() >= 3 {
                    for i in 1..idx.len() - 1 {
                        let (a, b, c) = (idx[0], idx[i], idx[i + 1]);
                        if a.0 < vertices.len() && b.0 < vertices.len() && c.0 < vertices.len() {
                            mesh.add_face(vertices[a.0], vertices[b.0], vertices[c.0], get_uv(a.1), get_uv(b.1), get_uv(c.1));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    mesh.finish();
    !mesh.faces.is_empty()
}

/// `loadMeshIntoMeshGroup`: load a model file (STL or OBJ) into the mesh group. The mesh's
/// settings are a copy of `object_parent_settings`.
pub fn load_mesh_into_mesh_group(meshgroup: &mut MeshGroup, filename: &str, transformation: &Matrix4x3D, object_parent_settings: &Settings) -> bool {
    let lower = filename.to_ascii_lowercase();
    let ext = filename.rfind('.').map(|i| &filename[i..]);
    if ext == Some(".stl") || ext == Some(".STL") {
        let mut mesh = Mesh::with_settings(object_parent_settings);
        let base_filename = match filename.rfind('.') {
            Some(i) => &filename[..i],
            None => filename,
        };
        let uv_filename = format!("{}.uv", base_filename);
        let load_success = match load_uv_coordinates_from_file(&uv_filename) {
            Some(uv) => {
                crate::log_info!("Loading STL with UV coordinates from: {}", uv_filename);
                match fs::read(filename) {
                    Ok(data) if !is_ascii_stl(&data) => {
                        mesh.mesh_name = filename.to_string();
                        load_mesh_stl_binary(&mut mesh, &data, transformation, Some(&uv))
                    }
                    Ok(_) => {
                        crate::log_warn!("ASCII STL with UV coordinates not supported, use binary STL: {}", filename);
                        false
                    }
                    Err(_) => false,
                }
            }
            None => load_mesh_stl(&mut mesh, filename, transformation),
        };
        if load_success {
            let texture_filename = format!("{}.png", base_filename);
            if std::path::Path::new(&texture_filename).exists() {
                crate::log_info!("Found texture file: {}", texture_filename);
                if crate::utils::mesh_utils::load_texture_from_file(&mut mesh, &texture_filename) {
                    crate::log_info!("Successfully loaded texture from: {}", texture_filename);
                } else {
                    crate::log_warn!("Failed to load texture from: {}", texture_filename);
                }
            }
            meshgroup.meshes.push(mesh);
            return true;
        }
        crate::log_warn!("loading STL '{}' failed", filename);
        return false;
    }
    if ext == Some(".obj") || ext == Some(".OBJ") {
        let mut mesh = Mesh::with_settings(object_parent_settings);
        if load_mesh_obj(&mut mesh, filename, transformation) {
            meshgroup.meshes.push(mesh);
            return true;
        }
        crate::log_warn!("loading OBJ '{}' failed", filename);
        return false;
    }
    let _ = lower;
    crate::log_warn!("Unable to recognize the extension of the file. Currently only .stl and .STL are supported.");
    false
}
