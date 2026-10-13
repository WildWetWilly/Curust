//! Prints permutation hashes of `cppstd::msvc::sort_by` for comparison with Microsoft's STL
//! `_Sort_unchecked` (compiled from the original source, see tools/difftest).
fn main() {
    let mut seed: u64 = 12345;
    for trial in 0..3000usize {
        let n = trial % 700;
        let m = 1 + (trial % 13) as u64;
        let mut v: Vec<(i32, i32)> = (0..n)
            .map(|i| {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                (((seed >> 33) % m) as i32, i as i32)
            })
            .collect();
        cppstd::msvc::sort_by(&mut v, &mut |a: &(i32, i32), b: &(i32, i32)| a.0 < b.0);
        let mut h: u64 = 1469598103934665603;
        for p in &v {
            h = (h ^ p.1 as u64).wrapping_mul(1099511628211);
        }
        println!("{} {:016x}", trial, h);
    }
}
