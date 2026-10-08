//! The audit log, the web's `settings/server/AuditLog.tsx`: everything people
//! did with their permissions, newest first, narrowed by who and what from
//! two menus; an entry opens to show its reason and each change.

use gpui_kit::{Div, Stateful};

use super::menu::Item;
use super::*;
use crate::ui::settings_controls::{Look, button};
use crate::ui::theme::{radius_2xl, radius_xl};

/// An entry's look: its icon, its tint (background, text), and its name in the "What" menu.
pub(super) fn kind(action: A, p: &Palette) -> (&'static str, Hsla, Hsla, &'static str) {
    let tw = |c: u32| -> (Hsla, Hsla) {
        let fg: Hsla = gpui_kit::rgb(c).into();
        (fg.opacity(0.15), fg)
    };
    let sky = tw(0x0ea5e9);
    let green = tw(0x10b981);
    let violet = tw(0x8b5cf6);
    let amber = tw(0xf59e0b);
    let orange = tw(0xf97316);
    let pink = tw(0xec4899);
    let lime: (Hsla, Hsla) = (Hsla::from(gpui_kit::rgb(0x84cc16)).opacity(0.15), gpui_kit::rgb(0x65a30d).into());
    let red: (Hsla, Hsla) = (alpha(p.destructive, 0.15), p.destructive.into());
    let muted: (Hsla, Hsla) = (p.muted.into(), p.muted_foreground.into());
    let (glyph, (bg, fg), key) = match action {
        A::Unspecified => ("scroll-text", muted, "anything"),
        A::ServerUpdate => ("settings", sky, "serverUpdate"),
        A::ChannelCreate => ("folder-plus", green, "channelCreate"),
        A::ChannelUpdate => ("hash", sky, "channelUpdate"),
        A::ChannelDelete => ("trash", red, "channelDelete"),
        A::ChannelsReorder => ("arrow-down-up", sky, "channelsReorder"),
        A::ChannelPermissionsUpdate => ("lock", sky, "channelPermissions"),
        A::RoleCreate => ("shield-plus", green, "roleCreate"),
        A::RoleUpdate => ("shield", violet, "roleUpdate"),
        A::RoleDelete => ("shield-x", red, "roleDelete"),
        A::RolesReorder => ("arrow-down-up", violet, "rolesReorder"),
        A::MemberRolesUpdate => ("user-cog", violet, "memberRoles"),
        A::MemberUpdate => ("user-cog", violet, "memberUpdate"),
        A::MemberTimeOut => ("hourglass", amber, "timeOut"),
        A::MemberKick => ("door-open", orange, "kick"),
        A::MemberBan => ("gavel", red, "ban"),
        A::MemberUnban => ("undo", green, "unban"),
        A::MessageDelete => ("message-square-x", red, "messageDelete"),
        A::OwnershipTransfer => ("crown", amber, "ownership"),
        A::InviteCreate => ("link", green, "inviteCreate"),
        A::InviteDelete => ("link-2-off", red, "inviteDelete"),
        A::ApplicationApprove => ("user-check", green, "applicationApprove"),
        A::ApplicationReject => ("user-x", red, "applicationReject"),
        A::JoinFormUpdate => ("clipboard-list", sky, "joinForm"),
        A::WelcomeScreenUpdate => ("party-popper", pink, "welcome"),
        A::AutoModRuleCreate => ("shield-check", green, "automodCreate"),
        A::AutoModRuleUpdate => ("shield-alert", sky, "automodUpdate"),
        A::AutoModRuleDelete => ("shield-x", red, "automodDelete"),
        A::AutoModTimeOut => ("bot", amber, "automodTimeOut"),
        A::AutoModMessageDelete => ("bot", red, "automodTakedown"),
        A::EmojiCreate => ("face-slightly-smiling-plus", green, "emojiCreate"),
        A::EmojiUpdate => ("face-slightly-smiling", sky, "emojiUpdate"),
        A::EmojiDelete => ("face-slightly-frowning", red, "emojiDelete"),
        A::ProfileItemCreate => ("sparkles", green, "profileItemCreate"),
        A::ProfileItemUpdate => ("sparkles", sky, "profileItemUpdate"),
        A::ProfileItemDelete => ("trash", red, "profileItemDelete"),
        A::WebhookCreate => ("webhook", green, "webhookCreate"),
        A::WebhookUpdate => ("webhook", sky, "webhookUpdate"),
        A::WebhookDelete => ("unplug", red, "webhookDelete"),
        A::AgentAdd => ("bot", violet, "agentAdd"),
        A::ShareCodeCreate => ("key-round", green, "shareCodeCreate"),
        A::ShareCodeDelete => ("key-square", red, "shareCodeDelete"),
        A::SharedChannelRequest => ("send", sky, "sharedRequest"),
        A::SharedChannelApprove => ("handshake", green, "sharedApprove"),
        A::SharedChannelDisconnect => ("unplug", red, "sharedDisconnect"),
        A::SharedChannelUpdate => ("sliders-horizontal", sky, "sharedUpdate"),
        A::SharedChannelBlock => ("ban", orange, "sharedBlock"),
        A::SharedChannelUnblock => ("undo", green, "sharedUnblock"),
        A::ThreadLock => ("lock", amber, "threadLock"),
        A::ThreadUnlock => ("lock-open", green, "threadUnlock"),
        A::ThreadDelete => ("messages-square", red, "threadDelete"),
        A::PollEnd => ("chart-column", amber, "pollEnd"),
        A::OnboardingUpdate => ("party-popper", pink, "onboarding"),
        A::MessagePin => ("pin", sky, "messagePin"),
        A::MessageUnpin => ("pin-off", muted, "messageUnpin"),
        A::LiveTileEnd => ("radio-tower", lime, "liveTileEnd"),
    };
    (glyph, bg, fg, key)
}

fn kind_label(action: A, p: &Palette) -> String {
    t(&format!("serversettings.audit.kind.{}", kind(action, p).3))
}

impl ServerSettingsView {
    /// A filter's button (`h-10 rounded-xl border px-3 text-sm`: its label, the value, a chevron).
    fn audit_filter(&self, id: &str, label: String, value: String, p: &Palette) -> Stateful<Div> {
        let open = self.menu_open(id);
        let hover = alpha(p.primary, 0.4);
        div()
            .id(SharedString::from(id.to_owned()))
            .h(px(40.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .rounded(radius_xl())
            .border_1()
            .px(px(12.0))
            .text_sm()
            .cursor_pointer()
            .map(|el| {
                if open {
                    el.border_color(alpha(p.primary, 0.6))
                } else {
                    el.border_color(p.border).hover(move |s| s.border_color(hover))
                }
            })
            .child(div().text_color(p.muted_foreground).child(label))
            .child(div().max_w(px(160.0)).truncate().font_weight(FontWeight::BOLD).child(value))
            .child(icon(if open { "chevron-up" } else { "chevron-down" }).size(px(16.0)).text_color(p.muted_foreground))
    }

    fn set_audit_filter(&mut self, actor: Option<String>, action: Option<A>, cx: &mut Context<Self>) {
        if let Some(a) = actor {
            self.pages.people.audit_actor = a;
        }
        if let Some(a) = action {
            self.audit_action = a;
        }
        self.audit_open = None;
        self.load_audit(false, cx);
    }

    pub(super) fn audit_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (channels, members, owner) = self.core.shared.read(|s| {
            let i = s.instance(&self.key);
            (
                i.and_then(|i| i.channels.get(&self.server).cloned()).unwrap_or_default(),
                i.and_then(|i| i.members.get(&self.server).cloned()).unwrap_or_default(),
                i.and_then(|i| i.server(&self.server)).map(|s| s.owner_id.clone()).unwrap_or_default(),
            )
        });
        // The people who can show up as actors: the owner, anyone with a role, and anyone seen doing something.
        let mut actors: Vec<pb::User> = members
            .iter()
            .filter(|m| !m.role_ids.is_empty() || m.user.as_ref().is_some_and(|u| u.id == owner))
            .filter_map(|m| m.user.clone())
            .collect();
        for e in self.audit.iter().flatten() {
            if let Some(u) = self.audit_people.get(&e.actor_id)
                && !actors.iter().any(|a| a.id == u.id)
            {
                actors.push(u.clone());
            }
        }
        let actor = self.pages.people.audit_actor.clone();
        let actor_label = if actor.is_empty() {
            t("serversettings.audit.anyone")
        } else {
            self.audit_people
                .get(&actor)
                .or_else(|| actors.iter().find(|u| u.id == actor))
                .map(user_name)
                .unwrap_or_else(|| t("common.someone"))
        };
        let mut who = vec![
            Item::action(t("serversettings.audit.anyone"), None, |this, _, cx| {
                this.set_audit_filter(Some(String::new()), None, cx)
            })
            .radio(actor.is_empty()),
        ];
        for u in &actors {
            let id = u.id.clone();
            who.push(
                Item::action(user_name(u), Some(avatar(Some(u), 20.0, p).into_any_element()), move |this, _, cx| {
                    this.set_audit_filter(Some(id.clone()), None, cx)
                })
                .radio(actor == u.id),
            );
        }
        let picked = self.audit_action;
        let mut what = Vec::new();
        for action in (0..100).filter_map(|n| A::try_from(n).ok()) {
            let (glyph, _, _, _) = kind(action, p);
            what.push(
                Item::action(
                    kind_label(action, p),
                    Some(icon(glyph).size(px(16.0)).text_color(p.muted_foreground).into_any_element()),
                    move |this, _, cx| this.set_audit_filter(None, Some(action), cx),
                )
                .radio(action == picked),
            );
        }
        let filters = div()
            .flex()
            .flex_wrap()
            .gap(px(8.0))
            .child(self.dropdown(
                "audit-by".into(),
                self.audit_filter("audit-by", t("serversettings.audit.by"), actor_label, p),
                who,
                false,
                240.0,
                44.0,
                p,
                cx,
            ))
            .child(self.dropdown(
                "audit-what".into(),
                self.audit_filter("audit-what", t("serversettings.audit.what"), kind_label(picked, p), p),
                what,
                false,
                240.0,
                44.0,
                p,
                cx,
            ));

        let mut col = div().flex().flex_col().gap(px(16.0)).child(filters);
        match &self.audit {
            None if self.error.is_some() => {
                col = col.child(super::pages::problem(self.error.as_deref().unwrap_or_default(), p))
            }
            None => col = col.child(super::pages::shimmers(4, 56.0, radius_2xl(), p, window)),
            Some(entries) if entries.is_empty() => {
                let filtered = !actor.is_empty() || picked != A::Unspecified;
                col = col.child(motion::rise(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(8.0))
                        .py(px(48.0))
                        .child(icon("scroll-text").size(px(32.0)).text_color(p.muted_foreground))
                        .child(div().font_weight(FontWeight::EXTRA_BOLD).child(t("serversettings.audit.empty")))
                        .child(div().text_sm().line_height(px(20.0)).text_color(p.muted_foreground).child(
                            if filtered {
                                t("serversettings.audit.noMatch")
                            } else {
                                t("serversettings.audit.emptyHint")
                            },
                        )),
                    SharedString::from(format!("audit-empty-{filtered}")),
                    Duration::ZERO,
                    8.0,
                ))
            }
            Some(entries) => {
                let mut list = div().flex().flex_col().gap(px(6.0));
                for (n, entry) in entries.iter().enumerate() {
                    list = list.child(self.audit_entry(entry, n, &channels, p, cx));
                }
                col = col.child(list);
            }
        }
        if self.audit_more && self.audit.is_some() {
            let loading = self.audit_loading;
            col = col.child(
                div().flex().justify_center().child(
                    button("audit-more", t("serversettings.audit.older"), None, Look::Outline, false, p)
                        .rounded(radius_xl())
                        .when(loading, |el| el.opacity(0.5))
                        .when(!loading, |el| el.on_click(cx.listener(|this, _, _, cx| this.load_audit(true, cx)))),
                ),
            );
        }
        col.into_any_element()
    }

    fn audit_entry(
        &self,
        entry: &pb::AuditEntry,
        n: usize,
        channels: &[pb::Channel],
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let action = entry.action();
        let (glyph, bg, fg, _) = kind(action, p);
        let actor = self.audit_people.get(&entry.actor_id);
        let at = entry.created_at.as_ref().map(|t| t.seconds * 1000).unwrap_or_default();
        let details: Vec<&pb::AuditChange> = entry.changes.iter().filter(|c| c.field != "deleted_messages").collect();
        let expandable = !details.is_empty() || !entry.reason.is_empty();
        let open = expandable && self.audit_open.as_deref() == Some(entry.id.as_str());
        let id = entry.id.clone();
        let text = sentence(entry, &self.audit_people, channels);
        let tint = self.role_tint(entry);
        let group = SharedString::from(format!("audit-g-{}", entry.id));
        let head = div()
            .id(SharedString::from(format!("audit-{}", entry.id)))
            .group(group.clone())
            .flex()
            .items_center()
            .gap(px(12.0))
            .p(px(12.0))
            .when(expandable, |el| el.cursor_pointer())
            .on_click(cx.listener(move |this, _, _, cx| {
                if !expandable {
                    return;
                }
                this.audit_open = if this.audit_open.as_deref() == Some(id.as_str()) { None } else { Some(id.clone()) };
                cx.notify();
            }))
            .child(
                div()
                    .relative()
                    .size(px(36.0))
                    .flex_none()
                    .rounded(radius_xl())
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(bg)
                    .text_color(fg)
                    .child(icon(glyph).size(px(16.0)))
                    .child(
                        div()
                            .absolute()
                            .right(px(-8.0))
                            .bottom(px(-8.0))
                            .rounded_full()
                            .border_2()
                            .border_color(p.background)
                            .child(avatar(actor, 20.0, p)),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().text_sm().line_height(px(20.0)).child(styled(&text, tint.as_ref(), p)))
                    .child(
                        div()
                            .text_xs()
                            .line_height(px(16.0))
                            .text_color(p.muted_foreground)
                            .child(crate::ui::text::when(at)),
                    ),
            )
            .when(expandable, |el| {
                el.child(motion::once(
                    icon("chevron-down").size(px(16.0)).text_color(p.muted_foreground),
                    SharedString::from(format!("audit-chev-{}-{open}", entry.id)),
                    Duration::from_millis(300),
                    move |el, t| {
                        let e = 1.0 - (1.0 - t).powi(3);
                        let turn = if open { e } else { 1.0 - e };
                        el.rotate(gpui_kit::radians(turn * std::f32::consts::PI))
                    },
                ))
            });
        let mut item = div()
            .rounded(radius_2xl())
            .border_1()
            .border_color(if open { alpha(p.primary, 0.4) } else { p.border.into() })
            .bg(if open { alpha(p.muted, 0.3) } else { alpha(p.background, 0.4) })
            .overflow_hidden()
            .child(head);
        if open {
            let one_side = match action {
                A::InviteCreate | A::AutoModRuleCreate | A::EmojiCreate | A::WebhookCreate => Some(true),
                A::InviteDelete | A::AutoModRuleDelete | A::EmojiDelete | A::WebhookDelete => Some(false),
                _ => None,
            };
            let mut more = div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .border_t_1()
                .border_color(p.border)
                .pr(px(12.0))
                .py(px(10.0))
                .pl(px(60.0))
                .text_sm()
                .line_height(px(20.0));
            if !entry.reason.is_empty() {
                more = more.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(4.0))
                        .child(div().text_color(p.muted_foreground).child(t("serversettings.audit.reason")))
                        .child(entry.reason.clone()),
                );
            }
            let emerald: Hsla = gpui_kit::rgb(if p.dark { 0x34d399 } else { 0x059669 }).into();
            let emerald_bg: Hsla = Hsla::from(gpui_kit::rgb(0x10b981)).opacity(0.1);
            for (k, change) in details.into_iter().enumerate() {
                let label = field_label(&change.field);
                let before = value(&change.field, &change.before, entry, &self.audit_people, channels);
                let after = value(&change.field, &change.after, entry, &self.audit_people, channels);
                let line = div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.0))
                    .child(div().text_color(p.muted_foreground).child(format!("{label}:")));
                let line = match one_side {
                    Some(after_side) => {
                        line.child(tag(if after_side { after } else { before }).bg(p.muted).text_color(p.foreground))
                    }
                    None => line
                        .child(tag(before).bg(alpha(p.destructive, 0.1)).text_color(p.destructive).line_through())
                        .child(icon("arrow-right").size(px(14.0)).text_color(p.muted_foreground))
                        .child(tag(after).bg(emerald_bg).text_color(emerald)),
                };
                more = more.child(motion::rise(
                    line,
                    SharedString::from(format!("change-{}-{k}", entry.id)),
                    Duration::from_millis(40 * k as u64),
                    4.0,
                ));
            }
            item = item.child(super::pages::slide_in(more, format!("audit-more-{}", entry.id)));
        }
        let delay = n.min(14) as f32 * 0.025;
        motion::once(
            item,
            SharedString::from(format!("audit-in-{}-{n}", entry.id)),
            Duration::from_secs_f32(0.4 + delay),
            move |el, t| {
                let start = delay / (0.4 + delay);
                let k = ((t - start) / (1.0 - start)).clamp(0.0, 1.0);
                let e = 1.0 - (1.0 - k).powi(3);
                el.opacity(e).relative().left(px(-12.0 * (1.0 - e)))
            },
        )
    }
}

impl ServerSettingsView {
    /// For entries about a role: the names it goes by in the sentence, and its color now.
    fn role_tint(&self, entry: &pb::AuditEntry) -> Option<(Vec<String>, Hsla)> {
        let action = entry.action();
        let change = |f: &str| entry.changes.iter().find(|c| c.field == f);
        let role_id = match action {
            A::MemberRolesUpdate => {
                let given = change("role")?;
                if given.after.is_empty() { given.before.clone() } else { given.after.clone() }
            }
            A::RoleCreate | A::RoleUpdate | A::RoleDelete => entry.target_id.clone(),
            _ => return None,
        };
        let role = self.core.shared.read(|s| {
            s.instance(&self.key)
                .and_then(|i| i.roles.get(&self.server))
                .and_then(|l| l.iter().find(|r| r.id == role_id).cloned())
        })?;
        let color: Hsla = gpui_kit::rgb(role.color? as u32).into();
        let mut names = vec![role.name.clone(), entry.role_name.clone()];
        if let Some(c) = change("name").filter(|_| action == A::RoleUpdate) {
            names.push(c.before.clone());
            names.push(c.after.clone());
        }
        Some((names, color))
    }
}

/// An entry's sentence (Markdown from `sentence`: escapes and `**bold**`) as text with its names in
/// bold, a role's in its color.
fn styled(md: &str, tint: Option<&(Vec<String>, Hsla)>, p: &Palette) -> gpui_kit::StyledText {
    let mut text = String::new();
    let mut runs: Vec<std::ops::Range<usize>> = Vec::new();
    let mut bold_from: Option<usize> = None;
    let mut chars = md.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(n) = chars.next() {
                    text.push(n);
                }
            }
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                match bold_from.take() {
                    Some(from) => runs.push(from..text.len()),
                    None => bold_from = Some(text.len()),
                }
            }
            c => text.push(c),
        }
    }
    let highlights = runs
        .into_iter()
        .map(|r| {
            let name = &text[r.clone()];
            let color = tint.filter(|(names, _)| names.iter().any(|n| !n.is_empty() && n == name)).map(|(_, c)| *c);
            (
                r,
                gpui_kit::HighlightStyle {
                    font_weight: Some(FontWeight::BOLD),
                    color: Some(color.unwrap_or(p.foreground.into())),
                    ..Default::default()
                },
            )
        })
        .collect::<Vec<_>>();
    gpui_kit::StyledText::new(text).with_highlights(highlights)
}

/// A value in an opened entry (`rounded-md px-1.5`).
fn tag(text: String) -> Div {
    div().px(px(6.0)).rounded(crate::ui::theme::radius_md()).child(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_has_a_label() {
        let p = Palette::of(&crate::core::themes::builtins()[1]);
        for action in (0..100).filter_map(|n| A::try_from(n).ok()) {
            let key = format!("serversettings.audit.kind.{}", kind(action, &p).3);
            assert_ne!(t(&key), key, "{action:?}");
        }
    }
}
