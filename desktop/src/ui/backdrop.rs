//! What's drawn behind the app (`docs/themes.md`, Backdrops): the theme's
//! background, a picture from a fuwa instance, the background again to dim
//! it, and a texture. The panels in front turn see-through over it
//! (`Palette::over`).
//!
//! Textures are small tiles made here (noise as PNG, patterns as SVG, the
//! same ideas as the web app's CSS), drawn once by GPUI and repeated, so a still
//! backdrop costs a handful of sprites and nothing between frames. Animated
//! effects are `effects.rs`. The picture loads through the window's HTTP
//! client, which only fetches from instances you added; blurred, it's made
//! smaller and blurred once off the main thread, then kept. A video or
//! picture of yours (`core::backdrop_files`) stands in for it, read from its
//! copy on this computer: a picture as any other, a video looped without
//! sound through `core::media`, holding still when the effects do.

use std::cell::RefCell;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, App, Bounds, Corners, ElementId, Image, ImageFormat, ImgResourceLoader,
    InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _, Pixels, RenderImage, Resource, SharedString,
    Styled as _, StyledImage as _, Window, div, img, px,
};
use parking_lot::Mutex;

use crate::core::backdrop_files::{BackdropFile, Kind};
use crate::core::media::Player;
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
    let picture = match &b.file {
        Some(file) => device_file(file, b.fit, b.blur, w, h, window, cx),
        None => (!b.image.is_empty()).then(|| picture(&b.image, b.fit, b.blur, w, h, window, cx)).flatten(),
    };
    let shown = b.file.as_ref().map_or(b.image.as_str(), |f| f.file.as_str());
    // A picture fades in once it has loaded, settling from 104%, so a slow one never pops in half drawn.
    let picture = picture.map(|pic| {
        div()
            .absolute()
            .inset_0()
            .child(pic)
            .with_animation(
                SharedString::from(format!("backdrop-picture|{shown}")),
                Animation::new(Duration::from_millis(700)).with_easing(gpui_kit::ease_out_quint()),
                |el, t| el.opacity(t).scale(1.04 - 0.04 * t),
            )
            .into_any_element()
    });
    let effect = crate::ui::effects::layer(b.effect, b.shader.as_ref(), b.intensity, b.speed, p, window, cx);
    // A new effect or texture fades in, as on the web.
    let fade = |el: AnyElement| {
        div()
            .absolute()
            .inset_0()
            .child(el)
            .with_animation(
                SharedString::from(format!("backdrop-effect|{:?}", b.effect)),
                Animation::new(Duration::from_millis(600)).with_easing(gpui_kit::ease_out_quint()),
                |el, t| el.opacity(t),
            )
            .into_any_element()
    };
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
                el.child(fade(
                    tiled(tile, size, size, w, h).opacity(f32::from(b.intensity) / 100.0 * 0.9).into_any_element(),
                ))
            })
            .when_some(effect, |el, effect| el.child(fade(effect)))
            .into_any_element(),
    )
}

/// The picture, once it has loaded (none until then).
fn picture(url: &str, fit: Fit, blur: u8, w: f32, h: f32, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let url = SharedString::from(url.to_owned());
    shown_picture(Tile::Uri(url.clone()), url.clone(), Resource::Uri(url.into()), fit, blur, w, h, window, cx)
}

/// A picture from `resource` (`key` names it), drawn as `tile` says, once it has loaded.
#[allow(clippy::too_many_arguments)]
fn shown_picture(
    tile: Tile,
    key: SharedString,
    resource: Resource,
    fit: Fit,
    blur: u8,
    w: f32,
    h: f32,
    window: &mut Window,
    cx: &mut App,
) -> Option<AnyElement> {
    if blur > 0 {
        return blurred(key, resource, fit, blur, w, h, window, cx);
    }
    let Some(Ok(image)) = window.use_asset::<ImgResourceLoader>(&resource, cx) else { return None };
    if fit == Fit::Tile {
        // Repeated at its own size, which is known once it has loaded.
        let s = image.size(0);
        let scale = window.scale_factor();
        let (tw, th) = (i32::from(s.width) as f32 / scale, i32::from(s.height) as f32 / scale);
        return (tw >= 1.0 && th >= 1.0).then(|| tiled(tile, tw, th, w, h).into_any_element());
    }
    Some(
        tile.element()
            .absolute()
            .inset_0()
            .size_full()
            .object_fit(if fit == Fit::Contain { ObjectFit::Contain } else { ObjectFit::Cover })
            .into_any_element(),
    )
}

/// A video or picture of yours, from its copy on this computer (none until it's ready).
fn device_file(
    file: &BackdropFile,
    fit: Fit,
    blur: u8,
    w: f32,
    h: f32,
    window: &mut Window,
    cx: &mut App,
) -> Option<AnyElement> {
    let path: Arc<Path> = file.path()?.into();
    match file.kind {
        Kind::Image => {
            let key = SharedString::from(path.display().to_string());
            shown_picture(Tile::Path(path.clone()), key, Resource::Path(path), fit, blur, w, h, window, cx)
        }
        Kind::Video => video(path, fit, window, cx),
    }
}

/// The video behind the app: opened once (read off the main thread) and
/// kept while it's the one picked, by every backdrop drawing it.
struct Playing {
    path: Arc<Path>,
    /// None while it's being read, or once it couldn't be played.
    player: Option<Player>,
    shown: Option<Arc<RenderImage>>,
    /// Frames replaced, to leave the window's atlas once they're painted over.
    retired: Vec<Arc<RenderImage>>,
}

thread_local! {
    static VIDEO: RefCell<Option<Playing>> = const { RefCell::new(None) };
}

/// A video of yours, looping without sound: it moves while the window is in
/// front and nothing covers it, and holds still on its frame otherwise or with
/// reduced motion, like the effects. Its frames come at most 30 a second.
fn video(path: Arc<Path>, fit: Fit, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let opened = VIDEO.with(|v| v.borrow().as_ref().is_some_and(|p| p.path == path));
    if !opened {
        VIDEO.with(|v| {
            *v.borrow_mut() = Some(Playing { path: path.clone(), player: None, shown: None, retired: Vec::new() })
        });
        let view = window.current_view();
        let read = cx.background_executor().spawn({
            let path = path.clone();
            async move {
                let bytes = std::fs::read(&path).ok()?;
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let player = Player::open(bytes.into(), "", &name).ok().filter(Player::has_picture)?;
                player.set_muted(true);
                Some(player)
            }
        });
        cx.spawn(async move |cx| {
            let player = read.await;
            VIDEO.with(|v| {
                if let Some(playing) = v.borrow_mut().as_mut().filter(|p| p.path == path) {
                    playing.player = player;
                }
            });
            cx.update(|cx| cx.notify(view));
        })
        .detach();
    }
    let running = window.is_window_active() && !cx.reduce_motion() && !crate::ui::effects::held();
    let (image, moving) = VIDEO
        .with(|v| {
            let mut v = v.borrow_mut();
            let playing = v.as_mut()?;
            let player = playing.player.as_ref()?;
            if running {
                // Round again from the start.
                if player.is_paused() || player.has_ended() {
                    player.play();
                }
            } else if !player.is_paused() {
                player.pause();
            }
            if let Some(frame) = player.take_picture() {
                let (width, height) = (frame.width, frame.height);
                if let Some(buffer) = image::RgbaImage::from_raw(width, height, frame.bgra) {
                    let image = Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]));
                    if let Some(old) = playing.shown.replace(image) {
                        playing.retired.push(old);
                    }
                }
            }
            Some((playing.shown.clone(), running))
        })
        .unwrap_or((None, false));
    if moving {
        again(window, cx);
    }
    let image = image?;
    let (iw, ih) = {
        let s = image.size(0);
        (i32::from(s.width) as f32, i32::from(s.height) as f32)
    };
    Some(
        gpui_kit::canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let gone = VIDEO.with(|v| v.borrow_mut().as_mut().map(|p| std::mem::take(&mut p.retired)));
                for old in gone.into_iter().flatten() {
                    let _ = window.drop_image(old);
                }
                let to = place(bounds, iw, ih, fit == Fit::Contain);
                let _ = window.paint_image(bounds, to, Corners::default(), image.clone(), 0, false);
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .into_any_element(),
    )
}

/// Where a picture this size goes in `bounds`: covering them (cut at the
/// edges), or as big as fits whole; centred either way.
fn place(bounds: Bounds<Pixels>, w: f32, h: f32, contain: bool) -> Bounds<Pixels> {
    let (bw, bh) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    if w <= 0.0 || h <= 0.0 || bw <= 0.0 || bh <= 0.0 {
        return bounds;
    }
    let scale = if contain { (bw / w).min(bh / h) } else { (bw / w).max(bh / h) };
    let (fw, fh) = (w * scale, h * scale);
    Bounds::new(
        gpui_kit::point(bounds.origin.x + px((bw - fw) / 2.0), bounds.origin.y + px((bh - fh) / 2.0)),
        gpui_kit::size(px(fw), px(fh)),
    )
}

/// Draws the view again a frame later, while the video moves.
fn again(window: &Window, cx: &mut App) {
    static WAITING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if WAITING.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let view = window.current_view();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(Duration::from_millis(33)).await;
        WAITING.store(false, std::sync::atomic::Ordering::Relaxed);
        cx.update(|cx| cx.notify(view));
    })
    .detach();
}

/// Lets go of the video once nothing picks it any more.
pub fn forget_video(picked: Option<&BackdropFile>) {
    let path = picked.filter(|f| f.kind == Kind::Video).and_then(BackdropFile::path);
    VIDEO.with(|v| {
        let mut v = v.borrow_mut();
        if v.as_ref().is_some_and(|p| Some(p.path.as_ref()) != path.as_deref()) {
            *v = None;
        }
    });
}

/// The longest side a blurred picture is made at: blurring hides the detail a bigger one would have.
const BLURRED_SIDE: f32 = 480.0;

/// The picture blurred by `blur` points. It's made smaller, blurred off the
/// main thread and kept; until it's ready (or while a resize makes it
/// again) the last one made for this picture shows, and nothing sharp does.
/// Once it's been made smaller, the full-size picture is let go: blurring it
/// again (a resize) starts from the smaller copy.
#[allow(clippy::too_many_arguments)]
fn blurred(
    url: SharedString,
    resource: Resource,
    fit: Fit,
    blur: u8,
    w: f32,
    h: f32,
    window: &mut Window,
    cx: &mut App,
) -> Option<AnyElement> {
    /// The picture made smaller (BGRA, `dw` by `dh`), and its own size.
    #[derive(Clone)]
    struct Small {
        url: SharedString,
        iw: f32,
        ih: f32,
        dw: usize,
        dh: usize,
        bytes: Arc<Vec<u8>>,
    }
    struct Made {
        key: String,
        working: Option<String>,
        image: Option<(SharedString, Arc<RenderImage>, f32, f32)>,
        small: Option<Small>,
    }
    static MADE: Mutex<Made> = Mutex::new(Made { key: String::new(), working: None, image: None, small: None });

    let small = MADE.lock().small.clone().filter(|s| s.url == url);
    // The full-size picture, only until it's been made smaller.
    let (source, iw, ih) = match &small {
        Some(s) => (None, s.iw, s.ih),
        None => {
            let Some(Ok(source)) = window.use_asset::<ImgResourceLoader>(&resource, cx) else { return None };
            let s = source.size(0);
            (Some(source), i32::from(s.width).max(1) as f32, i32::from(s.height).max(1) as f32)
        }
    };
    let shrink = (iw.max(ih) / BLURRED_SIDE).max(1.0);
    let (dw, dh) = ((iw / shrink).round().max(1.0) as usize, (ih / shrink).round().max(1.0) as usize);
    let scale = window.scale_factor();
    // How many points one pixel of the smaller picture covers on screen.
    let shown = match fit {
        Fit::Cover => (w / dw as f32).max(h / dh as f32),
        Fit::Contain => (w / dw as f32).min(h / dh as f32),
        Fit::Tile => iw / scale / dw as f32,
    };
    // Three box blurs this wide add up to a Gaussian of about `blur`, like CSS's.
    let radius = (f32::from(blur) / shown.max(0.01)).round().clamp(1.0, 64.0) as usize;
    let key = format!("{url}|{dw}x{dh}|{radius}|{}", fit == Fit::Tile);

    let mut made = MADE.lock();
    if made.key != key && made.working.as_deref() != Some(key.as_str()) {
        made.working = Some(key.clone());
        let view = window.current_view();
        let wrap = fit == Fit::Tile;
        let work = cx.background_executor().spawn({
            let url = url.clone();
            async move {
                let small = match (small, source) {
                    (Some(small), _) => small,
                    (None, Some(source)) => {
                        let bytes = Arc::new(shrink_picture(&source, dw, dh)?);
                        Small { url, iw, ih, dw, dh, bytes }
                    }
                    (None, None) => return None,
                };
                let picture = blur_picture(&small.bytes, small.dw, small.dh, radius, wrap)?;
                Some((small, picture))
            }
        });
        let (url, key) = (url.clone(), key.clone());
        window
            .spawn(cx, async move |cx| {
                let done = work.await;
                let mut made = MADE.lock();
                let (mut let_go, mut replaced) = (false, None);
                if made.working.as_deref() == Some(key.as_str()) {
                    made.working = None;
                    if let Some((small, picture)) = done {
                        made.key = key;
                        let (tw, th) = (iw / scale, ih / scale);
                        replaced = made.image.replace((url, picture, tw, th)).map(|(_, old, ..)| old);
                        let_go = made.small.as_ref().is_none_or(|s| s.url != small.url);
                        made.small = Some(small);
                    }
                }
                drop(made);
                let _ = cx.update(|window, cx| {
                    if let_go {
                        cx.remove_asset::<ImgResourceLoader>(&resource);
                    }
                    if let Some(old) = replaced {
                        // The last blur, out of the window's textures; everything is drawn
                        // again so nothing kept still shows it.
                        cx.drop_image(old, Some(window));
                        window.refresh();
                    }
                    cx.notify(view)
                });
            })
            .detach();
    }
    let (_, image, tw, th) = made.image.as_ref().filter(|(u, ..)| *u == url)?;
    if fit == Fit::Tile {
        return Some(tiled(Tile::Render(image.clone()), *tw, *th, w, h).into_any_element());
    }
    Some(
        img(image.clone())
            .absolute()
            .inset_0()
            .size_full()
            .object_fit(if fit == Fit::Contain { ObjectFit::Contain } else { ObjectFit::Cover })
            .into_any_element(),
    )
}

/// A loaded picture (BGRA) made `dw` by `dh`, each pixel the average of the ones it covers.
fn shrink_picture(source: &RenderImage, dw: usize, dh: usize) -> Option<Vec<u8>> {
    let s = source.size(0);
    let (sw, sh) = (usize::try_from(i32::from(s.width)).ok()?, usize::try_from(i32::from(s.height)).ok()?);
    let bytes = source.as_bytes(0)?;
    if sw == 0 || sh == 0 || bytes.len() < sw * sh * 4 {
        return None;
    }
    let mut out = vec![0u8; dw * dh * 4];
    for y in 0..dh {
        let (y0, y1) = (y * sh / dh, ((y + 1) * sh / dh).max(y * sh / dh + 1).min(sh));
        for x in 0..dw {
            let (x0, x1) = (x * sw / dw, ((x + 1) * sw / dw).max(x * sw / dw + 1).min(sw));
            let mut sum = [0f32; 4];
            for row in y0..y1 {
                for col in x0..x1 {
                    let at = (row * sw + col) * 4;
                    for (c, v) in sum.iter_mut().zip(&bytes[at..at + 4]) {
                        *c += f32::from(*v);
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as f32;
            let at = (y * dw + x) * 4;
            for c in 0..4 {
                out[at + c] = (sum[c] / n).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    Some(out)
}

/// A small picture (BGRA, `dw` by `dh`) blurred by three box blurs of
/// `radius`, ready to draw. `wrap` blurs across the edges, so a repeated
/// picture stays seamless.
fn blur_picture(small: &[u8], dw: usize, dh: usize, radius: usize, wrap: bool) -> Option<Arc<RenderImage>> {
    let mut px: Vec<f32> = small.iter().map(|v| f32::from(*v)).collect();
    let mut line = Vec::new();
    for _ in 0..3 {
        box_blur(&mut px, dw, dh, 1, dw, radius, wrap, &mut line);
        box_blur(&mut px, dh, dw, dw, 1, radius, wrap, &mut line);
    }
    // Still BGRA, as the window draws pictures.
    let bgra: Vec<u8> = px.iter().map(|v| v.round().clamp(0.0, 255.0) as u8).collect();
    let buffer = image::RgbaImage::from_raw(dw as u32, dh as u32, bgra)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

/// One box blur pass along lines of `len` pixels `step` apart, `lines` of
/// them `stride` apart, with a running sum so it costs the same at any radius.
#[allow(clippy::too_many_arguments)]
fn box_blur(
    px: &mut [f32],
    len: usize,
    lines: usize,
    step: usize,
    stride: usize,
    radius: usize,
    wrap: bool,
    line: &mut Vec<f32>,
) {
    let r = radius as isize;
    let n = len as isize;
    let pick = |i: isize| -> usize { if wrap { i.rem_euclid(n) as usize } else { i.clamp(0, n - 1) as usize } };
    let width = (2 * radius + 1) as f32;
    for l in 0..lines {
        let base = l * stride;
        line.clear();
        line.extend((0..len).flat_map(|i| {
            let at = base + i * step;
            [px[at * 4], px[at * 4 + 1], px[at * 4 + 2], px[at * 4 + 3]]
        }));
        let mut sum = [0f32; 4];
        for i in -r..=r {
            let at = pick(i) * 4;
            for c in 0..4 {
                sum[c] += line[at + c];
            }
        }
        for i in 0..n {
            let at = (base + i as usize * step) * 4;
            for c in 0..4 {
                px[at + c] = sum[c] / width;
            }
            let (out, inn) = (pick(i - r) * 4, pick(i + r + 1) * 4);
            for c in 0..4 {
                sum[c] += line[inn + c] - line[out + c];
            }
        }
    }
}

#[derive(Clone)]
enum Tile {
    Uri(SharedString),
    Path(Arc<Path>),
    Image(Arc<Image>),
    Render(Arc<RenderImage>),
}

impl Tile {
    fn element(&self) -> gpui_kit::Stateful<gpui_kit::Img> {
        match self {
            Tile::Uri(u) => crate::ui::widgets::picture(u.clone()),
            Tile::Path(path) => img(path.clone()).id(ElementId::Name(format!("picture|{}", path.display()).into())),
            Tile::Image(i) => img(i.clone()).id("texture-tile"),
            Tile::Render(i) => img(i.clone()).id("texture-tile"),
        }
    }
}

/// A picture repeated over `w` by `h`, scaled up when it's tiny so it stays a few hundred tiles.
fn tiled(tile: Tile, tw: f32, th: f32, w: f32, h: f32) -> gpui_kit::Div {
    let count = (w / tw).ceil() * (h / th).ceil();
    let grow = if count > MAX_TILES { (count / MAX_TILES).sqrt() } else { 1.0 };
    let (tw, th) = (tw * grow, th * grow);
    let (cols, rows) = ((w / tw).ceil().max(1.0) as usize, (h / th).ceil().max(1.0) as usize);
    let mut grid = div().absolute().top_0().left_0().w(px(cols as f32 * tw)).flex().flex_wrap();
    for _ in 0..cols * rows {
        grid = grid.child(tile.element().flex_none().w(px(tw)).h(px(th)).object_fit(ObjectFit::Fill));
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
            Some((crate::ui::png::png(n as u32, n as u32, &rgba, false), n as f32))
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
            Some((crate::ui::png::png(n as u32, n as u32, &rgba, false), n as f32))
        }
        _ => None,
    }
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
    fn blurring_spreads_a_spot_and_keeps_what_is_flat() {
        let (w, h) = (16usize, 8usize);
        let mut px = vec![100f32; w * h * 4];
        let mut line = Vec::new();
        box_blur(&mut px, w, h, 1, w, 3, false, &mut line);
        box_blur(&mut px, h, w, w, 1, 3, false, &mut line);
        assert!(px.iter().all(|v| (v - 100.0).abs() < 1e-3));

        // A spot in a corner, repeated: wrapping carries it across the edge and loses nothing.
        let mut px = vec![0f32; w * h * 4];
        px[..4].copy_from_slice(&[255.0; 4]);
        box_blur(&mut px, w, h, 1, w, 2, true, &mut line);
        box_blur(&mut px, h, w, w, 1, 2, true, &mut line);
        let total: f32 = px.iter().step_by(4).sum();
        assert!((total - 255.0).abs() < 0.01, "{total}");
        assert!(px[(w - 1) * 4] > 0.0 && px[((h - 1) * w) * 4] > 0.0);
    }

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
