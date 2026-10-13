//! `slicer.h/.cpp`: intersect a mesh with horizontal planes and assemble the cross sections
//! into polygons.

use crate::geometry::lines_set::CheckNonEmptyParam;
use crate::geometry::point2ll::{dot, lerp as lerp_point, shorter_then, v_size, v_size2, v_sizef, Point2LL};
use crate::geometry::points_f::{lerp_2f, Point2F, Point3D};
use crate::geometry::{OpenLinesSet, OpenPolyline, Polygon, Polyline, Shape};
use crate::mesh::Mesh;
use crate::settings::adaptive_layer_heights::AdaptiveLayer;
use crate::settings::enums::{ESurfaceMode, SlicingTolerance};
use crate::utils::coord::Coord;
use crate::utils::simplify::Simplify;
use crate::utils::sparse_grid::{SparsePointGrid, SparsePointGridInclusive};
use crate::utils::thread_pool::{parallel_for_mut, UnsafeSlice};
use crate::geometry::point3ll::Point3LL;
use clipper::JoinType;
use cppstd::PriorityQueue;
use std::collections::HashMap;
use std::sync::Arc;

const LARGEST_NEGLECTED_GAP_FIRST_PHASE: Coord = 10; // MM2INT(0.01)
const LARGEST_NEGLECTED_GAP_SECOND_PHASE: Coord = 20; // MM2INT(0.02)
const MAX_STITCH1: Coord = 10000; // MM2INT(10.0)

#[derive(Clone, Debug)]
pub struct SlicerSegment {
    pub start: Point2LL,
    pub end: Point2LL,
    pub uv_start: Option<Point2F>,
    pub uv_end: Option<Point2F>,
    pub face_index: i32,
    /// The index of the other face connected via the edge that created end.
    pub end_other_face_idx: i32,
    /// If end corresponds to a vertex of the mesh, then this is populated with that vertex (index).
    pub end_vertex: Option<u32>,
    /// Whether the segment has been added to a polygon loop.
    pub added_to_polygon: bool,
}

impl Default for SlicerSegment {
    fn default() -> Self {
        SlicerSegment {
            start: Point2LL::default(),
            end: Point2LL::default(),
            uv_start: None,
            uv_end: None,
            face_index: -1,
            end_other_face_idx: -1,
            end_vertex: None,
            added_to_polygon: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct ClosePolygonResult {
    polygon_idx: usize,
    point_idx: usize,
}

#[derive(Clone, Copy, Debug, Default)]
struct GapCloserResult {
    len: Coord,
    polygon_idx: usize,
    point_idx_a: usize,
    point_idx_b: usize,
    a_to_b: bool,
}

/// `SlicerLayer::Terminus`: a polyline endpoint, encoded as `polyline_idx * 2 + is_end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Terminus {
    idx: usize,
}

impl Terminus {
    const INVALID: Terminus = Terminus { idx: usize::MAX };

    fn from_index(idx: usize) -> Self {
        Terminus { idx }
    }
    fn new(polyline_idx: usize, is_end: bool) -> Self {
        Terminus { idx: polyline_idx * 2 + if is_end { 1 } else { 0 } }
    }
    fn get_polyline_idx(&self) -> usize {
        self.idx / 2
    }
    fn is_end(&self) -> bool {
        (self.idx & 1) == 1
    }
    fn as_index(&self) -> usize {
        self.idx
    }
    fn end_index_from_polyline_end_index(polyline_end_idx: u32) -> usize {
        // `unsigned int` arithmetic.
        polyline_end_idx.wrapping_mul(2) as usize
    }
}

#[derive(Clone, Copy, Debug)]
struct PossibleStitch {
    dist2: i64,
    terminus_0: Terminus,
    terminus_1: Terminus,
}

impl PossibleStitch {
    fn in_order(&self) -> bool {
        // in order is using back of line 0 and front of line 1
        self.terminus_0.is_end() && !self.terminus_1.is_end()
    }

    /// `PossibleStitch::operator<` (the priority queue returns the "largest" element first).
    fn less(&self, other: &PossibleStitch) -> bool {
        // better if lower distance
        if self.dist2 > other.dist2 {
            return true;
        } else if self.dist2 < other.dist2 {
            return false;
        }

        // better if in order instead of reversed
        if !self.in_order() && other.in_order() {
            return true;
        }

        // better if lower Terminus::Index for terminus_0
        // This just defines a more total order and isn't strictly necessary.
        if self.terminus_0.as_index() > other.terminus_0.as_index() {
            return true;
        } else if self.terminus_0.as_index() < other.terminus_0.as_index() {
            return false;
        }

        // better if lower Terminus::Index for terminus_1
        // This just defines a more total order and isn't strictly necessary.
        if self.terminus_1.as_index() > other.terminus_1.as_index() {
            return true;
        } else if self.terminus_1.as_index() < other.terminus_1.as_index() {
            return false;
        }

        // The stitches have equal goodness
        false
    }
}

/// `SlicerLayer::TerminusTrackingMap`.
struct TerminusTrackingMap {
    terminus_old_to_cur_map: Vec<Terminus>,
    terminus_cur_to_old_map: Vec<Terminus>,
}

impl TerminusTrackingMap {
    fn new(end_idx: usize) -> Self {
        let mut old_to_cur = vec![Terminus::INVALID; end_idx];
        for idx in 0..end_idx {
            old_to_cur[idx] = Terminus::from_index(idx);
        }
        let cur_to_old = old_to_cur.clone();
        TerminusTrackingMap { terminus_old_to_cur_map: old_to_cur, terminus_cur_to_old_map: cur_to_old }
    }

    fn get_cur_from_old(&self, old: &Terminus) -> Terminus {
        self.terminus_old_to_cur_map[old.as_index()]
    }

    fn get_old_from_cur(&self, cur: &Terminus) -> Terminus {
        self.terminus_cur_to_old_map[cur.as_index()]
    }

    fn mark_removed(&mut self, cur: &Terminus) {
        let old = self.get_old_from_cur(cur);
        self.terminus_old_to_cur_map[old.as_index()] = Terminus::INVALID;
        self.terminus_cur_to_old_map[cur.as_index()] = Terminus::INVALID;
    }

    fn update_map(&mut self, cur_terms: &[Terminus], next_terms: &[Terminus], removed_cur_terms: &[Terminus]) {
        let num_terms = cur_terms.len();
        // save old locations
        let mut old_terms = vec![Terminus::INVALID; num_terms];
        for idx in 0..num_terms {
            old_terms[idx] = self.get_old_from_cur(&cur_terms[idx]);
        }
        // update using maps old <-> cur and cur <-> next
        for idx in 0..num_terms {
            self.terminus_old_to_cur_map[old_terms[idx].as_index()] = next_terms[idx];
            let next_term = next_terms[idx];
            if next_term != Terminus::INVALID {
                self.terminus_cur_to_old_map[next_term.as_index()] = old_terms[idx];
            }
        }
        // remove next locations that no longer exist
        for rem in removed_cur_terms {
            self.terminus_cur_to_old_map[rem.as_index()] = Terminus::INVALID;
        }
    }
}

/// `SlicedUVCoordinates`: lookup of texture coordinates for a sliced layer.
pub struct SlicedUVCoordinates {
    located_uv_coordinates: SparsePointGridInclusive<Point2F>,
    segments: Vec<UvSegment>,
}

struct UvSegment {
    start: Point2LL,
    end: Point2LL,
    uv_start: Point2F,
    uv_end: Point2F,
}

impl SlicedUVCoordinates {
    const CELL_SIZE: Coord = 1000;
    const SEARCH_RADIUS: Coord = 1000;

    pub fn new(segments: &[SlicerSegment]) -> Self {
        let mut located = SparsePointGridInclusive::new(Self::CELL_SIZE, 0);
        let mut segs = Vec::with_capacity(segments.len());
        for segment in segments {
            if let (Some(uv_start), Some(uv_end)) = (segment.uv_start, segment.uv_end) {
                located.insert(segment.start, uv_start);
                located.insert(segment.end, uv_end);
                segs.push(UvSegment { start: segment.start, end: segment.end, uv_start, uv_end });
            }
        }
        SlicedUVCoordinates { located_uv_coordinates: located, segments: segs }
    }

    pub fn get_closest_uv_coordinates(&self, position: &Point2LL) -> Option<Point2F> {
        // First, try to find a point on the grid which is very close to the given position.
        if let Some(nearest) = self.located_uv_coordinates.get_nearest(*position, Self::SEARCH_RADIUS, |_| true) {
            return Some(nearest.val);
        }

        // We didn't find a close-enough point, so check all the segments to find the closest one.
        let mut closest_distance = f64::MAX;
        let mut closest_uv_coordinates: Option<Point2F> = None;
        for segment in &self.segments {
            let segment_length = v_sizef(segment.end - segment.start) as f64;
            if segment_length.abs() < 0.001 {
                continue;
            }
            let dot_product = dot(*position - segment.start, segment.end - segment.start) as f64 / segment_length;
            let distance_to_segment;
            let interpolate_factor;
            if dot_product > segment_length {
                interpolate_factor = 1.0;
                distance_to_segment = v_sizef(*position - segment.end) as f64;
            } else if dot_product < 0.0 {
                interpolate_factor = 0.0;
                distance_to_segment = v_sizef(*position - segment.start) as f64;
            } else {
                interpolate_factor = dot_product / segment_length;
                let projected_position = lerp_point(segment.start, segment.end, interpolate_factor);
                distance_to_segment = v_sizef(*position - projected_position) as f64;
            }
            if distance_to_segment < closest_distance {
                closest_distance = distance_to_segment;
                closest_uv_coordinates = Some(lerp_2f(&segment.uv_start, &segment.uv_end, interpolate_factor as f32));
            }
        }
        closest_uv_coordinates
    }
}

/// `SlicerLayer`: the cross section of one mesh at one height.
#[derive(Default)]
pub struct SlicerLayer {
    pub segments: Vec<SlicerSegment>,
    pub face_idx_to_segment_idx: HashMap<i32, i32>,
    pub z: i32,
    pub polygons: Shape,
    pub open_polylines: OpenLinesSet,
    pub sliced_uv_coordinates: Option<Arc<SlicedUVCoordinates>>,
}

impl SlicerLayer {
    pub fn new() -> Self {
        SlicerLayer { z: -1, ..Default::default() }
    }

    /// Connect the segments into polygons for this layer.
    pub fn make_polygons(&mut self, mesh: &Mesh) {
        let mut open_polylines = OpenLinesSet::default();

        self.make_basic_polygon_loops(mesh, &mut open_polylines);

        self.connect_open_polylines(&mut open_polylines);

        // TODO: (?) for mesh surface mode: connect open polygons. Maybe the above algorithm can create two open polygons which are actually connected when the starting segment is in the middle between the two open polygons.

        if mesh.settings.get_enum::<ESurfaceMode>("magic_mesh_surface_mode") == ESurfaceMode::NORMAL {
            // don't stitch when using (any) mesh surface mode, i.e. also don't stitch when using mixed mesh surface and closed polygons, because then polylines which are supposed to be open will be closed
            self.stitch(&mut open_polylines);
        }

        if mesh.settings.get_bool("meshfix_extensive_stitching") {
            self.stitch_extensive(&mut open_polylines);
        }

        if mesh.settings.get_bool("meshfix_keep_open_polygons") {
            for polyline in open_polylines.iter() {
                self.polygons.push_checked(Polygon::new(polyline.points.clone(), false), CheckNonEmptyParam::OnlyIfNotEmpty);
            }
        }

        for polyline in open_polylines.lines.drain(..) {
            // `std::move` of a const reference copies.
            self.open_polylines.push_checked(polyline, CheckNonEmptyParam::OnlyIfNotEmpty);
        }

        // Remove all the tiny polygons, or polygons that are not closed. As they do not contribute to the actual print.
        let snap_distance = mesh.settings.get_coord("minimum_polygon_circumference").max(1);
        self.polygons.lines.retain(|poly| !poly.shorter_than(snap_distance));

        // Finally optimize all the polygons. Every point removed saves time in the long run.
        self.polygons = Simplify::from_settings(&mesh.settings).shape(&self.polygons);

        self.polygons.remove_degenerate_verts(); // remove verts connected to overlapping line segments

        // Clean up polylines for Surface Mode printing
        self.open_polylines.lines.retain(|line| !line.shorter_than(snap_distance));

        self.open_polylines.remove_degenerate_verts();

        self.sliced_uv_coordinates = Some(Arc::new(SlicedUVCoordinates::new(&self.segments)));

        self.segments.clear();
    }

    fn make_basic_polygon_loops(&mut self, mesh: &Mesh, open_polylines: &mut OpenLinesSet) {
        for start_segment_idx in 0..self.segments.len() {
            if !self.segments[start_segment_idx].added_to_polygon {
                self.make_basic_polygon_loop(mesh, open_polylines, start_segment_idx);
            }
        }
        // Clear the segment list to save memory, it is no longer needed after this point.
        // (Note: the C++ code keeps the segments until the end of makePolygons.)
    }

    fn make_basic_polygon_loop(&mut self, mesh: &Mesh, open_polylines: &mut OpenLinesSet, start_segment_idx: usize) {
        let mut poly = Polygon::new(Vec::new(), true);
        poly.points.push(self.segments[start_segment_idx].start);

        let mut segment_idx = start_segment_idx as i32;
        while segment_idx != -1 {
            let segment = &mut self.segments[segment_idx as usize];
            poly.points.push(segment.end);
            segment.added_to_polygon = true;
            let segment = self.segments[segment_idx as usize].clone();
            segment_idx = self.get_next_segment_idx(mesh, &segment, start_segment_idx);
            if segment_idx == start_segment_idx as i32 {
                // polyon is closed
                self.polygons.push(poly);
                return;
            }
        }
        // polygon couldn't be closed
        open_polylines.push(OpenPolyline::new(poly.points));
    }

    fn try_face_next_segment_idx(&self, segment: &SlicerSegment, face_idx: i32, start_segment_idx: usize) -> i32 {
        if let Some(&segment_idx) = self.face_idx_to_segment_idx.get(&face_idx) {
            let p1 = self.segments[segment_idx as usize].start;
            let diff = segment.end - p1;
            if shorter_then(diff, LARGEST_NEGLECTED_GAP_FIRST_PHASE) {
                if segment_idx == start_segment_idx as i32 {
                    return start_segment_idx as i32;
                }
                if self.segments[segment_idx as usize].added_to_polygon {
                    return -1;
                }
                return segment_idx;
            }
        }
        -1
    }

    fn get_next_segment_idx(&self, mesh: &Mesh, segment: &SlicerSegment, start_segment_idx: usize) -> i32 {
        let mut next_segment_idx = -1;

        let segment_ended_at_edge = segment.end_vertex.is_none();
        if segment_ended_at_edge {
            let face_to_try = segment.end_other_face_idx;
            if face_to_try == -1 {
                return -1;
            }
            return self.try_face_next_segment_idx(segment, face_to_try, start_segment_idx);
        } else {
            // segment ended at vertex

            let faces_to_try = &mesh.vertices[segment.end_vertex.unwrap() as usize].connected_faces;
            for &face_to_try in faces_to_try {
                let result_segment_idx = self.try_face_next_segment_idx(segment, face_to_try as i32, start_segment_idx);
                if result_segment_idx == start_segment_idx as i32 {
                    return start_segment_idx as i32;
                } else if result_segment_idx != -1 {
                    // not immediately returned since we might still encounter the start_segment_idx
                    next_segment_idx = result_segment_idx;
                }
            }
        }

        next_segment_idx
    }

    fn connect_open_polylines(&mut self, open_polylines: &mut OpenLinesSet) {
        let allow_reverse = false;
        // Search a bit fewer cells but at cost of covering more area.
        // Since acceptance area is small to start with, the extra is unlikely to hurt much.
        let cell_size = LARGEST_NEGLECTED_GAP_FIRST_PHASE * 2;
        self.connect_open_polylines_impl(open_polylines, LARGEST_NEGLECTED_GAP_SECOND_PHASE, cell_size, allow_reverse);
    }

    fn stitch(&mut self, open_polylines: &mut OpenLinesSet) {
        let allow_reverse = true;
        self.connect_open_polylines_impl(open_polylines, MAX_STITCH1, MAX_STITCH1, allow_reverse);
    }

    fn find_possible_stitches(
        &self,
        open_polylines: &OpenLinesSet,
        max_dist: Coord,
        cell_size: Coord,
        allow_reverse: bool,
    ) -> PriorityQueue<PossibleStitch, fn(&PossibleStitch, &PossibleStitch) -> bool> {
        let mut stitch_queue: PriorityQueue<PossibleStitch, fn(&PossibleStitch, &PossibleStitch) -> bool> =
            PriorityQueue::new(|a: &PossibleStitch, b: &PossibleStitch| a.less(b));

        // maximum distance squared
        let max_dist2 = max_dist * max_dist;

        // Represents a terminal point of a polyline in open_polylines.
        #[derive(Clone, Copy)]
        struct StitchGridVal {
            polyline_idx: u32,
            // Depending on the SparsePointGridInclusive, this might be the start or the end of the polyline.
            polyline_term_pt: Point2LL,
        }
        let locator = |val: &StitchGridVal| val.polyline_term_pt;

        // Used to find nearby end points within a fixed maximum radius
        let mut grid_ends = SparsePointGrid::new(cell_size, 0, locator);
        // Used to find nearby start points within a fixed maximum radius
        let mut grid_starts = SparsePointGrid::new(cell_size, 0, locator);

        // populate grids

        // Inserts the ends of all polylines into the grid (does not
        //   insert the starts of the polylines).
        for polyline_0_idx in 0..open_polylines.len() {
            let polyline_0 = &open_polylines[polyline_0_idx];
            if polyline_0.len() < 1 {
                continue;
            }
            grid_ends.insert(StitchGridVal { polyline_idx: polyline_0_idx as u32, polyline_term_pt: *polyline_0.last().unwrap() });
        }

        // Inserts the start of all polylines into the grid.
        if allow_reverse {
            for polyline_0_idx in 0..open_polylines.len() {
                let polyline_0 = &open_polylines[polyline_0_idx];
                if polyline_0.len() < 1 {
                    continue;
                }
                grid_starts.insert(StitchGridVal { polyline_idx: polyline_0_idx as u32, polyline_term_pt: polyline_0[0] });
            }
        }

        // search for nearby end points
        for polyline_1_idx in 0..open_polylines.len() {
            let polyline_1 = &open_polylines[polyline_1_idx];

            if polyline_1.len() < 1 {
                continue;
            }

            // Check for stitches that append polyline_1 onto polyline_0
            // in natural order.  These are stitches that use the end of
            // polyline_0 and the start of polyline_1.
            let nearby_ends = grid_ends.get_nearby(polyline_1[0], max_dist);
            for nearby_end in &nearby_ends {
                let diff = nearby_end.polyline_term_pt - polyline_1[0];
                let dist2 = v_size2(diff);
                if dist2 < max_dist2 {
                    stitch_queue.push(PossibleStitch {
                        dist2,
                        terminus_0: Terminus::new(nearby_end.polyline_idx as usize, true),
                        terminus_1: Terminus::new(polyline_1_idx, false),
                    });
                }
            }

            if allow_reverse {
                // Check for stitches that append polyline_1 onto polyline_0
                // by reversing order of polyline_1.  These are stitches that
                // use the end of polyline_0 and the end of polyline_1.
                let nearby_ends = grid_ends.get_nearby(*polyline_1.last().unwrap(), max_dist);
                for nearby_end in &nearby_ends {
                    // Disallow stitching with self with same end point
                    if nearby_end.polyline_idx as usize == polyline_1_idx {
                        continue;
                    }

                    let diff = nearby_end.polyline_term_pt - *polyline_1.last().unwrap();
                    let dist2 = v_size2(diff);
                    if dist2 < max_dist2 {
                        stitch_queue.push(PossibleStitch {
                            dist2,
                            terminus_0: Terminus::new(nearby_end.polyline_idx as usize, true),
                            terminus_1: Terminus::new(polyline_1_idx, true),
                        });
                    }
                }

                // Check for stitches that append polyline_1 onto polyline_0
                // by reversing order of polyline_0.  These are stitches that
                // use the start of polyline_0 and the start of polyline_1.
                let nearby_starts = grid_starts.get_nearby(polyline_1[0], max_dist);
                for nearby_start in &nearby_starts {
                    // Disallow stitching with self with same end point
                    if nearby_start.polyline_idx as usize == polyline_1_idx {
                        continue;
                    }

                    let diff = nearby_start.polyline_term_pt - polyline_1[0];
                    let dist2 = v_size2(diff);
                    if dist2 < max_dist2 {
                        stitch_queue.push(PossibleStitch {
                            dist2,
                            terminus_0: Terminus::new(nearby_start.polyline_idx as usize, false),
                            terminus_1: Terminus::new(polyline_1_idx, false),
                        });
                    }
                }
            }
        }

        stitch_queue
    }

    fn plan_polyline_stitch(open_polylines: &OpenLinesSet, terminus_0: &mut Terminus, terminus_1: &mut Terminus, reverse: &mut [bool; 2]) {
        let polyline_0_idx = terminus_0.get_polyline_idx();
        let polyline_1_idx = terminus_1.get_polyline_idx();
        let back_0 = terminus_0.is_end();
        let back_1 = terminus_1.is_end();
        reverse[0] = false;
        reverse[1] = false;
        if back_0 {
            if back_1 {
                // back of both polylines
                // we can reverse either one and then append onto the other
                // reverse the smaller polyline
                if open_polylines[polyline_0_idx].len() < open_polylines[polyline_1_idx].len() {
                    std::mem::swap(terminus_0, terminus_1);
                }
                reverse[1] = true;
            } else {
                // back of 0, front of 1
                // already in order, nothing to do
            }
        } else {
            if back_1 {
                // front of 0, back of 1
                // in order if we swap 0 and 1
                std::mem::swap(terminus_0, terminus_1);
            } else {
                // front of both polylines
                // we can reverse either one and then prepend to the other
                // reverse the smaller polyline
                if open_polylines[polyline_0_idx].len() > open_polylines[polyline_1_idx].len() {
                    std::mem::swap(terminus_0, terminus_1);
                }
                reverse[0] = true;
            }
        }
    }

    fn join_polylines(open_polylines: &mut OpenLinesSet, idx_0: usize, idx_1: usize, reverse: &[bool; 2]) {
        if reverse[0] {
            // reverse polyline_0
            let polyline_0 = &mut open_polylines[idx_0];
            let size_0 = polyline_0.len();
            for idx in 0..size_0 / 2 {
                polyline_0.points.swap(idx, size_0 - 1 - idx);
            }
        }
        let polyline_1_points = std::mem::take(&mut open_polylines[idx_1].points);
        let polyline_0 = &mut open_polylines[idx_0];
        if reverse[1] {
            // reverse polyline_1 by adding in reverse order
            for p in polyline_1_points.iter().rev() {
                polyline_0.points.push(*p);
            }
        } else {
            // append polyline_1 onto polyline_0
            for p in polyline_1_points.iter() {
                polyline_0.points.push(*p);
            }
        }
        // `polyline_1.clear()` (already empty).
    }

    fn connect_open_polylines_impl(&mut self, open_polylines: &mut OpenLinesSet, max_dist: Coord, cell_size: Coord, allow_reverse: bool) {
        // below code closes smallest gaps first

        let mut stitch_queue = self.find_possible_stitches(open_polylines, max_dist, cell_size, allow_reverse);

        let invalid_terminus = Terminus::INVALID;
        let terminus_end_idx = Terminus::end_index_from_polyline_end_index(open_polylines.len() as u32);
        // Keeps track of how polyline end point locations move around
        let mut terminus_tracking_map = TerminusTrackingMap::new(terminus_end_idx);

        while let Some(next_stitch) = stitch_queue.pop() {
            let old_terminus_0 = next_stitch.terminus_0;
            let mut terminus_0 = terminus_tracking_map.get_cur_from_old(&old_terminus_0);
            if terminus_0 == invalid_terminus {
                // if we already used this terminus, then this stitch is no longer usable
                continue;
            }
            let old_terminus_1 = next_stitch.terminus_1;
            let mut terminus_1 = terminus_tracking_map.get_cur_from_old(&old_terminus_1);
            if terminus_1 == invalid_terminus {
                // if we already used this terminus, then this stitch is no longer usable
                continue;
            }

            let mut best_polyline_0_idx = terminus_0.get_polyline_idx();
            let mut best_polyline_1_idx = terminus_1.get_polyline_idx();

            // check to see if this completes a polygon
            let completed_poly = best_polyline_0_idx == best_polyline_1_idx;
            if completed_poly {
                // finished polygon
                let points = std::mem::take(&mut open_polylines[best_polyline_0_idx].points);
                self.polygons.push(Polygon::new(points, true));
                let cur_terms = [Terminus::new(best_polyline_0_idx, false), Terminus::new(best_polyline_0_idx, true)];
                for t in &cur_terms {
                    terminus_tracking_map.mark_removed(t);
                }
                continue;
            }

            // we need to join these polylines

            // plan how to join polylines
            let mut reverse = [false; 2];
            Self::plan_polyline_stitch(open_polylines, &mut terminus_0, &mut terminus_1, &mut reverse);

            // need to reread since planPolylineStitch can swap terminus_0/1
            best_polyline_0_idx = terminus_0.get_polyline_idx();
            best_polyline_1_idx = terminus_1.get_polyline_idx();

            // join polylines according to plan
            Self::join_polylines(open_polylines, best_polyline_0_idx, best_polyline_1_idx, &reverse);

            // update terminus_tracking_map
            let cur_terms = [
                Terminus::new(best_polyline_0_idx, false),
                Terminus::new(best_polyline_0_idx, true),
                Terminus::new(best_polyline_1_idx, false),
                Terminus::new(best_polyline_1_idx, true),
            ];
            let mut next_terms = [Terminus::new(best_polyline_0_idx, false), invalid_terminus, invalid_terminus, Terminus::new(best_polyline_0_idx, true)];
            if reverse[0] {
                next_terms.swap(0, 1);
            }
            if reverse[1] {
                next_terms.swap(2, 3);
            }
            // cur_terms -> next_terms has movement map
            // best_polyline_1 is always removed
            terminus_tracking_map.update_map(&cur_terms, &next_terms, &cur_terms[2..4]);
        }
    }

    fn stitch_extensive(&mut self, open_polylines: &mut OpenLinesSet) {
        // For extensive stitching find 2 open polygons that are touching 2 closed polygons.
        //  Then find the shortest path over this polygon that can be used to connect the open polygons,
        //  And generate a path over this shortest bit to link up the 2 open polygons.
        //  (If these 2 open polygons are the same polygon, then the final result is a closed polyon)

        loop {
            let mut best_polyline_1_idx: u32 = u32::MAX;
            let mut best_polyline_2_idx: u32 = u32::MAX;
            let mut best_result: Option<GapCloserResult> = None;

            for polyline_1_idx in 0..open_polylines.len() as u32 {
                let polyline_1 = &open_polylines[polyline_1_idx as usize];
                if polyline_1.len() < 1 {
                    continue;
                }

                {
                    let res = self.find_polygon_gap_closer(polyline_1[0], *polyline_1.last().unwrap());
                    if let Some(r) = res {
                        if best_result.is_none() || r.len < best_result.unwrap().len {
                            best_polyline_1_idx = polyline_1_idx;
                            best_polyline_2_idx = polyline_1_idx;
                            best_result = res;
                        }
                    }
                }

                for polyline_2_idx in 0..open_polylines.len() as u32 {
                    let polyline_2 = &open_polylines[polyline_2_idx as usize];
                    if polyline_2.len() < 1 || polyline_1_idx == polyline_2_idx {
                        continue;
                    }

                    let res = self.find_polygon_gap_closer(polyline_1[0], *polyline_2.last().unwrap());
                    if let Some(r) = res {
                        if best_result.is_none() || r.len < best_result.unwrap().len {
                            best_polyline_1_idx = polyline_1_idx;
                            best_polyline_2_idx = polyline_2_idx;
                            best_result = res;
                        }
                    }
                }
            }

            if let Some(best) = best_result {
                let b1 = best_polyline_1_idx as usize;
                let b2 = best_polyline_2_idx as usize;
                let pidx = best.polygon_idx;
                if best_polyline_1_idx == best_polyline_2_idx {
                    if best.point_idx_a == best.point_idx_b {
                        self.polygons.push(Polygon::new(open_polylines[b1].points.clone(), true));
                        open_polylines[b1].points.clear();
                    } else if best.a_to_b {
                        let mut poly = Polygon::default();
                        let mut j = best.point_idx_a as u32;
                        while j as usize != best.point_idx_b {
                            poly.points.push(self.polygons[pidx][j as usize]);
                            j = (j + 1) % self.polygons[pidx].len() as u32;
                        }
                        let ol = &open_polylines[b1];
                        let mut j = (ol.len() as u32).wrapping_sub(1);
                        while (j as i32) >= 0 {
                            poly.points.push(ol[j as usize]);
                            j = j.wrapping_sub(1);
                        }
                        self.polygons.push(poly);
                        open_polylines[b1].points.clear();
                    } else {
                        let n = self.polygons.len();
                        self.polygons.push(Polygon::new(open_polylines[b1].points.clone(), true));
                        let mut j = best.point_idx_b as u32;
                        while j as usize != best.point_idx_a {
                            let p = self.polygons[pidx][j as usize];
                            self.polygons[n].points.push(p);
                            j = (j + 1) % self.polygons[pidx].len() as u32;
                        }
                        open_polylines[b1].points.clear();
                    }
                } else {
                    if best.point_idx_a == best.point_idx_b {
                        let src = open_polylines[b1].points.clone();
                        open_polylines[b2].points.extend_from_slice(&src);
                        open_polylines[b1].points.clear();
                    } else if best.a_to_b {
                        let mut poly = Polygon::default();
                        let mut n = best.point_idx_a as u32;
                        while n as usize != best.point_idx_b {
                            poly.points.push(self.polygons[pidx][n as usize]);
                            n = (n + 1) % self.polygons[pidx].len() as u32;
                        }
                        let mut n = (poly.len() as u32).wrapping_sub(1);
                        while (n as i32) >= 0 {
                            open_polylines[b2].points.push(poly[n as usize]);
                            n = n.wrapping_sub(1);
                        }
                        let src = open_polylines[b1].points.clone();
                        open_polylines[b2].points.extend_from_slice(&src);
                        open_polylines[b1].points.clear();
                    } else {
                        let mut n = best.point_idx_b as u32;
                        while n as usize != best.point_idx_a {
                            let p = self.polygons[pidx][n as usize];
                            open_polylines[b2].points.push(p);
                            n = (n + 1) % self.polygons[pidx].len() as u32;
                        }
                        let src = open_polylines[b1].points.clone();
                        let mut n = (src.len() as u32).wrapping_sub(1);
                        while (n as i32) >= 0 {
                            open_polylines[b2].points.push(src[n as usize]);
                            n = n.wrapping_sub(1);
                        }
                        open_polylines[b1].points.clear();
                    }
                }
            } else {
                break;
            }
        }
    }

    fn find_polygon_gap_closer(&self, ip0: Point2LL, ip1: Point2LL) -> Option<GapCloserResult> {
        let c1 = self.find_polygon_point_closest_to(ip0);
        let c2 = self.find_polygon_point_closest_to(ip1);
        let (c1, c2) = match (c1, c2) {
            (Some(a), Some(b)) if a.polygon_idx == b.polygon_idx => (a, b),
            _ => return None,
        };

        let mut ret = GapCloserResult { polygon_idx: c1.polygon_idx, point_idx_a: c1.point_idx, point_idx_b: c2.point_idx, a_to_b: true, len: 0 };
        let poly = &self.polygons[ret.polygon_idx];

        if ret.point_idx_a == ret.point_idx_b {
            // Connection points are on the same line segment.
            ret.len = v_size(ip0 - ip1);
        } else {
            // Find out if we have should go from A to B or the other way around.
            let mut p0 = poly[ret.point_idx_a];
            let mut len_a: i64 = v_size(p0 - ip0);
            let mut i = ret.point_idx_a as u32;
            while i as usize != ret.point_idx_b {
                let p1 = poly[i as usize];
                len_a += v_size(p0 - p1);
                p0 = p1;
                i = (i + 1) % poly.len() as u32;
            }
            len_a += v_size(p0 - ip1);

            p0 = poly[ret.point_idx_b];
            let mut len_b: i64 = v_size(p0 - ip1);
            let mut i = ret.point_idx_b as u32;
            while i as usize != ret.point_idx_a {
                let p1 = poly[i as usize];
                len_b += v_size(p0 - p1);
                p0 = p1;
                i = (i + 1) % poly.len() as u32;
            }
            len_b += v_size(p0 - ip0);

            if len_a < len_b {
                ret.a_to_b = true;
                ret.len = len_a;
            } else {
                ret.a_to_b = false;
                ret.len = len_b;
            }
        }
        Some(ret)
    }

    fn find_polygon_point_closest_to(&self, input: Point2LL) -> Option<ClosePolygonResult> {
        for n in 0..self.polygons.len() {
            let poly = &self.polygons[n];
            let mut p0 = poly[poly.len() - 1];
            for i in 0..poly.len() {
                let p1 = poly[i];

                // Q = A + Normal( B - A ) * ((( B - A ) dot ( P - A )) / VSize( A - B ));
                let p_diff = p1 - p0;
                let line_length: i64 = v_size(p_diff);
                if line_length > 1 {
                    let dist_on_line: i64 = dot(p_diff, input - p0) / line_length;
                    if dist_on_line >= 0 && dist_on_line <= line_length {
                        let q = p0 + p_diff * dist_on_line / line_length;
                        if shorter_then(q - input, 100) {
                            return Some(ClosePolygonResult { polygon_idx: n, point_idx: i });
                        }
                    }
                }
                p0 = p1;
            }
        }
        None
    }
}

/// `Slicer`: slices one mesh into layers.
pub struct Slicer {
    pub layers: Vec<SlicerLayer>,
    pub mesh: *const Mesh,
}

unsafe impl Send for Slicer {}
unsafe impl Sync for Slicer {}

impl Slicer {
    pub fn mesh(&self) -> &Mesh {
        // SAFETY: meshes live in the current mesh group for the duration of the slice.
        unsafe { &*self.mesh }
    }

    pub fn new(
        mesh: &mut Mesh,
        thickness: Coord,
        slice_layer_count: usize,
        use_variable_layer_heights: bool,
        adaptive_layers: Option<&Vec<AdaptiveLayer>>,
        slicing_tolerance: SlicingTolerance,
        initial_layer_thickness: Coord,
    ) -> Slicer {
        assert!(slice_layer_count > 0);

        let timer = std::time::Instant::now();

        let mut layers = Self::build_layers_with_height(
            slice_layer_count,
            slicing_tolerance,
            initial_layer_thickness,
            thickness,
            use_variable_layer_heights,
            adaptive_layers,
        );

        let zbbox = Self::build_z_heights_for_faces(mesh);

        Self::build_segments(mesh, &zbbox, slicing_tolerance, &mut layers);

        crate::log_info!("Slice of mesh took {:.3} seconds", timer.elapsed().as_secs_f64());
        let timer = std::time::Instant::now();

        Self::make_polygons(mesh, slicing_tolerance, &mut layers);
        crate::log_info!("Make polygons took {:.3} seconds", timer.elapsed().as_secs_f64());

        Slicer { layers, mesh: mesh as *const Mesh }
    }

    fn build_segments(mesh: &Mesh, zbbox: &[(i32, i32)], slicing_tolerance: SlicingTolerance, layers: &mut [SlicerLayer]) {
        parallel_for_mut(layers, |_, layer| {
            let z = layer.z;
            layer.segments.reserve(100);

            // loop over all mesh faces
            for face_idx in 0..mesh.faces.len() {
                if z < zbbox[face_idx].0 || z > zbbox[face_idx].1 {
                    continue;
                }

                // get all vertices per face
                let face = &mesh.faces[face_idx];
                let v0i = face.vertex_index[0] as usize;
                let v1i = face.vertex_index[1] as usize;
                let v2i = face.vertex_index[2] as usize;
                let uv0 = face.uv_coordinates[0];
                let uv1 = face.uv_coordinates[1];
                let uv2 = face.uv_coordinates[2];

                // get all vertices represented as 3D point
                let mut p0 = mesh.vertices[v0i].p;
                let mut p1 = mesh.vertices[v1i].p;
                let mut p2 = mesh.vertices[v2i].p;

                // Compensate for points exactly on the slice-boundary, except for 'inclusive', which already handles this correctly.
                if slicing_tolerance != SlicingTolerance::INCLUSIVE {
                    let z64 = z as i64;
                    p0.z += ((p0.z == z64) as i32 * -((p0.z < 1) as i32)) as i64;
                    p1.z += ((p1.z == z64) as i32 * -((p1.z < 1) as i32)) as i64;
                    p2.z += ((p2.z == z64) as i32 * -((p2.z < 1) as i32)) as i64;
                }

                let mut s;
                let end_edge_idx: usize;
                let zz = z as i64;

                // Here we look if the face is in the layer
                if p0.z < zz && p1.z > zz && p2.z > zz {
                    //  p1   p2
                    //  --------
                    //    \  /
                    //     \/
                    //     p0
                    s = Self::project_2d(&p0, &p2, &p1, &uv0, &uv2, &uv1, zz);
                    end_edge_idx = 0;
                } else if p0.z > zz && p1.z <= zz && p2.z <= zz {
                    //     p0
                    //     /\
                    //    /  \
                    //  --------
                    //  p1  p2
                    s = Self::project_2d(&p0, &p1, &p2, &uv0, &uv1, &uv2, zz);
                    end_edge_idx = 2;
                    if p2.z == zz {
                        s.end_vertex = Some(v2i as u32);
                    }
                } else if p1.z < zz && p0.z > zz && p2.z > zz {
                    s = Self::project_2d(&p1, &p0, &p2, &uv1, &uv0, &uv2, zz);
                    end_edge_idx = 1;
                } else if p1.z > zz && p0.z <= zz && p2.z <= zz {
                    s = Self::project_2d(&p1, &p2, &p0, &uv1, &uv2, &uv0, zz);
                    end_edge_idx = 0;
                    if p0.z == zz {
                        s.end_vertex = Some(v0i as u32);
                    }
                } else if p2.z < zz && p1.z > zz && p0.z > zz {
                    s = Self::project_2d(&p2, &p1, &p0, &uv2, &uv1, &uv0, zz);
                    end_edge_idx = 2;
                } else if p2.z > zz && p1.z <= zz && p0.z <= zz {
                    s = Self::project_2d(&p2, &p0, &p1, &uv2, &uv0, &uv1, zz);
                    end_edge_idx = 1;
                    if p1.z == zz {
                        s.end_vertex = Some(v1i as u32);
                    }
                } else {
                    // Not all cases create a segment, because a point of a face could create just a dot, and two touching faces
                    //   on the slice would create two segments
                    continue;
                }

                // store the segments per layer
                let seg_idx = layer.segments.len() as i32;
                layer.face_idx_to_segment_idx.entry(face_idx as i32).or_insert(seg_idx);
                s.face_index = face_idx as i32;
                s.end_other_face_idx = face.connected_face_index[end_edge_idx];
                s.added_to_polygon = false;
                layer.segments.push(s);
            }
        });
    }

    fn build_layers_with_height(
        slice_layer_count: usize,
        slicing_tolerance: SlicingTolerance,
        initial_layer_thickness: Coord,
        thickness: Coord,
        use_variable_layer_heights: bool,
        adaptive_layers: Option<&Vec<AdaptiveLayer>>,
    ) -> Vec<SlicerLayer> {
        let mut layers_res: Vec<SlicerLayer> = (0..slice_layer_count).map(|_| SlicerLayer::new()).collect();

        // set (and initialize compensation for) initial layer, depending on slicing mode
        layers_res[0].z = if slicing_tolerance == SlicingTolerance::INCLUSIVE { 0 } else { 0i64.max(initial_layer_thickness - thickness) as i32 };
        let mut adjusted_layer_offset = initial_layer_thickness;
        if use_variable_layer_heights {
            layers_res[0].z = adaptive_layers.unwrap()[0].z_position as i32;
        } else if slicing_tolerance == SlicingTolerance::MIDDLE {
            layers_res[0].z = (initial_layer_thickness / 2) as i32;
            adjusted_layer_offset = initial_layer_thickness + (thickness / 2);
        }

        // define all layer z positions (depending on slicing mode, see above)
        for layer_nr in 1..slice_layer_count {
            if use_variable_layer_heights {
                layers_res[layer_nr].z = adaptive_layers.unwrap()[layer_nr].z_position as i32;
            } else {
                layers_res[layer_nr].z = (adjusted_layer_offset + (thickness * (layer_nr as i64 - 1))) as i32;
            }
        }

        layers_res
    }

    fn make_polygons(mesh: &mut Mesh, slicing_tolerance: SlicingTolerance, layers: &mut Vec<SlicerLayer>) {
        {
            let mesh_ref: &Mesh = mesh;
            parallel_for_mut(layers, |_, layer| {
                layer.make_polygons(mesh_ref);
            });
        }

        match slicing_tolerance {
            SlicingTolerance::INCLUSIVE => {
                for layer_nr in 0..layers.len().saturating_sub(1) {
                    let next = std::mem::take(&mut layers[layer_nr + 1].polygons);
                    layers[layer_nr].polygons = layers[layer_nr].polygons.union_polygons_with(&next, clipper::PolyFillType::NonZero);
                    layers[layer_nr + 1].polygons = next;
                }
            }
            SlicingTolerance::EXCLUSIVE => {
                for layer_nr in 0..layers.len().saturating_sub(1) {
                    let next = std::mem::take(&mut layers[layer_nr + 1].polygons);
                    layers[layer_nr].polygons = layers[layer_nr].polygons.intersection(&next);
                    layers[layer_nr + 1].polygons = next;
                }
                layers.last_mut().unwrap().polygons.clear();
            }
            _ => {}
        }

        let mut layer_apply_initial_xy_offset: usize = 0;
        if !layers.is_empty()
            && layers[0].polygons.is_empty()
            && !mesh.settings.get_bool("support_mesh")
            && !mesh.settings.get_bool("anti_overhang_mesh")
            && !mesh.settings.get_bool("cutting_mesh")
            && !mesh.settings.get_bool("infill_mesh")
        {
            layer_apply_initial_xy_offset = 1;
        }

        let xy_offset = mesh.settings.get_coord("xy_offset");
        let xy_offset_0 = mesh.settings.get_coord("xy_offset_layer_0");
        let xy_offset_hole = mesh.settings.get_coord("hole_xy_offset");
        let hole_offset_max_diameter = mesh.settings.get_coord("hole_xy_offset_max_diameter");

        let max_hole_area = std::f64::consts::PI / 4.0 * (hole_offset_max_diameter * hole_offset_max_diameter) as f64;

        let layers_slice = UnsafeSlice::new(layers.as_mut_slice());
        crate::utils::thread_pool::parallel_for(0, layers_slice.len(), |layer_nr| {
            // SAFETY: each layer is only touched by its own iteration.
            let layer = unsafe { layers_slice.get_mut(layer_nr) };
            let xy_offset_local = if layer_nr <= layer_apply_initial_xy_offset { xy_offset_0 } else { xy_offset };
            if xy_offset_local != 0 {
                layer.polygons = layer.polygons.offset(xy_offset_local, JoinType::Round, 1.2);
            }
            if xy_offset_hole != 0 {
                let parts = layer.polygons.split_into_parts(false);
                layer.polygons.clear();
                for part in &parts {
                    let mut holes = Shape::default();
                    let mut outline = Shape::default();
                    for poly in part.0.iter() {
                        let area = poly.area();
                        let abs_area = area.abs();
                        let is_hole = area < 0.0;
                        if is_hole {
                            if hole_offset_max_diameter == 0 {
                                holes.push_all_owned(poly.offset(xy_offset_hole as i32, JoinType::Miter, 1.2));
                            } else if abs_area < max_hole_area {
                                let distance = cppstd::lerp(xy_offset_hole as f64, 0.0, abs_area / max_hole_area) as i32;
                                holes.push_all_owned(poly.offset(distance, JoinType::Miter, 1.2));
                            } else {
                                holes.push(poly.clone());
                            }
                        } else {
                            outline.push(poly.clone());
                        }
                    }
                    layer.polygons.push_all_owned(outline.difference(&holes.union_polygons()));
                }
            }
        });

        mesh.expand_xy(xy_offset);
    }

    fn build_z_heights_for_faces(mesh: &Mesh) -> Vec<(i32, i32)> {
        let mut z_heights = Vec::with_capacity(mesh.faces.len());
        for face in &mesh.faces {
            // const int idx0 = face.vertex_index[0];
            let p0 = mesh.vertices[face.vertex_index[0] as usize].p;
            let p1 = mesh.vertices[face.vertex_index[1] as usize].p;
            let p2 = mesh.vertices[face.vertex_index[2] as usize].p;

            // find the minimum and maximum z point
            let mut min_z = p0.z as i32;
            if (p1.z as i32) < min_z {
                min_z = p1.z as i32;
            }
            if (p2.z as i32) < min_z {
                min_z = p2.z as i32;
            }

            let mut max_z = p0.z as i32;
            if (p1.z as i32) > max_z {
                max_z = p1.z as i32;
            }
            if (p2.z as i32) > max_z {
                max_z = p2.z as i32;
            }

            z_heights.push((min_z, max_z));
        }
        z_heights
    }

    fn project_2d(
        p0: &Point3LL,
        p1: &Point3LL,
        p2: &Point3LL,
        uv0: &Option<Point2F>,
        uv1: &Option<Point2F>,
        uv2: &Option<Point2F>,
        z: Coord,
    ) -> SlicerSegment {
        let mut seg = SlicerSegment::default();

        seg.start.x = Self::interpolate(z, p0.z, p1.z, p0.x, p1.x);
        seg.start.y = Self::interpolate(z, p0.z, p1.z, p0.y, p1.y);
        seg.end.x = Self::interpolate(z, p0.z, p2.z, p0.x, p2.x);
        seg.end.y = Self::interpolate(z, p0.z, p2.z, p0.y, p2.y);

        if let (Some(uv0), Some(uv1), Some(uv2)) = (uv0, uv1, uv2) {
            let start_barycentric = Self::get_barycentric_coordinates(&Point3LL::new(seg.start.x, seg.start.y, z), p0, p1, p2);
            let end_barycentric = Self::get_barycentric_coordinates(&Point3LL::new(seg.end.x, seg.end.y, z), p0, p1, p2);

            if let (Some(sb), Some(eb)) = (start_barycentric, end_barycentric) {
                seg.uv_start = Some(Self::interpolate_uv(&sb, uv0, uv1, uv2));
                seg.uv_end = Some(Self::interpolate_uv(&eb, uv0, uv1, uv2));
            }
        }

        seg
    }

    fn get_barycentric_coordinates(point: &Point3LL, p0: &Point3LL, p1: &Point3LL, p2: &Point3LL) -> Option<Point3D> {
        let v0 = *p1 - *p0;
        let v1 = *p2 - *p0;
        let v2 = *point - *p0;

        let d00 = v0.dot(&v0) as f64;
        let d01 = v0.dot(&v1) as f64;
        let d11 = v1.dot(&v1) as f64;
        let d20 = v2.dot(&v0) as f64;
        let d21 = v2.dot(&v1) as f64;
        let denom = d00 * d11 - d01 * d01;

        if denom.abs() < 0.000001 {
            // Triangle is degenerate
            return None;
        }

        let v = (d11 * d20 - d01 * d21) / denom;
        let w = (d00 * d21 - d01 * d20) / denom;
        let u = 1.0 - v - w;

        Some(Point3D::new(u, v, w))
    }

    fn interpolate_uv(b: &Point3D, uv0: &Point2F, uv1: &Point2F, uv2: &Point2F) -> Point2F {
        Point2F::new(
            (b.x * uv0.x as f64 + b.y * uv1.x as f64 + b.z * uv2.x as f64) as f32,
            (b.x * uv0.y as f64 + b.y * uv1.y as f64 + b.z * uv2.y as f64) as f32,
        )
    }

    /// Linear interpolation between y0 and y1, rounded to nearest.
    fn interpolate(x: Coord, x0: Coord, x1: Coord, y0: Coord, y1: Coord) -> Coord {
        let dx_01 = x1 - x0;
        let mut num = (y1 - y0) * (x - x0);
        num += if num > 0 { dx_01 / 4 } else { -dx_01 / 4 }; // add in offset to round result
        y0 + num / dx_01
    }
}
