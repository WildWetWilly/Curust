//! Locating Cura's resource files (machine definitions, materials, profiles) and Curust's own
//! configuration directory.

use std::path::{Path, PathBuf};

fn is_resource_dir(p: &Path) -> bool {
    p.join("definitions").join("fdmprinter.def.json").is_file()
}

/// The directory where Curust stores its configuration (printers, preferences).
pub fn config_dir() -> PathBuf {
    if let Ok(p) = std::env::var("CURUST_CONFIG_DIR") {
        return PathBuf::from(p);
    }
    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata).join("Curust");
        }
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join("curust");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".config").join("curust");
    }
    PathBuf::from(".curust")
}

/// Installed Cura versions (newest first) that carry a resource directory.
fn installed_cura_resources() -> Vec<PathBuf> {
    let mut found: Vec<(Vec<u64>, PathBuf)> = Vec::new();
    let mut bases: Vec<PathBuf> = Vec::new();
    #[cfg(windows)]
    {
        for var in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)", "LOCALAPPDATA"] {
            if let Ok(p) = std::env::var(var) {
                bases.push(PathBuf::from(&p));
                bases.push(PathBuf::from(&p).join("Programs"));
            }
        }
        bases.push(PathBuf::from("C:\\Program Files"));
    }
    #[cfg(not(windows))]
    {
        bases.push(PathBuf::from("/opt"));
        bases.push(PathBuf::from("/usr/share"));
        if let Ok(home) = std::env::var("HOME") {
            bases.push(PathBuf::from(home).join("Applications"));
        }
    }
    for base in bases {
        let Ok(rd) = std::fs::read_dir(&base) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            if !(name.contains("cura")) {
                continue;
            }
            let res = e.path().join("share").join("cura").join("resources");
            let res2 = e.path().join("resources");
            let version: Vec<u64> = name.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).filter_map(|s| s.parse().ok()).collect();
            if is_resource_dir(&res) {
                found.push((version, res));
            } else if is_resource_dir(&res2) {
                found.push((version, res2));
            }
        }
    }
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().map(|(_, p)| p).collect()
}

/// Cura's resources compiled into the executable (the portable build sets
/// `CURUST_EMBED_RESOURCES` to a `curust-resources.zip` when building); empty otherwise.
static EMBEDDED: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/embedded_resources.zip"));

/// Identifies a resources archive: its size and FNV-1a hash (the same archive next to the
/// executable or built into it is only extracted once).
fn archive_stamp(data: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in data {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    format!("{}-{:016x}", data.len(), h)
}

/// The extracted resources, when they come from the archive `data`; otherwise extract it.
fn extracted_from(data: &[u8]) -> Option<PathBuf> {
    let stamp = archive_stamp(data);
    let target = config_dir().join("resources");
    if is_resource_dir(&target) && std::fs::read_to_string(target.join(".bundle-size")).ok().as_deref() == Some(stamp.as_str()) {
        return Some(target);
    }
    extract_zip(data, &stamp)
}

/// Extract `curust-resources.zip` (shipped next to the executable) into the configuration
/// directory, once per archive.
fn extract_bundle(zip_path: &Path) -> Option<PathBuf> {
    extracted_from(&std::fs::read(zip_path).ok()?)
}

/// Unpack a resources archive into the configuration directory and record `stamp`.
fn extract_zip(data: &[u8], stamp: &str) -> Option<PathBuf> {
    let target = config_dir().join("resources");
    let entries = crate::zip::read_zip(data).ok()?;
    for e in entries {
        if e.name.ends_with('/') || e.name.contains("..") {
            continue;
        }
        let rel = e.name.trim_start_matches("resources/");
        let p = target.join(rel);
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&p, &e.data).ok()?;
    }
    let _ = std::fs::write(target.join(".bundle-size"), stamp);
    if is_resource_dir(&target) {
        Some(target)
    } else {
        None
    }
}

/// The embedded resources, extracted on first use (and again when a build embeds other ones).
fn embedded_resources() -> Option<PathBuf> {
    if EMBEDDED.is_empty() {
        return None;
    }
    extracted_from(EMBEDDED)
}

/// Find the resource directory: `--resources`/`CURUST_RESOURCES`, `resources/` next to the
/// executable, the bundled archive, the resources built into the executable, or an installed
/// copy of UltiMaker Cura.
pub fn find_resources(explicit: Option<&str>) -> Result<PathBuf, String> {
    if let Some(p) = explicit {
        let p = PathBuf::from(p);
        if is_resource_dir(&p) {
            return Ok(p);
        }
        if is_resource_dir(&p.join("share").join("cura").join("resources")) {
            return Ok(p.join("share").join("cura").join("resources"));
        }
        return Err(format!("{} is not a Cura resources directory", p.display()));
    }
    if let Ok(p) = std::env::var("CURUST_RESOURCES") {
        return find_resources(Some(&p));
    }
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(|p| p.to_path_buf());
        // Next to the executable, or in the repository when running from target/release.
        for _ in 0..4 {
            let Some(d) = dir.clone() else { break };
            if is_resource_dir(&d.join("resources")) {
                return Ok(d.join("resources"));
            }
            let z = d.join("curust-resources.zip");
            if z.is_file() {
                if let Some(p) = extract_bundle(&z) {
                    return Ok(p);
                }
            }
            dir = d.parent().map(|p| p.to_path_buf());
        }
    }
    if let Some(p) = embedded_resources() {
        return Ok(p);
    }
    let extracted = config_dir().join("resources");
    if is_resource_dir(&extracted) {
        return Ok(extracted);
    }
    if let Some(p) = installed_cura_resources().into_iter().next() {
        return Ok(p);
    }
    for p in ["/usr/share/cura/resources", "/usr/local/share/cura/resources"] {
        if is_resource_dir(Path::new(p)) {
            return Ok(PathBuf::from(p));
        }
    }
    Err("Could not find Cura's resource files. Install UltiMaker Cura 5.x, put curust-resources.zip next to curust.exe, or pass --resources <dir>.".into())
}
