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
use gpui_kit::component::text::{TextView, TextViewStyle};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Div, Focusable as _, FontWeight, Hsla, InteractiveElement as _,
    IntoElement, MouseButton, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _,
    WeakEntity, Window, div, px, rgb,
};

use crate::core::config::Density;
use crate::core::shared;
use crate::core::store::{Connection, InstanceState, user_name};
use crate::core::vault::ItemKind;
use crate::pb;
use crate::ui::app::{Dialog, FuwaApp, Menu, Nav, Target};
use crate::ui::context_menu::{self, MenuOf};
use crate::ui::members::{MembersEvent, MembersView};
use crate::ui::mentions::{Look, Pick, SCHEME, mention_links};
use crate::ui::motion;
use crate::ui::text::{clock, images_as_links, ms_of, when};
use crate::ui::theme::{Palette, alpha, corner, mix};
use crate::ui::timestamps::timestamp_nodes;
use crate::ui::widgets::{
    app_badge, avatar, card, conn_dot, error_line, fuwa_mark, icon, icon_button, icon_button_in, is_agent, pal,
    primary_button, soft_button,
};

/// Who wrote something in a conversation or a secure channel, as shown.
pub(crate) struct Who {
    pub name: String,
    pub color: Option<Hsla>,
    pub user: Option<pb::User>,
}

fn plain_msg(id: String, who: Who, content: String, at: i64, mine: bool) -> Msg {
    Msg {
        id,
        shown: timestamp_nodes(&images_as_links(&content)),
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
        thread: ThreadBits::default(),
        sig: 0,
    }
}

/// The rows of a conversation or a secure channel (`moderate` is Some for a
/// channel: whether you may delete others' messages there), from what this
/// device opened.
pub(crate) fn encrypted_rows(
    i: &InstanceState,
    id: &str,
    start: Row,
    who: &dyn Fn(&str) -> Who,
    moderate: Option<bool>,
    editing: Option<&str>,
) -> Vec<Row> {
    let me = i.me.clone();
    let me_id = me.as_ref().map(|m| m.id.clone()).unwrap_or_default();
    let items = i.dms.items.get(id).map(Vec::as_slice).unwrap_or_default();
    let channel = moderate.is_some();
    let shared = items.iter().any(|it| !it.shared_by.is_empty());
    let mut rows = vec![start];
    for item in items {
        let mine = item.sender_id == me_id;
        let can_delete = mine || moderate == Some(true);
        match item.kind {
            ItemKind::Text if !item.deleted => {
                let mut m = plain_msg(item.seq.to_string(), who(&item.sender_id), item.content.clone(), item.at, mine);
                m.editing = editing == Some(m.id.as_str());
                m.edited = item.edited_at > 0;
                m.can_delete = can_delete;
                m.voice = item.voice.as_ref().map(|v| Rc::new(crate::ui::voice_notes::VoiceCard::of(v)));
                rows.push(Row::Msg(Rc::new(m)));
            }
            ItemKind::Text => {}
            // A channel says it in a line; a conversation shows the message it couldn't open.
            ItemKind::Unreadable if !channel => {
                let text = "This message can't be opened on this device.".to_owned();
                let mut m = plain_msg(item.seq.to_string(), who(&item.sender_id), text, item.at, mine);
                m.shown = String::new();
                m.unreadable = true;
                m.can_delete = can_delete;
                rows.push(Row::Msg(Rc::new(m)));
            }
            ItemKind::Joined if !channel => rows.push(Row::Note {
                id: item.seq.to_string(),
                icon: "shield-check",
                text: "This device joined the conversation. What came before stays on the devices that were here."
                    .into(),
            }),
            // The devices a conversation started with aren't news.
            ItemKind::Devices if !channel && !rows.iter().any(|r| matches!(r, Row::Msg(_))) => {}
            ItemKind::Devices if !channel => {
                let added = item.added.iter().filter(|d| d.user_id != me_id || d.device_id != i.dms.device_id);
                let mut lines: Vec<String> = Vec::new();
                for d in added {
                    lines.push(format!("{} added a device", who(&d.user_id).name));
                }
                for d in &item.removed {
                    lines.push(format!("{} removed a device", who(&d.user_id).name));
                }
                if !lines.is_empty() {
                    lines.dedup();
                    rows.push(Row::Note { id: item.seq.to_string(), icon: "laptop", text: lines.join(" · ") });
                }
            }
            ItemKind::Reset | ItemKind::Setting if !channel => {}
            _ => {
                let name = |id: &str| who(id).name;
                let (icon, text) = crate::ui::secure::channel_line(item, &name, &me_id, shared);
                rows.push(Row::Note { id: item.seq.to_string(), icon, text });
            }
        }
    }
    for (n, text) in i.dms.sending.get(id).into_iter().flatten().enumerate() {
        let who = Who { name: me.as_ref().map(user_name).unwrap_or_default(), color: None, user: me.clone() };
        let mut m = plain_msg(format!("s{n}"), who, text.clone(), crate::core::dms::now_ms(), true);
        m.can_delete = false;
        m.pending = true;
        rows.push(Row::Msg(Rc::new(m)));
    }
    group(&mut rows);
    rows
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
    Msg(Rc<Msg>),
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
    /// Its thread, or the thread it's in.
    pub thread: ThreadBits,
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
            Row::Msg(m) => &m.id,
        }
    }

    fn digest(&self, h: &mut DefaultHasher) {
        match self {
            Row::Older { loading } => ("older", loading).hash(h),
            Row::Start { title, shared, .. } => ("start", title, shared).hash(h),
            Row::Note { id, text, .. } => (id, text).hash(h),
            Row::Divider { text } => ("divider", text).hash(h),
            Row::Msg(m) if m.sig != 0 => (m.sig, m.head).hash(h),
            Row::Msg(m) => {
                (&m.id, &m.shown, m.edited, m.head, m.pending, &m.failed, &m.name, m.editing, m.mentions_me).hash(h);
                m.voice.as_ref().map(|v| v.digest()).hash(h);
            }
        }
    }
}

impl FuwaApp {
    /// What the open list shows, row by row. `built` keeps the messages
    /// from the last time, so a new message doesn't redo the whole channel.
    fn rows(&self, built: &mut Built) -> Vec<Row> {
        match self.target() {
            Some(Target::Channel { key, server, channel }) => self.core.shared.read(|s| match s.instance(&key) {
                Some(i) => self.server_rows(i, &key, &server, &channel, None, built),
                None => Vec::new(),
            }),
            Some(Target::Dm { key, conversation }) => self.core.shared.read(|s| {
                let Some(i) = s.instance(&key) else { return Vec::new() };
                let me = i.me.as_ref().map(|m| m.id.clone()).unwrap_or_default();
                let users: Vec<pb::User> =
                    i.dms.conversations.iter().find(|c| c.id == conversation).map(|c| c.users.clone()).unwrap_or_default();
                let person = |id: &str| users.iter().find(|u| u.id == id).cloned().or_else(|| i.users.get(id).cloned());
                let other = users.iter().find(|u| u.id != me).map(user_name).unwrap_or_else(|| "them".into());
                let start = Row::Start {
                    icon: "lock",
                    title: other.clone(),
                    body: format!(
                        "This is the start of your private conversation with {other}. It's end-to-end encrypted: only your devices and theirs can read it."
                    ),
                    shared: None,
                };
                let who = |id: &str| {
                    let user = person(id);
                    Who { name: user.as_ref().map(user_name).unwrap_or_else(|| "Someone".into()), color: None, user }
                };
                let mut rows = encrypted_rows(i, &conversation, start, &who, None, self.editing.as_deref());
                self.dress_voice(&mut rows);
                rows
            }),
            Some(Target::Secure { key, server, channel }) => self.core.shared.read(|s| {
                let Some(i) = s.instance(&key) else { return Vec::new() };
                let name = i.channel(&server, &channel).map(|c| c.name.clone()).unwrap_or_default();
                let shares = i.dms.secure_history.get(&channel).copied().unwrap_or(false);
                let start = Row::Start {
                    icon: "shield-check",
                    title: format!("Welcome to #{name}"),
                    body: crate::ui::secure::beginning(shares),
                    shared: None,
                };
                let who = |id: &str| Who {
                    name: i.display_name(Some(&server), id),
                    color: i.name_color(&server, id).map(|c| rgb(c).into()),
                    user: i.users.get(id).cloned(),
                };
                let manage = i.access(&server).has_in(&channel, pb::Permission::ManageMessages);
                encrypted_rows(i, &channel, start, &who, Some(manage), self.editing.as_deref())
            }),
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
        type Author = Rc<(String, Option<Hsla>, Option<&'static str>, Option<pb::User>)>;
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
                    ))
                })
                .clone()
        };
        let manage = i.access(&server).has_in(&channel, pb::Permission::ManageMessages);
        let can_vote = !guest_side && !i.access(&server).pending;
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
                    0 => "No replies yet. Start the thread!".to_owned(),
                    1 => "1 reply".to_owned(),
                    n => format!("{n} replies"),
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
            rows.push(Row::Start {
                icon: "hash",
                title: format!("Welcome to #{name}"),
                body: format!("This is the start of #{name}."),
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
        for m in parent.into_iter().chain(&loaded.items) {
            let is_parent = parent.is_some_and(|p| std::ptr::eq(p, m));
            if m.kind == pb::MessageKind::MemberJoined as i32 {
                rows.push(Row::Note {
                    id: m.id.clone(),
                    icon: "sparkles",
                    text: format!("{} joined the server. Say hi!", author(&m.author_id).0),
                });
                continue;
            }
            if m.kind == pb::MessageKind::AutoModAlert as i32 {
                if let Some(alert) = &m.auto_mod {
                    rows.push(Row::Msg(Rc::new(auto_mod_row(i, &server, m, alert, manage))));
                }
                continue;
            }
            let hook = m.webhook.as_ref();
            let who: Author = match hook {
                Some(w) => Rc::new((w.name.clone(), None, Some("APP"), Some(webhook_author(w)))),
                None => author(&m.author_id),
            };
            let (author_name, color, badge, user) = &*who;
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
            let mut h = DefaultHasher::new();
            bits.digest(&mut h);
            if let Some(card) = &card {
                card.digest(&mut h);
            }
            (&m.content, m.edited_at.as_ref().map(|t| (t.seconds, t.nanos)), m.embeds.len()).hash(&mut h);
            m.emojis.iter().map(|e| (&e.id, &e.url)).for_each(|e| e.hash(&mut h));
            m.attachments.iter().map(|a| (&a.url, &a.filename)).for_each(|a| a.hash(&mut h));
            (author_name, color.map(|c| [c.h, c.s, c.l, c.a].map(f32::to_bits)), badge).hash(&mut h);
            user.as_ref().map(|u| (&u.avatar_url, &u.username)).hash(&mut h);
            (look.digest, editing, manage, suppress, &me, &mine).hash(&mut h);
            (m.mentions_everyone, &m.mention_role_ids).hash(&mut h);
            (from.as_ref().map(|f| (&f.id, &f.name, &f.icon_url)), keep_out, keeping_out, can_delete).hash(&mut h);
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
                    shown: mention_links(&timestamp_nodes(&images_as_links(&m.content)), &look.with(&m.emojis)),
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
                    voice: None,
                    attachments: m.attachments.clone(),
                    thread: bits.clone(),
                    sig,
                }),
            };
            kept.insert(key, msg.clone());
            rows.push(Row::Msg(msg));
            if is_parent {
                divider(&mut rows);
            }
        }
        for p in i.pending.get(&at).into_iter().flatten() {
            rows.push(Row::Msg(Rc::new(Msg {
                id: format!("p{}", p.nonce),
                user: i.me.clone(),
                name: i.display_name(Some(&server), &me),
                color: i.name_color(&server, &me).map(|c| rgb(c).into()),
                content: p.content.clone(),
                shown: mention_links(&timestamp_nodes(&images_as_links(&p.content)), &look.with(&p.emojis)),
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
                voice: None,
                attachments: p.attachments.clone(),
                thread: ThreadBits::default(),
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
                    if matches!(r, Row::Msg(_) | Row::Note { .. }) && !self.fresh.contains_key(&r.id()) {
                        self.fresh.insert(r.id(), now);
                    }
                }
                // Only what's actually new rises in.
                let old: std::collections::HashSet<&str> =
                    rows[..list.len.min(rows.len())].iter().map(Row::id_str).collect();
                self.fresh.retain(|id, at| !old.contains(id.as_str()) || at.elapsed() < Duration::from_millis(900));
                let changed = changed_rows(&list.rows, &digests, 0);
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
            Nav::Instance { key } => self.instance_page(&key, window, cx),
        };
        div().flex_1().h_full().min_w_0().flex().bg(p.chat_surface).child(body)
    }

    // ───────────────────────── A channel ─────────────────────────

    fn channel_view(&mut self, key: &str, server: &str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        if let Some(locked) = self.sso_locked(key, server) {
            return self.sso_gate(key, &locked, &p, window, cx);
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
                .gap(px(10.0))
                .px(px(20.0))
                .border_b_1()
                .border_color(p.border)
                .child(icon("hash").size(px(20.0)).text_color(p.muted_foreground))
                .child(div().font_weight(FontWeight::EXTRA_BOLD).child(channel.name.clone()))
                .when_some(shared::pill_text(&channel), |el, text| {
                    el.child(crate::ui::shared_marks::pill(text, &channel.id, &p))
                })
                .when(!channel.topic.is_empty(), |el| {
                    el.child(div().w(px(1.0)).h(px(20.0)).bg(p.border)).child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .text_color(p.muted_foreground)
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(channel.topic.clone()),
                    )
                })
                .when(channel.topic.is_empty(), |el| el.child(div().flex_1()))
        }
        .when(!secure, |el| el.child(self.search_field(window, cx)))
        .child(self.header_buttons(key, server, &channel, &p, cx));

        let blocked = self.channel_blocked(key, server, &channel.id);
        let blocked = if secure { self.secure_blocked(key, server, &channel.id, blocked) } else { blocked };
        let column = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(header)
            .child(self.message_list(window, cx))
            .child(self.composer_bar(blocked, window, cx));

        let mut view = div().size_full().relative().flex().child(column);
        if let Some(panel) = self.search_panel(window, cx) {
            view = view.child(panel);
        } else if let Some(panel) = self.thread_panel(window, cx) {
            view = view.child(panel);
        } else if let Some(panel) = self.threads_list_panel(window, cx) {
            view = view.child(panel);
        } else if self.members_open {
            view = view.child(self.members_panel(key, server, window, cx));
        }
        if let Some(Menu::Channel { key, server, channel }) = self.menu.clone() {
            view = view.child(self.bell_menu(&key, &server, &channel, window, cx));
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
        cx: &mut Context<Self>,
    ) -> Div {
        let p = *p;
        let channel_id = channel.id.as_str();
        // Threads aren't in shared channels yet, nor in secure ones (theirs are their own).
        let threads = channel.shared.is_none() && channel.r#type != pb::ChannelType::Secure as i32;
        let side = self.threads.open.is_some() || self.threads.listing.is_some();
        div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .child({
                let muted = self.core.shared.read(|s| {
                    s.instance(key).is_some_and(|i| i.is_muted(server, channel_id, crate::core::dms::now_ms()))
                });
                let menu =
                    Menu::Channel { key: key.to_owned(), server: server.to_owned(), channel: channel_id.to_owned() };
                let open = self.menu.as_ref() == Some(&menu);
                icon_button("bell", if muted { "bell-off" } else { "bell" }, &p)
                    .when(open || muted, |el| el.text_color(p.primary))
                    .when(open, |el| el.bg(alpha(p.primary, 0.12)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu = if this.menu.as_ref() == Some(&menu) { None } else { Some(menu.clone()) };
                        cx.notify();
                    }))
            })
            .when(threads, |el| {
                let open = self.threads.listing.is_some();
                el.child(
                    icon_button("threads-toggle", "messages-square", &p)
                        .when(open, |el| el.text_color(p.primary).bg(alpha(p.primary, 0.12)))
                        .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Threads").build(window, cx))
                        .on_click(cx.listener(|this, _, window, cx| this.toggle_threads_list(window, cx))),
                )
            })
            .child({
                let shown = self.members_open && !side && self.search.panel.is_none();
                icon_button("members-toggle", "users", &p)
                    .when(shown, |el| el.text_color(p.primary).bg(alpha(p.primary, 0.12)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.close_thread(cx);
                        this.threads.listing = None;
                        if this.search.panel.is_some() {
                            this.close_search(window, cx);
                        }
                        this.members_open = !shown;
                        cx.notify();
                    }))
            })
    }

    fn message_list(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let rows = self.rows.clone();
        let ctx = self.row_ctx(None, cx);
        let target = self.list.target.clone().unwrap_or_default();
        let loading = rows.is_empty();
        div().flex_1().min_h_0().relative().child(if loading {
            crate::ui::sidebar::loading_rows(&p).into_any_element()
        } else {
            MessageScroller::new(
                SharedString::from(format!("list|{target}")),
                self.scroller.clone(),
                move |ix, _window, cx| match rows.get(ix) {
                    Some(row) => render_row(row, ix, &ctx, cx),
                    None => div().into_any_element(),
                },
            )
            .with_bottom_fade(p.background)
            .with_row_style(gpui_kit::StyleRefinement::default().pb(px(0.0)).px(px(0.0)))
            .with_list_style(gpui_kit::StyleRefinement::default().pb(px(12.0)))
            .with_jump_button_label("Jump to the latest")
            .size_full()
            .into_any_element()
        })
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
        let focused = self.composer.read(cx).focus_handle(cx).is_focused(window);
        let typed = !self.composer.read(cx).value().trim().is_empty();
        let ring = motion::follow("composer-ring", if focused { 1.0 } else { 0.0 }, window, cx);
        let ready = motion::follow("composer-send", if typed { 1.0 } else { 0.0 }, window, cx);
        if let Some(blocked) = blocked {
            return div()
                .flex_none()
                .px(px(20.0))
                .pb(px(20.0))
                .child(motion::rise(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .px(px(16.0))
                        .py(px(10.0))
                        .min_h(px(52.0))
                        .rounded(corner(16.0))
                        .bg(alpha(p.muted_foreground, 0.1))
                        .text_sm()
                        .text_color(p.muted_foreground)
                        .child(icon("lock").size(px(16.0)))
                        .child(div().flex_1().child(blocked.text))
                        .when_some(blocked.action, |el, (label, dialog)| {
                            el.child(primary_button("blocked-action", label, &p).h(px(34.0)).text_sm().on_click(
                                cx.listener(move |this, _, window, cx| this.open_dialog(dialog.clone(), window, cx)),
                            ))
                        }),
                    "composer-blocked",
                    Duration::ZERO,
                    8.0,
                ))
                .into_any_element();
        }
        let emoji_panel = self.emoji_open.then(|| self.emoji_panel(&p, cx));
        let time_panel = self.time_picker_panel(&p, cx);
        let tray = self.file_tray(&p, cx);
        let recording = self.recording_here();
        // The microphone takes the send button's place while nothing's typed, as on the web.
        let voice = (recording || (!typed && self.can_record())).then(|| self.voice_button(&p, cx));
        let field: AnyElement = if recording {
            self.recording_bar(&p, cx)
        } else {
            div()
                .flex_1()
                .min_w_0()
                .py(px(4.0))
                .child(Textarea::new(&self.composer).appearance(false).context_menu(composer_menu))
                .into_any_element()
        };
        div()
            .flex_none()
            .relative()
            .px(px(20.0))
            .pb(px(20.0))
            .when_some(self.picker.clone(), |el, picker| el.child(self.picker_list(picker, &p, cx)))
            .children(emoji_panel)
            .children(time_panel)
            .children(tray)
            .child(
                div()
                    .id("composer-box")
                    .flex()
                    .items_end()
                    .gap(px(8.0))
                    .pl(px(16.0))
                    .pr(px(8.0))
                    .py(px(8.0))
                    .rounded(corner(18.0))
                    .bg(p.card)
                    .border_1()
                    .border_color(mix(p.border, p.primary, ring))
                    .shadow(vec![gpui_kit::BoxShadow {
                        color: alpha(p.primary, 0.22 * ring),
                        offset: gpui_kit::point(px(0.0), px(8.0)),
                        blur_radius: px(24.0),
                        spread_radius: px(-8.0),
                        inset: false,
                    }])
                    .map(|el| self.droppable(el, &p, cx))
                    .child(field)
                    .when(!recording && self.can_attach(), |el| el.child(self.attach_button(&p, cx)))
                    .when(!recording, |el| el.child(self.timestamp_button(&p, cx)).child(self.emoji_button(&p, cx)))
                    .when(self.can_poll(), |el| {
                        el.child(
                            icon_button("poll-open", "chart-column", &p)
                                .size(px(36.0))
                                .tooltip(|window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new("Make a poll").build(window, cx)
                                })
                                .on_click(cx.listener(|this, _, window, cx| this.open_poll_editor(window, cx))),
                        )
                    })
                    .when_some(voice, |el, voice| el.child(voice))
                    .when(!recording && (typed || !self.can_record()), |el| {
                        el.child(
                            div()
                                .id("send")
                                .size(px(36.0))
                                .flex_none()
                                .rounded(corner(12.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(mix(p.muted, p.primary, ready))
                                .text_color(mix(p.muted_foreground, p.primary_foreground, ready))
                                .cursor_pointer()
                                .active(|s| s.top(px(1.0)))
                                .on_click(cx.listener(|this, _, window, cx| this.send_from_button(window, cx)))
                                .child(
                                    div().relative().left(px(-3.0 + 3.0 * ready)).child(icon("send").size(px(18.0))),
                                ),
                        )
                    }),
            )
            .into_any_element()
    }

    /// The @ list, floating over the composer: people, roles, @everyone.
    fn picker_list(&self, picker: crate::ui::app::Picker, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let hl = alpha(p.primary, 0.14);
        let hover = alpha(p.primary, 0.08);
        let mut list = div().flex().flex_col().p(px(6.0)).child(
            div()
                .px(px(10.0))
                .pt(px(6.0))
                .pb(px(4.0))
                .text_size(px(11.0))
                .font_weight(FontWeight::EXTRA_BOLD)
                .text_color(p.muted_foreground)
                .child(if matches!(picker.options.first(), Some(Pick::Emoji(_))) { "EMOJI" } else { "MENTION" }),
        );
        for (n, pick) in picker.options.iter().enumerate() {
            let active = n == picker.active;
            let (lead, name, sub): (AnyElement, String, String) = match pick {
                Pick::Member { user, name } => {
                    (avatar(Some(user), 24.0, p).into_any_element(), name.clone(), format!("@{}", user.username))
                }
                Pick::Role { name, color, .. } => (
                    div()
                        .size(px(24.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .size(px(12.0))
                                .rounded_full()
                                .bg(color.map(|c| Hsla::from(rgb(c))).unwrap_or(p.muted_foreground.into())),
                        )
                        .into_any_element(),
                    format!("@{name}"),
                    "Role".into(),
                ),
                Pick::Everyone(which) => (
                    div()
                        .size(px(24.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(p.primary)
                        .child(icon("at-sign").size(px(16.0)))
                        .into_any_element(),
                    format!("@{which}"),
                    if *which == "everyone" { "Everyone in the channel".into() } else { "Everyone online".into() },
                ),
                Pick::Emoji(choice) => (
                    emoji_glyph(choice, 22.0),
                    format!(":{}:", choice.name),
                    match (&choice.from, &choice.url) {
                        (Some(server), _) => server.clone(),
                        (None, Some(_)) => "This server".into(),
                        (None, None) => String::new(),
                    },
                ),
            };
            let pick = pick.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("pick|{}", pick.id())))
                    .h(px(38.0))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .rounded(corner(10.0))
                    .cursor_pointer()
                    .when(active, |el| el.bg(hl))
                    .when(!active, |el| el.hover(move |s| s.bg(hover)))
                    .on_click(cx.listener(move |this, _, window, cx| this.pick_mention(pick.clone(), window, cx)))
                    .child(lead)
                    .child(div().font_weight(FontWeight::BOLD).text_sm().child(name))
                    .child(div().flex_1())
                    .child(div().text_xs().text_color(p.muted_foreground).child(sub)),
            );
        }
        div().absolute().left(px(20.0)).right(px(20.0)).bottom(gpui_kit::relative(1.0)).child(motion::rise(
            card(p).mb(px(-12.0)).child(list),
            SharedString::from(format!("picker-{}", picker.start)),
            Duration::ZERO,
            10.0,
        ))
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
        gpui_kit::AnyView::from(view).cached(gpui_kit::StyleRefinement::default().w(px(232.0)).h_full().flex_none())
    }

    // ───────────────────────── A private conversation ─────────────────────────

    fn dm_view(&mut self, key: &str, id: &str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let (other, safety, verified, blocked, joining, status) = self.core.shared.read(|s| {
            let Some(i) = s.instance(key) else { return (None, None, None, None, false, None) };
            let me = i.me.as_ref().map(|m| m.id.clone()).unwrap_or_default();
            let other = i
                .dms
                .conversations
                .iter()
                .find(|c| c.id == id)
                .and_then(|c| c.users.iter().find(|u| u.id != me).cloned());
            (
                other,
                i.dms.safety.get(id).cloned(),
                i.dms.verified.get(id).cloned(),
                i.dms.blocked.get(id).cloned(),
                i.dms.joining.contains(id),
                Some(i.dms.status),
            )
        });
        let name = other.as_ref().map(user_name).unwrap_or_else(|| "Someone".into());
        let is_verified = verified.is_some() && verified == safety;
        let changed = verified.is_some() && verified != safety;
        let header = div()
            .h(px(56.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.0))
            .px(px(20.0))
            .border_b_1()
            .border_color(p.border)
            .child(avatar(other.as_ref(), 28.0, &p))
            .child(div().font_weight(FontWeight::EXTRA_BOLD).child(name))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(10.0))
                    .py(px(4.0))
                    .rounded_full()
                    .bg(alpha(p.success, 0.14))
                    .text_color(p.success)
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .child(icon("lock").size(px(12.0)))
                    .child("End-to-end encrypted"),
            )
            .child(div().flex_1())
            .child(
                div()
                    .id("safety")
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(12.0))
                    .h(px(32.0))
                    .rounded(corner(10.0))
                    .cursor_pointer()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(if changed {
                        p.destructive
                    } else if is_verified {
                        p.success
                    } else {
                        p.muted_foreground
                    })
                    .hover({
                        let bg = alpha(p.primary, 0.1);
                        move |s| s.bg(bg)
                    })
                    .on_click(cx.listener({
                        let (key, id) = (key.to_owned(), id.to_owned());
                        move |this, _, window, cx| {
                            this.open_dialog(Dialog::Safety { key: key.clone(), conversation: id.clone() }, window, cx)
                        }
                    }))
                    .child(
                        icon(if is_verified {
                            "shield-check"
                        } else if changed {
                            "shield-alert"
                        } else {
                            "shield"
                        })
                        .size(px(16.0)),
                    )
                    .child(if is_verified {
                        "Verified"
                    } else if changed {
                        "Safety number changed"
                    } else {
                        "Verify"
                    }),
            );
        let blocked = if !status.is_some_and(|s| s.is_ready()) {
            Some("Encrypted messages are still getting ready on this device…".to_owned())
        } else if joining {
            Some("Joining the conversation on this device…".to_owned())
        } else {
            blocked
        }
        .map(|text| Blocked { text, action: None });
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(self.message_list(window, cx))
            .child(self.composer_bar(blocked, window, cx))
            .into_any_element()
    }

    // ───────────────────────── An instance ─────────────────────────

    fn instance_page(&mut self, key: &str, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let Some((name, url, me, connection, problem, can_create)) = self.core.shared.read(|s| {
            s.instance(key).map(|i| {
                let creation = i.node.as_ref().map(|n| n.server_creation).unwrap_or_default();
                let can_create = creation == pb::ServerCreation::Everyone as i32
                    || creation == pb::ServerCreation::Unspecified as i32
                    || (creation == pb::ServerCreation::Admins as i32 && i.admin);
                (i.name(), i.url.clone(), i.me.clone(), i.connection, i.problem.clone(), can_create)
            })
        }) else {
            return div().into_any_element();
        };
        let streamer = self.prefs.streamer_mode;
        let state = match connection {
            Connection::Live => "Connected",
            Connection::Connecting => "Connecting…",
            Connection::Reconnecting => "Reconnecting…",
            Connection::Offline => "Offline",
            Connection::SignedOut => "Signed out",
        };
        let signed_out = connection == Connection::SignedOut;
        let k = key.to_owned();
        let body = card(&p)
            .w(px(520.0))
            .p(px(28.0))
            .flex()
            .flex_col()
            .gap(px(18.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(16.0))
                    .child(
                        div()
                            .size(px(64.0))
                            .rounded(corner(20.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(mix(p.card, p.primary, 0.16))
                            .child(fuwa_mark(42.0, &p)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(div().text_xl().font_weight(FontWeight::EXTRA_BOLD).child(name))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .text_sm()
                                    .text_color(p.muted_foreground)
                                    .child(conn_dot(connection, &p).border_color(p.card))
                                    .child(state)
                                    .when(!streamer, |el| el.child("·").child(url.clone())),
                            ),
                    ),
            )
            .when_some(me.filter(|_| !signed_out), |el, me| {
                el.child(div().flex().items_center().gap(px(12.0)).child(avatar(Some(&me), 40.0, &p)).child(
                    div().flex().flex_col().child(div().font_weight(FontWeight::BOLD).child(user_name(&me))).child(
                        div().text_sm().text_color(p.muted_foreground).child(if streamer {
                            "Signed in".to_owned()
                        } else {
                            format!("Signed in as @{}", me.username)
                        }),
                    ),
                ))
            })
            .when_some(error_line(problem.as_deref().filter(|_| connection != Connection::Live), &p), |el, e| {
                el.child(e)
            })
            .child(div().flex().gap(px(10.0)).flex_wrap().map(|el| {
                if signed_out {
                    el.child(primary_button("again", "Sign in again", &p).on_click(cx.listener({
                        let k = k.clone();
                        move |this, _, window, cx| this.reconnect(&k, window, cx)
                    })))
                } else {
                    el.when(can_create, |el| {
                        el.child(primary_button("create", "Make a server", &p).on_click(cx.listener({
                            let k = k.clone();
                            move |this, _, window, cx| {
                                this.open_dialog(Dialog::CreateServer { key: k.clone() }, window, cx)
                            }
                        })))
                    })
                    .child(soft_button("join", "Join with an invite", &p).on_click(cx.listener(
                        {
                            let k = k.clone();
                            move |this, _, window, cx| {
                                this.open_dialog(Dialog::JoinInvite { key: k.clone() }, window, cx)
                            }
                        },
                    )))
                }
            }));
        div()
            .id("instance-page")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .p(px(32.0))
            .child(motion::rise(body, SharedString::from(format!("instance|{key}")), Duration::ZERO, 18.0))
            .into_any_element()
    }

    /// Polls go in a server's plain channels, never shared ones, for those who may make them,
    /// on an instance that has them.
    fn can_poll(&self) -> bool {
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
fn group(rows: &mut [Row]) {
    let mut last: Option<(String, i64)> = None;
    for row in rows.iter_mut() {
        match row {
            Row::Msg(m) => {
                let author = m.user.as_ref().map(|u| u.id.clone()).unwrap_or_else(|| m.name.clone());
                let head = !matches!(&last, Some((a, at)) if *a == author && m.at - at < GROUP_MS && m.at >= *at);
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
    fresh: std::collections::HashMap<String, Instant>,
    this: WeakEntity<FuwaApp>,
    compact: bool,
    edit_box: gpui_kit::Entity<gpui_kit::component::input::TextareaState>,
    key: String,
    server: Option<String>,
    /// The instance's address, which pictures of other servers must come from.
    url: String,
    /// The message a search result opened, which glows a moment.
    jumped: Option<(String, Instant)>,
    /// Drawing the open thread's panel: the id of the message it's under.
    thread: Option<String>,
    /// What an open right-click menu is for, lit while it's open.
    lit: Option<String>,
}

pub(crate) fn render_row(row: &Row, ix: usize, ctx: &Rc<RowCtx>, cx: &mut App) -> AnyElement {
    let p = pal(cx);
    let is_fresh = ctx.fresh.contains_key(&row.id());
    let el: AnyElement =
        match row {
            Row::Older { loading } => {
                let (this, in_thread) = (ctx.this.clone(), ctx.thread.is_some());
                div()
                    .flex()
                    .justify_center()
                    .py(px(16.0))
                    .child(
                        soft_button("older", if *loading { "Loading…" } else { "Show older messages" }, &p).on_click(
                            move |_, _, cx| {
                                let _ = this.update(cx, |this, cx| {
                                    if in_thread { this.load_older_replies(cx) } else { this.load_older(cx) }
                                });
                            },
                        ),
                    )
                    .into_any_element()
            }
            // A secure channel's start says how it's kept private, in its own colour.
            Row::Start { icon: glyph, title, body, .. } if *glyph == "shield-check" => {
                crate::ui::secure::start(title.clone(), body.clone(), &p).into_any_element()
            }
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
            Row::Divider { text } => div()
                .px(px(20.0))
                .py(px(8.0))
                .flex()
                .items_center()
                .gap(px(10.0))
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(p.muted_foreground)
                .child(div().flex_1().h(px(1.0)).bg(p.border))
                .child(text.clone())
                .child(div().flex_1().h(px(1.0)).bg(p.border))
                .into_any_element(),
            Row::Msg(m) => message(m, &p, ctx, cx),
        };
    let el = if is_fresh {
        motion::rise(div().child(el), SharedString::from(format!("rise|{}|{ix}", row.id())), Duration::ZERO, 14.0)
            .into_any_element()
    } else {
        el
    };
    match &ctx.jumped {
        // Where a search result led: a glow that fades by itself.
        Some((id, at)) if id == row.id_str() => {
            let glow = p.primary;
            gpui_kit::AnimationExt::with_animation(
                div().child(el),
                SharedString::from(format!("jumped|{id}|{at:?}")),
                // Stays lit a moment, then fades.
                gpui_kit::Animation::new(JUMP_GLOW).with_easing(|t: f32| t * t * t),
                move |el, t| el.bg(alpha(glow, 0.22 * (1.0 - t))),
            )
            .into_any_element()
        }
        _ => el,
    }
}

/// How long a message a search result opened glows.
const JUMP_GLOW: Duration = Duration::from_millis(2400);

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
    let hover = alpha(p.foreground, if p.dark { 0.035 } else { 0.03 });
    let gutter = if compact { 0.0 } else { 56.0 };
    let content: AnyElement = if m.editing {
        edit_box(m, p, ctx).into_any_element()
    } else if let Some(card) = &m.voice {
        crate::ui::voice_notes::voice_card(&m.id, card, p, &ctx.this)
    } else if m.poll.is_some() && m.content.trim().is_empty() {
        div().into_any_element()
    } else if m.unreadable {
        div().italic().text_color(p.muted_foreground).child(m.content.clone()).into_any_element()
    } else {
        let (this, key, server) = (ctx.this.clone(), ctx.key.clone(), ctx.server.clone());
        TextView::markdown(SharedString::from(format!("md|{}", m.id)), m.shown.clone())
            .markdown_extensions(crate::ui::emoji::markdown_extensions())
            .selectable(true)
            .style(TextViewStyle { paragraph_gap: gpui_kit::rems(0.35), ..TextViewStyle::default() })
            .on_link_click(move |url, _, window, cx| match url.strip_prefix(SCHEME) {
                Some(mention) => {
                    let Some(username) = mention.strip_prefix("user/") else { return };
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
            .w_full()
            .into_any_element()
    };
    // Apps and bots have no profile to open; agents do.
    let author = m.user.as_ref().filter(|_| matches!(m.badge, None | Some("AGENT"))).map(|u| u.id.clone());
    let name = div()
        .id(SharedString::from(format!("name|{}", m.id)))
        .font_weight(FontWeight::EXTRA_BOLD)
        .text_color(m.color.unwrap_or(p.foreground.into()))
        .cursor_pointer()
        .hover(|s| s.underline())
        .when_some(author.clone(), |el, id| {
            let ctx = ctx.clone();
            el.on_mouse_down(MouseButton::Right, person_menu(&ctx, id.clone()))
                .on_click(move |_, window, cx| open_profile(&ctx, id.clone(), window, cx))
        })
        .child(m.name.clone());
    let time = div().text_xs().text_color(p.muted_foreground).child(when(m.at));
    let mut body = div().flex_1().min_w_0().flex().flex_col();
    if m.thread.also_in.is_some() || m.thread.also_sent {
        body = body.child(crate::ui::threads::also_note(&m.id, m.thread.also_in.as_deref(), p, &ctx.this));
    }
    if m.head {
        body = body.child(
            div()
                .flex()
                .items_baseline()
                .gap(px(8.0))
                .child(name)
                .when_some(m.from.as_ref(), |el, from| {
                    el.child(div().self_center().child(crate::ui::shared_marks::server_tag(from, &ctx.url, p)))
                })
                .when_some(m.badge, |el, badge| {
                    el.child(app_badge(SharedString::from(format!("badge|{}", m.id)), badge, p))
                })
                .child(time),
        );
    }
    body = body.child(
        div()
            .flex()
            .items_baseline()
            .gap(px(6.0))
            .child(div().flex_1().min_w_0().when(m.pending && m.failed.is_none(), |el| el.opacity(0.55)).child(content))
            .when(m.edited && !m.editing, |el| {
                el.child(div().text_xs().text_color(p.muted_foreground).child("(edited)"))
            }),
    );
    if !m.embeds.is_empty() {
        body = body.child(crate::ui::embeds::embeds(&m.id, &m.embeds, p));
    }
    if let Some(card) = &m.poll {
        body = body.child(crate::ui::polls::poll_card(&m.id, card, p, &ctx.this));
    }
    if !m.attachments.is_empty() {
        body = body.child(crate::ui::attachments::attachments_view(
            &m.id,
            &m.attachments,
            &ctx.url,
            p,
            &ctx.this,
            &ctx.key,
        ));
    }
    if let Some(replies) = &m.thread.replies {
        body = body.child(crate::ui::threads::replies_row(&m.id, replies, p, &ctx.this));
    }
    if let Some(reason) = &m.failed {
        let (retry, dismiss) = (ctx.this.clone(), ctx.this.clone());
        let nonce = m.nonce;
        let (thread, thread_d) = (ctx.thread.clone(), ctx.thread.clone());
        body = body.child(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .text_xs()
                .text_color(p.destructive)
                .child(format!("Couldn't send: {reason}"))
                .child(
                    div()
                        .id(SharedString::from(format!("retry|{nonce}")))
                        .font_weight(FontWeight::BOLD)
                        .cursor_pointer()
                        .hover(|s| s.underline())
                        .on_click(move |_, _, cx| {
                            let _ = retry.update(cx, |this, cx| this.retry(nonce, thread.clone(), cx));
                        })
                        .child("Retry"),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("dismiss|{nonce}")))
                        .font_weight(FontWeight::BOLD)
                        .cursor_pointer()
                        .hover(|s| s.underline())
                        .on_click(move |_, _, cx| {
                            let _ = dismiss.update(cx, |this, _| {
                                if let Some(Target::Channel { key, channel, .. }) = this.target() {
                                    let at = thread_d.as_deref().map_or(channel, crate::core::threads::thread_key);
                                    this.core.dismiss_pending(&key, &at, nonce);
                                }
                            });
                        })
                        .child("Dismiss"),
                ),
        );
    }

    let left: AnyElement = if compact {
        div().into_any_element()
    } else if m.head {
        div()
            .id(SharedString::from(format!("face|{}", m.id)))
            .w(px(gutter))
            .flex_none()
            .pt(px(2.0))
            .cursor_pointer()
            .when_some(author, |el, id| {
                let ctx = ctx.clone();
                el.on_mouse_down(MouseButton::Right, person_menu(&ctx, id.clone()))
                    .on_click(move |_, window, cx| open_profile(&ctx, id.clone(), window, cx))
            })
            .child(avatar(m.user.as_ref(), 40.0, p))
            .into_any_element()
    } else {
        div()
            .w(px(gutter))
            .flex_none()
            .pt(px(3.0))
            .text_xs()
            .text_color(p.muted_foreground)
            .opacity(0.0)
            .group_hover("msg", |s| s.opacity(1.0))
            .child(clock(m.at))
            .into_any_element()
    };

    let can_edit = m.mine && !m.pending && !m.unreadable && !m.editing && m.poll.is_none() && m.voice.is_none();
    let can_delete = m.can_delete && !m.pending && !m.editing;
    let keep_out = m.keep_out && !m.editing;
    let can_thread = m.thread.can_thread && !m.pending && !m.editing && !m.keeping_out;
    let actions = (can_edit || can_delete || keep_out || can_thread).then(|| {
        let id = m.id.clone();
        div()
            .absolute()
            .right(px(16.0))
            .top(px(-10.0))
            .opacity(if m.keeping_out { 1.0 } else { 0.0 })
            .group_hover("msg", |s| s.opacity(1.0))
            .flex()
            .p(px(2.0))
            .gap(px(2.0))
            .rounded(corner(12.0))
            .bg(p.card)
            .border_1()
            .border_color(p.border)
            .shadow(vec![gpui_kit::BoxShadow {
                color: alpha(p.foreground, 0.08),
                offset: gpui_kit::point(px(0.0), px(4.0)),
                blur_radius: px(12.0),
                spread_radius: px(-4.0),
                inset: false,
            }])
            .when(can_thread, |el| {
                let (this, id) = (ctx.this.clone(), id.clone());
                let label = if m.thread.replies.is_some() { "Open thread" } else { "Reply in thread" };
                el.child(
                    icon_button(SharedString::from(format!("thread|{id}")), "message-square-reply", p)
                        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(label).build(window, cx))
                        .on_click(move |_, window, cx| {
                            let _ = this.update(cx, |this, cx| this.open_thread(id.clone(), window, cx));
                        }),
                )
            })
            .when(can_edit, |el| {
                let (this, id, in_thread) = (ctx.this.clone(), id.clone(), ctx.thread.is_some());
                el.child(icon_button(SharedString::from(format!("edit|{id}")), "pencil", p).on_click(
                    move |_, window, cx| {
                        let _ = this.update(cx, |this, cx| {
                            this.start_edit(id.clone(), window, cx);
                            this.edit_in_thread = in_thread;
                            this.sync_list(cx);
                            this.sync_thread(cx);
                        });
                    },
                ))
            })
            .when(can_delete && !m.keeping_out, |el| {
                let (this, id) = (ctx.this.clone(), id.clone());
                el.child(icon_button_in(SharedString::from(format!("del|{id}")), "trash", p, p.destructive).on_click(
                    move |_, _, cx| {
                        let _ = this.update(cx, |this, cx| this.delete(id.clone(), cx));
                    },
                ))
            })
            .when(keep_out && !m.keeping_out, |el| {
                let (this, id) = (ctx.this.clone(), id.clone());
                el.child(
                    icon_button_in(SharedString::from(format!("keep|{id}")), "user-x", p, p.destructive)
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("Keep out of this channel").build(window, cx)
                        })
                        .on_click(move |_, _, cx| {
                            let _ = this.update(cx, |this, cx| {
                                this.keeping_out = Some(id.clone());
                                this.sync_list(cx);
                                cx.notify();
                            });
                        }),
                )
            })
            .when(m.keeping_out, |el| {
                let (yes, no) = (ctx.this.clone(), ctx.this.clone());
                let (id, author) = (id.clone(), m.user.as_ref().map(|u| u.id.clone()).unwrap_or_default());
                let name = m.name.clone();
                el.child(motion::slide_in(
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
                                .child(format!("Keep {name} out?")),
                        )
                        .child(
                            icon_button_in(SharedString::from(format!("keep-yes|{id}")), "check", p, p.destructive)
                                .on_click(move |_, _, cx| {
                                    let _ = yes.update(cx, |this, cx| this.keep_out(author.clone(), name.clone(), cx));
                                }),
                        )
                        .child(icon_button(SharedString::from(format!("keep-no|{id}")), "x", p).on_click(
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
                ))
            })
    });

    let ping = alpha(p.primary, if p.dark { 0.12 } else { 0.09 });
    let ping_hover = alpha(p.primary, if p.dark { 0.16 } else { 0.13 });
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
        move |hovered: &bool, _: &mut Window, cx: &mut App| {
            let of =
                MenuOf::Message { msg: msg.clone(), thread: thread.clone(), picture: None, selection: String::new() };
            let _ = this.update(cx, |this, _| this.set_hover_target(of, *hovered));
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
        .mx(px(4.0))
        .rounded(corner(10.0))
        .when(m.head, |el| el.mt(px(if compact { 4.0 } else { 10.0 })))
        .py(px(if compact { 1.0 } else { 3.0 }))
        .map(|el| {
            if m.mentions_me {
                el.bg(if lit { ping_hover } else { ping }).hover(move |s| s.bg(ping_hover))
            } else if m.editing || lit {
                el.bg(hover)
            } else {
                el.hover(move |s| s.bg(hover))
            }
        })
        .when(m.mentions_me, |el| {
            el.child(div().absolute().left_0().top(px(4.0)).bottom(px(4.0)).w(px(3.0)).rounded_full().bg(bar))
        })
        .child(left)
        .when(compact && m.head, |el| el.gap(px(8.0)))
        .child(body)
        .when_some(actions, |el, a| el.child(a))
        .into_any_element()
}

/// The message box's right-click menu: the usual cut, copy and paste, then
/// an emoji or a timestamp at the caret.
fn composer_menu(
    menu: gpui_kit::component::native_menu::NativeMenu,
    _: &mut Window,
    _: &mut App,
) -> gpui_kit::component::native_menu::NativeMenu {
    use gpui_kit::component::input::{Copy, Cut, Paste, SelectAll};
    menu.menu("Cut", Box::new(Cut))
        .menu("Copy", Box::new(Copy))
        .menu("Paste", Box::new(Paste))
        .separator()
        .menu("Select all", Box::new(SelectAll))
        .separator()
        .menu("Emoji", Box::new(crate::ui::app::ComposerEmoji))
        .menu("Timestamp", Box::new(crate::ui::app::ComposerTimestamp))
}

/// Editing in place: Enter saves, Escape stops.
fn edit_box(m: &Msg, p: &Palette, ctx: &Rc<RowCtx>) -> impl IntoElement {
    let (save, cancel) = (ctx.this.clone(), ctx.this.clone());
    let link = |id: &str, label: &'static str, p: &Palette| {
        div()
            .id(SharedString::from(format!("{id}|{}", m.id)))
            .text_color(p.primary)
            .font_weight(FontWeight::BOLD)
            .cursor_pointer()
            .hover(|s| s.underline())
            .child(label)
    };
    motion::rise(
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .py(px(4.0))
            .child(
                div()
                    .px(px(12.0))
                    .py(px(8.0))
                    .rounded(corner(12.0))
                    .bg(p.card)
                    .border_1()
                    .border_color(p.primary)
                    .shadow(vec![gpui_kit::BoxShadow {
                        color: alpha(p.primary, 0.2),
                        offset: gpui_kit::point(px(0.0), px(6.0)),
                        blur_radius: px(18.0),
                        spread_radius: px(-8.0),
                        inset: false,
                    }])
                    .child(Textarea::new(&ctx.edit_box).appearance(false)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .text_xs()
                    .text_color(p.muted_foreground)
                    .child("Escape to")
                    .child(link("cancel", "cancel", p).on_click(move |_, window, cx| {
                        let _ = cancel.update(cx, |this, cx| this.cancel_edit(window, cx));
                    }))
                    .child("· Enter to")
                    .child(link("save", "save", p).on_click(move |_, window, cx| {
                        let _ = save.update(cx, |this, cx| this.save_edit(window, cx));
                    })),
            ),
        SharedString::from(format!("editing|{}", m.id)),
        Duration::ZERO,
        6.0,
    )
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
                                    .text_xl()
                                    .font_weight(FontWeight::EXTRA_BOLD)
                                    .child(format!("Sign in with {provider}")),
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

/// What AutoMod caught, posted in its alert channel for moderators.
fn auto_mod_row(i: &InstanceState, server: &str, m: &pb::Message, alert: &pb::AutoModAlert, manage: bool) -> Msg {
    let why = match pb::AutoModTrigger::try_from(alert.trigger) {
        Ok(pb::AutoModTrigger::Keywords) => "blocked words",
        Ok(pb::AutoModTrigger::MentionSpam) => "mention spam",
        Ok(pb::AutoModTrigger::Links) => "a link",
        _ => "breaking a rule",
    };
    let what = if alert.blocked { "Blocked" } else { "Flagged" };
    let place = i.channel(server, &alert.channel_id).map(|c| format!(" in **#{}**", c.name)).unwrap_or_default();
    let quote: String = alert.content.lines().map(|l| format!("> {l}\n")).collect();
    let mut text = if alert.capped_per_day > 0 {
        format!(
            "The Smart filter used up today's **{}** checks, so messages go through it unchecked until midnight UTC. \
             Your other rules still apply.\n\nRule: {}",
            alert.capped_per_day, alert.rule_name
        )
    } else {
        format!("{what} a message from **{}**{place} for {why}.\n\n{quote}", i.display_name(Some(server), &m.author_id))
    };
    if !alert.matched.is_empty() {
        text.push_str(&format!(
            "\nMatched: {}",
            alert.matched.iter().map(|w| format!("`{w}`")).collect::<Vec<_>>().join(", ")
        ));
    }
    if alert.timed_out_seconds > 0 {
        text.push_str(&format!("\n\nTimed out for {}.", span(i64::from(alert.timed_out_seconds))));
    }
    Msg {
        id: m.id.clone(),
        user: None,
        name: "AutoMod".into(),
        color: Some(rgb(0xf59e0b).into()),
        content: text.clone(),
        shown: text,
        mentions_me: false,
        editing: false,
        can_delete: manage,
        at: ms_of(m.created_at.as_ref()),
        edited: false,
        head: true,
        mine: false,
        pending: false,
        failed: None,
        nonce: 0,
        unreadable: false,
        badge: Some("BOT"),
        embeds: Vec::new(),
        from: None,
        keep_out: false,
        keeping_out: false,
        poll: None,
        voice: None,
        attachments: Vec::new(),
        thread: ThreadBits::default(),
        sig: 0,
    }
}

fn span(seconds: i64) -> String {
    let (n, unit) = match seconds {
        s if s >= 86_400 => (s / 86_400, "day"),
        s if s >= 3_600 => (s / 3_600, "hour"),
        s if s >= 60 => (s / 60, "minute"),
        s => (s, "second"),
    };
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

/// An emoji to pick: a server's own picture, or the character.
pub(crate) fn emoji_glyph(choice: &crate::ui::emoji::Choice, size: f32) -> AnyElement {
    let base = div().size(px(size + 2.0)).flex_none().flex().items_center().justify_center();
    match &choice.url {
        Some(url) => base
            .child({
                use gpui_kit::StyledImage as _;
                gpui_kit::img(SharedString::from(url.clone())).size(px(size)).object_fit(gpui_kit::ObjectFit::Contain)
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
