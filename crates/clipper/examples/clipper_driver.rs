//! Differential test driver: same protocol as tools/difftest/clipper_driver.cpp.
use clipper::*;
use std::io::{self, BufRead, Write};

fn read_path(it: &mut std::str::SplitWhitespace) -> Path {
    let n: usize = it.next().unwrap().parse().unwrap();
    (0..n)
        .map(|_| {
            let x = it.next().unwrap().parse().unwrap();
            let y = it.next().unwrap().parse().unwrap();
            IntPoint::new(x, y)
        })
        .collect()
}
fn fmt_path(p: &[IntPoint]) -> String {
    p.iter().map(|q| format!("{} {}", q.x, q.y)).collect::<Vec<_>>().join(" ")
}
fn print_tree(out: &mut impl Write, t: &PolyTree) {
    let mut n = t.get_first();
    while let Some(i) = n {
        let mut depth = 1;
        let mut p = t.node(i).parent;
        while let Some(pp) = p {
            depth += 1;
            p = t.node(pp).parent;
        }
        writeln!(out, "N {} {} {} {}", depth, t.node(i).is_open() as i32, t.is_hole(i) as i32, fmt_path(&t.node(i).contour)).unwrap();
        n = t.get_next(Some(i));
    }
}
fn ct(i: i32) -> ClipType {
    [ClipType::Intersection, ClipType::Union, ClipType::Difference, ClipType::Xor][i as usize]
}
fn pft(i: i32) -> PolyFillType {
    [PolyFillType::EvenOdd, PolyFillType::NonZero, PolyFillType::Positive, PolyFillType::Negative][i as usize]
}
fn main() {
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();
    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    while let Some(Ok(line)) = lines.next() {
        let mut hs = line.split_whitespace();
        let mode = hs.next().unwrap_or("");
        let nums: Vec<String> = hs.map(|s| s.to_string()).collect();
        match mode {
            "clip" => {
                let v: Vec<i32> = nums.iter().map(|s| s.parse().unwrap()).collect();
                let mut c = Clipper::new(0);
                c.set_strictly_simple(v[3] != 0);
                c.set_preserve_collinear(v[5] != 0);
                c.set_reverse_solution(v[6] != 0);
                loop {
                    let l = lines.next().unwrap().unwrap();
                    if l == "END" {
                        break;
                    }
                    let mut it = l.split_whitespace();
                    let t = it.next().unwrap();
                    let closed: i32 = it.next().unwrap().parse().unwrap();
                    let p = read_path(&mut it);
                    c.add_path(&p, if t == "S" { PolyType::Subject } else { PolyType::Clip }, closed != 0);
                }
                if v[4] != 0 {
                    let mut t = PolyTree::new();
                    let ok = c.execute_tree(ct(v[0]), &mut t, pft(v[1]), pft(v[2]));
                    writeln!(out, "OK {}", ok as i32).unwrap();
                    print_tree(&mut out, &t);
                } else {
                    let mut sol = Paths::new();
                    let ok = c.execute(ct(v[0]), &mut sol, pft(v[1]), pft(v[2]));
                    writeln!(out, "OK {}", ok as i32).unwrap();
                    for p in &sol {
                        writeln!(out, "{}", fmt_path(p)).unwrap();
                    }
                }
                writeln!(out, "DONE").unwrap();
            }
            "offset" => {
                let delta: f64 = nums[0].parse().unwrap();
                let ml: f64 = nums[1].parse().unwrap();
                let at: f64 = nums[2].parse().unwrap();
                let tree: i32 = nums[3].parse().unwrap();
                let mut o = ClipperOffset::new(ml, at);
                loop {
                    let l = lines.next().unwrap().unwrap();
                    if l == "END" {
                        break;
                    }
                    let mut it = l.split_whitespace();
                    it.next();
                    let jt: usize = it.next().unwrap().parse().unwrap();
                    let et: usize = it.next().unwrap().parse().unwrap();
                    let p = read_path(&mut it);
                    o.add_path(
                        &p,
                        [JoinType::Square, JoinType::Round, JoinType::Miter][jt],
                        [EndType::ClosedPolygon, EndType::ClosedLine, EndType::OpenButt, EndType::OpenSquare, EndType::OpenRound][et],
                    );
                }
                writeln!(out, "OK 1").unwrap();
                if tree != 0 {
                    let mut t = PolyTree::new();
                    o.execute_tree(&mut t, delta);
                    print_tree(&mut out, &t);
                } else {
                    for p in o.execute_paths(delta) {
                        writeln!(out, "{}", fmt_path(&p)).unwrap();
                    }
                }
                writeln!(out, "DONE").unwrap();
            }
            "clean" => {
                let d: f64 = nums[0].parse().unwrap();
                let l = lines.next().unwrap().unwrap();
                let mut it = l.split_whitespace();
                it.next();
                let p = read_path(&mut it);
                lines.next();
                writeln!(out, "OK 1\n{}\nDONE", fmt_path(&clean_polygon(&p, d))).unwrap();
            }
            "sort" => {
                let n: usize = nums[0].parse().unwrap();
                let mut v: Vec<(i64, usize)> = (0..n).map(|i| (nums[1 + i].parse().unwrap(), i)).collect();
                cppstd::sort_by(&mut v, |a, b| a.0 < b.0);
                writeln!(out, "OK 1\n{}\nDONE", v.iter().map(|x| x.1.to_string()).collect::<Vec<_>>().join(" ")).unwrap();
            }
            "heap" => {
                let n: usize = nums[0].parse().unwrap();
                let mut pq = cppstd::PriorityQueue::new(|a: &(i64, usize), b: &(i64, usize)| a.0 < b.0);
                for i in 0..n {
                    pq.push((nums[1 + i].parse().unwrap(), i));
                }
                let mut r = vec![];
                while let Some(x) = pq.pop() {
                    r.push(x.1.to_string());
                }
                writeln!(out, "OK 1\n{}\nDONE", r.join(" ")).unwrap();
            }
            _ => {}
        }
        out.flush().unwrap();
    }
}
