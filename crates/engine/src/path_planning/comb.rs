//! `pathPlanning/Comb.h/.cpp`: generating full combing actions from a travel move from a start point
//! to an end point.
//!
//! Typical output: A combing path to the boundary of the polygon + a move through air avoiding other
//! parts in the layer + a combing path from the boundary of the ending polygon to the end point.

use crate::extruder_train::ExtruderTrain;
use crate::geometry::lines_set::NO_INDEX;
use crate::geometry::point2ll::{normal, shorter_then, v_size, v_size2, Point2LL};
use crate::geometry::{PartsView, Polygon, Shape, SingleShape};
use crate::path_planning::line_polygons_crossings::{CombPath, CombPaths, LinePolygonsCrossings};
use crate::settings::types::LayerIndex;
use crate::slice_data_storage::SliceDataStorage;
use crate::utils::coord::Coord;
use crate::utils::linear_alg_2d as la;
use crate::utils::polygon_utils::{ClosestPointPolygon, LocToLineGrid, PolygonUtils, NO_PENALTY};
use std::collections::{HashMap, HashSet};

/// Distance to enlarge the move_inside distance with for specific cases with on-border issues
const MAX_MOVE_INSIDE_ENLARGE_DISTANCE: Coord = 250;
/// in order to prevent on-boundary vs crossing boundary confusions (precision thing)
const OFFSET_DIST_TO_GET_FROM_ON_THE_POLYGON_TO_OUTSIDE: Coord = 40;
/// Distance to move start point and end point toward eachother to extra avoid collision with the boundaries.
const OFFSET_EXTRA_START_END: Coord = 100;

/// `vSize2(p / divisor)` narrowed to `int` like the C++ penalty functions (`std::function<int(Point2LL)>`).
#[inline]
fn penalty_vsize2_div(p: Point2LL, divisor: Coord) -> i32 {
    v_size2(Point2LL::new(p.x / divisor, p.y / divisor)) as i32
}

/// A crossing from the inside boundary to the outside boundary.
struct Crossing<'a> {
    /// Whether the startPoint or endPoint is inside the inside boundary
    dest_is_inside: bool,
    /// The point on the inside boundary, or in between the inside and outside boundary if the start/end point isn't inside the inside boudary
    in_or_mid: Point2LL,
    /// The point on the outside boundary
    out: Point2LL,
    /// The assembled inside-boundary SingleShape in which the dest_point lies.
    dest_part: SingleShape,
    /// The polygon of the part in which dest_point lies, which will be crossed (often will be the outside polygon)
    dest_crossing_poly: Option<Polygon>,
    /// The inside boundary as in Comb::boundary_inside
    boundary_inside: &'a Shape,
    /// The loc to line grid Comb::inside_loc_to_line
    inside_loc_to_line: &'a LocToLineGrid,
    /// Either the eventual startPoint or the eventual endPoint of this combing move
    dest_point: Point2LL,
    /// The index into Comb:partsView_inside of the part in which the dest_point is.
    dest_part_idx: u32,
}

impl<'a> Crossing<'a> {
    fn new(
        dest_point: Point2LL,
        dest_is_inside: bool,
        dest_part_idx: u32,
        dest_part_boundary_crossing_poly_idx: u32,
        boundary_inside: &'a Shape,
        inside_loc_to_line: &'a LocToLineGrid,
    ) -> Self {
        let dest_crossing_poly = if dest_is_inside {
            // initialize with most obvious poly, cause mostly a combing move will move outside the part, rather than inside a hole in the part
            Some(boundary_inside[dest_part_boundary_crossing_poly_idx as usize].clone())
        } else {
            None
        };
        Crossing {
            dest_is_inside,
            in_or_mid: Point2LL::default(),
            out: Point2LL::default(),
            dest_part: SingleShape::default(),
            dest_crossing_poly,
            boundary_inside,
            inside_loc_to_line,
            dest_point,
            dest_part_idx,
        }
    }

    /// Find the not-outside location (Combing::in_or_mid) of the crossing between to the outside boundary
    fn find_crossing_in_or_mid(&mut self, parts_view_inside: &PartsView, close_to: Point2LL) {
        if self.dest_is_inside {
            // in-case
            // find the point on the start inside-polygon closest to the endpoint, but also kind of close to the start point
            let dest_point = self.dest_point;
            let close_towards_start_penalty_function = move |candidate: Point2LL| -> i32 { penalty_vsize2_div(candidate - dest_point, 10) };
            self.dest_part = parts_view_inside.assemble_part(self.boundary_inside, self.dest_part_idx as usize);

            let mut boundary_crossing_point = ClosestPointPolygon::default();
            {
                // set [result] to a point on the destination part closest to close_to (but also a bit close to _dest_point)
                let mut dest_part_poly_indices: HashSet<u32> = HashSet::new();
                for &poly_idx in &parts_view_inside[self.dest_part_idx as usize] {
                    dest_part_poly_indices.insert(poly_idx as u32);
                }
                let mut dist2_score = Coord::MAX;
                self.inside_loc_to_line.process_line((dest_point, close_to), |boundary_segment| {
                    if !dest_part_poly_indices.contains(&(boundary_segment.poly_idx as u32)) {
                        // we're not looking at a polygon from the dest_part
                        return true; // a.k.a. continue;
                    }
                    let closest_here = la::get_closest_on_line_segment(close_to, boundary_segment.p(), boundary_segment.next().p());
                    let dist2_score_here = v_size2(close_to - closest_here) + v_size2(dest_point - closest_here) / 10;
                    if dist2_score_here < dist2_score {
                        dist2_score = dist2_score_here;
                        boundary_crossing_point = ClosestPointPolygon::with_poly_idx(
                            closest_here,
                            boundary_segment.point_idx,
                            boundary_segment.get_polygon() as *const Polygon,
                            boundary_segment.poly_idx,
                        );
                    }
                    true
                });
            }

            let mut result = boundary_crossing_point.p(); // the inside point of the crossing
            if !boundary_crossing_point.is_valid() {
                // no point has been found in the sparse grid
                result = self.dest_point;
            }

            let crossing_1_in_cp = PolygonUtils::ensure_inside_or_outside_cpp(
                &self.dest_part.0,
                &mut result,
                &boundary_crossing_point,
                OFFSET_DIST_TO_GET_FROM_ON_THE_POLYGON_TO_OUTSIDE as i32,
                Some(self.boundary_inside),
                Some(self.inside_loc_to_line),
                &close_towards_start_penalty_function,
            );
            if crossing_1_in_cp.is_valid() {
                self.dest_crossing_poly = Some(crossing_1_in_cp.poly().clone());
                self.in_or_mid = result;
            } else {
                // part is too small to be ensuring a point inside with the given distance
                self.in_or_mid = self.dest_point; // just use the startPoint or endPoint itself
            }
        } else {
            // mid-case
            self.in_or_mid = self.dest_point;
        }
    }

    /// Find the outside location (Combing::out)
    fn find_outside(&mut self, train: &ExtruderTrain, outside: &Shape, close_to: Point2LL, fail_on_unavoidable_obstacles: bool, comber: &mut CombOutside) -> bool {
        self.out = self.in_or_mid;
        if self.dest_is_inside || outside.inside(self.in_or_mid, true) {
            // start in_between
            // move outside
            let preferred_crossing_1_out = self.in_or_mid + normal(close_to - self.in_or_mid, comber.offset_from_inside_to_outside);
            let close_to_penalty_function = move |candidate: Point2LL| -> i32 { penalty_vsize2_div(candidate - preferred_crossing_1_out, 2) };
            let grid = comber.get_outside_loc_to_line(train);
            let crossing_1_out_cpp = PolygonUtils::find_close(self.in_or_mid, outside, grid, &close_to_penalty_function);
            if let Some(cpp) = crossing_1_out_cpp {
                self.out = PolygonUtils::move_outside_cpp(&cpp, OFFSET_DIST_TO_GET_FROM_ON_THE_POLYGON_TO_OUTSIDE as i32);
            } else {
                PolygonUtils::move_outside(outside, &mut self.out, OFFSET_DIST_TO_GET_FROM_ON_THE_POLYGON_TO_OUTSIDE as i32, i64::MAX);
            }
        }
        let in_out_dist2_1 = v_size2(self.out - self.in_or_mid);
        if self.dest_is_inside && in_out_dist2_1 > comber.max_crossing_dist2 {
            // moveInside moved too far
            // if move is too far over in_between
            // find crossing closer by
            let from = self.dest_crossing_poly.clone().expect("destination crossing poly should have been instantiated!");
            let best = self.find_best_crossing(train, outside, &from, self.dest_point, close_to, comber);
            if let Some((best_in, best_out)) = best {
                self.in_or_mid = PolygonUtils::move_inside_cpp(&best_in, OFFSET_DIST_TO_GET_FROM_ON_THE_POLYGON_TO_OUTSIDE as i32);
                self.out = PolygonUtils::move_outside_cpp(&best_out, OFFSET_DIST_TO_GET_FROM_ON_THE_POLYGON_TO_OUTSIDE as i32);
            }
            if fail_on_unavoidable_obstacles && v_size2(self.out - self.in_or_mid) > comber.max_crossing_dist2 {
                // moveInside moved still too far
                return false;
            }
        }
        true
    }

    /// Find the best crossing from some inside polygon to the outside boundary.
    fn find_best_crossing(
        &self,
        train: &ExtruderTrain,
        outside: &Shape,
        from: &Polygon,
        estimated_start: Point2LL,
        estimated_end: Point2LL,
        comber: &mut CombOutside,
    ) -> Option<(ClosestPointPolygon, ClosestPointPolygon)> {
        let mut best_idx: Option<usize> = None;
        let mut best_detour_score = Coord::MAX;
        let mut best_crossing_dist2: Coord = 0;
        let max_crossing_dist2 = comber.max_crossing_dist2;
        let grid = comber.get_outside_loc_to_line(train);
        let mut crossing_out_candidates = PolygonUtils::find_close_polygon(from, outside, grid, NO_PENALTY);
        let mut seen_close_enough_connection = false;
        for (idx, crossing_candidate) in crossing_out_candidates.iter().enumerate() {
            let crossing_dist2 = v_size2(crossing_candidate.0.location - crossing_candidate.1.location);
            if crossing_dist2 > max_crossing_dist2 * 2 {
                // preliminary filtering
                continue;
            }

            let dist_to_start = v_size(crossing_candidate.1.location - estimated_start); // use outside location, so that the crossing direction is taken into account
            let dist_to_end = v_size(crossing_candidate.1.location - estimated_end);
            let detour_dist = dist_to_start + dist_to_end;
            let detour_score = crossing_dist2 + detour_dist * detour_dist / 1000; // prefer a closest connection over a detour
            if (!seen_close_enough_connection && detour_score < best_detour_score)
                || (!seen_close_enough_connection && crossing_dist2 <= max_crossing_dist2)
                || (seen_close_enough_connection && crossing_dist2 <= max_crossing_dist2 && detour_score < best_detour_score)
            {
                if !seen_close_enough_connection && crossing_dist2 <= max_crossing_dist2 {
                    seen_close_enough_connection = true;
                }
                best_idx = Some(idx);
                best_detour_score = detour_score;
                best_crossing_dist2 = crossing_dist2;
            }
        }
        if best_detour_score == Coord::MAX {
            // i.e. if best_in == nullptr or if best_out == nullptr
            return None;
        }
        let best_idx = best_idx.unwrap();
        if best_crossing_dist2 > max_crossing_dist2 {
            // find closer point on line segments, rather than moving between vertices of the polygons only
            let (best_in, best_out) = &mut crossing_out_candidates[best_idx];
            PolygonUtils::walk_to_nearest_smallest_connection(best_in, best_out);
            best_crossing_dist2 = v_size2(best_in.location - best_out.location);
            if best_crossing_dist2 > max_crossing_dist2 {
                return None;
            }
        }
        Some(crossing_out_candidates[best_idx])
    }
}

/// The part of the combing state which is lazily computed (the outside boundaries) and the
/// constants needed by the crossings.
struct CombOutside {
    storage: *const SliceDataStorage,
    layer_nr: LayerIndex,
    travel_avoid_distance: Coord,
    offset_from_inside_to_outside: Coord,
    max_crossing_dist2: Coord,
    /// The boundary outside of which to stay to avoid collision with other layer parts.
    /// Boxed: the location-to-line grids keep raw pointers to these shapes, so they must not move on rehash.
    boundary_outside: HashMap<usize, Box<Shape>>,
    /// The boundary of the model itself
    model_boundary: HashMap<usize, Box<Shape>>,
    /// The SparsePointGridInclusive mapping locations to line segments of the outside boundary.
    outside_loc_to_line: HashMap<usize, Box<LocToLineGrid>>,
    /// The SparsePointGridInclusive mapping locations to line segments of the model boundary
    model_boundary_loc_to_line: HashMap<usize, Box<LocToLineGrid>>,
}

impl CombOutside {
    fn storage(&self) -> &SliceDataStorage {
        unsafe { &*self.storage }
    }

    /// Get the SparsePointGridInclusive mapping locations to line segments of the outside boundary. Calculate it when it hasn't been calculated yet.
    fn get_outside_loc_to_line(&mut self, train: &ExtruderTrain) -> &LocToLineGrid {
        let nr = train.extruder_nr;
        if !self.outside_loc_to_line.contains_key(&nr) {
            let square = self.offset_from_inside_to_outside * 3 / 2;
            let grid = PolygonUtils::create_loc_to_line_grid(self.get_boundary_outside(train), square as i32);
            self.outside_loc_to_line.insert(nr, grid);
        }
        &self.outside_loc_to_line[&nr]
    }

    /// Get the boundary_outside, which is an offset from the outlines of all meshes in the layer. Calculate it when it hasn't been calculated yet.
    fn get_boundary_outside(&mut self, train: &ExtruderTrain) -> &Shape {
        let nr = train.extruder_nr;
        let needs = self.boundary_outside.get(&nr).map(|s| s.is_empty()).unwrap_or(true);
        if needs {
            let travel_avoid_supports = train.settings.get_bool("travel_avoid_supports");
            let shape = self
                .storage()
                .get_layer_outlines(self.layer_nr, travel_avoid_supports, travel_avoid_supports, false, -1, true)
                .offset_default(self.travel_avoid_distance);
            self.boundary_outside.insert(nr, Box::new(shape));
        }
        &self.boundary_outside[&nr]
    }

    /// `getModelBoundary`. NOTE: like the C++ code this computes `model_boundary_` but returns
    /// `boundary_outside_` (which is default-constructed when it hasn't been computed yet).
    fn get_model_boundary(&mut self, train: &ExtruderTrain) -> &Shape {
        let nr = train.extruder_nr;
        let needs = self.model_boundary.get(&nr).map(|s| s.is_empty()).unwrap_or(true);
        if needs {
            let travel_avoid_supports = train.settings.get_bool("travel_avoid_supports");
            let shape = self.storage().get_layer_outlines(self.layer_nr, travel_avoid_supports, travel_avoid_supports, false, -1, true);
            self.model_boundary.insert(nr, Box::new(shape));
        }
        self.boundary_outside.entry(nr).or_default()
    }

    /// Get the SparsePointGridInclusive mapping locations to line segments of the model boundary.
    fn get_model_boundary_loc_to_line(&mut self, train: &ExtruderTrain) -> &LocToLineGrid {
        let nr = train.extruder_nr;
        if !self.model_boundary_loc_to_line.contains_key(&nr) {
            let square = self.offset_from_inside_to_outside * 3 / 2;
            let grid = PolygonUtils::create_loc_to_line_grid(self.get_model_boundary(train), square as i32);
            self.model_boundary_loc_to_line.insert(nr, grid);
        }
        &self.model_boundary_loc_to_line[&nr]
    }
}

pub struct Comb {
    /// Offset from the boundary of a part to the comb path. (nozzle width / 2)
    offset_from_outlines: Coord,
    /// Maximal distance of a point to the Comb::boundary_inside which is still to be considered inside.
    max_move_inside_distance2: Coord,
    /// Enlarged distance for moving points inside.
    max_move_inside_distance_enlarged2: Coord,

    /// The boundary within which to comb. (Will be reordered by the partsView_inside_minimum)
    /// Boxed: the location-to-line grids keep raw pointers to this shape, so it must not move with the Comb.
    boundary_inside_minimum: Box<Shape>,
    /// The boundary within which to comb. (Will be reordered by the partsView_inside_optimal)
    boundary_inside_optimal: Box<Shape>,
    parts_view_inside_minimum: PartsView,
    parts_view_inside_optimal: PartsView,
    inside_loc_to_line_minimum: Box<LocToLineGrid>,
    inside_loc_to_line_optimal: Box<LocToLineGrid>,
    outside: CombOutside,
    /// When using comb_boundary_inside_minimum for combing it tries to move points inside by this amount.
    move_inside_distance: Coord,
}

impl Comb {
    /// Initialises the combing areas for every mesh in the layer (not support).
    pub fn new(
        storage: &SliceDataStorage,
        layer_nr: LayerIndex,
        comb_boundary_inside_minimum: &Shape,
        comb_boundary_inside_optimal: &Shape,
        comb_boundary_offset: Coord,
        travel_avoid_distance: Coord,
        move_inside_distance: Coord,
    ) -> Self {
        let offset_from_outlines = comb_boundary_offset; // between second wall and infill / other walls
        let offset_from_inside_to_outside = offset_from_outlines + travel_avoid_distance;
        // copy the boundary, because the partsView_inside will reorder the polygons
        let mut boundary_inside_minimum = Box::new(comb_boundary_inside_minimum.clone());
        let mut boundary_inside_optimal = Box::new(comb_boundary_inside_optimal.clone());
        let parts_view_inside_minimum = boundary_inside_minimum.split_into_parts_view(false); // WARNING !! changes the order of boundary_inside !!
        let parts_view_inside_optimal = boundary_inside_optimal.split_into_parts_view(false); // WARNING !! changes the order of boundary_inside !!
        let inside_loc_to_line_minimum = PolygonUtils::create_loc_to_line_grid(&boundary_inside_minimum, comb_boundary_offset as i32);
        let inside_loc_to_line_optimal = PolygonUtils::create_loc_to_line_grid(&boundary_inside_optimal, comb_boundary_offset as i32);
        Comb {
            offset_from_outlines,
            max_move_inside_distance2: offset_from_outlines * offset_from_outlines,
            max_move_inside_distance_enlarged2: ((offset_from_outlines + MAX_MOVE_INSIDE_ENLARGE_DISTANCE) as f64).powi(2) as Coord,
            boundary_inside_minimum,
            boundary_inside_optimal,
            parts_view_inside_minimum,
            parts_view_inside_optimal,
            inside_loc_to_line_minimum,
            inside_loc_to_line_optimal,
            outside: CombOutside {
                storage: storage as *const SliceDataStorage,
                layer_nr,
                travel_avoid_distance,
                offset_from_inside_to_outside,
                // so max_crossing_dist = offset_from_inside_to_outside * sqrt(2) =approx 1.5 to allow for slightly diagonal crossings and slightly inaccurate crossing computation
                max_crossing_dist2: offset_from_inside_to_outside * offset_from_inside_to_outside * 2,
                boundary_outside: HashMap::new(),
                model_boundary: HashMap::new(),
                outside_loc_to_line: HashMap::new(),
                model_boundary_loc_to_line: HashMap::new(),
            },
            move_inside_distance,
        }
    }

    pub fn offset_from_outlines(&self) -> Coord {
        self.offset_from_outlines
    }

    /// Move the startPoint or endPoint inside when it should be inside
    fn move_inside(
        boundary_inside: &Shape,
        is_inside: bool,
        inside_loc_to_line: &LocToLineGrid,
        dest_point: &mut Point2LL,
        inside_poly: &mut usize,
        max_move_inside_distance_squared: Coord,
    ) -> bool {
        if is_inside {
            let cpp = PolygonUtils::ensure_inside_or_outside(
                boundary_inside,
                dest_point,
                OFFSET_EXTRA_START_END as i32,
                max_move_inside_distance_squared,
                Some(boundary_inside),
                Some(inside_loc_to_line),
                NO_PENALTY,
            );
            if !cpp.is_valid() {
                return false;
            } else {
                *inside_poly = cpp.poly_idx;
                return true;
            }
        }
        false
    }

    /// Try to move comb_path_input points inside by the amount of `move_inside_distance` and see if
    /// the points are still in boundary_inside_optimal, add result in comb_path_output
    fn move_comb_path_inside(&self, comb_path_input: &CombPath, comb_path_output: &mut CombPath) {
        let boundary_inside = &self.boundary_inside_minimum;
        let boundary_inside_optimal = &self.boundary_inside_optimal;
        let dist = self.move_inside_distance;
        let dist2 = dist * dist;

        if comb_path_input.is_empty() {
            return;
        }
        comb_path_output.push(comb_path_input[0]);
        for point_idx in 1..comb_path_input.len().saturating_sub(1) {
            let mut new_point = comb_path_input[point_idx];
            PolygonUtils::move_inside(boundary_inside, &mut new_point, dist as i32, dist2);

            if boundary_inside_optimal.inside(new_point, false) {
                comb_path_output.push(new_point);
            } else {
                comb_path_output.push(comb_path_input[point_idx]);
            }
        }
        if comb_path_input.len() > 1 {
            comb_path_output.push(comb_path_input[comb_path_input.len() - 1]);
        }
    }

    /// Calculate the comb paths (if any), one for each polygon combed alternated with travel paths.
    ///
    /// Returns whether combing has succeeded; otherwise a retraction is needed.
    #[allow(clippy::too_many_arguments)]
    pub fn calc(
        &mut self,
        perform_z_hops: bool,
        perform_z_hops_only_when_collides: bool,
        train: &ExtruderTrain,
        mut start_point: Point2LL,
        mut end_point: Point2LL,
        comb_paths: &mut CombPaths,
        start_inside_: bool,
        end_inside_: bool,
        max_comb_distance_ignored: Coord,
        unretract_before_last_travel_move: &mut bool,
        do_retracted_move: &mut bool,
    ) -> bool {
        if shorter_then(end_point - start_point, max_comb_distance_ignored) {
            return true;
        }
        let travel_end_point_before_combing = end_point;
        let offset = -OFFSET_DIST_TO_GET_FROM_ON_THE_POLYGON_TO_OUTSIDE;

        // Move start and end point inside the optimal comb boundary
        let mut start_inside_poly = NO_INDEX;
        let start_inside = Self::move_inside(
            &self.boundary_inside_optimal,
            start_inside_,
            &self.inside_loc_to_line_optimal,
            &mut start_point,
            &mut start_inside_poly,
            self.max_move_inside_distance2,
        );

        let mut end_inside_poly = NO_INDEX;
        let end_inside =
            Self::move_inside(&self.boundary_inside_optimal, end_inside_, &self.inside_loc_to_line_optimal, &mut end_point, &mut end_inside_poly, self.max_move_inside_distance2);

        let mut start_part_boundary_poly_idx = NO_INDEX;
        let mut end_part_boundary_poly_idx = NO_INDEX;
        let start_part_idx =
            if start_inside_poly == NO_INDEX { NO_INDEX } else { self.parts_view_inside_optimal.get_part_containing(start_inside_poly, Some(&mut start_part_boundary_poly_idx)) };
        let end_part_idx =
            if end_inside_poly == NO_INDEX { NO_INDEX } else { self.parts_view_inside_optimal.get_part_containing(end_inside_poly, Some(&mut end_part_boundary_poly_idx)) };

        let fail_on_unavoidable_obstacles = perform_z_hops && perform_z_hops_only_when_collides;

        // normal combing within part using optimal comb boundary
        if start_inside && end_inside && start_part_idx == end_part_idx {
            let part = self.parts_view_inside_optimal.assemble_part(&self.boundary_inside_optimal, start_part_idx);
            comb_paths.push(CombPath::default());
            let combing_succeeded = LinePolygonsCrossings::comb(
                &part.0,
                &self.inside_loc_to_line_optimal,
                start_point,
                end_point,
                comb_paths.last_mut().unwrap(),
                offset,
                max_comb_distance_ignored,
                fail_on_unavoidable_obstacles,
            );
            // If the endpoint of the travel path changes with combing, then it means that we are moving to an outer wall
            // and we should unretract before the last travel move when travelling to that outer wall
            *unretract_before_last_travel_move = combing_succeeded && end_point != travel_end_point_before_combing;
            return combing_succeeded;
        }

        // Move start and end point inside the optimal comb boundary
        // Give more tolerancy when calculating move inside positions, because the target points in this case will be on the borders
        let mut start_inside_poly_optimal = NO_INDEX;
        let start_inside_optimal = Self::move_inside(
            &self.boundary_inside_optimal,
            start_inside_,
            &self.inside_loc_to_line_optimal,
            &mut start_point,
            &mut start_inside_poly_optimal,
            self.max_move_inside_distance_enlarged2,
        );

        let mut end_inside_poly_optimal = NO_INDEX;
        let end_inside_optimal = Self::move_inside(
            &self.boundary_inside_optimal,
            end_inside_,
            &self.inside_loc_to_line_optimal,
            &mut end_point,
            &mut end_inside_poly_optimal,
            self.max_move_inside_distance_enlarged2,
        );

        let mut start_part_boundary_poly_idx_optimal: usize = 0;
        let mut end_part_boundary_poly_idx_optimal: usize = 0;
        let start_part_idx_optimal = if start_inside_poly_optimal == NO_INDEX {
            NO_INDEX
        } else {
            self.parts_view_inside_optimal.get_part_containing(start_inside_poly_optimal, Some(&mut start_part_boundary_poly_idx_optimal))
        };
        let end_part_idx_optimal = if end_inside_poly_optimal == NO_INDEX {
            NO_INDEX
        } else {
            self.parts_view_inside_optimal.get_part_containing(end_inside_poly_optimal, Some(&mut end_part_boundary_poly_idx_optimal))
        };

        let mut result_path = CombPath::default();

        if start_inside_optimal && end_inside_optimal && start_part_idx_optimal == end_part_idx_optimal {
            let part = self.parts_view_inside_optimal.assemble_part(&self.boundary_inside_optimal, start_part_idx_optimal);
            comb_paths.push(CombPath::default());

            let comb_result = LinePolygonsCrossings::comb(
                &part.0,
                &self.inside_loc_to_line_optimal,
                start_point,
                end_point,
                &mut result_path,
                offset,
                max_comb_distance_ignored,
                fail_on_unavoidable_obstacles,
            );
            let mut out = std::mem::take(comb_paths.last_mut().unwrap());
            self.move_comb_path_inside(&result_path, &mut out); // add altered result_path to combPaths.back()
            *comb_paths.last_mut().unwrap() = out;
            *unretract_before_last_travel_move = comb_result && end_point != travel_end_point_before_combing;
            return comb_result;
        }

        // Move start and end point inside the minimum comb boundary
        let mut start_inside_poly_min = NO_INDEX;
        let start_inside_min = Self::move_inside(
            &self.boundary_inside_minimum,
            start_inside_,
            &self.inside_loc_to_line_minimum,
            &mut start_point,
            &mut start_inside_poly_min,
            self.max_move_inside_distance2,
        );

        let mut end_inside_poly_min = NO_INDEX;
        let end_inside_min = Self::move_inside(
            &self.boundary_inside_minimum,
            end_inside_,
            &self.inside_loc_to_line_minimum,
            &mut end_point,
            &mut end_inside_poly_min,
            self.max_move_inside_distance2,
        );

        let mut start_part_boundary_poly_idx_min: usize = 0;
        let mut end_part_boundary_poly_idx_min: usize = 0;
        let start_part_idx_min = if start_inside_poly_min == NO_INDEX {
            NO_INDEX
        } else {
            self.parts_view_inside_minimum.get_part_containing(start_inside_poly_min, Some(&mut start_part_boundary_poly_idx_min))
        };
        let end_part_idx_min = if end_inside_poly_min == NO_INDEX {
            NO_INDEX
        } else {
            self.parts_view_inside_minimum.get_part_containing(end_inside_poly_min, Some(&mut end_part_boundary_poly_idx_min))
        };

        // normal combing within part using minimum comb boundary
        if start_inside_min && end_inside_min && start_part_idx_min == end_part_idx_min {
            let part = self.parts_view_inside_minimum.assemble_part(&self.boundary_inside_minimum, start_part_idx_min);
            comb_paths.push(CombPath::default());

            let comb_result = LinePolygonsCrossings::comb(
                &part.0,
                &self.inside_loc_to_line_minimum,
                start_point,
                end_point,
                &mut result_path,
                offset,
                max_comb_distance_ignored,
                fail_on_unavoidable_obstacles,
            );
            let mut out = std::mem::take(comb_paths.last_mut().unwrap());
            self.move_comb_path_inside(&result_path, &mut out); // add altered result_path to combPaths.back()
            *comb_paths.last_mut().unwrap() = out;
            *unretract_before_last_travel_move = comb_result && end_point != travel_end_point_before_combing;
            // When using the minimum comb boundary, the travel move may end up closer to the walls than expected, so do a retracted move
            *do_retracted_move = true;
            return comb_result;
        }

        // comb inside part to edge (if needed) >> move through air avoiding other parts >> comb inside end part upto the endpoint (if needed)

        if perform_z_hops && !perform_z_hops_only_when_collides {
            // Combing via outside makes combing fail.
            return false;
        }

        // Find the crossings using the minimum comb boundary, since it's guaranteed to be as close as we can get to the destination.
        // Getting as close as possible prevents exiting the polygon in the wrong direction (e.g. into a hole instead of to the outside).
        let mut start_crossing = Crossing::new(
            start_point,
            start_inside_min,
            start_part_idx_min as u32,
            start_part_boundary_poly_idx_min as u32,
            &self.boundary_inside_minimum,
            &self.inside_loc_to_line_minimum,
        );
        let mut end_crossing = Crossing::new(
            end_point,
            end_inside_min,
            end_part_idx_min as u32,
            end_part_boundary_poly_idx_min as u32,
            &self.boundary_inside_minimum,
            &self.inside_loc_to_line_minimum,
        );

        {
            // find crossing over the in-between area between inside and outside
            start_crossing.find_crossing_in_or_mid(&self.parts_view_inside_minimum, end_point);
            end_crossing.find_crossing_in_or_mid(&self.parts_view_inside_minimum, start_crossing.in_or_mid);
        }

        let offset_from_inside_to_outside = self.outside.offset_from_inside_to_outside;
        let mut skip_avoid_other_parts_path = false;
        if v_size2(start_crossing.in_or_mid - end_crossing.in_or_mid) < offset_from_inside_to_outside * offset_from_inside_to_outside * 4 {
            // parts are next to each other, i.e. the direct crossing will always be smaller than two crossings via outside
            skip_avoid_other_parts_path = true;
        }

        let travel_avoid_other_parts = train.settings.get_bool("travel_avoid_other_parts");

        if travel_avoid_other_parts && !skip_avoid_other_parts_path {
            // compute the crossing points when moving through air
            // comb through all air, since generally the outside consists of a single part
            let outside_boundary = self.outside.get_boundary_outside(train).clone();
            let success = start_crossing.find_outside(train, &outside_boundary, end_crossing.in_or_mid, fail_on_unavoidable_obstacles, &mut self.outside);
            if !success {
                return false;
            }
            let success = end_crossing.find_outside(train, &outside_boundary, start_crossing.out, fail_on_unavoidable_obstacles, &mut self.outside);
            if !success {
                return false;
            }
        }

        // generate the actual comb paths
        if start_inside_min {
            // start to boundary
            debug_assert!(!start_crossing.dest_part.is_empty(), "The part we start inside when combing should have been computed already!");
            comb_paths.push(CombPath::default());
            // If we're inside the optimal bound, first try the optimal combing path. If it fails, use the minimum path instead.
            let fail_for_optimum_bound = true;
            let mut combing_succeeded = start_inside
                && LinePolygonsCrossings::comb(
                    &self.boundary_inside_optimal,
                    &self.inside_loc_to_line_optimal,
                    start_point,
                    start_crossing.in_or_mid,
                    comb_paths.last_mut().unwrap(),
                    offset,
                    max_comb_distance_ignored,
                    fail_for_optimum_bound,
                );
            if !combing_succeeded {
                combing_succeeded = LinePolygonsCrossings::comb(
                    &start_crossing.dest_part.0,
                    &self.inside_loc_to_line_minimum,
                    start_point,
                    start_crossing.in_or_mid,
                    comb_paths.last_mut().unwrap(),
                    offset,
                    max_comb_distance_ignored,
                    fail_on_unavoidable_obstacles,
                );
            }
            if !combing_succeeded {
                // Couldn't comb between start point and computed crossing from the start part! Happens for very thin parts when the offset_to_get_off_boundary moves points to outside
                // the polygon
                return false;
            }
        }

        // through air from boundary to boundary
        if travel_avoid_other_parts && !skip_avoid_other_parts_path {
            comb_paths.push(CombPath::default());
            comb_paths.through_air = true;
            if v_size(start_crossing.in_or_mid - end_crossing.in_or_mid)
                < v_size(start_crossing.in_or_mid - start_crossing.out) + v_size(end_crossing.in_or_mid - end_crossing.out)
            {
                // via outside is moving more over the in-between zone
                comb_paths.last_mut().unwrap().push(start_crossing.in_or_mid);
                comb_paths.last_mut().unwrap().push(end_crossing.in_or_mid);
            } else {
                let mut tmp_comb_path = CombPath::default();
                let outside_boundary = self.outside.get_boundary_outside(train).clone();
                let grid = self.outside.get_outside_loc_to_line(train);
                let combing_succeeded = LinePolygonsCrossings::comb(
                    &outside_boundary,
                    grid,
                    start_crossing.out,
                    end_crossing.out,
                    &mut tmp_comb_path,
                    OFFSET_DIST_TO_GET_FROM_ON_THE_POLYGON_TO_OUTSIDE,
                    max_comb_distance_ignored,
                    true,
                );

                if combing_succeeded {
                    // add combing travel moves if the combing was successful
                    comb_paths.push(tmp_comb_path);
                } else {
                    // if combing is not possible then move directly to the target destination
                    // this happens for instance when trying to avoid skin-regions and combing from
                    // an origin that is on a hole-boundary to a destination that is on the outline-border
                    comb_paths.push(CombPath::default());
                    comb_paths.through_air = true;
                    let back = comb_paths.last_mut().unwrap();
                    back.cross_boundary = true;
                    back.push(start_crossing.in_or_mid);
                    back.push(end_crossing.in_or_mid);

                    if fail_on_unavoidable_obstacles {
                        return false;
                    }
                }
            }
        } else {
            // directly through air (not avoiding other parts)
            comb_paths.push(CombPath::default());
            comb_paths.through_air = true;
            let back = comb_paths.last_mut().unwrap();
            back.cross_boundary = true; // note: we don't actually know whether this is cross boundary, but it might very well be
            back.push(start_crossing.in_or_mid);
            back.push(end_crossing.in_or_mid);
        }
        if skip_avoid_other_parts_path {
            if start_inside == end_inside && start_part_idx == end_part_idx {
                if start_inside {
                    // both start and end are inside
                    comb_paths.last_mut().unwrap().cross_boundary =
                        PolygonUtils::polygon_collides_with_line_segment_grid(start_point, end_point, &self.inside_loc_to_line_optimal, None);
                } else {
                    // both start and end are outside
                    let grid = self.outside.get_model_boundary_loc_to_line(train);
                    let collides = PolygonUtils::polygon_collides_with_line_segment_grid(start_point, end_point, grid, None);
                    comb_paths.last_mut().unwrap().cross_boundary = collides;
                }
            } else {
                comb_paths.last_mut().unwrap().cross_boundary = true;
            }
        }

        if end_inside {
            // boundary to end
            debug_assert!(!end_crossing.dest_part.is_empty(), "The part we end up inside when combing should have been computed already!");
            comb_paths.push(CombPath::default());
            // If we're inside the optimal bound, first try the optimal combing path. If it fails, use the minimum path instead.
            let fail_for_optimum_bound = true;
            let mut combing_succeeded = end_inside
                && LinePolygonsCrossings::comb(
                    &self.boundary_inside_optimal,
                    &self.inside_loc_to_line_optimal,
                    end_crossing.in_or_mid,
                    end_point,
                    comb_paths.last_mut().unwrap(),
                    offset,
                    max_comb_distance_ignored,
                    fail_for_optimum_bound,
                );
            if !combing_succeeded {
                combing_succeeded = LinePolygonsCrossings::comb(
                    &end_crossing.dest_part.0,
                    &self.inside_loc_to_line_minimum,
                    end_crossing.in_or_mid,
                    end_point,
                    comb_paths.last_mut().unwrap(),
                    offset,
                    max_comb_distance_ignored,
                    fail_on_unavoidable_obstacles,
                );
            }
            // If the endpoint of the travel path changes with combing, then it means that we are moving to an outer wall
            // and we should unretract before the last travel move when traveling to that outer wall
            *unretract_before_last_travel_move = combing_succeeded && end_point != travel_end_point_before_combing;
            if !combing_succeeded {
                // Couldn't comb between end point and computed crossing to the end part! Happens for very thin parts when the offset_to_get_off_boundary moves points to outside the
                // polygon
                return false;
            }
        }

        true
    }
}
