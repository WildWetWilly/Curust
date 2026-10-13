//! Cura's version upgrades of `quality_changes` profiles (the `VersionUpgrade*` plug-ins'
//! `upgradeInstanceContainer` steps from Cura 3.5 / 4.0 to 5.12), on top of an emulation of
//! Python's `configparser` (which the plug-ins use to read and write the files).

/// Python `configparser.ConfigParser(interpolation=None)`: ordered sections of ordered,
/// lower-cased options.
#[derive(Clone, Debug, Default)]
pub struct ConfigParser {
    pub sections: Vec<(String, Vec<(String, String)>)>,
}

impl ConfigParser {
    /// `read_string`. `comment_prefixes`: `("#", ";")` by default, `()` in most upgrades.
    pub fn read(text: &str, comments: bool) -> Result<ConfigParser, String> {
        Self::read_opts(text, comments, true)
    }

    /// `read_string` with `optionxform = str` when `lowercase` is false.
    pub fn read_opts(text: &str, comments: bool, lowercase: bool) -> Result<ConfigParser, String> {
        let mut p = ConfigParser::default();
        let mut cur: Option<usize> = None;
        let mut optname: Option<String> = None;
        let mut indent_level = usize::MAX;
        // Each option's value lines, joined at the end.
        let mut lines_of: Vec<Vec<Vec<String>>> = Vec::new();
        let text = text.replace("\r\n", "\n");
        for line in text.split_inclusive('\n') {
            let line = line.strip_suffix('\n').unwrap_or(line);
            let stripped = line.trim();
            let is_comment = comments && (stripped.starts_with('#') || stripped.starts_with(';'));
            let value = if is_comment { "" } else { stripped };
            if value.is_empty() {
                // empty_lines_in_values: an empty line continues a value; a comment does not.
                if !is_comment {
                    if let (Some(s), Some(o)) = (cur, &optname) {
                        if let Some(k) = p.sections[s].1.iter().position(|(n, _)| n == o) {
                            lines_of[s][k].push(String::new());
                        }
                    }
                } else {
                    indent_level = usize::MAX;
                }
                continue;
            }
            let cur_indent = line.len() - line.trim_start().len();
            if let (Some(s), Some(o)) = (cur, &optname) {
                if cur_indent > indent_level {
                    if let Some(k) = p.sections[s].1.iter().position(|(n, _)| n == o) {
                        lines_of[s][k].push(value.to_string());
                    }
                    continue;
                }
            }
            indent_level = cur_indent;
            // SECTCRE = \[(?P<header>.+)\]  (matched at the start, `.+` greedy)
            if value.starts_with('[') {
                if let Some(end) = value.rfind(']') {
                    if end > 1 {
                        let header = &value[1..end];
                        if p.sections.iter().any(|(n, _)| n == header) {
                            return Err(format!("While reading: section '{}' already exists", header));
                        }
                        p.sections.push((header.to_string(), Vec::new()));
                        lines_of.push(Vec::new());
                        cur = Some(p.sections.len() - 1);
                        optname = None;
                        continue;
                    }
                }
            }
            let Some(s) = cur else { return Err("File contains no section headers.".into()) };
            // OPTCRE = (?P<option>.*?)\s*(?P<vi>=|:)\s*(?P<value>.*)$
            let Some(d) = value.find(|c| c == '=' || c == ':') else { return Err(format!("Source contains parsing errors: {}", value)) };
            let name = if lowercase { value[..d].trim_end().to_lowercase() } else { value[..d].trim_end().to_string() };
            if name.is_empty() {
                return Err(format!("Source contains parsing errors: {}", value));
            }
            if p.sections[s].1.iter().any(|(n, _)| *n == name) {
                return Err(format!("While reading: option '{}' in section '{}' already exists", name, p.sections[s].0));
            }
            let v = value[d + 1..].trim().to_string();
            p.sections[s].1.push((name.clone(), String::new()));
            lines_of[s].push(vec![v]);
            optname = Some(name);
        }
        for (s, sec) in p.sections.iter_mut().enumerate() {
            for (k, (_, v)) in sec.1.iter_mut().enumerate() {
                *v = lines_of[s][k].join("\n").trim_end().to_string();
            }
        }
        Ok(p)
    }

    /// `write` (with spaces around `=`).
    pub fn write(&self) -> String {
        let mut out = String::new();
        for (name, items) in &self.sections {
            out.push_str(&format!("[{}]\n", name));
            for (k, v) in items {
                out.push_str(&format!("{} = {}\n", k, v.replace('\n', "\n\t")));
            }
            out.push('\n');
        }
        out
    }

    pub fn has_section(&self, s: &str) -> bool {
        self.sections.iter().any(|(n, _)| n == s)
    }

    fn section_mut(&mut self, s: &str) -> &mut Vec<(String, String)> {
        if !self.has_section(s) {
            self.sections.push((s.to_string(), Vec::new()));
        }
        &mut self.sections.iter_mut().find(|(n, _)| n == s).unwrap().1
    }

    pub fn get(&self, s: &str, k: &str) -> Option<&str> {
        let k = k.to_lowercase();
        self.sections.iter().find(|(n, _)| n == s)?.1.iter().find(|(n, _)| *n == k).map(|(_, v)| v.as_str())
    }

    pub fn has(&self, s: &str, k: &str) -> bool {
        self.get(s, k).is_some()
    }

    /// `parser[s][k] = v` (the section must exist, like Python which raises KeyError).
    pub fn set(&mut self, s: &str, k: &str, v: &str) -> Result<(), String> {
        if !self.has_section(s) {
            return Err(format!("KeyError: '{}'", s));
        }
        let k = k.to_lowercase();
        let sec = self.section_mut(s);
        if let Some(slot) = sec.iter_mut().find(|(n, _)| *n == k) {
            slot.1 = v.to_string();
        } else {
            sec.push((k, v.to_string()));
        }
        Ok(())
    }

    pub fn remove(&mut self, s: &str, k: &str) {
        let k = k.to_lowercase();
        if self.has_section(s) {
            self.section_mut(s).retain(|(n, _)| *n != k);
        }
    }

    /// `if s not in parser: parser[s] = {}`.
    pub fn ensure_section(&mut self, s: &str) {
        self.section_mut(s);
    }
}

/// `UM.Util.parseBool`.
fn parse_bool(v: &str) -> bool {
    matches!(v, "True" | "true" | "Yes" | "yes")
}

/// Python `float(str)`.
fn py_float(s: &str) -> Result<f64, String> {
    let t = s.trim().replace('_', "");
    let l = t.to_lowercase();
    let r = match l.trim_start_matches(['+', '-']) {
        "inf" | "infinity" => Ok(if l.starts_with('-') { f64::NEG_INFINITY } else { f64::INFINITY }),
        "nan" => Ok(f64::NAN),
        _ => t.parse::<f64>().map_err(|_| ()),
    };
    r.map_err(|_| format!("ValueError: could not convert string to float: '{}'", s))
}

/// Python `round(x)` (half to even) as an integer.
fn py_round(x: f64) -> i64 {
    let r = x.round();
    if (x - x.trunc()).abs() == 0.5 {
        (2.0 * (x / 2.0).round()) as i64
    } else {
        r as i64
    }
}

fn strip_eq(v: &str) -> &str {
    v.strip_prefix('=').unwrap_or(v)
}

fn basename(f: &str) -> &str {
    f.rsplit(['/', '\\']).next().unwrap_or(f)
}

include!("upgrade_tables.rs");

/// `VersionUpgradeManager.updateFilesData("quality_changes", ...)` for one file: runs the
/// upgrade steps until the profile is at version 4 / setting version 26 (Cura 5.12). Profiles
/// that are already current come back unchanged.
pub fn upgrade_quality_changes(serialized: &str, filename: &str) -> Result<String, String> {
    let mut text = serialized.to_string();
    for _ in 0..64 {
        let p = crate::instance::parse_cfg(&text);
        let get = |sec: &str, k: &str| p.iter().find(|(s, _)| s == sec).and_then(|(_, kv)| kv.iter().find(|(x, _)| x == k)).map(|(_, v)| v.clone());
        let version: i64 = get("general", "version").and_then(|v| v.trim().parse().ok()).ok_or("Missing required 'version' property")?;
        let setting_version: i64 = get("metadata", "setting_version").and_then(|v| v.trim().parse().ok()).unwrap_or(0);
        let v = version * 1_000_000 + setting_version;
        if v >= 4_000_026 {
            return Ok(text);
        }
        text = match step(v, &text, filename)? {
            Some(t) => t,
            None => return Err(format!("This profile is from a Cura version that is too old to import (file format {}.{}).", version, setting_version)),
        };
    }
    Err("Profile upgrade did not finish".into())
}

/// One upgrade step from configuration version `v`; `None` when there is no step.
fn step(v: i64, text: &str, filename: &str) -> Result<Option<String>, String> {
    let read = |comments: bool| ConfigParser::read(text, comments);
    let mut p;
    match v {
        // VersionUpgrade34to35
        4_000_004 => {
            p = read(true)?;
            p.set("general", "version", "4")?;
            p.set("metadata", "setting_version", "5")?;
            if p.has_section("values") {
                for key in ["infill_pattern", "support_pattern", "support_interface_pattern", "support_roof_pattern", "support_bottom_pattern"] {
                    if p.get("values", key) == Some("concentric_3d") {
                        p.remove("values", key);
                    }
                }
                for k in ["dual_pre_wipe", "prime_tower_purge_volume", "prime_tower_wall_thickness"] {
                    p.remove("values", k);
                }
                if p.get("values", "retraction_combing") == Some("noskin") {
                    p.set("values", "retraction_combing", "infill")?;
                }
            }
        }
        // VersionUpgrade35to40
        4_000_005 => {
            p = read(true)?;
            p.set("general", "version", "4")?;
            p.set("metadata", "setting_version", "6")?;
            if p.has_section("values") {
                p.remove("values", "bridge_wall_max_overhang");
            }
        }
        // VersionUpgrade40to41
        4_000_006 => {
            p = read(true)?;
            p.set("general", "version", "4")?;
            p.set("metadata", "setting_version", "7")?;
            if let Some(res) = p.get("values", "meshfix_maximum_resolution").map(String::from) {
                p.set("values", "meshfix_maximum_deviation", &format!("=({}) / 2", strip_eq(&res)))?;
                p.remove("values", "meshfix_maximum_resolution");
            }
        }
        // VersionUpgrade41to42
        4_000_007 => {
            p = read(false)?;
            p.set("metadata", "setting_version", "8")?;
            let old_def = p.get("general", "definition").ok_or("KeyError: 'definition'")?.to_string();
            if let Some((_, n)) = RENAMED_PROFILES_41_42.iter().find(|(o, _)| *o == old_def) {
                p.set("general", "definition", n)?;
            }
            if p.has_section("values") {
                if let Some(v) = p.get("values", "support_minimal_diameter").map(String::from) {
                    p.set("values", "support_tower_maximum_supported_diameter", &v)?;
                    p.remove("values", "support_minimal_diameter");
                }
                for k in ["prime_tower_circular", "max_feedrate_z_override"] {
                    p.remove("values", k);
                }
            }
            if p.get("metadata", "type") == Some("quality_changes") {
                for printer in QUALITY_CHANGES_TO_CREALITY_BASE {
                    if basename(filename).starts_with(&format!("{}_", printer)) {
                        p.set("general", "definition", "creality_base")?;
                        let qt = p.get("metadata", "quality_type").ok_or("KeyError: 'quality_type'")?.to_string();
                        let n = CREALITY_LIMITED_QUALITY_TYPE.iter().find(|(o, _)| *o == qt).map(|(_, n)| *n).unwrap_or("draft");
                        p.set("metadata", "quality_type", n)?;
                        break;
                    }
                }
            }
        }
        // VersionUpgrade42to43
        4_000_008 => {
            p = read(false)?;
            p.set("metadata", "setting_version", "9")?;
            if p.has_section("values") {
                if let Some(v) = p.get("values", "support_infill_angle").map(String::from) {
                    p.set("values", "support_infill_angles", &v)?;
                    p.remove("values", "support_infill_angle");
                }
                p.remove("values", "start_layers_at_same_position");
                if let Some(v) = p.get("values", "support_infill_angles").map(String::from) {
                    let f = py_float(&v)?;
                    p.set("values", "support_infill_angles", &format!("[{}]", py_round(f)))?;
                }
            }
        }
        // VersionUpgrade43to44
        4_000_009 => {
            p = read(false)?;
            p.set("metadata", "setting_version", "10")?;
            if p.get("metadata", "type").unwrap_or("") == "quality_changes" {
                p.set("metadata", "intent_category", "default")?;
            }
            if p.has_section("values") {
                if p.get("values", "skin_alternate_rotation").map(parse_bool).unwrap_or(false) {
                    p.set("values", "skin_angles", "[45, 135, 0, 90]")?;
                }
                if let Some(v) = p.get("values", "adaptive_layer_height_threshold").map(String::from) {
                    p.set("values", "adaptive_layer_height_threshold", &format!("=({}) / 1000", strip_eq(&v)))?;
                }
            }
        }
        // VersionUpgrade44to45
        4_000_010 => {
            p = read(false)?;
            p.set("metadata", "setting_version", "11")?;
            if p.has_section("values") {
                for (preferred, removed) in [("machine_head_with_fans_polygon", "machine_head_polygon"), ("support_wall_count", "support_tree_wall_count")] {
                    if let Some(v) = p.get("values", removed).map(String::from) {
                        if !p.has("values", preferred) {
                            p.set("values", preferred, &v)?;
                        }
                        p.remove("values", removed);
                    }
                }
                p.remove("values", "support_tree_wall_thickness");
            }
        }
        // VersionUpgrade45to46
        4_000_011 | 4_000_012 => {
            p = read(false)?;
            p.set("metadata", "setting_version", "13")?;
            if p.has_section("values") {
                p.remove("values", "machine_filament_park_distance");
                if let Some(v) = p.get("values", "meshfix_maximum_deviation").map(String::from) {
                    p.set("values", "meshfix_maximum_deviation", &format!("=({}) / 2", strip_eq(&v)))?;
                }
            }
        }
        // VersionUpgrade460to462 (upgradeExtruderInstanceContainer; the Deltacomb extruder split
        // only renames the definitions here).
        4_000_013 => {
            p = read(false)?;
            match p.get("general", "definition") {
                Some("deltacomb_extruder_0") => p.set("general", "definition", "deltacomb_base_extruder_0")?,
                Some("deltacomb_extruder_1") => p.set("general", "definition", "deltacomb_base_extruder_1")?,
                _ => {}
            }
            p.set("metadata", "setting_version", "14")?;
            if let Some(v) = p.get("values", "meshfix_maximum_deviation").map(String::from) {
                p.set("values", "meshfix_maximum_deviation", &format!("=({}) * 2", strip_eq(&v)))?;
            }
        }
        // VersionUpgrade462to47
        4_000_014 => {
            p = read(false)?;
            p.set("metadata", "setting_version", "15")?;
            if p.has_section("values") {
                if let Some(v) = p.get("values", "meshfix_maximum_deviation").map(String::from) {
                    p.set("values", "meshfix_maximum_deviation", &format!("=({}) / 2", strip_eq(&v)))?;
                }
                if let Some(v) = p.get("values", "ironing_inset").map(String::from) {
                    let correction = if p.get("values", "ironing_pattern") == Some("concentric") {
                        " + ironing_line_spacing - skin_line_width * (1.0 + ironing_flow / 100) / 2"
                    } else {
                        " + skin_line_width * (1.0 - ironing_flow / 100) / 2"
                    };
                    p.set("values", "ironing_inset", &format!("=({}){}", strip_eq(&v), correction))?;
                }
                if p.get("values", "support_tree_enable").map(parse_bool).unwrap_or(false) {
                    p.set("values", "support_structure", "tree")?;
                    p.set("values", "support_enable", "True")?;
                }
                for k in REMOVED_462_47 {
                    p.remove("values", k);
                }
            }
            if p.get("general", "definition") == Some("dagoma_discoeasy200") {
                p.set("general", "definition", "dagoma_discoeasy200_bicolor")?;
            }
        }
        // VersionUpgrade47to48 (reads meshfix_maximum_deviation, as Cura does)
        4_000_015 => {
            p = read(false)?;
            p.set("metadata", "setting_version", "16")?;
            if p.has("values", "material_shrinkage_percentage") {
                let v = p.get("values", "meshfix_maximum_deviation").ok_or("KeyError: 'meshfix_maximum_deviation'")?.to_string();
                p.set("values", "material_shrinkage_percentage", &format!("=({}) + 100", strip_eq(&v)))?;
            }
        }
        // VersionUpgrade49to410
        4_000_016 => {
            p = read(false)?;
            p.ensure_section("metadata");
            p.set("metadata", "setting_version", "17")?;
            p.ensure_section("general");
            let old_def = p.get("general", "definition").unwrap_or("").to_string();
            if let Some((_, n)) = RENAMED_PROFILES_49_410.iter().find(|(o, _)| *o == old_def) {
                p.set("general", "definition", n)?;
            }
            if p.get("metadata", "type").unwrap_or("") == "quality_changes" {
                for printer in ["twotrees_bluer", "twotrees_bluer_extruder_0", "twotrees_bluer_extruder_1"] {
                    if basename(filename).starts_with(&format!("{}_", printer)) {
                        p.set("general", "definition", "two_trees_base")?;
                        let qt = p.get("metadata", "quality_type").unwrap_or("fast").to_string();
                        p.set("metadata", "quality_type", TWO_TREES_QUALITY.iter().find(|(o, _)| *o == qt).map(|(_, n)| *n).unwrap_or("standard"))?;
                        break;
                    }
                    if basename(filename).starts_with("deltacomb_") {
                        p.set("general", "definition", "deltacomb_base")?;
                        let qt = p.get("metadata", "quality_type").unwrap_or("c").to_string();
                        p.set("metadata", "quality_type", DELTACOMB_QUALITY.iter().find(|(o, _)| *o == qt).map(|(_, n)| *n).unwrap_or("D015"))?;
                        break;
                    }
                }
            }
        }
        // VersionUpgrade411to412
        4_000_017 => {
            p = read(false)?;
            p.ensure_section("metadata");
            p.set("metadata", "setting_version", "19")?;
            if basename(filename).starts_with("flsun_sr_") && p.get("metadata", "type") == Some("quality_changes") && p.get("general", "definition") == Some("fdmprinter") {
                let old = p.get("metadata", "quality_type").unwrap_or("normal").to_string();
                p.set("general", "definition", "flsun_sr")?;
                p.set("metadata", "quality_type", FLSUN_QUALITY.iter().find(|(o, _)| *o == old).map(|(_, n)| *n).unwrap_or("normal"))?;
            }
        }
        // VersionUpgrade413to50
        4_000_019 => {
            p = read(false)?;
            p.set("metadata", "setting_version", "20")?;
            if p.has_section("values") {
                for k in REMOVED_413_50 {
                    p.remove("values", k);
                }
                if let Some(v) = p.get("values", "outer_inset_first").map(String::from) {
                    p.set("values", "inset_direction", &format!("='outside_in' if ({}) else 'inside_out'", strip_eq(&v)))?;
                }
                if let Some(v) = p.get("values", "speed_equalize_flow_enabled").map(String::from) {
                    p.set("values", "speed_equalize_flow_width_factor", &format!("=100 if ({}) else 0", strip_eq(&v)))?;
                }
            }
        }
        // VersionUpgrade52to53 / 53to54 / 56to57
        4_000_020 | 4_000_021 | 4_000_022 => {
            p = read(false)?;
            let (new, removed): (&str, &[&str]) = match v {
                4_000_020 => ("21", &["limit_support_retractions", "material_flow_dependent_temperature"]),
                4_000_021 => ("22", REMOVED_53_54),
                _ => ("23", &["support_interface_skip_height"]),
            };
            p.set("metadata", "setting_version", new)?;
            if p.has_section("values") {
                for k in removed {
                    p.remove("values", k);
                }
                if v == 4_000_022 {
                    if let Some(b) = p.get("values", "brim_outside_only").map(String::from) {
                        p.set("values", "brim_location", if b == "True" { "outside" } else { "everywhere" })?;
                        p.remove("values", "brim_outside_only");
                    }
                }
            }
        }
        // VersionUpgrade58to59: settings of the former gradual flow plug-in lose their prefix.
        4_000_023 => {
            p = read(false)?;
            p.set("metadata", "setting_version", "24")?;
            if p.has_section("values") {
                let keys: Vec<(String, String)> = p.sections.iter().find(|(n, _)| n == "values").unwrap().1.clone();
                for (k, val) in keys {
                    if k.starts_with("_plugin__curaenginegradualflow") {
                        let new = k.rsplit("__").next().unwrap_or(&k).to_string();
                        p.set("values", &new, &val)?;
                        p.remove("values", &k);
                    }
                }
            }
        }
        // VersionUpgrade59to510
        4_000_024 => {
            p = read(false)?;
            p.set("metadata", "setting_version", "25")?;
            if p.has_section("values") {
                if let Some(v) = p.get("values", "wall_overhang_speed_factor").map(String::from) {
                    p.set("values", "wall_overhang_speed_factors", &v)?;
                    p.remove("values", "wall_overhang_speed_factor");
                }
                if let Some(v) = p.get("values", "wall_overhang_speed_factors").map(String::from) {
                    let f = py_float(&v)?;
                    p.set("values", "wall_overhang_speed_factor", &format!("[{}]", py_round(f).max(1)))?;
                }
            }
        }
        // VersionUpgrade511to512
        4_000_025 => {
            p = read(false)?;
            p.set("metadata", "setting_version", "26")?;
            if p.has_section("values") {
                if let Some(v) = p.get("values", "bridge_sparse_infill_max_density").map(String::from) {
                    p.set("values", "skin_support", &format!("=infill_sparse_density < {}", v))?;
                }
                for k in ["skin_edge_support_thickness", "skin_edge_support_layers", "extra_infill_lines_to_support_skins", "bridge_sparse_infill_max_density"] {
                    p.remove("values", k);
                }
                let def = p.get("general", "definition").unwrap_or("").to_string();
                let pattern = p.get("values", "infill_pattern").unwrap_or("").to_string();
                if def == "ultimaker_factor4" && ["grid", "triangles", "cubic"].contains(&pattern.as_str()) {
                    p.set("values", "infill_pattern", "honeycomb")?;
                }
            }
        }
        _ => return Ok(None),
    }
    Ok(Some(p.write()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configparser_roundtrip() {
        let t = "[general]\nversion = 4\nName = x\n\n[values]\nmachine_start_gcode = G28\n\tG1 X0\n\n";
        let p = ConfigParser::read(t, false).unwrap();
        assert_eq!(p.get("general", "name"), Some("x"));
        assert_eq!(p.get("values", "machine_start_gcode"), Some("G28\nG1 X0"));
        assert_eq!(p.write(), "[general]\nversion = 4\nname = x\n\n[values]\nmachine_start_gcode = G28\n\tG1 X0\n\n");
    }

    #[test]
    fn upgrade_chain() {
        let t = "[general]\nversion = 4\nname = p\ndefinition = creality_base\n\n[metadata]\ntype = quality_changes\nquality_type = standard\nsetting_version = 24\n\n[values]\nwall_overhang_speed_factor = 80\nbridge_sparse_infill_max_density = 20\n\n";
        let out = upgrade_quality_changes(t, "x").unwrap();
        assert!(out.contains("setting_version = 26"));
        assert!(out.contains("skin_support = =infill_sparse_density < 20"));
        assert!(out.contains("wall_overhang_speed_factor = [80]"));
        assert!(!out.contains("bridge_sparse"));
    }
}
