//! `infill.h/.cpp`: generating the infill pattern of an area.

use crate::geometry::matrix::PointMatrix;
use crate::geometry::point2ll::{v_size, v_size2, Point2LL};
use crate::geometry::{OpenLinesSet, OpenPolyline, Shape};
use crate::infill::abstract_lines::{AbstractLinesInfill, RegularNGonType};
use crate::infill::zigzag_connector::ZigzagConnectorProcessor;
use crate::settings::enums::EFillMethod;
use crate::settings::settings::Settings;
use crate::settings::types::AngleDegrees;
use crate::skin::generate_infill_wall_tool_paths;
use crate::slice_data_storage::SliceMeshStorage;
use crate::utils::aabb::AABB;
use crate::utils::coord::Coord;
use crate::utils::extrusion_line::VariableWidthLines;
use crate::utils::linear_alg_2d as la;
use crate::utils::polyline_stitcher;
use crate::utils::section_type::SectionType;
use crate::utils::simplify::Simplify;
use crate::wall_tool_paths::WallToolPaths;

/// Function which returns the scanline_idx for a given x coordinate. For negative `x` this is
/// different from simple division. (Arguments are `int` in C++.)
#[inline]
fn compute_scan_segment_idx(x: i32, line_width: i32) -> i32 {
    if x < 0 {
        return (x + 1) / line_width - 1;
        // - 1 because -1 belongs to scansegment -1
        // + 1 because -line_width belongs to scansegment -1
    }
    x / line_width
}

const ONE_OVER_SQRT_2: f64 = 1.0 / std::f64::consts::SQRT_2;

/// An infill line segment used by the line connecting algorithm.
#[derive(Clone, Debug)]
#[allow(dead_code)]
struct InfillLineSegment {
    /// Where the line segment starts.
    start: Point2LL,
    /// If the line-segment needs to be altered during print (only used in the connecting lines algorithm).
    altered_start: Point2LL,
    /// Which polygon line segment the start of this infill line belongs to.
    start_segment: usize,
    /// Which polygon the start of this infill line belongs to.
    start_polygon: usize,
    /// If the line-segment starts at a different point due to prevention of crossing near the boundary, it gets a 'bend'.
    start_bend: Option<Point2LL>,
    /// Where the line segment ends.
    end: Point2LL,
    altered_end: Point2LL,
    end_segment: usize,
    end_polygon: usize,
    end_bend: Option<Point2LL>,
    /// The previous line segment that this line segment is connected to, if any.
    previous: Option<usize>,
    /// The next line segment that this line segment is connected to, if any.
    next: Option<usize>,
}

impl InfillLineSegment {
    fn new(start: Point2LL, start_segment: usize, start_polygon: usize, end: Point2LL, end_segment: usize, end_polygon: usize) -> Self {
        InfillLineSegment {
            start,
            altered_start: start,
            start_segment,
            start_polygon,
            start_bend: None,
            end,
            altered_end: end,
            end_segment,
            end_polygon,
            end_bend: None,
            previous: None,
            next: None,
        }
    }

    fn swap_direction(&mut self) {
        std::mem::swap(&mut self.start, &mut self.end);
        std::mem::swap(&mut self.altered_start, &mut self.altered_end);
        std::mem::swap(&mut self.start_bend, &mut self.end_bend);
        std::mem::swap(&mut self.next, &mut self.previous);
    }

    fn append_to(&self, result_polyline: &mut OpenPolyline, include_start: bool) {
        if include_start {
            result_polyline.points.push(self.altered_start);
        }
        if let Some(b) = self.start_bend {
            result_polyline.points.push(b);
        }
        if let Some(b) = self.end_bend {
            result_polyline.points.push(b);
        }
        result_polyline.points.push(self.altered_end);
    }
}

/// `UnionFind<E>` specialised for segment ids (with the exact C++ semantics, including `unite`
/// setting the parent of the given handles rather than of their roots).
struct UnionFind {
    items: Vec<usize>,
    element_to_position: std::collections::HashMap<usize, usize>,
    parent_index: Vec<usize>,
    rank: Vec<usize>,
}

impl UnionFind {
    fn new() -> Self {
        UnionFind { items: Vec::new(), element_to_position: std::collections::HashMap::new(), parent_index: Vec::new(), rank: Vec::new() }
    }

    fn add(&mut self, item: usize) -> usize {
        self.items.push(item);
        let handle = self.parent_index.len();
        self.element_to_position.insert(item, handle);
        self.parent_index.push(handle);
        self.rank.push(1);
        handle
    }

    fn find(&mut self, item: usize) -> usize {
        match self.element_to_position.get(&item) {
            None => usize::MAX,
            Some(&index) => self.find_by_handle(index),
        }
    }

    fn find_by_handle(&mut self, item_handle: usize) -> usize {
        if item_handle >= self.parent_index.len() {
            return usize::MAX;
        }
        let parent = self.parent_index[item_handle];
        if parent != item_handle {
            // This is a root.
            let p = self.find_by_handle(parent);
            self.parent_index[item_handle] = p;
        }
        self.parent_index[item_handle]
    }

    fn unite(&mut self, first: usize, second: usize) -> usize {
        let first_root = self.find_by_handle(first);
        let second_root = self.find_by_handle(second);
        // The tree with the greatest rank becomes the parent. This creates shallower trees.
        if self.rank[first_root] < self.rank[second_root] {
            self.parent_index[first] = second;
            self.rank[second_root] += self.rank[first_root];
            second
        } else {
            self.parent_index[second] = first;
            self.rank[first_root] += self.rank[second_root];
            first
        }
    }
}

/// `Infill`
#[derive(Clone, Debug)]
pub struct Infill {
    /// the space filling pattern of the infill to generate
    pattern: EFillMethod,
    /// Whether to connect the end pieces of the support lines via the wall
    zig_zaggify: bool,
    /// Whether the lines and zig_zaggification are generated by the connectLines algorithm
    connect_lines: bool,
    /// Whether to connect as much polygons together into a single path
    connect_polygons: bool,
    /// The area that originally needs to be filled with infill. The input of the algorithm.
    outer_contour: Shape,
    /// The part of the contour that will get filled with an infill pattern. Equals outer_contour minus the extra infill walls.
    inner_contour: Shape,
    /// The line width of the infill lines to generate
    infill_line_width: Coord,
    /// The distance between two infill lines / polygons
    line_distance: Coord,
    /// The distance by which to overlap the inner infill pattern with the infill walls
    infill_overlap: Coord,
    /// the number of infill lines next to each other
    infill_multiplier: usize,
    /// for linear infill types: the angle of the infill lines (or the angle of the grid)
    fill_angle: AngleDegrees,
    /// height of the layer for which we generate infill
    z: Coord,
    /// shift of the scanlines in the direction perpendicular to the fill_angle
    shift: Coord,
    /// Min feature size of the output
    max_resolution: Coord,
    /// Max deviation fro the original poly when enforcing max_resolution
    max_deviation: Coord,
    /// Number of walls to generate at the boundary of the infill region, spaced \ref infill_line_width apart
    wall_line_count: usize,
    /// Maximum width of a small infill region to be filled with walls
    small_area_width: Coord,
    /// origin of the infill pattern
    infill_origin: Point2LL,
    /// Whether to bypass the line stitching normally performed for polyline type infills
    skip_line_stitching: bool,
    /// Whether to fill gaps in strips of infill that would be too thin to fit the infill lines.
    fill_gaps: bool,
    /// (ZigZag) Whether endpieces of zigzag infill should be connected to the nearest infill line on both sides of the zigzag connector
    connected_zigzags: bool,
    /// (ZigZag) Whether to include endpieces: zigzag connector segments from one infill line to itself
    use_endpieces: bool,
    /// (ZigZag) Whether to skip some zags
    skip_some_zags: bool,
    /// (ZigZag) To skip one zag in every N if skip some zags is enabled
    zag_skip_count: usize,
    /// The size of the pockets at the intersections of the fractal in the cross 3d pattern
    pocket_size: Coord,
    /// Indication in which offset direction the extra infill lines are made
    mirror_offset: bool,

    /// The crossings of the infill lines with each line segment of each polygon (ids into `segments`).
    crossings_on_line: Vec<Vec<Vec<usize>>>,
    /// Storage of the infill line segments of the connecting algorithm.
    segments: Vec<InfillLineSegment>,
}

fn calc_connect_lines(pattern: EFillMethod, zig_zaggify: bool) -> bool {
    zig_zaggify
        && matches!(
            pattern,
            EFillMethod::LINES | EFillMethod::TRIANGLES | EFillMethod::GRID | EFillMethod::CUBIC | EFillMethod::TETRAHEDRAL | EFillMethod::QUARTER_CUBIC | EFillMethod::TRIHEXAGON
        )
}

/// Optional extra parameters of the C++ constructor overloads.
#[derive(Clone, Copy, Debug)]
pub struct InfillExtra {
    pub wall_line_count: usize,
    pub small_area_width: Coord,
    pub infill_origin: Point2LL,
    pub skip_line_stitching: bool,
    pub zigzag: Option<InfillZigzag>,
}

impl Default for InfillExtra {
    fn default() -> Self {
        InfillExtra { wall_line_count: 0, small_area_width: 0, infill_origin: Point2LL::new(0, 0), skip_line_stitching: false, zigzag: None }
    }
}

/// The extra parameters of the long C++ constructor.
#[derive(Clone, Copy, Debug)]
pub struct InfillZigzag {
    pub fill_gaps: bool,
    pub connected_zigzags: bool,
    pub use_endpieces: bool,
    pub skip_some_zags: bool,
    pub zag_skip_count: usize,
    pub pocket_size: Coord,
}

impl Infill {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pattern: EFillMethod,
        zig_zaggify: bool,
        connect_polygons: bool,
        in_outline: Shape,
        infill_line_width: Coord,
        line_distance: Coord,
        infill_overlap: Coord,
        infill_multiplier: usize,
        fill_angle: AngleDegrees,
        z: Coord,
        shift: Coord,
        max_resolution: Coord,
        max_deviation: Coord,
        extra: InfillExtra,
    ) -> Self {
        let mut ret = Infill {
            pattern,
            zig_zaggify,
            connect_lines: calc_connect_lines(pattern, zig_zaggify),
            connect_polygons,
            outer_contour: in_outline,
            inner_contour: Shape::default(),
            infill_line_width,
            line_distance,
            infill_overlap,
            infill_multiplier,
            fill_angle,
            z,
            shift,
            max_resolution,
            max_deviation,
            wall_line_count: extra.wall_line_count,
            small_area_width: extra.small_area_width,
            infill_origin: extra.infill_origin,
            skip_line_stitching: extra.skip_line_stitching,
            fill_gaps: true,
            connected_zigzags: false,
            use_endpieces: false,
            skip_some_zags: false,
            zag_skip_count: 0,
            pocket_size: 0,
            mirror_offset: false,
            crossings_on_line: Vec::new(),
            segments: Vec::new(),
        };
        if let Some(zz) = extra.zigzag {
            ret.fill_gaps = zz.fill_gaps;
            ret.connected_zigzags = zz.connected_zigzags;
            ret.use_endpieces = zz.use_endpieces;
            ret.skip_some_zags = zz.skip_some_zags;
            ret.zag_skip_count = zz.zag_skip_count;
            ret.pocket_size = zz.pocket_size;
            ret.mirror_offset = zig_zaggify;
        }
        ret
    }

    pub fn get_line_distance(&self) -> Coord {
        self.line_distance
    }

    pub fn get_inner_contour(&self) -> &Shape {
        &self.inner_contour
    }

    /// Generate the infill.
    #[allow(clippy::too_many_arguments)]
    pub fn generate(
        &mut self,
        toolpaths: &mut Vec<VariableWidthLines>,
        result_polygons: &mut Shape,
        result_lines: &mut OpenLinesSet,
        settings: &Settings,
        layer_idx: i32,
        section_type: SectionType,
        cross_fill_provider: Option<&crate::infill::sierpinski_fill_provider::SierpinskiFillProvider>,
        lightning_trees: Option<&crate::infill::lightning_generator::LightningLayer>,
        mesh: Option<&SliceMeshStorage>,
        prevent_small_exposed_to_air: &Shape,
    ) {
        if self.outer_contour.is_empty() {
            return;
        }

        self.inner_contour =
            generate_infill_wall_tool_paths(toolpaths, &self.outer_contour, self.wall_line_count, self.infill_line_width, settings, layer_idx as _, section_type);

        self.inner_contour = self.inner_contour.offset_default(self.infill_overlap);

        // It does not make sense to print a pattern in a small region. So the infill region
        // is split into a small region that will be filled with walls and the normal region
        // that will be filled with the pattern.
        if self.pattern != EFillMethod::CONCENTRIC && self.small_area_width > 0 {
            // INT2MM(static_cast<double>(infill_line_width_) / 2.0)
            let too_small_length = (self.infill_line_width as f64 / 2.0) / 1000.0;

            // Split the infill region in a narrow region and the normal region.
            let mut small_infill = self.inner_contour.clone();
            self.inner_contour = self.inner_contour.offset_default(-self.small_area_width / 2);
            self.inner_contour.remove_small_areas(too_small_length * too_small_length, true);
            self.inner_contour = self.inner_contour.offset_default(self.small_area_width / 2);
            self.inner_contour = self.inner_contour.union_polygons_other(prevent_small_exposed_to_air).intersection(&small_infill);
            self.inner_contour = Simplify::new(self.max_resolution, self.max_deviation, 0).shape(&self.inner_contour);
            small_infill = small_infill.difference(&self.inner_contour);

            // Small corners of a bigger area should not be considered narrow and are therefore added to the bigger area again.
            let small_infill_parts = small_infill.split_into_parts(false);
            small_infill.clear();
            let w = self.infill_line_width;
            for small_infill_part in small_infill_parts {
                if small_infill_part.0.offset_default(-w / 2).offset_default(w / 2).area() < (w * w * 10) as f64
                    && !self.inner_contour.intersection(&small_infill_part.0.offset_default(w / 4)).is_empty()
                {
                    self.inner_contour.push_all(&small_infill_part.0);
                } else {
                    // the part must still be printed, so re-add it
                    small_infill.push_all(&small_infill_part.0);
                }
            }
            self.inner_contour = self.inner_contour.union_polygons();

            // Fill narrow area with walls.
            let narrow_wall_count = (self.small_area_width / self.infill_line_width + 1) as usize;
            let mut wall_toolpaths = WallToolPaths::new_uniform(&small_infill, self.infill_line_width, narrow_wall_count, 0, settings, layer_idx as _, section_type);
            let small_infill_paths = wall_toolpaths.get_tool_paths().clone();
            for small_infill_path in small_infill_paths {
                toolpaths.push(small_infill_path);
            }
        }

        // apply an extra offset in case the pattern prints along the sides of the area.
        if self.pattern == EFillMethod::ZIG_ZAG // Zig-zag prints the zags along the walls.
            || (self.zig_zaggify
                && matches!(
                    self.pattern,
                    EFillMethod::LINES
                        | EFillMethod::TRIANGLES
                        | EFillMethod::GRID
                        | EFillMethod::CUBIC
                        | EFillMethod::TETRAHEDRAL
                        | EFillMethod::QUARTER_CUBIC
                        | EFillMethod::TRIHEXAGON
                        | EFillMethod::GYROID
                        | EFillMethod::HONEYCOMB
                        | EFillMethod::OCTAGON
                        | EFillMethod::CROSS
                        | EFillMethod::CROSS_3D
                ))
            || self.infill_multiplier % 2 == 0
        {
            // Multiplied infill prints loops of infill, partly along the walls, if even.
            self.inner_contour = self.inner_contour.offset_default(-self.infill_line_width / 2);
            self.inner_contour = Simplify::new(self.max_resolution, self.max_deviation, 0).shape(&self.inner_contour);
        }

        if self.infill_multiplier > 1 {
            let zig_zaggify_real = self.zig_zaggify;
            if self.infill_multiplier % 2 == 0 {
                self.zig_zaggify = false;
            }
            let mut generated_result_polygons = Shape::default();
            let mut generated_result_lines = OpenLinesSet::default();

            self.generate_inner(toolpaths, &mut generated_result_polygons, &mut generated_result_lines, settings, cross_fill_provider, lightning_trees, mesh);

            self.zig_zaggify = zig_zaggify_real;
            self.multiply_infill(&mut generated_result_polygons, &mut generated_result_lines);
            result_polygons.push_all_owned(generated_result_polygons);
            result_lines.push_all_owned(generated_result_lines);
        } else {
            // _generate may clear() the generated_result_lines, but this is an output variable that may contain data before we start.
            let mut generated_result_polygons = Shape::default();
            let mut generated_result_lines = OpenLinesSet::default();

            self.generate_inner(toolpaths, &mut generated_result_polygons, &mut generated_result_lines, settings, cross_fill_provider, lightning_trees, mesh);

            result_polygons.push_all_owned(generated_result_polygons);
            result_lines.push_all_owned(generated_result_lines);
        }

        if self.connect_polygons {
            // remove too small polygons
            let snap_distance = self.infill_line_width * 2; // polygons with a span of max 1 * nozzle_size are too small
            result_polygons.lines.retain(|poly| !crate::geometry::Polyline::shorter_than(poly, snap_distance));

            let (connected_polygons, connected_paths) = crate::utils::polygon_connector::PolygonConnector::connect_all(self.infill_line_width, result_polygons, toolpaths);
            *result_polygons = connected_polygons;
            *toolpaths = connected_paths;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn generate_inner(
        &mut self,
        toolpaths: &mut Vec<VariableWidthLines>,
        result_polygons: &mut Shape,
        result_lines: &mut OpenLinesSet,
        settings: &Settings,
        cross_fill_provider: Option<&crate::infill::sierpinski_fill_provider::SierpinskiFillProvider>,
        lightning_trees: Option<&crate::infill::lightning_generator::LightningLayer>,
        mesh: Option<&SliceMeshStorage>,
    ) {
        if self.inner_contour.is_empty() {
            return;
        }
        if self.line_distance == 0 {
            return;
        }

        match self.pattern {
            EFillMethod::GRID => self.generate_grid_infill(result_lines),
            EFillMethod::LINES => {
                let a = self.fill_angle.value();
                self.generate_line_infill(result_lines, self.line_distance as i32, a, 0)
            }
            EFillMethod::CUBIC => self.generate_cubic_infill(result_lines),
            EFillMethod::TETRAHEDRAL => self.generate_tetrahedral_infill(result_lines),
            EFillMethod::QUARTER_CUBIC => self.generate_quarter_cubic_infill(result_lines),
            EFillMethod::TRIANGLES => self.generate_triangle_infill(result_lines),
            EFillMethod::TRIHEXAGON => self.generate_trihexagon_infill(result_lines),
            EFillMethod::CONCENTRIC => self.generate_concentric_infill(toolpaths, settings),
            EFillMethod::ZIG_ZAG => {
                let a = self.fill_angle.value();
                self.generate_zig_zag_infill(result_lines, self.line_distance, a)
            }
            EFillMethod::CUBICSUBDIV => match mesh {
                None => crate::log_error!("Cannot generate Cubic Subdivision infill without a mesh!"),
                Some(mesh) => self.generate_cubic_sub_div_infill(result_lines, mesh),
            },
            EFillMethod::CROSS | EFillMethod::CROSS_3D => match cross_fill_provider {
                None => crate::log_error!("Cannot generate Cross infill without a cross fill provider!\n"),
                Some(p) => self.generate_cross_infill(p, result_polygons, result_lines),
            },
            EFillMethod::GYROID => {
                AbstractLinesInfill::Gyroid.generate_infill(
                    result_lines,
                    result_polygons,
                    self.zig_zaggify,
                    self.line_distance,
                    &self.inner_contour,
                    self.z,
                    self.infill_line_width,
                    self.fill_angle,
                );
            }
            EFillMethod::LIGHTNING => self.generate_lightning_infill(lightning_trees, result_lines),
            EFillMethod::HONEYCOMB => {
                AbstractLinesInfill::RegularNGonal(RegularNGonType::Hexagon).generate_infill(
                    result_lines,
                    result_polygons,
                    self.zig_zaggify,
                    self.line_distance,
                    &self.inner_contour,
                    self.z,
                    self.infill_line_width,
                    self.fill_angle,
                );
            }
            EFillMethod::OCTAGON => {
                AbstractLinesInfill::RegularNGonal(RegularNGonType::Octagon).generate_infill(
                    result_lines,
                    result_polygons,
                    self.zig_zaggify,
                    self.line_distance,
                    &self.inner_contour,
                    self.z,
                    self.infill_line_width,
                    self.fill_angle,
                );
            }
            EFillMethod::PLUGIN => {}
            _ => crate::log_error!("Fill pattern has unknown value.\n"),
        }

        if self.connect_lines {
            // The list should be empty because it will be again filled completely. Otherwise, might have double lines.
            result_lines.clear();
            self.connect_lines_fn(result_lines);
        }

        let simplifier = Simplify::new(self.max_resolution, self.max_deviation, 0);
        *result_polygons = simplifier.shape(result_polygons);

        if !self.skip_line_stitching
            && (self.zig_zaggify
                || matches!(
                    self.pattern,
                    EFillMethod::CROSS
                        | EFillMethod::CROSS_3D
                        | EFillMethod::CUBICSUBDIV
                        | EFillMethod::GYROID
                        | EFillMethod::HONEYCOMB
                        | EFillMethod::OCTAGON
                        | EFillMethod::ZIG_ZAG
                ))
        {
            // don't stich for non-zig-zagged line infill types
            let mut stitched_lines = OpenLinesSet::default();
            polyline_stitcher::stitch_open_into_shape(result_lines, &mut stitched_lines, result_polygons, self.infill_line_width, 10);
            *result_lines = stitched_lines;
        }
        *result_lines = simplifier.polylines(result_lines);
    }

    fn multiply_infill(&mut self, result_polygons: &mut Shape, result_lines: &mut OpenLinesSet) {
        if self.pattern == EFillMethod::CONCENTRIC {
            *result_polygons = result_polygons.process_even_odd(clipper::PolyFillType::EvenOdd); // make into areas
        }

        let odd_multiplier = self.infill_multiplier % 2 == 1;
        let offset = if odd_multiplier { self.infill_line_width } else { self.infill_line_width / 2 };

        // Get the first offset these are mirrored from the original center line
        let mut result = Shape::default();
        let first_offset;
        {
            let first_offset_lines = result_lines.offset_default(offset); // make lines on both sides of the input lines
            let first_offset_polygons_inward = result_polygons.offset_default(-offset); // make lines on the inside of the input polygons
            let first_offset_polygons_outward = result_polygons.offset_default(offset); // make lines on the other side of the input polygons
            let first_offset_polygons = first_offset_polygons_outward.difference(&first_offset_polygons_inward);
            let mut fo = first_offset_lines.union_polygons_other(&first_offset_polygons);
            if self.zig_zaggify {
                fo = self.inner_contour.difference(&fo);
            }
            first_offset = fo;
        }
        result.push_all(&first_offset);

        // Create the additional offsets from the first offsets, generated earlier, the direction of these offsets is
        // depended on whether these lines should be connected or not.
        if self.infill_multiplier > 3 {
            let mut reference_polygons = first_offset.clone();
            let multiplier = self.infill_multiplier / 2;

            let extra_offset: i32 = if self.mirror_offset { -(self.infill_line_width as i32) } else { self.infill_line_width as i32 };
            for _infill_line in 1..multiplier {
                let extra_polys = reference_polygons.offset_default(extra_offset as Coord);
                result.push_all(&extra_polys);
                reference_polygons = extra_polys;
            }
        }
        if self.zig_zaggify {
            result = result.intersection(&self.inner_contour);
        }

        // Remove the original center lines when there are an even number of lines required.
        if !odd_multiplier {
            result_polygons.clear();
            result_lines.clear();
        }
        result_polygons.push_all_owned(result);
        if !self.zig_zaggify {
            let polylines = self.inner_contour.intersection_polylines(result_polygons, true, 10, true);
            result_polygons.clear();
            polyline_stitcher::stitch_open_into_shape(&polylines, result_lines, result_polygons, self.infill_line_width, 10);
        }
    }

    fn generate_lightning_infill(&mut self, trees: Option<&crate::infill::lightning_generator::LightningLayer>, result_lines: &mut OpenLinesSet) {
        // Don't need to support areas smaller than line width, as they are always within radius:
        if self.inner_contour.area().abs() < self.infill_line_width as f64 || trees.is_none() {
            return;
        }
        result_lines.push_all_owned(trees.unwrap().convert_to_lines(&self.inner_contour, self.infill_line_width));
    }

    fn generate_concentric_infill(&mut self, toolpaths: &mut Vec<VariableWidthLines>, settings: &Settings) {
        let min_area = self.infill_line_width * self.infill_line_width;

        let mut current_inset = self.inner_contour.clone();
        let simplifier = Simplify::from_settings(settings);
        loop {
            // If line_distance is 0, start from the same contour as the previous line, except where the previous line closed up the shape.
            // So we add the whole nominal line width first (to allow lines to be closer together than 1 line width if the line distance is smaller) and then subtract line_distance.
            current_inset = current_inset.offset_default(self.infill_line_width - self.line_distance);
            current_inset = simplifier.shape(&current_inset); // Many insets lead to increasingly detailed shapes. Simplify to speed up processing.
            if current_inset.area() < min_area as f64 {
                // So small that it's inconsequential. Stop here.
                break;
            }

            let inset_wall_count = 1; // 1 wall at a time.
            let wall_0_inset = 0; // Don't apply any outer wall inset for these. That's just for the outer wall.
            let mut wall_toolpaths =
                WallToolPaths::new_uniform(&current_inset, self.infill_line_width, inset_wall_count, wall_0_inset, settings, 0, SectionType::CONCENTRIC_INFILL);
            let inset_paths = wall_toolpaths.get_tool_paths().clone();
            toolpaths.extend(inset_paths);

            current_inset = wall_toolpaths.get_inner_contour().clone();
        }
    }

    fn generate_grid_infill(&mut self, result: &mut OpenLinesSet) {
        let a = self.fill_angle;
        self.generate_line_infill(result, self.line_distance as i32, a.value(), 0);
        self.generate_line_infill(result, self.line_distance as i32, (a + AngleDegrees::new(90.0)).value(), 0);
    }

    fn generate_cubic_infill(&mut self, result: &mut OpenLinesSet) {
        let shift = (ONE_OVER_SQRT_2 * self.z as f64) as Coord;
        let a = self.fill_angle;
        self.generate_line_infill(result, self.line_distance as i32, a.value(), shift);
        self.generate_line_infill(result, self.line_distance as i32, (a + AngleDegrees::new(120.0)).value(), shift);
        self.generate_line_infill(result, self.line_distance as i32, (a + AngleDegrees::new(240.0)).value(), shift);
    }

    fn generate_tetrahedral_infill(&mut self, result: &mut OpenLinesSet) {
        self.generate_half_tetrahedral_infill(0.0, 0, result);
        self.generate_half_tetrahedral_infill(0.0, 90, result);
    }

    fn generate_quarter_cubic_infill(&mut self, result: &mut OpenLinesSet) {
        self.generate_half_tetrahedral_infill(0.0, 0, result);
        self.generate_half_tetrahedral_infill(0.5, 90, result);
    }

    fn generate_half_tetrahedral_infill(&mut self, pattern_z_shift: f64, angle_shift: i32, result: &mut OpenLinesSet) {
        let period = self.line_distance * 2;
        let mut shift = ((ONE_OVER_SQRT_2 * (self.z as f64 + pattern_z_shift * period as f64 * 2.0)) as Coord) % period;
        shift = shift.min(period - shift); // symmetry due to the fact that we are applying the shift in both directions
        shift = shift.min(period / 2 - self.infill_line_width / 2); // don't put lines too close to each other
        shift = shift.max(self.infill_line_width / 2); // don't put lines too close to each other
        let angle = (self.fill_angle + AngleDegrees::new(angle_shift as f64)).value();
        self.generate_line_infill(result, period as i32, angle, shift);
        self.generate_line_infill(result, period as i32, angle, -shift);
    }

    fn generate_triangle_infill(&mut self, result: &mut OpenLinesSet) {
        let a = self.fill_angle;
        self.generate_line_infill(result, self.line_distance as i32, a.value(), 0);
        self.generate_line_infill(result, self.line_distance as i32, (a + AngleDegrees::new(60.0)).value(), 0);
        self.generate_line_infill(result, self.line_distance as i32, (a + AngleDegrees::new(120.0)).value(), 0);
    }

    fn generate_trihexagon_infill(&mut self, result: &mut OpenLinesSet) {
        let a = self.fill_angle;
        self.generate_line_infill(result, self.line_distance as i32, a.value(), 0);
        self.generate_line_infill(result, self.line_distance as i32, (a + AngleDegrees::new(60.0)).value(), 0);
        self.generate_line_infill(result, self.line_distance as i32, (a + AngleDegrees::new(120.0)).value(), self.line_distance / 2);
    }

    fn generate_cubic_sub_div_infill(&mut self, result: &mut OpenLinesSet, mesh: &SliceMeshStorage) {
        let mut uncropped = OpenLinesSet::default();
        mesh.base_subdiv_cube.as_ref().expect("cubic subdivision octree").generate_subdivision_lines(self.z, &mut uncropped);
        let restitch = false; // cubic subdivision lines are always single line segments - not polylines consisting of multiple segments.
        *result = self.outer_contour.offset_default(self.infill_overlap).intersection_polylines(&uncropped, restitch, 10, true);
    }

    fn generate_cross_infill(
        &mut self,
        cross_fill_provider: &crate::infill::sierpinski_fill_provider::SierpinskiFillProvider,
        result_polygons: &mut Shape,
        result_lines: &mut OpenLinesSet,
    ) {
        let cross_pattern_polygon = cross_fill_provider.generate(self.pattern, self.z, self.infill_line_width, self.pocket_size);

        if cross_pattern_polygon.is_empty() {
            return;
        }

        if self.zig_zaggify {
            let mut cross_pattern_polygons = Shape::default();
            cross_pattern_polygons.push(cross_pattern_polygon);
            result_polygons.push_all_owned(self.inner_contour.intersection(&cross_pattern_polygons));
        } else {
            // make the polyline closed in order to handle cross_pattern_polygon as a polyline, rather than a closed polygon
            let mut cross_pattern_polylines = OpenLinesSet::default();
            cross_pattern_polylines.push(crate::geometry::polyline::ClosedLine::to_pseudo_open_polyline(&cross_pattern_polygon));
            let poly_lines = self.inner_contour.intersection_polylines(&cross_pattern_polylines, true, 10, true);
            polyline_stitcher::stitch_open_into_shape(&poly_lines, result_lines, result_polygons, self.infill_line_width, 10);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn add_line_infill(
        &self,
        result: &mut OpenLinesSet,
        rotation_matrix: &PointMatrix,
        scanline_min_idx: i32,
        line_distance: i32,
        boundary: AABB,
        cut_list: &mut [Vec<Coord>],
        shift: Coord,
    ) {
        debug_assert!(!self.connect_lines, "connectLines() should add the infill lines, not addLineInfill");

        let mut scanline_idx: usize = 0;
        let mut x: Coord = scanline_min_idx as Coord * line_distance as Coord + shift;
        while x < boundary.max.x {
            if scanline_idx >= cut_list.len() {
                break;
            }
            let crossings = &mut cut_list[scanline_idx];
            crossings.sort(); // sort by increasing Y coordinates (stable; equal values are indistinguishable)
            let mut crossing_idx = 0usize;
            while crossing_idx + 1 < crossings.len() {
                if crossings[crossing_idx + 1] - crossings[crossing_idx] < self.infill_line_width / 5 {
                    // segment is too short to create infill
                    crossing_idx += 2;
                    continue;
                }
                result.add_segment(
                    rotation_matrix.unapply(Point2LL::new(x, crossings[crossing_idx])),
                    rotation_matrix.unapply(Point2LL::new(x, crossings[crossing_idx + 1])),
                );
                crossing_idx += 2;
            }
            scanline_idx += 1;
            x += line_distance as Coord;
        }
    }

    fn get_shift_offset_from_infill_origin_and_rotation(&self, infill_rotation: f64) -> Coord {
        if self.infill_origin.x != 0 || self.infill_origin.y != 0 {
            let rotation_rads = infill_rotation * std::f64::consts::PI / 180.0;
            return (self.infill_origin.x as f64 * rotation_rads.cos() - self.infill_origin.y as f64 * rotation_rads.sin()) as Coord;
        }
        0
    }

    fn generate_line_infill(&mut self, result: &mut OpenLinesSet, line_distance: i32, infill_rotation: f64, mut shift: Coord) {
        shift += self.get_shift_offset_from_infill_origin_and_rotation(infill_rotation);
        let rotation_matrix = PointMatrix::from_degrees(infill_rotation);
        let mut lines_processor = ZigzagConnectorProcessor::new_no_zigzag(&rotation_matrix);
        let connected_zigzags = false;
        self.generate_linear_based_infill(result, line_distance, &rotation_matrix, &mut lines_processor, connected_zigzags, shift);
    }

    fn generate_zig_zag_infill(&mut self, result: &mut OpenLinesSet, line_distance: Coord, infill_rotation: f64) {
        let shift = self.get_shift_offset_from_infill_origin_and_rotation(infill_rotation);

        let rotation_matrix = PointMatrix::from_degrees(infill_rotation);
        let mut zigzag_processor =
            ZigzagConnectorProcessor::new(&rotation_matrix, self.use_endpieces, self.connected_zigzags, self.skip_some_zags, self.zag_skip_count as i32);
        let connected = self.connected_zigzags;
        self.generate_linear_based_infill(result, line_distance as i32, &rotation_matrix, &mut zigzag_processor, connected, shift);
    }

    /// Generate infill lines based on the scanline algorithm (see the C++ documentation of `generateLinearBasedInfill`).
    fn generate_linear_based_infill(
        &mut self,
        result: &mut OpenLinesSet,
        line_distance: i32,
        rotation_matrix: &PointMatrix,
        zigzag_connector_processor: &mut ZigzagConnectorProcessor,
        connected_zigzags: bool,
        extra_shift: Coord,
    ) {
        if line_distance == 0 || self.inner_contour.is_empty() {
            // No infill to generate (0% density) or no area to generate it in.
            return;
        }

        let mut outline = self.inner_contour.clone(); // Make a copy. We'll be rotating this outline to make intersections always horizontal, for better performance.
        outline.apply_matrix(rotation_matrix);

        let ld = line_distance as Coord;
        let mut shift = extra_shift + self.shift;
        if shift < 0 {
            shift = ld - (-shift) % ld;
        } else {
            shift %= ld;
        }

        let boundary = AABB::from_shape(&outline);

        let scanline_min_idx = compute_scan_segment_idx((boundary.min.x - shift) as i32, line_distance);
        let line_count = compute_scan_segment_idx((boundary.max.x - shift) as i32, line_distance) + 1 - scanline_min_idx;

        let mut cut_list: Vec<Vec<Coord>> = vec![Vec::new(); line_count.max(0) as usize]; // mapping from scanline to all intersections with polygon segments

        #[derive(Clone, Copy)]
        struct Crossing {
            coordinate: Point2LL,
            polygon_index: usize,
            vertex_index: usize,
        }
        let min_scanline_index = compute_scan_segment_idx((boundary.min.x - shift) as i32, line_distance) + 1;
        let max_scanline_index = compute_scan_segment_idx((boundary.max.x - shift) as i32, line_distance) + 1;
        let mut crossings_per_scanline: Vec<Vec<Crossing>> = vec![Vec::new(); (max_scanline_index - min_scanline_index).max(0) as usize];
        if self.connect_lines {
            self.crossings_on_line.resize(outline.len(), Vec::new()); // One for each polygon.
        }

        for poly_idx in 0..outline.len() {
            let poly = &outline[poly_idx];
            if self.connect_lines {
                self.crossings_on_line[poly_idx].resize(poly.len(), Vec::new()); // One for each line in this polygon.
            }
            let mut p0 = *poly.last().unwrap();
            zigzag_connector_processor.register_vertex(p0); // always adds the first point to ZigzagConnectorProcessorEndPieces::first_zigzag_connector when using a zigzag infill type

            for point_idx in 0..poly.len() {
                let p1 = poly[point_idx];
                if p1.x == p0.x {
                    zigzag_connector_processor.register_vertex(p1);
                    p0 = p1;
                    continue;
                }

                let scanline_idx0;
                let scanline_idx1;
                // this way of handling the indices takes care of the case where a boundary line segment ends exactly on a scanline:
                // in case the next segment moves back from that scanline either 2 or 0 scanline-boundary intersections are created
                // otherwise only 1 will be created, counting as an actual intersection
                let mut direction = 1;
                if p0.x < p1.x {
                    scanline_idx0 = compute_scan_segment_idx((p0.x - shift) as i32, line_distance) + 1; // + 1 cause we don't cross the scanline of the first scan segment
                    scanline_idx1 = compute_scan_segment_idx((p1.x - shift) as i32, line_distance); // -1 cause the vertex point is handled in the next segment (or not in the case which looks like >)
                } else {
                    direction = -1;
                    scanline_idx0 = compute_scan_segment_idx((p0.x - shift) as i32, line_distance); // -1 cause the vertex point is handled in the previous segment (or not in the case which looks like >)
                    scanline_idx1 = compute_scan_segment_idx((p1.x - shift) as i32, line_distance) + 1; // + 1 cause we don't cross the scanline of the first scan segment
                }

                let mut scanline_idx = scanline_idx0;
                while scanline_idx != scanline_idx1 + direction {
                    let x: i32 = (scanline_idx.wrapping_mul(line_distance) as Coord + shift) as i32;
                    let y: i32 = (p1.y + (p0.y - p1.y) * (x as Coord - p1.x) / (p0.x - p1.x)) as i32;
                    cut_list[(scanline_idx - scanline_min_idx) as usize].push(y as Coord);
                    let scanline_linesegment_intersection = Point2LL::new(x as Coord, y as Coord);
                    zigzag_connector_processor.register_scanline_segment_intersection(scanline_linesegment_intersection, scanline_idx, (line_distance / 4) as Coord, result);
                    crossings_per_scanline[(scanline_idx - min_scanline_index) as usize].push(Crossing {
                        coordinate: scanline_linesegment_intersection,
                        polygon_index: poly_idx,
                        vertex_index: point_idx,
                    });
                    scanline_idx += direction;
                }
                zigzag_connector_processor.register_vertex(p1);
                p0 = p1;
            }
            zigzag_connector_processor.register_poly_finished(result);
        }

        if self.connect_lines {
            // Gather all crossings per scanline and find out which crossings belong together, then store them in crossings_on_line.
            for scanline_index in min_scanline_index..max_scanline_index {
                let crossings = &mut crossings_per_scanline[(scanline_index - min_scanline_index) as usize];
                // Sorts them by Y coordinate.
                crossings.sort_by(|a, b| a.coordinate.y.cmp(&b.coordinate.y)); // stable
                // Combine each 2 subsequent crossings together.
                let mut crossing_index: i64 = 0;
                while crossing_index < crossings.len() as i64 - 1 {
                    let first = crossings[crossing_index as usize];
                    let second = crossings[crossing_index as usize + 1];
                    crossing_index += 2;
                    // Avoid creating zero length crossing lines
                    let unrotated_first = rotation_matrix.unapply(first.coordinate);
                    let unrotated_second = rotation_matrix.unapply(second.coordinate);
                    if unrotated_first == unrotated_second {
                        continue;
                    }
                    let new_segment = self.segments.len();
                    self.segments.push(InfillLineSegment::new(
                        unrotated_first,
                        first.vertex_index,
                        first.polygon_index,
                        unrotated_second,
                        second.vertex_index,
                        second.polygon_index,
                    ));
                    // Put the same line segment in the data structure twice: Once for each of the polygon line segment that it crosses.
                    self.crossings_on_line[first.polygon_index][first.vertex_index].push(new_segment);
                    self.crossings_on_line[second.polygon_index][second.vertex_index].push(new_segment);
                }
            }
        } else {
            if cut_list.is_empty() {
                return;
            }
            if connected_zigzags && cut_list.len() == 1 && cut_list[0].len() <= 2 {
                return; // don't add connection if boundary already contains whole outline!
            }

            // We have to create our own lines when they are not created by the method connectLines.
            self.add_line_infill(result, rotation_matrix, scanline_min_idx, line_distance, boundary, &mut cut_list, shift);
        }
    }

    /// Point reference used by `resolve_intersection` (which field of which segment to modify).
    #[allow(clippy::too_many_arguments)]
    fn resolve_intersection(&mut self, at_distance: Coord, intersect: Point2LL, connect_start: &mut Point2LL, connect_end: &mut Point2LL, a: usize, b: usize) {
        // Select wich ends of the line need to 'bend'.
        let forward_line_a = self.segments[a].end == *connect_start;
        let forward_line_b = self.segments[b].start == *connect_end;

        // Set values ('pre existing' values are needed when feeding these as reference parameters to functions that need a value).
        let mut bend_a = Point2LL::new(0, 0);
        let mut bend_b = Point2LL::new(0, 0);
        let mut end_a = if forward_line_a { self.segments[a].altered_end } else { self.segments[a].altered_start };
        let mut end_b = if forward_line_b { self.segments[b].altered_start } else { self.segments[b].altered_end };

        // Find a bisector of the intersection; specifically, the one that crosses the connection & offset it by 1/2 distance to each side.
        let large_enough_vec_len: Coord = 0xFFFF_FFFF;
        let bisect = la::get_bisector_vector(intersect, *connect_start, *connect_end, large_enough_vec_len);
        let offset = Point2LL::new(((at_distance / 2) * -bisect.y) / large_enough_vec_len, ((at_distance / 2) * bisect.x) / large_enough_vec_len);
        let q = intersect + offset;
        let r = q + bisect;
        let s = intersect - offset;
        let t = s + bisect;

        // In certain rare conditions, the lines do not actually intersect in a way that we can solve with the current algorithm.
        let mut is_resolved = true;

        let (a_start, a_end, b_start, b_end) = (self.segments[a].start, self.segments[a].end, self.segments[b].start, self.segments[b].end);
        let lli = |p0: Point2LL, p1: Point2LL, p2: Point2LL, p3: Point2LL, out: &mut Point2LL| -> bool {
            match la::line_line_intersection(p0, p1, p2, p3) {
                Some(r) => {
                    *out = r;
                    true
                }
                None => false,
            }
        };

        // Use both of the resulting lines to place the 'bends' by intersecting with the original line-segments.
        let r1 = lli(q, r, a_start, a_end, &mut bend_a) && la::point_is_projected_beyond_line(bend_a, a_start, a_end) == 0;
        is_resolved &= r1;
        let r2 = lli(s, t, b_start, b_end, &mut bend_b) && la::point_is_projected_beyond_line(bend_b, b_start, b_end) == 0;
        is_resolved &= r2;

        // Also set the new end-points
        let r3 = lli(*connect_start, *connect_end, q, r, &mut end_a) && la::point_is_projected_beyond_line(end_a, *connect_start, *connect_end) == 0;
        is_resolved &= r3;
        let r4 = lli(*connect_start, *connect_end, s, t, &mut end_b) && la::point_is_projected_beyond_line(end_b, *connect_start, *connect_end) == 0;
        is_resolved &= r4;

        // Write back the (possibly altered) end points, like the references in C++.
        if forward_line_a {
            self.segments[a].altered_end = end_a;
        } else {
            self.segments[a].altered_start = end_a;
        }
        if forward_line_b {
            self.segments[b].altered_start = end_b;
        } else {
            self.segments[b].altered_end = end_b;
        }

        if is_resolved {
            // The connecting line will be made from the end-points.
            *connect_start = end_a;
            *connect_end = end_b;
            if forward_line_a {
                self.segments[a].end_bend = Some(bend_a);
            } else {
                self.segments[a].start_bend = Some(bend_a);
            }
            if forward_line_b {
                self.segments[b].start_bend = Some(bend_b);
            } else {
                self.segments[b].end_bend = Some(bend_b);
            }
        } else {
            // Put everything that has now potentially become messed up, back.
            // (the bends are reset; the altered end points stay modified like in C++)
            if forward_line_a {
                self.segments[a].end_bend = None;
            } else {
                self.segments[a].start_bend = None;
            }
            if forward_line_b {
                self.segments[b].start_bend = None;
            } else {
                self.segments[b].end_bend = None;
            }
        }
    }

    /// Connects infill lines together so that they form polylines.
    fn connect_lines_fn(&mut self, result_lines: &mut OpenLinesSet) {
        let mut connected_lines = UnionFind::new(); // Keeps track of which lines are connected to which.
        for crossings_on_polygon in &self.crossings_on_line {
            for crossings_on_polygon_segment in crossings_on_polygon {
                for &infill_line in crossings_on_polygon_segment {
                    if connected_lines.find(infill_line) == usize::MAX {
                        connected_lines.add(infill_line); // Put every line in there as a separate set.
                    }
                }
            }
        }

        let half_line_distance_squared = (self.line_distance * self.line_distance) / 4;
        for polygon_index in 0..self.inner_contour.len() {
            let inner_contour_polygon = self.inner_contour[polygon_index].clone();
            if inner_contour_polygon.is_empty() {
                continue;
            }
            let mut previous_crossing: Option<usize> = None; // The crossing that we should connect to. If none, we have been skipping until we find the next crossing.
            let mut previous_segment: Option<usize> = None; // The last segment we were connecting while drawing a line along the border.
            let mut vertex_before = *inner_contour_polygon.last().unwrap();
            for vertex_index in 0..inner_contour_polygon.len() {
                let vertex_after = inner_contour_polygon[vertex_index];

                // Sort crossings on every line by how far they are from their initial point.
                {
                    let segments = &self.segments;
                    let crossings_on_polygon_segment = &mut self.crossings_on_line[polygon_index][vertex_index];
                    let key = |s: usize| -> Coord {
                        let seg = &segments[s];
                        let choose = seg.start_segment == vertex_index && seg.start_polygon == polygon_index;
                        let point = if choose { seg.start } else { seg.end };
                        v_size(point - vertex_before)
                    };
                    crossings_on_polygon_segment.sort_by(|l, r| key(*l).cmp(&key(*r))); // stable
                }

                let crossings_on_polygon_segment = self.crossings_on_line[polygon_index][vertex_index].clone();
                for crossing in crossings_on_polygon_segment {
                    if previous_crossing.is_none() {
                        // If we're not yet drawing, then we have been trying to find the next vertex. We found it! Let's start drawing.
                        previous_crossing = Some(crossing);
                        previous_segment = Some(crossing);
                    } else {
                        let crossing_handle = connected_lines.find(crossing);
                        let previous_crossing_handle = connected_lines.find(previous_crossing.unwrap());
                        if crossing_handle == previous_crossing_handle {
                            // These two infill lines are already connected. Don't create a loop now. Continue connecting with the next crossing.
                            continue;
                        }

                        // Join two infill lines together with a connecting line.
                        // Here the InfillLineSegments function as a linked list, so that they can easily be joined.
                        let ps = previous_segment.unwrap();
                        let previous_forward = self.segments[ps].start_segment == vertex_index && self.segments[ps].start_polygon == polygon_index;
                        let next_forward = self.segments[crossing].start_segment == vertex_index && self.segments[crossing].start_polygon == polygon_index;
                        let mut previous_point = if previous_forward { self.segments[ps].start } else { self.segments[ps].end };
                        let mut next_point = if next_forward { self.segments[crossing].start } else { self.segments[crossing].end };

                        let new_segment;
                        // If the segment is near length, we avoid creating it but still want to connect the crossing with the previous segment.
                        if previous_point == next_point {
                            if previous_forward {
                                self.segments[ps].previous = Some(crossing);
                            } else {
                                self.segments[ps].next = Some(crossing);
                            }
                            new_segment = ps;
                        } else {
                            let epsilon_sqd: Coord = 25;

                            // Resolve any intersections of the fill lines close to the boundary, by inserting extra points so the lines don't create a tiny 'loop'.
                            let (ps_start, ps_end, c_start, c_end) =
                                (self.segments[ps].start, self.segments[ps].end, self.segments[crossing].start, self.segments[crossing].end);
                            if v_size2(previous_point - next_point) < half_line_distance_squared {
                                if let Some(intersect) = la::line_line_intersection(ps_start, ps_end, c_start, c_end) {
                                    if la::point_is_projected_beyond_line(intersect, ps_start, ps_end) == 0
                                        && la::point_is_projected_beyond_line(intersect, c_start, c_end) == 0
                                        && v_size2(previous_point - intersect) > epsilon_sqd
                                        && v_size2(next_point - intersect) > epsilon_sqd
                                    {
                                        self.resolve_intersection(self.infill_line_width, intersect, &mut previous_point, &mut next_point, ps, crossing);
                                    }
                                }
                            }
                            // NOTE: in C++ previous_point/next_point are references to the start/end of the segments,
                            // so the endpoints are updated when the intersection is resolved.
                            if previous_forward {
                                self.segments[ps].start = previous_point;
                            } else {
                                self.segments[ps].end = previous_point;
                            }
                            if next_forward {
                                self.segments[crossing].start = next_point;
                            } else {
                                self.segments[crossing].end = next_point;
                            }

                            // A connecting line between them.
                            let mut seg = InfillLineSegment::new(previous_point, vertex_index, polygon_index, next_point, vertex_index, polygon_index);
                            seg.altered_start = previous_point;
                            seg.altered_end = next_point;
                            seg.previous = Some(ps);
                            seg.next = Some(crossing);
                            new_segment = self.segments.len();
                            self.segments.push(seg);
                            if previous_forward {
                                self.segments[ps].previous = Some(new_segment);
                            } else {
                                self.segments[ps].next = Some(new_segment);
                            }
                        }

                        if next_forward {
                            self.segments[crossing].previous = Some(new_segment);
                        } else {
                            self.segments[crossing].next = Some(new_segment);
                        }
                        connected_lines.unite(crossing_handle, previous_crossing_handle);
                        previous_crossing = None;
                        previous_segment = None;
                    }
                }

                // Upon going to the next vertex, if we're drawing, put an extra vertex in our infill lines.
                if previous_crossing.is_some() {
                    let ps = previous_segment.unwrap();
                    let choose_side = vertex_index == self.segments[ps].start_segment && polygon_index == self.segments[ps].start_polygon;
                    let previous_side = if choose_side { self.segments[ps].start } else { self.segments[ps].end };
                    if previous_side == vertex_after {
                        // Edge case when an infill line ends directly on top of vertex_after: We skip the extra connecting line segment, as that would be 0-length.
                        previous_segment = None;
                        previous_crossing = None;
                    } else {
                        let mut seg = InfillLineSegment::new(
                            previous_side,
                            vertex_index,
                            polygon_index,
                            vertex_after,
                            (vertex_index + 1) % self.inner_contour[polygon_index].len(),
                            polygon_index,
                        );
                        seg.previous = Some(ps);
                        let new_segment = self.segments.len();
                        self.segments.push(seg);
                        if choose_side {
                            self.segments[ps].previous = Some(new_segment);
                        } else {
                            self.segments[ps].next = Some(new_segment);
                        }
                        previous_segment = Some(new_segment);
                    }
                }

                vertex_before = vertex_after;
                self.crossings_on_line[polygon_index][vertex_index].clear();
            }
        }

        // Save all lines, now connected, to the output.
        let mut completed_groups: std::collections::HashSet<usize> = std::collections::HashSet::new();
        let items = connected_lines.items.clone();
        for infill_line in items {
            let group = connected_lines.find(infill_line);
            if completed_groups.contains(&group) {
                // We already completed this group.
                continue;
            }

            // Find where the polyline ends by searching through previous and next lines.
            let mut previous_vertex = self.segments[infill_line].start; // Take one side arbitrarily to start from.
            let mut current_infill_line = infill_line;
            while self.segments[current_infill_line].next.is_some() && self.segments[current_infill_line].previous.is_some() {
                // Until we reached an endpoint.
                let cur = &self.segments[current_infill_line];
                let choose_side = previous_vertex == cur.start;
                let next_vertex = if choose_side { cur.end } else { cur.start };
                current_infill_line = if choose_side { cur.next.unwrap() } else { cur.previous.unwrap() };
                previous_vertex = next_vertex;
            }

            // Now go along the linked list of infill lines and output the infill lines to the actual result.
            let mut result_line = OpenPolyline::default();
            if self.segments[current_infill_line].previous.is_some() {
                self.segments[current_infill_line].swap_direction();
            }
            self.segments[current_infill_line].append_to(&mut result_line, true);
            previous_vertex = self.segments[current_infill_line].end;
            let mut next = self.segments[current_infill_line].next;
            while let Some(cur) = next {
                if previous_vertex != self.segments[cur].start {
                    self.segments[cur].swap_direction();
                }
                let next_vertex = self.segments[cur].end; // Opposite side of the line.
                let polyline_break = false;
                self.segments[cur].append_to(&mut result_line, polyline_break);
                next = self.segments[cur].next;
                previous_vertex = next_vertex;
            }
            result_lines.push(result_line);

            completed_groups.insert(group);
        }
        self.segments.clear();
        self.crossings_on_line.clear();
    }
}
