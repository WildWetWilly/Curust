//! Differential test driver: prints the full diagram in the same format as the C++ driver.
use std::io::Read;
use voronoi::*;

fn hexf(v: f64) -> String {
    // printf("%a") formatting as glibc does it.
    if v == 0.0 {
        return if v.is_sign_negative() { "-0x0p+0".into() } else { "0x0p+0".into() };
    }
    if v.is_nan() {
        return if v.is_sign_negative() { "-nan".into() } else { "nan".into() };
    }
    if v.is_infinite() {
        return if v < 0.0 { "-inf".into() } else { "inf".into() };
    }
    let bits = v.to_bits();
    let sign = if bits >> 63 != 0 { "-" } else { "" };
    let exp_bits = ((bits >> 52) & 0x7ff) as i64;
    let mut mant = bits & ((1u64 << 52) - 1);
    let (lead, exp) = if exp_bits == 0 { (0, -1022) } else { (1, exp_bits - 1023) };
    let mut digits = 13;
    while digits > 0 && mant & 0xf == 0 {
        mant >>= 4;
        digits -= 1;
    }
    if digits == 0 {
        format!("{}0x{}p{:+}", sign, lead, exp)
    } else {
        format!("{}0x{}.{:0width$x}p{:+}", sign, lead, mant, exp, width = digits)
    }
}

fn main() {
    let mut s = String::new();
    std::io::stdin().read_to_string(&mut s).unwrap();
    let mut it = s.split_ascii_whitespace().map(|t| t.parse::<i64>().unwrap());
    let np = it.next().unwrap();
    let mut b = VoronoiBuilder::new();
    for _ in 0..np {
        let x = it.next().unwrap();
        let y = it.next().unwrap();
        b.insert_point(x as i32, y as i32);
    }
    let ns = it.next().unwrap();
    for _ in 0..ns {
        let v: Vec<i64> = (0..4).map(|_| it.next().unwrap()).collect();
        b.insert_segment(v[0] as i32, v[1] as i32, v[2] as i32, v[3] as i32);
    }
    let mut vd = VoronoiDiagram::new();
    b.construct(&mut vd);
    let idx = |i: usize| -> i64 { if i == NIL { -1 } else { i as i64 } };
    let mut out = String::new();
    use std::fmt::Write;
    writeln!(out, "cells {}", vd.num_cells()).unwrap();
    for c in vd.cells() {
        writeln!(out, "c {} {} {} {}", c.source_index(), c.source_category() as usize, idx(c.incident_edge()), c.color()).unwrap();
    }
    writeln!(out, "vertices {}", vd.num_vertices()).unwrap();
    for v in vd.vertices() {
        writeln!(out, "v {} {} {}", hexf(v.x()), hexf(v.y()), idx(v.incident_edge())).unwrap();
    }
    writeln!(out, "edges {}", vd.num_edges()).unwrap();
    for e in vd.edges() {
        writeln!(out, "e {} {} {} {} {} {} {}", e.cell(), idx(e.vertex0()), idx(e.twin()), idx(e.next()), idx(e.prev()), e.is_linear() as i32, e.is_primary() as i32).unwrap();
    }
    print!("{}", out);
}
