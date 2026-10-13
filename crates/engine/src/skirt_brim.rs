//! `SkirtBrim.h/.cpp`: generation of the skirt, brim and support brim on the first layer.

use crate::application::current_slice;
use crate::geometry::mixed_lines_set::{AnyPolyline, MixedLinesSet};
use crate::geometry::{OpenLinesSet, Polyline, Shape};
use crate::settings::enums::{BrimLocation, EPlatformAdhesion};
use crate::slice_data_storage::SliceDataStorage;
use crate::utils::aabb::AABB;
use crate::utils::coord::Coord;
use crate::utils::polyline_stitcher;
use crate::utils::simplify::Simplify;
use clipper::JoinType;

/// open polyline brim lines smaller than this will be removed
pub const MIN_BRIM_LINE_LENGTH: Coord = 3000;

/// A collection of outlines from which to start a brim.
#[derive(Clone, Default)]
struct Outline {
    /// Outlines for which we want to start a brim by applying the gap for easy detaching
    gapped: Shape,
    /// Outlines for which we want the brim to touch, so that it has better adhesion
    touching: Shape,
}

/// `std::variant<Outline*, int>`: the reference outline of an offset, or the index of the brim
/// line to offset from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Reference {
    /// `&starting_outlines[extruder_nr]`
    StartingOutline(usize),
    /// The local `reference_outline` of `generateSecondarySkirtBrim`.
    LocalOutline,
    /// Index of the brim line to offset from.
    Index(i32),
}

/// A helper class to store an offset yet to be performed on either an outline polygon, or based on an earlier generated brim line.
#[derive(Clone, Debug)]
struct Offset {
    reference_outline_or_index: Reference,
    /// Wether to offset outward from the reference polygons
    outside: bool,
    /// Wether to offset inward from the reference polygons
    inside: bool,
    /// Distance by which to offset from the reference, for outlines with gap applied
    offset_value_gapped: Coord,
    /// Distance by which to offset from the reference, for outlines with no gap applied
    offset_value_touching: Coord,
    /// Total distance from the model
    total_offset: Coord,
    /// The outset index of this brimline
    inset_idx: i32,
    /// The extruder by which to print this brim line
    extruder_nr: usize,
    /// Whether this is the last planned offset for this extruder.
    is_last: bool,
}

/// `OffsetSorter`
fn offset_sorter(a: &Offset, b: &Offset) -> std::cmp::Ordering {
    // Use extruder_nr in case both extruders have the same offset settings.
    let less = |a: &Offset, b: &Offset| if a.total_offset != b.total_offset { a.total_offset < b.total_offset } else { a.extruder_nr < b.extruder_nr };
    if less(a, b) {
        std::cmp::Ordering::Less
    } else if less(b, a) {
        std::cmp::Ordering::Greater
    } else {
        std::cmp::Ordering::Equal
    }
}

/// Container to store the pre-extracted settings of an extruder.
#[derive(Clone, Copy, Default)]
struct ExtruderConfig {
    /// Whether the extruder is actually used in this print
    extruder_is_used: bool,
    /// Whether to generate brim on the outside
    outside_polys: bool,
    /// Whether to generate brim on the inside
    inside_polys: bool,
    /// The skirt/brim line width
    line_width: Coord,
    /// The minimal brim length
    skirt_brim_minimal_length: Coord,
    /// The (minimal) number of brim lines to generate
    line_count: i32,
    /// The gap between the part and the first brim/skirt line
    gap: Coord,
}

pub struct SkirtBrim {
    /// Whether we are generating brim, skirt, or raft
    adhesion_type: EPlatformAdhesion,
    /// Whether the meshgroup has an ooze shield
    has_ooze_shield: bool,
    /// Whether the meshgroup has a draft shield
    has_draft_shield: bool,
    /// The total number of extruders
    extruder_count: usize,
    /// The first extruder which is used
    first_used_extruder_nr: usize,
    /// The extruder with which the skirt/brim is printed or -1 if printed with both
    skirt_brim_extruder_nr: i32,
    /// The brim setup for each extruder
    extruders_configs: Vec<ExtruderConfig>,
}

impl SkirtBrim {
    pub fn new(storage: &SliceDataStorage) -> Self {
        let scene = &current_slice().scene;
        let mesh_group_settings = &scene.current_mesh_group().settings;
        let adhesion_type = mesh_group_settings.get_enum::<EPlatformAdhesion>("adhesion_type");
        let has_ooze_shield = !storage.ooze_shield.is_empty() && !storage.ooze_shield[0].is_empty();
        let has_draft_shield = !storage.draft_protection_shield.is_empty();
        let extruders = &scene.extruders;
        let extruder_count = extruders.len();
        let mut extruders_configs = vec![ExtruderConfig::default(); extruder_count];

        let used_extruders = storage.get_extruders_used();

        let mut first_used_extruder_nr: Option<usize> = None;
        for extruder_nr in 0..extruder_count {
            let extruder_is_used = used_extruders[extruder_nr];
            extruders_configs[extruder_nr].extruder_is_used = extruder_is_used;
            if extruder_is_used && first_used_extruder_nr.is_none() {
                first_used_extruder_nr = Some(extruder_nr);
            }
        }
        let first_used_extruder_nr = first_used_extruder_nr.unwrap_or(0);

        let mut skirt_brim_extruder_nr = mesh_group_settings.get_int("skirt_brim_extruder_nr");
        if skirt_brim_extruder_nr == -1 && adhesion_type == EPlatformAdhesion::SKIRT {
            // Skirt is always printed with all extruders in order to satisfy minimum legnth constraint
            // NOTE: the line count will only be satisfied for the first extruder used.
            skirt_brim_extruder_nr = first_used_extruder_nr as i32;
        }

        for extruder_nr in 0..extruder_count {
            let extruder_config = &mut extruders_configs[extruder_nr];
            if !extruder_config.extruder_is_used {
                continue;
            }

            let extruder = &extruders[extruder_nr];
            let location = extruder.settings.get_enum::<BrimLocation>("brim_location");

            extruder_config.line_width = (extruder.settings.get_coord("skirt_brim_line_width") as f64 * extruder.settings.get_ratio("initial_layer_line_width_factor")) as Coord;
            extruder_config.skirt_brim_minimal_length = extruder.settings.get_coord("skirt_brim_minimal_length");
            extruder_config.outside_polys = adhesion_type == EPlatformAdhesion::SKIRT || location.and(BrimLocation::OUTSIDE) != 0;
            extruder_config.inside_polys = adhesion_type == EPlatformAdhesion::BRIM && location.and(BrimLocation::INSIDE) != 0;
            extruder_config.line_count = extruder.settings.get_int(if adhesion_type == EPlatformAdhesion::BRIM { "brim_line_count" } else { "skirt_line_count" });
            extruder_config.gap = extruder.settings.get_coord(if adhesion_type == EPlatformAdhesion::BRIM { "brim_gap" } else { "skirt_gap" });
        }

        SkirtBrim { adhesion_type, has_ooze_shield, has_draft_shield, extruder_count, first_used_extruder_nr, skirt_brim_extruder_nr, extruders_configs }
    }

    /// Plan the offsets which we will be going to perform and put them in the right order.
    fn generate_brim_offset_plan(&self, storage: &mut SliceDataStorage, starting_outlines: &mut [Outline]) -> Vec<Offset> {
        let mut all_brim_offsets: Vec<Offset> = Vec::new();

        if self.skirt_brim_extruder_nr >= 0 {
            starting_outlines[self.skirt_brim_extruder_nr as usize] = self.get_first_layer_outline(storage, -1);
        } else {
            for extruder_nr in 0..self.extruder_count {
                if !self.extruders_configs[extruder_nr].extruder_is_used {
                    continue;
                }
                starting_outlines[extruder_nr] = self.get_first_layer_outline(storage, extruder_nr as i32);
            }
        }

        for extruder_nr in 0..self.extruder_count {
            let extruder_config = &self.extruders_configs[extruder_nr];
            let semi_line_width = extruder_config.line_width / 2;

            if !extruder_config.extruder_is_used
                || (self.skirt_brim_extruder_nr >= 0 && extruder_nr as i32 != self.skirt_brim_extruder_nr)
                || (starting_outlines[extruder_nr].gapped.is_empty() && starting_outlines[extruder_nr].touching.is_empty())
            {
                continue; // only include offsets for brim extruder
            }

            for line_idx in 0..extruder_config.line_count {
                let is_last = line_idx == extruder_config.line_count - 1;
                let offset_touching = semi_line_width + extruder_config.line_width * line_idx as Coord;
                let offset_gapped = offset_touching + extruder_config.gap;
                if line_idx == 0 {
                    all_brim_offsets.push(Offset {
                        reference_outline_or_index: Reference::StartingOutline(extruder_nr),
                        outside: extruder_config.outside_polys,
                        inside: extruder_config.inside_polys,
                        offset_value_gapped: offset_gapped,
                        offset_value_touching: offset_touching,
                        total_offset: offset_gapped,
                        inset_idx: line_idx,
                        extruder_nr,
                        is_last,
                    });
                } else {
                    all_brim_offsets.push(Offset {
                        reference_outline_or_index: Reference::Index(line_idx - 1),
                        outside: extruder_config.outside_polys,
                        inside: extruder_config.inside_polys,
                        offset_value_gapped: extruder_config.line_width,
                        offset_value_touching: extruder_config.line_width,
                        total_offset: offset_gapped,
                        inset_idx: line_idx,
                        extruder_nr,
                        is_last,
                    });
                }
            }
        }

        all_brim_offsets.sort_by(offset_sorter); // stable
        all_brim_offsets
    }

    /// Generate skirt and brim lines and store them in `storage.skirt_brim`.
    pub fn generate(&mut self, storage: &mut SliceDataStorage) {
        let mut starting_outlines: Vec<Outline> = vec![Outline::default(); self.extruder_count];
        let mut all_brim_offsets = self.generate_brim_offset_plan(storage, &mut starting_outlines);
        let mut allowed_areas_per_extruder = self.generate_allowed_areas(storage, &starting_outlines);

        // Apply 'approximate convex hull' if the adhesion is skirt _after_ any skirt but also prime-tower-brim adhesion.
        // Otherwise, the now expanded convex hull covered areas will mess with that brim. Fortunately this does not mess
        // with the other area calculation above, since they are either itself a simple/convex shape or relevant for brim.
        let mut covered_area = storage.get_layer_outlines(0, true, self.adhesion_type == EPlatformAdhesion::SKIRT, false, -1, true);
        if self.adhesion_type == EPlatformAdhesion::SKIRT {
            covered_area = covered_area.approx_convex_hull(0);
        }

        let mut total_length = self.generate_primary_brim(storage, &starting_outlines, &mut all_brim_offsets, &mut covered_area, &mut allowed_areas_per_extruder);

        // ooze/draft shield brim
        self.generate_shield_brim(storage, &mut covered_area, &mut allowed_areas_per_extruder);

        {
            // only allow secondary skirt/brim to appear on the very outside
            covered_area = covered_area.get_outside_polygons();
            for extruder_nr in 0..self.extruder_count {
                allowed_areas_per_extruder[extruder_nr] = allowed_areas_per_extruder[extruder_nr].difference(&covered_area);
            }
        }

        // Secondary brim of all other materials which don't meet minimum length constraint yet
        self.generate_secondary_skirt_brim(storage, &mut covered_area, &mut allowed_areas_per_extruder, &mut total_length);

        // simplify paths to prevent buffer unnerruns in firmware
        let global_settings = &current_slice().scene.current_mesh_group().settings;
        let maximum_resolution = global_settings.get_coord("meshfix_maximum_resolution");
        let maximum_deviation = global_settings.get_coord("meshfix_maximum_deviation");
        const MAX_AREA_DEV: Coord = 0; // No area deviation applied
        for extruder_nr in 0..self.extruder_count {
            for lines in storage.skirt_brim[extruder_nr].iter_mut() {
                *lines = Simplify::new(maximum_resolution, maximum_deviation, MAX_AREA_DEV).mixed(lines);
            }
        }
    }

    /// Generate the brim which is printed from the outlines of the model/support inward or outward.
    fn generate_primary_brim(
        &self,
        storage: &mut SliceDataStorage,
        starting_outlines: &[Outline],
        all_brim_offsets: &mut Vec<Offset>,
        covered_area: &mut Shape,
        allowed_areas_per_extruder: &mut [Shape],
    ) -> Vec<Coord> {
        let mut total_length: Vec<Coord> = vec![0; self.extruder_count];

        let mut offset_idx = 0usize;
        while offset_idx < all_brim_offsets.len() {
            let offset = all_brim_offsets[offset_idx].clone();
            if storage.skirt_brim[offset.extruder_nr].len() <= offset.inset_idx as usize {
                storage.skirt_brim[offset.extruder_nr].resize_with(offset.inset_idx as usize + 1, MixedLinesSet::default);
            }
            let mut output_location = std::mem::take(&mut storage.skirt_brim[offset.extruder_nr][offset.inset_idx as usize]);
            let reference = match offset.reference_outline_or_index {
                Reference::StartingOutline(idx) => Some(&starting_outlines[idx]),
                _ => None,
            };
            let added_length = self.generate_offset(storage, &offset, reference, covered_area, allowed_areas_per_extruder, &mut output_location);
            storage.skirt_brim[offset.extruder_nr][offset.inset_idx as usize] = output_location;

            if added_length == 0 {
                // no more place for more brim. Trying to satisfy minimum length constraint with generateSecondarySkirtBrim
                offset_idx += 1;
                continue;
            }
            total_length[offset.extruder_nr] += added_length;

            let extruder_config = &self.extruders_configs[offset.extruder_nr];

            if offset.is_last
                && total_length[offset.extruder_nr] < extruder_config.skirt_brim_minimal_length // This was the last offset of this extruder, but the brim lines don't meet minimal length yet
                && total_length[offset.extruder_nr] > 0
            // No lines got added; we have no extrusion lines to build on
            {
                all_brim_offsets[offset_idx].is_last = false;
                let is_last = true;
                all_brim_offsets.push(Offset {
                    reference_outline_or_index: Reference::Index(offset.inset_idx),
                    outside: extruder_config.outside_polys,
                    inside: extruder_config.inside_polys,
                    offset_value_gapped: extruder_config.line_width,
                    offset_value_touching: offset.total_offset + extruder_config.line_width,
                    total_offset: offset.total_offset + extruder_config.line_width,
                    inset_idx: offset.inset_idx + 1,
                    extruder_nr: offset.extruder_nr,
                    is_last,
                });
                all_brim_offsets[offset_idx + 1..].sort_by(offset_sorter); // reorder remaining offsets
            }
            offset_idx += 1;
        }
        total_length
    }

    /// Generate the brim inside the ooze shield and draft shield.
    fn generate_shield_brim(&self, storage: &mut SliceDataStorage, brim_covered_area: &mut Shape, allowed_areas_per_extruder: &mut [Shape]) {
        let mut extruder_nr = self.skirt_brim_extruder_nr;
        if extruder_nr < 0 {
            // the shields are always printed with all extruders, so it doesn't really matter with which extruder we print the brim on the first layer
            extruder_nr = self.first_used_extruder_nr as i32;
        }
        let extruder_nr = extruder_nr as usize;

        let extruder_config = &self.extruders_configs[extruder_nr];

        // generate brim for ooze shield and draft shield
        if self.adhesion_type == EPlatformAdhesion::BRIM && (self.has_ooze_shield || self.has_draft_shield) {
            let primary_extruder_skirt_brim_line_width = extruder_config.line_width;
            let primary_line_count = extruder_config.line_count;

            // generate areas where to make extra brim for the shields
            // avoid gap in the middle
            //    V
            //  +---+     +----+
            //  |+-+|     |+--+|
            //  || ||     ||[]|| > expand to fit an extra brim line
            //  |+-+|     |+--+|
            //  +---+     +----+
            let primary_skirt_brim_width = (primary_line_count + primary_line_count % 2) as Coord * primary_extruder_skirt_brim_line_width; // always use an even number, because we will fil the area from both sides

            let mut shield_brim = Shape::default();
            if self.has_ooze_shield {
                shield_brim = storage.ooze_shield[0].difference(&storage.ooze_shield[0].offset_default(-primary_skirt_brim_width - primary_extruder_skirt_brim_line_width));
            }
            if self.has_draft_shield {
                shield_brim = shield_brim.union_polygons_other(
                    &storage.draft_protection_shield.difference(&storage.draft_protection_shield.offset_default(-primary_skirt_brim_width - primary_extruder_skirt_brim_line_width)),
                );
            }
            shield_brim = shield_brim.intersection(&allowed_areas_per_extruder[extruder_nr].offset_default(primary_extruder_skirt_brim_line_width / 2));
            let layer_outlines = storage.get_layer_outlines(0, false, true, false, -1, true);
            shield_brim = shield_brim.difference(&layer_outlines.get_outside_polygons()); // don't generate any shield brim inside holes

            let covered_area = shield_brim.offset_default(primary_extruder_skirt_brim_line_width / 2);
            *brim_covered_area = brim_covered_area.union_polygons_other(&covered_area);
            allowed_areas_per_extruder[extruder_nr] = allowed_areas_per_extruder[extruder_nr].difference(&covered_area);

            // generate brim within shield_brim
            storage.skirt_brim[extruder_nr].push(MixedLinesSet::default());
            storage.skirt_brim[extruder_nr].last_mut().unwrap().push_shape(shield_brim.clone());
            while !shield_brim.is_empty() {
                shield_brim = shield_brim.offset_default(-primary_extruder_skirt_brim_line_width);
                // throw all polygons for the shileds onto one heap; because the brim lines are
                // generated from both sides the order will not be important
                storage.skirt_brim[extruder_nr].last_mut().unwrap().push_shape(shield_brim.clone());
            }
        }

        if self.adhesion_type == EPlatformAdhesion::SKIRT {
            if self.has_ooze_shield {
                let covered_area = storage.ooze_shield[0].offset_default(extruder_config.line_width / 2);
                *brim_covered_area = brim_covered_area.union_polygons_other(&covered_area);
                allowed_areas_per_extruder[extruder_nr] = allowed_areas_per_extruder[extruder_nr].difference(&covered_area);
            }
            if self.has_draft_shield {
                let covered_area = storage.draft_protection_shield.offset_default(extruder_config.line_width / 2);
                *brim_covered_area = brim_covered_area.union_polygons_other(&covered_area);
                allowed_areas_per_extruder[extruder_nr] = allowed_areas_per_extruder[extruder_nr].difference(&covered_area);
            }
        }
    }

    /// The first layer outline from which to start the skirt/brim (or the outline of a single extruder).
    fn get_first_layer_outline(&self, storage: &mut SliceDataStorage, extruder_nr: i32) -> Outline {
        let mut first_layer_outline = Outline::default();
        let global_settings = &current_slice().scene.current_mesh_group().settings;
        let mut reference_extruder_nr = self.skirt_brim_extruder_nr;
        debug_assert!(
            !(reference_extruder_nr == -1 && extruder_nr == -1),
            "We should only request the outlines of all layers when the brim is being generated for only one material"
        );
        if reference_extruder_nr == -1 {
            reference_extruder_nr = extruder_nr;
        }
        let reference_extruder_config = self.extruders_configs[reference_extruder_nr as usize];
        let primary_line_count = reference_extruder_config.line_count;
        let layer_nr = 0;
        if self.adhesion_type == EPlatformAdhesion::SKIRT {
            let mut skirt_height: i32 = 0;
            for extruder in &current_slice().scene.extruders {
                if extruder_nr == -1 || extruder_nr == extruder.extruder_nr as i32 {
                    skirt_height = skirt_height.max(extruder.settings.get_int("skirt_height"));
                }
            }
            skirt_height = skirt_height.min(storage.print_layer_count as i32);

            for i_layer in layer_nr..skirt_height {
                let include_support = true;
                let include_prime_tower = true;
                first_layer_outline.gapped =
                    first_layer_outline.gapped.union_polygons_other(&storage.get_layer_outlines(i_layer as _, include_support, include_prime_tower, true, -1, true));
            }

            let mut shields = Shape::default();
            if self.has_ooze_shield {
                shields = storage.ooze_shield[0].clone();
            }
            if self.has_draft_shield {
                shields = shields.union_polygons_other(&storage.draft_protection_shield);
            }
            first_layer_outline.gapped = first_layer_outline.gapped.union_polygons_other(&shields.offset_default(
                reference_extruder_config.line_width / 2 // because the shield is printed *on* the stored polygons; not inside hteir area
                    - reference_extruder_config.gap, // so that when we apply the gap we will end up right next to the shield
            ));
            // NOTE: offsetting by -gap here and by +gap in the main brim algorithm effectively performs a morphological close,
            // so in some cases with a large skirt gap and small models and small shield distance
            // the skirt lines can cross the shield lines.
            // This shouldn't be a big problem, since the skirt lines are far away from the model.
            first_layer_outline.gapped = first_layer_outline.gapped.approx_convex_hull(0);
        } else {
            // add brim underneath support by removing support where there's brim around the model
            {
                let include_support = false; // Don't include the supports yet because we need to reduce them before
                let include_prime_tower = false; // Not included, has its own brim
                let external_polys_only = false; // Gather all polygons and treat them separately.
                first_layer_outline.gapped = storage.get_layer_outlines(layer_nr as _, include_support, include_prime_tower, external_polys_only, extruder_nr, true);
                first_layer_outline.gapped = first_layer_outline.gapped.union_polygons(); // To guard against overlapping outlines, which would produce holes according to the even-odd rule.
            }

            if storage.support.generated
                && primary_line_count > 0
                && !storage.support.support_layers.is_empty()
                && (extruder_nr == -1 || extruder_nr == global_settings.get_int("support_infill_extruder_nr"))
            {
                // remove model-brim from support
                let support_infill_extruder = global_settings.get_extruder_train("support_infill_extruder_nr");
                if support_infill_extruder.settings.get_bool("brim_replaces_support") {
                    // avoid gap in the middle
                    //    V
                    //  +---+     +----+
                    //  |+-+|     |+--+|
                    //  || ||     ||[]|| > expand to fit an extra brim line
                    //  |+-+|     |+--+|
                    //  +---+     +----+
                    let primary_extruder_skirt_brim_line_width = reference_extruder_config.line_width;
                    let mut model_brim_covered_area = Shape::default();

                    // always leave a gap of an even number of brim lines, so that it fits if it's generating brim from both sides
                    let offset = primary_extruder_skirt_brim_line_width * (primary_line_count + primary_line_count % 2) as Coord;

                    for polygon in first_layer_outline.gapped.iter() {
                        // Compute the fringe that the brim is going to cover around the model
                        let mut outset = Shape::default();
                        let mut inset = Shape::default();

                        let area = polygon.area();
                        if area > 0.0 && reference_extruder_config.outside_polys {
                            outset = polygon.offset(offset as i32, JoinType::Round, 1.2);
                            inset.push(polygon.clone());
                        } else if area < 0.0 && reference_extruder_config.inside_polys {
                            outset.push(polygon.clone());
                            inset = polygon.offset(-offset as i32, JoinType::Round, 1.2);
                        }

                        outset = outset.difference(&inset);
                        model_brim_covered_area = model_brim_covered_area.union_polygons_other(&outset);
                    }

                    let model_brim_covered_area_boundary_box = AABB::from_shape(&model_brim_covered_area);
                    let support_layer = &mut storage.support.support_layers[0];
                    support_layer.exclude_areas_from_support_infill_areas(&model_brim_covered_area, &model_brim_covered_area_boundary_box);

                    // If the gap between the model and the BP is small enough, support starts with the interface instead, so remove it there as well:
                    support_layer.support_roof = support_layer.support_roof.difference(&model_brim_covered_area);
                }

                let support_layer = &storage.support.support_layers[0];
                for support_infill_part in &support_layer.support_infill_parts {
                    first_layer_outline.touching.push_all(&support_infill_part.outline.0);
                }
                first_layer_outline.touching.push_all(&support_layer.support_bottom);
                first_layer_outline.touching.push_all(&support_layer.support_roof);
            }
        }

        // NOTE: The C++ code here loops over `std::array{ first_layer_outline.gapped, first_layer_outline.touching }`,
        // i.e. over *copies* of the outlines, so the join/simplify operations performed in that loop have no effect
        // on the returned outline. They are therefore omitted.
        first_layer_outline
    }

    /// Generate a brim line with offset parameters given by `offset` from the `starting_outlines` and store it in `result`.
    fn generate_offset(
        &self,
        storage: &SliceDataStorage,
        offset: &Offset,
        reference_outline: Option<&Outline>,
        covered_area: &mut Shape,
        allowed_areas_per_extruder: &mut [Shape],
        result: &mut MixedLinesSet,
    ) -> Coord {
        let mut brim = Shape::default();
        let extruder_config = &self.extruders_configs[offset.extruder_nr];

        if let Some(reference_outline) = reference_outline {
            for (shape, offset_value) in [(&reference_outline.gapped, offset.offset_value_gapped), (&reference_outline.touching, offset.offset_value_touching)] {
                for polygon in shape.iter() {
                    let area = polygon.area();
                    if area > 0.0 && offset.outside {
                        brim.push_all_owned(polygon.offset(offset_value as i32, JoinType::Round, 1.2));
                    } else if area < 0.0 && offset.inside {
                        brim.push_all_owned(polygon.offset(-offset_value as i32, JoinType::Round, 1.2));
                    }
                }
            }
        } else {
            let reference_idx = match offset.reference_outline_or_index {
                Reference::Index(i) => i,
                _ => unreachable!(),
            };
            let offset_dist = extruder_config.line_width;

            brim.push_all_owned(storage.skirt_brim[offset.extruder_nr][reference_idx as usize].offset(offset_dist, JoinType::Round, 1.2));
        }

        // limit brim lines to allowed areas, stitch them and store them in the result
        brim = Simplify::from_settings(&current_slice().scene.extruders[offset.extruder_nr].settings).shape(&brim);

        let brim_lines: OpenLinesSet = allowed_areas_per_extruder[offset.extruder_nr].intersection_polylines(&brim, false, 10, true);
        let length_added = brim_lines.length();

        let newly_covered = brim_lines.offset(extruder_config.line_width / 2 + 10, JoinType::Round, 1.2);

        let max_stitch_distance = extruder_config.line_width;
        mixed_polyline_stitch(&brim_lines, result, max_stitch_distance, 10);

        // clean up too small lines (only open ones, which was done historically but may be a mistake)
        result.lines.retain(|line| match &**line {
            AnyPolyline::Open(open_line) => !open_line.shorter_than(MIN_BRIM_LINE_LENGTH),
            _ => true,
        });

        // update allowed_areas_per_extruder
        *covered_area = covered_area.union_polygons_other(&newly_covered.union_polygons());
        for extruder_nr in 0..self.extruder_count {
            if self.extruders_configs[extruder_nr].extruder_is_used {
                allowed_areas_per_extruder[extruder_nr] = allowed_areas_per_extruder[extruder_nr].difference(covered_area);
            }
        }

        length_added
    }

    /// Generate a skirt of extruders which don't satisfy the min length constraint yet.
    fn generate_secondary_skirt_brim(
        &self,
        storage: &mut SliceDataStorage,
        covered_area: &mut Shape,
        allowed_areas_per_extruder: &mut [Shape],
        total_length: &mut [Coord],
    ) {
        let bogus_total_offset: Coord = 0; // Doesn't matter. The offsets won't be sorted here.
        let is_last = false; // Doesn't matter. Isn't used in the algorithm below.
        for extruder_nr in 0..self.extruder_count {
            let mut first = true;
            let reference_outline = Outline { gapped: Shape::default(), touching: covered_area.clone() };
            let extruder_config = self.extruders_configs[extruder_nr];
            while total_length[extruder_nr] < extruder_config.skirt_brim_minimal_length {
                let ref_polys_or_idx;
                let offset_from_reference;
                if first {
                    ref_polys_or_idx = Reference::LocalOutline;
                    offset_from_reference = extruder_config.line_width / 2;
                } else {
                    ref_polys_or_idx = Reference::Index((storage.skirt_brim[extruder_nr].len() as i32).wrapping_sub(1));
                    offset_from_reference = extruder_config.line_width;
                }
                let outside_polys = extruder_config.outside_polys;
                let inside_polys = extruder_config.inside_polys;
                let extra_offset = Offset {
                    reference_outline_or_index: ref_polys_or_idx,
                    outside: outside_polys,
                    inside: inside_polys,
                    offset_value_gapped: offset_from_reference,
                    offset_value_touching: offset_from_reference,
                    total_offset: bogus_total_offset,
                    inset_idx: storage.skirt_brim[extruder_nr].len() as i32,
                    extruder_nr,
                    is_last,
                };

                storage.skirt_brim[extruder_nr].push(MixedLinesSet::default());
                let output_idx = storage.skirt_brim[extruder_nr].len() - 1;
                let mut output_location = std::mem::take(&mut storage.skirt_brim[extruder_nr][output_idx]);
                let reference = if first { Some(&reference_outline) } else { None };
                let added_length = self.generate_offset(storage, &extra_offset, reference, covered_area, allowed_areas_per_extruder, &mut output_location);
                storage.skirt_brim[extruder_nr][output_idx] = output_location;

                if added_length == 0 {
                    crate::log_warn!("Couldn't satisfy minimum length constraint of extruder {}!\n", extruder_nr);
                    break;
                }

                total_length[extra_offset.extruder_nr] += added_length;

                first = false;
            }
        }
    }

    /// Generate the allowed areas for each extruder.
    fn generate_allowed_areas(&self, storage: &SliceDataStorage, starting_outlines: &[Outline]) -> Vec<Shape> {
        let layer_nr = 0;

        // For each extruder, pre-compute the areas covered by models/supports/prime tower
        #[derive(Default, Clone)]
        struct ExtruderOutlines {
            models_outlines: Shape,
            supports_outlines: Shape,
        }

        let mut covered_area_by_extruder: Vec<ExtruderOutlines> = Vec::new();
        if self.adhesion_type == EPlatformAdhesion::BRIM {
            covered_area_by_extruder.resize(self.extruder_count, ExtruderOutlines::default());
            for extruder_nr in 0..self.extruder_count {
                if self.extruders_configs[extruder_nr].extruder_is_used {
                    // Gather models/support/prime tower areas separately to apply different margins
                    let extruder_outlines = &mut covered_area_by_extruder[extruder_nr];
                    let external_polys_only = false;
                    {
                        let include_support = false;
                        let include_prime_tower = false;
                        let include_model = true;
                        extruder_outlines.models_outlines =
                            storage.get_layer_outlines(layer_nr, include_support, include_prime_tower, external_polys_only, extruder_nr as i32, include_model);
                    }
                    {
                        let include_support = true;
                        let include_prime_tower = true;
                        let include_model = false;
                        extruder_outlines.supports_outlines =
                            storage.get_layer_outlines(layer_nr, include_support, include_prime_tower, external_polys_only, extruder_nr as i32, include_model);
                    }
                }
            }
        }

        let mut allowed_areas_per_extruder: Vec<Shape> = vec![Shape::default(); self.extruder_count];
        for extruder_nr in 0..self.extruder_count {
            let extruder_config = &self.extruders_configs[extruder_nr];

            if !extruder_config.extruder_is_used {
                continue;
            }

            // Initialize allowed area to full build plate, then remove disallowed areas
            let mut allowed_areas = storage.get_machine_border(extruder_nr as i32);

            if self.adhesion_type == EPlatformAdhesion::BRIM {
                let settings = &current_slice().scene.extruders[extruder_nr].settings;
                let hole_brim_distance = settings.get_coord("brim_inside_margin");

                for (other_extruder_nr, extruder_outlines) in covered_area_by_extruder.iter().enumerate() {
                    let base_offset = extruder_config.line_width / 2;

                    // Remove areas covered by models
                    for covered_surface in extruder_outlines.models_outlines.iter() {
                        let mut offset = base_offset;
                        let covered_area = covered_surface.area();

                        if (other_extruder_nr == extruder_nr || extruder_nr as i32 == self.skirt_brim_extruder_nr)
                            && ((covered_area > 0.0 && extruder_config.outside_polys) || (covered_area < 0.0 && extruder_config.inside_polys))
                        {
                            // This is an area we are gonna intentionnally print brim in, use the actual gap
                            offset += extruder_config.gap - 50; // Lower margin a bit to avoid discarding legitimate lines
                        } else {
                            // This is an area we do not expect brim to be printed in, use a larger gap to keep the printed surface clean
                            offset += hole_brim_distance;
                        }

                        if covered_area < 0.0 {
                            // Invert offset to make holes grow inside
                            allowed_areas.push_all_owned(covered_surface.offset(-offset as i32, JoinType::Round, 1.2));
                        } else {
                            allowed_areas = allowed_areas.difference(&covered_surface.offset(offset as i32, JoinType::Round, 1.2));
                        }
                    }

                    // Remove areas covered by support, with a low margin because we don't care if the brim touches it
                    allowed_areas = allowed_areas.difference(&extruder_outlines.supports_outlines.offset_default(base_offset - 50));
                }
            }

            // Anyway, don't allow a brim/skirt to grow inside itself, which may happen e.g. with ooze shield+skirt
            allowed_areas = allowed_areas.difference(&starting_outlines[extruder_nr].gapped.offset(extruder_config.gap - 50, JoinType::Round, 1.2));
            allowed_areas = allowed_areas.difference(&starting_outlines[extruder_nr].touching.offset(-50, JoinType::Round, 1.2));
            allowed_areas_per_extruder[extruder_nr] = allowed_areas;
        }

        allowed_areas_per_extruder
    }

    /// Generate the brim around the support on the first layer.
    pub fn generate_support_brim(&mut self, storage: &mut SliceDataStorage) {
        const BRIM_AREA_MINIMUM_HOLE_SIZE_MULTIPLIER: Coord = 100;

        let scene = &current_slice().scene;
        let support_infill_extruder = scene.current_mesh_group().settings.get_extruder_train("support_infill_extruder_nr");
        let brim_line_width =
            (support_infill_extruder.settings.get_coord("skirt_brim_line_width") as f64 * support_infill_extruder.settings.get_ratio("initial_layer_line_width_factor")) as Coord;
        let mut line_count = support_infill_extruder.settings.get_usize("support_brim_line_count");
        let minimal_length = support_infill_extruder.settings.get_coord("skirt_brim_minimal_length");
        if !storage.support.generated || line_count == 0 || storage.support.support_layers.is_empty() {
            return;
        }

        let brim_width = brim_line_width * line_count as Coord;
        let mut skirt_brim_length: Coord = 0;

        let support_extruder_nr = support_infill_extruder.extruder_nr;
        if storage.skirt_brim[support_extruder_nr].is_empty() {
            storage.skirt_brim[support_extruder_nr].push(MixedLinesSet::default());
        }

        for brim_line in &storage.skirt_brim[support_extruder_nr] {
            skirt_brim_length += brim_line.length();
        }

        let support_layer = &mut storage.support.support_layers[0];

        let mut support_outline = Shape::default();
        for part in &support_layer.support_infill_parts {
            support_outline.push_all(&part.outline.0);
        }
        let brim_area = support_outline.difference(&support_outline.offset_default(-brim_width));
        support_layer.exclude_areas_from_support_infill_areas(&brim_area, &AABB::from_shape(&brim_area));

        let mut offset_distance = brim_line_width / 2;
        let mut skirt_brim_number: usize = 0;
        while skirt_brim_number < line_count {
            offset_distance -= brim_line_width;

            let mut brim_line = support_outline.offset(offset_distance, JoinType::Round, 1.2);

            // Remove small inner skirt and brim holes. Holes have a negative area, remove anything smaller then multiplier x extrusion "area"
            let mut n: usize = 0;
            while n < brim_line.len() {
                let area = brim_line[n].area();
                if area < 0.0 && area > -(brim_line_width * brim_line_width * BRIM_AREA_MINIMUM_HOLE_SIZE_MULTIPLIER) as f64 {
                    brim_line.remove_at(n);
                    n = n.wrapping_sub(1);
                }
                n = n.wrapping_add(1);
            }

            let brim_line_empty = brim_line.is_empty(); // Store before moving
            for polygon in brim_line {
                storage.support_brim.push(polygon.as_closed_polyline());
            }
            // In case of adhesion::NONE length of support brim is only the length of the brims formed for the support
            let length = if self.adhesion_type == EPlatformAdhesion::NONE { skirt_brim_length } else { skirt_brim_length + storage.support_brim.length() };
            if skirt_brim_number + 1 >= line_count && length > 0 && length < minimal_length {
                // Make brim or skirt have more lines when total length is too small.
                line_count += 1;
            }
            if brim_line_empty {
                // the fist layer of support is fully filled with brim
                break;
            }
            skirt_brim_number += 1;
        }
    }
}

/// `MixedPolylineStitcher::stitch`.
pub fn mixed_polyline_stitch(lines: &OpenLinesSet, result: &mut MixedLinesSet, max_stitch_distance: Coord, snap_distance: Coord) {
    let mut open_lines = OpenLinesSet::default();
    let mut closed_lines = crate::geometry::ClosedLinesSet::default();

    polyline_stitcher::stitch_open_into_closed(lines, &mut open_lines, &mut closed_lines, max_stitch_distance, snap_distance);

    result.push_open_set(open_lines);

    for closed_line in closed_lines.iter_mut() {
        // Base stitch method will create explicitly closed polylines, but won't tag them as such
        // because it is a generic algorithm. Tag them now.
        closed_line.explicitly_closed = true;
    }

    result.push_closed_set(closed_lines);
}
