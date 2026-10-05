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
use crate::ui::text::{ms_of, when};
use crate::ui::theme::{Palette, alpha, corner, mix};
use crate::ui::widgets::{avatar, icon, icon_button, pal};

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
    let (this, open) = (this.clone(), id.to_owned());
    let word = if r.count == 1 { "reply" } else { "replies" };
    div()
        .id(SharedString::from(format!("replies|{id}")))
        .group("replies")
        .mt(px(6.0))
        .max_w(px(480.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .px(px(8.0))
        .py(px(5.0))
        .rounded(corner(12.0))
        .border_1()
        .border_color(gpui_kit::transparent_black())
        .cursor_pointer()
        .hover(|s| s.bg(p.card).border_color(p.border))
        .on_click(move |_, window, cx| {
            let open = open.clone();
            let _ = this.update(cx, |this, cx| this.open_thread(open, window, cx));
        })
        .child(div().flex().flex_none().children(r.faces.iter().enumerate().map(|(n, u)| {
            div()
                .when(n > 0, |el| el.ml(px(-6.0)))
                .rounded_full()
                .border_2()
                .border_color(p.chat_surface)
                .child(avatar(Some(u), 20.0, p))
        })))
        .child(
            div()
                .flex_none()
                .text_sm()
                .font_weight(FontWeight::EXTRA_BOLD)
                .text_color(p.primary)
                .child(format!("{} {word}", r.count)),
        )
        .when(r.new > 0, |el| {
            el.child(motion::rise(
                div()
                    .flex_none()
                    .px(px(6.0))
                    .rounded_full()
                    .bg(p.primary)
                    .text_color(p.primary_foreground)
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .child(format!("{} new", r.new)),
                SharedString::from(format!("replies-new|{id}|{}", r.new)),
                Duration::ZERO,
                4.0,
            ))
        })
        .when(r.locked, |el| el.child(icon("lock").size(px(13.0)).text_color(p.muted_foreground)))
        .child(div().flex_1().min_w_0().truncate().text_xs().text_color(p.muted_foreground).child(if r.archived {
            "Archived".to_owned()
        } else if r.count == 0 {
            "No replies yet".to_owned()
        } else {
            format!("Last reply {}", when(r.last_at))
        }))
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(2.0))
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(p.muted_foreground)
                .opacity(0.0)
                .group_hover("replies", |s| s.opacity(1.0))
                .child("View thread")
                .child(icon("chevron-right").size(px(14.0))),
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
        .mb(px(2.0))
        .flex()
        .items_center()
        .gap(px(4.0))
        .text_xs()
        .text_color(p.muted_foreground)
        .child(icon("corner-down-right").size(px(13.0)));
    match thread {
        Some(thread) => {
            let (this, thread) = (this.clone(), thread.to_owned());
            el.cursor_pointer()
                .hover(|s| s.text_color(p.primary))
                .on_click(move |_, window, cx| {
                    let thread = thread.clone();
                    let _ = this.update(cx, |this, cx| this.open_thread(thread, window, cx));
                })
                .child("Replied in a thread")
        }
        None => el.child("Also sent to the channel"),
    }
}

impl FuwaApp {
    /// The open thread beside its channel: the message, its replies and where you answer.
    pub(crate) fn thread_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.threads.open.clone()?;
        let p = pal(cx);
        let (name, locked, archived, following, manage, loading) = self.core.shared.read(|s| {
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
            ))
        })?;

        let header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.0))
            .pl(px(16.0))
            .pr(px(10.0))
            .h(px(56.0))
            .border_b_1()
            .border_color(p.border)
            .child(icon("messages-square").size(px(18.0)).text_color(p.primary))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_baseline()
                    .gap(px(6.0))
                    .child(div().flex_none().text_lg().font_weight(FontWeight::EXTRA_BOLD).child("Thread"))
                    .child(div().min_w_0().truncate().text_sm().text_color(p.muted_foreground).child(format!(
                        "#{name}{}{}",
                        if archived { " · archived" } else { "" },
                        if locked { " · locked" } else { "" }
                    ))),
            )
            .when_some(following, |el, following| {
                el.child(
                    icon_button("thread-follow", if following { "bell-ring" } else { "bell" }, &p)
                        .when(following, |el| el.text_color(p.primary).bg(alpha(p.primary, 0.12)))
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(if following {
                                "Stop following"
                            } else {
                                "Follow this thread"
                            })
                            .build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| this.follow_open_thread(!following, cx))),
                )
            })
            .when(manage, |el| {
                el.child(
                    icon_button("thread-lock", if locked { "lock" } else { "lock-open" }, &p)
                        .when(locked, |el| el.text_color(p.primary).bg(alpha(p.primary, 0.12)))
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(if locked {
                                "Unlock thread"
                            } else {
                                "Lock thread"
                            })
                            .build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| this.lock_open_thread(!locked, cx))),
                )
            })
            .child(
                icon_button("thread-jump", "arrow-up-right", &p)
                    .tooltip(|window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new("Jump to the message").build(window, cx)
                    })
                    .on_click(cx.listener(|this, _, window, cx| this.jump_to_thread_message(window, cx))),
            )
            .child(icon_button("thread-close", "x", &p).on_click(cx.listener(|this, _, window, cx| {
                this.close_thread(cx);
                this.composer.update(cx, |state, cx| state.focus(window, cx));
            })));

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
            .flex()
            .flex_col()
            .pb(px(12.0))
            .when(loading, |el| el.child(crate::ui::sidebar::loading_rows(&p)))
            .when(!loading, |el| {
                el.children(rows.iter().enumerate().map(|(ix, row)| crate::ui::chat::render_row(row, ix, &ctx, cx)))
            });

        let reply = self.reply_box(&p, window, cx);
        Some(
            motion::slide_in(
                div()
                    .w(px(420.0))
                    .h_full()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .border_l_1()
                    .border_color(p.border)
                    .bg(p.chat_surface)
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
        div()
            .flex_none()
            .px(px(12.0))
            .pb(px(12.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap(px(8.0))
                    .pl(px(14.0))
                    .pr(px(6.0))
                    .py(px(6.0))
                    .rounded(corner(16.0))
                    .bg(p.card)
                    .border_1()
                    .border_color(mix(p.border, p.primary, ring))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .py(px(4.0))
                            .child(Textarea::new(&self.threads.reply).appearance(false)),
                    )
                    .child(
                        div()
                            .id("thread-send")
                            .size(px(32.0))
                            .flex_none()
                            .rounded(corner(10.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(mix(p.muted, p.primary, ready))
                            .text_color(mix(p.muted_foreground, p.primary_foreground, ready))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, window, cx| this.send_reply(window, cx)))
                            .child(icon("send").size(px(16.0))),
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
                        .text_xs()
                        .text_color(if also { p.foreground } else { p.muted_foreground })
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
                        .child(format!("Also send to #{name}")),
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

    /// The channel's threads, behind the header's Threads button.
    pub(crate) fn threads_list_panel(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let listing = self.threads.listing.as_ref()?;
        let p = pal(cx);
        let hours = self.core.shared.read(|s| {
            s.instance(&listing.key)
                .and_then(|i| i.server(&listing.server))
                .map(|s| s.thread_archive_hours)
                .unwrap_or(0)
        });
        let archived = listing.archived;
        let tab = |id: &'static str, label: &'static str, on: bool, value: bool, cx: &mut Context<Self>| {
            div()
                .id(id)
                .px(px(12.0))
                .py(px(4.0))
                .rounded_full()
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .cursor_pointer()
                .when(on, |el| el.bg(alpha(p.primary, 0.14)).text_color(p.primary))
                .when(!on, |el| el.text_color(p.muted_foreground).hover(|s| s.bg(p.muted)))
                .on_click(cx.listener(move |this, _, _, cx| this.show_archived_threads(value, cx)))
                .child(label)
        };
        let header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.0))
            .pl(px(16.0))
            .pr(px(10.0))
            .h(px(56.0))
            .border_b_1()
            .border_color(p.border)
            .child(icon("messages-square").size(px(18.0)).text_color(p.primary))
            .child(div().flex_1().text_lg().font_weight(FontWeight::EXTRA_BOLD).child("Threads"))
            .child(icon_button("threads-close", "x", &p).on_click(cx.listener(|this, _, _, cx| {
                this.threads.listing = None;
                cx.notify();
            })));
        let tools = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .px(px(12.0))
            .pt(px(10.0))
            .pb(px(6.0))
            .child(Input::new(&self.threads.query).prefix(icon("search").size(px(14.0)).text_color(p.muted_foreground)))
            .when(hours > 0, |el| {
                el.child(div().flex().gap(px(4.0)).child(tab("threads-open", "Open", !archived, false, cx)).child(tab(
                    "threads-archived",
                    "Archived",
                    archived,
                    true,
                    cx,
                )))
            });

        let listing = self.threads.listing.as_ref()?;
        let (users, unread) = self
            .core
            .shared
            .read(|s| s.instance(&listing.key).map(|i| (i.users.clone(), i.thread_unread.clone())))
            .unwrap_or_default();
        let searched = !listing.query.is_empty();
        let mut body = div().id("threads-list").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().pb(px(12.0));
        if listing.loading && listing.items.is_empty() {
            body = body.child(crate::ui::search::skeleton(4, &p));
        } else if let Some(error) = &listing.error {
            body = body.child(crate::ui::search::empty("circle-alert", "Couldn't load threads", error, &p));
        } else if listing.items.is_empty() && listing.after.is_empty() {
            let (title, line) = match (searched, archived) {
                (true, _) => ("No threads found", "Try other words."),
                (false, true) => ("No archived threads", "Threads quiet for a while end up here."),
                (false, false) => ("No threads yet", "Hover a message and pick Reply in thread to start one."),
            };
            body = body.child(crate::ui::search::empty("messages-square", title, line, &p));
        } else {
            for (n, m) in listing.items.iter().enumerate() {
                let new = unread.get(&m.id).copied().unwrap_or(0);
                body = body.child(self.thread_list_row(m, n, &users, new, &p, cx));
            }
            if listing.loading {
                body = body.child(crate::ui::search::skeleton(1, &p));
            } else if !listing.after.is_empty() {
                body = body.child(
                    div().flex().justify_center().py(px(10.0)).child(
                        div()
                            .id("threads-more")
                            .px(px(16.0))
                            .py(px(6.0))
                            .rounded_full()
                            .bg(p.muted)
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(p.muted_foreground)
                            .cursor_pointer()
                            .hover(|s| s.bg(alpha(p.primary, 0.1)).text_color(p.primary))
                            .child(if searched && listing.items.is_empty() {
                                "Search older threads"
                            } else {
                                "Show more"
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.fetch_threads(true, cx))),
                    ),
                );
            }
        }
        Some(
            motion::slide_in(
                div()
                    .w(px(360.0))
                    .h_full()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .border_l_1()
                    .border_color(p.border)
                    .bg(p.background)
                    .child(header)
                    .child(tools)
                    .child(body),
                "threads-list-in",
                24.0,
            )
            .into_any_element(),
        )
    }

    fn thread_list_row(
        &self,
        m: &pb::Message,
        n: usize,
        users: &std::collections::HashMap<String, pb::User>,
        new: u32,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *p;
        let author = match &m.webhook {
            Some(w) => w.name.clone(),
            None => users.get(&m.author_id).map(crate::core::store::user_name).unwrap_or_else(|| "Someone".into()),
        };
        let t = m.thread.clone().unwrap_or_default();
        let last = ms_of(t.last_reply_at.as_ref());
        let now = crate::core::dms::now_ms();
        let first_line =
            m.content.lines().find(|l| !l.trim().is_empty()).unwrap_or("").chars().take(140).collect::<String>();
        let preview = if first_line.is_empty() {
            if m.attachments.is_empty() { "A message".to_owned() } else { "A file".to_owned() }
        } else {
            first_line
        };
        let open = m.clone();
        let word = if t.reply_count == 1 { "reply" } else { "replies" };
        motion::rise(
            div()
                .id(SharedString::from(format!("thread-row|{}", m.id)))
                .mx(px(8.0))
                .mb(px(6.0))
                .px(px(12.0))
                .py(px(10.0))
                .rounded(corner(14.0))
                .border_1()
                .border_color(gpui_kit::transparent_black())
                .bg(alpha(p.card, 0.6))
                .cursor_pointer()
                .hover(|s| s.bg(p.card).border_color(p.border))
                .on_click(cx.listener(move |this, _, window, cx| this.open_listed(open.clone(), window, cx)))
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(avatar(users.get(&m.author_id), 20.0, &p))
                        .child(div().min_w_0().truncate().text_sm().font_weight(FontWeight::EXTRA_BOLD).child(author))
                        .when(t.locked, |el| el.child(icon("lock").size(px(12.0)).text_color(p.muted_foreground))),
                )
                .child(div().text_sm().line_clamp(2).child(preview))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_xs()
                        .text_color(p.muted_foreground)
                        .child(
                            div()
                                .font_weight(FontWeight::BOLD)
                                .text_color(p.primary)
                                .child(format!("{} {word}", t.reply_count)),
                        )
                        .when(new > 0, |el| {
                            el.child(
                                div()
                                    .px(px(6.0))
                                    .rounded_full()
                                    .bg(p.primary)
                                    .text_color(p.primary_foreground)
                                    .font_weight(FontWeight::BOLD)
                                    .child(format!("{new} new")),
                            )
                        })
                        .child("·")
                        .child(if last > 0 && last <= now {
                            format!("Last reply {}", when(last))
                        } else {
                            String::new()
                        }),
                ),
            SharedString::from(format!("thread-row-in|{}", m.id)),
            Duration::from_millis(25 * n.min(10) as u64),
            8.0,
        )
        .into_any_element()
    }
}
