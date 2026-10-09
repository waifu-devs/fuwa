//! Framing a picture before it's uploaded, as the web's `PictureCropper.tsx`:
//! drag to move it, scroll, pinch or use the slider to zoom, and the arrow
//! keys and + and - work too. The frame has the shape people will see (a
//! circle for avatars), and a grid shows while it's being moved. What comes
//! out is the kind's size as a PNG, written by `png.rs`.
//!
//! `choose` is the whole field's flow: ask for a file, send a GIF as it is
//! (it would stop moving if it were redrawn), frame anything else.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::base::slider::{SliderEvent, SliderState};
use gpui_kit::base::{Slider as BaseSlider, SliderIndicator, SliderThumb, SliderTrack};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Bounds, Context, Entity, EventEmitter, FocusHandle, FontWeight, Image, ImageFormat,
    InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    ParentElement as _, PinchEvent, Pixels, Point, Render, RenderImage, ScrollWheelEvent,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, canvas, div, img, px, relative,
};

use crate::core::Core;
use crate::core::account::{picture_type, read_picture};
use crate::core::i18n::t;
use crate::core::pictures::{
    Crop, MAX_ZOOM, PictureKind, Size, at_start, centered, clamp_crop, cropped, fit, placement, zoom_around,
};
use crate::ui::app::CloseOverlay;
use crate::ui::motion;
use crate::ui::overlay::{dialog_card, dialog_close, dialog_header, scrim};
use crate::ui::settings_controls::{Look, button};
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_lg, radius_xl};
use crate::ui::widgets::{icon, pal};

/// The longest side a picture is drawn at while it's framed; the crop itself
/// is cut from the whole picture.
const SHOWN: u32 = 4096;

/// What the cropper says when it's done.
pub(crate) enum CropEvent {
    /// The framed picture, as a PNG.
    Done(Vec<u8>),
    Cancel,
}

/// The picture once it's been read.
struct Loaded {
    /// Every pixel (BGRA), for the crop.
    full: Arc<RenderImage>,
    /// What's drawn in the frame.
    shown: Arc<Image>,
    size: Size,
}

pub(crate) struct PictureCropper {
    kind: PictureKind,
    picture: Option<Loaded>,
    failed: Option<String>,
    crop: Crop,
    /// Where the pointer was, while the picture is being dragged.
    moving: Option<Point<Pixels>>,
    saving: bool,
    focus: FocusHandle,
    focused: bool,
    /// The frame on screen, for pointer positions within it.
    frame_at: Rc<Cell<Option<Bounds<Pixels>>>>,
    zoom: Entity<SliderState>,
    zoom_held: bool,
    _zoom: Subscription,
}

impl EventEmitter<CropEvent> for PictureCropper {}

/// The frame fills the dialog's width (less its padding) at the kind's shape.
fn frame_of(kind: PictureKind) -> Size {
    let shape = kind.shape();
    let width = if kind == PictureKind::Banner { 624.0 } else { 400.0 };
    Size { width, height: width * shape.height as f32 / shape.width as f32 }
}

impl PictureCropper {
    pub(crate) fn new(kind: PictureKind, bytes: Vec<u8>, mime: &'static str, cx: &mut Context<Self>) -> Self {
        let zoom = cx.new(|_| SliderState::new().min(1.0).max(MAX_ZOOM).step(0.01).default_value(1.0));
        let sub = cx.subscribe(&zoom, |this, _, event: &SliderEvent, cx| match event {
            SliderEvent::Change(v) => {
                this.zoom_held = true;
                this.zoom_to(v.end(), cx);
            }
            SliderEvent::Release(v) => {
                this.zoom_held = false;
                this.zoom_to(v.end(), cx);
            }
        });
        let svg = cx.svg_renderer();
        let read = cx.background_spawn(async move {
            let format = match mime {
                "image/png" => ImageFormat::Png,
                "image/jpeg" => ImageFormat::Jpeg,
                _ => ImageFormat::Webp,
            };
            let full = Image::from_bytes(format, bytes.clone()).to_image_data(svg).ok()?;
            let s = full.size(0);
            let (w, h) = (i32::from(s.width).max(0) as u32, i32::from(s.height).max(0) as u32);
            let px = full.as_bytes(0).filter(|b| w > 0 && h > 0 && b.len() >= (w * h * 4) as usize)?;
            // A huge picture is drawn smaller; the frame shows it the same.
            let shown = if w.max(h) > SHOWN {
                let (mut small, sw, sh) = fit(px, w, h, SHOWN);
                bgra_to_rgba(&mut small);
                Image::from_bytes(ImageFormat::Png, crate::ui::png::png(sw, sh, &small, false))
            } else {
                Image::from_bytes(format, bytes)
            };
            Some(Loaded { full, shown: Arc::new(shown), size: Size { width: w as f32, height: h as f32 } })
        });
        cx.spawn(async move |this, cx| {
            let loaded = read.await;
            let _ = this.update(cx, |this, cx| {
                match loaded {
                    Some(l) => {
                        this.crop = centered(l.size);
                        this.picture = Some(l);
                    }
                    None => this.failed = Some(t("desktop.server.emoji.unreadable")),
                }
                cx.notify();
            });
        })
        .detach();
        Self {
            kind,
            picture: None,
            failed: None,
            crop: Crop { cx: 0.0, cy: 0.0, zoom: 1.0 },
            moving: None,
            saving: false,
            focus: cx.focus_handle(),
            focused: false,
            frame_at: Rc::new(Cell::new(None)),
            zoom,
            zoom_held: false,
            _zoom: sub,
        }
    }

    fn size(&self) -> Option<Size> {
        self.picture.as_ref().map(|l| l.size)
    }

    fn set(&mut self, crop: Crop, cx: &mut Context<Self>) {
        if let Some(size) = self.size() {
            self.crop = clamp_crop(crop, size, frame_of(self.kind));
            cx.notify();
        }
    }

    /// Zooms to `zoom` around the frame's middle.
    fn zoom_to(&mut self, zoom: f32, cx: &mut Context<Self>) {
        let Some(size) = self.size() else { return };
        let frame = frame_of(self.kind);
        let middle = (frame.width / 2.0, frame.height / 2.0);
        self.crop = zoom_around(self.crop, zoom / self.crop.zoom, middle, size, frame);
        cx.notify();
    }

    /// Zooms by `factor` keeping the picture under the pointer where it is.
    fn zoom_at(&mut self, factor: f32, at: Point<Pixels>, cx: &mut Context<Self>) {
        let (Some(size), Some(bounds)) = (self.size(), self.frame_at.get()) else { return };
        if self.saving {
            return;
        }
        let local = (f32::from(at.x - bounds.origin.x), f32::from(at.y - bounds.origin.y));
        self.crop = zoom_around(self.crop, factor, local, size, frame_of(self.kind));
        cx.notify();
    }

    fn drag(&mut self, e: &MouseMoveEvent, cx: &mut Context<Self>) {
        let (Some(last), Some(size)) = (self.moving, self.size()) else { return };
        if e.pressed_button != Some(MouseButton::Left) {
            self.moving = None;
            cx.notify();
            return;
        }
        let scale = placement(self.crop, size, frame_of(self.kind)).scale;
        let (dx, dy) = (f32::from(e.position.x - last.x), f32::from(e.position.y - last.y));
        self.moving = Some(e.position);
        self.set(Crop { cx: self.crop.cx - dx / scale, cy: self.crop.cy - dy / scale, ..self.crop }, cx);
    }

    fn keys(&mut self, e: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(size) = self.size() else { return };
        if self.saving {
            return;
        }
        let step = 12.0 / placement(self.crop, size, frame_of(self.kind)).scale;
        let c = self.crop;
        match e.keystroke.key.as_str() {
            "left" => self.set(Crop { cx: c.cx - step, ..c }, cx),
            "right" => self.set(Crop { cx: c.cx + step, ..c }, cx),
            "up" => self.set(Crop { cy: c.cy - step, ..c }, cx),
            "down" => self.set(Crop { cy: c.cy + step, ..c }, cx),
            "+" | "=" => self.zoom_to(c.zoom * 1.1, cx),
            "-" => self.zoom_to(c.zoom / 1.1, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        if !self.saving {
            cx.emit(CropEvent::Cancel);
        }
    }

    /// Cuts the framed part out at the kind's size and hands it over as a PNG.
    fn use_it(&mut self, cx: &mut Context<Self>) {
        let Some(l) = self.picture.as_ref() else { return };
        if self.saving {
            return;
        }
        self.saving = true;
        let (full, size, crop, kind) = (l.full.clone(), l.size, self.crop, self.kind);
        let work = cx.background_spawn(async move {
            let px = full.as_bytes(0)?;
            let mut out = cropped(px, size.width as u32, size.height as u32, crop, kind, frame_of(kind));
            bgra_to_rgba(&mut out);
            let shape = kind.shape();
            Some(crate::ui::png::png(shape.width, shape.height, &out, true))
        });
        cx.spawn(async move |this, cx| {
            let png = work.await;
            let _ = this.update(cx, |this, cx| {
                this.saving = false;
                match png {
                    Some(png) => cx.emit(CropEvent::Done(png)),
                    None => this.failed = Some(t("workspace.picture.cropFailed")),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

fn bgra_to_rgba(px: &mut [u8]) {
    for p in px.as_chunks_mut::<4>().0 {
        p.swap(0, 2);
    }
}

impl Render for PictureCropper {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // It takes the keys (Escape, arrows, + and -) while it's open.
        if !self.focused {
            self.focused = true;
            self.focus.focus(window, cx);
        }
        let p = pal(cx);
        let kind = self.kind.key();
        let wide = self.kind == PictureKind::Banner;
        let body = match &self.failed {
            Some(failed) => div()
                .rounded(radius_2xl())
                .bg(alpha(p.destructive, 0.1))
                .p(px(16.0))
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .text_color(p.destructive)
                .child(failed.clone())
                .into_any_element(),
            None => self.framer(&p, window, cx),
        };
        let card = dialog_card(wide, &p)
            .id("cropper-card")
            // A press inside never closes it, even if it ends outside (a drag).
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(dialog_header(
                t(&format!("workspace.picture.frame.{kind}")),
                Some(t("workspace.picture.frameAbout").into_any_element()),
                &p,
            ))
            .child(body)
            .child(dialog_close("cropper-close", &p).on_click(cx.listener(|this, _, _, cx| this.cancel(cx))));

        div()
            .id("cropper")
            .absolute()
            .inset_0()
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &CloseOverlay, _, cx| this.cancel(cx)))
            .on_key_down(cx.listener(Self::keys))
            .child(motion::fade_in(
                scrim("cropper-scrim", &p)
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.cancel(cx)))
                    .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| this.drag(e, cx)))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.moving = None;
                            cx.notify();
                        }),
                    )
                    .child(motion::dialog_in(div().child(card), "cropper-panel")),
                "cropper-fade",
                Duration::from_millis(200),
            ))
    }
}

impl PictureCropper {
    /// The frame, the zoom and the buttons.
    fn framer(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let frame = frame_of(self.kind);
        let ready = self.picture.is_some();
        let zoom = self.crop.zoom;
        // Something else moved the zoom (the wheel, a key): the thumb follows, unless it's held.
        let shown = self.zoom.read(cx).value().end();
        if !self.zoom_held && (shown - zoom).abs() > 0.005 {
            self.zoom.update(cx, |s, cx| s.set_value(zoom, window, cx));
        }
        let saving = self.saving;

        let zoom_button = |id: &'static str, glyph: &'static str, off: bool| {
            let (hover, fg) = (p.muted, p.foreground);
            div()
                .id(id)
                .size(px(36.0))
                .flex_none()
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(p.muted_foreground)
                .when(off, |el| el.opacity(0.4))
                // `active:scale-90`.
                .when(!off, |el| {
                    el.cursor_pointer().hover(move |s| s.bg(hover).text_color(fg)).active(|s| s.scale(0.9))
                })
                .child(icon(glyph).size(px(16.0)))
        };
        let zoom_row = div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .child(zoom_button("cropper-zoom-out", "zoom-out", !ready || zoom <= 1.0).on_click(cx.listener(
                |this, _, _, cx| {
                    let z = this.crop.zoom;
                    this.zoom_to(z / 1.25, cx)
                },
            )))
            .child(div().flex_1().child(self.slider(p, window, cx)))
            .child(zoom_button("cropper-zoom-in", "zoom-in", !ready || zoom >= MAX_ZOOM).on_click(cx.listener(
                |this, _, _, cx| {
                    let z = this.crop.zoom;
                    this.zoom_to(z * 1.25, cx)
                },
            )));

        let reset = button("cropper-reset", t("workspace.picture.reset"), Some("rotate-ccw"), Look::Ghost, false, p)
            .rounded(radius_xl())
            .when(!ready || saving, |el| el.opacity(0.5))
            .when(ready && !saving, |el| {
                el.on_click(cx.listener(|this, _, _, cx| {
                    if let Some(size) = this.size() {
                        this.crop = centered(size);
                        cx.notify();
                    }
                }))
            });
        let cancel = button("cropper-cancel", t("common.cancel"), None, Look::Ghost, false, p)
            .rounded(radius_xl())
            .when(saving, |el| el.opacity(0.5))
            .on_click(cx.listener(|this, _, _, cx| this.cancel(cx)));
        let done = button("cropper-done", t("workspace.picture.useIt"), Some("check"), Look::Primary, false, p)
            .rounded(radius_xl())
            .px(px(16.0))
            .font_weight(FontWeight::BOLD)
            .when(!ready || saving, |el| el.opacity(0.5))
            .when(ready && !saving, |el| el.on_click(cx.listener(|this, _, _, cx| this.use_it(cx))));
        // Reset on its own at the start, the other two at the end.
        let actions =
            div().flex().items_center().gap(px(8.0)).child(reset).child(div().flex_1()).child(cancel).child(done);

        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(self.frame(frame, p, cx))
            .child(zoom_row)
            .child(actions)
            .into_any_element()
    }

    /// The picture in its frame: dimmed outside the shape, whose edge glows.
    fn frame(&mut self, frame: Size, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let (fw, fh) = (frame.width, frame.height);
        let moving = self.moving.is_some();
        let cell = self.frame_at.clone();
        let mut el = div()
            .id("cropper-frame")
            .relative()
            .flex_none()
            .w(px(fw))
            .h(px(fh))
            .rounded(radius_2xl())
            .overflow_hidden()
            .bg(p.muted)
            .child(canvas(move |bounds, _, _| cell.set(Some(bounds)), |_, _, _, _| {}).absolute().inset_0());

        if let Some(l) = &self.picture {
            let at = placement(self.crop, l.size, frame);
            el = el
                .when(!self.saving, |el| if moving { el.cursor_grabbing() } else { el.cursor_grab() })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, e: &MouseDownEvent, _, cx| {
                        if !this.saving {
                            this.moving = Some(e.position);
                            cx.notify();
                        }
                    }),
                )
                .on_scroll_wheel(cx.listener(|this, e: &ScrollWheelEvent, _, cx| {
                    // The web zooms by e^(-deltaY * 0.0015); a wheel's notch is about 100 pixels there.
                    let dy = f32::from(e.delta.pixel_delta(px(100.0 / 3.0)).y);
                    this.zoom_at((dy * 0.0015).exp(), e.position, cx);
                }))
                .on_pinch(cx.listener(|this, e: &PinchEvent, _, cx| this.zoom_at(1.0 + e.delta, e.position, cx)))
                .child(motion::fade_in(
                    img(l.shown.clone())
                        .absolute()
                        .left(px(at.x))
                        .top(px(at.y))
                        .w(px(l.size.width * at.scale))
                        .h(px(l.size.height * at.scale)),
                    "cropper-picture",
                    Duration::from_millis(450),
                ));
        }

        el.child(shape_mask(self.kind, frame))
            .when(moving, |el| el.child(grid(frame)))
            .when(self.size().is_some_and(|s| !moving && at_start(self.crop, s)), |el| el.child(drag_hint()))
            .child(corners(frame, p))
            .into_any_element()
    }

    /// The zoom slider, on the look of the app's other sliders, with the zoom in a bubble while it's held.
    fn slider(&self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let zoom = self.crop.zoom;
        let frac = ((zoom - 1.0) / (MAX_ZOOM - 1.0)).clamp(0.0, 1.0);
        let held = self.zoom_held;
        let grow = motion::follow("cropper-zoom-held", if held { 1.0 } else { 0.0 }, window, cx);
        let thumb = SliderThumb::new(&self.zoom)
            .absolute()
            .top(px(-6.0 - 2.5 * grow))
            .left(relative(frac))
            .ml(px(-10.0 - 2.5 * grow))
            .size(px(20.0 + 5.0 * grow))
            .rounded_full()
            .border(px(3.0))
            .border_color(p.primary)
            .bg(p.background)
            .cursor_pointer();
        let bubble = held.then(|| {
            div().absolute().bottom(px(20.0)).left(relative(frac)).child(
                div().relative().left(px(-40.0)).w(px(80.0)).flex().justify_center().child(
                    div()
                        .rounded(radius_lg())
                        .bg(p.primary)
                        .px(px(8.0))
                        .py(px(2.0))
                        .text_xs()
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .text_color(p.primary_foreground)
                        .child(format!("{}%", (zoom * 100.0).round())),
                ),
            )
        });
        let track =
            SliderTrack::new(&self.zoom).relative().w_full().h(px(20.0)).flex().items_center().cursor_pointer().child(
                SliderIndicator::new(&self.zoom)
                    .relative()
                    .w_full()
                    .h(px(8.0))
                    .rounded_full()
                    .bg(p.muted)
                    .child(div().absolute().top_0().bottom_0().left_0().w(relative(frac)).rounded_full().bg(p.primary))
                    .child(thumb),
            );
        div()
            .pt(px(28.0))
            .pb(px(4.0))
            .child(BaseSlider::new(&self.zoom).relative().w_full().child(track).children(bubble))
            .into_any_element()
    }
}

/// Everything outside the shape dims and the shape's edge glows. GPUI can't
/// cut a hole, so the dimming is a thick border around it with the shape's
/// corners on its inside.
fn shape_mask(kind: PictureKind, frame: Size) -> impl IntoElement {
    let (fw, fh) = (frame.width, frame.height);
    // The web's: `rounded-full`, `rounded-[32%]` for icons, `rounded-2xl` for banners.
    let r = if kind.shape().round {
        fw.min(fh) / 2.0
    } else if kind == PictureKind::Icon {
        fw * 0.32
    } else {
        f32::from(radius_2xl())
    };
    let ring = 2.0;
    // Thick enough to reach the frame's corners from the shape's.
    let b = r.max(1.0);
    div()
        .absolute()
        .inset_0()
        .child(
            div()
                .absolute()
                .left(px(-b))
                .top(px(-b))
                .w(px(fw + 2.0 * b))
                .h(px(fh + 2.0 * b))
                .rounded(px(r + b))
                .border(px(b))
                .border_color(gpui_kit::hsla(0.0, 0.0, 0.0, 0.5)),
        )
        .child(
            div()
                .absolute()
                .left(px(-ring))
                .top(px(-ring))
                .w(px(fw + 2.0 * ring))
                .h(px(fh + 2.0 * ring))
                .rounded(px(r + ring))
                .border(px(ring))
                .border_color(gpui_kit::hsla(0.0, 0.0, 1.0, 0.8)),
        )
}

/// The frame's own rounded corners, drawn in the card's color over the
/// picture, since GPUI clips children only to a rectangle.
fn corners(frame: Size, p: &Palette) -> impl IntoElement {
    let r = f32::from(radius_2xl());
    div()
        .absolute()
        .left(px(-r))
        .top(px(-r))
        .w(px(frame.width + 2.0 * r))
        .h(px(frame.height + 2.0 * r))
        .rounded(px(2.0 * r))
        .border(px(r))
        .border_color(p.card)
}

/// A rule-of-thirds grid while the picture is being moved.
fn grid(frame: Size) -> impl IntoElement {
    let line = gpui_kit::hsla(0.0, 0.0, 1.0, 0.35);
    let mut el = div().absolute().inset_0();
    for at in [0.33, 0.66] {
        el = el
            .child(div().absolute().top_0().bottom_0().left(px(frame.width * at)).w(px(1.0)).bg(line))
            .child(div().absolute().left_0().right_0().top(px(frame.height * at)).h(px(1.0)).bg(line));
    }
    motion::fade_in(el, "cropper-grid", Duration::from_millis(200))
}

/// "Drag to move", while the picture sits as it started.
fn drag_hint() -> impl IntoElement {
    div().absolute().left_0().right_0().bottom(px(12.0)).flex().justify_center().child(motion::rise(
        div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .rounded_full()
            .bg(gpui_kit::hsla(0.0, 0.0, 0.0, 0.6))
            .px(px(12.0))
            .py(px(4.0))
            .text_xs()
            .font_weight(FontWeight::BOLD)
            .whitespace_nowrap()
            .text_color(gpui_kit::white())
            .child(icon("move").size(px(14.0)))
            .child(t("workspace.picture.dragToMove")),
        "cropper-hint",
        Duration::ZERO,
        6.0,
    ))
}

/// The cropper a view has open, and what it listens for.
pub(crate) struct CropSlot {
    pub(crate) view: Entity<PictureCropper>,
    _sub: Subscription,
}

/// What a view does with a picture once it's ready: its bytes and their type.
pub(crate) type Ready<V> = Rc<dyn Fn(&mut V, Vec<u8>, &'static str, &mut Context<V>)>;

/// Asks the system for a picture of `kind` and gets it ready to upload: a GIF
/// as it is, anything else framed in a cropper kept in `slot` (which the view
/// draws over itself). `fail` says what went wrong; `ready` gets the result.
pub(crate) fn choose<V: 'static>(
    core: Arc<Core>,
    kind: PictureKind,
    prompt: String,
    cx: &mut Context<V>,
    slot: fn(&mut V) -> &mut Option<CropSlot>,
    fail: impl Fn(&mut V, String, &mut Context<V>) + 'static,
    ready: Ready<V>,
) {
    let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: Some(prompt.into()),
    });
    cx.spawn(async move |this, cx| {
        let Ok(Ok(Some(paths))) = paths.await else { return };
        let Some(path) = paths.into_iter().next() else { return };
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let Some(mime) = picture_type(&name) else {
            let _ = this.update(cx, |this, cx| fail(this, t("desktop.account.notAPicture"), cx));
            return;
        };
        let read = core.spawn(async move { read_picture(&path).await });
        let bytes = match read.await {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(err)) => {
                let _ = this.update(cx, |this, cx| fail(this, err.message, cx));
                return;
            }
            Err(_) => return,
        };
        let _ = this.update(cx, |this, cx| {
            // A GIF would stop moving if it were redrawn, so it goes up as it is.
            if mime == "image/gif" {
                return ready(this, bytes, mime, cx);
            }
            let view = cx.new(|cx| PictureCropper::new(kind, bytes, mime, cx));
            let sub = cx.subscribe(&view, move |this, _, event: &CropEvent, cx| {
                *slot(this) = None;
                if let CropEvent::Done(png) = event {
                    ready(this, png.clone(), "image/png", cx);
                }
                cx.notify();
            });
            *slot(this) = Some(CropSlot { view, _sub: sub });
            cx.notify();
        });
    })
    .detach();
}

/// The cropper a view has open, to draw over it.
pub(crate) fn layer(slot: &Option<CropSlot>) -> Option<AnyElement> {
    slot.as_ref().map(|s| s.view.clone().into_any_element())
}
