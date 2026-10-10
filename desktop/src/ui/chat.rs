//! The middle of the window: a channel or a private conversation (its
//! messages and the composer), the member list, an instance's page, or home.

use crate::ui::emoji::InColor as _;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash as _, Hasher as _};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::component::input::Textarea;
use gpui_kit::component::message_scroller::MessageScroller;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Div, Focusable as _, FontWeight, Hsla, InteractiveElement as _,
    IntoElement, MouseButton, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _,
    WeakEntity, Window, div, px, rgb,
};

use crate::core::config::Density;
use crate::core::i18n::{Arg, t};
use crate::core::shared;
use crate::core::store::InstanceState;
use crate::pb;
use crate::ui::app::{Dialog, FuwaApp, Nav, Target};
use crate::ui::context_menu::{self, MenuOf};
use crate::ui::members::{MembersEvent, MembersView};
use crate::ui::mentions::{Look, SCHEME, mention_links};
use crate::ui::motion;
use crate::ui::text::{TIGHT, clock, images_as_links, ms_of, tracked, when};
use crate::ui::theme::{Palette, alpha, corner, mix};
use crate::ui::timestamps::timestamp_nodes;
use crate::ui::widgets::{
    app_badge, avatar, fuwa_mark, header_button, icon, icon_button, is_agent, pal, primary_button, soft_button,
};

/// Who wrote something in a conversation or a secure channel, as shown.
pub(crate) struct Who {
    pub name: String,
    pub color: Option<Hsla>,
    pub user: Option<pb::User>,
}

/// Why you can't write here, and what would let you.
pub struct Blocked {
    pub text: String,
    pub action: Option<(&'static str, Dialog)>,
}

/// The rows (as runs, in the new list) whose digest differs from the row
/// that was `shift` places earlier before: the ones to measure again.
fn changed_rows(before: &[u64], after: &[u64], shift: usize) -> Vec<std::ops::Range<usize>> {
    let mut runs: Vec<std::ops::Range<usize>> = Vec::new();
    for (n, was) in before.iter().enumerate() {
        let at = n + shift;
        if after.get(at).is_some_and(|now| now != was) {
            match runs.last_mut() {
                Some(run) if run.end == at => run.end = at + 1,
                _ => runs.push(at..at + 1),
            }
        }
    }
    runs
}

/// The open channel's messages as built for the list, by id, with a
/// signature of everything each was built from: a message whose signature
/// hasn't changed is reused rather than worked out again.
pub type Built = HashMap<String, Rc<Msg>>;

/// How a floating panel looks once it has gone (the web's `exit`): scaled to
/// `scale` around `origin` (fractions of its size) and moved `(x, y)` pixels.
#[derive(Clone, Copy)]
pub(crate) struct Gone {
    pub scale: f32,
    pub x: f32,
    pub y: f32,
    pub origin: (f32, f32),
}

/// A floating panel that has closed, `t` of the way out ([`motion::kept`]):
/// it fades toward `gone`, and a cover over it takes the pointer meanwhile so
/// nothing in it can be pressed as it goes. It adds no element id, so what's
/// in it keeps its state (an entrance that has played stays played).
pub(crate) fn closing<E: gpui_kit::Styled + gpui_kit::ParentElement>(el: E, t: f32, gone: Gone) -> E {
    let e = gpui_kit::ease_out_quint()(t.clamp(0.0, 1.0));
    el.opacity(1.0 - e)
        .transform_origin(gone.origin.0, gone.origin.1)
        .scale(1.0 + (gone.scale - 1.0) * e)
        .translate_x(px(gone.x * e))
        .translate_y(px(gone.y * e))
        .child(div().absolute().inset_0().occlude())
}

/// Messages from the same person this close together sit under one header.
const GROUP_MS: i64 = 7 * 60 * 1000;

#[derive(Clone)]
pub enum Row {
    /// More history above: a button that loads it.
    Older {
        loading: bool,
    },
    /// The top of a channel or a conversation.
    Start {
        icon: &'static str,
        title: String,
        body: String,
        /// A shared channel's note under it, until closed: its key in prefs, a title and a line.
        shared: Option<(String, String, String)>,
    },
    /// Something that happened, not something said.
    Note {
        id: String,
        icon: &'static str,
        text: String,
    },
    /// Between a thread's message and its replies.
    Divider {
        text: String,
    },
    /// A line between days: "Today", "Yesterday", "Monday, June 3".
    Day {
        id: String,
        text: String,
    },
    /// Someone joined the server, with a wave for them.
    Join(Rc<JoinLine>),
    /// What AutoMod caught, for the moderators in its alert channel.
    AutoMod(Rc<AlertLine>),
    Msg(Rc<Msg>),
    /// The top of a private conversation.
    DmStart(Rc<crate::ui::dm_view::DmStart>),
    /// A line about an encrypted conversation's devices, not something said.
    Line(Rc<crate::ui::dm_view::SysLine>),
}

/// "Someone joined", as the web's `JoinRow`.
#[derive(Clone, PartialEq)]
pub struct JoinLine {
    pub id: String,
    pub user: Option<pb::User>,
    pub user_id: String,
    pub name: String,
    pub color: Option<Hsla>,
    pub at: i64,
    pub mine: bool,
    /// You may send here, so you may wave.
    pub can_wave: bool,
    pub can_delete: bool,
}

/// What AutoMod caught, as the web's `AutoModAlertRow`.
#[derive(Clone, PartialEq)]
pub struct AlertLine {
    pub id: String,
    pub alert: pb::AutoModAlert,
    pub user_id: String,
    pub name: String,
    pub color: Option<Hsla>,
    /// The channel it happened in, when it's known.
    pub channel: Option<String>,
    pub at: i64,
    pub can_delete: bool,
}

/// What the message list keeps between frames.
#[derive(Default)]
pub struct MsgUi {
    /// The message (or join line, or alert) asking "Delete?".
    pub deleting: Option<String>,
    /// The message whose text was just copied, and when.
    pub copied: Option<(String, Instant)>,
    /// New messages that came in while the list was scrolled up.
    pub missed: usize,
    /// Join lines waved at this run, and the waves on their way.
    pub waved: std::collections::HashSet<String>,
    pub waving: std::collections::HashSet<String>,
    /// When older messages were last asked for, so a failing load isn't asked again every frame.
    pub older_at: Option<Instant>,
    /// The row under the pointer, and the row whose tools' card is (it pokes out above its row).
    pub hovered: Option<String>,
    pub hovered_tools: Option<String>,
    /// The bell menu's "Mute channel" list is out.
    pub bell_sub: bool,
}

/// Where a message stands with threads.
#[derive(Clone, Default, PartialEq)]
pub struct ThreadBits {
    /// The replies under it, in the channel.
    pub replies: Option<Rc<crate::ui::threads::Replies>>,
    /// In the channel, a reply also sent there: the thread it's in.
    pub also_in: Option<String>,
    /// In its thread, a reply also sent to the channel.
    pub also_sent: bool,
    /// "Reply in thread" shows on hover.
    pub can_thread: bool,
}

impl ThreadBits {
    fn digest(&self, h: &mut DefaultHasher) {
        (&self.also_in, self.also_sent, self.can_thread).hash(h);
        if let Some(r) = &self.replies {
            (r.count, r.new, r.locked, r.archived, r.last_at).hash(h);
            r.faces.iter().map(|u| (&u.id, &u.avatar_url)).for_each(|f| f.hash(h));
        }
    }
}

#[derive(Clone)]
pub struct Msg {
    /// The message's id, a private message's sequence, or `p{nonce}` while sending.
    pub id: String,
    pub user: Option<pb::User>,
    pub name: String,
    pub color: Option<Hsla>,
    /// What was written, as it's edited.
    pub content: String,
    /// What's drawn: Markdown with mentions as links and pictures as links.
    pub shown: String,
    /// It pings you: by name, a role of yours, or @everyone.
    pub mentions_me: bool,
    pub editing: bool,
    /// Yours, or you may manage messages here.
    pub can_delete: bool,
    pub at: i64,
    pub edited: bool,
    pub head: bool,
    pub mine: bool,
    pub pending: bool,
    pub failed: Option<String>,
    pub nonce: u64,
    pub unreadable: bool,
    /// Not a person: "APP" for what a webhook posted, "BOT" for AutoMod.
    pub badge: Option<&'static str>,
    /// Cards an app posted with it.
    pub embeds: Vec<pb::Embed>,
    /// In a shared channel: the server the author is from, when it isn't this one.
    pub from: Option<pb::SharedServer>,
    /// At a shared channel's home, with Kick Members: this author can be kept out of it.
    pub keep_out: bool,
    /// Asking whether to keep this author out.
    pub keeping_out: bool,
    /// The poll it is, as its card draws it.
    pub poll: Option<Rc<crate::ui::polls::PollCard>>,
    /// The voice message it is, in a private conversation.
    pub voice: Option<Rc<crate::ui::voice_notes::VoiceCard>>,
    /// The files it came with.
    pub attachments: Vec<pb::Attachment>,
    /// The GIF it is.
    pub gif: Option<pb::MessageGif>,
    /// Its thread, or the thread it's in.
    pub thread: ThreadBits,
    /// An agent's buttons, and over its answer who used what.
    pub agent: Option<Rc<crate::ui::commands::AgentBits>>,
    /// Pinned in its channel or thread.
    pub pinned: bool,
    /// You may pin it or unpin it.
    pub can_pin: bool,
    /// The decoration around the author's avatar, its picture's link (docs/profile-items.md).
    pub decoration: Option<SharedString>,
    /// Its author owns the server: a crown by the name.
    pub owner: bool,
    /// In a conversation or a secure channel: what its encryption adds.
    pub enc: Option<Rc<crate::ui::dm_view::EncBits>>,
    /// Its reactions, where messages here can have them (`ui::reactions`).
    pub reactions: Option<Rc<crate::ui::reactions::ReactBits>>,
    /// What it was built from (0 when it isn't kept between changes).
    pub sig: u64,
}

impl Row {
    fn id(&self) -> String {
        self.id_str().to_owned()
    }

    pub(crate) fn id_str(&self) -> &str {
        match self {
            Row::Older { .. } => "older",
            Row::Start { .. } => "start",
            Row::Note { id, .. } => id,
            Row::Divider { .. } => "divider",
            Row::Day { id, .. } => id,
            Row::Join(j) => &j.id,
            Row::AutoMod(a) => &a.id,
            Row::Msg(m) => &m.id,
            Row::DmStart(_) => "start",
            Row::Line(l) => &l.id,
        }
    }

    fn digest(&self, h: &mut DefaultHasher) {
        match self {
            Row::Older { loading } => ("older", loading).hash(h),
            Row::Start { title, body, shared, .. } => ("start", title, body, shared).hash(h),
            Row::Note { id, text, .. } => (id, text).hash(h),
            Row::Divider { text } => ("divider", text).hash(h),
            Row::Day { id, text } => (id, text).hash(h),
            Row::Join(j) => {
                (&j.id, &j.name, j.can_wave, j.can_delete).hash(h);
                j.color.map(|c| [c.h, c.s, c.l, c.a].map(f32::to_bits)).hash(h);
                j.user.as_ref().map(|u| &u.username).hash(h);
            }
            Row::AutoMod(a) => {
                (&a.id, &a.name, &a.channel, a.can_delete).hash(h);
                a.color.map(|c| [c.h, c.s, c.l, c.a].map(f32::to_bits)).hash(h);
            }
            Row::DmStart(s) => ("dm-start", &s.name, s.user.as_ref().map(|u| (&u.username, &u.avatar_url))).hash(h),
            Row::Line(l) => (&l.id, l.kind, &l.text, l.at).hash(h),
            Row::Msg(m) if m.sig != 0 => (m.sig, m.head).hash(h),
            Row::Msg(m) => {
                (&m.id, &m.shown, m.edited, m.head, m.pending, &m.failed, &m.name, m.editing, m.mentions_me, m.pinned)
                    .hash(h);
                m.voice.as_ref().map(|v| v.digest()).hash(h);
                m.thread.digest(h);
                m.can_delete.hash(h);
                m.enc.as_ref().map(|e| (e.deleted, e.shared, e.files.len(), e.sending.len())).hash(h);
                if let Some(r) = &m.reactions {
                    r.digest(h);
                }
            }
        }
    }
}

impl FuwaApp {
    /// What the open list shows, row by row. `built` keeps the messages
    /// from the last time, so a new message doesn't redo the whole channel.
    fn rows(&self, built: &mut Built) -> Vec<Row> {
        match self.target() {
            Some(Target::Channel { key, server, channel }) => {
                let mut rows = self.core.shared.read(|s| match s.instance(&key) {
                    Some(i) => self.server_rows(i, &key, &server, &channel, None, built),
                    None => Vec::new(),
                });
                self.dress_voice(&mut rows);
                rows
            }
            Some(Target::Dm { key, conversation }) => {
                let mut rows = self
                    .core
                    .shared
                    .read(|s| s.instance(&key).map(|i| self.dm_rows(i, &conversation)).unwrap_or_default());
                self.dress_voice(&mut rows);
                rows
            }
            Some(Target::Secure { key, server, channel }) => self
                .core
                .shared
                .read(|s| s.instance(&key).map(|i| self.secure_rows(i, &key, &server, &channel)).unwrap_or_default()),
            None => Vec::new(),
        }
    }

    /// A server channel's rows, or with `thread` those of the thread under
    /// that message: the message, then its replies.
    pub(crate) fn server_rows(
        &self,
        i: &InstanceState,
        key: &str,
        server: &str,
        channel: &str,
        thread: Option<&str>,
        built: &mut Built,
    ) -> Vec<Row> {
        let (key, server, channel) = (key.to_owned(), server.to_owned(), channel.to_owned());
        let me = i.me.as_ref().map(|m| m.id.clone()).unwrap_or_default();
        let mut rows = Vec::new();
        let at = thread.map_or_else(|| channel.clone(), crate::core::threads::thread_key);
        let Some(loaded) = i.messages.get(&at) else { return rows };
        let here = i.channel(&server, &channel);
        let name = here.map(|c| c.name.clone()).unwrap_or_default();
        // In a shared channel each side moderates its own people: a guest's moderators can't
        // delete the home's, and only the home keeps someone from another server out.
        let shared = here.and_then(|c| c.shared.as_ref());
        let guest_side = shared.is_some_and(|s| !s.home);
        let keeps_out = shared.is_some_and(|s| s.home) && i.access(&server).has(pb::Permission::KickMembers);
        let look = Look::of(i, &server);
        let mut kept = Built::default();
        // Each author as shown (name, colour, badge, picture), looked up once.
        type Author = Rc<(String, Option<Hsla>, Option<&'static str>, Option<pb::User>, Option<SharedString>)>;
        let mut authors: HashMap<String, Author> = HashMap::new();
        let mut author = |id: &str| -> Author {
            authors
                .entry(id.to_owned())
                .or_insert_with(|| {
                    Rc::new((
                        i.display_name(Some(&server), id),
                        i.name_color(&server, id).map(|c| rgb(c).into()),
                        is_agent(i.users.get(id)).then_some("AGENT"),
                        i.users.get(id).cloned(),
                        i.decoration_of(Some(&server), id).map(|u| SharedString::from(u.to_owned())),
                    ))
                })
                .clone()
        };
        let manage = i.access(&server).has_in(&channel, pb::Permission::ManageMessages);
        let owner_id = i.server(&server).map(|s| s.owner_id.clone()).unwrap_or_default();
        let can_send = i.access(&server).has_in(&channel, pb::Permission::SendMessages);
        // Pins are the home's in a shared channel, and need the instance to keep them.
        let pins_here = i.has("pins") && manage && !guest_side;
        let can_vote = !guest_side && !i.access(&server).pending;
        // Reactions need the instance to keep them; in a guest's shared channel only the home clears them.
        let reactions_here = i.has("reactions");
        let timed_out = i
            .my_member(&server)
            .and_then(|m| crate::core::moderation::timed_out_until(m, crate::core::dms::now_ms()))
            .is_some();
        let can_react = reactions_here && {
            let access = i.access(&server);
            access.has_in(&channel, pb::Permission::AddReactions) && (!timed_out || access.owner)
        };
        let now = crate::core::dms::now_ms();
        let suppress = i.effective_notifications(&server, &channel, 0).suppress_everyone;
        // My roles, which decide whether a role mention pings me.
        let mine: Vec<String> = i.my_member(&server).map(|m| m.role_ids.clone()).unwrap_or_default();
        // Threads go under messages in the channel itself, not under replies, and not in shared channels yet.
        let threads_here = thread.is_none() && shared.is_none();
        let can_start = threads_here && i.access(&server).has_in(&channel, pb::Permission::CreateThreads);
        let can_reply = threads_here && i.access(&server).has_in(&channel, pb::Permission::SendMessages);
        // In a thread: the message it's under comes first, then its replies.
        let parent = thread.and_then(|id| {
            i.thread_parents.get(id).or_else(|| i.messages.get(&channel)?.items.iter().find(|m| m.id == id))
        });
        let divider = |rows: &mut Vec<Row>| {
            let count = parent.and_then(|m| m.thread.as_ref()).map_or(0, |t| t.reply_count);
            rows.push(Row::Divider {
                text: match count {
                    0 => t("chat.threads.noReplies"),
                    n => crate::core::i18n::t_with("chat.threads.replies", &[("count", Arg::Num(i64::from(n)))]),
                },
            });
            if loaded.has_more {
                rows.push(Row::Older { loading: loaded.loading });
            }
        };
        if thread.is_some() {
            if parent.is_none() {
                divider(&mut rows);
            }
        } else if loaded.has_more {
            rows.push(Row::Older { loading: loaded.loading });
        } else {
            let note = here.filter(|_| !self.prefs.shared_notes_closed.contains(&format!("{key}/{channel}")));
            let topic = here.map(|c| c.topic.trim().to_owned()).filter(|t| !t.is_empty());
            let start = crate::core::i18n::t_with("chat.beginning.start", &[("channel", Arg::Str(&name))]);
            rows.push(Row::Start {
                icon: "sparkles",
                title: crate::core::i18n::t_with("chat.beginning.title", &[("channel", Arg::Str(&name))]),
                body: match topic {
                    Some(topic) => format!("{start} {topic}"),
                    None => start,
                },
                shared: note.and_then(|c| {
                    let label = shared::shared_label(c)?;
                    let sh = c.shared.as_ref()?;
                    let home = if sh.home {
                        "this server".to_owned()
                    } else {
                        sh.home_server.as_ref().map_or("the other server".into(), |s| s.name.clone())
                    };
                    let others = if label.names.is_empty() { "another server".into() } else { label.names };
                    Some((
                        format!("{key}/{channel}"),
                        if sh.home {
                            format!("You share this channel with {others}")
                        } else {
                            format!("This channel comes from {others}")
                        },
                        format!(
                            "People from both servers read and write here. Messages are kept only on {home}, \
                             and each server looks after its own people."
                        ),
                    ))
                }),
            });
        }
        let mut last_day: Option<i64> = None;
        for m in parent.into_iter().chain(&loaded.items) {
            let is_parent = parent.is_some_and(|p| std::ptr::eq(p, m));
            // A divider for each day (not between a thread's message and its replies).
            let at = ms_of(m.created_at.as_ref());
            if !is_parent && !last_day.is_some_and(|d| crate::ui::text::same_day(d, at)) {
                rows.push(Row::Day { id: format!("day|{}", m.id), text: crate::ui::text::day(at) });
            }
            last_day = Some(at);
            if m.kind == pb::MessageKind::MemberJoined as i32 {
                let who = author(&m.author_id);
                rows.push(Row::Join(Rc::new(JoinLine {
                    id: m.id.clone(),
                    user: who.3.clone(),
                    user_id: m.author_id.clone(),
                    name: who.0.clone(),
                    color: who.1,
                    at,
                    mine: m.author_id == me,
                    can_wave: can_send && !i.access(&server).pending,
                    can_delete: manage,
                })));
                continue;
            }
            if m.kind == pb::MessageKind::AutoModAlert as i32 {
                if let Some(alert) = &m.auto_mod {
                    let who = author(&m.author_id);
                    rows.push(Row::AutoMod(Rc::new(AlertLine {
                        id: m.id.clone(),
                        alert: alert.clone(),
                        user_id: m.author_id.clone(),
                        name: who.0.clone(),
                        color: who.1,
                        channel: i.channel(&server, &alert.channel_id).map(|c| c.name.clone()),
                        at,
                        can_delete: manage,
                    })));
                }
                continue;
            }
            let hook = m.webhook.as_ref();
            let who: Author = match hook {
                Some(w) => Rc::new((w.name.clone(), None, Some("APP"), Some(webhook_author(w)), None)),
                None => author(&m.author_id),
            };
            let (author_name, color, badge, user, decoration) = &*who;
            let editing = self.editing.as_deref() == Some(m.id.as_str()) && thread.is_some() == self.edit_in_thread;
            let from = shared::foreign_server(m, &server).cloned();
            let keep_out = keeps_out && from.is_some() && hook.is_none();
            let keeping_out = keep_out && self.keeping_out.as_deref() == Some(m.id.as_str());
            let can_delete = m.author_id == me || (manage && !(guest_side && from.is_some()));
            let mut card = m.poll.as_ref().map(|poll| {
                crate::ui::polls::PollCard::of(
                    poll,
                    &m.id,
                    m.author_id == me,
                    can_vote,
                    manage && !guest_side,
                    &self.polls,
                    &look,
                    now,
                )
            });
            let bits = ThreadBits {
                replies: thread
                    .is_none()
                    .then(|| crate::ui::threads::Replies::of(i, &server, m, now))
                    .flatten()
                    .map(Rc::new),
                also_in: (thread.is_none() && !m.thread_id.is_empty()).then(|| m.thread_id.clone()),
                also_sent: thread.is_some() && !is_parent && m.also_in_channel,
                can_thread: m.thread_id.is_empty() && if m.thread.is_some() { can_reply } else { can_start },
            };
            let agent = crate::ui::commands::AgentBits::of(i, &server, m, can_vote, &self.commands).map(Rc::new);
            let pinned = m.pinned_at.is_some();
            let can_pin = pins_here && m.kind == pb::MessageKind::Unspecified as i32;
            let reactions = (reactions_here && m.kind == pb::MessageKind::Unspecified as i32)
                .then(|| Rc::new(crate::ui::reactions::ReactBits::server(m, can_react, manage && !guest_side)));
            let mut h = DefaultHasher::new();
            bits.digest(&mut h);
            if let Some(agent) = &agent {
                agent.digest(&mut h);
            }
            if let Some(card) = &card {
                card.digest(&mut h);
            }
            (&m.content, m.edited_at.as_ref().map(|t| (t.seconds, t.nanos)), m.embeds.len()).hash(&mut h);
            m.emojis.iter().map(|e| (&e.id, &e.url)).for_each(|e| e.hash(&mut h));
            m.attachments.iter().map(|a| (&a.url, &a.filename)).for_each(|a| a.hash(&mut h));
            m.gif.as_ref().map(|g| (&g.url, g.width, g.height, g.provider)).hash(&mut h);
            (author_name, color.map(|c| [c.h, c.s, c.l, c.a].map(f32::to_bits)), badge).hash(&mut h);
            user.as_ref().map(|u| (&u.avatar_url, &u.username)).hash(&mut h);
            decoration.hash(&mut h);
            (look.digest, editing, manage, suppress, &me, &mine).hash(&mut h);
            (m.mentions_everyone, &m.mention_role_ids).hash(&mut h);
            (from.as_ref().map(|f| (&f.id, &f.name, &f.icon_url)), keep_out, keeping_out, can_delete).hash(&mut h);
            (pinned, can_pin).hash(&mut h);
            if let Some(r) = &reactions {
                r.digest(&mut h);
            }
            // Never 0, which means "not kept".
            let sig = h.finish() | 1;
            let (key, was) = match built.remove_entry(&m.id) {
                Some((key, was)) => (key, Some(was)),
                None => (m.id.clone(), None),
            };
            // A changed poll's bars grow from where they were.
            if let (Some(card), Some(old)) = (card.as_mut(), was.as_ref().and_then(|w| w.poll.as_ref())) {
                card.from = old.shares();
            }
            let card = card.map(Rc::new);
            let msg = match was {
                Some(was) if was.sig == sig => was,
                _ => Rc::new(Msg {
                    id: m.id.clone(),
                    user: user.clone(),
                    name: author_name.clone(),
                    color: *color,
                    content: m.content.clone(),
                    shown: mention_links(
                        &timestamp_nodes(&crate::ui::text::hard_breaks(&images_as_links(&m.content))),
                        &look.with(&m.emojis),
                    ),
                    mentions_me: i.pings_me(&server, m, suppress),
                    editing,
                    can_delete,
                    at: ms_of(m.created_at.as_ref()),
                    edited: m.edited_at.is_some(),
                    head: true,
                    mine: m.author_id == me,
                    pending: false,
                    failed: None,
                    nonce: 0,
                    unreadable: false,
                    badge: *badge,
                    embeds: m.embeds.clone(),
                    from: from.clone(),
                    keep_out,
                    keeping_out,
                    poll: card.clone(),
                    // A voice message sent here plays from its card, like one in a conversation.
                    voice: crate::ui::voice_notes::channel_voice(&m.attachments).0,
                    attachments: crate::ui::voice_notes::channel_voice(&m.attachments).1,
                    gif: m.gif.clone(),
                    thread: bits.clone(),
                    agent: agent.clone(),
                    pinned,
                    can_pin,
                    reactions: reactions.clone(),
                    decoration: decoration.clone(),
                    owner: !m.author_id.is_empty() && hook.is_none() && m.author_id == owner_id,
                    enc: None,
                    sig,
                }),
            };
            kept.insert(key, msg.clone());
            rows.push(Row::Msg(msg));
            if is_parent {
                divider(&mut rows);
                // The replies' days start over below it, as the thread's own list does.
                last_day = None;
            }
        }
        for p in i.pending.get(&at).into_iter().flatten() {
            rows.push(Row::Msg(Rc::new(Msg {
                id: format!("p{}", p.nonce),
                user: i.me.clone(),
                name: i.display_name(Some(&server), &me),
                color: i.name_color(&server, &me).map(|c| rgb(c).into()),
                content: p.content.clone(),
                shown: mention_links(
                    &timestamp_nodes(&crate::ui::text::hard_breaks(&images_as_links(&p.content))),
                    &look.with(&p.emojis),
                ),
                mentions_me: false,
                editing: false,
                can_delete: false,
                at: p.created_at_ms,
                edited: false,
                head: true,
                mine: true,
                pending: true,
                failed: p.failed.clone(),
                nonce: p.nonce,
                unreadable: false,
                badge: None,
                embeds: Vec::new(),
                from: None,
                keep_out: false,
                keeping_out: false,
                poll: None,
                voice: crate::ui::voice_notes::channel_voice(&p.attachments).0,
                attachments: crate::ui::voice_notes::channel_voice(&p.attachments).1,
                gif: None,
                thread: ThreadBits::default(),
                agent: None,
                pinned: false,
                reactions: None,
                can_pin: false,
                decoration: None,
                owner: me == owner_id,
                enc: None,
                sig: 0,
            })));
        }
        *built = kept;
        group(&mut rows);
        rows
    }

    /// The open thread's rows, built again from the store.
    pub(crate) fn sync_thread(&mut self, cx: &mut Context<Self>) {
        let Some(open) = self.threads.open.clone() else { return };
        let mut built = std::mem::take(&mut self.threads.built);
        let rows = self.core.shared.read(|s| {
            s.instance(&open.key)
                .map(|i| self.server_rows(i, &open.key, &open.server, &open.channel, Some(&open.id), &mut built))
                .unwrap_or_default()
        });
        self.threads.built = built;
        self.threads.rows = Rc::new(rows);
        cx.notify();
    }

    /// Keeps the message list in step: grows at the bottom (and lets new
    /// messages rise in), grows at the top for older ones, or starts over.
    pub(crate) fn sync_list(&mut self, cx: &mut Context<Self>) {
        let mut built = std::mem::take(&mut self.built);
        let rows = self.rows(&mut built);
        self.built = built;
        let target = self.target().map(|t| t.id());
        let digests: Vec<u64> = rows
            .iter()
            .map(|r| {
                let mut h = DefaultHasher::new();
                r.digest(&mut h);
                h.finish()
            })
            .collect();
        let mut h = DefaultHasher::new();
        digests.hash(&mut h);
        let digest = h.finish();
        let first = rows.iter().find(|r| matches!(r, Row::Msg(_) | Row::Note { .. })).map(Row::id).unwrap_or_default();
        let len = rows.len();
        let list = &mut self.list;
        if list.target != target {
            self.scroller.update(cx, |s, cx| {
                s.reset(len, cx);
                s.scroll_to_end(cx);
            });
            self.fresh.clear();
        } else if list.digest != digest {
            if len > list.len && first == list.first {
                let added = len - list.len;
                let now = Instant::now();
                for r in &rows[list.len.saturating_sub(1)..] {
                    if matches!(r, Row::Msg(_) | Row::Note { .. } | Row::Line(_) | Row::Join(_) | Row::AutoMod(_))
                        && !self.fresh.contains_key(&r.id())
                    {
                        self.fresh.insert(r.id(), now);
                    }
                }
                // Only what's actually new rises in.
                let old: std::collections::HashSet<&str> =
                    rows[..list.len.min(rows.len())].iter().map(Row::id_str).collect();
                self.fresh.retain(|id, at| !old.contains(id.as_str()) || at.elapsed() < Duration::from_millis(900));
                let changed = changed_rows(&list.rows, &digests, 0);
                // New messages that came in while you read further up.
                if self.scroller.read(cx).is_scrolled_up() {
                    let new =
                        rows[list.len.min(rows.len())..].iter().filter(|r| matches!(r, Row::Msg(m) if !m.mine)).count();
                    self.msg_ui.missed += new;
                }
                self.scroller.update(cx, |s, cx| {
                    s.append(added, cx);
                    for range in changed {
                        s.remeasure_items(range, cx);
                    }
                });
            } else if len > list.len && first != list.first {
                let added = len - list.len;
                let changed = changed_rows(&list.rows, &digests, added);
                self.scroller.update(cx, |s, cx| {
                    s.prepend(added, cx);
                    for range in changed {
                        s.remeasure_items(range, cx);
                    }
                });
            } else if len == list.len {
                let changed = changed_rows(&list.rows, &digests, 0);
                self.scroller.update(cx, |s, cx| {
                    for range in changed {
                        s.remeasure_items(range, cx);
                    }
                });
            } else {
                self.scroller.update(cx, |s, cx| s.reset(len, cx));
            }
        }
        self.list.target = target;
        self.list.len = len;
        self.list.first = first;
        self.list.digest = digest;
        self.list.rows = digests;
        self.fresh.retain(|_, at| at.elapsed() < Duration::from_secs(2));
        self.poll_tick(&rows, cx);
        self.rows = Rc::new(rows);
        self.time_tick(cx);
    }

    pub(crate) fn render_main(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let body: AnyElement = match self.nav.clone() {
            Nav::Server { key, server } => self.channel_view(&key, &server, window, cx),
            Nav::Home { dm: Some((key, id)) } => self.dm_view(&key, &id, window, cx),
            Nav::Home { dm: None } => home_splash(&p, window).into_any_element(),
            Nav::Friends { key } => self.friends_view(&key, window, cx),
            Nav::Instance { key } => self.instance_home(&key, window, cx),
        };
        div().flex_1().h_full().min_w_0().flex().bg(p.chat_surface).child(body)
    }

    // ───────────────────────── A channel ─────────────────────────

    fn channel_view(&mut self, key: &str, server: &str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        if let Some(locked) = self.sso_locked(key, server) {
            return self.sso_gate(key, &locked, &p, window, cx);
        }
        // A voice channel opens as its stage (voice_stage.rs).
        if let Some(stage) = self.stage_in(key, server)
            && let Some(voice) =
                self.core.shared.read(|s| s.instance(key).and_then(|i| i.channel(server, &stage).cloned()))
        {
            return self.voice_stage(key, server, &voice, window, cx);
        }
        let channel = self
            .channel_in(key, server)
            .and_then(|id| self.core.shared.read(|s| s.instance(key).and_then(|i| i.channel(server, &id).cloned())));
        let Some(channel) = channel else {
            let loading = self.core.shared.read(|s| s.instance(key).is_some_and(|i| !i.channels.contains_key(server)));
            return empty_state(
                &p,
                if loading { "loader-circle" } else { "hash" },
                if loading { "Getting the server ready…" } else { "No text channels here yet" },
                if loading { "" } else { "Someone who can manage channels can add one from the web app." },
            )
            .into_any_element();
        };
        let secure = channel.r#type == pb::ChannelType::Secure as i32;
        let header = if secure {
            self.secure_header(key, server, &channel, &p, cx)
        } else {
            div()
                .h(px(56.0))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(8.0))
                .px(px(16.0))
                .border_b_1()
                .border_color(p.border)
                .child(icon(crate::ui::sidebar::channel_glyph(&channel)).size(px(20.0)).text_color(p.muted_foreground))
                // A renamed channel's name swaps in place (the web's `SwapText`).
                .child(
                    div()
                        .flex_none()
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .text_size(px(16.0))
                        .line_height(px(24.0))
                        .child(motion::swap_text(
                            SharedString::from(format!("channel-name|{}", channel.id)),
                            channel.name.clone(),
                            16.0,
                            window,
                            cx,
                        )),
                )
                .when_some(shared::pill_text(&channel), |el, text| {
                    el.child(crate::ui::shared_marks::pill(text, &channel.id, &p))
                })
                .when(!channel.topic.is_empty(), |el| {
                    el.child(div().flex_none().w(px(1.0)).h(px(20.0)).bg(p.border)).child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_sm()
                            .text_color(p.muted_foreground)
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(channel.topic.clone()),
                    )
                })
                .when(channel.topic.is_empty(), |el| el.child(div().flex_1()))
        }
        .when(!secure, |el| el.child(self.search_field(window, cx)));
        // A secure channel's header has its own buttons (`secure_header`).
        let header =
            if secure { header } else { header.child(self.header_buttons(key, server, &channel, &p, window, cx)) };

        let body = if secure {
            self.secure_body(key, server, &channel, window, cx)
        } else {
            let blocked = self.channel_blocked(key, server, &channel.id);
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(self.message_list(window, cx))
                .child(self.composer_bar(blocked, window, cx))
                .into_any_element()
        };
        let column = div().flex_1().min_w_0().h_full().flex().flex_col().child(header).child(body);

        let mut view = div().size_full().relative().flex().child(column);
        if secure {
            // No member list beside it: nobody's in sight there.
            self.core.set_in_view(key, server, &[]);
            // A secure channel's threads are its own (`secure_threads.rs`).
            return view.children(self.secure_side(window, cx)).into_any_element();
        }
        let mut members = false;
        if let Some(panel) = self.search_panel(window, cx) {
            view = view.child(panel);
        } else if let Some(panel) = self.thread_panel(window, cx) {
            view = view.child(panel);
        } else if let Some(panel) = self.threads_list_panel(window, cx) {
            view = view.child(panel);
        } else if self.members_open {
            members = true;
            view = view.child(self.members_panel(key, server, window, cx));
        }
        // The member list hidden: its people aren't on screen (a live connection's focus).
        if !members {
            self.core.set_in_view(key, server, &[]);
        }
        view.into_any_element()
    }

    /// Why you can't write in a server's channel, if you can't: the rules, a time out, or its permissions.
    pub(crate) fn channel_blocked(&self, key: &str, server: &str, channel_id: &str) -> Option<Blocked> {
        self.core.shared.read(|s| {
            let i = s.instance(key)?;
            let access = i.access(server);
            if access.pending {
                return Some(Blocked {
                    text: "Agree to this server's rules to start talking.".into(),
                    action: Some(("Read the rules", Dialog::Rules { key: key.to_owned(), server: server.to_owned() })),
                });
            }
            let now = crate::core::dms::now_ms();
            if let Some(until) = i.my_member(server).and_then(|m| m.timed_out_until.as_ref()).map(|t| ms_of(Some(t)))
                && until > now
                && !access.owner
            {
                return Some(Blocked { text: format!("You're timed out until {}.", clock(until)), action: None });
            }
            (!access.has_in(channel_id, pb::Permission::SendMessages))
                .then(|| Blocked { text: "You can't send messages in this channel.".into(), action: None })
        })
    }

    /// A channel header's bell and member list buttons.
    fn header_buttons(
        &mut self,
        key: &str,
        server: &str,
        channel: &pb::Channel,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = *p;
        let channel_id = channel.id.as_str();
        // Threads aren't in shared channels yet, nor in secure ones (theirs are their own).
        let threads = channel.shared.is_none() && channel.r#type != pb::ChannelType::Secure as i32;
        let side = self.threads.open.is_some() || self.threads.listing.is_some();
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.0))
            .child(self.bell_button(key, server, channel_id, cx))
            .when(self.pins_here(key, channel), |el| {
                let open = self.pins.as_ref().is_some_and(
                    |p| !matches!(&p.place, crate::ui::pins::PinPlace::Channel { thread, .. } if !thread.is_empty()),
                );
                el.child(self.pins_button_in("pins-toggle", open, window, cx))
            })
            .when(threads, |el| {
                let open = self.threads.listing.is_some();
                el.child(
                    header_button("threads-toggle", "messages-square", open, -8.0, &p)
                        .tooltip(|window, cx| crate::ui::overlay::Tip::new("Threads").build(window, cx))
                        .on_click(cx.listener(|this, _, window, cx| this.toggle_threads_list(window, cx))),
                )
            })
            .child({
                let shown = self.members_open && !side && self.search.panel.is_none();
                header_button("members-toggle", "users", shown, -12.0, &p).on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.close_thread(cx);
                        this.threads.listing = None;
                        this.pins = None;
                        if this.search.panel.is_some() {
                            this.close_search(window, cx);
                        }
                        this.members_open = !shown;
                        cx.notify();
                    },
                ))
            })
    }

    pub(crate) fn message_list(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let rows = self.rows.clone();
        let ctx = self.row_ctx(None, cx);
        let target = self.list.target.clone().unwrap_or_default();
        let loading = rows.is_empty();
        crate::ui::mentions::set_app(cx.entity().downgrade());
        // Back to the newest messages once you've scrolled up, counting what came in meanwhile.
        let up = !loading && self.scroller.read(cx).is_scrolled_up();
        if !up {
            self.msg_ui.missed = 0;
        }
        // An encrypted list notes where its start and last row land, to sit at the bottom while short.
        let fits = matches!(rows.first(), Some(Row::DmStart(_)) | Some(Row::Start { icon: "shield-check", .. }));
        // Its pill shows only what came in meanwhile, as the web's `EncryptedMessages` does.
        let shown = (up && (!fits || self.msg_ui.missed > 0)).then_some(self.msg_ui.missed);
        // Once it's no longer wanted, it sinks away a moment more.
        let going = motion::kept(&format!("jump-pill|{target}"), shown.as_ref(), window, cx);
        let jump = match (shown, going) {
            (Some(missed), _) => Some(crate::ui::chat_rows::jump_pill(missed, None, &p, window, cx)),
            (None, Some((missed, t))) => Some(crate::ui::chat_rows::jump_pill(missed, Some(t), &p, window, cx)),
            (None, None) => None,
        };
        let len = rows.len();
        div()
            .flex_1()
            .min_h_0()
            .relative()
            .child(if loading {
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .justify_end()
                    .child(crate::ui::chat_rows::skeleton(6, &p))
                    .into_any_element()
            } else {
                // A channel's messages rise in as it opens (`MessageList`'s first motion).
                let list = MessageScroller::new(
                    SharedString::from(format!("list|{target}")),
                    self.scroller.clone(),
                    move |ix, _window, cx| match rows.get(ix) {
                        Some(row) if fits => {
                            crate::ui::dm_view::placed(render_row(row, ix, &ctx, cx), ix == 0, ix + 1 == len)
                        }
                        Some(row) => render_row(row, ix, &ctx, cx),
                        None => div().into_any_element(),
                    },
                )
                .jump_button(false)
                .with_row_style(gpui_kit::StyleRefinement::default().pb(px(0.0)).px(px(0.0)))
                .with_list_style(gpui_kit::StyleRefinement::default().pt(px(0.0)).pb(px(12.0)))
                .size_full();
                motion::rise(
                    div().size_full().child(list),
                    SharedString::from(format!("list-in|{target}")),
                    Duration::ZERO,
                    12.0,
                )
                .into_any_element()
            })
            .children(jump)
    }

    /// What every row of a list needs from the window; `thread` for the open thread's panel.
    pub(crate) fn row_ctx(&self, thread: Option<String>, cx: &mut Context<Self>) -> Rc<RowCtx> {
        Rc::new(RowCtx {
            fresh: if thread.is_some() { HashMap::new() } else { self.fresh.clone() },
            this: cx.entity().downgrade(),
            compact: self.prefs.density == Density::Compact,
            edit_box: self.edit_box.clone(),
            key: self.target().map(|t| t.key().to_owned()).unwrap_or_default(),
            server: match self.target() {
                Some(Target::Channel { server, .. }) => Some(server),
                _ => None,
            },
            url: self
                .target()
                .and_then(|t| self.core.shared.read(|s| s.instance(t.key()).map(|i| i.url.clone())))
                .unwrap_or_default(),
            jumped: self.search.jumped.clone().filter(|(_, at)| at.elapsed() < JUMP_GLOW),
            thread,
            lit: self.context.as_ref().map(|c| c.of.lit()),
            saved_gifs: crate::ui::gifs::saved_here(self, self.target().as_ref().map_or("", |t| t.key())),
            hover: self.msg_ui.hovered.clone().or_else(|| self.msg_ui.hovered_tools.clone()),
            deleting: self.msg_ui.deleting.clone(),
            copied: self.msg_ui.copied.as_ref().map(|(id, _)| id.clone()),
            developer: self.prefs.developer_mode,
            waved: self.msg_ui.waved.clone(),
            waving: self.msg_ui.waving.clone(),
            send_keys: match self.core.prefs().send_with {
                crate::core::config::SendWith::Enter => "Enter".to_owned(),
                crate::core::config::SendWith::ModEnter => {
                    format!("{}+Enter", if cfg!(target_os = "macos") { "⌘" } else { "Ctrl" })
                }
            },
        })
    }

    /// Where you write: grows with what you type, Enter sends, Shift+Enter
    /// adds a line. `blocked` says why you can't send here, if you can't.
    fn composer_bar(
        &mut self,
        blocked: Option<Blocked>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = pal(cx);
        self.follow_send_with(cx);
        // Rules to agree to, a time-out or roles that only read: a notice in the box's place.
        if let Some(notice) = self.composer_notice(blocked.as_ref(), &p, cx) {
            return notice;
        }
        // The web lights the box while anything in it has the focus, a command's fields too.
        let focused = self.composer.read(cx).focus_handle(cx).is_focused(window) || self.commands.form.is_some();
        // A picked command takes the box's place, with its options as fields.
        let command = self.command_form_here();
        let gate = self.send_gate();
        let files = self.files_state();
        let length = self.composer.read(cx).value().chars().count();
        let typed = !self.composer.read(cx).value().trim().is_empty();
        let cooling = gate.as_ref().is_some_and(|g| g.cooling());
        let can_send = if command {
            self.command_ready(cx)
        } else {
            (typed || files.any)
                && length <= crate::ui::composer::MAX_CHARS
                && !cooling
                && !files.uploading
                && !files.broken
        };
        let ring = motion::follow("composer-ring", if focused { 1.0 } else { 0.0 }, window, cx);
        let ready = motion::follow("composer-send", if can_send { 1.0 } else { 0.0 }, window, cx);
        // Not ready (and not counting down) sits a little smaller, as on the web.
        let rest = motion::follow("composer-send-scale", if can_send || cooling { 1.0 } else { 0.9 }, window, cx);
        let emoji_panel = self.emoji_panel(&p, window, cx);
        let gif_panel = self.gif_panel(&p, window, cx);
        let time_panel = self.time_picker_leaving(&p, window, cx);
        let tray = self.file_tray(&p, cx);
        let recording = !command && self.recording_here();
        let gif_button = (!command && !recording).then(|| self.gif_button(&p, cx)).flatten();
        // The microphone takes the send button's place while nothing's typed, as on the web.
        let voice = (!command && (recording || (!typed && !files.any && !cooling && self.can_record())))
            .then(|| self.voice_button(&p, cx));
        let commands_list = self.command_list_view(&p, window, cx);
        // The @ list, once closed, a moment more on its way out.
        let mentions_going = motion::kept("mention-picker", self.picker.as_ref(), window, cx);
        let picks = self.command_picks_view(&p, window, cx);
        let form = self.command_form_view(&p, cx);
        let field: AnyElement = if let Some(form) = form {
            form
        } else if recording {
            self.recording_bar(&p, cx)
        } else {
            // The editor keeps 10px of its own on the left and 8px above and below its
            // 24px lines; the web's box has 6px above and below and nothing at the side.
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
                .child(Textarea::new(&self.composer).appearance(false).text_size(px(15.2)).line_height(px(24.0)))
                .into_any_element()
        };
        let hint = self.composer_hint(&p);
        let attach = (!command && !recording && self.can_attach()).then(|| self.attach_button(&p, cx));
        let tools = (!command && !recording).then(|| {
            div().flex().items_end().gap(px(8.0)).child(self.timestamp_button(&p, cx)).child(self.emoji_button(&p, cx))
        });
        let poll = (!command && self.can_poll()).then(|| {
            crate::ui::widgets::tool_button("poll-open", "chart-column", self.polls.editor.is_some(), &p)
                .hover(|s| s.scale(1.12).translate_y(px(-1.0)))
                .active(|s| s.scale(0.85))
                .tooltip(|window, cx| {
                    crate::ui::overlay::Tip::new(crate::core::i18n::t("chat.composer.makePoll")).build(window, cx)
                })
                .on_click(cx.listener(|this, _, window, cx| this.open_poll_editor(window, cx)))
        });
        let show_send = command || (!recording && voice.is_none());
        let send = show_send.then(|| {
            div()
                .id("send")
                .size(px(36.0))
                .mb(px(2.0))
                .flex_none()
                .rounded(crate::ui::theme::radius_xl())
                .flex()
                .items_center()
                .justify_center()
                .bg(alpha(p.primary, ready))
                .text_color(mix(p.muted_foreground, p.primary_foreground, ready))
                .shadow(vec![gpui_kit::BoxShadow {
                    color: alpha(p.primary, ready),
                    offset: gpui_kit::point(px(0.0), px(6.0)),
                    blur_radius: px(18.0),
                    spread_radius: px(-8.0),
                    inset: false,
                }])
                .cursor_pointer()
                // The whole button rests at 90% while there's nothing to send, and gives under a press.
                .scale(rest)
                .active(|s| s.scale(0.85))
                .on_click(cx.listener(|this, _, window, cx| {
                    if this.commands.form.is_some() {
                        this.run_picked_command(window, cx)
                    } else {
                        this.send_from_button(window, cx)
                    }
                }))
                .child(match &gate {
                    Some(gate) if gate.cooling() && !command => crate::ui::composer::cooldown(gate, &p),
                    _ if files.uploading && !command => crate::ui::composer::upload_ring(files.share, &p),
                    _ => div()
                        .relative()
                        .left(px(-3.0 + 3.0 * ready))
                        .child(icon("send-horizontal").size(px(18.0)))
                        .into_any_element(),
                })
        });
        // The web's composer: a card holding the files to send, then a row with
        // the attach button first, the box, the tools, and send (or the
        // microphone while there's nothing to send).
        let row = div()
            .flex()
            .items_end()
            .gap(px(8.0))
            .children(attach)
            .child(field)
            .children(if command || recording { None } else { self.chars_left_in(length, &p, window, cx) })
            .children(tools)
            .children(gif_button)
            .children(poll)
            .children(voice)
            .children(send);
        let card = div()
            .id("composer-box")
            .flex()
            .flex_col()
            .px(px(12.0))
            .py(px(8.0))
            .rounded(crate::ui::theme::radius_2xl())
            .bg(p.card)
            .border_1()
            .border_color(mix(p.border, mix(p.border, p.primary, 0.6).into(), ring))
            .shadow(vec![
                gpui_kit::BoxShadow {
                    color: alpha(p.primary, 0.14 * ring),
                    offset: gpui_kit::point(px(0.0), px(0.0)),
                    blur_radius: px(0.0),
                    spread_radius: px(4.0),
                    inset: false,
                },
                gpui_kit::BoxShadow {
                    color: alpha(p.primary, ring),
                    offset: gpui_kit::point(px(0.0), px(12.0)),
                    blur_radius: px(30.0),
                    spread_radius: px(-18.0),
                    inset: false,
                },
            ])
            .map(|el| self.droppable(el, &p, cx))
            .map(|el| self.mic_slide(el, cx))
            .children(tray)
            .child(row);
        let footer = self.composer_footer(hint, gate.as_ref(), &p);
        if let Some(gate) = &gate {
            self.tick_gate(gate, cx);
        }
        div()
            .flex_none()
            .relative()
            .px(px(16.0))
            .pb(px(12.0))
            .when_some(self.picker.clone(), |el, picker| el.child(self.picker_list(picker, None, &p, cx)))
            .when_some(mentions_going, |el, (picker, t)| el.child(self.picker_list(picker, Some(t), &p, cx)))
            .children(commands_list)
            .children(picks)
            .children(emoji_panel)
            .children(gif_panel)
            .children(time_panel)
            .child(self.shaken(card))
            .child(footer)
            .children(self.drop_overlay(&p, window, cx))
            .into_any_element()
    }

    fn members_panel(
        &mut self,
        key: &str,
        server: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let current = self.members_view.as_ref().filter(|v| {
            let v = v.read(cx);
            v.key == key && v.server == server
        });
        let view = match current {
            Some(view) => view.clone(),
            None => {
                let core = self.core.clone();
                let view = cx.new(|cx| MembersView::new(core, key.to_owned(), server.to_owned(), window, cx));
                let (k, s) = (key.to_owned(), server.to_owned());
                cx.subscribe_in(&view, window, move |this, _, event: &MembersEvent, window, cx| match event {
                    MembersEvent::Open { user_id } => {
                        let dialog =
                            Dialog::Profile { key: k.clone(), user_id: user_id.clone(), server: Some(s.clone()) };
                        this.open_dialog(dialog, window, cx)
                    }
                    MembersEvent::Menu { user_id, at } => {
                        let of = MenuOf::Member { key: k.clone(), server: Some(s.clone()), user_id: user_id.clone() };
                        this.open_context_menu(of, *at, window, cx)
                    }
                    MembersEvent::Hover { user_id, on } => {
                        let of = MenuOf::Member { key: k.clone(), server: Some(s.clone()), user_id: user_id.clone() };
                        this.set_hover_target(of, *on)
                    }
                })
                .detach();
                self.members_view = Some(view.clone());
                view
            }
        };
        // Its rows in sight, again on screen after it was hidden (it says so itself as it scrolls).
        self.core.set_in_view(key, server, &view.read(cx).in_view);
        gpui_kit::AnyView::from(view).cached(gpui_kit::StyleRefinement::default().w(px(240.0)).h_full().flex_none())
    }

    /// Polls go in a server's plain channels, never shared ones, for those who may make them,
    /// on an instance that has them.
    pub(crate) fn can_poll(&self) -> bool {
        let Some(Target::Channel { key, server, channel }) = self.target() else { return false };
        self.core.shared.read(|s| {
            s.instance(&key).is_some_and(|i| {
                let versions = i.node.as_ref().and_then(|n| n.versions.as_ref());
                crate::core::compat::instance_has(versions, "polls", &crate::core::compat::FEATURES)
                    && i.access(&server).has_in(&channel, pb::Permission::CreatePolls)
                    && i.channel(&server, &channel).is_some_and(|c| c.shared.is_none())
            })
        })
    }

    fn send_from_button(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.composer.update(cx, |state, cx| state.focus(window, cx));
        let text = self.composer.read(cx).value().to_string();
        if !text.trim().is_empty() {
            // The same as pressing Enter.
            self.send_now(window, cx);
        }
    }
}

/// Puts follow-ups from the same person under one header.
pub(crate) fn group(rows: &mut [Row]) {
    let mut last: Option<(String, i64)> = None;
    for row in rows.iter_mut() {
        match row {
            Row::Msg(m) => {
                let author = m.user.as_ref().map(|u| u.id.clone()).unwrap_or_else(|| m.name.clone());
                // An agent's answer starts its own group, under who used what.
                let used = m.agent.as_ref().is_some_and(|a| a.used.is_some());
                let head =
                    used || !matches!(&last, Some((a, at)) if *a == author && m.at - at < GROUP_MS && m.at >= *at);
                if m.head != head {
                    Rc::make_mut(m).head = head;
                }
                last = Some((author, m.at));
            }
            _ => last = None,
        }
    }
}

/// What every row of the open list needs from the window.
pub(crate) struct RowCtx {
    pub(crate) fresh: std::collections::HashMap<String, Instant>,
    pub(crate) this: WeakEntity<FuwaApp>,
    pub(crate) compact: bool,
    pub(crate) edit_box: gpui_kit::Entity<gpui_kit::component::input::TextareaState>,
    pub(crate) key: String,
    pub(crate) server: Option<String>,
    /// The instance's address, which pictures of other servers must come from.
    pub(crate) url: String,
    /// The message a search result opened, which glows a moment.
    pub(crate) jumped: Option<(String, Instant)>,
    /// Drawing the open thread's panel: the id of the message it's under.
    pub(crate) thread: Option<String>,
    /// What an open right-click menu is for, lit while it's open.
    pub(crate) lit: Option<String>,
    /// Your saved GIFs' links here, for the stars on GIFs.
    pub(crate) saved_gifs: Rc<std::collections::HashSet<String>>,
    /// The row under the pointer (or whose tools are), which shows its tools.
    pub(crate) hover: Option<String>,
    /// The row asking "Delete?".
    pub(crate) deleting: Option<String>,
    /// The message whose text was just copied.
    pub(crate) copied: Option<String>,
    /// Developer mode: a button to copy a message's id.
    pub(crate) developer: bool,
    /// Join lines waved at, and waves on their way.
    pub(crate) waved: std::collections::HashSet<String>,
    pub(crate) waving: std::collections::HashSet<String>,
    /// How the send keys read in the edit box's hint.
    pub(crate) send_keys: String,
}

pub(crate) fn render_row(row: &Row, ix: usize, ctx: &Rc<RowCtx>, cx: &mut App) -> AnyElement {
    let p = pal(cx);
    let is_fresh = ctx.fresh.contains_key(row.id_str());
    let el: AnyElement = match row {
        // More history above: in a channel it comes in by itself as this row shows (the web's
        // scroll to the top), shimmering meanwhile; a thread's panel keeps its button.
        Row::Older { loading } if ctx.thread.is_none() => {
            if !*loading {
                let this = ctx.this.clone();
                cx.defer(move |cx| {
                    let _ = this.update(cx, |this, cx| {
                        if this.msg_ui.older_at.is_some_and(|at| at.elapsed() < Duration::from_secs(1)) {
                            return;
                        }
                        this.msg_ui.older_at = Some(Instant::now());
                        this.load_older(cx);
                    });
                });
            }
            crate::ui::chat_rows::skeleton(2, &p)
        }
        Row::Older { loading } => {
            let this = ctx.this.clone();
            div()
                .flex()
                .justify_center()
                .py(px(16.0))
                .child(soft_button("older", if *loading { "Loading…" } else { "Show older messages" }, &p).on_click(
                    move |_, _, cx| {
                        let _ = this.update(cx, |this, cx| this.load_older_replies(cx));
                    },
                ))
                .into_any_element()
        }
        // A secure channel's start says how it's kept private, in its own colour.
        Row::Start { icon: glyph, title, body, .. } if *glyph == "shield-check" => {
            crate::ui::secure::start(title.clone(), body.clone(), &p).into_any_element()
        }
        // A channel's beginning, as the web's `Beginning`.
        Row::Start { icon: glyph, title, body, shared } if *glyph == "sparkles" => crate::ui::chat_rows::beginning(
            &ctx.key,
            glyph,
            title,
            body,
            shared.clone().map(|(note, title, line)| shared_note(note, title, line, &p, ctx).into_any_element()),
            &p,
        ),
        Row::Start { icon: glyph, title, body, shared } => div()
            .px(px(20.0))
            .pt(px(32.0))
            .pb(px(16.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .size(px(64.0))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(alpha(p.primary, 0.14))
                    .text_color(p.primary)
                    .child(icon(glyph).size(px(30.0))),
            )
            .child(div().text_2xl().font_weight(FontWeight::EXTRA_BOLD).child(title.clone()))
            .child(div().text_color(p.muted_foreground).child(body.clone()))
            .when_some(shared.clone(), |el, (note, title, line)| el.child(shared_note(note, title, line, &p, ctx)))
            .into_any_element(),
        Row::Note { icon: glyph, text, .. } => div()
            .px(px(20.0))
            .py(px(6.0))
            .flex()
            .items_center()
            .gap(px(10.0))
            .text_sm()
            .text_color(p.muted_foreground)
            .child(
                div()
                    .w(px(40.0))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .child(icon(glyph).size(px(16.0)).text_color(p.primary)),
            )
            .child(div().flex_1().min_w_0().child(div().flex_none().child(text.clone())))
            .into_any_element(),
        // Under a thread's message (`ThreadStart`): how many replies, then a rule.
        Row::Divider { text } => div()
            .px(px(16.0))
            .my(px(12.0))
            .flex()
            .items_center()
            .gap(px(12.0))
            .text_xs()
            .line_height(px(16.0))
            .font_weight(FontWeight::BOLD)
            .text_color(p.muted_foreground)
            .child(text.clone())
            .child(div().flex_1().h(px(1.0)).bg(p.border))
            .into_any_element(),
        Row::Day { text, .. } => crate::ui::chat_rows::day_row(text, &p),
        Row::DmStart(start) => crate::ui::dm_view::render_start(start, &p),
        Row::Line(line) => crate::ui::dm_view::render_line(line, &p),
        Row::Join(j) => crate::ui::chat_rows::join_row(j, ctx, &p),
        Row::AutoMod(a) => crate::ui::chat_rows::alert_row(a, ctx, &p),
        Row::Msg(m) => message(m, &p, ctx, cx),
    };
    // A message of yours the server took glows once (the web's `.landed`).
    let el = match row {
        Row::Msg(m) if is_fresh && m.mine && !m.pending => {
            let glow = p.primary;
            div()
                .relative()
                .child(gpui_kit::AnimationExt::with_animation(
                    div().absolute().inset_0().bg(alpha(glow, 0.1)).border_l(px(3.0)).border_color(glow),
                    SharedString::from(format!("landed|{}", m.id)),
                    gpui_kit::Animation::new(Duration::from_millis(1400)),
                    |el, t| el.opacity(1.0 - t),
                ))
                .child(el)
                .into_any_element()
        }
        _ => el,
    };
    let el = if is_fresh {
        motion::rise(div().child(el), SharedString::from(format!("rise|{}|{ix}", row.id())), Duration::ZERO, 12.0)
            .into_any_element()
    } else {
        el
    };
    match &ctx.jumped {
        // Where a search result led: a glow that holds a moment, then fades by itself.
        Some((id, at)) if id == row.id_str() => {
            let glow = p.primary;
            div()
                .relative()
                .child(el)
                .child(gpui_kit::AnimationExt::with_animation(
                    div().absolute().inset_0().bg(alpha(glow, 0.16)).border_l(px(3.0)).border_color(glow),
                    SharedString::from(format!("jumped|{id}|{at:?}")),
                    gpui_kit::Animation::new(JUMP_GLOW),
                    |el, t| el.opacity(if t < 0.35 { 1.0 } else { 1.0 - (t - 0.35) / 0.65 }),
                ))
                .into_any_element()
        }
        _ => el,
    }
}

/// How long a message a search result opened glows.
const JUMP_GLOW: Duration = Duration::from_millis(2200);

/// Said once at the start of a shared channel: which servers talk here and
/// where what's said is kept. Closing it is remembered on this computer.
fn shared_note(note: String, title: String, line: String, p: &Palette, ctx: &Rc<RowCtx>) -> impl IntoElement {
    let this = ctx.this.clone();
    motion::rise(
        div()
            .relative()
            .mt(px(16.0))
            .max_w(px(576.0))
            .flex()
            .items_start()
            .gap(px(12.0))
            .p(px(12.0))
            .pr(px(40.0))
            .rounded(corner(16.0))
            .border_1()
            .border_color(alpha(p.primary, 0.25))
            .bg(alpha(p.primary, 0.05))
            .child(
                div()
                    .flex_none()
                    .size(px(36.0))
                    .rounded(corner(12.0))
                    .bg(alpha(p.primary, 0.15))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(crate::ui::shared_marks::glyph(20.0, p.primary)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .child(div().font_weight(FontWeight::BOLD).child(title))
                    .child(div().text_color(p.muted_foreground).child(line)),
            )
            .child(div().absolute().top(px(8.0)).right(px(8.0)).child(
                icon_button(SharedString::from(format!("shared-note-close|{note}")), "x", p).on_click(
                    move |_, _, cx| {
                        let note = note.clone();
                        let _ = this.update(cx, |this, cx| {
                            this.core.set_prefs(|prefs| {
                                prefs.shared_notes_closed.insert(note);
                            });
                            this.prefs = this.core.prefs();
                            this.sync_list(cx);
                            cx.notify();
                        });
                    },
                ),
            )),
        "shared-note",
        Duration::from_millis(150),
        10.0,
    )
}

/// Right-clicking someone's name or picture in a message opens their menu.
fn person_menu(
    ctx: &Rc<RowCtx>,
    user_id: String,
) -> impl Fn(&gpui_kit::MouseDownEvent, &mut Window, &mut App) + 'static {
    let (key, server) = (ctx.key.clone(), ctx.server.clone());
    context_menu::on_right_click(ctx.this.clone(), move |_, _| {
        Some(MenuOf::Member { key: key.clone(), server: server.clone(), user_id: user_id.clone() })
    })
}

/// Opens someone's card from a message: their name, their picture, or a mention.
fn open_profile(ctx: &RowCtx, user_id: String, window: &mut Window, cx: &mut App) {
    open_card(&ctx.this, &ctx.key, ctx.server.clone(), user_id, window, cx);
}

fn open_card(
    this: &WeakEntity<FuwaApp>,
    key: &str,
    server: Option<String>,
    user_id: String,
    window: &mut Window,
    cx: &mut App,
) {
    let key = key.to_owned();
    let _ = this.update(cx, |this, cx| {
        this.open_dialog(Dialog::Profile { key, user_id, server }, window, cx);
    });
}

fn message(m: &Rc<Msg>, p: &Palette, ctx: &Rc<RowCtx>, _cx: &mut App) -> AnyElement {
    let compact = ctx.compact;
    // The web's .message-row: the muted color at 45% on hover, 70% while its menu is open.
    let hover = alpha(p.muted, 0.45);
    let held = alpha(p.muted, 0.7);
    // A message AutoMod stopped: why, without a retry that would only be stopped again.
    let blocked = m.failed.as_deref().and_then(|f| f.strip_prefix("AutoMod: ")).map(str::to_owned);
    let deleted = m.enc.as_ref().is_some_and(|e| e.deleted);
    let content: AnyElement = if deleted {
        crate::ui::dm_view::deleted_text(p)
    } else if m.editing {
        edit_box(m, p, ctx).into_any_element()
    } else if let Some(card) = &m.voice {
        crate::ui::voice_notes::voice_card(&m.id, card, p, &ctx.this)
    } else if m.content.trim().is_empty() {
        div().into_any_element()
    } else if m.unreadable {
        div().italic().text_color(p.muted_foreground).child(m.content.clone()).into_any_element()
    } else {
        let (this, key, server) = (ctx.this.clone(), ctx.key.clone(), ctx.server.clone());
        let jumbo = crate::ui::mentions::only_emoji(&m.content);
        let text = div()
            .w_full()
            .when(m.failed.is_some(), |el| el.text_color(p.destructive))
            .when(blocked.is_some(), |el| el.line_through())
            // A message of nothing but emoji draws them big (`.jumbo`).
            .when(jumbo, |el| el.text_size(px(33.75)).line_height(px(42.0)))
            .child(
                gpui_kit::base::TextView::markdown(SharedString::from(format!("md|{}", m.id)), m.shown.clone())
                    .markdown_extensions(crate::ui::emoji::markdown_extensions())
                    .selectable(true)
                    .style(chat_markdown(p))
                    .on_link_click(move |url, _, window, cx| match url.strip_prefix(SCHEME) {
                        Some(mention) => {
                            let Some(username) = mention.strip_prefix("user/") else { return };
                            let username = username.split('/').next().unwrap_or_default();
                            let found = this.upgrade().and_then(|app| {
                                let server = server.clone()?;
                                app.read(cx).core.shared.read(|s| {
                                    s.instance(&key)?
                                        .members
                                        .get(&server)?
                                        .iter()
                                        .filter_map(|m| m.user.as_ref())
                                        .find(|u| u.username.eq_ignore_ascii_case(username))
                                        .map(|u| u.id.clone())
                                })
                            });
                            if let Some(id) = found {
                                open_card(&this, &key, server.clone(), id, window, cx);
                            }
                        }
                        None => crate::ui::text::open_link(url, cx),
                    })
                    .w_full(),
            );
        // Big emoji of yours pop in as the server takes them (the web's `.landed .jumbo`).
        if jumbo && m.mine && !m.pending && ctx.fresh.contains_key(&m.id) {
            motion::once(
                text.transform_origin(0.0, 1.0),
                SharedString::from(format!("jumbo|{}", m.id)),
                Duration::from_millis(450),
                |el, t| {
                    let t = gpui_kit::ease_out_quint()(t);
                    let k = if t < 0.6 { 0.4 + 0.72 * t / 0.6 } else { 1.12 - 0.12 * (t - 0.6) / 0.4 };
                    el.scale(k).opacity((t / 0.6).min(1.0))
                },
            )
        } else {
            text.into_any_element()
        }
    };
    // Apps and bots have no profile to open; agents do.
    let author = m.user.as_ref().filter(|_| matches!(m.badge, None | Some("AGENT"))).map(|u| u.id.clone());
    let tint = crate::ui::widgets::name_tint(m.user.as_ref().map(|u| u.id.as_str()).unwrap_or(&m.name), p);
    // The web's `AuthorName`: the name in its role's colour, a crown for the owner, the app tag.
    let author_name = || {
        div()
            .flex()
            .flex_none()
            .min_w_0()
            .max_w(px(if compact { 280.0 } else { 520.0 }))
            .items_center()
            .gap(px(4.0))
            .child(
                div()
                    .id(SharedString::from(format!("name|{}", m.id)))
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .font_weight(FontWeight::BOLD)
                    .text_size(px(16.0))
                    .text_color(m.color.filter(|_| crate::ui::theme::role_names()).unwrap_or(tint))
                    .when(author.is_some(), |el| el.cursor_pointer().hover(|s| s.underline()))
                    .when_some(author.clone(), |el, id| {
                        let ctx = ctx.clone();
                        el.child(crate::ui::profile_card::mark(&id, crate::ui::profile_card::Side::Right))
                            .on_mouse_down(MouseButton::Right, person_menu(&ctx, id.clone()))
                            .on_click(move |_, window, cx| open_profile(&ctx, id.clone(), window, cx))
                    })
                    .child(m.name.clone()),
            )
            .when(m.owner, |el| {
                el.child(
                    div()
                        .id(SharedString::from(format!("crown|{}", m.id)))
                        .flex_none()
                        .tooltip(|window, cx| crate::ui::overlay::Tip::new(t("chat.author.owner")).build(window, cx))
                        .child(icon("crown").size(px(14.0)).text_color(rgb(0xfbbf24))),
                )
            })
            .when_some(m.badge, |el, badge| {
                el.child(app_badge(SharedString::from(format!("badge|{}", m.id)), badge, p))
            })
    };
    // Encrypted messages say they're being encrypted while they go.
    let sending = if m.enc.is_some() { t("dms-calls.dm.pending.encrypting") } else { t("chat.pending.sending") };
    let stamp: SharedString = if m.pending { sending.clone().into() } else { when(m.at).into() };
    // A time shows the whole date while the pointer rests on it (the web's `title`).
    let full_time = |id: &str, el: Div| -> AnyElement {
        if m.pending {
            return el.into_any_element();
        }
        let full = crate::ui::text::full(m.at);
        el.id(SharedString::from(format!("{id}|{}", m.id)))
            .tooltip(move |window, cx| crate::ui::overlay::Tip::new(full.clone()).build(window, cx))
            .into_any_element()
    };
    let mut body = div().flex_1().min_w_0().flex().flex_col();
    if m.head && !compact {
        body = body.child(
            div()
                .flex()
                .items_baseline()
                .gap(px(8.0))
                .h(px(24.0))
                .line_height(px(24.0))
                .child(author_name())
                .when_some(m.from.as_ref(), |el, from| {
                    el.child(div().self_center().child(crate::ui::shared_marks::server_tag(from, &ctx.url, p)))
                })
                .child(full_time(
                    "stamp",
                    div().flex_none().text_xs().text_color(p.muted_foreground).child(stamp.clone()),
                )),
        );
    }
    // Everything under the name is the web's `.chat-text`: 15px on a 1.625 line.
    let mut text = div()
        .flex()
        .flex_col()
        .text_size(px(crate::ui::theme::chat_font()))
        .line_height(px(crate::ui::theme::chat_font() * 1.625));
    // An encrypted line says it under its words, as the web's `SecureAlsoSent`.
    let also = m.thread.also_in.is_some() || m.thread.also_sent;
    if also && m.enc.is_none() {
        text = text.child(crate::ui::threads::also_note(&m.id, m.thread.also_in.as_deref(), p, &ctx.this));
    }
    if let Some(used) = m.agent.as_ref().and_then(|a| crate::ui::commands::used_line(&m.id, a, p)) {
        text = text.child(used);
    }
    let compact_head = compact.then(|| {
        // Compact: the time and the name in front of every message, on its line.
        div()
            .flex_none()
            .flex()
            .items_baseline()
            .gap(px(6.0))
            .child(full_time(
                "stamp",
                div()
                    .w(px(48.0))
                    .mr(px(2.0))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .text_size(px(10.5))
                    .whitespace_nowrap()
                    .text_color(p.muted_foreground)
                    .child(if m.pending { sending.clone() } else { clock(m.at) }),
            ))
            .child(author_name())
            .when_some(m.from.as_ref(), |el, from| {
                el.child(div().self_center().child(crate::ui::shared_marks::server_tag(from, &ctx.url, p)))
            })
    });
    let dim = m.pending && m.failed.is_none();
    text = text.child(
        div()
            .flex()
            .items_baseline()
            .children(compact_head)
            .child(div().flex_1().min_w_0().when(dim, |el| el.opacity(0.55)).child(content)),
    );
    // "(edited)" and the pin go on the line after the text, as on the web.
    let shared = m.enc.as_ref().is_some_and(|e| e.shared) && !deleted;
    if (m.edited && !m.editing && !deleted) || m.pinned || shared {
        text = text.child(
            div()
                .flex()
                .items_center()
                .h(px(crate::ui::theme::chat_font() * 1.625))
                .when(m.edited && !m.editing, |el| {
                    el.child(
                        div()
                            .id(SharedString::from(format!("edited|{}", m.id)))
                            .text_size(px(11.2))
                            .text_color(p.muted_foreground)
                            // When it was edited, as the web's `title`: looked up only once pointed at.
                            .when(m.enc.is_none(), |el| {
                                let (this, key, id) = (ctx.this.clone(), ctx.key.clone(), m.id.clone());
                                el.tooltip(move |window, cx| {
                                    let at = edited_at(&this, &key, &id, cx);
                                    let tip = at.map_or_else(|| t("chat.messages.edited"), crate::ui::text::full);
                                    crate::ui::overlay::Tip::new(tip).build(window, cx)
                                })
                            })
                            .child(t("chat.messages.edited")),
                    )
                })
                .when(shared, |el| el.child(div().ml(px(6.0)).child(crate::ui::dm_view::shared_pill(&m.id, p))))
                .when(m.pinned, |el| el.child(div().ml(px(6.0)).child(crate::ui::pins::pin_mark(&m.id, p)))),
        );
    }
    if let Some(enc) = m.enc.as_ref().filter(|e| !e.files.is_empty()) {
        text = text.child(crate::ui::sealed_files::files_view(&m.id, &enc.files, p, &ctx.this, &ctx.key, _cx));
    }
    if let Some(enc) = m.enc.as_ref().filter(|e| !e.sending.is_empty()) {
        text = text.child(crate::ui::sealed_files::pending_files(&enc.sending, p));
    }
    if !m.attachments.is_empty() {
        text = text.child(crate::ui::attachments::attachments_view(
            &m.id,
            &m.attachments,
            &ctx.url,
            p,
            &ctx.this,
            &ctx.key,
            ctx.fresh.contains_key(&m.id),
        ));
    }
    if !m.embeds.is_empty() {
        text = text.child(crate::ui::embeds::embeds(&m.id, &m.embeds, p, ctx.fresh.contains_key(&m.id)));
    }
    if let Some(gif) = m.gif.as_ref().filter(|g| !g.url.is_empty()) {
        let starred = ctx.saved_gifs.contains(&gif.url);
        text = text.child(crate::ui::gifs::gif_in_message(&m.id, gif, starred, p, &ctx.this, &ctx.key));
    }
    if also && m.enc.is_some() {
        text = text.child(crate::ui::threads::also_note(&m.id, m.thread.also_in.as_deref(), p, &ctx.this));
    }
    if let Some(replies) = &m.thread.replies {
        text = text.child(crate::ui::threads::replies_row(&m.id, replies, p, &ctx.this));
    }
    if let Some(card) = &m.poll {
        text = text.child(crate::ui::polls::poll_card(&m.id, card, p, &ctx.this, ctx.fresh.contains_key(&m.id)));
    }
    if let Some(buttons) = m
        .agent
        .as_ref()
        .and_then(|a| crate::ui::commands::buttons_view(&m.id, a, p, &ctx.this, &ctx.key, ctx.server.as_deref()))
    {
        text = text.child(buttons);
    }
    // Drawn even with none yet, so the first one to come springs in.
    if let Some(bits) = m.reactions.as_ref().filter(|_| !deleted && !m.pending) {
        text = text.child(crate::ui::reactions::ReactionRow {
            msg: reacts_as(&m.id),
            bits: bits.clone(),
            this: ctx.this.clone(),
            key: ctx.key.clone(),
        });
    }
    if let Some(reason) = &m.failed {
        text = text.child(failed_line(m, reason, blocked.as_deref(), p, ctx));
    }
    body = body.child(text);

    let lit = author.is_some();
    let left: Option<AnyElement> = if compact {
        None
    } else if m.head {
        Some(
            div()
                .id(SharedString::from(format!("face|{}", m.id)))
                .w(px(40.0))
                .flex_none()
                .mt(px(2.0))
                // The web's `active:scale-95`.
                .when(author.is_some(), |el| el.cursor_pointer().active(|s| s.scale(0.95)))
                .when_some(author, |el, id| {
                    let ctx = ctx.clone();
                    el.child(crate::ui::profile_card::mark(&id, crate::ui::profile_card::Side::Right))
                        .on_mouse_down(MouseButton::Right, person_menu(&ctx, id.clone()))
                        .on_click(move |_, window, cx| open_profile(&ctx, id.clone(), window, cx))
                })
                .child(crate::ui::widgets::decorated(avatar(m.user.as_ref(), 40.0, p), 40.0, m.decoration.as_deref()))
                // The web's `hover:brightness-110`, as a faint white veil over the face.
                .when(lit, |el| {
                    el.relative().group(SharedString::from("face")).child(
                        div()
                            .id(SharedString::from(format!("face-lit|{}", m.id)))
                            .absolute()
                            .top_0()
                            .left_0()
                            .size(px(40.0))
                            .rounded_full()
                            .bg(gpui_kit::white())
                            .opacity(0.0)
                            .group_hover("face", |s| s.opacity(0.08)),
                    )
                })
                .into_any_element(),
        )
    } else {
        // The web's gutter time: 10px, right-aligned, reaching 12px into the gap, fading in on hover.
        let shown = !m.pending && ctx.hover.as_deref() == Some(crate::ui::chat_rows::hover_key(&m.id, ctx).as_str());
        Some(
            div()
                .w(px(40.0))
                .flex_none()
                .pt(px(4.0))
                .flex()
                .justify_end()
                .text_size(px(10.0))
                .line_height(px(15.0))
                .whitespace_nowrap()
                .text_color(p.muted_foreground)
                .when(shown, |el| {
                    el.child(motion::fade_in(
                        div().child(full_time("gutter", div().child(clock(m.at)))),
                        SharedString::from(format!("gutter-in|{}", m.id)),
                        Duration::from_millis(150),
                    ))
                })
                .into_any_element(),
        )
    };

    let tools = message_tools(m, p, ctx);

    // The web's .mention-me: the primary at 12% and a 3px bar inside the left edge.
    let ping = alpha(p.primary, 0.12);
    let bar = p.primary;
    let lit = ctx.lit.as_deref().and_then(|l| l.strip_prefix("msg|")) == Some(m.id.as_str());
    let menu_of = {
        let (msg, thread, this) = (m.clone(), ctx.thread.clone(), ctx.this.clone());
        move |window: &mut Window, cx: &mut App| {
            // A picture right-clicked in it says so first (see `attachments_view`).
            let picture = this
                .update(cx, |this, _| this.right_picture.take())
                .ok()
                .flatten()
                .filter(|(id, _)| *id == msg.id)
                .map(|(_, n)| n);
            let selection = gpui_kit::base::TextSelection::selected_text(window, cx);
            MenuOf::Message { msg: msg.clone(), thread: thread.clone(), picture, selection }
        }
    };
    let hover_of = {
        let (msg, thread, this) = (m.clone(), ctx.thread.clone(), ctx.this.clone());
        let key = crate::ui::chat_rows::hover_key(&m.id, ctx);
        move |hovered: &bool, _: &mut Window, cx: &mut App| {
            let of =
                MenuOf::Message { msg: msg.clone(), thread: thread.clone(), picture: None, selection: String::new() };
            let id = key.clone();
            let _ = this.update(cx, |this, cx| {
                this.set_hover_target(of, *hovered);
                this.hover_row(&id, *hovered, false, cx);
            });
        }
    };
    div()
        .id(SharedString::from(format!("msg|{}", m.id)))
        .group("msg")
        .on_mouse_down(
            MouseButton::Right,
            context_menu::on_right_click(ctx.this.clone(), move |w, cx| Some(menu_of(w, cx))),
        )
        .on_hover(hover_of)
        .relative()
        .flex()
        .px(px(16.0))
        // The web's spacing (app.css, at the density's scale): 2px around each
        // line, and a run's first line 12px down with 4px more above it.
        .map(|el| {
            if compact {
                if m.head { el.mt(px(6.0)).pt(px(2.0)).pb(px(1.0)) } else { el.py(px(1.0)) }
            } else if m.head {
                el.mt(px(12.0)).pt(px(4.0)).pb(px(2.0))
            } else {
                el.py(px(2.0))
            }
        })
        .map(|el| {
            if lit {
                el.bg(held)
            } else if m.mentions_me {
                el.bg(ping).hover(move |s| s.bg(hover))
            } else {
                el.hover(move |s| s.bg(hover))
            }
        })
        .when(m.mentions_me, |el| el.child(div().absolute().left_0().top_0().bottom_0().w(px(3.0)).bg(bar)))
        .children(left)
        .gap(px(12.0))
        .child(body)
        .children(tools)
        .into_any_element()
}

/// The message a row's reactions are to: a secure thread's top copy
/// (`top|<record>`) reacts to the message itself.
fn reacts_as(id: &str) -> String {
    id.strip_prefix("top|").unwrap_or(id).to_owned()
}

/// The web's chat Markdown (`.markdown.chat` in app.css): code on the muted
/// colour, tight paragraphs, quotes behind a bar in the primary, headings no
/// bigger than the page's own.
pub(crate) fn chat_markdown(p: &Palette) -> gpui_kit::base::TextViewStyle {
    let code = gpui_kit::StyleRefinement::default()
        .rounded(corner(12.0))
        .px(px(13.125))
        .py(px(9.84))
        .my(px(5.25))
        .text_size(px(13.125))
        .line_height(px(21.0));
    gpui_kit::base::TextViewStyle::default()
        .with_foreground(p.foreground.into())
        .with_muted_foreground(p.muted_foreground.into())
        .with_link(p.primary.into())
        .with_selection(alpha(p.primary, 0.3))
        .with_code_background(p.muted.into())
        // Quotes' bar (`.markdown blockquote`); tables and rules share it.
        .with_border(p.primary.into())
        .with_paragraph_gap(gpui_kit::rems(0.3))
        .with_heading(|level| {
            let size = if level <= 2 { 17.25 } else { 15.0 };
            gpui_kit::StyleRefinement::default()
                .text_size(px(size))
                .line_height(px(size * 1.3))
                .font_weight(FontWeight::EXTRA_BOLD)
        })
        .with_code_block(code)
        .with_inline_code(gpui_kit::HighlightStyle { background_color: Some(p.muted.into()), ..Default::default() })
        .with_dark(p.dark)
}

/// Under a message that didn't go: why, and what to do (`PendingRow`'s ends).
/// When a message on the instance `key` was last edited, if it's loaded.
fn edited_at(this: &WeakEntity<FuwaApp>, key: &str, id: &str, cx: &App) -> Option<i64> {
    let app = this.upgrade()?;
    app.read(cx).core.shared.read(|s| {
        let i = s.instance(key)?;
        let m = i.messages.values().find_map(|c| c.items.iter().rev().find(|m| m.id == id))?;
        m.edited_at.as_ref().map(|t| ms_of(Some(t)))
    })
}

fn failed_line(m: &Msg, reason: &str, blocked: Option<&str>, p: &Palette, ctx: &Rc<RowCtx>) -> AnyElement {
    let nonce = m.nonce;
    let dismiss = {
        let (this, thread) = (ctx.this.clone(), ctx.thread.clone());
        div()
            .id(SharedString::from(format!("dismiss|{nonce}")))
            .font_weight(FontWeight::BOLD)
            .text_color(p.muted_foreground)
            .cursor_pointer()
            .hover(|s| s.underline())
            .on_click(move |_, _, cx| {
                let _ = this.update(cx, |this, _| match this.target() {
                    Some(Target::Channel { key, channel, .. }) => {
                        let at = thread.as_deref().map_or(channel, crate::core::threads::thread_key);
                        this.core.dismiss_pending(&key, &at, nonce);
                    }
                    Some(Target::Dm { key, conversation: id } | Target::Secure { key, channel: id, .. }) => {
                        this.core.dismiss_dm(&key, &id, nonce);
                    }
                    None => {}
                });
            })
            .child(t("chat.pending.dismiss"))
    };
    let upper = |s: &str| {
        let mut c = s.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    };
    if let Some(why) = blocked {
        let amber = rgb(0xf59e0b);
        let content = m.content.clone();
        let this = ctx.this.clone();
        return motion::rise(
            div()
                .mt(px(6.0))
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(8.0))
                .px(px(12.0))
                .py(px(8.0))
                .rounded(crate::ui::theme::radius_xl())
                .border_1()
                .border_color(alpha(amber, 0.3))
                .bg(alpha(amber, 0.1))
                .text_xs()
                .line_height(px(16.0))
                .text_color(p.foreground)
                // The shield gives a little shake (`rotate: [0, -12, 12, -6, 0]`).
                .child(motion::once(
                    div().child(icon("shield-alert").size(px(16.0)).text_color(rgb(if p.dark {
                        0xfbbf24
                    } else {
                        0xd97706
                    }))),
                    SharedString::from(format!("blocked-shake|{nonce}")),
                    Duration::from_millis(750),
                    |el, t| {
                        const KEYS: [f32; 5] = [0.0, -12.0, 12.0, -6.0, 0.0];
                        let x = ((t - 0.2) / 0.8).clamp(0.0, 1.0) * 4.0;
                        let n = (x.floor() as usize).min(3);
                        el.rotate(gpui_kit::radians((KEYS[n] + (KEYS[n + 1] - KEYS[n]) * (x - n as f32)).to_radians()))
                    },
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_wrap()
                        .gap_x(px(4.0))
                        .child(div().font_weight(FontWeight::BOLD).child(t("chat.pending.blocked")))
                        .child(div().text_color(p.muted_foreground).child(upper(why))),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("blocked-copy|{nonce}")))
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.primary)
                        .cursor_pointer()
                        .hover(|s| s.underline())
                        .on_click(move |_, _, cx| {
                            let text = content.clone();
                            let _ = this.update(cx, |_, cx| {
                                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
                            });
                        })
                        .child(icon("copy").size(px(12.0)))
                        .child(t("chat.messages.copyText")),
                )
                .child(dismiss),
            SharedString::from(format!("blocked|{nonce}")),
            Duration::ZERO,
            -4.0,
        )
        .into_any_element();
    }
    let (this, thread) = (ctx.this.clone(), ctx.thread.clone());
    div()
        .mt(px(4.0))
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(8.0))
        .text_xs()
        .line_height(px(16.0))
        .child(div().text_color(p.destructive).child(format!("{}.", upper(reason.trim_end_matches('.')))))
        .child(
            div()
                .id(SharedString::from(format!("retry|{nonce}")))
                .flex()
                .items_center()
                .gap(px(4.0))
                .font_weight(FontWeight::BOLD)
                .text_color(p.primary)
                .cursor_pointer()
                .hover(|s| s.underline())
                .on_click(move |_, _, cx| {
                    let _ = this.update(cx, |this, cx| this.retry(nonce, thread.clone(), cx));
                })
                .child(icon("rotate-cw").size(px(12.0)))
                .child(t("chat.pending.retry")),
        )
        .child(dismiss)
        .into_any_element()
}

/// The tools over a hovered message (`MessageTools`), in the web's order:
/// copy text, its thread, pin, copy ID (developer mode), edit, keep out,
/// delete; or the question when deleting it (or keeping its author out) needs a yes.
fn message_tools(m: &Rc<Msg>, p: &Palette, ctx: &Rc<RowCtx>) -> Option<AnyElement> {
    use crate::ui::chat_rows::{confirm_delete, tool, tool_with, tools_frame, tools_in, tools_shown};
    if m.pending || m.editing || m.unreadable || m.enc.as_ref().is_some_and(|e| e.deleted) {
        return None;
    }
    let deleting = ctx.deleting.as_deref() == Some(m.id.as_str());
    if !(tools_shown(&m.id, ctx) || m.keeping_out || deleting) {
        return None;
    }
    let id = m.id.clone();
    let frame = tools_frame(&id, ctx, p);
    if deleting && m.can_delete {
        // An encrypted message asks whom it goes for, and offers to keep it.
        let confirm = match &m.enc {
            Some(enc) => {
                crate::ui::chat_rows::confirm_delete_as(&id, &enc.question, t("dms-calls.dm.row.keep"), ctx, p)
            }
            None => confirm_delete(&id, t("common.cancel"), ctx, p),
        };
        return Some(tools_in(frame.child(confirm), &id));
    }
    if m.keeping_out {
        let (yes, no) = (ctx.this.clone(), ctx.this.clone());
        let (author, name) = (m.user.as_ref().map(|u| u.id.clone()).unwrap_or_default(), m.name.clone());
        return Some(tools_in(
            frame.child(motion::slide_in(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .child(
                        div()
                            .px(px(8.0))
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(p.destructive)
                            .child(crate::core::i18n::t_with("chat.messages.keepOutAsk", &[("name", Arg::Str(&name))])),
                    )
                    .child(tool(format!("keep-yes|{id}"), "check", t("chat.messages.keepOut"), true, p).on_click(
                        move |_, _, cx| {
                            let _ = yes.update(cx, |this, cx| this.keep_out(author.clone(), name.clone(), cx));
                        },
                    ))
                    .child(tool(format!("keep-no|{id}"), "x", t("common.cancel"), false, p).on_click(
                        move |_, _, cx| {
                            let _ = no.update(cx, |this, cx| {
                                this.keeping_out = None;
                                this.sync_list(cx);
                                cx.notify();
                            });
                        },
                    )),
                SharedString::from(format!("keep-ask|{id}")),
                8.0,
            )),
            &id,
        ));
    }
    let mut frame = frame;
    if !m.content.trim().is_empty() && m.voice.is_none() {
        let copied = ctx.copied.as_deref() == Some(id.as_str());
        let (this, mid, text) = (ctx.this.clone(), id.clone(), m.content.clone());
        let label = if copied { t("chat.messages.copied") } else { t("chat.messages.copyText") };
        // The check pops in, turning upright, as the web's `CopyTextButton` does.
        let glyph = if copied {
            motion::pop(
                div().child(icon("check").size(px(16.0)).text_color(p.primary)),
                SharedString::from(format!("copied|{id}")),
                0.3,
                -45.0,
                Duration::ZERO,
            )
            .into_any_element()
        } else {
            icon("copy").size(px(16.0)).into_any_element()
        };
        let button = tool_with(format!("copy|{id}"), glyph, label, false, p).on_click(move |_, _, cx| {
            let _ = this.update(cx, |this, cx| this.copy_message_text(mid.clone(), text.clone(), cx));
        });
        frame = frame.child(button);
    }
    if m.thread.can_thread {
        let (this, mid) = (ctx.this.clone(), id.clone());
        let label =
            if m.thread.replies.is_some() { t("chat.messages.openThread") } else { t("chat.messages.replyInThread") };
        frame = frame.child(tool(format!("thread|{id}"), "message-square-reply", label, false, p).on_click(
            move |_, window, cx| {
                let _ = this.update(cx, |this, cx| this.open_thread(mid.clone(), window, cx));
            },
        ));
    }
    if let Some(bits) = m.reactions.as_ref().filter(|r| r.can_add) {
        let (this, mid, standard_only) = (ctx.this.clone(), reacts_as(&id), bits.standard_only);
        frame = frame.child(
            tool(format!("react|{id}"), "face-slightly-smiling-plus", t("chattools.reactions.add"), false, p).on_click(
                move |ev, window, cx| {
                    let at = ev.position();
                    let _ =
                        this.update(cx, |this, cx| this.open_react_picker(mid.clone(), standard_only, at, window, cx));
                },
            ),
        );
    }
    if m.can_pin {
        let (this, mid, pinned, in_thread) = (ctx.this.clone(), id.clone(), m.pinned, ctx.thread.is_some());
        let label = if pinned { t("chattools.pins.unpinMessage") } else { t("chattools.pins.pin") };
        frame =
            frame.child(tool(format!("pin|{id}"), if pinned { "pin-off" } else { "pin" }, label, false, p).on_click(
                move |_, _, cx| {
                    let _ = this.update(cx, |this, cx| this.toggle_pin(mid.clone(), !pinned, in_thread, cx));
                },
            ));
    }
    if ctx.developer && ctx.server.is_some() {
        let (this, mid) = (ctx.this.clone(), id.clone());
        frame = frame.child(
            tool(format!("copy-id|{id}"), "fingerprint-pattern", t("chat.messages.copyId"), false, p).on_click(
                move |_, _, cx| {
                    let _ = this.update(cx, |this, cx| context_menu::copy(this, mid.clone(), "message ID", cx));
                },
            ),
        );
    }
    let can_edit = m.mine && m.poll.is_none() && m.voice.is_none();
    if can_edit {
        let (this, mid, in_thread) = (ctx.this.clone(), id.clone(), ctx.thread.is_some());
        frame = frame.child(tool(format!("edit|{id}"), "pencil", t("chat.messages.edit"), false, p).on_click(
            move |_, window, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.start_edit(mid.clone(), window, cx);
                    this.edit_in_thread = in_thread;
                    this.sync_list(cx);
                    this.sync_thread(cx);
                });
            },
        ));
    }
    if m.keep_out {
        let (this, mid) = (ctx.this.clone(), id.clone());
        frame = frame.child(tool(format!("keep|{id}"), "user-x", t("chat.messages.keepOut"), true, p).on_click(
            move |_, _, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.keeping_out = Some(mid.clone());
                    this.sync_list(cx);
                    cx.notify();
                });
            },
        ));
    }
    if m.can_delete {
        let (this, mid) = (ctx.this.clone(), id.clone());
        frame = frame.child(tool(format!("del|{id}"), "trash", t("chat.messages.delete"), true, p).on_click(
            move |_, _, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.msg_ui.deleting = Some(mid.clone());
                    cx.notify();
                });
            },
        ));
    }
    Some(tools_in(frame, &id))
}

/// Editing in place: Enter saves, Escape stops.
fn edit_box(m: &Msg, p: &Palette, ctx: &Rc<RowCtx>) -> impl IntoElement {
    let (save, cancel) = (ctx.this.clone(), ctx.this.clone());
    let link = |id: &str, label: String, p: &Palette| {
        div()
            .id(SharedString::from(format!("{id}|{}", m.id)))
            .text_color(p.primary)
            .font_weight(FontWeight::BOLD)
            .cursor_pointer()
            .hover(|s| s.underline())
            .child(label)
    };
    // "Escape to {cancel} · {keys} to {save}", with cancel and save as links.
    // Filled with its own placeholders, which are split out below.
    let template = crate::core::i18n::t_with(
        "chat.edit.hint",
        &[("cancel", Arg::Str("{cancel}")), ("keys", Arg::Str("{keys}")), ("save", Arg::Str("{save}"))],
    );
    let mut hint =
        div().flex().flex_wrap().items_center().text_xs().line_height(px(16.0)).text_color(p.muted_foreground);
    let mut rest = template.as_str();
    let (mut save, mut cancel) = (Some(save), Some(cancel));
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}').map(|c| open + c) else { break };
        if open > 0 {
            hint = hint.child(div().whitespace_nowrap().child(rest[..open].to_owned()));
        }
        hint = match &rest[open + 1..close] {
            "cancel" => match cancel.take() {
                Some(this) => hint.child(link("cancel", t("chat.edit.cancel"), p).on_click(move |_, window, cx| {
                    let _ = this.update(cx, |this, cx| this.cancel_edit(window, cx));
                })),
                None => hint,
            },
            "save" => match save.take() {
                Some(this) => hint.child(link("save", t("chat.edit.save"), p).on_click(move |_, window, cx| {
                    let _ = this.update(cx, |this, cx| this.save_edit(window, cx));
                })),
                None => hint,
            },
            "keys" => hint.child(div().whitespace_nowrap().child(ctx.send_keys.clone())),
            other => hint.child(other.to_owned()),
        };
        rest = &rest[close + 1..];
    }
    if !rest.is_empty() {
        hint = hint.child(div().whitespace_nowrap().child(rest.to_owned()));
    }
    // The web's `.composer` box, lit as it is while focused.
    let ring = mix(p.border, p.primary, 0.6);
    div()
        .mt(px(4.0))
        .flex()
        .flex_col()
        .child(
            div()
                .w_full()
                .px(px(12.0))
                .py(px(8.0))
                .rounded(crate::ui::theme::radius_xl())
                .bg(p.card)
                .border_1()
                .border_color(ring)
                .shadow(vec![
                    gpui_kit::BoxShadow {
                        color: alpha(p.primary, 0.14),
                        offset: gpui_kit::point(px(0.0), px(0.0)),
                        blur_radius: px(0.0),
                        spread_radius: px(4.0),
                        inset: false,
                    },
                    gpui_kit::BoxShadow {
                        color: p.primary.into(),
                        offset: gpui_kit::point(px(0.0), px(12.0)),
                        blur_radius: px(30.0),
                        spread_radius: px(-18.0),
                        inset: false,
                    },
                ])
                .child(
                    div().mx(px(-10.0)).my(px(-8.0)).child(
                        Textarea::new(&ctx.edit_box).appearance(false).text_size(px(15.2)).line_height(px(24.0)),
                    ),
                ),
        )
        .child(hint)
}

/// Home, with nothing open: the cloud bobbing, and what lives here.
impl FuwaApp {
    /// Where a server's channels were, for a member whose single sign-on is
    /// missing or ran out: a padlock swinging inside rings that ripple out,
    /// and the way back in. They stay a member; the channels come back
    /// through the event stream once they sign in (`SsoGate.tsx` on the web).
    fn sso_gate(
        &mut self,
        key: &str,
        server: &pb::Server,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let provider = crate::ui::overlay::provider_name(&server.sso_name).to_owned();
        let days = server.sso_recheck_days;
        let every = match days {
            0 => String::new(),
            1 => " every day".into(),
            n => format!(" every {n} days"),
        };
        let waiting = self.sso_waiting.as_deref() == Some(server.id.as_str());
        const BADGE: f32 = 80.0;
        const AREA: f32 = 132.0;
        let ring = |n: u64| {
            let el = div().absolute().rounded_full().border_2().border_color(alpha(p.primary, 0.4));
            motion::ambient(el, SharedString::from(format!("sso-ring-{n}")), Duration::from_millis(2400), window, {
                move |el, t| {
                    // Two rings half a beat apart, each growing from the badge and fading out.
                    let t = (t + n as f32 * 0.5) % 1.0;
                    let eased = 1.0 - (1.0 - t).powi(3);
                    let size = BADGE * (1.0 + 0.65 * eased);
                    let at = (AREA - size) / 2.0;
                    el.left(px(at)).top(px(at)).size(px(size)).opacity(0.7 * (1.0 - t))
                }
            })
        };
        let padlock = motion::ambient(
            icon("lock-keyhole").size(px(36.0)),
            "sso-swing",
            Duration::from_millis(3600),
            window,
            |el, t| {
                // A swing on its chain for the first part of each beat, then rest.
                let k = (t / 0.4).min(1.0);
                let swing = (k * std::f32::consts::TAU * 2.0).sin() * (1.0 - k) * 0.16;
                el.rotate(gpui_kit::radians(swing))
            },
        );
        let badge = div()
            .absolute()
            .left(px((AREA - BADGE) / 2.0))
            .top(px((AREA - BADGE) / 2.0))
            .size(px(BADGE))
            .rounded(px(28.0))
            .flex()
            .items_center()
            .justify_center()
            // Solid, so the rings pass behind it rather than through it.
            .bg(mix(p.background, p.primary, 0.15))
            .text_color(p.primary)
            .shadow(vec![gpui_kit::BoxShadow {
                color: alpha(p.primary, 0.3),
                offset: gpui_kit::point(px(0.0), px(20.0)),
                blur_radius: px(40.0),
                spread_radius: px(-24.0),
                inset: false,
            }])
            .child(padlock);
        let (k, sid) = (key.to_owned(), server.id.clone());
        let button = primary_button("sso-gate-sign-in", "", p)
            .w_full()
            .when(waiting, |el| el.opacity(0.75))
            .child(if waiting {
                motion::ambient(
                    icon("loader-circle").size(px(18.0)),
                    "sso-wait",
                    Duration::from_millis(900),
                    window,
                    |el, t| el.rotate(gpui_kit::radians(t * std::f32::consts::TAU)),
                )
            } else {
                icon("building").size(px(18.0)).into_any_element()
            })
            .child(if waiting { "Waiting for your browser…".to_owned() } else { format!("Continue with {provider}") })
            .on_click(cx.listener(move |this, _, _, cx| this.sign_in_server(k.clone(), sid.clone(), cx)));
        let tag = format!("{key}|{}", server.id);
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .p(px(24.0))
            .child(
                div()
                    .w_full()
                    .max_w(px(384.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(16.0))
                    .child(motion::rise(
                        div().relative().size(px(AREA)).child(ring(0)).child(ring(1)).child(badge),
                        SharedString::from(format!("sso-badge|{tag}")),
                        Duration::ZERO,
                        20.0,
                    ))
                    .child(motion::rise(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(6.0))
                            .text_center()
                            .child(
                                div()
                                    .flex()
                                    .justify_center()
                                    .text_xl()
                                    .font_weight(FontWeight::EXTRA_BOLD)
                                    .child(tracked(format!("Sign in with {provider}"), TIGHT)),
                            )
                            .child(div().text_sm().text_color(p.muted_foreground).child(format!(
                                "{} asks members to sign in through {provider}{every}. You're still a member; the channels come back once you do.",
                                server.name
                            ))),
                        SharedString::from(format!("sso-words|{tag}")),
                        Duration::from_millis(60),
                        14.0,
                    ))
                    .when(!server.sso_host.is_empty(), |el| {
                        el.child(motion::rise(
                            div().w_full().child(crate::ui::overlay::host_notice(&server.sso_host, p)),
                            SharedString::from(format!("sso-host|{tag}")),
                            Duration::from_millis(150),
                            8.0,
                        ))
                    })
                    .child(motion::rise(
                        div().w_full().child(button),
                        SharedString::from(format!("sso-go|{tag}")),
                        Duration::from_millis(210),
                        8.0,
                    )),
            )
            .into_any_element()
    }
}

fn home_splash(p: &Palette, window: &Window) -> impl IntoElement {
    let bob = fuwa_mark(96.0, p);
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(14.0))
        .child(motion::ambient(div().child(bob), "splash-bob", Duration::from_millis(3200), window, |el, t| {
            el.relative().top(px((t * std::f32::consts::TAU).sin() * 6.0))
        }))
        .child(motion::rise(
            div().text_2xl().font_weight(FontWeight::EXTRA_BOLD).child("Your private messages"),
            "splash-title",
            Duration::from_millis(80),
            12.0,
        ))
        .child(motion::rise(
            div()
                .max_w(px(440.0))
                .text_center()
                .text_color(p.muted_foreground)
                .child("Pick a conversation on the left, or message a friend or someone from a server's member list. Everything here is end-to-end encrypted: only your devices and theirs can read it."),
            "splash-body",
            Duration::from_millis(160),
            12.0,
        ))
}

/// A quiet message in the middle of a pane.
fn empty_state(p: &Palette, glyph: &str, title: &str, body: &str) -> impl IntoElement {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(10.0))
        .child(
            div()
                .size(px(64.0))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(alpha(p.primary, 0.12))
                .text_color(p.primary)
                .child(icon(glyph).size(px(28.0))),
        )
        .child(div().text_lg().font_weight(FontWeight::EXTRA_BOLD).child(title.to_owned()))
        .when(!body.is_empty(), |el| el.child(div().text_color(p.muted_foreground).child(body.to_owned())))
}

/// An emoji to pick: a server's own picture, or the character.
pub(crate) fn emoji_glyph(choice: &crate::ui::emoji::Choice, size: f32) -> AnyElement {
    let base = div().size(px(size + 2.0)).flex_none().flex().items_center().justify_center();
    match &choice.url {
        Some(url) => base
            .child({
                use gpui_kit::StyledImage as _;
                crate::ui::widgets::picture(url.clone()).size(px(size)).object_fit(gpui_kit::ObjectFit::Contain)
            })
            .into_any_element(),
        None => base.in_color().text_size(px(size * 0.85)).child(choice.insert.clone()).into_any_element(),
    }
}

/// Who a webhook message says it's from, drawn like a person with no profile.
fn webhook_author(w: &pb::MessageWebhook) -> pb::User {
    pb::User {
        id: w.webhook_id.clone(),
        username: w.name.clone(),
        display_name: w.name.clone(),
        avatar_url: w.avatar_url.clone(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::changed_rows;

    #[test]
    fn only_changed_rows_are_measured_again() {
        // A message added at the end that changes the one before it (its header).
        assert_eq!(changed_rows(&[1, 2, 3], &[1, 2, 9, 4], 0), vec![2..3]);
        // Older messages added at the top: the old rows moved down by two.
        assert_eq!(changed_rows(&[1, 2, 3], &[7, 8, 5, 2, 3], 2), vec![2..3]);
        // Two edits side by side and one further on.
        assert_eq!(changed_rows(&[1, 2, 3, 4, 5], &[1, 9, 9, 4, 9], 0), vec![1..3, 4..5]);
        assert!(changed_rows(&[1, 2], &[1, 2], 0).is_empty());
    }
}
