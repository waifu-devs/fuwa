//! A GIF's size and its first frame, for what can't read a moving picture:
//! AutoMod providers take PNG, JPEG or WebP, so a GIF is checked by its
//! first frame, written out as a PNG here. Hand-written (an LZW decoder and
//! a PNG writer over flate2) so the server needs no image library.

use std::io::Write;

/// The biggest frame decoded, in pixels.
const MAX_PIXELS: usize = 4096 * 4096;

/// A GIF's width and height, from its header.
pub fn gif_size(gif: &[u8]) -> Option<(u32, u32)> {
    if gif.len() < 10 || !(gif.starts_with(b"GIF87a") || gif.starts_with(b"GIF89a")) {
        return None;
    }
    let width = u16::from_le_bytes([gif[6], gif[7]]) as u32;
    let height = u16::from_le_bytes([gif[8], gif[9]]) as u32;
    (width > 0 && height > 0).then_some((width, height))
}

/// The GIF's first frame, drawn on its screen (transparent around it) and
/// written as a PNG. `None` when it isn't a GIF or doesn't read.
pub fn first_frame_png(gif: &[u8]) -> Option<Vec<u8>> {
    let (width, height) = gif_size(gif)?;
    let (width, height) = (width as usize, height as usize);
    if gif.len() < 13 || width * height > MAX_PIXELS {
        return None;
    }
    let mut at = 13;
    let flags = gif[10];
    let global = take_table(gif, &mut at, flags)?;
    let mut transparent: Option<u8> = None;
    loop {
        match *gif.get(at)? {
            0x21 => {
                let label = *gif.get(at + 1)?;
                at += 2;
                if label == 0xf9 && gif.get(at) == Some(&4) {
                    let packed = *gif.get(at + 1)?;
                    transparent = (packed & 1 == 1).then_some(*gif.get(at + 4)?);
                }
                skip_blocks(gif, &mut at)?;
            }
            0x2c => {
                let d = gif.get(at + 1..at + 10)?;
                at += 10;
                let left = u16::from_le_bytes([d[0], d[1]]) as usize;
                let top = u16::from_le_bytes([d[2], d[3]]) as usize;
                let w = u16::from_le_bytes([d[4], d[5]]) as usize;
                let h = u16::from_le_bytes([d[6], d[7]]) as usize;
                let local = take_table(gif, &mut at, d[8])?;
                let interlaced = d[8] & 0x40 != 0;
                let min_code = *gif.get(at)?;
                at += 1;
                let mut data = Vec::new();
                loop {
                    let n = *gif.get(at)? as usize;
                    at += 1;
                    if n == 0 {
                        break;
                    }
                    data.extend_from_slice(gif.get(at..at + n)?);
                    at += n;
                }
                if w * h > MAX_PIXELS {
                    return None;
                }
                let indices = lzw(min_code, &data, w * h)?;
                let table = local.or(global)?;
                let mut canvas = vec![0u8; width * height * 4];
                let rows = row_order(h, interlaced);
                for (n, &y) in rows.iter().enumerate() {
                    let cy = top + y;
                    if cy >= height {
                        continue;
                    }
                    for x in 0..w {
                        let cx = left + x;
                        if cx >= width {
                            break;
                        }
                        let index = indices[n * w + x];
                        if Some(index) == transparent {
                            continue;
                        }
                        let Some(rgb) = table.get(index as usize * 3..index as usize * 3 + 3) else { continue };
                        let p = (cy * width + cx) * 4;
                        canvas[p..p + 3].copy_from_slice(rgb);
                        canvas[p + 3] = 255;
                    }
                }
                return png(width as u32, height as u32, &canvas);
            }
            _ => return None,
        }
    }
}

/// The colour table `flags` says follows at `at`, if any.
fn take_table<'a>(gif: &'a [u8], at: &mut usize, flags: u8) -> Option<Option<&'a [u8]>> {
    if flags & 0x80 == 0 {
        return Some(None);
    }
    let size = 3 << ((flags & 7) + 1);
    let table = gif.get(*at..*at + size)?;
    *at += size;
    Some(Some(table))
}

fn skip_blocks(gif: &[u8], at: &mut usize) -> Option<()> {
    loop {
        let n = *gif.get(*at)? as usize;
        *at += 1 + n;
        if n == 0 {
            return Some(());
        }
    }
}

/// The picture's rows in the order they're stored.
fn row_order(height: usize, interlaced: bool) -> Vec<usize> {
    if !interlaced {
        return (0..height).collect();
    }
    [(0, 8), (4, 8), (2, 4), (1, 2)].iter().flat_map(|&(start, step)| (start..height).step_by(step)).collect()
}

/// GIF's LZW: `want` colour indices from the image data. A stream that ends
/// early is padded; one that doesn't read is `None`.
fn lzw(min_code: u8, data: &[u8], want: usize) -> Option<Vec<u8>> {
    if !(2..=8).contains(&min_code) {
        return None;
    }
    let clear = 1usize << min_code;
    let end = clear + 1;
    let mut prefix = vec![0u16; 4096];
    let mut suffix = vec![0u8; 4096];
    let mut first = vec![0u8; 4096];
    let mut length = vec![0u16; 4096];
    for i in 0..clear {
        suffix[i] = i as u8;
        first[i] = i as u8;
        length[i] = 1;
    }
    let mut size = min_code as u32 + 1;
    let mut next = end + 1;
    let mut previous: Option<usize> = None;
    let mut out = Vec::with_capacity(want);
    let (mut acc, mut bits, mut byte) = (0u32, 0u32, 0usize);
    let emit = |code: usize, out: &mut Vec<u8>, prefix: &[u16], suffix: &[u8], length: &[u16]| {
        let n = length[code] as usize;
        let start = out.len();
        out.resize(start + n, 0);
        let mut c = code;
        for i in (0..n).rev() {
            out[start + i] = suffix[c];
            c = prefix[c] as usize;
        }
    };
    while out.len() < want {
        while bits < size {
            let Some(&b) = data.get(byte) else {
                out.resize(want, 0);
                return Some(out);
            };
            acc |= (b as u32) << bits;
            bits += 8;
            byte += 1;
        }
        let code = (acc & ((1 << size) - 1)) as usize;
        acc >>= size;
        bits -= size;
        if code == clear {
            size = min_code as u32 + 1;
            next = end + 1;
            previous = None;
            continue;
        }
        if code == end {
            break;
        }
        let Some(p) = previous else {
            if code >= clear {
                return None;
            }
            emit(code, &mut out, &prefix, &suffix, &length);
            previous = Some(code);
            continue;
        };
        if code > next || (code == next && next >= 4096) {
            return None;
        }
        if next < 4096 {
            let head = if code < next { first[code] } else { first[p] };
            prefix[next] = p as u16;
            suffix[next] = head;
            first[next] = first[p];
            length[next] = length[p] + 1;
            next += 1;
            if next == 1 << size && size < 12 {
                size += 1;
            }
        }
        emit(code, &mut out, &prefix, &suffix, &length);
        previous = Some(code);
    }
    out.resize(want, 0);
    Some(out)
}

/// An 8-bit RGBA PNG of `rgba`.
fn png(width: u32, height: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    let mut raw = Vec::with_capacity(rgba.len() + height as usize);
    for row in rgba.chunks(width as usize * 4) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    z.write_all(&raw).ok()?;
    let compressed = z.finish().ok()?;
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &compressed);
    chunk(&mut out, b"IEND", &[]);
    Some(out)
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = flate2::Crc::new();
    crc.update(kind);
    crc.update(data);
    out.extend_from_slice(&crc.sum().to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 3×2 GIF: red, green, blue over white, black, then transparent, with
    /// a looping extension and a comment before the frame.
    pub(crate) fn tiny_gif() -> Vec<u8> {
        let mut g = b"GIF89a".to_vec();
        g.extend_from_slice(&[3, 0, 2, 0, 0x82, 0, 0]); // 3×2, global table of 8
        let table = [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255, 0, 0, 0, 9, 9, 9, 9, 9, 9, 9, 9, 9];
        g.extend_from_slice(&table);
        g.extend_from_slice(&[0x21, 0xff, 11]);
        g.extend_from_slice(b"NETSCAPE2.0");
        g.extend_from_slice(&[3, 1, 0, 0, 0]);
        g.extend_from_slice(&[0x21, 0xfe, 2, b'h', b'i', 0]);
        g.extend_from_slice(&[0x21, 0xf9, 4, 1, 10, 0, 5, 0]); // index 5 is transparent
        g.extend_from_slice(&[0x2c, 0, 0, 0, 0, 3, 0, 2, 0, 0]);
        // LZW, min code 3: clear, 0 1 2 3 4 5, end; each code 4 bits.
        let codes = [8u32, 0, 1, 2, 3, 4, 5, 9];
        let (mut acc, mut bits, mut bytes) = (0u64, 0u32, Vec::new());
        let mut size = 4;
        let mut next = 10;
        for (n, &code) in codes.iter().enumerate() {
            acc |= (code as u64) << bits;
            bits += size;
            // The decoder grows its code size as entries are added (from the
            // second code after a clear).
            if n >= 2 && code != 9 {
                next += 1;
                if next == 1 << size {
                    size += 1;
                }
            }
        }
        while bits > 0 {
            bytes.push(acc as u8);
            acc >>= 8;
            bits = bits.saturating_sub(8);
        }
        g.push(3);
        g.push(bytes.len() as u8);
        g.extend_from_slice(&bytes);
        g.extend_from_slice(&[0, 0x3b]);
        g
    }

    #[test]
    fn sizes_come_from_the_header() {
        assert_eq!(gif_size(&tiny_gif()), Some((3, 2)));
        assert_eq!(gif_size(b"GIF89a"), None);
        assert_eq!(gif_size(b"\x89PNG\r\n\x1a\n\0\0\0\0"), None);
    }

    #[test]
    fn the_first_frame_becomes_a_png() {
        let png = first_frame_png(&tiny_gif()).expect("it reads");
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(&png[16..24], &[0, 0, 0, 3, 0, 0, 0, 2]);
        // Back out of the zlib stream: the pixels, row by row.
        let idat = png.windows(4).position(|w| w == b"IDAT").unwrap();
        let length = u32::from_be_bytes(png[idat - 4..idat].try_into().unwrap()) as usize;
        let mut raw = Vec::new();
        std::io::Read::read_to_end(&mut flate2::read::ZlibDecoder::new(&png[idat + 4..idat + 4 + length]), &mut raw)
            .unwrap();
        assert_eq!(raw.len(), 2 * (1 + 3 * 4));
        assert_eq!(&raw[1..5], &[255, 0, 0, 255]);
        assert_eq!(&raw[9..13], &[0, 0, 255, 255]);
        assert_eq!(&raw[14..18], &[255, 255, 255, 255]);
        assert_eq!(&raw[22..26], &[0, 0, 0, 0], "transparent stays clear");
        // The provider's own size check reads it.
        assert!(crate::automod::providers::Picture::read(png.into()).is_some());
    }

    #[test]
    fn broken_gifs_dont_read() {
        let gif = tiny_gif();
        assert!(first_frame_png(&gif[..20]).is_none());
        assert!(first_frame_png(b"not a gif at all").is_none());
        let mut huge = gif.clone();
        huge[6..10].copy_from_slice(&[0xff, 0xff, 0xff, 0xff]);
        assert!(first_frame_png(&huge).is_none());
    }

    #[test]
    fn interlaced_rows_come_back_in_order() {
        assert_eq!(row_order(8, true), vec![0, 4, 2, 6, 1, 3, 5, 7]);
        assert_eq!(row_order(3, false), vec![0, 1, 2]);
    }
}
