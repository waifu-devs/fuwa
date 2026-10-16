//! Videos and sounds in messages, played where they are (the web's `Video`
//! and `Audio` in `chat/Attachments.tsx`, whose `<video>` and `<audio>`
//! have the browser's controls): a video in its box with a play button over
//! it and, once it's started, a bar to pause, jump, see the time and set the
//! sound; a sound file in a card with the same controls under its name. The
//! file is fetched the first time it's played (`core/media.rs` plays it).
//! Starting one pauses the others, and one that has left the screen pauses.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Bounds, Corners, FontWeight, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _,
    Pixels, RenderImage, SharedString, StatefulInteractiveElement as _, Styled as _, WeakEntity, div, px, rgb,
};

use crate::core::attachments;
use crate::core::i18n::t;
use crate::core::media::{Player, Unplayable};
use crate::pb;
use crate::ui::app::FuwaApp;
use crate::ui::theme::{Palette, alpha};
use crate::ui::widgets::icon;

/// Players kept with their files after they've stopped, so playing one again starts at once.
const KEPT: usize = 3;
/// A playing one not drawn for this long has left the screen, and pauses.
const OFF_SCREEN: Duration = Duration::from_millis(1500);

enum State {
    /// Coming from the instance: this much of it so far.
    Fetching {
        got: Arc<AtomicU64>,
        size: i64,
    },
    Ready(Rc<Player>),
    Failed(String),
}

struct Entry {
    state: State,
    /// The frame on screen.
    shown: Option<Arc<RenderImage>>,
    /// Frames replaced, to leave the window's atlas once they're painted over.
    retired: Vec<Arc<RenderImage>>,
    used: Instant,
    drawn: Instant,
}

#[derive(Default)]
struct Media {
    entries: HashMap<String, Entry>,
    /// The sound, the same for every player: how loud, and whether it's muted.
    volume: Option<(f32, bool)>,
    /// Whether the watch for players that left the screen is running.
    watching: bool,
}

thread_local! {
    static MEDIA: RefCell<Media> = RefCell::new(Media::default());
}

/// Where a file's player is kept: its media id, or its link.
fn id_of(file: &pb::Attachment) -> String {
    crate::ui::attachments::spoiler_id(file).to_owned()
}

fn player(id: &str) -> Option<Rc<Player>> {
    MEDIA.with(|m| match m.borrow().entries.get(id).map(|e| &e.state) {
        Some(State::Ready(p)) => Some(p.clone()),
        _ => None,
    })
}

/// What a file's controls show.
enum Shown {
    Idle,
    Fetching(f32),
    Failed(String),
    Playing { player: Rc<Player> },
}

fn shown(id: &str) -> Shown {
    MEDIA.with(|m| match m.borrow().entries.get(id).map(|e| &e.state) {
        None => Shown::Idle,
        Some(State::Fetching { got, size }) => {
            let got = got.load(Ordering::Relaxed) as f32;
            Shown::Fetching(if *size > 0 { (got / *size as f32).clamp(0.0, 1.0) } else { 0.0 })
        }
        Some(State::Failed(why)) => Shown::Failed(why.clone()),
        Some(State::Ready(player)) => Shown::Playing { player: player.clone() },
    })
}

/// Plays or pauses a file, fetching it the first time (and again after it failed).
fn toggle(file: &pb::Attachment, key: &str, cx: &mut gpui_kit::App) {
    let id = id_of(file);
    if let Some(player) = player(&id) {
        if player.is_paused() {
            start(&id, &player, cx);
        } else {
            player.pause();
        }
        cx.refresh_windows();
        return;
    }
    if matches!(shown(&id), Shown::Fetching(_)) {
        return;
    }
    let Some(app) = crate::ui::mentions::app() else { return };
    let got = Arc::new(AtomicU64::new(0));
    let now = Instant::now();
    MEDIA.with(|m| {
        let mut m = m.borrow_mut();
        m.entries.insert(
            id.clone(),
            Entry {
                state: State::Fetching { got: got.clone(), size: file.size },
                shown: None,
                retired: Vec::new(),
                used: now,
                drawn: now,
            },
        );
        forget_stopped(&mut m);
    });
    let (key, url, size) = (key.to_owned(), file.url.clone(), file.size);
    let (content_type, name) = (file.content_type.clone(), file.filename.clone());
    let _ = app.update(cx, |this, cx| {
        let core = this.core.clone();
        let fetch = async move {
            let bytes = core.fetch_media(&key, &url, size, got).await.map_err(|p| p.message)?;
            let opened = tokio::task::spawn_blocking(move || Player::open(bytes, &content_type, &name)).await;
            match opened {
                Ok(Ok(player)) => Ok(player),
                Ok(Err(Unplayable::Kind)) => Err(t("desktop.media.cantPlay")),
                Ok(Err(Unplayable::Broken)) | Err(_) => Err(t("desktop.media.broken")),
            }
        };
        this.run(cx, fetch, move |_, result, cx| {
            MEDIA.with(|m| {
                let mut m = m.borrow_mut();
                // Gone meanwhile (pushed out by others): it's fetched again when played.
                let Some(entry) = m.entries.get_mut(&id) else { return };
                entry.state = match result {
                    Ok(player) => State::Ready(Rc::new(player)),
                    Err(why) => State::Failed(why),
                };
            });
            if let Some(player) = player(&id) {
                start(&id, &player, cx);
            }
            cx.notify();
        });
    });
    cx.refresh_windows();
}

/// Plays one, pausing the others, at the sound everything plays at.
fn start(id: &str, player: &Rc<Player>, cx: &mut gpui_kit::App) {
    let now = Instant::now();
    let watch = MEDIA.with(|m| {
        let mut m = m.borrow_mut();
        let (volume, muted) = m.volume.unwrap_or((1.0, false));
        player.set_volume(volume);
        player.set_muted(muted);
        for (other, entry) in &mut m.entries {
            match &entry.state {
                State::Ready(p) if other != id => p.pause(),
                _ => {
                    entry.used = now;
                    entry.drawn = now;
                }
            }
        }
        !std::mem::replace(&mut m.watching, true)
    });
    player.play();
    crate::core::reports::used(if player.has_picture() { "attachment.video" } else { "attachment.audio" });
    if watch {
        watch_off_screen(cx);
    }
}

/// Keeps the few players used last; the rest let go of their files.
fn forget_stopped(m: &mut Media) {
    loop {
        let stopped: Vec<(&String, &Entry)> = m
            .entries
            .iter()
            .filter(|(_, e)| match &e.state {
                State::Ready(p) => p.is_paused(),
                State::Failed(_) => true,
                State::Fetching { .. } => false,
            })
            .collect();
        if m.entries.len() <= KEPT || stopped.is_empty() {
            return;
        }
        let Some(oldest) = stopped.iter().min_by_key(|(_, e)| e.used).map(|(id, _)| (*id).clone()) else { return };
        m.entries.remove(&oldest);
    }
}

/// While something plays, pauses what's no longer drawn: its message
/// scrolled away, or another channel opened.
fn watch_off_screen(cx: &mut gpui_kit::App) {
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(Duration::from_millis(500)).await;
            let (playing, stopped) = MEDIA.with(|m| {
                let mut m = m.borrow_mut();
                let now = Instant::now();
                let (mut playing, mut stopped) = (false, false);
                for entry in m.entries.values() {
                    let State::Ready(player) = &entry.state else { continue };
                    if player.is_paused() {
                        continue;
                    }
                    if now - entry.drawn > OFF_SCREEN {
                        player.pause();
                        stopped = true;
                    } else {
                        playing = true;
                    }
                }
                m.watching = playing;
                (playing, stopped)
            });
            if stopped {
                cx.update(|cx| cx.refresh_windows());
            }
            if !playing {
                break;
            }
        }
    })
    .detach();
}

/// The sound every player plays at, kept for this run of the app.
fn set_volume(volume: Option<f32>, muted: Option<bool>) {
    MEDIA.with(|m| {
        let mut m = m.borrow_mut();
        let (mut v, mut mute) = m.volume.unwrap_or((1.0, false));
        if let Some(volume) = volume {
            v = volume.clamp(0.0, 1.0);
            // Turning it up unmutes it, as a browser's does.
            mute = v == 0.0;
        }
        if let Some(muted) = muted {
            mute = muted;
        }
        m.volume = Some((v, mute));
        for entry in m.entries.values() {
            if let State::Ready(p) = &entry.state {
                p.set_volume(v);
                p.set_muted(mute);
            }
        }
    });
}

fn volume() -> (f32, bool) {
    MEDIA.with(|m| m.borrow().volume.unwrap_or((1.0, false)))
}

/// "1:05", or "1:02:05" past an hour.
fn clock(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

// ───────────────────────── Drawing ─────────────────────────

/// Draws the newest frame of `id`'s video in its box, scaled to fit, and
/// asks for the next frame while it plays (or while it's coming).
fn picture(id: String) -> impl IntoElement {
    gpui_kit::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let now = Instant::now();
            let (image, moving, gone) = MEDIA.with(|m| {
                let mut m = m.borrow_mut();
                let Some(entry) = m.entries.get_mut(&id) else { return (None, false, Vec::new()) };
                entry.drawn = now;
                let gone = std::mem::take(&mut entry.retired);
                let moving = match &entry.state {
                    State::Fetching { .. } => true,
                    State::Ready(player) => {
                        if let Some(frame) = player.take_picture()
                            && let Some(buffer) = image::RgbaImage::from_raw(frame.width, frame.height, frame.bgra)
                        {
                            let image = Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]));
                            if let Some(old) = entry.shown.replace(image) {
                                entry.retired.push(old);
                            }
                        }
                        !player.is_paused()
                    }
                    State::Failed(_) => false,
                };
                (entry.shown.clone(), moving, gone)
            });
            for old in gone {
                let _ = window.drop_image(old);
            }
            if let Some(image) = image {
                let size = image.size(0);
                let fit = contain(bounds, size.width.0 as f32, size.height.0 as f32);
                let _ = window.paint_image(bounds, fit, Corners::default(), image, 0, false);
            }
            if moving {
                window.request_animation_frame();
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// Where a picture this size goes in `bounds`, as big as fits, centred.
fn contain(bounds: Bounds<Pixels>, w: f32, h: f32) -> Bounds<Pixels> {
    let (bw, bh) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    if w <= 0.0 || h <= 0.0 || bw <= 0.0 || bh <= 0.0 {
        return bounds;
    }
    let scale = (bw / w).min(bh / h);
    let (fw, fh) = (w * scale, h * scale);
    Bounds::new(
        gpui_kit::point(bounds.origin.x + px((bw - fw) / 2.0), bounds.origin.y + px((bh - fh) / 2.0)),
        gpui_kit::size(px(fw), px(fh)),
    )
}

/// Notes that `id` is on screen, and keeps redrawing while it plays.
fn heartbeat(id: String) -> impl IntoElement {
    gpui_kit::canvas(
        |_, _, _| {},
        move |_, _, window, _| {
            let moving = MEDIA.with(|m| {
                let mut m = m.borrow_mut();
                let Some(entry) = m.entries.get_mut(&id) else { return false };
                entry.drawn = Instant::now();
                match &entry.state {
                    State::Fetching { .. } => true,
                    State::Ready(player) => !player.is_paused(),
                    State::Failed(_) => false,
                }
            });
            if moving {
                window.request_animation_frame();
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// A bar to click or drag along: `at` (0 to 1) filled, `pick` told where it was pressed.
fn track(
    id: SharedString,
    at: f32,
    enabled: bool,
    fill: gpui_kit::Hsla,
    empty: gpui_kit::Hsla,
    pick: impl Fn(f32, &mut gpui_kit::App) + 'static,
) -> AnyElement {
    let bounds: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::default();
    let pick = Rc::new(pick);
    let fraction = {
        let bounds = bounds.clone();
        move |x: Pixels| {
            let b = bounds.get()?;
            let w = f32::from(b.size.width);
            (w > 0.0).then(|| (f32::from(x - b.origin.x) / w).clamp(0.0, 1.0))
        }
    };
    let measure = {
        let bounds = bounds.clone();
        gpui_kit::canvas(move |b, _, _| bounds.set(Some(b)), |_, _, _, _| {}).absolute().top_0().left_0().size_full()
    };
    let at = at.clamp(0.0, 1.0);
    div()
        .id(id)
        .relative()
        .flex_1()
        .min_w(px(24.0))
        .h(px(16.0))
        .flex()
        .items_center()
        .when(enabled, |el| {
            let (down, drag) = (fraction.clone(), fraction);
            let (pick_down, pick_drag) = (pick.clone(), pick);
            el.cursor_pointer()
                .on_mouse_down(MouseButton::Left, move |e, _, cx| {
                    cx.stop_propagation();
                    if let Some(f) = down(e.position.x) {
                        pick_down(f, cx);
                    }
                })
                .on_mouse_move(move |e: &gpui_kit::MouseMoveEvent, _, cx| {
                    if e.pressed_button == Some(MouseButton::Left)
                        && let Some(f) = drag(e.position.x)
                    {
                        pick_drag(f, cx);
                    }
                })
                .on_click(|_, _, cx| cx.stop_propagation())
        })
        .child(measure)
        .child(
            div()
                .relative()
                .w_full()
                .h(px(4.0))
                .rounded_full()
                .bg(empty)
                .child(div().absolute().top_0().left_0().h_full().w(gpui_kit::relative(at)).rounded_full().bg(fill)),
        )
        .child(
            // The knob, where it's at.
            div()
                .absolute()
                .top(px(3.0))
                .left(gpui_kit::relative(at))
                .ml(px(-5.0))
                .size(px(10.0))
                .rounded_full()
                .bg(fill),
        )
        .into_any_element()
}

/// A round button with an icon, for the bars.
fn button(
    id: SharedString,
    glyph: &'static str,
    tip: String,
    size: f32,
    p: &Palette,
    on: bool,
    press: impl Fn(&mut gpui_kit::App) + 'static,
) -> AnyElement {
    let inner = match glyph {
        "play" | "pause" => gpui_kit::component::Icon::default()
            .path(SharedString::from(format!("fuwa/{glyph}-filled.svg")))
            .flex_none(),
        _ => icon(glyph),
    }
    .size(px(size * 0.5));
    let hover = alpha(p.foreground, 0.12);
    div()
        .id(id)
        .size(px(size))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .when(on, |el| el.bg(p.primary).text_color(p.primary_foreground))
        .when(!on, move |el| el.hover(move |s| s.bg(hover)))
        .active(|s| s.scale(0.9))
        .tooltip(move |window, cx| crate::ui::overlay::Tip::new(tip.clone()).build(window, cx))
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            press(cx);
        })
        // The play triangle sits a pixel right of centre, where it looks centred.
        .child(if glyph == "play" {
            div().ml(px(1.0)).child(inner).into_any_element()
        } else {
            inner.into_any_element()
        })
        .into_any_element()
}

/// Play or pause, the time, the place in it, and the sound: the browser's controls.
fn controls(id: &str, file: &pb::Attachment, key: &str, light: bool, p: &Palette) -> AnyElement {
    let state = shown(id);
    let (at, length, paused) = match &state {
        Shown::Playing { player } => {
            let (at, length) = player.position_ms();
            (at, length, player.is_paused())
        }
        _ => (0, 0, true),
    };
    let fg: gpui_kit::Hsla = if light { gpui_kit::white() } else { p.foreground.into() };
    let empty = if light { gpui_kit::hsla(0.0, 0.0, 1.0, 0.3) } else { alpha(p.foreground, 0.15) };
    let fill: gpui_kit::Hsla = if light { gpui_kit::white() } else { p.primary.into() };
    let play = {
        let (file, key) = (file.clone(), key.to_owned());
        let (glyph, tip) = if paused { ("play", t("desktop.media.play")) } else { ("pause", t("desktop.media.pause")) };
        button(SharedString::from(format!("media-play|{id}")), glyph, tip, 32.0, p, !light, move |cx| {
            toggle(&file, &key, cx)
        })
    };
    let time = match &state {
        Shown::Fetching(f) => format!("{}%", (f * 100.0).round() as u32),
        _ if length > 0 => format!("{} / {}", clock(at), clock(length)),
        _ => clock(at),
    };
    let place = {
        let id = id.to_owned();
        let fraction = if length > 0 { at as f32 / length as f32 } else { 0.0 };
        track(SharedString::from(format!("media-track|{id}")), fraction, length > 0, fill, empty, move |f, cx| {
            if let Some(player) = player(&id) {
                player.seek(f);
                cx.refresh_windows();
            }
        })
    };
    let (volume, muted) = volume();
    let mute = {
        let glyph = if muted || volume == 0.0 {
            "volume-x"
        } else if volume < 0.5 {
            "volume-1"
        } else {
            "volume-2"
        };
        let tip = if muted { t("desktop.media.unmute") } else { t("desktop.media.mute") };
        button(SharedString::from(format!("media-mute|{id}")), glyph, tip, 28.0, p, false, move |cx| {
            set_volume(None, Some(!muted));
            cx.refresh_windows();
        })
    };
    let loudness = div().w(px(56.0)).flex_none().flex().child(track(
        SharedString::from(format!("media-volume|{id}")),
        if muted { 0.0 } else { volume },
        true,
        fill,
        empty,
        |f, cx| {
            set_volume(Some(f), None);
            cx.refresh_windows();
        },
    ));
    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .w_full()
        .text_color(fg)
        .child(play)
        .child(
            div()
                .flex_none()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(if light { fg } else { p.muted_foreground.into() })
                .child(time),
        )
        .child(place)
        .child(mute)
        .child(loudness)
        .into_any_element()
}

/// A video in a message (the web's `Video`): its box, black until it plays,
/// with the play button over it, and its name, size and save button under it.
pub(crate) fn video(
    mid: &str,
    n: usize,
    file: &pb::Attachment,
    p: &Palette,
    this: &WeakEntity<FuwaApp>,
    key: &str,
) -> AnyElement {
    let id = id_of(file);
    let (w, h) = attachments::fit_box(file.width, file.height, crate::ui::attachments::MEDIA_BOX);
    let state = shown(&id);
    let started = matches!(state, Shown::Playing { .. });
    let paused = match &state {
        Shown::Playing { player } => player.is_paused(),
        _ => true,
    };
    let group = SharedString::from(format!("media-box|{mid}|{n}"));
    let middle: Option<AnyElement> = match &state {
        Shown::Idle => Some(big_play(&id, p)),
        Shown::Playing { .. } if paused => Some(big_play(&id, p)),
        Shown::Fetching(f) => Some(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(6.0))
                .text_color(gpui_kit::white())
                .child(spinner(&id))
                .child(div().text_xs().font_weight(FontWeight::BOLD).child(format!("{}%", (f * 100.0).round() as u32)))
                .into_any_element(),
        ),
        Shown::Failed(why) => Some(
            div()
                .max_w(px(w - 32.0))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(6.0))
                .text_center()
                .text_xs()
                .text_color(gpui_kit::hsla(0.0, 0.0, 1.0, 0.85))
                .child(icon("circle-alert").size(px(20.0)))
                .child(why.clone())
                .into_any_element(),
        ),
        Shown::Playing { .. } => None,
    };
    let toggle_click = {
        let (file, key) = (file.clone(), key.to_owned());
        move |_: &gpui_kit::ClickEvent, _: &mut gpui_kit::Window, cx: &mut gpui_kit::App| toggle(&file, &key, cx)
    };
    let screen = div()
        .id(SharedString::from(format!("media-video|{mid}|{n}")))
        .group(group.clone())
        .relative()
        .w_full()
        .h(px(h))
        .rounded(crate::ui::theme::radius_xl())
        .overflow_hidden()
        .border_1()
        .border_color(p.border)
        .bg(rgb(0x000000))
        .cursor_pointer()
        .on_click(toggle_click)
        .child(picture(id.clone()))
        .children(middle.map(|m| div().absolute().inset_0().flex().items_center().justify_center().child(m)))
        .when(started, |el| {
            // The bar shows while it's paused, or under the pointer while it plays.
            let bar = div()
                .id(SharedString::from(format!("media-bar|{mid}|{n}")))
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .px(px(8.0))
                .pt(px(20.0))
                .pb(px(6.0))
                .bg(gpui_kit::linear_gradient(
                    180.0,
                    gpui_kit::linear_color_stop(gpui_kit::hsla(0.0, 0.0, 0.0, 0.0), 0.0),
                    gpui_kit::linear_color_stop(gpui_kit::hsla(0.0, 0.0, 0.0, 0.65), 1.0),
                ))
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(controls(&id, file, key, true, p));
            el.child(if paused { bar } else { bar.opacity(0.0).group_hover(group.clone(), |s| s.opacity(1.0)) })
        });
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .w(px(w))
        .max_w_full()
        .child(screen)
        .child(crate::ui::attachments::file_line(&format!("{mid}|{n}"), file, p, this, key))
        .into_any_element()
}

/// The play button in the middle of a video that isn't playing.
fn big_play(id: &str, p: &Palette) -> AnyElement {
    div()
        .id(SharedString::from(format!("media-big|{id}")))
        .size(px(52.0))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(alpha(p.primary, 0.9))
        .text_color(p.primary_foreground)
        .shadow_lg()
        .hover(|s| s.scale(1.06))
        .child(
            div()
                .ml(px(2.0))
                .child(gpui_kit::component::Icon::default().path("fuwa/play-filled.svg").flex_none().size(px(22.0))),
        )
        .into_any_element()
}

/// A turning ring while a file comes in.
fn spinner(id: &str) -> AnyElement {
    use gpui_kit::{Animation, AnimationExt as _};
    icon("loader-circle")
        .size(px(24.0))
        .with_animation(
            SharedString::from(format!("media-spin|{id}")),
            Animation::new(Duration::from_millis(900)).repeat(),
            |el, t| el.rotate(gpui_kit::percentage(t)),
        )
        .into_any_element()
}

/// A sound file in a message (the web's `Audio`): a card with its icon,
/// name, size and save button, and the controls under them.
pub(crate) fn audio(
    mid: &str,
    n: usize,
    file: &pb::Attachment,
    p: &Palette,
    this: &WeakEntity<FuwaApp>,
    key: &str,
) -> AnyElement {
    let id = id_of(file);
    let problem = match shown(&id) {
        Shown::Failed(why) => Some(why),
        Shown::Playing { player } if player.had_no_speakers() => Some(t("desktop.media.noSpeakers")),
        _ => None,
    };
    div()
        .id(SharedString::from(format!("media-audio|{mid}|{n}")))
        .relative()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .w(px(448.0))
        .max_w_full()
        .p(px(12.0))
        .rounded(crate::ui::theme::radius_2xl())
        .bg(crate::ui::theme::mix(p.chat_surface.into(), p.card, 0.7))
        .border_1()
        .border_color(p.border)
        .shadow(crate::ui::polls::shadow_sm())
        .child(heartbeat(id.clone()))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(crate::ui::attachments::badge(
                    if file.filename.is_empty() { "audio.mp3" } else { &file.filename },
                    40.0,
                ))
                .child(crate::ui::attachments::file_line(&format!("{mid}|{n}"), file, p, this, key)),
        )
        .child(match problem {
            Some(why) => div()
                .h(px(32.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .text_xs()
                .text_color(p.destructive)
                .child(icon("circle-alert").size(px(16.0)))
                .child(why)
                .into_any_element(),
            None => {
                div().h(px(32.0)).flex().items_center().child(controls(&id, file, key, false, p)).into_any_element()
            }
        })
        .into_any_element()
}
