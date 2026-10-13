//! Profile import and export, ported from Cura's `CuraProfileReader`, `CuraProfileWriter`,
//! `GCodeProfileReader` and `CuraContainerRegistry.importProfile` / `exportQualityProfile`.
//!
//! A custom profile is a group of quality_changes containers (one global, one per extruder).
//! `.curaprofile` files are ZIP archives with one serialized container per entry; G-code files
//! carry the profile in their `;SETTING_3` footer.

use crate::session::{CustomProfile, Session};
use crate::zip;
use curust_settings::definition::Prop;
use curust_settings::instance::{InstValue, InstanceContainer};
use curust_settings::json::Json;
use curust_settings::machine;
use curust_settings::py::Value;
use curust_settings::registry::Registry;
use curust_settings::stack::StackId;
use curust_settings::upgrade;
use std::rc::Rc;

/// The outcome of an import, as Cura reports it (`status` is "ok", "warning" or "error").
#[derive(Debug, Clone)]
pub struct ImportResult {
    pub status: &'static str,
    pub message: String,
    pub profile: Option<usize>,
}

impl ImportResult {
    fn error(message: String) -> ImportResult {
        ImportResult { status: "error", message, profile: None }
    }
}

/// `InstanceContainer.serialize` (Uranium): `[general]`, the metadata in insertion order (without
/// id / version / name / container_type / definition) and the values sorted by key, each
/// converted with its setting type and written back with `str()`, through `configparser`.
pub fn cura_serialize(reg: &Registry, c: &InstanceContainer) -> String {
    let def = reg.definition(&c.definition);
    let mut values: Vec<(String, String)> = Vec::new();
    for k in c.keys() {
        let v = match c.get_value(&k, def.as_deref()) {
            Some(Prop::Value(v)) => v.py_str(),
            Some(Prop::Func(f)) => f.to_string(),
            None => continue,
        };
        values.push((k.to_lowercase(), v));
    }
    values.sort();
    let metadata: Vec<(String, String)> = c
        .metadata
        .iter()
        .filter(|(k, _)| !matches!(k.as_str(), "id" | "version" | "name" | "container_type" | "definition"))
        .map(|(k, v)| (k.to_lowercase(), v.py_str()))
        .collect();
    let p = upgrade::ConfigParser {
        sections: vec![
            ("general".into(), vec![("version".into(), "4".into()), ("name".into(), c.name.clone()), ("definition".into(), c.definition.clone())]),
            ("metadata".into(), metadata),
            ("values".into(), values),
        ],
    };
    p.write()
}

/// `InstanceContainer.deserialize` with the version upgrade Uranium applies first
/// (`_updateSerialized`); when the upgrade fails the original text is used, like Cura.
fn deserialize(id: &str, text: &str, file_name: &str) -> Result<InstanceContainer, String> {
    let upgraded = upgrade::upgrade_quality_changes(text, file_name).unwrap_or_else(|_| text.to_string());
    let version = curust_settings::instance::parse_cfg(&upgraded)
        .iter()
        .find(|(s, _)| s == "general")
        .and_then(|(_, kv)| kv.iter().find(|(k, _)| k == "version").map(|(_, v)| v.clone()))
        .ok_or("Missing required 'version' property")?;
    if version.trim() != "4" {
        return Err(format!("Reported version {} but expected version 4", version.trim()));
    }
    InstanceContainer::from_cfg(id, &upgraded)
}

/// What a profile reader produced.
enum ReadResult {
    Profiles(Vec<InstanceContainer>),
    /// `NoProfileException`.
    NoProfile,
}

/// `CuraProfileReader.read` + `_upgradeProfile` + `_loadProfile`.
fn read_curaprofile(file_name: &str, data: &[u8], quality_definition: &str) -> Result<ReadResult, String> {
    let entries: Vec<(String, String)> = match zip::read_zip(data) {
        Ok(entries) => entries.into_iter().map(|e| (e.name, String::from_utf8_lossy(&e.data).into_owned())).collect(),
        // Not a ZIP archive: an older profile from Cura 2.1, a single serialized container.
        Err(_) => vec![(file_name.to_string(), String::from_utf8_lossy(data).into_owned())],
    };
    let mut out = Vec::new();
    for (profile_id, serialized) in entries {
        let parser = upgrade::ConfigParser::read(&serialized, true)?;
        if !parser.has_section("general") || !parser.has("general", "version") {
            continue;
        }
        let version: i64 = parser.get("general", "version").unwrap_or("").trim().parse().map_err(|_| "invalid literal for int()".to_string())?;
        if !parser.has_section("metadata") {
            return Err("'metadata'".into());
        }
        if version != 4 {
            // Profiles from Cura 2.x / 3.x would need the old upgrade chain.
            continue;
        }
        let Ok(mut profile) = deserialize(&profile_id, &serialized, &profile_id) else { continue };
        if profile.definition != quality_definition {
            profile.definition = quality_definition.to_string();
        }
        out.push(profile);
    }
    Ok(ReadResult::Profiles(out))
}

/// `unescapeGcodeComment`.
fn unescape_gcode_comment(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < c.len() {
        if c[i] == '\\' && i + 1 < c.len() {
            match c[i + 1] {
                '\\' => {
                    out.push('\\');
                    i += 2;
                    continue;
                }
                'n' => {
                    out.push('\n');
                    i += 2;
                    continue;
                }
                'r' => {
                    out.push('\r');
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

/// `GCodeProfileReader.read`.
fn read_gcode_profile(data: &[u8]) -> Result<ReadResult, String> {
    let text = String::from_utf8_lossy(data).replace("\r\n", "\n").replace('\r', "\n");
    let prefix = ";SETTING_3 ";
    let mut serialized = String::new();
    for line in text.split_inclusive('\n') {
        if let Some(rest) = line.strip_prefix(prefix) {
            // `line[prefix_length:-1]` drops the newline (or the last character of a last line
            // without one).
            let mut rest: Vec<char> = rest.chars().collect();
            rest.pop();
            serialized.extend(rest);
        }
    }
    let serialized = unescape_gcode_comment(&serialized);
    let serialized = serialized.trim();
    if serialized.is_empty() {
        return Ok(ReadResult::NoProfile);
    }
    let Ok(json) = curust_settings::json::parse(serialized) else { return Ok(ReadResult::Profiles(Vec::new())) };
    let global = json.get("global_quality").and_then(|v| v.as_str()).ok_or("'global_quality'")?;
    let mut out = Vec::new();
    let mut g = deserialize("", global, "").map_err(|_| "'NoneType' object has no attribute 'getMetaDataEntry'".to_string())?;
    // Profiles from Cura 2.3.0 had an "extruder" entry in the global profile.
    if g.meta("extruder").is_some() {
        g.set_meta("extruder", Value::None);
    }
    out.push(g);
    if let Some(Json::Array(list)) = json.get("extruder_quality") {
        for e in list {
            if let Some(s) = e.as_str() {
                out.push(deserialize("", s, "")?);
            }
        }
    }
    Ok(ReadResult::Profiles(out))
}

impl Session {
    /// `ContainerRegistry.uniqueName` over the registry and the custom profiles.
    pub fn unique_name(&self, original: &str) -> String {
        let taken = |n: &str| self.profiles.iter().any(|p| p.containers().any(|c| c.id.eq_ignore_ascii_case(n) || c.name == n));
        self.reg.unique_name_where(original, &taken)
    }

    /// `CuraContainerRegistry.createUniqueName("quality_changes", "", seed, "Custom profile")`.
    fn create_unique_qc_name(&self, seed: &str, also_taken: &dyn Fn(&str) -> bool) -> String {
        let mut name = seed.trim().to_string();
        if let Some(p) = name.rfind('#') {
            if name[p + 1..].chars().all(|c| c.is_ascii_digit()) && p + 1 < name.len() {
                name = name[..p].trim_end().to_string();
            }
        }
        if name.is_empty() {
            name = "Custom profile".into();
        }
        let exists = |n: &str| also_taken(n) || self.profiles.iter().any(|p| p.containers().any(|c| c.id.eq_ignore_ascii_case(n) || c.name == n));
        let mut unique = name.clone();
        let mut i = 1;
        while exists(&unique) {
            i += 1;
            unique = format!("{} #{}", name, i);
        }
        unique
    }

    /// `CuraContainerRegistry.importProfile`: read a `.curaprofile` or G-code file and add its
    /// profile to the active printer's custom profiles.
    pub fn import_profile(&mut self, file_name: &str, data: &[u8]) -> ImportResult {
        let base = file_name.rsplit(['/', '\\']).next().unwrap_or(file_name).to_string();
        if file_name.is_empty() {
            return ImportResult::error(format!("Failed to import profile from {}: Invalid path", file_name));
        }
        let Some(m) = self.machine() else {
            return ImportResult::error(format!("Can't import profile from {} before a printer is added.", base));
        };
        let expected_definition = self.quality_definition().unwrap_or_default();
        let extension = file_name.rsplit('.').next().unwrap_or("").to_string();
        let read = match extension.as_str() {
            "curaprofile" => read_curaprofile(file_name, data, &expected_definition),
            "gcode" => read_gcode_profile(data),
            _ => return ImportResult::error(format!("Profile {} has an unknown file type or is corrupted.", base)),
        };
        let mut list = match read {
            Ok(ReadResult::NoProfile) => return ImportResult { status: "ok", message: format!("No custom profile to import in file {}", base), profile: None },
            Ok(ReadResult::Profiles(p)) => p,
            Err(e) => return ImportResult::error(format!("Failed to import profile from {}:\n{}", base, e)),
        };
        if list.is_empty() {
            return ImportResult::error(format!("File {} does not contain any valid profile.", base));
        }
        // The global profile first, then the extruder profiles by position.
        let mut global = None;
        let mut extruder_profiles = Vec::new();
        if list.len() == 1 {
            global = list.pop();
        } else {
            for p in list.drain(..) {
                if p.meta_str("position").map(|s| !s.is_empty()).unwrap_or(false) {
                    extruder_profiles.push(p);
                } else {
                    global = Some(p);
                }
            }
        }
        extruder_profiles.sort_by_key(|p| p.meta_str("position").and_then(|s| s.parse::<i64>().ok()).unwrap_or(0));
        let Some(mut global) = global else {
            return ImportResult::error(format!("This profile {} contains incorrect data, could not import it.", base));
        };
        let profile_definition = global.definition.clone();
        let Some(machine_def) = self.reg.metadata(&profile_definition).filter(|e| matches!(e.source, curust_settings::registry::Source::Definition(_))) else {
            return ImportResult::error(format!("This profile {} contains incorrect data, could not import it.", base));
        };
        let has_machine_quality = machine_def.get("has_machine_quality").map(|v| curust_settings::stack::parse_bool(v)).unwrap_or(false);
        let profile_definition = if has_machine_quality { machine_def.get_str("quality_definition").unwrap_or_else(|| profile_definition.clone()) } else { "fdmprinter".into() };
        if profile_definition != expected_definition {
            global.definition = expected_definition.clone();
            for e in extruder_profiles.iter_mut() {
                e.definition = expected_definition.clone();
            }
        }
        let quality_name = global.name.clone();
        let quality_type = global.meta_str("quality_type");
        let name_seed = match base.rfind('.') {
            Some(p) if p > 0 => base[..p].to_string(),
            _ => base.clone(),
        };
        let new_name = self.unique_name(&name_seed);

        // A single profile: make extruder profiles and move the per-extruder settings into the
        // first one.
        let extruder_list = m.extruder_list();
        if extruder_profiles.is_empty() {
            let global_def = self.reg.definition(&global.definition);
            for (idx, _) in extruder_list.iter().enumerate() {
                let mut p = InstanceContainer::new(&format!("{}_extruder_{}", m.global.id, idx + 1), &quality_name, &expected_definition);
                p.set_meta("setting_version", Value::str(crate::session::CURA_SETTING_VERSION));
                p.set_meta("type", Value::str("quality_changes"));
                p.set_meta("quality_type", quality_type.as_deref().map(Value::str).unwrap_or(Value::None));
                if idx == 0 {
                    for key in global.keys() {
                        let per_extruder = m.get_property(StackId::Global, &key, "settable_per_extruder", None).map(|v| v.truthy()).unwrap_or(false);
                        if per_extruder {
                            if let Some(v) = global.get_value(&key, global_def.as_deref()) {
                                if m.reg.definition(&m.global.definition().id).and_then(|d| d.find(&key).map(|_| ())).is_some() {
                                    p.set_value(&key, match v {
                                        Prop::Value(v) => InstValue::Typed(v),
                                        Prop::Func(f) => InstValue::Func(f),
                                    });
                                }
                            }
                            global.remove_value(&key);
                        }
                    }
                }
                extruder_profiles.push(p);
            }
        }

        // Configure and add all profiles.
        let mut configured: Vec<InstanceContainer> = Vec::new();
        let mut additional_message = None;
        let mut all = vec![global];
        all.extend(extruder_profiles);
        let groups = machine::current_quality_groups(m, &self.tree);
        for (profile_index, mut profile) in all.into_iter().enumerate() {
            let id_seed = if profile_index == 0 {
                format!("{}_{}", m.global.definition().id, name_seed).to_lowercase().replace(' ', "_")
            } else if profile_index < extruder_list.len() + 1 {
                let e = &m.extruders[extruder_list[profile_index - 1]];
                profile.set_meta("position", Value::str(&(profile_index - 1).to_string()));
                format!("{}_{}", e.definition().id, name_seed).to_lowercase().replace(' ', "_")
            } else {
                continue;
            };
            // `_configureProfile`.
            let new_id = self.create_unique_qc_name(&id_seed, &|n: &str| configured.iter().any(|c| c.id.eq_ignore_ascii_case(n) || c.name == n));
            profile.id = new_id;
            profile.name = new_name.clone();
            profile.definition = expected_definition.clone();
            profile.set_meta("type", Value::str("quality_changes"));
            let qt = profile.meta_str("quality_type").filter(|q| !q.is_empty() && q != "None");
            let Some(qt) = qt else {
                return ImportResult::error(format!("Failed to import profile from {}: Profile is missing a quality type.", base));
            };
            if qt != "not_supported" {
                match groups.get(&qt) {
                    None => {
                        return ImportResult::error(format!(
                            "Failed to import profile from {}: Quality type '{}' is not compatible with the current active machine definition '{}'.",
                            base, qt, expected_definition
                        ))
                    }
                    Some(g) if !g.is_available => {
                        additional_message = Some(format!(
                            "\n\nWarning: The profile is not visible because its quality type '{}' is not available for the current configuration. Switch to a material/nozzle combination that can use this quality type.",
                            qt
                        ));
                    }
                    _ => {}
                }
            }
            configured.push(profile);
        }
        let mut it = configured.into_iter();
        let global = it.next().expect("global profile");
        let extruders: Vec<(String, Rc<InstanceContainer>)> = it.map(|c| (c.meta_str("position").unwrap_or_default(), Rc::new(c))).collect();
        let profile = CustomProfile::from_containers(Rc::new(global), extruders);
        let name = profile.name.clone();
        self.profiles.push(profile);
        let idx = self.profiles.len() - 1;
        let mut message = format!("Successfully imported profile {}.", name);
        let status = match additional_message {
            Some(m) => {
                message.push_str(&m);
                "warning"
            }
            None => "ok",
        };
        ImportResult { status, message, profile: Some(idx) }
    }

    /// `ContainerManager.exportQualityChangesGroup` + `CuraProfileWriter.write`: the profile's
    /// containers (global first) in a `.curaprofile` archive.
    pub fn export_profile(&self, idx: usize) -> Result<(String, Vec<u8>), String> {
        let p = self.profiles.get(idx).ok_or("Unknown profile")?;
        let entries: Vec<(String, Vec<u8>)> = p.containers().map(|c| (c.id.clone(), cura_serialize(&self.reg, c).into_bytes())).collect();
        Ok((format!("{}.curaprofile", p.name), zip::write_zip_python_stored(&entries)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescape() {
        assert_eq!(unescape_gcode_comment("a\\\\nb\\nc\\rd"), "a\\nb\nc\rd");
    }
}
