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
use crate::ui::theme::{Palette, alpha};
use crate::ui::widgets::{card, error_line, icon, icon_button, labeled, pal, primary_button, soft_button};

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
    pub(crate) fn render_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.dialog.clone()?;
        let p = pal(cx);
        if let Dialog::Profile { key, user_id, server } = &dialog {
            return Some(self.render_profile(key, user_id, server.as_deref(), cx));
        }
        let busy = self.dialog_busy;
        let field = || Input::new(&self.dialog_input).large();
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
                        .rounded(px(12.0))
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
                    .rounded(px(14.0))
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
            Dialog::CreateChannel { key, server, parent, category } => {
                let under = self
                    .core
                    .shared
                    .read(|s| s.instance(key).and_then(|i| i.channel(server, parent)).map(|c| c.name.clone()));
                let kinds = div().flex().gap(px(8.0)).children(
                    [(false, "hash", "Text channel"), (true, "folder", "Category")].into_iter().map(
                        |(cat, glyph, label)| {
                            let on = cat == *category;
                            div()
                                .id(SharedString::from(format!("kind-{label}")))
                                .flex_1()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .px(px(12.0))
                                .h(px(44.0))
                                .rounded(px(12.0))
                                .border_1()
                                .border_color(if on { p.primary } else { p.border })
                                .bg(if on { alpha(p.primary, 0.12) } else { p.secondary.into() })
                                .text_color(if on { p.primary } else { p.foreground })
                                .font_weight(FontWeight::BOLD)
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if let Some(Dialog::CreateChannel { category, parent, .. }) = &mut this.dialog {
                                        *category = cat;
                                        if cat {
                                            parent.clear();
                                        }
                                    }
                                    let hint = if cat { "Cozy corner" } else { "new-channel" };
                                    this.dialog_input.update(cx, |s, cx| s.set_placeholder(hint, window, cx));
                                    cx.notify();
                                }))
                                .child(icon(glyph).size(px(16.0)))
                                .child(label)
                        },
                    ),
                );
                (
                    if *category { "folder-plus" } else { "hash" },
                    if *category { "Make a category".into() } else { "Make a channel".into() },
                    match under {
                        Some(name) if !*category => format!("It goes in {name}."),
                        _ if *category => "Categories group channels together in the sidebar.".into(),
                        _ => "A place to talk about one thing.".into(),
                    },
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(14.0))
                        .when(parent.is_empty(), |el| el.child(kinds))
                        .child(labeled(if *category { "Category name" } else { "Channel name" }, field(), &p))
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
                                    .rounded(px(12.0))
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
            Dialog::Profile { .. } => unreachable!("drawn on its own"),
        };
        let danger = matches!(dialog, Dialog::LeaveServer { .. });
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
                            .rounded(px(14.0))
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
                    .child(
                        soft_button("dialog-cancel", "Cancel", &p)
                            .on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))),
                    )
                    .when_some(action, |el, label| {
                        let button = primary_button("dialog-ok", label, &p)
                            .when(danger, |el| el.bg(p.destructive))
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
            div().flex().flex_col().child(div().text_xl().font_weight(FontWeight::EXTRA_BOLD).child(name)).child(
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
                    .rounded(px(12.0))
                    .bg(p.secondary)
                    .text_sm()
                    .child(icon("message-circle-heart").size(px(14.0)).text_color(p.primary))
                    .child(status),
            );
        }
        if let Some(bio) = profile.as_ref().map(|pr| pr.bio.clone()).filter(|b| !b.is_empty()) {
            info = info.child(
                div().flex().flex_col().gap(px(4.0)).child(section_title("About", &p)).child(
                    gpui_kit::component::text::TextView::markdown(
                        "profile-bio",
                        crate::ui::text::images_as_links(&bio),
                    )
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
        if !me {
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
            .child(div().h(px(96.0)).rounded_t(px(20.0)).bg(accent))
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
                        .rounded(px(12.0))
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
