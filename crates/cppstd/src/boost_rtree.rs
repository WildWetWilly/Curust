//! Emulation of `boost::geometry::index::rtree<Point3, quadratic<8>>` built from a range (packing
//! algorithm) and queried with `nearest(point, 1)`, as in Boost 1.86. Ties between equally distant
//! values are resolved exactly like Boost does, which depends on the packed tree structure (built
//! with libstdc++'s `std::nth_element`) and on the order of the branch priority queue.

use crate::{nth_element_by, PriorityQueue};

const MAX_ELEMENTS: usize = 8;
const MIN_ELEMENTS: usize = 2; // default_min_elements_s<8> = 8 * 3 / 10

#[derive(Clone, Copy, Debug)]
struct Box3 {
    min: [f64; 3],
    max: [f64; 3],
}

impl Box3 {
    fn from_point(p: &[f64; 3]) -> Self {
        Box3 { min: *p, max: *p }
    }
    fn expand_point(&mut self, p: &[f64; 3]) {
        for d in 0..3 {
            if p[d] < self.min[d] {
                self.min[d] = p[d];
            }
            if p[d] > self.max[d] {
                self.max[d] = p[d];
            }
        }
    }
    fn expand_box(&mut self, b: &Box3) {
        for d in 0..3 {
            if b.min[d] < self.min[d] {
                self.min[d] = b.min[d];
            }
            if b.max[d] > self.max[d] {
                self.max[d] = b.max[d];
            }
        }
    }
}

enum Node {
    Internal(Vec<(Box3, usize)>),
    Leaf(Vec<usize>),
}

/// A packed, read-only R-tree over 3D points; values are indices into the point list it was built from.
pub struct PackedRtree3 {
    points: Vec<[f64; 3]>,
    nodes: Vec<Node>,
    root: Option<usize>,
    leafs_level: usize,
}

#[derive(Clone, Copy)]
struct SubtreeCounts {
    maxc: usize,
    minc: usize,
}

/// `comparable_distance(point, point)`: `dz² + (dy² + (dx² + 0))`.
#[inline]
fn point_point_distance(a: &[f64; 3], b: &[f64; 3]) -> f64 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    dz * dz + (dy * dy + (dx * dx + 0.0))
}

/// `comparable_distance(point, box)`: accumulated from the last dimension to the first.
#[inline]
fn point_box_distance(p: &[f64; 3], b: &Box3) -> f64 {
    let mut result = 0.0;
    for d in (0..3).rev() {
        if p[d] < b.min[d] {
            let diff = b.min[d] - p[d];
            result += diff * diff;
        }
        if p[d] > b.max[d] {
            let diff = p[d] - b.max[d];
            result += diff * diff;
        }
    }
    result
}

impl PackedRtree3 {
    /// `rtree(first, last)` (packing).
    pub fn new(points: Vec<[f64; 3]>) -> Self {
        let mut tree = PackedRtree3 { points, nodes: Vec::new(), root: None, leafs_level: 0 };
        let n = tree.points.len();
        if n == 0 {
            return tree;
        }
        let mut hint_box = Box3::from_point(&tree.points[0]);
        for p in tree.points.iter().skip(1) {
            hint_box.expand_point(p);
        }
        let mut entries: Vec<([f64; 3], usize)> = tree.points.iter().enumerate().map(|(i, p)| (*p, i)).collect();
        let (counts, leafs_level) = Self::calculate_subtree_elements_counts(n);
        tree.leafs_level = leafs_level;
        let (_, root) = tree.per_level(&mut entries, hint_box, n, counts);
        tree.root = Some(root);
        tree
    }

    fn calculate_subtree_elements_counts(elements_count: usize) -> (SubtreeCounts, usize) {
        let mut res = SubtreeCounts { maxc: 1, minc: 1 };
        let mut leafs_level = 0;
        let mut smax = MAX_ELEMENTS;
        while smax < elements_count {
            res.maxc = smax;
            smax *= MAX_ELEMENTS;
            leafs_level += 1;
        }
        res.minc = MIN_ELEMENTS * (res.maxc / MAX_ELEMENTS);
        (res, leafs_level)
    }

    fn calculate_median_count(count: usize, sc: SubtreeCounts) -> usize {
        let mut n = count / sc.maxc;
        let mut r = count % sc.maxc;
        let mut median_count = (n / 2) * sc.maxc;
        if r != 0 {
            if sc.minc <= r {
                median_count = ((n + 1) / 2) * sc.maxc;
            } else {
                let count_minus_min = count - sc.minc;
                n = count_minus_min / sc.maxc;
                r = count_minus_min % sc.maxc;
                if r == 0 {
                    median_count = ((n + 1) / 2) * sc.maxc;
                } else if n == 0 {
                    median_count = r;
                } else {
                    median_count = ((n + 2) / 2) * sc.maxc;
                }
            }
        }
        median_count
    }

    fn per_level(&mut self, entries: &mut [([f64; 3], usize)], hint_box: Box3, values_count: usize, sc: SubtreeCounts) -> (Box3, usize) {
        if sc.maxc <= 1 {
            let mut elements_box = Box3::from_point(&self.points[entries[0].1]);
            let mut values = Vec::with_capacity(entries.len());
            values.push(entries[0].1);
            for e in entries.iter().skip(1) {
                elements_box.expand_point(&self.points[e.1]);
                values.push(e.1);
            }
            self.nodes.push(Node::Leaf(values));
            return (elements_box, self.nodes.len() - 1);
        }
        let next = SubtreeCounts { maxc: sc.maxc / MAX_ELEMENTS, minc: sc.minc / MAX_ELEMENTS };
        let mut elements: Vec<(Box3, usize)> = Vec::new();
        let mut elements_box: Option<Box3> = None;
        self.per_level_packets(entries, hint_box, values_count, sc, next, &mut elements, &mut elements_box);
        self.nodes.push(Node::Internal(elements));
        (elements_box.unwrap(), self.nodes.len() - 1)
    }

    #[allow(clippy::too_many_arguments)]
    fn per_level_packets(
        &mut self,
        entries: &mut [([f64; 3], usize)],
        hint_box: Box3,
        values_count: usize,
        sc: SubtreeCounts,
        next: SubtreeCounts,
        elements: &mut Vec<(Box3, usize)>,
        elements_box: &mut Option<Box3>,
    ) {
        if values_count <= sc.maxc {
            let el = self.per_level(entries, hint_box, values_count, next);
            elements.push(el);
            match elements_box {
                None => *elements_box = Some(el.0),
                Some(b) => b.expand_box(&el.0),
            }
            return;
        }
        let median_count = Self::calculate_median_count(values_count, sc);
        // biggest_edge: strict comparison keeps the lowest dimension on ties.
        let mut greatest_dim_index = 0;
        let mut greatest_length = hint_box.max[0] - hint_box.min[0];
        for d in 1..3 {
            let curr = hint_box.max[d] - hint_box.min[d];
            if greatest_length < curr {
                greatest_dim_index = d;
                greatest_length = curr;
            }
        }
        let d = greatest_dim_index;
        nth_element_by(entries, median_count, |a, b| a.0[d] < b.0[d]);
        let mut left = hint_box;
        let mut right = hint_box;
        let mi = hint_box.min[d];
        let ma = hint_box.max[d];
        let center = mi + (ma - mi) / 2.0;
        left.max[d] = center;
        right.min[d] = center;
        let (lo, hi) = entries.split_at_mut(median_count);
        self.per_level_packets(lo, left, median_count, sc, next, elements, elements_box);
        self.per_level_packets(hi, right, values_count - median_count, sc, next, elements, elements_box);
    }

    /// `query(nearest(point, 1))`: index of the nearest value, ties resolved like Boost.
    pub fn nearest(&self, query: &[f64; 3]) -> Option<usize> {
        let root = self.root?;
        struct Branch {
            distance: f64,
            reverse_level: usize,
            node: usize,
        }
        // branch_data_comp: b1 "less than" b2 when farther, or equally far and higher up.
        let mut branches = PriorityQueue::new(|b1: &Branch, b2: &Branch| {
            b1.distance > b2.distance || (b1.distance == b2.distance && b1.reverse_level > b2.reverse_level)
        });
        let mut best: Option<(f64, usize)> = None;
        let ignore_branch = |best: &Option<(f64, usize)>, d: f64| -> bool {
            match best {
                Some((bd, _)) => *bd <= d,
                None => false,
            }
        };
        let mut node = root;
        let mut reverse_level = self.leafs_level;
        loop {
            match &self.nodes[node] {
                Node::Internal(children) => {
                    for (b, child) in children {
                        let d = point_box_distance(query, b);
                        if !ignore_branch(&best, d) {
                            branches.push(Branch { distance: d, reverse_level: reverse_level - 1, node: *child });
                        }
                    }
                }
                Node::Leaf(values) => {
                    for &v in values {
                        let d = point_point_distance(query, &self.points[v]);
                        match best {
                            None => best = Some((d, v)),
                            Some((bd, _)) => {
                                if d < bd {
                                    best = Some((d, v));
                                }
                            }
                        }
                    }
                }
            }
            match branches.top() {
                None => break,
                Some(top) => {
                    if ignore_branch(&best, top.distance) {
                        break;
                    }
                }
            }
            let top = branches.pop().unwrap();
            node = top.node;
            reverse_level = top.reverse_level;
        }
        best.map(|b| b.1)
    }
}
