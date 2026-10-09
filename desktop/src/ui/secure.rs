//! A secure channel: a server's channel whose messages are end-to-end
//! encrypted between the devices of the people who can see it. It reads and
//! writes through this device's encryption (`core/dms.rs`), like a direct
//! message, and never through the server's messages. The web's
//! `chat/SecureChannelView.tsx`; the list, its lines and the composer are a
//! conversation's (`ui/dm_view.rs`).

use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, Div, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    Stateful, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::core::dms::{DmMember, DmState, DmStatus, SECURE_BROKEN};
use crate::core::i18n::{Arg, t, t_with};
use crate::core::store::InstanceState;
use crate::core::vault::{Item, ItemKind};
use crate::pb;
use crate::ui::app::{Dialog, FuwaApp};
use crate::ui::chat::Row;
use crate::ui::dm_view::{Composer, Earlier, Trust, seal, trust_pill};
use crate::ui::motion;
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_xl};
use crate::ui::widgets::{avatar, icon, pal};

/// What a secure channel can't do, said once: the server can't read it, so nothing that needs to can work.
const CANT: [(&str, &str); 4] = [
    ("shield-off", "chat.secure.cant.automod"),
    ("search-x", "chat.secure.cant.search"),
    ("bot-off", "chat.secure.cant.bots"),
    ("image-off", "chat.secure.cant.links"),
];

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

/// The top of a secure channel's list: its title, and how it's kept private
/// (with `{secure}` for the words in bold).
pub fn beginning(name: &str, shares_history: bool) -> Row {
    let about = t_with("chat.secure.beginning.about", &[("secure", Arg::Str("{secure}"))]);
    let history = t(if shares_history { "chat.secure.beginning.history" } else { "chat.secure.beginning.noHistory" });
    Row::Start {
        icon: "shield-check",
        title: t_with("chat.beginning.title", &[("channel", Arg::Str(name))]),
        body: format!("{about} {history}"),
        shared: None,
    }
}

/// The top of a secure channel (the web's `SecureBeginning`): a shield with a
/// lock that pops into place, its title, and how it's kept private.
pub fn start(title: String, body: String, p: &Palette) -> Div {
    let s = seal(p);
    let card: gpui_kit::Hsla = p.card.into();
    let badge = motion::once(
        div().absolute().right(px(-6.0)).bottom(px(-6.0)).size(px(28.0)).flex().items_center().justify_center(),
        "secure-start-badge",
        Duration::from_millis(900),
        move |el, t| {
            let k = ((t - 0.5) / 0.5).clamp(0.0, 1.0);
            let pop = if k < 1.0 { 1.0 - (1.0 - k).powi(3) + (k * std::f32::consts::PI).sin() * 0.12 } else { 1.0 };
            el.scale(pop.max(0.001)).child(
                div()
                    .size(px(28.0))
                    .rounded_full()
                    .bg(s.green)
                    .shadow(vec![gpui_kit::BoxShadow {
                        color: card,
                        offset: gpui_kit::point(px(0.0), px(0.0)),
                        blur_radius: px(0.0),
                        spread_radius: px(4.0),
                        inset: false,
                    }])
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(gpui_kit::white())
                    .child(icon("lock-keyhole").size(px(14.0))),
            )
        },
    );
    let note = crate::ui::text::hint_line(&body, &[("secure", &t("chat.secure.beginning.secureChannel"))], p);
    div()
        .px(px(16.0))
        .pt(px(40.0 + crate::ui::dm_view::fit()))
        .pb(px(16.0))
        .flex()
        .flex_col()
        .items_start()
        .child(
            div()
                .relative()
                .size(px(64.0))
                .rounded(radius_2xl())
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui_kit::Hsla { a: 0.15, ..s.green })
                .text_color(s.icon)
                .child(icon("shield-check").size(px(32.0)))
                .child(badge),
        )
        .child(
            div()
                .mt(px(12.0))
                .text_size(px(30.0))
                .line_height(px(36.0))
                .font_weight(FontWeight::EXTRA_BOLD)
                .child(title),
        )
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
                .bg(gpui_kit::Hsla { a: 0.1, ..s.green })
                .text_sm()
                .line_height(px(20.0))
                .text_color(s.note)
                .child(icon("lock-keyhole").size(px(16.0)).flex_none().mt(px(2.0)).text_color(s.icon))
                .child(div().flex_1().min_w_0().child(note)),
        )
}

/// What changed about the channel's devices, in words, from the commit itself
/// (not from the server): the web's `channelLine`.
pub fn channel_line(item: &Item, name_of: &dyn Fn(&str) -> String, me: &str, earlier: Earlier) -> String {
    let mine = item.sender_id == me;
    let capital = |text: String| {
        let mut chars = text.chars();
        chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
    };
    let sender = name_of(&item.sender_id);
    // A line about what the sender did: theirs by name, or yours.
    let said = |theirs: &str, yours: &str, values: &[(&str, &str)]| {
        let mut args: Vec<(&str, Arg)> = values.iter().map(|(k, v)| (*k, Arg::Str(v))).collect();
        if mine {
            capital(t_with(yours, &args))
        } else {
            args.push(("name", Arg::Str(&sender)));
            capital(t_with(theirs, &args))
        }
    };
    match item.kind {
        ItemKind::Joined => {
            return t(match earlier {
                Earlier::Shared => "chat.secure.line.joinedShared",
                Earlier::Backup => "chat.secure.line.joinedBackup",
                Earlier::Restorable => "chat.secure.line.joinedRestorable",
                Earlier::None => "chat.secure.line.joined",
            });
        }
        ItemKind::Unreadable => {
            return if mine {
                t("chat.secure.line.unreadableMine")
            } else {
                t_with("chat.secure.line.unreadable", &[("name", Arg::Str(&sender))])
            };
        }
        ItemKind::Setting if item.content == "on" => {
            return said("chat.secure.line.historyOn", "chat.secure.line.historyOnMine", &[]);
        }
        ItemKind::Setting => return said("chat.secure.line.historyOff", "chat.secure.line.historyOffMine", &[]),
        ItemKind::Thread if item.content == "locked" => {
            return said("chat.secure.line.locked", "chat.secure.line.lockedMine", &[]);
        }
        ItemKind::Thread => return said("chat.secure.line.unlocked", "chat.secure.line.unlockedMine", &[]),
        ItemKind::Reset => return said("chat.secure.line.reset", "chat.secure.line.resetMine", &[]),
        _ => {}
    }
    let devices = |list: &[crate::core::vault::DeviceRef]| -> String {
        let mut order: Vec<&str> = Vec::new();
        for d in list {
            if !order.contains(&d.user_id.as_str()) {
                order.push(&d.user_id);
            }
        }
        let parts: Vec<String> = order
            .into_iter()
            .map(|user| {
                let count = Arg::Num(list.iter().filter(|d| d.user_id == user).count() as i64);
                if user == me {
                    t_with("chat.secure.line.yourDevices", &[("count", count)])
                } else {
                    t_with("chat.secure.line.theirDevices", &[("count", count), ("name", Arg::Str(&name_of(user)))])
                }
            })
            .collect();
        crate::core::shared::list_names(parts.iter().map(String::as_str))
    };
    let (added, removed) = (devices(&item.added), devices(&item.removed));
    let alone = item.added.len() == 1 && item.added[0].user_id == item.sender_id && item.removed.is_empty();
    if item.seq == 1 {
        return if added.is_empty() {
            said("chat.secure.line.startedAlone", "chat.secure.line.startedAloneMine", &[])
        } else {
            said("chat.secure.line.started", "chat.secure.line.startedMine", &[("added", &added)])
        };
    }
    if alone {
        return said("chat.secure.line.newDevice", "chat.secure.line.newDeviceMine", &[]);
    }
    match (added.is_empty(), removed.is_empty()) {
        (false, false) => said(
            "chat.secure.line.addedRemoved",
            "chat.secure.line.addedRemovedMine",
            &[("added", &added), ("removed", &removed)],
        ),
        (false, true) => said("chat.secure.line.added", "chat.secure.line.addedMine", &[("added", &added)]),
        (true, false) => said("chat.secure.line.removed", "chat.secure.line.removedMine", &[("removed", &removed)]),
        (true, true) => said("chat.secure.line.refreshed", "chat.secure.line.refreshedMine", &[]),
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
    /// A secure channel's rows, from what this device opened (its threads' replies stay in their threads).
    pub(crate) fn secure_rows(&self, i: &InstanceState, key: &str, server: &str, channel: &str) -> Vec<Row> {
        self.secure_list(i, key, server, channel, None)
    }

    /// A secure channel's header (the web's `SecureHeader`): its shield, name
    /// and topic, the connection when it isn't live, the bell, and the pill
    /// that says who can read it.
    pub(crate) fn secure_header(
        &mut self,
        key: &str,
        server: &str,
        channel: &pb::Channel,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let s = seal(p);
        let dialog = Dialog::Secure { key: key.to_owned(), server: server.to_owned(), channel: channel.id.clone() };
        let connection = self.core.shared.read(|st| st.instance(key).map(|i| i.connection));
        let offline = connection.filter(|c| *c != crate::core::store::Connection::Live);
        // Threads show once encryption runs here.
        let ready = self.core.shared.read(|st| st.instance(key).is_some_and(|i| i.dms.status == DmStatus::Ready));
        div()
            .h(px(56.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(16.0))
            .border_b_1()
            .border_color(p.border)
            .child(motion::rise(
                div()
                    .flex()
                    .min_w_0()
                    .flex_shrink(1.0)
                    .items_center()
                    .gap(px(8.0))
                    .child(icon("shield-check").size(px(20.0)).text_color(s.icon))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(16.0))
                            .line_height(px(24.0))
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .child(channel.name.clone()),
                    ),
                SharedString::from(format!("secure-title-{}", channel.id)),
                Duration::ZERO,
                10.0,
            ))
            .when(!channel.topic.is_empty(), |el| {
                el.child(div().flex_none().w(px(1.0)).h(px(20.0)).bg(p.border)).child(
                    div()
                        .min_w_0()
                        .flex_shrink(1.0)
                        .truncate()
                        .text_sm()
                        .line_height(px(20.0))
                        .text_color(p.muted_foreground)
                        .child(channel.topic.clone()),
                )
            })
            .child(div().flex_1())
            .when_some(offline, |el, c| {
                el.child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .px(px(10.0))
                        .py(px(4.0))
                        .rounded_full()
                        .bg(p.muted)
                        .text_xs()
                        .line_height(px(16.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.muted_foreground)
                        .child(crate::ui::widgets::conn_dot(c, p))
                        .child(crate::ui::instance_home::connection_label(c)),
                )
            })
            .child(self.bell_button(key, server, &channel.id, cx))
            .when(ready, |el| el.child(self.secure_threads_button(&channel.id, p, cx)))
            .child(
                trust_pill("secure-pill", Trust::Encrypted, t("chat.secure.encrypted"), p)
                    .tooltip(|window, cx| crate::ui::overlay::Tip::new(t("chat.secure.seeWho")).build(window, cx))
                    .on_click(cx.listener(move |this, _, window, cx| this.open_dialog(dialog.clone(), window, cx))),
            )
    }

    /// Under a secure channel's header: what was said and the composer once
    /// encryption runs here, or why it doesn't yet (the web's `NotReady`).
    pub(crate) fn secure_body(
        &mut self,
        key: &str,
        server: &str,
        channel: &pb::Channel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = pal(cx);
        let (status, problem, me, broken, can_send, can_reset, can_attach) = self.core.shared.read(|s| {
            let Some(i) = s.instance(key) else { return (DmStatus::Off, None, false, false, false, false, false) };
            let access = i.access(server);
            (
                i.dms.status,
                i.dms.problem.clone(),
                i.me.is_some(),
                i.dms.blocked.get(&channel.id).is_some_and(|b| b == SECURE_BROKEN),
                access.has_in(&channel.id, pb::Permission::SendMessages),
                access.has_in(&channel.id, pb::Permission::ManageChannels),
                access.has_in(&channel.id, pb::Permission::SendMessages)
                    && access.has_in(&channel.id, pb::Permission::AttachFiles),
            )
        });
        if status != DmStatus::Ready || !me {
            return match status {
                DmStatus::Failed => crate::ui::dm_view::unavailable(
                    &problem.unwrap_or_else(|| t("chat.secure.unavailable")),
                    "shield-off",
                    &p,
                ),
                _ => crate::ui::dm_view::starting(&p, window),
            };
        }
        let action = (broken && can_reset).then(|| {
            self.reset_button("secure-reset-composer", key, server, &channel.id, false, &p, cx).into_any_element()
        });
        let composer = self.encrypted_composer(
            Composer {
                key: key.to_owned(),
                id: channel.id.clone(),
                promise: t("chat.secure.promise"),
                locked: (!can_send && !broken).then(|| t("chat.secure.noPermission")),
                action,
                files: can_attach,
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
            .child(self.fitted_list(window, cx))
            .child(composer)
            .into_any_element()
    }

    /// Starts the channel's encryption over (Manage Channels), after asking once (the web's `ResetButton`).
    #[allow(clippy::too_many_arguments)]
    fn reset_button(
        &self,
        id: &'static str,
        key: &str,
        server: &str,
        channel: &str,
        in_dialog: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let asking = self.secure_reset == Reset::Ask;
        let busy = self.secure_reset == Reset::Busy;
        let (fg, bg, hover) = if asking {
            (p.destructive, alpha(p.destructive, 0.12), alpha(p.destructive, 0.2))
        } else {
            (p.primary, alpha(p.primary, 0.0), alpha(p.primary, 0.1))
        };
        let (k, s, c) = (key.to_owned(), server.to_owned(), channel.to_owned());
        let glyph: AnyElement = if busy {
            gpui_kit::AnimationExt::with_animation(
                icon("loader").size(px(14.0)),
                "secure-reset-spin",
                gpui_kit::Animation::new(Duration::from_millis(1000)).repeat(),
                |el, t| el.rotate(gpui_kit::percentage(t)),
            )
            .into_any_element()
        } else {
            // The key turns back while the button's pointed at.
            div()
                .id("secure-reset-icon")
                .group_hover(id, |st| st.rotate(gpui_kit::radians(-std::f32::consts::FRAC_PI_4)))
                .child(icon("rotate-ccw-key").size(px(14.0)))
                .into_any_element()
        };
        div()
            .id(id)
            .group(id)
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.0))
            .px(px(12.0))
            .py(px(6.0))
            .rounded(radius_xl())
            .text_xs()
            .line_height(px(16.0))
            .font_weight(FontWeight::BOLD)
            .text_color(fg)
            .bg(bg)
            .hover(move |st| st.bg(hover))
            .cursor_pointer()
            .active(|st| st.scale(0.94))
            .when(busy, |el| el.opacity(0.5))
            .on_click(
                cx.listener(move |this, _, _, cx| this.reset_secure(k.clone(), s.clone(), c.clone(), in_dialog, cx)),
            )
            .child(glyph)
            .child(motion::slide_in(
                div().child(t(if asking { "chat.secure.resetAsk" } else { "chat.secure.reset" })),
                SharedString::from(format!("{id}-{asking}")),
                4.0,
            ))
    }

    /// How a secure channel is kept private (the web's `SecureChannelDialog`):
    /// what the server can't do, and every person (and how many devices) that
    /// can read it. Someone who manages the channel can turn history sharing
    /// on or off, or start its encryption over.
    pub(crate) fn render_secure(
        &mut self,
        key: &str,
        server: &str,
        channel_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = pal(cx);
        let s = seal(&p);
        let me = self.core.shared.read(|s| s.instance(key).and_then(|i| i.me.as_ref().map(|m| m.id.clone())));
        let me = me.unwrap_or_default();
        let (name, rows, verified, shares, can_reset) = self.core.shared.read(|s| {
            let Some(i) = s.instance(key) else { return Default::default() };
            let members = i.dms.members.get(channel_id).cloned().unwrap_or_default();
            let rows: Vec<(String, usize, String, Option<pb::User>)> = people(i, server, &members)
                .into_iter()
                .map(|(id, n)| {
                    let shown = if id == me { t("chat.secure.dialog.you") } else { i.display_name(Some(server), &id) };
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
        let later = if shares { "chat.secure.later.on" } else { "chat.secure.later.off" };
        let lines = CANT.iter().copied().chain([("user-plus", later)]);
        let cant = div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .p(px(12.0))
            .rounded(radius_2xl())
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.muted, 0.4))
            .text_sm()
            .line_height(px(20.0))
            .children(lines.enumerate().map(|(n, (glyph, text))| {
                motion::slide_in(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .text_color(p.muted_foreground)
                        .child(icon(glyph).size(px(16.0)))
                        .child(div().flex_1().min_w_0().child(t(text))),
                    SharedString::from(format!("secure-cant-{n}")),
                    -8.0 - 2.0 * n as f32,
                )
            }));
        let count = rows.len();
        let mut list = div().id("secure-people").max_h(px(256.0)).overflow_y_scroll().mx(px(-4.0)).px(px(4.0));
        for (n, (id, devices, shown, user)) in rows.into_iter().enumerate() {
            let ok = verified.contains(&id);
            let hover = alpha(p.muted, 0.6);
            let devices = Arg::Num(devices as i64);
            list = list.child(motion::rise(
                div()
                    .id(SharedString::from(format!("secure-person-{id}")))
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .px(px(8.0))
                    .py(px(6.0))
                    .rounded(radius_xl())
                    .hover(move |st| st.bg(hover))
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
                                    .text_size(px(16.0))
                                    .line_height(px(24.0))
                                    .font_weight(FontWeight::BOLD)
                                    .child(div().min_w_0().truncate().child(shown))
                                    .when(ok, |el| el.child(icon("badge-check").size(px(16.0)).text_color(s.green))),
                            )
                            .child(div().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(if ok {
                                t_with("chat.secure.dialog.devicesVerified", &[("count", devices)])
                            } else {
                                t_with("chat.secure.dialog.devices", &[("count", devices)])
                            })),
                    ),
                SharedString::from(format!("secure-person-in-{n}")),
                Duration::from_millis(30 * n.min(10) as u64),
                6.0,
            ));
        }
        let (k, sv, c) = (key.to_owned(), server.to_owned(), channel_id.to_owned());
        let history = can_reset.then(|| {
            let (k, sv, c) = (k.clone(), sv.clone(), c.clone());
            let hover = alpha(p.muted, 0.4);
            let saving = self.secure_saving;
            let (k2, sv2, c2) = (k.clone(), sv.clone(), c.clone());
            div()
                .id("secure-history-row")
                .mt(px(16.0))
                .flex()
                .items_start()
                .gap(px(12.0))
                .px(px(12.0))
                .py(px(10.0))
                .rounded(radius_2xl())
                .border_1()
                .border_color(p.border)
                .cursor_pointer()
                .hover(move |st| st.bg(hover))
                .when(!saving, |el| {
                    el.on_click(cx.listener(move |this, _, _, cx| {
                        this.set_secure_history(k2.clone(), sv2.clone(), c2.clone(), !shares, cx)
                    }))
                })
                .child(icon("rotate-ccw-clock").size(px(16.0)).mt(px(2.0)).text_color(p.muted_foreground))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_sm()
                                .line_height(px(20.0))
                                .font_weight(FontWeight::BOLD)
                                .child(t("chat.secure.dialog.shareHistory")),
                        )
                        .child(
                            div()
                                .text_xs()
                                .line_height(px(16.0))
                                .text_color(p.muted_foreground)
                                .child(t("chat.secure.dialog.shareHistoryAbout")),
                        )
                        .when_some(self.dialog_error.clone(), |el, e| {
                            el.child(div().mt(px(4.0)).text_xs().text_color(p.destructive).child(e))
                        }),
                )
                .child(crate::ui::dm_dialogs::web_switch(
                    "secure-history",
                    shares,
                    saving,
                    &p,
                    window,
                    cx,
                    move |this, on, cx| this.set_secure_history(k.clone(), sv.clone(), c.clone(), on, cx),
                ))
        });
        let reset = can_reset.then(|| {
            div()
                .mt(px(12.0))
                .flex()
                .items_center()
                .gap(px(12.0))
                .px(px(12.0))
                .py(px(10.0))
                .rounded(radius_2xl())
                .border_1()
                .border_dashed()
                .border_color(p.border)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_xs()
                        .line_height(px(16.0))
                        .text_color(p.muted_foreground)
                        .child(t("chat.secure.dialog.resetAbout")),
                )
                .child(self.reset_button("secure-reset", key, server, channel_id, true, &p, cx))
        });
        // "Who can read it {count}": the words around the count, which is muted.
        let template = t_with("chat.secure.dialog.whoCanRead", &[("count", Arg::Str("\u{1}"))]);
        let (before, after) = template.split_once('\u{1}').unwrap_or((template.as_str(), ""));
        let who_before = (!before.trim().is_empty()).then(|| div().child(before.trim().to_owned()));
        let who_after = (!after.trim().is_empty()).then(|| div().child(after.trim().to_owned()));
        let body = div()
            .flex()
            .flex_col()
            .child(div().mb(px(16.0)).flex().justify_center().child(crate::ui::dm_dialogs::padlock("secure", &p)))
            .child(crate::ui::dm_dialogs::dialog_header(
                t_with("chat.secure.dialog.title", &[("channel", Arg::Str(&name))]).into_any_element(),
                Some(t("chat.secure.dialog.description").into_any_element()),
                &p,
            ))
            .child(cant)
            .child(
                div()
                    .mt(px(20.0))
                    .mb(px(8.0))
                    .text_sm()
                    .line_height(px(20.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .flex()
                    .gap(px(4.0))
                    .children(who_before)
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .text_color(p.muted_foreground)
                            .child(t_with("chat.secure.dialog.peopleCount", &[("count", Arg::Num(count as i64))])),
                    )
                    .children(who_after),
            )
            .child(list)
            .child(
                div()
                    .mt(px(16.0))
                    .text_xs()
                    .line_height(px(16.0))
                    .text_color(p.muted_foreground)
                    .child(t("chat.secure.dialog.whoNote")),
            )
            .children(history)
            .children(reset)
            .when_some(self.dialog_error.clone().filter(|_| !can_reset), |el, e| {
                el.child(div().mt(px(8.0)).text_xs().text_color(p.destructive).child(e))
            });
        crate::ui::dm_dialogs::dialog_shell("secure", 512.0, body, window, cx)
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
    fn reset_secure(&mut self, key: String, server: String, channel: String, in_dialog: bool, cx: &mut Context<Self>) {
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
                    move |this, result, cx| {
                        this.secure_reset = Reset::Idle;
                        match result {
                            Ok(()) => {
                                if in_dialog {
                                    this.dialog = None;
                                }
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
            channel_line(&started, &names, "me", Earlier::None),
            "You started this secure channel and added your device and Yuki's 2 devices."
        );
        let mut new_device = item(ItemKind::Devices, 5, "u3");
        new_device.added = vec![DeviceRef { user_id: "u3".into(), device_id: "z".into() }];
        assert_eq!(channel_line(&new_device, &names, "me", Earlier::None), "Rin came in on a new device.");
        let mut removed = item(ItemKind::Devices, 6, "u2");
        removed.removed = vec![DeviceRef { user_id: "u3".into(), device_id: "z".into() }];
        assert_eq!(channel_line(&removed, &names, "me", Earlier::None), "Yuki removed Rin's device.");
        let mut on = item(ItemKind::Setting, 7, "me");
        on.content = "on".into();
        assert!(channel_line(&on, &names, "me", Earlier::None).starts_with("You turned on sharing earlier messages"));
        assert!(channel_line(&item(ItemKind::Joined, 8, "me"), &names, "me", Earlier::Shared).contains("passed on"));
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
