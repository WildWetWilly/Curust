//! Images as models: Cura's ImageReader (height maps and lithophanes from PNG, JPEG, BMP and GIF
//! files), reproduced with the same float32 / float64 arithmetic as its numpy code.

use crate::mesh_io::LoadedModel;
use crate::qimage::{self, QImage};

/// The options of Cura's image import dialog (`ImageReaderUI`).
#[derive(Clone, Debug)]
pub struct ImageOptions {
    /// The larger of width and depth (mm).
    pub size: f64,
    /// "Height (mm)": the maximum distance of each pixel from the base.
    pub peak_height: f64,
    /// "Base (mm)".
    pub base_height: f64,
    pub smoothing: usize,
    /// "Lighter is higher" (height maps) instead of "Darker is higher" (lithophanes).
    pub lighter_is_higher: bool,
    /// Colour model "Translucency" (lithophanes) instead of "Linear".
    pub use_transparency_model: bool,
    /// "1mm Transmittance (%)".
    pub transmittance_1mm: f64,
}

impl Default for ImageOptions {
    fn default() -> Self {
        ImageOptions { size: 120.0, peak_height: 2.5, base_height: 0.4, smoothing: 1, lighter_is_higher: false, use_transparency_model: true, transmittance_1mm: 50.0 }
    }
}

pub fn is_image_file(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    [".jpg", ".jpeg", ".bmp", ".gif", ".png"].iter().any(|e| l.ends_with(e))
}

/// Python's `round()` (half to even) for a float, as an integer.
fn py_round(x: f64) -> i64 {
    let r = x.round();
    if (x - x.trunc()).abs() == 0.5 {
        2 * (x / 2.0).round() as i64
    } else {
        r as i64
    }
}

/// `ImageReader._generateSceneNode`: the mesh (triangles, Y up) for an image.
pub fn generate_mesh(img: &QImage, opts: &ImageOptions) -> Vec<[f32; 3]> {
    let max_size = 512usize;
    let mut width = img.width.max(2);
    let mut height = img.height.max(2);
    let aspect = height as f64 / width as f64;
    let mut img = if img.width < 2 || img.height < 2 { img.scaled(width, height) } else { img.clone() };

    let height_from_base = opts.peak_height.max(0.0);
    let base_height = opts.base_height.max(0.0);
    let xz_size = opts.size.max(1.0);
    let (mut sx, sy, mut sz) = (xz_size, height_from_base, xz_size);
    if width > height {
        sz *= aspect;
    } else if height > width {
        sx /= aspect;
    }
    if width > max_size || height > max_size {
        let scale_factor = if height > width { max_size as f64 / height as f64 } else { max_size as f64 / width as f64 };
        width = py_round(width as f64 * scale_factor).max(2) as usize;
        height = py_round(height as f64 * scale_factor).max(2) as usize;
        img = img.scaled(width, height);
    }
    let wm1 = width - 1;
    let hm1 = height - 1;
    let texel_width = 1.0 / wm1 as f64 * sx;
    let texel_height = 1.0 / hm1 as f64 * sz;

    // Heights (float32), from the pixel luminance.
    let pow_table: Vec<f64> = (0..256).map(|i| (i as f64 / 255.0).powf(2.2)).collect();
    let mut hd = vec![0f32; width * height];
    for y in 0..height {
        for x in 0..width {
            let p = img.pixel(x, y);
            let (r, g, b) = ((p >> 16) & 255, (p >> 8) & 255, p & 255);
            hd[y * width + x] = if opts.use_transparency_model {
                (0.299 * pow_table[r as usize] + 0.587 * pow_table[g as usize] + 0.114 * pow_table[b as usize]) as f32
            } else {
                ((0.212655 * r as f64 + 0.715158 * g as f64 + 0.072187 * b as f64) / 255.0) as f32
            };
        }
    }
    if opts.lighter_is_higher == opts.use_transparency_model {
        hd.iter_mut().for_each(|v| *v = 1.0f32 - *v);
    }
    for _ in 0..opts.smoothing {
        // numpy.pad(mode="edge") then the eight neighbours, added in Cura's order.
        let src = hd.clone();
        let at = |x: isize, y: isize| -> f32 {
            let x = x.clamp(0, width as isize - 1) as usize;
            let y = y.clamp(0, height as isize - 1) as usize;
            src[y * width + x]
        };
        for y in 0..height as isize {
            for x in 0..width as isize {
                let mut v = hd[y as usize * width + x as usize];
                for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
                    v += at(x + dx, y + dy);
                }
                hd[y as usize * width + x as usize] = v / 9.0f32;
            }
        }
    }
    if opts.use_transparency_model {
        let divisor = 1.0 / (opts.transmittance_1mm / 100.0).ln();
        let min_luminance = (opts.transmittance_1mm / 100.0).powf(height_from_base);
        for v in hd.iter_mut() {
            let mapped = min_luminance + (1.0 - min_luminance) * *v as f64;
            *v = (base_height + divisor * mapped.ln()) as f32;
        }
    } else {
        let sy32 = sy as f32;
        let base32 = base_height as f32;
        for v in hd.iter_mut() {
            *v *= sy32;
            *v += base32;
        }
    }
    if img.has_alpha_channel() {
        for y in 0..height {
            for x in 0..width {
                let a = (img.pixel(x, y) >> 24) as f64;
                let v = &mut hd[y * width + x];
                *v = (*v as f64 * (a / 255.0)) as f32;
            }
        }
    }
    let h = |x: usize, y: usize| hd[y * width + x];

    let mut out: Vec<[f32; 3]> = Vec::with_capacity(6 * wm1 * hm1 + 6 + 12 * (wm1 + hm1));
    // Height map quads (float32 arithmetic like the numpy code).
    let tw = texel_width as f32;
    let th = texel_height as f32;
    for z in 0..hm1 {
        let oz = z as f32 * th;
        for x in 0..wm1 {
            let ox = x as f32 * tw;
            let corners = [(0.0f32, 0.0f32), (0.0, th), (tw, th), (tw, th), (tw, 0.0), (0.0, 0.0)];
            let ys = [h(x, z), h(x, z + 1), h(x + 1, z + 1), h(x + 1, z + 1), h(x + 1, z), h(x, z)];
            for (k, (cx, cz)) in corners.iter().enumerate() {
                out.push([cx + ox, ys[k], cz + oz]);
            }
        }
    }
    let geo_width = wm1 as f64 * texel_width;
    let geo_height = hm1 as f64 * texel_height;
    let mut face = |a: [f64; 3], b: [f64; 3], c: [f64; 3]| {
        for p in [a, b, c] {
            out.push([p[0] as f32, p[1] as f32, p[2] as f32]);
        }
    };
    // Bottom.
    face([0.0, 0.0, 0.0], [0.0, 0.0, geo_height], [geo_width, 0.0, geo_height]);
    face([geo_width, 0.0, geo_height], [geo_width, 0.0, 0.0], [0.0, 0.0, 0.0]);
    // North and south walls.
    for n in 0..wm1 {
        let x = n as f64 * texel_width;
        let nx = (n + 1) as f64 * texel_width;
        let (hn0, hn1) = (h(n, 0) as f64, h(n + 1, 0) as f64);
        let (hs0, hs1) = (h(n, hm1) as f64, h(n + 1, hm1) as f64);
        face([x, 0.0, 0.0], [nx, 0.0, 0.0], [nx, hn1, 0.0]);
        face([nx, hn1, 0.0], [x, hn0, 0.0], [x, 0.0, 0.0]);
        face([x, 0.0, geo_height], [nx, 0.0, geo_height], [nx, hs1, geo_height]);
        face([nx, hs1, geo_height], [x, hs0, geo_height], [x, 0.0, geo_height]);
    }
    // West and east walls.
    for n in 0..hm1 {
        let y = n as f64 * texel_height;
        let ny = (n + 1) as f64 * texel_height;
        let (hw0, hw1) = (h(0, n) as f64, h(0, n + 1) as f64);
        let (he0, he1) = (h(wm1, n) as f64, h(wm1, n + 1) as f64);
        face([0.0, 0.0, y], [0.0, 0.0, ny], [0.0, hw1, ny]);
        face([0.0, hw1, ny], [0.0, hw0, y], [0.0, 0.0, y]);
        face([geo_width, 0.0, y], [geo_width, 0.0, ny], [geo_width, he1, ny]);
        face([geo_width, he1, ny], [geo_width, he0, y], [geo_width, 0.0, y]);
    }
    out
}

/// Read an image file as a model (`ImageReader.read`).
pub fn image_model(name: &str, data: &[u8], opts: &ImageOptions) -> Result<LoadedModel, String> {
    let img = qimage::load(data).ok_or("Image is corrupt or in an unsupported format")?;
    let vertices = generate_mesh(&img, opts);
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name).to_string();
    Ok(LoadedModel { name: base, vertices, transform: None, settings: Vec::new() })
}

