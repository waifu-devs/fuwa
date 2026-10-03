//! PNGs of RGBA pixels, written here so the app needs no image encoder of
//! its own: stored as they are (fast, for pictures that are only shown), or
//! filtered and compressed with fixed-Huffman deflate (for pictures that are
//! uploaded, like emoji).

/// A PNG of `w` by `h` RGBA pixels; `compress` makes it small, at some cost in time.
pub fn png(w: u32, h: u32, rgba: &[u8], compress: bool) -> Vec<u8> {
    let stride = w as usize * 4;
    let mut raw = Vec::with_capacity((stride + 1) * h as usize);
    let mut above = vec![0u8; stride];
    for row in rgba.chunks(stride) {
        if compress {
            filter(row, &above, &mut raw);
        } else {
            raw.push(0);
            raw.extend_from_slice(row);
        }
        above.copy_from_slice(row);
    }
    let mut z = vec![0x78, 0x01];
    if compress {
        deflate(&raw, &mut z);
    } else {
        let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
        for (i, block) in blocks.iter().enumerate() {
            z.push(u8::from(i + 1 == blocks.len()));
            let len = block.len() as u16;
            z.extend_from_slice(&len.to_le_bytes());
            z.extend_from_slice(&(!len).to_le_bytes());
            z.extend_from_slice(block);
        }
    }
    let (mut a, mut b) = (1u32, 0u32);
    for byte in &raw {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn crc(bytes: &[u8]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for b in bytes {
        c ^= u32::from(*b);
        for _ in 0..8 {
            c = if c & 1 == 1 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut body = kind.to_vec();
    body.extend_from_slice(data);
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc(&body).to_be_bytes());
}

/// One row, behind the PNG filter that leaves the smallest numbers (the usual guess at what compresses best).
fn filter(row: &[u8], above: &[u8], out: &mut Vec<u8>) {
    let left = |i: usize| if i >= 4 { row[i - 4] } else { 0 };
    let corner = |i: usize| if i >= 4 { above[i - 4] } else { 0 };
    let paeth = |a: u8, b: u8, c: u8| {
        let p = i16::from(a) + i16::from(b) - i16::from(c);
        let (pa, pb, pc) = ((p - i16::from(a)).abs(), (p - i16::from(b)).abs(), (p - i16::from(c)).abs());
        if pa <= pb && pa <= pc {
            a
        } else if pb <= pc {
            b
        } else {
            c
        }
    };
    let apply = |kind: u8, i: usize| -> u8 {
        let x = row[i];
        match kind {
            1 => x.wrapping_sub(left(i)),
            2 => x.wrapping_sub(above[i]),
            3 => x.wrapping_sub(((u16::from(left(i)) + u16::from(above[i])) / 2) as u8),
            4 => x.wrapping_sub(paeth(left(i), above[i], corner(i))),
            _ => x,
        }
    };
    let cost = |kind: u8| (0..row.len()).map(|i| u32::from((apply(kind, i) as i8).unsigned_abs())).sum::<u32>();
    let best = (0..5u8).min_by_key(|k| cost(*k)).unwrap_or(0);
    out.push(best);
    out.extend((0..row.len()).map(|i| apply(best, i)));
}

struct Bits<'a> {
    out: &'a mut Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits<'_> {
    fn put(&mut self, value: u32, n: u32) {
        self.acc |= u64::from(value) << self.n;
        self.n += n;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }

    /// A Huffman code, which deflate writes from its top bit down.
    fn code(&mut self, code: u32, len: u32) {
        self.put(code.reverse_bits() >> (32 - len), len);
    }

    fn literal(&mut self, v: u32) {
        match v {
            0..=143 => self.code(0x30 + v, 8),
            144..=255 => self.code(0x190 + v - 144, 9),
            256..=279 => self.code(v - 256, 7),
            _ => self.code(0xc0 + v - 280, 8),
        }
    }

    fn finish(&mut self) {
        if self.n > 0 {
            self.out.push(self.acc as u8);
        }
        self.acc = 0;
        self.n = 0;
    }
}

const LEN_BASE: [u16; 29] =
    [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LEN_EXTRA: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145,
    8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] =
    [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];

/// `data` as one fixed-Huffman deflate block, finding repeats with a hash of
/// the next three bytes and a short chain of earlier places it was seen.
fn deflate(data: &[u8], out: &mut Vec<u8>) {
    const WINDOW: usize = 32_768;
    const CHAIN: usize = 48;
    const HASH: usize = 1 << 15;
    let hash = |i: usize| {
        ((u32::from(data[i]) << 10 ^ u32::from(data[i + 1]) << 5 ^ u32::from(data[i + 2])) as usize) & (HASH - 1)
    };
    let mut head = vec![usize::MAX; HASH];
    let mut prev = vec![usize::MAX; data.len()];
    let mut bits = Bits { out, acc: 0, n: 0 };
    bits.put(1, 1); // the last block
    bits.put(1, 2); // fixed Huffman codes
    let insert = |i: usize, head: &mut [usize], prev: &mut [usize]| {
        if i + 2 < data.len() {
            let h = hash(i);
            prev[i] = head[h];
            head[h] = i;
        }
    };
    let mut i = 0;
    while i < data.len() {
        let (mut best_len, mut best_dist) = (0, 0);
        if i + 2 < data.len() {
            let mut at = head[hash(i)];
            let max = (data.len() - i).min(258);
            for _ in 0..CHAIN {
                if at == usize::MAX || i - at > WINDOW {
                    break;
                }
                let len = data[at..].iter().zip(&data[i..i + max]).take_while(|(a, b)| a == b).count();
                if len > best_len {
                    (best_len, best_dist) = (len, i - at);
                    if len == max {
                        break;
                    }
                }
                at = prev[at];
            }
        }
        if best_len >= 3 {
            let code = LEN_BASE.iter().rposition(|b| usize::from(*b) <= best_len).unwrap_or(0);
            bits.literal(257 + code as u32);
            bits.put((best_len - usize::from(LEN_BASE[code])) as u32, u32::from(LEN_EXTRA[code]));
            let d = DIST_BASE.iter().rposition(|b| usize::from(*b) <= best_dist).unwrap_or(0);
            bits.code(d as u32, 5);
            bits.put((best_dist - usize::from(DIST_BASE[d])) as u32, u32::from(DIST_EXTRA[d]));
            for k in i..i + best_len {
                insert(k, &mut head, &mut prev);
            }
            i += best_len;
        } else {
            bits.literal(u32::from(data[i]));
            insert(i, &mut head, &mut prev);
            i += 1;
        }
    }
    bits.literal(256);
    bits.finish();
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui_kit::{Image, ImageFormat, SvgRenderer};

    use super::*;

    fn decode(bytes: Vec<u8>) -> (usize, usize, Vec<u8>) {
        let svg = SvgRenderer::new(Arc::new(crate::ui::assets::Assets));
        let image = Image::from_bytes(ImageFormat::Png, bytes).to_image_data(svg).expect("a PNG that decodes");
        let s = image.size(0);
        let mut px = image.as_bytes(0).unwrap().to_vec();
        for p in px.chunks_mut(4) {
            p.swap(0, 2); // BGRA to RGBA
        }
        (i32::from(s.width) as usize, i32::from(s.height) as usize, px)
    }

    #[test]
    fn compressed_pngs_decode_to_the_same_pixels() {
        let (w, h) = (97, 61);
        let mut rgba = Vec::new();
        let mut seed = 7u32;
        for y in 0..h {
            for x in 0..w {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                let noise = if y > 40 { (seed >> 16) as u8 } else { 0 };
                rgba.extend_from_slice(&[(x * 2) as u8, (y * 4) as u8, noise, if x < 10 { 0 } else { 255 }]);
            }
        }
        let small = png(w as u32, h as u32, &rgba, true);
        let plain = png(w as u32, h as u32, &rgba, false);
        assert!(small.len() < plain.len() / 2, "{} vs {}", small.len(), plain.len());
        for bytes in [small, plain] {
            let (dw, dh, px) = decode(bytes);
            assert_eq!((dw, dh), (w, h));
            // Fully see-through pixels may come back with any color.
            for (a, b) in px.chunks(4).zip(rgba.chunks(4)) {
                assert_eq!(a[3], b[3]);
                if b[3] > 0 {
                    assert_eq!(a, b);
                }
            }
        }
    }
}
