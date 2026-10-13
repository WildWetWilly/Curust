//! `utils/Segment3LL`, `utils/PlanarPolygon3LL`, `utils/VoxelGrid` and `utils/SpatialLookup`
//! (used by the multi-material mesh splitter).
//!
//! The voxel grid's occupation map is a `boost::concurrent_flat_map`, whose iteration order is
//! emulated with [`cppstd::boost_foa`]; the spatial lookup is a packed Boost R-tree, emulated with
//! [`cppstd::boost_rtree`].

use cppstd::boost_foa::FoaMap;
use cppstd::boost_rtree::PackedRtree3;

use crate::geometry::point3ll::Point3LL;
use crate::geometry::points_f::Point3D;
use crate::utils::aabb::AABB3D;
use crate::utils::coord::{Coord, EPSILON};

/// `Segment3LL`.
#[derive(Clone, Copy, Debug)]
pub struct Segment3LL {
    pub start: Point3LL,
    pub end: Point3LL,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LayerLocation {
    Below,
    Inside,
    Above,
}

#[derive(Clone, Copy)]
enum Axis {
    X,
    Y,
    Z,
}

impl Segment3LL {
    pub fn new(start: Point3LL, end: Point3LL) -> Self {
        Segment3LL { start, end }
    }

    fn point_at(&self, axis: Axis, c: Coord) -> Point3LL {
        let direction = self.end - self.start;
        match axis {
            Axis::X => {
                let factor = (c - self.start.x) as f64 / direction.x as f64;
                Point3LL::new(
                    c,
                    cppstd::llrint(self.start.y as f64 + factor * direction.y as f64),
                    cppstd::llrint(self.start.z as f64 + factor * direction.z as f64),
                )
            }
            Axis::Y => {
                let factor = (c - self.start.y) as f64 / direction.y as f64;
                Point3LL::new(
                    cppstd::llrint(self.start.x as f64 + factor * direction.x as f64),
                    c,
                    cppstd::llrint(self.start.z as f64 + factor * direction.z as f64),
                )
            }
            Axis::Z => {
                let factor = (c - self.start.z) as f64 / direction.z as f64;
                Point3LL::new(
                    cppstd::llrint(self.start.x as f64 + factor * direction.x as f64),
                    cppstd::llrint(self.start.y as f64 + factor * direction.y as f64),
                    c,
                )
            }
        }
    }

    fn point_is_inside(point: Coord, layer_start: Coord, layer_end: Coord) -> LayerLocation {
        if point < layer_start {
            LayerLocation::Below
        } else if point > layer_end {
            LayerLocation::Above
        } else {
            LayerLocation::Inside
        }
    }

    fn cropped_point(&self, axis: Axis, point: Point3LL, insideness: LayerLocation, layer_start: Coord, layer_end: Coord) -> Point3LL {
        match insideness {
            LayerLocation::Inside => point,
            LayerLocation::Below => self.point_at(axis, layer_start),
            LayerLocation::Above => self.point_at(axis, layer_end),
        }
    }

    fn intersection_with_layer(&self, axis: Axis, layer_start: Coord, layer_end: Coord) -> Option<Segment3LL> {
        let (sc, ec) = match axis {
            Axis::X => (self.start.x, self.end.x),
            Axis::Y => (self.start.y, self.end.y),
            Axis::Z => (self.start.z, self.end.z),
        };
        let segment_start_inside = Self::point_is_inside(sc, layer_start, layer_end);
        let segment_end_inside = Self::point_is_inside(ec, layer_start, layer_end);
        if segment_end_inside == segment_start_inside {
            if segment_start_inside == LayerLocation::Inside {
                return Some(*self);
            }
            return None;
        }
        let new_segment_start = self.cropped_point(axis, self.start, segment_start_inside, layer_start, layer_end);
        let new_segment_end = self.cropped_point(axis, self.end, segment_end_inside, layer_start, layer_end);
        if (new_segment_end - new_segment_start).v_size2() < EPSILON * EPSILON {
            return None;
        }
        Some(Segment3LL::new(new_segment_start, new_segment_end))
    }
}

/// `PlanarPolygon3LL`.
#[derive(Clone, Debug)]
pub struct PlanarPolygon3LL {
    segments: Vec<Segment3LL>,
}

impl PlanarPolygon3LL {
    pub fn new(segments: Vec<Segment3LL>) -> Self {
        PlanarPolygon3LL { segments }
    }

    fn intersection_with_layer(&self, axis: Axis, layer_start: Coord, layer_end: Coord) -> Option<PlanarPolygon3LL> {
        let mut new_segments: Vec<Segment3LL> = Vec::new();
        let join_segments = |new_segments: &mut Vec<Segment3LL>, next_segment: &Segment3LL| {
            let join_distance = (new_segments.last().unwrap().end - next_segment.start).v_size2();
            if join_distance > EPSILON * EPSILON {
                let s = Segment3LL::new(new_segments.last().unwrap().end, next_segment.start);
                new_segments.push(s);
            } else {
                new_segments.last_mut().unwrap().end = next_segment.start;
            }
        };
        for segment in &self.segments {
            if let Some(cropped_segment) = segment.intersection_with_layer(axis, layer_start, layer_end) {
                if !new_segments.is_empty() {
                    join_segments(&mut new_segments, &cropped_segment);
                }
                new_segments.push(cropped_segment);
            }
        }
        if new_segments.len() < 2 {
            return None;
        }
        let front = new_segments[0];
        join_segments(&mut new_segments, &front);
        Some(PlanarPolygon3LL::new(new_segments))
    }

    pub fn intersection_with_x_layer(&self, s: Coord, e: Coord) -> Option<PlanarPolygon3LL> {
        self.intersection_with_layer(Axis::X, s, e)
    }
    pub fn intersection_with_y_layer(&self, s: Coord, e: Coord) -> Option<PlanarPolygon3LL> {
        self.intersection_with_layer(Axis::Y, s, e)
    }
    pub fn intersection_with_z_layer(&self, s: Coord, e: Coord) -> Option<PlanarPolygon3LL> {
        self.intersection_with_layer(Axis::Z, s, e)
    }

    fn minmax(&self, f: impl Fn(&Segment3LL) -> Coord) -> (Coord, Coord) {
        let mut mn = f(&self.segments[0]);
        let mut mx = mn;
        for s in &self.segments[1..] {
            let v = f(s);
            if v < mn {
                mn = v;
            }
            if v > mx {
                mx = v;
            }
        }
        (mn, mx)
    }
    pub fn minmax_x(&self) -> (Coord, Coord) {
        self.minmax(|s| s.start.x)
    }
    pub fn minmax_y(&self) -> (Coord, Coord) {
        self.minmax(|s| s.start.y)
    }
    pub fn minmax_z(&self) -> (Coord, Coord) {
        self.minmax(|s| s.start.z)
    }
}

/// `VoxelGrid::LocalCoordinates` (a union with a 64-bit key: x | y << 16 | z << 32).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LocalCoordinates {
    pub x: u16,
    pub y: u16,
    pub z: u16,
}

impl LocalCoordinates {
    pub fn new(x: u16, y: u16, z: u16) -> Self {
        LocalCoordinates { x, y, z }
    }
    #[inline]
    pub fn key(&self) -> u64 {
        self.x as u64 | (self.y as u64) << 16 | (self.z as u64) << 32
    }
}

/// `hash_value(LocalCoordinates)` = `boost::hash<uint64_t>()(key)` (identity; mixed by the table).
pub fn local_coordinates_hash(p: &LocalCoordinates) -> u64 {
    p.key()
}

/// `double` → `uint16_t` as compiled for x86-64 (truncation through a 32-bit integer).
#[inline]
fn to_u16(v: f64) -> u16 {
    (v as i32) as u16
}

/// `VoxelGrid`.
pub struct VoxelGrid {
    resolution: Point3D,
    origin: Point3D,
    slices_count: Point3LL,
    occupied_voxels: FoaMap<LocalCoordinates, u8>,
}

impl VoxelGrid {
    pub fn new(bounding_box: &AABB3D, max_resolution: Coord) -> Self {
        let origin = Point3D::new(bounding_box.min.x as f64, bounding_box.min.y as f64, bounding_box.min.z as f64);
        let set_resolution = |span: f64| -> (Coord, f64) {
            let slices_count = (span / max_resolution as f64) as Coord + 1;
            (slices_count, span / slices_count as f64)
        };
        let (cx, rx) = set_resolution(bounding_box.span_x() as f64);
        let (cy, ry) = set_resolution(bounding_box.span_y() as f64);
        let (cz, rz) = set_resolution(bounding_box.span_z() as f64);
        VoxelGrid {
            resolution: Point3D::new(rx, ry, rz),
            origin,
            slices_count: Point3LL::new(cx, cy, cz),
            occupied_voxels: FoaMap::new(local_coordinates_hash),
        }
    }

    pub fn get_resolution(&self) -> &Point3D {
        &self.resolution
    }

    pub fn get_slices_count(&self) -> Point3LL {
        self.slices_count
    }

    pub fn to_global_coordinates(&self, p: &LocalCoordinates, at_center: bool) -> Point3D {
        Point3D::new(self.to_global_x(p.x, at_center), self.to_global_y(p.y, at_center), self.to_global_z(p.z, at_center))
    }

    /// `setOccupation` (`insert_or_assign`).
    pub fn set_occupation(&mut self, p: LocalCoordinates, extruder_nr: u8) {
        self.occupied_voxels.insert_or_assign(p, extruder_nr);
    }

    /// `setOrUpdateOccupation` (`insert_or_visit` keeping the minimum).
    pub fn set_or_update_occupation(&mut self, p: LocalCoordinates, extruder_nr: u8) {
        self.occupied_voxels.emplace_or_visit(p, extruder_nr, |v| *v = (*v).min(extruder_nr));
    }

    pub fn get_occupation(&self, p: &LocalCoordinates) -> Option<u8> {
        self.occupied_voxels.get(p).copied()
    }

    pub fn has_occupation(&self, p: &LocalCoordinates) -> bool {
        self.occupied_voxels.contains(p)
    }

    pub fn occupied_count(&self) -> usize {
        self.occupied_voxels.len()
    }

    /// `visitOccupiedVoxels` order.
    pub fn occupied_voxels(&self) -> impl Iterator<Item = (&LocalCoordinates, &u8)> {
        self.occupied_voxels.iter()
    }

    pub fn get_voxels_around(&self, point: &LocalCoordinates) -> Vec<LocalCoordinates> {
        let mut voxels_around = Vec::with_capacity(26);
        for dx in -1i64..2 {
            let pos_x = point.x as i64 + dx;
            if pos_x < 0 || pos_x >= self.slices_count.x {
                continue;
            }
            for dy in -1i64..2 {
                let pos_y = point.y as i64 + dy;
                if pos_y < 0 || pos_y >= self.slices_count.y {
                    continue;
                }
                for dz in -1i64..2 {
                    let pos_z = point.z as i64 + dz;
                    if pos_z < 0 || pos_z >= self.slices_count.z {
                        continue;
                    }
                    if dx != 0 || dy != 0 || dz != 0 {
                        voxels_around.push(LocalCoordinates::new(pos_x as u16, pos_y as u16, pos_z as u16));
                    }
                }
            }
        }
        voxels_around
    }

    pub fn to_local_x(&self, x: f64) -> u16 {
        to_u16((x - self.origin.x) / self.resolution.x)
    }
    pub fn to_local_y(&self, y: f64) -> u16 {
        to_u16((y - self.origin.y) / self.resolution.y)
    }
    pub fn to_local_z(&self, z: f64) -> u16 {
        to_u16((z - self.origin.z) / self.resolution.z)
    }
    pub fn to_global_x(&self, x: u16, at_center: bool) -> f64 {
        (x as f64 * self.resolution.x) + self.origin.x + if at_center { self.resolution.x / 2.0 } else { 0.0 }
    }
    pub fn to_global_y(&self, y: u16, at_center: bool) -> f64 {
        (y as f64 * self.resolution.y) + self.origin.y + if at_center { self.resolution.y / 2.0 } else { 0.0 }
    }
    pub fn to_global_z(&self, z: u16, at_center: bool) -> f64 {
        (z as f64 * self.resolution.z) + self.origin.z + if at_center { self.resolution.z / 2.0 } else { 0.0 }
    }

    /// `getTraversedVoxels`.
    pub fn get_traversed_voxels(&self, triangle: &[Point3LL; 3]) -> Vec<LocalCoordinates> {
        let mut traversed_voxels = Vec::new();
        let polygon = PlanarPolygon3LL::new(vec![
            Segment3LL::new(triangle[0], triangle[1]),
            Segment3LL::new(triangle[1], triangle[2]),
            Segment3LL::new(triangle[2], triangle[0]),
        ]);
        let (mnx, mxx) = polygon.minmax_x();
        let min_x = self.to_local_x(mnx as f64);
        let max_x = self.to_local_x(mxx as f64);
        let mut x = min_x;
        while x <= max_x {
            let layer_start_x = self.to_global_x(x, false);
            let layer_end_x = self.to_global_x(x.wrapping_add(1), false);
            if let Some(px) = polygon.intersection_with_x_layer(layer_start_x as Coord, layer_end_x as Coord) {
                let (mny, mxy) = px.minmax_y();
                let min_y = self.to_local_y(mny as f64);
                let max_y = self.to_local_y(mxy as f64);
                let mut y = min_y;
                while y <= max_y {
                    let layer_start_y = self.to_global_y(y, false);
                    let layer_end_y = self.to_global_y(y.wrapping_add(1), false);
                    if let Some(pxy) = px.intersection_with_y_layer(layer_start_y as Coord, layer_end_y as Coord) {
                        let (mnz, mxz) = pxy.minmax_z();
                        let min_z = self.to_local_z(mnz as f64);
                        let max_z = self.to_local_z(mxz as f64);
                        let mut z = min_z;
                        while z <= max_z {
                            let layer_start_z = self.to_global_z(z, false);
                            let layer_end_z = self.to_global_z(z.wrapping_add(1), false);
                            if pxy.intersection_with_z_layer(layer_start_z as Coord, layer_end_z as Coord).is_some() {
                                traversed_voxels.push(LocalCoordinates::new(x, y, z));
                            }
                            if z == u16::MAX {
                                break;
                            }
                            z += 1;
                        }
                    }
                    if y == u16::MAX {
                        break;
                    }
                    y += 1;
                }
            }
            if x == u16::MAX {
                break;
            }
            x += 1;
        }
        traversed_voxels
    }
}

/// `OccupiedPosition`.
#[derive(Clone, Copy, Debug)]
pub struct OccupiedPosition {
    pub position: Point3D,
    pub occupation: u8,
}

/// `SpatialLookup`: nearest occupied voxel lookup.
pub struct SpatialLookup {
    occupied_positions: Vec<OccupiedPosition>,
    tree: PackedRtree3,
}

impl SpatialLookup {
    /// `makeSpatialLookupFromVoxelGrid`.
    pub fn from_voxel_grid(voxel_grid: &VoxelGrid) -> Self {
        let occupied_positions: Vec<OccupiedPosition> = voxel_grid
            .occupied_voxels()
            .map(|(p, o)| OccupiedPosition { position: voxel_grid.to_global_coordinates(p, true), occupation: *o })
            .collect();
        let tree = PackedRtree3::new(occupied_positions.iter().map(|o| [o.position.x, o.position.y, o.position.z]).collect());
        SpatialLookup { occupied_positions, tree }
    }

    /// `findClosestOccupation`.
    pub fn find_closest_occupation(&self, position: &Point3D) -> Option<OccupiedPosition> {
        self.tree.nearest(&[position.x, position.y, position.z]).map(|i| self.occupied_positions[i])
    }
}
