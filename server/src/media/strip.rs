//! Takes what a picture says about where and how it was taken out of it
//! before it's kept: a phone photo's EXIF carries its GPS position, the
//! camera, and when it was taken, and XMP and text chunks can carry the same.
//! Pictures are public at their links, so none of that should go with them.
//!
//! Only the containers are rewritten, never the picture itself: JPEG drops
//! APP1 (EXIF, XMP), APP3 to APP13, APP15, comments, APP2 other than its
//! colour profile, and anything after the end of the picture (where phones
//! append extra images with their own EXIF); PNG drops eXIf, tEXt, iTXt, zTXt
//! and tIME, and anything after IEND; WebP drops its EXIF and XMP chunks;
//! GIF drops comments, application extensions other than looping and the
//! colour profile (XMP among them), and anything after its end. Colour
//! profiles, Adobe's colour transform and animation stay.
//!
//! AVIF keeps its layout, since its boxes point at each other by offset:
//! the bytes of its Exif and XMP items (and of an XMP `uuid` box) are
//! overwritten with zeros where they are, so the file stays the same size.

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
        "image/gif" => gif(r, w),
        "image/avif" => avif(r, w),
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

/// GIF's application extensions that stay: looping (Netscape's and the
/// older AnimExts) and the colour profile.
const GIF_KEPT_APPS: [&[u8; 11]; 3] = [b"NETSCAPE2.0", b"ANIMEXTS1.0", b"ICCRGBG1012"];

/// Copies (or, with `w` `None`, skips) GIF data sub-blocks up to and
/// including their empty terminator.
fn gif_blocks(r: &mut impl Read, mut w: Option<&mut dyn Write>) -> io::Result<()> {
    loop {
        let len = byte(r)?.ok_or_else(|| bad("cut off before its end"))?;
        let mut block = [0u8; 255];
        r.read_exact(&mut block[..usize::from(len)])?;
        if let Some(w) = w.as_deref_mut() {
            w.write_all(&[len])?;
            w.write_all(&block[..usize::from(len)])?;
        }
        if len == 0 {
            return Ok(());
        }
    }
}

/// Bytes of a GIF colour table the `flags` byte says follows, if any.
fn gif_table(flags: u8) -> usize {
    if flags & 0x80 == 0 { 0 } else { 3 << ((flags & 0x07) + 1) }
}

fn gif(r: &mut impl Read, w: &mut impl Write) -> io::Result<()> {
    // The header and the logical screen descriptor.
    let mut head = [0u8; 13];
    r.read_exact(&mut head)?;
    if &head[..6] != b"GIF87a" && &head[..6] != b"GIF89a" {
        return Err(bad("not a GIF"));
    }
    w.write_all(&head)?;
    io::copy(&mut r.by_ref().take(gif_table(head[10]) as u64), w)?;
    loop {
        match byte(r)?.ok_or_else(|| bad("cut off before its end"))? {
            // An image: its descriptor, colour table, LZW code size and data.
            0x2c => {
                let mut descriptor = [0u8; 10];
                descriptor[0] = 0x2c;
                r.read_exact(&mut descriptor[1..])?;
                w.write_all(&descriptor)?;
                let table = gif_table(descriptor[9]) as u64;
                if io::copy(&mut r.by_ref().take(table + 1), w)? != table + 1 {
                    return Err(bad("cut off before its end"));
                }
                gif_blocks(r, Some(w))?;
            }
            0x21 => {
                let label = byte(r)?.ok_or_else(|| bad("cut off before its end"))?;
                match label {
                    // Comments go.
                    0xfe => gif_blocks(r, None)?,
                    // Application extensions: their first block names them.
                    0xff => {
                        let len = byte(r)?.ok_or_else(|| bad("cut off before its end"))?;
                        let mut name = vec![0u8; usize::from(len)];
                        r.read_exact(&mut name)?;
                        if GIF_KEPT_APPS.iter().any(|kept| name == kept.as_slice()) {
                            w.write_all(&[0x21, 0xff, len])?;
                            w.write_all(&name)?;
                            gif_blocks(r, Some(w))?;
                        } else if len != 0 {
                            gif_blocks(r, None)?;
                        }
                    }
                    // Frame timing and plain text are part of the picture.
                    _ => {
                        w.write_all(&[0x21, label])?;
                        gif_blocks(r, Some(w))?;
                    }
                }
            }
            // The end: anything after it is left out.
            0x3b => {
                w.write_all(&[0x3b])?;
                return Ok(());
            }
            _ => return Err(bad("a GIF block of no kind")),
        }
    }
}

/// The most of an AVIF's `meta` box read to find its metadata.
const MAX_META: u64 = 16 << 20;
/// The `uuid` box XMP is sometimes kept in.
const XMP_UUID: [u8; 16] = *b"\xbe\x7a\xcf\xcb\x97\xa9\x42\xe8\x9c\x71\x99\x94\x91\xe3\xaf\xac";

/// Reads a big-endian number `size` bytes long (0, 4 or 8) from `data` at `*at`.
fn be(data: &[u8], at: &mut usize, size: usize) -> io::Result<u64> {
    let bytes = data.get(*at..*at + size).ok_or_else(|| bad("an AVIF box is cut off"))?;
    *at += size;
    Ok(bytes.iter().fold(0, |n, b| (n << 8) | u64::from(*b)))
}

/// An ISO box's header at `at` in `data`: its type, where its body starts
/// and where it ends.
fn iso_box(data: &[u8], at: usize) -> io::Result<([u8; 4], usize, usize)> {
    let mut pos = at;
    let size = be(data, &mut pos, 4)?;
    let kind: [u8; 4] = data.get(pos..pos + 4).ok_or_else(|| bad("an AVIF box is cut off"))?.try_into().expect("four");
    pos += 4;
    let end = match size {
        0 => data.len(),
        1 => at + usize::try_from(be(data, &mut pos, 8)?).map_err(|_| bad("an AVIF box is too big"))?,
        n => at + usize::try_from(n).map_err(|_| bad("an AVIF box is too big"))?,
    };
    if end < pos || end > data.len() {
        return Err(bad("an AVIF box is cut off"));
    }
    Ok((kind, pos, end))
}

/// Where (from the start of the file) an AVIF's Exif and XMP items are,
/// from its `meta` box (`meta` its body, after the box header, starting at
/// `meta_at` in the file).
fn avif_metadata(meta: &[u8], meta_at: u64) -> io::Result<Vec<(u64, u64)>> {
    // A full box: its version and flags, then boxes.
    let mut children = Vec::new();
    let mut at = 4;
    while at < meta.len() {
        let (kind, body, end) = iso_box(meta, at)?;
        children.push((kind, body, end));
        at = end;
    }
    let find = |name: &[u8; 4]| children.iter().find(|(kind, _, _)| kind == name).map(|&(_, body, end)| (body, end));

    // Which items are metadata: Exif, and XMP (a `mime` item of RDF).
    let mut items = Vec::new();
    if let Some((body, end)) = find(b"iinf") {
        let iinf = &meta[body..end];
        let mut at = 0;
        let version = be(iinf, &mut at, 1)?;
        at += 3;
        let count = be(iinf, &mut at, if version == 0 { 2 } else { 4 })?;
        for _ in 0..count {
            let (kind, body, end) = iso_box(iinf, at)?;
            at = end;
            if &kind != b"infe" {
                continue;
            }
            let infe = &iinf[body..end];
            let mut pos = 0;
            let version = be(infe, &mut pos, 1)?;
            if version < 2 {
                continue;
            }
            pos += 3;
            let id = be(infe, &mut pos, if version == 2 { 2 } else { 4 })?;
            pos += 2;
            let item_type = infe.get(pos..pos + 4).ok_or_else(|| bad("an AVIF box is cut off"))?;
            pos += 4;
            let is_xmp = item_type == b"mime" && {
                let rest = infe.get(pos..).unwrap_or_default();
                let name = rest.split(|b| *b == 0).nth(1).unwrap_or_default();
                name == b"application/rdf+xml"
            };
            if item_type == b"Exif" || is_xmp {
                items.push(id);
            }
        }
    }
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let idat = find(b"idat").map(|(body, _)| meta_at + body as u64);

    // Where those items' bytes are.
    let (body, end) = find(b"iloc").ok_or_else(|| bad("an AVIF's metadata has no location"))?;
    let iloc = &meta[body..end];
    let mut at = 0;
    let version = be(iloc, &mut at, 1)?;
    at += 3;
    let sizes = be(iloc, &mut at, 2)?;
    let (offset_size, length_size) = ((sizes >> 12) as usize, ((sizes >> 8) & 0xf) as usize);
    let (base_size, index_size) = (((sizes >> 4) & 0xf) as usize, (sizes & 0xf) as usize);
    let count = be(iloc, &mut at, if version < 2 { 2 } else { 4 })?;
    let mut ranges = Vec::new();
    for _ in 0..count {
        let id = be(iloc, &mut at, if version < 2 { 2 } else { 4 })?;
        let method = if version == 0 { 0 } else { be(iloc, &mut at, 2)? & 0xf };
        let reference = be(iloc, &mut at, 2)?;
        let base = be(iloc, &mut at, base_size)?;
        let extents = be(iloc, &mut at, 2)?;
        for _ in 0..extents {
            if version > 0 {
                be(iloc, &mut at, index_size)?;
            }
            let offset = be(iloc, &mut at, offset_size)?;
            let length = be(iloc, &mut at, length_size)?;
            if !items.contains(&id) {
                continue;
            }
            if reference != 0 {
                // Kept in another file, so not in this one.
                continue;
            }
            let start = match method {
                0 => base,
                1 => idat.ok_or_else(|| bad("an AVIF item points into an idat it hasn't"))? + base,
                _ => return Err(bad("an AVIF's metadata is made from other items")),
            }
            .checked_add(offset)
            .ok_or_else(|| bad("an AVIF item is out of place"))?;
            ranges.push((start, length));
        }
    }
    Ok(ranges)
}

fn avif(r: &mut impl Read, w: &mut (impl Write + Seek)) -> io::Result<()> {
    let start = w.stream_position()?;
    let mut at: u64 = 0;
    let mut ranges = Vec::new();
    loop {
        let mut head = [0u8; 8];
        match r.read(&mut head[..1])? {
            0 => break,
            _ => r.read_exact(&mut head[1..])?,
        }
        w.write_all(&head)?;
        let size = u64::from(u32::from_be_bytes(head[..4].try_into().expect("four bytes")));
        let kind: [u8; 4] = head[4..].try_into().expect("four bytes");
        let mut header = 8;
        let size = match size {
            // To the end of the file.
            0 => None,
            1 => {
                let mut large = [0u8; 8];
                r.read_exact(&mut large)?;
                w.write_all(&large)?;
                header = 16;
                Some(u64::from_be_bytes(large))
            }
            n => Some(n),
        };
        if at == 0 && &kind != b"ftyp" {
            return Err(bad("not an AVIF"));
        }
        let body = match size {
            Some(size) => size.checked_sub(header).ok_or_else(|| bad("an AVIF box is too short"))?,
            None => u64::MAX,
        };
        if &kind == b"meta" {
            if body > MAX_META {
                return Err(bad("an AVIF's meta box is too big"));
            }
            let mut meta = vec![0u8; body as usize];
            r.read_exact(&mut meta)?;
            w.write_all(&meta)?;
            ranges.extend(avif_metadata(&meta, at + header)?);
        } else if &kind == b"uuid" {
            let mut uuid = [0u8; 16];
            r.read_exact(&mut uuid)?;
            w.write_all(&uuid)?;
            let rest = io::copy(&mut r.by_ref().take(body.saturating_sub(16)), w)?;
            if uuid == XMP_UUID {
                ranges.push((at + header + 16, rest));
            }
            if size.is_some() && rest != body.saturating_sub(16) {
                return Err(bad("cut off before its end"));
            }
        } else {
            let copied = io::copy(&mut r.by_ref().take(body), w)?;
            if size.is_some() && copied != body {
                return Err(bad("cut off before its end"));
            }
        }
        match size {
            Some(size) => at += size,
            None => break,
        }
    }
    let end = w.stream_position()?;
    let length = end - start;
    let zeros = [0u8; 8192];
    for (from, len) in ranges {
        // An extent of length 0 runs to the end of the file.
        let to =
            if len == 0 { length } else { from.checked_add(len).ok_or_else(|| bad("an AVIF item is out of place"))? };
        if to > length || from > to {
            return Err(bad("an AVIF item is out of place"));
        }
        w.seek(SeekFrom::Start(start + from))?;
        let mut left = to - from;
        while left > 0 {
            let n = left.min(zeros.len() as u64) as usize;
            w.write_all(&zeros[..n])?;
            left -= n as u64;
        }
    }
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
    fn gifs_lose_comments_and_xmp_and_keep_looping() {
        let head = [&b"GIF89a"[..], &[1, 0, 1, 0, 0x80, 0, 0], &[0, 0, 0, 255, 255, 255]].concat();
        let looping = [&[0x21, 0xff, 11][..], b"NETSCAPE2.0", &[3, 1, 0, 0, 0]].concat();
        let timing = [0x21, 0xf9, 4, 0, 10, 0, 0, 0];
        let image = [&[0x2c, 0, 0, 0, 0, 1, 0, 1, 0, 0][..], &[2, 2, 0x4c, 0x01, 0]].concat();
        let comment = [&[0x21, 0xfe, 13][..], b"taken at home", &[0]].concat();
        let xmp = [&[0x21, 0xff, 11][..], b"XMP DataXMP", &[9], b"GPS 35.6N", &[0]].concat();
        let input = [&head[..], &looping, &comment, &timing, &xmp, &image, &[0x3b], b"after"].concat();
        let out = run("image/gif", &input).unwrap();
        assert_eq!(out, [&head[..], &looping, &timing, &image, &[0x3b]].concat());
        assert!(!String::from_utf8_lossy(&out).contains("GPS"));
        assert!(run("image/gif", &input[..input.len() - 20]).is_err(), "cut off");
        assert!(run("image/gif", b"GIF89a").is_err());
    }

    fn iso(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        [&((body.len() + 8) as u32).to_be_bytes()[..], kind, body].concat()
    }

    /// A small AVIF: item 1 the picture in `mdat`, item 2 Exif in `mdat`,
    /// item 3 XMP in `idat`.
    fn avif_file(exif: &[u8], xmp: &[u8]) -> Vec<u8> {
        let ftyp = iso(b"ftyp", b"avif\0\0\0\0avifmif1");
        let infe = |id: u16, kind: &[u8; 4], extra: &[u8]| {
            iso(b"infe", &[&[2, 0, 0, 0][..], &id.to_be_bytes(), &[0, 0], kind, b"\0", extra].concat())
        };
        let iinf = iso(
            b"iinf",
            &[
                &[0, 0, 0, 0, 0, 3][..],
                &infe(1, b"av01", b""),
                &infe(2, b"Exif", b""),
                &infe(3, b"mime", b"application/rdf+xml\0"),
            ]
            .concat(),
        );
        let idat = iso(b"idat", xmp);
        let picture = b"AV1 frame bytes";
        // The meta box's size doesn't depend on the offsets, so lay it out
        // once to learn where mdat starts.
        let meta_with = |mdat_at: u32| {
            let entry = |id: u16, method: u16, offset: u32, len: u32| {
                [
                    &id.to_be_bytes()[..],
                    &method.to_be_bytes(),
                    &[0, 0],
                    &[0, 1],
                    &offset.to_be_bytes(),
                    &len.to_be_bytes(),
                ]
                .concat()
            };
            let iloc = iso(
                b"iloc",
                &[
                    &[1, 0, 0, 0, 0x44, 0x00, 0, 3][..],
                    &entry(1, 0, mdat_at + 8, picture.len() as u32),
                    &entry(2, 0, mdat_at + 8 + picture.len() as u32, exif.len() as u32),
                    &entry(3, 1, 0, xmp.len() as u32),
                ]
                .concat(),
            );
            iso(b"meta", &[&[0, 0, 0, 0][..], &iso(b"hdlr", &[0; 24]), &iinf, &iloc, &idat].concat())
        };
        let mdat_at = (ftyp.len() + meta_with(0).len()) as u32;
        let mdat = iso(b"mdat", &[&picture[..], exif].concat());
        [ftyp, meta_with(mdat_at), mdat].concat()
    }

    #[test]
    fn avifs_keep_their_layout_with_exif_and_xmp_zeroed() {
        let exif = b"\0\0\0\0MM\0*GPS 35.68N";
        let xmp = b"<x:xmpmeta>GPS 35.68N</x:xmpmeta>";
        let input = avif_file(exif, xmp);
        let out = run("image/avif", &input).unwrap();
        assert_eq!(out.len(), input.len());
        assert_eq!(out, avif_file(&[0; 18], &[0; 33]));
        assert!(!String::from_utf8_lossy(&out).contains("GPS"));
        assert!(String::from_utf8_lossy(&out).contains("AV1 frame bytes"));

        // XMP in a uuid box goes the same way.
        let uuid = iso(b"uuid", &[&XMP_UUID[..], b"GPS here"].concat());
        let with_uuid = [&input[..], &uuid].concat();
        let out = run("image/avif", &with_uuid).unwrap();
        assert_eq!(out.len(), with_uuid.len());
        assert!(!String::from_utf8_lossy(&out).contains("GPS"));

        assert!(run("image/avif", &input[..input.len() - 3]).is_err(), "cut off");
        assert!(run("image/avif", &iso(b"mdat", b"x")).is_err(), "no ftyp");
    }
}
