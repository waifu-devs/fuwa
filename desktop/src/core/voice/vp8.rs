//! VP8 through libvpx, and the pictures on either side of it. Cameras and
//! shared screens travel as VP8 (the media part takes only Opus and VP8);
//! here they're encoded in the three simulcast sizes the web's
//! calls/video.ts sends, decoded, and turned into BGRA for the window.
//!
//! Pictures in between are I420: full-size luma, then the two chroma
//! planes at half the width and height, packed tight, as libvpx takes them.

#[cfg(not(any(windows, feature = "system-libvpx")))]
use shiguredo_libvpx as vpx;

#[cfg(any(windows, feature = "system-libvpx"))]
pub use super::libvpx::{Decoder, Encoder};

/// A picture in I420, packed: Y, then U, then V.
#[derive(Clone, Debug, Default)]
pub struct Yuv {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Yuv {
    /// A black picture this size (both even).
    pub fn new(width: u32, height: u32) -> Self {
        let mut yuv = Self::default();
        yuv.resize(width, height);
        yuv
    }

    /// Makes it this size, keeping its buffer when it can.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        let (y, c) = sizes(width, height);
        self.data.resize(y + 2 * c, 0);
    }

    pub fn planes(&self) -> (&[u8], &[u8], &[u8]) {
        let (y, c) = sizes(self.width, self.height);
        let (luma, chroma) = self.data.split_at(y);
        let (u, v) = chroma.split_at(c);
        (luma, u, &v[..c])
    }

    pub fn planes_mut(&mut self) -> (&mut [u8], &mut [u8], &mut [u8]) {
        let (y, c) = sizes(self.width, self.height);
        let (luma, chroma) = self.data.split_at_mut(y);
        let (u, v) = chroma.split_at_mut(c);
        (luma, u, &mut v[..c])
    }
}

/// The bytes of a picture's luma plane and of each chroma plane.
fn sizes(width: u32, height: u32) -> (usize, usize) {
    let (w, h) = (width as usize, height as usize);
    (w * h, w.div_ceil(2) * h.div_ceil(2))
}

/// A picture for the window: BGRA, four bytes a pixel, rows packed.
#[derive(Clone, Debug, Default)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

// ───────────────────────── Colors ─────────────────────────

fn clamp(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// Writes an I420 picture (planes with their strides) as BGRA, BT.601 as
/// VP8 has it, flipped left to right with `mirror`. `out` is reused.
#[allow(clippy::too_many_arguments)]
pub fn to_bgra(
    (y, y_stride): (&[u8], usize),
    (u, u_stride): (&[u8], usize),
    (v, v_stride): (&[u8], usize),
    width: u32,
    height: u32,
    mirror: bool,
    out: &mut Vec<u8>,
) {
    let (w, h) = (width as usize, height as usize);
    out.resize(w * h * 4, 0);
    for (row, line) in out.chunks_exact_mut(w * 4).enumerate().take(h) {
        let ys = &y[row * y_stride..row * y_stride + w];
        let us = &u[(row / 2) * u_stride..];
        let vs = &v[(row / 2) * v_stride..];
        for (x, &luma) in ys.iter().enumerate() {
            let c = 298 * (i32::from(luma) - 16);
            let d = i32::from(us[x / 2]) - 128;
            let e = i32::from(vs[x / 2]) - 128;
            let at = if mirror { (w - 1 - x) * 4 } else { x * 4 };
            let px = &mut line[at..at + 4];
            px[0] = clamp((c + 516 * d + 128) >> 8);
            px[1] = clamp((c - 100 * d - 208 * e + 128) >> 8);
            px[2] = clamp((c + 409 * e + 128) >> 8);
            px[3] = 255;
        }
    }
}

/// An I420 picture as BGRA.
pub fn yuv_to_bgra(yuv: &Yuv, mirror: bool, out: &mut Vec<u8>) {
    let (y, u, v) = yuv.planes();
    let cw = yuv.width.div_ceil(2) as usize;
    to_bgra((y, yuv.width as usize), (u, cw), (v, cw), yuv.width, yuv.height, mirror, out);
}

/// Where each color sits in a pixel of 3 or 4 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub bytes: usize,
    pub r: usize,
    pub g: usize,
    pub b: usize,
}

impl Layout {
    pub const BGRA: Self = Self { bytes: 4, r: 2, g: 1, b: 0 };
    pub const RGBA: Self = Self { bytes: 4, r: 0, g: 1, b: 2 };
    pub const XBGR: Self = Self { bytes: 4, r: 3, g: 2, b: 1 };
    pub const RGB: Self = Self { bytes: 3, r: 0, g: 1, b: 2 };
    pub const BGR: Self = Self { bytes: 3, r: 2, g: 1, b: 0 };
}

/// Fills `out` (already sized, even) from packed RGB-like pixels, `stride`
/// bytes a row, scaling the source to fit if it's another size.
pub fn from_rgb(src: &[u8], stride: usize, width: u32, height: u32, layout: Layout, out: &mut Yuv) {
    let (sw, sh) = (width as usize, height as usize);
    let (dw, dh) = (out.width as usize, out.height as usize);
    let cw = dw.div_ceil(2);
    let (ys, us, vs) = out.planes_mut();
    let pixel = |x: usize, y: usize| -> (i32, i32, i32) {
        let (x, y) = (x * sw / dw, y * sh / dh);
        let at = y * stride + x * layout.bytes;
        match src.get(at..at + layout.bytes) {
            Some(p) => (i32::from(p[layout.r]), i32::from(p[layout.g]), i32::from(p[layout.b])),
            None => (0, 0, 0),
        }
    };
    for cy in 0..dh.div_ceil(2) {
        for cx in 0..cw {
            let (mut rs, mut gs, mut bs) = (0, 0, 0);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (x, y) = ((cx * 2 + dx).min(dw - 1), (cy * 2 + dy).min(dh - 1));
                let (r, g, b) = pixel(x, y);
                ys[y * dw + x] = clamp(((66 * r + 129 * g + 25 * b + 128) >> 8) + 16);
                rs += r;
                gs += g;
                bs += b;
            }
            let (r, g, b) = (rs / 4, gs / 4, bs / 4);
            us[cy * cw + cx] = clamp(((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128);
            vs[cy * cw + cx] = clamp(((112 * r - 94 * g - 18 * b + 128) >> 8) + 128);
        }
    }
}

/// Fills `out` from YUYV (YUY2) pixels, scaling to fit.
pub fn from_yuyv(src: &[u8], width: u32, height: u32, out: &mut Yuv) {
    let (sw, sh) = (width as usize, height as usize);
    let (dw, dh) = (out.width as usize, out.height as usize);
    let cw = dw.div_ceil(2);
    let stride = sw * 2;
    let (ys, us, vs) = out.planes_mut();
    for y in 0..dh {
        let sy = y * sh / dh;
        for x in 0..dw {
            let sx = x * sw / dw;
            ys[y * dw + x] = src.get(sy * stride + sx * 2).copied().unwrap_or(16);
        }
    }
    for cy in 0..dh.div_ceil(2) {
        let sy = (cy * 2) * sh / dh;
        for cx in 0..cw {
            let sx = ((cx * 2) * sw / dw) & !1;
            let at = sy * stride + sx * 2;
            us[cy * cw + cx] = src.get(at + 1).copied().unwrap_or(128);
            vs[cy * cw + cx] = src.get(at + 3).copied().unwrap_or(128);
        }
    }
}

/// Fills `out` from NV12 (luma, then interleaved U and V), scaling to fit.
pub fn from_nv12(src: &[u8], width: u32, height: u32, out: &mut Yuv) {
    let (sw, sh) = (width as usize, height as usize);
    let (luma, chroma) = src.split_at((sw * sh).min(src.len()));
    let mut u = vec![0u8; sw.div_ceil(2) * sh.div_ceil(2)];
    let mut v = u.clone();
    for (n, pair) in chroma.as_chunks::<2>().0.iter().take(u.len()).enumerate() {
        u[n] = pair[0];
        v[n] = pair[1];
    }
    scale_planes((luma, &u, &v), width, height, out);
}

/// Scales three I420 planes (packed) into `out`, whatever its size.
pub fn scale_planes((y, u, v): (&[u8], &[u8], &[u8]), width: u32, height: u32, out: &mut Yuv) {
    let (sw, sh) = (width as usize, height as usize);
    let (dw, dh) = (out.width as usize, out.height as usize);
    let (oy, ou, ov) = out.planes_mut();
    resize_plane(y, sw, sh, oy, dw, dh);
    resize_plane(u, sw.div_ceil(2), sh.div_ceil(2), ou, dw.div_ceil(2), dh.div_ceil(2));
    resize_plane(v, sw.div_ceil(2), sh.div_ceil(2), ov, dw.div_ceil(2), dh.div_ceil(2));
}

/// Scales one plane: halving by averaging each 2×2 block, anything else
/// bilinear.
fn resize_plane(src: &[u8], sw: usize, sh: usize, dst: &mut [u8], dw: usize, dh: usize) {
    if sw == 0 || sh == 0 || src.len() < sw * sh {
        dst.fill(0);
        return;
    }
    if sw == dw && sh == dh {
        dst[..dw * dh].copy_from_slice(&src[..dw * dh]);
        return;
    }
    if dw * 2 == sw && dh * 2 == sh {
        for y in 0..dh {
            let (a, b) = (&src[y * 2 * sw..], &src[(y * 2 + 1) * sw..]);
            for x in 0..dw {
                let sum = u32::from(a[x * 2]) + u32::from(a[x * 2 + 1]) + u32::from(b[x * 2]) + u32::from(b[x * 2 + 1]);
                dst[y * dw + x] = ((sum + 2) / 4) as u8;
            }
        }
        return;
    }
    // Fixed point, 8 bits of fraction.
    let fx = ((sw as u64) << 8) / dw.max(1) as u64;
    let fy = ((sh as u64) << 8) / dh.max(1) as u64;
    for y in 0..dh {
        let at = (y as u64 * fy) as usize;
        let (y0, wy) = ((at >> 8).min(sh - 1), (at & 255) as u32);
        let y1 = (y0 + 1).min(sh - 1);
        for x in 0..dw {
            let at = (x as u64 * fx) as usize;
            let (x0, wx) = ((at >> 8).min(sw - 1), (at & 255) as u32);
            let x1 = (x0 + 1).min(sw - 1);
            let p = |x: usize, y: usize| u32::from(src[y * sw + x]);
            let top = p(x0, y0) * (256 - wx) + p(x1, y0) * wx;
            let bottom = p(x0, y1) * (256 - wx) + p(x1, y1) * wx;
            dst[y * dw + x] = ((top * (256 - wy) + bottom * wy + 32768) >> 16) as u8;
        }
    }
}

/// Half of a picture each way (both halves even).
pub fn half(src: &Yuv, out: &mut Yuv) {
    out.resize((src.width / 2) & !1, (src.height / 2) & !1);
    scale_planes(src.planes(), src.width, src.height, out);
}

/// The size a picture `width`×`height` goes out at: no bigger than
/// `most_w`×`most_h` either way, keeping its shape, in steps of 4 so its
/// half and quarter come out even.
pub fn fit(width: u32, height: u32, most_w: u32, most_h: u32) -> (u32, u32) {
    let (w, h) = (width.max(4) as f64, height.max(4) as f64);
    let k = (most_w as f64 / w).min(most_h as f64 / h).min(1.0);
    let w = ((w * k) as u32 & !3).max(4);
    let h = ((h * k) as u32 & !3).max(4);
    (w, h)
}

// ───────────────────────── Encoding ─────────────────────────

/// One of the simulcast sizes: its rid, how much smaller than full, and its
/// caps (a camera's bitrate is 0: it comes from the picture's size).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    pub rid: &'static str,
    pub divide: u32,
    pub bitrate: u32,
    pub fps: u32,
}

/// A shared screen's three sizes at the default quality, with more bits for text.
pub const SCREEN: [Size; 3] = screen_sizes(Share::DEFAULT);

/// How sharp and smooth a shared screen goes out (the web's `lib/screen-share.ts`):
/// the full size's height at most (16:9 fits in it) and its frame rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Share {
    pub height: u32,
    pub fps: u32,
}

impl Share {
    pub const HEIGHTS: [u32; 3] = [720, 1080, 1440];
    pub const FPS: [u32; 3] = [15, 30, 60];
    /// What sharing used before there was a choice: 1080p at 30.
    pub const DEFAULT: Self = Self { height: 1080, fps: 30 };

    /// A saved choice read back, anything unknown as the default.
    pub fn new(height: u32, fps: u32) -> Self {
        Self {
            height: if Self::HEIGHTS.contains(&height) { height } else { Self::DEFAULT.height },
            fps: if Self::FPS.contains(&fps) { fps } else { Self::DEFAULT.fps },
        }
    }

    /// The box the full size fits in.
    pub fn most(self) -> (u32, u32) {
        (self.height * 16 / 9, self.height)
    }

    /// About how much upload it takes, in Mbit/s, every size together.
    pub fn mbps(self) -> f32 {
        let bits: u32 = screen_sizes(self).iter().map(|s| s.bitrate).sum();
        (bits / 100_000) as f32 / 10.0
    }
}

/// A shared screen's three sizes: a quarter for thumbnails, half, and full
/// at the frame rate picked, with the web's bitrates (`screenEncodings`).
pub const fn screen_sizes(share: Share) -> [Size; 3] {
    let top = match (share.height, share.fps) {
        (720, 15) => 1_200_000,
        (720, 30) => 1_800_000,
        (720, _) => 2_800_000,
        (1440, 15) => 3_000_000,
        (1440, 30) => 4_500_000,
        (1440, _) => 6_000_000,
        (_, 15) => 1_800_000,
        (_, 30) => 2_500_000,
        (_, _) => 4_000_000,
    };
    let middle = if share.height == 1440 { 1_000_000 } else { 700_000 };
    [
        Size { rid: "l", divide: 4, bitrate: 200_000, fps: 5 },
        Size { rid: "m", divide: 2, bitrate: middle, fps: 15 },
        Size { rid: "h", divide: 1, bitrate: top, fps: share.fps },
    ]
}

/// One frame out of the encoder.
#[derive(Clone, Debug)]
pub struct Encoded {
    pub data: Vec<u8>,
    pub keyframe: bool,
}

/// A VP8 encoder for one size, at a steady bitrate, made for calls: as
/// fast as it must be, no frames held back.
#[cfg(not(any(windows, feature = "system-libvpx")))]
pub struct Encoder {
    inner: vpx::Encoder,
    pub width: u32,
    pub height: u32,
}

#[cfg(not(any(windows, feature = "system-libvpx")))]
impl Encoder {
    pub fn new(width: u32, height: u32, bitrate: u32, fps: u32, screen: bool) -> Result<Self, String> {
        let codec = vpx::CodecConfig::Vp8(vpx::Vp8Config {
            // Screens change little between frames: what stays still costs nothing.
            static_threshold: screen.then_some(100),
            max_intra_bitrate_pct: Some(if screen { 600 } else { 300 }),
            ..Default::default()
        });
        let mut config = vpx::EncoderConfig::new(width as usize, height as usize, vpx::ImageFormat::I420, codec);
        config.fps_numerator = fps.max(1) as usize;
        config.fps_denominator = 1;
        config.target_bitrate = bitrate as usize;
        config.min_quantizer = 2;
        config.max_quantizer = 56;
        // Positive in real time: libvpx picks its speed by how long frames take.
        config.cpu_used = Some(8);
        config.deadline = vpx::EncodingDeadline::Realtime;
        config.rate_control = vpx::RateControlMode::Cbr;
        config.error_resilient = true;
        // Keyframes come when asked (a viewer starting or switching), and every 10 s anyway.
        config.keyframe_interval = std::num::NonZeroUsize::new((fps * 10) as usize);
        config.threads = std::num::NonZeroUsize::new(if width * height >= 1280 * 720 { 4 } else { 1 });
        let inner = vpx::Encoder::new(config).map_err(|e| e.to_string())?;
        Ok(Self { inner, width, height })
    }

    /// Encodes one picture this encoder's size.
    pub fn encode(&mut self, yuv: &Yuv, keyframe: bool) -> Vec<Encoded> {
        let (y, u, v) = yuv.planes();
        let image = vpx::ImageData::I420 { y, u, v };
        let mut out = Vec::new();
        if self.inner.encode(&image, &vpx::EncodeOptions { force_keyframe: keyframe }).is_err() {
            return out;
        }
        while let Some(frame) = self.inner.next_frame() {
            out.push(Encoded { data: frame.data().to_vec(), keyframe: frame.is_keyframe() });
        }
        out
    }
}

// ───────────────────────── Decoding ─────────────────────────

/// A VP8 decoder that hands back pictures as BGRA.
#[cfg(not(any(windows, feature = "system-libvpx")))]
pub struct Decoder {
    inner: vpx::Decoder,
}

#[cfg(not(any(windows, feature = "system-libvpx")))]
impl Decoder {
    pub fn new() -> Result<Self, String> {
        let inner = vpx::Decoder::new(vpx::DecoderConfig::new(vpx::DecoderCodec::Vp8)).map_err(|e| e.to_string())?;
        Ok(Self { inner })
    }

    /// Decodes one frame into `out` (BGRA, its buffer reused), giving its
    /// size; None when it didn't decode or held no picture.
    pub fn decode(&mut self, frame: &[u8], out: &mut Vec<u8>) -> Option<(u32, u32)> {
        self.inner.decode(frame).ok()?;
        let mut size = None;
        while let Ok(Some(picture)) = self.inner.next_frame() {
            if picture.is_high_depth() {
                continue;
            }
            let (w, h) = (picture.width() as u32, picture.height() as u32);
            to_bgra(
                (picture.y_plane(), picture.y_stride()),
                (picture.u_plane(), picture.u_stride()),
                (picture.v_plane(), picture.v_stride()),
                w,
                h,
                false,
                out,
            );
            size = Some((w, h));
        }
        size
    }
}

/// Whether a VP8 frame is a keyframe: its first bit is 0 on one.
pub fn is_keyframe(frame: &[u8]) -> bool {
    frame.first().is_some_and(|b| b & 1 == 0)
}

/// A keyframe's picture size, from its header.
pub fn keyframe_size(frame: &[u8]) -> Option<(u32, u32)> {
    if !is_keyframe(frame) || frame.len() < 10 || frame[3..6] != [0x9d, 0x01, 0x2a] {
        return None;
    }
    let w = u16::from_le_bytes([frame[6], frame[7]]) & 0x3fff;
    let h = u16::from_le_bytes([frame[8], frame[9]]) & 0x3fff;
    Some((u32::from(w), u32::from(h)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_shares_match_the_web() {
        assert_eq!(SCREEN.map(|s| (s.bitrate, s.fps)), [(200_000, 5), (700_000, 15), (2_500_000, 30)]);
        assert_eq!(screen_sizes(Share { height: 1440, fps: 60 })[2].bitrate, 6_000_000);
        assert_eq!(screen_sizes(Share { height: 720, fps: 15 })[2].bitrate, 1_200_000);
        for height in Share::HEIGHTS {
            for fps in Share::FPS {
                let sizes = screen_sizes(Share { height, fps });
                assert!(sizes.iter().map(|s| s.bitrate).sum::<u32>() <= 7_500_000);
                assert_eq!(sizes[2].fps, fps);
            }
        }
        assert_eq!(Share::DEFAULT.mbps(), 3.4);
        assert_eq!(Share::new(4320, 7), Share::DEFAULT);
        assert_eq!(Share { height: 1440, fps: 60 }.most(), (2560, 1440));
    }

    /// A picture with something in it: a gradient, and a bright square.
    fn pattern(w: u32, h: u32) -> Yuv {
        let mut yuv = Yuv::new(w, h);
        let (y, u, v) = yuv.planes_mut();
        for row in 0..h as usize {
            for col in 0..w as usize {
                let lit = (row / 16 + col / 16) % 2 == 0;
                y[row * w as usize + col] = if lit { 200 } else { 40 + (col % 64) as u8 };
            }
        }
        u.fill(90);
        v.fill(170);
        yuv
    }

    #[test]
    fn a_frame_the_desktop_encodes_decodes() {
        let yuv = pattern(320, 180);
        let mut encoder = Encoder::new(320, 180, 500_000, 30, false).unwrap();
        let first = encoder.encode(&yuv, true);
        assert_eq!(first.len(), 1);
        assert!(first[0].keyframe && is_keyframe(&first[0].data));
        assert_eq!(keyframe_size(&first[0].data), Some((320, 180)));
        let next = encoder.encode(&yuv, false);
        assert!(!next[0].keyframe && !is_keyframe(&next[0].data));

        let mut decoder = Decoder::new().unwrap();
        let mut bgra = Vec::new();
        assert_eq!(decoder.decode(&first[0].data, &mut bgra), Some((320, 180)));
        assert_eq!(bgra.len(), 320 * 180 * 4);
        // The square at the top left is bright, the one beside it dark.
        let at = |x: usize, y: usize| bgra[(y * 320 + x) * 4 + 1];
        assert!(at(4, 4) > at(20, 4) + 60, "{} {}", at(4, 4), at(20, 4));
        assert_eq!(decoder.decode(&next[0].data, &mut bgra), Some((320, 180)));
        assert!(decoder.decode(b"not vp8 at all", &mut bgra).is_none());
    }

    #[test]
    fn sizes_keep_their_shape_and_halve_evenly() {
        assert_eq!(fit(1920, 1080, 1280, 720), (1280, 720));
        assert_eq!(fit(640, 480, 1280, 720), (640, 480));
        assert_eq!(fit(2560, 1600, 1920, 1080), (1728, 1080));
        let (w, h) = fit(1366, 768, 1920, 1080);
        assert_eq!((w % 4, h % 4), (0, 0));
        let mut out = Yuv::default();
        half(&pattern(640, 360), &mut out);
        assert_eq!((out.width, out.height), (320, 180));
        assert_eq!(out.data.len(), 320 * 180 * 3 / 2);
    }

    #[test]
    fn colors_go_back_and_forth() {
        let mut bgra = vec![];
        for _ in 0..16 {
            bgra.extend_from_slice(&[30, 140, 220, 255]);
        }
        let mut yuv = Yuv::new(4, 4);
        from_rgb(&bgra, 16, 4, 4, Layout::BGRA, &mut yuv);
        let mut back = Vec::new();
        yuv_to_bgra(&yuv, false, &mut back);
        for (a, b) in back[..3].iter().zip([30u8, 140, 220]) {
            assert!(a.abs_diff(b) <= 3, "{back:?}");
        }
        // Mirrored, a row reads the other way.
        let mut row = Yuv::new(2, 2);
        row.planes_mut().0.copy_from_slice(&[16, 235, 16, 235]);
        row.planes_mut().1.fill(128);
        row.planes_mut().2.fill(128);
        let mut out = Vec::new();
        yuv_to_bgra(&row, true, &mut out);
        assert_eq!((out[0], out[4]), (255, 0));
    }
}
