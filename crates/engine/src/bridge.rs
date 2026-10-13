//! `bridge/bridge.cpp` and the helpers in `bridge/*`: detection of bridging areas and angles, and
//! the splitting of wall segments into bridging sub-segments.

use crate::geometry::matrix::PointMatrix;
use crate::geometry::mixed_lines_set::{AnyPolyline, MixedLinesSet};
use crate::geometry::point2ll::{lerp, pt, v_size, Point2LL};
use crate::geometry::{ClosedLine, OpenLinesSet, Polygon, Polyline, Shape};
use crate::layer_plan::LayerPlan;
use crate::path_adapter::PathAdapter;
use crate::settings::enums::EFillMethod;
use crate::settings::types::{AngleDegrees, AngleRadians, Ratio};
use crate::slice_data_storage::{SliceDataStorage, SliceMeshStorage, SupportLayer};
use crate::utils::aabb::AABB;
use crate::utils::coord::{fuzzy_equal, fuzzy_is_greater_or_equal, fuzzy_is_lesser_or_equal, fuzzy_not_equal, Coord, EPSILON};
use crate::utils::linear_alg_2d as la;
use crate::utils::math::{inverse_lerp, is_zero};

/// `sign(coord_t)`: `std::signbit(value) ? -1 : 1`.
#[inline]
fn sign_c(value: Coord) -> i8 {
    if value < 0 {
        -1
    } else {
        1
    }
}

/// `SegmentOverlappingType`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SegmentOverlappingType {
    Full,
    Bottom,
    Top,
    Middle,
}

/// `TransformedSegment`
#[derive(Clone, Copy, Debug, Default)]
struct TransformedSegment {
    start: Point2LL,
    end: Point2LL,
    min_y: Coord,
    max_y: Coord,
}

/// `SegmentOverlapping`
#[derive(Clone, Copy, Debug)]
struct SegmentOverlapping {
    type_: SegmentOverlappingType,
    other_overlapping_part: TransformedSegment,
}

impl TransformedSegment {
    fn new(transformed_start: Point2LL, transformed_end: Point2LL) -> Self {
        let mut s = TransformedSegment { start: transformed_start, end: transformed_end, min_y: 0, max_y: 0 };
        s.update_min_max();
        s
    }

    fn with_matrix(start: Point2LL, end: Point2LL, matrix: &PointMatrix) -> Self {
        Self::new(matrix.apply(start), matrix.apply(end))
    }

    fn set_start(&mut self, p: Point2LL) {
        self.start = p;
        self.update_min_max();
    }

    fn set_end(&mut self, p: Point2LL) {
        self.end = p;
        self.update_min_max();
    }

    fn update_min_max(&mut self) {
        // std::minmax(a, b) returns (b, a) only if b < a
        if self.end.y < self.start.y {
            self.min_y = self.end.y;
            self.max_y = self.start.y;
        } else {
            self.min_y = self.start.y;
            self.max_y = self.end.y;
        }
    }

    fn crop_top(&mut self, new_max_y: Coord) {
        let x = la::line_horizontal_line_intersection(self.start, self.end, new_max_y).unwrap_or(0);
        self.set_end(pt(x, new_max_y));
    }

    fn crop_bottom(&mut self, new_min_y: Coord) {
        let x = la::line_horizontal_line_intersection(self.start, self.end, new_min_y).unwrap_or(0);
        self.set_start(pt(x, new_min_y));
    }

    fn make_non_intersecting_overlapping(overlap_top: bool, overlap_bottom: bool) -> SegmentOverlappingType {
        if overlap_top && overlap_bottom {
            SegmentOverlappingType::Full
        } else if overlap_bottom {
            SegmentOverlappingType::Bottom
        } else if overlap_top {
            SegmentOverlappingType::Top
        } else {
            SegmentOverlappingType::Middle
        }
    }

    fn calculate_overlapping(&self, other: &TransformedSegment, expand_direction: i8) -> Option<SegmentOverlapping> {
        if fuzzy_is_greater_or_equal(other.min_y, self.max_y) || fuzzy_is_lesser_or_equal(other.max_y, self.min_y) {
            return None;
        }

        let overlapping = SegmentOverlappingData::new(self.min_y, self.max_y, other.min_y, other.max_y, Some((self.start, self.end)), other.start, other.end);

        if fuzzy_equal(overlapping.this_x_min, overlapping.other_x_min) && fuzzy_equal(overlapping.this_x_max, overlapping.other_x_max) {
            return None;
        }

        let mut sign_min = sign_c(overlapping.other_x_min - overlapping.this_x_min);
        let mut sign_max = sign_c(overlapping.other_x_max - overlapping.this_x_max);
        let overlap_top = overlapping.y_max == self.max_y;
        let overlap_bottom = overlapping.y_min == self.min_y;

        let mut line_part = overlapping.make_other_overlapping_part();

        if sign_min != sign_max {
            if let Some((intersection_segment, _)) = la::segment_segment_intersection(self.start, self.end, other.start, other.end) {
                let intersection = lerp(self.start, self.end, intersection_segment as f64);
                if intersection.y >= overlapping.y_max - EPSILON {
                    sign_max = sign_min;
                } else if intersection.y <= overlapping.y_min + EPSILON {
                    sign_min = sign_max;
                } else {
                    let type_;
                    if sign_max == expand_direction {
                        type_ = if overlap_top { SegmentOverlappingType::Top } else { SegmentOverlappingType::Middle };
                        line_part.set_start(intersection);
                    } else {
                        type_ = if overlap_bottom { SegmentOverlappingType::Bottom } else { SegmentOverlappingType::Middle };
                        line_part.set_end(intersection);
                    }
                    return Some(SegmentOverlapping { type_, other_overlapping_part: line_part });
                }
            }
        }
        let _ = sign_max;

        if sign_min != expand_direction {
            None
        } else {
            Some(SegmentOverlapping { type_: Self::make_non_intersecting_overlapping(overlap_top, overlap_bottom), other_overlapping_part: line_part })
        }
    }
}

/// `SegmentOverlappingData`
struct SegmentOverlappingData {
    y_min: Coord,
    y_max: Coord,
    this_x_min: Coord,
    this_x_max: Coord,
    other_x_min: Coord,
    other_x_max: Coord,
}

impl SegmentOverlappingData {
    fn new(
        this_min_y: Coord,
        this_max_y: Coord,
        other_min_y: Coord,
        other_max_y: Coord,
        this_segment: Option<(Point2LL, Point2LL)>,
        other_start: Point2LL,
        other_end: Point2LL,
    ) -> Self {
        let y_min = this_min_y.max(other_min_y);
        let y_max = if other_max_y < this_max_y { other_max_y } else { this_max_y };
        let (this_x_min, this_x_max) = match this_segment {
            Some((s, e)) => (
                la::line_horizontal_line_intersection(s, e, y_min).expect("optional value"),
                la::line_horizontal_line_intersection(s, e, y_max).expect("optional value"),
            ),
            None => (0, 0),
        };
        SegmentOverlappingData {
            y_min,
            y_max,
            this_x_min,
            this_x_max,
            other_x_min: la::line_horizontal_line_intersection(other_start, other_end, y_min).expect("optional value"),
            other_x_max: la::line_horizontal_line_intersection(other_start, other_end, y_max).expect("optional value"),
        }
    }

    fn make_other_overlapping_part(&self) -> TransformedSegment {
        TransformedSegment::new(pt(self.other_x_min, self.y_min), pt(self.other_x_max, self.y_max))
    }
}

/// `TransformedShape`
#[derive(Clone, Debug)]
struct TransformedShape {
    matrix: PointMatrix,
    segments: Vec<TransformedSegment>,
    min_y: Coord,
    max_y: Coord,
}

impl Default for TransformedShape {
    fn default() -> Self {
        TransformedShape { matrix: PointMatrix::default(), segments: Vec::new(), min_y: Coord::MAX, max_y: Coord::MIN }
    }
}

impl TransformedShape {
    fn new(matrix: PointMatrix) -> Self {
        TransformedShape { matrix, ..Default::default() }
    }

    fn from_shape(shape: &Shape, matrix: PointMatrix) -> Self {
        let mut s = Self::new(matrix);
        s.add_shape(shape, false);
        s
    }

    fn from_polygon(polygon: &Polygon, matrix: PointMatrix) -> Self {
        let mut s = Self::new(matrix);
        s.add_polygon(polygon, false);
        s
    }

    fn add_shape(&mut self, shape: &Shape, filter_out_horizontal: bool) {
        for polygon in shape.iter() {
            self.add_polygon(polygon, filter_out_horizontal);
        }
    }

    fn add_polygon(&mut self, polygon: &Polygon, filter_out_horizontal: bool) {
        for (s, e) in polygon.segments() {
            self.add_segment(s, e, filter_out_horizontal);
        }
    }

    fn add_segment(&mut self, start: Point2LL, end: Point2LL, filter_out_horizontal: bool) {
        let segment = TransformedSegment::with_matrix(start, end, &self.matrix);
        if filter_out_horizontal && fuzzy_equal(segment.min_y, segment.max_y) {
            return;
        }
        self.min_y = self.min_y.min(segment.min_y);
        self.max_y = self.max_y.max(segment.max_y);
        self.segments.push(segment);
    }
}

/// `ExpansionRange`
#[derive(Clone, Copy, Debug)]
struct ExpansionRange {
    /// Either the projected segment, or a plain (min_y, max_y) range.
    segment: TransformedSegment,
    range_min_y: Coord,
    range_max_y: Coord,
    /// Index of the supporting segment in `infill_lines_below`, or `None` for the segment being expanded itself.
    supporting_segment: Option<usize>,
    is_projected: bool,
}

impl ExpansionRange {
    fn projected(segment: TransformedSegment, supporting_segment: usize) -> Self {
        ExpansionRange { segment, range_min_y: 0, range_max_y: 0, supporting_segment: Some(supporting_segment), is_projected: true }
    }

    fn range(min_y: Coord, max_y: Coord, expanded_segment: Option<usize>) -> Self {
        ExpansionRange { segment: TransformedSegment::default(), range_min_y: min_y, range_max_y: max_y, supporting_segment: expanded_segment, is_projected: false }
    }

    fn min_y(&self) -> Coord {
        if self.is_projected {
            self.segment.min_y
        } else {
            self.range_min_y
        }
    }

    fn max_y(&self) -> Coord {
        if self.is_projected {
            self.segment.max_y
        } else {
            self.range_max_y
        }
    }

    fn crop_top(&mut self, new_max_y: Coord) {
        if self.is_projected {
            self.segment.crop_top(new_max_y);
        } else {
            self.range_max_y = new_max_y;
        }
    }

    fn crop_bottom(&mut self, new_min_y: Coord) {
        if self.is_projected {
            self.segment.crop_bottom(new_min_y);
        } else {
            self.range_min_y = new_min_y;
        }
    }

    fn is_valid(&self) -> bool {
        fuzzy_not_equal(self.max_y(), self.min_y())
    }

    fn set_projected_end(&mut self, end: Point2LL) {
        self.segment.set_end(end);
    }

    fn calculate_overlapping(&self, other: &TransformedSegment, expand_direction: i8) -> Option<SegmentOverlapping> {
        if self.is_projected {
            return self.segment.calculate_overlapping(other, expand_direction);
        }

        if fuzzy_is_greater_or_equal(other.min_y, self.max_y()) || fuzzy_is_lesser_or_equal(other.max_y, self.min_y()) {
            return None;
        }

        let overlapping = SegmentOverlappingData::new(self.range_min_y, self.range_max_y, other.min_y, other.max_y, None, other.start, other.end);

        let overlap_top = other.max_y == self.range_max_y;
        let overlap_bottom = other.min_y == self.range_min_y;

        Some(SegmentOverlapping {
            type_: TransformedSegment::make_non_intersecting_overlapping(overlap_top, overlap_bottom),
            other_overlapping_part: overlapping.make_other_overlapping_part(),
        })
    }
}

/// Calculates all the intersections between a horizontal line and the given transformed shape.
fn shape_line_intersections(line_y: Coord, transformed_shape: &TransformedShape) -> Vec<Coord> {
    let mut intersections = Vec::new();
    for transformed_segment in &transformed_shape.segments {
        if transformed_segment.min_y > line_y || transformed_segment.max_y < line_y {
            continue;
        }
        if let Some(intersection) = la::line_horizontal_line_intersection(transformed_segment.start, transformed_segment.end, line_y) {
            intersections.push(intersection);
        }
    }
    intersections
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BridgeStatus {
    Outside,
    Hanging,
    Anchored,
    Supported,
}

/// Evaluates a potential bridging line to see if it can actually bridge between two supported regions.
fn evaluate_bridge_line(line_y: Coord, transformed_skin_area: &TransformedShape, transformed_supported_area: &TransformedShape) -> Coord {
    let mut skin_outline_intersections = shape_line_intersections(line_y, transformed_skin_area);
    if skin_outline_intersections.len() < 2 {
        return 0;
    }
    skin_outline_intersections.sort(); // stable sort of integers

    let mut supported_regions_intersections = shape_line_intersections(line_y, transformed_supported_area);
    supported_regions_intersections.sort();

    let mut skin_idx = 0usize;
    let mut supported_idx = 0usize;

    let mut inside_skin_area = false;
    let mut inside_supported_area = false;
    let mut last_position: Coord = 0;
    let mut segment_score: Coord = 0;
    let mut bridge_status = BridgeStatus::Outside;
    while skin_idx < skin_outline_intersections.len() || supported_idx < supported_regions_intersections.len() {
        let skin_empty = skin_idx >= skin_outline_intersections.len();
        let supported_empty = supported_idx >= supported_regions_intersections.len();
        let mut next_intersection_is_skin_area = false;
        let mut next_intersection_is_supported_area = false;
        if skin_empty {
            next_intersection_is_supported_area = true;
        } else if supported_empty {
            next_intersection_is_skin_area = true;
        } else {
            let next_intersection_skin_area = skin_outline_intersections[skin_idx] as f64;
            let next_intersection_supported_area = supported_regions_intersections[supported_idx] as f64;

            if is_zero(next_intersection_skin_area - next_intersection_supported_area) {
                next_intersection_is_skin_area = true;
                next_intersection_is_supported_area = true;
            } else if next_intersection_skin_area <= next_intersection_supported_area {
                next_intersection_is_skin_area = true;
                if inside_skin_area && inside_supported_area {
                    next_intersection_is_supported_area = true;
                }
            } else {
                next_intersection_is_supported_area = true;
                if !inside_supported_area && !inside_skin_area {
                    next_intersection_is_skin_area = true;
                }
            }
        }

        let mut next_inside_skin_area = inside_skin_area;
        let mut next_inside_supported_area = inside_supported_area;
        let mut next_intersection: Coord = 0;
        if next_intersection_is_skin_area {
            next_intersection = skin_outline_intersections[skin_idx];
            skin_idx += 1;
            next_inside_skin_area = !next_inside_skin_area;
        }
        if next_intersection_is_supported_area {
            next_intersection = supported_regions_intersections[supported_idx];
            supported_idx += 1;
            next_inside_supported_area = !next_inside_supported_area;
        }

        let leaving_skin = next_intersection_is_skin_area && !next_inside_skin_area;
        let reaching_supported = next_intersection_is_supported_area && next_inside_supported_area;
        let mut add_segment_score_weight = 0.0;

        match bridge_status {
            BridgeStatus::Outside => {
                bridge_status = if reaching_supported { BridgeStatus::Supported } else { BridgeStatus::Hanging };
            }
            BridgeStatus::Supported => {
                bridge_status = if leaving_skin { BridgeStatus::Outside } else { BridgeStatus::Anchored };
                add_segment_score_weight = -0.1;
            }
            BridgeStatus::Hanging => {
                add_segment_score_weight = -1.0;
                bridge_status = if reaching_supported { BridgeStatus::Supported } else { BridgeStatus::Outside };
            }
            BridgeStatus::Anchored => {
                if reaching_supported {
                    add_segment_score_weight = 1.0;
                    bridge_status = BridgeStatus::Supported;
                } else if leaving_skin {
                    add_segment_score_weight = -1.0;
                    bridge_status = BridgeStatus::Outside;
                }
            }
        }

        if add_segment_score_weight != 0.0 {
            let segment_length = next_intersection - last_position;
            segment_score += cppstd::llrint(segment_length as f64 * add_segment_score_weight);
        }

        last_position = next_intersection;
        inside_skin_area = next_inside_skin_area;
        inside_supported_area = next_inside_supported_area;
    }

    segment_score
}

/// Evaluate the bridging lines scoring for the given angle.
fn evaluate_bridge_lines(skin_outline: &Shape, supported_regions: &Shape, line_width: Coord, angle: AngleDegrees) -> Coord {
    let matrix = PointMatrix::from_angle_degrees(angle);
    let transformed_skin_area = TransformedShape::from_shape(skin_outline, matrix);
    let transformed_supported_area = TransformedShape::from_shape(supported_regions, matrix);

    if transformed_skin_area.min_y >= transformed_skin_area.max_y || transformed_supported_area.min_y >= transformed_supported_area.max_y {
        return Coord::MIN;
    }

    let bridge_lines_count = ((transformed_skin_area.max_y - transformed_skin_area.min_y) / line_width) as usize;
    if bridge_lines_count == 0 {
        return Coord::MIN;
    }

    let line_min = (transformed_skin_area.min_y as f64 + line_width as f64 * 0.5) as Coord;

    let mut line_score: Coord = 0;
    let empty_transformed_shape = TransformedShape::default();
    for i in 0..bridge_lines_count {
        let line_y = line_min.wrapping_add((i as Coord).wrapping_mul(line_width));
        let has_supports = line_y >= transformed_supported_area.min_y && line_y <= transformed_supported_area.max_y;
        line_score += evaluate_bridge_line(line_y, &transformed_skin_area, if has_supports { &transformed_supported_area } else { &empty_transformed_shape });
    }

    line_score
}

/// Gets the proper angle when bridging over infill.
fn bridge_over_infill_angle(mesh: &SliceMeshStorage, layer_nr: u32) -> AngleDegrees {
    if layer_nr == 0 {
        return AngleDegrees::new(0.0);
    }

    let infill_angles = &mesh.infill_angles;
    let infill_angle_below = infill_angles[((layer_nr - 1) as usize) % infill_angles.len()];
    let infill_pattern: EFillMethod = mesh.settings().get_enum("infill_pattern");

    match infill_pattern {
        EFillMethod::CROSS | EFillMethod::CROSS_3D => AngleDegrees::new(22.5),
        EFillMethod::GYROID | EFillMethod::CONCENTRIC | EFillMethod::LIGHTNING | EFillMethod::PLUGIN | EFillMethod::NONE => AngleDegrees::new(45.0),
        EFillMethod::CUBICSUBDIV => infill_angle_below + AngleDegrees::new(45.0),
        EFillMethod::TRIANGLES
        | EFillMethod::LINES
        | EFillMethod::TRIHEXAGON
        | EFillMethod::CUBIC
        | EFillMethod::ZIG_ZAG
        | EFillMethod::HONEYCOMB
        | EFillMethod::OCTAGON => infill_angle_below + AngleDegrees::new(90.0),
        EFillMethod::QUARTER_CUBIC | EFillMethod::TETRAHEDRAL | EFillMethod::GRID => infill_angle_below,
    }
}

/// `bridgeAngle`: computes the angle that the skin lines should take when bridging, or `None` when
/// the skin is not a bridge.
pub fn bridge_angle(
    mesh: &SliceMeshStorage,
    skin_outline: &Shape,
    storage: &SliceDataStorage,
    layer_nr: u32,
    bridge_layer: u32,
    support_layer: Option<&SupportLayer>,
    supported_regions: &mut Shape,
) -> Option<AngleDegrees> {
    let settings = mesh.settings();
    let bridge_settings_enabled = settings.get_bool("bridge_settings_enabled");
    if !bridge_settings_enabled {
        return None;
    }

    let boundary_box = AABB::from_shape(skin_outline);

    let line_width = settings.get_coord("skin_line_width");

    let mut islands = Shape::default();
    let mut prev_layer_outline = Shape::default();
    let mut prev_layer_infill = Shape::default();

    for mesh_ptr in storage.meshes.iter() {
        let mesh = &**mesh_ptr;
        if mesh.is_printed() {
            let infill_line_distance = mesh.settings().get_coord("infill_line_distance");
            let part_has_sparse_infill = infill_line_distance == 0;

            for prev_layer_part in mesh.layers[(layer_nr - bridge_layer) as usize].parts.iter() {
                let prev_layer_part_infill = prev_layer_part.get_own_infill_area().clone();
                prev_layer_infill = prev_layer_infill.union_polygons_other(&prev_layer_part_infill);

                let mut solid_below = prev_layer_part.outline.0.clone();
                if bridge_layer == 1 && part_has_sparse_infill {
                    solid_below = solid_below.difference(&prev_layer_part_infill);
                }
                prev_layer_outline.push_all(&solid_below);

                if !boundary_box.hit(&prev_layer_part.boundary_box) {
                    continue;
                }

                islands.push_all(&skin_outline.intersection(&solid_below));
            }
        }
    }
    *supported_regions = islands;

    if let Some(support_layer) = support_layer {
        if !support_layer.support_roof.is_empty() {
            let support_roof_bb = AABB::from_shape(&support_layer.support_roof);
            if boundary_box.hit(&support_roof_bb) {
                prev_layer_outline.push_all(&support_layer.support_roof);

                let supported_skin = skin_outline.intersection(&support_layer.support_roof);
                if !supported_skin.is_empty() {
                    supported_regions.push_all(&supported_skin);
                }
            }
        } else {
            for support_part in support_layer.support_infill_parts.iter() {
                let support_part_bb = AABB::from_shape(support_part.get_infill_area());
                if boundary_box.hit(&support_part_bb) {
                    prev_layer_outline.push_all(support_part.get_infill_area());

                    let supported_skin = skin_outline.intersection(support_part.get_infill_area());
                    if !supported_skin.is_empty() {
                        supported_regions.push_all(&supported_skin);
                    }
                }
            }
        }
    }

    let support_threshold = settings.get_ratio("bridge_skin_support_threshold");
    if support_threshold == 0.0 || (supported_regions.area() / (skin_outline.area() + 1.0)) >= support_threshold {
        return None;
    }

    prev_layer_infill = skin_outline.intersection(&prev_layer_infill);
    let infill_ratio = prev_layer_infill.area() / (skin_outline.area() + 1.0);
    if infill_ratio > 0.5 {
        return Some(bridge_over_infill_angle(mesh, layer_nr));
    }

    let mut bridge_area_lines = OpenLinesSet::default();
    for path in skin_outline.iter() {
        bridge_area_lines.push(ClosedLine::to_pseudo_open_polyline(path));
    }
    bridge_area_lines = bridge_area_lines.difference(&supported_regions.offset_default(10));

    if bridge_area_lines.is_empty() {
        let mut skin_outline_ = skin_outline.clone();
        skin_outline_.make_convex();
        let (aabb, mut angle) = AABB::minimum_area_oriented_bounding_box(&skin_outline_);
        if aabb.height() > aabb.width() {
            angle += AngleRadians::from(AngleDegrees::new(90.0));
        }
        // `-AngleDegrees(angle)`: AngleDegrees converts to double, which is negated and converted back.
        return Some(AngleDegrees::new(-AngleDegrees::from(angle).value()));
    }

    let mut best_score = Coord::MIN;
    let mut best_angle: Option<AngleDegrees> = None;
    let mut angle = AngleDegrees::new(0.0);
    while angle.value() < 180.0 {
        let score = evaluate_bridge_lines(skin_outline, supported_regions, line_width, angle);
        if score > best_score {
            best_score = score;
            best_angle = Some(angle + AngleDegrees::new(90.0));
        }
        angle += AngleDegrees::new(1.0);
    }

    best_angle
}

/// Make the expanded ranges for the given segment.
fn make_expanded_ranges(segment: &TransformedSegment, infill_lines_below: &[TransformedSegment], expand_direction: i8) -> Vec<ExpansionRange> {
    let mut expanded_ranges: Vec<ExpansionRange> = vec![ExpansionRange::range(segment.min_y, segment.max_y, None)];

    if !expanded_ranges[0].is_valid() {
        return expanded_ranges;
    }

    for (infill_line_idx, infill_line_below) in infill_lines_below.iter().enumerate() {
        let overlapping = match segment.calculate_overlapping(infill_line_below, expand_direction) {
            Some(o) => o,
            None => continue,
        };

        let line_part = overlapping.other_overlapping_part;

        let mut new_expanded_ranges: Vec<ExpansionRange> = Vec::new();
        let mut replacing_range: Option<ExpansionRange> = None;

        let commit_replacing_range = |replacing_range: &mut Option<ExpansionRange>, new_expanded_ranges: &mut Vec<ExpansionRange>| {
            if let Some(r) = replacing_range.take() {
                if r.is_valid() {
                    new_expanded_ranges.push(r);
                }
            }
        };

        for expanded_range in expanded_ranges.iter() {
            let range_overlapping = match expanded_range.calculate_overlapping(&line_part, -expand_direction) {
                Some(o) => o,
                None => {
                    commit_replacing_range(&mut replacing_range, &mut new_expanded_ranges);
                    new_expanded_ranges.push(*expanded_range);
                    continue;
                }
            };

            if range_overlapping.type_ != SegmentOverlappingType::Bottom && range_overlapping.type_ != SegmentOverlappingType::Full {
                commit_replacing_range(&mut replacing_range, &mut new_expanded_ranges);

                let mut cropped_range_bottom = *expanded_range;
                cropped_range_bottom.crop_top(range_overlapping.other_overlapping_part.min_y);
                if cropped_range_bottom.is_valid() {
                    new_expanded_ranges.push(cropped_range_bottom);
                }
            }

            match replacing_range.as_mut() {
                None => {
                    replacing_range = Some(ExpansionRange::projected(range_overlapping.other_overlapping_part, infill_line_idx));
                }
                Some(r) => {
                    r.set_projected_end(range_overlapping.other_overlapping_part.end);
                }
            }

            if range_overlapping.type_ != SegmentOverlappingType::Top && range_overlapping.type_ != SegmentOverlappingType::Full {
                commit_replacing_range(&mut replacing_range, &mut new_expanded_ranges);

                // Note: in C++ the range was moved-from above (a trivially copyable type, so it is unchanged).
                let mut cropped_range_top = *expanded_range;
                cropped_range_top.crop_bottom(range_overlapping.other_overlapping_part.max_y);
                if cropped_range_top.is_valid() {
                    new_expanded_ranges.push(cropped_range_top);
                }
            }
        }

        commit_replacing_range(&mut replacing_range, &mut new_expanded_ranges);

        if !new_expanded_ranges.is_empty() {
            expanded_ranges = new_expanded_ranges;
        }
    }

    expanded_ranges
}

/// Identifies the supporting segment of a range: `None` is the segment itself (which is never
/// `nullptr`), `Some(i)` an infill line below.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Supporting {
    Nothing,
    Segment(Option<usize>, u64),
}

/// Update the expanded polygon area according to the given expansion ranges.
fn update_expanded_polygon(
    segment: &TransformedSegment,
    segment_id: u64,
    expanded_ranges: &[ExpansionRange],
    expand_direction: i8,
    expanded_polygon: &mut Polygon,
    current_supporting_infill_line: &mut Supporting,
    current_supporting_line_data: &mut TransformedSegment,
    infill_lines_below: &[TransformedSegment],
) {
    for expanded_range in expanded_ranges {
        // Pointer identity: an infill line below is identified by its index; the expanded segment itself by its id.
        let range_supporting = match expanded_range.supporting_segment {
            Some(i) => Supporting::Segment(Some(i), 0),
            None => Supporting::Segment(None, segment_id),
        };
        if range_supporting == *current_supporting_infill_line {
            continue;
        }

        if *current_supporting_infill_line == Supporting::Nothing {
            if !expanded_range.is_projected {
                expanded_polygon.push(segment.start);
            } else {
                expanded_polygon.push(if expand_direction > 0 { expanded_range.segment.start } else { expanded_range.segment.end });
            }
        } else {
            let next_start_position = if !expanded_range.is_projected {
                let position_switch_y = if expand_direction > 0 { expanded_range.min_y() } else { expanded_range.max_y() };
                pt(la::line_horizontal_line_intersection(segment.start, segment.end, position_switch_y).unwrap_or(0), position_switch_y)
            } else if expand_direction > 0 {
                expanded_range.segment.start
            } else {
                expanded_range.segment.end
            };

            let cur = *current_supporting_line_data;
            expanded_polygon.push_back_if_forming_segment(pt(
                la::line_horizontal_line_intersection(cur.start, cur.end, next_start_position.y).unwrap_or(0),
                next_start_position.y,
            ));

            expanded_polygon.push_back_if_forming_segment(next_start_position);
        }

        *current_supporting_infill_line = range_supporting;
        *current_supporting_line_data = match expanded_range.supporting_segment {
            Some(i) => infill_lines_below[i],
            None => *segment,
        };
    }
}

/// Expands a segment of a polygon so that bridging will always have supporting infill lines below.
fn expand_segment(
    segment: &TransformedSegment,
    segment_id: u64,
    infill_lines_below: &[TransformedSegment],
    expanded_polygon: &mut Polygon,
    current_supporting_infill_line: &mut Supporting,
    current_supporting_line_data: &mut TransformedSegment,
) {
    let expand_direction = sign_c(segment.end.y - segment.start.y);

    let mut expanded_ranges = make_expanded_ranges(segment, infill_lines_below, expand_direction);

    if expand_direction < 0 {
        expanded_ranges.reverse();
    }

    update_expanded_polygon(
        segment,
        segment_id,
        &expanded_ranges,
        expand_direction,
        expanded_polygon,
        current_supporting_infill_line,
        current_supporting_line_data,
        infill_lines_below,
    );
}

/// `makeBridgeOverInfillPrintable`
pub fn make_bridge_over_infill_printable(
    infill_contour: &Shape,
    infill_below_skin_area: &Shape,
    mesh: &SliceMeshStorage,
    completed_layer_plan_below: &LayerPlan,
    mesh_idx: usize,
    layer_nr: u32,
) -> (Shape, AngleDegrees) {
    if layer_nr == 0 || infill_below_skin_area.is_empty() {
        return (Shape::default(), AngleDegrees::new(0.0));
    }

    let mut unsupported_infill_below_skin_area = infill_below_skin_area.clone();
    for part_below in mesh.layers[(layer_nr - 1) as usize].parts.iter() {
        for skin_part_below in part_below.skin_parts.iter() {
            unsupported_infill_below_skin_area = unsupported_infill_below_skin_area.difference(&skin_part_below.outline.0);
        }
    }

    if unsupported_infill_below_skin_area.is_empty() {
        return (Shape::default(), AngleDegrees::new(0.0));
    }

    let bridge_angle = bridge_over_infill_angle(mesh, layer_nr);

    let expansion_angle = bridge_angle + AngleDegrees::new(90.0);
    let infill_lines_below: MixedLinesSet = completed_layer_plan_below.get_generated_infill_lines(mesh_idx);

    let matrix = PointMatrix::from_angle_degrees(expansion_angle);
    let mut filtered_infill_lines_below = TransformedShape::new(matrix);

    let filter_out_horizontal = true;
    for infill_line_below in infill_lines_below.iter() {
        match &**infill_line_below {
            AnyPolyline::Open(l) => {
                for (s, e) in l.segments() {
                    filtered_infill_lines_below.add_segment(s, e, filter_out_horizontal);
                }
            }
            AnyPolyline::Closed(l) => {
                for (s, e) in l.segments() {
                    filtered_infill_lines_below.add_segment(s, e, filter_out_horizontal);
                }
            }
            AnyPolyline::Polygon(l) => {
                for (s, e) in l.segments() {
                    filtered_infill_lines_below.add_segment(s, e, filter_out_horizontal);
                }
            }
        }
    }

    filtered_infill_lines_below.add_shape(infill_contour, filter_out_horizontal);

    let mut transformed_expanded_infill_below_skin_area = Shape::default();
    let mut segment_id: u64 = 1;
    for infill_below_skin_polygon in unsupported_infill_below_skin_area.iter() {
        let transformed_infill_below_skin_polygon = TransformedShape::from_polygon(infill_below_skin_polygon, matrix);

        let mut expanded_polygon = Polygon::default();
        let mut current_supporting_infill_line = Supporting::Nothing;
        let mut current_supporting_line_data = TransformedSegment::default();
        for transformed_infill_below_skin_segment in transformed_infill_below_skin_polygon.segments.iter() {
            expand_segment(
                transformed_infill_below_skin_segment,
                segment_id,
                &filtered_infill_lines_below.segments,
                &mut expanded_polygon,
                &mut current_supporting_infill_line,
                &mut current_supporting_line_data,
            );
            segment_id += 1;
        }
        transformed_expanded_infill_below_skin_area.push(expanded_polygon);
    }

    transformed_expanded_infill_below_skin_area.apply_matrix(&matrix.inverse());

    transformed_expanded_infill_below_skin_area =
        transformed_expanded_infill_below_skin_area.offset_default(EPSILON).offset_default(-EPSILON).intersection(infill_contour);

    (transformed_expanded_infill_below_skin_area, bridge_angle)
}

/// Calculates whether the given tip of an extrusion segment is properly anchored for bridging.
#[allow(clippy::too_many_arguments)]
fn is_wall_tip_anchored(
    bridge_mask: &Shape,
    wall: &PathAdapter,
    tip_index: usize,
    tip: Point2LL,
    other_tip: Point2LL,
    tip_inside_bridge_mask: bool,
    segment_direction: isize,
    min_anchoring_length: Coord,
    intersections: &[f32],
) -> bool {
    let mut remaining_anchoring_length = min_anchoring_length;

    if !tip_inside_bridge_mask {
        let closest_intersection: f32 = if segment_direction > 0 { intersections[0] } else { (1.0 - *intersections.last().unwrap() as f64) as f32 };
        let segment_length = v_size(other_tip - tip);
        let inside_anchoring_length = (closest_intersection * segment_length as f32) as Coord;
        remaining_anchoring_length -= inside_anchoring_length;
    }

    if remaining_anchoring_length > 0 {
        let max_outside_search_length = min_anchoring_length * 2;
        let mut remaining_search_length = max_outside_search_length;

        let mut over_air = tip_inside_bridge_mask;
        let mut following_segment_p0 = tip;
        let mut next_point_index: isize = tip_index as isize;
        let wall_size = wall.size();

        let increment_index = |next_point_index: &mut isize| {
            *next_point_index = (((*next_point_index - segment_direction) as usize).wrapping_add(wall_size) % wall_size) as isize;
        };

        let account_for_segment = |start: Point2LL, end: Point2LL, over_air: bool, remaining_anchoring_length: &mut Coord, remaining_search_length: &mut Coord| {
            let segment_length = v_size(end - start);
            if !over_air {
                *remaining_anchoring_length -= (*remaining_search_length).min(segment_length);
            }
            *remaining_search_length = 0.max(*remaining_search_length - segment_length);
        };

        increment_index(&mut next_point_index);
        while remaining_search_length > 0 && remaining_anchoring_length > 0 {
            let following_segment_p1 = wall.point_at(next_point_index as usize);
            let mut following_segment_intersections = bridge_mask.intersections_with_segment(following_segment_p0, following_segment_p1);

            let mut current_position = following_segment_p0;

            cppstd::stable_sort_by(&mut following_segment_intersections, |a, b| a < b);
            for &next_intersection in following_segment_intersections.iter() {
                let next_intersection_position = lerp(following_segment_p0, following_segment_p1, next_intersection as f64);
                account_for_segment(current_position, next_intersection_position, over_air, &mut remaining_anchoring_length, &mut remaining_search_length);

                over_air = !over_air;
                current_position = next_intersection_position;
            }

            account_for_segment(current_position, following_segment_p1, over_air, &mut remaining_anchoring_length, &mut remaining_search_length);

            following_segment_p0 = following_segment_p1;
            increment_index(&mut next_point_index);
        }
    }

    remaining_anchoring_length <= 0
}

/// `wallSegmentUsesBridging`: the sub-segments (as ratios of the given sub-part of the segment)
/// that should be printed as bridges.
#[allow(clippy::too_many_arguments)]
pub fn wall_segment_uses_bridging(
    bridge_mask_bb: &AABB,
    bridge_mask: &Shape,
    wall: &PathAdapter,
    segment_index: usize,
    segment_start_ratio: Ratio,
    segment_end_ratio: Ratio,
    min_bridge_line_len: Coord,
    line_width: Coord,
) -> Vec<(Ratio, Ratio)> {
    let p0_index = segment_index;
    let p1_index = (segment_index + 1) % wall.size();
    let p0 = wall.point_at(p0_index);
    let p1 = wall.point_at(p1_index);
    let segment_length = v_size(p0 - p1);

    if !bridge_mask_bb.hit(&AABB::from_points(&[p0, p1])) || segment_length < min_bridge_line_len {
        return Vec::new();
    }

    let p0_inside_mask = bridge_mask.inside(p0, false);
    let p1_inside_mask = bridge_mask.inside(p1, false);
    let mut intersections = bridge_mask.intersections_with_segment(p0, p1);

    if !p0_inside_mask && !p1_inside_mask && intersections.is_empty() {
        return Vec::new();
    }

    let min_anchoring_length = line_width;
    cppstd::stable_sort_by(&mut intersections, |a, b| a < b);
    if !is_wall_tip_anchored(bridge_mask, wall, p0_index, p0, p1, p0_inside_mask, 1, min_anchoring_length, &intersections)
        || !is_wall_tip_anchored(bridge_mask, wall, p1_index, p1, p0, p1_inside_mask, -1, min_anchoring_length, &intersections)
    {
        return Vec::new();
    }

    let mut current_position: Ratio = 0.0;
    let mut bridging = p0_inside_mask;
    let mut bridging_subsegments: Vec<(Ratio, Ratio)> = Vec::new();
    let mut add_bridging_subsegment = |start: Ratio, end: Ratio| {
        let bridge_line_len = ((end - start) * segment_length as f64) as Coord;
        if bridge_line_len >= min_bridge_line_len && end >= segment_start_ratio && start <= segment_end_ratio {
            let full_segment_start_ratio = if start < segment_start_ratio { segment_start_ratio } else { start };
            let full_segment_end_ratio = if segment_end_ratio < end { segment_end_ratio } else { end };

            let sub_segment_start_ratio = inverse_lerp(segment_start_ratio, segment_end_ratio, full_segment_start_ratio);
            let sub_segment_end_ratio = inverse_lerp(segment_start_ratio, segment_end_ratio, full_segment_end_ratio);

            bridging_subsegments.push((sub_segment_start_ratio, sub_segment_end_ratio));
        }
    };

    for &next_intersection in intersections.iter() {
        if bridging {
            add_bridging_subsegment(current_position, next_intersection as f64);
        }
        current_position = next_intersection as f64;
        bridging = !bridging;
    }

    if bridging {
        add_bridging_subsegment(current_position, 1.0);
    }

    bridging_subsegments
}
