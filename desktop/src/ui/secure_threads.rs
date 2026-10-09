//! Threads inside a secure channel (the web's `chat/SecureThreads.tsx`): the
//! same panel and list as an ordinary channel's (`ui/threads.rs`), fed from
//! what this device opened instead of the server's summaries, since the
//! server can't read the channel and can't tell which lines are replies
//! (`core/secure_threads.rs`). Nothing here asks it for anything but sending.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::input::Input;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, ScrollHandle,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px, rgb,
};

use crate::core::dms::{Content, DmPending, DmStatus};
use crate::core::i18n::{Arg, t, t_with};
use crate::core::secure_threads::{self as st, Organized};
use crate::core::store::InstanceState;
use crate::core::vault::{Item, ItemKind};
use crate::pb;
use crate::ui::app::{FuwaApp, Target};
use crate::ui::chat::{Msg, Row, Who};
use crate::ui::dm_view::{Composer, Earlier, Lines, earlier_from};
use crate::ui::motion;
use crate::ui::theme::{Palette, alpha, radius_lg, radius_xl};
use crate::ui::widgets::{avatar, icon, pal};

/// Which messages on their way a list shows.
type Pending = Box<dyn Fn(&DmPending) -> bool>;
/// A thread in the list: it, its message, that message's author and name, unread replies, and faces.
type Listed = (st::SecureThread, Option<Item>, Option<pb::User>, String, u32, Vec<pb::User>);

/// What's open beside a secure channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Side {
    /// One thread, under the message with this record.
    Thread { key: String, server: String, channel: String, parent: i64 },
    /// The channel's threads.
    List { key: String, server: String, channel: String, archived: bool },
}

impl Side {
    fn channel(&self) -> &str {
        match self {
            Side::Thread { channel, .. } | Side::List { channel, .. } => channel,
        }
    }
}

thread_local! {
    static SIDE: RefCell<Option<Side>> = const { RefCell::new(None) };
    /// "Also send to #channel" under the open thread's box.
    static ALSO: Cell<bool> = const { Cell::new(false) };
    static SCROLL: ScrollHandle = ScrollHandle::new();
    /// How many rows the open thread drew last, so it follows new replies down.
    static DRAWN: Cell<usize> = const { Cell::new(0) };
    /// The last reply noted as seen, so it's noted once.
    static MARKED: RefCell<(String, i64, i64)> = const { RefCell::new((String::new(), 0, 0)) };
    /// The time picker was opened from the thread's box.
    static TIME_IN_THREAD: Cell<bool> = const { Cell::new(false) };
}

/// Whether the time picker belongs to the open thread's box (not the channel's).
pub(crate) fn time_in_thread() -> bool {
    TIME_IN_THREAD.with(Cell::get)
}

pub(crate) fn set_time_in_thread(on: bool) {
    TIME_IN_THREAD.with(|t| t.set(on));
}

/// What's open beside the secure channel `channel`, if anything.
pub(crate) fn side_of(channel: &str) -> Option<Side> {
    SIDE.with(|s| s.borrow().clone().filter(|side| side.channel() == channel))
}

/// The thread open beside the shown secure channel: its channel and message.
fn open_thread(app: &FuwaApp) -> Option<(String, String, String, i64)> {
    let Some(Target::Secure { channel: shown, .. }) = app.target() else { return None };
    match side_of(&shown)? {
        Side::Thread { key, server, channel, parent } => Some((key, server, channel, parent)),
        Side::List { .. } => None,
    }
}

/// Where a thread's files and problems are kept, apart from its channel's.
pub(crate) fn place_of(channel: &str, parent: i64) -> String {
    format!("{channel}#{parent}")
}

/// Who has Manage Messages in a secure channel, by this device's view of the
/// server's roles and the channel's overwrites: whose lock on a thread counts.
pub(crate) fn moderates<'a>(i: &'a InstanceState, server: &'a str, channel: &'a str) -> impl Fn(&str) -> bool + 'a {
    let owner = i.server(server).map(|s| s.owner_id.clone()).unwrap_or_default();
    let roles = i.roles.get(server).map(Vec::as_slice).unwrap_or_default();
    let channels = i.channels.get(server).map(Vec::as_slice).unwrap_or_default();
    let members = i.members.get(server).map(Vec::as_slice).unwrap_or_default();
    move |user: &str| {
        let Some(member) = members.iter().find(|m| m.user.as_ref().is_some_and(|u| u.id == user)) else {
            return false;
        };
        crate::core::permissions::access_of(server, &owner, roles, channels, user, &member.role_ids, false)
            .has_in(channel, pb::Permission::ManageMessages)
    }
}

/// A secure channel's lines, sorted into the channel and its threads.
pub(crate) fn organized(i: &InstanceState, server: &str, channel: &str) -> Organized {
    let items = i.dms.items.get(channel).map(Vec::as_slice).unwrap_or_default();
    st::organize(items, &moderates(i, server, channel))
}

impl FuwaApp {
    /// A secure channel's rows (`thread` None), or one of its threads' (the
    /// rows after its message), from what this device opened.
    pub(crate) fn secure_list(
        &self,
        i: &InstanceState,
        key: &str,
        server: &str,
        channel: &str,
        thread: Option<i64>,
    ) -> Vec<Row> {
        let name = i.channel(server, channel).map(|c| c.name.clone()).unwrap_or_default();
        let shares = i.dms.secure_history.get(channel).copied().unwrap_or(false);
        let me = i.me.as_ref().map(|m| m.id.clone()).unwrap_or_default();
        let who = |id: &str| Who {
            name: i.display_name(Some(server), id),
            color: i.name_color(server, id).map(|c| rgb(c).into()),
            user: i.users.get(id).cloned(),
        };
        let items = i.dms.items.get(channel).map(Vec::as_slice).unwrap_or_default();
        let locked = crate::core::backup::state(key).status == crate::core::backup::Status::Locked;
        let earlier = earlier_from(items, locked);
        let name_of = |id: &str| i.display_name(Some(server), id);
        let describe = |item: &Item| {
            let earlier = if item.kind == ItemKind::Joined { earlier } else { Earlier::None };
            crate::ui::secure::channel_line(item, &name_of, &me, earlier)
        };
        let access = i.access(server);
        let manage = access.has_in(channel, pb::Permission::ManageMessages);
        let can_send = access.has_in(channel, pb::Permission::SendMessages);
        let can_start = access.has_in(channel, pb::Permission::CreateThreads);
        let timed_out = i
            .my_member(server)
            .and_then(|m| crate::core::moderation::timed_out_until(m, crate::core::dms::now_ms()))
            .is_some();
        let react = i
            .has("reactions")
            .then(|| access.has_in(channel, pb::Permission::AddReactions) && (!timed_out || access.owner));
        let question = t("chat.secure.deleteQuestion");
        let org = organized(i, server, channel);
        let note = i.dms.thread_notes.get(channel);
        let hours = i.server(server).map(|s| s.thread_archive_hours).unwrap_or(0);
        let now = crate::core::dms::now_ms();
        let none: Vec<Item> = Vec::new();
        let replies_of = |parent: i64| org.in_thread.get(&parent).unwrap_or(&none);
        let dress = |item: &Item, m: &mut Msg| {
            if let Some(parent) = thread {
                // In its thread: that it went to the channel too.
                m.thread.also_sent = item.in_channel && item.thread == parent;
                return;
            }
            if item.thread != 0 {
                // In the channel, a reply also sent there: a way into its thread.
                m.thread.also_in = item.in_channel.then(|| item.thread.to_string());
                return;
            }
            if let Some(t) = org.threads.get(&item.seq) {
                let unread = if st::following(note, t.parent, items, &me) {
                    st::unread_in(note, t.parent, replies_of(t.parent), &me)
                } else {
                    0
                };
                m.thread.replies = Some(Rc::new(crate::ui::threads::Replies {
                    count: t.replies as i32,
                    faces: t.participants.iter().take(3).filter_map(|id| who(id).user).collect(),
                    new: unread,
                    locked: t.locked,
                    archived: st::archived(Some(t), hours, now),
                    last_at: t.last_at,
                }));
            }
            // A thread starts with Start threads; replying in one that's there takes only Send messages.
            m.thread.can_thread =
                st::can_have_thread(Some(item)) && can_send && (can_start || org.threads.contains_key(&item.seq));
            // Its author can't delete it once someone else replied in its thread (moderators still can).
            let kept = replies_of(item.seq)
                .iter()
                .any(|r| r.kind == ItemKind::Text && !r.deleted && r.sender_id != item.sender_id);
            if kept && !manage {
                m.can_delete = false;
            }
        };
        let (lines, start, place, pending): (&[Item], Row, String, Pending) = match thread {
            None => (
                &org.channel,
                crate::ui::secure::beginning(&name, shares),
                channel.to_owned(),
                Box::new(|p: &DmPending| p.thread == 0 || p.in_channel),
            ),
            Some(parent) => (
                replies_of(parent),
                Row::Day { id: "thread-top".into(), text: String::new() },
                place_of(channel, parent),
                Box::new(move |p: &DmPending| p.thread == parent),
            ),
        };
        let lines = Lines { items: lines, pending: &*pending, place, dress: &dress, in_thread: thread.is_some() };
        let mut rows = crate::ui::dm_view::encrypted_rows(
            i,
            channel,
            start,
            &who,
            &describe,
            Some(manage),
            self.editing.as_deref(),
            None,
            &question,
            Some(lines),
            react,
        );
        if thread.is_some() {
            // The panel draws the thread's message itself.
            rows.remove(0);
        }
        rows
    }

    // ───────────────────────── Opening and closing ─────────────────────────

    /// Opens the thread under a secure channel's line (`id` is its record), starting it if it has none.
    pub(crate) fn open_secure_thread(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Target::Secure { key, server, channel }) = self.target() else { return };
        let Ok(parent) = id.parse::<i64>() else { return };
        let side = Side::Thread { key, server, channel, parent };
        if SIDE.with(|s| s.borrow().as_ref() == Some(&side)) {
            return;
        }
        SIDE.with(|s| *s.borrow_mut() = Some(side));
        ALSO.with(|a| a.set(false));
        DRAWN.with(|d| d.set(0));
        self.pins = None;
        self.threads.listing = None;
        self.threads.reply.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.focus(window, cx);
        });
        crate::core::reports::used("thread.open");
        cx.notify();
    }

    /// Opens the secure channel's list of threads, or closes what's open beside it.
    pub(crate) fn toggle_secure_threads(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Target::Secure { key, server, channel }) = self.target() else { return };
        let listing = matches!(side_of(&channel), Some(Side::List { .. }));
        SIDE.with(|s| {
            *s.borrow_mut() = (!listing).then_some(Side::List { key, server, channel, archived: false });
        });
        if !listing {
            self.threads.query.update(cx, |state, cx| state.set_value("", window, cx));
        }
        cx.notify();
    }

    fn close_secure_side(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        SIDE.with(|s| *s.borrow_mut() = None);
        self.composer.update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }

    // ───────────────────────── Replying ─────────────────────────

    /// Sends what's in the open secure thread's box (and any files picked there).
    pub(crate) fn send_secure_reply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((key, _, channel, parent)) = open_thread(self) else { return };
        if self.secure_thread_locked().is_some() {
            return;
        }
        let text = self.threads.reply.read(cx).value().trim().to_owned();
        let also = ALSO.with(Cell::get);
        let place = place_of(&channel, parent);
        if !crate::ui::sealed_files::picked(&place).is_empty() {
            if self.send_picked_in(key, channel, place, text, Some((parent, also)), cx) {
                self.threads.reply.update(cx, |state, cx| state.set_value("", window, cx));
                ALSO.with(|a| a.set(false));
            }
            return;
        }
        let ready = self.core.shared.read(|s| s.instance(&key).is_some_and(|i| i.dms.status == DmStatus::Ready));
        if text.is_empty() || text.chars().count() > crate::core::dms::MAX_DM || !ready {
            return;
        }
        self.threads.reply.update(cx, |state, cx| state.set_value("", window, cx));
        ALSO.with(|a| a.set(false));
        crate::ui::dm_view::set_problem(&place, None);
        let core = self.core.clone();
        let content = Content::Reply { text, thread: parent, in_channel: also, files: Vec::new() };
        self.run(cx, async move { core.send_dm(&key, &channel, content).await }, move |this, result, cx| {
            crate::ui::dm_view::set_problem(&place, result.err().map(|e| e.0));
            this.sync_list(cx);
            cx.notify();
        });
        self.sync_list(cx);
        cx.notify();
    }

    /// Why you can't reply in the open secure thread, if you can't.
    pub(crate) fn secure_thread_locked(&self) -> Option<String> {
        let (key, server, channel, parent) = open_thread(self)?;
        self.core.shared.read(|s| {
            let i = s.instance(&key)?;
            let access = i.access(&server);
            if !access.has_in(&channel, pb::Permission::SendMessages) {
                return Some(t("chat.secure.noPermission"));
            }
            let locked = organized(i, &server, &channel).threads.get(&parent).is_some_and(|t| t.locked);
            (locked && !access.has_in(&channel, pb::Permission::ManageMessages)).then(|| t("chat.secureThreads.locked"))
        })
    }

    /// Whether "Also send to #channel" is ticked under the open thread's box.
    pub(crate) fn secure_also(&self) -> bool {
        ALSO.with(Cell::get)
    }

    pub(crate) fn set_secure_also(&mut self, on: bool, cx: &mut Context<Self>) {
        ALSO.with(|a| a.set(on));
        cx.notify();
    }

    // ───────────────────────── Drawing ─────────────────────────

    /// The header's Threads button: the list's own, lit while anything's open beside the channel.
    pub(crate) fn secure_threads_button(&self, channel: &str, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let side = side_of(channel);
        crate::ui::widgets::header_button("secure-threads", "messages-square", side.is_some(), -8.0, p)
            .tooltip(|window, cx| crate::ui::overlay::Tip::new(t("chat.threads.threads")).build(window, cx))
            .on_click(cx.listener(|this, _, window, cx| this.toggle_secure_threads(window, cx)))
            .into_any_element()
    }

    /// What's open beside the shown secure channel: a thread or the list of them.
    pub(crate) fn secure_side(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Some(Target::Secure { channel, .. }) = self.target() else { return None };
        let side = side_of(&channel)?;
        let ready = self.core.shared.read(|s| {
            s.instance(match &side {
                Side::Thread { key, .. } | Side::List { key, .. } => key,
            })
            .is_some_and(|i| i.dms.status == DmStatus::Ready && i.me.is_some())
        });
        if !ready {
            return None;
        }
        let p = pal(cx);
        let panel = match side {
            Side::Thread { key, server, channel, parent } => {
                self.secure_thread_panel(&key, &server, &channel, parent, window, cx)
            }
            Side::List { key, server, channel, archived } => {
                self.secure_thread_list(&key, &server, &channel, archived, window, cx)
            }
        };
        Some(
            motion::slide_in(
                div()
                    .w(px(440.0))
                    .h_full()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .border_l_1()
                    .border_color(p.border)
                    .bg(p.side_surface)
                    .child(panel),
                "secure-side",
                32.0,
            )
            .into_any_element(),
        )
    }

    /// A secure channel's thread: its message, its replies and lock changes as this device
    /// opened them, and a box of its own with "Also send to #channel". Follow and lock (for
    /// moderators) sit in its header.
    fn secure_thread_panel(
        &mut self,
        key: &str,
        server: &str,
        channel: &str,
        parent: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = pal(cx);
        let read = self.core.shared.read(|s| {
            let i = s.instance(key)?;
            let me = i.me.as_ref()?.id.clone();
            let items = i.dms.items.get(channel).cloned().unwrap_or_default();
            let org = organized(i, server, channel);
            let note = i.dms.thread_notes.get(channel).cloned();
            let hours = i.server(server).map(|s| s.thread_archive_hours).unwrap_or(0);
            let access = i.access(server);
            let message = items.iter().find(|m| m.seq == parent && m.kind == ItemKind::Text && !m.deleted).cloned();
            let author = message.as_ref().map(|m| Who {
                name: i.display_name(Some(server), &m.sender_id),
                color: i.name_color(server, &m.sender_id).map(|c| rgb(c).into()),
                user: i.users.get(&m.sender_id).cloned(),
            });
            let thread = org.threads.get(&parent).cloned();
            let last = org.in_thread.get(&parent).and_then(|l| l.last()).map(|l| l.seq).unwrap_or(0);
            Some((
                i.channel(server, channel).map(|c| c.name.clone()).unwrap_or_default(),
                message,
                author,
                st::following(note.as_ref(), parent, &items, &me),
                thread.as_ref().is_some_and(|t| t.locked),
                st::archived(thread.as_ref(), hours, crate::core::dms::now_ms()),
                thread.map_or(0, |t| t.replies),
                last,
                access.has_in(channel, pb::Permission::ManageMessages)
                    && access.has_in(channel, pb::Permission::SendMessages),
                access.has_in(channel, pb::Permission::SendMessages)
                    && access.has_in(channel, pb::Permission::AttachFiles),
                self.secure_list(i, key, server, channel, Some(parent)),
            ))
        });
        let Some((name, message, author, followed, locked, archived, count, last, can_lock, can_attach, rows)) = read
        else {
            return div().into_any_element();
        };
        // What's open is read.
        let seen = MARKED.with(|m| *m.borrow() == (channel.to_owned(), parent, last));
        if last > 0 && !seen {
            MARKED.with(|m| *m.borrow_mut() = (channel.to_owned(), parent, last));
            let (core, k, c) = (self.core.clone(), key.to_owned(), channel.to_owned());
            self.run(cx, async move { core.mark_secure_thread_read(&k, &c, parent, last).await }, |_, _, _| {});
        }

        let panel_button = |id: &'static str, glyph: &'static str, label: String, on: bool| {
            let hover = p.muted;
            div()
                .id(id)
                .size(px(32.0))
                .flex_none()
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_color(if on { p.primary } else { p.muted_foreground })
                .when(on, |el| el.bg(alpha(p.primary, 0.1)))
                .hover(move |s| s.bg(hover))
                .tooltip(move |window, cx| crate::ui::overlay::Tip::new(label.clone()).build(window, cx))
                .child(icon(glyph).size(px(16.0)))
        };
        let where_key = match (archived, locked) {
            (true, true) => "chat.threads.whereArchivedLocked",
            (true, false) => "chat.threads.whereArchived",
            (false, true) => "chat.threads.whereLocked",
            (false, false) => "chat.threads.where",
        };
        let (k, c) = (key.to_owned(), channel.to_owned());
        let header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(12.0))
            .h(px(56.0))
            .border_b_1()
            .border_color(p.border)
            .child(icon("messages-square").size(px(20.0)).text_color(p.primary))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(16.0))
                            .line_height(px(20.0))
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .child(t("chat.threads.thread")),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .line_height(px(16.0))
                            .text_color(p.muted_foreground)
                            .child(t_with(where_key, &[("channel", Arg::Str(&name))])),
                    ),
            )
            .child({
                let label = if followed { t("chat.threads.unfollow") } else { t("chat.threads.follow") };
                let (k, c) = (k.clone(), c.clone());
                panel_button("secure-thread-follow", if followed { "bell" } else { "bell-off" }, label, followed)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let (core, k, c) = (this.core.clone(), k.clone(), c.clone());
                        this.run(
                            cx,
                            async move { core.follow_secure_thread(&k, &c, parent, !followed).await },
                            move |this, result, cx| match result {
                                Ok(()) => this.toast(
                                    if followed { "bell-off" } else { "bell-ring" },
                                    t(if followed { "chat.threads.unfollowed" } else { "chat.threads.followed" }),
                                    String::new(),
                                    None,
                                    None,
                                    cx,
                                ),
                                Err(err) => this.toast("circle-alert", err.0, String::new(), None, None, cx),
                            },
                        );
                    }))
            })
            .when(can_lock, |el| {
                let label = if locked { t("chat.threads.unlock") } else { t("chat.threads.lock") };
                let (k, c) = (k.clone(), c.clone());
                el.child(
                    panel_button("secure-thread-lock", if locked { "lock" } else { "lock-open" }, label, locked)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let (core, k, c) = (this.core.clone(), k.clone(), c.clone());
                            this.run(
                                cx,
                                async move { core.lock_secure_thread(&k, &c, parent, !locked).await },
                                |this, result, cx| {
                                    if let Err(err) = result {
                                        this.toast("circle-alert", err.0, String::new(), None, None, cx);
                                    }
                                    this.sync_list(cx);
                                },
                            );
                        })),
                )
            })
            .child(
                panel_button("secure-thread-close", "x", t("chat.threads.close"), false)
                    .on_click(cx.listener(|this, _, window, cx| this.close_secure_side(window, cx))),
            );

        // The thread's message (if this device has it), then how many replies follow.
        let top: AnyElement = match (&message, author) {
            (Some(m), Some(who)) => {
                let msg = crate::ui::dm_view::plain_msg(
                    format!("top|{}", m.seq),
                    who,
                    m.content.clone(),
                    m.at,
                    false,
                    crate::ui::dm_view::EncBits { files: m.files.clone(), ..Default::default() },
                );
                let mut msg = msg;
                msg.can_delete = false;
                msg.edited = m.edited_at > 0;
                // Its reactions, as the channel's list has them.
                let seq = m.seq.to_string();
                msg.reactions = self.rows.iter().find_map(|r| match r {
                    Row::Msg(c) if c.id == seq => c.reactions.clone(),
                    _ => None,
                });
                crate::ui::chat::render_row(&Row::Msg(Rc::new(msg)), 0, &self.row_ctx(Some("secure".into()), cx), cx)
            }
            _ => div()
                .mx(px(16.0))
                .px(px(12.0))
                .py(px(10.0))
                .rounded(crate::ui::theme::radius_2xl())
                .border_1()
                .border_dashed()
                .border_color(p.border)
                .text_sm()
                .line_height(px(20.0))
                .text_color(p.muted_foreground)
                .child(t("chat.secureThreads.parentMissing"))
                .into_any_element(),
        };
        let separator = div()
            .my(px(12.0))
            .flex()
            .items_center()
            .gap(px(12.0))
            .px(px(16.0))
            .text_xs()
            .line_height(px(16.0))
            .font_weight(FontWeight::BOLD)
            .text_color(p.muted_foreground)
            .child(if count == 0 {
                t("chat.threads.noReplies")
            } else {
                t_with("chat.threads.replies", &[("count", Arg::Num(i64::from(count)))])
            })
            .child(div().flex_1().h(px(1.0)).bg(p.border));

        // New replies keep the panel at the bottom.
        let len = rows.len();
        SCROLL.with(|scroll| {
            let drawn = DRAWN.with(Cell::get);
            if len != drawn {
                let max = scroll.max_offset();
                if drawn == 0 || f32::from(max.y) + f32::from(scroll.offset().y) < 80.0 {
                    scroll.scroll_to_bottom();
                }
                DRAWN.with(|d| d.set(len));
            }
        });
        let ctx = self.row_ctx(Some("secure".into()), cx);
        let scroll = SCROLL.with(Clone::clone);
        let body = div()
            .id(SharedString::from(format!("secure-thread-rows|{channel}|{parent}")))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .child(
                div()
                    .min_h_full()
                    .flex()
                    .flex_col()
                    .justify_end()
                    .pb(px(12.0))
                    .child(div().pt(px(12.0)).child(top).child(separator))
                    .children(
                        rows.iter().enumerate().map(|(ix, row)| crate::ui::chat::render_row(row, ix + 1, &ctx, cx)),
                    ),
            );
        let composer = self.encrypted_composer(
            Composer {
                key: key.to_owned(),
                id: channel.to_owned(),
                promise: t("chat.secure.promise"),
                locked: self.secure_thread_locked(),
                action: None,
                files: can_attach,
                thread: Some((parent, name)),
            },
            window,
            cx,
        );
        div().size_full().flex().flex_col().child(header).child(body).child(composer).into_any_element()
    }

    /// A secure channel's threads, the latest reply first, open or archived, searched on this device.
    fn secure_thread_list(
        &mut self,
        key: &str,
        server: &str,
        channel: &str,
        archived: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = pal(cx);
        let pill = motion::follow("secure-threads-pill", if archived { 1.0 } else { 0.0 }, window, cx);
        let query = self.threads.query.read(cx).value().to_string();
        let read = self.core.shared.read(|s| {
            let i = s.instance(key)?;
            let me = i.me.as_ref()?.id.clone();
            let items = i.dms.items.get(channel).cloned().unwrap_or_default();
            let org = organized(i, server, channel);
            let note = i.dms.thread_notes.get(channel);
            let hours = i.server(server).map(|s| s.thread_archive_hours).unwrap_or(0);
            let now = crate::core::dms::now_ms();
            let seqs = st::by_seq(&items);
            let none = Vec::new();
            let listed: Vec<Listed> = st::search(&org, &seqs, &query)
                .into_iter()
                .filter(|th| st::archived(Some(th), hours, now) == archived)
                .map(|th| {
                    let m = seqs.get(&th.parent).map(|m| (*m).clone());
                    let author = m.as_ref().and_then(|m| i.users.get(&m.sender_id).cloned());
                    let name = m.as_ref().map(|m| i.display_name(Some(server), &m.sender_id)).unwrap_or_default();
                    let unread = if st::following(note, th.parent, &items, &me) {
                        st::unread_in(note, th.parent, org.in_thread.get(&th.parent).unwrap_or(&none), &me)
                    } else {
                        0
                    };
                    let faces = th.participants.iter().take(3).filter_map(|id| i.users.get(id).cloned()).collect();
                    (th.clone(), m, author, name, unread, faces)
                })
                .collect();
            Some((i.channel(server, channel).map(|c| c.name.clone()).unwrap_or_default(), hours, listed))
        });
        let Some((name, hours, listed)) = read else { return div().into_any_element() };
        let hover = p.muted;
        let header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(12.0))
            .h(px(56.0))
            .border_b_1()
            .border_color(p.border)
            .child(icon("messages-square").size(px(20.0)).text_color(p.primary))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(16.0))
                            .line_height(px(20.0))
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .child(t("chat.threads.threads")),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .line_height(px(16.0))
                            .text_color(p.muted_foreground)
                            .child(t_with("chat.secureThreads.where", &[("channel", Arg::Str(&name))])),
                    ),
            )
            .child(
                div()
                    .id("secure-threads-close")
                    .size(px(32.0))
                    .flex_none()
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .text_color(p.muted_foreground)
                    .hover(move |s| s.bg(hover))
                    .tooltip(|window, cx| crate::ui::overlay::Tip::new(t("chat.threads.closeList")).build(window, cx))
                    .on_click(cx.listener(|this, _, window, cx| this.close_secure_side(window, cx)))
                    .child(icon("x").size(px(16.0))),
            );
        let (k, s, c) = (key.to_owned(), server.to_owned(), channel.to_owned());
        let tab = |id: &'static str, glyph: &'static str, label: String, value: bool, cx: &mut Context<Self>| {
            let on = archived == value;
            let (k, s, c) = (k.clone(), s.clone(), c.clone());
            div()
                .id(id)
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .gap(px(4.0))
                .py(px(6.0))
                .rounded(radius_lg())
                .cursor_pointer()
                .text_color(if on { p.foreground } else { p.muted_foreground })
                .on_click(cx.listener(move |_, _, _, cx| {
                    let side = Side::List { key: k.clone(), server: s.clone(), channel: c.clone(), archived: value };
                    SIDE.with(|st| *st.borrow_mut() = Some(side));
                    cx.notify();
                }))
                .child(icon(glyph).size(px(14.0)))
                .child(label)
        };
        let tools = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .p(px(12.0))
            .border_b_1()
            .border_color(p.border)
            // The search is this device's own: every key typed filters again.
            .on_key_up(cx.listener(|_, _, _, cx| cx.notify()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(10.0))
                    .h(px(34.0))
                    .overflow_hidden()
                    .rounded(radius_xl())
                    .border_1()
                    .border_color(p.border)
                    .bg(p.card)
                    .child(icon("search").size(px(16.0)).text_color(p.muted_foreground))
                    .child(
                        div().flex_1().min_w_0().ml(px(-10.0)).child(Input::new(&self.threads.query).appearance(false)),
                    ),
            )
            .when(hours > 0, |el| {
                el.child(
                    div()
                        .flex()
                        .p(px(2.0))
                        .rounded(radius_xl())
                        .bg(p.muted)
                        .text_xs()
                        .line_height(px(16.0))
                        .font_weight(FontWeight::BOLD)
                        .child(
                            div()
                                .relative()
                                .flex_1()
                                .flex()
                                // The picked tab's card glides between them (the web's `layoutId`).
                                .child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .bottom_0()
                                        .left(gpui_kit::relative(0.5 * pill))
                                        .w(gpui_kit::relative(0.5))
                                        .rounded(radius_lg())
                                        .bg(p.card)
                                        .shadow(crate::ui::polls::shadow_sm()),
                                )
                                .child(tab("secure-threads-open", "messages-square", t("chat.threads.open"), false, cx))
                                .child(tab("secure-threads-archived", "archive", t("chat.threads.archived"), true, cx)),
                        ),
                )
            });
        let mut body =
            div().id("secure-threads-list").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().p(px(8.0));
        if listed.is_empty() {
            let searched = !query.trim().is_empty();
            let (title, line) = match (searched, archived) {
                (true, _) => (t("chat.threads.noMatches"), t("chat.secureThreads.searchNote")),
                (false, true) if hours >= 48 => (
                    t("chat.threads.noArchived"),
                    t_with(
                        "chat.threads.archivedHintDays",
                        &[("count", Arg::Num(((hours as f64) / 24.0).round() as i64))],
                    ),
                ),
                (false, true) => (
                    t("chat.threads.noArchived"),
                    t_with("chat.threads.archivedHintHours", &[("count", Arg::Num(i64::from(hours)))]),
                ),
                (false, false) => (t("chat.threads.none"), t("chat.threads.noneHint")),
            };
            body = body.child(motion::rise(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(24.0))
                    .py(px(48.0))
                    .text_center()
                    .child(
                        div()
                            .size(px(48.0))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(alpha(p.primary, 0.1))
                            .text_color(p.primary)
                            .child(icon("messages-square").size(px(24.0))),
                    )
                    .child(div().text_sm().line_height(px(20.0)).font_weight(FontWeight::BOLD).child(title))
                    .child(div().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(line)),
                "secure-threads-none",
                Duration::ZERO,
                8.0,
            ));
        }
        for (n, (th, m, author, name, unread, faces)) in listed.into_iter().enumerate() {
            let parent = th.parent;
            let row_hover = alpha(p.muted, 0.7);
            let preview = m.as_ref().map_or_else(
                || t("chat.secureThreads.notHere"),
                |m| {
                    let text = st::plain(&m.content);
                    if text.is_empty() { "…".to_owned() } else { text }
                },
            );
            let now = crate::core::dms::now_ms();
            let row = div()
                .id(SharedString::from(format!("secure-thread-row|{parent}")))
                .flex()
                .gap(px(10.0))
                .p(px(10.0))
                .rounded(radius_xl())
                .cursor_pointer()
                .hover(move |s| s.bg(row_hover))
                .on_click(
                    cx.listener(move |this, _, window, cx| this.open_secure_thread(&parent.to_string(), window, cx)),
                )
                .child(avatar(author.as_ref(), 32.0, &p))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex()
                                .items_baseline()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_sm()
                                        .line_height(px(20.0))
                                        .font_weight(FontWeight::BOLD)
                                        .child(if m.is_some() { name } else { t("chat.secureThreads.earlier") }),
                                )
                                .when_some(m.as_ref(), |el, m| {
                                    el.child(
                                        div()
                                            .flex_none()
                                            .text_size(px(11.2))
                                            .text_color(p.muted_foreground)
                                            .child(crate::ui::text::ago(m.at, now)),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .text_sm()
                                .line_height(px(20.0))
                                .text_color(p.muted_foreground)
                                .line_clamp(2)
                                .child(preview),
                        )
                        .child(
                            div()
                                .mt(px(4.0))
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .text_xs()
                                .line_height(px(16.0))
                                .child(div().flex().flex_none().children(faces.iter().enumerate().map(|(n, u)| {
                                    div()
                                        .relative()
                                        .size(px(16.0))
                                        .when(n > 0, |el| el.ml(px(-4.0)))
                                        .rounded_full()
                                        .border_1()
                                        .border_color(p.background)
                                        .child(avatar(Some(u), 16.0, &p))
                                })))
                                .child(div().flex_none().font_weight(FontWeight::BOLD).text_color(p.primary).child(
                                    t_with("chat.threads.replies", &[("count", Arg::Num(i64::from(th.replies)))]),
                                ))
                                .when(unread > 0, |el| {
                                    el.child(
                                        div()
                                            .flex_none()
                                            .px(px(6.0))
                                            .rounded_full()
                                            .bg(p.primary)
                                            .text_color(p.primary_foreground)
                                            .text_size(px(10.4))
                                            .font_weight(FontWeight::EXTRA_BOLD)
                                            .child(if unread > 99 {
                                                t("chat.threads.newMany")
                                            } else {
                                                t_with(
                                                    "chat.threads.newCount",
                                                    &[("count", Arg::Num(i64::from(unread)))],
                                                )
                                            }),
                                    )
                                })
                                .when(th.locked, |el| {
                                    el.child(icon("lock").size(px(12.0)).text_color(p.muted_foreground))
                                })
                                .when(th.last_at > 0, |el| {
                                    let time = crate::ui::text::ago(th.last_at, now);
                                    el.child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_color(p.muted_foreground)
                                            .child(t_with("chat.threads.lastAgo", &[("time", Arg::Str(&time))])),
                                    )
                                }),
                        ),
                );
            body = body.child(motion::rise(
                row,
                SharedString::from(format!("secure-thread-in|{parent}")),
                Duration::from_millis(25 * n.min(8) as u64),
                8.0,
            ));
        }
        div().size_full().flex().flex_col().child(header).child(tools).child(body).into_any_element()
    }
}
