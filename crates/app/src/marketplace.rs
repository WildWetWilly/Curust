//! The Cura Marketplace: browse and search UltiMaker's package API (the same server and SDK
//! version Cura 5.12 uses), download `.curapackage` files and install them the way Cura's
//! `PackageManager` does (`files/materials`, `files/qualities`, `files/variants`,
//! `files/intent(s)` into the user resources, `files/plugins` into the plugins folder).
//!
//! Curust uses everything in a package that is data: materials, quality / intent / variant
//! profiles, printer definitions shipped inside plugins, and post-processing scripts. Plugins
//! whose features are Python code for Cura's Qt interface install but have no effect.

use crate::materials::user_resources_dir;
use crate::resources::config_dir;
use curust_settings::json::{self, Json};
use std::path::{Path, PathBuf};

/// Cura 5.12's `CuraSDKVersion` and `CuraCloudAPIVersion`.
pub const SDK_VERSION: &str = "8.12.0";
pub const API_ROOT: &str = "https://api.ultimaker.com/cura-packages/v1";

pub fn packages_url() -> String {
    std::env::var("CURUST_MARKETPLACE_URL").unwrap_or_else(|_| format!("{}/cura/v{}/packages", API_ROOT, SDK_VERSION))
}

/// HTTP GET through the system's `curl` (Windows 10+, macOS and Linux ship it), so proxies
/// and certificates work like in a browser.
pub fn http_get(url: &str) -> Result<Vec<u8>, String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("only http(s) URLs can be downloaded".into());
    }
    let mut cmd = std::process::Command::new(if cfg!(windows) { "curl.exe" } else { "curl" });
    cmd.args(["-sSL", "--fail", "--max-time", "120", "-A", "Cura/5.12.0 (Curust)", "-H", "Accept: application/json, */*", url]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().map_err(|e| format!("Could not run curl to reach the Marketplace ({}). Install curl or download the package in a browser and install the file.", e))?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if msg.is_empty() { "Could not reach the Marketplace.".into() } else { format!("Could not reach the Marketplace: {}", msg) });
    }
    Ok(out.stdout)
}

fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// `RemotePackageList._initialRequestUrl`.
pub fn search_url(package_type: &str, search: &str) -> String {
    let mut url = format!("{}?limit=20", packages_url());
    if !package_type.is_empty() {
        url.push_str(&format!("&package_type={}", enc(package_type)));
    }
    if !search.is_empty() {
        url.push_str(&format!("&search={}", enc(search)));
    }
    url.push_str("&sort_by=last_updated");
    url
}

// ---------------------------------------------------------------------- installed packages

/// Where Curust keeps installed plugin packages.
pub fn plugins_dir() -> PathBuf {
    config_dir().join("plugins")
}

fn index_path() -> PathBuf {
    config_dir().join("packages.json")
}

/// Installed packages: package_id → package.json (plus Curust's notes).
pub fn installed() -> Vec<(String, Json)> {
    let Ok(text) = std::fs::read_to_string(index_path()) else { return Vec::new() };
    match json::parse(&text) {
        Ok(Json::Object(items)) => items,
        _ => Vec::new(),
    }
}

fn save_index(items: &[(String, Json)]) {
    let _ = std::fs::create_dir_all(config_dir());
    let _ = std::fs::write(index_path(), Json::Object(items.to_vec()).to_string_pretty());
}

/// The newest UltiMaker Cura user data folder (packages and custom materials the user
/// installed in Cura).
pub fn cura_data_dirs() -> Vec<PathBuf> {
    if std::env::var_os("CURUST_NO_CURA_USER_DIR").is_some() {
        return Vec::new();
    }
    let mut bases: Vec<PathBuf> = Vec::new();
    #[cfg(windows)]
    {
        if let Ok(a) = std::env::var("APPDATA") {
            bases.push(PathBuf::from(a).join("cura"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(h) = std::env::var("HOME") {
            bases.push(PathBuf::from(h).join("Library").join("Application Support").join("cura"));
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Ok(h) = std::env::var("HOME") {
            bases.push(PathBuf::from(&h).join(".local").join("share").join("cura"));
        }
    }
    let mut out = Vec::new();
    for base in bases {
        let Ok(rd) = std::fs::read_dir(&base) else { continue };
        let mut versions: Vec<(Vec<u64>, PathBuf)> = rd
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| {
                let n = e.file_name().to_string_lossy().to_string();
                let v: Vec<u64> = n.split('.').map(|x| x.parse().ok()).collect::<Option<Vec<u64>>>()?;
                (v.first().copied().unwrap_or(0) >= 5).then_some((v, e.path()))
            })
            .collect();
        versions.sort();
        if let Some((_, p)) = versions.pop() {
            out.push(p);
        }
    }
    out
}

fn has_resource_dirs(p: &Path) -> bool {
    ["definitions", "extruders", "materials", "quality", "variants", "intent"].iter().any(|d| p.join(d).is_dir())
}

/// Folders inside installed plugins (Curust's and Cura's) that hold Cura resources.
pub fn plugin_resource_roots() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut plugin_dirs = vec![plugins_dir()];
    for d in cura_data_dirs() {
        plugin_dirs.push(d.join("plugins"));
    }
    for pd in plugin_dirs {
        let mut stack = vec![(pd, 0)];
        while let Some((dir, depth)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if !p.is_dir() {
                    continue;
                }
                if has_resource_dirs(&p) {
                    out.push(p.clone());
                } else if depth < 4 {
                    stack.push((p, depth + 1));
                }
            }
        }
    }
    out.sort();
    out
}

/// `scripts` folders inside installed plugins that contain post-processing scripts
/// (Curust's packages, and with `include_cura` the plugins installed in UltiMaker Cura).
pub fn plugin_script_dirs(include_cura: bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut plugin_dirs = vec![plugins_dir()];
    if include_cura {
        for d in cura_data_dirs() {
            plugin_dirs.push(d.join("plugins"));
        }
    }
    for pd in plugin_dirs {
        let mut stack = vec![(pd, 0)];
        while let Some((dir, depth)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if !p.is_dir() {
                    continue;
                }
                if p.file_name().is_some_and(|n| n == "scripts") && is_script_dir(&p) {
                    out.push(p.clone());
                } else if depth < 5 {
                    stack.push((p, depth + 1));
                }
            }
        }
    }
    out.sort();
    out
}

fn is_script_dir(p: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(p) else { return false };
    rd.flatten().any(|e| {
        let path = e.path();
        path.extension().is_some_and(|x| x == "py")
            && std::fs::read_to_string(&path).map(|s| s.contains("getSettingData") && s.contains("def execute")).unwrap_or(false)
    })
}

/// What an installed package contributes to Curust.
#[derive(Default, Debug)]
pub struct InstallReport {
    pub package_id: String,
    pub display_name: String,
    pub package_type: String,
    pub materials: usize,
    pub profiles: usize,
    pub definitions: usize,
    pub scripts: usize,
    pub plugin_files: usize,
}

impl InstallReport {
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.materials > 0 {
            parts.push(format!("{} material profile{}", self.materials, if self.materials == 1 { "" } else { "s" }));
        }
        if self.profiles > 0 {
            parts.push(format!("{} print profile{}", self.profiles, if self.profiles == 1 { "" } else { "s" }));
        }
        if self.definitions > 0 {
            parts.push(format!("{} printer definition{}", self.definitions, if self.definitions == 1 { "" } else { "s" }));
        }
        if self.scripts > 0 {
            parts.push(format!("{} post-processing script{}", self.scripts, if self.scripts == 1 { "" } else { "s" }));
        }
        if parts.is_empty() {
            if self.plugin_files > 0 {
                return format!("{} was installed, but it is a Cura plugin written for Cura's Python interface; Curust can't run that kind of plugin.", self.display_name);
            }
            let kind = if self.package_type.is_empty() { "package".to_string() } else { format!("{} package", self.package_type) };
            return format!("{} was installed, but this {} contains nothing Curust can use.", self.display_name, kind);
        }
        format!("Installed {}: {}.", self.display_name, parts.join(", "))
    }
    pub fn usable(&self) -> bool {
        self.materials + self.profiles + self.definitions + self.scripts > 0
    }
}

fn safe_rel(name: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for part in name.split(['/', '\\']) {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." || part.contains(':') {
            return None;
        }
        out.push(part);
    }
    Some(out)
}

/// `PackageManager._installPackage` for a `.curapackage` file.
pub fn install_package(data: &[u8]) -> Result<InstallReport, String> {
    let entries = crate::zip::read_zip(data).map_err(|e| format!("Not a valid package: {}", e))?;
    let info = entries
        .iter()
        .find(|e| e.name.ends_with("package.json"))
        .and_then(|e| json::parse(&String::from_utf8_lossy(&e.data)).ok())
        .ok_or("The package has no package.json.")?;
    let package_id = info.get("package_id").and_then(|v| v.as_str()).ok_or("package.json has no package_id")?.to_string();
    if safe_rel(&package_id).map(|p| p.components().count() != 1).unwrap_or(true) {
        return Err("Invalid package id".into());
    }
    let mut report = InstallReport {
        display_name: info.get("display_name").and_then(|v| v.as_str()).unwrap_or(&package_id).to_string(),
        package_type: info.get("package_type").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        package_id: package_id.clone(),
        ..Default::default()
    };
    // Remove an older version first (`_purgePackage`).
    uninstall_files(&package_id);
    let user = user_resources_dir();
    let targets: &[(&str, PathBuf)] = &[
        ("materials", user.join("materials")),
        ("qualities", user.join("quality")),
        ("variants", user.join("variants")),
        ("intents", user.join("intent")),
        ("intent", user.join("intent")),
        ("images", user.join("images")),
        ("plugins", plugins_dir()),
    ];
    for e in &entries {
        if e.name.ends_with('/') {
            continue;
        }
        let Some(rel) = e.name.strip_prefix("files/") else { continue };
        let Some((sub, rest)) = rel.split_once('/') else { continue };
        let Some((_, root)) = targets.iter().find(|(n, _)| *n == sub) else { continue };
        let Some(rest) = safe_rel(rest) else { continue };
        let dst = root.join(&package_id).join(&rest);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {}", parent.display(), e))?;
        }
        std::fs::write(&dst, &e.data).map_err(|e| format!("{}: {}", dst.display(), e))?;
        let name = rest.to_string_lossy().to_lowercase();
        match sub {
            "materials" if name.ends_with(".xml.fdm_material") => report.materials += 1,
            "qualities" | "intents" | "intent" | "variants" if name.ends_with(".inst.cfg") => report.profiles += 1,
            "plugins" => {
                report.plugin_files += 1;
                if name.ends_with(".def.json") {
                    report.definitions += 1;
                } else if name.ends_with(".xml.fdm_material") {
                    report.materials += 1;
                } else if name.ends_with(".inst.cfg") {
                    report.profiles += 1;
                }
            }
            _ => {}
        }
    }
    // Post-processing scripts shipped inside plugins.
    let pdir = plugins_dir().join(&package_id);
    if pdir.is_dir() {
        let mut stack = vec![pdir];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    if p.file_name().is_some_and(|n| n == "scripts") && is_script_dir(&p) {
                        report.scripts += std::fs::read_dir(&p).map(|r| r.flatten().filter(|f| f.path().extension().is_some_and(|x| x == "py")).count()).unwrap_or(0);
                    } else {
                        stack.push(p);
                    }
                }
            }
        }
    }
    let mut index = installed();
    index.retain(|(k, _)| *k != package_id);
    let mut entry = match info {
        Json::Object(items) => items,
        _ => Vec::new(),
    };
    entry.retain(|(k, _)| !k.starts_with("curust_"));
    entry.push(("curust_summary".into(), Json::Str(report.summary())));
    entry.push(("curust_usable".into(), Json::Bool(report.usable())));
    index.push((package_id, Json::Object(entry)));
    save_index(&index);
    Ok(report)
}

fn uninstall_files(package_id: &str) {
    let user = user_resources_dir();
    for d in [user.join("materials"), user.join("quality"), user.join("variants"), user.join("intent"), user.join("images"), plugins_dir()] {
        let p = d.join(package_id);
        if p.is_dir() {
            let _ = std::fs::remove_dir_all(&p);
        }
    }
}

pub fn uninstall(package_id: &str) -> Result<(), String> {
    if safe_rel(package_id).map(|p| p.components().count() != 1).unwrap_or(true) {
        return Err("Invalid package id".into());
    }
    uninstall_files(package_id);
    let mut index = installed();
    index.retain(|(k, _)| k != package_id);
    save_index(&index);
    Ok(())
}

/// Packages installed in UltiMaker Cura (its `packages.json`), shown as "installed in Cura".
pub fn cura_installed() -> Vec<(String, Json)> {
    let mut out = Vec::new();
    for d in cura_data_dirs() {
        let Ok(text) = std::fs::read_to_string(d.join("packages.json")) else { continue };
        let Ok(j) = json::parse(&text) else { continue };
        if let Some(Json::Object(items)) = j.get("installed") {
            for (k, v) in items {
                let info = v.get("package_info").cloned().unwrap_or(Json::Null);
                out.push((k.clone(), info));
            }
        }
    }
    out
}

/// A page of the Marketplace (pass-through of the API's `data` and `links.next`), with the
/// installed state of each package.
pub fn search(url: &str) -> Result<Json, String> {
    if !url.starts_with(&packages_url()) {
        return Err("unexpected Marketplace URL".into());
    }
    let body = http_get(url)?;
    let j = json::parse(&String::from_utf8_lossy(&body)).map_err(|e| format!("Could not interpret the server's response: {}", e))?;
    let data = match j.get("data") {
        Some(Json::Array(a)) => a.clone(),
        _ => return Err("Could not interpret the server's response.".into()),
    };
    let next = j.get("links").and_then(|l| l.get("next")).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let inst = installed();
    let cura = cura_installed();
    let items: Vec<Json> = data
        .into_iter()
        .map(|p| {
            let id = p.get("package_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let mut items = match p {
                Json::Object(o) => o,
                _ => Vec::new(),
            };
            items.push(("curust_installed".into(), Json::Bool(inst.iter().any(|(k, _)| *k == id))));
            items.push(("cura_installed".into(), Json::Bool(cura.iter().any(|(k, _)| *k == id))));
            Json::Object(items)
        })
        .collect();
    Ok(Json::Object(vec![("data".into(), Json::Array(items)), ("next".into(), Json::Str(next))]))
}

/// Download (`PackageList.download`) and install a package.
pub fn download_and_install(package_id: &str, download_url: &str) -> Result<InstallReport, String> {
    let url = if download_url.is_empty() { format!("{}/{}/download", packages_url(), enc(package_id)) } else { download_url.to_string() };
    let data = http_get(&url)?;
    install_package(&data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(search_url("material", "pla plus"), "https://api.ultimaker.com/cura-packages/v1/cura/v8.12.0/packages?limit=20&package_type=material&search=pla%20plus&sort_by=last_updated");
        assert!(safe_rel("../x").is_none());
        assert_eq!(safe_rel("a/b/c.txt").unwrap(), PathBuf::from("a").join("b").join("c.txt"));
    }
}
