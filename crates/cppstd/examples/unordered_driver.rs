//! Differential test driver for `cppstd::unordered::UnorderedSet` (see tools/difftest/unordered_driver.cpp).
use cppstd::unordered::UnorderedSet;
use std::io::{BufRead, Write};

fn hash3(p: &(i64, i64, i64)) -> u64 {
    let prime: i32 = 31;
    let mut result: i32 = 89;
    result = ((result.wrapping_mul(prime) as i64).wrapping_add(p.0)) as i32;
    result = ((result.wrapping_mul(prime) as i64).wrapping_add(p.1)) as i32;
    result = ((result.wrapping_mul(prime) as i64).wrapping_add(p.2)) as i32;
    result as i64 as u64
}

fn main() {
    let mut sets: Vec<UnorderedSet<(i64, i64, i64)>> = (0..4).map(|_| UnorderedSet::new(hash3)).collect();
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    for line in stdin.lock().lines() {
        let line = line.unwrap();
        let t: Vec<&str> = line.split_whitespace().collect();
        match t[0] {
            "I" | "E" => {
                let id: usize = t[1].parse().unwrap();
                let p = (t[2].parse().unwrap(), t[3].parse().unwrap(), t[4].parse().unwrap());
                if t[0] == "I" {
                    sets[id].insert(p);
                } else {
                    sets[id].remove(&p);
                }
            }
            "M" => {
                let a: usize = t[1].parse().unwrap();
                let b: usize = t[2].parse().unwrap();
                if a == b {
                    continue; // merging a set into itself moves nothing
                }
                let mut src = std::mem::replace(&mut sets[b], UnorderedSet::new(hash3));
                sets[a].merge(&mut src);
                sets[b] = src;
            }
            "P" => {
                let id: usize = t[1].parse().unwrap();
                write!(out, "{} {}:", sets[id].len(), sets[id].bucket_count()).unwrap();
                for p in sets[id].iter() {
                    write!(out, " {},{},{}", p.0, p.1, p.2).unwrap();
                }
                writeln!(out).unwrap();
            }
            _ => {}
        }
    }
}
