//! A port of mapbox/wagyu 0.5.0 (ISC license), the polygon clipper CuraEngine uses in
//! `Shape::removeNearSelfIntersections`. Only the 64 bit integer instantiation used by CuraEngine
//! is provided. Pointers of the C++ code are replaced by indices into arenas; the algorithm,
//! including the iteration orders of its containers, is reproduced exactly.

#![allow(clippy::too_many_arguments)]
#![allow(clippy::collapsible_else_if)]
#![allow(clippy::needless_range_loop)]

use std::collections::{HashMap, HashSet, VecDeque};

pub type T = i64;
const NIL: usize = usize::MAX;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct P {
    pub x: T,
    pub y: T,
}

impl P {
    pub fn new(x: T, y: T) -> Self {
        P { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PD {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BoxT {
    pub min: P,
    pub max: P,
}

pub type LinearRing = Vec<P>;
pub type Polygon = Vec<LinearRing>;
pub type MultiPolygon = Vec<Polygon>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipType {
    Intersection,
    Union,
    Difference,
    XOr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolygonType {
    Subject,
    Clip,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillType {
    EvenOdd,
    NonZero,
    Positive,
    Negative,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EdgeSide {
    Left,
    Right,
}

// ---------------------------------------------------------------------------------------------
// almost_equal.hpp / util.hpp
// ---------------------------------------------------------------------------------------------

const K_SIGN_BIT_MASK: u64 = 1u64 << 63;
const K_FRACTION_BIT_MASK: u64 = !0u64 >> 12;
const K_EXPONENT_BIT_MASK: u64 = !(K_SIGN_BIT_MASK | K_FRACTION_BIT_MASK);
const K_MAX_ULPS: u64 = 4;

fn fp_is_nan(bits: u64) -> bool {
    (bits & K_EXPONENT_BIT_MASK) == K_EXPONENT_BIT_MASK && (bits & K_FRACTION_BIT_MASK) != 0
}

fn sign_and_magnitude_to_biased(sam: u64) -> u64 {
    if K_SIGN_BIT_MASK & sam != 0 {
        (!sam).wrapping_add(1)
    } else {
        K_SIGN_BIT_MASK | sam
    }
}

fn distance_between_sign_and_magnitude_numbers(sam1: u64, sam2: u64) -> u64 {
    let biased1 = sign_and_magnitude_to_biased(sam1);
    let biased2 = sign_and_magnitude_to_biased(sam2);
    if biased1 >= biased2 {
        biased1 - biased2
    } else {
        biased2 - biased1
    }
}

pub fn values_are_equal(x: f64, y: f64) -> bool {
    let (bx, by) = (x.to_bits(), y.to_bits());
    if fp_is_nan(bx) || fp_is_nan(by) {
        return false;
    }
    distance_between_sign_and_magnitude_numbers(bx, by) <= K_MAX_ULPS
}

pub fn value_is_zero(val: f64) -> bool {
    values_are_equal(val, 0.0)
}

fn greater_than_or_equal(x: f64, y: f64) -> bool {
    x > y || values_are_equal(x, y)
}

fn greater_than(x: f64, y: f64) -> bool {
    !values_are_equal(x, y) && x > y
}

fn less_than(x: f64, y: f64) -> bool {
    !values_are_equal(x, y) && x < y
}

fn slopes_equal3(pt1: P, pt2: P, pt3: P) -> bool {
    (pt1.y - pt2.y).wrapping_mul(pt2.x - pt3.x) == (pt1.x - pt2.x).wrapping_mul(pt2.y - pt3.y)
}

/// `::llround`.
fn llround(v: f64) -> i64 {
    v.round() as i64
}

fn wround(value: f64) -> T {
    llround(value)
}

/// `area(linear_ring)` from util.hpp.
pub fn area(poly: &[P]) -> f64 {
    let size = poly.len();
    if size < 3 {
        return 0.0;
    }
    let mut a = 0.0;
    let mut itr_prev = poly[size - 1];
    for &itr in poly {
        a += (itr_prev.x + itr.x) as f64 * (itr_prev.y - itr.y) as f64;
        itr_prev = itr;
    }
    -a * 0.5
}

// ---------------------------------------------------------------------------------------------
// edge.hpp
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct Edge {
    bot: P,
    top: P,
    dx: f64,
}

impl Edge {
    fn new(current: P, next_pt: P) -> Self {
        let mut bot = current;
        let mut top = current;
        if current.y >= next_pt.y {
            top = next_pt;
        } else {
            bot = next_pt;
        }
        let dy = (top.y - bot.y) as f64;
        let dx = if value_is_zero(dy) { f64::INFINITY } else { (top.x - bot.x) as f64 / dy };
        Edge { bot, top, dx }
    }
}

fn slopes_equal_edges(e1: &Edge, e2: &Edge) -> bool {
    (e1.top.y - e1.bot.y).wrapping_mul(e2.top.x - e2.bot.x) == (e1.top.x - e1.bot.x).wrapping_mul(e2.top.y - e2.bot.y)
}

fn is_horizontal(e: &Edge) -> bool {
    e.dx.is_infinite()
}

fn get_current_x(edge: &Edge, current_y: T) -> f64 {
    if current_y == edge.top.y {
        edge.top.x as f64
    } else {
        edge.bot.x as f64 + edge.dx * (current_y - edge.bot.y) as f64
    }
}

// ---------------------------------------------------------------------------------------------
// bound.hpp / local_minimum.hpp
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Bound {
    edges: Vec<Edge>,
    current_edge: usize,
    next_edge: usize,
    last_point: P,
    ring: usize,
    maximum_bound: usize,
    current_x: f64,
    pos: usize,
    winding_count: i32,
    winding_count2: i32,
    winding_delta: i8,
    poly_type: PolygonType,
    side: EdgeSide,
}

impl Bound {
    fn new() -> Self {
        Bound {
            edges: Vec::new(),
            current_edge: 0,
            next_edge: 0,
            last_point: P::new(0, 0),
            ring: NIL,
            maximum_bound: NIL,
            current_x: 0.0,
            pos: 0,
            winding_count: 0,
            winding_count2: 0,
            winding_delta: 0,
            poly_type: PolygonType::Subject,
            side: EdgeSide::Left,
        }
    }
    #[inline]
    fn cur(&self) -> &Edge {
        &self.edges[self.current_edge]
    }
    #[inline]
    fn nxt(&self) -> &Edge {
        &self.edges[self.next_edge]
    }
    #[inline]
    fn has_next(&self) -> bool {
        self.next_edge != self.edges.len()
    }
}

#[derive(Clone, Debug)]
struct LocalMinimum {
    left_bound: usize,
    right_bound: usize,
    y: T,
    minimum_has_horizontal: bool,
}

// ---------------------------------------------------------------------------------------------
// point.hpp / ring.hpp
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Pt {
    ring: usize,
    x: T,
    y: T,
    next: usize,
    prev: usize,
}

#[derive(Clone, Debug)]
struct Ring {
    ring_index: usize,
    size: usize,
    area: f64,
    bbox: BoxT,
    parent: usize,
    children: Vec<usize>,
    points: usize,
    bottom_point: usize,
    is_hole: bool,
    corrected: bool,
}

impl Ring {
    fn new(ring_index: usize) -> Self {
        Ring {
            ring_index,
            size: 0,
            area: f64::NAN,
            bbox: BoxT::default(),
            parent: NIL,
            children: Vec::new(),
            points: NIL,
            bottom_point: NIL,
            is_hole: false,
            corrected: false,
        }
    }
}

/// All state of one `execute` call: the bound arena and the ring manager.
struct Ctx {
    bounds: Vec<Bound>,
    // ring_manager
    children: Vec<usize>,
    all_points: Vec<usize>,
    hot_pixels: Vec<P>,
    current_hp_itr: usize,
    points: Vec<Pt>,
    rings: Vec<Ring>,
    index: usize,
}

impl Ctx {
    #[inline]
    fn pt(&self, p: usize) -> P {
        P { x: self.points[p].x, y: self.points[p].y }
    }
    #[inline]
    fn peq(&self, a: usize, b: usize) -> bool {
        self.points[a].x == self.points[b].x && self.points[a].y == self.points[b].y
    }
    #[inline]
    fn next(&self, p: usize) -> usize {
        self.points[p].next
    }
    #[inline]
    fn prev(&self, p: usize) -> usize {
        self.points[p].prev
    }

    // ---- ring.hpp ----

    fn area_from_point(&self, mut op: usize, size: &mut usize, bbox: &mut BoxT) -> f64 {
        let start_op = op;
        *size = 0;
        let mut a = 0.0f64;
        let mut min_x = self.points[op].x;
        let mut max_x = self.points[op].x;
        let mut min_y = self.points[op].y;
        let mut max_y = self.points[op].y;
        loop {
            *size += 1;
            let (x, y) = (self.points[op].x, self.points[op].y);
            if x > max_x {
                max_x = x;
            } else if x < min_x {
                min_x = x;
            }
            if y > max_y {
                max_y = y;
            } else if y < min_y {
                min_y = y;
            }
            let pv = self.points[op].prev;
            a += (self.points[pv].x + x) as f64 * (self.points[pv].y - y) as f64;
            op = self.points[op].next;
            if op == start_op {
                break;
            }
        }
        bbox.min.x = min_x;
        bbox.max.x = max_x;
        bbox.min.y = min_y;
        bbox.max.y = max_y;
        a * 0.5
    }

    fn reset_stats(&mut self, r: usize) {
        let ring = &mut self.rings[r];
        ring.area = f64::NAN;
        ring.is_hole = false;
        ring.bbox = BoxT::default();
        ring.size = 0;
    }

    fn recalculate_stats(&mut self, r: usize) {
        if self.rings[r].points != NIL {
            let mut size = 0;
            let mut bbox = BoxT::default();
            let a = self.area_from_point(self.rings[r].points, &mut size, &mut bbox);
            let ring = &mut self.rings[r];
            ring.area = a;
            ring.size = size;
            ring.bbox = bbox;
            ring.is_hole = !(a > 0.0);
        }
    }

    fn set_stats(&mut self, r: usize, a: f64, s: usize, b: BoxT) {
        let ring = &mut self.rings[r];
        ring.bbox = b;
        ring.area = a;
        ring.size = s;
        ring.is_hole = !(a > 0.0);
    }

    fn ring_area(&mut self, r: usize) -> f64 {
        if self.rings[r].area.is_nan() {
            self.recalculate_stats(r);
        }
        self.rings[r].area
    }

    fn ring_is_hole_flag(&mut self, r: usize) -> bool {
        if self.rings[r].area.is_nan() {
            self.recalculate_stats(r);
        }
        self.rings[r].is_hole
    }

    fn ring_size(&mut self, r: usize) -> usize {
        if self.rings[r].area.is_nan() {
            self.recalculate_stats(r);
        }
        self.rings[r].size
    }

    fn create_new_ring(&mut self) -> usize {
        let id = self.rings.len();
        self.rings.push(Ring::new(self.index));
        self.index += 1;
        id
    }

    fn create_new_point(&mut self, r: usize, pt: P) -> usize {
        let id = self.points.len();
        self.points.push(Pt { ring: r, x: pt.x, y: pt.y, next: id, prev: id });
        self.all_points.push(id);
        id
    }

    fn create_new_point_before(&mut self, r: usize, pt: P, before_this_point: usize) -> usize {
        let id = self.points.len();
        let prev = self.points[before_this_point].prev;
        self.points.push(Pt { ring: r, x: pt.x, y: pt.y, next: before_this_point, prev });
        self.points[before_this_point].prev = id;
        self.points[prev].next = id;
        self.all_points.push(id);
        id
    }

    fn children_of(&mut self, parent: usize) -> &mut Vec<usize> {
        if parent == NIL {
            &mut self.children
        } else {
            &mut self.rings[parent].children
        }
    }

    fn set_to_children(&mut self, r: usize, parent: usize) {
        let children = self.children_of(parent);
        for c in children.iter_mut() {
            if *c == NIL {
                *c = r;
                return;
            }
        }
        children.push(r);
    }

    fn remove_from_children(&mut self, r: usize, parent: usize) {
        let children = self.children_of(parent);
        for c in children.iter_mut() {
            if *c == r {
                *c = NIL;
                return;
            }
        }
    }

    fn assign_as_child(&mut self, new_ring: usize, parent: usize) {
        let new_is_hole = self.ring_is_hole_flag(new_ring);
        if (parent == NIL && new_is_hole) || (parent != NIL && new_is_hole == self.ring_is_hole_flag(parent)) {
            panic!("Trying to assign a child that is the same orientation as the parent");
        }
        self.set_to_children(new_ring, parent);
        self.rings[new_ring].parent = parent;
    }

    fn reassign_as_child(&mut self, ring: usize, parent: usize) {
        let ring_is_hole = self.ring_is_hole_flag(ring);
        if (parent == NIL && ring_is_hole) || (parent != NIL && ring_is_hole == self.ring_is_hole_flag(parent)) {
            panic!("Trying to re-assign a child that is the same orientation as the parent");
        }
        let old_parent = self.rings[ring].parent;
        self.remove_from_children(ring, old_parent);
        self.set_to_children(ring, parent);
        self.rings[ring].parent = parent;
    }

    fn assign_as_sibling(&mut self, new_ring: usize, sibling: usize) {
        if self.ring_is_hole_flag(new_ring) != self.ring_is_hole_flag(sibling) {
            panic!("Trying to assign to be a sibling that is not the same orientation as the sibling");
        }
        let sp = self.rings[sibling].parent;
        self.set_to_children(new_ring, sp);
        self.rings[new_ring].parent = sp;
    }

    fn ring1_replaces_ring2(&mut self, ring1: usize, ring2: usize) {
        debug_assert!(ring1 != ring2);
        let n = self.rings[ring2].children.len();
        for i in 0..n {
            let c = self.rings[ring2].children[i];
            if c == NIL {
                continue;
            }
            self.rings[c].parent = ring1;
            self.set_to_children(c, ring1);
            self.rings[ring2].children[i] = NIL;
        }
        let old_parent = self.rings[ring2].parent;
        self.remove_from_children(ring2, old_parent);
        self.rings[ring2].points = NIL;
        self.reset_stats(ring2);
    }

    fn remove_ring_and_points(&mut self, r: usize, remove_children: bool, remove_from_parent: bool) {
        let n = self.rings[r].children.len();
        for i in 0..n {
            let c = self.rings[r].children[i];
            if c == NIL {
                continue;
            }
            if remove_children {
                self.remove_ring_and_points(c, true, false);
            }
            self.rings[r].children[i] = NIL;
        }
        if remove_from_parent {
            let parent = self.rings[r].parent;
            self.remove_from_children(r, parent);
        }
        let mut pt = self.rings[r].points;
        if pt != NIL {
            let pv = self.points[pt].prev;
            self.points[pv].next = NIL;
            while pt != NIL {
                let tmp = pt;
                pt = self.points[pt].next;
                self.points[tmp].next = NIL;
                self.points[tmp].prev = NIL;
                self.points[tmp].ring = NIL;
            }
        }
        self.rings[r].points = NIL;
        self.reset_stats(r);
    }

    fn remove_ring(&mut self, r: usize, remove_children: bool, remove_from_parent: bool) {
        let n = self.rings[r].children.len();
        for i in 0..n {
            let c = self.rings[r].children[i];
            if c == NIL {
                continue;
            }
            if remove_children {
                self.remove_ring(c, true, false);
            }
            self.rings[r].children[i] = NIL;
        }
        if remove_from_parent {
            let parent = self.rings[r].parent;
            self.remove_from_children(r, parent);
        }
        self.rings[r].points = NIL;
        self.reset_stats(r);
    }

    fn ring_depth(&self, mut r: usize) -> usize {
        let mut depth = 0;
        if r == NIL {
            return depth;
        }
        while self.rings[r].parent != NIL {
            depth += 1;
            r = self.rings[r].parent;
        }
        depth
    }

    fn ring_is_hole_depth(&self, r: usize) -> bool {
        self.ring_depth(r) & 1 == 1
    }

    fn reverse_ring(&mut self, pp: usize) {
        if pp == NIL {
            return;
        }
        let mut pp1 = pp;
        loop {
            let pp2 = self.points[pp1].next;
            self.points[pp1].next = self.points[pp1].prev;
            self.points[pp1].prev = pp2;
            pp1 = pp2;
            if pp1 == pp {
                break;
            }
        }
    }

    // ---- active_bound_list.hpp ----

    fn is_maxima(&self, b: usize, y: T) -> bool {
        let bnd = &self.bounds[b];
        !bnd.has_next() && bnd.cur().top.y == y
    }

    fn is_intermediate(&self, b: usize, y: T) -> bool {
        let bnd = &self.bounds[b];
        bnd.has_next() && bnd.cur().top.y == y
    }

    fn current_edge_is_horizontal(&self, b: usize) -> bool {
        is_horizontal(self.bounds[b].cur())
    }

    fn next_edge_is_horizontal(&self, b: usize) -> bool {
        is_horizontal(self.bounds[b].nxt())
    }

    fn next_edge_in_bound(&mut self, b: usize, scanbeam: &mut Vec<T>) {
        let bnd = &mut self.bounds[b];
        bnd.current_edge += 1;
        if bnd.current_edge != bnd.edges.len() {
            bnd.next_edge += 1;
            bnd.current_x = bnd.edges[bnd.current_edge].bot.x as f64;
            if !is_horizontal(&bnd.edges[bnd.current_edge]) {
                let y = bnd.edges[bnd.current_edge].top.y;
                insert_sorted_scanbeam(scanbeam, y);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// scanbeam.hpp
// ---------------------------------------------------------------------------------------------

fn insert_sorted_scanbeam(scanbeam: &mut Vec<T>, t: T) {
    let i = scanbeam.partition_point(|&v| v < t);
    if i == scanbeam.len() || t < scanbeam[i] {
        scanbeam.insert(i, t);
    }
}

fn pop_from_scanbeam(y: &mut T, scanbeam: &mut Vec<T>) -> bool {
    match scanbeam.pop() {
        Some(v) => {
            *y = v;
            true
        }
        None => false,
    }
}

fn setup_scanbeam(minima_list: &[LocalMinimum], scanbeam: &mut Vec<T>) {
    scanbeam.reserve(minima_list.len());
    for lm in minima_list {
        scanbeam.push(lm.y);
    }
    cppstd::sort(scanbeam);
}

// ---------------------------------------------------------------------------------------------
// build_edges.hpp
// ---------------------------------------------------------------------------------------------

fn point_2_is_between_point_1_and_point_3(pt1: P, pt2: P, pt3: P) -> bool {
    if pt1 == pt3 || pt1 == pt2 || pt3 == pt2 {
        false
    } else if pt1.x != pt3.x {
        (pt2.x > pt1.x) == (pt2.x < pt3.x)
    } else {
        (pt2.y > pt1.y) == (pt2.y < pt3.y)
    }
}

fn build_edge_list(path_geometry: &[P], edges: &mut Vec<Edge>) -> bool {
    let n = path_geometry.len();
    if n < 3 {
        return false;
    }

    // As this is a loop, we need to first go backwards from end to try and find
    // the proper starting point for the iterators before the beginning
    // Reverse iterators are represented by the index of their element.
    let mut itr_rev: usize = n - 1;
    let mut itr: usize = 0;
    let mut pt1 = path_geometry[itr_rev];
    let mut pt2 = path_geometry[itr];

    // Find next non repeated point going backwards from
    // end for pt1
    while pt1 == pt2 {
        if itr_rev == 0 {
            return false;
        }
        itr_rev -= 1;
        pt1 = path_geometry[itr_rev];
    }
    itr += 1;
    let mut pt3 = path_geometry[itr];
    // itr_rev.base() points one past the reverse element.
    let mut itr_last = itr_rev + 1;
    let mut front_pt = P::default();
    let mut back_pt = P::default();
    loop {
        if pt3 == pt2 {
            // Duplicate point advance itr, but do not
            // advance other points
            if itr == itr_last {
                break;
            }
            itr += 1;
            if itr == itr_last {
                if edges.is_empty() {
                    break;
                }
                pt3 = front_pt;
            } else {
                pt3 = path_geometry[itr];
            }
            continue;
        }

        // Now check if slopes are equal between two segments - either
        // a spike or a collinear point - if so drop point number 2.
        if slopes_equal3(pt1, pt2, pt3) {
            // We need to reconsider previously added points
            // because the point it was using was found to be collinear
            // or a spike
            pt2 = pt1;
            if !edges.is_empty() {
                edges.pop(); // remove previous edge (pt1)
            }
            if !edges.is_empty() {
                let back_top = edges.last().unwrap().top;
                if back_pt.x == back_top.x && back_pt.y == back_top.y {
                    let back_bot = edges.last().unwrap().bot;
                    pt1 = back_bot;
                } else {
                    pt1 = back_top;
                }
                back_pt = pt1;
            } else {
                // If this occurs we must look to the back of the
                // ring for new points.
                while path_geometry[itr_rev] == pt2 {
                    // ++itr_rev on a reverse iterator moves towards the front.
                    if itr_rev == 0 {
                        // `(itr + 1) == itr_rev.base()` can never be reached before rend in valid
                        // input; guard against walking off the front.
                        return false;
                    }
                    itr_rev -= 1;
                    if (itr + 1) == itr_rev + 1 {
                        return false;
                    }
                }
                pt1 = path_geometry[itr_rev];
                itr_last = itr_rev + 1;
            }
            continue;
        }

        if edges.is_empty() {
            front_pt = pt2;
        }
        edges.push(Edge::new(pt2, pt3));
        back_pt = pt2;
        if itr == itr_last {
            break;
        }
        pt1 = pt2;
        pt2 = pt3;
        itr += 1;
        if itr == itr_last {
            if edges.is_empty() {
                break;
            }
            pt3 = front_pt;
        } else {
            pt3 = path_geometry[itr];
        }
    }

    let mut modified;
    loop {
        modified = false;
        if edges.len() < 3 {
            return false;
        }
        let last = edges.len() - 1;
        let f = edges[0];
        let b = edges[last];
        if slopes_equal_edges(&f, &b) {
            if f.bot == b.top {
                if f.top == b.bot {
                    edges.pop();
                    edges.remove(0);
                } else {
                    edges[0].bot = b.bot;
                    edges.pop();
                }
                modified = true;
            } else if f.top == b.bot {
                edges[0].top = b.top;
                edges.pop();
                modified = true;
            } else if f.top == b.top && f.bot == b.bot {
                edges.pop();
                edges.remove(0);
                modified = true;
            } else if f.top == b.top {
                if point_2_is_between_point_1_and_point_3(f.top, f.bot, b.bot) {
                    edges[last].top = f.bot;
                    edges.remove(0);
                } else {
                    edges[0].top = b.bot;
                    edges.pop();
                }
                modified = true;
            } else if f.bot == b.bot {
                if point_2_is_between_point_1_and_point_3(f.bot, f.top, b.top) {
                    edges[last].bot = f.top;
                    edges.remove(0);
                } else {
                    edges[0].bot = b.top;
                    edges.pop();
                }
                modified = true;
            }
        }
        if !modified {
            break;
        }
    }

    true
}

// ---------------------------------------------------------------------------------------------
// local_minimum_util.hpp
// ---------------------------------------------------------------------------------------------

fn reverse_horizontal(e: &mut Edge) {
    std::mem::swap(&mut e.top.x, &mut e.bot.x);
}

fn start_list_on_local_maximum(edges: &mut Vec<Edge>) {
    if edges.len() <= 2 {
        return;
    }
    // Find the first local maximum going forward in the list
    let mut prev_edge = edges.len() - 1;
    let mut prev_edge_is_horizontal = is_horizontal(&edges[prev_edge]);
    let mut edge = 0;
    let mut y_decreasing_before_last_horizontal = false; // assume false at start

    while edge != edges.len() {
        let edge_is_horizontal = is_horizontal(&edges[edge]);
        if !prev_edge_is_horizontal && !edge_is_horizontal && edges[edge].top == edges[prev_edge].top {
            break;
        }
        if !edge_is_horizontal && prev_edge_is_horizontal {
            if y_decreasing_before_last_horizontal && (edges[edge].top == edges[prev_edge].bot || edges[edge].top == edges[prev_edge].top) {
                break;
            }
        } else if !y_decreasing_before_last_horizontal
            && !prev_edge_is_horizontal
            && edge_is_horizontal
            && (edges[prev_edge].top == edges[edge].top || edges[prev_edge].top == edges[edge].bot)
        {
            y_decreasing_before_last_horizontal = true;
        }
        prev_edge_is_horizontal = edge_is_horizontal;
        prev_edge = edge;
        edge += 1;
    }
    edges.rotate_left(edge);
}

fn create_bound_towards_minimum(edges: &mut Vec<Edge>) -> Bound {
    if edges.len() == 1 {
        if is_horizontal(&edges[0]) {
            reverse_horizontal(&mut edges[0]);
        }
        let mut bnd = Bound::new();
        std::mem::swap(&mut bnd.edges, edges);
        return bnd;
    }
    let mut next_edge = 0usize;
    let mut edge = next_edge;
    next_edge += 1;
    let mut edge_is_horizontal = is_horizontal(&edges[edge]);
    if edge_is_horizontal {
        reverse_horizontal(&mut edges[edge]);
    }
    let mut y_increasing_before_last_horizontal = false; // assume false at start

    while next_edge != edges.len() {
        let next_edge_is_horizontal = is_horizontal(&edges[next_edge]);
        if !next_edge_is_horizontal && !edge_is_horizontal && edges[edge].bot == edges[next_edge].bot {
            break;
        }
        if !next_edge_is_horizontal && edge_is_horizontal {
            if y_increasing_before_last_horizontal && (edges[next_edge].bot == edges[edge].bot || edges[next_edge].bot == edges[edge].top) {
                break;
            }
        } else if !y_increasing_before_last_horizontal
            && !edge_is_horizontal
            && next_edge_is_horizontal
            && (edges[edge].bot == edges[next_edge].top || edges[edge].bot == edges[next_edge].bot)
        {
            y_increasing_before_last_horizontal = true;
        }
        edge_is_horizontal = next_edge_is_horizontal;
        edge = next_edge;
        if edge_is_horizontal {
            reverse_horizontal(&mut edges[edge]);
        }
        next_edge += 1;
    }
    let mut bnd = Bound::new();
    if next_edge == edges.len() {
        std::mem::swap(edges, &mut bnd.edges);
    } else {
        bnd.edges = edges.drain(0..next_edge).collect();
    }
    bnd.edges.reverse();
    bnd
}

fn create_bound_towards_maximum(edges: &mut Vec<Edge>) -> Bound {
    if edges.len() == 1 {
        let mut bnd = Bound::new();
        std::mem::swap(&mut bnd.edges, edges);
        return bnd;
    }
    let mut next_edge = 0usize;
    let mut edge = next_edge;
    next_edge += 1;
    let mut edge_is_horizontal = is_horizontal(&edges[edge]);
    let mut y_decreasing_before_last_horizontal = false; // assume false at start

    while next_edge != edges.len() {
        let next_edge_is_horizontal = is_horizontal(&edges[next_edge]);
        if !next_edge_is_horizontal && !edge_is_horizontal && edges[edge].top == edges[next_edge].top {
            break;
        }
        if !next_edge_is_horizontal && edge_is_horizontal {
            if y_decreasing_before_last_horizontal && (edges[next_edge].top == edges[edge].bot || edges[next_edge].top == edges[edge].top) {
                break;
            }
        } else if !y_decreasing_before_last_horizontal
            && !edge_is_horizontal
            && next_edge_is_horizontal
            && (edges[edge].top == edges[next_edge].top || edges[edge].top == edges[next_edge].bot)
        {
            y_decreasing_before_last_horizontal = true;
        }
        edge_is_horizontal = next_edge_is_horizontal;
        edge = next_edge;
        next_edge += 1;
    }
    let mut bnd = Bound::new();
    if next_edge == edges.len() {
        std::mem::swap(&mut bnd.edges, edges);
    } else {
        bnd.edges = edges.drain(0..next_edge).collect();
    }
    bnd
}

fn fix_horizontals(bnd: &mut Bound) {
    let mut edge_itr = 0usize;
    let next_itr = edge_itr + 1;
    if next_itr == bnd.edges.len() {
        return;
    }
    if is_horizontal(&bnd.edges[edge_itr]) && bnd.edges[next_itr].bot != bnd.edges[edge_itr].top {
        reverse_horizontal(&mut bnd.edges[edge_itr]);
    }
    let mut prev_itr = edge_itr;
    edge_itr += 1;
    while edge_itr != bnd.edges.len() {
        if is_horizontal(&bnd.edges[edge_itr]) && bnd.edges[prev_itr].top != bnd.edges[edge_itr].bot {
            reverse_horizontal(&mut bnd.edges[edge_itr]);
        }
        prev_itr = edge_itr;
        edge_itr += 1;
    }
}

fn move_horizontals_on_left_to_right(left_bound: &mut Bound, right_bound: &mut Bound) {
    // We want all the horizontal segments that are at the same Y as the minimum to be on the right
    // bound
    let mut edge_itr = 0usize;
    while edge_itr != left_bound.edges.len() {
        if !is_horizontal(&left_bound.edges[edge_itr]) {
            break;
        }
        reverse_horizontal(&mut left_bound.edges[edge_itr]);
        edge_itr += 1;
    }
    if edge_itr == 0 {
        return;
    }
    left_bound.edges[0..edge_itr].reverse();
    let dist = edge_itr;
    let moved: Vec<Edge> = left_bound.edges.drain(0..edge_itr).collect();
    right_bound.edges.extend(moved);
    let len = right_bound.edges.len();
    right_bound.edges.rotate_left(len - dist);
}

// ---------------------------------------------------------------------------------------------
// The wagyu clipper.
// ---------------------------------------------------------------------------------------------

/// `mapbox::geometry::wagyu::wagyu<T>`.
pub struct Wagyu {
    bounds: Vec<Bound>,
    minima_list: Vec<LocalMinimum>,
    reverse_output: bool,
}

impl Default for Wagyu {
    fn default() -> Self {
        Self::new()
    }
}

impl Wagyu {
    pub fn new() -> Self {
        Wagyu { bounds: Vec::new(), minima_list: Vec::new(), reverse_output: false }
    }

    pub fn add_ring(&mut self, pg: &[P], p_type: PolygonType) -> bool {
        let mut new_edges: Vec<Edge> = Vec::with_capacity(pg.len());
        if !build_edge_list(pg, &mut new_edges) || new_edges.is_empty() {
            return false;
        }
        self.add_ring_to_local_minima_list(new_edges, p_type);
        true
    }

    pub fn add_polygon(&mut self, ppg: &[LinearRing], p_type: PolygonType) -> bool {
        let mut result = false;
        for r in ppg {
            if self.add_ring(r, p_type) {
                result = true;
            }
        }
        result
    }

    pub fn reverse_rings(&mut self, value: bool) {
        self.reverse_output = value;
    }

    pub fn clear(&mut self) {
        self.minima_list.clear();
        self.bounds.clear();
    }

    fn add_ring_to_local_minima_list(&mut self, mut edges: Vec<Edge>, poly_type: PolygonType) {
        if edges.is_empty() {
            return;
        }
        // Adjust the order of the ring so we start on a local maximum
        // therefore we start right away on a bound.
        start_list_on_local_maximum(&mut edges);

        let mut first_minimum: usize = NIL;
        let mut last_maximum: usize = NIL;
        while !edges.is_empty() {
            let mut lm_minimum_has_horizontal = false;
            let mut to_minimum = create_bound_towards_minimum(&mut edges);
            if edges.is_empty() {
                panic!("Edges is empty after only creating a single bound.");
            }
            let mut to_maximum = create_bound_towards_maximum(&mut edges);
            fix_horizontals(&mut to_minimum);
            fix_horizontals(&mut to_maximum);
            let mut to_max_first_non_horizontal = 0usize;
            let mut to_min_first_non_horizontal = 0usize;
            let minimum_is_left;
            while to_max_first_non_horizontal != to_maximum.edges.len() && is_horizontal(&to_maximum.edges[to_max_first_non_horizontal]) {
                lm_minimum_has_horizontal = true;
                to_max_first_non_horizontal += 1;
            }
            while to_min_first_non_horizontal != to_minimum.edges.len() && is_horizontal(&to_minimum.edges[to_min_first_non_horizontal]) {
                lm_minimum_has_horizontal = true;
                to_min_first_non_horizontal += 1;
            }

            if to_max_first_non_horizontal == to_maximum.edges.len() || to_min_first_non_horizontal == to_minimum.edges.len() {
                panic!("should not have a horizontal only bound for a ring");
            }

            if lm_minimum_has_horizontal {
                if to_maximum.edges[to_max_first_non_horizontal].bot.x > to_minimum.edges[to_min_first_non_horizontal].bot.x {
                    minimum_is_left = true;
                    move_horizontals_on_left_to_right(&mut to_minimum, &mut to_maximum);
                } else {
                    minimum_is_left = false;
                    move_horizontals_on_left_to_right(&mut to_maximum, &mut to_minimum);
                }
            } else {
                minimum_is_left = !(to_maximum.edges[to_max_first_non_horizontal].dx > to_minimum.edges[to_min_first_non_horizontal].dx);
            }
            let min_front_y = to_minimum.edges[0].bot.y;
            if last_maximum != NIL {
                to_minimum.maximum_bound = last_maximum;
            }
            to_minimum.poly_type = poly_type;
            to_maximum.poly_type = poly_type;
            if !minimum_is_left {
                to_minimum.side = EdgeSide::Right;
                to_maximum.side = EdgeSide::Left;
                to_minimum.winding_delta = -1;
                to_maximum.winding_delta = 1;
                let left = self.bounds.len();
                self.bounds.push(to_maximum);
                let right = self.bounds.len();
                self.bounds.push(to_minimum);
                self.minima_list.push(LocalMinimum { left_bound: left, right_bound: right, y: min_front_y, minimum_has_horizontal: lm_minimum_has_horizontal });
                if last_maximum == NIL {
                    first_minimum = right;
                } else {
                    self.bounds[last_maximum].maximum_bound = right;
                }
                last_maximum = left;
            } else {
                to_minimum.side = EdgeSide::Left;
                to_maximum.side = EdgeSide::Right;
                to_minimum.winding_delta = -1;
                to_maximum.winding_delta = 1;
                let left = self.bounds.len();
                self.bounds.push(to_minimum);
                let right = self.bounds.len();
                self.bounds.push(to_maximum);
                self.minima_list.push(LocalMinimum { left_bound: left, right_bound: right, y: min_front_y, minimum_has_horizontal: lm_minimum_has_horizontal });
                if last_maximum == NIL {
                    first_minimum = left;
                } else {
                    self.bounds[last_maximum].maximum_bound = left;
                }
                last_maximum = right;
            }
        }
        self.bounds[last_maximum].maximum_bound = first_minimum;
        self.bounds[first_minimum].maximum_bound = last_maximum;
    }

    pub fn get_bounds(&self) -> BoxT {
        let mut min = P::new(0, 0);
        let mut max = P::new(0, 0);
        if self.minima_list.is_empty() {
            return BoxT { min, max };
        }
        let mut first_set = false;
        for lm in &self.minima_list {
            for &b in &[lm.left_bound, lm.right_bound] {
                let edges = &self.bounds[b].edges;
                if !edges.is_empty() {
                    if !first_set {
                        min = edges[0].top;
                        max = edges[edges.len() - 1].bot;
                        first_set = true;
                    } else {
                        min.y = min.y.min(edges[0].top.y);
                        max.y = max.y.max(edges[edges.len() - 1].bot.y);
                        max.x = max.x.max(edges[edges.len() - 1].top.x);
                        min.x = min.x.min(edges[edges.len() - 1].top.x);
                    }
                    for e in edges {
                        max.x = max.x.max(e.bot.x);
                        min.x = min.x.min(e.bot.x);
                    }
                }
            }
        }
        BoxT { min, max }
    }

    /// `execute(cliptype, solution, subject_fill_type, clip_fill_type)`.
    pub fn execute(&mut self, cliptype: ClipType, solution: &mut MultiPolygon, subject_fill_type: FillType, clip_fill_type: FillType) -> bool {
        if self.minima_list.is_empty() {
            return false;
        }

        let mut ctx = Ctx {
            bounds: std::mem::take(&mut self.bounds),
            children: Vec::new(),
            all_points: Vec::new(),
            hot_pixels: Vec::new(),
            current_hp_itr: 0,
            points: Vec::new(),
            rings: Vec::new(),
            index: 0,
        };

        ctx.build_hot_pixels(&self.minima_list);

        ctx.execute_vatti(&self.minima_list, cliptype, subject_fill_type, clip_fill_type);

        ctx.correct_topology();

        ctx.build_result(solution, self.reverse_output);

        self.bounds = ctx.bounds;
        true
    }
}

/// `local_minimum_sorter`.
fn local_minimum_less(lm1: &LocalMinimum, lm2: &LocalMinimum) -> bool {
    if lm2.y == lm1.y {
        return lm2.minimum_has_horizontal != lm1.minimum_has_horizontal && lm1.minimum_has_horizontal;
    }
    lm2.y < lm1.y
}

fn sorted_minima(minima_list: &[LocalMinimum]) -> Vec<usize> {
    let mut minima_sorted: Vec<usize> = (0..minima_list.len()).collect();
    cppstd::stable_sort_by(&mut minima_sorted, |&a, &b| local_minimum_less(&minima_list[a], &minima_list[b]));
    minima_sorted
}

/// `round_towards_min`.
fn round_towards_min(val: f64) -> T {
    // 0.5 rounds to 0
    // 0.0 rounds to 0
    // -0.5 rounds to -1
    let half = val.floor() + 0.5;
    if values_are_equal(val, half) {
        val.floor() as T
    } else {
        llround(val)
    }
}

/// `round_towards_max`.
fn round_towards_max(val: f64) -> T {
    // 0.5 rounds to 1
    // 0.0 rounds to 0
    // -0.5 rounds to 0
    let half = val.floor() + 0.5;
    if values_are_equal(val, half) {
        val.ceil() as T
    } else {
        llround(val)
    }
}

fn round_point(pt: PD) -> P {
    P::new(round_towards_max(pt.x), round_towards_max(pt.y))
}

fn get_edge_min_x(edge: &Edge, current_y: T) -> T {
    if is_horizontal(edge) {
        if edge.bot.x < edge.top.x {
            edge.bot.x
        } else {
            edge.top.x
        }
    } else if edge.dx > 0.0 {
        if current_y == edge.top.y {
            edge.top.x
        } else {
            let lower_range_y = (current_y - edge.bot.y) as f64 - 0.5;
            let return_val = edge.bot.x as f64 + edge.dx * lower_range_y;
            round_towards_min(return_val)
        }
    } else {
        if current_y == edge.bot.y {
            edge.bot.x
        } else {
            let return_val = edge.bot.x as f64 + edge.dx * ((current_y - edge.bot.y) as f64 + 0.5);
            round_towards_min(return_val)
        }
    }
}

fn get_edge_max_x(edge: &Edge, current_y: T) -> T {
    if is_horizontal(edge) {
        if edge.bot.x > edge.top.x {
            edge.bot.x
        } else {
            edge.top.x
        }
    } else if edge.dx < 0.0 {
        if current_y == edge.top.y {
            edge.top.x
        } else {
            let lower_range_y = (current_y - edge.bot.y) as f64 - 0.5;
            let return_val = edge.bot.x as f64 + edge.dx * lower_range_y;
            round_towards_max(return_val)
        }
    } else {
        if current_y == edge.bot.y {
            edge.bot.x
        } else {
            let return_val = edge.bot.x as f64 + edge.dx * ((current_y - edge.bot.y) as f64 + 0.5);
            round_towards_max(return_val)
        }
    }
}

/// `get_edge_intersection<T, double>`.
fn get_edge_intersection(e1: &Edge, e2: &Edge) -> Option<PD> {
    let p0_x = e1.bot.x as f64;
    let p0_y = e1.bot.y as f64;
    let p1_x = e1.top.x as f64;
    let p1_y = e1.top.y as f64;
    let p2_x = e2.bot.x as f64;
    let p2_y = e2.bot.y as f64;
    let p3_x = e2.top.x as f64;
    let p3_y = e2.top.y as f64;
    let s1_x = p1_x - p0_x;
    let s1_y = p1_y - p0_y;
    let s2_x = p3_x - p2_x;
    let s2_y = p3_y - p2_y;

    let s = (-s1_y * (p0_x - p2_x) + s1_x * (p0_y - p2_y)) / (-s2_x * s1_y + s1_x * s2_y);
    let t = (s2_x * (p0_y - p2_y) - s2_y * (p0_x - p2_x)) / (-s2_x * s1_y + s1_x * s2_y);

    if (0.0..=1.0).contains(&s) && (0.0..=1.0).contains(&t) {
        Some(PD { x: p0_x + (t * s1_x), y: p0_y + (t * s1_y) })
    } else {
        None
    }
}

#[derive(Clone, Copy, Debug)]
struct IntersectNode {
    bound1: usize,
    bound2: usize,
    pt: PD,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PointInPolygonResult {
    OnPolygon,
    InsidePolygon,
    OutsidePolygon,
}

#[derive(Clone, Copy, Debug)]
struct PointPtrPair {
    op1: usize,
    op2: usize,
}

/// Model of `std::unordered_multimap<ring_ptr, point_ptr_pair>` as used by wagyu: only
/// `equal_range` iteration is ever used, and libstdc++ inserts a new element in front of
/// existing elements with an equivalent key.
#[derive(Default)]
struct ConnectionMap {
    map: HashMap<usize, Vec<PointPtrPair>>,
}

impl ConnectionMap {
    fn emplace(&mut self, key: usize, value: PointPtrPair) {
        self.map.entry(key).or_default().insert(0, value);
    }
    fn len_of(&self, key: usize) -> usize {
        self.map.get(&key).map(|v| v.len()).unwrap_or(0)
    }
    fn get(&self, key: usize, i: usize) -> PointPtrPair {
        self.map[&key][i]
    }
    fn erase_at(&mut self, key: usize, i: usize) {
        if let Some(v) = self.map.get_mut(&key) {
            v.remove(i);
        }
    }
    fn erase_key(&mut self, key: usize) {
        self.map.remove(&key);
    }
}

impl Ctx {
    // ---- active_bound_list.hpp ----

    fn is_even_odd_fill_type(&self, b: usize, subject_fill_type: FillType, clip_fill_type: FillType) -> bool {
        if self.bounds[b].poly_type == PolygonType::Subject {
            subject_fill_type == FillType::EvenOdd
        } else {
            clip_fill_type == FillType::EvenOdd
        }
    }

    fn is_even_odd_alt_fill_type(&self, b: usize, subject_fill_type: FillType, clip_fill_type: FillType) -> bool {
        if self.bounds[b].poly_type == PolygonType::Subject {
            clip_fill_type == FillType::EvenOdd
        } else {
            subject_fill_type == FillType::EvenOdd
        }
    }

    /// `bound_insert_location(bound2)(b)`.
    fn bound_insert_location(&self, bound2: usize, b: usize) -> bool {
        let bound1 = &self.bounds[b];
        let bound2 = &self.bounds[bound2];
        if values_are_equal(bound2.current_x, bound1.current_x) {
            if bound2.cur().top.y > bound1.cur().top.y {
                less_than(bound2.cur().top.x as f64, get_current_x(bound1.cur(), bound2.cur().top.y))
            } else {
                greater_than(bound1.cur().top.x as f64, get_current_x(bound2.cur(), bound1.cur().top.y))
            }
        } else {
            bound2.current_x < bound1.current_x
        }
    }

    fn insert_bound_into_abl(&self, left: usize, right: usize, active_bounds: &mut Vec<usize>) -> usize {
        let mut itr = active_bounds.len();
        for (i, &b) in active_bounds.iter().enumerate() {
            if self.bound_insert_location(left, b) {
                itr = i;
                break;
            }
        }
        active_bounds.insert(itr, right);
        active_bounds.insert(itr, left);
        itr
    }

    fn get_maxima_pair(&self, bnd_idx: usize, active_bounds: &[usize]) -> usize {
        let maximum = self.bounds[active_bounds[bnd_idx]].maximum_bound;
        active_bounds.iter().position(|&b| b == maximum).unwrap_or(active_bounds.len())
    }

    fn set_winding_count(&mut self, bnd_itr: usize, active_bounds: &[usize], subject_fill_type: FillType, clip_fill_type: FillType) {
        let b = active_bounds[bnd_itr];
        // reverse iterator from bnd_itr: element at index rev - 1
        let mut rev: usize = bnd_itr;
        if rev == 0 {
            self.bounds[b].winding_count = self.bounds[b].winding_delta as i32;
            self.bounds[b].winding_count2 = 0;
            return;
        }

        // find the edge of the same polytype that immediately preceeds 'edge' in
        // AEL
        while rev != 0 && self.bounds[active_bounds[rev - 1]].poly_type != self.bounds[b].poly_type {
            rev -= 1;
        }
        if rev == 0 {
            self.bounds[b].winding_count = self.bounds[b].winding_delta as i32;
            self.bounds[b].winding_count2 = 0;
        } else if self.is_even_odd_fill_type(b, subject_fill_type, clip_fill_type) {
            // EvenOdd filling ...
            let rb = active_bounds[rev - 1];
            self.bounds[b].winding_count = self.bounds[b].winding_delta as i32;
            self.bounds[b].winding_count2 = self.bounds[rb].winding_count2;
        } else {
            // nonZero, Positive or Negative filling ...
            let rb = active_bounds[rev - 1];
            let rb_wc = self.bounds[rb].winding_count;
            let rb_wd = self.bounds[rb].winding_delta as i32;
            let b_wd = self.bounds[b].winding_delta as i32;
            if rb_wc * rb_wd < 0 {
                // prev edge is 'decreasing' WindCount (WC) toward zero
                // so we're outside the previous polygon ...
                if rb_wc.abs() > 1 {
                    // outside prev poly but still inside another.
                    // when reversing direction of prev poly use the same WC
                    if rb_wd * b_wd < 0 {
                        self.bounds[b].winding_count = rb_wc;
                    } else {
                        // otherwise continue to 'decrease' WC ...
                        self.bounds[b].winding_count = rb_wc + b_wd;
                    }
                } else {
                    // now outside all polys of same polytype so set own WC ...
                    self.bounds[b].winding_count = b_wd;
                }
            } else {
                // prev edge is 'increasing' WindCount (WC) away from zero
                // so we're inside the previous polygon ...
                if rb_wd * b_wd < 0 {
                    // if wind direction is reversing then use same WC
                    self.bounds[b].winding_count = rb_wc;
                } else {
                    // otherwise add to WC ...
                    self.bounds[b].winding_count = rb_wc + b_wd;
                }
            }
            self.bounds[b].winding_count2 = self.bounds[rb].winding_count2;
        }

        // update winding_count2 ...
        let mut bnd_itr_forward = rev; // rev_bnd_itr.base()
        if self.is_even_odd_alt_fill_type(b, subject_fill_type, clip_fill_type) {
            // EvenOdd filling ...
            while bnd_itr_forward != bnd_itr {
                self.bounds[b].winding_count2 = if self.bounds[b].winding_count2 == 0 { 1 } else { 0 };
                bnd_itr_forward += 1;
            }
        } else {
            // nonZero, Positive or Negative filling ...
            while bnd_itr_forward != bnd_itr {
                let d = self.bounds[active_bounds[bnd_itr_forward]].winding_delta as i32;
                self.bounds[b].winding_count2 += d;
                bnd_itr_forward += 1;
            }
        }
    }

    fn is_contributing(&self, b: usize, cliptype: ClipType, subject_fill_type: FillType, clip_fill_type: FillType) -> bool {
        let bnd = &self.bounds[b];
        let mut pft = subject_fill_type;
        let mut pft2 = clip_fill_type;
        if bnd.poly_type != PolygonType::Subject {
            pft = clip_fill_type;
            pft2 = subject_fill_type;
        }

        match pft {
            FillType::EvenOdd => {}
            FillType::NonZero => {
                if bnd.winding_count.abs() != 1 {
                    return false;
                }
            }
            FillType::Positive => {
                if bnd.winding_count != 1 {
                    return false;
                }
            }
            FillType::Negative => {
                if bnd.winding_count != -1 {
                    return false;
                }
            }
        }

        match cliptype {
            ClipType::Intersection => match pft2 {
                FillType::EvenOdd | FillType::NonZero => bnd.winding_count2 != 0,
                FillType::Positive => bnd.winding_count2 > 0,
                FillType::Negative => bnd.winding_count2 < 0,
            },
            ClipType::Union => match pft2 {
                FillType::EvenOdd | FillType::NonZero => bnd.winding_count2 == 0,
                FillType::Positive => bnd.winding_count2 <= 0,
                FillType::Negative => bnd.winding_count2 >= 0,
            },
            ClipType::Difference => {
                if bnd.poly_type == PolygonType::Subject {
                    match pft2 {
                        FillType::EvenOdd | FillType::NonZero => bnd.winding_count2 == 0,
                        FillType::Positive => bnd.winding_count2 <= 0,
                        FillType::Negative => bnd.winding_count2 >= 0,
                    }
                } else {
                    match pft2 {
                        FillType::EvenOdd | FillType::NonZero => bnd.winding_count2 != 0,
                        FillType::Positive => bnd.winding_count2 > 0,
                        FillType::Negative => bnd.winding_count2 < 0,
                    }
                }
            }
            ClipType::XOr => true,
        }
    }

    fn insert_lm_left_and_right_bound(
        &mut self,
        left_bound: usize,
        right_bound: usize,
        active_bounds: &mut Vec<usize>,
        scanbeam: &mut Vec<T>,
        cliptype: ClipType,
        subject_fill_type: FillType,
        clip_fill_type: FillType,
    ) {
        // Both left and right bound
        let lb_abl_itr = self.insert_bound_into_abl(left_bound, right_bound, active_bounds);
        let rb_abl_itr = lb_abl_itr + 1;
        self.set_winding_count(lb_abl_itr, active_bounds, subject_fill_type, clip_fill_type);
        let lb = active_bounds[lb_abl_itr];
        let rb = active_bounds[rb_abl_itr];
        self.bounds[rb].winding_count = self.bounds[lb].winding_count;
        self.bounds[rb].winding_count2 = self.bounds[lb].winding_count2;
        if self.is_contributing(left_bound, cliptype, subject_fill_type, clip_fill_type) {
            let pt = self.bounds[lb].cur().bot;
            self.add_local_minimum_point(lb, rb, active_bounds, pt);
        }

        // Add top of edges to scanbeam
        insert_sorted_scanbeam(scanbeam, self.bounds[lb].cur().top.y);

        if !self.current_edge_is_horizontal(rb) {
            insert_sorted_scanbeam(scanbeam, self.bounds[rb].cur().top.y);
        }
    }

    fn initialize_lm(&mut self, lm: &LocalMinimum) {
        for &(b, side) in &[(lm.left_bound, EdgeSide::Left), (lm.right_bound, EdgeSide::Right)] {
            let bnd = &mut self.bounds[b];
            if !bnd.edges.is_empty() {
                bnd.current_edge = 0;
                bnd.next_edge = 1;
                bnd.current_x = bnd.edges[0].bot.x as f64;
                bnd.winding_count = 0;
                bnd.winding_count2 = 0;
                bnd.side = side;
                bnd.ring = NIL;
            }
        }
    }

    fn insert_local_minima_into_abl(
        &mut self,
        bot_y: T,
        minima_list: &[LocalMinimum],
        minima_sorted: &[usize],
        current_lm: &mut usize,
        active_bounds: &mut Vec<usize>,
        scanbeam: &mut Vec<T>,
        cliptype: ClipType,
        subject_fill_type: FillType,
        clip_fill_type: FillType,
    ) {
        while *current_lm != minima_sorted.len() && bot_y == minima_list[minima_sorted[*current_lm]].y {
            let lm = &minima_list[minima_sorted[*current_lm]];
            self.initialize_lm(lm);
            self.insert_lm_left_and_right_bound(lm.left_bound, lm.right_bound, active_bounds, scanbeam, cliptype, subject_fill_type, clip_fill_type);
            *current_lm += 1;
        }
    }

    fn insert_horizontal_local_minima_into_abl(
        &mut self,
        top_y: T,
        minima_list: &[LocalMinimum],
        minima_sorted: &[usize],
        current_lm: &mut usize,
        active_bounds: &mut Vec<usize>,
        scanbeam: &mut Vec<T>,
        cliptype: ClipType,
        subject_fill_type: FillType,
        clip_fill_type: FillType,
    ) {
        while *current_lm != minima_sorted.len()
            && top_y == minima_list[minima_sorted[*current_lm]].y
            && minima_list[minima_sorted[*current_lm]].minimum_has_horizontal
        {
            let lm = &minima_list[minima_sorted[*current_lm]];
            self.initialize_lm(lm);
            self.insert_lm_left_and_right_bound(lm.left_bound, lm.right_bound, active_bounds, scanbeam, cliptype, subject_fill_type, clip_fill_type);
            *current_lm += 1;
        }
    }

    // ---- ring_util.hpp ----

    fn set_hole_state(&mut self, bnd: usize, active_bounds: &[usize]) {
        // find &bnd from the back
        let mut pos = active_bounds.iter().rposition(|&b| b == bnd);
        // ++bnd_itr (move towards the front)
        let mut i: isize = match pos.take() {
            Some(p) => p as isize - 1,
            None => -1,
        };
        let mut bnd_tmp: usize = NIL;
        // Find first non line ring to the left of current bound.
        while i >= 0 {
            let b = active_bounds[i as usize];
            if b == NIL {
                i -= 1;
                continue;
            }
            if self.bounds[b].ring != NIL {
                if bnd_tmp == NIL {
                    bnd_tmp = b;
                } else if self.bounds[bnd_tmp].ring == self.bounds[b].ring {
                    bnd_tmp = NIL;
                }
            }
            i -= 1;
        }
        let r = self.bounds[bnd].ring;
        if bnd_tmp == NIL {
            self.rings[r].parent = NIL;
            self.children.push(r);
        } else {
            let pr = self.bounds[bnd_tmp].ring;
            self.rings[r].parent = pr;
            self.rings[pr].children.push(r);
        }
    }

    fn update_current_hp_itr(&mut self, scanline_y: T) {
        while self.hot_pixels[self.current_hp_itr].y > scanline_y {
            self.current_hp_itr += 1;
        }
    }

    fn hot_pixel_set_left_to_right(&mut self, y: T, start_x: T, end_x: T, bnd: usize, itr: &mut usize, end: usize, add_end_point: bool) {
        let mut x_min = get_edge_min_x(self.bounds[bnd].cur(), y);
        x_min = x_min.max(start_x);
        let mut x_max = get_edge_max_x(self.bounds[bnd].cur(), y);
        x_max = x_max.min(end_x);
        while *itr != end {
            let hp = self.hot_pixels[*itr];
            if hp.x < x_min {
                *itr += 1;
                continue;
            }
            if hp.x > x_max {
                break;
            }
            if !add_end_point && hp.x == end_x {
                *itr += 1;
                continue;
            }
            let r = self.bounds[bnd].ring;
            let op = self.rings[r].points;
            let to_front = self.bounds[bnd].side == EdgeSide::Left;
            if to_front && hp == self.pt(op) {
                *itr += 1;
                continue;
            } else if !to_front && hp == self.pt(self.prev(op)) {
                *itr += 1;
                continue;
            }
            let new_point = self.create_new_point_before(r, hp, op);
            if to_front {
                self.rings[r].points = new_point;
            }
            *itr += 1;
        }
    }

    /// Reverse iteration over `hot_pixels[end..itr)` from `itr - 1` down to `end`
    /// (`itr`/`end` are reverse iterator bases).
    fn hot_pixel_set_right_to_left(&mut self, y: T, start_x: T, end_x: T, bnd: usize, itr: &mut usize, end: usize, add_end_point: bool) {
        let mut x_min = get_edge_min_x(self.bounds[bnd].cur(), y);
        x_min = x_min.max(end_x);
        let mut x_max = get_edge_max_x(self.bounds[bnd].cur(), y);
        x_max = x_max.min(start_x);
        while *itr != end {
            let hp = self.hot_pixels[*itr - 1];
            if hp.x > x_max {
                *itr -= 1;
                continue;
            }
            if hp.x < x_min {
                break;
            }
            if !add_end_point && hp.x == end_x {
                *itr -= 1;
                continue;
            }
            let r = self.bounds[bnd].ring;
            let op = self.rings[r].points;
            let to_front = self.bounds[bnd].side == EdgeSide::Left;
            if to_front && hp == self.pt(op) {
                *itr -= 1;
                continue;
            } else if !to_front && hp == self.pt(self.prev(op)) {
                *itr -= 1;
                continue;
            }
            let new_point = self.create_new_point_before(r, hp, op);
            if to_front {
                self.rings[r].points = new_point;
            }
            *itr -= 1;
        }
    }

    fn sort_hot_pixels(&mut self) {
        cppstd::sort_by(&mut self.hot_pixels, |pt1: &P, pt2: &P| if pt1.y == pt2.y { pt1.x < pt2.x } else { pt1.y > pt2.y });
        self.hot_pixels.dedup();
    }

    fn insert_hot_pixels_in_path(&mut self, bnd: usize, end_pt: P, add_end_point: bool) {
        if end_pt == self.bounds[bnd].last_point {
            return;
        }

        let start_y = self.bounds[bnd].last_point.y;
        let start_x = self.bounds[bnd].last_point.x;
        let end_y = end_pt.y;
        let end_x = end_pt.x;

        let mut itr = self.current_hp_itr;
        while self.hot_pixels[itr].y <= start_y && itr != 0 {
            itr -= 1;
        }
        let hp_end = self.hot_pixels.len();
        if start_x > end_x {
            while itr != hp_end {
                if self.hot_pixels[itr].y > start_y {
                    itr += 1;
                    continue;
                }
                if self.hot_pixels[itr].y < end_y {
                    break;
                }
                let y = self.hot_pixels[itr].y;
                let last_itr = itr; // reverse iterator base
                while itr != hp_end && self.hot_pixels[itr].y == y {
                    itr += 1;
                }
                let mut first_itr = itr; // reverse iterator base
                let add_end_point_itr = y != end_pt.y || add_end_point;
                self.hot_pixel_set_right_to_left(y, start_x, end_x, bnd, &mut first_itr, last_itr, add_end_point_itr);
            }
        } else {
            while itr != hp_end {
                if self.hot_pixels[itr].y > start_y {
                    itr += 1;
                    continue;
                }
                if self.hot_pixels[itr].y < end_y {
                    break;
                }
                let y = self.hot_pixels[itr].y;
                let mut first_itr = itr;
                while itr != hp_end && self.hot_pixels[itr].y == y {
                    itr += 1;
                }
                let last_itr = itr;
                let add_end_point_itr = y != end_pt.y || add_end_point;
                self.hot_pixel_set_left_to_right(y, start_x, end_x, bnd, &mut first_itr, last_itr, add_end_point_itr);
            }
        }
        self.bounds[bnd].last_point = end_pt;
    }

    fn add_to_hot_pixels(&mut self, pt: P) {
        self.hot_pixels.push(pt);
    }

    fn add_first_point(&mut self, bnd: usize, active_bounds: &[usize], pt: P) {
        let r = self.create_new_ring();
        self.bounds[bnd].ring = r;
        let p = self.create_new_point(r, pt);
        self.rings[r].points = p;
        self.set_hole_state(bnd, active_bounds);
        self.bounds[bnd].last_point = pt;
    }

    fn add_point_to_ring(&mut self, bnd: usize, pt: P) {
        debug_assert!(self.bounds[bnd].ring != NIL);
        // Handle hot pixels
        self.insert_hot_pixels_in_path(bnd, pt, false);

        // bnd.ring->points is the 'Left-most' point & bnd.ring->points->prev is the
        // 'Right-most'
        let r = self.bounds[bnd].ring;
        let op = self.rings[r].points;
        let to_front = self.bounds[bnd].side == EdgeSide::Left;
        if to_front && pt == self.pt(op) {
            return;
        } else if !to_front && pt == self.pt(self.prev(op)) {
            return;
        }
        let new_point = self.create_new_point_before(r, pt, op);
        if to_front {
            self.rings[r].points = new_point;
        }
    }

    fn add_point(&mut self, bnd: usize, active_bounds: &[usize], pt: P) {
        if self.bounds[bnd].ring == NIL {
            self.add_first_point(bnd, active_bounds, pt);
        } else {
            self.add_point_to_ring(bnd, pt);
        }
    }

    fn add_local_minimum_point(&mut self, b1: usize, b2: usize, active_bounds: &[usize], pt: P) {
        if is_horizontal(self.bounds[b2].cur()) || self.bounds[b1].cur().dx > self.bounds[b2].cur().dx {
            self.add_point(b1, active_bounds, pt);
            self.bounds[b2].last_point = pt;
            self.bounds[b2].ring = self.bounds[b1].ring;
            self.bounds[b1].side = EdgeSide::Left;
            self.bounds[b2].side = EdgeSide::Right;
        } else {
            self.add_point(b2, active_bounds, pt);
            self.bounds[b1].last_point = pt;
            self.bounds[b1].ring = self.bounds[b2].ring;
            self.bounds[b1].side = EdgeSide::Right;
            self.bounds[b2].side = EdgeSide::Left;
        }
    }

    fn get_dx(&self, pt1: usize, pt2: usize) -> f64 {
        let (p1, p2) = (self.pt(pt1), self.pt(pt2));
        if p1.y == p2.y {
            f64::INFINITY
        } else {
            (p2.x - p1.x) as f64 / (p2.y - p1.y) as f64
        }
    }

    fn first_is_bottom_point(&self, btm_pt1: usize, btm_pt2: usize) -> bool {
        let mut p = self.prev(btm_pt1);
        while self.peq(p, btm_pt1) && p != btm_pt1 {
            p = self.prev(p);
        }
        let dx1p = self.get_dx(btm_pt1, p).abs();

        p = self.next(btm_pt1);
        while self.peq(p, btm_pt1) && p != btm_pt1 {
            p = self.next(p);
        }
        let dx1n = self.get_dx(btm_pt1, p).abs();

        p = self.prev(btm_pt2);
        while self.peq(p, btm_pt2) && p != btm_pt2 {
            p = self.prev(p);
        }
        let dx2p = self.get_dx(btm_pt2, p).abs();

        p = self.next(btm_pt2);
        while self.peq(p, btm_pt2) && p != btm_pt2 {
            p = self.next(p);
        }
        let dx2n = self.get_dx(btm_pt2, p).abs();

        if values_are_equal(dx1p.max(dx1n), dx2p.max(dx2n)) && values_are_equal(dx1p.min(dx1n), dx2p.min(dx2n)) {
            let mut s = 0;
            let mut bbox = BoxT::default();
            return self.area_from_point(btm_pt1, &mut s, &mut bbox) > 0.0; // if otherwise identical use orientation
        }
        (greater_than_or_equal(dx1p, dx2p) && greater_than_or_equal(dx1p, dx2n)) || (greater_than_or_equal(dx1n, dx2p) && greater_than_or_equal(dx1n, dx2n))
    }

    fn get_bottom_point(&self, mut pp: usize) -> usize {
        let mut dups = NIL;
        let mut p = self.next(pp);
        while p != pp {
            if self.points[p].y > self.points[pp].y {
                pp = p;
                dups = NIL;
            } else if self.points[p].y == self.points[pp].y && self.points[p].x <= self.points[pp].x {
                if self.points[p].x < self.points[pp].x {
                    dups = NIL;
                    pp = p;
                } else {
                    if self.next(p) != pp && self.prev(p) != pp {
                        dups = p;
                    }
                }
            }
            p = self.next(p);
        }
        if dups != NIL {
            // there appears to be at least 2 vertices at bottom_point so ...
            while dups != p {
                if !self.first_is_bottom_point(p, dups) {
                    pp = dups;
                }
                dups = self.next(dups);
                while !self.peq(dups, pp) {
                    dups = self.next(dups);
                }
            }
        }
        pp
    }

    fn get_lower_most_ring(&mut self, ring1: usize, ring2: usize) -> usize {
        // work out which polygon fragment has the correct hole state ...
        if self.rings[ring1].bottom_point == NIL {
            self.rings[ring1].bottom_point = self.get_bottom_point(self.rings[ring1].points);
        }
        if self.rings[ring2].bottom_point == NIL {
            self.rings[ring2].bottom_point = self.get_bottom_point(self.rings[ring2].points);
        }
        let pt1 = self.rings[ring1].bottom_point;
        let pt2 = self.rings[ring2].bottom_point;
        let (p1, p2) = (self.pt(pt1), self.pt(pt2));
        if p1.y > p2.y {
            ring1
        } else if p1.y < p2.y {
            ring2
        } else if p1.x < p2.x {
            ring1
        } else if p1.x > p2.x {
            ring2
        } else if self.next(pt1) == pt1 {
            ring2
        } else if self.next(pt2) == pt2 {
            ring1
        } else if self.first_is_bottom_point(pt1, pt2) {
            ring1
        } else {
            ring2
        }
    }

    fn ring1_child_below_ring2(&self, mut ring1: usize, ring2: usize) -> bool {
        loop {
            ring1 = self.rings[ring1].parent;
            if ring1 == ring2 {
                return true;
            }
            if ring1 == NIL {
                break;
            }
        }
        false
    }

    fn update_points_ring(&mut self, ring: usize) {
        let start = self.rings[ring].points;
        let mut op = start;
        loop {
            self.points[op].ring = ring;
            op = self.prev(op);
            if op == start {
                break;
            }
        }
    }

    fn append_ring(&mut self, b1: usize, b2: usize, active_bounds: &[usize]) {
        // get the start and ends of both output polygons ...
        let out_rec1 = self.bounds[b1].ring;
        let out_rec2 = self.bounds[b2].ring;

        let (keep_ring, keep_bound, remove_ring, remove_bound);
        if self.ring1_child_below_ring2(out_rec1, out_rec2) {
            keep_ring = out_rec2;
            keep_bound = b2;
            remove_ring = out_rec1;
            remove_bound = b1;
        } else if self.ring1_child_below_ring2(out_rec2, out_rec1) {
            keep_ring = out_rec1;
            keep_bound = b1;
            remove_ring = out_rec2;
            remove_bound = b2;
        } else if out_rec1 == self.get_lower_most_ring(out_rec1, out_rec2) {
            keep_ring = out_rec1;
            keep_bound = b1;
            remove_ring = out_rec2;
            remove_bound = b2;
        } else {
            keep_ring = out_rec2;
            keep_bound = b2;
            remove_ring = out_rec1;
            remove_bound = b1;
        }

        // get the start and ends of both output polygons and
        // join b2 poly onto b1 poly and delete pointers to b2 ...
        let p1_lft = self.rings[keep_ring].points;
        let p1_rt = self.prev(p1_lft);
        let p2_lft = self.rings[remove_ring].points;
        let p2_rt = self.prev(p2_lft);

        if self.bounds[keep_bound].side == EdgeSide::Left {
            if self.bounds[remove_bound].side == EdgeSide::Left {
                // z y x a b c
                self.reverse_ring(p2_lft);
                self.points[p2_lft].next = p1_lft;
                self.points[p1_lft].prev = p2_lft;
                self.points[p1_rt].next = p2_rt;
                self.points[p2_rt].prev = p1_rt;
                self.rings[keep_ring].points = p2_rt;
            } else {
                // x y z a b c
                self.points[p2_rt].next = p1_lft;
                self.points[p1_lft].prev = p2_rt;
                self.points[p2_lft].prev = p1_rt;
                self.points[p1_rt].next = p2_lft;
                self.rings[keep_ring].points = p2_lft;
            }
        } else {
            if self.bounds[remove_bound].side == EdgeSide::Right {
                // a b c z y x
                self.reverse_ring(p2_lft);
                self.points[p1_rt].next = p2_rt;
                self.points[p2_rt].prev = p1_rt;
                self.points[p2_lft].next = p1_lft;
                self.points[p1_lft].prev = p2_lft;
            } else {
                // a b c x y z
                self.points[p1_rt].next = p2_lft;
                self.points[p2_lft].prev = p1_rt;
                self.points[p1_lft].prev = p2_rt;
                self.points[p2_rt].next = p1_lft;
            }
        }

        self.rings[keep_ring].bottom_point = NIL;
        let keep_is_hole = self.ring_is_hole_depth(keep_ring);
        let remove_is_hole = self.ring_is_hole_depth(remove_ring);

        self.rings[remove_ring].points = NIL;
        self.rings[remove_ring].bottom_point = NIL;
        if keep_is_hole != remove_is_hole {
            let p = self.rings[keep_ring].parent;
            self.ring1_replaces_ring2(p, remove_ring);
        } else {
            self.ring1_replaces_ring2(keep_ring, remove_ring);
        }

        self.update_points_ring(keep_ring);

        // nb: safe because we only get here via AddLocalMaxPoly
        self.bounds[keep_bound].ring = NIL;
        self.bounds[remove_bound].ring = NIL;

        for &b in active_bounds {
            if b == NIL {
                continue;
            }
            if self.bounds[b].ring == remove_ring {
                self.bounds[b].ring = keep_ring;
                self.bounds[b].side = self.bounds[keep_bound].side;
                break; // Not sure why there is a break here but was transfered logic from angus
            }
        }
    }

    fn add_local_maximum_point(&mut self, b1: usize, b2: usize, pt: P, active_bounds: &[usize]) {
        self.insert_hot_pixels_in_path(b2, pt, false);
        self.add_point(b1, active_bounds, pt);
        if self.bounds[b1].ring == self.bounds[b2].ring {
            self.bounds[b1].ring = NIL;
            self.bounds[b2].ring = NIL;
            // I am not certain that order is important here?
        } else if self.rings[self.bounds[b1].ring].ring_index < self.rings[self.bounds[b2].ring].ring_index {
            self.append_ring(b1, b2, active_bounds);
        } else {
            self.append_ring(b2, b1, active_bounds);
        }
    }

    fn point_in_polygon(&self, pt: P, mut op: usize) -> PointInPolygonResult {
        // returns 0 if false, +1 if true, -1 if pt ON polygon boundary
        let mut result = PointInPolygonResult::OutsidePolygon;
        let start_op = op;
        loop {
            let o = &self.points[op];
            let n = &self.points[o.next];
            if n.y == pt.y {
                if n.x == pt.x || (o.y == pt.y && ((n.x > pt.x) == (o.x < pt.x))) {
                    return PointInPolygonResult::OnPolygon;
                }
            }
            if (o.y < pt.y) != (n.y < pt.y) {
                if o.x >= pt.x {
                    if n.x > pt.x {
                        // Switch between point outside polygon and point inside
                        // polygon
                        result = if result == PointInPolygonResult::OutsidePolygon { PointInPolygonResult::InsidePolygon } else { PointInPolygonResult::OutsidePolygon };
                    } else {
                        let d = (o.x - pt.x) as f64 * (n.y - pt.y) as f64 - (n.x - pt.x) as f64 * (o.y - pt.y) as f64;
                        if value_is_zero(d) {
                            return PointInPolygonResult::OnPolygon;
                        }
                        if (d > 0.0) == (n.y > o.y) {
                            result = if result == PointInPolygonResult::OutsidePolygon { PointInPolygonResult::InsidePolygon } else { PointInPolygonResult::OutsidePolygon };
                        }
                    }
                } else {
                    if n.x > pt.x {
                        let d = (o.x - pt.x) as f64 * (n.y - pt.y) as f64 - (n.x - pt.x) as f64 * (o.y - pt.y) as f64;
                        if value_is_zero(d) {
                            return PointInPolygonResult::OnPolygon;
                        }
                        if (d > 0.0) == (n.y > o.y) {
                            result = if result == PointInPolygonResult::OutsidePolygon { PointInPolygonResult::InsidePolygon } else { PointInPolygonResult::OutsidePolygon };
                        }
                    }
                }
            }
            op = o.next;
            if start_op == op {
                break;
            }
        }
        result
    }

    fn point_in_polygon_d(&self, pt: PD, mut op: usize) -> PointInPolygonResult {
        let mut result = PointInPolygonResult::OutsidePolygon;
        let start_op = op;
        loop {
            let o = &self.points[op];
            let n = &self.points[o.next];
            let op_x = o.x as f64;
            let op_y = o.y as f64;
            let op_next_x = n.x as f64;
            let op_next_y = n.y as f64;
            if values_are_equal(op_next_y, pt.y) {
                if values_are_equal(op_next_x, pt.x) || (values_are_equal(op_y, pt.y) && ((op_next_x > pt.x) == (op_x < pt.x))) {
                    return PointInPolygonResult::OnPolygon;
                }
            }
            if (op_y < pt.y) != (op_next_y < pt.y) {
                if greater_than_or_equal(op_x, pt.x) {
                    if op_next_x > pt.x {
                        result = if result == PointInPolygonResult::OutsidePolygon { PointInPolygonResult::InsidePolygon } else { PointInPolygonResult::OutsidePolygon };
                    } else {
                        let d = (op_x - pt.x) * (op_next_y - pt.y) - (op_next_x - pt.x) * (op_y - pt.y);
                        if value_is_zero(d) {
                            return PointInPolygonResult::OnPolygon;
                        }
                        if (d > 0.0) == (op_next_y > op_y) {
                            result = if result == PointInPolygonResult::OutsidePolygon { PointInPolygonResult::InsidePolygon } else { PointInPolygonResult::OutsidePolygon };
                        }
                    }
                } else {
                    if op_next_x > pt.x {
                        let d = (op_x - pt.x) * (op_next_y - pt.y) - (op_next_x - pt.x) * (op_y - pt.y);
                        if value_is_zero(d) {
                            return PointInPolygonResult::OnPolygon;
                        }
                        if (d > 0.0) == (op_next_y > op_y) {
                            result = if result == PointInPolygonResult::OutsidePolygon { PointInPolygonResult::InsidePolygon } else { PointInPolygonResult::OutsidePolygon };
                        }
                    }
                }
            }
            op = o.next;
            if start_op == op {
                break;
            }
        }
        result
    }

    fn is_convex(&mut self, edge: usize) -> bool {
        let prev = self.prev(edge);
        let next = self.next(edge);
        let v1x = self.points[edge].x - self.points[prev].x;
        let v1y = self.points[edge].y - self.points[prev].y;
        let v2x = self.points[next].x - self.points[edge].x;
        let v2y = self.points[next].y - self.points[edge].y;
        let cross = v1x.wrapping_mul(v2y).wrapping_sub(v2x.wrapping_mul(v1y));
        let r = self.points[edge].ring;
        if cross < 0 && self.ring_area(r) > 0.0 {
            true
        } else {
            cross > 0 && self.ring_area(r) < 0.0
        }
    }

    fn centroid_of_points(&self, edge: usize) -> PD {
        let prev = self.prev(edge);
        let next = self.next(edge);
        PD {
            x: (self.points[prev].x + self.points[edge].x + self.points[next].x) as f64 / 3.0,
            y: (self.points[prev].y + self.points[edge].y + self.points[next].y) as f64 / 3.0,
        }
    }

    fn inside_or_outside_special(&mut self, first_pt: usize, other_poly: usize) -> PointInPolygonResult {
        // We are going to loop through all the points
        // of the original triangle. The goal is to find a convex edge
        // that with its next and previous forms a triangle with its centroid
        // that is within the first ring. Then we will check the other polygon.
        let mut itr = first_pt;
        loop {
            if self.is_convex(itr) {
                let pt = self.centroid_of_points(itr);
                if PointInPolygonResult::InsidePolygon == self.point_in_polygon_d(pt, first_pt) {
                    return self.point_in_polygon_d(pt, other_poly);
                }
            }
            itr = self.next(itr);
            if itr == first_pt {
                break;
            }
        }
        panic!("Could not find a point within the polygon to test");
    }

    fn poly2_contains_poly1(&mut self, ring1: usize, ring2: usize) -> bool {
        let (b1, b2) = (self.rings[ring1].bbox, self.rings[ring2].bbox);
        if !(b2.max.x >= b1.max.x && b2.max.y >= b1.max.y && b2.min.x <= b1.min.x && b2.min.y <= b1.min.y) {
            return false;
        }

        // Check the orientation and the area of the rings.
        if self.ring_area(ring2).abs() < self.ring_area(ring1).abs() {
            return false;
        }
        let outpt1 = self.next(self.rings[ring1].points);
        let outpt2 = self.next(self.rings[ring2].points);
        let mut op = outpt1;
        loop {
            // nb: PointInPolygon returns 0 if false, +1 if true, -1 if pt on polygon
            let res = self.point_in_polygon(self.pt(op), outpt2);
            if res != PointInPolygonResult::OnPolygon {
                return res == PointInPolygonResult::InsidePolygon;
            }
            op = self.next(op);
            if op == outpt1 {
                break;
            }
        }
        let res = self.inside_or_outside_special(outpt1, outpt2);
        res == PointInPolygonResult::InsidePolygon
    }

    // ---- intersect_util.hpp ----

    fn intersection_compare(&self, b1: usize, b2: usize) -> bool {
        !(self.bounds[b1].current_x > self.bounds[b2].current_x && !slopes_equal_edges(self.bounds[b1].cur(), self.bounds[b2].cur()))
    }

    fn intersect_bounds(&mut self, b1: usize, b2: usize, pt: P, cliptype: ClipType, subject_fill_type: FillType, clip_fill_type: FillType, active_bounds: &[usize]) {
        let b1_contributing = self.bounds[b1].ring != NIL;
        let b2_contributing = self.bounds[b2].ring != NIL;

        // update winding counts...
        // assumes that b1 will be to the right of b2 ABOVE the intersection
        if self.bounds[b1].poly_type == self.bounds[b2].poly_type {
            if self.is_even_odd_fill_type(b1, subject_fill_type, clip_fill_type) {
                let tmp = self.bounds[b1].winding_count;
                self.bounds[b1].winding_count = self.bounds[b2].winding_count;
                self.bounds[b2].winding_count = tmp;
            } else {
                let b2_wd = self.bounds[b2].winding_delta as i32;
                let b1_wd = self.bounds[b1].winding_delta as i32;
                if self.bounds[b1].winding_count + b2_wd == 0 {
                    self.bounds[b1].winding_count = -self.bounds[b1].winding_count;
                } else {
                    self.bounds[b1].winding_count += b2_wd;
                }
                if self.bounds[b2].winding_count - b1_wd == 0 {
                    self.bounds[b2].winding_count = -self.bounds[b2].winding_count;
                } else {
                    self.bounds[b2].winding_count -= b1_wd;
                }
            }
        } else {
            if !self.is_even_odd_fill_type(b2, subject_fill_type, clip_fill_type) {
                self.bounds[b1].winding_count2 += self.bounds[b2].winding_delta as i32;
            } else {
                self.bounds[b1].winding_count2 = if self.bounds[b1].winding_count2 == 0 { 1 } else { 0 };
            }
            if !self.is_even_odd_fill_type(b1, subject_fill_type, clip_fill_type) {
                self.bounds[b2].winding_count2 -= self.bounds[b1].winding_delta as i32;
            } else {
                self.bounds[b2].winding_count2 = if self.bounds[b2].winding_count2 == 0 { 1 } else { 0 };
            }
        }

        let (b1_fill_type, b1_fill_type2) =
            if self.bounds[b1].poly_type == PolygonType::Subject { (subject_fill_type, clip_fill_type) } else { (clip_fill_type, subject_fill_type) };
        let (b2_fill_type, b2_fill_type2) =
            if self.bounds[b2].poly_type == PolygonType::Subject { (subject_fill_type, clip_fill_type) } else { (clip_fill_type, subject_fill_type) };

        let wc = |ft: FillType, w: i32| -> i32 {
            match ft {
                FillType::Positive => w,
                FillType::Negative => -w,
                _ => w.abs(),
            }
        };
        let b1_wc = wc(b1_fill_type, self.bounds[b1].winding_count);
        let b2_wc = wc(b2_fill_type, self.bounds[b2].winding_count);

        let swap_sides = |s: &mut Ctx| {
            let side = s.bounds[b1].side;
            s.bounds[b1].side = s.bounds[b2].side;
            s.bounds[b2].side = side;
        };
        let swap_rings = |s: &mut Ctx| {
            let ring = s.bounds[b1].ring;
            s.bounds[b1].ring = s.bounds[b2].ring;
            s.bounds[b2].ring = ring;
        };

        if b1_contributing && b2_contributing {
            if (b1_wc != 0 && b1_wc != 1)
                || (b2_wc != 0 && b2_wc != 1)
                || (self.bounds[b1].poly_type != self.bounds[b2].poly_type && cliptype != ClipType::XOr)
            {
                self.add_local_maximum_point(b1, b2, pt, active_bounds);
            } else {
                self.add_point(b1, active_bounds, pt);
                self.add_point(b2, active_bounds, pt);
                swap_sides(self);
                swap_rings(self);
            }
        } else if b1_contributing {
            if b2_wc == 0 || b2_wc == 1 {
                self.add_point(b1, active_bounds, pt);
                self.bounds[b2].last_point = pt;
                swap_sides(self);
                swap_rings(self);
            }
        } else if b2_contributing {
            if b1_wc == 0 || b1_wc == 1 {
                self.bounds[b1].last_point = pt;
                self.add_point(b2, active_bounds, pt);
                swap_sides(self);
                swap_rings(self);
            }
        } else if (b1_wc == 0 || b1_wc == 1) && (b2_wc == 0 || b2_wc == 1) {
            // neither bound is currently contributing ...
            let b1_wc2 = wc(b1_fill_type2, self.bounds[b1].winding_count2);
            let b2_wc2 = wc(b2_fill_type2, self.bounds[b2].winding_count2);

            if self.bounds[b1].poly_type != self.bounds[b2].poly_type {
                self.add_local_minimum_point(b1, b2, active_bounds, pt);
            } else if b1_wc == 1 && b2_wc == 1 {
                match cliptype {
                    ClipType::Intersection => {
                        if b1_wc2 > 0 && b2_wc2 > 0 {
                            self.add_local_minimum_point(b1, b2, active_bounds, pt);
                        }
                    }
                    ClipType::Union => {
                        if b1_wc2 <= 0 && b2_wc2 <= 0 {
                            self.add_local_minimum_point(b1, b2, active_bounds, pt);
                        }
                    }
                    ClipType::Difference => {
                        if ((self.bounds[b1].poly_type == PolygonType::Clip) && (b1_wc2 > 0) && (b2_wc2 > 0))
                            || ((self.bounds[b1].poly_type == PolygonType::Subject) && (b1_wc2 <= 0) && (b2_wc2 <= 0))
                        {
                            self.add_local_minimum_point(b1, b2, active_bounds, pt);
                        }
                    }
                    ClipType::XOr => {
                        self.add_local_minimum_point(b1, b2, active_bounds, pt);
                    }
                }
            } else {
                swap_sides(self);
            }
        }
    }

    fn update_current_x(&mut self, active_bounds: &[usize], top_y: T) {
        for (pos, &b) in active_bounds.iter().enumerate() {
            let bnd = &mut self.bounds[b];
            bnd.pos = pos;
            bnd.current_x = get_current_x(&bnd.edges[bnd.current_edge], top_y);
        }
    }

    /// `bubble_sort(active_bounds, intersection_compare, on_swap)`.
    fn bubble_sort_bounds<F: FnMut(&mut Ctx, usize, usize)>(&mut self, active_bounds: &mut [usize], mut on_swap: F) {
        if active_bounds.is_empty() {
            return;
        }
        let last = active_bounds.len() - 1;
        loop {
            let mut modified = false;
            for itr in 0..last {
                let next = itr + 1;
                if !self.intersection_compare(active_bounds[itr], active_bounds[next]) {
                    on_swap(self, active_bounds[itr], active_bounds[next]);
                    active_bounds.swap(itr, next);
                    modified = true;
                }
            }
            if !modified {
                break;
            }
        }
    }

    fn process_intersect_list(&mut self, intersects: &mut [IntersectNode], cliptype: ClipType, subject_fill_type: FillType, clip_fill_type: FillType, active_bounds: &mut Vec<usize>) {
        let find_first_bound = |ab: &[usize], node: &IntersectNode| -> usize { ab.iter().position(|&b| b == node.bound1 || b == node.bound2).unwrap_or(ab.len()) };
        let bounds_adjacent = |node: &IntersectNode, next: usize| -> bool { next == node.bound2 || next == node.bound1 };
        for node_itr in 0..intersects.len() {
            let mut b1 = find_first_bound(active_bounds, &intersects[node_itr]);
            let mut b2 = b1 + 1;
            if !bounds_adjacent(&intersects[node_itr], active_bounds[b2]) {
                let mut next_itr = node_itr + 1;
                while next_itr != intersects.len() {
                    let n1 = find_first_bound(active_bounds, &intersects[next_itr]);
                    let n2 = n1 + 1;
                    if bounds_adjacent(&intersects[next_itr], active_bounds[n2]) {
                        b1 = n1;
                        b2 = n2;
                        break;
                    }
                    next_itr += 1;
                }
                if next_itr == intersects.len() {
                    panic!("Could not properly correct intersection order.");
                }
                intersects.swap(node_itr, next_itr);
            }
            let node = intersects[node_itr];
            let pt = round_point(node.pt);
            self.intersect_bounds(node.bound1, node.bound2, pt, cliptype, subject_fill_type, clip_fill_type, active_bounds);
            active_bounds.swap(b1, b2);
        }
    }

    fn process_intersections(&mut self, top_y: T, active_bounds: &mut Vec<usize>, cliptype: ClipType, subject_fill_type: FillType, clip_fill_type: FillType) {
        if active_bounds.is_empty() {
            return;
        }
        self.update_current_x(active_bounds, top_y);
        let mut intersects: Vec<IntersectNode> = Vec::new();
        self.bubble_sort_bounds(active_bounds, |s, b1, b2| {
            let pt = match get_edge_intersection(s.bounds[b1].cur(), s.bounds[b2].cur()) {
                Some(p) => p,
                None => panic!("Trying to find intersection of lines that do not intersect"),
            };
            intersects.push(IntersectNode { bound1: b1, bound2: b2, pt });
        });
        if intersects.is_empty() {
            return;
        }

        // Restore order of active bounds list
        {
            let bounds = &self.bounds;
            cppstd::stable_sort_by(active_bounds, |&b1, &b2| bounds[b1].pos < bounds[b2].pos);
        }

        // Sort the intersection list
        {
            let bounds = &self.bounds;
            cppstd::stable_sort_by(&mut intersects, |node1, node2| {
                if !values_are_equal(node2.pt.y, node1.pt.y) {
                    node2.pt.y < node1.pt.y
                } else {
                    (bounds[node2.bound1].winding_count2 + bounds[node2.bound2].winding_count2)
                        > (bounds[node1.bound1].winding_count2 + bounds[node1.bound2].winding_count2)
                }
            });
        }

        self.process_intersect_list(&mut intersects, cliptype, subject_fill_type, clip_fill_type, active_bounds);
    }

    // ---- process_horizontal.hpp ----

    /// Returns the new position of the iteration in the active bound list.
    fn process_horizontal_left_to_right(
        &mut self,
        scanline_y: T,
        horz_bound_in: usize,
        active_bounds: &mut Vec<usize>,
        scanbeam: &mut Vec<T>,
        cliptype: ClipType,
        subject_fill_type: FillType,
        clip_fill_type: FillType,
    ) -> usize {
        let mut horz_bound = horz_bound_in;
        let mut horizontal_itr_behind = horz_bound;
        let mut shifted = false;
        let hb = active_bounds[horz_bound];
        let is_maxima_edge = self.is_maxima(hb, scanline_y);
        let mut bound_max_pair = active_bounds.len();
        if is_maxima_edge {
            bound_max_pair = self.get_maxima_pair(horz_bound, active_bounds);
        }

        let mut hp_itr = self.current_hp_itr;
        while hp_itr != self.hot_pixels.len()
            && (self.hot_pixels[hp_itr].y > scanline_y || (self.hot_pixels[hp_itr].y == scanline_y && self.hot_pixels[hp_itr].x < self.bounds[hb].cur().bot.x))
        {
            hp_itr += 1;
        }

        let mut bnd = horz_bound + 1;

        while bnd != active_bounds.len() {
            if active_bounds[bnd] == NIL {
                bnd += 1;
                continue;
            }
            let hb = active_bounds[horz_bound];
            let b = active_bounds[bnd];

            // Insert all the hot pixels between the bound and the current horizontal
            while hp_itr != self.hot_pixels.len()
                && self.hot_pixels[hp_itr].y == scanline_y
                && self.hot_pixels[hp_itr].x < wround(self.bounds[b].current_x)
                && self.hot_pixels[hp_itr].x < self.bounds[hb].cur().top.x
            {
                if self.bounds[hb].ring != NIL {
                    let hp = self.hot_pixels[hp_itr];
                    self.add_point_to_ring(hb, hp);
                }
                hp_itr += 1;
            }

            if greater_than(self.bounds[b].current_x, self.bounds[hb].cur().top.x as f64) {
                break;
            }
            // Also break if we've got to the end of an intermediate horizontal edge ...
            // nb: Smaller Dx's are to the right of larger Dx's ABOVE the horizontal.
            if wround(self.bounds[b].current_x) == self.bounds[hb].cur().top.x && self.bounds[hb].has_next() && self.bounds[hb].cur().dx < self.bounds[hb].nxt().dx {
                break;
            }

            // note: may be done multiple times
            if self.bounds[hb].ring != NIL {
                let p = P::new(wround(self.bounds[b].current_x), scanline_y);
                self.add_point_to_ring(hb, p);
            }

            // OK, so far we're still in range of the horizontal Edge  but make sure
            // we're at the last of consec. horizontals when matching with eMaxPair
            if is_maxima_edge && bnd == bound_max_pair {
                let bm = active_bounds[bound_max_pair];
                if self.bounds[hb].ring != NIL && self.bounds[bm].ring != NIL {
                    let top = self.bounds[hb].cur().top;
                    self.add_local_maximum_point(hb, bm, top, active_bounds);
                }
                active_bounds[bound_max_pair] = NIL;
                active_bounds[horz_bound] = NIL;
                if !shifted {
                    horizontal_itr_behind += 1;
                }
                return horizontal_itr_behind;
            }

            let p = P::new(wround(self.bounds[b].current_x), scanline_y);
            self.intersect_bounds(hb, b, p, cliptype, subject_fill_type, clip_fill_type, active_bounds);
            active_bounds.swap(horz_bound, bnd);
            horz_bound = bnd;
            bnd += 1;
            shifted = true;
        }

        let hb = active_bounds[horz_bound];
        if self.bounds[hb].ring != NIL {
            while hp_itr != self.hot_pixels.len() && self.hot_pixels[hp_itr].y == scanline_y && self.hot_pixels[hp_itr].x < self.bounds[hb].cur().top.x {
                let hp = self.hot_pixels[hp_itr];
                self.add_point_to_ring(hb, hp);
                hp_itr += 1;
            }
        }

        if self.bounds[hb].ring != NIL {
            let top = self.bounds[hb].cur().top;
            self.add_point_to_ring(hb, top);
        }

        if self.bounds[hb].has_next() {
            self.next_edge_in_bound(hb, scanbeam);
        } else {
            active_bounds[horz_bound] = NIL;
        }
        if !shifted {
            horizontal_itr_behind += 1;
        }
        horizontal_itr_behind
    }

    fn process_horizontal_right_to_left(
        &mut self,
        scanline_y: T,
        horz_bound_fwd: usize,
        active_bounds: &mut Vec<usize>,
        scanbeam: &mut Vec<T>,
        cliptype: ClipType,
        subject_fill_type: FillType,
        clip_fill_type: FillType,
    ) -> usize {
        let next_bnd_itr = horz_bound_fwd + 1;
        let hbf = active_bounds[horz_bound_fwd];
        let is_maxima_edge = self.is_maxima(hbf, scanline_y);
        // Reverse iterators are represented by the index of the element they refer to;
        // `usize::MAX` (wrapping -1) is rend.
        const REND: usize = usize::MAX;
        let mut bound_max_pair = REND;
        if is_maxima_edge {
            // rev(fwd) then -- : refers to the same element as fwd
            let fwd = self.get_maxima_pair(horz_bound_fwd, active_bounds);
            bound_max_pair = fwd;
        }

        let mut hp_itr_fwd = self.current_hp_itr;
        while hp_itr_fwd != self.hot_pixels.len()
            && (self.hot_pixels[hp_itr_fwd].y < scanline_y || (self.hot_pixels[hp_itr_fwd].y == scanline_y && self.hot_pixels[hp_itr_fwd].x < self.bounds[hbf].cur().top.x))
        {
            hp_itr_fwd += 1;
        }
        // reverse hot pixel iterator: element hp_itr - 1, rend when hp_itr == 0
        let mut hp_itr = hp_itr_fwd;

        let mut bnd: usize = horz_bound_fwd.wrapping_sub(1); // element before horz
        let mut horz_bound: usize = horz_bound_fwd;

        while bnd != REND {
            if active_bounds[bnd] == NIL {
                bnd = bnd.wrapping_sub(1);
                continue;
            }
            let hb = active_bounds[horz_bound];
            let b = active_bounds[bnd];
            while hp_itr != 0
                && self.hot_pixels[hp_itr - 1].y == scanline_y
                && self.hot_pixels[hp_itr - 1].x > wround(self.bounds[b].current_x)
                && self.hot_pixels[hp_itr - 1].x > self.bounds[hb].cur().top.x
            {
                if self.bounds[hb].ring != NIL {
                    let hp = self.hot_pixels[hp_itr - 1];
                    self.add_point_to_ring(hb, hp);
                }
                hp_itr -= 1;
            }

            if less_than(self.bounds[b].current_x, self.bounds[hb].cur().top.x as f64) {
                break;
            }

            // Also break if we've got to the end of an intermediate horizontal edge ...
            // nb: Smaller Dx's are to the right of larger Dx's ABOVE the horizontal.
            if wround(self.bounds[b].current_x) == self.bounds[hb].cur().top.x && self.bounds[hb].has_next() && self.bounds[hb].cur().dx < self.bounds[hb].nxt().dx {
                break;
            }

            // note: may be done multiple times
            if self.bounds[hb].ring != NIL {
                let p = P::new(wround(self.bounds[b].current_x), scanline_y);
                self.add_point_to_ring(hb, p);
            }

            // OK, so far we're still in range of the horizontal Edge  but make sure
            // we're at the last of consec. horizontals when matching with eMaxPair
            if is_maxima_edge && bnd == bound_max_pair {
                let bm = active_bounds[bound_max_pair];
                if self.bounds[hb].ring != NIL && self.bounds[bm].ring != NIL {
                    let top = self.bounds[hb].cur().top;
                    self.add_local_maximum_point(hb, bm, top, active_bounds);
                }
                active_bounds[bound_max_pair] = NIL;
                active_bounds[horz_bound] = NIL;
                return next_bnd_itr;
            }

            let p = P::new(wround(self.bounds[b].current_x), scanline_y);
            self.intersect_bounds(b, hb, p, cliptype, subject_fill_type, clip_fill_type, active_bounds);
            active_bounds.swap(horz_bound, bnd);
            horz_bound = bnd;
            bnd = bnd.wrapping_sub(1);
        }

        let hb = active_bounds[horz_bound];
        if self.bounds[hb].ring != NIL {
            while hp_itr != 0 && self.hot_pixels[hp_itr - 1].y == scanline_y && self.hot_pixels[hp_itr - 1].x > self.bounds[hb].cur().top.x {
                let hp = self.hot_pixels[hp_itr - 1];
                self.add_point_to_ring(hb, hp);
                hp_itr -= 1;
            }
        }

        if self.bounds[hb].ring != NIL {
            let top = self.bounds[hb].cur().top;
            self.add_point_to_ring(hb, top);
        }

        if self.bounds[hb].has_next() {
            self.next_edge_in_bound(hb, scanbeam);
        } else {
            active_bounds[horz_bound] = NIL;
        }
        next_bnd_itr
    }

    fn process_horizontals(&mut self, scanline_y: T, active_bounds: &mut Vec<usize>, scanbeam: &mut Vec<T>, cliptype: ClipType, subject_fill_type: FillType, clip_fill_type: FillType) {
        let mut bnd_itr = 0usize;
        while bnd_itr != active_bounds.len() {
            let b = active_bounds[bnd_itr];
            if b != NIL && self.current_edge_is_horizontal(b) {
                bnd_itr = if self.bounds[b].cur().bot.x < self.bounds[b].cur().top.x {
                    self.process_horizontal_left_to_right(scanline_y, bnd_itr, active_bounds, scanbeam, cliptype, subject_fill_type, clip_fill_type)
                } else {
                    self.process_horizontal_right_to_left(scanline_y, bnd_itr, active_bounds, scanbeam, cliptype, subject_fill_type, clip_fill_type)
                };
            } else {
                bnd_itr += 1;
            }
        }
        active_bounds.retain(|&b| b != NIL);
    }

    // ---- process_maxima.hpp ----

    fn do_maxima(&mut self, bnd_in: usize, bnd_max_pair: usize, cliptype: ClipType, subject_fill_type: FillType, clip_fill_type: FillType, active_bounds: &mut Vec<usize>) -> usize {
        let mut bnd = bnd_in;
        let mut bnd_next = bnd + 1;
        let mut return_bnd = bnd;
        let mut skipped = false;
        while bnd_next != active_bounds.len() && bnd_next != bnd_max_pair {
            if active_bounds[bnd_next] == NIL {
                bnd_next += 1;
                continue;
            }
            skipped = true;
            let b = active_bounds[bnd];
            let top = self.bounds[b].cur().top;
            self.intersect_bounds(b, active_bounds[bnd_next], top, cliptype, subject_fill_type, clip_fill_type, active_bounds);
            active_bounds.swap(bnd, bnd_next);
            bnd = bnd_next;
            bnd_next += 1;
        }

        let b = active_bounds[bnd];
        let bm = active_bounds[bnd_max_pair];
        if self.bounds[b].ring != NIL && self.bounds[bm].ring != NIL {
            let top = self.bounds[b].cur().top;
            self.add_local_maximum_point(b, bm, top, active_bounds);
        } else if self.bounds[b].ring != NIL || self.bounds[bm].ring != NIL {
            panic!("DoMaxima error");
        }
        active_bounds[bnd_max_pair] = NIL;
        active_bounds[bnd] = NIL;
        if !skipped {
            return_bnd += 1;
        }
        return_bnd
    }

    fn process_edges_at_top_of_scanbeam(
        &mut self,
        top_y: T,
        active_bounds: &mut Vec<usize>,
        scanbeam: &mut Vec<T>,
        minima_list: &[LocalMinimum],
        minima_sorted: &[usize],
        current_lm: &mut usize,
        cliptype: ClipType,
        subject_fill_type: FillType,
        clip_fill_type: FillType,
    ) {
        let mut bnd = 0usize;
        while bnd != active_bounds.len() {
            let b = active_bounds[bnd];
            if b == NIL {
                bnd += 1;
                continue;
            }
            // 1. Process maxima, treating them as if they are "bent" horizontal edges,
            // but exclude maxima with horizontal edges.

            let mut is_maxima_edge = self.is_maxima(b, top_y);

            if is_maxima_edge {
                let bnd_max_pair = self.get_maxima_pair(bnd, active_bounds);
                is_maxima_edge = (bnd_max_pair == active_bounds.len() || !self.current_edge_is_horizontal(active_bounds[bnd_max_pair]))
                    && self.is_maxima(active_bounds[bnd_max_pair], top_y);
                if is_maxima_edge {
                    bnd = self.do_maxima(bnd, bnd_max_pair, cliptype, subject_fill_type, clip_fill_type, active_bounds);
                    continue;
                }
            }

            // 2. Promote horizontal edges.
            if self.is_intermediate(b, top_y) && self.next_edge_is_horizontal(b) {
                if self.bounds[b].ring != NIL {
                    let top = self.bounds[b].cur().top;
                    self.insert_hot_pixels_in_path(b, top, false);
                }
                self.next_edge_in_bound(b, scanbeam);
                if self.bounds[b].ring != NIL {
                    let bot = self.bounds[b].cur().bot;
                    self.add_point_to_ring(b, bot);
                }
            } else {
                let x = get_current_x(self.bounds[b].cur(), top_y);
                self.bounds[b].current_x = x;
            }

            bnd += 1;
        }
        active_bounds.retain(|&b| b != NIL);

        self.insert_horizontal_local_minima_into_abl(top_y, minima_list, minima_sorted, current_lm, active_bounds, scanbeam, cliptype, subject_fill_type, clip_fill_type);

        self.process_horizontals(top_y, active_bounds, scanbeam, cliptype, subject_fill_type, clip_fill_type);

        // 4. Promote intermediate vertices

        for bnd in 0..active_bounds.len() {
            let b = active_bounds[bnd];
            if self.is_intermediate(b, top_y) {
                if self.bounds[b].ring != NIL {
                    let top = self.bounds[b].cur().top;
                    self.add_point_to_ring(b, top);
                }
                self.next_edge_in_bound(b, scanbeam);
            }
        }
    }

    // ---- snap_rounding.hpp ----

    fn process_hot_pixel_intersections(&mut self, top_y: T, active_bounds: &mut Vec<usize>) {
        if active_bounds.is_empty() {
            return;
        }
        self.update_current_x(active_bounds, top_y);
        // bubblesort ...
        self.bubble_sort_bounds(active_bounds, |s, b1, b2| {
            let pt = match get_edge_intersection(s.bounds[b1].cur(), s.bounds[b2].cur()) {
                Some(p) => p,
                None => panic!("Trying to find intersection of lines that do not intersect"),
            };
            s.add_to_hot_pixels(round_point(pt));
        });
    }

    fn horizontals_at_top_scanbeam(&mut self, top_y: T, bnd_curr: &mut usize, active_bounds: &mut Vec<usize>) -> bool {
        let mut shifted = false;
        let bc = active_bounds[*bnd_curr];
        {
            let x = self.bounds[bc].cur().top.x as f64;
            self.bounds[bc].current_x = x;
        }
        if self.bounds[bc].cur().bot.x < self.bounds[bc].cur().top.x {
            let mut bnd_next = *bnd_curr + 1;
            while bnd_next != active_bounds.len()
                && (active_bounds[bnd_next] == NIL || self.bounds[active_bounds[bnd_next]].current_x < self.bounds[active_bounds[*bnd_curr]].current_x)
            {
                let bn = active_bounds[bnd_next];
                if bn != NIL && self.bounds[bn].cur().top.y != top_y && self.bounds[bn].cur().bot.y != top_y {
                    let pt = P::new(wround(self.bounds[bn].current_x), top_y);
                    self.add_to_hot_pixels(pt);
                }
                active_bounds.swap(*bnd_curr, bnd_next);
                *bnd_curr += 1;
                bnd_next += 1;
                shifted = true;
            }
        } else {
            if *bnd_curr != 0 {
                let mut bnd_prev = *bnd_curr - 1;
                while *bnd_curr != 0 && (active_bounds[bnd_prev] == NIL || self.bounds[active_bounds[bnd_prev]].current_x > self.bounds[active_bounds[*bnd_curr]].current_x) {
                    let bp = active_bounds[bnd_prev];
                    if bp != NIL && self.bounds[bp].cur().top.y != top_y && self.bounds[bp].cur().bot.y != top_y {
                        let pt = P::new(wround(self.bounds[bp].current_x), top_y);
                        self.add_to_hot_pixels(pt);
                    }
                    active_bounds.swap(*bnd_curr, bnd_prev);
                    *bnd_curr -= 1;
                    if *bnd_curr != 0 {
                        bnd_prev -= 1;
                    }
                }
            }
        }
        shifted
    }

    fn process_hot_pixel_edges_at_top_of_scanbeam(&mut self, top_y: T, scanbeam: &mut Vec<T>, active_bounds: &mut Vec<usize>) {
        let mut bnd = 0usize;
        while bnd != active_bounds.len() {
            let current_bound = active_bounds[bnd];
            if current_bound == NIL {
                bnd += 1;
                continue;
            }
            let mut bnd_curr = bnd;
            let mut shifted = false;
            while self.bounds[current_bound].current_edge != self.bounds[current_bound].edges.len() && self.bounds[current_bound].cur().top.y == top_y {
                let top = self.bounds[current_bound].cur().top;
                self.add_to_hot_pixels(top);
                if is_horizontal(self.bounds[current_bound].cur()) {
                    if self.horizontals_at_top_scanbeam(top_y, &mut bnd_curr, active_bounds) {
                        shifted = true;
                    }
                }
                self.next_edge_in_bound(current_bound, scanbeam);
            }
            if self.bounds[current_bound].current_edge == self.bounds[current_bound].edges.len() {
                active_bounds[bnd_curr] = NIL;
            }
            if !shifted {
                bnd += 1;
            }
        }
        active_bounds.retain(|&b| b != NIL);
    }

    fn insert_local_minima_into_abl_hot_pixel(&mut self, top_y: T, minima_list: &[LocalMinimum], minima_sorted: &[usize], lm: &mut usize, active_bounds: &mut Vec<usize>, scanbeam: &mut Vec<T>) {
        while *lm != minima_sorted.len() && minima_list[minima_sorted[*lm]].y == top_y {
            let m = &minima_list[minima_sorted[*lm]];
            let pt = self.bounds[m.left_bound].edges[0].bot;
            self.add_to_hot_pixels(pt);
            for &b in &[m.left_bound, m.right_bound] {
                let bnd = &mut self.bounds[b];
                bnd.current_edge = 0;
                bnd.next_edge = 1;
                bnd.current_x = bnd.edges[0].bot.x as f64;
            }
            let lb_abl_itr = self.insert_bound_into_abl(m.left_bound, m.right_bound, active_bounds);
            let lb = active_bounds[lb_abl_itr];
            if !self.current_edge_is_horizontal(lb) {
                insert_sorted_scanbeam(scanbeam, self.bounds[lb].cur().top.y);
            }
            let rb = active_bounds[lb_abl_itr + 1];
            if !self.current_edge_is_horizontal(rb) {
                insert_sorted_scanbeam(scanbeam, self.bounds[rb].cur().top.y);
            }
            *lm += 1;
        }
    }

    fn build_hot_pixels(&mut self, minima_list: &[LocalMinimum]) {
        let mut active_bounds: Vec<usize> = Vec::new();
        let mut scanbeam: Vec<T> = Vec::new();
        let mut scanline_y: T = T::MAX;

        let minima_sorted = sorted_minima(minima_list);
        let mut current_lm = 0usize;

        setup_scanbeam(minima_list, &mut scanbeam);

        // Estimate size for reserving hot pixels
        let mut reserve = 0usize;
        for lm in minima_list {
            reserve += self.bounds[lm.left_bound].edges.len() + 2;
            reserve += self.bounds[lm.right_bound].edges.len() + 2;
        }
        self.hot_pixels.reserve(reserve);

        while pop_from_scanbeam(&mut scanline_y, &mut scanbeam) || current_lm != minima_sorted.len() {
            self.process_hot_pixel_intersections(scanline_y, &mut active_bounds);

            self.insert_local_minima_into_abl_hot_pixel(scanline_y, minima_list, &minima_sorted, &mut current_lm, &mut active_bounds, &mut scanbeam);

            self.process_hot_pixel_edges_at_top_of_scanbeam(scanline_y, &mut scanbeam, &mut active_bounds);
        }
        self.sort_hot_pixels();
    }

    // ---- vatti.hpp ----

    fn execute_vatti(&mut self, minima_list: &[LocalMinimum], cliptype: ClipType, subject_fill_type: FillType, clip_fill_type: FillType) {
        let mut active_bounds: Vec<usize> = Vec::new();
        let mut scanbeam: Vec<T> = Vec::new();
        let mut scanline_y: T = T::MAX;

        let minima_sorted = sorted_minima(minima_list);
        let mut current_lm = 0usize;

        setup_scanbeam(minima_list, &mut scanbeam);
        self.current_hp_itr = 0;

        while pop_from_scanbeam(&mut scanline_y, &mut scanbeam) || current_lm != minima_sorted.len() {
            self.process_intersections(scanline_y, &mut active_bounds, cliptype, subject_fill_type, clip_fill_type);

            self.update_current_hp_itr(scanline_y);

            // First we process bounds that has already been added to the active bound list --
            // if the active bound list is empty local minima that are at this scanline_y and
            // have a horizontal edge at the local minima will be processed
            self.process_edges_at_top_of_scanbeam(scanline_y, &mut active_bounds, &mut scanbeam, minima_list, &minima_sorted, &mut current_lm, cliptype, subject_fill_type, clip_fill_type);

            // Next we will add local minima bounds to the active bounds list that are on the local
            // minima queue at
            // this current scanline_y
            self.insert_local_minima_into_abl(scanline_y, minima_list, &minima_sorted, &mut current_lm, &mut active_bounds, &mut scanbeam, cliptype, subject_fill_type, clip_fill_type);
        }
    }

    // ---- build_result.hpp ----

    fn push_ring_to_polygon(&mut self, poly: &mut Polygon, r: usize, reverse_output: bool) {
        let mut lr: LinearRing = Vec::with_capacity(self.ring_size(r) + 1);
        let first_pt = self.rings[r].points;
        let mut pt_it = first_pt;
        if reverse_output {
            loop {
                lr.push(self.pt(pt_it));
                pt_it = self.next(pt_it);
                if pt_it == first_pt {
                    break;
                }
            }
        } else {
            loop {
                lr.push(self.pt(pt_it));
                pt_it = self.prev(pt_it);
                if pt_it == first_pt {
                    break;
                }
            }
        }
        lr.push(self.pt(first_pt)); // close the ring
        poly.push(lr);
    }

    fn build_result_polygons(&mut self, solution: &mut MultiPolygon, rings: &[usize], reverse_output: bool) {
        for &r in rings {
            if r == NIL {
                continue;
            }
            debug_assert!(self.rings[r].points != NIL);
            solution.push(Vec::new());
            let mut poly = solution.pop().unwrap();
            self.push_ring_to_polygon(&mut poly, r, reverse_output);
            let children = self.rings[r].children.clone();
            for &c in &children {
                if c == NIL {
                    continue;
                }
                debug_assert!(self.rings[c].points != NIL);
                self.push_ring_to_polygon(&mut poly, c, reverse_output);
            }
            solution.push(poly);
            for &c in &children {
                if c == NIL {
                    continue;
                }
                if !self.rings[c].children.is_empty() {
                    let gc = self.rings[c].children.clone();
                    self.build_result_polygons(solution, &gc, reverse_output);
                }
            }
        }
    }

    fn build_result(&mut self, solution: &mut MultiPolygon, reverse_output: bool) {
        let children = self.children.clone();
        self.build_result_polygons(solution, &children, reverse_output);
    }

    // ---- topology_correction.hpp ----

    fn point_ptr_less(&self, op1: usize, op2: usize) -> bool {
        let (p1, p2) = (&self.points[op1], &self.points[op2]);
        if p1.y != p2.y {
            p1.y > p2.y
        } else if p1.x != p2.x {
            p1.x < p2.x
        } else {
            let depth_1 = self.ring_depth(p1.ring);
            let depth_2 = self.ring_depth(p2.ring);
            depth_1 > depth_2
        }
    }

    fn find_intersect_loop(
        &mut self,
        dupe_ring: &mut ConnectionMap,
        i_list: &mut VecDeque<(usize, PointPtrPair)>,
        ring_parent: usize,
        ring_origin: usize,
        ring_search: usize,
        visited: &mut HashSet<usize>,
        orig_pt: usize,
        prev_pt: usize,
    ) -> bool {
        {
            let mut i = 0usize;
            while i < dupe_ring.len_of(ring_search) {
                let it = dupe_ring.get(ring_search, i);
                let it_ring1 = self.points[it.op1].ring;
                let it_ring2 = self.points[it.op2].ring;
                if it_ring1 == NIL || it_ring2 == NIL || it_ring1 != ring_search || (!self.ring_is_hole_flag(it_ring1) && !self.ring_is_hole_flag(it_ring2)) {
                    dupe_ring.erase_at(ring_search, i);
                    continue;
                }
                if it_ring2 == ring_origin
                    && (ring_parent == it_ring2 || ring_parent == self.rings[it_ring2].parent)
                    && !self.peq(prev_pt, it.op2)
                    && !self.peq(orig_pt, it.op2)
                {
                    i_list.push_front((ring_search, it));
                    return true;
                }
                i += 1;
            }
        }
        visited.insert(ring_search);
        // Check for connection through chain of other intersections
        let mut i = 0usize;
        while i < dupe_ring.len_of(ring_search) {
            let it = dupe_ring.get(ring_search, i);
            let it_ring = self.points[it.op2].ring;
            if visited.contains(&it_ring)
                || it_ring == NIL
                || (ring_parent != it_ring && ring_parent != self.rings[it_ring].parent)
                || value_is_zero(self.ring_area(it_ring))
                || self.peq(prev_pt, it.op2)
            {
                i += 1;
                continue;
            }
            if self.find_intersect_loop(dupe_ring, i_list, ring_parent, ring_origin, it_ring, visited, orig_pt, it.op2) {
                i_list.push_front((ring_search, it));
                return true;
            }
            i += 1;
        }
        false
    }

    fn correct_orientations(&mut self) {
        for r in 0..self.rings.len() {
            if self.rings[r].points == NIL {
                continue;
            }
            self.recalculate_stats(r);
            if self.ring_size(r) < 3 {
                self.remove_ring_and_points(r, false, true);
                continue;
            }
            if self.ring_is_hole_depth(r) != self.ring_is_hole_flag(r) {
                let pts = self.rings[r].points;
                self.reverse_ring(pts);
                self.recalculate_stats(r);
            }
        }
    }

    fn sort_ring_points(&self, r: usize) -> Vec<usize> {
        let mut sorted_points = Vec::new();
        let mut point_itr = self.rings[r].points;
        let last_point = self.prev(point_itr);
        while point_itr != last_point {
            sorted_points.push(point_itr);
            point_itr = self.next(point_itr);
        }
        sorted_points.push(last_point);
        cppstd::stable_sort_by(&mut sorted_points, |&pt1, &pt2| {
            let (a, b) = (&self.points[pt1], &self.points[pt2]);
            if a.y != b.y {
                return a.y > b.y;
            }
            a.x < b.x
        });
        sorted_points
    }

    fn correct_self_intersection(&mut self, pt1: usize, pt2: usize) -> usize {
        if self.points[pt1].ring != self.points[pt2].ring {
            return NIL;
        }

        let ring = self.points[pt1].ring;

        // split the polygon into two ...
        let pt3 = self.prev(pt1);
        let pt4 = self.prev(pt2);
        self.points[pt1].prev = pt4;
        self.points[pt4].next = pt1;
        self.points[pt2].prev = pt3;
        self.points[pt3].next = pt2;

        let new_ring = self.create_new_ring();
        let mut size_1 = 0;
        let mut size_2 = 0;
        let mut box1 = BoxT::default();
        let mut box2 = BoxT::default();
        let area_1 = self.area_from_point(pt1, &mut size_1, &mut box1);
        let area_2 = self.area_from_point(pt2, &mut size_2, &mut box2);
        if area_1.abs() > area_2.abs() {
            self.rings[ring].points = pt1;
            self.set_stats(ring, area_1, size_1, box1);
            self.rings[new_ring].points = pt2;
            self.set_stats(new_ring, area_2, size_2, box2);
        } else {
            self.rings[ring].points = pt2;
            self.set_stats(ring, area_2, size_2, box2);
            self.rings[new_ring].points = pt1;
            self.set_stats(new_ring, area_1, size_1, box1);
        }
        self.update_points_ring(new_ring);
        new_ring
    }

    fn correct_repeated_points(&mut self, new_rings: &mut Vec<usize>, pts: &[usize]) {
        for i1 in 0..pts.len() {
            if self.points[pts[i1]].ring == NIL {
                continue;
            }
            for i2 in i1 + 1..pts.len() {
                if self.points[pts[i2]].ring == NIL {
                    continue;
                }
                let new_ring = self.correct_self_intersection(pts[i1], pts[i2]);
                if new_ring != NIL {
                    new_rings.push(new_ring);
                }
            }
        }
    }

    /// Iterates over runs of equal points in `points`, calling `f` for each run (as in the
    /// `count`/`prev_itr` loops of the C++ code).
    fn for_each_repeat_run<F: FnMut(&mut Ctx, &[usize])>(&mut self, points: &[usize], mut f: F) {
        let mut count = 0usize;
        let mut prev_itr = 0usize;
        let mut itr = 1usize;
        while itr < points.len() {
            if self.peq(points[prev_itr], points[itr]) {
                count += 1;
                prev_itr += 1;
                itr += 1;
                if itr != points.len() {
                    continue;
                } else {
                    prev_itr += 1;
                }
            } else {
                prev_itr += 1;
                itr += 1;
            }
            if count == 0 {
                continue;
            }
            let first = prev_itr - (count + 1);
            let run: Vec<usize> = points[first..prev_itr].to_vec();
            f(self, &run);
            count = 0;
        }
    }

    fn find_and_correct_repeated_points(&mut self, r: usize, new_rings: &mut Vec<usize>) {
        let sorted_points = self.sort_ring_points(r);
        // Find sets of repeated points
        self.for_each_repeat_run(&sorted_points, |s, run| s.correct_repeated_points(new_rings, run));
    }

    fn reassign_children_if_necessary(&mut self, new_ring: usize, sibling_ring: usize, new_rings: &[usize]) {
        let children = if sibling_ring == NIL { self.children.clone() } else { self.rings[sibling_ring].children.clone() };
        // NOTE: iterating a copy is equivalent here: reassign_as_child only nulls entries in this
        // vector (which are skipped anyway) and appends to the new ring's children.
        for (i, &c0) in children.iter().enumerate() {
            // Re-read the live entry: it may have been nulled by an earlier reassignment.
            let c = if sibling_ring == NIL { self.children.get(i).copied().unwrap_or(NIL) } else { self.rings[sibling_ring].children.get(i).copied().unwrap_or(NIL) };
            let _ = c0;
            if c == NIL {
                continue;
            }
            if new_rings.contains(&c) {
                continue;
            }
            if self.poly2_contains_poly1(c, new_ring) {
                self.reassign_as_child(c, new_ring);
            }
        }
    }

    fn find_parent_in_tree(&mut self, r: usize, possible_parent: usize) -> bool {
        // Before starting this function we have already checked the parent of the
        // possible parent ring. So we will search the tree to find the deepest
        // possible parent.
        let n = self.rings[possible_parent].children.len();
        for i in 0..n {
            let c = self.rings[possible_parent].children.get(i).copied().unwrap_or(NIL);
            if c == NIL {
                continue;
            }
            let m = self.rings[c].children.len();
            for j in 0..m {
                let gc = self.rings[c].children.get(j).copied().unwrap_or(NIL);
                if gc == NIL {
                    continue;
                }
                if self.find_parent_in_tree(r, gc) {
                    return true;
                }
            }
        }
        if self.poly2_contains_poly1(r, possible_parent) {
            self.reassign_as_child(r, possible_parent);
            return true;
        }
        false
    }

    fn assign_new_ring_parents(&mut self, original_ring: usize, new_rings: &mut Vec<usize>) {
        // First lets remove any rings that have zero area
        // or have no points
        let mut kept = Vec::with_capacity(new_rings.len());
        for &r in new_rings.iter() {
            let remove = if self.rings[r].points == NIL { true } else { value_is_zero(self.ring_area(r)) };
            if !remove {
                kept.push(r);
            }
        }
        *new_rings = kept;

        if new_rings.is_empty() {
            // No new rings created simply return;
            return;
        }

        // We should not have to re-assign the parent of the original ring
        // because we always maintained the largest ring to be the original ring.
        let original_ring_area = self.ring_area(original_ring);
        let original_positive = original_ring_area > 0.0;

        // If there is only one new ring the logic is very simple and we
        // do not have to check which ring contains, we only need to compare
        // the areas of the original ring and that of the new ring.
        if new_rings.len() == 1 {
            let nr = new_rings[0];
            let new_ring_area = self.ring_area(nr);
            let new_positive = new_ring_area > 0.0;
            if original_positive == new_positive {
                // The rings should be siblings!
                let p = self.rings[original_ring].parent;
                self.assign_as_child(nr, p);
                self.reassign_children_if_necessary(nr, original_ring, new_rings);
            } else {
                // The new ring is a child of original ring
                // Check the
                self.assign_as_child(nr, original_ring);
                let p = self.rings[original_ring].parent;
                self.reassign_children_if_necessary(nr, p, new_rings);
            }
            return;
        }

        // Now we want to sort rings from the largest in absolute area to the smallest
        // as we will assign the rings with the largest areas first
        {
            for &r in new_rings.iter() {
                self.ring_area(r);
            }
            let rings = &self.rings;
            cppstd::stable_sort_by(new_rings, |&r1, &r2| rings[r1].area.abs() > rings[r2].area.abs());
        }

        for r_itr in 0..new_rings.len() {
            let r = new_rings[r_itr];
            let new_ring_area = self.ring_area(r);
            let new_positive = new_ring_area > 0.0;
            let same_orientation = new_positive == original_positive;
            let mut found = false;
            // First lets check the other new rings
            for s_itr in 0..r_itr {
                let s = new_rings[s_itr];
                if self.rings[s].parent != self.rings[original_ring].parent {
                    continue;
                }
                if same_orientation {
                    let n = self.rings[s].children.len();
                    for i in 0..n {
                        let s_child = self.rings[s].children.get(i).copied().unwrap_or(NIL);
                        if s_child == NIL {
                            continue;
                        }
                        if self.find_parent_in_tree(r, s_child) {
                            self.reassign_children_if_necessary(r, original_ring, new_rings);
                            found = true;
                            break;
                        }
                    }
                } else {
                    if self.find_parent_in_tree(r, s) {
                        let p = self.rings[original_ring].parent;
                        self.reassign_children_if_necessary(r, p, new_rings);
                        found = true;
                    }
                }
                if found {
                    break;
                }
            }
            if found {
                continue;
            }
            if same_orientation {
                let n = self.rings[original_ring].children.len();
                for i in 0..n {
                    let o_child = self.rings[original_ring].children.get(i).copied().unwrap_or(NIL);
                    if o_child == NIL {
                        continue;
                    }
                    if self.find_parent_in_tree(r, o_child) {
                        self.reassign_children_if_necessary(r, original_ring, new_rings);
                        found = true;
                        break;
                    }
                }
                if !found {
                    // If we didn't find any parent at this point, the new ring is a child of the original ring's parent.
                    let p = self.rings[original_ring].parent;
                    self.assign_as_child(r, p);
                    self.reassign_children_if_necessary(r, original_ring, new_rings);
                }
            } else {
                // If we did not find a child of the original ring that contains this ring
                // then it must be a child of the original ring
                if self.find_parent_in_tree(r, original_ring) {
                    let p = self.rings[original_ring].parent;
                    self.reassign_children_if_necessary(r, p, new_rings);
                } else {
                    panic!("Unable to find a proper parent ring");
                }
            }
        }
    }

    fn correct_ring_self_intersections(&mut self, r: usize, correct_tree: bool) -> bool {
        if self.rings[r].corrected || self.rings[r].points == NIL {
            return false;
        }

        let mut new_rings: Vec<usize> = Vec::new();

        self.find_and_correct_repeated_points(r, &mut new_rings);

        if correct_tree {
            self.assign_new_ring_parents(r, &mut new_rings);
        }

        self.rings[r].corrected = true;
        true
    }

    fn process_single_intersection(&mut self, connection_map: &mut ConnectionMap, op_j: usize, op_k: usize) {
        let ring_j = self.points[op_j].ring;
        let ring_k = self.points[op_k].ring;
        if ring_j == ring_k {
            return;
        }

        if !self.ring_is_hole_flag(ring_j) && !self.ring_is_hole_flag(ring_k) {
            // Both are not holes, return nothing to do.
            return;
        }

        let mut ring_origin;
        let ring_search;
        let mut ring_parent;
        let mut op_origin_1;
        let mut op_origin_2;
        if !self.ring_is_hole_flag(ring_j) {
            ring_origin = ring_j;
            ring_parent = ring_origin;
            ring_search = ring_k;
            op_origin_1 = op_j;
            op_origin_2 = op_k;
        } else if !self.ring_is_hole_flag(ring_k) {
            ring_origin = ring_k;
            ring_parent = ring_origin;
            ring_search = ring_j;
            op_origin_1 = op_k;
            op_origin_2 = op_j;
        } else {
            // both are holes
            // Order doesn't matter
            ring_origin = ring_j;
            ring_parent = self.rings[ring_origin].parent;
            ring_search = ring_k;
            op_origin_1 = op_j;
            op_origin_2 = op_k;
        }
        if ring_parent != self.rings[ring_search].parent {
            // The two holes do not have the same parent, do not add them
            // simply return!
            return;
        }
        let mut found = false;
        let mut i_list: VecDeque<(usize, PointPtrPair)> = VecDeque::new();
        {
            let mut i = 0usize;
            while i < connection_map.len_of(ring_search) {
                let it = connection_map.get(ring_search, i);
                if self.points[it.op1].ring == NIL {
                    connection_map.erase_at(ring_search, i);
                    continue;
                }
                if self.points[it.op2].ring == NIL {
                    connection_map.erase_at(ring_search, i);
                    continue;
                }
                let it_ring2 = self.points[it.op2].ring;
                if it_ring2 == ring_origin {
                    found = true;
                    if !self.peq(op_origin_1, it.op2) {
                        i_list.push_back((ring_search, it));
                        break;
                    }
                }
                i += 1;
            }
        }
        if i_list.is_empty() {
            let mut visited: HashSet<usize> = HashSet::new();
            visited.insert(ring_search);
            let mut i = 0usize;
            while i < connection_map.len_of(ring_search) {
                let it = connection_map.get(ring_search, i);
                let it_ring = self.points[it.op2].ring;
                if it_ring != ring_search
                    && !self.peq(op_origin_2, it.op2)
                    && it_ring != NIL
                    && (ring_parent == it_ring || ring_parent == self.rings[it_ring].parent)
                    && !value_is_zero(self.ring_area(it_ring))
                    && self.find_intersect_loop(connection_map, &mut i_list, ring_parent, ring_origin, it_ring, &mut visited, op_origin_2, it.op2)
                {
                    found = true;
                    i_list.push_front((ring_search, it));
                    break;
                }
                i += 1;
            }
        }
        if !found {
            let int_pt_origin = PointPtrPair { op1: op_origin_1, op2: op_origin_2 };
            let int_pt_search = PointPtrPair { op1: op_origin_2, op2: op_origin_1 };
            connection_map.emplace(ring_origin, int_pt_origin);
            connection_map.emplace(ring_search, int_pt_search);
            return;
        }

        if i_list.is_empty() {
            // The situation where both origin and search are holes might have a missing
            // search condition, we must check if a new pair must be added.
            let mut missing = true;
            for i in 0..connection_map.len_of(ring_origin) {
                let it = connection_map.get(ring_origin, i);
                let it_ring2 = self.points[it.op2].ring;
                if it_ring2 == ring_search {
                    missing = false;
                }
            }
            if missing {
                let int_pt_origin = PointPtrPair { op1: op_origin_1, op2: op_origin_2 };
                connection_map.emplace(ring_origin, int_pt_origin);
            }
            return;
        }

        if self.ring_is_hole_flag(ring_origin) {
            for i_ring in i_list.iter_mut() {
                let ring_itr = i_ring.0;
                if !self.ring_is_hole_flag(ring_itr) {
                    // Make the hole the origin!
                    let op1 = op_origin_1;
                    op_origin_1 = i_ring.1.op1;
                    i_ring.1.op1 = op1;
                    let op2 = op_origin_2;
                    op_origin_2 = i_ring.1.op2;
                    i_ring.1.op2 = op2;
                    i_ring.0 = ring_origin;
                    ring_origin = ring_itr;
                    ring_parent = ring_origin;
                    break;
                }
            }
        }
        let origin_is_hole = self.ring_is_hole_flag(ring_origin);

        // Switch
        let op_origin_1_next = self.next(op_origin_1);
        let op_origin_2_next = self.next(op_origin_2);
        self.points[op_origin_1].next = op_origin_2_next;
        self.points[op_origin_2].next = op_origin_1_next;
        self.points[op_origin_1_next].prev = op_origin_2;
        self.points[op_origin_2_next].prev = op_origin_1;

        for i_ring in i_list.iter() {
            let op_search_1 = i_ring.1.op1;
            let op_search_2 = i_ring.1.op2;
            let op_search_1_next = self.next(op_search_1);
            let op_search_2_next = self.next(op_search_2);
            self.points[op_search_1].next = op_search_2_next;
            self.points[op_search_2].next = op_search_1_next;
            self.points[op_search_1_next].prev = op_search_2;
            self.points[op_search_2_next].prev = op_search_1;
        }

        let ring_new = self.create_new_ring();
        self.rings[ring_origin].corrected = false;
        let mut size_1 = 0;
        let mut size_2 = 0;
        let mut box1 = BoxT::default();
        let mut box2 = BoxT::default();
        let area_1 = self.area_from_point(op_origin_1, &mut size_1, &mut box1);
        let area_2 = self.area_from_point(op_origin_2, &mut size_2, &mut box2);
        if origin_is_hole && (area_1 < 0.0) {
            self.rings[ring_origin].points = op_origin_1;
            self.set_stats(ring_origin, area_1, size_1, box1);
            self.rings[ring_new].points = op_origin_2;
            self.set_stats(ring_new, area_2, size_2, box2);
        } else {
            self.rings[ring_origin].points = op_origin_2;
            self.set_stats(ring_origin, area_2, size_2, box2);
            self.rings[ring_new].points = op_origin_1;
            self.set_stats(ring_new, area_1, size_1, box1);
        }

        self.update_points_ring(ring_origin);
        self.update_points_ring(ring_new);

        self.rings[ring_origin].bottom_point = NIL;

        for i_ring in i_list.iter() {
            let ring_itr = i_ring.0;
            self.rings[ring_itr].bottom_point = NIL;
            if origin_is_hole {
                self.ring1_replaces_ring2(ring_origin, ring_itr);
            } else {
                let p = self.rings[ring_origin].parent;
                self.ring1_replaces_ring2(p, ring_itr);
            }
        }
        if origin_is_hole {
            self.assign_as_child(ring_new, ring_origin);
            // The parent ring in this situation might need to change.
            let n = self.children_len(ring_parent);
            for i in 0..n {
                let c = self.child_at(ring_parent, i);
                if c == NIL {
                    continue;
                }
                if self.poly2_contains_poly1(c, ring_new) {
                    self.reassign_as_child(c, ring_new);
                }
            }
        } else {
            // The new ring and the origin ring need to be siblings
            // however some children ring from the ring origin might
            // need to be re-assigned to the new ring
            self.assign_as_sibling(ring_new, ring_origin);
            let n = self.rings[ring_origin].children.len();
            for i in 0..n {
                let c = self.rings[ring_origin].children.get(i).copied().unwrap_or(NIL);
                if c == NIL {
                    continue;
                }
                if self.poly2_contains_poly1(c, ring_new) {
                    self.reassign_as_child(c, ring_new);
                }
            }
        }

        let mut move_list: Vec<(usize, PointPtrPair)> = Vec::new();

        for i_ring in i_list.iter() {
            let key = i_ring.0;
            let n = connection_map.len_of(key);
            if n != 0 {
                for i in 0..n {
                    let it = connection_map.get(key, i);
                    let it_ring = self.points[it.op1].ring;
                    let it_ring2 = self.points[it.op2].ring;
                    if it_ring == NIL || it_ring2 == NIL || it_ring == it_ring2 {
                        continue;
                    }
                    if self.ring_is_hole_flag(it_ring) || self.ring_is_hole_flag(it_ring2) {
                        move_list.push((it_ring, it));
                    }
                }
                connection_map.erase_key(key);
            }
        }

        {
            let mut i = 0usize;
            while i < connection_map.len_of(ring_origin) {
                let it = connection_map.get(ring_origin, i);
                let it_ring = self.points[it.op1].ring;
                let it_ring2 = self.points[it.op2].ring;
                if it_ring == NIL || it_ring2 == NIL || it_ring == it_ring2 {
                    connection_map.erase_at(ring_origin, i);
                    continue;
                }
                if it_ring != ring_origin {
                    if self.ring_is_hole_flag(it_ring) || self.ring_is_hole_flag(it_ring2) {
                        move_list.push((it_ring, it));
                    }
                    connection_map.erase_at(ring_origin, i);
                } else {
                    if self.ring_is_hole_flag(it_ring) || self.ring_is_hole_flag(it_ring2) {
                        i += 1;
                    } else {
                        connection_map.erase_at(ring_origin, i);
                    }
                }
            }
        }

        for (k, v) in move_list {
            connection_map.emplace(k, v);
        }
    }

    fn children_len(&self, parent: usize) -> usize {
        if parent == NIL {
            self.children.len()
        } else {
            self.rings[parent].children.len()
        }
    }

    fn child_at(&self, parent: usize, i: usize) -> usize {
        let v = if parent == NIL { &self.children } else { &self.rings[parent].children };
        v.get(i).copied().unwrap_or(NIL)
    }

    fn correct_chained_rings(&mut self) {
        if self.all_points.len() < 2 {
            return;
        }
        let mut connection_map = ConnectionMap::default();
        let all_points = self.all_points.clone();
        self.for_each_repeat_run(&all_points, |s, run| {
            for i1 in 0..run.len() {
                if s.points[run[i1]].ring == NIL {
                    continue;
                }
                for i2 in i1 + 1..run.len() {
                    if s.points[run[i2]].ring == NIL {
                        continue;
                    }
                    s.process_single_intersection(&mut connection_map, run[i1], run[i2]);
                }
            }
        });
    }

    fn sort_rings_by_area(&mut self, largest_first: bool) -> Vec<usize> {
        let mut sorted_rings: Vec<usize> = (0..self.rings.len()).collect();
        // `area()` lazily computes stats; compute them up front (same values).
        for r in 0..self.rings.len() {
            if self.rings[r].points != NIL {
                self.ring_area(r);
            }
        }
        let rings = &self.rings;
        cppstd::stable_sort_by(&mut sorted_rings, |&r1, &r2| {
            let (a, b) = (&rings[r1], &rings[r2]);
            if a.points == NIL || b.points == NIL {
                return a.points != NIL;
            }
            if largest_first {
                a.area.abs() > b.area.abs()
            } else {
                a.area.abs() < b.area.abs()
            }
        });
        sorted_rings
    }

    fn fix_collinear_path(&mut self, path: &CollinearPath) -> (usize, usize) {
        // The theory behind this is that a collinear path
        // has two 'pathways' that are the same.
        let spike_left = path.start_1 == path.end_2;
        let spike_right = path.start_2 == path.end_1;
        if spike_left && spike_right {
            // If both ends are spikes we should simply delete all the points.
            // (they should be in a loop)
            let mut itr = path.start_1;
            while itr != NIL {
                let pv = self.prev(itr);
                self.points[pv].next = NIL;
                self.points[itr].prev = NIL;
                self.points[itr].ring = NIL;
                itr = self.next(itr);
            }
            (NIL, NIL)
        } else if spike_left {
            let prev = self.prev(path.start_2);
            let mut itr = path.start_2;
            while itr != path.end_1 {
                let pv = self.prev(itr);
                self.points[pv].next = NIL;
                self.points[itr].prev = NIL;
                self.points[itr].ring = NIL;
                itr = self.next(itr);
            }
            self.points[prev].next = path.end_1;
            self.points[path.end_1].prev = prev;
            (path.end_1, NIL)
        } else if spike_right {
            let prev = self.prev(path.start_1);
            let mut itr = path.start_1;
            while itr != path.end_2 {
                let pv = self.prev(itr);
                self.points[pv].next = NIL;
                self.points[itr].prev = NIL;
                self.points[itr].ring = NIL;
                itr = self.next(itr);
            }
            self.points[prev].next = path.end_2;
            self.points[path.end_2].prev = prev;
            (path.end_2, NIL)
        } else {
            let prev_1 = self.prev(path.start_1);
            let prev_2 = self.prev(path.start_2);
            let mut itr = path.start_1;
            loop {
                let pv = self.prev(itr);
                self.points[pv].next = NIL;
                self.points[itr].prev = NIL;
                self.points[itr].ring = NIL;
                itr = self.next(itr);
                if !(itr != path.end_1 && itr != NIL) {
                    break;
                }
            }
            itr = path.start_2;
            loop {
                let pv = self.prev(itr);
                self.points[pv].next = NIL;
                self.points[itr].prev = NIL;
                self.points[itr].ring = NIL;
                itr = self.next(itr);
                if !(itr != path.end_2 && itr != NIL) {
                    break;
                }
            }
            if path.start_1 == path.end_1 && path.start_2 == path.end_2 {
                (NIL, NIL)
            } else if path.start_1 == path.end_1 {
                self.points[prev_2].next = path.end_2;
                self.points[path.end_2].prev = prev_2;
                (path.end_2, NIL)
            } else if path.start_2 == path.end_2 {
                self.points[prev_1].next = path.end_1;
                self.points[path.end_1].prev = prev_1;
                (path.end_1, NIL)
            } else {
                self.points[prev_1].next = path.end_2;
                self.points[path.end_2].prev = prev_1;
                self.points[prev_2].next = path.end_1;
                self.points[path.end_1].prev = prev_2;
                (path.end_1, path.end_2)
            }
        }
    }

    fn find_start_and_end_of_collinear_edges(&self, pt_a: usize, pt_b: usize) -> CollinearPath {
        // Search backward on A, forwards on B first
        let same_ring = self.points[pt_a].ring == self.points[pt_b].ring;
        let mut back = pt_a;
        let mut forward = pt_b;
        let mut first = true;
        loop {
            while self.peq(self.prev(back), back) && back != forward {
                back = self.prev(back);
                if back == pt_a {
                    break;
                }
            }
            if back == forward {
                back = self.prev(back);
                forward = self.next(forward);
                break;
            }
            while self.peq(self.next(forward), forward) && back != forward {
                forward = self.next(forward);
                if forward == pt_b {
                    break;
                }
            }
            if !first && (back == pt_a || forward == pt_b) {
                break;
            }
            if back == forward {
                back = self.prev(back);
                forward = self.next(forward);
                break;
            }
            back = self.prev(back);
            forward = self.next(forward);
            first = false;
            if !self.peq(back, forward) {
                break;
            }
        }
        let mut start_a = self.next(back);
        // If there are repeated points at the start, we want to shift them
        while !same_ring && self.peq(start_a, self.next(start_a)) && start_a != pt_a {
            start_a = self.next(start_a);
        }
        let mut end_b = self.prev(forward);
        // If there are repeated points at the end, we want to shift them
        while !same_ring && self.peq(end_b, self.prev(end_b)) && end_b != pt_b {
            end_b = self.prev(end_b);
        }

        // Search backward on B, forwards on A
        back = pt_b;
        forward = pt_a;
        first = true;
        loop {
            while self.peq(self.prev(back), back) && back != forward {
                back = self.prev(back);
                if back == pt_b {
                    break;
                }
            }
            if back == forward {
                back = self.prev(back);
                forward = self.next(forward);
                break;
            }
            while self.peq(self.next(forward), forward) && back != forward {
                forward = self.next(forward);
                if forward == pt_a {
                    break;
                }
            }
            if !first && (back == pt_b || forward == pt_a) {
                break;
            }
            if back == forward || (!first && (back == end_b || forward == start_a)) {
                back = self.prev(back);
                forward = self.next(forward);
                break;
            }
            back = self.prev(back);
            forward = self.next(forward);
            first = false;
            if !self.peq(back, forward) {
                break;
            }
        }
        let mut start_b = self.next(back);
        // If there are repeated points at the start, we want to shift them
        while !same_ring && self.peq(start_b, self.next(start_b)) && start_b != pt_b {
            start_b = self.next(start_b);
        }
        let mut end_a = self.prev(forward);
        // If there are repeated points at the end, we want to shift them
        while !same_ring && self.peq(end_a, self.prev(end_a)) && end_a != pt_a {
            end_a = self.prev(end_a);
        }
        CollinearPath { start_1: start_a, end_1: end_a, start_2: start_b, end_2: end_b }
    }

    fn has_collinear_edge(&self, pt_a: usize, pt_b: usize) -> bool {
        self.peq(self.next(pt_a), self.prev(pt_b)) || self.peq(self.next(pt_b), self.prev(pt_a))
    }

    fn process_collinear_edges_same_ring(&mut self, pt_a: usize, pt_b: usize) {
        let original_ring = self.points[pt_a].ring;
        // As they are the same ring that are forming a collinear edge
        // we should expect the creation of two different rings.
        let path = self.find_start_and_end_of_collinear_edges(pt_a, pt_b);
        let results = self.fix_collinear_path(&path);
        if results.0 == NIL {
            // If pt1 is a nullptr, both values
            // are nullptrs. This mean the ring was
            // removed during this processing.
            self.remove_ring(original_ring, false, true);
        } else if results.1 == NIL {
            // If a single value is returned, we only have one ring
            // left, so we will set the original ring to that point.
            self.rings[original_ring].points = results.0;
            self.recalculate_stats(original_ring);
        } else {
            // If we have two values, we have two rings, but we need to create
            // a new ring for the second value.
            let ring_new = self.create_new_ring();
            self.rings[ring_new].points = results.1;
            self.recalculate_stats(ring_new);
            self.update_points_ring(ring_new);
            self.rings[original_ring].points = results.0;
            self.recalculate_stats(original_ring);
        }
    }

    fn process_collinear_edges_different_rings(&mut self, pt_a: usize, pt_b: usize) {
        let ring_a = self.points[pt_a].ring;
        let ring_b = self.points[pt_b].ring;
        let ring_a_larger = self.ring_area(ring_a).abs() > self.ring_area(ring_b).abs();
        let path = self.find_start_and_end_of_collinear_edges(pt_a, pt_b);
        // This should result in two rings becoming one.
        let results = self.fix_collinear_path(&path);
        if results.0 == NIL {
            self.remove_ring(ring_a, false, true);
            self.remove_ring(ring_b, false, true);
            return;
        }
        // If we didn't return a nullptr we have to update the ring
        let merged_ring = if ring_a_larger { ring_a } else { ring_b };
        let deleted_ring = if ring_a_larger { ring_b } else { ring_a };

        self.rings[merged_ring].points = results.0;
        self.update_points_ring(merged_ring);
        self.recalculate_stats(merged_ring);
        if self.ring_size(merged_ring) < 3 {
            self.remove_ring_and_points(merged_ring, false, true);
        }
        self.remove_ring(deleted_ring, false, true);
    }

    fn remove_duplicate_points(&mut self, pt_a: usize, pt_b: usize) -> bool {
        if self.points[pt_a].ring == self.points[pt_b].ring {
            if self.next(pt_a) == pt_b {
                let nb = self.next(pt_b);
                self.points[pt_a].next = nb;
                self.points[nb].prev = pt_a;
                self.points[pt_b].next = NIL;
                self.points[pt_b].prev = NIL;
                self.points[pt_b].ring = NIL;
                let r = self.points[pt_a].ring;
                if self.rings[r].points == pt_b {
                    self.rings[r].points = pt_a;
                }
                return true;
            } else if self.next(pt_b) == pt_a {
                let pb = self.prev(pt_b);
                self.points[pt_a].prev = pb;
                self.points[pb].next = pt_a;
                self.points[pt_b].next = NIL;
                self.points[pt_b].prev = NIL;
                self.points[pt_b].ring = NIL;
                let r = self.points[pt_a].ring;
                if self.rings[r].points == pt_b {
                    self.rings[r].points = pt_a;
                }
                return true;
            }
        }
        while self.peq(self.next(pt_a), pt_a) && self.next(pt_a) != pt_a {
            let remove = self.next(pt_a);
            let rn = self.next(remove);
            self.points[pt_a].next = rn;
            self.points[rn].prev = pt_a;
            self.points[remove].next = NIL;
            self.points[remove].prev = NIL;
            self.points[remove].ring = NIL;
            let r = self.points[pt_a].ring;
            if self.rings[r].points == remove {
                self.rings[r].points = pt_a;
            }
        }
        while self.peq(self.prev(pt_a), pt_a) && self.prev(pt_a) != pt_a {
            let remove = self.prev(pt_a);
            let rp = self.prev(remove);
            self.points[pt_a].prev = rp;
            self.points[rp].next = pt_a;
            self.points[remove].next = NIL;
            self.points[remove].prev = NIL;
            self.points[remove].ring = NIL;
            let r = self.points[pt_a].ring;
            if self.rings[r].points == remove {
                self.rings[r].points = pt_a;
            }
        }
        if self.next(pt_a) == pt_a {
            let r = self.points[pt_a].ring;
            self.remove_ring_and_points(r, false, true);
            return true;
        }
        if self.points[pt_b].ring == NIL {
            return true;
        }
        while self.peq(self.next(pt_b), pt_b) && self.next(pt_b) != pt_b {
            let remove = self.next(pt_b);
            let rn = self.next(remove);
            self.points[pt_b].next = rn;
            self.points[rn].prev = pt_b;
            self.points[remove].next = NIL;
            self.points[remove].prev = NIL;
            self.points[remove].ring = NIL;
            let r = self.points[pt_b].ring;
            if self.rings[r].points == remove {
                self.rings[r].points = pt_b;
            }
        }
        while self.peq(self.prev(pt_b), pt_b) && self.prev(pt_b) != pt_b {
            let remove = self.prev(pt_b);
            let rp = self.prev(remove);
            self.points[pt_b].prev = rp;
            self.points[rp].next = pt_b;
            self.points[remove].next = NIL;
            self.points[remove].prev = NIL;
            self.points[remove].ring = NIL;
            let r = self.points[pt_b].ring;
            if self.rings[r].points == remove {
                self.rings[r].points = pt_b;
            }
        }
        if self.next(pt_b) == pt_b {
            let r = self.points[pt_b].ring;
            self.remove_ring_and_points(r, false, true);
            return true;
        }
        if self.points[pt_a].ring == NIL {
            return true;
        }
        false
    }

    fn process_collinear_edges(&mut self, pt_a: usize, pt_b: usize) -> bool {
        // Neither point assigned to a ring (deleted points)
        if self.points[pt_a].ring == NIL || self.points[pt_b].ring == NIL {
            return false;
        }

        if self.remove_duplicate_points(pt_a, pt_b) {
            return true;
        }

        if !self.has_collinear_edge(pt_a, pt_b) {
            if self.points[pt_a].ring == self.points[pt_b].ring {
                self.correct_self_intersection(pt_a, pt_b);
                return true;
            }
            return false;
        }

        if self.points[pt_a].ring == self.points[pt_b].ring {
            self.process_collinear_edges_same_ring(pt_a, pt_b);
        } else {
            self.process_collinear_edges_different_rings(pt_a, pt_b);
        }
        true
    }

    fn correct_collinear_repeats(&mut self, run: &[usize]) {
        for i1 in 0..run.len() {
            if self.points[run[i1]].ring == NIL {
                continue;
            }
            let mut i2 = 0usize;
            while i2 != run.len() {
                if self.points[run[i1]].ring == NIL {
                    break;
                }
                if self.points[run[i2]].ring == NIL || run[i2] == run[i1] {
                    i2 += 1;
                    continue;
                }
                if self.process_collinear_edges(run[i1], run[i2]) {
                    i2 = 0;
                } else {
                    i2 += 1;
                }
            }
        }
    }

    fn correct_collinear_edges(&mut self) {
        if self.all_points.len() < 2 {
            return;
        }
        let all_points = self.all_points.clone();
        self.for_each_repeat_run(&all_points, |s, run| s.correct_collinear_repeats(run));
    }

    fn correct_tree(&mut self) {
        let sorted_rings = self.sort_rings_by_area(true);
        for itr in 0..sorted_rings.len() {
            let r = sorted_rings[itr];
            if self.rings[r].points == NIL {
                continue;
            }
            if self.ring_size(r) < 3 || value_is_zero(self.ring_area(r)) {
                self.remove_ring_and_points(r, false, true);
                continue;
            }
            self.rings[r].corrected = true;
            let mut found = false;
            // Search in reverse from the current iterator back to the begining
            // to see if any of those rings might be its parent.
            let mut r_itr = itr;
            while r_itr > 0 {
                r_itr -= 1;
                let s = sorted_rings[r_itr];
                // If orientations are not different, this can't be its parent.
                if self.ring_is_hole_flag(s) == self.ring_is_hole_flag(r) {
                    continue;
                }
                if self.poly2_contains_poly1(r, s) {
                    self.reassign_as_child(r, s);
                    found = true;
                    break;
                }
            }
            if !found {
                if self.ring_is_hole_flag(r) {
                    panic!("Could not properly place hole to a parent.");
                } else {
                    // Assign to base of tree by passing nullptr
                    self.reassign_as_child(r, NIL);
                }
            }
        }
    }

    fn correct_self_intersections(&mut self, correct_tree: bool) -> bool {
        let mut fixed_intersections = false;
        let sorted_rings = self.sort_rings_by_area(false);
        for r in sorted_rings {
            if self.correct_ring_self_intersections(r, correct_tree) {
                fixed_intersections = true;
            }
        }
        fixed_intersections
    }

    fn correct_topology(&mut self) {
        // Sort all the points, this will be used for the locating of chained rings
        // and the collinear edges and only needs to be done once.
        {
            let mut all_points = std::mem::take(&mut self.all_points);
            cppstd::stable_sort_by(&mut all_points, |&a, &b| self.point_ptr_less(a, b));
            self.all_points = all_points;
        }

        // Initially the orientations of the rings
        // could be incorrect, we need to adjust them
        self.correct_orientations();

        // We should only have to fix collinear edges once.
        // During this we also correct self intersections
        self.correct_collinear_edges();

        self.correct_self_intersections(false);

        self.correct_tree();

        let mut fixed_intersections = true;
        while fixed_intersections {
            self.correct_chained_rings();
            fixed_intersections = self.correct_self_intersections(true);
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct CollinearPath {
    start_1: usize,
    end_1: usize,
    start_2: usize,
    end_2: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x0: T, y0: T, s: T) -> LinearRing {
        vec![P::new(x0, y0), P::new(x0 + s, y0), P::new(x0 + s, y0 + s), P::new(x0, y0 + s), P::new(x0, y0)]
    }

    #[test]
    fn union_of_two_squares() {
        let mut w = Wagyu::new();
        w.add_ring(&square(0, 0, 10), PolygonType::Subject);
        w.add_ring(&square(5, 5, 10), PolygonType::Subject);
        let mut sol = MultiPolygon::new();
        assert!(w.execute(ClipType::Union, &mut sol, FillType::EvenOdd, FillType::EvenOdd));
        // Even-odd: the overlap is not filled, leaving two L shapes touching at corners.
        assert_eq!(sol.len(), 2);
    }
}
