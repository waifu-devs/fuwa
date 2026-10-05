//! Pinned messages on screen, as the web app's `components/chat/Pins.tsx`:
//! the pin in a channel's header opens what's pinned there, the latest pin
//! first, to read, jump to or (for people who may) unpin. Pinned messages
//! carry a small pin. The list is read only when opened (`core::pins`).

use std::collections::HashSet;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, ScrollHandle,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::core::i18n::{Arg, t, t_with};
use crate::core::pins::{PinStatus, pins_key};
use crate::core::store::user_name;
use crate::pb;
use crate::ui::app::{FuwaApp, Target};
use crate::ui::motion;
use crate::ui::search::{empty, skeleton};
use crate::ui::text::{images_as_links, ms_of, when};
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::{avatar, icon, icon_button, pal};

/// The open list: a channel's pins.
pub struct PinsPanel {
    pub key: String,
    pub server: String,
    pub channel: String,
    scroll: ScrollHandle,
    /// Pins being taken off.
    unpinning: HashSet<String>,
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

    /// Opens the channel's pins, or closes them.
    pub(crate) fn toggle_pins(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pins.take().is_some() {
            cx.notify();
            return;
        }
        let Some(Target::Channel { key, server, channel }) = self.target() else { return };
        if self.search.panel.is_some() {
            self.close_search(window, cx);
        }
        self.close_thread(cx);
        self.threads.listing = None;
        self.pins = Some(PinsPanel {
            key: key.clone(),
            server: server.clone(),
            channel: channel.clone(),
            scroll: ScrollHandle::new(),
            unpinning: HashSet::new(),
        });
        self.load_pins(false, cx);
        cx.notify();
    }

    /// The pins go when you leave their channel.
    pub(crate) fn pins_after_move(&mut self) {
        let here = match self.target() {
            Some(Target::Channel { key, channel, .. }) => Some((key, channel)),
            _ => None,
        };
        if self.pins.as_ref().is_some_and(|p| here != Some((p.key.clone(), p.channel.clone()))) {
            self.pins = None;
        }
    }

    fn load_pins(&mut self, more: bool, cx: &mut Context<Self>) {
        let Some(panel) = &self.pins else { return };
        let core = self.core.clone();
        let (k, s, c) = (panel.key.clone(), panel.server.clone(), panel.channel.clone());
        self.run(cx, async move { core.load_pins(&k, &s, &c, "", more).await }, |_, _, cx| cx.notify());
    }

    /// Pins a message where you are, or unpins it, saying so.
    pub(crate) fn toggle_pin(&mut self, id: String, pinned: bool, in_thread: bool, cx: &mut Context<Self>) {
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
            move |this, result, cx| match result {
                Ok(()) if pinned => this.toast(
                    "pin",
                    t_with("chattools.pins.pinnedToast", &[("place", Arg::Str(&place))]),
                    String::new(),
                    None,
                    None,
                    cx,
                ),
                Ok(()) => this.toast("pin-off", t("chattools.pins.unpinnedToast"), String::new(), None, None, cx),
                Err(err) => this.toast("circle-alert", err.message, String::new(), None, None, cx),
            },
        );
    }

    fn unpin_listed(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(panel) = self.pins.as_mut() else { return };
        if !panel.unpinning.insert(id.clone()) {
            return;
        }
        let core = self.core.clone();
        let (k, s, c) = (panel.key.clone(), panel.server.clone(), panel.channel.clone());
        let done = id.clone();
        self.run(cx, async move { core.pin_message(&k, &s, &c, &id, false).await }, move |this, result, cx| {
            if let Some(panel) = this.pins.as_mut() {
                panel.unpinning.remove(&done);
            }
            if let Err(err) = result {
                this.toast("circle-alert", err.message, String::new(), None, None, cx);
            }
            cx.notify();
        });
    }

    /// Jumps to a pinned message: in its thread when it's only there.
    fn jump_to_pin(&mut self, message: pb::Message, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = self.pins.take() else { return };
        if !message.thread_id.is_empty() && !message.also_in_channel {
            self.open_thread(message.thread_id.clone(), window, cx);
            return;
        }
        self.jump_to_message(&panel.key, &panel.server, message, window, cx);
    }

    /// The open list, beside the messages.
    pub(crate) fn pins_panel(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let panel = self.pins.as_ref()?;
        let p = pal(cx);
        let (list, users, can_unpin) = self.core.shared.read(|s| {
            let i = s.instance(&panel.key)?;
            let guest =
                i.channel(&panel.server, &panel.channel).and_then(|c| c.shared.as_ref()).is_some_and(|s| !s.home);
            let can = !guest && i.access(&panel.server).has_in(&panel.channel, pb::Permission::ManageMessages);
            let list = i.pins.get(&pins_key(&panel.channel, "")).cloned();
            let users = list.as_ref().map(|l| {
                l.messages
                    .iter()
                    .map(|m| {
                        let user = i.users.get(&m.author_id).cloned();
                        let name = match &m.webhook {
                            Some(w) => w.name.clone(),
                            None => i.display_name(Some(&panel.server), &m.author_id),
                        };
                        (user, name)
                    })
                    .collect::<Vec<_>>()
            });
            Some((list, users.unwrap_or_default(), can))
        })?;
        let count = list.as_ref().filter(|l| !l.has_more && l.status == PinStatus::Ready).map(|l| l.messages.len());
        let header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(14.0))
            .h(px(56.0))
            .border_b_1()
            .border_color(p.border)
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
                this.pins = None;
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
        match &list {
            None => body = body.child(skeleton(3, &p)),
            Some(l) if l.messages.is_empty() && l.status == PinStatus::Loading => body = body.child(skeleton(3, &p)),
            Some(l) if l.messages.is_empty() && l.status == PinStatus::Failed => {
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
            Some(l) if l.messages.is_empty() => {
                body = body.child(empty("pin", &t("chattools.pins.empty"), &t("chattools.pins.emptyChannel"), &p))
            }
            Some(l) => {
                for (n, (m, (user, name))) in l.messages.iter().zip(users).enumerate() {
                    let busy = panel.unpinning.contains(&m.id);
                    body = body.child(self.pin_row(m, user, name, n, can_unpin, busy, &p, cx));
                }
                if l.has_more {
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
                    .w(px(400.0))
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

    #[allow(clippy::too_many_arguments)]
    fn pin_row(
        &self,
        m: &pb::Message,
        user: Option<pb::User>,
        name: String,
        n: usize,
        can_unpin: bool,
        busy: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *p;
        let id = m.id.clone();
        let user = user.or_else(|| {
            m.webhook.as_ref().map(|w| pb::User {
                id: w.webhook_id.clone(),
                username: w.name.clone(),
                display_name: w.name.clone(),
                avatar_url: w.avatar_url.clone(),
                ..Default::default()
            })
        });
        let name = if name.is_empty() { user.as_ref().map(user_name).unwrap_or_default() } else { name };
        let body: AnyElement = if m.content.trim().is_empty() {
            div().text_sm().italic().text_color(p.muted_foreground).child(t("chattools.pins.files")).into_any_element()
        } else {
            div()
                .text_sm()
                .line_clamp(4)
                .child(crate::ui::text::markdown(
                    SharedString::from(format!("pin-body|{id}")),
                    images_as_links(&m.content),
                ))
                .into_any_element()
        };
        let jump = m.clone();
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
                            .child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(p.muted_foreground)
                                    .child(when(ms_of(m.created_at.as_ref()))),
                            ),
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
                    .child(
                        pill(
                            SharedString::from(format!("pin-jump|{id}")),
                            "corner-down-right",
                            t("chattools.pins.jump"),
                            &p,
                        )
                        .on_click(cx.listener(move |this, _, window, cx| this.jump_to_pin(jump.clone(), window, cx))),
                    )
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
            motion::rise(row, SharedString::from(format!("pin-in|{}", m.id)), Duration::from_millis(30 * n as u64), 8.0)
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
