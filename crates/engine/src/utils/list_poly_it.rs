//! `utils/ListPolyIt.h`: a polygon stored as a (cyclic) linked list, with stable iterators.
//!
//! The C++ code uses `std::list<Point2LL>` with an iterator wrapper that wraps around at the
//! end. Here the list is an index arena; an iterator is the index of a node.

use crate::geometry::lines_set::Shape;
use crate::geometry::point2ll::Point2LL;
use crate::geometry::polyline::Polygon;

const NIL: usize = usize::MAX;

/// An iterator into a [`ListPolygon`] (node index).
pub type ListPolyIt = usize;

#[derive(Clone, Copy, Debug)]
struct Node {
    p: Point2LL,
    prev: usize,
    next: usize,
}

#[derive(Clone, Debug, Default)]
pub struct ListPolygon {
    nodes: Vec<Node>,
    head: usize,
    len: usize,
}

impl ListPolygon {
    pub fn new() -> Self {
        ListPolygon { nodes: Vec::new(), head: NIL, len: 0 }
    }

    pub fn from_polygon(poly: &[Point2LL]) -> Self {
        let mut l = ListPolygon::new();
        for &p in poly {
            l.push_back(p);
        }
        l
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// `poly.begin()` (NIL if empty).
    #[inline]
    pub fn begin(&self) -> ListPolyIt {
        self.head
    }

    pub fn push_back(&mut self, p: Point2LL) -> ListPolyIt {
        let idx = self.nodes.len();
        if self.head == NIL {
            self.nodes.push(Node { p, prev: idx, next: idx });
            self.head = idx;
        } else {
            let tail = self.nodes[self.head].prev;
            self.nodes.push(Node { p, prev: tail, next: self.head });
            self.nodes[tail].next = idx;
            let h = self.head;
            self.nodes[h].prev = idx;
        }
        self.len += 1;
        idx
    }

    #[inline]
    pub fn p(&self, it: ListPolyIt) -> Point2LL {
        self.nodes[it].p
    }

    #[inline]
    pub fn set_p(&mut self, it: ListPolyIt, p: Point2LL) {
        self.nodes[it].p = p;
    }

    /// `ListPolyIt::next()` (wraps around).
    #[inline]
    pub fn next(&self, it: ListPolyIt) -> ListPolyIt {
        self.nodes[it].next
    }

    /// `ListPolyIt::prev()` (wraps around).
    #[inline]
    pub fn prev(&self, it: ListPolyIt) -> ListPolyIt {
        self.nodes[it].prev
    }

    /// `ListPolyIt::remove()`: erase the node from the list.
    pub fn remove(&mut self, it: ListPolyIt) {
        let Node { prev, next, .. } = self.nodes[it];
        self.len -= 1;
        if self.len == 0 {
            self.head = NIL;
            return;
        }
        self.nodes[prev].next = next;
        self.nodes[next].prev = prev;
        if self.head == it {
            self.head = next;
        }
    }

    /// `std::list::insert(pos, value)`: insert before `pos`, returns the new node.
    pub fn insert_before(&mut self, pos: ListPolyIt, p: Point2LL) -> ListPolyIt {
        let idx = self.nodes.len();
        let prev = self.nodes[pos].prev;
        self.nodes.push(Node { p, prev, next: pos });
        self.nodes[prev].next = idx;
        self.nodes[pos].prev = idx;
        if pos == self.head {
            self.head = idx;
        }
        self.len += 1;
        idx
    }

    /// Iterate from begin to end.
    pub fn iter(&self) -> impl Iterator<Item = Point2LL> + '_ {
        let mut it = self.head;
        let mut remaining = self.len;
        std::iter::from_fn(move || {
            if remaining == 0 {
                return None;
            }
            remaining -= 1;
            let p = self.nodes[it].p;
            it = self.nodes[it].next;
            Some(p)
        })
    }

    /// `convertListPolygonToPolygon`: appends the points to `polygon`.
    pub fn to_polygon(&self, polygon: &mut Polygon) {
        for p in self.iter() {
            polygon.push(p);
        }
    }
}

/// `ListPolyIt::insertPointNonDuplicate(before, after, to_insert)`.
pub fn insert_point_non_duplicate(poly: &mut ListPolygon, before: ListPolyIt, after: ListPolyIt, to_insert: Point2LL) -> ListPolyIt {
    if to_insert == poly.p(before) {
        before
    } else if to_insert == poly.p(after) {
        after
    } else {
        poly.insert_before(after, to_insert)
    }
}

/// `ListPolyIt::convertPolygonsToLists`.
pub fn convert_polygons_to_lists(shape: &Shape) -> Vec<ListPolygon> {
    shape.iter().map(|p| ListPolygon::from_polygon(p)).collect()
}

/// `ListPolyIt::convertListPolygonsToPolygons`: replaces the points of each polygon in `polygons`.
pub fn convert_list_polygons_to_polygons(list_polygons: &[ListPolygon], polygons: &mut Shape) {
    for poly_idx in 0..polygons.len() {
        polygons[poly_idx].clear();
        list_polygons[poly_idx].to_polygon(&mut polygons[poly_idx]);
    }
}
