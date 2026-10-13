//! `ConicalOverhang.h/.cpp`: make overhangs printable by widening the layers below.

use crate::geometry::Shape;
use crate::mesh::Mesh;
use crate::settings::types::LayerIndex;
use crate::slicer::Slicer;
use crate::utils::simplify::Simplify;
use clipper::{JoinType, PolyFillType};

pub struct ConicalOverhang;

impl ConicalOverhang {
    /// Change the slice data such that the model becomes more printable by removing overhanging areas.
    pub fn apply(slicer: &mut Slicer, mesh: &Mesh) {
        let angle = mesh.settings.get_angle_radians("conical_overhang_angle");
        let max_hole_area = mesh.settings.get_f64("conical_overhang_hole_size");
        let tan_angle = angle.value().tan(); // the XY-component of the angle
        let layer_thickness = mesh.settings.get_coord("layer_height");
        let max_dist_from_lower_layer = cppstd::llround(tan_angle * layer_thickness as f64); // max dist which can be bridged

        let mut layer_nr = slicer.layers.len() as LayerIndex - 2;
        while layer_nr >= 0 {
            let above_polygons = slicer.layers[layer_nr as usize + 1].polygons.clone();
            let layer = &mut slicer.layers[layer_nr as usize];
            if max_dist_from_lower_layer.abs() < 5 {
                // magically nothing happens when max_dist_from_lower_layer == 0
                // below magic code solves that
                let safe_dist: i64 = 20;
                let diff = above_polygons.difference(&layer.polygons.offset(-safe_dist, JoinType::Miter, 1.2));
                layer.polygons = layer.polygons.union_polygons_with(&diff, PolyFillType::NonZero);
                layer.polygons = layer.polygons.smooth(safe_dist as i32);
                layer.polygons = Simplify::new(safe_dist, safe_dist / 2, 0).shape(&layer.polygons);
                // somehow layer.polygons get really jagged lines with a lot of vertices
                // without the above steps slicing goes really slow
            } else {
                // Get the current layer and split it into parts
                let layer_parts = layer.polygons.split_into_parts(false);
                // Get a copy of the layer above to prune away before we shrink it
                let mut above = above_polygons;

                // Now go through all the holes in the current layer and check if they intersect anything in the layer above
                // If not, then they're the top of a hole and should be cut from the layer above before the layer above is shrunk
                for part in &layer_parts {
                    if part.0.len() > 1 {
                        // first poly is the outer perimeter
                        for hole_nr in 1..part.0.len() {
                            let mut hole_poly = Shape::default();
                            hole_poly.push(part.0[hole_nr].clone());
                            if max_hole_area > 0.0 && hole_poly.area().abs() / 1_000_000.0 < max_hole_area {
                                let hole_with_above = hole_poly.intersection(&above);
                                if !hole_with_above.is_empty() {
                                    // The hole had some intersection with the above layer, check if it's a complete overlap
                                    let hole_difference = hole_poly.xor_polygons(&hole_with_above, PolyFillType::EvenOdd);
                                    if hole_difference.is_empty() {
                                        // The hole was returned unchanged, so the layer above must completely cover it.  Remove the hole from the layer above.
                                        above = above.difference(&hole_poly);
                                    }
                                }
                            }
                        }
                    }
                }
                // And now union with offset of the resulting above layer
                layer.polygons = layer.polygons.union_polygons_with(&above.offset(-max_dist_from_lower_layer, JoinType::Miter, 1.2), PolyFillType::NonZero);
            }
            layer_nr -= 1;
        }
    }
}
