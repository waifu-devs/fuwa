//! The menu under your name at the bottom of the sidebar, like the web app's
//! `UserPanel.tsx` and `AccountSwitcher.tsx`: your status (with what each
//! means), your custom status, whether you share what you're doing, and the
//! accounts kept on this instance, to switch between or add one.

use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, Div, FontWeight, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Rgba,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px, rgb,
};

use crate::core::i18n::{Arg, t, t_with};
use crate::pb;
use crate::ui::app::FuwaApp;
use crate::ui::motion;
use crate::ui::theme::{Palette, alpha, radius_md, radius_sm};
use crate::ui::widgets::{avatar, icon, pal};

/// The statuses you can pick, and the hint under each.
const CHOICES: [(pb::PresenceStatus, &str); 4] = [
    (pb::PresenceStatus::Online, ""),
    (pb::PresenceStatus::Idle, "workspace.userPanel.hint.idle"),
    (pb::PresenceStatus::DoNotDisturb, "workspace.userPanel.hint.dnd"),
    (pb::PresenceStatus::Invisible, "workspace.userPanel.hint.invisible"),
];

/// A status's name, as the web's `STATUS_LABEL`.
pub fn status_name(status: pb::PresenceStatus) -> String {
    t(match status {
        pb::PresenceStatus::Idle => "workspace.presence.status.idle",
        pb::PresenceStatus::DoNotDisturb => "workspace.presence.status.dnd",
        pb::PresenceStatus::Invisible => "workspace.presence.status.invisible",
        pb::PresenceStatus::Offline => "workspace.presence.status.offline",
        _ => "workspace.presence.status.online",
    })
}

/// The web's `.presence-dot` (0.7rem): green, an amber moon, red with a bar,
/// or a grey ring for offline and invisible. `ring` draws the web's
/// `ring-[3px]` around it in `under`, the color it sits on, which is also
/// what the moon and the bar are cut from.
pub fn presence_dot(status: pb::PresenceStatus, size: f32, ring: f32, under: Hsla, p: &Palette) -> Div {
    let dot = div().relative().size(px(size)).flex_none().rounded_full();
    let dot = match status {
        pb::PresenceStatus::Idle => dot.bg(rgb(0xf5a524)).child(
            // radial-gradient(circle at 22% 22%, transparent 34%, …): a bite from the top left.
            div()
                .absolute()
                .left(px(size * (0.22 - 0.375)))
                .top(px(size * (0.22 - 0.375)))
                .size(px(size * 0.75))
                .rounded_full()
                .bg(under),
        ),
        pb::PresenceStatus::DoNotDisturb => dot.bg(rgb(0xf04848)).child(
            div()
                .absolute()
                .left(px(size * 0.22))
                .right(px(size * 0.22))
                .top(px(size * 0.42))
                .h(px(size * 0.16))
                .rounded_full()
                .bg(p.card),
        ),
        pb::PresenceStatus::Invisible | pb::PresenceStatus::Offline => {
            dot.border(px(2.56)).border_color(alpha(p.muted_foreground, 0.7))
        }
        _ => dot.bg(rgb(0x3ecf8e)),
    };
    if ring <= 0.0 {
        return dot;
    }
    div().flex_none().p(px(ring)).rounded_full().bg(under).child(dot)
}

/// Where a status's dot sits: what you picked, idle from the instance when it says so.
fn shown_status(picked: pb::PresenceStatus) -> pb::PresenceStatus {
    match picked {
        pb::PresenceStatus::Unspecified => pb::PresenceStatus::Online,
        other => other,
    }
}

impl FuwaApp {
    /// The menu over your name, for the instance `key`.
    pub(crate) fn status_menu(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let popover: Rgba = p.card;
        let (me, settings) =
            self.core.shared.read(|s| s.instance(key).map(|i| (i.me.clone(), i.presence.clone()))).unwrap_or_default();
        let now = crate::core::dms::now_ms();
        let status = me.as_ref().and_then(|m| crate::ui::presence::custom_status(m, now));
        let picked = settings.as_ref().map(|s| shown_status(s.status())).unwrap_or(pb::PresenceStatus::Online);
        let hover_id = |id: &str| format!("um|{id}");
        let lit = |this: &Self, id: &str| this.hovered.as_deref() == Some(hover_id(id).as_str());
        let mut list = div().flex().flex_col();
        let mut n = 0usize;
        let mut rise = |el: AnyElement, list: Div| -> Div {
            let at = n;
            n += 1;
            list.child(motion::rise(
                div().child(el),
                SharedString::from(format!("um-in|{at}")),
                Duration::from_millis(12 * at.min(10) as u64),
                4.0,
            ))
        };

        if settings.is_some() {
            for (status, hint) in CHOICES {
                let id = format!("status-{}", status as i32);
                let on = lit(self, &id);
                let key = key.to_owned();
                let row = self
                    .menu_row(&id, on, &p, cx)
                    .items_start()
                    .gap(px(10.0))
                    .py(px(8.0))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu = None;
                        if status != picked {
                            let (core, key) = (this.core.clone(), key.clone());
                            this.run(cx, async move { core.set_status(&key, status).await }, |this, result, cx| {
                                if let Err(err) = result {
                                    this.toast(
                                        "circle-alert",
                                        t_with(
                                            "workspace.userPanel.statusFailed",
                                            &[("error", Arg::Str(&err.message))],
                                        ),
                                        String::new(),
                                        None,
                                        None,
                                        cx,
                                    );
                                }
                            });
                        }
                        cx.notify();
                    }))
                    .child(div().mt(px(4.0)).child(presence_dot(status, 11.2, 0.0, popover.into(), &p)))
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().font_weight(FontWeight::BOLD).child(status_name(status)))
                            .when(!hint.is_empty(), |el| {
                                el.child(
                                    div()
                                        .text_size(px(12.0))
                                        .line_height(px(16.0))
                                        .text_color(p.muted_foreground)
                                        .child(t(hint)),
                                )
                            }),
                    )
                    .when(picked == status, |el| {
                        el.child(div().ml_auto().self_center().size(px(6.0)).flex_none().rounded_full().bg(p.primary))
                    });
                list = rise(row.into_any_element(), list);
            }
            list = list.child(separator(&p));
        }

        // Your custom status: set or edit it in your profile, or clear it here.
        {
            let k = key.to_owned();
            let label =
                if status.is_some() { t("workspace.userPanel.editStatus") } else { t("workspace.userPanel.setStatus") };
            let row = self.icon_row("edit-status", "pencil", label, &p, cx).on_click(cx.listener(
                move |this, _, window, cx| {
                    this.menu = None;
                    this.open_profile_settings(&k, window, cx);
                },
            ));
            list = rise(row.into_any_element(), list);
        }
        if status.is_some() {
            let k = key.to_owned();
            let row = self.icon_row("clear-status", "x", t("workspace.userPanel.clearStatus"), &p, cx).on_click(
                cx.listener(move |this, _, _, cx| {
                    this.menu = None;
                    let (core, key) = (this.core.clone(), k.clone());
                    let patch =
                        crate::core::account::ProfilePatch { status: Some(String::new()), ..Default::default() };
                    this.run(cx, async move { core.update_profile(&key, patch).await }, |this, result, cx| {
                        if result.is_err() {
                            this.toast(
                                "circle-alert",
                                t("workspace.userPanel.clearFailed"),
                                String::new(),
                                None,
                                None,
                                cx,
                            );
                        }
                    });
                    cx.notify();
                }),
            );
            list = rise(row.into_any_element(), list);
        }
        if let Some(settings) = &settings {
            let k = key.to_owned();
            let sharing = settings.show_activity;
            let row = self
                .icon_row(
                    "sharing",
                    if sharing { "eye" } else { "eye-off" },
                    if sharing { t("workspace.userPanel.sharing") } else { t("workspace.userPanel.notSharing") },
                    &p,
                    cx,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.menu = None;
                    this.open_friend_settings(&k, window, cx);
                }));
            list = rise(row.into_any_element(), list);
        }

        // The accounts kept here: the one in use with a check, the others to switch to.
        list = list.child(separator(&p)).child(
            div()
                .px(px(8.0))
                .py(px(6.0))
                .text_size(px(12.0))
                .line_height(px(16.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(p.muted_foreground)
                .child(t("connect.accounts.title")),
        );
        let me_id = me.as_ref().map(|m| m.id.clone()).unwrap_or_default();
        let streamer = self.prefs.streamer_mode;
        for account in self.core.kept_accounts(key).into_iter().filter(|a| !a.user_id.is_empty()) {
            let active = account.user_id == me_id;
            // The one in use draws from what the instance says now.
            let user = if active { me.clone().unwrap_or_default() } else { crate::core::accounts::user_of(&account) };
            let name = crate::core::store::user_name(&user);
            let id = format!("acct-{}", account.user_id);
            let on = lit(self, &id);
            let (k, uid) = (key.to_owned(), account.user_id.clone());
            let face_scale =
                motion::follow(SharedString::from(format!("um|{id}|scale")), if on { 1.1 } else { 1.0 }, window, cx);
            let row = self
                .menu_row(&id, on, &p, cx)
                .gap(px(10.0))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.menu = None;
                    if !active && this.core.switch_account(&k, &uid) {
                        // The page open may be a server the other account isn't in.
                        this.navigate(crate::ui::app::Nav::Instance { key: k.clone() }, window, cx);
                    }
                    cx.notify();
                }))
                .child(
                    div()
                        .size(px(28.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .scale(face_scale)
                        .child(avatar(Some(&user), 28.0, &p)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().truncate().font_weight(FontWeight::BOLD).child(name))
                        .child(
                            div()
                                .truncate()
                                .text_size(px(12.0))
                                .line_height(px(16.0))
                                .text_color(p.muted_foreground)
                                .child(if streamer {
                                    format!("@{}", crate::core::accounts::mask_name(&account.username))
                                } else {
                                    format!("@{}", account.username)
                                }),
                        ),
                )
                .when(active, |el| el.child(icon("check").size(px(16.0)).text_color(p.primary)));
            list = rise(row.into_any_element(), list);
        }
        {
            let k = key.to_owned();
            let row = self.icon_row("add-account", "user-plus", t("connect.accounts.add"), &p, cx).on_click(
                cx.listener(move |this, _, window, cx| {
                    this.menu = None;
                    this.add_account(&k, window, cx);
                }),
            );
            list = rise(row.into_any_element(), list);
        }

        div()
            .id("menu-status-away")
            .absolute()
            .inset_0()
            .occlude()
            .on_click(cx.listener(|this, _, _, cx| {
                this.menu = None;
                cx.notify();
            }))
            .child(
                div()
                    .id("menu-status")
                    .absolute()
                    // The web's side="top" align="start", 4px over the button under your name.
                    .bottom(px(56.0))
                    .left(px(crate::ui::rail::RAIL + 8.0))
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(motion::rise(
                        div()
                            .w(px(256.0))
                            .p(px(4.0))
                            .rounded(radius_md())
                            .border_1()
                            .border_color(p.border)
                            .bg(popover)
                            .shadow_md()
                            .text_size(px(14.0))
                            .line_height(px(20.0))
                            .text_color(p.foreground)
                            .child(list),
                        "menu-status",
                        Duration::ZERO,
                        8.0,
                    )),
            )
            .into_any_element()
    }

    /// One of the menu's lines: the web's `DropdownMenuItem` (px-2 py-1.5, a
    /// rounded-sm highlight in the accent color under the pointer).
    fn menu_row(&self, id: &str, on: bool, p: &Palette, cx: &mut Context<Self>) -> gpui_kit::Stateful<Div> {
        let hover_key = format!("um|{id}");
        div()
            .id(SharedString::from(hover_key.clone()))
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(8.0))
            .py(px(6.0))
            .rounded(radius_sm())
            .cursor_pointer()
            .when(on, |el| el.bg(p.accent))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.hovered = Some(hover_key.clone());
                } else if this.hovered.as_deref() == Some(hover_key.as_str()) {
                    this.hovered = None;
                }
                cx.notify();
            }))
    }

    /// A line with a 16px icon in the muted color, then its words.
    fn icon_row(
        &self,
        id: &str,
        glyph: &str,
        label: String,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<Div> {
        let on = self.hovered.as_deref() == Some(format!("um|{id}").as_str());
        self.menu_row(id, on, p, cx).child(icon(glyph).size(px(16.0)).text_color(p.muted_foreground)).child(label)
    }

    /// Settings, on your profile for one instance (where the custom status is set).
    pub(crate) fn open_profile_settings(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_settings(window, cx);
        if let Some(view) = &self.settings {
            let key = key.to_owned();
            view.update(cx, |view, cx| {
                view.page = crate::ui::settings::Page::Profile;
                view.account.key = Some(key);
                cx.notify();
            });
        }
    }

    /// Signs in to one more account on this instance; it becomes the one in use, and the others stay kept.
    pub(crate) fn add_account(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let found = self.core.shared.read(|s| s.instance(key).map(|i| (i.url.clone(), i.name())));
        let Some((url, name)) = found else { return };
        self.open_connect(true, window, cx);
        if let Some(view) = &self.connect {
            view.update(cx, |view, cx| view.add_account(&url, &name, window, cx));
        }
    }
}

/// The web's `DropdownMenuSeparator`: a line across the whole menu, 4px each side.
fn separator(p: &Palette) -> Div {
    div().mx(px(-4.0)).my(px(4.0)).h(px(1.0)).bg(p.border)
}
