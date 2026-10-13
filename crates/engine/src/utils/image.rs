//! Minimal image loading with the same output conventions as `stb_image`'s `stbi_load(file, &x,
//! &y, &n, 0)`: 8 bits per channel, rows from top to bottom, and the channel count of the file
//! (palette images are expanded to RGB/RGBA, a tRNS chunk adds an alpha channel).
//!
//! Supported: PNG (all bit depths, color types and interlacing), BMP (uncompressed 24/32 bit and
//! 8 bit paletted), binary PGM/PPM (P5/P6) and uncompressed TGA. JPEG is not supported yet.

/// Returns (width, height, channels, data).
pub fn load_image(filename: &str) -> Result<(usize, usize, usize, Vec<u8>), String> {
    let data = std::fs::read(filename).map_err(|e| format!("can't fopen ({e})"))?;
    decode_image(&data)
}

pub fn decode_image(data: &[u8]) -> Result<(usize, usize, usize, Vec<u8>), String> {
    if data.len() >= 8 && data[..8] == [137, 80, 78, 71, 13, 10, 26, 10] {
        return png::decode(data);
    }
    if data.len() >= 2 && &data[..2] == b"BM" {
        return decode_bmp(data);
    }
    if data.len() >= 2 && (data[..2] == *b"P5" || data[..2] == *b"P6") {
        return decode_pnm(data);
    }
    if data.len() >= 3 && data[0] == 0xFF && data[1] == 0xD8 && data[2] == 0xFF {
        return Err("JPEG images are not supported yet".to_string());
    }
    if data.len() >= 18 && (data[2] == 2 || data[2] == 3) && data[1] == 0 {
        return decode_tga(data);
    }
    Err("unknown image type".to_string())
}

fn le16(d: &[u8], i: usize) -> u32 {
    d[i] as u32 | (d[i + 1] as u32) << 8
}
fn le32(d: &[u8], i: usize) -> u32 {
    le16(d, i) | le16(d, i + 2) << 16
}

fn decode_bmp(d: &[u8]) -> Result<(usize, usize, usize, Vec<u8>), String> {
    if d.len() < 54 {
        return Err("corrupt BMP".into());
    }
    let offset = le32(d, 10) as usize;
    let header_size = le32(d, 14);
    let width = le32(d, 18) as i32;
    let height_raw = le32(d, 22) as i32;
    let bpp = le16(d, 28);
    let compression = le32(d, 30);
    if header_size < 40 || (compression != 0 && compression != 3) {
        return Err("unsupported BMP".into());
    }
    let flip = height_raw > 0;
    let (w, h) = (width.unsigned_abs() as usize, height_raw.unsigned_abs() as usize);
    let channels = if bpp == 32 { 4 } else { 3 };
    let mut out = vec![0u8; w * h * channels];
    let row_size = (bpp as usize * w).div_ceil(32) * 4;
    let palette_start = 14 + header_size as usize;
    for row in 0..h {
        let src_row = offset + row * row_size;
        let dst_row = if flip { h - 1 - row } else { row };
        for x in 0..w {
            let dst = (dst_row * w + x) * channels;
            match bpp {
                24 | 32 => {
                    let s = src_row + x * (bpp as usize / 8);
                    if s + 2 >= d.len() {
                        return Err("corrupt BMP".into());
                    }
                    out[dst] = d[s + 2];
                    out[dst + 1] = d[s + 1];
                    out[dst + 2] = d[s];
                    if channels == 4 {
                        out[dst + 3] = d[s + 3];
                    }
                }
                8 => {
                    let idx = d[src_row + x] as usize;
                    let p = palette_start + idx * 4;
                    out[dst] = d[p + 2];
                    out[dst + 1] = d[p + 1];
                    out[dst + 2] = d[p];
                }
                _ => return Err("unsupported BMP bit depth".into()),
            }
        }
    }
    Ok((w, h, channels, out))
}

fn decode_pnm(d: &[u8]) -> Result<(usize, usize, usize, Vec<u8>), String> {
    let channels = if d[1] == b'5' { 1 } else { 3 };
    let mut pos = 2;
    let mut fields = Vec::new();
    while fields.len() < 3 {
        while pos < d.len() && (d[pos].is_ascii_whitespace() || d[pos] == b'#') {
            if d[pos] == b'#' {
                while pos < d.len() && d[pos] != b'\n' {
                    pos += 1;
                }
            } else {
                pos += 1;
            }
        }
        let start = pos;
        while pos < d.len() && d[pos].is_ascii_digit() {
            pos += 1;
        }
        let v: usize = std::str::from_utf8(&d[start..pos]).ok().and_then(|s| s.parse().ok()).ok_or("corrupt PNM")?;
        fields.push(v);
    }
    pos += 1;
    let (w, h, maxv) = (fields[0], fields[1], fields[2]);
    if maxv > 255 {
        return Err("unsupported PNM".into());
    }
    let n = w * h * channels;
    if d.len() < pos + n {
        return Err("corrupt PNM".into());
    }
    Ok((w, h, channels, d[pos..pos + n].to_vec()))
}

fn decode_tga(d: &[u8]) -> Result<(usize, usize, usize, Vec<u8>), String> {
    let id_len = d[0] as usize;
    let image_type = d[2];
    let w = le16(d, 12) as usize;
    let h = le16(d, 14) as usize;
    let bpp = d[16];
    let descriptor = d[17];
    if image_type != 2 && image_type != 3 {
        return Err("unsupported TGA".into());
    }
    let channels = match bpp {
        8 => 1,
        24 => 3,
        32 => 4,
        _ => return Err("unsupported TGA".into()),
    };
    let top_down = descriptor & 0x20 != 0;
    let start = 18 + id_len;
    let mut out = vec![0u8; w * h * channels];
    for row in 0..h {
        let dst_row = if top_down { row } else { h - 1 - row };
        for x in 0..w {
            let s = start + (row * w + x) * channels;
            let dst = (dst_row * w + x) * channels;
            if s + channels > d.len() {
                return Err("corrupt TGA".into());
            }
            if channels == 1 {
                out[dst] = d[s];
            } else {
                out[dst] = d[s + 2];
                out[dst + 1] = d[s + 1];
                out[dst + 2] = d[s];
                if channels == 4 {
                    out[dst + 3] = d[s + 3];
                }
            }
        }
    }
    Ok((w, h, channels, out))
}

/// zlib / DEFLATE decompression.
pub mod inflate {
    struct BitReader<'a> {
        data: &'a [u8],
        pos: usize,
        bit: u32,
        nbits: u32,
    }

    impl<'a> BitReader<'a> {
        fn bits(&mut self, n: u32) -> Result<u32, String> {
            while self.nbits < n {
                if self.pos >= self.data.len() {
                    return Err("unexpected end of compressed data".into());
                }
                self.bit |= (self.data[self.pos] as u32) << self.nbits;
                self.pos += 1;
                self.nbits += 8;
            }
            let v = if n == 32 { self.bit } else { self.bit & ((1u32 << n) - 1) };
            self.bit = if n == 32 { 0 } else { self.bit >> n };
            self.nbits -= n;
            Ok(v)
        }
        fn align(&mut self) {
            self.bit = 0;
            self.nbits = 0;
        }
    }

    struct Huffman {
        counts: [u16; 16],
        symbols: Vec<u16>,
    }

    impl Huffman {
        fn new(lengths: &[u8]) -> Self {
            let mut counts = [0u16; 16];
            for &l in lengths {
                counts[l as usize] += 1;
            }
            counts[0] = 0;
            let mut offs = [0u16; 16];
            for i in 1..16 {
                offs[i] = offs[i - 1] + counts[i - 1];
            }
            let mut symbols = vec![0u16; lengths.len()];
            for (sym, &l) in lengths.iter().enumerate() {
                if l != 0 {
                    symbols[offs[l as usize] as usize] = sym as u16;
                    offs[l as usize] += 1;
                }
            }
            Huffman { counts, symbols }
        }

        fn decode(&self, br: &mut BitReader) -> Result<u16, String> {
            let mut code: i32 = 0;
            let mut first: i32 = 0;
            let mut index: i32 = 0;
            for len in 1..16 {
                code |= br.bits(1)? as i32;
                let count = self.counts[len] as i32;
                if code - count < first {
                    return Ok(self.symbols[(index + (code - first)) as usize]);
                }
                index += count;
                first += count;
                first <<= 1;
                code <<= 1;
            }
            Err("invalid Huffman code".into())
        }
    }

    const LEN_BASE: [u16; 29] = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
    const LEN_EXTRA: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
    const DIST_BASE: [u16; 30] =
        [1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577];
    const DIST_EXTRA: [u8; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];

    /// Decompress a zlib stream (2 byte header, deflate data, adler32).
    pub fn zlib_decompress(data: &[u8]) -> Result<Vec<u8>, String> {
        if data.len() < 2 || (data[0] & 0x0F) != 8 || ((data[0] as u16) << 8 | data[1] as u16) % 31 != 0 {
            return Err("bad zlib header".into());
        }
        inflate(&data[2..])
    }

    /// Decompress raw DEFLATE data.
    pub fn inflate(data: &[u8]) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        let mut br = BitReader { data, pos: 0, bit: 0, nbits: 0 };
        loop {
            let last = br.bits(1)?;
            let ty = br.bits(2)?;
            match ty {
                0 => {
                    br.align();
                    if br.pos + 4 > data.len() {
                        return Err("truncated stored block".into());
                    }
                    let len = data[br.pos] as usize | (data[br.pos + 1] as usize) << 8;
                    br.pos += 4;
                    if br.pos + len > data.len() {
                        return Err("truncated stored block".into());
                    }
                    out.extend_from_slice(&data[br.pos..br.pos + len]);
                    br.pos += len;
                }
                1 => {
                    let mut lengths = [0u8; 288];
                    for (i, l) in lengths.iter_mut().enumerate() {
                        *l = match i {
                            0..=143 => 8,
                            144..=255 => 9,
                            256..=279 => 7,
                            _ => 8,
                        };
                    }
                    let lit = Huffman::new(&lengths);
                    let dist = Huffman::new(&[5u8; 30]);
                    inflate_block(&mut br, &mut out, &lit, &dist)?;
                }
                2 => {
                    let hlit = br.bits(5)? as usize + 257;
                    let hdist = br.bits(5)? as usize + 1;
                    let hclen = br.bits(4)? as usize + 4;
                    const ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
                    let mut cl = [0u8; 19];
                    for &o in ORDER.iter().take(hclen) {
                        cl[o] = br.bits(3)? as u8;
                    }
                    let clh = Huffman::new(&cl);
                    let mut lengths = vec![0u8; hlit + hdist];
                    let mut i = 0;
                    while i < hlit + hdist {
                        let sym = clh.decode(&mut br)?;
                        match sym {
                            0..=15 => {
                                lengths[i] = sym as u8;
                                i += 1;
                            }
                            16 => {
                                if i == 0 {
                                    return Err("bad code lengths".into());
                                }
                                let prev = lengths[i - 1];
                                for _ in 0..3 + br.bits(2)? {
                                    if i >= lengths.len() {
                                        return Err("bad code lengths".into());
                                    }
                                    lengths[i] = prev;
                                    i += 1;
                                }
                            }
                            17 => i += 3 + br.bits(3)? as usize,
                            _ => i += 11 + br.bits(7)? as usize,
                        }
                    }
                    if i > hlit + hdist {
                        return Err("bad code lengths".into());
                    }
                    let lit = Huffman::new(&lengths[..hlit]);
                    let dist = Huffman::new(&lengths[hlit..]);
                    inflate_block(&mut br, &mut out, &lit, &dist)?;
                }
                _ => return Err("invalid deflate block type".into()),
            }
            if last == 1 {
                break;
            }
        }
        Ok(out)
    }

    fn inflate_block(br: &mut BitReader, out: &mut Vec<u8>, lit: &Huffman, dist: &Huffman) -> Result<(), String> {
        loop {
            let sym = lit.decode(br)? as usize;
            if sym < 256 {
                out.push(sym as u8);
            } else if sym == 256 {
                return Ok(());
            } else {
                let li = sym - 257;
                if li >= 29 {
                    return Err("bad length symbol".into());
                }
                let len = LEN_BASE[li] as usize + br.bits(LEN_EXTRA[li] as u32)? as usize;
                let di = dist.decode(br)? as usize;
                if di >= 30 {
                    return Err("bad distance symbol".into());
                }
                let d = DIST_BASE[di] as usize + br.bits(DIST_EXTRA[di] as u32)? as usize;
                if d > out.len() {
                    return Err("bad distance".into());
                }
                let start = out.len() - d;
                for k in 0..len {
                    let b = out[start + k];
                    out.push(b);
                }
            }
        }
    }
}

pub mod png {
    use super::inflate::zlib_decompress;

    fn be32(d: &[u8], i: usize) -> u32 {
        (d[i] as u32) << 24 | (d[i + 1] as u32) << 16 | (d[i + 2] as u32) << 8 | d[i + 3] as u32
    }

    fn paeth(a: i32, b: i32, c: i32) -> u8 {
        let p = a + b - c;
        let pa = (p - a).abs();
        let pb = (p - b).abs();
        let pc = (p - c).abs();
        if pa <= pb && pa <= pc {
            a as u8
        } else if pb <= pc {
            b as u8
        } else {
            c as u8
        }
    }

    /// Unfilter one (sub)image; returns the raw scanline bytes without filter bytes.
    fn unfilter(data: &[u8], w: usize, h: usize, bits_per_pixel: usize) -> Result<Vec<u8>, String> {
        let stride = (w * bits_per_pixel).div_ceil(8);
        let bpp = bits_per_pixel.div_ceil(8);
        if data.len() < h * (stride + 1) {
            return Err("not enough pixels".into());
        }
        let mut out = vec![0u8; h * stride];
        for y in 0..h {
            let ft = data[y * (stride + 1)];
            let src = &data[y * (stride + 1) + 1..y * (stride + 1) + 1 + stride];
            for x in 0..stride {
                let a = if x >= bpp { out[y * stride + x - bpp] as i32 } else { 0 };
                let b = if y > 0 { out[(y - 1) * stride + x] as i32 } else { 0 };
                let c = if y > 0 && x >= bpp { out[(y - 1) * stride + x - bpp] as i32 } else { 0 };
                let v = src[x];
                out[y * stride + x] = match ft {
                    0 => v,
                    1 => v.wrapping_add(a as u8),
                    2 => v.wrapping_add(b as u8),
                    3 => v.wrapping_add(((a + b) / 2) as u8),
                    4 => v.wrapping_add(paeth(a, b, c)),
                    _ => return Err("invalid filter".into()),
                };
            }
        }
        Ok(out)
    }

    /// A PNG text chunk as libpng's `png_get_text` reports it after `png_read_info` (i.e. only
    /// chunks before the first IDAT). `text` is what `text_length` covers: iTXt chunks report a
    /// `text_length` of 0 (their length is in `itxt_length`).
    pub struct PngText {
        pub key: String,
        pub text: Vec<u8>,
    }

    /// The result of libpng's simplified read API (`png_image_begin_read_from_memory` +
    /// `png_image_finish_read` with the file's own format): 8-bit samples, gray / gray+alpha /
    /// RGB / RGBA, with tRNS turned into alpha.
    pub struct SimplifiedPng {
        pub width: usize,
        pub height: usize,
        pub bytes_per_pixel: usize,
        pub data: Vec<u8>,
        pub texts: Vec<PngText>,
    }

    /// The text chunks before the first IDAT, in file order.
    pub fn read_texts(d: &[u8]) -> Result<Vec<PngText>, String> {
        let mut texts = Vec::new();
        let mut pos = 8;
        while pos + 8 <= d.len() {
            let len = be32(d, pos) as usize;
            let ty = &d[pos + 4..pos + 8];
            let body_start = pos + 8;
            if body_start + len > d.len() {
                return Err("corrupt PNG".into());
            }
            let body = &d[body_start..body_start + len];
            match ty {
                b"IDAT" | b"IEND" => break,
                b"tEXt" => {
                    let z = body.iter().position(|&b| b == 0).unwrap_or(body.len());
                    let key = String::from_utf8_lossy(&body[..z]).into_owned();
                    let text = if z < body.len() { body[z + 1..].to_vec() } else { Vec::new() };
                    texts.push(PngText { key, text });
                }
                b"zTXt" => {
                    let z = body.iter().position(|&b| b == 0).unwrap_or(body.len());
                    let key = String::from_utf8_lossy(&body[..z]).into_owned();
                    if z + 2 <= body.len() {
                        if let Ok(text) = zlib_decompress(&body[z + 2..]) {
                            texts.push(PngText { key, text });
                        }
                    }
                }
                b"iTXt" => {
                    let z = body.iter().position(|&b| b == 0).unwrap_or(body.len());
                    let key = String::from_utf8_lossy(&body[..z]).into_owned();
                    texts.push(PngText { key, text: Vec::new() });
                }
                _ => {}
            }
            pos = body_start + len + 4;
        }
        Ok(texts)
    }

    /// The file gamma libpng ends up with for a PNG (`png_ptr->colorspace.gamma` after the chunks
    /// before the first IDAT): `gAMA` / `sRGB` handling of libpng 1.6, 0 if there is none.
    fn file_gamma(d: &[u8]) -> i64 {
        const FROM_GAMA: u32 = 1;
        const FROM_SRGB: u32 = 2;
        const HAVE_GAMMA: u32 = 4;
        const INVALID: u32 = 8;
        let significant = |g: i64| g < 100000 - 5000 || g > 100000 + 5000;
        let muldiv = |a: i64, times: i64, divisor: i64| -> Option<i64> {
            if divisor == 0 {
                return None;
            }
            let r = ((a as f64) * times as f64 / divisor as f64 + 0.5).floor();
            if (-2147483648.0..=2147483647.0).contains(&r) {
                Some(r as i64)
            } else {
                None
            }
        };
        let (mut gamma, mut flags, mut have_plte) = (0i64, 0u32, false);
        // png_colorspace_check_gamma: whether the new value may be stored.
        let check_gamma = |gamma: i64, flags: u32, new: i64, from: u32| -> bool {
            if flags & HAVE_GAMMA != 0 && muldiv(gamma, 100000, new).is_none_or(significant) {
                if flags & FROM_SRGB != 0 || from == 2 {
                    return from == 2;
                }
                return from == 1;
            }
            true
        };
        let mut pos = 8;
        while pos + 8 <= d.len() {
            let len = be32(d, pos) as usize;
            let ty = &d[pos + 4..pos + 8];
            let body_start = pos + 8;
            if body_start + len > d.len() {
                break;
            }
            let body = &d[body_start..body_start + len];
            match ty {
                b"IDAT" | b"IEND" => break,
                b"PLTE" => have_plte = true,
                b"gAMA" if !have_plte && len == 4 => {
                    let v = be32(body, 0);
                    let v: i64 = if v > 0x7fffffff { -1 } else { v as i64 };
                    if !(16..=625000000).contains(&v) || flags & FROM_GAMA != 0 {
                        flags |= INVALID;
                    } else if flags & INVALID == 0 && check_gamma(gamma, flags, v, 1) {
                        gamma = v;
                        flags |= HAVE_GAMMA | FROM_GAMA;
                    }
                }
                b"sRGB" if !have_plte && len == 1 => {
                    if flags & INVALID == 0 && body[0] <= 3 && flags & FROM_SRGB == 0 && check_gamma(gamma, flags, 45455, 2) {
                        gamma = 45455;
                        flags |= HAVE_GAMMA | FROM_SRGB;
                    }
                }
                _ => {}
            }
            pos = body_start + len + 4;
        }
        gamma
    }

    /// libpng's simplified read of a PNG in its natural format (`png_image_begin_read_from_memory`
    /// + `png_image_finish_read` with the format it reported): palette images fail (no colour-map
    /// buffer); 8-bit output is gamma corrected to sRGB, 16-bit output is linear, little-endian and
    /// alpha-premultiplied. 16-bit images that carry gamma information (which libpng would convert
    /// to linear light) are not supported.
    pub fn decode_simplified(d: &[u8]) -> Result<SimplifiedPng, String> {
        if d.len() < 8 || d[..8] != [137, 80, 78, 71, 13, 10, 26, 10] {
            return Err("Not a PNG file".into());
        }
        if d.len() < 8 + 8 + 13 || &d[12..16] != b"IHDR" {
            return Err("corrupt PNG".into());
        }
        let depth = d[16 + 8];
        let color = d[16 + 9];
        if color == 3 {
            return Err("png_image_finish_read[color-map]: no color-map".into());
        }
        let muldiv = |a: i64, times: i64, divisor: i64| ((a as f64) * times as f64 / divisor as f64 + 0.5).floor() as i64;
        let significant = |g: i64| !(100000 - 5000..=100000 + 5000).contains(&g);
        let file_gamma = file_gamma(d);
        let texts = read_texts(d)?;

        if depth == 16 {
            // Linear output: the default file gamma is linear too.
            if file_gamma != 0 && significant(muldiv(file_gamma, 100000, 100000)) {
                return Err("16-bit (linear) textures with gamma information are not supported".into());
            }
            let raw = decode_raw(d)?;
            let has_alpha = raw.color == 4 || raw.color == 6 || raw.has_trns;
            let out_n = if raw.has_trns { raw.img_n + 1 } else { raw.img_n };
            let mut data = Vec::with_capacity(raw.w * raw.h * out_n * 2);
            let mut px_out = vec![0u16; out_n];
            for px in raw.samples.chunks(raw.img_n) {
                px_out[..raw.img_n].copy_from_slice(px);
                if raw.has_trns {
                    let transparent = if raw.color == 0 {
                        px[0] == raw.trns_gray
                    } else {
                        px[0] == raw.trns_rgb[0] && px[1] == raw.trns_rgb[1] && px[2] == raw.trns_rgb[2]
                    };
                    px_out[out_n - 1] = if transparent { 0 } else { 0xffff };
                }
                if has_alpha {
                    // png_do_compose on a black background (PNG_ALPHA_STANDARD), "no division" variant.
                    let a = px_out[out_n - 1];
                    if a == 0 {
                        for v in &mut px_out[..out_n - 1] {
                            *v = 0;
                        }
                    } else if a != 0xffff {
                        for v in &mut px_out[..out_n - 1] {
                            let temp = (*v as u32) * (a as u32) + 32768;
                            *v = ((temp + (temp >> 16)) >> 16) as u16;
                        }
                    }
                }
                for v in &px_out {
                    data.extend_from_slice(&v.to_le_bytes());
                }
            }
            return Ok(SimplifiedPng { width: raw.w, height: raw.h, bytes_per_pixel: out_n * 2, data, texts });
        }

        let (width, height, bytes_per_pixel, mut data) = decode(d)?;
        // sRGB output: gamma correct when the file gamma is not (close to) sRGB.
        const SCREEN_GAMMA: i64 = 220000; // PNG_GAMMA_sRGB
        if file_gamma != 0 && significant(muldiv(file_gamma, SCREEN_GAMMA, 100000)) {
            let gamma_val = {
                let r = (1e15 / file_gamma as f64 / SCREEN_GAMMA as f64 + 0.5).floor();
                if (-2147483648.0..=2147483647.0).contains(&r) {
                    r as i64
                } else {
                    0
                }
            };
            if significant(gamma_val) {
                let mut table = [0u8; 256];
                for (i, t) in table.iter_mut().enumerate() {
                    *t = if i > 0 && i < 255 { (255.0 * (i as f64 / 255.0).powf(gamma_val as f64 * 0.00001) + 0.5).floor() as u8 } else { i as u8 };
                }
                let color_channels = if bytes_per_pixel == 2 || bytes_per_pixel == 4 { bytes_per_pixel - 1 } else { bytes_per_pixel };
                for px in data.chunks_mut(bytes_per_pixel) {
                    for v in &mut px[..color_channels] {
                        *v = table[*v as usize];
                    }
                }
            }
        }
        Ok(SimplifiedPng { width, height, bytes_per_pixel, data, texts })
    }

    /// A PNG's samples (one `u16` per channel per pixel, in the file's bit depth) and the chunk
    /// data the decoders need.
    pub struct RawPng {
        pub w: usize,
        pub h: usize,
        pub depth: u8,
        pub color: u8,
        pub img_n: usize,
        pub samples: Vec<u16>,
        pub palette: Vec<[u8; 4]>,
        pub has_trns: bool,
        pub trns_gray: u16,
        pub trns_rgb: [u16; 3],
    }

    pub fn decode(d: &[u8]) -> Result<(usize, usize, usize, Vec<u8>), String> {
        let raw = decode_raw(d)?;
        let RawPng { w, h, depth, color, img_n, samples, palette, has_trns, trns_gray, trns_rgb } = raw;
        // Convert to 8 bit like stb_image.
        let to8 = |v: u16| -> u8 {
            match depth {
                16 => (v >> 8) as u8,
                8 => v as u8,
                _ => (v as u32 * 255 / ((1u32 << depth) - 1)) as u8,
            }
        };
        if color == 3 {
            let out_n = if has_trns { 4 } else { 3 };
            let mut out = Vec::with_capacity(w * h * out_n);
            for &idx in &samples {
                let c = palette.get(idx as usize).copied().unwrap_or([0, 0, 0, 255]);
                out.extend_from_slice(&c[..out_n]);
            }
            return Ok((w, h, out_n, out));
        }
        let out_n = if has_trns && (color == 0 || color == 2) { img_n + 1 } else { img_n };
        let mut out = Vec::with_capacity(w * h * out_n);
        for px in samples.chunks(img_n) {
            for &v in px {
                out.push(to8(v));
            }
            if out_n > img_n {
                let transparent = if color == 0 { px[0] == trns_gray } else { px[0] == trns_rgb[0] && px[1] == trns_rgb[1] && px[2] == trns_rgb[2] };
                out.push(if transparent { 0 } else { 255 });
            }
        }
        Ok((w, h, out_n, out))
    }

    pub fn decode_raw(d: &[u8]) -> Result<RawPng, String> {
        let mut pos = 8;
        let (mut w, mut h, mut depth, mut color, mut interlace) = (0usize, 0usize, 0u8, 0u8, 0u8);
        let mut palette: Vec<[u8; 4]> = Vec::new();
        let mut has_trns = false;
        let mut trns_gray: u16 = 0;
        let mut trns_rgb: [u16; 3] = [0; 3];
        let mut idat = Vec::new();
        while pos + 8 <= d.len() {
            let len = be32(d, pos) as usize;
            let ty = &d[pos + 4..pos + 8];
            let body_start = pos + 8;
            if body_start + len > d.len() {
                return Err("corrupt PNG".into());
            }
            let body = &d[body_start..body_start + len];
            match ty {
                b"IHDR" => {
                    w = be32(body, 0) as usize;
                    h = be32(body, 4) as usize;
                    depth = body[8];
                    color = body[9];
                    interlace = body[12];
                }
                b"PLTE" => {
                    palette = body.chunks(3).map(|c| [c[0], c[1], c[2], 255]).collect();
                }
                b"tRNS" => {
                    has_trns = true;
                    match color {
                        3 => {
                            for (i, &a) in body.iter().enumerate() {
                                if i < palette.len() {
                                    palette[i][3] = a;
                                }
                            }
                        }
                        0 => trns_gray = (body[0] as u16) << 8 | body[1] as u16,
                        2 => {
                            for (k, t) in trns_rgb.iter_mut().enumerate() {
                                *t = (body[2 * k] as u16) << 8 | body[2 * k + 1] as u16;
                            }
                        }
                        _ => {}
                    }
                }
                b"IDAT" => idat.extend_from_slice(body),
                b"IEND" => break,
                _ => {}
            }
            pos = body_start + len + 4;
        }
        if w == 0 || h == 0 {
            return Err("corrupt PNG".into());
        }
        let img_n = match color {
            0 => 1,
            2 => 3,
            3 => 1,
            4 => 2,
            6 => 4,
            _ => return Err("bad color type".into()),
        };
        let raw = zlib_decompress(&idat)?;
        let bits_per_pixel = img_n * depth as usize;

        // Unpack to one 16 bit sample per channel per pixel, full image.
        let mut samples = vec![0u16; w * h * img_n];
        let unpack_into = |img: &[u8], sw: usize, sh: usize, place: &mut dyn FnMut(usize, usize, usize, u16)| {
            let stride = (sw * bits_per_pixel).div_ceil(8);
            for y in 0..sh {
                let row = &img[y * stride..(y + 1) * stride];
                for x in 0..sw {
                    for c in 0..img_n {
                        let v = match depth {
                            16 => {
                                let i = (x * img_n + c) * 2;
                                (row[i] as u16) << 8 | row[i + 1] as u16
                            }
                            8 => row[x * img_n + c] as u16,
                            _ => {
                                let bit = (x * img_n + c) * depth as usize;
                                let byte = row[bit / 8];
                                let shift = 8 - depth as usize - bit % 8;
                                ((byte >> shift) & ((1u8 << depth) - 1)) as u16
                            }
                        };
                        place(x, y, c, v);
                    }
                }
            }
        };
        if interlace == 0 {
            let img = unfilter(&raw, w, h, bits_per_pixel)?;
            unpack_into(&img, w, h, &mut |x, y, c, v| samples[(y * w + x) * img_n + c] = v);
        } else {
            const XO: [usize; 7] = [0, 4, 0, 2, 0, 1, 0];
            const YO: [usize; 7] = [0, 0, 4, 0, 2, 0, 1];
            const XS: [usize; 7] = [8, 8, 4, 4, 2, 2, 1];
            const YS: [usize; 7] = [8, 8, 8, 4, 4, 2, 2];
            let mut offset = 0;
            for p in 0..7 {
                let sw = (w + XS[p] - 1 - XO[p]) / XS[p];
                let sh = (h + YS[p] - 1 - YO[p]) / YS[p];
                if sw == 0 || sh == 0 {
                    continue;
                }
                let stride = (sw * bits_per_pixel).div_ceil(8);
                let size = sh * (stride + 1);
                if offset + size > raw.len() {
                    return Err("not enough pixels".into());
                }
                let img = unfilter(&raw[offset..offset + size], sw, sh, bits_per_pixel)?;
                unpack_into(&img, sw, sh, &mut |x, y, c, v| {
                    let (fx, fy) = (x * XS[p] + XO[p], y * YS[p] + YO[p]);
                    samples[(fy * w + fx) * img_n + c] = v;
                });
                offset += size;
            }
        }

        Ok(RawPng { w, h, depth, color, img_n, samples, palette, has_trns, trns_gray, trns_rgb })
    }
}
