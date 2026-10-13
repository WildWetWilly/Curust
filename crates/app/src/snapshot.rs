//! `cura.Snapshot.snapshot`: a picture of the build plate's models for thumbnails, rendered in
//! software with Cura's camera placement (look at the scene's bounding box from (-1, 1, 2) ×
//! 1.75 × size, 30° field of view, halved once when the models fill less than half the image),
//! the preview pass's shading (`overhang.shader`: ambient 0.1, |N·L| diffuse with the
//! "prettier" material colour, specular 0.6^20, light 50 mm above the camera), Cura's
//! square crop around the models, and a smooth downscale. Encoded as PNG.

use crate::session::Session;
use curust_settings::stack::index;

struct Tri {
    v: [[f64; 3]; 3],
    color: [f64; 3],
}

type V3 = [f64; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn norm(a: V3) -> V3 {
    let l = dot(a, a).sqrt();
    if l == 0.0 {
        a
    } else {
        [a[0] / l, a[1] / l, a[2] / l]
    }
}

fn parse_color(hex: &str) -> [f64; 3] {
    let h = hex.trim_start_matches('#');
    let c = |i: usize| -> f64 { h.get(i..i + 2).and_then(|x| u8::from_str_radix(x, 16).ok()).unwrap_or(200) as f64 / 255.0 };
    [c(0), c(2), c(4)]
}

/// `PreviewPass.prettier_color`.
fn prettier(c: [f64; 3]) -> [f64; 3] {
    let m = c[0].max(c[1]).max(c[2]);
    let f = if m > 0.0 { (1.0 / m).min(2.5) } else { 1.0 };
    [c[0] * f, c[1] * f, c[2] * f]
}

fn collect(s: &Session) -> Vec<Tri> {
    let mut out = Vec::new();
    let m = s.machine();
    for (idx, n) in s.nodes.iter().enumerate() {
        if !s.is_normal_mesh(idx) {
            continue;
        }
        let color = m
            .and_then(|m| {
                let list = m.extruder_list();
                let e = list.get(n.extruder).copied().or_else(|| list.first().copied())?;
                let c = m.extruders[e].container(index::MATERIAL);
                c.meta_str("color_code").or_else(|| s.reg.metadata(c.id()).and_then(|e| e.get_str("color_code")))
            })
            .unwrap_or_else(|| "#ffc924".into());
        let color = prettier(parse_color(&color));
        let w = n.world();
        let tf = |p: &[f32; 3]| -> V3 {
            let (x, y, z) = (p[0] as f64, p[1] as f64, p[2] as f64);
            [w[0][0] * x + w[0][1] * y + w[0][2] * z + w[0][3], w[1][0] * x + w[1][1] * y + w[1][2] * z + w[1][3], w[2][0] * x + w[2][1] * y + w[2][2] * z + w[2][3]]
        };
        for f in n.mesh.vertices.chunks_exact(3) {
            out.push(Tri { v: [tf(&f[0]), tf(&f[1]), tf(&f[2])], color });
        }
    }
    out
}

struct Camera {
    pos: V3,
    right: V3,
    up: V3,
    fwd: V3,
    /// 1 / tan(fovy / 2)
    f: f64,
    aspect: f64,
}

impl Camera {
    fn look(pos: V3, at: V3, fovy_deg: f64, aspect: f64) -> Camera {
        let fwd = norm(sub(at, pos));
        let right = norm(cross(fwd, [0.0, 1.0, 0.0]));
        let up = cross(right, fwd);
        Camera { pos, right, up, fwd, f: 1.0 / (fovy_deg.to_radians() / 2.0).tan(), aspect }
    }
    /// Screen position (pixels) and view depth.
    fn project(&self, p: V3, w: usize, h: usize) -> (f64, f64, f64) {
        let d = sub(p, self.pos);
        let z = dot(d, self.fwd);
        let x = dot(d, self.right) * self.f / self.aspect / z;
        let y = dot(d, self.up) * self.f / z;
        ((x + 1.0) * 0.5 * w as f64, (1.0 - y) * 0.5 * h as f64, z)
    }
}

/// Render RGBA (straight alpha, 0..1 floats).
fn render(tris: &[Tri], cam: &Camera, w: usize, h: usize) -> Vec<[f64; 4]> {
    let mut color = vec![[0.0f64; 4]; w * h];
    let mut depth = vec![f64::INFINITY; w * h];
    let light = [cam.pos[0], cam.pos[1] + 50.0, cam.pos[2]];
    for t in tris {
        let n = norm(cross(sub(t.v[1], t.v[0]), sub(t.v[2], t.v[0])));
        let p: Vec<(f64, f64, f64)> = t.v.iter().map(|&v| cam.project(v, w, h)).collect();
        // Uranium's projection: near 1, far 500.
        if p.iter().any(|q| q.2 < 1.0) {
            continue;
        }
        let min_x = p.iter().map(|q| q.0).fold(f64::INFINITY, f64::min).floor().max(0.0) as i64;
        let max_x = p.iter().map(|q| q.0).fold(f64::NEG_INFINITY, f64::max).ceil().min(w as f64 - 1.0) as i64;
        let min_y = p.iter().map(|q| q.1).fold(f64::INFINITY, f64::min).floor().max(0.0) as i64;
        let max_y = p.iter().map(|q| q.1).fold(f64::NEG_INFINITY, f64::max).ceil().min(h as f64 - 1.0) as i64;
        if min_x > max_x || min_y > max_y {
            continue;
        }
        let area = (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[2].0 - p[0].0) * (p[1].1 - p[0].1);
        if area.abs() < 1e-12 {
            continue;
        }
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                let w0 = ((p[1].0 - px) * (p[2].1 - py) - (p[2].0 - px) * (p[1].1 - py)) / area;
                let w1 = ((p[2].0 - px) * (p[0].1 - py) - (p[0].0 - px) * (p[2].1 - py)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                // Perspective-correct interpolation of the world position.
                let iz = w0 / p[0].2 + w1 / p[1].2 + w2 / p[2].2;
                let z = 1.0 / iz;
                if z > 500.0 {
                    continue;
                }
                let k = y as usize * w + x as usize;
                if z >= depth[k] {
                    continue;
                }
                depth[k] = z;
                let a = [w0 / p[0].2 * z, w1 / p[1].2 * z, w2 / p[2].2 * z];
                let wp = [
                    a[0] * t.v[0][0] + a[1] * t.v[1][0] + a[2] * t.v[2][0],
                    a[0] * t.v[0][1] + a[1] * t.v[1][1] + a[2] * t.v[2][1],
                    a[0] * t.v[0][2] + a[1] * t.v[1][2] + a[2] * t.v[2][2],
                ];
                let ld = norm(sub(light, wp));
                let ndotl = dot(n, ld).abs().clamp(0.0, 1.0);
                let refl = {
                    let i = [-ld[0], -ld[1], -ld[2]];
                    let d = 2.0 * dot(n, i);
                    [i[0] - d * n[0], i[1] - d * n[1], i[2] - d * n[2]]
                };
                let view = norm(sub(cam.pos, wp));
                let spec = dot(view, refl).clamp(0.0, 1.0).powf(20.0) * 0.6;
                let mut c = [0.1 + ndotl * t.color[0] + spec, 0.1 + ndotl * t.color[1] + spec, 0.1 + ndotl * t.color[2] + spec];
                if wp[1] <= 0.0 {
                    c = [1.0 - c[0], 1.0 - c[1], 1.0 - c[2]];
                }
                color[k] = [c[0].clamp(0.0, 1.0), c[1].clamp(0.0, 1.0), c[2].clamp(0.0, 1.0), 1.0];
            }
        }
    }
    color
}

fn bounds_of(img: &[[f64; 4]], w: usize, h: usize) -> Option<(usize, usize, usize, usize)> {
    let (mut x0, mut x1, mut y0, mut y1) = (usize::MAX, 0, usize::MAX, 0);
    for y in 0..h {
        for x in 0..w {
            if img[y * w + x][3] > 0.0 {
                x0 = x0.min(x);
                x1 = x1.max(x);
                y0 = y0.min(y);
                y1 = y1.max(y);
            }
        }
    }
    if x0 == usize::MAX {
        None
    } else {
        Some((x0, x1, y0, y1))
    }
}

/// Area-averaging resample of a (premultiplied) crop to `ow` × `oh`.
fn resample(img: &[[f64; 4]], w: usize, h: usize, cx: i64, cy: i64, cw: usize, ch: usize, ow: usize, oh: usize) -> Vec<[f64; 4]> {
    let mut out = vec![[0.0; 4]; ow * oh];
    let sx = cw as f64 / ow as f64;
    let sy = ch as f64 / oh as f64;
    for oy in 0..oh {
        for ox in 0..ow {
            let (fx0, fx1) = (ox as f64 * sx, (ox + 1) as f64 * sx);
            let (fy0, fy1) = (oy as f64 * sy, (oy + 1) as f64 * sy);
            let mut acc = [0.0; 4];
            let mut total = 0.0;
            let mut yy = fy0.floor() as i64;
            while (yy as f64) < fy1 {
                let wy = (fy1.min(yy as f64 + 1.0) - fy0.max(yy as f64)).max(0.0);
                let mut xx = fx0.floor() as i64;
                while (xx as f64) < fx1 {
                    let wx = (fx1.min(xx as f64 + 1.0) - fx0.max(xx as f64)).max(0.0);
                    let (px, py) = (cx + xx, cy + yy);
                    let c = if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h { img[py as usize * w + px as usize] } else { [0.0; 4] };
                    let wt = wx * wy;
                    for k in 0..3 {
                        acc[k] += c[k] * c[3] * wt;
                    }
                    acc[3] += c[3] * wt;
                    total += wt;
                    xx += 1;
                }
                yy += 1;
            }
            if total > 0.0 && acc[3] > 0.0 {
                out[oy * ow + ox] = [acc[0] / acc[3], acc[1] / acc[3], acc[2] / acc[3], acc[3] / total];
            }
        }
    }
    out
}

/// PNG (RGBA, 8 bit).
pub fn encode_png(w: usize, h: usize, rgba: &[u8]) -> Vec<u8> {
    fn chunk(out: &mut Vec<u8>, ty: &[u8], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut c = ty.to_vec();
        c.extend_from_slice(data);
        out.extend_from_slice(&c);
        out.extend_from_slice(&crate::zip::crc32(&c).to_be_bytes());
    }
    let mut raw = Vec::with_capacity((w * 4 + 1) * h);
    for y in 0..h {
        raw.push(0);
        raw.extend_from_slice(&rgba[y * w * 4..(y + 1) * w * 4]);
    }
    let mut z = vec![0x78, 0x9c];
    z.extend(crate::zip::deflate(&raw));
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn snapshot_png(tris: &[Tri], width: u32, height: u32) -> Option<Vec<u8>> {
    if tris.is_empty() {
        return None;
    }
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for t in tris {
        for v in &t.v {
            for k in 0..3 {
                lo[k] = lo[k].min(v[k]);
                hi[k] = hi[k].max(v[k]);
            }
        }
    }
    let center = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
    let size = (hi[0] - lo[0]).max(hi[1] - lo[1]).max((hi[2] - lo[2]) * 0.5);
    let mut offset = [-1.0, 1.0, 2.0];
    if size > 0.0 {
        offset = [-size * 1.75, size * 1.75, 2.0 * size * 1.75];
    }
    let pos = [center[0] + offset[0], center[1] + offset[1], center[2] + offset[2]];
    // Render at a fixed square "window" size with 2x supersampling.
    let rw = 1200usize;
    let rh = 1200usize;
    let mut fovy = 30.0;
    let mut tried_once = false;
    loop {
        let cam = Camera::look(pos, center, fovy, rw as f64 / rh as f64);
        let img = render(tris, &cam, rw, rh);
        let (x0, x1, y0, y1) = bounds_of(&img, rw, rh)?;
        let frac = ((x1 - x0) as f64 / rw as f64).max((y1 - y0) as f64 / rh as f64);
        if frac > 0.5 || tried_once {
            let (mut min_x, mut max_x, mut min_y, mut max_y) = (x0 as i64, x1 as i64, y0 as i64, y1 as i64);
            if max_x - min_x >= max_y - min_y {
                let c = (max_y + min_y) as f64 / 2.0;
                let half = (max_x - min_x) as f64 / 2.0;
                min_y = (c - half) as i64;
                max_y = (c + half) as i64;
            } else {
                let c = (max_x + min_x) as f64 / 2.0;
                let half = (max_y - min_y) as f64 / 2.0;
                min_x = (c - half) as i64;
                max_x = (c + half) as i64;
            }
            let cw = (max_x - min_x).max(1) as usize;
            let ch = (max_y - min_y).max(1) as usize;
            let out = resample(&img, rw, rh, min_x, min_y, cw, ch, width as usize, height as usize);
            let mut rgba = Vec::with_capacity(out.len() * 4);
            for p in out {
                for k in 0..4 {
                    rgba.push((p[k] * 255.0).round().clamp(0.0, 255.0) as u8);
                }
            }
            return Some(encode_png(width as usize, height as usize, &rgba));
        }
        tried_once = true;
        fovy *= 0.5;
    }
}

/// A snapshot function over the current scene (models are copied, so it can outlive the borrow).
pub fn snapshot_fn(s: &Session) -> Option<Box<dyn Fn(u32, u32) -> Option<Vec<u8>>>> {
    let tris = collect(s);
    if tris.is_empty() {
        return None;
    }
    Some(Box::new(move |w, h| snapshot_png(&tris, w, h)))
}
