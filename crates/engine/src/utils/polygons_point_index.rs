//! `PolygonsPointIndex.h` and `PolygonsSegmentIndex.h`: a reference to a vertex in a set of paths.
//!
//! Like the C++ code this stores a raw pointer to the container; the container must outlive
//! (and not move during) the use of the index.

use crate::geometry::lines_set::LinesSet;
use crate::geometry::point2ll::Point2LL;
use crate::geometry::{Polygon, Polyline, Shape};
use crate::utils::extrusion_line::ExtrusionLine;
use std::hash::{Hash, Hasher};

/// Access to the vertices of a set of paths.
pub trait PathsAccess {
    fn path_len(&self, poly_idx: usize) -> usize;
    fn point_at(&self, poly_idx: usize, point_idx: usize) -> Point2LL;
}

impl<L: Polyline> PathsAccess for LinesSet<L> {
    #[inline]
    fn path_len(&self, poly_idx: usize) -> usize {
        self.lines[poly_idx].points().len()
    }
    #[inline]
    fn point_at(&self, poly_idx: usize, point_idx: usize) -> Point2LL {
        self.lines[poly_idx].points()[point_idx]
    }
}

impl PathsAccess for Vec<ExtrusionLine> {
    #[inline]
    fn path_len(&self, poly_idx: usize) -> usize {
        self[poly_idx].junctions.len()
    }
    #[inline]
    fn point_at(&self, poly_idx: usize, point_idx: usize) -> Point2LL {
        self[poly_idx].junctions[point_idx].p
    }
}

impl PathsAccess for Vec<Vec<Point2LL>> {
    #[inline]
    fn path_len(&self, poly_idx: usize) -> usize {
        self[poly_idx].len()
    }
    #[inline]
    fn point_at(&self, poly_idx: usize, point_idx: usize) -> Point2LL {
        self[poly_idx][point_idx]
    }
}

/// `PathsPointIndex<Paths>`.
pub struct PathsPointIndex<P> {
    pub polygons: *const P,
    pub poly_idx: usize,
    pub point_idx: usize,
}

unsafe impl<P> Send for PathsPointIndex<P> {}
unsafe impl<P> Sync for PathsPointIndex<P> {}

impl<P> Clone for PathsPointIndex<P> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<P> Copy for PathsPointIndex<P> {}

impl<P> std::fmt::Debug for PathsPointIndex<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PathsPointIndex({:p}, {}, {})", self.polygons, self.poly_idx, self.point_idx)
    }
}

impl<P> Default for PathsPointIndex<P> {
    fn default() -> Self {
        PathsPointIndex { polygons: std::ptr::null(), poly_idx: 0, point_idx: 0 }
    }
}

impl<P> PartialEq for PathsPointIndex<P> {
    fn eq(&self, other: &Self) -> bool {
        self.polygons == other.polygons && self.poly_idx == other.poly_idx && self.point_idx == other.point_idx
    }
}
impl<P> Eq for PathsPointIndex<P> {}

impl<P: PathsAccess> Hash for PathsPointIndex<P> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // std::hash<PolygonsPointIndex> hashes the point location.
        let p = self.p();
        p.x.hash(state);
        p.y.hash(state);
    }
}

impl<P> PathsPointIndex<P> {
    pub fn new(polygons: *const P, poly_idx: usize, point_idx: usize) -> Self {
        PathsPointIndex { polygons, poly_idx, point_idx }
    }

    pub fn initialized(&self) -> bool {
        !self.polygons.is_null()
    }

    /// # Safety
    /// The referenced container must still be alive.
    #[inline]
    pub fn polygons(&self) -> &P {
        unsafe { &*self.polygons }
    }
}

impl<P: PathsAccess> PathsPointIndex<P> {
    pub fn p(&self) -> Point2LL {
        if self.polygons.is_null() {
            return Point2LL::new(0, 0);
        }
        self.polygons().point_at(self.poly_idx, self.point_idx)
    }

    /// `operator++`
    pub fn inc(&mut self) {
        self.point_idx = (self.point_idx + 1) % self.polygons().path_len(self.poly_idx);
    }

    /// `operator--`
    pub fn dec(&mut self) {
        if self.point_idx == 0 {
            self.point_idx = self.polygons().path_len(self.poly_idx);
        }
        self.point_idx -= 1;
    }

    pub fn next(&self) -> Self {
        let mut ret = *self;
        ret.inc();
        ret
    }

    pub fn prev(&self) -> Self {
        let mut ret = *self;
        ret.dec();
        ret
    }

    /// `operator<`: compares the point locations.
    pub fn less(&self, other: &Self) -> bool {
        crate::geometry::point2ll::point_less(self.p(), other.p())
    }
}

impl PathsPointIndex<Shape> {
    pub fn get_polygon(&self) -> &Polygon {
        &self.polygons().lines[self.poly_idx]
    }
}

pub type PolygonsPointIndex = PathsPointIndex<Shape>;

/// `PolygonsPointIndexSegmentLocator`.
pub fn polygons_point_index_segment_locator(val: &PolygonsPointIndex) -> (Point2LL, Point2LL) {
    let poly = &val.polygons().lines[val.poly_idx];
    let start = poly[val.point_idx];
    let next_point_idx = (val.point_idx + 1) % poly.len();
    let end = poly[next_point_idx];
    (start, end)
}

/// `PathsPointIndexLocator`.
pub fn paths_point_index_locator<P: PathsAccess>(val: &PathsPointIndex<P>) -> Point2LL {
    val.p()
}

/// `PolygonsSegmentIndex`.
pub type PolygonsSegmentIndex = PolygonsPointIndex;

impl PathsPointIndex<Shape> {
    pub fn from(&self) -> Point2LL {
        self.p()
    }
    pub fn to(&self) -> Point2LL {
        self.next().p()
    }
}
