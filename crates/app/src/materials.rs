//! Material management like Cura's Preferences > Materials: create, duplicate, edit, remove,
//! import and export `.xml.fdm_material` profiles (`MaterialManagementModel`,
//! `ContainerManager.importMaterialContainer` / `exportContainer`).
//!
//! User materials are ordinary `.xml.fdm_material` files in Curust's user resource folder
//! (`<config>/resources/materials`), so Cura can import them and vice versa. After a change the
//! container registry is reloaded, like Cura's registry signals update its material tree.

use crate::session::Session;
use curust_settings::registry::Source;
use curust_settings::xml::{self, Element};
use std::path::PathBuf;

const UM: &str = "http://www.ultimaker.com/material";
const CURA: &str = "http://www.ultimaker.com/cura";
const NAMESPACES: &[(&str, &str)] = &[(UM, ""), (CURA, "cura")];

/// Metadata tags Cura writes in its own namespace (`_metadata_tags_that_have_cura_namespace`).
const CURA_NS_TAGS: &[&str] = &["pva_compatible", "breakaway_compatible"];

/// The print settings the material editor shows (`MaterialSettingsVisibilityHandler`) with their
/// names in the material file.
pub const EDITABLE_SETTINGS: &[(&str, &str)] = &[
    ("default_material_print_temperature", "print temperature"),
    ("default_material_bed_temperature", "heated bed temperature"),
    ("material_standby_temperature", "standby temperature"),
    ("cool_fan_speed", "print cooling"),
    ("retraction_amount", "retraction amount"),
    ("retraction_speed", "retraction speed"),
];

/// The folder with the user's resources (materials, and printer files from the Marketplace).
pub fn user_resources_dir() -> PathBuf {
    crate::resources::config_dir().join("resources")
}

fn user_materials_dir() -> PathBuf {
    user_resources_dir().join("materials")
}

/// `ContainerInterface.getFileName` for materials: the id with spaces as "+".
fn file_name_for(id: &str) -> String {
    format!("{}.xml.fdm_material", id.replace(' ', "+"))
}

/// A random UUID (version 4), like `str(uuid.uuid4())`.
pub fn uuid4() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut bytes = [0u8; 16];
    let stack_addr = &bytes as *const _ as usize;
    for (k, chunk) in bytes.chunks_mut(8).enumerate() {
        let mut h = RandomState::new().build_hasher();
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        h.write_usize(k);
        h.write_usize(stack_addr);
        h.write_u64(COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        chunk.copy_from_slice(&h.finish().to_le_bytes());
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

/// Details of one material for the material manager.
pub struct MaterialDetails {
    pub path: PathBuf,
    pub user: bool,
    pub doc: Element,
}

impl MaterialDetails {
    fn metadata(&self) -> Option<&Element> {
        self.doc.find(UM, "metadata")
    }
    pub fn meta_text(&self, tag: &str) -> String {
        let md = self.metadata();
        let e = md.and_then(|m| m.find(UM, tag).or_else(|| m.find(CURA, tag)));
        e.and_then(|e| e.text.clone()).unwrap_or_default().trim().to_string()
    }
    pub fn property(&self, tag: &str) -> String {
        self.doc.find(UM, "properties").and_then(|p| p.find(UM, tag)).and_then(|e| e.text.clone()).unwrap_or_default().trim().to_string()
    }
}

impl Session {
    fn user_material_ids(&self) -> Vec<String> {
        let dir = user_materials_dir();
        self.reg
            .all()
            .iter()
            .filter(|e| match &e.source {
                Source::Material { path, .. } => path.starts_with(&dir) && e.get_str("base_file").as_deref() == Some(e.id.as_str()),
                _ => false,
            })
            .map(|e| e.id.clone())
            .collect()
    }

    /// The XML document of a material (by its base file id).
    pub fn material_details(&self, base_file: &str) -> Result<MaterialDetails, String> {
        let e = self.reg.metadata(base_file).ok_or_else(|| format!("Unknown material {}", base_file))?;
        let Source::Material { path, .. } = &e.source else { return Err(format!("{} is not a material", base_file)) };
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {}", path.display(), e))?;
        let doc = xml::parse(&text)?;
        Ok(MaterialDetails { path: path.clone(), user: path.starts_with(user_materials_dir()), doc })
    }

    fn write_user_material(&self, id: &str, doc: &Element) -> Result<PathBuf, String> {
        let dir = user_materials_dir();
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
        let path = dir.join(file_name_for(id));
        std::fs::write(&path, xml::write_document(doc, NAMESPACES)).map_err(|e| format!("{}: {}", path.display(), e))?;
        Ok(path)
    }

    /// Whether an extruder of any printer uses the material (`canMaterialBeRemoved`).
    pub fn material_in_use(&self, base_file: &str) -> bool {
        self.machines.iter().any(|m| m.extruders.iter().any(|e| e.container(curust_settings::stack::index::MATERIAL).meta_str("base_file").as_deref() == Some(base_file)))
    }

    /// `duplicateMaterialByBaseFile`: a copy of a material file under a new id, with metadata
    /// changes (`(tag, value)`; "name" sets the label).
    fn duplicate_material_file(&mut self, base_file: &str, new_id: &str, changes: &[(&str, &str)]) -> Result<String, String> {
        let src = self.material_details(base_file)?;
        let mut doc = src.doc;
        for (k, v) in changes {
            set_material_field(&mut doc, k, v);
        }
        self.write_user_material(new_id, &doc)?;
        crate::persist::reload(self);
        Ok(new_id.to_string())
    }

    /// `MaterialManagementModel.createMaterial`: a copy of the preferred material for the active
    /// extruder with a new GUID, named "Custom Material" by brand "Custom".
    pub fn create_material(&mut self) -> Result<String, String> {
        let m = self.require_machine()?;
        let i = *m.extruder_list().get(self.active_extruder).unwrap_or(&0);
        let node = self.tree.machine(&self.reg, &m.global.definition().id);
        let vn = m.extruders[i].container(curust_settings::stack::index::VARIANT).name();
        let variant = node.variants.get(&vn).or_else(|| node.variants.first()).ok_or("No materials for this printer")?;
        let approx = curust_settings::machine::approximate_material_diameter(m, i);
        let preferred = variant.preferred_material(&self.reg, &node, approx).base_file.clone();
        let new_id = self.unique_name("custom_material");
        let guid = uuid4();
        self.duplicate_material_file(&preferred, &new_id, &[("name", "Custom Material"), ("brand", "Custom"), ("GUID", &guid)])
    }

    /// `MaterialManagementModel.duplicateMaterial` (the copy keeps the GUID, like Cura).
    pub fn duplicate_material(&mut self, base_file: &str) -> Result<String, String> {
        let new_id = self.unique_name(base_file);
        self.duplicate_material_file(base_file, &new_id, &[])
    }

    pub fn remove_material(&mut self, base_file: &str) -> Result<(), String> {
        let d = self.material_details(base_file)?;
        if !d.user {
            return Err("Materials that come with Cura can't be removed".into());
        }
        if self.material_in_use(base_file) {
            return Err("This material is in use by a printer. Select another material first.".into());
        }
        std::fs::remove_file(&d.path).map_err(|e| format!("{}: {}", d.path.display(), e))?;
        crate::persist::reload(self);
        Ok(())
    }

    /// Change a field of a user material: name, brand, material, color_name, color_code,
    /// description, adhesion_info, density, diameter, or a print setting (by setting key).
    pub fn update_material(&mut self, base_file: &str, field: &str, value: &str) -> Result<(), String> {
        let mut d = self.material_details(base_file)?;
        if !d.user {
            return Err("Materials that come with Cura are read-only. Duplicate the material to change it.".into());
        }
        if let Some((_, xml_key)) = EDITABLE_SETTINGS.iter().find(|(k, _)| *k == field) {
            let v: f64 = value.trim().replace(',', ".").parse().map_err(|_| format!("{} is not a number", value))?;
            // Like Cura, the value goes into the material container the active extruder uses:
            // the machine / nozzle specific part of the file, or the general settings.
            let (definition, variant) = self.active_material_container(base_file).unwrap_or(("fdmprinter".into(), None));
            let reg = self.reg.clone();
            let target = settings_target(&mut d.doc, &reg, &definition, variant.as_deref());
            set_setting(target, xml_key, &curust_settings::py::float_repr(v));
        } else {
            set_material_field(&mut d.doc, field, value);
        }
        std::fs::write(&d.path, xml::write_document(&d.doc, NAMESPACES)).map_err(|e| format!("{}: {}", d.path.display(), e))?;
        crate::persist::reload(self);
        Ok(())
    }

    /// The (definition, variant name) of the container of `base_file` that the active extruder
    /// would use.
    fn active_material_container(&self, base_file: &str) -> Option<(String, Option<String>)> {
        let m = self.machine()?;
        let i = *m.extruder_list().get(self.active_extruder)?;
        let node = self.tree.machine(&self.reg, &m.global.definition().id);
        let vn = m.extruders[i].container(curust_settings::stack::index::VARIANT).name();
        let variant = node.variants.get(&vn).or_else(|| node.variants.first())?;
        let mn = variant.materials.get(base_file)?;
        let e = self.reg.metadata(&mn.container_id)?;
        Some((e.get_str("definition").unwrap_or_else(|| "fdmprinter".into()), e.get_str("variant_name")))
    }

    /// `ContainerManager.importMaterialContainer`: the file is added under a unique id.
    pub fn import_material(&mut self, file_name: &str, data: &[u8]) -> Result<String, String> {
        let base = file_name.rsplit(['/', '\\']).next().unwrap_or(file_name);
        let stem = curust_settings::registry::unquote_plus(base.strip_suffix(".xml.fdm_material").or_else(|| base.strip_suffix(".fdm_material")).ok_or("Not a material file (.xml.fdm_material)")?);
        let text = String::from_utf8_lossy(data);
        let doc = xml::parse(&text).map_err(|_| "The material file appears to be corrupt.".to_string())?;
        if doc.local_name() != "fdmmaterial" {
            return Err("The material file appears to be corrupt.".into());
        }
        let id = self.unique_name(&stem);
        let dir = user_materials_dir();
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
        let path = dir.join(file_name_for(&id));
        std::fs::write(&path, data).map_err(|e| format!("{}: {}", path.display(), e))?;
        crate::persist::reload(self);
        if self.reg.metadata(&id).is_none() {
            let _ = std::fs::remove_file(&path);
            crate::persist::reload(self);
            return Err("The material file appears to be corrupt.".into());
        }
        Ok(id)
    }

    /// The material file (`ContainerManager.exportContainer`).
    pub fn export_material(&self, base_file: &str) -> Result<(String, Vec<u8>), String> {
        let d = self.material_details(base_file)?;
        let data = std::fs::read(&d.path).map_err(|e| format!("{}: {}", d.path.display(), e))?;
        Ok((file_name_for(base_file), data))
    }

    /// All user material ids.
    pub fn user_materials(&self) -> Vec<String> {
        self.user_material_ids()
    }
}

/// Set a metadata field in a material document the way `XmlMaterialProfile.serialize` writes
/// it: name parts in `<name>`, density / diameter in `<properties>`, the rest in `<metadata>`.
fn set_material_field(doc: &mut Element, field: &str, value: &str) {
    match field {
        "name" | "label" => {
            doc.child_mut(UM, "metadata").child_mut(UM, "name").child_mut(UM, "label").text = Some(value.to_string());
        }
        "brand" | "material" => {
            doc.child_mut(UM, "metadata").child_mut(UM, "name").child_mut(UM, field).text = Some(value.to_string());
        }
        "color_name" => {
            doc.child_mut(UM, "metadata").child_mut(UM, "name").child_mut(UM, "color").text = Some(value.to_string());
        }
        "density" | "diameter" | "weight" => {
            let v: f64 = value.trim().replace(',', ".").parse().unwrap_or(0.0);
            doc.child_mut(UM, "properties").child_mut(UM, field).text = Some(curust_settings::py::float_repr(v));
        }
        _ => {
            let ns = if CURA_NS_TAGS.contains(&field) { CURA } else { UM };
            let md = doc.child_mut(UM, "metadata");
            md.child_mut(ns, field).text = Some(value.to_string());
        }
    }
}

/// The element whose `<setting>`s belong to the material container for `definition` /
/// `variant`: `<settings>`, a `<machine>` in it, or a `<hotend>` of that machine.
fn settings_target<'a>(doc: &'a mut Element, reg: &curust_settings::registry::Registry, definition: &str, variant: Option<&str>) -> &'a mut Element {
    let settings = doc.child_mut(UM, "settings");
    if definition == "fdmprinter" {
        return settings;
    }
    let machine_idx = settings.children.iter().position(|c| {
        c.local_name() == "machine"
            && c.find_all(UM, "machine_identifier").any(|mi| mi.attr("product").map(|p| curust_settings::material::machine_ids_for(p, reg).iter().any(|d| d == definition)).unwrap_or(false))
    });
    let Some(mut mi) = machine_idx else { return settings };
    // One <machine> element can list several printers; Cura keeps a container per printer and
    // writes them separately, so an edit for one printer splits it off first.
    let matches = |e: &Element| e.local_name() == "machine_identifier" && e.attr("product").map(|p| curust_settings::material::machine_ids_for(p, reg).iter().any(|d| d == definition)).unwrap_or(false);
    let identifiers = settings.children[mi].children.iter().filter(|c| c.local_name() == "machine_identifier").count();
    if identifiers > 1 {
        let mut own = settings.children[mi].clone();
        own.children.retain(|c| c.local_name() != "machine_identifier" || matches(c));
        settings.children[mi].children.retain(|c| !matches(c));
        settings.children.insert(mi + 1, own);
        mi += 1;
    }
    let machine = &mut settings.children[mi];
    if let Some(v) = variant {
        if let Some(hi) = machine.children.iter().position(|c| c.local_name() == "hotend" && c.attr("id") == Some(v)) {
            return &mut machine.children[hi];
        }
    }
    machine
}

/// Set a `<setting key="...">` of a settings element (before any `<machine>` / `<hotend>`
/// child).
fn set_setting(settings: &mut Element, key: &str, value: &str) {
    if let Some(s) = settings.children.iter_mut().find(|c| c.local_name() == "setting" && c.attr("key") == Some(key)) {
        s.text = Some(value.to_string());
        return;
    }
    let mut e = Element::new(&format!("{{{}}}setting", UM));
    e.set_attr("key", key);
    e.text = Some(value.to_string());
    let at = settings.children.iter().position(|c| c.local_name() != "setting").unwrap_or(settings.children.len());
    settings.children.insert(at, e);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_format() {
        let u = uuid4();
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
        assert_ne!(uuid4(), u);
    }

    #[test]
    fn edit_roundtrip() {
        let text = r#"<?xml version="1.0" encoding="UTF-8"?>
<fdmmaterial xmlns="http://www.ultimaker.com/material" xmlns:cura="http://www.ultimaker.com/cura" version="1.3">
    <metadata>
        <name>
            <brand>Generic</brand>
            <material>PLA</material>
            <color>Generic</color>
        </name>
        <GUID>506c9f0d-e3aa-4bd4-b2d2-23e2425b1aa9</GUID>
        <version>1</version>
        <color_code>#ffc924</color_code>
    </metadata>
    <properties>
        <density>1.24</density>
        <diameter>2.85</diameter>
    </properties>
    <settings>
        <setting key="print temperature">200</setting>
        <machine>
            <machine_identifier manufacturer="Ultimaker B.V." product="Ultimaker S5"/>
        </machine>
    </settings>
</fdmmaterial>"#;
        let mut doc = xml::parse(text).unwrap();
        set_material_field(&mut doc, "name", "My PLA");
        set_material_field(&mut doc, "brand", "Me & Co");
        set_material_field(&mut doc, "density", "1,3");
        set_setting(doc.child_mut(UM, "settings"), "retraction amount", "6.5");
        let out = xml::write_document(&doc, NAMESPACES);
        assert!(out.starts_with("<?xml version='1.0' encoding='utf-8'?>\n<fdmmaterial xmlns=\"http://www.ultimaker.com/material\" xmlns:cura=\"http://www.ultimaker.com/cura\" version=\"1.3\">\n  <metadata>\n    <name>\n      <brand>Me &amp; Co</brand>"));
        assert!(out.contains("<label>My PLA</label>"));
        assert!(out.contains("<density>1.3</density>"));
        assert!(out.contains("<setting key=\"retraction amount\">6.5</setting>\n    <machine>"));
        assert!(out.contains("<machine_identifier manufacturer=\"Ultimaker B.V.\" product=\"Ultimaker S5\" />"));
        let back = xml::parse(&out).unwrap();
        assert_eq!(back.find(UM, "properties").unwrap().find(UM, "diameter").unwrap().text.as_deref(), Some("2.85"));
    }
}
