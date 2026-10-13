//! Which models are printable (`BuildVolume.updateNodeBoundaryCheck`): inside the build plate
//! minus the edge border, below the maximum height, away from the printer's disallowed areas and
//! assigned to an enabled extruder.

use crate::hull::{self, P2};
use crate::session::Session;
use curust_settings::py::Value;
use curust_settings::stack::StackId;

fn num(v: Option<Value>) -> f64 {
    v.and_then(|v| v.as_f64()).unwrap_or(0.0)
}

/// `getEdgeDisallowedSize` (with the extruders that print models standing in for
/// `getUsedExtruderStacks`).
pub fn edge_disallowed_size(s: &Session) -> f64 {
    let Some(m) = s.machine() else { return 0.0 };
    let g = |k: &str| m.get_value(StackId::Global, k);
    if g("print_sequence").map(|v| v.py_str()) == Some("one_at_a_time".into()) {
        return 0.1;
    }
    let (w, d, _) = s.machine_size();
    let max_len = 0.5 * w.min(d);
    let adhesion = g("adhesion_type").map(|v| v.py_str()).unwrap_or_default();
    let bed_adhesion = if adhesion == "raft" { num(g("raft_margin")).min(max_len) } else { 0.0f64.min(max_len) };
    let support_expansion = if g("support_enable").map(|v| v.truthy()).unwrap_or(false) { num(g("support_offset")) } else { 0.0 };
    let mut shield = 0.0f64;
    if g("draft_shield_enabled").map(|v| v.truthy()).unwrap_or(false) {
        shield = shield.max(num(g("draft_shield_dist")));
    }
    if g("ooze_shield_enabled").map(|v| v.truthy()).unwrap_or(false) {
        shield = shield.max(num(g("ooze_shield_dist")));
    }
    let mut move_from_wall = 0.0f64;
    let list = m.extruder_list();
    let mut used: Vec<usize> = s.nodes.iter().filter_map(|n| list.get(n.extruder).copied()).collect();
    used.sort();
    used.dedup();
    if used.is_empty() {
        used = list.first().copied().into_iter().collect();
    }
    for i in used {
        let e = |k: &str| m.get_value(StackId::Extruder(i), k);
        if e("travel_avoid_other_parts").map(|v| v.truthy()).unwrap_or(false) {
            move_from_wall = move_from_wall.max(num(e("travel_avoid_distance")));
        }
        let mut wipe = num(e("infill_wipe_dist"));
        let walls = num(e("wall_line_count"));
        if walls >= 1.0 {
            wipe -= num(e("wall_line_width_0"));
            if walls >= 2.0 {
                wipe -= num(e("wall_line_width_x")) * (walls - 1.0);
            }
        }
        move_from_wall = move_from_wall.max(wipe);
    }
    move_from_wall.max(support_expansion + shield).max(support_expansion + bed_adhesion)
}

/// Raft thickness above the plate (`BuildVolume._updateRaftThickness`).
pub fn raft_thickness(s: &Session) -> f64 {
    let Some(m) = s.machine() else { return 0.0 };
    let g = |k: &str| num(m.get_value(StackId::Global, k));
    if m.get_value(StackId::Global, "adhesion_type").map(|v| v.py_str()) != Some("raft".into()) {
        return 0.0;
    }
    g("raft_base_thickness") + g("raft_interface_layers") * g("raft_interface_thickness") + g("raft_surface_layers") * g("raft_surface_thickness") + g("raft_airgap")
        - g("layer_0_z_overlap")
}

/// The disallowed areas of the printer (`machine_disallowed_areas`), in the plate plane.
pub fn disallowed_areas(s: &Session) -> Vec<Vec<P2>> {
    let Some(m) = s.machine() else { return Vec::new() };
    let mut out = Vec::new();
    for key in ["machine_disallowed_areas", "nozzle_disallowed_areas"] {
        if let Some(v) = m.get_value(StackId::Global, key) {
            for area in v.iter_items().unwrap_or_default() {
                let pts: Vec<P2> = area
                    .iter_items()
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|p| {
                        let c = p.iter_items().ok()?;
                        Some([c.first()?.as_f64()?, c.get(1)?.as_f64()?])
                    })
                    .collect();
                if pts.len() >= 3 {
                    out.push(hull::convex_hull(&pts));
                }
            }
        }
    }
    out
}

/// The plate-plane convex hull of a node.
pub fn node_hull(n: &crate::scene::Node) -> Vec<P2> {
    let w = n.world();
    let pts: Vec<P2> = n
        .mesh
        .vertices
        .iter()
        .map(|p| {
            let (x, y, z) = (p[0] as f64, p[1] as f64, p[2] as f64);
            [w[0][0] * x + w[0][1] * y + w[0][2] * z + w[0][3], w[2][0] * x + w[2][1] * y + w[2][2] * z + w[2][3]]
        })
        .collect();
    hull::convex_hull(&pts)
}

pub fn outside_flags(s: &Session) -> Vec<bool> {
    let Some(m) = s.machine() else { return vec![false; s.nodes.len()] };
    let (w, d, h) = s.machine_size();
    let border = edge_disallowed_size(s);
    let raft = raft_thickness(s);
    let elliptic = m.get_value(StackId::Global, "machine_shape").map(|v| v.py_str()) == Some("elliptic".into());
    let one_at_a_time = m.get_value(StackId::Global, "print_sequence").map(|v| v.py_str()) == Some("one_at_a_time".into());
    let mut max_h = h - raft;
    if one_at_a_time && s.nodes.len() > 1 {
        max_h = max_h.min(num(m.get_value(StackId::Global, "gantry_height")));
    }
    let areas = disallowed_areas(s);
    let list = m.extruder_list();
    s.nodes
        .iter()
        .map(|n| {
            let (min, max) = n.world_bounds();
            if max[1] < 0.0 {
                return true;
            }
            if max[1] > max_h + 1e-6 {
                return true;
            }
            if min[0] < -w / 2.0 - 1e-6 || max[0] > w / 2.0 + 1e-6 || min[2] < -d / 2.0 - 1e-6 || max[2] > d / 2.0 + 1e-6 {
                return true;
            }
            let hull_pts = node_hull(n);
            for p in &hull_pts {
                if elliptic {
                    let rx = w / 2.0 - border;
                    let ry = d / 2.0 - border;
                    if rx <= 0.0 || ry <= 0.0 || (p[0] / rx).powi(2) + (p[1] / ry).powi(2) > 1.0 + 1e-9 {
                        return true;
                    }
                } else if p[0] < -w / 2.0 + border - 1e-6 || p[0] > w / 2.0 - border + 1e-6 || p[1] < -d / 2.0 + border - 1e-6 || p[1] > d / 2.0 - border + 1e-6 {
                    return true;
                }
            }
            let grown = if border > 0.0 { hull::minkowski(&hull_pts, &hull::approximated_circle(border)) } else { hull_pts.clone() };
            if areas.iter().any(|a| hull::intersects(&grown, a)) {
                return true;
            }
            match list.get(n.extruder) {
                Some(&i) => !m.extruders[i].is_enabled(),
                None => true,
            }
        })
        .collect()
}
