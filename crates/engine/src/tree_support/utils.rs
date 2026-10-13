//! `TreeSupportUtils.h`.

use crate::geometry::lines_set::{OpenLinesSet, Shape};
use crate::geometry::point2ll::{scale_f64, v_size, v_size2, Point2LL};
use crate::geometry::polyline::{ClosedLine, OpenPolyline};
use crate::infill::sierpinski_fill_provider::SierpinskiFillProvider;
use crate::settings::enums::EFillMethod;
use crate::settings::types::LayerIndex;
use crate::utils::coord::Coord;
use crate::utils::extrusion_line::VariableWidthLines;
use crate::utils::polygon_utils::PolygonUtils;
use crate::utils::section_type::SectionType;
use crate::utils::simplify::Simplify;
use clipper::JoinType;

use super::settings::{cmax, TreeSupportSettings};

/// `toPolylines(const Shape&)`.
pub fn to_polylines(poly: &Shape) -> OpenLinesSet {
    let mut result = OpenLinesSet::default();
    for path in poly.iter() {
        result.push(ClosedLine::to_pseudo_open_polyline(path));
    }
    result
}

/// `toPolylines(const std::vector<VariableWidthLines>)`.
pub fn to_polylines_vwl(toolpaths: &[VariableWidthLines]) -> OpenLinesSet {
    let mut result = OpenLinesSet::default();
    for lines in toolpaths {
        for line in lines {
            if line.junctions.is_empty() {
                continue;
            }
            let mut pts: Vec<Point2LL> = line.junctions.iter().map(|j| j.p).collect();
            if line.is_closed {
                pts.push(line.junctions[0].p);
            }
            result.push(OpenPolyline::new(pts));
        }
    }
    result
}

/// `generateSupportInfillLines`.
#[allow(clippy::too_many_arguments)]
pub fn generate_support_infill_lines(
    area: &Shape,
    config: &TreeSupportSettings,
    roof: bool,
    layer_idx: LayerIndex,
    support_infill_distance: Coord,
    cross_fill_provider: Option<&SierpinskiFillProvider>,
    include_walls: bool,
    generate_support_supporting: bool,
) -> OpenLinesSet {
    let pattern = if generate_support_supporting {
        EFillMethod::GRID
    } else if roof {
        config.roof_pattern
    } else {
        config.support_pattern
    };
    let zig_zaggify_infill = if roof { pattern == EFillMethod::ZIG_ZAG } else { config.zig_zaggify_support };
    let connect_polygons = false;
    let support_roof_overlap: Coord = 0;
    let infill_multiplier: usize = 1;
    let support_shift: i32 = if roof { 0 } else { (support_infill_distance / 2) as i32 };
    let wall_line_count: usize =
        if include_walls { (if !roof { config.support_wall_count } else { config.support_roof_wall_count }) as i64 as usize } else { 0 };
    let narrow_area_width: Coord = 0;
    let infill_origin = Point2LL::new(0, 0);
    let skip_stitching = false;
    let fill_gaps = true;
    let use_endpieces = true;
    let connected_zigzags = if roof { false } else { config.connect_zigzags };
    let skip_some_zags = if roof { false } else { config.skip_some_zags };
    let zag_skip_count = if roof { 0 } else { config.zag_skip_count };
    let pocket_size: Coord = 0;
    let angles = if roof { &config.support_roof_angles } else { &config.support_infill_angles };
    let mut toolpaths: Vec<VariableWidthLines> = Vec::new();
    let z = config.get_actual_z(layer_idx);
    let divisor = angles.len() as i64;
    let index = (((layer_idx % divisor) + divisor) % divisor) as usize;
    let fill_angle = angles[index];
    let mut roof_computation = crate::fff_gcode_writer::make_infill(
        pattern,
        zig_zaggify_infill,
        connect_polygons,
        area.clone(),
        if roof { config.support_roof_line_width } else { config.support_line_width },
        support_infill_distance,
        support_roof_overlap,
        infill_multiplier,
        fill_angle,
        z,
        support_shift as Coord,
        config.maximum_resolution,
        config.maximum_deviation,
        wall_line_count,
        narrow_area_width,
        infill_origin,
        skip_stitching,
        fill_gaps,
        connected_zigzags,
        use_endpieces,
        skip_some_zags,
        zag_skip_count,
        pocket_size,
    );
    let mut areas = Shape::default();
    let mut lines = OpenLinesSet::default();
    roof_computation.generate(
        &mut toolpaths,
        &mut areas,
        &mut lines,
        config.settings,
        layer_idx as i32,
        SectionType::SUPPORT,
        cross_fill_provider,
        None,
        None,
        &Shape::default(),
    );
    lines.push_all_owned(to_polylines(&areas));
    lines.push_all_owned(to_polylines_vwl(&toolpaths));
    lines
}

/// `safeUnion(first, second = Shape())`.
pub fn safe_union(first: &Shape, second: &Shape) -> Shape {
    let was_empty = first.is_empty() && second.is_empty();
    let result = first.union_polygons_other(second);
    if result.is_empty() && !was_empty {
        crate::log_warn!("Caught an area destroying union, enlarging areas a bit.");
        return to_polylines(first).offset_default(2).union_polygons_other(&to_polylines(second).offset_default(2));
    }
    result
}

/// `safeUnion(first)`.
pub fn safe_union1(first: &Shape) -> Shape {
    safe_union(first, &Shape::default())
}

/// `safeOffsetInc`.
#[allow(clippy::too_many_arguments)]
pub fn safe_offset_inc(
    me: &Shape,
    distance: Coord,
    collision: &Shape,
    safe_step_size: Coord,
    last_step_offset_without_check: Coord,
    min_amount_offset: usize,
    min_offset_per_step: Coord,
    simplifier: Option<&Simplify>,
) -> Shape {
    let mut do_final_difference = last_step_offset_without_check == 0;
    let mut ret = safe_union1(me);
    if distance == 0 {
        return (if do_final_difference { ret.difference(collision) } else { ret }).union_polygons();
    }
    if safe_step_size < 0 || last_step_offset_without_check < 0 {
        crate::log_error!("Offset increase got invalid parameter!");
        return (if do_final_difference { ret.difference(collision) } else { ret }).union_polygons();
    }

    let mut step_size: Coord = cmax(min_offset_per_step, safe_step_size);
    // size_t arithmetic mixed with coord_t (converted to unsigned).
    let du = distance as u64;
    let mut steps: u64 = if distance > last_step_offset_without_check { ((distance - last_step_offset_without_check) / step_size) as u64 } else { 0 };
    if du.wrapping_sub(steps.wrapping_mul(step_size as u64)) > last_step_offset_without_check as u64 {
        if (steps + 1).wrapping_mul(step_size as u64) <= du {
            steps += 1;
        } else {
            do_final_difference = true;
        }
    }

    let extra = (distance < last_step_offset_without_check || distance % step_size != 0) as u64;
    if steps + extra < min_amount_offset as u64 && min_amount_offset > 1 {
        // `distance / min_amount_offset` is an unsigned division.
        step_size = (du / min_amount_offset as u64) as Coord;
        if step_size >= safe_step_size {
            step_size = safe_step_size;
            steps = min_amount_offset as u64;
        } else {
            steps = (distance / step_size) as u64;
        }
    }

    for i in 0..steps {
        ret = ret.offset_join(step_size, JoinType::Round).difference(collision).union_polygons();
        if i % 10 == 7 {
            if let Some(s) = simplifier {
                ret = s.shape(&ret);
            }
        }
    }
    ret = ret.offset_join(du.wrapping_sub(steps.wrapping_mul(step_size as u64)) as Coord, JoinType::Round);
    if let Some(s) = simplifier {
        ret = s.shape(&ret);
    }
    if do_final_difference {
        ret = ret.difference(collision);
    }
    ret.union_polygons()
}

/// `movePointsOutside`.
pub fn move_points_outside(polylines: &OpenLinesSet, area: &Shape, max_allowed_distance: Coord) -> OpenLinesSet {
    let mut result = OpenLinesSet::default();
    for line in polylines.iter() {
        let mut next_line = OpenPolyline::default();
        for &p in line.iter() {
            if area.inside(p, false) {
                let mut next_outside = p;
                PolygonUtils::move_outside(area, &mut next_outside, 0, i64::MAX);
                if v_size2(p - next_outside) < max_allowed_distance * max_allowed_distance {
                    next_line.push(next_outside);
                } else {
                    let max_partial_move_proportion = max_allowed_distance as f64 / v_size(p - next_outside) as f64;
                    next_outside = p + scale_f64(next_outside - p, max_partial_move_proportion);
                    next_line.push(next_outside);
                }
            } else {
                next_line.push(p);
            }
        }
        if !next_line.is_empty() {
            result.push(next_line);
        }
    }
    result
}
