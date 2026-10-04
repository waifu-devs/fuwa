//! What floats over the window: dialogs and toasts.

use std::time::Duration;

use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::Input;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, Div, ElementId, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::ui::app::{Dialog, FuwaApp};
use crate::ui::motion;
use crate::ui::text::safety_rows;
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::{
    card, danger_button, error_line, icon, icon_button, labeled, pal, primary_button, soft_button,
};

/// What a server or instance calls its identity provider, for "Continue with …".
pub fn provider_name(name: &str) -> &str {
    if name.trim().is_empty() { "your organization" } else { name }
}

/// Names the site a sign-in sends people to before they go: it sees their
/// IP address, which fuwa itself never hands out.
pub fn host_notice(host: &str, p: &Palette) -> AnyElement {
    if host.is_empty() {
        return div().into_any_element();
    }
    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .px(px(12.0))
        .py(px(10.0))
        .rounded(corner(12.0))
        .bg(alpha(p.primary, 0.08))
        .text_sm()
        .text_color(p.muted_foreground)
        .child(icon("globe").size(px(16.0)).flex_none().text_color(p.primary))
        .child(div().flex_1().min_w_0().child(host_sentence(host, p)))
        .into_any_element()
}

/// "Signs you in at <host>, which sees your IP address.", the host in bold.
pub fn host_sentence(host: &str, p: &Palette) -> gpui_kit::StyledText {
    let lead = "Signs you in at ";
    let text = format!("{lead}{host}, which sees your IP address.");
    let bold = gpui_kit::HighlightStyle {
        font_weight: Some(FontWeight::BOLD),
        color: Some(p.foreground.into()),
        ..Default::default()
    };
    gpui_kit::StyledText::new(text).with_highlights([(lead.len()..lead.len() + host.len(), bold)])
}

/// A dim layer over the window, fading in, that swallows clicks.
pub fn scrim(id: impl Into<ElementId>, p: &Palette) -> Stateful<Div> {
    let id = id.into();
    div()
        .id(id)
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(alpha(p.rail, if p.dark { 0.72 } else { 0.55 }))
        .occlude()
}

impl FuwaApp {
    pub(crate) fn render_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.dialog.clone()?;
        let p = pal(cx);
        if let Dialog::Profile { key, user_id, server } = &dialog {
            return Some(self.render_profile(key, user_id, server.as_deref(), cx));
        }
        if let Dialog::Welcome { key, server } = &dialog {
            return Some(self.render_welcome(key, server, cx));
        }
        if let Dialog::Secure { key, server, channel } = &dialog {
            return Some(self.render_secure(key, server, channel, window, cx));
        }
        let busy = self.dialog_busy;
        let field = || Input::new(&self.dialog_input).large();
        let sso_label = match &dialog {
            Dialog::SsoJoin { .. } if busy => "Waiting for your browser…".to_owned(),
            Dialog::SsoJoin { provider, .. } => format!("Continue with {}", provider_name(provider)),
            _ => String::new(),
        };
        let (glyph, title, body, content, action): (&str, String, String, AnyElement, Option<&str>) = match &dialog {
            Dialog::CreateServer { .. } => (
                "sparkles",
                "Make a server".into(),
                "A home for your people. You can change its name and picture later.".into(),
                labeled("Server name", field(), &p).into_any_element(),
                Some(if busy { "Making it…" } else { "Make it" }),
            ),
            Dialog::JoinInvite { .. } => (
                "user-plus",
                "Join a server".into(),
                "Paste the invite link someone sent you, or just its code.".into(),
                labeled("Invite", field(), &p).into_any_element(),
                Some(if busy { "Joining…" } else { "Join" }),
            ),
            Dialog::SsoJoin { name, provider, host, .. } => (
                "lock-keyhole",
                format!("Sign in to join {name}"),
                format!(
                    "{name} asks its members to sign in through {}. Your browser opens to do it.",
                    provider_name(provider)
                ),
                host_notice(host, &p),
                Some(sso_label.as_str()),
            ),
            Dialog::LeaveServer { key, server } => {
                let name = self
                    .core
                    .shared
                    .read(|s| s.instance(key).and_then(|i| i.server(server)).map(|s| s.name.clone()))
                    .unwrap_or_default();
                (
                    "log-out",
                    format!("Leave {name}?"),
                    "You'll need a new invite to come back.".into(),
                    div().into_any_element(),
                    Some(if busy { "Leaving…" } else { "Leave" }),
                )
            }
            Dialog::Invite { link, .. } => {
                let streamer = self.prefs.streamer_mode;
                let copied = self.copied.is_some_and(|at| at.elapsed() < Duration::from_secs(3));
                let shown = match link {
                    Some(_) if streamer => "Hidden in streamer mode".to_owned(),
                    Some(l) => l.clone(),
                    None => "Making an invite…".to_owned(),
                };
                (
                    "user-plus",
                    "Invite people".into(),
                    "Anyone with this link can join. It doesn't run out.".into(),
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .px(px(14.0))
                        .h(px(44.0))
                        .rounded(corner(12.0))
                        .bg(p.secondary)
                        .border_1()
                        .border_color(p.border)
                        .child(div().flex_1().min_w_0().whitespace_nowrap().text_ellipsis().child(shown))
                        .when(copied, |el| {
                            el.child(motion::rise(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .text_color(p.success)
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .child(icon("check").size(px(14.0)))
                                    .child("Copied"),
                                "copied",
                                Duration::ZERO,
                                6.0,
                            ))
                        })
                        .into_any_element(),
                    link.as_ref().map(|_| "Copy link"),
                )
            }
            Dialog::Safety { key, conversation } => {
                let (safety, verified, other) = self.core.shared.read(|s| {
                    let i = s.instance(key);
                    let me = i.and_then(|i| i.me.as_ref().map(|m| m.id.clone())).unwrap_or_default();
                    (
                        i.and_then(|i| i.dms.safety.get(conversation).cloned()),
                        i.and_then(|i| i.dms.verified.get(conversation).cloned()),
                        i.and_then(|i| i.dms.conversations.iter().find(|c| &c.id == conversation))
                            .and_then(|c| c.users.iter().find(|u| u.id != me).map(crate::core::store::user_name)),
                    )
                });
                let other = other.unwrap_or_else(|| "them".into());
                let is_verified = verified.is_some() && verified == safety;
                let rows = safety.as_deref().map(safety_rows).unwrap_or_default();
                let grid = div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .p(px(18.0))
                    .rounded(corner(14.0))
                    .bg(p.secondary)
                    .font_family("monospace")
                    .text_lg()
                    .text_center()
                    .children(rows.into_iter().enumerate().map(|(n, row)| {
                        motion::rise(
                            div().child(row),
                            SharedString::from(format!("safety-{n}")),
                            Duration::from_millis(60 * n as u64),
                            8.0,
                        )
                    }))
                    .when(safety.is_none(), |el| el.child("Still working it out…"));
                (
                    if is_verified { "shield-check" } else { "shield" },
                    if is_verified { "You've verified this conversation".into() } else { format!("Verify {other}") },
                    format!(
                        "Compare these numbers with {other}, in person or somewhere you trust. If they match, nobody is listening in between."
                    ),
                    grid.into_any_element(),
                    (!is_verified && safety.is_some()).then_some("They match"),
                )
            }
            Dialog::CreateChannel { key, server, parent, kind } => {
                let category = *kind == crate::pb::ChannelType::Category;
                let under = self
                    .core
                    .shared
                    .read(|s| s.instance(key).and_then(|i| i.channel(server, parent)).map(|c| c.name.clone()));
                let in_category = !parent.is_empty();
                let tiles = crate::ui::secure::KINDS
                    .into_iter()
                    .filter(|(k, ..)| !(in_category && *k == crate::pb::ChannelType::Category))
                    .enumerate()
                    .map(|(n, (k, glyph, label, hint))| {
                        let on = k == *kind;
                        let tint = if k == crate::pb::ChannelType::Secure { p.success } else { p.primary };
                        let hover = alpha(tint, 0.06);
                        motion::rise(
                            div()
                                .id(SharedString::from(format!("kind-{label}")))
                                .w(px(200.0))
                                .flex_grow(1.0)
                                .flex()
                                .items_center()
                                .gap(px(10.0))
                                .p(px(10.0))
                                .rounded(corner(14.0))
                                .border_1()
                                .border_color(if on { tint } else { p.border })
                                .bg(if on { alpha(tint, 0.1) } else { p.secondary.into() })
                                .when(!on, |el| el.hover(move |s| s.bg(hover)))
                                .cursor_pointer()
                                .active(|s| s.top(px(1.0)))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if let Some(Dialog::CreateChannel { kind, parent, .. }) = &mut this.dialog {
                                        *kind = k;
                                        if k == crate::pb::ChannelType::Category {
                                            parent.clear();
                                        }
                                    }
                                    let hint = crate::ui::secure::name_hint(k);
                                    this.dialog_input.update(cx, |s, cx| s.set_placeholder(hint, window, cx));
                                    cx.notify();
                                }))
                                .child(
                                    div()
                                        .size(px(34.0))
                                        .flex_none()
                                        .rounded(corner(10.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .bg(if on { tint } else { p.muted })
                                        .text_color(if on { p.primary_foreground } else { p.muted_foreground })
                                        .child(motion::rise(
                                            icon(glyph).size(px(18.0)),
                                            SharedString::from(format!("kind-glyph-{label}-{on}")),
                                            Duration::ZERO,
                                            4.0,
                                        )),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .child(div().text_sm().font_weight(FontWeight::BOLD).child(label))
                                        .child(div().text_xs().text_color(p.muted_foreground).child(hint)),
                                ),
                            SharedString::from(format!("kind-in-{n}")),
                            Duration::from_millis(30 * n as u64),
                            6.0,
                        )
                    });
                let kinds = div().flex().flex_wrap().gap(px(8.0)).children(tiles);
                (
                    match kind {
                        crate::pb::ChannelType::Category => "folder-plus",
                        crate::pb::ChannelType::Secure => "shield-check",
                        crate::pb::ChannelType::Voice => "volume-2",
                        crate::pb::ChannelType::Announcement => "megaphone",
                        _ => "hash",
                    },
                    if category { "Make a category".into() } else { "Make a channel".into() },
                    match under {
                        Some(name) if !category => format!("It goes in {name}."),
                        _ if category => "Categories group channels together in the sidebar.".into(),
                        _ => "A place to talk about one thing.".into(),
                    },
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(14.0))
                        .child(kinds)
                        .child(labeled(if category { "Category name" } else { "Channel name" }, field(), &p))
                        .into_any_element(),
                    Some(if busy { "Making it…" } else { "Make it" }),
                )
            }
            Dialog::Rules { key, server } => {
                let name = self
                    .core
                    .shared
                    .read(|s| s.instance(key).and_then(|i| i.server(server)).map(|s| s.name.clone()))
                    .unwrap_or_default();
                let list = match &self.rules {
                    None => div().text_sm().text_color(p.muted_foreground).child("Getting the rules…"),
                    Some(rules) if rules.is_empty() => {
                        div().text_sm().text_color(p.muted_foreground).child("This server has no rules written down.")
                    }
                    Some(rules) => {
                        div().flex().flex_col().gap(px(8.0)).children(rules.iter().enumerate().map(|(n, rule)| {
                            motion::rise(
                                div()
                                    .flex()
                                    .gap(px(12.0))
                                    .p(px(12.0))
                                    .rounded(corner(12.0))
                                    .bg(p.secondary)
                                    .child(
                                        div()
                                            .size(px(24.0))
                                            .flex_none()
                                            .rounded_full()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .bg(alpha(p.primary, 0.16))
                                            .text_color(p.primary)
                                            .text_xs()
                                            .font_weight(FontWeight::EXTRA_BOLD)
                                            .child((n + 1).to_string()),
                                    )
                                    .child(div().flex_1().min_w_0().text_sm().child(rule.clone())),
                                SharedString::from(format!("rule-{n}")),
                                Duration::from_millis(50 * n.min(10) as u64),
                                8.0,
                            )
                        }))
                    }
                };
                (
                    "scroll-text",
                    format!("{name}'s rules"),
                    "Read them, then agree to start talking.".into(),
                    div().id("rules").max_h(px(360.0)).overflow_y_scroll().child(list).into_any_element(),
                    self.rules.as_ref().map(|_| if busy { "Agreeing…" } else { "I agree" }),
                )
            }
            Dialog::Moderate { key, server, user_id, action } => self.moderate_parts(key, server, user_id, *action, cx),
            Dialog::AllowGame { name, .. } => (
                "gamepad-2",
                format!("{name} wants to show what you're playing"),
                "It reports to Discord's apps on this computer, and fuwa can show it to people you share a server with, if you turned that on. You can change your mind in Privacy settings.".into(),
                div().into_any_element(),
                Some("Allow"),
            ),
            Dialog::Profile { .. } | Dialog::Welcome { .. } | Dialog::Secure { .. } => unreachable!("drawn on its own"),
        };
        let danger = matches!(
            dialog,
            Dialog::LeaveServer { .. }
                | Dialog::Moderate {
                    action: crate::core::moderation::Action::Kick | crate::core::moderation::Action::Ban(_),
                    ..
                }
        );
        let panel = card(&p)
            .w(px(460.0))
            .p(px(24.0))
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(14.0))
                    .child(
                        div()
                            .size(px(44.0))
                            .flex_none()
                            .rounded(corner(14.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(alpha(if danger { p.destructive } else { p.primary }, 0.14))
                            .text_color(if danger { p.destructive } else { p.primary })
                            .child(icon(glyph).size(px(22.0))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(div().text_lg().font_weight(FontWeight::EXTRA_BOLD).child(title))
                            .child(div().text_sm().text_color(p.muted_foreground).child(body)),
                    )
                    .child(
                        icon_button("dialog-close", "x", &p)
                            .on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))),
                    ),
            )
            .child(content)
            .when_some(error_line(self.dialog_error.as_deref(), &p), |el, e| el.child(e))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(10.0))
                    .child(if matches!(dialog, Dialog::AllowGame { .. }) {
                        soft_button("dialog-cancel", "Don't allow", &p)
                            .on_click(cx.listener(|this, _, _, cx| this.refuse_game(cx)))
                    } else {
                        soft_button("dialog-cancel", "Cancel", &p)
                            .on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx)))
                    })
                    .when_some(action, |el, label| {
                        let button = if danger {
                            danger_button("dialog-ok", label, &p)
                        } else {
                            primary_button("dialog-ok", label, &p)
                        }
                        .when(busy, |el| el.opacity(0.7))
                        .on_click(cx.listener(|this, _, window, cx| this.confirm_dialog(window, cx)));
                        el.child(button)
                    }),
            );
        let tag = match &dialog {
            Dialog::CreateServer { .. } => "create",
            Dialog::JoinInvite { .. } => "join",
            Dialog::Invite { .. } => "invite",
            Dialog::Safety { .. } => "safety",
            Dialog::LeaveServer { .. } => "leave",
            Dialog::CreateChannel { .. } => "channel",
            Dialog::Rules { .. } => "rules",
            Dialog::Profile { .. } => "profile",
            Dialog::Welcome { .. } => "welcome",
            Dialog::Secure { .. } => "secure",
            Dialog::Moderate { .. } => "moderate",
            Dialog::SsoJoin { .. } => "sso",
            Dialog::AllowGame { .. } => "game",
        };
        Some(
            motion::fade_in(
                scrim("dialog-scrim", &p).on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))).child(
                    motion::rise(
                        div().id("dialog-panel").on_click(|_, _, cx| cx.stop_propagation()).child(panel),
                        SharedString::from(format!("dialog-{tag}")),
                        Duration::ZERO,
                        24.0,
                    ),
                ),
                SharedString::from(format!("dialog-fade-{tag}")),
                Duration::from_millis(180),
            )
            .into_any_element(),
        )
    }

    /// Someone's card: a band in their color, their picture over it, their
    /// names, pronouns and bio, and a button to message them.
    fn render_profile(&mut self, key: &str, user_id: &str, server: Option<&str>, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let (user, nickname, me, roles) = self.core.shared.read(|s| {
            let Some(i) = s.instance(key) else { return (None, String::new(), false, Vec::new()) };
            let member = server.and_then(|sid| {
                i.members.get(sid)?.iter().find(|m| m.user.as_ref().is_some_and(|u| u.id == user_id)).cloned()
            });
            let roles: Vec<(String, Option<u32>)> = match (server, &member) {
                (Some(sid), Some(m)) => i
                    .roles
                    .get(sid)
                    .into_iter()
                    .flatten()
                    .filter(|r| m.role_ids.contains(&r.id))
                    .map(|r| (r.name.clone(), r.color.map(|c| c as u32)))
                    .collect(),
                _ => Vec::new(),
            };
            (
                i.users.get(user_id).cloned(),
                member.map(|m| m.nickname).unwrap_or_default(),
                i.me.as_ref().is_some_and(|m| m.id == user_id),
                roles,
            )
        });
        let profile = self.profile.clone().filter(|pr| pr.user.as_ref().is_some_and(|u| u.id == user_id));
        let user = profile.as_ref().and_then(|pr| pr.user.clone()).or(user);
        let name = if nickname.is_empty() {
            user.as_ref().map(crate::core::store::user_name).unwrap_or_else(|| "Someone".into())
        } else {
            nickname
        };
        let accent = profile
            .as_ref()
            .and_then(|pr| pr.accent_color)
            .map(|c| gpui_kit::Hsla::from(gpui_kit::rgb(c as u32)))
            .unwrap_or_else(|| crate::ui::widgets::hue_color(user_id, p.dark));
        let streamer = self.prefs.streamer_mode;
        let status = user.as_ref().map(|u| u.status.clone()).unwrap_or_default();
        let mut info = div().px(px(20.0)).pb(px(20.0)).flex().flex_col().gap(px(10.0)).child(
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(div().text_xl().font_weight(FontWeight::EXTRA_BOLD).child(name))
                        .when(crate::ui::widgets::is_agent(user.as_ref()), |el| {
                            el.child(crate::ui::widgets::app_badge("profile-badge", "AGENT", &p))
                        }),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(6.0))
                        .text_sm()
                        .text_color(p.muted_foreground)
                        .when(!streamer || !me, |el| {
                            el.child(format!("@{}", user.as_ref().map(|u| u.username.as_str()).unwrap_or("")))
                        })
                        .when_some(
                            profile.as_ref().map(|pr| pr.pronouns.clone()).filter(|x| !x.is_empty()),
                            |el, pronouns| el.child("·").child(pronouns),
                        ),
                ),
        );
        if !status.is_empty() {
            info = info.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(12.0))
                    .py(px(8.0))
                    .rounded(corner(12.0))
                    .bg(p.secondary)
                    .text_sm()
                    .child(icon("message-circle-heart").size(px(14.0)).text_color(p.primary))
                    .child(status),
            );
        }
        if let Some(bio) = profile.as_ref().map(|pr| pr.bio.clone()).filter(|b| !b.is_empty()) {
            info = info.child(
                div().flex().flex_col().gap(px(4.0)).child(section_title("About", &p)).child(
                    crate::ui::text::markdown("profile-bio", crate::ui::text::images_as_links(&bio))
                        .selectable(true)
                        .w_full(),
                ),
            );
        } else if profile.is_none() {
            info = info.child(div().h(px(14.0)).w(px(180.0)).rounded_full().bg(alpha(p.muted_foreground, 0.12)));
        }
        if !roles.is_empty() {
            info = info.child(div().flex().flex_col().gap(px(6.0)).child(section_title("Roles", &p)).child(
                div().flex().flex_wrap().gap(px(6.0)).children(roles.into_iter().map(|(role, color)| {
                    let dot =
                        color.map(|c| gpui_kit::Hsla::from(gpui_kit::rgb(c))).unwrap_or(p.muted_foreground.into());
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .px(px(10.0))
                        .h(px(26.0))
                        .rounded_full()
                        .bg(p.secondary)
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .child(div().size(px(10.0)).rounded_full().bg(dot))
                        .child(role)
                })),
            ));
        }
        if let Some(buttons) = server.and_then(|sid| self.moderation_buttons(key, sid, user_id, &p, cx)) {
            info = info.child(buttons);
        }
        // Agents have no private messages.
        if !me && !crate::ui::widgets::is_agent(user.as_ref()) {
            info = info.child(
                primary_button("profile-message", "Message", &p)
                    .w_full()
                    .child(icon("lock").size(px(14.0)))
                    .on_click(cx.listener(|this, _, window, cx| this.confirm_dialog(window, cx))),
            );
        }
        let panel = card(&p)
            .w(px(380.0))
            .overflow_hidden()
            .child(div().h(px(96.0)).rounded_t(corner(20.0)).bg(accent))
            .child(
                div().px(px(20.0)).mt(px(-44.0)).mb(px(10.0)).child(
                    div()
                        .size(px(88.0))
                        .rounded_full()
                        .border_4()
                        .border_color(p.card)
                        .bg(p.card)
                        .child(crate::ui::widgets::avatar(user.as_ref(), 80.0, &p)),
                ),
            )
            .child(info);
        motion::fade_in(
            scrim("dialog-scrim", &p).on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))).child(
                motion::rise(
                    div().id("dialog-panel").on_click(|_, _, cx| cx.stop_propagation()).child(panel),
                    SharedString::from(format!("profile-{user_id}")),
                    Duration::ZERO,
                    24.0,
                ),
            ),
            "dialog-fade-profile",
            Duration::from_millis(160),
        )
        .into_any_element()
    }

    /// A server's welcome screen: its icon and name, a few words, and the
    /// channels it suggests, each a card that rises after the one before.
    fn render_welcome(&mut self, key: &str, server_id: &str, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let (server, channels, look) = self.core.shared.read(|s| {
            let i = s.instance(key);
            (
                i.and_then(|i| i.server(server_id)).cloned(),
                i.and_then(|i| i.channels.get(server_id)).cloned().unwrap_or_default(),
                i.map(|i| crate::ui::mentions::Look::of(i, server_id)).unwrap_or_default(),
            )
        });
        let Some(server) = server else { return div().into_any_element() };
        let mut body = div().flex().flex_col().items_center().gap(px(12.0)).child(motion::rise(
            div()
                .relative()
                .child(crate::ui::widgets::server_icon(&server, 64.0, 20.0, &p))
                .child(div().absolute().top(px(-10.0)).right(px(-14.0)).text_size(px(22.0)).child("👋")),
            "welcome-icon",
            Duration::ZERO,
            14.0,
        ));
        body = body.child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .child(section_title("WELCOME TO", &p))
                .child(div().text_xl().font_weight(FontWeight::EXTRA_BOLD).child(server.name.clone())),
        );
        match &self.welcome {
            None => {
                body = body.child(
                    div()
                        .text_sm()
                        .text_color(p.muted_foreground)
                        .child(self.dialog_error.clone().unwrap_or_else(|| "Getting the welcome screen…".into())),
                )
            }
            Some(screen) => {
                if !screen.description.trim().is_empty() {
                    let shown = crate::ui::mentions::mention_links(
                        &crate::ui::text::images_as_links(&screen.description),
                        &look,
                    );
                    body = body.child(motion::rise(
                        div().w_full().min_w_0().text_sm().text_color(p.muted_foreground).child(
                            crate::ui::text::markdown("welcome-description", shown)
                                .markdown_extensions(crate::ui::emoji::markdown_extensions()),
                        ),
                        "welcome-description",
                        Duration::from_millis(80),
                        8.0,
                    ));
                }
                let suggested: Vec<_> = screen
                    .channels
                    .iter()
                    .filter_map(|w| channels.iter().find(|c| c.id == w.channel_id).map(|c| (w.clone(), c.clone())))
                    .collect();
                if !suggested.is_empty() {
                    let mut list = div().w_full().flex().flex_col().gap(px(8.0)).child(section_title("START HERE", &p));
                    for (n, (w, channel)) in suggested.into_iter().enumerate() {
                        let hover = alpha(p.primary, 0.08);
                        let border = p.primary;
                        let (k, sid, cid) = (key.to_owned(), server_id.to_owned(), channel.id.clone());
                        let lead = welcome_emoji(&w.emoji, &look, &p);
                        list = list.child(motion::rise(
                            div()
                                .id(SharedString::from(format!("welcome-{}", channel.id)))
                                .flex()
                                .items_center()
                                .gap(px(12.0))
                                .p(px(12.0))
                                .rounded(corner(14.0))
                                .border_1()
                                .border_color(p.border)
                                .bg(p.secondary)
                                .cursor_pointer()
                                .hover(move |s| s.bg(hover).border_color(border))
                                .active(|s| s.top(px(1.0)))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.dialog = None;
                                    this.open_channel(&k, &sid, &cid, window, cx);
                                }))
                                .child(lead)
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .font_weight(FontWeight::BOLD)
                                                .text_sm()
                                                .child(format!("#{}", channel.name)),
                                        )
                                        .when(!w.description.is_empty(), |el| {
                                            el.child(
                                                div()
                                                    .text_xs()
                                                    .text_color(p.muted_foreground)
                                                    .child(w.description.clone()),
                                            )
                                        }),
                                )
                                .child(icon("arrow-right").size(px(16.0)).text_color(p.muted_foreground)),
                            SharedString::from(format!("welcome-in-{n}")),
                            Duration::from_millis(160 + 70 * n as u64),
                            14.0,
                        ));
                    }
                    body = body.child(list);
                }
            }
        }
        body = body.child(
            div()
                .id("welcome-skip")
                .mt(px(4.0))
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .text_color(p.muted_foreground)
                .cursor_pointer()
                .hover({
                    let fg = p.foreground;
                    move |s| s.text_color(fg)
                })
                .on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx)))
                .child("I'll look around myself"),
        );
        let glow = alpha(p.primary, 0.18);
        let panel = card(&p)
            .w(px(440.0))
            .overflow_hidden()
            .relative()
            .child(div().absolute().top_0().left_0().right_0().h(px(120.0)).bg(gpui_kit::linear_gradient(
                180.0,
                gpui_kit::linear_color_stop(glow, 0.0),
                gpui_kit::linear_color_stop(alpha(p.primary, 0.0), 1.0),
            )))
            .child(div().relative().p(px(24.0)).child(body));
        motion::fade_in(
            scrim("dialog-scrim", &p).on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))).child(
                motion::rise(
                    div().id("dialog-panel").on_click(|_, _, cx| cx.stop_propagation()).child(panel),
                    "dialog-welcome",
                    Duration::ZERO,
                    24.0,
                ),
            ),
            "dialog-fade-welcome",
            Duration::from_millis(160),
        )
        .into_any_element()
    }

    pub(crate) fn render_toasts(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let mut stack = div().absolute().right(px(20.0)).bottom(px(100.0)).flex().flex_col().gap(px(10.0)).w(px(340.0));
        for toast in &self.toasts {
            let id = toast.id;
            let open = toast.open.clone();
            let channel = toast.channel.clone();
            let item = card(&p)
                .id(SharedString::from(format!("toast-{id}")))
                .p(px(14.0))
                .flex()
                .gap(px(12.0))
                .cursor_pointer()
                .hover({
                    let border = p.primary;
                    move |s| s.border_color(border)
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(nav) = open.clone() {
                        if let (crate::ui::app::Nav::Server { key, server }, Some(channel)) = (&nav, &channel) {
                            this.open_channel(&key.clone(), &server.clone(), channel, window, cx);
                        } else {
                            this.navigate(nav, window, cx);
                        }
                    }
                    this.dismiss_toast(id, cx);
                }))
                .child(
                    div()
                        .size(px(36.0))
                        .flex_none()
                        .rounded(corner(12.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(alpha(p.primary, 0.14))
                        .text_color(p.primary)
                        .child(icon(toast.icon).size(px(18.0))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(div().font_weight(FontWeight::EXTRA_BOLD).text_sm().child(toast.title.clone()))
                        .child(div().text_sm().text_color(p.muted_foreground).line_clamp(2).child(toast.body.clone())),
                );
            let el: AnyElement = if toast.leaving {
                div().child(item).with_animation_out(SharedString::from(format!("toast-out-{id}"))).into_any_element()
            } else {
                motion::slide_in(div().child(item), SharedString::from(format!("toast-in-{id}")), 60.0)
                    .into_any_element()
            };
            stack = stack.child(el);
        }
        stack
    }
}

fn section_title(text: &str, p: &Palette) -> Div {
    div()
        .text_size(px(11.0))
        .font_weight(FontWeight::EXTRA_BOLD)
        .text_color(p.muted_foreground)
        .child(text.to_uppercase())
}

trait LeaveExt: Sized {
    fn with_animation_out(self, id: SharedString) -> AnyElement;
}

impl LeaveExt for Div {
    /// Slides a toast away to the right as it fades.
    fn with_animation_out(self, id: SharedString) -> AnyElement {
        use gpui_kit::{Animation, AnimationExt as _};
        self.with_animation(
            id,
            Animation::new(Duration::from_millis(240)).with_easing(gpui_kit::ease_in_out),
            |el, t| el.opacity(1.0 - t).relative().left(px(80.0 * t)),
        )
        .into_any_element()
    }
}

/// A suggested channel's emoji: a Unicode one, one of the server's own, or a #.
pub(crate) fn welcome_emoji(emoji: &str, look: &crate::ui::mentions::Look, p: &Palette) -> AnyElement {
    let base = div()
        .size(px(36.0))
        .flex_none()
        .rounded(corner(12.0))
        .flex()
        .items_center()
        .justify_center()
        .bg(alpha(p.primary, 0.12))
        .text_color(p.primary);
    let own = emoji
        .strip_prefix('<')
        .and_then(|e| e.strip_suffix('>'))
        .and_then(|e| e.rsplit_once(':'))
        .and_then(|(_, id)| look.emojis.get(&id.to_uppercase()));
    match own {
        Some(url) => {
            use gpui_kit::StyledImage as _;
            base.child(
                gpui_kit::img(SharedString::from(url.clone())).size(px(24.0)).object_fit(gpui_kit::ObjectFit::Contain),
            )
            .into_any_element()
        }
        None if !emoji.is_empty() && !emoji.starts_with('<') => {
            base.text_size(px(20.0)).child(emoji.to_owned()).into_any_element()
        }
        None => base.child(icon("hash").size(px(18.0))).into_any_element(),
    }
}
