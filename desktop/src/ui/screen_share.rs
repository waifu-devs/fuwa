//! Starting a screen share (the web's `calls/ScreenShareDialog.tsx`): every
//! screen and window as a small picture of what's on it, then how sharp and
//! how smooth it goes out. The browser's own picker does the first part on
//! the web; here the app draws it. Pictures are taken one frame each, off the
//! main thread, as the dialog opens; a window that won't give one (minimized)
//! keeps its icon.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, Div, FontWeight, InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _,
    RenderImage, SharedString, StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window, div, px,
};

use crate::core::i18n::{Arg, t, t_with};
use crate::core::voice::capture::{self, Screen};
use crate::core::voice::vp8::Share;
use crate::ui::app::{Dialog, FuwaApp};
use crate::ui::motion;
use crate::ui::overlay::dialog_button;
use crate::ui::settings_controls::{Look, shadow_sm};
use crate::ui::theme::{Palette, alpha, radius_lg, radius_xl};
use crate::ui::widgets::icon;

/// How wide each picture is taken, in pixels (tiles draw it smaller, sharp on any screen).
const THUMBNAIL_WIDTH: u32 = 480;

/// What the share dialog holds while it's open.
#[derive(Default)]
pub(crate) struct SharePicker {
    /// What can be shared: screens first, then windows.
    pub screens: Vec<Screen>,
    /// Each one's picture, by id, as it comes in.
    pictures: HashMap<u32, Arc<RenderImage>>,
    /// The ones whose picture isn't coming (no frame in time).
    missing: std::collections::HashSet<u32>,
    /// The one picked to share.
    picked: Option<u32>,
    share: Option<Share>,
    /// Which opening the pictures coming in belong to.
    round: u64,
}

impl FuwaApp {
    /// Opens the share dialog: reads what can be shared, picks the first
    /// screen, and starts taking each one's picture.
    pub(crate) fn open_share_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.calls.pop = None;
        let old = std::mem::take(&mut self.calls.picker.pictures);
        for (_, image) in old {
            cx.drop_image(image, Some(window));
        }
        let picker = &mut self.calls.picker;
        picker.screens = capture::screens();
        picker.missing.clear();
        picker.picked = picker.screens.first().map(|s| s.id);
        picker.share = Some(Share::new(self.prefs.share_height, self.prefs.share_fps));
        picker.round += 1;
        let round = picker.round;
        for screen in picker.screens.clone() {
            let id = screen.id;
            self.run(
                cx,
                async move {
                    tokio::task::spawn_blocking(move || capture::thumbnail(id, THUMBNAIL_WIDTH)).await.ok().flatten()
                },
                move |this, picture, cx| {
                    let picker = &mut this.calls.picker;
                    if picker.round != round {
                        return;
                    }
                    match picture.and_then(|p| image::RgbaImage::from_raw(p.width, p.height, p.bgra)) {
                        Some(buffer) => {
                            let image = Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]));
                            picker.pictures.insert(id, image);
                        }
                        None => {
                            picker.missing.insert(id);
                        }
                    }
                    cx.notify();
                },
            );
        }
        self.open_dialog(Dialog::ShareScreen, window, cx);
    }

    /// Shares what's picked, as picked, and keeps the choice for next time.
    pub(crate) fn start_share(&mut self, cx: &mut Context<Self>) {
        let picker = &self.calls.picker;
        let Some(id) = picker.picked else { return };
        let share = picker.share.unwrap_or(Share::DEFAULT);
        self.core.set_prefs(|pr| (pr.share_height, pr.share_fps) = (share.height, share.fps));
        self.prefs.share_height = share.height;
        self.prefs.share_fps = share.fps;
        self.dialog = None;
        self.core.set_screen(true, Some(id));
        cx.notify();
    }

    pub(crate) fn share_panel(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let picker = &self.calls.picker;
        let share = picker.share.unwrap_or(Share::DEFAULT);
        let (displays, windows): (Vec<_>, Vec<_>) = picker.screens.iter().cloned().partition(|s| s.display);

        let mut list = div().id("share-list").max_h(px(340.0)).overflow_y_scroll().flex().flex_col().gap(px(16.0));
        if picker.screens.is_empty() {
            list = list.child(
                div()
                    .py(px(24.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.0))
                    .text_sm()
                    .text_color(p.muted_foreground)
                    .child(icon("monitor-x").size(px(28.0)))
                    .child(t("dms-calls.calls.share.nothing")),
            );
        }
        for (label, group, columns) in [
            (t("dms-calls.calls.share.screens"), displays, 2usize),
            (t("dms-calls.calls.share.windows"), windows, 3usize),
        ] {
            if group.is_empty() {
                continue;
            }
            let mut grid = div().flex().flex_wrap().gap(px(10.0));
            // The dialog's inside is 624 wide; tiles share it with 10 between.
            let tile_w = (624.0 - 10.0 * (columns as f32 - 1.0) - 8.0) / columns as f32;
            for (n, screen) in group.iter().enumerate() {
                grid = grid.child(self.share_tile(screen, tile_w, n, p, window, cx));
            }
            list = list.child(div().flex().flex_col().gap(px(8.0)).child(heading(label, p)).child(grid));
        }

        let heights = Share::HEIGHTS
            .iter()
            .map(|h| (format!("{h}p"), t(&format!("dms-calls.calls.share.height.{h}"))))
            .collect::<Vec<_>>();
        let rates = Share::FPS
            .iter()
            .map(|f| {
                (
                    t_with("dms-calls.calls.share.fps", &[("fps", Arg::Num(i64::from(*f)))]),
                    t(&format!("dms-calls.calls.share.fps.{f}")),
                )
            })
            .collect::<Vec<_>>();
        let height_at = Share::HEIGHTS.iter().position(|h| *h == share.height).unwrap_or(1);
        let fps_at = Share::FPS.iter().position(|f| *f == share.fps).unwrap_or(1);
        let quality = div()
            .mt(px(20.0))
            .flex()
            .gap(px(16.0))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(heading(t("dms-calls.calls.share.resolution"), p))
                    .child(picks("share-height", heights, height_at, p, window, cx, |this, n, cx| {
                        if let Some(share) = this.calls.picker.share.as_mut() {
                            share.height = Share::HEIGHTS[n];
                        }
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(heading(t("dms-calls.calls.share.frameRate"), p))
                    .child(picks("share-fps", rates, fps_at, p, window, cx, |this, n, cx| {
                        if let Some(share) = this.calls.picker.share.as_mut() {
                            share.fps = Share::FPS[n];
                        }
                        cx.notify();
                    })),
            );
        let mbps = format!("{:.1}", share.mbps());
        let upload = motion::rise(
            div()
                .text_xs()
                .line_height(px(16.0))
                .text_color(p.muted_foreground)
                .child(t_with("dms-calls.calls.share.upload", &[("mbps", Arg::Str(&mbps))])),
            SharedString::from(format!("share-upload-{mbps}")),
            Duration::ZERO,
            6.0,
        );

        let picked = picker.picked.is_some();
        let footer = div()
            .mt(px(20.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(div().flex_1().min_w_0().child(upload))
            .child(
                dialog_button("share-cancel", t("common.cancel"), Look::Ghost, p)
                    .on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))),
            )
            .child(
                dialog_button("share-start", t("dms-calls.calls.share.start"), Look::Primary, p)
                    .when(!picked, |el| el.opacity(0.5))
                    .when(picked, |el| el.on_click(cx.listener(|this, _, _, cx| this.start_share(cx)))),
            );

        crate::ui::overlay::dialog_card(true, p)
            .child(crate::ui::overlay::dialog_header(
                t("dms-calls.calls.share.title"),
                Some(t("dms-calls.calls.share.description").into_any_element()),
                p,
            ))
            .child(list)
            .child(quality)
            .child(footer)
    }

    /// One screen or window: its picture (or its icon while there's none),
    /// its name, and a ring and a check when it's the one picked.
    fn share_tile(
        &self,
        screen: &Screen,
        w: f32,
        n: usize,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let picker = &self.calls.picker;
        let on = picker.picked == Some(screen.id);
        let id = screen.id;
        let h = w * 9.0 / 16.0;
        let picture: AnyElement = match picker.pictures.get(&id) {
            Some(image) => motion::fade_in(
                div().size_full().child(gpui_kit::img(image.clone()).size_full().object_fit(ObjectFit::Contain)),
                SharedString::from(format!("share-picture-{id}")),
                Duration::from_millis(240),
            )
            .into_any_element(),
            None => {
                let glyph = icon(if screen.display { "monitor" } else { "app-window" })
                    .size(px(24.0))
                    .text_color(p.muted_foreground);
                let waiting = !picker.missing.contains(&id);
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(if waiting {
                        motion::ambient(
                            div().child(glyph),
                            SharedString::from(format!("share-wait-{id}")),
                            Duration::from_millis(1400),
                            window,
                            |el, t| el.opacity(0.35 + 0.45 * (1.0 - (t * 2.0 - 1.0).abs())),
                        )
                    } else {
                        div().child(glyph).into_any_element()
                    })
                    .into_any_element()
            }
        };
        let (ring, hover) = (p.primary, alpha(p.primary, 0.4));
        let tile = div()
            .id(SharedString::from(format!("share-tile-{}-{id}", screen.display)))
            .w(px(w))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .p(px(6.0))
            .rounded(radius_xl())
            .border_2()
            .border_color(if on { ring.into() } else { alpha(p.border, 0.0) })
            .when(on, |el| el.bg(alpha(p.primary, 0.08)))
            .when(!on, |el| el.hover(move |s| s.border_color(hover)))
            .cursor_pointer()
            .active(|s| s.top(px(1.0)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.calls.picker.picked = Some(id);
                cx.notify();
            }))
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(h))
                    .rounded(radius_lg())
                    .overflow_hidden()
                    .bg(p.muted)
                    .child(picture)
                    .when(on, |el| {
                        el.child(
                            div().absolute().top(px(6.0)).right(px(6.0)).child(motion::pop(
                                div()
                                    .size(px(22.0))
                                    .rounded_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(p.primary)
                                    .text_color(p.primary_foreground)
                                    .shadow(shadow_sm())
                                    .child(icon("check").size(px(14.0))),
                                SharedString::from(format!("share-check-{id}")),
                                0.4,
                                -30.0,
                                Duration::ZERO,
                            )),
                        )
                    }),
            )
            .child(
                div()
                    .px(px(2.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        icon(if screen.display { "monitor" } else { "app-window" })
                            .size(px(14.0))
                            .flex_none()
                            .text_color(if on { p.primary } else { p.muted_foreground }),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_sm()
                            .line_height(px(20.0))
                            .font_weight(if on { FontWeight::BOLD } else { FontWeight::MEDIUM })
                            .child(screen.name.clone()),
                    ),
            );
        motion::rise(tile, SharedString::from(format!("share-rise-{id}")), Duration::from_millis(20 * n as u64), 8.0)
            .into_any_element()
    }
}

/// A section's name, small and quiet (`text-xs font-bold uppercase`).
fn heading(text: String, p: &Palette) -> Div {
    div()
        .text_xs()
        .line_height(px(16.0))
        .font_weight(FontWeight::BOLD)
        .text_color(p.muted_foreground)
        .child(text.to_uppercase())
}

/// Choices side by side, each a value and what it's for, with a highlight
/// that glides to the picked one (the web's `Segmented` in the share dialog).
fn picks(
    id: &'static str,
    options: Vec<(String, String)>,
    chosen: usize,
    p: &Palette,
    window: &mut Window,
    cx: &mut Context<FuwaApp>,
    pick: impl Fn(&mut FuwaApp, usize, &mut Context<FuwaApp>) + 'static,
) -> AnyElement {
    let count = options.len().max(1) as f32;
    // Half the wide dialog's inside, less the gap between the two rows of choices.
    let item_w = (304.0 - 8.0) / count;
    let x = motion::follow(SharedString::from(format!("{id}-pill")), chosen as f32 * item_w, window, cx);
    let pick = std::rc::Rc::new(pick);
    let mut row = div().relative().flex().p(px(4.0)).rounded(radius_xl()).bg(p.muted).child(
        div()
            .absolute()
            .top(px(4.0))
            .bottom(px(4.0))
            .left(px(4.0 + x))
            .w(px(item_w))
            .rounded(radius_lg())
            .bg(p.card)
            .border_1()
            .border_color(alpha(p.primary, 0.3))
            .shadow(shadow_sm()),
    );
    for (n, (value, hint)) in options.into_iter().enumerate() {
        let on = n == chosen;
        let pick = pick.clone();
        let fg = p.foreground;
        row = row.child(
            div()
                .id(SharedString::from(format!("{id}-{n}")))
                .relative()
                .w(px(item_w))
                .px(px(4.0))
                .py(px(6.0))
                .flex()
                .flex_col()
                .items_center()
                .text_color(if on { p.foreground } else { p.muted_foreground })
                .cursor_pointer()
                .hover(move |s| s.text_color(fg))
                .active(|s| s.top(px(1.0)))
                .on_click(cx.listener(move |this, _, _, cx| pick(this, n, cx)))
                .child(div().text_sm().line_height(px(20.0)).font_weight(FontWeight::EXTRA_BOLD).child(value))
                .child(div().text_xs().line_height(px(14.0)).text_center().child(hint)),
        );
    }
    row.into_any_element()
}
