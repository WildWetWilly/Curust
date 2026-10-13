//! Cross / cross-3D infill: `infill/SierpinskiFill`, `SierpinskiFillProvider`, `DensityProvider`,
//! `UniformDensityProvider` and `ImageBasedDensityProvider`.
//!
//! The C++ `SierpinskiFill` keeps a `std::list<SierpinskiTriangle*>` and works with list
//! iterators; this port uses an index-based doubly linked list with a sentinel node, so that
//! iterator semantics (including `std::prev(begin())` being the sentinel) are kept.

use crate::geometry::point2ll::{dot, normal, v_size, v_size2, v_size_mm, NO_POINT};
use crate::geometry::point3ll::Point3LL;
use crate::geometry::{Point2LL, Polygon};
use crate::settings::enums::EFillMethod;
use crate::utils::aabb::{AABB, AABB3D};
use crate::utils::coord::{int2mm, int2mm2, Coord};

const ALLOWED_LENGTH_ERROR: f64 = 0.01;
const SQRT2: f64 = std::f64::consts::SQRT_2;

/// `DensityProvider`: determines the requested infill density of a region.
pub enum DensityProvider {
    /// `UniformDensityProvider`
    Uniform(f64),
    /// `ImageBasedDensityProvider`
    Image(ImageBasedDensityProvider),
}

impl DensityProvider {
    fn density(&self, aabb: &AABB3D) -> f64 {
        match self {
            DensityProvider::Uniform(density) => *density,
            DensityProvider::Image(provider) => provider.density(aabb),
        }
    }
}

/// `ImageBasedDensityProvider`: the requested density is read from the lightness of an image.
pub struct ImageBasedDensityProvider {
    /// dimensions of the image. Third dimension is the amount of channels.
    image_size: Point3LL,
    /// image data: rows of channel data per pixel.
    image: Vec<u8>,
    /// bounding box of print coordinates in which to apply the image
    print_aabb: AABB,
}

impl ImageBasedDensityProvider {
    pub fn new(filename: &str, model_aabb: AABB) -> Self {
        let (img_x, img_y, img_z, image) = match crate::utils::image::load_image(filename) {
            Ok(img) => img,
            Err(reason) => {
                crate::log_error!("Cannot load image {}: {}", filename, reason);
                std::process::exit(-1);
            }
        };
        let image_size = Point3LL::new(img_x as Coord, img_y as Coord, img_z as Coord);
        // compute aabb
        let middle = model_aabb.get_middle();
        let model_aabb_size = model_aabb.max - model_aabb.min;
        let image_size2 = Point2LL::new(image_size.x, image_size.y);
        let aabb_aspect_ratio = model_aabb_size.x as f64 / model_aabb_size.y as f64;
        let image_aspect_ratio = image_size.x as f64 / image_size.y as f64;
        let aabb_size = if image_aspect_ratio < aabb_aspect_ratio {
            Point2LL::new(image_size2.x * model_aabb_size.x / image_size.x, image_size2.y * model_aabb_size.x / image_size.x)
        } else {
            Point2LL::new(image_size2.x * model_aabb_size.y / image_size.y, image_size2.y * model_aabb_size.y / image_size.y)
        };
        let half = Point2LL::new(aabb_size.x / 2, aabb_size.y / 2);
        let print_aabb = AABB::new(middle - half, middle + half);
        ImageBasedDensityProvider { image_size, image, print_aabb }
    }

    fn pixel(&self, x: Coord, y: Coord, z: Coord) -> i64 {
        self.image[(((self.image_size.y - 1 - y) * self.image_size.x + x) * self.image_size.z + z) as usize] as i64
    }

    fn density(&self, query_cube: &AABB3D) -> f64 {
        let query_min = Point2LL::new(query_cube.min.x, query_cube.min.y);
        let query_max = Point2LL::new(query_cube.max.x, query_cube.max.y);
        let pa = &self.print_aabb;
        let a = query_min - pa.min - Point2LL::new(1, 1);
        let img_min = Point2LL::new(a.x * self.image_size.x / (pa.max.x - pa.min.x), a.y * self.image_size.x / (pa.max.x - pa.min.x));
        let b = query_max - pa.min + Point2LL::new(1, 1);
        let img_max = Point2LL::new(b.x * self.image_size.y / (pa.max.y - pa.min.y), b.y * self.image_size.y / (pa.max.y - pa.min.y));
        let mut total_lightness: i64 = 0;
        let mut value_count: i32 = 0;
        let x_start = if 0 < img_min.x { img_min.x } else { 0 };
        let x_end = if img_max.x < self.image_size.x - 1 { img_max.x } else { self.image_size.x - 1 };
        let y_start = if 0 < img_min.y { img_min.y } else { 0 };
        let y_end = if img_max.y < self.image_size.y - 1 { img_max.y } else { self.image_size.y - 1 };
        let mut x = x_start as i32 as Coord;
        while x <= x_end {
            let mut y = y_start as i32 as Coord;
            while y <= y_end {
                for z in 0..self.image_size.z {
                    total_lightness += self.pixel(x, y, z);
                    value_count += 1;
                }
                y += 1;
            }
            x += 1;
        }
        if value_count == 0 {
            // triangle falls outside of image or in between pixels, so we return the closest pixel
            let mut closest_pixel = Point2LL::new((img_min.x + img_max.x) / 2, (img_min.y + img_max.y) / 2);
            let clamp = |v: Coord, hi: Coord| -> Coord {
                let m = if v < hi { v } else { hi };
                if 0 < m {
                    m
                } else {
                    0
                }
            };
            closest_pixel.x = clamp(closest_pixel.x, self.image_size.x - 1);
            closest_pixel.y = clamp(closest_pixel.y, self.image_size.y - 1);
            for z in 0..self.image_size.z {
                total_lightness += self.pixel(closest_pixel.x, closest_pixel.y, z);
                value_count += 1;
            }
        }
        1.0 - (total_lightness as f64) / value_count as f64 / 255.0
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SierpinskiDirection {
    AcToAb,
    AcToBc,
    AbToBc,
}

#[derive(Clone, Copy, Debug)]
struct Edge {
    l: Point2LL,
    r: Point2LL,
}

#[derive(Clone, Debug)]
struct SierpinskiTriangle {
    /// C, the 90* corner of the triangle
    straight_corner: Point2LL,
    /// The corner closer to the start of the space filling curve
    a: Point2LL,
    /// The corner closer to the end of the space filling curve
    b: Point2LL,
    dir: SierpinskiDirection,
    straight_corner_is_left: bool,
    depth: i32,
    area: f64,
    requested_length: f64,
    realized_length: f64,
    total_child_realized_length: f64,
    error_left: f64,
    error_right: f64,
    children: Vec<usize>,
}

impl SierpinskiTriangle {
    fn new(straight_corner: Point2LL, a: Point2LL, b: Point2LL, dir: SierpinskiDirection, straight_corner_is_left: bool, depth: i32) -> Self {
        SierpinskiTriangle {
            straight_corner,
            a,
            b,
            dir,
            straight_corner_is_left,
            depth,
            area: 0.0,
            requested_length: 0.0,
            realized_length: 0.0,
            total_child_realized_length: 0.0,
            error_left: 0.0,
            error_right: 0.0,
            children: Vec::new(),
        }
    }

    fn get_from_edge(&self) -> Edge {
        let mut ret = match self.dir {
            SierpinskiDirection::AbToBc => Edge { l: self.a, r: self.b },
            SierpinskiDirection::AcToAb => Edge { l: self.straight_corner, r: self.a },
            SierpinskiDirection::AcToBc => Edge { l: self.straight_corner, r: self.a },
        };
        if !self.straight_corner_is_left {
            std::mem::swap(&mut ret.l, &mut ret.r);
        }
        ret
    }

    fn get_to_edge(&self) -> Edge {
        let mut ret = match self.dir {
            SierpinskiDirection::AbToBc => Edge { l: self.straight_corner, r: self.b },
            SierpinskiDirection::AcToAb => Edge { l: self.b, r: self.a },
            SierpinskiDirection::AcToBc => Edge { l: self.straight_corner, r: self.b },
        };
        if !self.straight_corner_is_left {
            std::mem::swap(&mut ret.l, &mut ret.r);
        }
        ret
    }

    fn get_total_error(&self) -> f64 {
        self.error_left + self.error_right
    }

    fn get_errored_value(&self) -> f64 {
        self.requested_length + self.get_total_error()
    }

    fn get_subdivision_error(&self) -> f64 {
        self.get_errored_value() - self.total_child_realized_length
    }

    fn get_value_error(&self) -> f64 {
        self.get_errored_value() - self.realized_length
    }
}

/// A list iterator: index of a list node; `END` is the sentinel.
type It = usize;
const END: It = 0;

#[derive(Clone, Copy, Debug)]
struct ListNode {
    tri: usize,
    prev: It,
    next: It,
}

/// `SierpinskiFill`: a (cross) fractal space filling curve with a density following a density provider.
pub struct SierpinskiFill {
    dithering: bool,
    constraint_error_diffusion: bool,
    use_errors_in_dithering: bool,
    line_width: Coord,
    max_depth: i32,
    /// All triangles; index 0 is the root.
    tris: Vec<SierpinskiTriangle>,
    /// `sequence_` as a doubly linked list with sentinel node 0.
    list: Vec<ListNode>,
}

impl SierpinskiFill {
    pub fn new(density_provider: &DensityProvider, aabb: AABB, max_depth: i32, line_width: Coord, dithering: bool) -> Self {
        let root = SierpinskiTriangle::new(NO_POINT, NO_POINT, NO_POINT, SierpinskiDirection::AbToBc, false, 0);
        let mut fill = SierpinskiFill {
            dithering,
            constraint_error_diffusion: dithering,
            use_errors_in_dithering: true,
            line_width,
            max_depth,
            tris: vec![root],
            list: vec![ListNode { tri: usize::MAX, prev: END, next: END }],
        };
        fill.create_tree(density_provider, aabb);
        fill.create_lower_bound_sequence();
        fill.settle_errors();
        fill.diffuse_error();
        fill
    }

    // ----- list helpers -----
    #[inline]
    fn begin(&self) -> It {
        self.list[END].next
    }
    #[inline]
    fn next(&self, it: It) -> It {
        self.list[it].next
    }
    #[inline]
    fn prev(&self, it: It) -> It {
        self.list[it].prev
    }
    #[inline]
    fn deref(&self, it: It) -> usize {
        debug_assert!(it != END);
        self.list[it].tri
    }
    fn insert_before(&mut self, pos: It, tri: usize) -> It {
        let prev = self.list[pos].prev;
        self.list.push(ListNode { tri, prev, next: pos });
        let id = self.list.len() - 1;
        self.list[prev].next = id;
        self.list[pos].prev = id;
        id
    }
    fn erase_range(&mut self, begin: It, end: It) {
        let before = self.list[begin].prev;
        self.list[before].next = end;
        self.list[end].prev = before;
    }

    fn create_tree(&mut self, density_provider: &DensityProvider, aabb: AABB) {
        let lt = Point2LL::new(aabb.min.x, aabb.max.y);
        let rb = Point2LL::new(aabb.max.x, aabb.min.y);

        let root_straight_corner_is_left = false;
        let root_depth = 1;
        let c0 = self.push_tri(SierpinskiTriangle::new(rb, aabb.min, aabb.max, SierpinskiDirection::AcToAb, root_straight_corner_is_left, root_depth));
        let c1 = self.push_tri(SierpinskiTriangle::new(lt, aabb.max, aabb.min, SierpinskiDirection::AcToAb, root_straight_corner_is_left, root_depth));
        self.tris[0].children = vec![c0, c1];
        for child in [c0, c1] {
            self.create_tree_rec(child);
        }

        // calculate node statistics
        self.create_tree_statistics(0);

        self.create_tree_requested_lengths(0, density_provider);
    }

    fn push_tri(&mut self, t: SierpinskiTriangle) -> usize {
        self.tris.push(t);
        self.tris.len() - 1
    }

    fn create_tree_rec(&mut self, sub_root: usize) {
        if self.tris[sub_root].depth < self.max_depth {
            // We need to subdivide.
            let t = self.tris[sub_root].clone();
            let middle = Point2LL::new((t.a.x + t.b.x) / 2, (t.a.y + t.b.y) / 2);
            // At each subdivision we divide the triangle in two.
            // Figure out which sort of triangle each child will be:
            let (first_dir, second_dir) = match t.dir {
                SierpinskiDirection::AbToBc => (SierpinskiDirection::AcToBc, SierpinskiDirection::AcToAb),
                SierpinskiDirection::AcToAb => (SierpinskiDirection::AbToBc, SierpinskiDirection::AcToBc),
                SierpinskiDirection::AcToBc => (SierpinskiDirection::AbToBc, SierpinskiDirection::AcToAb),
            };
            let c0 = self.push_tri(SierpinskiTriangle::new(middle, t.a, t.straight_corner, first_dir, !t.straight_corner_is_left, t.depth + 1));
            let c1 = self.push_tri(SierpinskiTriangle::new(middle, t.straight_corner, t.b, second_dir, !t.straight_corner_is_left, t.depth + 1));
            self.tris[sub_root].children = vec![c0, c1];
            for child in [c0, c1] {
                self.create_tree_rec(child);
            }
        }
    }

    fn create_tree_statistics(&mut self, triangle: usize) {
        let t = &mut self.tris[triangle];
        let ac = t.straight_corner - t.a;
        let area = 0.5 * int2mm2(v_size2(ac));
        let short_length = 0.5 * v_size_mm(ac);
        let long_length = 0.5 * v_size_mm(t.b - t.a);
        t.area = area;
        t.realized_length = if t.dir == SierpinskiDirection::AcToBc { long_length } else { short_length };
        let children = t.children.clone();
        for child in children {
            self.create_tree_statistics(child);
        }
    }

    fn create_tree_requested_lengths(&mut self, triangle: usize, density_provider: &DensityProvider) {
        if self.tris[triangle].children.is_empty() {
            // set requested_length of leaves
            let t = &self.tris[triangle];
            let mut triangle_aabb = AABB::default();
            triangle_aabb.include(t.a);
            triangle_aabb.include(t.b);
            triangle_aabb.include(t.straight_corner);
            let triangle_aabb3d =
                AABB3D::new(Point3LL::new(triangle_aabb.min.x, triangle_aabb.min.y, 0), Point3LL::new(triangle_aabb.max.x, triangle_aabb.max.y, 1));
            // The density of the square around the triangle is a rough estimate of the density of the triangle.
            let density = density_provider.density(&triangle_aabb3d);
            let line_width = self.line_width;
            let t = &mut self.tris[triangle];
            t.requested_length = density * t.area / int2mm(line_width);
        } else {
            // bubble total up requested_length and total_child_realized_length
            let children = self.tris[triangle].children.clone();
            for child in children {
                self.create_tree_requested_lengths(child, density_provider);

                let (requested, realized) = (self.tris[child].requested_length, self.tris[child].realized_length);
                self.tris[triangle].requested_length += requested;
                self.tris[triangle].total_child_realized_length += realized;
            }
        }
    }

    fn create_lower_bound_sequence(&mut self) {
        // sequence_.emplace_front(&root_)
        let b = self.begin();
        self.insert_before(b, 0);

        for _iteration in 0..999 {
            let mut change = false;
            change |= self.subdivide_all();

            if self.constraint_error_diffusion {
                change |= self.bubble_up_constraint_errors();
            }

            if !change {
                break;
            }
        }
    }

    /// `getDepthOrdered`. NOTE: the C++ code constructs `max_depth_ + 1` buckets and then resizes to
    /// `max_depth_`; the bucket for the maximum depth keeps working through `operator[]` but is not
    /// visited by range-based for loops. The returned `usize` is the number of visited buckets.
    fn get_depth_ordered(&self) -> (Vec<Vec<It>>, usize) {
        let mut depth_ordered: Vec<Vec<It>> = vec![Vec::new(); (self.max_depth + 2).max(1) as usize];
        let mut it = self.begin();
        while it != END {
            let node = self.deref(it);
            depth_ordered[self.tris[node].depth as usize].push(it);
            it = self.next(it);
        }
        (depth_ordered, self.max_depth.max(0) as usize)
    }

    fn subdivide_all(&mut self) -> bool {
        let (depth_ordered, iterable) = self.get_depth_ordered();

        let mut change = false;
        for depth_nodes in depth_ordered.iter().take(iterable) {
            for &it in depth_nodes {
                let node = self.deref(it);

                // The range of consecutive triangles to consider for subdivision simultaneously.
                // Two triangles connected to each other via the long edge must be subdivided simultaneously,
                // so then the range will be two long rather than one.
                let mut begin = it;
                let end = self.next(it);
                if self.tris[node].dir == SierpinskiDirection::AcToAb && end != END {
                    continue; // don't subdivide these two triangles just yet, wait till next iteration
                }
                if self.tris[node].dir == SierpinskiDirection::AbToBc && begin != self.begin() {
                    begin = self.prev(it);
                }
                let is_constrained = self.is_constrained_backward(begin) || self.is_constrained_forward(self.prev(end));
                // Don't check for constraining in between the cells in the range;
                // the range is defined as the range of triangles which are constraining each other simultaneously.

                if self.tris[node].depth == self.max_depth {
                    // Never subdivide beyond maximum depth.
                    continue;
                }
                let total_subdiv_error = self.get_subdivision_error_range(begin, end);
                if !self.tris[node].children.is_empty() && total_subdiv_error >= 0.0 && !is_constrained {
                    let redistribute_errors = true;
                    self.subdivide(begin, end, redistribute_errors);
                    change = true;
                }
            }
        }
        change
    }

    fn bubble_up_constraint_errors(&mut self) -> bool {
        let (depth_ordered, _) = self.get_depth_ordered();

        let mut redistributed_anything = false;

        let mut depth = self.max_depth;
        while depth >= 0 {
            let depth_nodes = &depth_ordered[depth as usize];
            for &it in depth_nodes {
                let node = self.deref(it);

                let mut unresolvable_error = self.tris[node].get_value_error();

                // If constrained in one direction, resolve the error in the other direction only.
                // If constrained in both directions, divide the error equally over both directions.
                let is_constrained_forward = self.is_constrained_forward(it);
                let is_constrained_backward = self.is_constrained_backward(it);
                if unresolvable_error > ALLOWED_LENGTH_ERROR && (is_constrained_forward || is_constrained_backward) {
                    if is_constrained_forward && is_constrained_backward {
                        // when constrained in both directions, then divide error equally
                        unresolvable_error *= 0.5;
                        // disperse half of the error forward and the other half backward
                    }
                    if is_constrained_forward {
                        let next = self.deref(self.next(it));
                        self.tris[node].error_right -= unresolvable_error;
                        self.tris[next].error_left += unresolvable_error;
                    }
                    if is_constrained_backward {
                        let prev = self.deref(self.prev(it));
                        self.tris[node].error_left -= unresolvable_error;
                        self.tris[prev].error_right += unresolvable_error;
                    }
                    if unresolvable_error.abs() > ALLOWED_LENGTH_ERROR {
                        redistributed_anything = true;
                    }
                }
            }
            depth -= 1;
        }
        redistributed_anything
    }

    /// Subdivide the nodes in [begin, end) and return the iterator to the last inserted child.
    fn subdivide(&mut self, begin: It, end: It, redistribute_errors: bool) -> It {
        if redistribute_errors {
            // move left-over errors
            let distribute_subdivision_errors = true;
            self.redistribute_leftover_errors(begin, end, distribute_subdivision_errors);

            let first = self.deref(begin);
            let last = self.deref(self.prev(end));
            let first_child = self.tris[first].children[0];
            let el = self.tris[first].error_left;
            self.tris[first_child].error_left += el;
            let last_child = *self.tris[last].children.last().unwrap();
            let er = self.tris[last].error_right;
            self.tris[last_child].error_right += er;
        }

        let mut first_child_it = self.prev(begin);
        // the actual subdivision
        let mut it = begin;
        while it != end {
            let node = self.deref(it);
            let children = self.tris[node].children.clone();
            for child in children {
                self.insert_before(begin, child);
            }
            it = self.next(it);
        }
        first_child_it = self.next(first_child_it);
        // removal of parents
        let last_child_it = self.prev(begin);
        self.erase_range(begin, end);

        if redistribute_errors {
            // make positive errors in children well balanced
            // Pass along error from parent
            let e = self.next(last_child_it);
            self.balance_errors(first_child_it, e);
        }

        last_child_it
    }

    fn redistribute_leftover_errors(&mut self, begin: It, end: It, distribute_subdivision_errors: bool) {
        let first = self.deref(begin);
        let last = self.deref(self.prev(end));

        // exchange intermediate errors
        let mut it = begin;
        while it != end && self.next(it) != end {
            let node = self.deref(it);
            let other = self.deref(self.next(it));
            let mut exchange = self.tris[node].error_right;
            if self.tris[node].error_right < self.tris[other].error_left {
                exchange *= -1.0;
            }
            self.tris[node].error_right -= exchange;
            self.tris[other].error_left += exchange;
            it = self.next(it);
        }

        let mut total_superfluous_error = 0.0;
        let mut it = begin;
        while it != end {
            let node = self.deref(it);
            total_superfluous_error += if distribute_subdivision_errors { self.tris[node].get_subdivision_error() } else { self.tris[node].get_value_error() };
            it = self.next(it);
        }
        if total_superfluous_error < ALLOWED_LENGTH_ERROR {
            // there is no significant left-over error
            return;
        }
        let at_begin = begin == self.begin();
        let at_end = end == END;
        if !at_begin && !at_end && self.tris[first].error_left > ALLOWED_LENGTH_ERROR && self.tris[last].error_right > ALLOWED_LENGTH_ERROR {
            let prev = self.deref(self.prev(begin));
            let next = self.deref(end);
            let total_error_input = self.tris[first].error_left + self.tris[last].error_right;
            // total superfluous error cannot be more than the influx of error
            total_superfluous_error = if total_error_input < total_superfluous_error { total_error_input } else { total_superfluous_error };
            let left_spillover = total_superfluous_error * self.tris[first].error_left / total_error_input;
            let right_spillover = total_superfluous_error * self.tris[last].error_right / total_error_input;
            self.tris[first].error_left -= left_spillover;
            self.tris[prev].error_right += left_spillover;
            self.tris[last].error_right -= right_spillover;
            self.tris[next].error_left += right_spillover;
        } else if !at_begin && self.tris[first].error_left > ALLOWED_LENGTH_ERROR {
            let prev = self.deref(self.prev(begin));
            let el = self.tris[first].error_left;
            total_superfluous_error = if el < total_superfluous_error { el } else { total_superfluous_error };
            self.tris[first].error_left -= total_superfluous_error;
            self.tris[prev].error_right += total_superfluous_error;
        } else if !at_end && self.tris[last].error_right > ALLOWED_LENGTH_ERROR {
            let next = self.deref(end);
            let er = self.tris[last].error_right;
            total_superfluous_error = if er < total_superfluous_error { er } else { total_superfluous_error };
            self.tris[last].error_right -= total_superfluous_error;
            self.tris[next].error_left += total_superfluous_error;
        }
    }

    fn balance_errors(&mut self, begin: It, end: It) {
        // copy sublist to array
        let mut nodes: Vec<usize> = Vec::new();
        let mut it = begin;
        while it != end {
            nodes.push(self.deref(it));
            it = self.next(it);
        }

        let mut node_error_compensation = vec![0.0f64; nodes.len()];

        // sort children on value_error, i.e. sort on total_value
        let mut order: Vec<usize> = (0..nodes.len()).collect();
        {
            let tris = &self.tris;
            cppstd::stable_sort_by(&mut order, |&a, &b| tris[nodes[a]].get_value_error() < tris[nodes[b]].get_value_error());
        }

        // add error to children with too low value
        let mut added = 0.0;
        let mut node_order_idx = 0;
        while node_order_idx < nodes.len() {
            let node_idx = order[node_order_idx];
            let value_error = self.tris[nodes[node_idx]].get_value_error();
            if value_error < 0.0 {
                added -= value_error;
                node_error_compensation[node_idx] = -value_error;
            } else {
                break;
            }
            node_order_idx += 1;
        }
        if added == 0.0 {
            return;
        }

        // subtract the added value from remaining children
        // divide acquired negative balancing error among remaining nodes with positive value error
        let mut subtracted = 0.0;
        // divide up added among remaining children in ratio to their value error
        let mut total_remaining_value_error = 0.0;
        for &node_idx in &order[node_order_idx..] {
            total_remaining_value_error += self.tris[nodes[node_idx]].get_value_error();
        }

        if total_remaining_value_error.abs() < 0.0001 {
            // Error is insignificant.
            return;
        }

        for &node_idx in &order[node_order_idx..] {
            let val_error = self.tris[nodes[node_idx]].get_value_error();
            let diff = added * val_error / total_remaining_value_error;
            subtracted += diff;
            node_error_compensation[node_idx] = -diff;
        }
        let _ = subtracted;

        let mut energy = 0.0;
        for (node_idx, &node) in nodes.iter().enumerate() {
            self.tris[node].error_left -= energy;
            energy += node_error_compensation[node_idx];
            self.tris[node].error_right += energy;
        }
    }

    fn settle_errors(&mut self) {
        let (depth_ordered, iterable) = self.get_depth_ordered();

        for depth_nodes in depth_ordered.iter().take(iterable) {
            for &it in depth_nodes {
                let n = self.next(it);
                self.redistribute_leftover_errors(it, n, false);
            }
        }
    }

    fn diffuse_error(&mut self) {
        let mut error = 0.0;
        let mut it = self.begin();
        while it != END {
            let triangle = self.deref(it);
            let t = &self.tris[triangle];

            let boundary = (t.realized_length + t.total_child_realized_length) * 0.5;

            let nodal_value = if self.use_errors_in_dithering { t.get_errored_value() } else { t.requested_length };

            let boundary_error = nodal_value - boundary + error;

            let mut begin = it;
            let end = self.next(it);
            if t.dir == SierpinskiDirection::AcToAb && end != END {
                // don't subdivide these two triangles just yet, wait till next iteration
                it = self.next(it);
                continue;
            }
            if t.dir == SierpinskiDirection::AbToBc && begin != self.begin() {
                begin = self.prev(it);
            }

            let mut is_constrained = false;
            let mut nested_it = begin;
            while nested_it != end {
                if self.is_constrained_backward(nested_it) || self.is_constrained_forward(nested_it) {
                    is_constrained = true;
                    break;
                }
                nested_it = self.next(nested_it);
            }
            let has_children = !self.tris[triangle].children.is_empty();
            if !is_constrained && boundary_error >= 0.0 && has_children {
                it = self.subdivide(begin, end, false);
                if self.dithering {
                    error += nodal_value - self.tris[triangle].total_child_realized_length;
                }
            } else if self.dithering {
                error += nodal_value - self.tris[triangle].realized_length;
            }
            it = self.next(it);
        }
    }

    fn is_constrained_backward(&self, it: It) -> bool {
        if it != self.begin() {
            let node = self.deref(it);
            if self.tris[node].dir == SierpinskiDirection::AbToBc {
                let prev = self.deref(self.prev(it));
                return self.tris[prev].depth < self.tris[node].depth;
            }
        }
        false
    }

    fn is_constrained_forward(&self, it: It) -> bool {
        if self.next(it) != END {
            let node = self.deref(it);
            if self.tris[node].dir == SierpinskiDirection::AcToAb {
                let next = self.deref(self.next(it));
                return self.tris[next].depth < self.tris[node].depth;
            }
        }
        false
    }

    fn get_subdivision_error_range(&self, begin: It, end: It) -> f64 {
        let mut ret = 0.0;
        let mut it = begin;
        while it != end {
            ret += self.tris[self.deref(it)].get_subdivision_error();
            it = self.next(it);
        }
        ret
    }

    /// `generateCross()`: the cross pattern, the same on all layers.
    pub fn generate_cross(&self) -> Polygon {
        let mut ret = Polygon::default();

        let mut it = self.begin();
        while it != END {
            let triangle = &self.tris[self.deref(it)];
            let mut edge_middle = triangle.a + triangle.b + triangle.straight_corner;
            match triangle.dir {
                SierpinskiDirection::AbToBc => edge_middle = edge_middle - triangle.a,
                SierpinskiDirection::AcToAb => edge_middle = edge_middle - triangle.straight_corner,
                SierpinskiDirection::AcToBc => edge_middle = edge_middle - triangle.a,
            }
            ret.points.push(Point2LL::new(edge_middle.x / 2, edge_middle.y / 2));
            it = self.next(it);
        }
        ret
    }

    /// `generateCross(z, min_dist_to_side, pocket_size)`: the cross-3D pattern for a given height.
    pub fn generate_cross_3d(&self, z: Coord, min_dist_to_side: Coord, pocket_size: Coord) -> Polygon {
        let mut ret = Polygon::default();

        // NOTE: the C++ std::function takes the period as `int`.
        let get_edge_crossing_location = |period: Coord, e: Edge| -> Point2LL {
            let period = period as i32 as Coord;
            let mut from_l = z % (period * 2);
            if from_l > period {
                from_l = period * 2 - from_l;
            }
            from_l = from_l * v_size(e.l - e.r) / period;
            from_l = if min_dist_to_side < from_l { from_l } else { min_dist_to_side };
            let upper = v_size(e.l - e.r) - min_dist_to_side;
            from_l = if from_l < upper { from_l } else { upper };
            e.l + normal(e.r - e.l, from_l)
        };

        let mut last_triangle: Option<usize> = None;
        let mut it = self.begin();
        while it != END {
            let tri = self.deref(it);
            let triangle = &self.tris[tri];

            // The length of a side of the triangle is used as the period of repetition. That way the
            // edges overhang by not more than 45 degrees.
            let period = v_size(triangle.straight_corner - triangle.a);
            ret.points.push(get_edge_crossing_location(period, triangle.get_from_edge()));

            last_triangle = Some(tri);
            it = self.next(it);
        }
        let last_triangle = &self.tris[last_triangle.expect("non-empty sierpinski sequence")];
        let period = v_size(last_triangle.straight_corner - last_triangle.a);
        ret.points.push(get_edge_crossing_location(period, last_triangle.get_to_edge()));

        if pocket_size > 10 {
            // round off corners by half square root 2 of the pocket size so that the whole hole will be sqrt_pocket_size wide
            let pocket_size_side = (pocket_size as f64 * SQRT2 / 2.0) as Coord;

            let mut pocketed = Polygon::default();
            pocketed.points.reserve(ret.points.len() * 3 / 2);

            let mut p0 = *ret.points.last().unwrap();
            for poly_idx in 0..ret.points.len() {
                let p1 = ret.points[poly_idx];
                let p2 = ret.points[(poly_idx + 1) % ret.points.len()];
                let v0 = p0 - p1;
                let v1 = p2 - p1;

                let prod = dot(v0, v1).abs();
                // allow for rounding errors of up to min_dist_to_side
                let is_straight_corner = (prod as f64) < ((v_size(v0) * v_size(v1)) as f64).sqrt() * min_dist_to_side as f64;
                if is_straight_corner {
                    let a = v_size(v0) / 3;
                    let b = v_size(v1) / 3;
                    let m = if a < pocket_size_side { a } else { pocket_size_side };
                    // a third so that if a line segment is shortened on both sides the middle remains
                    let pocket_rounding = if b < m { b } else { m };
                    pocketed.points.push(p1 + normal(v0, pocket_rounding));
                    pocketed.points.push(p1 + normal(v1, pocket_rounding));
                } else {
                    pocketed.points.push(p1);
                }
                p0 = p1;
            }

            pocketed
        } else {
            ret
        }
    }
}

/// `SierpinskiFillProvider`
pub struct SierpinskiFillProvider {
    /// The fill pattern if one and the same pattern is used on all layers
    fill_pattern_for_all_layers: Option<SierpinskiFill>,
}

/// Whether to employ dithering and error propagation
const USE_DITHERING: bool = true;

impl SierpinskiFillProvider {
    pub fn new(aabb_3d: &AABB3D, min_line_distance: Coord, line_width: Coord) -> Self {
        let (depth, aabb) = Self::get_fractal_config(aabb_3d, min_line_distance);
        let density_provider = DensityProvider::Uniform(line_width as f64 / min_line_distance as f64);
        SierpinskiFillProvider { fill_pattern_for_all_layers: Some(SierpinskiFill::new(&density_provider, aabb, depth, line_width, USE_DITHERING)) }
    }

    pub fn new_with_image(aabb_3d: &AABB3D, min_line_distance: Coord, line_width: Coord, cross_subdisivion_spec_image_file: &str) -> Self {
        let (depth, aabb) = Self::get_fractal_config(aabb_3d, min_line_distance);
        let density_provider = DensityProvider::Image(ImageBasedDensityProvider::new(cross_subdisivion_spec_image_file, aabb_3d.flatten()));
        SierpinskiFillProvider { fill_pattern_for_all_layers: Some(SierpinskiFill::new(&density_provider, aabb, depth, line_width, USE_DITHERING)) }
    }

    pub fn generate(&self, pattern: EFillMethod, z: Coord, line_width: Coord, pocket_size: Coord) -> Polygon {
        match &self.fill_pattern_for_all_layers {
            Some(fill) => {
                if pattern == EFillMethod::CROSS_3D {
                    fill.generate_cross_3d(z, line_width / 2, pocket_size)
                } else {
                    fill.generate_cross()
                }
            }
            None => {
                crate::log_error!("Different density sierpinski fill for different layers is not implemented yet!");
                std::process::exit(-1);
            }
        }
    }

    /// Get the parameters with which to generate a sierpinski fractal for this model.
    fn get_fractal_config(aabb_3d: &AABB3D, min_line_distance: Coord) -> (i32, AABB) {
        let model_aabb = aabb_3d.flatten();
        let model_aabb_size = model_aabb.max - model_aabb.min;
        let max_side_length = if model_aabb_size.x < model_aabb_size.y { model_aabb_size.y } else { model_aabb_size.x };
        let model_middle = model_aabb.get_middle();

        let mut depth = 0;
        let mut aabb_size = min_line_distance;
        while aabb_size < max_side_length {
            aabb_size *= 2;
            depth += 2;
        }
        let half_sqrt2 = 0.5 * SQRT2;
        if depth > 0 && aabb_size as f64 * half_sqrt2 >= max_side_length as f64 {
            aabb_size = (aabb_size as f64 * half_sqrt2) as Coord;
            depth -= 1;
        }

        let radius = Point2LL::new(aabb_size / 2, aabb_size / 2);
        let aabb = AABB::new(model_middle - radius, model_middle + radius);

        (depth, aabb)
    }
}
