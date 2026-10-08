//! The Keyboard page: every shortcut, changed by pressing the new keys,
//! taken away or put back, and extra ones for any action (the web's
//! Keybinds page; the same settings, written the same way).

use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, Keystroke,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, div, px,
};

use crate::core::config::Prefs;
use crate::core::i18n::{Arg, t, t_with};
use crate::core::keybinds::{self, ACTIONS, CustomKeybind, Except, Group};
use crate::ui::keys::{combo_of, keycaps};
use crate::ui::motion;
use crate::ui::settings::SettingsView;
use crate::ui::settings_controls::{Look, button};
use crate::ui::text::{WIDE, tracked};
use crate::ui::theme::radius_xl;
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::icon;

/// Which shortcut is listening for keys.
#[derive(Clone, PartialEq, Eq)]
pub enum Recording {
    Action(&'static str),
    Custom(String),
}

/// What the page keeps between frames.
#[derive(Default)]
pub struct Keys {
    pub recording: Option<Recording>,
    /// A custom keybind being added, before it has keys.
    draft: Option<CustomKeybind>,
    /// Why the last keys couldn't be used, for which row, and when (the row shakes).
    problem: Option<(String, String, Instant)>,
    /// The custom keybind whose action list is open.
    picking: Option<String>,
}

fn row_key(r: &Recording) -> String {
    match r {
        Recording::Action(id) => format!("a:{id}"),
        Recording::Custom(id) => format!("c:{id}"),
    }
}

impl SettingsView {
    pub(crate) fn recording(&self) -> bool {
        self.keys.recording.is_some()
    }

    pub(crate) fn show_keyboard(&mut self, cx: &mut Context<Self>) {
        self.page = crate::ui::settings::Page::Keybinds;
        cx.notify();
    }

    fn start_recording(&mut self, r: Recording, cx: &mut Context<Self>) {
        // Leaving a new custom keybind without keys drops it.
        if let Some(d) = &self.keys.draft
            && r != Recording::Custom(d.id.clone())
        {
            self.keys.draft = None;
        }
        self.keys.recording = Some(r);
        self.keys.problem = None;
        self.keys.picking = None;
        cx.notify();
    }

    fn stop_recording(&mut self, cx: &mut Context<Self>) {
        if let Some(Recording::Custom(id)) = &self.keys.recording
            && self.keys.draft.as_ref().is_some_and(|d| &d.id == id)
        {
            self.keys.draft = None;
        }
        self.keys.recording = None;
        cx.notify();
    }

    /// A key pressed while a shortcut is listening: Escape stops, a combo is tried.
    pub(crate) fn record(&mut self, key: &Keystroke, cx: &mut Context<Self>) {
        let Some(recording) = self.keys.recording.clone() else { return };
        let m = &key.modifiers;
        if key.key == "escape" && !(m.shift || m.control || m.alt || m.platform) {
            self.stop_recording(cx);
            return;
        }
        let Some(combo) = combo_of(key) else { return };
        let combo = keybinds::normalize(&combo);
        let prefs = self.core.prefs();
        let except = match &recording {
            Recording::Action(id) => Except { action: Some(id), custom: None },
            Recording::Custom(id) => Except { action: None, custom: Some(id) },
        };
        if let Some(why) = keybinds::problem_with(&combo, &prefs.keybinds, &prefs.custom_keybinds, except) {
            self.keys.problem = Some((row_key(&recording), why, Instant::now()));
            cx.notify();
            return;
        }
        self.keys.problem = None;
        match recording {
            Recording::Action(id) => {
                let default = keybinds::action_by_id(id).and_then(|a| a.combo);
                self.set(cx, |pr| {
                    // Pressing the default again goes back to following the default.
                    if default == Some(combo.as_str()) {
                        pr.keybinds.remove(id);
                    } else {
                        pr.keybinds.insert(id.to_owned(), Some(combo));
                    }
                });
            }
            Recording::Custom(id) => {
                if let Some(mut draft) = self.keys.draft.take().filter(|d| d.id == id) {
                    draft.id = new_custom_id();
                    draft.combo = combo;
                    self.set(cx, |pr| pr.custom_keybinds.push(draft));
                } else {
                    self.set(cx, |pr| {
                        if let Some(c) = pr.custom_keybinds.iter_mut().find(|c| c.id == id) {
                            c.combo = combo;
                        }
                    });
                }
            }
        }
        self.keys.recording = None;
        cx.notify();
    }

    pub(crate) fn keyboard_page(
        &mut self,
        prefs: &Prefs,
        p: &Palette,
        _window: &mut gpui_kit::Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let note = div()
            .flex()
            .gap(px(8.0))
            .px(px(12.0))
            .py(px(10.0))
            .rounded(radius_xl())
            .bg(alpha(p.muted, 0.6))
            .text_sm()
            .text_color(p.muted_foreground)
            .child(icon("info").size(px(16.0)).mt(px(2.0)))
            .child(div().flex_1().min_w_0().child(t("desktop.keys.note")));

        // Extra keybinds.
        let mut custom_rows = div().flex().flex_col().gap(px(8.0));
        let rows: Vec<CustomKeybind> = prefs.custom_keybinds.iter().cloned().chain(self.keys.draft.clone()).collect();
        if rows.is_empty() {
            custom_rows = custom_rows.child(
                div()
                    .px(px(12.0))
                    .py(px(12.0))
                    .rounded(radius_xl())
                    .border_1()
                    .border_dashed()
                    .border_color(p.border)
                    .text_sm()
                    .text_color(p.muted_foreground)
                    .child(t("appsettings.keybinds.noneYet")),
            );
        }
        for bind in rows {
            custom_rows = custom_rows.child(self.custom_row(bind, p, cx));
        }
        let adding = self.keys.draft.is_some();
        let custom = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(
                        div()
                            .flex_1()
                            .child(div().font_weight(FontWeight::EXTRA_BOLD).child(t("appsettings.keybinds.custom")))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(p.muted_foreground)
                                    .child(t("appsettings.keybinds.customHint")),
                            ),
                    )
                    .child(
                        button("add-keybind", t("appsettings.keybinds.add"), Some("plus"), Look::Primary, true, p)
                            .rounded(radius_xl())
                            .font_weight(FontWeight::BOLD)
                            .when(adding, |el| el.opacity(0.5))
                            .when(!adding, |el| {
                                el.on_click(cx.listener(|this, _, _, cx| {
                                    let draft = CustomKeybind {
                                        id: "draft".into(),
                                        action: ACTIONS[0].id.into(),
                                        combo: String::new(),
                                    };
                                    this.keys.draft = Some(draft);
                                    this.start_recording(Recording::Custom("draft".into()), cx);
                                }))
                            }),
                    ),
            )
            .child(custom_rows);
        let custom = self.found_mark("custom-keybinds", custom, p);

        let mut page = div().flex().flex_col().gap(px(32.0)).child(note).child(custom);
        let mut n = 0usize;
        for group in Group::ALL {
            let mut section = div().flex().flex_col().child(
                div()
                    .mb(px(4.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.muted_foreground)
                    .child(tracked(group.name().to_uppercase(), WIDE)),
            );
            for action in ACTIONS.iter().filter(|a| a.group == group) {
                section = section.child(self.action_row(action, n, prefs, p, cx));
                n += 1;
            }
            page = page.child(section);
        }
        if !prefs.keybinds.is_empty() || !prefs.custom_keybinds.is_empty() {
            page = page.child(motion::rise(
                div().flex().child(
                    button(
                        "reset-keys",
                        t("appsettings.keybinds.resetAll"),
                        Some("rotate-ccw"),
                        Look::Outline,
                        false,
                        p,
                    )
                    .rounded(radius_xl())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.keys = Keys::default();
                        this.set(cx, |pr| {
                            pr.keybinds.clear();
                            pr.custom_keybinds.clear();
                        });
                    })),
                ),
                "reset-keys-rise",
                Duration::ZERO,
                8.0,
            ));
        }
        page.into_any_element()
    }

    fn action_row(
        &mut self,
        action: &'static keybinds::Action,
        n: usize,
        prefs: &Prefs,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let combo = keybinds::binding_of(action, &prefs.keybinds);
        let changed = prefs.keybinds.contains_key(action.id);
        let me = Recording::Action(action.id);
        let recording = self.keys.recording.as_ref() == Some(&me);
        let id = action.id;
        let mut controls = div().flex().items_center().gap(px(4.0)).child(recorder(
            SharedString::from(format!("rec-{id}")),
            combo.as_deref(),
            recording,
            p,
            cx.listener(move |this, _, _, cx| {
                if this.keys.recording == Some(Recording::Action(id)) {
                    this.stop_recording(cx);
                } else {
                    this.start_recording(Recording::Action(id), cx);
                }
            }),
        ));
        if changed {
            let back = match action.combo {
                Some(combo) => t_with("appsettings.keybinds.backTo", &[("keys", Arg::Str(&keybinds::label(combo)))]),
                None => t("appsettings.keybinds.backToNone"),
            };
            controls = controls.child(small_button(
                SharedString::from(format!("reset-{id}")),
                "rotate-ccw",
                &back,
                p,
                cx.listener(move |this, _, _, cx| this.set(cx, |pr| _ = pr.keybinds.remove(id))),
            ));
        }
        if combo.is_some() {
            controls = controls.child(small_button(
                SharedString::from(format!("clear-{id}")),
                "x",
                &t("appsettings.keybinds.removeShortcut"),
                p,
                cx.listener(move |this, _, _, cx| this.set(cx, |pr| _ = pr.keybinds.insert(id.to_owned(), None))),
            ));
        }
        let row = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .min_h(px(40.0))
            .child(div().flex_1().min_w_0().text_sm().font_weight(FontWeight::BOLD).child(action.label))
            .child(controls);
        let body = div()
            .py(px(10.0))
            .border_b_1()
            .border_color(alpha(p.border, 0.7))
            .child(self.shaking(row_key(&me), row))
            .when_some(self.problem_for(&row_key(&me)), |el, why| el.child(problem(why, p)));
        let body = self.found_mark(setting_id(id), body, p);
        motion::rise(
            div().child(body),
            SharedString::from(format!("key-row-{id}")),
            Duration::from_millis(30 * n as u64),
            8.0,
        )
        .into_any_element()
    }

    fn custom_row(&mut self, bind: CustomKeybind, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let me = Recording::Custom(bind.id.clone());
        let recording = self.keys.recording.as_ref() == Some(&me);
        let label =
            keybinds::action_by_id(&bind.action).map_or(t("appsettings.keybinds.pickAction"), |a| a.label.to_owned());
        let picking = self.keys.picking.as_deref() == Some(bind.id.as_str());
        let id = bind.id.clone();
        let picker = div()
            .id(SharedString::from(format!("pick-{id}")))
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(6.0))
            .px(px(8.0))
            .py(px(6.0))
            .rounded(corner(8.0))
            .cursor_pointer()
            .text_sm()
            .font_weight(FontWeight::BOLD)
            .hover(|s| s.bg(alpha(p.muted_foreground, 0.1)))
            .on_click({
                let id = id.clone();
                cx.listener(move |this, _, _, cx| {
                    this.keys.picking =
                        if this.keys.picking.as_deref() == Some(id.as_str()) { None } else { Some(id.clone()) };
                    cx.notify();
                })
            })
            .child(div().truncate().child(label))
            .child(icon("chevron-down").size(px(16.0)).text_color(p.muted_foreground));
        let combo = (!bind.combo.is_empty()).then_some(bind.combo.as_str());
        let toggle = {
            let id = id.clone();
            cx.listener(move |this, _, _, cx| {
                if this.keys.recording == Some(Recording::Custom(id.clone())) {
                    this.stop_recording(cx);
                } else {
                    this.start_recording(Recording::Custom(id.clone()), cx);
                }
            })
        };
        let remove = {
            let id = id.clone();
            cx.listener(move |this, _, _, cx| {
                if this.keys.draft.as_ref().is_some_and(|d| d.id == id) {
                    this.keys.draft = None;
                    this.keys.recording = None;
                    cx.notify();
                } else {
                    this.set(cx, |pr| pr.custom_keybinds.retain(|c| c.id != id));
                }
            })
        };
        let row = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(picker)
            .child(recorder(SharedString::from(format!("rec-c-{id}")), combo, recording, p, toggle))
            .child(small_button(
                SharedString::from(format!("rm-{id}")),
                "trash",
                &t("system.picture.remove"),
                p,
                remove,
            ));
        let mut card = div()
            .px(px(12.0))
            .py(px(8.0))
            .rounded(radius_xl())
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.card, 0.6))
            .child(self.shaking(row_key(&me), row))
            .when_some(self.problem_for(&row_key(&me)), |el, why| el.child(problem(why, p)));
        if picking {
            let mut list = div().mt(px(6.0)).flex().flex_col().gap(px(2.0));
            for group in Group::ALL {
                list = list.child(
                    div()
                        .px(px(8.0))
                        .pt(px(6.0))
                        .text_size(px(11.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.muted_foreground)
                        .child(group.name().to_uppercase()),
                );
                for action in ACTIONS.iter().filter(|a| a.group == group) {
                    let on = action.id == bind.action;
                    let (id, pick) = (id.clone(), action.id);
                    list = list.child(
                        div()
                            .id(SharedString::from(format!("pick-{id}-{pick}")))
                            .px(px(8.0))
                            .py(px(6.0))
                            .rounded(corner(8.0))
                            .text_sm()
                            .cursor_pointer()
                            .when(on, |el| el.text_color(p.primary).font_weight(FontWeight::BOLD))
                            .hover(|s| s.bg(alpha(p.primary, 0.08)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.keys.picking = None;
                                if let Some(d) = this.keys.draft.as_mut().filter(|d| d.id == id) {
                                    d.action = pick.to_owned();
                                    cx.notify();
                                } else {
                                    this.set(cx, |pr| {
                                        if let Some(c) = pr.custom_keybinds.iter_mut().find(|c| c.id == id) {
                                            c.action = pick.to_owned();
                                        }
                                    });
                                }
                            }))
                            .child(action.label),
                    );
                }
            }
            card = card.child(motion::rise(list, SharedString::from(format!("pick-list-{id}")), Duration::ZERO, -6.0));
        }
        motion::rise(card, SharedString::from(format!("custom-{id}")), Duration::ZERO, -8.0).into_any_element()
    }

    fn problem_for(&self, key: &str) -> Option<String> {
        self.keys.problem.as_ref().filter(|(k, ..)| k == key).map(|(_, why, _)| why.clone())
    }

    /// A row that shakes once when keys it can't take were pressed.
    fn shaking(&self, key: String, row: gpui_kit::Div) -> AnyElement {
        match self.keys.problem.as_ref().filter(|(k, ..)| *k == key) {
            Some((_, _, at)) => row
                .with_animation(
                    SharedString::from(format!("shake-{key}-{at:?}")),
                    Animation::new(Duration::from_millis(400)),
                    |el, t| {
                        let x = (t * std::f32::consts::TAU * 2.5).sin() * 8.0 * (1.0 - t);
                        el.relative().left(px(x))
                    },
                )
                .into_any_element(),
            None => row.into_any_element(),
        }
    }
}

/// The search target of an action's row ("key-<action>"), kept for the app's life.
fn setting_id(action: &str) -> &'static str {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static IDS: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let mut ids = IDS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    ids.entry(action.to_owned()).or_insert_with(|| Box::leak(format!("key-{action}").into_boxed_str()))
}

/// Where the Keybinds page's own rows sit for settings search (the web's `keybindSettings`).
pub(crate) fn keybind_settings() -> Vec<(&'static str, String, &'static str)> {
    let mut list = vec![("custom-keybinds", t("appsettings.keybinds.custom"), "add shortcut")];
    list.extend(ACTIONS.iter().map(|a| (setting_id(a.id), a.label.to_owned(), "shortcut hotkey")));
    list
}

fn new_custom_id() -> String {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let (mut n, mut out) = (ms, String::new());
    while n > 0 {
        let d = (n % 36) as u32;
        out.insert(0, char::from_digit(d, 36).unwrap_or('0'));
        n /= 36;
    }
    format!("custom-{out}")
}

/// The button that shows a shortcut and listens for a new one when clicked.
fn recorder(
    id: SharedString,
    combo: Option<&str>,
    recording: bool,
    p: &Palette,
    on_click: impl Fn(&gpui_kit::ClickEvent, &mut gpui_kit::Window, &mut gpui_kit::App) + 'static,
) -> impl IntoElement {
    let inner: AnyElement = if recording {
        div()
            .font_weight(FontWeight::BOLD)
            .text_color(p.primary)
            .child(t("appsettings.keybinds.pressKeys"))
            .with_animation("press-keys", Animation::new(Duration::from_millis(1200)).repeat(), |el, t| {
                el.opacity(0.55 + 0.45 * (t * std::f32::consts::TAU).cos().abs())
            })
            .into_any_element()
    } else {
        match combo {
            Some(c) => keycaps(c, p).into_any_element(),
            None => div().text_color(p.muted_foreground).child(t("appsettings.keybinds.notSet")).into_any_element(),
        }
    };
    div()
        .id(id)
        .min_w(px(150.0))
        .h(px(40.0))
        .px(px(12.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius_xl())
        .border_1()
        .text_sm()
        .cursor_pointer()
        .map(|el| {
            if recording {
                el.border_color(p.primary).bg(alpha(p.primary, 0.1))
            } else {
                let hover = alpha(p.primary, 0.5);
                el.border_color(p.border).hover(move |s| s.border_color(hover))
            }
        })
        .on_click(on_click)
        .child(inner)
}

fn small_button(
    id: SharedString,
    glyph: &str,
    tip: &str,
    p: &Palette,
    on_click: impl Fn(&gpui_kit::ClickEvent, &mut gpui_kit::Window, &mut gpui_kit::App) + 'static,
) -> impl IntoElement {
    let tip = SharedString::from(tip.to_owned());
    div()
        .id(id)
        .size(px(32.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(corner(8.0))
        .cursor_pointer()
        .text_color(p.muted_foreground)
        .hover({
            let (bg, fg) = (alpha(p.muted_foreground, 0.12), p.foreground);
            move |s| s.bg(bg).text_color(fg)
        })
        .active(|s| s.top(px(1.0)))
        .tooltip(move |window, cx| crate::ui::overlay::Tip::new(tip.clone()).build(window, cx))
        .on_click(on_click)
        .child(icon(glyph).size(px(16.0)))
}

fn problem(why: String, p: &Palette) -> impl IntoElement {
    motion::rise(
        div().pt(px(6.0)).text_xs().font_weight(FontWeight::BOLD).text_color(p.destructive).child(why),
        "key-problem",
        Duration::ZERO,
        -4.0,
    )
}
