//! Differential test driver for `cppstd::boost_foa` (see tools/difftest/foa_driver.cpp).
use cppstd::boost_foa::{FoaMap, FoaSet};
use std::io::{BufRead, Write};

fn h(k: &u64) -> u64 {
    *k
}

fn main() {
    let mut maps: Vec<FoaMap<u64, u8>> = (0..2).map(|_| FoaMap::new(h)).collect();
    let mut sets: Vec<FoaSet<u64>> = (0..2).map(|_| FoaSet::new(h)).collect();
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    for line in stdin.lock().lines() {
        let line = line.unwrap();
        let t: Vec<&str> = line.split_whitespace().collect();
        let id: usize = t[1].parse().unwrap();
        let n = |i: usize| -> u64 { t[i].parse().unwrap() };
        match t[0] {
            "A" => {
                maps[id].insert_or_assign(n(2), n(3) as u8);
            }
            "V" => {
                let vv = n(3) as u8;
                maps[id].emplace_or_visit(n(2), vv, |e| *e = (*e).min(vv));
            }
            "S" => {
                sets[id].insert(n(2));
            }
            "E" => {
                maps[id].erase(&n(2));
            }
            "F" => {
                sets[id].erase(&n(2));
            }
            "X" => {
                let (m, r) = (n(2), n(3));
                maps[id].erase_if(|k, _| k % m == r);
            }
            "Y" => {
                let (m, r) = (n(2), n(3));
                sets[id].erase_if(|k| k % m == r);
            }
            "P" => {
                write!(out, "{}:", maps[id].len()).unwrap();
                for (k, v) in maps[id].iter() {
                    write!(out, " {}={}", k, v).unwrap();
                }
                writeln!(out).unwrap();
            }
            "Q" => {
                write!(out, "{}:", sets[id].len()).unwrap();
                for k in sets[id].iter() {
                    write!(out, " {}", k).unwrap();
                }
                writeln!(out).unwrap();
            }
            _ => {}
        }
    }
}
