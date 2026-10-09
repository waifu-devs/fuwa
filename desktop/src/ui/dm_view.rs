//! A private conversation, as the web's `dm/DmView.tsx` draws it: the
//! header with who it's with and the lock that opens how it's encrypted, the
//! start of the conversation, what this device opened (messages in the same
//! rows as a channel's, and lines about devices coming and going), and the
//! encrypted composer. Secure channels (`ui/secure.rs`) share the list, the
//! lines and the composer, as the web's `SecureChannelView` does.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::input::Textarea;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, BoxShadow, Context, Div, Focusable as _, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, Window, div, point, px,
    rgb,
};

use crate::core::config::SendWith;
use crate::core::dms::DmStatus;
use crate::core::i18n::{Arg, t, t_with};
use crate::core::store::{InstanceState, user_name};
use crate::core::vault::{Item, ItemKind};
use crate::pb;
use crate::ui::app::{FuwaApp, Target};
use crate::ui::chat::{Msg, Row, ThreadBits, Who};
use crate::ui::motion;
use crate::ui::text::{clock, hard_breaks, images_as_links};
use crate::ui::theme::{Palette, alpha, mix, radius_2xl, radius_xl};
use crate::ui::timestamps::timestamp_nodes;
use crate::ui::widgets::{avatar, icon, pal};

// ───────────────────────── Colors ─────────────────────────

/// Tailwind's emerald and amber, which the web draws encryption in (its
/// marks, the start's note, the lines about devices).
#[derive(Clone, Copy)]
pub(crate) struct Seal {
    /// emerald-500: badges, line icons, the composer's lock.
    pub green: Hsla,
    /// emerald-700 (300 in the dark): the pill's words.
    pub pill: Hsla,
    /// emerald-600 (400 in the dark): icons on a tinted ground, the promise.
    pub icon: Hsla,
    /// emerald-900 (100 in the dark): the start's note.
    pub note: Hsla,
    /// amber-500: lines about something wrong.
    pub amber: Hsla,
    /// amber-700 (300 in the dark): the pill's words once the safety number changed.
    pub amber_text: Hsla,
}

pub(crate) fn seal(p: &Palette) -> Seal {
    let dark = p.dark;
    Seal {
        green: rgb(0x00bc7d).into(),
        pill: rgb(if dark { 0x5ee9b5 } else { 0x007a55 }).into(),
        icon: rgb(if dark { 0x00d492 } else { 0x009966 }).into(),
        note: rgb(if dark { 0xd0fae5 } else { 0x004f3b }).into(),
        amber: rgb(0xfe9a00).into(),
        amber_text: rgb(if dark { 0xffd230 } else { 0xbb4d00 }).into(),
    }
}

fn with_alpha(c: Hsla, a: f32) -> Hsla {
    Hsla { a: c.a * a, ..c }
}

/// The web's `.shadow-2xl`.
pub(crate) fn shadow_2xl() -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: gpui_kit::hsla(0.0, 0.0, 0.0, 0.25),
        offset: point(px(0.0), px(25.0)),
        blur_radius: px(50.0),
        spread_radius: px(-12.0),
        inset: false,
    }]
}

/// A ring of the card's color around something (Tailwind's `ring-4 ring-card`).
fn ring(p: &Palette, width: f32) -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: p.card.into(),
        offset: point(px(0.0), px(0.0)),
        blur_radius: px(0.0),
        spread_radius: px(width),
        inset: false,
    }]
}

// ───────────────────────── Rows ─────────────────────────

/// The top of a conversation (the web's `Beginning`): who it's with, and that it's encrypted.
#[derive(Clone, PartialEq)]
pub struct DmStart {
    pub user: Option<pb::User>,
    pub name: String,
}

/// What a line about the conversation itself says it's about, for its icon and color.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum LineKind {
    /// Devices came or went, or this one joined.
    Key,
    /// A record this device couldn't open.
    Unreadable,
    /// A secure channel's encryption started over.
    Reset,
    /// A secure channel's history sharing turned on or off.
    Setting,
    /// A secure channel's thread locked or unlocked.
    Thread,
    /// This device is still joining: a key that wobbles, no time.
    Joining,
}

/// A line about the conversation, not something said (the web's `SystemLine`).
#[derive(Clone, PartialEq)]
pub struct SysLine {
    pub id: String,
    pub kind: LineKind,
    pub text: String,
    pub at: i64,
}

/// What an encrypted message adds to a row: it was deleted, passed on as
/// earlier history, and what deleting it asks.
#[derive(Clone, PartialEq, Default)]
pub struct EncBits {
    pub deleted: bool,
    pub shared: bool,
    pub question: String,
    /// The files it carries, sealed.
    pub files: Vec<crate::core::vault::FileRef>,
    /// While it's on its way: the files going with it, by name and size.
    pub sending: Vec<(String, i64)>,
}

/// Where the messages from before this device joined came from (the web's `earlierFrom`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Earlier {
    Shared,
    Backup,
    Restorable,
    None,
}

pub fn earlier_from(items: &[Item], locked: bool) -> Earlier {
    if items.iter().any(|i| !i.shared_by.is_empty()) {
        return Earlier::Shared;
    }
    let joined = items.iter().rev().find(|i| i.kind == ItemKind::Joined);
    if let Some(joined) = joined
        && items.iter().any(|i| i.kind == ItemKind::Text && i.seq < joined.seq)
    {
        return Earlier::Backup;
    }
    if locked { Earlier::Restorable } else { Earlier::None }
}

fn capital(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// What changed about a conversation's devices, in words (the web's `deviceLine`).
pub fn device_line(item: &Item, name_of: &dyn Fn(&str) -> String, me: &str, earlier: Earlier) -> String {
    let count = |n: usize| Arg::Num(n as i64);
    match item.kind {
        ItemKind::Joined => {
            return t(match earlier {
                Earlier::Backup => "dms-calls.dm.devices.joinedBackup",
                Earlier::Restorable => "dms-calls.dm.devices.joinedRestorable",
                _ => "dms-calls.dm.devices.joined",
            });
        }
        ItemKind::Unreadable => {
            return if item.sender_id == me {
                t("dms-calls.dm.devices.unreadableMine")
            } else {
                t_with("dms-calls.dm.devices.unreadable", &[("name", Arg::Str(&name_of(&item.sender_id)))])
            };
        }
        _ => {}
    }
    // The conversation's first record is the commit that made its group.
    if item.seq == 1 {
        return capital(&if item.sender_id == me {
            t("dms-calls.dm.devices.startedMine")
        } else {
            t_with("dms-calls.dm.devices.started", &[("name", Arg::Str(&name_of(&item.sender_id)))])
        });
    }
    let people = |list: &[crate::core::vault::DeviceRef]| {
        let mut order: Vec<String> = Vec::new();
        for d in list {
            if !order.contains(&d.user_id) {
                order.push(d.user_id.clone());
            }
        }
        order.into_iter().map(|u| (list.iter().filter(|d| d.user_id == u).count(), u)).collect::<Vec<_>>()
    };
    let mut parts: Vec<String> = Vec::new();
    for (n, user) in people(&item.added) {
        parts.push(if user == me {
            t_with("dms-calls.dm.devices.addedMine", &[("count", count(n))])
        } else {
            t_with("dms-calls.dm.devices.added", &[("count", count(n)), ("name", Arg::Str(&name_of(&user)))])
        });
    }
    for (n, user) in people(&item.removed) {
        parts.push(if user == me {
            t_with("dms-calls.dm.devices.removedMine", &[("count", count(n))])
        } else {
            t_with("dms-calls.dm.devices.removed", &[("count", count(n)), ("name", Arg::Str(&name_of(&user)))])
        });
    }
    let changes = parts
        .into_iter()
        .reduce(|list, next| {
            t_with("dms-calls.dm.devices.and", &[("list", Arg::Str(&list)), ("next", Arg::Str(&next))])
        })
        .unwrap_or_default();
    capital(&t_with("dms-calls.dm.devices.changed", &[("changes", Arg::Str(&changes))]))
}

fn line_kind(kind: ItemKind) -> LineKind {
    match kind {
        ItemKind::Unreadable => LineKind::Unreadable,
        ItemKind::Reset => LineKind::Reset,
        ItemKind::Setting => LineKind::Setting,
        ItemKind::Thread => LineKind::Thread,
        _ => LineKind::Key,
    }
}

pub(crate) fn plain_msg(id: String, who: Who, content: String, at: i64, mine: bool, enc: EncBits) -> Msg {
    Msg {
        id,
        shown: timestamp_nodes(&hard_breaks(&images_as_links(&content))),
        user: who.user,
        name: who.name,
        color: who.color,
        content,
        mentions_me: false,
        editing: false,
        can_delete: mine,
        at,
        edited: false,
        head: true,
        mine,
        pending: false,
        failed: None,
        nonce: 0,
        unreadable: false,
        badge: None,
        embeds: Vec::new(),
        from: None,
        keep_out: false,
        keeping_out: false,
        poll: None,
        voice: None,
        attachments: Vec::new(),
        gif: None,
        thread: ThreadBits::default(),
        agent: None,
        pinned: false,
        can_pin: false,
        decoration: None,
        owner: false,
        enc: Some(Rc::new(enc)),
        sig: 0,
    }
}

/// Which lines an encrypted list shows and what it adds to them, when it
/// isn't simply everything in the conversation (a secure channel without its
/// threads' replies, or one thread).
pub(crate) struct Lines<'a> {
    /// The lines to show.
    pub items: &'a [Item],
    /// Which messages on their way belong here.
    pub pending: &'a dyn Fn(&crate::core::dms::DmPending) -> bool,
    /// Where files on their way are kept (the channel, or `{id}#{parent}` for a thread).
    pub place: String,
    /// What a message row adds (its thread's replies, that it went to the channel too...).
    pub dress: &'a dyn Fn(&Item, &mut Msg),
    /// A thread panel: no "joining" line.
    pub in_thread: bool,
}

/// The rows of a conversation or a secure channel, from what this device
/// opened: `start`, then each day's divider, messages and lines, then what's
/// on its way. `moderate` is Some for a channel (whether you may delete
/// others' messages there); `pins` is Some in a conversation whose instance
/// keeps pins; `describe` puts a line in words.
#[allow(clippy::too_many_arguments)]
pub(crate) fn encrypted_rows(
    i: &InstanceState,
    id: &str,
    start: Row,
    who: &dyn Fn(&str) -> Who,
    describe: &dyn Fn(&Item) -> String,
    moderate: Option<bool>,
    editing: Option<&str>,
    pins: Option<&[pb::DmPin]>,
    question: &str,
    lines: Option<Lines>,
) -> Vec<Row> {
    let me = i.me.clone();
    let me_id = me.as_ref().map(|m| m.id.clone()).unwrap_or_default();
    let stored = i.dms.items.get(id);
    let items = match &lines {
        Some(l) => l.items,
        None => stored.map(Vec::as_slice).unwrap_or_default(),
    };
    let in_thread = lines.as_ref().is_some_and(|l| l.in_thread);
    let mut rows = vec![start];
    // Still joining (or nothing opened yet): a key that wobbles under the start.
    if !in_thread && (stored.is_none() || i.dms.joining.contains(id)) {
        let text = t(if moderate.is_some() { "chat.secure.joining" } else { "dms-calls.dm.view.joining" });
        rows.push(Row::Line(Rc::new(SysLine { id: "joining".into(), kind: LineKind::Joining, text, at: 0 })));
    }
    let mut last_day: Option<i64> = None;
    let mut day = |rows: &mut Vec<Row>, at: i64, id: String| {
        if !last_day.is_some_and(|d| crate::ui::text::same_day(d, at)) {
            rows.push(Row::Day { id: format!("day|{id}"), text: crate::ui::text::day(at) });
        }
        last_day = Some(at);
    };
    for item in items {
        // The first device to join a conversation doesn't need telling it joined.
        if item.kind == ItemKind::Joined && item.seq <= 1 {
            continue;
        }
        day(&mut rows, item.at, item.seq.to_string());
        let mine = item.sender_id == me_id;
        if item.kind != ItemKind::Text {
            // A lock change shows only in its thread.
            if item.kind == ItemKind::Thread && !in_thread {
                continue;
            }
            rows.push(Row::Line(Rc::new(SysLine {
                id: format!("s{}", item.seq),
                kind: line_kind(item.kind),
                text: describe(item),
                at: item.at,
            })));
            continue;
        }
        let enc = EncBits {
            deleted: item.deleted,
            shared: !item.shared_by.is_empty(),
            question: question.to_owned(),
            files: if item.deleted { Vec::new() } else { item.files.clone() },
            sending: Vec::new(),
        };
        let mut m = plain_msg(item.seq.to_string(), who(&item.sender_id), item.content.clone(), item.at, mine, enc);
        m.editing = editing == Some(m.id.as_str());
        m.edited = item.edited_at > 0;
        m.can_delete = !item.deleted && (mine || moderate == Some(true));
        m.voice = item.voice.as_ref().map(|v| Rc::new(crate::ui::voice_notes::VoiceCard::of(v)));
        if let Some(pins) = pins.filter(|_| !item.deleted) {
            m.can_pin = true;
            m.pinned = pins.iter().any(|p| p.sequence == item.seq);
        }
        if let Some(l) = &lines {
            (l.dress)(item, &mut m);
        }
        rows.push(Row::Msg(Rc::new(m)));
    }
    let shows = |p: &crate::core::dms::DmPending| lines.as_ref().is_none_or(|l| (l.pending)(p));
    for pending in i.dms.sending.get(id).into_iter().flatten().filter(|p| shows(p)) {
        let who = Who { name: me.as_ref().map(user_name).unwrap_or_default(), color: None, user: me.clone() };
        let enc = EncBits { question: question.to_owned(), ..Default::default() };
        let mut m = plain_msg(format!("p{}", pending.nonce), who, pending.text.clone(), pending.created_at, true, enc);
        m.can_delete = false;
        m.pending = true;
        m.nonce = pending.nonce;
        m.failed = pending.failed.clone();
        rows.push(Row::Msg(Rc::new(m)));
    }
    // Files being sealed and sent, with their caption.
    let place = lines.as_ref().map_or(id, |l| l.place.as_str());
    for (nonce, text, files) in crate::ui::sealed_files::pending_in(place) {
        let who = Who { name: me.as_ref().map(user_name).unwrap_or_default(), color: None, user: me.clone() };
        let enc = EncBits { question: question.to_owned(), sending: files, ..Default::default() };
        let mut m = plain_msg(format!("f{nonce}"), who, text, crate::core::dms::now_ms(), true, enc);
        m.can_delete = false;
        m.pending = true;
        rows.push(Row::Msg(Rc::new(m)));
    }
    // One person's messages close together share a header, as in a channel.
    crate::ui::chat::group(&mut rows);
    rows
}

impl FuwaApp {
    /// A conversation's rows, from the store.
    pub(crate) fn dm_rows(&self, i: &InstanceState, conversation: &str) -> Vec<Row> {
        let me = i.me.as_ref().map(|m| m.id.clone()).unwrap_or_default();
        let users: Vec<pb::User> =
            i.dms.conversations.iter().find(|c| c.id == conversation).map(|c| c.users.clone()).unwrap_or_default();
        let person = |id: &str| users.iter().find(|u| u.id == id).cloned().or_else(|| i.users.get(id).cloned());
        let partner = users.iter().find(|u| u.id != me).or(users.first()).cloned();
        let start = Row::DmStart(Rc::new(DmStart {
            name: partner.as_ref().map(user_name).unwrap_or_else(|| t("dms-calls.dm.view.untitled")),
            user: partner,
        }));
        let who = |id: &str| {
            let user = person(id);
            Who { name: user.as_ref().map(user_name).unwrap_or_else(|| "Someone".into()), color: None, user }
        };
        let items = i.dms.items.get(conversation).map(Vec::as_slice).unwrap_or_default();
        let locked = crate::core::backup::state(&self.target().map(|t| t.key().to_owned()).unwrap_or_default()).status
            == crate::core::backup::Status::Locked;
        let earlier = earlier_from(items, locked);
        let name_of = |id: &str| who(id).name;
        let describe = |item: &Item| device_line(item, &name_of, &me, earlier);
        let pins = i.has("pins").then(|| i.dms.pins.get(conversation).map(|l| l.pins.as_slice()).unwrap_or_default());
        let question = t("dms-calls.dm.view.deleteQuestion");
        encrypted_rows(i, conversation, start, &who, &describe, None, self.editing.as_deref(), pins, &question, None)
    }
}

/// Draws the rows only encrypted lists have.
pub(crate) fn render_start(start: &DmStart, p: &Palette) -> AnyElement {
    let s = seal(p);
    let card: Hsla = p.card.into();
    let username = start.user.as_ref().map(|u| format!("@{}", u.username));
    let badge = motion::once(
        div().absolute().right(px(-4.0)).bottom(px(-4.0)).size(px(32.0)).flex().items_center().justify_center(),
        "dm-start-badge",
        Duration::from_millis(900),
        move |el, t| {
            // Springs in after the picture, as the web's badge does (0.45s in, a little overshoot).
            let k = ((t - 0.5) / 0.5).clamp(0.0, 1.0);
            let pop = if k < 1.0 { 1.0 - (1.0 - k).powi(3) + (k * std::f32::consts::PI).sin() * 0.12 } else { 1.0 };
            el.scale(pop.max(0.001)).child(
                div()
                    .size(px(32.0))
                    .rounded_full()
                    .bg(s.green)
                    .shadow(ring_of(card, 4.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(gpui_kit::white())
                    .child(icon("lock-keyhole").size(px(16.0))),
            )
        },
    );
    let note = crate::ui::text::hint_line(
        &t_with(
            "dms-calls.dm.beginning.text",
            &[("encrypted", Arg::Str("{encrypted}")), ("name", Arg::Str(&start.name))],
        ),
        &[("encrypted", &t("dms-calls.dm.beginning.encrypted"))],
        p,
    );
    motion::rise(
        div()
            .px(px(16.0))
            .pt(px(40.0 + fit()))
            .pb(px(16.0))
            .flex()
            .flex_col()
            .items_start()
            .child(
                div()
                    .relative()
                    .size(px(80.0))
                    .child(div().rounded_full().shadow(ring(p, 4.0)).child(avatar(start.user.as_ref(), 80.0, p)))
                    .child(badge),
            )
            .child(
                div()
                    .mt(px(12.0))
                    .text_size(px(30.0))
                    .line_height(px(36.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .child(start.name.clone()),
            )
            .when_some(username, |el, u| {
                el.child(div().text_size(px(16.0)).line_height(px(24.0)).text_color(p.muted_foreground).child(u))
            })
            .child(
                div()
                    .mt(px(12.0))
                    .w_full()
                    .max_w(px(576.0))
                    .flex()
                    .items_start()
                    .gap(px(8.0))
                    .px(px(12.0))
                    .py(px(10.0))
                    .rounded(radius_2xl())
                    .bg(with_alpha(s.green, 0.1))
                    .text_sm()
                    .line_height(px(20.0))
                    .text_color(s.note)
                    .child(icon("lock-keyhole").size(px(16.0)).mt(px(2.0)).text_color(s.icon))
                    .child(div().flex_1().min_w_0().child(note)),
            ),
        "dm-start",
        Duration::from_millis(50),
        16.0,
    )
    .into_any_element()
}

fn ring_of(color: Hsla, width: f32) -> Vec<BoxShadow> {
    vec![BoxShadow {
        color,
        offset: point(px(0.0), px(0.0)),
        blur_radius: px(0.0),
        spread_radius: px(width),
        inset: false,
    }]
}

/// A line about the conversation (the web's `SystemLine`), or "joining" with its wobbling key.
pub(crate) fn render_line(line: &SysLine, p: &Palette) -> AnyElement {
    let s = seal(p);
    if line.kind == LineKind::Joining {
        return div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .px(px(16.0))
            .py(px(12.0))
            .text_sm()
            .line_height(px(20.0))
            .text_color(p.muted_foreground)
            .child(
                div()
                    .size(px(32.0))
                    .flex_none()
                    .rounded_full()
                    .bg(with_alpha(s.green, 0.15))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(gpui_kit::AnimationExt::with_animation(
                        icon("key-round").size(px(16.0)).text_color(s.icon),
                        "joining-key",
                        gpui_kit::Animation::new(Duration::from_millis(1200)).repeat(),
                        |el, t| el.rotate(gpui_kit::radians(wobble(t, &[0.0, -16.0, 12.0, 0.0]).to_radians())),
                    )),
            )
            .child(line.text.clone())
            .into_any_element();
    }
    let (glyph, color) = match line.kind {
        LineKind::Unreadable => ("shield-alert", s.amber),
        LineKind::Reset => ("rotate-ccw-key", s.amber),
        LineKind::Setting => ("rotate-ccw-clock", s.green),
        LineKind::Thread => ("lock", s.green),
        _ => ("key-round", s.green),
    };
    let hover = alpha(p.muted, 0.45);
    div()
        .id(SharedString::from(format!("line|{}", line.id)))
        .group("sysline")
        .flex()
        .items_center()
        .gap(px(12.0))
        .px(px(16.0))
        // app.css's `.message-row` padding wins over the line's own `py-1.5`.
        .py(px(2.0))
        .hover(move |st| st.bg(hover))
        .child(
            // The icon tips back while the line's pointed at.
            div().w(px(40.0)).flex_none().flex().justify_center().child(
                div()
                    .id("sysline-icon")
                    .group_hover("sysline", |st| st.rotate(gpui_kit::radians(-20f32.to_radians())))
                    .child(icon(glyph).size(px(16.0)).text_color(color)),
            ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_wrap()
                .items_baseline()
                .gap_x(px(3.6))
                .text_color(p.muted_foreground)
                // Word by word, so the time follows the last one on its line, as inline text does on the web.
                .children(
                    line.text.split_whitespace().map(|word| {
                        div().max_w_full().text_size(px(14.4)).line_height(px(21.6)).child(word.to_owned())
                    }),
                )
                .child(div().text_xs().line_height(px(16.0)).whitespace_nowrap().child(clock(line.at))),
        )
        .into_any_element()
}

/// A keyframed swing (degrees at evenly spaced times), eased between them.
fn wobble(t: f32, keys: &[f32]) -> f32 {
    let n = keys.len() - 1;
    let at = t.clamp(0.0, 1.0) * n as f32;
    let i = (at.floor() as usize).min(n - 1);
    let f = at - i as f32;
    let f = f * f * (3.0 - 2.0 * f);
    keys[i] + (keys[i + 1] - keys[i]) * f
}

// ───────────────────────── Fitting a short list to the bottom ─────────────────────────

thread_local! {
    /// How far the open list's start is pushed down so a short conversation
    /// sits at the bottom, as the web's `justify-end` list does, and for which list.
    static FIT: RefCell<(String, f32)> = const { RefCell::new((String::new(), 0.0)) };
}

/// The space above the open encrypted list's start.
pub(crate) fn fit() -> f32 {
    FIT.with(|f| f.borrow().1)
}

fn fit_for(target: &str) -> f32 {
    FIT.with(|f| {
        let f = f.borrow();
        if f.0 == target { f.1 } else { 0.0 }
    })
}

// ───────────────────────── The view ─────────────────────────

/// What the conversation's header shows about its safety number.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Trust {
    Encrypted,
    Verified,
    Changed,
}

impl Trust {
    pub fn of(safety: Option<&String>, verified: Option<&String>) -> Self {
        let safety = safety.filter(|s| !s.is_empty());
        let verified = verified.filter(|s| !s.is_empty());
        match (safety, verified) {
            (Some(s), Some(v)) if s == v => Trust::Verified,
            (Some(_), Some(_)) => Trust::Changed,
            _ => Trust::Encrypted,
        }
    }
}

/// The pill in the header (the web's `TrustPill`, and the secure channel's): a lock (a badge once
/// verified, a warning once the number changed) and what it means. It opens the encryption dialog.
pub(crate) fn trust_pill(id: &str, trust: Trust, label: String, p: &Palette) -> Stateful<Div> {
    let s = seal(p);
    let (bg, hover, fg) = match trust {
        Trust::Changed => (with_alpha(s.amber, 0.15), with_alpha(s.amber, 0.25), s.amber_text),
        _ => (with_alpha(s.green, 0.12), with_alpha(s.green, 0.2), s.pill),
    };
    let glyph = match trust {
        Trust::Verified => "badge-check",
        Trust::Changed => "shield-alert",
        Trust::Encrypted => "lock-keyhole",
    };
    let state = match trust {
        Trust::Verified => "verified",
        Trust::Changed => "changed",
        Trust::Encrypted => "encrypted",
    };
    // A glint sweeps across it once as it shows (the web's `.shine`).
    let glint = motion::once(
        div().absolute().top_0().bottom_0().w(px(48.0)),
        SharedString::from(format!("{id}-glint-{state}")),
        Duration::from_millis(1600),
        move |el, t| {
            let k = ((t - 0.3) / 0.7).clamp(0.0, 1.0);
            let white = gpui_kit::hsla(0.0, 0.0, 1.0, if k > 0.0 && k < 1.0 { 0.45 } else { 0.0 });
            let clear = gpui_kit::hsla(0.0, 0.0, 1.0, 0.0);
            el.left(px(-60.0 + 260.0 * k))
                .child(div().absolute().left_0().top_0().bottom_0().w(px(24.0)).bg(gpui_kit::linear_gradient(
                    90.0,
                    gpui_kit::linear_color_stop(clear, 0.0),
                    gpui_kit::linear_color_stop(white, 1.0),
                )))
                .child(div().absolute().left(px(24.0)).top_0().bottom_0().w(px(24.0)).bg(gpui_kit::linear_gradient(
                    90.0,
                    gpui_kit::linear_color_stop(white, 0.0),
                    gpui_kit::linear_color_stop(clear, 1.0),
                )))
        },
    );
    let mark = motion::once(
        div().flex().items_center().justify_center().size(px(14.0)),
        SharedString::from(format!("{id}-mark-{state}")),
        Duration::from_millis(450),
        move |el, t| {
            let pop = 0.3 + 0.7 * (1.0 - (1.0 - t).powi(3)) + (t * std::f32::consts::PI).sin() * 0.15;
            el.scale(pop.min(1.15))
                .rotate(gpui_kit::radians((-40.0 * (1.0 - t)).to_radians()))
                .child(icon(glyph).size(px(14.0)))
        },
    );
    let group = SharedString::from(format!("{id}-pill"));
    // The mark tips and grows while the pill's pointed at.
    let mark = div()
        .id(SharedString::from(format!("{id}-mark")))
        .group_hover(group.clone(), |st| st.rotate(gpui_kit::radians(-0.21)).scale(1.1))
        .child(mark);
    div()
        .id(SharedString::from(id.to_owned()))
        .group(group)
        .relative()
        .overflow_hidden()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(6.0))
        .px(px(10.0))
        .py(px(4.0))
        .rounded_full()
        .bg(bg)
        .text_color(fg)
        .text_xs()
        .line_height(px(16.0))
        .font_weight(FontWeight::BOLD)
        .cursor_pointer()
        .hover(move |st| st.bg(hover))
        .active(|st| st.scale(0.92))
        .child(glint)
        .child(mark)
        .child(label)
}

impl FuwaApp {
    /// The conversation: its header, then what was said and where you write,
    /// or why it can't show (encryption starting, not working here, or not yours).
    pub(crate) fn dm_view(&mut self, key: &str, id: &str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let (conversation, me, status, problem, safety, verified, pins_here) = self.core.shared.read(|s| {
            let Some(i) = s.instance(key) else { return (None, None, DmStatus::Off, None, None, None, false) };
            (
                i.dms.conversations.iter().find(|c| c.id == id).cloned(),
                i.me.clone(),
                i.dms.status,
                i.dms.problem.clone(),
                i.dms.safety.get(id).cloned(),
                i.dms.verified.get(id).cloned(),
                i.has("pins"),
            )
        });
        let me_id = me.as_ref().map(|m| m.id.clone()).unwrap_or_default();
        let partner =
            conversation.as_ref().and_then(|c| c.users.iter().find(|u| u.id != me_id).or(c.users.first()).cloned());
        let ready = status == DmStatus::Ready;
        let header = div()
            .h(px(56.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(16.0))
            .border_b_1()
            .border_color(p.border)
            .child(partner_name(partner.as_ref(), &p, window, cx))
            .child(div().flex_1())
            .when(conversation.is_some() && ready && pins_here, |el| {
                let open = self.pins.is_some();
                el.child(self.pins_button("pins-toggle", open, cx))
            })
            .when_some(
                (conversation.is_some() && ready).then(|| self.dm_call_button(key, id, window, cx)).flatten(),
                |el, button| el.child(button),
            )
            .when(conversation.is_some(), |el| {
                let trust = Trust::of(safety.as_ref(), verified.as_ref());
                let label = t(match trust {
                    Trust::Verified => "dms-calls.dm.trust.verified",
                    Trust::Changed => "dms-calls.dm.trust.changed",
                    Trust::Encrypted => "dms-calls.dm.encrypted",
                });
                let (key, id) = (key.to_owned(), id.to_owned());
                el.child(
                    trust_pill("dm-trust", trust, label, &p)
                        .tooltip(|window, cx| {
                            crate::ui::overlay::Tip::new(t("dms-calls.dm.trust.title")).build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let dialog = crate::ui::app::Dialog::Safety { key: key.clone(), conversation: id.clone() };
                            this.open_dialog(dialog, window, cx)
                        })),
                )
            });
        let body: AnyElement = if conversation.is_some() && me.is_some() {
            let composer = self.encrypted_composer(
                Composer {
                    key: key.to_owned(),
                    id: id.to_owned(),
                    promise: t("dms-calls.dm.view.promise"),
                    locked: None,
                    action: None,
                    files: true,
                    thread: None,
                },
                window,
                cx,
            );
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .when_some(self.dm_call_strip(key, id, window, cx), |el, strip| el.child(strip))
                .child(self.fitted_list(window, cx))
                .child(composer)
                .into_any_element()
        } else {
            no_conversation(status, problem, &p, window)
        };
        let column = div().flex_1().min_w_0().h_full().flex().flex_col().child(header).child(body);
        div().size_full().flex().child(column).into_any_element()
    }

    /// The message list, its start pushed down while everything fits, so a
    /// short conversation sits at the bottom as on the web.
    pub(crate) fn fitted_list(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let target = self.list.target.clone().unwrap_or_default();
        FIT.with(|f| {
            if f.borrow().0 != target {
                *f.borrow_mut() = (target.clone(), 0.0);
            }
        });
        let this = cx.entity().downgrade();
        let watch = gpui_kit::canvas(
            |_, _, _| {},
            move |bounds, _, window, cx| {
                // After the rows are laid out: where the start and the last row ended up.
                let (start, last) = PLACED.with(|p| p.take());
                let current = fit_for(&target);
                let next = match (start, last) {
                    // Everything fits: push the start down by what's left below the last row.
                    (Some(top), Some(bottom)) if top >= bounds.origin.y - px(0.5) => {
                        let room = f32::from(bounds.origin.y + bounds.size.height - px(12.0) - bottom);
                        (current + room).max(0.0)
                    }
                    // The start is above the top: it doesn't fit, so less room (none, once it's out of sight).
                    (Some(top), _) => (current - f32::from(bounds.origin.y - top)).max(0.0),
                    (None, _) => 0.0,
                };
                if (next - current).abs() > 0.5 {
                    FIT.with(|f| *f.borrow_mut() = (target.clone(), next));
                    let this = this.clone();
                    window.defer(cx, move |_, cx| {
                        let _ = this.update(cx, |this, cx| {
                            this.scroller.update(cx, |s, cx| {
                                s.remeasure_items(0..1, cx);
                            });
                            cx.notify();
                        });
                    });
                }
            },
        )
        .absolute()
        .inset_0();
        div()
            .flex_1()
            .min_h_0()
            .relative()
            .flex()
            .flex_col()
            .child(self.message_list(window, cx))
            .child(watch)
            .into_any_element()
    }
}

thread_local! {
    /// Where the open list's start (its top) and its last row (its bottom) were laid out this frame.
    static PLACED: RefCell<(Option<gpui_kit::Pixels>, Option<gpui_kit::Pixels>)> = const { RefCell::new((None, None)) };
}

/// Notes where a row of an encrypted list was laid out, for [`FuwaApp::fitted_list`].
pub(crate) fn placed(el: AnyElement, start: bool, last: bool) -> AnyElement {
    if !start && !last {
        return el;
    }
    div()
        .relative()
        .child(el)
        .child(
            gpui_kit::canvas(
                move |bounds, _, _| {
                    PLACED.with(|p| {
                        let mut p = p.borrow_mut();
                        if start {
                            p.0 = Some(bounds.origin.y);
                        }
                        if last {
                            p.1 = Some(bounds.origin.y + bounds.size.height);
                        }
                    })
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        )
        .into_any_element()
}

/// Who the conversation is with, in the header; it slides up when that changes.
fn partner_name(partner: Option<&pb::User>, p: &Palette, window: &mut Window, cx: &mut gpui_kit::App) -> AnyElement {
    let key = partner.map_or_else(|| "none".to_owned(), |u| u.id.clone());
    let name = partner.map_or_else(|| t("dms-calls.dm.view.untitled"), user_name);
    motion::rise(
        div().flex().min_w_0().flex_shrink(1.0).items_center().gap(px(10.0)).child(avatar(partner, 32.0, p)).child(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .truncate()
                        .text_size(px(16.0))
                        .line_height(px(20.0))
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .child(motion::swap_text(format!("dm-partner-name|{key}"), name, 16.0, window, cx)),
                )
                .when_some(partner, |el, u| {
                    el.child(
                        div()
                            .truncate()
                            .text_xs()
                            .line_height(px(15.0))
                            .text_color(p.muted_foreground)
                            .child(format!("@{}", u.username)),
                    )
                }),
        ),
        SharedString::from(format!("dm-partner|{key}")),
        Duration::ZERO,
        10.0,
    )
    .into_any_element()
}

/// In place of a conversation that can't show (the web's `NoConversation`).
fn no_conversation(status: DmStatus, problem: Option<String>, p: &Palette, window: &Window) -> AnyElement {
    match status {
        DmStatus::Failed => unavailable(&problem.unwrap_or_else(|| t("dms-calls.dm.unavailable")), "shield-off", p),
        DmStatus::Ready => unavailable(&t("dms-calls.dm.view.notHere"), "user-round-x", p),
        _ => starting(p, window),
    }
}

/// Encryption getting ready on this device: a key that swings (the web's `Starting`).
pub(crate) fn starting(p: &Palette, window: &Window) -> AnyElement {
    let s = seal(p);
    let key = motion::ambient(
        icon("key-round").size(px(28.0)).text_color(s.icon),
        "starting-key",
        Duration::from_millis(2000),
        window,
        |el, t| {
            // 1.6s of swing, then 0.4s still.
            let k = (t * 2.0 / 1.6).min(1.0);
            el.rotate(gpui_kit::radians(wobble(k, &[0.0, -18.0, 14.0, -8.0, 0.0]).to_radians()))
        },
    );
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .items_center()
        .justify_center()
        .p(px(24.0))
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .child(
                    div()
                        .size(px(56.0))
                        .rounded(radius_2xl())
                        .bg(with_alpha(s.green, 0.15))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(key),
                )
                .child(
                    div()
                        .mt(px(16.0))
                        .text_size(px(16.0))
                        .line_height(px(24.0))
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .child(t("dms-calls.dm.starting.title")),
                )
                .child(
                    div()
                        .mt(px(4.0))
                        .max_w(px(320.0))
                        .text_sm()
                        .line_height(px(20.0))
                        .text_center()
                        .text_color(p.muted_foreground)
                        .child(t("dms-calls.dm.starting.text")),
                ),
        )
        .into_any_element()
}

/// Why encrypted messages can't show here (the web's `Unavailable`).
pub(crate) fn unavailable(text: &str, glyph: &str, p: &Palette) -> AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .items_center()
        .justify_center()
        .p(px(24.0))
        .child(motion::rise(
            div()
                .flex()
                .flex_col()
                .items_center()
                .child(
                    div()
                        .size(px(56.0))
                        .rounded(radius_2xl())
                        .bg(p.muted)
                        .text_color(p.muted_foreground)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(glyph).size(px(28.0))),
                )
                .child(
                    div()
                        .mt(px(16.0))
                        .max_w(px(384.0))
                        .text_sm()
                        .line_height(px(20.0))
                        .text_center()
                        .text_color(p.muted_foreground)
                        .child(text.to_owned()),
                ),
            "unavailable",
            Duration::ZERO,
            10.0,
        ))
        .into_any_element()
}

// ───────────────────────── The composer ─────────────────────────

/// Where an encrypted composer writes and what it says.
pub(crate) struct Composer {
    pub key: String,
    /// The conversation or secure channel.
    pub id: String,
    /// Under the box: who can read it.
    pub promise: String,
    /// Why you can't write here at all (no permission), if you can't.
    pub locked: Option<String>,
    /// Shown where "Try again" is when you can't write (a secure channel's "Start encryption over").
    pub action: Option<AnyElement>,
    /// Files can go with messages here (sealed on this device).
    pub files: bool,
    /// In a secure channel's thread: the thread's message, and the channel's name for "Also send to #channel".
    pub thread: Option<(i64, String)>,
}

impl Composer {
    /// Where its files and what went wrong are kept: the conversation or channel, or the thread.
    fn place(&self) -> String {
        match &self.thread {
            Some((parent, _)) => crate::ui::secure_threads::place_of(&self.id, *parent),
            None => self.id.clone(),
        }
    }
}

thread_local! {
    /// What went wrong sending, per conversation, said under the box until the next send.
    static PROBLEM: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

/// Says (or with None clears) what went wrong sending in a conversation.
pub(crate) fn set_problem(id: &str, problem: Option<String>) {
    PROBLEM.with(|m| match problem {
        Some(why) => {
            m.borrow_mut().insert(id.to_owned(), why);
        }
        None => {
            m.borrow_mut().remove(id);
        }
    });
}

impl FuwaApp {
    /// Sends again a message that didn't go, in the open conversation or secure channel.
    pub(crate) fn retry_dm(&mut self, nonce: u64, cx: &mut Context<Self>) {
        let Some(Target::Dm { key, conversation: id } | Target::Secure { key, channel: id, .. }) = self.target() else {
            return;
        };
        set_problem(&id, None);
        let (core, place) = (self.core.clone(), id.clone());
        self.run(cx, async move { core.retry_dm(&key, &id, nonce).await }, move |_, result, cx| {
            set_problem(&place, result.err().map(|e| e.0));
            cx.notify();
        });
    }

    /// The web's `EncryptedComposer`: a lock that says it's sealed on this device, the box, the
    /// timestamp tool, then send (or the microphone in a conversation while there's nothing to
    /// send); under it the keys and who can read it. When you can't write, why, in its place.
    pub(crate) fn encrypted_composer(
        &mut self,
        c: Composer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = pal(cx);
        let s = seal(&p);
        self.follow_send_with(cx);
        let (status, stuck) = self.core.shared.read(|st| {
            st.instance(&c.key)
                .map(|i| (i.dms.status, i.dms.blocked.get(&c.id).cloned().filter(|b| !b.is_empty())))
                .unwrap_or((DmStatus::Off, None))
        });
        let ready = status == DmStatus::Ready;
        let locked = c.locked.clone();
        let blocked = locked.clone().or(stuck);
        let problem = PROBLEM.with(|m| m.borrow().get(&c.place()).cloned());
        let note = problem.unwrap_or(c.promise.clone());
        let also = c.thread.as_ref().map(|(_, name)| name.clone());
        let footer = self.encrypted_footer(note, blocked.is_some(), also, &p, cx);
        let Some(why) = blocked else {
            return self.composer_box(c, ready, footer, &p, &s, window, cx);
        };
        let action = match c.action {
            Some(action) => Some(action),
            // A thread's box offers nothing in its place.
            None if locked.is_none() && c.thread.is_none() => {
                let (key, id) = (c.key.clone(), c.id.clone());
                Some(
                    div()
                        .id("enc-try-again")
                        .flex_none()
                        .px(px(12.0))
                        .py(px(6.0))
                        .rounded(radius_xl())
                        .text_xs()
                        .line_height(px(16.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.primary)
                        .cursor_pointer()
                        .hover({
                            let bg = alpha(p.primary, 0.1);
                            move |st| st.bg(bg)
                        })
                        .active(|st| st.translate_y(px(1.0)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let core = this.core.clone();
                            let (key, id) = (key.clone(), id.clone());
                            this.run(cx, async move { core.prepare_conversation(&key, &id).await }, |_, _, cx| {
                                cx.notify()
                            });
                        }))
                        .child(t("dms-calls.dm.composer.tryAgain"))
                        .into_any_element(),
                )
            }
            None => None,
        };
        let title = t(if locked.is_some() { "dms-calls.dm.composer.locked" } else { "dms-calls.dm.composer.blocked" });
        let notice = motion::rise(
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .px(px(12.0))
                .py(px(10.0))
                .rounded(radius_2xl())
                .border_1()
                .border_dashed()
                .border_color(p.border)
                .bg(alpha(p.muted, 0.4))
                .child(
                    div()
                        .size(px(36.0))
                        .flex_none()
                        .rounded(radius_xl())
                        .bg(p.muted)
                        .text_color(p.muted_foreground)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(motion::once(
                            icon("key-round").size(px(18.0)),
                            "blocked-key",
                            Duration::from_millis(700),
                            |el, t| el.rotate(gpui_kit::radians(wobble(t, &[-20.0, -10.0, 8.0, 0.0]).to_radians())),
                        )),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_sm().line_height(px(20.0)).font_weight(FontWeight::BOLD).child(title))
                        .child(div().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(why)),
                )
                .children(action),
            SharedString::from(format!("enc-blocked|{}", c.id)),
            Duration::ZERO,
            12.0,
        );
        div().flex_none().px(px(16.0)).pb(px(12.0)).child(notice).child(footer).into_any_element()
    }

    /// Under the box (the web's `ComposerFooter`): "Also send to #channel" in a thread, which keys
    /// send, and the promise or what went wrong.
    fn encrypted_footer(
        &self,
        note: String,
        hidden: bool,
        also: Option<String>,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = seal(p);
        let label = |combo: &str| combo.replace("Mod", if cfg!(target_os = "macos") { "⌘" } else { "Ctrl" });
        let (send, line) = match self.core.prefs().send_with {
            SendWith::Enter => (label("Enter"), label("Shift+Enter")),
            SendWith::ModEnter => (label("Mod+Enter"), label("Enter")),
        };
        let keys = crate::ui::text::hint_line(
            &t_with("dms-calls.dm.composer.keys", &[("send", Arg::Str("{send}")), ("newLine", Arg::Str("{newLine}"))]),
            &[("send", &send), ("newLine", &line)],
            p,
        );
        div()
            .mt(px(4.0))
            .px(px(4.0))
            .flex()
            .items_center()
            .gap(px(12.0))
            .text_size(px(11.2))
            .line_height(px(16.8))
            .text_color(p.muted_foreground)
            .when(hidden, |el| el.opacity(0.0))
            .map(|el| match also {
                Some(channel) => {
                    let on = self.secure_also();
                    el.child(
                        div()
                            .id("secure-also")
                            .min_w_0()
                            .flex_shrink(1.0)
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .font_weight(FontWeight::BOLD)
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| this.set_secure_also(!on, cx)))
                            .child(
                                div()
                                    .size(px(14.0))
                                    .flex_none()
                                    .rounded(px(3.0))
                                    .border_1()
                                    .border_color(if on { p.primary } else { p.muted_foreground })
                                    .bg(if on { p.primary.into() } else { gpui_kit::transparent_black() })
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .when(on, |el| {
                                        el.child(icon("check").size(px(11.0)).text_color(p.primary_foreground))
                                    }),
                            )
                            .child(
                                div().truncate().child(t_with(
                                    "dms-calls.dm.composer.alsoSend",
                                    &[("channel", Arg::Str(&channel))],
                                )),
                            ),
                    )
                }
                None => el.child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(keys)),
            })
            .child(
                div()
                    .ml_auto()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(s.icon)
                    .child(icon("lock-keyhole").size(px(12.0)))
                    .child(note),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn composer_box(
        &mut self,
        c: Composer,
        ready: bool,
        footer: AnyElement,
        p: &Palette,
        s: &Seal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *p;
        let thread = c.thread.is_some();
        let place = c.place();
        // A thread has a box of its own.
        let input = if thread { self.threads.reply.clone() } else { self.composer.clone() };
        let focused = input.read(cx).focus_handle(cx).is_focused(window);
        let value = input.read(cx).value().to_string();
        let length = value.chars().count();
        let typed = !value.trim().is_empty();
        let picked = c.files && !crate::ui::sealed_files::picked(&place).is_empty();
        let can_send = (typed || picked) && length <= crate::core::dms::MAX_DM && ready;
        let tag = if thread { "thread-composer" } else { "composer" };
        let ring =
            motion::follow(SharedString::from(format!("{tag}-ring")), if focused { 1.0 } else { 0.0 }, window, cx);
        let lit =
            motion::follow(SharedString::from(format!("{tag}-send")), if can_send { 1.0 } else { 0.0 }, window, cx);
        let rest = motion::follow(
            SharedString::from(format!("{tag}-send-scale")),
            if can_send { 1.0 } else { 0.9 },
            window,
            cx,
        );
        // The time picker opens over the box whose button opened it.
        let time_panel =
            if thread == crate::ui::secure_threads::time_in_thread() { self.time_picker_panel(&p, cx) } else { None };
        let recording = !thread && self.recording_here();
        // Voice messages only in conversations: the microphone takes send's place while nothing's typed.
        let voice_here = !thread && matches!(self.target(), Some(Target::Dm { .. }));
        let voice =
            (voice_here && (recording || (!typed && !picked && self.can_record()))).then(|| self.voice_button(&p, cx));
        let attach = (c.files && !recording).then(|| self.encrypted_attach(&place, ready, &p, cx));
        let tray = if c.files { self.picked_tray(&place, &p, cx) } else { None };
        let field: AnyElement = if recording {
            self.recording_bar(&p, cx)
        } else {
            div()
                .id("composer-field")
                .flex_1()
                .min_w_0()
                .ml(px(-10.0))
                .my(px(-2.0))
                .on_mouse_down(
                    gpui_kit::MouseButton::Right,
                    self.right_click(crate::ui::context_menu::MenuOf::Composer, cx),
                )
                .child(Textarea::new(&input).appearance(false).text_size(px(15.2)).line_height(px(24.0)))
                .into_any_element()
        };
        let lock = div()
            .id("enc-lock")
            .mb(px(8.0))
            .size(px(20.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .text_color(s.green)
            .tooltip(|window, cx| crate::ui::overlay::Tip::new(t("dms-calls.dm.composer.sealed")).build(window, cx))
            .child(icon("lock-keyhole").size(px(16.0)));
        let send = voice.is_none().then(|| {
            div()
                .id("send")
                .size(px(36.0))
                .mb(px(2.0))
                .flex_none()
                .rounded(radius_xl())
                .flex()
                .items_center()
                .justify_center()
                .bg(alpha(p.primary, lit))
                .text_color(mix(p.muted_foreground, p.primary_foreground, lit))
                .shadow(vec![BoxShadow {
                    color: alpha(p.primary, lit),
                    offset: point(px(0.0), px(6.0)),
                    blur_radius: px(18.0),
                    spread_radius: px(-8.0),
                    inset: false,
                }])
                .cursor_pointer()
                // It grows when there's something to send, and gives when pressed.
                .scale(rest)
                .active(|st| st.scale(0.85))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if thread {
                        this.threads.reply.update(cx, |state, cx| state.focus(window, cx));
                        return this.send_secure_reply(window, cx);
                    }
                    this.composer.update(cx, |state, cx| state.focus(window, cx));
                    this.send_now(window, cx);
                }))
                .child(icon("send-horizontal").size(px(18.0)))
        });
        let row = div()
            .flex()
            .items_end()
            .gap(px(8.0))
            .when(!recording, |el| el.child(lock))
            .child(field)
            .children(if recording { None } else { self.chars_left(length, &p) })
            .children(attach)
            .when(!recording, |el| {
                el.child(
                    div()
                        .on_mouse_down(gpui_kit::MouseButton::Left, move |_, _, _| {
                            crate::ui::secure_threads::set_time_in_thread(thread)
                        })
                        .child(self.timestamp_button(&p, cx)),
                )
            })
            .children(voice)
            .children(send);
        let card = div()
            .id("composer-box")
            .flex()
            .flex_col()
            .px(px(12.0))
            .py(px(8.0))
            .rounded(radius_2xl())
            .bg(p.card)
            .border_1()
            .border_color(mix(p.border, mix(p.border, p.primary, 0.6).into(), ring))
            .shadow(vec![
                BoxShadow {
                    color: alpha(p.primary, 0.14 * ring),
                    offset: point(px(0.0), px(0.0)),
                    blur_radius: px(0.0),
                    spread_radius: px(4.0),
                    inset: false,
                },
                BoxShadow {
                    color: alpha(p.primary, ring),
                    offset: point(px(0.0), px(12.0)),
                    blur_radius: px(30.0),
                    spread_radius: px(-18.0),
                    inset: false,
                },
            ])
            .map(|el| self.mic_slide(el, cx))
            .children(tray)
            .child(row);
        div()
            .flex_none()
            .relative()
            .px(px(16.0))
            .pb(px(12.0))
            .children(time_panel)
            .child(self.shaken(card))
            .child(footer)
            .into_any_element()
    }
}

/// The marks on an encrypted message: that it was deleted, or passed on when this device joined.
pub(crate) fn deleted_text(p: &Palette) -> AnyElement {
    div().text_sm().italic().text_color(p.muted_foreground).child(t("dms-calls.dm.deleted")).into_any_element()
}

/// The "shared" pill after a message passed on as earlier history.
pub(crate) fn shared_pill(id: &str, p: &Palette) -> AnyElement {
    div()
        .id(SharedString::from(format!("shared|{id}")))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(4.0))
        .px(px(6.0))
        .py(px(1.0))
        .rounded_full()
        .bg(p.muted)
        .text_size(px(10.4))
        .line_height(px(14.0))
        .font_weight(FontWeight::BOLD)
        .text_color(p.muted_foreground)
        .tooltip(|window, cx| crate::ui::overlay::Tip::new(t("dms-calls.dm.row.sharedTitle")).build(window, cx))
        .child(icon("rotate-ccw-clock").size(px(12.0)))
        .child(t("dms-calls.dm.row.shared"))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::vault::DeviceRef;

    fn names(id: &str) -> String {
        match id {
            "u2" => "Yuki".into(),
            _ => "Someone".into(),
        }
    }

    #[test]
    fn device_lines_read_like_the_web() {
        let mut started = Item::new(1, ItemKind::Devices, 0, "u2", "d");
        started.added = vec![DeviceRef { user_id: "me".into(), device_id: "a".into() }];
        assert_eq!(device_line(&started, &names, "me", Earlier::None), "Yuki started this encrypted conversation.");
        let mut added = Item::new(5, ItemKind::Devices, 0, "u2", "d");
        added.added = vec![
            DeviceRef { user_id: "me".into(), device_id: "b".into() },
            DeviceRef { user_id: "u2".into(), device_id: "c".into() },
            DeviceRef { user_id: "u2".into(), device_id: "e".into() },
        ];
        added.removed = vec![DeviceRef { user_id: "u2".into(), device_id: "z".into() }];
        assert_eq!(
            device_line(&added, &names, "me", Earlier::None),
            "You signed in on a new device, and Yuki signed in on 2 new devices, and one of Yuki's devices signed out. The safety number changed."
        );
        let joined = Item::new(9, ItemKind::Joined, 0, "me", "d");
        assert!(device_line(&joined, &names, "me", Earlier::Backup).contains("message backup"));
        let unreadable = Item::new(10, ItemKind::Unreadable, 0, "u2", "d");
        assert_eq!(
            device_line(&unreadable, &names, "me", Earlier::None),
            "A message from Yuki couldn't be opened on this device."
        );
    }

    #[test]
    fn earlier_messages_came_from_somewhere() {
        let mut text = Item::new(2, ItemKind::Text, 0, "u2", "d");
        let joined = Item::new(3, ItemKind::Joined, 0, "me", "d");
        assert_eq!(earlier_from(&[text.clone(), joined.clone()], false), Earlier::Backup);
        assert_eq!(earlier_from(std::slice::from_ref(&joined), true), Earlier::Restorable);
        text.shared_by = "x".into();
        assert_eq!(earlier_from(&[text, joined], false), Earlier::Shared);
    }

    #[test]
    fn a_conversations_pinned_messages_are_marked() {
        let mut i = InstanceState::new("k", "https://k");
        let item = |seq: i64| {
            serde_json::from_value(serde_json::json!({
                "seq": seq, "kind": "text", "at": seq, "sender_id": "u", "device_id": "d", "content": "hi"
            }))
            .unwrap()
        };
        i.dms.items.insert("c".into(), vec![item(1), item(2), item(3)]);
        let who = |_: &str| Who { name: "U".into(), color: None, user: None };
        let start = || Row::Older { loading: false };
        let marks = |pins: Option<&[pb::DmPin]>| {
            encrypted_rows(&i, "c", start(), &who, &|_| String::new(), None, None, pins, "", None)
                .iter()
                .filter_map(|r| match r {
                    Row::Msg(m) => Some((m.id.clone(), m.pinned, m.can_pin)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let pin = pb::DmPin { conversation_id: "c".into(), sequence: 2, pinned_at: None };
        assert_eq!(
            marks(Some(&[pin])),
            [("1".into(), false, true), ("2".into(), true, true), ("3".into(), false, true)]
        );
        // Where the instance doesn't keep pins (or in a secure channel): nothing to pin.
        assert_eq!(marks(None), [("1".into(), false, false), ("2".into(), false, false), ("3".into(), false, false)]);
    }

    #[test]
    fn the_header_trusts_only_a_matching_number() {
        let (a, b) = ("1".to_owned(), "2".to_owned());
        assert!(Trust::of(Some(&a), Some(&a)) == Trust::Verified);
        assert!(Trust::of(Some(&a), Some(&b)) == Trust::Changed);
        assert!(Trust::of(Some(&a), None) == Trust::Encrypted);
    }
}
