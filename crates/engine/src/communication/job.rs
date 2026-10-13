//! Slicing driven by a job file: the in-process equivalent of `ArcusCommunication`.
//!
//! The front-end writes a [`SliceJob`] (the contents of Cura's `proto::Slice` message: global and
//! per-extruder settings, `limit_to_extruder` and the object lists with mesh data and per-object
//! settings), runs `curust engine-job <job> <result>`, reads progress lines from the engine's
//! standard output and finally the [`SliceResult`]: the G-code prefix (header), the G-code body,
//! the print time and material estimates and the layer data for the layer view, exactly as Cura's
//! `CuraEngineBackend` would receive them over the socket.

use super::Communication;
use crate::application::{current_slice_mut, set_current_slice};
use crate::extruder_train::ExtruderTrain;
use crate::fff_processor::FffProcessor;
use crate::geometry::matrix::Matrix4x3D;
use crate::geometry::point3ll::Point3LL;
use crate::geometry::points_f::{Point2F, Point3F};
use crate::mesh::Mesh;
use crate::print_feature::PrintFeatureType;
use crate::settings::settings::Settings;
use crate::settings::types::Velocity;
use crate::slice::Slice;
use crate::utils::coord::{int2mm, Coord};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------------------------
// Job and result data
// ---------------------------------------------------------------------------------------------

/// `proto::Object`.
#[derive(Clone, Debug, Default)]
pub struct JobObject {
    pub name: String,
    /// Three `f32` x/y/z vertices per face, in millimetres, Z up.
    pub vertices: Vec<f32>,
    /// Two `f32` per vertex (optional).
    pub uv_coordinates: Vec<f32>,
    /// PNG texture data (optional).
    pub texture: Vec<u8>,
    pub settings: Vec<(String, String)>,
}

/// `proto::ObjectList` (a mesh group).
#[derive(Clone, Debug, Default)]
pub struct JobObjectList {
    pub settings: Vec<(String, String)>,
    pub objects: Vec<JobObject>,
}

/// `proto::Slice`.
#[derive(Clone, Debug, Default)]
pub struct SliceJob {
    pub global_settings: Vec<(String, String)>,
    pub extruders: Vec<(i32, Vec<(String, String)>)>,
    pub limit_to_extruder: Vec<(String, i32)>,
    pub object_lists: Vec<JobObjectList>,
}

/// `proto::PathSegment`.
#[derive(Clone, Debug, Default)]
pub struct PathSegment {
    pub extruder: i32,
    /// Feature type per line segment.
    pub line_type: Vec<u8>,
    /// x, y, z per point (N + 1 points for N line segments).
    pub points: Vec<f32>,
    pub line_width: Vec<f32>,
    pub line_thickness: Vec<f32>,
    pub line_feedrate: Vec<f32>,
}

/// `proto::LayerOptimized`.
#[derive(Clone, Debug, Default)]
pub struct LayerData {
    pub id: i64,
    /// Z of the layer in micrometres.
    pub height: i64,
    pub thickness: i64,
    pub path_segments: Vec<PathSegment>,
}

/// Everything the engine sends back.
#[derive(Clone, Debug, Default)]
pub struct SliceResult {
    pub gcode_prefix: String,
    pub gcode: String,
    pub slice_uuid: String,
    /// Print time per feature in seconds, indexed by `PrintFeatureType`.
    pub time_per_feature: Vec<f64>,
    /// Material volume per extruder in mm³ (`None` when the extruder is not used).
    pub material_amounts: Vec<Option<f64>>,
    pub layers: Vec<LayerData>,
    /// Engine warnings and errors.
    pub log: Vec<String>,
    /// Byte lengths of the G-code messages (`GCodeLayer`) that make up `gcode`, in order.
    pub gcode_chunks: Vec<u32>,
}

impl SliceResult {
    /// The `proto::PrintTimeMaterialEstimates` fields, in Cura's order.
    pub fn time_estimates(&self) -> Vec<(&'static str, f64)> {
        let t = |f: PrintFeatureType| self.time_per_feature.get(f as usize).copied().unwrap_or(0.0);
        vec![
            ("time_infill", t(PrintFeatureType::Infill)),
            ("time_inset_0", t(PrintFeatureType::OuterWall)),
            ("time_inset_x", t(PrintFeatureType::InnerWall)),
            ("time_none", t(PrintFeatureType::NoneType)),
            ("time_retract", t(PrintFeatureType::StationaryRetractUnretract)),
            ("time_skin", t(PrintFeatureType::Skin)),
            ("time_skirt", t(PrintFeatureType::SkirtBrim)),
            ("time_support", t(PrintFeatureType::Support)),
            ("time_support_infill", t(PrintFeatureType::SupportInfill)),
            ("time_support_interface", t(PrintFeatureType::SupportInterface)),
            ("time_prime_tower", t(PrintFeatureType::PrimeTower)),
            (
                "time_travel",
                t(PrintFeatureType::MoveUnretracted) + t(PrintFeatureType::MoveRetracted) + t(PrintFeatureType::MoveWhileRetracting) + t(PrintFeatureType::MoveWhileUnretracting),
            ),
        ]
    }
}

// ---------------------------------------------------------------------------------------------
// Binary encoding
// ---------------------------------------------------------------------------------------------

const JOB_MAGIC: &[u8; 8] = b"CRSTJOB1";
const RESULT_MAGIC: &[u8; 8] = b"CRSTRES1";

#[derive(Default)]
pub struct Writer {
    pub buf: Vec<u8>,
}

impl Writer {
    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn i32(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn i64(&mut self, v: i64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn f64(&mut self, v: f64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn bytes(&mut self, b: &[u8]) {
        self.u32(b.len() as u32);
        self.buf.extend_from_slice(b);
    }
    pub fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }
    pub fn f32s(&mut self, v: &[f32]) {
        self.u32(v.len() as u32);
        for x in v {
            self.buf.extend_from_slice(&x.to_le_bytes());
        }
    }
    pub fn settings(&mut self, s: &[(String, String)]) {
        self.u32(s.len() as u32);
        for (k, v) in s {
            self.str(k);
            self.str(v);
        }
    }
}

pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.pos + n > self.data.len() {
            return Err("unexpected end of data".into());
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    pub fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn i32(&mut self) -> Result<i32, String> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn i64(&mut self) -> Result<i64, String> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn f64(&mut self) -> Result<f64, String> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn bytes(&mut self) -> Result<Vec<u8>, String> {
        let n = self.u32()? as usize;
        Ok(self.take(n)?.to_vec())
    }
    pub fn str(&mut self) -> Result<String, String> {
        String::from_utf8(self.bytes()?).map_err(|e| e.to_string())
    }
    pub fn f32s(&mut self) -> Result<Vec<f32>, String> {
        let n = self.u32()? as usize;
        let raw = self.take(n * 4)?;
        Ok(raw.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect())
    }
    pub fn settings(&mut self) -> Result<Vec<(String, String)>, String> {
        let n = self.u32()? as usize;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let k = self.str()?;
            let v = self.str()?;
            out.push((k, v));
        }
        Ok(out)
    }
}

impl SliceJob {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::default();
        w.buf.extend_from_slice(JOB_MAGIC);
        w.settings(&self.global_settings);
        w.u32(self.extruders.len() as u32);
        for (id, s) in &self.extruders {
            w.i32(*id);
            w.settings(s);
        }
        w.u32(self.limit_to_extruder.len() as u32);
        for (k, e) in &self.limit_to_extruder {
            w.str(k);
            w.i32(*e);
        }
        w.u32(self.object_lists.len() as u32);
        for ol in &self.object_lists {
            w.settings(&ol.settings);
            w.u32(ol.objects.len() as u32);
            for o in &ol.objects {
                w.str(&o.name);
                w.f32s(&o.vertices);
                w.f32s(&o.uv_coordinates);
                w.bytes(&o.texture);
                w.settings(&o.settings);
            }
        }
        w.buf
    }

    pub fn decode(data: &[u8]) -> Result<SliceJob, String> {
        if data.len() < 8 || &data[..8] != JOB_MAGIC {
            return Err("not a Curust slice job".into());
        }
        let mut r = Reader::new(&data[8..]);
        let mut job = SliceJob { global_settings: r.settings()?, ..Default::default() };
        for _ in 0..r.u32()? {
            let id = r.i32()?;
            job.extruders.push((id, r.settings()?));
        }
        for _ in 0..r.u32()? {
            let k = r.str()?;
            job.limit_to_extruder.push((k, r.i32()?));
        }
        for _ in 0..r.u32()? {
            let mut ol = JobObjectList { settings: r.settings()?, objects: Vec::new() };
            for _ in 0..r.u32()? {
                let name = r.str()?;
                let vertices = r.f32s()?;
                let uv_coordinates = r.f32s()?;
                let texture = r.bytes()?;
                let settings = r.settings()?;
                ol.objects.push(JobObject { name, vertices, uv_coordinates, texture, settings });
            }
            job.object_lists.push(ol);
        }
        Ok(job)
    }
}

impl SliceResult {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::default();
        w.buf.extend_from_slice(RESULT_MAGIC);
        w.str(&self.gcode_prefix);
        w.str(&self.gcode);
        w.str(&self.slice_uuid);
        w.u32(self.time_per_feature.len() as u32);
        for t in &self.time_per_feature {
            w.f64(*t);
        }
        w.u32(self.material_amounts.len() as u32);
        for m in &self.material_amounts {
            w.f64(m.unwrap_or(f64::NAN));
        }
        w.u32(self.layers.len() as u32);
        for l in &self.layers {
            w.i64(l.id);
            w.i64(l.height);
            w.i64(l.thickness);
            w.u32(l.path_segments.len() as u32);
            for p in &l.path_segments {
                w.i32(p.extruder);
                w.bytes(&p.line_type);
                w.f32s(&p.points);
                w.f32s(&p.line_width);
                w.f32s(&p.line_thickness);
                w.f32s(&p.line_feedrate);
            }
        }
        w.u32(self.log.len() as u32);
        for l in &self.log {
            w.str(l);
        }
        w.u32(self.gcode_chunks.len() as u32);
        for c in &self.gcode_chunks {
            w.u32(*c);
        }
        w.buf
    }

    pub fn decode(data: &[u8]) -> Result<SliceResult, String> {
        if data.len() < 8 || &data[..8] != RESULT_MAGIC {
            return Err("not a Curust slice result".into());
        }
        let mut r = Reader::new(&data[8..]);
        let mut res = SliceResult { gcode_prefix: r.str()?, gcode: r.str()?, slice_uuid: r.str()?, ..Default::default() };
        for _ in 0..r.u32()? {
            res.time_per_feature.push(r.f64()?);
        }
        for _ in 0..r.u32()? {
            let v = r.f64()?;
            res.material_amounts.push(if v.is_nan() { None } else { Some(v) });
        }
        for _ in 0..r.u32()? {
            let mut l = LayerData { id: r.i64()?, height: r.i64()?, thickness: r.i64()?, path_segments: Vec::new() };
            for _ in 0..r.u32()? {
                let extruder = r.i32()?;
                let line_type = r.bytes()?;
                let points = r.f32s()?;
                let line_width = r.f32s()?;
                let line_thickness = r.f32s()?;
                let line_feedrate = r.f32s()?;
                l.path_segments.push(PathSegment { extruder, line_type, points, line_width, line_thickness, line_feedrate });
            }
            res.layers.push(l);
        }
        for _ in 0..r.u32()? {
            res.log.push(r.str()?);
        }
        if let Ok(n) = r.u32() {
            for _ in 0..n {
                res.gcode_chunks.push(r.u32()?);
            }
        }
        Ok(res)
    }
}

// ---------------------------------------------------------------------------------------------
// The communication channel
// ---------------------------------------------------------------------------------------------

/// A `Write` sink shared between the G-code writer and the communication object
/// (`ArcusCommunication::Private::gcode_output_stream`).
#[derive(Clone, Default)]
struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

impl Write for SharedBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// `ArcusCommunication::PathCompiler`.
struct PathCompiler {
    layer_nr: i64,
    extruder: usize,
    line_types: Vec<u8>,
    line_widths: Vec<f32>,
    line_thicknesses: Vec<f32>,
    line_velocities: Vec<f32>,
    points: Vec<f32>,
    last_point: Point3LL,
}

/// `SliceDataStruct<proto::LayerOptimized>`.
#[derive(Default)]
struct OptimizedLayers {
    sliced_objects: usize,
    current_layer_count: i64,
    current_layer_offset: i64,
    slice_data: BTreeMap<i64, LayerData>,
}

pub struct JobCommunication {
    job: Option<SliceJob>,
    result_path: String,
    gcode_stream: SharedBuffer,
    result: std::cell::RefCell<SliceResult>,
    path: PathCompiler,
    layers: OptimizedLayers,
    object_count: usize,
    last_sent_progress: std::cell::Cell<i32>,
    slice_count: usize,
}

impl JobCommunication {
    pub fn new(job: SliceJob, result_path: &str) -> Self {
        JobCommunication {
            job: Some(job),
            result_path: result_path.to_string(),
            gcode_stream: SharedBuffer::default(),
            result: std::cell::RefCell::new(SliceResult::default()),
            path: PathCompiler {
                layer_nr: 0,
                extruder: 0,
                line_types: Vec::new(),
                line_widths: Vec::new(),
                line_thicknesses: Vec::new(),
                line_velocities: Vec::new(),
                points: Vec::new(),
                last_point: Point3LL::new(0, 0, 0),
            },
            layers: OptimizedLayers::default(),
            object_count: 0,
            last_sent_progress: std::cell::Cell::new(-1),
            slice_count: 0,
        }
    }

    /// `getOptimizedLayerById`.
    fn layer_by_id(&mut self, layer_nr: i64) -> &mut LayerData {
        let layer_nr = layer_nr + self.layers.current_layer_offset;
        if !self.layers.slice_data.contains_key(&layer_nr) {
            self.layers.current_layer_count += 1;
            self.layers.slice_data.insert(layer_nr, LayerData { id: layer_nr, ..Default::default() });
        }
        self.layers.slice_data.get_mut(&layer_nr).unwrap()
    }

    fn flush_path_segments(&mut self) {
        if self.path.line_types.is_empty() {
            return;
        }
        let seg = PathSegment {
            extruder: self.path.extruder as i32,
            line_type: std::mem::take(&mut self.path.line_types),
            points: std::mem::take(&mut self.path.points),
            line_width: std::mem::take(&mut self.path.line_widths),
            line_thickness: std::mem::take(&mut self.path.line_thicknesses),
            line_feedrate: std::mem::take(&mut self.path.line_velocities),
        };
        let layer_nr = self.path.layer_nr;
        self.layer_by_id(layer_nr).path_segments.push(seg);
    }

    fn add_point_3d(&mut self, p: Point3LL) {
        self.path.points.push(int2mm(p.x) as f32);
        self.path.points.push(int2mm(p.y) as f32);
        self.path.points.push(int2mm(p.z) as f32);
        self.path.last_point = p;
    }

    fn add_line_segment(&mut self, t: PrintFeatureType, p: Point3LL, width: Coord, thickness: Coord, velocity: Velocity) {
        self.add_point_3d(p);
        self.path.line_types.push(t as u8);
        self.path.line_widths.push(int2mm(width) as f32);
        self.path.line_thicknesses.push(int2mm(thickness) as f32);
        self.path.line_velocities.push(velocity as f32);
    }

    fn handle_initial_point(&mut self, p: Point3LL) {
        if self.path.points.is_empty() {
            self.add_point_3d(p);
        } else if p != self.path.last_point {
            self.add_line_segment(PrintFeatureType::NoneType, p, 1, 0, 0.0);
        }
    }

    /// `readMeshGroupMessage`.
    fn read_mesh_group(slice: &mut Slice, object_count: &mut usize, mesh_group_message: &JobObjectList) {
        if mesh_group_message.objects.is_empty() {
            return;
        }
        let mesh_group_idx = *object_count;
        for (k, v) in &mesh_group_message.settings {
            slice.scene.mesh_groups[mesh_group_idx].settings.add(k, v);
        }
        let matrix = Matrix4x3D::default();
        for object in &mesh_group_message.objects {
            let face_count = object.vertices.len() / 9;
            if face_count == 0 {
                crate::log_warn!("Got an empty mesh. Ignoring it!");
                continue;
            }
            let mut mesh = Mesh::default();
            for (k, v) in &object.settings {
                mesh.settings.add(k, v);
            }
            let extruder_nr = mesh.settings.get_usize("extruder_nr");
            let extruder_settings: *const Settings = &slice.scene.extruders[extruder_nr].settings;
            mesh.settings.set_parent(extruder_settings);
            let has_uv = object.uv_coordinates.len() >= face_count * 6;
            for face in 0..face_count {
                let v = &object.vertices[face * 9..face * 9 + 9];
                let p = |i: usize| Point3F { x: v[i * 3], y: v[i * 3 + 1], z: v[i * 3 + 2] }.to_point3d();
                let verts = [matrix.apply_3d(&p(0)), matrix.apply_3d(&p(1)), matrix.apply_3d(&p(2))];
                let uv = |i: usize| -> Option<Point2F> {
                    if has_uv {
                        Some(Point2F { x: object.uv_coordinates[face * 6 + i * 2], y: object.uv_coordinates[face * 6 + i * 2 + 1] })
                    } else {
                        None
                    }
                };
                mesh.add_face(verts[0], verts[1], verts[2], uv(0), uv(1), uv(2));
            }
            if !object.texture.is_empty() {
                crate::utils::mesh_utils::load_texture_from_png_data(&object.texture, &mut mesh, "network data", false);
            }
            mesh.mesh_name = object.name.clone();
            mesh.finish();
            slice.scene.mesh_groups[mesh_group_idx].meshes.push(mesh);
        }
        *object_count += 1;
        slice.scene.mesh_groups[mesh_group_idx].finalize();
    }

    fn write_result(&mut self) {
        let mut res = std::mem::take(&mut *self.result.borrow_mut());
        res.layers = std::mem::take(&mut self.layers.slice_data).into_values().collect();
        let data = res.encode();
        if let Err(e) = std::fs::write(&self.result_path, data) {
            crate::log_error!("Failed to write slice result {}: {}", self.result_path, e);
        }
    }
}

impl Communication for JobCommunication {
    fn has_slice(&self) -> bool {
        self.slice_count < 1
    }
    fn is_sequential(&self) -> bool {
        false
    }
    fn send_progress(&self, progress: f64) {
        let rounded = (1000.0 * progress) as i32;
        if self.last_sent_progress.get() == rounded {
            return;
        }
        let n = self.object_count.max(1) as f64;
        let mut all = progress / n;
        all += self.layers.sliced_objects as f64 * (1.0 / n);
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "PROGRESS {:.4}", all);
        let _ = out.flush();
        self.last_sent_progress.set(rounded);
    }
    fn send_layer_complete(&mut self, layer_nr: i64, z: Coord, thickness: Coord) {
        let l = self.layer_by_id(layer_nr);
        l.height = z;
        l.thickness = thickness;
    }
    fn send_line_to(&mut self, feature_type: PrintFeatureType, to: Point3LL, line_width: Coord, line_thickness: Coord, velocity: Velocity) {
        if to != self.path.last_point {
            self.add_line_segment(feature_type, to, line_width, line_thickness, velocity);
        }
    }
    fn send_current_position(&mut self, position: Point3LL) {
        self.handle_initial_point(position);
    }
    fn set_extruder_for_send(&mut self, extruder: &ExtruderTrain) {
        if self.path.extruder != extruder.extruder_nr {
            self.flush_path_segments();
            self.path.extruder = extruder.extruder_nr;
        }
    }
    fn set_layer_for_send(&mut self, layer_nr: i64) {
        if self.path.layer_nr != layer_nr {
            self.flush_path_segments();
            self.path.layer_nr = layer_nr;
        }
    }
    fn send_optimized_layer_data(&mut self) {
        self.flush_path_segments();
        self.layers.sliced_objects += 1;
        self.layers.current_layer_offset = self.layers.current_layer_count;
        // All layers are kept until the result is written.
        if self.layers.sliced_objects >= self.object_count {
            self.layers.sliced_objects = 0;
        }
    }
    fn send_print_time_material_estimates(&self) {
        let fff = FffProcessor::get_instance();
        let times = fff.get_total_print_time_per_feature();
        let mut res = self.result.borrow_mut();
        res.time_per_feature = times.iter().map(|d| d.value()).collect();
        let n = crate::application::current_slice().scene.extruders.len();
        res.material_amounts = (0..n)
            .map(|e| if fff.gcode_writer.get_extruder_actual_use(e) { Some(fff.get_total_filament_used(e)) } else { None })
            .collect();
    }
    fn begin_gcode(&mut self) {
        FffProcessor::get_instance().gcode_writer.set_target_stream(Box::new(self.gcode_stream.clone()));
    }
    fn flush_gcode(&mut self) {
        FffProcessor::get_instance().gcode_writer.flush_target_stream();
        let mut buf = self.gcode_stream.0.lock().unwrap();
        if buf.is_empty() {
            return;
        }
        let text = String::from_utf8_lossy(&buf).into_owned();
        let mut res = self.result.borrow_mut();
        res.gcode_chunks.push(text.len() as u32);
        res.gcode.push_str(&text);
        buf.clear();
    }
    fn send_gcode_prefix(&self, prefix: &str) {
        self.result.borrow_mut().gcode_prefix = prefix.to_string();
    }
    fn send_slice_uuid(&self, slice_uuid: &str) {
        self.result.borrow_mut().slice_uuid = slice_uuid.to_string();
    }
    fn send_finished_slicing(&self) {}

    /// `ArcusCommunication::sliceNext`.
    fn slice_next(&mut self) {
        let Some(job) = self.job.take() else {
            self.slice_count += 1;
            return;
        };
        set_current_slice(Slice::new(job.object_lists.len()));
        let slice = current_slice_mut();

        // readGlobalSettingsMessage
        for (k, v) in &job.global_settings {
            slice.scene.settings.add(k, v);
        }
        // readExtruderSettingsMessage
        let extruder_count = slice.scene.settings.get_usize("machine_extruder_count");
        slice.scene.extruders.reserve(extruder_count);
        let scene_settings: *const Settings = &slice.scene.settings;
        for extruder_nr in 0..extruder_count {
            slice.scene.extruders.push(ExtruderTrain::new(extruder_nr, scene_settings));
        }
        for (id, settings) in &job.extruders {
            if *id < 0 || *id as usize >= extruder_count {
                crate::log_warn!("Received extruder index that is out of range: {}", id);
                continue;
            }
            for (k, v) in settings {
                slice.scene.extruders[*id as usize].settings.add(k, v);
            }
        }
        let extruder_count = slice.scene.extruders.len();
        for (name, extruder_nr) in &job.limit_to_extruder {
            if *extruder_nr < 0 || *extruder_nr as usize >= extruder_count {
                continue;
            }
            slice.scene.limit_to_extruder.entry(name.clone()).or_insert(*extruder_nr as usize);
        }
        // Load all mesh groups, meshes and their settings.
        self.object_count = 0;
        for ol in &job.object_lists {
            Self::read_mesh_group(slice, &mut self.object_count, ol);
        }

        if !slice.scene.mesh_groups.is_empty() {
            slice.compute();
            FffProcessor::get_instance().finalize();
            self.flush_gcode();
            self.send_print_time_material_estimates();
            self.send_finished_slicing();
        }
        self.slice_count += 1;
        self.write_result();
    }
}

/// `curust engine-job <job> <result>`: run one slice job.
pub fn run_job(job_path: &str, result_path: &str) -> Result<(), String> {
    let mut data = Vec::new();
    std::fs::File::open(job_path).and_then(|mut f| f.read_to_end(&mut data)).map_err(|e| format!("{}: {}", job_path, e))?;
    let job = SliceJob::decode(&data)?;
    crate::application::set_communication(Box::new(JobCommunication::new(job, result_path)));
    crate::application::start_thread_pool(0);
    while crate::application::communication().has_slice() {
        crate::application::communication().slice_next();
    }
    Ok(())
}
