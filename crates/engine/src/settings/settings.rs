//! `settings/Settings.h`: a string-keyed settings container with a parent chain.
//!
//! Every setting is stored as a string and converted on access, exactly like CuraEngine. Lookups
//! fall back to the extruder a setting is limited to (`limit_to_extruder`) and then to the
//! parent container (mesh -> extruder -> mesh group -> scene).

use super::enums::SettingEnum;
use super::flow_temp_graph::FlowTempGraph;
use super::types::*;
use crate::geometry::lines_set::Shape;
use crate::geometry::matrix::Matrix4x3D;
use crate::geometry::point2ll::pt;
use crate::utils::coord::{mm2int, Coord};
use std::collections::HashMap;
use std::f64::consts::PI;

pub const MAX_EXTRUDERS: usize = 16;
pub const MAX_INFILL_COMBINE: usize = 8;

pub const CURA_ENGINE_VERSION: &str = "5.12.0";

#[derive(Debug)]
pub struct Settings {
    settings: HashMap<String, String>,
    /// Parent container. The pointee is owned by the global scene and outlives this object
    /// (mirrors the raw `Settings* parent` of the C++ code).
    parent: *const Settings,
}

// Settings are only mutated during set-up; afterwards they're read concurrently.
unsafe impl Send for Settings {}
unsafe impl Sync for Settings {}

impl Clone for Settings {
    fn clone(&self) -> Self {
        Settings { settings: self.settings.clone(), parent: self.parent }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings { settings: HashMap::new(), parent: std::ptr::null() }
    }
}

impl Settings {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, key: &str, value: &str) {
        self.settings.insert(key.to_string(), value.to_string());
    }

    pub fn remove(&mut self, key: &str) {
        self.settings.remove(key);
    }

    pub fn has(&self, key: &str) -> bool {
        self.settings.contains_key(key)
    }

    pub fn set_parent(&mut self, new_parent: *const Settings) {
        self.parent = new_parent;
    }

    pub fn parent(&self) -> Option<&Settings> {
        if self.parent.is_null() {
            None
        } else {
            // SAFETY: parents are owned by the global scene which outlives all users.
            Some(unsafe { &*self.parent })
        }
    }

    /// Own keys (unordered).
    pub fn own_keys(&self) -> impl Iterator<Item = &String> {
        self.settings.keys()
    }

    /// `getKeys()`: the keys of the root of the parent chain.
    pub fn get_keys(&self) -> Vec<String> {
        if let Some(p) = self.parent() {
            return p.get_keys();
        }
        self.settings.keys().cloned().collect()
    }

    pub fn get_flattened_settings(&self) -> HashMap<String, String> {
        self.get_keys().into_iter().map(|k| {
            let v = self.get_string(&k);
            (k, v)
        }).collect()
    }

    /// Look up a raw setting value (`get<std::string>`), or None if it isn't set anywhere.
    pub fn try_get_str(&self, key: &str) -> Option<&str> {
        if let Some(v) = self.settings.get(key) {
            return Some(v.as_str());
        }
        if let Some(slice) = crate::application::try_current_slice() {
            if let Some(&extruder_nr) = slice.scene.limit_to_extruder.get(key) {
                return slice.scene.extruders[extruder_nr].settings.try_get_without_limiting(key);
            }
        }
        if let Some(p) = self.parent() {
            return p.try_get_str(key);
        }
        None
    }

    fn try_get_without_limiting(&self, key: &str) -> Option<&str> {
        if let Some(v) = self.settings.get(key) {
            Some(v.as_str())
        } else if let Some(p) = self.parent() {
            p.try_get_str(key)
        } else {
            None
        }
    }

    /// `get<std::string>`: a missing setting is a fatal error (exit code 2), as in CuraEngine.
    pub fn get_str(&self, key: &str) -> &str {
        match self.try_get_str(key) {
            Some(v) => v,
            None => {
                crate::log_error!("Trying to retrieve setting with no value given: {}", key);
                std::process::exit(2);
            }
        }
    }

    pub fn get_string(&self, key: &str) -> String {
        self.get_str(key).to_string()
    }

    /// `get<double>` (atof).
    pub fn get_f64(&self, key: &str) -> f64 {
        cppstd::atof(self.get_str(key))
    }

    /// `get<size_t>` (std::stoul).
    pub fn get_usize(&self, key: &str) -> usize {
        let s = self.get_str(key);
        match cppstd::stoul(s) {
            Some(v) => v as usize,
            None => {
                crate::log_error!("Invalid unsigned integer for setting {}: '{}'", key, s);
                std::process::exit(1);
            }
        }
    }

    /// `get<int>` (atoi).
    pub fn get_int(&self, key: &str) -> i32 {
        cppstd::atoi(self.get_str(key))
    }

    /// `get<bool>`.
    pub fn get_bool(&self, key: &str) -> bool {
        let value = self.get_str(key);
        if value == "on" || value == "yes" || value == "true" || value == "True" {
            return true;
        }
        cppstd::atoi(value) != 0
    }

    /// `get<coord_t>`: millimetres to micrometres.
    pub fn get_coord(&self, key: &str) -> Coord {
        mm2int(self.get_f64(key))
    }

    /// `get<LayerIndex>`: users count from 1, the engine from 0.
    pub fn get_layer_index(&self, key: &str) -> LayerIndex {
        cppstd::atoi(self.get_str(key)) as LayerIndex - 1
    }

    /// `get<AngleRadians>`: setting in degrees.
    pub fn get_angle_radians(&self, key: &str) -> AngleRadians {
        AngleRadians::new(self.get_f64(key) * PI / 180.0)
    }

    pub fn get_angle_degrees(&self, key: &str) -> AngleDegrees {
        AngleDegrees::new(self.get_f64(key))
    }

    pub fn get_temperature(&self, key: &str) -> Temperature {
        self.get_f64(key)
    }

    pub fn get_velocity(&self, key: &str) -> Velocity {
        self.get_f64(key)
    }

    pub fn get_acceleration(&self, key: &str) -> Acceleration {
        self.get_f64(key)
    }

    /// `get<Ratio>`: setting in percent.
    pub fn get_ratio(&self, key: &str) -> Ratio {
        self.get_f64(key) / 100.0
    }

    pub fn get_duration(&self, key: &str) -> Duration {
        Duration::new(self.get_f64(key))
    }

    pub fn get_enum<E: SettingEnum>(&self, key: &str) -> E {
        E::from_setting(self.get_str(key))
    }

    /// `get<ExtruderTrain&>`: index of the extruder (negative values mean `extruder_nr`).
    pub fn get_extruder_nr(&self, key: &str) -> usize {
        let mut extruder_nr = cppstd::atoi(self.get_str(key));
        if extruder_nr < 0 {
            extruder_nr = self.get_usize("extruder_nr") as i32;
        }
        extruder_nr as usize
    }

    /// `get<ExtruderTrain&>`.
    pub fn get_extruder_train(&self, key: &str) -> &'static crate::extruder_train::ExtruderTrain {
        let nr = self.get_extruder_nr(key);
        &crate::application::current_slice().scene.extruders[nr]
    }

    /// `get<std::vector<ExtruderTrain*>>`: all extruders if the value is negative.
    pub fn get_extruder_trains(&self, key: &str) -> Vec<usize> {
        let extruder_nr = cppstd::atoi(self.get_str(key));
        if extruder_nr < 0 {
            (0..crate::application::current_slice().scene.extruders.len()).collect()
        } else {
            vec![extruder_nr as usize]
        }
    }

    pub fn get_flow_temp_graph(&self, key: &str) -> FlowTempGraph {
        let value_string = self.get_str(key);
        let mut result = FlowTempGraph::default();
        if value_string.is_empty() {
            return result; // Empty at this point.
        }
        for (first, second) in match_point_pairs(value_string) {
            match (parse_stod(&first), parse_stod(&second)) {
                (Some(a), Some(b)) => result.data.push((a, b)),
                _ => crate::log_error!("Couldn't read 2D graph element [{},{}] in setting {}. Ignored.", first, second, key),
            }
        }
        result
    }

    /// `get<Shape>`: a list of polygons in millimetres, e.g. `[[[0,0],[1,0],[1,1]]]`.
    pub fn get_shape(&self, key: &str) -> Shape {
        let value_string = self.get_str(key);
        let mut result = Shape::default();
        if value_string.is_empty() {
            return result; // Empty at this point.
        }
        // Regex "\[(.*)\]?": everything after the first '[' up to the end of the line.
        let first_line = value_string.split('\n').next().unwrap_or("");
        let Some(start) = first_line.find('[') else {
            return result;
        };
        let polygons_string = &first_line[start + 1..];
        for polygon_str in match_polygons(polygons_string) {
            let poly = result.new_line();
            for (first, second) in match_point_pairs(&polygon_str) {
                match (parse_stod(&first), parse_stod(&second)) {
                    (Some(a), Some(b)) => poly.push(pt(mm2int(a), mm2int(b))),
                    _ => crate::log_error!("Couldn't read 2D graph element [{},{}] in setting '{}'. Ignored.", first, second, key),
                }
            }
        }
        result
    }

    /// `get<Matrix4x3D>`: a 3x3 matrix `[[a,b,c],[d,e,f],[g,h,i]]` (transposed into the matrix).
    pub fn get_matrix4x3d(&self, key: &str) -> Matrix4x3D {
        let value_string = self.get_str(key);
        let mut result = Matrix4x3D::default();
        if value_string.is_empty() {
            return result; // Standard matrix ([[1,0,0], [0,1,0], [0,0,1]]).
        }
        match parse_matrix_3x3(value_string) {
            Some(cells) => {
                let mut i = 0;
                for x in 0..3 {
                    for y in 0..3 {
                        result.m[y][x] = cppstd::atof(&cells[i]);
                        i += 1;
                    }
                }
            }
            None => {
                crate::log_warn!("Mesh transformation matrix could not be parsed!");
            }
        }
        result
    }

    /// `get<std::vector<double>>`.
    pub fn get_vec_f64(&self, key: &str) -> Vec<f64> {
        let value_string = self.get_str(key);
        let mut result = Vec::new();
        if value_string.is_empty() {
            return result;
        }
        // "\[([^\]]*)\]?": search
        let Some(start) = value_string.find('[') else {
            return result;
        };
        let rest = &value_string[start + 1..];
        let elements = match rest.find(']') {
            Some(e) => &rest[..e],
            None => rest,
        };
        for token in match_list_elements(elements) {
            match parse_stod(&token) {
                Some(v) => result.push(v),
                None => crate::log_error!("Couldn't read floating point value ({}) in setting {}. Ignored.", token, key),
            }
        }
        result
    }

    pub fn get_vec_ratio(&self, key: &str) -> Vec<Ratio> {
        self.get_vec_f64(key).into_iter().map(|v| v / 100.0).collect()
    }

    pub fn get_vec_int(&self, key: &str) -> Vec<i32> {
        self.get_vec_f64(key).into_iter().map(|v| v.round() as i32).collect()
    }

    pub fn get_vec_angle_degrees(&self, key: &str) -> Vec<AngleDegrees> {
        self.get_vec_f64(key).into_iter().map(AngleDegrees::new).collect()
    }

    /// `getAllSettingsString()`.
    pub fn get_all_settings_string(&self) -> String {
        let mut s = String::new();
        for (k, v) in &self.settings {
            s.push_str(&format!(" -s {}=\"{}\"", k, escape(v)));
        }
        s
    }
}

/// `std::stod`: parse with leading whitespace; None if no number could be parsed.
fn parse_stod(s: &str) -> Option<f64> {
    let (v, consumed) = cppstd::strtod_prefix(s);
    if consumed == 0 {
        None
    } else {
        Some(v)
    }
}

fn escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '\x07' => out.push_str("\\a"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\x0b' => out.push_str("\\v"),
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '"' => out.push_str("\\\""),
            '?' => out.push_str("\\?"),
            _ => out.push(c),
        }
    }
    out
}

/// Successive matches of `\[([^,\[]*),([^,\]]*)\]`, returning the two captured groups.
fn match_point_pairs(s: &str) -> Vec<(String, String)> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'[' {
            // group 1: [^,\[]*
            let mut j = i + 1;
            while j < b.len() && b[j] != b',' && b[j] != b'[' {
                j += 1;
            }
            if j < b.len() && b[j] == b',' {
                let g1 = &s[i + 1..j];
                let mut k = j + 1;
                while k < b.len() && b[k] != b',' && b[k] != b']' {
                    k += 1;
                }
                if k < b.len() && b[k] == b']' {
                    let g2 = &s[j + 1..k];
                    out.push((g1.to_string(), g2.to_string()));
                    i = k + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

fn is_ecma_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// Successive (whole) matches of `\[((\[[^\[\]]*\]\s*,?\s*)*)\]\s*,?`.
fn match_polygons(s: &str) -> Vec<String> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    'outer: while i < b.len() {
        if b[i] != b'[' {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        loop {
            // try one repetition of (\[[^\[\]]*\]\s*,?\s*)
            if j < b.len() && b[j] == b'[' {
                let mut k = j + 1;
                while k < b.len() && b[k] != b'[' && b[k] != b']' {
                    k += 1;
                }
                if k < b.len() && b[k] == b']' {
                    k += 1;
                    while k < b.len() && is_ecma_space(b[k]) {
                        k += 1;
                    }
                    if k < b.len() && b[k] == b',' {
                        k += 1;
                    }
                    while k < b.len() && is_ecma_space(b[k]) {
                        k += 1;
                    }
                    j = k;
                    continue;
                }
            }
            break;
        }
        if j < b.len() && b[j] == b']' {
            let mut k = j + 1;
            while k < b.len() && is_ecma_space(b[k]) {
                k += 1;
            }
            if k < b.len() && b[k] == b',' {
                k += 1;
            }
            out.push(s[i..k].to_string());
            i = k;
            continue 'outer;
        }
        i += 1;
    }
    out
}

/// Successive (whole) matches of `\s*([+-]?[0-9]*\.?[0-9]+)\s*,?`.
fn match_list_elements(s: &str) -> Vec<String> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let mut j = i;
        while j < b.len() && is_ecma_space(b[j]) {
            j += 1;
        }
        // Because of backtracking on \s*, a match may also start later; trying each start
        // position in order (like the regex engine) gives the same result as this greedy attempt
        // followed by i += 1 on failure.
        let mut k = j;
        if k < b.len() && (b[k] == b'+' || b[k] == b'-') {
            k += 1;
        }
        let d1_start = k;
        while k < b.len() && b[k].is_ascii_digit() {
            k += 1;
        }
        let d1 = k - d1_start;
        let mut matched_end = None;
        if k < b.len() && b[k] == b'.' && k + 1 < b.len() && b[k + 1].is_ascii_digit() {
            let mut m = k + 1;
            while m < b.len() && b[m].is_ascii_digit() {
                m += 1;
            }
            matched_end = Some(m);
        } else if d1 > 0 {
            matched_end = Some(k);
        }
        match matched_end {
            Some(mut m) => {
                while m < b.len() && is_ecma_space(b[m]) {
                    m += 1;
                }
                if m < b.len() && b[m] == b',' {
                    m += 1;
                }
                out.push(s[i..m].to_string());
                i = m;
            }
            None => i += 1,
        }
    }
    out
}

/// Parse `[[a,b,c],[d,e,f],[g,h,i]]` (whitespace tolerant), returning the 9 cell strings.
fn parse_matrix_3x3(s: &str) -> Option<Vec<String>> {
    let t = s.trim();
    let inner = t.strip_prefix('[')?.trim_start();
    let mut cells = Vec::new();
    let mut rest = inner;
    for row in 0..3 {
        rest = rest.trim_start();
        rest = rest.strip_prefix('[')?;
        let end = rest.find(']')?;
        let row_str = &rest[..end];
        let parts: Vec<&str> = row_str.split(',').collect();
        if parts.len() != 3 {
            return None;
        }
        for p in parts {
            let p = p.trim();
            if p.contains(' ') {
                return None;
            }
            cells.push(p.to_string());
        }
        rest = rest[end + 1..].trim_start();
        if row < 2 {
            rest = rest.strip_prefix(',')?;
        }
    }
    let rest = rest.trim_start().strip_prefix(']')?;
    if !rest.trim().is_empty() {
        return None;
    }
    Some(cells)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists() {
        assert_eq!(match_list_elements("0, 90").len(), 2);
        let mut s = Settings::new();
        s.add("a", "[0, 90, -45.5]");
        assert_eq!(s.get_vec_f64("a"), vec![0.0, 90.0, -45.5]);
        s.add("b", "[[-20, 10], [10, 10], [10, -10], [-20, -10]]");
        s.add("p", "[[[-20, 10], [10, 10], [10, -10]], [[1,2],[3,4],[5,6]]]");
        let shape = s.get_shape("p");
        assert_eq!(shape.len(), 2);
        assert_eq!(shape[0][0], pt(-20000, 10000));
        s.add("m", "[[1,0,0],[0,1,0],[0,0,1]]");
        let m = s.get_matrix4x3d("m");
        assert_eq!(m.m[0][0], 1.0);
        s.add("t", "True");
        assert!(s.get_bool("t"));
        s.add("f", "False");
        assert!(!s.get_bool("f"));
    }
}
