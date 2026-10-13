//! A port of the Boost.Polygon Voronoi diagram builder (Boost 1.86, by Andrii Sydorchuk) for
//! integral input (points and non-intersecting segments), as used by CuraEngine's Arachne wall
//! generator. The sweepline algorithm, its robust predicates (including the lazy/exact
//! evaluation with extended precision integers) and the output construction are reproduced
//! exactly, so the produced diagram is identical to the one built by the C++ library.
//!
//! Distributed under the Boost Software License, Version 1.0.

pub mod builder;
pub mod ctypes;
pub mod diagram;
pub mod predicates;
pub mod robust_fpt;
pub mod structures;

pub use builder::VoronoiBuilder;
pub use diagram::{VoronoiCell, VoronoiDiagram, VoronoiEdge, VoronoiVertex};
pub use structures::{SourceCategory, NIL};

/// `boost::polygon::construct_voronoi(points.., segments.., &vd)` with the default (int32) builder.
/// Coordinates are given as 64 bit values and narrowed to 32 bit like the implicit conversion in
/// the C++ library.
pub fn construct_voronoi(points: &[(i64, i64)], segments: &[((i64, i64), (i64, i64))], vd: &mut VoronoiDiagram) {
    let mut builder = VoronoiBuilder::new();
    for p in points {
        builder.insert_point(p.0 as i32, p.1 as i32);
    }
    for s in segments {
        builder.insert_segment(s.0 .0 as i32, s.0 .1 as i32, s.1 .0 as i32, s.1 .1 as i32);
    }
    builder.construct(vd);
}
