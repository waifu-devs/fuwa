//! A board's settings, as the web app's `board/BoardSettingsDialog.tsx`: its
//! columns (name, color, order, whether cards in it are done, a card limit)
//! and its labels (name and color), for people who can manage the channel.
//! Opened from the gear in the board's header; it starts from the board as
//! it is each time, and Save sets both lists whole (`SetBoard`). A column
//! still holding cards can't be taken off, nor the last one.

use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, Focusable as _, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px, rgb,
};

use crate::core::board::{CARD_LIMIT_MAX, COLUMN_NAME_MAX, COLUMNS_MAX, LABEL_NAME_MAX, LABELS_MAX, PALETTE};
use crate::core::i18n::{Arg, t, t_with};
use crate::pb;
use crate::ui::app::{Dialog, FuwaApp};
use crate::ui::motion;
use crate::ui::overlay::{dialog_button, dialog_card, dialog_close, dialog_header, dialog_layer};
use crate::ui::server_settings::roles::switch;
use crate::ui::settings_controls::Look;
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_xl};
use crate::ui::widgets::{error_line, icon, pal};

/// The settings dialog while it's open.
pub struct BoardSettings {
    key: String,
    server: String,
    channel: String,
    columns: Vec<ColumnRow>,
    labels: Vec<LabelRow>,
    /// The next row's key, so rows keep their place as others move or go.
    next: u64,
    /// The row whose colors are showing, by its key.
    picking: Option<u64>,
    busy: bool,
    error: Option<String>,
    _subs: Vec<Subscription>,
}

/// A column being edited: new ones have no id yet.
struct ColumnRow {
    key: u64,
    id: String,
    name: Entity<InputState>,
    color: i32,
    done: bool,
    /// The card limit as typed, empty for none.
    limit: Entity<InputState>,
}

struct LabelRow {
    key: u64,
    id: String,
    name: Entity<InputState>,
    color: i32,
}

/// Keeps a field to `max` characters.
fn clip(input: &Entity<InputState>, max: usize, window: &mut Window, cx: &mut Context<FuwaApp>) {
    let value = input.read(cx).value().to_string();
    if value.chars().count() > max {
        let cut: String = value.chars().take(max).collect();
        input.update(cx, |s, cx| s.set_value(cut, window, cx));
    }
}

/// A card limit as typed: digits only, at most [`CARD_LIMIT_MAX`], 0 for none.
pub(crate) fn card_limit(typed: &str) -> i32 {
    let digits: String = typed.chars().filter(char::is_ascii_digit).take(6).collect();
    digits.parse::<i32>().unwrap_or(0).clamp(0, CARD_LIMIT_MAX)
}

/// Keeps a card limit field to digits, at most [`CARD_LIMIT_MAX`].
fn clip_limit(input: &Entity<InputState>, window: &mut Window, cx: &mut Context<FuwaApp>) {
    let value = input.read(cx).value().to_string();
    let n = card_limit(&value);
    let shown = if n == 0 { String::new() } else { n.to_string() };
    if shown != value {
        input.update(cx, |s, cx| s.set_value(shown, window, cx));
    }
}

/// `0xRRGGBB` as a colour; None for 0.
fn color_of(c: i32) -> Option<gpui_kit::Rgba> {
    (c != 0).then(|| rgb(c as u32 & 0xff_ffff))
}

impl FuwaApp {
    /// Opens a board's settings, starting from the board as it is.
    pub(crate) fn open_board_settings(
        &mut self,
        key: &str,
        server: &str,
        channel: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((columns, labels)) = self.core.shared.read(|s| {
            let b = s.instance(key)?.boards.get(channel)?;
            b.loaded.then(|| (b.columns.clone(), b.labels.clone()))
        }) else {
            return;
        };
        let mut subs = Vec::new();
        let mut next = 0;
        let columns = columns
            .into_iter()
            .map(|c| {
                next += 1;
                self.column_row(next, c, &mut subs, window, cx)
            })
            .collect();
        let labels = labels
            .into_iter()
            .map(|l| {
                next += 1;
                self.label_row(next, l, &mut subs, window, cx)
            })
            .collect();
        let dialog = Dialog::BoardSettings { key: key.into(), server: server.into(), channel: channel.into() };
        self.open_dialog(dialog, window, cx);
        self.board.settings = Some(BoardSettings {
            key: key.into(),
            server: server.into(),
            channel: channel.into(),
            columns,
            labels,
            next,
            picking: None,
            busy: false,
            error: None,
            _subs: subs,
        });
        cx.notify();
    }

    fn column_row(
        &mut self,
        key: u64,
        c: pb::BoardColumn,
        subs: &mut Vec<Subscription>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ColumnRow {
        let name = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t("chattools.board.columnName")).default_value(c.name.clone())
        });
        let shown = if c.card_limit > 0 { c.card_limit.to_string() } else { String::new() };
        let limit =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("chattools.board.noLimit")).default_value(shown));
        subs.push(cx.subscribe_in(&name, window, |_: &mut Self, input, event: &InputEvent, window, cx| {
            if let InputEvent::Change = event {
                clip(input, COLUMN_NAME_MAX, window, cx);
                cx.notify();
            }
        }));
        subs.push(cx.subscribe_in(&limit, window, |_: &mut Self, input, event: &InputEvent, window, cx| {
            if let InputEvent::Change = event {
                clip_limit(input, window, cx);
                cx.notify();
            }
        }));
        ColumnRow { key, id: c.id, name, color: c.color, done: c.done, limit }
    }

    fn label_row(
        &mut self,
        key: u64,
        l: pb::BoardLabel,
        subs: &mut Vec<Subscription>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> LabelRow {
        let name = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t("chattools.board.labelName")).default_value(l.name.clone())
        });
        subs.push(cx.subscribe_in(&name, window, |_: &mut Self, input, event: &InputEvent, window, cx| {
            if let InputEvent::Change = event {
                clip(input, LABEL_NAME_MAX, window, cx);
                cx.notify();
            }
        }));
        LabelRow { key, id: l.id, name, color: l.color }
    }

    fn add_board_column(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut s) = self.board.settings.take() else { return };
        if s.columns.len() < COLUMNS_MAX {
            s.next += 1;
            let color = PALETTE[s.columns.len() % PALETTE.len()];
            let mut subs = std::mem::take(&mut s._subs);
            let row = self.column_row(s.next, pb::BoardColumn { color, ..Default::default() }, &mut subs, window, cx);
            s._subs = subs;
            row.name.update(cx, |i, cx| i.focus(window, cx));
            s.columns.push(row);
        }
        self.board.settings = Some(s);
        cx.notify();
    }

    fn add_board_label(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut s) = self.board.settings.take() else { return };
        if s.labels.len() < LABELS_MAX {
            s.next += 1;
            let color = PALETTE[(s.labels.len() * 5) % PALETTE.len()];
            let mut subs = std::mem::take(&mut s._subs);
            let row = self.label_row(s.next, pb::BoardLabel { color, ..Default::default() }, &mut subs, window, cx);
            s._subs = subs;
            row.name.update(cx, |i, cx| i.focus(window, cx));
            s.labels.push(row);
        }
        self.board.settings = Some(s);
        cx.notify();
    }

    /// The columns and labels as typed, when every one has a name.
    fn board_draft(&self, cx: &gpui_kit::App) -> Option<(Vec<pb::BoardColumn>, Vec<pb::BoardLabel>)> {
        let s = self.board.settings.as_ref()?;
        let columns: Vec<_> = s
            .columns
            .iter()
            .map(|c| pb::BoardColumn {
                id: c.id.clone(),
                name: c.name.read(cx).value().trim().to_owned(),
                color: c.color,
                done: c.done,
                card_limit: card_limit(&c.limit.read(cx).value()),
            })
            .collect();
        let labels: Vec<_> = s
            .labels
            .iter()
            .map(|l| pb::BoardLabel {
                id: l.id.clone(),
                name: l.name.read(cx).value().trim().to_owned(),
                color: l.color,
            })
            .collect();
        let valid = !columns.is_empty()
            && columns.iter().all(|c| !c.name.is_empty())
            && labels.iter().all(|l| !l.name.is_empty());
        valid.then_some((columns, labels))
    }

    /// Save: sets the board's columns and labels whole, and closes once they're in.
    pub(crate) fn save_board_settings(&mut self, cx: &mut Context<Self>) {
        let Some((columns, labels)) = self.board_draft(cx) else { return };
        let Some(s) = self.board.settings.as_mut().filter(|s| !s.busy) else { return };
        s.busy = true;
        s.error = None;
        let (core, key, server, channel) = (self.core.clone(), s.key.clone(), s.server.clone(), s.channel.clone());
        self.run(
            cx,
            async move { core.set_board(&key, &server, &channel, columns, labels).await },
            |this, result, cx| {
                match result {
                    Ok(()) => {
                        this.board.settings = None;
                        if matches!(this.dialog, Some(Dialog::BoardSettings { .. })) {
                            this.dialog = None;
                        }
                    }
                    Err(err) => {
                        if let Some(s) = this.board.settings.as_mut() {
                            s.busy = false;
                            s.error = Some(err.message);
                        }
                    }
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    /// The dialog: columns, then labels, then Cancel and Save.
    pub(crate) fn render_board_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let ready = self.board_draft(cx).is_some();
        let Some(s) = self.board.settings.as_ref() else { return div().into_any_element() };
        // Cards still in each column (archived ones aside), which keep it from going.
        let counts: std::collections::HashMap<String, usize> = self.core.shared.read(|st| {
            st.instance(&s.key)
                .and_then(|i| i.boards.get(&s.channel))
                .map(|b| b.columns.iter().map(|c| (c.id.clone(), b.cards_in(&c.id).count())).collect())
                .unwrap_or_default()
        });
        let focused = |input: &Entity<InputState>, window: &Window, cx: &gpui_kit::App| {
            input.read(cx).focus_handle(cx).is_focused(window)
        };
        let heading = |text: String| {
            div()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(p.muted_foreground)
                .child(crate::ui::text::tracked(text.to_uppercase(), crate::ui::text::WIDE))
        };

        let count = s.columns.len();
        let mut columns = div().flex().flex_col().gap(px(8.0)).child(heading(t("chattools.board.columns")));
        for (n, c) in s.columns.iter().enumerate() {
            let key = c.key;
            let held = if c.id.is_empty() { 0 } else { counts.get(&c.id).copied().unwrap_or(0) };
            let remove_tip = if held > 0 {
                t_with("chattools.board.columnHasCards", &[("count", Arg::Num(held as i64))])
            } else {
                t("chattools.board.removeColumn")
            };
            let top = div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(swatch_button(format!("col-color|{key}"), c.color, s.picking == Some(key), &p).on_click(
                    cx.listener(move |this, _, _, cx| {
                        if let Some(s) = this.board.settings.as_mut() {
                            s.picking = if s.picking == Some(key) { None } else { Some(key) };
                        }
                        cx.notify();
                    }),
                ))
                .child(div().flex_1().min_w_0().child(field(&c.name, 36.0, focused(&c.name, window, cx), &p)))
                .child(
                    row_button(format!("col-up|{key}"), "arrow-up", t("chattools.board.moveUp"), n == 0, false, &p)
                        .when(n > 0, |el| {
                            el.on_click(cx.listener(move |this, _, _, cx| this.move_board_column(key, -1, cx)))
                        }),
                )
                .child(
                    row_button(
                        format!("col-down|{key}"),
                        "arrow-down",
                        t("chattools.board.moveDown"),
                        n + 1 == count,
                        false,
                        &p,
                    )
                    .when(n + 1 < count, |el| {
                        el.on_click(cx.listener(move |this, _, _, cx| this.move_board_column(key, 1, cx)))
                    }),
                )
                .child({
                    let off = held > 0 || count == 1;
                    row_button(format!("col-remove|{key}"), "trash", remove_tip, off, true, &p).when(!off, |el| {
                        el.on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(s) = this.board.settings.as_mut() {
                                s.columns.retain(|c| c.key != key);
                            }
                            cx.notify();
                        }))
                    })
                });
            let options = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_x(px(20.0))
                .gap_y(px(8.0))
                .pl(px(4.0))
                .text_sm()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(switch(
                            SharedString::from(format!("col-done|{key}")),
                            c.done,
                            false,
                            cx,
                            move |this: &mut Self, on, cx| {
                                if let Some(c) = this
                                    .board
                                    .settings
                                    .as_mut()
                                    .and_then(|s| s.columns.iter_mut().find(|c| c.key == key))
                                {
                                    c.done = on;
                                }
                                cx.notify();
                            },
                        ))
                        .child(t("chattools.board.doneColumn")),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(t("chattools.board.cardLimit"))
                        .child(div().w(px(96.0)).child(field(&c.limit, 32.0, focused(&c.limit, window, cx), &p))),
                );
            let mut card = div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .p(px(12.0))
                .rounded(radius_2xl())
                .border_1()
                .border_color(p.border)
                .bg(alpha(p.muted, 0.3))
                .child(top);
            if s.picking == Some(key) {
                card = card.child(self.swatches(key, c.color, &p, cx));
            }
            card = card.child(options);
            columns =
                columns.child(motion::rise(card, SharedString::from(format!("col-row|{key}")), Duration::ZERO, 8.0));
        }
        if count < COLUMNS_MAX {
            columns = columns.child(
                add_button("board-add-column", t("chattools.board.addColumn"), &p)
                    .on_click(cx.listener(|this, _, window, cx| this.add_board_column(window, cx))),
            );
        }

        let mut labels = div().flex().flex_col().gap(px(8.0)).child(heading(t("chattools.board.labels")));
        for l in &s.labels {
            let key = l.key;
            let name = l.name.read(cx).value().to_string();
            let preview = pb::BoardLabel {
                name: if name.trim().is_empty() { "…".into() } else { name },
                color: l.color,
                ..Default::default()
            };
            let mut row = div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(swatch_button(format!("lab-color|{key}"), l.color, s.picking == Some(key), &p).on_click(
                    cx.listener(move |this, _, _, cx| {
                        if let Some(s) = this.board.settings.as_mut() {
                            s.picking = if s.picking == Some(key) { None } else { Some(key) };
                        }
                        cx.notify();
                    }),
                ))
                .child(div().flex_1().min_w_0().child(field(&l.name, 36.0, focused(&l.name, window, cx), &p)))
                .child(
                    div()
                        .w(px(112.0))
                        .flex_none()
                        .flex()
                        .justify_center()
                        .child(crate::ui::board::label_chip(&preview, &p)),
                )
                .child(
                    row_button(format!("lab-remove|{key}"), "trash", t("chattools.board.removeLabel"), false, true, &p)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(s) = this.board.settings.as_mut() {
                                s.labels.retain(|l| l.key != key);
                            }
                            cx.notify();
                        })),
                )
                .into_any_element();
            if s.picking == Some(key) {
                row = div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(row)
                    .child(self.swatches(key, l.color, &p, cx))
                    .into_any_element();
            }
            labels = labels.child(motion::rise(
                div().child(row),
                SharedString::from(format!("lab-row|{key}")),
                Duration::ZERO,
                8.0,
            ));
        }
        if s.labels.is_empty() {
            labels =
                labels.child(div().text_sm().text_color(p.muted_foreground).child(t("chattools.board.noBoardLabels")));
        }
        if s.labels.len() < LABELS_MAX {
            labels = labels.child(
                add_button("board-add-label", t("chattools.board.addLabel"), &p)
                    .on_click(cx.listener(|this, _, window, cx| this.add_board_label(window, cx))),
            );
        }

        let busy = s.busy;
        let panel = dialog_card(true, &p)
            .child(dialog_header(
                t("chattools.board.settings"),
                Some(t("chattools.board.settingsAbout").into_any_element()),
                &p,
            ))
            .child(div().flex().flex_col().gap(px(20.0)).child(columns).child(labels))
            .when_some(error_line(s.error.as_deref(), &p), |el, e| el.child(div().mt(px(12.0)).child(e)))
            .child(
                div()
                    .mt(px(20.0))
                    .flex()
                    .justify_end()
                    .gap(px(8.0))
                    .child(
                        dialog_button("board-settings-cancel", t("chattools.board.cancel"), Look::Ghost, &p)
                            .on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))),
                    )
                    .child(
                        dialog_button("board-settings-save", t("chattools.board.save"), Look::Primary, &p)
                            .when(!ready || busy, |el| el.opacity(0.5))
                            .when(ready && !busy, |el| {
                                el.on_click(cx.listener(|this, _, _, cx| this.save_board_settings(cx)))
                            }),
                    ),
            )
            .child(
                dialog_close("board-settings-close", &p).on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))),
            );
        dialog_layer("board-settings", panel, &p, cx.listener(|this, _, _, cx| this.close_dialog(cx)), cx)
    }

    fn move_board_column(&mut self, key: u64, by: isize, cx: &mut Context<Self>) {
        if let Some(s) = self.board.settings.as_mut()
            && let Some(at) = s.columns.iter().position(|c| c.key == key)
        {
            let to = at as isize + by;
            if (0..s.columns.len() as isize).contains(&to) {
                s.columns.swap(at, to as usize);
            }
        }
        cx.notify();
    }

    /// The palette under a row, and none: a click picks one and closes it.
    fn swatches(&self, key: u64, value: i32, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let mut grid = div().flex().flex_wrap().gap(px(6.0)).p(px(8.0)).rounded(radius_2xl()).border_1();
        grid = grid.border_color(p.border).bg(p.card);
        for color in std::iter::once(0).chain(PALETTE) {
            let on = color == value;
            let tip = if color == 0 { t("chattools.board.noColor") } else { format!("#{color:06x}") };
            grid = grid.child(
                div()
                    .id(SharedString::from(format!("swatch|{key}|{color}")))
                    .size(px(24.0))
                    .flex_none()
                    .rounded_full()
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.85))
                    .map(|el| match color_of(color) {
                        Some(c) => el.bg(c),
                        None => el.border_1().border_dashed().border_color(p.muted_foreground),
                    })
                    .when(on, |el| el.border_2().border_color(p.primary))
                    .tooltip(move |window, cx| crate::ui::overlay::Tip::new(tip.clone()).build(window, cx))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(s) = this.board.settings.as_mut() {
                            if let Some(c) = s.columns.iter_mut().find(|c| c.key == key) {
                                c.color = color;
                            } else if let Some(l) = s.labels.iter_mut().find(|l| l.key == key) {
                                l.color = color;
                            }
                            s.picking = None;
                        }
                        cx.notify();
                    })),
            );
        }
        div().flex().child(grid).into_any_element()
    }
}

/// The color button at a row's start: its color in a dot, or grey for none.
fn swatch_button(id: String, color: i32, open: bool, p: &Palette) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(SharedString::from(id))
        .size(px(36.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius_xl())
        .border_1()
        .border_color(if open { alpha(p.primary, 0.6) } else { p.border.into() })
        .bg(p.card)
        .cursor_pointer()
        .tooltip(|window, cx| crate::ui::overlay::Tip::new(t("chattools.board.color")).build(window, cx))
        .child(div().size(px(16.0)).rounded_full().bg(color_of(color).unwrap_or(p.muted_foreground)))
}

/// A round icon button beside a row; red on hover for `danger`, faint and still when `off`.
fn row_button(
    id: String,
    glyph: &'static str,
    tip: String,
    off: bool,
    danger: bool,
    p: &Palette,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let (bg, fg) = (p.muted, if danger { p.destructive } else { p.foreground });
    div()
        .id(SharedString::from(id))
        .size(px(32.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .text_color(p.muted_foreground)
        .when(off, |el| el.opacity(0.3))
        .when(!off, |el| el.cursor_pointer().hover(move |s| s.bg(bg).text_color(fg)))
        .tooltip(move |window, cx| crate::ui::overlay::Tip::new(tip.clone()).build(window, cx))
        .child(icon(glyph).size(px(16.0)))
}

/// "Add a column" and "Add a label": bold primary text with a plus.
fn add_button(id: &'static str, label: String, p: &Palette) -> gpui_kit::Stateful<gpui_kit::Div> {
    let hover = alpha(p.primary, 0.1);
    div()
        .id(id)
        .flex()
        .flex_none()
        .self_start()
        .items_center()
        .gap(px(6.0))
        .px(px(8.0))
        .py(px(6.0))
        .rounded(radius_xl())
        .text_sm()
        .font_weight(FontWeight::BOLD)
        .text_color(p.primary)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .active(|s| s.opacity(0.85))
        .child(icon("plus").size(px(16.0)))
        .child(label)
}

/// A text field drawn like the web's `Input`: rounded, ringed while focused.
fn field(input: &Entity<InputState>, height: f32, focused: bool, p: &Palette) -> gpui_kit::Div {
    div()
        .h(px(height))
        .flex()
        .items_center()
        .px(px(2.0))
        .rounded(radius_xl())
        .border_1()
        .border_color(if focused { alpha(p.primary, 0.6) } else { p.border.into() })
        .bg(p.card)
        .text_sm()
        .when(focused, |el| {
            el.shadow(vec![gpui_kit::BoxShadow {
                color: alpha(p.primary, 0.25),
                offset: gpui_kit::point(px(0.0), px(0.0)),
                blur_radius: px(0.0),
                spread_radius: px(3.0),
                inset: false,
            }])
        })
        .child(div().flex_1().min_w_0().child(Input::new(input).appearance(false)))
}

#[cfg(test)]
mod tests {
    use super::card_limit;

    #[test]
    fn card_limits_as_typed() {
        assert_eq!(card_limit(""), 0);
        assert_eq!(card_limit("12"), 12);
        assert_eq!(card_limit("1a2"), 12);
        assert_eq!(card_limit("99999"), 10_000);
        assert_eq!(card_limit("-3"), 3);
    }
}
