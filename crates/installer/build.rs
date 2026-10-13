// Embeds the payload (a zip with the application files) named by CURUST_PAYLOAD; without it the
// installer is built with an empty payload (useful for checking that it compiles).
// On Windows targets with LLVM's linker (or MSVC) it also links windows/setup.res: the icon, the
// manifest (administrator rights, visual styles, DPI awareness) and the version information.
fn main() {
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("payload.zip");
    match std::env::var("CURUST_PAYLOAD") {
        Ok(p) if !p.is_empty() => {
            std::fs::copy(&p, &out).unwrap_or_else(|e| panic!("CURUST_PAYLOAD={}: {}", p, e));
            println!("cargo:rerun-if-changed={}", p);
        }
        _ => {
            std::fs::write(&out, b"").unwrap();
        }
    }
    println!("cargo:rerun-if-env-changed=CURUST_PAYLOAD");
    println!("cargo:rerun-if-changed=windows/setup.res");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let abi = std::env::var("CARGO_CFG_TARGET_ABI").unwrap_or_default();
    if os == "windows" && (env == "msvc" || (env == "gnu" && abi == "llvm")) {
        let res = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("windows").join("setup.res");
        println!("cargo:rustc-link-arg-bins={}", res.display());
    }
}
