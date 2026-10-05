//! Pinned messages on screen, as the web app's `components/chat/Pins.tsx`:
//! the pin in a channel's header opens what's pinned there, the latest pin
//! first, to read, jump to or (for people who may) unpin. Pinned messages
//! carry a small pin. The list is read only when opened (`core::pins`).

use std::collections::HashSet;
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, ScrollHandle,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::core::i18n::{Arg, t, t_with};
use crate::core::pins::{PinStatus, pins_key};
use crate::core::store::user_name;
use crate::core::vault::ItemKind;
use crate::pb;
use crate::ui::app::{FuwaApp, Target};
use crate::ui::motion;
use crate::ui::search::{empty, skeleton};
use crate::ui::text::{images_as_links, ms_of, when};
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::{avatar, icon, icon_button, pal};

/// Where an open list's pins are.
#[derive(Clone, PartialEq, Eq)]
pub enum PinPlace {
    /// A channel's, or (with `thread` set) one of its threads'.
    Channel { server: String, channel: String, thread: String },
    /// A private conversation's.
    Dm { conversation: String },
}

/// The open list.
pub struct PinsPanel {
    pub key: String,
    pub place: PinPlace,
    scroll: ScrollHandle,
    /// Pins being taken off.
    unpinning: HashSet<String>,
}

impl PinsPanel {
    fn new(key: String, place: PinPlace) -> Self {
        Self { key, place, scroll: ScrollHandle::new(), unpinning: HashSet::new() }
    }

    /// Whether these are an open thread's pins (they sit over it).
    pub fn of_thread(&self, id: &str) -> bool {
        matches!(&self.place, PinPlace::Channel { thread, .. } if thread == id)
    }
}

/// Where a listed pin's Jump goes.
#[derive(Clone)]
enum Jump {
    Message(Box<pb::Message>),
    Dm(i64),
}

/// An open list as read: how far it's read (None before any read), its pins, and whether you may unpin.
struct Read {
    list: Option<(PinStatus, bool)>,
    rows: Vec<Listed>,
    can_unpin: bool,
}

/// A listed pin as drawn, whichever kind of list it's in.
struct Listed {
    id: String,
    user: Option<pb::User>,
    name: String,
    at: i64,
    body: Body,
    jump: Option<Jump>,
}

enum Body {
    Text(String),
    Files,
    Voice,
    NotOnDevice,
}

/// The small pin by a pinned message.
pub fn pin_mark(id: &str, p: &Palette) -> AnyElement {
    motion::rise(
        div()
            .flex_none()
            .flex()
            .items_center()
            .px(px(6.0))
            .py(px(1.0))
            .rounded_full()
            .bg(alpha(p.primary, 0.12))
            .text_color(p.primary)
            .child(icon("pin").size(px(11.0))),
        SharedString::from(format!("pin-mark|{id}")),
        Duration::ZERO,
        4.0,
    )
    .into_any_element()
}

impl FuwaApp {
    /// Whether this channel's header shows the pin: the instance keeps pins,
    /// and the channel isn't a secure one, whose messages the instance can't read.
    pub(crate) fn pins_here(&self, key: &str, channel: &pb::Channel) -> bool {
        channel.r#type != pb::ChannelType::Secure as i32
            && self.core.shared.read(|s| s.instance(key).is_some_and(|i| i.has("pins")))
    }

    /// Opens the channel's (or the conversation's) pins, or closes them.
    pub(crate) fn toggle_pins(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (key, place) = match self.target() {
            Some(Target::Channel { key, server, channel }) => {
                (key, PinPlace::Channel { server, channel, thread: String::new() })
            }
            Some(Target::Dm { key, conversation }) => (key, PinPlace::Dm { conversation }),
            _ => return,
        };
        if self.pins.take().is_some_and(|p| p.place == place) {
            cx.notify();
            return;
        }
        if self.search.panel.is_some() {
            self.close_search(window, cx);
        }
        self.close_thread(cx);
        self.threads.listing = None;
        self.pins = Some(PinsPanel::new(key, place));
        self.load_pins(false, cx);
        cx.notify();
    }

    /// Opens the open thread's pins over it.
    pub(crate) fn open_thread_pins(&mut self, cx: &mut Context<Self>) {
        let Some(open) = self.threads.open.clone() else { return };
        self.pins = Some(PinsPanel::new(
            open.key,
            PinPlace::Channel { server: open.server, channel: open.channel, thread: open.id },
        ));
        self.load_pins(false, cx);
        cx.notify();
    }

    /// The pins go when you leave where they are.
    pub(crate) fn pins_after_move(&mut self) {
        let keep = self.pins.as_ref().is_some_and(|p| match (&p.place, self.target()) {
            (PinPlace::Channel { channel, .. }, Some(Target::Channel { key, channel: here, .. })) => {
                key == p.key && here == *channel
            }
            (PinPlace::Dm { conversation }, Some(Target::Dm { key, conversation: here })) => {
                key == p.key && here == *conversation
            }
            _ => false,
        });
        if !keep {
            self.pins = None;
        }
    }

    /// A conversation's pins are read when it opens, for the marks on its messages.
    pub(crate) fn load_dm_pins_here(&mut self, cx: &mut Context<Self>) {
        let Some(Target::Dm { key, conversation }) = self.target() else { return };
        if !self.core.shared.read(|s| s.instance(&key).is_some_and(|i| i.has("pins"))) {
            return;
        }
        let core = self.core.clone();
        self.run(cx, async move { core.load_dm_pins(&key, &conversation, false).await }, |this, _, cx| {
            this.sync_list(cx);
            cx.notify();
        });
    }

    fn load_pins(&mut self, more: bool, cx: &mut Context<Self>) {
        let Some(panel) = &self.pins else { return };
        let core = self.core.clone();
        let key = panel.key.clone();
        match panel.place.clone() {
            PinPlace::Channel { server, channel, thread } => {
                self.run(cx, async move { core.load_pins(&key, &server, &channel, &thread, more).await }, |_, _, cx| {
                    cx.notify()
                })
            }
            PinPlace::Dm { conversation } => {
                self.run(cx, async move { core.load_dm_pins(&key, &conversation, more).await }, |this, _, cx| {
                    this.sync_list(cx);
                    cx.notify();
                })
            }
        }
    }

    /// Pins a message where you are, or unpins it, saying so.
    pub(crate) fn toggle_pin(&mut self, id: String, pinned: bool, in_thread: bool, cx: &mut Context<Self>) {
        if let Some(Target::Dm { key, conversation }) = self.target() {
            let Ok(seq) = id.parse::<i64>() else { return };
            let core = self.core.clone();
            self.run(
                cx,
                async move { core.pin_dm(&key, &conversation, seq, pinned).await },
                move |this, result, cx| {
                    this.pin_toast(result, pinned, t("chattools.pins.thisConversation"), cx);
                    this.sync_list(cx);
                },
            );
            return;
        }
        let Some(Target::Channel { key, server, channel }) = self.target() else { return };
        let place = if in_thread {
            t("chat.threads.thread")
        } else {
            let name = self
                .core
                .shared
                .read(|s| s.instance(&key).and_then(|i| i.channel(&server, &channel)).map(|c| c.name.clone()));
            format!("#{}", name.unwrap_or_default())
        };
        let core = self.core.clone();
        self.run(
            cx,
            async move { core.pin_message(&key, &server, &channel, &id, pinned).await },
            move |this, result, cx| this.pin_toast(result, pinned, place, cx),
        );
    }

    fn pin_toast(
        &mut self,
        result: Result<(), crate::core::api::Problem>,
        pinned: bool,
        place: String,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(()) if pinned => self.toast(
                "pin",
                t_with("chattools.pins.pinnedToast", &[("place", Arg::Str(&place))]),
                String::new(),
                None,
                None,
                cx,
            ),
            Ok(()) => self.toast("pin-off", t("chattools.pins.unpinnedToast"), String::new(), None, None, cx),
            Err(err) => self.toast("circle-alert", err.message, String::new(), None, None, cx),
        }
    }

    fn unpin_listed(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(panel) = self.pins.as_mut() else { return };
        if !panel.unpinning.insert(id.clone()) {
            return;
        }
        let core = self.core.clone();
        let key = panel.key.clone();
        let place = panel.place.clone();
        let done = id.clone();
        self.run(
            cx,
            async move {
                match place {
                    PinPlace::Channel { server, channel, .. } => {
                        core.pin_message(&key, &server, &channel, &id, false).await
                    }
                    PinPlace::Dm { conversation } => match id.parse::<i64>() {
                        Ok(seq) => core.pin_dm(&key, &conversation, seq, false).await,
                        Err(_) => Ok(()),
                    },
                }
            },
            move |this, result, cx| {
                if let Some(panel) = this.pins.as_mut() {
                    panel.unpinning.remove(&done);
                }
                if let Err(err) = result {
                    this.toast("circle-alert", err.message, String::new(), None, None, cx);
                }
                this.sync_list(cx);
                cx.notify();
            },
        );
    }

    /// Jumps to a pinned message: in the open thread when the list is its, in
    /// its thread when it's only there, or else in the channel or conversation.
    fn jump_to_pin(&mut self, jump: Jump, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = self.pins.take() else { return };
        match jump {
            Jump::Message(message) => {
                if let PinPlace::Channel { thread, .. } = &panel.place
                    && !thread.is_empty()
                {
                    self.search.jumped = Some((message.id.clone(), Instant::now()));
                    if let Some(ix) = self.threads.rows.iter().position(|r| r.id_str() == message.id) {
                        self.threads.scroll.scroll_to_item(ix);
                    }
                    cx.notify();
                    return;
                }
                if !message.thread_id.is_empty() && !message.also_in_channel {
                    self.open_thread(message.thread_id.clone(), window, cx);
                    return;
                }
                if let PinPlace::Channel { server, .. } = &panel.place {
                    self.jump_to_message(&panel.key, server, *message, window, cx);
                }
            }
            Jump::Dm(seq) => {
                let id = seq.to_string();
                self.search.jumped = Some((id.clone(), Instant::now()));
                self.sync_list(cx);
                if let Some(ix) = self.rows.iter().position(|r| r.id_str() == id) {
                    self.scroller.update(cx, |s, cx| s.scroll_to_item(ix, cx));
                }
                cx.notify();
            }
        }
    }

    /// What the open list shows, as read now; None when there's nowhere to read it.
    fn listed(&self, panel: &PinsPanel) -> Option<Read> {
        self.core.shared.read(|s| {
            let i = s.instance(&panel.key)?;
            match &panel.place {
                PinPlace::Channel { server, channel, thread } => {
                    let guest = i.channel(server, channel).and_then(|c| c.shared.as_ref()).is_some_and(|s| !s.home);
                    let can = !guest && i.access(server).has_in(channel, pb::Permission::ManageMessages);
                    let list = i.pins.get(&pins_key(channel, thread));
                    let rows = list
                        .map(|l| {
                            l.messages
                                .iter()
                                .map(|m| {
                                    let user = i.users.get(&m.author_id).cloned().or_else(|| {
                                        m.webhook.as_ref().map(|w| pb::User {
                                            id: w.webhook_id.clone(),
                                            username: w.name.clone(),
                                            display_name: w.name.clone(),
                                            avatar_url: w.avatar_url.clone(),
                                            ..Default::default()
                                        })
                                    });
                                    let name = match &m.webhook {
                                        Some(w) => w.name.clone(),
                                        None => i.display_name(Some(server), &m.author_id),
                                    };
                                    let body = if m.content.trim().is_empty() {
                                        Body::Files
                                    } else {
                                        Body::Text(m.content.clone())
                                    };
                                    Listed {
                                        id: m.id.clone(),
                                        user,
                                        name,
                                        at: ms_of(m.created_at.as_ref()),
                                        body,
                                        jump: Some(Jump::Message(Box::new(m.clone()))),
                                    }
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    Some(Read { list: list.map(|l| (l.status, l.has_more)), rows, can_unpin: can })
                }
                PinPlace::Dm { conversation } => {
                    let list = i.dms.pins.get(conversation);
                    let items = i.dms.items.get(conversation).map(Vec::as_slice).unwrap_or_default();
                    let users = i.dms.conversations.iter().find(|c| &c.id == conversation).map(|c| c.users.as_slice());
                    let rows = list
                        .map(|l| {
                            l.pins
                                .iter()
                                .map(|pin| {
                                    // Only what this device opened says what a pin is.
                                    let item = items
                                        .iter()
                                        .find(|it| it.seq == pin.sequence && it.kind == ItemKind::Text && !it.deleted);
                                    let user = item.and_then(|it| {
                                        users
                                            .and_then(|u| u.iter().find(|u| u.id == it.sender_id))
                                            .or_else(|| i.users.get(&it.sender_id))
                                            .cloned()
                                    });
                                    let body = match item {
                                        None => Body::NotOnDevice,
                                        Some(it) if it.voice.is_some() => Body::Voice,
                                        Some(it) if it.content.trim().is_empty() => Body::Files,
                                        Some(it) => Body::Text(it.content.clone()),
                                    };
                                    Listed {
                                        id: pin.sequence.to_string(),
                                        name: user.as_ref().map(user_name).unwrap_or_default(),
                                        user,
                                        at: item.map(|it| it.at).unwrap_or_else(|| ms_of(pin.pinned_at.as_ref())),
                                        body,
                                        jump: item.map(|_| Jump::Dm(pin.sequence)),
                                    }
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    // Either person in a conversation may unpin.
                    Some(Read { list: list.map(|l| (l.status, l.has_more)), rows, can_unpin: true })
                }
            }
        })
    }

    /// The open list, beside the messages.
    pub(crate) fn pins_panel(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let panel = self.pins.as_ref()?;
        let p = pal(cx);
        let Read { list, rows, can_unpin } = self.listed(panel)?;
        let in_thread = matches!(&panel.place, PinPlace::Channel { thread, .. } if !thread.is_empty());
        let in_dm = matches!(panel.place, PinPlace::Dm { .. });
        let status = list.map(|(s, _)| s);
        let has_more = list.is_some_and(|(_, m)| m);
        let count = (status == Some(PinStatus::Ready) && !has_more).then_some(rows.len());
        let header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(14.0))
            .h(px(56.0))
            .border_b_1()
            .border_color(p.border)
            .when(in_thread, |el| {
                el.child(
                    icon_button("pins-back", "arrow-left", &p)
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(t("common.back")).build(window, cx)
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.pins = None;
                            cx.notify();
                        })),
                )
            })
            .child(icon("pin").size(px(16.0)).text_color(p.primary))
            .child(div().text_lg().font_weight(FontWeight::EXTRA_BOLD).child(t("chattools.pins.title")))
            .when_some(count.filter(|n| *n > 0), |el, n| {
                el.child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.muted_foreground)
                        .child(t_with("chattools.pins.count", &[("count", Arg::Num(n as i64))])),
                )
            })
            .child(div().flex_1())
            .child(icon_button("pins-close", "x", &p).on_click(cx.listener(|this, _, _, cx| {
                // A thread's pins close with it.
                if this
                    .pins
                    .take()
                    .is_some_and(|p| matches!(&p.place, PinPlace::Channel { thread, .. } if !thread.is_empty()))
                {
                    this.close_thread(cx);
                }
                cx.notify();
            })));
        let mut body = div()
            .id("pins-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&panel.scroll)
            .flex()
            .flex_col()
            .py(px(8.0));
        let empty_hint = if in_dm { t("chattools.pins.emptyDm") } else { t("chattools.pins.emptyChannel") };
        match status {
            None | Some(PinStatus::Loading) if rows.is_empty() => body = body.child(skeleton(3, &p)),
            Some(PinStatus::Failed) if rows.is_empty() => {
                body = body.child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(8.0))
                        .px(px(24.0))
                        .py(px(32.0))
                        .text_sm()
                        .text_center()
                        .text_color(p.muted_foreground)
                        .child(t("chattools.pins.failed"))
                        .child(
                            pill("pins-retry", "rotate-cw", t("chattools.pins.retry"), &p)
                                .on_click(cx.listener(|this, _, _, cx| this.load_pins(false, cx))),
                        ),
                )
            }
            _ if rows.is_empty() => body = body.child(empty("pin", &t("chattools.pins.empty"), &empty_hint, &p)),
            _ => {
                for (n, row) in rows.into_iter().enumerate() {
                    let busy = panel.unpinning.contains(&row.id);
                    body = body.child(self.pin_row(row, n, can_unpin, busy, &p, cx));
                }
                if has_more {
                    body = body.child(
                        div().flex().justify_center().py(px(8.0)).child(
                            pill("pins-more", "chevron-down", t("chattools.pins.more"), &p)
                                .on_click(cx.listener(|this, _, _, cx| this.load_pins(true, cx))),
                        ),
                    );
                }
            }
        }
        Some(
            motion::slide_in(
                div()
                    .w(px(if in_thread { 420.0 } else { 400.0 }))
                    .h_full()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .border_l_1()
                    .border_color(p.border)
                    .bg(p.background)
                    .child(header)
                    .child(body),
                "pins-panel-in",
                24.0,
            )
            .into_any_element(),
        )
    }

    fn pin_row(
        &self,
        row: Listed,
        n: usize,
        can_unpin: bool,
        busy: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *p;
        let Listed { id, user, name, at, body, jump } = row;
        let name = if name.is_empty() { user.as_ref().map(user_name).unwrap_or_default() } else { name };
        let quiet = |text: String| div().text_sm().italic().text_color(p.muted_foreground).child(text);
        let body: AnyElement = match body {
            Body::Text(text) => div()
                .text_sm()
                .line_clamp(4)
                .child(crate::ui::text::markdown(SharedString::from(format!("pin-body|{id}")), images_as_links(&text)))
                .into_any_element(),
            Body::Files => quiet(t("chattools.pins.files")).into_any_element(),
            Body::Voice => quiet(t("chattools.pins.voice")).into_any_element(),
            Body::NotOnDevice => quiet(t("chattools.pins.notOnDevice"))
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(icon("lock-keyhole").size(px(13.0)))
                .into_any_element(),
        };
        let row = div()
            .id(SharedString::from(format!("pin|{id}")))
            .group("pin")
            .relative()
            .flex()
            .gap(px(12.0))
            .mx(px(8.0))
            .mb(px(4.0))
            .px(px(10.0))
            .py(px(8.0))
            .rounded(corner(14.0))
            .hover(|s| s.bg(alpha(p.muted, 0.6)))
            .when(busy, |el| el.opacity(0.5))
            .child(avatar(user.as_ref(), 32.0, &p))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(8.0))
                            .child(div().truncate().text_sm().font_weight(FontWeight::BOLD).child(name))
                            .child(div().flex_none().text_xs().text_color(p.muted_foreground).child(when(at))),
                    )
                    .child(body),
            )
            .child(
                div()
                    .absolute()
                    .top(px(6.0))
                    .right(px(6.0))
                    .flex()
                    .gap(px(4.0))
                    .invisible()
                    .group_hover("pin", |s| s.visible())
                    .when_some(jump, |el, jump| {
                        el.child(
                            pill(
                                SharedString::from(format!("pin-jump|{id}")),
                                "corner-down-right",
                                t("chattools.pins.jump"),
                                &p,
                            )
                            .on_click(
                                cx.listener(move |this, _, window, cx| this.jump_to_pin(jump.clone(), window, cx)),
                            ),
                        )
                    })
                    .when(can_unpin && !busy, |el| {
                        let id = id.clone();
                        el.child(
                            icon_button(SharedString::from(format!("pin-off|{id}")), "pin-off", &p)
                                .tooltip({
                                    let label = t("chattools.pins.unpin");
                                    move |window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(label.clone()).build(window, cx)
                                    }
                                })
                                .on_click(cx.listener(move |this, _, _, cx| this.unpin_listed(id.clone(), cx))),
                        )
                    }),
            );
        if n < 8 {
            motion::rise(row, SharedString::from(format!("pin-in|{id}")), Duration::from_millis(30 * n as u64), 8.0)
                .into_any_element()
        } else {
            row.into_any_element()
        }
    }
}

/// A small rounded button with an icon and a few words.
fn pill(
    id: impl Into<gpui_kit::ElementId>,
    glyph: &'static str,
    label: String,
    p: &Palette,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let p = *p;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(4.0))
        .px(px(10.0))
        .py(px(3.0))
        .rounded_full()
        .border_1()
        .border_color(p.border)
        .bg(p.card)
        .text_xs()
        .font_weight(FontWeight::BOLD)
        .text_color(p.foreground)
        .cursor_pointer()
        .hover(move |s| s.border_color(p.primary).text_color(p.primary))
        .child(icon(glyph).size(px(12.0)))
        .child(label)
}
