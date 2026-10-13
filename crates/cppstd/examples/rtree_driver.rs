//! Differential test driver for `cppstd::boost_rtree` (see tools/difftest/rtree_driver.cpp).
use cppstd::boost_rtree::PackedRtree3;
use std::io::{Read, Write};

fn main() {
    let mut s = String::new();
    std::io::stdin().read_to_string(&mut s).unwrap();
    let mut it = s.split_whitespace();
    let n: usize = it.next().unwrap().parse().unwrap();
    let mut pts = Vec::with_capacity(n);
    for _ in 0..n {
        let x: f64 = it.next().unwrap().parse().unwrap();
        let y: f64 = it.next().unwrap().parse().unwrap();
        let z: f64 = it.next().unwrap().parse().unwrap();
        pts.push([x, y, z]);
    }
    let tree = PackedRtree3::new(pts);
    let out = std::io::stdout();
    let mut out = std::io::BufWriter::new(out.lock());
    while let Some(x) = it.next() {
        let q = [x.parse().unwrap(), it.next().unwrap().parse().unwrap(), it.next().unwrap().parse().unwrap()];
        writeln!(out, "{}", tree.nearest(&q).map(|i| i as i64).unwrap_or(-1)).unwrap();
    }
}
