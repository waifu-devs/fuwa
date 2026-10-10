//! The game overlay: a see-through window over the whole screen, above the
//! game you're playing, that every click passes through (`click_through`).
//! In a corner it shows who's in your call, lit up while they talk, and the
//! messages that would notify you. Its key (`toggleOverlay`, Shift+` unless
//! changed) puts it in use: it takes clicks, to mute, deafen, hang up or open
//! a message in fuwa, until the key again, Esc, or a click outside its cards.
//!
//! It shows while a game is in front (a program covering its screen, from
//! `core::foreground`, or a game reporting what you play, `core::presence`),
//! or whenever you're in a call (`OverlayShow`), never while fuwa itself is
//! in front, and only where keys can be heard from a game (`core::hotkeys`):
//! not on Wayland. A game in exclusive full screen draws over every window,
//! so it shows over borderless and windowed games only.
//!
//! Like the main window it's a view of the core; `FuwaApp` opens and closes
//! it and hands it the keys `core::hotkeys` hears.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AnyWindowHandle, AppContext as _, Context, Entity, FocusHandle, FontWeight, InteractiveElement as _,
    IntoElement, KeyDownEvent, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Task, WeakEntity, Window, WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, div, px,
};

use crate::core::Core;
use crate::core::config::{InputMode, OverlayShow, Prefs};
use crate::core::hotkeys::{Combo, Hotkeys, Press, Reach};
use crate::core::i18n::{Arg, t, t_with};
use crate::core::keybinds;
use crate::core::voice::{CallView, Status};
use crate::pb;
use crate::ui::app::FuwaApp;
use crate::ui::call_parts::{green, red, voice_avatar};
use crate::ui::click_through;
use crate::ui::motion;
use crate::ui::notify::Clicked;
use crate::ui::theme::{FONT, alpha, radius_lg, radius_xl};
use crate::ui::widgets::{icon, pal};

/// How long a message stays in the overlay, before it fades.
const NOTE_STAYS: Duration = Duration::from_secs(6);
/// How long it takes to fade.
const NOTE_FADES: Duration = Duration::from_millis(400);
/// The most messages shown at once.
const NOTES: usize = 3;
/// How often it looks for a game in front, while it waits for one.
const LOOK_EVERY: Duration = Duration::from_millis(1500);
/// How often it puts itself back on top, where a game may have gone over it.
const RAISE_EVERY: Duration = Duration::from_secs(2);
/// How far from the screen's edges its cards sit.
const MARGIN: f32 = 24.0;
/// The call card's width.
const CARD_W: f32 = 248.0;

/// Whether the overlay can be offered here: keys heard while a game is in
/// front, and a window clicks pass through (not Wayland).
pub(crate) fn offered() -> bool {
    Hotkeys::reach() != Reach::No
}

/// Every combo an action has now: its own and the custom keybinds for it.
fn combos(action: &str, prefs: &Prefs) -> Vec<String> {
    let mut out: Vec<String> =
        keybinds::action_by_id(action).and_then(|a| keybinds::binding_of(a, &prefs.keybinds)).into_iter().collect();
    out.extend(prefs.custom_keybinds.iter().filter(|c| c.action == action).map(|c| c.combo.clone()));
    out
}

/// The overlay's key in words, for the hints ("Shift+`"); None without one.
pub(crate) fn key_label(prefs: &Prefs) -> Option<String> {
    combos("toggleOverlay", prefs).first().map(|c| keybinds::label(c))
}

/// What `FuwaApp` keeps about the overlay.
#[derive(Default)]
pub(crate) struct OverlayUi {
    /// The overlay's window, while it's open.
    open: Option<(AnyWindowHandle, Entity<GameOverlay>)>,
    /// Whether it's in use, shared with it, so this never reads it while it's busy.
    using: Rc<Cell<bool>>,
    /// The main window, to tell it apart from the overlay when one is in front.
    main: Option<AnyWindowHandle>,
    /// A program covers its screen (`core::foreground`), as of the last look.
    game_in_front: bool,
    /// Looking for one every so often, while the overlay waits for games.
    looking: Option<Task<()>>,
}

/// A message, as the overlay shows it.
struct Note {
    id: u64,
    icon: &'static str,
    title: String,
    body: String,
    target: Clicked,
    at: Instant,
}

pub(crate) struct GameOverlay {
    core: Arc<Core>,
    app: WeakEntity<FuwaApp>,
    /// In use: taking clicks and keys (shared with `OverlayUi`).
    using: Rc<Cell<bool>>,
    /// Whether clicks go through the window now, as last set (None before).
    through: Option<bool>,
    notes: Vec<Note>,
    next_note: u64,
    focus: FocusHandle,
}

impl GameOverlay {
    fn new(
        core: Arc<Core>,
        app: WeakEntity<FuwaApp>,
        using: Rc<Cell<bool>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Who's talking, who's in the call.
        let mut changes = core.changes();
        cx.spawn_in(window, async move |this, cx| {
            while changes.changed().await.is_ok() {
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(RAISE_EVERY).await;
                if this.update_in(cx, |_, window, _| click_through::keep_on_top(window)).is_err() {
                    break;
                }
            }
        })
        .detach();
        using.set(false);
        Self { core, app, using, through: None, notes: Vec::new(), next_note: 1, focus: cx.focus_handle() }
    }

    /// Puts the overlay in use (taking clicks and keys) or back out of the way.
    pub(crate) fn set_using(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.using.get() == on {
            return;
        }
        self.using.set(on);
        if on {
            window.activate_window();
            self.focus.focus(window, cx);
        } else {
            // Messages kept while it was in use get their time again.
            let now = Instant::now();
            for note in &mut self.notes {
                note.at = now;
            }
            if !self.notes.is_empty() {
                self.redraw_after(NOTE_STAYS, cx);
            }
            // Done: the main window decides whether it's still wanted.
            let app = self.app.clone();
            cx.defer(move |cx| {
                let _ = app.update(cx, |app, cx| app.sync_overlay(cx));
            });
        }
        cx.notify();
    }

    fn push(&mut self, icon: &'static str, title: String, body: String, target: Clicked, cx: &mut Context<Self>) {
        let id = self.next_note;
        self.next_note += 1;
        self.notes.push(Note { id, icon, title, body, target, at: Instant::now() });
        if self.notes.len() > NOTES {
            self.notes.remove(0);
        }
        // Again as it starts fading.
        self.redraw_after(NOTE_STAYS, cx);
        cx.notify();
    }

    fn redraw_after(&self, wait: Duration, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.using.get() {
            return;
        }
        let key = &ev.keystroke;
        let combo = crate::ui::keys::combo_of(key).map(|c| keybinds::normalize(&c));
        let own = combo
            .is_some_and(|c| combos("toggleOverlay", &self.core.prefs()).iter().any(|k| keybinds::normalize(k) == c));
        if key.key == "escape" || own {
            cx.stop_propagation();
            self.set_using(false, window, cx);
        }
    }

    /// Who's in the call, in the order they joined.
    fn people(&self, call: &CallView) -> Vec<pb::VoiceState> {
        self.core.shared.read(|s| {
            let Some(i) = s.instance(&call.instance) else { return Vec::new() };
            if call.is_dm() {
                i.dms.calls.get(&call.conversation_id).map(|c| c.participants.clone()).unwrap_or_default()
            } else {
                crate::core::calls::in_channel(&i.voice, &call.server_id, &call.channel_id)
                    .into_iter()
                    .cloned()
                    .collect()
            }
        })
    }

    /// Where the call is: "#channel" or the person in a direct message.
    fn place(&self, call: &CallView) -> String {
        self.core.shared.read(|s| {
            let Some(i) = s.instance(&call.instance) else { return String::new() };
            if call.is_dm() {
                let me = i.me.as_ref().map(|m| m.id.clone()).unwrap_or_default();
                return i
                    .dms
                    .conversations
                    .iter()
                    .find(|c| c.id == call.conversation_id)
                    .and_then(|c| c.users.iter().find(|u| u.id != me))
                    .map(crate::core::store::user_name)
                    .unwrap_or_default();
            }
            i.channel(&call.server_id, &call.channel_id).map(|c| c.name.clone()).unwrap_or_default()
        })
    }

    fn call_card(&self, call: &CallView, prefs: &Prefs, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let solid = f32::from(prefs.overlay_opacity) / 100.0;
        let me =
            self.core.shared.read(|s| s.instance(&call.instance).and_then(|i| i.me.as_ref().map(|m| m.id.clone())));
        let server = (!call.is_dm()).then(|| call.server_id.clone());
        let mut people = self.people(call);
        if prefs.overlay_speakers_only && !self.using.get() {
            people.retain(|v| call.speaking.contains(&v.user_id));
        }
        let (dot, status) = match call.status {
            Status::Connected => (gpui_kit::Hsla::from(green()), None),
            Status::Connecting => (p.muted_foreground.into(), Some(t("dms-calls.calls.status.connecting"))),
            Status::Reconnecting => {
                (crate::ui::call_parts::amber().into(), Some(t("dms-calls.calls.status.reconnecting")))
            }
        };
        let head = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .text_xs()
            .font_weight(FontWeight::BOLD)
            .text_color(gpui_kit::hsla(0.0, 0.0, 1.0, 0.75))
            .child(div().flex_none().size(px(8.0)).rounded_full().bg(dot))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(status.unwrap_or_else(|| self.place(call))),
            )
            .when(call.self_mute || call.self_deaf, |el| {
                el.child(
                    icon(if call.self_deaf { "headphone-off" } else { "mic-off" }).size(px(14.0)).text_color(red()),
                )
            });
        let mut rows = div().flex().flex_col().gap(px(4.0));
        for v in &people {
            let speaking = call.speaking.contains(&v.user_id);
            let (name, user) = self.core.shared.read(|s| {
                let i = s.instance(&call.instance);
                (
                    i.map(|i| i.display_name(server.as_deref(), &v.user_id)).unwrap_or_default(),
                    i.and_then(|i| i.users.get(&v.user_id).cloned()),
                )
            });
            let mine = me.as_deref() == Some(v.user_id.as_str());
            let (muted, deaf) = if mine {
                (call.self_mute, call.self_deaf)
            } else {
                (v.self_mute || v.server_mute || v.suppress, v.self_deaf || v.server_deaf)
            };
            let row = div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(voice_avatar(
                    user.as_ref(),
                    &v.user_id,
                    24.0,
                    10.0,
                    2.0,
                    speaking,
                    &format!("overlay-{}", v.user_id),
                    window,
                    cx,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_sm()
                        .font_weight(if speaking { FontWeight::EXTRA_BOLD } else { FontWeight::MEDIUM })
                        .text_color(if speaking { gpui_kit::white() } else { gpui_kit::hsla(0.0, 0.0, 1.0, 0.8) })
                        .child(name),
                )
                .when(muted || deaf, |el| {
                    el.child(
                        icon(if deaf { "headphone-off" } else { "mic-off" })
                            .size(px(14.0))
                            .text_color(gpui_kit::hsla(0.0, 0.0, 1.0, 0.6)),
                    )
                });
            rows = rows.child(motion::rise(
                row,
                SharedString::from(format!("overlay-row-{}", v.user_id)),
                Duration::ZERO,
                4.0,
            ));
        }
        let mut card = div()
            .id("overlay-call")
            .w(px(CARD_W))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .p(px(10.0))
            .rounded(radius_xl())
            .bg(gpui_kit::hsla(0.0, 0.0, 0.06, 0.7 * solid))
            .border_1()
            .border_color(gpui_kit::hsla(0.0, 0.0, 1.0, 0.08 * solid))
            .child(head)
            .child(rows);
        if self.using.get() {
            card = card.occlude().child(self.controls(call, cx));
        }
        motion::pop_in(card, "overlay-call-in", (0.0, 0.0), 0.95, 6.0).into_any_element()
    }

    /// Mute, deafen and hang up, while the overlay's in use.
    fn controls(&self, call: &CallView, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let round = |id: &'static str, glyph: &'static str, on: bool, label: String| {
            div()
                .id(id)
                .flex_1()
                .h(px(36.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(radius_lg())
                .cursor_pointer()
                .bg(if on { alpha(red(), 0.25) } else { gpui_kit::hsla(0.0, 0.0, 1.0, 0.1) })
                .text_color(if on { gpui_kit::Hsla::from(red()) } else { gpui_kit::white() })
                .hover(|s| s.bg(gpui_kit::hsla(0.0, 0.0, 1.0, 0.18)))
                .active(|s| s.scale(0.92))
                .tooltip(move |window, cx| crate::ui::overlay::Tip::new(label.clone()).build(window, cx))
                .child(icon(glyph).size(px(16.0)))
        };
        let (mute, deaf) = (call.self_mute, call.self_deaf);
        div()
            .flex()
            .gap(px(6.0))
            .child(
                round(
                    "overlay-mute",
                    if mute || deaf { "mic-off" } else { "mic" },
                    mute || deaf,
                    t(if mute || deaf { "dms-calls.calls.controls.unmute" } else { "dms-calls.calls.controls.mute" }),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.core.set_self_mute(!(mute || deaf));
                    cx.notify();
                })),
            )
            .child(
                round(
                    "overlay-deafen",
                    if deaf { "headphone-off" } else { "headphones" },
                    deaf,
                    t(if deaf { "dms-calls.calls.controls.undeafen" } else { "dms-calls.calls.controls.deafen" }),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.core.set_self_deaf(!deaf);
                    cx.notify();
                })),
            )
            .child(
                div()
                    .id("overlay-hang-up")
                    .flex_1()
                    .h(px(36.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(radius_lg())
                    .cursor_pointer()
                    .bg(p.destructive)
                    .text_color(gpui_kit::white())
                    .hover(|s| s.opacity(0.9))
                    .active(|s| s.scale(0.92))
                    .tooltip(|window, cx| {
                        crate::ui::overlay::Tip::new(t("dms-calls.calls.controls.disconnect")).build(window, cx)
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.core.leave_voice();
                        this.set_using(false, window, cx);
                    }))
                    .child(icon("phone-off").size(px(16.0))),
            )
    }

    fn note_card(&self, note: &Note, solid: f32, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let age = note.at.elapsed();
        // Fading once its time is up, unless the overlay's in use.
        let fade = if self.using.get() || age < NOTE_STAYS {
            1.0
        } else {
            window.request_animation_frame();
            1.0 - ((age - NOTE_STAYS).as_secs_f32() / NOTE_FADES.as_secs_f32()).clamp(0.0, 1.0)
        };
        let target = note.target.clone();
        let card = div()
            .id(SharedString::from(format!("overlay-note-{}", note.id)))
            .w(px(CARD_W))
            .flex()
            .items_start()
            .gap(px(10.0))
            .p(px(10.0))
            .rounded(radius_xl())
            .bg(gpui_kit::hsla(0.0, 0.0, 0.06, 0.7 * solid))
            .border_1()
            .border_color(gpui_kit::hsla(0.0, 0.0, 1.0, 0.08 * solid))
            .opacity(fade)
            .child(
                div()
                    .flex_none()
                    .size(px(28.0))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(alpha(p.primary, 0.25))
                    .text_color(p.primary)
                    .child(icon(note.icon).size(px(14.0))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(note.title.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .line_height(px(16.0))
                            .text_color(gpui_kit::hsla(0.0, 0.0, 1.0, 0.75))
                            .line_clamp(2)
                            .child(note.body.clone()),
                    ),
            )
            .when(self.using.get(), |el| {
                el.occlude()
                    .cursor_pointer()
                    .hover(|s| s.bg(gpui_kit::hsla(0.0, 0.0, 0.12, 0.85)))
                    .active(|s| s.scale(0.98))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        crate::ui::notify::click(target.clone());
                        this.set_using(false, window, cx);
                    }))
            });
        motion::slide_in(card, SharedString::from(format!("overlay-note-in-{}", note.id)), -16.0).into_any_element()
    }
}

impl Render for GameOverlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let prefs = self.core.prefs();
        // Clicks go through unless it's in use.
        let through = !self.using.get();
        if self.through != Some(through) && click_through::set(window, through) {
            self.through = Some(through);
        }
        // In use, messages wait to be clicked.
        if !self.using.get() {
            self.notes.retain(|n| n.at.elapsed() < NOTE_STAYS + NOTE_FADES);
        }
        let solid = f32::from(prefs.overlay_opacity) / 100.0;
        let corner = prefs.overlay_corner;

        let mut column = div().absolute().flex().flex_col().gap(px(8.0));
        column = if corner.top() { column.top(px(MARGIN)) } else { column.bottom(px(MARGIN)).flex_col_reverse() };
        column =
            if corner.left() { column.left(px(MARGIN)).items_start() } else { column.right(px(MARGIN)).items_end() };
        if let Some(call) = self.core.call() {
            column = column.child(self.call_card(&call, &prefs, window, cx));
        }
        let notes: Vec<AnyElement> = {
            let notes = std::mem::take(&mut self.notes);
            let cards = notes.iter().map(|n| self.note_card(n, solid, window, cx)).collect();
            self.notes = notes;
            cards
        };
        column = column.children(notes);

        let mut root = div()
            .id("game-overlay")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .size_full()
            .relative()
            .font_family(FONT)
            .text_color(gpui_kit::white());
        if self.using.get() {
            let hint = match key_label(&prefs) {
                Some(keys) => t_with("desktop.overlay.using", &[("keys", Arg::Str(&keys))]),
                None => t("desktop.overlay.usingNoKey"),
            };
            root = root
                .child(
                    // A click anywhere but the cards goes back to the game.
                    motion::fade_in(
                        div()
                            .id("overlay-backdrop")
                            .absolute()
                            .inset_0()
                            .bg(gpui_kit::hsla(0.0, 0.0, 0.0, 0.35))
                            .on_click(cx.listener(|this, _, window, cx| this.set_using(false, window, cx))),
                        "overlay-backdrop-in",
                        Duration::from_millis(180),
                    ),
                )
                .child(
                    div().absolute().top(px(MARGIN)).left_0().right_0().flex().justify_center().child(motion::pop_in(
                        div()
                            .px(px(14.0))
                            .py(px(8.0))
                            .rounded_full()
                            .bg(gpui_kit::hsla(0.0, 0.0, 0.06, 0.85))
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .child(hint),
                        "overlay-hint-in",
                        (0.5, 0.0),
                        0.9,
                        -8.0,
                    )),
                );
        }
        root.child(column)
    }
}

impl FuwaApp {
    /// Starts hearing the keys `core::hotkeys` reads, and noticing when the
    /// main window comes in front or goes, for the overlay.
    pub(crate) fn listen_for_hotkeys(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay.main = Some(window.window_handle());
        if let Some(mut presses) = self.core.hotkeys.presses() {
            cx.spawn_in(window, async move |this, cx| {
                while let Some(press) = presses.recv().await {
                    if this.update_in(cx, |this, window, cx| this.on_hotkey(press, window, cx)).is_err() {
                        break;
                    }
                }
            })
            .detach();
        }
        cx.observe_window_activation(window, |this, _, cx| this.sync_overlay(cx)).detach();
    }

    /// Whether push to talk is heard by `core::hotkeys`, so the window leaves it alone.
    pub(crate) fn ptt_heard_anywhere(&self) -> bool {
        Hotkeys::reach() == Reach::Yes && self.core.hotkeys.watches("pushToTalk")
    }

    fn on_hotkey(&mut self, press: Press, window: &mut Window, cx: &mut Context<Self>) {
        if press.action == "pushToTalk" {
            if self.core.prefs().input_mode == InputMode::Ptt {
                self.core.set_pushing(press.down);
                cx.notify();
            }
            return;
        }
        // With one of fuwa's windows in front, its own shortcuts already ran.
        if !press.down || cx.active_window().is_some() {
            return;
        }
        match press.action {
            "toggleOverlay" => self.toggle_overlay(window, cx),
            id => {
                if let Some(action) = keybinds::action_by_id(id) {
                    self.run_shortcut(action, window, cx);
                }
            }
        }
    }

    /// The overlay's key: in use, or back out of the way. It opens the
    /// overlay if it wasn't showing.
    pub(crate) fn toggle_overlay(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !offered() || !self.core.prefs().overlay {
            return;
        }
        let using = self.overlay.open.is_some() && self.overlay.using.get();
        if self.overlay.open.is_none() {
            self.open_overlay(cx);
        }
        if let Some((handle, overlay)) = self.overlay.open.clone() {
            let _ = handle.update(cx, |_, window, cx| overlay.update(cx, |o, cx| o.set_using(!using, window, cx)));
        }
    }

    /// Hands a message that would notify you to the overlay, while it shows
    /// them; false if it doesn't, so the system shows it.
    pub(crate) fn overlay_notice(
        &mut self,
        icon: &'static str,
        title: &str,
        body: &str,
        target: &Clicked,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.core.prefs().overlay_notifications {
            return false;
        }
        let Some((_, overlay)) = &self.overlay.open else { return false };
        overlay.update(cx, |o, cx| o.push(icon, title.to_owned(), body.to_owned(), target.clone(), cx));
        true
    }

    /// Opens or closes the overlay as things stand, keeps the keys heard
    /// from a game in step, and looks for games while it waits for one.
    pub(crate) fn sync_overlay(&mut self, cx: &mut Context<Self>) {
        let prefs = self.core.prefs();
        let in_call = self.core.call().is_some();
        let on = prefs.overlay && offered();

        let mut watch = Vec::new();
        let mut hear = |action: &'static str| {
            watch.extend(combos(action, &prefs).iter().filter_map(|c| Combo::parse(c)).map(|c| (action, c)));
        };
        if in_call {
            if prefs.input_mode == InputMode::Ptt {
                hear("pushToTalk");
            }
            hear("toggleMute");
            hear("toggleDeafen");
        }
        if on {
            hear("toggleOverlay");
        }
        self.core.hotkeys.watch(watch);

        let looking = on && prefs.overlay_show == OverlayShow::Games;
        if !looking {
            self.overlay.looking = None;
            self.overlay.game_in_front = false;
        } else if self.overlay.looking.is_none() {
            self.overlay.looking = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(LOOK_EVERY).await;
                    let seen = cx.background_executor().spawn(async { crate::core::foreground::full_screen() }).await;
                    let going = this.update(cx, |this, cx| {
                        if let Some(seen) = seen
                            && seen != this.overlay.game_in_front
                        {
                            this.overlay.game_in_front = seen;
                            this.sync_overlay(cx);
                        }
                    });
                    if going.is_err() {
                        break;
                    }
                }
            }));
        }

        let using = self.overlay.open.is_some() && self.overlay.using.get();
        let fuwa_in_front = cx.active_window().is_some_and(|w| Some(w) == self.overlay.main);
        let wanted = match prefs.overlay_show {
            OverlayShow::Games => {
                (self.overlay.game_in_front || !self.core.games.activities().is_empty())
                    && (in_call || prefs.overlay_notifications)
            }
            OverlayShow::Calls => in_call,
        };
        let want = on && (using || (wanted && !fuwa_in_front));
        if want && self.overlay.open.is_none() {
            self.open_overlay(cx);
        } else if !want && let Some((handle, _)) = self.overlay.open.take() {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
    }

    fn open_overlay(&mut self, cx: &mut Context<Self>) {
        let Some(display) = cx.primary_display() else { return };
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(display.bounds())),
            titlebar: None,
            focus: false,
            show: true,
            kind: WindowKind::PopUp,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            display_id: Some(display.id()),
            window_background: WindowBackgroundAppearance::Transparent,
            app_id: Some("fuwa".into()),
            ..Default::default()
        };
        let (core, using) = (self.core.clone(), self.overlay.using.clone());
        let app = cx.entity().downgrade();
        match gpui_kit::open_window(options, cx, move |window, cx| {
            cx.new(|cx| GameOverlay::new(core, app, using, window, cx))
        }) {
            Ok(opened) => self.overlay.open = Some(opened),
            Err(err) => tracing::warn!("couldn't open the game overlay: {err}"),
        }
    }
}
