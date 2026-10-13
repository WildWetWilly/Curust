//! Counterpart of `tools/difftest/png_simplified_driver.c`: decode PNG files like
//! `MeshUtils::loadTextureFromPngData` and print the result for comparison with libpng.

use curust_engine::utils::image::png;

fn fnv(d: &[u8]) -> u64 {
    let mut h: u64 = 1469598103934665603;
    for &b in d {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    h
}

fn main() {
    for path in std::env::args().skip(1) {
        let data = std::fs::read(&path).unwrap();
        match png::decode_simplified(&data) {
            Err(_) => println!("{} FAIL", path),
            Ok(img) => {
                let mut line = format!("{} OK {} {} {} {:016x} texts={}", path, img.width, img.height, img.bytes_per_pixel, fnv(&img.data), img.texts.len());
                for t in &img.texts {
                    line += &format!(" [{}:{:016x}:{}]", t.key, fnv(&t.text), t.text.len());
                }
                println!("{}", line);
            }
        }
    }
}
