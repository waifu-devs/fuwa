//! Threads under a channel's messages, as the web app's `chat/Threads.tsx`:
//! a "N replies" row under each message that has one, "Reply in thread" on
//! hover, a panel beside the channel with the message, its replies and a
//! box of its own to answer in (and "Also send to #channel"), the bell to
//! follow it, the lock for people who manage messages, and the channel's
//! list of threads behind the header's Threads button, searchable, with
//! archived ones on a tab of their own.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, Focusable as _, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task,
    Window, div, px,
};

use crate::core::store::InstanceState;
use crate::core::threads::{self, ThreadTarget};
use crate::pb;
use crate::ui::app::FuwaApp;
use crate::ui::chat::{Built, Row};
use crate::ui::motion;
use crate::ui::text::ms_of;
use crate::ui::theme::{Palette, alpha, corner, mix};
use crate::ui::widgets::{avatar, icon, pal};

/// How long the threads search waits for typing to stop.
const SEARCH_PAUSE: Duration = Duration::from_millis(250);
/// The longest threads search, as the server takes it.
pub const QUERY: usize = 100;

/// A thread open beside its channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Open {
    pub key: String,
    pub server: String,
    pub channel: String,
    /// The message it's under.
    pub id: String,
}

/// A channel's threads, as the list shows them.
pub struct Listing {
    pub key: String,
    pub server: String,
    pub channel: String,
    pub archived: bool,
    pub items: Vec<pb::Message>,
    /// Where the next page starts, when there's one.
    pub after: String,
    pub loading: bool,
    pub error: Option<String>,
    /// What the list was searched for.
    pub query: String,
    /// Counts lists, so an answer to an old one is dropped.
    pub run: u64,
}

/// The replies under a channel's message, as its row shows them.
#[derive(Debug, Clone, PartialEq)]
pub struct Replies {
    pub count: i32,
    /// The latest repliers, up to three.
    pub faces: Vec<pb::User>,
    /// Replies from others since you last looked, in a thread you follow.
    pub new: u32,
    pub locked: bool,
    pub archived: bool,
    pub last_at: i64,
}

impl Replies {
    pub fn of(i: &InstanceState, server: &str, m: &pb::Message, now: i64) -> Option<Self> {
        let t = m.thread.as_ref().filter(|t| t.reply_count > 0 || t.locked)?;
        let hours = i.server(server).map(|s| s.thread_archive_hours).unwrap_or(0);
        Some(Replies {
            count: t.reply_count,
            faces: t.participant_ids.iter().take(3).filter_map(|id| i.users.get(id).cloned()).collect(),
            new: i.thread_unread.get(&m.id).copied().unwrap_or(0),
            locked: t.locked,
            archived: threads::is_archived(t, hours, now),
            last_at: ms_of(t.last_reply_at.as_ref()),
        })
    }
}

/// The thread panel and the threads list.
pub struct Threads {
    pub open: Option<Open>,
    pub listing: Option<Listing>,
    /// Where you reply in the open thread.
    pub reply: Entity<TextareaState>,
    /// Also send the reply to the channel.
    pub also: bool,
    pub rows: Rc<Vec<Row>>,
    pub built: Built,
    pub scroll: ScrollHandle,
    /// How many rows the panel drew last, so it follows new replies down.
    pub drawn: usize,
    pub query: Entity<InputState>,
    pub run: u64,
    pub searching: Option<Task<()>>,
    /// Asks the server about something, so it isn't asked twice at once.
    pub busy: bool,
}

impl Threads {
    pub fn new(window: &mut Window, cx: &mut Context<FuwaApp>) -> (Self, Vec<Subscription>) {
        let reply = cx.new(|cx| {
            TextareaState::new(window, cx).auto_grow(1, 8).submit_on_enter(true).placeholder("Reply in thread")
        });
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search threads"));
        let subs = vec![
            cx.subscribe_in(&reply, window, |this: &mut FuwaApp, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { shift: false, .. } => this.send_reply(window, cx),
                InputEvent::Change | InputEvent::Focus | InputEvent::Blur => cx.notify(),
                _ => {}
            }),
            cx.subscribe_in(&query, window, |this: &mut FuwaApp, _, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    this.search_threads_soon(cx);
                }
            }),
        ];
        (
            Threads {
                open: None,
                listing: None,
                reply,
                also: false,
                rows: Rc::new(Vec::new()),
                built: Built::default(),
                scroll: ScrollHandle::new(),
                drawn: 0,
                query,
                run: 0,
                searching: None,
                busy: false,
            },
            subs,
        )
    }
}

impl FuwaApp {
    // ───────────────────────── Opening and closing ─────────────────────────

    /// Opens the thread under a message of the open channel beside it.
    pub(crate) fn open_thread(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(crate::ui::app::Target::Secure { .. }) = self.target() {
            return self.open_secure_thread(&id, window, cx);
        }
        let Some(crate::ui::app::Target::Channel { key, server, channel }) = self.target() else { return };
        let open = Open { key: key.clone(), server: server.clone(), channel: channel.clone(), id: id.clone() };
        if self.threads.open.as_ref() == Some(&open) {
            return;
        }
        self.pins = None;
        if self.search.panel.is_some() {
            self.close_search(window, cx);
        }
        self.threads.open = Some(open);
        self.threads.also = false;
        self.threads.drawn = 0;
        self.threads.built.clear();
        self.threads.reply.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.focus(window, cx);
        });
        self.core.focus_thread(&key, Some(&id));
        crate::core::reports::used("thread.open");
        let core = self.core.clone();
        self.run(
            cx,
            async move {
                core.load_followed(&key, &server).await.ok();
                core.load_thread(&key, &server, &channel, &id, false).await
            },
            |this, _, cx| {
                this.sync_thread(cx);
                cx.notify();
            },
        );
        self.sync_thread(cx);
        cx.notify();
    }

    pub(crate) fn close_thread(&mut self, cx: &mut Context<Self>) {
        if let Some(open) = self.threads.open.take() {
            self.core.focus_thread(&open.key, None);
            if self.pins.as_ref().is_some_and(|p| p.of_thread(&open.id)) {
                self.pins = None;
            }
        }
        self.threads.rows = Rc::new(Vec::new());
        self.threads.built.clear();
        cx.notify();
    }

    /// Opens the channel's list of threads, or closes it.
    pub(crate) fn toggle_threads_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.threads.listing.is_some() {
            self.threads.listing = None;
            cx.notify();
            return;
        }
        let Some(crate::ui::app::Target::Channel { key, server, channel }) = self.target() else { return };
        if self.search.panel.is_some() {
            self.close_search(window, cx);
        }
        self.close_thread(cx);
        self.pins = None;
        self.threads.query.update(cx, |state, cx| state.set_value("", window, cx));
        self.threads.listing = Some(Listing {
            key,
            server,
            channel,
            archived: false,
            items: Vec::new(),
            after: String::new(),
            loading: true,
            error: None,
            query: String::new(),
            run: 0,
        });
        self.fetch_threads(false, cx);
    }

    /// Moving elsewhere closes what belonged to the channel left.
    pub(crate) fn threads_after_move(&mut self, cx: &mut Context<Self>) {
        let here = match self.target() {
            Some(crate::ui::app::Target::Channel { key, channel, .. }) => Some((key, channel)),
            _ => None,
        };
        let stays = |k: &str, c: &str| here.as_ref().is_some_and(|(key, channel)| key == k && channel == c);
        if self.threads.open.as_ref().is_some_and(|o| !stays(&o.key, &o.channel)) {
            self.close_thread(cx);
        }
        if self.threads.listing.as_ref().is_some_and(|l| !stays(&l.key, &l.channel)) {
            self.threads.listing = None;
        }
    }

    // ───────────────────────── The list ─────────────────────────

    fn search_threads_soon(&mut self, cx: &mut Context<Self>) {
        let typed: String = self.threads.query.read(cx).value().trim().chars().take(QUERY).collect();
        if self.threads.listing.as_ref().is_none_or(|l| l.query == typed) {
            return;
        }
        self.threads.searching = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_PAUSE).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(listing) = &mut this.threads.listing {
                    listing.query = typed;
                    this.fetch_threads(false, cx);
                }
            });
        }));
    }

    pub(crate) fn show_archived_threads(&mut self, archived: bool, cx: &mut Context<Self>) {
        let Some(listing) = &mut self.threads.listing else { return };
        if listing.archived == archived {
            return;
        }
        listing.archived = archived;
        self.fetch_threads(false, cx);
    }

    /// The first page of the list, or with `more` the next.
    pub(crate) fn fetch_threads(&mut self, more: bool, cx: &mut Context<Self>) {
        self.threads.run += 1;
        let run = self.threads.run;
        let Some(listing) = &mut self.threads.listing else { return };
        if more && (listing.after.is_empty() || listing.loading) {
            return;
        }
        if !more {
            listing.items.clear();
            listing.after.clear();
        }
        listing.loading = true;
        listing.error = None;
        listing.run = run;
        let (key, server, channel) = (listing.key.clone(), listing.server.clone(), listing.channel.clone());
        let (query, archived, after) = (listing.query.clone(), listing.archived, listing.after.clone());
        let core = self.core.clone();
        self.run(
            cx,
            async move { core.list_threads(&key, &server, &channel, &query, archived, &after).await },
            move |this, result, cx| {
                let Some(listing) = this.threads.listing.as_mut().filter(|l| l.run == run) else { return };
                listing.loading = false;
                match result {
                    Ok(res) => {
                        for t in res.threads {
                            if !listing.items.iter().any(|m| m.id == t.id) {
                                listing.items.push(t);
                            }
                        }
                        listing.after = if res.has_more { res.next_after_thread_id } else { String::new() };
                    }
                    Err(err) => listing.error = Some(err.message),
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    /// Opens a thread from the list, closing the list.
    fn open_listed(&mut self, message: pb::Message, window: &mut Window, cx: &mut Context<Self>) {
        // A thread listed but not loaded yet: its message is what the panel shows until it is.
        if let Some(listing) = &self.threads.listing {
            let key = listing.key.clone();
            self.core.shared.update(|s| {
                if let Some(i) = s.instances.get_mut(&key) {
                    i.thread_parents.entry(message.id.clone()).or_insert_with(|| message.clone());
                }
            });
        }
        self.threads.listing = None;
        self.open_thread(message.id, window, cx);
    }

    // ───────────────────────── Replying ─────────────────────────

    pub(crate) fn send_reply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(crate::ui::app::Target::Secure { .. }) = self.target() {
            return self.send_secure_reply(window, cx);
        }
        let Some(open) = self.threads.open.clone() else { return };
        let text = self.threads.reply.read(cx).value().trim().to_owned();
        if text.is_empty() || self.thread_blocked().is_some() {
            return;
        }
        self.threads.reply.update(cx, |state, cx| state.set_value("", window, cx));
        let text = self.encode_mentions(&text);
        let target = ThreadTarget { thread_id: open.id.clone(), also_to_channel: self.threads.also };
        let core = self.core.clone();
        self.run(
            cx,
            async move { core.send_reply(&open.key, &open.server, &open.channel, &text, target).await },
            |this, _, cx| {
                this.sync_thread(cx);
                cx.notify();
            },
        );
        self.sync_thread(cx);
    }

    /// Why you can't reply in the open thread, if you can't.
    pub(crate) fn thread_blocked(&self) -> Option<String> {
        let open = self.threads.open.as_ref()?;
        if let Some(blocked) = self.channel_blocked(&open.key, &open.server, &open.channel) {
            return Some(blocked.text);
        }
        self.core.shared.read(|s| {
            let i = s.instance(&open.key)?;
            let parent = i
                .thread_parents
                .get(&open.id)
                .or_else(|| i.messages.get(&open.channel)?.items.iter().find(|m| m.id == open.id))?;
            let locked = parent.thread.as_ref().is_some_and(|t| t.locked);
            let manage = i.access(&open.server).has_in(&open.channel, pb::Permission::ManageMessages);
            (locked && !manage).then(|| "This thread is locked. Only moderators can reply.".to_owned())
        })
    }

    pub(crate) fn follow_open_thread(&mut self, follow: bool, cx: &mut Context<Self>) {
        let Some(open) = self.threads.open.clone() else { return };
        let core = self.core.clone();
        self.run(
            cx,
            async move { core.follow_thread(&open.key, &open.server, &open.channel, &open.id, follow).await },
            move |this, result, cx| match result {
                Ok(()) => this.toast(
                    if follow { "bell-ring" } else { "bell-off" },
                    if follow { "Following this thread".into() } else { "Stopped following".into() },
                    if follow {
                        "You'll hear about new replies here.".into()
                    } else {
                        "You won't hear about this thread anymore.".into()
                    },
                    None,
                    None,
                    cx,
                ),
                Err(err) => this.toast("circle-alert", "Couldn't change that".into(), err.message, None, None, cx),
            },
        );
    }

    pub(crate) fn lock_open_thread(&mut self, locked: bool, cx: &mut Context<Self>) {
        let Some(open) = self.threads.open.clone() else { return };
        let core = self.core.clone();
        self.run(
            cx,
            async move { core.lock_thread(&open.key, &open.server, &open.channel, &open.id, locked).await },
            |this, result, cx| {
                if let Err(err) = result {
                    this.toast("circle-alert", "Couldn't change that".into(), err.message, None, None, cx);
                }
                this.sync_thread(cx);
                cx.notify();
            },
        );
    }

    pub(crate) fn load_older_replies(&mut self, cx: &mut Context<Self>) {
        let Some(open) = self.threads.open.clone() else { return };
        let core = self.core.clone();
        self.run(
            cx,
            async move { core.load_thread(&open.key, &open.server, &open.channel, &open.id, true).await },
            |this, _, cx| {
                this.sync_thread(cx);
                cx.notify();
            },
        );
    }
}

// ───────────────────────── Drawing ─────────────────────────

/// The row under a message with replies: who replied, how many, and how
/// recently. Clicking it opens the thread.
pub(crate) fn replies_row(
    id: &str,
    r: &Replies,
    p: &Palette,
    this: &gpui_kit::WeakEntity<FuwaApp>,
) -> impl IntoElement {
    use crate::core::i18n::{Arg, t, t_with};
    let (this, open) = (this.clone(), id.to_owned());
    let last = if r.archived {
        t("chat.threads.archived")
    } else {
        let time = crate::ui::text::ago(r.last_at, crate::core::dms::now_ms());
        t_with("chat.threads.lastReply", &[("time", Arg::Str(&time))])
    };
    let (hover_bg, hover_border) = (alpha(p.card, 0.7), p.border);
    motion::rise(
        div()
            .id(SharedString::from(format!("replies|{id}")))
            .group("replies")
            .mt(px(4.0))
            .ml(px(-6.0))
            .max_w_full()
            .flex()
            .flex_none()
            .self_start()
            .items_center()
            .gap(px(8.0))
            .px(px(6.0))
            .py(px(4.0))
            .rounded(crate::ui::theme::radius_xl())
            .border_1()
            .border_color(gpui_kit::transparent_black())
            .text_xs()
            .line_height(px(16.0))
            .cursor_pointer()
            .hover(move |s| s.bg(hover_bg).border_color(hover_border))
            .active(|s| s.scale(0.98))
            .on_click(move |_, window, cx| {
                let open = open.clone();
                let _ = this.update(cx, |this, cx| this.open_thread(open, window, cx));
            })
            // Up to three faces, overlapping, each in a ring of the page's colour.
            .child(div().flex().flex_none().children(r.faces.iter().enumerate().map(|(n, u)| {
                div().relative().size(px(20.0)).when(n > 0, |el| el.ml(px(-6.0))).child(
                    div()
                        .absolute()
                        .left(px(-2.0))
                        .top(px(-2.0))
                        .size(px(24.0))
                        .rounded_full()
                        .bg(p.background)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(avatar(Some(u), 20.0, p)),
                )
            })))
            // The count rolls when replies come in (the web's `Count`).
            .child(
                crate::ui::motion::counted(
                    SharedString::from(format!("replies-count|{id}")),
                    "chat.threads.replies",
                    r.count.max(0) as u64,
                    12.0,
                )
                .flex_none()
                .font_weight(FontWeight::BOLD)
                .text_color(p.primary),
            )
            .when(r.new > 0, |el| {
                el.child(motion::pop_in(
                    div()
                        .flex_none()
                        .px(px(6.0))
                        .rounded_full()
                        .bg(p.primary)
                        .text_color(p.primary_foreground)
                        .text_size(px(10.4))
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .child(if r.new > 99 {
                            t("chat.threads.newMany")
                        } else {
                            t_with("chat.threads.newCount", &[("count", Arg::Num(i64::from(r.new)))])
                        }),
                    SharedString::from(format!("replies-new|{id}")),
                    (0.5, 0.5),
                    0.0,
                    0.0,
                ))
            })
            .when(r.locked, |el| el.child(icon("lock").size(px(12.0)).text_color(p.muted_foreground)))
            // The last reply, or "View thread" while hovered, in one place: one
            // rises out as the other rises in.
            .child(
                div()
                    .relative()
                    .min_w_0()
                    .text_color(p.muted_foreground)
                    .child(
                        div()
                            .id(SharedString::from(format!("replies-last|{id}")))
                            .truncate()
                            .group_hover("replies", |s| s.opacity(0.0).translate_y(px(-4.0)))
                            .child(last),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("replies-view|{id}")))
                            .absolute()
                            .left_0()
                            .top_0()
                            .whitespace_nowrap()
                            .opacity(0.0)
                            .translate_y(px(4.0))
                            .group_hover("replies", |s| s.opacity(1.0).translate_y(px(0.0)))
                            .child(t("chat.threads.view")),
                    ),
            ),
        SharedString::from(format!("replies-in|{id}")),
        Duration::ZERO,
        4.0,
    )
}

/// Under a reply also sent to the channel: where else it is. In the channel
/// it opens the thread.
pub(crate) fn also_note(
    id: &str,
    thread: Option<&str>,
    p: &Palette,
    this: &gpui_kit::WeakEntity<FuwaApp>,
) -> impl IntoElement {
    let el = div()
        .id(SharedString::from(format!("also|{id}")))
        .flex()
        .items_center()
        .gap(px(4.0))
        .text_size(px(11.2))
        .line_height(px(18.2))
        .font_weight(FontWeight::BOLD)
        .text_color(p.muted_foreground);
    match thread {
        Some(thread) => {
            let (this, thread) = (this.clone(), thread.to_owned());
            el.cursor_pointer()
                .hover(|s| s.text_color(p.primary))
                .on_click(move |_, window, cx| {
                    let thread = thread.clone();
                    let _ = this.update(cx, |this, cx| this.open_thread(thread, window, cx));
                })
                .child(icon("corner-down-right").size(px(12.0)))
                .child(crate::core::i18n::t("chat.threads.repliedInThread"))
        }
        None => el.child(crate::core::i18n::t("chat.threads.alsoSent")),
    }
}

impl FuwaApp {
    /// The open thread beside its channel: the message, its replies and where you answer.
    pub(crate) fn thread_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.threads.open.clone()?;
        let p = pal(cx);
        let (name, locked, archived, following, manage, loading, pins_here) = self.core.shared.read(|s| {
            let i = s.instance(&open.key)?;
            let channel = i.channel(&open.server, &open.channel)?;
            let parent = i
                .thread_parents
                .get(&open.id)
                .or_else(|| i.messages.get(&open.channel)?.items.iter().find(|m| m.id == open.id));
            let summary = parent.and_then(|m| m.thread.as_ref());
            let hours = i.server(&open.server).map(|s| s.thread_archive_hours).unwrap_or(0);
            Some((
                channel.name.clone(),
                summary.is_some_and(|t| t.locked),
                summary.is_some_and(|t| threads::is_archived(t, hours, crate::core::dms::now_ms())),
                threads::follows(i, &open.server, &open.id),
                i.access(&open.server).has_in(&open.channel, pb::Permission::ManageMessages),
                i.messages.get(&threads::thread_key(&open.id)).is_none_or(|l| l.loading && l.items.is_empty()),
                // A shared channel's pins are its home's; a guest doesn't list them.
                i.has("pins") && channel.shared.as_ref().is_none_or(|s| s.home),
            ))
        })?;

        use crate::core::i18n::{Arg, t, t_with};
        // The web's `PanelButton`: 32px and round, a 16px icon, the primary at 10% while on,
        // pressed down to 85%. The bell pops in, turning upright, when it changes.
        let panel_button = |id: &'static str, glyph: &'static str, label: String, on: bool| {
            let hover = p.muted;
            let glyph: AnyElement = match id {
                "thread-follow" => motion::pop(
                    div().child(icon(glyph).size(px(16.0))),
                    SharedString::from(format!("{id}|{glyph}")),
                    0.6,
                    -25.0,
                    Duration::ZERO,
                )
                .into_any_element(),
                _ => icon(glyph).size(px(16.0)).into_any_element(),
            };
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
                .active(|s| s.scale(0.85))
                .tooltip(move |window, cx| crate::ui::overlay::Tip::new(label.clone()).build(window, cx))
                .child(glyph)
        };
        let where_key = match (archived, locked) {
            (true, true) => "chat.threads.whereArchivedLocked",
            (true, false) => "chat.threads.whereArchived",
            (false, true) => "chat.threads.whereLocked",
            (false, false) => "chat.threads.where",
        };
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
            .when_some(following, |el, following| {
                let label = if following { t("chat.threads.unfollow") } else { t("chat.threads.follow") };
                el.child(
                    panel_button("thread-follow", if following { "bell" } else { "bell-off" }, label, following)
                        .on_click(cx.listener(move |this, _, _, cx| this.follow_open_thread(!following, cx))),
                )
            })
            .when(manage, |el| {
                let label = if locked { t("chat.threads.unlock") } else { t("chat.threads.lock") };
                el.child(
                    panel_button("thread-lock", if locked { "lock" } else { "lock-open" }, label, locked)
                        .on_click(cx.listener(move |this, _, _, cx| this.lock_open_thread(!locked, cx))),
                )
            })
            .when(pins_here, |el| {
                let open = self.pins.as_ref().is_some_and(|p| p.of_thread(&open.id));
                el.child(self.pins_button("thread-pins", open, cx))
            })
            .child(
                panel_button("thread-jump", "corner-up-left", t("chat.threads.jump"), false)
                    .on_click(cx.listener(|this, _, window, cx| this.jump_to_thread_message(window, cx))),
            )
            .child(panel_button("thread-close", "x", t("chat.threads.close"), false).on_click(cx.listener(
                |this, _, window, cx| {
                    this.close_thread(cx);
                    this.composer.update(cx, |state, cx| state.focus(window, cx));
                },
            )));

        // New replies keep the panel at the bottom, as the channel does.
        let len = self.threads.rows.len();
        if len != self.threads.drawn {
            let max = self.threads.scroll.max_offset();
            let at_end = self.threads.drawn == 0 || f32::from(max.y) + f32::from(self.threads.scroll.offset().y) < 80.0;
            if at_end {
                self.threads.scroll.scroll_to_bottom();
            }
            self.threads.drawn = len;
        }
        let ctx = self.row_ctx(Some(open.id.clone()), cx);
        let rows = self.threads.rows.clone();
        let body = div()
            .id(SharedString::from(format!("thread-rows|{}", open.id)))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.threads.scroll)
            .child(
                // Short threads sit at the bottom, by the box, as the channel's messages do.
                div()
                    .min_h_full()
                    .flex()
                    .flex_col()
                    .justify_end()
                    .pt(px(12.0))
                    .pb(px(12.0))
                    .when(loading, |el| el.child(crate::ui::chat_rows::skeleton(2, &p)))
                    .when(!loading, |el| {
                        el.children(
                            rows.iter().enumerate().map(|(ix, row)| crate::ui::chat::render_row(row, ix, &ctx, cx)),
                        )
                    }),
            );

        let reply = self.reply_box(&p, window, cx);
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
                    .child(header)
                    .child(body)
                    .child(reply),
                SharedString::from(format!("thread-panel|{}", open.id)),
                24.0,
            )
            .into_any_element(),
        )
    }

    /// Where you answer in the open thread, and whether the channel sees it too.
    fn reply_box(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = *p;
        if let Some(why) = self.thread_blocked() {
            return div()
                .flex_none()
                .p(px(12.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .px(px(14.0))
                        .py(px(10.0))
                        .rounded(corner(14.0))
                        .bg(alpha(p.muted_foreground, 0.1))
                        .text_sm()
                        .text_color(p.muted_foreground)
                        .child(icon("lock").size(px(15.0)))
                        .child(div().flex_1().child(why)),
                )
                .into_any_element();
        }
        let channel = self.threads.open.as_ref().and_then(|o| {
            self.core
                .shared
                .read(|s| s.instance(&o.key).and_then(|i| i.channel(&o.server, &o.channel)).map(|c| c.name.clone()))
        });
        let focused = self.threads.reply.read(cx).focus_handle(cx).is_focused(window);
        let typed = !self.threads.reply.read(cx).value().trim().is_empty();
        let ring = motion::follow("thread-reply-ring", if focused { 1.0 } else { 0.0 }, window, cx);
        let ready = motion::follow("thread-reply-send", if typed { 1.0 } else { 0.0 }, window, cx);
        let also = self.threads.also;
        // The web's composer card (`.composer`), lit while focused.
        div()
            .flex_none()
            .px(px(16.0))
            .pb(px(12.0))
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap(px(8.0))
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
                    .child(div().flex_1().min_w_0().ml(px(-10.0)).my(px(-2.0)).child(
                        Textarea::new(&self.threads.reply).appearance(false).text_size(px(15.2)).line_height(px(24.0)),
                    ))
                    .child(
                        div()
                            .id("thread-send")
                            .size(px(36.0))
                            .mb(px(2.0))
                            .flex_none()
                            .rounded(crate::ui::theme::radius_xl())
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(alpha(p.primary, ready))
                            .text_color(mix(p.muted_foreground, p.primary_foreground, ready))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, window, cx| this.send_reply(window, cx)))
                            .child(icon("send-horizontal").size(px(18.0))),
                    ),
            )
            .when_some(channel, |el, name| {
                el.child(
                    div()
                        .id("thread-also")
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .px(px(4.0))
                        .text_size(px(11.2))
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.muted_foreground)
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.threads.also = !this.threads.also;
                            cx.notify();
                        }))
                        .child(
                            div()
                                .size(px(14.0))
                                .flex_none()
                                .rounded(corner(4.0))
                                .border_1()
                                .border_color(if also { p.primary } else { p.border })
                                .bg(if also { p.primary.into() } else { gpui_kit::transparent_black() })
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(also, |el| {
                                    el.child(icon("check").size(px(11.0)).text_color(p.primary_foreground))
                                }),
                        )
                        .child(crate::core::i18n::t_with(
                            "chat.composer.alsoSend",
                            &[("channel", crate::core::i18n::Arg::Str(&name))],
                        )),
                )
            })
            .into_any_element()
    }

    /// Shows the message the open thread is under, in its channel.
    fn jump_to_thread_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.threads.open.clone() else { return };
        let message = self.core.shared.read(|s| s.instance(&open.key)?.thread_parents.get(&open.id).cloned());
        let message = message.unwrap_or(pb::Message {
            id: open.id.clone(),
            channel_id: open.channel.clone(),
            ..Default::default()
        });
        self.jump_to_message(&open.key, &open.server, message, window, cx);
    }

    /// The channel's threads, behind the header's Threads button (`ThreadList`).
    pub(crate) fn threads_list_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        use crate::core::i18n::{Arg, t, t_with};
        let listing = self.threads.listing.as_ref()?;
        let p = pal(cx);
        let (hours, channel_name) = self.core.shared.read(|s| {
            let i = s.instance(&listing.key);
            (
                i.and_then(|i| i.server(&listing.server)).map(|s| s.thread_archive_hours).unwrap_or(0),
                i.and_then(|i| i.channel(&listing.server, &listing.channel))
                    .map(|c| c.name.clone())
                    .unwrap_or_default(),
            )
        });
        let archived = listing.archived;
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
                            .child(t_with("chat.threads.where", &[("channel", Arg::Str(&channel_name))])),
                    ),
            )
            .child(
                div()
                    .id("threads-close")
                    .size(px(32.0))
                    .flex_none()
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .text_color(p.muted_foreground)
                    .hover(move |s| s.bg(hover))
                    .tooltip(|window, cx| {
                        crate::ui::overlay::Tip::new(crate::core::i18n::t("chat.threads.closeList")).build(window, cx)
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.threads.listing = None;
                        cx.notify();
                    }))
                    .child(icon("x").size(px(16.0))),
            );
        // Open and Archived, the picked one on a card that glides (the web's `layoutId`).
        let tab = |id: &'static str, glyph: &'static str, label: String, value: bool, cx: &mut Context<Self>| {
            let on = archived == value;
            div()
                .id(id)
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .gap(px(4.0))
                .py(px(6.0))
                .rounded(crate::ui::theme::radius_lg())
                .cursor_pointer()
                .text_color(if on { p.foreground } else { p.muted_foreground })
                .on_click(cx.listener(move |this, _, _, cx| this.show_archived_threads(value, cx)))
                .child(icon(glyph).size(px(14.0)))
                .child(label)
        };
        let glide = motion::follow("threads-tab-glide", if archived { 1.0 } else { 0.0 }, window, cx);
        let focused = self.threads.query.read(cx).focus_handle(cx).is_focused(window);
        let tools = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .p(px(12.0))
            .border_b_1()
            .border_color(p.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(10.0))
                    .h(px(34.0))
                    .overflow_hidden()
                    .rounded(crate::ui::theme::radius_xl())
                    .border_1()
                    // `focus-within:border-primary/50`.
                    .border_color(if focused { alpha(p.primary, 0.5) } else { p.border.into() })
                    .bg(p.card)
                    .child(icon("search").size(px(16.0)).text_color(p.muted_foreground))
                    .child(
                        div().flex_1().min_w_0().ml(px(-10.0)).child(Input::new(&self.threads.query).appearance(false)),
                    ),
            )
            .when(hours > 0, |el| {
                el.child(
                    div()
                        .relative()
                        .flex()
                        .p(px(2.0))
                        .rounded(crate::ui::theme::radius_xl())
                        .bg(p.muted)
                        .text_xs()
                        .line_height(px(16.0))
                        .font_weight(FontWeight::BOLD)
                        // The card under the picked tab, half the width, sliding between the two.
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .p(px(2.0))
                                .flex()
                                .child(div().w(gpui_kit::relative(glide * 0.5)))
                                .child(
                                    div()
                                        .w(gpui_kit::relative(0.5))
                                        .h_full()
                                        .rounded(crate::ui::theme::radius_lg())
                                        .bg(p.card)
                                        .shadow(crate::ui::polls::shadow_sm()),
                                ),
                        )
                        .child(tab("threads-open", "messages-square", t("chat.threads.open"), false, cx))
                        .child(tab("threads-archived", "archive", t("chat.threads.archived"), true, cx)),
                )
            });

        let listing = self.threads.listing.as_ref()?;
        let users = self.core.shared.read(|s| s.instance(&listing.key).map(|i| i.users.clone())).unwrap_or_default();
        let searched = !listing.query.is_empty();
        let mut body = div().id("threads-list").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().p(px(8.0));
        if let Some(error) = &listing.error {
            body = body.child(div().p(px(12.0)).text_sm().text_color(p.destructive).child(error.clone()));
        } else if !listing.loading && listing.items.is_empty() && listing.after.is_empty() {
            let (title, line) = match (searched, archived) {
                (true, _) => (t("chat.threads.noMatches"), t("chat.threads.tryOtherWords")),
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
                    .child(div().text_sm().font_weight(FontWeight::BOLD).child(title))
                    .child(div().text_xs().text_color(p.muted_foreground).child(line)),
                "threads-none",
                Duration::ZERO,
                8.0,
            ));
        }
        for (n, m) in listing.items.iter().enumerate() {
            body = body.child(self.thread_list_row(m, n, &users, &p, cx));
        }
        if listing.loading {
            body = body.child(div().m(px(8.0)).h(px(64.0)).rounded(crate::ui::theme::radius_xl()).bg(p.muted));
        } else if !listing.after.is_empty() {
            let primary = alpha(p.primary, 0.1);
            body = body.child(
                div().flex().justify_center().my(px(8.0)).child(
                    div()
                        .id("threads-more")
                        .px(px(12.0))
                        .py(px(4.0))
                        .rounded_full()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.primary)
                        .cursor_pointer()
                        .hover(move |s| s.bg(primary))
                        .child(if listing.items.is_empty() {
                            t("chat.threads.searchOlder")
                        } else {
                            t("chat.threads.showMore")
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.fetch_threads(true, cx))),
                ),
            );
        }
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
                    .child(header)
                    .child(tools)
                    .child(body),
                "threads-list-in",
                24.0,
            )
            .into_any_element(),
        )
    }

    /// One thread in the list (`ThreadRow`): who started it, what it says, and how its replies are going.
    fn thread_list_row(
        &self,
        m: &pb::Message,
        n: usize,
        users: &std::collections::HashMap<String, pb::User>,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::core::i18n::{Arg, t_with};
        let p = *p;
        let author = match &m.webhook {
            Some(w) => w.name.clone(),
            None => users.get(&m.author_id).map(crate::core::store::user_name).unwrap_or_else(|| "Someone".into()),
        };
        let summary = m.thread.clone().unwrap_or_default();
        let now = crate::core::dms::now_ms();
        // Previews show text, not Markdown's marks.
        let plain =
            m.content.replace(['*', '_', '~', '`', '>', '#'], "").split_whitespace().collect::<Vec<_>>().join(" ");
        let preview = if plain.is_empty() {
            m.embeds.first().map(|e| e.title.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| "…".to_owned())
        } else {
            plain
        };
        let last = crate::ui::text::ago(ms_of(summary.last_reply_at.as_ref()), now);
        let open = m.clone();
        let hover = alpha(p.muted, 0.7);
        let faces: Vec<pb::User> =
            summary.participant_ids.iter().take(3).filter_map(|id| users.get(id).cloned()).collect();
        motion::rise(
            div()
                .id(SharedString::from(format!("thread-row|{}", m.id)))
                .flex()
                .gap(px(10.0))
                .p(px(10.0))
                .rounded(crate::ui::theme::radius_xl())
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .on_click(cx.listener(move |this, _, window, cx| this.open_listed(open.clone(), window, cx)))
                .child(avatar(users.get(&m.author_id), 32.0, &p))
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
                                .child(div().min_w_0().truncate().text_sm().font_weight(FontWeight::BOLD).child(author))
                                .child(
                                    div()
                                        .flex_none()
                                        .text_size(px(11.2))
                                        .text_color(p.muted_foreground)
                                        .child(crate::ui::text::ago(ms_of(m.created_at.as_ref()), now)),
                                ),
                        )
                        .child(
                            div()
                                .text_sm()
                                .line_height(px(20.0))
                                .line_clamp(2)
                                .text_color(p.muted_foreground)
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
                                    div().relative().size(px(16.0)).when(n > 0, |el| el.ml(px(-6.0))).child(
                                        div()
                                            .absolute()
                                            .left(px(-2.0))
                                            .top(px(-2.0))
                                            .size(px(20.0))
                                            .rounded_full()
                                            .bg(p.background)
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(avatar(Some(u), 16.0, &p)),
                                    )
                                })))
                                .child(div().font_weight(FontWeight::BOLD).text_color(p.primary).child(t_with(
                                    "chat.threads.replies",
                                    &[("count", Arg::Num(i64::from(summary.reply_count)))],
                                )))
                                .when(summary.locked, |el| {
                                    el.child(icon("lock").size(px(12.0)).text_color(p.muted_foreground))
                                })
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_color(p.muted_foreground)
                                        .child(t_with("chat.threads.lastAgo", &[("time", Arg::Str(&last))])),
                                ),
                        ),
                ),
            SharedString::from(format!("thread-row-in|{}", m.id)),
            Duration::from_millis(25 * n.min(8) as u64),
            8.0,
        )
        .into_any_element()
    }
}
