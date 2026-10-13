//! Links the Windows resources (application icon, version information) into the executables,
//! and embeds Cura's resources for the portable build.
//! `windows/curust.res` is compiled from `windows/curust.rc` (`llvm-rc curust.rc` or `rc curust.rc`)
//! and checked in, so building needs no resource compiler. MSVC's link.exe and LLVM's lld accept
//! .res files directly; GNU ld does not, so the plain `windows-gnu` target goes without an icon.

fn main() {
    println!("cargo:rerun-if-changed=windows/curust.res");
    // The portable build embeds Cura's resources (`CURUST_EMBED_RESOURCES=curust-resources.zip`).
    println!("cargo:rerun-if-env-changed=CURUST_EMBED_RESOURCES");
    let embedded = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("embedded_resources.zip");
    match std::env::var("CURUST_EMBED_RESOURCES") {
        Ok(p) if !p.is_empty() => {
            println!("cargo:rerun-if-changed={}", p);
            std::fs::copy(&p, &embedded).unwrap_or_else(|e| panic!("CURUST_EMBED_RESOURCES={}: {}", p, e));
        }
        _ => std::fs::write(&embedded, b"").unwrap(),
    }
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let abi = std::env::var("CARGO_CFG_TARGET_ABI").unwrap_or_default();
    if os == "windows" && (env == "msvc" || (env == "gnu" && abi == "llvm")) {
        let res = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("windows").join("curust.res");
        println!("cargo:rustc-link-arg-bins={}", res.display());
    }
}
