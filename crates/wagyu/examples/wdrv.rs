//! Differential test driver: same text protocol as the C++ reference driver used during porting.
use std::io::Read;
use wagyu::*;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let ct = args.get(1).map(|s| s.parse::<i32>().unwrap()).unwrap_or(1);
    let ft = args.get(2).map(|s| s.parse::<i32>().unwrap()).unwrap_or(0);
    let mut s = String::new();
    std::io::stdin().read_to_string(&mut s).unwrap();
    let mut it = s.split_ascii_whitespace().map(|t| t.parse::<i64>().unwrap());
    let nr = it.next().unwrap();
    let mut w = Wagyu::new();
    for _ in 0..nr {
        let pt = it.next().unwrap();
        let n = it.next().unwrap();
        let mut ring = Vec::new();
        for _ in 0..n {
            let x = it.next().unwrap();
            let y = it.next().unwrap();
            ring.push(P::new(x, y));
        }
        let ok = w.add_ring(&ring, if pt != 0 { PolygonType::Clip } else { PolygonType::Subject });
        println!("add {}", ok as i32);
    }
    let ct = [ClipType::Intersection, ClipType::Union, ClipType::Difference, ClipType::XOr][ct as usize];
    let ft = [FillType::EvenOdd, FillType::NonZero, FillType::Positive, FillType::Negative][ft as usize];
    let mut sln = MultiPolygon::new();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| w.execute(ct, &mut sln, ft, ft)));
    match r {
        Err(_) => {
            println!("exception");
            return;
        }
        Ok(ok) => println!("exec {}", ok as i32),
    }
    println!("{}", sln.len());
    for p in &sln {
        println!("{}", p.len());
        for r in p {
            print!("{}", r.len());
            for q in r {
                print!(" {} {}", q.x, q.y);
            }
            println!();
        }
    }
}
