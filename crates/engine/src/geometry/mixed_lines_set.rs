//! `geometry/MixedLinesSet.h`: a set of polylines of any type.

use super::lines_set::{ClosedLinesSet, OpenLinesSet, Shape};
use super::polyline::{ClosedPolyline, OpenPolyline, Polygon, Polyline};
use crate::utils::coord::Coord;
use clipper::{ClipperOffset, EndType, JoinType};
use std::sync::Arc;

/// Any polyline (replaces `std::shared_ptr<Polyline>` + dynamic casts).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnyPolyline {
    Open(OpenPolyline),
    Closed(ClosedPolyline),
    Polygon(Polygon),
}

impl AnyPolyline {
    pub fn points(&self) -> &Vec<clipper::IntPoint> {
        match self {
            AnyPolyline::Open(l) => &l.points,
            AnyPolyline::Closed(l) => &l.points,
            AnyPolyline::Polygon(l) => &l.points,
        }
    }
    pub fn has_closing_segment(&self) -> bool {
        match self {
            AnyPolyline::Open(l) => l.has_closing_segment(),
            AnyPolyline::Closed(l) => l.has_closing_segment(),
            AnyPolyline::Polygon(l) => l.has_closing_segment(),
        }
    }
    pub fn length(&self) -> Coord {
        match self {
            AnyPolyline::Open(l) => l.length(),
            AnyPolyline::Closed(l) => l.length(),
            AnyPolyline::Polygon(l) => l.length(),
        }
    }
    pub fn as_polygon(&self) -> Option<&Polygon> {
        match self {
            AnyPolyline::Polygon(p) => Some(p),
            _ => None,
        }
    }
}

pub type PolylinePtr = Arc<AnyPolyline>;

#[derive(Clone, Debug, Default)]
pub struct MixedLinesSet {
    pub lines: Vec<PolylinePtr>,
}

impl std::ops::Deref for MixedLinesSet {
    type Target = Vec<PolylinePtr>;
    fn deref(&self) -> &Vec<PolylinePtr> {
        &self.lines
    }
}
impl std::ops::DerefMut for MixedLinesSet {
    fn deref_mut(&mut self) -> &mut Vec<PolylinePtr> {
        &mut self.lines
    }
}

impl MixedLinesSet {
    pub fn offset(&self, distance: Coord, join_type: JoinType, miter_limit: f64) -> Shape {
        if distance == 0 {
            // Return a shape that contains only actual polygons
            let mut result = Shape::default();
            for line in &self.lines {
                if let Some(polygon) = line.as_polygon() {
                    result.push(polygon.clone());
                }
            }
            return result;
        }

        let mut polygons = Shape::default();
        let mut clipper = ClipperOffset::new(miter_limit, 10.0);

        for line in &self.lines {
            if let Some(polygon) = line.as_polygon() {
                // Union all polygons first and add them later
                polygons.push(polygon.clone());
            } else {
                let end_type = if line.has_closing_segment() {
                    EndType::ClosedLine
                } else if join_type == JoinType::Miter {
                    EndType::OpenSquare
                } else {
                    EndType::OpenRound
                };
                clipper.add_path(line.points(), join_type, end_type);
            }
        }

        if !polygons.is_empty() {
            polygons = polygons.union_polygons();
            for polygon in &polygons.lines {
                clipper.add_path(polygon, join_type, EndType::ClosedPolygon);
            }
        }

        clipper.miter_limit = miter_limit;
        let mut result = Vec::new();
        clipper.execute(&mut result, distance as f64);
        Shape::from_paths(result)
    }

    pub fn push_open(&mut self, line: OpenPolyline) {
        self.lines.push(Arc::new(AnyPolyline::Open(line)));
    }
    pub fn push_closed(&mut self, line: ClosedPolyline) {
        self.lines.push(Arc::new(AnyPolyline::Closed(line)));
    }
    pub fn push_polygon(&mut self, line: Polygon) {
        self.lines.push(Arc::new(AnyPolyline::Polygon(line)));
    }
    pub fn push_ptr(&mut self, line: PolylinePtr) {
        self.lines.push(line);
    }
    pub fn push_open_set(&mut self, lines_set: OpenLinesSet) {
        self.lines.reserve(lines_set.len());
        for line in lines_set {
            self.push_open(line);
        }
    }
    pub fn push_closed_set(&mut self, lines_set: ClosedLinesSet) {
        self.lines.reserve(lines_set.len());
        for line in lines_set {
            self.push_closed(line);
        }
    }
    pub fn push_shape(&mut self, lines_set: Shape) {
        self.lines.reserve(lines_set.len());
        for line in lines_set {
            self.push_polygon(line);
        }
    }
    pub fn push_mixed(&mut self, lines_set: &MixedLinesSet) {
        self.lines.extend(lines_set.lines.iter().cloned());
    }

    pub fn length(&self) -> Coord {
        self.lines.iter().fold(0i64, |value, line| value + line.length())
    }
}
