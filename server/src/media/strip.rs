//! Takes what a picture says about where and how it was taken out of it
//! before it's kept: a phone photo's EXIF carries its GPS position, the
//! camera, and when it was taken, and XMP and text chunks can carry the same.
//! Pictures are public at their links, so none of that should go with them.
//!
//! Only the containers are rewritten, never the picture itself: JPEG drops
//! APP1 (EXIF, XMP), APP3 to APP13, APP15, comments, APP2 other than its
//! colour profile, and anything after the end of the picture (where phones
//! append extra images with their own EXIF); PNG drops eXIf, tEXt, iTXt, zTXt
//! and tIME, and anything after IEND; WebP drops its EXIF and XMP chunks.
//! Colour profiles, Adobe's colour transform and animation stay.

use std::io::{self, Read, Seek, SeekFrom, Write};

fn bad(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why.to_string())
}

fn byte(r: &mut impl Read) -> io::Result<Option<u8>> {
    let mut b = [0u8; 1];
    match r.read(&mut b)? {
        0 => Ok(None),
        _ => Ok(Some(b[0])),
    }
}

/// Copies `r` to `w` without the picture's metadata. `kind` is the type
/// [`super::sniff`] found; types this doesn't rewrite are an error.
pub fn strip(kind: &str, r: &mut impl Read, w: &mut (impl Write + Seek)) -> io::Result<()> {
    match kind {
        "image/jpeg" => jpeg(r, w),
        "image/png" => png(r, w),
        "image/webp" => webp(r, w),
        _ => Err(bad("not a type metadata is taken out of")),
    }
}

/// Whether a JPEG segment stays.
fn keep_jpeg(marker: u8, data: &[u8]) -> bool {
    match marker {
        // JFIF, and Adobe's (it says how the colours were stored).
        0xe0 | 0xee => true,
        // APP2 is the colour profile, or MPF, which points at the extra
        // images left out below.
        0xe2 => data.starts_with(b"ICC_PROFILE\0"),
        0xe1 | 0xe3..=0xef | 0xfe => false,
        _ => true,
    }
}

fn jpeg(r: &mut impl Read, w: &mut impl Write) -> io::Result<()> {
    let mut soi = [0u8; 2];
    r.read_exact(&mut soi)?;
    if soi != [0xff, 0xd8] {
        return Err(bad("not a JPEG"));
    }
    w.write_all(&soi)?;
    let mut pending: Option<u8> = None;
    loop {
        let marker = match pending.take() {
            Some(marker) => marker,
            None => {
                if byte(r)?.ok_or_else(|| bad("cut off before its end"))? != 0xff {
                    return Err(bad("a JPEG segment doesn't start with a marker"));
                }
                let mut marker = byte(r)?.ok_or_else(|| bad("cut off before its end"))?;
                while marker == 0xff {
                    marker = byte(r)?.ok_or_else(|| bad("cut off before its end"))?;
                }
                marker
            }
        };
        match marker {
            // The end: what phones append after it (more pictures, each with
            // its own EXIF) is left out.
            0xd9 => {
                w.write_all(&[0xff, 0xd9])?;
                return Ok(());
            }
            0x01 | 0xd0..=0xd7 => {
                w.write_all(&[0xff, marker])?;
                continue;
            }
            _ => {}
        }
        let mut len = [0u8; 2];
        r.read_exact(&mut len)?;
        let len = u16::from_be_bytes(len) as usize;
        if len < 2 {
            return Err(bad("a JPEG segment is too short"));
        }
        let mut data = vec![0u8; len - 2];
        r.read_exact(&mut data)?;
        if keep_jpeg(marker, &data) {
            w.write_all(&[0xff, marker])?;
            w.write_all(&((len as u16).to_be_bytes()))?;
            w.write_all(&data)?;
        }
        if marker != 0xda {
            continue;
        }
        // The compressed picture after a start of scan runs until a marker
        // that isn't a stuffed 0xff or a restart.
        loop {
            let Some(b) = byte(r)? else {
                // Cut off inside the picture: what came is kept, as
                // browsers show it.
                return Ok(());
            };
            if b != 0xff {
                w.write_all(&[b])?;
                continue;
            }
            let mut next = byte(r)?;
            while next == Some(0xff) {
                next = byte(r)?;
            }
            match next {
                None => return Ok(()),
                Some(n @ (0x00 | 0xd0..=0xd7)) => w.write_all(&[0xff, n])?,
                Some(n) => {
                    pending = Some(n);
                    break;
                }
            }
        }
    }
}

const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

fn png(r: &mut impl Read, w: &mut impl Write) -> io::Result<()> {
    let mut signature = [0u8; 8];
    r.read_exact(&mut signature)?;
    if &signature != PNG_SIGNATURE {
        return Err(bad("not a PNG"));
    }
    w.write_all(&signature)?;
    loop {
        let mut head = [0u8; 8];
        r.read_exact(&mut head)?;
        let len = u32::from_be_bytes(head[..4].try_into().expect("four bytes"));
        if len > 0x7fff_ffff {
            return Err(bad("a PNG chunk is too long"));
        }
        let kind: [u8; 4] = head[4..].try_into().expect("four bytes");
        // Its data, then its CRC.
        let mut rest = r.by_ref().take(u64::from(len) + 4);
        if matches!(&kind, b"eXIf" | b"tEXt" | b"iTXt" | b"zTXt" | b"tIME") {
            io::copy(&mut rest, &mut io::sink())?;
        } else {
            w.write_all(&head)?;
            io::copy(&mut rest, w)?;
        }
        if rest.limit() != 0 {
            return Err(bad("cut off before its end"));
        }
        if &kind == b"IEND" {
            return Ok(());
        }
    }
}

fn webp(r: &mut impl Read, w: &mut (impl Write + Seek)) -> io::Result<()> {
    let mut head = [0u8; 12];
    r.read_exact(&mut head)?;
    if &head[..4] != b"RIFF" || &head[8..] != b"WEBP" {
        return Err(bad("not a WebP"));
    }
    let size = u64::from(u32::from_le_bytes(head[4..8].try_into().expect("four bytes")));
    let start = w.stream_position()?;
    w.write_all(&head)?;
    let mut body = r.by_ref().take(size.saturating_sub(4));
    let mut written: u64 = 4;
    loop {
        let mut chunk = [0u8; 8];
        match body.read(&mut chunk[..1])? {
            0 => break,
            _ => body.read_exact(&mut chunk[1..])?,
        }
        let len = u64::from(u32::from_le_bytes(chunk[4..].try_into().expect("four bytes")));
        let padded = len + (len & 1);
        let fourcc = &chunk[..4];
        if fourcc == b"EXIF" || fourcc == b"XMP " {
            let skipped = io::copy(&mut body.by_ref().take(padded), &mut io::sink())?;
            if skipped < len {
                return Err(bad("cut off before its end"));
            }
            continue;
        }
        w.write_all(&chunk)?;
        let mut data = body.by_ref().take(padded);
        if fourcc == b"VP8X" {
            // Its flags say whether EXIF and XMP chunks follow.
            let mut flags = [0u8; 1];
            data.read_exact(&mut flags)?;
            w.write_all(&[flags[0] & !(0x08 | 0x04)])?;
        }
        io::copy(&mut data, w)?;
        if data.limit() > padded - len {
            return Err(bad("cut off before its end"));
        }
        // A last chunk can leave out its padding byte.
        if data.limit() == 1 {
            w.write_all(&[0])?;
        }
        written += 8 + padded;
    }
    let end = w.stream_position()?;
    w.seek(SeekFrom::Start(start + 4))?;
    w.write_all(&u32::try_from(written).map_err(|_| bad("too big"))?.to_le_bytes())?;
    w.seek(SeekFrom::Start(end))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn run(kind: &str, input: &[u8]) -> io::Result<Vec<u8>> {
        let mut out = Cursor::new(Vec::new());
        strip(kind, &mut Cursor::new(input), &mut out)?;
        Ok(out.into_inner())
    }

    fn segment(marker: u8, data: &[u8]) -> Vec<u8> {
        let mut s = vec![0xff, marker];
        s.extend_from_slice(&((data.len() + 2) as u16).to_be_bytes());
        s.extend_from_slice(data);
        s
    }

    #[test]
    fn jpegs_lose_exif_xmp_comments_and_what_comes_after_them() {
        let jfif = segment(0xe0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
        let exif = segment(0xe1, b"Exif\0\0GPS 35.68N 139.76E");
        let xmp = segment(0xe1, b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta/>");
        let icc = segment(0xe2, b"ICC_PROFILE\0\x01\x01profile");
        let mpf = segment(0xe2, b"MPF\0pointers");
        let comment = segment(0xfe, b"taken at home");
        let adobe = segment(0xee, b"Adobe\0\x64\0\0\0\0\x01");
        let iptc = segment(0xed, b"Photoshop 3.0\0city");
        let quant = segment(0xdb, &[0; 65]);
        let scan = segment(0xda, &[1, 1, 0, 0, 0x3f, 0]);
        // Compressed data with a stuffed 0xff and a restart marker in it.
        let entropy = [0x12, 0xff, 0x00, 0x34, 0xff, 0xd0, 0x56];
        let trailer = [&[0xff, 0xd8][..], &segment(0xe1, b"Exif\0\0GPS again"), &[0xff, 0xd9]].concat();
        let input = [
            &[0xff, 0xd8][..],
            &jfif,
            &exif,
            &xmp,
            &icc,
            &mpf,
            &comment,
            &adobe,
            &iptc,
            &quant,
            &scan,
            &entropy,
            &[0xff, 0xd9],
            &trailer,
        ]
        .concat();
        let out = run("image/jpeg", &input).unwrap();
        let expected = [&[0xff, 0xd8][..], &jfif, &icc, &adobe, &quant, &scan, &entropy, &[0xff, 0xd9]].concat();
        assert_eq!(out, expected);
        let text = String::from_utf8_lossy(&out);
        assert!(!text.contains("GPS") && !text.contains("home") && !text.contains("city"));
    }

    #[test]
    fn progressive_jpegs_keep_every_scan() {
        let scan = segment(0xda, &[1, 1, 0, 0, 0x3f, 0]);
        let huffman = segment(0xc4, &[0; 20]);
        let input = [
            &[0xff, 0xd8][..],
            &scan,
            &[0x01, 0x02],
            &huffman,
            &scan,
            &[0x03, 0xff, 0x00],
            &segment(0xe1, b"Exif\0\0late"),
            &[0xff, 0xd9],
        ]
        .concat();
        let out = run("image/jpeg", &input).unwrap();
        let expected =
            [&[0xff, 0xd8][..], &scan, &[0x01, 0x02], &huffman, &scan, &[0x03, 0xff, 0x00], &[0xff, 0xd9]].concat();
        assert_eq!(out, expected);
    }

    #[test]
    fn broken_jpegs_are_refused_before_their_picture_starts() {
        assert!(run("image/jpeg", &[0xff, 0xd8, 0x00]).is_err());
        assert!(run("image/jpeg", &[0xff, 0xd8, 0xff, 0xe1, 0x00, 0x10, b'E']).is_err());
        // Cut off inside the compressed picture: what came is kept.
        let cut = [&[0xff, 0xd8][..], &segment(0xda, &[1, 1, 0, 0, 0x3f, 0]), &[0x12, 0x34]].concat();
        assert_eq!(run("image/jpeg", &cut).unwrap(), cut);
    }

    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut c = (data.len() as u32).to_be_bytes().to_vec();
        c.extend_from_slice(kind);
        c.extend_from_slice(data);
        c.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
        c
    }

    #[test]
    fn pngs_lose_text_exif_and_time() {
        let ihdr = chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]);
        let iccp = chunk(b"iCCP", b"sRGB\0\0profile");
        let idat = chunk(b"IDAT", &[1, 2, 3]);
        let iend = chunk(b"IEND", &[]);
        let input = [
            &PNG_SIGNATURE[..],
            &ihdr,
            &chunk(b"eXIf", b"MM\0*GPS"),
            &iccp,
            &chunk(b"tEXt", b"Comment\0at home"),
            &chunk(b"iTXt", b"XML:com.adobe.xmp\0\0\0\0\0<x/>"),
            &chunk(b"zTXt", b"Raw\0\0zz"),
            &chunk(b"tIME", &[7, 234, 10, 3, 12, 0, 0]),
            &idat,
            &iend,
            b"trailing EXIF",
        ]
        .concat();
        let out = run("image/png", &input).unwrap();
        assert_eq!(out, [&PNG_SIGNATURE[..], &ihdr, &iccp, &idat, &iend].concat());
        assert!(run("image/png", &[&PNG_SIGNATURE[..], &ihdr[..10]].concat()).is_err());
    }

    fn riff(chunks: &[Vec<u8>]) -> Vec<u8> {
        let body: Vec<u8> = chunks.concat();
        [&b"RIFF"[..], &((body.len() + 4) as u32).to_le_bytes(), b"WEBP", &body].concat()
    }

    fn webp_chunk(fourcc: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut c = fourcc.to_vec();
        c.extend_from_slice(&(data.len() as u32).to_le_bytes());
        c.extend_from_slice(data);
        if data.len() % 2 == 1 {
            c.push(0);
        }
        c
    }

    #[test]
    fn webps_lose_exif_and_xmp_and_say_so() {
        let mut vp8x = vec![0x08 | 0x04 | 0x20, 0, 0, 0];
        vp8x.extend_from_slice(&[0xff, 0x07, 0, 0x37, 0x04, 0]);
        let iccp = webp_chunk(b"ICCP", b"profile");
        let image = webp_chunk(b"VP8L", &[0x2f, 1, 2, 3, 4]);
        let input = riff(&[
            webp_chunk(b"VP8X", &vp8x),
            iccp.clone(),
            image.clone(),
            webp_chunk(b"EXIF", b"MM\0*GPS 35N"),
            webp_chunk(b"XMP ", b"<x:xmpmeta/>"),
        ]);
        let out = run("image/webp", &[&input[..], b"after"].concat()).unwrap();
        let mut cleared = vp8x.clone();
        cleared[0] = 0x20;
        assert_eq!(out, riff(&[webp_chunk(b"VP8X", &cleared), iccp, image]));
        assert!(!String::from_utf8_lossy(&out).contains("GPS"));
    }

    #[test]
    fn other_types_are_left_to_the_caller() {
        assert!(run("image/gif", b"GIF89a").is_err());
    }
}
