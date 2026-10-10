//! Menus under a button on the settings pages (shadcn's `DropdownMenu`, as
//! the web uses it): one open at a time, closed by picking or clicking
//! anywhere else, drawn above the page, which it hides from the mouse (so
//! scrolling the menu never scrolls the page too).

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, Div, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    Stateful, StatefulInteractiveElement as _, Styled as _, div, px,
};

use crate::ui::motion;
use crate::ui::settings::SettingsView;
use crate::ui::settings_controls::shadow_lg;
use crate::ui::theme::{Palette, alpha, radius_md};
use crate::ui::widgets::icon;

type V = SettingsView;

/// One row of a [`SettingsView::dropdown`].
pub(crate) enum Item {
    Label(String),
    Separator,
    Action { label: String, glyph: Option<&'static str>, danger: bool, run: crate::ui::settings_controls::Run },
}

impl Item {
    pub(crate) fn action(
        label: impl Into<String>,
        glyph: Option<&'static str>,
        run: impl Fn(&mut V, &mut Context<V>) + 'static,
    ) -> Self {
        Item::Action { label: label.into(), glyph, danger: false, run: Rc::new(run) }
    }

    pub(crate) fn danger(
        label: impl Into<String>,
        glyph: Option<&'static str>,
        run: impl Fn(&mut V, &mut Context<V>) + 'static,
    ) -> Self {
        Item::Action { label: label.into(), glyph, danger: true, run: Rc::new(run) }
    }
}

impl SettingsView {
    /// A trigger that opens a menu under it, aligned to its right edge with `right`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn dropdown(
        &self,
        id: String,
        trigger: Stateful<Div>,
        items: Vec<Item>,
        right: bool,
        width: f32,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.state.menu.as_deref() == Some(id.as_str());
        let toggle_id = id.clone();
        let trigger = trigger.on_click(cx.listener(move |this, _, _, cx| {
            this.state.menu =
                if this.state.menu.as_deref() == Some(toggle_id.as_str()) { None } else { Some(toggle_id.clone()) };
            cx.notify();
        }));
        let mut wrap = div().relative().flex_none().child(trigger);
        if open {
            let mut list = div()
                .id(SharedString::from(format!("{id}-menu")))
                .occlude()
                .absolute()
                .top(px(40.0))
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
                .shadow(shadow_lg())
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.state.menu = None;
                    cx.notify();
                }))
                .map(|el| if right { el.right_0() } else { el.left_0() });
            for (n, item) in items.into_iter().enumerate() {
                list = list.child(match item {
                    Item::Label(text) => div()
                        .px(px(8.0))
                        .py(px(6.0))
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(p.muted_foreground)
                        .child(text)
                        .into_any_element(),
                    Item::Separator => div().my(px(4.0)).mx(px(-4.0)).h(px(1.0)).bg(p.border).into_any_element(),
                    Item::Action { label, glyph, danger, run } => {
                        let hover = if danger { alpha(p.destructive, 0.1) } else { p.accent.into() };
                        let tint = if danger { p.destructive } else { p.muted_foreground };
                        div()
                            .id(SharedString::from(format!("{id}-item-{n}")))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .px(px(8.0))
                            .py(px(6.0))
                            .rounded(px((f32::from(radius_md()) - 2.0).max(0.0)))
                            .text_sm()
                            .text_color(if danger { p.destructive } else { p.foreground })
                            .cursor_pointer()
                            .hover(move |s| s.bg(hover))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state.menu = None;
                                run(this, cx);
                                cx.notify();
                            }))
                            .when_some(glyph, |el, g| el.child(icon(g).size(px(16.0)).text_color(tint)))
                            .child(label)
                            .into_any_element()
                    }
                });
            }
            wrap = wrap.child(
                gpui_kit::deferred(motion::rise(
                    list,
                    SharedString::from(format!("{id}-menu-in")),
                    Duration::ZERO,
                    -4.0,
                ))
                .with_priority(1),
            );
        }
        wrap.into_any_element()
    }
}
