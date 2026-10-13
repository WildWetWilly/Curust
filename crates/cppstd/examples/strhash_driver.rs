//! Reads "inserted keys" lines (tab separated) on stdin and prints the libstdc++
//! `std::unordered_map<std::string, ...>` iteration order (see tools/difftest for the C++ side).
use cppstd::unordered::{string_hash, UnorderedSet};
use std::io::BufRead;

fn main() {
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap();
        let mut set: UnorderedSet<String> = UnorderedSet::new(string_hash);
        for k in line.strip_suffix('\t').unwrap_or(&line).split('\t') {
            set.insert(k.to_string());
        }
        let order: Vec<String> = set.iter().cloned().collect();
        println!("{}\t", order.join("\t"));
    }
    println!("{}", string_hash(&"extruder.0".to_string()));
}
