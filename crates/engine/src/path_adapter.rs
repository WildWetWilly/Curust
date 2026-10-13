//! `PathAdapter.h/.cpp`: gives uniform access to the points and line widths of either an
//! `ExtrusionLine` or a `Polygon` (with a fixed line width).

use crate::geometry::point2ll::{v_size, Point2LL};
use crate::utils::coord::Coord;
use crate::utils::extrusion_line::ExtrusionLine;

#[derive(Clone, Copy)]
pub enum PathAdapter<'p> {
    /// `PathAdapter<ExtrusionLine>`
    Extrusion(&'p ExtrusionLine),
    /// `PathAdapter<Polygon>`: the points of a closed polygon (without explicit closing point) and its fixed line width.
    Polygon(&'p [Point2LL], Coord),
}

impl<'p> PathAdapter<'p> {
    pub fn is_extrusion_line(&self) -> bool {
        matches!(self, PathAdapter::Extrusion(_))
    }

    pub fn empty(&self) -> bool {
        self.size() == 0
    }

    pub fn size(&self) -> usize {
        match self {
            PathAdapter::Extrusion(l) => l.junctions.len(),
            PathAdapter::Polygon(p, _) => p.len(),
        }
    }

    /// `path_.length()`: for a polygon this includes the closing segment.
    pub fn length(&self) -> Coord {
        match self {
            PathAdapter::Extrusion(l) => l.length(),
            PathAdapter::Polygon(p, _) => {
                if p.is_empty() {
                    return 0;
                }
                let mut len: Coord = 0;
                let mut prev = *p.last().unwrap();
                for &next in p.iter() {
                    len += v_size(next - prev);
                    prev = next;
                }
                len
            }
        }
    }

    pub fn point_at(&self, index: usize) -> Point2LL {
        match self {
            PathAdapter::Extrusion(l) => l.junctions[index].p,
            PathAdapter::Polygon(p, _) => p[index],
        }
    }

    pub fn line_width_at(&self, index: usize) -> Coord {
        match self {
            PathAdapter::Extrusion(l) => l.junctions[index].w,
            PathAdapter::Polygon(_, w) => *w,
        }
    }

    /// `getPath()` for the extrusion line variant.
    pub fn extrusion_line(&self) -> Option<&'p ExtrusionLine> {
        match self {
            PathAdapter::Extrusion(l) => Some(l),
            PathAdapter::Polygon(..) => None,
        }
    }
}
