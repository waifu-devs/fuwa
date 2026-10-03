//! The emoji picker over the composer: the server's own first, then the
//! everyday ones, with a search box. They ripple in, one after another.

use std::time::Duration;

use gpui_kit::component::input::Input;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::ui::app::{FuwaApp, Target};
use crate::ui::chat::emoji_glyph;
use crate::ui::emoji::{self, Choice};
use crate::ui::motion;
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::{card, icon, icon_button};

impl FuwaApp {
    /// The smiley in the composer that opens the picker.
    pub(crate) fn emoji_button(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let open = self.emoji_open;
        icon_button("emoji-open", "face-slightly-smiling", p)
            .size(px(36.0))
            .when(open, |el| el.bg(alpha(p.primary, 0.12)).text_color(p.primary))
            .on_click(cx.listener(|this, _, window, cx| {
                if this.emoji_open {
                    this.close_emoji(window, cx);
                } else {
                    this.emoji_open = true;
                    this.picker = None;
                    this.emoji_query.update(cx, |state, cx| {
                        state.set_value("", window, cx);
                        state.focus(window, cx);
                    });
                    cx.notify();
                }
            }))
            .into_any_element()
    }

    pub(crate) fn close_emoji(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.emoji_open = false;
        self.composer.update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }

    /// The picker itself, floating over the composer's right end.
    pub(crate) fn emoji_panel(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let query = self.emoji_query.read(cx).value().to_string();
        let own_emoji = match self.target() {
            Some(Target::Channel { key, server, .. }) => self
                .core
                .shared
                .read(|s| s.instance(&key).and_then(|i| i.emojis.get(&server)).cloned().unwrap_or_default()),
            _ => Vec::new(),
        };
        let (own, plain) = emoji::all(&query, &own_emoji);
        let mut grid = div().id("emoji-grid").h(px(280.0)).overflow_y_scroll().px(px(10.0)).pb(px(10.0));
        let mut n = 0usize;
        for (title, list) in [("THIS SERVER", own), ("EVERYDAY", plain)] {
            if list.is_empty() {
                continue;
            }
            grid = grid.child(
                div()
                    .px(px(4.0))
                    .pt(px(10.0))
                    .pb(px(4.0))
                    .text_size(px(11.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_color(p.muted_foreground)
                    .child(title),
            );
            let mut row = div().flex().flex_wrap();
            for choice in list {
                row = row.child(self.emoji_cell(choice, n, p, cx));
                n += 1;
            }
            grid = grid.child(row);
        }
        if n == 0 {
            grid = grid.child(
                div()
                    .pt(px(48.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.0))
                    .text_color(p.muted_foreground)
                    .child(icon("search-x").size(px(28.0)))
                    .child(div().text_sm().child("No emoji by that name")),
            );
        }
        let body = card(p)
            .w(px(340.0))
            .rounded(corner(18.0))
            .overflow_hidden()
            .child(
                div()
                    .p(px(10.0))
                    .border_b_1()
                    .border_color(p.border)
                    .child(Input::new(&self.emoji_query).prefix(icon("search").size(px(16.0)))),
            )
            .child(grid);
        div()
            .id("emoji-panel")
            .absolute()
            .right(px(20.0))
            .bottom(gpui_kit::relative(1.0))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_emoji(window, cx)))
            .child(motion::rise(body.mb(px(-8.0)), "emoji-panel-rise", Duration::ZERO, 12.0))
            .into_any_element()
    }

    fn emoji_cell(&self, choice: Choice, n: usize, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let hover = alpha(p.primary, 0.12);
        let id = SharedString::from(format!("emoji|{n}|{}", choice.name));
        let insert = choice.insert.clone();
        div()
            .id(id)
            .size(px(40.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(corner(10.0))
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .active(|s| s.top(px(1.0)))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.emoji_open = false;
                this.insert_text(&format!("{insert} "), window, cx);
            }))
            .child(motion::rise(
                div().child(emoji_glyph(&choice, 26.0)),
                SharedString::from(format!("emoji-in|{n}")),
                Duration::from_millis(8 * n.min(30) as u64),
                6.0,
            ))
            .into_any_element()
    }
}
