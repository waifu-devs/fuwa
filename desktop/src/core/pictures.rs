//! Getting a picture ready to upload, as the web's `lib/pictures.ts`: what
//! each kind is cropped to, the crop itself (a center point and a zoom over
//! the image), and drawing the result. Pure, so the window only draws it.

/// What a picture is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PictureKind {
    Avatar,
    Banner,
    Icon,
}

/// The size a kind is saved at, and whether people see it round.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    pub width: u32,
    pub height: u32,
    pub round: bool,
}

impl PictureKind {
    /// The web's `PICTURE`.
    pub fn shape(self) -> Shape {
        match self {
            Self::Avatar => Shape { width: 512, height: 512, round: true },
            Self::Banner => Shape { width: 1500, height: 600, round: false },
            Self::Icon => Shape { width: 512, height: 512, round: false },
        }
    }

    /// Its name in the catalogs' keys (`workspace.picture.frame.avatar`).
    pub fn key(self) -> &'static str {
        match self {
            Self::Avatar => "avatar",
            Self::Banner => "banner",
            Self::Icon => "icon",
        }
    }
}

/// The largest zoom the cropper allows.
pub const MAX_ZOOM: f32 = 4.0;

/// A width and a height, in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

/// A crop: the image point at the frame's center, in image pixels, and a zoom of 1 or more.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crop {
    pub cx: f32,
    pub cy: f32,
    pub zoom: f32,
}

/// Where the image goes in the frame: its top-left corner and scale, in frame pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub x: f32,
    pub y: f32,
    pub scale: f32,
}

/// The scale that makes the image just cover the frame, at zoom 1.
pub fn cover_scale(image: Size, frame: Size) -> f32 {
    (frame.width / image.width).max(frame.height / image.height)
}

/// Keeps the frame inside the image: zoom within bounds, center far enough from the edges.
pub fn clamp_crop(crop: Crop, image: Size, frame: Size) -> Crop {
    let zoom = crop.zoom.clamp(1.0, MAX_ZOOM);
    let scale = cover_scale(image, frame) * zoom;
    let half_w = frame.width / 2.0 / scale;
    let half_h = frame.height / 2.0 / scale;
    let within = |v: f32, lo: f32, hi: f32| if lo > hi { (lo + hi) / 2.0 } else { v.clamp(lo, hi) };
    Crop { zoom, cx: within(crop.cx, half_w, image.width - half_w), cy: within(crop.cy, half_h, image.height - half_h) }
}

/// Where to draw the image in the frame for a crop.
pub fn placement(crop: Crop, image: Size, frame: Size) -> Placement {
    let scale = cover_scale(image, frame) * crop.zoom;
    Placement { x: frame.width / 2.0 - crop.cx * scale, y: frame.height / 2.0 - crop.cy * scale, scale }
}

/// Zooms by `factor` keeping the image point under `at` (in frame pixels) where it is.
pub fn zoom_around(crop: Crop, factor: f32, at: (f32, f32), image: Size, frame: Size) -> Crop {
    let before = placement(crop, image, frame);
    let px = (at.0 - before.x) / before.scale;
    let py = (at.1 - before.y) / before.scale;
    let zoom = (crop.zoom * factor).clamp(1.0, MAX_ZOOM);
    let scale = cover_scale(image, frame) * zoom;
    clamp_crop(
        Crop { zoom, cx: px - (at.0 - frame.width / 2.0) / scale, cy: py - (at.1 - frame.height / 2.0) / scale },
        image,
        frame,
    )
}

/// The crop a picture starts at: its middle, not zoomed.
pub fn centered(image: Size) -> Crop {
    Crop { cx: image.width / 2.0, cy: image.height / 2.0, zoom: 1.0 }
}

/// Whether the picture sits as it started.
pub fn at_start(crop: Crop, image: Size) -> bool {
    crop == centered(image)
}

/// The cropped part of a picture (`pixels`, four bytes each, `width` by
/// `height`) at the kind's size, as the web draws it on a canvas. The bytes
/// keep the order they came in (RGBA or BGRA).
pub fn cropped(pixels: &[u8], width: u32, height: u32, crop: Crop, kind: PictureKind, frame: Size) -> Vec<u8> {
    let image = Size { width: width as f32, height: height as f32 };
    let out = kind.shape();
    let scale = cover_scale(image, frame) * crop.zoom;
    let (sw, sh) = (frame.width / scale, frame.height / scale);
    let source = (crop.cx - sw / 2.0, crop.cy - sh / 2.0, sw, sh);
    resample(pixels, (width, height), source, (out.width, out.height))
}

/// A picture made to fit within `longest` on its longest side (for showing a
/// big one); small ones come back as they are.
pub fn fit(pixels: &[u8], width: u32, height: u32, longest: u32) -> (Vec<u8>, u32, u32) {
    if width.max(height) <= longest {
        return (pixels.to_vec(), width, height);
    }
    let (w, h) = if width >= height {
        (longest, (height * longest).div_ceil(width).max(1))
    } else {
        ((width * longest).div_ceil(height).max(1), longest)
    };
    let all = (0.0, 0.0, width as f32, height as f32);
    (resample(pixels, (width, height), all, (w, h)), w, h)
}

/// Draws the `source` rectangle (x, y, width, height in image pixels) of a
/// picture into `out` pixels: each one the average of what it covers when
/// shrinking, between its neighbours when growing. Colors are weighted by
/// how opaque they are, so see-through edges don't go dark.
fn resample(pixels: &[u8], (w, h): (u32, u32), source: (f32, f32, f32, f32), (ow, oh): (u32, u32)) -> Vec<u8> {
    let (w, h, ow, oh) = (w as usize, h as usize, ow as usize, oh as usize);
    if w == 0 || h == 0 || pixels.len() < w * h * 4 {
        return vec![0; ow * oh * 4];
    }
    let cols = taps(source.0, source.2 / ow as f32, ow, w);
    let rows = taps(source.1, source.3 / oh as f32, oh, h);
    // Across first, for just the rows some output row reads.
    let lo = rows.iter().flatten().map(|t| t.0).min().unwrap_or(0);
    let hi = rows.iter().flatten().map(|t| t.0).max().unwrap_or(0);
    let mut across = vec![0f32; (hi - lo + 1) * ow * 4];
    for y in lo..=hi {
        let line = &pixels[y * w * 4..(y + 1) * w * 4];
        for (x, taps) in cols.iter().enumerate() {
            let mut sum = [0f32; 4];
            for &(at, weight) in taps {
                let px = &line[at * 4..at * 4 + 4];
                let a = f32::from(px[3]) * weight;
                for c in 0..3 {
                    sum[c] += f32::from(px[c]) * a;
                }
                sum[3] += a;
            }
            across[((y - lo) * ow + x) * 4..][..4].copy_from_slice(&sum);
        }
    }
    let mut out = Vec::with_capacity(ow * oh * 4);
    for taps in &rows {
        for x in 0..ow {
            let mut sum = [0f32; 4];
            for &(at, weight) in taps {
                let px = &across[((at - lo) * ow + x) * 4..][..4];
                for c in 0..4 {
                    sum[c] += px[c] * weight;
                }
            }
            let a = sum[3];
            let color = |c: f32| if a > 0.0 { (c / a).round().clamp(0.0, 255.0) as u8 } else { 0 };
            out.extend_from_slice(&[color(sum[0]), color(sum[1]), color(sum[2]), a.round().clamp(0.0, 255.0) as u8]);
        }
    }
    out
}

/// For each of `n` output pixels from `start`, `step` source pixels apart,
/// the source pixels it reads and how much of each (adding up to 1).
fn taps(start: f32, step: f32, n: usize, len: usize) -> Vec<Vec<(usize, f32)>> {
    let last = len as f32 - 1.0;
    let at = |i: f32| i.clamp(0.0, last) as usize;
    (0..n)
        .map(|i| {
            let mut taps = Vec::new();
            if step >= 1.0 {
                let (a, b) = (start + i as f32 * step, start + (i + 1) as f32 * step);
                let mut j = a.floor();
                while j < b {
                    let cover = b.min(j + 1.0) - a.max(j);
                    if cover > 0.0 {
                        taps.push((at(j), cover));
                    }
                    j += 1.0;
                }
            } else {
                let c = start + (i as f32 + 0.5) * step - 0.5;
                let j = c.floor();
                let f = c - j;
                taps.push((at(j), 1.0 - f));
                taps.push((at(j + 1.0), f));
            }
            let total: f32 = taps.iter().map(|t| t.1).sum();
            if total > 0.0 {
                for t in &mut taps {
                    t.1 /= total;
                }
            }
            taps
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDE: Size = Size { width: 2000.0, height: 1000.0 };
    const SQUARE: Size = Size { width: 400.0, height: 400.0 };

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn shapes_match_the_web() {
        assert_eq!(PictureKind::Avatar.shape(), Shape { width: 512, height: 512, round: true });
        assert_eq!(PictureKind::Banner.shape(), Shape { width: 1500, height: 600, round: false });
        assert_eq!(PictureKind::Icon.shape(), Shape { width: 512, height: 512, round: false });
    }

    #[test]
    fn cover_scale_fills_the_frame() {
        // A wide picture in a square frame: its height decides.
        assert!(near(cover_scale(WIDE, SQUARE), 0.4));
        // A tall one in a banner: its width does.
        let tall = Size { width: 600.0, height: 1200.0 };
        assert!(near(cover_scale(tall, Size { width: 624.0, height: 249.6 }), 1.04));
    }

    #[test]
    fn clamp_keeps_the_frame_inside() {
        let c = clamp_crop(Crop { cx: 0.0, cy: 0.0, zoom: 1.0 }, WIDE, SQUARE);
        // At zoom 1 the frame is 1000 image pixels across: its center can't be nearer the edge than 500.
        assert!(near(c.cx, 500.0) && near(c.cy, 500.0));
        let c = clamp_crop(Crop { cx: 5000.0, cy: 5000.0, zoom: 2.0 }, WIDE, SQUARE);
        assert!(near(c.cx, 1750.0) && near(c.cy, 750.0));
    }

    #[test]
    fn clamp_bounds_the_zoom() {
        assert!(near(clamp_crop(Crop { cx: 1000.0, cy: 500.0, zoom: 0.2 }, WIDE, SQUARE).zoom, 1.0));
        assert!(near(clamp_crop(Crop { cx: 1000.0, cy: 500.0, zoom: 9.0 }, WIDE, SQUARE).zoom, MAX_ZOOM));
    }

    #[test]
    fn placement_centers_the_crop() {
        let at = placement(centered(WIDE), WIDE, SQUARE);
        assert!(near(at.scale, 0.4));
        assert!(near(at.x, 200.0 - 1000.0 * 0.4) && near(at.y, 0.0));
    }

    #[test]
    fn zoom_around_keeps_the_point_still() {
        let start = centered(WIDE);
        let point = (100.0, 300.0);
        let before = placement(start, WIDE, SQUARE);
        let image_point = ((point.0 - before.x) / before.scale, (point.1 - before.y) / before.scale);
        let zoomed = zoom_around(start, 2.0, point, WIDE, SQUARE);
        assert!(near(zoomed.zoom, 2.0));
        let after = placement(zoomed, WIDE, SQUARE);
        assert!(near(after.x + image_point.0 * after.scale, point.0));
        assert!(near(after.y + image_point.1 * after.scale, point.1));
    }

    #[test]
    fn zoom_around_stays_within_bounds() {
        let out = zoom_around(centered(WIDE), 0.5, (200.0, 200.0), WIDE, SQUARE);
        assert_eq!(out, centered(WIDE));
        let deep = zoom_around(centered(WIDE), 100.0, (0.0, 0.0), WIDE, SQUARE);
        assert!(near(deep.zoom, MAX_ZOOM));
        assert_eq!(deep, clamp_crop(deep, WIDE, SQUARE));
    }

    #[test]
    fn at_start_until_moved() {
        assert!(at_start(centered(WIDE), WIDE));
        assert!(!at_start(Crop { zoom: 1.5, ..centered(WIDE) }, WIDE));
    }

    /// A picture whose left half is red and right half blue.
    fn halves(w: u32, h: u32) -> Vec<u8> {
        (0..h)
            .flat_map(|_| (0..w).flat_map(move |x| if x < w / 2 { [255, 0, 0, 255] } else { [0, 0, 255, 255] }))
            .collect()
    }

    #[test]
    fn cropped_is_the_kinds_size() {
        let px = halves(64, 32);
        let frame = Size { width: 400.0, height: 400.0 };
        let image = Size { width: 64.0, height: 32.0 };
        let out = cropped(&px, 64, 32, centered(image), PictureKind::Avatar, frame);
        assert_eq!(out.len(), 512 * 512 * 4);
        let banner = Size { width: 624.0, height: 249.6 };
        let out = cropped(&px, 64, 32, centered(image), PictureKind::Banner, banner);
        assert_eq!(out.len(), 1500 * 600 * 4);
    }

    #[test]
    fn cropped_takes_what_the_frame_shows() {
        let (w, h) = (2000, 1000);
        let px = halves(w, h);
        let image = Size { width: w as f32, height: h as f32 };
        // Moved all the way left, the square frame sees only red.
        let left = clamp_crop(Crop { cx: 0.0, cy: 500.0, zoom: 1.0 }, image, SQUARE);
        let out = cropped(&px, w, h, left, PictureKind::Icon, SQUARE);
        assert!(out.chunks(4).all(|p| p == [255, 0, 0, 255]));
        // Centered, the left half of the output is red and the right blue.
        let out = cropped(&px, w, h, centered(image), PictureKind::Icon, SQUARE);
        assert_eq!(&out[(100 * 512 + 10) * 4..][..4], &[255, 0, 0, 255]);
        assert_eq!(&out[(100 * 512 + 500) * 4..][..4], &[0, 0, 255, 255]);
    }

    #[test]
    fn shrinking_averages_and_keeps_clear_pixels_out() {
        // Two pixels, one red and one fully clear (black): shrunk to one, it's red at half opacity.
        let px = [255, 0, 0, 255, 0, 0, 0, 0];
        let out = resample(&px, (2, 1), (0.0, 0.0, 2.0, 1.0), (1, 1));
        assert_eq!(out, vec![255, 0, 0, 128]);
    }

    #[test]
    fn growing_blends_neighbours() {
        let px = [0, 0, 0, 255, 200, 200, 200, 255];
        let out = resample(&px, (2, 1), (0.0, 0.0, 2.0, 1.0), (4, 1));
        let reds: Vec<u8> = out.chunks(4).map(|p| p[0]).collect();
        assert_eq!(reds, vec![0, 50, 150, 200]);
    }

    #[test]
    fn fit_shrinks_only_big_pictures() {
        let px = halves(8, 4);
        assert_eq!(fit(&px, 8, 4, 16), (px.clone(), 8, 4));
        let (small, w, h) = fit(&px, 8, 4, 4);
        assert_eq!((w, h, small.len()), (4, 2, 4 * 2 * 4));
    }
}
