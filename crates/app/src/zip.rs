//! Minimal ZIP archive support for 3MF files: reading (stored and DEFLATE entries, with a
//! complete RFC 1951 inflater) and writing (DEFLATE with fixed Huffman codes or stored).

use std::collections::HashMap;

pub struct ZipEntry {
    pub name: String,
    pub data: Vec<u8>,
}

fn u16le(b: &[u8], p: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(p..p + 2)?.try_into().ok()?))
}
fn u32le(b: &[u8], p: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(p..p + 4)?.try_into().ok()?))
}

/// Read all entries of a ZIP archive (using the central directory).
pub fn read_zip(data: &[u8]) -> Result<Vec<ZipEntry>, String> {
    // Find the end of central directory record.
    let min = data.len().saturating_sub(65536 + 22);
    let mut eocd = None;
    let mut i = data.len().saturating_sub(22);
    loop {
        if u32le(data, i) == Some(0x06054b50) {
            eocd = Some(i);
            break;
        }
        if i == 0 || i <= min {
            break;
        }
        i -= 1;
    }
    let eocd = eocd.ok_or("not a ZIP archive")?;
    let count = u16le(data, eocd + 10).ok_or("bad zip")? as usize;
    let mut cd = u32le(data, eocd + 16).ok_or("bad zip")? as usize;
    let mut out = Vec::new();
    for _ in 0..count {
        if u32le(data, cd) != Some(0x02014b50) {
            return Err("bad central directory".into());
        }
        let method = u16le(data, cd + 10).ok_or("bad zip")?;
        let csize = u32le(data, cd + 20).ok_or("bad zip")? as usize;
        let name_len = u16le(data, cd + 28).ok_or("bad zip")? as usize;
        let extra_len = u16le(data, cd + 30).ok_or("bad zip")? as usize;
        let comment_len = u16le(data, cd + 32).ok_or("bad zip")? as usize;
        let local = u32le(data, cd + 42).ok_or("bad zip")? as usize;
        let name = String::from_utf8_lossy(data.get(cd + 46..cd + 46 + name_len).ok_or("bad zip")?).into_owned();
        cd += 46 + name_len + extra_len + comment_len;
        if u32le(data, local) != Some(0x04034b50) {
            return Err(format!("bad local header for {}", name));
        }
        let lname = u16le(data, local + 26).ok_or("bad zip")? as usize;
        let lextra = u16le(data, local + 28).ok_or("bad zip")? as usize;
        let start = local + 30 + lname + lextra;
        let raw = data.get(start..start + csize).ok_or("truncated zip")?;
        let content = match method {
            0 => raw.to_vec(),
            8 => inflate(raw)?,
            m => return Err(format!("unsupported compression method {} for {}", m, name)),
        };
        out.push(ZipEntry { name, data: content });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Inflate (RFC 1951)
// ---------------------------------------------------------------------------------------------

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    bit: u32,
    nbits: u32,
}

impl<'a> Bits<'a> {
    fn need(&mut self, n: u32) -> Result<(), String> {
        while self.nbits < n {
            let b = *self.data.get(self.pos).ok_or("inflate: unexpected end of data")?;
            self.pos += 1;
            self.bit |= (b as u32) << self.nbits;
            self.nbits += 8;
        }
        Ok(())
    }
    fn get(&mut self, n: u32) -> Result<u32, String> {
        if n == 0 {
            return Ok(0);
        }
        self.need(n)?;
        let v = self.bit & ((1u32 << n) - 1);
        self.bit >>= n;
        self.nbits -= n;
        Ok(v)
    }
}

struct Huffman {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    fn new(lengths: &[u8]) -> Huffman {
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
        for (s, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbols[offs[l as usize] as usize] = s as u16;
                offs[l as usize] += 1;
            }
        }
        Huffman { counts, symbols }
    }
    fn decode(&self, b: &mut Bits) -> Result<u16, String> {
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: i32 = 0;
        for len in 1..16 {
            code |= b.get(1)? as i32;
            let count = self.counts[len] as i32;
            if code - count < first {
                return Ok(self.symbols[(index + (code - first)) as usize]);
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err("inflate: bad code".into())
    }
}

const LEN_BASE: [u16; 29] = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LEN_EXTRA: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DIST_BASE: [u16; 30] = [1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577];
const DIST_EXTRA: [u8; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];

fn fixed_tables() -> (Huffman, Huffman) {
    let mut l = [0u8; 288];
    for (i, v) in l.iter_mut().enumerate() {
        *v = if i < 144 {
            8
        } else if i < 256 {
            9
        } else if i < 280 {
            7
        } else {
            8
        };
    }
    (Huffman::new(&l), Huffman::new(&[5u8; 30]))
}

fn inflate_block(b: &mut Bits, out: &mut Vec<u8>, lit: &Huffman, dist: &Huffman) -> Result<(), String> {
    loop {
        let sym = lit.decode(b)?;
        if sym < 256 {
            out.push(sym as u8);
        } else if sym == 256 {
            return Ok(());
        } else {
            let s = (sym - 257) as usize;
            if s >= 29 {
                return Err("inflate: bad length symbol".into());
            }
            let len = LEN_BASE[s] as usize + b.get(LEN_EXTRA[s] as u32)? as usize;
            let ds = dist.decode(b)? as usize;
            if ds >= 30 {
                return Err("inflate: bad distance symbol".into());
            }
            let d = DIST_BASE[ds] as usize + b.get(DIST_EXTRA[ds] as u32)? as usize;
            if d > out.len() {
                return Err("inflate: distance too far back".into());
            }
            let start = out.len() - d;
            for i in 0..len {
                let c = out[start + i];
                out.push(c);
            }
        }
    }
}

/// Decompress a raw DEFLATE stream.
pub fn inflate(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut b = Bits { data, pos: 0, bit: 0, nbits: 0 };
    let mut out = Vec::with_capacity(data.len() * 4);
    loop {
        let last = b.get(1)?;
        let ty = b.get(2)?;
        match ty {
            0 => {
                b.bit = 0;
                b.nbits = 0;
                let len = u16le(data, b.pos).ok_or("inflate: truncated")? as usize;
                b.pos += 4;
                out.extend_from_slice(data.get(b.pos..b.pos + len).ok_or("inflate: truncated stored block")?);
                b.pos += len;
            }
            1 => {
                let (l, d) = fixed_tables();
                inflate_block(&mut b, &mut out, &l, &d)?;
            }
            2 => {
                let nlen = b.get(5)? as usize + 257;
                let ndist = b.get(5)? as usize + 1;
                let ncode = b.get(4)? as usize + 4;
                const ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
                let mut cl = [0u8; 19];
                for &o in ORDER.iter().take(ncode) {
                    cl[o] = b.get(3)? as u8;
                }
                let clh = Huffman::new(&cl);
                let mut lengths = vec![0u8; nlen + ndist];
                let mut i = 0;
                while i < nlen + ndist {
                    let sym = clh.decode(&mut b)?;
                    match sym {
                        0..=15 => {
                            lengths[i] = sym as u8;
                            i += 1;
                        }
                        16 => {
                            if i == 0 {
                                return Err("inflate: repeat without previous length".into());
                            }
                            let prev = lengths[i - 1];
                            for _ in 0..3 + b.get(2)? {
                                if i < lengths.len() {
                                    lengths[i] = prev;
                                    i += 1;
                                }
                            }
                        }
                        17 => i += 3 + b.get(3)? as usize,
                        _ => i += 11 + b.get(7)? as usize,
                    }
                }
                if i > nlen + ndist {
                    return Err("inflate: too many lengths".into());
                }
                let lh = Huffman::new(&lengths[..nlen]);
                let dh = Huffman::new(&lengths[nlen..]);
                inflate_block(&mut b, &mut out, &lh, &dh)?;
            }
            _ => return Err("inflate: invalid block type".into()),
        }
        if last == 1 {
            break;
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------------------------

pub fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let t = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB88320 ^ (c >> 1) } else { c >> 1 };
            }
            *e = c;
        }
        t
    });
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c = t[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

struct BitWriter {
    out: Vec<u8>,
    bit: u64,
    n: u32,
}

impl BitWriter {
    fn put(&mut self, v: u32, n: u32) {
        self.bit |= (v as u64) << self.n;
        self.n += n;
        while self.n >= 8 {
            self.out.push(self.bit as u8);
            self.bit >>= 8;
            self.n -= 8;
        }
    }
    /// Huffman codes are written most significant bit first.
    fn put_rev(&mut self, code: u32, n: u32) {
        let mut r = 0;
        for i in 0..n {
            r |= ((code >> i) & 1) << (n - 1 - i);
        }
        self.put(r, n);
    }
    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.out.push(self.bit as u8);
        }
        self.out
    }
}

fn put_fixed_lit(w: &mut BitWriter, sym: u32) {
    match sym {
        0..=143 => w.put_rev(0x30 + sym, 8),
        144..=255 => w.put_rev(0x190 + sym - 144, 9),
        256..=279 => w.put_rev(sym - 256, 7),
        _ => w.put_rev(0xC0 + sym - 280, 8),
    }
}

/// DEFLATE with fixed Huffman codes and a hash-chain LZ77 matcher (good enough for 3MF exports).
pub fn deflate(data: &[u8]) -> Vec<u8> {
    let mut w = BitWriter { out: Vec::with_capacity(data.len() / 2), bit: 0, n: 0 };
    w.put(1, 1);
    w.put(1, 2);
    let mut head: HashMap<u32, usize> = HashMap::new();
    let mut prev: Vec<usize> = vec![usize::MAX; data.len()];
    let mut i = 0;
    let key = |i: usize| -> u32 { (data[i] as u32) | (data[i + 1] as u32) << 8 | (data[i + 2] as u32) << 16 };
    while i < data.len() {
        let mut best_len = 0;
        let mut best_dist = 0;
        if i + 3 <= data.len() {
            let k = key(i);
            let mut cand = head.get(&k).copied().unwrap_or(usize::MAX);
            let mut chain = 0;
            while cand != usize::MAX && i - cand <= 32768 && chain < 32 {
                let mut l = 0;
                while l < 258 && i + l < data.len() && data[cand + l] == data[i + l] {
                    l += 1;
                }
                if l > best_len {
                    best_len = l;
                    best_dist = i - cand;
                }
                cand = prev[cand];
                chain += 1;
            }
            prev[i] = head.get(&k).copied().unwrap_or(usize::MAX);
            head.insert(k, i);
        }
        if best_len >= 3 {
            let li = LEN_BASE.iter().rposition(|&b| b as usize <= best_len).unwrap();
            put_fixed_lit(&mut w, 257 + li as u32);
            w.put((best_len - LEN_BASE[li] as usize) as u32, LEN_EXTRA[li] as u32);
            let di = DIST_BASE.iter().rposition(|&b| b as usize <= best_dist).unwrap();
            w.put_rev(di as u32, 5);
            w.put((best_dist - DIST_BASE[di] as usize) as u32, DIST_EXTRA[di] as u32);
            for j in i + 1..i + best_len {
                if j + 3 <= data.len() {
                    let k = key(j);
                    prev[j] = head.get(&k).copied().unwrap_or(usize::MAX);
                    head.insert(k, j);
                }
            }
            i += best_len;
        } else {
            put_fixed_lit(&mut w, data[i] as u32);
            i += 1;
        }
    }
    put_fixed_lit(&mut w, 256);
    w.finish()
}

/// Write a ZIP archive with DEFLATE-compressed entries.
pub fn write_zip(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let crc = crc32(data);
        let comp = deflate(data);
        let offset = out.len() as u32;
        let push16 = |v: &mut Vec<u8>, x: u16| v.extend_from_slice(&x.to_le_bytes());
        let push32 = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes());
        push32(&mut out, 0x04034b50);
        push16(&mut out, 20);
        push16(&mut out, 0);
        push16(&mut out, 8);
        push16(&mut out, 0);
        push16(&mut out, 0x21);
        push32(&mut out, crc);
        push32(&mut out, comp.len() as u32);
        push32(&mut out, data.len() as u32);
        push16(&mut out, name.len() as u16);
        push16(&mut out, 0);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&comp);
        push32(&mut central, 0x02014b50);
        push16(&mut central, 20);
        push16(&mut central, 20);
        push16(&mut central, 0);
        push16(&mut central, 8);
        push16(&mut central, 0);
        push16(&mut central, 0x21);
        push32(&mut central, crc);
        push32(&mut central, comp.len() as u32);
        push32(&mut central, data.len() as u32);
        push16(&mut central, name.len() as u16);
        push16(&mut central, 0);
        push16(&mut central, 0);
        push16(&mut central, 0);
        push16(&mut central, 0);
        push32(&mut central, 0);
        push32(&mut central, offset);
        central.extend_from_slice(name.as_bytes());
    }
    let cd_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x06054b50u32.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        let mut data = Vec::new();
        for i in 0..20000u32 {
            data.extend_from_slice(format!("vertex {} {} {}\n", i % 17, i % 5, i * 7 % 13).as_bytes());
        }
        let z = write_zip(&[("a.txt".into(), data.clone()), ("b/empty".into(), Vec::new())]);
        let e = read_zip(&z).unwrap();
        assert_eq!(e[0].data, data);
        assert_eq!(e[1].data.len(), 0);
        assert!(z.len() < data.len() / 3);
    }
}

/// A ZIP archive exactly as Python's `zipfile` writes entries added with
/// `writestr(ZipInfo(name), data)` (stored, dated 1980-01-01, as Cura's profile writer does).
pub fn write_zip_python_stored(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let push16 = |v: &mut Vec<u8>, x: u16| v.extend_from_slice(&x.to_le_bytes());
    let push32 = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes());
    let create_system: u8 = if cfg!(windows) { 0 } else { 3 };
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let crc = crc32(data);
        let offset = out.len() as u32;
        let flags: u16 = if name.is_ascii() { 0 } else { 0x800 };
        push32(&mut out, 0x04034b50);
        push16(&mut out, 20);
        push16(&mut out, flags);
        push16(&mut out, 0);
        push16(&mut out, 0);
        push16(&mut out, 0x21);
        push32(&mut out, crc);
        push32(&mut out, data.len() as u32);
        push32(&mut out, data.len() as u32);
        push16(&mut out, name.len() as u16);
        push16(&mut out, 0);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        push32(&mut central, 0x02014b50);
        central.push(20);
        central.push(create_system);
        push16(&mut central, 20);
        push16(&mut central, flags);
        push16(&mut central, 0);
        push16(&mut central, 0);
        push16(&mut central, 0x21);
        push32(&mut central, crc);
        push32(&mut central, data.len() as u32);
        push32(&mut central, data.len() as u32);
        push16(&mut central, name.len() as u16);
        push16(&mut central, 0);
        push16(&mut central, 0);
        push16(&mut central, 0);
        push16(&mut central, 0);
        push32(&mut central, 0o600 << 16);
        push32(&mut central, offset);
        central.extend_from_slice(name.as_bytes());
    }
    let cd_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x06054b50u32.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out
}

#[cfg(test)]
mod python_zip_tests {
    #[test]
    fn matches_python() {
        let z = super::write_zip_python_stored(&[("abc_def".into(), b"hello\n".to_vec())]);
        let hex: String = z.iter().map(|b| format!("{:02x}", b)).collect();
        if cfg!(not(windows)) {
            assert_eq!(hex, "504b03041400000000000000210020303a360600000006000000070000006162635f64656668656c6c6f0a504b010214031400000000000000210020303a3606000000060000000700000000000000000000008001000000006162635f646566504b05060000000001000100350000002b0000000000");
        }
    }
}
