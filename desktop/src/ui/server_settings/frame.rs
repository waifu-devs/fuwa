//! The screen around a server's settings, the web's `SettingsScreen` as
//! `ServerSettingsDialog.tsx` uses it: the side menu (the server's name, a
//! search that finds pages and single settings, the server's pages, the
//! people ones, then the dangerous ones apart), the page under its heading,
//! the close button, and unsaved edits that hold the page (the save bar
//! shakes instead).

use std::cell::Cell;
use std::time::{Duration, Instant};

use gpui_kit::component::input::Input;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, point, px,
};

use super::{Found, Page, ServerSettingsEvent, ServerSettingsView, search};
use crate::core::i18n::{Arg, t, t_with};
use crate::ui::motion;
use crate::ui::theme::{Palette, alpha, radius_lg, radius_md};
use crate::ui::widgets::icon;

/// How long the screen takes to come and go.
pub(super) const OPENING: Duration = Duration::from_millis(280);

thread_local! {
    /// The save bar's alarm while someone just tried to leave, read by `save_bar` as pages draw.
    pub(super) static ALARM: Cell<Option<u32>> = const { Cell::new(None) };
}

impl ServerSettingsView {
    /// Opens a page (and, from search, one setting on it), unless unsaved edits hold the page.
    pub(super) fn choose(&mut self, page: Page, setting: Option<&'static str>, cx: &mut Context<Self>) {
        if Some(page) != self.page && self.held {
            self.hold_on(cx);
            return;
        }
        if Some(page) != self.page {
            self.scroll.set_offset(point(px(0.0), px(0.0)));
            self.places.borrow_mut().clear();
            self.open(page, cx);
        }
        if let Some(id) = setting {
            // Settings that are a tab open it; those that share a block light the block up.
            match id {
                "role-permissions" => self.roles_tab(super::roles::RoleTab::Permissions),
                "role-members" => self.roles_tab(super::roles::RoleTab::Members),
                "channel-permissions" => self.channels_tab_permissions(),
                _ => {}
            }
            let id = match id {
                "banner-focus" | "accent-color" => "banner-picture",
                "welcome-description" | "welcome-channels" => "welcome-enabled",
                "onboarding-steps" => "onboarding-enabled",
                other => other,
            };
            self.scroll_since = Some(Instant::now());
            let n = self.glow.map_or(0, |(_, n)| n + 1);
            self.glow = Some((id, n));
            self.scroll_to = Some(id);
        }
        cx.notify();
    }

    /// Someone tried to leave with unsaved edits: the save bar shakes and turns red.
    pub(super) fn hold_on(&mut self, cx: &mut Context<Self>) {
        self.nudge = (self.nudge.0 + 1, Some(Instant::now()));
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1850)).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    pub(super) fn alarm(&self) -> Option<u32> {
        match self.nudge {
            (n, Some(at)) if at.elapsed() < Duration::from_millis(1800) => Some(n),
            _ => None,
        }
    }

    /// Escape: clears a search first, then closes (unless unsaved edits hold the screen).
    pub fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pages.people.nickname.take().is_some() || self.pages.menu.take().is_some() {
            cx.notify();
            return;
        }
        if !self.query.read(cx).value().is_empty() {
            self.query.update(cx, |q, cx| q.set_value("", window, cx));
            cx.notify();
            return;
        }
        self.close(cx);
    }

    /// Closes with the screen fading and growing away, unless unsaved edits hold it.
    pub(super) fn close(&mut self, cx: &mut Context<Self>) {
        if self.held {
            self.hold_on(cx);
            return;
        }
        if self.closing.is_some() {
            return;
        }
        if cx.reduce_motion() {
            cx.emit(ServerSettingsEvent::Close);
            return;
        }
        self.closing = Some(Instant::now());
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(OPENING).await;
            let _ = this.update(cx, |_, cx| cx.emit(ServerSettingsEvent::Close));
        })
        .detach();
    }

    fn pick_first(&mut self, cx: &mut Context<Self>, shown: &[Page]) {
        let query = self.query.read(cx).value().to_string();
        let first = search(shown, &query)
            .and_then(|results| results.first().map(|(page, settings)| (*page, settings.first().map(|s| s.0))));
        if let Some((page, setting)) = first {
            self.choose(page, setting, cx);
        }
    }

    /// The side menu: the server's name, the search, then the groups (or what the search found).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn menu(
        &mut self,
        title: &str,
        shown: &[Page],
        page: Page,
        badges: &dyn Fn(Page) -> usize,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let query = self.query.read(cx).value().to_string();
        let focused = gpui_kit::Focusable::focus_handle(self.query.read(cx), cx).is_focused(window);
        let search_box = div()
            .relative()
            .h(px(36.0))
            .rounded(radius_lg())
            .border_1()
            .border_color(if focused { alpha(p.primary, 0.5) } else { alpha(p.border, 0.0) })
            .bg(if focused { p.background.into() } else { alpha(p.muted, 0.7) })
            .when(focused, |el| {
                el.shadow(vec![gpui_kit::BoxShadow {
                    color: alpha(p.primary, 0.1),
                    offset: point(px(0.0), px(0.0)),
                    blur_radius: px(0.0),
                    spread_radius: px(4.0),
                    inset: false,
                }])
            })
            .flex()
            .items_center()
            .pl(px(32.0))
            .pr(px(32.0))
            .text_sm()
            .child(
                div()
                    .absolute()
                    .left(px(9.0))
                    .top(px(9.0))
                    .text_color(if focused { p.primary } else { p.muted_foreground })
                    .child(icon("search").size(px(16.0))),
            )
            .child(div().flex_1().min_w_0().child(Input::new(&self.query).appearance(false)))
            .when(!query.is_empty(), |el| {
                let (hover, fg) = (p.muted, p.foreground);
                el.child(
                    div()
                        .id("server-search-clear")
                        .absolute()
                        .right(px(5.0))
                        .top(px(5.0))
                        .size(px(24.0))
                        .rounded(radius_md())
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(p.muted_foreground)
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover).text_color(fg))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.query.update(cx, |q, cx| q.set_value("", window, cx));
                            cx.notify();
                        }))
                        .child(icon("x").size(px(14.0))),
                )
            });
        let shown_for_enter = shown.to_vec();
        let search_box = div()
            .on_key_down(cx.listener(move |this, e: &gpui_kit::KeyDownEvent, _, cx| {
                if e.keystroke.key == "enter" {
                    this.pick_first(cx, &shown_for_enter);
                }
            }))
            .child(search_box);

        let mut nav = div()
            .w(px(240.0))
            .flex_none()
            .pt(px(64.0))
            .pb(px(64.0))
            .pr(px(12.0))
            .pl(px(20.0))
            .flex()
            .flex_col()
            .gap(px(20.0))
            .child(
                div()
                    .px(px(8.0))
                    .child(
                        div()
                            .text_lg()
                            .line_height(px(28.0))
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .truncate()
                            .child(title.to_owned()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .line_height(px(16.0))
                            .text_color(p.muted_foreground)
                            .truncate()
                            .child(t("serversettings.nav.subtitle")),
                    ),
            )
            .child(search_box);

        if let Some(results) = search(shown, &query) {
            nav = nav.child(self.results(&results, &query, p, cx));
            return nav.into_any_element();
        }

        let groups: Vec<(Option<String>, Vec<Page>)> =
            [Some(title.to_owned()), Some(t("serversettings.nav.people")), None]
                .into_iter()
                .enumerate()
                .map(|(g, label)| (label, shown.iter().copied().filter(|pg| pg.group() == g).collect::<Vec<_>>()))
                .filter(|(_, list)| !list.is_empty())
                .collect();

        // The highlight glides between rows: where the chosen one sits in the list.
        let mut start = 0.0;
        let mut at = None;
        let mut list = div().relative().flex().flex_col().gap(px(20.0));
        let mut n = 0usize;
        for (g, (label, sections)) in groups.iter().enumerate() {
            let mut block = div().flex().flex_col().gap(px(2.0));
            let mut y = start;
            if g > 0 {
                block = block.border_t_1().border_color(alpha(p.border, 0.7)).pt(px(12.0));
                y += 13.0;
            }
            if let Some(label) = label {
                block = block.child(
                    div()
                        .mb(px(4.0))
                        .px(px(8.0))
                        .text_size(px(11.2))
                        .line_height(px(16.8))
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.muted_foreground)
                        .truncate()
                        .child(label.to_uppercase()),
                );
                y += 16.8 + 4.0 + 2.0;
            }
            for (i, &section) in sections.iter().enumerate() {
                if i > 0 {
                    y += 2.0;
                }
                let active = section == page;
                let danger = section.danger();
                if active {
                    at = Some((y, danger));
                }
                let trailing = label.is_none();
                let (hover_bg, hover_fg) = if danger {
                    (alpha(p.destructive, 0.1), p.destructive)
                } else {
                    (alpha(p.muted, 0.7), p.foreground)
                };
                let color: gpui_kit::Hsla = match (active, danger) {
                    (true, true) => p.destructive.into(),
                    (true, false) => p.primary.into(),
                    (false, true) => alpha(p.destructive, 0.8),
                    (false, false) => p.muted_foreground.into(),
                };
                let badge = badges(section);
                let row = div()
                    .id(SharedString::from(format!("smenu-{section:?}")))
                    .relative()
                    .h(px(32.0))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .rounded(radius_lg())
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(color)
                    .cursor_pointer()
                    .when(!active, |el| el.hover(move |s| s.bg(hover_bg).text_color(hover_fg)))
                    .active(|s| s.top(px(1.0)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        window.blur(cx);
                        this.choose(section, None, cx)
                    }))
                    .child(div().flex_1().min_w_0().truncate().child(section.label()))
                    .when(badge > 0, |el| el.child(count_badge(badge, p)))
                    .when(trailing, |el| el.child(page_glyph(section, 16.0, color)));
                block = block.child(slide(row, SharedString::from(format!("smenu-in-{section:?}")), n));
                n += 1;
                y += 32.0;
            }
            start = y + 20.0;
            list = list.child(block);
        }
        if let Some((top, danger)) = at {
            let top = motion::follow("server-settings-hl", top, window, cx);
            list = div()
                .relative()
                .child(
                    div().absolute().left_0().right_0().top(px(top)).h(px(32.0)).rounded(radius_lg()).bg(if danger {
                        alpha(p.destructive, 0.12)
                    } else {
                        alpha(p.primary, 0.15)
                    }),
                )
                .child(list);
        }
        nav.child(list).into_any_element()
    }

    /// What a search found: pages, and under each the single settings that matched.
    fn results(&mut self, results: &[Found], query: &str, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        if results.is_empty() {
            return motion::rise(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(8.0))
                    .py(px(24.0))
                    .text_sm()
                    .text_color(p.muted_foreground)
                    .child(motion::once(
                        div().child(icon("search-x").size(px(28.0))),
                        SharedString::from(format!("server-nomatch-{query}")),
                        Duration::from_millis(700),
                        |el, t| {
                            let k = if t < 1.0 / 7.0 { 0.0 } else { (t - 1.0 / 7.0) * 7.0 / 6.0 };
                            let wiggle = (k * std::f32::consts::TAU * 2.0).sin() * (1.0 - k) * 2.0;
                            el.relative().left(px(wiggle))
                        },
                    ))
                    .child(t_with("settings.screen.noMatches", &[("query", Arg::Str(query))])),
                "server-settings-nomatch",
                Duration::ZERO,
                8.0,
            )
            .into_any_element();
        }
        let mut list = div().flex().flex_col().gap(px(2.0));
        let mut n = 0;
        for (page, settings) in results {
            let page = *page;
            let hover = alpha(p.muted, 0.7);
            list = list.child(slide(
                div()
                    .id(SharedString::from(format!("sresult-{page:?}")))
                    .h(px(32.0))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .rounded(radius_lg())
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(if page.danger() { alpha(p.destructive, 0.8) } else { p.foreground.into() })
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        window.blur(cx);
                        this.choose(page, None, cx)
                    }))
                    .child(page_glyph(
                        page,
                        16.0,
                        if page.danger() { alpha(p.destructive, 0.8) } else { p.foreground.into() },
                    ))
                    .child(div().truncate().child(page.label())),
                SharedString::from(format!("sresult-in-{page:?}")),
                n,
            ));
            n += 1;
            for (id, label, _) in settings.iter() {
                let id: &'static str = id;
                let fg = p.foreground;
                list = list.child(slide(
                    div()
                        .id(SharedString::from(format!("sresult-{page:?}-{id}")))
                        .h(px(30.0))
                        .pl(px(24.0))
                        .pr(px(10.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .rounded(radius_lg())
                        .text_size(px(12.8))
                        .text_color(p.muted_foreground)
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover).text_color(fg))
                        .on_click(cx.listener(move |this, _, _, cx| this.choose(page, Some(id), cx)))
                        .child(div().opacity(0.6).child(icon("corner-down-right").size(px(14.0))))
                        .child(div().truncate().child(label.clone())),
                    SharedString::from(format!("sresult-in-{page:?}-{id}")),
                    n,
                ));
                n += 1;
            }
        }
        list.into_any_element()
    }
}

/// A page's icon in `color` (shared channels have their own linked rings).
pub(super) fn page_glyph(page: Page, size: f32, color: impl Into<gpui_kit::Hsla>) -> AnyElement {
    let color = color.into();
    if page == Page::Shared {
        return crate::ui::shared_marks::glyph(size, color).into_any_element();
    }
    icon(page.glyph()).size(px(size)).text_color(color).into_any_element()
}

/// Something waiting on a page, as a red count (the web's menu badge).
fn count_badge(n: usize, p: &Palette) -> impl IntoElement {
    let _ = p;
    motion::once(
        div()
            .relative()
            .h(px(20.0))
            .min_w(px(20.0))
            .px(px(6.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(p.destructive)
            .text_size(px(10.4))
            .font_weight(FontWeight::EXTRA_BOLD)
            .text_color(gpui_kit::white())
            .child(if n > 99 { "99+".to_owned() } else { n.to_string() }),
        SharedString::from(format!("smenu-badge-{n}")),
        Duration::from_millis(260),
        |el, t| el.opacity(t),
    )
}

impl ServerSettingsView {
    /// Marks part of a page as a search target: where it's drawn is kept for scrolling there, and
    /// it glows when it's the one picked (the web's `data-setting` and `.found`).
    pub(super) fn mark<E: IntoElement + gpui_kit::ParentElement + gpui_kit::Styled + 'static>(
        &self,
        id: &'static str,
        el: E,
        p: &Palette,
    ) -> AnyElement {
        use gpui_kit::base::ElementExt as _;
        let places = self.places.clone();
        let el = el.on_prepaint(move |bounds, _, _| {
            places.borrow_mut().insert(id, bounds);
        });
        match &self.glow {
            Some((glowing, n)) if *glowing == id => {
                let color = p.primary;
                motion::once(
                    el,
                    SharedString::from(format!("sfound-{id}-{n}")),
                    Duration::from_millis(1800),
                    move |el, t| {
                        // 0..15% in, held to 60%, out by 100%.
                        let k = if t < 0.15 {
                            t / 0.15
                        } else if t < 0.6 {
                            1.0
                        } else {
                            1.0 - (t - 0.6) / 0.4
                        };
                        el.bg(alpha(color, 0.12 * k)).rounded(px(12.0))
                    },
                )
            }
            _ => el.into_any_element(),
        }
    }
}

/// A menu row sliding in from the left, a beat after the one above it.
fn slide(el: impl IntoElement + gpui_kit::Styled + 'static, id: SharedString, n: usize) -> AnyElement {
    use gpui_kit::{Animation, AnimationExt as _};
    let delay = 0.05 + n as f32 * 0.03;
    let total = Duration::from_secs_f32(delay + 0.4);
    let start = delay / total.as_secs_f32();
    el.with_animation(id, Animation::new(total), move |el, t| {
        let k = if t <= start { 0.0 } else { ((t - start) / (1.0 - start)).clamp(0.0, 1.0) };
        let e = 1.0 - (1.0 - k).powi(3);
        el.opacity(e).left(px(-10.0 * (1.0 - e)))
    })
    .into_any_element()
}
