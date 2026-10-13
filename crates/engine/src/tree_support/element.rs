//! `TreeSupportElement.h`.
//!
//! The C++ code allocates elements with `new` and links them with raw pointers. Here every
//! heap-allocated element lives in an [`ElemArena`]; an [`ElemId`] is its allocation serial. The
//! reference engine is patched (see `tools/difftest/reference`) to order its pointer-keyed
//! containers by the same allocation serial, so `BTreeSet<ElemId>` reproduces
//! `std::set<TreeSupportElement*>`.

use crate::geometry::lines_set::Shape;
use crate::geometry::point2ll::{pt, Point2LL};
use crate::settings::types::LayerIndex;
use crate::utils::coord::Coord;

use super::settings::{base_circle, cmax, cmin, AvoidanceType, BASE_RADIUS};

pub type ElemId = usize;

/// `AreaIncreaseSettings`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AreaIncreaseSettings {
    pub type_: AvoidanceType,
    pub increase_speed: Coord,
    pub increase_radius: bool,
    pub no_error: bool,
    pub use_min_distance: bool,
    pub move_: bool,
}

impl Default for AreaIncreaseSettings {
    fn default() -> Self {
        AreaIncreaseSettings { type_: AvoidanceType::Fast, increase_speed: 0, increase_radius: false, no_error: false, use_min_distance: false, move_: false }
    }
}

impl AreaIncreaseSettings {
    pub fn new(type_: AvoidanceType, increase_speed: Coord, increase_radius: bool, simplify: bool, use_min_distance: bool, move_: bool) -> Self {
        AreaIncreaseSettings { type_, increase_speed, increase_radius, no_error: simplify, use_min_distance, move_ }
    }
}

/// `TreeSupportElement` (without its `area_` pointer, which lives in the arena).
#[derive(Clone, Debug)]
pub struct TreeSupportElement {
    pub target_height: LayerIndex,
    pub target_position: Point2LL,
    pub next_position: Point2LL,
    pub next_height: LayerIndex,
    pub effective_radius_height: usize,
    pub to_buildplate: bool,
    pub parents: Vec<ElemId>,
    pub distance_to_top: usize,
    pub result_on_layer: Point2LL,
    pub increased_to_model_radius: Coord,
    pub to_model_gracious: bool,
    pub buildplate_radius_increases: f64,
    pub use_min_xy_dist: bool,
    pub supports_roof: bool,
    pub dont_move_until: usize,
    pub can_use_safe_radius: bool,
    pub last_area_increase: AreaIncreaseSettings,
    pub missing_roof_layers: usize,
    pub skip_ovalisation: bool,
    pub all_tips: Vec<Point2LL>,
    pub influence_area_limit_active: bool,
    pub influence_area_limit_range: Coord,
    pub influence_area_limit_area: Shape,
    pub additional_ovalization_targets: Vec<Point2LL>,
}

pub const UNSET: Point2LL = pt(-1, -1);

/// Key of the maps ordered by `TreeSupportElement::operator<`: lexicographic on
/// (target_height, X, Y); equality is on target position and height only.
pub type ElemKey = (LayerIndex, Coord, Coord);

impl TreeSupportElement {
    /// The tip constructor.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        distance_to_top: Coord,
        target_height: usize,
        target_position: Point2LL,
        to_buildplate: bool,
        to_model_gracious: bool,
        use_min_xy_dist: bool,
        dont_move_until: usize,
        supports_roof: bool,
        can_use_safe_radius: bool,
        force_tips_to_roof: bool,
        skip_ovalisation: bool,
        influence_area_limit_active: bool,
        influence_area_limit_range: Coord,
    ) -> Self {
        let mut e = TreeSupportElement {
            target_height: target_height as LayerIndex,
            target_position,
            next_position: target_position,
            next_height: target_height as LayerIndex,
            effective_radius_height: distance_to_top as usize,
            to_buildplate,
            parents: Vec::new(),
            distance_to_top: distance_to_top as usize,
            result_on_layer: target_position,
            increased_to_model_radius: 0,
            to_model_gracious,
            buildplate_radius_increases: 0.0,
            use_min_xy_dist,
            supports_roof,
            dont_move_until,
            can_use_safe_radius,
            last_area_increase: AreaIncreaseSettings::new(AvoidanceType::Fast, 0, false, false, false, false),
            missing_roof_layers: if force_tips_to_roof { dont_move_until } else { 0 },
            skip_ovalisation,
            all_tips: vec![target_position],
            influence_area_limit_active,
            influence_area_limit_range,
            influence_area_limit_area: Shape::default(),
            additional_ovalization_targets: Vec::new(),
        };
        e.recreate_influence_limit_area();
        e
    }

    /// `TreeSupportElement(TreeSupportElement* element_above)`.
    pub fn from_above(above: &TreeSupportElement, above_id: ElemId) -> Self {
        TreeSupportElement {
            target_height: above.target_height,
            target_position: above.target_position,
            next_position: above.next_position,
            next_height: above.next_height,
            effective_radius_height: above.effective_radius_height,
            to_buildplate: above.to_buildplate,
            parents: vec![above_id],
            distance_to_top: above.distance_to_top + 1,
            result_on_layer: UNSET,
            increased_to_model_radius: above.increased_to_model_radius,
            to_model_gracious: above.to_model_gracious,
            buildplate_radius_increases: above.buildplate_radius_increases,
            use_min_xy_dist: above.use_min_xy_dist,
            supports_roof: above.supports_roof,
            dont_move_until: above.dont_move_until,
            can_use_safe_radius: above.can_use_safe_radius,
            last_area_increase: above.last_area_increase,
            missing_roof_layers: above.missing_roof_layers,
            skip_ovalisation: false,
            all_tips: above.all_tips.clone(),
            influence_area_limit_active: above.influence_area_limit_active,
            influence_area_limit_range: above.influence_area_limit_range,
            influence_area_limit_area: above.influence_area_limit_area.clone(),
            // Not copied by this constructor.
            additional_ovalization_targets: Vec::new(),
        }
    }

    /// `TreeSupportElement(const TreeSupportElement&, Shape*)`: copies everything except the
    /// additional ovalization targets (which the copy constructor does not copy either).
    pub fn copy_for_new_area(elem: &TreeSupportElement) -> Self {
        let mut e = elem.clone();
        e.additional_ovalization_targets = Vec::new();
        e
    }

    /// The merge constructor. `arena` is needed for `setToBuildplateForAllParents`.
    #[allow(clippy::too_many_arguments)]
    pub fn merged(
        first: &TreeSupportElement,
        second: &TreeSupportElement,
        next_height: LayerIndex,
        next_position: Point2LL,
        increased_to_model_radius: Coord,
        get_radius: &dyn Fn(usize, f64) -> Coord,
        diameter_scale_bp_radius: f64,
        branch_radius: Coord,
        diameter_angle_scale_factor: f64,
        arena: &mut ElemArena,
    ) -> Self {
        let (target_height, target_position) = if first.target_height > second.target_height {
            (first.target_height, first.target_position)
        } else {
            (second.target_height, second.target_position)
        };
        let effective_radius_height = cmax(first.effective_radius_height, second.effective_radius_height);
        let distance_to_top = cmax(first.distance_to_top, second.distance_to_top);
        let to_buildplate = first.to_buildplate && second.to_buildplate;
        let to_model_gracious = first.to_model_gracious && second.to_model_gracious;
        let mut parents = first.parents.clone();
        parents.extend_from_slice(&second.parents);
        let mut buildplate_radius_increases = 0.0;
        if diameter_scale_bp_radius > 0.0 {
            let foot_increase_radius = (cmax(
                get_radius(second.effective_radius_height, second.buildplate_radius_increases),
                get_radius(first.effective_radius_height, first.buildplate_radius_increases),
            ) - get_radius(effective_radius_height, 0.0))
            .abs();
            buildplate_radius_increases =
                foot_increase_radius as f64 / (branch_radius as f64 * (diameter_scale_bp_radius - diameter_angle_scale_factor));
        }
        let fl = &first.last_area_increase;
        let sl = &second.last_area_increase;
        let last_area_increase = AreaIncreaseSettings::new(
            cmin(fl.type_, sl.type_),
            cmin(fl.increase_speed, sl.increase_speed),
            fl.increase_radius || sl.increase_radius,
            fl.no_error || sl.no_error,
            fl.use_min_distance && sl.use_min_distance,
            fl.move_ || sl.move_,
        );
        let mut all_tips = first.all_tips.clone();
        all_tips.extend_from_slice(&second.all_tips);
        let mut e = TreeSupportElement {
            target_height,
            target_position,
            next_position,
            next_height,
            effective_radius_height,
            to_buildplate,
            parents,
            distance_to_top,
            result_on_layer: UNSET,
            increased_to_model_radius,
            to_model_gracious,
            buildplate_radius_increases,
            use_min_xy_dist: first.use_min_xy_dist || second.use_min_xy_dist,
            supports_roof: first.supports_roof || second.supports_roof,
            dont_move_until: cmax(first.dont_move_until, second.dont_move_until),
            can_use_safe_radius: first.can_use_safe_radius || second.can_use_safe_radius,
            last_area_increase,
            missing_roof_layers: cmin(first.missing_roof_layers, second.missing_roof_layers),
            skip_ovalisation: false,
            all_tips,
            influence_area_limit_active: first.influence_area_limit_active || second.influence_area_limit_active,
            influence_area_limit_range: cmax(first.influence_area_limit_range, second.influence_area_limit_range),
            influence_area_limit_area: Shape::default(),
            additional_ovalization_targets: Vec::new(),
        };
        e.recreate_influence_limit_area();
        if first.to_buildplate != second.to_buildplate {
            let v = e.to_buildplate;
            e.to_buildplate = v;
            e.to_model_gracious |= v;
            arena.set_to_buildplate_for_parents(&e.parents.clone(), v);
        }
        e
    }

    #[inline]
    pub fn key(&self) -> ElemKey {
        (self.target_height, self.target_position.x, self.target_position.y)
    }

    #[inline]
    pub fn is_result_on_layer_set(&self) -> bool {
        self.result_on_layer != UNSET
    }

    /// `RecreateInfluenceLimitArea`.
    pub fn recreate_influence_limit_area(&mut self) {
        if self.influence_area_limit_active {
            self.influence_area_limit_area = Shape::default();
            for &p in &self.all_tips {
                let mut pts = Vec::with_capacity(base_circle().len());
                for corner in base_circle().iter() {
                    // `corner * influence_area_limit_range_ / double(base_radius)`: the double converts back
                    // to coord_t for the only available `operator/(Point2LL, coord_t)`.
                    pts.push(p + (*corner * self.influence_area_limit_range) / BASE_RADIUS);
                }
                let circle = crate::geometry::polyline::Polygon::from_points(pts);
                if self.influence_area_limit_area.is_empty() {
                    self.influence_area_limit_area = circle.offset_default(0);
                } else {
                    self.influence_area_limit_area = self.influence_area_limit_area.intersection(&circle.offset_default(0));
                }
            }
        }
    }
}

/// The heap of `new`-allocated elements (with their `area_`).
#[derive(Default)]
pub struct ElemArena {
    pub elems: Vec<TreeSupportElement>,
    pub areas: Vec<Shape>,
}

impl ElemArena {
    /// `new TreeSupportElement(elem, new Shape(area))` (or the tip constructor followed by setting the area).
    pub fn alloc(&mut self, elem: TreeSupportElement, area: Shape) -> ElemId {
        self.elems.push(elem);
        self.areas.push(area);
        self.elems.len() - 1
    }

    /// `setToBuildplateForAllParents` applied to the (grand)parents only.
    pub fn set_to_buildplate_for_parents(&mut self, parents: &[ElemId], new_value: bool) {
        let mut grandparents: Vec<ElemId> = parents.to_vec();
        while !grandparents.is_empty() {
            let mut next_parents = Vec::new();
            for &gp in &grandparents {
                next_parents.extend_from_slice(&self.elems[gp].parents);
                self.elems[gp].to_buildplate = new_value;
                self.elems[gp].to_model_gracious |= new_value;
            }
            grandparents = next_parents;
        }
    }

    /// `setToBuildplateForAllParents` on an arena element.
    pub fn set_to_buildplate_for_all_parents(&mut self, id: ElemId, new_value: bool) {
        self.elems[id].to_buildplate = new_value;
        self.elems[id].to_model_gracious |= new_value;
        let parents = self.elems[id].parents.clone();
        self.set_to_buildplate_for_parents(&parents, new_value);
    }
}
