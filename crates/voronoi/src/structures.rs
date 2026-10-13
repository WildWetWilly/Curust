//! Port of `boost/polygon/detail/voronoi_structures.hpp` and `voronoi_geometry_type.hpp`.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum SourceCategory {
    SinglePoint = 0x0,
    SegmentStartPoint = 0x1,
    SegmentEndPoint = 0x2,
    InitialSegment = 0x8,
    ReverseSegment = 0x9,
}

pub const SOURCE_CATEGORY_GEOMETRY_SHIFT: usize = 0x3;
pub const SOURCE_CATEGORY_BITMASK: usize = 0x1F;

impl SourceCategory {
    pub fn from_bits(bits: usize) -> SourceCategory {
        match bits & SOURCE_CATEGORY_BITMASK {
            0x0 => SourceCategory::SinglePoint,
            0x1 => SourceCategory::SegmentStartPoint,
            0x2 => SourceCategory::SegmentEndPoint,
            0x8 => SourceCategory::InitialSegment,
            0x9 => SourceCategory::ReverseSegment,
            other => panic!("invalid voronoi source category {other}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeometryCategory {
    Point = 0x0,
    Segment = 0x1,
}

/// `belongs(source_category, geometry_category)`.
pub fn belongs(source_category: SourceCategory, geometry_category: GeometryCategory) -> bool {
    (source_category as usize >> SOURCE_CATEGORY_GEOMETRY_SHIFT) == geometry_category as usize
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Point2d {
    pub x: i32,
    pub y: i32,
}

impl Point2d {
    #[inline]
    pub fn new(x: i32, y: i32) -> Self {
        Point2d { x, y }
    }
    #[inline]
    pub fn x(&self) -> i32 {
        self.x
    }
    #[inline]
    pub fn y(&self) -> i32 {
        self.y
    }
}

const IS_INVERSE: usize = 0x20;

/// `site_event<int32>`.
#[derive(Clone, Copy, Debug)]
pub struct SiteEvent {
    pub point0: Point2d,
    pub point1: Point2d,
    pub sorted_index: usize,
    pub initial_index: usize,
    pub flags: usize,
}

impl PartialEq for SiteEvent {
    fn eq(&self, that: &Self) -> bool {
        self.point0 == that.point0 && self.point1 == that.point1
    }
}

impl SiteEvent {
    pub fn from_point(p: Point2d) -> Self {
        SiteEvent { point0: p, point1: p, sorted_index: 0, initial_index: 0, flags: 0 }
    }

    pub fn from_points(p1: Point2d, p2: Point2d) -> Self {
        SiteEvent { point0: p1, point1: p2, sorted_index: 0, initial_index: 0, flags: 0 }
    }

    #[inline]
    pub fn x(&self) -> i32 {
        self.point0.x
    }
    #[inline]
    pub fn y(&self) -> i32 {
        self.point0.y
    }
    #[inline]
    pub fn x0(&self) -> i32 {
        self.point0.x
    }
    #[inline]
    pub fn y0(&self) -> i32 {
        self.point0.y
    }
    #[inline]
    pub fn x1(&self) -> i32 {
        self.point1.x
    }
    #[inline]
    pub fn y1(&self) -> i32 {
        self.point1.y
    }

    #[inline]
    pub fn is_inverse(&self) -> bool {
        self.flags & IS_INVERSE != 0
    }

    #[inline]
    pub fn inverse(&mut self) -> &mut Self {
        std::mem::swap(&mut self.point0, &mut self.point1);
        self.flags ^= IS_INVERSE;
        self
    }

    #[inline]
    pub fn source_category(&self) -> SourceCategory {
        SourceCategory::from_bits(self.flags & SOURCE_CATEGORY_BITMASK)
    }

    #[inline]
    pub fn set_source_category(&mut self, c: SourceCategory) {
        self.flags |= c as usize;
    }

    #[inline]
    pub fn is_point(&self) -> bool {
        self.point0.x == self.point1.x && self.point0.y == self.point1.y
    }

    #[inline]
    pub fn is_segment(&self) -> bool {
        self.point0.x != self.point1.x || self.point0.y != self.point1.y
    }
}

/// `circle_event<double>`.
#[derive(Clone, Copy, Debug)]
pub struct CircleEvent {
    pub center_x: f64,
    pub center_y: f64,
    pub lower_x: f64,
    pub is_active: bool,
}

impl Default for CircleEvent {
    fn default() -> Self {
        CircleEvent { center_x: 0.0, center_y: 0.0, lower_x: 0.0, is_active: true }
    }
}

impl CircleEvent {
    pub fn new(c_x: f64, c_y: f64, lower_x: f64) -> Self {
        CircleEvent { center_x: c_x, center_y: c_y, lower_x, is_active: true }
    }
    #[inline]
    pub fn x(&self) -> f64 {
        self.center_x
    }
    #[inline]
    pub fn y(&self) -> f64 {
        self.center_y
    }
    #[inline]
    pub fn lower_x(&self) -> f64 {
        self.lower_x
    }
    #[inline]
    pub fn lower_y(&self) -> f64 {
        self.center_y
    }
}

/// `beach_line_node_key`.
#[derive(Clone, Copy, Debug)]
pub struct BeachLineKey {
    pub left_site: SiteEvent,
    pub right_site: SiteEvent,
}

impl BeachLineKey {
    pub fn single(new_site: SiteEvent) -> Self {
        BeachLineKey { left_site: new_site, right_site: new_site }
    }
    pub fn new(left_site: SiteEvent, right_site: SiteEvent) -> Self {
        BeachLineKey { left_site, right_site }
    }
}

pub const NIL: usize = usize::MAX;

/// `beach_line_node_data`: index of the circle event (into the builder's event arena) and of the
/// half-edge in the output diagram.
#[derive(Clone, Copy, Debug)]
pub struct BeachLineData {
    pub circle_event: usize,
    pub edge: usize,
}

impl BeachLineData {
    pub fn new(edge: usize) -> Self {
        BeachLineData { circle_event: NIL, edge }
    }
}
