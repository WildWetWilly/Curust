//! Debug dumps of intermediate data, used to compare this port against an instrumented build of
//! the C++ engine. Enabled by setting the environment variable `CURA_DUMP` to a file path.

use crate::geometry::{Polyline, Shape};
use crate::utils::extrusion_line::VariableWidthLines;
use std::fs::File;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

fn sink() -> &'static Option<Mutex<File>> {
    static SINK: OnceLock<Option<Mutex<File>>> = OnceLock::new();
    SINK.get_or_init(|| std::env::var("CURA_DUMP").ok().and_then(|p| File::create(p).ok()).map(Mutex::new))
}

pub fn enabled() -> bool {
    sink().is_some()
}

pub fn write(text: &str) {
    if let Some(m) = sink() {
        let _ = m.lock().unwrap().write_all(text.as_bytes());
    }
}

pub fn shape_to_string(shape: &Shape) -> String {
    let mut s = String::new();
    for poly in shape.iter() {
        s.push_str(&format!("  poly {}:", poly.points().len()));
        for p in poly.points() {
            s.push_str(&format!(" {},{}", p.x, p.y));
        }
        s.push('\n');
    }
    s
}

pub fn dump_shape(header: &str, shape: &Shape) {
    if !enabled() {
        return;
    }
    write(&format!("{}\n{}", header, shape_to_string(shape)));
}

pub fn toolpaths_to_string(paths: &[VariableWidthLines]) -> String {
    let mut s = String::new();
    for (i, lines) in paths.iter().enumerate() {
        for line in lines {
            s.push_str(&format!("  inset {} line idx={} odd={} closed={} n={}:", i, line.inset_idx, line.is_odd as i32, line.is_closed as i32, line.junctions.len()));
            for j in &line.junctions {
                s.push_str(&format!(" {},{},{}", j.p.x, j.p.y, j.w));
            }
            s.push('\n');
        }
    }
    s
}

/// Dump the overhang areas (when `with_infill` is false) and the support layers.
pub fn dump_support(storage: &crate::slice_data_storage::SliceDataStorage, tag: &str, with_infill: bool) {
    if !enabled() {
        return;
    }
    if !with_infill {
        for (mesh_idx, mesh) in storage.meshes.iter().enumerate() {
            for layer_idx in 0..mesh.overhang_areas.len() {
                let mut s = format!("overhang mesh={} layer={}\n", mesh_idx, layer_idx);
                s.push_str(" overhang\n");
                s.push_str(&shape_to_string(&mesh.overhang_areas[layer_idx]));
                if layer_idx < mesh.full_overhang_areas.len() {
                    s.push_str(" full_overhang\n");
                    s.push_str(&shape_to_string(&mesh.full_overhang_areas[layer_idx]));
                }
                write(&s);
            }
        }
    }
    write(&format!(
        "{} mesh=-1 layer=-1 generated={} max_filled={} nlayers={}\n",
        tag,
        storage.support.generated as i32,
        storage.support.layer_nr_max_filled_layer,
        storage.support.support_layers.len()
    ));
    for (layer_idx, layer) in storage.support.support_layers.iter().enumerate() {
        let mut s = format!("{} mesh=-1 layer={} nparts={}\n", tag, layer_idx, layer.support_infill_parts.len());
        for part in &layer.support_infill_parts {
            let bb = &part.outline_boundary_box;
            s.push_str(&format!(
                " part w={} insets={} cld={} frac={} bb={},{},{},{}\n",
                part.support_line_width, part.inset_count_to_generate, part.custom_line_distance, part.use_fractional_config as i32, bb.min.x, bb.min.y, bb.max.x, bb.max.y
            ));
            s.push_str(" outline\n");
            s.push_str(&shape_to_string(&part.outline.0));
            if with_infill {
                for (d, per_combine) in part.infill_area_per_combine_per_density.iter().enumerate() {
                    for (c, area) in per_combine.iter().enumerate() {
                        s.push_str(&format!(" infill d={} c={}\n", d, c));
                        s.push_str(&shape_to_string(area));
                    }
                }
                s.push_str(" toolpaths\n");
                s.push_str(&toolpaths_to_string(&part.wall_toolpaths));
            }
        }
        s.push_str(" bottom\n");
        s.push_str(&shape_to_string(&layer.support_bottom));
        s.push_str(" roof\n");
        s.push_str(&shape_to_string(&layer.support_roof));
        s.push_str(" fractional_roof\n");
        s.push_str(&shape_to_string(&layer.support_fractional_roof));
        s.push_str(" mesh_drop_down\n");
        s.push_str(&shape_to_string(&layer.support_mesh_drop_down));
        s.push_str(" support_mesh\n");
        s.push_str(&shape_to_string(&layer.support_mesh));
        s.push_str(" anti_overhang\n");
        s.push_str(&shape_to_string(&layer.anti_overhang));
        write(&s);
    }
}

/// Dump skirt/brim, support brim, raft outlines and shields.
pub fn dump_adhesion(storage: &crate::slice_data_storage::SliceDataStorage) {
    use crate::geometry::mixed_lines_set::AnyPolyline;
    if !enabled() {
        return;
    }
    let pts = |s: &mut String, points: &[crate::geometry::Point2LL]| {
        for p in points {
            s.push_str(&format!(" {},{}", p.x, p.y));
        }
        s.push('\n');
    };
    for (extruder_nr, insets) in storage.skirt_brim.iter().enumerate() {
        for (inset_idx, lines) in insets.iter().enumerate() {
            let mut s = format!("adhesion mesh={} layer={} n={}\n", extruder_nr, inset_idx, lines.len());
            for line in lines.iter() {
                let t = match &**line {
                    AnyPolyline::Polygon(_) => 'P',
                    AnyPolyline::Closed(c) => {
                        if c.explicitly_closed {
                            'C'
                        } else {
                            'c'
                        }
                    }
                    AnyPolyline::Open(_) => 'O',
                };
                s.push_str(&format!("  {} {}:", t, line.points().len()));
                pts(&mut s, line.points());
            }
            write(&s);
        }
    }
    let mut s = String::from("adhesion mesh=-1 layer=-1\n");
    s.push_str(" support_brim\n");
    for line in storage.support_brim.iter() {
        s.push_str(&format!("  line {}:", line.points.len()));
        pts(&mut s, &line.points);
    }
    s.push_str(" raft_base\n");
    s.push_str(&shape_to_string(&storage.raft_base_outline));
    s.push_str(" raft_interface\n");
    s.push_str(&shape_to_string(&storage.raft_interface_outline));
    s.push_str(" raft_surface\n");
    s.push_str(&shape_to_string(&storage.raft_surface_outline));
    s.push_str(" draft_shield\n");
    s.push_str(&shape_to_string(&storage.draft_protection_shield));
    write(&s);
    for (layer_idx, shield) in storage.ooze_shield.iter().enumerate() {
        write(&format!("adhesion mesh=-2 layer={}\n{}", layer_idx, shape_to_string(shield)));
    }
}
