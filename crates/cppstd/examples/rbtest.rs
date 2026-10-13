use cppstd::rbtree::RbMap;
fn main() {
    let mut m: RbMap<i64, i64> = RbMap::new();
    let less = |a: &i64, b: &i64| a < b;
    let mut x: u64 = 987654321;
    let mut its: std::collections::HashMap<i64, usize> = Default::default();
    for step in 0..20000 {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let k = ((x >> 33) % 300) as i64;
        let mode = (x >> 20) % 4;
        if let Some(&it) = its.get(&k) {
            m.erase(it);
            its.remove(&k);
        } else {
            let it = match mode {
                0 => m.insert(k, 0, less).0,
                1 => { let h = m.lower_bound(&k, less); m.insert_hint(h, k, 0, less) }
                2 => { let h = m.end(); m.insert_hint(h, k, 0, less) }
                _ => { let h = m.begin(); m.insert_hint(h, k, 0, less) }
            };
            its.insert(k, it);
        }
        if step % 97 == 0 { println!("{}", m.debug_structure()); }
    }
    println!("{}", m.debug_structure());
}
