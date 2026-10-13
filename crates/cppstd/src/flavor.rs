//! Which C++ standard library CuraEngine was built with.
//!
//! UltiMaker ships CuraEngine built with GCC and libstdc++ on Linux and with Microsoft's compiler,
//! STL and Universal CRT on Windows. Where the C++ standard leaves results to the implementation
//! (the order of equal elements after `std::sort`, `std::lerp` rounding, `rand()`, unordered
//! container iteration), the two builds produce different G-code for the same input. Curust follows
//! the official build of the platform it runs on: libstdc++ semantics on Linux / macOS, Microsoft's
//! on Windows. `CURUST_CXX_FLAVOR=gnu|msvc` overrides it (for testing).

use std::sync::atomic::{AtomicU8, Ordering};

static FLAVOR: AtomicU8 = AtomicU8::new(0); // 0 = not yet decided, 1 = libstdc++, 2 = MSVC

/// Whether Microsoft STL / UCRT semantics are emulated.
#[inline]
pub fn msvc() -> bool {
    match FLAVOR.load(Ordering::Relaxed) {
        1 => false,
        2 => true,
        _ => {
            let m = match std::env::var("CURUST_CXX_FLAVOR").ok().as_deref() {
                Some("msvc") | Some("windows") => true,
                Some("gnu") | Some("gcc") | Some("linux") | Some("libstdc++") => false,
                _ => cfg!(windows),
            };
            FLAVOR.store(if m { 2 } else { 1 }, Ordering::Relaxed);
            m
        }
    }
}

/// Choose the emulated standard library explicitly.
pub fn set_msvc(on: bool) {
    FLAVOR.store(if on { 2 } else { 1 }, Ordering::Relaxed);
}
