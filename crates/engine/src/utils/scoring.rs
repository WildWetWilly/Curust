//! `utils/scoring/*`: criteria to score candidate elements and `BestElementFinder` to find the best one.

use crate::geometry::point2ll::{v_size, v_size_mm, Point2LL};
use crate::geometry::Shape;
use crate::settings::enums::EZSeamCornerPrefType;
use crate::texture_data_provider::{TextureArea, TextureDataProvider};
use crate::utils::coord::{int2mm, Coord};
use crate::utils::linear_alg_2d as la;
use std::f64::consts::PI;

/// `cura::inverse_lerp`
#[inline]
pub fn inverse_lerp(range_min: f64, range_max: f64, value: f64) -> f64 {
    if range_min == range_max {
        return 0.0;
    }
    (value - range_min) / (range_max - range_min)
}

/// `cura::randf<double>()`
#[inline]
pub fn randf() -> f64 {
    cppstd::crand::rand() as f64 / cppstd::crand::rand_max() as f64
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DistanceType {
    /// Classic euclidian distance between the points
    Euclidian,
    /// Only difference on X coordinate
    XOnly,
    /// Only difference on Y coordinate
    YOnly,
}

/// `DistanceScoringCriterion`: criterion that scores points according to their distance to a target.
#[derive(Clone, Debug)]
pub struct DistanceScoringCriterion<'a> {
    points: &'a [Point2LL],
    target_pos: Point2LL,
    distance_type: DistanceType,
    /// Fixed divider for shortest distances computation.
    distance_divider: f64,
}

impl<'a> DistanceScoringCriterion<'a> {
    pub fn new(points: &'a [Point2LL], target_pos: Point2LL, distance_type: DistanceType, distance_divider: f64) -> Self {
        DistanceScoringCriterion { points, target_pos, distance_type, distance_divider }
    }

    pub fn with_defaults(points: &'a [Point2LL], target_pos: Point2LL) -> Self {
        Self::new(points, target_pos, DistanceType::Euclidian, 20.0)
    }

    fn compute_score_pos(&self, candidate_position: Point2LL) -> f64 {
        let distance = match self.distance_type {
            // Use actual (non-squared) distance to ensure a proper scoring distribution
            DistanceType::Euclidian => v_size_mm(candidate_position - self.target_pos),
            DistanceType::XOnly => int2mm((candidate_position.x - self.target_pos.x).abs()),
            DistanceType::YOnly => int2mm((candidate_position.y - self.target_pos.y).abs()),
        };
        // Use reciprocal function to normalize distance score decreasingly
        1.0 / (1.0 + (distance / self.distance_divider))
    }
}

/// `CornerScoringCriterion`: criterion that scores points according to the sharpness of the corner.
#[derive(Clone, Debug)]
pub struct CornerScoringCriterion<'a> {
    points: &'a [Point2LL],
    corner_preference: EZSeamCornerPrefType,
    /// Contains the size of each segment of the polygon (segment i goes from point i to point i+1)
    segments_sizes: Vec<Coord>,
    total_length: Coord,
}

impl<'a> CornerScoringCriterion<'a> {
    pub fn new(points: &'a [Point2LL], corner_preference: EZSeamCornerPrefType) -> Self {
        let mut segments_sizes = vec![0; points.len()];
        let mut total_length: Coord = 0;
        // Pre-calculate the segments lengths because we are going to need them multiple times
        for i in 0..points.len() {
            let here = points[i];
            let next = points[(i + 1) % points.len()];
            let segment_size = v_size(next - here);
            segments_sizes[i] = segment_size;
            total_length += segment_size;
        }
        CornerScoringCriterion { points, corner_preference, segments_sizes, total_length }
    }

    fn compute_score(&self, candidate_index: usize) -> f64 {
        let corner_angle = self.corner_angle(candidate_index, 1000);
        // angles < 0 are concave (left turning)
        // angles > 0 are convex (right turning)
        match self.corner_preference {
            // Give advantage to concave corners. More advantage for sharper corners.
            EZSeamCornerPrefType::Z_SEAM_CORNER_PREF_INNER => inverse_lerp(1.0, -1.0, corner_angle),
            // Give advantage to convex corners. More advantage for sharper corners.
            EZSeamCornerPrefType::Z_SEAM_CORNER_PREF_OUTER => inverse_lerp(-1.0, 1.0, corner_angle),
            // Still give sharper corners more advantage.
            EZSeamCornerPrefType::Z_SEAM_CORNER_PREF_ANY => corner_angle.abs(),
            // Give sharper corners some advantage, but sharper concave corners even more.
            EZSeamCornerPrefType::Z_SEAM_CORNER_PREF_WEIGHTED => {
                if corner_angle < 0.0 {
                    -corner_angle
                } else {
                    corner_angle / 2.0
                }
            }
            EZSeamCornerPrefType::PLUGIN => 0.0,
        }
    }

    /// Some angle related to the corner at the vertex: -1 is a very sharp concave corner, 0 is a straight line, 1 is a very sharp convex corner.
    fn corner_angle(&self, vertex_index: usize, angle_query_distance: Coord) -> f64 {
        let bounded_distance = angle_query_distance.min(self.total_length / 2);
        let here = self.points[vertex_index];
        let next = self.find_neighbour_point(vertex_index, bounded_distance);
        let previous = self.find_neighbour_point(vertex_index, -bounded_distance);

        let angle = la::get_angle_left(previous, here, next) - PI;

        angle / PI
    }

    /// Finds a neighbour point on the path, located before or after the given reference point.
    fn find_neighbour_point(&self, vertex_index: usize, mut distance: Coord) -> Point2LL {
        let size = self.points.len() as u64;
        let direction: i32 = if distance > 0 { 1 } else { -1 };
        let size_delta: i32 = if distance > 0 { -1 } else { 0 };
        distance = distance.abs();

        // Travel on the path until we reach the distance
        let mut actual_delta: i32 = 0;
        let mut travelled_distance: Coord = 0;
        let mut segment_size: Coord = 0;
        // NOTE: mixed size_t/int arithmetic wraps modulo 2^64 like the C++ code.
        let idx = |delta: i32| -> usize { ((vertex_index as u64).wrapping_add(delta as i64 as u64).wrapping_add(size) % size) as usize };
        while travelled_distance < distance {
            actual_delta += direction;
            segment_size = self.segments_sizes[idx(actual_delta + size_delta)];
            travelled_distance += segment_size;
        }

        let next_pos = self.points[idx(actual_delta)];

        if travelled_distance > distance {
            // We have overtaken the required distance, go backward on the last segment
            let prev = idx(actual_delta - direction) as i32 as usize;
            let prev_pos = self.points[prev];

            let vector = next_pos - prev_pos;
            let unit_vector = Point2LL::new((vector.x * 1000) / segment_size, (vector.y * 1000) / segment_size);
            let f = segment_size - (travelled_distance - distance);
            let vector_delta = Point2LL::new(unit_vector.x * f, unit_vector.y * f);
            prev_pos + Point2LL::new(vector_delta.x / 1000, vector_delta.y / 1000)
        } else {
            // Luckily, the required distance stops exactly on an existing point
            next_pos
        }
    }
}

/// `ExclusionAreaScoringCriterion`: points inside the area get a score of 0, others 1.
#[derive(Clone, Debug)]
pub struct ExclusionAreaScoringCriterion<'a> {
    points: &'a [Point2LL],
    exclusion_area: &'a Shape,
}

impl<'a> ExclusionAreaScoringCriterion<'a> {
    pub fn new(points: &'a [Point2LL], exclusion_area: &'a Shape) -> Self {
        ExclusionAreaScoringCriterion { points, exclusion_area }
    }
}

/// `TextureScoringCriterion`: scores points according to the painted texture preference of a feature.
#[derive(Clone)]
pub struct TextureScoringCriterion<'a> {
    points: &'a [Point2LL],
    texture_data_provider: &'a TextureDataProvider,
    feature_name: &'static str,
}

impl<'a> TextureScoringCriterion<'a> {
    pub fn new(points: &'a [Point2LL], texture_data_provider: &'a TextureDataProvider, feature_name: &'static str) -> Self {
        TextureScoringCriterion { points, texture_data_provider, feature_name }
    }

    fn compute_score_pos(&self, candidate_position: Point2LL) -> f64 {
        match self.texture_data_provider.get_area_preference(&candidate_position, self.feature_name) {
            Some(TextureArea::NORMAL) => 0.5,
            Some(TextureArea::PREFERRED) => 1.0,
            Some(TextureArea::AVOID) => 0.0,
            _ => 0.5,
        }
    }
}

impl std::fmt::Debug for TextureScoringCriterion<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TextureScoringCriterion({})", self.feature_name)
    }
}

/// A scoring criterion (`ScoringCriterion` and its subclasses).
#[derive(Clone, Debug)]
pub enum ScoringCriterion<'a> {
    Distance(DistanceScoringCriterion<'a>),
    Corner(CornerScoringCriterion<'a>),
    Random,
    ExclusionArea(ExclusionAreaScoringCriterion<'a>),
    Texture(TextureScoringCriterion<'a>),
}

impl<'a> ScoringCriterion<'a> {
    pub fn compute_score(&self, candidate_index: usize) -> f64 {
        match self {
            ScoringCriterion::Distance(c) => c.compute_score_pos(c.points[candidate_index]),
            ScoringCriterion::Corner(c) => c.compute_score(candidate_index),
            ScoringCriterion::Random => randf(),
            ScoringCriterion::ExclusionArea(c) => {
                if c.exclusion_area.inside(c.points[candidate_index], true) {
                    0.0
                } else {
                    1.0
                }
            }
            ScoringCriterion::Texture(c) => c.compute_score_pos(c.points[candidate_index]),
        }
    }
}

#[derive(Clone, Debug)]
pub struct WeighedCriterion<'a> {
    pub criterion: ScoringCriterion<'a>,
    /// The weight to be given when using this criterion to compute the global score.
    pub weight: f64,
}

#[derive(Clone, Debug, Default)]
pub struct CriteriaPass<'a> {
    pub criteria: Vec<WeighedCriterion<'a>>,
    /// Once the scores have been calculated, the candidates which have a score below this delta from the best one are kept.
    pub outsider_delta_threshold: f64,
}

#[derive(Clone, Copy, Debug)]
struct Candidate {
    candidate_index: usize,
    score: f64,
}

/// `BestElementFinder`: finds the element with the best score, by applying successive criteria passes.
#[derive(Clone, Debug, Default)]
pub struct BestElementFinder<'a> {
    criteria: Vec<CriteriaPass<'a>>,
}

impl<'a> BestElementFinder<'a> {
    pub fn new() -> Self {
        BestElementFinder { criteria: Vec::new() }
    }

    pub fn append_criteria_pass(&mut self, pass: CriteriaPass<'a>) {
        self.criteria.push(pass);
    }

    pub fn append_single_criterion_pass(&mut self, criterion: ScoringCriterion<'a>, outsider_delta_threshold: f64) {
        self.append_criteria_pass(CriteriaPass { criteria: vec![WeighedCriterion { criterion, weight: 1.0 }], outsider_delta_threshold });
    }

    pub fn find_best_element(&self, candidates_count: usize) -> Option<usize> {
        // Start by initializing the candidates list in natural order
        let mut best_candidates: Vec<Candidate> = (0..candidates_count).map(|i| Candidate { candidate_index: i, score: 0.0 }).collect();

        // Now run the criteria passes until we have a single outsider or no more criteria
        for (pass_index, criteria_pass) in self.criteria.iter().enumerate() {
            // For each element, reset score, process each criterion and apply weights to get the global score
            let mut best: Option<usize> = None;
            for i in 0..best_candidates.len() {
                let mut score = 0.0;
                for weighed_criterion in &criteria_pass.criteria {
                    score += weighed_criterion.criterion.compute_score(best_candidates[i].candidate_index) * weighed_criterion.weight;
                }
                best_candidates[i].score = score;
                if best.is_none() || score > best_candidates[best.unwrap()].score {
                    best = Some(i);
                }
            }

            let best = match best {
                // Something went wrong, we don't have a best candidate
                None => return None,
                Some(b) => b,
            };

            // Early out for last pass, just keep the best candidate
            if pass_index == self.criteria.len() - 1 {
                return Some(best_candidates[best].candidate_index);
            }

            // Skip candidates that have a score too far from the actual best one
            let delta_threshold = criteria_pass.outsider_delta_threshold + f64::EPSILON;
            // NOTE: emulates libstdc++'s std::remove_if exactly. The C++ predicate reads the best score through an
            // iterator into the range being compacted, so once elements have been moved over that position the
            // predicate compares against the moved-in element. This is reproduced here.
            let n = best_candidates.len();
            let pred = |arr: &Vec<Candidate>, c: Candidate| arr[best].score - c.score > delta_threshold;
            let mut first = 0usize;
            while first < n && !pred(&best_candidates, best_candidates[first]) {
                first += 1;
            }
            let new_end = if first == n {
                n
            } else {
                let mut result = first;
                first += 1;
                while first < n {
                    let c = best_candidates[first];
                    if !pred(&best_candidates, c) {
                        best_candidates[result] = c;
                        result += 1;
                    }
                    first += 1;
                }
                result
            };
            best_candidates.truncate(new_end);

            if best_candidates.len() == 1 {
                // We have a single outsider, don't go further
                return Some(best_candidates[0].candidate_index);
            }
        }

        best_candidates.first().map(|c| c.candidate_index)
    }
}
