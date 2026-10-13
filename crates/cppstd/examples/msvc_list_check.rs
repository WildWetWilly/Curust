//! Prints permutation hashes of `cppstd::msvc::list_sort_perm` for comparison with Microsoft's
//! `std::list::sort` (verbatim `_Merge_same` / `_Sort`), including inconsistent comparators.
fn main() {
    let mut seed: u64 = 4242;
    for trial in 0..3000usize {
        let n = trial % 500;
        let m = 2 + (trial % 30) as u64;
        let kind = trial % 3;
        let v: Vec<(i32, i32)> = (0..n)
            .map(|i| {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                (((seed >> 33) % m) as i32, i as i32)
            })
            .collect();
        let perm = match kind {
            0 => cppstd::msvc::list_sort_perm(&v, &mut |a: &(i32, i32), b: &(i32, i32)| a.0 < b.0),
            1 => cppstd::msvc::list_sort_perm(&v, &mut |a: &(i32, i32), b: &(i32, i32)| a.0 + 2 < b.0),
            _ => cppstd::msvc::list_sort_perm(&v, &mut |a: &(i32, i32), b: &(i32, i32)| (a.0 ^ b.1) % 3 == 0),
        };
        let mut h: u64 = 1469598103934665603;
        for &p in &perm {
            h = (h ^ v[p].1 as u64).wrapping_mul(1099511628211);
        }
        println!("{} {:016x}", trial, h);
    }
}
