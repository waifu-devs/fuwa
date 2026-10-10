//! Menus under a button on the server settings pages (the web's animated
//! shadcn `DropdownMenu`): one open at a time, closed by picking or by
//! clicking anywhere else, drawn above the page.

use std::rc::Rc;

use gpui_kit::{Div, Stateful};

use super::*;
use crate::ui::settings_controls::shadow_lg;
use crate::ui::theme::radius_md;

type Run = Rc<dyn Fn(&mut ServerSettingsView, &mut Window, &mut Context<ServerSettingsView>)>;

/// One row of a [`ServerSettingsView::dropdown`].
pub(super) enum Item {
    Label(String),
    Separator,
    Action {
        label: String,
        /// What leads the row (an icon, a role's dot, an avatar).
        lead: Option<AnyElement>,
        /// A check at the row's end (the web's picked role).
        check: bool,
        /// A radio item: the dot at its start, `Some(true)` when it's the one picked.
        radio: Option<bool>,
        danger: bool,
        /// Picking keeps the menu open (roles toggled from a list).
        stay: bool,
        run: Run,
    },
}

impl Item {
    pub(super) fn action(
        label: impl Into<String>,
        lead: Option<AnyElement>,
        run: impl Fn(&mut ServerSettingsView, &mut Window, &mut Context<ServerSettingsView>) + 'static,
    ) -> Self {
        Item::Action {
            label: label.into(),
            lead,
            check: false,
            radio: None,
            danger: false,
            stay: false,
            run: Rc::new(run),
        }
    }

    pub(super) fn danger(mut self) -> Self {
        if let Item::Action { danger, .. } = &mut self {
            *danger = true;
        }
        self
    }

    pub(super) fn checked(mut self, on: bool) -> Self {
        if let Item::Action { check, .. } = &mut self {
            *check = on;
        }
        self
    }

    pub(super) fn radio(mut self, on: bool) -> Self {
        if let Item::Action { radio, .. } = &mut self {
            *radio = Some(on);
        }
        self
    }

    pub(super) fn stay(mut self) -> Self {
        if let Item::Action { stay, .. } = &mut self {
            *stay = true;
        }
        self
    }
}

/// An icon as a menu row leads with it (`size-4`, muted unless told otherwise).
pub(super) fn lead_icon(name: &str, p: &Palette) -> Option<AnyElement> {
    Some(icon(name).size(px(16.0)).text_color(p.muted_foreground).into_any_element())
}

/// A role's colored dot (`RoleDot`: `size-2.5 rounded-full`, the role's color or muted).
pub(super) fn role_dot(color: Option<u32>, p: &Palette) -> Div {
    let c: Hsla = color.filter(|c| *c != 0).map(|c| gpui_kit::rgb(c).into()).unwrap_or(p.muted_foreground.into());
    div().size(px(10.0)).flex_none().rounded_full().bg(c)
}

impl ServerSettingsView {
    pub(super) fn menu_open(&self, id: &str) -> bool {
        self.pages.menu.as_deref() == Some(id)
    }

    /// A trigger that opens a menu `below` px under its top, aligned to its right edge with `right`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn dropdown(
        &self,
        id: String,
        trigger: Stateful<Div>,
        items: Vec<Item>,
        right: bool,
        width: f32,
        below: f32,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.menu_open(&id);
        let toggle_id = id.clone();
        let trigger = trigger.on_click(cx.listener(move |this, _, _, cx| {
            this.pages.menu =
                if this.pages.menu.as_deref() == Some(toggle_id.as_str()) { None } else { Some(toggle_id.clone()) };
            cx.notify();
        }));
        let mut wrap = div().relative().flex_none().child(trigger);
        if open {
            let mut list = div()
                .id(SharedString::from(format!("{id}-menu")))
                .occlude()
                .w(px(width))
                .max_h(px(288.0))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .p(px(4.0))
                .rounded(radius_md())
                .border_1()
                .border_color(p.border)
                .bg(p.card)
                .text_color(p.foreground)
                .shadow(shadow_lg())
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.pages.menu = None;
                    cx.notify();
                }));
            for (n, item) in items.into_iter().enumerate() {
                list = list.child(match item {
                    Item::Label(text) => div()
                        .px(px(8.0))
                        .py(px(6.0))
                        .text_xs()
                        .line_height(px(16.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(p.muted_foreground)
                        .child(text)
                        .into_any_element(),
                    Item::Separator => div().my(px(4.0)).mx(px(-4.0)).h(px(1.0)).bg(p.border).into_any_element(),
                    Item::Action { label, lead, check, radio, danger, stay, run } => {
                        let hover = if danger { alpha(p.destructive, 0.1) } else { p.accent.into() };
                        div()
                            .id(SharedString::from(format!("{id}-item-{n}")))
                            .relative()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .pl(px(if radio.is_some() { 32.0 } else { 8.0 }))
                            .pr(px(8.0))
                            .py(px(6.0))
                            .rounded(px((f32::from(radius_md()) - 2.0).max(0.0)))
                            .text_sm()
                            .line_height(px(20.0))
                            .text_color(if danger { p.destructive } else { p.foreground })
                            .cursor_pointer()
                            .hover(move |s| s.bg(hover))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if !stay {
                                    this.pages.menu = None;
                                }
                                run(this, window, cx);
                                cx.notify();
                            }))
                            .when(radio == Some(true), |el| {
                                el.child(
                                    div()
                                        .absolute()
                                        .left(px(8.0))
                                        .top_0()
                                        .bottom_0()
                                        .w(px(14.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(div().size(px(8.0)).rounded_full().bg(p.foreground)),
                                )
                            })
                            .when_some(lead, |el, l| {
                                el.child(if danger { div().text_color(p.destructive).child(l) } else { div().child(l) })
                            })
                            .child(div().flex_1().min_w_0().truncate().child(label))
                            .when(check, |el| {
                                el.child(motion::once(
                                    div().text_color(p.primary).child(icon("check").size(px(16.0))),
                                    SharedString::from(format!("{id}-check-{n}")),
                                    Duration::from_millis(260),
                                    |el, t| el.opacity(t),
                                ))
                            })
                            .into_any_element()
                    }
                });
            }
            // A point under the trigger the menu hangs from, drawn over everything and kept in the
            // window; laid out apart, so opening it never moves the row it's in.
            let corner = if right { gpui_kit::Anchor::TopRight } else { gpui_kit::Anchor::TopLeft };
            wrap = wrap.child(
                div().absolute().top(px(below)).map(|el| if right { el.right_0() } else { el.left_0() }).child(
                    gpui_kit::deferred(
                        gpui_kit::anchored()
                            .anchor(corner)
                            .snap_to_window_with_margin(gpui_kit::Edges::all(px(8.0)))
                            .child(motion::rise(
                                list,
                                SharedString::from(format!("{id}-menu-in")),
                                Duration::ZERO,
                                -4.0,
                            )),
                    )
                    .with_priority(1),
                ),
            );
        }
        wrap.into_any_element()
    }
}
