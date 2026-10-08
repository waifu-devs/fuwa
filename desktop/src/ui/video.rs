//! Cameras and shared screens on screen (the web's components/calls/Video.tsx):
//! each feed's latest picture as a GPUI image, shared by every window that
//! shows it, the tile media that shows a camera or the person's avatar, and
//! saying how big each window shows each feed, so the call asks for the
//! size that fits and stops what nobody sees.
//!
//! A new picture becomes a new image in the windows' atlases; the one it
//! replaced leaves them at once, so a feed only ever holds one.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{
    AnyElement, App, Div, FontWeight, Global, InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _,
    RenderImage, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window, div,
    px,
};

use crate::core::Core;
use crate::core::i18n::t;
use crate::ui::call_parts::red;
use crate::ui::motion;
use crate::ui::theme::{Palette, alpha, radius_md, radius_xl};
use crate::ui::widgets::icon;

/// How long a picture no window drew stays in the atlases.
const UNUSED: Duration = Duration::from_secs(2);

struct Shown {
    image: Arc<RenderImage>,
    used: Instant,
}

/// Every feed's picture as it's drawn now, and what each window showed this frame.
#[derive(Default)]
pub(crate) struct VideoCache {
    shown: HashMap<String, Shown>,
    /// How tall each window draws each feed this frame, in device pixels.
    drawing: HashMap<u64, HashMap<String, u32>>,
}

impl Global for VideoCache {}

/// The picture to draw of `feed` now: the newest from the call, or the last one.
pub(crate) fn picture(core: &Core, feed: &str, window: &mut Window, cx: &mut App) -> Option<Arc<RenderImage>> {
    let fresh = core.videos().take(feed).and_then(|p| {
        let buffer = image::RgbaImage::from_raw(p.width, p.height, p.bgra)?;
        Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
    });
    let cache = cx.default_global::<VideoCache>();
    let old = match fresh {
        Some(image) => cache.shown.insert(feed.to_owned(), Shown { image, used: Instant::now() }),
        None => None,
    };
    let current = cache.shown.get_mut(feed).map(|s| {
        s.used = Instant::now();
        s.image.clone()
    });
    if let Some(old) = old {
        cx.drop_image(old.image, Some(window));
    }
    current
}

/// Notes that this window draws `feed` `height` device pixels tall, if any
/// of it is in sight (an element that measures itself as it's painted).
fn measure(feed: String) -> impl IntoElement {
    gpui_kit::canvas(
        move |bounds, window, cx| {
            let seen = window.content_mask().bounds.intersect(&bounds);
            let visible = seen.size.width > px(0.0) && seen.size.height > px(0.0);
            let height = if visible { (f32::from(bounds.size.height) * window.scale_factor()) as u32 } else { 0 };
            if height == 0 {
                return;
            }
            let id = window.window_handle().window_id().as_u64();
            let cache = cx.default_global::<VideoCache>();
            let at = cache.drawing.entry(id).or_default().entry(feed.clone()).or_default();
            *at = (*at).max(height);
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// Goes last in a window: hands what it drew this frame to the call, and
/// lets go of pictures nothing drew for a while.
pub(crate) fn commit(core: Arc<Core>) -> impl IntoElement {
    gpui_kit::canvas(
        move |_, window, cx| {
            let id = window.window_handle().window_id().as_u64();
            let cache = cx.default_global::<VideoCache>();
            let drawn = cache.drawing.remove(&id).unwrap_or_default();
            let stale: Vec<String> =
                cache.shown.iter().filter(|(_, s)| s.used.elapsed() > UNUSED).map(|(f, _)| f.clone()).collect();
            let gone: Vec<Arc<RenderImage>> =
                stale.iter().filter_map(|f| cache.shown.remove(f)).map(|s| s.image).collect();
            for image in gone {
                cx.drop_image(image, Some(window));
            }
            core.videos().want(id, drawn);
        },
        |_, _, _, _| {},
    )
    .absolute()
    .size_0()
}

/// A window closed: it shows nothing any more.
pub(crate) fn forget_window(core: &Core, id: u64, cx: &mut App) {
    cx.default_global::<VideoCache>().drawing.remove(&id);
    core.videos().want(id, HashMap::new());
}

/// Lets go of every picture (the call ended).
pub(crate) fn clear(window: &mut Window, cx: &mut App) {
    let shown = std::mem::take(&mut cx.default_global::<VideoCache>().shown);
    for (_, s) in shown {
        cx.drop_image(s.image, Some(window));
    }
}

/// A feed playing over whatever is under it (the person's avatar): once its
/// first picture is in, the picture `fit` to the box on black, faded in.
/// It asks for its size from the start, or no picture would come.
pub(crate) fn feed_view(core: &Core, feed: &str, fit: ObjectFit, window: &mut Window, cx: &mut App) -> AnyElement {
    let image = picture(core, feed, window, cx);
    let shown = image.map(|image| {
        let el =
            div().absolute().inset_0().bg(gpui_kit::black()).child(gpui_kit::img(image).size_full().object_fit(fit));
        motion::once(el, SharedString::from(format!("video-in|{feed}")), Duration::from_millis(350), |el, t| {
            el.opacity(t)
        })
    });
    div().absolute().inset_0().children(shown).child(measure(feed.to_owned())).into_any_element()
}

/// The little "LIVE" mark on a shared screen.
pub(crate) fn live_badge() -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(4.0))
        .rounded(radius_md())
        .bg(red())
        .px(px(6.0))
        .text_size(px(10.0))
        .line_height(px(14.0))
        .font_weight(FontWeight::EXTRA_BOLD)
        .text_color(gpui_kit::white())
        .child(div().size(px(6.0)).rounded_full().bg(gpui_kit::white()))
        .child(t("dms-calls.calls.video.live"))
}

/// The pop-out button on a tile, shown while the tile is hovered (`group`).
pub(crate) fn pop_out_button(id: impl Into<SharedString>, label: String, group: &str, p: &Palette) -> Stateful<Div> {
    let hover = p.background;
    div()
        .id(id.into())
        .absolute()
        .top(px(8.0))
        .right(px(8.0))
        .size(px(32.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius_xl())
        .bg(alpha(p.background, 0.75))
        .text_color(p.foreground)
        .cursor_pointer()
        .opacity(0.0)
        .group_hover(SharedString::from(group.to_owned()), |s| s.opacity(1.0))
        .hover(move |s| s.bg(hover))
        .active(|s| s.opacity(0.8))
        .tooltip(move |window, cx| crate::ui::overlay::Tip::new(label.clone()).build(window, cx))
        .child(icon("picture-in-picture-2").size(px(16.0)))
}
