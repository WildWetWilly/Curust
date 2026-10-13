//! `include/geometry`.

pub mod lines_set;
pub mod matrix;
pub mod mended_shape;
pub mod mixed_lines_set;
pub mod point2ll;
pub mod point3ll;
pub mod points_f;
pub mod polyline;
pub mod wagyu_bridge;

pub use lines_set::{ClosedLinesSet, LinesSet, OpenLinesSet, PartsView, Shape, SingleShape};
pub use point2ll::Point2LL;
pub use point3ll::Point3LL;
pub use polyline::{ClosedLine, ClosedPolyline, OpenPolyline, Polygon, Polyline};
