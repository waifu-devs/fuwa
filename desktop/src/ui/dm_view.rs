//! A private conversation, as the web's `dm/DmView.tsx` draws it: the
//! header with who it's with and the lock that opens how it's encrypted, the
//! start of the conversation, what this device opened (messages in the same
//! rows as a channel's, and lines about devices coming and going), and the
//! encrypted composer. Secure channels (`ui/secure.rs`) share the list, the
//! lines and the composer, as the web's `SecureChannelView` does.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash as _, Hasher};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::input::Textarea;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, BoxShadow, Context, Div, Focusable as _, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, RenderOnce, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, Window, div,
    point, px, rgb,
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

/// What the encryption marks are drawn in (the web's `--seal-*`, from
/// `core::accessibility::seal_colors`): each on a solid surface of its own,
/// so they read over see-through panels and backdrop pictures alike.
#[derive(Clone, Copy)]
pub(crate) struct Seal {
    /// A mark on its own (line icons, the composer's lock), on the card or background.
    pub green: Hsla,
    /// Words and icons on the seal's surface; also the solid badges' fill.
    pub pill: Hsla,
    /// The same, for icons on the surface.
    pub icon: Hsla,
    /// The start's note: the theme's own text, on the surface.
    pub note: Hsla,
    /// The pills', notes' and tiles' solid surface, pointed at, and outline.
    pub surface: Hsla,
    pub surface_hover: Hsla,
    pub edge: Hsla,
    /// A lone mark about something wrong.
    pub amber: Hsla,
    /// Words on the amber surface, once the safety number changed.
    pub amber_text: Hsla,
    pub amber_surface: Hsla,
    pub amber_surface_hover: Hsla,
    pub amber_edge: Hsla,
}

pub(crate) fn seal(p: &Palette) -> Seal {
    let c = p.seal;
    let h = |hex: u32| -> Hsla { rgb(hex).into() };
    Seal {
        green: h(c.icon),
        pill: h(c.ink),
        icon: h(c.ink),
        note: p.card_foreground.into(),
        surface: h(c.surface),
        surface_hover: h(c.surface_hover),
        edge: h(c.edge),
        amber: h(c.warn_icon),
        amber_text: h(c.warn_ink),
        amber_surface: h(c.warn_surface),
        amber_surface_hover: h(c.warn_surface_hover),
        amber_edge: h(c.warn_edge),
    }
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
    /// The line it answers, quoted over it.
    pub quote: Option<Quote>,
    /// "Reply" shows on hover and in its menu: you may write where it is.
    pub can_reply: bool,
}

/// Over a reply: the line it answers, as this device opened it (the web's `ReplyQuote`).
#[derive(Clone, PartialEq, Default)]
pub struct Quote {
    /// The answered line's record, which clicking the quote brings into view.
    pub seq: i64,
    /// Who wrote it and their name as shown; None when it's not here or was deleted.
    pub who: Option<(Option<pb::User>, String)>,
    /// One line of what it said, or (`missing`) that it's gone or not on this device.
    pub text: String,
    pub missing: bool,
}

/// The quote of the line at `seq`, from a conversation's lines by record.
fn quote_of(seq: i64, by_seq: &HashMap<i64, &Item>, who: &dyn Fn(&str) -> Who) -> Quote {
    match by_seq.get(&seq) {
        None => Quote { seq, who: None, text: t("dms-calls.dm.reply.missing"), missing: true },
        Some(item) if item.deleted => Quote { seq, who: None, text: t("dms-calls.dm.deleted"), missing: true },
        Some(item) => {
            let author = who(&item.sender_id);
            Quote { seq, who: Some((author.user, author.name)), text: quote_text(item), missing: false }
        }
    }
}

/// What a quote shows of a line: one line of its text (or what it carries), without Markdown's marks.
fn quote_text(item: &Item) -> String {
    let text = if let Some(voice) = &item.voice {
        let secs = voice.duration_ms / 1000;
        let length = format!("{}:{:02}", secs / 60, secs % 60);
        t_with("system.e2ee.voiceLine", &[("length", Arg::Str(&length))])
    } else if item.content.trim().is_empty() && !item.files.is_empty() {
        match item.files.as_slice() {
            [one] => t_with("system.e2ee.lineFile", &[("name", Arg::Str(&one.name))]),
            many => t_with("system.e2ee.lineFiles", &[("count", Arg::Num(many.len() as i64))]),
        }
    } else {
        crate::core::spoilers::hide(&item.content)
    };
    let plain: String = text.chars().filter(|c| !matches!(c, '*' | '_' | '~' | '`' | '>' | '#' | '|')).collect();
    plain.split_whitespace().collect::<Vec<_>>().join(" ")
}

thread_local! {
    /// The line the next message answers, per place (a conversation, a secure
    /// channel, or a secure thread's `{id}#{parent}`). Only on this device
    /// until it's sent, and then only inside the encryption.
    static REPLYING: RefCell<HashMap<String, i64>> = RefCell::new(HashMap::new());
}

/// The line a place's next message answers, or 0.
pub(crate) fn replying(place: &str) -> i64 {
    REPLYING.with(|r| r.borrow().get(place).copied().unwrap_or(0))
}

/// Starts answering the line at `seq` from a place's box, or stops (0).
pub(crate) fn set_replying(place: &str, seq: i64) {
    REPLYING.with(|r| {
        let mut r = r.borrow_mut();
        if seq > 0 {
            r.insert(place.to_owned(), seq);
        } else {
            r.remove(place);
        }
    });
}

/// The line a place's next message answers, no longer waiting once it's taken.
pub(crate) fn take_replying(place: &str) -> i64 {
    REPLYING.with(|r| r.borrow_mut().remove(place).unwrap_or(0))
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
    let shown = timestamp_nodes(&hard_breaks(&crate::ui::spoilers::spoiler_nodes(&images_as_links(&content), &id)));
    Msg {
        shown,
        id,
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
        can_hide_previews: false,
        from: None,
        keep_out: false,
        keeping_out: false,
        poll: None,
        voice: None,
        attachments: Vec::new(),
        gif: None,
        gacha: None,
        thread: ThreadBits::default(),
        agent: None,
        pinned: false,
        reactions: None,
        can_pin: false,
        decoration: None,
        owner: false,
        enc: Some(Rc::new(enc)),
        sig: 0,
        pre: 0,
    }
}

/// An encrypted message's text with the server emoji it carried drawn as
/// their pictures; any other token shows as its `:name:`.
fn carried_shown(m: &mut Msg, emojis: impl IntoIterator<Item = (String, String)>) {
    if m.content.contains('<') {
        m.shown = crate::ui::mentions::emoji_links(&m.shown, emojis.into_iter().collect());
    }
}

/// What you're sending writes of your servers' emoji, to draw them before it's read back.
fn sending_emojis(i: &InstanceState, text: &str) -> Vec<(String, String)> {
    crate::core::emoji::carried(i, text).into_iter().map(|e| (e.id, e.url)).collect()
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
/// keeps pins; `describe` puts a line in words. `react` is Some where messages
/// have reactions (whether you may add one).
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
    react: Option<bool>,
    can_reply: bool,
) -> Vec<Row> {
    let me = i.me.clone();
    let me_id = me.as_ref().map(|m| m.id.clone()).unwrap_or_default();
    let stored = i.dms.items.get(id);
    let items = match &lines {
        Some(l) => l.items,
        None => stored.map(|v| v.as_slice()).unwrap_or_default(),
    };
    let in_thread = lines.as_ref().is_some_and(|l| l.in_thread);
    // Every line opened here, by record, for the quotes over replies.
    let by_seq: HashMap<i64, &Item> = stored.map(|v| v.iter().map(|i| (i.seq, i)).collect()).unwrap_or_default();
    let mut rows = vec![start];
    // Still joining (or nothing opened yet): a key that wobbles under the start.
    if !in_thread && (stored.is_none() || i.dms.joining.contains(id)) {
        let text = t(if moderate.is_some() { "chat.secure.joining" } else { "dms-calls.dm.view.joining" });
        rows.push(Row::Line(Rc::new(SysLine { id: "joining".into(), kind: LineKind::Joining, text, at: 0 })));
    }
    let mut days = crate::ui::text::Days::default();
    let mut day = |rows: &mut Vec<Row>, at: i64, id: String| {
        if days.starts(at) {
            rows.push(Row::Day { id: format!("day|{id}"), text: crate::ui::text::day(at) });
        }
    };
    // Each sender as shown, looked up once.
    let people: RefCell<HashMap<String, Who>> = RefCell::default();
    let who_of = |user: &str| -> Who {
        if let Some(w) = people.borrow().get(user) {
            return w.clone();
        }
        let w = who(user);
        people.borrow_mut().insert(user.to_owned(), w.clone());
        w
    };
    // The messages built last time for this list (`BUILT`), and those built now.
    let place_key = lines.as_ref().map_or(id, |l| l.place.as_str()).to_owned();
    let mut kept = BUILT.with_borrow_mut(|b| b.remove(&place_key)).unwrap_or_default();
    let mut built = crate::ui::chat::Built::default();
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
        let author = who_of(&item.sender_id);
        let key = item.seq.to_string();
        let editing = editing == Some(key.as_str());
        let can_delete = !item.deleted && (mine || moderate == Some(true));
        let pinned = pins.filter(|_| !item.deleted).map(|pins| pins.iter().any(|p| p.sequence == item.seq));
        let reactions = react.filter(|_| !item.deleted).map(|can_add| {
            let name_of = |u: &str| who_of(u).name;
            Rc::new(crate::ui::reactions::ReactBits::encrypted_item(i, id, item, can_add, &name_of))
        });
        let quote = (item.reply_to > 0).then(|| quote_of(item.reply_to, &by_seq, &who_of));
        // Everything the row is made from, so an unchanged line is reused rather than built again.
        let sig = {
            let mut h = std::hash::DefaultHasher::new();
            item_digest(item, &mut h);
            (&author.name, author.color.map(|c| [c.h, c.s, c.l, c.a].map(f32::to_bits))).hash(&mut h);
            author.user.as_ref().map(prost::Message::encode_to_vec).hash(&mut h);
            (mine, editing, can_delete, pinned, question, can_reply).hash(&mut h);
            quote.as_ref().map(|q| (&q.text, q.missing, q.who.as_ref().map(|w| &w.1))).hash(&mut h);
            if let Some(r) = &reactions {
                r.digest(&mut h);
            }
            // Never 0, which means "not kept".
            h.finish() | 1
        };
        let m = match kept.remove(&key) {
            Some(was) if was.sig == sig => was,
            _ => {
                let enc = EncBits {
                    deleted: item.deleted,
                    shared: !item.shared_by.is_empty(),
                    question: question.to_owned(),
                    files: if item.deleted { Vec::new() } else { item.files.clone() },
                    sending: Vec::new(),
                    quote,
                    can_reply: can_reply && !item.deleted,
                };
                let mut m = plain_msg(key.clone(), author, item.content.clone(), item.at, mine, enc);
                carried_shown(&mut m, item.emojis.iter().map(|e| (e.id.clone(), e.url.clone())));
                m.editing = editing;
                m.edited = item.edited_at > 0;
                m.can_delete = can_delete;
                m.voice = item.voice.as_ref().map(|v| Rc::new(crate::ui::voice_notes::VoiceCard::of(v)));
                if let Some(pinned) = pinned {
                    m.can_pin = true;
                    m.pinned = pinned;
                }
                m.reactions = reactions;
                m.sig = sig;
                Rc::new(m)
            }
        };
        let m = match &lines {
            // What a list adds isn't in the signature: the row is told apart by its content.
            Some(l) => {
                let mut dressed = (*m).clone();
                (l.dress)(item, &mut dressed);
                dressed.sig = 0;
                built.insert(key, m);
                Rc::new(dressed)
            }
            None => {
                built.insert(key, m.clone());
                m
            }
        };
        rows.push(Row::Msg(m));
    }
    let shows = |p: &crate::core::dms::DmPending| lines.as_ref().is_none_or(|l| (l.pending)(p));
    for pending in i.dms.sending.get(id).into_iter().flatten().filter(|p| shows(p)) {
        let who = Who { name: me.as_ref().map(user_name).unwrap_or_default(), color: None, user: me.clone() };
        let quote = (pending.reply_to > 0).then(|| quote_of(pending.reply_to, &by_seq, &who_of));
        let enc = EncBits { question: question.to_owned(), quote, ..Default::default() };
        let mut m = plain_msg(format!("p{}", pending.nonce), who, pending.text.clone(), pending.created_at, true, enc);
        carried_shown(&mut m, sending_emojis(i, &pending.text));
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
        let sending = sending_emojis(i, &m.content);
        carried_shown(&mut m, sending);
        m.can_delete = false;
        m.pending = true;
        rows.push(Row::Msg(Rc::new(m)));
    }
    // One person's messages close together share a header, as in a channel.
    crate::ui::chat::group(&mut rows);
    if lines.is_none() {
        crate::ui::chat::keep_grouped(&rows, &mut built);
    }
    drop(kept);
    BUILT.with_borrow_mut(|b| {
        // A few lists at a time: the open one, its thread, one left a moment ago.
        if b.len() >= 4 {
            b.clear();
        }
        b.insert(place_key, built);
    });
    rows
}

thread_local! {
    /// Each encrypted list's messages as last built, by sequence, so a new
    /// line doesn't redo the conversation.
    static BUILT: RefCell<HashMap<String, crate::ui::chat::Built>> = RefCell::new(HashMap::new());
}

/// What an encrypted line shows of itself.
fn item_digest(item: &Item, h: &mut impl Hasher) {
    (item.seq, item.at, &item.sender_id, &item.content, item.edited_at, item.deleted, &item.shared_by).hash(h);
    for f in &item.files {
        (&f.media_id, &f.name, &f.content_type, f.size, f.file_size, f.width, f.height).hash(h);
    }
    item.voice.as_ref().map(|v| (&v.media_id, v.duration_ms, &v.waveform)).hash(h);
    (item.thread, item.reply_to).hash(h);
    item.emojis.iter().for_each(|e| (&e.id, &e.url).hash(h));
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
        let items = i.dms.items.get(conversation).map(|v| v.as_slice()).unwrap_or_default();
        let locked = crate::core::backup::state(&self.target().map(|t| t.key().to_owned()).unwrap_or_default()).status
            == crate::core::backup::Status::Locked;
        let earlier = earlier_from(items, locked);
        let name_of = |id: &str| who(id).name;
        let describe = |item: &Item| device_line(item, &name_of, &me, earlier);
        let pins = i.has("pins").then(|| i.dms.pins.get(conversation).map(|l| l.pins.as_slice()).unwrap_or_default());
        let question = t("dms-calls.dm.view.deleteQuestion");
        let react = i.has("reactions").then(|| !i.dms.blocked.contains_key(conversation));
        encrypted_rows(
            i,
            conversation,
            start,
            &who,
            &describe,
            None,
            self.editing.as_deref(),
            pins,
            &question,
            None,
            react,
            true,
        )
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
                    .bg(s.pill)
                    .shadow(ring_of(card, 4.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(s.surface)
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
                    .bg(s.surface)
                    .border_1()
                    .border_color(s.edge)
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
                    .bg(s.surface)
                    .border_1()
                    .border_color(s.edge)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(looping(
                        div().child(icon("key-round").size(px(16.0)).text_color(s.icon)),
                        "joining-key",
                        Duration::from_millis(1200),
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

/// A loop ([`motion::ambient`]: still while the window's in the background)
/// for places drawn without the window at hand.
#[derive(IntoElement)]
pub(crate) struct Looping {
    el: Div,
    id: SharedString,
    period: Duration,
    pose: fn(Div, f32) -> Div,
}

/// `el` posed by `pose` over and over, every `period`.
pub(crate) fn looping(el: Div, id: impl Into<SharedString>, period: Duration, pose: fn(Div, f32) -> Div) -> Looping {
    Looping { el, id: id.into(), period, pose }
}

impl RenderOnce for Looping {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        motion::ambient(self.el, self.id, self.period, window, self.pose)
    }
}

/// A button's icon that pops to its new look when it changes (the web's
/// `AnimatePresence` swaps: a tick after copying, a spinner while busy), and
/// turns while it's the spinner. Nothing moves when it first shows.
#[derive(IntoElement)]
pub(crate) struct GlyphSwap {
    id: SharedString,
    glyph: &'static str,
    size: f32,
    color: Option<Hsla>,
    /// How small it starts, and its spring (stiffness, damping).
    from: f32,
    spring: (f32, f32),
}

/// `glyph` at `size`, popping in from 40% on framer's default spring when it changes.
pub(crate) fn glyph_swap(id: impl Into<SharedString>, glyph: &'static str, size: f32) -> GlyphSwap {
    GlyphSwap { id: id.into(), glyph, size, color: None, from: 0.4, spring: (500.0, 25.0) }
}

impl GlyphSwap {
    pub fn color(mut self, color: impl Into<Hsla>) -> Self {
        self.color = Some(color.into());
        self
    }

    /// Starts at `from` of its size, on a spring of (stiffness, damping).
    pub fn pop(mut self, from: f32, spring: (f32, f32)) -> Self {
        (self.from, self.spring) = (from, spring);
        self
    }
}

impl RenderOnce for GlyphSwap {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state =
            window.use_keyed_state(SharedString::from(format!("{}|glyph", self.id)), cx, |_, _| (self.glyph, 0u64));
        let changes = state.update(cx, |(was, changes), _| {
            if *was != self.glyph {
                *was = self.glyph;
                *changes += 1;
            }
            *changes
        });
        let el = div()
            .flex_none()
            .child(icon(self.glyph).size(px(self.size)).when_some(self.color, |el, c| el.text_color(c)));
        let el = if matches!(self.glyph, "loader" | "loader-circle") {
            let spin = SharedString::from(format!("{}|spin", self.id));
            div().child(motion::ambient(el, spin, Duration::from_millis(1000), window, |el, t| {
                el.rotate(gpui_kit::percentage(t))
            }))
        } else {
            el
        };
        if changes == 0 {
            return el.into_any_element();
        }
        let from = self.from;
        motion::spring_in(
            el,
            SharedString::from(format!("{}|in{changes}", self.id)),
            self.spring,
            Duration::ZERO,
            move |el, t| el.opacity(t.clamp(0.0, 1.0)).scale(from + (1.0 - from) * t),
        )
    }
}

/// The trust pill coming in (the web's `initial={{ opacity: 0, scale: 0.8 }}` on `SPRING`).
pub(crate) fn pill_in(pill: Stateful<Div>, id: &str) -> AnyElement {
    motion::spring_in(
        div().flex_none().child(pill),
        SharedString::from(format!("{id}-in")),
        (520.0, 34.0),
        Duration::ZERO,
        |el, t| el.opacity(t.clamp(0.0, 1.0)).scale(0.8 + 0.2 * t),
    )
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
    let (bg, hover, fg, edge) = match trust {
        Trust::Changed => (s.amber_surface, s.amber_surface_hover, s.amber_text, s.amber_edge),
        _ => (s.surface, s.surface_hover, s.pill, s.edge),
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
        .border_1()
        .border_color(edge)
        .shadow(vec![BoxShadow {
            color: gpui_kit::hsla(0.0, 0.0, 0.0, 0.12),
            offset: point(px(0.0), px(1.0)),
            blur_radius: px(2.0),
            spread_radius: px(0.0),
            inset: false,
        }])
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
        let dot = partner.as_ref().and_then(|u| {
            self.core.shared.read(|s| s.instance(key).and_then(|i| crate::ui::presence::partner_status(i, &u.id)))
        });
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
            .child(partner_name(partner.as_ref(), dot, &p, window, cx))
            .child(div().flex_1())
            .when(conversation.is_some() && ready && pins_here, |el| {
                let open = self.pins.is_some();
                el.child(self.pins_button_in("pins-toggle", open, window, cx))
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
                el.child(pill_in(
                    trust_pill("dm-trust", trust, label, &p)
                        .tooltip(|window, cx| {
                            crate::ui::overlay::Tip::new(t("dms-calls.dm.trust.title")).build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let dialog = crate::ui::app::Dialog::Safety { key: key.clone(), conversation: id.clone() };
                            this.open_dialog(dialog, window, cx)
                        })),
                    "dm-trust",
                ))
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
                    drop_to: Some(t_with(
                        "chat.files.dropToPerson",
                        &[(
                            "name",
                            Arg::Str(&format!(
                                "@{}",
                                partner.as_ref().map_or_else(|| t("dms-calls.dm.view.untitled"), user_name)
                            )),
                        )],
                    )),
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
/// `dot`: their presence dot, while they show one.
fn partner_name(
    partner: Option<&pb::User>,
    dot: Option<pb::PresenceStatus>,
    p: &Palette,
    window: &mut Window,
    cx: &mut gpui_kit::App,
) -> AnyElement {
    let key = partner.map_or_else(|| "none".to_owned(), |u| u.id.clone());
    let name = partner.map_or_else(|| t("dms-calls.dm.view.untitled"), user_name);
    let face =
        div().relative().flex_none().child(avatar(partner, 32.0, p)).when_some(dot, |el, status| {
            el.child(crate::ui::presence::avatar_dot(status, 11.2, 3.0, p.card.into(), p))
        });
    motion::rise(
        div().flex().min_w_0().flex_shrink(1.0).items_center().gap(px(10.0)).child(face).child(
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
                        .bg(s.surface)
                        .border_1()
                        .border_color(s.edge)
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
    /// Where files dropped on the window go, said over it ("Drop to send in …"), if they come here.
    pub drop_to: Option<String>,
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
    /// Where the open conversation's or secure channel's box answers from: the
    /// list's own, or (`in_thread`) the open secure thread's.
    fn reply_place(&self, in_thread: bool) -> Option<String> {
        if in_thread {
            let (_, _, channel, parent) = crate::ui::secure_threads::open_thread(self)?;
            return Some(crate::ui::secure_threads::place_of(&channel, parent));
        }
        match self.target()? {
            Target::Dm { conversation: id, .. } | Target::Secure { channel: id, .. } => Some(id),
            Target::Channel { .. } => None,
        }
    }

    /// Answers a line of the open conversation or secure channel (`id` is its
    /// record, or a secure thread's top copy) from the box under it, quoting it.
    pub(crate) fn start_reply(&mut self, id: &str, in_thread: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(seq) = id.strip_prefix("top|").unwrap_or(id).parse::<i64>() else { return };
        let Some(place) = self.reply_place(in_thread) else { return };
        set_replying(&place, seq);
        let input = if in_thread { self.threads.reply.clone() } else { self.composer.clone() };
        input.update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }

    /// Stops answering a line from the focused box; false when it wasn't answering one.
    pub(crate) fn cancel_reply_here(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
        let in_thread = self.threads.reply.read(cx).focus_handle(cx).is_focused(window);
        if !in_thread && !self.composer.read(cx).focus_handle(cx).is_focused(window) {
            return false;
        }
        let Some(place) = self.reply_place(in_thread) else { return false };
        if replying(&place) == 0 {
            return false;
        }
        set_replying(&place, 0);
        cx.notify();
        true
    }

    /// Brings a line of the open list into view and lets it glow (a reply's
    /// quote, clicked); in a secure thread's panel, it glows where it is.
    pub(crate) fn jump_to_line(&mut self, seq: i64, in_thread: bool, cx: &mut Context<Self>) {
        let id = seq.to_string();
        self.search.jumped = Some((id.clone(), std::time::Instant::now()));
        if !in_thread {
            self.sync_list(cx);
            if let Some(ix) = self.rows.iter().position(|r| r.id_str() == id) {
                self.scroller.update(cx, |s, cx| s.scroll_to_item(ix, cx));
            }
        }
        cx.notify();
    }

    /// Over the box while it answers a line (the web's `ReplyBar`): whose, and the × that stops. Esc stops too.
    fn reply_bar(&self, c: &Composer, place: &str, p: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let seq = replying(place);
        if seq == 0 {
            return None;
        }
        let target = self.target();
        let name = self.core.shared.read(|s| {
            let i = s.instance(&c.key)?;
            let sender = i.dms.items.get(&c.id)?.iter().find(|i| i.seq == seq)?.sender_id.clone();
            Some(match &target {
                Some(Target::Secure { server, .. }) => i.display_name(Some(server), &sender),
                _ => i
                    .dms
                    .conversations
                    .iter()
                    .find(|v| v.id == c.id)
                    .and_then(|v| v.users.iter().find(|u| u.id == sender))
                    .or_else(|| i.users.get(&sender))
                    .map(user_name)
                    .unwrap_or_else(|| "Someone".into()),
            })
        });
        let label: AnyElement = match name {
            Some(name) => crate::ui::text::hint_line(
                &t_with("dms-calls.dm.reply.to", &[("name", Arg::Str("{name}"))]),
                &[("name", &name)],
                p,
            )
            .into_any_element(),
            None => div().child(t("dms-calls.dm.reply.toLine")).into_any_element(),
        };
        let place = place.to_owned();
        Some(
            motion::rise(
                div()
                    .mb(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(12.0))
                    .py(px(6.0))
                    .rounded(radius_xl())
                    .bg(alpha(p.muted, 0.6))
                    .text_xs()
                    .line_height(px(16.0))
                    .text_color(p.muted_foreground)
                    .child(icon("reply").size(px(14.0)))
                    .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(label))
                    .child(
                        div()
                            .id(SharedString::from(format!("reply-cancel|{place}")))
                            .size(px(20.0))
                            .flex_none()
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover({
                                let (bg, fg) = (p.muted, p.foreground);
                                move |st| st.bg(bg).text_color(fg)
                            })
                            .tooltip(|window, cx| {
                                crate::ui::overlay::Tip::new(t("dms-calls.dm.reply.cancel")).build(window, cx)
                            })
                            .on_click(cx.listener(move |_, _, _, cx| {
                                set_replying(&place, 0);
                                cx.notify();
                            }))
                            .child(icon("x").size(px(14.0))),
                    ),
                SharedString::from(format!("reply-bar|{}|{seq}", c.id)),
                Duration::ZERO,
                8.0,
            )
            .into_any_element(),
        )
    }

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
        let (send, line) = match self.core.prefs_now().send_with {
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
                    .px(px(6.0))
                    .rounded_full()
                    .bg(s.surface)
                    .border_1()
                    .border_color(s.edge)
                    .font_weight(FontWeight::BOLD)
                    .text_color(s.pill)
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
        let time_panel = if thread == crate::ui::secure_threads::time_in_thread() {
            self.time_picker_leaving(&p, window, cx)
        } else {
            None
        };
        let recording = !thread && self.recording_here();
        // Voice messages only in conversations: the microphone takes send's place while nothing's typed.
        let voice_here = !thread && matches!(self.target(), Some(Target::Dm { .. }));
        let voice =
            (voice_here && (recording || (!typed && !picked && self.can_record()))).then(|| self.voice_button(&p, cx));
        let attach = (c.files && !recording).then(|| self.encrypted_attach(&place, ready, &p, cx));
        // GIFs go sealed like files, from the conversation's or channel's own box.
        let gif_button = (c.files && ready && !thread && !recording).then(|| self.gif_button(&p, cx)).flatten();
        let gif_panel = if thread { None } else { self.gif_panel(&p, window, cx) };
        let tray = if c.files { self.picked_tray(&place, &p, cx) } else { None };
        let field: AnyElement = if recording {
            self.recording_bar(&p, cx)
        } else {
            // Pasted pictures and files join the ones picked, sealed on this device like them.
            let mut box_ = Textarea::new(&input).appearance(false).text_size(px(15.2)).line_height(px(24.0));
            if c.files && ready {
                box_ = box_.on_paste(self.paste_to_sealed(place.clone(), cx));
            }
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
                .child(box_)
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
            .children(if recording { None } else { self.chars_left_in(length, &p, window, cx) })
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
            .children(gif_button)
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
        // Who's typing here: a secure channel's thread goes by its parent record's sequence.
        let typing = match self.target() {
            Some(Target::Secure { server, .. }) => {
                let at = c.thread.as_ref().map_or(String::new(), |(parent, _)| parent.to_string());
                Some((crate::core::typing::Place::channel(&c.key, &c.id, &at), Some(server)))
            }
            _ if thread => None,
            Some(Target::Dm { .. }) => Some((crate::core::typing::Place::dm(&c.key, &c.id), None)),
            _ => None,
        };
        let typing = self.typing_line(&place, typing, &p, window, cx);
        let replying = self.reply_bar(&c, &place, &p, cx);
        // Files dragged over the window go with the next message here, as on the web: not
        // from a thread's box, whose channel's box takes them.
        let drop = match &c.drop_to {
            Some(heading) if c.files && ready && !recording && self.dialog.is_none() => {
                let place = place.clone();
                Some(self.drop_sheet(
                    heading.clone(),
                    t("dms-calls.dm.composer.drop"),
                    &p,
                    window,
                    cx,
                    move |this, paths, cx| this.add_sealed_files(place.clone(), paths, cx),
                ))
            }
            _ => None,
        };
        div()
            .flex_none()
            .relative()
            .px(px(16.0))
            .pb(px(12.0))
            .children(typing)
            .children(time_panel)
            .children(replying)
            .children(gif_panel)
            .child(self.shaken(card))
            .child(footer)
            .children(drop)
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
        i.dms.items.insert("c".into(), std::sync::Arc::new(vec![item(1), item(2), item(3)]));
        let who = |_: &str| Who { name: "U".into(), color: None, user: None };
        let start = || Row::Older { loading: false };
        let marks = |pins: Option<&[pb::DmPin]>| {
            encrypted_rows(&i, "c", start(), &who, &|_| String::new(), None, None, pins, "", None, None, true)
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
