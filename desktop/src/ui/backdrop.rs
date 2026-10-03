//! What's drawn behind the app (`docs/themes.md`, Backdrops): the theme's
//! background, a picture from a fuwa instance, the background again to dim
//! it, and a texture. The panels in front turn see-through over it
//! (`Palette::over`).
//!
//! Textures are small tiles made here (noise as PNG, patterns as SVG, the
//! same ideas as the web app's CSS), drawn once by GPUI and repeated, so a still
//! backdrop costs a handful of sprites and nothing between frames. The
//! picture loads through the window's HTTP client, which only fetches from
//! instances you added.

use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Image, ImageFormat, ImgResourceLoader, IntoElement, ObjectFit, ParentElement as _, Resource,
    SharedString, Styled as _, StyledImage as _, Window, div, img, px,
};
use parking_lot::Mutex;

use crate::core::themes::{Backdrop, Effect, Fit, hex};
use crate::ui::theme::{Palette, alpha};

/// The most tiles a repeated picture or texture draws; small ones are scaled up to stay under it.
const MAX_TILES: f32 = 400.0;

/// The layers behind the app, filling their parent; none when there's nothing to draw.
pub fn layers(b: &Backdrop, p: &Palette, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    if !b.any() {
        return None;
    }
    let size = window.viewport_size();
    let (w, h) = (f32::from(size.width), f32::from(size.height));
    let picture = (!b.image.is_empty()).then(|| picture(&b.image, b.fit, w, h, window, cx));
    Some(
        div()
            .absolute()
            .inset_0()
            .overflow_hidden()
            .bg(p.background)
            .when_some(picture, |el, pic| {
                el.child(pic).child(div().absolute().inset_0().bg(alpha(p.background, f32::from(b.dim) / 100.0)))
            })
            .when_some(texture(b.effect, p), |el, (tile, size)| {
                el.child(tiled(tile, size, size, w, h).opacity(f32::from(b.intensity) / 100.0))
            })
            .into_any_element(),
    )
}

fn picture(url: &str, fit: Fit, w: f32, h: f32, window: &mut Window, cx: &mut App) -> AnyElement {
    let url = SharedString::from(url.to_owned());
    if fit == Fit::Tile {
        // Repeated at its own size, which is known once it has loaded.
        let resource = Resource::Uri(url.clone().into());
        if let Some(Ok(image)) = window.use_asset::<ImgResourceLoader>(&resource, cx) {
            let s = image.size(0);
            let scale = window.scale_factor();
            let (tw, th) = (i32::from(s.width) as f32 / scale, i32::from(s.height) as f32 / scale);
            if tw >= 1.0 && th >= 1.0 {
                return tiled(Tile::Uri(url), tw, th, w, h).into_any_element();
            }
        }
        return div().into_any_element();
    }
    img(url)
        .absolute()
        .inset_0()
        .size_full()
        .object_fit(if fit == Fit::Contain { ObjectFit::Contain } else { ObjectFit::Cover })
        .into_any_element()
}

#[derive(Clone)]
enum Tile {
    Uri(SharedString),
    Image(Arc<Image>),
}

/// A picture repeated over `w` by `h`, scaled up when it's tiny so it stays a few hundred tiles.
fn tiled(tile: Tile, tw: f32, th: f32, w: f32, h: f32) -> gpui_kit::Div {
    let count = (w / tw).ceil() * (h / th).ceil();
    let grow = if count > MAX_TILES { (count / MAX_TILES).sqrt() } else { 1.0 };
    let (tw, th) = (tw * grow, th * grow);
    let (cols, rows) = ((w / tw).ceil().max(1.0) as usize, (h / th).ceil().max(1.0) as usize);
    let mut grid = div().absolute().top_0().left_0().w(px(cols as f32 * tw)).flex().flex_wrap();
    for _ in 0..cols * rows {
        let el = match &tile {
            Tile::Uri(u) => img(u.clone()),
            Tile::Image(i) => img(i.clone()),
        };
        grid = grid.child(el.flex_none().w(px(tw)).h(px(th)).object_fit(ObjectFit::Fill));
    }
    div().absolute().inset_0().overflow_hidden().child(grid)
}

/// The texture's tile and its size, made for the theme's colors and kept for reuse.
fn texture(effect: Effect, p: &Palette) -> Option<(Tile, f32)> {
    if !effect.texture() {
        return None;
    }
    let primary = hex(rgb_of(p.primary));
    let key = format!("{effect:?}|{primary}");
    static MADE: Mutex<Option<(String, Arc<Image>, f32)>> = Mutex::new(None);
    let mut made = MADE.lock();
    if let Some((k, image, size)) = made.as_ref()
        && *k == key
    {
        return Some((Tile::Image(image.clone()), *size));
    }
    let (image, size) = match noise_png(effect) {
        Some((png, size)) => (Image::from_bytes(ImageFormat::Png, png), size),
        None => {
            let (svg, size) = texture_svg(effect, &primary)?;
            (Image::from_bytes(ImageFormat::Svg, svg.into_bytes()), size)
        }
    };
    let image = Arc::new(image);
    *made = Some((key, image.clone(), size));
    Some((Tile::Image(image), size))
}

/// The noise textures (film grain and paper fibers) as PNG tiles that
/// repeat seamlessly: the same idea as the web app's `feTurbulence`, made
/// here so they look the same whatever draws SVG.
pub fn noise_png(effect: Effect) -> Option<(Vec<u8>, f32)> {
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut random = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 40) as f32 / (1u64 << 24) as f32
    };
    match effect {
        Effect::Grain => {
            let n = 200;
            let mut rgba = Vec::with_capacity(n * n * 4);
            for _ in 0..n * n {
                let v = random();
                let (c, a) = if v > 0.5 { (255, v - 0.5) } else { (0, 0.5 - v) };
                rgba.extend_from_slice(&[c, c, c, (a * 0.5 * 255.0) as u8]);
            }
            Some((png(n as u32, n as u32, &rgba), n as f32))
        }
        Effect::Paper => {
            // Smooth noise on a lattice that wraps: long across, short down, for fibers.
            let n = 320usize;
            let octaves: [(usize, usize, f32); 2] = [(4, 120, 0.7), (8, 240, 0.3)];
            let lattices: Vec<Vec<f32>> =
                octaves.iter().map(|(cx, cy, _)| (0..cx * cy).map(|_| random()).collect()).collect();
            let smooth = |t: f32| t * t * (3.0 - 2.0 * t);
            let mut rgba = Vec::with_capacity(n * n * 4);
            for y in 0..n {
                for x in 0..n {
                    let mut v = 0.0;
                    for ((cx, cy, weight), lattice) in octaves.iter().zip(&lattices) {
                        let (fx, fy) = (x as f32 * *cx as f32 / n as f32, y as f32 * *cy as f32 / n as f32);
                        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
                        let (tx, ty) = (smooth(fx.fract()), smooth(fy.fract()));
                        let at = |i: usize, j: usize| lattice[(j % cy) * cx + (i % cx)];
                        let top = at(x0, y0) * (1.0 - tx) + at(x0 + 1, y0) * tx;
                        let bottom = at(x0, y0 + 1) * (1.0 - tx) + at(x0 + 1, y0 + 1) * tx;
                        v += (top * (1.0 - ty) + bottom * ty) * weight;
                    }
                    let a = ((v - 0.45) * 0.9).clamp(0.0, 1.0) * 0.6;
                    rgba.extend_from_slice(&[140, 115, 89, (a * 255.0) as u8]);
                }
            }
            Some((png(n as u32, n as u32, &rgba), n as f32))
        }
        _ => None,
    }
}

/// A plain PNG (stored, not compressed) of RGBA pixels.
fn png(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
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
    let mut raw = Vec::with_capacity((w as usize * 4 + 1) * h as usize);
    for row in rgba.chunks(w as usize * 4) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (i, block) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        let len = block.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(block);
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

/// A tile as SVG, drawn at twice its size so it stays sharp on high-density screens.
pub fn texture_svg(effect: Effect, primary: &str) -> Option<(String, f32)> {
    let wrap = |size: f32, body: String| {
        (
            format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{w}" viewBox="0 0 {size} {size}">{body}</svg>"#,
                w = size * 2.0
            ),
            size,
        )
    };
    Some(match effect {
        Effect::Dots => {
            let mut body = String::new();
            for y in 0..10 {
                for x in 0..10 {
                    body.push_str(&format!(
                        r#"<circle cx="{}" cy="{}" r="1.4" fill="{primary}" fill-opacity="0.45"/>"#,
                        11 + x * 22,
                        11 + y * 22
                    ));
                }
            }
            wrap(220.0, body)
        }
        Effect::Grid => {
            let mut body = String::new();
            for n in 0..10 {
                let at = n * 28;
                body.push_str(&format!(
                    r#"<rect x="{at}" y="0" width="1" height="280" fill="{primary}" fill-opacity="0.18"/><rect x="0" y="{at}" width="280" height="1" fill="{primary}" fill-opacity="0.18"/>"#
                ));
            }
            wrap(280.0, body)
        }
        _ => return None,
    })
}

fn rgb_of(c: gpui_kit::Rgba) -> u32 {
    let ch = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    (ch(c.r) << 16) | (ch(c.g) << 8) | ch(c.b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn textures_are_svg_tiles_with_the_theme_color_and_nothing_from_elsewhere() {
        for effect in [Effect::Dots, Effect::Grid] {
            let (svg, size) = texture_svg(effect, "#f06292").unwrap();
            assert!(size >= 200.0);
            assert!(svg.starts_with("<svg") && !svg.contains("href"), "{svg}");
        }
        for effect in [Effect::Grain, Effect::Paper] {
            let (png, size) = noise_png(effect).unwrap();
            assert!(png.starts_with(b"\x89PNG") && png.len() > (size * size * 4.0) as usize);
        }
        assert!(texture_svg(Effect::Dots, "#f06292").unwrap().0.contains("#f06292"));
        assert!(texture_svg(Effect::Aurora, "#f06292").is_none());
    }
}
