//! `geometry/MendedShape.h/.cpp`: an outline prepared for Voronoi-based wall generation (no
//! slivers, no self intersections or near self intersections).

use crate::geometry::lines_set::Shape;
use crate::settings::settings::Settings;
use crate::settings::types::AngleRadians;
use crate::utils::polygon_utils::PolygonUtils;
use crate::utils::section_type::SectionType;
use crate::utils::simplify::Simplify;
use crate::utils::smooth::Smooth;
use clipper::JoinType;

pub struct MendedShape {
    shape: Shape,
}

impl MendedShape {
    pub fn new(settings: &Settings, section_type: SectionType, shape: &Shape) -> Self {
        // Sometimes small slivers of polygons mess up the prepared_outline. By performing an open-close operation
        // with half the minimum printable feature size or minimum line width, these slivers are removed, while still
        // keeping enough information to not degrade the print quality;
        // These features can't be printed anyhow. See PR CuraEngine#1811 for some screenshots
        let allowed_distance = settings.get_coord("meshfix_maximum_deviation");
        let open_close_distance = if settings.get_bool("fill_outline_gaps") {
            settings.get_coord("min_feature_size") / 2 - 5
        } else {
            settings.get_coord("min_wall_line_width") / 2 - 5
        };
        let epsilon_offset = (allowed_distance / 2) - 1;
        let small_area_length = settings.get_f64("wall_line_width_0") / 2.0;

        // Simplify outline for boost::voronoi consumption. Absolutely no self intersections or near-self intersections allowed:
        let mut shape_ = shape
            .offset(-open_close_distance, JoinType::Miter, 1.2)
            .offset(open_close_distance * 2, JoinType::Miter, 1.2)
            .offset(-open_close_distance, JoinType::Miter, 1.2);
        shape_.remove_small_areas(small_area_length * small_area_length, false);
        shape_ = Simplify::from_settings(settings).shape(&shape_);
        if settings.get_bool("meshfix_fluid_motion_enabled") && section_type != SectionType::SUPPORT {
            // No need to smooth support walls
            let smoother = Smooth::from_settings(settings);
            for polygon in shape_.iter_mut() {
                let pts = smoother.apply(&polygon.points);
                polygon.points = pts;
            }
        }

        PolygonUtils::fix_self_intersections(epsilon_offset, &mut shape_);
        shape_.remove_degenerate_verts();
        shape_.remove_colinear_edges(AngleRadians::new(0.005));
        // Removing collinear edges may introduce self intersections, so we need to fix them again
        PolygonUtils::fix_self_intersections(epsilon_offset, &mut shape_);
        shape_.remove_degenerate_verts();
        shape_ = shape_.union_polygons();
        shape_ = Simplify::from_settings(settings).shape(&shape_).remove_near_self_intersections();

        MendedShape { shape: shape_ }
    }

    pub fn get_shape(&self) -> &Shape {
        &self.shape
    }
}
