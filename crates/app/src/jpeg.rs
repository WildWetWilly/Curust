//! JPEG decoder reproducing libjpeg-turbo 2.1 (as used by Qt's JPEG plug-in, which Cura's
//! ImageReader reads images with) bit for bit: baseline and progressive Huffman coding, the
//! accurate integer IDCT (`jpeg_idct_islow`), "fancy" chroma upsampling and libjpeg's YCbCr→RGB
//! tables. Output: 1 (grey), 3 (RGB) or 4 (CMYK, as stored) components per pixel.

pub struct JpegImage {
    pub width: usize,
    pub height: usize,
    pub components: usize,
    pub data: Vec<u8>,
}

/// `jpeg_natural_order`, padded with 16 extra entries for corrupt data (`k` may exceed 63).
const NATURAL: [usize; 80] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58,
    59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63, 63, 63, 63, 63, 63, 63, 63, 63, 63, 63, 63, 63, 63, 63, 63, 63,
];

#[derive(Clone, Default)]
struct Huffman {
    /// (code length, code) → symbol, via canonical decoding tables.
    maxcode: [i32; 18],
    valoffset: [i32; 18],
    huffval: Vec<u8>,
}

impl Huffman {
    fn new(bits: &[u8; 17], vals: &[u8]) -> Huffman {
        // jdhuff.c jpeg_make_d_derived_tbl
        let mut huffsize = Vec::new();
        for l in 1..=16 {
            for _ in 0..bits[l] {
                huffsize.push(l as i32);
            }
        }
        let mut huffcode = vec![0i32; huffsize.len()];
        let mut code = 0i32;
        let mut si = huffsize.first().copied().unwrap_or(1);
        let mut p = 0;
        while p < huffsize.len() {
            while p < huffsize.len() && huffsize[p] == si {
                huffcode[p] = code;
                code += 1;
                p += 1;
            }
            code <<= 1;
            si += 1;
        }
        let mut h = Huffman { huffval: vals.to_vec(), ..Default::default() };
        let mut p = 0usize;
        for l in 1..=16 {
            if bits[l] > 0 {
                h.valoffset[l] = p as i32 - huffcode[p];
                p += bits[l] as usize;
                h.maxcode[l] = huffcode[p - 1];
            } else {
                h.maxcode[l] = -1;
            }
        }
        h.maxcode[17] = 0x7fff_ffff;
        h
    }
}

#[derive(Clone)]
struct Component {
    id: u8,
    h: usize,
    v: usize,
    tq: usize,
    /// Blocks per row / column of the coefficient buffer (padded to whole MCUs).
    bw: usize,
    bh: usize,
    /// `width_in_blocks`, `height_in_blocks` (unpadded).
    wib: usize,
    hib: usize,
    /// `downsampled_width`, `downsampled_height`.
    dw: usize,
    dh: usize,
    coefs: Vec<[i16; 64]>,
    dc_tbl: usize,
    ac_tbl: usize,
}

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    acc: u64,
    bits: u32,
    /// A marker was hit; the decoder then reads zero bits (libjpeg's behaviour).
    marker: bool,
}

impl<'a> BitReader<'a> {
    fn fill(&mut self, need: u32) {
        while self.bits < need {
            let mut byte = 0u8;
            if !self.marker && self.pos < self.data.len() {
                let b = self.data[self.pos];
                if b == 0xFF {
                    // Skip fill bytes; 0xFF00 is a stuffed 0xFF; anything else is a marker.
                    let mut q = self.pos + 1;
                    while q < self.data.len() && self.data[q] == 0xFF {
                        q += 1;
                    }
                    if q < self.data.len() && self.data[q] == 0 {
                        byte = 0xFF;
                        self.pos = q + 1;
                    } else {
                        self.marker = true;
                        self.pos = q.saturating_sub(1);
                    }
                } else {
                    byte = b;
                    self.pos += 1;
                }
            }
            self.acc = (self.acc << 8) | byte as u64;
            self.bits += 8;
        }
    }
    fn get(&mut self, n: u32) -> i32 {
        if n == 0 {
            return 0;
        }
        self.fill(n);
        self.bits -= n;
        ((self.acc >> self.bits) & ((1u64 << n) - 1)) as i32
    }
    fn bit(&mut self) -> i32 {
        self.get(1)
    }
    fn decode(&mut self, h: &Huffman) -> i32 {
        let mut code = self.bit();
        let mut l = 1;
        while l <= 16 && code > h.maxcode[l] {
            code = (code << 1) | self.bit();
            l += 1;
        }
        if l > 16 {
            return 0; // corrupt data: libjpeg returns 0
        }
        let idx = (code + h.valoffset[l]) as usize;
        h.huffval.get(idx).copied().unwrap_or(0) as i32
    }
    /// Skip to the next RSTn marker (after a restart interval) and reset.
    fn restart(&mut self) {
        self.acc = 0;
        self.bits = 0;
        // Find the marker.
        let mut p = self.pos;
        while p + 1 < self.data.len() {
            if self.data[p] == 0xFF && (0xD0..=0xD7).contains(&self.data[p + 1]) {
                self.pos = p + 2;
                self.marker = false;
                return;
            }
            if self.data[p] == 0xFF && self.data[p + 1] != 0 && self.data[p + 1] != 0xFF {
                // Another marker: leave it for the caller.
                self.pos = p;
                self.marker = true;
                return;
            }
            p += 1;
        }
        self.pos = self.data.len();
        self.marker = false;
    }
}

/// `HUFF_EXTEND`.
fn extend(r: i32, s: i32) -> i32 {
    if s == 0 {
        0
    } else if r < (1 << (s - 1)) {
        r + ((-1i32) << s) + 1
    } else {
        r
    }
}

fn be16(d: &[u8], i: usize) -> usize {
    ((d[i] as usize) << 8) | d[i + 1] as usize
}

pub fn decode(data: &[u8]) -> Result<JpegImage, String> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return Err("not a JPEG file".into());
    }
    let mut qt: [[u16; 64]; 4] = [[0; 64]; 4];
    let mut dc: Vec<Huffman> = vec![Huffman::default(); 4];
    let mut ac: Vec<Huffman> = vec![Huffman::default(); 4];
    let mut comps: Vec<Component> = Vec::new();
    let (mut width, mut height) = (0usize, 0usize);
    let mut progressive = false;
    let mut restart_interval = 0usize;
    let mut saw_jfif = false;
    let mut adobe: Option<u8> = None;
    let (mut max_h, mut max_v) = (1usize, 1usize);
    let (mut mcux, mut mcuy) = (0usize, 0usize);
    let mut pos = 2;
    let mut frame_seen = false;
    loop {
        // Find the next marker.
        while pos < data.len() && data[pos] != 0xFF {
            pos += 1;
        }
        while pos < data.len() && data[pos] == 0xFF {
            pos += 1;
        }
        if pos >= data.len() {
            break;
        }
        let marker = data[pos];
        pos += 1;
        match marker {
            0xD8 | 0x01 | 0xD0..=0xD7 => continue,
            0xD9 => break,
            _ => {}
        }
        if pos + 2 > data.len() {
            break;
        }
        let len = be16(data, pos);
        if len < 2 || pos + len > data.len() {
            if marker == 0xDA && frame_seen {
                // fall through with what we have
            } else {
                return Err("corrupt JPEG".into());
            }
        }
        let seg = &data[pos + 2..(pos + len).min(data.len())];
        match marker {
            0xE0 => {
                if seg.len() >= 12 && &seg[..5] == b"JFIF\0" {
                    saw_jfif = true;
                }
            }
            0xEE => {
                // "Adobe", version, flags0, flags1, transform.
                if seg.len() >= 12 && &seg[..5] == b"Adobe" {
                    adobe = Some(seg[11]);
                }
            }
            0xDB => {
                let mut i = 0;
                while i < seg.len() {
                    let pq = seg[i] >> 4;
                    let tq = (seg[i] & 15) as usize & 3;
                    i += 1;
                    for k in 0..64 {
                        let v = if pq == 0 {
                            let v = *seg.get(i).unwrap_or(&0) as u16;
                            i += 1;
                            v
                        } else {
                            let v = ((*seg.get(i).unwrap_or(&0) as u16) << 8) | *seg.get(i + 1).unwrap_or(&0) as u16;
                            i += 2;
                            v
                        };
                        qt[tq][NATURAL[k]] = v;
                    }
                }
            }
            0xC4 => {
                let mut i = 0;
                while i + 17 <= seg.len() {
                    let tc = seg[i] >> 4;
                    let th = (seg[i] & 15) as usize & 3;
                    let mut bits = [0u8; 17];
                    let mut count = 0usize;
                    for l in 1..=16 {
                        bits[l] = seg[i + l];
                        count += bits[l] as usize;
                    }
                    i += 17;
                    let vals = &seg[i..(i + count).min(seg.len())];
                    i += count;
                    let h = Huffman::new(&bits, vals);
                    if tc == 0 {
                        dc[th] = h;
                    } else {
                        ac[th] = h;
                    }
                }
            }
            0xDD => {
                restart_interval = be16(seg, 0);
            }
            0xC0 | 0xC1 | 0xC2 => {
                if seg.len() < 6 {
                    return Err("corrupt JPEG".into());
                }
                if seg[0] != 8 {
                    return Err("unsupported JPEG precision".into());
                }
                progressive = marker == 0xC2;
                height = be16(seg, 1);
                width = be16(seg, 3);
                let n = seg[5] as usize;
                if width == 0 || height == 0 || n == 0 || seg.len() < 6 + 3 * n {
                    return Err("corrupt JPEG".into());
                }
                comps.clear();
                for c in 0..n {
                    let b = &seg[6 + 3 * c..9 + 3 * c];
                    let (h, v) = ((b[1] >> 4) as usize, (b[1] & 15) as usize);
                    if !(1..=4).contains(&h) || !(1..=4).contains(&v) {
                        return Err("bad sampling factors".into());
                    }
                    comps.push(Component { id: b[0], h, v, tq: (b[2] & 3) as usize, bw: 0, bh: 0, wib: 0, hib: 0, dw: 0, dh: 0, coefs: Vec::new(), dc_tbl: 0, ac_tbl: 0 });
                }
                max_h = comps.iter().map(|c| c.h).max().unwrap();
                max_v = comps.iter().map(|c| c.v).max().unwrap();
                mcux = width.div_ceil(8 * max_h);
                mcuy = height.div_ceil(8 * max_v);
                for c in comps.iter_mut() {
                    c.dw = (width * c.h).div_ceil(max_h);
                    c.dh = (height * c.v).div_ceil(max_v);
                    c.wib = (width * c.h).div_ceil(max_h * 8);
                    c.hib = (height * c.v).div_ceil(max_v * 8);
                    c.bw = mcux * c.h;
                    c.bh = mcuy * c.v;
                    c.coefs = vec![[0i16; 64]; c.bw * c.bh];
                }
                frame_seen = true;
            }
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => return Err("unsupported JPEG process".into()),
            0xDA => {
                if !frame_seen {
                    return Err("corrupt JPEG".into());
                }
                let ns = seg[0] as usize;
                let mut scomp = Vec::new();
                for i in 0..ns {
                    let id = seg[1 + 2 * i];
                    let t = seg[2 + 2 * i];
                    let ci = comps.iter().position(|c| c.id == id).ok_or("bad scan component")?;
                    comps[ci].dc_tbl = (t >> 4) as usize & 3;
                    comps[ci].ac_tbl = (t & 15) as usize & 3;
                    scomp.push(ci);
                }
                let p = 1 + 2 * ns;
                let (ss, se, ah, al) = (seg[p] as usize, seg[p + 1] as usize, (seg[p + 2] >> 4) as i32, (seg[p + 2] & 15) as i32);
                let start = pos + len;
                let mut br = BitReader { data, pos: start, acc: 0, bits: 0, marker: false };
                decode_scan(&mut br, &mut comps, &scomp, &dc, &ac, progressive, ss, se.min(63), ah, al, restart_interval, mcux, mcuy);
                // Continue after the entropy-coded data.
                pos = br.pos;
                continue;
            }
            _ => {}
        }
        pos += len;
    }
    if !frame_seen {
        return Err("corrupt JPEG".into());
    }

    // IDCT into component planes.
    let mut planes: Vec<Vec<u8>> = Vec::new();
    for c in &comps {
        let pw = c.bw * 8;
        let mut plane = vec![0u8; pw * c.bh * 8];
        let q = &qt[c.tq];
        for by in 0..c.bh {
            for bx in 0..c.bw {
                let mut out = [0u8; 64];
                idct_islow(&c.coefs[by * c.bw + bx], q, &mut out);
                for r in 0..8 {
                    let o = (by * 8 + r) * pw + bx * 8;
                    plane[o..o + 8].copy_from_slice(&out[r * 8..r * 8 + 8]);
                }
            }
        }
        planes.push(plane);
    }

    // Upsample to full resolution.
    let full: Vec<Vec<u8>> = comps.iter().zip(planes.iter()).map(|(c, p)| upsample(c, p, c.bw * 8, width, height, max_h, max_v)).collect();

    // Colour conversion (jdcolor.c).
    let n = comps.len();
    let (components, data_out) = match n {
        1 => (1, full[0].clone()),
        3 => {
            let rgb = if saw_jfif {
                false
            } else if let Some(t) = adobe {
                t == 0
            } else {
                comps[0].id == 82 && comps[1].id == 71 && comps[2].id == 66
            };
            let mut out = vec![0u8; width * height * 3];
            if rgb {
                for i in 0..width * height {
                    out[3 * i] = full[0][i];
                    out[3 * i + 1] = full[1][i];
                    out[3 * i + 2] = full[2][i];
                }
            } else {
                let t = YccTables::new();
                for i in 0..width * height {
                    let (y, cb, cr) = (full[0][i] as i32, full[1][i] as usize, full[2][i] as usize);
                    out[3 * i] = clamp(y + t.cr_r[cr]);
                    out[3 * i + 1] = clamp(y + ((t.cb_g[cb] + t.cr_g[cr]) >> 16) as i32);
                    out[3 * i + 2] = clamp(y + t.cb_b[cb]);
                }
            }
            (3, out)
        }
        4 => {
            let ycck = adobe.map(|t| t != 0).unwrap_or(false);
            let mut out = vec![0u8; width * height * 4];
            let t = YccTables::new();
            for i in 0..width * height {
                if ycck {
                    let (y, cb, cr) = (full[0][i] as i32, full[1][i] as usize, full[2][i] as usize);
                    out[4 * i] = clamp(255 - (y + t.cr_r[cr]));
                    out[4 * i + 1] = clamp(255 - (y + ((t.cb_g[cb] + t.cr_g[cr]) >> 16) as i32));
                    out[4 * i + 2] = clamp(255 - (y + t.cb_b[cb]));
                } else {
                    out[4 * i] = full[0][i];
                    out[4 * i + 1] = full[1][i];
                    out[4 * i + 2] = full[2][i];
                }
                out[4 * i + 3] = full[3][i];
            }
            (4, out)
        }
        _ => return Err("unsupported JPEG colour space".into()),
    };
    Ok(JpegImage { width, height, components, data: data_out })
}

fn clamp(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

struct YccTables {
    cr_r: [i32; 256],
    cb_b: [i32; 256],
    cr_g: [i64; 256],
    cb_g: [i64; 256],
}

impl YccTables {
    /// `build_ycc_rgb_table`.
    fn new() -> YccTables {
        const ONE_HALF: i64 = 1 << 15;
        let fix = |x: f64| (x * 65536.0 + 0.5) as i64;
        let mut t = YccTables { cr_r: [0; 256], cb_b: [0; 256], cr_g: [0; 256], cb_g: [0; 256] };
        for i in 0..256 {
            let x = i as i64 - 128;
            t.cr_r[i] = ((fix(1.40200) * x + ONE_HALF) >> 16) as i32;
            t.cb_b[i] = ((fix(1.77200) * x + ONE_HALF) >> 16) as i32;
            t.cr_g[i] = -fix(0.71414) * x;
            t.cb_g[i] = -fix(0.34414) * x + ONE_HALF;
        }
        t
    }
}

#[allow(clippy::too_many_arguments)]
fn decode_scan(
    br: &mut BitReader,
    comps: &mut [Component],
    scomp: &[usize],
    dc: &[Huffman],
    ac: &[Huffman],
    progressive: bool,
    ss: usize,
    se: usize,
    ah: i32,
    al: i32,
    restart_interval: usize,
    mcux: usize,
    mcuy: usize,
) {
    let mut pred = vec![0i32; comps.len()];
    let mut eobrun: u32 = 0;
    // MCU layout: a single-component scan is not interleaved.
    let (mcus_x, mcus_y) = if scomp.len() == 1 { (comps[scomp[0]].wib, comps[scomp[0]].hib) } else { (mcux, mcuy) };
    let total = mcus_x * mcus_y;
    let mut restarts_left = restart_interval;
    for m in 0..total {
        if restart_interval > 0 {
            if restarts_left == 0 {
                br.restart();
                pred.iter_mut().for_each(|p| *p = 0);
                eobrun = 0;
                restarts_left = restart_interval;
            }
            restarts_left -= 1;
        }
        let (mx, my) = (m % mcus_x, m / mcus_x);
        for &ci in scomp {
            let blocks: Vec<(usize, usize)> = if scomp.len() == 1 {
                vec![(mx, my)]
            } else {
                let c = &comps[ci];
                let mut b = Vec::with_capacity(c.h * c.v);
                for v in 0..c.v {
                    for h in 0..c.h {
                        b.push((mx * c.h + h, my * c.v + v));
                    }
                }
                b
            };
            for (bx, by) in blocks {
                let c = &mut comps[ci];
                let bwidth = c.bw;
                let block = &mut c.coefs[by * bwidth + bx];
                let (dct, act) = (&dc[c.dc_tbl], &ac[c.ac_tbl]);
                if !progressive {
                    // jdhuff.c decode_mcu
                    let s = br.decode(dct);
                    let r = br.get(s as u32);
                    pred[ci] += extend(r, s);
                    block[0] = pred[ci] as i16;
                    let mut k = 1;
                    while k < 64 {
                        let rs = br.decode(act);
                        let (r, s) = (rs >> 4, rs & 15);
                        if s != 0 {
                            k += r as usize;
                            let v = br.get(s as u32);
                            block[NATURAL[k.min(79)]] = extend(v, s) as i16;
                        } else {
                            if r != 15 {
                                break;
                            }
                            k += 15;
                        }
                        k += 1;
                    }
                } else if ss == 0 {
                    // DC scans
                    if ah == 0 {
                        let s = br.decode(dct);
                        let r = br.get(s as u32);
                        pred[ci] += extend(r, s);
                        block[0] = (pred[ci] << al) as i16;
                    } else if br.bit() != 0 {
                        block[0] |= (1i32 << al) as i16;
                    }
                } else if ah == 0 {
                    // AC first
                    if eobrun > 0 {
                        eobrun -= 1;
                        continue;
                    }
                    let mut k = ss;
                    while k <= se {
                        let rs = br.decode(act);
                        let (r, s) = (rs >> 4, rs & 15);
                        if s != 0 {
                            k += r as usize;
                            let v = br.get(s as u32);
                            block[NATURAL[k.min(79)]] = (extend(v, s) << al) as i16;
                        } else if r == 15 {
                            k += 15;
                        } else {
                            eobrun = 1 << r;
                            if r != 0 {
                                eobrun += br.get(r as u32) as u32;
                            }
                            eobrun -= 1;
                            break;
                        }
                        k += 1;
                    }
                } else {
                    // AC refinement (jdphuff.c decode_mcu_AC_refine)
                    let p1: i32 = 1 << al;
                    let m1: i32 = -1 << al;
                    let mut k = ss;
                    if eobrun == 0 {
                        while k <= se {
                            let rs = br.decode(act);
                            let mut r = rs >> 4;
                            let s0 = rs & 15;
                            let mut s = 0i32;
                            if s0 != 0 {
                                s = if br.bit() != 0 { p1 } else { m1 };
                            } else if r != 15 {
                                eobrun = 1 << r;
                                if r != 0 {
                                    eobrun += br.get(r as u32) as u32;
                                }
                                break;
                            }
                            // Advance over nonzero coefficients (appending correction bits) and r
                            // zero coefficients.
                            loop {
                                let z = NATURAL[k.min(79)];
                                let coef = block[z] as i32;
                                if coef != 0 {
                                    if br.bit() != 0 && (coef & p1) == 0 {
                                        block[z] = if coef >= 0 { (coef + p1) as i16 } else { (coef + m1) as i16 };
                                    }
                                } else {
                                    r -= 1;
                                    if r < 0 {
                                        break;
                                    }
                                }
                                k += 1;
                                if k > se {
                                    break;
                                }
                            }
                            if s != 0 {
                                block[NATURAL[k.min(79)]] = s as i16;
                            }
                            k += 1;
                        }
                    }
                    if eobrun > 0 {
                        while k <= se {
                            let z = NATURAL[k];
                            let coef = block[z] as i32;
                            if coef != 0 && br.bit() != 0 && (coef & p1) == 0 {
                                block[z] = if coef >= 0 { (coef + p1) as i16 } else { (coef + m1) as i16 };
                            }
                            k += 1;
                        }
                        eobrun -= 1;
                    }
                }
            }
        }
    }
    // Skip to the end of the entropy-coded segment.
    while br.pos + 1 < br.data.len() {
        if br.data[br.pos] == 0xFF && br.data[br.pos + 1] != 0 && !(0xD0..=0xD7).contains(&br.data[br.pos + 1]) && br.data[br.pos + 1] != 0xFF {
            break;
        }
        br.pos += 1;
    }
}

/// `jpeg_idct_islow`, with libjpeg's range limiting.
fn idct_islow(coef: &[i16; 64], q: &[u16; 64], out: &mut [u8; 64]) {
    const CONST_BITS: i32 = 13;
    const PASS1_BITS: i32 = 2;
    const F0_298: i64 = 2446;
    const F0_390: i64 = 3196;
    const F0_541: i64 = 4433;
    const F0_765: i64 = 6270;
    const F0_899: i64 = 7373;
    const F1_175: i64 = 9633;
    const F1_501: i64 = 12299;
    const F1_847: i64 = 15137;
    const F1_961: i64 = 16069;
    const F2_053: i64 = 16819;
    const F2_562: i64 = 20995;
    const F3_072: i64 = 25172;
    let descale = |x: i64, n: i32| (x + (1i64 << (n - 1))) >> n;
    let range = |x: i64| -> u8 {
        let k = (x as i32) & 1023;
        let v = if k >= 512 { k - 1024 } else { k };
        (v + 128).clamp(0, 255) as u8
    };
    let mut ws = [0i32; 64];
    let dq = |i: usize| coef[i] as i64 * q[i] as i64;
    for c in 0..8 {
        if (1..8).all(|r| coef[r * 8 + c] == 0) {
            let dcval = (dq(c) << PASS1_BITS) as i32;
            for r in 0..8 {
                ws[r * 8 + c] = dcval;
            }
            continue;
        }
        let z2 = dq(16 + c);
        let z3 = dq(48 + c);
        let z1 = (z2 + z3) * F0_541;
        let tmp2 = z1 + z3 * -F1_847;
        let tmp3 = z1 + z2 * F0_765;
        let z2 = dq(c);
        let z3 = dq(32 + c);
        let tmp0 = (z2 + z3) << CONST_BITS;
        let tmp1 = (z2 - z3) << CONST_BITS;
        let tmp10 = tmp0 + tmp3;
        let tmp13 = tmp0 - tmp3;
        let tmp11 = tmp1 + tmp2;
        let tmp12 = tmp1 - tmp2;
        let (mut t0, mut t1, mut t2, mut t3) = (dq(56 + c), dq(40 + c), dq(24 + c), dq(8 + c));
        let z1 = t0 + t3;
        let z2 = t1 + t2;
        let z3 = t0 + t2;
        let z4 = t1 + t3;
        let z5 = (z3 + z4) * F1_175;
        t0 *= F0_298;
        t1 *= F2_053;
        t2 *= F3_072;
        t3 *= F1_501;
        let z1 = z1 * -F0_899;
        let z2 = z2 * -F2_562;
        let z3 = z3 * -F1_961 + z5;
        let z4 = z4 * -F0_390 + z5;
        t0 += z1 + z3;
        t1 += z2 + z4;
        t2 += z2 + z3;
        t3 += z1 + z4;
        let n = CONST_BITS - PASS1_BITS;
        ws[c] = descale(tmp10 + t3, n) as i32;
        ws[56 + c] = descale(tmp10 - t3, n) as i32;
        ws[8 + c] = descale(tmp11 + t2, n) as i32;
        ws[48 + c] = descale(tmp11 - t2, n) as i32;
        ws[16 + c] = descale(tmp12 + t1, n) as i32;
        ws[40 + c] = descale(tmp12 - t1, n) as i32;
        ws[24 + c] = descale(tmp13 + t0, n) as i32;
        ws[32 + c] = descale(tmp13 - t0, n) as i32;
    }
    for r in 0..8 {
        let w = &ws[r * 8..r * 8 + 8];
        let o = &mut out[r * 8..r * 8 + 8];
        if w[1..].iter().all(|&v| v == 0) {
            let v = range(descale(w[0] as i64, PASS1_BITS + 3));
            o.iter_mut().for_each(|x| *x = v);
            continue;
        }
        let z2 = w[2] as i64;
        let z3 = w[6] as i64;
        let z1 = (z2 + z3) * F0_541;
        let tmp2 = z1 + z3 * -F1_847;
        let tmp3 = z1 + z2 * F0_765;
        let tmp0 = (w[0] as i64 + w[4] as i64) << CONST_BITS;
        let tmp1 = (w[0] as i64 - w[4] as i64) << CONST_BITS;
        let tmp10 = tmp0 + tmp3;
        let tmp13 = tmp0 - tmp3;
        let tmp11 = tmp1 + tmp2;
        let tmp12 = tmp1 - tmp2;
        let (mut t0, mut t1, mut t2, mut t3) = (w[7] as i64, w[5] as i64, w[3] as i64, w[1] as i64);
        let z1 = t0 + t3;
        let z2 = t1 + t2;
        let z3 = t0 + t2;
        let z4 = t1 + t3;
        let z5 = (z3 + z4) * F1_175;
        t0 *= F0_298;
        t1 *= F2_053;
        t2 *= F3_072;
        t3 *= F1_501;
        let z1 = z1 * -F0_899;
        let z2 = z2 * -F2_562;
        let z3 = z3 * -F1_961 + z5;
        let z4 = z4 * -F0_390 + z5;
        t0 += z1 + z3;
        t1 += z2 + z4;
        t2 += z2 + z3;
        t3 += z1 + z4;
        let n = CONST_BITS + PASS1_BITS + 3;
        o[0] = range(descale(tmp10 + t3, n));
        o[7] = range(descale(tmp10 - t3, n));
        o[1] = range(descale(tmp11 + t2, n));
        o[6] = range(descale(tmp11 - t2, n));
        o[2] = range(descale(tmp12 + t1, n));
        o[5] = range(descale(tmp12 - t1, n));
        o[3] = range(descale(tmp13 + t0, n));
        o[4] = range(descale(tmp13 - t0, n));
    }
}

/// jdsample.c: bring a component to full resolution (`width` × `height`).
fn upsample(c: &Component, plane: &[u8], pw: usize, width: usize, height: usize, max_h: usize, max_v: usize) -> Vec<u8> {
    let (dw, dh) = (c.dw, c.dh);
    // Rows beyond the last real row repeat it (set_bottom_pointers); the row above the first is
    // the first (make_funny_pointers).
    let row = |y: isize| -> &[u8] {
        let y = y.clamp(0, dh as isize - 1) as usize;
        &plane[y * pw..y * pw + pw]
    };
    let mut out = vec![0u8; width * height];
    let put = |out: &mut Vec<u8>, x: usize, y: usize, v: u8| {
        if x < width && y < height {
            out[y * width + x] = v;
        }
    };
    let (hf, vf) = (max_h / c.h, max_v / c.v);
    let h2v1 = c.h * 2 == max_h && c.v == max_v;
    let h2v2 = c.h * 2 == max_h && c.v * 2 == max_v;
    let h1v2 = c.h == max_h && c.v * 2 == max_v;
    if c.h == max_h && c.v == max_v {
        for y in 0..height {
            out[y * width..y * width + width].copy_from_slice(&row(y as isize)[..width]);
        }
    } else if h2v1 && dw > 2 {
        for y in 0..dh.min(height) {
            let r = row(y as isize);
            let mut o = Vec::with_capacity(dw * 2);
            let v0 = r[0] as i32;
            o.push(v0 as u8);
            o.push(((v0 * 3 + r[1] as i32 + 2) >> 2) as u8);
            for x in 1..dw - 1 {
                let v = r[x] as i32 * 3;
                o.push(((v + r[x - 1] as i32 + 1) >> 2) as u8);
                o.push(((v + r[x + 1] as i32 + 2) >> 2) as u8);
            }
            let v = r[dw - 1] as i32;
            o.push(((v * 3 + r[dw - 2] as i32 + 1) >> 2) as u8);
            o.push(v as u8);
            for (x, &v) in o.iter().enumerate() {
                put(&mut out, x, y, v);
            }
        }
    } else if h1v2 {
        for y in 0..dh {
            let r0 = row(y as isize);
            for (k, (other, bias)) in [(row(y as isize - 1), 1), (row(y as isize + 1), 2)].into_iter().enumerate() {
                for x in 0..dw {
                    let s = r0[x] as i32 * 3 + other[x] as i32;
                    put(&mut out, x, 2 * y + k, ((s + bias) >> 2) as u8);
                }
            }
        }
    } else if h2v2 && dw > 2 {
        for y in 0..dh {
            let r0 = row(y as isize);
            for (k, other) in [row(y as isize - 1), row(y as isize + 1)].into_iter().enumerate() {
                let col = |x: usize| r0[x] as i32 * 3 + other[x] as i32;
                let oy = 2 * y + k;
                let mut this = col(0);
                let mut next = col(1);
                put(&mut out, 0, oy, ((this * 4 + 8) >> 4) as u8);
                put(&mut out, 1, oy, ((this * 3 + next + 7) >> 4) as u8);
                let mut last = this;
                this = next;
                for x in 2..dw {
                    next = col(x);
                    put(&mut out, 2 * (x - 1), oy, ((this * 3 + last + 8) >> 4) as u8);
                    put(&mut out, 2 * (x - 1) + 1, oy, ((this * 3 + next + 7) >> 4) as u8);
                    last = this;
                    this = next;
                }
                put(&mut out, 2 * (dw - 1), oy, ((this * 3 + last + 8) >> 4) as u8);
                put(&mut out, 2 * (dw - 1) + 1, oy, ((this * 4 + 7) >> 4) as u8);
            }
        }
    } else {
        // h2v1 / h2v2 without fancy upsampling, and other integral factors: replicate.
        for y in 0..height {
            let r = row((y / vf) as isize);
            for x in 0..width {
                out[y * width + x] = r[x / hf];
            }
        }
    }
    out
}
