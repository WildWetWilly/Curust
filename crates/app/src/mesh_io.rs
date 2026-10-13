//! Model file readers (STL, OBJ, 3MF) producing Cura-style mesh data: float32 triangle soup in
//! Cura's scene coordinates (Y up, Z towards the front: `(x, z, -y)` of the file's Z-up space).

use crate::xmlmini::{self, Element};
use crate::zip;

/// A model read from a file, before it is placed in the scene.
#[derive(Clone, Debug, Default)]
pub struct LoadedModel {
    pub name: String,
    /// Three vertices per face, Cura coordinates (Y up).
    pub vertices: Vec<[f32; 3]>,
    /// Row-major 4x4 transformation (Cura scene space) given by the file (3MF build items); the
    /// mesh is then not centred by the reader.
    pub transform: Option<[[f64; 4]; 4]>,
    /// Per-object settings stored in the file (3MF `cura:` metadata).
    pub settings: Vec<(String, String)>,
}

pub fn load_model_data(name: &str, data: &[u8], machine_width: f64, machine_depth: f64) -> Result<Vec<LoadedModel>, String> {
    let lower = name.to_ascii_lowercase();
    let models = if lower.ends_with(".stl") {
        vec![LoadedModel { name: name.to_string(), vertices: read_stl(data)?, ..Default::default() }]
    } else if lower.ends_with(".obj") {
        vec![LoadedModel { name: name.to_string(), vertices: read_obj(data)?, ..Default::default() }]
    } else if lower.ends_with(".3mf") {
        read_3mf(name, data, machine_width, machine_depth)?
    } else {
        return Err(format!("Unsupported file type: {}", name));
    };
    let models: Vec<LoadedModel> = models.into_iter().filter(|m| !m.vertices.is_empty()).collect();
    if models.is_empty() {
        return Err(format!("{} does not contain any triangles", name));
    }
    Ok(models)
}

// ---------------------------------------------------------------------------------------------
// STL
// ---------------------------------------------------------------------------------------------

/// numpy-stl reads into float32; Uranium negates Y and swaps Y/Z.
fn stl_vertex(x: f32, y: f32, z: f32) -> [f32; 3] {
    [x, z, -y]
}

pub fn read_stl(data: &[u8]) -> Result<Vec<[f32; 3]>, String> {
    let is_binary_size = data.len() >= 84 && {
        let n = u32::from_le_bytes(data[80..84].try_into().unwrap()) as usize;
        84 + n * 50 == data.len()
    };
    let header_is_solid = {
        let start = data.iter().position(|c| !c.is_ascii_whitespace()).unwrap_or(0);
        data.len() >= start + 5 && data[start..start + 5].eq_ignore_ascii_case(b"solid")
    };
    if header_is_solid && !is_binary_size {
        if let Ok(v) = read_stl_ascii(data) {
            if !v.is_empty() {
                return Ok(v);
            }
        }
    }
    if data.len() < 84 {
        return Err("STL file too small".into());
    }
    let n = u32::from_le_bytes(data[80..84].try_into().unwrap()) as usize;
    let n = n.min((data.len() - 84) / 50);
    let mut out = Vec::with_capacity(n * 3);
    for i in 0..n {
        let base = 84 + i * 50 + 12;
        for v in 0..3 {
            let f = |k: usize| f32::from_le_bytes(data[base + v * 12 + k * 4..base + v * 12 + k * 4 + 4].try_into().unwrap());
            out.push(stl_vertex(f(0), f(1), f(2)));
        }
    }
    Ok(out)
}

fn read_stl_ascii(data: &[u8]) -> Result<Vec<[f32; 3]>, String> {
    let text = String::from_utf8_lossy(data);
    let mut out = Vec::new();
    let mut tokens = text.split_ascii_whitespace();
    while let Some(t) = tokens.next() {
        if t.eq_ignore_ascii_case("vertex") {
            let mut c = [0f32; 3];
            for v in c.iter_mut() {
                let s = tokens.next().ok_or("truncated vertex")?;
                *v = s.parse::<f64>().map_err(|_| format!("bad number {}", s))? as f32;
            }
            out.push(stl_vertex(c[0], c[1], c[2]));
        }
    }
    if out.len() % 3 != 0 {
        out.truncate(out.len() / 3 * 3);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// OBJ (Uranium OBJReader)
// ---------------------------------------------------------------------------------------------

pub fn read_obj(data: &[u8]) -> Result<Vec<[f32; 3]>, String> {
    let text = String::from_utf8_lossy(data);
    let mut vertex_list: Vec<[f64; 3]> = Vec::new();
    let mut faces: Vec<[i64; 3]> = Vec::new();
    let mut previous: Vec<String> = Vec::new();
    let to_abs = |count: usize, i: i64| -> i64 {
        if i < 0 {
            count as i64 + i + 1
        } else {
            i
        }
    };
    for line in text.lines() {
        let mut parts: Vec<String> = std::mem::take(&mut previous);
        parts.extend(line.split_whitespace().map(String::from));
        if parts.is_empty() {
            continue;
        }
        if parts.last().map(|s| s == "\\").unwrap_or(false) {
            parts.pop();
            previous = parts;
            continue;
        }
        match parts[0].as_str() {
            "f" => {
                let idx: Vec<i64> = parts[1..].iter().map(|p| p.split('/').next().unwrap_or("0").parse::<i64>().unwrap_or(0)).collect();
                for k in 0..idx.len().saturating_sub(2) {
                    faces.push([to_abs(vertex_list.len(), idx[0]), to_abs(vertex_list.len(), idx[k + 1]), to_abs(vertex_list.len(), idx[k + 2])]);
                }
            }
            "v" if parts.len() >= 4 => {
                let f = |i: usize| parts[i].parse::<f64>().unwrap_or(0.0);
                vertex_list.push([f(1), f(3), -f(2)]);
            }
            _ => {}
        }
    }
    let n = vertex_list.len() as i64;
    let mut out = Vec::with_capacity(faces.len() * 3);
    for f in faces {
        for &i in &f {
            let mut i = i - 1;
            if i < 0 || i >= n {
                i = 0;
            }
            if n == 0 {
                continue;
            }
            let v = vertex_list[i as usize];
            out.push([v[0] as f32, v[1] as f32, v[2] as f32]);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// 3MF
// ---------------------------------------------------------------------------------------------

type Mat4 = [[f64; 4]; 4];

fn identity() -> Mat4 {
    [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]
}

pub fn mat_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut r = [[0.0; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            r[i][j] = (0..4).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    r
}

/// A 3MF `transform` attribute ("m00 m01 m02 m10 m11 m12 m20 m21 m22 m30 m31 m32", row vectors)
/// as a column-vector matrix.
fn parse_3mf_transform(s: Option<&str>) -> Mat4 {
    let Some(s) = s else { return identity() };
    let v: Vec<f64> = s.split_whitespace().filter_map(|t| t.parse().ok()).collect();
    if v.len() != 12 {
        return identity();
    }
    [[v[0], v[3], v[6], v[9]], [v[1], v[4], v[7], v[10]], [v[2], v[5], v[8], v[11]], [0.0, 0.0, 0.0, 1.0]]
}

struct Object3mf {
    vertices: Vec<[f32; 3]>,
    triangles: Vec<[usize; 3]>,
    components: Vec<(String, Mat4)>,
    name: String,
    settings: Vec<(String, String)>,
}

fn unit_scale(unit: &str) -> f64 {
    match unit {
        "micron" => 0.001,
        "centimeter" => 10.0,
        "inch" => 25.4,
        "foot" => 304.8,
        "meter" => 1000.0,
        _ => 1.0,
    }
}

fn collect_mesh(objects: &std::collections::HashMap<String, Object3mf>, id: &str, m: &Mat4, out: &mut Vec<[f32; 3]>, depth: usize) {
    if depth > 16 {
        return;
    }
    let Some(o) = objects.get(id) else { return };
    for t in &o.triangles {
        for &i in t {
            if let Some(v) = o.vertices.get(i) {
                let (x, y, z) = (v[0] as f64, v[1] as f64, v[2] as f64);
                let p = [m[0][0] * x + m[0][1] * y + m[0][2] * z + m[0][3], m[1][0] * x + m[1][1] * y + m[1][2] * z + m[1][3], m[2][0] * x + m[2][1] * y + m[2][2] * z + m[2][3]];
                out.push([p[0] as f32, p[1] as f32, p[2] as f32]);
            }
        }
    }
    for (cid, cm) in &o.components {
        let mm = mat_mul(m, cm);
        collect_mesh(objects, cid, &mm, out, depth + 1);
    }
}

fn metadata_settings(e: &Element) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for g in e.children_named("metadatagroup") {
        for md in g.children_named("metadata") {
            if let Some(n) = md.attr("name") {
                let n = n.strip_prefix("cura:").unwrap_or(n);
                out.push((n.to_string(), md.text.clone()));
            }
        }
    }
    out
}

pub fn read_3mf(file_name: &str, data: &[u8], machine_width: f64, machine_depth: f64) -> Result<Vec<LoadedModel>, String> {
    let entries = zip::read_zip(data)?;
    let mut model_files: Vec<&zip::ZipEntry> = entries.iter().filter(|e| e.name.to_ascii_lowercase().ends_with(".model")).collect();
    // The root model (3D/3dmodel.model) last so that its objects can reference the others.
    model_files.sort_by_key(|e| e.name.to_ascii_lowercase() == "3d/3dmodel.model");
    let mut objects: std::collections::HashMap<String, Object3mf> = std::collections::HashMap::new();
    let mut build_items: Vec<(String, Mat4, Vec<(String, String)>)> = Vec::new();
    let mut unit = "millimeter".to_string();
    for mf in model_files {
        let text = String::from_utf8_lossy(&mf.data);
        let root = xmlmini::parse(&text).map_err(|e| format!("{}: {}", mf.name, e))?;
        let is_root = mf.name.to_ascii_lowercase() == "3d/3dmodel.model";
        if is_root {
            if let Some(u) = root.attr("unit") {
                unit = u.to_string();
            }
        }
        let prefix = if is_root { String::new() } else { format!("/{}", mf.name) };
        if let Some(res) = root.child("resources") {
            for o in res.children_named("object") {
                let id = o.attr("id").unwrap_or("").to_string();
                let mut obj = Object3mf { vertices: Vec::new(), triangles: Vec::new(), components: Vec::new(), name: o.attr("name").unwrap_or("").to_string(), settings: metadata_settings(o) };
                if let Some(mesh) = o.child("mesh") {
                    if let Some(vs) = mesh.child("vertices") {
                        for v in vs.children_named("vertex") {
                            let f = |k: &str| v.attr(k).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) as f32;
                            obj.vertices.push([f("x"), f("y"), f("z")]);
                        }
                    }
                    if let Some(ts) = mesh.child("triangles") {
                        for t in ts.children_named("triangle") {
                            let f = |k: &str| t.attr(k).and_then(|s| s.parse::<usize>().ok()).unwrap_or(usize::MAX);
                            obj.triangles.push([f("v1"), f("v2"), f("v3")]);
                        }
                    }
                }
                if let Some(comps) = o.child("components") {
                    for c in comps.children_named("component") {
                        let oid = c.attr("objectid").unwrap_or("").to_string();
                        let path = c.attr_any(&["path", "p:path"]).map(|p| p.to_string());
                        let key = match path {
                            Some(p) if !p.is_empty() && p.trim_start_matches('/').to_ascii_lowercase() != "3d/3dmodel.model" => format!("{}#{}", p, oid),
                            _ => format!("{}#{}", prefix, oid),
                        };
                        obj.components.push((key, parse_3mf_transform(c.attr("transform"))));
                    }
                }
                objects.insert(format!("{}#{}", prefix, id), obj);
            }
        }
        if is_root {
            if let Some(build) = root.child("build") {
                for item in build.children_named("item") {
                    let oid = item.attr("objectid").unwrap_or("").to_string();
                    build_items.push((format!("#{}", oid), parse_3mf_transform(item.attr("transform")), metadata_settings(item)));
                }
            }
        }
    }
    // Z-up 3MF space (origin at the front left corner of the plate) to Cura's Y-up centred space.
    let s = unit_scale(&unit);
    let to_cura: Mat4 = mat_mul(
        &[[1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, -1.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0]],
        &mat_mul(&[[1.0, 0.0, 0.0, -machine_width / 2.0], [0.0, 1.0, 0.0, -machine_depth / 2.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]], &[[s, 0.0, 0.0, 0.0], [0.0, s, 0.0, 0.0], [0.0, 0.0, s, 0.0], [0.0, 0.0, 0.0, 1.0]]),
    );
    let mut out = Vec::new();
    for (n, (oid, m, item_settings)) in build_items.iter().enumerate() {
        let mut verts = Vec::new();
        collect_mesh(&objects, oid, &identity(), &mut verts, 0);
        if verts.is_empty() {
            continue;
        }
        // Mesh in object space (Z up) → Cura axes without translation; the item transform goes to
        // the node.
        let axes = [[1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, -1.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
        let axes_inv = [[1.0, 0.0, 0.0, 0.0], [0.0, 0.0, -1.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
        let vertices: Vec<[f32; 3]> = verts.iter().map(|v| [v[0], v[2], -v[1]]).collect();
        let node_transform = mat_mul(&mat_mul(&to_cura, m), &axes_inv);
        let _ = axes;
        let obj = objects.get(oid);
        let mut name = obj.map(|o| o.name.clone()).unwrap_or_default();
        if name.is_empty() {
            name = if build_items.len() == 1 { file_name.to_string() } else { format!("{} ({})", file_name, n + 1) };
        }
        let mut settings = obj.map(|o| o.settings.clone()).unwrap_or_default();
        settings.extend(item_settings.iter().cloned());
        out.push(LoadedModel { name, vertices, transform: Some(node_transform), settings });
    }
    Ok(out)
}

/// Write the scene's models as a 3MF file (world-space meshes, like Cura's 3MF writer without
/// project data).
pub fn write_3mf(models: &[(String, Vec<[f32; 3]>, Vec<(String, String)>)], machine_width: f64, machine_depth: f64) -> Vec<u8> {
    use std::fmt::Write as _;
    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<model unit=\"millimeter\" xml:lang=\"en-US\" xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\" xmlns:cura=\"http://software.ultimaker.com/xml/cura/3mf/2015/10\">\n  <metadata name=\"Application\">Curust</metadata>\n  <resources>\n");
    for (i, (name, verts, settings)) in models.iter().enumerate() {
        let _ = writeln!(xml, "    <object id=\"{}\" name=\"{}\" type=\"model\">", i + 1, xmlmini::escape(name));
        if !settings.is_empty() {
            xml.push_str("      <metadatagroup>\n");
            for (k, v) in settings {
                let _ = writeln!(xml, "        <metadata name=\"cura:{}\" preserve=\"true\" type=\"xs:string\">{}</metadata>", xmlmini::escape(k), xmlmini::escape(v));
            }
            xml.push_str("      </metadatagroup>\n");
        }
        xml.push_str("      <mesh>\n        <vertices>\n");
        // Cura (x, y-up, z) → 3MF (x, -z, y) with the origin at the front left corner.
        let mut index: std::collections::HashMap<[u32; 3], usize> = std::collections::HashMap::new();
        let mut tris = Vec::new();
        let mut count = 0usize;
        for f in verts.chunks(3) {
            let mut t = [0usize; 3];
            for (k, v) in f.iter().enumerate() {
                let p = [v[0] + (machine_width / 2.0) as f32, -v[2] + (machine_depth / 2.0) as f32, v[1]];
                let key = [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()];
                t[k] = *index.entry(key).or_insert_with(|| {
                    let _ = writeln!(xml, "          <vertex x=\"{}\" y=\"{}\" z=\"{}\" />", p[0], p[1], p[2]);
                    count += 1;
                    count - 1
                });
            }
            tris.push(t);
        }
        xml.push_str("        </vertices>\n        <triangles>\n");
        for t in tris {
            let _ = writeln!(xml, "          <triangle v1=\"{}\" v2=\"{}\" v3=\"{}\" />", t[0], t[1], t[2]);
        }
        xml.push_str("        </triangles>\n      </mesh>\n    </object>\n");
    }
    xml.push_str("  </resources>\n  <build>\n");
    for i in 0..models.len() {
        let _ = writeln!(xml, "    <item objectid=\"{}\" />", i + 1);
    }
    xml.push_str("  </build>\n</model>\n");
    let content_types = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\" /><Default Extension=\"model\" ContentType=\"application/vnd.ms-package.3dmanufacturing-3dmodel+xml\" /></Types>\n";
    let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Target=\"/3D/3dmodel.model\" Id=\"rel0\" Type=\"http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel\" /></Relationships>\n";
    zip::write_zip(&[
        ("[Content_Types].xml".into(), content_types.as_bytes().to_vec()),
        ("_rels/.rels".into(), rels.as_bytes().to_vec()),
        ("3D/3dmodel.model".into(), xml.into_bytes()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stl_ascii_and_binary() {
        let ascii = "solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 2\nendloop\nendfacet\nendsolid t\n";
        let v = read_stl(ascii.as_bytes()).unwrap();
        assert_eq!(v, vec![[0.0, 0.0, -0.0], [1.0, 0.0, -0.0], [0.0, 2.0, -1.0]]);
        let mut bin = vec![0u8; 80];
        bin.extend_from_slice(&1u32.to_le_bytes());
        bin.extend_from_slice(&[0u8; 12]);
        for c in [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 2.0] {
            bin.extend_from_slice(&c.to_le_bytes());
        }
        bin.extend_from_slice(&[0, 0]);
        assert_eq!(read_stl(&bin).unwrap(), v);
    }
    #[test]
    fn obj_quads() {
        let obj = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf 1 2 3 4\n";
        assert_eq!(read_obj(obj.as_bytes()).unwrap().len(), 6);
    }
    #[test]
    fn threemf_roundtrip() {
        let tri = vec![[0.0f32, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 5.0, 0.0], [0.0, 0.0, 0.0], [0.0, 5.0, 0.0], [0.0, 0.0, -3.0]];
        let data = write_3mf(&[("t".into(), tri.clone(), vec![("infill_sparse_density".into(), "40".into())])], 200.0, 200.0);
        let m = read_3mf("t.3mf", &data, 200.0, 200.0).unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].settings, vec![("infill_sparse_density".to_string(), "40".to_string())]);
        let t = m[0].transform.unwrap();
        for (a, b) in m[0].vertices.iter().zip(tri.iter()) {
            let (x, y, z) = (a[0] as f64, a[1] as f64, a[2] as f64);
            let w = [t[0][0] * x + t[0][1] * y + t[0][2] * z + t[0][3], t[1][0] * x + t[1][1] * y + t[1][2] * z + t[1][3], t[2][0] * x + t[2][1] * y + t[2][2] * z + t[2][3]];
            for k in 0..3 {
                assert!((w[k] - b[k] as f64).abs() < 1e-4, "{:?} vs {:?}", w, b);
            }
        }
    }
}
