//! A secure channel: a server's channel whose messages are end-to-end
//! encrypted between the devices of the people who can see it. It reads and
//! writes through this device's encryption (`core/dms.rs`), like a direct
//! message, and never through the server's messages. The web's
//! `chat/SecureChannelView.tsx`.

use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, Div, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::core::dms::{DmMember, DmState, SECURE_BROKEN};
use crate::core::store::InstanceState;
use crate::core::vault::{Item, ItemKind};
use crate::pb;
use crate::ui::app::{Dialog, FuwaApp};
use crate::ui::chat::Blocked;
use crate::ui::motion;
use crate::ui::overlay::scrim;
use crate::ui::server_settings::roles::switch;
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::{avatar, card, error_line, icon, icon_button, pal, primary_button};

/// What a secure channel can't do, said once: the server can't read it, so nothing that needs to can work.
const CANT: [(&str, &str); 4] = [
    ("shield-off", "AutoMod can't check messages here"),
    ("search-x", "Search can't find them"),
    ("bot-off", "Bots, agents and webhooks can't post or read"),
    ("image-off", "Links stay links: no previews or inline pictures"),
];

const LATER_OFF: &str = "People added later only see what's sent after they join";
const LATER_ON: &str =
    "People added later get recent messages from members' devices, each checked against its sender's signature";

/// Starting a channel's encryption over: asked once, then running.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Reset {
    #[default]
    Idle,
    Ask,
    Busy,
}

/// What the name box suggests for a new channel of this kind.
pub fn name_hint(kind: pb::ChannelType) -> &'static str {
    match kind {
        pb::ChannelType::Category => "Cozy corner",
        pb::ChannelType::Voice => "Lounge",
        _ => "new-channel",
    }
}

/// What the top of a secure channel says about it.
pub fn beginning(shares_history: bool) -> String {
    format!(
        "This is a secure channel. Messages here are end-to-end encrypted: only the people in this channel can read \
         them, on their own devices. Not this fuwa server, and not whoever runs it. That means AutoMod, search, link \
         previews, bots and agents don't work here. {}",
        if shares_history {
            "People added later get recent messages, passed on by members' devices."
        } else {
            "People who join later only see messages sent after they join."
        }
    )
}

/// The top of a secure channel: a shield with a lock that pops into place, and how it's kept private.
pub fn start(title: String, body: String, p: &Palette) -> Div {
    let green = p.success;
    div()
        .px(px(20.0))
        .pt(px(32.0))
        .pb(px(16.0))
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .relative()
                .size(px(64.0))
                .rounded(corner(18.0))
                .flex()
                .items_center()
                .justify_center()
                .bg(alpha(green, 0.15))
                .text_color(green)
                .child(icon("shield-check").size(px(32.0)))
                .child(
                    div().absolute().right(px(-6.0)).bottom(px(-6.0)).child(motion::rise(
                        div()
                            .size(px(28.0))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(green)
                            .border_4()
                            .border_color(p.chat_surface)
                            .text_color(gpui_kit::white())
                            .child(icon("lock-keyhole").size(px(13.0))),
                        "secure-start-lock",
                        Duration::from_millis(450),
                        10.0,
                    )),
                ),
        )
        .child(div().text_2xl().font_weight(FontWeight::EXTRA_BOLD).child(title))
        .child(
            div()
                .max_w(px(620.0))
                .flex()
                .items_start()
                .gap(px(8.0))
                .px(px(12.0))
                .py(px(10.0))
                .rounded(corner(16.0))
                .bg(alpha(green, 0.1))
                .text_sm()
                .child(icon("lock-keyhole").size(px(16.0)).flex_none().mt(px(2.0)).text_color(green))
                .child(div().flex_1().min_w_0().child(body)),
        )
}

/// What changed about the channel's devices, in words, from the commit itself (not from the server).
pub fn channel_line(item: &Item, name_of: &dyn Fn(&str) -> String, me: &str, shared: bool) -> (&'static str, String) {
    let name = |id: &str| if id == me { "you".to_owned() } else { name_of(id) };
    let whose = |id: &str| if id == me { "your".to_owned() } else { format!("{}'s", name_of(id)) };
    let by = capital(&name(&item.sender_id));
    match item.kind {
        ItemKind::Joined if shared => (
            "shield-check",
            "This device joined the channel. The messages above were passed on by a member's device.".into(),
        ),
        ItemKind::Joined => {
            ("shield-check", "This device joined the channel. Messages from before it can't be read here.".into())
        }
        ItemKind::Unreadable => {
            ("lock-keyhole", format!("A message from {} couldn't be opened on this device.", name(&item.sender_id)))
        }
        ItemKind::Setting if item.content == "on" => (
            "messages-square",
            format!(
                "{by} turned on sharing earlier messages: people added from now on get recent history, passed on by members' devices."
            ),
        ),
        ItemKind::Setting => (
            "messages-square",
            format!(
                "{by} turned off sharing earlier messages: people added from now on only see what's sent after they join."
            ),
        ),
        ItemKind::Reset => (
            "rotate-ccw-key",
            format!(
                "{by} started this channel's encryption over. What came before stays on the devices that already read it."
            ),
        ),
        _ => {
            let devices = |list: &[crate::core::vault::DeviceRef]| -> Vec<String> {
                let mut counts: BTreeMap<usize, (String, usize)> = BTreeMap::new();
                let mut order: Vec<&str> = Vec::new();
                for d in list {
                    match order.iter().position(|u| *u == d.user_id) {
                        Some(n) => counts.get_mut(&n).expect("counted").1 += 1,
                        None => {
                            counts.insert(order.len(), (d.user_id.clone(), 1));
                            order.push(&d.user_id);
                        }
                    }
                }
                counts
                    .into_values()
                    .map(|(user, n)| {
                        format!("{} {}", whose(&user), if n == 1 { "device".into() } else { format!("{n} devices") })
                    })
                    .collect()
            };
            let (added, removed) = (devices(&item.added), devices(&item.removed));
            let alone = item.added.len() == 1 && item.added[0].user_id == item.sender_id && item.removed.is_empty();
            let text = if item.seq == 1 {
                if added.is_empty() {
                    format!("{by} started this secure channel.")
                } else {
                    format!("{by} started this secure channel and added {}.", list(&added))
                }
            } else if alone {
                format!("{by} came in on a new device.")
            } else {
                let parts: Vec<String> = [
                    (!added.is_empty()).then(|| format!("added {}", list(&added))),
                    (!removed.is_empty()).then(|| format!("removed {}", list(&removed))),
                ]
                .into_iter()
                .flatten()
                .collect();
                if parts.is_empty() {
                    format!("{by} refreshed the channel's keys.")
                } else {
                    format!("{by} {}.", parts.join(", and "))
                }
            };
            ("laptop", text)
        }
    }
}

fn capital(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

fn list(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The people whose safety number you checked in a direct message, where
/// every device of theirs in this channel was in it.
pub fn verified_people(dms: &DmState, me: &str, members: &[DmMember]) -> HashSet<String> {
    let mut out = HashSet::new();
    let people: HashSet<&str> = members.iter().map(|m| m.user_id.as_str()).collect();
    for user in people {
        if user == me {
            continue;
        }
        let Some(c) = dms
            .conversations
            .iter()
            .find(|c| c.users.iter().any(|u| u.id == user) && c.users.iter().any(|u| u.id == me))
        else {
            continue;
        };
        let safety = dms.safety.get(&c.id).filter(|s| !s.is_empty());
        if safety.is_none() || dms.verified.get(&c.id) != safety {
            continue;
        }
        let checked: HashSet<&[u8]> = dms
            .members
            .get(&c.id)
            .into_iter()
            .flatten()
            .filter(|m| m.user_id == user)
            .map(|m| m.signature_key.as_slice())
            .collect();
        if members.iter().filter(|m| m.user_id == user).all(|m| checked.contains(m.signature_key.as_slice())) {
            out.insert(user.to_owned());
        }
    }
    out
}

/// Everyone who can read a secure channel, with how many devices each, by name.
fn people(i: &InstanceState, server: &str, members: &[DmMember]) -> Vec<(String, usize)> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for m in members {
        *counts.entry(m.user_id.clone()).or_default() += 1;
    }
    let mut out: Vec<(String, usize)> = counts.into_iter().collect();
    out.sort_by_cached_key(|(id, _)| i.display_name(Some(server), id).to_lowercase());
    out
}

impl FuwaApp {
    /// A secure channel's header: its shield, name and topic, and the pill that says who can read it.
    pub(crate) fn secure_header(
        &mut self,
        key: &str,
        server: &str,
        channel: &pb::Channel,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let green = p.success;
        let dialog = Dialog::Secure { key: key.to_owned(), server: server.to_owned(), channel: channel.id.clone() };
        div()
            .h(px(56.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.0))
            .px(px(20.0))
            .border_b_1()
            .border_color(p.border)
            .child(motion::slide_in(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(icon("shield-check").size(px(20.0)).text_color(green))
                    .child(div().font_weight(FontWeight::EXTRA_BOLD).child(channel.name.clone())),
                SharedString::from(format!("secure-title-{}", channel.id)),
                10.0,
            ))
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
            .child(motion::rise(
                div()
                    .id("secure-pill")
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .h(px(28.0))
                    .px(px(10.0))
                    .rounded_full()
                    .bg(alpha(green, 0.13))
                    .text_color(green)
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .hover(move |s| s.bg(alpha(green, 0.22)))
                    .active(|s| s.top(px(1.0)))
                    .tooltip(|window, cx| {
                        crate::ui::overlay::Tip::new("See who can read this channel").build(window, cx)
                    })
                    .on_click(cx.listener(move |this, _, window, cx| this.open_dialog(dialog.clone(), window, cx)))
                    .child(icon("lock-keyhole").size(px(14.0)))
                    .child("End-to-end encrypted"),
                "secure-pill-in",
                Duration::from_millis(120),
                6.0,
            ))
    }

    /// Why you can't write in a secure channel right now, if you can't: on top
    /// of a channel's usual reasons, this device's encryption getting ready,
    /// or the channel's being broken (which someone who manages it can fix).
    pub(crate) fn secure_blocked(
        &self,
        key: &str,
        server: &str,
        channel_id: &str,
        usual: Option<Blocked>,
    ) -> Option<Blocked> {
        let (status, problem, joining, broken, can_reset, can_send) = self.core.shared.read(|s| {
            let i = s.instance(key)?;
            let access = i.access(server);
            Some((
                i.dms.status,
                i.dms.problem.clone(),
                i.dms.joining.contains(channel_id),
                i.dms.blocked.get(channel_id).is_some_and(|b| b == SECURE_BROKEN),
                access.has_in(channel_id, pb::Permission::ManageChannels),
                access.has_in(channel_id, pb::Permission::SendMessages),
            ))
        })?;
        let text = |t: &str| Some(Blocked { text: t.to_owned(), action: None });
        if status == crate::core::dms::DmStatus::Failed {
            return text(problem.as_deref().unwrap_or("Encrypted messages aren't available here."));
        }
        if !status.is_ready() {
            return text("Encrypted messages are still getting ready on this device…");
        }
        if broken {
            let dialog =
                Dialog::Secure { key: key.to_owned(), server: server.to_owned(), channel: channel_id.to_owned() };
            return Some(Blocked {
                text: SECURE_BROKEN.to_owned(),
                action: can_reset.then_some(("Start encryption over", dialog)),
            });
        }
        if joining {
            return text("Unlocking the channel on this device…");
        }
        match usual {
            Some(_) if !can_send => text("You don't have permission to send messages in this channel."),
            other => other,
        }
    }

    /// How a secure channel is kept private: what the server can't do, and
    /// every person (and how many devices) that can read it. Someone who
    /// manages the channel can turn history sharing on or off, or start its
    /// encryption over.
    pub(crate) fn render_secure(
        &mut self,
        key: &str,
        server: &str,
        channel_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = pal(cx);
        let green = p.success;
        let me = self.core.shared.read(|s| s.instance(key).and_then(|i| i.me.as_ref().map(|m| m.id.clone())));
        let me = me.unwrap_or_default();
        let (name, rows, verified, shares, can_reset) = self.core.shared.read(|s| {
            let Some(i) = s.instance(key) else { return Default::default() };
            let members = i.dms.members.get(channel_id).cloned().unwrap_or_default();
            let rows: Vec<(String, usize, String, Option<pb::User>)> = people(i, server, &members)
                .into_iter()
                .map(|(id, n)| {
                    let shown = if id == me { "You".to_owned() } else { i.display_name(Some(server), &id) };
                    let user = i.users.get(&id).cloned();
                    (id, n, shown, user)
                })
                .collect();
            (
                i.channel(server, channel_id).map(|c| c.name.clone()).unwrap_or_default(),
                rows,
                verified_people(&i.dms, &me, &members),
                i.dms.secure_history.get(channel_id).copied().unwrap_or(false),
                i.access(server).has_in(channel_id, pb::Permission::ManageChannels),
            )
        });
        let lines = CANT.iter().copied().chain([("user-plus", if shares { LATER_ON } else { LATER_OFF })]);
        let cant = div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .p(px(12.0))
            .rounded(corner(16.0))
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.muted, 0.4));
        let cant = cant.children(lines.enumerate().map(|(n, (glyph, text))| {
            motion::rise(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .text_sm()
                    .text_color(p.muted_foreground)
                    .child(icon(glyph).size(px(16.0)).flex_none())
                    .child(div().flex_1().min_w_0().child(text)),
                SharedString::from(format!("secure-cant-{n}")),
                Duration::from_millis(100 + 40 * n as u64),
                6.0,
            )
        }));
        let count = rows.len();
        let mut list = div().flex().flex_col();
        for (n, (id, devices, shown, user)) in rows.into_iter().enumerate() {
            let ok = verified.contains(&id);
            let hover = alpha(p.muted, 0.6);
            list = list.child(motion::rise(
                div()
                    .id(SharedString::from(format!("secure-person-{id}")))
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .px(px(8.0))
                    .py(px(6.0))
                    .rounded(corner(12.0))
                    .hover(move |s| s.bg(hover))
                    .child(avatar(user.as_ref(), 32.0, &p))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .font_weight(FontWeight::BOLD)
                                    .child(div().min_w_0().truncate().child(shown))
                                    .when(ok, |el| el.child(icon("badge-check").size(px(16.0)).text_color(green))),
                            )
                            .child(div().text_xs().text_color(p.muted_foreground).child(format!(
                                "{}{}",
                                if devices == 1 { "1 device".to_owned() } else { format!("{devices} devices") },
                                if ok { " · verified in your direct messages" } else { "" }
                            ))),
                    ),
                SharedString::from(format!("secure-person-in-{n}")),
                Duration::from_millis(30 * n.min(10) as u64),
                6.0,
            ));
        }
        let (k, s, c) = (key.to_owned(), server.to_owned(), channel_id.to_owned());
        let history = can_reset.then(|| {
            let (k, s, c) = (k.clone(), s.clone(), c.clone());
            div()
                .flex()
                .items_start()
                .gap(px(12.0))
                .px(px(12.0))
                .py(px(10.0))
                .rounded(corner(16.0))
                .border_1()
                .border_color(p.border)
                .child(icon("messages-square").size(px(16.0)).flex_none().mt(px(2.0)).text_color(p.muted_foreground))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(div().text_sm().font_weight(FontWeight::BOLD).child("Share earlier messages with people added later"))
                        .child(div().text_xs().text_color(p.muted_foreground).child(
                            "The device that adds someone passes on recent messages, still end-to-end encrypted. Each one is \
                             checked against the signature of the device that sent it, so nobody can change or make one up. \
                             Turning it on doesn't send anything to people already here.",
                        )),
                )
                .child(switch("secure-history".into(), shares, self.secure_saving, cx, move |this, on, cx| {
                    this.set_secure_history(k.clone(), s.clone(), c.clone(), on, cx)
                }))
        });
        let reset = can_reset.then(|| {
            let asking = self.secure_reset == Reset::Ask;
            let busy = self.secure_reset == Reset::Busy;
            let color = if asking { p.destructive } else { p.primary };
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .px(px(12.0))
                .py(px(10.0))
                .rounded(corner(16.0))
                .border_1()
                .border_dashed()
                .border_color(p.border)
                .child(div().flex_1().min_w_0().text_xs().text_color(p.muted_foreground).child(
                    "If the channel's encryption stops working for everyone, start it over. Messages already read stay on \
                     the devices that read them.",
                ))
                .child(
                    div()
                        .id("secure-reset")
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .h(px(32.0))
                        .px(px(12.0))
                        .rounded(corner(12.0))
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(color)
                        .when(asking, |el| el.bg(alpha(color, 0.12)))
                        .hover(move |st| st.bg(alpha(color, 0.18)))
                        .cursor_pointer()
                        .active(|st| st.top(px(1.0)))
                        .when(busy, |el| el.opacity(0.7))
                        .on_click(cx.listener(move |this, _, _, cx| this.reset_secure(k.clone(), s.clone(), c.clone(), cx)))
                        .child(icon(if busy { "loader-circle" } else { "rotate-ccw-key" }).size(px(14.0)))
                        .child(motion::slide_in(
                            div().child(if asking { "Start over for everyone?" } else { "Start encryption over" }),
                            SharedString::from(format!("secure-reset-{asking}")),
                            4.0,
                        )),
                )
        });
        let panel = card(&p)
            .w(px(500.0))
            .p(px(24.0))
            .flex()
            .flex_col()
            .gap(px(14.0))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(14.0))
                    .child(motion::rise(
                        div()
                            .size(px(48.0))
                            .flex_none()
                            .rounded(corner(16.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(alpha(green, 0.15))
                            .text_color(green)
                            .child(icon("lock-keyhole").size(px(24.0))),
                        "secure-padlock",
                        Duration::from_millis(80),
                        12.0,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::EXTRA_BOLD)
                                    .child(format!("#{name} is end-to-end encrypted")),
                            )
                            .child(div().text_sm().text_color(p.muted_foreground).child(
                                "Messages are locked on the sender's device and only open on the devices below. This fuwa \
                                 server keeps and passes along what it can't read.",
                            )),
                    )
                    .child(icon_button("secure-close", "x", &p).on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx)))),
            )
            .child(
                // Everything between the title and Done scrolls when the window is short.
                div()
                    .id("secure-body")
                    .max_h(window.viewport_size().height - px(300.0))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(14.0))
                    .child(cant)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_sm()
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .child("Who can read it")
                        .child(div().text_color(p.muted_foreground).font_weight(FontWeight::BOLD).child(format!("· {count}"))),
                )
                .child(list)
                .child(div().text_xs().text_color(p.muted_foreground).child(
                    "Who's in it follows the channel's permissions. Compare safety numbers in a direct message to verify \
                     someone's devices.",
                ))
                .when_some(history, |el, h| el.child(h))
                .when_some(reset, |el, r| el.child(r))
            )
            .when_some(error_line(self.dialog_error.as_deref(), &p), |el, e| el.child(e))
            .child(
                div().flex().justify_end().child(
                    primary_button("secure-done", "Done", &p).on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))),
                ),
            );
        motion::fade_in(
            scrim("dialog-scrim", &p).on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))).child(
                motion::rise(
                    div().id("dialog-panel").on_click(|_, _, cx| cx.stop_propagation()).child(panel),
                    "dialog-secure",
                    Duration::ZERO,
                    24.0,
                ),
            ),
            "dialog-fade-secure",
            Duration::from_millis(180),
        )
        .into_any_element()
    }

    fn set_secure_history(&mut self, key: String, server: String, channel: String, on: bool, cx: &mut Context<Self>) {
        self.secure_saving = true;
        self.dialog_error = None;
        let core = self.core.clone();
        self.run(cx, async move { core.set_secure_history(&key, &server, &channel, on).await }, |this, result, cx| {
            this.secure_saving = false;
            if let Err(err) = result {
                this.dialog_error = Some(err.0);
            }
            cx.notify();
        });
        cx.notify();
    }

    /// Starts the channel's encryption over (Manage Channels), after asking once.
    fn reset_secure(&mut self, key: String, server: String, channel: String, cx: &mut Context<Self>) {
        match self.secure_reset {
            Reset::Busy => return,
            Reset::Idle => {
                self.secure_reset = Reset::Ask;
                // The question goes away by itself if nobody answers it.
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(Duration::from_secs(4)).await;
                    let _ = this.update(cx, |this, cx| {
                        if this.secure_reset == Reset::Ask {
                            this.secure_reset = Reset::Idle;
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
            Reset::Ask => {
                self.secure_reset = Reset::Busy;
                self.dialog_error = None;
                let write = self.core.shared.read(|s| {
                    s.instance(&key).is_some_and(|i| i.access(&server).has_in(&channel, pb::Permission::SendMessages))
                });
                let core = self.core.clone();
                self.run(
                    cx,
                    async move { core.reset_secure_channel(&key, &server, &channel, write).await },
                    |this, result, cx| {
                        this.secure_reset = Reset::Idle;
                        match result {
                            Ok(()) => {
                                this.dialog = None;
                                this.toast(
                                    "rotate-ccw-key",
                                    "Encryption started over".into(),
                                    "New messages here use fresh keys.".into(),
                                    None,
                                    None,
                                    cx,
                                );
                            }
                            Err(err) => this.dialog_error = Some(err.0),
                        }
                        cx.notify();
                    },
                );
            }
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::vault::DeviceRef;

    fn item(kind: ItemKind, seq: i64, sender: &str) -> Item {
        Item::new(seq, kind, 0, sender, "d")
    }

    fn names(id: &str) -> String {
        match id {
            "u2" => "Yuki".into(),
            "u3" => "Rin".into(),
            _ => "Someone".into(),
        }
    }

    #[test]
    fn lines_say_who_did_what() {
        let mut started = item(ItemKind::Devices, 1, "me");
        started.added = vec![
            DeviceRef { user_id: "me".into(), device_id: "a".into() },
            DeviceRef { user_id: "u2".into(), device_id: "b".into() },
            DeviceRef { user_id: "u2".into(), device_id: "c".into() },
        ];
        assert_eq!(
            channel_line(&started, &names, "me", false).1,
            "You started this secure channel and added your device and Yuki's 2 devices."
        );
        let mut new_device = item(ItemKind::Devices, 5, "u3");
        new_device.added = vec![DeviceRef { user_id: "u3".into(), device_id: "z".into() }];
        assert_eq!(channel_line(&new_device, &names, "me", false).1, "Rin came in on a new device.");
        let mut removed = item(ItemKind::Devices, 6, "u2");
        removed.removed = vec![DeviceRef { user_id: "u3".into(), device_id: "z".into() }];
        assert_eq!(channel_line(&removed, &names, "me", false).1, "Yuki removed Rin's device.");
        let mut on = item(ItemKind::Setting, 7, "me");
        on.content = "on".into();
        assert!(channel_line(&on, &names, "me", false).1.starts_with("You turned on sharing earlier messages"));
        assert!(channel_line(&item(ItemKind::Joined, 8, "me"), &names, "me", true).1.contains("passed on"));
    }

    #[test]
    fn verified_only_when_every_device_was_checked() {
        let mut dms = DmState::default();
        let conversation = pb::Conversation {
            id: "c1".into(),
            users: vec![
                pb::User { id: "me".into(), ..Default::default() },
                pb::User { id: "u2".into(), ..Default::default() },
            ],
            ..Default::default()
        };
        dms.conversations.push(conversation);
        dms.safety.insert("c1".into(), "123".into());
        dms.verified.insert("c1".into(), "123".into());
        let device = |user: &str, key: u8| DmMember {
            user_id: user.into(),
            device_id: format!("{key}"),
            signature_key: vec![key],
        };
        dms.members.insert("c1".into(), vec![device("me", 1), device("u2", 2)]);
        assert!(verified_people(&dms, "me", &[device("me", 1), device("u2", 2)]).contains("u2"));
        // A device of theirs that wasn't in the conversation when you checked.
        assert!(verified_people(&dms, "me", &[device("u2", 2), device("u2", 3)]).is_empty());
        dms.verified.insert("c1".into(), "old".into());
        assert!(verified_people(&dms, "me", &[device("u2", 2)]).is_empty());
    }
}
