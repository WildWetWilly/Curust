//! What Qt 6.6's `QImage(fileName)` gives Cura's ImageReader: the image format Qt chooses for a
//! PNG / JPEG / BMP / GIF file, the colours `QImage::pixel` returns, `hasAlphaChannel`, and
//! `QImage::scaled(w, h)` with the default fast (nearest neighbour) transformation.

use curust_engine::utils::image::png;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Mono,
    Indexed8,
    Rgb32,
    Argb32,
    Grayscale8,
    Grayscale16,
    Rgbx64,
    Rgba64,
}

impl Format {
    /// `QImage::Format` enum value.
    pub fn qt_value(self) -> i32 {
        match self {
            Format::Mono => 1,
            Format::Indexed8 => 3,
            Format::Rgb32 => 4,
            Format::Argb32 => 5,
            Format::Grayscale8 => 24,
            Format::Rgbx64 => 25,
            Format::Rgba64 => 26,
            Format::Grayscale16 => 28,
        }
    }
}

#[derive(Clone, Debug)]
pub struct QImage {
    pub width: usize,
    pub height: usize,
    pub format: Format,
    /// Indexed formats: colour indices; other formats: unused.
    pub indices: Vec<u8>,
    pub color_table: Vec<u32>,
    /// Non-indexed formats: `pixel(x, y)` per pixel.
    pub pixels: Vec<u32>,
}

/// `QRgba64::div_257`.
fn div_257(x: u16) -> u32 {
    let x = x as u32 + 128;
    (x - (x >> 8)) >> 8
}

/// Premultiply in 16-bit precision (truncating, as Qt's raster engine does) and unpremultiply
/// again (`QRgba64::unpremultiplied`), back to 8 bits per channel.
fn premultiply_roundtrip(v: u32) -> u32 {
    let a = v >> 24;
    if a == 0 {
        return 0;
    }
    if a == 255 {
        return v;
    }
    let a16 = a as u64 * 257;
    let fa = (0xffff_0000_8000u64 + a16 / 2) / a16;
    let ch = |c: u32| -> u32 {
        let c16 = c as u64 * 257;
        let m = c16 * a16;
        let p = (m + (m >> 16)) >> 16;
        let u = ((p * fa + 0x8000_0000) >> 32).min(65535);
        div_257(u as u16)
    };
    qrgba(ch((v >> 16) & 255), ch((v >> 8) & 255), ch(v & 255), a)
}

fn qrgba(r: u32, g: u32, b: u32, a: u32) -> u32 {
    (a << 24) | (r << 16) | (g << 8) | b
}

impl QImage {
    fn direct(width: usize, height: usize, format: Format, pixels: Vec<u32>) -> QImage {
        QImage { width, height, format, indices: Vec::new(), color_table: Vec::new(), pixels }
    }

    fn indexed(width: usize, height: usize, format: Format, indices: Vec<u8>, color_table: Vec<u32>) -> QImage {
        QImage { width, height, format, indices, color_table, pixels: Vec::new() }
    }

    pub fn pixel(&self, x: usize, y: usize) -> u32 {
        match self.format {
            Format::Mono | Format::Indexed8 => {
                let i = self.indices[y * self.width + x] as usize;
                self.color_table.get(i).copied().unwrap_or(0)
            }
            _ => self.pixels[y * self.width + x],
        }
    }

    pub fn has_alpha_channel(&self) -> bool {
        match self.format {
            Format::Argb32 | Format::Rgba64 => true,
            Format::Mono | Format::Indexed8 => self.color_table.iter().any(|c| c >> 24 != 0xff),
            _ => false,
        }
    }

    /// `QImage::scaled(w, h, Qt::IgnoreAspectRatio, Qt::FastTransformation)`.
    pub fn scaled(&self, w: usize, h: usize) -> QImage {
        let (w, h) = (w.max(1), h.max(1));
        if w == self.width && h == self.height {
            return self.clone();
        }
        match self.format {
            Format::Mono | Format::Indexed8 => {
                // qt_xForm_helper with the inverted matrix in 20.12 fixed point.
                let fixed = |src: usize, dst: usize| -> Vec<Option<usize>> {
                    let m = dst as f64 / src as f64;
                    let inv = 1.0 / m;
                    let step = (inv * 4096.0) as i64;
                    let start = step / 2;
                    (0..dst)
                        .map(|i| {
                            let t = (start + i as i64 * step) as u32;
                            if (t as u64) < ((src as u64) << 12) {
                                Some((t >> 12) as usize)
                            } else {
                                None
                            }
                        })
                        .collect()
                };
                let xs = fixed(self.width, w);
                let ys = fixed(self.height, h);
                let mut table = self.color_table.clone();
                let fill = if table.len() < 256 && self.format == Format::Indexed8 {
                    table.push(0);
                    (table.len() - 1) as u8
                } else {
                    0
                };
                let mut idx = vec![fill; w * h];
                for (y, sy) in ys.iter().enumerate() {
                    for (x, sx) in xs.iter().enumerate() {
                        if let (Some(sx), Some(sy)) = (sx, sy) {
                            idx[y * w + x] = self.indices[sy * self.width + sx];
                        }
                    }
                }
                QImage::indexed(w, h, self.format, idx, table)
            }
            _ => {
                // QPainter::drawImage with a scaling transform: qt_scale_image_32bit's 16.16
                // fixed-point nearest neighbour.
                let map = |src: usize, dst: usize| -> Vec<usize> {
                    let m = dst as f64 / src as f64;
                    let target = src as f64 * m;
                    let sx = src as f64 / target;
                    let ix = (65536.0 * sx) as i32 as u32;
                    let base = ((0.5 * sx * 65536.0).ceil() as i64 - 1) as u32;
                    (0..dst).map(|i| ((base.wrapping_add((i as u32).wrapping_mul(ix))) >> 16) as usize).map(|s| s.min(src - 1)).collect()
                };
                let xs = map(self.width, w);
                let ys = map(self.height, h);
                let mut px = vec![0u32; w * h];
                // Formats with alpha are drawn premultiplied (16 bits per channel) and stored back
                // unpremultiplied: fully transparent pixels become 0, and alpha 1 loses a little.
                let alpha = matches!(self.format, Format::Argb32 | Format::Rgba64);
                for (y, &sy) in ys.iter().enumerate() {
                    for (x, &sx) in xs.iter().enumerate() {
                        let v = self.pixels[sy * self.width + sx];
                        px[y * w + x] = if alpha { premultiply_roundtrip(v) } else { v };
                    }
                }
                QImage::direct(w, h, self.format, px)
            }
        }
    }
}

/// Load an image the way `QImage(fileName)` does (format by content). `None` = `isNull()`.
pub fn load(data: &[u8]) -> Option<QImage> {
    if data.starts_with(&[0x89, b'P', b'N', b'G']) {
        load_png(data)
    } else if data.starts_with(&[0xFF, 0xD8]) {
        load_jpeg(data)
    } else if data.starts_with(b"GIF8") {
        load_gif(data)
    } else if data.starts_with(b"BM") {
        load_bmp(data)
    } else {
        None
    }
}

/// qpnghandler.cpp `setup_qt` and the libpng transformations it requests.
fn load_png(data: &[u8]) -> Option<QImage> {
    let raw = png::decode_raw(data).ok()?;
    let (w, h, depth, color) = (raw.w, raw.h, raw.depth, raw.color);
    let n = raw.img_n;
    let s = &raw.samples;
    let px = |i: usize, c: usize| s[i * n + c];
    match color {
        0 => {
            if depth == 1 && !raw.has_trns {
                // png_set_invert_mono: index 1 = black.
                let idx: Vec<u8> = (0..w * h).map(|i| if px(i, 0) == 1 { 0 } else { 1 }).collect();
                return Some(QImage::indexed(w, h, Format::Mono, idx, vec![qrgba(255, 255, 255, 255), qrgba(0, 0, 0, 255)]));
            }
            if depth == 1 {
                let idx: Vec<u8> = (0..w * h).map(|i| if px(i, 0) == 1 { 0 } else { 1 }).collect();
                let mut table = vec![qrgba(255, 255, 255, 255), qrgba(0, 0, 0, 255)];
                if raw.trns_gray == 0 {
                    table[1] = 0;
                } else if raw.trns_gray == 1 {
                    table[0] = qrgba(255, 255, 255, 0);
                }
                return Some(QImage::indexed(w, h, Format::Mono, idx, table));
            }
            if depth == 16 && !raw.has_trns {
                let p = (0..w * h).map(|i| {
                    let g = div_257(px(i, 0));
                    qrgba(g, g, g, 255)
                });
                return Some(QImage::direct(w, h, Format::Grayscale16, p.collect()));
            }
            if depth == 16 {
                // Expanded with the tRNS colour as transparent, RGBA64.
                let p = (0..w * h).map(|i| {
                    let g = div_257(px(i, 0));
                    let a = if px(i, 0) == raw.trns_gray { 0 } else { 255 };
                    qrgba(g, g, g, a)
                });
                return Some(QImage::direct(w, h, Format::Rgba64, p.collect()));
            }
            if depth == 8 && !raw.has_trns {
                let p = (0..w * h).map(|i| {
                    let g = px(i, 0) as u32;
                    qrgba(g, g, g, 255)
                });
                return Some(QImage::direct(w, h, Format::Grayscale8, p.collect()));
            }
            let ncols = if depth < 8 { 1usize << depth } else { 256 };
            let mut table: Vec<u32> = (0..ncols).map(|i| {
                let c = (i * 255 / (ncols - 1)) as u32;
                qrgba(c, c, c, 255)
            }).collect();
            if raw.has_trns && (raw.trns_gray as usize) < ncols {
                table[raw.trns_gray as usize] = 0;
            }
            let idx: Vec<u8> = (0..w * h).map(|i| px(i, 0) as u8).collect();
            Some(QImage::indexed(w, h, Format::Indexed8, idx, table))
        }
        3 => {
            if raw.palette.is_empty() || raw.palette.len() > 256 {
                return None;
            }
            let format = if depth == 1 { Format::Mono } else { Format::Indexed8 };
            let table: Vec<u32> = raw.palette.iter().map(|c| qrgba(c[0] as u32, c[1] as u32, c[2] as u32, c[3] as u32)).collect();
            let table = if format == Format::Mono { table.into_iter().chain(std::iter::repeat(0)).take(2).collect() } else { table };
            let idx: Vec<u8> = (0..w * h).map(|i| px(i, 0) as u8).collect();
            Some(QImage::indexed(w, h, format, idx, table))
        }
        _ if depth == 16 => {
            // RGB(A) / grey+alpha, 16 bits per channel.
            let has_alpha = color == 4 || color == 6;
            let format = if has_alpha || raw.has_trns { Format::Rgba64 } else { Format::Rgbx64 };
            let p = (0..w * h).map(|i| {
                let (r, g, b, a) = match color {
                    2 => (px(i, 0), px(i, 1), px(i, 2), 0xffff),
                    4 => (px(i, 0), px(i, 0), px(i, 0), px(i, 1)),
                    _ => (px(i, 0), px(i, 1), px(i, 2), px(i, 3)),
                };
                qrgba(div_257(r), div_257(g), div_257(b), div_257(a))
            });
            Some(QImage::direct(w, h, format, p.collect()))
        }
        _ => {
            // 8-bit RGB, RGBA, grey+alpha (and expanded tRNS): ARGB32 or RGB32.
            let has_alpha = color == 4 || color == 6 || raw.has_trns;
            let format = if has_alpha { Format::Argb32 } else { Format::Rgb32 };
            let p = (0..w * h).map(|i| {
                let (r, g, b, a) = match color {
                    2 => {
                        let (r, g, b) = (px(i, 0), px(i, 1), px(i, 2));
                        let a = if raw.has_trns && [r, g, b] == raw.trns_rgb { 0 } else { 255 };
                        (r, g, b, a)
                    }
                    4 => (px(i, 0), px(i, 0), px(i, 0), px(i, 1)),
                    _ => (px(i, 0), px(i, 1), px(i, 2), px(i, 3)),
                };
                qrgba(r as u32, g as u32, b as u32, a as u32)
            });
            Some(QImage::direct(w, h, format, p.collect()))
        }
    }
}

/// qjpeghandler.cpp: Grayscale8 for one component, RGB32 otherwise (CMYK multiplied by K).
fn load_jpeg(data: &[u8]) -> Option<QImage> {
    let j = crate::jpeg::decode(data).ok()?;
    let (w, h) = (j.width, j.height);
    let d = &j.data;
    match j.components {
        1 => Some(QImage::direct(w, h, Format::Grayscale8, d.iter().map(|&g| qrgba(g as u32, g as u32, g as u32, 255)).collect())),
        3 => Some(QImage::direct(w, h, Format::Rgb32, (0..w * h).map(|i| qrgba(d[3 * i] as u32, d[3 * i + 1] as u32, d[3 * i + 2] as u32, 255)).collect())),
        4 => Some(QImage::direct(
            w,
            h,
            Format::Rgb32,
            (0..w * h)
                .map(|i| {
                    let k = d[4 * i + 3] as u32;
                    qrgba(k * d[4 * i] as u32 / 255, k * d[4 * i + 1] as u32 / 255, k * d[4 * i + 2] as u32 / 255, 255)
                })
                .collect(),
        )),
        _ => None,
    }
}

fn le16(d: &[u8], i: usize) -> u32 {
    d[i] as u32 | (d[i + 1] as u32) << 8
}
fn le32(d: &[u8], i: usize) -> u32 {
    le16(d, i) | le16(d, i + 2) << 16
}

/// qbmphandler.cpp `read_dib_body`.
fn load_bmp(d: &[u8]) -> Option<QImage> {
    if d.len() < 26 {
        return None;
    }
    let datapos = le32(d, 10) as usize;
    let start = 14;
    let bisize = le32(d, start) as usize;
    let (w, hh, nbits, comp, clr_used) = if bisize == 12 {
        (le16(d, start + 4) as i32, le16(d, start + 6) as i16 as i32, le16(d, start + 10), 0, 0)
    } else {
        if d.len() < start + 40 {
            return None;
        }
        (le32(d, start + 4) as i32, le32(d, start + 8) as i32, le16(d, start + 14), le32(d, start + 16), le32(d, start + 32))
    };
    if w <= 0 || hh == 0 {
        return None;
    }
    let w = w as usize;
    let h = hh.unsigned_abs() as usize;
    let bitfields = comp == 3 || comp == 6;
    let (mut rm, mut gm, mut bm, mut am) = (0u32, 0u32, 0u32, 0u32);
    let mut p = start + bisize;
    if bisize >= 108 {
        rm = le32(d, start + 40);
        gm = le32(d, start + 44);
        bm = le32(d, start + 48);
        am = le32(d, start + 52);
    } else if bitfields && (nbits == 16 || nbits == 32) {
        rm = le32(d, p);
        gm = le32(d, p + 4);
        bm = le32(d, p + 8);
        p += 12;
        if comp == 6 {
            am = le32(d, p);
            p += 4;
        }
    }
    let transp = (bitfields || (comp == 0 && nbits == 32 && am == 0xff000000)) && am != 0;
    let calc_shift = |m: u32| if m == 0 { 0 } else { m.trailing_zeros() };
    let calc_scale = |mut low: u32| {
        let mut r = 8u32;
        while low != 0 && r != 0 {
            r -= 1;
            low >>= 1;
        }
        r
    };
    let apply_scale = |v: u32, scale: u32| -> u32 {
        if scale & 7 == 0 {
            return v;
        }
        let mut filled = 8 - scale;
        let mut r = v << scale;
        loop {
            r |= r >> filled;
            filled <<= 1;
            if filled >= 8 {
                break;
            }
        }
        r
    };
    let row_order = |i: usize| if hh > 0 { h - 1 - i } else { i };
    match nbits {
        1 | 4 | 8 => {
            let ncols = if clr_used != 0 { clr_used as usize } else { 1 << nbits };
            if ncols < 1 || ncols > 256 {
                return None;
            }
            let entry = if bisize == 12 { 3 } else { 4 };
            let mut table = Vec::with_capacity(ncols);
            for i in 0..ncols {
                let o = p + i * entry;
                if o + 3 > d.len() {
                    return None;
                }
                table.push(qrgba(d[o + 2] as u32, d[o + 1] as u32, d[o] as u32, 255));
            }
            let format = if nbits == 1 { Format::Mono } else { Format::Indexed8 };
            let mut idx = vec![0u8; w * h];
            let data = d.get(datapos.max(p + ncols * entry)..).unwrap_or(&[]);
            if (nbits == 8 && comp == 1) || (nbits == 4 && comp == 2) {
                // Run-length encoded (bottom-up).
                let (mut x, mut y, mut i) = (0usize, 0usize, 0usize);
                let get = |i: &mut usize| -> Option<u8> {
                    let v = data.get(*i).copied();
                    *i += 1;
                    v
                };
                while y < h {
                    let Some(b) = get(&mut i) else { break };
                    let row = h - 1 - y;
                    if b == 0 {
                        let Some(e) = get(&mut i) else { break };
                        match e {
                            1 => break,
                            0 => {
                                x = 0;
                                y += 1;
                            }
                            2 => {
                                x += get(&mut i).unwrap_or(0) as usize;
                                y += get(&mut i).unwrap_or(0) as usize;
                                x = x.min(w - 1);
                                y = y.min(h - 1);
                            }
                            n => {
                                let n = (n as usize).min(w.saturating_sub(x));
                                if nbits == 8 {
                                    for _ in 0..n {
                                        let v = get(&mut i).unwrap_or(0);
                                        if x < w {
                                            idx[row * w + x] = v;
                                        }
                                        x += 1;
                                    }
                                    if n & 1 == 1 {
                                        i += 1;
                                    }
                                } else {
                                    let mut k = 0;
                                    while k < n {
                                        let v = get(&mut i).unwrap_or(0);
                                        for nib in [v >> 4, v & 15] {
                                            if k < n && x < w {
                                                idx[row * w + x] = nib;
                                                x += 1;
                                            }
                                            k += 1;
                                        }
                                    }
                                    if (((n & 3) + 1) & 2) == 2 {
                                        i += 1;
                                    }
                                }
                            }
                        }
                    } else {
                        let n = (b as usize).min(w.saturating_sub(x));
                        let v = get(&mut i).unwrap_or(0);
                        for k in 0..n {
                            if x < w {
                                idx[row * w + x] = if nbits == 8 { v } else if k % 2 == 0 { v >> 4 } else { v & 15 };
                            }
                            x += 1;
                        }
                    }
                }
            } else {
                let bpl = (w * nbits as usize).div_ceil(32) * 4;
                for i in 0..h {
                    let row = row_order(i);
                    let line = data.get(i * bpl..(i + 1) * bpl);
                    let Some(line) = line else { break };
                    for x in 0..w {
                        idx[row * w + x] = match nbits {
                            1 => (line[x / 8] >> (7 - x % 8)) & 1,
                            4 => {
                                let b = line[x / 2];
                                if x % 2 == 0 {
                                    b >> 4
                                } else {
                                    b & 15
                                }
                            }
                            _ => line[x],
                        };
                    }
                }
            }
            Some(QImage::indexed(w, h, format, idx, table))
        }
        16 | 24 | 32 => {
            let (rs, gs, bs, as_, rsc, gsc, bsc, asc);
            if bitfields && (nbits == 16 || nbits == 32) {
                rs = calc_shift(rm);
                gs = calc_shift(gm);
                bs = calc_shift(bm);
                as_ = calc_shift(am);
                rsc = calc_scale(rm >> rs);
                gsc = calc_scale(gm >> gs);
                bsc = calc_scale(bm >> bs);
                asc = calc_scale(am >> as_);
            } else if comp == 0 && (nbits == 24 || nbits == 32) {
                bm = 0xff;
                gm = 0xff00;
                rm = 0xff0000;
                bs = 0;
                gs = 8;
                rs = 16;
                bsc = 0;
                gsc = 0;
                rsc = 0;
                as_ = if transp { calc_shift(am) } else { 0 };
                asc = if transp { calc_scale(am >> as_) } else { 0 };
            } else if comp == 0 && nbits == 16 {
                bm = 0x1f;
                gm = 0x3e0;
                rm = 0x7c00;
                bs = 0;
                gs = 5;
                rs = 10;
                bsc = 3;
                gsc = 3;
                rsc = 3;
                as_ = 0;
                asc = 0;
            } else {
                return None;
            }
            let format = if transp { Format::Argb32 } else { Format::Rgb32 };
            let mut px = vec![0u32; w * h];
            let bpl = (w * nbits as usize).div_ceil(32) * 4;
            let data = d.get(datapos.max(p)..).unwrap_or(&[]);
            let bytes = nbits as usize / 8;
            for i in 0..h {
                let row = row_order(i);
                let Some(line) = data.get(i * bpl..(i + 1) * bpl) else { break };
                for x in 0..w {
                    let b = &line[x * bytes..];
                    let mut c = b[0] as u32 | (b[1] as u32) << 8;
                    if nbits > 16 {
                        c |= (b[2] as u32) << 16;
                    }
                    if nbits > 24 {
                        c |= (b[3] as u32) << 24;
                    }
                    let a = if transp { apply_scale((c & am) >> as_, asc) } else { 0xff };
                    px[row * w + x] = qrgba(apply_scale((c & rm) >> rs, rsc) & 0xff, apply_scale((c & gm) >> gs, gsc) & 0xff, apply_scale((c & bm) >> bs, bsc) & 0xff, a & 0xff);
                }
            }
            Some(QImage::direct(w, h, format, px))
        }
        _ => None,
    }
}

/// qgifhandler.cpp: the first frame, RGB32 (ARGB32 when it has a transparent colour).
fn load_gif(d: &[u8]) -> Option<QImage> {
    if d.len() < 13 {
        return None;
    }
    let mut sw = le16(d, 6) as i64;
    let mut sh = le16(d, 8) as i64;
    let flags = d[10];
    let bgindex = d[11] as usize;
    let mut p = 13;
    let gcmap = flags & 0x80 != 0;
    let gncols = if gcmap { 2usize << (flags & 7) } else { 0 };
    let mut global: Vec<u32> = Vec::new();
    if gcmap {
        for i in 0..gncols {
            let o = p + 3 * i;
            global.push(qrgba(*d.get(o)? as u32, *d.get(o + 1)? as u32, *d.get(o + 2)? as u32, 255));
        }
        global.push(0x00ff_ffff);
        p += 3 * gncols;
    }
    let bgcol: i64 = if gcmap { bgindex as i64 } else { -1 };
    let mut trans_index: i64 = -1;
    while p < d.len() {
        match d[p] {
            0x21 => {
                // Extension
                let label = *d.get(p + 1)?;
                p += 2;
                let mut first = true;
                loop {
                    let n = *d.get(p)? as usize;
                    p += 1;
                    if n == 0 {
                        break;
                    }
                    if label == 0xF9 && first && n >= 4 {
                        let packed = d[p];
                        trans_index = if packed & 1 != 0 { d[p + 3] as i64 } else { -1 };
                    }
                    first = false;
                    p += n;
                }
            }
            0x2C => {
                let hold = d.get(p..p + 10)?;
                let left = le16(hold, 1) as i64;
                let top = le16(hold, 3) as i64;
                let width = le16(hold, 5) as i64;
                let height = le16(hold, 7) as i64;
                if sw / 10 > width.max(16384) {
                    sw = -1;
                }
                if sh / 10 > height.max(16384) {
                    sh = -1;
                }
                if sw <= 0 {
                    sw = left + width;
                }
                if sh <= 0 {
                    sh = top + height;
                }
                let (swu, shu) = (sw as usize, sh as usize);
                if swu == 0 || shu == 0 || swu * shu > 1 << 28 {
                    return None;
                }
                let lflags = hold[9];
                p += 10;
                let lcmap = lflags & 0x80 != 0;
                let interlace = lflags & 0x40 != 0;
                let lncols = if lcmap { 2usize << (lflags & 7) } else { 0 };
                let mut local: Vec<u32> = Vec::new();
                if lcmap {
                    for i in 0..lncols {
                        let o = p + 3 * i;
                        local.push(qrgba(*d.get(o)? as u32, *d.get(o + 1)? as u32, *d.get(o + 2)? as u32, 255));
                    }
                    local.push(0x00ff_ffff);
                    p += 3 * lncols;
                }
                let ncols = if lcmap { lncols } else { gncols };
                let map = if lcmap { &local } else { &global };
                let color = |index: usize| -> u32 {
                    if index > ncols {
                        return 0x00ff_ffff;
                    }
                    let col = map.get(index).copied().unwrap_or(0);
                    if index as i64 == trans_index {
                        col & 0x00ff_ffff
                    } else {
                        col
                    }
                };
                let format = if trans_index >= 0 { Format::Argb32 } else { Format::Rgb32 };
                let mut img = vec![0u32; swu * shu];
                if left != 0 || top != 0 || width < sw || height < sh {
                    let fill = if trans_index >= 0 {
                        Some(color(trans_index as usize))
                    } else if bgcol >= 0 {
                        Some(color(bgcol as usize))
                    } else {
                        None
                    };
                    if let Some(f) = fill {
                        img.iter_mut().for_each(|v| *v = f);
                    }
                }
                // LZW data.
                let lzw_size = *d.get(p)? as usize;
                p += 1;
                let mut stream = Vec::new();
                loop {
                    let n = *d.get(p)? as usize;
                    p += 1;
                    if n == 0 {
                        break;
                    }
                    stream.extend_from_slice(d.get(p..p + n)?);
                    p += n;
                }
                let indices = lzw_decode(&stream, lzw_size);
                // Place the pixels (interlaced rows in GIF pass order).
                let rows: Vec<i64> = if interlace {
                    let mut r = Vec::new();
                    for (start, step) in [(0, 8), (4, 8), (2, 4), (1, 2)] {
                        let mut y = start;
                        while y < height {
                            r.push(top + y);
                            y += step;
                        }
                    }
                    r
                } else {
                    (0..height).map(|y| top + y).collect()
                };
                let mut k = 0;
                for &y in &rows {
                    for x in left..left + width {
                        if k >= indices.len() {
                            break;
                        }
                        let index = indices[k];
                        k += 1;
                        if x < sw && y < sh {
                            img[y as usize * swu + x as usize] = color(index as usize);
                        }
                    }
                }
                if interlace {
                    // Qt's progressive display copies rows of the early passes downwards; later
                    // passes overwrite them, so only incomplete data shows a difference.
                }
                let _ = bgcol;
                return Some(QImage::direct(swu, shu, format, img));
            }
            0x3B => return None,
            _ => return None,
        }
    }
    None
}

/// GIF LZW decoding (variable code size, max 12 bits).
fn lzw_decode(data: &[u8], min_size: usize) -> Vec<u8> {
    let min_size = min_size.clamp(1, 11);
    let clear = 1usize << min_size;
    let end = clear + 1;
    let mut out = Vec::new();
    let mut prefix = vec![0u16; 4096];
    let mut suffix = vec![0u8; 4096];
    let mut code_size = min_size + 1;
    let mut next = clear + 2;
    let mut old: Option<usize> = None;
    let mut first = 0u8;
    let (mut acc, mut bits, mut pos) = (0u32, 0u32, 0usize);
    let mut stack = Vec::new();
    loop {
        while bits < code_size as u32 {
            if pos >= data.len() {
                return out;
            }
            acc |= (data[pos] as u32) << bits;
            bits += 8;
            pos += 1;
        }
        let code = (acc & ((1 << code_size) - 1)) as usize;
        acc >>= code_size;
        bits -= code_size as u32;
        if code == clear {
            code_size = min_size + 1;
            next = clear + 2;
            old = None;
            continue;
        }
        if code == end {
            return out;
        }
        let Some(prev) = old else {
            if code < clear {
                out.push(code as u8);
                first = code as u8;
                old = Some(code);
            }
            continue;
        };
        let mut c = code;
        if code >= next {
            stack.push(first);
            c = prev;
        }
        while c >= clear + 2 {
            if c >= 4096 {
                return out;
            }
            stack.push(suffix[c]);
            c = prefix[c] as usize;
        }
        first = c as u8;
        stack.push(first);
        while let Some(v) = stack.pop() {
            out.push(v);
        }
        if next < 4096 {
            prefix[next] = prev as u16;
            suffix[next] = first;
            next += 1;
            if next >= (1 << code_size) && code_size < 12 {
                code_size += 1;
            }
        }
        old = Some(code);
    }
}
