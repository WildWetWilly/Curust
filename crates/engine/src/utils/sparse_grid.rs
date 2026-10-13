//! `utils/SquareGrid.h`, `SparseGrid.h`, `SparsePointGrid.h`, `SparseLineGrid.h`,
//! `SparsePointGridInclusive.h`.
//!
//! The C++ SparseGrid stores its elements in a `std::unordered_multimap<GridPoint, Elem>`. With
//! libstdc++, elements with the same key are visited in *reverse* insertion order by
//! `equal_range`, with Microsoft's STL in insertion order; this implementation stores each cell as
//! a vector and iterates it in the order of the emulated library ([`cppstd::flavor`]) so that
//! "first found" tie-breaking is identical.

use crate::geometry::point2ll::{pt, shorter_then, v_size2, Point2LL};
use crate::utils::coord::Coord;
use std::collections::HashMap;

pub type GridPoint = Point2LL;
pub type GridCoord = Coord;

#[derive(Clone, Copy, Debug)]
pub struct SquareGrid {
    pub cell_size: Coord,
}

impl SquareGrid {
    pub fn new(cell_size: Coord) -> Self {
        debug_assert!(cell_size > 0);
        SquareGrid { cell_size }
    }

    pub fn get_cell_size(&self) -> Coord {
        self.cell_size
    }

    #[inline]
    pub fn to_grid_point(&self, point: Point2LL) -> GridPoint {
        pt(self.to_grid_coord(point.x), self.to_grid_coord(point.y))
    }

    #[inline]
    pub fn to_grid_coord(&self, coord: Coord) -> GridCoord {
        // This mapping via truncation results in the cells with
        // GridPoint.x==0 being twice as large and similarly for
        // GridPoint.y==0.  This doesn't cause any incorrect behavior,
        // just changes the running time slightly.  The change in
        // running time from this is probably not worth doing a proper
        // floor operation.
        coord / self.cell_size
    }

    #[inline]
    pub fn to_lower_corner(&self, location: GridPoint) -> Point2LL {
        pt(self.to_lower_coord(location.x), self.to_lower_coord(location.y))
    }

    #[inline]
    pub fn to_lower_coord(&self, grid_coord: GridCoord) -> Coord {
        // This mapping via truncation results in the cells with
        // GridPoint.x==0 being twice as large and similarly for
        // GridPoint.y==0.  This doesn't cause any incorrect behavior,
        // just changes the running time slightly.  The change in
        // running time from this is probably not worth doing a proper
        // floor operation.
        grid_coord * self.cell_size
    }

    #[inline]
    pub fn nonzero_sign(z: GridCoord) -> GridCoord {
        (z >= 0) as GridCoord - (z < 0) as GridCoord
    }

    pub fn process_line_cells<F: FnMut(GridPoint) -> bool>(&self, line: (Point2LL, Point2LL), mut process_cell_func: F) -> bool {
        let mut start = line.0;
        let mut end = line.1;
        if end.x < start.x {
            // make sure X increases between start and end
            std::mem::swap(&mut start, &mut end);
        }

        let start_cell = self.to_grid_point(start);
        let end_cell = self.to_grid_point(end);
        let y_diff = end.y - start.y;
        let y_dir = Self::nonzero_sign(y_diff);

        /* This line drawing algorithm iterates over the range of Y coordinates, and
        for each Y coordinate computes the range of X coordinates crossed in one
        unit of Y. These ranges are rounded to be inclusive, so effectively this
        creates a "fat" line, marking more cells than a strict one-cell-wide path.*/
        let mut x_cell_start = start_cell.x;
        let mut cell_y = start_cell.y;
        while cell_y * y_dir <= end_cell.y * y_dir {
            // for all Y from start to end
            // nearest y coordinate of the cells in the next row
            let nearest_next_y = self.to_lower_coord(cell_y + if Self::nonzero_sign(cell_y) == y_dir || cell_y == 0 { y_dir } else { 0 });
            let mut x_cell_end; // the X coord of the last cell to include from this row
            if y_diff == 0 {
                x_cell_end = end_cell.x;
            } else {
                let area = (end.x - start.x) * (nearest_next_y - start.y);
                // corresponding_x: the x coordinate corresponding to nearest_next_y
                let corresponding_x = start.x + area / y_diff;
                x_cell_end = self.to_grid_coord(corresponding_x + ((corresponding_x < 0) && ((area % y_diff) != 0)) as Coord);
                if x_cell_end < start_cell.x {
                    // process at least one cell!
                    x_cell_end = x_cell_start;
                }
            }

            let mut cell_x = x_cell_start;
            while cell_x <= x_cell_end {
                let grid_loc = pt(cell_x, cell_y);
                if !process_cell_func(grid_loc) {
                    return false;
                }
                if grid_loc == end_cell {
                    return true;
                }
                cell_x += 1;
            }
            // TODO: this causes at least a one cell overlap for each row, which
            // includes extra cells when crossing precisely on the corners
            // where positive slope where x > 0 and negative slope where x < 0
            x_cell_start = x_cell_end;
            cell_y += y_dir;
        }
        debug_assert!(false, "We should have returned already before here!");
        false
    }

    pub fn process_axis_aligned_triangle_dir<F: FnMut(GridPoint) -> bool>(&self, from: Point2LL, to: Point2LL, to_the_right: bool, process_cell_func: F) -> bool {
        let mut a = from;
        let mut b = to;
        if ((a.x < b.x) == (a.y < b.y)) != to_the_right {
            std::mem::swap(&mut a, &mut b);
        }
        self.process_axis_aligned_triangle(a, b, process_cell_func)
    }

    pub fn process_axis_aligned_triangle<F: FnMut(GridPoint) -> bool>(&self, from: Point2LL, to: Point2LL, mut process_cell_func: F) -> bool {
        let mut last = GridPoint::default();
        let grid_to = self.to_grid_point(to);
        self.process_line_cells((from, to), |grid_loc: GridPoint| {
            if grid_loc.y != last.y {
                let sign = Self::nonzero_sign(grid_to.x - grid_loc.x);
                let mut x = grid_loc.x;
                while x * sign <= grid_to.x * sign {
                    if !process_cell_func(pt(x, grid_loc.y)) {
                        return false;
                    }
                    x += sign;
                }
            } else if !process_cell_func(grid_loc) {
                return false;
            }
            last = grid_loc;
            true
        })
    }

    pub fn process_nearby<F: FnMut(GridPoint) -> bool>(&self, query_pt: Point2LL, radius: Coord, mut process_func: F) -> bool {
        let min_loc = pt(query_pt.x - radius, query_pt.y - radius);
        let max_loc = pt(query_pt.x + radius, query_pt.y + radius);

        let min_grid = self.to_grid_point(min_loc);
        let max_grid = self.to_grid_point(max_loc);

        for grid_y in min_grid.y..=max_grid.y {
            for grid_x in min_grid.x..=max_grid.x {
                let grid_pt = pt(grid_x, grid_y);
                if !process_func(grid_pt) {
                    return false;
                }
            }
        }
        true
    }
}

/// The elements stored under one key of an `unordered_multimap`, in `equal_range` order.
pub fn multimap_order<E>(v: &[E]) -> Box<dyn Iterator<Item = &E> + '_> {
    if cppstd::flavor::msvc() {
        Box::new(v.iter())
    } else {
        Box::new(v.iter().rev())
    }
}

/// `SparseGrid<ElemT>`.
#[derive(Clone, Debug)]
pub struct SparseGrid<E> {
    pub square: SquareGrid,
    pub grid: HashMap<GridPoint, Vec<E>>,
}

impl<E> SparseGrid<E> {
    pub fn new(cell_size: Coord) -> Self {
        SparseGrid { square: SquareGrid::new(cell_size), grid: HashMap::new() }
    }

    pub fn with_capacity(cell_size: Coord, elem_reserve: usize) -> Self {
        SparseGrid { square: SquareGrid::new(cell_size), grid: HashMap::with_capacity(elem_reserve) }
    }

    #[inline]
    pub fn cell_size(&self) -> Coord {
        self.square.cell_size
    }

    /// `grid_.emplace(grid_loc, elem)`.
    #[inline]
    pub fn insert_at(&mut self, grid_loc: GridPoint, elem: E) {
        self.grid.entry(grid_loc).or_default().push(elem);
    }

    /// Visit the elements of one cell in `equal_range` order (libstdc++: newest first; Microsoft
    /// STL: oldest first).
    #[inline]
    pub fn process_from_cell<F: FnMut(&E) -> bool>(&self, grid_pt: GridPoint, process_func: &mut F) -> bool {
        if let Some(v) = self.grid.get(&grid_pt) {
            for e in multimap_order(v) {
                if !process_func(e) {
                    return false;
                }
            }
        }
        true
    }

    pub fn process_nearby<F: FnMut(&E) -> bool>(&self, query_pt: Point2LL, radius: Coord, mut process_func: F) -> bool {
        self.square.process_nearby(query_pt, radius, |grid_pt| self.process_from_cell(grid_pt, &mut process_func))
    }

    pub fn process_line<F: FnMut(&E) -> bool>(&self, query_line: (Point2LL, Point2LL), mut process_elem_func: F) -> bool {
        self.square.process_line_cells(query_line, |grid_loc| self.process_from_cell(grid_loc, &mut process_elem_func))
    }

    pub fn get_nearby(&self, query_pt: Point2LL, radius: Coord) -> Vec<E>
    where
        E: Clone,
    {
        let mut ret = Vec::new();
        self.process_nearby(query_pt, radius, |elem| {
            ret.push(elem.clone());
            true
        });
        ret
    }

    /// Number of stored elements (counting duplicates in multiple cells).
    pub fn len(&self) -> usize {
        self.grid.values().map(|v| v.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.grid.is_empty()
    }
}

/// `SparseGrid::getNearest` for elements that expose a point.
pub fn get_nearest<E: Clone, P: Fn(&E) -> Point2LL, C: Fn(&E) -> bool>(
    grid: &SparseGrid<E>,
    query_pt: Point2LL,
    radius: Coord,
    point_of: P,
    precondition: C,
) -> Option<E> {
    let mut found: Option<E> = None;
    let mut best_dist2 = radius * radius;
    grid.process_nearby(query_pt, radius, |elem| {
        if !precondition(elem) {
            return true;
        }
        let dist2 = v_size2(point_of(elem) - query_pt);
        if dist2 < best_dist2 {
            found = Some(elem.clone());
            best_dist2 = dist2;
        }
        true
    });
    found
}

/// `SparsePointGrid<ElemT, Locator>`: a grid of elements located at points.
#[derive(Clone, Debug)]
pub struct SparsePointGrid<E, L: Fn(&E) -> Point2LL> {
    pub base: SparseGrid<E>,
    pub locator: L,
}

impl<E, L: Fn(&E) -> Point2LL> SparsePointGrid<E, L> {
    pub fn new(cell_size: Coord, elem_reserve: usize, locator: L) -> Self {
        SparsePointGrid { base: SparseGrid::with_capacity(cell_size, elem_reserve), locator }
    }

    pub fn insert(&mut self, elem: E) {
        let loc = (self.locator)(&elem);
        let grid_loc = self.base.square.to_grid_point(loc);
        self.base.insert_at(grid_loc, elem);
    }

    pub fn get_any_nearby(&self, query_pt: Point2LL, radius: Coord) -> Option<&E> {
        let mut ret: Option<&E> = None;
        let locator = &self.locator;
        // Note: the closure returns references into the grid, so it is implemented directly.
        let square = &self.base.square;
        square.process_nearby(query_pt, radius, |grid_pt| {
            if let Some(v) = self.base.grid.get(&grid_pt) {
                for e in multimap_order(v) {
                    if shorter_then(locator(e) - query_pt, radius) {
                        ret = Some(e);
                        return false;
                    }
                }
            }
            true
        });
        ret
    }

    pub fn process_nearby<F: FnMut(&E) -> bool>(&self, query_pt: Point2LL, radius: Coord, process_func: F) -> bool {
        self.base.process_nearby(query_pt, radius, process_func)
    }

    pub fn get_nearby(&self, query_pt: Point2LL, radius: Coord) -> Vec<E>
    where
        E: Clone,
    {
        self.base.get_nearby(query_pt, radius)
    }
}

/// `SparseLineGrid<ElemT, Locator>`: a grid of elements located along line segments.
#[derive(Clone, Debug)]
pub struct SparseLineGrid<E, L: Fn(&E) -> (Point2LL, Point2LL)> {
    pub base: SparseGrid<E>,
    pub locator: L,
}

impl<E: Clone, L: Fn(&E) -> (Point2LL, Point2LL)> SparseLineGrid<E, L> {
    pub fn new(cell_size: Coord, elem_reserve: usize, locator: L) -> Self {
        SparseLineGrid { base: SparseGrid::with_capacity(cell_size, elem_reserve), locator }
    }

    /// `getCellSize()`
    pub fn cell_size(&self) -> Coord {
        self.base.cell_size()
    }

    pub fn insert(&mut self, elem: E) {
        let line = (self.locator)(&elem);
        // below is a workaround for the fact that lambda functions cannot be used to initialize function pointers.
        let square = self.base.square;
        let grid = &mut self.base.grid;
        square.process_line_cells(line, |grid_loc| {
            grid.entry(grid_loc).or_default().push(elem.clone());
            true
        });
    }

    pub fn process_nearby<F: FnMut(&E) -> bool>(&self, query_pt: Point2LL, radius: Coord, process_func: F) -> bool {
        self.base.process_nearby(query_pt, radius, process_func)
    }

    pub fn process_line<F: FnMut(&E) -> bool>(&self, query_line: (Point2LL, Point2LL), process_func: F) -> bool {
        self.base.process_line(query_line, process_func)
    }

    pub fn get_nearby(&self, query_pt: Point2LL, radius: Coord) -> Vec<E> {
        self.base.get_nearby(query_pt, radius)
    }
}

/// `SparsePointGridInclusiveElem<Val>`.
#[derive(Clone, Debug, Default)]
pub struct SparsePointGridInclusiveElem<V> {
    pub point: Point2LL,
    pub val: V,
}

/// `SparsePointGridInclusive<Val>`.
#[derive(Clone, Debug)]
pub struct SparsePointGridInclusive<V> {
    pub base: SparseGrid<SparsePointGridInclusiveElem<V>>,
}

impl<V: Clone> SparsePointGridInclusive<V> {
    pub fn new(cell_size: Coord, elem_reserve: usize) -> Self {
        SparsePointGridInclusive { base: SparseGrid::with_capacity(cell_size, elem_reserve) }
    }

    pub fn insert(&mut self, point: Point2LL, val: V) {
        let grid_loc = self.base.square.to_grid_point(point);
        self.base.insert_at(grid_loc, SparsePointGridInclusiveElem { point, val });
    }

    pub fn get_nearby_vals(&self, query_pt: Point2LL, radius: Coord) -> Vec<V> {
        let mut ret = Vec::new();
        self.base.process_nearby(query_pt, radius, |elem| {
            ret.push(elem.val.clone());
            true
        });
        ret
    }

    pub fn get_nearby(&self, query_pt: Point2LL, radius: Coord) -> Vec<SparsePointGridInclusiveElem<V>> {
        self.base.get_nearby(query_pt, radius)
    }

    pub fn get_nearest<C: Fn(&SparsePointGridInclusiveElem<V>) -> bool>(
        &self,
        query_pt: Point2LL,
        radius: Coord,
        precondition: C,
    ) -> Option<SparsePointGridInclusiveElem<V>> {
        get_nearest(&self.base, query_pt, radius, |e| e.point, precondition)
    }

    pub fn process_nearby<F: FnMut(&SparsePointGridInclusiveElem<V>) -> bool>(&self, query_pt: Point2LL, radius: Coord, process_func: F) -> bool {
        self.base.process_nearby(query_pt, radius, process_func)
    }
}
