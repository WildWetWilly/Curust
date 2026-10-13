fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n: usize = args[1].parse().unwrap();
    let seed: u32 = args[2].parse().unwrap();
    cppstd::crand::srand(seed);
    let mut v: Vec<(i32, usize)> = (0..n).map(|i| (cppstd::crand::rand() % 50, i)).collect();
    cppstd::stable_sort_by(&mut v, |a, b| a.0 + 2 < b.0);
    let s: Vec<String> = v.iter().map(|p| p.1.to_string()).collect();
    println!("{} ", s.join(" "));
}
