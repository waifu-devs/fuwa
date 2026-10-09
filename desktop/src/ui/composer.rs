//! The composer's gate, as the web's `Composer.tsx` has it: what keeps you
//! from writing in a channel (the rules to agree to, a time-out counting
//! down, roles that only let you read), slow mode's wait between your
//! messages (a note under the box and a ring that winds down around the send
//! button), how many characters are left near the limit, and the ring that
//! fills while files go up. The box itself is `chat.rs`'s `composer_bar`.

use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnimationExt as _, AnyElement, Context, FontWeight, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    PathBuilder, SharedString, StatefulInteractiveElement as _, Styled as _, Transformation, Window, canvas, div,
    percentage, point, px, rgb,
};

use crate::core::i18n::{Arg, t, t_with};
use crate::pb;
use crate::ui::app::{Dialog, FuwaApp, Target};
use crate::ui::chat::Blocked;
use crate::ui::motion;
use crate::ui::text::ms_of;
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_xl};
use crate::ui::widgets::{icon, primary_button};

/// The most a message may hold, as the instance counts it (characters).
pub const MAX_CHARS: usize = 4000;
/// The count shows once this few are left.
const COUNT_FROM: usize = MAX_CHARS - 500;

/// What the composer keeps between frames.
#[derive(Default)]
pub struct Composing {
    /// A countdown is redrawing the composer.
    ticking: bool,
    /// The box shook (a send it couldn't do yet): how many times, and when last.
    shakes: u32,
    shaken_at: Option<Instant>,
    /// Why a voice message didn't go, and the place it was for.
    pub voice_problem: Option<(String, String)>,
    /// Whether the box was last told Enter sends (the Chat setting).
    enter_sends: Option<bool>,
}

/// Whether a key sends, by the Chat setting (the web's `sendsMessage`): Enter,
/// or Ctrl+Enter (Cmd+Return on a Mac). The other adds a line.
pub fn sends_message(key: &gpui_kit::Keystroke, with: crate::core::config::SendWith) -> bool {
    if key.key != "enter" {
        return false;
    }
    let m = &key.modifiers;
    let held = if cfg!(target_os = "macos") { m.platform } else { m.control };
    match with {
        crate::core::config::SendWith::Enter => !m.shift && !held,
        crate::core::config::SendWith::ModEnter => held,
    }
}

/// What lets you send in a server's channel right now, worked out as the web's `useSendGate`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Gate {
    pub now: i64,
    /// Allowed to write here.
    pub can_send: bool,
    pub can_attach: bool,
    /// Joined, but hasn't agreed to the server's rules yet.
    pub pending: bool,
    /// Slow mode's seconds between your messages (0 when it doesn't hold you back).
    pub slowmode: i64,
    /// The channel's slow mode, whoever it holds back.
    pub channel_slowmode: i64,
    /// Slow mode is on here, but it doesn't hold you back.
    pub exempt: bool,
    /// When a time-out ends, or 0.
    pub timed_out_until: i64,
    /// When slow mode lets you send again, or 0.
    pub cooldown_until: i64,
}

impl Gate {
    pub fn cooling(&self) -> bool {
        self.cooldown_until > 0
    }

    pub fn timed_out(&self) -> bool {
        self.timed_out_until > 0
    }
}

/// What shows in place of the box when you can't write in a channel.
#[derive(Clone, Debug, PartialEq)]
pub enum Notice {
    Rules,
    TimedOut { left: i64 },
    ReadOnly { title: String, about: String },
}

/// The notice for a gate, or None when you may write.
pub fn notice_of(gate: &Gate, channel_name: &str) -> Option<Notice> {
    if gate.pending {
        return Some(Notice::Rules);
    }
    if gate.timed_out() {
        return Some(Notice::TimedOut { left: gate.timed_out_until - gate.now });
    }
    (!gate.can_send).then(|| Notice::ReadOnly {
        title: t_with("chat.composer.noMessages", &[("channel", Arg::Str(channel_name))]),
        about: t("chat.composer.readOnlyAbout"),
    })
}

/// Time left as a countdown, the web's `formatLeft`: "0:42", "12:05", "3h 20m", "2d 4h".
pub fn format_left(ms: i64) -> String {
    let s = (ms.max(0) + 999) / 1000;
    if s < 3_600 {
        format!("{}:{:02}", s / 60, s % 60)
    } else if s < 86_400 {
        format!("{}h {}m", s / 3_600, s % 3_600 / 60)
    } else {
        format!("{}d {}h", s / 86_400, s % 86_400 / 3_600)
    }
}

/// A length of time in its largest whole unit, the web's `formatDuration`: "30 seconds", "5 minutes", "1 hour".
pub fn format_duration(seconds: i64) -> String {
    let (n, unit) = match seconds {
        s if s >= 86_400 && s % 86_400 == 0 => (s / 86_400, "day"),
        s if s >= 3_600 && s % 3_600 == 0 => (s / 3_600, "hour"),
        s if s >= 60 && s % 60 == 0 => (s / 60, "minute"),
        s => (s, "second"),
    };
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

/// The composer's tools whose pickers open above them, lined up on their right edges.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Timestamp,
    Emoji,
    Gif,
}

/// Amber, the web's `amber-500`, and its text shade for the theme (`amber-600`, `dark:amber-400`).
fn amber(p: &Palette) -> (gpui_kit::Rgba, gpui_kit::Rgba) {
    (rgb(0xf59e0b), if p.dark { rgb(0xfbbf24) } else { rgb(0xd97706) })
}

impl FuwaApp {
    /// The send gate for the open server channel (not secure channels or direct messages).
    pub(crate) fn send_gate(&self) -> Option<Gate> {
        let Some(Target::Channel { key, server, channel }) = self.target() else { return None };
        // Read before the store is (its lock isn't taken twice).
        let can_attach = self.can_attach();
        self.core.shared.read(|s| {
            let i = s.instance(&key)?;
            let c = i.channel(&server, &channel)?;
            let now = crate::core::dms::now_ms();
            let me = i.me.as_ref().map(|m| m.id.as_str()).unwrap_or_default();
            let member = i.my_member(&server);
            let access = i.access(&server);
            let exempt = access.has_in(&channel, pb::Permission::ManageMessages)
                || access.has_in(&channel, pb::Permission::ManageChannels);
            let channel_slowmode = i64::from(c.slowmode_seconds.max(0));
            let slowmode = if exempt { 0 } else { channel_slowmode };
            // When you last sent here: your newest message, or one still on its way.
            let mut last = i
                .messages
                .get(&channel)
                .and_then(|m| {
                    m.items.iter().rev().find(|m| m.author_id == me && m.kind == pb::MessageKind::Unspecified as i32)
                })
                .map_or(0, |m| ms_of(m.created_at.as_ref()));
            for p in i.pending.get(&channel).into_iter().flatten() {
                if p.failed.is_none() {
                    last = last.max(p.created_at_ms);
                }
            }
            let until = member.and_then(|m| m.timed_out_until.as_ref()).map_or(0, |t| ms_of(Some(t)));
            let ready = if slowmode > 0 && last > 0 { last + slowmode * 1000 } else { 0 };
            Some(Gate {
                now,
                can_send: member.is_none() || access.has_in(&channel, pb::Permission::SendMessages),
                can_attach: member.is_some() && can_attach,
                pending: member.is_some() && access.pending,
                slowmode,
                channel_slowmode,
                exempt: exempt && channel_slowmode > 0,
                timed_out_until: if until > now { until } else { 0 },
                cooldown_until: if ready > now { ready } else { 0 },
            })
        })
    }

    /// How far a tool button's right edge is from the composer's outer one, so
    /// its picker lines up with it (the web's `top-end` placement): the bar's
    /// 16px, the card's border and 12px, and 44px for each button after it
    /// (the GIFs, the poll, then send or the microphone).
    pub(crate) fn tool_right(&self, tool: Tool) -> f32 {
        let gifs = self.gif_place().and_then(|(key, _, _)| self.gifs_on(&key)).is_some();
        let mut after = 1 + usize::from(self.can_poll());
        if tool != Tool::Gif {
            after += usize::from(gifs);
        }
        if tool == Tool::Timestamp {
            after += 1;
        }
        16.0 + 1.0 + 12.0 + after as f32 * 44.0
    }

    /// Tells the box whether Enter sends or adds a line, as the Chat setting says.
    pub(crate) fn follow_send_with(&mut self, cx: &mut Context<Self>) {
        let enter = self.core.prefs().send_with == crate::core::config::SendWith::Enter;
        if self.composing.enter_sends != Some(enter) {
            self.composing.enter_sends = Some(enter);
            self.composer.update(cx, |state, cx| state.set_submit_on_enter(enter, cx));
        }
    }

    /// Keeps the composer redrawing while a time-out or slow mode counts down.
    pub(crate) fn tick_gate(&mut self, gate: &Gate, cx: &mut Context<Self>) {
        if self.composing.ticking || !(gate.timed_out() || gate.cooling()) {
            return;
        }
        self.composing.ticking = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(250)).await;
                let more = this.update(cx, |this, cx| {
                    cx.notify();
                    let more = this.send_gate().is_some_and(|g| g.timed_out() || g.cooling());
                    this.composing.ticking = more;
                    more
                });
                if !matches!(more, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// Whether a send has to wait (slow mode, files still going up or broken, too
    /// long, a time-out): the box shakes when waiting would help.
    pub(crate) fn send_held(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        if self.commands.form.is_some() {
            return false;
        }
        if text.chars().count() > MAX_CHARS {
            return true;
        }
        let gate = self.send_gate();
        if gate.as_ref().is_some_and(Gate::timed_out) {
            return true;
        }
        let files = self.files_state();
        if gate.as_ref().is_some_and(Gate::cooling) || files.uploading || files.broken {
            self.shake(cx);
            return true;
        }
        false
    }

    fn shake(&mut self, cx: &mut Context<Self>) {
        self.composing.shakes += 1;
        self.composing.shaken_at = Some(Instant::now());
        cx.notify();
    }

    /// The composer box, shaken from side to side for a moment after a send it couldn't do.
    pub(crate) fn shaken<E: IntoElement + gpui_kit::Styled + 'static>(&self, el: E) -> AnyElement {
        let fresh = self.composing.shaken_at.is_some_and(|at| at.elapsed() < Duration::from_millis(400));
        if !fresh {
            return el.into_any_element();
        }
        // The web's x: [0, -5, 5, -3, 3, 0] over 0.4s.
        const KEYS: [f32; 6] = [0.0, -5.0, 5.0, -3.0, 3.0, 0.0];
        motion::once(
            div().child(el),
            SharedString::from(format!("composer-shake|{}", self.composing.shakes)),
            Duration::from_millis(400),
            |el, t| {
                let at = t.clamp(0.0, 1.0) * 5.0;
                let n = (at.floor() as usize).min(4);
                let x = KEYS[n] + (KEYS[n + 1] - KEYS[n]) * (at - n as f32);
                el.relative().left(px(x))
            },
        )
    }

    /// What goes in place of the box when you can't write: the web's notices
    /// for the open channel, or the plain line others give (`blocked`).
    pub(crate) fn composer_notice(
        &mut self,
        blocked: Option<&Blocked>,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let gate = self.send_gate();
        let name = match self.target() {
            Some(Target::Channel { key, server, channel }) => self
                .core
                .shared
                .read(|s| s.instance(&key).and_then(|i| i.channel(&server, &channel).map(|c| c.name.clone()))),
            _ => None,
        };
        let notice = match (&gate, &name) {
            (Some(gate), Some(name)) => notice_of(gate, name),
            _ => None,
        };
        if let Some(gate) = &gate {
            self.tick_gate(gate, cx);
        }
        let body: AnyElement = match notice {
            Some(Notice::Rules) => {
                let Some(Target::Channel { key, server, .. }) = self.target() else { return None };
                agree_first(
                    p,
                    cx.listener(move |this, _, window, cx| {
                        this.open_dialog(Dialog::Rules { key: key.clone(), server: server.clone() }, window, cx)
                    }),
                )
            }
            Some(Notice::TimedOut { left }) => timed_out(left, p),
            Some(Notice::ReadOnly { title, about }) => read_only(&title, &about, p),
            None => plain(blocked?, p, cx),
        };
        // The keys still show under it, as on the web.
        let footer = self.composer_footer(self.composer_hint(p), gate.as_ref(), p);
        Some(div().flex_none().px(px(16.0)).pb(px(12.0)).child(body).child(footer).into_any_element())
    }

    /// The count of characters left, once the message nears the limit.
    /// As [`chars_left`](Self::chars_left), shrinking and fading away (`exit={{
    /// opacity: 0, scale: 0.8 }}`) once there's room again.
    pub(crate) fn chars_left_in(
        &self,
        length: usize,
        p: &Palette,
        window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::App,
    ) -> Option<AnyElement> {
        let now = (length > COUNT_FROM).then_some(length);
        match (now, motion::kept("chars-left", now.as_ref(), window, cx)) {
            (Some(length), _) => self.chars_left(length, p),
            (None, Some((was, t))) => {
                let e = gpui_kit::ease_out_quint()(t);
                let count = self.chars_left(was, p)?;
                Some(div().flex_none().opacity(1.0 - e).scale(1.0 - 0.2 * e).child(count).into_any_element())
            }
            (None, None) => None,
        }
    }

    pub(crate) fn chars_left(&self, length: usize, p: &Palette) -> Option<AnyElement> {
        if length <= COUNT_FROM {
            return None;
        }
        let over = length > MAX_CHARS;
        let left = MAX_CHARS as i64 - length as i64;
        Some(motion::once(
            div()
                .flex_none()
                .mb(px(8.0))
                .text_xs()
                .line_height(px(16.0))
                .when(over, |el| el.font_weight(FontWeight::BOLD).text_color(p.destructive))
                .when(!over, |el| el.text_color(p.muted_foreground))
                .child(crate::core::i18n::format_number(left, &crate::core::i18n::current().1)),
            "chars-left",
            Duration::from_millis(220),
            // The web's `opacity: 0, scale: 0.8`.
            |el, t| {
                let t = 1.0 - (1.0 - t) * (1.0 - t);
                el.opacity(t).scale(0.8 + 0.2 * t)
            },
        ))
    }

    /// Which keys send and which add a line (the web's `ComposerHint`), or how
    /// to run the command being filled in.
    pub(crate) fn composer_hint(&self, p: &Palette) -> AnyElement {
        let label = |combo: &str| combo.replace("Mod", if cfg!(target_os = "macos") { "⌘" } else { "Ctrl" });
        if let Some(command) = self.commands.picked_name() {
            // The command fills in plain; the keys stay placeholders for `hint_line` to bold.
            let template = t_with(
                "chat.composer.hintCommand",
                &[("command", Arg::Str(&command)), ("keys", Arg::Str("{keys}")), ("esc", Arg::Str("{esc}"))],
            );
            return crate::ui::text::hint_line(&template, &[("keys", "Enter"), ("esc", "Esc")], p).into_any_element();
        }
        let (send, line) = match self.core.prefs().send_with {
            crate::core::config::SendWith::Enter => (label("Enter"), label("Shift+Enter")),
            crate::core::config::SendWith::ModEnter => (label("Mod+Enter"), label("Enter")),
        };
        // Where commands run (a server's own channels), whether or not you may write just now.
        let commands = match self.target() {
            Some(Target::Channel { key, server, channel }) => self.core.shared.read(|s| {
                s.instance(&key).is_some_and(|i| {
                    i.has("agent-commands")
                        && i.channel(&server, &channel).is_some_and(crate::core::commands::takes_commands)
                })
            }),
            _ => false,
        };
        crate::ui::text::hint_line(
            &t(if commands { "chat.composer.hintCommands" } else { "chat.composer.hint" }),
            &[("send", &send), ("newLine", &line)],
            p,
        )
        .into_any_element()
    }

    /// Under the box: a voice message's problem or the keys, then slow mode's note.
    pub(crate) fn composer_footer(&self, hint: AnyElement, gate: Option<&Gate>, p: &Palette) -> AnyElement {
        let place = self.target().map(|t| t.id());
        let problem = self
            .composing
            .voice_problem
            .as_ref()
            .filter(|(at, _)| Some(at) == place.as_ref())
            .map(|(_, why)| why.clone());
        let mut row = div()
            .mt(px(4.0))
            .px(px(4.0))
            .flex()
            .items_center()
            .gap(px(12.0))
            .text_size(px(11.2))
            .line_height(px(16.0))
            .text_color(p.muted_foreground);
        row = match problem {
            Some(why) => row.child(
                div().flex_1().min_w_0().truncate().font_weight(FontWeight::BOLD).text_color(p.destructive).child(why),
            ),
            None => row.child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(hint)),
        };
        if let Some(gate) = gate.filter(|g| (g.slowmode > 0 || g.exempt) && !g.timed_out() && g.can_send && !g.pending)
        {
            row = row.child(slow_note(gate, p));
        }
        row.into_any_element()
    }

    /// Clears a voice message's problem once something new happens in its place.
    pub(crate) fn set_voice_problem(&mut self, problem: Option<String>) {
        let place = self.target().map(|t| t.id()).unwrap_or_default();
        self.composing.voice_problem = problem.map(|why| (place, why));
    }
}

/// Slow mode's pace, or how long until you can send again.
fn slow_note(gate: &Gate, p: &Palette) -> AnyElement {
    let cooling = gate.cooling();
    let (_, amber_text) = amber(p);
    let text = if gate.exempt {
        t_with("chat.composer.slowOthers", &[("duration", Arg::Str(&format_duration(gate.channel_slowmode)))])
    } else if cooling {
        t_with("chat.composer.slowCooling", &[("time", Arg::Str(&format_left(gate.cooldown_until - gate.now)))])
    } else {
        t_with("chat.composer.slowEvery", &[("duration", Arg::Str(&format_duration(gate.slowmode)))])
    };
    let snail = icon("snail").size(px(14.0));
    let snail: AnyElement = if cooling {
        // The web's crawl: a little way forward and back.
        snail
            .with_animation("slow-crawl", gpui_kit::Animation::new(Duration::from_millis(1600)).repeat(), |el, t| {
                let x = (t * std::f32::consts::TAU).sin().abs() * 1.5;
                el.transform(Transformation::translate(point(px(x), px(0.0))))
            })
            .into_any_element()
    } else {
        snail.into_any_element()
    };
    let note = div()
        .id("slow-note")
        .ml_auto()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(4.0))
        .font_weight(FontWeight::BOLD)
        .when(cooling, |el| el.text_color(amber_text))
        .child(snail)
        .child(text)
        .when(gate.exempt, |el| {
            el.tooltip(|window, cx| crate::ui::overlay::Tip::new(t("chat.composer.slowExempt")).build(window, cx))
        });
    motion::slide_in(note, "slow-note-in", 8.0).into_any_element()
}

/// The notices share a card: an icon in a tile, a title and a muted line under it.
fn notice_card(tile: AnyElement, title: &str, about: &str, p: &Palette) -> gpui_kit::Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.0))
        .px(px(12.0))
        .py(px(10.0))
        .rounded(radius_2xl())
        .border_1()
        .text_color(p.foreground)
        .child(tile)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .truncate()
                        .text_sm()
                        .line_height(px(20.0))
                        .font_weight(FontWeight::BOLD)
                        .child(title.to_owned()),
                )
                .child(div().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(about.to_owned())),
        )
}

/// An icon in its tile that wobbles once as it comes in (the web's rotate
/// keyframes: a swing one way, then the other, then still).
fn tile(id: &'static str, glyph: &str, swing: f32, bg: Hsla, fg: Hsla) -> AnyElement {
    let glyph = icon(glyph).size(px(18.0)).with_animation(
        id,
        gpui_kit::Animation::new(Duration::from_millis(850)),
        move |el, t| {
            let t = ((t * 850.0 - 150.0) / 700.0).clamp(0.0, 1.0);
            // 0 -> -swing -> +0.8 swing -> 0, in thirds.
            let k = [0.0, -swing, swing * 0.8, 0.0];
            let at = t * 3.0;
            let n = (at.floor() as usize).min(2);
            let deg = k[n] + (k[n + 1] - k[n]) * (at - n as f32);
            el.transform(Transformation::rotate(gpui_kit::radians(deg.to_radians())))
        },
    );
    // The tile grows in as the card rises (the web's `scale: 0.6`).
    motion::pop(
        div()
            .size(px(36.0))
            .flex_none()
            .rounded(radius_xl())
            .flex()
            .items_center()
            .justify_center()
            .bg(bg)
            .text_color(fg)
            .child(glyph),
        SharedString::from(format!("{id}|in")),
        0.6,
        0.0,
        Duration::ZERO,
    )
    .into_any_element()
}

fn rises(el: impl IntoElement + gpui_kit::Styled + 'static, id: &'static str) -> AnyElement {
    motion::rise(el, id, Duration::ZERO, 12.0).into_any_element()
}

/// In place of the box until you agree to the server's rules: one button to read them.
fn agree_first(
    p: &Palette,
    on_read: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> AnyElement {
    let card = notice_card(
        tile("agree-tile", "scroll-text", 12.0, alpha(p.primary, 0.15), p.primary.into()),
        &t("chat.composer.agreeTitle"),
        &t("chat.composer.agreeAbout"),
        p,
    )
    .flex_wrap()
    .border_color(alpha(p.primary, 0.4))
    .bg(alpha(p.primary, 0.1))
    .child(
        primary_button("read-rules", t("chat.composer.readRules"), p)
            .h(px(36.0))
            .px(px(16.0))
            .rounded(radius_xl())
            .text_sm()
            .on_click(on_read),
    );
    rises(card, "composer-agree")
}

/// In place of the box while you're timed out: how long until you can talk again.
fn timed_out(left: i64, p: &Palette) -> AnyElement {
    let (amber_bg, amber_text) = amber(p);
    let hourglass = icon("hourglass").size(px(18.0)).with_animation(
        "hourglass-flip",
        gpui_kit::Animation::new(Duration::from_millis(3000)).repeat(),
        |el, t| {
            // The web's flip: still, then half a turn, still again.
            let turn = ((t - 0.4) / 0.2).clamp(0.0, 1.0);
            let eased = turn * turn * (3.0 - 2.0 * turn);
            el.transform(Transformation::rotate(percentage(0.5 * eased)))
        },
    );
    let tile = div()
        .size(px(36.0))
        .flex_none()
        .rounded(radius_xl())
        .flex()
        .items_center()
        .justify_center()
        .bg(alpha(amber_bg, 0.15))
        .text_color(amber_text)
        .child(hourglass);
    let card =
        notice_card(tile.into_any_element(), &t("chat.composer.timedOutTitle"), &t("chat.composer.timedOutAbout"), p)
            .border_color(alpha(amber_bg, 0.4))
            .bg(alpha(amber_bg, 0.1))
            .child(
                div()
                    .flex_none()
                    .rounded_full()
                    .bg(alpha(amber_bg, 0.15))
                    .px(px(10.0))
                    .py(px(4.0))
                    .text_sm()
                    .line_height(px(20.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_color(amber_text)
                    .child(format_left(left)),
            );
    rises(card, "composer-timed-out")
}

/// In place of the box where your roles don't let you write.
fn read_only(title: &str, about: &str, p: &Palette) -> AnyElement {
    let card =
        notice_card(tile("read-only-tile", "lock", 10.0, p.muted.into(), p.muted_foreground.into()), title, about, p)
            .border_dashed()
            .border_color(p.border)
            .bg(alpha(p.muted, 0.4));
    rises(card, "composer-read-only")
}

/// The plain line other places give (direct messages, secure channels).
fn plain(blocked: &Blocked, p: &Palette, cx: &mut Context<FuwaApp>) -> AnyElement {
    let action = blocked.action.clone();
    motion::rise(
        div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .px(px(16.0))
            .py(px(10.0))
            .min_h(px(52.0))
            .rounded(radius_2xl())
            .bg(alpha(p.muted_foreground, 0.1))
            .text_sm()
            .text_color(p.muted_foreground)
            .child(icon("lock").size(px(16.0)))
            .child(div().flex_1().child(blocked.text.clone()))
            .when_some(action, |el, (label, dialog)| {
                el.child(
                    primary_button("blocked-action", label, p)
                        .h(px(34.0))
                        .text_sm()
                        .on_click(cx.listener(move |this, _, window, cx| this.open_dialog(dialog.clone(), window, cx))),
                )
            }),
        "composer-blocked",
        Duration::ZERO,
        8.0,
    )
    .into_any_element()
}

/// A ring drawn around a 36px button: a faint track, and `share` of it in `color`
/// from the top, clockwise (the web's 36-unit SVG with r=15 and a 2.5 stroke).
pub(crate) fn ring(share: f32, track: Hsla, color: Hsla) -> AnyElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let side = f32::from(bounds.size.width).min(f32::from(bounds.size.height));
            let scale = side / 36.0;
            let (cx, cy) = (
                f32::from(bounds.origin.x) + f32::from(bounds.size.width) / 2.0,
                f32::from(bounds.origin.y) + f32::from(bounds.size.height) / 2.0,
            );
            let r = 15.0 * scale;
            let arc = |from: f32, sweep: f32| {
                let steps = ((sweep.abs() * 64.0).ceil() as usize).max(2);
                let mut path = PathBuilder::stroke(px(2.5 * scale));
                for n in 0..=steps {
                    let a =
                        (from + sweep * n as f32 / steps as f32) * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                    let at = point(px(cx + r * a.cos()), px(cy + r * a.sin()));
                    if n == 0 { path.move_to(at) } else { path.line_to(at) }
                }
                path.build().ok()
            };
            if let Some(path) = arc(0.0, 1.0) {
                window.paint_path(path, track);
            }
            let share = share.clamp(0.0, 1.0);
            if share > 0.001
                && let Some(path) = arc(0.0, share)
            {
                window.paint_path(path, color);
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}

/// The send button's face while slow mode holds you back: the ring winding
/// down and the seconds left (a snail past 99).
pub(crate) fn cooldown(gate: &Gate, p: &Palette) -> AnyElement {
    let (amber_bg, amber_text) = amber(p);
    let left = gate.cooldown_until - gate.now;
    let total = gate.slowmode * 1000;
    let share = if total > 0 { (left as f32 / total as f32).min(1.0) } else { 0.0 };
    let seconds = (left + 999) / 1000;
    // It pops in over the plane (the web's `scale: 0.4` on a lively spring).
    let ring_face = div()
        .relative()
        .size(px(36.0))
        .flex()
        .items_center()
        .justify_center()
        .text_color(amber_text)
        .child(ring(share, alpha(amber_bg, 0.15), amber_bg.into()))
        .child(if seconds > 99 {
            icon("snail").size(px(16.0)).into_any_element()
        } else {
            motion::rise(
                div().text_size(px(10.4)).font_weight(FontWeight::EXTRA_BOLD).child(seconds.to_string()),
                SharedString::from(format!("cooldown|{seconds}")),
                Duration::ZERO,
                8.0,
            )
            .into_any_element()
        });
    motion::pop(ring_face, "cooldown-in", 0.4, 0.0, Duration::ZERO).into_any_element()
}

/// Around the send button while files go up: how far they've got, together.
pub(crate) fn upload_ring(share: f32, p: &Palette) -> AnyElement {
    let face = div()
        .relative()
        .size(px(36.0))
        .flex()
        .items_center()
        .justify_center()
        .text_color(p.primary)
        .child(ring(share, alpha(p.primary, 0.15), p.primary.into()))
        .child(icon("upload").size(px(14.0)));
    motion::pop(face, "upload-ring-in", 0.4, 0.0, Duration::ZERO).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countdowns_read_like_the_web() {
        assert_eq!(format_left(42_000), "0:42");
        assert_eq!(format_left(41_001), "0:42");
        assert_eq!(format_left(12 * 60_000 + 5_000), "12:05");
        assert_eq!(format_left(3 * 3_600_000 + 20 * 60_000), "3h 20m");
        assert_eq!(format_left(2 * 86_400_000 + 4 * 3_600_000), "2d 4h");
        assert_eq!(format_left(-5), "0:00");
    }

    #[test]
    fn durations_take_their_largest_whole_unit() {
        assert_eq!(format_duration(30), "30 seconds");
        assert_eq!(format_duration(60), "1 minute");
        assert_eq!(format_duration(300), "5 minutes");
        assert_eq!(format_duration(3_600), "1 hour");
        assert_eq!(format_duration(90), "90 seconds");
        assert_eq!(format_duration(21_600), "6 hours");
    }

    #[test]
    fn the_chat_setting_picks_the_key_that_sends() {
        use crate::core::config::SendWith;
        let key = |k: &str| gpui_kit::Keystroke::parse(k).unwrap();
        let held = if cfg!(target_os = "macos") { "cmd-enter" } else { "ctrl-enter" };
        assert!(sends_message(&key("enter"), SendWith::Enter));
        assert!(!sends_message(&key("shift-enter"), SendWith::Enter));
        assert!(!sends_message(&key(held), SendWith::Enter));
        assert!(sends_message(&key(held), SendWith::ModEnter));
        assert!(!sends_message(&key("enter"), SendWith::ModEnter));
        assert!(!sends_message(&key("a"), SendWith::Enter));
    }

    #[test]
    fn the_notice_follows_the_gate() {
        let open = Gate { can_send: true, ..Default::default() };
        assert_eq!(notice_of(&open, "general"), None);
        assert_eq!(notice_of(&Gate { pending: true, ..open.clone() }, "general"), Some(Notice::Rules));
        assert_eq!(
            notice_of(&Gate { now: 1_000, timed_out_until: 61_000, ..open.clone() }, "general"),
            Some(Notice::TimedOut { left: 60_000 })
        );
        assert!(matches!(notice_of(&Gate::default(), "general"), Some(Notice::ReadOnly { .. })));
    }
}
