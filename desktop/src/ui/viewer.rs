//! Pictures opened large over everything (the web's `chat/ImageViewer.tsx`):
//! a message's attachments, its link previews' and embeds' pictures, a GIF,
//! and the pictures an encrypted message carries once this device opened
//! them. The wheel or a trackpad pinch zooms around the pointer, a zoomed
//! picture is dragged to look around, a double click goes between fitted and
//! 1:1, the arrows (on screen and on the keyboard) step through the
//! message's other pictures, + - and 0 zoom, and Escape, a click outside or
//! the button close it. The rules are `core::zoom`'s; where it stands is kept
//! here, each picture starting fitted.

use std::cell::RefCell;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, ImgResourceLoader, InteractiveElement as _, IntoElement, Keystroke, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, ParentElement as _, PinchEvent, Pixels, Point, Resource,
    ScrollWheelEvent, SharedString, StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window, div, img,
    px, radians,
};

use crate::core::attachments;
use crate::core::i18n::{Arg, t, t_with};
use crate::core::zoom::{self, FITTED, View};
use crate::pb;
use crate::ui::app::{Dialog, FuwaApp};
use crate::ui::motion;
use crate::ui::overlay::{Tip, shade, shadow_2xl};
use crate::ui::spoilers;
use crate::ui::theme::radius_lg;
use crate::ui::widgets::icon;

/// Room kept clear around the picture, and under it for the caption.
const EDGE: f32 = 24.0;
const CAPTION: f32 = 64.0;

/// Where a picture comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// An address on a fuwa instance.
    Url(String),
    /// A file of an encrypted message this device opened, by its media id
    /// (`sealed_files` keeps it). Never opened anywhere else, so it has no
    /// "open the original".
    Sealed(String),
}

/// One picture the viewer can show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Viewed {
    pub source: Source,
    pub name: String,
    /// Its size as the sender's app gave it, used until it loads (0 when unknown).
    pub width: i32,
    pub height: i32,
    /// Its size in bytes, when known.
    pub bytes: i64,
}

impl Viewed {
    /// A message's attachment.
    pub fn attachment(file: &pb::Attachment) -> Self {
        Viewed {
            source: Source::Url(file.url.clone()),
            name: file.filename.clone(),
            width: file.width,
            height: file.height,
            bytes: file.size,
        }
    }

    /// A picture at an address, with no size known (an embed's, a link preview's).
    pub fn url(url: &str, name: &str, width: i32, height: i32) -> Self {
        Viewed { source: Source::Url(url.to_owned()), name: name.to_owned(), width, height, bytes: 0 }
    }
}

/// The pictures a message shows in a row that the viewer steps through:
/// those not covered as spoilers (an uncovered one counts once it's clicked).
pub fn viewable_attachments(files: &[&pb::Attachment]) -> Vec<Viewed> {
    files
        .iter()
        .filter(|f| spoilers::file_shown(crate::ui::attachments::spoiler_id(f), f.spoiler) == spoilers::Shown::Revealed)
        .map(|f| Viewed::attachment(f))
        .collect()
}

/// A picture's name for its caption and saving: the last part of its address.
pub fn name_of(url: &str, fallback: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.path_segments().and_then(|mut s| s.rfind(|p| !p.is_empty()).map(str::to_owned)))
        .map(|last| percent_decode(&last))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| fallback.to_owned())
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = text.get(i + 1..i + 3)
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Where the open picture stands.
#[derive(Default)]
struct State {
    view: Option<View>,
    /// Where the left button went down, and whether it has dragged since.
    press: Option<(Point<Pixels>, bool)>,
    /// Whether that press was on the picture (a click elsewhere closes it).
    on_picture: bool,
    /// The picture's own size and the room it has, as last drawn.
    natural: (f32, f32),
    room: (f32, f32),
    /// The room's middle, in the window.
    middle: (f32, f32),
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn view() -> View {
    STATE.with(|s| s.borrow().view.unwrap_or(FITTED))
}

/// Changes the open picture's zoom by `f`, with its size and room as last drawn.
fn change(f: impl FnOnce(View, (f32, f32), (f32, f32)) -> View) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let next = f(s.view.unwrap_or(FITTED), s.natural, s.room);
        s.view = Some(next);
    });
}

/// From the window to the room's middle.
fn from_middle(at: Point<Pixels>) -> (f32, f32) {
    let middle = STATE.with(|s| s.borrow().middle);
    (f32::from(at.x) - middle.0, f32::from(at.y) - middle.1)
}

fn reset() {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.view = None;
        s.press = None;
    });
}

impl FuwaApp {
    /// Opens `pictures` in the viewer at `at`, fitted.
    pub(crate) fn view_pictures(
        &mut self,
        key: &str,
        pictures: Vec<Viewed>,
        at: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if pictures.is_empty() {
            return;
        }
        reset();
        let at = at.min(pictures.len() - 1);
        self.open_dialog(Dialog::Picture { key: key.to_owned(), pictures, at }, window, cx);
    }

    fn step_picture(&mut self, forward: bool, cx: &mut Context<Self>) {
        if let Some(Dialog::Picture { pictures, at, .. }) = &mut self.dialog
            && pictures.len() > 1
        {
            *at = zoom::step_index(*at, pictures.len(), forward);
            reset();
            cx.notify();
        }
    }

    /// The viewer's keys, while it's open: the arrows step, + - and 0 zoom.
    pub(crate) fn viewer_keys(&mut self, key: &Keystroke, cx: &mut Context<Self>) -> bool {
        if !matches!(self.dialog, Some(Dialog::Picture { .. })) {
            return false;
        }
        let m = &key.modifiers;
        if m.alt || m.platform || m.control {
            return false;
        }
        match key.key.as_str() {
            "left" => self.step_picture(false, cx),
            "right" => self.step_picture(true, cx),
            "+" | "=" => change(|v, n, r| zoom::zoom_at(v, v.scale * zoom::STEP, (0.0, 0.0), n, r)),
            "-" | "_" => change(|v, n, r| zoom::zoom_at(v, v.scale / zoom::STEP, (0.0, 0.0), n, r)),
            "0" => change(|_, _, _| FITTED),
            _ => return false,
        }
        cx.notify();
        true
    }

    fn save_viewed(&mut self, key: &str, picture: &Viewed, cx: &mut Context<Self>) {
        match &picture.source {
            Source::Url(url) => self.save_file(key.to_owned(), url.clone(), picture.name.clone(), picture.bytes, cx),
            Source::Sealed(id) => {
                if let Some((_, bytes)) = crate::ui::sealed_files::opened_picture(id) {
                    crate::ui::sealed_files::save_opened(&cx.weak_entity(), picture.name.clone(), bytes, cx);
                }
            }
        }
    }

    /// The viewer, over everything.
    pub(crate) fn render_picture(&self, open: &Dialog, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Dialog::Picture { key, pictures, at } = open else { return div().into_any_element() };
        let Some(picture) = pictures.get(*at).cloned() else { return div().into_any_element() };
        let (key, at, count) = (key.clone(), *at, pictures.len());
        let many = count > 1;

        let viewport = window.viewport_size();
        let (vw, vh) = (f32::from(viewport.width), f32::from(viewport.height));
        let room = ((vw - EDGE * 2.0).max(1.0), (vh - CAPTION - EDGE * 2.0).max(1.0));
        let middle = (vw / 2.0, (vh - CAPTION) / 2.0);

        // Its own size: the loaded picture's, else what its message said.
        let mut natural = (picture.width.max(0) as f32, picture.height.max(0) as f32);
        let drawn: AnyElement = match &picture.source {
            Source::Url(url) => {
                let resource = Resource::Uri(SharedString::from(url.clone()).into());
                if let Some(Ok(image)) = window.use_asset::<ImgResourceLoader>(&resource, cx) {
                    let s = image.size(0);
                    natural = (i32::from(s.width) as f32, i32::from(s.height) as f32);
                }
                crate::ui::widgets::picture(url.clone()).size_full().object_fit(ObjectFit::Contain).into_any_element()
            }
            Source::Sealed(id) => match crate::ui::sealed_files::opened_picture(id) {
                Some((image, _)) => img(image).size_full().object_fit(ObjectFit::Contain).into_any_element(),
                None => div().into_any_element(),
            },
        };
        if !(natural.0 > 0.0 && natural.1 > 0.0) {
            natural = room;
        }
        let v = STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.natural = natural;
            s.room = room;
            s.middle = middle;
            // The window may have changed size since it was zoomed.
            let v = zoom::clamp(s.view.unwrap_or(FITTED), natural, room);
            s.view = Some(v);
            v
        });
        let fit = zoom::fitted(natural, room);
        let (w, h) = (fit.0 * v.scale, fit.1 * v.scale);
        let (left, top) = (middle.0 + v.x - w / 2.0, middle.1 + v.y - h / 2.0);
        let zoomed = v.scale > 1.001;
        let dragging = STATE.with(|s| s.borrow().press.is_some_and(|(_, moved)| moved));
        let percent = (v.scale / zoom::actual_scale(natural, room) * 100.0).round() as i64;
        let picture_box = (left, top, w, h);

        let white = |a: f32| gpui_kit::Hsla { h: 0.0, s: 0.0, l: 1.0, a };
        // The web's caption: white on black at 60%, its buttons lit white at 15% on hover.
        let round = |id: &'static str, glyph: &str, label: String| {
            div()
                .id(id)
                .size(px(32.0))
                .flex_none()
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(move |s| s.bg(white(0.15)))
                .tooltip(move |window, cx| Tip::new(label.clone()).build(window, cx))
                .child(icon(glyph).size(px(16.0)))
        };
        let original = match &picture.source {
            Source::Url(url) => Some(url.clone()),
            Source::Sealed(_) => None,
        };
        let savable = match &picture.source {
            Source::Url(url) => url::Url::parse(url).is_ok_and(|u| u.path().starts_with("/media/")),
            Source::Sealed(_) => true,
        };
        let caption = div()
            .id("picture-caption-bar")
            .max_w(px((vw - 24.0).max(200.0)))
            .flex()
            .items_center()
            .gap(px(4.0))
            .pl(px(16.0))
            .pr(px(6.0))
            .py(px(6.0))
            .rounded_full()
            .bg(gpui_kit::Hsla { h: 0.0, s: 0.0, l: 0.0, a: 0.6 })
            .text_sm()
            .text_color(white(1.0))
            // A press here is never a click outside the picture.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::BOLD)
                    .child(attachments::short_name(&picture.name, 48)),
            )
            .when(picture.bytes > 0, |el| {
                el.child(
                    div()
                        .ml(px(4.0))
                        .flex_none()
                        .text_color(white(0.7))
                        .child(attachments::format_bytes(picture.bytes)),
                )
            })
            .when(many, |el| {
                el.child(div().ml(px(4.0)).flex_none().text_color(white(0.7)).child(t_with(
                    "chat.viewer.counter",
                    &[("n", Arg::Num(at as i64 + 1)), ("total", Arg::Num(count as i64))],
                )))
            })
            .child(div().mx(px(6.0)).h(px(20.0)).w(px(1.0)).flex_none().bg(white(0.2)))
            .child(
                round("picture-zoom-out", "zoom-out", t("chat.viewer.zoomOut"))
                    .when(!zoomed, |el| el.opacity(0.4))
                    .on_click(cx.listener(|_, _, _, cx| {
                        change(|v, n, r| zoom::zoom_at(v, v.scale / zoom::STEP, (0.0, 0.0), n, r));
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("picture-percent")
                    .h(px(32.0))
                    .w(px(56.0))
                    .flex_none()
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .hover(move |s| s.bg(white(0.15)))
                    .tooltip(move |window, cx| {
                        Tip::new(if zoomed { t("chat.viewer.fit") } else { t("chat.viewer.actual") }).build(window, cx)
                    })
                    .on_click(cx.listener(move |_, _, _, cx| {
                        change(|v, n, r| if zoomed { FITTED } else { zoom::toggle_at(v, (0.0, 0.0), n, r) });
                        cx.notify();
                    }))
                    .child(format!("{percent}%")),
            )
            .child(round("picture-zoom-in", "zoom-in", t("chat.viewer.zoomIn")).on_click(cx.listener(|_, _, _, cx| {
                change(|v, n, r| zoom::zoom_at(v, v.scale * zoom::STEP, (0.0, 0.0), n, r));
                cx.notify();
            })))
            .when_some(original, |el, url| {
                el.child(
                    round("picture-original", "external-link", t("chat.viewer.original"))
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                )
            })
            .when(savable, |el| {
                let (key, picture) = (key.clone(), picture.clone());
                el.child(
                    round(
                        "picture-save",
                        "download",
                        t_with("chat.attachments.download", &[("name", Arg::Str(&picture.name))]),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.save_viewed(&key, &picture, cx))),
                )
            })
            .child(
                // It turns a quarter under the pointer (`hover:rotate-90`).
                round("picture-close", "x", t("common.close"))
                    .hover(|s| s.rotate(radians(std::f32::consts::FRAC_PI_2)))
                    .on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))),
            );

        let side = |id: &'static str, glyph: &str, label: String, forward: bool| {
            div()
                .id(id)
                .absolute()
                .top(px(middle.1 - 22.0))
                .size(px(44.0))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui_kit::hsla(0.0, 0.0, 0.0, 0.55))
                .text_color(white(1.0))
                .cursor_pointer()
                .hover(|s| s.bg(gpui_kit::hsla(0.0, 0.0, 0.0, 0.75)).scale(1.08))
                .active(|s| s.scale(0.92))
                .tooltip(move |window, cx| Tip::new(label.clone()).build(window, cx))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _, cx| this.step_picture(forward, cx)))
                .child(icon(glyph).size(px(24.0)))
        };

        let stage = shade("picture-scrim", 0.85, 4.0)
            .when(zoomed, |el| if dragging { el.cursor_grabbing() } else { el.cursor_grab() })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_, e: &MouseDownEvent, _, cx| {
                    let (x, y) = (f32::from(e.position.x), f32::from(e.position.y));
                    let (l, t, w, h) = picture_box;
                    let on = x >= l && x <= l + w && y >= t && y <= t + h;
                    STATE.with(|s| {
                        let mut s = s.borrow_mut();
                        s.press = Some((e.position, false));
                        s.on_picture = on;
                    });
                    if e.click_count == 2 && on {
                        let at = from_middle(e.position);
                        change(|v, n, r| zoom::toggle_at(v, at, n, r));
                        cx.notify();
                    }
                }),
            )
            .on_mouse_move(cx.listener(|_, e: &MouseMoveEvent, _, cx| {
                let Some((last, moved)) = STATE.with(|s| s.borrow().press) else { return };
                if e.pressed_button != Some(MouseButton::Left) {
                    STATE.with(|s| s.borrow_mut().press = None);
                    cx.notify();
                    return;
                }
                let (dx, dy) = (f32::from(e.position.x - last.x), f32::from(e.position.y - last.y));
                // A press that moves past a few pixels is a drag, never a click.
                if !moved && dx.abs() + dy.abs() < 4.0 {
                    return;
                }
                STATE.with(|s| s.borrow_mut().press = Some((e.position, true)));
                if view().scale > 1.001 {
                    change(|v, n, r| zoom::pan_by(v, dx, dy, n, r));
                }
                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    let (press, on) = STATE.with(|s| {
                        let mut s = s.borrow_mut();
                        (s.press.take(), s.on_picture)
                    });
                    match press {
                        Some((_, false)) if !on => this.close_dialog(cx),
                        _ => cx.notify(),
                    }
                }),
            )
            .on_scroll_wheel(cx.listener(|_, e: &ScrollWheelEvent, _, cx| {
                cx.stop_propagation();
                let dy = f32::from(e.delta.pixel_delta(px(100.0 / 3.0)).y);
                // GPUI's wheel moves content down for a positive delta, the opposite of the web's deltaY.
                let factor = zoom::wheel_factor(-dy);
                let at = from_middle(e.position);
                change(|v, n, r| zoom::zoom_at(v, v.scale * factor, at, n, r));
                cx.notify();
            }))
            .on_pinch(cx.listener(|_, e: &PinchEvent, _, cx| {
                let at = from_middle(e.position);
                change(|v, n, r| zoom::zoom_at(v, v.scale * (1.0 + e.delta), at, n, r));
                cx.notify();
            }))
            .child(
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .w(px(w))
                    .h(px(h))
                    .rounded(radius_lg())
                    .shadow(shadow_2xl())
                    .child(motion::grow_in(
                        div().size_full().child(drawn),
                        SharedString::from(format!("picture-zoom|{at}")),
                        0.94,
                    )),
            )
            .when(many, |el| {
                el.child(side("picture-previous", "chevron-left", t("chat.viewer.previous"), false).left(px(12.0)))
                    .child(side("picture-next", "chevron-right", t("chat.viewer.next"), true).right(px(12.0)))
            })
            .child(div().absolute().left_0().right_0().bottom(px(16.0)).flex().justify_center().child(motion::rise(
                caption,
                "picture-caption",
                Duration::from_millis(50),
                8.0,
            )));
        motion::fade_in(stage, "picture-fade", Duration::from_millis(160)).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_come_from_the_address() {
        assert_eq!(name_of("https://fuwa.chat/media/outside/abc/cat%20pic.png", "Picture"), "cat pic.png");
        assert_eq!(name_of("https://fuwa.chat/media/x/", "Picture"), "x");
        assert_eq!(name_of("https://fuwa.chat/", "Picture"), "Picture");
        assert_eq!(name_of("not a link", "Picture"), "Picture");
    }
}
