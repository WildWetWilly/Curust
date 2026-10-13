//! Rust port of Clipper 6.4.2 (Angus Johnson, <http://www.angusj.com>).
//!
//! Original copyright: Angus Johnson 2010-2017.
//! License: Boost Software License 1.0 (<http://www.boost.org/LICENSE_1_0.txt>).
//!
//! This is a faithful, line-by-line translation of the C++ `ClipperLib` as used by
//! CuraEngine (compiled with `use_lines`, without `use_int32`/`use_xyz`). Raw pointers of the
//! original are replaced by indices into arenas; the algorithmic behaviour, including the
//! order of the output polygons and their starting vertices, is intended to be identical.
//!
//! The code in this library is an extension of Bala Vatti's clipping algorithm:
//! "A generic solution to polygon clipping", Communications of the ACM, Vol 35, Issue 7
//! (July 1992) pp 56-63.

#![allow(clippy::too_many_arguments)]
#![allow(clippy::collapsible_else_if)]
#![allow(clippy::collapsible_if)]

use std::collections::BinaryHeap;
use std::ops::{Add, AddAssign, Div, Mul, Neg, Rem, Sub, SubAssign};

pub type CInt = i64;

pub const LO_RANGE: CInt = 0x3FFF_FFFF;
pub const HI_RANGE: CInt = 0x3FFF_FFFF_FFFF_FFFF;

/// A 2D integer point (`ClipperLib::IntPoint`). CuraEngine's `Point2LL` is this type.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IntPoint {
    pub x: CInt,
    pub y: CInt,
}

impl IntPoint {
    #[inline]
    pub const fn new(x: CInt, y: CInt) -> Self {
        IntPoint { x, y }
    }
}

impl Add for IntPoint {
    type Output = IntPoint;
    #[inline]
    fn add(self, o: IntPoint) -> IntPoint {
        IntPoint::new(self.x + o.x, self.y + o.y)
    }
}
impl Sub for IntPoint {
    type Output = IntPoint;
    #[inline]
    fn sub(self, o: IntPoint) -> IntPoint {
        IntPoint::new(self.x - o.x, self.y - o.y)
    }
}
impl Neg for IntPoint {
    type Output = IntPoint;
    #[inline]
    fn neg(self) -> IntPoint {
        IntPoint::new(-self.x, -self.y)
    }
}
impl Mul<CInt> for IntPoint {
    type Output = IntPoint;
    #[inline]
    fn mul(self, i: CInt) -> IntPoint {
        IntPoint::new(self.x * i, self.y * i)
    }
}
impl Div<CInt> for IntPoint {
    type Output = IntPoint;
    #[inline]
    fn div(self, i: CInt) -> IntPoint {
        IntPoint::new(self.x / i, self.y / i)
    }
}
impl Rem<CInt> for IntPoint {
    type Output = IntPoint;
    #[inline]
    fn rem(self, i: CInt) -> IntPoint {
        IntPoint::new(self.x % i, self.y % i)
    }
}
impl AddAssign for IntPoint {
    #[inline]
    fn add_assign(&mut self, o: IntPoint) {
        self.x += o.x;
        self.y += o.y;
    }
}
impl SubAssign for IntPoint {
    #[inline]
    fn sub_assign(&mut self, o: IntPoint) {
        self.x -= o.x;
        self.y -= o.y;
    }
}

pub type Path = Vec<IntPoint>;
pub type Paths = Vec<Path>;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DoublePoint {
    pub x: f64,
    pub y: f64,
}

impl DoublePoint {
    #[inline]
    pub fn new(x: f64, y: f64) -> Self {
        DoublePoint { x, y }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipType {
    Intersection,
    Union,
    Difference,
    Xor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolyType {
    Subject,
    Clip,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolyFillType {
    EvenOdd,
    NonZero,
    Positive,
    Negative,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinType {
    Square,
    Round,
    Miter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndType {
    ClosedPolygon,
    ClosedLine,
    OpenButt,
    OpenSquare,
    OpenRound,
}

pub const IO_REVERSE_SOLUTION: i32 = 1;
pub const IO_STRICTLY_SIMPLE: i32 = 2;
pub const IO_PRESERVE_COLLINEAR: i32 = 4;

#[derive(Clone, Copy, Debug, Default)]
pub struct IntRect {
    pub left: CInt,
    pub top: CInt,
    pub right: CInt,
    pub bottom: CInt,
}

#[derive(Debug, Clone)]
pub struct ClipperError(pub &'static str);

// ------------------------------------------------------------------------------------------
// PolyTree
// ------------------------------------------------------------------------------------------

/// A node of a [`PolyTree`]. Children are indices into [`PolyTree::all_nodes`].
#[derive(Clone, Debug, Default)]
pub struct PolyNode {
    pub contour: Path,
    pub childs: Vec<usize>,
    /// `None` means the parent is the tree root.
    pub parent: Option<usize>,
    index: usize,
    is_open: bool,
    join_type: Option<JoinType>,
    end_type: Option<EndType>,
}

impl PolyNode {
    pub fn child_count(&self) -> usize {
        self.childs.len()
    }
    pub fn is_open(&self) -> bool {
        self.is_open
    }
}

/// `ClipperLib::PolyTree`. The tree itself acts as the root node: its top-level children are
/// in [`PolyTree::childs`].
#[derive(Clone, Debug, Default)]
pub struct PolyTree {
    pub all_nodes: Vec<PolyNode>,
    pub childs: Vec<usize>,
}

impl PolyTree {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn clear(&mut self) {
        self.all_nodes.clear();
        self.childs.clear();
    }
    pub fn child_count(&self) -> usize {
        self.childs.len()
    }
    pub fn node(&self, idx: usize) -> &PolyNode {
        &self.all_nodes[idx]
    }
    pub fn node_mut(&mut self, idx: usize) -> &mut PolyNode {
        &mut self.all_nodes[idx]
    }
    pub fn total(&self) -> usize {
        let mut result = self.all_nodes.len();
        // with negative offsets, ignore the hidden outer polygon ...
        if result > 0 && (self.childs.is_empty() || self.childs[0] != 0) {
            result -= 1;
        }
        result
    }
    pub fn get_first(&self) -> Option<usize> {
        self.childs.first().copied()
    }
    pub fn is_hole(&self, idx: usize) -> bool {
        // C++: result = true; for each ancestor (including the tree root) result = !result.
        let mut result = true;
        let mut node = self.all_nodes[idx].parent;
        loop {
            result = !result;
            match node {
                Some(p) => node = self.all_nodes[p].parent,
                None => break,
            }
        }
        result
    }
    fn add_child_to_root(&mut self, child: usize) {
        let cnt = self.childs.len();
        self.childs.push(child);
        self.all_nodes[child].parent = None;
        self.all_nodes[child].index = cnt;
    }
    fn add_child(&mut self, parent: usize, child: usize) {
        let cnt = self.all_nodes[parent].childs.len();
        self.all_nodes[parent].childs.push(child);
        self.all_nodes[child].parent = Some(parent);
        self.all_nodes[child].index = cnt;
    }
    /// Depth-first traversal successor (`PolyNode::GetNext`). `None` input means the root.
    pub fn get_next(&self, node: Option<usize>) -> Option<usize> {
        let childs = match node {
            None => &self.childs,
            Some(n) => &self.all_nodes[n].childs,
        };
        if !childs.is_empty() {
            Some(childs[0])
        } else {
            match node {
                None => None,
                Some(n) => self.get_next_sibling_up(n),
            }
        }
    }
    fn get_next_sibling_up(&self, n: usize) -> Option<usize> {
        let node = &self.all_nodes[n];
        let siblings = match node.parent {
            None => &self.childs,
            Some(p) => &self.all_nodes[p].childs,
        };
        if node.index == siblings.len() - 1 {
            match node.parent {
                None => None,
                Some(p) => self.get_next_sibling_up(p),
            }
        } else {
            Some(siblings[node.index + 1])
        }
    }
}

// ------------------------------------------------------------------------------------------
// Internal structures
// ------------------------------------------------------------------------------------------

const NIL: usize = usize::MAX;
const UNASSIGNED: i32 = -1; // edge not currently 'owning' a solution
const SKIP: i32 = -2; // edge that would otherwise close a path
const HORIZONTAL: f64 = -1.0E+40;
const TOLERANCE: f64 = 1.0e-20;

#[inline]
fn near_zero(val: f64) -> bool {
    val > -TOLERANCE && val < TOLERANCE
}

const ES_LEFT: u8 = 1;
const ES_RIGHT: u8 = 2;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    RightToLeft,
    LeftToRight,
}

#[derive(Clone, Copy, Debug)]
struct TEdge {
    bot: IntPoint,
    curr: IntPoint, // current (updated for every new scanbeam)
    top: IntPoint,
    dx: f64,
    poly_typ: PolyType,
    side: u8, // side only refers to current side of solution poly
    wind_delta: i32, // 1 or -1 depending on winding direction
    wind_cnt: i32,
    wind_cnt2: i32, // winding count of the opposite polytype
    out_idx: i32,
    next: usize,
    prev: usize,
    next_in_lml: usize,
    next_in_ael: usize,
    prev_in_ael: usize,
    next_in_sel: usize,
    prev_in_sel: usize,
}

impl Default for TEdge {
    fn default() -> Self {
        // Equivalent of memset(e, 0, sizeof(TEdge)) followed by the pointer fields being NULL.
        TEdge {
            bot: IntPoint::default(),
            curr: IntPoint::default(),
            top: IntPoint::default(),
            dx: 0.0,
            poly_typ: PolyType::Subject,
            side: 0,
            wind_delta: 0,
            wind_cnt: 0,
            wind_cnt2: 0,
            out_idx: 0,
            next: NIL,
            prev: NIL,
            next_in_lml: NIL,
            next_in_ael: NIL,
            prev_in_ael: NIL,
            next_in_sel: NIL,
            prev_in_sel: NIL,
        }
    }
}

#[derive(Clone, Copy)]
struct IntersectNode {
    edge1: usize,
    edge2: usize,
    pt: IntPoint,
}

#[derive(Clone, Copy)]
struct LocalMinimum {
    y: CInt,
    left_bound: usize,
    right_bound: usize,
}

#[derive(Clone, Copy)]
struct OutRec {
    idx: i32,
    is_hole: bool,
    is_open: bool,
    first_left: usize, // index into poly_outs, NIL if none
    poly_nd: usize,    // index into PolyTree::all_nodes, NIL if none
    pts: usize,        // index into out_pts, NIL if none
    bottom_pt: usize,
}

#[derive(Clone, Copy)]
struct OutPt {
    idx: i32,
    pt: IntPoint,
    next: usize,
    prev: usize,
}

#[derive(Clone, Copy)]
struct Join {
    out_pt1: usize,
    out_pt2: usize,
    off_pt: IntPoint,
}

// ------------------------------------------------------------------------------------------
// Miscellaneous helpers
// ------------------------------------------------------------------------------------------

#[inline]
fn round(val: f64) -> CInt {
    if val < 0.0 {
        (val - 0.5) as CInt
    } else {
        (val + 0.5) as CInt
    }
}

#[inline]
fn abs(val: CInt) -> CInt {
    if val < 0 {
        -val
    } else {
        val
    }
}

/// `ClipperLib::Orientation`.
pub fn orientation(poly: &[IntPoint]) -> bool {
    area(poly) >= 0.0
}

/// `ClipperLib::Area`.
pub fn area(poly: &[IntPoint]) -> f64 {
    let size = poly.len();
    if size < 3 {
        return 0.0;
    }
    let mut a = 0.0f64;
    let mut j = size - 1;
    for i in 0..size {
        a += (poly[j].x as f64 + poly[i].x as f64) * (poly[j].y as f64 - poly[i].y as f64);
        j = i;
    }
    -a * 0.5
}

/// `ClipperLib::PointInPolygon`: returns 0 if false, +1 if true, -1 if pt ON polygon boundary.
pub fn point_in_polygon(pt: IntPoint, path: &[IntPoint]) -> i32 {
    let mut result = 0;
    let cnt = path.len();
    if cnt < 3 {
        return 0;
    }
    let mut ip = path[0];
    for i in 1..=cnt {
        let ip_next = if i == cnt { path[0] } else { path[i] };
        if ip_next.y == pt.y {
            if ip_next.x == pt.x || (ip.y == pt.y && ((ip_next.x > pt.x) == (ip.x < pt.x))) {
                return -1;
            }
        }
        if (ip.y < pt.y) != (ip_next.y < pt.y) {
            if ip.x >= pt.x {
                if ip_next.x > pt.x {
                    result = 1 - result;
                } else {
                    let d = (ip.x - pt.x) as f64 * (ip_next.y - pt.y) as f64
                        - (ip_next.x - pt.x) as f64 * (ip.y - pt.y) as f64;
                    if d == 0.0 {
                        return -1;
                    }
                    if (d > 0.0) == (ip_next.y > ip.y) {
                        result = 1 - result;
                    }
                }
            } else {
                if ip_next.x > pt.x {
                    let d = (ip.x - pt.x) as f64 * (ip_next.y - pt.y) as f64
                        - (ip_next.x - pt.x) as f64 * (ip.y - pt.y) as f64;
                    if d == 0.0 {
                        return -1;
                    }
                    if (d > 0.0) == (ip_next.y > ip.y) {
                        result = 1 - result;
                    }
                }
            }
        }
        ip = ip_next;
    }
    result
}

#[inline]
fn slopes_equal3(pt1: IntPoint, pt2: IntPoint, pt3: IntPoint) -> bool {
    (pt1.y - pt2.y) as i128 * (pt2.x - pt3.x) as i128 == (pt1.x - pt2.x) as i128 * (pt2.y - pt3.y) as i128
}

#[inline]
fn slopes_equal4(pt1: IntPoint, pt2: IntPoint, pt3: IntPoint, pt4: IntPoint) -> bool {
    (pt1.y - pt2.y) as i128 * (pt3.x - pt4.x) as i128 == (pt1.x - pt2.x) as i128 * (pt3.y - pt4.y) as i128
}

#[inline]
fn slopes_equal_edges(e1: &TEdge, e2: &TEdge) -> bool {
    (e1.top.y - e1.bot.y) as i128 * (e2.top.x - e2.bot.x) as i128
        == (e1.top.x - e1.bot.x) as i128 * (e2.top.y - e2.bot.y) as i128
}

#[inline]
fn is_horizontal(e: &TEdge) -> bool {
    e.dx == HORIZONTAL
}

#[inline]
fn get_dx(pt1: IntPoint, pt2: IntPoint) -> f64 {
    if pt1.y == pt2.y {
        HORIZONTAL
    } else {
        (pt2.x - pt1.x) as f64 / (pt2.y - pt1.y) as f64
    }
}

#[inline]
fn set_dx(e: &mut TEdge) {
    let dy = e.top.y - e.bot.y;
    if dy == 0 {
        e.dx = HORIZONTAL;
    } else {
        e.dx = (e.top.x - e.bot.x) as f64 / dy as f64;
    }
}

#[inline]
fn top_x(edge: &TEdge, current_y: CInt) -> CInt {
    if current_y == edge.top.y {
        edge.top.x
    } else {
        edge.bot.x + round(edge.dx * (current_y - edge.bot.y) as f64)
    }
}

fn intersect_point(edge1: &TEdge, edge2: &TEdge) -> IntPoint {
    let mut ip = IntPoint::default();
    let b1: f64;
    let b2: f64;
    if edge1.dx == edge2.dx {
        ip.y = edge1.curr.y;
        ip.x = top_x(edge1, ip.y);
        return ip;
    } else if edge1.dx == 0.0 {
        ip.x = edge1.bot.x;
        if is_horizontal(edge2) {
            ip.y = edge2.bot.y;
        } else {
            b2 = edge2.bot.y as f64 - (edge2.bot.x as f64 / edge2.dx);
            ip.y = round(ip.x as f64 / edge2.dx + b2);
        }
    } else if edge2.dx == 0.0 {
        ip.x = edge2.bot.x;
        if is_horizontal(edge1) {
            ip.y = edge1.bot.y;
        } else {
            b1 = edge1.bot.y as f64 - (edge1.bot.x as f64 / edge1.dx);
            ip.y = round(ip.x as f64 / edge1.dx + b1);
        }
    } else {
        b1 = edge1.bot.x as f64 - edge1.bot.y as f64 * edge1.dx;
        b2 = edge2.bot.x as f64 - edge2.bot.y as f64 * edge2.dx;
        let q = (b2 - b1) / (edge1.dx - edge2.dx);
        ip.y = round(q);
        if edge1.dx.abs() < edge2.dx.abs() {
            ip.x = round(edge1.dx * q + b1);
        } else {
            ip.x = round(edge2.dx * q + b2);
        }
    }

    if ip.y < edge1.top.y || ip.y < edge2.top.y {
        if edge1.top.y > edge2.top.y {
            ip.y = edge1.top.y;
        } else {
            ip.y = edge2.top.y;
        }
        if edge1.dx.abs() < edge2.dx.abs() {
            ip.x = top_x(edge1, ip.y);
        } else {
            ip.x = top_x(edge2, ip.y);
        }
    }
    // finally, don't allow 'ip' to be BELOW curr.Y (ie bottom of scanbeam) ...
    if ip.y > edge1.curr.y {
        ip.y = edge1.curr.y;
        // use the more vertical edge to derive X ...
        if edge1.dx.abs() > edge2.dx.abs() {
            ip.x = top_x(edge2, ip.y);
        } else {
            ip.x = top_x(edge1, ip.y);
        }
    }
    ip
}

#[inline]
fn reverse_horizontal(e: &mut TEdge) {
    // swap horizontal edges' Top and Bottom x's so they follow the natural
    // progression of the bounds - ie so their xbots will align with the
    // adjoining lower edge. [Helpful in the ProcessHorizontal() method.]
    std::mem::swap(&mut e.top.x, &mut e.bot.x);
}

#[allow(dead_code)]
fn get_overlap_segment(
    mut pt1a: IntPoint,
    mut pt1b: IntPoint,
    mut pt2a: IntPoint,
    mut pt2b: IntPoint,
) -> Option<(IntPoint, IntPoint)> {
    // precondition: segments are Collinear.
    if abs(pt1a.x - pt1b.x) > abs(pt1a.y - pt1b.y) {
        if pt1a.x > pt1b.x {
            std::mem::swap(&mut pt1a, &mut pt1b);
        }
        if pt2a.x > pt2b.x {
            std::mem::swap(&mut pt2a, &mut pt2b);
        }
        let pt1 = if pt1a.x > pt2a.x { pt1a } else { pt2a };
        let pt2 = if pt1b.x < pt2b.x { pt1b } else { pt2b };
        if pt1.x < pt2.x {
            Some((pt1, pt2))
        } else {
            None
        }
    } else {
        if pt1a.y < pt1b.y {
            std::mem::swap(&mut pt1a, &mut pt1b);
        }
        if pt2a.y < pt2b.y {
            std::mem::swap(&mut pt2a, &mut pt2b);
        }
        let pt1 = if pt1a.y < pt2a.y { pt1a } else { pt2a };
        let pt2 = if pt1b.y > pt2b.y { pt1b } else { pt2b };
        if pt1.y > pt2.y {
            Some((pt1, pt2))
        } else {
            None
        }
    }
}


#[inline]
fn pt2_is_between_pt1_and_pt3(pt1: IntPoint, pt2: IntPoint, pt3: IntPoint) -> bool {
    if pt1 == pt3 || pt1 == pt2 || pt3 == pt2 {
        false
    } else if pt1.x != pt3.x {
        (pt2.x > pt1.x) == (pt2.x < pt3.x)
    } else {
        (pt2.y > pt1.y) == (pt2.y < pt3.y)
    }
}

#[inline]
fn horz_segments_overlap(mut seg1a: CInt, mut seg1b: CInt, mut seg2a: CInt, mut seg2b: CInt) -> bool {
    if seg1a > seg1b {
        std::mem::swap(&mut seg1a, &mut seg1b);
    }
    if seg2a > seg2b {
        std::mem::swap(&mut seg2a, &mut seg2b);
    }
    (seg1a < seg2b) && (seg2a < seg1b)
}

fn range_test(pt: IntPoint, use_full_range: &mut bool) -> Result<(), ClipperError> {
    if *use_full_range {
        if pt.x > HI_RANGE || pt.y > HI_RANGE || -pt.x > HI_RANGE || -pt.y > HI_RANGE {
            return Err(ClipperError("Coordinate outside allowed range"));
        }
    } else if pt.x > LO_RANGE || pt.y > LO_RANGE || -pt.x > LO_RANGE || -pt.y > LO_RANGE {
        *use_full_range = true;
        return range_test(pt, use_full_range);
    }
    Ok(())
}

#[inline]
fn get_overlap(a1: CInt, a2: CInt, b1: CInt, b2: CInt) -> (bool, CInt, CInt) {
    let (left, right);
    if a1 < a2 {
        if b1 < b2 {
            left = a1.max(b1);
            right = a2.min(b2);
        } else {
            left = a1.max(b2);
            right = a2.min(b1);
        }
    } else {
        if b1 < b2 {
            left = a2.max(b1);
            right = a1.min(b2);
        } else {
            left = a2.max(b2);
            right = a1.min(b1);
        }
    }
    (left < right, left, right)
}

// ------------------------------------------------------------------------------------------
// Clipper
// ------------------------------------------------------------------------------------------

/// `ClipperLib::Clipper` (including the `ClipperBase` functionality).
pub struct Clipper {
    // ClipperBase
    edges: Vec<TEdge>,
    minima_list: Vec<LocalMinimum>,
    current_lm: usize,
    use_full_range: bool,
    preserve_collinear: bool,
    has_open_paths: bool,
    poly_outs: Vec<OutRec>,
    out_pts: Vec<OutPt>,
    active_edges: usize,
    scanbeam: BinaryHeap<CInt>,
    // Clipper
    joins: Vec<Join>,
    ghost_joins: Vec<Join>,
    intersect_list: Vec<IntersectNode>,
    clip_type: ClipType,
    maxima: Vec<CInt>,
    sorted_edges: usize,
    execute_locked: bool,
    clip_fill_type: PolyFillType,
    subj_fill_type: PolyFillType,
    reverse_output: bool,
    using_poly_tree: bool,
    strict_simple: bool,
}

impl Default for Clipper {
    fn default() -> Self {
        Clipper::new(0)
    }
}

impl Clipper {
    pub fn new(init_options: i32) -> Self {
        Clipper {
            edges: Vec::new(),
            minima_list: Vec::new(),
            current_lm: 0,
            use_full_range: false,
            preserve_collinear: (init_options & IO_PRESERVE_COLLINEAR) != 0,
            has_open_paths: false,
            poly_outs: Vec::new(),
            out_pts: Vec::new(),
            active_edges: NIL,
            scanbeam: BinaryHeap::new(),
            joins: Vec::new(),
            ghost_joins: Vec::new(),
            intersect_list: Vec::new(),
            clip_type: ClipType::Intersection,
            maxima: Vec::new(),
            sorted_edges: NIL,
            execute_locked: false,
            clip_fill_type: PolyFillType::EvenOdd,
            subj_fill_type: PolyFillType::EvenOdd,
            reverse_output: (init_options & IO_REVERSE_SOLUTION) != 0,
            using_poly_tree: false,
            strict_simple: (init_options & IO_STRICTLY_SIMPLE) != 0,
        }
    }

    pub fn preserve_collinear(&self) -> bool {
        self.preserve_collinear
    }
    pub fn set_preserve_collinear(&mut self, v: bool) {
        self.preserve_collinear = v;
    }
    pub fn reverse_solution(&self) -> bool {
        self.reverse_output
    }
    pub fn set_reverse_solution(&mut self, v: bool) {
        self.reverse_output = v;
    }
    pub fn strictly_simple(&self) -> bool {
        self.strict_simple
    }
    pub fn set_strictly_simple(&mut self, v: bool) {
        self.strict_simple = v;
    }

    // --------------------------------------------------------------------------------------
    // ClipperBase
    // --------------------------------------------------------------------------------------

    #[inline]
    fn e(&self, i: usize) -> &TEdge {
        &self.edges[i]
    }
    #[inline]
    fn em(&mut self, i: usize) -> &mut TEdge {
        &mut self.edges[i]
    }

    fn init_edge(&mut self, e: usize, e_next: usize, e_prev: usize, pt: IntPoint) {
        let edge = &mut self.edges[e];
        *edge = TEdge::default();
        edge.next = e_next;
        edge.prev = e_prev;
        edge.curr = pt;
        edge.out_idx = UNASSIGNED;
    }

    fn init_edge2(&mut self, e: usize, pt: PolyType) {
        let next_curr = self.edges[self.edges[e].next].curr;
        let edge = &mut self.edges[e];
        if edge.curr.y >= next_curr.y {
            edge.bot = edge.curr;
            edge.top = next_curr;
        } else {
            edge.top = edge.curr;
            edge.bot = next_curr;
        }
        set_dx(edge);
        edge.poly_typ = pt;
    }

    fn remove_edge(&mut self, e: usize) -> usize {
        // removes e from double_linked_list (but without removing from memory)
        let prev = self.edges[e].prev;
        let next = self.edges[e].next;
        self.edges[prev].next = next;
        self.edges[next].prev = prev;
        self.edges[e].prev = NIL; // flag as removed (see ClipperBase.Clear)
        next
    }

    fn find_next_loc_min(&self, mut e: usize) -> usize {
        loop {
            while self.e(e).bot != self.e(self.e(e).prev).bot || self.e(e).curr == self.e(e).top {
                e = self.e(e).next;
            }
            if !is_horizontal(self.e(e)) && !is_horizontal(self.e(self.e(e).prev)) {
                break;
            }
            while is_horizontal(self.e(self.e(e).prev)) {
                e = self.e(e).prev;
            }
            let e2 = e;
            while is_horizontal(self.e(e)) {
                e = self.e(e).next;
            }
            if self.e(e).top.y == self.e(self.e(e).prev).bot.y {
                continue; // ie just an intermediate horz.
            }
            if self.e(self.e(e2).prev).bot.x < self.e(e).bot.x {
                e = e2;
            }
            break;
        }
        e
    }

    fn process_bound(&mut self, mut e: usize, next_is_forward: bool) -> usize {
        let mut result = e;
        let horz;

        if self.e(e).out_idx == SKIP {
            // if edges still remain in the current bound beyond the skip edge then
            // create another LocMin and call ProcessBound once more
            if next_is_forward {
                while self.e(e).top.y == self.e(self.e(e).next).bot.y {
                    e = self.e(e).next;
                }
                // don't include top horizontals when parsing a bound a second time,
                // they will be contained in the opposite bound ...
                while e != result && is_horizontal(self.e(e)) {
                    e = self.e(e).prev;
                }
            } else {
                while self.e(e).top.y == self.e(self.e(e).prev).bot.y {
                    e = self.e(e).prev;
                }
                while e != result && is_horizontal(self.e(e)) {
                    e = self.e(e).next;
                }
            }

            if e == result {
                if next_is_forward {
                    result = self.e(e).next;
                } else {
                    result = self.e(e).prev;
                }
            } else {
                // there are more edges in the bound beyond result starting with E
                if next_is_forward {
                    e = self.e(result).next;
                } else {
                    e = self.e(result).prev;
                }
                let loc_min = LocalMinimum { y: self.e(e).bot.y, left_bound: NIL, right_bound: e };
                self.em(e).wind_delta = 0;
                result = self.process_bound(e, next_is_forward);
                self.minima_list.push(loc_min);
            }
            return result;
        }

        let mut e_start;

        if is_horizontal(self.e(e)) {
            // We need to be careful with open paths because this may not be a
            // true local minima (ie E may be following a skip edge).
            // Also, consecutive horz. edges may start heading left before going right.
            if next_is_forward {
                e_start = self.e(e).prev;
            } else {
                e_start = self.e(e).next;
            }
            if is_horizontal(self.e(e_start)) {
                // ie an adjoining horizontal skip edge
                if self.e(e_start).bot.x != self.e(e).bot.x && self.e(e_start).top.x != self.e(e).bot.x {
                    reverse_horizontal(self.em(e));
                }
            } else if self.e(e_start).bot.x != self.e(e).bot.x {
                reverse_horizontal(self.em(e));
            }
        }

        e_start = e;
        if next_is_forward {
            while self.e(result).top.y == self.e(self.e(result).next).bot.y && self.e(self.e(result).next).out_idx != SKIP {
                result = self.e(result).next;
            }
            if is_horizontal(self.e(result)) && self.e(self.e(result).next).out_idx != SKIP {
                // nb: at the top of a bound, horizontals are added to the bound
                // only when the preceding edge attaches to the horizontal's left vertex
                // unless a Skip edge is encountered when that becomes the top divide
                let mut h = result;
                while is_horizontal(self.e(self.e(h).prev)) {
                    h = self.e(h).prev;
                }
                horz = h;
                if self.e(self.e(horz).prev).top.x > self.e(self.e(result).next).top.x {
                    result = self.e(horz).prev;
                }
            }
            while e != result {
                let next = self.e(e).next;
                self.em(e).next_in_lml = next;
                if is_horizontal(self.e(e)) && e != e_start && self.e(e).bot.x != self.e(self.e(e).prev).top.x {
                    reverse_horizontal(self.em(e));
                }
                e = self.e(e).next;
            }
            if is_horizontal(self.e(e)) && e != e_start && self.e(e).bot.x != self.e(self.e(e).prev).top.x {
                reverse_horizontal(self.em(e));
            }
            result = self.e(result).next; // move to the edge just beyond current bound
        } else {
            while self.e(result).top.y == self.e(self.e(result).prev).bot.y && self.e(self.e(result).prev).out_idx != SKIP {
                result = self.e(result).prev;
            }
            if is_horizontal(self.e(result)) && self.e(self.e(result).prev).out_idx != SKIP {
                let mut h = result;
                while is_horizontal(self.e(self.e(h).next)) {
                    h = self.e(h).next;
                }
                horz = h;
                let hn_top_x = self.e(self.e(horz).next).top.x;
                let rp_top_x = self.e(self.e(result).prev).top.x;
                if hn_top_x == rp_top_x || hn_top_x > rp_top_x {
                    result = self.e(horz).next;
                }
            }

            while e != result {
                let prev = self.e(e).prev;
                self.em(e).next_in_lml = prev;
                if is_horizontal(self.e(e)) && e != e_start && self.e(e).bot.x != self.e(self.e(e).next).top.x {
                    reverse_horizontal(self.em(e));
                }
                e = self.e(e).prev;
            }
            if is_horizontal(self.e(e)) && e != e_start && self.e(e).bot.x != self.e(self.e(e).next).top.x {
                reverse_horizontal(self.em(e));
            }
            result = self.e(result).prev; // move to the edge just beyond current bound
        }

        result
    }

    /// `ClipperBase::AddPath`. Open paths must be subject paths.
    pub fn add_path(&mut self, pg: &[IntPoint], poly_typ: PolyType, closed: bool) -> bool {
        if !closed && poly_typ == PolyType::Clip {
            panic!("AddPath: Open paths must be subject.");
        }
        if pg.is_empty() {
            return false;
        }

        let mut high_i = pg.len() as isize - 1;
        if closed {
            while high_i > 0 && pg[high_i as usize] == pg[0] {
                high_i -= 1;
            }
        }
        while high_i > 0 && pg[high_i as usize] == pg[high_i as usize - 1] {
            high_i -= 1;
        }
        if (closed && high_i < 2) || (!closed && high_i < 1) {
            return false;
        }
        let high_i = high_i as usize;

        // create a new edge array ...
        let base = self.edges.len();
        self.edges.resize(base + high_i + 1, TEdge::default());

        let mut is_flat = true;
        // 1. Basic (first) edge initialization ...
        let mut ufr = self.use_full_range;
        if range_test(pg[0], &mut ufr).is_err() || range_test(pg[high_i], &mut ufr).is_err() {
            self.edges.truncate(base);
            panic!("Coordinate outside allowed range");
        }
        self.use_full_range = ufr;
        self.edges[base + 1].curr = pg[1];
        self.init_edge(base, base + 1, base + high_i, pg[0]);
        self.init_edge(base + high_i, base, base + high_i - 1, pg[high_i]);
        let mut i = high_i as isize - 1;
        while i >= 1 {
            let iu = i as usize;
            let mut ufr = self.use_full_range;
            if range_test(pg[iu], &mut ufr).is_err() {
                self.edges.truncate(base);
                panic!("Coordinate outside allowed range");
            }
            self.use_full_range = ufr;
            self.init_edge(base + iu, base + iu + 1, base + iu - 1, pg[iu]);
            i -= 1;
        }
        let mut e_start = base;

        // 2. Remove duplicate vertices, and (when closed) collinear edges ...
        let mut e = e_start;
        let mut e_loop_stop = e_start;
        loop {
            // nb: allows matching start and end points when not Closed ...
            if self.e(e).curr == self.e(self.e(e).next).curr && (closed || self.e(e).next != e_start) {
                if e == self.e(e).next {
                    break;
                }
                if e == e_start {
                    e_start = self.e(e).next;
                }
                e = self.remove_edge(e);
                e_loop_stop = e;
                continue;
            }
            if self.e(e).prev == self.e(e).next {
                break; // only two vertices
            } else if closed
                && slopes_equal3(self.e(self.e(e).prev).curr, self.e(e).curr, self.e(self.e(e).next).curr)
                && (!self.preserve_collinear
                    || !pt2_is_between_pt1_and_pt3(self.e(self.e(e).prev).curr, self.e(e).curr, self.e(self.e(e).next).curr))
            {
                // Collinear edges are allowed for open paths but in closed paths
                // the default is to merge adjacent collinear edges into a single edge.
                // However, if the PreserveCollinear property is enabled, only overlapping
                // collinear edges (ie spikes) will be removed from closed paths.
                if e == e_start {
                    e_start = self.e(e).next;
                }
                e = self.remove_edge(e);
                e = self.e(e).prev;
                e_loop_stop = e;
                continue;
            }
            e = self.e(e).next;
            if e == e_loop_stop || (!closed && self.e(e).next == e_start) {
                break;
            }
        }

        if (!closed && e == self.e(e).next) || (closed && self.e(e).prev == self.e(e).next) {
            self.edges.truncate(base);
            return false;
        }

        if !closed {
            self.has_open_paths = true;
            let p = self.e(e_start).prev;
            self.em(p).out_idx = SKIP;
        }

        // 3. Do second stage of edge initialization ...
        e = e_start;
        loop {
            self.init_edge2(e, poly_typ);
            e = self.e(e).next;
            if is_flat && self.e(e).curr.y != self.e(e_start).curr.y {
                is_flat = false;
            }
            if e == e_start {
                break;
            }
        }

        // 4. Finally, add edge bounds to LocalMinima list ...

        // Totally flat paths must be handled differently when adding them
        // to LocalMinima list to avoid endless loops etc ...
        if is_flat {
            if closed {
                self.edges.truncate(base);
                return false;
            }
            let p = self.e(e).prev;
            self.em(p).out_idx = SKIP;
            let loc_min = LocalMinimum { y: self.e(e).bot.y, left_bound: NIL, right_bound: e };
            self.em(e).side = ES_RIGHT;
            self.em(e).wind_delta = 0;
            loop {
                if self.e(e).bot.x != self.e(self.e(e).prev).top.x {
                    reverse_horizontal(self.em(e));
                }
                if self.e(self.e(e).next).out_idx == SKIP {
                    break;
                }
                let next = self.e(e).next;
                self.em(e).next_in_lml = next;
                e = next;
            }
            self.minima_list.push(loc_min);
            return true;
        }

        let mut left_bound_is_forward;
        let mut e_min = NIL;

        // workaround to avoid an endless loop in the while loop below when
        // open paths have matching start and end points ...
        if self.e(self.e(e).prev).bot == self.e(self.e(e).prev).top {
            e = self.e(e).next;
        }

        loop {
            e = self.find_next_loc_min(e);
            if e == e_min {
                break;
            } else if e_min == NIL {
                e_min = e;
            }

            // E and E.Prev now share a local minima (left aligned if horizontal).
            // Compare their slopes to find which starts which bound ...
            let mut loc_min = LocalMinimum { y: self.e(e).bot.y, left_bound: NIL, right_bound: NIL };
            if self.e(e).dx < self.e(self.e(e).prev).dx {
                loc_min.left_bound = self.e(e).prev;
                loc_min.right_bound = e;
                left_bound_is_forward = false; // Q.nextInLML = Q.prev
            } else {
                loc_min.left_bound = e;
                loc_min.right_bound = self.e(e).prev;
                left_bound_is_forward = true; // Q.nextInLML = Q.next
            }

            let lb = loc_min.left_bound;
            let rb = loc_min.right_bound;
            if !closed {
                self.em(lb).wind_delta = 0;
            } else if self.e(lb).next == rb {
                self.em(lb).wind_delta = -1;
            } else {
                self.em(lb).wind_delta = 1;
            }
            let wd = self.e(lb).wind_delta;
            self.em(rb).wind_delta = -wd;

            e = self.process_bound(lb, left_bound_is_forward);
            if self.e(e).out_idx == SKIP {
                e = self.process_bound(e, left_bound_is_forward);
            }

            let mut e2 = self.process_bound(rb, !left_bound_is_forward);
            if self.e(e2).out_idx == SKIP {
                e2 = self.process_bound(e2, !left_bound_is_forward);
            }

            if self.e(lb).out_idx == SKIP {
                loc_min.left_bound = NIL;
            } else if self.e(rb).out_idx == SKIP {
                loc_min.right_bound = NIL;
            }
            self.minima_list.push(loc_min);
            if !left_bound_is_forward {
                e = e2;
            }
        }
        true
    }

    /// `ClipperBase::AddPaths`.
    pub fn add_paths<P: AsRef<[IntPoint]>>(&mut self, ppg: &[P], poly_typ: PolyType, closed: bool) -> bool {
        let mut result = false;
        for p in ppg {
            if self.add_path(p.as_ref(), poly_typ, closed) {
                result = true;
            }
        }
        result
    }

    /// `ClipperBase::Clear`.
    pub fn clear(&mut self) {
        self.minima_list.clear();
        self.current_lm = 0;
        self.edges.clear();
        self.use_full_range = false;
        self.has_open_paths = false;
    }

    fn reset(&mut self) {
        self.current_lm = 0;
        if self.minima_list.is_empty() {
            return; // ie nothing to process
        }
        cppstd::sort_by(&mut self.minima_list, |a, b| b.y < a.y);

        self.scanbeam.clear(); // clears/resets priority_queue
        // reset all edges ...
        for i in 0..self.minima_list.len() {
            let lm = self.minima_list[i];
            self.scanbeam.push(lm.y);
            let e = lm.left_bound;
            if e != NIL {
                let edge = &mut self.edges[e];
                edge.curr = edge.bot;
                edge.side = ES_LEFT;
                edge.out_idx = UNASSIGNED;
            }
            let e = lm.right_bound;
            if e != NIL {
                let edge = &mut self.edges[e];
                edge.curr = edge.bot;
                edge.side = ES_RIGHT;
                edge.out_idx = UNASSIGNED;
            }
        }
        self.active_edges = NIL;
        self.current_lm = 0;
    }

    fn pop_local_minima(&mut self, y: CInt) -> Option<LocalMinimum> {
        if self.current_lm >= self.minima_list.len() || self.minima_list[self.current_lm].y != y {
            return None;
        }
        let lm = self.minima_list[self.current_lm];
        self.current_lm += 1;
        Some(lm)
    }

    #[inline]
    fn local_minima_pending(&self) -> bool {
        self.current_lm < self.minima_list.len()
    }

    /// `ClipperBase::GetBounds`.
    pub fn get_bounds(&self) -> IntRect {
        let mut result = IntRect::default();
        if self.minima_list.is_empty() {
            return result;
        }
        let first = &self.minima_list[0];
        result.left = self.e(first.left_bound).bot.x;
        result.top = self.e(first.left_bound).bot.y;
        result.right = self.e(first.left_bound).bot.x;
        result.bottom = self.e(first.left_bound).bot.y;
        for lm in &self.minima_list {
            // todo - needs fixing for open paths
            result.bottom = result.bottom.max(self.e(lm.left_bound).bot.y);
            let mut e = lm.left_bound;
            loop {
                let bottom_e = e;
                while self.e(e).next_in_lml != NIL {
                    if self.e(e).bot.x < result.left {
                        result.left = self.e(e).bot.x;
                    }
                    if self.e(e).bot.x > result.right {
                        result.right = self.e(e).bot.x;
                    }
                    e = self.e(e).next_in_lml;
                }
                result.left = result.left.min(self.e(e).bot.x);
                result.right = result.right.max(self.e(e).bot.x);
                result.left = result.left.min(self.e(e).top.x);
                result.right = result.right.max(self.e(e).top.x);
                result.top = result.top.min(self.e(e).top.y);
                if bottom_e == lm.left_bound {
                    e = lm.right_bound;
                } else {
                    break;
                }
            }
        }
        result
    }

    #[inline]
    fn insert_scanbeam(&mut self, y: CInt) {
        self.scanbeam.push(y);
    }

    fn pop_scanbeam(&mut self) -> Option<CInt> {
        let y = self.scanbeam.pop()?;
        while let Some(&top) = self.scanbeam.peek() {
            if top == y {
                self.scanbeam.pop(); // Pop duplicates.
            } else {
                break;
            }
        }
        Some(y)
    }

    fn dispose_all_out_recs(&mut self) {
        self.poly_outs.clear();
        self.out_pts.clear();
    }

    fn delete_from_ael(&mut self, e: usize) {
        let ael_prev = self.e(e).prev_in_ael;
        let ael_next = self.e(e).next_in_ael;
        if ael_prev == NIL && ael_next == NIL && e != self.active_edges {
            return; // already deleted
        }
        if ael_prev != NIL {
            self.em(ael_prev).next_in_ael = ael_next;
        } else {
            self.active_edges = ael_next;
        }
        if ael_next != NIL {
            self.em(ael_next).prev_in_ael = ael_prev;
        }
        self.em(e).next_in_ael = NIL;
        self.em(e).prev_in_ael = NIL;
    }

    fn create_out_rec(&mut self) -> usize {
        let idx = self.poly_outs.len();
        self.poly_outs.push(OutRec {
            idx: idx as i32,
            is_hole: false,
            is_open: false,
            first_left: NIL,
            poly_nd: NIL,
            pts: NIL,
            bottom_pt: NIL,
        });
        idx
    }

    fn new_out_pt(&mut self, idx: i32, pt: IntPoint) -> usize {
        let i = self.out_pts.len();
        self.out_pts.push(OutPt { idx, pt, next: NIL, prev: NIL });
        i
    }

    fn swap_positions_in_ael(&mut self, edge1: usize, edge2: usize) {
        // check that one or other edge hasn't already been removed from AEL ...
        if self.e(edge1).next_in_ael == self.e(edge1).prev_in_ael || self.e(edge2).next_in_ael == self.e(edge2).prev_in_ael {
            return;
        }

        if self.e(edge1).next_in_ael == edge2 {
            let next = self.e(edge2).next_in_ael;
            if next != NIL {
                self.em(next).prev_in_ael = edge1;
            }
            let prev = self.e(edge1).prev_in_ael;
            if prev != NIL {
                self.em(prev).next_in_ael = edge2;
            }
            self.em(edge2).prev_in_ael = prev;
            self.em(edge2).next_in_ael = edge1;
            self.em(edge1).prev_in_ael = edge2;
            self.em(edge1).next_in_ael = next;
        } else if self.e(edge2).next_in_ael == edge1 {
            let next = self.e(edge1).next_in_ael;
            if next != NIL {
                self.em(next).prev_in_ael = edge2;
            }
            let prev = self.e(edge2).prev_in_ael;
            if prev != NIL {
                self.em(prev).next_in_ael = edge1;
            }
            self.em(edge1).prev_in_ael = prev;
            self.em(edge1).next_in_ael = edge2;
            self.em(edge2).prev_in_ael = edge1;
            self.em(edge2).next_in_ael = next;
        } else {
            let next = self.e(edge1).next_in_ael;
            let prev = self.e(edge1).prev_in_ael;
            let e2n = self.e(edge2).next_in_ael;
            self.em(edge1).next_in_ael = e2n;
            if e2n != NIL {
                self.em(e2n).prev_in_ael = edge1;
            }
            let e2p = self.e(edge2).prev_in_ael;
            self.em(edge1).prev_in_ael = e2p;
            if e2p != NIL {
                self.em(e2p).next_in_ael = edge1;
            }
            self.em(edge2).next_in_ael = next;
            if next != NIL {
                self.em(next).prev_in_ael = edge2;
            }
            self.em(edge2).prev_in_ael = prev;
            if prev != NIL {
                self.em(prev).next_in_ael = edge2;
            }
        }

        if self.e(edge1).prev_in_ael == NIL {
            self.active_edges = edge1;
        } else if self.e(edge2).prev_in_ael == NIL {
            self.active_edges = edge2;
        }
    }

    fn update_edge_into_ael(&mut self, e: &mut usize) -> Result<(), ClipperError> {
        let ei = *e;
        let nlml = self.e(ei).next_in_lml;
        if nlml == NIL {
            return Err(ClipperError("UpdateEdgeIntoAEL: invalid call"));
        }

        self.em(nlml).out_idx = self.e(ei).out_idx;
        let ael_prev = self.e(ei).prev_in_ael;
        let ael_next = self.e(ei).next_in_ael;
        if ael_prev != NIL {
            self.em(ael_prev).next_in_ael = nlml;
        } else {
            self.active_edges = nlml;
        }
        if ael_next != NIL {
            self.em(ael_next).prev_in_ael = nlml;
        }
        let (side, wd, wc, wc2) = {
            let o = self.e(ei);
            (o.side, o.wind_delta, o.wind_cnt, o.wind_cnt2)
        };
        {
            let n = self.em(nlml);
            n.side = side;
            n.wind_delta = wd;
            n.wind_cnt = wc;
            n.wind_cnt2 = wc2;
        }
        *e = nlml;
        let n = self.em(nlml);
        n.curr = n.bot;
        n.prev_in_ael = ael_prev;
        n.next_in_ael = ael_next;
        if !is_horizontal(self.e(nlml)) {
            let ty = self.e(nlml).top.y;
            self.insert_scanbeam(ty);
        }
        Ok(())
    }

    // --------------------------------------------------------------------------------------
    // Clipper
    // --------------------------------------------------------------------------------------

    /// `Clipper::Execute(clipType, Paths&, subjFillType, clipFillType)`.
    pub fn execute(
        &mut self,
        clip_type: ClipType,
        solution: &mut Paths,
        subj_fill_type: PolyFillType,
        clip_fill_type: PolyFillType,
    ) -> bool {
        if self.execute_locked {
            return false;
        }
        if self.has_open_paths {
            panic!("Error: PolyTree struct is needed for open path clipping.");
        }
        self.execute_locked = true;
        solution.clear();
        self.subj_fill_type = subj_fill_type;
        self.clip_fill_type = clip_fill_type;
        self.clip_type = clip_type;
        self.using_poly_tree = false;
        let succeeded = self.execute_internal();
        if succeeded {
            self.build_result(solution);
        }
        self.dispose_all_out_recs();
        self.execute_locked = false;
        succeeded
    }

    /// Convenience: execute with the same fill type for subject and clip.
    pub fn execute_paths(&mut self, clip_type: ClipType, fill_type: PolyFillType) -> Paths {
        let mut sol = Paths::new();
        self.execute(clip_type, &mut sol, fill_type, fill_type);
        sol
    }

    /// `Clipper::Execute(clipType, PolyTree&, subjFillType, clipFillType)`.
    pub fn execute_tree(
        &mut self,
        clip_type: ClipType,
        polytree: &mut PolyTree,
        subj_fill_type: PolyFillType,
        clip_fill_type: PolyFillType,
    ) -> bool {
        if self.execute_locked {
            return false;
        }
        self.execute_locked = true;
        self.subj_fill_type = subj_fill_type;
        self.clip_fill_type = clip_fill_type;
        self.clip_type = clip_type;
        self.using_poly_tree = true;
        let succeeded = self.execute_internal();
        if succeeded {
            self.build_result2(polytree);
        }
        self.dispose_all_out_recs();
        self.execute_locked = false;
        succeeded
    }

    fn fix_hole_linkage(&mut self, outrec: usize) {
        // skip OutRecs that (a) contain outermost polygons or
        // (b) already have the correct owner/child linkage ...
        let fl = self.poly_outs[outrec].first_left;
        if fl == NIL || (self.poly_outs[outrec].is_hole != self.poly_outs[fl].is_hole && self.poly_outs[fl].pts != NIL) {
            return;
        }
        let mut orfl = fl;
        while orfl != NIL && (self.poly_outs[orfl].is_hole == self.poly_outs[outrec].is_hole || self.poly_outs[orfl].pts == NIL) {
            orfl = self.poly_outs[orfl].first_left;
        }
        self.poly_outs[outrec].first_left = orfl;
    }

    fn execute_internal(&mut self) -> bool {
        let mut succeeded = self.execute_internal_loop().unwrap_or(false);

        if succeeded {
            // fix orientations ...
            for i in 0..self.poly_outs.len() {
                let out_rec = self.poly_outs[i];
                if out_rec.pts == NIL || out_rec.is_open {
                    continue;
                }
                if (out_rec.is_hole ^ self.reverse_output) == (self.area_outrec(i) > 0.0) {
                    self.reverse_poly_pt_links(out_rec.pts);
                }
            }

            if !self.joins.is_empty() {
                self.join_common_edges();
            }

            // unfortunately FixupOutPolygon() must be done after JoinCommonEdges()
            for i in 0..self.poly_outs.len() {
                if self.poly_outs[i].pts == NIL {
                    continue;
                }
                if self.poly_outs[i].is_open {
                    self.fixup_out_polyline(i);
                } else {
                    self.fixup_out_polygon(i);
                }
            }

            if self.strict_simple {
                self.do_simple_polygons();
            }
        } else {
            succeeded = false;
        }

        self.joins.clear();
        self.ghost_joins.clear();
        succeeded
    }

    fn execute_internal_loop(&mut self) -> Result<bool, ClipperError> {
        self.reset();
        self.maxima.clear();
        self.sorted_edges = NIL;

        let mut bot_y = match self.pop_scanbeam() {
            Some(y) => y,
            None => return Ok(false),
        };
        self.insert_local_minima_into_ael(bot_y)?;
        loop {
            let top_y_opt = self.pop_scanbeam();
            if top_y_opt.is_none() && !self.local_minima_pending() {
                break;
            }
            // nb: in C++, topY keeps its previous (uninitialised) value when the scanbeam is
            // empty but local minima are pending; that cannot happen for valid input since
            // every local minimum's Y is inserted into the scanbeam.
            let top_y = top_y_opt.unwrap_or(bot_y);
            self.process_horizontals()?;
            self.ghost_joins.clear();
            if !self.process_intersections(top_y)? {
                return Ok(false);
            }
            self.process_edges_at_top_of_scanbeam(top_y)?;
            bot_y = top_y;
            self.insert_local_minima_into_ael(bot_y)?;
        }
        Ok(true)
    }

    fn set_winding_count(&mut self, edge: usize) {
        let mut e = self.e(edge).prev_in_ael;
        // find the edge of the same polytype that immediately preceeds 'edge' in AEL
        while e != NIL && (self.e(e).poly_typ != self.e(edge).poly_typ || self.e(e).wind_delta == 0) {
            e = self.e(e).prev_in_ael;
        }
        if e == NIL {
            if self.e(edge).wind_delta == 0 {
                let pft = if self.e(edge).poly_typ == PolyType::Subject { self.subj_fill_type } else { self.clip_fill_type };
                self.em(edge).wind_cnt = if pft == PolyFillType::Negative { -1 } else { 1 };
            } else {
                let wd = self.e(edge).wind_delta;
                self.em(edge).wind_cnt = wd;
            }
            self.em(edge).wind_cnt2 = 0;
            e = self.active_edges; // ie get ready to calc WindCnt2
        } else if self.e(edge).wind_delta == 0 && self.clip_type != ClipType::Union {
            self.em(edge).wind_cnt = 1;
            let wc2 = self.e(e).wind_cnt2;
            self.em(edge).wind_cnt2 = wc2;
            e = self.e(e).next_in_ael; // ie get ready to calc WindCnt2
        } else if self.is_even_odd_fill_type(edge) {
            // EvenOdd filling ...
            if self.e(edge).wind_delta == 0 {
                // are we inside a subj polygon ...
                let mut inside = true;
                let mut e2 = self.e(e).prev_in_ael;
                while e2 != NIL {
                    if self.e(e2).poly_typ == self.e(e).poly_typ && self.e(e2).wind_delta != 0 {
                        inside = !inside;
                    }
                    e2 = self.e(e2).prev_in_ael;
                }
                self.em(edge).wind_cnt = if inside { 0 } else { 1 };
            } else {
                let wd = self.e(edge).wind_delta;
                self.em(edge).wind_cnt = wd;
            }
            let wc2 = self.e(e).wind_cnt2;
            self.em(edge).wind_cnt2 = wc2;
            e = self.e(e).next_in_ael; // ie get ready to calc WindCnt2
        } else {
            // nonZero, Positive or Negative filling ...
            let (e_wc, e_wd) = (self.e(e).wind_cnt, self.e(e).wind_delta);
            let edge_wd = self.e(edge).wind_delta;
            if e_wc * e_wd < 0 {
                // prev edge is 'decreasing' WindCount (WC) toward zero
                // so we're outside the previous polygon ...
                if e_wc.abs() > 1 {
                    // outside prev poly but still inside another.
                    // when reversing direction of prev poly use the same WC
                    if e_wd * edge_wd < 0 {
                        self.em(edge).wind_cnt = e_wc;
                    } else {
                        // otherwise continue to 'decrease' WC ...
                        self.em(edge).wind_cnt = e_wc + edge_wd;
                    }
                } else {
                    // now outside all polys of same polytype so set own WC ...
                    self.em(edge).wind_cnt = if edge_wd == 0 { 1 } else { edge_wd };
                }
            } else {
                // prev edge is 'increasing' WindCount (WC) away from zero
                // so we're inside the previous polygon ...
                if edge_wd == 0 {
                    self.em(edge).wind_cnt = if e_wc < 0 { e_wc - 1 } else { e_wc + 1 };
                } else if e_wd * edge_wd < 0 {
                    // if wind direction is reversing prev then use same WC
                    self.em(edge).wind_cnt = e_wc;
                } else {
                    // otherwise add to WC ...
                    self.em(edge).wind_cnt = e_wc + edge_wd;
                }
            }
            let wc2 = self.e(e).wind_cnt2;
            self.em(edge).wind_cnt2 = wc2;
            e = self.e(e).next_in_ael; // ie get ready to calc WindCnt2
        }

        // update WindCnt2 ...
        if self.is_even_odd_alt_fill_type(edge) {
            // EvenOdd filling ...
            while e != edge {
                if self.e(e).wind_delta != 0 {
                    let w = self.e(edge).wind_cnt2;
                    self.em(edge).wind_cnt2 = if w == 0 { 1 } else { 0 };
                }
                e = self.e(e).next_in_ael;
            }
        } else {
            // nonZero, Positive or Negative filling ...
            while e != edge {
                let wd = self.e(e).wind_delta;
                self.em(edge).wind_cnt2 += wd;
                e = self.e(e).next_in_ael;
            }
        }
    }

    #[inline]
    fn is_even_odd_fill_type(&self, edge: usize) -> bool {
        if self.e(edge).poly_typ == PolyType::Subject {
            self.subj_fill_type == PolyFillType::EvenOdd
        } else {
            self.clip_fill_type == PolyFillType::EvenOdd
        }
    }

    #[inline]
    fn is_even_odd_alt_fill_type(&self, edge: usize) -> bool {
        if self.e(edge).poly_typ == PolyType::Subject {
            self.clip_fill_type == PolyFillType::EvenOdd
        } else {
            self.subj_fill_type == PolyFillType::EvenOdd
        }
    }

    fn is_contributing(&self, edge: usize) -> bool {
        let ed = self.e(edge);
        let (pft, pft2) = if ed.poly_typ == PolyType::Subject {
            (self.subj_fill_type, self.clip_fill_type)
        } else {
            (self.clip_fill_type, self.subj_fill_type)
        };

        match pft {
            PolyFillType::EvenOdd => {
                // return false if a subj line has been flagged as inside a subj polygon
                if ed.wind_delta == 0 && ed.wind_cnt != 1 {
                    return false;
                }
            }
            PolyFillType::NonZero => {
                if ed.wind_cnt.abs() != 1 {
                    return false;
                }
            }
            PolyFillType::Positive => {
                if ed.wind_cnt != 1 {
                    return false;
                }
            }
            PolyFillType::Negative => {
                if ed.wind_cnt != -1 {
                    return false;
                }
            }
        }

        match self.clip_type {
            ClipType::Intersection => match pft2 {
                PolyFillType::EvenOdd | PolyFillType::NonZero => ed.wind_cnt2 != 0,
                PolyFillType::Positive => ed.wind_cnt2 > 0,
                PolyFillType::Negative => ed.wind_cnt2 < 0,
            },
            ClipType::Union => match pft2 {
                PolyFillType::EvenOdd | PolyFillType::NonZero => ed.wind_cnt2 == 0,
                PolyFillType::Positive => ed.wind_cnt2 <= 0,
                PolyFillType::Negative => ed.wind_cnt2 >= 0,
            },
            ClipType::Difference => {
                if ed.poly_typ == PolyType::Subject {
                    match pft2 {
                        PolyFillType::EvenOdd | PolyFillType::NonZero => ed.wind_cnt2 == 0,
                        PolyFillType::Positive => ed.wind_cnt2 <= 0,
                        PolyFillType::Negative => ed.wind_cnt2 >= 0,
                    }
                } else {
                    match pft2 {
                        PolyFillType::EvenOdd | PolyFillType::NonZero => ed.wind_cnt2 != 0,
                        PolyFillType::Positive => ed.wind_cnt2 > 0,
                        PolyFillType::Negative => ed.wind_cnt2 < 0,
                    }
                }
            }
            ClipType::Xor => {
                if ed.wind_delta == 0 {
                    // XOr always contributing unless open
                    match pft2 {
                        PolyFillType::EvenOdd | PolyFillType::NonZero => ed.wind_cnt2 == 0,
                        PolyFillType::Positive => ed.wind_cnt2 <= 0,
                        PolyFillType::Negative => ed.wind_cnt2 >= 0,
                    }
                } else {
                    true
                }
            }
        }
    }

    fn add_local_min_poly(&mut self, e1: usize, e2: usize, pt: IntPoint) -> usize {
        let result;
        let e;
        let prev_e;
        if is_horizontal(self.e(e2)) || self.e(e1).dx > self.e(e2).dx {
            result = self.add_out_pt(e1, pt);
            let oi = self.e(e1).out_idx;
            self.em(e2).out_idx = oi;
            self.em(e1).side = ES_LEFT;
            self.em(e2).side = ES_RIGHT;
            e = e1;
            if self.e(e).prev_in_ael == e2 {
                prev_e = self.e(e2).prev_in_ael;
            } else {
                prev_e = self.e(e).prev_in_ael;
            }
        } else {
            result = self.add_out_pt(e2, pt);
            let oi = self.e(e2).out_idx;
            self.em(e1).out_idx = oi;
            self.em(e1).side = ES_RIGHT;
            self.em(e2).side = ES_LEFT;
            e = e2;
            if self.e(e).prev_in_ael == e1 {
                prev_e = self.e(e1).prev_in_ael;
            } else {
                prev_e = self.e(e).prev_in_ael;
            }
        }

        if prev_e != NIL && self.e(prev_e).out_idx >= 0 && self.e(prev_e).top.y < pt.y && self.e(e).top.y < pt.y {
            let x_prev = top_x(self.e(prev_e), pt.y);
            let x_e = top_x(self.e(e), pt.y);
            if x_prev == x_e
                && self.e(e).wind_delta != 0
                && self.e(prev_e).wind_delta != 0
                && slopes_equal4(IntPoint::new(x_prev, pt.y), self.e(prev_e).top, IntPoint::new(x_e, pt.y), self.e(e).top)
            {
                let out_pt = self.add_out_pt(prev_e, pt);
                let off = self.e(e).top;
                self.add_join(result, out_pt, off);
            }
        }
        result
    }

    fn add_local_max_poly(&mut self, e1: usize, e2: usize, pt: IntPoint) {
        self.add_out_pt(e1, pt);
        if self.e(e2).wind_delta == 0 {
            self.add_out_pt(e2, pt);
        }
        if self.e(e1).out_idx == self.e(e2).out_idx {
            self.em(e1).out_idx = UNASSIGNED;
            self.em(e2).out_idx = UNASSIGNED;
        } else if self.e(e1).out_idx < self.e(e2).out_idx {
            self.append_polygon(e1, e2);
        } else {
            self.append_polygon(e2, e1);
        }
    }

    fn add_edge_to_sel(&mut self, edge: usize) {
        // SEL pointers in PEdge are reused to build a list of horizontal edges.
        // However, we don't need to worry about order with horizontal edge processing.
        if self.sorted_edges == NIL {
            self.sorted_edges = edge;
            self.em(edge).prev_in_sel = NIL;
            self.em(edge).next_in_sel = NIL;
        } else {
            let se = self.sorted_edges;
            self.em(edge).next_in_sel = se;
            self.em(edge).prev_in_sel = NIL;
            self.em(se).prev_in_sel = edge;
            self.sorted_edges = edge;
        }
    }

    fn pop_edge_from_sel(&mut self) -> Option<usize> {
        if self.sorted_edges == NIL {
            return None;
        }
        let edge = self.sorted_edges;
        self.delete_from_sel(edge);
        Some(edge)
    }

    fn copy_ael_to_sel(&mut self) {
        let mut e = self.active_edges;
        self.sorted_edges = e;
        while e != NIL {
            let (p, n) = (self.e(e).prev_in_ael, self.e(e).next_in_ael);
            self.em(e).prev_in_sel = p;
            self.em(e).next_in_sel = n;
            e = n;
        }
    }

    #[inline]
    fn add_join(&mut self, op1: usize, op2: usize, off_pt: IntPoint) {
        self.joins.push(Join { out_pt1: op1, out_pt2: op2, off_pt });
    }

    #[inline]
    fn add_ghost_join(&mut self, op: usize, off_pt: IntPoint) {
        self.ghost_joins.push(Join { out_pt1: op, out_pt2: NIL, off_pt });
    }

    fn insert_local_minima_into_ael(&mut self, bot_y: CInt) -> Result<(), ClipperError> {
        while let Some(lm) = self.pop_local_minima(bot_y) {
            let lb = lm.left_bound;
            let rb = lm.right_bound;

            let mut op1 = NIL;
            if lb == NIL {
                // nb: don't insert LB into either AEL or SEL
                self.insert_edge_into_ael(rb, NIL);
                self.set_winding_count(rb);
                if self.is_contributing(rb) {
                    let b = self.e(rb).bot;
                    op1 = self.add_out_pt(rb, b);
                }
            } else if rb == NIL {
                self.insert_edge_into_ael(lb, NIL);
                self.set_winding_count(lb);
                if self.is_contributing(lb) {
                    let b = self.e(lb).bot;
                    op1 = self.add_out_pt(lb, b);
                }
                let ty = self.e(lb).top.y;
                self.insert_scanbeam(ty);
            } else {
                self.insert_edge_into_ael(lb, NIL);
                self.insert_edge_into_ael(rb, lb);
                self.set_winding_count(lb);
                let (wc, wc2) = (self.e(lb).wind_cnt, self.e(lb).wind_cnt2);
                self.em(rb).wind_cnt = wc;
                self.em(rb).wind_cnt2 = wc2;
                if self.is_contributing(lb) {
                    let b = self.e(lb).bot;
                    op1 = self.add_local_min_poly(lb, rb, b);
                }
                let ty = self.e(lb).top.y;
                self.insert_scanbeam(ty);
            }

            if rb != NIL {
                if is_horizontal(self.e(rb)) {
                    self.add_edge_to_sel(rb);
                    let nl = self.e(rb).next_in_lml;
                    if nl != NIL {
                        let ty = self.e(nl).top.y;
                        self.insert_scanbeam(ty);
                    }
                } else {
                    let ty = self.e(rb).top.y;
                    self.insert_scanbeam(ty);
                }
            }

            if lb == NIL || rb == NIL {
                continue;
            }

            // if any output polygons share an edge, they'll need joining later ...
            if op1 != NIL && is_horizontal(self.e(rb)) && !self.ghost_joins.is_empty() && self.e(rb).wind_delta != 0 {
                for i in 0..self.ghost_joins.len() {
                    let jr = self.ghost_joins[i];
                    // if the horizontal Rb and a 'ghost' horizontal overlap, then convert
                    // the 'ghost' join to a real join ready for later ...
                    if horz_segments_overlap(self.out_pts[jr.out_pt1].pt.x, jr.off_pt.x, self.e(rb).bot.x, self.e(rb).top.x) {
                        self.add_join(jr.out_pt1, op1, jr.off_pt);
                    }
                }
            }

            let lb_prev = self.e(lb).prev_in_ael;
            if self.e(lb).out_idx >= 0
                && lb_prev != NIL
                && self.e(lb_prev).curr.x == self.e(lb).bot.x
                && self.e(lb_prev).out_idx >= 0
                && slopes_equal4(self.e(lb_prev).bot, self.e(lb_prev).top, self.e(lb).curr, self.e(lb).top)
                && self.e(lb).wind_delta != 0
                && self.e(lb_prev).wind_delta != 0
            {
                let b = self.e(lb).bot;
                let op2 = self.add_out_pt(lb_prev, b);
                let t = self.e(lb).top;
                self.add_join(op1, op2, t);
            }

            if self.e(lb).next_in_ael != rb {
                let rb_prev = self.e(rb).prev_in_ael;
                if self.e(rb).out_idx >= 0
                    && self.e(rb_prev).out_idx >= 0
                    && slopes_equal4(self.e(rb_prev).curr, self.e(rb_prev).top, self.e(rb).curr, self.e(rb).top)
                    && self.e(rb).wind_delta != 0
                    && self.e(rb_prev).wind_delta != 0
                {
                    let b = self.e(rb).bot;
                    let op2 = self.add_out_pt(rb_prev, b);
                    let t = self.e(rb).top;
                    self.add_join(op1, op2, t);
                }

                let mut e = self.e(lb).next_in_ael;
                if e != NIL {
                    while e != rb {
                        // nb: For calculating winding counts etc, IntersectEdges() assumes
                        // that param1 will be to the Right of param2 ABOVE the intersection ...
                        let c = self.e(lb).curr;
                        self.intersect_edges(rb, e, c); // order important here
                        e = self.e(e).next_in_ael;
                    }
                }
            }
        }
        Ok(())
    }

    fn delete_from_sel(&mut self, e: usize) {
        let sel_prev = self.e(e).prev_in_sel;
        let sel_next = self.e(e).next_in_sel;
        if sel_prev == NIL && sel_next == NIL && e != self.sorted_edges {
            return; // already deleted
        }
        if sel_prev != NIL {
            self.em(sel_prev).next_in_sel = sel_next;
        } else {
            self.sorted_edges = sel_next;
        }
        if sel_next != NIL {
            self.em(sel_next).prev_in_sel = sel_prev;
        }
        self.em(e).next_in_sel = NIL;
        self.em(e).prev_in_sel = NIL;
    }

    fn intersect_edges(&mut self, e1: usize, e2: usize, pt: IntPoint) {
        let e1_contributing = self.e(e1).out_idx >= 0;
        let e2_contributing = self.e(e2).out_idx >= 0;

        // if either edge is on an OPEN path ...
        if self.e(e1).wind_delta == 0 || self.e(e2).wind_delta == 0 {
            // ignore subject-subject open path intersections UNLESS they
            // are both open paths, AND they are both 'contributing maximas' ...
            if self.e(e1).wind_delta == 0 && self.e(e2).wind_delta == 0 {
                return;
            }
            // if intersecting a subj line with a subj poly ...
            else if self.e(e1).poly_typ == self.e(e2).poly_typ
                && self.e(e1).wind_delta != self.e(e2).wind_delta
                && self.clip_type == ClipType::Union
            {
                if self.e(e1).wind_delta == 0 {
                    if e2_contributing {
                        self.add_out_pt(e1, pt);
                        if e1_contributing {
                            self.em(e1).out_idx = UNASSIGNED;
                        }
                    }
                } else {
                    if e1_contributing {
                        self.add_out_pt(e2, pt);
                        if e2_contributing {
                            self.em(e2).out_idx = UNASSIGNED;
                        }
                    }
                }
            } else if self.e(e1).poly_typ != self.e(e2).poly_typ {
                // toggle subj open path OutIdx on/off when Abs(clip.WndCnt) == 1 ...
                if self.e(e1).wind_delta == 0
                    && self.e(e2).wind_cnt.abs() == 1
                    && (self.clip_type != ClipType::Union || self.e(e2).wind_cnt2 == 0)
                {
                    self.add_out_pt(e1, pt);
                    if e1_contributing {
                        self.em(e1).out_idx = UNASSIGNED;
                    }
                } else if self.e(e2).wind_delta == 0
                    && self.e(e1).wind_cnt.abs() == 1
                    && (self.clip_type != ClipType::Union || self.e(e1).wind_cnt2 == 0)
                {
                    self.add_out_pt(e2, pt);
                    if e2_contributing {
                        self.em(e2).out_idx = UNASSIGNED;
                    }
                }
            }
            return;
        }

        // update winding counts...
        // assumes that e1 will be to the Right of e2 ABOVE the intersection
        if self.e(e1).poly_typ == self.e(e2).poly_typ {
            if self.is_even_odd_fill_type(e1) {
                let old_e1_wind_cnt = self.e(e1).wind_cnt;
                let w2 = self.e(e2).wind_cnt;
                self.em(e1).wind_cnt = w2;
                self.em(e2).wind_cnt = old_e1_wind_cnt;
            } else {
                let e2wd = self.e(e2).wind_delta;
                let e1wd = self.e(e1).wind_delta;
                if self.e(e1).wind_cnt + e2wd == 0 {
                    let w = self.e(e1).wind_cnt;
                    self.em(e1).wind_cnt = -w;
                } else {
                    self.em(e1).wind_cnt += e2wd;
                }
                if self.e(e2).wind_cnt - e1wd == 0 {
                    let w = self.e(e2).wind_cnt;
                    self.em(e2).wind_cnt = -w;
                } else {
                    self.em(e2).wind_cnt -= e1wd;
                }
            }
        } else {
            if !self.is_even_odd_fill_type(e2) {
                let wd = self.e(e2).wind_delta;
                self.em(e1).wind_cnt2 += wd;
            } else {
                let w = self.e(e1).wind_cnt2;
                self.em(e1).wind_cnt2 = if w == 0 { 1 } else { 0 };
            }
            if !self.is_even_odd_fill_type(e1) {
                let wd = self.e(e1).wind_delta;
                self.em(e2).wind_cnt2 -= wd;
            } else {
                let w = self.e(e2).wind_cnt2;
                self.em(e2).wind_cnt2 = if w == 0 { 1 } else { 0 };
            }
        }

        let (e1_fill_type, e1_fill_type2) = if self.e(e1).poly_typ == PolyType::Subject {
            (self.subj_fill_type, self.clip_fill_type)
        } else {
            (self.clip_fill_type, self.subj_fill_type)
        };
        let (e2_fill_type, e2_fill_type2) = if self.e(e2).poly_typ == PolyType::Subject {
            (self.subj_fill_type, self.clip_fill_type)
        } else {
            (self.clip_fill_type, self.subj_fill_type)
        };

        let e1_wc: CInt = match e1_fill_type {
            PolyFillType::Positive => self.e(e1).wind_cnt as CInt,
            PolyFillType::Negative => -(self.e(e1).wind_cnt as CInt),
            _ => (self.e(e1).wind_cnt as CInt).abs(),
        };
        let e2_wc: CInt = match e2_fill_type {
            PolyFillType::Positive => self.e(e2).wind_cnt as CInt,
            PolyFillType::Negative => -(self.e(e2).wind_cnt as CInt),
            _ => (self.e(e2).wind_cnt as CInt).abs(),
        };

        if e1_contributing && e2_contributing {
            if (e1_wc != 0 && e1_wc != 1)
                || (e2_wc != 0 && e2_wc != 1)
                || (self.e(e1).poly_typ != self.e(e2).poly_typ && self.clip_type != ClipType::Xor)
            {
                self.add_local_max_poly(e1, e2, pt);
            } else {
                self.add_out_pt(e1, pt);
                self.add_out_pt(e2, pt);
                self.swap_sides(e1, e2);
                self.swap_poly_indexes(e1, e2);
            }
        } else if e1_contributing {
            if e2_wc == 0 || e2_wc == 1 {
                self.add_out_pt(e1, pt);
                self.swap_sides(e1, e2);
                self.swap_poly_indexes(e1, e2);
            }
        } else if e2_contributing {
            if e1_wc == 0 || e1_wc == 1 {
                self.add_out_pt(e2, pt);
                self.swap_sides(e1, e2);
                self.swap_poly_indexes(e1, e2);
            }
        } else if (e1_wc == 0 || e1_wc == 1) && (e2_wc == 0 || e2_wc == 1) {
            // neither edge is currently contributing ...
            let e1_wc2: CInt = match e1_fill_type2 {
                PolyFillType::Positive => self.e(e1).wind_cnt2 as CInt,
                PolyFillType::Negative => -(self.e(e1).wind_cnt2 as CInt),
                _ => (self.e(e1).wind_cnt2 as CInt).abs(),
            };
            let e2_wc2: CInt = match e2_fill_type2 {
                PolyFillType::Positive => self.e(e2).wind_cnt2 as CInt,
                PolyFillType::Negative => -(self.e(e2).wind_cnt2 as CInt),
                _ => (self.e(e2).wind_cnt2 as CInt).abs(),
            };

            if self.e(e1).poly_typ != self.e(e2).poly_typ {
                self.add_local_min_poly(e1, e2, pt);
            } else if e1_wc == 1 && e2_wc == 1 {
                match self.clip_type {
                    ClipType::Intersection => {
                        if e1_wc2 > 0 && e2_wc2 > 0 {
                            self.add_local_min_poly(e1, e2, pt);
                        }
                    }
                    ClipType::Union => {
                        if e1_wc2 <= 0 && e2_wc2 <= 0 {
                            self.add_local_min_poly(e1, e2, pt);
                        }
                    }
                    ClipType::Difference => {
                        if (self.e(e1).poly_typ == PolyType::Clip && e1_wc2 > 0 && e2_wc2 > 0)
                            || (self.e(e1).poly_typ == PolyType::Subject && e1_wc2 <= 0 && e2_wc2 <= 0)
                        {
                            self.add_local_min_poly(e1, e2, pt);
                        }
                    }
                    ClipType::Xor => {
                        self.add_local_min_poly(e1, e2, pt);
                    }
                }
            } else {
                self.swap_sides(e1, e2);
            }
        }
    }

    #[inline]
    fn swap_sides(&mut self, e1: usize, e2: usize) {
        let s = self.e(e1).side;
        let s2 = self.e(e2).side;
        self.em(e1).side = s2;
        self.em(e2).side = s;
    }

    #[inline]
    fn swap_poly_indexes(&mut self, e1: usize, e2: usize) {
        let o = self.e(e1).out_idx;
        let o2 = self.e(e2).out_idx;
        self.em(e1).out_idx = o2;
        self.em(e2).out_idx = o;
    }

    fn set_hole_state(&mut self, e: usize, outrec: usize) {
        let mut e2 = self.e(e).prev_in_ael;
        let mut e_tmp = NIL;
        while e2 != NIL {
            if self.e(e2).out_idx >= 0 && self.e(e2).wind_delta != 0 {
                if e_tmp == NIL {
                    e_tmp = e2;
                } else if self.e(e_tmp).out_idx == self.e(e2).out_idx {
                    e_tmp = NIL;
                }
            }
            e2 = self.e(e2).prev_in_ael;
        }
        if e_tmp == NIL {
            self.poly_outs[outrec].first_left = NIL;
            self.poly_outs[outrec].is_hole = false;
        } else {
            let fl = self.e(e_tmp).out_idx as usize;
            self.poly_outs[outrec].first_left = fl;
            self.poly_outs[outrec].is_hole = !self.poly_outs[fl].is_hole;
        }
    }

    // ---- OutPt helpers --------------------------------------------------------------------

    #[inline]
    fn op(&self, i: usize) -> &OutPt {
        &self.out_pts[i]
    }

    fn area_op(&self, op: usize) -> f64 {
        if op == NIL {
            return 0.0;
        }
        let start = op;
        let mut op = op;
        let mut a = 0.0f64;
        loop {
            let prev = self.op(self.op(op).prev).pt;
            let cur = self.op(op).pt;
            a += (prev.x + cur.x) as f64 * (prev.y - cur.y) as f64;
            op = self.op(op).next;
            if op == start {
                break;
            }
        }
        a * 0.5
    }

    #[inline]
    fn area_outrec(&self, outrec: usize) -> f64 {
        self.area_op(self.poly_outs[outrec].pts)
    }

    fn reverse_poly_pt_links(&mut self, pp: usize) {
        if pp == NIL {
            return;
        }
        let mut pp1 = pp;
        loop {
            let pp2 = self.out_pts[pp1].next;
            self.out_pts[pp1].next = self.out_pts[pp1].prev;
            self.out_pts[pp1].prev = pp2;
            pp1 = pp2;
            if pp1 == pp {
                break;
            }
        }
    }

    fn point_in_polygon_op(&self, pt: IntPoint, op: usize) -> i32 {
        // returns 0 if false, +1 if true, -1 if pt ON polygon boundary
        let mut result = 0;
        let start_op = op;
        let mut op = op;
        loop {
            let o = self.op(op);
            let n = self.op(o.next);
            if n.pt.y == pt.y {
                if n.pt.x == pt.x || (o.pt.y == pt.y && ((n.pt.x > pt.x) == (o.pt.x < pt.x))) {
                    return -1;
                }
            }
            if (o.pt.y < pt.y) != (n.pt.y < pt.y) {
                if o.pt.x >= pt.x {
                    if n.pt.x > pt.x {
                        result = 1 - result;
                    } else {
                        let d = (o.pt.x - pt.x) as f64 * (n.pt.y - pt.y) as f64 - (n.pt.x - pt.x) as f64 * (o.pt.y - pt.y) as f64;
                        if d == 0.0 {
                            return -1;
                        }
                        if (d > 0.0) == (n.pt.y > o.pt.y) {
                            result = 1 - result;
                        }
                    }
                } else {
                    if n.pt.x > pt.x {
                        let d = (o.pt.x - pt.x) as f64 * (n.pt.y - pt.y) as f64 - (n.pt.x - pt.x) as f64 * (o.pt.y - pt.y) as f64;
                        if d == 0.0 {
                            return -1;
                        }
                        if (d > 0.0) == (n.pt.y > o.pt.y) {
                            result = 1 - result;
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

    fn poly2_contains_poly1(&self, out_pt1: usize, out_pt2: usize) -> bool {
        let mut op = out_pt1;
        loop {
            // nb: PointInPolygon returns 0 if false, +1 if true, -1 if pt on polygon
            let res = self.point_in_polygon_op(self.op(op).pt, out_pt2);
            if res >= 0 {
                return res > 0;
            }
            op = self.op(op).next;
            if op == out_pt1 {
                break;
            }
        }
        true
    }

    fn first_is_bottom_pt(&self, btm_pt1: usize, btm_pt2: usize) -> bool {
        let b1 = self.op(btm_pt1).pt;
        let mut p = self.op(btm_pt1).prev;
        while self.op(p).pt == b1 && p != btm_pt1 {
            p = self.op(p).prev;
        }
        let dx1p = get_dx(b1, self.op(p).pt).abs();
        p = self.op(btm_pt1).next;
        while self.op(p).pt == b1 && p != btm_pt1 {
            p = self.op(p).next;
        }
        let dx1n = get_dx(b1, self.op(p).pt).abs();

        let b2 = self.op(btm_pt2).pt;
        p = self.op(btm_pt2).prev;
        while self.op(p).pt == b2 && p != btm_pt2 {
            p = self.op(p).prev;
        }
        let dx2p = get_dx(b2, self.op(p).pt).abs();
        p = self.op(btm_pt2).next;
        while self.op(p).pt == b2 && p != btm_pt2 {
            p = self.op(p).next;
        }
        let dx2n = get_dx(b2, self.op(p).pt).abs();

        if dx1p.max(dx1n) == dx2p.max(dx2n) && dx1p.min(dx1n) == dx2p.min(dx2n) {
            self.area_op(btm_pt1) > 0.0 // if otherwise identical use orientation
        } else {
            (dx1p >= dx2p && dx1p >= dx2n) || (dx1n >= dx2p && dx1n >= dx2n)
        }
    }

    fn get_bottom_pt(&self, pp: usize) -> usize {
        let mut pp = pp;
        let mut dups = NIL;
        let mut p = self.op(pp).next;
        while p != pp {
            if self.op(p).pt.y > self.op(pp).pt.y {
                pp = p;
                dups = NIL;
            } else if self.op(p).pt.y == self.op(pp).pt.y && self.op(p).pt.x <= self.op(pp).pt.x {
                if self.op(p).pt.x < self.op(pp).pt.x {
                    dups = NIL;
                    pp = p;
                } else {
                    if self.op(p).next != pp && self.op(p).prev != pp {
                        dups = p;
                    }
                }
            }
            p = self.op(p).next;
        }
        if dups != NIL {
            // there appears to be at least 2 vertices at BottomPt so ...
            while dups != p {
                if !self.first_is_bottom_pt(p, dups) {
                    pp = dups;
                }
                dups = self.op(dups).next;
                while self.op(dups).pt != self.op(pp).pt {
                    dups = self.op(dups).next;
                }
            }
        }
        pp
    }

    fn get_lowermost_rec(&mut self, out_rec1: usize, out_rec2: usize) -> usize {
        // work out which polygon fragment has the correct hole state ...
        if self.poly_outs[out_rec1].bottom_pt == NIL {
            let b = self.get_bottom_pt(self.poly_outs[out_rec1].pts);
            self.poly_outs[out_rec1].bottom_pt = b;
        }
        if self.poly_outs[out_rec2].bottom_pt == NIL {
            let b = self.get_bottom_pt(self.poly_outs[out_rec2].pts);
            self.poly_outs[out_rec2].bottom_pt = b;
        }
        let out_pt1 = self.poly_outs[out_rec1].bottom_pt;
        let out_pt2 = self.poly_outs[out_rec2].bottom_pt;
        let p1 = self.op(out_pt1).pt;
        let p2 = self.op(out_pt2).pt;
        if p1.y > p2.y {
            out_rec1
        } else if p1.y < p2.y {
            out_rec2
        } else if p1.x < p2.x {
            out_rec1
        } else if p1.x > p2.x {
            out_rec2
        } else if self.op(out_pt1).next == out_pt1 {
            out_rec2
        } else if self.op(out_pt2).next == out_pt2 {
            out_rec1
        } else if self.first_is_bottom_pt(out_pt1, out_pt2) {
            out_rec1
        } else {
            out_rec2
        }
    }

    fn out_rec1_right_of_out_rec2(&self, mut out_rec1: usize, out_rec2: usize) -> bool {
        loop {
            out_rec1 = self.poly_outs[out_rec1].first_left;
            if out_rec1 == out_rec2 {
                return true;
            }
            if out_rec1 == NIL {
                break;
            }
        }
        false
    }

    fn get_out_rec(&self, idx: i32) -> usize {
        let mut outrec = idx as usize;
        while outrec != self.poly_outs[outrec].idx as usize {
            outrec = self.poly_outs[outrec].idx as usize;
        }
        outrec
    }

    fn append_polygon(&mut self, e1: usize, e2: usize) {
        // get the start and ends of both output polygons ...
        let out_rec1 = self.e(e1).out_idx as usize;
        let out_rec2 = self.e(e2).out_idx as usize;

        let hole_state_rec = if self.out_rec1_right_of_out_rec2(out_rec1, out_rec2) {
            out_rec2
        } else if self.out_rec1_right_of_out_rec2(out_rec2, out_rec1) {
            out_rec1
        } else {
            self.get_lowermost_rec(out_rec1, out_rec2)
        };

        // get the start and ends of both output polygons and
        // join e2 poly onto e1 poly and delete pointers to e2 ...
        let p1_lft = self.poly_outs[out_rec1].pts;
        let p1_rt = self.op(p1_lft).prev;
        let p2_lft = self.poly_outs[out_rec2].pts;
        let p2_rt = self.op(p2_lft).prev;

        // join e2 poly onto e1 poly and delete pointers to e2 ...
        if self.e(e1).side == ES_LEFT {
            if self.e(e2).side == ES_LEFT {
                // z y x a b c
                self.reverse_poly_pt_links(p2_lft);
                self.out_pts[p2_lft].next = p1_lft;
                self.out_pts[p1_lft].prev = p2_lft;
                self.out_pts[p1_rt].next = p2_rt;
                self.out_pts[p2_rt].prev = p1_rt;
                self.poly_outs[out_rec1].pts = p2_rt;
            } else {
                // x y z a b c
                self.out_pts[p2_rt].next = p1_lft;
                self.out_pts[p1_lft].prev = p2_rt;
                self.out_pts[p2_lft].prev = p1_rt;
                self.out_pts[p1_rt].next = p2_lft;
                self.poly_outs[out_rec1].pts = p2_lft;
            }
        } else {
            if self.e(e2).side == ES_RIGHT {
                // a b c z y x
                self.reverse_poly_pt_links(p2_lft);
                self.out_pts[p1_rt].next = p2_rt;
                self.out_pts[p2_rt].prev = p1_rt;
                self.out_pts[p2_lft].next = p1_lft;
                self.out_pts[p1_lft].prev = p2_lft;
            } else {
                // a b c x y z
                self.out_pts[p1_rt].next = p2_lft;
                self.out_pts[p2_lft].prev = p1_rt;
                self.out_pts[p1_lft].prev = p2_rt;
                self.out_pts[p2_rt].next = p1_lft;
            }
        }

        self.poly_outs[out_rec1].bottom_pt = NIL;
        if hole_state_rec == out_rec2 {
            if self.poly_outs[out_rec2].first_left != out_rec1 {
                self.poly_outs[out_rec1].first_left = self.poly_outs[out_rec2].first_left;
            }
            self.poly_outs[out_rec1].is_hole = self.poly_outs[out_rec2].is_hole;
        }
        self.poly_outs[out_rec2].pts = NIL;
        self.poly_outs[out_rec2].bottom_pt = NIL;
        self.poly_outs[out_rec2].first_left = out_rec1;

        let ok_idx = self.e(e1).out_idx;
        let obsolete_idx = self.e(e2).out_idx;

        self.em(e1).out_idx = UNASSIGNED; // nb: safe because we only get here via AddLocalMaxPoly
        self.em(e2).out_idx = UNASSIGNED;

        let mut e = self.active_edges;
        let e1_side = self.e(e1).side;
        while e != NIL {
            if self.e(e).out_idx == obsolete_idx {
                self.em(e).out_idx = ok_idx;
                self.em(e).side = e1_side;
                break;
            }
            e = self.e(e).next_in_ael;
        }

        self.poly_outs[out_rec2].idx = self.poly_outs[out_rec1].idx;
    }

    fn add_out_pt(&mut self, e: usize, pt: IntPoint) -> usize {
        if self.e(e).out_idx < 0 {
            let out_rec = self.create_out_rec();
            let is_open = self.e(e).wind_delta == 0;
            self.poly_outs[out_rec].is_open = is_open;
            let idx = self.poly_outs[out_rec].idx;
            let new_op = self.new_out_pt(idx, pt);
            self.poly_outs[out_rec].pts = new_op;
            self.out_pts[new_op].next = new_op;
            self.out_pts[new_op].prev = new_op;
            if !is_open {
                self.set_hole_state(e, out_rec);
            }
            self.em(e).out_idx = idx;
            new_op
        } else {
            let out_rec = self.e(e).out_idx as usize;
            // OutRec.Pts is the 'Left-most' point & OutRec.Pts.Prev is the 'Right-most'
            let op = self.poly_outs[out_rec].pts;

            let to_front = self.e(e).side == ES_LEFT;
            if to_front && pt == self.op(op).pt {
                return op;
            } else if !to_front && pt == self.op(self.op(op).prev).pt {
                return self.op(op).prev;
            }

            let idx = self.poly_outs[out_rec].idx;
            let new_op = self.new_out_pt(idx, pt);
            let op_prev = self.op(op).prev;
            self.out_pts[new_op].next = op;
            self.out_pts[new_op].prev = op_prev;
            self.out_pts[op_prev].next = new_op;
            self.out_pts[op].prev = new_op;
            if to_front {
                self.poly_outs[out_rec].pts = new_op;
            }
            new_op
        }
    }

    fn get_last_out_pt(&self, e: usize) -> usize {
        let out_rec = self.e(e).out_idx as usize;
        if self.e(e).side == ES_LEFT {
            self.poly_outs[out_rec].pts
        } else {
            self.op(self.poly_outs[out_rec].pts).prev
        }
    }

    fn process_horizontals(&mut self) -> Result<(), ClipperError> {
        while let Some(horz_edge) = self.pop_edge_from_sel() {
            self.process_horizontal(horz_edge)?;
        }
        Ok(())
    }

    #[inline]
    fn is_maxima(&self, e: usize, y: CInt) -> bool {
        e != NIL && self.e(e).top.y == y && self.e(e).next_in_lml == NIL
    }

    #[inline]
    fn is_intermediate(&self, e: usize, y: CInt) -> bool {
        self.e(e).top.y == y && self.e(e).next_in_lml != NIL
    }

    fn get_maxima_pair(&self, e: usize) -> usize {
        let ed = self.e(e);
        if self.e(ed.next).top == ed.top && self.e(ed.next).next_in_lml == NIL {
            ed.next
        } else if self.e(ed.prev).top == ed.top && self.e(ed.prev).next_in_lml == NIL {
            ed.prev
        } else {
            NIL
        }
    }

    fn get_maxima_pair_ex(&self, e: usize) -> usize {
        // as GetMaximaPair() but returns 0 if MaxPair isn't in AEL (unless it's horizontal)
        let result = self.get_maxima_pair(e);
        if result != NIL
            && (self.e(result).out_idx == SKIP
                || (self.e(result).next_in_ael == self.e(result).prev_in_ael && !is_horizontal(self.e(result))))
        {
            return NIL;
        }
        result
    }

    fn swap_positions_in_sel(&mut self, edge1: usize, edge2: usize) {
        if self.e(edge1).next_in_sel == NIL && self.e(edge1).prev_in_sel == NIL {
            return;
        }
        if self.e(edge2).next_in_sel == NIL && self.e(edge2).prev_in_sel == NIL {
            return;
        }

        if self.e(edge1).next_in_sel == edge2 {
            let next = self.e(edge2).next_in_sel;
            if next != NIL {
                self.em(next).prev_in_sel = edge1;
            }
            let prev = self.e(edge1).prev_in_sel;
            if prev != NIL {
                self.em(prev).next_in_sel = edge2;
            }
            self.em(edge2).prev_in_sel = prev;
            self.em(edge2).next_in_sel = edge1;
            self.em(edge1).prev_in_sel = edge2;
            self.em(edge1).next_in_sel = next;
        } else if self.e(edge2).next_in_sel == edge1 {
            let next = self.e(edge1).next_in_sel;
            if next != NIL {
                self.em(next).prev_in_sel = edge2;
            }
            let prev = self.e(edge2).prev_in_sel;
            if prev != NIL {
                self.em(prev).next_in_sel = edge1;
            }
            self.em(edge1).prev_in_sel = prev;
            self.em(edge1).next_in_sel = edge2;
            self.em(edge2).prev_in_sel = edge1;
            self.em(edge2).next_in_sel = next;
        } else {
            let next = self.e(edge1).next_in_sel;
            let prev = self.e(edge1).prev_in_sel;
            let e2n = self.e(edge2).next_in_sel;
            self.em(edge1).next_in_sel = e2n;
            if e2n != NIL {
                self.em(e2n).prev_in_sel = edge1;
            }
            let e2p = self.e(edge2).prev_in_sel;
            self.em(edge1).prev_in_sel = e2p;
            if e2p != NIL {
                self.em(e2p).next_in_sel = edge1;
            }
            self.em(edge2).next_in_sel = next;
            if next != NIL {
                self.em(next).prev_in_sel = edge2;
            }
            self.em(edge2).prev_in_sel = prev;
            if prev != NIL {
                self.em(prev).next_in_sel = edge2;
            }
        }

        if self.e(edge1).prev_in_sel == NIL {
            self.sorted_edges = edge1;
        } else if self.e(edge2).prev_in_sel == NIL {
            self.sorted_edges = edge2;
        }
    }

    #[inline]
    fn get_next_in_ael(&self, e: usize, dir: Direction) -> usize {
        if dir == Direction::LeftToRight {
            self.e(e).next_in_ael
        } else {
            self.e(e).prev_in_ael
        }
    }

    #[inline]
    fn get_horz_direction(&self, horz_edge: usize) -> (Direction, CInt, CInt) {
        let h = self.e(horz_edge);
        if h.bot.x < h.top.x {
            (Direction::LeftToRight, h.bot.x, h.top.x)
        } else {
            (Direction::RightToLeft, h.top.x, h.bot.x)
        }
    }

    /*******************************************************************************
     * Notes: Horizontal edges (HEs) at scanline intersections (ie at the Top or    *
     * Bottom of a scanbeam) are processed as if layered. The order in which HEs    *
     * are processed doesn't matter. HEs intersect with other HE Bot.Xs only [#]    *
     * (or they could intersect with Top.Xs only, ie EITHER Bot.Xs OR Top.Xs),      *
     * and with other non-horizontal edges [*]. Once these intersections are        *
     * processed, intermediate HEs then 'promote' the Edge above (NextInLML) into   *
     * the AEL. These 'promoted' edges may in turn intersect [%] with other HEs.    *
     *******************************************************************************/
    fn process_horizontal(&mut self, horz_edge_in: usize) -> Result<(), ClipperError> {
        let mut horz_edge = horz_edge_in;
        let is_open = self.e(horz_edge).wind_delta == 0;

        let (mut dir, mut horz_left, mut horz_right) = self.get_horz_direction(horz_edge);

        let mut e_last_horz = horz_edge;
        let mut e_max_pair = NIL;
        while self.e(e_last_horz).next_in_lml != NIL && is_horizontal(self.e(self.e(e_last_horz).next_in_lml)) {
            e_last_horz = self.e(e_last_horz).next_in_lml;
        }
        if self.e(e_last_horz).next_in_lml == NIL {
            e_max_pair = self.get_maxima_pair(e_last_horz);
        }

        // Iterators into m_Maxima (a sorted list). max_it: forward index; max_rit: number of
        // elements remaining when iterating backwards (element = maxima[max_rit - 1]).
        let maxima_len = self.maxima.len();
        let mut max_it = maxima_len; // "end"
        let mut max_rit = 0usize; // "rend"
        if maxima_len > 0 {
            // get the first maxima in range (X) ...
            if dir == Direction::LeftToRight {
                max_it = 0;
                while max_it != maxima_len && self.maxima[max_it] <= self.e(horz_edge).bot.x {
                    max_it += 1;
                }
                if max_it != maxima_len && self.maxima[max_it] >= self.e(e_last_horz).top.x {
                    max_it = maxima_len;
                }
            } else {
                max_rit = maxima_len;
                while max_rit != 0 && self.maxima[max_rit - 1] > self.e(horz_edge).bot.x {
                    max_rit -= 1;
                }
                if max_rit != 0 && self.maxima[max_rit - 1] <= self.e(e_last_horz).top.x {
                    max_rit = 0;
                }
            }
        }

        let mut op1 = NIL;

        loop {
            // loop through consec. horizontal edges
            let is_last_horz = horz_edge == e_last_horz;
            let mut e = self.get_next_in_ael(horz_edge, dir);
            while e != NIL {
                // this code block inserts extra coords into horizontal edges (in output
                // polygons) whereever maxima touch these horizontal edges. This helps
                // 'simplifying' polygons (ie if the Simplify property is set).
                if !self.maxima.is_empty() {
                    if dir == Direction::LeftToRight {
                        while max_it != maxima_len && self.maxima[max_it] < self.e(e).curr.x {
                            if self.e(horz_edge).out_idx >= 0 && !is_open {
                                let p = IntPoint::new(self.maxima[max_it], self.e(horz_edge).bot.y);
                                self.add_out_pt(horz_edge, p);
                            }
                            max_it += 1;
                        }
                    } else {
                        while max_rit != 0 && self.maxima[max_rit - 1] > self.e(e).curr.x {
                            if self.e(horz_edge).out_idx >= 0 && !is_open {
                                let p = IntPoint::new(self.maxima[max_rit - 1], self.e(horz_edge).bot.y);
                                self.add_out_pt(horz_edge, p);
                            }
                            max_rit -= 1;
                        }
                    }
                }

                if (dir == Direction::LeftToRight && self.e(e).curr.x > horz_right)
                    || (dir == Direction::RightToLeft && self.e(e).curr.x < horz_left)
                {
                    break;
                }

                // Also break if we've got to the end of an intermediate horizontal edge ...
                // nb: Smaller Dx's are to the right of larger Dx's ABOVE the horizontal.
                let hn = self.e(horz_edge).next_in_lml;
                if self.e(e).curr.x == self.e(horz_edge).top.x && hn != NIL && self.e(e).dx < self.e(hn).dx {
                    break;
                }

                if self.e(horz_edge).out_idx >= 0 && !is_open {
                    // note: may be done multiple times
                    let c = self.e(e).curr;
                    op1 = self.add_out_pt(horz_edge, c);
                    let mut e_next_horz = self.sorted_edges;
                    while e_next_horz != NIL {
                        if self.e(e_next_horz).out_idx >= 0
                            && horz_segments_overlap(
                                self.e(horz_edge).bot.x,
                                self.e(horz_edge).top.x,
                                self.e(e_next_horz).bot.x,
                                self.e(e_next_horz).top.x,
                            )
                        {
                            let op2 = self.get_last_out_pt(e_next_horz);
                            let t = self.e(e_next_horz).top;
                            self.add_join(op2, op1, t);
                        }
                        e_next_horz = self.e(e_next_horz).next_in_sel;
                    }
                    let b = self.e(horz_edge).bot;
                    self.add_ghost_join(op1, b);
                }

                // OK, so far we're still in range of the horizontal Edge  but make sure
                // we're at the last of consec. horizontals when matching with eMaxPair
                if e == e_max_pair && is_last_horz {
                    if self.e(horz_edge).out_idx >= 0 {
                        let t = self.e(horz_edge).top;
                        self.add_local_max_poly(horz_edge, e_max_pair, t);
                    }
                    self.delete_from_ael(horz_edge);
                    self.delete_from_ael(e_max_pair);
                    return Ok(());
                }

                if dir == Direction::LeftToRight {
                    let pt = IntPoint::new(self.e(e).curr.x, self.e(horz_edge).curr.y);
                    self.intersect_edges(horz_edge, e, pt);
                } else {
                    let pt = IntPoint::new(self.e(e).curr.x, self.e(horz_edge).curr.y);
                    self.intersect_edges(e, horz_edge, pt);
                }
                let e_next = self.get_next_in_ael(e, dir);
                self.swap_positions_in_ael(horz_edge, e);
                e = e_next;
            } // end while(e)

            // Break out of loop if HorzEdge.NextInLML is not also horizontal ...
            let hn = self.e(horz_edge).next_in_lml;
            if hn == NIL || !is_horizontal(self.e(hn)) {
                break;
            }

            self.update_edge_into_ael(&mut horz_edge)?;
            if self.e(horz_edge).out_idx >= 0 {
                let b = self.e(horz_edge).bot;
                self.add_out_pt(horz_edge, b);
            }
            let (d, l, r) = self.get_horz_direction(horz_edge);
            dir = d;
            horz_left = l;
            horz_right = r;
        } // end for (;;)

        if self.e(horz_edge).out_idx >= 0 && op1 == NIL {
            op1 = self.get_last_out_pt(horz_edge);
            let mut e_next_horz = self.sorted_edges;
            while e_next_horz != NIL {
                if self.e(e_next_horz).out_idx >= 0
                    && horz_segments_overlap(
                        self.e(horz_edge).bot.x,
                        self.e(horz_edge).top.x,
                        self.e(e_next_horz).bot.x,
                        self.e(e_next_horz).top.x,
                    )
                {
                    let op2 = self.get_last_out_pt(e_next_horz);
                    let t = self.e(e_next_horz).top;
                    self.add_join(op2, op1, t);
                }
                e_next_horz = self.e(e_next_horz).next_in_sel;
            }
            let t = self.e(horz_edge).top;
            self.add_ghost_join(op1, t);
        }

        if self.e(horz_edge).next_in_lml != NIL {
            if self.e(horz_edge).out_idx >= 0 {
                let t = self.e(horz_edge).top;
                op1 = self.add_out_pt(horz_edge, t);
                self.update_edge_into_ael(&mut horz_edge)?;
                if self.e(horz_edge).wind_delta == 0 {
                    return Ok(());
                }
                // nb: HorzEdge is no longer horizontal here
                let e_prev = self.e(horz_edge).prev_in_ael;
                let e_next = self.e(horz_edge).next_in_ael;
                if e_prev != NIL
                    && self.e(e_prev).curr.x == self.e(horz_edge).bot.x
                    && self.e(e_prev).curr.y == self.e(horz_edge).bot.y
                    && self.e(e_prev).wind_delta != 0
                    && (self.e(e_prev).out_idx >= 0
                        && self.e(e_prev).curr.y > self.e(e_prev).top.y
                        && slopes_equal_edges(self.e(horz_edge), self.e(e_prev)))
                {
                    let b = self.e(horz_edge).bot;
                    let op2 = self.add_out_pt(e_prev, b);
                    let t = self.e(horz_edge).top;
                    self.add_join(op1, op2, t);
                } else if e_next != NIL
                    && self.e(e_next).curr.x == self.e(horz_edge).bot.x
                    && self.e(e_next).curr.y == self.e(horz_edge).bot.y
                    && self.e(e_next).wind_delta != 0
                    && self.e(e_next).out_idx >= 0
                    && self.e(e_next).curr.y > self.e(e_next).top.y
                    && slopes_equal_edges(self.e(horz_edge), self.e(e_next))
                {
                    let b = self.e(horz_edge).bot;
                    let op2 = self.add_out_pt(e_next, b);
                    let t = self.e(horz_edge).top;
                    self.add_join(op1, op2, t);
                }
            } else {
                self.update_edge_into_ael(&mut horz_edge)?;
            }
        } else {
            if self.e(horz_edge).out_idx >= 0 {
                let t = self.e(horz_edge).top;
                self.add_out_pt(horz_edge, t);
            }
            self.delete_from_ael(horz_edge);
        }
        Ok(())
    }

    fn process_intersections(&mut self, top_y: CInt) -> Result<bool, ClipperError> {
        if self.active_edges == NIL {
            return Ok(true);
        }
        self.build_intersect_list(top_y);
        let il_size = self.intersect_list.len();
        if il_size == 0 {
            return Ok(true);
        }
        if il_size == 1 || self.fixup_intersection_order() {
            self.process_intersect_list();
        } else {
            self.sorted_edges = NIL;
            self.intersect_list.clear();
            return Ok(false);
        }
        self.sorted_edges = NIL;
        Ok(true)
    }

    fn build_intersect_list(&mut self, top_y: CInt) {
        if self.active_edges == NIL {
            return;
        }

        // prepare for sorting ...
        let mut e = self.active_edges;
        self.sorted_edges = e;
        while e != NIL {
            let (p, n) = (self.e(e).prev_in_ael, self.e(e).next_in_ael);
            self.em(e).prev_in_sel = p;
            self.em(e).next_in_sel = n;
            let tx = top_x(self.e(e), top_y);
            self.em(e).curr.x = tx;
            e = n;
        }

        // bubblesort ...
        loop {
            let mut is_modified = false;
            e = self.sorted_edges;
            while self.e(e).next_in_sel != NIL {
                let e_next = self.e(e).next_in_sel;
                if self.e(e).curr.x > self.e(e_next).curr.x {
                    let mut pt = intersect_point(self.e(e), self.e(e_next));
                    if pt.y < top_y {
                        pt = IntPoint::new(top_x(self.e(e), top_y), top_y);
                    }
                    self.intersect_list.push(IntersectNode { edge1: e, edge2: e_next, pt });
                    self.swap_positions_in_sel(e, e_next);
                    is_modified = true;
                } else {
                    e = e_next;
                }
            }
            let p = self.e(e).prev_in_sel;
            if p != NIL {
                self.em(p).next_in_sel = NIL;
            } else {
                break;
            }
            if !is_modified {
                break;
            }
        }
        self.sorted_edges = NIL; // important
    }

    fn process_intersect_list(&mut self) {
        for i in 0..self.intersect_list.len() {
            let inode = self.intersect_list[i];
            self.intersect_edges(inode.edge1, inode.edge2, inode.pt);
            self.swap_positions_in_ael(inode.edge1, inode.edge2);
        }
        self.intersect_list.clear();
    }

    #[inline]
    fn edges_adjacent(&self, inode: &IntersectNode) -> bool {
        self.e(inode.edge1).next_in_sel == inode.edge2 || self.e(inode.edge1).prev_in_sel == inode.edge2
    }

    fn fixup_intersection_order(&mut self) -> bool {
        // pre-condition: intersections are sorted Bottom-most first.
        // Now it's crucial that intersections are made only between adjacent edges,
        // so to ensure this the order of intersections may need adjusting ...
        self.copy_ael_to_sel();
        cppstd::sort_by(&mut self.intersect_list, |n1, n2| n2.pt.y < n1.pt.y);
        let cnt = self.intersect_list.len();
        for i in 0..cnt {
            if !self.edges_adjacent(&self.intersect_list[i]) {
                let mut j = i + 1;
                while j < cnt && !self.edges_adjacent(&self.intersect_list[j]) {
                    j += 1;
                }
                if j == cnt {
                    return false;
                }
                self.intersect_list.swap(i, j);
            }
            let n = self.intersect_list[i];
            self.swap_positions_in_sel(n.edge1, n.edge2);
        }
        true
    }

    fn do_maxima(&mut self, e: usize) -> Result<(), ClipperError> {
        let e_max_pair = self.get_maxima_pair_ex(e);
        if e_max_pair == NIL {
            if self.e(e).out_idx >= 0 {
                let t = self.e(e).top;
                self.add_out_pt(e, t);
            }
            self.delete_from_ael(e);
            return Ok(());
        }

        let mut e_next = self.e(e).next_in_ael;
        while e_next != NIL && e_next != e_max_pair {
            let t = self.e(e).top;
            self.intersect_edges(e, e_next, t);
            self.swap_positions_in_ael(e, e_next);
            e_next = self.e(e).next_in_ael;
        }

        if self.e(e).out_idx == UNASSIGNED && self.e(e_max_pair).out_idx == UNASSIGNED {
            self.delete_from_ael(e);
            self.delete_from_ael(e_max_pair);
        } else if self.e(e).out_idx >= 0 && self.e(e_max_pair).out_idx >= 0 {
            if self.e(e).out_idx >= 0 {
                let t = self.e(e).top;
                self.add_local_max_poly(e, e_max_pair, t);
            }
            self.delete_from_ael(e);
            self.delete_from_ael(e_max_pair);
        } else if self.e(e).wind_delta == 0 {
            if self.e(e).out_idx >= 0 {
                let t = self.e(e).top;
                self.add_out_pt(e, t);
                self.em(e).out_idx = UNASSIGNED;
            }
            self.delete_from_ael(e);

            if self.e(e_max_pair).out_idx >= 0 {
                let t = self.e(e).top;
                self.add_out_pt(e_max_pair, t);
                self.em(e_max_pair).out_idx = UNASSIGNED;
            }
            self.delete_from_ael(e_max_pair);
        } else {
            return Err(ClipperError("DoMaxima error"));
        }
        Ok(())
    }

    fn process_edges_at_top_of_scanbeam(&mut self, top_y: CInt) -> Result<(), ClipperError> {
        let mut e = self.active_edges;
        while e != NIL {
            // 1. process maxima, treating them as if they're 'bent' horizontal edges,
            //    but exclude maxima with horizontal edges. nb: e can't be a horizontal.
            let mut is_maxima_edge = self.is_maxima(e, top_y);

            if is_maxima_edge {
                let e_max_pair = self.get_maxima_pair_ex(e);
                is_maxima_edge = e_max_pair == NIL || !is_horizontal(self.e(e_max_pair));
            }

            if is_maxima_edge {
                if self.strict_simple {
                    let tx = self.e(e).top.x;
                    self.maxima.push(tx);
                }
                let e_prev = self.e(e).prev_in_ael;
                self.do_maxima(e)?;
                if e_prev == NIL {
                    e = self.active_edges;
                } else {
                    e = self.e(e_prev).next_in_ael;
                }
            } else {
                // 2. promote horizontal edges, otherwise update Curr.X and Curr.Y ...
                if self.is_intermediate(e, top_y) && is_horizontal(self.e(self.e(e).next_in_lml)) {
                    self.update_edge_into_ael(&mut e)?;
                    if self.e(e).out_idx >= 0 {
                        let b = self.e(e).bot;
                        self.add_out_pt(e, b);
                    }
                    self.add_edge_to_sel(e);
                } else {
                    let tx = top_x(self.e(e), top_y);
                    self.em(e).curr.x = tx;
                    self.em(e).curr.y = top_y;
                }

                // When StrictlySimple and 'e' is being touched by another edge, then
                // make sure both edges have a vertex here ...
                if self.strict_simple {
                    let e_prev = self.e(e).prev_in_ael;
                    if self.e(e).out_idx >= 0
                        && self.e(e).wind_delta != 0
                        && e_prev != NIL
                        && self.e(e_prev).out_idx >= 0
                        && self.e(e_prev).curr.x == self.e(e).curr.x
                        && self.e(e_prev).wind_delta != 0
                    {
                        let pt = self.e(e).curr;
                        let op = self.add_out_pt(e_prev, pt);
                        let op2 = self.add_out_pt(e, pt);
                        self.add_join(op, op2, pt); // StrictlySimple (type-3) join
                    }
                }

                e = self.e(e).next_in_ael;
            }
        }

        // 3. Process horizontals at the Top of the scanbeam ...
        self.maxima.sort_unstable();
        self.process_horizontals()?;
        self.maxima.clear();

        // 4. Promote intermediate vertices ...
        e = self.active_edges;
        while e != NIL {
            if self.is_intermediate(e, top_y) {
                let mut op = NIL;
                if self.e(e).out_idx >= 0 {
                    let t = self.e(e).top;
                    op = self.add_out_pt(e, t);
                }
                self.update_edge_into_ael(&mut e)?;

                // if output polygons share an edge, they'll need joining later ...
                let e_prev = self.e(e).prev_in_ael;
                let e_next = self.e(e).next_in_ael;
                if e_prev != NIL
                    && self.e(e_prev).curr.x == self.e(e).bot.x
                    && self.e(e_prev).curr.y == self.e(e).bot.y
                    && op != NIL
                    && self.e(e_prev).out_idx >= 0
                    && self.e(e_prev).curr.y > self.e(e_prev).top.y
                    && slopes_equal4(self.e(e).curr, self.e(e).top, self.e(e_prev).curr, self.e(e_prev).top)
                    && self.e(e).wind_delta != 0
                    && self.e(e_prev).wind_delta != 0
                {
                    let b = self.e(e).bot;
                    let op2 = self.add_out_pt(e_prev, b);
                    let t = self.e(e).top;
                    self.add_join(op, op2, t);
                } else if e_next != NIL
                    && self.e(e_next).curr.x == self.e(e).bot.x
                    && self.e(e_next).curr.y == self.e(e).bot.y
                    && op != NIL
                    && self.e(e_next).out_idx >= 0
                    && self.e(e_next).curr.y > self.e(e_next).top.y
                    && slopes_equal4(self.e(e).curr, self.e(e).top, self.e(e_next).curr, self.e(e_next).top)
                    && self.e(e).wind_delta != 0
                    && self.e(e_next).wind_delta != 0
                {
                    let b = self.e(e).bot;
                    let op2 = self.add_out_pt(e_next, b);
                    let t = self.e(e).top;
                    self.add_join(op, op2, t);
                }
            }
            e = self.e(e).next_in_ael;
        }
        Ok(())
    }

    fn fixup_out_polyline(&mut self, outrec: usize) {
        let mut pp = self.poly_outs[outrec].pts;
        let mut last_pp = self.op(pp).prev;
        while pp != last_pp {
            pp = self.op(pp).next;
            if self.op(pp).pt == self.op(self.op(pp).prev).pt {
                if pp == last_pp {
                    last_pp = self.op(pp).prev;
                }
                let tmp_pp = self.op(pp).prev;
                let pn = self.op(pp).next;
                self.out_pts[tmp_pp].next = pn;
                self.out_pts[pn].prev = tmp_pp;
                pp = tmp_pp;
            }
        }

        if pp == self.op(pp).prev {
            self.poly_outs[outrec].pts = NIL;
        }
    }

    fn fixup_out_polygon(&mut self, outrec: usize) {
        // FixupOutPolygon() - removes duplicate points and simplifies consecutive
        // parallel edges by removing the middle vertex.
        let mut last_ok = NIL;
        self.poly_outs[outrec].bottom_pt = NIL;
        let mut pp = self.poly_outs[outrec].pts;
        let preserve_col = self.preserve_collinear || self.strict_simple;

        loop {
            if self.op(pp).prev == pp || self.op(pp).prev == self.op(pp).next {
                self.poly_outs[outrec].pts = NIL;
                return;
            }

            let prev = self.op(pp).prev;
            let next = self.op(pp).next;
            let (pt, ptp, ptn) = (self.op(pp).pt, self.op(prev).pt, self.op(next).pt);
            // test for duplicate points and collinear edges ...
            if pt == ptn || pt == ptp || (slopes_equal3(ptp, pt, ptn) && (!preserve_col || !pt2_is_between_pt1_and_pt3(ptp, pt, ptn))) {
                last_ok = NIL;
                self.out_pts[prev].next = next;
                self.out_pts[next].prev = prev;
                pp = prev;
            } else if pp == last_ok {
                break;
            } else {
                if last_ok == NIL {
                    last_ok = pp;
                }
                pp = self.op(pp).next;
            }
        }
        self.poly_outs[outrec].pts = pp;
    }

    fn point_count(&self, pts: usize) -> usize {
        if pts == NIL {
            return 0;
        }
        let mut result = 0;
        let mut p = pts;
        loop {
            result += 1;
            p = self.op(p).next;
            if p == pts {
                break;
            }
        }
        result
    }

    fn build_result(&self, polys: &mut Paths) {
        polys.reserve(self.poly_outs.len());
        for rec in &self.poly_outs {
            if rec.pts == NIL {
                continue;
            }
            let mut p = self.op(rec.pts).prev;
            let cnt = self.point_count(p);
            if cnt < 2 {
                continue;
            }
            let mut pg = Path::with_capacity(cnt);
            for _ in 0..cnt {
                pg.push(self.op(p).pt);
                p = self.op(p).prev;
            }
            polys.push(pg);
        }
    }

    fn build_result2(&mut self, polytree: &mut PolyTree) {
        polytree.clear();
        polytree.all_nodes.reserve(self.poly_outs.len());
        // add each output polygon/contour to polytree ...
        for i in 0..self.poly_outs.len() {
            let out_rec = self.poly_outs[i];
            let cnt = self.point_count(out_rec.pts);
            if (out_rec.is_open && cnt < 2) || (!out_rec.is_open && cnt < 3) {
                continue;
            }
            self.fix_hole_linkage(i);
            let pn = polytree.all_nodes.len();
            let mut node = PolyNode::default();
            node.contour.reserve(cnt);
            let mut op = self.op(out_rec.pts).prev;
            for _ in 0..cnt {
                node.contour.push(self.op(op).pt);
                op = self.op(op).prev;
            }
            polytree.all_nodes.push(node);
            self.poly_outs[i].poly_nd = pn;
        }

        // fixup PolyNode links etc ...
        polytree.childs.reserve(self.poly_outs.len());
        for i in 0..self.poly_outs.len() {
            let out_rec = self.poly_outs[i];
            if out_rec.poly_nd == NIL {
                continue;
            }
            if out_rec.is_open {
                polytree.all_nodes[out_rec.poly_nd].is_open = true;
                polytree.add_child_to_root(out_rec.poly_nd);
            } else if out_rec.first_left != NIL && self.poly_outs[out_rec.first_left].poly_nd != NIL {
                let parent = self.poly_outs[out_rec.first_left].poly_nd;
                polytree.add_child(parent, out_rec.poly_nd);
            } else {
                polytree.add_child_to_root(out_rec.poly_nd);
            }
        }
    }

    fn insert_edge_into_ael(&mut self, edge: usize, start_edge: usize) {
        let mut start_edge = start_edge;
        if self.active_edges == NIL {
            self.em(edge).prev_in_ael = NIL;
            self.em(edge).next_in_ael = NIL;
            self.active_edges = edge;
        } else if start_edge == NIL && self.e2_inserts_before_e1(self.active_edges, edge) {
            let ae = self.active_edges;
            self.em(edge).prev_in_ael = NIL;
            self.em(edge).next_in_ael = ae;
            self.em(ae).prev_in_ael = edge;
            self.active_edges = edge;
        } else {
            if start_edge == NIL {
                start_edge = self.active_edges;
            }
            while self.e(start_edge).next_in_ael != NIL && !self.e2_inserts_before_e1(self.e(start_edge).next_in_ael, edge) {
                start_edge = self.e(start_edge).next_in_ael;
            }
            let sn = self.e(start_edge).next_in_ael;
            self.em(edge).next_in_ael = sn;
            if sn != NIL {
                self.em(sn).prev_in_ael = edge;
            }
            self.em(edge).prev_in_ael = start_edge;
            self.em(start_edge).next_in_ael = edge;
        }
    }

    #[inline]
    fn e2_inserts_before_e1(&self, e1: usize, e2: usize) -> bool {
        let e1 = self.e(e1);
        let e2 = self.e(e2);
        if e2.curr.x == e1.curr.x {
            if e2.top.y > e1.top.y {
                e2.top.x < top_x(e1, e2.top.y)
            } else {
                e1.top.x > top_x(e2, e1.top.y)
            }
        } else {
            e2.curr.x < e1.curr.x
        }
    }

    fn dup_out_pt(&mut self, out_pt: usize, insert_after: bool) -> usize {
        let (pt, idx) = (self.op(out_pt).pt, self.op(out_pt).idx);
        let result = self.new_out_pt(idx, pt);
        if insert_after {
            let n = self.op(out_pt).next;
            self.out_pts[result].next = n;
            self.out_pts[result].prev = out_pt;
            self.out_pts[n].prev = result;
            self.out_pts[out_pt].next = result;
        } else {
            let p = self.op(out_pt).prev;
            self.out_pts[result].prev = p;
            self.out_pts[result].next = out_pt;
            self.out_pts[p].next = result;
            self.out_pts[out_pt].prev = result;
        }
        result
    }

    fn join_horz(&mut self, mut op1: usize, mut op1b: usize, mut op2: usize, mut op2b: usize, pt: IntPoint, discard_left: bool) -> bool {
        let dir1 = if self.op(op1).pt.x > self.op(op1b).pt.x { Direction::RightToLeft } else { Direction::LeftToRight };
        let dir2 = if self.op(op2).pt.x > self.op(op2b).pt.x { Direction::RightToLeft } else { Direction::LeftToRight };
        if dir1 == dir2 {
            return false;
        }

        // When DiscardLeft, we want Op1b to be on the Left of Op1, otherwise we
        // want Op1b to be on the Right. (And likewise with Op2 and Op2b.)
        // So, to facilitate this while inserting Op1b and Op2b ...
        // when DiscardLeft, make sure we're AT or RIGHT of Pt before adding Op1b,
        // otherwise make sure we're AT or LEFT of Pt. (Likewise with Op2b.)
        if dir1 == Direction::LeftToRight {
            while self.op(self.op(op1).next).pt.x <= pt.x
                && self.op(self.op(op1).next).pt.x >= self.op(op1).pt.x
                && self.op(self.op(op1).next).pt.y == pt.y
            {
                op1 = self.op(op1).next;
            }
            if discard_left && self.op(op1).pt.x != pt.x {
                op1 = self.op(op1).next;
            }
            op1b = self.dup_out_pt(op1, !discard_left);
            if self.op(op1b).pt != pt {
                op1 = op1b;
                self.out_pts[op1].pt = pt;
                op1b = self.dup_out_pt(op1, !discard_left);
            }
        } else {
            while self.op(self.op(op1).next).pt.x >= pt.x
                && self.op(self.op(op1).next).pt.x <= self.op(op1).pt.x
                && self.op(self.op(op1).next).pt.y == pt.y
            {
                op1 = self.op(op1).next;
            }
            if !discard_left && self.op(op1).pt.x != pt.x {
                op1 = self.op(op1).next;
            }
            op1b = self.dup_out_pt(op1, discard_left);
            if self.op(op1b).pt != pt {
                op1 = op1b;
                self.out_pts[op1].pt = pt;
                op1b = self.dup_out_pt(op1, discard_left);
            }
        }

        if dir2 == Direction::LeftToRight {
            while self.op(self.op(op2).next).pt.x <= pt.x
                && self.op(self.op(op2).next).pt.x >= self.op(op2).pt.x
                && self.op(self.op(op2).next).pt.y == pt.y
            {
                op2 = self.op(op2).next;
            }
            if discard_left && self.op(op2).pt.x != pt.x {
                op2 = self.op(op2).next;
            }
            op2b = self.dup_out_pt(op2, !discard_left);
            if self.op(op2b).pt != pt {
                op2 = op2b;
                self.out_pts[op2].pt = pt;
                op2b = self.dup_out_pt(op2, !discard_left);
            }
        } else {
            while self.op(self.op(op2).next).pt.x >= pt.x
                && self.op(self.op(op2).next).pt.x <= self.op(op2).pt.x
                && self.op(self.op(op2).next).pt.y == pt.y
            {
                op2 = self.op(op2).next;
            }
            if !discard_left && self.op(op2).pt.x != pt.x {
                op2 = self.op(op2).next;
            }
            op2b = self.dup_out_pt(op2, discard_left);
            if self.op(op2b).pt != pt {
                op2 = op2b;
                self.out_pts[op2].pt = pt;
                op2b = self.dup_out_pt(op2, discard_left);
            }
        }

        if (dir1 == Direction::LeftToRight) == discard_left {
            self.out_pts[op1].prev = op2;
            self.out_pts[op2].next = op1;
            self.out_pts[op1b].next = op2b;
            self.out_pts[op2b].prev = op1b;
        } else {
            self.out_pts[op1].next = op2;
            self.out_pts[op2].prev = op1;
            self.out_pts[op1b].prev = op2b;
            self.out_pts[op2b].next = op1b;
        }
        true
    }

    fn join_points(&mut self, j: usize, out_rec1: usize, out_rec2: usize) -> bool {
        let mut op1 = self.joins[j].out_pt1;
        let mut op1b;
        let mut op2 = self.joins[j].out_pt2;
        let mut op2b;
        let off_pt = self.joins[j].off_pt;

        // There are 3 kinds of joins for output polygons ...
        // 1. Horizontal joins where Join.OutPt1 & Join.OutPt2 are vertices anywhere
        // along (horizontal) collinear edges (& Join.OffPt is on the same horizontal).
        // 2. Non-horizontal joins where Join.OutPt1 & Join.OutPt2 are at the same
        // location at the Bottom of the overlapping segment (& Join.OffPt is above).
        // 3. StrictSimple joins where edges touch but are not collinear and where
        // Join.OutPt1, Join.OutPt2 & Join.OffPt all share the same point.
        let is_horizontal = self.op(op1).pt.y == off_pt.y;

        if is_horizontal && off_pt == self.op(op1).pt && off_pt == self.op(op2).pt {
            // Strictly Simple join ...
            if out_rec1 != out_rec2 {
                return false;
            }
            op1b = self.op(op1).next;
            while op1b != op1 && self.op(op1b).pt == off_pt {
                op1b = self.op(op1b).next;
            }
            let reverse1 = self.op(op1b).pt.y > off_pt.y;
            op2b = self.op(op2).next;
            while op2b != op2 && self.op(op2b).pt == off_pt {
                op2b = self.op(op2b).next;
            }
            let reverse2 = self.op(op2b).pt.y > off_pt.y;
            if reverse1 == reverse2 {
                return false;
            }
            if reverse1 {
                op1b = self.dup_out_pt(op1, false);
                op2b = self.dup_out_pt(op2, true);
                self.out_pts[op1].prev = op2;
                self.out_pts[op2].next = op1;
                self.out_pts[op1b].next = op2b;
                self.out_pts[op2b].prev = op1b;
                self.joins[j].out_pt1 = op1;
                self.joins[j].out_pt2 = op1b;
                true
            } else {
                op1b = self.dup_out_pt(op1, true);
                op2b = self.dup_out_pt(op2, false);
                self.out_pts[op1].next = op2;
                self.out_pts[op2].prev = op1;
                self.out_pts[op1b].prev = op2b;
                self.out_pts[op2b].next = op1b;
                self.joins[j].out_pt1 = op1;
                self.joins[j].out_pt2 = op1b;
                true
            }
        } else if is_horizontal {
            // treat horizontal joins differently to non-horizontal joins since with
            // them we're not yet sure where the overlapping is. OutPt1.Pt & OutPt2.Pt
            // may be anywhere along the horizontal edge.
            op1b = op1;
            while self.op(self.op(op1).prev).pt.y == self.op(op1).pt.y && self.op(op1).prev != op1b && self.op(op1).prev != op2 {
                op1 = self.op(op1).prev;
            }
            while self.op(self.op(op1b).next).pt.y == self.op(op1b).pt.y && self.op(op1b).next != op1 && self.op(op1b).next != op2 {
                op1b = self.op(op1b).next;
            }
            if self.op(op1b).next == op1 || self.op(op1b).next == op2 {
                return false; // a flat 'polygon'
            }

            op2b = op2;
            while self.op(self.op(op2).prev).pt.y == self.op(op2).pt.y && self.op(op2).prev != op2b && self.op(op2).prev != op1b {
                op2 = self.op(op2).prev;
            }
            while self.op(self.op(op2b).next).pt.y == self.op(op2b).pt.y && self.op(op2b).next != op2 && self.op(op2b).next != op1 {
                op2b = self.op(op2b).next;
            }
            if self.op(op2b).next == op2 || self.op(op2b).next == op1 {
                return false; // a flat 'polygon'
            }

            // Op1 --> Op1b & Op2 --> Op2b are the extremites of the horizontal edges
            let (ok, left, right) = get_overlap(self.op(op1).pt.x, self.op(op1b).pt.x, self.op(op2).pt.x, self.op(op2b).pt.x);
            if !ok {
                return false;
            }

            // DiscardLeftSide: when overlapping edges are joined, a spike will created
            // which needs to be cleaned up. However, we don't want Op1 or Op2 caught up
            // on the discard Side as either may still be needed for other joins ...
            let pt;
            let discard_left_side;
            let (p1, p1b, p2, p2b) = (self.op(op1).pt, self.op(op1b).pt, self.op(op2).pt, self.op(op2b).pt);
            if p1.x >= left && p1.x <= right {
                pt = p1;
                discard_left_side = p1.x > p1b.x;
            } else if p2.x >= left && p2.x <= right {
                pt = p2;
                discard_left_side = p2.x > p2b.x;
            } else if p1b.x >= left && p1b.x <= right {
                pt = p1b;
                discard_left_side = p1b.x > p1.x;
            } else {
                pt = p2b;
                discard_left_side = p2b.x > p2.x;
            }
            self.joins[j].out_pt1 = op1;
            self.joins[j].out_pt2 = op2;
            self.join_horz(op1, op1b, op2, op2b, pt, discard_left_side)
        } else {
            // nb: For non-horizontal joins ...
            //    1. Jr.OutPt1.Pt.Y == Jr.OutPt2.Pt.Y
            //    2. Jr.OutPt1.Pt > Jr.OffPt.Y

            // make sure the polygons are correctly oriented ...
            op1b = self.op(op1).next;
            while self.op(op1b).pt == self.op(op1).pt && op1b != op1 {
                op1b = self.op(op1b).next;
            }
            let reverse1 = self.op(op1b).pt.y > self.op(op1).pt.y || !slopes_equal3(self.op(op1).pt, self.op(op1b).pt, off_pt);
            if reverse1 {
                op1b = self.op(op1).prev;
                while self.op(op1b).pt == self.op(op1).pt && op1b != op1 {
                    op1b = self.op(op1b).prev;
                }
                if self.op(op1b).pt.y > self.op(op1).pt.y || !slopes_equal3(self.op(op1).pt, self.op(op1b).pt, off_pt) {
                    return false;
                }
            }
            op2b = self.op(op2).next;
            while self.op(op2b).pt == self.op(op2).pt && op2b != op2 {
                op2b = self.op(op2b).next;
            }
            let reverse2 = self.op(op2b).pt.y > self.op(op2).pt.y || !slopes_equal3(self.op(op2).pt, self.op(op2b).pt, off_pt);
            if reverse2 {
                op2b = self.op(op2).prev;
                while self.op(op2b).pt == self.op(op2).pt && op2b != op2 {
                    op2b = self.op(op2b).prev;
                }
                if self.op(op2b).pt.y > self.op(op2).pt.y || !slopes_equal3(self.op(op2).pt, self.op(op2b).pt, off_pt) {
                    return false;
                }
            }

            if op1b == op1 || op2b == op2 || op1b == op2b || (out_rec1 == out_rec2 && reverse1 == reverse2) {
                return false;
            }

            if reverse1 {
                op1b = self.dup_out_pt(op1, false);
                op2b = self.dup_out_pt(op2, true);
                self.out_pts[op1].prev = op2;
                self.out_pts[op2].next = op1;
                self.out_pts[op1b].next = op2b;
                self.out_pts[op2b].prev = op1b;
                self.joins[j].out_pt1 = op1;
                self.joins[j].out_pt2 = op1b;
                true
            } else {
                op1b = self.dup_out_pt(op1, true);
                op2b = self.dup_out_pt(op2, false);
                self.out_pts[op1].next = op2;
                self.out_pts[op2].prev = op1;
                self.out_pts[op1b].prev = op2b;
                self.out_pts[op2b].next = op1b;
                self.joins[j].out_pt1 = op1;
                self.joins[j].out_pt2 = op1b;
                true
            }
        }
    }

    fn parse_first_left(&self, mut first_left: usize) -> usize {
        while first_left != NIL && self.poly_outs[first_left].pts == NIL {
            first_left = self.poly_outs[first_left].first_left;
        }
        first_left
    }

    fn fixup_first_lefts1(&mut self, old_out_rec: usize, new_out_rec: usize) {
        // tests if NewOutRec contains the polygon before reassigning FirstLeft
        for i in 0..self.poly_outs.len() {
            let out_rec = self.poly_outs[i];
            let first_left = self.parse_first_left(out_rec.first_left);
            if out_rec.pts != NIL && first_left == old_out_rec {
                if self.poly2_contains_poly1(out_rec.pts, self.poly_outs[new_out_rec].pts) {
                    self.poly_outs[i].first_left = new_out_rec;
                }
            }
        }
    }

    fn fixup_first_lefts2(&mut self, inner_out_rec: usize, outer_out_rec: usize) {
        // A polygon has split into two such that one is now the inner of the other.
        // It's possible that these polygons now wrap around other polygons, so check
        // every polygon that's also contained by OuterOutRec's FirstLeft container
        // (including 0) to see if they've become inner to the new inner polygon ...
        let orfl = self.poly_outs[outer_out_rec].first_left;
        for i in 0..self.poly_outs.len() {
            let out_rec = self.poly_outs[i];
            if out_rec.pts == NIL || i == outer_out_rec || i == inner_out_rec {
                continue;
            }
            let first_left = self.parse_first_left(out_rec.first_left);
            if first_left != orfl && first_left != inner_out_rec && first_left != outer_out_rec {
                continue;
            }
            if self.poly2_contains_poly1(out_rec.pts, self.poly_outs[inner_out_rec].pts) {
                self.poly_outs[i].first_left = inner_out_rec;
            } else if self.poly2_contains_poly1(out_rec.pts, self.poly_outs[outer_out_rec].pts) {
                self.poly_outs[i].first_left = outer_out_rec;
            } else if out_rec.first_left == inner_out_rec || out_rec.first_left == outer_out_rec {
                self.poly_outs[i].first_left = orfl;
            }
        }
    }

    fn fixup_first_lefts3(&mut self, old_out_rec: usize, new_out_rec: usize) {
        // reassigns FirstLeft WITHOUT testing if NewOutRec contains the polygon
        for i in 0..self.poly_outs.len() {
            let out_rec = self.poly_outs[i];
            let first_left = self.parse_first_left(out_rec.first_left);
            if out_rec.pts != NIL && first_left == old_out_rec {
                self.poly_outs[i].first_left = new_out_rec;
            }
        }
    }

    fn update_out_pt_idxs(&mut self, outrec: usize) {
        let start = self.poly_outs[outrec].pts;
        let idx = self.poly_outs[outrec].idx;
        let mut op = start;
        loop {
            self.out_pts[op].idx = idx;
            op = self.op(op).prev;
            if op == start {
                break;
            }
        }
    }

    fn join_common_edges(&mut self) {
        let mut i = 0;
        while i < self.joins.len() {
            let join = self.joins[i];

            let mut out_rec1 = self.get_out_rec(self.op(join.out_pt1).idx);
            let mut out_rec2 = self.get_out_rec(self.op(join.out_pt2).idx);

            if self.poly_outs[out_rec1].pts == NIL || self.poly_outs[out_rec2].pts == NIL {
                i += 1;
                continue;
            }
            if self.poly_outs[out_rec1].is_open || self.poly_outs[out_rec2].is_open {
                i += 1;
                continue;
            }

            // get the polygon fragment with the correct hole state (FirstLeft)
            // before calling JoinPoints() ...
            let hole_state_rec = if out_rec1 == out_rec2 {
                out_rec1
            } else if self.out_rec1_right_of_out_rec2(out_rec1, out_rec2) {
                out_rec2
            } else if self.out_rec1_right_of_out_rec2(out_rec2, out_rec1) {
                out_rec1
            } else {
                self.get_lowermost_rec(out_rec1, out_rec2)
            };

            if !self.join_points(i, out_rec1, out_rec2) {
                i += 1;
                continue;
            }
            let join = self.joins[i];

            if out_rec1 == out_rec2 {
                // instead of joining two polygons, we've just created a new one by
                // splitting one polygon into two.
                self.poly_outs[out_rec1].pts = join.out_pt1;
                self.poly_outs[out_rec1].bottom_pt = NIL;
                out_rec2 = self.create_out_rec();
                self.poly_outs[out_rec2].pts = join.out_pt2;

                // update all OutRec2.Pts Idx's ...
                self.update_out_pt_idxs(out_rec2);

                if self.poly2_contains_poly1(self.poly_outs[out_rec2].pts, self.poly_outs[out_rec1].pts) {
                    // outRec1 contains outRec2 ...
                    self.poly_outs[out_rec2].is_hole = !self.poly_outs[out_rec1].is_hole;
                    self.poly_outs[out_rec2].first_left = out_rec1;

                    if self.using_poly_tree {
                        self.fixup_first_lefts2(out_rec2, out_rec1);
                    }

                    if (self.poly_outs[out_rec2].is_hole ^ self.reverse_output) == (self.area_outrec(out_rec2) > 0.0) {
                        self.reverse_poly_pt_links(self.poly_outs[out_rec2].pts);
                    }
                } else if self.poly2_contains_poly1(self.poly_outs[out_rec1].pts, self.poly_outs[out_rec2].pts) {
                    // outRec2 contains outRec1 ...
                    self.poly_outs[out_rec2].is_hole = self.poly_outs[out_rec1].is_hole;
                    self.poly_outs[out_rec1].is_hole = !self.poly_outs[out_rec2].is_hole;
                    self.poly_outs[out_rec2].first_left = self.poly_outs[out_rec1].first_left;
                    self.poly_outs[out_rec1].first_left = out_rec2;

                    if self.using_poly_tree {
                        self.fixup_first_lefts2(out_rec1, out_rec2);
                    }

                    if (self.poly_outs[out_rec1].is_hole ^ self.reverse_output) == (self.area_outrec(out_rec1) > 0.0) {
                        self.reverse_poly_pt_links(self.poly_outs[out_rec1].pts);
                    }
                } else {
                    // the 2 polygons are completely separate ...
                    self.poly_outs[out_rec2].is_hole = self.poly_outs[out_rec1].is_hole;
                    self.poly_outs[out_rec2].first_left = self.poly_outs[out_rec1].first_left;

                    // fixup FirstLeft pointers that may need reassigning to OutRec2
                    if self.using_poly_tree {
                        self.fixup_first_lefts1(out_rec1, out_rec2);
                    }
                }
            } else {
                // joined 2 polygons together ...
                self.poly_outs[out_rec2].pts = NIL;
                self.poly_outs[out_rec2].bottom_pt = NIL;
                self.poly_outs[out_rec2].idx = self.poly_outs[out_rec1].idx;

                self.poly_outs[out_rec1].is_hole = self.poly_outs[hole_state_rec].is_hole;
                if hole_state_rec == out_rec2 {
                    self.poly_outs[out_rec1].first_left = self.poly_outs[out_rec2].first_left;
                }
                self.poly_outs[out_rec2].first_left = out_rec1;

                if self.using_poly_tree {
                    self.fixup_first_lefts3(out_rec2, out_rec1);
                }
            }
            let _ = &mut out_rec1;
            i += 1;
        }
    }

    fn do_simple_polygons(&mut self) {
        let mut i = 0;
        while i < self.poly_outs.len() {
            let outrec = i;
            i += 1;
            let mut op = self.poly_outs[outrec].pts;
            if op == NIL || self.poly_outs[outrec].is_open {
                continue;
            }
            loop {
                // for each Pt in Polygon until duplicate found do ...
                let mut op2 = self.op(op).next;
                while op2 != self.poly_outs[outrec].pts {
                    if self.op(op).pt == self.op(op2).pt && self.op(op2).next != op && self.op(op2).prev != op {
                        // split the polygon into two ...
                        let op3 = self.op(op).prev;
                        let op4 = self.op(op2).prev;
                        self.out_pts[op].prev = op4;
                        self.out_pts[op4].next = op;
                        self.out_pts[op2].prev = op3;
                        self.out_pts[op3].next = op2;

                        self.poly_outs[outrec].pts = op;
                        let outrec2 = self.create_out_rec();
                        self.poly_outs[outrec2].pts = op2;
                        self.update_out_pt_idxs(outrec2);
                        if self.poly2_contains_poly1(self.poly_outs[outrec2].pts, self.poly_outs[outrec].pts) {
                            // OutRec2 is contained by OutRec1 ...
                            self.poly_outs[outrec2].is_hole = !self.poly_outs[outrec].is_hole;
                            self.poly_outs[outrec2].first_left = outrec;
                            if self.using_poly_tree {
                                self.fixup_first_lefts2(outrec2, outrec);
                            }
                        } else if self.poly2_contains_poly1(self.poly_outs[outrec].pts, self.poly_outs[outrec2].pts) {
                            // OutRec1 is contained by OutRec2 ...
                            self.poly_outs[outrec2].is_hole = self.poly_outs[outrec].is_hole;
                            self.poly_outs[outrec].is_hole = !self.poly_outs[outrec2].is_hole;
                            self.poly_outs[outrec2].first_left = self.poly_outs[outrec].first_left;
                            self.poly_outs[outrec].first_left = outrec2;
                            if self.using_poly_tree {
                                self.fixup_first_lefts2(outrec, outrec2);
                            }
                        } else {
                            // the 2 polygons are separate ...
                            self.poly_outs[outrec2].is_hole = self.poly_outs[outrec].is_hole;
                            self.poly_outs[outrec2].first_left = self.poly_outs[outrec].first_left;
                            if self.using_poly_tree {
                                self.fixup_first_lefts1(outrec, outrec2);
                            }
                        }
                        op2 = op; // ie get ready for the Next iteration
                    }
                    op2 = self.op(op2).next;
                }
                op = self.op(op).next;
                if op == self.poly_outs[outrec].pts {
                    break;
                }
            }
        }
    }
}

// ------------------------------------------------------------------------------------------
// ClipperOffset
// ------------------------------------------------------------------------------------------

const PI: f64 = 3.141592653589793238;
const TWO_PI: f64 = PI * 2.0;
const DEF_ARC_TOLERANCE: f64 = 0.25;

fn get_unit_normal(pt1: IntPoint, pt2: IntPoint) -> DoublePoint {
    if pt2.x == pt1.x && pt2.y == pt1.y {
        return DoublePoint::new(0.0, 0.0);
    }
    let mut dx = (pt2.x - pt1.x) as f64;
    let mut dy = (pt2.y - pt1.y) as f64;
    let f = 1.0 * 1.0 / (dx * dx + dy * dy).sqrt();
    dx *= f;
    dy *= f;
    DoublePoint::new(dy, -dx)
}

/// `ClipperLib::ClipperOffset`.
pub struct ClipperOffset {
    pub miter_limit: f64,
    pub arc_tolerance: f64,
    dest_polys: Paths,
    src_poly: Path,
    dest_poly: Path,
    normals: Vec<DoublePoint>,
    delta: f64,
    sin_a: f64,
    sin: f64,
    cos: f64,
    miter_lim: f64,
    steps_per_rad: f64,
    lowest: IntPoint,
    poly_nodes: Vec<PolyNode>,
}

impl Default for ClipperOffset {
    fn default() -> Self {
        ClipperOffset::new(2.0, DEF_ARC_TOLERANCE)
    }
}

impl ClipperOffset {
    pub fn new(miter_limit: f64, arc_tolerance: f64) -> Self {
        ClipperOffset {
            miter_limit,
            arc_tolerance,
            dest_polys: Paths::new(),
            src_poly: Path::new(),
            dest_poly: Path::new(),
            normals: Vec::new(),
            delta: 0.0,
            sin_a: 0.0,
            sin: 0.0,
            cos: 0.0,
            miter_lim: 0.0,
            steps_per_rad: 0.0,
            lowest: IntPoint::new(-1, 0),
            poly_nodes: Vec::new(),
        }
    }

    pub fn clear(&mut self) {
        self.poly_nodes.clear();
        self.lowest.x = -1;
    }

    pub fn add_path(&mut self, path: &[IntPoint], join_type: JoinType, end_type: EndType) {
        if path.is_empty() {
            return;
        }
        let mut high_i = path.len() - 1;
        let mut new_node = PolyNode { join_type: Some(join_type), end_type: Some(end_type), ..Default::default() };

        // strip duplicate points from path and also get index to the lowest point ...
        if end_type == EndType::ClosedLine || end_type == EndType::ClosedPolygon {
            while high_i > 0 && path[0] == path[high_i] {
                high_i -= 1;
            }
        }
        new_node.contour.reserve(high_i + 1);
        new_node.contour.push(path[0]);
        let mut j = 0usize;
        let mut k = 0usize;
        for i in 1..=high_i {
            if new_node.contour[j] != path[i] {
                j += 1;
                new_node.contour.push(path[i]);
                if path[i].y > new_node.contour[k].y || (path[i].y == new_node.contour[k].y && path[i].x < new_node.contour[k].x) {
                    k = j;
                }
            }
        }
        if end_type == EndType::ClosedPolygon && j < 2 {
            return;
        }
        let contour_k = new_node.contour[k];
        self.poly_nodes.push(new_node);

        // if this path's lowest pt is lower than all the others then update m_lowest
        if end_type != EndType::ClosedPolygon {
            return;
        }
        if self.lowest.x < 0 {
            self.lowest = IntPoint::new(self.poly_nodes.len() as CInt - 1, k as CInt);
        } else {
            let ip = self.poly_nodes[self.lowest.x as usize].contour[self.lowest.y as usize];
            if contour_k.y > ip.y || (contour_k.y == ip.y && contour_k.x < ip.x) {
                self.lowest = IntPoint::new(self.poly_nodes.len() as CInt - 1, k as CInt);
            }
        }
    }

    pub fn add_paths<P: AsRef<[IntPoint]>>(&mut self, paths: &[P], join_type: JoinType, end_type: EndType) {
        for p in paths {
            self.add_path(p.as_ref(), join_type, end_type);
        }
    }

    fn fix_orientations(&mut self) {
        // fixup orientations of all closed paths if the orientation of the
        // closed path with the lowermost vertex is wrong ...
        if self.lowest.x >= 0 && !orientation(&self.poly_nodes[self.lowest.x as usize].contour) {
            for node in self.poly_nodes.iter_mut() {
                if node.end_type == Some(EndType::ClosedPolygon) || (node.end_type == Some(EndType::ClosedLine) && orientation(&node.contour)) {
                    node.contour.reverse();
                }
            }
        } else {
            for node in self.poly_nodes.iter_mut() {
                if node.end_type == Some(EndType::ClosedLine) && !orientation(&node.contour) {
                    node.contour.reverse();
                }
            }
        }
    }

    /// `ClipperOffset::Execute(Paths&, delta)`.
    pub fn execute(&mut self, solution: &mut Paths, delta: f64) {
        solution.clear();
        self.fix_orientations();
        self.do_offset(delta);

        // now clean up 'corners' ...
        let mut clpr = Clipper::new(0);
        clpr.add_paths(&self.dest_polys, PolyType::Subject, true);
        if delta > 0.0 {
            clpr.execute(ClipType::Union, solution, PolyFillType::Positive, PolyFillType::Positive);
        } else {
            let r = clpr.get_bounds();
            let outer = vec![
                IntPoint::new(r.left - 10, r.bottom + 10),
                IntPoint::new(r.right + 10, r.bottom + 10),
                IntPoint::new(r.right + 10, r.top - 10),
                IntPoint::new(r.left - 10, r.top - 10),
            ];
            clpr.add_path(&outer, PolyType::Subject, true);
            clpr.set_reverse_solution(true);
            clpr.execute(ClipType::Union, solution, PolyFillType::Negative, PolyFillType::Negative);
            if !solution.is_empty() {
                solution.remove(0);
            }
        }
    }

    /// Convenience wrapper returning the solution.
    pub fn execute_paths(&mut self, delta: f64) -> Paths {
        let mut sol = Paths::new();
        self.execute(&mut sol, delta);
        sol
    }

    /// `ClipperOffset::Execute(PolyTree&, delta)`.
    pub fn execute_tree(&mut self, solution: &mut PolyTree, delta: f64) {
        solution.clear();
        self.fix_orientations();
        self.do_offset(delta);

        // now clean up 'corners' ...
        let mut clpr = Clipper::new(0);
        clpr.add_paths(&self.dest_polys, PolyType::Subject, true);
        if delta > 0.0 {
            clpr.execute_tree(ClipType::Union, solution, PolyFillType::Positive, PolyFillType::Positive);
        } else {
            let r = clpr.get_bounds();
            let outer = vec![
                IntPoint::new(r.left - 10, r.bottom + 10),
                IntPoint::new(r.right + 10, r.bottom + 10),
                IntPoint::new(r.right + 10, r.top - 10),
                IntPoint::new(r.left - 10, r.top - 10),
            ];
            clpr.add_path(&outer, PolyType::Subject, true);
            clpr.set_reverse_solution(true);
            clpr.execute_tree(ClipType::Union, solution, PolyFillType::Negative, PolyFillType::Negative);
            // remove the outer PolyNode rectangle ...
            if solution.childs.len() == 1 && !solution.all_nodes[solution.childs[0]].childs.is_empty() {
                let outer_node = solution.childs[0];
                let outer_childs = solution.all_nodes[outer_node].childs.clone();
                solution.childs.clear();
                solution.childs.push(outer_childs[0]);
                solution.all_nodes[outer_childs[0]].parent = None;
                solution.all_nodes[outer_childs[0]].index = 0;
                for &c in outer_childs.iter().skip(1) {
                    solution.add_child_to_root(c);
                }
                // The outer node stays in all_nodes (as in C++), but is detached.
                solution.all_nodes[outer_node].childs.clear();
            } else {
                solution.clear();
            }
        }
    }

    fn do_offset(&mut self, delta: f64) {
        self.dest_polys.clear();
        self.delta = delta;

        // if Zero offset, just copy any CLOSED polygons to m_p and return ...
        if near_zero(delta) {
            self.dest_polys.reserve(self.poly_nodes.len());
            for node in &self.poly_nodes {
                if node.end_type == Some(EndType::ClosedPolygon) {
                    self.dest_polys.push(node.contour.clone());
                }
            }
            return;
        }

        // see offset_triginometry3.svg in the documentation folder ...
        if self.miter_limit > 2.0 {
            self.miter_lim = 2.0 / (self.miter_limit * self.miter_limit);
        } else {
            self.miter_lim = 0.5;
        }

        let y = if self.arc_tolerance <= 0.0 {
            DEF_ARC_TOLERANCE
        } else if self.arc_tolerance > delta.abs() * DEF_ARC_TOLERANCE {
            delta.abs() * DEF_ARC_TOLERANCE
        } else {
            self.arc_tolerance
        };
        // see offset_triginometry2.svg in the documentation folder ...
        let mut steps = PI / (1.0 - y / delta.abs()).acos();
        if steps > delta.abs() * PI {
            steps = delta.abs() * PI; // ie excessive precision check
        }
        self.sin = (TWO_PI / steps).sin();
        self.cos = (TWO_PI / steps).cos();
        self.steps_per_rad = steps / TWO_PI;
        if delta < 0.0 {
            self.sin = -self.sin;
        }

        self.dest_polys.reserve(self.poly_nodes.len() * 2);
        for ni in 0..self.poly_nodes.len() {
            let join_type = self.poly_nodes[ni].join_type.unwrap_or(JoinType::Square);
            let end_type = self.poly_nodes[ni].end_type.unwrap_or(EndType::ClosedPolygon);
            self.src_poly = self.poly_nodes[ni].contour.clone();

            let len = self.src_poly.len();
            if len == 0 || (delta <= 0.0 && (len < 3 || end_type != EndType::ClosedPolygon)) {
                continue;
            }

            self.dest_poly.clear();
            if len == 1 {
                if join_type == JoinType::Round {
                    let mut x = 1.0f64;
                    let mut y = 0.0f64;
                    let mut j: CInt = 1;
                    while (j as f64) <= steps {
                        self.dest_poly.push(IntPoint::new(
                            round(self.src_poly[0].x as f64 + x * delta),
                            round(self.src_poly[0].y as f64 + y * delta),
                        ));
                        let x2 = x;
                        x = x * self.cos - self.sin * y;
                        y = x2 * self.sin + y * self.cos;
                        j += 1;
                    }
                } else {
                    let mut x = -1.0f64;
                    let mut y = -1.0f64;
                    for _ in 0..4 {
                        self.dest_poly.push(IntPoint::new(
                            round(self.src_poly[0].x as f64 + x * delta),
                            round(self.src_poly[0].y as f64 + y * delta),
                        ));
                        if x < 0.0 {
                            x = 1.0;
                        } else if y < 0.0 {
                            y = 1.0;
                        } else {
                            x = -1.0;
                        }
                    }
                }
                let dp = std::mem::take(&mut self.dest_poly);
                self.dest_polys.push(dp);
                continue;
            }
            // build m_normals ...
            self.normals.clear();
            self.normals.reserve(len);
            for j in 0..len - 1 {
                self.normals.push(get_unit_normal(self.src_poly[j], self.src_poly[j + 1]));
            }
            if end_type == EndType::ClosedLine || end_type == EndType::ClosedPolygon {
                self.normals.push(get_unit_normal(self.src_poly[len - 1], self.src_poly[0]));
            } else {
                let n = self.normals[len - 2];
                self.normals.push(n);
            }

            if end_type == EndType::ClosedPolygon {
                let mut k = len - 1;
                for j in 0..len {
                    self.offset_point(j, &mut k, join_type);
                }
                let dp = std::mem::take(&mut self.dest_poly);
                self.dest_polys.push(dp);
            } else if end_type == EndType::ClosedLine {
                let mut k = len - 1;
                for j in 0..len {
                    self.offset_point(j, &mut k, join_type);
                }
                let dp = std::mem::take(&mut self.dest_poly);
                self.dest_polys.push(dp);
                self.dest_poly.clear();
                // re-build m_normals ...
                let n = self.normals[len - 1];
                for j in (1..len).rev() {
                    self.normals[j] = DoublePoint::new(-self.normals[j - 1].x, -self.normals[j - 1].y);
                }
                self.normals[0] = DoublePoint::new(-n.x, -n.y);
                k = 0;
                for j in (0..len).rev() {
                    self.offset_point(j, &mut k, join_type);
                }
                let dp = std::mem::take(&mut self.dest_poly);
                self.dest_polys.push(dp);
            } else {
                let mut k = 0usize;
                for j in 1..len - 1 {
                    self.offset_point(j, &mut k, join_type);
                }

                if end_type == EndType::OpenButt {
                    let j = len - 1;
                    let pt1 = IntPoint::new(
                        round(self.src_poly[j].x as f64 + self.normals[j].x * delta),
                        round(self.src_poly[j].y as f64 + self.normals[j].y * delta),
                    );
                    self.dest_poly.push(pt1);
                    let pt1 = IntPoint::new(
                        round(self.src_poly[j].x as f64 - self.normals[j].x * delta),
                        round(self.src_poly[j].y as f64 - self.normals[j].y * delta),
                    );
                    self.dest_poly.push(pt1);
                } else {
                    let j = len - 1;
                    k = len - 2;
                    self.sin_a = 0.0;
                    self.normals[j] = DoublePoint::new(-self.normals[j].x, -self.normals[j].y);
                    if end_type == EndType::OpenSquare {
                        self.do_square(j, k);
                    } else {
                        self.do_round(j, k);
                    }
                }

                // re-build m_normals ...
                for j in (1..len).rev() {
                    self.normals[j] = DoublePoint::new(-self.normals[j - 1].x, -self.normals[j - 1].y);
                }
                self.normals[0] = DoublePoint::new(-self.normals[1].x, -self.normals[1].y);

                k = len - 1;
                let mut j = k as isize - 1;
                while j > 0 {
                    self.offset_point(j as usize, &mut k, join_type);
                    j -= 1;
                }

                if end_type == EndType::OpenButt {
                    let pt1 = IntPoint::new(
                        round(self.src_poly[0].x as f64 - self.normals[0].x * delta),
                        round(self.src_poly[0].y as f64 - self.normals[0].y * delta),
                    );
                    self.dest_poly.push(pt1);
                    let pt1 = IntPoint::new(
                        round(self.src_poly[0].x as f64 + self.normals[0].x * delta),
                        round(self.src_poly[0].y as f64 + self.normals[0].y * delta),
                    );
                    self.dest_poly.push(pt1);
                } else {
                    self.sin_a = 0.0;
                    if end_type == EndType::OpenSquare {
                        self.do_square(0, 1);
                    } else {
                        self.do_round(0, 1);
                    }
                }
                let dp = std::mem::take(&mut self.dest_poly);
                self.dest_polys.push(dp);
            }
        }
    }

    fn offset_point(&mut self, j: usize, k: &mut usize, jointype: JoinType) {
        let nk = self.normals[*k];
        let nj = self.normals[j];
        // cross product ...
        self.sin_a = nk.x * nj.y - nj.x * nk.y;
        if (self.sin_a * self.delta).abs() < 1.0 {
            // dot product ...
            let cos_a = nk.x * nj.x + nj.y * nk.y;
            if cos_a > 0.0 {
                // angle => 0 degrees
                self.dest_poly.push(IntPoint::new(
                    round(self.src_poly[j].x as f64 + nk.x * self.delta),
                    round(self.src_poly[j].y as f64 + nk.y * self.delta),
                ));
                return;
            }
            // else angle => 180 degrees
        } else if self.sin_a > 1.0 {
            self.sin_a = 1.0;
        } else if self.sin_a < -1.0 {
            self.sin_a = -1.0;
        }

        if self.sin_a * self.delta < 0.0 {
            self.dest_poly.push(IntPoint::new(
                round(self.src_poly[j].x as f64 + nk.x * self.delta),
                round(self.src_poly[j].y as f64 + nk.y * self.delta),
            ));
            self.dest_poly.push(self.src_poly[j]);
            self.dest_poly.push(IntPoint::new(
                round(self.src_poly[j].x as f64 + nj.x * self.delta),
                round(self.src_poly[j].y as f64 + nj.y * self.delta),
            ));
        } else {
            match jointype {
                JoinType::Miter => {
                    let r = 1.0 + (nj.x * nk.x + nj.y * nk.y);
                    if r >= self.miter_lim {
                        self.do_miter(j, *k, r);
                    } else {
                        self.do_square(j, *k);
                    }
                }
                JoinType::Square => self.do_square(j, *k),
                JoinType::Round => self.do_round(j, *k),
            }
        }
        *k = j;
    }

    fn do_square(&mut self, j: usize, k: usize) {
        let nk = self.normals[k];
        let nj = self.normals[j];
        let dx = (self.sin_a.atan2(nk.x * nj.x + nk.y * nj.y) / 4.0).tan();
        self.dest_poly.push(IntPoint::new(
            round(self.src_poly[j].x as f64 + self.delta * (nk.x - nk.y * dx)),
            round(self.src_poly[j].y as f64 + self.delta * (nk.y + nk.x * dx)),
        ));
        self.dest_poly.push(IntPoint::new(
            round(self.src_poly[j].x as f64 + self.delta * (nj.x + nj.y * dx)),
            round(self.src_poly[j].y as f64 + self.delta * (nj.y - nj.x * dx)),
        ));
    }

    fn do_miter(&mut self, j: usize, k: usize, r: f64) {
        let nk = self.normals[k];
        let nj = self.normals[j];
        let q = self.delta / r;
        self.dest_poly.push(IntPoint::new(
            round(self.src_poly[j].x as f64 + (nk.x + nj.x) * q),
            round(self.src_poly[j].y as f64 + (nk.y + nj.y) * q),
        ));
    }

    fn do_round(&mut self, j: usize, k: usize) {
        let nk = self.normals[k];
        let nj = self.normals[j];
        let a = self.sin_a.atan2(nk.x * nj.x + nk.y * nj.y);
        let steps = (round(self.steps_per_rad * a.abs()) as i32).max(1);

        let mut x = nk.x;
        let mut y = nk.y;
        for _ in 0..steps {
            self.dest_poly.push(IntPoint::new(
                round(self.src_poly[j].x as f64 + x * self.delta),
                round(self.src_poly[j].y as f64 + y * self.delta),
            ));
            let x2 = x;
            x = x * self.cos - self.sin * y;
            y = x2 * self.sin + y * self.cos;
        }
        self.dest_poly.push(IntPoint::new(
            round(self.src_poly[j].x as f64 + nj.x * self.delta),
            round(self.src_poly[j].y as f64 + nj.y * self.delta),
        ));
    }
}

// ------------------------------------------------------------------------------------------
// Miscellaneous public functions
// ------------------------------------------------------------------------------------------

/// `ClipperLib::ReversePath`.
pub fn reverse_path(p: &mut Path) {
    p.reverse();
}

/// `ClipperLib::ReversePaths`.
pub fn reverse_paths(p: &mut Paths) {
    for path in p.iter_mut() {
        path.reverse();
    }
}

/// `ClipperLib::SimplifyPolygon`.
pub fn simplify_polygon(in_poly: &[IntPoint], fill_type: PolyFillType) -> Paths {
    let mut c = Clipper::new(0);
    c.set_strictly_simple(true);
    c.add_path(in_poly, PolyType::Subject, true);
    let mut out = Paths::new();
    c.execute(ClipType::Union, &mut out, fill_type, fill_type);
    out
}

/// `ClipperLib::SimplifyPolygons`.
pub fn simplify_polygons<P: AsRef<[IntPoint]>>(in_polys: &[P], fill_type: PolyFillType) -> Paths {
    let mut c = Clipper::new(0);
    c.set_strictly_simple(true);
    c.add_paths(in_polys, PolyType::Subject, true);
    let mut out = Paths::new();
    c.execute(ClipType::Union, &mut out, fill_type, fill_type);
    out
}

#[inline]
fn distance_from_line_sqrd(pt: IntPoint, ln1: IntPoint, ln2: IntPoint) -> f64 {
    // The equation of a line in general form (Ax + By + C = 0)
    // given 2 points (x1,y1) & (x2,y2) is ...
    // (y1 - y2)x + (x2 - x1)y + (y2 - y1)x1 - (x2 - x1)y1 = 0
    // A = (y1 - y2); B = (x2 - x1); C = (y2 - y1)x1 - (x2 - x1)y1
    // perpendicular distance of point (x3,y3) = (Ax3 + By3 + C)/Sqrt(A^2 + B^2)
    let a = (ln1.y - ln2.y) as f64;
    let b = (ln2.x - ln1.x) as f64;
    let mut c = a * ln1.x as f64 + b * ln1.y as f64;
    c = a * pt.x as f64 + b * pt.y as f64 - c;
    (c * c) / (a * a + b * b)
}

fn slopes_near_collinear(pt1: IntPoint, pt2: IntPoint, pt3: IntPoint, dist_sqrd: f64) -> bool {
    // this function is more accurate when the point that's geometrically
    // between the other 2 points is the one that's tested for distance.
    // ie makes it more likely to pick up 'spikes' ...
    if abs(pt1.x - pt2.x) > abs(pt1.y - pt2.y) {
        if (pt1.x > pt2.x) == (pt1.x < pt3.x) {
            distance_from_line_sqrd(pt1, pt2, pt3) < dist_sqrd
        } else if (pt2.x > pt1.x) == (pt2.x < pt3.x) {
            distance_from_line_sqrd(pt2, pt1, pt3) < dist_sqrd
        } else {
            distance_from_line_sqrd(pt3, pt1, pt2) < dist_sqrd
        }
    } else {
        if (pt1.y > pt2.y) == (pt1.y < pt3.y) {
            distance_from_line_sqrd(pt1, pt2, pt3) < dist_sqrd
        } else if (pt2.y > pt1.y) == (pt2.y < pt3.y) {
            distance_from_line_sqrd(pt2, pt1, pt3) < dist_sqrd
        } else {
            distance_from_line_sqrd(pt3, pt1, pt2) < dist_sqrd
        }
    }
}

#[inline]
fn points_are_close(pt1: IntPoint, pt2: IntPoint, dist_sqrd: f64) -> bool {
    let dx = pt1.x as f64 - pt2.x as f64;
    let dy = pt1.y as f64 - pt2.y as f64;
    (dx * dx) + (dy * dy) <= dist_sqrd
}

/// `ClipperLib::CleanPolygon`.
pub fn clean_polygon(in_poly: &[IntPoint], distance: f64) -> Path {
    // distance = proximity in units/pixels below which vertices
    // will be stripped. Default ~= sqrt(2).
    let mut size = in_poly.len();
    if size == 0 {
        return Path::new();
    }

    struct P {
        pt: IntPoint,
        next: usize,
        prev: usize,
        idx: i32,
    }
    let mut out_pts: Vec<P> = (0..size)
        .map(|i| P { pt: in_poly[i], next: (i + 1) % size, prev: (i + size - 1) % size, idx: 0 })
        .collect();

    let exclude_op = |pts: &mut Vec<P>, op: usize| -> usize {
        let result = pts[op].prev;
        let n = pts[op].next;
        pts[result].next = n;
        pts[n].prev = result;
        pts[result].idx = 0;
        result
    };

    let dist_sqrd = distance * distance;
    let mut op = 0usize;
    while out_pts[op].idx == 0 && out_pts[op].next != out_pts[op].prev {
        let prev = out_pts[op].prev;
        let next = out_pts[op].next;
        if points_are_close(out_pts[op].pt, out_pts[prev].pt, dist_sqrd) {
            op = exclude_op(&mut out_pts, op);
            size -= 1;
        } else if points_are_close(out_pts[prev].pt, out_pts[next].pt, dist_sqrd) {
            exclude_op(&mut out_pts, next);
            op = exclude_op(&mut out_pts, op);
            size -= 2;
        } else if slopes_near_collinear(out_pts[prev].pt, out_pts[op].pt, out_pts[next].pt, dist_sqrd) {
            op = exclude_op(&mut out_pts, op);
            size -= 1;
        } else {
            out_pts[op].idx = 1;
            op = out_pts[op].next;
        }
    }

    if size < 3 {
        size = 0;
    }
    let mut out_poly = Path::with_capacity(size);
    for _ in 0..size {
        out_poly.push(out_pts[op].pt);
        op = out_pts[op].next;
    }
    out_poly
}

/// `ClipperLib::CleanPolygons`.
pub fn clean_polygons<P: AsRef<[IntPoint]>>(in_polys: &[P], distance: f64) -> Paths {
    in_polys.iter().map(|p| clean_polygon(p.as_ref(), distance)).collect()
}

fn minkowski(poly: &[IntPoint], path: &[IntPoint], is_sum: bool, is_closed: bool) -> Paths {
    let delta = if is_closed { 1 } else { 0 };
    let poly_cnt = poly.len();
    let path_cnt = path.len();
    let mut pp: Paths = Vec::with_capacity(path_cnt);
    if is_sum {
        for i in 0..path_cnt {
            pp.push(poly.iter().map(|q| IntPoint::new(path[i].x + q.x, path[i].y + q.y)).collect());
        }
    } else {
        for i in 0..path_cnt {
            pp.push(poly.iter().map(|q| IntPoint::new(path[i].x - q.x, path[i].y - q.y)).collect());
        }
    }

    let mut solution = Paths::with_capacity((path_cnt + delta) * (poly_cnt + 1));
    if path_cnt == 0 {
        return solution;
    }
    for i in 0..(path_cnt - 1 + delta) {
        for j in 0..poly_cnt {
            let mut quad = vec![
                pp[i % path_cnt][j % poly_cnt],
                pp[(i + 1) % path_cnt][j % poly_cnt],
                pp[(i + 1) % path_cnt][(j + 1) % poly_cnt],
                pp[i % path_cnt][(j + 1) % poly_cnt],
            ];
            if !orientation(&quad) {
                quad.reverse();
            }
            solution.push(quad);
        }
    }
    solution
}

/// `ClipperLib::MinkowskiSum(pattern, path, ...)`.
pub fn minkowski_sum(pattern: &[IntPoint], path: &[IntPoint], path_is_closed: bool) -> Paths {
    let solution = minkowski(pattern, path, true, path_is_closed);
    let mut c = Clipper::new(0);
    c.add_paths(&solution, PolyType::Subject, true);
    let mut out = Paths::new();
    c.execute(ClipType::Union, &mut out, PolyFillType::NonZero, PolyFillType::NonZero);
    out
}

/// `ClipperLib::MinkowskiSum(pattern, paths, ...)`.
pub fn minkowski_sum_paths<P: AsRef<[IntPoint]>>(pattern: &[IntPoint], paths: &[P], path_is_closed: bool) -> Paths {
    let mut c = Clipper::new(0);
    for p in paths {
        let p = p.as_ref();
        let tmp = minkowski(pattern, p, true, path_is_closed);
        c.add_paths(&tmp, PolyType::Subject, true);
        if path_is_closed {
            let tmp2: Path = p.iter().map(|q| IntPoint::new(q.x + pattern[0].x, q.y + pattern[0].y)).collect();
            c.add_path(&tmp2, PolyType::Clip, true);
        }
    }
    let mut out = Paths::new();
    c.execute(ClipType::Union, &mut out, PolyFillType::NonZero, PolyFillType::NonZero);
    out
}

/// `ClipperLib::MinkowskiDiff`.
pub fn minkowski_diff(poly1: &[IntPoint], poly2: &[IntPoint]) -> Paths {
    let solution = minkowski(poly1, poly2, false, true);
    let mut c = Clipper::new(0);
    c.add_paths(&solution, PolyType::Subject, true);
    let mut out = Paths::new();
    c.execute(ClipType::Union, &mut out, PolyFillType::NonZero, PolyFillType::NonZero);
    out
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum NodeType {
    Any,
    Open,
    Closed,
}

fn add_poly_node_to_paths(tree: &PolyTree, node: Option<usize>, nodetype: NodeType, paths: &mut Paths) {
    let mut is_match = true;
    if let Some(n) = node {
        let pn = &tree.all_nodes[n];
        if nodetype == NodeType::Closed {
            is_match = !pn.is_open;
        } else if nodetype == NodeType::Open {
            return;
        }
        if !pn.contour.is_empty() && is_match {
            paths.push(pn.contour.clone());
        }
    }
    let childs = match node {
        None => &tree.childs,
        Some(n) => &tree.all_nodes[n].childs,
    };
    for &c in childs {
        add_poly_node_to_paths(tree, Some(c), nodetype, paths);
    }
}

/// `ClipperLib::PolyTreeToPaths`.
pub fn poly_tree_to_paths(polytree: &PolyTree) -> Paths {
    let mut paths = Paths::with_capacity(polytree.total());
    add_poly_node_to_paths(polytree, None, NodeType::Any, &mut paths);
    paths
}

/// `ClipperLib::ClosedPathsFromPolyTree`.
pub fn closed_paths_from_poly_tree(polytree: &PolyTree) -> Paths {
    let mut paths = Paths::with_capacity(polytree.total());
    add_poly_node_to_paths(polytree, None, NodeType::Closed, &mut paths);
    paths
}

/// `ClipperLib::OpenPathsFromPolyTree`.
pub fn open_paths_from_poly_tree(polytree: &PolyTree) -> Paths {
    let mut paths = Paths::with_capacity(polytree.total());
    // Open paths are top level only, so ...
    for &c in &polytree.childs {
        if polytree.all_nodes[c].is_open {
            paths.push(polytree.all_nodes[c].contour.clone());
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sq(x: i64, y: i64, s: i64) -> Path {
        vec![IntPoint::new(x, y), IntPoint::new(x + s, y), IntPoint::new(x + s, y + s), IntPoint::new(x, y + s)]
    }

    #[test]
    fn union_of_two_squares() {
        let mut c = Clipper::new(0);
        c.add_path(&sq(0, 0, 100), PolyType::Subject, true);
        c.add_path(&sq(50, 50, 100), PolyType::Clip, true);
        let r = c.execute_paths(ClipType::Union, PolyFillType::NonZero);
        assert_eq!(r.len(), 1);
        assert_eq!(area(&r[0]).abs(), 17500.0);
    }

    #[test]
    fn offset_square() {
        let mut o = ClipperOffset::default();
        o.add_path(&sq(0, 0, 100), JoinType::Miter, EndType::ClosedPolygon);
        let r = o.execute_paths(10.0);
        assert_eq!(r.len(), 1);
        assert_eq!(area(&r[0]).abs(), 14400.0);
        let mut o = ClipperOffset::default();
        o.add_path(&sq(0, 0, 100), JoinType::Miter, EndType::ClosedPolygon);
        let r = o.execute_paths(-10.0);
        assert_eq!(r.len(), 1);
        assert_eq!(area(&r[0]).abs(), 6400.0);
    }

    #[test]
    fn open_path_clip() {
        let mut c = Clipper::new(0);
        c.add_path(&[IntPoint::new(-50, 50), IntPoint::new(150, 50)], PolyType::Subject, false);
        c.add_path(&sq(0, 0, 100), PolyType::Clip, true);
        let mut tree = PolyTree::new();
        c.execute_tree(ClipType::Intersection, &mut tree, PolyFillType::NonZero, PolyFillType::NonZero);
        let open = open_paths_from_poly_tree(&tree);
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].len(), 2);
    }
}
