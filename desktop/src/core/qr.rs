//! QR codes for setting up two-step sign-in: byte mode, medium error
//! correction, versions 1 to 10 (an `otpauth://` address fits well inside),
//! the mask picked by the standard's penalty rules, so the code comes out
//! the same as the web's (`uqr`). After Project Nayuki's qrcodegen.

/// A QR code: `size` by `size` modules, true for dark.
pub struct Qr {
    pub size: usize,
    modules: Vec<bool>,
    function: Vec<bool>,
}

/// (total codewords, error correction codewords per block, blocks) for versions 1 to 10 at level M.
const TABLE: [(usize, usize, usize); 10] = [
    (26, 10, 1),
    (44, 16, 1),
    (70, 26, 1),
    (100, 18, 2),
    (134, 24, 2),
    (172, 16, 4),
    (196, 18, 4),
    (242, 22, 4),
    (292, 22, 5),
    (346, 26, 5),
];

impl Qr {
    pub fn get(&self, x: usize, y: usize) -> bool {
        self.modules[y * self.size + x]
    }

    /// Encodes `text`, or None when it's too long for version 10.
    pub fn encode(text: &str) -> Option<Qr> {
        let data = text.as_bytes();
        let version = (1..=10).find(|v| {
            let count_bits = if *v < 10 { 8 } else { 16 };
            let (total, ec, blocks) = TABLE[v - 1];
            4 + count_bits + data.len() * 8 <= (total - ec * blocks) * 8
        })?;
        let (total, ec, blocks) = TABLE[version - 1];
        let capacity = (total - ec * blocks) * 8;
        let mut bits: Vec<bool> = Vec::new();
        let push = |value: u32, len: usize, bits: &mut Vec<bool>| {
            for i in (0..len).rev() {
                bits.push((value >> i) & 1 == 1);
            }
        };
        push(0b0100, 4, &mut bits);
        push(data.len() as u32, if version < 10 { 8 } else { 16 }, &mut bits);
        for b in data {
            push(u32::from(*b), 8, &mut bits);
        }
        let end = (capacity - bits.len()).min(4);
        push(0, end, &mut bits);
        let pad = (8 - bits.len() % 8) % 8;
        push(0, pad, &mut bits);
        let mut pad_byte = 0xEC;
        while bits.len() < capacity {
            push(pad_byte, 8, &mut bits);
            pad_byte ^= 0xEC ^ 0x11;
        }
        let codewords: Vec<u8> =
            bits.chunks(8).map(|c| c.iter().fold(0u8, |acc, b| (acc << 1) | u8::from(*b))).collect();
        let all = add_ecc(&codewords, version);
        let size = version * 4 + 17;
        let mut qr = Qr { size, modules: vec![false; size * size], function: vec![false; size * size] };
        qr.draw_function_patterns(version);
        qr.draw_codewords(&all);
        // The mask with the lowest penalty.
        let mut best = (u32::MAX, 0u8);
        for mask in 0..8u8 {
            qr.apply_mask(mask);
            qr.draw_format(mask);
            let penalty = qr.penalty();
            if penalty < best.0 {
                best = (penalty, mask);
            }
            qr.apply_mask(mask);
        }
        qr.apply_mask(best.1);
        qr.draw_format(best.1);
        let _ = total;
        Some(qr)
    }

    fn set_function(&mut self, x: usize, y: usize, dark: bool) {
        let i = y * self.size + x;
        self.modules[i] = dark;
        self.function[i] = true;
    }

    fn draw_function_patterns(&mut self, version: usize) {
        let size = self.size;
        for i in 0..size {
            self.set_function(6, i, i % 2 == 0);
            self.set_function(i, 6, i % 2 == 0);
        }
        self.draw_finder(3, 3);
        self.draw_finder(size as i32 - 4, 3);
        self.draw_finder(3, size as i32 - 4);
        let positions = alignment_positions(version);
        let n = positions.len();
        for i in 0..n {
            for j in 0..n {
                if (i == 0 && j == 0) || (i == 0 && j == n - 1) || (i == n - 1 && j == 0) {
                    continue;
                }
                self.draw_alignment(positions[i], positions[j]);
            }
        }
        // Reserve the format areas (drawn properly later) and the version.
        self.draw_format(0);
        if version >= 7 {
            let mut rem = version as u32;
            for _ in 0..12 {
                rem = (rem << 1) ^ ((rem >> 11) * 0x1F25);
            }
            let bits = ((version as u32) << 12) | rem;
            for i in 0..18 {
                let dark = (bits >> i) & 1 == 1;
                let a = size - 11 + i % 3;
                let b = i / 3;
                self.set_function(a, b, dark);
                self.set_function(b, a, dark);
            }
        }
    }

    fn draw_finder(&mut self, x: i32, y: i32) {
        for dy in -4..=4i32 {
            for dx in -4..=4i32 {
                let dist = dx.abs().max(dy.abs());
                let (xx, yy) = (x + dx, y + dy);
                if (0..self.size as i32).contains(&xx) && (0..self.size as i32).contains(&yy) {
                    self.set_function(xx as usize, yy as usize, dist != 2 && dist != 4);
                }
            }
        }
    }

    fn draw_alignment(&mut self, x: usize, y: usize) {
        for dy in -2..=2i32 {
            for dx in -2..=2i32 {
                let dark = dx.abs().max(dy.abs()) != 1;
                self.set_function((x as i32 + dx) as usize, (y as i32 + dy) as usize, dark);
            }
        }
    }

    fn draw_format(&mut self, mask: u8) {
        // Level M is 0b00.
        let data = u32::from(mask);
        let mut rem = data;
        for _ in 0..10 {
            rem = (rem << 1) ^ ((rem >> 9) * 0x537);
        }
        let bits = ((data << 10) | rem) ^ 0x5412;
        let bit = |i: u32| (bits >> i) & 1 == 1;
        for i in 0..=5 {
            self.set_function(8, i, bit(i as u32));
        }
        self.set_function(8, 7, bit(6));
        self.set_function(8, 8, bit(7));
        self.set_function(7, 8, bit(8));
        for i in 9..15 {
            self.set_function(14 - i, 8, bit(i as u32));
        }
        let size = self.size;
        for i in 0..8 {
            self.set_function(size - 1 - i, 8, bit(i as u32));
        }
        for i in 8..15 {
            self.set_function(8, size - 15 + i, bit(i as u32));
        }
        self.set_function(8, size - 8, true);
    }

    fn draw_codewords(&mut self, data: &[u8]) {
        let size = self.size as i32;
        let mut i = 0usize;
        let mut right = size - 1;
        while right >= 1 {
            if right == 6 {
                right = 5;
            }
            for vert in 0..size {
                for j in 0..2 {
                    let x = (right - j) as usize;
                    let upward = ((right + 1) & 2) == 0;
                    let y = (if upward { size - 1 - vert } else { vert }) as usize;
                    if !self.function[y * self.size + x] && i < data.len() * 8 {
                        self.modules[y * self.size + x] = (data[i >> 3] >> (7 - (i & 7))) & 1 == 1;
                        i += 1;
                    }
                }
            }
            right -= 2;
        }
    }

    fn apply_mask(&mut self, mask: u8) {
        for y in 0..self.size {
            for x in 0..self.size {
                let invert = match mask {
                    0 => (x + y) % 2 == 0,
                    1 => y % 2 == 0,
                    2 => x % 3 == 0,
                    3 => (x + y) % 3 == 0,
                    4 => (x / 3 + y / 2) % 2 == 0,
                    5 => x * y % 2 + x * y % 3 == 0,
                    6 => (x * y % 2 + x * y % 3) % 2 == 0,
                    _ => ((x + y) % 2 + x * y % 3) % 2 == 0,
                };
                let i = y * self.size + x;
                if invert && !self.function[i] {
                    self.modules[i] = !self.modules[i];
                }
            }
        }
    }

    fn penalty(&self) -> u32 {
        let size = self.size;
        let mut result = 0u32;
        let line = |get: &dyn Fn(usize) -> bool| -> u32 {
            let mut score = 0u32;
            let mut run_color = false;
            let mut run = 0usize;
            let mut history = [0usize; 7];
            for i in 0..size {
                if get(i) == run_color {
                    run += 1;
                    if run == 5 {
                        score += 3;
                    } else if run > 5 {
                        score += 1;
                    }
                } else {
                    finder_add(run, &mut history, size);
                    if !run_color {
                        score += finder_count(&history) * 40;
                    }
                    run_color = get(i);
                    run = 1;
                }
            }
            score + finder_terminate(run_color, run, &mut history, size) * 40
        };
        for y in 0..size {
            result += line(&|x| self.get(x, y));
        }
        for x in 0..size {
            result += line(&|y| self.get(x, y));
        }
        for y in 0..size - 1 {
            for x in 0..size - 1 {
                let c = self.get(x, y);
                if c == self.get(x + 1, y) && c == self.get(x, y + 1) && c == self.get(x + 1, y + 1) {
                    result += 3;
                }
            }
        }
        let dark = self.modules.iter().filter(|m| **m).count() as i64;
        let total = (size * size) as i64;
        let k = ((dark * 20 - total * 10).abs() + total - 1) / total - 1;
        result + (k as u32) * 10
    }
}

fn finder_count(h: &[usize; 7]) -> u32 {
    let n = h[1];
    let core = n > 0 && h[2] == n && h[3] == n * 3 && h[4] == n && h[5] == n;
    u32::from(core && h[0] >= n * 4 && h[6] >= n) + u32::from(core && h[6] >= n * 4 && h[0] >= n)
}

fn finder_terminate(color: bool, mut run: usize, h: &mut [usize; 7], size: usize) -> u32 {
    if color {
        finder_add(run, h, size);
        run = 0;
    }
    run += size;
    finder_add(run, h, size);
    finder_count(h)
}

fn finder_add(mut run: usize, h: &mut [usize; 7], size: usize) {
    if h[0] == 0 {
        run += size;
    }
    h.copy_within(0..6, 1);
    h[0] = run;
}

fn alignment_positions(version: usize) -> Vec<usize> {
    if version == 1 {
        return Vec::new();
    }
    let count = version / 7 + 2;
    let size = version * 4 + 17;
    let step = if version == 32 { 26 } else { (version * 4 + count * 2 + 1) / (count * 2 - 2) * 2 };
    let mut out: Vec<usize> = (0..count - 1).map(|i| size - 7 - i * step).collect();
    out.push(6);
    out.reverse();
    out
}

fn add_ecc(data: &[u8], version: usize) -> Vec<u8> {
    let (total, ec, blocks) = TABLE[version - 1];
    let short_blocks = blocks - total % blocks;
    let short_len = total / blocks;
    let divisor = rs_divisor(ec);
    let mut all_blocks: Vec<Vec<u8>> = Vec::new();
    let mut k = 0;
    for i in 0..blocks {
        let len = short_len - ec + usize::from(i >= short_blocks);
        let dat = data[k..k + len].to_vec();
        k += len;
        let ecc = rs_remainder(&dat, &divisor);
        let mut block = dat;
        if i < short_blocks {
            block.push(0);
        }
        block.extend(ecc);
        all_blocks.push(block);
    }
    let mut out = Vec::with_capacity(total);
    for i in 0..all_blocks[0].len() {
        for (j, block) in all_blocks.iter().enumerate() {
            if i != short_len - ec || j >= short_blocks {
                out.push(block[i]);
            }
        }
    }
    out
}

fn rs_multiply(x: u8, y: u8) -> u8 {
    let mut z: u32 = 0;
    for i in (0..8).rev() {
        z = (z << 1) ^ ((z >> 7) * 0x11D);
        z ^= ((u32::from(y) >> i) & 1) * u32::from(x);
    }
    z as u8
}

fn rs_divisor(degree: usize) -> Vec<u8> {
    let mut result = vec![0u8; degree];
    result[degree - 1] = 1;
    let mut root = 1u8;
    for _ in 0..degree {
        for j in 0..degree {
            result[j] = rs_multiply(result[j], root);
            if j + 1 < degree {
                result[j] ^= result[j + 1];
            }
        }
        root = rs_multiply(root, 0x02);
    }
    result
}

fn rs_remainder(data: &[u8], divisor: &[u8]) -> Vec<u8> {
    let mut result = vec![0u8; divisor.len()];
    for b in data {
        let factor = b ^ result[0];
        result.remove(0);
        result.push(0);
        for (r, d) in result.iter_mut().zip(divisor) {
            *r ^= rs_multiply(*d, factor);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(qr: &Qr) -> Vec<String> {
        (0..qr.size).map(|y| (0..qr.size).map(|x| if qr.get(x, y) { '1' } else { '0' }).collect()).collect()
    }

    #[test]
    fn matches_the_web() {
        // The web's uqr, encode("hello", { ecc: "M", border: 0 }).
        let qr = Qr::encode("hello").unwrap();
        assert_eq!(
            rows(&qr).join(" "),
            "111111100110001111111 100000101100001000001 101110100101101011101 101110100011001011101 101110101100101011101 100000100000101000001 111111101010101111111 000000000011100000000 101010100101000010010 001011000010001000011 010100101110100011111 110010000000001000010 011010110010101010000 000000001111010100111 111111100011011100111 100000100011110110000 101110101011011100011 101110100100001100110 101110101110100010101 100000100100001010010 111111101110101100011"
        );
        let qr = Qr::encode("otpauth://totp/fuwa:alice?secret=JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP&issuer=fuwa").unwrap();
        assert_eq!(qr.size, 37);
        assert_eq!(
            rows(&qr).join(" "),
            "1111111001010010111100010010001111111 1000001011000001000111111011101000001 1011101001010110001100001010001011101 1011101000111000111100000011001011101 1011101011110101001000010101001011101 1000001001111101010110011101101000001 1111111010101010101010101010101111111 0000000000100001011111100100100000000 1010101000001010101101001001100010010 1011000111011011110010101111100001101 1101111100010101111001001000110001011 0111110111010000110111101111001101010 1000101000000101100001001100101001011 1011110110101111101001101110011101010 0001011011000100110011100010101000111 0100110100111001000111110101001110010 1000101011101100000001011000011010001 1011010000010111010011100101100001001 0111001000011010011011101100110001011 0010010100101111011011110111101110001 1111011011011100000001001100011101110 0011110011100101111011001000101101000 0111011110011110101000001110111011111 1000000000101111111111111111101110010 1100011101011000101011010110111011000 0010110010010001111011011111100001011 1011011000111011110000001010011010111 0101100100010000111111110110010100000 1010111101000111100111011110111111110 0000000010001101101011001110100010010 1111111000000000010010000111101011111 1000001001000110000001110110100010000 1011101010100010000001000000111111011 1011101000101111000001111010100111110 1011101011011110001011101111000110111 1000001001111001011001110100111100010 1111111010011000011001001100111111111"
        );
        assert!(Qr::encode(&"x".repeat(300)).is_none());
    }
}
