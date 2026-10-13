//! `utils/MeshUtils.h/.cpp`: barycentric / UV helpers and loading of painted textures (PNG with
//! a JSON paint-feature manifest in its `Description` text chunk).

use std::sync::Arc;

use crate::geometry::point3ll::Point3LL;
use crate::geometry::points_f::{Point2F, Point3D};
use crate::mesh::{Image, Mesh};
use crate::texture_data_provider::{TextureBitField, TextureDataMapping};
use crate::utils::image::png;
use crate::utils::json::{self, JsonValue};

/// `getBarycentricCoordinates`: (u, v, w) of `point` in `triangle`, `None` if degenerate.
pub fn get_barycentric_coordinates(point: &Point3LL, triangle: &[Point3LL; 3]) -> Option<Point3D> {
    let v0 = triangle[1] - triangle[0];
    let v1 = triangle[2] - triangle[0];
    let v2 = *point - triangle[0];
    let d00 = v0.dot(&v0);
    let d01 = v0.dot(&v1);
    let d11 = v1.dot(&v1);
    let d20 = v2.dot(&v0);
    let d21 = v2.dot(&v1);
    let denom = d00.wrapping_mul(d11).wrapping_sub(d01.wrapping_mul(d01));
    if denom == 0 {
        return None;
    }
    let v = (d11.wrapping_mul(d20).wrapping_sub(d01.wrapping_mul(d21))) as f64 / denom as f64;
    let w = (d00.wrapping_mul(d21).wrapping_sub(d01.wrapping_mul(d20))) as f64 / denom as f64;
    let u = 1.0 - v - w;
    Some(Point3D::new(u, v, w))
}

/// `getUVCoordinates`: computed in double precision, stored as float.
pub fn get_uv_coordinates(b: &Point3D, uv: &[Point2F; 3]) -> Point2F {
    Point2F::new(
        ((uv[0].x as f64 * b.x) + (uv[1].x as f64 * b.y) + (uv[2].x as f64 * b.z)) as f32,
        ((uv[0].y as f64 * b.x) + (uv[1].y as f64 * b.y) + (uv[2].y as f64 * b.z)) as f32,
    )
}

/// `rapidjson::Value::GetUint()` for the manifest bit indices.
fn get_uint(v: &JsonValue) -> u32 {
    match v {
        JsonValue::Int(i) => *i as u32,
        JsonValue::Uint(u) => *u as u32,
        JsonValue::Double(d) => *d as u32,
        _ => 0,
    }
}

/// `loadTextureFromPngData`.
pub fn load_texture_from_png_data(texture_data: &[u8], mesh: &mut Mesh, source_description: &str, log_no_metadata: bool) -> bool {
    if texture_data.is_empty() {
        return false;
    }
    let image = match png::decode_simplified(texture_data) {
        Ok(img) => img,
        Err(msg) => {
            if log_no_metadata {
                crate::log_warn!("Error reading PNG texture {}: {}", source_description, msg);
            } else {
                crate::log_error!("Error when beginning reading mesh texture: {}", msg);
            }
            return false;
        }
    };
    if image.texts.is_empty() {
        if log_no_metadata {
            crate::log_info!("No metadata found in texture: {}", source_description);
        }
        return false;
    }

    let mut texture_data_mapping = TextureDataMapping::new();
    for text in &image.texts {
        if text.key == "Description" {
            let document = match json::parse(&text.text) {
                Ok(d) => d,
                Err(e) => {
                    if log_no_metadata {
                        crate::log_warn!("Error parsing texture metadata in {}: {}", source_description, e);
                    } else {
                        crate::log_error!("Error parsing texture data mapping: {}", e);
                    }
                    return false;
                }
            };
            for (feature_name, array) in document.members() {
                if array.is_array() && array.elements().len() == 2 {
                    texture_data_mapping.insert(
                        feature_name.clone(),
                        TextureBitField {
                            bit_range_start_index: get_uint(&array.elements()[0]) as usize,
                            bit_range_end_index: get_uint(&array.elements()[1]) as usize,
                        },
                    );
                }
            }
            break;
        }
    }

    if !texture_data_mapping.is_empty() {
        let n = texture_data_mapping.len();
        mesh.texture = Some(Arc::new(Image::new(image.width, image.height, image.bytes_per_pixel, image.data)));
        mesh.texture_data_mapping = Some(Arc::new(texture_data_mapping));
        if log_no_metadata {
            crate::log_info!("Loaded texture {} with {} paint features", source_description, n);
        }
        return true;
    }
    false
}

/// `loadTextureFromFile`.
pub fn load_texture_from_file(mesh: &mut Mesh, texture_filename: &str) -> bool {
    if !std::path::Path::new(texture_filename).exists() {
        return false;
    }
    match std::fs::read(texture_filename) {
        Ok(data) => load_texture_from_png_data(&data, mesh, texture_filename, true),
        Err(_) => {
            crate::log_warn!("Failed to open texture file: {}", texture_filename);
            false
        }
    }
}
